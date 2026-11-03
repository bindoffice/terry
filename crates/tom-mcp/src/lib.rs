//! A minimal Model Context Protocol client for `tom`.
//!
//! Scope is deliberately narrow: connect to servers over stdio or streamable
//! HTTP, discover their tools, and expose those tools to the agent as ordinary
//! [`tom_agent::AgentTool`]s named `mcp__<server>__<tool>`. Resources, prompts,
//! sampling and roots are out of scope — this crate exists so a coding agent can
//! reuse the MCP tool ecosystem, not so it can be an MCP host.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use std::path::Path;
//!
//! let mut config = tom_mcp::McpConfig::load(Path::new("/home/me/.tom/agent/mcp.json"))?;
//! let project = tom_mcp::McpConfig::load(Path::new(".tom/mcp.json"))?;
//! config.merge(project);
//!
//! let (servers, issues) = config.resolve(tom_mcp::ConfigSource::User, Path::new("."));
//! for issue in &issues {
//!     eprintln!("mcp: {issue}");
//! }
//!
//! let connected = tom_mcp::connect_all(&servers).await;
//! for (server, tools) in connected.tools_by_server() {
//!     println!("{server}: {}", tools.len());
//! }
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod tools;
pub mod transport;

pub use client::{
    decorate_description, exposed_tool_name, normalize_arguments, parse_exposed_tool_name,
    render_inventory, McpClient, McpTool, McpToolOutcome, PROTOCOL_VERSION,
};
pub use config::{
    ConfigIssue, ConfigSource, Exposure, McpConfig, ResolvedServer, ServerEntry,
    Transport as ServerTransport,
};
pub use error::McpError;
pub use tools::create_mcp_tool;

use std::sync::Arc;

/// One server's connection attempt: either a live client or the reason it
/// failed. A failure never aborts the whole load.
#[derive(Debug)]
pub struct ServerOutcome {
    pub name: String,
    pub error: Option<String>,
    pub tools: Vec<McpTool>,
}

/// The result of loading every configured server.
#[derive(Default)]
pub struct LoadedServers {
    clients: Vec<Arc<McpClient>>,
    tools: Vec<McpTool>,
    outcomes: Vec<ServerOutcome>,
}

impl LoadedServers {
    /// Live clients, in configuration order.
    pub fn clients(&self) -> &[Arc<McpClient>] {
        &self.clients
    }

    /// Every discovered tool, across all connected servers.
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Per-server outcomes, including failures, for diagnostics and `/mcp`.
    pub fn outcomes(&self) -> &[ServerOutcome] {
        &self.outcomes
    }

    /// `(server, tools)` pairs, for listings.
    pub fn tools_by_server(&self) -> Vec<(&str, Vec<&McpTool>)> {
        self.clients
            .iter()
            .map(|client| {
                let tools = self
                    .tools
                    .iter()
                    .filter(|tool| tool.server == client.name())
                    .collect();
                (client.name(), tools)
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// One-line-per-server summary suitable for a startup notice.
    pub fn summary(&self) -> String {
        render_inventory(&self.clients, &self.tools)
    }

    /// Build `AgentTool`s for every discovered tool, ready to merge into the
    /// agent's tool set.
    pub fn agent_tools(&self) -> Vec<Arc<dyn tom_agent::agent_tool::AgentTool>> {
        let mut out = Vec::with_capacity(self.tools.len());
        for tool in &self.tools {
            let Some(client) = self
                .clients
                .iter()
                .find(|client| client.name() == tool.server)
            else {
                continue;
            };
            out.push(create_mcp_tool(tool.clone(), client.clone()));
        }
        out
    }

    /// Which of these tools should be declared to the model rather than left for
    /// `tool_search` to load.
    ///
    /// Needed before the tools are built so the host can split the registry into
    /// "declared" and "deferred" in one pass: `exposure` is per server, and the
    /// default depends on whether a `tool_search` tool exists at all.
    pub fn direct_tool_names(&self, tool_search_available: bool) -> Vec<String> {
        self.tools
            .iter()
            .filter(|tool| {
                let setting = self
                    .clients
                    .iter()
                    .find(|client| client.name() == tool.server)
                    .and_then(|client| client.exposure());
                !crate::config::Exposure::resolve(setting, tool_search_available).is_deferred()
            })
            .map(|tool| tool.exposed_name.clone())
            .collect()
    }
}

/// Connect every server in parallel and collect their tools.
///
/// Servers are independent, so one slow or broken process cannot delay the
/// others. Each failure is recorded in the outcome instead of propagating.
pub async fn connect_all(servers: &[ResolvedServer]) -> LoadedServers {
    let attempts = servers.iter().cloned().map(|server| async move {
        let name = server.name.clone();
        match McpClient::connect(server).await {
            Ok(client) => match client.list_tools().await {
                Ok(tools) => (Some(client), tools, name, None),
                Err(error) => {
                    // A server that initializes but cannot list tools is still
                    // useless; report it as a failure so the user can act.
                    (None, Vec::new(), name, Some(error.to_string()))
                }
            },
            Err(error) => (None, Vec::new(), name, Some(error.to_string())),
        }
    });

    let results = futures::future::join_all(attempts).await;
    let mut loaded = LoadedServers::default();
    for (client, tools, name, error) in results {
        if let Some(client) = client {
            loaded.clients.push(client);
        }
        loaded.tools.extend(tools.iter().cloned());
        loaded.outcomes.push(ServerOutcome { name, error, tools });
    }
    loaded
}
