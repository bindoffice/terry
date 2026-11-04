//! HTTP endpoint for companion clients (phone, tablet).
//!
//! The daemon speaks a length-prefixed frame protocol over a Unix socket, which
//! only a local process can use. A phone needs HTTP, so this module adds an
//! opt-in listener that exposes the same information the GUI's `/api/*` routes
//! do — but for sessions the daemon owns, so it keeps working with the desktop
//! app closed.
//!
//! Deliberately **not** in scope: anything that needs a window. There are no
//! groups, panes, fonts or color schemes here, because the daemon has no idea
//! what a pane is.
//!
//! # Security
//!
//! Every `/api` route requires the bearer token except `/api/health`. The
//! embedded client's static assets do not, since they are what a device loads
//! before it has a token and they expose no session data.
//!
//! The endpoint can spawn arbitrary programs, so it is exactly as sensitive as
//! a shell: it is off unless `--http` is passed, and it should only be reached
//! over a private network (Tailscale, WireGuard or the LAN), never the public
//! internet.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::agents;
use crate::protocol::SessionInfo;
use crate::{SessionSpec, new_session_id};

/// Snapshot of a session, as the HTTP layer reports it.
pub struct SessionSummary {
    pub session: String,
    pub title: String,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
    pub agent: Option<String>,
    pub state: Option<&'static str>,
    /// True when the running program set application cursor keys, which decides
    /// between `ESC O A` and `ESC [ A`. A client that ignores it types `^[[A`
    /// into vim.
    pub application_cursor: bool,
}

/// Everything the handlers need, independent of the Unix-socket server.
///
/// This is a trait rather than direct access to the daemon's registry so the
/// HTTP layer can be tested without a real PTY, and so the two sides stay
/// honest about what the endpoint is allowed to do.
pub trait Sessions: Send + Sync + 'static {
    /// Every live session.
    fn list(&self) -> Vec<SessionSummary>;
    /// The styled screen of one session, encoded like the GUI's `/api/screen`.
    fn screen(&self, session: &str) -> Option<Value>;
    /// The last `count` non-empty lines of one session.
    fn read_screen(&self, session: &str, count: usize) -> Option<Vec<String>>;
    /// Sends bytes to a session's PTY.
    fn send_text(&self, session: &str, text: &str) -> Result<(), String>;
    /// Sends a named key (`ctrl-c`, `escape`, `up`, …) to a session's PTY.
    ///
    /// Encoded against the session's own modes, so arrows work inside vim.
    fn send_keys(&self, session: &str, keys: &[String]) -> Result<(), String>;
    /// The modes a client needs to encode keys correctly.
    fn key_modes(&self, session: &str) -> Option<terminal_core::keys::KeyModes>;
    /// Starts a session, or reattaches to one with the same id.
    fn spawn(&self, spec: SessionSpec) -> Result<String, String>;
    /// Ends a session's process.
    fn close(&self, session: &str) -> Result<(), String>;
    /// Detaches every attached client, leaving the process running.
    fn detach(&self, session: &str) -> Result<(), String>;
    /// Applies an agent state reported by an in-terminal integration.
    fn report_agent_state(
        &self,
        token: &str,
        agent: Option<String>,
        state: &str,
        message: Option<String>,
    ) -> Result<(), String>;
    /// Records the agent a session is running, as reported by its integration.
    fn report_agent_session(&self, token: &str, agent: &str) -> Result<(), String>;
}

/// The bearer token the endpoint accepts.
///
/// Read from disk on every request rather than captured at startup, so
/// `terry daemon rotate-token` revokes access immediately. Capturing it would
/// leave a leaked token valid until the next restart, which is exactly when the
/// user cannot afford it — rotating is the only way to revoke without stopping
/// every session the daemon owns.
#[derive(Clone, Debug)]
pub struct AuthToken {
    inner: Arc<AuthTokenInner>,
}

#[derive(Debug)]
enum AuthTokenInner {
    /// A fixed token, for tests and for an explicit `--token`.
    Fixed(String),
    /// A token read from this file on each request.
    File(PathBuf),
}

impl AuthToken {
    /// A token that never changes.
    pub fn fixed(token: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(AuthTokenInner::Fixed(token.into())),
        }
    }

    /// A token re-read from `path`, so rotating the file takes effect at once.
    ///
    /// A missing or unreadable file authorises nothing: if the file was deleted,
    /// failing closed is the safe answer for something that grants shell access.
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(AuthTokenInner::File(path.into())),
        }
    }

    /// Whether `candidate` is the current token.
    pub fn matches(&self, candidate: &str) -> bool {
        let expected = match self.inner.as_ref() {
            AuthTokenInner::Fixed(token) => Some(token.clone()),
            AuthTokenInner::File(path) => std::fs::read_to_string(path)
                .ok()
                .map(|token| token.trim().to_string()),
        };
        match expected {
            Some(expected) if !expected.is_empty() => {
                constant_time_eq(expected.as_bytes(), candidate.as_bytes())
            }
            _ => false,
        }
    }
}

