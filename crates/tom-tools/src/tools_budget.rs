//! Tool declaration budgeting: what tom can do about "the tool schemas are the
//! same every turn".
//!
//! ## Why this is not a `world-state diff`
//!
//! Codex sends `added` / `removed` / `schema_changed` deltas because its
//! transport keeps server-side state: the previous response is addressable, so
//! the next request can legitimately omit what is unchanged. tom deliberately
//! does **not** work that way — every request is a stateless replay of the
//! provider-neutral transcript (`store: false` on the Responses API, full
//! `messages` on Anthropic). Under that contract, omitting an unchanged tool
//! declaration does not save a token: the model would simply not see the tool.
//!
//! The per-turn cost of unvarying declarations is therefore attacked where it
//! can actually be attacked — the prompt cache. Anthropic only places a cache
//! breakpoint on the **last** immediate tool, and cache hits are prefix-based,
//! so *deleting* an unchanged schema from the middle of the list would change
//! the prefix and destroy the very cache that makes these tokens cheap. This is
//! also why tool-list order is kept stable (registry order, filtered in place).
//!
//! What remains is the one lever that does reduce the prefix: the declarations
//! themselves. A namespace with twenty tools whose descriptions are each a
//! paragraph costs a fixed few thousand tokens *before* the model reads
//! anything. [`apply_tools_budget`] trims descriptions — keeping the name and
//! the parameter schema intact, because those are what the model needs to call
//! the tool correctly — and leaves a visible notice rather than silently
//! degrading a tool.
//!
//! [`ToolFingerprint`] exists so a host can tell *whether* the declaration set
//! changed at all: unchanged fingerprints across turns mean the input side of
//! the request is byte-identical, which is the condition under which the
//! provider's cache can hit. That is the diagnostic the diff would have given.

use std::collections::BTreeMap;

use tom_ai::types::Tool;

/// Tokens a description is trimmed to when a namespace exceeds its budget.
///
/// Enough for one sentence about what the tool does, which is all a model needs
/// to choose it; the parameter schema carries the details.
pub const TRIMMED_DESCRIPTION_TOKENS: usize = 40;

/// Rough bytes-per-token, matching the rest of the harness.
const BYTES_PER_TOKEN: usize = 4;

/// What one tool declaration costs, in the units the harness reasons about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCost {
    pub name_bytes: usize,
    pub description_bytes: usize,
    pub schema_bytes: usize,
}

impl ToolCost {
    pub fn total_bytes(self) -> usize {
        self.name_bytes + self.description_bytes + self.schema_bytes
    }

    /// Approximate tokens, using the harness-wide `chars / 4` estimate.
    pub fn tokens(self) -> usize {
        self.total_bytes().div_ceil(BYTES_PER_TOKEN)
    }
}

/// Measure one declaration.
pub fn tool_cost(tool: &Tool) -> ToolCost {
    ToolCost {
        name_bytes: tool.name.len(),
        description_bytes: tool.description.len(),
        schema_bytes: serde_json::to_string(&tool.parameters)
            .map(|s| s.len())
            .unwrap_or(0),
    }
}

/// Total approximate tokens of a declaration set. This is the fixed cost paid
/// on every request before the model sees a single message.
pub fn tools_tokens(tools: &[Tool]) -> usize {
    tools.iter().map(|tool| tool_cost(tool).tokens()).sum()
}

