//! Built-in `docs` tool for looking up tom usage documentation.
//!
//! Pi ships its coding-agent documentation with the CLI and lets the model
//! reach it through the normal read/documentation flow. tom keeps the same
//! model-facing behavior with a small, read-only topic index. The selected
//! Markdown pages are embedded at compile time so an installed binary does
//! not depend on the source checkout being present.

use std::sync::Arc;

use async_trait::async_trait;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, ToolExecutionMode, ToolResultPartial};
use tom_ai::types::Tool;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const MAX_SEARCH_RESULTS: usize = 8;
const MAX_RESULT_CHARS: usize = 18_000;

struct DocPage {
    topic: &'static str,
    description: &'static str,
    content: &'static str,
    aliases: &'static [&'static str],
}

static DOCS: &[DocPage] = &[
    DocPage {
        topic: "authoring",
        description: "Rust cdylib 扩展开发最佳实践：统一 C ABI 入口、工具生命周期、事件处理器、资源发现、tom dev 开发、tom-extcore 复用边界与发布检查清单",
        content: include_str!("../embedded-docs/extension-authoring.md"),
        aliases: &["extension-authoring", "create-package", "create-extension"],
    },
    DocPage {
        topic: "guide",
        description: "完整使用手册：安装、模型、CLI、.tom 资源、Rust 扩展、SDK、排错和发布",
        content: include_str!("../embedded-docs/user-guide.md"),
        aliases: &["manual", "user-guide", "cli", "quickstart", "extension", "extensions"],
    },
    DocPage {
        topic: "packages",
        description: "资源包作者指南：从 npm / git 分发 skills、prompts、themes 与 extensions，清单格式、完整性校验、所有权标记与资源优先级",
        content: include_str!("../embedded-docs/package-authoring.md"),
        aliases: &[
            "package-authoring",
            "resource-package",
            "npm-package",
            "distribute",
            "package-add",
        ],
    },
    DocPage {
        topic: "codemode",
        description: "codemode：在 QuickJS 沙箱里写 JavaScript 调用工具。脚本环境（tools/text/exit/store/searchTools）、工具调用走同一条管线、沙箱边界与结果格式",
        content: include_str!("../embedded-docs/codemode.md"),
        aliases: &["code-mode", "script", "sandbox", "javascript", "batch"],
    },
    DocPage {
        topic: "overview",
        description: "tom installation, built-in tools, configuration, extensions, and release basics",
        content: include_str!("../embedded-docs/README.md"),
        aliases: &["readme", "getting-started", "start", "usage"],
    },
    DocPage {
        topic: "agent",
        description: "用 tom SDK 创建自己的 Agent 项目的默认结构与规范：目录布局、内嵌 Agent、cdylib 适配、工具单一登记点、prompts/skills、配置与开发循环",
        content: include_str!("../embedded-docs/agent-project.md"),
        aliases: &[
            "agent-project",
            "project-layout",
            "create-agent",
            "scaffold",
            "project-structure",
        ],
    },
    DocPage {
        topic: "architecture",
        description: "tom crate layering, agent loop, provider, harness, sessions, and extension boundaries",
        content: include_str!("../embedded-docs/architecture.md"),
        aliases: &["design", "crates", "sdk"],
    },
    DocPage {
        topic: "debugging",
        description: "Rust cdylib 扩展编写（统一 C ABI、工具生命周期、事件处理器）与 agent/扩展调试（事件日志、tom dev-local、常见失败定位）",
        content: include_str!("../embedded-docs/rust-debugging.md"),
        aliases: &["rust", "rust-debug", "extension-debug", "troubleshoot", "debug"],
    },
];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DocsInput {
    /// Topic to open. Omit it, or use `list`, to list available topics.
    #[serde(default)]
    pub topic: Option<String>,
    /// Optional case-insensitive text to find inside the selected topic.
    #[serde(default)]
    pub query: Option<String>,
}

pub struct DocsTool {
    schema: Tool,
}

