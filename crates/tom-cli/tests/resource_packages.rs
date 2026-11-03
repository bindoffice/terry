//! End-to-end resource-package installs.
//!
//! These drive the real code paths, not stubs:
//!
//! * **npm** — a local HTTP server serves a registry document and a real `.tgz`
//!   built in the test, so extraction, integrity verification and manifest
//!   reading are all exercised. `TOM_NPM_REGISTRY` points at it, which is also
//!   the documented mirror knob.
//! * **git** — a real `git init` repository with one commit, so the clone and the
//!   resolved commit hash are real. Skipped when `git` is unavailable.
//! * **resources** — after install, the package's skills are visible through
//!   `resource_dirs`, which is the only thing that makes an install useful.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use tom_cli::resource_packages::install::{install, package_dir, remove};
use tom_cli::resource_packages::{
    read_manifest, resource_dirs, ManagedMarker, PackageEnv, PackageRegistry, PackageSource,
};

/// Build a gzip-compressed tarball with a single top-level `package/` directory,
/// which is npm's layout.
fn npm_tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("package/{path}"), body.as_bytes())
            .unwrap();
    }
    let uncompressed = builder.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&uncompressed).unwrap();
    encoder.finish().unwrap()
}

/// A one-shot HTTP server that answers the registry document and the tarball.
fn serve_npm(package: &str, version: &str, tarball: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_thread = Arc::clone(&requests);

    let encoded_name = package.replace('@', "%40").replace('/', "%2F");
    let document = format!(
        r#"{{"name":"{package}","versions":{{"{version}":{{"version":"{version}","dist":{{"tarball":"{base}/tarballs/{encoded_name}-{version}.tgz"}}}}}}}}"#
    );
    let document_route = format!("/{encoded_name}");
    let tarball_route = format!("/tarballs/{encoded_name}-{version}.tgz");

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            // Drain headers.
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            requests_thread.lock().unwrap().push(path.clone());
            let (content_type, body): (&str, Vec<u8>) = if path == document_route {
                ("application/json", document.clone().into_bytes())
            } else if path == tarball_route {
                ("application/octet-stream", tarball.clone())
            } else {
                ("text/plain", b"not found".to_vec())
            };
            let status = if path == document_route || path == tarball_route {
                "200 OK"
            } else {
                "404 Not Found"
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (base, requests)
}

fn skill_body(name: &str) -> String {
    format!("---\nname: {name}\ndescription: a packaged skill\n---\nbody of {name}\n")
}

#[test]
fn an_npm_package_installs_verifies_and_its_skills_become_visible() {
    let tarball = npm_tarball(&[
        ("package.json", r#"{"name":"demo-pack","version":"1.2.0"}"#),
        ("skills/alpha/SKILL.md", &skill_body("alpha")),
        ("skills/beta/SKILL.md", &skill_body("beta")),
    ]);
    let (base, requests) = serve_npm("demo-pack", "1.2.0", tarball);

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let env = PackageEnv {
        vars: BTreeMap::new(),
        npm_registry: Some(base),
    };
    let source = PackageSource::parse("npm:demo-pack").unwrap();
    let resolved = install(&source, &root, "demo-pack", false, &env).expect("install");

    assert_eq!(resolved.version, "1.2.0");
    assert_eq!(
        resolved.name, "demo-pack",
        "the manifest's name is recorded, not the directory name"
    );
    let installed = package_dir(&root, "demo-pack");
    assert!(installed.join("skills/alpha/SKILL.md").is_file());
    assert!(installed.join("skills/beta/SKILL.md").is_file());

    // The tarball's `package/` prefix is stripped, not nested a level down.
    assert!(!installed.join("package").exists());

    let requests = requests.lock().unwrap().clone();
    assert!(requests.iter().any(|path| path.contains("demo-pack")));
    assert!(requests.iter().any(|path| path.contains("tarballs")));

    // The package contributes both skills through the ordinary resource path.
    let manifest = read_manifest(&installed).unwrap();
    let skills = resource_dirs(&installed, "skills", &manifest);
    assert_eq!(skills, vec![installed.join("skills")]);

    // `install` is the mechanism; recording the install is the command's job,
    // so do what `tom package add` does and then assert the record.
    let mut registry = PackageRegistry::load(&root).unwrap();
    registry.upsert(tom_cli::resource_packages::InstalledPackage {
        name: "demo-pack".into(),
        version: resolved.version.clone(),
        source: source.canonical(),
        integrity: resolved.integrity.clone(),
        commit: resolved.commit.clone(),
    });
    registry.save(&root).unwrap();
    let registry = PackageRegistry::load(&root).unwrap();
    let record = registry.get("demo-pack").expect("registered");
    assert_eq!(record.version, "1.2.0");
    assert_eq!(record.source, "npm:demo-pack");
}

#[test]
fn a_corrupt_tarball_is_refused_and_nothing_lands() {
    // Reuse the working server helper, then break the integrity assertion by
    // publishing a value the served bytes cannot match. `serve_npm` builds the
    // document from the same tarball bytes, so the digests are computed here and
    // only the published value is wrong.
    let tarball = npm_tarball(&[("package.json", r#"{"name":"x"}"#)]);
    // `serve_npm` publishes no integrity; patch the recorded document instead by
    // pointing at a server that does.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let wrong = "A".repeat(86);
    let document = format!(
        "{{\"name\":\"x\",\"versions\":{{\"1.0.0\":{{\"version\":\"1.0.0\",\"dist\":{{\"tarball\":\"{base}/x.tgz\",\"integrity\":\"sha512-{wrong}==\"}}}}}}}}"
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (content_type, body): (&str, Vec<u8>) = if path.ends_with("x.tgz") {
                ("application/octet-stream", tarball.clone())
            } else {
                ("application/json", document.clone().into_bytes())
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let env = PackageEnv {
        vars: BTreeMap::new(),
        npm_registry: Some(base),
    };
    let error = install(
        &PackageSource::parse("npm:x@1.0.0").unwrap(),
        &root,
        "x",
        false,
        &env,
    )
    .unwrap_err();
    assert!(error.contains("integrity"), "{error}");
    assert!(!package_dir(&root, "x").exists(), "nothing may land");
    assert!(
        !root.join(".x.incoming").exists(),
        "the staging directory is cleaned up"
    );
}

#[test]
fn a_local_package_is_installed_and_contributing_its_declared_paths() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("src");
    std::fs::create_dir_all(source_dir.join("resources/skills/one")).unwrap();
    std::fs::write(
        source_dir.join("resources/skills/one/SKILL.md"),
        skill_body("one"),
    )
    .unwrap();
    std::fs::write(
        source_dir.join("tom-package.json"),
        r#"{"name":"local-pack","version":"0.1.0","skills":["./resources/skills"]}"#,
    )
    .unwrap();

    let root = dir.path().join("packages");
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
    let resolved = install(&source, &root, "local-pack", false, &PackageEnv::default()).unwrap();
    assert_eq!(resolved.version, "0.1.0");

    let installed = package_dir(&root, "local-pack");
    let manifest = read_manifest(&installed).unwrap();
    assert_eq!(
        resource_dirs(&installed, "skills", &manifest),
        vec![installed.join("resources/skills")],
        "the manifest path is used, not the conventional skills/ directory"
    );
}

#[test]
fn a_git_package_installs_with_its_commit_recorded() {
    let available = Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        eprintln!("skipping: git is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("skills/from-git")).unwrap();
    std::fs::write(
        repo.join("skills/from-git/SKILL.md"),
        skill_body("from-git"),
    )
    .unwrap();
    std::fs::write(
        repo.join("tom-package.json"),
        r#"{"name":"git-pack","version":"2.0.0"}"#,
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    let root = dir.path().join("packages");
    let source = PackageSource::Git {
        url: repo.to_string_lossy().into_owned(),
        reference: None,
    };
    let resolved =
        install(&source, &root, "git-pack", false, &PackageEnv::default()).expect("clone installs");
    assert_eq!(resolved.version, "2.0.0");
    let commit = resolved.commit.expect("a commit hash is recorded");
    assert_eq!(commit.len(), 40, "a full hash: {commit}");

    let installed = package_dir(&root, "git-pack");
    assert!(installed.join("skills/from-git/SKILL.md").is_file());
    assert!(
        !installed.join(".git").exists(),
        "the clone's metadata is not part of the package"
    );
    let marker = ManagedMarker::read(&installed).unwrap();
    assert_eq!(marker.commit.as_deref(), Some(commit.as_str()));
}

#[test]
fn installing_over_a_hand_written_directory_is_refused_then_forced() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let target = root.join("mine");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("notes.md"), "hand written").unwrap();

    let source_dir = dir.path().join("src");
    std::fs::create_dir_all(source_dir.join("skills")).unwrap();
    std::fs::write(
        source_dir.join("tom-package.json"),
        r#"{"name":"mine","version":"1.0.0"}"#,
    )
    .unwrap();
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();

    let error = install(&source, &root, "mine", false, &PackageEnv::default()).unwrap_err();
    assert!(error.contains("not a package tom installed"), "{error}");
    assert_eq!(
        std::fs::read_to_string(target.join("notes.md")).unwrap(),
        "hand written",
        "the refusal must leave the directory untouched"
    );

    install(&source, &root, "mine", true, &PackageEnv::default()).expect("--force replaces");
    assert!(!target.join("notes.md").exists());
    assert!(ManagedMarker::read(&target).is_some());
}

#[test]
fn reinstalling_the_same_source_is_an_upgrade_not_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let source_dir = dir.path().join("src");
    std::fs::create_dir_all(source_dir.join("skills")).unwrap();
    let write_manifest = |version: &str| {
        std::fs::write(
            source_dir.join("tom-package.json"),
            format!(r#"{{"name":"p","version":"{version}"}}"#),
        )
        .unwrap();
    };

    write_manifest("1.0.0");
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
    install(&source, &root, "p", false, &PackageEnv::default()).expect("first install");

    write_manifest("1.1.0");
    let upgraded = install(&source, &root, "p", false, &PackageEnv::default())
        .expect("same source upgrades without --force");
    assert_eq!(upgraded.version, "1.1.0");
    assert_eq!(
        ManagedMarker::read(&package_dir(&root, "p"))
            .unwrap()
            .version,
        "1.1.0"
    );
}

#[test]
fn remove_takes_the_tree_and_a_registry_without_it_stays_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let source_dir = dir.path().join("src");
    std::fs::create_dir_all(source_dir.join("skills")).unwrap();
    std::fs::write(
        source_dir.join("tom-package.json"),
        r#"{"name":"gone","version":"1.0.0"}"#,
    )
    .unwrap();
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
    install(&source, &root, "gone", false, &PackageEnv::default()).unwrap();

    let mut registry = PackageRegistry::load(&root).unwrap();
    registry.upsert(tom_cli::resource_packages::InstalledPackage {
        name: "gone".into(),
        version: "1.0.0".into(),
        source: source.canonical(),
        integrity: None,
        commit: None,
    });
    registry.save(&root).unwrap();

    remove(&root, "gone", &source.canonical()).expect("removed");
    assert!(!package_dir(&root, "gone").exists());
    registry.packages.retain(|package| package.name != "gone");
    registry.save(&root).unwrap();
    assert!(PackageRegistry::load(&root).unwrap().packages.is_empty());
}

#[test]
fn an_installed_package_lands_in_the_resource_search_path() {
    // The join that makes an install useful: install into a project's packages
    // root, then resolve the skills directory the harness will load.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let source_dir = project.join("src");
    std::fs::create_dir_all(source_dir.join("skills/demo")).unwrap();
    std::fs::write(source_dir.join("skills/demo/SKILL.md"), skill_body("demo")).unwrap();
    std::fs::write(
        source_dir.join("tom-package.json"),
        r#"{"name":"visible","version":"1.0.0"}"#,
    )
    .unwrap();

    let root = project.join(".tom/packages");
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
    install(&source, &root, "visible", false, &PackageEnv::default()).unwrap();
    let mut registry = PackageRegistry::load(&root).unwrap();
    registry.upsert(tom_cli::resource_packages::InstalledPackage {
        name: "visible".into(),
        version: "1.0.0".into(),
        source: source.canonical(),
        integrity: None,
        commit: None,
    });
    registry.save(&root).unwrap();

    let dirs = tom_cli::resource_dirs::project_skill_dirs(project);
    assert!(
        dirs.contains(&root.join("visible/skills")),
        "the package's skills directory must be in the project search path: {dirs:?}"
    );
}

#[test]
fn a_package_theme_directory_is_discovered() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let source_dir = project.join("src");
    std::fs::create_dir_all(source_dir.join("themes")).unwrap();
    std::fs::write(source_dir.join("themes/ocean.json"), r#"{"name":"ocean"}"#).unwrap();
    std::fs::write(
        source_dir.join("tom-package.json"),
        r#"{"name":"themed","version":"1.0.0"}"#,
    )
    .unwrap();
    let root = project.join(".tom/packages");
    let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
    install(&source, &root, "themed", false, &PackageEnv::default()).unwrap();
    let mut registry = PackageRegistry::load(&root).unwrap();
    registry.upsert(tom_cli::resource_packages::InstalledPackage {
        name: "themed".into(),
        version: "1.0.0".into(),
        source: source.canonical(),
        integrity: None,
        commit: None,
    });
    registry.save(&root).unwrap();

    let themes = tom_cli::resource_dirs::package_theme_dirs(project, true);
    assert!(
        themes.contains(&root.join("themed/themes")),
        "package themes are discoverable: {themes:?}"
    );
}

/// Helper so the tests above can share the same request-log type.
type Mutex<T> = std::sync::Mutex<T>;

#[allow(dead_code)]
fn unused(_: PathBuf, _: &Path) {}
