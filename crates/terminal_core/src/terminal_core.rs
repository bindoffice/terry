//! Screen snapshots and text extraction for the terminal emulator.
//!
//! Everything here is a pure function of the VT state machine: a caller needs
//! an `alacritty_terminal::term::Term` and nothing else — no window, no layout,
//! no theme, no gpui `App`.
//!
//! That is the point of this crate. Both the GUI (through `/api/screen`, which
//! repaints a terminal for an out-of-process client) and the terminal daemon
//! (which has no GUI at all) need to turn PTY bytes into cells and lines. They
//! must agree on the result, so the code lives here instead of in either one.

pub mod palette;
pub mod screen_json;

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell as AlacCell, Flags};
use alacritty_terminal::vte::ansi::CursorShape as AlacCursorShape;
use vte::ansi::{Color, Processor, StdSyncHandler};

/// One cell of the visible screen, with enough styling for an
/// out-of-process client to repaint it.
///
/// Colors are unresolved ANSI values; callers that draw outside the app
/// resolve them against a palette (see [`palette`]).
#[derive(Clone, Debug)]
pub struct ScreenCell {
    /// Row inside the viewport, `0` at the top, independent of scrollback.
    pub row: usize,
    /// Column inside the viewport.
    pub column: usize,
    /// The cell's text: its character plus any zero-width combiners.
    pub text: String,
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikeout: bool,
    pub dim: bool,
    pub inverse: bool,
    /// This cell only holds the trailing half of a wide character.
    pub wide_char_spacer: bool,
}

/// The kind of cursor the terminal asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    HollowBlock,
    Hidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenCursor {
    pub row: usize,
    pub column: usize,
    pub shape: CursorShape,
    /// Whether the terminal asked for the cursor to be drawn (`DECTCEM`).
    pub visible: bool,
}

/// A snapshot of the visible screen, for clients that repaint the terminal
/// outside the app.
#[derive(Clone, Debug)]
pub struct ScreenSnapshot {
    pub rows: usize,
    pub columns: usize,
    /// Viewport-relative cursor, `None` when it is not on screen (for example
    /// while the viewport is scrolled back into the history).
    pub cursor: Option<ScreenCursor>,
    pub alternate_screen: bool,
    /// How far the viewport is scrolled back into the scrollback history.
    pub display_offset: usize,
    /// Cells in row-major order, one entry per grid cell.
    pub cells: Vec<ScreenCell>,
}

/// A fixed grid size for hosts that do not have a window to measure.
///
/// `alacritty_terminal` only implements `Dimensions` for tuples in its own test
/// builds, so a headless host defines its own.
pub struct FixedSize {
    pub rows: usize,
    pub columns: usize,
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

/// A headless emulator: the VT parser plus the `Term` it writes into.
///
/// This is what a host without a window uses to keep a screen around. The
/// daemon owns one per session so `/api/read_screen` and agent detection cost
/// nothing at request time — the alternative would be replaying the whole
/// output buffer on every call.
pub struct Emulator {
    term: Term<TitleListener>,
    parser: Processor<StdSyncHandler>,
    /// Cloned into the `Term`, so the title can be read without borrowing it.
    title: TitleListener,
}

impl Emulator {
    /// `scrolling_history` is the number of lines kept above the screen. A host
    /// that only reads the screen tail (the daemon) should pass a small value.
    pub fn new(columns: usize, rows: usize, scrolling_history: usize) -> Self {
        let size = FixedSize {
            rows: rows.max(1),
            columns: columns.max(1),
        };
        let config = alacritty_terminal::term::Config {
            scrolling_history,
            ..Default::default()
        };
        let title = TitleListener::default();
        Self {
            term: Term::new(config, &size, title.clone()),
            parser: Processor::new(),
            title,
        }
    }

    /// Feeds raw PTY bytes and returns how many bytes were consumed.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let size = FixedSize {
            rows: rows.max(1),
            columns: columns.max(1),
        };
        self.term.resize(size);
    }

    pub fn term(&self) -> &Term<TitleListener> {
        &self.term
    }

    /// The terminal (OSC 0/2) title, empty when unset.
    pub fn title(&self) -> String {
        self.title.title()
    }

    pub fn screen_snapshot(&self) -> ScreenSnapshot {
        make_screen_snapshot(&self.term)
    }

    /// The last `line_count` non-empty lines, oldest first.
    pub fn tail_lines(&self, line_count: usize) -> Vec<String> {
        last_non_empty_lines(&self.term, line_count)
    }
}

