use gpui::{App, Menu, MenuItem, OsAction, actions};

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
        let version = env!("CARGO_PKG_VERSION");
        let message = format!("Terry {version}");
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_root, window, cx| {
                let _ = window.prompt(
                    gpui::PromptLevel::Info,
                    &message,
                    Some(i18n::t_str("about_terry_description")),
                    &[i18n::t_str("ok")],
                    cx,
                );
            });
        }
    });

    cx.observe_new(|workspace: &mut workspace::Workspace, _, _| {
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

pub fn app_menus(_cx: &App) -> Vec<Menu> {
    vec![
        Menu {
            name: "Terry".into(),
            disabled: false,
            items: vec![
                MenuItem::action(i18n::t("about_terry"), zed_actions::About),
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
