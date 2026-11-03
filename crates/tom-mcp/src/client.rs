//! One connected MCP server: handshake, tool discovery, tool invocation.
//!
//! The tool surface is deliberately model-facing and protocol-free. Callers get
//! [`McpTool`] values and hand them to [`crate::tools::create_mcp_tool`], which
//! adapts them into `tom_agent::AgentTool`s. Nothing here reaches into the agent
//! loop, so the same client serves the CLI, an embedded SDK user, and tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config::{ResolvedServer, Transport as ServerTransport};
use crate::error::McpError;
use crate::transport::Transport;

/// Protocol revision this client implements. Servers negotiate down if needed.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Client name reported in the handshake.
const CLIENT_NAME: &str = "tom";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A tool as advertised by a server, normalized to what a model needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Name exactly as the server advertised it (used on the wire).
    pub server_name: String,
    /// Flattened, model-facing name: `mcp__<server>__<tool>`.
    pub exposed_name: String,
    /// The server this tool belongs to.
    pub server: String,
    pub description: String,
    /// JSON Schema for the arguments. Always an object schema.
    pub input_schema: Value,
}

/// A connected server.
pub struct McpClient {
    server: ResolvedServer,
    transport: Transport,
    initialized: AtomicBool,
    /// The `instructions` the server sent with its handshake, if any. They are
    /// usage guidance a model cannot infer from the tool list ("call search
    /// before get"), so `codemode`'s `describeNamespace()` reports them.
    instructions: std::sync::OnceLock<String>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server", &self.server.name)
            .field("transport", &self.server.transport.kind())
            .field("initialized", &self.initialized.load(Ordering::Acquire))
            .finish()
    }
}

impl McpClient {
    /// Connect and perform the MCP handshake.
    pub async fn connect(server: ResolvedServer) -> Result<Arc<Self>, McpError> {
        let transport = match &server.transport {
            ServerTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => Transport::spawn_stdio(command, args, env, cwd.as_deref())
                .await
                .map_err(|e| McpError::Initialize {
                    server: server.name.clone(),
                    message: e.to_string(),
                })?,
            ServerTransport::Http { url, headers } => Transport::http(url.clone(), headers.clone())
                .map_err(|e| McpError::Initialize {
                    server: server.name.clone(),
                    message: e.to_string(),
                })?,
        };

        let client = Arc::new(Self {
            server,
            transport,
            initialized: AtomicBool::new(false),
            instructions: std::sync::OnceLock::new(),
        });
        client.initialize().await?;
        Ok(client)
    }

    pub fn name(&self) -> &str {
        &self.server.name
    }

    pub fn description(&self) -> Option<&str> {
        self.server.description.as_deref()
    }

