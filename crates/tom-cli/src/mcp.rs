//! `tom mcp` — inspect and manage MCP servers, and load their tools into a
//! session.
//!
//! Two entry points share this module:
//!
//! - [`run`] is the shell command (`tom mcp list|add|remove|status`). It never
//!   needs a session, so it works before authentication.
//! - [`load_for_session`] runs during session construction and returns the
//!   tools to merge into the agent's tool set.
//!
//! Configuration comes from `~/.tom/agent/mcp.json` and, in a trusted project,
//! `.tom/mcp.json`. A project entry replaces a user entry of the same name;
//! one that names only `enabled`/`description`/`timeout` overrides just those.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::config;

/// Generic failure exit code, matching the rest of the CLI.
const EXIT_FAILURE: i32 = 1;

/// Project-level MCP configuration file, relative to the project root.
pub const PROJECT_MCP_FILE: &str = ".tom/mcp.json";

/// Parse `tom mcp <sub> [args]`. Returns a process exit code.
///
/// Async because `list` connects to every server; the CLI already runs inside a
/// tokio runtime, so creating one here would panic ("cannot start a runtime from
/// within a runtime").
pub async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("add") => add(&args[1..]),
        Some("remove") | Some("rm") => remove(&args[1..]),
        Some("list") | Some("ls") => list(&args[1..], false).await,
        Some("status") => list(&args[1..], true).await,
        Some("--help" | "-h" | "help") | None => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("error: unknown `tom mcp` subcommand `{other}`");
            print_help();
            crate::app::EXIT_USAGE
        }
    }
}

fn print_help() {
    println!(
        "Usage: tom mcp <subcommand>

Subcommands:
  list                 Show configured servers and whether they connect
  status               Show configured servers without connecting
  add <name> [opts]    Add or replace a server entry
  remove <name>        Remove a server entry

`add` options:
  --command <exe>      stdio server executable
  --arg <value>        stdio argument (repeatable)
  --env K=V            stdio environment entry (repeatable)
  --cwd <dir>          stdio working directory
  --url <url>          streamable HTTP endpoint
  --header 'K: V'      HTTP header (repeatable)
  --description <text> One sentence describing the server
  --timeout <seconds>  Per-request timeout (default 60)
  --local, -l          Write to .tom/mcp.json instead of the user config
  --disabled           Write the entry with `enabled: false`

Files:
  user     ~/.tom/agent/mcp.json
  project  .tom/mcp.json  (only read in a trusted project)"
    );
}

/// Resolve both config files into one merged config, reporting issues.
fn load_config(
    project_root: &Path,
    trusted: bool,
) -> (tom_mcp::McpConfig, Vec<tom_mcp::ConfigIssue>) {
    let mut merged = config::mcp_path()
        .ok()
        .and_then(|path| tom_mcp::McpConfig::load(&path).ok())
        .unwrap_or_default();

    if trusted {
        let project_path = project_root.join(PROJECT_MCP_FILE);
        if let Ok(project) = tom_mcp::McpConfig::load(&project_path) {
            merged.merge(project);
        }
    }

    let source = if trusted {
        tom_mcp::ConfigSource::Project
    } else {
        tom_mcp::ConfigSource::User
    };
    let (_, issues) = merged.resolve(source, project_root);
    (merged, issues)
}

/// Build the MCP tools for a session.
///
/// Returns `None` when no server is configured, so the caller can skip the
/// whole code path (and its startup cost) for the common case of a user who has
/// no `mcp.json`.
pub async fn load_for_session(
    project_root: &Path,
    trusted: bool,
) -> Option<crate::mcp_session::McpSession> {
    let (merged, issues) = load_config(project_root, trusted);
    if merged.mcp_servers.is_empty() {
        return None;
    }

    let source = if trusted {
        tom_mcp::ConfigSource::Project
    } else {
        tom_mcp::ConfigSource::User
    };
    let (servers, _) = merged.resolve(source, project_root);

    for issue in &issues {
        eprintln!("warning: mcp: {issue}");
    }
    if servers.is_empty() {
        return None;
    }

    let loaded = tom_mcp::connect_all(&servers).await;
    for outcome in loaded.outcomes() {
        if let Some(error) = &outcome.error {
            eprintln!(
                "warning: mcp: server `{}` unavailable: {error}",
                outcome.name
            );
        }
    }

    Some(crate::mcp_session::McpSession::new(loaded))
}

