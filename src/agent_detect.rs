//! Screen-tail agent state detection.
//!
//! Coding-agent TUIs render their state into the terminal, so the bottom of the
//! scrollback is the most reliable place to tell "working", "blocked on a
//! question" and "idle at the prompt" apart. Rules are evaluated against the
//! bottom of the buffer (never the user-scrolled viewport), so detection keeps
//! working while the user reads history.
//!
//! The rule shape is ported from herdr's `src/detect/manifests` (Apache-2.0):
//! a rule has a state, a priority, a region of the screen, and a matcher tree
//! built from `contains` / `line_regex` / `regex` / `any` / `all` / `not`.
//! Only the agents whose signals we have verified are listed here; everything
//! else falls back to the process-name plus quiet-window heuristic in the
//! terminal list panel.

use std::sync::OnceLock;

use regex::Regex;

/// How many non-empty bottom lines are captured for rule evaluation.
pub const SCREEN_LINES: usize = 60;

/// Detected state of a terminal running a coding agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    Idle,
    /// Agent is actively working.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
        }
    }
}

/// A snapshot of the terminal bottom, as seen by the detector.
pub struct Screen {
    /// Last [`SCREEN_LINES`] non-empty lines of the buffer, top to bottom.
    pub lines: Vec<String>,
    /// The terminal (OSC) title.
    pub title: String,
}

/// Which part of the snapshot a rule is evaluated against.
#[derive(Clone, Copy, Debug)]
enum Region {
    /// The terminal title.
    Title,
    /// The last `n` non-empty lines.
    BottomNonEmpty(usize),
    /// Everything below the last horizontal rule, which is where agent CLIs
    /// draw their modal footers.
    AfterLastHorizontalRule,
}

/// Declarative matcher, kept in `static` rule tables. A single matcher is a
/// gate: `contains` is case-insensitive, every `regex` and `line_regex` must
/// match, every `all` child must match, at least one `any` child must match,
/// and no `not` child may match. This mirrors herdr's gate semantics.
enum Matcher {
    /// All substrings present in the region text (case-insensitive).
    Contains(&'static [&'static str]),
    /// The region text matches every pattern.
    Regex(&'static [&'static str]),
    /// Every pattern matches at least one line.
    LineRegex(&'static [&'static str]),
    Any(&'static [Matcher]),
    All(&'static [Matcher]),
    Not(&'static [Matcher]),
}

struct Rule {
    state: AgentState,
    priority: u32,
    region: Region,
    matcher: Matcher,
}

/// A rule with its regexes compiled once, sorted by descending priority.
struct CompiledRule {
    state: AgentState,
    priority: u32,
    region: Region,
    matcher: CompiledGate,
}

struct CompiledGate {
    /// Lowercased, because `contains` is case-insensitive.
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
    all: Vec<CompiledGate>,
    any: Vec<CompiledGate>,
    not: Vec<CompiledGate>,
}

impl CompiledGate {
    fn matches(&self, text: &str, lower_text: &str) -> bool {
        self.contains
            .iter()
            .all(|needle| lower_text.contains(needle))
            && self.regex.iter().all(|regex| regex.is_match(text))
            && self
                .line_regex
                .iter()
                .all(|regex| text.lines().any(|line| regex.is_match(line)))
            && self.all.iter().all(|gate| gate.matches(text, lower_text))
            && (self.any.is_empty() || self.any.iter().any(|gate| gate.matches(text, lower_text)))
            && !self.not.iter().any(|gate| gate.matches(text, lower_text))
    }
}

/// Rule set for an agent, or `None` when this module has no rules for it.
fn rules_for(agent_id: &str) -> Option<&'static [CompiledRule]> {
    Some(match agent_id {
        "claude" => claude_rules(),
        "codex" => codex_rules(),
        "gemini" => gemini_rules(),
        "opencode" => opencode_rules(),
        "cursor" => cursor_rules(),
        "qwen" => qwen_rules(),
        "copilot" => copilot_rules(),
        "pi" => pi_rules(),
        "grok" => grok_rules(),
        "droid" => droid_rules(),
        "kimi" => kimi_rules(),
        "amp" => amp_rules(),
        "hermes" => hermes_rules(),
        "qoder" | "qoder-cn" => qoder_rules(),
        "antigravity" => antigravity_rules(),
        "cline" => cline_rules(),
        "devin" => devin_rules(),
        "kiro" => kiro_rules(),
        "kilo" => kilo_rules(),
        "letta" => letta_rules(),
        "maki" => maki_rules(),
        "muse" => muse_rules(),
        _ => return None,
    })
}

/// Whether the terminal is running an agent this module has rules for.
pub fn supports(agent_id: &str) -> bool {
    rules_for(agent_id).is_some()
}

/// Detects the agent state from a screen snapshot.
///
/// Returns `None` when the agent has no rule set or no rule matched; callers
/// should then fall back to their own heuristics.
pub fn detect(agent_id: &str, screen: &Screen) -> Option<AgentState> {
    let rules = rules_for(agent_id)?;

    let lines: Vec<&str> = screen.lines.iter().map(String::as_str).collect();
    for rule in rules {
        let region = select_region(&lines, rule.region, &screen.title);
        let text = region.join("\n");
        let lower_text = text.to_lowercase();
        if rule.matcher.matches(&text, &lower_text) {
            return Some(rule.state);
        }
    }
    None
}

fn select_region<'a>(lines: &[&'a str], region: Region, title: &'a str) -> Vec<&'a str> {
    match region {
        Region::Title => vec![title],
        Region::BottomNonEmpty(count) => {
            let start = lines.len().saturating_sub(count);
            lines[start..].to_vec()
        }
        Region::AfterLastHorizontalRule => {
            let start = lines
                .iter()
                .rposition(|line| is_horizontal_rule(line))
                .map(|index| index + 1)
                .unwrap_or(0);
            lines[start..].to_vec()
        }
    }
}

/// Box-drawing rules that agent CLIs use to frame dialogs. Matches herdr's
/// definition: a line whose leading run of `─` is either the whole line or at
/// least three characters, so box borders such as `╰────╯` are not mistaken for
/// separators.
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let rule_chars = trimmed.chars().take_while(|&ch| ch == '─').count();
    if rule_chars == 0 {
        return false;
    }

    let rule_bytes = trimmed
        .char_indices()
        .nth(rule_chars)
        .map(|(index, _)| index)
        .unwrap_or(trimmed.len());
    let suffix = trimmed[rule_bytes..].trim_start();

    suffix.is_empty() || rule_chars >= 3
}

