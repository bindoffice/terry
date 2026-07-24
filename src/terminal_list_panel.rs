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
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Which window currently owns the shared restore file for a workspace key.
/// A second window for the same workspace starts fresh instead of respawning
/// the same PTYs.
fn session_owners() -> &'static Mutex<HashMap<String, EntityId>> {
    static OWNERS: OnceLock<Mutex<HashMap<String, EntityId>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn try_claim_session(workspace_key: &str, owner: EntityId) -> bool {
    let mut owners = session_owners()
        .lock()
        .expect("session owners mutex poisoned");
    match owners.get(workspace_key) {
        Some(existing) if *existing != owner => false,
        _ => {
            owners.insert(workspace_key.to_string(), owner);
            true
        }
    }
}

fn release_session_claim(workspace_key: &str, owner: EntityId) {
    let mut owners = session_owners()
        .lock()
        .expect("session owners mutex poisoned");
    if owners.get(workspace_key) == Some(&owner) {
        owners.remove(workspace_key);
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct PersistedTerminal {
    cwd: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PersistedAxis {
    Horizontal,
    Vertical,
}

impl From<Axis> for PersistedAxis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::Horizontal => Self::Horizontal,
            Axis::Vertical => Self::Vertical,
        }
    }
}

impl From<PersistedAxis> for Axis {
    fn from(axis: PersistedAxis) -> Self {
        match axis {
            PersistedAxis::Horizontal => Self::Horizontal,
            PersistedAxis::Vertical => Self::Vertical,
        }
    }
}

/// Split layout for a terminal group, keyed by index into `group.terminals`
/// so it survives app restarts (EntityIds do not).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GroupLayoutNode {
    Pane {
        terminals: Vec<usize>,
        #[serde(default)]
        active: Option<usize>,
    },
    Split {
        axis: PersistedAxis,
        flexes: Vec<f32>,
        children: Vec<GroupLayoutNode>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedGroup {
    name: String,
    collapsed: bool,
    terminals: Vec<PersistedTerminal>,
    #[serde(default)]
    layout: Option<GroupLayoutNode>,
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
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                return;
            };
            // Must use window.defer (not cx.defer_in): defer_in would re-enter a
            // Workspace update, and new_terminal reads workspace for the active pane.
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            });
        });
        // Welcome page and default keymaps dispatch workspace::NewTerminal.
        // Terry has no TerminalPanel dock, so that handler no-ops — route here.
        // Registered before terminal_view::init so we run first in the bubble phase.
        workspace.register_action(|workspace, _: &workspace::NewTerminal, window, cx| {
            let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
                cx.propagate();
                return;
            };
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
            });
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
    /// Last known center split layout while this group was active.
    saved_layout: Option<GroupLayoutNode>,
    /// How many terminals this group had in the session file (restore waits).
    session_terminal_count: Option<usize>,
}

pub struct TerminalListPanel {
    workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    focus_handle: FocusHandle,
    position: DockPosition,
    _workspace_subscription: Subscription,
    _project_subscription: Subscription,
    _quit_subscription: Subscription,
    /// The center pane that displays the active group's terminals as tabs.
    display_pane: WeakEntity<Pane>,
    /// Pane that should receive the next newly spawned terminal (e.g. where
    /// "+" was clicked). Cleared when consumed by sync.
    pending_spawn_pane: Option<WeakEntity<Pane>>,
    groups: Vec<TerminalGroup>,
    active_group_id: GroupId,
    next_group_id: usize,
    /// Guards against reconciling the model while a group switch is
    /// transiently removing/adding items in the display pane.
    switching: bool,
    /// Outgoing group whose live split layout must be captured before any
    /// sync may clear its terminals from the center panes.
    pending_switch_from: Option<GroupId>,
    /// Apply `saved_layout` once after session restore when terminals are ready.
    pending_layout_restore: bool,
    /// True while session terminals are still spawning. Blocks disk writes and
    /// auto-spawn (worktree / orphan claim) so we neither wipe groups to
    /// `terminals: []` nor inflate counts mid-restore.
    session_restoring: bool,
    /// Ensures [`Self::finish_session_restore`] is deferred at most once.
    session_finish_scheduled: bool,
    /// Stable key for this workspace (db id / project paths / "default").
    session_workspace_key: String,
    /// File stem under `sessions/` — workspace key for the owner, or
    /// `{key}-w{panel_id}` for secondary windows.
    session_file_stem: String,
    /// True when this panel owns the shared restore file for `session_workspace_key`.
    owns_workspace_session: bool,
    _session_release_subscription: Subscription,
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

