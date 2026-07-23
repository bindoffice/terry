mod agent_panel;
mod app_icon;
mod app_menus;
mod app_title_bar;
mod file_list_panel;
mod keymap_settings;
mod llm_provider_settings;
mod settings_window;
mod terminal_list_panel;
use std::sync::Arc;

use assets::Assets;
use client::{Client, ProxySettings, UserStore};
use db::kvp::KeyValueStore;
use file_list_panel::FileListPanel;
use fs::{Fs, RealFs};
use futures::channel::oneshot;
use git::GitHostingProviderRegistry;
use gpui::{
    Action, Anchor, AppContext as _, Application, IntoElement, ParentElement, Styled, TaskExt,
    point, px,
};
use gpui_tokio::Tokio;
use language::LanguageRegistry;
use node_runtime::{NodeBinaryOptions, NodeRuntime};
use project::Project;
use release_channel::AppVersion;
use reqwest_client::ReqwestClient;
use session::{AppSession, Session};
use settings::{
    DEFAULT_KEYMAP_PATH, KeybindSource, KeymapFile, Settings, SettingsStore, VIM_KEYMAP_PATH,
};
use terminal_list_panel::TerminalListPanel;
use theme::{ActiveTheme, LoadThemes};
use ui::{ButtonCommon, Clickable, ContextMenu, PopoverMenu, Tooltip};
use util::ResultExt;
use uuid::Uuid;
use vim_mode_setting::VimModeSetting;
use workspace::{AppState, OpenOptions, Workspace, WorkspaceStore};

fn main() {
    // load_login_shell_environment / project env capture invoke
    // `current_exe --printenv` expecting a quick JSON dump + exit.
    // Without this, each spawn starts another GUI instance → fork bomb.
    if std::env::args().any(|arg| arg == "--printenv") {
        util::shell_env::print_env();
        return;
    }

    zlog::init();
    zlog::init_output_stderr();

    let app_version = AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);

    let app =
        Application::with_platform(gpui_platform::current_platform(false)).with_assets(Assets);

    // Start IPC Server
    let (ipc_port, ipc_token) = session::ipc_server::start_ipc_server().unwrap();
    let ipc_info = serde_json::json!({
        "port": ipc_port,
        "token": ipc_token
    });

    let ipc_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("terry/ipc.json");
    if let Some(parent) = ipc_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&ipc_path, serde_json::to_string(&ipc_info).unwrap()).ok();

    let app_db = db::AppDatabase::new();
    let session_id = Uuid::new_v4().to_string();
    let session = app.background_executor().spawn(Session::new(
        session_id,
        KeyValueStore::from_app_db(&app_db),
    ));
    let background_executor = app.background_executor();

    let git_hosting_provider_registry = Arc::new(GitHostingProviderRegistry::new());
    let fs = Arc::new(RealFs::new(None, background_executor.clone()));

    let (shell_env_loaded_tx, shell_env_loaded_rx) = oneshot::channel();
    background_executor
        .spawn(async {
            #[cfg(unix)]
            util::load_login_shell_environment().await.log_err();
            shell_env_loaded_tx.send(()).ok();
        })
        .detach();

    let (user_keymap_file_rx, user_keymap_watcher) = settings::watch_config_file(
        &background_executor,
        fs.clone(),
        paths::keymap_file().clone(),
    );

    app.run(move |cx| {
        cx.set_global(app_db);
        // Identity + dock icon before any windows open.
        cx.set_app_identity("dev.terry.Terry", "Terry");
        app_icon::apply_dock_icon();

        menu::init();
        zed_actions::init();
        app_menus::init(cx);

        release_channel::init(app_version, cx);
        gpui_tokio::init(cx);
        settings::init(cx);
        i18n::init(cx);
        settings_window::init(cx);
        keymap_settings::init(cx);
        llm_provider_settings::init(cx);

        let user_agent = format!(
            "Terry/{} ({}; {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let proxy_url = ProxySettings::get_global(cx).proxy_url();
        let http = {
            let _guard = Tokio::handle(cx).enter();
            ReqwestClient::proxy_and_user_agent(proxy_url, &user_agent)
                .expect("could not start HTTP client")
        };
        cx.set_http_client(Arc::new(http));

        <dyn Fs>::set_global(fs.clone(), cx);

        GitHostingProviderRegistry::set_global(git_hosting_provider_registry, cx);
        git_hosting_providers::init(cx);

        let client = Client::production(cx);
        cx.set_http_client(client.http_client());

        let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));

        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
        let workspace_store = cx.new(|cx| WorkspaceStore::new(client.clone(), cx));

        Client::set_global(client.clone(), cx);

        let (mut node_options_tx, node_options_rx) = watch::channel(None);
        node_options_tx
            .send(Some(NodeBinaryOptions {
                allow_path_lookup: true,
                allow_binary_download: false,
                use_paths: None,
            }))
            .log_err();
        let node_runtime = NodeRuntime::new(
            client.http_client(),
            Some(shell_env_loaded_rx),
            node_options_rx,
        );

        Project::init(&client, cx);
        client::init(&client, cx);
        feature_flags::FeatureFlagStore::init(cx);

        let session = cx.foreground_executor().block_on(session);
        let app_session = cx.new(|cx| AppSession::new(session, cx));

        let app_state = Arc::new(AppState {
            languages,
            client: client.clone(),
            user_store,
            fs: fs.clone(),
            build_window_options,
            workspace_store,
            node_runtime,
            session: app_session,
        });
        AppState::set_global(app_state.clone(), cx);

        theme_settings::init(LoadThemes::All(Box::new(Assets)), cx);
        Assets.load_fonts(cx).log_err();

        editor::init(cx);
        workspace::init(app_state.clone(), cx);
        language_model::init(cx);
        client::llm_token::RefreshLlmTokenListener::register(
            client.clone(),
            app_state.user_store.clone(),
            cx,
        );
        language_models::init(app_state.user_store.clone(), client.clone(), cx);
        agent_settings::AgentSettings::register(cx);
        terminal_list_panel::init(cx);
        agent_panel::init(cx);
        file_list_panel::init(cx);
        app_title_bar::init(cx);
        command_palette_hooks::init(cx);
        search::init(cx);
        cx.set_global(workspace::PaneSearchBarCallbacks {
            setup_search_bar: |languages, toolbar, window, cx| {
                let search_bar = cx.new(|cx| search::BufferSearchBar::new(languages, window, cx));
                toolbar.update(cx, |toolbar, cx| {
                    toolbar.add_item(search_bar, window, cx);
                });
            },
            wrap_div_with_search_actions: search::buffer_search::register_pane_search_actions,
        });
        vim::init(cx);
        terminal_view::init(cx);
        command_palette::init(cx);
        go_to_line::init(cx);
        outline::init(cx);
        tab_switcher::init(cx);
        language_selector::init(cx);
        theme_selector::init(cx);
        encoding_selector::init(cx);
        line_ending_selector::init(cx);
        markdown_preview::init(cx);
        image_viewer::init(cx);

        // Enable vim mode by default for this app.
        SettingsStore::update(cx, |store, cx| {
            store.update_default_settings(cx, |content| {
                content.vim_mode = Some(true);
            });
        });

        handle_keymap_file_changes(user_keymap_file_rx, user_keymap_watcher, cx);

        cx.set_menus(app_menus::app_menus(cx));
        cx.activate(true);

        workspace::open_new(OpenOptions::default(), app_state.clone(), cx, {
            move |workspace, window, cx| {
                init_workspace(workspace, window, cx);
            }
        })
        .detach_and_log_err(cx);
    });
}

