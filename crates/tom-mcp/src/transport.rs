//! JSON-RPC 2.0 transports.
//!
//! Two transports, one interface ([`Transport`]): a stdio child process whose
//! frames are newline-delimited JSON, and streamable HTTP where each request is
//! one POST and the answer may be a JSON body or an SSE stream.
//!
//! stdio requests are correlated by id through a shared pending map, so a
//! reader task can hand responses to whoever is waiting even when several calls
//! are in flight. HTTP has no long-lived reader, so a response is resolved
//! inline.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use crate::error::McpError;

/// A message the server sent that is not a response to one of our requests.
/// Handling these is optional: a client that ignores them stays correct for
/// tool calling, and the transport answers nothing on its own.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A server-initiated request (e.g. `roots/list`).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// A notification (no id, no response expected).
    Notification { method: String, params: Value },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>>;

/// A connected transport. Dropping it tears down the child process.
pub struct Transport {
    kind: TransportKind,
    next_id: AtomicU64,
    pending: Pending,
    cancel: CancellationToken,
}

enum TransportKind {
    Stdio {
        child: tokio::process::Child,
        /// Writes are serialized through this lock: a child's stdin is a single
        /// resource, and interleaving two frames would corrupt the stream.
        stdin: Mutex<tokio::process::ChildStdin>,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Mutex<Option<String>>,
    },
}

impl Transport {
    /// Spawn a stdio server. `command` is a single executable; arguments are
    /// passed as-is and never through a shell.
    pub async fn spawn_stdio(
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: Option<&std::path::Path>,
    ) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The child must die with tom; an orphaned server would keep holding
            // files, ports, or a lock after the CLI exits.
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|source| McpError::Spawn {
            command: command.to_string(),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("no stdin pipe".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("no stdout pipe".into()))?;
        if let Some(stderr) = child.stderr.take() {
            // Drain stderr, or a chatty server fills the pipe buffer and blocks
            // forever on its next write.
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "tom_mcp", "server stderr: {line}");
                }
            });
        }

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        spawn_reader(stdout, pending.clone(), cancel.clone());

        Ok(Self {
            kind: TransportKind::Stdio {
                child,
                stdin: Mutex::new(stdin),
            },
            next_id: AtomicU64::new(1),
            pending,
            cancel,
        })
    }

    /// Connect to a streamable HTTP endpoint. No network call happens until the
    /// first request.
    pub fn http(
        url: String,
        headers: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, McpError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| McpError::Protocol(e.to_string()))?;
        Ok(Self {
            kind: TransportKind::Http {
                client,
                url,
                headers: headers.into_iter().collect(),
                session: Mutex::new(None),
            },
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            cancel: CancellationToken::new(),
        })
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Send a request and wait for its response.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});

        // HTTP resolves inline, so it needs no pending slot.
        if matches!(self.kind, TransportKind::Http { .. }) {
            let response = tokio::time::timeout(timeout, self.post_http(&message))
                .await
                .map_err(|_| McpError::Timeout {
                    method: method.to_string(),
                    secs: timeout.as_secs(),
                })??;
            return interpret_response(response);
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        if let Err(e) = self.write_line(&message).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                Err(McpError::TransportClosed)
            }
            Err(_) => {
                // Retire the slot so a late response cannot resolve a request
                // that reused the id.
                self.pending.lock().await.remove(&id);
                Err(McpError::Timeout {
                    method: method.to_string(),
                    secs: timeout.as_secs(),
                })
            }
        }
    }

    /// Fire-and-forget notification.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.kind {
            TransportKind::Stdio { .. } => self.write_line(&message).await,
            TransportKind::Http { .. } => self.post_http(&message).await.map(|_| ()),
        }
    }

    async fn write_line(&self, message: &Value) -> Result<(), McpError> {
        let TransportKind::Stdio { stdin, .. } = &self.kind else {
            return Err(McpError::Protocol("not a stdio transport".into()));
        };
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut stdin = stdin.lock().await;
        stdin.write_all(&line).await.map_err(McpError::Io)?;
        stdin.flush().await.map_err(McpError::Io)
    }

    /// POST one message. Returns the JSON response for a request, and `None`
    /// for a notification (202 with no body).
    async fn post_http(&self, message: &Value) -> Result<Option<Value>, McpError> {
        let TransportKind::Http {
            client,
            url,
            headers,
            session,
        } = &self.kind
        else {
            return Err(McpError::Protocol("not an http transport".into()));
        };

        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(message);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        if let Some(session) = session.lock().await.as_ref() {
            request = request.header("mcp-session-id", session);
        }

        let response = request.send().await.map_err(McpError::Http)?;
        // The session id is issued on the initialize response and must be
        // echoed on every later call.
        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *session.lock().await = Some(session_id.to_string());
        }

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::HttpStatus {
                status: status.as_u16(),
                body: body.chars().take(512).collect(),
            });
        }

        if message.get("id").is_none() {
            return Ok(None);
        }

        let is_event_stream = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));

        let body = response.text().await.map_err(McpError::Http)?;
        let payload = if is_event_stream {
            parse_sse_payload(&body)?.to_string()
        } else if body.trim().is_empty() {
            return Err(McpError::TransportClosed);
        } else {
            body
        };

        Ok(Some(serde_json::from_str(payload.trim())?))
    }
}

