//! Session recording in asciicast v2 format (output only).
//!
//! While recording, every chunk of raw PTY output is written as a timed event
//! `[elapsed, "o", "<data>"]` to a `.cast` file under the app data directory.
//! The result can be replayed with `asciinema play <file>` or any other
//! asciicast-compatible player.
//!
//! Recording taps the PTY output at the terminal layer, so it captures exactly
//! what the application emitted (ANSI sequences included), independent of the
//! renderer.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, BorrowAppContext as _, Entity, EntityId, Global, actions};
use terminal_view::TerminalView;

actions!(session_recording, [ToggleSessionRecording]);

/// App-wide recording state: which terminals are being recorded and where.
#[derive(Default)]
pub struct SessionRecording {
    active: HashMap<EntityId, PathBuf>,
}

impl Global for SessionRecording {}

/// Directory recordings are written to.
pub fn recordings_dir() -> PathBuf {
    paths::data_dir().join("recordings")
}

pub fn is_recording(id: EntityId, cx: &App) -> bool {
    cx.try_global::<SessionRecording>()
        .is_some_and(|recording| recording.active.contains_key(&id))
}

/// Path of the file currently being recorded for `id`, if any.
#[allow(dead_code)]
pub fn recording_path(id: EntityId, cx: &App) -> Option<PathBuf> {
    cx.try_global::<SessionRecording>()
        .and_then(|recording| recording.active.get(&id).cloned())
}

/// Starts or stops recording `view`. Returns whether it is now recording.
pub fn toggle(view: &Entity<TerminalView>, cx: &mut App) -> bool {
    let id = view.entity_id();
    if is_recording(id, cx) {
        stop(view, cx);
        false
    } else {
        start(view, cx);
        true
    }
}

fn start(view: &Entity<TerminalView>, cx: &mut App) {
    let id = view.entity_id();
    let directory = recordings_dir();
    if let Err(error) = std::fs::create_dir_all(&directory) {
        log::error!(
            "session recording: failed to create {}: {error}",
            directory.display()
        );
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let path = directory.join(format!("{timestamp}-{}.cast", id.as_u64()));

    let header = format!(
        "{{\"version\": 2, \"width\": 80, \"height\": 24, \"timestamp\": {timestamp}, \"env\": {{\"TERM\": \"xterm-256color\"}}}}\n"
    );
    let file = match std::fs::File::create(&path) {
        Ok(file) => file,
        Err(error) => {
            log::error!(
                "session recording: failed to create {}: {error}",
                path.display()
            );
            return;
        }
    };

    let mut file = std::io::BufWriter::new(file);
    if file.write_all(header.as_bytes()).is_err() {
        log::error!(
            "session recording: failed to write header to {}",
            path.display()
        );
        return;
    }
    let _ = file.flush();

    let (tx, mut rx) = futures::channel::mpsc::unbounded::<(f64, Vec<u8>)>();
    cx.background_executor()
        .spawn(async move {
            use futures::StreamExt as _;
            while let Some((elapsed, bytes)) = rx.next().await {
                let text = String::from_utf8_lossy(&bytes);
                let Ok(escaped) = serde_json::to_string(&text) else {
                    continue;
                };
                let line = format!("[{elapsed:.6}, \"o\", {escaped}]\n");
                if file.write_all(line.as_bytes()).is_err() {
                    break;
                }
                let _ = file.flush();
            }
        })
        .detach();

    let started = Instant::now();
    let sink: terminal::OutputSink = Arc::new(move |bytes: &[u8]| {
        let _ = tx.unbounded_send((started.elapsed().as_secs_f64(), bytes.to_vec()));
    });

    view.update(cx, |view, cx| {
        view.terminal().update(cx, |terminal, _cx| {
            terminal.set_output_sink(Some(sink));
        });
    });

    cx.update_global::<SessionRecording, _>(|recording, _| {
        recording.active.insert(id, path.clone());
    });
    log::info!("session recording: writing {}", path.display());
}

fn stop(view: &Entity<TerminalView>, cx: &mut App) {
    let id = view.entity_id();
    view.update(cx, |view, cx| {
        view.terminal().update(cx, |terminal, _cx| {
            // Dropping the sink closes the channel and ends the writer task.
            terminal.set_output_sink(None);
        });
    });
    cx.update_global::<SessionRecording, _>(|recording, _| {
        recording.active.remove(&id);
    });
}

/// Human-readable list of recording files, newest first.
#[allow(dead_code)]
pub fn recent_recordings() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(recordings_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "cast")
        })
        .collect();
    files.sort();
    files.reverse();
    files
}

/// Reveals the recordings directory in the platform file manager.
pub fn reveal_recordings(cx: &mut App) {
    let directory = recordings_dir();
    std::fs::create_dir_all(&directory)
        .map_err(|error| log::error!("session recording: {error}"))
        .ok();
    cx.reveal_path(&directory);
}
