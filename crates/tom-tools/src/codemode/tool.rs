//! The `codemode` tool: run model-written JavaScript that calls other tools.
//!
//! Port of `packages/coding-agent/src/extensions/codemode/{tool,execute}.ts` plus
//! the sandbox in `packages/codemode`. The point (`todo.md` §3) is what a model
//! is bad at when every step is one tool call: batch, chain, filter, branch.
//! Nested calls and their results never enter the model's context — only the
//! script's own output does.
//!
//! A script may do exactly what the host injects: `tools.<name>`, the discovery
//! globals (`searchTools`, `describeTool`, `describeNamespace`, `ALL_TOOLS`),
//! `text`/`image`/`console`/`exit`, and `store`/`load`. There is no file system,
//! network, timer or module access, and the sandbox can reach the host only
//! through the bridge the prelude keeps in a closure.
//!
//! Tool calls go through the *same* pipeline a model-issued call does —
//! validation, `before_tool_call`/`after_tool_call` hooks, permission gates —
//! because nested calls are executed by the host as ordinary tool calls. Only
//! where their result goes differs: back into the script.
//!
//! `store()`/`load()` keep values across calls as a `codemode-store` custom entry
//! on the session branch, so `/tree`, resume and fork each see the values written
//! on their own path.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use tom_ai::types::{ImageContent, ImageContentType, Schema, TextContent, TextContentType, Tool};

use super::declarations::{render_tool_sample, to_codemode_identifier, CodemodeDeclaration};
use super::sandbox::{
    run_codemode_script, CodemodeGlobalSpec, CodemodeHost, CodemodeOutcome, CodemodeOutputItem,
    CodemodeSandboxOptions, CodemodeToolSpec,
};
use super::source::{parse_codemode_source, CodemodeSourceOptions};

/// The tool the model calls.
pub const CODEMODE_TOOL_NAME: &str = "codemode";

/// Custom entry holding one script's `store()` writes.
pub const CODEMODE_STORE_ENTRY_TYPE: &str = "codemode-store";

/// Default token budget for script output.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 10_000;

/// How the description presents tools the model already has declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodemodeMode {
    /// Keep direct tools declared; list only the tools a script can reach that the
    /// model cannot (the deferred ones).
    #[default]
    On,
    /// List every script-callable tool in the description.
    Only,
}

/// Estimated tokens the tool declarations in the description may use.
pub const DEFAULT_CODEMODE_INLINE_BUDGET: usize = 3000;

/// Characters per token when estimating a declaration's cost.
const CHARS_PER_TOKEN: usize = 4;

/// How many matches `searchTools()` returns by default.
const DEFAULT_SEARCH_LIMIT: usize = 8;

/// Fixed part of the description: the script environment and its globals. It
/// deliberately does not list tools, so it stays the same while servers connect
/// (a description that changed per request would invalidate the prompt cache).
const DESCRIPTION_INTRO: &str = "Run JavaScript that calls other tools. The input is raw JavaScript (not JSON, no code fence), run as an async function body in a QuickJS sandbox: top-level `await` and `return` work. No Node, file system, network, or timers.
- `await tools.<name>({ ...args })` resolves to a string, or an object if the tool's declaration says so, and rejects with an Error on failure.
- Calls still running when the script ends are cancelled.
- Optional first line: `// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}`";

const DESCRIPTION_GLOBALS: &str = "Globals:
- `text(value)`, `image(dataUrlOrImageBlock)`, `console.log(...)`, and top-level `return` add output; `exit()` ends the script. With several text items, each starts with a `==> text N/M <==` line, and `console` lines follow the other output in one `<console_output>` block.
- `store(key, value)` and `load(key)` keep JSON values across codemode calls.
- `ALL_TOOLS`, `await searchTools(query, { limit?, namespace? })`, `await describeTool(name)`, `await describeNamespace(name)`: find tools the description does not list.";

/// Input of the `codemode` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CodemodeInput {
    /// Raw JavaScript source.
    pub code: String,
}

/// One tool a script may call.
#[derive(Debug, Clone)]
pub struct CodemodeTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// The tool's structured output schema; without one a script gets its text.
    pub output_schema: Option<Value>,
    /// Whether the model is also given this tool directly. Only matters for what
    /// the description lists: a declared tool is already visible, so repeating it
    /// in the codemode description spends tokens on nothing.
    pub direct: bool,
}

