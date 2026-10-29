use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures::{
    Stream,
    channel::{mpsc, oneshot},
    stream,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    net::TcpListener,
    path::PathBuf,
    time::{Duration, Instant},
};

/// The work an IPC request asks the GUI to perform.
#[derive(Debug)]
pub enum IpcRequestKind {
    /// Open a new terminal tab, optionally in `cwd`.
    NewTab { cwd: Option<PathBuf> },
    /// Paste text into a terminal. `terminal_id` defaults to the active one.
    SendText {
        terminal_id: Option<u64>,
        text: String,
    },
    /// Bring the running workspace window to the front (used when a second
    /// `terry` launch finds an existing instance).
    Focus,
    /// Focus a specific terminal, switching its group first if needed.
    FocusTerminal { terminal_id: Option<u64> },
    /// List groups and terminals in the active workspace window.
    ListSessions,
    /// Send text or a keystroke sequence to a terminal. `terminal_id` defaults
    /// to the active terminal.
    SendKeys {
        terminal_id: Option<u64>,
        keys: String,
    },
    /// Read the terminal's content as plain text. `lines` limits the result to
    /// the last N non-empty lines; `None` returns the whole buffer.
    ReadScreen {
        terminal_id: Option<u64>,
        lines: Option<u32>,
    },
    /// Report the active (or requested) terminal's title, cwd, and command.
    GetContext { terminal_id: Option<u64> },
    /// Post an OS notification.
    Notify {
        title: Option<String>,
        message: String,
    },
    /// Close a terminal without prompting. `terminal_id` defaults to the
    /// active terminal.
    CloseSession { terminal_id: Option<u64> },
    /// Detach a terminal while keeping its PTY alive.
    DetachSession { terminal_id: Option<u64> },
    /// Reattach a detached terminal into the active group.
    ReattachSession { terminal_id: u64 },
    /// List detached terminals.
    ListDetached,
    /// Rename a terminal tab.
    RenameSession {
        terminal_id: Option<u64>,
        title: Option<String>,
    },
    /// Switch to a group by id.
    FocusGroup { group_id: u64 },
    /// Create a group and its first terminal.
    NewGroup {
        name: Option<String>,
        cwd: Option<PathBuf>,
    },
    /// Delete a group and all its terminals.
    DeleteGroup { group_id: u64 },
    /// Rename a group.
    RenameGroup { group_id: u64, name: String },
    /// Move a terminal to another group.
    MoveSession { terminal_id: u64, group_id: u64 },
    /// Set a group's tiling layout.
    SetGroupLayout { group_id: u64, layout: String },
    /// Enable or disable broadcast input for a group (default: active group).
    SetBroadcast {
        group_id: Option<u64>,
        enabled: bool,
    },
    /// Scroll a terminal.
    Scroll {
        terminal_id: Option<u64>,
        to: String,
    },
    /// Set or clear a terminal's font-size override.
    SetFontSize {
        terminal_id: Option<u64>,
        size: Option<f32>,
    },
    /// Set or clear a terminal's color scheme.
    SetColorScheme {
        terminal_id: Option<u64>,
        name: Option<String>,
    },
    /// Paste text into a terminal and optionally submit it with Enter.
    Run {
        terminal_id: Option<u64>,
        text: String,
        enter: bool,
    },
    /// Open a new workspace window.
    NewWindow,
    /// List saved session templates.
    ListSessionTemplates,
    /// Save the active group as a session template.
    SaveSessionTemplate { name: String },
    /// Create a group from a saved session template.
    LoadSessionTemplate { name: String },
    /// Delete a saved session template.
    DeleteSessionTemplate { name: String },
    /// List terminals running a coding agent with their effective state.
    ListAgentStates,
    /// An agent (via its integration hook) reports its own state.
    ReportAgentState {
        token: String,
        state: String,
        message: Option<String>,
        agent: Option<String>,
    },
    /// An agent reports the session it can be resumed from.
    ReportAgentSession {
        token: String,
        agent: String,
        session_path: Option<String>,
        session_id: Option<String>,
    },
}

/// A request forwarded from the HTTP server to the GUI event loop. The HTTP
/// handler parks on `reply` until the GUI answers; `Ok` becomes a 200 and
/// `Err` a 400 carrying `{"error": ...}`.
pub struct IpcRequest {
    pub kind: IpcRequestKind,
    pub reply: oneshot::Sender<Result<Value, String>>,
}

#[derive(Clone)]
struct IpcState {
    token: String,
    requests: mpsc::UnboundedSender<IpcRequest>,
}

