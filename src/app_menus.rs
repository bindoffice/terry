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
        let message = format!("Ink {version}");
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_root, window, cx| {
                let _ = window.prompt(
                    gpui::PromptLevel::Info,
                    &message,
                    Some("A terminal-focused editor."),
                    &["OK"],
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
            name: "Ink".into(),
            disabled: false,
            items: vec![
                MenuItem::action("About Ink", zed_actions::About),
                MenuItem::separator(),
                MenuItem::action("Settings…", zed_actions::OpenSettings),
                MenuItem::action(
                    "Select Theme…",
                    zed_actions::theme_selector::Toggle::default(),
                ),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("Services", gpui::SystemMenuType::Services),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action("Hide Ink", Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action("Hide Others", HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action("Show All", ShowAll),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                MenuItem::action("Quit Ink", zed_actions::Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("Undo", editor::actions::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", editor::actions::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", editor::actions::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", editor::actions::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", editor::actions::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action(
                    "Select All",
                    editor::actions::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Toggle Left Dock", workspace::ToggleLeftDock),
                MenuItem::action("Toggle Right Dock", workspace::ToggleRightDock),
                MenuItem::action("Toggle Bottom Dock", workspace::ToggleBottomDock),
                MenuItem::action("Toggle All Docks", workspace::ToggleAllDocks),
                MenuItem::separator(),
                MenuItem::action("Terminal Panel", terminal_view::terminal_panel::Toggle),
                MenuItem::separator(),
                MenuItem::action("Command Palette…", zed_actions::command_palette::Toggle),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Toggle Full Screen", ToggleFullScreen),
            ],
        },
    ]
}