async fn list(args: &[String], force_offline: bool) -> i32 {
    let offline = force_offline || args.iter().any(|a| a == "--offline");
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let trusted = config::project_trust_decision(&project_root)
        .ok()
        .flatten()
        .unwrap_or(true);
    let (merged, issues) = load_config(&project_root, trusted);

    if merged.mcp_servers.is_empty() {
        println!("No MCP servers configured.");
        println!("Add one with: tom mcp add <name> --command <exe>");
        return 0;
    }

    for issue in &issues {
        eprintln!("warning: mcp: {issue}");
    }

    let source = if trusted {
        tom_mcp::ConfigSource::Project
    } else {
        tom_mcp::ConfigSource::User
    };
    let (servers, _) = merged.resolve(source, &project_root);

    if offline {
        for server in &servers {
            println!(
                "{} ({}){}",
                server.name,
                server.transport.kind(),
                if server.is_enabled() {
                    ""
                } else {
                    " [disabled]"
                }
            );
        }
        return 0;
    }

    let loaded = tom_mcp::connect_all(&servers).await;

    let mut failed = false;
    for outcome in loaded.outcomes() {
        match &outcome.error {
            None => {
                println!(
                    "{} ({}): {} tool{}",
                    outcome.name,
                    servers
                        .iter()
                        .find(|s| s.name == outcome.name)
                        .map(|s| s.transport.kind())
                        .unwrap_or("?"),
                    outcome.tools.len(),
                    if outcome.tools.len() == 1 { "" } else { "s" }
                );
                for tool in &outcome.tools {
                    println!("  - {} ({})", tool.exposed_name, tool.server_name);
                }
            }
            Some(error) => {
                failed = true;
                println!("{}: unavailable — {error}", outcome.name);
            }
        }
    }

    if failed {
        EXIT_FAILURE
    } else {
        0
    }
}

fn add(args: &[String]) -> i32 {
    let Some(name) = args.first().filter(|a| !a.starts_with('-')) else {
        eprintln!("error: `tom mcp add` needs a server name");
        return crate::app::EXIT_USAGE;
    };
    if !tom_mcp::config::valid_server_name(name) {
        eprintln!("error: server name `{name}` may contain only letters, digits, '_' and '-'");
        return crate::app::EXIT_USAGE;
    }

    let mut command: Option<String> = None;
    let mut url: Option<String> = None;
    let mut cmd_args: Vec<String> = Vec::new();
    let mut env = serde_json::Map::new();
    let mut headers = serde_json::Map::new();
    let mut cwd: Option<String> = None;
    let mut description: Option<String> = None;
    let mut timeout: Option<u64> = None;
    let mut local = false;
    let mut enabled = true;

    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        let mut value = || -> Option<String> {
            index += 1;
            args.get(index).cloned()
        };
        match flag {
            "--command" => match value() {
                Some(v) => command = Some(v),
                None => return missing_value(flag),
            },
            "--arg" => match value() {
                Some(v) => cmd_args.push(v),
                None => return missing_value(flag),
            },
            "--url" => match value() {
                Some(v) => url = Some(v),
                None => return missing_value(flag),
            },
            "--cwd" => match value() {
                Some(v) => cwd = Some(v),
                None => return missing_value(flag),
            },
            "--description" => match value() {
                Some(v) => description = Some(v),
                None => return missing_value(flag),
            },
            "--env" => match value() {
                Some(pair) => match pair.split_once('=') {
                    Some((key, val)) => {
                        env.insert(key.to_string(), json!(val));
                    }
                    None => {
                        eprintln!("error: `--env` expects KEY=VALUE, got `{pair}`");
                        return crate::app::EXIT_USAGE;
                    }
                },
                None => return missing_value(flag),
            },
            "--header" => match value() {
                Some(pair) => match pair.split_once(':') {
                    Some((key, val)) => {
                        headers.insert(key.trim().to_string(), json!(val.trim()));
                    }
                    None => {
                        eprintln!("error: `--header` expects 'Key: Value', got `{pair}`");
                        return crate::app::EXIT_USAGE;
                    }
                },
                None => return missing_value(flag),
            },
            "--timeout" => match value().and_then(|v| v.parse::<u64>().ok()) {
                Some(v) => timeout = Some(v),
                None => {
                    eprintln!("error: `--timeout` expects a number of seconds");
                    return crate::app::EXIT_USAGE;
                }
            },
            "--local" | "-l" => local = true,
            "--disabled" => enabled = false,
            other => {
                eprintln!("error: unknown option `{other}` for `tom mcp add`");
                return crate::app::EXIT_USAGE;
            }
        }
        index += 1;
    }

    if command.is_none() && url.is_none() {
        eprintln!("error: pass `--command <exe>` (stdio) or `--url <url>` (http)");
        return crate::app::EXIT_USAGE;
    }
    if command.is_some() && url.is_some() {
        eprintln!("error: `--command` and `--url` are mutually exclusive");
        return crate::app::EXIT_USAGE;
    }

    let mut entry = serde_json::Map::new();
    if let Some(command) = command {
        entry.insert("command".into(), json!(command));
        if !cmd_args.is_empty() {
            entry.insert("args".into(), json!(cmd_args));
        }
        if !env.is_empty() {
            entry.insert("env".into(), serde_json::Value::Object(env));
        }
        if let Some(cwd) = cwd {
            entry.insert("cwd".into(), json!(cwd));
        }
    } else if let Some(url) = url {
        entry.insert("url".into(), json!(url));
        if !headers.is_empty() {
            entry.insert("headers".into(), serde_json::Value::Object(headers));
        }
    }
    if let Some(description) = description {
        entry.insert("description".into(), json!(description));
    }
    if let Some(timeout) = timeout {
        entry.insert("timeout".into(), json!(timeout));
    }
    if !enabled {
        entry.insert("enabled".into(), json!(false));
    }

    let path = if local {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cwd.join(PROJECT_MCP_FILE)
    } else {
        match config::mcp_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("error: cannot resolve the tom config directory: {e}");
                return EXIT_FAILURE;
            }
        }
    };

    match write_entry(&path, name, entry) {
        Ok(()) => {
            println!("Added MCP server `{name}` to {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_FAILURE
        }
    }
}