/// Compares two byte strings without short-circuiting on the first difference.
///
/// The token is a bearer credential, so the comparison should not leak its
/// length or prefix through timing.
fn constant_time_eq(expected: &[u8], candidate: &[u8]) -> bool {
    if expected.len() != candidate.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in expected.iter().zip(candidate.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

/// Shared handler state.
#[derive(Clone)]
pub struct HttpState {
    pub sessions: Arc<dyn Sessions>,
    pub token: AuthToken,
    /// Fires whenever a session is added, removed or changes agent state.
    pub events: broadcast::Sender<SessionEvent>,
}

/// One change a client is told about.
#[derive(Clone, Debug)]
pub struct SessionEvent {
    /// `snapshot`, `agent_state`, `terminal_added` or `terminal_removed`.
    pub kind: &'static str,
    pub session: String,
    pub payload: Value,
}

/// A running listener.
pub struct Server {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stops accepting new connections. Open connections are left to finish.
    pub fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// Starts the listener on `bind` (`host:port`, port `0` picks a free one).
///
/// Runs on its own thread and tokio runtime so the daemon's synchronous
/// Unix-socket server is unaffected by a slow or stuck HTTP client.
pub fn serve(
    bind: &str,
    token: AuthToken,
    sessions: Arc<dyn Sessions>,
    mut events: broadcast::Receiver<SessionEvent>,
) -> Result<Server, String> {
    let listener = std::net::TcpListener::bind(bind)
        .map_err(|error| format!("failed to listen on {bind}: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;

    let (tx, _rx) = broadcast::channel(256);
    let state = HttpState {
        sessions,
        token,
        events: tx,
    };
    let app = build_router(state.clone());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                log::error!("daemon http: failed to start a runtime: {error}");
                return;
            }
        };
        runtime.block_on(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(error) => {
                    log::error!("daemon http: {error}");
                    return;
                }
            };

            // Relay daemon-side changes into the listener's broadcast channel so
            // the SSE handler has to know nothing about the session registry.
            let fanout = state.events.clone();
            tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    let _ = fanout.send(event);
                }
            });

            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            });
            if let Err(error) = server.await {
                log::error!("daemon http: {error}");
            }
        });
    });

    Ok(Server {
        port,
        shutdown: Some(shutdown_tx),
    })
}

fn build_router(state: HttpState) -> Router {
    let protected = Router::new()
        .route("/api/list_sessions", post(list_sessions))
        .route("/api/list_agent_states", post(list_agent_states))
        .route("/api/screen", post(screen))
        .route("/api/read_screen", post(read_screen))
        .route("/api/send_text", post(send_text))
        .route("/api/send_keys", post(send_keys))
        .route("/api/new_tab", post(new_tab))
        .route("/api/list_agents", post(list_agents))
        .route("/api/close_session", post(close_session))
        .route("/api/detach_session", post(detach_session))
        .route("/api/report_agent_state", post(report_agent_state))
        .route("/api/report_agent_session", post(report_agent_session))
        .route("/api/events", get(events))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    protected
        .route("/api/health", post(|| async { "OK" }))
        .with_state(state)
        .merge(assets_router())
}

/// The phone client, served from the daemon itself.
///
/// Embedding it keeps deployment to one step: a device opens
/// `http://<host>:<port>/` and gets the app. The files are unauthenticated on
/// purpose — they are static assets, and the page is where the token is entered.
/// Nothing here reads session data, so serving it without a token exposes
/// nothing that `/api/health` did not already.
fn assets_router() -> Router {
    Router::new()
        .route("/", get(index_html))
        .route("/app.css", get(app_css))
        .route("/app.js", get(app_js))
        .route("/manifest.webmanifest", get(manifest))
}

async fn index_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../../website/webapp/index.html"),
    )
}

async fn app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../../website/webapp/app.css"),
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../../website/webapp/app.js"),
    )
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../../../website/webapp/manifest.webmanifest"),
    )
}

async fn require_bearer_auth(
    State(state): State<HttpState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| state.token.matches(candidate));
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "missing or invalid bearer token" })),
        )
            .into_response();
    }
    next.run(request).await
}

/// Parses a JSON body, tolerating an empty one (clients send `{}` or nothing).
fn parse_body(body: &str) -> Result<Value, Response> {
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid JSON body: {error}") })),
        )
            .into_response()
    })
}

/// Reads `session` from the body, which every session-scoped route requires.
fn session_param(payload: &Value) -> Result<String, Response> {
    payload
        .get("session")
        .or_else(|| payload.get("terminal_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing \"session\"" })),
            )
                .into_response()
        })
}

