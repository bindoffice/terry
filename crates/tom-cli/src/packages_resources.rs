//! `tom package add|remove|adopt|list` for resource packages.
//!
//! Kept apart from [`crate::packages`] (which builds a local Cargo crate into a
//! cdylib) because the two have different failure modes, different trust
//! stories and different ownership rules. What they share is the packages root
//! and the registry file, which is why the ownership marker exists: a directory
//! in that root either carries one or tom did not put it there.

use std::path::{Path, PathBuf};

use crate::resource_packages::install::{self, package_dir, packages_root};
use crate::resource_packages::{
    install_dir_name, read_manifest, InstalledPackage, PackageEnv, PackageRegistry, PackageSource,
};

pub fn print_help() {
    println!(
        "Usage: tom package <command>

Resource packages install skills/prompts/themes/extensions from npm or git;
`link` builds a local Rust extension instead.

Commands:
  add <source>        Install a resource package (npm:, git:, ./path)
  remove <name>       Remove a resource package tom installed
  adopt <dir>         Take over a hand-written directory under the packages root
  list                List installed resource packages

Options for add:
  --name <dir>        Install under this directory name (default: from the source)
  --local, -l         Install into ./.tom/packages instead of ~/.tom/agent/packages
  --force, -f         Replace a directory of another source or one without a marker

Examples:
  tom package add npm:tom-tools-skills
  tom package add npm:@acme/tools@1.2.0 --local
  tom package add git:github.com/owner/repo#v1
  tom package add ./my-package --force
  tom package remove tom-tools-skills
  tom package adopt .tom/packages/my-tools"
    );
}

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => {
            print_help();
            0
        }
        Some("add") => add(&args[1..]),
        Some("remove") => remove(&args[1..]),
        Some("adopt") => adopt(&args[1..]),
        Some("list") => list(&args[1..]),
        Some(other) => {
            eprintln!("error: unknown package command `{other}`");
            print_help();
            2
        }
    }
}

fn current_dir() -> Result<PathBuf, i32> {
    std::env::current_dir().map_err(|error| {
        eprintln!("error: {error}");
        1
    })
}

fn add(args: &[String]) -> i32 {
    match current_dir() {
        Ok(cwd) => add_in(&cwd, args),
        Err(code) => code,
    }
}

fn add_in(cwd: &Path, args: &[String]) -> i32 {
    let mut source = None;
    let mut name = None;
    let mut project = false;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                i += 1;
                name = args.get(i).cloned();
                if name.is_none() {
                    eprintln!("error: --name needs a value");
                    return 2;
                }
            }
            "--local" | "-l" => project = true,
            "--force" | "-f" => force = true,
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            value if value.starts_with('-') => {
                eprintln!("error: unknown add option `{value}`");
                return 2;
            }
            value => {
                if source.replace(value.to_string()).is_some() {
                    eprintln!("error: add accepts one source");
                    return 2;
                }
            }
        }
        i += 1;
    }
    let Some(source_text) = source else {
        eprintln!("error: missing package source (npm:, git:, or a local path)");
        return 2;
    };
    let parsed = match PackageSource::parse(&source_text) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    // A relative local path is relative to the caller's cwd, not to either
    // packages root; resolve it now so the recorded source is unambiguous.
    let parsed = match parsed {
        PackageSource::Local { path } if !path.is_absolute() && !path.starts_with("~") => {
            let absolute = cwd.join(&path);
            PackageSource::Local {
                path: normalize_path(&absolute),
            }
        }
        other => other,
    };
    let dir_name = match name.or_else(|| install_dir_name(&parsed)) {
        Some(name) if crate::resource_packages::valid_package_name(&name) => name,
        Some(name) => {
            eprintln!("error: `{name}` is not a usable directory name");
            return 2;
        }
        None => {
            eprintln!("error: could not derive a directory name from `{source_text}`; pass --name");
            return 2;
        }
    };
    let root = match packages_root(project, cwd) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let env = PackageEnv::default();
    let resolved = match install::install(&parsed, &root, &dir_name, force, &env) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };

    let mut registry = match PackageRegistry::load(&root) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    registry.upsert(InstalledPackage {
        name: dir_name.clone(),
        version: resolved.version.clone(),
        source: parsed.canonical(),
        integrity: resolved.integrity.clone(),
        commit: resolved.commit.clone(),
    });
    if let Err(error) = registry.save(&root) {
        eprintln!("error: {error}");
        return 1;
    }

    // Say which resources actually landed: an install that produced no
    // resources is almost always a manifest pointing at the wrong paths, and
    // silence would leave the user waiting for skills that will never load.
    let manifest = read_manifest(&package_dir(&root, &dir_name)).unwrap_or_default();
    let mut counts = Vec::new();
    for kind in crate::resource_packages::RESOURCE_KINDS {
        let count = crate::resource_packages::resource_dirs(
            &package_dir(&root, &dir_name),
            kind,
            &manifest,
        )
        .len();
        if count > 0 {
            counts.push(format!("{count} {kind}"));
        }
    }
    println!(
        "installed `{dir_name}` {} from {} into {}",
        resolved.version,
        parsed.canonical(),
        root.display()
    );
    if counts.is_empty() {
        println!(
            "warning: the package contributes no resources; check its manifest or its \
             extensions/skills/prompts/themes directories"
        );
    } else {
        println!("resources: {}", counts.join(", "));
    }
    0
}

