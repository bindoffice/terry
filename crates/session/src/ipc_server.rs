use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};
use std::{net::TcpListener, path::PathBuf};

/// The work an IPC request asks the GUI to perform.
#[derive(Debug)]
pub enum IpcRequestKind {
    /// Open a new terminal tab, optionally in `cwd`.
    NewTab { cwd: Option<PathBuf> },
    /// Paste text into the active terminal.
    SendText { text: String },
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
    /// Read the terminal's content as plain text.
    ReadScreen { terminal_id: Option<u64> },
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
