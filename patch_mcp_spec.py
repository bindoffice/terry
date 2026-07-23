import re

content = open("ink-mcp-server/src/main.rs").read()

replace = """
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
        
        let mut out = serde_json::to_string(&response)?;"""

search = """        let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let method = request["method"].as_str().unwrap_or("");
        
        let client = reqwest::Client::new();
        let auth = format!("Bearer {}", conn_info.token);
        
        let url = format!("http://127.0.0.1:{}/api/{}", conn_info.port, method.replace("tools/", ""));
        
        let mut response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request["id"],
        });

        if let Ok(res) = client.post(&url).header("Authorization", &auth).json(&request["params"]).send().await {
            let text = res.text().await.unwrap_or_default();
            response["result"] = serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!(text));
        } else {
            response["error"] = serde_json::json!({
                "code": -32601,
                "message": "Method not found or MCP not fully implemented yet"
            });
        }
        
        let mut out = serde_json::to_string(&response)?;"""

content = content.replace(search, replace)
open("ink-mcp-server/src/main.rs", "w").write(content)
