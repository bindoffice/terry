//! Move a tab into a brand-new window.
//!
//! True cross-window tab dragging is not supported by the platform layer (the
//! mouse-up is only delivered to the window where the drag started), so this
//! exposes the same operation as a command. The item handle is moved without
//! closing it, which keeps the terminal's PTY and scrollback alive.

use gpui::{App, AppContext as _, Context, Window, actions};
use terminal_view::TerminalView;
use workspace::{MultiWorkspace, Workspace};

use crate::terminal_list_panel::TerminalListPanel;

actions!(
    move_to_window,
    [
        /// Moves the active terminal tab to a new window, keeping its process alive.
        MoveTabToNewWindow,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &MoveTabToNewWindow, window, cx| {
            move_active_tab_to_new_window(workspace, window, cx);
        });
    })
    .detach();
}

fn move_active_tab_to_new_window(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(active_item) = workspace.active_pane().read(cx).active_item() else {
        return;
    };
    let Some(terminal_view) = active_item.act_as::<TerminalView>(cx) else {
        return;
    };
    let Some(panel) = workspace.panel::<TerminalListPanel>(cx) else {
        return;
    };
    let (cwd, launch) = panel
        .read(cx)
        .terminal_move_metadata(terminal_view.entity_id());

    // Remove the item from its pane while keeping the handle (and therefore the
    // PTY) alive until it is added to the new window. The panel must not treat
    // this as a close (it would drop the launch recipe and fill the reopen
    // stack with a still-running terminal).
    panel.update(cx, |panel, _| panel.set_suppress_recently_closed(true));
    let pane = workspace.active_pane().clone();
    let item = pane.update(cx, |pane, cx| pane.take_active_item(window, cx));
    panel.update(cx, |panel, _| panel.set_suppress_recently_closed(false));
    let Some(item) = item else {
        return;
    };

    let project = workspace.project().clone();
    let app_state = workspace.app_state().clone();
    cx.defer(move |cx| {
        let options = (app_state.build_window_options)(None, cx);
        let result = cx.open_window(options, move |window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace =
                    Workspace::new(None, project.clone(), app_state.clone(), window, cx);
                crate::init_workspace(&mut workspace, window, cx);
                workspace
            });

            if let Some(panel) = workspace.read(cx).panel::<TerminalListPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.prepare_for_moved_terminal(
                        &terminal_view,
                        cwd.clone(),
                        launch.clone(),
                        cx,
                    );
                });
            }

            let pane = workspace.read(cx).active_pane().clone();
            pane.update(cx, |pane, cx| {
                pane.add_item(item, true, true, None, window, cx);
            });

            if let Some(panel) = workspace.read(cx).panel::<TerminalListPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.adopt_moved_terminal(&terminal_view, cwd.clone(), launch.clone(), cx);
                });
            }

            let workspace_weak = workspace.downgrade();
            let project_weak = workspace.read(cx).project().downgrade();
            terminal_view.update(cx, |view, cx| {
                view.rebind_workspace(workspace_weak, project_weak, window, cx);
            });

            cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
        });

        if let Err(error) = result {
            log::error!("move tab to new window: {error:#}");
        }
    });
}