impl CodemodeTool {
    pub fn declaration(&self) -> CodemodeDeclaration {
        CodemodeDeclaration {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.parameters.clone(),
            output_schema: self.output_schema.clone(),
        }
    }

    pub fn sample(&self) -> String {
        render_tool_sample(&self.declaration())
    }

    fn spec(&self) -> CodemodeToolSpec {
        CodemodeToolSpec {
            name: self.name.clone(),
            js_name: to_codemode_identifier(&self.name),
            description: self.sample(),
        }
    }
}

/// A group of tools named by a common prefix, such as an MCP server's
/// `mcp__github__`. `searchTools()` filters by it and `describeNamespace()`
/// reports its description and instructions.
#[derive(Debug, Clone, Default)]
pub struct CodemodeNamespace {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    /// Tool names starting with it belong to the namespace; the longest matching
    /// prefix wins.
    pub prefix: String,
}

impl CodemodeNamespace {
    fn matches(&self, query: &str) -> bool {
        let query_id = to_codemode_identifier(query);
        std::iter::once(&self.name)
            .chain(self.aliases.iter())
            .any(|name| name == query || to_codemode_identifier(name) == query_id)
    }

    fn of<'a>(namespaces: &'a [CodemodeNamespace], tool: &str) -> Option<&'a CodemodeNamespace> {
        namespaces
            .iter()
            .filter(|namespace| tool.starts_with(&namespace.prefix))
            .max_by_key(|namespace| namespace.prefix.len())
    }
}

/// Where `store()` reads from and where one script's writes go.
pub trait CodemodeStore: Send + Sync {
    fn load(&self) -> BoxFuture<'static, Result<BTreeMap<String, Value>, String>>;

    /// Persist one script's writes.
    fn append(
        &self,
        writes: Vec<(String, Option<Value>)>,
    ) -> BoxFuture<'static, Result<(), String>>;
}

/// A store that keeps nothing: `load()` sees an empty map and writes are dropped.
/// Used by a harness that has no session hook (an embedded agent, tests).
pub struct NoCodemodeStore;

