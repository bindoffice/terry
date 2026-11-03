# tom-tools

[![crates.io](https://img.shields.io/crates/v/tom-tools.svg)](https://crates.io/crates/tom-tools)
[![docs.rs](https://docs.rs/tom-tools/badge.svg)](https://docs.rs/tom-tools)

The built-in agent tools and the execution environment they run against:
`read`, `write`, `edit`, `bash`, plus `grep`, `find`, `ls` and `powershell`,
implemented as `AgentTool`s that any agent built on
[`tom-agent`](https://crates.io/crates/tom-agent) can use. It also carries
`tool_search`, the BM25 retrieval tool that loads tools a host chose not to
declare up front (see [Tool search](#tool-search)).

The point of the crate is the split between *what a tool does* and *where it
runs*. Tools are written against `ExecutionEnv`, so the same `read` tool works
against the real filesystem, an in-memory fake, or a remote backend — and the
test suite never touches your disk.

## Install

```toml
[dependencies]
tom-tools = "0.3"
tom-agent = "0.3"
```

## Execution environments

| Backend | Purpose |
| ------- | ------- |
| `OsExecutionEnv` | The real filesystem and real shell processes; resolves relative paths against a configurable cwd |
| `InMemoryExecutionEnv` | A fake filesystem plus scripted shell responses, for deterministic tests |

Both implement `ExecutionEnv`; mutating backends additionally implement
`MutatingEnv`. Tools are constructed from an `ExecutionToolContext` that carries
the environment:

```rust
use std::sync::Arc;

use tom_tools::{create_bash_tool, create_write_tool, ExecutionToolContext, OsExecutionEnv};

let env = Arc::new(OsExecutionEnv::with_cwd("/tmp/workspace".into()));
let ctx = ExecutionToolContext::new(
    env.clone() as Arc<dyn tom_tools::ExecutionEnv>,
    Some(env as Arc<dyn tom_tools::MutatingEnv>),
);

let write_tool = create_write_tool(&ctx);
let bash_tool = create_bash_tool(&ctx, None);

let agent = tom_agent::AgentBuilder::new()
    .model(model)
    .stream_fn(stream_fn)
    .tools(vec![write_tool, bash_tool])
    .build()?;
```

A complete, runnable version — a faux provider that writes a file through the
real OS backend and reads it back — is in
[`examples/tools`](https://github.com/solos/tom/tree/main/examples/tools)
(`cargo run -p tools`).

## Testing against the in-memory backend

```rust
use std::sync::Arc;
use tom_tools::{ExecutionToolContext, InMemoryExecutionEnv, MutationQueueRegistry};

let env = Arc::new(InMemoryExecutionEnv::new());
let ctx = ExecutionToolContext::new(
    env.clone() as Arc<dyn tom_tools::ExecutionEnv>,
    Some(env as Arc<dyn tom_tools::MutatingEnv>),
);
let read = tom_tools::create_read_tool(&ctx, None);
```

Because the backend is in memory, a test asserts on tool *behaviour* rather than
on whatever happens to be in the working directory, and it runs in parallel with
every other test.

## Safety properties this crate is responsible for

- **Path containment.** Resolving a tool argument into a path is centralized in
  `path_utils`, so a tool cannot silently escape the environment root.
- **Mutation ordering.** `file_mutation_queue` serializes writes to the same
  path, so two concurrent `edit` calls cannot interleave into a corrupted file.
- **Bounded output.** Shell and file output are truncated through `truncate` +
  `shell_output` before reaching the model, so one `cat` of a huge log cannot
  blow up the context window.
- **Cancellation.** A blocking shell command is tied to the run's cancellation
  token, including process-tree termination on Windows.

## Tools available

`read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, `powershell`.
`create_*_tool` constructors all take the shared `ExecutionToolContext`;
`create_read_tool` and `create_bash_tool` additionally take an options argument
(`None` for the defaults).

The `tom` CLI loads `read`, `write`, `edit` and `bash` by default — the
Pi-compatible set — and exposes the rest through the `default_tools` setting.

## Tool search

Declaring every tool on every request stops scaling once a host loads MCP
servers: one filesystem server is ten-plus tools, each with a schema. `tool_search`
lets the model ask for a tool by describing it instead.

```rust
use std::sync::Arc;
use tom_tools::{create_tool_search_tool, DeferredTool, DeferredToolSource};

let source: Arc<dyn DeferredToolSource> = Arc::new(|| {
    vec![
        DeferredTool::new("mcp__fs__read_file", "Read a file from the filesystem."),
        DeferredTool::new("mcp__fs__list_dir", "List a directory."),
    ]
});
let tool = create_tool_search_tool(source);
```

The ranker is BM25 over the tool's name, description and parameter schema; a
query that *is* a tool name ranks that tool first. A hit is reported through
`AgentToolResult::added_tool_names`, which is what the host turns into a tool
load. `Bm25Ranker` and `create_tool_search_document` are public if you want the
ranking without the tool — `codemode`-style script APIs use the same ranker.

## Related crates

| Crate | Role |
| ----- | ---- |
| [`tom-agent`](https://crates.io/crates/tom-agent) | The `AgentTool` trait these tools implement |
| [`tom-ai`](https://crates.io/crates/tom-ai) | Message and tool-schema types |
| [`tom-harness`](https://crates.io/crates/tom-harness) | Sessions and the run loop that drives them |
| [`tom-cli`](https://crates.io/crates/tom-cli) | The terminal agent |

## Documentation

- Crate docs: <https://docs.rs/tom-tools>
- Architecture: [docs/architecture/overview.md](https://github.com/solos/tom/blob/main/docs/architecture/overview.md)

## License

MIT. See [LICENSE](https://github.com/solos/tom/blob/main/LICENSE)
and [NOTICE](https://github.com/solos/tom/blob/main/NOTICE).
