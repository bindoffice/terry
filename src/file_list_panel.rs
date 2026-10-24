use std::path::{Path, PathBuf};

use fs::{Fs, RemoveOptions};
use gpui::{
    Action, AnyElement, App, ClipboardItem, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, IntoElement, PromptLevel, Render, SharedString, Subscription, TaskExt, WeakEntity,
    Window, div, px,
};
use ui::{
    ContextMenu, IconButton, IconName, Label, LabelSize, Tooltip, prelude::*, right_click_menu,
};
use workspace::Workspace;
use workspace::OpenOptions;
use workspace::dock::{DockPosition, Panel, PanelEvent, PanelSizeState, PanelStatusButton};
use zed_actions::file_list_panel::{ShowFinder, ShowList, ToggleFocus};

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<FileListPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &ShowList, window, cx| {
            show_file_panel(workspace, FileViewMode::List, window, cx);
        });
        workspace.register_action(|workspace, _: &ShowFinder, window, cx| {
            show_file_panel(workspace, FileViewMode::Finder, window, cx);
        });
    })
    .detach();
}

fn show_file_panel(
    workspace: &mut Workspace,
    mode: FileViewMode,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    workspace.open_panel::<FileListPanel>(window, cx);
    let current = workspace.panel_size_state::<FileListPanel>(cx);
    let restore = workspace.panel::<FileListPanel>(cx).and_then(|panel| {
        panel.update(cx, |panel, cx| panel.switch_view_mode(mode, current, cx))
    });
    if let Some(size_state) = restore {
        workspace.set_panel_size_state::<FileListPanel>(size_state, window, cx);
        cx.notify();
    }
    workspace.focus_panel::<FileListPanel>(window, cx);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileViewMode {
    List,
    Finder,
}

#[derive(Clone)]
struct FileEntry {
    path: PathBuf,
    name: SharedString,
    is_dir: bool,
}

impl FileEntry {
    fn is_parent_row(&self) -> bool {
        self.name.as_ref() == ".."
    }

    fn terminal_cwd(&self) -> PathBuf {
        if self.is_dir {
            self.path.clone()
        } else {
            self.path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.path.clone())
        }
    }

    fn create_target_dir(&self) -> PathBuf {
        if self.is_dir && !self.is_parent_row() {
            self.path.clone()
        } else {
            self.path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.path.clone())
        }
    }
}

pub struct FileListPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    position: DockPosition,
    current_dir: PathBuf,
    /// Cached directory contents. Rebuilt only when `current_dir` changes or
    /// the user explicitly refreshes; reading and stat-ing a directory on every
    /// frame made large folders needlessly expensive.
    entries: Vec<FileEntry>,
    /// Directory `entries` was read from. `None` forces the next render to
    /// reload (refresh button, create/delete, or a directory change).
    entries_dir: Option<PathBuf>,
    view_mode: FileViewMode,
    /// Width last used in list mode. Restored when switching back.
    list_size: Option<PanelSizeState>,
    /// Width last used in Finder mode.
    finder_size: Option<PanelSizeState>,
    /// Mode whose width is currently applied to the dock.
    applied_mode: Option<FileViewMode>,
    /// Terminal last observed, so a title refresh can be told apart from a
    /// real directory or selection change.
    followed_item: Option<EntityId>,
    followed_cwd: Option<PathBuf>,
    /// Live cwd changes for the terminal currently being followed.
    _terminal_subscription: Option<Subscription>,
    _workspace_subscription: Subscription,
}