impl CodemodeStore for NoCodemodeStore {
    fn load(&self) -> BoxFuture<'static, Result<BTreeMap<String, Value>, String>> {
        Box::pin(async { Ok(BTreeMap::new()) })
    }

    fn append(
        &self,
        _writes: Vec<(String, Option<Value>)>,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// Everything the tool needs to know about the session it runs in.
///
/// The tool exists before the harness does (tools are built first and the
/// harness adopts them), while nested calls are a harness operation. This cell
/// closes that gap the same way `tool_search`'s does: it is created with the
/// static half and filled in once `AgentHarness::create` returns.
pub trait CodemodeHostBinding: Send + Sync {
    /// The tools a script may call, live: a tool loaded by `tool_search` during
    /// this turn is callable in the next script.
    fn callable_tools(&self) -> Vec<CodemodeTool>;

    fn namespaces(&self) -> Vec<CodemodeNamespace> {
        Vec::new()
    }

    /// Run one nested tool call through the agent's tool pipeline.
    fn run_tool(
        &self,
        name: String,
        args: Value,
        signal: CancellationToken,
    ) -> BoxFuture<'static, Result<NestedToolOutcome, String>>;
}

/// What a nested call produced.
#[derive(Debug, Clone)]
pub struct NestedToolOutcome {
    pub content: Vec<TextContentOrImage>,
    /// The tool's structured value, when it produced one.
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl NestedToolOutcome {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                TextContentOrImage::Text(text) => Some(text.text.as_str()),
                TextContentOrImage::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The `codemode` tool.
pub struct CodemodeToolImpl {
    schema: Tool,
    /// The description rebuilt once the host (and therefore the tool set) is
    /// known. `OnceLock` rather than a lock because `schema()` hands out a
    /// borrow: the set the model saw first is the one it keeps, and tools that
    /// appear later are reachable through `searchTools()`.
    bound_schema: std::sync::OnceLock<Tool>,
    store: std::sync::Mutex<Arc<dyn CodemodeStore>>,
    host: std::sync::OnceLock<Arc<dyn CodemodeHostBinding>>,
    /// Candidates used while no host is bound (embedded callers, tests).
    fallback_tools: Vec<CodemodeTool>,
    fallback_namespaces: Vec<CodemodeNamespace>,
    inline_budget: usize,
    mode: CodemodeMode,
    default_timeout: std::time::Duration,
}

impl CodemodeToolImpl {
    /// A tool over a fixed tool list, with no store and no nested-call pipeline.
    ///
    /// Its calls resolve against `fallback_tools` only, which is what an embedded
    /// caller or a test wants; the CLI binds a harness with [`Self::set_host`].
    pub fn new(tools: Vec<CodemodeTool>) -> Arc<Self> {
        Arc::new(Self {
            schema: Self::build_schema(
                &tools,
                &[],
                DEFAULT_CODEMODE_INLINE_BUDGET,
                CodemodeMode::default(),
            ),
            bound_schema: std::sync::OnceLock::new(),
            store: std::sync::Mutex::new(Arc::new(NoCodemodeStore)),
            host: std::sync::OnceLock::new(),
            fallback_tools: tools,
            fallback_namespaces: Vec::new(),
            inline_budget: DEFAULT_CODEMODE_INLINE_BUDGET,
            mode: CodemodeMode::default(),
            default_timeout: super::DEFAULT_TIMEOUT,
        })
    }

    /// Build the tool with its dependencies.
    pub fn with_options(options: CodemodeToolOptions) -> Arc<Self> {
        let tools = options.tools;
        Arc::new(Self {
            schema: Self::build_schema(
                &tools,
                &options.namespaces,
                options.inline_budget,
                options.mode,
            ),
            bound_schema: std::sync::OnceLock::new(),
            store: std::sync::Mutex::new(options.store),
            host: std::sync::OnceLock::new(),
            fallback_tools: tools,
            fallback_namespaces: options.namespaces,
            inline_budget: options.inline_budget,
            mode: options.mode,
            default_timeout: options.default_timeout,
        })
    }

    /// Point the tool at the harness that runs its nested calls, and name the
    /// tools a script can call in the description.
    pub fn set_host(&self, host: Arc<dyn CodemodeHostBinding>) {
        let namespaces = host.namespaces();
        let tools = host.callable_tools();
        let _ = self.bound_schema.set(Self::build_schema(
            &tools,
            &namespaces,
            self.inline_budget,
            self.mode,
        ));
        let _ = self.host.set(host);
    }

    /// Point `store()`/`load()` at the session that owns them.
    pub fn set_store(self: &Arc<Self>, store: Arc<dyn CodemodeStore>) {
        // The store is read per execution, so a session swap (fork, `/tree`) takes
        // effect on the next script without rebuilding the tool.
        *self.store.lock().unwrap_or_else(|error| error.into_inner()) = store;
    }

    fn host(&self) -> Option<&Arc<dyn CodemodeHostBinding>> {
        self.host.get()
    }

    fn store_handle(&self) -> Arc<dyn CodemodeStore> {
        Arc::clone(&self.store.lock().unwrap_or_else(|error| error.into_inner()))
    }

    fn callable(&self) -> Vec<CodemodeTool> {
        match self.host() {
            Some(host) => host.callable_tools(),
            None => self.fallback_tools.clone(),
        }
    }

    fn namespaces(&self) -> Vec<CodemodeNamespace> {
        match self.host() {
            Some(host) => host.namespaces(),
            None => self.fallback_namespaces.clone(),
        }
    }

    fn build_schema(
        tools: &[CodemodeTool],
        namespaces: &[CodemodeNamespace],
        inline_budget: usize,
        mode: CodemodeMode,
    ) -> Tool {
        Tool {
            name: CODEMODE_TOOL_NAME.to_string(),
            description: codemode_description(tools, namespaces, inline_budget, mode),
            parameters: Schema::new(
                serde_json::to_value(schemars::schema_for!(CodemodeInput)).unwrap_or_default(),
            ),
            constrained_sampling: None,
        }
    }
}

/// How to build the tool.
pub struct CodemodeToolOptions {
    pub tools: Vec<CodemodeTool>,
    pub namespaces: Vec<CodemodeNamespace>,
    pub store: Arc<dyn CodemodeStore>,
    pub inline_budget: usize,
    /// How the description presents tools the model already has declared.
    pub mode: CodemodeMode,
    pub default_timeout: std::time::Duration,
}

impl Default for CodemodeToolOptions {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            namespaces: Vec::new(),
            store: Arc::new(NoCodemodeStore),
            inline_budget: DEFAULT_CODEMODE_INLINE_BUDGET,
            mode: CodemodeMode::default(),
            default_timeout: super::DEFAULT_TIMEOUT,
        }
    }
}

/// The model-facing description: the helper list, how to find unlisted tools, and
/// one section per listed tool grouped by namespace, within `inline_budget`
/// estimated tokens. Tools that do not fit are left out; `searchTools()` finds
/// them.
pub fn codemode_description(
    tools: &[CodemodeTool],
    namespaces: &[CodemodeNamespace],
    inline_budget: usize,
    mode: CodemodeMode,
) -> String {
    let mut sections = vec![
        DESCRIPTION_INTRO.to_string(),
        DESCRIPTION_GLOBALS.to_string(),
    ];
    // `on` keeps the declared tools declared; repeating them here would spend
    // tokens to say what the model already has. `only` lists everything callable.
    let tools: Vec<&CodemodeTool> = tools
        .iter()
        .filter(|tool| mode == CodemodeMode::Only || !tool.direct)
        .collect();
    if tools.is_empty() {
        return sections.join("\n\n");
    }
    // Group by namespace: tools without one come first, then namespaces by name.
    let mut groups: Vec<CatalogGroup<'_>> = vec![(None, Vec::new())];
    for tool in &tools {
        let namespace = CodemodeNamespace::of(namespaces, &tool.name);
        let section = format!(
            "### `{id}`{raw}\n{sample}",
            id = to_codemode_identifier(&tool.name),
            raw = if to_codemode_identifier(&tool.name) == tool.name {
                String::new()
            } else {
                format!(" (`{}`)", tool.name)
            },
            sample = tool
                .sample()
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .trim(),
        );
        let index = match namespace {
            None => 0,
            Some(namespace) => match groups
                .iter()
                .position(|(existing, _)| existing.is_some_and(|n| n.name == namespace.name))
            {
                Some(index) => index,
                None => {
                    groups.push((Some(namespace), Vec::new()));
                    groups.len() - 1
                }
            },
        };
        groups[index].1.push((tool.name.clone(), section));
    }
    let listed = select_listed(&mut groups, inline_budget);
    let mut listing = vec!["Tools scripts can call (`await tools.<name>(args)`):".to_string()];
    for (namespace, entries) in &groups {
        if entries.is_empty() {
            continue;
        }
        let visible: Vec<&(String, String)> = entries
            .iter()
            .filter(|(name, _)| listed.contains(name))
            .collect();
        if let Some(namespace) = namespace {
            let note = if visible.len() == entries.len() {
                ""
            } else if visible.is_empty() {
                " (tools not listed)"
            } else {
                " (some tools not listed)"
            };
            let description = namespace
                .description
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            listing.push(format!(
                "## {}{note}{}",
                namespace.name,
                if description.is_empty() {
                    String::new()
                } else {
                    format!("\n{description}")
                }
            ));
        } else if visible.len() < entries.len() {
            listing.push("Some tools are not listed: find them with `searchTools()`.".to_string());
        }
        for (_, section) in visible {
            listing.push(section.clone());
        }
    }
    sections.push(listing.join("\n\n"));
    sections.join("\n\n")
}

/// `(namespace, [(tool name, rendered section)])`, in the order the description
/// prints them.
type CatalogGroup<'a> = (Option<&'a CodemodeNamespace>, Vec<(String, String)>);

