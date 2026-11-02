//! Encodes a [`ScreenSnapshot`] as the JSON an out-of-process client repaints.
//!
//! This is the payload of `/api/screen`. It lives here rather than next to the
//! IPC handlers because the terminal daemon serves the same endpoint without a
//! GUI, and both sides must produce an identical shape: a phone client written
//! against the desktop endpoint has to keep working when it points at the
//! daemon.

use serde_json::{Value, json};

use crate::palette::{Palette, Rgb};
use crate::{CursorShape, ScreenCell, ScreenSnapshot};

/// A cell ready for encoding: colors already resolved, no theme needed.
struct StyledCell {
    row: usize,
    column: usize,
    text: String,
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    attributes: Vec<&'static str>,
}

/// A run of adjacent viewport cells that share one style.
struct ScreenRun {
    row: usize,
    column: usize,
    /// Column just past the run, used to merge adjacent cells.
    end_column: usize,
    text: String,
    /// `None` when the run uses the terminal's default color.
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    attributes: Vec<&'static str>,
}

impl ScreenRun {
    fn into_json(self) -> Value {
        let mut run = serde_json::Map::new();
        run.insert("col".into(), json!(self.column));
        run.insert("text".into(), json!(self.text));
        if let Some(foreground) = self.foreground {
            run.insert("fg".into(), json!(foreground.to_hex()));
        }
        if let Some(background) = self.background {
            run.insert("bg".into(), json!(background.to_hex()));
        }
        if !self.attributes.is_empty() {
            run.insert("attrs".into(), json!(self.attributes));
        }
        Value::Object(run)
    }
}

/// Serializes a screen snapshot.
///
/// Each line is a list of styled runs; every run carries the column it starts
/// at, so clients can position it on a monospace grid (and stay aligned across
/// wide characters, which occupy two cells). Blank cells with the default
/// background are dropped, and colors matching the defaults are reported as
/// absent, which keeps the payload small.
pub fn screen_payload(id: u64, snapshot: &ScreenSnapshot, palette: &Palette) -> Value {
    let default_foreground = palette.foreground;
    let default_background = palette.background;

    let cells = snapshot
        .cells
        .iter()
        // A wide character owns its leading cell; the trailing half carries no
        // text of its own.
        .filter(|cell| !cell.wide_char_spacer)
        .map(|cell| {
            let (foreground, background) =
                run_colors(cell, palette, default_foreground, default_background);
            StyledCell {
                row: cell.row,
                column: cell.column,
                text: cell.text.clone(),
                foreground,
                background,
                attributes: cell_attributes(cell),
            }
        });

    let lines = encode_lines(snapshot.rows, cells);

    let cursor = snapshot.cursor.map(|cursor| {
        json!({
            "row": cursor.row,
            "col": cursor.column,
            "shape": cursor_shape_name(cursor.shape),
            "visible": cursor.visible,
        })
    });

    json!({
        "terminal_id": id,
        "rows": snapshot.rows,
        "cols": snapshot.columns,
        "alternate_screen": snapshot.alternate_screen,
        "display_offset": snapshot.display_offset,
        "default_fg": default_foreground.to_hex(),
        "default_bg": default_background.to_hex(),
        "cursor": cursor,
        "lines": lines,
    })
}

/// Groups styled cells into one run list per row.
///
/// Adjacent cells with the same style merge into one run; spaces that are
/// unstyled on the default background are dropped. Every run records the
/// column it starts at, so dropped cells never shift the rest of the row.
fn encode_lines(rows: usize, cells: impl IntoIterator<Item = StyledCell>) -> Vec<Vec<Value>> {
    let mut lines: Vec<Vec<Value>> = vec![Vec::new(); rows];
    let mut current: Option<ScreenRun> = None;

    for cell in cells {
        // Spaces with no styling and no background carry no information.
        if cell.text.trim_end().is_empty()
            && cell.background.is_none()
            && cell.attributes.is_empty()
        {
            continue;
        }

        match &mut current {
            Some(run)
                if run.row == cell.row
                    && run.end_column == cell.column
                    && run.foreground == cell.foreground
                    && run.background == cell.background
                    && run.attributes == cell.attributes =>
            {
                run.text.push_str(&cell.text);
                run.end_column = cell.column + 1;
            }
            _ => {
                if let Some(run) = current.take() {
                    push_run(&mut lines, run);
                }
                current = Some(ScreenRun {
                    row: cell.row,
                    column: cell.column,
                    end_column: cell.column + 1,
                    text: cell.text,
                    foreground: cell.foreground,
                    background: cell.background,
                    attributes: cell.attributes,
                });
            }
        }
    }

    if let Some(run) = current.take() {
        push_run(&mut lines, run);
    }

    lines
}