fn init_workspace(
    workspace: &mut Workspace,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let workspace_handle = cx.entity();
    let display_pane = workspace.active_pane().clone();
    let project = workspace.project().clone();
    let panel = cx.new(|cx| {
        TerminalListPanel::new(
            workspace_handle.clone(),
            display_pane.clone(),
            project,
            window,
            cx,
        )
    });
    workspace.add_panel(panel.clone(), window, cx);

    // Tab bar: quick "+" creates a terminal in the active group; the following
    // button restores the original pane "New…" popover menu unchanged.
    display_pane.update(cx, |pane, cx| {
        pane.set_render_tab_bar_buttons(cx, |pane, _window, _cx| {
            let new_item_menu_handle = pane.new_item_context_menu_handle.clone();
            let right_children = ui::h_flex()
                .gap_1()
                .child(
                    ui::IconButton::new("new-terminal-tab", ui::IconName::Plus)
                        .icon_size(ui::IconSize::Small)
                        .tooltip(Tooltip::text("New Terminal"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(terminal_list_panel::NewTerminal), cx);
                        }),
                )
                .child(
                    PopoverMenu::new("pane-tab-bar-popover-menu")
                        .trigger_with_tooltip(
                            ui::IconButton::new("new-menu", ui::IconName::ChevronDown)
                                .icon_size(ui::IconSize::Small),
                            Tooltip::text("New…"),
                        )
                        .anchor(Anchor::TopRight)
                        .with_handle(new_item_menu_handle)
                        .menu(move |window, cx| {
                            Some(ContextMenu::build(window, cx, |menu, _, _| {
                                menu.action("New File", workspace::NewFile.boxed_clone())
                                    .action(
                                        "Open File",
                                        workspace::ToggleFileFinder::default().boxed_clone(),
                                    )
                                    .separator()
                                    .action(
                                        "Search Project",
                                        workspace::DeploySearch::default().boxed_clone(),
                                    )
                                    .action(
                                        "Search Symbols",
                                        workspace::ToggleProjectSymbols.boxed_clone(),
                                    )
                                    .separator()
                                    .action(
                                        "New Terminal",
                                        workspace::NewTerminal::default().boxed_clone(),
                                    )
                                    .action(
                                        "New Center Terminal",
                                        workspace::NewCenterTerminal::default().boxed_clone(),
                                    )
                            }))
                        }),
                )
                .into_any_element()
                .into();
            (None, right_children)
        });
    });

    let file_panel = cx.new(|cx| FileListPanel::new(workspace_handle.clone(), cx));
    workspace.add_panel(file_panel, window, cx);
    let agent_fs = workspace.app_state().fs.clone();
    let agent_project = workspace.project().clone();
    let agent_panel = cx.new(|cx| {
        agent_panel::AgentPanel::new(
            workspace_handle.clone(),
            agent_project,
            agent_fs,
            window,
            cx,
        )
    });
    workspace.add_panel(agent_panel, window, cx);
    let active_file_name = cx.new(|_| workspace::active_file_name::ActiveFileName::new());
    let active_buffer_encoding =
        cx.new(|_| encoding_selector::ActiveBufferEncoding::new(workspace));
    let active_buffer_language =
        cx.new(|_| language_selector::ActiveBufferLanguage::new(workspace));
    let line_ending_indicator = cx.new(|_| line_ending_selector::LineEndingIndicator::default());
    let vim_mode_indicator = cx.new(|cx| vim::ModeIndicator::new(window, cx));
    let cursor_position = cx.new(|_| go_to_line::cursor_position::CursorPosition::new(workspace));

    workspace.status_bar().update(cx, |status_bar, cx| {
        let agent_button = cx
            .new(|cx| agent_panel::AgentPanelButton::new(workspace_handle.clone().downgrade(), cx));
        status_bar.add_right_item(agent_button, window, cx);
        status_bar.add_left_item(active_file_name, window, cx);
        status_bar.add_right_item(active_buffer_encoding, window, cx);
        status_bar.add_right_item(active_buffer_language, window, cx);
        status_bar.add_right_item(line_ending_indicator, window, cx);
        status_bar.add_right_item(vim_mode_indicator, window, cx);
        status_bar.add_right_item(cursor_position, window, cx);
    });

    // Seed the terminal list panel with a default group holding one terminal.
    panel.update(cx, |panel, cx| {
        panel.create_default_group(window, cx);
    });
}