impl FileListPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _workspace_subscription = cx.subscribe(&workspace, |this, _, event, cx| {
            if let workspace::Event::ActiveItemChanged { activated } = event {
                this.update_from_active_item(*activated, cx);
            }
            cx.notify();
        });
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let panel = Self {
            workspace: workspace.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            current_dir,
            entries: Vec::new(),
            entries_dir: None,
            view_mode: FileViewMode::List,
            list_size: None,
            finder_size: None,
            applied_mode: None,
            followed_item: None,
            followed_cwd: None,
            _terminal_subscription: None,
            _workspace_subscription,
        };
        panel.schedule_size_load(cx);
        panel
    }

    fn schedule_size_load(&self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    this.load_remembered_sizes(cx);
                    this.initialize_current_dir(cx);
                });
            }
        });
    }

    /// Prefer the project root over the process cwd (which is often `/` for a
    /// packaged app). Skipped once the panel has started following a terminal.
    fn initialize_current_dir(&mut self, cx: &mut Context<Self>) {
        if self.entries_dir.is_some() || self.followed_item.is_some() {
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let project = workspace.read(cx).project().clone();
        let root = project
            .read(cx)
            .visible_worktrees(cx)
            .find_map(|worktree| {
                let path = worktree.read(cx).abs_path().to_path_buf();
                path.is_dir().then_some(path)
            });
        if let Some(root) = root {
            self.current_dir = root;
            cx.notify();
        }
    }

    /// Keeps the current dock width for the mode being left, then returns the
    /// width to apply for `mode`. `None` means the dock width should stay.
    fn switch_view_mode(
        &mut self,
        mode: FileViewMode,
        current: Option<PanelSizeState>,
        cx: &mut Context<Self>,
    ) -> Option<PanelSizeState> {
        if mode == self.view_mode {
            self.applied_mode = Some(mode);
            cx.emit(PanelEvent::StatusButtonsChanged);
            cx.notify();
            return None;
        }
        if let Some(current) = current {
            self.remember_size(self.view_mode, current, cx);
        }
        self.view_mode = mode;
        self.applied_mode = Some(mode);
        cx.emit(PanelEvent::StatusButtonsChanged);
        cx.notify();
        if mode == FileViewMode::Finder && self.finder_size.is_none() {
            let size = PanelSizeState {
                size: Some(px(360.)),
                flex: None,
            };
            self.remember_size(FileViewMode::Finder, size, cx);
            return Some(size);
        }
        self.stored_size(mode)
    }

    fn stored_size(&self, mode: FileViewMode) -> Option<PanelSizeState> {
        match mode {
            FileViewMode::List => self.list_size,
            FileViewMode::Finder => self.finder_size,
        }
    }

    fn remember_size(&mut self, mode: FileViewMode, size: PanelSizeState, cx: &mut Context<Self>) {
        match mode {
            FileViewMode::List => self.list_size = Some(size),
            FileViewMode::Finder => self.finder_size = Some(size),
        }
        let workspace = self.workspace.clone();
        let key = match mode {
            FileViewMode::List => "file_list_panel:list",
            FileViewMode::Finder => "file_list_panel:finder",
        };
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    workspace.persist_panel_size_state(key, size, cx);
                });
            }
        });
    }

    fn load_remembered_sizes(&mut self, cx: &App) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let workspace = workspace.read(cx);
        if self.list_size.is_none() {
            self.list_size = workspace.persisted_panel_size_state("file_list_panel:list", cx);
        }
        if self.finder_size.is_none() {
            self.finder_size = workspace.persisted_panel_size_state("file_list_panel:finder", cx);
        }
    }

    fn update_from_active_item(&mut self, activated: bool, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(active_item) = workspace.read(cx).active_item(cx) else {
            self.stop_following_terminal();
            return;
        };
        let Some(terminal_view) = active_item.downcast::<terminal_view::TerminalView>() else {
            self.stop_following_terminal();
            return;
        };
        let item_id = active_item.item_id();

        // Follow the active terminal's cwd while it stays active. TitleChanged
        // is emitted after the cached process cwd changes; CwdChanged covers
        // remote terminals reporting OSC 7.
        if self.followed_item != Some(item_id) {
            let terminal = terminal_view.read(cx).terminal().clone();
            self._terminal_subscription = Some(cx.subscribe(
                &terminal,
                move |this, _terminal, event, cx| {
                    if matches!(
                        event,
                        terminal::Event::CwdChanged | terminal::Event::TitleChanged
                    ) {
                        this.update_from_active_item(false, cx);
                    }
                },
            ));
        }

        let cwd = terminal_view
            .read(cx)
            .terminal()
            .read(cx)
            .working_directory();
        let unseen = self.followed_item.is_none();
        let unchanged = self.followed_item == Some(item_id) && self.followed_cwd == cwd;
        self.followed_item = Some(item_id);
        self.followed_cwd = cwd.clone();
        // Sidebar drags resize the terminal and refresh its title without a
        // new selection. Keep the folder Finder is already showing.
        if self.view_mode == FileViewMode::Finder && !activated && (unchanged || unseen) {
            return;
        }
        if let Some(cwd) = cwd {
            if self.current_dir != cwd {
                self.current_dir = cwd;
                cx.notify();
            }
        }
    }

    fn stop_following_terminal(&mut self) {
        self._terminal_subscription = None;
        self.followed_item = None;
        self.followed_cwd = None;
    }

    fn collect_entries(dir: &Path) -> Vec<FileEntry> {
        let mut entries = match std::fs::read_dir(dir) {
            Ok(read_dir) => read_dir
                .filter_map(|entry| entry.ok())
                .map(|entry| {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    FileEntry {
                        path,
                        name: SharedString::from(name),
                        is_dir,
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        // Directories first, then alphabetical.
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        // Keep ".." above every folder so it is not sorted in with the names.
        if let Some(parent) = dir.parent() {
            entries.insert(
                0,
                FileEntry {
                    path: parent.to_path_buf(),
                    name: "..".into(),
                    is_dir: true,
                },
            );
        }
        entries
    }

    /// Return the cached entries, reloading only when the directory changed or
    /// the cache was explicitly invalidated (refresh / create / delete).
    fn entries_for_render(&mut self) -> Vec<FileEntry> {
        if self.entries_dir.as_deref() != Some(self.current_dir.as_path()) {
            let dir = self.current_dir.clone();
            self.entries = Self::collect_entries(&dir);
            self.entries_dir = Some(dir);
        }
        self.entries.clone()
    }

    fn invalidate_entries(&mut self) {
        self.entries_dir = None;
    }

    fn open_terminal_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_terminal_at(self.current_dir.clone(), window, cx);
    }

    fn navigate_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.current_dir.parent().map(|p| p.to_path_buf()) {
            self.current_dir = parent;
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.invalidate_entries();
        cx.notify();
    }

    fn entry_clicked(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.is_dir {
            if window.modifiers().secondary() {
                if let Some(workspace) = self.workspace.upgrade() {
                    if let Some(active_item) = workspace.read(cx).active_item(cx) {
                        if let Some(terminal_view) =
                            active_item.downcast::<terminal_view::TerminalView>()
                        {
                            let command = format!("cd {}\n", Self::shell_quote_path(&entry.path));
                            let terminal = terminal_view.read(cx).terminal().clone();
                            terminal.update(cx, |t, _cx| {
                                t.input(command.into_bytes());
                            });
                        }
                    }
                }
            }
            self.current_dir = entry.path;
            cx.notify();
        } else {
            self.open_file(entry.path, window, cx);
        }
    }

    /// Quote a path for typing into an interactive shell. `cd {:?}` is not
    /// safe: debug formatting does not escape `$(...)`, backticks or `!`, so a
    /// crafted directory name could execute commands.
    fn shell_quote_path(path: &Path) -> String {
        let value = path.to_string_lossy();
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace
                .open_abs_path(path, OpenOptions::default(), window, cx)
                .detach_and_log_err(cx);
        });
    }

    fn open_with_system(&self, path: &Path, cx: &App) {
        cx.open_with_system(path);
    }

    fn reveal_in_file_manager(&self, path: &Path, cx: &App) {
        cx.reveal_path(path);
    }

    fn copy_path(&self, path: &Path, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
    }

    fn copy_name(&self, name: &str, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(name.to_string()));
    }

    fn open_terminal_at(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(panel) = workspace
            .read(cx)
            .panel::<crate::terminal_list_panel::TerminalListPanel>(cx)
        else {
            return;
        };
        panel.update(cx, |panel, cx| {
            panel.open_shell_at(cwd, window, cx);
        });
    }

    fn unique_child_path(parent: &Path, base: &str) -> PathBuf {
        let candidate = parent.join(base);
        if !candidate.exists() {
            return candidate;
        }
        for index in 2..10_000 {
            let candidate = parent.join(format!("{base} {index}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        parent.join(format!("{base} {}", uuid::Uuid::new_v4()))
    }

    fn create_new_file_in(
        &mut self,
        parent: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = Self::unique_child_path(&parent, "untitled");
        if std::fs::File::create(&path).is_ok() {
            self.invalidate_entries();
            self.open_file(path, window, cx);
            cx.notify();
        }
    }

    fn create_new_folder_in(&mut self, parent: PathBuf, cx: &mut Context<Self>) {
        let path = Self::unique_child_path(&parent, "untitled folder");
        if std::fs::create_dir(&path).is_ok() {
            self.invalidate_entries();
            cx.notify();
        }
    }

    fn delete_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let message = format!("{} \"{}\"?", i18n::t("delete_confirm"), name);
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            None,
            &[i18n::t_str("delete"), i18n::t_str("cancel")],
            cx,
        );
        // Prefer the platform trash over permanent deletion.
        let fs = <dyn Fs>::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let delete_result = fs.trash(&path, RemoveOptions::default()).await;
            match delete_result {
                Ok(_) => {
                    this.update(cx, |this, cx| {
                        this.invalidate_entries();
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => {
                    log::error!("failed to move {} to trash: {error}", path.display());
                }
            }
        })
        .detach();
    }

    fn build_finder_context_menu(
        panel: Entity<Self>,
        entry: FileEntry,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        let is_parent = entry.is_parent_row();
        let reveal_label = ui::utils::reveal_in_file_manager_label(false);
        ContextMenu::build(window, cx, move |menu, _, _| {
            let open_panel = panel.clone();
            let open_entry = entry.clone();
            let mut menu = menu.entry(i18n::t("open_item"), None, move |window, cx| {
                open_panel.update(cx, |this, cx| {
                    this.entry_clicked(open_entry.clone(), window, cx);
                });
            });

            if !is_parent {
                let system_panel = panel.clone();
                let system_path = entry.path.clone();
                menu = menu.entry(i18n::t("open_in_default_app"), None, move |_window, cx| {
                    system_panel.update(cx, |this, cx| {
                        this.open_with_system(&system_path, cx);
                    });
                });
            }

            let reveal_panel = panel.clone();
            let reveal_path = entry.path.clone();
            menu = menu.entry(reveal_label, None, move |_window, cx| {
                reveal_panel.update(cx, |this, cx| {
                    this.reveal_in_file_manager(&reveal_path, cx);
                });
            });

            let terminal_panel = panel.clone();
            let terminal_cwd = entry.terminal_cwd();
            menu = menu
                .separator()
                .entry(i18n::t("new_terminal_here"), None, move |window, cx| {
                    terminal_panel.update(cx, |this, cx| {
                        this.open_terminal_at(terminal_cwd.clone(), window, cx);
                    });
                });

            let copy_path_panel = panel.clone();
            let copy_path = entry.path.clone();
            let copy_name_panel = panel.clone();
            let copy_name = entry.name.to_string();
            menu = menu
                .separator()
                .entry(i18n::t("copy_path"), None, move |_window, cx| {
                    copy_path_panel.update(cx, |this, cx| {
                        this.copy_path(&copy_path, cx);
                    });
                })
                .entry(i18n::t("copy_name"), None, move |_window, cx| {
                    copy_name_panel.update(cx, |this, cx| {
                        this.copy_name(&copy_name, cx);
                    });
                });

            if !is_parent {
                let create_dir = entry.create_target_dir();
                let new_file_panel = panel.clone();
                let new_file_dir = create_dir.clone();
                let new_folder_panel = panel.clone();
                let new_folder_dir = create_dir;
                menu = menu
                    .separator()
                    .entry(i18n::t("new_file"), None, move |window, cx| {
                        new_file_panel.update(cx, |this, cx| {
                            this.create_new_file_in(new_file_dir.clone(), window, cx);
                        });
                    })
                    .entry(i18n::t("new_folder"), None, move |_window, cx| {
                        new_folder_panel.update(cx, |this, cx| {
                            this.create_new_folder_in(new_folder_dir.clone(), cx);
                        });
                    });

                let delete_panel = panel.clone();
                let delete_path = entry.path.clone();
                menu = menu.separator().entry(i18n::t("delete"), None, move |window, cx| {
                    delete_panel.update(cx, |this, cx| {
                        this.delete_entry(delete_path.clone(), window, cx);
                    });
                });
            }

            menu
        })
    }

    fn dir_label(&self) -> SharedString {
        SharedString::from(self.current_dir.display().to_string())
    }
}

impl Focusable for FileListPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for FileListPanel {}

impl Render for FileListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries_for_render();
        let theme = cx.theme().clone();
        let dir_label = self.dir_label();

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
                    .child(Label::new(i18n::t("files")).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                IconButton::new("show-terminal-list", IconName::Terminal)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("terminal_list")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(zed_actions::terminal_list_panel::ToggleFocus),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                IconButton::new("show-file-list", IconName::File)
                                    .icon_size(IconSize::Small)
                                    .toggle_state(self.view_mode == FileViewMode::List)
                                    .tooltip(Tooltip::text(i18n::t("file_list")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ShowList), cx);
                                    }),
                            )
                            .child(
                                IconButton::new("show-finder", IconName::FolderOpen)
                                    .icon_size(IconSize::Small)
                                    .toggle_state(self.view_mode == FileViewMode::Finder)
                                    .tooltip(Tooltip::text(i18n::t("finder")))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ShowFinder), cx);
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
                                IconButton::new("navigate-up", IconName::ArrowUp)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("up_one_level")))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.navigate_up(cx);
                                    })),
                            )
                            .when(self.view_mode == FileViewMode::Finder, |this| {
                                this.child(
                                    IconButton::new("finder-new-terminal", IconName::Plus)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text(i18n::t("new_terminal")))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_terminal_here(window, cx);
                                        })),
                                )
                            })
                            .child(
                                IconButton::new("refresh-files", IconName::ArrowCircle)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(i18n::t("refresh")))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .child(
                        Label::new(dir_label)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    ),
            )
            .child(match self.view_mode {
                FileViewMode::List => render_file_rows(entries, theme, cx),
                FileViewMode::Finder => render_finder_grid(entries, theme, cx),
            })
    }
}

