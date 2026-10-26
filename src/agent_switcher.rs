//! Agent switcher: a fuzzy picker over agent terminals.
//!
//! Lists only terminals that currently run a coding agent, shows whether each
//! one is working, idle or waiting for input, and focuses the selected
//! terminal. Made for juggling several agents at once.

use gpui::{
    App, AppContext as _, Context, Entity, EntityId, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, actions, div, px,
};
use ui::{
    Color, Icon, IconName, IconSize, Label, LabelSize, ListItem, ListItemSpacing, prelude::*,
};
use util::ResultExt;

use crate::terminal_list_panel::{AgentTerminalEntry, TerminalListPanel};

actions!(agent_switcher, [OpenAgentSwitcher]);

/// Case-insensitive subsequence match.
fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut haystack = haystack.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .flat_map(char::to_lowercase)
        .all(|needle| haystack.by_ref().any(|candidate| candidate == needle))
}

fn status_label(entry: &AgentTerminalEntry) -> String {
    if entry.waiting {
        i18n::t("agent_waiting_for_input")
    } else if entry.working {
        i18n::t("agent_working")
    } else {
        i18n::t("agent_idle")
    }
}

pub fn open(panel: WeakEntity<TerminalListPanel>, cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<AgentSwitcherWindow>())
    {
        existing
            .update(cx, |this, window, cx| {
                this.panel = panel;
                window.activate_window();
                cx.notify();
            })
            .log_err();
        return;
    }

    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("agent_switcher")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(560.), px(480.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(380.),
                    height: px(260.),
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| AgentSwitcherWindow::new(panel, window, cx)),
        )
        .log_err();
    });
}

pub struct AgentSwitcherWindow {
    focus_handle: FocusHandle,
    search: Entity<editor::Editor>,
    query: String,
    panel: WeakEntity<TerminalListPanel>,
    _subscriptions: Vec<Subscription>,
}

impl AgentSwitcherWindow {
    fn new(
        panel: WeakEntity<TerminalListPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let placeholder = i18n::t("agent_switcher_search");

        let search = cx.new(|cx| {
            let mut editor = editor::Editor::single_line(window, cx);
            editor.set_placeholder_text(placeholder.as_str(), window, cx);
            editor
        });
        window.focus(&search.read(cx).focus_handle(cx), cx);

        let subscriptions = vec![
            cx.subscribe(&search, |this, editor, event: &editor::EditorEvent, cx| {
                if matches!(event, editor::EditorEvent::BufferEdited) {
                    this.query = editor.read(cx).text(cx);
                    cx.notify();
                }
            }),
            // Re-render when the panel's agent states change.
            cx.observe(
                &panel.upgrade().expect("panel exists when opening"),
                |_, _, cx| cx.notify(),
            ),
        ];

        Self {
            focus_handle,
            search,
            query: String::new(),
            panel,
            _subscriptions: subscriptions,
        }
    }

    fn filtered(&self, cx: &App) -> Vec<AgentTerminalEntry> {
        let query = self.query.trim();
        let Some(panel) = self.panel.upgrade() else {
            return Vec::new();
        };
        panel
            .read(cx)
            .agent_terminals(cx)
            .into_iter()
            .filter(|entry| {
                query.is_empty()
                    || fuzzy_match(&entry.agent, query)
                    || fuzzy_match(&entry.title, query)
                    || fuzzy_match(&entry.group, query)
            })
            .collect()
    }

    fn focus_entry(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |panel, cx| {
                panel.focus_terminal_by_id(id, window, cx);
            })
            .log_err();
        window.remove_window();
    }

    fn render_entry(
        &self,
        index: usize,
        entry: AgentTerminalEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let status = status_label(&entry);
        let detail = format!("{} · {status} · {}", entry.agent, entry.group);
        let color = if entry.waiting || entry.working {
            entry.color
        } else {
            Color::Muted
        };
        let id = entry.id;

        ListItem::new(index)
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| this.focus_entry(id, window, cx))
                    .ok();
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(entry.icon).size(IconSize::Small).color(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(Label::new(entry.title).truncate())
                            .child(
                                Label::new(detail)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for AgentSwitcherWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentSwitcherWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.filtered(cx);
        let is_filtering = !self.query.trim().is_empty();

        let body = if entries.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Sparkle)
                        .size(IconSize::XLarge)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(if is_filtering {
                        i18n::t("agent_switcher_no_matches")
                    } else {
                        i18n::t("agent_switcher_empty")
                    })
                    .color(Color::Muted),
                )
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap_0p5()
                .children(
                    entries
                        .into_iter()
                        .enumerate()
                        .map(|(index, entry)| self.render_entry(index, entry, cx)),
                )
                .into_any_element()
        };

        div()
            .id("terry-agent-switcher")
            .key_context("AgentSwitcher")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, window, _cx| {
                window.remove_window();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Icon::new(IconName::Sparkle)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new(i18n::t("agent_switcher"))),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        div()
                            .w_full()
                            .h_8()
                            .px_2()
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(self.search.clone()),
                    ),
            )
            .child(
                div()
                    .id("terry-agent-switcher-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(body),
            )
    }
}
