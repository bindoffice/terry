//! Reports the agent's run state to the host terminal.
//!
//! Terry (the terminal `tom` runs inside) shows each coding agent's state in its
//! sidebar: working, waiting for input, or idle. Most agents are read off their
//! screen; `tom` can do better than that because it *is* the agent, so it
//! reports the exact state instead of letting the host guess.
//!
//! The transport is Terry's local IPC API — the same endpoint the installed
//! integrations for other agents POST to
//! (`/api/report_agent_state`). Terry passes `TERRY_IPC_URL`,
//! `TERRY_IPC_TOKEN`, and `TERRY_TERMINAL_TOKEN` into every terminal it spawns;
//! when they are absent (tom running in another terminal, or standalone) this
//! module does nothing, so it costs one env lookup.
//!
//! Reporting is fire-and-forget on a detached task: a slow or wedged host must
//! never stall the agent's render loop or block a run.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The agent id Terry knows this as (`coding_agents()` in its terminal panel).
const AGENT_ID: &str = "tom";

/// Where to report to, resolved once from the environment.
#[derive(Clone, Debug)]
struct Endpoint {
    url: String,
    token: String,
    terminal_token: String,
}

impl Endpoint {
    fn from_env() -> Option<Self> {
        let url = non_empty(std::env::var("TERRY_IPC_URL").ok()?)?;
        let token = non_empty(std::env::var("TERRY_IPC_TOKEN").ok()?)?;
        let terminal_token = non_empty(std::env::var("TERRY_TERMINAL_TOKEN").ok()?)?;
        Some(Self {
            url,
            token,
            terminal_token,
        })
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The states Terry understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRunState {
    /// A turn is in flight.
    Working,
    /// A modal is waiting for the user.
    Blocked,
    /// Nothing is running.
    Idle,
}

impl AgentRunState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
        }
    }
}

/// Reports `state`, plus an optional message for Terry to display.
///
/// Never blocks the caller and never fails loudly: an unreachable host is not
/// the agent's problem.
pub fn report(state: AgentRunState, message: Option<String>) {
    let Some(endpoint) = Endpoint::from_env() else {
        return;
    };
    // Resolve the runtime handle on the calling thread; `tokio::spawn` inside
    // the spawned task would need a runtime already in scope there.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // Called from a non-async context (or before the runtime exists).
        // Reporting is best-effort, so dropping it beats spinning one up.
        return;
    };
    handle.spawn(async move {
        let body = serde_json::json!({
            "token": endpoint.terminal_token,
            "agent": AGENT_ID,
            "state": state.as_str(),
            "message": message,
        });
        let _ = post(&endpoint, &body.to_string()).await;
    });
}

/// Reports the resumable session id so a restart can `tom -r <id>`.
pub fn report_session(session_id: &str) {
    let Some(endpoint) = Endpoint::from_env() else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let session_id = session_id.to_string();
    handle.spawn(async move {
        let body = serde_json::json!({
            "token": endpoint.terminal_token,
            "agent": AGENT_ID,
            "session_id": session_id,
        });
        let _ = post(&endpoint, &body.to_string()).await;
    });
}

/// POSTs a JSON body to the host.
///
/// The host listens on loopback over plain HTTP and the payload is tiny, so a
/// hand-rolled request avoids pulling an HTTP client into this path (and avoids
/// the agent's proxy settings applying to a loopback call).
async fn post(endpoint: &Endpoint, body: &str) -> std::io::Result<()> {
    let path = match endpoint.url.strip_prefix("http://") {
        Some(rest) => rest,
        None => return Ok(()),
    };
    let (authority, base_path) = match path.find('/') {
        Some(index) => (&path[..index], &path[index..]),
        None => (path, ""),
    };
    let base_path = base_path.trim_end_matches('/');

    let body = body.to_string();
    // `authority` and `base_path` borrow `endpoint`; own them before moving
    // into the blocking task.
    let authority = authority.to_string();
    let base_path = base_path.to_string();
    let token = endpoint.token.clone();
    // Blocking I/O on a blocking thread: this runs on the Tokio runtime, and a
    // `TcpStream` read would otherwise occupy an async worker.
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let mut stream = TcpStream::connect(&authority)?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let request = format!(
            "POST {base_path}/api/report_agent_state HTTP/1.1\r\n\
             Host: {authority}\r\n\
             Authorization: Bearer {token}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {length}\r\n\
             Connection: close\r\n\r\n{body}",
            length = body.len(),
        );
        stream.write_all(request.as_bytes())?;
        // Drain so the host is not left writing into a closed socket; the
        // response body is not interesting.
        let mut sink = [0u8; 256];
        let _ = stream.read(&mut sink);
        Ok(())
    })
    .await
    .unwrap_or_else(|error| Err(std::io::Error::other(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_map_to_terry_vocabulary() {
        assert_eq!(AgentRunState::Working.as_str(), "working");
        assert_eq!(AgentRunState::Blocked.as_str(), "blocked");
        assert_eq!(AgentRunState::Idle.as_str(), "idle");
    }

    #[test]
    fn blank_environment_values_are_treated_as_absent() {
        assert!(non_empty("   ".to_string()).is_none());
        assert!(non_empty(String::new()).is_none());
        assert_eq!(non_empty(" x ".to_string()).as_deref(), Some("x"));
    }

    /// Drives `post` against a one-shot local listener, so the hand-rolled
    /// HTTP framing is verified rather than assumed.
    #[tokio::test]
    async fn post_sends_a_well_formed_request() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 2048];
            let read = socket.read(&mut buffer).expect("read");
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .ok();
            String::from_utf8_lossy(&buffer[..read]).to_string()
        });

        let endpoint = Endpoint {
            url: format!("http://127.0.0.1:{port}"),
            token: "test-token".to_string(),
            terminal_token: "terminal-token".to_string(),
        };
        post(&endpoint, r#"{"state":"working"}"#)
            .await
            .expect("post succeeds");

        let request = handle.join().expect("listener thread");
        assert!(
            request.starts_with("POST /api/report_agent_state HTTP/1.1\r\n"),
            "got: {request}"
        );
        assert!(request.contains("Authorization: Bearer test-token"));
        assert!(request.contains("Content-Type: application/json"));
        // Content-Length must match the body, or the host would hang waiting.
        let body = r#"{"state":"working"}"#;
        assert!(
            request.contains(&format!("Content-Length: {}", body.len())),
            "got: {request}"
        );
        assert!(request.ends_with(body), "got: {request}");
    }

    #[test]
    fn a_non_http_url_is_ignored() {
        let endpoint = Endpoint {
            url: "https://example.test".to_string(),
            token: "t".to_string(),
            terminal_token: "tt".to_string(),
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        // Loopback IPC is plain HTTP; anything else is silently skipped rather
        // than attempting a TLS handshake we do not implement.
        assert!(runtime.block_on(post(&endpoint, "{}")).is_ok());
    }

    #[test]
    fn a_missing_environment_disables_reporting() {
        // The test process has no TERRY_* variables set, so reporting must be a
        // no-op rather than a panic or a connection attempt.
        assert!(Endpoint::from_env().is_none());
        report(AgentRunState::Working, None);
        report_session("session-1");
    }
}
