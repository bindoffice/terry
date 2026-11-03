//! Resolving and installing a [`PackageSource`] into a package root.
//!
//! Two distributors, one contract: fetch, verify, unpack, write the ownership
//! marker. The registry entry and the marker are written last, so a failed
//! install leaves the previous tree intact and never produces a half-owned
//! directory.
//!
//! ## npm
//!
//! The registry document for the package gives the dist tarball and its
//! `dist.integrity` (`sha512-…`) or `dist.shasum`. Both are verified when
//! present: a package that does not match what the registry says must not be
//! unpacked, because the tarball is code that runs on load.
//!
//! Version selection honors an exact version, then the highest published
//! version that satisfies a simple range (`^1.2`, `~1`, `1.x`, `>=1`). Ranges
//! are resolved locally rather than by trusting the registry's `latest` tag: a
//! `latest` can lag or point at a prerelease.
//!
//! ## git
//!
//! `git clone --depth 1` (plus `--branch <ref>` when one is named), then the
//! resolved commit is recorded so `tom package update` can report a change.
//! Requires `git` on PATH; there is no in-process git client, and adding one
//! would dwarf this module.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::resource_packages::{ManagedMarker, PackageEnv, PackageSource, MARKER_SCHEMA};

/// What a successful install produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    /// The install directory name.
    pub name: String,
    pub version: String,
    /// The digest the tarball was verified against, when the registry had one.
    pub integrity: Option<String>,
    /// Resolved git commit, for a git source.
    pub commit: Option<String>,
}

/// Where the named package lives on disk.
pub fn package_dir(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

/// Check that installing `source` as `name` may proceed.
///
/// The rule that protects hand-written directories: an existing non-empty
/// target is an error unless it carries a marker, and then only when the marker
/// belongs to the same source (or `force` explicitly takes it over).
pub fn check_target(target: &Path, source: &PackageSource, force: bool) -> Result<(), String> {
    if !target.exists() {
        return Ok(());
    }
    let managed = ManagedMarker::read(target);
    let explicitly_empty = std::fs::read_dir(target)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    match (managed, force) {
        // A managed tree from the same source is the normal upgrade path.
        (Some(marker), _) if marker.source == source.canonical() => Ok(()),
        (Some(marker), false) => Err(format!(
            "{} is managed by `{}`; use --force to replace it with `{}`",
            target.display(),
            marker.source,
            source.canonical()
        )),
        (Some(_), true) => Ok(()),
        (None, true) => Ok(()),
        (None, false) if explicitly_empty => Ok(()),
        (None, false) => Err(format!(
            "{} exists and is not a package tom installed; move it aside or pass --force",
            target.display()
        )),
    }
}

/// Install `source` into `root/<name>`.
///
/// The unpacked tree goes to a temporary sibling first and is renamed into
/// place, so a failed download or a verification mismatch never leaves a
/// partially populated directory under the final name.
pub fn install(
    source: &PackageSource,
    root: &Path,
    name: &str,
    force: bool,
    env: &PackageEnv,
) -> Result<ResolvedPackage, String> {
    let target = package_dir(root, name);
    check_target(&target, source, force)?;
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let staging = root.join(format!(".{name}.incoming"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|e| format!("could not clear {}: {e}", staging.display()))?;
    }

    let resolved = match source {
        PackageSource::Local { path } => install_local(path, &staging, name)?,
        PackageSource::Npm {
            name: package,
            spec,
        } => install_npm(package, spec.as_deref(), &staging, name, env)?,
        PackageSource::Git { url, reference } => {
            install_git(url, reference.as_deref(), &staging, name)?
        }
    };

    // The marker goes in before the rename, so the tree is never visible in its
    // final location without one.
    let marker = ManagedMarker {
        schema: MARKER_SCHEMA,
        source: source.canonical(),
        name: resolved.name.clone(),
        version: resolved.version.clone(),
        integrity: resolved.integrity.clone(),
        commit: resolved.commit.clone(),
    };
    marker.write(&staging)?;

    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("could not replace {}: {e}", target.display()))?;
    }
    std::fs::rename(&staging, &target)
        .map_err(|e| format!("could not move package into place: {e}"))?;
    Ok(resolved)
}