/// Starts the loopback IPC HTTP server on its own thread + tokio runtime.
///
/// Handlers never touch gpui directly — every request is forwarded through the
/// `requests` channel and answered via its oneshot reply, because the gpui
/// `AsyncApp` is not `Send` and must stay on the main thread. Returns
/// `(port, bearer token)`; the caller persists both in `ipc.json`.
pub fn start_ipc_server(
    requests: mpsc::UnboundedSender<IpcRequest>,
) -> Result<(u16, String), String> {
    let token = uuid::Uuid::new_v4().to_string();
    let state = IpcState {
        token: token.clone(),
        requests,
    };

    // Every route below requires the bearer token persisted in ipc.json.
    let protected = Router::new()
        .route("/api/new_tab", post(new_tab))
        .route("/api/send_text", post(send_text))
        .route("/api/focus", post(focus))
        .route("/api/list_sessions", post(list_sessions))
        .route("/api/focus_session", post(focus_session))
        .route("/api/send_keys", post(send_keys))
        .route("/api/read_screen", post(read_screen))
        .route("/api/get_context", post(get_context))
        .route("/api/notify", post(notify))
        .route("/api/close_session", post(close_session))
        .route("/api/close_terminal", post(close_session))
        .route("/api/detach_session", post(detach_session))
        .route("/api/reattach_session", post(reattach_session))
        .route("/api/list_detached", post(list_detached))
        .route("/api/rename_session", post(rename_session))
        .route("/api/focus_group", post(focus_group))
        .route("/api/new_group", post(new_group))
        .route("/api/delete_group", post(delete_group))
        .route("/api/close_group", post(delete_group))
        .route("/api/rename_group", post(rename_group))
        .route("/api/move_session", post(move_session))
        .route("/api/set_group_layout", post(set_group_layout))
        .route("/api/set_broadcast", post(set_broadcast))
        .route("/api/scroll", post(scroll))
        .route("/api/set_font_size", post(set_font_size))
        .route("/api/set_color_scheme", post(set_color_scheme))
        .route("/api/run", post(run))
        .route("/api/new_window", post(new_window))
        .route("/api/list_session_templates", post(list_session_templates))
        .route("/api/save_session_template", post(save_session_template))
        .route("/api/load_session_template", post(load_session_template))
        .route(
            "/api/delete_session_template",
            post(delete_session_template),
        )
        .route("/api/create_new_terminal", post(create_new_terminal))
        .route("/api/list_agent_states", post(list_agent_states))
        .route("/api/report_agent_state", post(report_agent_state))
        .route("/api/report_agent_session", post(report_agent_session))
        .route("/api/wait_agent_state", post(wait_agent_state))
        .route("/api/wait_for_output", post(wait_for_output))
        .route("/api/events", get(events))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    let app = protected
        .route("/api/health", post(|| async { "OK" }))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    listener.set_nonblocking(true).unwrap();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let tokio_listener = tokio::net::TcpListener::from_std(listener).unwrap();
            if let Err(error) = axum::serve(tokio_listener, app).await {
                eprintln!("terry ipc server stopped: {error}");
            }
        });
    });

    Ok((port, token))
}

async fn require_bearer_auth(
    State(state): State<IpcState>,
    request: Request,
    next: Next,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing or invalid bearer token"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn new_tab(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    dispatch(state, IpcRequestKind::NewTab { cwd }).await
}

async fn send_text(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match payload.get("text").and_then(Value::as_str) {
        Some(text) => {
            dispatch(
                state,
                IpcRequestKind::SendText {
                    terminal_id: parse_terminal_id(&payload),
                    text: text.to_string(),
                },
            )
            .await
        }
        None => bad_request("missing \"text\" field".to_string()),
    }
}

async fn focus(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::Focus).await
}

/// An empty body is treated as `{}` so `curl -X POST` without data works.
fn parse_body(body: &str) -> Result<Value, Response> {
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(body).map_err(|error| bad_request(format!("invalid JSON body: {error}")))
}

async fn dispatch(state: IpcState, kind: IpcRequestKind) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    if state
        .requests
        .unbounded_send(IpcRequest {
            kind,
            reply: reply_tx,
        })
        .is_err()
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "terry ipc channel is closed"})),
        )
            .into_response();
    }
    match reply_rx.await {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err(message)) => bad_request(message),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "terry did not reply to the ipc request"})),
        )
            .into_response(),
    }
}

fn bad_request(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

async fn list_sessions(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::ListSessions).await
}

async fn focus_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    dispatch(
        state,
        IpcRequestKind::FocusTerminal {
            terminal_id: parse_terminal_id(&payload),
        },
    )
    .await
}

async fn send_keys(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(keys) = payload.get("keys").and_then(Value::as_str) else {
        return bad_request("missing \"keys\" field".to_string());
    };
    if keys.is_empty() {
        return bad_request("\"keys\" must not be empty".to_string());
    }
    dispatch(
        state,
        IpcRequestKind::SendKeys {
            terminal_id: parse_terminal_id(&payload),
            keys: keys.to_string(),
        },
    )
    .await
}

async fn read_screen(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    dispatch(
        state,
        IpcRequestKind::ReadScreen {
            terminal_id: parse_terminal_id(&payload),
            lines: payload
                .get("lines")
                .and_then(parse_u64_value)
                .map(|lines| lines.min(u32::MAX as u64) as u32),
        },
    )
    .await
}

async fn get_context(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    dispatch(
        state,
        IpcRequestKind::GetContext {
            terminal_id: parse_terminal_id(&payload),
        },
    )
    .await
}

async fn notify(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(message) = payload.get("message").and_then(Value::as_str) else {
        return bad_request("missing \"message\" field".to_string());
    };
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    dispatch(
        state,
        IpcRequestKind::Notify {
            title,
            message: message.to_string(),
        },
    )
    .await
}

async fn create_new_terminal(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    dispatch(state, IpcRequestKind::NewTab { cwd }).await
}

async fn close_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    dispatch(
        state,
        IpcRequestKind::CloseSession {
            terminal_id: parse_terminal_id(&payload),
        },
    )
    .await
}

