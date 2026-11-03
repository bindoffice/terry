//! Adapt MCP tools into `tom_agent::AgentTool`s.
//!
//! The adapter is thin on purpose: it forwards arguments unchanged and maps a
//! server-side error into an `Ok(AgentToolResult::error_text(..))` rather than
//! an `Err`. A failing MCP tool is a tool result the model can react to, not a
//! harness failure that aborts the run. Only a closed transport (the server is
//! gone) aborts, because continuing would just produce one timeout per tool.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, ToolResultPartial};
use tom_ai::types::{Schema, Tool};

use crate::client::{decorate_description, normalize_arguments, McpClient, McpTool};

/// Build a model-facing tool backed by one MCP server.
pub fn create_mcp_tool(mcp_tool: McpTool, client: Arc<McpClient>) -> Arc<dyn AgentTool> {
    // The provider-facing schema is built once: `AgentTool::schema` hands out a
    // reference, so the value has to be owned by the adapter for its lifetime.
    let schema = Tool {
        name: mcp_tool.exposed_name.clone(),
        description: decorate_description(&mcp_tool.server, &mcp_tool.description),
        parameters: Schema::new(mcp_tool.input_schema.clone()),
        constrained_sampling: None,
    };
    Arc::new(McpAgentTool {
        schema,
        mcp_tool,
        client,
    })
}

struct McpAgentTool {
    schema: Tool,
    mcp_tool: McpTool,
    client: Arc<McpClient>,
}

#[async_trait]
impl AgentTool for McpAgentTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        &self.mcp_tool.exposed_name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }

        let arguments = normalize_arguments(params);
        let outcome = self
            .client
            .call_tool(&self.mcp_tool.server_name, arguments, &signal)
            .await;

        match outcome {
            Ok(outcome) => {
                let mut result = if outcome.text.is_empty() {
                    AgentToolResult::text("(the MCP server returned no text content)")
                } else {
                    AgentToolResult::text(outcome.text)
                };
                result.details = serde_json::json!({
                    "mcp": {
                        "server": self.mcp_tool.server,
                        "tool": self.mcp_tool.server_name,
                        "isError": outcome.is_error,
                    }
                });
                Ok(result)
            }
            Err(crate::error::McpError::TransportClosed) => Err(AgentError::Abort),
            Err(error) => Ok(AgentToolResult::error_text(format!(
                "MCP call to `{}` on server `{}` failed: {error}",
                self.mcp_tool.server_name, self.mcp_tool.server
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> McpTool {
        McpTool {
            server_name: "read_file".into(),
            exposed_name: "mcp__fs__read_file".into(),
            server: "fs".into(),
            description: "Read a file".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }
    }

    /// A client is required by the constructor but cannot be built without a
    /// server, so the schema-shape assertions live here on the pure inputs.
    #[test]
    fn schema_description_names_the_server() {
        assert_eq!(
            decorate_description("fs", &tool().description),
            "MCP tool from server `fs`: Read a file"
        );
    }

    #[test]
    fn exposed_name_is_used_as_the_tool_name() {
        assert_eq!(tool().exposed_name, "mcp__fs__read_file");
    }
}