fn build_window_options(_display_uuid: Option<Uuid>, cx: &mut gpui::App) -> gpui::WindowOptions {
    gpui::WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Terry".into()),
            appears_transparent: true,
            // Keep traffic lights clear of the custom title text.
            traffic_light_position: Some(point(px(12.), px(11.))),
        }),
        focus: true,
        show: true,
        window_background: cx.theme().window_background_appearance(),
        app_id: Some("dev.terry.Terry".into()),
        // Used on X11; harmless elsewhere.
        icon: app_icon::app_icon_image(),
        ..Default::default()
    }
}

fn handle_keymap_file_changes(
    mut user_keymap_file_rx: futures::channel::mpsc::UnboundedReceiver<String>,
    user_keymap_watcher: gpui::Task<()>,
    cx: &mut gpui::App,
) {
    load_default_keymap(cx);

    cx.spawn(async move |cx| {
        let _user_keymap_watcher = user_keymap_watcher;
        use futures::StreamExt;
        while let Some(user_keymap_content) = user_keymap_file_rx.next().await {
            cx.update(|cx| {
                let load_result = KeymapFile::load(&user_keymap_content, cx);
                if let settings::KeymapFileLoadResult::Success { key_bindings } = load_result {
                    reload_keymaps(cx, key_bindings);
                }
            });
        }
    })
    .detach();
}

fn reload_keymaps(cx: &mut gpui::App, mut user_key_bindings: Vec<gpui::KeyBinding>) {
    cx.clear_key_bindings();
    load_default_keymap(cx);
    for key_binding in &mut user_key_bindings {
        key_binding.set_meta(KeybindSource::User.meta());
    }
    cx.bind_keys(user_key_bindings);
    cx.set_menus(app_menus::app_menus(cx));
}

fn load_default_keymap(cx: &mut gpui::App) {
    let mut key_bindings = KeymapFile::load_asset_allow_partial_failure(DEFAULT_KEYMAP_PATH, cx)
        .expect("failed to load default keymap");
    for key_binding in &mut key_bindings {
        key_binding.set_meta(KeybindSource::Default.meta());
    }
    cx.bind_keys(key_bindings);

    if VimModeSetting::get_global(cx).0 {
        // This app vendors a subset of Zed's crates, so `vim.json` may reference
        // actions that don't exist here (debugger, outline_panel, settings_editor,
        // agent, skill_creator, ...). Use the partial-failure-tolerant loader so
        // those bindings are skipped instead of aborting startup, then tag the
        // surviving bindings with the Vim source (mirroring `load_asset`).
        let mut vim_key_bindings =
            KeymapFile::load_asset_allow_partial_failure(VIM_KEYMAP_PATH, cx)
                .expect("failed to load vim keymap");
        for key_binding in &mut vim_key_bindings {
            key_binding.set_meta(KeybindSource::Vim.meta());
        }
        cx.bind_keys(vim_key_bindings);
    }
}
