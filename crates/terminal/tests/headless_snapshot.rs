//! Verifies that the snapshot and text-extraction helpers can be used without
//! a GUI: only `alacritty_terminal` (a pure VT state machine), `vte` and
//! `agent_detect` are involved. This is the contract the IPC endpoints and a
//! future headless/daemon-side server rely on.

use agent_detect::{AgentState, SCREEN_LINES, Screen, detect};
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use terminal::{last_non_empty_lines, make_screen_snapshot};
use vte::ansi::{Processor, StdSyncHandler};

/// Fixed grid size; `alacritty_terminal` only implements `Dimensions` for
/// tuples in its own test builds, so a headless user defines its own.
struct FixedSize {
    rows: usize,
    columns: usize,
}

impl Dimensions for FixedSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Feeds raw PTY bytes into a bare `Term` with no window, theme, or event loop.
fn feed(columns: usize, rows: usize, bytes: &[u8]) -> Term<VoidListener> {
    let size = FixedSize { rows, columns };
    let mut term = Term::new(
        Config {
            scrolling_history: 1000,
            ..Config::default()
        },
        &size,
        VoidListener,
    );
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut term, bytes);
    term
}

#[test]
fn screen_snapshot_needs_no_gui() {
    let term = feed(20, 4, b"hello\r\nworld");

    let snapshot = make_screen_snapshot(&term);

    assert_eq!(snapshot.rows, 4);
    assert_eq!(snapshot.columns, 20);
    // Colors stay as unresolved ANSI values here; resolving them against a
    // theme is the caller's job (`/api/screen` does it in the GUI process).
    assert!(!snapshot.cells.is_empty());

    let first_row: String = snapshot
        .cells
        .iter()
        .filter(|cell| cell.row == 0)
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(first_row.starts_with("hello"), "row 0 was {first_row:?}");
}

#[test]
fn last_non_empty_lines_feeds_the_agent_detector() {
    // A block-style prompt drawn at the bottom of the buffer.
    let term = feed(
        40,
        8,
        "some earlier output\r\n\
         \u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\r\n\
         \u{2502} Do you want to proceed?\r\n\
         \u{2502} \u{276f} 1. Yes\r\n\
         \u{2502}   2. No\r\n"
            .as_bytes(),
    );

    let lines = last_non_empty_lines(&term, SCREEN_LINES);
    assert!(!lines.is_empty());
    assert!(lines.iter().any(|line| line.contains("proceed")));

    let screen = Screen {
        lines,
        title: String::new(),
    };
    // Whatever the rules decide, the call must not need any GUI state.
    let _: Option<AgentState> = detect("claude", &screen);
}

#[test]
fn both_helpers_are_usable_from_a_plain_function() {
    // Mirrors what a non-GUI server would do: own a `Term`, answer questions.
    fn describe(bytes: &[u8]) -> (usize, usize, Vec<String>) {
        let term = feed(40, 6, bytes);
        let snapshot = make_screen_snapshot(&term);
        let lines = last_non_empty_lines(&term, 5);
        (snapshot.rows, snapshot.columns, lines)
    }

    let (rows, columns, lines) = describe(b"one\r\ntwo\r\n");
    assert_eq!((rows, columns), (6, 40));
    assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
}
