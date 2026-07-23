use std::path::PathBuf;

use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, TaskExt, WeakEntity, Window, div, px,
};
use ui::{IconButton, IconName, Label, LabelSize, Tooltip, prelude::*};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::OpenOptions;
use zed_actions::file_list_panel::ToggleFocus;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<FileListPanel>(window, cx);
        });
    })
    .detach();
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
        Self {
            workspace: workspace.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            current_dir,
            _workspace_subscription,
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
        let Ok(read_dir) = std::fs::read_dir(&self.current_dir) else {
            return Vec::new();
        };
        let mut entries: Vec<FileEntry> = read_dir
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
            .collect();
        // Directories first, then alphabetical.
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.cmp(&b.name))
        });
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
                                    .toggle_state(true)
                                    .tooltip(Tooltip::text(i18n::t("file_list"))),
                            )
                            .child(
                                IconButton::new("show-agent", IconName::Sparkle)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text("Agent"))
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
            .child(
                v_flex()
                    .id("file-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(entries.into_iter().enumerate().map(|(index, entry)| {
                        let colors = theme.colors().clone();
                        let is_dir = entry.is_dir;
                        let icon = if is_dir {
                            IconName::Folder
                        } else {
                            IconName::File
                        };
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
                                    .child(Label::new(entry.name.clone()).size(LabelSize::Small).truncate()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let entry = FileEntry {
                                    path: entry.path.clone(),
                                    name: entry.name.clone(),
                                    is_dir,
                                };
                                this.entry_clicked(entry, window, cx);
                            }))
                    })),
            )
    }
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

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::File)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some(i18n::t_str("file_list"))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        // Prefer the terminal list as the default left dock panel.
        false
    }

    fn activation_priority(&self) -> u32 {
        2
    }
}
