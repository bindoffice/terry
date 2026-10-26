//! A single picker used for SSH hosts, tmux sessions, and terminal color
//! schemes. Each mode shares the same filtered list; confirming an entry calls
//! back into `TerminalListPanel`.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    App, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    ParentElement, Render, SharedString, Task, WeakEntity, Window,
};
use picker::{Picker, PickerDelegate};
use ui::{HighlightedLabel, Label, LabelSize, ListItem, ListItemSpacing, prelude::*};
use workspace::{ModalView, Workspace};

use crate::remote_hosts::{self, SshTarget, TmuxSession};
use crate::terminal_list_panel::TerminalListPanel;

/// What the picker is currently listing.
pub enum TerminalPickerMode {
    SshHosts {
        targets: Vec<SshTarget>,
        /// Attach to (or create) a tmux session on the remote host after
        /// connecting. This is the remote-mux workflow: the connection is
        /// shared and the session survives disconnects.
        remote_tmux: bool,
    },
    TmuxSessions {
        sessions: Vec<TmuxSession>,
        /// Mirror the session's panes as native terminals via control mode
        /// instead of attaching a raw PTY.
        native: bool,
    },
    ColorSchemes {
        terminal_id: EntityId,
        current: Option<String>,
    },
}

#[derive(Clone, Copy)]
enum Kind {
    Ssh { remote_tmux: bool },
    Tmux { native: bool },
    ColorScheme,
}

#[derive(Clone)]
enum EntryAction {
    Ssh {
        target: SshTarget,
        remote_tmux: Option<String>,
    },
    Tmux {
        session: String,
        native: bool,
    },
    ColorScheme {
        name: Option<String>,
    },
}

#[derive(Clone)]
struct Entry {
    label: String,
    detail: Option<String>,
    action: EntryAction,
}

/// Modal picker wrapper.
pub struct TerminalPicker {
    picker: Entity<Picker<TerminalPickerDelegate>>,
}

impl TerminalPicker {
    /// Opens the picker as a workspace modal.
    pub fn open(
        workspace: &mut Workspace,
        panel: Entity<TerminalListPanel>,
        mode: TerminalPickerMode,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        workspace.toggle_modal(window, cx, move |window, cx| {
            Self::new(panel, mode, window, cx)
        });
    }

    fn new(
        panel: Entity<TerminalListPanel>,
        mode: TerminalPickerMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (kind, placeholder, no_matches, base_entries): (
            Kind,
            Arc<str>,
            Option<SharedString>,
            Vec<Entry>,
        ) = match mode {
            TerminalPickerMode::SshHosts {
                targets,
                remote_tmux,
            } => {
                let entries = targets
                    .into_iter()
                    .map(|target| {
                        let session =
                            remote_tmux.then(|| remote_hosts::tmux_session_name(&target.label));
                        Entry {
                            label: target.label.clone(),
                            detail: Some(target.detail()),
                            action: EntryAction::Ssh {
                                target,
                                remote_tmux: session,
                            },
                        }
                    })
                    .collect();
                (
                    Kind::Ssh { remote_tmux },
                    "Connect to SSH host…".into(),
                    Some("Type a hostname and press enter".into()),
                    entries,
                )
            }
            TerminalPickerMode::TmuxSessions { sessions, native } => {
                let entries = sessions
                    .into_iter()
                    .map(|session| Entry {
                        label: session.name.clone(),
                        detail: Some(session.detail()),
                        action: EntryAction::Tmux {
                            session: session.name,
                            native,
                        },
                    })
                    .collect();
                (
                    Kind::Tmux { native },
                    "Attach tmux session…".into(),
                    Some("Type a session name and press enter".into()),
                    entries,
                )
            }
            TerminalPickerMode::ColorSchemes {
                terminal_id,
                current,
            } => {
                let mut entries = vec![Entry {
                    label: "Follow UI Theme".to_string(),
                    detail: Some("use the workspace theme".to_string()),
                    action: EntryAction::ColorScheme { name: None },
                }];
                let mut seen = HashSet::new();
                let palettes = terminal::color_scheme::ColorSchemeRegistry::try_global(cx)
                    .map(|registry| registry.names())
                    .unwrap_or_default();
                for name in palettes {
                    seen.insert(name.clone());
                    let checked = current.as_deref() == Some(name.as_str());
                    entries.push(Entry {
                        label: marker(checked, &name),
                        detail: Some("terminal color scheme".to_string()),
                        action: EntryAction::ColorScheme { name: Some(name) },
                    });
                }
                for theme in theme::ThemeRegistry::global(cx).list_names() {
                    let name = theme.to_string();
                    if !seen.insert(name.clone()) {
                        continue;
                    }
                    let checked = current.as_deref() == Some(name.as_str());
                    entries.push(Entry {
                        label: marker(checked, &name),
                        detail: Some("theme".to_string()),
                        action: EntryAction::ColorScheme { name: Some(name) },
                    });
                }
                let delegate = TerminalPickerDelegate {
                    picker: cx.entity().downgrade(),
                    panel,
                    base_entries: entries.clone(),
                    entries,
                    matches: Vec::new(),
                    selected_index: 0,
                    placeholder: "Select terminal color scheme…".into(),
                    no_matches: Some("No matching color scheme".into()),
                    kind: Kind::ColorScheme,
                    color_scheme_target: Some(terminal_id),
                };
                let picker = cx.new(|cx| {
                    let mut delegate = delegate;
                    delegate.matches = (0..delegate.entries.len()).collect();
                    Picker::uniform_list(delegate, window, cx)
                });
                return Self { picker };
            }
        };

        let mut delegate = TerminalPickerDelegate {
            picker: cx.entity().downgrade(),
            panel,
            base_entries: base_entries.clone(),
            entries: base_entries.clone(),
            matches: Vec::new(),
            selected_index: 0,
            placeholder,
            no_matches,
            kind,
            color_scheme_target: None,
        };
        delegate.matches = (0..delegate.entries.len()).collect();
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

fn marker(checked: bool, name: &str) -> String {
    if checked {
        format!("✓ {name}")
    } else {
        name.to_string()
    }
}

impl Render for TerminalPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("TerminalPicker")
            .child(self.picker.clone())
    }
}

