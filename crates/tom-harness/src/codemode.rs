//! The `codemode` tool's harness binding.
//!
//! The tool has to exist **before** the harness does (tools are built first and
//! the harness adopts them), but a nested tool call is a harness operation: it
//! runs through the agent loop's pipeline with the harness's hooks, active tool
//! set and cancellation. This binds the two the same way `tool_search` does —
//! created empty, filled immediately after `AgentHarness::create` — and gives the
//! tool a live view of the tools a script may call.
//!
//! ## Which tools a script can call
//!
//! The **active** set plus every deferred tool: a tool the model has not been
//! given is still callable from a script, which is the whole point of pairing
//! `codemode` with a large MCP server. The codemode tool itself is excluded
//! (scripts must not start scripts), and so is `tool_search` unless it is active
//! (it is a model-facing loader; a script uses `searchTools()`).
//!
//! ## Store
//!
//! `store()`/`load()` are per branch, not per process: each successful script
//! appends a `codemode-store` custom entry, and `load()` replays the entries on
//! the branch. That is what makes `/tree`, resume and fork see the values written
//! on their own path instead of whatever the process happens to hold.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use tom_tools::codemode::{
    CodemodeHostBinding, CodemodeNamespace, CodemodeStore, CodemodeTool, NestedToolOutcome,
    CODEMODE_STORE_ENTRY_TYPE, CODEMODE_TOOL_NAME,
};

use crate::agent_harness::{AgentHarness, WeakAgentHarness};
use crate::session::types::{BranchBounds, Entry, EntryOrder, EntryQuery};

/// Binds the `codemode` tool to a harness.
pub struct CodemodeBinding {
    harness: WeakAgentHarness,
    /// MCP servers, as the namespaces scripts see: tool metadata lives on the
    /// tools themselves, but a server's description and instructions do not.
    namespaces: Vec<CodemodeNamespace>,
}

impl CodemodeBinding {
    pub fn new(harness: &AgentHarness, namespaces: Vec<CodemodeNamespace>) -> Arc<Self> {
        Arc::new(Self {
            harness: harness.downgrade(),
            namespaces,
        })
    }

    fn harness(&self) -> Option<AgentHarness> {
        self.harness.upgrade()
    }
}

impl CodemodeHostBinding for CodemodeBinding {
    fn callable_tools(&self) -> Vec<CodemodeTool> {
        let Some(harness) = self.harness() else {
            return Vec::new();
        };
        let active = harness.active_tools_for_nested_calls().unwrap_or_default();
        let deferred_names: std::collections::BTreeSet<String> = harness
            .deferred_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let deferred = harness.deferred_tools();
        let mut tools: Vec<CodemodeTool> = active
            .iter()
            .filter(|tool| tool.schema().name != CODEMODE_TOOL_NAME)
            .map(|tool| {
                let schema = tool.schema();
                CodemodeTool {
                    name: schema.name.clone(),
                    description: schema.description.clone(),
                    parameters: schema.parameters.as_value().clone(),
                    output_schema: None,
                    // `tool_search` is declared but is not a script tool: a script
                    // uses `searchTools()` instead, so it is never advertised here.
                    direct: !deferred_names.contains(&schema.name),
                }
            })
            .collect();
        // Deferred tools are registered but not declared, which is exactly where a
        // script is the better interface: the model names the capability, the
        // script calls it. Their metadata is all `deferred_tools()` carries.
        for tool in deferred {
            if tool.name == CODEMODE_TOOL_NAME
                || tools.iter().any(|existing| existing.name == tool.name)
            {
                continue;
            }
            tools.push(CodemodeTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
                output_schema: None,
                direct: false,
            });
        }
        tools
    }

    fn namespaces(&self) -> Vec<CodemodeNamespace> {
        self.namespaces.clone()
    }

    fn run_tool(
        &self,
        name: String,
        args: Value,
        signal: CancellationToken,
    ) -> BoxFuture<'static, Result<NestedToolOutcome, String>> {
        let Some(harness) = self.harness() else {
            return Box::pin(async { Err("the codemode session is gone".to_string()) });
        };
        // Nested calls are attributed to the script's own tool call, which the
        // harness records as the parent of the calls it made.
        Box::pin(async move { harness.run_nested_tool(&name, args, signal).await })
    }
}

/// `store()`/`load()` over the session branch.
pub struct SessionCodemodeStore {
    harness: WeakAgentHarness,
}

impl SessionCodemodeStore {
    pub fn new(harness: &AgentHarness) -> Arc<Self> {
        Arc::new(Self {
            harness: harness.downgrade(),
        })
    }
}

impl CodemodeStore for SessionCodemodeStore {
    fn load(&self) -> BoxFuture<'static, Result<BTreeMap<String, Value>, String>> {
        let Some(harness) = self.harness.upgrade() else {
            return Box::pin(async { Ok(BTreeMap::new()) });
        };
        Box::pin(async move {
            let entries = harness
                .find_entries_on_branch(
                    &EntryQuery {
                        order: Some(EntryOrder::OldestFirst),
                        ..Default::default()
                    },
                    &BranchBounds::default(),
                )
                .await
                .map_err(|error| error.to_string())?;
            let mut store: BTreeMap<String, Value> = BTreeMap::new();
            for entry in entries {
                let Entry::Custom(custom) = entry else {
                    continue;
                };
                if custom.custom_type != CODEMODE_STORE_ENTRY_TYPE {
                    continue;
                }
                let Some(data) = custom.data else {
                    continue;
                };
                let Some(object) = data.as_object() else {
                    continue;
                };
                if let Some(deleted) = object.get("delete").and_then(Value::as_array) {
                    for key in deleted.iter().filter_map(Value::as_str) {
                        store.remove(key);
                    }
                }
                if let Some(set) = object.get("set").and_then(Value::as_object) {
                    for (key, value) in set {
                        store.insert(key.clone(), value.clone());
                    }
                }
            }
            Ok(store)
        })
    }

    fn append(
        &self,
        writes: Vec<(String, Option<Value>)>,
    ) -> BoxFuture<'static, Result<(), String>> {
        let Some(harness) = self.harness.upgrade() else {
            return Box::pin(async { Ok(()) });
        };
        Box::pin(async move {
            let mut set = serde_json::Map::new();
            let mut delete = Vec::new();
            for (key, value) in writes {
                match value {
                    Some(value) => {
                        set.insert(key, value);
                    }
                    None => delete.push(Value::String(key)),
                }
            }
            let data = serde_json::json!({ "set": set, "delete": delete });
            harness
                .append_custom_entry(CODEMODE_STORE_ENTRY_TYPE, Some(data))
                .await
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }
}
