//! Tool retrieval: a BM25 ranker over tool metadata plus the `tool_search` tool.
//!
//! Port of `packages/coding-agent/src/extensions/tool-search/tool.ts` (and the
//! `@earendil-works/pi-durable` copy of the same ranker). The point is the one
//! `todo.md` §4 names: with MCP connected, a session's tool count is no longer
//! "the eight built-ins", and declaring every tool on every request spends
//! context and lowers selection accuracy. A deferred tool is registered in the
//! harness but **not** in the active set, so it is never declared; `tool_search`
//! ranks the deferred tools, and the matches enter the active set — which the
//! harness records as an `active_tools_change` entry, so they stay declared on
//! that branch across `/tree`, resume and fork.
//!
//! Retrieval is lexical on purpose (pi's choice too): keyword/BM25 answers
//! "which tool does X" for tool names and descriptions, costs no model call, and
//! has no index to keep in sync as servers connect and drop tools.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, ToolResultPartial};
use tom_ai::types::{Schema, Tool};

/// The tool the model calls to load deferred tools.
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";

/// How many matches a call loads when the model does not pass `limit`.
pub const DEFAULT_TOOL_SEARCH_LIMIT: usize = 8;

/// Fixed description. It deliberately does not list the searchable tools, so it
/// stays identical while MCP servers connect and change their tool lists — a
/// description that changed per request would invalidate the prompt cache.
pub const TOOL_SEARCH_DESCRIPTION: &str = "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nSome of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`.";

const DEFAULT_K1: f64 = 1.2;
const DEFAULT_B: f64 = 0.75;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on",
    "or", "that", "the", "this", "to", "with",
];

/// A tool as the ranker sees it: its name and the text built by
/// [`create_tool_search_document`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchDocument {
    pub name: String,
    pub text: String,
}

/// One ranked match.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchMatch {
    pub name: String,
    pub score: f64,
}

/// Naive singular form, so `issues` matches `issue` and `searches` matches
/// `search`.
fn stem(term: &str) -> String {
    let len = term.chars().count();
    if len > 4 && term.ends_with("ies") {
        return format!("{}y", &term[..term.len() - 3]);
    }
    if len > 4
        && ["ches", "shes", "sses", "xes", "zes"]
            .iter()
            .any(|suffix| term.ends_with(suffix))
    {
        return term[..term.len() - 2].to_string();
    }
    if len > 3 && term.ends_with('s') && !term.ends_with("ss") {
        return term[..term.len() - 1].to_string();
    }
    term.to_string()
}

fn is_uppercase(c: char) -> bool {
    c.is_uppercase()
}

fn is_lowercase_or_digit(c: char) -> bool {
    c.is_lowercase() || c.is_ascii_digit()
}

/// Lowercase terms, split at camelCase boundaries and non-alphanumerics, with
/// stop words dropped and plurals folded.
pub fn tokenize(text: &str) -> Vec<String> {
    // First pass: `listIssues` → `list Issues`, and `HTTPServer` → `HTTP Server`
    // so a run of capitals followed by a lowercase word splits before its last
    // letter. Done in two passes over chars so multi-byte input is safe.
    let mut spaced = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let split_lower_upper = is_lowercase_or_digit(prev) && is_uppercase(c);
            let split_acronym =
                is_uppercase(prev) && is_uppercase(c) && next.is_some_and(|n| n.is_lowercase());
            if split_lower_upper || split_acronym {
                spaced.push(' ');
            }
        }
        spaced.push(c);
    }

    let lower = spaced.to_lowercase();
    let mut terms = Vec::new();
    for raw in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if raw.is_empty() || STOP_WORDS.contains(&raw) {
            continue;
        }
        terms.push(stem(raw));
    }
    terms
}

fn is_object(value: &serde_json::Value) -> bool {
    value.is_object()
}

/// Collect schema descriptions and property names, recursively.
fn schema_text(schema: &serde_json::Value, parts: &mut Vec<String>) {
    if !is_object(schema) {
        return;
    }
    if let Some(description) = schema.get("description").and_then(|v| v.as_str()) {
        parts.push(description.to_string());
    }
    if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
        for (name, property) in properties {
            parts.push(name.clone());
            schema_text(property, parts);
        }
    }
    if let Some(items) = schema.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = schema.get(key).and_then(|v| v.as_array()) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

/// The search text of a tool: the name, the name with `_` as spaces, the
/// description, and the parameter schema's descriptions and property names.
pub fn create_tool_search_document(
    name: &str,
    description: &str,
    parameters: &serde_json::Value,
) -> ToolSearchDocument {
    let mut parts = vec![
        name.to_string(),
        name.replace('_', " "),
        description.to_string(),
    ];
    schema_text(parameters, &mut parts);
    let text = parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    ToolSearchDocument {
        name: name.to_string(),
        text,
    }
}

