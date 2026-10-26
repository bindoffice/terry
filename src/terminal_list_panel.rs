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
use workspace::{ItemHandle, Member, Pane, PaneAxis, PaneGroup};
use zed_actions::terminal_list_panel::{
    AttachTmuxSession, ConnectSshHost, ConnectSshHostWithTmux, ImportTerminalColorScheme, NewGroup,
    NewTerminal, NextGroup, PreviousGroup, SelectTerminalColorScheme, ToggleFocus,
};

use serde::{Deserialize, Serialize};
use settings::Settings;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

actions!(
    terminal_list_panel,
    [
        /// Focuses the next terminal whose agent is waiting for input.
        FocusNextWaitingAgent,
    ]
);

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
    agent_mentioned_in_text(&terminal.title(false))
        .or_else(|| agent_mentioned_in_text(&terminal.breadcrumb_text))
}

/// How long after the last output a terminal running an agent is still treated
/// as "working" rather than waiting for input.
const AGENT_WORKING_WINDOW: Duration = Duration::from_secs(5);

/// Snapshot of an agent terminal's state, rendered as a badge in the sidebar.
#[derive(Clone)]
struct AgentBadge {
    name: SharedString,
    working: bool,
    /// Whether the agent produced output and then went quiet, which usually
    /// means it is waiting for the user to answer or approve something.
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

/// Builds the sidebar agent badge for a terminal, if a coding agent is running.
fn agent_badge_for(
    view: &Entity<TerminalView>,
    terminal_activity: &HashMap<EntityId, std::time::Instant>,
    agent_waiting: &HashSet<EntityId>,
    cx: &App,
) -> Option<AgentBadge> {
    // Only trust the foreground process here: title/breadcrumb matching is fine
    // for a one-off "park this session" decision, but would light up the badge
    // for ordinary terminals whose title happens to mention an agent.
    let view_id = view.entity_id();
    let view = view.read(cx);
    let terminal = view.terminal().read(cx);
    let command = terminal.foreground_process_command_name()?;
    let agent = agent_for_basename(&command_basename(&command))?;
    let working = terminal_activity
        .get(&view_id)
        .is_some_and(|last| last.elapsed() < AGENT_WORKING_WINDOW);
    let (color, icon) = agent_style(agent);
    Some(AgentBadge {
        name: agent_display_name(agent),
        working,
        waiting: agent_waiting.contains(&view_id),
        color,
        icon,
    })
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
        let agent_name = terminal
            .foreground_process_command_name()
            .map(|command| command_basename(&command))
            .and_then(|base| agent_for_basename(&base))
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
    if terminal
        .foreground_process_command_name()
        .is_some_and(|name| is_coding_agent_name(&name))
    {
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
}

/// Replayable launch information for a persisted terminal.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PersistedLaunch {
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
}

impl GroupLayoutMode {
    fn label(self) -> String {
        match self {
            GroupLayoutMode::Manual => i18n::t("layout_manual"),
            GroupLayoutMode::Tall => i18n::t("layout_tall"),
            GroupLayoutMode::Grid => i18n::t("layout_grid"),
            GroupLayoutMode::Stack => i18n::t("layout_stack"),
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
    /// Spawn-time / last-known cwd per terminal view. Filled with the directory
    /// the shell was actually started in (after defaulting), and refreshed from
    /// live process info so session writes don't permanently store `null`.
    terminal_spawn_cwds: HashMap<EntityId, Option<PathBuf>>,
    /// Launch origin per terminal view (profile or direct command), persisted so
    /// restore can replay `ssh`/`tmux` terminals instead of opening a shell.
    terminal_launches: HashMap<EntityId, PersistedLaunch>,
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
    /// Pending refresh that lets the agent badge fall back to idle once output
    /// stops, so the panel does not re-render continuously.
    agent_idle_refresh: Option<Task<()>>,
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
            terminal_spawn_cwds: HashMap::default(),
            terminal_launches: HashMap::default(),
            _terminal_subscriptions: HashMap::default(),
            terminal_activity: HashMap::default(),
            agent_waiting: HashSet::default(),
            agent_idle_refresh: None,
            cwd_persist_task: Task::ready(()),
            command_history_persist: Task::ready(()),
            restore_cwd_guard_until: None,
            parked_agent_sessions: Vec::new(),
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
        let mut roots: Vec<String> = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .collect();
        roots.sort();
        roots.dedup();
        // Empty welcome windows share one stable key so the first window can
        // restore `default.json` across restarts; later windows are isolated
        // via ownership (they get a `-w{id}` file stem).
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

    /// Terminal view by id, including detached sessions.
    fn view_by_id_any(&self, id: EntityId) -> Option<Entity<TerminalView>> {
        self.groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .chain(self.detached_sessions.iter().map(|session| &session.view))
            .find(|view| view.entity_id() == id)
            .cloned()
    }

    /// Focuses the next terminal whose agent is waiting for input, wrapping
    /// around and switching groups when needed.
    pub fn focus_next_waiting_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ordered: Vec<EntityId> = self
            .groups
            .iter()
            .flat_map(|group| group.terminals.iter())
            .map(|view| view.entity_id())
            .filter(|id| self.agent_waiting.contains(id))
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

        if session.groups.is_empty() {
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
            } = p_term;
            // Replay the original launch (named profile, `ssh`, `tmux`, ...)
            // instead of opening a plain shell. A profile that no longer exists
            // falls back to the default shell rather than failing the restore.
            let (profile, launch) = match launch {
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
            };
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
                },
                None,
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

        // When opening a folder, worktrees are already on the project before this
        // panel is created — prefer those cwds over the global session file, which
        // still holds the empty-welcome home directory.
        let project_dirs = self.project_working_directories(cx);
        let profile = self.default_terminal_profile(cx);
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

        if self.load_persisted_session(window, cx) {
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
        ContextMenu::build(window, cx, move |menu, _, _| {
            let view1 = panel.clone();
            let view2 = panel.clone();
            let view3 = panel.clone();
            let view4 = panel.clone();
            let view5 = panel.clone();
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
                });

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
        for id in &terminal_ids {
            self._terminal_subscriptions.remove(id);
            self.terminal_activity.remove(id);
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
            Some(id) => self.terminal_view_by_id(id)?,
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
        }))
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
                    "terminals": terminals,
                })
            })
            .collect();
        serde_json::json!({ "groups": groups })
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
        } = restore;
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
        let profile_env = profile.as_ref().map(|profile| profile.env.clone());
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
            let terminal = project
                .update(cx, |project, cx| match source.as_ref() {
                    Some(view) => {
                        let terminal = view.read(cx).terminal().clone();
                        project.clone_terminal(&terminal, cx, working_directory.clone())
                    }
                    None => project.create_terminal_shell_with_options(
                        working_directory,
                        profile_shell,
                        profile_env,
                        cx,
                    ),
                })?
                .await?;

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
                terminal::Event::TitleChanged | terminal::Event::BreadcrumbsChanged
            ) {
                return;
            }
            if matches!(event, terminal::Event::TitleChanged) {
                // The sidebar uses the terminal title as its label.
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
                let terminal::Event::UserInput(keystroke) = event else {
                    return;
                };
                this.forward_broadcast(&sender, keystroke, cx);
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
                if !was_working || was_waiting {
                    cx.notify();
                }
                if this.agent_idle_refresh.is_none() {
                    this.agent_idle_refresh = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(AGENT_WORKING_WINDOW).await;
                        this.update(cx, |this, cx| {
                            this.agent_idle_refresh = None;
                            let now = std::time::Instant::now();
                            let expired: Vec<EntityId> = this
                                .terminal_activity
                                .iter()
                                .filter(|(_, last)| {
                                    now.duration_since(**last) >= AGENT_WORKING_WINDOW
                                })
                                .map(|(id, _)| *id)
                                .collect();
                            for id in &expired {
                                this.terminal_activity.remove(id);
                            }
                            if expired.is_empty() {
                                return;
                            }
                            // An agent that stops producing output is most
                            // likely waiting for input; record it and notify.
                            let mut newly_waiting = Vec::new();
                            for id in expired {
                                if this.agent_waiting.contains(&id) {
                                    continue;
                                }
                                let Some(view) = this.view_by_id_any(id) else {
                                    continue;
                                };
                                if agent_badge_for(
                                    &view,
                                    &this.terminal_activity,
                                    &this.agent_waiting,
                                    cx,
                                )
                                .is_some()
                                {
                                    this.agent_waiting.insert(id);
                                    newly_waiting.push(view);
                                }
                            }
                            if !newly_waiting.is_empty() {
                                cx.notify();
                                notify_agents_waiting(&newly_waiting, cx);
                            }
                        })
                        .ok();
                    }));
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
                        term.try_keystroke(keystroke, option_as_meta),
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
        self._terminal_subscriptions.remove(&id);
        self.terminal_activity.remove(&id);
        self.terminal_spawn_cwds.remove(&id);
        self.terminal_launches.remove(&id);
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
                let mode = crate::terminal_picker::TerminalPickerMode::TmuxSessions { sessions };
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
        self._terminal_subscriptions.remove(&item_id);
        self.terminal_activity.remove(&item_id);
        let mut changed = false;
        let mut emptied_group: Option<GroupId> = None;
        let mut removed_from_active_auto = false;
        let mut parked: Option<ParkedAgentSession> = None;
        let mut remembered: Option<(&'static str, &'static str, Option<PathBuf>)> = None;
        for group in &mut self.groups {
            if let Some(removed_ix) = group
                .terminals
                .iter()
                .position(|tv| tv.entity_id() == item_id)
            {
                let removed = group.terminals.remove(removed_ix);
                let remembered_cwd = self.terminal_spawn_cwds.remove(&item_id).flatten();
                self.terminal_launches.remove(&item_id);
                let cwd = remembered_cwd
                    .or_else(|| removed.read(cx).terminal().read(cx).working_directory());
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
        if let Some(session) = parked {
            self.parked_agent_sessions.push(session);
            if self.parked_agent_sessions.len() > PARKED_AGENT_SESSION_LIMIT {
                let overflow = self.parked_agent_sessions.len() - PARKED_AGENT_SESSION_LIMIT;
                self.parked_agent_sessions.drain(0..overflow);
            }
        }
        if let Some((agent_id, bin, cwd)) = remembered {
            self.remember_agent_session(agent_id, bin, cwd);
        }
        if !changed {
            return;
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
            )>,
        )> = Vec::new();
        for group in &self.groups {
            let is_active = group.id == active_group_id;
            let collapsed = group.collapsed;
            let has_unread = group.has_unread;
            let mut terminals = Vec::new();
            if !collapsed {
                for tv in &group.terminals {
                    let badge =
                        agent_badge_for(tv, &self.terminal_activity, &self.agent_waiting, cx);
                    let progress = tv.read(cx).notification_progress();
                    terminals.push((tv.clone(), sidebar_terminal_name(tv, cx), badge, progress));
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
                for (ix, (terminal_view, title, agent_badge, progress)) in
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
    if let Some(custom) = view.custom_title().filter(|title| !title.trim().is_empty()) {
        return custom.to_string().into();
    }

    let terminal = view.terminal().read(cx);
    if let Some(name) = terminal
        .working_directory()
        .and_then(|cwd| {
            cwd.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
    {
        return name.into();
    }

    let title = terminal.title(true);
    title
        .split_once(" — ")
        .or_else(|| title.split_once(" - "))
        .map(|(left, _)| left)
        .unwrap_or(&title)
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
    use workspace::{AppState, MultiWorkspace};

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
}
