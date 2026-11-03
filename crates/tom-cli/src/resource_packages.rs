//! Resource packages: install `skills` / `prompts` / `themes` / `extensions`
//! as one unit from npm or git, without a Rust toolchain.
//!
//! This is a different thing from [`crate::packages`], which builds a local
//! Cargo crate into a cdylib. Both end up under a package directory, and the
//! ownership marker below is what tells them apart.
//!
//! ## Why a marker file
//!
//! The hard requirement is that uninstalling one package must never remove a
//! file the user wrote by hand. So an installed tree always carries
//! `.tom-managed.json` (source, resolved version, integrity) inside it, and
//! removal only happens for a directory that
//!
//! 1. is named in the registry,
//! 2. exists,
//! 3. still has a marker whose `source` matches the registry entry.
//!
//! A directory the user created by hand has no marker, so it fails (2)/(3) and
//! the command refuses instead of guessing.
//!
//! ## Manifest
//!
//! `tom-package.json` at the package root, or a `tom` key in `package.json`
//! (the shape pi uses, so an existing package needs no rewrite):
//!
//! ```json
//! {
//!   "name": "my-tools",
//!   "pi":  { "skills": ["./resources/skills"], "prompts": ["./resources/prompts/*.md"] },
//!   "tom": { "skills": ["./resources/skills"] }
//! }
//! ```
//!
//! Without a manifest, the conventional directories `extensions/`, `skills/`,
//! `prompts/` and `themes/` are used, which covers the simple case with no
//! configuration at all.

pub mod install;

pub use install::{install, package_dir, packages_root, remove, ResolvedPackage};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Marker file written inside every managed package directory.
pub const MARKER_FILE: &str = ".tom-managed.json";

/// Dedicated manifest name. `package.json`'s `tom` key is also accepted.
pub const MANIFEST_FILE: &str = "tom-package.json";

/// Resource kinds a package can contribute.
pub const RESOURCE_KINDS: &[&str] = &["extensions", "skills", "prompts", "themes"];

/// Where a package came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    /// `npm:<name>[@<version|range>]`
    Npm { name: String, spec: Option<String> },
    /// `git:<url>[#<ref>]`
    Git {
        url: String,
        reference: Option<String>,
    },
    /// A local directory, referenced in place rather than copied.
    Local { path: PathBuf },
}

impl PackageSource {
    /// Parse a user-supplied source string.
    ///
    /// Accepted shapes:
    /// - `npm:@scope/name`, `npm:name@1.2.3`, `npm:name@^1`
    /// - `git:https://host/repo[#ref]`, `git:github.com/owner/repo[#ref]`
    /// - `./relative`, `/absolute`, `~/home` — a local directory
    /// - a bare `https://…` URL is treated as git (the common paste)
    /// - a bare name containing `/` but no scheme is treated as a local path
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("package source is empty".into());
        }
        if let Some(rest) = trimmed.strip_prefix("npm:") {
            let rest = rest.trim();
            if rest.is_empty() {
                return Err("npm source is missing a package name".into());
            }
            let (name, spec) = split_npm_spec(rest)?;
            return Ok(PackageSource::Npm {
                name,
                spec: spec.map(str::to_string),
            });
        }
        if let Some(rest) = trimmed.strip_prefix("git:") {
            let rest = rest.trim();
            if rest.is_empty() {
                return Err("git source is missing a URL".into());
            }
            let (url, reference) = split_git_ref(rest);
            return Ok(PackageSource::Git {
                url: url.to_string(),
                reference: reference.map(str::to_string),
            });
        }
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let (url, reference) = split_git_ref(trimmed);
            return Ok(PackageSource::Git {
                url: url.to_string(),
                reference: reference.map(str::to_string),
            });
        }
        Ok(PackageSource::Local {
            path: PathBuf::from(expand_home(trimmed)),
        })
    }

    /// The canonical string recorded in the registry and the marker.
    pub fn canonical(&self) -> String {
        match self {
            PackageSource::Npm { name, spec } => match spec {
                Some(spec) => format!("npm:{name}@{spec}"),
                None => format!("npm:{name}"),
            },
            PackageSource::Git { url, reference } => match reference {
                Some(reference) => format!("git:{url}#{reference}"),
                None => format!("git:{url}"),
            },
            PackageSource::Local { path } => path.to_string_lossy().into_owned(),
        }
    }

    /// Whether the source is pinned to one immutable version.
    ///
    /// Only a pinned source may record an "already installed, skip" decision:
    /// `npm:foo` and `git:url` both mean "whatever is current", so they are
    /// re-resolved on every install.
    pub fn is_pinned(&self) -> bool {
        match self {
            PackageSource::Npm { spec, .. } => spec.as_deref().is_some_and(is_exact_version),
            PackageSource::Git { reference, .. } => reference
                .as_deref()
                .is_some_and(|r| r.len() >= 7 && r.chars().all(|c| c.is_ascii_hexdigit())),
            PackageSource::Local { .. } => false,
        }
    }
}