fn summary_json(summary: &SessionSummary) -> Value {
    json!({
        "session": summary.session,
        "title": summary.title,
        "cwd": summary.cwd,
        "cols": summary.cols,
        "rows": summary.rows,
        "exited": summary.exited,
        "agent": summary.agent,
        "state": summary.state,
        "application_cursor": summary.application_cursor,
    })
}

async fn list_sessions(State(state): State<HttpState>, body: String) -> Response {
    if let Err(response) = parse_body(&body) {
        return response;
    }
    let sessions: Vec<Value> = state.sessions.list().iter().map(summary_json).collect();
    Json(json!({ "sessions": sessions })).into_response()
}

/// The endpoint a phone polls to know which agent is waiting for input.
async fn list_agent_states(State(state): State<HttpState>, body: String) -> Response {
    if let Err(response) = parse_body(&body) {
        return response;
    }
    // Only terminals a rule set or a report recognises; a plain shell is not an
    // agent and must not show up as an idle one.
    let terminals: Vec<Value> = state
        .sessions
        .list()
        .iter()
        .filter(|summary| summary.state.is_some())
        .map(summary_json)
        .collect();
    Json(json!({ "terminals": terminals })).into_response()
}

async fn screen(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.sessions.screen(&session) {
        Some(screen) => Json(screen).into_response(),
        None => unknown_session(&session),
    }
}

async fn read_screen(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let count = payload
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 1000) as usize;

    match state.sessions.read_screen(&session, count) {
        Some(lines) => Json(json!({
            "session": session,
            "content": lines.join("\n"),
            "lines": lines,
        }))
        .into_response(),
        None => unknown_session(&session),
    }
}

async fn send_text(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(text) = payload.get("text").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing \"text\"" })),
        )
            .into_response();
    };

    match state.sessions.send_text(&session, text) {
        Ok(()) => Json(json!({ "status": "ok", "action": "send_text" })).into_response(),
        // An unknown session is a 404 on every route, so a client can tell
        // "wrong id" apart from "bad request" without matching message text.
        Err(message) if message.starts_with("unknown session") => unknown_session(&session),
        Err(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
    }
}

/// Sends named keys, which is how a phone's key bar drives a terminal.
///
/// `{ "session": "...", "keys": ["escape", "ctrl-c"] }` or a single
/// `{ "keys": "ctrl-c" }`. Names are encoded against the session's modes, so
/// arrows work inside vim and less as well as at a plain prompt.
async fn send_keys(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };

    let keys: Vec<String> = match payload.get("keys") {
        Some(Value::String(key)) => vec![key.clone()],
        Some(Value::Array(keys)) => keys
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing \"keys\"" })),
            )
                .into_response();
        }
    };
    if keys.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "\"keys\" is empty" })),
        )
            .into_response();
    }

    match state.sessions.send_keys(&session, &keys) {
        Ok(()) => Json(json!({ "status": "ok", "action": "send_keys" })).into_response(),
        Err(message) if message.starts_with("unknown session") => unknown_session(&session),
        Err(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
    }
}

/// The coding agents this build knows how to launch.
///
/// A phone that offers "start an agent" needs the ids and display names without
/// hardcoding them, so the catalogue is served rather than duplicated in the
/// client. `launch` is the executable; the resume/past/fork templates are omitted
/// because a phone starting a fresh task has no session to continue.
async fn list_agents(State(_state): State<HttpState>, body: String) -> Response {
    if let Err(response) = parse_body(&body) {
        return response;
    }
    let agents: Vec<Value> = agent_catalog::coding_agents()
        .iter()
        .map(|agent| {
            json!({
                "id": agent.id,
                "launch": agent.bin,
                "names": agent.names,
                "can_resume": agent.resume.is_some(),
                // Whether the executable is on this daemon's PATH. A client that
                // offers "start an agent" should grey out the rest instead of
                // letting the user pick one that will fail to launch.
                "installed": on_path(agent.bin),
            })
        })
        .collect();
    Json(json!({ "agents": agents })).into_response()
}

