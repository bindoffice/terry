use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Empty, Entity, EventEmitter, IntoElement, ParentElement, Render, SharedString,
    Subscription, WeakEntity, Window,
};
use terminal::Event as TerminalEvent;
use terminal_view::TerminalView;
use ui::{Button, Color, Icon, IconName, IconSize, LabelSize, Tooltip, prelude::*};
use util::paths::PathExt;
use workspace::{HideStatusItem, StatusItemView, item::ItemHandle};

use crate::terminal_list_panel::TerminalListPanel;

const CWD_MAX_CHARS: usize = 48;
const SSH_DESTINATION_MAX_CHARS: usize = 36;

/// Status-bar item: active group name and current terminal title.
pub struct ActiveTerminalContext {
    panel: WeakEntity<TerminalListPanel>,
    active_terminal: Option<WeakEntity<TerminalView>>,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    _terminal_subscription: Option<Subscription>,
    _panel_observation: Option<Subscription>,
}

impl ActiveTerminalContext {
    pub fn new(panel: &Entity<TerminalListPanel>) -> Self {
        Self {
            panel: panel.downgrade(),
            active_terminal: None,
            label: None,
            tooltip: None,
            _terminal_subscription: None,
            _panel_observation: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let group_name = self
            .panel
            .upgrade()
            .and_then(|panel| panel.read(cx).active_group_name());
        let broadcasting = self
            .panel
            .upgrade()
            .is_some_and(|panel| panel.read(cx).active_group_broadcast_input());

        let terminal_info = self.active_terminal.as_ref().and_then(|tv| {
            let tv = tv.upgrade()?;
            let view = tv.read(cx);
            let terminal = view.terminal().read(cx);
            Some((
                view.custom_title()
                    .map(|title| title.to_string())
                    .unwrap_or_else(|| terminal.title(true)),
                terminal.last_exit_code(),
            ))
        });
        let (terminal_title, exit_code) = terminal_info
            .map(|(title, code)| (Some(title), code))
            .unwrap_or_else(|| (None, None));

        // Only surface failures; `exit 0` is the norm and would just add noise.
        let exit_suffix = match exit_code {
            Some(0) | None => String::new(),
            Some(code) => format!(" · exit {code}"),
        };
        // Broadcast input is dangerous: make sure it is always visible.
        let broadcast_suffix = if broadcasting {
            format!(" · {}", i18n::t("broadcast_input"))
        } else {
            String::new()
        };

        match (group_name, terminal_title) {
            (Some(group), Some(title)) if !title.is_empty() && title != group.as_ref() => {
                let label = format!("{group} · {title}{exit_suffix}{broadcast_suffix}");
                self.tooltip = Some(label.clone().into());
                self.label = Some(label.into());
            }
            (Some(group), _) => {
                let label = format!("{group}{exit_suffix}{broadcast_suffix}");
                self.tooltip = Some(label.clone().into());
                self.label = Some(label.into());
            }
            (None, Some(title)) if !title.is_empty() => {
                let label = format!("{title}{exit_suffix}{broadcast_suffix}");
                self.tooltip = Some(label.clone().into());
                self.label = Some(label.into());
            }
            _ => self.clear(),
        }
        cx.notify();
    }

    fn clear(&mut self) {
        self.label = None;
        self.tooltip = None;
    }

    fn track_active_terminal(
        &mut self,
        terminal_view: Option<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        self.active_terminal = terminal_view.as_ref().map(|tv| tv.downgrade());
        self._terminal_subscription = terminal_view.map(|tv| {
            let terminal = tv.read(cx).terminal().clone();
            cx.subscribe(&terminal, |this, _, event, cx| {
                if matches!(
                    event,
                    TerminalEvent::TitleChanged
                        | TerminalEvent::BreadcrumbsChanged
                        | TerminalEvent::CommandStarted
                        | TerminalEvent::LastCommandFinished(_)
                ) {
                    this.refresh(cx);
                }
            })
        });
    }

    fn ensure_panel_observation(&mut self, cx: &mut Context<Self>) {
        if self._panel_observation.is_some() {
            return;
        }
        self._panel_observation = self.panel.upgrade().map(|panel| {
            cx.observe(&panel, |this, _, cx| {
                this.refresh(cx);
            })
        });
    }
}

impl Render for ActiveTerminalContext {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(label) = self.label.clone() else {
            return Empty.into_any_element();
        };
        let tooltip = self.tooltip.clone().unwrap_or_else(|| label.clone());