async fn detach_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    dispatch(
        state,
        IpcRequestKind::DetachSession {
            terminal_id: parse_terminal_id(&payload),
        },
    )
    .await
}

async fn reattach_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(terminal_id) = parse_terminal_id(&payload) else {
        return bad_request("missing \"terminal_id\" field".to_string());
    };
    dispatch(state, IpcRequestKind::ReattachSession { terminal_id }).await
}

async fn list_detached(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::ListDetached).await
}

async fn rename_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let title = match payload.get("title") {
        Some(Value::Null) => None,
        Some(Value::String(title)) => Some(title.clone()),
        Some(_) => return bad_request("\"title\" must be a string or null".to_string()),
        None => return bad_request("missing \"title\" field".to_string()),
    };
    dispatch(
        state,
        IpcRequestKind::RenameSession {
            terminal_id: parse_terminal_id(&payload),
            title,
        },
    )
    .await
}

async fn focus_group(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(group_id) = parse_group_id(&payload) else {
        return bad_request("missing \"group_id\" field".to_string());
    };
    dispatch(state, IpcRequestKind::FocusGroup { group_id }).await
}

async fn new_group(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let name = match payload.get("name") {
        Some(Value::Null) | None => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => return bad_request("\"name\" must be a string or null".to_string()),
    };
    let cwd = match payload.get("cwd") {
        Some(Value::Null) | None => None,
        Some(Value::String(cwd)) => Some(PathBuf::from(cwd)),
        Some(_) => return bad_request("\"cwd\" must be a string or null".to_string()),
    };
    dispatch(state, IpcRequestKind::NewGroup { name, cwd }).await
}

async fn delete_group(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(group_id) = parse_group_id(&payload) else {
        return bad_request("missing \"group_id\" field".to_string());
    };
    dispatch(state, IpcRequestKind::DeleteGroup { group_id }).await
}

async fn rename_group(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(group_id) = parse_group_id(&payload) else {
        return bad_request("missing \"group_id\" field".to_string());
    };
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return bad_request("missing \"name\" field".to_string());
    };
    if name.trim().is_empty() {
        return bad_request("\"name\" must not be empty".to_string());
    }
    dispatch(
        state,
        IpcRequestKind::RenameGroup {
            group_id,
            name: name.to_string(),
        },
    )
    .await
}

async fn move_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(terminal_id) = parse_terminal_id(&payload) else {
        return bad_request("missing \"terminal_id\" field".to_string());
    };
    let Some(group_id) = parse_group_id(&payload) else {
        return bad_request("missing \"group_id\" field".to_string());
    };
    dispatch(
        state,
        IpcRequestKind::MoveSession {
            terminal_id,
            group_id,
        },
    )
    .await
}

async fn set_group_layout(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(group_id) = parse_group_id(&payload) else {
        return bad_request("missing \"group_id\" field".to_string());
    };
    let Some(layout) = payload.get("layout").and_then(Value::as_str) else {
        return bad_request("missing \"layout\" field".to_string());
    };
    if !matches!(
        layout,
        "manual" | "splits" | "tall" | "fat" | "grid" | "stack" | "horizontal" | "vertical"
    ) {
        return bad_request(format!("unknown layout {layout:?}"));
    }
    dispatch(
        state,
        IpcRequestKind::SetGroupLayout {
            group_id,
            layout: layout.to_string(),
        },
    )
    .await
}

async fn set_broadcast(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) else {
        return bad_request("missing boolean \"enabled\" field".to_string());
    };
    dispatch(
        state,
        IpcRequestKind::SetBroadcast {
            group_id: parse_group_id(&payload),
            enabled,
        },
    )
    .await
}

async fn scroll(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(to) = payload.get("to").and_then(Value::as_str) else {
        return bad_request("missing \"to\" field".to_string());
    };
    if !matches!(
        to,
        "top" | "bottom" | "page_up" | "page_down" | "line_up" | "line_down"
    ) {
        return bad_request(format!("unknown scroll target {to:?}"));
    }
    dispatch(
        state,
        IpcRequestKind::Scroll {
            terminal_id: parse_terminal_id(&payload),
            to: to.to_string(),
        },
    )
    .await
}

async fn set_font_size(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let size = match payload.get("size") {
        Some(Value::Null) => None,
        Some(value) => {
            let Some(size) = value.as_f64() else {
                return bad_request("\"size\" must be a number or null".to_string());
            };
            if !size.is_finite() || !(0.0 < size && size <= 200.0) {
                return bad_request("\"size\" must be greater than 0 and at most 200".to_string());
            }
            Some(size as f32)
        }
        None => return bad_request("missing \"size\" field".to_string()),
    };
    dispatch(
        state,
        IpcRequestKind::SetFontSize {
            terminal_id: parse_terminal_id(&payload),
            size,
        },
    )
    .await
}

async fn set_color_scheme(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let name = match payload.get("name") {
        Some(Value::Null) | None => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => return bad_request("\"name\" must be a string or null".to_string()),
    };
    dispatch(
        state,
        IpcRequestKind::SetColorScheme {
            terminal_id: parse_terminal_id(&payload),
            name,
        },
    )
    .await
}

async fn run(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(text) = payload.get("text").and_then(Value::as_str) else {
        return bad_request("missing \"text\" field".to_string());
    };
    if text.is_empty() {
        return bad_request("\"text\" must not be empty".to_string());
    }
    let enter = payload
        .get("enter")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    dispatch(
        state,
        IpcRequestKind::Run {
            terminal_id: parse_terminal_id(&payload),
            text: text.to_string(),
            enter,
        },
    )
    .await
}