/// Lowercase with characters invalid in an identifier replaced by `_`, so
/// `dev-radius` matches `dev_radius`.
fn normalize_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Whether `query` names the tool: its full name or a suffix after a `__`
/// separator, so `thread_read` and `app__thread_read` both name
/// `mcp__app__thread_read`.
fn is_name_query(query: &str, name: &str) -> bool {
    let normalized = normalize_name(name);
    normalized == query || normalized.ends_with(&format!("__{query}"))
}

/// Okapi BM25. A query that names a tool ranks that tool first, because length
/// normalization can otherwise rank a short description sharing the name's words
/// above it. Ties keep document order.
#[derive(Debug, Clone)]
pub struct Bm25Ranker {
    k1: f64,
    b: f64,
}

impl Default for Bm25Ranker {
    fn default() -> Self {
        Self {
            k1: DEFAULT_K1,
            b: DEFAULT_B,
        }
    }
}

impl Bm25Ranker {
    pub fn new(k1: f64, b: f64) -> Self {
        Self { k1, b }
    }

    pub fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch> {
        if limit == 0 || documents.is_empty() {
            return Vec::new();
        }
        let query_terms: BTreeSet<String> = tokenize(query).into_iter().collect();
        if query_terms.is_empty() {
            return Vec::new();
        }

        let term_counts: Vec<BTreeMap<String, usize>> = documents
            .iter()
            .map(|document| {
                let mut counts = BTreeMap::new();
                for term in tokenize(&document.text) {
                    *counts.entry(term).or_insert(0) += 1;
                }
                counts
            })
            .collect();
        let lengths: Vec<usize> = term_counts
            .iter()
            .map(|counts| counts.values().sum())
            .collect();
        let total: usize = lengths.iter().sum();
        let average_length = if total == 0 {
            1.0
        } else {
            total as f64 / documents.len() as f64
        };

        let idf: BTreeMap<&String, f64> = query_terms
            .iter()
            .map(|term| {
                let frequency = term_counts
                    .iter()
                    .filter(|counts| counts.contains_key(term))
                    .count() as f64;
                let n = documents.len() as f64;
                (term, (1.0 + (n - frequency + 0.5) / (frequency + 0.5)).ln())
            })
            .collect();

        let name_query = normalize_name(query);
        let mut matches: Vec<(ToolSearchMatch, bool)> = Vec::new();
        for (index, document) in documents.iter().enumerate() {
            let mut score = 0.0;
            for term in &query_terms {
                let Some(count) = term_counts[index].get(term).copied() else {
                    continue;
                };
                let norm =
                    self.k1 * (1.0 - self.b + (self.b * lengths[index] as f64) / average_length);
                let count = count as f64;
                score += idf.get(term).copied().unwrap_or(0.0)
                    * ((count * (self.k1 + 1.0)) / (count + norm));
            }
            if score > 0.0 {
                let named = is_name_query(&name_query, &document.name);
                matches.push((
                    ToolSearchMatch {
                        name: document.name.clone(),
                        score,
                    },
                    named,
                ));
            }
        }
        matches.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                b.0.score
                    .partial_cmp(&a.0.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        matches
            .into_iter()
            .take(limit)
            .map(|(matched, _)| matched)
            .collect()
    }
}

/// Input for the `tool_search` tool.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ToolSearchInput {
    /// Search query for deferred tools.
    pub query: String,
    /// Maximum number of tools to return. Defaults to 8.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Where `tool_search` gets its candidates: the tools that are registered but
/// not declared to the model. Called on every `execute`, so a tool that was
/// loaded by an earlier call is no longer offered.
pub trait DeferredToolSource: Send + Sync {
    fn deferred_tools(&self) -> Vec<DeferredTool>;
}

impl<F> DeferredToolSource for F
where
    F: Fn() -> Vec<DeferredTool> + Send + Sync,
{
    fn deferred_tools(&self) -> Vec<DeferredTool> {
        self()
    }
}

/// The `tool_search` tool. It only ranks and reports: what "loading" means is
/// the harness's business (it records the load as an active-tool change), so
/// this stays a pure tool fed by a [`DeferredToolSource`].
pub struct ToolSearchTool {
    schema: Tool,
    source: Arc<dyn DeferredToolSource>,
}

struct Candidate {
    document: ToolSearchDocument,
    description: String,
}

impl ToolSearchTool {
    fn schema() -> Tool {
        Tool {
            name: TOOL_SEARCH_TOOL_NAME.to_string(),
            description: TOOL_SEARCH_DESCRIPTION.to_string(),
            parameters: Schema::new(
                serde_json::to_value(schemars::schema_for!(ToolSearchInput)).unwrap_or_default(),
            ),
            constrained_sampling: None,
        }
    }
}

/// A tool that `tool_search` may load, as the host supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredTool {
    pub name: String,
    pub description: String,
    /// The model-facing JSON Schema, searched for property names and
    /// descriptions.
    pub parameters: serde_json::Value,
}