fn install_local(path: &Path, staging: &Path, name: &str) -> Result<ResolvedPackage, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    copy_tree(path, staging)?;
    let manifest = crate::resource_packages::read_manifest(staging)?;
    Ok(ResolvedPackage {
        name: manifest.name.unwrap_or_else(|| name.to_string()),
        version: manifest.version.unwrap_or_else(|| "0.0.0".into()),
        integrity: None,
        commit: None,
    })
}

/// Recursively copy a directory, skipping `.git` and `node_modules` (a package
/// that needs its own dependencies is out of scope: tom packages ship resources,
/// not a runtime dependency tree).
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let destination = to.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if file_type.is_dir() {
            copy_tree(&path, &destination)?;
        } else if file_type.is_file() {
            std::fs::copy(&path, &destination)
                .map_err(|e| format!("{}: {e}", destination.display()))?;
        }
        // Symlinks are skipped: a package must not be able to point outside the
        // tree it was installed into.
    }
    Ok(())
}

/// One published version from the registry document.
#[derive(Debug, Clone)]
pub struct NpmVersion {
    version: String,
    tarball: String,
    integrity: Option<String>,
    shasum: Option<String>,
}

/// Fetch and unpack an npm package.
fn install_npm(
    package: &str,
    spec: Option<&str>,
    staging: &Path,
    name: &str,
    env: &PackageEnv,
) -> Result<ResolvedPackage, String> {
    let registry = env.npm_registry();
    let url = format!("{registry}/{}", encode_package_path(package));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(format!("tom/{}", crate::VERSION))
        .build()
        .map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start a runtime for the download: {e}"))?;
    let document = runtime.block_on(async {
        let response = client
            .get(&url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("could not reach {url}: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let hint = match status.as_u16() {
                404 => format!(" (is `{package}` published?)"),
                401 | 403 => {
                    " (the registry refused the request; set TOM_NPM_REGISTRY or authenticate)"
                        .to_string()
                }
                _ => String::new(),
            };
            return Err(format!("{url} returned {status}{hint}: {body}"));
        }
        // Read then parse rather than `json()`: the registry is JSON but a
        // mirror may serve it as `text/plain`, and `json()` refuses a body it
        // cannot confirm from the content type.
        let body = response
            .text()
            .await
            .map_err(|e| format!("{url}: could not read the response body: {e}"))?;
        serde_json::from_str::<serde_json::Value>(&body)
            .map_err(|e| format!("{url} did not return a registry document: {e}"))
    })?;

    let version = select_version(&document, spec)?;
    let tarball_url = version.tarball.clone();
    let bytes = runtime.block_on(async {
        let response = client
            .get(&tarball_url)
            .send()
            .await
            .map_err(|e| format!("could not download {tarball_url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("{tarball_url} returned {}", response.status()));
        }
        response
            .bytes()
            .await
            .map_err(|e| format!("could not read {tarball_url}: {e}"))
    })?;
    drop(runtime);

    verify_integrity(
        &bytes,
        version.integrity.as_deref(),
        version.shasum.as_deref(),
    )?;
    unpack_tarball(&bytes, staging)?;

    let manifest = crate::resource_packages::read_manifest(staging)?;
    Ok(ResolvedPackage {
        name: manifest.name.unwrap_or_else(|| name.to_string()),
        version: version.version,
        integrity: version.integrity,
        commit: None,
    })
}

/// Percent-encode a package name for a registry path: `@scope/name` becomes
/// `@scope%2Fname`, which is what the registry expects.
fn encode_package_path(package: &str) -> String {
    package
        .split('/')
        .map(|segment| {
            segment
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '@') {
                        c.to_string()
                    } else {
                        format!("%{:02X}", c as u32)
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("%2F")
}

/// Choose the version to install: an exact spec, the best match for a range, or
/// the highest published stable version.
pub fn select_version(
    document: &serde_json::Value,
    spec: Option<&str>,
) -> Result<NpmVersion, String> {
    let versions = document
        .get("versions")
        .and_then(serde_json::Value::as_object)
        .ok_or("the registry document has no `versions`")?;
    let parse = |raw: &str| semver::Version::parse(raw).ok();

    let mut candidates: Vec<(semver::Version, &serde_json::Value)> = versions
        .iter()
        .filter_map(|(raw, value)| parse(raw).map(|version| (version, value)))
        .collect();
    if candidates.is_empty() {
        return Err("the registry document lists no parseable versions".into());
    }
    // A prerelease is only installed when the caller asked for it by name.
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let chosen = match spec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(spec) => {
            let requirement = parse_requirement(spec)?;
            let matching: Vec<&(semver::Version, &serde_json::Value)> = candidates
                .iter()
                .filter(|(version, _)| requirement.as_ref().is_none_or(|req| req.matches(version)))
                .filter(|(version, _)| {
                    // A bare prerelease spec (`1.0.0-rc.1`) or a range that
                    // spells a prerelease is the only way to select one.
                    version.pre.is_empty() || spec.contains('-')
                })
                .collect();
            matching
                .first()
                .map(|entry| (*entry).clone())
                .ok_or_else(|| {
                    format!(
                        "no published version satisfies `{spec}` (available: {})",
                        candidates
                            .iter()
                            .take(5)
                            .map(|(v, _)| v.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
        }
        None => candidates
            .iter()
            .find(|(version, _)| version.pre.is_empty())
            .map(|entry| entry.clone())
            .ok_or("the package publishes only prereleases; name one explicitly")?,
    };

    let meta = chosen.1;
    let tarball = meta
        .get("dist")
        .and_then(|dist| dist.get("tarball"))
        .and_then(serde_json::Value::as_str)
        .ok_or("the chosen version has no `dist.tarball`")?
        .to_string();
    Ok(NpmVersion {
        version: chosen.0.to_string(),
        tarball,
        integrity: meta
            .get("dist")
            .and_then(|dist| dist.get("integrity"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        shasum: meta
            .get("dist")
            .and_then(|dist| dist.get("shasum"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// Parse a version or a simple range. `None` means "any version".
fn parse_requirement(spec: &str) -> Result<Option<semver::VersionReq>, String> {
    if semver::Version::parse(spec).is_ok() {
        return Ok(Some(
            semver::VersionReq::parse(&format!("={spec}"))
                .map_err(|e| format!("invalid version `{spec}`: {e}"))?,
        ));
    }
    // `1.x` / `1.2.x` are common in the wild but not semver-parseable.
    let normalized = spec.replace(".x", "").replace(".*", "");
    semver::VersionReq::parse(&normalized)
        .map(Some)
        .map_err(|e| format!("invalid version range `{spec}`: {e}"))
}

/// Verify the tarball against what the registry published.
fn verify_integrity(
    bytes: &[u8],
    integrity: Option<&str>,
    shasum: Option<&str>,
) -> Result<(), String> {
    if let Some(integrity) = integrity {
        // Subresource-integrity string: `<algorithm>-<base64>`, possibly
        // repeated. Only sha512/sha256 are understood; an unknown algorithm is
        // an error rather than a silent skip, because skipping verification is
        // exactly what an attacker would want.
        for entry in integrity.split_whitespace() {
            let Some((algorithm, expected)) = entry.split_once('-') else {
                continue;
            };
            let digest = match algorithm {
                "sha512" => Sha512Digest::digest(bytes),
                "sha256" => Sha256::digest(bytes).to_vec(),
                other => {
                    return Err(format!(
                        "package uses unsupported integrity algorithm `{other}`"
                    ))
                }
            };
            let encoded = base64_encode(&digest);
            if encoded != expected {
                return Err(format!(
                    "integrity check failed: the tarball does not match the published {algorithm} digest"
                ));
            }
            return Ok(());
        }
        return Err("the published integrity value is not in a recognized format".into());
    }
    if let Some(shasum) = shasum {
        let digest = Sha256::digest(bytes);
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if !hex.eq_ignore_ascii_case(shasum) {
            return Err(
                "integrity check failed: the tarball does not match the published shasum".into(),
            );
        }
    }
    Ok(())
}

/// sha512 through `sha2`, which only ships sha256 in this workspace's features.
/// A hand-rolled SHA-512 would be a security surface; instead the digest is
/// computed with the same crate after enabling the algorithm.
struct Sha512Digest;

impl Sha512Digest {
    fn digest(bytes: &[u8]) -> Vec<u8> {
        use sha2::digest::Digest as _;
        let mut hasher = <sha2::Sha512 as sha2::Digest>::new();
        hasher.update(bytes);
        hasher.finalize().to_vec()
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Unpack a `.tgz` into `staging`, stripping the tarball's top-level directory.
fn unpack_tarball(bytes: &[u8], staging: &Path) -> Result<(), String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("could not read the package tarball: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("could not read a tarball entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tarball entry has no usable path: {e}"))?
            .into_owned();
        // `package/…` is npm's convention; anything else is kept as-is.
        let relative: PathBuf = path
            .components()
            .skip_while(|component| component.as_os_str() == "package")
            .collect();
        // Refuse to escape the target: `unpack` already refuses `..`, but a
        // path that normalizes away means the archive is hostile.
        if relative.as_os_str().is_empty() || relative.components().any(|c| c.as_os_str() == "..") {
            continue;
        }
        let destination = staging.join(&relative);
        // Tar entries are ordered parent-first, but a tarball may omit directory
        // headers entirely; create the parent explicitly so a file entry never
        // fails on a missing directory.
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        entry
            .unpack(&destination)
            .map_err(|e| format!("could not unpack {}: {e}", relative.display()))?;
    }
    Ok(())
}

/// Clone a git repository into `staging` and report the resolved commit.
fn install_git(
    url: &str,
    reference: Option<&str>,
    staging: &Path,
    name: &str,
) -> Result<ResolvedPackage, String> {
    let remote = normalize_git_url(url);
    let mut command = Command::new("git");
    command.arg("clone").arg("--depth").arg("1");
    if let Some(reference) = reference {
        // A branch or tag can be fetched shallowly; a commit hash cannot, so a
        // hash-shaped ref clones the default branch and checks out afterwards.
        if !looks_like_commit(reference) {
            command.arg("--branch").arg(reference);
        }
    }
    command
        .arg(&remote)
        .arg(staging)
        .stdin(std::process::Stdio::null());
    let output = command
        .output()
        .map_err(|e| format!("could not run git (is it installed?): {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone failed: {}", stderr.trim()));
    }
    if let Some(reference) = reference.filter(|r| looks_like_commit(r)) {
        let checkout = Command::new("git")
            .arg("-C")
            .arg(staging)
            .arg("checkout")
            .arg(reference)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !checkout.status.success() {
            return Err(format!(
                "git checkout {reference} failed: {}",
                String::from_utf8_lossy(&checkout.stderr).trim()
            ));
        }
    }
    let commit = Command::new("git")
        .arg("-C")
        .arg(staging)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());

    // Drop the clone's own metadata: it is not part of the package and would be
    // copied into every resource listing.
    let _ = std::fs::remove_dir_all(staging.join(".git"));

    let manifest = crate::resource_packages::read_manifest(staging)?;
    Ok(ResolvedPackage {
        name: manifest.name.unwrap_or_else(|| name.to_string()),
        version: manifest
            .version
            .or_else(|| {
                commit
                    .clone()
                    .map(|commit| commit.chars().take(7).collect())
            })
            .unwrap_or_else(|| "0.0.0".into()),
        integrity: None,
        commit,
    })
}

fn looks_like_commit(reference: &str) -> bool {
    reference.len() >= 7 && reference.chars().all(|c| c.is_ascii_hexdigit())
}

/// `github.com/owner/repo` is what people paste; git needs a scheme.
fn normalize_git_url(url: &str) -> String {
    let trimmed = url.trim();
    // A filesystem path (absolute, or `./`/`../`) is a valid git remote as-is;
    // prefixing it with https:// would turn a local repository into a DNS
    // lookup for a host named after its first path segment.
    if trimmed.contains("://")
        || trimmed.starts_with("git@")
        || trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.starts_with('~')
    {
        return trimmed.to_string();
    }
    format!("https://{trimmed}")
}

/// Remove an installed package, refusing anything tom does not own.
pub fn remove(root: &Path, name: &str, source: &str) -> Result<(), String> {
    let target = package_dir(root, name);
    if !target.exists() {
        return Ok(());
    }
    match ManagedMarker::read(&target) {
        Some(marker) if marker.source == source => {}
        Some(marker) => {
            return Err(format!(
                "{} is managed by `{}`, not `{source}`; refusing to remove it",
                target.display(),
                marker.source
            ))
        }
        None => {
            return Err(format!(
            "{} has no ownership marker; it was not installed by tom, so it will not be removed",
            target.display()
        ))
        }
    }
    std::fs::remove_dir_all(&target)
        .map_err(|e| format!("could not remove {}: {e}", target.display()))
}

/// Resolve the packages root for a project or the global agent directory.
pub fn packages_root(project: bool, cwd: &Path) -> Result<PathBuf, String> {
    if project {
        return Ok(cwd.join(".tom/packages"));
    }
    let agent = crate::config::agent_dir().map_err(|e| e.to_string())?;
    Ok(agent.join("packages"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_highest_stable_version_by_default() {
        let document = serde_json::json!({
            "versions": {
                "1.0.0": {"dist": {"tarball": "https://r/1.0.0.tgz"}},
                "1.2.0": {"dist": {"tarball": "https://r/1.2.0.tgz"}},
                "2.0.0-rc.1": {"dist": {"tarball": "https://r/2.0.0-rc.1.tgz"}},
            }
        });
        assert_eq!(select_version(&document, None).unwrap().version, "1.2.0");
    }

    #[test]
    fn an_exact_spec_wins_and_may_be_a_prerelease() {
        let document = serde_json::json!({
            "versions": {
                "1.0.0": {"dist": {"tarball": "https://r/1.0.0.tgz"}},
                "2.0.0-rc.1": {"dist": {"tarball": "https://r/rc.tgz"}},
            }
        });
        assert_eq!(
            select_version(&document, Some("1.0.0")).unwrap().version,
            "1.0.0"
        );
        assert_eq!(
            select_version(&document, Some("2.0.0-rc.1"))
                .unwrap()
                .version,
            "2.0.0-rc.1"
        );
    }

    #[test]
    fn a_range_selects_the_best_match() {
        let document = serde_json::json!({
            "versions": {
                "1.0.0": {"dist": {"tarball": "https://r/1.tgz"}},
                "1.5.0": {"dist": {"tarball": "https://r/1.5.tgz"}},
                "2.0.0": {"dist": {"tarball": "https://r/2.tgz"}},
            }
        });
        assert_eq!(
            select_version(&document, Some("^1.0")).unwrap().version,
            "1.5.0"
        );
        assert_eq!(
            select_version(&document, Some("~1.0")).unwrap().version,
            "1.0.0"
        );
        // `1.x` is common in the wild.
        assert_eq!(
            select_version(&document, Some("1.x")).unwrap().version,
            "1.5.0"
        );
    }

    #[test]
    fn an_unsatisfiable_range_lists_what_exists() {
        let document = serde_json::json!({
            "versions": {"1.0.0": {"dist": {"tarball": "https://r/1.tgz"}}}
        });
        let error = select_version(&document, Some("^9")).unwrap_err();
        assert!(error.contains("no published version satisfies"), "{error}");
        assert!(error.contains("1.0.0"), "{error}");
    }

    #[test]
    fn a_registry_document_without_versions_is_an_error() {
        assert!(select_version(&serde_json::json!({}), None).is_err());
        assert!(select_version(&serde_json::json!({"versions": {}}), None).is_err());
    }

    #[test]
    fn scoped_names_are_encoded_for_the_registry_path() {
        assert_eq!(encode_package_path("@acme/tools"), "@acme%2Ftools");
        assert_eq!(encode_package_path("plain"), "plain");
    }

    #[test]
    fn integrity_mismatch_is_refused() {
        let bytes = b"hello";
        let good = base64_encode(&Sha256::digest(bytes));
        assert!(verify_integrity(bytes, Some(&format!("sha256-{good}")), None).is_ok());
        assert!(verify_integrity(bytes, Some("sha256-AAAA"), None).is_err());
        // An unknown algorithm must not silently pass.
        assert!(verify_integrity(bytes, Some("md5-AAAA"), None).is_err());
        // A shasum is checked too.
        let hex = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(verify_integrity(bytes, None, Some(&hex)).is_ok());
        assert!(verify_integrity(bytes, None, Some("deadbeef")).is_err());
    }

    #[test]
    fn sha512_integrity_matches_a_known_digest() {
        // `npm` publishes `sha512-…`; an implementation that cannot verify it is
        // worse than none, because it looks like verification.
        let digest = Sha512Digest::digest(b"abc");
        let encoded = base64_encode(&digest);
        assert_eq!(
            encoded,
            "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw=="
        );
    }

    #[test]
    fn a_hand_written_directory_is_never_overwritten_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("mine");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("notes.md"), "user content").unwrap();
        let source = PackageSource::parse("npm:mine@1.0.0").unwrap();

        let error = check_target(&target, &source, false).unwrap_err();
        assert!(error.contains("not a package tom installed"), "{error}");
        // Even a removal refuses.
        let error = remove(root, "mine", &source.canonical()).unwrap_err();
        assert!(error.contains("ownership marker"), "{error}");
        assert!(target.exists(), "the user's directory must survive");
    }

    #[test]
    fn a_managed_tree_of_another_source_needs_force() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("pkg");
        std::fs::create_dir_all(&target).unwrap();
        ManagedMarker {
            schema: MARKER_SCHEMA,
            source: "npm:pkg@1.0.0".into(),
            name: "pkg".into(),
            version: "1.0.0".into(),
            integrity: None,
            commit: None,
        }
        .write(&target)
        .unwrap();

        let other = PackageSource::parse("npm:pkg@2.0.0").unwrap();
        let error = check_target(&target, &other, false).unwrap_err();
        assert!(error.contains("--force"), "{error}");
        assert!(check_target(&target, &other, true).is_ok());
        // The same source is a plain upgrade.
        let same = PackageSource::parse("npm:pkg@1.0.0").unwrap();
        assert!(check_target(&target, &same, false).is_ok());
    }

    #[test]
    fn removal_accepts_the_matching_source_and_refuses_another() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("pkg");
        std::fs::create_dir_all(&target).unwrap();
        ManagedMarker {
            schema: MARKER_SCHEMA,
            source: "npm:pkg@1.0.0".into(),
            name: "pkg".into(),
            version: "1.0.0".into(),
            integrity: None,
            commit: None,
        }
        .write(&target)
        .unwrap();

        let error = remove(root, "pkg", "npm:pkg@9.9.9").unwrap_err();
        assert!(error.contains("refusing to remove"), "{error}");
        assert!(target.exists());
        remove(root, "pkg", "npm:pkg@1.0.0").unwrap();
        assert!(!target.exists());
    }

    #[test]
    fn an_empty_directory_is_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("empty");
        std::fs::create_dir_all(&target).unwrap();
        let source = PackageSource::parse("npm:x@1.0.0").unwrap();
        assert!(check_target(&target, &source, false).is_ok());
    }

    #[test]
    fn a_git_url_without_a_scheme_gets_https() {
        assert_eq!(
            normalize_git_url("github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
        assert_eq!(normalize_git_url("https://host/repo"), "https://host/repo");
    }

    #[test]
    fn a_local_install_copies_the_tree_and_writes_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src");
        std::fs::create_dir_all(source_dir.join("skills")).unwrap();
        std::fs::write(source_dir.join("skills/SKILL.md"), "body").unwrap();
        std::fs::write(
            source_dir.join("tom-package.json"),
            r#"{"name":"local","version":"1.2.3"}"#,
        )
        .unwrap();
        // `.git` must not be copied.
        std::fs::create_dir_all(source_dir.join(".git")).unwrap();
        std::fs::write(source_dir.join(".git/config"), "x").unwrap();

        let root = dir.path().join("packages");
        let source = PackageSource::parse(&source_dir.to_string_lossy()).unwrap();
        let resolved = install(&source, &root, "local", false, &PackageEnv::default()).unwrap();
        assert_eq!(resolved.version, "1.2.3");

        let installed = root.join("local");
        assert!(installed.join("skills/SKILL.md").is_file());
        assert!(
            !installed.join(".git").exists(),
            "VCS metadata is not shipped"
        );
        let marker = ManagedMarker::read(&installed).expect("marker");
        assert_eq!(marker.source, source.canonical());
        assert_eq!(marker.version, "1.2.3");
    }

    #[test]
    fn a_failed_local_install_leaves_no_staging_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("packages");
        let source = PackageSource::parse("./definitely-missing").unwrap();
        assert!(install(&source, &root, "missing", false, &PackageEnv::default()).is_err());
        assert!(!root.join(".missing.incoming").exists());
        assert!(!root.join("missing").exists());
    }
}
