//! `codemode` end-to-end: a script's nested tool calls run through the harness's
//! own pipeline, and `store()` survives on the branch.
//!
//! The three things that can silently break, and what is asserted here:
//!
//! 1. **The pipeline is shared.** A nested call must meet the same
//!    `before_tool_call` hook a model-issued call does; otherwise a permission
//!    gate is bypassed by asking a script instead of the model. The hook here
//!    blocks one tool, and the script must see the block.
//! 2. **Nested results do not enter the context.** Only the script's output is
//!    the tool result — that is the whole token-efficiency argument.
//! 3. **`store()` is durable.** A value written by one script is readable by the
//!    next, through a `codemode-store` entry on the branch, so `/tree`, resume and
//!    fork each see their own path's values.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::message::AgentMessage;
use tom_agent::types::{AgentToolResult, BeforeToolCallResult, ToolResultPartial};
use tom_ai::model::Model;
use tom_ai::provider::SimpleStreamOptions;
use tom_ai::providers::faux::{faux_assistant_message, FauxProvider, FauxScript, FauxStep};
use tom_ai::types::{Content, Schema, StopReason, Tool, ToolCall, UserContent, UserMessage};
use tom_ai::{Context, Provider};
use tom_harness::agent_harness::{AgentHarness, HarnessRunOutcome};
use tom_harness::codemode::{CodemodeBinding, SessionCodemodeStore};
use tom_harness::session::memory::{InMemorySessionStorage, SystemClock};
use tom_harness::session::types::{Entry, EntryOrder, EntryQuery, SessionMetadata};
use tom_harness::session::{DefaultIdGenerator, Session};
use tom_harness::types::{AgentHarnessOptions, HarnessTool};
use tom_tools::codemode::{CodemodeToolImpl, CodemodeToolOptions, CODEMODE_TOOL_NAME};

/// A tool that records its calls and answers with a fixed text.
struct NamedTool {
    schema: Tool,
    calls: Arc<Mutex<Vec<Value>>>,
    result: String,
}

fn named_tool(
    name: &str,
    description: &str,
    parameters: Value,
    calls: Arc<Mutex<Vec<Value>>>,
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
        params: Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.calls.lock().unwrap().push(params);
        Ok(AgentToolResult::text(self.result.clone()))
    }
}

/// A faux provider that also records the tool names it was asked to declare.
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

fn tool_use(id: &str, name: &str, arguments: Value) -> FauxStep {
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
    calls: Arc<Mutex<Vec<Value>>>,
    provider: Arc<FauxProvider>,
}

/// Build a harness with the built-in `codemode` tool plus two script-callable
/// tools. `deferred` keeps `deferred_tool` out of the declared set.
async fn build(steps: Vec<FauxStep>, before: Option<tom_agent::BeforeToolCall>) -> Built {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let script = FauxScript::new();
    script.set_responses(steps);
    let provider = FauxProvider::new(script);
    let model = provider.default_model().clone();
    let declared = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(RecordingProvider {
        models: provider.models().to_vec(),
        inner: Arc::clone(&provider),
        declared,
    });

    let read = HarnessTool::new(named_tool(
        "read",
        "Read file contents",
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        Arc::clone(&calls),
        "file contents",
    ));
    let bash = HarnessTool::new(named_tool(
        "bash",
        "Execute Bash commands",
        json!({"type":"object","properties":{"command":{"type":"string"}}}),
        Arc::clone(&calls),
        "exit 1",
    ));
    let codemode = CodemodeToolImpl::with_options(CodemodeToolOptions::default());
    let codemode_tool = HarnessTool::new(Arc::clone(&codemode) as Arc<dyn AgentTool>);

    let storage = Arc::new(InMemorySessionStorage::new(
        SessionMetadata {
            id: "codemode".into(),
            created_at: 0,
            parent_session_id: None,
        },
        Arc::new(SystemClock),
        Arc::new(DefaultIdGenerator::new()),
    ));
    let harness = AgentHarness::create(AgentHarnessOptions {
        model,
        active_tool_names: vec![
            CODEMODE_TOOL_NAME.to_string(),
            "read".to_string(),
            "bash".to_string(),
        ],
        tools: vec![read, bash, codemode_tool],
        session: Session::new(storage, None),
        models: vec![recording as Arc<dyn Provider>],
        before_tool_call: before,
        ..Default::default()
    })
    .await
    .expect("create harness");
    codemode.set_host(CodemodeBinding::new(&harness, Vec::new()));
    codemode.set_store(SessionCodemodeStore::new(&harness));
    Built {
        harness,
        calls,
        provider,
    }
}

