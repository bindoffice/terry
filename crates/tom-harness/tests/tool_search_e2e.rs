//! `tool_search` end-to-end: a deferred tool is registered but not declared, a
//! `tool_search` call loads it, and the load is durable.
//!
//! The scenario is the one that motivated the feature (`todo.md` §4): an MCP
//! server contributes more tools than the model should see on every request. The
//! assertions therefore cover the three things that can silently break:
//!
//! 1. the deferred tool is *not* in the provider's tool list before the load — if
//!    it were, the feature would cost tokens and change nothing;
//! 2. the load is recorded as an `active_tools_change` entry — without the entry
//!    the tool disappears on `/tree`, resume and fork while the model keeps
//!    calling it;
//! 3. the tool *is* declared on the next request, and a fresh harness built on
//!    the same session declares it too.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::message::AgentMessage;
use tom_agent::types::{AgentToolResult, ToolResultPartial};
use tom_ai::model::Model;
use tom_ai::provider::SimpleStreamOptions;
use tom_ai::providers::faux::{faux_assistant_message, FauxProvider, FauxScript, FauxStep};
use tom_ai::types::{Content, Schema, StopReason, Tool, ToolCall, UserContent, UserMessage};
use tom_ai::{Context, Provider};
use tom_harness::agent_harness::{AgentHarness, AgentLane, HarnessRunOutcome};
use tom_harness::session::memory::{InMemorySessionStorage, SystemClock};
use tom_harness::session::types::{Entry, EntryOrder, EntryQuery, SessionMetadata};
use tom_harness::session::{DefaultIdGenerator, Session};
use tom_harness::tool_search::ToolSearchToolCell;
use tom_harness::types::{AgentHarnessOptions, HarnessTool};
use tom_tools::{DeferredTool, TOOL_SEARCH_TOOL_NAME};

/// A tool that records being called. Stands in for an MCP tool: its description
/// and schema are the only things `tool_search` can rank on.
struct NamedTool {
    schema: Tool,
    calls: Arc<Mutex<Vec<String>>>,
    result: String,
}

fn named_tool(
    name: &str,
    description: &str,
    parameters: serde_json::Value,
    calls: Arc<Mutex<Vec<String>>>,
    result: &str,
) -> Arc<dyn AgentTool> {
    Arc::new(NamedTool {
        schema: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema::new(parameters),
            constrained_sampling: None,
        },
        calls,
        result: result.to_string(),
    })
}

#[async_trait]
impl AgentTool for NamedTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        &self.schema.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.calls.lock().unwrap().push(self.schema.name.clone());
        Ok(AgentToolResult::text(self.result.clone()))
    }
}

/// A faux provider that also records the tool names it was asked to declare, per
/// request. That list is the only observable proof of what the model can call.
struct RecordingProvider {
    models: Vec<Model>,
    inner: Arc<FauxProvider>,
    declared: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl Provider for RecordingProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    async fn stream_simple(
        &self,
        model: &Model,
        ctx: &Context,
        opts: &SimpleStreamOptions,
    ) -> tom_ai::event_stream::AssistantMessageEventStream {
        self.declared.lock().unwrap().push(
            ctx.tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
        );
        self.inner.stream_simple(model, ctx, opts).await
    }
}

fn tool_use(id: &str, name: &str, arguments: serde_json::Value) -> FauxStep {
    let mut message = faux_assistant_message("", StopReason::ToolUse);
    message.content = vec![Content::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
        namespace: None,
        kind: Default::default(),
        recorded: None,
    })];
    FauxStep::message(message)
}

struct Built {
    harness: AgentHarness,
    declared: Arc<Mutex<Vec<Vec<String>>>>,
    calls: Arc<Mutex<Vec<String>>>,
    provider: Arc<FauxProvider>,
}

