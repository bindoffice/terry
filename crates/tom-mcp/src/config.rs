//! `mcp.json` parsing.
//!
//! The on-disk shape matches the other MCP clients (and pi): a top-level
//! `mcpServers` object keyed by server name. A `command` selects the stdio
//! transport, a `url` selects streamable HTTP. Invalid entries are reported and
//! skipped rather than aborting the whole file, because one broken server must
//! not cost the user every other server.
//!
//! Precedence is handled by the caller ([`crate::McpConfig::merge`]): a project
//! entry replaces a user-level entry with the same name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A parsed `mcp.json`. Unknown top-level keys are ignored so a file written
/// for a newer client still loads.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, ServerEntry>,
}

/// One server definition. Field presence decides the transport:
/// `command` ⇒ stdio, `url` ⇒ streamable HTTP.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerEntry {
    /// stdio executable. One executable, not a shell command string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// stdio arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment for the stdio child. Values expand `${VAR}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// stdio working directory. A leading `~/` names the home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// streamable HTTP endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Extra HTTP headers. Values expand `${VAR}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    /// Optional explicit transport. When present it must agree with the fields
    /// above. `sse` is rejected: it is a legacy transport this client does not
    /// implement, and servers that document it usually also serve `/mcp`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,

    /// `false` keeps the entry configured without connecting to it.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// One sentence describing what the server offers. Used in the tool
    /// description prefix so the model can tell servers apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Per-request timeout in seconds. Defaults to [`DEFAULT_TIMEOUT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// How this server's tools reach the model. `deferred` keeps them out of the
    /// declared set until `tool_search` loads them, which is what keeps a large
    /// server (a filesystem server is 10+ tools) from spending context on every
    /// request. Default is `direct` — see [`Exposure::resolve`] for why deferral
    /// is opt-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<Exposure>,
}

/// How a server's tools are offered to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Exposure {
    /// Declared to the model like a built-in tool.
    Direct,
    /// Registered but not declared; `tool_search` can load a match.
    Deferred,
}

impl Exposure {
    /// Effective exposure for a server: an explicit setting wins, otherwise
    /// `direct`.
    ///
    /// `direct` is the default even when a `tool_search` tool exists, because
    /// deferral is a *trade*, not a free win: a deferred tool costs its loader a
    /// search call before the model can use it, which is a bad deal for the small
    /// servers most users configure. A server with ten or more tools (a
    /// filesystem or GitHub server) is where it pays off, and that is a decision
    /// the user makes per server with `"exposure": "deferred"`.
    pub fn resolve(setting: Option<Exposure>, _tool_search_available: bool) -> Exposure {
        setting.unwrap_or(Exposure::Direct)
    }

    pub fn is_deferred(self) -> bool {
        matches!(self, Exposure::Deferred)
    }
}

fn default_enabled() -> bool {
    true
}

pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Where a [`ServerEntry`] came from. Used for diagnostics and for `/mcp`-style
/// reporting; the merge itself is by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    User,
    Project,
}

/// A validated server ready to connect.
#[derive(Debug, Clone)]
pub struct ResolvedServer {
    pub name: String,
    pub transport: Transport,
    pub description: Option<String>,
    pub timeout: std::time::Duration,
    pub source: ConfigSource,
    /// The entry's own `exposure`, before [`Exposure::resolve`] fills in the
    /// host-dependent default.
    pub exposure: Option<Exposure>,
}

impl ResolvedServer {
    pub fn is_enabled(&self) -> bool {
        // Disabled entries are filtered out during resolution; keeping the
        // predicate explicit documents that `Transport` alone cannot encode it.
        true
    }
}

#[derive(Debug, Clone)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl Transport {
    pub fn kind(&self) -> &'static str {
        match self {
            Transport::Stdio { .. } => "stdio",
            Transport::Http { .. } => "http",
        }
    }
}

/// A single problem with one entry. The server is skipped; everything else
/// still loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigIssue {
    pub server: String,
    pub message: String,
}

impl std::fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.server, self.message)
    }
}

