use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, Menu, MenuItem, OsAction, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, img, px,
};
use settings::Settings;
use terminal::terminal_settings::TerminalSettings;
use theme::ActiveTheme;
use ui::{Color, Headline, Label, LabelSize, prelude::*};
use util::ResultExt;

actions!(
    app_menus,
    [
        /// Hides the application (macOS).
        Hide,
        /// Hides other applications (macOS).
        HideOthers,
        /// Shows all applications (macOS).
        ShowAll,
        /// Minimizes the active window.
        Minimize,
        /// Zooms the active window.
        Zoom,
        /// Toggles fullscreen for the active window.
        ToggleFullScreen,
    ]
);

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    }

    cx.on_action(|_: &zed_actions::Quit, cx| {
        cx.quit();
    });

    cx.on_action(|_: &zed_actions::About, cx| {
        open_about_window(cx);
    });

    cx.observe_new(|workspace: &mut workspace::Workspace, _, _| {
        workspace.register_action(|_, _: &zed_actions::About, _window, cx| {
            open_about_window(cx);
        });
        workspace
            .register_action(|_, _: &Minimize, window, _| {
                window.minimize_window();
            })
            .register_action(|_, _: &Zoom, window, _| {
                window.zoom_window();
            })
            .register_action(|_, _: &ToggleFullScreen, window, _| {
                window.toggle_fullscreen();
            });
    })
    .detach();
}

struct AboutWindow {
    focus_handle: FocusHandle,
}

impl AboutWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for AboutWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let version = env!("TERRY_VERSION");
        v_flex()
            .id("terry-about")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(
                img("images/terry_logo.png")
                    .size(rems_from_px(128.))
                    .rounded_xl()
                    .overflow_hidden(),
            )
            .child(Headline::new("Terry"))
            .child(
                Label::new(version)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                div().max_w(px(280.)).text_center().child(
                    Label::new(i18n::t("about_terry_description"))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
    }
}

fn open_about_window(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<AboutWindow>())
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
                    title: Some(i18n::t("about_terry").into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(360.), px(420.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(320.),
                    height: px(360.),
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| AboutWindow::new(cx)),
        )
        .log_err();
    });
}

pub fn app_menus(cx: &App) -> Vec<Menu> {
    vec![
        Menu {
            name: "Terry".into(),
            disabled: false,
            items: vec![
                MenuItem::action(i18n::t("about_terry"), zed_actions::About),
                MenuItem::action(
                    i18n::t("check_for_updates"),
                    update_checker::CheckForUpdates,
                ),
                MenuItem::separator(),
                MenuItem::action(i18n::t("settings"), zed_actions::OpenSettings),
                MenuItem::action(
                    i18n::t("select_theme"),
                    zed_actions::theme_selector::Toggle::default(),
                ),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu(i18n::t("services"), gpui::SystemMenuType::Services),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action(i18n::t("hide_terry"), Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action(i18n::t("hide_others"), HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action(i18n::t("show_all"), ShowAll),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                MenuItem::action(i18n::t("quit_terry"), zed_actions::Quit),
            ],
        },
        Menu {
            name: i18n::t("menu_file").into(),
            disabled: false,
            items: vec![
                MenuItem::action(i18n::t("open"), workspace::Open::default()),
                MenuItem::action(i18n::t("open_recent"), zed_actions::OpenRecent::default()),
                MenuItem::separator(),
                MenuItem::action(
                    i18n::t("add_folder_to_project"),
                    workspace::AddFolderToProject,
                ),
                MenuItem::separator(),
                MenuItem::action(i18n::t("close_window"), workspace::CloseWindow),
            ],
        },
        Menu {
            name: i18n::t("menu_edit").into(),
            disabled: false,
            items: vec![
                MenuItem::os_action(i18n::t("undo"), editor::actions::Undo, OsAction::Undo),
                MenuItem::os_action(i18n::t("redo"), editor::actions::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(i18n::t("cut"), editor::actions::Cut, OsAction::Cut),
                MenuItem::os_action(i18n::t("copy"), editor::actions::Copy, OsAction::Copy),
                MenuItem::os_action(i18n::t("paste"), editor::actions::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action(
                    i18n::t("select_all"),
                    editor::actions::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            name: i18n::t("menu_view").into(),
            disabled: false,
            items: vec![
                MenuItem::action(i18n::t("toggle_left_dock"), workspace::ToggleLeftDock),
                MenuItem::action(i18n::t("toggle_right_dock"), workspace::ToggleRightDock),
                MenuItem::action(i18n::t("toggle_bottom_dock"), workspace::ToggleBottomDock),
                MenuItem::action(i18n::t("toggle_all_docks"), workspace::ToggleAllDocks),
                MenuItem::separator(),
                MenuItem::action(
                    i18n::t("terminal_panel"),
                    terminal_view::terminal_panel::Toggle,
                ),
                MenuItem::separator(),
                MenuItem::action(
                    i18n::t("command_palette"),
                    zed_actions::command_palette::Toggle,
                ),
            ],
        },
        Menu {
            name: i18n::t("menu_terminal").into(),
            disabled: false,
            items: {
                let mut items = vec![
                    MenuItem::action(
                        i18n::t("new_terminal"),
                        zed_actions::terminal_list_panel::NewTerminal,
                    ),
                    MenuItem::action(
                        i18n::t("new_group"),
                        zed_actions::terminal_list_panel::NewGroup,
                    ),
                ];
                let profiles = TerminalSettings::get_global(cx).profiles.clone();
                if !profiles.is_empty() {
                    items.push(MenuItem::submenu(Menu {
                        name: i18n::t("new_terminal_with_profile").into(),
                        disabled: false,
                        items: profiles
                            .into_iter()
                            .map(|profile| {
                                let action =
                                    terminal_view::NewTerminalWithProfile(profile.name.clone());
                                MenuItem::action(profile.name, action)
                            })
                            .collect(),
                    }));
                }
                items.extend([
                    MenuItem::separator(),
                    MenuItem::action(
                        i18n::t("next_group"),
                        zed_actions::terminal_list_panel::NextGroup,
                    ),
                    MenuItem::action(
                        i18n::t("previous_group"),
                        zed_actions::terminal_list_panel::PreviousGroup,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        i18n::t("jump_to_previous_prompt"),
                        terminal_view::JumpToPreviousPrompt,
                    ),
                    MenuItem::action(
                        i18n::t("jump_to_next_prompt"),
                        terminal_view::JumpToNextPrompt,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        i18n::t("copy_last_command_output"),
                        terminal_view::CopyLastCommandOutput,
                    ),
                    MenuItem::action(
                        i18n::t("select_last_command_output"),
                        terminal_view::SelectLastCommandOutput,
                    ),
                    MenuItem::action(
                        i18n::t("rerun_last_command"),
                        terminal_view::RerunLastCommand,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        i18n::t("broadcast_input"),
                        terminal_view::ToggleBroadcastInput,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        i18n::t("toggle_quake_terminal"),
                        crate::quake_terminal::Toggle,
                    ),
                ]);
                items
            },
        },
        Menu {
            name: i18n::t("menu_window").into(),
            disabled: false,
            items: vec![
                MenuItem::action(i18n::t("minimize"), Minimize),
                MenuItem::action(i18n::t("zoom"), Zoom),
                MenuItem::separator(),
                MenuItem::action(i18n::t("toggle_full_screen"), ToggleFullScreen),
            ],
        },
    ]
}
