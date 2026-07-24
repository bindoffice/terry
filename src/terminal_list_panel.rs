use editor::{Editor, MultiBufferOffset};
use gpui::{
    Action, AnyElement, App, Axis, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription, TaskExt, WeakEntity, Window, div, px,
};
use project::Project;
use terminal_view::TerminalView;
use ui::{
    ContextMenu, IconButton, IconName, Label, LabelSize, PopoverMenu, Tooltip, prelude::*,
    right_click_menu,
};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::{ItemHandle, Member, Pane, PaneAxis, PaneGroup};
use zed_actions::terminal_list_panel::{NewTerminal, ToggleFocus};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
        // Welcome page and default keymaps dispatch workspace::NewTerminal.
        // Terry has no TerminalPanel dock, so that handler no-ops — route here.
        // Registered before terminal_view::init so we run first in the bubble phase.
        workspace.register_action(|workspace, _: &workspace::NewTerminal, window, cx| {
            if let Some(panel) = workspace.panel::<TerminalListPanel>(cx) {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            } else {
                cx.propagate();
            }
        });
    })
    .detach();
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GroupId(usize);

/// In-memory split layout for a terminal group (entity ids of that group's
/// terminals). Restored when switching back to the group.
#[derive(Clone, Debug)]
enum GroupLayoutNode {
    Pane {
        terminals: Vec<EntityId>,
        active: Option<EntityId>,
    },
    Split {
        axis: Axis,
        flexes: Vec<f32>,
        children: Vec<GroupLayoutNode>,
    },
}

/// A named collection of terminals. Selecting a group shows its terminals as
/// tabs in the center pane.
struct TerminalGroup {
    id: GroupId,
    name: SharedString,
    terminals: Vec<Entity<TerminalView>>,
    collapsed: bool,
    #[allow(dead_code)]
    pub has_unread: bool,
    /// Last known center split layout while this group was active.
    saved_layout: Option<GroupLayoutNode>,
}

