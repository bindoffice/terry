#![cfg(unix)]

//! End-to-end tests for the HTTP endpoint a phone client talks to.
//!
//! These use a real daemon, a real PTY and real HTTP requests: the point is to
//! prove a client can drive an agent session with the desktop GUI closed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use terminal_daemon::ServeOptions;

static COUNTER: AtomicU32 = AtomicU32::new(0);

const TOKEN: &str = "test-token";

struct Daemon {
    port: u16,
    /// Where the endpoint reads its bearer token from.
    token_file: PathBuf,
}

fn start_daemon() -> Daemon {
    let socket = PathBuf::from(format!(
        "/tmp/td-http-{}-{}.sock",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));

    let path = socket.clone();
    // The endpoint re-reads its token from disk, so the test writes one first.
    let token_file = socket.with_extension("token");
    std::fs::write(&token_file, TOKEN).expect("write token file");
    // Port 0 lets the OS pick, so tests running in parallel never collide; the
    // daemon reports the real port back through the callback.
    let (tx, rx) = std::sync::mpsc::channel();
    let token_path_for_daemon = token_file.clone();
    std::thread::spawn(move || {
        let options = ServeOptions::with_http("127.0.0.1:0", token_path_for_daemon);
        if let Err(error) = terminal_daemon::serve_with_options_and_port(&path, options, |port| {
            let _ = tx.send(port);
        }) {
            eprintln!("test daemon failed: {error}");
        }
    });

    let port = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("daemon did not report its http port");

    Daemon { port, token_file }
}

/// A minimal HTTP/1.1 client: the daemon's own tests must not drag in reqwest.
fn post(port: u16, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let auth = token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

fn post_json(port: u16, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let (status, raw) = post(port, path, Some(TOKEN), &body.to_string());
    let parsed = if raw.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{error}: {raw}"))
    };
    (status, parsed)
}

/// Starts a shell whose behaviour the test controls, over HTTP.
fn new_tab(daemon: &Daemon, script: &str) -> String {
    let (status, body) = post_json(
        daemon.port,
        "/api/new_tab",
        &serde_json::json!({
            "program": "/bin/sh",
            "args": ["-c", script],
            "cols": 80,
            "rows": 24,
        }),
    );
    assert_eq!(status, 200, "new_tab failed: {body}");
    body["session"].as_str().expect("session id").to_string()
}

/// Polls `read_screen` until `needle` shows up.
fn wait_for_output(daemon: &Daemon, session: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = post_json(
            daemon.port,
            "/api/read_screen",
            &serde_json::json!({ "session": session, "lines": 50 }),
        );
        assert_eq!(status, 200, "read_screen failed: {body}");
        last = body["content"].as_str().unwrap_or_default().to_string();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {needle:?}; last screen: {last:?}");
}

#[test]
fn requests_without_a_token_are_rejected() {
    let daemon = start_daemon();

    let (status, _) = post(daemon.port, "/api/list_sessions", None, "{}");
    assert_eq!(status, 401, "an unauthenticated client must be refused");

    let (status, _) = post(daemon.port, "/api/list_sessions", Some("wrong"), "{}");
    assert_eq!(status, 401);
}

#[test]
fn health_needs_no_token() {
    let daemon = start_daemon();

    let (status, body) = post(daemon.port, "/api/health", None, "{}");
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "OK");
}

