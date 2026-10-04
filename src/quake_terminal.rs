//! Quake-style drop-down terminal: a global hotkey (ctrl+backtick) toggles a
//! top-anchored floating terminal window from anywhere in the system.
//!
//! MVP notes:
//! - The hotkey is registered via gpui's global-hotkey API (macOS only, backed
//!   by Carbon `RegisterEventHotKey`; needs no accessibility permissions).
//! - No slide animation: the window is shown/hidden in one step.
//! - The window hosts a standalone `TerminalView` with an invalid (never
//!   upgradable) `WeakEntity<Workspace>`; `TerminalView` degrades gracefully
//!   when the workspace handle cannot be upgraded (all call sites use
//!   `upgrade() … else return`), and workspace-specific context-menu actions
//!   are hidden via `set_show_workspace_actions(false)`.

use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, Global, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, UpdateGlobal, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, px, size,
};
use project::{LocalProjectFlags, Project};
use terminal_view::TerminalView;
use theme::ActiveTheme;
use util::ResultExt;
use workspace::{AppState, Workspace};

/// Carbon virtual key code `kVK_ANSI_Grave` (HIToolbox/Events.h) — the
/// backtick / grave accent key on ANSI layouts.
const KEY_CODE_ANSI_GRAVE: u32 = 0x32;
/// Carbon modifier mask `controlKey` (HIToolbox/Events.h, 1 << 12).
const MODIFIER_CONTROL_KEY: u32 = 1 << 12;

/// Per-app state for the quake terminal.
#[derive(Default)]
struct QuakeState {
    /// The quake window, if it has been created and not closed.
    window: Option<WindowHandle<QuakeTerminalRoot>>,
    /// Standalone project backing the terminal. Created once, reused across
    /// window recreations (closing the window drops the terminal and its PTY,
    /// but the project itself is cheap to keep).
    project: Option<Entity<Project>>,
    /// Whether the quake window is currently shown. Tracking this ourselves
    /// because gpui has no cross-platform "is visible" API.
    visible: bool,
}

impl Global for QuakeState {}

/// Registers the global hotkey (ctrl+backtick) that toggles the quake window.
pub fn init(cx: &mut App) {
    QuakeState::set_global(cx, QuakeState::default());

    // macOS-only: on other platforms `on_global_hotkey` is a no-op and the
    // callback is never invoked.
    cx.on_global_hotkey(
        gpui::GlobalHotkey {
            key_code: KEY_CODE_ANSI_GRAVE,
            modifiers: MODIFIER_CONTROL_KEY,
        },
        |cx| toggle(cx),
    );
}

/// Show/hide semantics: visible-and-key → hide; otherwise → show + focus.
fn toggle(cx: &mut App) {
    QuakeState::update_global(cx, |state, cx| {
        let Some(handle) = state.window else {
            create_window(cx, state);
            return;
        };

        if state.visible && handle.is_active(cx) == Some(true) {
            state.visible = false;
            if handle.update(cx, |_, window, _| window.hide()).is_err() {
                // Window was closed since the last toggle; recreate next time.
                state.window = None;
            }
            return;
        }

        show_window(cx, state, handle);
    });
}

/// Brings the quake window to the front, makes it key, and focuses the
/// terminal. Recreates the window if it was closed.
fn show_window(cx: &mut App, state: &mut QuakeState, handle: WindowHandle<QuakeTerminalRoot>) {
    state.visible = true;
    let shown = handle
        .update(cx, |root, window, cx| {
            // makeKeyAndOrderFront alone does not activate the app when
            // another app is frontmost, which is exactly when quake is used.
            cx.activate(true);
            window.activate_window();
            if let Some(terminal_view) = root.terminal_view.clone() {
                let focus = terminal_view.focus_handle(cx);
                window.focus(&focus, cx);
            }
        })
        .is_ok();
    if !shown {
        state.window = None;
        create_window(cx, state);
    }
}

/// Creates the quake window (hidden) and spawns its shell asynchronously.
fn create_window(cx: &mut App, state: &mut QuakeState) {
    let Some(app_state) = AppState::try_global(cx) else {
        log::error!("quake terminal: AppState not initialized yet; ignoring hotkey");
        return;
    };

    let project = match state.project.clone() {
        Some(project) => project,
        None => {
            let project = Project::local(
                app_state.client.clone(),
                app_state.node_runtime.clone(),
                app_state.user_store.clone(),
                app_state.languages.clone(),
                app_state.fs.clone(),
                None,
                LocalProjectFlags::default(),
                cx,
            );
            state.project = Some(project.clone());
            project
        }
    };

    // Top-center of the primary display, Guake-style proportions.
    let display = cx.primary_display();
    let display_bounds = display
        .as_ref()
        .map(|display| display.bounds())
        .unwrap_or_default();
    let display_width = f32::from(display_bounds.size.width);
    let display_height = f32::from(display_bounds.size.height);
    let width = px(display_width * 0.55);
    let height = px(display_height * 0.45);
    let x = f32::from(display_bounds.origin.x) + (display_width - f32::from(width)) / 2.0;
    let bounds = Bounds::new(point(px(x), display_bounds.origin.y), size(width, height));

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        // Created hidden; toggling shows it. Avoids flashing an empty window.
        show: false,
        kind: WindowKind::Floating,
        is_movable: false,
        display_id: display.map(|display| display.id()),
        app_id: Some("dev.terry.Terry".into()),
        ..Default::default()
    };

    let window = match cx.open_window(options, |_window, cx| {
        cx.new(|_cx| QuakeTerminalRoot {
            terminal_view: None,
        })
    }) {
        Ok(window) => window,
        Err(error) => {
            log::error!("quake terminal: failed to open window: {error:#}");
            return;
        }
    };

    // Spawn the shell now; the window renders an empty background until the
    // TerminalView is created.
    let terminal_task = project.update(cx, |project, cx| project.create_terminal_shell(None, cx));
    let Some(root) = window.entity(cx).log_err().map(|root| root.downgrade()) else {
        return;
    };
    let project_handle = project.downgrade();
    cx.spawn(async move |cx| {
        let terminal = match terminal_task.await {
            Ok(terminal) => terminal,
            Err(error) => {
                log::error!("quake terminal: failed to spawn shell: {error:#}");
                return;
            }
        };
        root.update_in(cx, |root, window, cx| {
            let view = cx.new(|cx| {
                // The quake window has no Workspace: hand TerminalView an
                // invalid weak handle, which it treats as "no workspace".
                let mut view = TerminalView::new(
                    terminal,
                    WeakEntity::<Workspace>::new_invalid(),
                    None,
                    project_handle.clone(),
                    window,
                    cx,
                );
                view.set_show_workspace_actions(false, cx);
                view
            });
            root.terminal_view = Some(view.clone());
            let focus = view.focus_handle(cx);
            window.focus(&focus, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();

    state.window = Some(window);
}

/// Root view of the quake window: hosts the standalone TerminalView.
struct QuakeTerminalRoot {
    terminal_view: Option<Entity<TerminalView>>,
}

impl Render for QuakeTerminalRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("quake-terminal-root")
            .size_full()
            .bg(cx.theme().colors().background)
            .children(self.terminal_view.clone())
    }
}