/// Trim tool descriptions so the declaration set fits `budget_tokens`.
///
/// Returns the tools actually sent and the number of descriptions trimmed.
/// Order is preserved — a reordered tool list would change the provider's cache
/// prefix and cost more than the trim saves.
///
/// Nothing is dropped wholesale: a tool without its description is a tool the
/// model cannot choose, which trades one problem for a worse one. Descriptions
/// are shortened longest-first (the biggest descriptions are the biggest win)
/// until the set fits or nothing is left to trim.
pub fn apply_tools_budget(tools: &[Tool], budget_tokens: usize) -> (Vec<Tool>, usize) {
    if budget_tokens == 0 || tools_tokens(tools) <= budget_tokens {
        return (tools.to_vec(), 0);
    }

    let mut trimmed = tools.to_vec();
    // Longest description first: trimming a 4000-byte description saves a
    // thousand tokens, trimming a 40-byte one saves nothing.
    let mut order: Vec<usize> = (0..trimmed.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(trimmed[i].description.len()));

    let mut trimmed_count = 0;
    for index in order {
        if tools_tokens(&trimmed) <= budget_tokens {
            break;
        }
        let shortened = shorten_description(&trimmed[index].description, TRIMMED_DESCRIPTION_TOKENS);
        if shortened.len() < trimmed[index].description.len() {
            trimmed[index].description = shortened;
            trimmed_count += 1;
        }
    }
    (trimmed, trimmed_count)
}

/// Cut a description to at most `tokens` tokens on a word boundary, leaving a
/// marker that tells the model the text was cut rather than ending oddly.
fn shorten_description(description: &str, tokens: usize) -> String {
    let max_bytes = tokens.saturating_mul(BYTES_PER_TOKEN);
    if description.len() <= max_bytes {
        return description.to_string();
    }
    // Prefer the first sentence when it fits: a truncated mid-clause sentence
    // reads like a mistake, a complete one does not.
    let first_sentence = description
        .find(". ")
        .map(|index| &description[..index + 1])
        .unwrap_or(description);
    if first_sentence.len() <= max_bytes {
        return first_sentence.to_string();
    }
    let mut cut = max_bytes;
    while cut > 0 && !description.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &description[..cut];
    let head = match head.rfind(' ') {
        Some(space) if space > cut / 2 => &head[..space],
        _ => head,
    };
    format!("{} …", head.trim_end())
}

/// The declared tool set, reduced to what a cache check needs.
///
/// Two turns with equal fingerprints sent byte-identical declarations, so any
/// prefix the provider cached still matches. A differing fingerprint names the
/// tool that changed, which is the useful half of a declaration diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolFingerprint {
    /// Stable hash per tool name. `BTreeMap` so iteration order never varies.
    entries: BTreeMap<String, u64>,
}

impl ToolFingerprint {
    /// Fingerprint a declaration set.
    pub fn of(tools: &[Tool]) -> Self {
        let mut entries = BTreeMap::new();
        for tool in tools {
            let mut hash = Fnv1a::new();
            hash.write(tool.name.as_bytes());
            hash.write(&[0]);
            hash.write(tool.description.as_bytes());
            hash.write(&[0]);
            match serde_json::to_string(&tool.parameters) {
                Ok(schema) => hash.write(schema.as_bytes()),
                Err(_) => hash.write(b"<unserializable>"),
            }
            entries.insert(tool.name.clone(), hash.finish());
        }
        Self { entries }
    }

    /// Names added, removed, or whose schema/description changed since `self`.
    ///
    /// This is the diff a host can log or show ("the tool set changed because
    /// `mcp__fs__read` appeared"), not a wire optimisation: see the module docs
    /// for why omitting unchanged declarations would not save tokens here.
    pub fn changes_since(&self, previous: &ToolFingerprint) -> ToolChanges {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();
        for (name, hash) in &self.entries {
            match previous.entries.get(name) {
                None => added.push(name.clone()),
                Some(old) if old != hash => changed.push(name.clone()),
                Some(_) => {}
            }
        }
        for name in previous.entries.keys() {
            if !self.entries.contains_key(name) {
                removed.push(name.clone());
            }
        }
        ToolChanges {
            added,
            removed,
            changed,
        }
    }
}

/// A declaration diff, in the shape a log line or a status line wants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl ToolChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

impl std::fmt::Display for ToolChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("tool declarations unchanged");
        }
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("added [{}]", self.added.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed [{}]", self.removed.join(", ")));
        }
        if !self.changed.is_empty() {
            parts.push(format!("changed [{}]", self.changed.join(", ")));
        }
        f.write_str(&parts.join("; "))
    }
}

