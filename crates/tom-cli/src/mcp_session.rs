//! The MCP tools a session carries.
//!
//! [`McpSession`] owns the live server connections. It must outlive the harness:
//! dropping it kills every stdio child, which would make every MCP tool fail
//! mid-session. It therefore travels alongside the extension session guard
//! rather than being flattened into a tool list.

use std::sync::Arc;

use tom_agent::agent_tool::AgentTool;
use tom_mcp::LoadedServers;

/// Live MCP connections plus the tools they expose.
pub struct McpSession {
    loaded: LoadedServers,
}

impl McpSession {
    pub(crate) fn new(loaded: LoadedServers) -> Self {
        Self { loaded }
    }

    /// One `AgentTool` per discovered MCP tool.
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.loaded.agent_tools()
    }

    /// `(server, tool count)` for the startup summary.
    pub fn summary_lines(&self) -> Vec<String> {
        self.loaded
            .tools_by_server()
            .into_iter()
            .map(|(server, tools)| format!("{server}: {} tool(s)", tools.len()))
            .collect()
    }

    /// Servers that failed to connect, as `name: reason`.
    pub fn failures(&self) -> Vec<String> {
        self.loaded
            .outcomes()
            .iter()
            .filter_map(|outcome| {
                outcome
                    .error
                    .as_ref()
                    .map(|error| format!("{}: {error}", outcome.name))
            })
            .collect()
    }

    /// Inventory text for a `/mcp`-style listing.
    pub fn inventory(&self) -> String {
        self.loaded.summary()
    }

    pub fn is_empty(&self) -> bool {
        self.loaded.tools().is_empty()
    }

    /// The tools that should be declared to the model rather than left for
    /// `tool_search` to load, per each server's `exposure`.
    pub fn direct_tool_names(&self, tool_search_available: bool) -> Vec<String> {
        self.loaded.direct_tool_names(tool_search_available)
    }
}

/// The namespaces a `codemode` script sees, one per connected server.
///
/// A namespace is what a server's tools share: the `mcp__<server>__` prefix, the
/// server's configured description, and its instructions. `codemode` groups the
/// declarations under it and `describeNamespace()` reports it, so a script can ask
/// "what does this server offer" without listing every tool by name.
pub fn codemode_namespaces(
    session: Option<&McpSession>,
) -> Vec<tom_tools::codemode::CodemodeNamespace> {
    let Some(session) = session else {
        return Vec::new();
    };
    session
        .loaded
        .tools_by_server()
        .into_iter()
        .filter(|(_, tools)| !tools.is_empty())
        .map(|(server, _)| tom_tools::codemode::CodemodeNamespace {
            name: format!("mcp__{server}"),
            aliases: vec![server.to_string()],
            description: session
                .loaded
                .clients()
                .iter()
                .find(|client| client.name() == server)
                .and_then(|client| client.description())
                .map(str::to_string),
            instructions: session
                .loaded
                .clients()
                .iter()
                .find(|client| client.name() == server)
                .and_then(|client| client.instructions())
                .map(str::to_string),
            prefix: format!("mcp__{server}__"),
        })
        .collect()
}