/// Pick the tool sections that fit the budget: in each round every group places
/// its cheapest remaining tool; a group whose next tool does not fit drops out
/// while the others continue. Every namespace is represented before any namespace
/// is complete.
fn select_listed(
    groups: &mut [CatalogGroup<'_>],
    budget: usize,
) -> std::collections::BTreeSet<String> {
    let mut queues: Vec<Vec<(String, usize)>> = groups
        .iter()
        .map(|(_, entries)| {
            let mut queue: Vec<(String, usize)> = entries
                .iter()
                .map(|(name, section)| {
                    (
                        name.clone(),
                        section.chars().count().div_ceil(CHARS_PER_TOKEN),
                    )
                })
                .collect();
            queue.sort_by_key(|(_, cost)| *cost);
            queue
        })
        .collect();
    let mut listed = std::collections::BTreeSet::new();
    let mut remaining = budget;
    let mut active: Vec<usize> = (0..queues.len())
        .filter(|index| !queues[*index].is_empty())
        .collect();
    while !active.is_empty() {
        let mut next_active = Vec::new();
        for index in active {
            let Some((name, cost)) = queues[index].first().cloned() else {
                continue;
            };
            if cost > remaining {
                continue;
            }
            remaining -= cost;
            listed.insert(name);
            queues[index].remove(0);
            if !queues[index].is_empty() {
                next_active.push(index);
            }
        }
        active = next_active;
    }
    listed
}

/// The globals a script uses to find tools the description does not list.
///
/// `ALL_TOOLS` needs no global: the prelude builds it from the tool list it was
/// given.
/// Cancels a token when dropped, so abandoning a future cancels it.
struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn discovery_globals() -> Vec<CodemodeGlobalSpec> {
    ["searchTools", "describeTool", "describeNamespace"]
        .into_iter()
        .map(|name| CodemodeGlobalSpec {
            name: name.to_string(),
            spread: true,
        })
        .collect()
}

/// The sandbox host: resolves `tools.*` against the live tool set and answers the
/// discovery globals from the same metadata.
struct SandboxHost {
    binding: Option<Arc<dyn CodemodeHostBinding>>,
    tools: Vec<CodemodeTool>,
    namespaces: Vec<CodemodeNamespace>,
}

impl SandboxHost {
    fn find(&self, name: &str) -> Option<&CodemodeTool> {
        self.tools
            .iter()
            .find(|tool| tool.name == name || to_codemode_identifier(&tool.name) == name)
    }

    fn search(&self, args: &Value) -> Result<Value, String> {
        let Value::Array(args) = args else {
            return Err("searchTools() expects a query string".into());
        };
        let Some(query) = args.first().and_then(Value::as_str) else {
            return Err("searchTools() expects a query string".into());
        };
        let options = args.get(1).and_then(Value::as_object);
        let limit = match options.and_then(|options| options.get("limit")) {
            None | Some(Value::Null) => DEFAULT_SEARCH_LIMIT,
            Some(value) => match value.as_u64() {
                Some(limit) if limit > 0 => limit as usize,
                _ => return Err("searchTools() limit must be a positive integer".into()),
            },
        };
        let namespace = match options.and_then(|options| options.get("namespace")) {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => return Err("searchTools() namespace must be a string".into()),
        };
        let documents: Vec<crate::tool_search::ToolSearchDocument> = self
            .tools
            .iter()
            .filter(|tool| match &namespace {
                None => true,
                Some(query) => CodemodeNamespace::of(&self.namespaces, &tool.name)
                    .is_some_and(|namespace| namespace.matches(query)),
            })
            .map(|tool| {
                crate::tool_search::create_tool_search_document(
                    &tool.name,
                    &tool.description,
                    &tool.parameters,
                )
            })
            .collect();
        let matches = crate::tool_search::Bm25Ranker::default().rank(query, &documents, limit);
        Ok(Value::Array(
            matches
                .into_iter()
                .map(|matched| {
                    let tool = self.find(&matched.name);
                    serde_json::json!({
                        "name": to_codemode_identifier(&matched.name),
                        "description": tool.map(|tool| tool.sample()).unwrap_or_default(),
                    })
                })
                .collect(),
        ))
    }

    fn describe_tool(&self, args: &Value) -> Result<Value, String> {
        let name = args
            .as_array()
            .and_then(|args| args.first())
            .and_then(Value::as_str)
            .ok_or_else(|| "describeTool() expects a tool name".to_string())?;
        Ok(match self.find(name) {
            Some(tool) => Value::String(tool.sample()),
            None => Value::Null,
        })
    }

    fn describe_namespace(&self, args: &Value) -> Result<Value, String> {
        let name = args
            .as_array()
            .and_then(|args| args.first())
            .and_then(Value::as_str)
            .ok_or_else(|| "describeNamespace() expects a namespace name".to_string())?;
        let Some(namespace) = self
            .namespaces
            .iter()
            .find(|namespace| namespace.matches(name))
        else {
            return Ok(Value::Null);
        };
        let tools: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| tool.name.starts_with(&namespace.prefix))
            .map(|tool| to_codemode_identifier(&tool.name))
            .collect();
        let mut value = serde_json::json!({ "name": namespace.name, "tools": tools });
        if let Some(description) = &namespace.description {
            value["description"] = Value::String(description.clone());
        }
        if let Some(instructions) = &namespace.instructions {
            value["instructions"] = Value::String(instructions.clone());
        }
        Ok(value)
    }
}