/// Split `name` / `@scope/name` from a trailing `@version`, keeping the `@` of
/// a scope.
fn split_npm_spec(input: &str) -> Result<(String, Option<&str>), String> {
    // A bare `@` names nothing; a scope needs `@scope/name`.
    if input == "@" {
        return Err("npm source is missing a package name".into());
    }
    let start = usize::from(input.starts_with('@'));
    match input[start..].find('@') {
        Some(offset) => {
            let at = start + offset;
            let name = input[..at].trim();
            let spec = input[at + 1..].trim();
            if name.is_empty() {
                return Err("npm source is missing a package name".into());
            }
            Ok((name.to_string(), (!spec.is_empty()).then_some(spec)))
        }
        None => Ok((input.to_string(), None)),
    }
}

/// Split `url#ref`. A `#` inside a URL fragment is rare enough that treating it
/// as the ref separator matches how git itself parses a remote ref.
fn split_git_ref(input: &str) -> (&str, Option<&str>) {
    match input.rsplit_once('#') {
        Some((url, reference)) if !url.is_empty() && !reference.is_empty() => {
            (url, Some(reference))
        }
        _ => (input, None),
    }
}

/// A `1.2.3`-shaped version, i.e. one that cannot move.
fn is_exact_version(spec: &str) -> bool {
    let trimmed = spec.trim().trim_start_matches('=').trim();
    !trimmed.is_empty()
        && trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
        && semver::Version::parse(trimmed).is_ok()
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    value.to_string()
}

/// A package's resource declarations, relative to its root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcePaths {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
}

impl ResourcePaths {
    fn kind(&self, kind: &str) -> Option<&Vec<String>> {
        match kind {
            "extensions" => self.extensions.as_ref(),
            "skills" => self.skills.as_ref(),
            "prompts" => self.prompts.as_ref(),
            "themes" => self.themes.as_ref(),
            _ => None,
        }
    }
}

/// A package manifest: `tom-package.json`, or `package.json`'s `tom`/`pi` key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageManifest {
    /// Declared name, when the manifest provides one.
    pub name: Option<String>,
    pub version: Option<String>,
    pub resources: ResourcePaths,
}

/// Read a package manifest from `root`, or return the empty (conventional)
/// manifest when there is none.
///
/// A `tom` key wins over `pi`: a package that ships both is being explicit about
/// which host it targets, and tom should not silently prefer the legacy key.
pub fn read_manifest(root: &Path) -> Result<PackageManifest, String> {
    let dedicated = root.join(MANIFEST_FILE);
    if dedicated.is_file() {
        let text = std::fs::read_to_string(&dedicated)
            .map_err(|e| format!("{}: {e}", dedicated.display()))?;
        return parse_dedicated_manifest(&text, &dedicated);
    }
    let package_json = root.join("package.json");
    if package_json.is_file() {
        let text = std::fs::read_to_string(&package_json)
            .map_err(|e| format!("{}: {e}", package_json.display()))?;
        return parse_package_json_manifest(&text, &package_json);
    }
    Ok(PackageManifest::default())
}

fn parse_dedicated_manifest(text: &str, path: &Path) -> Result<PackageManifest, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Dedicated {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        extensions: Option<Vec<String>>,
        #[serde(default)]
        skills: Option<Vec<String>>,
        #[serde(default)]
        prompts: Option<Vec<String>>,
        #[serde(default)]
        themes: Option<Vec<String>>,
        /// Optional nested object, so one file can mirror `package.json`.
        #[serde(default)]
        tom: Option<ResourcePaths>,
    }
    let parsed: Dedicated =
        serde_json::from_str(text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    let nested = parsed.tom.unwrap_or_default();
    let resources = ResourcePaths {
        extensions: nested.extensions.or(parsed.extensions),
        skills: nested.skills.or(parsed.skills),
        prompts: nested.prompts.or(parsed.prompts),
        themes: nested.themes.or(parsed.themes),
    };
    Ok(PackageManifest {
        name: parsed.name,
        version: parsed.version,
        resources,
    })
}

