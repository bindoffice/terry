use std::path::Path;

use gpui::{
    App, Context, Empty, Entity, EventEmitter, IntoElement, ParentElement, Render, SharedString,
    Subscription, WeakEntity, Window,
};
use terminal::Event as TerminalEvent;
use terminal_view::TerminalView;
use ui::{Button, LabelSize, Tooltip, prelude::*};
use util::paths::PathExt;
use workspace::{HideStatusItem, StatusItemView, Workspace, item::ItemHandle};

use crate::terminal_list_panel::TerminalListPanel;

const CWD_MAX_CHARS: usize = 48;

/// Status-bar item: active group name and current terminal title.
pub struct ActiveTerminalContext {
    workspace: WeakEntity<Workspace>,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    _terminal_subscription: Option<Subscription>,
    _panel_observation: Option<Subscription>,
}

impl ActiveTerminalContext {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.weak_handle(),
            label: None,
            tooltip: None,
            _terminal_subscription: None,
            _panel_observation: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            self.clear();
            cx.notify();
            return;
        };
        let Some(panel) = workspace.read(cx).panel::<TerminalListPanel>(cx) else {
            self.clear();
            cx.notify();
            return;
        };

        let panel = panel.read(cx);
        let group_name = panel.active_group_name();
        let terminal_title = panel.active_terminal_view(cx).map(|tv| {
            let view = tv.read(cx);
            view.custom_title()
                .map(|title| title.to_string())
                .unwrap_or_else(|| view.terminal().read(cx).title(true))
        });

        match (group_name, terminal_title) {
            (Some(group), Some(title)) if !title.is_empty() && title != group.as_ref() => {
                let label = format!("{group} · {title}");
                self.tooltip = Some(label.clone().into());
                self.label = Some(label.into());
            }
            (Some(group), _) => {
                self.tooltip = Some(group.clone());
                self.label = Some(group);
            }
            (None, Some(title)) if !title.is_empty() => {
                self.tooltip = Some(title.clone().into());
                self.label = Some(title.into());
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
        self._terminal_subscription = terminal_view.map(|tv| {
            let terminal = tv.read(cx).terminal().clone();
            cx.subscribe(&terminal, |this, _, event, cx| {
                if matches!(
                    event,
                    TerminalEvent::TitleChanged | TerminalEvent::BreadcrumbsChanged
                ) {
                    this.refresh(cx);
                }
            })
        });
    }

    fn track_panel(&mut self, cx: &mut Context<Self>) {
        self._panel_observation = self
            .workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).panel::<TerminalListPanel>(cx))
            .map(|panel| {
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
        self.track_panel(cx);
        self.refresh(cx);
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Always relevant while a terminal group exists.
        None
    }
}

/// Status-bar item: working directory of the focused terminal.
pub struct ActiveTerminalCwd {
    workspace: WeakEntity<Workspace>,
    display: Option<SharedString>,
    full: Option<SharedString>,
    _terminal_subscription: Option<Subscription>,
    _panel_observation: Option<Subscription>,
}

impl ActiveTerminalCwd {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.weak_handle(),
            display: None,
            full: None,
            _terminal_subscription: None,
            _panel_observation: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            self.clear();
            cx.notify();
            return;
        };
        let Some(panel) = workspace.read(cx).panel::<TerminalListPanel>(cx) else {
            self.clear();
            cx.notify();
            return;
        };

        let cwd = panel
            .read(cx)
            .active_terminal_view(cx)
            .and_then(|tv| tv.read(cx).terminal().read(cx).working_directory());

        match cwd {
            Some(path) => {
                let (display, full) = format_cwd(&path);
                self.display = Some(display);
                self.full = Some(full);
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
        self._terminal_subscription = terminal_view.map(|tv| {
            let terminal = tv.read(cx).terminal().clone();
            cx.subscribe(&terminal, |this, _, event, cx| {
                if matches!(
                    event,
                    TerminalEvent::TitleChanged | TerminalEvent::BreadcrumbsChanged
                ) {
                    this.refresh(cx);
                }
            })
        });
    }

    fn track_panel(&mut self, cx: &mut Context<Self>) {
        self._panel_observation = self
            .workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).panel::<TerminalListPanel>(cx))
            .map(|panel| {
                cx.observe(&panel, |this, _, cx| {
                    this.refresh(cx);
                })
            });
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
        self.track_panel(cx);
        self.refresh(cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_cwd_compacts_home() {
        let home = util::paths::home_dir();
        let path = home.join("dev/terminal_app");
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
}