fn entry_icon(entry: &FileEntry) -> IconName {
    if entry.name.as_ref() == ".." {
        IconName::ArrowUp
    } else if entry.is_dir {
        IconName::Folder
    } else {
        IconName::File
    }
}

fn entry_clicked(
    this: &mut FileListPanel,
    entry: &FileEntry,
    window: &mut Window,
    cx: &mut Context<FileListPanel>,
) {
    this.entry_clicked(
        FileEntry {
            path: entry.path.clone(),
            name: entry.name.clone(),
            is_dir: entry.is_dir,
        },
        window,
        cx,
    );
}

fn render_file_rows(
    entries: Vec<FileEntry>,
    theme: std::sync::Arc<theme::Theme>,
    cx: &mut Context<FileListPanel>,
) -> AnyElement {
    v_flex()
        .id("file-list")
        .flex_1()
        .overflow_y_scroll()
        .children(entries.into_iter().enumerate().map(|(index, entry)| {
            let colors = theme.colors().clone();
            let icon = entry_icon(&entry);
            div()
                .id(index)
                .px_2()
                .py_1()
                .mx_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            ui::Icon::new(icon)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(entry.name.clone())
                                .size(LabelSize::Small)
                                .truncate(),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    entry_clicked(this, &entry, window, cx);
                }))
        }))
        .into_any_element()
}

