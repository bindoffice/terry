use editor::{Editor, MultiBufferOffset};
use futures::FutureExt;
use gpui::Keystroke;
use gpui::{
    Action, Animation, AnimationExt, AnyElement, App, Axis, Context, Entity, EntityId,
    EventEmitter, FocusHandle, Focusable, PromptLevel, Render, SharedString,
    StatefulInteractiveElement, Subscription, Task, TaskExt, WeakEntity, Window, actions, div,
    pulsating_between, px,
};
use project::Project;
use terminal::terminal_settings::{TerminalProfile, TerminalSettings};
use terminal_view::{NewTerminalWithProfile, TerminalView};
use ui::{
    ContextMenu, IconButton, IconName, Label, LabelSize, PopoverMenu, Tooltip, prelude::*,
    right_click_menu,
};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent, PanelStatusButton};
use workspace::{ItemHandle, Member, Pane, PaneAxis, PaneGroup, ReopenClosedItem};
use zed_actions::terminal_list_panel::{
    AttachTmuxSession, ConnectSshHost, ConnectSshHostWithTmux, ImportTerminalColorScheme, NewGroup,
    NewTerminal, NextGroup, PreviousGroup, ReopenClosedTerminal, SelectTerminalColorScheme,
    ToggleFocus,
};

use crate::agent_detect::{self, AgentState};
use crate::persistent_sessions;

use serde::{Deserialize, Serialize};
use settings::Settings;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

actions!(
    terminal_list_panel,
    [
        /// Focuses the next terminal whose agent is waiting for input.
        FocusNextWaitingAgent,
        /// Opens the project diff so the changes an agent made can be reviewed.
        ReviewAgentChanges,
    ]
);

/// Max consecutive automatic reconnects for a remote terminal.
const MAX_REMOTE_RECONNECT_ATTEMPTS: u32 = 5;

/// Programs that should be reconnected automatically when they exit because
/// the remote side went away (as opposed to the user closing the tab).
fn is_remote_program(program: &str) -> bool {
    matches!(command_basename(program).as_str(), "ssh" | "mosh")
}

/// Which window currently owns the shared restore file for a workspace key.
/// A second window for the same workspace starts fresh instead of respawning
/// the same PTYs.
fn session_owners() -> &'static Mutex<HashMap<String, EntityId>> {
    static OWNERS: OnceLock<Mutex<HashMap<String, EntityId>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn try_claim_session(workspace_key: &str, owner: EntityId) -> bool {
    let mut owners = session_owners()
        .lock()
        .expect("session owners mutex poisoned");
    match owners.get(workspace_key) {
        Some(existing) if *existing != owner => false,
        _ => {
            owners.insert(workspace_key.to_string(), owner);
            true
        }
    }
}

fn release_session_claim(workspace_key: &str, owner: EntityId) {
    let mut owners = session_owners()
        .lock()
        .expect("session owners mutex poisoned");
    if owners.get(workspace_key) == Some(&owner) {
        owners.remove(workspace_key);
    }
}

/// A terminal whose foreground process is a coding agent. Kept alive after the
/// tab is closed so opening a terminal again continues the same session.
struct ParkedAgentSession {
    view: Entity<TerminalView>,
    cwd: Option<PathBuf>,
}

/// A terminal the user closed, remembered so it can be reopened with the same
/// cwd, launch (profile / ssh / tmux), colors and font size.
struct ClosedTerminal {
    group_id: GroupId,
    terminal: PersistedTerminal,
}

/// A live terminal kept alive outside every group so its PTY, scrollback and
/// running processes survive being detached from the sidebar.
struct DetachedSession {
    view: Entity<TerminalView>,
    title: SharedString,
    cwd: Option<PathBuf>,
}

/// Drag payload for reordering groups in the sidebar.
#[derive(Clone)]
struct DraggedGroup {
    id: GroupId,
    name: SharedString,
}

/// Minimal preview rendered under the cursor while dragging a group.
struct DraggedGroupPreview {
    name: SharedString,
}

impl Render for DraggedGroupPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().element_background)
            .child(Label::new(self.name.clone()).size(LabelSize::Small))
    }
}

/// Group-menu actions for the coding agent in that group.
struct AgentSessionActions {
    cwd: Option<PathBuf>,
    resume: bool,
    resume_command: Option<String>,
    fork: Option<String>,
    past: Option<String>,
}

/// Agent identity remembered after a terminal closes, including after the
/// process itself has exited. `pending_resume` is consumed by the next new
/// terminal. Fork and past-session actions keep working afterwards.
struct RememberedAgentSession {
    agent_id: &'static str,
    /// Allowlisted CLI name used in the resume command. Never a raw process path.
    bin: &'static str,
    cwd: Option<PathBuf>,
    pending_resume: bool,
}

const PARKED_AGENT_SESSION_LIMIT: usize = 8;
const REMEMBERED_AGENT_SESSION_LIMIT: usize = 8;

/// Public CLI commands for continuing, listing, or forking a coding-agent
/// session. `{bin}` is replaced with an allowlisted executable name.
///
/// Commands are only set when the CLI documents a one-shot flag or subcommand.
/// Agents without a published command stay detected so a live process can be
/// reattached, and do not get a guessed flag.
struct AgentCli {
    id: &'static str,
    names: &'static [&'static str],
    bin: &'static str,
    resume: Option<&'static str>,
    past: Option<&'static str>,
    fork: Option<&'static str>,
}

fn coding_agents() -> &'static [AgentCli] {
    &[
        AgentCli {
            id: "claude",
            names: &["claude", "claude-code"],
            bin: "claude",
            resume: Some("{bin} --continue"),
            past: Some("{bin} --resume"),
            fork: Some("{bin} --continue --fork-session"),
        },
        AgentCli {
            id: "codex",
            names: &["codex"],
            bin: "codex",
            resume: Some("{bin} resume --last"),
            past: Some("{bin} resume"),
            fork: Some("{bin} fork"),
        },
        AgentCli {
            id: "trae",
            names: &["traecli", "trae", "traecode"],
            bin: "traecli",
            resume: Some("{bin} resume"),
            past: None,
            fork: Some("{bin} fork"),
        },
        AgentCli {
            id: "opencode",
            names: &["opencode"],
            bin: "opencode",
            resume: Some("{bin} --continue"),
            past: None,
            fork: Some("{bin} --continue --fork"),
        },
        AgentCli {
            id: "omp",
            names: &["omp", "oh-my-pi"],
            bin: "omp",
            resume: Some("{bin} --continue"),
            past: Some("{bin} --resume"),
            // `--fork` requires a session id, so there is no one-shot command.
            fork: None,
        },
        AgentCli {
            id: "qoder",
            names: &["qoder", "qodercli"],
            bin: "qoder",
            resume: Some("{bin} -c"),
            past: Some("{bin} -r"),
            fork: Some("{bin} --fork-session -r"),
        },
        AgentCli {
            id: "qoder-cn",
            names: &["qoder-cn"],
            bin: "qoder-cn",
            resume: Some("{bin} -c"),
            past: Some("{bin} -r"),
            fork: Some("{bin} --fork-session -r"),
        },
        AgentCli {
            id: "gemini",
            names: &["gemini"],
            bin: "gemini",
            resume: Some("{bin} --resume"),
            past: None,
            fork: None,
        },
        AgentCli {
            id: "cursor",
            names: &["cursor-agent", "cursor"],
            bin: "cursor-agent",
            resume: Some("{bin} --resume"),
            past: None,
            fork: None,
        },
        // Detected, and a still-running process is reattached. No documented
        // one-shot resume, fork, or past-session command.
        AgentCli {
            id: "grok",
            names: &["grok"],
            bin: "grok",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "prime",
            names: &["prime-agent", "prime"],
            bin: "prime-agent",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "droid",
            names: &["droid"],
            bin: "droid",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "qwen",
            names: &["qwen", "qwen-code"],
            bin: "qwen",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "goose",
            names: &["goose"],
            bin: "goose",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "codebuddy",
            names: &["codebuddy"],
            bin: "codebuddy",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "copilot",
            names: &["copilot"],
            bin: "copilot",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kimi",
            names: &["kimi"],
            bin: "kimi",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "pi",
            names: &["pi"],
            bin: "pi",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "crush",
            names: &["crush"],
            bin: "crush",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "antigravity",
            names: &["agy", "antigravity"],
            bin: "agy",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "aider",
            names: &["aider"],
            bin: "aider",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "amp",
            names: &["amp"],
            bin: "amp",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "auggie",
            names: &["auggie"],
            bin: "auggie",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "hermes",
            names: &["hermes"],
            bin: "hermes",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "vibe",
            names: &["vibe"],
            bin: "vibe",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "empryo",
            names: &["empryo"],
            bin: "empryo",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "cline",
            names: &["cline"],
            bin: "cline",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "devin",
            names: &["devin", "devin-cli"],
            bin: "devin",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "mastracode",
            names: &["mastracode", "mastra-code"],
            bin: "mastracode",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kiro",
            names: &["kiro", "kiro-cli"],
            bin: "kiro-cli",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "kilo",
            names: &["kilo", "kilo-code"],
            bin: "kilo",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "letta",
            names: &["letta", "letta-code"],
            bin: "letta",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "maki",
            names: &["maki"],
            bin: "maki",
            resume: None,
            past: None,
            fork: None,
        },
        AgentCli {
            id: "muse",
            names: &["muse", "muse-code", "muse-cli"],
            bin: "muse",
            resume: None,
            past: None,
            fork: None,
        },
    ]
}

fn command_basename(name: &str) -> String {
    let base = name
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(name);
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    base.to_ascii_lowercase()
}

fn agent_for_basename(base: &str) -> Option<&'static AgentCli> {
    coding_agents().iter().find(|agent| {
        agent.names.iter().any(|name| {
            base == *name
                || base.starts_with(&format!("{name}-"))
                || base.starts_with(&format!("{name}_"))
        })
    })
}

/// Recognises coding agents installed as npm packages.
///
/// npm-installed CLIs run as `node .../node_modules/<package>/dist/cli.js`, so
/// the foreground process name is the runtime (`node`) rather than the agent.
/// Matching the package entry point keeps those terminals identifiable.
fn agent_for_package_path(path: &str) -> Option<&'static AgentCli> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();

    const PACKAGES: &[(&str, &str)] = &[
        (
            "node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
            "pi",
        ),
        (
            "node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
            "pi",
        ),
        ("node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js", "omp"),
        ("node_modules/@moonshot-ai/kimi-code/dist/main.mjs", "kimi"),
        ("node_modules/@qwen-code/qwen-code/dist/index.js", "qwen"),
        ("node_modules/@qwen-code/qwen-code/dist/index.mjs", "qwen"),
        ("node_modules/mastracode/dist/cli", "mastracode"),
        ("node_modules/mastracode/dist/cli.js", "mastracode"),
        ("node_modules/@letta-ai/letta-code/letta", "letta"),
    ];

    for (suffix, id) in PACKAGES {
        if normalized.ends_with(suffix) {
            return agent_by_id(id);
        }
    }
    None
}

/// Coding agent running in `terminal`, matching either the process name or the
/// npm package entry point of a runtime-hosted CLI.
fn coding_agent_for_terminal(terminal: &terminal::Terminal) -> Option<&'static AgentCli> {
    if let Some(command) = terminal.foreground_process_command_name() {
        let base = command_basename(&command);
        if let Some(agent) = agent_for_basename(&base) {
            return Some(agent);
        }
    }
    terminal
        .foreground_process_script_path()
        .as_deref()
        .and_then(agent_for_package_path)
}

fn invocation_bin(agent: &'static AgentCli, base: &str) -> &'static str {
    agent
        .names
        .iter()
        .copied()
        .find(|name| *name == base)
        .unwrap_or(agent.bin)
}

fn agent_command(template: &str, bin: &str) -> String {
    template.replace("{bin}", bin)
}

/// One-shot resume command for a session reported by an integration.
fn agent_session_resume_command(agent_id: &str, session: &str) -> Option<String> {
    let template = match agent_id {
        "pi" => "{bin} --session {session}",
        "opencode" => "{bin} --session {session}",
        "claude" => "{bin} --resume {session}",
        "kimi" => "{bin} --session {session}",
        "qwen" => "{bin} --resume {session}",
        "codex" => "{bin} resume {session}",
        _ => return None,
    };
    let agent = agent_by_id(agent_id)?;
    Some(
        template
            .replace("{bin}", agent.bin)
            .replace("{session}", &shell_quote(session)),
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn agent_by_id(id: &str) -> Option<&'static AgentCli> {
    coding_agents().iter().find(|agent| agent.id == id)
}

fn is_coding_agent_name(name: &str) -> bool {
    agent_for_basename(&command_basename(name)).is_some()
}

fn text_mentions_coding_agent(text: &str) -> bool {
    agent_mentioned_in_text(text).is_some()
}

fn agent_mentioned_in_text(text: &str) -> Option<(&'static AgentCli, &'static str)> {
    // Short command names (pi, amp, omp, agy) match process names only.
    // Matching them inside a title produces false positives.
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .filter(|token| token.chars().count() >= 4)
        .find_map(|token| {
            let base = command_basename(token);
            let agent = agent_for_basename(&base)?;
            Some((agent, invocation_bin(agent, &base)))
        })
}

fn coding_agent_from_view(
    view: &Entity<TerminalView>,
    cx: &App,
) -> Option<(&'static AgentCli, &'static str)> {
    let terminal = view.read(cx).terminal().read(cx);
    if let Some(name) = terminal.foreground_process_command_name() {
        let base = command_basename(&name);
        if let Some(agent) = agent_for_basename(&base) {
            return Some((agent, invocation_bin(agent, &base)));
        }
    }
    if let Some(agent) = terminal
        .foreground_process_script_path()
        .as_deref()
        .and_then(agent_for_package_path)
    {
        return Some((agent, agent.bin));
    }
    agent_mentioned_in_text(&terminal.title(false))
        .or_else(|| agent_mentioned_in_text(&terminal.breadcrumb_text))
}

/// How long after the last output a terminal running an agent is still treated
/// as "working" rather than waiting for input.
const AGENT_WORKING_WINDOW: Duration = Duration::from_secs(5);

/// Delay after the last output before the screen-tail detector runs, so it sees
/// a settled frame instead of a half-drawn one.
const AGENT_DETECT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Re-scan cadence while an agent is working or blocked. Some state changes
/// (answering a dialog, a silent thinking pause) redraw the screen without
/// producing PTY output, so a slow poll keeps the badge honest.
const AGENT_DETECT_POLL: Duration = Duration::from_secs(1);

/// Snapshot of an agent terminal's state, rendered as a badge in the sidebar.
#[derive(Clone)]
struct AgentBadge {
    name: SharedString,
    working: bool,
    /// Whether the agent is waiting for the user to answer or approve
    /// something. Set from the screen-tail detector when it recognises a
    /// blocking dialog, otherwise from the quiet-window heuristic.
    waiting: bool,
    /// Accent used for the agent's icon and name while it is working.
    color: Color,
    /// Small icon that identifies the agent CLI.
    icon: IconName,
}

fn agent_display_name(agent: &AgentCli) -> SharedString {
    let label = match agent.id {
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "gemini" => "Gemini".to_string(),
        "cursor" => "Cursor".to_string(),
        "opencode" => "OpenCode".to_string(),
        "qoder" => "Qoder".to_string(),
        "qoder-cn" => "Qoder CN".to_string(),
        "qwen" => "Qwen".to_string(),
        "copilot" => "Copilot".to_string(),
        "antigravity" => "Antigravity".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    };
    SharedString::from(label)
}

/// Icon and color that tell agent CLIs apart at a glance.
fn agent_style(agent: &AgentCli) -> (Color, IconName) {
    match agent.id {
        "claude" => (Color::Warning, IconName::AiClaude),
        "codex" | "copilot" => (Color::Success, IconName::AiOpenAi),
        "gemini" => (Color::Info, IconName::AiGemini),
        "opencode" => (Color::Accent, IconName::AiOpenCode),
        "grok" => (Color::Default, IconName::AiXAi),
        "qwen" => (Color::Created, IconName::AiGoogle),
        "antigravity" => (Color::Info, IconName::AiGoogle),
        "cursor" | "aider" => (Color::Modified, IconName::Code),
        "kimi" => (Color::Accent, IconName::Star),
        _ => (Color::Accent, IconName::Sparkle),
    }
}

/// Whether an agent still looks busy even though it has been quiet for a
/// while. Two cheap signals are used: `OSC 9` progress reported by the
/// application, and spinner/busy markers in the terminal title.
fn agent_is_busy(view: &Entity<TerminalView>, cx: &App) -> bool {
    let view = view.read(cx);
    if view.notification_progress().is_some() {
        return true;
    }
    let terminal = view.terminal().read(cx);
    title_looks_busy(&terminal.title(false)) || title_looks_busy(&terminal.breadcrumb_text)
}

/// Heuristic busy markers used by coding-agent TUIs. Braille spinners are
/// matched by codepoint so any spinner frame counts.
fn title_looks_busy(text: &str) -> bool {
    if text
        .chars()
        .any(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
    {
        return true;
    }
    let lower = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "esc to interrupt",
        "esc to cancel",
        "thinking",
        "working",
        "generating",
        "processing",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Builds the sidebar agent badge for a terminal, if a coding agent is running.
fn agent_badge_for(
    view: &Entity<TerminalView>,
    terminal_activity: &HashMap<EntityId, std::time::Instant>,
    agent_waiting: &HashSet<EntityId>,
    agent_states: &HashMap<EntityId, AgentState>,
    cx: &App,
) -> Option<AgentBadge> {
    // Only trust the foreground process here: title/breadcrumb matching is fine
    // for a one-off "park this session" decision, but would light up the badge
    // for ordinary terminals whose title happens to mention an agent.
    let view_id = view.entity_id();
    let busy = agent_is_busy(view, cx);
    let view = view.read(cx);
    let terminal = view.terminal().read(cx);
    let agent = coding_agent_for_terminal(&terminal)?;
    // Screen-tail detection is authoritative when it has an answer; the
    // quiet-window heuristic only covers agents without rules and screens the
    // rules do not recognise.
    let (working, waiting) = match agent_states.get(&view_id) {
        Some(AgentState::Working) => (true, false),
        Some(AgentState::Blocked) => (false, true),
        Some(AgentState::Idle) => (false, false),
        None => {
            let working = terminal_activity
                .get(&view_id)
                .is_some_and(|last| last.elapsed() < AGENT_WORKING_WINDOW)
                || busy;
            (working, agent_waiting.contains(&view_id))
        }
    };
    let (color, icon) = agent_style(agent);
    Some(AgentBadge {
        name: agent_display_name(agent),
        working,
        waiting,
        color,
        icon,
    })
}

/// Branch and dirty marker for a terminal's working directory.
#[derive(Clone)]
struct GitBadge {
    branch: SharedString,
    dirty: usize,
}

/// State reported by an in-terminal agent integration.
#[derive(Clone)]
struct ReportedAgentState {
    state: AgentState,
    message: Option<String>,
}

/// Snapshot of an agent terminal for the agent switcher.
#[derive(Clone)]
pub struct AgentTerminalEntry {
    pub id: EntityId,
    pub title: SharedString,
    pub agent: SharedString,
    pub group: SharedString,
    pub working: bool,
    pub waiting: bool,
    pub icon: IconName,
    pub color: Color,
}

/// Tells the user that one or more agents are waiting for input: a system
/// notification plus a dock/window attention request.
fn notify_agents_waiting(views: &[Entity<TerminalView>], cx: &mut App) {
    let Some(first) = views.first() else {
        return;
    };
    let (agent_name, terminal_title) = {
        let view = first.read(cx);
        let terminal = view.terminal().read(cx);
        let agent_name = coding_agent_for_terminal(&terminal)
            .map(agent_display_name)
            .unwrap_or_else(|| SharedString::from(i18n::t("agent")));
        (agent_name, terminal.title(false))
    };

    let state = i18n::t("agent_waiting_for_input");
    let title = if views.len() > 1 {
        format!("{state} · {}", views.len())
    } else {
        format!("{agent_name} · {state}")
    };
    cx.show_system_notification(gpui::SystemNotification {
        tag: "terry-agent-waiting".into(),
        title: title.into(),
        body: terminal_title.trim().to_string().into(),
        actions: Vec::new(),
    });

    // Bounce the dock / flash the taskbar entry so the reminder is visible
    // even when notifications are muted.
    for handle in cx.windows() {
        handle
            .update(cx, |_, window, _| window.request_attention())
            .ok();
    }
}

/// Renders `[icon] Name ●` for an agent terminal. The dot breathes while the
/// agent is producing output and is muted when it is waiting for input.
fn agent_badge_view(id: SharedString, badge: &AgentBadge) -> AnyElement {
    let state = if badge.waiting {
        i18n::t("agent_waiting_for_input")
    } else if badge.working {
        i18n::t("agent_working")
    } else {
        i18n::t("agent_idle")
    };
    let tooltip = format!("{} · {}", badge.name, state);

    // The dot breathes while the agent is working, turns solid when it is
    // waiting for input, and is muted when idle.
    let dot: AnyElement = if badge.working {
        Label::new("●")
            .size(LabelSize::XSmall)
            .color(badge.color)
            .with_animation(
                SharedString::from(id.to_string() + "-pulse"),
                Animation::new(Duration::from_secs(2))
                    .repeat()
                    .with_easing(pulsating_between(0.35, 1.0)),
                |label, delta| label.alpha(delta),
            )
            .into_any_element()
    } else if badge.waiting {
        Label::new("●")
            .size(LabelSize::XSmall)
            .color(badge.color)
            .into_any_element()
    } else {
        Label::new("●")
            .size(LabelSize::XSmall)
            .color(Color::Muted)
            .into_any_element()
    };

    h_flex()
        .id(id)
        .gap_1()
        .items_center()
        .tooltip(Tooltip::text(tooltip))
        .child(
            ui::Icon::new(badge.icon)
                .size(IconSize::Small)
                .color(badge.color),
        )
        .child(
            Label::new(badge.name.clone())
                .size(LabelSize::XSmall)
                .color(if badge.working {
                    badge.color
                } else {
                    Color::Muted
                }),
        )
        .child(dot)
        .into_any_element()
}

fn should_park_agent_session(view: &Entity<TerminalView>, cx: &App) -> bool {
    let terminal = view.read(cx).terminal().read(cx);
    if terminal.has_exited() {
        return false;
    }
    if coding_agent_for_terminal(&terminal).is_some() {
        return true;
    }
    text_mentions_coding_agent(&terminal.title(false))
        || text_mentions_coding_agent(&terminal.breadcrumb_text)
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct PersistedTerminal {
    cwd: Option<PathBuf>,
    /// User-renamed tab/sidebar title. Optional for older session files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Theme name from the launch profile that created this terminal, so a
    /// restored terminal keeps its per-profile colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color_scheme: Option<String>,
    /// Per-terminal font size from the zoom actions, so a zoomed terminal keeps
    /// its size across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    font_size: Option<f32>,
    /// How this terminal was launched (profile name or a raw command such as
    /// `ssh`/`tmux`), so restore can replay it instead of opening a plain shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch: Option<PersistedLaunch>,
    /// Daemon session backing this terminal when persistent sessions are on.
    /// Reusing the id reattaches to the running process instead of starting a
    /// new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daemon_session: Option<String>,
    /// Agent session reported by an integration, so restore can resume the
    /// exact conversation instead of starting a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_session: Option<PersistedAgentSession>,
}

/// A resumable agent session reported through `report_agent_session`.
#[derive(Clone, Serialize, Deserialize, Debug)]
struct PersistedAgentSession {
    agent: String,
    value: String,
}

/// Replayable launch information for a persisted terminal.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum PersistedLaunch {
    /// A named profile from `terminal.profiles`.
    Profile { name: String },
    /// A direct program invocation (SSH, tmux, ...).
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// Restore hints passed through the async spawn path. Indexed separately from
/// the spawn arguments because async completion can reorder terminals.
#[derive(Clone, Default)]
struct SpawnRestore {
    index: Option<usize>,
    title: Option<String>,
    color_scheme: Option<String>,
    font_size: Option<f32>,
    launch: Option<PersistedLaunch>,
    daemon_session: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PersistedAxis {
    Horizontal,
    Vertical,
}

impl From<Axis> for PersistedAxis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::Horizontal => Self::Horizontal,
            Axis::Vertical => Self::Vertical,
        }
    }
}

impl From<PersistedAxis> for Axis {
    fn from(axis: PersistedAxis) -> Self {
        match axis {
            PersistedAxis::Horizontal => Self::Horizontal,
            PersistedAxis::Vertical => Self::Vertical,
        }
    }
}

/// Split layout for a terminal group, keyed by index into `group.terminals`
/// so it survives app restarts (EntityIds do not).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GroupLayoutNode {
    Pane {
        terminals: Vec<usize>,
        #[serde(default)]
        active: Option<usize>,
    },
    Split {
        axis: PersistedAxis,
        flexes: Vec<f32>,
        children: Vec<GroupLayoutNode>,
    },
}

/// Auto-tiling layout mode for a group's terminals, mirroring kitty's window
/// layouts. `Manual` keeps user-managed splits; the others regenerate the
/// split tree from the group's terminal list whenever it changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GroupLayoutMode {
    /// User-managed splits (the default).
    #[default]
    Manual,
    /// First terminal full-height on the left; the rest stacked on the right.
    Tall,
    /// Terminals arranged in an even grid.
    Grid,
    /// All terminals maximized, one visible at a time (tab-like).
    Stack,
    /// First terminal full-width on top; the rest side by side below.
    Fat,
    /// All terminals arranged in a single row.
    Horizontal,
    /// All terminals arranged in a single column.
    Vertical,
}

impl GroupLayoutMode {
    fn label(self) -> String {
        match self {
            GroupLayoutMode::Manual => i18n::t("layout_manual"),
            GroupLayoutMode::Tall => i18n::t("layout_tall"),
            GroupLayoutMode::Grid => i18n::t("layout_grid"),
            GroupLayoutMode::Stack => i18n::t("layout_stack"),
            GroupLayoutMode::Fat => i18n::t("layout_fat"),
            GroupLayoutMode::Horizontal => i18n::t("layout_horizontal"),
            GroupLayoutMode::Vertical => i18n::t("layout_vertical"),
        }
    }

    fn api_name(self) -> &'static str {
        match self {
            GroupLayoutMode::Manual => "manual",
            GroupLayoutMode::Tall => "tall",
            GroupLayoutMode::Grid => "grid",
            GroupLayoutMode::Stack => "stack",
            GroupLayoutMode::Fat => "fat",
            GroupLayoutMode::Horizontal => "horizontal",
            GroupLayoutMode::Vertical => "vertical",
        }
    }

    /// Generate the split tree for `count` terminals under this layout mode.
    fn generate_layout(self, count: usize) -> Option<GroupLayoutNode> {
        if count == 0 {
            return None;
        }
        let pane = |i: usize| GroupLayoutNode::Pane {
            terminals: vec![i],
            active: Some(i),
        };
        match self {
            GroupLayoutMode::Manual => None,
            GroupLayoutMode::Stack => Some(GroupLayoutNode::Pane {
                terminals: (0..count).collect(),
                active: Some(count - 1),
            }),
            GroupLayoutMode::Horizontal => {
                if count == 1 {
                    return Some(pane(0));
                }
                Some(GroupLayoutNode::Split {
                    axis: PersistedAxis::Horizontal,
                    flexes: vec![1.0; count],
                    children: (0..count).map(pane).collect(),
                })
            }
            GroupLayoutMode::Vertical => {
                if count == 1 {
                    return Some(pane(0));
                }
                Some(GroupLayoutNode::Split {
                    axis: PersistedAxis::Vertical,
                    flexes: vec![1.0; count],
                    children: (0..count).map(pane).collect(),
                })
            }
            GroupLayoutMode::Fat => {
                if count == 1 {
                    return Some(pane(0));
                }
                let bottom = if count == 2 {
                    pane(1)
                } else {
                    GroupLayoutNode::Split {
                        axis: PersistedAxis::Horizontal,
                        flexes: vec![1.0; count - 1],
                        children: (1..count).map(pane).collect(),
                    }
                };
                Some(GroupLayoutNode::Split {
                    axis: PersistedAxis::Vertical,
                    flexes: vec![0.5, 0.5],
                    children: vec![pane(0), bottom],
                })
            }
            GroupLayoutMode::Tall => {
                if count == 1 {
                    return Some(pane(0));
                }
                let right = if count == 2 {
                    pane(1)
                } else {
                    GroupLayoutNode::Split {
                        axis: PersistedAxis::Vertical,
                        flexes: vec![1.0; count - 1],
                        children: (1..count).map(pane).collect(),
                    }
                };
                Some(GroupLayoutNode::Split {
                    axis: PersistedAxis::Horizontal,
                    flexes: vec![0.5, 0.5],
                    children: vec![pane(0), right],
                })
            }
            GroupLayoutMode::Grid => {
                if count == 1 {
                    return Some(pane(0));
                }
                // Rows stacked vertically; each row is a horizontal split.
                let cols = (count as f32).sqrt().ceil() as usize;
                let rows = count.div_ceil(cols);
                let mut rows_nodes = Vec::new();
                let mut next = 0usize;
                for _ in 0..rows {
                    let row_count = cols.min(count - next);
                    let mut panes = Vec::with_capacity(row_count);
                    for _ in 0..row_count {
                        panes.push(pane(next));
                        next += 1;
                    }
                    let node = if row_count == 1 {
                        panes.into_iter().next().unwrap()
                    } else {
                        GroupLayoutNode::Split {
                            axis: PersistedAxis::Horizontal,
                            flexes: vec![1.0; row_count],
                            children: panes,
                        }
                    };
                    rows_nodes.push(node);
                }
                let node = if rows == 1 {
                    rows_nodes.into_iter().next().unwrap()
                } else {
                    GroupLayoutNode::Split {
                        axis: PersistedAxis::Vertical,
                        flexes: vec![1.0; rows],
                        children: rows_nodes,
                    }
                };
                Some(node)
            }
        }
    }
}

/// A saved workspace layout that can be re-created as a new group.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkspaceTemplate {
    name: String,
    terminals: Vec<PersistedTerminal>,
    #[serde(default)]
    layout: Option<GroupLayoutNode>,
    #[serde(default)]
    layout_mode: Option<GroupLayoutMode>,
    #[serde(default)]
    broadcast_input: bool,
}

/// Converts a parsed tmux layout into a group layout tree. Leaves resolve to
/// the terminal index that mirrors the pane; `fallback` assigns indices in
/// traversal order when the layout does not carry pane ids.
fn tmux_layout_to_node(
    layout: &crate::tmux_control::Layout,
    pane_order: &[u32],
    fallback: &mut usize,
) -> Option<GroupLayoutNode> {
    use crate::tmux_control::{Layout, Orientation};

    let child_extent = |orientation: Orientation, child: &Layout| -> f32 {
        match (orientation, child) {
            (Orientation::LeftRight, Layout::Leaf { width, .. })
            | (Orientation::LeftRight, Layout::Split { width, .. }) => *width as f32,
            (Orientation::TopBottom, Layout::Leaf { height, .. })
            | (Orientation::TopBottom, Layout::Split { height, .. }) => *height as f32,
        }
    };

    match layout {
        Layout::Leaf { pane, .. } => {
            let index = pane
                .and_then(|pane| pane_order.iter().position(|id| *id == pane))
                .unwrap_or_else(|| {
                    let index = *fallback;
                    *fallback += 1;
                    index
                });
            Some(GroupLayoutNode::Pane {
                terminals: vec![index],
                active: None,
            })
        }
        Layout::Split {
            orientation,
            children,
            ..
        } => {
            let axis = match orientation {
                Orientation::LeftRight => PersistedAxis::Horizontal,
                Orientation::TopBottom => PersistedAxis::Vertical,
            };
            let mut nodes = Vec::with_capacity(children.len());
            let mut flexes = Vec::with_capacity(children.len());
            for child in children {
                nodes.push(tmux_layout_to_node(child, pane_order, fallback)?);
                flexes.push(child_extent(*orientation, child));
            }
            if nodes.len() < 2 {
                return nodes.pop();
            }
            Some(GroupLayoutNode::Split {
                axis,
                flexes,
                children: nodes,
            })
        }
    }
}

fn templates_path() -> std::path::PathBuf {
    paths::data_dir().join("workspace_templates.json")
}