async fn new_window(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::NewWindow).await
}

async fn list_session_templates(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::ListSessionTemplates).await
}

async fn save_session_template(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return bad_request("missing \"name\" field".to_string());
    };
    if name.trim().is_empty() {
        return bad_request("\"name\" must not be empty".to_string());
    }
    dispatch(
        state,
        IpcRequestKind::SaveSessionTemplate {
            name: name.to_string(),
        },
    )
    .await
}

async fn load_session_template(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return bad_request("missing \"name\" field".to_string());
    };
    dispatch(
        state,
        IpcRequestKind::LoadSessionTemplate {
            name: name.to_string(),
        },
    )
    .await
}

async fn delete_session_template(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let Some(name) = payload.get("name").and_then(Value::as_str) else {
        return bad_request("missing \"name\" field".to_string());
    };
    dispatch(
        state,
        IpcRequestKind::DeleteSessionTemplate {
            name: name.to_string(),
        },
    )
    .await
}

// -------------------------------------------------------------------------
// Agent state: list, wait, events
// -------------------------------------------------------------------------

/// How often `wait_agent_state` re-reads the terminal list.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How often the event stream re-reads the terminal list.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Default `wait_agent_state` timeout. `0` disables the timeout.
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 120_000;

/// States accepted by `wait_agent_state`. `any`/`change` match any state that
/// differs from the one seen on the first poll.
const WAITABLE_STATES: &[&str] = &["idle", "working", "blocked", "any", "change"];

async fn list_agent_states(State(state): State<IpcState>) -> Response {
    dispatch(state, IpcRequestKind::ListAgentStates).await
}

/// Agent integrations report the session they can be resumed from here.
async fn report_agent_session(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };

    let Some(token) = payload.get("token").and_then(Value::as_str) else {
        return bad_request("missing \"token\" field".to_string());
    };
    let Some(agent) = payload.get("agent").and_then(Value::as_str) else {
        return bad_request("missing \"agent\" field".to_string());
    };
    let session_path = payload
        .get("session_path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if session_path.is_none() && session_id.is_none() {
        return bad_request("missing \"session_path\" or \"session_id\"".to_string());
    }

    dispatch(
        state,
        IpcRequestKind::ReportAgentSession {
            token: token.to_string(),
            agent: agent.to_string(),
            session_path,
            session_id,
        },
    )
    .await
}

/// Agent integrations (e.g. the Pi extension) report lifecycle state here.
///
/// `token` comes from `TERRY_TERMINAL_TOKEN`, which Terry injects into the
/// terminal's environment, so a report is attributed to the right terminal.
async fn report_agent_state(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };

    let Some(token) = payload.get("token").and_then(Value::as_str) else {
        return bad_request("missing \"token\" field".to_string());
    };
    let Some(agent_state) = payload.get("state").and_then(Value::as_str) else {
        return bad_request("missing \"state\" field".to_string());
    };
    if !matches!(agent_state, "working" | "blocked" | "idle") {
        return bad_request("state must be working, blocked, or idle".to_string());
    }

    dispatch(
        state,
        IpcRequestKind::ReportAgentState {
            token: token.to_string(),
            state: agent_state.to_string(),
            message: payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
            agent: payload
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    )
    .await
}

/// Blocks until a terminal's agent reaches one of the requested states.
///
/// The wait is implemented as a poll loop on the server thread: every poll is a
/// separate request through the IPC channel, so a long wait never stalls the
/// GUI's request loop.
async fn wait_agent_state(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };

    let terminal_id = parse_terminal_id(&payload);
    let wanted = match parse_wanted_states(&payload) {
        Ok(wanted) => wanted,
        Err(message) => return bad_request(message),
    };
    let timeout_ms = payload
        .get("timeout_ms")
        .and_then(parse_u64_value)
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };

    let started = Instant::now();
    let mut initial_state: Option<String> = None;

    loop {
        let snapshot = match request_value(&state, IpcRequestKind::ListAgentStates).await {
            Ok(snapshot) => snapshot,
            Err(message) => return bad_request(message),
        };

        let Some(terminal) = find_terminal(&snapshot, terminal_id) else {
            let message = match terminal_id {
                Some(_) => "no agent terminal matches the requested id",
                None => "the active terminal is not running a coding agent",
            };
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": message,
                    "terminal_id": terminal_id,
                })),
            )
                .into_response();
        };

        let current = terminal
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let exited = terminal
            .get("exited")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let id = terminal.get("terminal_id").cloned().unwrap_or(Value::Null);

        let initial = initial_state.get_or_insert_with(|| current.clone());
        let matched = wanted.iter().any(|want| match want.as_str() {
            "any" | "change" => initial != &current,
            other => other == current,
        });

        if matched || exited {
            return (
                StatusCode::OK,
                Json(json!({
                    "terminal_id": id,
                    "state": current,
                    "exited": exited,
                    "matched": matched,
                    "waited_ms": started.elapsed().as_millis() as u64,
                })),
            )
                .into_response();
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({
                    "error": format!("timed out waiting for {}", wanted.join(" or ")),
                    "terminal_id": id,
                    "state": current,
                    "timed_out": true,
                    "waited_ms": started.elapsed().as_millis() as u64,
                })),
            )
                .into_response();
        }

        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
}

