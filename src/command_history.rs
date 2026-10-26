//! Cross-session command history.
//!
//! Commands are captured when the shell finishes them (`OSC 133` shell
//! integration markers surface as [`terminal::Event::LastCommandFinished`]),
//! together with their captured output, working directory and exit code. The
//! history is persisted, so it is shared across terminals and survives
//! restarts.
//!
//! A dedicated window lists the history with fuzzy search and lets the user
//! re-run any entry in the currently active terminal.

use std::time::{SystemTime, UNIX_EPOCH};

use db::kvp::KeyValueStore;
use editor::Editor;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement,
    Render, SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, actions, div, px,
};
use serde::{Deserialize, Serialize};
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Disableable, Icon, IconButton, IconName, IconSize,
    Label, LabelSize, ListItem, ListItemSpacing, Tooltip, prelude::*,
};
use util::ResultExt;

use crate::terminal_list_panel::TerminalListPanel;

actions!(command_history, [OpenCommandHistory]);

/// Key used to persist the history in the app's key-value store.
const STORAGE_KEY: &str = "command_history_v1";
/// Upper bound on retained entries.
const MAX_ENTRIES: usize = 200;
/// Output cap per entry, so the persisted history stays small.
const MAX_OUTPUT_CHARS: usize = 1500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandEntry {
    pub command: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

/// Global, app-wide command history, newest entry first.
pub struct CommandHistory {
    entries: Vec<CommandEntry>,
}

impl gpui::Global for CommandHistory {}

impl CommandHistory {
    fn load(cx: &App) -> Self {
        let raw = KeyValueStore::global(cx)
            .read_kvp(STORAGE_KEY)
            .log_err()
            .flatten();
        let entries = raw
            .and_then(|raw| serde_json::from_str::<Vec<CommandEntry>>(&raw).log_err())
            .unwrap_or_default();
        Self { entries }
    }

    fn push(&mut self, entry: CommandEntry) -> bool {
        if entry.command.trim().is_empty() {
            return false;
        }
        // A shell that reports the same command twice in a row (for example
        // when a prompt is redrawn) should not duplicate the entry.
        if let Some(latest) = self.entries.first()
            && latest.command == entry.command
            && latest.cwd == entry.cwd
            && entry.at.saturating_sub(latest.at) < 2
        {
            self.entries[0] = entry;
            return true;
        }
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
        true
    }

    fn remove(&mut self, index: usize) -> bool {
        if index < self.entries.len() {
            self.entries.remove(index);
            true
        } else {
            false
        }
    }

    fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            false
        } else {
            self.entries.clear();
            true
        }
    }

    pub(crate) fn persist(cx: &App) {
        let Ok(payload) = serde_json::to_string(&cx.global::<Self>().entries) else {
            return;
        };
        let db = KeyValueStore::global(cx);
        db::write_and_log(cx, move || async move {
            db.write_kvp(STORAGE_KEY.to_string(), payload).await
        });
    }

    /// Records a finished command. Called from the terminal panel when the
    /// shell reports `OSC 133;D`.
    pub fn record(
        command: String,
        output: Option<String>,
        cwd: Option<String>,
        exit_code: Option<i64>,
        cx: &mut App,
    ) {
        let entry = CommandEntry {
            command,
            output: truncate_output(output.unwrap_or_default()),
            cwd,
            exit_code,
            at: now_secs(),
        };
        let _ = cx.update_global::<Self, _>(|history, _| history.push(entry));
    }
}

fn truncate_output(output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }
    let mut truncated: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    truncated.push('…');
    truncated
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Case-insensitive subsequence match, cheap enough to run per keystroke.
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

fn format_ago(at: u64) -> String {
    let delta = now_secs().saturating_sub(at);
    if delta < 60 {
        i18n::t("clipboard_just_now")
    } else if delta < 3600 {
        format!("{}m", delta / 60)
    } else if delta < 86_400 {
        format!("{}h", delta / 3600)
    } else {
        format!("{}d", delta / 86_400)
    }
}

/// Collapses runs of whitespace so the preview fits on a single line.
fn preview_text(text: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut out = String::with_capacity(MAX_CHARS);
    let mut last_was_space = true;
    for ch in text.chars() {
        let ch = if ch == '\n' || ch == '\t' || ch == '\r' {
            ' '
        } else {
            ch
        };
        if ch == ' ' {
            if last_was_space {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        if out.chars().count() >= MAX_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

pub fn init(cx: &mut App) {
    cx.set_global(CommandHistory::load(cx));
}

/// Opens (or focuses) the command history window. `panel` is used to run the
/// selected command in the active terminal.
pub fn open(panel: Option<WeakEntity<TerminalListPanel>>, cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<CommandHistoryWindow>())
    {
        existing
            .update(cx, |this, window, cx| {
                if panel.is_some() {
                    this.panel = panel;
                }
                window.activate_window();
                cx.notify();
            })
            .log_err();
        return;
    }

    let panel = panel.clone();
    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("command_history")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(720.), px(620.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(420.),
                    height: px(360.),
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| CommandHistoryWindow::new(panel, window, cx)),
        )
        .log_err();
    });
}