        div()
            .child(
                Button::new("active-terminal-context", label)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip)),
            )
            .into_any_element()
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for ActiveTerminalContext {}

impl StatusItemView for ActiveTerminalContext {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_view = active_pane_item.and_then(|item| item.act_as::<TerminalView>(cx));
        self.track_active_terminal(terminal_view, cx);
        self.ensure_panel_observation(cx);
        self.refresh(cx);
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Always relevant while a terminal group exists.
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SshSession {
    destination: String,
    /// Directory the remote shell reported, if it has one yet.
    remote_cwd: Option<PathBuf>,
}

/// Status-bar item: indicates when the focused terminal is connected over SSH.
///
/// The SSH connection runs inside the terminal's local shell, so nothing about
/// the tab looks remote on its own. This item makes it explicit: the host, and
/// the remote working directory once the remote shell integration reports one.
pub struct ActiveTerminalSsh {
    active_terminal: Option<WeakEntity<TerminalView>>,
    session: Option<SshSession>,
    _terminal_subscription: Option<Subscription>,
}

impl ActiveTerminalSsh {
    pub fn new() -> Self {
        Self {
            active_terminal: None,
            session: None,
            _terminal_subscription: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.session = self.active_terminal.as_ref().and_then(|tv| {
            let tv = tv.upgrade()?;
            let terminal = tv.read(cx).terminal().read(cx);
            ssh_session_for_terminal(&terminal)
        });
        cx.notify();
    }

    fn track_active_terminal(
        &mut self,
        terminal_view: Option<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        self.active_terminal = terminal_view.as_ref().map(|tv| tv.downgrade());
        self._terminal_subscription = terminal_view.map(|tv| {
            let terminal = tv.read(cx).terminal().clone();
            cx.subscribe(&terminal, |this, _, event, cx| {
                if matches!(
                    event,
                    TerminalEvent::TitleChanged
                        | TerminalEvent::BreadcrumbsChanged
                        | TerminalEvent::CwdChanged
                        | TerminalEvent::CommandStarted
                        | TerminalEvent::LastCommandFinished(_)
                ) {
                    this.refresh(cx);
                }
            })
        });
    }
}

impl Default for ActiveTerminalSsh {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ActiveTerminalSsh {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session.as_ref() else {
            return Empty.into_any_element();
        };
        let destination = format_ssh_destination(&session.destination);
        let label = match &session.remote_cwd {
            Some(cwd) => format!("SSH · {destination} · {}", format_remote_cwd(cwd)),
            None => format!("SSH · {destination}"),
        };

