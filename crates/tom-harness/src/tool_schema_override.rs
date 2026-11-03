//! Serve one tool under a different *declaration* than it implements.
//!
//! Tools own their schema: `AgentTool::schema` returns `&Tool`, which is what
//! makes a declaration stable and cheap. The tools-budget path needs one narrow
//! exception — send a shorter description than the tool declares — without
//! rewriting every tool to accept a mutable schema.
//!
//! This wrapper is that exception. It only ever replaces the description text:
//! the name and parameter schema are copied from the tool, so a model that
//! needed the tall declaration can still make exactly the same call.
//!
//! Scope is deliberate — this is for budget trimming, not a general-purpose
//! "rebrand a tool" facility. Anything else belongs in the tool.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, ToolResultPartial};
use tom_ai::types::Tool;

/// Wrap `tool` so it declares `schema` instead of its own.
pub fn with_schema(tool: Arc<dyn AgentTool>, schema: Tool) -> Arc<dyn AgentTool> {
    Arc::new(SchemaOverrideTool { tool, schema })
}

struct SchemaOverrideTool {
    tool: Arc<dyn AgentTool>,
    schema: Tool,
}

#[async_trait]
impl AgentTool for SchemaOverrideTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        self.tool.label()
    }

    /// Preserved so a trimmed tool keeps its concurrency behavior: forcing a
    /// whole batch sequential because of a description change would be a
    /// surprising side effect of a token budget.
    fn execution_mode(&self) -> tom_agent::types::ToolExecutionMode {
        self.tool.execution_mode()
    }

    fn prepare_arguments(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AgentError> {
        self.tool.prepare_arguments(args)
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.tool
            .execute(tool_call_id, params, signal, on_update)
            .await
    }
}