impl McpConfig {
    /// Read one `mcp.json`. A missing file is an empty config, not an error.
    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Parse the JSON body. Errors here are syntactic only; per-entry problems
    /// surface later from [`Self::resolve`].
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(trimmed)
    }

    /// Overlay `other` on top of `self`: same-named servers are replaced.
    ///
    /// A project entry that names only `enabled` (or `description`/`timeout`)
    /// and no transport is a *partial override*: it keeps the user-level
    /// command/url/env/headers and only changes the named fields. That is the
    /// documented way to switch one server off inside a project, so treating it
    /// as a full replacement would silently break the server.
    pub fn merge(&mut self, other: McpConfig) {
        for (name, incoming) in other.mcp_servers {
            match self.mcp_servers.get_mut(&name) {
                Some(existing) if incoming.is_partial_override() => {
                    existing.apply_override(incoming);
                }
                _ => {
                    self.mcp_servers.insert(name, incoming);
                }
            }
        }
    }

    /// Validate every entry and return the connectable servers plus the issues
    /// found. Disabled entries are dropped without being reported as issues.
    pub fn resolve(
        &self,
        source: ConfigSource,
        base_cwd: &Path,
    ) -> (Vec<ResolvedServer>, Vec<ConfigIssue>) {
        let mut servers = Vec::new();
        let mut issues = Vec::new();

        for (name, entry) in &self.mcp_servers {
            if !valid_server_name(name) {
                issues.push(ConfigIssue {
                    server: name.clone(),
                    message: "name may contain only letters, digits, '_' and '-'".into(),
                });
                continue;
            }
            if !entry.enabled {
                continue;
            }
            match entry.resolve(name, source, base_cwd) {
                Ok(server) => servers.push(server),
                Err(message) => issues.push(ConfigIssue {
                    server: name.clone(),
                    message,
                }),
            }
        }

        (servers, issues)
    }
}

impl ServerEntry {
    /// True when the entry carries no transport of its own, i.e. it can only be
    /// a delta on an inherited server.
    fn is_partial_override(&self) -> bool {
        self.command.is_none() && self.url.is_none() && self.transport.is_none()
    }

    fn apply_override(&mut self, other: ServerEntry) {
        self.enabled = other.enabled;
        if other.description.is_some() {
            self.description = other.description;
        }
        if other.timeout.is_some() {
            self.timeout = other.timeout;
        }
        if other.exposure.is_some() {
            self.exposure = other.exposure;
        }
    }

    fn resolve(
        &self,
        name: &str,
        source: ConfigSource,
        base_cwd: &Path,
    ) -> Result<ResolvedServer, String> {
        let declared = self.transport.as_deref();
        if let Some(declared) = declared {
            if !matches!(declared, "stdio" | "http" | "streamable-http") {
                return Err(format!(
                    "unsupported transport {declared:?} (expected stdio, http or streamable-http)"
                ));
            }
            if declared == "sse" {
                return Err("the legacy SSE transport is not supported".into());
            }
        }

        let transport = match (&self.command, &self.url) {
            (Some(_), Some(_)) => {
                return Err("set either `command` (stdio) or `url` (http), not both".into())
            }
            (None, None) => {
                return Err("missing `command` (stdio) or `url` (http)".into());
            }
            (Some(command), None) => {
                if matches!(declared, Some("http" | "streamable-http")) {
                    return Err("`type` says http but only `command` is set".into());
                }
                if command.trim().is_empty() {
                    return Err("`command` is empty".into());
                }
                Transport::Stdio {
                    command: expand_home(command),
                    args: self.args.iter().map(|a| expand_home(a)).collect(),
                    env: self
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), expand_vars(v)))
                        .collect(),
                    cwd: self.cwd.as_ref().map(|cwd| base_cwd.join(expand_home(cwd))),
                }
            }
            (None, Some(url)) => {
                if declared == Some("stdio") {
                    return Err("`type` says stdio but only `url` is set".into());
                }
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err("`url` must start with http:// or https://".into());
                }
                Transport::Http {
                    url: url.clone(),
                    headers: self
                        .headers
                        .iter()
                        .map(|(k, v)| (k.clone(), expand_vars(v)))
                        .collect(),
                }
            }
        };

        Ok(ResolvedServer {
            name: name.to_string(),
            transport,
            description: self.description.clone(),
            timeout: std::time::Duration::from_secs(self.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS)),
            source,
            exposure: self.exposure,
        })
    }
}

