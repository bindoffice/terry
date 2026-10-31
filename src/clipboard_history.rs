//! Built-in clipboard history.
//!
//! Terry keeps a bounded history of recently copied text. The history is
//! populated by polling the system clipboard (so copies made in other apps are
//! captured too) and persisted to the app database, so it survives restarts.
//!
//! A dedicated window lists the entries. Clicking one copies it back to the
//! system clipboard, which makes it possible to keep several pieces of text
//! around and reuse any of them later.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use db::kvp::KeyValueStore;
use editor::Editor;
use gpui::{
    App, AppContext as _, BorrowAppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, SharedString, Subscription, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowOptions, actions, div, px,
};
use serde::{Deserialize, Serialize};
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, Disableable, Icon, IconButton, IconName,
    IconSize, Label, LabelSize, ListItem, ListItemSpacing, Tooltip, prelude::*, right_click_menu,
};
use util::ResultExt;

actions!(clipboard_history, [OpenClipboardHistory]);

/// How often the system clipboard is checked for new text.
const POLL_INTERVAL: Duration = Duration::from_millis(700);
/// Upper bound on the number of entries kept in the history.
const MAX_ENTRIES: usize = 100;
/// Key used to persist the history in the app's key-value store.
const STORAGE_KEY: &str = "clipboard_history_v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ClipboardEntry {
    text: String,
    /// Seconds since the Unix epoch, used for the relative timestamp.
    copied_at: u64,
}

/// Global, app-wide clipboard history, newest entry first.
pub struct ClipboardHistory {
    entries: Vec<ClipboardEntry>,
}

impl gpui::Global for ClipboardHistory {}

impl ClipboardHistory {
    fn load(cx: &App) -> Self {
        let raw = KeyValueStore::global(cx)
            .read_kvp(STORAGE_KEY)
            .log_err()
            .flatten();
        let entries = raw
            .and_then(|raw| serde_json::from_str::<Vec<ClipboardEntry>>(&raw).log_err())
            .unwrap_or_default();
        Self { entries }
    }

    /// Move `text` to the top of the history, inserting it when it is new.
    /// Returns whether anything changed.
    fn touch(&mut self, text: &str) -> bool {
        if self.entries.first().is_some_and(|entry| entry.text == text) {
            return false;
        }
        self.entries.retain(|entry| entry.text != text);
        self.entries.insert(
            0,
            ClipboardEntry {
                text: text.to_string(),
                copied_at: now_secs(),
            },
        );
        self.entries.truncate(MAX_ENTRIES);
        true
    }

    fn remove(&mut self, text: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.text != text);
        self.entries.len() != before
    }

    fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            false
        } else {
            self.entries.clear();
            true
        }
    }

    fn persist(cx: &App) {
        let Ok(payload) = serde_json::to_string(&cx.global::<Self>().entries) else {
            return;
        };
        let db = KeyValueStore::global(cx);
        db::write_and_log(cx, move || async move {
            db.write_kvp(STORAGE_KEY.to_string(), payload).await
        });
    }

    /// Record text that just appeared on the system clipboard.
    fn observe_clipboard_text(text: String, cx: &mut App) {
        if text.trim().is_empty() {
            return;
        }
        let already_latest = cx
            .global::<Self>()
            .entries
            .first()
            .is_some_and(|entry| entry.text == text);
        if already_latest {
            return;
        }
        cx.global_mut::<Self>().touch(&text);
        Self::persist(cx);
    }

    /// Record a copy initiated from within the clipboard window.
    fn record_copy(text: &str, cx: &mut Context<ClipboardHistoryWindow>) {
        let changed = cx.update_global::<Self, _>(|history, _| history.touch(text));
        if changed {
            Self::persist(cx);
        }
    }
}

/// Adds `text` to the clipboard history from outside the clipboard window.
pub fn add_text(text: &str, cx: &mut App) {
    if text.trim().is_empty() {
        return;
    }
    cx.global_mut::<ClipboardHistory>().touch(text);
    ClipboardHistory::persist(cx);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn watch_clipboard(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
            if let Some(text) = text {
                cx.update(|cx| ClipboardHistory::observe_clipboard_text(text, cx));
            }
        }
    })
    .detach();
}

pub fn init(cx: &mut App) {
    cx.set_global(ClipboardHistory::load(cx));
    cx.on_action(|_: &OpenClipboardHistory, cx| open_clipboard_history(cx));
    watch_clipboard(cx);
}

fn open_clipboard_history(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ClipboardHistoryWindow>())
    {
        existing
            .update(cx, |_, window, _| {
                window.activate_window();
            })
            .log_err();
        return;
    }

    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("clipboard")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(480.), px(600.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(360.),
                    height: px(360.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ClipboardHistoryWindow::new(window, cx)),
        )
        .log_err();
    });
}

