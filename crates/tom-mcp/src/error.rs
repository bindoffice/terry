//! Errors surfaced by the MCP client.

/// Everything that can go wrong talking to an MCP server.
///
/// Connection problems are reported per server and never abort the session: one
/// unreachable server must not cost the user the others, or their tools.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("failed to spawn `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("mcp transport i/o: {0}")]
    Io(#[source] std::io::Error),

    #[error("mcp http request failed: {0}")]
    Http(#[source] reqwest::Error),

    #[error("mcp server returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },

    #[error("mcp protocol violation: {0}")]
    Protocol(String),

    #[error("mcp malformed json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("mcp rpc error {code}: {message}")]
    Rpc { code: i64, message: String },

    #[error("mcp transport closed")]
    TransportClosed,

    #[error("mcp `{method}` timed out after {secs}s")]
    Timeout { method: String, secs: u64 },

    #[error("mcp server `{server}` failed to initialize: {message}")]
    Initialize { server: String, message: String },
}

impl McpError {
    /// True when the failure means the connection is unusable, so a caller
    /// should stop issuing further requests to this server.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            McpError::TransportClosed
                | McpError::Io(_)
                | McpError::Spawn { .. }
                | McpError::Http(_)
        )
    }
}
