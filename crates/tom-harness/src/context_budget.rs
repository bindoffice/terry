//! Harness binding for the `get_context_remaining` tool.
//!
//! The tool lives in `tom-tools` (it has no harness dependency), but the answer
//! — "how full is the context" — is a harness fact. This adapter bridges the two
//! and is installed right after `AgentHarness::create`, the same late-binding
//! pattern `tool_search` uses.

use async_trait::async_trait;

use crate::agent_harness::{AgentHarness, WeakAgentHarness};

/// Answers [`tom_tools::ContextUsageProvider`] from a live harness.
pub struct HarnessContextUsage {
    harness: WeakAgentHarness,
}

impl HarnessContextUsage {
    pub fn new(harness: &AgentHarness) -> Self {
        Self {
            harness: harness.downgrade(),
        }
    }
}

#[async_trait]
impl tom_tools::ContextUsageProvider for HarnessContextUsage {
    async fn context_usage(&self) -> Result<tom_tools::ContextUsage, String> {
        let harness = self
            .harness
            .upgrade()
            .ok_or_else(|| "harness is no longer alive".to_string())?;
        harness
            .context_usage()
            .await
            .map_err(|error| error.to_string())
    }
}