fn render_finder_grid(
    entries: Vec<FileEntry>,
    theme: std::sync::Arc<theme::Theme>,
    cx: &mut Context<FileListPanel>,
) -> AnyElement {
    let panel = cx.entity();
    h_flex()
        .id("finder")
        .flex_1()
        .flex_wrap()
        .content_start()
        .items_start()
        .p_2()
        .gap_1()
        .overflow_y_scroll()
        .children(entries.into_iter().enumerate().map(|(index, entry)| {
            let colors = theme.colors().clone();
            let icon = entry_icon(&entry);
            let icon_color = if entry.is_dir && entry.name.as_ref() != ".." {
                Color::Accent
            } else {
                Color::Muted
            };
            let panel = panel.clone();
            let menu_entry = entry.clone();
            let label = entry.name.clone();
            let on_click = cx.listener({
                let entry = entry.clone();
                move |this, _, window, cx| {
                    entry_clicked(this, &entry, window, cx);
                }
            });
            right_click_menu(format!("finder-rc-{index}"))
                .trigger(move |_is_open, _window, _cx| {
                    v_flex()
                        .id(index)
                        .w(px(108.))
                        .items_center()
                        .gap_1()
                        .px_1()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .child(ui::Icon::new(icon).size(IconSize::XLarge).color(icon_color))
                        .child(
                            div().w(px(96.)).text_center().child(
                                Label::new(label).size(LabelSize::XSmall).truncate(),
                            ),
                        )
                        .on_click(on_click)
                })
                .menu({
                    let panel = panel.clone();
                    move |window, cx| {
                        FileListPanel::build_finder_context_menu(
                            panel.clone(),
                            menu_entry.clone(),
                            window,
                            cx,
                        )
                    }
                })
                .into_any_element()
        }))
        .into_any_element()
}

