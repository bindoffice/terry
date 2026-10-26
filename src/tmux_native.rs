//! Native tmux panes via control mode (experimental).
//!
//! Attaches to a tmux session with `tmux -CC`, mirrors every pane into a
//! display-only terminal and forwards keystrokes back with `send-keys`. Panes
//! are added to the active group and tiled.
//!
//! Layout fidelity is approximate: panes are tiled by the group's layout mode
//! rather than reproducing tmux's split tree.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use gpui::{Context, Entity, Window, actions};
use terminal::Terminal;

use crate::terminal_list_panel::TerminalListPanel;
use crate::tmux_control::{
    ControlClient, ControlEvent, LIST_PANES_SIMPLE, PaneId, parse_line, send_key, send_keys_literal,
};

actions!(tmux_native, [AttachTmuxControlMode]);

/// Shared handle to the control client currently feeding mirrored panes.
/// `None` while disconnected, so input forwarding becomes a no-op.
pub type SharedTmuxClient = Arc<Mutex<Option<Arc<Mutex<ControlClient>>>>>;

/// Max consecutive reconnect attempts for a dropped control client.
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Attaches to (or creates) `target` and mirrors its panes as native terminals,
/// reconnecting with backoff when the client exits.
pub fn attach(target: String, window: &mut Window, cx: &mut Context<TerminalListPanel>) {
    cx.spawn_in(window, async move |panel, cx| {
        // Mirrored state survives reconnects; a fresh client resets `requested`.
        let mut panes: HashMap<u32, Entity<Terminal>> = HashMap::new();
        let mut pending: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut pane_order: HashMap<u32, Vec<u32>> = HashMap::new();
        // Holds the client currently feeding the panes, so input forwarding keeps
        // working after a reconnect.
        let current: SharedTmuxClient = Arc::new(Mutex::new(None));
        let mut attempts = 0u32;

        loop {
            let mut client = match ControlClient::attach_or_create(&target) {
                Ok(client) => client,
                Err(error) => {
                    log::error!("tmux native: failed to attach to {target}: {error}");
                    if !backoff(&mut attempts, &target, cx).await {
                        break;
                    }
                    continue;
                }
            };
            let Some(reader) = client.take_reader() else {
                break;
            };
            let client = Arc::new(Mutex::new(client));
            if let Ok(mut slot) = current.lock() {
                *slot = Some(client.clone());
            }

            let (tx, mut rx) = futures::channel::mpsc::unbounded::<ControlEvent>();
            // Blocking line reader on its own thread; it never touches the UI.
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut line = String::new();
                loop {
                    line.clear();
                    match std::io::BufRead::read_line(&mut reader, &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if tx.unbounded_send(parse_line(&line)).is_err() {
                                break;
                            }
                        }
                    }
                }
            });

            let mut requested = false;
            let mut reconnect = false;
            while let Some(event) = rx.next().await {
                match event {
                    ControlEvent::SessionChanged { .. } => {
                        if !requested {
                            requested = true;
                            if let Ok(mut client) = client.lock() {
                                let _ = client.send(LIST_PANES_SIMPLE);
                            }
                        }
                    }
                    ControlEvent::Output { pane, data } => {
                        if let Some(terminal) = panes.get(&pane.0) {
                            let terminal = terminal.clone();
                            terminal.update(cx, |terminal, cx| {
                                terminal.write_output(&data, cx);
                            });
                        } else {
                            pending.entry(pane.0).or_default().extend_from_slice(&data);
                        }
                    }
                    ControlEvent::Response(line) => {
                        let Some((window_id, pane_id, cwd)) = parse_pane_line(&line) else {
                            continue;
                        };
                        if panes.contains_key(&pane_id) {
                            continue;
                        }
                        let buffered = pending.remove(&pane_id).unwrap_or_default();
                        let pane_client = current.clone();
                        let created = panel.update_in(cx, |panel, window, cx| {
                            panel.add_mirrored_pane(
                                window_id,
                                pane_id,
                                cwd,
                                buffered,
                                pane_client,
                                window,
                                cx,
                            )
                        });
                        match created {
                            Ok(Some(terminal)) => {
                                pane_order.entry(window_id).or_default().push(pane_id);
                                // Best-effort: tell tmux the client viewport size so
                                // its panes match the mirrored terminals.
                                let (columns, rows) =
                                    terminal.update(cx, |terminal, _cx| terminal.grid_size());
                                if columns > 0 && rows > 0 {
                                    if let Ok(mut client) = client.lock() {
                                        let _ =
                                            client.send(&crate::tmux_control::refresh_client_size(
                                                columns, rows,
                                            ));
                                    }
                                }
                                panes.insert(pane_id, terminal);
                            }
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                    ControlEvent::LayoutChange { window, layout } => {
                        let window_id = window.0;
                        let order = pane_order.get(&window_id).cloned().unwrap_or_default();
                        let applied = panel.update_in(cx, |panel, window, cx| {
                            panel.apply_tmux_layout(window_id, &order, &layout, window, cx);
                        });
                        if applied.is_err() {
                            break;
                        }
                    }
                    ControlEvent::WindowRenamed { window, name } => {
                        let window_id = window.0;
                        if panel
                            .update_in(cx, |panel, _window, cx| {
                                panel.rename_tmux_group(window_id, name, cx)
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    ControlEvent::Exit(_) => {
                        reconnect = true;
                        break;
                    }
                    _ => {}
                }
            }

            if let Ok(mut slot) = current.lock() {
                *slot = None;
            }
            if !reconnect {
                break;
            }

            // Drop the stale mirror so the rebuilt one does not duplicate panes.
            if panel
                .update_in(cx, |panel, window, cx| panel.reset_tmux_mirror(window, cx))
                .is_err()
            {
                break;
            }
            panes.clear();
            pending.clear();
            pane_order.clear();
            if !backoff(&mut attempts, &target, cx).await {
                break;
            }
        }
    })
    .detach();
}

/// Waits before a reconnect attempt. Returns false when the budget is spent.
async fn backoff(attempts: &mut u32, target: &str, cx: &mut gpui::AsyncWindowContext) -> bool {
    *attempts += 1;
    if *attempts > MAX_RECONNECT_ATTEMPTS {
        log::warn!("tmux native: giving up on {target} after {attempts} attempts");
        return false;
    }
    let delay = std::time::Duration::from_secs(1u64 << (*attempts - 1).min(4));
    log::info!("tmux native: reconnecting to {target} in {delay:?} (attempt {attempts})");
    cx.background_executor().timer(delay).await;
    true
}

/// Parses `window_id|||pane_id|||cwd` lines returned by
/// [`LIST_PANES_SIMPLE`].
fn parse_pane_line(line: &str) -> Option<(u32, u32, Option<std::path::PathBuf>)> {
    let mut parts = line.split("|||");
    let window = parts.next()?.trim().strip_prefix('@')?.parse().ok()?;
    let pane = parts.next()?.trim().strip_prefix('%')?.parse().ok()?;
    let cwd = parts.next().unwrap_or("").trim();
    let cwd = (!cwd.is_empty()).then(|| std::path::PathBuf::from(cwd));
    Some((window, pane, cwd))
}

/// Forwards a chunk of input to a pane, mapping CR to the Enter key.
#[allow(dead_code)]
pub fn forward_input(client: &Mutex<ControlClient>, pane: PaneId, bytes: &[u8]) -> Option<()> {
    let text = String::from_utf8_lossy(bytes).to_string();
    if text.is_empty() {
        return None;
    }
    let mut client = client.lock().ok()?;
    let command = if text == "\r" {
        send_key(pane, "Enter")
    } else {
        send_keys_literal(pane, &text)
    };
    client.send(&command).ok()?;
    Some(())
}