/// How often `wait_for_output` re-reads the terminal.
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// How many bottom lines `wait_for_output` reads by default. Matching only the
/// tail avoids matching output that has already scrolled far away.
const DEFAULT_OUTPUT_TAIL_LINES: u64 = 50;

/// Matches `wait_for_output` against the terminal text.
enum OutputMatcher {
    Substring(String),
    Regex(regex::Regex),
}

impl OutputMatcher {
    fn find<'a>(&self, text: &'a str) -> Option<&'a str> {
        match self {
            Self::Substring(needle) => text.lines().find(|line| line.contains(needle.as_str())),
            Self::Regex(regex) => text.lines().find(|line| regex.is_match(line)),
        }
    }
}

fn parse_output_matcher(payload: &Value) -> Result<OutputMatcher, String> {
    if let Some(pattern) = payload.get("regex").and_then(Value::as_str) {
        return regex::Regex::new(pattern)
            .map(OutputMatcher::Regex)
            .map_err(|error| format!("invalid regex: {error}"));
    }
    if let Some(needle) = payload.get("match").and_then(Value::as_str) {
        if needle.is_empty() {
            return Err("\"match\" must not be empty".to_string());
        }
        return Ok(OutputMatcher::Substring(needle.to_string()));
    }
    Err("missing \"match\" (substring) or \"regex\" field".to_string())
}

/// Blocks until the terminal's recent output matches a substring or regex.
async fn wait_for_output(State(state): State<IpcState>, body: String) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(response) => return response,
    };

    let terminal_id = parse_terminal_id(&payload);
    let matcher = match parse_output_matcher(&payload) {
        Ok(matcher) => matcher,
        Err(message) => return bad_request(message),
    };
    let tail_lines = payload
        .get("tail_lines")
        .and_then(parse_u64_value)
        .unwrap_or(DEFAULT_OUTPUT_TAIL_LINES);
    let timeout_ms = payload
        .get("timeout_ms")
        .and_then(parse_u64_value)
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };

    let started = Instant::now();

    loop {
        let snapshot = match request_value(
            &state,
            IpcRequestKind::ReadScreen {
                terminal_id,
                lines: (tail_lines > 0).then_some(tail_lines.min(u32::MAX as u64) as u32),
            },
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(message) => return bad_request(message),
        };

        let id = snapshot.get("terminal_id").cloned().unwrap_or(Value::Null);
        let content = snapshot
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if let Some(line) = matcher.find(content) {
            return (
                StatusCode::OK,
                Json(json!({
                    "terminal_id": id,
                    "matched": true,
                    "line": line,
                    "waited_ms": started.elapsed().as_millis() as u64,
                })),
            )
                .into_response();
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({
                    "error": "timed out waiting for output",
                    "terminal_id": id,
                    "timed_out": true,
                    "waited_ms": started.elapsed().as_millis() as u64,
                })),
            )
                .into_response();
        }

        tokio::time::sleep(OUTPUT_POLL_INTERVAL).await;
    }
}

fn parse_wanted_states(payload: &Value) -> Result<Vec<String>, String> {
    let mut wanted = Vec::new();
    match payload.get("states") {
        Some(Value::Array(items)) => {
            for item in items {
                let Some(state) = item.as_str() else {
                    return Err("\"states\" must be an array of strings".to_string());
                };
                wanted.push(state.to_ascii_lowercase());
            }
        }
        Some(_) => return Err("\"states\" must be an array of strings".to_string()),
        None => {
            if let Some(state) = payload.get("state").and_then(Value::as_str) {
                wanted.push(state.to_ascii_lowercase());
            }
        }
    }

    if wanted.is_empty() {
        wanted.push("blocked".to_string());
    }
    for state in &wanted {
        if !WAITABLE_STATES.contains(&state.as_str()) {
            return Err(format!(
                "unknown agent state {state:?}; expected one of {WAITABLE_STATES:?}"
            ));
        }
    }
    Ok(wanted)
}

/// Finds the requested terminal, defaulting to the workspace's active one.
fn find_terminal<'a>(snapshot: &'a Value, terminal_id: Option<u64>) -> Option<&'a Value> {
    let terminals = snapshot.get("terminals")?.as_array()?;
    let id = terminal_id.or_else(|| snapshot.get("active_terminal_id").and_then(Value::as_u64))?;
    terminals
        .iter()
        .find(|terminal| terminal.get("terminal_id").and_then(Value::as_u64) == Some(id))
}

/// Sends one request through the IPC channel and awaits its reply, without
/// turning failures into an HTTP response.
async fn request_value(state: &IpcState, kind: IpcRequestKind) -> Result<Value, String> {
    let (reply_tx, reply_rx) = oneshot::channel();
    state
        .requests
        .unbounded_send(IpcRequest {
            kind,
            reply: reply_tx,
        })
        .map_err(|_| "terry ipc channel is closed".to_string())?;
    match reply_rx.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(message),
        Err(_) => Err("terry did not reply to the ipc request".to_string()),
    }
}

struct EventStreamState {
    state: IpcState,
    previous: Option<Value>,
    pending: VecDeque<(String, String)>,
}

