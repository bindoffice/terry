use axum::{routing::post, Router, Json};
use std::net::TcpListener;
use serde_json::{Value, json};

pub fn start_ipc_server() -> Result<(u16, String), String> {
    let token = uuid::Uuid::new_v4().to_string();

    let app = Router::new()
        .route("/api/health", post(|| async { "OK" }))
        .route("/api/list_sessions", post(list_sessions))
        .route("/api/focus_session", post(focus_session))
        .route("/api/send_keys", post(send_keys))
        .route("/api/read_screen", post(read_screen))
        .route("/api/get_context", post(get_context))
        .route("/api/notify", post(notify))
        .route("/api/create_new_terminal", post(create_new_terminal));

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    
    listener.set_nonblocking(true).unwrap();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let tokio_listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(tokio_listener, app).await.unwrap();
        });
    });

    Ok((port, token))
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
