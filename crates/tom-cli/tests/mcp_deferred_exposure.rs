//! End-to-end: a server marked `"exposure": "deferred"` is not declared until
//! `tool_search` loads it, and the loaded tool is then callable in the *same*
//! run.
//!
//! This is the join no unit test covers: config → per-server exposure → the
//! declared tool list sent to the provider → the loader's load → the declared
//! list of the next request. A mock LLM over a local socket records every
//! request, so each assertion is on what the provider was actually asked to
//! declare, and on the MCP answer that comes back to the model.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// A fake OpenAI-compatible endpoint that records every request body and plays a
/// scripted sequence of tool calls.
///
/// The turn is chosen from the request, not a counter: tom may retry, and a
/// counter would then shift every subsequent step.
fn start_fake_llm() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_thread = Arc::clone(&requests);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));

            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);
            let request = String::from_utf8_lossy(&body).into_owned();
            requests_thread
                .lock()
                .expect("requests")
                .push(request.clone());

            // Turn selection by *tool results seen*, not by name: the tools array
            // contains both names on every request, so a name probe would match
            // the declaration and mislabel the first turn.
            let tool_results = request.matches(r#""role":"tool""#).count();
            let (delta, finish_reason) = if tool_results >= 2 {
                (r#"{"role":"assistant","content":"finished"}"#, "stop")
            } else if tool_results == 1 {
                (
                    r#"{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c2","type":"function","function":{"name":"mcp__demo__add","arguments":"{\"a\":1}"}}]}"#,
                    "tool_calls",
                )
            } else {
                (
                    r#"{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"tool_search","arguments":"{\"query\":\"add numbers\"}"}}]}"#,
                    "tool_calls",
                )
            };

            let first = format!(
                "data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":null}}]}}\n\n"
            );
            let second = format!(
                "data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{finish_reason}\"}}],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}}}\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{first}{second}data: [DONE]\n\n"
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), requests)
}

/// The `"name":"…"` entries of the `tools` array in one request body. Crude on
/// purpose: the body is a single JSON line, and a substring probe would confuse
/// a tool named in a message with one declared in the tool list.
fn declared_tools(request: &str) -> Vec<String> {
    let Some(start) = request.find(r#""tools":["#) else {
        return Vec::new();
    };
    let tail = &request[start..];
    // The tools array is the last field tom sends, so scanning to the end is safe.
    let mut names = Vec::new();
    let mut rest = tail;
    while let Some(index) = rest.find(r#""function":{"name":""#) {
        let after = &rest[index + r#""function":{"name":""#.len()..];
        let Some(end) = after.find('"') else { break };
        names.push(after[..end].to_string());
        rest = &after[end..];
    }
    names
}

#[test]
fn a_deferred_server_is_undeclared_until_tool_search_loads_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let agent_dir = dir.path().join("agent");
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    let server = dir.path().join("server.sh");
    std::fs::write(
        &server,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"demo","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"add","description":"Add two numbers together","inputSchema":{"type":"object","properties":{"a":{"type":"number"}},"required":["a"]}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"MCP-ANSWER-42"}]}}\n' "$id" ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();

    let (base_url, requests) = start_fake_llm();

    std::fs::write(
        agent_dir.join("models.json"),
        serde_json::json!({
            "providers": {
                "local": {
                    "api": "openai-completions",
                    "baseUrl": base_url,
                    "apiKey": "dummy",
                    "models": [{
                        "id": "m",
                        "name": "m",
                        "contextWindow": 128000,
                        "maxTokens": 4096
                    }]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    std::fs::write(
        agent_dir.join("mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "demo": {
                    "command": "/bin/sh",
                    "args": [server.to_string_lossy()],
                    "description": "demo",
                    "exposure": "deferred"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tom"))
        .env("TOM_CODING_AGENT_DIR", &agent_dir)
        .current_dir(&project_dir)
        .args(["--provider", "local", "--model", "m", "-p", "add 1"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run tom");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "tom failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let requests = requests.lock().expect("requests").clone();
    assert!(
        requests.len() >= 3,
        "expected search → call → stop, got {} request(s):\n{}",
        requests.len(),
        {
            let mut dump = String::new();
            for (i, request) in requests.iter().enumerate() {
                dump.push_str(&format!(
                    "\n=== request {i}: declared={:?}\n",
                    declared_tools(request)
                ));
                let mut idx = 0;
                while let Some(found) = request[idx..].find("\"role\":\"tool\"") {
                    let start = idx + found;
                    dump.push_str(&request[start..(start + 300).min(request.len())]);
                    dump.push('\n');
                    idx = start + 10;
                }
            }
            dump
        }
    );

    // Request 1: the server is deferred, so the model sees the loader and not the
    // MCP tool. This is the whole point of the feature — if the tool were
    // declared here, deferral would cost a round trip and save nothing.
    let first = declared_tools(&requests[0]);
    assert!(
        first.iter().any(|name| name == "tool_search"),
        "the loader must be declared: {first:?}"
    );
    assert!(
        !first.iter().any(|name| name == "mcp__demo__add"),
        "a deferred tool must not be declared before it is loaded: {first:?}"
    );

    // Request 2 (after the search): the load is visible in the same run.
    let second = declared_tools(&requests[1]);
    assert!(
        second.iter().any(|name| name == "mcp__demo__add"),
        "the loaded tool must be declared on the next request: {second:?}"
    );
    assert!(
        requests[1].contains(r#"\"query\":\"add numbers\""#),
        "request 2 must carry the search call: {}",
        requests[1]
    );

    // Request 3: the MCP answer reached the model.
    assert!(
        requests
            .last()
            .is_some_and(|request| request.contains("MCP-ANSWER-42")),
        "the MCP server's answer never reached the model; last request was:\n{}",
        requests.last().cloned().unwrap_or_default()
    );
}