fn push_run(lines: &mut [Vec<Value>], run: ScreenRun) {
    if let Some(line) = lines.get_mut(run.row) {
        line.push(run.into_json());
    }
}

/// Resolves a cell's colors against the palette.
///
/// Returns `None` for a color that matches the terminal default, since clients
/// fall back to `default_fg` / `default_bg`. Reverse video is applied here so
/// clients do not have to know about it.
fn run_colors(
    cell: &ScreenCell,
    palette: &Palette,
    default_foreground: Rgb,
    default_background: Rgb,
) -> (Option<Rgb>, Option<Rgb>) {
    let mut foreground = palette.resolve(&cell.foreground);
    let mut background = palette.resolve(&cell.background);

    if cell.inverse {
        std::mem::swap(&mut foreground, &mut background);
    }

    (
        (foreground != default_foreground).then_some(foreground),
        (background != default_background).then_some(background),
    )
}

fn cell_attributes(cell: &ScreenCell) -> Vec<&'static str> {
    let mut attributes = Vec::new();
    if cell.bold {
        attributes.push("bold");
    }
    if cell.italic {
        attributes.push("italic");
    }
    if cell.underline {
        attributes.push("underline");
    }
    if cell.undercurl {
        attributes.push("undercurl");
    }
    if cell.strikeout {
        attributes.push("strikeout");
    }
    if cell.dim {
        attributes.push("dim");
    }
    attributes
}

fn cursor_shape_name(shape: CursorShape) -> &'static str {
    match shape {
        CursorShape::Block => "block",
        CursorShape::Underline => "underline",
        CursorShape::Bar => "bar",
        CursorShape::HollowBlock => "hollow",
        CursorShape::Hidden => "hidden",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    fn snapshot_of(bytes: &[u8]) -> ScreenSnapshot {
        let mut emulator = Emulator::new(20, 4, 1000);
        emulator.advance(bytes);
        emulator.screen_snapshot()
    }

    #[test]
    fn payload_reports_the_frame_geometry() {
        let payload = screen_payload(7, &snapshot_of(b"hello"), &Palette::xterm());

        assert_eq!(payload["terminal_id"], 7);
        assert_eq!(payload["rows"], 4);
        assert_eq!(payload["cols"], 20);
        assert_eq!(payload["default_bg"], Palette::xterm().background.to_hex());
    }

    #[test]
    fn adjacent_cells_with_one_style_form_a_run() {
        let payload = screen_payload(1, &snapshot_of(b"\x1b[31mred\x1b[0m"), &Palette::xterm());
        let runs = payload["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .flat_map(|line| line.as_array().into_iter().flatten())
            .collect::<Vec<_>>();

        let red = runs
            .iter()
            .find(|run| run["text"].as_str().is_some_and(|t| t.contains("red")))
            .expect("the red text should be encoded");
        assert_eq!(red["col"], 0);
        assert_eq!(
            red["fg"], "#cd0000",
            "the run carries the resolved palette color"
        );
    }

    #[test]
    fn unstyled_spaces_are_dropped_without_shifting_columns() {
        // "a" then spaces then "b": the second run must still report col 10.
        let payload = screen_payload(1, &snapshot_of(b"a         b"), &Palette::xterm());
        let runs = payload["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .flat_map(|line| line.as_array().into_iter().flatten())
            .collect::<Vec<_>>();

        assert_eq!(runs.len(), 2, "the gap is dropped: {runs:?}");
        assert_eq!(runs[1]["col"], 10);
    }

    #[test]
    fn inverse_video_swaps_the_colors() {
        let payload = screen_payload(1, &snapshot_of(b"\x1b[7mX\x1b[0m"), &Palette::xterm());
        let run = payload["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .flat_map(|line| line.as_array().into_iter().flatten())
            .next()
            .expect("one run");

        let palette = Palette::xterm();
        assert_eq!(
            run["fg"],
            palette.background.to_hex(),
            "reverse video is resolved server-side"
        );
        assert_eq!(run["bg"], palette.foreground.to_hex());
    }
}