fn load_templates() -> Vec<WorkspaceTemplate> {
    let Ok(text) = std::fs::read_to_string(templates_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_templates(templates: &[WorkspaceTemplate]) {
    let path = templates_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(templates) {
        Ok(text) => {
            if let Err(error) = std::fs::write(&path, text) {
                log::error!(
                    "workspace templates: failed to write {}: {error}",
                    path.display()
                );
            }
        }
        Err(error) => log::error!("workspace templates: failed to serialize: {error}"),
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedGroup {
    name: String,
    collapsed: bool,
    terminals: Vec<PersistedTerminal>,
    #[serde(default)]
    layout: Option<GroupLayoutNode>,
    #[serde(default)]
    layout_mode: Option<GroupLayoutMode>,
    /// Older session files lack this key; defaults to false.
    #[serde(default)]
    broadcast_input: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedSession {
    groups: Vec<PersistedGroup>,
    active_group_index: usize,
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<TerminalListPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &NewTerminal, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            // Must use window.defer (not cx.defer_in): defer_in would re-enter a
            // Workspace update, and new_terminal reads workspace for the active pane.
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            });
        });
        workspace.register_action(|workspace, _: &ReopenClosedTerminal, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                let reopened =
                    panel.update(cx, |panel, cx| panel.reopen_closed_terminal(window, cx));
                if !reopened {
                    window.dispatch_action(ReopenClosedItem.boxed_clone(), cx);
                }
            });
        });
        workspace.register_action(|workspace, _: &NewGroup, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.create_group(window, cx));
            });
        });
        workspace.register_action(|workspace, _: &NextGroup, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.switch_group_offset(1, window, cx));
            });
        });
        workspace.register_action(|workspace, _: &PreviousGroup, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.switch_group_offset(-1, window, cx));
            });
        });
        workspace.register_action(|workspace, _: &ConnectSshHost, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.open_ssh_picker(false, window, cx));
            });
        });
        workspace.register_action(|workspace, _: &ConnectSshHostWithTmux, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.open_ssh_picker(true, window, cx));
            });
        });
        workspace.register_action(|workspace, _: &AttachTmuxSession, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.open_tmux_picker(window, cx));
            });
        });
        workspace.register_action(
            |workspace, _: &crate::command_history::OpenCommandHistory, _window, cx| {
                let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                    return;
                };
                crate::command_history::open(Some(panel.downgrade()), cx);
            },
        );
        workspace.register_action(
            |workspace, _: &crate::scrollback_export::CopyScrollback, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::scrollback_export::copy(&view, cx);
                    });
                }
            },
        );
        workspace.register_action(
            |workspace, _: &crate::scrollback_export::CopyScrollbackAsMarkdown, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::scrollback_export::copy_markdown(&view, cx);
                    });
                }
            },
        );
        workspace.register_action(
            |workspace, _: &crate::scrollback_export::ScrollbackToClipboardHistory, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::scrollback_export::add_to_history(&view, cx);
                    });
                }
            },
        );
        workspace.register_action(
            |workspace, _: &crate::scrollback_export::SaveScrollbackToFile, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::scrollback_export::save_to_file(&view, cx);
                    });
                }
            },
        );
        workspace.register_action(|workspace, _: &FocusNextWaitingAgent, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.focus_next_waiting_agent(window, cx);
                });
            });
        });
        workspace.register_action(
            |workspace, _: &crate::session_recording::ToggleSessionRecording, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::session_recording::toggle(&view, cx);
                    });
                }
            },
        );
        workspace.register_action(
            |workspace, _: &crate::agent_switcher::OpenAgentSwitcher, _window, cx| {
                let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                    return;
                };
                crate::agent_switcher::open(panel.downgrade(), cx);
            },
        );
        workspace.register_action(|workspace, _: &ReviewAgentChanges, window, cx| {
            git_ui::project_diff::ProjectDiff::deploy_at(workspace, None, window, cx);
        });
        workspace.register_action(
            |workspace, _: &crate::tmux_native::AttachTmuxControlMode, window, cx| {
                let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                    return;
                };
                window.defer(cx, move |window, cx| {
                    panel.update(cx, |_, cx| {
                        crate::tmux_native::attach("terry".to_string(), window, cx);
                    });
                });
            },
        );
        workspace.register_action(
            |workspace, _: &crate::process_inspector::OpenProcessInspector, window, cx| {
                if let Some(view) = crate::scrollback_export::active_terminal(workspace, cx) {
                    window.defer(cx, move |_window, cx| {
                        crate::process_inspector::open(view.downgrade(), cx);
                    });
                }
            },
        );
        workspace.register_action(|workspace, _: &SelectTerminalColorScheme, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.open_color_scheme_picker(window, cx));
            });
        });
        workspace.register_action(|workspace, _: &ImportTerminalColorScheme, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.import_color_scheme(window, cx));
            });
        });
        workspace.register_action(|workspace, action: &NewTerminalWithProfile, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            let name = action.0.clone();
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| {
                    let profile = TerminalSettings::get_global(cx).profile(&name).cloned();
                    panel.new_terminal_with_profile(profile, window, cx);
                });
            });
        });
        workspace.register_action(
            |workspace, _: &terminal_view::ToggleBroadcastInput, window, cx| {
                let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                    return;
                };
                window.defer(cx, move |window, cx| {
                    panel.update(cx, |panel, cx| {
                        let group_id = panel.active_group_id;
                        panel.toggle_broadcast_input(group_id, window, cx);
                    });
                });
            },
        );
        // Welcome page and default keymaps dispatch workspace::NewTerminal.
        // Terry has no TerminalPanel dock, so that handler no-ops — route here.
        // Registered before terminal_view::init so we run first in the bubble phase.
        workspace.register_action(|workspace, _: &workspace::NewTerminal, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                cx.propagate();
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            });
        });
    })
    .detach();
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GroupId(usize);

/// A named collection of terminals. Selecting a group shows its terminals as
/// tabs in the center pane.
struct TerminalGroup {
    id: GroupId,
    name: SharedString,
    terminals: Vec<Entity<TerminalView>>,
    collapsed: bool,
    #[allow(dead_code)]
    pub has_unread: bool,
    /// Last known center split layout while this group was active.
    saved_layout: Option<GroupLayoutNode>,
    /// Auto-tiling layout mode. `Manual` keeps user-managed splits.
    layout_mode: GroupLayoutMode,
    /// When on, keystrokes accepted by any terminal in the group are relayed
    /// to every other terminal in it (broadcast input).
    broadcast_input: bool,
    /// How many terminals this group had in the session file (restore waits).
    session_terminal_count: Option<usize>,
    /// Session restore slots keyed by persisted index so async spawn completion
    /// order cannot scramble layout indices / titles.
    restore_slots: Vec<Option<Entity<TerminalView>>>,
}

pub struct TerminalListPanel {
    workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    focus_handle: FocusHandle,
    position: DockPosition,
    _workspace_subscription: Subscription,
    _project_subscription: Subscription,
    _quit_subscription: Subscription,
    /// The center pane that displays the active group's terminals as tabs.
    display_pane: WeakEntity<Pane>,
    /// Pane that should receive the next newly spawned terminal (e.g. where
    /// "+" was clicked). Cleared when consumed by sync.
    pending_spawn_pane: Option<WeakEntity<Pane>>,
    /// Terminal to activate after a cross-group switch finishes restoring
    /// layout. Set by sidebar clicks; applied (and cleared) by sync.
    pending_focus_terminal: Option<EntityId>,
    groups: Vec<TerminalGroup>,
    active_group_id: GroupId,
    next_group_id: usize,
    /// Guards against reconciling the model while a group switch is
    /// transiently removing/adding items in the display pane.
    switching: bool,
    /// Depth of in-flight center-layout rebuilds. Workspace events
    /// (ItemRemoved/ItemAdded/PaneAdded) are dispatched asynchronously after
    /// the rebuild's update stack unwinds — by then the `switching` flag set
    /// by callers has already been cleared. A positive depth suppresses those
    /// events until the rebuild's deferred decrement runs.
    reflowing_layout_depth: usize,
    /// Outgoing group whose live split layout must be captured before any
    /// sync may clear its terminals from the center panes.
    pending_switch_from: Option<GroupId>,
    /// Apply `saved_layout` once after session restore when terminals are ready.
    pending_layout_restore: bool,
    /// True while session terminals are still spawning. Blocks disk writes and
    /// auto-spawn (worktree / orphan claim) so we neither wipe groups to
    /// `terminals: []` nor inflate counts mid-restore.
    session_restoring: bool,
    /// Active group has already been shown; remaining groups may still spawn.
    session_active_revealed: bool,
    /// Ensures [`Self::finish_session_restore`] is deferred at most once.
    session_finish_scheduled: bool,
    /// Ensures [`Self::reveal_active_group_during_restore`] is deferred at most once.
    session_reveal_scheduled: bool,
    /// Stable key for this workspace (db id / project paths / "default").
    session_workspace_key: String,
    /// File stem under `sessions/` — workspace key for the owner, or
    /// `{key}-w{panel_id}` for secondary windows.
    session_file_stem: String,
    /// True when this panel owns the shared restore file for `session_workspace_key`.
    owns_workspace_session: bool,
    /// Workspace DB id this panel was created with, if it was known. Kept so a
    /// later worktree load can migrate an empty-window session key to the
    /// project key even when the workspace itself is not readable here.
    session_database_id: Option<workspace::WorkspaceId>,
    /// Set once the empty-window key was migrated (or found unnecessary), so a
    /// later worktree never renames the session file again.
    session_key_migrated: bool,
    /// Spawn-time / last-known cwd per terminal view. Filled with the directory
    /// the shell was actually started in (after defaulting), and refreshed from
    /// live process info so session writes don't permanently store `null`.
    terminal_spawn_cwds: HashMap<EntityId, Option<PathBuf>>,
    /// Launch origin per terminal view (profile or direct command), persisted so
    /// restore can replay `ssh`/`tmux` terminals instead of opening a shell.
    terminal_launches: HashMap<EntityId, PersistedLaunch>,
    /// Daemon session backing each terminal when persistent sessions are on.
    terminal_daemon_sessions: HashMap<EntityId, String>,
    /// Subscription objects for each terminal view keyed by its entity id.
    /// Removed when the terminal is closed so long sessions do not accumulate
    /// dead subscriptions.
    _terminal_subscriptions: HashMap<EntityId, Vec<Subscription>>,
    /// Timestamp of the most recent output per terminal, used to decide whether
    /// the sidebar's agent badge should show as working or idle.
    terminal_activity: HashMap<EntityId, std::time::Instant>,
    /// Terminals whose agent went quiet after producing output, i.e. are most
    /// likely waiting for user input.
    agent_waiting: HashSet<EntityId>,
    /// Attempt counter for automatic remote reconnects, keyed by launch.
    remote_reconnects: HashMap<String, (u32, std::time::Instant)>,
    /// Mapping from tmux window ids to the group mirroring that window.
    tmux_window_groups: HashMap<u32, GroupId>,
    /// Terminal views created by the tmux control-mode bridge, so they can be
    /// dropped before reconnecting.
    tmux_mirrored: HashSet<EntityId>,
    /// Repository subscriptions, so git changes refresh the sidebar.
    git_subscriptions: Vec<Subscription>,
    git_subscribed: HashSet<EntityId>,
    _git_store_subscription: Subscription,
    /// Re-renders the sidebar when a terminal starts or stops recording.
    _recording_observation: Subscription,
    /// Pending refresh that lets the agent badge fall back to idle once output
    /// stops, so the panel does not re-render continuously.
    agent_idle_refresh: Option<Task<()>>,
    /// Screen-tail detection result per terminal. Absent when the terminal is
    /// not running an agent with rules, or when no rule matched and the coarse
    /// quiet-window heuristic should apply instead.
    agent_states: HashMap<EntityId, AgentState>,
    /// State reported by an in-terminal agent integration (for example the Pi
    /// extension). Authoritative over screen detection.
    reported_agent_states: HashMap<EntityId, ReportedAgentState>,
    /// `TERRY_TERMINAL_TOKEN` value per terminal, so integration reports can be
    /// attributed to the right terminal.
    terminal_tokens: HashMap<String, EntityId>,
    /// Resumable agent session per terminal, reported by an integration.
    agent_session_refs: HashMap<EntityId, PersistedAgentSession>,
    /// Debounced re-scan of the terminal bottom after output settles.
    agent_detect_task: Option<Task<()>>,
    /// Debounced disk write after a terminal cwd change.
    cwd_persist_task: Task<()>,
    /// Debounced disk write after the command history changes.
    command_history_persist: Task<()>,
    /// After session restore, ignore process-info reports of `$HOME` that would
    /// clobber a more specific restored path while shell rc finishes.
    restore_cwd_guard_until: Option<std::time::Instant>,
    /// Coding-agent terminals closed by the user. The PTY stays alive so the
    /// next new terminal can reattach instead of starting a fresh shell.
    parked_agent_sessions: Vec<ParkedAgentSession>,
    /// Terminals closed by the user, most recent last, so they can be reopened.
    recently_closed: Vec<ClosedTerminal>,
    /// Set while removals must not populate [`Self::recently_closed`] (tab moved
    /// to another window, tmux mirror rebuilt).
    suppress_recently_closed: bool,
    /// Live terminals removed from every group but still running. They are not
    /// persisted: quitting the app releases their PTYs like any other view.
    detached_sessions: Vec<DetachedSession>,
    /// Transient "index/total · name" hint shown after a group shortcut.
    group_switch_hint: Option<(SharedString, u64)>,
    /// Monotonic token so an older timer cannot clear a newer hint.
    switch_hint_generation: u64,
    /// Agent and directory remembered at close time, so a later terminal can
    /// run the CLI's resume command after the process has exited.
    remembered_agent_sessions: Vec<RememberedAgentSession>,
    _session_release_subscription: Subscription,
    renaming_group_id: Option<GroupId>,
    rename_editor: Entity<Editor>,
}

