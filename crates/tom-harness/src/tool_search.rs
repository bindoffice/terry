//! The host-side `tool_search` binding.
//!
//! The tool has to exist **before** the harness does: tools are built first and
//! the harness adopts them. But loading is a harness operation — the matches go
//! into the harness's active-tool set and are recorded as an
//! `active_tools_change` entry. This cell closes that gap the same way
//! `HarnessActionHost` does for runtime actions: it is created empty, holds a
//! weak harness binding, and is filled immediately after `AgentHarness::create`.
//!
//! One subtlety that is easy to get wrong: the *candidate* list must be live, not
//! a snapshot taken at build time. A load grows the harness's active set, so the
//! candidate list shrinks; a snapshot would keep offering tools that are already
//! declared, and a second search would report them as "loaded" again. The cell
//! therefore asks the harness for candidates on every call.

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, ToolResultPartial};
use tom_ai::types::Tool;

use crate::agent_harness::{AgentHarness, WeakAgentHarness};

/// A late-bound `tool_search` tool.
///
/// Before [`ToolSearchToolCell::set_harness`] runs it still carries a usable
/// schema (so the declaration is stable from the first request) and searches the
/// fallback candidates instead of failing the run.
pub struct ToolSearchToolCell {
    schema: Tool,
    harness: OnceLock<WeakAgentHarness>,
    /// Candidates used while no harness is bound (embedded callers, tests).
    fallback: Mutex<Vec<tom_tools::DeferredTool>>,
}

impl ToolSearchToolCell {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            schema: Self::schema(),
            harness: OnceLock::new(),
            fallback: Mutex::new(Vec::new()),
        })
    }

    fn schema() -> Tool {
        Tool {
            name: tom_tools::TOOL_SEARCH_TOOL_NAME.to_string(),
            description: tom_tools::TOOL_SEARCH_DESCRIPTION.to_string(),
            parameters: tom_ai::types::Schema::new(
                serde_json::to_value(schemars::schema_for!(tom_tools::ToolSearchInput))
                    .unwrap_or_default(),
            ),
            constrained_sampling: None,
        }
    }

    /// Point the tool at the harness whose registry it searches.
    pub fn set_harness(&self, harness: &AgentHarness) {
        let _ = self.harness.set(harness.downgrade());
    }

    /// Replace the candidates used when no harness is bound.
    pub fn set_fallback_candidates(&self, candidates: Vec<tom_tools::DeferredTool>) {
        *self.fallback.lock().unwrap_or_else(|e| e.into_inner()) = candidates;
    }

    fn harness(&self) -> Option<AgentHarness> {
        self.harness.get().and_then(WeakAgentHarness::upgrade)
    }

    /// The deferred tools, live from the harness when one is bound.
    pub fn deferred_tools(&self) -> Vec<tom_tools::DeferredTool> {
        match self.harness() {
            Some(harness) => harness.deferred_tools(),
            None => self
                .fallback
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }
}

#[async_trait]
impl AgentTool for ToolSearchToolCell {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        tom_tools::TOOL_SEARCH_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        // Rank against the current candidates, then hand the matches to the
        // harness, which admits them and records the change.
        let tool = tom_tools::create_tool_search_tool_with(self.deferred_tools());
        let mut result = tool
            .execute("tool_search", params, signal, Arc::new(|_| {}))
            .await?;
        // The ranked names are the *candidates*; only the harness can say which
        // of them it actually admitted (a tool may have been removed from the
        // registry between the snapshot and the load).
        if result.added_tool_names.is_empty() {
            return Ok(result);
        }
        let Some(harness) = self.harness() else {
            return Ok(result);
        };
        let added = harness
            .load_tools(&result.added_tool_names)
            .await
            .map_err(|error| {
                AgentError::Tool(format!(
                    "tool_search matched tools but could not record the load: {error}"
                ))
            })?;
        result.added_tool_names = added;
        Ok(result)
    }
}