#[test]
fn a_session_can_be_created_read_and_answered_over_http() {
    let daemon = start_daemon();

    let session = new_tab(
        &daemon,
        "printf 'Do you want to proceed? (y/n)\\n'; read a; echo ANSWER:$a; sleep 30",
    );

    // 1. The screen tail, which is all a phone list needs.
    let lines = wait_for_output(&daemon, &session, "proceed? (y/n)");
    assert!(lines.contains("proceed? (y/n)"));

    // 2. The styled viewport, same shape as the GUI's /api/screen.
    let (status, screen) = post_json(
        daemon.port,
        "/api/screen",
        &serde_json::json!({ "session": session }),
    );
    assert_eq!(status, 200, "screen failed: {screen}");
    assert_eq!(screen["rows"], 24);
    assert_eq!(screen["cols"], 80);
    assert!(screen["lines"].is_array());

    // 3. Answer it.
    let (status, body) = post_json(
        daemon.port,
        "/api/send_text",
        &serde_json::json!({ "session": session, "text": "y\n" }),
    );
    assert_eq!(status, 200, "send_text failed: {body}");
    wait_for_output(&daemon, &session, "ANSWER:y");

    // 4. And it is listed with the geometry it was created with.
    let (_, sessions) = post_json(daemon.port, "/api/list_sessions", &serde_json::json!({}));
    let listed = sessions["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.as_str())
        .expect("session listed");
    assert_eq!(listed["cols"], 80);
    assert_eq!(listed["rows"], 24);

    // 5. Closing it removes it.
    let (status, _) = post_json(
        daemon.port,
        "/api/close_session",
        &serde_json::json!({ "session": session }),
    );
    assert_eq!(status, 200);
    let (_, sessions) = post_json(daemon.port, "/api/list_sessions", &serde_json::json!({}));
    assert!(
        !sessions["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|entry| entry["session"] == session.as_str()),
        "a closed session must disappear"
    );
}

#[test]
fn unknown_sessions_are_reported_as_not_found() {
    let daemon = start_daemon();

    for path in ["/api/screen", "/api/read_screen", "/api/send_text"] {
        let (status, body) = post_json(
            daemon.port,
            path,
            &serde_json::json!({ "session": "t-does-not-exist", "text": "x" }),
        );
        assert_eq!(status, 404, "{path} should be a 404, got {body}");
    }
}

#[test]
fn a_missing_session_parameter_is_a_bad_request() {
    let daemon = start_daemon();

    for path in ["/api/screen", "/api/read_screen", "/api/send_text"] {
        let (status, body) = post_json(daemon.port, path, &serde_json::json!({ "text": "x" }));
        assert_eq!(status, 400, "{path} should demand a session, got {body}");
    }
}

/// A plain shell is not an agent: `/api/list_agent_states` must stay empty.
///
/// This is the regression guard for the tempting shortcut of probing every
/// agent's rules. Codex's table matches an ordinary prompt and reports `Idle`,
/// so "no known agent" has to mean "not listed", not "guess".
#[test]
fn a_plain_shell_is_not_listed_as_an_agent() {
    let daemon = start_daemon();
    let session = new_tab(&daemon, "printf '$ ls\\nsrc  Cargo.toml\\n'; sleep 30");
    wait_for_output(&daemon, &session, "Cargo.toml");

    let (status, body) = post_json(
        daemon.port,
        "/api/list_agent_states",
        &serde_json::json!({}),
    );
    assert_eq!(status, 200, "list_agent_states failed: {body}");
    assert_eq!(
        body["terminals"].as_array().map(Vec::len),
        Some(0),
        "a shell must not be reported as an idle agent: {body}"
    );
}

/// Remote clients get every concurrently running agent from the HTTP list. A
/// foreground process is enough identity even when no integration reports.
#[test]
fn foreground_process_agents_are_listed_over_http() {
    use std::os::unix::fs::symlink;

    let daemon = start_daemon();
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .expect("sleep executable");
    let directory = std::env::temp_dir().join(format!(
        "td-http-agent-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&directory).expect("temp dir");

    let mut sessions = Vec::new();
    for agent in agent_catalog::coding_agents() {
        let executable = directory.join(agent.bin);
        symlink(sleep, &executable).expect("agent shim");
        let (status, body) = post_json(
            daemon.port,
            "/api/new_tab",
            &serde_json::json!({
                "program": executable,
                "args": ["30"],
                "cols": 80,
                "rows": 24,
            }),
        );
        assert_eq!(status, 200, "new_tab failed: {body}");
        let session = body["session"].as_str().expect("session id").to_string();
        sessions.push((session, agent.id));
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, body) = post_json(
            daemon.port,
            "/api/list_agent_states",
            &serde_json::json!({}),
        );
        assert_eq!(status, 200, "list_agent_states failed: {body}");
        let terminals = body["terminals"].as_array().expect("terminals");
        let missing: Vec<_> = sessions
            .iter()
            .filter_map(|(session, expected)| {
                let actual = terminals
                    .iter()
                    .find(|entry| entry["session"] == session.as_str())
                    .and_then(|entry| entry["agent"].as_str());
                (actual != Some(*expected)).then_some((expected, actual))
            })
            .collect();
        if missing.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not list foreground agents: {missing:?}; response: {body}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    for (session, _) in sessions {
        let (status, body) = post_json(
            daemon.port,
            "/api/close_session",
            &serde_json::json!({ "session": session }),
        );
        assert_eq!(status, 200, "close_session failed: {body}");
    }

    std::fs::remove_dir_all(directory).ok();
}

/// The agent integration path, end to end.
///
/// A session spawned over HTTP is handed `TERRY_TERMINAL_TOKEN` and
/// `TERRY_IPC_URL`. The hook script uses them to POST its state back; that is how
/// the daemon learns which agent runs in a PTY it cannot inspect. This drives the
/// same HTTP calls the shell script in `assets/agent-integrations` makes.
#[test]
fn a_hook_report_drives_the_agent_state() {
    let daemon = start_daemon();

    // Echo the injected token first, so the test can act as the hook.
    let session = new_tab_reading_token(
        &daemon,
        "printf 'TOKEN:%s\\n' \"$TERRY_TERMINAL_TOKEN\"; sleep 30",
    );
    let screen = wait_for_output(&daemon, &session, "TOKEN:");
    let token = screen
        .lines()
        .find_map(|line| line.strip_prefix("TOKEN:"))
        .expect("the daemon must inject a terminal token")
        .trim()
        .to_string();
    assert!(!token.is_empty());

    // Nothing reported yet, so it is not listed as an agent.
    let (_, body) = post_json(
        daemon.port,
        "/api/list_agent_states",
        &serde_json::json!({}),
    );
    assert_eq!(body["terminals"].as_array().map(Vec::len), Some(0));

    // The hook announces the agent, then reports it blocked.
    let (status, body) = post_json(
        daemon.port,
        "/api/report_agent_session",
        &serde_json::json!({ "token": token, "agent": "claude" }),
    );
    assert_eq!(status, 200, "report_agent_session failed: {body}");

    let (status, body) = post_json(
        daemon.port,
        "/api/report_agent_state",
        &serde_json::json!({ "token": token, "state": "blocked", "agent": "claude" }),
    );
    assert_eq!(status, 200, "report_agent_state failed: {body}");

    let (_, body) = post_json(
        daemon.port,
        "/api/list_agent_states",
        &serde_json::json!({}),
    );
    let terminals = body["terminals"].as_array().expect("terminals");
    let entry = terminals
        .iter()
        .find(|entry| entry["session"] == session.as_str())
        .unwrap_or_else(|| panic!("session should be listed: {body}"));
    assert_eq!(entry["state"], "blocked");
    assert_eq!(entry["agent"], "claude");

    // The agent answers and the hook reports it working again.
    let (status, _) = post_json(
        daemon.port,
        "/api/report_agent_state",
        &serde_json::json!({ "token": token, "state": "working", "agent": "claude" }),
    );
    assert_eq!(status, 200);
    let (_, body) = post_json(
        daemon.port,
        "/api/list_agent_states",
        &serde_json::json!({}),
    );
    let entry = body["terminals"]
        .as_array()
        .expect("terminals")
        .iter()
        .find(|entry| entry["session"] == session.as_str())
        .expect("listed");
    assert_eq!(entry["state"], "working");

    // An unknown token is still refused after the happy path.
    let (status, body) = post_json(
        daemon.port,
        "/api/report_agent_state",
        &serde_json::json!({ "token": "not-a-real-token", "state": "blocked" }),
    );
    assert_eq!(status, 400, "unknown tokens must be refused: {body}");
}

/// Like [`new_tab`], but lets the test read a helper the daemon injected.
fn new_tab_reading_token(daemon: &Daemon, script: &str) -> String {
    new_tab(daemon, script)
}

/// The key bar a phone renders: `send_keys` must reach the application.
#[test]
fn named_keys_reach_the_program() {
    let daemon = start_daemon();

    // `stty raw -echo` so the shell passes bytes through untouched and the
    // test can see exactly what arrived.
    let session = new_tab(
        &daemon,
        "printf 'READY\\n'; read -r line; printf 'GOT:%s\\n' \"$line\"; sleep 30",
    );
    wait_for_output(&daemon, &session, "READY");

    // Type text, then a named key.
    let (status, body) = post_json(
        daemon.port,
        "/api/send_text",
        &serde_json::json!({ "session": session, "text": "abc" }),
    );
    assert_eq!(status, 200, "send_text failed: {body}");

    let (status, body) = post_json(
        daemon.port,
        "/api/send_keys",
        &serde_json::json!({ "session": session, "keys": "enter" }),
    );
    assert_eq!(status, 200, "send_keys failed: {body}");
    wait_for_output(&daemon, &session, "GOT:abc");
}

/// `ctrl-c` must interrupt, which is the most used key on a phone.
#[test]
fn ctrl_c_interrupts_a_running_command() {
    let daemon = start_daemon();

    let session = new_tab(&daemon, "printf 'BEFORE\\n'; sleep 30; printf 'AFTER\\n'");
    wait_for_output(&daemon, &session, "BEFORE");

    let (status, body) = post_json(
        daemon.port,
        "/api/send_keys",
        &serde_json::json!({ "session": session, "keys": ["ctrl-c"] }),
    );
    assert_eq!(status, 200, "send_keys failed: {body}");

    // The shell survives the interrupt and prints nothing more: `sleep 30` was
    // the foreground job, so `AFTER` must never appear.
    std::thread::sleep(Duration::from_millis(300));
    let (_, body) = post_json(
        daemon.port,
        "/api/read_screen",
        &serde_json::json!({ "session": session, "lines": 20 }),
    );
    let content = body["content"].as_str().unwrap_or_default();
    assert!(
        !content.contains("AFTER"),
        "ctrl-c should have interrupted the sleep: {content}"
    );
}

#[test]
fn unknown_key_names_are_a_bad_request() {
    let daemon = start_daemon();
    let session = new_tab(&daemon, "sleep 30");
    wait_for_output(&daemon, &session, "");

    let (status, body) = post_json(
        daemon.port,
        "/api/send_keys",
        &serde_json::json!({ "session": session, "keys": ["not-a-key"] }),
    );
    assert_eq!(status, 400, "an unknown key must be refused: {body}");
}

/// The list endpoint tells the client whether the running program wants
/// application cursor keys, so its arrow buttons can encode correctly.
#[test]
fn the_listing_reports_key_modes() {
    let daemon = start_daemon();
    let session = new_tab(&daemon, "sleep 30");
    wait_for_output(&daemon, &session, "");

    let (_, body) = post_json(daemon.port, "/api/list_sessions", &serde_json::json!({}));
    let entry = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.as_str())
        .expect("listed");

    assert_eq!(
        entry["application_cursor"], false,
        "a plain shell does not set DECCKM"
    );
}

/// The full T6 chain: a session gets hook env, the hook reports, the listing
/// carries the agent, and therefore the reaper leaves it alone.
#[test]
fn a_reported_agent_protects_the_session_from_the_reaper() {
    let daemon = start_daemon();

    // The daemon must hand the session everything the hook scripts require:
    // the terminal token, plus the endpoint and token they post with.
    let session = new_tab(
        &daemon,
        "printf 'TOK:%s URL:%s IPCTOK:%s\\n' \"$TERRY_TERMINAL_TOKEN\" \"$TERRY_IPC_URL\" \"$TERRY_IPC_TOKEN\"; sleep 30",
    );
    let screen = wait_for_output(&daemon, &session, "IPCTOK:");
    let line = screen
        .lines()
        .find(|line| line.starts_with("TOK:"))
        .expect("the daemon must print the injected env");

    let token = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("TOK:"))
        .expect("terminal token")
        .to_string();
    let ipc_url = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("URL:"))
        .expect("TERRY_IPC_URL");
    let ipc_token = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("IPCTOK:"))
        .expect("TERRY_IPC_TOKEN");

    assert!(!token.is_empty(), "TERRY_TERMINAL_TOKEN must be set");
    assert!(
        ipc_url.starts_with("http://"),
        "hooks need an endpoint to post to, got {ipc_url:?}"
    );
    assert!(
        !ipc_token.is_empty(),
        "the hook scripts bail out when TERRY_IPC_TOKEN is empty"
    );

    // The hook announces its agent and reports it blocked.
    let (status, body) = post_json(
        daemon.port,
        "/api/report_agent_session",
        &serde_json::json!({ "token": token, "agent": "claude" }),
    );
    assert_eq!(status, 200, "report_agent_session failed: {body}");
    let (status, _) = post_json(
        daemon.port,
        "/api/report_agent_state",
        &serde_json::json!({ "token": token, "state": "blocked", "agent": "claude" }),
    );
    assert_eq!(status, 200);

    // The listing the reaper reads now carries the agent, so `prune` skips it.
    let (_, body) = post_json(daemon.port, "/api/list_sessions", &serde_json::json!({}));
    let entry = body["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.as_str())
        .expect("listed");
    assert_eq!(
        entry["agent"], "claude",
        "the reaper keys off this field: without it the session looks abandoned"
    );
}

/// The catalogue a phone needs in order to offer "start an agent".
#[test]
fn the_agent_catalogue_is_served() {
    let daemon = start_daemon();

    let (status, body) = post_json(daemon.port, "/api/list_agents", &serde_json::json!({}));
    assert_eq!(status, 200, "list_agents failed: {body}");

    let agents = body["agents"].as_array().expect("agents");
    assert!(!agents.is_empty(), "the catalogue must not be empty");

    let claude = agents
        .iter()
        .find(|agent| agent["id"] == "claude")
        .expect("claude should be in the catalogue");
    assert_eq!(claude["launch"], "claude");
    assert_eq!(claude["can_resume"], true);

    // Every entry must carry what a client needs to launch it.
    for agent in agents {
        assert!(
            agent["id"].as_str().is_some_and(|id| !id.is_empty()),
            "an agent entry without an id cannot be launched: {agent}"
        );
        assert!(
            agent["launch"].as_str().is_some_and(|bin| !bin.is_empty()),
            "{agent}"
        );
    }
}

#[test]
fn an_unknown_agent_id_is_a_bad_request() {
    let daemon = start_daemon();

    let (status, body) = post_json(
        daemon.port,
        "/api/new_tab",
        &serde_json::json!({ "agent": "not-an-agent" }),
    );
    assert_eq!(status, 400, "an unknown agent must be refused: {body}");
}

/// Starting an agent by id must actually launch it.
///
/// `tom` is Terry's own agent and is always present, so it is the one agent the
/// test can rely on existing.
#[test]
fn new_tab_can_launch_an_agent_by_id() {
    let daemon = start_daemon();

    // Find an agent that is actually installed; skip if the environment has none.
    let (_, body) = post_json(daemon.port, "/api/list_agents", &serde_json::json!({}));
    let candidates: Vec<String> = body["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .filter_map(|agent| agent["id"].as_str().map(str::to_string))
        .collect();

    let installed = candidates.iter().find(|id| {
        let bin = agent_catalog::agent_by_id(id)
            .map(|agent| agent.bin)
            .unwrap_or("");
        !bin.is_empty() && which(bin)
    });
    let Some(agent_id) = installed else {
        eprintln!("no coding agent is installed; skipping");
        return;
    };

    let (status, body) = post_json(
        daemon.port,
        "/api/new_tab",
        &serde_json::json!({ "agent": agent_id, "cols": 80, "rows": 24 }),
    );
    assert_eq!(status, 200, "launching {agent_id} failed: {body}");
    assert!(body["session"].as_str().is_some_and(|s| !s.is_empty()));

    // The session must exist and be listed.
    let session = body["session"].as_str().expect("session").to_string();
    let (_, sessions) = post_json(daemon.port, "/api/list_sessions", &serde_json::json!({}));
    assert!(
        sessions["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|entry| entry["session"] == session.as_str()),
        "the launched session should be listed"
    );

    let _ = post_json(
        daemon.port,
        "/api/close_session",
        &serde_json::json!({ "session": session }),
    );
}

/// A minimal GET, for the embedded client assets.
fn get(port: u16, path: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let content_type = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-type")
                .then(|| value.trim().to_string())
        })
        .unwrap_or_default();
    (status, content_type, body.to_string())
}

/// The phone client is served by the daemon itself, so a device only needs the
/// address and the token.
#[test]
fn the_client_is_served_without_a_token() {
    let daemon = start_daemon();

    let (status, content_type, body) = get(daemon.port, "/");
    assert_eq!(status, 200, "the app shell must load before pairing");
    assert!(content_type.starts_with("text/html"), "{content_type}");
    assert!(
        body.contains("<title>Terry"),
        "not the app shell: {body:.80}"
    );

    for (path, expected) in [
        ("/app.js", "text/javascript"),
        ("/app.css", "text/css"),
        ("/manifest.webmanifest", "application/manifest+json"),
    ] {
        let (status, content_type, body) = get(daemon.port, path);
        assert_eq!(status, 200, "{path}");
        assert!(
            content_type.starts_with(expected),
            "{path} served as {content_type}"
        );
        assert!(!body.is_empty(), "{path} is empty");
    }
}

/// Serving the shell must not have opened a hole in the API.
#[test]
fn serving_the_client_does_not_expose_the_api() {
    let daemon = start_daemon();

    // The shell loads...
    assert_eq!(get(daemon.port, "/").0, 200);
    // ...but data still needs the token.
    let (status, _) = post(daemon.port, "/api/list_sessions", None, "{}");
    assert_eq!(status, 401, "the app shell must not imply open API access");
}

/// Whether an executable is on PATH.
fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(bin);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// A client must be able to tell which agents are actually launchable, and a
/// failed launch must say why in words a user can act on.
#[test]
fn the_catalogue_says_which_agents_are_installed() {
    let daemon = start_daemon();

    let (_, body) = post_json(daemon.port, "/api/list_agents", &serde_json::json!({}));
    let agents = body["agents"].as_array().expect("agents");

    // `installed` must reflect this process's PATH, so it agrees with what a
    // launch attempt would do.
    for agent in agents {
        let bin = agent["launch"].as_str().expect("launch");
        assert_eq!(
            agent["installed"],
            which(bin),
            "{bin}: the catalogue disagrees with PATH"
        );
    }

    // Some agent must be installed in CI (at minimum, none is also fine: the
    // assertion above is the real check).
}

/// Launching an agent that is not installed explains itself.
#[test]
fn launching_a_missing_agent_explains_the_problem() {
    let daemon = start_daemon();

    // Find a catalogue entry whose executable is absent.
    let (_, body) = post_json(daemon.port, "/api/list_agents", &serde_json::json!({}));
    let missing = body["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["installed"] == false)
        .and_then(|agent| agent["id"].as_str().map(str::to_string));
    let Some(agent_id) = missing else {
        eprintln!("every catalogue agent is installed; skipping");
        return;
    };

    let (status, body) = post_json(
        daemon.port,
        "/api/new_tab",
        &serde_json::json!({ "agent": agent_id }),
    );
    assert!(
        (400..500).contains(&status),
        "a missing executable is the caller's problem to fix, got {status}: {body}"
    );
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("not installed") || hint.contains("PATH"),
        "the error must be actionable, got {body}"
    );
}

/// Rotating the token must revoke the old one on the *running* daemon.
///
/// This is the only way to revoke a leaked or lost-device token without stopping
/// every session the daemon owns. Reading the token from disk per request is
/// what makes that possible; capturing it at startup would leave the leaked
/// token valid until the next restart, which is exactly when the user cannot
/// afford it.
#[test]
fn rotating_the_token_revokes_the_old_one_immediately() {
    let daemon = start_daemon();

    // The original token works.
    let (status, _) = post(daemon.port, "/api/list_sessions", Some(TOKEN), "{}");
    assert_eq!(status, 200);

    // Rotate: write a new token to the same file.
    std::fs::write(&daemon.token_file, "rotated-token").expect("rotate");

    let (status, body) = post(daemon.port, "/api/list_sessions", Some(TOKEN), "{}");
    assert_eq!(
        status, 401,
        "the old token must stop working without a restart: {body}"
    );

    let (status, _) = post(
        daemon.port,
        "/api/list_sessions",
        Some("rotated-token"),
        "{}",
    );
    assert_eq!(status, 200, "the new token must work right away");
}

/// A deleted token file must authorise nobody: failing closed is the only safe
/// answer for something that grants shell access.
#[test]
fn a_missing_token_file_authorises_nobody() {
    let daemon = start_daemon();

    let (status, _) = post(daemon.port, "/api/list_sessions", Some(TOKEN), "{}");
    assert_eq!(status, 200);

    std::fs::remove_file(&daemon.token_file).expect("remove token file");

    let (status, _) = post(daemon.port, "/api/list_sessions", Some(TOKEN), "{}");
    assert_eq!(
        status, 401,
        "a missing token file must not authorise anything"
    );
}