async fn new_tab(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    // Either an `agent` id from the catalogue, or an explicit program. The
    // catalogue wins when both are given: it is the one that knows how the CLI
    // is actually invoked.
    let agent = payload.get("agent").and_then(Value::as_str);
    let (program, args) = match agent {
        Some(id) => match agent_catalog::agent_by_id(id) {
            Some(agent) => (Some(agent.bin.to_string()), Vec::new()),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("unknown agent {id:?}") })),
                )
                    .into_response();
            }
        },
        None => (
            payload
                .get("program")
                .and_then(Value::as_str)
                .map(str::to_string),
            payload
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        ),
    };
    let cols = payload.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = payload.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    let spec = SessionSpec {
        session: new_session_id(),
        cwd,
        program,
        args,
        env: Vec::new(),
        cols,
        rows,
    };
    let agent_id = agent.map(str::to_string);
    match state.sessions.spawn(spec) {
        Ok(session) => Json(json!({ "session": session })).into_response(),
        Err(message) => {
            // A missing executable is by far the most likely failure here, and
            // the raw OS error ("No such file or directory (os error 2)") does
            // not tell a phone user what to do about it.
            let hint = match &agent_id {
                Some(id) if message.contains("No such file") => {
                    let bin = agent_catalog::agent_by_id(id)
                        .map(|agent| agent.bin)
                        .unwrap_or(id.as_str());
                    Some(format!(
                        "{bin} is not installed, or not on the PATH of the process running the daemon"
                    ))
                }
                _ => None,
            };
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": message, "hint": hint })),
            )
                .into_response()
        }
    }
}

async fn close_session(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.sessions.close(&session) {
        Ok(()) => Json(json!({ "status": "ok", "action": "close_session" })).into_response(),
        Err(message) => unknown_session(&message),
    }
}

async fn detach_session(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let session = match session_param(&payload) {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.sessions.detach(&session) {
        Ok(()) => Json(json!({ "status": "ok", "action": "detach_session" })).into_response(),
        Err(message) => unknown_session(&message),
    }
}

/// Called by the agent integration scripts installed in `assets/agent-integrations`.
///
/// They send `{ token, state, agent?, message? }`, where `token` is the
/// per-terminal value the daemon injected as `TERRY_TERMINAL_TOKEN`.
async fn report_agent_state(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(token) = payload.get("token").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing \"token\"" })),
        )
            .into_response();
    };
    let Some(state_word) = payload.get("state").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing \"state\"" })),
        )
            .into_response();
    };
    let agent = payload
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);

    match state
        .sessions
        .report_agent_state(token, agent, state_word, message)
    {
        Ok(()) => Json(json!({ "status": "ok" })).into_response(),
        Err(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
    }
}

async fn report_agent_session(State(state): State<HttpState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(token) = payload.get("token").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing \"token\"" })),
        )
            .into_response();
    };
    let Some(agent) = payload.get("agent").and_then(Value::as_str) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing \"agent\"" })),
        )
            .into_response();
    };

    match state.sessions.report_agent_session(token, agent) {
        Ok(()) => Json(json!({ "status": "ok" })).into_response(),
        Err(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
    }
}

/// Server-sent events: one message per change, so a client can avoid polling.
///
/// A client must tolerate long quiet periods: the keep-alive below sends a
/// comment every 15s, and a stream can legitimately carry no data for minutes
/// when the agent is simply thinking. Treating "no data" as a dead connection
/// would make the phone reconnect in a loop.
async fn events(
    State(state): State<HttpState>,
) -> Sse<impl futures::Stream<Item = Result<SseEvent, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = futures::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let data = json!({
                        "type": event.kind,
                        "session": event.session,
                        "payload": event.payload,
                    });
                    let message = SseEvent::default().event(event.kind).data(data.to_string());
                    return Some((Ok(message), receiver));
                }
                // A slow client dropped messages; resuming is better than
                // killing the stream, and the client refetches on reconnect.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// Whether `bin` can be executed by this process.
///
/// Searches `PATH` the way `execvp` would: the agent is launched by the daemon,
/// so the daemon's environment is what decides whether it will start.
fn on_path(bin: &str) -> bool {
    if bin.contains('/') || bin.contains('\\') {
        return std::path::Path::new(bin).is_file();
    }
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(bin);
                if candidate.is_file() {
                    return true;
                }
                #[cfg(windows)]
                {
                    // Windows resolves the extension itself.
                    if candidate.with_extension("exe").is_file() {
                        return true;
                    }
                }
                false
            })
        })
        .unwrap_or(false)
}

fn unknown_session(session: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": format!("unknown session {session}") })),
    )
        .into_response()
}

/// Builds a [`SessionSummary`] for a session the daemon reports.
///
/// Shared with the Unix-socket server so both transports agree on what "agent
/// state" means.
pub fn summarize(
    info: &SessionInfo,
    title: String,
    cwd: Option<String>,
    reported: Option<&agents::Reported>,
    agent_hint: Option<&str>,
    lines: &[String],
    application_cursor: bool,
) -> SessionSummary {
    let status = agents::resolve(reported, agent_hint, &title, lines, info.idle_secs < 5);
    SessionSummary {
        session: info.session.clone(),
        title,
        cwd,
        cols: info.cols,
        rows: info.rows,
        exited: info.exited,
        agent: status.as_ref().and_then(|status| status.agent.clone()),
        state: status.as_ref().map(|status| status.state.as_str()),
        application_cursor,
    }
}
