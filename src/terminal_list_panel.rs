use editor::{Editor, MultiBufferOffset};
use gpui::{
    Action, AnyElement, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription, TaskExt, WeakEntity, Window, actions, div, px,
};
use project::Project;
use terminal_view::TerminalView;
use ui::{
    ContextMenu, IconButton, IconName, Label, LabelSize, PopoverMenu, Tooltip, prelude::*,
    right_click_menu,
};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::{ItemHandle, Pane};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
struct PersistedTerminal {
    cwd: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedGroup {
    name: String,
    collapsed: bool,
    terminals: Vec<PersistedTerminal>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedSession {
    groups: Vec<PersistedGroup>,
    active_group_index: usize,
}

actions!(
    terminal_list_panel,
    [
        /// Toggles focus on the terminal list panel.
        ToggleFocus,
        /// Creates a new terminal in the active group.
        NewTerminal
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<TerminalListPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &NewTerminal, window, cx| {
            if let Some(panel) = workspace.panel::<TerminalListPanel>(cx) {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            }
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
}

pub struct TerminalListPanel {
    workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    focus_handle: FocusHandle,
    position: DockPosition,
    _workspace_subscription: Subscription,
    /// The center pane that displays the active group's terminals as tabs.
    display_pane: WeakEntity<Pane>,
    groups: Vec<TerminalGroup>,
    active_group_id: GroupId,
    next_group_id: usize,
    /// Guards against reconciling the model while a group switch is
    /// transiently removing/adding items in the display pane.
    switching: bool,
    renaming_group_id: Option<GroupId>,
    rename_editor: Entity<Editor>,
}

impl TerminalListPanel {
    pub fn new(
        workspace: Entity<Workspace>,
        display_pane: Entity<Pane>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let _workspace_subscription = cx.subscribe(&workspace, |this, _, event, cx| {
            this.on_workspace_event(event, cx);
        });

        let rename_editor = cx.new(|cx| Editor::single_line(window, cx));
        cx.subscribe(&rename_editor, |this, _editor, event, cx| {
            if let editor::EditorEvent::Blurred = event {
                this.commit_rename(cx);
            }
        })
        .detach();

        Self {
            workspace: workspace.downgrade(),
            project: project.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            _workspace_subscription,
            display_pane: display_pane.downgrade(),
            groups: Vec::new(),
            active_group_id: GroupId(0),
            next_group_id: 0,
            switching: false,
            renaming_group_id: None,
            rename_editor,
        }
    }

    /// Creates the initial group (with one terminal) if none exists yet.

    fn session_file_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("terry/sessions/default.json")
    }

    pub fn save_session(&self, cx: &App) {
        let mut session = PersistedSession {
            groups: Vec::new(),
            active_group_index: 0,
        };

        for (i, group) in self.groups.iter().enumerate() {
            if group.id == self.active_group_id {
                session.active_group_index = i;
            }

            let mut p_group = PersistedGroup {
                name: group.name.to_string(),
                collapsed: group.collapsed,
                terminals: Vec::new(),
            };

            for view_ent in &group.terminals {
                let cwd = view_ent.read(cx).terminal().read(cx).working_directory();
                p_group.terminals.push(PersistedTerminal { cwd });
            }

            session.groups.push(p_group);
        }

        let path = Self::session_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json = serde_json::to_string(&session).unwrap_or_default();
        std::fs::write(path, json).ok();
    }

    pub fn load_persisted_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let path = Self::session_file_path();
        let Ok(data) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(session): Result<PersistedSession, _> = serde_json::from_str(&data) else {
            return false;
        };

        if session.groups.is_empty() {
            return false;
        }

        for (i, p_group) in session.groups.into_iter().enumerate() {
            let id = self.new_group_id();
            let name = SharedString::from(p_group.name);
            self.groups.push(TerminalGroup {
                id,
                name,
                terminals: Vec::new(),
                collapsed: p_group.collapsed,
                has_unread: false,
            });

            if i == session.active_group_index {
                self.active_group_id = id;
            }

            for p_term in p_group.terminals {
                self.spawn_terminal(id, p_term.cwd, window, cx);
            }
        }

        // After loading groups, switch to the active one
        self.switch_group(self.active_group_id, window, cx);
        true
    }

    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.groups.is_empty() {
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
        });
        self.active_group_id = id;
        self.spawn_terminal(id, None, window, cx);
        self.save_session(cx);
    }

    fn new_group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Resolves the pane used to display the active group's terminals,
    /// falling back to the workspace's active pane if the original is gone.
    fn display_pane_entity(&self, cx: &App) -> Option<Entity<Pane>> {
        if let Some(pane) = self.display_pane.upgrade() {
            Some(pane)
        } else {
            self.workspace
                .upgrade()
                .map(|workspace| workspace.read(cx).active_pane().clone())
        }
    }