    pub fn transport_kind(&self) -> &'static str {
        self.server.transport.kind()
    }

    /// Usage guidance the server sent with its handshake.
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.get().map(String::as_str)
    }

    /// The server's configured `exposure`, if it set one.
    pub fn exposure(&self) -> Option<crate::config::Exposure> {
        self.server.exposure
    }

    fn timeout(&self) -> Duration {
        self.server.timeout
    }

    async fn initialize(&self) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                // No sampling, roots, or elicitation: this client injects tools
                // and nothing else, so claiming more would invite requests it
                // would have to refuse.
                "roots": Value::Null,
            },
            "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
        });

        let response = self
            .transport
            .request("initialize", params, self.timeout())
            .await
            .map_err(|e| McpError::Initialize {
                server: self.server.name.clone(),
                message: e.to_string(),
            })?;
        if let Some(instructions) = response
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            let _ = self.instructions.set(instructions.to_string());
        }

        // `initialized` is a notification; some servers refuse `tools/list`
        // until they receive it.
        let _ = self
            .transport
            .notify("notifications/initialized", json!({}))
            .await;

        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// List every tool the server exposes, with flattened names.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        // Bounded so a server that never stops returning a cursor cannot spin
        // forever. 100 pages is far past any real server's tool count.
        for _ in 0..100 {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let result = self
                .transport
                .request("tools/list", params, self.timeout())
                .await?;

            let page = result.get("tools").and_then(Value::as_array);
            if let Some(page) = page {
                for raw in page {
                    if let Some(tool) = self.normalize_tool(raw) {
                        tools.push(tool);
                    }
                }
            }

            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => break,
            }
        }

        Ok(tools)
    }

    fn normalize_tool(&self, raw: &Value) -> Option<McpTool> {
        let server_name = raw.get("name").and_then(Value::as_str)?;
        if server_name.is_empty() {
            return None;
        }
        let description = raw
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // A tool may omit `inputSchema`, or ship one that is not an object
        // schema. Providers require `type: "object"`, so normalize rather than
        // discard a usable tool.
        let input_schema = match raw.get("inputSchema") {
            Some(schema) if schema.is_object() => {
                let mut schema = schema.clone();
                if schema.get("type").is_none() {
                    schema["type"] = json!("object");
                }
                schema
            }
            _ => json!({"type": "object", "properties": {}}),
        };

        Some(McpTool {
            server_name: server_name.to_string(),
            exposed_name: exposed_tool_name(&self.server.name, server_name),
            server: self.server.name.clone(),
            description,
            input_schema,
        })
    }

    /// Call a tool by its **server-side** name. Returns the concatenated text
    /// content, which is what a model can consume.
    pub async fn call_tool(
        &self,
        server_name: &str,
        arguments: Value,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<McpToolOutcome, McpError> {
        let params = json!({"name": server_name, "arguments": arguments});

        let request = self.transport.request("tools/call", params, self.timeout());
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(McpError::TransportClosed),
            result = request => result?,
        };

        Ok(McpToolOutcome::from_result(&result))
    }
}

/// A tool call's result, split into model-facing text and structured metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolOutcome {
    pub text: String,
    pub is_error: bool,
    /// Whether the server reported the call as unsuccessful.
    pub raw: Value,
}

impl McpToolOutcome {
    fn from_result(result: &Value) -> Self {
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut parts: Vec<String> = Vec::new();
        if let Some(content) = result.get("content").and_then(Value::as_array) {
            for block in content {
                match block.get("type").and_then(Value::as_str) {
                    // Images and audio cannot be represented as text, so name
                    // them instead of silently dropping the block: the model
                    // should know the server returned something it cannot see.
                    Some(kind @ ("image" | "audio")) => {
                        let mime = block
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        parts.push(format!("[{kind} content omitted: {mime}]"));
                    }
                    Some("resource") => {
                        if let Some(text) = block
                            .get("resource")
                            .and_then(|r| r.get("text"))
                            .and_then(Value::as_str)
                        {
                            parts.push(text.to_string());
                        } else {
                            parts.push("[resource content omitted]".to_string());
                        }
                    }
                    _ => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                }
            }
        }

        // Some servers answer with a bare `structuredContent` and no `content`.
        let text = if parts.is_empty() {
            match result.get("structuredContent") {
                Some(structured) if !structured.is_null() => {
                    serde_json::to_string_pretty(structured).unwrap_or_default()
                }
                _ => String::new(),
            }
        } else {
            parts.join("\n")
        };

        Self {
            text,
            is_error,
            raw: result.clone(),
        }
    }
}

/// Flatten a server and tool name into the single identifier a provider accepts.
///
/// Provider tool names allow only `[A-Za-z0-9_-]`, so every other character is
/// replaced. A deterministic hash suffix keeps two servers whose names differ
/// only in punctuation (`my-tools` / `my_tools`) from colliding.
pub fn exposed_tool_name(server: &str, tool: &str) -> String {
    let server = sanitize(server);
    let tool = sanitize(tool);
    format!("mcp__{server}__{tool}")
}

fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut changed = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
            changed = true;
        }
    }
    if changed {
        // The suffix is only needed when sanitizing actually lost information,
        // so ordinary names stay readable.
        out.push_str(&format!("_{:08x}", fnv1a(name)));
    }
    out
}