/// Server-sent events for agent state changes.
///
/// The first event is a `snapshot` of every agent terminal; after that,
/// `agent_state` fires when a terminal's state, title or exit status changes,
/// and `terminal_added` / `terminal_removed` cover lifecycle changes.
async fn events(
    State(state): State<IpcState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = EventStreamState {
        state,
        previous: None,
        pending: VecDeque::new(),
    };

    let stream = stream::unfold(initial, |mut stream_state| async move {
        loop {
            if let Some((name, data)) = stream_state.pending.pop_front() {
                let event = Event::default().event(name).data(data);
                return Some((Ok::<_, Infallible>(event), stream_state));
            }

            if let Ok(snapshot) =
                request_value(&stream_state.state, IpcRequestKind::ListAgentStates).await
            {
                stream_state.pending = diff_agent_events(stream_state.previous.as_ref(), &snapshot);
                stream_state.previous = Some(snapshot);
            }

            if stream_state.pending.is_empty() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn terminals_by_id(value: &Value) -> HashMap<u64, &Value> {
    value
        .get("terminals")
        .and_then(Value::as_array)
        .map(|terminals| {
            terminals
                .iter()
                .filter_map(|terminal| {
                    terminal
                        .get("terminal_id")
                        .and_then(Value::as_u64)
                        .map(|id| (id, terminal))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diff_agent_events(previous: Option<&Value>, current: &Value) -> VecDeque<(String, String)> {
    let mut events = VecDeque::new();
    let Some(previous) = previous else {
        events.push_back(("snapshot".to_string(), current.to_string()));
        return events;
    };

    let previous_terminals = terminals_by_id(previous);
    let current_terminals = terminals_by_id(current);

    for (id, terminal) in &current_terminals {
        match previous_terminals.get(id) {
            None => events.push_back(("terminal_added".to_string(), terminal.to_string())),
            Some(old) => {
                let changed = ["state", "exited", "title"]
                    .iter()
                    .any(|key| old.get(*key) != terminal.get(*key));
                if changed {
                    events.push_back(("agent_state".to_string(), terminal.to_string()));
                }
            }
        }
    }

    for id in previous_terminals.keys() {
        if !current_terminals.contains_key(id) {
            events.push_back((
                "terminal_removed".to_string(),
                json!({ "terminal_id": id }).to_string(),
            ));
        }
    }

    events
}

fn parse_terminal_id(payload: &Value) -> Option<u64> {
    payload
        .get("terminal_id")
        .or_else(|| payload.get("session_id"))
        .and_then(parse_u64_value)
}

fn parse_group_id(payload: &Value) -> Option<u64> {
    payload.get("group_id").and_then(parse_u64_value)
}

fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct TestServer {
        port: u16,
        token: String,
        requests: mpsc::UnboundedReceiver<IpcRequest>,
    }

    fn spawn_server() -> TestServer {
        let (tx, requests) = mpsc::unbounded();
        let (port, token) = start_ipc_server(tx).expect("ipc server starts");
        TestServer {
            port,
            token,
            requests,
        }
    }

    /// Sends one HTTP request on a raw socket and returns (status, raw
    /// response). The connection is closed after one request.
    async fn post(port: u16, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");

        let mut request = format!(
            "POST {path} HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             ",
            body.len()
        );
        if let Some(token) = token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);

        stream.write_all(request.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw))
            .await
            .expect("read timed out")
            .unwrap();
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let status = raw
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .expect("HTTP status line");
        (status, raw)
    }

    #[tokio::test]
    async fn health_is_public() {
        let server = spawn_server();
        let (status, _) = post(server.port, "/api/health", None, "").await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn missing_token_is_unauthorized() {
        let server = spawn_server();
        let (status, body) = post(server.port, "/api/new_tab", None, "{}").await;
        assert_eq!(status, 401);
        assert!(body.contains("bearer"), "body: {body}");
    }

    #[tokio::test]
    async fn wrong_token_is_unauthorized() {
        let server = spawn_server();
        let (status, _) = post(server.port, "/api/new_tab", Some("wrong-token"), "{}").await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn new_tab_is_forwarded_to_the_app() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/new_tab", Some(&token), r#"{"cwd": "/tmp"}"#);
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            let IpcRequestKind::NewTab { cwd } = request.kind else {
                panic!("expected a NewTab request");
            };
            assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp")));
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
        assert!(
            body.to_ascii_lowercase().contains("content-length:"),
            "responses must carry content-length for the raw CLI client: {body}"
        );
        assert!(
            !body.to_ascii_lowercase().contains("transfer-encoding"),
            "chunked responses would break the minimal CLI parser: {body}"
        );
        assert!(body.contains("\"status\":\"ok\""), "body: {body}");
    }

    #[tokio::test]
    async fn app_error_reply_becomes_400() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/new_tab", Some(&token), "{}");
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            request
                .reply
                .send(Err("no such directory".to_string()))
                .ok();
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 400);
        assert!(body.contains("no such directory"), "body: {body}");
    }

    #[tokio::test]
    async fn send_text_requires_text_field() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        drop(requests);

        let (status, _) = post(port, "/api/send_text", Some(&token), "{}").await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn send_keys_parses_session_id_and_forwards_keys() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/send_keys",
            Some(&token),
            r#"{"session_id":"42","keys":"ctrl-c"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::SendKeys { terminal_id, keys } => {
                    assert_eq!(terminal_id, Some(42));
                    assert_eq!(keys, "ctrl-c");
                }
                other => panic!("expected SendKeys, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn close_session_forwards_terminal_id() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/close_session",
            Some(&token),
            r#"{"session_id":"7"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::CloseSession { terminal_id } => {
                    assert_eq!(terminal_id, Some(7));
                }
                other => panic!("expected CloseSession, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn new_group_forwards_name_and_cwd() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/new_group",
            Some(&token),
            r#"{"name":"Work","cwd":"/tmp"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::NewGroup { name, cwd } => {
                    assert_eq!(name.as_deref(), Some("Work"));
                    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp")));
                }
                other => panic!("expected NewGroup, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn save_session_template_forwards_name() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/save_session_template",
            Some(&token),
            r#"{"name":"Work"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::SaveSessionTemplate { name } => assert_eq!(name, "Work"),
                other => panic!("expected SaveSessionTemplate, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn set_group_layout_rejects_unknown_layout() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        drop(requests);

        let (status, body) = post(
            port,
            "/api/set_group_layout",
            Some(&token),
            r#"{"group_id":0,"layout":"diagonal"}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("unknown layout"), "body: {body}");
    }

    #[tokio::test]
    async fn set_font_size_accepts_null_to_clear_the_override() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/set_font_size", Some(&token), r#"{"size":null}"#);
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::SetFontSize { size, .. } => assert_eq!(size, None),
                other => panic!("expected SetFontSize, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    fn agent_snapshot(terminals: &[(u64, &str, &str)]) -> Value {
        let terminals: Vec<Value> = terminals
            .iter()
            .map(|(id, state, title)| {
                json!({
                    "terminal_id": id,
                    "title": title,
                    "group": "main",
                    "agent": "Claude",
                    "state": state,
                    "exited": false,
                })
            })
            .collect();
        json!({ "active_terminal_id": 1, "terminals": terminals })
    }

    #[tokio::test]
    async fn list_agent_states_is_forwarded() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/list_agent_states", Some(&token), "{}");
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            assert!(matches!(request.kind, IpcRequestKind::ListAgentStates));
            request
                .reply
                .send(Ok(agent_snapshot(&[(1, "blocked", "claude")])))
                .ok();
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
        assert!(body.contains("\"blocked\""), "body: {body}");
    }

    #[tokio::test]
    async fn wait_agent_state_returns_when_the_state_matches() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_agent_state",
            Some(&token),
            r#"{"state":"blocked","timeout_ms":5000}"#,
        );
        let app_side = async {
            for call in 0..3 {
                let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                    .await
                    .expect("ipc request never arrived")
                    .expect("ipc channel closed");
                assert!(matches!(request.kind, IpcRequestKind::ListAgentStates));
                let state = if call < 2 { "working" } else { "blocked" };
                request
                    .reply
                    .send(Ok(agent_snapshot(&[(1, state, "claude")])))
                    .ok();
            }
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
        assert!(body.contains("\"matched\":true"), "body: {body}");
        assert!(body.contains("\"state\":\"blocked\""), "body: {body}");
    }

    #[tokio::test]
    async fn wait_agent_state_times_out() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_agent_state",
            Some(&token),
            r#"{"state":"blocked","timeout_ms":150}"#,
        );
        let app_side = async {
            for _ in 0..10 {
                let Ok(Some(request)) =
                    tokio::time::timeout(Duration::from_millis(500), requests.next()).await
                else {
                    break;
                };
                request
                    .reply
                    .send(Ok(agent_snapshot(&[(1, "working", "claude")])))
                    .ok();
            }
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 408);
        assert!(body.contains("timed_out"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_agent_state_rejects_unknown_states() {
        let server = spawn_server();
        let (status, body) = post(
            server.port,
            "/api/wait_agent_state",
            Some(&server.token),
            r#"{"state":"sleeping"}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("unknown agent state"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_agent_state_reports_a_missing_terminal() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_agent_state",
            Some(&token),
            r#"{"terminal_id":99}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            request
                .reply
                .send(Ok(agent_snapshot(&[(1, "idle", "claude")])))
                .ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 404);
    }

    #[test]
    fn event_diff_reports_state_changes_and_lifecycle() {
        let first = agent_snapshot(&[(1, "working", "claude")]);
        let mut events = diff_agent_events(None, &first);
        assert_eq!(
            events.pop_front().map(|(name, _)| name),
            Some("snapshot".to_string())
        );

        let second = agent_snapshot(&[(1, "blocked", "claude"), (2, "idle", "codex")]);
        let events = diff_agent_events(Some(&first), &second);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"agent_state"), "names: {names:?}");
        assert!(names.contains(&"terminal_added"), "names: {names:?}");

        let third = agent_snapshot(&[(1, "blocked", "claude")]);
        let events = diff_agent_events(Some(&second), &third);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["terminal_removed"]);
    }

    #[tokio::test]
    async fn send_text_forwards_terminal_id() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/send_text",
            Some(&token),
            r#"{"terminal_id":7,"text":"hi"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::SendText { terminal_id, text } => {
                    assert_eq!(terminal_id, Some(7));
                    assert_eq!(text, "hi");
                }
                other => panic!("expected SendText, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn read_screen_forwards_line_limit() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/read_screen", Some(&token), r#"{"lines":20}"#);
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::ReadScreen { terminal_id, lines } => {
                    assert_eq!(terminal_id, None);
                    assert_eq!(lines, Some(20));
                }
                other => panic!("expected ReadScreen, got {other:?}"),
            }
            request
                .reply
                .send(Ok(json!({ "terminal_id": 1, "content": "" })))
                .ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn wait_for_output_matches_a_substring() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_for_output",
            Some(&token),
            r#"{"match":"BUILD OK","timeout_ms":5000}"#,
        );
        let app_side = async {
            for content in ["compiling", "compiling\nBUILD OK"] {
                let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                    .await
                    .expect("ipc request never arrived")
                    .expect("ipc channel closed");
                match request.kind {
                    IpcRequestKind::ReadScreen { terminal_id, lines } => {
                        assert_eq!(terminal_id, None);
                        assert_eq!(lines, Some(DEFAULT_OUTPUT_TAIL_LINES as u32));
                    }
                    other => panic!("expected ReadScreen, got {other:?}"),
                }
                request
                    .reply
                    .send(Ok(json!({ "terminal_id": 1, "content": content })))
                    .ok();
            }
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
        assert!(body.contains("\"matched\":true"), "body: {body}");
        assert!(body.contains("BUILD OK"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_for_output_matches_a_regex() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_for_output",
            Some(&token),
            r#"{"regex":"BUILD (OK|FAILED)","terminal_id":3,"tail_lines":0}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::ReadScreen { terminal_id, lines } => {
                    assert_eq!(terminal_id, Some(3));
                    // `tail_lines: 0` means the whole buffer.
                    assert_eq!(lines, None);
                }
                other => panic!("expected ReadScreen, got {other:?}"),
            }
            request
                .reply
                .send(Ok(json!({ "terminal_id": 3, "content": "BUILD FAILED" })))
                .ok();
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
        assert!(body.contains("\"matched\":true"), "body: {body}");
        assert!(body.contains("BUILD FAILED"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_for_output_times_out() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/wait_for_output",
            Some(&token),
            r#"{"match":"never","timeout_ms":150}"#,
        );
        let app_side = async {
            for _ in 0..10 {
                let Ok(Some(request)) =
                    tokio::time::timeout(Duration::from_millis(500), requests.next()).await
                else {
                    break;
                };
                request
                    .reply
                    .send(Ok(json!({ "terminal_id": 1, "content": "still running" })))
                    .ok();
            }
        };

        let ((status, body), _) = tokio::join!(http, app_side);
        assert_eq!(status, 408);
        assert!(body.contains("timed_out"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_for_output_requires_a_matcher() {
        let server = spawn_server();
        let (status, body) = post(
            server.port,
            "/api/wait_for_output",
            Some(&server.token),
            "{}",
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("missing"), "body: {body}");
    }

    #[tokio::test]
    async fn wait_for_output_rejects_an_invalid_regex() {
        let server = spawn_server();
        let (status, body) = post(
            server.port,
            "/api/wait_for_output",
            Some(&server.token),
            r#"{"regex":"("}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("invalid regex"), "body: {body}");
    }

    #[tokio::test]
    async fn report_agent_state_forwards_state_and_token() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/report_agent_state",
            Some(&token),
            r#"{"token":"term-1","state":"blocked","message":"approve?","agent":"pi"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::ReportAgentState {
                    token,
                    state,
                    message,
                    agent,
                } => {
                    assert_eq!(token, "term-1");
                    assert_eq!(state, "blocked");
                    assert_eq!(message.as_deref(), Some("approve?"));
                    assert_eq!(agent.as_deref(), Some("pi"));
                }
                other => panic!("expected ReportAgentState, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn report_agent_state_rejects_unknown_states() {
        let server = spawn_server();
        let (status, body) = post(
            server.port,
            "/api/report_agent_state",
            Some(&server.token),
            r#"{"token":"t","state":"sleeping"}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("working, blocked, or idle"), "body: {body}");
    }

    #[tokio::test]
    async fn report_agent_session_forwards_the_reference() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(
            port,
            "/api/report_agent_session",
            Some(&token),
            r#"{"token":"t","agent":"pi","session_path":"/tmp/s.json"}"#,
        );
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            match request.kind {
                IpcRequestKind::ReportAgentSession {
                    token,
                    agent,
                    session_path,
                    session_id,
                } => {
                    assert_eq!(token, "t");
                    assert_eq!(agent, "pi");
                    assert_eq!(session_path.as_deref(), Some("/tmp/s.json"));
                    assert_eq!(session_id, None);
                }
                other => panic!("expected ReportAgentSession, got {other:?}"),
            }
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn report_agent_session_requires_a_reference() {
        let server = spawn_server();
        let (status, body) = post(
            server.port,
            "/api/report_agent_session",
            Some(&server.token),
            r#"{"token":"t","agent":"pi"}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert!(body.contains("session_path"), "body: {body}");
    }

    #[tokio::test]
    async fn new_tab_with_empty_body_is_forwarded() {
        let TestServer {
            port,
            token,
            requests,
        } = spawn_server();
        let mut requests = requests;

        let http = post(port, "/api/new_tab", Some(&token), "");
        let app_side = async {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("ipc request never arrived")
                .expect("ipc channel closed");
            let IpcRequestKind::NewTab { cwd } = request.kind else {
                panic!("expected a NewTab request");
            };
            assert_eq!(cwd, None);
            request.reply.send(Ok(json!({ "status": "ok" }))).ok();
        };

        let ((status, _), _) = tokio::join!(http, app_side);
        assert_eq!(status, 200);
    }
}