    /// Adds a new terminal to the currently active group.
    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_terminal(self.active_group_id, None, window, cx);
        self.save_session(cx);
    }

    /// Creates a new group, switches to it and spawns its first terminal.
    fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.new_group_id();
        let name = SharedString::from(format!("{} {}", i18n::t("group"), id.0 + 1));
        self.groups.push(TerminalGroup {
            id,
            name,
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
        });
        self.switch_group(id, window, cx);
        self.spawn_terminal(id, None, window, cx);
        self.save_session(cx);
    }

    /// Spawns a shell terminal and attaches it to the given group. The
    /// terminal is only displayed if that group is still active by the time
    /// the (async) terminal is ready.
    fn spawn_terminal(
        &mut self,
        group_id: GroupId,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let working_directory = cwd.or_else(|| std::env::current_dir().ok());
            let terminal = project
                .update(cx, |project, cx| {
                    project.create_terminal_shell(working_directory, cx)
                })?
                .await?;

            this.update_in(cx, |this, window, cx| {
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let weak_workspace = workspace.read(cx).weak_handle();
                let workspace_id = workspace.read(cx).database_id();
                let weak_project = workspace.read(cx).project().downgrade();
                let focus_item =
                    !workspace.update(cx, |workspace, cx| workspace.has_active_modal(window, cx));

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

                this.add_terminal_to_group(group_id, terminal_view.clone(), cx);

                // Only show the terminal if its group is still the active one.
                if group_id == this.active_group_id
                    && let Some(pane) = this.display_pane_entity(cx)
                {
                    pane.update(cx, |pane, cx| {
                        pane.add_item(Box::new(terminal_view), true, focus_item, None, window, cx);
                    });
                }
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn add_terminal_to_group(
        &mut self,
        group_id: GroupId,
        terminal_view: Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.terminals.push(terminal_view);
        }
        cx.notify();
    }

    /// Switches the display pane to show the given group's terminals as tabs.
    fn switch_group(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if group_id == self.active_group_id {
            return;
        }
        let Some(pane) = self.display_pane_entity(cx) else {
            return;
        };

        let new_terminals: Vec<Entity<TerminalView>> = self
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .map(|g| g.terminals.clone())
            .unwrap_or_default();

        // While swapping items the pane emits ItemRemoved events for the
        // outgoing group's terminals; those must not be treated as closes.
        self.switching = true;
        pane.update(cx, |pane, cx| {
            let to_remove: Vec<EntityId> = pane
                .items()
                .filter_map(|item| item.act_as::<TerminalView>(cx).map(|tv| tv.entity_id()))
                .collect();
            for id in to_remove {
                pane.remove_item(id, false, false, window, cx);
            }
            for terminal_view in new_terminals {
                pane.add_item(Box::new(terminal_view), true, false, None, window, cx);
            }
        });
        self.active_group_id = group_id;
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }
        self.switching = false;
        cx.notify();
    }

    /// Activates a specific terminal's tab in the display pane.
    fn focus_terminal(
        &mut self,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.display_pane_entity(cx) else {
            return;
        };
        let target_id = terminal_view.entity_id();
        pane.update(cx, |pane, cx| {
            let index = pane.items().position(|item| item.item_id() == target_id);
            if let Some(index) = index {
                pane.activate_item(index, true, true, window, cx);
            }
        });
    }

    /// Toggles the collapsed state of a group without switching to it.
    fn toggle_group_collapse(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = !group.collapsed;
        }
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
        if group_id != self.active_group_id {
            self.switch_group(group_id, window, cx);
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
            }
            cx.notify();
        }
    }

    fn move_group_up(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index > 0 {
                self.groups.swap(index, index - 1);
                cx.notify();
            }
        }
    }

    fn move_group_down(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index + 1 < self.groups.len() {
                self.groups.swap(index, index + 1);
                cx.notify();
            }
        }
    }

    fn on_workspace_event(&mut self, event: &workspace::Event, cx: &mut Context<Self>) {
        // A tab closed directly in the center pane must be dropped from the
        // model too. During a group switch the removals are intentional, so we
        // must not treat them as user closes or the outgoing group's terminals
        // would be dropped from the model and vanish on switch.
        if self.switching {
            cx.notify();
            return;
        }
        if let workspace::Event::ItemRemoved { item_id } = event {
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
                self.remove_terminal_by_id(*item_id);
            }
        }
        cx.notify();
    }

    fn remove_terminal_by_id(&mut self, item_id: EntityId) {
        for group in &mut self.groups {
            group.terminals.retain(|tv| tv.entity_id() != item_id);
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
            usize,
            bool,
            bool,
            Vec<(Entity<TerminalView>, SharedString)>,
        )> = Vec::new();
        for group in &self.groups {
            let is_active = group.id == active_group_id;
            let collapsed = group.collapsed;
            let mut terminals = Vec::new();
            if !collapsed {
                for tv in &group.terminals {
                    terminals.push((tv.clone(), tv.tab_content_text(0, cx)));
                }
            }
            groups_snapshot.push((
                group.id,
                group.name.clone(),
                group.terminals.len(),
                is_active,
                collapsed,
                terminals,
            ));
        }

        let mut rows: Vec<AnyElement> = Vec::new();
        for (group_id, name, count, is_active, collapsed, terminals) in groups_snapshot {
            let _colors = theme.colors().clone();
            let is_expanded = !collapsed;
            rows.push(
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
                                    .id(SharedString::from(format!("group-chevron-{}", group_id.0)))
                                    .child(
                                        ui::Icon::new(if is_expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_group_collapse(group_id, cx);
                                    })),
                            )
                            .child(
                                ui::Icon::new(if is_active {
                                    IconName::FolderOpen
                                } else {
                                    IconName::Folder
                                })
                                .size(IconSize::Small)
                                .color(if is_active {
                                    Color::Accent
                                } else {
                                    Color::Muted
                                }),
                            )
                            .map(|this| {
                                if Some(group_id) == self.renaming_group_id {
                                    this.child(
                                        div()
                                            .w_full()
                                            .h_5()
                                            .bg(cx.theme().colors().editor_background)
                                            .on_action(cx.listener(
                                                |this, _: &menu::Confirm, _window, cx| {
                                                    this.commit_rename(cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &menu::Cancel, _window, cx| {
                                                    this.commit_rename(cx)
                                                },
                                            ))
                                            .child(self.rename_editor.clone()),
                                    )
                                } else {
                                    this.child(Label::new(name).size(LabelSize::Small).truncate())
                                        .child(
                                            Label::new(format!("{count}"))
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                }
                            })
                            .child({
                                let view = cx.entity().clone();
                                h_flex().flex_grow(1.).justify_end().child(
                                    PopoverMenu::new(SharedString::from(format!(
                                        "menu-{}",
                                        group_id.0
                                    )))
                                    .trigger(
                                        IconButton::new(
                                            SharedString::from(format!("menu-btn-{}", group_id.0)),
                                            IconName::Ellipsis,
                                        )
                                        .icon_size(IconSize::Small),
                                    )
                                    .menu(
                                        move |window, cx| {
                                            let view = view.clone();
                                            Some(ContextMenu::build(
                                                window,
                                                cx,
                                                move |menu, _, _| {
                                                    let view1 = view.clone();
                                                    let view2 = view.clone();
                                                    let view3 = view.clone();
                                                    menu.entry("Rename", None, move |window, cx| {
                                                        view1.update(cx, |this, cx| {
                                                            this.start_renaming(
                                                                group_id, window, cx,
                                                            )
                                                        });
                                                    })
                                                    .entry("Move Up", None, move |_window, cx| {
                                                        view2.update(cx, |this, cx| {
                                                            this.move_group_up(group_id, cx)
                                                        });
                                                    })
                                                    .entry("Move Down", None, move |_window, cx| {
                                                        view3.update(cx, |this, cx| {
                                                            this.move_group_down(group_id, cx)
                                                        });
                                                    })
                                                },
                                            ))
                                        },
                                    ),
                                )
                            }),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            if event.click_count() == 2 {
                                this.start_renaming(group_id, window, cx);
                            } else {
                                this.switch_group(group_id, window, cx);
                            }
                        }),
                    )
                    .into_any_element(),
            );

            if is_expanded {
                for (ix, (terminal_view, title)) in terminals.into_iter().enumerate() {
                    let is_terminal_active = Some(terminal_view.entity_id()) == active_terminal_id;
                    let display_pane = self.display_pane.clone();
                    let terminal_id = terminal_view.entity_id();

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
                                    .pl_5()
                                    .pr_2()
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
                                                Label::new(title.clone())
                                                    .size(LabelSize::Small)
                                                    .truncate(),
                                            ),
                                    )
                                    .on_click(on_click)
                            })
                            .menu(move |window, cx| {
                                let display_pane = display_pane.clone();
                                let terminal_view = terminal_view.clone();
                                ContextMenu::build(window, cx, move |menu, _, _| {
                                    let display_pane = display_pane.clone();
                                    menu.entry("Rename", None, move |window, cx| {
                                        terminal_view.update(cx, |this, cx| {
                                            this.rename_terminal(
                                                &terminal_view::RenameTerminal,
                                                window,
                                                cx,
                                            )
                                        });
                                    })
                                    .entry(
                                        "Close",
                                        None,
                                        move |window, cx| {
                                            if let Some(pane) = display_pane.upgrade() {
                                                pane.update(cx, |pane, cx| {
                                                    pane.close_item_by_id(
                                                        terminal_id,
                                                        workspace::SaveIntent::Close,
                                                        window,
                                                        cx,
                                                    )
                                                    .detach_and_log_err(cx);
                                                });
                                            }
                                        },
                                    )
                                })
                            })
                            .into_any_element(),
                    );
                }
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
                                            Box::new(crate::file_list_panel::ToggleFocus),
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
            .child(
                v_flex()
                    .id("terminal-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows),
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

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }

    fn activation_priority(&self) -> u32 {
        3
    }
}