fn parse_package_json_manifest(text: &str, path: &Path) -> Result<PackageManifest, String> {
    #[derive(Deserialize)]
    struct PackageJson {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        tom: Option<ResourcePaths>,
        #[serde(default)]
        pi: Option<ResourcePaths>,
    }
    let parsed: PackageJson =
        serde_json::from_str(text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    Ok(PackageManifest {
        name: parsed.name,
        version: parsed.version,
        resources: parsed.tom.or(parsed.pi).unwrap_or_default(),
    })
}

/// Resolve the directories a package contributes for one resource kind.
///
/// An explicit manifest path is used as-is when it exists. Glob patterns are
/// matched against the package tree; a pattern that matches nothing yields
/// nothing (a declared-but-missing path is a packaging bug, not a reason to
/// fail the whole install).
///
/// With no declaration for a kind, the conventional `<root>/<kind>` directory is
/// used when it exists.
pub fn resource_dirs(root: &Path, kind: &str, manifest: &PackageManifest) -> Vec<PathBuf> {
    match manifest.resources.kind(kind) {
        Some(list) => list
            .iter()
            .flat_map(|pattern| resolve_pattern(root, pattern))
            .collect(),
        None => {
            let conventional = root.join(kind);
            if conventional.is_dir() {
                vec![conventional]
            } else {
                Vec::new()
            }
        }
    }
}

/// Resolve one manifest entry: an exact path, or a glob.
///
/// Supports the `*` and `**` wildcards, which is what package manifests use in
/// practice (`./resources/prompts/*.md`). Anything else is treated literally.
fn resolve_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let relative = trimmed.trim_start_matches("./");
    let candidate = root.join(relative);
    if !trimmed.contains('*') {
        return if candidate.exists() {
            vec![candidate]
        } else {
            Vec::new()
        };
    }
    let Ok(glob) = globset::GlobBuilder::new(relative)
        .literal_separator(false)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let mut out = Vec::new();
    walk(root, root, &matcher, &mut out, 0);
    out.sort();
    out
}

fn walk(
    root: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    out: &mut Vec<PathBuf>,
    depth: usize,
) {
    // A package tree is small, but a symlink loop is not: bound the walk.
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if matcher.is_match(relative) {
            out.push(path.clone());
        }
        if file_type.is_dir() {
            walk(root, &path, matcher, out, depth + 1);
        }
    }
}

/// The ownership marker written inside an installed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedMarker {
    /// Marker schema version, for a future migration.
    pub schema: u32,
    /// The canonical source this tree came from.
    pub source: String,
    pub name: String,
    pub version: String,
    /// The integrity value the tree was verified against, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    /// Resolved git commit, for a `git:` source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

pub const MARKER_SCHEMA: u32 = 1;