impl Focusable for TerminalPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for TerminalPicker {}
impl ModalView for TerminalPicker {}

pub struct TerminalPickerDelegate {
    picker: WeakEntity<TerminalPicker>,
    panel: Entity<TerminalListPanel>,
    base_entries: Vec<Entry>,
    entries: Vec<Entry>,
    matches: Vec<usize>,
    selected_index: usize,
    placeholder: Arc<str>,
    no_matches: Option<SharedString>,
    kind: Kind,
    color_scheme_target: Option<EntityId>,
}

impl TerminalPickerDelegate {
    fn rebuild_matches(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        self.entries = self.base_entries.clone();

        // Offer an escape hatch for values that are not in the list: a hostname
        // for SSH, or a new session name for tmux.
        if !query.is_empty() {
            match self.kind {
                Kind::Ssh { remote_tmux } => {
                    let session = remote_tmux.then(|| remote_hosts::tmux_session_name(&query));
                    let target = SshTarget::ad_hoc(&query);
                    self.entries.push(Entry {
                        label: format!("Connect to “{query}”"),
                        detail: Some(target.detail()),
                        action: EntryAction::Ssh {
                            target,
                            remote_tmux: session,
                        },
                    });
                }
                Kind::Tmux { native } => {
                    let session = remote_hosts::tmux_session_name(&query);
                    self.entries.push(Entry {
                        label: format!("Create session “{session}”"),
                        detail: Some("new tmux session".to_string()),
                        action: EntryAction::Tmux { session, native },
                    });
                }
                Kind::ColorScheme => {}
            }
        }

        self.matches = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                query.is_empty()
                    || entry.label.to_lowercase().contains(&query)
                    || entry
                        .detail
                        .as_deref()
                        .is_some_and(|detail| detail.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect();
        self.selected_index = 0;
    }
}

impl PickerDelegate for TerminalPickerDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "terminal_extras"
    }

    fn match_count(&self) -> usize {
        self.matches.len()
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

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        self.placeholder.clone()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        self.no_matches.clone()
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.rebuild_matches(&query);
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let action = self
            .matches
            .get(self.selected_index)
            .and_then(|index| self.entries.get(*index))
            .map(|entry| entry.action.clone());

        if let Some(action) = action {
            let panel = self.panel.clone();
            let color_scheme_target = self.color_scheme_target;
            {
                // Reborrow so `window` is still usable for `dismissed` below.
                let window = &mut *window;
                panel.update(cx, move |panel, cx| match action {
                    EntryAction::Ssh {
                        target,
                        remote_tmux,
                    } => {
                        panel.connect_ssh(target, remote_tmux, window, cx);
                    }
                    EntryAction::Tmux { session, native } => {
                        if native {
                            panel.attach_tmux_control_session(session, window, cx);
                        } else {
                            panel.attach_tmux_session(session, window, cx);
                        }
                    }
                    EntryAction::ColorScheme { name } => {
                        if let Some(terminal_id) = color_scheme_target {
                            panel.set_terminal_color_scheme(terminal_id, name, cx);
                        }
                    }
                });
            }
        }

        self.dismissed(window, cx);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let _ = self.picker.update(cx, |_, cx| cx.emit(DismissEvent));
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.entries.get(self.matches.get(ix).copied()?)?;
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(
                    v_flex()
                        .child(HighlightedLabel::new(entry.label.clone(), Vec::new()))
                        .when_some(entry.detail.clone(), |this, detail| {
                            this.child(
                                Label::new(detail)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                        }),
                ),
        )
    }
}