impl DeferredTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::Value::Null,
        }
    }

    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    fn into_candidate(self) -> Candidate {
        let document = create_tool_search_document(&self.name, &self.description, &self.parameters);
        Candidate {
            document,
            description: self.description,
        }
    }
}

/// Build the `tool_search` tool over a live view of the deferred tools.
pub fn create_tool_search_tool(source: Arc<dyn DeferredToolSource>) -> Arc<dyn AgentTool> {
    Arc::new(ToolSearchTool {
        schema: ToolSearchTool::schema(),
        source,
    })
}

/// Build the `tool_search` tool over a fixed candidate list, for callers that
/// have no registry to consult (tests, embedded single-run agents).
pub fn create_tool_search_tool_with(candidates: Vec<DeferredTool>) -> Arc<dyn AgentTool> {
    let source: Arc<dyn DeferredToolSource> = Arc::new(move || candidates.clone());
    create_tool_search_tool(source)
}

#[async_trait]
impl AgentTool for ToolSearchTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        TOOL_SEARCH_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: ToolSearchInput = serde_json::from_value(params)
            .map_err(|e| AgentError::Validation(format!("tool_search input invalid: {e}")))?;
        if input.query.trim().is_empty() {
            return Err(AgentError::Validation("query must not be empty".into()));
        }
        if matches!(input.limit, Some(0)) {
            return Err(AgentError::Validation(
                "limit must be a positive integer".into(),
            ));
        }
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }

        let limit = input.limit.unwrap_or(DEFAULT_TOOL_SEARCH_LIMIT);
        let candidates: Vec<Candidate> = self
            .source
            .deferred_tools()
            .into_iter()
            .map(DeferredTool::into_candidate)
            .collect();
        let documents: Vec<ToolSearchDocument> = candidates
            .iter()
            .map(|candidate| candidate.document.clone())
            .collect();
        let matches = Bm25Ranker::default().rank(&input.query, &documents, limit);

        let mut result = if matches.is_empty() {
            AgentToolResult::text("No matching tools found.")
        } else {
            let lines = matches
                .iter()
                .map(|matched| {
                    let description = candidates
                        .iter()
                        .find(|candidate| candidate.document.name == matched.name)
                        .map(|candidate| candidate.description.trim())
                        .unwrap_or_default();
                    let first_line = description.lines().next().unwrap_or_default();
                    format!("- {}: {first_line}", matched.name)
                })
                .collect::<Vec<_>>()
                .join("\n");
            AgentToolResult::text(format!(
                "Loaded {} tool{}. They are available from your next call:\n{lines}",
                matches.len(),
                if matches.len() == 1 { "" } else { "s" },
            ))
        };

        let loaded: Vec<String> = matches.into_iter().map(|matched| matched.name).collect();
        result.added_tool_names = loaded.clone();
        result.details = serde_json::json!({ "loaded": loaded });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(name: &str, description: &str, properties: serde_json::Value) -> ToolSearchDocument {
        create_tool_search_document(name, description, &properties)
    }

    /// The four tools the upstream test uses.
    fn github_tools() -> Vec<ToolSearchDocument> {
        vec![
            doc(
                "mcp__github__list_issues",
                "List issues in a repository.",
                json!({"type":"object","properties":{"state":{"type":"string","description":"open or closed"}}}),
            ),
            doc(
                "mcp__github__create_pull_request",
                "Open a pull request.",
                json!({"type":"object","properties":{}}),
            ),
            doc(
                "mcp__linear__search_issues",
                "Search Linear issues by text.",
                json!({"type":"object","properties":{}}),
            ),
            doc(
                "mcp__docs__search",
                "Search the documentation.",
                json!({"type":"object","properties":{}}),
            ),
        ]
    }

    #[test]
    fn tokenize_splits_camel_and_snake_case_drops_stop_words_and_folds_plurals() {
        assert_eq!(
            tokenize("listIssues for the GitHub_repo"),
            vec!["list", "issue", "git", "hub", "repo"]
        );
        assert_eq!(
            tokenize("searches queries HTTPServer"),
            vec!["search", "query", "http", "server"]
        );
    }

    #[test]
    fn ranks_by_term_relevance_and_respects_the_limit() {
        let documents = github_tools();
        let ranker = Bm25Ranker::default();
        let names = |matches: Vec<ToolSearchMatch>| {
            matches
                .into_iter()
                .map(|matched| matched.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(ranker.rank("issue", &documents, 8)),
            vec!["mcp__linear__search_issues", "mcp__github__list_issues"]
        );
        assert_eq!(
            names(ranker.rank("pull requests", &documents, 8))[0],
            "mcp__github__create_pull_request"
        );
        assert_eq!(ranker.rank("search", &documents, 1).len(), 1);
        // Property names and their descriptions are searchable.
        assert_eq!(
            names(ranker.rank("closed", &documents, 8)),
            vec!["mcp__github__list_issues"]
        );
    }

    #[test]
    fn returns_nothing_for_unknown_or_empty_queries() {
        let documents = github_tools();
        let ranker = Bm25Ranker::default();
        assert!(ranker.rank("kubernetes", &documents, 8).is_empty());
        assert!(ranker.rank("the", &documents, 8).is_empty());
        // Known v1 limit: no synonyms, so "tickets" does not find "issues".
        assert!(ranker.rank("tickets", &documents, 8).is_empty());
    }

    /// Regression from upstream (pi#11331): a query that *is* the tool name has
    /// to rank that tool first, not a shorter description that shares a word.
    #[test]
    fn ranks_a_tool_first_when_the_query_is_its_name() {
        let documents = vec![
            doc(
                "mcp__app__queue_read",
                "Read a queued message of this thread.",
                json!({"type":"object","properties":{}}),
            ),
            doc(
                "mcp__dev-app__thread_read",
                "Read durable state and a paginated timeline from any thread. The default view returns \
                 user messages, assistant messages and plans. Continue with afterPosition until hasMore \
                 is false.",
                json!({"type":"object","properties":{}}),
            ),
        ];
        let ranker = Bm25Ranker::default();
        for query in [
            "thread_read",
            "dev_app__thread_read",
            "mcp__dev-app__thread_read",
            "Thread_Read",
            "thread read",
        ] {
            let matches = ranker.rank(query, &documents, 1);
            assert_eq!(
                matches.first().map(|matched| matched.name.as_str()),
                Some("mcp__dev-app__thread_read"),
                "query {query:?}"
            );
        }
        // A partial name still ranks by BM25.
        assert_eq!(
            ranker.rank("read", &documents, 1)[0].name,
            "mcp__app__queue_read"
        );
    }

    #[test]
    fn ties_keep_document_order() {
        let documents = vec![
            doc("a_tool", "Same text here.", json!({})),
            doc("b_tool", "Same text here.", json!({})),
        ];
        let matches = Bm25Ranker::default().rank("same", &documents, 8);
        assert_eq!(matches[0].name, "a_tool");
        assert_eq!(matches[1].name, "b_tool");
    }

    #[tokio::test]
    async fn the_tool_reports_the_loaded_names_and_rejects_bad_input() {
        let tool = create_tool_search_tool_with(vec![
            DeferredTool::new("mcp__fs__read_file", "Read a file from disk.")
                .with_parameters(json!({"type":"object","properties":{"path":{"type":"string"}}})),
            DeferredTool::new("mcp__fs__list_dir", "List a directory."),
        ]);

        let result = tool
            .execute(
                "call",
                json!({"query": "read file"}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(result.added_tool_names, vec!["mcp__fs__read_file"]);
        let text = format!("{:?}", result.content);
        assert!(text.contains("Loaded 1 tool."), "{text}");

        let empty = tool
            .execute(
                "call",
                json!({"query": "kubernetes"}),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert!(empty.added_tool_names.is_empty());

        for bad in [json!({"query": "  "}), json!({"query": "x", "limit": 0})] {
            let error = tool
                .execute("call", bad, CancellationToken::new(), Arc::new(|_| {}))
                .await
                .unwrap_err();
            assert!(matches!(error, AgentError::Validation(_)));
        }
    }

    #[test]
    fn the_schema_is_stable_and_names_the_tool() {
        let schema = ToolSearchTool::schema();
        assert_eq!(schema.name, "tool_search");
        assert!(schema.description.contains("tool_search"));
        // The description must not enumerate tools, or it changes whenever an
        // MCP server connects and breaks the prompt cache.
        assert!(!schema.description.contains("mcp__"));
    }
}
