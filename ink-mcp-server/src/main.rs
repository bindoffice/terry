use std::env;
use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
struct ConnectionInfo {
    port: u16,
    token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let conn_file = args
        .iter()
        .position(|arg| arg == "--connect")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".ink")
                .join("ipc.json")
        });

    if !conn_file.exists() {
        eprintln!("Connection file {:?} not found. Is Ink running?", conn_file);
        std::process::exit(1);
    }

    let conn_data = fs::read_to_string(&conn_file)?;
    let conn_info: ConnectionInfo = serde_json::from_str(&conn_data)
        .context("Failed to parse connection info")?;

    eprintln!("Connected to Ink on port {}", conn_info.port);

    // Minimal JSON-RPC MCP loop over Stdio
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        // Read JSON-RPC from client (e.g., Claude)
        // Parse it, route it to HTTP API
        // Wrap response and output to stdout
        

        let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let method = request["method"].as_str().unwrap_or("");
        
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
                    "name": "ink-mcp-server",
                    "version": "1.0.0"
                }
            });
        } else if method == "notifications/initialized" {
            continue; // No response needed
        } else if method == "tools/list" {
            response["result"] = serde_json::json!({
                "tools": [
                    {
                        "name": "list_sessions",
                        "description": "List all terminal sessions",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "focus_session",
                        "description": "Focus on a specific session window",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "session_id": { "type": "string" } },
                            "required": ["session_id"]
                        }
                    },
                    {
                        "name": "send_keys",
                        "description": "Send keystrokes to a session",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "session_id": { "type": "string" }, "keys": { "type": "string" } },
                            "required": ["session_id", "keys"]
                        }
                    },
                    {
                        "name": "read_screen",
                        "description": "Read the terminal screen",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "session_id": { "type": "string" }, "raw_ansi": { "type": "boolean" } },
                            "required": ["session_id"]
                        }
                    },
                    {
                        "name": "get_context",
                        "description": "Get current session context",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "session_id": { "type": "string" } }
                        }
                    },
                    {
                        "name": "notify",
                        "description": "Trigger attention system notification",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "message": { "type": "string" }, "session_id": { "type": "string" } },
                            "required": ["message"]
                        }
                    },
                    {
                        "name": "create_new_terminal",
                        "description": "Create a new terminal session",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "cwd": { "type": "string" }, "title": { "type": "string" } }
                        }
                    }
                ]
            });
        } else if method == "tools/call" {
            let tool_name = request["params"]["name"].as_str().unwrap_or("");
            let tool_args = &request["params"]["arguments"];
            
            let client = reqwest::Client::new();
            let auth = format!("Bearer {}", conn_info.token);
            let url = format!("http://127.0.0.1:{}/api/{}", conn_info.port, tool_name);

            if let Ok(res) = client.post(&url).header("Authorization", &auth).json(tool_args).send().await {
                // MCP format expects result.content array
                let text = res.text().await.unwrap_or_default();
                response["result"] = serde_json::json!({
                    "content": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ]
                });
            } else {
                response["result"] = serde_json::json!({
                    "isError": true,
                    "content": [{"type": "text", "text": "Failed to connect to Ink locally"}]
                });
            }
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
