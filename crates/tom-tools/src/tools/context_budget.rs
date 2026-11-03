//! `get_context_remaining` — lets the model see its own context budget.
//!
//! Codex ships a `get_context_remaining` tool plus a `<rollout_budget>` prompt
//! fragment so the *model* decides when to compact or start a new window,
//! instead of waiting for the host's threshold to fire. tom has the threshold
//! (`should_compact`) but no way for the model to ask. This tool closes that
//! gap.
//!
//! The tool itself is intentionally a thin shell over a [`ContextUsageProvider`]:
//! the harness knows the live context size, while `tom-tools` must not depend on
//! the harness. The provider is bound late, the same way `tool_search` is bound
//! after `AgentHarness::create`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use tom_ai::types::Tool;

pub const GET_CONTEXT_REMAINING_TOOL_NAME: &str = "get_context_remaining";

/// Current context usage, in the same token units as the compaction threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    /// Estimated tokens already in the projected context.
    pub used_tokens: i64,
    /// The model's context window; `0` means the model does not declare one.
    pub context_window: i64,
}

impl ContextUsage {
    pub fn remaining_tokens(self) -> i64 {
        (self.context_window - self.used_tokens).max(0)
    }
}

/// The host that can answer "how full is the context right now".
#[async_trait]
pub trait ContextUsageProvider: Send + Sync {
    async fn context_usage(&self) -> Result<ContextUsage, String>;
}

/// Input (empty: the tool takes no arguments).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct GetContextRemainingInput {}

/// The `get_context_remaining` tool.
pub struct GetContextRemainingTool {
    schema: Tool,
    provider: Mutex<Option<Arc<dyn ContextUsageProvider>>>,
}

impl GetContextRemainingTool {
    fn schema() -> Tool {
        Tool {
            name: GET_CONTEXT_REMAINING_TOOL_NAME.to_string(),
            description: "Report how much of the model's context window is left. \
                          Use it to decide whether to compact, summarise, or start a \
                          new context before a large read or command."
                .to_string(),
            parameters: tom_ai::types::Schema::new(
                serde_json::to_value(schemars::schema_for!(GetContextRemainingInput))
                    .unwrap_or_default(),
            ),
            constrained_sampling: None,
        }
    }

    /// Bind the live provider after the harness exists.
    pub fn set_provider(&self, provider: Arc<dyn ContextUsageProvider>) {
        *self
            .provider
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(provider);
    }
}

/// Build the `get_context_remaining` tool.
pub fn create_get_context_remaining_tool() -> Arc<GetContextRemainingTool> {
    Arc::new(GetContextRemainingTool {
        schema: GetContextRemainingTool::schema(),
        provider: Mutex::new(None),
    })
}

#[async_trait]
impl AgentTool for GetContextRemainingTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        GET_CONTEXT_REMAINING_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let provider = self
            .provider
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(provider) = provider else {
            return Err(AgentError::Tool(
                "get_context_remaining is not bound to a harness".to_string(),
            ));
        };
        let usage = provider.context_usage().await.map_err(AgentError::Tool)?;
        let text = if usage.context_window <= 0 {
            format!(
                "Context usage: ~{} tokens used. The model does not declare a context window, \
                 so remaining budget is unknown.",
                usage.used_tokens
            )
        } else {
            let remaining = usage.remaining_tokens();
            let percent = (remaining as f64 / usage.context_window as f64 * 100.0).round() as i64;
            format!(
                "Context usage: ~{} of {} tokens used, ~{} remaining ({percent}%).",
                usage.used_tokens, usage.context_window, remaining
            )
        };
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text(text)],
            details: serde_json::to_value(usage).unwrap_or(serde_json::Value::Null),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Option<ContextUsage>);

    #[async_trait]
    impl ContextUsageProvider for Fixed {
        async fn context_usage(&self) -> Result<ContextUsage, String> {
            self.0.ok_or_else(|| "no snapshot".to_string())
        }
    }

    async fn run(usage: Option<ContextUsage>) -> Result<AgentToolResult, AgentError> {
        let tool = create_get_context_remaining_tool();
        tool.set_provider(Arc::new(Fixed(usage)));
        tool.execute(
            "call",
            serde_json::json!({}),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
    }

    fn text(result: &AgentToolResult) -> String {
        match &result.content[0] {
            TextContentOrImage::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn unbound_tool_says_so_instead_of_guessing() {
        let tool = create_get_context_remaining_tool();
        let error = tool
            .execute(
                "call",
                serde_json::json!({}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect_err("an unbound tool has no answer");
        assert!(error.to_string().contains("not bound"), "{error}");
    }

    #[tokio::test]
    async fn reports_used_and_remaining_from_the_live_window() {
        // 64k window, 16k used → 48k left, which is 75%.
        let result = run(Some(ContextUsage {
            used_tokens: 16_000,
            context_window: 64_000,
        }))
        .await
        .unwrap();
        let text = text(&result);
        assert!(text.contains("16000"), "{text}");
        assert!(text.contains("48000"), "{text}");
        assert!(text.contains("75%"), "{text}");
    }

    #[tokio::test]
    async fn a_full_window_clamps_remaining_to_zero() {
        let usage = ContextUsage {
            used_tokens: 70_000,
            context_window: 64_000,
        };
        assert_eq!(usage.remaining_tokens(), 0);
        let text = text(&run(Some(usage)).await.unwrap());
        assert!(text.contains("0 remaining"), "{text}");
    }

    #[tokio::test]
    async fn an_undeclared_window_is_reported_as_unknown() {
        let text = text(
            &run(Some(ContextUsage {
                used_tokens: 1_234,
                context_window: 0,
            }))
            .await
            .unwrap(),
        );
        assert!(text.contains("1234"), "{text}");
        assert!(text.contains("unknown"), "{text}");
    }

    #[tokio::test]
    async fn a_provider_failure_surfaces_as_a_tool_error() {
        let error = run(None).await.expect_err("provider failure");
        assert!(error.to_string().contains("no snapshot"), "{error}");
    }
}