impl TerminalListPanel {
    pub fn new(
        workspace: Entity<Workspace>,
        display_pane: Entity<Pane>,
        project: Entity<Project>,
        database_id: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // Reap daemon sessions nothing references. Runs once per process, after
        // a delay so window restore has a chance to reattach first.
        if persistent_sessions::is_enabled(cx) && persistent_sessions::claim_startup_prune() {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(60));
                let referenced = persistent_sessions::referenced_sessions();
                match persistent_sessions::prune(&referenced, 3600, false) {
                    Ok(pruned) if !pruned.is_empty() => {
                        log::info!("pruned {} orphaned daemon session(s)", pruned.len());
                    }
                    Ok(_) => {}
                    Err(error) => log::debug!("daemon prune failed: {error}"),
                }
            });
        }
        // subscribe_in so ItemAdded/ItemRemoved reconciliation can update the pane.
        let _workspace_subscription =
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| {
                this.on_workspace_event(event, window, cx);
            });
        // Use subscribe_in so WorktreeAdded always has the panel's window —
        // cx.active_window() is often None during async project open.
        let _project_subscription =
            cx.subscribe_in(&project, window, |this, _project, event, window, cx| {
                if let project::Event::WorktreeAdded(id) = event {
                    this.on_worktree_added(*id, window, cx);
                }
            });

        let rename_editor = cx.new(|cx| Editor::single_line(window, cx));
        cx.subscribe(&rename_editor, |this, _editor, event, cx| {
            if let editor::EditorEvent::Blurred = event {
                this.commit_rename(cx);
            }
        })
        .detach();

        let _quit_subscription = cx.on_app_quit(|this, cx| {
            // Skip layout capture on quit — Workspace may already be tearing
            // down and nested reads can panic, which would skip the cwd write.
            // Prefer remembered paths: process-info often reports `$HOME` while
            // the app is shutting down, which used to wipe restored project cwds.
            this.write_session_file(cx, true);
            async {}
        });

        let panel_id = cx.entity_id();
        // Must not `workspace.read(cx)` here — `new` often runs inside a
        // Workspace update (observe_new / init_workspace).
        let session_workspace_key = Self::compute_workspace_session_key(&project, database_id, cx);
        let owns_workspace_session = try_claim_session(&session_workspace_key, panel_id);
        let session_file_stem = if owns_workspace_session {
            session_workspace_key.clone()
        } else {
            format!("{}-w{}", session_workspace_key, panel_id.as_u64())
        };

        let _session_release_subscription = cx.on_release(move |this, _cx| {
            if this.owns_workspace_session {
                release_session_claim(&this.session_workspace_key, panel_id);
            }
        });

        // Refresh the sidebar when git status or the checked-out branch changes.
        let _recording_observation =
            cx.observe_global::<crate::session_recording::SessionRecording>(|_, cx| cx.notify());
        let _git_store_subscription = {
            let git_store = project.read(cx).git_store().clone();
            cx.subscribe(
                &git_store,
                |this, _, _: &project::git_store::GitStoreEvent, cx| {
                    this.sync_git_subscriptions(cx);
                    cx.notify();
                },
            )
        };
        let mut git_subscriptions: Vec<Subscription> = Vec::new();
        let mut git_subscribed: HashSet<EntityId> = HashSet::default();
        let repositories: Vec<_> = project
            .read(cx)
            .repositories(cx)
            .values()
            .cloned()
            .collect();
        for repository in repositories {
            git_subscribed.insert(repository.entity_id());
            git_subscriptions.push(cx.subscribe(
                &repository,
                |_, _, event: &project::git_store::RepositoryEvent, cx| {
                    if matches!(
                        event,
                        project::git_store::RepositoryEvent::StatusesChanged
                            | project::git_store::RepositoryEvent::HeadChanged
                    ) {
                        cx.notify();
                    }
                },
            ));
        }

        Self {
            workspace: workspace.downgrade(),
            project: project.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            _workspace_subscription,
            _project_subscription,
            _quit_subscription,
            display_pane: display_pane.downgrade(),
            pending_spawn_pane: None,
            pending_focus_terminal: None,
            groups: Vec::new(),
            active_group_id: GroupId(0),
            next_group_id: 0,
            switching: false,
            reflowing_layout_depth: 0,
            pending_switch_from: None,
            pending_layout_restore: false,
            session_restoring: false,
            session_active_revealed: false,
            session_finish_scheduled: false,
            session_reveal_scheduled: false,
            session_workspace_key,
            session_file_stem,
            owns_workspace_session,
            session_database_id: database_id,
            session_key_migrated: false,
            terminal_spawn_cwds: HashMap::default(),
            terminal_launches: HashMap::default(),
            terminal_daemon_sessions: HashMap::default(),
            _terminal_subscriptions: HashMap::default(),
            terminal_activity: HashMap::default(),
            agent_waiting: HashSet::default(),
            remote_reconnects: HashMap::default(),
            tmux_window_groups: HashMap::default(),
            tmux_mirrored: HashSet::default(),
            git_subscriptions,
            git_subscribed,
            _git_store_subscription,
            _recording_observation,
            agent_idle_refresh: None,
            agent_states: HashMap::default(),
            reported_agent_states: HashMap::default(),
            terminal_tokens: HashMap::default(),
            agent_session_refs: HashMap::default(),
            agent_detect_task: None,
            cwd_persist_task: Task::ready(()),
            command_history_persist: Task::ready(()),
            restore_cwd_guard_until: None,
            parked_agent_sessions: Vec::new(),
            recently_closed: Vec::new(),
            suppress_recently_closed: false,
            detached_sessions: Vec::new(),
            group_switch_hint: None,
            switch_hint_generation: 0,
            remembered_agent_sessions: Vec::new(),
            _session_release_subscription,
            renaming_group_id: None,
            rename_editor,
        }
    }

    /// Creates the initial group (with one terminal) if none exists yet.

    fn compute_workspace_session_key(
        project: &Entity<Project>,
        database_id: Option<workspace::WorkspaceId>,
        cx: &App,
    ) -> String {
        let roots = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .collect();
        Self::session_key_for(database_id, roots)
    }

    /// Session file key for a workspace.
    ///
    /// Empty windows (the welcome window, or a window whose worktrees have not
    /// loaded yet) must not be keyed by the workspace DB id: unlike project
    /// workspaces, which the DB matches by their paths and therefore reuse the
    /// same id, an empty workspace is not addressable by paths and gets a brand
    /// new row id on every launch. Keying by that id made the panel look for
    /// `ws-<new id>.json` each time and made persistent sessions appear lost on
    /// every restart. They use the stable `default` key instead; once a
    /// worktree arrives the key is migrated to the project key (see
    /// [`Self::migrate_session_key`]).
    ///
    /// When roots are known, the workspace id wins over hashing them so the
    /// same project maps to the same file whether it was restored directly or
    /// opened inside an empty window. Without a database id the key falls back
    /// to the worktree roots, and to `default` for the empty welcome window.
    /// Secondary windows are isolated via ownership (they get a `-w{id}` file
    /// stem).
    fn session_key_for(database_id: Option<workspace::WorkspaceId>, roots: Vec<String>) -> String {
        let mut roots = roots;
        roots.sort();
        roots.dedup();
        if roots.is_empty() {
            return "default".to_string();
        }
        if let Some(id) = database_id {
            return format!("ws-{}", i64::from(id));
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        roots.hash(&mut hasher);
        format!("paths-{:x}", hasher.finish())
    }

    fn session_file_path(&self) -> PathBuf {
        paths::data_dir()
            .join("sessions")
            .join(format!("{}.json", self.session_file_stem))
    }

    /// Moves the session file of an empty window to the project key once its
    /// first worktree loads.
    ///
    /// A window created empty (the welcome window, or a project whose worktrees
    /// had not loaded yet) uses the stable `default` key. Opening a folder in
    /// that window turns it into a project workspace, and the next launch
    /// restores it through `Workspace::new_local`, which keys it by the
    /// workspace id. Renaming the file here keeps the terminals (and their
    /// daemon sessions) attached across that transition instead of starting
    /// fresh shells on the next launch.
    fn migrate_session_key(&mut self, cx: &mut Context<Self>) {
        if self.session_key_migrated || !self.owns_workspace_session {
            return;
        }
        let Some(project) = self.project.upgrade() else {
            return;
        };
        let roots: Vec<String> = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .collect();
        if roots.is_empty() {
            return;
        }
        self.session_key_migrated = true;

        // Use the id captured at construction: reading the Workspace from a
        // worktree event could re-enter a Workspace update. Every workspace
        // the DB will restore was created with its id already assigned.
        let new_key = Self::session_key_for(self.session_database_id, roots);
        if new_key == self.session_workspace_key {
            return;
        }
        // Only the empty-window fallback is provisional; a project key is final.
        if self.session_workspace_key != "default" {
            return;
        }

        let old_path = self.session_file_path();
        let panel_id = cx.entity_id();
        release_session_claim(&self.session_workspace_key, panel_id);
        self.owns_workspace_session = try_claim_session(&new_key, panel_id);
        self.session_workspace_key = new_key;
        self.session_file_stem = if self.owns_workspace_session {
            self.session_workspace_key.clone()
        } else {
            format!("{}-w{}", self.session_workspace_key, panel_id.as_u64())
        };

        let new_path = self.session_file_path();
        if old_path != new_path && old_path.exists() && !new_path.exists() {
            if let Some(parent) = new_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(&old_path, &new_path);
        }
    }

    pub fn save_session(&mut self, cx: &App) {
        // Persist in-memory state only. Do not capture live layout here —
        // save_session is often called from workspace actions where
        // workspace.read would panic (nested entity lease).
        self.write_session_file(cx, false);
    }

    /// Capture the active group's live split layout, then write the session.
    /// Only call when Workspace is not already being updated.
    fn refresh_active_layout_and_save(&mut self, cx: &App) {
        if self.session_restoring {
            return;
        }
        // Auto-tiling groups derive their layout from the terminal list; a
        // stale capture here (the list can be mid-reflow) would overwrite
        // saved_layout with a mismatched tree and re-trigger the rebuild.
        let is_auto = self
            .groups
            .iter()
            .find(|g| g.id == self.active_group_id)
            .is_some_and(|g| g.layout_mode != GroupLayoutMode::Manual);
        if is_auto {
            self.write_session_file(cx, false);
            return;
        }
        if let Some(layout) = self.capture_active_group_layout(cx) {
            if let Some(group) = self
                .groups
                .iter_mut()
                .find(|g| g.id == self.active_group_id)
            {
                group.saved_layout = Some(layout);
            }
        }
        self.write_session_file(cx, false);
    }

    fn schedule_cwd_persist(&mut self, cx: &mut Context<Self>) {
        if self.session_restoring {
            return;
        }
        self.cwd_persist_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            this.update(cx, |this, cx| {
                this.write_session_file(cx, false);
            })
            .ok();
        });
    }

    /// Schedules the check that turns quiet agent terminals into "waiting for
    /// input". No-op when a check is already pending.
    fn ensure_agent_idle_refresh(&mut self, cx: &mut Context<Self>) {
        if self.agent_idle_refresh.is_some() {
            return;
        }
        self.agent_idle_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AGENT_WORKING_WINDOW).await;
            this.update(cx, |this, cx| {
                this.agent_idle_refresh = None;
                let now = std::time::Instant::now();
                let expired: Vec<EntityId> = this
                    .terminal_activity
                    .iter()
                    .filter(|(_, last)| now.duration_since(**last) >= AGENT_WORKING_WINDOW)
                    .map(|(id, _)| *id)
                    .collect();
                for id in &expired {
                    this.terminal_activity.remove(id);
                }
                if expired.is_empty() {
                    return;
                }
                // An agent that stops producing output is usually waiting for
                // input, but it may also be thinking. Skip terminals that still
                // report progress or advertise a busy state in their title.
                let mut newly_waiting = Vec::new();
                let mut skipped_busy = false;
                for id in expired {
                    if this.agent_waiting.contains(&id) {
                        continue;
                    }
                    // The screen-tail detector already classified this
                    // terminal; the quiet-window heuristic is only a fallback.
                    if this.agent_states.contains_key(&id) {
                        continue;
                    }
                    let Some(view) = this.view_by_id_any(id) else {
                        continue;
                    };
                    if agent_badge_for(
                        &view,
                        &this.terminal_activity,
                        &this.agent_waiting,
                        &this.agent_states,
                        cx,
                    )
                    .is_none()
                    {
                        continue;
                    }
                    if agent_is_busy(&view, cx) {
                        skipped_busy = true;
                        continue;
                    }
                    this.agent_waiting.insert(id);
                    newly_waiting.push(view);
                }
                if !newly_waiting.is_empty() {
                    cx.notify();
                    notify_agents_waiting(&newly_waiting, cx);
                }
                if skipped_busy {
                    // Re-check later; the agent may finish thinking and become
                    // idle without producing any further output.
                    this.ensure_agent_idle_refresh(cx);
                }
            })
            .ok();
        }));
    }

    /// Re-scans the bottom of every agent terminal after `delay`. Replacing the
    /// task cancels the previous timer, so bursts of output collapse into one
    /// scan once the terminal settles.
    fn schedule_agent_detection(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.agent_detect_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.agent_detect_task = None;
                this.refresh_agent_states(cx);
            })
            .ok();
        }));
    }

    /// Applies the screen-tail rules to every terminal running an agent with a
    /// rule set, notifying once when an agent newly blocks on a question.
    fn refresh_agent_states(&mut self, cx: &mut Context<Self>) {
        let views: Vec<Entity<TerminalView>> = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter().cloned())
            .chain(
                self.detached_sessions
                    .iter()
                    .map(|session| session.view.clone()),
            )
            .collect();

        let mut observed = Vec::with_capacity(views.len());
        let mut still_streaming = false;
        for view in views {
            let detected = {
                let view_ref = view.read(cx);
                let terminal = view_ref.terminal().read(cx);
                let Some(agent) = coding_agent_for_terminal(&terminal) else {
                    continue;
                };
                if !agent_detect::supports(agent.id) {
                    continue;
                }
                if self
                    .terminal_activity
                    .get(&view.entity_id())
                    .is_some_and(|last| last.elapsed() < AGENT_DETECT_DEBOUNCE)
                {
                    still_streaming = true;
                }
                let screen = agent_detect::Screen {
                    lines: terminal.last_n_non_empty_lines(agent_detect::SCREEN_LINES),
                    title: terminal.title(false),
                };
                self.reported_agent_states
                    .get(&view.entity_id())
                    .map(|reported| reported.state)
                    .or_else(|| agent_detect::detect(agent.id, &screen))
            };
            observed.push((view, detected));
        }

        let mut changed = false;
        let mut newly_blocked = Vec::new();

        // Forget terminals that are no longer running an agent we can read, so
        // the quiet-window heuristic takes over again.
        let observed_ids: HashSet<EntityId> =
            observed.iter().map(|(view, _)| view.entity_id()).collect();
        let stale: Vec<EntityId> = self
            .agent_states
            .keys()
            .copied()
            .filter(|id| !observed_ids.contains(id))
            .collect();
        for id in stale {
            self.agent_states.remove(&id);
            changed = true;
        }

        for (view, detected) in observed {
            let id = view.entity_id();
            match detected {
                Some(state) => {
                    let previous = self.agent_states.insert(id, state);
                    // Screen rules supersede the coarse quiet-window flag for
                    // this terminal, so a stale flag cannot fire a second
                    // notification.
                    self.agent_waiting.remove(&id);
                    if previous != Some(state) {
                        changed = true;
                        if state == AgentState::Blocked {
                            newly_blocked.push(view);
                        }
                    }
                }
                None => {
                    if self.agent_states.remove(&id).is_some() {
                        changed = true;
                    }
                }
            }
        }

        if changed {
            cx.notify();
        }
        if !newly_blocked.is_empty() {
            notify_agents_waiting(&newly_blocked, cx);
        }
        // While the agent is still producing output the frame we just read was
        // partial, so scan again once it settles. Otherwise keep a slow poll
        // running while any agent is busy, because some state changes redraw
        // without emitting output.
        let next_scan = if still_streaming {
            Some(AGENT_DETECT_DEBOUNCE)
        } else if self
            .agent_states
            .values()
            .any(|state| *state != AgentState::Idle)
        {
            Some(AGENT_DETECT_POLL)
        } else {
            None
        };
        if let Some(delay) = next_scan {
            self.schedule_agent_detection(delay, cx);
        }
    }

    /// Terminal view by id, including detached sessions.
    pub fn view_by_id_any(&self, id: EntityId) -> Option<Entity<TerminalView>> {
        self.groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .chain(self.detached_sessions.iter().map(|session| &session.view))
            .find(|view| view.entity_id() == id)
            .cloned()
    }

    /// Subscribes to repositories that appeared after the panel was created.
    fn sync_git_subscriptions(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.upgrade() else {
            return;
        };
        let repositories: Vec<_> = project
            .read(cx)
            .repositories(cx)
            .values()
            .cloned()
            .collect();
        for repository in repositories {
            if !self.git_subscribed.insert(repository.entity_id()) {
                continue;
            }
            self.git_subscriptions.push(cx.subscribe(
                &repository,
                |_, _, event: &project::git_store::RepositoryEvent, cx| {
                    if matches!(
                        event,
                        project::git_store::RepositoryEvent::StatusesChanged
                            | project::git_store::RepositoryEvent::HeadChanged
                    ) {
                        cx.notify();
                    }
                },
            ));
        }
    }

    /// Branch and dirty-file count for the repository containing `cwd`.
    fn git_badge_for(&self, cwd: Option<&std::path::Path>, cx: &App) -> Option<GitBadge> {
        let cwd = cwd?;
        let project = self.project.upgrade()?;
        let project = project.read(cx);
        let mut best: Option<(usize, GitBadge)> = None;
        for repository in project.repositories(cx).values() {
            let snapshot = repository.read(cx).snapshot();
            let root = snapshot.work_directory_abs_path.as_ref();
            if !cwd.starts_with(root) {
                continue;
            }
            let depth = root.components().count();
            if best
                .as_ref()
                .is_some_and(|(best_depth, _)| *best_depth >= depth)
            {
                continue;
            }
            let branch = snapshot
                .branch
                .as_ref()
                .map(|branch| branch.name().to_string())
                .or_else(|| {
                    snapshot
                        .head_commit
                        .as_ref()
                        .map(|commit| commit.short_sha().to_string())
                })
                .unwrap_or_default();
            best = Some((
                depth,
                GitBadge {
                    branch: branch.into(),
                    dirty: snapshot.status_summary().count,
                },
            ));
        }
        best.map(|(_, badge)| badge)
    }

    /// Shell process ids of every terminal in this panel, including detached
    /// sessions. Used by the resource monitor.
    pub fn session_pids(&self, cx: &App) -> Vec<u32> {
        self.groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .chain(self.detached_sessions.iter().map(|session| &session.view))
            .filter_map(|view| {
                view.read(cx)
                    .terminal()
                    .read(cx)
                    .pid_getter()
                    .map(|getter| getter.fallback_pid().as_u32())
            })
            .collect()
    }

    /// Agent terminals across all groups, for the agent switcher.
    pub fn agent_terminals(&self, cx: &App) -> Vec<AgentTerminalEntry> {
        let mut entries = Vec::new();
        for group in &self.groups {
            for view in &group.terminals {
                let Some(badge) = agent_badge_for(
                    view,
                    &self.terminal_activity,
                    &self.agent_waiting,
                    &self.agent_states,
                    cx,
                ) else {
                    continue;
                };
                entries.push(AgentTerminalEntry {
                    id: view.entity_id(),
                    title: sidebar_terminal_name(view, cx),
                    agent: badge.name,
                    group: group.name.clone(),
                    working: badge.working,
                    waiting: badge.waiting,
                    icon: badge.icon,
                    color: badge.color,
                });
            }
        }
        // Waiting agents first, then working ones, then the rest.
        entries.sort_by_key(|entry| (!entry.waiting, !entry.working));
        entries
    }

    /// Focuses the next terminal whose agent is waiting for input, wrapping
    /// around and switching groups when needed.
    pub fn focus_next_waiting_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ordered: Vec<EntityId> = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .filter(|view| {
                agent_badge_for(
                    view,
                    &self.terminal_activity,
                    &self.agent_waiting,
                    &self.agent_states,
                    cx,
                )
                .is_some_and(|badge| badge.waiting)
            })
            .map(|view| view.entity_id())
            .collect();
        let Some(first) = ordered.first().copied() else {
            return;
        };
        let current = self.focused_terminal_id(cx);
        let target = current
            .and_then(|current| ordered.iter().position(|id| *id == current))
            .map(|index| ordered[(index + 1) % ordered.len()])
            .unwrap_or(first);
        self.focus_terminal_by_id(target, window, cx);
    }

    fn schedule_command_history_persist(&mut self, cx: &mut Context<Self>) {
        self.command_history_persist = cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            cx.update(|cx| crate::command_history::CommandHistory::persist(cx));
        });
    }

    /// Writes `command` to the active terminal and submits it. Used by the
    /// command history window.
    pub fn run_command_in_active_terminal(&mut self, command: &str, cx: &mut Context<Self>) {
        let Some(view) = self.active_terminal_view(cx) else {
            return;
        };
        let mut input = command.as_bytes().to_vec();
        input.push(b'\r');
        view.update(cx, |view, cx| {
            view.terminal().update(cx, |terminal, cx| {
                terminal.input(input);
                cx.notify();
            });
        });
    }

    fn session_spawns_complete(&self) -> bool {
        self.groups
            .iter()
            .all(|group| match group.session_terminal_count {
                None => true,
                Some(expected) => {
                    let filled = if !group.restore_slots.is_empty() {
                        group
                            .restore_slots
                            .iter()
                            .filter(|slot| slot.is_some())
                            .count()
                    } else {
                        group.terminals.len()
                    };
                    filled >= expected
                }
            })
    }

    fn clear_session_terminal_counts(&mut self) {
        for group in &mut self.groups {
            group.session_terminal_count = None;
            group.restore_slots.clear();
        }
    }

    /// Show the active group as soon as its terminals are ready. Other groups
    /// keep spawning in the background — we still wait for them before clearing
    /// `session_restoring` and writing the session file.
    fn reveal_active_group_during_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session_restoring
            || self.session_active_revealed
            || self.session_reveal_scheduled
            || !self.active_group_layout_ready()
        {
            return;
        }
        self.session_reveal_scheduled = true;
        cx.defer_in(window, |this, window, cx| {
            this.session_reveal_scheduled = false;
            if !this.session_restoring || this.session_active_revealed {
                return;
            }
            if !this.active_group_layout_ready() {
                return;
            }
            this.session_active_revealed = true;

            if this.pending_layout_restore {
                this.pending_layout_restore = false;
                this.switching = true;
                this.restore_active_group_layout(window, cx);
                this.switching = false;
            } else {
                this.switching = true;
                this.sync_active_group_to_pane(window, cx);
                this.switching = false;
            }
            cx.notify();
        });
    }

    /// Called when every group's expected session terminals have been spawned.
    fn finish_session_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session_restoring || self.session_finish_scheduled {
            return;
        }
        self.session_finish_scheduled = true;
        // Defer: load/spawn completion may still hold Workspace leases.
        cx.defer_in(window, |this, window, cx| {
            this.session_finish_scheduled = false;
            if !this.session_restoring {
                return;
            }

            // Active group may not have been revealed yet (e.g. empty session
            // finished immediately, or active group was the last to complete).
            if !this.session_active_revealed {
                if this.pending_layout_restore {
                    if this.active_group_layout_ready() {
                        this.pending_layout_restore = false;
                        this.switching = true;
                        this.restore_active_group_layout(window, cx);
                        this.switching = false;
                    } else {
                        this.pending_layout_restore = false;
                        this.switching = true;
                        this.sync_active_group_to_pane(window, cx);
                        this.switching = false;
                    }
                } else {
                    this.switching = true;
                    this.sync_active_group_to_pane(window, cx);
                    this.switching = false;
                }
                this.session_active_revealed = true;
            }

            // Capture whether every persisted terminal actually spawned before
            // dropping the expected counts. A timeout/failed spawn reaches
            // this path with incomplete groups; rewriting the session then
            // would delete the terminals that failed to restore.
            let all_restored = this.session_spawns_complete();
            this.clear_session_terminal_counts();
            this.session_restoring = false;
            this.session_active_revealed = false;
            this.restore_cwd_guard_until = Some(std::time::Instant::now() + Duration::from_secs(8));

            if all_restored {
                // First safe disk write after restore — preserve restored spawn
                // paths (prefer_remembered) so flaky early $HOME reads cannot
                // wipe them.
                this.write_session_file(cx, true);
            } else {
                log::warn!(
                    "terminal session restore did not finish; leaving the saved session file untouched"
                );
            }
            cx.notify();
        });
    }

    fn is_home_cwd(path: &PathBuf) -> bool {
        dirs::home_dir().as_ref().is_some_and(|home| path == home)
    }

    /// Pick a cwd for disk. Login-shell rc often reports `$HOME` while the
    /// terminal was restored (or last used) in a project path — never let that
    /// transient home reading wipe a more specific remembered/previous path.
    fn pick_persisted_cwd(
        live: Option<PathBuf>,
        remembered: Option<PathBuf>,
        previous: Option<PathBuf>,
        prefer_remembered: bool,
    ) -> Option<PathBuf> {
        let home_clobber = |candidate: &Option<PathBuf>, better: &Option<PathBuf>| {
            matches!(
                (candidate.as_ref(), better.as_ref()),
                (Some(home), Some(other))
                    if Self::is_home_cwd(home) && !Self::is_home_cwd(other)
            )
        };

        if prefer_remembered {
            return remembered.or(previous).or(live);
        }

        if home_clobber(&live, &remembered) {
            return remembered.or(previous).or(live);
        }
        if home_clobber(&live, &previous) {
            return previous.or(remembered).or(live);
        }
        live.or(remembered).or(previous)
    }

    /// `prefer_remembered`: after session restore, trust spawn/restored paths
    /// over flaky early process-info reads that often report `$HOME`.
    fn write_session_file(&mut self, cx: &App, prefer_remembered: bool) {
        // Never persist a half-restored session: async spawns leave
        // `group.terminals` empty until complete, which used to wipe the file
        // to `terminals: []` and drop inactive-group layouts on the next load.
        if self.session_restoring {
            return;
        }

        // Preserve previously saved cwds when live process info is still empty,
        // so a post-restore write cannot wipe paths we just loaded.
        let previous_cwds = self.read_previous_session_cwds();

        // Snapshot live cwds first so we can update `terminal_spawn_cwds`
        // without holding a borrow on `groups`.
        let mut live_cwds: Vec<(EntityId, Option<PathBuf>)> = Vec::new();
        for group in &self.groups {
            for view_ent in &group.terminals {
                let terminal = view_ent.read(cx).terminal().read(cx);
                // Read the background-refreshed cached cwd, not
                // latest_working_directory(): the latter force-refreshes process
                // info via blocking syscalls for every terminal on the UI
                // thread, which stalls session writes (and terminal creation).
                let live = terminal
                    .working_directory()
                    .filter(|cwd| !cwd.as_os_str().is_empty());
                live_cwds.push((view_ent.entity_id(), live));
            }
        }
        if !prefer_remembered {
            for (view_id, live) in &live_cwds {
                let Some(path) = live else {
                    continue;
                };
                let remembered = self
                    .terminal_spawn_cwds
                    .get(view_id)
                    .and_then(|c| c.as_ref());
                // Don't let a transient `$HOME` process-info reading clobber a
                // more specific path we already trust.
                if Self::is_home_cwd(path)
                    && remembered.is_some_and(|prev| !Self::is_home_cwd(prev))
                {
                    continue;
                }
                self.terminal_spawn_cwds
                    .insert(*view_id, Some(path.clone()));
            }
        }

        let mut session = PersistedSession {
            groups: Vec::new(),
            active_group_index: 0,
        };

        let mut live_ix = 0;
        for (i, group) in self.groups.iter().enumerate() {
            if group.id == self.active_group_id {
                session.active_group_index = i;
            }

            // Auto-tiling groups derive their layout from the terminal list;
            // saved_layout may lag behind (spawn/close between re-flows), so
            // serialize a fresh tree. A stale layout would prune terminals on
            // the next load.
            let layout = if group.layout_mode != GroupLayoutMode::Manual {
                GroupLayoutMode::generate_layout(group.layout_mode, group.terminals.len())
            } else {
                group.saved_layout.clone()
            };
            let mut p_group = PersistedGroup {
                name: group.name.to_string(),
                collapsed: group.collapsed,
                terminals: Vec::new(),
                layout,
                layout_mode: Some(group.layout_mode),
                broadcast_input: group.broadcast_input,
            };

            for (term_ix, view_ent) in group.terminals.iter().enumerate() {
                let view_id = view_ent.entity_id();
                let live = live_cwds.get(live_ix).and_then(|(_, cwd)| cwd.clone());
                live_ix += 1;
                let remembered = self.terminal_spawn_cwds.get(&view_id).cloned().flatten();
                let previous = previous_cwds.get(&(i, term_ix)).cloned().flatten();
                let cwd = Self::pick_persisted_cwd(live, remembered, previous, prefer_remembered);
                let title = view_ent
                    .read(cx)
                    .custom_title()
                    .map(|title| title.trim())
                    .filter(|title| !title.is_empty())
                    .map(|title| title.to_string());
                let color_scheme = view_ent.read(cx).color_scheme().map(str::to_owned);
                let font_size = view_ent.read(cx).font_size_override().map(f32::from);
                let launch = self.terminal_launches.get(&view_id).cloned();
                p_group.terminals.push(PersistedTerminal {
                    cwd,
                    title,
                    color_scheme,
                    font_size,
                    launch,
                    daemon_session: self.terminal_daemon_sessions.get(&view_id).cloned(),
                    agent_session: self.agent_session_refs.get(&view_id).cloned(),
                });
            }

            session.groups.push(p_group);
        }

        let path = self.session_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            // Write via a temp file so a crash mid-write cannot truncate the
            // session file to invalid JSON (which would silently lose the
            // whole workspace on the next launch).
            let temp_path = path.with_extension("json.tmp");
            if std::fs::write(&temp_path, json).is_ok()
                && std::fs::rename(&temp_path, &path).is_err()
            {
                // Windows does not replace an existing file on rename.
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::rename(&temp_path, &path);
            }
        }
    }

    fn read_previous_session_cwds(&self) -> HashMap<(usize, usize), Option<PathBuf>> {
        let Ok(data) = std::fs::read_to_string(self.session_file_path()) else {
            return HashMap::default();
        };
        let Ok(session): Result<PersistedSession, _> = serde_json::from_str(&data) else {
            return HashMap::default();
        };
        let mut out = HashMap::default();
        for (group_ix, group) in session.groups.into_iter().enumerate() {
            for (term_ix, term) in group.terminals.into_iter().enumerate() {
                if term.cwd.is_some() {
                    out.insert((group_ix, term_ix), term.cwd);
                }
            }
        }
        out
    }

    fn layout_referenced_indices(node: &GroupLayoutNode) -> std::collections::HashSet<usize> {
        let mut indices = std::collections::HashSet::default();
        Self::collect_layout_indices(node, &mut indices);
        indices
    }

    fn collect_layout_indices(
        node: &GroupLayoutNode,
        indices: &mut std::collections::HashSet<usize>,
    ) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                indices.extend(terminals.iter().copied());
                if let Some(active) = active {
                    indices.insert(*active);
                }
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::collect_layout_indices(child, indices);
                }
            }
        }
    }

    fn remap_layout_indices(node: &mut GroupLayoutNode, old_to_new: &HashMap<usize, usize>) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                *terminals = terminals
                    .iter()
                    .filter_map(|i| old_to_new.get(i).copied())
                    .collect();
                *active = active.and_then(|a| old_to_new.get(&a).copied());
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::remap_layout_indices(child, old_to_new);
                }
            }
        }
    }

    pub fn load_persisted_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Secondary windows must not restore the shared session — that would
        // duplicate every PTY from the first window.
        if !self.owns_workspace_session {
            return false;
        }

        let path = self.session_file_path();
        let Ok(data) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(session): Result<PersistedSession, _> = serde_json::from_str(&data) else {
            return false;
        };

        // Nothing to restore: no groups, or a session whose terminals were all
        // closed before quitting. Report false so the caller falls back to
        // spawning a terminal instead of opening a window with none.
        if !session
            .groups
            .iter()
            .any(|group| !group.terminals.is_empty())
        {
            return false;
        }

        // Two-pass: create groups and set active_group_id *before* any spawn
        // completes. Otherwise early groups match the constructor's GroupId(0)
        // and get added to the pane, while switch_group later no-ops because
        // active was already updated in the same loop.
        let mut to_spawn: Vec<(GroupId, usize, PersistedTerminal)> = Vec::new();
        let mut active_has_layout = false;
        for (i, p_group) in session.groups.into_iter().enumerate() {
            let id = self.new_group_id();
            let name = SharedString::from(p_group.name);
            let is_active = i == session.active_group_index;

            // If a layout exists, keep only terminals it references. Older bugs
            // (workspace DB restore + Terry session) left orphan terminals in
            // the JSON that are not part of the split — drop them on load.
            let (terminals, layout) = match p_group.layout {
                Some(mut layout) => {
                    let referenced = Self::layout_referenced_indices(&layout);
                    if referenced.is_empty() {
                        // Empty/broken layout — keep all terminals, drop layout.
                        (p_group.terminals, None)
                    } else {
                        let mut keep: Vec<usize> = referenced.into_iter().collect();
                        keep.sort_unstable();
                        keep.retain(|&ix| ix < p_group.terminals.len());
                        let old_to_new: HashMap<usize, usize> = keep
                            .iter()
                            .enumerate()
                            .map(|(new_ix, &old_ix)| (old_ix, new_ix))
                            .collect();
                        Self::remap_layout_indices(&mut layout, &old_to_new);
                        let terminals = keep
                            .iter()
                            .filter_map(|&ix| p_group.terminals.get(ix).cloned())
                            .collect();
                        (terminals, Some(layout))
                    }
                }
                None => (p_group.terminals, None),
            };

            // Drop layouts that still can't be applied after pruning.
            let term_count = terminals.len();
            let layout = layout.and_then(|layout| match Self::layout_max_index(&layout) {
                None => Some(layout),
                Some(max) if term_count > max => Some(layout),
                Some(_) => None,
            });
            if is_active && layout.is_some() {
                active_has_layout = true;
            }
            self.groups.push(TerminalGroup {
                id,
                name,
                terminals: Vec::new(),
                collapsed: p_group.collapsed,
                has_unread: false,
                saved_layout: layout,
                layout_mode: p_group.layout_mode.unwrap_or_default(),
                // Never resume broadcasting silently after a restart.
                broadcast_input: false,
                session_terminal_count: Some(term_count),
                restore_slots: vec![None; term_count],
            });

            if is_active {
                self.active_group_id = id;
            }

            for (index, p_term) in terminals.into_iter().enumerate() {
                to_spawn.push((id, index, p_term));
            }
        }

        self.pending_layout_restore = active_has_layout;
        self.session_restoring = true;
        self.session_active_revealed = false;
        self.session_reveal_scheduled = false;
        self.session_finish_scheduled = false;

        // A spawn that fails (missing shell, PTY exhaustion, ...) never fills
        // its restore slot, so `session_spawns_complete` would stay false
        // forever and the panel would remain in "restoring" mode: no session
        // writes, no worktree auto-spawn, and the active group may never be
        // revealed. Recover after a bounded wait.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(10))
                .await;
            this.update_in(cx, |this, window, cx| {
                if !this.session_restoring {
                    return;
                }
                let restored: usize = this.groups.iter().map(|g| g.terminals.len()).sum();
                let expected: usize = this
                    .groups
                    .iter()
                    .filter_map(|g| g.session_terminal_count)
                    .sum();
                log::warn!(
                    "terminal session restore timed out ({restored}/{expected} terminals); continuing"
                );
                this.finish_session_restore(window, cx);
            })
            .ok();
        })
        .detach();

        for (id, index, p_term) in to_spawn {
            let PersistedTerminal {
                cwd,
                title,
                color_scheme,
                font_size,
                launch,
                daemon_session,
                agent_session,
            } = p_term;
            let init_command = agent_session
                .as_ref()
                .and_then(|session| agent_session_resume_command(&session.agent, &session.value));
            // Replay the original launch (named profile, `ssh`, `tmux`, ...)
            // instead of opening a plain shell. A profile that no longer exists
            // falls back to the default shell rather than failing the restore.
            let (profile, launch) = Self::resolve_persisted_launch(launch, cx);
            self.spawn_terminal(
                id,
                cwd,
                None,
                None,
                SpawnRestore {
                    index: Some(index),
                    title,
                    color_scheme,
                    font_size,
                    launch,
                    daemon_session,
                },
                init_command,
                profile,
                window,
                cx,
            );
        }

        // Active group empty / already satisfied: show it immediately while
        // other groups continue spawning (or finish if nothing left).
        self.reveal_active_group_during_restore(window, cx);
        if self.session_spawns_complete() {
            self.finish_session_restore(window, cx);
        }
        true
    }

    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.groups.is_empty() {
            return;
        }

        // Worktrees may have finished loading since the panel was created, which
        // changes an empty window's key to the project key. Do this before
        // restoring so it reads the right file.
        self.migrate_session_key(cx);

        let profile = self.default_terminal_profile(cx);

        // Restore this workspace's previous terminals first. This is the normal
        // restart path: the session file already records the cwds, titles,
        // layouts and daemon sessions, so it restores the terminals the user had
        // instead of opening a brand-new shell in each project directory. Trying
        // the project directories first (as this used to) dropped the whole
        // session whenever the worktrees happened to be loaded already.
        if self.load_persisted_session(window, cx) {
            return;
        }

        // No session file yet (a folder opened for the first time): open one
        // terminal per project directory rather than a single shell in `$HOME`.
        let project_dirs = self.project_working_directories(cx);
        if !project_dirs.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
                layout_mode: GroupLayoutMode::Manual,
                broadcast_input: false,
                session_terminal_count: None,
                restore_slots: Vec::new(),
            });
            self.active_group_id = id;
            for cwd in project_dirs {
                self.spawn_terminal(
                    id,
                    Some(cwd),
                    None,
                    None,
                    SpawnRestore::default(),
                    None,
                    profile.clone(),
                    window,
                    cx,
                );
            }
            self.save_session(cx);
            return;
        }

        let id = self.new_group_id();
        self.groups.push(TerminalGroup {
            id,
            name: SharedString::from(i18n::t("terminals")),
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: None,
            layout_mode: GroupLayoutMode::Manual,
            broadcast_input: false,
            session_terminal_count: None,
            restore_slots: Vec::new(),
        });
        self.active_group_id = id;
        self.spawn_terminal(
            id,
            None,
            None,
            None,
            SpawnRestore::default(),
            None,
            profile,
            window,
            cx,
        );
        self.save_session(cx);
    }

    fn project_working_directories(&self, cx: &App) -> Vec<PathBuf> {
        let Some(project) = self.project.upgrade() else {
            return Vec::new();
        };
        project
            .read(cx)
            .visible_worktrees(cx)
            .filter_map(|worktree| Self::cwd_for_worktree(worktree.read(cx)))
            .collect()
    }

    fn cwd_for_worktree(worktree: &project::Worktree) -> Option<PathBuf> {
        if worktree.root_entry().is_some_and(|entry| entry.is_dir()) {
            Some(worktree.abs_path().to_path_buf())
        } else {
            worktree.abs_path().parent().map(|path| path.to_path_buf())
        }
    }

    fn new_group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Resolves the pane used for new tabs: the focused center pane after
    /// splits, so group operations stay in the pane the user is working in.
    fn display_pane_entity(&self, cx: &App) -> Option<Entity<Pane>> {
        self.workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).active_pane().clone())
            .or_else(|| self.display_pane.upgrade())
    }

    /// All center panes — splits belong to the active group and must be
    /// reconciled together so terminals are never duplicated across panes.
    fn center_panes(&self, cx: &App) -> Vec<Entity<Pane>> {
        self.workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).panes().to_vec())
            .unwrap_or_default()
    }

    /// Adds a new terminal to the currently active group, using the configured
    /// default profile when one is set.
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.default_terminal_profile(cx);
        self.new_terminal_with_destination_and_profile(None, profile, window, cx);
    }

    /// Adds a new terminal using an explicit profile. `None` means the normal
    /// shell configuration.
    pub fn new_terminal_with_profile(
        &mut self,
        profile: Option<TerminalProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_terminal_with_destination_and_profile(None, profile, window, cx);
    }

    fn default_terminal_profile(&self, cx: &App) -> Option<TerminalProfile> {
        TerminalSettings::get_global(cx)
            .default_profile_config()
            .cloned()
    }

    /// Opens a new shell in `cwd` and shows it in the active group.
    /// Does not resume a parked agent session.
    pub fn open_shell_at(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let group_id = self.active_group_id;
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.collapsed = false;
        }
        let init_command = cwd.to_str().map(|path| {
            let quoted = path.replace('\'', "'\\''");
            format!("cd '{quoted}'")
        });
        self.spawn_terminal(
            group_id,
            Some(cwd),
            None,
            self.display_pane_entity(cx),
            SpawnRestore::default(),
            init_command,
            None,
            window,
            cx,
        );
        self.save_session(cx);
    }

    /// Pin the pane that should receive the next spawned terminal.
    pub fn pin_spawn_pane(&mut self, pane: Entity<Pane>) {
        self.pending_spawn_pane = Some(pane.downgrade());
    }

    /// Create a terminal, optionally forcing it into `destination` (the split
    /// whose "+" was clicked). Prefer this over pin + new_terminal so the
    /// destination cannot be cleared by a mid-flight sync.
    pub fn new_terminal_with_destination(
        &mut self,
        destination: Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self.default_terminal_profile(cx);
        self.new_terminal_with_destination_and_profile(destination, profile, window, cx);
    }

    fn new_terminal_with_destination_and_profile(
        &mut self,
        destination: Option<Entity<Pane>>,
        profile: Option<TerminalProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = destination
            .or_else(|| self.pending_spawn_pane.take().and_then(|p| p.upgrade()))
            .or_else(|| self.display_pane_entity(cx));
        if let Some(pane) = destination.as_ref() {
            self.pending_spawn_pane = Some(pane.downgrade());
            self.display_pane = pane.downgrade();
        }
        self.new_terminal_in_group_with_destination(
            self.active_group_id,
            destination,
            profile,
            window,
            cx,
        );
    }

    /// Name of the currently selected group, if any.
    pub fn active_group_name(&self) -> Option<SharedString> {
        self.groups
            .iter()
            .find(|group| group.id == self.active_group_id)
            .map(|group| group.name.clone())
    }

    /// Whether the active group is currently broadcasting keystrokes to every
    /// terminal in it.
    pub fn active_group_broadcast_input(&self) -> bool {
        self.groups
            .iter()
            .find(|group| group.id == self.active_group_id)
            .is_some_and(|group| group.broadcast_input)
    }

    /// Currently focused terminal view (pane active item, else last in the
    /// active group).
    pub fn active_terminal_view(&self, cx: &App) -> Option<Entity<TerminalView>> {
        if let Some(tv) = self
            .display_pane_entity(cx)
            .and_then(|pane| pane.read(cx).active_item())
            .and_then(|item| item.act_as::<TerminalView>(cx))
        {
            return Some(tv);
        }
        self.groups
            .iter()
            .find(|group| group.id == self.active_group_id)
            .and_then(|group| group.terminals.last().cloned())
    }

    /// The terminal actually active in the center pane, ignoring the
    /// last-terminal fallback in [`Self::active_terminal_view`].
    pub fn focused_terminal_id(&self, cx: &App) -> Option<EntityId> {
        self.display_pane_entity(cx)
            .and_then(|pane| pane.read(cx).active_item())
            .and_then(|item| item.act_as::<TerminalView>(cx))
            .map(|view| view.entity_id())
    }

    /// Number of terminals across every group, including inactive groups.
    pub fn terminal_count(&self) -> usize {
        self.groups.iter().map(|group| group.terminals.len()).sum()
    }

    /// Cached cwd of the currently focused terminal. Reads the value the
    /// background process-info refresh keeps warm rather than force-refreshing
    /// on the UI thread (which blocks terminal creation on macOS).
    pub fn active_terminal_cwd(&self, cx: &App) -> Option<PathBuf> {
        self.active_terminal_view(cx)
            .and_then(|tv| tv.read(cx).terminal().read(cx).working_directory())
    }

    /// Adds a new terminal to the given group, switching to it if needed.
    fn new_terminal_in_group(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self.default_terminal_profile(cx);
        self.new_terminal_in_group_with_destination(group_id, None, profile, window, cx);
    }

    fn new_terminal_in_group_with_profile(
        &mut self,
        group_id: GroupId,
        profile: TerminalProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_terminal_in_group_with_destination(group_id, None, Some(profile), window, cx);
    }

    fn new_terminal_in_group_with_destination(
        &mut self,
        group_id: GroupId,
        destination: Option<Entity<Pane>>,
        profile: Option<TerminalProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        let source = self.active_terminal_view(cx);
        let cwd = self.active_terminal_cwd(cx);
        if group_id != self.active_group_id {
            self.switch_group(group_id, window, cx);
        }
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }
        if let Some(view) = self.take_parked_agent_session(cwd.as_deref(), cx) {
            self.clear_pending_resume(cwd.as_deref());
            self.add_terminal_to_group(group_id, view, None, cwd, cx);
            self.defer_sync_active_group_to_pane(window, cx);
            self.save_session(cx);
            return;
        }
        if let Some((bin, session_cwd, template)) = self.take_pending_resume(cwd.as_deref()) {
            let command = agent_command(template, bin);
            self.spawn_terminal(
                group_id,
                session_cwd.or(cwd),
                source,
                destination,
                SpawnRestore::default(),
                Some(command),
                None,
                window,
                cx,
            );
            self.save_session(cx);
            return;
        }
        self.spawn_terminal(
            group_id,
            cwd,
            source,
            destination,
            SpawnRestore::default(),
            None,
            profile,
            window,
            cx,
        );
        self.save_session(cx);
    }

    /// Clones a terminal: spawns a new one in the same group, starting in the
    /// source terminal's current working directory (live PTY cwd, falling back
    /// to its original working directory).
    fn clone_terminal(
        &mut self,
        group_id: GroupId,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        let terminal = terminal_view.read(cx).terminal().clone();
        // Use the cached cwd (kept fresh by the background process-info
        // refresh). Force-refreshing here via latest_working_directory() blocks
        // the UI thread on macOS process syscalls and can hang terminal cloning.
        let cwd = terminal.read(cx).working_directory();
        if group_id != self.active_group_id {
            self.switch_group(group_id, window, cx);
        }
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }
        self.spawn_terminal(
            group_id,
            cwd,
            Some(terminal_view),
            None,
            SpawnRestore::default(),
            None,
            None,
            window,
            cx,
        );
        self.save_session(cx);
    }

    fn build_group_context_menu(
        panel: Entity<Self>,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        let can_delete = panel.read(cx).groups.len() > 1;
        let current_mode = panel
            .read(cx)
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.layout_mode);
        let agent_actions = panel.read(cx).agent_session_actions(group_id, cx);
        let profiles = TerminalSettings::get_global(cx).profiles.clone();
        let broadcast_input = panel
            .read(cx)
            .groups
            .iter()
            .any(|g| g.id == group_id && g.broadcast_input);
        let group_name = panel
            .read(cx)
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.name.to_string())
            .unwrap_or_default();
        let templates = load_templates();
        ContextMenu::build(window, cx, move |menu, _, _| {
            let view1 = panel.clone();
            let view2 = panel.clone();
            let view3 = panel.clone();
            let view4 = panel.clone();
            let view5 = panel.clone();
            let view6 = panel.clone();
            let view7 = panel.clone();
            let menu = menu
                .entry(i18n::t("rename"), None, move |window, cx| {
                    view1.update(cx, |this, cx| {
                        this.start_renaming(group_id, window, cx);
                    });
                })
                .entry(i18n::t("move_up"), None, move |_window, cx| {
                    view2.update(cx, |this, cx| {
                        this.move_group_up(group_id, cx);
                    });
                })
                .entry(i18n::t("move_down"), None, move |_window, cx| {
                    view3.update(cx, |this, cx| {
                        this.move_group_down(group_id, cx);
                    });
                })
                .separator()
                .entry(i18n::t("new_terminal"), None, move |window, cx| {
                    view4.update(cx, |this, cx| {
                        this.new_terminal_in_group(group_id, window, cx);
                    });
                });
            let menu = if profiles.is_empty() {
                menu
            } else {
                let submenu_panel = panel.clone();
                menu.submenu(
                    i18n::t("new_terminal_with_profile"),
                    move |mut menu, _window, _cx| {
                        for profile in &profiles {
                            let panel = submenu_panel.clone();
                            let label = profile.name.clone();
                            let profile = profile.clone();
                            menu = menu.entry(label, None, move |window, cx| {
                                panel.update(cx, |this, cx| {
                                    this.new_terminal_in_group_with_profile(
                                        group_id,
                                        profile.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            });
                        }
                        menu
                    },
                )
            };
            let panel_a = panel.clone();
            let panel_b = panel.clone();
            let panel_c = panel.clone();
            let panel_d = panel.clone();
            let menu = menu
                .separator()
                .entry(i18n::t("connect_ssh_host"), None, move |window, cx| {
                    panel_a.update(cx, |this, cx| this.open_ssh_picker(false, window, cx));
                })
                .entry(i18n::t("connect_ssh_host_tmux"), None, move |window, cx| {
                    panel_b.update(cx, |this, cx| this.open_ssh_picker(true, window, cx));
                })
                .entry(i18n::t("attach_tmux_session"), None, move |window, cx| {
                    panel_c.update(cx, |this, cx| this.open_tmux_picker(window, cx));
                })
                .entry(i18n::t("attach_tmux_native"), None, move |window, cx| {
                    panel_d.update(cx, |this, cx| this.open_tmux_picker_mode(true, window, cx));
                });
            let menu = if let Some(actions) = agent_actions {
                let mut menu = menu.separator();
                if actions.resume {
                    let view = panel.clone();
                    menu = menu.entry(i18n::t("resume_agent_session"), None, move |window, cx| {
                        view.update(cx, |this, cx| {
                            this.resume_agent_session(group_id, window, cx);
                        });
                    });
                }
                if actions.fork.is_some() {
                    let view = panel.clone();
                    menu = menu.entry(i18n::t("fork_agent_session"), None, move |window, cx| {
                        view.update(cx, |this, cx| {
                            this.fork_agent_session(group_id, window, cx);
                        });
                    });
                }
                if actions.past.is_some() {
                    let view = panel.clone();
                    menu = menu.entry(i18n::t("past_agent_sessions"), None, move |window, cx| {
                        view.update(cx, |this, cx| {
                            this.past_agent_sessions(group_id, window, cx);
                        });
                    });
                }
                menu
            } else {
                menu
            };
            let menu = menu.separator().header(i18n::t("layout"));
            let mut menu = menu;
            for mode in [
                GroupLayoutMode::Manual,
                GroupLayoutMode::Tall,
                GroupLayoutMode::Grid,
                GroupLayoutMode::Stack,
                GroupLayoutMode::Fat,
                GroupLayoutMode::Horizontal,
                GroupLayoutMode::Vertical,
            ] {
                let is_current = current_mode == Some(mode);
                let checked = if is_current { "\u{2713} " } else { "" };
                let panel = panel.clone();
                let label = format!("{checked}{}", mode.label());
                menu = menu.entry(label, None, move |window, cx| {
                    panel.update(cx, |this, cx| {
                        this.set_group_layout(group_id, mode, window, cx);
                    });
                });
            }
            let checked = if broadcast_input { "\u{2713} " } else { "" };
            let panel = panel.clone();
            menu = menu.separator().entry(
                format!("{checked}{}", i18n::t("broadcast_input")),
                None,
                move |window, cx| {
                    panel.update(cx, |this, cx| {
                        this.toggle_broadcast_input(group_id, window, cx);
                    });
                },
            );
            let template_panel = view6;
            let template_name = group_name.clone();
            let menu = menu.separator().entry(
                i18n::t("save_group_as_template"),
                None,
                move |window, cx| {
                    template_panel.update(cx, |this, cx| {
                        this.save_active_group_as_template(template_name.clone(), cx);
                    });
                    let _ = window;
                },
            );
            let menu = if templates.is_empty() {
                menu
            } else {
                let submenu_panel = view7;
                let templates = templates.clone();
                menu.submenu(
                    i18n::t("new_group_from_template"),
                    move |mut menu, _window, _cx| {
                        for template in &templates {
                            let panel = submenu_panel.clone();
                            let template = template.clone();
                            let label = template.name.clone();
                            menu = menu.entry(label, None, move |window, cx| {
                                panel.update(cx, |this, cx| {
                                    this.add_group_from_template(template.clone(), window, cx);
                                });
                            });
                        }
                        menu
                    },
                )
            };
            if can_delete {
                menu.separator()
                    .entry(i18n::t("delete_group"), None, move |window, cx| {
                        view5.update(cx, |this, cx| {
                            this.delete_group(group_id, window, cx);
                        });
                    })
            } else {
                menu
            }
        })
    }

    /// Context menu for a terminal row: clone, rename, move to another group,
    /// and close.
    fn build_terminal_context_menu(
        panel: Entity<Self>,
        source_group: GroupId,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        let terminal_id = terminal_view.entity_id();
        let recording = crate::session_recording::is_recording(terminal_id, cx);
        let move_targets: Vec<(GroupId, SharedString)> = panel
            .read(cx)
            .groups
            .iter()
            .filter(|group| group.id != source_group)
            .map(|group| (group.id, group.name.clone()))
            .collect();

        ContextMenu::build(window, cx, move |menu, _, _| {
            let clone_panel = panel.clone();
            let clone_view = terminal_view.clone();
            let rename_view = terminal_view.clone();
            let close_panel = panel.clone();
            let moved_panel = panel.clone();
            let menu = menu
                .entry(i18n::t("clone_terminal"), None, move |window, cx| {
                    clone_panel.update(cx, |this, cx| {
                        this.clone_terminal(source_group, clone_view.clone(), window, cx);
                    });
                })
                .entry(i18n::t("rename"), None, move |window, cx| {
                    rename_view.update(cx, |this, cx| {
                        this.rename_terminal(&terminal_view::RenameTerminal, window, cx)
                    });
                })
                .entry(
                    i18n::t("move_tab_to_new_window"),
                    None,
                    move |window, cx| {
                        // The command moves the active tab, so focus this row
                        // first when it was right-clicked from the sidebar.
                        moved_panel.update(cx, |this, cx| {
                            this.focus_terminal_by_id(terminal_id, window, cx);
                        });
                        window.dispatch_action(
                            crate::move_to_window::MoveTabToNewWindow.boxed_clone(),
                            cx,
                        );
                    },
                );

            let menu = if move_targets.is_empty() {
                menu
            } else {
                let submenu_panel = panel.clone();
                let targets = move_targets.clone();
                menu.separator()
                    .submenu(i18n::t("move_to_group"), move |mut menu, _window, _cx| {
                        for (target_group_id, name) in &targets {
                            let panel = submenu_panel.clone();
                            let target_group_id = *target_group_id;
                            let name = name.clone();
                            menu = menu.entry(name, None, move |window, cx| {
                                panel.update(cx, |this, cx| {
                                    this.move_terminal_to_group(
                                        terminal_id,
                                        target_group_id,
                                        window,
                                        cx,
                                    );
                                });
                            });
                        }
                        menu
                    })
            };

            let color_panel = panel.clone();
            let import_panel = panel.clone();
            let menu = menu
                .separator()
                .entry(i18n::t("terminal_color_scheme"), None, move |window, cx| {
                    color_panel.update(cx, |this, cx| {
                        this.open_color_scheme_picker_for(terminal_id, window, cx);
                    });
                })
                .entry(i18n::t("import_color_scheme"), None, move |window, cx| {
                    import_panel.update(cx, |this, cx| this.import_color_scheme(window, cx));
                });

            let detach_panel = panel.clone();
            let menu =
                menu.separator()
                    .entry(i18n::t("detach_terminal"), None, move |window, cx| {
                        detach_panel.update(cx, |this, cx| {
                            this.detach_terminal(terminal_id, window, cx);
                        });
                    });

            let record_view = terminal_view.clone();
            let menu = menu.entry(
                i18n::t(if recording {
                    "stop_recording"
                } else {
                    "start_recording"
                }),
                None,
                move |_window, cx| {
                    crate::session_recording::toggle(&record_view, cx);
                },
            );
            let menu = menu.entry(i18n::t("open_recordings"), None, move |_window, cx| {
                crate::session_recording::reveal_recordings(cx);
            });

            let inspect_view = terminal_view.clone();
            let menu = menu
                .separator()
                .entry(i18n::t("inspect_processes"), None, move |_window, cx| {
                    crate::process_inspector::open(inspect_view.downgrade(), cx);
                })
                .entry(i18n::t("switch_worktree"), None, move |window, cx| {
                    window.dispatch_action(zed_actions::git::Worktree.boxed_clone(), cx);
                });

            let export_view = terminal_view.clone();
            let menu = menu.submenu(i18n::t("export_scrollback"), move |menu, _window, _cx| {
                let copy_view = export_view.clone();
                let markdown_view = export_view.clone();
                let history_view = export_view.clone();
                let file_view = export_view.clone();
                menu.entry(
                    crate::scrollback_export::label("export_scrollback_clipboard"),
                    None,
                    move |_window, cx| {
                        crate::scrollback_export::copy(&copy_view, cx);
                    },
                )
                .entry(
                    crate::scrollback_export::label("export_scrollback_markdown"),
                    None,
                    move |_window, cx| {
                        crate::scrollback_export::copy_markdown(&markdown_view, cx);
                    },
                )
                .entry(
                    crate::scrollback_export::label("export_scrollback_history"),
                    None,
                    move |_window, cx| {
                        crate::scrollback_export::add_to_history(&history_view, cx);
                    },
                )
                .entry(
                    crate::scrollback_export::label("export_scrollback_file"),
                    None,
                    move |_window, cx| {
                        crate::scrollback_export::save_to_file(&file_view, cx);
                    },
                )
            });

            menu.entry(i18n::t("close"), None, move |window, cx| {
                close_panel.update(cx, |this, cx| {
                    this.close_terminal(terminal_id, window, cx);
                });
            })
        })
    }

    /// Moves a terminal into another group without recreating its view or PTY.
    ///
    /// The source group's manual split layout is remapped so its persisted
    /// indices keep pointing at the same terminals; auto-tiling groups re-flow
    /// from their terminal list. If the source group is active, the tab also
    /// leaves the center panes; the active group (either source or target) is
    /// synced afterwards.
    fn move_terminal_to_group(
        &mut self,
        terminal_id: EntityId,
        target_group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_restoring {
            return;
        }
        let Some(source_group_id) = self
            .groups
            .iter()
            .find(|group| {
                group
                    .terminals
                    .iter()
                    .any(|view| view.entity_id() == terminal_id)
            })
            .map(|group| group.id)
        else {
            return;
        };
        if source_group_id == target_group_id
            || !self.groups.iter().any(|group| group.id == target_group_id)
        {
            return;
        }

        let Some(source_group_ix) = self
            .groups
            .iter()
            .position(|group| group.id == source_group_id)
        else {
            return;
        };
        let Some(removed_ix) = self.groups[source_group_ix]
            .terminals
            .iter()
            .position(|view| view.entity_id() == terminal_id)
        else {
            return;
        };

        // Manual layouts store indices into `group.terminals`; keep them
        // pointing at the same terminals after the list shifts left.
        if self.groups[source_group_ix].layout_mode == GroupLayoutMode::Manual {
            if let Some(layout) = self.groups[source_group_ix].saved_layout.as_mut() {
                Self::remap_layout_after_remove(layout, removed_ix);
            }
        }
        let terminal_view = self.groups[source_group_ix].terminals.remove(removed_ix);
        if self.groups[source_group_ix].terminals.is_empty() {
            self.groups[source_group_ix].has_unread = false;
            if self.groups[source_group_ix].layout_mode == GroupLayoutMode::Manual {
                self.groups[source_group_ix].saved_layout = None;
            }
        }

        let Some(target_group) = self
            .groups
            .iter_mut()
            .find(|group| group.id == target_group_id)
        else {
            return;
        };
        target_group.collapsed = false;
        target_group.terminals.push(terminal_view);

        let source_active = source_group_id == self.active_group_id;
        let target_active = target_group_id == self.active_group_id;

        // Only the active group's terminals belong in the center panes. Remove
        // the moved tab before syncing so it does not stay visible under the
        // source group.
        if source_active {
            if let Some(workspace) = self.workspace.upgrade() {
                let panes = workspace.read(cx).panes().to_vec();
                for pane in panes {
                    let shows_terminal = pane
                        .read(cx)
                        .items()
                        .any(|item| item.item_id() == terminal_id);
                    if shows_terminal {
                        pane.update(cx, |pane, cx| {
                            pane.remove_item(terminal_id, false, false, window, cx);
                        });
                    }
                }
            }
        }

        if source_active || target_active {
            self.defer_sync_active_group_to_pane(window, cx);
        }

        self.save_session(cx);
        cx.notify();
    }

    /// Switches a group's auto-tiling layout mode and re-flows its panes.
    fn set_group_layout(
        &mut self,
        group_id: GroupId,
        mode: GroupLayoutMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.iter().find(|g| g.id == group_id) else {
            return;
        };
        if group.layout_mode == mode {
            return;
        }
        let is_active = group_id == self.active_group_id;
        // Auto modes regenerate the tree; manual mode falls back to whatever
        // the panes currently look like (captured below if active).
        let generated = if mode != GroupLayoutMode::Manual {
            GroupLayoutMode::generate_layout(mode, group.terminals.len())
        } else {
            None
        };

        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.layout_mode = mode;
            group.saved_layout = generated.clone();
        }
        self.save_session(cx);
        cx.notify();

        if !is_active {
            return;
        }
        if let Some(layout) = generated {
            // Rebuild immediately so the user sees the tiling take effect.
            // Guard against ItemRemoved events dropping model terminals.
            self.switching = true;
            self.apply_layout_to_center(group_id, &layout, window, cx);
            self.switching = false;
        } else {
            // Back to manual: collapse to a single pane holding the group's
            // terminals as tabs.
            self.switching = true;
            self.restore_active_group_layout(window, cx);
            self.switching = false;
        }
    }

    /// Creates a new group, switches to it and spawns its first terminal.
    fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.active_terminal_view(cx);
        let cwd = self.active_terminal_cwd(cx);
        let id = self.new_group_id();
        let name = SharedString::from(format!("{} {}", i18n::t("group"), id.0 + 1));
        self.groups.push(TerminalGroup {
            id,
            name,
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: None,
            layout_mode: GroupLayoutMode::Manual,
            broadcast_input: false,
            session_terminal_count: None,
            restore_slots: Vec::new(),
        });
        self.switch_group(id, window, cx);
        let profile = self.default_terminal_profile(cx);
        self.spawn_terminal(
            id,
            cwd,
            source,
            None,
            SpawnRestore::default(),
            None,
            profile,
            window,
            cx,
        );
        self.save_session(cx);
    }

    /// Deletes a group and drops all of its `TerminalView` entities so PTYs
    /// are killed. The last remaining group cannot be deleted.
    /// Deletes a group, asking once when any of its terminals is still running
    /// a command.
    fn delete_group(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if self.groups.len() <= 1 {
            return;
        }
        let has_running_command = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .is_some_and(|group| {
                group
                    .terminals
                    .iter()
                    .any(|view| view.read(cx).terminal().read(cx).is_running_command())
            });
        if !has_running_command {
            self.delete_group_now(group_id, window, cx);
            return;
        }

        let title = i18n::t("delete_group_running");
        let detail = i18n::t("delete_group_running_detail");
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[i18n::t_str("delete"), i18n::t_str("cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    this.delete_group_now(group_id, window, cx)
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// Ends a daemon session without blocking the caller.
    ///
    /// `kill` round-trips to the daemon over a socket, and an unreachable
    /// daemon can take seconds to fail, so close paths must not wait on it.
    fn kill_daemon_session(session: String) {
        std::thread::spawn(move || {
            if let Err(error) = crate::persistent_sessions::kill(&session) {
                log::debug!("failed to kill daemon session {session}: {error}");
            }
        });
    }

    fn delete_group_now(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if self.groups.len() <= 1 {
            return;
        }
        let Some(index) = self.groups.iter().position(|g| g.id == group_id) else {
            return;
        };

        let was_active = self.active_group_id == group_id;
        let removed = self.groups.remove(index);
        let terminal_ids: std::collections::HashSet<EntityId> =
            removed.terminals.iter().map(|tv| tv.entity_id()).collect();
        // Deleting a group ends its processes, including daemon-backed ones;
        // detaching here would leave sessions nobody can reach from the UI.
        for id in &terminal_ids {
            if let Some(session) = self.terminal_daemon_sessions.remove(id) {
                Self::kill_daemon_session(session);
            }
        }
        for id in &terminal_ids {
            self._terminal_subscriptions.remove(id);
            self.terminal_activity.remove(id);
            self.agent_waiting.remove(id);
            self.agent_states.remove(id);
            self.reported_agent_states.remove(id);
            self.agent_session_refs.remove(id);
            self.terminal_tokens.retain(|_, view| view != id);
        }

        // Pull closed items out of panes before dropping Entities.
        self.switching = true;
        if let Some(workspace) = self.workspace.upgrade() {
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                pane.update(cx, |pane, cx| {
                    for id in &terminal_ids {
                        if pane.items().any(|item| item.item_id() == *id) {
                            pane.remove_item(*id, false, false, window, cx);
                        }
                    }
                });
            }
        }

        if self.renaming_group_id == Some(group_id) {
            self.renaming_group_id = None;
        }

        if was_active {
            let next_index = index.min(self.groups.len().saturating_sub(1));
            let next_id = self.groups[next_index].id;
            self.active_group_id = next_id;
            self.pending_switch_from = None;
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == next_id) {
                group.collapsed = false;
            }
            self.restore_active_group_layout(window, cx);
        }
        self.switching = false;

        // Drop TerminalViews (and their Terminal/PTY) — last strong refs.
        drop(removed);

        self.save_session(cx);
        cx.notify();
    }

    /// Closes a terminal, asking for confirmation when a command is still
    /// running in it. The actual removal is deferred until the prompt (which
    /// may be `Task::ready` for idle shells) resolves.
    fn close_terminal(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.terminal_view_by_id(terminal_id) else {
            self.close_terminal_now(terminal_id, window, cx);
            return;
        };
        let confirmed = view.confirm_close(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if confirmed.await {
                this.update_in(cx, |this, window, cx| {
                    this.close_terminal_now(terminal_id, window, cx)
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub fn terminal_view_by_id(&self, terminal_id: EntityId) -> Option<Entity<TerminalView>> {
        self.groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .find(|view| view.entity_id() == terminal_id)
            .cloned()
    }

    /// Focus a terminal by id, switching to its group when necessary.
    /// Returns `false` when the id is unknown.
    pub fn focus_terminal_by_id(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(group_id) = self
            .groups
            .iter()
            .find(|group| {
                group
                    .terminals
                    .iter()
                    .any(|view| view.entity_id() == terminal_id)
            })
            .map(|group| group.id)
        else {
            return false;
        };
        let Some(view) = self.terminal_view_by_id(terminal_id) else {
            return false;
        };
        self.focus_terminal_in_group(group_id, view, window, cx);
        true
    }

    /// JSON description of one terminal (or the active terminal when `None`).
    pub fn terminal_context(
        &self,
        terminal_id: Option<EntityId>,
        cx: &App,
    ) -> Option<serde_json::Value> {
        let view = match terminal_id {
            Some(id) => self.view_by_id_any(id)?,
            None => self.active_terminal_view(cx)?,
        };
        let id = view.entity_id();
        let view = view.read(cx);
        let terminal = view.terminal().read(cx);
        let group = self.groups.iter().find(|group| {
            group
                .terminals
                .iter()
                .any(|terminal_view| terminal_view.entity_id() == id)
        });
        Some(serde_json::json!({
            "terminal_id": id.as_u64(),
            "title": view
                .custom_title()
                .map(str::to_string)
                .unwrap_or_else(|| terminal.title(false)),
            "cwd": terminal
                .working_directory()
                .map(|cwd| cwd.display().to_string()),
            "command": terminal.foreground_process_command_name(),
            "exit_code": terminal.last_exit_code(),
            "exited": terminal.has_exited(),
            "group": group.map(|group| group.name.to_string()),
            "agent_state": self.agent_states.get(&id).map(|state| state.as_str()),
        }))
    }

    /// Applies a resumable session report from an in-terminal agent integration.
    pub fn apply_reported_agent_session(
        &mut self,
        token: &str,
        agent: &str,
        session_path: Option<String>,
        session_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let Some(view_id) = self.terminal_tokens.get(token).copied() else {
            return Err("unknown terminal token".to_string());
        };
        let value = session_path
            .or(session_id)
            .ok_or_else(|| "missing session reference".to_string())?;

        self.agent_session_refs.insert(
            view_id,
            PersistedAgentSession {
                agent: agent.to_string(),
                value,
            },
        );
        cx.notify();

        Ok(serde_json::json!({
            "status": "ok",
            "terminal_id": view_id.as_u64(),
        }))
    }

    /// Applies a state report from an in-terminal agent integration.
    ///
    /// The report is authoritative: it feeds the badge directly and screen
    /// detection defers to it until the terminal closes.
    pub fn apply_reported_agent_state(
        &mut self,
        token: &str,
        state: &str,
        message: Option<String>,
        agent: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let Some(view_id) = self.terminal_tokens.get(token).copied() else {
            return Err("unknown terminal token".to_string());
        };

        let state = match state {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            _ => AgentState::Idle,
        };

        self.reported_agent_states
            .insert(view_id, ReportedAgentState { state, message });
        // Badges read `agent_states`; screen detection defers to reports.
        self.agent_states.insert(view_id, state);
        self.agent_waiting.remove(&view_id);
        cx.notify();

        Ok(serde_json::json!({
            "status": "ok",
            "terminal_id": view_id.as_u64(),
            "agent": agent,
        }))
    }

    /// Agent terminals with their effective state, for the remote-control API.
    ///
    /// The state matches what the sidebar badge shows: screen-tail detection
    /// when it has an answer, otherwise the quiet-window heuristic.
    pub fn agent_states_json(&self, cx: &App) -> serde_json::Value {
        let mut terminals = Vec::new();

        let grouped = self.groups.iter().flat_map(|group| {
            group
                .terminals
                .iter()
                .map(move |view| (view, Some(group.name.clone())))
        });
        let detached = self
            .detached_sessions
            .iter()
            .map(|session| (&session.view, None::<SharedString>));

        for (view, group) in grouped.chain(detached) {
            let Some(badge) = agent_badge_for(
                view,
                &self.terminal_activity,
                &self.agent_waiting,
                &self.agent_states,
                cx,
            ) else {
                continue;
            };
            let id = view.entity_id();
            let state = if badge.waiting {
                "blocked"
            } else if badge.working {
                "working"
            } else {
                "idle"
            };
            let terminal = view.read(cx).terminal().read(cx);
            terminals.push(serde_json::json!({
                "terminal_id": id.as_u64(),
                "title": sidebar_terminal_name(view, cx),
                "group": group.map(|name| name.to_string()),
                "agent": badge.name.to_string(),
                "state": state,
                "exited": terminal.has_exited(),
                "cwd": terminal
                    .working_directory()
                    .map(|cwd| cwd.display().to_string()),
            }));
        }

        serde_json::json!({
            "active_terminal_id": self
                .active_terminal_view(cx)
                .map(|view| view.entity_id().as_u64()),
            "terminals": terminals,
        })
    }

    /// Persistent terminals keep running after their tab is closed. Tell the
    /// user once, so a closed terminal is not mistaken for a killed one.
    fn notice_detached_terminal(&self, terminal_id: EntityId, cx: &mut App) {
        if !persistent_sessions::is_enabled(cx)
            || !self.terminal_daemon_sessions.contains_key(&terminal_id)
        {
            return;
        }
        // Closing a terminal whose shell already exited (typed `exit`, Ctrl+D)
        // ends its session rather than detaching it, so there is nothing left
        // running to announce. Checking this first also keeps the one-time
        // notice for a live terminal that really is detached.
        let still_running = self
            .terminal_view_by_id(terminal_id)
            .is_some_and(|view| !view.read(cx).terminal().read(cx).has_exited());
        if !still_running || !persistent_sessions::should_notice_detach() {
            return;
        }
        cx.show_system_notification(gpui::SystemNotification {
            tag: "terry-persistent-detach".into(),
            title: i18n::t("persistent_sessions").into(),
            body: i18n::t("persistent_sessions_detached_notice").into(),
            actions: Vec::new(),
        });
    }

    /// Closes a grouped or detached terminal without showing a confirmation
    /// prompt. Used by the remote-control API.
    pub fn close_session_by_id(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(index) = self
            .detached_sessions
            .iter()
            .position(|session| session.view.entity_id() == terminal_id)
        {
            self.close_detached_session(index, cx);
            return true;
        }

        if self.terminal_view_by_id(terminal_id).is_none() {
            return false;
        }
        self.close_terminal_now(terminal_id, window, cx);
        true
    }

    /// Detaches a grouped terminal, keeping its PTY alive.
    pub fn detach_terminal_by_id(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal_view_by_id(terminal_id).is_none() {
            return false;
        }
        self.detach_terminal(terminal_id, window, cx);
        true
    }

    /// Reattaches a detached terminal into the active group.
    pub fn reattach_terminal_by_id(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self
            .detached_sessions
            .iter()
            .position(|session| session.view.entity_id() == terminal_id)
        else {
            return false;
        };
        self.reattach_detached(index, window, cx);
        true
    }

    /// Machine-readable list of detached sessions.
    pub fn detached_sessions_json(&self, cx: &App) -> serde_json::Value {
        let sessions: Vec<_> = self
            .detached_sessions
            .iter()
            .map(|session| {
                let terminal = session.view.read(cx).terminal().read(cx);
                serde_json::json!({
                    "terminal_id": session.view.entity_id().as_u64(),
                    "title": session.title.to_string(),
                    "cwd": session.cwd.as_ref().map(|cwd| cwd.display().to_string()),
                    "command": terminal.foreground_process_command_name(),
                    "exited": terminal.has_exited(),
                })
            })
            .collect();
        serde_json::json!({ "sessions": sessions })
    }

    /// Creates a group from the remote-control API and spawns its first
    /// terminal. Returns the group id.
    pub fn create_group_from_api(
        &mut self,
        name: Option<String>,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let source = self.active_terminal_view(cx);
        let cwd = cwd.or_else(|| self.active_terminal_cwd(cx));
        let id = self.new_group_id();
        let name = name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("{} {}", i18n::t("group"), id.0 + 1));
        self.groups.push(TerminalGroup {
            id,
            name: SharedString::from(name),
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: None,
            layout_mode: GroupLayoutMode::Manual,
            broadcast_input: false,
            session_terminal_count: None,
            restore_slots: Vec::new(),
        });
        self.switch_group(id, window, cx);
        let profile = self.default_terminal_profile(cx);
        self.spawn_terminal(
            id,
            cwd,
            source,
            None,
            SpawnRestore::default(),
            None,
            profile,
            window,
            cx,
        );
        self.save_session(cx);
        id.0 as u64
    }

    /// Deletes a group and drops all its terminals. Refuses to delete the last
    /// group.
    pub fn delete_group_by_id(
        &mut self,
        group_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.groups.len() <= 1 {
            return Err("cannot delete the last group".to_string());
        }
        let Some(id) = self
            .groups
            .iter()
            .find(|group| group.id.0 as u64 == group_id)
            .map(|group| group.id)
        else {
            return Err(format!("no group with id {group_id}"));
        };
        self.delete_group_now(id, window, cx);
        Ok(())
    }

    /// Renames a group.
    pub fn rename_group_by_id(
        &mut self,
        group_id: u64,
        name: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(group) = self
            .groups
            .iter_mut()
            .find(|group| group.id.0 as u64 == group_id)
        else {
            return false;
        };
        group.name = SharedString::from(name);
        self.save_session(cx);
        cx.notify();
        true
    }

    /// Switches to a group by its numeric id.
    pub fn focus_group_by_id(
        &mut self,
        group_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self
            .groups
            .iter()
            .find(|group| group.id.0 as u64 == group_id)
            .map(|group| group.id)
        else {
            return false;
        };
        self.switch_group(id, window, cx);
        true
    }

    /// Moves a terminal to another group.
    pub fn move_terminal_to_group_by_id(
        &mut self,
        terminal_id: EntityId,
        group_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_group_id) = self
            .groups
            .iter()
            .find(|group| group.id.0 as u64 == group_id)
            .map(|group| group.id)
        else {
            return false;
        };
        if self.terminal_view_by_id(terminal_id).is_none() {
            return false;
        }
        self.move_terminal_to_group(terminal_id, target_group_id, window, cx);
        true
    }

    /// Sets a group's tiling layout from an API name.
    pub fn set_group_layout_by_id(
        &mut self,
        group_id: u64,
        layout: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mode = match layout {
            "manual" | "splits" => GroupLayoutMode::Manual,
            "tall" => GroupLayoutMode::Tall,
            "fat" => GroupLayoutMode::Fat,
            "grid" => GroupLayoutMode::Grid,
            "stack" => GroupLayoutMode::Stack,
            "horizontal" => GroupLayoutMode::Horizontal,
            "vertical" => GroupLayoutMode::Vertical,
            other => return Err(format!("unknown layout {other:?}")),
        };
        let Some(id) = self
            .groups
            .iter()
            .find(|group| group.id.0 as u64 == group_id)
            .map(|group| group.id)
        else {
            return Err(format!("no group with id {group_id}"));
        };
        self.set_group_layout(id, mode, window, cx);
        Ok(())
    }

    /// Enables or disables broadcast input without showing a confirmation
    /// prompt. Returns the affected group id.
    pub fn set_broadcast_input_by_id(
        &mut self,
        group_id: Option<u64>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        let id = match group_id {
            Some(group_id) => self
                .groups
                .iter()
                .find(|group| group.id.0 as u64 == group_id)
                .map(|group| group.id)
                .ok_or_else(|| format!("no group with id {group_id}"))?,
            None => self.active_group_id,
        };
        let Some(group) = self.groups.iter_mut().find(|group| group.id == id) else {
            return Err("active group is missing".to_string());
        };
        group.broadcast_input = enabled;
        self.save_session(cx);
        cx.notify();
        Ok(id.0 as u64)
    }

    /// Machine-readable summary of every group and terminal, used by the IPC
    /// API (`/api/list_sessions`).
    pub fn session_info(&self, cx: &App) -> serde_json::Value {
        let focused = self.focused_terminal_id(cx);
        let groups: Vec<serde_json::Value> = self
            .groups
            .iter()
            .map(|group| {
                let terminals: Vec<serde_json::Value> = group
                    .terminals
                    .iter()
                    .map(|view_entity| {
                        let id = view_entity.entity_id();
                        let view = view_entity.read(cx);
                        let terminal = view.terminal().read(cx);
                        let title = view
                            .custom_title()
                            .map(str::to_string)
                            .unwrap_or_else(|| terminal.title(false));
                        serde_json::json!({
                            "id": id.as_u64(),
                            "title": title,
                            "cwd": terminal
                                .working_directory()
                                .map(|cwd| cwd.display().to_string()),
                            "command": terminal.foreground_process_command_name(),
                            "exit_code": terminal.last_exit_code(),
                            "exited": terminal.has_exited(),
                            "active": focused == Some(id),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "id": group.id.0,
                    "name": group.name.to_string(),
                    "active": group.id == self.active_group_id,
                    "collapsed": group.collapsed,
                    "broadcast_input": group.broadcast_input,
                    "layout": group.layout_mode.api_name(),
                    "terminals": terminals,
                })
            })
            .collect();
        serde_json::json!({
            "active_group_id": self.active_group_id.0,
            "groups": groups,
            "detached": self.detached_sessions_json(cx)["sessions"].clone(),
        })
    }

    /// Closes a terminal without asking for confirmation: remove it from every
    /// pane, then drop the model Entity so the PTY is killed even if the
    /// terminal belonged to an inactive group.
    fn close_terminal_now(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                let has_item = pane
                    .read(cx)
                    .items()
                    .any(|item| item.item_id() == terminal_id);
                if has_item {
                    pane.update(cx, |pane, cx| {
                        pane.remove_item(terminal_id, false, false, window, cx);
                    });
                }
            }
        }
        self.remove_terminal_by_id(terminal_id, window, cx);
    }

    /// Creates a group when the panel has none and records the launch recipe
    /// for a terminal tab moving in from another window.
    pub(crate) fn prepare_for_moved_terminal(
        &mut self,
        view: &Entity<TerminalView>,
        cwd: Option<PathBuf>,
        launch: Option<PersistedLaunch>,
        cx: &mut Context<Self>,
    ) {
        if self.groups.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
                layout_mode: GroupLayoutMode::Manual,
                broadcast_input: false,
                session_terminal_count: None,
                restore_slots: Vec::new(),
            });
            self.active_group_id = id;
        }
        let view_id = view.entity_id();
        self.terminal_spawn_cwds.insert(view_id, cwd);
        if let Some(launch) = launch {
            self.terminal_launches.insert(view_id, launch);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Ensures a terminal moved in from another window is tracked by the
    /// active group, even when `ItemAdded` was skipped (e.g. during session
    /// restore). Idempotent: an already tracked view is left in place.
    pub(crate) fn adopt_moved_terminal(
        &mut self,
        view: &Entity<TerminalView>,
        cwd: Option<PathBuf>,
        launch: Option<PersistedLaunch>,
        cx: &mut Context<Self>,
    ) {
        if self.groups.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
                layout_mode: GroupLayoutMode::Manual,
                broadcast_input: false,
                session_terminal_count: None,
                restore_slots: Vec::new(),
            });
            self.active_group_id = id;
        }
        let view_id = view.entity_id();
        let tracked = self
            .groups
            .iter()
            .any(|group| group.terminals.iter().any(|t| t.entity_id() == view_id));
        if !tracked {
            if let Some(group) = self
                .groups
                .iter_mut()
                .find(|group| group.id == self.active_group_id)
            {
                group.terminals.push(view.clone());
            }
        }
        self.terminal_spawn_cwds.insert(view_id, cwd);
        if let Some(launch) = launch {
            self.terminal_launches.insert(view_id, launch);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Stops or resumes remembering closed terminals around non-close removals.
    pub(crate) fn set_suppress_recently_closed(&mut self, suppress: bool) {
        self.suppress_recently_closed = suppress;
    }

    /// Cwd and launch recipe captured before a terminal leaves this panel.
    pub(crate) fn terminal_move_metadata(
        &self,
        view_id: EntityId,
    ) -> (Option<PathBuf>, Option<PersistedLaunch>) {
        (
            self.terminal_spawn_cwds.get(&view_id).cloned().flatten(),
            self.terminal_launches.get(&view_id).cloned(),
        )
    }

    /// Remembers a terminal that is about to close so it can be reopened.
    fn remember_closed_terminal(&mut self, terminal_id: EntityId, cx: &App) {
        const MAX_RECENTLY_CLOSED: usize = 10;
        let Some(view) = self.terminal_view_by_id(terminal_id) else {
            return;
        };
        let Some(group_id) = self.groups.iter().find_map(|group| {
            group
                .terminals
                .iter()
                .any(|view| view.entity_id() == terminal_id)
                .then_some(group.id)
        }) else {
            return;
        };
        let (cwd, title, color_scheme, font_size) = {
            let view = view.read(cx);
            let terminal = view.terminal().read(cx);
            let cwd = self
                .terminal_spawn_cwds
                .get(&terminal_id)
                .cloned()
                .flatten()
                .or_else(|| terminal.working_directory());
            (
                cwd,
                view.custom_title().map(str::to_owned),
                view.color_scheme().map(str::to_owned),
                view.font_size_override().map(f32::from),
            )
        };
        let launch = self.terminal_launches.get(&terminal_id).cloned();
        if self.recently_closed.len() >= MAX_RECENTLY_CLOSED {
            self.recently_closed.remove(0);
        }
        self.recently_closed.push(ClosedTerminal {
            group_id,
            terminal: PersistedTerminal {
                cwd,
                title,
                color_scheme,
                font_size,
                launch,
                daemon_session: self.terminal_daemon_sessions.get(&terminal_id).cloned(),
                agent_session: self.agent_session_refs.get(&terminal_id).cloned(),
            },
        });
    }

    /// Reopens the most recently closed terminal in its original group when it
    /// still exists, otherwise in the active group. Returns false when there is
    /// nothing left to reopen.
    pub fn reopen_closed_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(closed) = self.recently_closed.pop() else {
            return false;
        };
        let group_id = if self.groups.iter().any(|group| group.id == closed.group_id) {
            closed.group_id
        } else if self
            .groups
            .iter()
            .any(|group| group.id == self.active_group_id)
        {
            self.active_group_id
        } else {
            return false;
        };
        let PersistedTerminal {
            cwd,
            title,
            color_scheme,
            font_size,
            launch,
            daemon_session,
            agent_session,
        } = closed.terminal;
        let init_command = agent_session
            .as_ref()
            .and_then(|session| agent_session_resume_command(&session.agent, &session.value));
        let (profile, launch) = Self::resolve_persisted_launch(launch, cx);
        self.spawn_terminal(
            group_id,
            cwd,
            None,
            None,
            SpawnRestore {
                index: None,
                title,
                color_scheme,
                font_size,
                launch,
                daemon_session,
            },
            init_command,
            profile,
            window,
            cx,
        );
        true
    }

    /// Resolves a persisted launch into the profile and launch record used by
    /// [`Self::spawn_terminal`]. A profile that no longer exists falls back to
    /// the default shell rather than failing.
    fn resolve_persisted_launch(
        launch: Option<PersistedLaunch>,
        cx: &App,
    ) -> (Option<TerminalProfile>, Option<PersistedLaunch>) {
        match launch {
            Some(PersistedLaunch::Profile { name }) => {
                let profile = TerminalSettings::get_global(cx).profile(&name).cloned();
                (profile, Some(PersistedLaunch::Profile { name }))
            }
            Some(PersistedLaunch::Command {
                program,
                args,
                title,
            }) => {
                let launch = PersistedLaunch::Command {
                    program: program.clone(),
                    args: args.clone(),
                    title: title.clone(),
                };
                let profile = TerminalProfile {
                    name: format!("restored:{program}"),
                    shell: Some(task::Shell::WithArguments {
                        program,
                        args,
                        title_override: title,
                    }),
                    working_directory: None,
                    env: Default::default(),
                    command: None,
                    color_scheme: None,
                };
                (Some(profile), Some(launch))
            }
            None => (None, None),
        }
    }

    /// Spawns a shell terminal and attaches it to the given group. The
    /// terminal is only displayed if that group is still active by the time
    /// the (async) terminal is ready.
    ///
    /// When `source` is set, clones that terminal (shell + env) into `cwd`.
    /// When `destination` is set, the new tab is added to that pane (the split
    /// whose "+" was clicked) instead of whatever happens to be active.
    ///
    /// `restore` carries the persisted title, color scheme, font size and
    /// launch origin for a rehydrated terminal.
    fn spawn_terminal(
        &mut self,
        group_id: GroupId,
        cwd: Option<std::path::PathBuf>,
        source: Option<Entity<TerminalView>>,
        destination: Option<Entity<Pane>>,
        restore: SpawnRestore,
        init_command: Option<String>,
        profile: Option<TerminalProfile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SpawnRestore {
            index: restore_index,
            title: restore_title,
            color_scheme: restore_color_scheme,
            font_size: restore_font_size,
            launch: restore_launch,
            daemon_session: restore_daemon_session,
        } = restore;
        let persistent_sessions_enabled = persistent_sessions::is_enabled(cx);
        let launch = restore_launch.or_else(|| {
            profile.as_ref().map(|profile| PersistedLaunch::Profile {
                name: profile.name.clone(),
            })
        });
        let project = self.project.clone();
        let workspace = self.workspace.clone();
        // A profile describes the whole launch environment; don't inherit the
        // active terminal's shell/env on top of it.
        let source_color_scheme = source
            .as_ref()
            .and_then(|view| view.read(cx).color_scheme().map(str::to_owned));
        let profile_color_scheme = profile
            .as_ref()
            .and_then(|profile| profile.color_scheme.clone());
        let source = if profile.is_some() { None } else { source };
        let init_command = profile
            .as_ref()
            .and_then(|profile| profile.command.clone())
            .or(init_command);
        let profile_shell = profile.as_ref().and_then(|profile| profile.shell.clone());
        // Every terminal carries a token so an in-terminal agent integration can
        // attribute its state reports, plus the IPC endpoint it should call.
        let terminal_token = uuid::Uuid::new_v4().to_string();
        let profile_env = {
            let mut env = profile
                .as_ref()
                .map(|profile| profile.env.clone())
                .unwrap_or_default();
            for (key, value) in crate::ipc::connection_env() {
                env.insert(key, value);
            }
            env.insert("TERRY_TERMINAL_TOKEN".to_string(), terminal_token.clone());
            Some(env)
        };
        let profile_working_directory = profile
            .as_ref()
            .and_then(|profile| profile.working_directory.clone());
        // Prefer the explicit destination; fall back to any leftover pin.
        let destination_pane = destination
            .map(|p| p.downgrade())
            .or_else(|| self.pending_spawn_pane.take());
        // Clear pin so a later sync cannot steal a stale target.
        self.pending_spawn_pane = None;
        let spawn_cwd = cwd.clone();
        cx.spawn_in(window, async move |this, cx| {
            let working_directory = match profile_working_directory {
                Some(setting) => workspace.upgrade().and_then(|workspace| {
                    cx.update(|_window, cx| {
                        terminal_view::default_working_directory_with(
                            workspace.read(cx),
                            &setting,
                            cx,
                        )
                    })
                    .ok()
                    .flatten()
                }),
                None => cwd.or_else(|| {
                    workspace.upgrade().and_then(|workspace| {
                        cx.update(|_window, cx| {
                            terminal_view::default_working_directory(workspace.read(cx), cx)
                        })
                        .ok()
                        .flatten()
                    })
                }),
            };
            // Remember the directory the shell actually starts in (after
            // defaulting), not the pre-default Option — otherwise session
            // writes store null and wipe restore paths.
            let remembered_cwd = working_directory.clone();
            let mut spawned_daemon_session: Option<String> = None;
            let daemon_session = restore_daemon_session.clone().or_else(|| {
                persistent_sessions_enabled.then(persistent_sessions::new_session_id)
            });

            let terminal = match daemon_session.clone() {
                Some(daemon_session) => {
                    let (program, args) = match profile_shell.as_ref() {
                        Some(task::Shell::Program(program)) => (Some(program.clone()), Vec::new()),
                        Some(task::Shell::WithArguments { program, args, .. }) => {
                            (Some(program.clone()), args.clone())
                        }
                        Some(task::Shell::System) | None => (None, Vec::new()),
                    };
                    let env = {
                        // The daemon falls back to `$SHELL` when no program is
                        // given, and shell integration is chosen from the
                        // program name. Resolve it the same way here; passing
                        // `/bin/sh` would skip integration (and OSC 7 cwd
                        // tracking) for the common system-shell case.
                        let program_for_env = persistent_sessions::shell_program_for_integration(
                            program.as_deref(),
                            std::env::var("SHELL").ok().as_deref(),
                        );
                        let extra = profile_env.clone().unwrap_or_default();
                        cx.update(|_window, cx| {
                            persistent_sessions::spawn_env(&program_for_env, &extra, cx)
                        })?
                    };
                    let spec = persistent_sessions::session_spec(
                        daemon_session.clone(),
                        working_directory.clone(),
                        program,
                        args,
                        env,
                    );
                    let attached = cx
                        .background_executor()
                        .spawn(async move { persistent_sessions::connect_and_spawn(spec) })
                        .await;

                    match attached {
                        Ok(attached) => {
                            let path_style =
                                project.update(cx, |project, cx| project.path_style(cx))?;
                            let settings = cx
                                .update(|_window, cx| TerminalSettings::get_global(cx).clone())?;
                            let terminal = cx.update(|_window, cx| {
                                let builder = terminal::TerminalBuilder::new_display_only(
                                    settings.cursor_shape,
                                    settings.alternate_scroll,
                                    settings.max_scroll_history_lines,
                                    0,
                                    cx.background_executor(),
                                    path_style,
                                );
                                cx.new(|cx| builder.subscribe(cx))
                            })?;
                            persistent_sessions::wire(
                                &terminal,
                                attached,
                                working_directory.clone(),
                                cx,
                            );
                            spawned_daemon_session = Some(daemon_session);
                            terminal
                        }
                        Err(error) => {
                            log::warn!(
                                "persistent session unavailable, falling back to a local PTY: {error}"
                            );
                            project
                                .update(cx, |project, cx| {
                                    project.create_terminal_shell_with_options(
                                        working_directory,
                                        profile_shell,
                                        profile_env,
                                        cx,
                                    )
                                })?
                                .await?
                        }
                    }
                }
                None => project
                    .update(cx, |project, cx| match source.as_ref() {
                        Some(view) => {
                            let terminal = view.read(cx).terminal().clone();
                            // Give the clone its own token so agent reports are
                            // attributed to the clone, not the source.
                            let env = profile_env.clone().unwrap_or_default();
                            project.clone_terminal_with_env(
                                &terminal,
                                cx,
                                working_directory.clone(),
                                &env,
                            )
                        }
                        None => project.create_terminal_shell_with_options(
                            working_directory,
                            profile_shell,
                            profile_env,
                            cx,
                        ),
                    })?
                    .await?,
            };

            this.update_in(cx, |this, window, cx| {
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let weak_workspace = workspace.read(cx).weak_handle();
                let workspace_id = workspace.read(cx).database_id();
                let weak_project = workspace.read(cx).project().downgrade();
                let terminal_view = cx.new(|cx| {
                    TerminalView::new(
                        terminal.clone(),
                        weak_workspace,
                        workspace_id,
                        weak_project,
                        window,
                        cx,
                    )
                });

                if let Some(session) = spawned_daemon_session {
                    this.terminal_daemon_sessions
                        .insert(terminal_view.entity_id(), session);
                }
                this.terminal_tokens
                    .insert(terminal_token.clone(), terminal_view.entity_id());

                // A profile's color scheme wins; otherwise a cloned terminal
                // keeps the source terminal's colors.
                if let Some(color_scheme) = profile_color_scheme
                    .clone()
                    .or_else(|| source_color_scheme.clone())
                    .or_else(|| restore_color_scheme.clone())
                {
                    terminal_view.update(cx, |view, cx| {
                        view.set_color_scheme(Some(color_scheme), cx);
                    });
                }

                // Remember how this terminal was launched so session writes can
                // replay profiles and `ssh`/`tmux` commands after a restart.
                if let Some(launch) = launch.clone() {
                    let view_id = terminal_view.entity_id();
                    this.terminal_launches.insert(view_id, launch);
                }
                if let Some(size) = restore_font_size {
                    terminal_view.update(cx, |view, cx| {
                        view.set_font_size_override(Some(px(size)), cx);
                    });
                }

                if let Some(title) = restore_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                {
                    terminal_view.update(cx, |view, cx| {
                        view.set_custom_title(Some(title.to_string()), cx);
                    });
                }

                if let Some(command) = init_command.clone() {
                    let startup = terminal.update(cx, |terminal, _cx| {
                        terminal.start_init_command_startup_handshake()
                    });
                    let terminal = terminal.clone();
                    cx.spawn(async move |_view, cx| {
                        let timeout = cx.background_executor().timer(Duration::from_secs(5));
                        futures::select_biased! {
                            _ = startup.fuse() => {}
                            _ = timeout.fuse() => {}
                        }
                        let mut input = command.into_bytes();
                        input.push(b'\x0d');
                        terminal.update(cx, |terminal, cx| {
                            terminal.write_init_command_after_startup(input, cx);
                        });
                    })
                    .detach();
                }

                this.add_terminal_to_group(
                    group_id,
                    terminal_view.clone(),
                    restore_index,
                    remembered_cwd.clone().or(spawn_cwd),
                    cx,
                );

                // Session restore: login-shell rc often `cd ~` after spawn, leaving
                // the PTY in $HOME even when alacritty started in the project path.
                // Re-apply `cd` once the shell is ready, only if still wrong.
                //
                // The injection is deliberately conservative: it happens at most
                // once, waits for the shell to report a directory when it can,
                // and never fires after the user has typed. Blindly retrying used
                // to splice `cd '...'` into whatever the user had started typing
                // (or into a full-screen program they had launched), which looked
                // like garbage being typed on its own.
                if restore_index.is_some() {
                    let path_for_cd = remembered_cwd
                        .clone()
                        .filter(|path| !Self::is_home_cwd(path));
                    if let Some(path) = path_for_cd {
                        let terminal = terminal.clone();
                        cx.spawn(async move |_, cx| {
                            let Some(path_str) = path.to_str().map(str::to_owned) else {
                                return;
                            };
                            let quoted = path_str.replace('\'', "'\\''");
                            let cmd = format!("cd '{quoted}'\n").into_bytes();

                            for (is_last_attempt, delay_ms) in [(false, 400u64), (true, 1500)] {
                                cx.background_executor()
                                    .timer(Duration::from_millis(delay_ms))
                                    .await;

                                let keep_waiting = terminal.update(cx, |term, _cx| {
                                    // Never splice a `cd` into a line the user
                                    // (or their program) is typing on.
                                    if term.keyboard_input_sent() {
                                        return false;
                                    }
                                    match term.working_directory() {
                                        // The shell reported a directory, so it
                                        // is up and idle: fix it if the
                                        // remembered path differs.
                                        Some(live) => {
                                            if live.as_path() != path.as_path() {
                                                term.write_init_command(cmd.clone());
                                            }
                                            false
                                        }
                                        // No directory report yet (OSC 7 shell
                                        // integration): give the shell one
                                        // more chance. On the last attempt
                                        // (integration disabled or an
                                        // unsupported shell) fall back to a
                                        // single best-effort `cd`.
                                        None if !is_last_attempt => true,
                                        None => {
                                            term.write_init_command(cmd.clone());
                                            false
                                        }
                                    }
                                });

                                if !keep_waiting {
                                    return;
                                }
                            }
                        })
                        .detach();
                    }
                }

                if this.session_restoring {
                    this.reveal_active_group_during_restore(window, cx);
                    if this.session_spawns_complete() {
                        cx.defer_in(window, |this, window, cx| {
                            this.finish_session_restore(window, cx);
                        });
                    }
                    return;
                }

                if group_id != this.active_group_id {
                    return;
                }
                if this.pending_layout_restore {
                    return;
                }

                let dest = destination_pane
                    .and_then(|p| p.upgrade())
                    .filter(|pane| workspace.read(cx).panes().iter().any(|p| p == pane));

                if let Some(pane) = dest {
                    let terminal_id = terminal_view.entity_id();
                    let already_visible = workspace
                        .read(cx)
                        .panes()
                        .iter()
                        .any(|p| p.read(cx).items().any(|item| item.item_id() == terminal_id));
                    if !already_visible {
                        // Keep switching through the deferred ItemAdded so
                        // sync_active_group_to_pane does not re-home this tab
                        // onto workspace.active_pane.
                        this.switching = true;
                        pane.update(cx, |pane, cx| {
                            pane.add_item(Box::new(terminal_view), true, true, None, window, cx);
                        });
                        this.display_pane = pane.downgrade();
                        // Auto-tiling groups re-derive the whole split tree
                        // from their terminal list, so only manual groups
                        // record the live split here; the deferred sync below
                        // re-flows the tiles for the new terminal count.
                        let auto_tiling = this
                            .groups
                            .iter()
                            .find(|g| g.id == this.active_group_id)
                            .is_some_and(|g| g.layout_mode != GroupLayoutMode::Manual);
                        if !auto_tiling {
                            if let Some(layout) = this.capture_active_group_layout(cx) {
                                if let Some(group) = this
                                    .groups
                                    .iter_mut()
                                    .find(|g| g.id == this.active_group_id)
                                {
                                    group.saved_layout = Some(layout);
                                }
                            }
                            this.write_session_file(cx, false);
                        }
                        cx.defer_in(window, |this, _window, _cx| {
                            this.switching = false;
                        });
                        if auto_tiling {
                            // Re-flow the tiles so the group's tiling keeps
                            // tracking its own terminal list.
                            cx.defer_in(window, move |this, window, cx| {
                                if this.active_group_id != group_id {
                                    return;
                                }
                                if this.session_restoring || this.pending_layout_restore {
                                    return;
                                }
                                this.sync_active_group_to_pane(window, cx);
                            });
                        }
                        return;
                    }
                }

                cx.defer_in(window, move |this, window, cx| {
                    if this.active_group_id != group_id {
                        return;
                    }
                    if this.session_restoring || this.pending_layout_restore {
                        return;
                    }
                    this.sync_active_group_to_pane(window, cx);
                });
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn on_worktree_added(
        &mut self,
        worktree_id: project::WorktreeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The first worktree turns an empty window into a project workspace, so
        // its session file must move to the project key.
        self.migrate_session_key(cx);

        // Restoring a session must not auto-spawn project terminals — that was
        // inflating group counts across restarts (cwd check fails while PTYs
        // are still coming up).
        if self.session_restoring {
            return;
        }
        let Some(project) = self.project.upgrade() else {
            return;
        };
        let Some(worktree) = project.read(cx).worktree_for_id(worktree_id, cx) else {
            return;
        };
        let worktree = worktree.read(cx);
        if !worktree.is_visible() {
            return;
        }
        let Some(cwd) = Self::cwd_for_worktree(worktree) else {
            return;
        };

        if self.groups.iter().any(|group| {
            group.terminals.iter().any(|tv| {
                tv.read(cx).terminal().read(cx).working_directory().as_ref() == Some(&cwd)
            })
        }) {
            return;
        }

        if self.groups.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
                layout_mode: GroupLayoutMode::Manual,
                broadcast_input: false,
                session_terminal_count: None,
                restore_slots: Vec::new(),
            });
        }
        let group_id = self.active_group_id;
        let profile = self.default_terminal_profile(cx);
        self.spawn_terminal(
            group_id,
            Some(cwd),
            None,
            None,
            SpawnRestore::default(),
            None,
            profile,
            window,
            cx,
        );
        self.save_session(cx);
    }

    fn add_terminal_to_group(
        &mut self,
        group_id: GroupId,
        terminal_view: Entity<TerminalView>,
        restore_index: Option<usize>,
        spawn_cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let view_id = terminal_view.entity_id();
        self.terminal_spawn_cwds.insert(view_id, spawn_cwd);

        // Keep remembered cwd in sync when the shell cds (TitleChanged), and
        // persist to disk so restart does not fall back to $HOME.
        let terminal = terminal_view.read(cx).terminal().clone();
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&terminal, move |this, term, event, cx| {
            if let terminal::Event::LastCommandFinished(code) = event {
                // Ignore the synthetic "command finished" emitted for the
                // initial prompt while a session is still restoring.
                if this.session_restoring
                    || this
                        .restore_cwd_guard_until
                        .is_some_and(|until| std::time::Instant::now() < until)
                {
                    return;
                }
                // Record the finished command in the cross-session history.
                let captured = {
                    let term = term.read(cx);
                    term.last_command_text().map(|command| {
                        (
                            command,
                            term.last_command_output(),
                            term.working_directory()
                                .map(|path| path.display().to_string()),
                        )
                    })
                };
                if let Some((command, output, cwd)) = captured {
                    crate::command_history::CommandHistory::record(command, output, cwd, *code, cx);
                    this.schedule_command_history_persist(cx);
                }
                // A command finished in a terminal the user is not looking
                // at: mark its group so the sidebar shows activity.
                if this.focused_terminal_id(cx) != Some(view_id) {
                    if let Some(group) = this.groups.iter_mut().find(|group| {
                        group
                            .terminals
                            .iter()
                            .any(|view| view.entity_id() == view_id)
                    }) {
                        if !group.has_unread {
                            group.has_unread = true;
                            cx.notify();
                        }
                    }
                }
                return;
            }
            if matches!(event, terminal::Event::Notification(..)) {
                // `OSC 9` progress updates change the progress badge in the
                // sidebar, so re-render even though nothing else changed.
                cx.notify();
                return;
            }
            if !matches!(
                event,
                terminal::Event::TitleChanged
                    | terminal::Event::BreadcrumbsChanged
                    | terminal::Event::CwdChanged
            ) {
                return;
            }
            if matches!(
                event,
                terminal::Event::TitleChanged | terminal::Event::CwdChanged
            ) {
                // The sidebar uses the terminal title and working directory as
                // its label. Display-only terminals (daemon-backed sessions)
                // report directory changes through `CwdChanged` only.
                cx.notify();
            }
            // TitleChanged is emitted right after the background refresh
            // updates the cached cwd, so read the cache instead of
            // force-refreshing again on the UI thread (redundant + blocking).
            let cwd = term
                .read(cx)
                .working_directory()
                .filter(|cwd| !cwd.as_os_str().is_empty());
            let Some(cwd) = cwd else {
                return;
            };
            // Shell init (login rc) often briefly reports $HOME even when we
            // restored a project path — don't let that wipe the session.
            let guarding = this
                .restore_cwd_guard_until
                .is_some_and(|until| std::time::Instant::now() < until);
            if guarding {
                if let Some(prev) = this
                    .terminal_spawn_cwds
                    .get(&view_id)
                    .and_then(|c| c.as_ref())
                {
                    if Self::is_home_cwd(&cwd) && !Self::is_home_cwd(prev) {
                        return;
                    }
                }
            }
            let changed = this
                .terminal_spawn_cwds
                .get(&view_id)
                .and_then(|c| c.as_ref())
                != Some(&cwd);
            this.terminal_spawn_cwds.insert(view_id, Some(cwd));
            if changed {
                this.schedule_cwd_persist(cx);
            }
        }));

        // Broadcast relay: accepted keystrokes from this view are forwarded to
        // its group peers while the group's broadcast-input flag is on. The
        // sender already consumed the keystroke on its own input path, so it
        // is never echoed back through here.
        subscriptions.push(cx.subscribe(
            &terminal_view,
            move |this, sender, event: &terminal::Event, cx| {
                let terminal::Event::UserInput(keystroke, event_type) = event else {
                    return;
                };
                this.forward_broadcast(&sender, keystroke, *event_type, cx);
            },
        ));

        // Track output activity so the sidebar can show whether an agent is
        // actively working or waiting for input.
        subscriptions.push(cx.subscribe(
            &terminal,
            move |this, _term, event: &terminal::Event, cx| {
                if !matches!(event, terminal::Event::Wakeup) {
                    return;
                }
                let now = std::time::Instant::now();
                let was_working = this
                    .terminal_activity
                    .get(&view_id)
                    .is_some_and(|last| now.duration_since(*last) < AGENT_WORKING_WINDOW);
                this.terminal_activity.insert(view_id, now);
                // New output means the agent is no longer waiting for input.
                let was_waiting = this.agent_waiting.remove(&view_id);
                // A quiescent agent that starts producing output is working
                // again. Drop a stale idle state so the badge reacts
                // immediately instead of waiting for the detection debounce.
                // A blocked state is kept until a scan confirms it changed, so
                // typing into a dialog does not re-fire the notification.
                let was_quiescent =
                    matches!(this.agent_states.get(&view_id), Some(AgentState::Idle));
                if was_quiescent {
                    this.agent_states.remove(&view_id);
                }
                if this.agent_detect_task.is_none() {
                    this.schedule_agent_detection(AGENT_DETECT_DEBOUNCE, cx);
                }
                if !was_working || was_waiting || was_quiescent {
                    cx.notify();
                }
                if this.agent_idle_refresh.is_none() {
                    this.ensure_agent_idle_refresh(cx);
                }
            },
        ));
        self._terminal_subscriptions.insert(view_id, subscriptions);

        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            if let Some(index) = restore_index {
                if group.session_terminal_count.is_some() {
                    if group.restore_slots.len() <= index {
                        group.restore_slots.resize_with(index + 1, || None);
                    }
                    group.restore_slots[index] = Some(terminal_view);
                    if group
                        .session_terminal_count
                        .is_some_and(|expected| expected > 0)
                        && group.restore_slots.len() >= group.session_terminal_count.unwrap_or(0)
                        && group.restore_slots.iter().all(|slot| slot.is_some())
                    {
                        group.terminals = group
                            .restore_slots
                            .drain(..)
                            .map(|slot| slot.expect("checked all some"))
                            .collect();
                    }
                } else {
                    // `restore_index` is set for terminals whose spawn started
                    // during session restore, but the restore may have timed
                    // out before they finished. `session_terminal_count` is
                    // cleared at that point, so fall back to appending;
                    // otherwise the view would sit in `restore_slots` forever
                    // and never be shown.
                    group.terminals.push(terminal_view);
                }
            } else {
                group.terminals.push(terminal_view);
            }
        }
        cx.notify();
    }

    /// Toggles a group's broadcast-input mode. Enabling asks for confirmation
    /// because every keystroke is then sent to all terminals in the group.
    fn toggle_broadcast_input(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(broadcast_input) = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| group.broadcast_input)
        else {
            return;
        };

        if broadcast_input {
            if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
                group.broadcast_input = false;
            }
            self.save_session(cx);
            cx.notify();
            return;
        }

        let title = i18n::t("broadcast_input_confirm_title");
        let detail = i18n::t("broadcast_input_confirm_detail");
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[i18n::t_str("enable"), i18n::t_str("cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    if let Some(group) = this.groups.iter_mut().find(|group| group.id == group_id) {
                        group.broadcast_input = true;
                        this.save_session(cx);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Relays `keystroke` from `sender` to every other terminal in its group
    /// while that group's broadcast-input flag is on.
    ///
    /// Forwards the keystroke itself rather than bytes so each peer resolves
    /// it through its own mode: a plain shell, vi mode, or a vim running
    /// inside the PTY all interpret the same key independently.
    fn forward_broadcast(
        &mut self,
        sender: &Entity<TerminalView>,
        keystroke: &Keystroke,
        event_type: terminal::KeyEventType,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.iter().find(|g| {
            g.broadcast_input
                && g.terminals
                    .iter()
                    .any(|t| t.entity_id() == sender.entity_id())
        }) else {
            return;
        };
        let peers = group.terminals.clone();
        let option_as_meta =
            terminal::terminal_settings::TerminalSettings::get_global(cx).option_as_meta;
        for peer in peers {
            if peer.entity_id() == sender.entity_id() {
                continue;
            }
            peer.update(cx, |view, cx| {
                let (handled, vi_mode_enabled) = view.terminal().update(cx, |term, _cx| {
                    (
                        term.try_key_event(keystroke, event_type, option_as_meta),
                        term.vi_mode_enabled(),
                    )
                });
                // Mirror TerminalView::process_keystroke: vi-mode navigation
                // only mutates local cursor state, so an explicit notify is
                // required to re-render.
                if handled && vi_mode_enabled {
                    cx.notify();
                }
            });
        }
    }

    /// Shows only the active group's terminals across all center panes.
    ///
    /// Split panes stay within the active group: a terminal already visible in
    /// any pane is left there (never copied into another pane — that would
    /// share one PTY across splits). Terminals from other groups are removed
    /// from every pane, and empty panes left behind are closed so the active
    /// group's terminals fill the center.
    ///
    /// Prefer [`Self::switch_group`] when changing groups so split layouts are
    /// saved and restored. This method is for in-group updates (spawn, etc.).
    fn sync_active_group_to_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // While a group switch is waiting to capture the outgoing layout, or a
        // session layout restore is pending, do not reconcile — that would
        // strip the outgoing/persisted split and write layout: null.
        if self.pending_switch_from.is_some() || self.pending_layout_restore {
            return;
        }

        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        // Auto-tiling modes derive their layout from the terminal list; when
        // the list changed (spawn / close), rebuild the whole split tree.
        let layout_mode = self
            .groups
            .iter()
            .find(|g| g.id == self.active_group_id)
            .map(|g| g.layout_mode)
            .unwrap_or(GroupLayoutMode::Manual);
        if layout_mode != GroupLayoutMode::Manual {
            let active_group = self
                .groups
                .iter()
                .find(|g| g.id == self.active_group_id)
                .map(|g| g.terminals.len())
                .unwrap_or(0);
            let generated = GroupLayoutMode::generate_layout(layout_mode, active_group);
            let stale = self
                .groups
                .iter()
                .find(|g| g.id == self.active_group_id)
                .is_some_and(|g| g.saved_layout != generated);
            if stale {
                if let Some(group) = self
                    .groups
                    .iter_mut()
                    .find(|g| g.id == self.active_group_id)
                {
                    group.saved_layout = generated.clone();
                }
                if let Some(layout) = &generated {
                    // The rebuild removes/re-adds items; guard so the
                    // ItemRemoved events don't drop them from the model.
                    // Restore the previous value instead of force-clearing:
                    // an outer caller may hold the guard (e.g. switch_group)
                    // and must not have it stripped mid-flight.
                    let was_switching = self.switching;
                    self.switching = true;
                    self.apply_layout_to_center(self.active_group_id, layout, window, cx);
                    self.switching = was_switching;
                }
                self.write_session_file(cx, false);
            }
            cx.notify();
            return;
        }

        let active_terminals: Vec<Entity<TerminalView>> = self
            .groups
            .iter()
            .find(|g| g.id == self.active_group_id)
            .map(|g| g.terminals.clone())
            .unwrap_or_default();
        let active_ids: std::collections::HashSet<EntityId> =
            active_terminals.iter().map(|tv| tv.entity_id()).collect();

        let panes = workspace.read(cx).panes().to_vec();

        let owns_switching_guard = !self.switching;
        if owns_switching_guard {
            self.switching = true;
        }

        for pane in &panes {
            pane.update(cx, |pane, cx| {
                let to_remove: Vec<EntityId> = pane
                    .items()
                    .filter_map(|item| {
                        let id = item.item_id();
                        if item.act_as::<TerminalView>(cx).is_some() && !active_ids.contains(&id) {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect();
                for id in to_remove {
                    pane.remove_item(id, false, false, window, cx);
                }
            });
        }

        let panes = workspace.read(cx).panes().to_vec();
        let mut visible: std::collections::HashSet<EntityId> = std::collections::HashSet::default();
        for pane in &panes {
            for item in pane.read(cx).items() {
                if active_ids.contains(&item.item_id()) {
                    visible.insert(item.item_id());
                }
            }
        }

        // Prefer: pane pinned by NewTerminal ("+" / menu) → active center pane
        // → first pane that already shows this group. Never dump new tabs into
        // an arbitrary first split when the user clicked another one.
        let active_pane = workspace.read(cx).active_pane().clone();
        let has_missing = active_terminals
            .iter()
            .any(|tv| !visible.contains(&tv.entity_id()));
        // Only consume the pin when we are about to place a new tab; other
        // syncs (ItemAdded mid-spawn) must not clear it early.
        let pinned = if has_missing {
            self.pending_spawn_pane
                .take()
                .and_then(|p| p.upgrade())
                .filter(|pane| panes.iter().any(|p| p == pane))
        } else {
            None
        };
        let target_pane = pinned.unwrap_or_else(|| {
            if panes.iter().any(|pane| pane == &active_pane) {
                active_pane
            } else {
                panes
                    .iter()
                    .find(|pane| {
                        pane.read(cx)
                            .items()
                            .any(|item| active_ids.contains(&item.item_id()))
                    })
                    .cloned()
                    .unwrap_or(active_pane)
            }
        });
        self.display_pane = target_pane.downgrade();

        target_pane.update(cx, |pane, cx| {
            for terminal_view in &active_terminals {
                let id = terminal_view.entity_id();
                if visible.contains(&id) {
                    continue;
                }
                pane.add_item(
                    Box::new(terminal_view.clone()),
                    false,
                    false,
                    None,
                    window,
                    cx,
                );
                visible.insert(id);
            }
        });

        // Close leftover panes that are truly empty. `cx.emit(Remove)` is
        // deferred, so we must not loop waiting for panes().len() to shrink
        // (that busy-waits forever). Workspace::remove_pane treats "already
        // gone" as a no-op if several Removes collapse the same split tree.
        //
        // Only close panes with no items at all: a pane that lost its
        // terminals but still holds editors (or other non-terminal tabs) must
        // survive. Closing it would drop those items and lose unsaved work.
        let panes = workspace.read(cx).panes().to_vec();
        if panes.len() > 1 {
            for pane in panes {
                if pane.entity_id() == target_pane.entity_id() {
                    continue;
                }
                if pane.read(cx).items_len() == 0 {
                    pane.update(cx, |_, cx| {
                        cx.emit(workspace::pane::Event::Remove {
                            focus_on_pane: Some(target_pane.clone()),
                        });
                    });
                }
            }
        }

        let focus_item = true;
        // Prefer a terminal the user clicked in the sidebar (across groups),
        // then fall back to the last terminal in the active group.
        let focus_target = self
            .pending_focus_terminal
            .and_then(|id| {
                active_terminals
                    .iter()
                    .find(|tv| tv.entity_id() == id)
                    .cloned()
            })
            .or_else(|| active_terminals.last().cloned());
        if self
            .pending_focus_terminal
            .is_some_and(|id| focus_target.as_ref().is_some_and(|tv| tv.entity_id() == id))
        {
            self.pending_focus_terminal = None;
        }
        if let Some(focus_target) = focus_target {
            let id = focus_target.entity_id();
            let index = target_pane
                .read(cx)
                .items()
                .position(|item| item.item_id() == id);
            if let Some(index) = index {
                target_pane.update(cx, |pane, cx| {
                    pane.activate_item(index, true, focus_item, window, cx);
                });
            } else {
                let panes = workspace.read(cx).panes().to_vec();
                for pane in panes {
                    let index = pane.read(cx).items().position(|item| item.item_id() == id);
                    if let Some(index) = index {
                        pane.update(cx, |pane, cx| {
                            pane.activate_item(index, true, focus_item, window, cx);
                        });
                        break;
                    }
                }
            }
        }

        if owns_switching_guard {
            // Keep saved_layout fresh after in-group splits/tab changes —
            // but never while a session layout restore is still pending
            // (progressive flat sync must not clobber the persisted tree).
            if !self.pending_layout_restore {
                if let Some(layout) = self.capture_active_group_layout(cx) {
                    if let Some(group) = self
                        .groups
                        .iter_mut()
                        .find(|g| g.id == self.active_group_id)
                    {
                        group.saved_layout = Some(layout);
                    }
                    // Persist immediately so every group's split survives
                    // restart even if the user never switches away.
                    self.write_session_file(cx, false);
                }
            }
            self.switching = false;
        }
    }

    /// Schedules a pane sync after the current update stack unwinds, so we
    /// never nest Workspace/Pane updates (which panics in gpui).
    fn defer_sync_active_group_to_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |this, window, cx| {
            this.sync_active_group_to_pane(window, cx);
        });
    }

    fn layout_max_index(node: &GroupLayoutNode) -> Option<usize> {
        match node {
            GroupLayoutNode::Pane { terminals, active } => terminals
                .iter()
                .copied()
                .chain(active.iter().copied())
                .max(),
            GroupLayoutNode::Split { children, .. } => {
                children.iter().filter_map(Self::layout_max_index).max()
            }
        }
    }

    fn active_group_layout_ready(&self) -> bool {
        let Some(group) = self.groups.iter().find(|g| g.id == self.active_group_id) else {
            return false;
        };
        if let Some(expected) = group.session_terminal_count {
            if group.terminals.len() < expected {
                return false;
            }
        }
        match &group.saved_layout {
            None => true,
            Some(layout) => match Self::layout_max_index(layout) {
                None => true,
                // Indices are 0-based; need len > max (== max+1 terminals).
                Some(max) => group.terminals.len() > max,
            },
        }
    }

    fn capture_layout_from_member(
        member: &Member,
        id_to_index: &HashMap<EntityId, usize>,
        cx: &App,
    ) -> Option<GroupLayoutNode> {
        match member {
            Member::Pane(pane) => {
                let terminals: Vec<usize> = pane
                    .read(cx)
                    .items()
                    .filter_map(|item| id_to_index.get(&item.item_id()).copied())
                    .collect();
                if terminals.is_empty() {
                    return None;
                }
                let active = pane
                    .read(cx)
                    .active_item()
                    .and_then(|item| id_to_index.get(&item.item_id()).copied());
                Some(GroupLayoutNode::Pane { terminals, active })
            }
            Member::Axis(axis) => {
                let children: Vec<GroupLayoutNode> = axis
                    .members
                    .iter()
                    .filter_map(|child| Self::capture_layout_from_member(child, id_to_index, cx))
                    .collect();
                match children.len() {
                    0 => None,
                    1 => children.into_iter().next(),
                    _ => Some(GroupLayoutNode::Split {
                        axis: PersistedAxis::from(axis.axis),
                        flexes: axis.flexes.lock().clone(),
                        children,
                    }),
                }
            }
        }
    }

    fn capture_active_group_layout(&self, cx: &App) -> Option<GroupLayoutNode> {
        self.capture_group_layout(self.active_group_id, cx)
    }

    fn capture_group_layout(&self, group_id: GroupId, cx: &App) -> Option<GroupLayoutNode> {
        let group = self.groups.iter().find(|g| g.id == group_id)?;
        if group.terminals.is_empty() {
            return None;
        }
        // Auto-tiling layouts are derived from the terminal list, not from the
        // live panes (which may be mid-reconcile).
        if group.layout_mode != GroupLayoutMode::Manual {
            return GroupLayoutMode::generate_layout(group.layout_mode, group.terminals.len());
        }
        let workspace = self.workspace.upgrade()?;
        let id_to_index: HashMap<EntityId, usize> = group
            .terminals
            .iter()
            .enumerate()
            .map(|(i, tv)| (tv.entity_id(), i))
            .collect();
        Self::capture_layout_from_member(&workspace.read(cx).center_root(), &id_to_index, cx)
    }

    fn clear_center_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let panes = workspace.read(cx).panes().to_vec();
        for pane in panes {
            pane.update(cx, |pane, cx| {
                let ids: Vec<EntityId> = pane
                    .items()
                    .filter_map(|item| item.act_as::<TerminalView>(cx).map(|tv| tv.entity_id()))
                    .collect();
                for id in ids {
                    pane.remove_item(id, false, false, window, cx);
                }
            });
        }
    }

    fn build_layout_member(
        workspace: &mut Workspace,
        node: &GroupLayoutNode,
        terminals: &[Entity<TerminalView>],
        window: &mut Window,
        cx: &mut gpui::Context<Workspace>,
    ) -> (Member, Entity<Pane>, Option<usize>) {
        match node {
            GroupLayoutNode::Pane {
                terminals: indices,
                active,
            } => {
                let pane = workspace.create_center_pane(window, cx);
                for &ix in indices {
                    if let Some(tv) = terminals.get(ix) {
                        pane.update(cx, |pane, cx| {
                            pane.add_item(Box::new(tv.clone()), false, false, None, window, cx);
                        });
                    }
                }
                if let Some(active_ix) = active {
                    if let Some(tv) = terminals.get(*active_ix) {
                        let id = tv.entity_id();
                        let index = pane.read(cx).items().position(|item| item.item_id() == id);
                        if let Some(index) = index {
                            pane.update(cx, |pane, cx| {
                                pane.activate_item(index, true, true, window, cx);
                            });
                        }
                    }
                }
                (Member::Pane(pane.clone()), pane, *active)
            }
            GroupLayoutNode::Split {
                axis,
                flexes,
                children,
            } => {
                let mut members = Vec::new();
                let mut focus_pane = None;
                let mut focus_terminal = None;
                for child in children {
                    let (member, pane, active) =
                        Self::build_layout_member(workspace, child, terminals, window, cx);
                    if focus_pane.is_none() {
                        focus_pane = Some(pane.clone());
                    }
                    if active.is_some() {
                        focus_pane = Some(pane);
                        focus_terminal = active;
                    }
                    members.push(member);
                }
                let focus_pane = focus_pane
                    .or_else(|| {
                        members.first().and_then(|m| match m {
                            Member::Pane(p) => Some(p.clone()),
                            _ => None,
                        })
                    })
                    .expect("split layout has children");
                let axis = PaneAxis::load((*axis).into(), members, Some(flexes.clone()));
                (Member::Axis(axis), focus_pane, focus_terminal)
            }
        }
    }

    /// Session templates are saved per group. The CLI calls this "session"
    /// save/load; each template recreates one workflow group.
    pub fn list_session_templates(&self) -> serde_json::Value {
        let templates: Vec<_> = load_templates()
            .into_iter()
            .map(|template| {
                serde_json::json!({
                    "name": template.name,
                    "layout": template.layout_mode.map(|mode| mode.api_name()),
                    "broadcast_input": template.broadcast_input,
                    "terminals": template.terminals.iter().map(|terminal| {
                        serde_json::json!({
                            "title": terminal.title,
                            "cwd": terminal.cwd.as_ref().map(|cwd| cwd.display().to_string()),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::json!({ "templates": templates })
    }

    pub fn save_session_template(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("session name must not be empty".to_string());
        }
        if !self
            .groups
            .iter()
            .any(|group| group.id == self.active_group_id)
        {
            return Err("no active group".to_string());
        }
        self.save_active_group_as_template(name, cx);
        Ok(())
    }

    pub fn load_session_template(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        let template = load_templates()
            .into_iter()
            .find(|template| template.name == name)
            .ok_or_else(|| format!("no session template named {name:?}"))?;
        self.add_group_from_template(template, window, cx);
        Ok(self.active_group_id.0 as u64)
    }

    pub fn delete_session_template(&mut self, name: &str) -> Result<(), String> {
        let mut templates = load_templates();
        let original_len = templates.len();
        templates.retain(|template| template.name != name);
        if templates.len() == original_len {
            return Err(format!("no session template named {name:?}"));
        }
        write_templates(&templates);
        Ok(())
    }

    /// Captures the active group (terminals, launch origins, cwd and layout)
    /// as a reusable template. Re-saving under the same name overwrites it.
    fn save_active_group_as_template(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(group) = self
            .groups
            .iter()
            .find(|group| group.id == self.active_group_id)
        else {
            return;
        };
        let group_name = group.name.clone();
        let layout_mode = Some(group.layout_mode);
        let broadcast_input = group.broadcast_input;
        let layout = group
            .saved_layout
            .clone()
            .or_else(|| self.capture_active_group_layout(cx));

        let mut terminals = Vec::new();
        for view in &group.terminals {
            let id = view.entity_id();
            let view_ref = view.read(cx);
            let cwd = view_ref.terminal().read(cx).working_directory();
            let title = view_ref.custom_title().map(|title| title.to_string());
            let launch = self.terminal_launches.get(&id).cloned();
            terminals.push(PersistedTerminal {
                cwd,
                title,
                color_scheme: view_ref.color_scheme().map(str::to_owned),
                font_size: view_ref.font_size_override().map(f32::from),
                launch,
                // Templates create fresh terminals, never reattach to a daemon.
                daemon_session: None,
                agent_session: None,
            });
        }

        let template = WorkspaceTemplate {
            name,
            terminals,
            layout,
            layout_mode,
            broadcast_input,
        };
        let mut templates = load_templates();
        templates.retain(|existing| existing.name != template.name);
        templates.push(template);
        write_templates(&templates);
        log::info!("workspace templates: saved template from group {group_name}");
        cx.notify();
    }

    /// Creates a new group from `template` and spawns its terminals.
    fn add_group_from_template(
        &mut self,
        template: WorkspaceTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group_id = self.new_group_id();
        self.groups.push(TerminalGroup {
            id: group_id,
            name: template.name.clone().into(),
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: template.layout.clone(),
            layout_mode: template.layout_mode.unwrap_or(GroupLayoutMode::Manual),
            broadcast_input: template.broadcast_input,
            session_terminal_count: None,
            restore_slots: Vec::new(),
        });
        self.active_group_id = group_id;

        let mut spawned = 0;
        for terminal in template.terminals {
            self.spawn_terminal(
                group_id,
                terminal.cwd.clone(),
                None,
                None,
                SpawnRestore {
                    index: None,
                    title: terminal.title.clone(),
                    color_scheme: terminal.color_scheme.clone(),
                    font_size: terminal.font_size,
                    launch: terminal.launch.clone(),
                    daemon_session: None,
                },
                None,
                None,
                window,
                cx,
            );
            spawned += 1;
        }
        // An empty template still yields a usable shell.
        if spawned == 0 {
            self.spawn_terminal(
                group_id,
                None,
                None,
                None,
                SpawnRestore::default(),
                None,
                None,
                window,
                cx,
            );
        }

        self.restore_active_group_layout(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Applies a tmux layout tree to the group mirroring `window_id`.
    /// `pane_order` maps the order panes were added to their tmux pane ids, so
    /// layout leaves land on the right terminal.
    pub(crate) fn apply_tmux_layout(
        &mut self,
        window_id: u32,
        pane_order: &[u32],
        layout: &crate::tmux_control::Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&group_id) = self.tmux_window_groups.get(&window_id) else {
            return;
        };
        let mut fallback = 0usize;
        let Some(node) = tmux_layout_to_node(layout, pane_order, &mut fallback) else {
            return;
        };
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.saved_layout = Some(node);
            group.layout_mode = GroupLayoutMode::Manual;
        }
        if group_id == self.active_group_id {
            self.restore_active_group_layout(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Renames the group mirroring a tmux window, keeping the active group in
    /// sync when needed.
    pub(crate) fn rename_tmux_group(
        &mut self,
        window_id: u32,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(&group_id) = self.tmux_window_groups.get(&window_id) else {
            return;
        };
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.name = name.into();
            self.save_session(cx);
            cx.notify();
        }
    }

    /// Drops every group and terminal created by the tmux bridge, so a
    /// reconnecting client can rebuild them from scratch.
    pub(crate) fn reset_tmux_mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let group_ids: Vec<GroupId> = self.tmux_window_groups.drain().map(|(_, id)| id).collect();
        let mirrored: Vec<EntityId> = self.tmux_mirrored.drain().collect();

        for view_id in &mirrored {
            self._terminal_subscriptions.remove(view_id);
            self.terminal_activity.remove(view_id);
            self.terminal_launches.remove(view_id);
            self.terminal_spawn_cwds.remove(view_id);
            self.agent_waiting.remove(view_id);
            self.agent_states.remove(view_id);
        }
        self.suppress_recently_closed = true;
        if let Some(pane) = self.display_pane_entity(cx) {
            for view_id in &mirrored {
                let view_id = *view_id;
                pane.update(cx, |pane, cx| {
                    pane.remove_item(view_id, false, false, window, cx);
                });
            }
        }
        self.suppress_recently_closed = false;
        for group_id in group_ids {
            if let Some(index) = self.groups.iter().position(|group| group.id == group_id) {
                self.groups.remove(index);
            }
        }
        if !self
            .groups
            .iter()
            .any(|group| group.id == self.active_group_id)
        {
            if let Some(first) = self.groups.first() {
                self.active_group_id = first.id;
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Creates the group for a tmux window that appeared after attach, so its
    /// layout and panes can be mirrored.
    pub(crate) fn tmux_window_added(&mut self, window_id: u32, cx: &mut Context<Self>) {
        self.ensure_tmux_group(window_id, cx);
    }

    /// Records the working directory of a mirrored tmux pane that was created
    /// from a layout event before its pane list entry arrived.
    pub(crate) fn note_mirrored_pane_cwd(
        &mut self,
        terminal_id: EntityId,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(view_id) = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .find(|view| view.read(cx).terminal().entity_id() == terminal_id)
            .map(|view| view.entity_id())
        else {
            return;
        };
        self.terminal_spawn_cwds.insert(view_id, cwd);
        self.save_session(cx);
    }

    /// Removes one mirrored pane (its tmux pane was closed) without disturbing
    /// the rest of the tmux window.
    pub(crate) fn remove_tmux_pane(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((group_id, view)) = self.groups.iter().find_map(|group| {
            group
                .terminals
                .iter()
                .find(|view| view.read(cx).terminal().entity_id() == terminal_id)
                .map(|view| (group.id, (*view).clone()))
        }) else {
            return;
        };
        let view_id = view.entity_id();
        self.tmux_mirrored.remove(&view_id);
        self._terminal_subscriptions.remove(&view_id);
        self.terminal_activity.remove(&view_id);
        self.terminal_launches.remove(&view_id);
        self.terminal_spawn_cwds.remove(&view_id);
        self.agent_waiting.remove(&view_id);
        self.agent_states.remove(&view_id);
        if group_id == self.active_group_id {
            self.suppress_recently_closed = true;
            if let Some(pane) = self.display_pane_entity(cx) {
                pane.update(cx, |pane, cx| {
                    pane.remove_item(view_id, false, false, window, cx);
                });
            }
            self.suppress_recently_closed = false;
        } else if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.terminals.retain(|view| view.entity_id() != view_id);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Removes the group mirroring a tmux window that was closed in tmux.
    pub(crate) fn remove_tmux_window(
        &mut self,
        window_id: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group_id) = self.tmux_window_groups.remove(&window_id) else {
            return;
        };
        let view_ids: Vec<EntityId> = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| {
                group
                    .terminals
                    .iter()
                    .map(|view| view.entity_id())
                    .collect()
            })
            .unwrap_or_default();

        for view_id in &view_ids {
            self._terminal_subscriptions.remove(view_id);
            self.terminal_activity.remove(view_id);
            self.terminal_launches.remove(view_id);
            self.terminal_spawn_cwds.remove(view_id);
            self.agent_waiting.remove(view_id);
            self.agent_states.remove(view_id);
            self.tmux_mirrored.remove(view_id);
        }
        self.suppress_recently_closed = true;
        if let Some(pane) = self.display_pane_entity(cx) {
            for view_id in view_ids {
                pane.update(cx, |pane, cx| {
                    pane.remove_item(view_id, false, false, window, cx);
                });
            }
        }
        self.suppress_recently_closed = false;
        if let Some(index) = self.groups.iter().position(|group| group.id == group_id) {
            self.groups.remove(index);
        }
        if !self
            .groups
            .iter()
            .any(|group| group.id == self.active_group_id)
        {
            if let Some(first) = self.groups.first() {
                self.active_group_id = first.id;
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Returns the group mirroring `window_id`, creating it on first use.
    fn ensure_tmux_group(&mut self, window_id: u32, cx: &mut Context<Self>) -> GroupId {
        if let Some(&group_id) = self.tmux_window_groups.get(&window_id) {
            return group_id;
        }
        let group_id = self.new_group_id();
        let became_first = self.groups.is_empty();
        self.groups.push(TerminalGroup {
            id: group_id,
            name: SharedString::from(format!("tmux @{window_id}")),
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: None,
            layout_mode: GroupLayoutMode::Grid,
            broadcast_input: false,
            session_terminal_count: None,
            restore_slots: Vec::new(),
        });
        self.tmux_window_groups.insert(window_id, group_id);
        if became_first {
            self.active_group_id = group_id;
        }
        self.save_session(cx);
        cx.notify();
        group_id
    }

    /// Creates a mirrored pane terminal (for example a tmux pane in control
    /// mode) and adds it to the active group. Input is routed to `client` via
    /// `send-keys`, output is written with `write_output`.
    pub(crate) fn add_mirrored_pane(
        &mut self,
        window_id: u32,
        pane_id: u32,
        cwd: Option<PathBuf>,
        buffered_output: Vec<u8>,
        client: crate::tmux_native::SharedTmuxClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<terminal::Terminal>> {
        let path_style = self.project.upgrade()?.read(cx).path_style(cx);
        let settings = TerminalSettings::get_global(cx).clone();
        let terminal = cx.new(|cx| {
            let builder = terminal::TerminalBuilder::new_display_only(
                settings.cursor_shape,
                settings.alternate_scroll,
                settings.max_scroll_history_lines,
                0,
                cx.background_executor(),
                path_style,
            );
            builder.subscribe(cx)
        });

        let pane = crate::tmux_control::PaneId(pane_id);
        let input_client = client.clone();
        terminal.update(cx, |terminal, _cx| {
            terminal.set_input_sink(Some(std::sync::Arc::new(move |bytes: &[u8]| {
                let text = String::from_utf8_lossy(bytes).to_string();
                if text.is_empty() {
                    return;
                }
                // Resolve the client on every keystroke so input keeps working
                // across reconnects.
                let Some(client) = input_client.lock().ok().and_then(|slot| slot.clone()) else {
                    return;
                };
                let Ok(mut client) = client.lock() else {
                    return;
                };
                let command = if text == "\r" {
                    crate::tmux_control::send_key(pane, "Enter")
                } else {
                    crate::tmux_control::send_keys_literal(pane, &text)
                };
                let _ = client.send(&command);
            })));
        });

        if !buffered_output.is_empty() {
            terminal.update(cx, |terminal, cx| {
                terminal.write_output(&buffered_output, cx);
            });
        }

        let project = self.project.clone();
        let view = cx.new(|cx| {
            TerminalView::new(
                terminal.clone(),
                WeakEntity::new_invalid(),
                None,
                project,
                window,
                cx,
            )
        });

        let group_id = self.ensure_tmux_group(window_id, cx);
        let view_id = view.entity_id();
        let first_pane = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .is_none_or(|group| group.terminals.is_empty());
        self.add_terminal_to_group(group_id, view, None, cwd, cx);
        self.tmux_mirrored.insert(view_id);
        if first_pane && group_id == self.active_group_id {
            self.set_group_layout(group_id, GroupLayoutMode::Grid, window, cx);
        }
        self.save_session(cx);
        cx.notify();
        Some(terminal)
    }

    fn restore_active_group_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let mut terminals = Vec::new();
        let mut layout = None;
        if let Some(group) = self.groups.iter().find(|g| g.id == self.active_group_id) {
            terminals = group.terminals.clone();
            layout = group.saved_layout.clone();
            // Auto-tiling modes always rebuild from the current terminal list.
            if group.layout_mode != GroupLayoutMode::Manual {
                layout = GroupLayoutMode::generate_layout(group.layout_mode, group.terminals.len());
                if let Some(group) = self
                    .groups
                    .iter_mut()
                    .find(|g| g.id == self.active_group_id)
                {
                    group.saved_layout = layout.clone();
                }
            }
        }

        if terminals.is_empty() {
            self.clear_center_terminals(window, cx);
            // Keep every pane that still holds non-terminal items (editors,
            // previews, …). Only collapse panes that are now completely empty;
            // dropping a pane with items would discard unsaved work.
            let panes = workspace.read(cx).panes().to_vec();
            let keep = panes
                .iter()
                .find(|pane| pane.read(cx).items_len() > 0)
                .cloned()
                .or_else(|| panes.first().cloned());
            if let Some(keep) = keep {
                for pane in panes {
                    if pane.entity_id() == keep.entity_id() {
                        continue;
                    }
                    if pane.read(cx).items_len() == 0 {
                        pane.update(cx, |_, cx| {
                            cx.emit(workspace::pane::Event::Remove {
                                focus_on_pane: Some(keep.clone()),
                            });
                        });
                    }
                }
            }
            return;
        }

        match layout {
            Some(layout) => {
                self.apply_layout_to_center(self.active_group_id, &layout, window, cx);
                // Terminals created after the layout was saved still need a home.
                self.sync_active_group_to_pane(window, cx);
            }
            None => {
                self.sync_active_group_to_pane(window, cx);
            }
        }
    }

    /// Replaces the workspace center layout with the given split tree, which
    /// belongs to `group_id` (its indices index into that group's `terminals`).
    /// Resolving the group explicitly keeps tiling from ever being applied
    /// against another group's terminal list.
    fn apply_layout_to_center(
        &mut self,
        group_id: GroupId,
        layout: &GroupLayoutNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Workspace events from this rebuild are dispatched asynchronously
        // (flush_effects) after the update stack unwinds. gpui's effect queue
        // is FIFO, so the decrement queued here runs BEFORE the pane events
        // this rebuild emits — the depth guard below can be down by the time
        // those events reach on_workspace_event. The real protection against
        // those events dropping model terminals is the still-visible check in
        // on_workspace_event's ItemRemoved arm; the depth guard remains as a
        // secondary filter for PaneAdded/PaneRemoved refreshes.
        self.reflowing_layout_depth += 1;
        cx.defer_in(window, |this, _window, cx| {
            this.reflowing_layout_depth -= 1;
            cx.notify();
        });

        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let terminals = match self.groups.iter().find(|g| g.id == group_id) {
            Some(group) => group.terminals.clone(),
            None => return,
        };

        // Collect non-terminal items (editors, previews, …) before the center
        // tree is replaced. `replace_center_layout` drops the old panes and
        // everything they hold, so without this a group switch or a tiling
        // reflow would silently close editors and discard unsaved changes.
        // Strong item handles keep the underlying views alive; they are
        // re-attached to the new focus pane below.
        let preserved_items: Vec<Box<dyn ItemHandle>> = {
            let mut seen: HashSet<EntityId> = HashSet::new();
            let mut items = Vec::new();
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                for item in pane.read(cx).items() {
                    if item.act_as::<TerminalView>(cx).is_some() {
                        continue;
                    }
                    if seen.insert(item.item_id()) {
                        items.push(item.boxed_clone());
                    }
                }
            }
            items
        };

        self.clear_center_terminals(window, cx);
        workspace.update(cx, |workspace, cx| {
            let (root, focus_pane, _) =
                Self::build_layout_member(workspace, layout, &terminals, window, cx);
            let new_center = PaneGroup::with_root(root);
            workspace.replace_center_layout(new_center, focus_pane.clone(), window, cx);

            // Re-home preserved editor-like tabs as regular tabs in the new
            // focus pane. Their previous split geometry cannot be restored
            // (the group layout only describes terminals), but keeping the
            // items alive and visible avoids losing unsaved buffers.
            if !preserved_items.is_empty() {
                focus_pane.update(cx, |pane, cx| {
                    let mut index = pane.items_len();
                    for item in preserved_items {
                        pane.add_item(item, false, false, Some(index), window, cx);
                        index += 1;
                    }
                });
            }
        });
        self.display_pane = workspace.read(cx).active_pane().downgrade();
    }

    /// Switches to the group `offset` positions away, wrapping at the ends.
    fn switch_group_offset(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.groups.is_empty() {
            return;
        }
        let current = self
            .groups
            .iter()
            .position(|group| group.id == self.active_group_id)
            .unwrap_or(0) as isize;
        let len = self.groups.len() as isize;
        let next = (current + offset).rem_euclid(len) as usize;
        let group_id = self.groups[next].id;
        self.switch_group(group_id, window, cx);
        self.show_group_switch_hint(group_id, cx);
    }

    /// Switches the display pane to show the given group's terminals as tabs,
    /// preserving each group's split layout across switches.
    fn switch_group(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.groups.iter().any(|g| g.id == group_id) {
            return;
        }

        if group_id == self.active_group_id {
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
                group.collapsed = false;
                group.has_unread = false;
            }
            cx.notify();
            return;
        }

        // Group-header switches (no sidebar terminal click) must not keep a
        // stale focus target from an earlier incomplete cross-group click.
        self.pending_focus_terminal = None;

        let outgoing = self.active_group_id;
        // Block sync until we have snapshotted `outgoing`'s live split layout.
        // Otherwise a deferred sync (ItemAdded / spawn) clears those panes and
        // the capture writes layout: null for every group except the last one.
        self.pending_switch_from = Some(outgoing);
        self.active_group_id = group_id;
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
            group.has_unread = false;
        }
        cx.notify();

        cx.defer_in(window, move |this, window, cx| {
            // Snapshot this switch's outgoing group while sync is still blocked
            // (pending_switch_from is Some), so its panes are intact.
            if let Some(layout) = this.capture_group_layout(outgoing, cx) {
                if let Some(group) = this.groups.iter_mut().find(|g| g.id == outgoing) {
                    group.saved_layout = Some(layout);
                }
            }

            // Only the switch that owns the current pending flag may clear it
            // and perform the restore (handles rapid A→B→C switches).
            if this.pending_switch_from == Some(outgoing) {
                this.pending_switch_from = None;
            }

            if this.active_group_id != group_id || this.pending_switch_from.is_some() {
                this.write_session_file(cx, false);
                return;
            }

            this.switching = true;
            this.restore_active_group_layout(window, cx);
            this.switching = false;
            this.write_session_file(cx, false);
            cx.notify();
        });
    }

    /// Activates a specific terminal's tab in whichever pane is showing it.
    fn focus_terminal(
        &mut self,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_id = terminal_view.entity_id();
        self.clear_unread_for_terminal(target_id);
        for pane in self.center_panes(cx) {
            let index = pane
                .read(cx)
                .items()
                .position(|item| item.item_id() == target_id);
            if let Some(index) = index {
                pane.update(cx, |pane, cx| {
                    pane.activate_item(index, true, true, window, cx);
                });
                return;
            }
        }
    }

    fn clear_unread_for_terminal(&mut self, terminal_id: EntityId) {
        if let Some(group) = self.groups.iter_mut().find(|group| {
            group
                .terminals
                .iter()
                .any(|view| view.entity_id() == terminal_id)
        }) {
            group.has_unread = false;
        }
    }

    /// Toggles the collapsed state of a group without switching to it.
    fn toggle_group_collapse(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = !group.collapsed;
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Focuses a terminal, switching to its group first if needed.
    fn focus_terminal_in_group(
        &mut self,
        group_id: GroupId,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_waiting.remove(&terminal_view.entity_id()) {
            cx.notify();
        }
        if group_id != self.active_group_id {
            // switch_group restores the destination layout asynchronously;
            // focus_terminal would no-op now because the target is not in the
            // center panes yet. Remember the click and apply it in sync.
            self.switch_group(group_id, window, cx);
            self.pending_focus_terminal = Some(terminal_view.entity_id());
            return;
        }
        self.focus_terminal(terminal_view, window, cx);
    }

    fn start_renaming(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter().find(|g| g.id == group_id) {
            let name = group.name.to_string();
            self.renaming_group_id = Some(group_id);
            self.rename_editor.update(cx, |editor, cx| {
                editor.set_text(name.clone(), window, cx);
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(0)..MultiBufferOffset(name.len())])
                });
            });
            window.focus(&self.rename_editor.read(cx).focus_handle(cx), cx);
            cx.notify();
        }
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(group_id) = self.renaming_group_id.take() {
            let new_name = self.rename_editor.read(cx).text(cx);
            if !new_name.trim().is_empty() {
                if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
                    group.name = new_name.into();
                }
                self.save_session(cx);
            }
            cx.notify();
        }
    }

    fn move_group_up(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index > 0 {
                self.groups.swap(index, index - 1);
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    fn move_group_down(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index + 1 < self.groups.len() {
                self.groups.swap(index, index + 1);
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    /// Removes a terminal from its group but keeps its PTY, scrollback and
    /// running processes alive so it can be reattached later.
    fn detach_terminal(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_restoring {
            return;
        }
        let Some(group_ix) = self.groups.iter().position(|group| {
            group
                .terminals
                .iter()
                .any(|view| view.entity_id() == terminal_id)
        }) else {
            return;
        };
        let group_id = self.groups[group_ix].id;
        let Some(removed_ix) = self.groups[group_ix]
            .terminals
            .iter()
            .position(|view| view.entity_id() == terminal_id)
        else {
            return;
        };

        let view = self.groups[group_ix].terminals.remove(removed_ix);
        let title = sidebar_terminal_name(&view, cx);
        let cwd = self
            .terminal_spawn_cwds
            .get(&terminal_id)
            .cloned()
            .flatten()
            .or_else(|| view.read(cx).terminal().read(cx).working_directory());

        // Manual layouts store indices into `group.terminals`; keep them
        // pointing at the same terminals after the list shifts left.
        if self.groups[group_ix].layout_mode == GroupLayoutMode::Manual {
            if let Some(layout) = self.groups[group_ix].saved_layout.as_mut() {
                Self::remap_layout_after_remove(layout, removed_ix);
            }
        }
        if self.groups[group_ix].terminals.is_empty() {
            self.groups[group_ix].has_unread = false;
            if self.groups[group_ix].layout_mode == GroupLayoutMode::Manual {
                self.groups[group_ix].saved_layout = None;
            }
        }

        // Only the active group's terminals live in the center panes.
        if group_id == self.active_group_id {
            if let Some(workspace) = self.workspace.upgrade() {
                let panes = workspace.read(cx).panes().to_vec();
                for pane in panes {
                    let shows_terminal = pane
                        .read(cx)
                        .items()
                        .any(|item| item.item_id() == terminal_id);
                    if shows_terminal {
                        pane.update(cx, |pane, cx| {
                            pane.remove_item(terminal_id, false, false, window, cx);
                        });
                    }
                }
            }
            self.defer_sync_active_group_to_pane(window, cx);
        }

        self.detached_sessions
            .push(DetachedSession { view, title, cwd });
        self.save_session(cx);
        cx.notify();
    }

    /// Puts a detached terminal back into the active group without recreating
    /// its view or PTY.
    fn reattach_detached(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.detached_sessions.len() {
            return;
        }
        if self.groups.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
                layout_mode: GroupLayoutMode::Manual,
                broadcast_input: false,
                session_terminal_count: None,
                restore_slots: Vec::new(),
            });
            self.active_group_id = id;
        }
        let session = self.detached_sessions.remove(index);
        let group_id = self.active_group_id;
        self.prepare_group_for_terminal(group_id, window, cx);
        self.add_terminal_to_group(group_id, session.view, None, session.cwd, cx);
        self.defer_sync_active_group_to_pane(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Drops a detached session for good, releasing its PTY.
    fn close_detached_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.detached_sessions.len() {
            return;
        }
        let session = self.detached_sessions.remove(index);
        let id = session.view.entity_id();
        if let Some(daemon_session) = self.terminal_daemon_sessions.remove(&id) {
            Self::kill_daemon_session(daemon_session);
        }
        self._terminal_subscriptions.remove(&id);
        self.terminal_activity.remove(&id);
        self.terminal_spawn_cwds.remove(&id);
        self.terminal_launches.remove(&id);
        self.agent_waiting.remove(&id);
        self.agent_states.remove(&id);
        drop(session.view);
        cx.notify();
    }

    /// Moves a group to `target`'s slot in the sidebar (drag and drop).
    fn reorder_group(&mut self, dragged: GroupId, target: GroupId, cx: &mut Context<Self>) {
        if dragged == target {
            return;
        }
        let (Some(from), Some(to)) = (
            self.groups.iter().position(|group| group.id == dragged),
            self.groups.iter().position(|group| group.id == target),
        ) else {
            return;
        };
        let group = self.groups.remove(from);
        let insert_at = to.min(self.groups.len());
        self.groups.insert(insert_at, group);
        self.save_session(cx);
        cx.notify();
    }

    /// Shows a transient "index/total · name" banner after a group shortcut.
    fn show_group_switch_hint(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        let Some(index) = self.groups.iter().position(|group| group.id == group_id) else {
            return;
        };
        let text: SharedString = format!(
            "{}/{} · {}",
            index + 1,
            self.groups.len(),
            self.groups[index].name
        )
        .into();
        self.switch_hint_generation += 1;
        let token = self.switch_hint_generation;
        self.group_switch_hint = Some((text, token));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;
            this.update(cx, |this, cx| {
                if this
                    .group_switch_hint
                    .as_ref()
                    .is_some_and(|(_, hint_token)| *hint_token == token)
                {
                    this.group_switch_hint = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_workspace_event(
        &mut self,
        event: &workspace::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A tab closed directly in the center pane must be dropped from the
        // model too. During a group switch the removals are intentional, so we
        // must not treat them as user closes or the outgoing group's terminals
        // would be dropped from the model and vanish on switch.
        if self.switching
            || self.reflowing_layout_depth > 0
            || self.pending_switch_from.is_some()
            || self.session_restoring
        {
            cx.notify();
            return;
        }
        match event {
            workspace::Event::ActiveItemChanged { .. } => {
                if let Some(id) = self.focused_terminal_id(cx) {
                    self.clear_unread_for_terminal(id);
                }
            }
            workspace::Event::ItemRemoved { item_id } => {
                // `apply_layout_to_center` removes every tile and re-adds it to
                // its new pane within the same update. Its guard decrement is
                // queued before the pane events it must cover (gpui effects
                // flush FIFO), so the guards above can already be down when
                // this handler runs. A rebuild always re-homes its tiles
                // before the events are processed — pane state is final — so
                // treat a removal as a user close only when the item is no
                // longer visible in any center pane. Otherwise a tiling
                // re-flow (clone / set layout / spawn) would drop the group's
                // terminals from the model and delete the emptied group.
                let still_visible = self
                    .center_panes(cx)
                    .iter()
                    .any(|pane| pane.read(cx).items().any(|item| item.item_id() == *item_id));
                if still_visible {
                    cx.notify();
                    return;
                }
                let mut belongs_to_active_group = false;
                for group in &self.groups {
                    if group.id == self.active_group_id {
                        let has_item = group.terminals.iter().any(|tv| tv.entity_id() == *item_id);
                        if has_item {
                            belongs_to_active_group = true;
                        }
                        break;
                    }
                }
                if belongs_to_active_group {
                    self.remove_terminal_by_id(*item_id, window, cx);
                }
            }
            workspace::Event::ItemAdded { item } => {
                // Terminals opened outside the panel (e.g. NewCenterTerminal
                // still handled by terminal_view) must join the active group,
                // and extras from other groups must leave the pane.
                if let Some(tv) = item.act_as::<TerminalView>(cx) {
                    let id = tv.entity_id();
                    let already = self
                        .groups
                        .iter()
                        .any(|g| g.terminals.iter().any(|t| t.entity_id() == id));
                    if already {
                        // Already tracked (e.g. "+" placement into a specific
                        // split). Do not sync — that would move the tab to
                        // workspace.active_pane.
                        return;
                    }
                    let spawn_cwd = tv.read(cx).terminal().read(cx).working_directory();
                    self.add_terminal_to_group(self.active_group_id, tv, None, spawn_cwd, cx);
                    self.save_session(cx);
                    // ItemAdded is emitted while Workspace is updating the
                    // pane — sync must wait until that stack unwinds.
                    self.defer_sync_active_group_to_pane(window, cx);
                }
            }
            // MovePane splits do not emit ItemAdded; still refresh layout.
            workspace::Event::PaneAdded(_) | workspace::Event::PaneRemoved => {
                cx.defer_in(window, |this, _window, cx| {
                    if this.switching
                        || this.reflowing_layout_depth > 0
                        || this.pending_switch_from.is_some()
                        || this.session_restoring
                        || this.pending_layout_restore
                    {
                        return;
                    }
                    this.refresh_active_layout_and_save(cx);
                });
            }
            _ => {}
        }
        cx.notify();
    }

    fn remember_agent_session(
        &mut self,
        agent_id: &'static str,
        bin: &'static str,
        cwd: Option<PathBuf>,
    ) {
        self.remembered_agent_sessions
            .retain(|session| !(session.agent_id == agent_id && session.cwd == cwd));
        self.remembered_agent_sessions.push(RememberedAgentSession {
            agent_id,
            bin,
            cwd,
            pending_resume: true,
        });
        if self.remembered_agent_sessions.len() > REMEMBERED_AGENT_SESSION_LIMIT {
            let overflow = self.remembered_agent_sessions.len() - REMEMBERED_AGENT_SESSION_LIMIT;
            self.remembered_agent_sessions.drain(0..overflow);
        }
    }

    fn clear_pending_resume(&mut self, cwd: Option<&std::path::Path>) {
        for session in &mut self.remembered_agent_sessions {
            let matches = match cwd {
                Some(cwd) => session.cwd.as_deref() == Some(cwd),
                None => true,
            };
            if matches {
                session.pending_resume = false;
            }
        }
    }

    /// Consumes the one-shot resume recorded when an agent terminal closed.
    /// Returns the allowlisted binary, the session directory, and the resume
    /// command template.
    fn take_pending_resume(
        &mut self,
        cwd: Option<&std::path::Path>,
    ) -> Option<(&'static str, Option<PathBuf>, &'static str)> {
        let index = if let Some(cwd) = cwd {
            self.remembered_agent_sessions.iter().rposition(|session| {
                session.pending_resume
                    && session.cwd.as_deref() == Some(cwd)
                    && agent_by_id(session.agent_id)
                        .and_then(|agent| agent.resume)
                        .is_some()
            })
        } else {
            self.remembered_agent_sessions.iter().rposition(|session| {
                session.pending_resume
                    && agent_by_id(session.agent_id)
                        .and_then(|agent| agent.resume)
                        .is_some()
            })
        }?;
        let session = &mut self.remembered_agent_sessions[index];
        session.pending_resume = false;
        let template = agent_by_id(session.agent_id)?.resume?;
        Some((session.bin, session.cwd.clone(), template))
    }

    fn group_agent_cwd(&self, group_id: GroupId, cx: &App) -> Option<PathBuf> {
        let group = self.groups.iter().find(|group| group.id == group_id)?;
        let view = group.terminals.last()?;
        view.read(cx)
            .terminal()
            .read(cx)
            .working_directory()
            .or_else(|| {
                self.terminal_spawn_cwds
                    .get(&view.entity_id())
                    .cloned()
                    .flatten()
            })
    }

    fn agent_session_actions(&self, group_id: GroupId, cx: &App) -> Option<AgentSessionActions> {
        let cwd = self.group_agent_cwd(group_id, cx);
        let live = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(|group| group.terminals.last())
            .and_then(|view| coding_agent_from_view(view, cx));
        let remembered = if let Some(cwd) = cwd.as_deref() {
            self.remembered_agent_sessions
                .iter()
                .rfind(|session| session.cwd.as_deref() == Some(cwd))
        } else {
            self.remembered_agent_sessions.last()
        };
        let (agent, bin, action_cwd) = if let Some((agent, bin)) = live {
            (agent, bin, cwd.clone())
        } else if let Some(session) = remembered {
            let agent = agent_by_id(session.agent_id)?;
            (agent, session.bin, session.cwd.clone().or(cwd))
        } else {
            return None;
        };
        let has_parked = self.parked_agent_sessions.iter().any(|session| {
            !session.view.read(cx).terminal().read(cx).has_exited()
                && match action_cwd.as_deref() {
                    Some(cwd) => session.cwd.as_deref() == Some(cwd),
                    None => true,
                }
        });
        let resume_command = agent.resume.map(|template| agent_command(template, bin));
        let fork = agent.fork.map(|template| agent_command(template, bin));
        let past = agent.past.map(|template| agent_command(template, bin));
        if !has_parked && resume_command.is_none() && fork.is_none() && past.is_none() {
            return None;
        }
        Some(AgentSessionActions {
            cwd: action_cwd,
            resume: has_parked || resume_command.is_some(),
            resume_command,
            fork,
            past,
        })
    }

    fn resume_agent_session(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(actions) = self.agent_session_actions(group_id, cx) else {
            return;
        };
        if let Some(view) = self.take_parked_agent_session(actions.cwd.as_deref(), cx) {
            self.prepare_group_for_terminal(group_id, window, cx);
            self.clear_pending_resume(actions.cwd.as_deref());
            self.add_terminal_to_group(group_id, view, None, actions.cwd, cx);
            self.defer_sync_active_group_to_pane(window, cx);
            self.save_session(cx);
            return;
        }
        if let Some(command) = actions.resume_command {
            self.clear_pending_resume(actions.cwd.as_deref());
            self.spawn_agent_command(group_id, actions.cwd, command, window, cx);
        }
    }

    fn fork_agent_session(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(actions) = self.agent_session_actions(group_id, cx) else {
            return;
        };
        let Some(command) = actions.fork else {
            return;
        };
        self.spawn_agent_command(group_id, actions.cwd, command, window, cx);
    }

    fn past_agent_sessions(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(actions) = self.agent_session_actions(group_id, cx) else {
            return;
        };
        let Some(command) = actions.past else {
            return;
        };
        self.spawn_agent_command(group_id, actions.cwd, command, window, cx);
    }

    /// Opens the SSH host picker for the active group. When `remote_tmux` is
    /// set, the selected host is launched into an attach-or-create tmux session
    /// instead of a plain shell.
    pub fn open_ssh_picker(
        &mut self,
        remote_tmux: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let targets = crate::remote_hosts::ssh_targets(TerminalSettings::get_global(cx));
        let panel = cx.entity();
        workspace.update(cx, |workspace, cx| {
            crate::terminal_picker::TerminalPicker::open(
                workspace,
                panel,
                crate::terminal_picker::TerminalPickerMode::SshHosts {
                    targets,
                    remote_tmux,
                },
                window,
                cx,
            );
        });
    }

    /// Lists local tmux sessions off the UI thread, then opens the picker.
    pub fn open_tmux_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tmux_picker_mode(false, window, cx);
    }

    /// Like [`Self::open_tmux_picker`], optionally mirroring panes natively.
    pub fn open_tmux_picker_mode(
        &mut self,
        native: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let sessions = cx
                .background_executor()
                .spawn(async { crate::remote_hosts::list_tmux_sessions() })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let panel = cx.entity();
                let mode =
                    crate::terminal_picker::TerminalPickerMode::TmuxSessions { sessions, native };
                workspace.update(cx, |workspace, cx| {
                    crate::terminal_picker::TerminalPicker::open(
                        workspace, panel, mode, window, cx,
                    );
                });
            })
            .ok();
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// Attaches a tmux session using control mode so its panes become native
    /// terminals.
    pub fn attach_tmux_control_session(
        &mut self,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::tmux_native::attach(session, window, cx);
    }

    /// Opens the color-scheme picker for the focused terminal.
    pub fn open_color_scheme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.active_terminal_view(cx) else {
            return;
        };
        let terminal_id = view.entity_id();
        self.open_color_scheme_picker_for(terminal_id, window, cx);
    }

    /// Opens the color-scheme picker for a specific terminal.
    pub fn open_color_scheme_picker_for(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(view) = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .find(|view| view.entity_id() == terminal_id)
            .cloned()
        else {
            return;
        };
        let current = view.read(cx).color_scheme().map(str::to_owned);
        let panel = cx.entity();
        workspace.update(cx, |workspace, cx| {
            crate::terminal_picker::TerminalPicker::open(
                workspace,
                panel,
                crate::terminal_picker::TerminalPickerMode::ColorSchemes {
                    terminal_id,
                    current,
                },
                window,
                cx,
            );
        });
    }

    /// Imports Alacritty / iTerm2 / Terry JSON color scheme files, saves them
    /// into the user scheme directory, then opens the picker.
    pub fn import_color_scheme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import terminal color scheme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return anyhow::Ok(());
            };
            let mut imported = Vec::new();
            for path in paths {
                match terminal::color_scheme::parse_scheme_file(&path) {
                    Some(scheme) => match terminal::color_scheme::save_user_scheme(&scheme) {
                        Ok(saved) => {
                            log::info!("imported terminal color scheme into {saved:?}");
                            imported.push(scheme);
                        }
                        Err(error) => {
                            log::warn!("could not save terminal color scheme: {error:#}");
                        }
                    },
                    None => log::warn!("unrecognized terminal color scheme: {path:?}"),
                }
            }
            this.update_in(cx, |this, window, cx| {
                if let Some(registry) = terminal::color_scheme::ColorSchemeRegistry::try_global(cx)
                {
                    for scheme in imported {
                        registry.insert(scheme);
                    }
                }
                this.open_color_scheme_picker(window, cx);
            })
            .ok();
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// Applies `name` (a terminal color scheme or theme name, or `None` for the
    /// UI theme) to one terminal and persists it with the session.
    pub fn set_terminal_color_scheme(
        &mut self,
        terminal_id: EntityId,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .find(|view| view.entity_id() == terminal_id)
            .cloned()
        else {
            return;
        };
        view.update(cx, |view, cx| view.set_color_scheme(name, cx));
        self.save_session(cx);
        cx.notify();
    }

    /// Connects to `target` in the active group, optionally attaching a remote
    /// tmux session so the shell survives a dropped connection.
    pub fn connect_ssh(
        &mut self,
        target: crate::remote_hosts::SshTarget,
        remote_tmux: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (program, args, title) = {
            let settings = TerminalSettings::get_global(cx);
            let options = crate::remote_hosts::SshOptions {
                reuse_connections: settings.ssh_reuse_connections,
                extra_args: &settings.ssh_extra_args,
            };
            crate::remote_hosts::ssh_invocation(&target, options, remote_tmux.as_deref())
        };
        crate::remote_hosts::record_recent_host(&target.host);
        let launch = PersistedLaunch::Command {
            program: program.clone(),
            args: args.clone(),
            title: Some(title.clone()),
        };
        self.spawn_program_terminal(program, args, title, Some(launch), window, cx);
    }

    /// Attaches to (or creates) a local tmux session in the active group.
    pub fn attach_tmux_session(
        &mut self,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (program, args, title) = crate::remote_hosts::local_tmux_invocation(&session);
        let launch = PersistedLaunch::Command {
            program: program.clone(),
            args: args.clone(),
            title: Some(title.clone()),
        };
        self.spawn_program_terminal(program, args, title, Some(launch), window, cx);
    }

    /// Spawns a terminal whose "shell" is a direct program (ssh, tmux, ...).
    fn spawn_program_terminal(
        &mut self,
        program: String,
        args: Vec<String>,
        title: String,
        launch: Option<PersistedLaunch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group_id = self.active_group_id;
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        let profile = TerminalProfile {
            name: format!("program:{title}"),
            shell: Some(task::Shell::WithArguments {
                program,
                args,
                title_override: Some(title),
            }),
            working_directory: None,
            env: Default::default(),
            command: None,
            color_scheme: None,
        };
        self.prepare_group_for_terminal(group_id, window, cx);
        self.spawn_terminal(
            group_id,
            None,
            None,
            None,
            SpawnRestore {
                launch,
                ..Default::default()
            },
            None,
            Some(profile),
            window,
            cx,
        );
        self.save_session(cx);
    }

    fn prepare_group_for_terminal(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if group_id != self.active_group_id {
            self.switch_group(group_id, window, cx);
        }
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.collapsed = false;
        }
    }

    fn spawn_agent_command(
        &mut self,
        group_id: GroupId,
        cwd: Option<PathBuf>,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        let source = self.active_terminal_view(cx);
        self.prepare_group_for_terminal(group_id, window, cx);
        self.spawn_terminal(
            group_id,
            cwd,
            source,
            None,
            SpawnRestore::default(),
            Some(command),
            None,
            window,
            cx,
        );
        self.save_session(cx);
    }

    fn take_parked_agent_session(
        &mut self,
        cwd: Option<&std::path::Path>,
        cx: &App,
    ) -> Option<Entity<TerminalView>> {
        self.parked_agent_sessions
            .retain(|session| !session.view.read(cx).terminal().read(cx).has_exited());
        let index = if let Some(cwd) = cwd {
            self.parked_agent_sessions
                .iter()
                .rposition(|session| session.cwd.as_deref() == Some(cwd))
        } else {
            self.parked_agent_sessions.len().checked_sub(1)
        }?;
        Some(self.parked_agent_sessions.remove(index).view)
    }

    fn remove_terminal_by_id(
        &mut self,
        item_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A terminal moving to another window keeps its session: the receiving
        // panel takes it over rather than starting a new shell.
        let moving_to_another_window = self.suppress_recently_closed;
        if !self.suppress_recently_closed {
            self.remember_closed_terminal(item_id, cx);
        }
        self.notice_detached_terminal(item_id, cx);
        self._terminal_subscriptions.remove(&item_id);
        self.terminal_activity.remove(&item_id);
        self.agent_waiting.remove(&item_id);
        self.agent_states.remove(&item_id);
        self.reported_agent_states.remove(&item_id);
        self.agent_session_refs.remove(&item_id);
        self.terminal_tokens.retain(|_, id| *id != item_id);
        let daemon_session = self.terminal_daemon_sessions.remove(&item_id);
        let mut changed = false;
        let mut emptied_group: Option<GroupId> = None;
        let mut removed_from_active_auto = false;
        let mut parked: Option<ParkedAgentSession> = None;
        let mut remembered: Option<(&'static str, &'static str, Option<PathBuf>)> = None;
        let mut reconnect: Option<(GroupId, Option<PathBuf>, PersistedLaunch)> = None;
        for group in &mut self.groups {
            if let Some(removed_ix) = group
                .terminals
                .iter()
                .position(|tv| tv.entity_id() == item_id)
            {
                let removed = group.terminals.remove(removed_ix);
                let remembered_cwd = self.terminal_spawn_cwds.remove(&item_id).flatten();
                let launch = self.terminal_launches.remove(&item_id);
                let exited = removed.read(cx).terminal().read(cx).has_exited();
                let cwd = remembered_cwd
                    .or_else(|| removed.read(cx).terminal().read(cx).working_directory());
                if exited {
                    if let Some(launch_value) = launch {
                        if let PersistedLaunch::Command { program, .. } = &launch_value {
                            if is_remote_program(program) {
                                reconnect = Some((group.id, cwd.clone(), launch_value));
                            }
                        }
                    }
                }
                if let Some((agent, bin)) = coding_agent_from_view(&removed, cx) {
                    remembered = Some((agent.id, bin, cwd.clone()));
                }
                if should_park_agent_session(&removed, cx) {
                    parked = Some(ParkedAgentSession { view: removed, cwd });
                }
                if group.layout_mode == GroupLayoutMode::Manual {
                    if let Some(layout) = group.saved_layout.as_mut() {
                        Self::remap_layout_after_remove(layout, removed_ix);
                    }
                } else {
                    // Leave the applied tree stale: the deferred sync below
                    // compares it against a fresh tree for the group's own
                    // terminal list and re-flows the tiles when they differ.
                    removed_from_active_auto = group.id == self.active_group_id;
                }
                if group.terminals.is_empty() {
                    emptied_group = Some(group.id);
                }
                changed = true;
                break;
            }
        }
        let was_parked = parked.is_some();
        if let Some(session) = parked {
            // A parked agent keeps running, so its daemon session keeps its
            // mapping and stays reachable once the view is reattached.
            let parked_id = session.view.entity_id();
            if let Some(daemon_session) = &daemon_session {
                self.terminal_daemon_sessions
                    .insert(parked_id, daemon_session.clone());
            }
            self.parked_agent_sessions.push(session);
            if self.parked_agent_sessions.len() > PARKED_AGENT_SESSION_LIMIT {
                let overflow = self.parked_agent_sessions.len() - PARKED_AGENT_SESSION_LIMIT;
                let dropped: Vec<EntityId> = self
                    .parked_agent_sessions
                    .drain(0..overflow)
                    .map(|session| session.view.entity_id())
                    .collect();
                // Dropping a parked view releases its PTY locally; a daemon
                // session would otherwise keep running with no view left.
                for id in dropped {
                    if let Some(session) = self.terminal_daemon_sessions.remove(&id) {
                        Self::kill_daemon_session(session);
                    }
                }
            }
        }
        if let Some((agent_id, bin, cwd)) = remembered {
            self.remember_agent_session(agent_id, bin, cwd);
        }
        if !changed {
            return;
        }

        // Closing a terminal ends its shell. Without this the daemon keeps the
        // PTY (and whatever it is running) alive, and reopening the terminal
        // reattaches to that same busy shell, so anything typed afterwards
        // queues behind the abandoned command. Explicit detach and parked agent
        // sessions keep their processes on purpose.
        if !was_parked && !moving_to_another_window {
            if let Some(session) = daemon_session {
                // Reopening must start a fresh shell: the old session is gone,
                // and reusing its id could race the kill below.
                for closed in self.recently_closed.iter_mut() {
                    if closed.terminal.daemon_session.as_deref() == Some(session.as_str()) {
                        closed.terminal.daemon_session = None;
                    }
                }
                Self::kill_daemon_session(session);
            }
        }

        // Auto-tiling groups need a re-flow once the pane removal settles.
        if removed_from_active_auto && !self.session_restoring && self.pending_switch_from.is_none()
        {
            self.defer_sync_active_group_to_pane(window, cx);
        }

        // Drop empty groups (keep at least one).
        if let Some(empty_id) = emptied_group
            && self.groups.len() > 1
        {
            if let Some(index) = self.groups.iter().position(|g| g.id == empty_id) {
                let was_active = self.active_group_id == empty_id;
                self.groups.remove(index);
                if was_active {
                    let next_index = index.min(self.groups.len().saturating_sub(1));
                    let next_id = self.groups[next_index].id;
                    self.active_group_id = next_id;
                    self.pending_switch_from = None;
                    self.switching = true;
                    if let Some(group) = self.groups.iter_mut().find(|g| g.id == next_id) {
                        group.collapsed = false;
                    }
                    self.restore_active_group_layout(window, cx);
                    self.switching = false;
                }
            }
        }

        self.save_session(cx);
        cx.notify();

        if let Some((group_id, cwd, launch)) = reconnect {
            self.schedule_remote_reconnect(group_id, cwd, launch, window, cx);
        }
    }

    /// Re-spawns a remote terminal after its connection dropped, backing off
    /// exponentially and giving up after a few attempts so a dead host cannot
    /// loop forever.
    fn schedule_remote_reconnect(
        &mut self,
        group_id: GroupId,
        cwd: Option<PathBuf>,
        launch: PersistedLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PersistedLaunch::Command { program, args, .. } = &launch else {
            return;
        };
        let key = format!("{program}:{}", args.join(" "));
        let now = std::time::Instant::now();
        let attempts = {
            let entry = self
                .remote_reconnects
                .entry(key.clone())
                .or_insert((0, now));
            // Forget old failures so a long-lived session starts fresh.
            if now.duration_since(entry.1) > Duration::from_secs(300) {
                *entry = (0, now);
            }
            entry.0 += 1;
            entry.1 = now;
            entry.0
        };
        if attempts > MAX_REMOTE_RECONNECT_ATTEMPTS {
            log::warn!("remote reconnect: giving up on {key} after {attempts} attempts");
            return;
        }
        let delay = Duration::from_secs(1u64 << (attempts - 1).min(4));
        log::info!("remote reconnect: retrying {key} in {delay:?} (attempt {attempts})");

        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| {
                if !this.groups.iter().any(|group| group.id == group_id) {
                    return;
                }
                this.spawn_terminal(
                    group_id,
                    cwd,
                    None,
                    None,
                    SpawnRestore {
                        launch: Some(launch),
                        ..SpawnRestore::default()
                    },
                    None,
                    None,
                    window,
                    cx,
                );
                this.save_session(cx);
            })
            .ok();
        })
        .detach();
    }

    fn remap_layout_after_remove(node: &mut GroupLayoutNode, removed: usize) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                terminals.retain(|&i| i != removed);
                for i in terminals.iter_mut() {
                    if *i > removed {
                        *i -= 1;
                    }
                }
                match active {
                    Some(a) if *a == removed => *active = None,
                    Some(a) if *a > removed => *a -= 1,
                    _ => {}
                }
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::remap_layout_after_remove(child, removed);
                }
            }
        }
    }
}