impl DocsTool {
    fn new() -> Self {
        let params = schemars::schema_for!(DocsInput);
        Self {
            schema: Tool {
                name: "docs".to_string(),
                description: "Look up tom usage documentation. Omit topic (or use topic=list) to list topics; pass a topic such as guide, authoring, codemode, agent, debugging, architecture, or overview. Add query to find relevant sections. Use this before guessing tom commands, creating an agent project or extension, extension APIs, or .tom configuration.".to_string(),
                parameters: tom_ai::types::Schema::new(
                    serde_json::to_value(params).unwrap_or_default(),
                ),
                constrained_sampling: None,
            },
        }
    }
}

pub fn create_docs_tool() -> Arc<dyn AgentTool> {
    Arc::new(DocsTool::new())
}

#[async_trait]
impl AgentTool for DocsTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        "docs"
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: DocsInput = serde_json::from_value(params)
            .map_err(|error| AgentError::Validation(format!("docs input invalid: {error}")))?;
        if signal.is_cancelled() {
            return Err(AgentError::Tool("docs lookup cancelled".into()));
        }

        let topic = input.topic.as_deref().unwrap_or("list").trim();
        if topic.is_empty() || topic.eq_ignore_ascii_case("list") || topic == "*" {
            return Ok(AgentToolResult::text(format_catalog()));
        }

        let page = DOCS
            .iter()
            .find(|page| {
                page.topic.eq_ignore_ascii_case(topic)
                    || page
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(topic))
            })
            .ok_or_else(|| {
                AgentError::Tool(format!(
                    "Unknown tom docs topic `{topic}`. Available topics: {}",
                    DOCS.iter()
                        .map(|page| page.topic)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;

        let query = input
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let body = match query {
            Some(query) => search_page(page, query),
            None => truncate_result(page.content),
        };
        Ok(AgentToolResult::text(format!(
            "# tom docs: {}\n\n{}",
            page.topic, body
        )))
    }
}

fn format_catalog() -> String {
    let mut out = String::from("Available tom documentation topics:\n");
    for page in DOCS {
        out.push_str(&format!("- {}: {}\n", page.topic, page.description));
    }
    out.push_str("\nUse docs with {\"topic\": \"agent\"} or add {\"query\": \"AgentBuilder\"} for a focused lookup.");
    out
}

fn search_page(page: &DocPage, query: &str) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = page.content.lines().collect();
    let mut selected = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains(&needle) {
            continue;
        }
        let start = index.saturating_sub(2);
        let end = (index + 3).min(lines.len());
        for line_no in start..end {
            if !selected.contains(&line_no) {
                selected.push(line_no);
            }
        }
        if selected.len() >= MAX_SEARCH_RESULTS * 5 {
            break;
        }
    }
    if selected.is_empty() {
        return format!(
            "No matches for `{query}` in `{}`. Try docs with topic=list or a broader query.",
            page.topic
        );
    }
    let mut out = format!("Matches for `{query}` in `{}`:\n\n", page.topic);
    for line_no in selected {
        out.push_str(&format!("{:>5}: {}\n", line_no + 1, lines[line_no]));
        if out.len() >= MAX_RESULT_CHARS {
            out.push_str("\n[Result truncated; run another focused docs query.]\n");
            break;
        }
    }
    out
}