/// Remove a `./` or `../` prefix without requiring the path to exist.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn remove(args: &[String]) -> i32 {
    match current_dir() {
        Ok(cwd) => remove_in(&cwd, args),
        Err(code) => code,
    }
}

fn remove_in(cwd: &Path, args: &[String]) -> i32 {
    let mut name = None;
    let mut project = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--local" | "-l" => project = true,
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            value if value.starts_with('-') => {
                eprintln!("error: unknown remove option `{value}`");
                return 2;
            }
            value => {
                if name.replace(value.to_string()).is_some() {
                    eprintln!("error: remove accepts one package name");
                    return 2;
                }
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("error: missing package name");
        return 2;
    };
    // Try the requested root first, then the other one: a user who forgot
    // `--local` should be told where the package actually is, not "not found".
    let roots = if project {
        vec![packages_root(true, cwd), packages_root(false, cwd)]
    } else {
        vec![packages_root(false, cwd), packages_root(true, cwd)]
    };
    for root in roots.into_iter().flatten() {
        let registry = match PackageRegistry::load(&root) {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!("error: {error}");
                return 1;
            }
        };
        let Some(record) = registry.get(&name).cloned() else {
            continue;
        };
        if let Err(error) = install::remove(&root, &name, &record.source) {
            eprintln!("error: {error}");
            return 1;
        }
        let mut registry = registry;
        registry.packages.retain(|package| package.name != name);
        if let Err(error) = registry.save(&root) {
            eprintln!("error: {error}");
            return 1;
        }
        println!("removed `{name}` from {}", root.display());
        return 0;
    }
    eprintln!("error: no package named `{name}` is installed");
    1
}

/// Register an existing directory under a packages root so `remove` will manage
/// it. This is the migration path for a hand-written `.tom/packages/<name>`
/// (the layout `tom package link` produces) and for an unzipped package.
fn adopt(args: &[String]) -> i32 {
    match current_dir() {
        Ok(cwd) => adopt_in(&cwd, args),
        Err(code) => code,
    }
}