impl Focusable for TerminalListPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TerminalListPanel {}

impl Render for TerminalListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active_group_id = self.active_group_id;
        let group_switch_hint = self.group_switch_hint.clone().map(|(text, _)| text);
        let detached_snapshot: Vec<(usize, SharedString, Option<AgentBadge>, Option<i64>)> = self
            .detached_sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                (
                    index,
                    session.title.clone(),
                    agent_badge_for(
                        &session.view,
                        &self.terminal_activity,
                        &self.agent_waiting,
                        &self.agent_states,
                        cx,
                    ),
                    session.view.read(cx).notification_progress(),
                )
            })
            .collect();

        let active_terminal_id = self
            .display_pane_entity(cx)
            .and_then(|pane| pane.read(cx).active_item())
            .and_then(|item| item.downcast::<TerminalView>())
            .map(|tv| tv.entity_id());

        // Snapshot everything needed for rendering so we don't borrow `self`
        // while building click listeners.
        let mut groups_snapshot: Vec<(
            GroupId,
            SharedString,
            bool,
            bool,
            bool,
            Vec<(
                Entity<TerminalView>,
                SharedString,
                Option<AgentBadge>,
                Option<i64>,
                Option<GitBadge>,
                bool,
            )>,
        )> = Vec::new();
        for group in &self.groups {
            let is_active = group.id == active_group_id;
            let collapsed = group.collapsed;
            let has_unread = group.has_unread;
            let mut terminals = Vec::new();
            if !collapsed {
                for tv in &group.terminals {
                    let badge = agent_badge_for(
                        tv,
                        &self.terminal_activity,
                        &self.agent_waiting,
                        &self.agent_states,
                        cx,
                    );
                    let progress = tv.read(cx).notification_progress();
                    let cwd = tv.read(cx).terminal().read(cx).working_directory();
                    let git = self.git_badge_for(cwd.as_deref(), cx);
                    let recording = crate::session_recording::is_recording(tv.entity_id(), cx);
                    terminals.push((
                        tv.clone(),
                        sidebar_terminal_name(tv, cx),
                        badge,
                        progress,
                        git,
                        recording,
                    ));
                }
            }
            groups_snapshot.push((
                group.id,
                group.name.clone(),
                is_active,
                collapsed,
                has_unread,
                terminals,
            ));
        }

        let mut rows: Vec<AnyElement> = Vec::new();
        for (group_id, name, is_active, collapsed, has_unread, terminals) in groups_snapshot {
            let _colors = theme.colors().clone();
            let is_expanded = !collapsed;
            let panel = cx.entity().clone();
            let is_renaming = Some(group_id) == self.renaming_group_id;
            let rename_editor = self.rename_editor.clone();
            let drag_name = name.clone();

            let on_group_click = cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    this.start_renaming(group_id, window, cx);
                } else {
                    this.switch_group(group_id, window, cx);
                }
            });
            let on_chevron_click = cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_group_collapse(group_id, cx);
            });
            let on_rename_confirm =
                cx.listener(|this, _: &menu::Confirm, _window, cx| this.commit_rename(cx));
            let on_rename_cancel =
                cx.listener(|this, _: &menu::Cancel, _window, cx| this.commit_rename(cx));

            rows.push(
                right_click_menu(format!("group-rc-{}", group_id.0))
                    .trigger({
                        let panel = panel.clone();
                        move |_is_open, _window, cx| {
                            div()
                                .id(SharedString::from(format!("group-{}", group_id.0)))
                                .px_2()
                                .py_1()
                                .mx_1()
                                .rounded_md()
                                .cursor_pointer()
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "group-chevron-{}",
                                                    group_id.0
                                                )))
                                                .child(
                                                    ui::Icon::new(if is_expanded {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .size(IconSize::Small)
                                                    .color(Color::Muted),
                                                )
                                                .on_click(on_chevron_click),
                                        )
                                        .child(
                                            ui::Icon::new(if is_active {
                                                IconName::FolderOpen
                                            } else {
                                                IconName::Folder
                                            })
                                            .size(IconSize::Small)
                                            .color(
                                                if is_active {
                                                    Color::Accent
                                                } else {
                                                    Color::Muted
                                                },
                                            ),
                                        )
                                        .map(|this| {
                                            if is_renaming {
                                                this.child(
                                                    div()
                                                        .w_full()
                                                        .h_5()
                                                        .bg(cx.theme().colors().editor_background)
                                                        .on_action(on_rename_confirm)
                                                        .on_action(on_rename_cancel)
                                                        .child(rename_editor),
                                                )
                                            } else {
                                                this.child(
                                                    Label::new(name)
                                                        .size(LabelSize::Small)
                                                        .truncate(),
                                                )
                                            }
                                        })
                                        .child(
                                            h_flex()
                                                .flex_grow(1.)
                                                .justify_end()
                                                .when(has_unread, |this| {
                                                    this.child(
                                                        Label::new("●")
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Accent),
                                                    )
                                                })
                                                .child(
                                                    PopoverMenu::new(SharedString::from(format!(
                                                        "menu-{}",
                                                        group_id.0
                                                    )))
                                                    .trigger(
                                                        IconButton::new(
                                                            SharedString::from(format!(
                                                                "menu-btn-{}",
                                                                group_id.0
                                                            )),
                                                            IconName::Ellipsis,
                                                        )
                                                        .icon_size(IconSize::Small),
                                                    )
                                                    .menu({
                                                        let panel = panel.clone();
                                                        move |window, cx| {
                                                            Some(Self::build_group_context_menu(
                                                                panel.clone(),
                                                                group_id,
                                                                window,
                                                                cx,
                                                            ))
                                                        }
                                                    }),
                                                ),
                                        ),
                                )
                                .on_drag(
                                    DraggedGroup {
                                        id: group_id,
                                        name: drag_name,
                                    },
                                    |dragged, _, _, cx| {
                                        cx.new(|_| DraggedGroupPreview {
                                            name: dragged.name.clone(),
                                        })
                                    },
                                )
                                .on_drop({
                                    let panel = panel.downgrade();
                                    move |dragged: &DraggedGroup, _window, cx: &mut App| {
                                        panel
                                            .update(cx, |this, cx| {
                                                this.reorder_group(dragged.id, group_id, cx);
                                            })
                                            .ok();
                                    }
                                })
                                .on_click(on_group_click)
                        }
                    })
                    .menu({
                        let panel = panel.clone();
                        move |window, cx| {
                            Self::build_group_context_menu(panel.clone(), group_id, window, cx)
                        }
                    })
                    .into_any_element(),
            );

            if is_expanded {
                for (ix, (terminal_view, title, agent_badge, progress, git_badge, recording)) in
                    terminals.into_iter().enumerate()
                {
                    let is_terminal_active = Some(terminal_view.entity_id()) == active_terminal_id;

                    let on_click = cx.listener({
                        let terminal_view = terminal_view.clone();
                        move |this, _, window, cx| {
                            this.focus_terminal_in_group(
                                group_id,
                                terminal_view.clone(),
                                window,
                                cx,
                            );
                        }
                    });

                    rows.push(
                        right_click_menu(format!("term-rc-{}-{ix}", group_id.0))
                            .trigger(move |_is_open, _window, _cx| {
                                div()
                                    .id(SharedString::from(format!("term-{}-{ix}", group_id.0)))
                                    // Match group row padding; spacer below equals the
                                    // chevron so the terminal icon lines up with Folder.
                                    .px_2()
                                    .py_1()
                                    .mx_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(!is_terminal_active, |el| {
                                        el.hover(|style| {
                                            style.bg(_cx.theme().colors().element_hover)
                                        })
                                    })
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(div().size(IconSize::Small.rems()))
                                            .child(
                                                ui::Icon::new(IconName::Terminal)
                                                    .size(IconSize::Small)
                                                    .color(if is_terminal_active {
                                                        Color::Accent
                                                    } else {
                                                        Color::Muted
                                                    }),
                                            )
                                            .child(
                                                div().flex_1().min_w_0().child(
                                                    Label::new(title.clone())
                                                        .size(LabelSize::Small)
                                                        .truncate(),
                                                ),
                                            )
                                            .map({
                                                move |this| {
                                                    if recording {
                                                        this.child(
                                                            Label::new("\u{25cf}")
                                                                .size(LabelSize::XSmall)
                                                                .color(Color::Error),
                                                        )
                                                    } else {
                                                        this
                                                    }
                                                }
                                            })
                                            .map({
                                                let git_badge = git_badge.clone();
                                                move |this| match git_badge {
                                                    Some(badge) => this.child(
                                                        h_flex()
                                                            .gap_1()
                                                            .items_center()
                                                            .child(
                                                                Icon::new(IconName::GitBranch)
                                                                    .size(IconSize::Indicator)
                                                                    .color(Color::Muted),
                                                            )
                                                            .child(
                                                                Label::new(badge.branch)
                                                                    .size(LabelSize::XSmall)
                                                                    .color(Color::Muted),
                                                            )
                                                            .when(badge.dirty > 0, |this| {
                                                                this.child(
                                                                    Label::new("\u{25cf}")
                                                                        .size(LabelSize::XSmall)
                                                                        .color(Color::Warning),
                                                                )
                                                            }),
                                                    ),
                                                    None => this,
                                                }
                                            })
                                            .map({
                                                move |this| match progress {
                                                    Some(progress) if progress >= 0 => this.child(
                                                        Label::new(format!(
                                                            "{}%",
                                                            progress.min(100)
                                                        ))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Accent),
                                                    ),
                                                    Some(_) => this.child(
                                                        Label::new("…")
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Accent),
                                                    ),
                                                    None => this,
                                                }
                                            })
                                            .map({
                                                let agent_badge = agent_badge.clone();
                                                let badge_id = SharedString::from(format!(
                                                    "term-agent-{}-{ix}",
                                                    group_id.0
                                                ));
                                                move |this| match agent_badge {
                                                    Some(badge) => this.child(agent_badge_view(
                                                        badge_id.clone(),
                                                        &badge,
                                                    )),
                                                    None => this,
                                                }
                                            }),
                                    )
                                    .on_click(on_click)
                            })
                            .menu({
                                let terminal_view = terminal_view.clone();
                                let panel = panel.clone();
                                move |window, cx| {
                                    Self::build_terminal_context_menu(
                                        panel.clone(),
                                        group_id,
                                        terminal_view.clone(),
                                        window,
                                        cx,
                                    )
                                }
                            })
                            .into_any_element(),
                    );
                }
            }
        }

        let mut detached_rows: Vec<AnyElement> = Vec::new();
        if !detached_snapshot.is_empty() {
            detached_rows.push(
                h_flex()
                    .px_2()
                    .py_1()
                    .mx_1()
                    .items_center()
                    .child(
                        Label::new(i18n::t("detached_sessions"))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
            for (index, title, agent_badge, progress) in detached_snapshot {
                let hover_background = cx.theme().colors().element_hover;
                let on_click = cx.listener(move |this, _, window, cx| {
                    this.reattach_detached(index, window, cx);
                });
                let on_close = cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_detached_session(index, cx);
                });
                detached_rows.push(
                    div()
                        .id(SharedString::from(format!("detached-{index}")))
                        .px_2()
                        .py_1()
                        .mx_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover_background))
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(div().size(IconSize::Small.rems()))
                                .child(
                                    ui::Icon::new(IconName::Terminal)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Label::new(title).size(LabelSize::Small).truncate()),
                                )
                                .map({
                                    move |this| match progress {
                                        Some(progress) if progress >= 0 => this.child(
                                            Label::new(format!("{}%", progress.min(100)))
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                        ),
                                        Some(_) => this.child(
                                            Label::new("…")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                        ),
                                        None => this,
                                    }
                                })
                                .map({
                                    let agent_badge = agent_badge.clone();
                                    let badge_id =
                                        SharedString::from(format!("detached-agent-{index}"));
                                    move |this| match agent_badge {
                                        Some(badge) => {
                                            this.child(agent_badge_view(badge_id.clone(), &badge))
                                        }
                                        None => this,
                                    }
                                })
                                .child(
                                    IconButton::new(
                                        SharedString::from(format!("detached-close-{index}")),
                                        IconName::Close,
                                    )
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("close")))
                                    .on_click(on_close),
                                ),
                        )
                        .on_click(on_click)
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .items_center()
                    .justify_between()
                    .child(Label::new(i18n::t("terminals")).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                IconButton::new("show-terminal-list", IconName::Terminal)
                                    .icon_size(IconSize::Small)
                                    .toggle_state(true)
                                    .tooltip(Tooltip::text(i18n::t("terminal_list"))),
                            )
                            .child(
                                IconButton::new("show-file-list", IconName::File)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("file_list")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(zed_actions::file_list_panel::ShowList),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("show-finder", IconName::FolderOpen)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("finder")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(zed_actions::file_list_panel::ShowFinder),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("show-agent", IconName::Sparkle)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("agent")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(zed_actions::assistant::ToggleFocus),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("review-agent-changes", IconName::Diff)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("review_agent_changes")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(
                                                crate::terminal_list_panel::ReviewAgentChanges,
                                            ),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("new-group", IconName::FolderAdd)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("new_group")))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.create_group(window, cx);
                                    })),
                            )
                            .child(
                                IconButton::new("new-terminal", IconName::Plus)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("new_terminal")))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_terminal(window, cx);
                                    })),
                            ),
                    ),
            )
            .when(group_switch_hint.is_some(), |el| {
                el.child(
                    div().px_3().pb_1().child(
                        Label::new(group_switch_hint.clone().unwrap_or_default())
                            .size(LabelSize::XSmall)
                            .color(Color::Accent),
                    ),
                )
            })
            .child(
                v_flex()
                    .id("terminal-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(detached_rows),
            )
    }
}

