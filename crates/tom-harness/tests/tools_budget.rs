//! The declaration budget is only useful if it reaches the request. These tests
//! drive a real harness and inspect the tools it actually declares.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use tom_ai::types::{Schema, Tool};
use tom_harness::agent_harness::AgentHarness;
use tom_harness::types::{AgentHarnessOptions, HarnessTool};

/// A tool with a deliberately huge description.
struct Chatty {
    schema: Tool,
}

impl Chatty {
    fn new(name: &str) -> Self {
        let description = "This tool does something useful. ".repeat(400);
        Self {
            schema: Tool {
                name: name.to_string(),
                description,
                parameters: Schema::new(serde_json::json!({"type":"object","properties":{}})),
                constrained_sampling: None,
            },
        }
    }
}

#[async_trait]
impl AgentTool for Chatty {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        _id: &str,
        _params: serde_json::Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text("ok")],
            ..Default::default()
        })
    }
}

async fn harness_with(tools: Vec<HarnessTool>) -> AgentHarness {
    let active = tools
        .iter()
        .map(|t| t.tool.schema().name.clone())
        .collect::<Vec<_>>();
    let mut options = AgentHarnessOptions::default();
    options.tools = tools;
    options.active_tool_names = active;
    AgentHarness::create(options).await.expect("harness")
}

/// The declarations the harness would send on the next request.
async fn declared_tools(harness: &AgentHarness) -> Vec<Tool> {
    harness
        .declared_tools()
        .expect("declared tools")
        .iter()
        .map(|tool| tool.schema().clone())
        .collect()
}

#[tokio::test]
async fn without_a_budget_descriptions_are_sent_verbatim() {
    let harness = harness_with(vec![HarnessTool::new(Arc::new(Chatty::new("chatty")))]).await;
    let declared = declared_tools(&harness).await;
    assert_eq!(declared.len(), 1);
    let original = Chatty::new("chatty").schema.description;
    assert_eq!(declared[0].description, original);
}

#[tokio::test]
async fn a_budget_shrinks_descriptions_without_dropping_tools() {
    let harness = harness_with(vec![HarnessTool::new(Arc::new(Chatty::new("chatty")))]).await;
    let before = declared_tools(&harness).await;
    let before_tokens = tom_tools::tools_tokens(&before);

    // A ceiling well under the declared cost.
    harness.set_tools_budget(Some(before_tokens / 4));
    assert_eq!(harness.tools_budget(), Some(before_tokens / 4));

    let after = declared_tools(&harness).await;
    // The tool is still declared — trimming must never make a tool uncallable.
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "chatty");
    // The schema is untouched: this is what makes a call correct.
    assert_eq!(after[0].parameters, before[0].parameters);
    assert!(tom_tools::tools_tokens(&after) < before_tokens);
}

#[tokio::test]
async fn clearing_the_budget_restores_the_declared_descriptions() {
    let harness = harness_with(vec![HarnessTool::new(Arc::new(Chatty::new("chatty")))]).await;
    let original = declared_tools(&harness).await[0].description.clone();

    harness.set_tools_budget(Some(100));
    let trimmed = declared_tools(&harness).await[0].description.clone();
    assert!(
        trimmed.len() < original.len(),
        "expected a shorter description, got {} bytes vs {}",
        trimmed.len(),
        original.len()
    );

    harness.set_tools_budget(None);
    assert_eq!(declared_tools(&harness).await[0].description, original);
}