impl CodemodeHost for SandboxHost {
    fn call_tool(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let Some(binding) = self.binding.clone() else {
            return Box::pin(async {
                Err("this codemode run has no tool pipeline to call".to_string())
            });
        };
        // A nested call is cancelled when the script stops waiting for it: the
        // sandbox drops this future, and the guard turns that drop into a cancel
        // so a tool that is mid-command (bash) stops instead of running on after
        // the script has returned.
        let signal = CancellationToken::new();
        let guard = CancelOnDrop(signal.clone());
        let tool = self.find(&name).cloned();
        Box::pin(async move {
            let _guard = guard;
            let Some(tool) = tool else {
                return Err(format!("unknown tool `{name}`"));
            };
            let outcome = binding.run_tool(tool.name.clone(), args, signal).await?;
            let text = outcome.text();
            match (&tool.output_schema, outcome.structured) {
                (Some(_), Some(value)) if !outcome.is_error => Ok(value),
                // A structured tool that failed still resolves when it produced a
                // value (`bash`'s `{ output, exitCode }` for a nonzero exit); the
                // script can check the fields.
                (Some(_), Some(value)) => Ok(value),
                _ if outcome.is_error => Err(if text.is_empty() {
                    format!("Tool \"{name}\" failed")
                } else {
                    text
                }),
                _ => Ok(Value::String(text)),
            }
        })
    }