/// Listener that records the terminal title and ignores everything else.
///
/// The title is not decoration: the agent detector matches rules against it, so
/// a host without a GUI still needs it. Everything else (clipboard requests,
/// notifications, wakeups) is meaningless without a window.
#[derive(Clone, Debug, Default)]
pub struct TitleListener {
    title: Arc<Mutex<String>>,
}

impl TitleListener {
    /// The most recent OSC title, empty when the program never set one.
    pub fn title(&self) -> String {
        self.title
            .lock()
            .expect("title mutex poisoned")
            .clone()
    }
}

impl EventListener for TitleListener {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event;
        match event {
            Event::Title(title) => {
                *self.title.lock().expect("title mutex poisoned") = title;
            }
            Event::ResetTitle => {
                self.title
                    .lock()
                    .expect("title mutex poisoned")
                    .clear();
            }
            _ => {}
        }
    }
}

/// Turns the visible grid into a snapshot.
///
/// Cells are reported in row-major order, including the trailing half of wide
/// characters (callers skip those via [`ScreenCell::wide_char_spacer`]).
pub fn make_screen_snapshot<L: EventListener>(term: &Term<L>) -> ScreenSnapshot {
    let content = term.renderable_content();
    let rows = term.screen_lines();
    let columns = term.grid().columns();

    let mut cells = Vec::with_capacity(content.display_iter.size_hint().0);

    // `display_iter` walks the visible grid top to bottom, but line numbers are
    // absolute and go negative while scrolled back, so the viewport row comes
    // from the order the lines are yielded in.
    let mut current_line: Option<Line> = None;
    let mut row = 0;
    let mut first_line: Option<Line> = None;

    for indexed in content.display_iter {
        if current_line != Some(indexed.point.line) {
            if current_line.is_some() {
                row += 1;
            }
            current_line = Some(indexed.point.line);
            first_line.get_or_insert(indexed.point.line);
        }

        let cell = &indexed.cell;
        let mut text = String::new();
        text.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth.iter().copied());
        }

        cells.push(ScreenCell {
            row,
            column: indexed.point.column.0,
            text,
            foreground: cell.fg,
            background: cell.bg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            undercurl: cell.flags.contains(Flags::UNDERCURL),
            strikeout: cell.flags.contains(Flags::STRIKEOUT),
            dim: cell.flags.contains(Flags::DIM),
            inverse: cell.flags.contains(Flags::INVERSE),
            wide_char_spacer: cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
        });
    }

    let cursor = first_line.and_then(|first_line| {
        let cursor = content.cursor;
        let cursor_row = cursor.point.line.0 - first_line.0;
        (0..rows as i32)
            .contains(&cursor_row)
            .then(|| ScreenCursor {
                row: cursor_row as usize,
                column: cursor.point.column.0,
                shape: cursor_shape_from_alacritty(cursor.shape),
                visible: content
                    .mode
                    .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR),
            })
    });

    ScreenSnapshot {
        rows,
        columns,
        cursor,
        alternate_screen: content
            .mode
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN),
        display_offset: content.display_offset,
        cells,
    }
}

fn cursor_shape_from_alacritty(shape: AlacCursorShape) -> CursorShape {
    match shape {
        AlacCursorShape::Block => CursorShape::Block,
        AlacCursorShape::Underline => CursorShape::Underline,
        AlacCursorShape::Beam => CursorShape::Bar,
        AlacCursorShape::HollowBlock => CursorShape::HollowBlock,
        AlacCursorShape::Hidden => CursorShape::Hidden,
    }
}

/// Returns the last `line_count` non-empty lines of the buffer, oldest first.
///
/// Rows that continue a soft-wrapped logical line are joined, and trailing
/// whitespace is stripped. Used by the agent state detector and by callers that
/// only need plain text. Pure function of the emulator state.
pub fn last_non_empty_lines<L: EventListener>(term: &Term<L>, line_count: usize) -> Vec<String> {
    let grid = term.grid();
    let mut lines = Vec::new();

    let mut current_line = grid.bottommost_line().0;
    let topmost_line = grid.topmost_line().0;

    while current_line >= topmost_line && lines.len() < line_count {
        let (logical_line_start, logical_line) =
            logical_line_for_row(grid, current_line, topmost_line);

        if let Some(line) = process_line(logical_line) {
            lines.push(line);
        }

        current_line = logical_line_start - 1;
    }

    lines.reverse();
    lines
}