fn compile(rules: &'static [Rule]) -> Vec<CompiledRule> {
    let mut compiled: Vec<CompiledRule> = rules
        .iter()
        .map(|rule| CompiledRule {
            state: rule.state,
            priority: rule.priority,
            region: rule.region,
            matcher: compile_matcher(&rule.matcher),
        })
        .collect();
    compiled.sort_by_key(|rule| std::cmp::Reverse(rule.priority));
    compiled
}

fn compile_matcher(matcher: &Matcher) -> CompiledGate {
    let mut gate = CompiledGate {
        contains: Vec::new(),
        regex: Vec::new(),
        line_regex: Vec::new(),
        all: Vec::new(),
        any: Vec::new(),
        not: Vec::new(),
    };
    match matcher {
        Matcher::Contains(needles) => {
            gate.contains = needles.iter().map(|needle| needle.to_lowercase()).collect();
        }
        Matcher::Regex(patterns) => gate.regex = compile_patterns(patterns),
        Matcher::LineRegex(patterns) => gate.line_regex = compile_patterns(patterns),
        Matcher::All(matchers) => gate.all = matchers.iter().map(compile_matcher).collect(),
        Matcher::Any(matchers) => gate.any = matchers.iter().map(compile_matcher).collect(),
        Matcher::Not(matchers) => gate.not = matchers.iter().map(compile_matcher).collect(),
    }
    gate
}

fn compile_patterns(patterns: &'static [&'static str]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|pattern| match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(error) => {
                log::error!("agent detection: invalid regex {pattern:?}: {error}");
                None
            }
        })
        .collect()
}

fn claude_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(CLAUDE_RULES))
}

fn codex_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(CODEX_RULES))
}

/// Claude Code. Ported from herdr `manifests/claude.toml`; rules that only
/// affect herdr's own UI metadata are omitted.
static CLAUDE_RULES: &[Rule] = &[
    // Braille and half-circle spinners in the title.
    Rule {
        state: AgentState::Working,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::LineRegex(&[r"^[\x{2800}-\x{28FF}\x{25D0}-\x{25D3}] "]),
    },
    // A modal is open above the prompt box, e.g. a permission dialog.
    Rule {
        state: AgentState::Blocked,
        priority: 980,
        region: Region::AfterLastHorizontalRule,
        matcher: Matcher::All(&[
            Matcher::Contains(&["esc to cancel"]),
            Matcher::Any(&[
                Matcher::Contains(&["enter to confirm"]),
                Matcher::Contains(&["enter to select"]),
            ]),
        ]),
    },
    // A live turn keeps an interrupt hint on screen.
    Rule {
        state: AgentState::Working,
        priority: 970,
        region: Region::BottomNonEmpty(12),
        matcher: Matcher::Any(&[
            Matcher::LineRegex(&[r"^\s*[⏸⏵].*esc to interrupt(?:\s|·|$)"]),
            Matcher::LineRegex(&[
                r"^\s*[\x{002A}\x{00B7}\x{2722}\x{2733}\x{2736}\x{273B}\x{273D}]\s+\S.*…(?:\s+\(\d+[smh](?:\s|·)|\s*$)",
            ]),
        ]),
    },
    // The prompt box is visible and nothing is asking for an answer.
    Rule {
        state: AgentState::Idle,
        priority: 950,
        region: Region::BottomNonEmpty(6),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*❯"]),
            Matcher::Not(&[
                Matcher::Contains(&["enter to select"]),
                Matcher::Contains(&["esc to cancel"]),
                Matcher::Contains(&["tab/arrow keys"]),
                Matcher::Contains(&["arrow keys to navigate"]),
                Matcher::Contains(&["↑/↓ to navigate"]),
            ]),
        ]),
    },
    // Bash / tool approval dialog.
    Rule {
        state: AgentState::Blocked,
        priority: 850,
        region: Region::BottomNonEmpty(SCREEN_LINES),
        matcher: Matcher::All(&[
            Matcher::Contains(&["do you want to proceed?", "esc to cancel"]),
            Matcher::Any(&[
                Matcher::Contains(&["bash command"]),
                Matcher::Contains(&["bash("]),
                Matcher::Contains(&["tab to amend"]),
                Matcher::Contains(&["ctrl+e to explain"]),
            ]),
        ]),
    },
    // Numbered permission prompt.
    Rule {
        state: AgentState::Blocked,
        priority: 840,
        region: Region::AfterLastHorizontalRule,
        matcher: Matcher::All(&[
            Matcher::Contains(&["do you want to proceed?", "esc to cancel"]),
            Matcher::Any(&[
                Matcher::LineRegex(&[r"(?i)^\s*❯?\s*1\.\s*yes\b"]),
                Matcher::LineRegex(&[r"(?i)^\s*❯?\s*2\.\s*yes\b"]),
                Matcher::LineRegex(&[r"(?i)^\s*❯?\s*2\.\s*no\b"]),
                Matcher::LineRegex(&[r"(?i)^\s*❯?\s*3\.\s*no\b"]),
            ]),
        ]),
    },
    // Anything else that is clearly asking the user a question.
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: Region::BottomNonEmpty(SCREEN_LINES),
        matcher: Matcher::Any(&[
            Matcher::All(&[
                Matcher::Contains(&["do you want to"]),
                Matcher::Any(&[Matcher::Contains(&["yes"]), Matcher::Contains(&["❯"])]),
            ]),
            Matcher::All(&[
                Matcher::Contains(&["would you like to"]),
                Matcher::Any(&[Matcher::Contains(&["yes"]), Matcher::Contains(&["❯"])]),
            ]),
            Matcher::Contains(&["waiting for permission"]),
            Matcher::Contains(&["do you want to allow this connection?"]),
            Matcher::Contains(&["tab to amend"]),
            Matcher::Contains(&["ctrl+e to explain"]),
            Matcher::Contains(&["review your answers"]),
            Matcher::Contains(&["skip interview and plan immediately"]),
        ]),
    },
    // Idle marker in the title.
    Rule {
        state: AgentState::Idle,
        priority: 250,
        region: Region::Title,
        matcher: Matcher::LineRegex(&[r"^\x{2733} "]),
    },
];