/// Server names are restricted because they become part of every tool name
/// (`mcp__<server>__<tool>`); arbitrary punctuation would make tool names
/// ambiguous and unparseable by the host.
pub fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    value.to_string()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand `${VAR}` references from the process environment. An unset variable
/// becomes the empty string, matching the shell-ish behavior other clients use;
/// a lone `$` is left alone.
pub fn expand_vars(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                out.push_str(&std::env::var(name).unwrap_or_default());
                rest = &after[end + 1..];
            }
            None => {
                // Unterminated `${`: keep it verbatim.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn parses_stdio_and_http_servers() {
        let config = McpConfig::parse(
            r#"{
              "mcpServers": {
                "filesystem": {
                  "command": "npx",
                  "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
                },
                "docs": {
                  "url": "https://example.com/mcp",
                  "headers": {"Authorization": "Bearer t"}
                }
              }
            }"#,
        )
        .expect("parse");

        let cwd = tempdir();
        let (servers, issues) = config.resolve(ConfigSource::User, cwd.path());
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(servers.len(), 2);

        let filesystem = servers.iter().find(|s| s.name == "filesystem").unwrap();
        match &filesystem.transport {
            Transport::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected stdio, got {other:?}"),
        }

        let docs = servers.iter().find(|s| s.name == "docs").unwrap();
        assert!(matches!(docs.transport, Transport::Http { .. }));
    }

    #[test]
    fn exposure_defaults_to_direct_and_can_be_deferred_per_server() {
        let config = McpConfig::parse(
            r#"{
              "mcpServers": {
                "small": { "command": "tiny-mcp" },
                "huge": { "command": "huge-mcp", "exposure": "deferred" }
              }
            }"#,
        )
        .expect("parse");
        let cwd = tempdir();
        let (servers, issues) = config.resolve(ConfigSource::User, cwd.path());
        assert!(issues.is_empty(), "{issues:?}");

        let small = servers.iter().find(|s| s.name == "small").unwrap();
        assert_eq!(small.exposure, None, "unset stays unset at parse time");
        assert!(
            !Exposure::resolve(small.exposure, true).is_deferred(),
            "deferral is opt-in, even when tool_search exists"
        );

        let huge = servers.iter().find(|s| s.name == "huge").unwrap();
        assert_eq!(huge.exposure, Some(Exposure::Deferred));
        assert!(Exposure::resolve(huge.exposure, true).is_deferred());
        // Without a loader, an explicit `deferred` still resolves to deferred —
        // the host warns rather than silently declaring the tools either way.
        assert!(Exposure::resolve(huge.exposure, false).is_deferred());
    }

    #[test]
    fn an_unknown_exposure_is_a_load_error() {
        // `deny_unknown_fields` plus the enum keep a typo from silently meaning
        // `direct`, which would defeat the point of opting into deferral.
        let error = McpConfig::parse(r#"{"mcpServers":{"x":{"command":"c","exposure":"maybe"}}}"#)
            .expect_err("unknown exposure");
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn a_partial_override_can_flip_exposure() {
        // Same rule as `enabled`: a project entry with no transport is a delta,
        // so it must be able to defer a server defined at user level.
        let mut config =
            McpConfig::parse(r#"{"mcpServers":{"shared":{"command":"mcp"}}}"#).expect("parse");
        config.merge(
            McpConfig::parse(r#"{"mcpServers":{"shared":{"exposure":"deferred"}}}"#)
                .expect("parse"),
        );
        let cwd = tempdir();
        let (servers, issues) = config.resolve(ConfigSource::Project, cwd.path());
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].exposure, Some(Exposure::Deferred));
        assert!(
            matches!(servers[0].transport, Transport::Stdio { .. }),
            "the inherited transport survives the override"
        );
    }

    #[test]
    fn missing_file_is_empty_config() {
        let dir = tempdir();
        let config = McpConfig::load(&dir.path().join("mcp.json")).expect("load");
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn rejects_unknown_transport_and_sse() {
        let config = McpConfig::parse(
            r#"{"mcpServers":{
                 "a": {"command":"x","type":"websocket"},
                 "b": {"url":"https://e.x/sse","type":"sse"}}}"#,
        )
        .unwrap();
        let (servers, issues) = config.resolve(ConfigSource::User, Path::new("."));
        assert!(servers.is_empty());
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn rejects_both_command_and_url() {
        let config =
            McpConfig::parse(r#"{"mcpServers":{"a":{"command":"x","url":"https://e.x"}}}"#)
                .unwrap();
        let (_, issues) = config.resolve(ConfigSource::User, Path::new("."));
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].message.contains("not both"),
            "{}",
            issues[0].message
        );
    }

    #[test]
    fn disabled_entries_are_dropped_silently() {
        let config =
            McpConfig::parse(r#"{"mcpServers":{"a":{"command":"x","enabled":false}}}"#).unwrap();
        let (servers, issues) = config.resolve(ConfigSource::User, Path::new("."));
        assert!(servers.is_empty());
        assert!(issues.is_empty());
    }

    #[test]
    fn partial_project_entry_keeps_inherited_transport() {
        let mut user = McpConfig::parse(
            r#"{"mcpServers":{"internal":{"command":"run","args":["--x"],"env":{"K":"v"}}}}"#,
        )
        .unwrap();
        let project = McpConfig::parse(r#"{"mcpServers":{"internal":{"enabled":false}}}"#).unwrap();
        user.merge(project);

        let entry = user.mcp_servers.get("internal").unwrap();
        assert!(!entry.enabled);
        assert_eq!(entry.command.as_deref(), Some("run"));
        assert_eq!(entry.args, vec!["--x".to_string()]);
        assert_eq!(entry.env.get("K").map(String::as_str), Some("v"));
    }

    #[test]
    fn full_project_entry_replaces_user_entry() {
        let mut user = McpConfig::parse(r#"{"mcpServers":{"s":{"command":"user-cmd"}}}"#).unwrap();
        let project =
            McpConfig::parse(r#"{"mcpServers":{"s":{"command":"project-cmd"}}}"#).unwrap();
        user.merge(project);
        assert_eq!(
            user.mcp_servers.get("s").unwrap().command.as_deref(),
            Some("project-cmd")
        );
    }

    #[test]
    fn expands_variables_and_unset_becomes_empty() {
        std::env::set_var("TOM_MCP_TEST_VAR", "hello");
        assert_eq!(expand_vars("a-${TOM_MCP_TEST_VAR}-b"), "a-hello-b");
        assert_eq!(expand_vars("x-${TOM_MCP_TEST_UNSET_VAR}-y"), "x--y");
        assert_eq!(expand_vars("price $5"), "price $5");
        assert_eq!(expand_vars("open ${UNTERMINATED"), "open ${UNTERMINATED");
        std::env::remove_var("TOM_MCP_TEST_VAR");
    }

    #[test]
    fn validates_server_names() {
        assert!(valid_server_name("filesystem"));
        assert!(valid_server_name("my-server_2"));
        assert!(!valid_server_name(""));
        assert!(!valid_server_name("has space"));
        assert!(!valid_server_name("semi;colon"));
    }

    #[test]
    fn relative_cwd_resolves_against_base() {
        let mut config =
            McpConfig::parse(r#"{"mcpServers":{"a":{"command":"x","cwd":"sub"}}}"#).unwrap();
        let (mut servers, _) = config.resolve(ConfigSource::Project, Path::new("/repo"));
        let server = servers.remove(0);
        match server.transport {
            Transport::Stdio { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some(Path::new("/repo/sub")));
            }
            other => panic!("expected stdio, got {other:?}"),
        }
        config.mcp_servers.clear();
    }
}