fn prompt(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage::new(UserContent::Text(text.into()), 0))
}

fn final_text(outcome: HarnessRunOutcome) -> String {
    let message = match outcome {
        HarnessRunOutcome::Completed { final_message, .. } => final_message,
        HarnessRunOutcome::Aborted { final_message, .. } => final_message,
        other => panic!("unexpected outcome: {other:?}"),
    };
    message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn tool_results(harness: &AgentHarness) -> Vec<String> {
    harness
        .find_entries_on_branch(
            &EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            },
            &Default::default(),
        )
        .await
        .expect("entries")
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message(message) => match &message.message {
                AgentMessage::ToolResult(result) => Some(
                    result
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            Content::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_calls_tools_through_the_harness_pipeline() {
    let built = build(
        vec![
            tool_use(
                "call-1",
                CODEMODE_TOOL_NAME,
                json!({"code": "const a = await tools.read({ path: 'a.txt' });\nconst b = await tools.bash({ command: 'true' });\ntext(a + ' | ' + b);\nreturn 'done';"}),
            ),
            FauxStep::message(faux_assistant_message("finished", StopReason::Stop)),
        ],
        None,
    )
    .await;
    let outcome = built
        .harness
        .send_message(
            prompt("run a script"),
            tom_harness::types::SendMessageOptions {
                trigger_turn: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("run");
    let text = match outcome {
        tom_harness::agent_harness::SendMessageResult::Run(result) => final_text(result.outcome),
        other => panic!("expected a run, got {other:?}"),
    };
    assert_eq!(text, "finished");

    // Both tools really ran, with the arguments the script passed.
    let calls = built.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0], json!({"path": "a.txt"}));
    assert_eq!(calls[1], json!({"command": "true"}));

    // The script's output — not the nested results — is the tool result.
    let results = tool_results(&built.harness).await;
    let script_result = results.last().expect("a tool result");
    assert!(
        script_result.contains("Script completed"),
        "{script_result}"
    );
    assert!(
        script_result.contains("file contents | exit 1"),
        "{script_result}"
    );
    assert!(script_result.contains("done"), "{script_result}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_call_meets_the_same_before_tool_call_gate() {
    // A permission gate that blocks `bash` for the model must block it for a
    // script too: otherwise the gate is one `codemode` call away from useless.
    let before: tom_agent::BeforeToolCall = Arc::new(move |context, _signal| {
        let name = context.tool_call.name.clone();
        Box::pin(async move {
            (name == "bash").then(|| BeforeToolCallResult {
                block: true,
                reason: Some("bash is not allowed here".into()),
                terminate: false,
                args: None,
            })
        })
    });
    let built = build(
        vec![
            tool_use(
                "call-1",
                CODEMODE_TOOL_NAME,
                json!({"code": "try { await tools.bash({ command: 'rm -rf /' }); return 'ran'; } catch (error) { return 'blocked: ' + error.message; }"}),
            ),
            FauxStep::message(faux_assistant_message("finished", StopReason::Stop)),
        ],
        Some(before),
    )
    .await;
    built
        .harness
        .send_message(
            prompt("try to run bash"),
            tom_harness::types::SendMessageOptions {
                trigger_turn: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("run");

    // The tool never executed.
    assert!(built.calls.lock().unwrap().is_empty());
    let results = tool_results(&built.harness).await;
    let script_result = results.last().expect("a tool result");
    assert!(
        script_result.contains("blocked: bash is not allowed here"),
        "{script_result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_survives_between_scripts_on_the_branch() {
    let built = build(
        vec![
            tool_use(
                "call-1",
                CODEMODE_TOOL_NAME,
                json!({"code": "store('counter', 1);\nreturn 'stored';"}),
            ),
            tool_use(
                "call-2",
                CODEMODE_TOOL_NAME,
                json!({"code": "return load('counter') + 1;"}),
            ),
            FauxStep::message(faux_assistant_message("finished", StopReason::Stop)),
        ],
        None,
    )
    .await;
    built
        .harness
        .send_message(
            prompt("store then load"),
            tom_harness::types::SendMessageOptions {
                trigger_turn: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("run");

    let results = tool_results(&built.harness).await;
    assert!(results[0].contains("stored"), "{}", results[0]);
    // The second script read what the first wrote: `1 + 1`.
    assert!(results[1].contains("2"), "{}", results[1]);

    // The write is durable, as a custom entry on the branch.
    let entries = built
        .harness
        .find_entries_on_branch(
            &EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            },
            &Default::default(),
        )
        .await
        .expect("entries");
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            Entry::Custom(custom) if custom.custom_type == tom_tools::codemode::CODEMODE_STORE_ENTRY_TYPE
        )),
        "a store write must be recorded on the branch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_is_declared_with_its_script_api() {
    let built = build(
        vec![FauxStep::message(faux_assistant_message(
            "no script needed",
            StopReason::Stop,
        ))],
        None,
    )
    .await;
    built
        .harness
        .send_message(
            prompt("hello"),
            tom_harness::types::SendMessageOptions {
                trigger_turn: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("run");
    let _ = &built.provider;
    let description = built
        .harness
        .get_tools()
        .await
        .expect("tools")
        .into_iter()
        .find(|tool| tool.tool.schema().name == CODEMODE_TOOL_NAME)
        .map(|tool| tool.tool.schema().description.clone())
        .expect("codemode is registered");
    assert!(description.contains("await tools.<name>"), "{description}");
    assert!(description.contains("store(key, value)"), "{description}");
    // Mode `on` (the default) does not repeat tools the model already has: a
    // declared tool's declaration in the description would cost tokens to say
    // what the tool list already says.
    assert!(!description.contains("### `read`"), "{description}");
    assert!(!description.contains("### `bash`"), "{description}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_tool_is_listed_and_callable_from_a_script() {
    // The pairing that motivates the feature: an MCP server's tools are deferred
    // (the model does not get them), and `codemode` is how the model uses them.
    // Mode `on` therefore lists exactly these — tools the model cannot see.
    let calls = Arc::new(Mutex::new(Vec::new()));
    let script = FauxScript::new();
    script.set_responses(vec![
        tool_use(
            "call-1",
            CODEMODE_TOOL_NAME,
            json!({"code": "const found = await tools.mcp__docs__search({ q: 'hooks' });\nreturn found;"}),
        ),
        FauxStep::message(faux_assistant_message("finished", StopReason::Stop)),
    ]);
    let provider = FauxProvider::new(script);
    let model = provider.default_model().clone();
    let declared = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(RecordingProvider {
        models: provider.models().to_vec(),
        inner: Arc::clone(&provider),
        declared: Arc::clone(&declared),
    });
    let docs = HarnessTool::new(named_tool(
        "mcp__docs__search",
        "Search the product documentation.",
        json!({"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}),
        Arc::clone(&calls),
        "hooks are callbacks",
    ));
    let codemode = CodemodeToolImpl::with_options(CodemodeToolOptions::default());
    let storage = Arc::new(InMemorySessionStorage::new(
        SessionMetadata {
            id: "codemode-deferred".into(),
            created_at: 0,
            parent_session_id: None,
        },
        Arc::new(SystemClock),
        Arc::new(DefaultIdGenerator::new()),
    ));
    let harness = AgentHarness::create(AgentHarnessOptions {
        model,
        // The MCP tool is registered but not declared, exactly like
        // `"exposure": "deferred"`.
        active_tool_names: vec![CODEMODE_TOOL_NAME.to_string()],
        tools: vec![
            docs,
            HarnessTool::new(Arc::clone(&codemode) as Arc<dyn AgentTool>),
        ],
        session: Session::new(storage, None),
        models: vec![recording as Arc<dyn Provider>],
        ..Default::default()
    })
    .await
    .expect("create harness");
    codemode.set_host(CodemodeBinding::new(&harness, Vec::new()));
    codemode.set_store(SessionCodemodeStore::new(&harness));

    // The description names the tool the model cannot otherwise see.
    let description = harness
        .get_tools()
        .await
        .expect("tools")
        .into_iter()
        .find(|tool| tool.tool.schema().name == CODEMODE_TOOL_NAME)
        .map(|tool| tool.tool.schema().description.clone())
        .expect("codemode");
    assert!(
        description.contains("### `mcp__docs__search`"),
        "{description}"
    );

    harness
        .send_message(
            prompt("search the docs"),
            tom_harness::types::SendMessageOptions {
                trigger_turn: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("run");

    // Deferred does not mean unavailable: the script reached it, and the model was
    // never asked to declare it.
    assert_eq!(calls.lock().unwrap().as_slice(), [json!({"q": "hooks"})]);
    assert!(
        declared
            .lock()
            .unwrap()
            .iter()
            .all(|names| !names.iter().any(|name| name == "mcp__docs__search")),
        "the deferred tool must stay out of the declared set"
    );
    let results = tool_results(&harness).await;
    assert!(
        results
            .last()
            .expect("a result")
            .contains("hooks are callbacks"),
        "{:?}",
        results.last()
    );
}
