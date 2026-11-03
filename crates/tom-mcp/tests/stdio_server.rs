//! End-to-end coverage against a real stdio MCP server.
//!
//! The server is a small shell script (and a batch file on Windows) that speaks
//! newline-delimited JSON-RPC, so the test exercises the actual child-process
//! path — spawn, handshake, `tools/list`, `tools/call` — rather than a mock
//! transport. A mock would not catch a broken stdin writer or a reader that
//! drops frames, which are the parts most likely to regress.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tom_mcp::{ConfigSource, McpClient, McpConfig};

/// Write a server script and return a [`ResolvedServer`] that runs it.
fn fake_server(dir: &Path, name: &str) -> tom_mcp::ResolvedServer {
    #[cfg(unix)]
    let (command, args, script_name) = {
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo the input","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#;
        let path = dir.join("fake-server.sh");
        std::fs::write(&path, script).expect("write script");
        // The script is invoked through `sh` so the test does not depend on the
        // executable bit surviving a checkout.
        (
            "sh".to_string(),
            vec![path.to_string_lossy().into_owned()],
            "sh",
        )
    };

    #[cfg(windows)]
    let (command, args, script_name) = {
        let script = r#"@echo off
setlocal enabledelayedexpansion
for /l %%i in (1,1,100000) do (
  set "line="
  set /p line=
  if not defined line goto :eof
  echo !line! | findstr /c:"initialize" >nul && echo {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}
)
"#;
        let path = dir.join("fake-server.bat");
        std::fs::write(&path, script).expect("write script");
        (
            "cmd".to_string(),
            vec!["/c".to_string(), path.to_string_lossy().into_owned()],
            "cmd",
        )
    };

    let _ = script_name;
    tom_mcp::ResolvedServer {
        name: name.to_string(),
        transport: tom_mcp::ServerTransport::Stdio {
            command,
            args,
            env: Default::default(),
            cwd: None,
        },
        description: Some("test server".into()),
        timeout: Duration::from_secs(10),
        source: ConfigSource::User,
        exposure: None,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn connects_lists_and_calls_over_stdio() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = fake_server(dir.path(), "fake");

    let client = McpClient::connect(server).await.expect("connect");

    let tools = client.list_tools().await.expect("list tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].server_name, "echo");
    assert_eq!(tools[0].exposed_name, "mcp__fake__echo");
    assert_eq!(tools[0].server, "fake");
    assert_eq!(tools[0].description, "Echo the input");
    assert_eq!(tools[0].input_schema["type"], json!("object"));

    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = client
        .call_tool("echo", json!({"text":"hi"}), &cancel)
        .await
        .expect("call tool");
    assert_eq!(outcome.text, "pong");
    assert!(!outcome.is_error);
}

#[cfg(unix)]
#[tokio::test]
async fn connect_all_reports_a_missing_command_without_failing_others() {
    let dir = tempfile::tempdir().expect("temp dir");

    let good = fake_server(dir.path(), "good");
    let broken = tom_mcp::ResolvedServer {
        name: "broken".into(),
        transport: tom_mcp::ServerTransport::Stdio {
            command: "tom-mcp-definitely-not-a-real-command".into(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
        },
        description: None,
        timeout: Duration::from_secs(5),
        source: ConfigSource::User,
        exposure: None,
    };

    let loaded = tom_mcp::connect_all(&[good, broken]).await;

    // The good server still connected and contributed its tool.
    assert_eq!(loaded.clients().len(), 1);
    assert_eq!(loaded.clients()[0].name(), "good");
    assert_eq!(loaded.tools().len(), 1);
    assert_eq!(loaded.agent_tools().len(), 1);

    // The broken one is reported, not silently dropped.
    let broken = loaded
        .outcomes()
        .iter()
        .find(|o| o.name == "broken")
        .expect("broken outcome");
    assert!(broken.error.is_some(), "{broken:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn config_file_drives_a_real_connection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = fake_server(dir.path(), "fake");
    let (command, args) = match &server.transport {
        tom_mcp::ServerTransport::Stdio { command, args, .. } => (command.clone(), args.clone()),
        other => panic!("expected stdio, got {other:?}"),
    };

    let config_path = dir.path().join("mcp.json");
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "fake": {
                    "command": command,
                    "args": args,
                    "description": "test server"
                }
            }
        }))
        .unwrap(),
    )
    .expect("write config");

    let config = McpConfig::load(&config_path).expect("load config");
    let (servers, issues) = config.resolve(ConfigSource::User, dir.path());
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(servers.len(), 1);

    let loaded = tom_mcp::connect_all(&servers).await;
    assert_eq!(loaded.tools().len(), 1);
    assert!(loaded.summary().contains("fake"));
}

/// The handshake must happen before `tools/list`, and a server that answers
/// `initialize` with an error must not be reported as connected.
#[cfg(unix)]
#[tokio::test]
async fn initialize_failure_is_reported() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("bad.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32600,"message":"nope"}}\n' "$id"
done
"#,
    )
    .expect("write script");

    let server = tom_mcp::ResolvedServer {
        name: "bad".into(),
        transport: tom_mcp::ServerTransport::Stdio {
            command: "sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: Default::default(),
            cwd: None,
        },
        description: None,
        timeout: Duration::from_secs(5),
        source: ConfigSource::User,
        exposure: None,
    };

    let error = McpClient::connect(server).await.expect_err("must fail");
    let message = error.to_string();
    assert!(message.contains("bad"), "{message}");
}

/// A tool that declares no `inputSchema` still has to reach the model as an
/// object schema; providers reject anything else.
#[cfg(unix)]
#[tokio::test]
async fn tool_without_schema_gets_an_object_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("noschema.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"bare"}]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .expect("write script");

    let server = tom_mcp::ResolvedServer {
        name: "bare".into(),
        transport: tom_mcp::ServerTransport::Stdio {
            command: "sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: Default::default(),
            cwd: None,
        },
        description: None,
        timeout: Duration::from_secs(5),
        source: ConfigSource::User,
        exposure: None,
    };

    let client = McpClient::connect(server).await.expect("connect");
    let tools = client.list_tools().await.expect("list");
    assert_eq!(tools.len(), 1);
    let Value::Object(schema) = &tools[0].input_schema else {
        panic!("expected object schema");
    };
    assert_eq!(schema.get("type"), Some(&json!("object")));
    assert!(schema.contains_key("properties"));
}