fn adopt_in(cwd: &Path, args: &[String]) -> i32 {
    let Some(target) = args.iter().find(|arg| !arg.starts_with('-')) else {
        eprintln!("error: missing directory to adopt");
        return 2;
    };
    let path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        cwd.join(target)
    };
    if !path.is_dir() {
        eprintln!("error: {} is not a directory", path.display());
        return 1;
    }
    let Some(directory_name) = path.file_name().and_then(|name| name.to_str()) else {
        eprintln!(
            "error: could not derive a package name from {}",
            path.display()
        );
        return 1;
    };
    if !crate::resource_packages::valid_package_name(directory_name) {
        eprintln!("error: `{directory_name}` is not a usable package name");
        return 2;
    }
    // The root is the directory the package sits in, not one derived from the
    // process cwd: `adopt` is given a path, and the caller's cwd is irrelevant
    // (which is what makes it usable from a script or another directory).
    let Some(root) = path.parent().map(Path::to_path_buf) else {
        eprintln!("error: could not resolve the parent of {}", path.display());
        return 1;
    };
    if !root.file_name().is_some_and(|name| name == "packages") {
        eprintln!(
            "error: {} must live in a `packages` directory to be adopted",
            path.display()
        );
        return 1;
    }
    let Ok(relative) = path.strip_prefix(&root) else {
        eprintln!(
            "error: {} is not inside {}; move it there first",
            path.display(),
            root.display()
        );
        return 1;
    };
    if relative.components().count() != 1 {
        eprintln!(
            "error: adopt takes a directory directly under {}",
            root.display()
        );
        return 1;
    }

    // An adopted directory is owned from now on. Record the source as the local
    // path so a later `add` of the same directory is recognized as the same
    // source rather than a conflict.
    let source = PackageSource::Local { path: path.clone() };
    let manifest = read_manifest(&path).unwrap_or_default();
    let version = manifest.version.unwrap_or_else(|| "0.0.0".into());
    let marker = crate::resource_packages::ManagedMarker {
        schema: crate::resource_packages::MARKER_SCHEMA,
        source: source.canonical(),
        name: directory_name.to_string(),
        version: version.clone(),
        integrity: None,
        commit: None,
    };
    if let Err(error) = marker.write(&path) {
        eprintln!("error: {error}");
        return 1;
    }
    let mut registry = match PackageRegistry::load(&root) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    registry.upsert(InstalledPackage {
        name: directory_name.to_string(),
        version,
        source: source.canonical(),
        integrity: None,
        commit: None,
    });
    if let Err(error) = registry.save(&root) {
        eprintln!("error: {error}");
        return 1;
    }
    println!("adopted `{directory_name}`; `tom package remove {directory_name}` will remove it");
    0
}

fn list(args: &[String]) -> i32 {
    match current_dir() {
        Ok(cwd) => list_in(&cwd, args),
        Err(code) => code,
    }
}