        let _quit_subscription = cx.on_app_quit(|this, cx| {
            this.refresh_active_layout_and_save(cx);
            async {}
        });

        let panel_id = cx.entity_id();
        let session_workspace_key = Self::compute_workspace_session_key(&workspace, &project, cx);
        let owns_workspace_session = try_claim_session(&session_workspace_key, panel_id);
        let session_file_stem = if owns_workspace_session {
            session_workspace_key.clone()
        } else {
            format!("{}-w{}", session_workspace_key, panel_id.as_u64())
        };

        let _session_release_subscription = cx.on_release(move |this, _cx| {
            if this.owns_workspace_session {
                release_session_claim(&this.session_workspace_key, panel_id);
            }
        });

        Self {
            workspace: workspace.downgrade(),
            project: project.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            _workspace_subscription,
            _project_subscription,
            _quit_subscription,
            display_pane: display_pane.downgrade(),
            pending_spawn_pane: None,
            groups: Vec::new(),
            active_group_id: GroupId(0),
            next_group_id: 0,
            switching: false,
            pending_switch_from: None,
            pending_layout_restore: false,
            session_restoring: false,
            session_finish_scheduled: false,
            session_workspace_key,
            session_file_stem,
            owns_workspace_session,
            _session_release_subscription,
            renaming_group_id: None,
            rename_editor,
        }
    }

    /// Creates the initial group (with one terminal) if none exists yet.

    fn compute_workspace_session_key(
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        cx: &App,
    ) -> String {
        let mut roots: Vec<String> = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .collect();
        roots.sort();
        roots.dedup();
        // Empty welcome windows share one stable key so the first window can
        // restore `default.json` across restarts; later windows are isolated
        // via ownership (they get a `-w{id}` file stem).
        if roots.is_empty() {
            return "default".to_string();
        }
        if let Some(id) = workspace.read(cx).database_id() {
            return format!("ws-{}", i64::from(id));
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        roots.hash(&mut hasher);
        format!("paths-{:x}", hasher.finish())
    }

    fn session_file_path(&self) -> PathBuf {
        paths::data_dir()
            .join("sessions")
            .join(format!("{}.json", self.session_file_stem))
    }

    pub fn save_session(&mut self, cx: &App) {
        // Persist in-memory state only. Do not capture live layout here —
        // save_session is often called from workspace actions where
        // workspace.read would panic (nested entity lease).
        self.write_session_file(cx);
    }

    /// Capture the active group's live split layout, then write the session.
    /// Only call when Workspace is not already being updated.
    fn refresh_active_layout_and_save(&mut self, cx: &App) {
        if self.session_restoring {
            return;
        }
        if let Some(layout) = self.capture_active_group_layout(cx) {
            if let Some(group) = self
                .groups
                .iter_mut()
                .find(|g| g.id == self.active_group_id)
            {
                group.saved_layout = Some(layout);
            }
        }
        self.write_session_file(cx);
    }

    fn session_spawns_complete(&self) -> bool {
        self.groups.iter().all(|group| match group.session_terminal_count {
            None => true,
            Some(expected) => group.terminals.len() >= expected,
        })
    }

    fn clear_session_terminal_counts(&mut self) {
        for group in &mut self.groups {
            group.session_terminal_count = None;
        }
    }

    /// Called when every group's expected session terminals have been spawned.
    fn finish_session_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session_restoring || self.session_finish_scheduled {
            return;
        }
        self.session_finish_scheduled = true;
        // Defer: load/spawn completion may still hold Workspace leases.
        cx.defer_in(window, |this, window, cx| {
            this.session_finish_scheduled = false;
            this.clear_session_terminal_counts();

            // Keep session_restoring true through layout restore so worktree /
            // orphan handlers cannot inflate groups mid-apply.
            if this.pending_layout_restore {
                if this.active_group_layout_ready() {
                    this.pending_layout_restore = false;
                    this.switching = true;
                    this.restore_active_group_layout(window, cx);
                    this.switching = false;
                } else {
                    // Layout references missing terminals (corrupted / wiped
                    // session) — fall back to a flat sync.
                    this.pending_layout_restore = false;
                    this.switching = true;
                    this.sync_active_group_to_pane(window, cx);
                    this.switching = false;
                }
            } else {
                this.switching = true;
                this.sync_active_group_to_pane(window, cx);
                this.switching = false;
            }

            this.session_restoring = false;

            // First safe disk write after restore — preserves full terminal
            // lists and layouts for every group.
            this.write_session_file(cx);
            cx.notify();
        });
    }

    fn write_session_file(&self, cx: &App) {
        // Never persist a half-restored session: async spawns leave
        // `group.terminals` empty until complete, which used to wipe the file
        // to `terminals: []` and drop inactive-group layouts on the next load.
        if self.session_restoring {
            return;
        }

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
                layout: group.saved_layout.clone(),
            };

            for view_ent in &group.terminals {
                let cwd = view_ent.read(cx).terminal().read(cx).working_directory();
                p_group.terminals.push(PersistedTerminal { cwd });
            }

            session.groups.push(p_group);
        }

        let path = self.session_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            let _ = std::fs::write(path, json);
        }
    }

    fn layout_referenced_indices(node: &GroupLayoutNode) -> std::collections::HashSet<usize> {
        let mut indices = std::collections::HashSet::default();
        Self::collect_layout_indices(node, &mut indices);
        indices
    }

    fn collect_layout_indices(
        node: &GroupLayoutNode,
        indices: &mut std::collections::HashSet<usize>,
    ) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                indices.extend(terminals.iter().copied());
                if let Some(active) = active {
                    indices.insert(*active);
                }
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::collect_layout_indices(child, indices);
                }
            }
        }
    }

    fn remap_layout_indices(node: &mut GroupLayoutNode, old_to_new: &HashMap<usize, usize>) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                *terminals = terminals
                    .iter()
                    .filter_map(|i| old_to_new.get(i).copied())
                    .collect();
                *active = active.and_then(|a| old_to_new.get(&a).copied());
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::remap_layout_indices(child, old_to_new);
                }
            }
        }
    }

    pub fn load_persisted_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Secondary windows must not restore the shared session — that would
        // duplicate every PTY from the first window.
        if !self.owns_workspace_session {
            return false;
        }

        let path = self.session_file_path();
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
        let mut active_has_layout = false;
        for (i, p_group) in session.groups.into_iter().enumerate() {
            let id = self.new_group_id();
            let name = SharedString::from(p_group.name);
            let is_active = i == session.active_group_index;

            // If a layout exists, keep only terminals it references. Older bugs
            // (workspace DB restore + Terry session) left orphan terminals in
            // the JSON that are not part of the split — drop them on load.
            let (terminals, layout) = match p_group.layout {
                Some(mut layout) => {
                    let referenced = Self::layout_referenced_indices(&layout);
                    if referenced.is_empty() {
                        // Empty/broken layout — keep all terminals, drop layout.
                        (p_group.terminals, None)
                    } else {
                        let mut keep: Vec<usize> = referenced.into_iter().collect();
                        keep.sort_unstable();
                        keep.retain(|&ix| ix < p_group.terminals.len());
                        let old_to_new: HashMap<usize, usize> = keep
                            .iter()
                            .enumerate()
                            .map(|(new_ix, &old_ix)| (old_ix, new_ix))
                            .collect();
                        Self::remap_layout_indices(&mut layout, &old_to_new);
                        let terminals = keep
                            .iter()
                            .filter_map(|&ix| p_group.terminals.get(ix).cloned())
                            .collect();
                        (terminals, Some(layout))
                    }
                }
                None => (p_group.terminals, None),
            };

            // Drop layouts that still can't be applied after pruning.
            let term_count = terminals.len();
            let layout = layout.and_then(|layout| match Self::layout_max_index(&layout) {
                None => Some(layout),
                Some(max) if term_count > max => Some(layout),
                Some(_) => None,
            });
            if is_active && layout.is_some() {
                active_has_layout = true;
            }
            self.groups.push(TerminalGroup {
                id,
                name,
                terminals: Vec::new(),
                collapsed: p_group.collapsed,
                has_unread: false,
                saved_layout: layout,
                session_terminal_count: Some(term_count),
            });

            if is_active {
                self.active_group_id = id;
            }

            for p_term in terminals {
                to_spawn.push((id, p_term.cwd));
            }
        }

        self.pending_layout_restore = active_has_layout;
        self.session_restoring = true;

        for (id, cwd) in to_spawn {
            self.spawn_terminal(id, cwd, None, None, window, cx);
        }

        // All groups empty (or already satisfied): finish immediately.
        if self.session_spawns_complete() {
            self.finish_session_restore(window, cx);
        }
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
                session_terminal_count: None,
            });
            self.active_group_id = id;
            for cwd in project_dirs {
                self.spawn_terminal(id, Some(cwd), None, None, window, cx);
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
            session_terminal_count: None,
        });
        self.active_group_id = id;
        self.spawn_terminal(id, None, None, None, window, cx);
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
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_with_destination(None, window, cx);
    }

    /// Pin the pane that should receive the next spawned terminal.
    pub fn pin_spawn_pane(&mut self, pane: Entity<Pane>) {
        self.pending_spawn_pane = Some(pane.downgrade());
    }

    /// Create a terminal, optionally forcing it into `destination` (the split
    /// whose "+" was clicked). Prefer this over pin + new_terminal so the
    /// destination cannot be cleared by a mid-flight sync.
    pub fn new_terminal_with_destination(
        &mut self,
        destination: Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = destination
            .or_else(|| self.pending_spawn_pane.take().and_then(|p| p.upgrade()))
            .or_else(|| self.display_pane_entity(cx));
        if let Some(pane) = destination.as_ref() {
            self.pending_spawn_pane = Some(pane.downgrade());
            self.display_pane = pane.downgrade();
        }
        self.new_terminal_in_group_with_destination(
            self.active_group_id,
            destination,
            window,
            cx,
        );
    }

    /// Name of the currently selected group, if any.
    pub fn active_group_name(&self) -> Option<SharedString> {
        self.groups
            .iter()
            .find(|group| group.id == self.active_group_id)
            .map(|group| group.name.clone())
    }

    /// Currently focused terminal view (pane active item, else last in the
    /// active group).
    pub fn active_terminal_view(&self, cx: &App) -> Option<Entity<TerminalView>> {
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
    pub fn active_terminal_cwd(&self, cx: &App) -> Option<PathBuf> {
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
        self.new_terminal_in_group_with_destination(group_id, None, window, cx);
    }

    fn new_terminal_in_group_with_destination(
        &mut self,
        group_id: GroupId,
        destination: Option<Entity<Pane>>,
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
        self.spawn_terminal(group_id, cwd, source, destination, window, cx);
        self.save_session(cx);
    }

    fn build_group_context_menu(
        panel: Entity<Self>,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        let can_delete = panel.read(cx).groups.len() > 1;
        ContextMenu::build(window, cx, move |menu, _, _| {
            let view1 = panel.clone();
            let view2 = panel.clone();
            let view3 = panel.clone();
            let view4 = panel.clone();
            let view5 = panel.clone();
            let menu = menu
                .entry(i18n::t("rename"), None, move |window, cx| {
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
                });
            if can_delete {
                menu.separator().entry(i18n::t("delete_group"), None, move |window, cx| {
                    view5.update(cx, |this, cx| {
                        this.delete_group(group_id, window, cx);
                    });
                })
            } else {
                menu
            }
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
            session_terminal_count: None,
        });
        self.switch_group(id, window, cx);
        self.spawn_terminal(id, cwd, source, None, window, cx);
        self.save_session(cx);
    }

    /// Deletes a group and drops all of its `TerminalView` entities so PTYs
    /// are killed. The last remaining group cannot be deleted.
    fn delete_group(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.groups.len() <= 1 {
            return;
        }
        let Some(index) = self.groups.iter().position(|g| g.id == group_id) else {
            return;
        };

        let was_active = self.active_group_id == group_id;
        let removed = self.groups.remove(index);
        let terminal_ids: std::collections::HashSet<EntityId> =
            removed.terminals.iter().map(|tv| tv.entity_id()).collect();

        // Pull closed items out of panes before dropping Entities.
        self.switching = true;
        if let Some(workspace) = self.workspace.upgrade() {
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                pane.update(cx, |pane, cx| {
                    for id in &terminal_ids {
                        if pane.items().any(|item| item.item_id() == *id) {
                            pane.remove_item(*id, false, false, window, cx);
                        }
                    }
                });
            }
        }

        if self.renaming_group_id == Some(group_id) {
            self.renaming_group_id = None;
        }

        if was_active {
            let next_index = index.min(self.groups.len().saturating_sub(1));
            let next_id = self.groups[next_index].id;
            self.active_group_id = next_id;
            self.pending_switch_from = None;
            if let Some(group) = self.groups.iter_mut().find(|g| g.id == next_id) {
                group.collapsed = false;
            }
            self.restore_active_group_layout(window, cx);
        }
        self.switching = false;

        // Drop TerminalViews (and their Terminal/PTY) — last strong refs.
        drop(removed);

        self.save_session(cx);
        cx.notify();
    }

    /// Closes a terminal: remove from every pane, then drop the model Entity
    /// so the PTY is killed even if the terminal belonged to an inactive group.
    fn close_terminal(
        &mut self,
        terminal_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            let panes = workspace.read(cx).panes().to_vec();
            for pane in panes {
                let has_item = pane
                    .read(cx)
                    .items()
                    .any(|item| item.item_id() == terminal_id);
                if has_item {
                    pane.update(cx, |pane, cx| {
                        pane.remove_item(terminal_id, false, false, window, cx);
                    });
                }
            }
        }
        self.remove_terminal_by_id(terminal_id, window, cx);
    }

    /// Spawns a shell terminal and attaches it to the given group. The
    /// terminal is only displayed if that group is still active by the time
    /// the (async) terminal is ready.
    ///
    /// When `source` is set, clones that terminal (shell + env) into `cwd`.
    /// When `destination` is set, the new tab is added to that pane (the split
    /// whose "+" was clicked) instead of whatever happens to be active.
    fn spawn_terminal(
        &mut self,
        group_id: GroupId,
        cwd: Option<std::path::PathBuf>,
        source: Option<Entity<TerminalView>>,
        destination: Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.clone();
        let workspace = self.workspace.clone();
        // Prefer the explicit destination; fall back to any leftover pin.
        let destination_pane = destination
            .map(|p| p.downgrade())
            .or_else(|| self.pending_spawn_pane.take());
        // Clear pin so a later sync cannot steal a stale target.
        self.pending_spawn_pane = None;
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

                this.add_terminal_to_group(group_id, terminal_view.clone(), cx);

                if this.session_restoring && this.session_spawns_complete() {
                    cx.defer_in(window, |this, window, cx| {
                        this.finish_session_restore(window, cx);
                    });
                    return;
                }

                if group_id != this.active_group_id {
                    return;
                }
                if this.session_restoring || this.pending_layout_restore {
                    return;
                }

                let dest = destination_pane.and_then(|p| p.upgrade()).filter(|pane| {
                    workspace.read(cx).panes().iter().any(|p| p == pane)
                });

                if let Some(pane) = dest {
                    let terminal_id = terminal_view.entity_id();
                    let already_visible = workspace.read(cx).panes().iter().any(|p| {
                        p.read(cx)
                            .items()
                            .any(|item| item.item_id() == terminal_id)
                    });
                    if !already_visible {
                        // Keep switching through the deferred ItemAdded so
                        // sync_active_group_to_pane does not re-home this tab
                        // onto workspace.active_pane.
                        this.switching = true;
                        pane.update(cx, |pane, cx| {
                            pane.add_item(
                                Box::new(terminal_view),
                                true,
                                true,
                                None,
                                window,
                                cx,
                            );
                        });
                        this.display_pane = pane.downgrade();
                        if let Some(layout) = this.capture_active_group_layout(cx) {
                            if let Some(group) = this
                                .groups
                                .iter_mut()
                                .find(|g| g.id == this.active_group_id)
                            {
                                group.saved_layout = Some(layout);
                            }
                        }
                        this.write_session_file(cx);
                        cx.defer_in(window, |this, _window, _cx| {
                            this.switching = false;
                        });
                        return;
                    }
                }

                cx.defer_in(window, move |this, window, cx| {
                    if this.active_group_id != group_id {
                        return;
                    }
                    if this.session_restoring || this.pending_layout_restore {
                        return;
                    }
                    this.sync_active_group_to_pane(window, cx);
                });
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
        // Restoring a session must not auto-spawn project terminals — that was
        // inflating group counts across restarts (cwd check fails while PTYs
        // are still coming up).
        if self.session_restoring {
            return;
        }
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
                session_terminal_count: None,
            });
            self.active_group_id = id;
        }
        let group_id = self.active_group_id;
        self.spawn_terminal(group_id, Some(cwd), None, None, window, cx);
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
        // While a group switch is waiting to capture the outgoing layout, or a
        // session layout restore is pending, do not reconcile — that would
        // strip the outgoing/persisted split and write layout: null.
        if self.pending_switch_from.is_some() || self.pending_layout_restore {
            return;
        }

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

        // Prefer: pane pinned by NewTerminal ("+" / menu) → active center pane
        // → first pane that already shows this group. Never dump new tabs into
        // an arbitrary first split when the user clicked another one.
        let active_pane = workspace.read(cx).active_pane().clone();
        let has_missing = active_terminals
            .iter()
            .any(|tv| !visible.contains(&tv.entity_id()));
        // Only consume the pin when we are about to place a new tab; other
        // syncs (ItemAdded mid-spawn) must not clear it early.
        let pinned = if has_missing {
            self.pending_spawn_pane
                .take()
                .and_then(|p| p.upgrade())
                .filter(|pane| panes.iter().any(|p| p == pane))
        } else {
            None
        };
        let target_pane = pinned.unwrap_or_else(|| {
            if panes.iter().any(|pane| pane == &active_pane) {
                active_pane
            } else {
                panes
                    .iter()
                    .find(|pane| {
                        pane.read(cx)
                            .items()
                            .any(|item| active_ids.contains(&item.item_id()))
                    })
                    .cloned()
                    .unwrap_or(active_pane)
            }
        });
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

        // Close empty leftover panes. `cx.emit(Remove)` is deferred, so we must
        // not loop waiting for panes().len() to shrink (that busy-waits forever).
        // Workspace::remove_pane treats "already gone" as a no-op if several
        // Removes collapse the same split tree.
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
        // Prefer the pane we just filled (or the pinned/active target), not the
        // first center pane that happens to contain the last terminal id.
        let last = active_terminals.last().cloned();
        if let Some(last) = last {
            let id = last.entity_id();
            let index = target_pane
                .read(cx)
                .items()
                .position(|item| item.item_id() == id);
            if let Some(index) = index {
                target_pane.update(cx, |pane, cx| {
                    pane.activate_item(index, true, focus_item, window, cx);
                });
            } else {
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
        }

        if owns_switching_guard {
            // Keep saved_layout fresh after in-group splits/tab changes —
            // but never while a session layout restore is still pending
            // (progressive flat sync must not clobber the persisted tree).
            if !self.pending_layout_restore {
                if let Some(layout) = self.capture_active_group_layout(cx) {
                    if let Some(group) = self
                        .groups
                        .iter_mut()
                        .find(|g| g.id == self.active_group_id)
                    {
                        group.saved_layout = Some(layout);
                    }
                    // Persist immediately so every group's split survives
                    // restart even if the user never switches away.
                    self.write_session_file(cx);
                }
            }
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

    fn layout_max_index(node: &GroupLayoutNode) -> Option<usize> {
        match node {
            GroupLayoutNode::Pane { terminals, active } => terminals
                .iter()
                .copied()
                .chain(active.iter().copied())
                .max(),
            GroupLayoutNode::Split { children, .. } => {
                children.iter().filter_map(Self::layout_max_index).max()
            }
        }
    }

    fn active_group_layout_ready(&self) -> bool {
        let Some(group) = self.groups.iter().find(|g| g.id == self.active_group_id) else {
            return false;
        };
        if let Some(expected) = group.session_terminal_count {
            if group.terminals.len() < expected {
                return false;
            }
        }
        match &group.saved_layout {
            None => true,
            Some(layout) => match Self::layout_max_index(layout) {
                None => true,
                // Indices are 0-based; need len > max (== max+1 terminals).
                Some(max) => group.terminals.len() > max,
            },
        }
    }

    fn capture_layout_from_member(
        member: &Member,
        id_to_index: &HashMap<EntityId, usize>,
        cx: &App,
    ) -> Option<GroupLayoutNode> {
        match member {
            Member::Pane(pane) => {
                let terminals: Vec<usize> = pane
                    .read(cx)
                    .items()
                    .filter_map(|item| id_to_index.get(&item.item_id()).copied())
                    .collect();
                if terminals.is_empty() {
                    return None;
                }
                let active = pane
                    .read(cx)
                    .active_item()
                    .and_then(|item| id_to_index.get(&item.item_id()).copied());
                Some(GroupLayoutNode::Pane { terminals, active })
            }
            Member::Axis(axis) => {
                let children: Vec<GroupLayoutNode> = axis
                    .members
                    .iter()
                    .filter_map(|child| Self::capture_layout_from_member(child, id_to_index, cx))
                    .collect();
                match children.len() {
                    0 => None,
                    1 => children.into_iter().next(),
                    _ => Some(GroupLayoutNode::Split {
                        axis: PersistedAxis::from(axis.axis),
                        flexes: axis.flexes.lock().clone(),
                        children,
                    }),
                }
            }
        }
    }

    fn capture_active_group_layout(&self, cx: &App) -> Option<GroupLayoutNode> {
        self.capture_group_layout(self.active_group_id, cx)
    }

    fn capture_group_layout(&self, group_id: GroupId, cx: &App) -> Option<GroupLayoutNode> {
        let workspace = self.workspace.upgrade()?;
        let group = self.groups.iter().find(|g| g.id == group_id)?;
        if group.terminals.is_empty() {
            return None;
        }
        let id_to_index: HashMap<EntityId, usize> = group
            .terminals
            .iter()
            .enumerate()
            .map(|(i, tv)| (tv.entity_id(), i))
            .collect();
        Self::capture_layout_from_member(&workspace.read(cx).center_root(), &id_to_index, cx)
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
        terminals: &[Entity<TerminalView>],
        window: &mut Window,
        cx: &mut gpui::Context<Workspace>,
    ) -> (Member, Entity<Pane>, Option<usize>) {
        match node {
            GroupLayoutNode::Pane {
                terminals: indices,
                active,
            } => {
                let pane = workspace.create_center_pane(window, cx);
                for &ix in indices {
                    if let Some(tv) = terminals.get(ix) {
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
                if let Some(active_ix) = active {
                    if let Some(tv) = terminals.get(*active_ix) {
                        let id = tv.entity_id();
                        let index = pane.read(cx).items().position(|item| item.item_id() == id);
                        if let Some(index) = index {
                            pane.update(cx, |pane, cx| {
                                pane.activate_item(index, true, true, window, cx);
                            });
                        }
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
                    .or_else(|| {
                        members.first().and_then(|m| match m {
                            Member::Pane(p) => Some(p.clone()),
                            _ => None,
                        })
                    })
                    .expect("split layout has children");
                let axis = PaneAxis::load((*axis).into(), members, Some(flexes.clone()));
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

        match layout {
            Some(layout) => {
                self.clear_center_terminals(window, cx);
                workspace.update(cx, |workspace, cx| {
                    let (root, focus_pane, _) =
                        Self::build_layout_member(workspace, &layout, &terminals, window, cx);
                    let new_center = PaneGroup::with_root(root);
                    workspace.replace_center_layout(new_center, focus_pane.clone(), window, cx);
                });
                self.display_pane = workspace.read(cx).active_pane().downgrade();
                // Terminals created after the layout was saved still need a home.
                self.sync_active_group_to_pane(window, cx);
            }
            None => {
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

        let outgoing = self.active_group_id;
        // Block sync until we have snapshotted `outgoing`'s live split layout.
        // Otherwise a deferred sync (ItemAdded / spawn) clears those panes and
        // the capture writes layout: null for every group except the last one.
        self.pending_switch_from = Some(outgoing);
        self.active_group_id = group_id;
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.collapsed = false;
        }
        cx.notify();

        cx.defer_in(window, move |this, window, cx| {
            // Snapshot this switch's outgoing group while sync is still blocked
            // (pending_switch_from is Some), so its panes are intact.
            if let Some(layout) = this.capture_group_layout(outgoing, cx) {
                if let Some(group) = this.groups.iter_mut().find(|g| g.id == outgoing) {
                    group.saved_layout = Some(layout);
                }
            }

            // Only the switch that owns the current pending flag may clear it
            // and perform the restore (handles rapid A→B→C switches).
            if this.pending_switch_from == Some(outgoing) {
                this.pending_switch_from = None;
            }

            if this.active_group_id != group_id || this.pending_switch_from.is_some() {
                this.write_session_file(cx);
                return;
            }

            this.switching = true;
            this.restore_active_group_layout(window, cx);
            this.switching = false;
            this.write_session_file(cx);
            cx.notify();
        });
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
        if self.switching || self.pending_switch_from.is_some() || self.session_restoring {
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
                    self.remove_terminal_by_id(*item_id, window, cx);
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
                    if already {
                        // Already tracked (e.g. "+" placement into a specific
                        // split). Do not sync — that would move the tab to
                        // workspace.active_pane.
                        return;
                    }
                    self.add_terminal_to_group(self.active_group_id, tv, cx);
                    self.save_session(cx);
                    // ItemAdded is emitted while Workspace is updating the
                    // pane — sync must wait until that stack unwinds.
                    self.defer_sync_active_group_to_pane(window, cx);
                }
            }
            // MovePane splits do not emit ItemAdded; still refresh layout.
            workspace::Event::PaneAdded(_) | workspace::Event::PaneRemoved => {
                cx.defer_in(window, |this, _window, cx| {
                    if this.switching
                        || this.pending_switch_from.is_some()
                        || this.session_restoring
                        || this.pending_layout_restore
                    {
                        return;
                    }
                    this.refresh_active_layout_and_save(cx);
                });
            }
            _ => {}
        }
        cx.notify();
    }

    fn remove_terminal_by_id(
        &mut self,
        item_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        let mut emptied_group: Option<GroupId> = None;
        for group in &mut self.groups {
            if let Some(removed_ix) = group
                .terminals
                .iter()
                .position(|tv| tv.entity_id() == item_id)
            {
                // Drop the Entity so Terminal::Drop kills the PTY. Pane refs
                // should already be gone when this runs after ItemRemoved.
                let _dropped = group.terminals.remove(removed_ix);
                if let Some(layout) = group.saved_layout.as_mut() {
                    Self::remap_layout_after_remove(layout, removed_ix);
                }
                if group.terminals.is_empty() {
                    emptied_group = Some(group.id);
                }
                changed = true;
                break;
            }
        }
        if !changed {
            return;
        }

        // Drop empty groups (keep at least one).
        if let Some(empty_id) = emptied_group
            && self.groups.len() > 1
        {
            if let Some(index) = self.groups.iter().position(|g| g.id == empty_id) {
                let was_active = self.active_group_id == empty_id;
                self.groups.remove(index);
                if was_active {
                    let next_index = index.min(self.groups.len().saturating_sub(1));
                    let next_id = self.groups[next_index].id;
                    self.active_group_id = next_id;
                    self.pending_switch_from = None;
                    self.switching = true;
                    if let Some(group) = self.groups.iter_mut().find(|g| g.id == next_id) {
                        group.collapsed = false;
                    }
                    self.restore_active_group_layout(window, cx);
                    self.switching = false;
                }
            }
        }

        self.save_session(cx);
        cx.notify();
    }

    fn remap_layout_after_remove(node: &mut GroupLayoutNode, removed: usize) {
        match node {
            GroupLayoutNode::Pane { terminals, active } => {
                terminals.retain(|&i| i != removed);
                for i in terminals.iter_mut() {
                    if *i > removed {
                        *i -= 1;
                    }
                }
                match active {
                    Some(a) if *a == removed => *active = None,
                    Some(a) if *a > removed => *a -= 1,
                    _ => {}
                }
            }
            GroupLayoutNode::Split { children, .. } => {
                for child in children {
                    Self::remap_layout_after_remove(child, removed);
                }
            }
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
                    terminals.push((tv.clone(), sidebar_terminal_name(tv, cx)));
                }
            }
            groups_snapshot.push((
                group.id,
                group.name.clone(),
                is_active,
                collapsed,
                terminals,
            ));
        }

        let mut rows: Vec<AnyElement> = Vec::new();
        for (group_id, name, is_active, collapsed, terminals) in groups_snapshot {
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
                    .menu({
                        let panel = panel.clone();
                        move |window, cx| {
                            Self::build_group_context_menu(panel.clone(), group_id, window, cx)
                        }
                    })
                    .into_any_element(),
            );

            if is_expanded {
                for (ix, (terminal_view, title)) in terminals.into_iter().enumerate() {
                    let is_terminal_active = Some(terminal_view.entity_id()) == active_terminal_id;
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
                                    // Match group row padding; spacer below equals the
                                    // chevron so the terminal icon lines up with Folder.
                                    .px_2()
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
                                            .child(div().size(IconSize::Small.rems()))
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
                            .menu({
                                let terminal_view = terminal_view.clone();
                                let panel = panel.clone();
                                move |window, cx| {
                                    let terminal_view = terminal_view.clone();
                                    let panel = panel.clone();
                                    ContextMenu::build(window, cx, move |menu, _, _| {
                                        let panel = panel.clone();
                                        menu.entry(i18n::t("rename"), None, move |window, cx| {
                                            terminal_view.update(cx, |this, cx| {
                                                this.rename_terminal(
                                                    &terminal_view::RenameTerminal,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        })
                                        .entry(i18n::t("close"), None, move |window, cx| {
                                            panel.update(cx, |this, cx| {
                                                this.close_terminal(terminal_id, window, cx);
                                            });
                                        })
                                    })
                                }
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

/// Sidebar label: custom title, else cwd basename — never appends the shell
/// name (`dir — zsh`).
fn sidebar_terminal_name(tv: &Entity<TerminalView>, cx: &App) -> SharedString {
    let view = tv.read(cx);
    if let Some(custom) = view.custom_title().filter(|title| !title.trim().is_empty()) {
        return custom.to_string().into();
    }

    let terminal = view.terminal().read(cx);
    if let Some(name) = terminal
        .working_directory()
        .and_then(|cwd| {
            cwd.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
    {
        return name.into();
    }

    let title = terminal.title(true);
    title
        .split_once(" — ")
        .or_else(|| title.split_once(" - "))
        .map(|(left, _)| left)
        .unwrap_or(&title)
        .to_string()
        .into()
}