fn logical_line_for_row(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> (i32, String) {
    let start = find_logical_line_start(grid, current, topmost);
    let mut line = String::new();
    for row in start..=current {
        line.push_str(&row_to_string(&grid[Line(row)]));
    }
    (start, line)
}

fn find_logical_line_start(grid: &Grid<AlacCell>, current: i32, topmost: i32) -> i32 {
    let mut line_start = current;
    while line_start > topmost {
        let previous_line = Line(line_start - 1);
        let last_cell = &grid[previous_line][Column(grid.columns() - 1)];
        if !last_cell.flags.contains(Flags::WRAPLINE) {
            break;
        }
        line_start -= 1;
    }
    line_start
}

fn row_to_string(row: &Row<AlacCell>) -> String {
    row[..Column(row.len())]
        .iter()
        .map(|cell| cell.c)
        .collect::<String>()
}

fn process_line(line: String) -> Option<String> {
    let trimmed = line.trim_end().to_string();
    if !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(columns: usize, rows: usize, bytes: &[u8]) -> ScreenSnapshot {
        let mut emulator = Emulator::new(columns, rows, 1000);
        emulator.advance(bytes);
        emulator.screen_snapshot()
    }

    #[test]
    fn snapshot_reports_the_grid_size() {
        let snapshot = feed(20, 4, b"hello");

        assert_eq!(snapshot.rows, 4);
        assert_eq!(snapshot.columns, 20);
        assert!(!snapshot.cells.is_empty());
    }

    #[test]
    fn snapshot_keeps_unresolved_ansi_colors() {
        let snapshot = feed(20, 4, b"\x1b[31mred\x1b[0m");
        let first = snapshot
            .cells
            .iter()
            .find(|cell| cell.text == "r")
            .expect("the red run should be on screen");

        assert_eq!(
            first.foreground,
            Color::Named(vte::ansi::NamedColor::Red),
            "colors stay unresolved so each host can pick its own palette"
        );
    }

    #[test]
    fn wide_characters_mark_their_spacer_cell() {
        // A CJK character occupies two columns; the trailing cell is a spacer.
        let snapshot = feed(10, 2, "漢".as_bytes());
        let spacers: Vec<_> = snapshot
            .cells
            .iter()
            .filter(|cell| cell.wide_char_spacer)
            .collect();

        assert_eq!(spacers.len(), 1, "one trailing half per wide character");
    }

    #[test]
    fn tail_lines_returns_the_last_non_empty_lines() {
        let mut emulator = Emulator::new(20, 6, 1000);
        emulator.advance(b"one\r\ntwo\r\n\r\nthree\r\n");

        assert_eq!(
            emulator.tail_lines(10),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn tail_lines_joins_soft_wrapped_rows() {
        let mut emulator = Emulator::new(10, 6, 1000);
        // 14 characters in a 10-column grid wraps onto a second row.
        emulator.advance(b"aaaaabbbbbcccc\r\n");

        assert_eq!(emulator.tail_lines(10), vec!["aaaaabbbbbcccc".to_string()]);
    }

    #[test]
    fn tracks_the_osc_title() {
        // Agent detection matches rules against the title, so a headless host
        // must not throw it away.
        let mut emulator = Emulator::new(20, 4, 1000);
        assert_eq!(emulator.title(), "");

        emulator.advance(b"\x1b]2;claude - myproject\x07");
        assert_eq!(emulator.title(), "claude - myproject");

        emulator.advance(b"\x1b]2;\x07");
        assert_eq!(emulator.title(), "", "an empty title resets it");
    }

    #[test]
    fn resize_keeps_the_emulator_usable() {
        let mut emulator = Emulator::new(20, 4, 1000);
        emulator.advance(b"hello");
        emulator.resize(40, 10);

        let snapshot = emulator.screen_snapshot();
        assert_eq!((snapshot.columns, snapshot.rows), (40, 10));
    }
}