impl Panel for TerminalListPanel {
    fn persistent_name() -> &'static str {
        "TerminalListPanel"
    }

    fn panel_key() -> &'static str {
        "terminal_list_panel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.position = position;
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Terminal)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some(i18n::t_str("terminal_list"))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn extra_status_buttons(&self, _window: &Window, _cx: &App) -> Vec<PanelStatusButton> {
        vec![
            PanelStatusButton {
                id: "new-group",
                icon: IconName::FolderAdd,
                tooltip: i18n::t_str("new_group"),
                action: Box::new(NewGroup),
                selected: false,
                trailing: true,
            },
            PanelStatusButton {
                id: "new-terminal",
                icon: IconName::Plus,
                tooltip: i18n::t_str("new_terminal"),
                action: Box::new(NewTerminal),
                selected: false,
                trailing: true,
            },
        ]
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }

    fn activation_priority(&self) -> u32 {
        2
    }
}

/// Sidebar label: custom title, else cwd basename — never appends the shell
/// name (`dir — zsh`).
fn sidebar_terminal_name(tv: &Entity<TerminalView>, cx: &App) -> SharedString {
    let view = tv.read(cx);
    let terminal = view.terminal().read(cx);
    let title = terminal.title(true);
    let working_directory = terminal.working_directory();
    sidebar_label(view.custom_title(), working_directory.as_deref(), &title)
}