    fn call_global(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let result = match name.as_str() {
            "searchTools" => self.search(&args),
            "describeTool" => self.describe_tool(&args),
            "describeNamespace" => self.describe_namespace(&args),
            other => Err(format!("unknown global `{other}`")),
        };
        Box::pin(async move { result })
    }
}

#[async_trait]
impl AgentTool for CodemodeToolImpl {
    fn schema(&self) -> &Tool {
        // Once the host is bound the description can name the tools a script may
        // call; before that it only describes the environment.
        self.bound_schema.get().unwrap_or(&self.schema)
    }

    fn label(&self) -> &str {
        CODEMODE_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: CodemodeInput = serde_json::from_value(params)
            .map_err(|error| AgentError::Validation(format!("codemode input invalid: {error}")))?;
        let parsed = parse_codemode_source(&input.code)
            .map_err(|error| AgentError::Validation(error.to_string()))?;
        let source: CodemodeSourceOptions = parsed.options;
        // A script shows as running before its first call, so the row's timer
        // starts where the model's turn did.
        on_update(AgentToolResult::default());

        let tools = self.callable();
        let namespaces = self.namespaces();
        let store = match self.store_handle().load().await {
            Ok(store) => store,
            Err(error) => {
                return Ok(AgentToolResult::text(format!(
                    "Script failed\nOutput:\n\nCould not read the codemode store: {error}"
                )))
            }
        };
        let host = Arc::new(SandboxHost {
            binding: self.host().cloned(),
            tools: tools.clone(),
            namespaces,
        });
        let timeout = source
            .timeout_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or(self.default_timeout);
        let outcome = run_codemode_script(
            CodemodeSandboxOptions {
                tools: tools.iter().map(CodemodeTool::spec).collect(),
                globals: discovery_globals(),
                store,
                timeout: Some(timeout),
                ..Default::default()
            },
            parsed.code,
            host,
            signal.child_token(),
        )
        .await;

        let max_tokens = source
            .max_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
        let content = render(&outcome, max_tokens);
        // A failed script is an error result, the same way a nonzero exit is for
        // `bash`: `is_error` is what tells the model to read the output instead of
        // treating a half-finished job as done. Its partial output is kept, so the
        // model can see what the script managed before it failed.
        if outcome.is_ok() {
            if !outcome.store_writes.is_empty() {
                if let Err(error) = self
                    .store_handle()
                    .append(outcome.store_writes.clone())
                    .await
                {
                    tracing::warn!("codemode store write failed: {error}");
                }
            }
            return Ok(AgentToolResult {
                content,
                details: details_json(&outcome),
                usage: None,
                added_tool_names: Vec::new(),
                terminate: false,
            });
        }
        Err(AgentError::Tool(
            content
                .into_iter()
                .filter_map(|block| match block {
                    TextContentOrImage::Text(text) => Some(text.text),
                    TextContentOrImage::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

/// Lay the script's result out the way the model reads it: a header, the text
/// items, the console block, and on failure the error with the calls already
/// made.
fn render(outcome: &CodemodeOutcome, max_tokens: u64) -> Vec<TextContentOrImage> {
    let mut items: Vec<TextContentOrImage> = Vec::new();
    let text_items: Vec<&String> = outcome
        .output
        .iter()
        .filter_map(|item| match item {
            CodemodeOutputItem::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    let mut returned = None;
    if outcome.is_ok() {
        if let Some(value) = &outcome.value {
            returned = Some(match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            });
        }
    }
    let total = text_items.len() + usize::from(returned.is_some());
    let mut index = 0;
    let mut push_text = |text: String, items: &mut Vec<TextContentOrImage>| {
        index += 1;
        if total > 1 {
            items.push(TextContentOrImage::Text(TextContent {
                kind: TextContentType,
                text: format!("==> text {index}/{total} <==\n{text}"),
                text_signature: None,
            }));
        } else {
            items.push(TextContentOrImage::Text(TextContent {
                kind: TextContentType,
                text,
                text_signature: None,
            }));
        }
    };
    for text in &text_items {
        push_text((*text).clone(), &mut items);
    }
    for item in &outcome.output {
        if let CodemodeOutputItem::Image { data, mime_type } = item {
            items.push(TextContentOrImage::Image(ImageContent {
                kind: ImageContentType,
                data: Arc::from(data.as_str()),
                mime_type: mime_type.clone(),
            }));
        }
    }
    if let Some(value) = returned {
        push_text(value, &mut items);
    }
    let console_lines: Vec<&String> = outcome
        .output
        .iter()
        .filter_map(|item| match item {
            CodemodeOutputItem::Console(text) => Some(text),
            _ => None,
        })
        .collect();
    if !console_lines.is_empty() {
        items.push(TextContentOrImage::text(format!(
            "<console_output>\n{}\n</console_output>",
            console_lines
                .iter()
                .map(|line| line.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }
    let is_error = !outcome.is_ok();
    if let Some(error) = &outcome.error {
        items.push(TextContentOrImage::text(format!(
            "Script error:\n{}\n\n{}",
            error.message,
            call_summary(&outcome.calls)
        )));
    }
    let header = format!(
        "Script {}\nWall time {:.1} seconds\nOutput:\n",
        if is_error { "failed" } else { "completed" },
        outcome.elapsed.as_secs_f64()
    );
    let joined = join_adjacent(items);
    let (items, _) = truncate_output(joined, max_tokens);
    let mut content = vec![TextContentOrImage::text(header)];
    content.extend(items);
    content
}

fn call_summary(calls: &[super::sandbox::CodemodeCallRecord]) -> String {
    if calls.is_empty() {
        return "No tool calls were made.".to_string();
    }
    format!(
        "Tool calls made before the failure (they are not undone): {}",
        calls
            .iter()
            .map(|call| format!(
                "{} ({})",
                call.name,
                match call.status {
                    super::sandbox::CodemodeCallStatus::Ok => "ok",
                    super::sandbox::CodemodeCallStatus::Error => "error",
                    super::sandbox::CodemodeCallStatus::Cancelled => "cancelled",
                }
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Join adjacent text blocks into one, each part starting on its own line. The
/// separator check reads the last part rather than the joined text: reading the
/// end of a growing concatenation copies it on every item.
fn join_adjacent(items: Vec<TextContentOrImage>) -> Vec<TextContentOrImage> {
    let mut joined = Vec::with_capacity(items.len());
    let mut parts: Vec<String> = Vec::new();
    let mut last_part = String::new();
    for item in items {
        match item {
            TextContentOrImage::Image(image) => {
                if !parts.is_empty() {
                    joined.push(TextContentOrImage::text(parts.join("")));
                    parts.clear();
                }
                last_part.clear();
                joined.push(TextContentOrImage::Image(image));
            }
            TextContentOrImage::Text(text) => {
                if !last_part.is_empty() && !last_part.ends_with('\n') {
                    parts.push("\n".to_string());
                    last_part = "\n".to_string();
                }
                if !text.text.is_empty() {
                    last_part = text.text.clone();
                }
                parts.push(text.text);
            }
        }
    }
    if !parts.is_empty() {
        joined.push(TextContentOrImage::text(parts.join("")));
    }
    joined
}

/// Apply the token budget: when the text exceeds it, keep the start and the end
/// and say how much was cut. Images follow the text.
fn truncate_output(
    items: Vec<TextContentOrImage>,
    max_tokens: u64,
) -> (Vec<TextContentOrImage>, bool) {
    let combined: String = items
        .iter()
        .filter_map(|item| match item {
            TextContentOrImage::Text(text) => Some(text.text.as_str()),
            TextContentOrImage::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let budget = (max_tokens as usize).saturating_mul(CHARS_PER_TOKEN);
    if combined.chars().count() <= budget || budget == 0 {
        return (items, false);
    }
    let chars: Vec<char> = combined.chars().collect();
    let head_chars = budget / 2;
    let tail_chars = budget - head_chars;
    let head: String = chars.iter().take(head_chars).collect();
    let tail: String = chars
        .iter()
        .skip(chars.len().saturating_sub(tail_chars))
        .collect();
    let removed = chars.len() - head_chars - tail_chars;
    let text = format!(
        "Warning: truncated output (original token count: {})\nTotal output lines: {}\n\n{head}…{} tokens truncated…{tail}",
        combined.chars().count().div_ceil(CHARS_PER_TOKEN),
        combined.lines().count(),
        removed.div_ceil(CHARS_PER_TOKEN),
    );
    let mut out = vec![TextContentOrImage::text(text)];
    out.extend(
        items
            .into_iter()
            .filter(|item| matches!(item, TextContentOrImage::Image(_))),
    );
    (out, true)
}

fn details_json(outcome: &CodemodeOutcome) -> Value {
    serde_json::json!({
        "calls": outcome.calls.iter().map(|call| serde_json::json!({
            "name": call.name,
            "args": call.args,
            "status": match call.status {
                super::sandbox::CodemodeCallStatus::Ok => "ok",
                super::sandbox::CodemodeCallStatus::Error => "error",
                super::sandbox::CodemodeCallStatus::Cancelled => "cancelled",
            },
            "durationMs": call.duration.as_millis() as u64,
            "error": call.error,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> CodemodeTool {
        deferred_tool(name, description, false)
    }

    fn deferred_tool(name: &str, description: &str, direct: bool) -> CodemodeTool {
        CodemodeTool {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            output_schema: None,
            direct,
        }
    }

    #[test]
    fn the_description_lists_the_callable_tools() {
        let description = codemode_description(
            &[tool("read", "Read file contents")],
            &[],
            3000,
            CodemodeMode::Only,
        );
        assert!(description.contains("### `read`"), "{description}");
        assert!(description.contains("tools.read"), "{description}");
        assert!(description.contains("searchTools"), "{description}");
    }

    #[test]
    fn the_description_is_stable_without_tools() {
        let description = codemode_description(&[], &[], 3000, CodemodeMode::Only);
        assert!(!description.contains("### `"), "{description}");
    }

    #[test]
    fn mode_on_lists_only_the_tools_the_model_cannot_see() {
        // A declared tool is already in the model's tool list; repeating its
        // declaration in the description would spend tokens on nothing.
        let tools = [
            deferred_tool("read", "Read file contents", true),
            deferred_tool("mcp__docs__search", "Search docs", false),
        ];
        let on = codemode_description(&tools, &[], 3000, CodemodeMode::On);
        assert!(!on.contains("### `read`"), "{on}");
        assert!(on.contains("### `mcp__docs__search`"), "{on}");

        let only = codemode_description(&tools, &[], 3000, CodemodeMode::Only);
        assert!(only.contains("### `read`"), "{only}");
        assert!(only.contains("### `mcp__docs__search`"), "{only}");
    }

    #[test]
    fn namespace_tools_are_grouped_under_their_namespace() {
        let namespaces = vec![CodemodeNamespace {
            name: "mcp__docs".into(),
            aliases: vec!["docs".into()],
            description: Some("Documentation server".into()),
            instructions: None,
            prefix: "mcp__docs__".into(),
        }];
        let description = codemode_description(
            &[
                tool("read", "Read"),
                deferred_tool("mcp__docs__search", "Search docs", false),
            ],
            &namespaces,
            3000,
            CodemodeMode::Only,
        );
        assert!(description.contains("## mcp__docs"), "{description}");
        assert!(
            description.contains("Documentation server"),
            "{description}"
        );
    }

    #[test]
    fn a_tight_budget_leaves_tools_out() {
        let tools: Vec<CodemodeTool> = (0..20)
            .map(|index| tool(&format!("tool{index}"), &"x".repeat(400)))
            .collect();
        let description = codemode_description(&tools, &[], 100, CodemodeMode::Only);
        assert!(
            description.contains("not listed"),
            "a budget that cannot hold every tool says so"
        );
    }
}