/// Build a harness whose registry holds two "MCP" tools. With `deferred`, only
/// the loader is active; otherwise both tools are declared as well.
async fn build(steps: Vec<FauxStep>, deferred: bool) -> Built {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let script = FauxScript::new();
    script.set_responses(steps);
    let provider = FauxProvider::new(script);
    let model = provider.default_model().clone();
    let declared = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(RecordingProvider {
        models: provider.models().to_vec(),
        inner: Arc::clone(&provider),
        declared: Arc::clone(&declared),
    });

    let read_file = HarnessTool::new(named_tool(
        "mcp__fs__read_file",
        "Read a file from the filesystem.",
        json!({"type":"object","properties":{"path":{"type":"string","description":"file to read"}}}),
        Arc::clone(&calls),
        "file contents",
    ));
    let list_dir = HarnessTool::new(named_tool(
        "mcp__fs__list_dir",
        "List a directory.",
        json!({"type":"object","properties":{}}),
        Arc::clone(&calls),
        "entries",
    ));
    let cell = ToolSearchToolCell::new();
    let search = HarnessTool::new(Arc::clone(&cell) as Arc<dyn AgentTool>);

    // `tool_search` is always active; the MCP tools only when not deferred.
    let mut active = vec![TOOL_SEARCH_TOOL_NAME.to_string()];
    if !deferred {
        active.push("mcp__fs__read_file".to_string());
        active.push("mcp__fs__list_dir".to_string());
    }

    let storage = Arc::new(InMemorySessionStorage::new(
        SessionMetadata {
            id: "tool-search".into(),
            created_at: 0,
            parent_session_id: None,
        },
        Arc::new(SystemClock),
        Arc::new(DefaultIdGenerator::new()),
    ));
    let harness = AgentHarness::create(AgentHarnessOptions {
        model,
        active_tool_names: active,
        tools: vec![read_file, list_dir, search],
        session: Session::new(storage, None),
        models: vec![recording as Arc<dyn Provider>],
        ..Default::default()
    })
    .await
    .expect("create harness");
    cell.set_harness(&harness);

    Built {
        harness,
        declared,
        calls,
        provider,
    }
}

/// Declared tool names, sorted, so a test asserts the *set* the model can call
/// without depending on declaration order.
fn sorted(names: &[String]) -> Vec<&str> {
    let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
    names.sort_unstable();
    names
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage::new(UserContent::Text(text.into()), 0))
}