/// Pure part of [`sidebar_terminal_name`], split out so the label precedence
/// can be tested without a workspace.
fn sidebar_label(
    custom_title: Option<&str>,
    working_directory: Option<&Path>,
    terminal_title: &str,
) -> SharedString {
    if let Some(custom) = custom_title.filter(|title| !title.trim().is_empty()) {
        return custom.to_string().into();
    }

    if let Some(name) = working_directory
        .and_then(|cwd| cwd.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
    {
        return name.into();
    }

    terminal_title
        .split_once(" — ")
        .or_else(|| terminal_title.split_once(" - "))
        .map(|(left, _)| left)
        .unwrap_or(terminal_title)
        .to_string()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use project::Project;
    use terminal::TerminalBuilder;
    use terminal::terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape};
    use util::paths::PathStyle;
    use workspace::{AppState, MultiWorkspace, WorkspaceId};

    #[test]
    fn recognizes_npm_installed_agents_by_package_path() {
        let cases = [
            (
                "/usr/local/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
                "pi",
            ),
            (
                "/Users/me/.npm/_npx/abc/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                "pi",
            ),
            (
                "/opt/homebrew/lib/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
                "omp",
            ),
            (
                "/usr/lib/node_modules/@moonshot-ai/kimi-code/dist/main.mjs",
                "kimi",
            ),
            (
                "/usr/lib/node_modules/@qwen-code/qwen-code/dist/index.js",
                "qwen",
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(
                agent_for_package_path(path).map(|agent| agent.id),
                Some(expected),
                "{path}"
            );
        }

        // Windows separators and pnpm-style nested node_modules.
        assert_eq!(
            agent_for_package_path(
                r"C:\Users\me\AppData\Roaming\npm\node_modules\@earendil-works\pi-coding-agent\dist\cli.js"
            )
            .map(|agent| agent.id),
            Some("pi")
        );
        assert_eq!(
            agent_for_package_path(
                "/x/node_modules/.pnpm/@earendil-works+pi-coding-agent@1.0.0/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
            )
            .map(|agent| agent.id),
            Some("pi")
        );

        assert!(agent_for_package_path("/usr/lib/node_modules/other-cli/dist/cli.js").is_none());
        assert!(agent_for_package_path("").is_none());
    }

    #[test]
    fn layout_presets_generate_expected_axes() {
        let horizontal = GroupLayoutMode::Horizontal.generate_layout(3).unwrap();
        match horizontal {
            GroupLayoutNode::Split {
                axis: PersistedAxis::Horizontal,
                children,
                ..
            } => assert_eq!(children.len(), 3),
            _ => panic!("horizontal layout should be a single horizontal split"),
        }

        let vertical = GroupLayoutMode::Vertical.generate_layout(3).unwrap();
        match vertical {
            GroupLayoutNode::Split {
                axis: PersistedAxis::Vertical,
                children,
                ..
            } => assert_eq!(children.len(), 3),
            _ => panic!("vertical layout should be a single vertical split"),
        }

        let fat = GroupLayoutMode::Fat.generate_layout(3).unwrap();
        match fat {
            GroupLayoutNode::Split {
                axis: PersistedAxis::Vertical,
                children,
                ..
            } => {
                assert_eq!(children.len(), 2);
                match &children[1] {
                    GroupLayoutNode::Split {
                        axis: PersistedAxis::Horizontal,
                        children,
                        ..
                    } => assert_eq!(children.len(), 2),
                    _ => panic!("fat layout should put the remaining panes in a row"),
                }
            }
            _ => panic!("fat layout should split top/bottom first"),
        }

        assert!(GroupLayoutMode::Manual.generate_layout(3).is_none());
    }

    #[test]
    fn coding_agent_names_match_known_clis() {
        assert!(is_coding_agent_name("claude"));
        assert!(is_coding_agent_name("/usr/local/bin/codex"));
        assert!(is_coding_agent_name("gemini.exe"));
        assert!(is_coding_agent_name("claude-code"));
        assert!(is_coding_agent_name("traecli"));
        assert!(is_coding_agent_name("omp"));
        assert!(is_coding_agent_name("prime-agent"));
        assert!(is_coding_agent_name("qodercli"));
        assert!(is_coding_agent_name("codebuddy"));
        assert!(is_coding_agent_name("agy"));
        assert!(is_coding_agent_name("cursor-agent"));
        assert!(is_coding_agent_name("hermes"));
        assert!(is_coding_agent_name("empryo"));
        assert!(!is_coding_agent_name("zsh"));
        assert!(!is_coding_agent_name("node"));
        assert!(!is_coding_agent_name("pine"));
        assert!(text_mentions_coding_agent("✳ Claude Code"));
        assert!(text_mentions_coding_agent("Qwen Code"));
        assert!(!text_mentions_coding_agent("bash"));
        assert!(!text_mentions_coding_agent("install pi"));
    }

    #[test]
    fn coding_agent_session_commands_follow_public_cli() {
        let claude = agent_by_id("claude").unwrap();
        assert_eq!(
            agent_command(claude.resume.unwrap(), claude.bin),
            "claude --continue"
        );
        assert_eq!(
            agent_command(claude.past.unwrap(), claude.bin),
            "claude --resume"
        );
        assert_eq!(
            agent_command(claude.fork.unwrap(), claude.bin),
            "claude --continue --fork-session"
        );

        let codex = agent_by_id("codex").unwrap();
        assert_eq!(
            agent_command(codex.resume.unwrap(), "codex"),
            "codex resume --last"
        );
        assert_eq!(agent_command(codex.past.unwrap(), "codex"), "codex resume");
        assert_eq!(agent_command(codex.fork.unwrap(), "codex"), "codex fork");

        let trae = agent_by_id("trae").unwrap();
        assert!(trae.resume.is_some() && trae.fork.is_some() && trae.past.is_none());
        assert_eq!(invocation_bin(trae, "trae"), "trae");

        let opencode = agent_by_id("opencode").unwrap();
        assert_eq!(
            agent_command(opencode.fork.unwrap(), opencode.bin),
            "opencode --continue --fork"
        );
        assert!(opencode.past.is_none());

        let omp = agent_by_id("omp").unwrap();
        assert!(omp.resume.is_some() && omp.past.is_some() && omp.fork.is_none());

        let gemini = agent_by_id("gemini").unwrap();
        assert!(gemini.resume.is_some() && gemini.fork.is_none() && gemini.past.is_none());

        for id in [
            "grok",
            "crush",
            "antigravity",
            "hermes",
            "aider",
            "copilot",
            "kimi",
            "pi",
        ] {
            let agent = agent_by_id(id).unwrap();
            assert!(agent.resume.is_none() && agent.fork.is_none() && agent.past.is_none());
        }
    }

    fn collect_panes(node: &GroupLayoutNode, out: &mut Vec<Vec<usize>>) {
        match node {
            GroupLayoutNode::Pane { terminals, .. } => out.push(terminals.clone()),
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    collect_panes(child, out);
                }
            }
        }
    }

    fn pane_indices(node: &GroupLayoutNode) -> Vec<Vec<usize>> {
        let mut panes = Vec::new();
        collect_panes(node, &mut panes);
        panes
    }

    #[test]
    fn stack_layout_single_pane_with_all_terminals() {
        let layout = GroupLayoutMode::Stack.generate_layout(4).unwrap();
        assert_eq!(pane_indices(&layout), vec![vec![0, 1, 2, 3]]);
    }

    #[test]
    fn tall_layout_first_alone_then_stack() {
        // Two terminals: left pane + right pane, no nested axis.
        let two = GroupLayoutMode::Tall.generate_layout(2).unwrap();
        assert_eq!(pane_indices(&two), vec![vec![0], vec![1]]);
        assert!(matches!(
            two,
            GroupLayoutNode::Split {
                axis: PersistedAxis::Horizontal,
                children,
                ..
            } if children.len() == 2
        ));

        let layout = GroupLayoutMode::Tall.generate_layout(3).unwrap();
        let panes = pane_indices(&layout);
        assert_eq!(panes, vec![vec![0], vec![1], vec![2]]);
        // First terminal left, the rest stacked on the right.
        match layout {
            GroupLayoutNode::Split { axis, children, .. } => {
                assert_eq!(axis, PersistedAxis::Horizontal);
                assert_eq!(children.len(), 2);
                assert!(
                    matches!(children[0], GroupLayoutNode::Pane { ref terminals, .. } if terminals == &vec![0])
                );
                assert!(matches!(
                    children[1],
                    GroupLayoutNode::Split {
                        axis: PersistedAxis::Vertical,
                        ..
                    }
                ));
            }
            _ => panic!("expected a split"),
        }
    }

    #[test]
    fn grid_layout_covers_all_terminals_once() {
        for count in 1..=8 {
            let layout = GroupLayoutMode::Grid.generate_layout(count).unwrap();
            let panes = pane_indices(&layout);
            let mut all: Vec<usize> = panes.iter().flatten().copied().collect();
            all.sort_unstable();
            assert_eq!(all, (0..count).collect::<Vec<_>>(), "count={count}");
        }
    }

    #[test]
    fn manual_mode_generates_nothing() {
        assert_eq!(GroupLayoutMode::Manual.generate_layout(5), None);
        assert_eq!(GroupLayoutMode::Tall.generate_layout(0), None);
    }

    /// Every auto-tiling mode must produce a tree that references exactly the
    /// group's own terminals (0..count) — never more, and never indices that
    /// would reach into another group's list.
    #[test]
    fn tiling_modes_cover_exactly_their_own_terminals() {
        for mode in [
            GroupLayoutMode::Tall,
            GroupLayoutMode::Grid,
            GroupLayoutMode::Stack,
        ] {
            for count in 0..=8 {
                let layout = mode.generate_layout(count);
                if count == 0 {
                    assert!(layout.is_none(), "{mode:?} count=0");
                    continue;
                }
                let layout = layout.expect("non-empty group generates a tree");
                let mut all: Vec<usize> = pane_indices(&layout).into_iter().flatten().collect();
                all.sort_unstable();
                assert_eq!(
                    all,
                    (0..count).collect::<Vec<_>>(),
                    "{mode:?} count={count}"
                );
            }
        }
    }

    fn new_display_only_terminal_view(
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TerminalView> {
        let terminal = cx.new(|cx| {
            TerminalBuilder::new_display_only(
                SettingsCursorShape::Block,
                AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                PathStyle::local(),
            )
            .subscribe(cx)
        });
        cx.new(|cx| {
            TerminalView::new(
                terminal.clone(),
                workspace.downgrade(),
                None,
                project.downgrade(),
                window,
                cx,
            )
        })
    }

    fn seed_group(
        panel: &Entity<TerminalListPanel>,
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        window_handle: &WindowHandle<MultiWorkspace>,
        cx: &mut TestAppContext,
    ) {
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let id = panel.new_group_id();
                    panel.groups.push(TerminalGroup {
                        id,
                        name: SharedString::from("test group"),
                        terminals: Vec::new(),
                        collapsed: false,
                        has_unread: false,
                        saved_layout: None,
                        layout_mode: GroupLayoutMode::Manual,
                        broadcast_input: false,
                        session_terminal_count: None,
                        restore_slots: Vec::new(),
                    });
                    if panel.active_group_id == GroupId(0) && panel.groups.len() == 1 {
                        panel.active_group_id = id;
                    }
                    for _ in 0..2 {
                        let tv = new_display_only_terminal_view(workspace, project, window, cx);
                        panel.add_terminal_to_group(id, tv, None, None, cx);
                    }
                    panel.sync_active_group_to_pane(window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
    }

    /// Regression test: auto-tiling re-flow (spawn / close / switch) must never
    /// hang the app or drop terminals from the group's model. A rebuild removes
    /// and re-adds items in the panes; those events must not feed back into
    /// `remove_terminal_by_id` / `sync_active_group_to_pane` in a loop.
    #[gpui::test]
    async fn tiling_reflow_keeps_group_terminals_and_does_not_hang(cx: &mut TestAppContext) {
        // Isolate session-file writes from the real data directory.
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 2);

        // Apply a tiling mode: replaces the center split tree.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.set_group_layout(GroupId(0), GroupLayoutMode::Grid, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 2);

        // "+"-style spawn inside the tiling group: the new tab lands in the
        // active pane, then the deferred sync re-flows the tiles.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let tv = new_display_only_terminal_view(&workspace, &project, window, cx);
                    panel.add_terminal_to_group(GroupId(0), tv.clone(), None, None, cx);
                    panel.switching = true;
                    let pane = panel.display_pane_entity(cx).unwrap();
                    pane.update(cx, |pane, cx| {
                        pane.add_item(Box::new(tv.clone()), true, true, None, window, cx);
                    });
                    panel.switching = false;
                    panel.sync_active_group_to_pane(window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 3);

        // Close a terminal in the tiling group: exactly one leaves the model
        // and the re-flow settles (no empty panes / no feedback loop).
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let tv = panel.groups[0].terminals[0].clone();
                    panel.close_terminal(tv.entity_id(), window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 2);

        // Switch to a second group and back: the tiling group's own list stays
        // intact and the switch settles.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let id = panel.new_group_id();
                    panel.groups.push(TerminalGroup {
                        id,
                        name: SharedString::from("second group"),
                        terminals: Vec::new(),
                        collapsed: false,
                        has_unread: false,
                        saved_layout: None,
                        layout_mode: GroupLayoutMode::Manual,
                        broadcast_input: false,
                        session_terminal_count: None,
                        restore_slots: Vec::new(),
                    });
                    let tv = new_display_only_terminal_view(&workspace, &project, window, cx);
                    panel.add_terminal_to_group(id, tv, None, None, cx);
                    panel.switch_group(id, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            panel.read_with(cx, |p, _| p
                .groups
                .iter()
                .filter(|g| g.id == GroupId(1))
                .count()),
            1
        );

        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.switch_group(GroupId(0), window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.active_group_id), GroupId(0));
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 2);
        assert_eq!(panel.read_with(cx, |p, _| p.groups[1].terminals.len()), 1);
    }

    /// Moving a terminal between groups must keep the same view (and PTY),
    /// update both groups' terminal lists, and never leave the moved tab
    /// visible under its old group in the center panes.
    #[gpui::test]
    async fn move_terminal_to_group_preserves_view_and_layouts(cx: &mut TestAppContext) {
        // Isolate session-file writes from the real data directory.
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);
        // seed_group creates GroupId(0) with two terminals and makes it active.
        let moved_id = panel.read_with(cx, |p, _| p.groups[0].terminals[0].entity_id());

        // Second, inactive group with its own terminal.
        let target_id = window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let id = panel.new_group_id();
                    panel.groups.push(TerminalGroup {
                        id,
                        name: SharedString::from("target group"),
                        terminals: Vec::new(),
                        collapsed: false,
                        has_unread: false,
                        saved_layout: None,
                        layout_mode: GroupLayoutMode::Manual,
                        broadcast_input: false,
                        session_terminal_count: None,
                        restore_slots: Vec::new(),
                    });
                    let tv = new_display_only_terminal_view(&workspace, &project, window, cx);
                    panel.add_terminal_to_group(id, tv, None, None, cx);
                    id
                })
            })
            .unwrap();
        cx.run_until_parked();

        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.move_terminal_to_group(moved_id, target_id, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.active_group_id, GroupId(0));
            assert_eq!(panel.groups[0].terminals.len(), 1);
            assert_eq!(panel.groups[1].terminals.len(), 2);
            assert!(
                panel.groups[0]
                    .terminals
                    .iter()
                    .all(|tv| tv.entity_id() != moved_id)
            );
            assert!(
                panel.groups[1]
                    .terminals
                    .iter()
                    .any(|tv| tv.entity_id() == moved_id)
            );
        });

        // The source group's manual layout must follow the shrunken list.
        let source_layout = panel
            .read_with(cx, |panel, _| panel.groups[0].saved_layout.clone())
            .expect("source group keeps a layout");
        let mut referenced: Vec<usize> =
            TerminalListPanel::layout_referenced_indices(&source_layout)
                .into_iter()
                .collect();
        referenced.sort_unstable();
        assert_eq!(referenced, vec![0]);

        // The moved terminal belongs to the inactive group, so it must not stay
        // visible in the center panes.
        let is_visible = |cx: &mut TestAppContext| {
            workspace.read_with(cx, |workspace, cx| {
                workspace
                    .panes()
                    .iter()
                    .any(|pane| pane.read(cx).items().any(|item| item.item_id() == moved_id))
            })
        };
        assert!(!is_visible(cx), "moved tab must leave the active group");

        // Switching to the target group shows it again.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.switch_group(target_id, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.active_group_id), target_id);
        assert!(is_visible(cx), "moved tab must appear in its new group");
        assert_eq!(panel.read_with(cx, |p, _| p.groups[1].terminals.len()), 2);
    }

    /// Detaching a terminal must keep the same view alive (and its PTY) while
    /// removing it from its group and the center panes; reattaching restores
    /// that same view instead of spawning a new shell.
    #[gpui::test]
    async fn detach_and_reattach_preserve_terminal_view(cx: &mut TestAppContext) {
        // Isolate session-file writes from the real data directory.
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);
        let detached_id = panel.read_with(cx, |p, _| p.groups[0].terminals[0].entity_id());
        let is_visible = |cx: &mut TestAppContext| {
            workspace.read_with(cx, |workspace, cx| {
                workspace.panes().iter().any(|pane| {
                    pane.read(cx)
                        .items()
                        .any(|item| item.item_id() == detached_id)
                })
            })
        };
        assert!(is_visible(cx));

        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.detach_terminal(detached_id, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.groups[0].terminals.len(), 1);
            assert_eq!(panel.detached_sessions.len(), 1);
            assert_eq!(panel.detached_sessions[0].view.entity_id(), detached_id);
        });
        assert!(!is_visible(cx), "detached terminal leaves the center panes");

        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.reattach_detached(0, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();

        panel.read_with(cx, |panel, _| {
            assert!(panel.detached_sessions.is_empty());
            assert_eq!(panel.groups[0].terminals.len(), 2);
            assert!(
                panel.groups[0]
                    .terminals
                    .iter()
                    .any(|tv| tv.entity_id() == detached_id),
                "reattach keeps the original view"
            );
        });
        assert!(is_visible(cx), "reattached terminal is shown again");
    }

    /// Regression test: replacing the center layout (a group switch or an
    /// auto-tiling re-flow) must re-home non-terminal items instead of
    /// dropping them. Dropping an editor here silently loses unsaved buffers.
    #[gpui::test]
    async fn layout_rebuild_preserves_editor_tabs(cx: &mut TestAppContext) {
        // Isolate session-file writes from the real data directory.
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);

        // Add a non-terminal item to the center pane, alongside the terminals.
        let editor = window_handle
            .update(cx, |_mw, window, cx| {
                let pane = workspace.read(cx).active_pane().clone();
                let editor = cx.new(|cx| Editor::single_line(window, cx));
                pane.update(cx, |pane, cx| {
                    pane.add_item(Box::new(editor.clone()), false, false, None, window, cx);
                });
                editor
            })
            .unwrap();
        cx.run_until_parked();

        let editor_id = editor.entity_id();
        let editor_is_visible = |cx: &mut TestAppContext| {
            workspace.read_with(cx, |workspace, cx| {
                workspace.panes().iter().any(|pane| {
                    pane.read(cx)
                        .items()
                        .any(|item| item.item_id() == editor_id)
                })
            })
        };
        assert!(
            editor_is_visible(cx),
            "editor should start in a center pane"
        );

        // Full center-tree rebuild, as performed by a tiling re-flow or a
        // group switch.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.set_group_layout(GroupId(0), GroupLayoutMode::Grid, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();

        assert!(
            editor_is_visible(cx),
            "editor tab must survive a center-layout rebuild",
        );
    }

    /// `set_custom_data_dir` may only be called once per process; all tests
    /// that write session files share one isolated data directory.
    fn ensure_test_data_dir() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            paths::set_custom_data_dir(
                &std::env::temp_dir()
                    .join(format!("terry-test-{}", std::process::id()))
                    .join("data")
                    .to_string_lossy(),
            );
        });
    }

    /// Empty windows must not follow the workspace DB id. The DB only matches
    /// project workspaces by their paths, so every launch of the welcome window
    /// allocates a new row id. Keying the session file by that id made the panel
    /// look for `ws-<new id>.json` on every restart and the terminals (and their
    /// daemon sessions) appeared lost.
    #[test]
    fn session_key_is_stable_for_empty_windows_across_workspace_ids() {
        assert_eq!(
            TerminalListPanel::session_key_for(None, Vec::new()),
            "default"
        );
        assert_eq!(
            TerminalListPanel::session_key_for(Some(WorkspaceId::from_i64(1)), Vec::new()),
            "default"
        );
        assert_eq!(
            TerminalListPanel::session_key_for(Some(WorkspaceId::from_i64(2)), Vec::new()),
            "default"
        );
    }

    /// A project window keeps using the workspace id once its roots are known,
    /// so the same project maps to one file whether it was restored directly or
    /// migrated from an empty window.
    #[test]
    fn session_key_prefers_the_workspace_id_when_roots_are_known() {
        let id = Some(WorkspaceId::from_i64(42));
        assert_eq!(
            TerminalListPanel::session_key_for(id, vec!["/tmp/project".to_string()]),
            "ws-42",
        );
    }

    #[test]
    fn session_key_falls_back_to_paths_without_a_database_id() {
        let unsorted =
            TerminalListPanel::session_key_for(None, vec!["/b".to_string(), "/a".to_string()]);
        let sorted =
            TerminalListPanel::session_key_for(None, vec!["/a".to_string(), "/b".to_string()]);
        assert_eq!(unsorted, sorted);
        assert!(unsorted.starts_with("paths-"), "unexpected key {unsorted}");
        assert_eq!(
            TerminalListPanel::session_key_for(None, Vec::new()),
            "default"
        );
    }

    /// Session files written before per-profile color schemes existed must
    /// still load, and the field must round-trip when set.
    #[test]
    fn persisted_terminal_color_scheme_is_backwards_compatible() {
        let old: PersistedTerminal = serde_json::from_str(r#"{ "cwd": null }"#).unwrap();
        assert_eq!(old.color_scheme, None);
        assert_eq!(old.font_size, None);
        assert_eq!(old.launch, None);

        let round_trip: PersistedTerminal =
            serde_json::from_str(r#"{ "cwd": null, "color_scheme": "One Dark" }"#).unwrap();
        assert_eq!(round_trip.color_scheme.as_deref(), Some("One Dark"));

        // Unset schemes are omitted so session files stay small.
        let serialized = serde_json::to_string(&old).unwrap();
        assert!(!serialized.contains("color_scheme"));
    }

    /// A zoomed terminal and a non-shell launch (ssh/tmux/profile) survive a
    /// restart: font size, launch origin and profile name round-trip.
    #[test]
    fn persisted_terminal_restores_font_size_and_launch() {
        let json = r#"{
            "cwd": "/tmp",
            "font_size": 15.5,
            "launch": { "type": "command", "program": "ssh", "args": ["box"], "title": "box" }
        }"#;
        let terminal: PersistedTerminal = serde_json::from_str(json).unwrap();
        assert_eq!(terminal.font_size, Some(15.5));
        assert_eq!(
            terminal.launch,
            Some(PersistedLaunch::Command {
                program: "ssh".into(),
                args: vec!["box".into()],
                title: Some("box".into()),
            })
        );

        let profile: PersistedTerminal = serde_json::from_str(
            r#"{ "cwd": null, "launch": { "type": "profile", "name": "Work" } }"#,
        )
        .unwrap();
        assert_eq!(
            profile.launch,
            Some(PersistedLaunch::Profile {
                name: "Work".into()
            })
        );
    }

    /// Auto-tiling groups must persist a layout that matches the current
    /// terminal count even when `saved_layout` is stale (spawn between
    /// re-flows); otherwise the next load prunes the newest terminals.
    #[gpui::test]
    async fn write_session_serializes_fresh_layout_for_auto_tiling_groups(cx: &mut TestAppContext) {
        // Isolate session-file writes from the real data directory.
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.set_group_layout(GroupId(0), GroupLayoutMode::Grid, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(panel.read_with(cx, |p, _| p.groups[0].terminals.len()), 2);

        // Simulate the stale window: a third terminal joins the group but the
        // deferred re-flow has not run yet, so saved_layout still covers 0..2.
        window_handle
            .update(cx, |_mw, window, cx| {
                panel.update(cx, |panel, cx| {
                    let tv = new_display_only_terminal_view(&workspace, &project, window, cx);
                    panel.add_terminal_to_group(GroupId(0), tv, None, None, cx);
                    panel.write_session_file(cx, false);
                });
            })
            .unwrap();

        let path = panel.read_with(cx, |p, _| p.session_file_path());
        let json = std::fs::read_to_string(path).unwrap();
        let session: PersistedSession = serde_json::from_str(&json).unwrap();
        let layout = session.groups[0].layout.as_ref().unwrap();
        let mut referenced: Vec<usize> = TerminalListPanel::layout_referenced_indices(layout)
            .into_iter()
            .collect();
        referenced.sort_unstable();
        assert_eq!(referenced, vec![0, 1, 2]);
    }

    /// Custom titles win, then the working directory, then a title stripped of
    /// its ` — <program>` suffix. Regression coverage for the sidebar label.
    #[test]
    fn sidebar_label_prefers_custom_title_then_directory() {
        let cwd = Path::new("/work/terry");

        assert_eq!(
            sidebar_label(Some("api"), Some(cwd), "terry — zsh"),
            SharedString::from("api")
        );
        // A whitespace-only custom title is ignored.
        assert_eq!(
            sidebar_label(Some("   "), Some(cwd), "terry — zsh"),
            SharedString::from("terry")
        );
        // The directory wins over the dynamic title.
        assert_eq!(
            sidebar_label(None, Some(cwd), "somewhere-else — zsh"),
            SharedString::from("terry")
        );
        // Neither a custom title nor a directory: strip the program suffix.
        assert_eq!(
            sidebar_label(None, None, "src — zsh"),
            SharedString::from("src")
        );
        assert_eq!(
            sidebar_label(None, None, "src - fish"),
            SharedString::from("src")
        );
        assert_eq!(
            sidebar_label(None, None, "plain title"),
            SharedString::from("plain title")
        );
        // A display-only terminal with no cwd report still shows its fallback.
        assert_eq!(
            sidebar_label(None, None, "Terminal"),
            SharedString::from("Terminal")
        );
        // The root directory has no basename; fall through to the title.
        assert_eq!(
            sidebar_label(None, Some(Path::new("/")), "root — zsh"),
            SharedString::from("root")
        );
    }

    /// A daemon-backed terminal is display-only, so its sidebar label must
    /// follow the directory the shell reports via OSC 7 (and the panel must
    /// re-render on that event). It used to stay "Terminal" forever.
    #[gpui::test]
    async fn sidebar_name_follows_display_only_cwd(cx: &mut TestAppContext) {
        ensure_test_data_dir();

        let params = cx.update(AppState::test);
        cx.update(|cx| {
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let project = Project::test(params.fs.clone(), [], cx).await;
        let window_handle =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window_handle
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        let panel = window_handle
            .update(cx, |mw, window, cx| {
                let workspace = mw.workspace().clone();
                let project = workspace.read(cx).project().clone();
                let display_pane = workspace.read(cx).active_pane().clone();
                let panel = cx.new(|cx| {
                    TerminalListPanel::new(
                        workspace.clone(),
                        display_pane,
                        project,
                        None,
                        window,
                        cx,
                    )
                });
                workspace.update(cx, |workspace, cx| {
                    workspace.add_panel(panel.clone(), window, cx);
                });
                panel
            })
            .unwrap();

        seed_group(&panel, &workspace, &project, &window_handle, cx);
        let view = panel.read_with(cx, |panel, _| panel.groups[0].terminals[0].clone());
        assert_eq!(
            panel.read_with(cx, |_, cx| sidebar_terminal_name(&view, cx)),
            SharedString::from("Terminal"),
            "without a cwd report a display-only terminal still has no name"
        );

        let terminal = view.read_with(cx, |view, cx| view.terminal().clone());
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"\x1b]7;file:///work/terry\x07", cx);
        });
        cx.run_until_parked();

        assert_eq!(
            panel.read_with(cx, |panel, _| panel
                .terminal_spawn_cwds
                .get(&view.entity_id())
                .cloned()
                .flatten()),
            Some(PathBuf::from("/work/terry")),
            "the panel should remember the directory reported by OSC 7"
        );
        assert_eq!(
            panel.read_with(cx, |_, cx| sidebar_terminal_name(&view, cx)),
            SharedString::from("terry"),
            "the sidebar label should track the shell's directory"
        );

        // `cd` again: the name follows without any other activity.
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(b"\x1b]7;file:///work/other\x07", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            panel.read_with(cx, |_, cx| sidebar_terminal_name(&view, cx)),
            SharedString::from("other")
        );
    }
}
