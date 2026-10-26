//! Export terminal scrollback.
//!
//! Exports the most recent lines of a terminal's scrollback buffer as plain
//! text or Markdown, either to the clipboard, to the clipboard history, or to
//! a file picked by the user.
//!
//! The actions live in the Terry namespace and act on the active terminal;
//! the sidebar's context menu calls the same helpers for a specific terminal.

use std::path::PathBuf;

use fs::Fs;
use gpui::{App, ClipboardItem, Entity, actions};
use terminal_view::TerminalView;

use crate::terminal_list_panel::TerminalListPanel;

actions!(
    scrollback_export,
    [
        /// Copies the last lines of the active terminal's scrollback.
        CopyScrollback,
        /// Copies the last lines of the active terminal as a Markdown block.
        CopyScrollbackAsMarkdown,
        /// Adds the last lines of the active terminal to the clipboard history.
        ScrollbackToClipboardHistory,
        /// Saves the last lines of the active terminal to a file.
        SaveScrollbackToFile,
    ]
);

/// Number of trailing lines exported by the actions.
pub const EXPORT_LINES: usize = 200;

/// Localized label for an export entry, with the line count substituted.
pub fn label(key: &str) -> String {
    i18n::t(key).replace("{count}", &EXPORT_LINES.to_string())
}

fn tail(text: &str, lines: usize) -> String {
    let mut all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all.drain(..start);
    all.join("\n")
}

struct Scrollback {
    content: String,
    title: String,
    cwd: Option<String>,
}

fn capture(view: &Entity<TerminalView>, cx: &App) -> Scrollback {
    let view = view.read(cx);
    let terminal = view.terminal().read(cx);
    Scrollback {
        content: tail(&terminal.get_content(), EXPORT_LINES),
        title: terminal.title(true),
        cwd: terminal
            .working_directory()
            .map(|path| path.display().to_string()),
    }
}

fn as_markdown(scrollback: &Scrollback) -> String {
    let mut out = String::new();
    out.push_str("# Terminal output\n\n");
    if !scrollback.title.trim().is_empty() {
        out.push_str(&format!("- Terminal: {}\n", scrollback.title));
    }
    if let Some(cwd) = scrollback.cwd.as_deref() {
        out.push_str(&format!("- Directory: `{cwd}`\n"));
    }
    out.push_str("\n```text\n");
    out.push_str(&scrollback.content);
    if !scrollback.content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

/// Copies the last [`EXPORT_LINES`] lines of `view` to the system clipboard.
pub fn copy(view: &Entity<TerminalView>, cx: &mut App) {
    let scrollback = capture(view, cx);
    cx.write_to_clipboard(ClipboardItem::new_string(scrollback.content));
}

/// Copies the last [`EXPORT_LINES`] lines of `view` as a Markdown block.
pub fn copy_markdown(view: &Entity<TerminalView>, cx: &mut App) {
    let scrollback = capture(view, cx);
    cx.write_to_clipboard(ClipboardItem::new_string(as_markdown(&scrollback)));
}

/// Adds the last [`EXPORT_LINES`] lines of `view` to the clipboard history.
pub fn add_to_history(view: &Entity<TerminalView>, cx: &mut App) {
    let scrollback = capture(view, cx);
    crate::clipboard_history::add_text(&scrollback.content, cx);
}

/// Prompts for a destination and writes the Markdown export there.
pub fn save_to_file(view: &Entity<TerminalView>, cx: &mut App) {
    let scrollback = capture(view, cx);
    let markdown = as_markdown(&scrollback);
    let directory = scrollback
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| util::paths::home_dir().clone());

    let rx = cx.prompt_for_new_path(&directory, Some("scrollback.md"));
    let fs = <dyn Fs>::global(cx);
    cx.spawn(async move |_cx| {
        let path = match rx.await {
            Ok(Ok(Some(path))) => path,
            Ok(_) => return,
            Err(error) => {
                log::error!("scrollback export: path prompt failed: {error}");
                return;
            }
        };
        if let Err(error) = fs.write(&path, markdown.as_bytes()).await {
            log::error!(
                "scrollback export: failed to write {}: {error}",
                path.display()
            );
        }
    })
    .detach();
}

/// Registers the export actions on `workspace`, acting on the active terminal.
pub fn active_terminal(workspace: &workspace::Workspace, cx: &App) -> Option<Entity<TerminalView>> {
    workspace
        .panel::<TerminalListPanel>(cx)?
        .read(cx)
        .active_terminal_view(cx)
}