fn truncate_result(content: &str) -> String {
    if content.len() <= MAX_RESULT_CHARS {
        return content.to_string();
    }
    let mut end = MAX_RESULT_CHARS;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[Document truncated; use docs with a query for a focused section.]",
        &content[..end]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn execute(input: serde_json::Value) -> String {
        let tool = DocsTool::new();
        let result = tool
            .execute("test", input, CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap();
        match &result.content[0] {
            tom_agent::types::TextContentOrImage::Text(text) => text.text.clone(),
            _ => panic!("docs returned an image"),
        }
    }

    #[tokio::test]
    async fn lists_topics() {
        let output = execute(serde_json::json!({})).await;
        assert!(output.contains("overview"));
        assert!(output.contains("guide"));
        assert!(output.contains("authoring"));
        assert!(output.contains("- agent:"));
        assert!(output.contains("debugging"));
        assert!(output.contains("统一 C ABI"));
        assert!(!output.contains("ABI v3/v2"));
        assert!(!output.contains("- extensions:"));
        assert!(!output.contains("- compatibility:"));
    }

    #[tokio::test]
    async fn returns_rust_debugging_guide() {
        let output = execute(serde_json::json!({"topic": "debugging", "query": "tom dev"})).await;
        assert!(output.contains("tom dev"));
        assert!(output.contains("cdylib"));
    }

    #[tokio::test]
    async fn returns_runtime_diagnostics_guide() {
        let output = execute(serde_json::json!({
            "topic": "debugging",
            "query": "TOM_FRAME_DIAGNOSTICS"
        }))
        .await;
        assert!(output.contains("TOM_FRAME_DIAGNOSTICS"));
        assert!(output.contains("frame writer"));
    }

    #[tokio::test]
    async fn returns_runtime_diagnostics_from_overview() {
        let output = execute(serde_json::json!({
            "topic": "overview",
            "query": "TOM_AGENT_DIAGNOSTICS"
        }))
        .await;
        assert!(output.contains("TOM_AGENT_DIAGNOSTICS"));
        assert!(output.contains("latencies"));
    }

    #[tokio::test]
    async fn returns_complete_user_guide() {
        let output = execute(serde_json::json!({"topic": "guide", "query": "cdylib"})).await;
        assert!(output.contains("cdylib"));
        assert!(output.contains("Rust"));
    }

    #[tokio::test]
    async fn returns_extension_authoring_practices() {
        let output = execute(serde_json::json!({"topic": "authoring", "query": "tom dev"})).await;
        assert!(output.contains("tom dev"));

        // The cdylib build contract is a separate section from the `tom`
        // dev workflow text, so query for it directly rather than relying
        // on both terms co-occurring in one excerpt window.
        let cdylib = execute(serde_json::json!({"topic": "authoring", "query": "cdylib"})).await;
        assert!(cdylib.contains("cdylib"));
    }

    #[tokio::test]
    async fn returns_extcore_reuse_guidance() {
        let output = execute(serde_json::json!({
            "topic": "authoring",
            "query": "tom-extcore"
        }))
        .await;
        assert!(output.contains("tom-extcore"));
        assert!(output.contains("tom-plugin-sdk"));
    }

    #[tokio::test]
    async fn returns_agent_project_guide() {
        let output = execute(serde_json::json!({
            "topic": "agent",
            "query": "AgentBuilder"
        }))
        .await;
        assert!(output.contains("AgentBuilder"));
        assert!(output.contains("Matches for"));
    }

    #[tokio::test]
    async fn agent_aliases_resolve() {
        let output = execute(serde_json::json!({"topic": "create-agent"})).await;
        assert!(output.contains("tom Agent 项目结构创建指南"));
    }

    #[tokio::test]
    async fn package_and_extension_aliases_resolve() {
        // `extension` aliases stay on the guide; `packages` is now its own page
        // (resource packages), which is the more useful answer for that word.
        for topic in ["extension", "extensions", "EXTENSIONS"] {
            let output = execute(serde_json::json!({"topic": topic, "query": "packages"})).await;
            assert!(output.starts_with("# tom docs: guide"));
            assert!(output.contains("Matches for"));
        }
        for topic in ["packages", "PACKAGES", "package-authoring", "npm-package"] {
            let output = execute(serde_json::json!({"topic": topic})).await;
            assert!(
                output.starts_with("# tom docs: packages"),
                "topic {topic} resolved to: {}",
                output.lines().next().unwrap_or_default()
            );
            assert!(output.contains("tom-package.json"));
        }
    }

    #[tokio::test]
    async fn rejects_unknown_topic() {
        let error = DocsTool::new()
            .execute(
                "test",
                serde_json::json!({"topic": "missing"}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unknown tom docs topic"));
    }
}
