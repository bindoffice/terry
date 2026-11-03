# tom-mcp

[![crates.io](https://img.shields.io/crates/v/tom-mcp.svg)](https://crates.io/crates/tom-mcp)
[![docs.rs](https://docs.rs/tom-mcp/badge.svg)](https://docs.rs/tom-mcp)

A minimal [Model Context Protocol](https://modelcontextprotocol.io) client for
`tom`. It connects to servers over stdio or streamable HTTP, discovers their
tools, and exposes each one to the agent as an ordinary `AgentTool` named
`mcp__<server>__<tool>`.

Scope is deliberately narrow. This crate exists so a coding agent can reuse the
MCP *tool* ecosystem; it is not an MCP host. Resources, prompts, sampling,
elicitation and roots are out of scope, and the client advertises no
capabilities beyond tool calling, so a conforming server never sends a request it
would have to refuse.

## Install

```toml
[dependencies]
tom-mcp = "0.16"
```

## Usage

```rust,no_run
use std::path::Path;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
// User-level servers, then project servers on top.
let mut config = tom_mcp::McpConfig::load(Path::new("/home/me/.tom/agent/mcp.json"))?;
config.merge(tom_mcp::McpConfig::load(Path::new(".tom/mcp.json"))?);

let (servers, issues) = config.resolve(tom_mcp::ConfigSource::Project, Path::new("."));
for issue in &issues {
    eprintln!("mcp: {issue}");
}

let loaded = tom_mcp::connect_all(&servers).await;
for outcome in loaded.outcomes() {
    match &outcome.error {
        Some(error) => eprintln!("{}: {error}", outcome.name),
        None => println!("{}: {} tools", outcome.name, outcome.tools.len()),
    }
}

// Hand the tools to an agent.
let tools = loaded.agent_tools();
# Ok(())
# }
```

## Configuration

The file format matches other MCP clients, so an existing `mcpServers` block can
be copied verbatim:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
    },
    "docs": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" }
    }
  }
}
```

`command` selects stdio and `url` selects streamable HTTP. Also supported:
`args`, `env`, `cwd`, `headers`, `description`, `timeout` (seconds, default 60),
`exposure` (`direct` or `deferred`) and `enabled: false`. `${VAR}` in `env`,
`headers` and `cwd` expands from the process environment; a leading `~/` names
the home directory.

A project entry that carries no transport of its own is a *partial override*: it
changes only `enabled`, `description`, `timeout` and `exposure` of the inherited
user-level server, which is how one server is switched off (or made deferred)
inside a single project.

### Deferred tools

`"exposure": "deferred"` marks a server whose tools should not be declared to
the model up front. `LoadedServers::direct_tool_names` reports which are
declared; the rest are reachable only through a `tool_search` tool
([`tom-tools::tool_search`](https://crates.io/crates/tom-tools)). The default is
`direct`, because deferring is a trade: the model must spend a search call before
it can use the tool. Deferral pays off for servers with many tools.

## Design notes

- **One bad server never costs the others.** Invalid entries and failed
  connections are reported per server; everything else still loads.
- **Failures are results, not aborts.** A tool that returns an error is delivered
  to the model as text so it can adapt. Only a closed transport aborts, because
  continuing would produce one timeout per remaining tool call.
- **Names are flattened safely.** Provider tool names allow only
  `[A-Za-z0-9_-]`, so other characters are replaced. Names that actually lost
  information get a deterministic hash suffix, keeping `my-tools` and `my_tools`
  distinct.
- **Children die with the host.** stdio servers are spawned with `kill_on_drop`
  and an explicit kill on drop, so no orphan holds a port or a file lock after
  the CLI exits.
- **stderr is drained.** A server that logs heavily cannot fill the pipe buffer
  and deadlock its own next write.

## License

MIT
