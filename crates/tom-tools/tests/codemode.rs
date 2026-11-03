//! The sandbox's contract: what a script can do, what it cannot, and what the
//! host sees when it stops.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use tom_tools::codemode::{
    run_codemode_script, CodemodeErrorKind, CodemodeGlobalSpec, CodemodeHost, CodemodeOutcome,
    CodemodeOutputItem, CodemodeSandboxOptions, CodemodeToolSpec,
};

/// A host whose `read` tool echoes its path and whose `slow` tool waits forever.
struct TestHost {
    tool_calls: AtomicUsize,
}

impl TestHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            tool_calls: AtomicUsize::new(0),
        })
    }
}

impl CodemodeHost for TestHost {
    fn call_tool(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match name.as_str() {
                "read" => {
                    let path = args
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Ok(Value::String(format!("contents of {path}")))
                }
                "fails" => Err("no such file".to_string()),
                "object" => Ok(json!({"count": args["n"].as_u64().unwrap_or(0) * 2})),
                "slow" => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(Value::Null)
                }
                other => Err(format!("unknown tool {other}")),
            }
        })
    }

    fn call_global(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async move {
            match name.as_str() {
                "searchTools" => Ok(json!([{"name": "read"}])),
                "describeTool" => Ok(Value::String(format!(
                    "described: {}",
                    args[0].as_str().unwrap_or_default()
                ))),
                other => Err(format!("unknown global {other}")),
            }
        })
    }
}

fn tools() -> Vec<CodemodeToolSpec> {
    ["read", "fails", "object", "slow"]
        .into_iter()
        .map(|name| CodemodeToolSpec {
            name: name.to_string(),
            js_name: name.to_string(),
            description: format!("{name} tool"),
        })
        .collect()
}

fn globals() -> Vec<CodemodeGlobalSpec> {
    vec![
        CodemodeGlobalSpec {
            name: "searchTools".into(),
            spread: true,
        },
        CodemodeGlobalSpec {
            name: "describeTool".into(),
            spread: true,
        },
    ]
}

async fn run(host: Arc<TestHost>, code: &str) -> CodemodeOutcome {
    run_with(host, code, CancellationToken::new(), None).await
}

async fn run_with(
    host: Arc<TestHost>,
    code: &str,
    signal: CancellationToken,
    timeout: Option<Duration>,
) -> CodemodeOutcome {
    run_codemode_script(
        CodemodeSandboxOptions {
            tools: tools(),
            globals: globals(),
            store: BTreeMap::new(),
            timeout,
            ..Default::default()
        },
        code.to_string(),
        host,
        signal,
    )
    .await
}