/// FNV-1a. A stable, dependency-free hash: the fingerprint only has to detect
/// change, and a cryptographic digest here would buy nothing.
struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tom_ai::types::Schema;

    fn tool(name: &str, description: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema::new(serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}}
            })),
            constrained_sampling: None,
        }
    }

    #[test]
    fn a_small_set_is_left_alone() {
        let tools = vec![tool("read", "Read a file.")];
        let (sent, trimmed) = apply_tools_budget(&tools, 100_000);
        assert_eq!(trimmed, 0);
        assert_eq!(sent, tools);
    }

    #[test]
    fn an_oversized_set_has_its_descriptions_trimmed_not_dropped() {
        let long = "word ".repeat(5000);
        let tools = vec![tool("read", &long), tool("write", &long)];
        let budget = 500;
        let (sent, trimmed) = apply_tools_budget(&tools, budget);

        assert!(trimmed > 0, "something should have been trimmed");
        // Every tool is still declared: a tool with no description is a tool
        // the model cannot choose between.
        assert_eq!(sent.len(), tools.len());
        assert_eq!(sent[0].name, "read");
        assert_eq!(sent[1].name, "write");
        // The parameter schema is what makes a call correct, so it survives.
        assert_eq!(sent[0].parameters, tools[0].parameters);
        assert!(sent[0].description.len() < tools[0].description.len());
        assert!(tools_tokens(&sent) <= tools_tokens(&tools));
    }

    #[test]
    fn trimming_keeps_the_first_sentence_when_it_fits() {
        // The description is long, but its first sentence is short: the trim
        // should stop at the sentence rather than cutting mid-clause.
        let description = format!(
            "Read a file from disk. {}",
            "It also follows symlinks and reports truncation in detail. ".repeat(200)
        );
        let tools = vec![tool("read", &description)];
        let (sent, trimmed) = apply_tools_budget(&tools, tools_tokens(&tools) / 2);
        assert!(trimmed > 0);
        assert_eq!(sent[0].description, "Read a file from disk.");
    }

    #[test]
    fn trimming_never_splits_a_multibyte_character() {
        let description = "读取文件内容。".repeat(200);
        let tools = vec![tool("read", &description)];
        let (sent, _) = apply_tools_budget(&tools, 10);
        // A byte split would have panicked above; assert we got valid text.
        assert!(std::str::from_utf8(sent[0].description.as_bytes()).is_ok());
    }

    #[test]
    fn identical_sets_fingerprint_equal() {
        let a = vec![tool("read", "Read a file."), tool("bash", "Run a command.")];
        let b = vec![tool("bash", "Run a command."), tool("read", "Read a file.")];
        // Order must not matter: the fingerprint is about content.
        assert_eq!(ToolFingerprint::of(&a), ToolFingerprint::of(&b));
        assert!(ToolFingerprint::of(&a)
            .changes_since(&ToolFingerprint::of(&b))
            .is_empty());
    }

    #[test]
    fn added_removed_and_changed_are_reported_separately() {
        let before = ToolFingerprint::of(&[
            tool("read", "Read a file."),
            tool("bash", "Run a command."),
        ]);
        let after = ToolFingerprint::of(&[
            tool("read", "Read a file, with a longer description."),
            tool("mcp__fs__stat", "Stat a path."),
        ]);
        let changes = after.changes_since(&before);
        assert_eq!(changes.added, vec!["mcp__fs__stat"]);
        assert_eq!(changes.removed, vec!["bash"]);
        assert_eq!(changes.changed, vec!["read"]);
        assert!(changes.to_string().contains("added [mcp__fs__stat]"));
    }

    #[test]
    fn cost_splits_by_component() {
        let cost = tool_cost(&tool("read", "Read."));
        assert_eq!(cost.name_bytes, 4);
        assert_eq!(cost.description_bytes, 5);
        assert!(cost.schema_bytes > 0);
        assert_eq!(cost.total_bytes(), 4 + 5 + cost.schema_bytes);
    }
}