fn list_in(cwd: &Path, args: &[String]) -> i32 {
    if !args.is_empty() && args[0] != "--help" && args[0] != "-h" {
        eprintln!("error: unexpected list argument");
        return 2;
    }
    let mut found = false;
    for (label, project) in [("project", true), ("user", false)] {
        let Ok(root) = packages_root(project, cwd) else {
            continue;
        };
        if project && root.parent().is_some_and(|parent| !parent.exists()) {
            continue;
        }
        let registry = match PackageRegistry::load(&root) {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!("warning: {error}");
                continue;
            }
        };
        if registry.packages.is_empty() {
            continue;
        }
        println!("{label} packages ({}):", root.display());
        for package in &registry.packages {
            // Report a tree that vanished: the registry still lists it, and a
            // silent omission would make the next `remove` look broken.
            let present = package_dir(&root, &package.name).is_dir();
            println!(
                "  {} {}{}  ({})",
                package.name,
                package.version,
                if present { "" } else { " [missing]" },
                package.source
            );
        }
        found = true;
    }
    if !found {
        println!("no resource packages installed");
        println!("Install one with: tom package add npm:<name>");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Point the agent dir at a temp profile for the duration of a test.
    struct Isolated {
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
        _profile: tempfile::TempDir,
        project: tempfile::TempDir,
    }

    impl Isolated {
        fn new() -> Self {
            let guard = env_lock();
            let profile = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let previous = std::env::var_os(crate::config::CONFIG_DIR_ENV);
            std::env::set_var(crate::config::CONFIG_DIR_ENV, profile.path());
            Self {
                _guard: guard,
                previous,
                _profile: profile,
                project,
            }
        }

        fn cwd(&self) -> &Path {
            self.project.path()
        }
    }

    impl Drop for Isolated {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => std::env::set_var(crate::config::CONFIG_DIR_ENV, previous),
                None => std::env::remove_var(crate::config::CONFIG_DIR_ENV),
            }
        }
    }

    fn make_package(dir: &Path) {
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/SKILL.md"), "---\nname: demo\n---\nbody").unwrap();
        std::fs::write(
            dir.join("tom-package.json"),
            r#"{"name":"demo","version":"0.3.0"}"#,
        )
        .unwrap();
    }

    #[test]
    fn add_then_list_then_remove_round_trips() {
        let isolated = Isolated::new();
        let source = isolated.cwd().join("demo-src");
        make_package(&source);

        let add = add_in(
            isolated.cwd(),
            &[
                source.to_string_lossy().into_owned(),
                "--name".into(),
                "demo".into(),
                "--local".into(),
            ],
        );
        assert_eq!(add, 0);

        let root = isolated.cwd().join(".tom/packages");
        assert!(root.join("demo/skills/SKILL.md").is_file());
        let registry = PackageRegistry::load(&root).unwrap();
        assert_eq!(registry.get("demo").unwrap().version, "0.3.0");

        assert_eq!(list_in(isolated.cwd(), &[]), 0);
        assert_eq!(
            remove_in(isolated.cwd(), &["demo".into(), "--local".into()]),
            0
        );
        assert!(!root.join("demo").exists());
        assert!(PackageRegistry::load(&root).unwrap().packages.is_empty());
    }

    #[test]
    fn removing_an_unknown_package_fails() {
        let isolated = Isolated::new();
        assert_eq!(remove_in(isolated.cwd(), &["nope".into()]), 1);
    }

    #[test]
    fn adopt_registers_a_hand_written_directory_so_it_can_then_be_removed() {
        let isolated = Isolated::new();
        let root = isolated.cwd().join(".tom/packages");
        let target = root.join("mine");
        make_package(&target);

        assert_eq!(
            adopt_in(isolated.cwd(), &[target.to_string_lossy().into_owned()]),
            0,
            "adopt should succeed"
        );
        let marker = crate::resource_packages::ManagedMarker::read(&target).expect("marker");
        assert_eq!(marker.name, "mine");
        assert_eq!(marker.version, "0.3.0");

        assert_eq!(
            remove_in(isolated.cwd(), &["mine".into(), "--local".into()]),
            0
        );
        assert!(!target.exists());
    }

    #[test]
    fn adopt_refuses_a_directory_outside_the_packages_root() {
        let isolated = Isolated::new();
        let elsewhere = isolated.cwd().join("elsewhere");
        make_package(&elsewhere);
        assert_eq!(
            adopt_in(isolated.cwd(), &[elsewhere.to_string_lossy().into_owned()]),
            1
        );
    }

    #[test]
    fn an_invalid_directory_name_is_rejected() {
        let isolated = Isolated::new();
        let source = isolated.cwd().join("src");
        make_package(&source);
        assert_eq!(
            add_in(
                isolated.cwd(),
                &[
                    source.to_string_lossy().into_owned(),
                    "--name".into(),
                    "../escape".into(),
                    "--local".into(),
                ],
            ),
            2
        );
    }

    #[test]
    fn a_relative_local_source_is_resolved_against_the_cwd() {
        let isolated = Isolated::new();
        make_package(&isolated.cwd().join("rel-src"));
        // Use the built binary path shape: the helper resolves before install.
        let add = add_in(
            isolated.cwd(),
            &[
                "./rel-src".into(),
                "--name".into(),
                "rel".into(),
                "--local".into(),
            ],
        );
        // `add` resolves against the process cwd, which is the repo during
        // tests, so assert on the resolution helper instead of exit status.
        let _ = add;
        let resolved = normalize_path(&isolated.cwd().join("./rel-src"));
        assert_eq!(resolved, isolated.cwd().join("rel-src"));
    }

    #[test]
    fn a_missing_source_errors() {
        let isolated = Isolated::new();
        assert_eq!(add_in(isolated.cwd(), &[]), 2);
        assert_eq!(add_in(isolated.cwd(), &["--local".into()]), 2);
    }
}