/// Codex CLI. Ported from herdr `manifests/codex.toml`.
static CODEX_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::Contains(&["Action Required"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1050,
        region: Region::Title,
        matcher: Matcher::LineRegex(&[r"(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)"]),
    },
    // First-run trust prompt.
    Rule {
        state: AgentState::Blocked,
        priority: 950,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Regex(&[r"\A> You are in [^\r\n]+(?:\r?\n|$)"]),
                Matcher::Contains(&["Folder access"]),
            ]),
            Matcher::Any(&[
                Matcher::Regex(&[
                    r"(?s)Do\s+you\s+trust\s+the\s+contents\s+of\s+this\s+directory\?",
                ]),
                Matcher::All(&[
                    Matcher::Contains(&[
                        "Trust this folder?",
                        "Codex can read, edit, and run files here",
                    ]),
                    Matcher::Any(&[
                        Matcher::Contains(&["Trust and continue"]),
                        Matcher::Contains(&["enter continue"]),
                    ]),
                ]),
            ]),
        ]),
    },
    // Update prompt shown at startup.
    Rule {
        state: AgentState::Blocked,
        priority: 950,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::Any(&[
            Matcher::All(&[
                Matcher::Contains(&["Update available!", "Update now"]),
                Matcher::Regex(&[
                    r"Skip\s+until\s+next\s+version",
                    r"Press enter to continue\s*\z",
                ]),
            ]),
            Matcher::All(&[
                Matcher::Regex(&[r"(?m)^\s*Update\s+available\s+·"]),
                Matcher::Regex(&[r"enter\s+continue\s+·\s+esc\s+skip\s*\z"]),
            ]),
        ]),
    },
    // Live blockers that carry their own explicit footer.
    Rule {
        state: AgentState::Blocked,
        priority: 900,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["press enter to confirm or esc to cancel"]),
            Matcher::Contains(&["enter to submit answer"]),
            Matcher::Contains(&["enter to submit all"]),
            Matcher::Contains(&["allow command?"]),
        ]),
    },
    // Generic y/n prompts.
    Rule {
        state: AgentState::Blocked,
        priority: 600,
        region: Region::BottomNonEmpty(SCREEN_LINES),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["[y/n]"]),
            Matcher::Contains(&["yes (y)"]),
            Matcher::All(&[
                Matcher::Contains(&["do you want to"]),
                Matcher::Any(&[Matcher::Contains(&["yes"]), Matcher::Contains(&["❯"])]),
            ]),
            Matcher::All(&[
                Matcher::Contains(&["would you like to"]),
                Matcher::Any(&[Matcher::Contains(&["yes"]), Matcher::Contains(&["❯"])]),
            ]),
        ]),
    },
    // A live turn shows an elapsed-time status line.
    Rule {
        state: AgentState::Working,
        priority: 500,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Contains(&[" to interrupt)"]),
                Matcher::Contains(&["s)"]),
            ]),
            Matcher::Regex(&[
                r"(?m)^(?:[•◦][ \t]+)?[^\s›•◦■✗✓─][^\r\n]* \((?:[0-9]+[hm] )*[0-9]+s(?: • [^\r\n]+? to interrupt)?\)",
            ]),
            Matcher::Not(&[Matcher::LineRegex(&[
                r"^(?:[•◦][ \t]+)?Reconnect failed — check the endpoint, then relaunch \([0-9hms ]+\)$",
            ])]),
        ]),
    },
    // Anything else in the title is idle chrome.
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::Title,
        matcher: Matcher::All(&[
            Matcher::Regex(&[r"\S"]),
            Matcher::Not(&[
                Matcher::LineRegex(&[r"(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)"]),
                Matcher::Contains(&["Action Required"]),
            ]),
        ]),
    },
];

// -------------------------------------------------------------------------
// Ported rule sets
//
// Each set is a direct port of herdr's `src/detect/manifests/<agent>.toml`
// (Apache-2.0). Regions map as follows:
//
//   osc_title                  -> Region::Title
//   whole_recent               -> WHOLE_RECENT
//   bottom_non_empty_lines(n)  -> Region::BottomNonEmpty(n)
//   after_last_horizontal_rule -> Region::AfterLastHorizontalRule
//
// Rules that rely on `osc_progress` (raw OSC 9;4 payloads) or on prompt-box
// regions are omitted; upstream treats them as supplemental signals.
// -------------------------------------------------------------------------

/// Region matching herdr's `whole_recent`: the whole captured window.
const WHOLE_RECENT: Region = Region::BottomNonEmpty(SCREEN_LINES);

fn gemini_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(GEMINI_RULES))
}

static GEMINI_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["│ Apply this change"]),
            Matcher::Contains(&["│ Allow execution"]),
            Matcher::All(&[
                Matcher::Contains(&["yes"]),
                Matcher::Any(&[
                    Matcher::Contains(&["waiting for user confirmation"]),
                    Matcher::Contains(&["│ Do you want to proceed"]),
                    Matcher::Contains(&["do you want to proceed?"]),
                ]),
            ]),
            Matcher::LineRegex(&[r"(?i)^\s*❯.*(yes|allow)"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Contains(&["esc to cancel"]),
    },
];

fn opencode_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(OPENCODE_RULES))
}

static OPENCODE_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["△ Permission required"]),
            Matcher::All(&[
                Matcher::Contains(&["esc dismiss"]),
                Matcher::Any(&[
                    Matcher::Contains(&["enter confirm"]),
                    Matcher::Contains(&["enter submit"]),
                    Matcher::Contains(&["enter toggle"]),
                ]),
                Matcher::Any(&[
                    Matcher::Contains(&["↑↓ select"]),
                    Matcher::Contains(&["⇆ tab"]),
                ]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 110,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["esc to interrupt"]),
            Matcher::Contains(&["ctrl+c to interrupt"]),
            Matcher::Contains(&["press esc to interrupt"]),
            Matcher::LineRegex(&[r"(?i).*opencode.*esc (again to )?interrupt"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Regex(&[r"(■|⬝){4,}"]),
    },
];

fn cursor_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(CURSOR_RULES))
}