/// The text of the first tool result in the session, for asserting what the model
/// was told.
async fn first_tool_result_text(harness: &AgentHarness) -> String {
    let entries = harness
        .session()
        .find_entries(&EntryQuery {
            entry_type: Some("message"),
            order: Some(EntryOrder::OldestFirst),
            ..Default::default()
        })
        .await
        .unwrap();
    entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Message(message) => match &message.message {
                AgentMessage::ToolResult(result) => Some(
                    result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("a tool result")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_tool_is_undeclared_until_tool_search_loads_it() {
    // Turn 1: the model searches. Turn 2: it calls the tool it just loaded.
    let built = build(
        vec![
            tool_use(
                "search-1",
                TOOL_SEARCH_TOOL_NAME,
                json!({"query": "read file"}),
            ),
            tool_use("read-1", "mcp__fs__read_file", json!({"path": "/tmp/x"})),
            FauxStep::text("done"),
        ],
        true,
    )
    .await;

    assert_eq!(
        built
            .harness
            .deferred_tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp__fs__read_file", "mcp__fs__list_dir"],
        "both MCP tools start deferred"
    );

    let result = built
        .harness
        .prompt_message(user_message("read a file for me"))
        .await
        .expect("run completes");
    assert!(matches!(
        result.outcome,
        HarnessRunOutcome::Completed { .. }
    ));

    let declared = built.declared.lock().unwrap().clone();
    assert_eq!(declared.len(), 3, "three provider calls: {declared:?}");
    // Before the load the model sees only the loader.
    assert_eq!(declared[0], vec![TOOL_SEARCH_TOOL_NAME.to_string()]);
    // The load added exactly the match; the unrelated tool stays deferred.
    assert_eq!(
        sorted(&declared[1]),
        vec!["mcp__fs__read_file", TOOL_SEARCH_TOOL_NAME]
    );
    assert_eq!(sorted(&declared[1]), sorted(&declared[2]));

    // The loaded tool actually ran, and only the undeclared one remains.
    assert_eq!(*built.calls.lock().unwrap(), vec!["mcp__fs__read_file"]);
    assert_eq!(
        built
            .harness
            .deferred_tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp__fs__list_dir"]
    );
    assert_eq!(
        built
            .provider
            .state()
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_is_recorded_and_survives_a_restored_session() {
    let built = build(
        vec![
            tool_use(
                "search-1",
                TOOL_SEARCH_TOOL_NAME,
                json!({"query": "list directory", "limit": 1}),
            ),
            FauxStep::text("loaded"),
        ],
        true,
    )
    .await;
    built
        .harness
        .prompt_message(user_message("find a directory tool"))
        .await
        .expect("run completes");

    // The load is a durable `active_tools_change` entry, not just memory.
    let entries = built
        .harness
        .session()
        .find_entries(&EntryQuery {
            entry_type: Some("active_tools_change"),
            order: Some(EntryOrder::OldestFirst),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "one load, one entry");
    let names = match &entries[0] {
        Entry::ActiveTools(entry) => entry.active_tool_names.clone(),
        other => panic!("unexpected entry {other:?}"),
    };
    assert_eq!(
        names,
        vec![
            TOOL_SEARCH_TOOL_NAME.to_string(),
            "mcp__fs__list_dir".to_string()
        ]
    );

    // A harness built on the same session (the `--continue` path) starts from the
    // recorded set, so the loaded tool is declared without a second search.
    let script = FauxScript::new();
    script.set_responses(vec![FauxStep::text("ok")]);
    let provider = FauxProvider::new(script);
    let declared = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(RecordingProvider {
        models: provider.models().to_vec(),
        inner: Arc::clone(&provider),
        declared: Arc::clone(&declared),
    });
    let calls = Arc::new(Mutex::new(Vec::new()));
    let cell = ToolSearchToolCell::new();
    let restored = AgentHarness::create(AgentHarnessOptions {
        model: provider.default_model().clone(),
        active_tool_names: names,
        tools: vec![
            HarnessTool::new(named_tool(
                "mcp__fs__read_file",
                "Read a file from the filesystem.",
                json!({"type":"object","properties":{}}),
                Arc::clone(&calls),
                "x",
            )),
            HarnessTool::new(named_tool(
                "mcp__fs__list_dir",
                "List a directory.",
                json!({"type":"object","properties":{}}),
                Arc::clone(&calls),
                "y",
            )),
            HarnessTool::new(Arc::clone(&cell) as Arc<dyn AgentTool>),
        ],
        session: built.harness.session().clone(),
        allow_existing_session: true,
        models: vec![recording as Arc<dyn Provider>],
        ..Default::default()
    })
    .await
    .expect("restore harness");
    cell.set_harness(&restored);
    restored
        .prompt_message(user_message("continue"))
        .await
        .expect("run completes");
    assert_eq!(
        sorted(&declared.lock().unwrap()[0]),
        vec!["mcp__fs__list_dir", TOOL_SEARCH_TOOL_NAME],
        "the loaded tool stays declared on the branch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_deferred_leaves_the_declared_set_alone() {
    let built = build(vec![FauxStep::text("ok")], false).await;
    assert!(built.harness.deferred_tools().is_empty());
    built
        .harness
        .prompt_message(user_message("hello"))
        .await
        .expect("run completes");
    assert_eq!(
        sorted(&built.declared.lock().unwrap()[0]),
        vec![
            "mcp__fs__list_dir",
            "mcp__fs__read_file",
            TOOL_SEARCH_TOOL_NAME
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_with_no_match_loads_nothing_and_records_nothing() {
    let built = build(
        vec![
            tool_use(
                "search-1",
                TOOL_SEARCH_TOOL_NAME,
                json!({"query": "kubernetes cluster"}),
            ),
            FauxStep::text("nothing found"),
        ],
        true,
    )
    .await;
    built
        .harness
        .prompt_message(user_message("find a k8s tool"))
        .await
        .expect("run completes");

    let entries = built
        .harness
        .session()
        .find_entries(&EntryQuery {
            entry_type: Some("active_tools_change"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(entries.is_empty());
    let declared = built.declared.lock().unwrap().clone();
    assert_eq!(sorted(&declared[0]), sorted(&declared[1]));
    assert!(first_tool_result_text(&built.harness)
        .await
        .contains("No matching tools found."));
}

#[test]
fn the_tool_description_is_stable_and_does_not_enumerate_tools() {
    // A description that changed as servers connected would invalidate the prompt
    // cache on every connect. Ours must not name deferred tools.
    let cell = ToolSearchToolCell::new();
    assert_eq!(cell.schema().name, TOOL_SEARCH_TOOL_NAME);
    assert!(!cell.schema().description.contains("mcp__"));
    assert!(cell.schema().description.contains(TOOL_SEARCH_TOOL_NAME));
}

#[test]
fn fallback_candidates_are_searchable_without_a_harness() {
    // Embedded callers bind no harness; the tool still works off the fallback.
    let cell = ToolSearchToolCell::new();
    cell.set_fallback_candidates(vec![DeferredTool::new("deploy", "Deploy the service.")]);
    assert_eq!(cell.deferred_tools().len(), 1);
    assert_eq!(cell.deferred_tools()[0].name, "deploy");
}