fn spawn_reader(stdout: tokio::process::ChildStdout, pending: Pending, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let read = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                result = reader.read_until(b'\n', &mut buffer) => result,
            };
            match read {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(target: "tom_mcp", "stdio read error: {e}");
                    break;
                }
            }
            let Ok(value) = serde_json::from_slice::<Value>(&buffer) else {
                // A server that prints a non-JSON banner must not kill the
                // session; skip the line.
                continue;
            };
            // Only responses are resolved here. Server-initiated requests are
            // dropped: this client does not advertise sampling or roots, so a
            // conforming server never sends them, and answering "method not
            // found" would be worse than staying silent.
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if value.get("method").is_some() {
                continue;
            }
            let Some(sender) = pending.lock().await.remove(&id) else {
                continue;
            };
            let _ = sender.send(interpret_response(Some(value)));
        }
        // Unblock every waiter: the peer is gone and no response is coming.
        pending.lock().await.clear();
    });
}

/// Turn a raw JSON-RPC response into a result or a typed error.
fn interpret_response(response: Option<Value>) -> Result<Value, McpError> {
    let Some(value) = response else {
        return Err(McpError::TransportClosed);
    };
    if let Some(error) = value.get("error") {
        return Err(McpError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
        });
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

/// Extract the `data:` payloads of an SSE body and concatenate them. MCP sends
/// one JSON message per event.
fn parse_sse_payload(body: &str) -> Result<String, McpError> {
    let mut data = String::new();
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(rest.trim_start());
    }
    if data.is_empty() {
        return Err(McpError::Protocol("empty event stream".into()));
    }
    Ok(data)
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let TransportKind::Stdio { child, .. } = &mut self.kind {
            // `kill_on_drop` handles the normal path; this makes the intent
            // explicit and covers a child that ignored the drop.
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_payload_joins_data_lines() {
        let body = "event: message\ndata: {\"a\":\ndata: 1}\n\n";
        assert_eq!(parse_sse_payload(body).unwrap(), "{\"a\":\n1}");
    }

    #[test]
    fn sse_without_data_is_an_error() {
        assert!(parse_sse_payload("event: ping\n\n").is_err());
    }

    #[test]
    fn result_payload_is_unwrapped() {
        let value = json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}});
        assert_eq!(interpret_response(Some(value)).unwrap()["ok"], json!(true));
    }

    #[test]
    fn rpc_error_is_mapped_with_code() {
        let value = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}});
        match interpret_response(Some(value)) {
            Err(McpError::Rpc { code, message }) => {
                assert_eq!(code, -32601);
                assert_eq!(message, "nope");
            }
            other => panic!("expected Rpc error, got {other:?}"),
        }
    }

    #[test]
    fn missing_response_is_transport_closed() {
        assert!(matches!(
            interpret_response(None),
            Err(McpError::TransportClosed)
        ));
    }
}