impl ManagedMarker {
    pub fn read(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(root.join(MARKER_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn write(&self, root: &Path) -> Result<(), String> {
        let path = root.join(MARKER_FILE);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// One installed package, as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// The registry file: `<root>/packages.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRegistry {
    #[serde(default)]
    pub packages: Vec<InstalledPackage>,
}

impl PackageRegistry {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("packages.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let path = root.join("packages.json");
        let tmp = root.join(".packages.json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&InstalledPackage> {
        self.packages.iter().find(|package| package.name == name)
    }

    pub fn upsert(&mut self, package: InstalledPackage) {
        self.packages
            .retain(|existing| existing.name != package.name);
        self.packages.push(package);
        self.packages.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// Whether `name` is usable as an install directory: it becomes a path segment,
/// so anything with a separator or traversal is rejected outright.
pub fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
}

/// Derive an install directory name from a source.
///
/// A scoped npm name keeps its scope as a flat prefix (`@acme/tools` →
/// `acme-tools`) so the directory cannot escape the packages root.
pub fn install_dir_name(source: &PackageSource) -> Option<String> {
    let raw = match source {
        PackageSource::Npm { name, .. } => name.trim_start_matches('@').replace('/', "-"),
        PackageSource::Git { url, .. } => {
            let tail = url
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .rsplit(['/', ':'])
                .next()
                .unwrap_or_default();
            tail.to_string()
        }
        PackageSource::Local { path } => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    valid_package_name(&raw).then_some(raw)
}

/// Environment names hoisted out of the process, for tests and for hosts that
/// resolve configuration from their own store.
#[derive(Debug, Clone, Default)]
pub struct PackageEnv {
    /// Extra env-var lookups; falls back to the process environment.
    pub vars: BTreeMap<String, String>,
    /// Registry base URL override (`TOM_NPM_REGISTRY`), for mirrors and tests.
    pub npm_registry: Option<String>,
}

pub const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";

impl PackageEnv {
    pub fn npm_registry(&self) -> String {
        self.npm_registry
            .clone()
            .or_else(|| {
                self.vars
                    .get("TOM_NPM_REGISTRY")
                    .cloned()
                    .or_else(|| std::env::var("TOM_NPM_REGISTRY").ok())
            })
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_NPM_REGISTRY.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_npm_sources_including_scopes_and_ranges() {
        assert_eq!(
            PackageSource::parse("npm:tom-tools").unwrap(),
            PackageSource::Npm {
                name: "tom-tools".into(),
                spec: None
            }
        );
        assert_eq!(
            PackageSource::parse("npm:tom-tools@1.2.3").unwrap(),
            PackageSource::Npm {
                name: "tom-tools".into(),
                spec: Some("1.2.3".into())
            }
        );
        // The scope's `@` is not a version separator.
        assert_eq!(
            PackageSource::parse("npm:@acme/tools@^2").unwrap(),
            PackageSource::Npm {
                name: "@acme/tools".into(),
                spec: Some("^2".into())
            }
        );
        assert_eq!(
            PackageSource::parse("npm:@acme/tools").unwrap(),
            PackageSource::Npm {
                name: "@acme/tools".into(),
                spec: None
            }
        );
    }

    #[test]
    fn rejects_an_empty_npm_source() {
        assert!(PackageSource::parse("npm:").is_err());
        assert!(PackageSource::parse("npm:@").is_err());
    }

    #[test]
    fn parses_git_sources_with_optional_refs() {
        assert_eq!(
            PackageSource::parse("git:https://host/repo.git").unwrap(),
            PackageSource::Git {
                url: "https://host/repo.git".into(),
                reference: None
            }
        );
        assert_eq!(
            PackageSource::parse("git:github.com/owner/repo#v1").unwrap(),
            PackageSource::Git {
                url: "github.com/owner/repo".into(),
                reference: Some("v1".into())
            }
        );
        // A bare https URL is the common paste, and it is a git source.
        assert!(matches!(
            PackageSource::parse("https://github.com/owner/repo").unwrap(),
            PackageSource::Git { .. }
        ));
    }

    #[test]
    fn a_local_path_stays_local() {
        assert_eq!(
            PackageSource::parse("./my-package").unwrap(),
            PackageSource::Local {
                path: PathBuf::from("./my-package")
            }
        );
    }

    #[test]
    fn only_an_immutable_spec_counts_as_pinned() {
        assert!(PackageSource::parse("npm:tools@1.2.3").unwrap().is_pinned());
        // A range can resolve differently tomorrow, so it is not pinned.
        assert!(!PackageSource::parse("npm:tools@^1.2").unwrap().is_pinned());
        assert!(!PackageSource::parse("npm:tools").unwrap().is_pinned());
        assert!(PackageSource::parse("git:https://x/y#deadbeef")
            .unwrap()
            .is_pinned());
        assert!(!PackageSource::parse("git:https://x/y#main")
            .unwrap()
            .is_pinned());
    }

    #[test]
    fn the_canonical_form_round_trips() {
        for input in [
            "npm:tools@1.0.0",
            "npm:@acme/tools",
            "git:https://host/repo#v1",
            "./local",
        ] {
            let source = PackageSource::parse(input).unwrap();
            let canonical = source.canonical();
            assert_eq!(PackageSource::parse(&canonical).unwrap(), source, "{input}");
        }
    }

    #[test]
    fn install_directory_names_cannot_escape_the_root() {
        // A scope becomes a flat prefix, not a nested path.
        assert_eq!(
            install_dir_name(&PackageSource::parse("npm:@acme/tools").unwrap()).as_deref(),
            Some("acme-tools")
        );
        assert_eq!(
            install_dir_name(&PackageSource::parse("git:https://host/owner/repo.git").unwrap())
                .as_deref(),
            Some("repo")
        );
        assert_eq!(
            install_dir_name(&PackageSource::parse("./my-tools").unwrap()).as_deref(),
            Some("my-tools")
        );
        // Path traversal never survives.
        assert!(install_dir_name(&PackageSource::Local {
            path: PathBuf::from("..")
        })
        .is_none());
        assert!(!valid_package_name(".hidden"));
        assert!(!valid_package_name("a/b"));
        assert!(valid_package_name("my-tools_2"));
    }

    #[test]
    fn reads_the_dedicated_manifest_with_flat_or_nested_lists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(MANIFEST_FILE),
            r#"{"name":"p","version":"1.0.0","skills":["./s"],"tom":{"prompts":["./p"]}}"#,
        )
        .unwrap();
        let manifest = read_manifest(dir.path()).unwrap();
        assert_eq!(manifest.name.as_deref(), Some("p"));
        assert_eq!(manifest.resources.skills, Some(vec!["./s".into()]));
        assert_eq!(manifest.resources.prompts, Some(vec!["./p".into()]));
    }

    #[test]
    fn reads_a_package_json_manifest_and_prefers_tom_over_pi() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"p","pi":{"skills":["./pi-skills"]},"tom":{"skills":["./tom-skills"]}}"#,
        )
        .unwrap();
        let manifest = read_manifest(dir.path()).unwrap();
        assert_eq!(manifest.resources.skills, Some(vec!["./tom-skills".into()]));

        // Pi-only packages still work: no rewrite needed to adopt one.
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"p","pi":{"skills":["./pi-skills"]}}"#,
        )
        .unwrap();
        let manifest = read_manifest(dir.path()).unwrap();
        assert_eq!(manifest.resources.skills, Some(vec!["./pi-skills".into()]));
    }

    #[test]
    fn no_manifest_falls_back_to_conventional_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("skills")).unwrap();
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        let manifest = read_manifest(dir.path()).unwrap();
        assert_eq!(manifest.resources, ResourcePaths::default());
        assert_eq!(
            resource_dirs(dir.path(), "skills", &manifest),
            vec![dir.path().join("skills")]
        );
        // A kind with no directory yields nothing rather than a bogus path.
        assert!(resource_dirs(dir.path(), "prompts", &manifest).is_empty());
    }

    #[test]
    fn an_explicit_manifest_path_overrides_the_conventional_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("skills")).unwrap();
        std::fs::create_dir_all(dir.path().join("resources/skills")).unwrap();
        let manifest = PackageManifest {
            resources: ResourcePaths {
                skills: Some(vec!["./resources/skills".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            resource_dirs(dir.path(), "skills", &manifest),
            vec![dir.path().join("resources/skills")],
            "the declared path wins over the conventional directory"
        );
    }

    #[test]
    fn a_glob_pattern_selects_matching_prompt_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources/prompts")).unwrap();
        std::fs::write(dir.path().join("resources/prompts/a.md"), "a").unwrap();
        std::fs::write(dir.path().join("resources/prompts/b.md"), "b").unwrap();
        std::fs::write(dir.path().join("resources/prompts/skip.json"), "{}").unwrap();
        let manifest = PackageManifest {
            resources: ResourcePaths {
                prompts: Some(vec!["./resources/prompts/*.md".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let dirs = resource_dirs(dir.path(), "prompts", &manifest);
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert!(dirs.iter().all(|path| path.extension().unwrap() == "md"));
    }

    #[test]
    fn a_declared_path_that_does_not_exist_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = PackageManifest {
            resources: ResourcePaths {
                skills: Some(vec!["./missing".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(resource_dirs(dir.path(), "skills", &manifest).is_empty());
    }

    #[test]
    fn the_registry_round_trips_and_upserts_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = PackageRegistry::default();
        registry.upsert(InstalledPackage {
            name: "a".into(),
            version: "1.0.0".into(),
            source: "npm:a@1.0.0".into(),
            integrity: None,
            commit: None,
        });
        registry.upsert(InstalledPackage {
            name: "a".into(),
            version: "2.0.0".into(),
            source: "npm:a@2.0.0".into(),
            integrity: None,
            commit: None,
        });
        registry.save(dir.path()).unwrap();
        let loaded = PackageRegistry::load(dir.path()).unwrap();
        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.get("a").unwrap().version, "2.0.0");
    }

    #[test]
    fn a_missing_registry_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(PackageRegistry::load(dir.path())
            .unwrap()
            .packages
            .is_empty());
    }

    #[test]
    fn the_marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let marker = ManagedMarker {
            schema: MARKER_SCHEMA,
            source: "npm:a@1.0.0".into(),
            name: "a".into(),
            version: "1.0.0".into(),
            integrity: Some("sha512-x".into()),
            commit: None,
        };
        marker.write(dir.path()).unwrap();
        assert_eq!(ManagedMarker::read(dir.path()), Some(marker));
        // A directory with no marker is not managed.
        let other = tempfile::tempdir().unwrap();
        assert!(ManagedMarker::read(other.path()).is_none());
    }
}
