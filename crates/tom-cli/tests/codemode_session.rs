//! `codemode` is built into a real session: registered, declared, bound to the
//! harness, and reachable from the tool policy.
//!
//! The unit tests cover the sandbox and the tool in isolation; this covers the
//! wiring, which is the part that silently rots — a tool that exists but is never
//! bound has no nested calls, and a tool that is bound but never declared costs
//! tokens for nothing.

use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use tom_agent::types::TextContentOrImage;
use tom_cli::args::Args;
use tom_cli::session::build;
use tom_harness::agent_harness::AgentLane;
use tom_tools::codemode::CODEMODE_TOOL_NAME;

fn args(tools: Option<Vec<String>>) -> Args {
    Args {
        no_session: true,
        no_context_files: true,
        no_skills: true,
        no_prompt_templates: true,
        no_extensions: true,
        dev_local_only: true,
        tools,
        ..Default::default()
    }
}

fn resolved() -> tom_cli::provider::ResolvedModel {
    tom_cli::provider::resolve(
        Some("anthropic"),
        Some(tom_cli::provider::DEFAULT_MODEL_ID),
        None,
        Some("local-test-key"),
        None,
    )
    .unwrap()
}

fn text_of(result: &tom_agent::types::AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextContentOrImage::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_session_registers_and_declares_codemode() {
    let cwd = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (harness, _events, _context) = build(&resolved(), &args(None), cwd.path(), true)
            .await
            .expect("build a session");

        // Registered and declared.
        let tools = harness.get_tools().await.expect("tools");
        let codemode = tools
            .iter()
            .find(|tool| tool.tool.schema().name == CODEMODE_TOOL_NAME)
            .expect("codemode is registered");
        let description = codemode.tool.schema().description.clone();
        assert!(
            description.contains("await tools.<name>"),
            "the description explains the script API: {description}"
        );
        // Every callable tool reaches the script even when the description does not
        // repeat a declaration the model already has (mode `on`).
        assert!(
            description.contains("ALL_TOOLS") && description.contains("searchTools"),
            "the description must say how to find unlisted tools: {description}"
        );
        let active = harness.get_active_tools().await.expect("active tools");
        assert!(
            active.iter().any(|name| name == CODEMODE_TOOL_NAME),
            "codemode must be declared: {active:?}"
        );

        // Bound: a script's nested call reaches the real `read` tool. The file is
        // written first, so the assertion is about the pipeline, not a stub.
        std::fs::write(cwd.path().join("note.txt"), "hello from the sandbox").unwrap();
        let result = codemode
            .tool
            .execute(
                "call-1",
                json!({"code": "const body = await tools.read({ path: 'note.txt' });\ntext(body);"}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect("a script that reads a file");
        let output = text_of(&result);
        assert!(output.contains("Script completed"), "{output}");
        assert!(
            output.contains("hello from the sandbox"),
            "the nested read's output must reach the script: {output}"
        );
    });
}

#[test]
fn the_tool_policy_can_exclude_codemode() {
    let cwd = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (harness, _events, _context) = build(
            &resolved(),
            &args(Some(vec!["read".into()])),
            cwd.path(),
            true,
        )
        .await
        .expect("build a session");
        let tools = harness.get_tools().await.expect("tools");
        assert!(
            !tools
                .iter()
                .any(|tool| tool.tool.schema().name == CODEMODE_TOOL_NAME),
            "an explicit --tools allowlist that omits codemode must not register it"
        );
    });
}
