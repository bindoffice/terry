use std::path::Path;

use gpui::{
    App, Context, Empty, Entity, EventEmitter, IntoElement, ParentElement, Render, SharedString,
    Subscription, WeakEntity, Window,
};
use terminal::Event as TerminalEvent;
use terminal_view::TerminalView;
use ui::{Button, LabelSize, Tooltip, prelude::*};
use util::paths::PathExt;
use workspace::{HideStatusItem, StatusItemView, item::ItemHandle};

use crate::terminal_list_panel::TerminalListPanel;

const CWD_MAX_CHARS: usize = 48;

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

        let terminal_title = self.active_terminal.as_ref().and_then(|tv| {
            let tv = tv.upgrade()?;
            let view = tv.read(cx);
            Some(
                view.custom_title()
                    .map(|title| title.to_string())
                    .unwrap_or_else(|| view.terminal().read(cx).title(true)),
            )
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
        self.active_terminal = terminal_view.as_ref().map(|tv| tv.downgrade());
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
        let cwd = self.active_terminal.as_ref().and_then(|tv| {
            let tv = tv.upgrade()?;
            tv.read(cx).terminal().read(cx).working_directory()
        });

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
        self.active_terminal = terminal_view.as_ref().map(|tv| tv.downgrade());
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
}
