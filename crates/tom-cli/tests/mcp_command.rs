//! `tom mcp` shell-command coverage.
//!
//! These run the real binary against a temporary config directory so the file
//! handling, the `kind` disambiguation between "add/list/remove", and the live
//! stdio connection are all exercised together — none of which a unit test of
//! `write_entry` alone would catch.

use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_tom")
}

/// A temp config dir plus a `tom` command bound to it, so the user's real
/// `~/.tom/agent` is never touched.
struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn agent_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("agent")
    }

    fn project_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("project")
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(binary());
        cmd.env("TOM_CODING_AGENT_DIR", self.agent_dir());
        cmd.env("TOM_OFFLINE", "1");
        cmd.current_dir(self.project_dir());
        cmd
    }

    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let output = self.command().args(args).output().expect("run tom");
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code().unwrap_or(-1),
        )
    }

    /// Write a stdio MCP server script that answers the handshake and exposes
    /// one `add` tool.
    fn write_server(&self) -> std::path::PathBuf {
        let path = self.dir.path().join("server.sh");
        std::fs::write(
            &path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"demo","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"add","description":"Add two numbers","inputSchema":{"type":"object","properties":{"a":{"type":"number"}},"required":["a"]}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"sum=42"}]}}\n' "$id" ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
        )
        .expect("write server script");
        path
    }
}

#[test]
fn add_then_list_connects_and_shows_the_tool() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();
    let server = harness.write_server();

    let (stdout, stderr, code) = harness.run(&[
        "mcp",
        "add",
        "demo",
        "--command",
        "/bin/sh",
        "--arg",
        &server.to_string_lossy(),
        "--description",
        "Demo adder",
    ]);
    assert_eq!(code, 0, "add failed: {stdout}{stderr}");
    assert!(stdout.contains("Added MCP server `demo`"), "{stdout}");

    let (stdout, stderr, code) = harness.run(&["mcp", "list"]);
    assert_eq!(code, 0, "list failed: {stdout}{stderr}");
    // The server is really connected: the tool name is the flattened form the
    // model will see.
    assert!(stdout.contains("demo (stdio): 1 tool"), "{stdout}");
    assert!(stdout.contains("mcp__demo__add"), "{stdout}");
}

#[test]
fn status_is_offline_and_needs_no_server() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();
    std::fs::write(
        harness.agent_dir().join("mcp.json"),
        r#"{"mcpServers":{"ghost":{"url":"https://127.0.0.1:1/mcp"}}}"#,
    )
    .unwrap();

    // `status` must not attempt a connection.
    let (stdout, stderr, code) = harness.run(&["mcp", "status"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("ghost (http)"), "{stdout}");
}

#[test]
fn list_reports_a_broken_server_and_exits_nonzero() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();
    std::fs::write(
        harness.agent_dir().join("mcp.json"),
        r#"{"mcpServers":{"broken":{"command":"tom-does-not-exist-xyz"}}}"#,
    )
    .unwrap();

    let (stdout, stderr, code) = harness.run(&["mcp", "list"]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("broken: unavailable"), "{stdout}");
}

#[test]
fn remove_deletes_the_entry_and_preserves_other_keys() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();
    let path = harness.agent_dir().join("mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"keep":{"command":"a"},"drop":{"command":"b"}},"other":1}"#,
    )
    .unwrap();

    let (_, stderr, code) = harness.run(&["mcp", "remove", "drop"]);
    assert_eq!(code, 0, "{stderr}");

    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(parsed["mcpServers"].get("keep").is_some());
    assert!(parsed["mcpServers"].get("drop").is_none());
    assert_eq!(parsed["other"], serde_json::json!(1));
}

#[test]
fn add_rejects_a_name_that_would_break_tool_names() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();

    let (_, stderr, code) = harness.run(&["mcp", "add", "bad name", "--command", "x"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("may contain only"), "{stderr}");
}

#[test]
fn add_requires_exactly_one_transport() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();

    let (_, stderr, code) = harness.run(&["mcp", "add", "s"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("--command <exe>"), "{stderr}");

    let (_, stderr, code) = harness.run(&[
        "mcp",
        "add",
        "s",
        "--command",
        "x",
        "--url",
        "https://e.x/mcp",
    ]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("mutually exclusive"), "{stderr}");
}

#[test]
fn missing_config_file_lists_nothing_without_failing() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();
    std::fs::create_dir_all(harness.agent_dir()).unwrap();

    let (stdout, stderr, code) = harness.run(&["mcp", "list"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("No MCP servers configured"), "{stdout}");
}

#[test]
fn help_is_printed_without_a_config() {
    let harness = Harness::new();
    std::fs::create_dir_all(harness.project_dir()).unwrap();

    let (stdout, _, code) = harness.run(&["mcp", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("tom mcp <subcommand>"), "{stdout}");
}

#[allow(dead_code)]
fn exists(path: &Path) -> bool {
    path.exists()
}
