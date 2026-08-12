//! Modal picker that lists every hyperlink visible in a terminal, letting the
//! user open one with the keyboard (kitty's "open URLs" hint mode).

use std::sync::Arc;

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Subscription, Task, Window, rems,
};
use picker::{Picker, PickerDelegate};
use terminal::Terminal;
use ui::{Icon, IconName, IconSize, Label, ListItem, ListItemSpacing, prelude::*};
use workspace::ModalView;

/// A link to offer in the picker: the target text and whether it is a URL.
pub(crate) type LinkTarget = (String, bool);

pub(crate) struct TerminalLinkPicker {
    pub picker: Entity<Picker<TerminalLinkDelegate>>,
    _subscription: Subscription,
}

impl TerminalLinkPicker {
    pub(crate) fn new(
        terminal: Entity<Terminal>,
        links: Vec<LinkTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            Picker::uniform_list(TerminalLinkDelegate::new(terminal, links), window, cx)
                .initial_width(rems(42.))
        });
        let _subscription = cx.subscribe(&picker, |_, _, _, cx| cx.emit(DismissEvent));
        Self {
            picker,
            _subscription,
        }
    }
}

impl ModalView for TerminalLinkPicker {}
impl EventEmitter<DismissEvent> for TerminalLinkPicker {}

impl Focusable for TerminalLinkPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for TerminalLinkPicker {
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

pub(crate) struct TerminalLinkDelegate {
    terminal: Entity<Terminal>,
    all_links: Vec<LinkTarget>,
    links: Vec<LinkTarget>,
    selected_index: usize,
}

impl TerminalLinkDelegate {
    fn new(terminal: Entity<Terminal>, links: Vec<LinkTarget>) -> Self {
        Self {
            terminal,
            links: links.clone(),
            all_links: links,
            selected_index: 0,
        }
    }
}

impl PickerDelegate for TerminalLinkDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "terminal link picker"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        i18n::t("select_link_to_open").into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some(i18n::t("no_links_found").into())
    }

    fn match_count(&self) -> usize {
        self.links.len()
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
        self.links = if query.is_empty() {
            self.all_links.clone()
        } else {
            self.all_links
                .iter()
                .filter(|(text, _)| text.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };
        self.selected_index = 0;
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some((text, is_url)) = self.links.get(self.selected_index()).cloned() else {
            return;
        };
        self.terminal
            .update(cx, |term, cx| term.open_link(&text, is_url, cx));
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
        let (text, is_url) = self.links.get(ix)?;
        let label = util::truncate_and_trailoff(text, 120);

        Some(
            ListItem::new(format!("terminal-link-{ix}"))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    Icon::new(if *is_url {
                        IconName::Link
                    } else {
                        IconName::File
                    })
                    .size(IconSize::Small),
                )
                .child(Label::new(label).truncate()),
        )
    }
}