impl Panel for FileListPanel {
    fn persistent_name() -> &'static str {
        "FileListPanel"
    }

    fn panel_key() -> &'static str {
        "file_list_panel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(&mut self, position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
        self.position = position;
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        px(240.)
    }

    fn size_state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.view_mode;
        cx.defer_in(window, move |this, _window, cx| {
            let Some(workspace) = this.workspace.upgrade() else {
                return;
            };
            let Some(size_state) = workspace.read(cx).panel_size_state::<FileListPanel>(cx) else {
                return;
            };
            this.remember_size(mode, size_state, cx);
        });
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !active {
            return;
        }
        cx.defer_in(window, |this, window, cx| {
            this.load_remembered_sizes(cx);
            if this.applied_mode == Some(this.view_mode) {
                return;
            }
            let mode = this.view_mode;
            let Some(size_state) = this.stored_size(mode) else {
                this.applied_mode = Some(mode);
                return;
            };
            this.applied_mode = Some(mode);
            let Some(workspace) = this.workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |workspace, cx| {
                workspace.set_panel_size_state::<FileListPanel>(size_state, window, cx);
                cx.notify();
            });
        });
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::File)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some(i18n::t_str("file_list"))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ShowList)
    }

    fn primary_status_button_selected(&self, _window: &Window, _cx: &App) -> bool {
        self.view_mode == FileViewMode::List
    }

    fn extra_status_buttons(&self, _window: &Window, _cx: &App) -> Vec<PanelStatusButton> {
        vec![PanelStatusButton {
            id: "show-finder",
            icon: IconName::FolderOpen,
            tooltip: i18n::t_str("finder"),
            action: Box::new(ShowFinder),
            selected: self.view_mode == FileViewMode::Finder,
            trailing: false,
        }]
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        // Prefer the terminal list as the default left dock panel.
        false
    }

    fn activation_priority(&self) -> u32 {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn shell_quote_path_handles_spaces_and_quotes() {
        assert_eq!(
            FileListPanel::shell_quote_path(Path::new("/tmp/a b")),
            "'/tmp/a b'"
        );
        assert_eq!(
            FileListPanel::shell_quote_path(Path::new("/tmp/it's here")),
            "'/tmp/it'\\''s here'"
        );
        // Command substitutions must stay literal inside the single quotes.
        assert_eq!(
            FileListPanel::shell_quote_path(Path::new("/tmp/$(touch pwned)")),
            "'/tmp/$(touch pwned)'"
        );
    }
}
