use std::path::PathBuf;

use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Render, SharedString, Subscription, TaskExt, WeakEntity, Window, div, px,
};
use ui::{IconButton, IconName, Label, LabelSize, Tooltip, prelude::*};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent, PanelSizeState, PanelStatusButton};
use workspace::OpenOptions;
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

struct FileEntry {
    path: PathBuf,
    name: SharedString,
    is_dir: bool,
}

pub struct FileListPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    position: DockPosition,
    current_dir: PathBuf,
    view_mode: FileViewMode,
    /// Width last used in list mode. Restored when switching back.
    list_size: Option<PanelSizeState>,
    /// Width last used in Finder mode.
    finder_size: Option<PanelSizeState>,
    /// Mode whose width is currently applied to the dock.
    applied_mode: Option<FileViewMode>,
    _workspace_subscription: Subscription,
}

impl FileListPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _workspace_subscription = cx.subscribe(&workspace, |this, _, event, cx| {
            if let workspace::Event::ActiveItemChanged = event {
                this.update_from_active_item(cx);
            }
            cx.notify();
        });
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let panel = Self {
            workspace: workspace.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            current_dir,
            view_mode: FileViewMode::List,
            list_size: None,
            finder_size: None,
            applied_mode: None,
            _workspace_subscription,
        };
        panel.schedule_size_load(cx);
        panel
    }

    fn schedule_size_load(&self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.load_remembered_sizes(cx));
            }
        });
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

    fn update_from_active_item(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            if let Some(active_item) = workspace.read(cx).active_item(cx) {
                if let Some(terminal_view) = active_item.downcast::<terminal_view::TerminalView>() {
                    if let Some(cwd) = terminal_view.read(cx).terminal().read(cx).working_directory() {
                        self.current_dir = cwd;
                        cx.notify();
                    }
                }
            }
        }
    }

    fn collect_entries(&self) -> Vec<FileEntry> {
        let mut entries = match std::fs::read_dir(&self.current_dir) {
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
        if let Some(parent) = self.current_dir.parent() {
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

    fn navigate_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.current_dir.parent().map(|p| p.to_path_buf()) {
            self.current_dir = parent;
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn entry_clicked(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.is_dir {
            if window.modifiers().secondary() {
                if let Some(workspace) = self.workspace.upgrade() {
                    if let Some(active_item) = workspace.read(cx).active_item(cx) {
                        if let Some(terminal_view) = active_item.downcast::<terminal_view::TerminalView>() {
                            let path_str = entry.path.to_string_lossy().to_string();
                            let terminal = terminal_view.read(cx).terminal().clone();
                            terminal.update(cx, |t, _cx| {
                                t.input(format!("cd {:?}\n", path_str).into_bytes());
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
        let entries = self.collect_entries();
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
                        Label::new(entry.name.clone())
                            .size(LabelSize::XSmall)
                            .truncate(),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    entry_clicked(this, &entry, window, cx);
                }))
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