fn fnv1a(value: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in value.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Split an exposed name back into its parts. Returns `None` when the name was
/// not produced by [`exposed_tool_name`].
pub fn parse_exposed_tool_name(exposed: &str) -> Option<(&str, &str)> {
    let rest = exposed.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some((server, tool))
}

/// Build a compact inventory for the system prompt or a `/mcp` listing.
pub fn render_inventory(clients: &[Arc<McpClient>], tools: &[McpTool]) -> String {
    if clients.is_empty() {
        return "No MCP servers configured.".to_string();
    }
    let mut out = String::new();
    for client in clients {
        let count = tools.iter().filter(|t| t.server == client.name()).count();
        out.push_str(&format!(
            "- {} ({}, {} tool{})",
            client.name(),
            client.transport_kind(),
            count,
            if count == 1 { "" } else { "s" }
        ));
        if let Some(description) = client.description() {
            out.push_str(&format!(" — {description}"));
        }
        out.push('\n');
    }
    out
}

/// Rewrite a tool's `description` so the model knows which server it belongs to
/// and that it is external. Servers give terse descriptions, and without the
/// prefix `search` from two servers is indistinguishable.
pub fn decorate_description(server: &str, description: &str) -> String {
    let body = description.trim();
    if body.is_empty() {
        format!("MCP tool from server `{server}`.")
    } else {
        format!("MCP tool from server `{server}`: {body}")
    }
}

/// Ensure an arguments object exists. Some models send `null` for a tool that
/// declares no parameters; providers reject `null` where an object is required.
pub fn normalize_arguments(arguments: Value) -> Value {
    match arguments {
        Value::Null => Value::Object(Map::new()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposed_name_is_prefixed_and_sanitized() {
        assert_eq!(
            exposed_tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
        // Spaces and dots cannot appear in provider tool names.
        let name = exposed_tool_name("my server", "read.file");
        assert!(name.starts_with("mcp__my_server_"), "{name}");
        assert!(name.contains("read_file_"), "{name}");
    }

    #[test]
    fn sanitization_is_deterministic_and_collision_resistant() {
        let a = exposed_tool_name("my-tools", "x");
        let b = exposed_tool_name("my_tools", "x");
        assert_ne!(a, b, "differing only in punctuation must not collide");
        assert_eq!(a, exposed_tool_name("my-tools", "x"));
    }

    #[test]
    fn plain_names_get_no_hash_suffix() {
        assert_eq!(exposed_tool_name("server", "tool"), "mcp__server__tool");
    }

    #[test]
    fn exposed_names_round_trip() {
        assert_eq!(
            parse_exposed_tool_name("mcp__fs__read"),
            Some(("fs", "read"))
        );
        assert_eq!(parse_exposed_tool_name("read"), None);
    }

    #[test]
    fn description_is_prefixed_and_blank_is_tolerated() {
        assert_eq!(
            decorate_description("fs", " Read a file "),
            "MCP tool from server `fs`: Read a file"
        );
        assert_eq!(
            decorate_description("fs", "   "),
            "MCP tool from server `fs`."
        );
    }

    #[test]
    fn null_arguments_become_an_object() {
        assert_eq!(normalize_arguments(Value::Null), json!({}));
        assert_eq!(normalize_arguments(json!({"a":1})), json!({"a":1}));
    }

    #[test]
    fn outcome_joins_text_blocks() {
        let result = json!({"content":[
            {"type":"text","text":"a"},
            {"type":"text","text":"b"}
        ]});
        let outcome = McpToolOutcome::from_result(&result);
        assert_eq!(outcome.text, "a\nb");
        assert!(!outcome.is_error);
    }

    #[test]
    fn outcome_marks_images_instead_of_dropping_them() {
        let result = json!({"content":[{"type":"image","mimeType":"image/png"}]});
        let outcome = McpToolOutcome::from_result(&result);
        assert!(
            outcome.text.contains("image content omitted"),
            "{outcome:?}"
        );
    }

    #[test]
    fn outcome_falls_back_to_structured_content() {
        let result = json!({"structuredContent":{"count":2}});
        let outcome = McpToolOutcome::from_result(&result);
        assert!(outcome.text.contains("\"count\""), "{outcome:?}");
    }

    #[test]
    fn outcome_propagates_server_error_flag() {
        let result = json!({"content":[{"type":"text","text":"bad"}],"isError":true});
        assert!(McpToolOutcome::from_result(&result).is_error);
    }
}