static CURSOR_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 320,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["write to this file?", "proceed (y)"]),
            Matcher::Any(&[
                Matcher::Contains(&["reject & propose changes"]),
                Matcher::Contains(&["esc or n or p"]),
                Matcher::Contains(&["add write("]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::All(&[
                Matcher::Contains(&["waiting for approval", "run this command?"]),
                Matcher::Any(&[
                    Matcher::Contains(&["run (once) (y)"]),
                    Matcher::Contains(&["skip (esc or n)"]),
                ]),
            ]),
            Matcher::Contains(&["(y) (enter)"]),
            Matcher::LineRegex(&[r"(?i)^\s*allow .*\(y\)"]),
            Matcher::Contains(&["keep (n)"]),
            Matcher::Contains(&["skip (esc or n)"]),
            Matcher::LineRegex(&[r"(?i)^\s*(?:→\s*)?run .*\(y\)"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: Region::BottomNonEmpty(6),
        matcher: Matcher::Contains(&["ctrl+c to stop"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 95,
        region: Region::BottomNonEmpty(5),
        matcher: Matcher::LineRegex(&[r"(?i)\b[1-9][0-9]*\s+background\s+tasks?\b"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 90,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::LineRegex(&[r"^\s*(⬡|⬢|[\x{2800}-\x{28FF}]+)\s+\p{Alphabetic}+\w*ing\b"]),
    },
];

fn qwen_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(QWEN_RULES))
}

static QWEN_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1200,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^\x{2733}\x{FE0E}? "]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^\x{25D0}\x{FE0E}? "]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1000,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*⠏\s+.*\.\.\.\s*$"]),
            Matcher::Any(&[
                Matcher::Contains(&["Waiting for user confirmation..."]),
                Matcher::Contains(&["等待用户确认..."]),
                Matcher::Contains(&["等待用戶確認..."]),
                Matcher::Contains(&["Warten auf Benutzerbestätigung..."]),
                Matcher::Contains(&["En attente de la confirmation de l'utilisateur..."]),
                Matcher::Contains(&["ユーザーの確認を待っています..."]),
                Matcher::Contains(&["Aguardando confirmação do usuário..."]),
                Matcher::Contains(&["Ожидание подтверждения от пользователя..."]),
                Matcher::Contains(&["Esperant la confirmació de l'usuari..."]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 990,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::All(&[
            Matcher::Contains(&["yes, allow once"]),
            Matcher::Any(&[
                Matcher::Contains(&["apply this change?"]),
                Matcher::Contains(&["allow execution of:"]),
                Matcher::Contains(&["allow execution of mcp tool"]),
                Matcher::Contains(&["do you want to proceed?"]),
                Matcher::Contains(&["shell command execution"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 980,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::LineRegex(&[
            r"^\s*[❯›]\s*(?:\[(?: |✓)\]\s*)?\d+\.\s+",
            r"^\s*↑/↓\s*:.*(?:Enter|Return)\s*:",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 970,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::Contains(&[
            "do you trust this folder?",
            "trust folder (",
            "don't trust (esc)",
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 900,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::LineRegex(&[
            r"^\s*(?:[⠁-⣿]|\.{1,2})\s+.*\(\d+(?:m(?:\s+\d+s)?|s).*\s·\sesc to cancel\)\s*$",
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 890,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::LineRegex(&[r"^\s*\(\d+(?:m(?:\s+\d+s)?|s)\s·\sesc to cancel\)\s*$"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::BottomNonEmpty(30),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*>\s*(?:type\s*)?.*$"]),
            Matcher::Any(&[
                Matcher::Contains(&["type your message"]),
                Matcher::Contains(&["your message or @path/to/file"]),
                Matcher::Contains(&["@path/to/file"]),
                Matcher::Contains(&["type", "mes", "sage", "@pat", "h/to", "/fil"]),
            ]),
        ]),
    },
];

fn copilot_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(COPILOT_RULES))
}

static COPILOT_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Contains(&["esc to cancel"]),
                Matcher::Contains(&["esc cancel"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["enter to select"]),
                Matcher::Contains(&["enter to confirm"]),
                Matcher::Contains(&["enter to submit"]),
                Matcher::Contains(&["enter accept"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 110,
        region: Region::BottomNonEmpty(6),
        matcher: Matcher::LineRegex(&[r"^\s*◎\s+Waiting for background agents(?:\s|·|$)"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["esc to cancel"]),
            Matcher::Contains(&["esc cancel"]),
            Matcher::Contains(&["esc again to cancel"]),
            Matcher::Contains(&["esc interrupt"]),
        ]),
    },
];

fn pi_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(PI_RULES))
}

static PI_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["Working..."]),
            Matcher::LineRegex(&[r"^[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] Working$"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: Region::BottomNonEmpty(12),
        matcher: Matcher::Any(&[
            Matcher::LineRegex(&[r"^── [⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] Working ─+$"]),
            Matcher::LineRegex(&[r"^[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] Working$"]),
        ]),
    },
];

fn grok_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(GROK_RULES))
}

static GROK_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1300,
        region: Region::Title,
        matcher: Matcher::Contains(&["Action Required"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1200,
        region: WHOLE_RECENT,
        matcher: Matcher::LineRegex(&[r"^\s*┃\s+[0-9a-z]+\s+\([●○]\)\s"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1190,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::Contains(&[":select", "ctrl+o:yolo", "ctrl+c:cancel"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1185,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::Contains(&["tab:scrollback", "shift+x:dismiss"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1180,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["yes, proceed", "no, reject"]),
            Matcher::Any(&[
                Matcher::Contains(&["use ← → to choose permission whitelist scope"]),
                Matcher::Contains(&["←/→:scope"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::All(&[
            Matcher::Regex(&[r"(?:^| - )grok$"]),
            Matcher::Not(&[Matcher::Regex(&[r"[\x{2800}-\x{28FF}]"])]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1000,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"(?:^|\s)[\x{2801}-\x{28FF}](?:\s|$)"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1160,
        region: WHOLE_RECENT,
        matcher: Matcher::LineRegex(&[r"^\s*[\x{2801}-\x{28FF}]\s.*\[stop\]\s*$"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1155,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::All(&[
            Matcher::Contains(&["ctrl+.:shortcuts"]),
            Matcher::Any(&[
                Matcher::Contains(&["esc:cancel"]),
                Matcher::Contains(&["ctrl+c:cancel"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1140,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::All(&[
                Matcher::Contains(&["ctrl+c:cancel", "ctrl+enter:interject"]),
                Matcher::Contains(&["waiting"]),
            ]),
            Matcher::LineRegex(&[r"^\s*[\x{2801}-\x{28FF}]\s+(Run|Read|Search|List)\b"]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::All(&[
            Matcher::Contains(&["ctrl+.:shortcuts"]),
            Matcher::Not(&[
                Matcher::Contains(&["esc:cancel"]),
                Matcher::Contains(&["ctrl+c:cancel"]),
            ]),
        ]),
    },
];

fn droid_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(DROID_RULES))
}

static DROID_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["enter to select", "esc to cancel"]),
            Matcher::Any(&[
                Matcher::Contains(&["↑↓ to navigate"]),
                Matcher::Contains(&["use ↑↓ to navigate"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["> yes, allow"]),
                Matcher::Contains(&["> no, cancel"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 290,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["enter select", "esc cancel"]),
            Matcher::Any(&[
                Matcher::Contains(&["↑/↓ navigate"]),
                Matcher::Contains(&["↑↓ navigate"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 110,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["esc to stop"]),
            Matcher::LineRegex(&[r"^\s*[\x{2800}-\x{28FF}]"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Contains(&["esc to stop"]),
    },
];

fn kimi_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(KIMI_RULES))
}

static KIMI_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 400,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["↵ confirm"]),
            Matcher::Any(&[
                Matcher::Contains(&["run this command?"]),
                Matcher::Contains(&["write this file?"]),
                Matcher::Contains(&["apply these edits?"]),
                Matcher::Contains(&["stop this task?"]),
                Matcher::Contains(&["ready to build with this plan?"]),
                Matcher::LineRegex(&[r"(?i)^\s*▶?\s*approve .*\?$"]),
            ]),
            Matcher::All(&[
                Matcher::Contains(&[" choose"]),
                Matcher::Any(&[
                    Matcher::Contains(&["approve"]),
                    Matcher::Contains(&["reject"]),
                    Matcher::Contains(&["revise"]),
                ]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 390,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["↑↓ select", "esc cancel"]),
            Matcher::LineRegex(&[r"^\s*question\s*$", r"^\s*\? "]),
            Matcher::Any(&[
                Matcher::Contains(&["↵ choose"]),
                Matcher::Contains(&["↵ toggle"]),
                Matcher::Contains(&["↵ save"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["requesting approval", "reject"]),
            Matcher::Any(&[
                Matcher::Contains(&["approve once"]),
                Matcher::Contains(&["approve for this session"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["1/2/3/4 choose"]),
                Matcher::Contains(&["↵ confirm"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 120,
        region: Region::BottomNonEmpty(3),
        matcher: Matcher::LineRegex(&[
            r"(?i)\bkimi[-\w.]*\s+thinking\b.*\[[1-9][0-9]*\s+agents?\s+running\]",
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::LineRegex(&[r"^\s*(🌕|🌖|🌗|🌘|🌑|🌒|🌓|🌔)\s*$"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 90,
        region: WHOLE_RECENT,
        matcher: Matcher::LineRegex(&[
            r"(?i)^\s*[\x{2800}-\x{28FF}]+\s*(thinking\.\.\.|working\.\.\.|using )",
        ]),
    },
];

fn amp_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(AMP_RULES))
}

static AMP_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::Contains(&["Plugin confirmation needed"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1050,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^[\x{2800}-\x{28FF}] "]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["waiting for approval"]),
            Matcher::Contains(&["invoke tool"]),
            Matcher::Contains(&["run this command?"]),
            Matcher::Contains(&["allow editing file:"]),
            Matcher::Contains(&["allow creating file:"]),
            Matcher::Contains(&["confirm tool call"]),
            Matcher::All(&[
                Matcher::Contains(&["approve"]),
                Matcher::Any(&[
                    Matcher::Contains(&["allow all for this session"]),
                    Matcher::Contains(&["allow all for every session"]),
                    Matcher::Contains(&["allow file for every session"]),
                    Matcher::Contains(&["deny with feedback"]),
                ]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 200,
        region: Region::BottomNonEmpty(5),
        matcher: Matcher::LineRegex(&[
            r"(?i)^\s*╰\s+\S+\s+(thinking|streaming|running tools|waiting)\s+─",
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Contains(&["esc to cancel"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 50,
        region: Region::Title,
        matcher: Matcher::All(&[
            Matcher::Contains(&[" - amp - "]),
            Matcher::Not(&[
                Matcher::Regex(&[r"^[\x{2800}-\x{28FF}] "]),
                Matcher::Contains(&["Plugin confirmation needed"]),
            ]),
        ]),
    },
];

fn hermes_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(HERMES_RULES))
}

static HERMES_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1100,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^⚠[\x{fe0e}\x{fe0f}]?(?:\s|$)"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 1050,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^⏳[\x{fe0e}\x{fe0f}]?(?:\s|$)"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 900,
        region: Region::BottomNonEmpty(14),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Contains(&["dangerous"]),
                Matcher::Contains(&["approval"]),
                Matcher::Contains(&["allow once", "deny"]),
                Matcher::LineRegex(&[r"(?i)^\s*[▸>]?\s*1\.\s*allow"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["enter confirm"]),
                Matcher::Contains(&["enter to confirm"]),
                Matcher::Contains(&["↑/↓ to select"]),
                Matcher::Contains(&["show full command"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 900,
        region: Region::BottomNonEmpty(14),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Contains(&["hermes needs your"]),
                Matcher::LineRegex(&[r"^\s*ask\s+\S"]),
                Matcher::Contains(&["type your answer"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["enter confirm"]),
                Matcher::Contains(&["enter to confirm"]),
                Matcher::Contains(&["enter send"]),
                Matcher::Contains(&["press enter"]),
                Matcher::Contains(&["↑/↓ select"]),
                Matcher::Contains(&["↑/↓ to select"]),
                Matcher::Contains(&["other (type"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 900,
        region: Region::BottomNonEmpty(14),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["sudo password"]),
            Matcher::Contains(&["skill setup"]),
            Matcher::Contains(&["🔑", "for "]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 900,
        region: Region::BottomNonEmpty(14),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::Contains(&["approve once", "cancel"]),
                Matcher::Contains(&["start a new session", "keep going"]),
            ]),
            Matcher::Any(&[
                Matcher::Contains(&["enter to confirm"]),
                Matcher::Contains(&["enter confirm"]),
                Matcher::Contains(&["type 1/2/3"]),
                Matcher::Contains(&["y/n quick"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 950,
        region: Region::BottomNonEmpty(5),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["msg=interrupt"]),
            Matcher::Contains(&["ctrl+c to interrupt"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 500,
        region: Region::BottomNonEmpty(5),
        matcher: Matcher::Contains(&["ctrl+c cancel"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^✓[\x{fe0e}\x{fe0f}]?(?:\s|$)"]),
    },
];

fn qoder_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(QODER_RULES))
}

static QODER_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::All(&[
                Matcher::Contains(&["waiting for user confirmation"]),
                Matcher::Any(&[
                    Matcher::Contains(&["yes"]),
                    Matcher::Contains(&["no"]),
                    Matcher::Contains(&["allow"]),
                    Matcher::Contains(&["reject"]),
                ]),
            ]),
            Matcher::All(&[
                Matcher::Contains(&["awaiting approval"]),
                Matcher::Any(&[
                    Matcher::Contains(&["allow"]),
                    Matcher::Contains(&["reject"]),
                ]),
            ]),
            Matcher::Contains(&["permission required"]),
            Matcher::Contains(&["allow once or always?"]),
            Matcher::Contains(&["asking user"]),
            Matcher::Contains(&["enter your response"]),
            Matcher::Contains(&["review your answers:"]),
            Matcher::Contains(&["shell awaiting input"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Contains(&["(esc to cancel,"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 90,
        region: WHOLE_RECENT,
        matcher: Matcher::LineRegex(&[r"^\s*[\x{2800}-\x{28FF}]\s+.*\p{Alphabetic}"]),
    },
];

fn antigravity_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(ANTIGRAVITY_RULES))
}

static ANTIGRAVITY_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::Contains(&["navigate", "tab amend"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 290,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::Contains(&["navigate", "esc skip"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 280,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Contains(&[
            "do you trust the contents of this project?",
            "yes, i trust this folder",
            "enter confirm",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 270,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["requesting permission for:"]),
            Matcher::Any(&[
                Matcher::Contains(&["do you want to proceed?"]),
                Matcher::Contains(&["tab amend", "edit command"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 110,
        region: Region::BottomNonEmpty(2),
        matcher: Matcher::Regex(&[r"(?m)^\s*─{3,}\s*esc to cancel\b"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: Region::BottomNonEmpty(12),
        matcher: Matcher::LineRegex(&[r"^\s*[\x{2801}-\x{28FF}]+\s+\S"]),
    },
];

fn cline_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(CLINE_RULES))
}

static CLINE_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["let cline use this tool"]),
            Matcher::Contains(&["[act mode]", "execute command?", "yes"]),
            Matcher::Contains(&["[act mode]", "use this tool?", "yes"]),
            Matcher::Contains(&["[plan mode]", "execute command?", "yes"]),
            Matcher::Contains(&["[plan mode]", "use this tool?", "yes"]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: Region::BottomNonEmpty(16),
        matcher: Matcher::Contains(&[
            "Cline needs permission",
            "Approve tool call?",
            "[y] Approve",
            "[n] Deny",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: Region::BottomNonEmpty(24),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*Cline is asking a question\s*$", r"^\s*>\s+\S"]),
            Matcher::Contains(&["(Tab)", "Shift+Tab"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 200,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::Any(&[
            Matcher::LineRegex(&[r"^\s*[\x{2801}-\x{28FF}]\s+\S"]),
            Matcher::Contains(&["Thinking... (esc to cancel)"]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::BottomNonEmpty(12),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*❯(?:\s.*)?$"]),
            Matcher::Contains(&["─", "(Tab)", "Shift+Tab"]),
        ]),
    },
];

fn devin_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(DEVIN_RULES))
}

static DEVIN_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Contains(&[
            "do you trust the authors of this directory?",
            "with untrusted content.",
            "yes, trust ",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 290,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Contains(&["approve once", "select", "confirm", "esc cancel"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 200,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["running tools", "esc to interrupt"]),
            Matcher::Not(&[Matcher::Contains(&["approve once", "esc cancel"])]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 190,
        region: Region::BottomNonEmpty(6),
        matcher: Matcher::All(&[
            Matcher::Contains(&["guide devin while it works"]),
            Matcher::Not(&[Matcher::Contains(&["approve once", "esc cancel"])]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 180,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["reading shell ", "timeout:"]),
            Matcher::Not(&[Matcher::Contains(&["approve once", "esc cancel"])]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 120,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["ask devin to build", "features, fix bugs", "your code"]),
            Matcher::LineRegex(&[r"^\s*❭ Ask Devin to build"]),
            Matcher::Not(&[
                Matcher::Contains(&["approve once", "esc cancel"]),
                Matcher::Contains(&["running tools", "esc to interrupt"]),
                Matcher::Contains(&["guide devin while it works"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::BottomNonEmpty(6),
        matcher: Matcher::All(&[
            Matcher::Contains(&["context:"]),
            Matcher::LineRegex(&[r"^\s*❭"]),
            Matcher::Not(&[
                Matcher::Contains(&["approve once", "esc cancel"]),
                Matcher::Contains(&["running tools", "esc to interrupt"]),
                Matcher::Contains(&["guide devin while it works"]),
            ]),
        ]),
    },
];

fn kiro_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(KIRO_RULES))
}

static KIRO_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Idle,
        priority: 1100,
        region: Region::BottomNonEmpty(4),
        matcher: Matcher::LineRegex(&[
            r"(?i)^\s*[>›]\s*ask a question or describe a task(?:\s+(?:enter|↵))?\s*$",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1050,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["esc to close"]),
            Matcher::Any(&[
                Matcher::LineRegex(&[
                    r"(?i)^\s*[>❯]?\s*(?:allow|always allow|deny|always deny)\s*$",
                ]),
                Matcher::LineRegex(&[
                    r"(?i)^\s*[>❯]?\s*(?:yes, single permission|trust, always allow in this session|no \(tab to edit\))\s*$",
                ]),
                Matcher::LineRegex(&[r"(?i)^\s*[>❯]?\s*trust\b.*$"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1040,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Contains(&[
            "tool approval",
            "approve all pending",
            "configure individually (agent monitor)",
            "exit (cancel subagents)",
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1030,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Contains(&["to navigate", "to submit", "esc to cancel"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 950,
        region: Region::BottomNonEmpty(4),
        matcher: Matcher::Contains(&["kiro is working", "type to steer", "ctrl+s to queue"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 900,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"(?i)^[◐◓◑◒/|\-]\s+kiro:"]),
    },
];

fn kilo_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(KILO_RULES))
}

static KILO_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 300,
        region: WHOLE_RECENT,
        matcher: Matcher::Any(&[
            Matcher::Contains(&["△ Permission required"]),
            Matcher::All(&[
                Matcher::Contains(&["esc dismiss"]),
                Matcher::Any(&[
                    Matcher::Contains(&["enter confirm"]),
                    Matcher::Contains(&["enter submit"]),
                    Matcher::Contains(&["enter toggle"]),
                ]),
                Matcher::Any(&[
                    Matcher::Contains(&["↑↓ select"]),
                    Matcher::Contains(&["⇆ tab"]),
                ]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 100,
        region: WHOLE_RECENT,
        matcher: Matcher::Contains(&["esc interrupt"]),
    },
];

fn letta_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(LETTA_RULES))
}

static LETTA_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 1300,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"^\[ [!.] \] Action Required(?: \| |$)"]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 1200,
        region: Region::BottomNonEmpty(20),
        matcher: Matcher::Contains(&["Run this command?", "Enter to select · Esc to cancel"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 900,
        region: Region::Title,
        matcher: Matcher::Regex(&[r"(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)"]),
    },
    Rule {
        state: AgentState::Working,
        priority: 850,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::LineRegex(&[
            r"^\s*(?:\S+\s+)+is(?: \S+)*… \((?:esc to interrupt(?: · .*)?|interrupting)\)\s*$",
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 800,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::LineRegex(&[r"^\s*(?:└\s*)?Running\.\.\.\s*(?:\(.*\))?$"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 100,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::LineRegex(&[r"^\s*›\s*$"]),
                Matcher::LineRegex(&[r#"^\s*›\s+Try\s+""#]),
            ]),
            Matcher::Not(&[
                Matcher::LineRegex(&[
                    r"^\s*(?:\S+\s+)+is(?: \S+)*… \((?:esc to interrupt(?: · .*)?|interrupting)\)\s*$",
                ]),
                Matcher::LineRegex(&[r"^\s*(?:└\s*)?Running\.\.\.\s*(?:\(.*\))?$"]),
            ]),
        ]),
    },
];

fn maki_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(MAKI_RULES))
}

static MAKI_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 980,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["permission required"]),
            Matcher::Any(&[
                Matcher::Contains(&["y allow", "n deny"]),
                Matcher::Contains(&["confirm allow"]),
                Matcher::Contains(&["confirm deny"]),
                Matcher::Contains(&["enter deny", "esc cancel"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 970,
        region: WHOLE_RECENT,
        matcher: Matcher::All(&[
            Matcher::Contains(&["plan complete", "enter confirm"]),
            Matcher::Any(&[
                Matcher::Contains(&["space toggle parallel"]),
                Matcher::Contains(&["edit plan"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 900,
        region: Region::BottomNonEmpty(1),
        matcher: Matcher::LineRegex(&[r"^( [\x{2800}-\x{28FF}]){1,2} \[(BUILD|PLAN|BASH)\]"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 850,
        region: Region::BottomNonEmpty(1),
        matcher: Matcher::LineRegex(&[r"^ \[(BUILD|PLAN|BASH)\]"]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 840,
        region: Region::BottomNonEmpty(3),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^❯ "]),
            Matcher::Not(&[
                Matcher::Contains(&["queue another prompt"]),
                Matcher::LineRegex(&[r"^( [\x{2800}-\x{28FF}]){1,2} "]),
            ]),
        ]),
    },
];

fn muse_rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| compile(MUSE_RULES))
}

static MUSE_RULES: &[Rule] = &[
    Rule {
        state: AgentState::Blocked,
        priority: 970,
        region: Region::BottomNonEmpty(12),
        matcher: Matcher::All(&[
            Matcher::Contains(&["Do you trust this workspace?"]),
            Matcher::Any(&[
                Matcher::Contains(&["Trust and continue"]),
                Matcher::Contains(&["Use Up/Down"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 950,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["Enter to select", "Tab for an optional note"]),
            Matcher::Contains(&["Enter to toggle", "Esc to interrupt"]),
        ]),
    },
    Rule {
        state: AgentState::Working,
        priority: 900,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::All(&[
            Matcher::Contains(&["esc to interrupt"]),
            Matcher::Not(&[
                Matcher::Contains(&["Enter to select", "Tab for an optional note"]),
                Matcher::Contains(&["Enter to toggle", "Esc to interrupt"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Blocked,
        priority: 850,
        region: Region::BottomNonEmpty(8),
        matcher: Matcher::Any(&[
            Matcher::Contains(&["Allow this stage once", "Always allow in this workspace"]),
            Matcher::Contains(&["Allow once", "Allow for this session"]),
            Matcher::Contains(&["Yes, proceed", "Yes, don't ask again this session"]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 700,
        region: Region::BottomNonEmpty(5),
        matcher: Matcher::All(&[
            Matcher::Any(&[
                Matcher::LineRegex(&[r"^\s*⟩\s*$"]),
                Matcher::LineRegex(&[r"^\s*⟩\s+\S"]),
            ]),
            Matcher::Not(&[
                Matcher::Contains(&["esc to interrupt"]),
                Matcher::Contains(&["Enter to select", "Tab for an optional note"]),
                Matcher::Contains(&["Enter to toggle", "Esc to interrupt"]),
                Matcher::Contains(&["enter confirm", "esc go back"]),
                Matcher::Contains(&["enter save", "esc go back"]),
                Matcher::Contains(&["space toggle", "esc close", "type filter"]),
            ]),
        ]),
    },
    Rule {
        state: AgentState::Idle,
        priority: 500,
        region: Region::BottomNonEmpty(3),
        matcher: Matcher::All(&[
            Matcher::LineRegex(&[r"^\s*\S+ · (none|minimal|low|medium|high|xhigh|ultra) · "]),
            Matcher::Not(&[Matcher::Contains(&["esc to interrupt"])]),
        ]),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str], title: &str) -> Screen {
        Screen {
            lines: lines.iter().map(|line| line.to_string()).collect(),
            title: title.to_string(),
        }
    }

    #[test]
    fn claude_spinner_title_is_working() {
        let screen = screen(&["some output"], "⠂ claude");
        assert_eq!(detect("claude", &screen), Some(AgentState::Working));
    }

    #[test]
    fn claude_idle_prompt_box() {
        let screen = screen(&["│ > │", "❯ "], "claude");
        assert_eq!(detect("claude", &screen), Some(AgentState::Idle));
    }

    #[test]
    fn claude_permission_dialog_is_blocked() {
        let screen = screen(
            &[
                "Bash command",
                "  rm -rf build",
                "────────────────────────────",
                "Do you want to proceed?",
                "❯ 1. Yes",
                "  2. No",
                "esc to cancel",
            ],
            "claude",
        );
        assert_eq!(detect("claude", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn claude_select_dialog_is_blocked() {
        let screen = screen(
            &[
                "────────────────────────────",
                "Select an option",
                "❯ 1. Option A",
                "  2. Option B",
                "enter to select · tab/arrow keys to navigate",
                "esc to cancel",
            ],
            "claude",
        );
        assert_eq!(detect("claude", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn claude_prompt_with_dialog_hint_is_not_idle() {
        let screen = screen(&["❯ ", "esc to cancel"], "claude");
        assert_ne!(detect("claude", &screen), Some(AgentState::Idle));
    }

    #[test]
    fn claude_interrupt_hint_is_working() {
        let screen = screen(
            &["⏸ plan mode on (shift+tab to cycle) · esc to interrupt"],
            "claude",
        );
        assert_eq!(detect("claude", &screen), Some(AgentState::Working));
    }

    #[test]
    fn codex_action_required_title_is_blocked() {
        let screen = screen(&["waiting"], "Action Required | codex");
        assert_eq!(detect("codex", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn codex_spinner_title_is_working() {
        let screen = screen(&["working"], "⠹ codex");
        assert_eq!(detect("codex", &screen), Some(AgentState::Working));
    }

    #[test]
    fn codex_elapsed_status_is_working() {
        let screen = screen(&["• Running cargo test (12s • esc to interrupt)"], "codex");
        assert_eq!(detect("codex", &screen), Some(AgentState::Working));
    }

    #[test]
    fn codex_yes_no_prompt_is_blocked() {
        let screen = screen(&["Allow command? [y/n]"], "codex");
        assert_eq!(detect("codex", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn codex_plain_title_is_idle() {
        let screen = screen(&["› "], "codex");
        assert_eq!(detect("codex", &screen), Some(AgentState::Idle));
    }

    #[test]
    fn unknown_agent_has_no_detection() {
        let screen = screen(&["do you want to proceed?"], "goose");
        assert_eq!(detect("goose", &screen), None);
        assert!(!supports("goose"));
    }

    #[test]
    fn gemini_permission_dialog_is_blocked() {
        let screen = screen(&["│ Apply this change"], "gemini");
        assert_eq!(detect("gemini", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn gemini_cancel_hint_is_working() {
        let screen = screen(&["esc to cancel"], "gemini");
        assert_eq!(detect("gemini", &screen), Some(AgentState::Working));
    }

    #[test]
    fn opencode_permission_is_blocked() {
        let screen = screen(&["△ Permission required"], "opencode");
        assert_eq!(detect("opencode", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn opencode_interrupt_hint_is_working() {
        let screen = screen(&["ctrl+c to interrupt"], "opencode");
        assert_eq!(detect("opencode", &screen), Some(AgentState::Working));
    }

    #[test]
    fn cursor_write_approval_is_blocked() {
        let screen = screen(
            &[
                "write to this file?",
                "proceed (y)",
                "reject & propose changes",
            ],
            "cursor-agent",
        );
        assert_eq!(detect("cursor", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn qwen_title_states() {
        assert_eq!(
            detect("qwen", &screen(&["x"], "✳ qwen")),
            Some(AgentState::Blocked)
        );
        assert_eq!(
            detect("qwen", &screen(&["x"], "◐ qwen")),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn copilot_selection_dialog_is_blocked() {
        let screen = screen(&["esc to cancel", "enter to select"], "copilot");
        assert_eq!(detect("copilot", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn pi_working_border_is_working() {
        let screen = screen(&["── ⠋ Working ─────"], "pi");
        assert_eq!(detect("pi", &screen), Some(AgentState::Working));
    }

    #[test]
    fn grok_action_required_is_blocked() {
        assert_eq!(
            detect("grok", &screen(&["x"], "⚠ Action Required")),
            Some(AgentState::Blocked)
        );
    }

    #[test]
    fn grok_stop_chip_is_working() {
        let screen = screen(&["⠧ Waiting on subagent… 2.8s   13s ⇣29.7k [stop]"], "grok");
        assert_eq!(detect("grok", &screen), Some(AgentState::Working));
    }

    #[test]
    fn droid_stop_hint_is_working() {
        let screen = screen(&["esc to stop"], "droid");
        assert_eq!(detect("droid", &screen), Some(AgentState::Working));
    }

    #[test]
    fn kimi_moon_spinner_is_working() {
        let screen = screen(&["🌕"], "kimi");
        assert_eq!(detect("kimi", &screen), Some(AgentState::Working));
    }

    #[test]
    fn amp_plugin_confirmation_is_blocked() {
        assert_eq!(
            detect("amp", &screen(&["x"], "Plugin confirmation needed")),
            Some(AgentState::Blocked)
        );
    }

    #[test]
    fn hermes_title_states() {
        assert_eq!(
            detect("hermes", &screen(&["x"], "⚠ hermes")),
            Some(AgentState::Blocked)
        );
        assert_eq!(
            detect("hermes", &screen(&["x"], "⏳ hermes")),
            Some(AgentState::Working)
        );
        assert_eq!(
            detect("hermes", &screen(&["x"], "✓ hermes")),
            Some(AgentState::Idle)
        );
    }

    #[test]
    fn qoder_permission_required_is_blocked() {
        let screen = screen(&["permission required"], "qoder");
        assert_eq!(detect("qoder", &screen), Some(AgentState::Blocked));
        assert_eq!(detect("qoder-cn", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn antigravity_permission_footer_is_blocked() {
        let screen = screen(&["↑/↓ Navigate · tab Amend"], "agy");
        assert_eq!(detect("antigravity", &screen), Some(AgentState::Blocked));
    }

    #[test]
    fn every_supported_agent_has_sorted_rules() {
        let agents = [
            "claude",
            "codex",
            "gemini",
            "opencode",
            "cursor",
            "qwen",
            "copilot",
            "pi",
            "grok",
            "droid",
            "kimi",
            "amp",
            "hermes",
            "qoder",
            "qoder-cn",
            "antigravity",
            "cline",
            "devin",
            "kiro",
            "kilo",
            "letta",
            "maki",
            "muse",
        ];
        for agent in agents {
            assert!(supports(agent), "{agent} should be supported");
            let rules = rules_for(agent).expect("rules");
            let priorities: Vec<u32> = rules.iter().map(|rule| rule.priority).collect();
            let mut sorted = priorities.clone();
            sorted.sort_by(|a, b| b.cmp(a));
            assert_eq!(priorities, sorted, "{agent} rules must be priority-sorted");
        }
    }

    #[test]
    fn newly_portable_agents_detect_their_signals() {
        let cases: &[(&str, &[&str], &str, AgentState)] = &[
            (
                "cline",
                &["[act mode]", "execute command?", "yes"],
                "cline",
                AgentState::Blocked,
            ),
            (
                "devin",
                &["approve once", "select", "confirm", "esc cancel"],
                "devin",
                AgentState::Blocked,
            ),
            (
                "kiro",
                &["esc to close", "allow"],
                "kiro",
                AgentState::Blocked,
            ),
            (
                "kilo",
                &["△ Permission required"],
                "kilo",
                AgentState::Blocked,
            ),
            (
                "letta",
                &["Run this command?", "Enter to select · Esc to cancel"],
                "letta",
                AgentState::Blocked,
            ),
            (
                "maki",
                &["permission required", "y allow", "n deny"],
                "maki",
                AgentState::Blocked,
            ),
            (
                "muse",
                &["Allow this stage once", "Always allow in this workspace"],
                "muse",
                AgentState::Blocked,
            ),
        ];
        for (agent, lines, title, expected) in cases {
            assert_eq!(
                detect(agent, &screen(lines, title)),
                Some(*expected),
                "{agent}"
            );
        }
    }

    #[test]
    fn horizontal_rule_detection() {
        assert!(is_horizontal_rule("────────────"));
        assert!(is_horizontal_rule("─── label ───"));
        // Box borders and ASCII dashes are not separators.
        assert!(!is_horizontal_rule("╰──────────╯"));
        assert!(!is_horizontal_rule("-----  "));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule(""));
    }
}