/// Insert or replace one server entry, preserving everything else in the file.
fn write_entry(
    path: &Path,
    name: &str,
    entry: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let mut root: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?,
        _ => json!({}),
    };

    if !root.is_object() {
        return Err(format!(
            "{}: expected a JSON object at the top level",
            path.display()
        ));
    }
    let servers = root
        .as_object_mut()
        .expect("checked object")
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        return Err(format!(
            "{}: `mcpServers` must be an object",
            path.display()
        ));
    }
    servers
        .as_object_mut()
        .expect("checked object")
        .insert(name.to_string(), serde_json::Value::Object(entry));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("{text}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

fn remove(args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("error: `tom mcp remove` needs a server name");
        return crate::app::EXIT_USAGE;
    };
    let local = args.iter().any(|a| a == "--local" || a == "-l");
    let path = if local {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cwd.join(PROJECT_MCP_FILE)
    } else {
        match config::mcp_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("error: cannot resolve the tom config directory: {e}");
                return EXIT_FAILURE;
            }
        }
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("error: {} does not exist", path.display());
            return EXIT_FAILURE;
        }
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            return EXIT_FAILURE;
        }
    };
    let mut root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("error: {}: invalid JSON: {e}", path.display());
            return EXIT_FAILURE;
        }
    };

    let removed = root
        .get_mut("mcpServers")
        .and_then(|s| s.as_object_mut())
        .and_then(|s| s.remove(name))
        .is_some();

    if !removed {
        eprintln!("error: no server named `{name}` in {}", path.display());
        return EXIT_FAILURE;
    }

    let text = match serde_json::to_string_pretty(&root) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error: {e}");
            return EXIT_FAILURE;
        }
    };
    match std::fs::write(&path, format!("{text}\n")) {
        Ok(()) => {
            println!("Removed MCP server `{name}` from {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            EXIT_FAILURE
        }
    }
}

fn missing_value(flag: &str) -> i32 {
    eprintln!("error: `{flag}` needs a value");
    crate::app::EXIT_USAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_entry_preserves_unrelated_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"old":{"command":"x"}},"other":true}"#,
        )
        .unwrap();

        let mut entry = serde_json::Map::new();
        entry.insert("command".into(), json!("npx"));
        write_entry(&path, "new", entry).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["other"], json!(true));
        assert_eq!(parsed["mcpServers"]["old"]["command"], json!("x"));
        assert_eq!(parsed["mcpServers"]["new"]["command"], json!("npx"));
    }

    #[test]
    fn write_entry_creates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp.json");
        let mut entry = serde_json::Map::new();
        entry.insert("url".into(), json!("https://e.x/mcp"));
        write_entry(&path, "docs", entry).unwrap();
        assert!(path.exists());
    }
}