pub struct CommandHistoryWindow {
    focus_handle: FocusHandle,
    search: Entity<Editor>,
    query: String,
    panel: Option<WeakEntity<TerminalListPanel>>,
    _subscriptions: Vec<Subscription>,
}

impl CommandHistoryWindow {
    fn new(
        panel: Option<WeakEntity<TerminalListPanel>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let placeholder = i18n::t("command_history_search");

        let search = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
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
            cx.observe_global::<CommandHistory>(|_, cx| cx.notify()),
        ];

        Self {
            focus_handle,
            search,
            query: String::new(),
            panel,
            _subscriptions: subscriptions,
        }
    }

    /// Indices into the global entry list that match the current query.
    fn filtered_indices(&self, cx: &App) -> Vec<usize> {
        let query = self.query.trim();
        cx.global::<CommandHistory>()
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                if query.is_empty() {
                    return true;
                }
                fuzzy_match(&entry.command, query)
                    || entry
                        .cwd
                        .as_deref()
                        .is_some_and(|cwd| fuzzy_match(cwd, query))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn run_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = cx.global::<CommandHistory>().entries.get(index).cloned() else {
            return;
        };
        if let Some(panel) = self.panel.clone() {
            panel
                .update(cx, |panel, cx| {
                    panel.run_command_in_active_terminal(&entry.command, cx);
                })
                .log_err();
        }
        window.remove_window();
    }

    fn remove_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let changed = cx.update_global::<CommandHistory, _>(|history, _| history.remove(index));
        if changed {
            CommandHistory::persist(cx);
        }
        cx.notify();
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        let changed = cx.update_global::<CommandHistory, _>(|history, _| history.clear());
        if changed {
            CommandHistory::persist(cx);
        }
        cx.notify();
    }

    fn render_header(&self, total: usize, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        h_flex()
            .px_4()
            .pt_3()
            .pb_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                Icon::new(IconName::Clock)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(i18n::t("command_history")))
            .child(
                Label::new(total.to_string())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .child(
                Button::new("clear-command-history", i18n::t("clipboard_clear_all"))
                    .style(ButtonStyle::Outlined)
                    .size(ButtonSize::Compact)
                    .disabled(total == 0)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.clear_history(cx)).ok();
                    }),
            )
            .into_any_element()
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
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
            )
            .into_any_element()
    }

    fn render_entry(
        &self,
        index: usize,
        entry: CommandEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let delete_this = cx.entity().downgrade();

        let mut meta = format_ago(entry.at);
        if let Some(cwd) = entry.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
            meta.push_str(" · ");
            meta.push_str(cwd);
        }
        if let Some(code) = entry.exit_code.filter(|code| *code != 0) {
            meta.push_str(" · ");
            meta.push_str(&i18n::t("command_history_exit").replace("{code}", &code.to_string()));
        }

        let output_preview = preview_text(&entry.output);

        let item = ListItem::new(index)
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .on_click(move |_, window, cx| {
                this.update(cx, |this, cx| this.run_entry(index, window, cx))
                    .ok();
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(Label::new(entry.command).truncate())
                    .child(Label::new(meta).size(LabelSize::XSmall).color(Color::Muted))
                    .when(!output_preview.is_empty(), |this| {
                        this.child(
                            Label::new(output_preview)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
            .end_slot(
                IconButton::new(
                    SharedString::from(format!("delete-command-{index}")),
                    IconName::Trash,
                )
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text(i18n::t("clipboard_delete")))
                .on_click(move |_, _, cx| {
                    delete_this
                        .update(cx, |this, cx| this.remove_entry(index, cx))
                        .ok();
                }),
            );

        item.into_any_element()
    }
}

impl Focusable for CommandHistoryWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommandHistoryWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let indices = self.filtered_indices(cx);
        let total = cx.global::<CommandHistory>().entries.len();
        let entries: Vec<(usize, CommandEntry)> = indices
            .iter()
            .filter_map(|index| {
                cx.global::<CommandHistory>()
                    .entries
                    .get(*index)
                    .cloned()
                    .map(|entry| (*index, entry))
            })
            .collect();
        let is_filtering = !self.query.trim().is_empty();

        let body = if entries.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Clock)
                        .size(IconSize::XLarge)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(if is_filtering {
                        i18n::t("command_history_no_matches")
                    } else {
                        i18n::t("command_history_empty")
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
                        .map(|(index, entry)| self.render_entry(index, entry, cx)),
                )
                .into_any_element()
        };

        div()
            .id("terry-command-history")
            .key_context("CommandHistory")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, window, _cx| {
                window.remove_window();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(self.render_header(total, cx))
            .child(self.render_search(cx))
            .child(
                div()
                    .id("terry-command-history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(body),
            )
    }
}
