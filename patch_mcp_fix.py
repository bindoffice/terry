import re

content = open("ink-mcp-server/src/main.rs").read()
search = """        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32601,
                "message": "Method not found or MCP not fully implemented yet"
            }
        });
        
        let mut out = serde_json::to_string(&response)?;"""

replace = """        let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
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