pub struct ClipboardHistoryWindow {
    focus_handle: FocusHandle,
    search: Entity<Editor>,
    query: String,
    /// Text of the entry that was most recently copied from this window.
    copied_text: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ClipboardHistoryWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let placeholder = i18n::t("clipboard_search");

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
            cx.observe_global::<ClipboardHistory>(|_, cx| cx.notify()),
        ];

        Self {
            focus_handle,
            search,
            query: String::new(),
            copied_text: None,
            _subscriptions: subscriptions,
        }
    }

    fn filtered_entries(&self, cx: &App) -> Vec<ClipboardEntry> {
        let query = self.query.trim().to_lowercase();
        cx.global::<ClipboardHistory>()
            .entries
            .iter()
            .filter(|entry| query.is_empty() || entry.text.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    fn copy_entry(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        ClipboardHistory::record_copy(&text, cx);

        self.copied_text = Some(text.clone());
        let copied_text = text;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;
            this.update(cx, |this, cx| {
                if this.copied_text.as_deref() == Some(copied_text.as_str()) {
                    this.copied_text = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    fn remove_entry(&mut self, text: &str, cx: &mut Context<Self>) {
        let changed = cx.update_global::<ClipboardHistory, _>(|history, _| history.remove(text));
        if changed {
            ClipboardHistory::persist(cx);
        }
        if self.copied_text.as_deref() == Some(text) {
            self.copied_text = None;
        }
        cx.notify();
    }

    /// Right-click menu for a history row: copy the entry (same as a click) or
    /// delete it.
    fn build_entry_context_menu(
        history: WeakEntity<Self>,
        text: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, move |menu, _, _| {
            let copy_history = history.clone();
            let copy_text = text.clone();
            let delete_history = history.clone();
            let delete_text = text.clone();
            menu.entry(i18n::t("copy"), None, move |window, cx| {
                copy_history
                    .update(cx, |this, cx| {
                        this.copy_entry(copy_text.clone(), window, cx)
                    })
                    .log_err();
            })
            .entry(i18n::t("clipboard_delete"), None, move |_, cx| {
                delete_history
                    .update(cx, |this, cx| this.remove_entry(&delete_text, cx))
                    .log_err();
            })
        })
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        let changed = cx.update_global::<ClipboardHistory, _>(|history, _| history.clear());
        if changed {
            ClipboardHistory::persist(cx);
        }
        self.copied_text = None;
        cx.notify();
    }

    fn render_header(&self, total: usize, cx: &mut Context<Self>) -> impl IntoElement {
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
                Icon::new(IconName::Copy)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(i18n::t("clipboard")))
            .child(
                Label::new(total.to_string())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .child(
                Button::new("clear-clipboard-history", i18n::t("clipboard_clear_all"))
                    .style(ButtonStyle::Outlined)
                    .size(ButtonSize::Compact)
                    .disabled(total == 0)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.clear_history(cx)).ok();
                    }),
            )
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
    }

    fn render_entry(
        &self,
        index: usize,
        entry: ClipboardEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.copied_text.as_deref() == Some(entry.text.as_str());
        let preview = preview_text(&entry.text);
        let chars =
            i18n::t("clipboard_chars").replace("{count}", &entry.text.chars().count().to_string());

        let this = cx.entity().downgrade();
        let click_text = entry.text.clone();
        let delete_this = cx.entity().downgrade();
        let delete_text = entry.text.clone();
        let menu_this = cx.entity().downgrade();
        let menu_text = entry.text.clone();

        let item = ListItem::new(index)
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .on_click(move |_, window, cx| {
                let text = click_text.clone();
                this.update(cx, |this, cx| this.copy_entry(text, window, cx))
                    .ok();
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(Label::new(preview).truncate())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new(format_ago(entry.copied_at))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(chars)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .when(copied, |this| {
                                this.child(
                                    Label::new(i18n::t("clipboard_copied"))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Accent),
                                )
                            }),
                    ),
            )
            .end_slot(
                IconButton::new(
                    SharedString::from(format!("delete-clipboard-entry-{index}")),
                    IconName::Trash,
                )
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text(i18n::t("clipboard_delete")))
                .on_click(move |_, _, cx| {
                    let text = delete_text.clone();
                    delete_this
                        .update(cx, |this, cx| this.remove_entry(&text, cx))
                        .ok();
                }),
            );

        right_click_menu(format!("clipboard-history-entry-{index}"))
            .trigger(move |_is_open, _window, _cx| item)
            .menu(move |window, cx| {
                Self::build_entry_context_menu(menu_this.clone(), menu_text.clone(), window, cx)
            })
            .into_any_element()
    }
}

impl Focusable for ClipboardHistoryWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ClipboardHistoryWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.filtered_entries(cx);
        let total = cx.global::<ClipboardHistory>().entries.len();
        let is_filtering = !self.query.trim().is_empty();

        let body = if entries.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Copy)
                        .size(IconSize::XLarge)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(if is_filtering {
                        i18n::t("clipboard_no_matches")
                    } else {
                        i18n::t("clipboard_empty")
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
            .id("terry-clipboard-history")
            .key_context("ClipboardHistory")
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
                    .id("terry-clipboard-history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .child(body),
            )
    }
}

/// Collapse runs of whitespace so the entry fits on a single line, then clip
/// to a readable length. The full text is what gets copied.
fn preview_text(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut out = String::with_capacity(MAX_CHARS + 1);
    let mut last_was_space = true;
    let mut pushed = 0;

    for ch in text.chars() {
        let ch = if ch.is_whitespace() { ' ' } else { ch };
        if ch == ' ' {
            if last_was_space {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        if pushed >= MAX_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
        pushed += 1;
    }

    out.trim().to_string()
}

fn format_ago(copied_at: u64) -> String {
    let now = now_secs();
    let delta = now.saturating_sub(copied_at);
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
