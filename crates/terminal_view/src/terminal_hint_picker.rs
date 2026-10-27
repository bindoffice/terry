//! Modal picker for generic terminal hints: URLs, paths, hashes, UUIDs,
//! IP addresses, and user-defined patterns.

use std::sync::Arc;

use gpui::{
    App, ClipboardItem, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Subscription, Task,
    Window, rems,
};
use picker::{Picker, PickerDelegate};
use terminal::{HintAction, HintKind, HintMatch, Terminal};
use ui::{Icon, IconName, IconSize, Label, ListItem, ListItemSpacing, prelude::*};
use workspace::ModalView;

fn action_label(action: HintAction) -> String {
    match action {
        HintAction::Open => i18n::t("hint_action_open"),
        HintAction::Copy => i18n::t("hint_action_copy"),
        HintAction::Paste => i18n::t("hint_action_paste"),
        HintAction::Select => i18n::t("hint_action_select"),
    }
}

fn hint_icon(kind: HintKind) -> IconName {
    match kind {
        HintKind::Url => IconName::Link,
        HintKind::Path => IconName::File,
        HintKind::GitHash | HintKind::Uuid | HintKind::Ip | HintKind::Custom => IconName::Copy,
    }
}

fn looks_like_url(text: &str) -> bool {
    text.contains("://")
        || ["mailto:", "ssh:", "git:", "file:", "ftp:"]
            .iter()
            .any(|prefix| text.starts_with(prefix))
}

/// Run a hint's configured action. Shared by the picker and the single-hint
/// fast path in the terminal view.
pub(crate) fn execute_hint(terminal: &Entity<Terminal>, hint: &HintMatch, cx: &mut App) {
    match hint.action {
        HintAction::Open => {
            let is_url = hint.kind == HintKind::Url || looks_like_url(&hint.text);
            terminal.update(cx, |term, cx| term.open_link(&hint.text, is_url, cx));
        }
        HintAction::Copy => {
            cx.write_to_clipboard(ClipboardItem::new_string(hint.text.clone()));
        }
        HintAction::Paste => {
            terminal.update(cx, |term, _| term.paste(&hint.text));
        }
        HintAction::Select => {
            terminal.update(cx, |term, _| term.select_range(hint.range));
        }
    }
}

pub(crate) struct TerminalHintPicker {
    pub picker: Entity<Picker<TerminalHintDelegate>>,
    _subscription: Subscription,
}

impl TerminalHintPicker {
    pub(crate) fn new(
        terminal: Entity<Terminal>,
        hints: Vec<HintMatch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            Picker::uniform_list(TerminalHintDelegate::new(terminal, hints), window, cx)
                .initial_width(rems(52.))
        });
        let _subscription = cx.subscribe(&picker, |_, _, _, cx| cx.emit(DismissEvent));
        Self {
            picker,
            _subscription,
        }
    }
}

impl ModalView for TerminalHintPicker {}
impl EventEmitter<DismissEvent> for TerminalHintPicker {}

impl Focusable for TerminalHintPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for TerminalHintPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .child(self.picker.clone())
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.picker.update(cx, |this, cx| {
                    this.cancel(&Default::default(), window, cx);
                })
            }))
    }
}

pub(crate) struct TerminalHintDelegate {
    terminal: Entity<Terminal>,
    all_hints: Vec<HintMatch>,
    hints: Vec<HintMatch>,
    selected_index: usize,
}

impl TerminalHintDelegate {
    fn new(terminal: Entity<Terminal>, hints: Vec<HintMatch>) -> Self {
        Self {
            terminal,
            hints: hints.clone(),
            all_hints: hints,
            selected_index: 0,
        }
    }
}

impl PickerDelegate for TerminalHintDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "terminal hint picker"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        i18n::t("select_hint").into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some(i18n::t("no_hints_found").into())
    }

    fn match_count(&self) -> usize {
        self.hints.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let query = query.trim().to_lowercase();
        self.hints = if query.is_empty() {
            self.all_hints.clone()
        } else {
            self.all_hints
                .iter()
                .filter(|hint| {
                    hint.text.to_lowercase().contains(&query)
                        || hint
                            .label
                            .as_deref()
                            .is_some_and(|label| label.to_lowercase().contains(&query))
                        || action_label(hint.action).to_lowercase().contains(&query)
                        || format!("{:?}", hint.kind).to_lowercase().contains(&query)
                })
                .cloned()
                .collect()
        };
        self.selected_index = 0;
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(hint) = self.hints.get(self.selected_index()).cloned() else {
            return;
        };

        execute_hint(&self.terminal, &hint, cx);
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let hint = self.hints.get(ix)?;
        let text = match &hint.label {
            Some(label) => format!("{label}: {}", hint.text),
            None => hint.text.clone(),
        };
        let text = util::truncate_and_trailoff(&text, 120);
        let label = format!("{} · {}", action_label(hint.action), text);

        Some(
            ListItem::new(format!("terminal-hint-{ix}"))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(Icon::new(hint_icon(hint.kind)).size(IconSize::Small))
                .child(Label::new(label).truncate()),
        )
    }
}
