//! End-to-end: the model asks for an MCP tool, tom executes it against a real
//! stdio MCP server, and the server's answer comes back to the model.
//!
//! No unit test covers this join. A mock LLM over a local socket drives the
//! loop, a shell script provides the MCP server, and the assertion is on what
//! the *second* LLM request contains — i.e. the tool result the agent actually
//! produced.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A fake OpenAI-compatible endpoint. Records the request body of the call that
/// follows the tool execution.
fn start_fake_llm() -> (String, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::new(Mutex::new(String::new()));

    let calls_thread = calls.clone();
    let recorded_thread = recorded.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        // Detect the post-tool request so the listener thread can exit instead
        // of keeping the test process alive.
        let _ = &stop_thread;
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

            calls_thread.fetch_add(1, Ordering::SeqCst);
            // Decide from the request itself, not a call counter: tom may retry,
            // and a counter would mislabel a retry as the post-tool request.
            let has_tool_result = request.contains(r#""role":"tool""#);
            if has_tool_result {
                *recorded_thread.lock().expect("recorded") = request;
            }
            let is_first = !has_tool_result;

            // tom speaks the streaming API: an event-stream of `data:` frames.
            // A plain JSON body is accepted by some providers but leaves the
            // agent without a tool call, which would time out the run.
            let (delta, finish_reason) = if is_first {
                (
                    r#"{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"mcp__demo__add","arguments":"{\"a\":1}"}}]}"#,
                    "tool_calls",
                )
            } else {
                (r#"{"role":"assistant","content":"done"}"#, "stop")
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

    (format!("http://127.0.0.1:{port}/v1"), recorded)
}

#[test]
fn model_tool_call_reaches_the_mcp_server_and_returns_its_result() {
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
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"add","description":"Add","inputSchema":{"type":"object","properties":{"a":{"type":"number"}},"required":["a"]}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"MCP-ANSWER-42"}]}}\n' "$id" ;;
    *) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();

    let (base_url, recorded) = start_fake_llm();

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
                    "description": "demo"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    // Prove the endpoint is reachable from this process before blaming tom.
    {
        use std::io::Write as _;
        let mut probe = std::net::TcpStream::connect(
            base_url
                .trim_start_matches("http://")
                .trim_end_matches("/v1"),
        )
        .expect("mock reachable");
        probe.write_all(b"POST /v1/chat/completions HTTP/1.1\r\nhost: x\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .unwrap();
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut probe, &mut buf);
        assert!(buf.contains("200"), "probe got: {buf}");
    }

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

    let recorded = recorded.lock().expect("recorded").clone();
    assert!(
        recorded.contains("MCP-ANSWER-42"),
        "the MCP server's answer never reached the model; second request was:\n{recorded}"
    );
}