        div()
            .child(
                Button::new("active-terminal-ssh", label)
                    .label_size(LabelSize::Small)
                    .color(Color::Accent)
                    .start_icon(
                        Icon::new(IconName::Server)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                    .tooltip(Tooltip::text(match &session.remote_cwd {
                        Some(cwd) => format!(
                            "Remote SSH session · {}\n{}",
                            session.destination,
                            cwd.display()
                        ),
                        None => format!("Remote SSH session · {}", session.destination),
                    })),
            )
            .into_any_element()
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for ActiveTerminalSsh {}

impl StatusItemView for ActiveTerminalSsh {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_view = active_pane_item.and_then(|item| item.act_as::<TerminalView>(cx));
        self.track_active_terminal(terminal_view, cx);
        self.refresh(cx);
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Only exists while an SSH connection is active.
        None
    }
}

/// Status-bar item: working directory of the focused terminal.
pub struct ActiveTerminalCwd {
    active_terminal: Option<WeakEntity<TerminalView>>,
    display: Option<SharedString>,
    full: Option<SharedString>,
    _terminal_subscription: Option<Subscription>,
}

impl ActiveTerminalCwd {
    pub fn new() -> Self {
        Self {
            active_terminal: None,
            display: None,
            full: None,
            _terminal_subscription: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (cwd, remote) = self
            .active_terminal
            .as_ref()
            .and_then(|tv| {
                let tv = tv.upgrade()?;
                let terminal = tv.read(cx).terminal().read(cx);
                let remote = terminal.remote_working_directory().is_some();
                Some((terminal.display_working_directory(), remote))
            })
            .unwrap_or((None, false));

        match cwd {
            Some(path) => {
                if remote {
                    // Local-path formatting (`~` compaction) does not apply to a
                    // remote path, and the label has to say it is remote.
                    let full: SharedString =
                        format!("{} ({})", path.display(), i18n::t("remote")).into();
                    self.display = Some(format_remote_cwd(&path));
                    self.full = Some(full);
                } else {
                    let (display, full) = format_cwd(&path);
                    self.display = Some(display);
                    self.full = Some(full);
                }
            }
            None => self.clear(),
        }
        cx.notify();
    }

    fn clear(&mut self) {
        self.display = None;
        self.full = None;
    }

    fn track_active_terminal(
        &mut self,
        terminal_view: Option<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        self.active_terminal = terminal_view.as_ref().map(|tv| tv.downgrade());
        self._terminal_subscription = terminal_view.map(|tv| {
            let terminal = tv.read(cx).terminal().clone();
            cx.subscribe(&terminal, |this, _, event, cx| {
                if matches!(
                    event,
                    TerminalEvent::TitleChanged
                        | TerminalEvent::BreadcrumbsChanged
                        | TerminalEvent::CwdChanged
                ) {
                    this.refresh(cx);
                }
            })
        });
    }
}

impl Default for ActiveTerminalCwd {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ActiveTerminalCwd {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(display) = self.display.clone() else {
            return Empty.into_any_element();
        };
        let tooltip = self.full.clone().unwrap_or_else(|| display.clone());

        div()
            .child(
                Button::new("active-terminal-cwd", display)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip)),
            )
            .into_any_element()
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for ActiveTerminalCwd {}

impl StatusItemView for ActiveTerminalCwd {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal_view = active_pane_item.and_then(|item| item.act_as::<TerminalView>(cx));
        self.track_active_terminal(terminal_view, cx);
        self.refresh(cx);
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}

/// Status-bar item: aggregated CPU and memory of every terminal session.
pub struct TerminalResourceUsage {
    panel: WeakEntity<TerminalListPanel>,
    display: Option<SharedString>,
    tooltip: Option<SharedString>,
    task: Option<gpui::Task<()>>,
}

impl TerminalResourceUsage {
    pub fn new(panel: &Entity<TerminalListPanel>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            panel: panel.downgrade(),
            display: None,
            tooltip: None,
            task: None,
        };
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(2500))
                    .await;