fn text_of(outcome: &CodemodeOutcome) -> String {
    outcome
        .output
        .iter()
        .filter_map(|item| match item {
            CodemodeOutputItem::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_script_can_return_a_value() {
    let outcome = run(TestHost::new(), "return 1 + 1").await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, Some(json!(2)));
    assert!(outcome.calls.is_empty());
}

#[tokio::test]
async fn text_and_console_become_output() {
    let outcome = run(
        TestHost::new(),
        "text('first');\nconsole.log('note', 2);\ntext({a: 1});",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(
        outcome.output,
        vec![
            CodemodeOutputItem::Text("first".into()),
            CodemodeOutputItem::Console("note 2".into()),
            CodemodeOutputItem::Text("{\"a\":1}".into()),
        ]
    );
}

#[tokio::test]
async fn a_script_may_shadow_a_global() {
    // The prelude must not depend on globals a script can rebind: `text` here is
    // the script's own variable, not the output helper.
    let outcome = run(
        TestHost::new(),
        "const text = await tools.read({ path: 'a.txt' });\nreturn text;",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, Some(json!("contents of a.txt")));
    assert!(
        outcome.output.is_empty(),
        "the shadowed global must not print: {:?}",
        outcome.output
    );
}

#[tokio::test]
async fn nested_calls_resolve_with_the_host_value() {
    let outcome = run(
        TestHost::new(),
        "const one = await tools.read({ path: 'a' });\nconst two = await tools.object({ n: 21 });\nreturn [one, two.count];",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, Some(json!(["contents of a", 42])));
    assert_eq!(outcome.calls.len(), 2);
    assert_eq!(outcome.calls[0].name, "read");
    assert!(outcome
        .calls
        .iter()
        .all(|call| call.status == tom_tools::codemode::CodemodeCallStatus::Ok));
}

#[tokio::test]
async fn promise_all_runs_calls_concurrently() {
    let host = TestHost::new();
    let outcome = run(
        host.clone(),
        "const paths = ['a', 'b', 'c', 'd'];\nreturn (await Promise.all(paths.map((path) => tools.read({ path })))).length;",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, Some(json!(4)));
    assert_eq!(host.tool_calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_failing_call_rejects_and_the_script_can_catch_it() {
    let outcome = run(
        TestHost::new(),
        "try { await tools.fails({}); return 'no'; } catch (error) { return 'caught: ' + error.message; }",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, Some(json!("caught: no such file")));
    assert_eq!(
        outcome.calls[0].status,
        tom_tools::codemode::CodemodeCallStatus::Error
    );
    assert_eq!(outcome.calls[0].error.as_deref(), Some("no such file"));
}

#[tokio::test]
async fn a_thrown_error_reports_the_script_line() {
    let outcome = run(TestHost::new(), "const x = 1;\nthrow new Error('boom');").await;
    assert!(!outcome.is_ok());
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, CodemodeErrorKind::Script);
    assert!(error.message.contains("boom"), "{}", error.message);
    assert!(
        error.message.contains(":3:") || error.message.contains(":2:"),
        "the message should name the script's line: {}",
        error.message
    );
}

#[tokio::test]
async fn a_syntax_error_does_not_panic() {
    let outcome = run(TestHost::new(), "function (").await;
    assert!(!outcome.is_ok());
    assert_eq!(outcome.error.unwrap().kind, CodemodeErrorKind::Script);
}

#[tokio::test]
async fn a_script_waiting_on_nothing_reports_a_stall() {
    let outcome = run(TestHost::new(), "await new Promise(() => {})").await;
    assert!(!outcome.is_ok());
    let message = outcome.error.unwrap().message;
    assert!(message.contains("never settle"), "{message}");
}

#[tokio::test]
async fn a_busy_loop_stops_at_the_deadline() {
    let started = std::time::Instant::now();
    let outcome = run_with(
        TestHost::new(),
        "while (true) {}",
        CancellationToken::new(),
        Some(Duration::from_millis(300)),
    )
    .await;
    assert_eq!(outcome.error.unwrap().kind, CodemodeErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the deadline must stop a spinning script, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn cancellation_stops_a_waiting_script() {
    let signal = CancellationToken::new();
    let cancel = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let started = std::time::Instant::now();
    let outcome = run_with(
        TestHost::new(),
        "await tools.slow({});",
        signal,
        Some(Duration::from_secs(60)),
    )
    .await;
    assert_eq!(outcome.error.unwrap().kind, CodemodeErrorKind::Aborted);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn the_sandbox_exposes_no_host_capabilities() {
    for source in [
        "return typeof require",
        "return typeof process",
        "return typeof fetch",
        "return typeof setTimeout",
        "return typeof WebAssembly",
        "return typeof XMLHttpRequest",
        "return typeof module",
    ] {
        let outcome = run(TestHost::new(), source).await;
        assert!(outcome.is_ok(), "{source}: {:?}", outcome.error);
        assert_eq!(
            outcome.value,
            Some(json!("undefined")),
            "{source} must not exist in the sandbox"
        );
    }
}

#[tokio::test]
async fn a_script_cannot_patch_a_built_in_to_lie_about_its_output() {
    let outcome = run(
        TestHost::new(),
        "try { JSON.stringify = () => 'pwned'; } catch (error) {}\nreturn JSON.stringify({a: 1});",
    )
    .await;
    assert_eq!(outcome.value, Some(json!("{\"a\":1}")));
}

#[tokio::test]
async fn discovery_globals_answer_from_the_host() {
    let outcome = run(
        TestHost::new(),
        "const found = await searchTools('read files');\nconst described = await describeTool('read');\nreturn [found, described];",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    let value = outcome.value.unwrap();
    assert_eq!(value[0][0]["name"], "read");
    assert_eq!(value[1], "described: read");
}

#[tokio::test]
async fn calling_a_tool_that_does_not_exist_names_the_close_match() {
    let outcome = run(TestHost::new(), "await tools.raed({});").await;
    let message = outcome.error.unwrap().message;
    assert!(message.contains("tools.raed does not exist"), "{message}");
    assert!(
        message.contains("Did you mean tools.read") || message.contains("Available: read"),
        "{message}"
    );
}

#[tokio::test]
async fn store_writes_are_reported() {
    let outcome = run(
        TestHost::new(),
        "store('kept', {a: 1});\nstore('gone', undefined);\nreturn load('kept');",
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(
        outcome.store_writes,
        vec![
            ("kept".to_string(), Some(json!({"a": 1}))),
            ("gone".to_string(), None),
        ]
    );
}

#[tokio::test]
async fn a_store_value_over_the_limit_is_a_script_error() {
    let outcome = run(TestHost::new(), "store('big', 'x'.repeat(300000));").await;
    let message = outcome.error.unwrap().message;
    assert!(message.contains("more than the limit"), "{message}");
}

#[tokio::test]
async fn exit_stops_the_script_and_keeps_earlier_output() {
    let outcome = run(TestHost::new(), "text('before');\nexit();\ntext('after');").await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(outcome.value, None);
    assert_eq!(text_of(&outcome), "before");
}

#[tokio::test]
async fn image_output_carries_the_detected_type() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==";
    let outcome = run(
        TestHost::new(),
        &format!("image('data:image/png;base64,{png}');"),
    )
    .await;
    assert!(outcome.is_ok(), "{:?}", outcome.error);
    assert_eq!(
        outcome.output,
        vec![CodemodeOutputItem::Image {
            data: png.to_string(),
            mime_type: "image/png".to_string(),
        }]
    );
}

#[tokio::test]
async fn a_remote_image_url_is_rejected() {
    let outcome = run(TestHost::new(), "image('https://example.com/a.png');").await;
    let message = outcome.error.unwrap().message;
    assert!(message.contains("remote image URLs"), "{message}");
}

#[tokio::test]
async fn the_memory_limit_stops_a_runaway_allocation() {
    let outcome = run_codemode_script(
        CodemodeSandboxOptions {
            tools: tools(),
            globals: globals(),
            store: BTreeMap::new(),
            timeout: Some(Duration::from_secs(30)),
            memory_limit: 4 * 1024 * 1024,
        },
        "const kept = [];\nwhile (true) { kept.push(new Array(1000).fill(1)); }".to_string(),
        TestHost::new(),
        CancellationToken::new(),
    )
    .await;
    assert!(!outcome.is_ok());
    let error = outcome.error.unwrap();
    assert_eq!(error.kind, CodemodeErrorKind::Script);
    assert!(
        error.message.contains("out of memory"),
        "an allocation over the heap limit must say so: {}",
        error.message
    );
}

// ---------------------------------------------------------------------------
// The tool's own behaviour: what the model reads back.
// ---------------------------------------------------------------------------

mod tool {
    use std::sync::Arc;

    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use tom_agent::agent_tool::AgentTool;
    use tom_tools::codemode::{
        CodemodeTool, CodemodeToolImpl, CodemodeToolOptions, DEFAULT_MAX_OUTPUT_TOKENS,
    };

    fn read_tool() -> CodemodeTool {
        CodemodeTool {
            name: "read".into(),
            description: "Read file contents".into(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "File to read"}},
                "required": ["path"]
            }),
            output_schema: None,
            direct: true,
        }
    }

    async fn run(code: &str) -> Result<String, String> {
        let tool = CodemodeToolImpl::new(vec![read_tool()]);
        match tool
            .execute(
                "call-1",
                json!({ "code": code }),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
        {
            Ok(result) => Ok(text(&result)),
            Err(error) => Err(error.to_string()),
        }
    }

    fn text(result: &tom_agent::types::AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                tom_agent::types::TextContentOrImage::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn the_result_starts_with_a_header_and_ends_with_the_output() {
        let result = run("text('hello')").await.expect("run");
        assert!(result.starts_with("Script completed\n"), "{result}");
        assert!(result.contains("Wall time"), "{result}");
        assert!(result.contains("hello"), "{result}");
    }

    #[tokio::test]
    async fn a_failed_script_is_an_error_result_with_its_partial_output() {
        // The failure has to reach the model as an error, or it will treat a
        // half-finished batch as done.
        let error = run("text('did the first thing');\nthrow new Error('boom');")
            .await
            .expect_err("a failed script is an error");
        assert!(error.contains("did the first thing"), "{error}");
        assert!(error.contains("boom"), "{error}");
        assert!(error.contains("Script error"), "{error}");
    }

    #[tokio::test]
    async fn several_text_items_are_numbered() {
        let result = run("text('one');\ntext('two');").await.expect("run");
        assert!(result.contains("==> text 1/2 <==\none"), "{result}");
        assert!(result.contains("==> text 2/2 <==\ntwo"), "{result}");
    }

    #[tokio::test]
    async fn a_returned_value_is_output_like_text() {
        let result = run("return { count: 3 };").await.expect("run");
        assert!(result.contains("{\"count\":3}"), "{result}");
    }

    #[tokio::test]
    async fn console_lines_are_collected_in_one_block() {
        let result = run("console.log('a');\nconsole.warn('b');\ntext('out');")
            .await
            .expect("run");
        assert!(
            result.contains("<console_output>\na\nb\n</console_output>"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn output_is_truncated_at_the_token_budget() {
        let tool = CodemodeToolImpl::with_options(CodemodeToolOptions::default());
        let _ = DEFAULT_MAX_OUTPUT_TOKENS;
        let result = tool
            .execute(
                "call-1",
                json!({"code": "// @options: {\"max_output_tokens\": 20}\ntext('x'.repeat(2000));"}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect("run");
        let text = text(&result);
        assert!(text.contains("truncated"), "{text}");
        assert!(
            text.chars().count() < 2000,
            "the budget must cut the text: {} chars",
            text.chars().count()
        );
    }

    #[tokio::test]
    async fn the_description_lists_the_live_tool_set_in_mode_only() {
        // Mode `only` is what lists a tool the model already has.
        let tool = CodemodeToolImpl::with_options(CodemodeToolOptions {
            tools: vec![read_tool()],
            mode: tom_tools::codemode::CodemodeMode::Only,
            ..Default::default()
        });
        let description = &tool.schema().description;
        assert!(description.contains("### `read`"), "{description}");
        assert!(description.contains("tools.read"), "{description}");
    }

    #[tokio::test]
    async fn mode_on_does_not_repeat_a_declared_tool() {
        let tool = CodemodeToolImpl::new(vec![read_tool()]);
        let description = &tool.schema().description;
        assert!(!description.contains("### `read`"), "{description}");
        // The environment is still described, so the model knows how to call one.
        assert!(description.contains("await tools.<name>"), "{description}");
        assert!(description.contains("ALL_TOOLS"), "{description}");
    }

    #[tokio::test]
    async fn an_empty_code_block_is_a_validation_error() {
        let error = run("   ").await.expect_err("empty source");
        assert!(error.contains("non-empty"), "{error}");
    }

    #[tokio::test]
    async fn calling_a_tool_that_is_not_registered_says_so() {
        let error = run("await tools.write({});")
            .await
            .expect_err("unknown tool");
        assert!(error.contains("does not exist"), "{error}");
    }

    #[tokio::test]
    async fn image_output_reaches_the_model_as_an_image_block() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==";
        let tool = CodemodeToolImpl::new(vec![read_tool()]);
        let result = tool
            .execute(
                "call-1",
                json!({"code": format!("image('data:image/png;base64,{png}');")}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect("run");
        assert!(
            result
                .content
                .iter()
                .any(|block| matches!(block, tom_agent::types::TextContentOrImage::Image(image) if image.mime_type == "image/png")),
            "{:?}",
            result.content
        );
    }

    #[tokio::test]
    async fn store_writes_reach_the_store() {
        use std::sync::Mutex;
        use tom_tools::codemode::CodemodeStore;

        struct RecordingStore {
            writes: Arc<Mutex<Vec<Value>>>,
        }
        impl CodemodeStore for RecordingStore {
            fn load(
                &self,
            ) -> futures::future::BoxFuture<
                'static,
                Result<std::collections::BTreeMap<String, Value>, String>,
            > {
                Box::pin(async { Ok(std::collections::BTreeMap::new()) })
            }
            fn append(
                &self,
                writes: Vec<(String, Option<Value>)>,
            ) -> futures::future::BoxFuture<'static, Result<(), String>> {
                let writes: Vec<Value> = writes
                    .into_iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect();
                self.writes.lock().unwrap().extend(writes);
                Box::pin(async { Ok(()) })
            }
        }

        let writes = Arc::new(Mutex::new(Vec::new()));
        let tool = CodemodeToolImpl::with_options(CodemodeToolOptions {
            tools: vec![read_tool()],
            store: Arc::new(RecordingStore {
                writes: Arc::clone(&writes),
            }),
            ..Default::default()
        });
        tool.execute(
            "call-1",
            json!({"code": "store('k', 1);"}),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .expect("run");
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            [json!({"key": "k", "value": 1})]
        );
    }
}