pub struct TerminalListPanel {
    workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    focus_handle: FocusHandle,
    position: DockPosition,
    _workspace_subscription: Subscription,
    _project_subscription: Subscription,
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
        // subscribe_in so ItemAdded/ItemRemoved reconciliation can update the pane.
        let _workspace_subscription =
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| {
                this.on_workspace_event(event, window, cx);
            });
        // Use subscribe_in so WorktreeAdded always has the panel's window —
        // cx.active_window() is often None during async project open.
        let _project_subscription =
            cx.subscribe_in(&project, window, |this, _project, event, window, cx| {
                if let project::Event::WorktreeAdded(id) = event {
                    this.on_worktree_added(*id, window, cx);
                }
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
            _project_subscription,
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
        paths::data_dir().join("sessions").join("default.json")
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
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            let _ = std::fs::write(path, json);
        }
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

        // Two-pass: create groups and set active_group_id *before* any spawn
        // completes. Otherwise early groups match the constructor's GroupId(0)
        // and get added to the pane, while switch_group later no-ops because
        // active was already updated in the same loop.
        let mut to_spawn: Vec<(GroupId, Option<PathBuf>)> = Vec::new();
        for (i, p_group) in session.groups.into_iter().enumerate() {
            let id = self.new_group_id();
            let name = SharedString::from(p_group.name);
            self.groups.push(TerminalGroup {
                id,
                name,
                terminals: Vec::new(),
                collapsed: p_group.collapsed,
                has_unread: false,
                saved_layout: None,
            });

            if i == session.active_group_index {
                self.active_group_id = id;
            }

            for p_term in p_group.terminals {
                to_spawn.push((id, p_term.cwd));
            }
        }

        for (id, cwd) in to_spawn {
            self.spawn_terminal(id, cwd, None, window, cx);
        }

        // Defer: load runs during workspace init, which already holds the
        // Workspace update lock.
        self.defer_sync_active_group_to_pane(window, cx);
        true
    }

    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.groups.is_empty() {
            return;
        }

        // When opening a folder, worktrees are already on the project before this
        // panel is created — prefer those cwds over the global session file, which
        // still holds the empty-welcome home directory.
        let project_dirs = self.project_working_directories(cx);
        if !project_dirs.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
            });
            self.active_group_id = id;
            for cwd in project_dirs {
                self.spawn_terminal(id, Some(cwd), None, window, cx);
            }
            self.save_session(cx);
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
            saved_layout: None,
        });
        self.active_group_id = id;
        self.spawn_terminal(id, None, None, window, cx);
        self.save_session(cx);
    }

    fn project_working_directories(&self, cx: &App) -> Vec<PathBuf> {
        let Some(project) = self.project.upgrade() else {
            return Vec::new();
        };
        project
            .read(cx)
            .visible_worktrees(cx)
            .filter_map(|worktree| Self::cwd_for_worktree(worktree.read(cx)))
            .collect()
    }

    fn cwd_for_worktree(worktree: &project::Worktree) -> Option<PathBuf> {
        if worktree.root_entry().is_some_and(|entry| entry.is_dir()) {
            Some(worktree.abs_path().to_path_buf())
        } else {
            worktree.abs_path().parent().map(|path| path.to_path_buf())
        }
    }

    fn new_group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Resolves the pane used for new tabs: the focused center pane after
    /// splits, so group operations stay in the pane the user is working in.
    fn display_pane_entity(&self, cx: &App) -> Option<Entity<Pane>> {
        self.workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).active_pane().clone())
            .or_else(|| self.display_pane.upgrade())
    }

    /// All center panes — splits belong to the active group and must be
    /// reconciled together so terminals are never duplicated across panes.
    fn center_panes(&self, cx: &App) -> Vec<Entity<Pane>> {
        self.workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).panes().to_vec())
            .unwrap_or_default()
    }

    /// Adds a new terminal to the currently active group.
    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_in_group(self.active_group_id, window, cx);
    }

    /// Currently focused terminal view (pane active item, else last in the
    /// active group).
    fn active_terminal_view(&self, cx: &App) -> Option<Entity<TerminalView>> {
        if let Some(tv) = self
            .display_pane_entity(cx)
            .and_then(|pane| pane.read(cx).active_item())
            .and_then(|item| item.act_as::<TerminalView>(cx))
        {
            return Some(tv);
        }
        self.groups
            .iter()
            .find(|group| group.id == self.active_group_id)
            .and_then(|group| group.terminals.last().cloned())
    }

    /// Live cwd of the currently focused terminal, force-refreshed from the PTY.
    fn active_terminal_cwd(&self, cx: &App) -> Option<PathBuf> {
        self.active_terminal_view(cx).and_then(|tv| {
            tv.read(cx)
                .terminal()
                .read(cx)
                .latest_working_directory()
        })
    }

    /// Adds a new terminal to the given group, switching to it if needed.
    fn new_terminal_in_group(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.groups.iter().any(|group| group.id == group_id) {
            return;
        }
        let source = self.active_terminal_view(cx);
        let cwd = self.active_terminal_cwd(cx);
        if group_id != self.active_group_id {
            self.switch_group(group_id, window, cx);
        }
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }
        self.spawn_terminal(group_id, cwd, source, window, cx);
        self.save_session(cx);
    }

    fn build_group_context_menu(
        panel: Entity<Self>,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, move |menu, _, _| {
            let view1 = panel.clone();
            let view2 = panel.clone();
            let view3 = panel.clone();
            let view4 = panel.clone();
            menu.entry(i18n::t("rename"), None, move |window, cx| {
                view1.update(cx, |this, cx| {
                    this.start_renaming(group_id, window, cx);
                });
            })
            .entry(i18n::t("move_up"), None, move |_window, cx| {
                view2.update(cx, |this, cx| {
                    this.move_group_up(group_id, cx);
                });
            })
            .entry(i18n::t("move_down"), None, move |_window, cx| {
                view3.update(cx, |this, cx| {
                    this.move_group_down(group_id, cx);
                });
            })
            .separator()
            .entry(i18n::t("new_terminal"), None, move |window, cx| {
                view4.update(cx, |this, cx| {
                    this.new_terminal_in_group(group_id, window, cx);
                });
            })
        })
    }

    /// Creates a new group, switches to it and spawns its first terminal.
    fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.active_terminal_view(cx);
        let cwd = self.active_terminal_cwd(cx);
        let id = self.new_group_id();
        let name = SharedString::from(format!("{} {}", i18n::t("group"), id.0 + 1));
        self.groups.push(TerminalGroup {
            id,
            name,
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
            saved_layout: None,
        });
        self.switch_group(id, window, cx);
        self.spawn_terminal(id, cwd, source, window, cx);
        self.save_session(cx);
    }

    /// Spawns a shell terminal and attaches it to the given group. The
    /// terminal is only displayed if that group is still active by the time
    /// the (async) terminal is ready.
    ///
    /// When `source` is set, clones that terminal (shell + env) into `cwd`.
    fn spawn_terminal(
        &mut self,
        group_id: GroupId,
        cwd: Option<std::path::PathBuf>,
        source: Option<Entity<TerminalView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.clone();
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let working_directory = cwd.or_else(|| {
                workspace.upgrade().and_then(|workspace| {
                    cx.update(|_window, cx| {
                        terminal_view::default_working_directory(workspace.read(cx), cx)
                    })
                    .ok()
                    .flatten()
                })
            });
            let terminal = project
                .update(cx, |project, cx| match source.as_ref() {
                    Some(view) => {
                        let terminal = view.read(cx).terminal().clone();
                        project.clone_terminal(&terminal, cx, working_directory.clone())
                    }
                    None => project.create_terminal_shell(working_directory, cx),
                })?
                .await?;

            this.update_in(cx, |this, window, cx| {
                let Some(workspace) = this.workspace.upgrade() else {
                    return;
                };
                let weak_workspace = workspace.read(cx).weak_handle();
                let workspace_id = workspace.read(cx).database_id();
                let weak_project = workspace.read(cx).project().downgrade();
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

                this.add_terminal_to_group(group_id, terminal_view, cx);

                // Reconcile the display pane so only the active group's
                // terminals are shown (handles races during session restore).
                if group_id == this.active_group_id {
                    this.sync_active_group_to_pane(window, cx);
                }
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn on_worktree_added(
        &mut self,
        worktree_id: project::WorktreeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.upgrade() else {
            return;
        };
        let Some(worktree) = project.read(cx).worktree_for_id(worktree_id, cx) else {
            return;
        };
        let worktree = worktree.read(cx);
        if !worktree.is_visible() {
            return;
        }
        let Some(cwd) = Self::cwd_for_worktree(worktree) else {
            return;
        };

        if self.groups.iter().any(|group| {
            group.terminals.iter().any(|tv| {
                tv.read(cx)
                    .terminal()
                    .read(cx)
                    .working_directory()
                    .as_ref()
                    == Some(&cwd)
            })
        }) {
            return;
        }

        if self.groups.is_empty() {
            let id = self.new_group_id();
            self.groups.push(TerminalGroup {
                id,
                name: SharedString::from(i18n::t("terminals")),
                terminals: Vec::new(),
                collapsed: false,
                has_unread: false,
                saved_layout: None,
            });
            self.active_group_id = id;
        }
        let group_id = self.active_group_id;
        self.spawn_terminal(group_id, Some(cwd), None, window, cx);
        self.save_session(cx);
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

    /// Shows only the active group's terminals across all center panes.
    ///
    /// Split panes stay within the active group: a terminal already visible in
    /// any pane is left there (never copied into another pane — that would
    /// share one PTY across splits). Terminals from other groups are removed
    /// from every pane, and empty panes left behind are closed so the active
    /// group's terminals fill the center.
    ///
    /// Prefer [`Self::switch_group`] when changing groups so split layouts are
    /// saved and restored. This method is for in-group updates (spawn, etc.).
    fn sync_active_group_to_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let active_terminals: Vec<Entity<TerminalView>> = self
            .groups
            .iter()
            .find(|g| g.id == self.active_group_id)
            .map(|g| g.terminals.clone())
            .unwrap_or_default();
        let active_ids: std::collections::HashSet<EntityId> =
            active_terminals.iter().map(|tv| tv.entity_id()).collect();

        let panes = workspace.read(cx).panes().to_vec();

        let owns_switching_guard = !self.switching;
        if owns_switching_guard {
            self.switching = true;
        }

        for pane in &panes {
            pane.update(cx, |pane, cx| {
                let to_remove: Vec<EntityId> = pane
                    .items()
                    .filter_map(|item| {
                        let id = item.item_id();
                        if item.act_as::<TerminalView>(cx).is_some() && !active_ids.contains(&id)
                        {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect();
                for id in to_remove {
                    pane.remove_item(id, false, false, window, cx);
                }
            });
        }

        let panes = workspace.read(cx).panes().to_vec();
        let mut visible: std::collections::HashSet<EntityId> =
            std::collections::HashSet::default();
        for pane in &panes {
            for item in pane.read(cx).items() {
                if active_ids.contains(&item.item_id()) {
                    visible.insert(item.item_id());
                }
            }
        }

        let target_pane = panes
            .iter()
            .find(|pane| {
                pane.read(cx)
                    .items()
                    .any(|item| active_ids.contains(&item.item_id()))
            })
            .cloned()
            .unwrap_or_else(|| workspace.read(cx).active_pane().clone());
        self.display_pane = target_pane.downgrade();

        target_pane.update(cx, |pane, cx| {
            for terminal_view in &active_terminals {
                let id = terminal_view.entity_id();
                if visible.contains(&id) {
                    continue;
                }
                pane.add_item(
                    Box::new(terminal_view.clone()),
                    false,
                    false,
                    None,
                    window,
                    cx,
                );
                visible.insert(id);
            }
        });

        let panes = workspace.read(cx).panes().to_vec();
        if panes.len() > 1 {
            for pane in panes {
                if pane.entity_id() == target_pane.entity_id() {
                    continue;
                }
                let has_terminal = pane
                    .read(cx)
                    .items()
                    .any(|item| item.act_as::<TerminalView>(cx).is_some());
                if !has_terminal {
                    pane.update(cx, |_, cx| {
                        cx.emit(workspace::pane::Event::Remove {
                            focus_on_pane: Some(target_pane.clone()),
                        });
                    });
                }
            }
        }

        let focus_item = true;
        if let Some(last) = active_terminals.last() {
            let id = last.entity_id();
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                let index = pane.read(cx).items().position(|item| item.item_id() == id);
                if let Some(index) = index {
                    pane.update(cx, |pane, cx| {
                        pane.activate_item(index, true, focus_item, window, cx);
                    });
                    break;
                }
            }
        }

        if owns_switching_guard {
            self.switching = false;
        }
    }

    /// Schedules a pane sync after the current update stack unwinds, so we
    /// never nest Workspace/Pane updates (which panics in gpui).
    fn defer_sync_active_group_to_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |this, window, cx| {
            this.sync_active_group_to_pane(window, cx);
        });
    }

    fn capture_layout_from_member(
        member: &Member,
        group_ids: &std::collections::HashSet<EntityId>,
        cx: &App,
    ) -> Option<GroupLayoutNode> {
        match member {
            Member::Pane(pane) => {
                let terminals: Vec<EntityId> = pane
                    .read(cx)
                    .items()
                    .filter_map(|item| {
                        let id = item.item_id();
                        group_ids.contains(&id).then_some(id)
                    })
                    .collect();
                if terminals.is_empty() {
                    return None;
                }
                let active = pane
                    .read(cx)
                    .active_item()
                    .map(|item| item.item_id())
                    .filter(|id| group_ids.contains(id));
                Some(GroupLayoutNode::Pane { terminals, active })
            }
            Member::Axis(axis) => {
                let children: Vec<GroupLayoutNode> = axis
                    .members
                    .iter()
                    .filter_map(|child| Self::capture_layout_from_member(child, group_ids, cx))
                    .collect();
                match children.len() {
                    0 => None,
                    1 => children.into_iter().next(),
                    _ => Some(GroupLayoutNode::Split {
                        axis: axis.axis,
                        flexes: axis.flexes.lock().clone(),
                        children,
                    }),
                }
            }
        }
    }

    fn capture_active_group_layout(&self, cx: &App) -> Option<GroupLayoutNode> {
        let workspace = self.workspace.upgrade()?;
        let group_ids: std::collections::HashSet<EntityId> = self
            .groups
            .iter()
            .find(|g| g.id == self.active_group_id)?
            .terminals
            .iter()
            .map(|tv| tv.entity_id())
            .collect();
        if group_ids.is_empty() {
            return None;
        }
        Self::capture_layout_from_member(&workspace.read(cx).center_root(), &group_ids, cx)
    }

    fn clear_center_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let panes = workspace.read(cx).panes().to_vec();
        for pane in panes {
            pane.update(cx, |pane, cx| {
                let ids: Vec<EntityId> = pane
                    .items()
                    .filter_map(|item| {
                        item.act_as::<TerminalView>(cx)
                            .map(|tv| tv.entity_id())
                    })
                    .collect();
                for id in ids {
                    pane.remove_item(id, false, false, window, cx);
                }
            });
        }
    }

    fn build_layout_member(
        workspace: &mut Workspace,
        node: &GroupLayoutNode,
        terminals: &HashMap<EntityId, Entity<TerminalView>>,
        window: &mut Window,
        cx: &mut gpui::Context<Workspace>,
    ) -> (Member, Entity<Pane>, Option<EntityId>) {
        match node {
            GroupLayoutNode::Pane {
                terminals: ids,
                active,
            } => {
                let pane = workspace.create_center_pane(window, cx);
                for id in ids {
                    if let Some(tv) = terminals.get(id) {
                        pane.update(cx, |pane, cx| {
                            pane.add_item(
                                Box::new(tv.clone()),
                                false,
                                false,
                                None,
                                window,
                                cx,
                            );
                        });
                    }
                }
                if let Some(active_id) = active {
                    let index = pane
                        .read(cx)
                        .items()
                        .position(|item| item.item_id() == *active_id);
                    if let Some(index) = index {
                        pane.update(cx, |pane, cx| {
                            pane.activate_item(index, true, true, window, cx);
                        });
                    }
                }
                (Member::Pane(pane.clone()), pane, *active)
            }
            GroupLayoutNode::Split {
                axis,
                flexes,
                children,
            } => {
                let mut members = Vec::new();
                let mut focus_pane = None;
                let mut focus_terminal = None;
                for child in children {
                    let (member, pane, active) =
                        Self::build_layout_member(workspace, child, terminals, window, cx);
                    if focus_pane.is_none() {
                        focus_pane = Some(pane.clone());
                    }
                    if active.is_some() {
                        focus_pane = Some(pane);
                        focus_terminal = active;
                    }
                    members.push(member);
                }
                let focus_pane = focus_pane
                    .or_else(|| members.first().and_then(|m| match m {
                        Member::Pane(p) => Some(p.clone()),
                        _ => None,
                    }))
                    .expect("split layout has children");
                let axis = PaneAxis::load(*axis, members, Some(flexes.clone()));
                (Member::Axis(axis), focus_pane, focus_terminal)
            }
        }
    }

    fn restore_active_group_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let (terminals, layout) = match self.groups.iter().find(|g| g.id == self.active_group_id) {
            Some(group) => (group.terminals.clone(), group.saved_layout.clone()),
            None => return,
        };

        if terminals.is_empty() {
            self.clear_center_terminals(window, cx);
            // Leave a single empty pane.
            let panes = workspace.read(cx).panes().to_vec();
            if let Some(keep) = panes.first().cloned() {
                for pane in panes.into_iter().skip(1) {
                    pane.update(cx, |_, cx| {
                        cx.emit(workspace::pane::Event::Remove {
                            focus_on_pane: Some(keep.clone()),
                        });
                    });
                }
            }
            return;
        }

        let terminals_by_id: HashMap<EntityId, Entity<TerminalView>> = terminals
            .iter()
            .map(|tv| (tv.entity_id(), tv.clone()))
            .collect();

        match layout {
            Some(layout) => {
                self.clear_center_terminals(window, cx);
                workspace.update(cx, |workspace, cx| {
                    let (root, focus_pane, _) =
                        Self::build_layout_member(workspace, &layout, &terminals_by_id, window, cx);
                    let new_center = PaneGroup::with_root(root);
                    workspace.replace_center_layout(new_center, focus_pane.clone(), window, cx);
                });
                self.display_pane = workspace.read(cx).active_pane().downgrade();
                // Terminals created after the layout was saved still need a home.
                self.sync_active_group_to_pane(window, cx);
            }
            None => {
                // No saved splits — show all terminals as tabs in one pane.
                self.sync_active_group_to_pane(window, cx);
            }
        }
    }

    /// Switches the display pane to show the given group's terminals as tabs,
    /// preserving each group's split layout across switches.
    fn switch_group(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.groups.iter().any(|g| g.id == group_id) {
            return;
        }

        if group_id == self.active_group_id {
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
                group.collapsed = false;
            }
            cx.notify();
            return;
        }

        // Persist the outgoing group's center layout before tearing it down.
        if let Some(layout) = self.capture_active_group_layout(cx) {
            if let Some(group) = self
                .groups
                .iter_mut()
                .find(|g| g.id == self.active_group_id)
            {
                group.saved_layout = Some(layout);
            }
        }

        self.active_group_id = group_id;
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }

        self.switching = true;
        self.restore_active_group_layout(window, cx);
        self.switching = false;

        self.save_session(cx);
        cx.notify();
    }

    /// Activates a specific terminal's tab in whichever pane is showing it.
    fn focus_terminal(
        &mut self,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_id = terminal_view.entity_id();
        for pane in self.center_panes(cx) {
            let index = pane
                .read(cx)
                .items()
                .position(|item| item.item_id() == target_id);
            if let Some(index) = index {
                pane.update(cx, |pane, cx| {
                    pane.activate_item(index, true, true, window, cx);
                });
                return;
            }
        }
    }

    /// Toggles the collapsed state of a group without switching to it.
    fn toggle_group_collapse(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = !group.collapsed;
        }
        self.save_session(cx);
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
                self.save_session(cx);
            }
            cx.notify();
        }
    }

    fn move_group_up(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index > 0 {
                self.groups.swap(index, index - 1);
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    fn move_group_down(&mut self, group_id: GroupId, cx: &mut Context<Self>) {
        if let Some(index) = self.groups.iter().position(|g| g.id == group_id) {
            if index + 1 < self.groups.len() {
                self.groups.swap(index, index + 1);
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    fn on_workspace_event(
        &mut self,
        event: &workspace::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A tab closed directly in the center pane must be dropped from the
        // model too. During a group switch the removals are intentional, so we
        // must not treat them as user closes or the outgoing group's terminals
        // would be dropped from the model and vanish on switch.
        if self.switching {
            cx.notify();
            return;
        }
        match event {
            workspace::Event::ItemRemoved { item_id } => {
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
                    self.remove_terminal_by_id(*item_id, cx);
                }
            }
            workspace::Event::ItemAdded { item } => {
                // Terminals opened outside the panel (e.g. NewCenterTerminal
                // still handled by terminal_view) must join the active group,
                // and extras from other groups must leave the pane.
                if let Some(tv) = item.act_as::<TerminalView>(cx) {
                    let id = tv.entity_id();
                    let already = self
                        .groups
                        .iter()
                        .any(|g| g.terminals.iter().any(|t| t.entity_id() == id));
                    if !already {
                        self.add_terminal_to_group(self.active_group_id, tv, cx);
                        self.save_session(cx);
                    }
                    // ItemAdded is emitted while Workspace is updating the
                    // pane — sync must wait until that stack unwinds.
                    self.defer_sync_active_group_to_pane(window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn remove_terminal_by_id(&mut self, item_id: EntityId, cx: &App) {
        let mut changed = false;
        for group in &mut self.groups {
            let before = group.terminals.len();
            group.terminals.retain(|tv| tv.entity_id() != item_id);
            changed |= group.terminals.len() != before;
        }
        if changed {
            self.save_session(cx);
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
            let panel = cx.entity().clone();
            let is_renaming = Some(group_id) == self.renaming_group_id;
            let rename_editor = self.rename_editor.clone();

            let on_group_click = cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    this.start_renaming(group_id, window, cx);
                } else {
                    this.switch_group(group_id, window, cx);
                }
            });
            let on_chevron_click = cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_group_collapse(group_id, cx);
            });
            let on_rename_confirm = cx.listener(|this, _: &menu::Confirm, _window, cx| {
                this.commit_rename(cx)
            });
            let on_rename_cancel = cx.listener(|this, _: &menu::Cancel, _window, cx| {
                this.commit_rename(cx)
            });

            rows.push(
                right_click_menu(format!("group-rc-{}", group_id.0))
                    .trigger({
                        let panel = panel.clone();
                        move |_is_open, _window, cx| {
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
                                                .id(SharedString::from(format!(
                                                    "group-chevron-{}",
                                                    group_id.0
                                                )))
                                                .child(
                                                    ui::Icon::new(if is_expanded {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .size(IconSize::Small)
                                                    .color(Color::Muted),
                                                )
                                                .on_click(on_chevron_click),
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
                                            if is_renaming {
                                                this.child(
                                                    div()
                                                        .w_full()
                                                        .h_5()
                                                        .bg(cx.theme().colors().editor_background)
                                                        .on_action(on_rename_confirm)
                                                        .on_action(on_rename_cancel)
                                                        .child(rename_editor),
                                                )
                                            } else {
                                                this.child(
                                                    Label::new(name)
                                                        .size(LabelSize::Small)
                                                        .truncate(),
                                                )
                                                .child(
                                                    Label::new(format!("{count}"))
                                                        .size(LabelSize::Small)
                                                        .color(Color::Muted),
                                                )
                                            }
                                        })
                                        .child(
                                            h_flex().flex_grow(1.).justify_end().child(
                                                PopoverMenu::new(SharedString::from(format!(
                                                    "menu-{}",
                                                    group_id.0
                                                )))
                                                .trigger(
                                                    IconButton::new(
                                                        SharedString::from(format!(
                                                            "menu-btn-{}",
                                                            group_id.0
                                                        )),
                                                        IconName::Ellipsis,
                                                    )
                                                    .icon_size(IconSize::Small),
                                                )
                                                .menu({
                                                    let panel = panel.clone();
                                                    move |window, cx| {
                                                        Some(Self::build_group_context_menu(
                                                            panel.clone(),
                                                            group_id,
                                                            window,
                                                            cx,
                                                        ))
                                                    }
                                                }),
                                            ),
                                        ),
                                )
                                .on_click(on_group_click)
                        }
                    })
                    .menu(move |window, cx| {
                        Self::build_group_context_menu(panel.clone(), group_id, window, cx)
                    })
                    .into_any_element(),
            );

            if is_expanded {
                for (ix, (terminal_view, title)) in terminals.into_iter().enumerate() {
                    let is_terminal_active = Some(terminal_view.entity_id()) == active_terminal_id;
                    let workspace = self.workspace.clone();
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
                                let workspace = workspace.clone();
                                let terminal_view = terminal_view.clone();
                                ContextMenu::build(window, cx, move |menu, _, _| {
                                    menu.entry(i18n::t("rename"), None, move |window, cx| {
                                        terminal_view.update(cx, |this, cx| {
                                            this.rename_terminal(
                                                &terminal_view::RenameTerminal,
                                                window,
                                                cx,
                                            )
                                        });
                                    })
                                    .entry(
                                        i18n::t("close"),
                                        None,
                                        move |window, cx| {
                                            let Some(workspace) = workspace.upgrade() else {
                                                return;
                                            };
                                            let panes = workspace.read(cx).panes().to_vec();
                                            for pane in panes {
                                                let has_item = pane
                                                    .read(cx)
                                                    .items()
                                                    .any(|item| item.item_id() == terminal_id);
                                                if has_item {
                                                    pane.update(cx, |pane, cx| {
                                                        pane.close_item_by_id(
                                                            terminal_id,
                                                            workspace::SaveIntent::Close,
                                                            window,
                                                            cx,
                                                        )
                                                        .detach_and_log_err(cx);
                                                    });
                                                    break;
                                                }
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
                                            Box::new(zed_actions::file_list_panel::ToggleFocus),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("show-agent", IconName::Sparkle)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("agent")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(zed_actions::assistant::ToggleFocus),
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
        2
    }
}