                let pids = this.update(cx, |this, cx| {
                    this.panel
                        .upgrade()
                        .map(|panel| panel.read(cx).session_pids(cx))
                        .unwrap_or_default()
                });
                let Ok(pids) = pids else {
                    break;
                };
                if pids.is_empty() {
                    let alive = this
                        .update(cx, |this, cx| {
                            this.clear();
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        break;
                    }
                    continue;
                }

                let usage = cx
                    .background_executor()
                    .spawn(async move { crate::resource_usage::measure(&pids) })
                    .await;
                let alive = this
                    .update(cx, |this, cx| {
                        this.set_usage(&usage);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    fn clear(&mut self) {
        self.display = None;
        self.tooltip = None;
    }

    fn set_usage(&mut self, usage: &crate::resource_usage::Usage) {
        if usage.total_processes == 0 {
            self.clear();
            return;
        }
        self.display = Some(
            format!(
                "{} · {:.0}%",
                format_bytes(usage.total_memory),
                usage.total_cpu
            )
            .into(),
        );

        let mut tooltip = String::new();
        for (index, session) in usage.sessions.iter().enumerate() {
            if index > 0 {
                tooltip.push('\n');
            }
            tooltip.push_str(&format!(
                "#{}  {:.0}%  {}  ({} procs)",
                session.pid,
                session.cpu,
                format_bytes(session.memory),
                session.processes
            ));
        }
        self.tooltip = Some(tooltip.into());
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{}M", bytes / MB)
    } else {
        format!("{}K", bytes.max(KB) / KB)
    }
}

impl Render for TerminalResourceUsage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(display) = self.display.clone() else {
            return Empty.into_any_element();
        };
        let tooltip = self.tooltip.clone().unwrap_or_else(|| display.clone());

        div()
            .child(
                Button::new("terminal-resource-usage", display)
                    .label_size(LabelSize::Small)
                    .tooltip(Tooltip::text(tooltip)),
            )
            .into_any_element()
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for TerminalResourceUsage {}

impl StatusItemView for TerminalResourceUsage {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}

fn format_cwd(path: &Path) -> (SharedString, SharedString) {
    let full: SharedString = path.display().to_string().into();
    let compact = path.compact();
    let compact_str = compact.display().to_string();
    if compact_str.chars().count() <= CWD_MAX_CHARS {
        return (compact_str.into(), full);
    }

    let parts: Vec<String> = compact
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let display = if parts.len() > 3 {
        format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        compact_str
    };
    (display.into(), full)
}

/// SSH state of `terminal`, if it is connected to a remote host.
fn ssh_session_for_terminal(terminal: &terminal::Terminal) -> Option<SshSession> {
    let session = terminal.ssh_session()?;
    Some(SshSession {
        destination: session.destination,
        remote_cwd: terminal.remote_working_directory(),
    })
}

/// The remote directory's last components, for the status bar.
///
/// Remote paths are not under the local home directory, so they are shown as
/// they are (only shortened) rather than `~`-compact.
fn format_remote_cwd(path: &Path) -> SharedString {
    const REMOTE_CWD_MAX_CHARS: usize = 32;
    let full = path.display().to_string();
    if full.chars().count() <= REMOTE_CWD_MAX_CHARS {
        return full.into();
    }

    let tail = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or(full);
    if tail.chars().count() + 2 > REMOTE_CWD_MAX_CHARS {
        let truncated: String = tail.chars().take(REMOTE_CWD_MAX_CHARS - 1).collect();
        return format!("{truncated}…").into();
    }
    format!("…/{tail}").into()
}

fn format_ssh_destination(destination: &str) -> SharedString {
    if destination.chars().count() <= SSH_DESTINATION_MAX_CHARS {
        return destination.to_string().into();
    }

    let truncated: String = destination
        .chars()
        .take(SSH_DESTINATION_MAX_CHARS)
        .collect();
    format!("{truncated}…").into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_cwd_compacts_home() {
        let home = util::paths::home_dir();
        let path = home.join("dev/terry");
        let (display, full) = format_cwd(&path);
        assert!(display.as_ref().starts_with("~/") || display.as_ref().starts_with("…/"));
        assert_eq!(full.as_ref(), path.display().to_string());
    }

    #[test]
    fn format_cwd_truncates_long_paths() {
        let path = PathBuf::from(
            "/very/long/directory/structure/that/exceeds/the/maximum/character/budget/for/status",
        );
        let (display, _) = format_cwd(&path);
        assert!(display.as_ref().chars().count() <= CWD_MAX_CHARS + 2);
        assert!(
            display.as_ref().contains('…')
                || display.as_ref().len() < path.display().to_string().len()
        );
    }

    #[test]
    fn remote_cwd_is_shortened_for_the_status_bar() {
        assert_eq!(
            format_remote_cwd(Path::new("/srv/www")),
            SharedString::from("/srv/www")
        );
        let long = format_remote_cwd(Path::new(
            "/var/lib/a/very/long/remote/directory/structure/project",
        ));
        assert_eq!(long, SharedString::from("…/project"));
    }

    #[test]
    fn ssh_destination_is_truncated_for_the_status_bar() {
        let destination = "alice@a-very-long-remote-hostname.example.com";
        let formatted = format_ssh_destination(destination);
        assert_eq!(formatted.chars().count(), SSH_DESTINATION_MAX_CHARS + 1);
        assert!(formatted.ends_with('…'));
    }
}
