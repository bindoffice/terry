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

/// Attaches to (or creates) `target` and mirrors its panes as native terminals.
pub fn attach(target: String, window: &mut Window, cx: &mut Context<TerminalListPanel>) {
    let mut client = match ControlClient::attach_or_create(&target) {
        Ok(client) => client,
        Err(error) => {
            log::error!("tmux native: failed to attach to {target}: {error}");
            return;
        }
    };
    let Some(reader) = client.take_reader() else {
        return;
    };

    let client = Arc::new(Mutex::new(client));
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

    cx.spawn_in(window, async move |panel, cx| {
        let mut panes: HashMap<u32, Entity<Terminal>> = HashMap::new();
        let mut pending: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut requested = false;

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
                        terminal.update(cx, |terminal, cx| terminal.write_output(&data, cx));
                    } else {
                        pending.entry(pane.0).or_default().extend_from_slice(&data);
                    }
                }
                ControlEvent::Response(line) => {
                    let Some((pane_id, cwd)) = parse_pane_line(&line) else {
                        continue;
                    };
                    if panes.contains_key(&pane_id) {
                        continue;
                    }
                    let buffered = pending.remove(&pane_id).unwrap_or_default();
                    let client = client.clone();
                    let created = panel.update_in(cx, |panel, window, cx| {
                        panel.add_mirrored_pane(pane_id, cwd, buffered, client, window, cx)
                    });
                    match created {
                        Ok(Some(terminal)) => {
                            panes.insert(pane_id, terminal);
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                ControlEvent::Exit(_) => break,
                _ => {}
            }
        }
    })
    .detach();
}

/// Parses `pane_id|||cwd` lines returned by [`LIST_PANES_SIMPLE`].
fn parse_pane_line(line: &str) -> Option<(u32, Option<std::path::PathBuf>)> {
    let (pane, cwd) = line.split_once("|||")?;
    let pane_id = pane.trim().strip_prefix('%')?.parse().ok()?;
    let cwd = cwd.trim();
    let cwd = (!cwd.is_empty()).then(|| std::path::PathBuf::from(cwd));
    Some((pane_id, cwd))
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
