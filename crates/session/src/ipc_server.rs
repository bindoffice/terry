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

    // Mock routes are kept for compatibility with existing tooling; they now
    // require the bearer token but still answer canned payloads.
    let protected = Router::new()
        .route("/api/new_tab", post(new_tab))
        .route("/api/send_text", post(send_text))
        .route("/api/list_sessions", post(list_sessions))
        .route("/api/focus_session", post(focus_session))
        .route("/api/send_keys", post(send_keys))
        .route("/api/read_screen", post(read_screen))
        .route("/api/get_context", post(get_context))
        .route("/api/notify", post(notify))
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

async fn list_sessions() -> Json<Value> {
    Json(json!({"sessions": []})) // Implementation is a mock placeholder
}

async fn focus_session(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"status": "focused"}))
}

async fn send_keys(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"status": "sent"}))
}

async fn read_screen(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"content": "terminal output"}))
}

async fn get_context(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"cwd": "/", "git_branch": "main"}))
}

async fn notify(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"status": "notified"}))
}

async fn create_new_terminal(Json(_payload): Json<Value>) -> Json<Value> {
    Json(json!({"status": "created", "id": "uuid-here"}))
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
