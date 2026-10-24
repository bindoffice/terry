use std::env;
use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct ConnectionInfo {
    port: u16,
    token: String,
}

/// Matches `paths::data_dir()` in the main app: `~/Library/Application
/// Support/Terry` on macOS, `~/.local/share/terry` on Linux, and
/// `%LOCALAPPDATA%\Terry` on Windows.
fn default_ipc_path() -> PathBuf {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let app_dir = if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        "terry"
    } else {
        "Terry"
    };
    base.join(app_dir).join("ipc.json")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let conn_file = args
        .iter()
        .position(|arg| arg == "--connect")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from)
        .or_else(|| env::var_os("TERRY_IPC_PATH").map(PathBuf::from))
        .unwrap_or_else(default_ipc_path);

    if !conn_file.exists() {
        eprintln!(
            "Connection file {} not found. Is Terry running? Pass --connect PATH or set TERRY_IPC_PATH.",
            conn_file.display()
        );
        std::process::exit(1);
    }

    let conn_data = fs::read_to_string(&conn_file)?;
    let conn_info: ConnectionInfo = serde_json::from_str(&conn_data)
        .context("Failed to parse connection info")?;

    eprintln!("Connected to Terry on port {}", conn_info.port);

    // Minimal JSON-RPC MCP loop over Stdio
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("Parse error: {error}") },
                });
                let mut out = serde_json::to_string(&response)?;
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };

        let method = request["method"].as_str().unwrap_or("");
        let is_notification = request.get("id").is_none();

        let mut response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request["id"],
        });

        if method == "initialize" {
            response["result"] = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "terry-mcp-server",
                    "version": "1.0.0"
                }
            });
        } else if method.starts_with("notifications/") {
            continue; // Notifications must not receive a response.
        } else if method == "ping" {
            response["result"] = serde_json::json!({});
        } else if method == "tools/list" {
            response["result"] = serde_json::json!({
                "tools": [
                    {
                        "name": "new_tab",
                        "description": "Open a new terminal tab in the running Terry instance",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "cwd": { "type": "string" } }
                        }
                    },
                    {
                        "name": "send_text",
                        "description": "Paste text into the active terminal without executing it",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string" } },
                            "required": ["text"]
                        }
                    },
                    {
                        "name": "create_new_terminal",
                        "description": "Alias for new_tab (kept for compatibility)",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "cwd": { "type": "string" }, "title": { "type": "string" } }
                        }
                    }
                ]
            });
        } else if method == "tools/call" {
            let tool_name = request["params"]["name"].as_str().unwrap_or("");
            let tool_args = request["params"]["arguments"].clone();
            let endpoint = match tool_name {
                "new_tab" | "create_new_terminal" => Some("new_tab"),
                "send_text" => Some("send_text"),
                _ => None,
            };

            let Some(endpoint) = endpoint else {
                response["error"] = serde_json::json!({
                    "code": -32602,
                    "message": format!("Unknown tool: {tool_name}"),
                });
                let mut out = serde_json::to_string(&response)?;
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
                continue;
            };

            let client = reqwest::Client::new();
            let auth = format!("Bearer {}", conn_info.token);
            let url = format!("http://127.0.0.1:{}/api/{}", conn_info.port, endpoint);

            match client
                .post(&url)
                .header("Authorization", &auth)
                .json(&tool_args)
                .send()
                .await
            {
                Ok(res) => {
                    let status = res.status();
                    let text = res.text().await.unwrap_or_default();
                    if status.is_success() {
                        response["result"] = serde_json::json!({
                            "content": [{ "type": "text", "text": text }]
                        });
                    } else {
                        response["result"] = serde_json::json!({
                            "isError": true,
                            "content": [{
                                "type": "text",
                                "text": format!("Terry returned {}: {text}", status.as_u16())
                            }]
                        });
                    }
                }
                Err(error) => {
                    response["result"] = serde_json::json!({
                        "isError": true,
                        "content": [{
                            "type": "text",
                            "text": format!("Failed to connect to Terry: {error}")
                        }]
                    });
                }
            }
        } else if is_notification {
            // Unknown notification: no response is allowed.
            continue;
        } else {
            response["error"] = serde_json::json!({
                "code": -32601,
                "message": "Method not found"
            });
        }

        let mut out = serde_json::to_string(&response)?;
        out.push('\n');
        stdout.write_all(out.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}
