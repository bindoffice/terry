//! Scan the visible terminal grid for things a keyboard-driven hint mode can
//! act on: URLs, paths, git hashes, UUIDs, IPs, and user-defined regexes.

use std::collections::HashSet;

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    term::cell::Flags,
};
use regex::Regex;

use crate::{
    Point, Range,
    terminal_settings::{HintAction, HintRule},
};

/// Kept in sync with the URL detector used for clickable links. It excludes
/// whitespace and the punctuation that commonly follows a URL in prose.
const URL_REGEX: &str = r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://|zed://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^⟨⟩`']+"#;
const GIT_HASH_REGEX: &str = r"\b[0-9a-fA-F]{7,40}\b";
const UUID_REGEX: &str =
    r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b";
const IPV4_REGEX: &str = r"\b(?:\d{1,3}\.){3}\d{1,3}(?::\d{1,5})?\b";

const WIDE_CHAR_SPACERS: Flags =
    Flags::from_bits(Flags::LEADING_WIDE_CHAR_SPACER.bits() | Flags::WIDE_CHAR_SPACER.bits())
        .unwrap();

const MAX_HINTS: usize = 500;

/// The source of a hint, used for icons and labels in the picker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintKind {
    Url,
    Path,
    GitHash,
    Uuid,
    Ip,
    Custom,
}

/// A single hint match on the visible screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HintMatch {
    /// Text to open, copy, paste, or select.
    pub text: String,
    /// Inclusive grid range covering the match.
    pub range: Range,
    /// Action configured for this hint.
    pub action: HintAction,
    /// What kind of match this is.
    pub kind: HintKind,
    /// Optional rule label, for custom hints.
    pub label: Option<String>,
}

/// A visible line plus a byte-offset to grid-column mapping. Regex matches are
/// byte ranges, while terminal selections use inclusive grid points.
struct LineText {
    text: String,
    columns: Vec<(usize, usize)>,
    end_column: usize,
}

impl LineText {
    fn new<T: EventListener>(term: &Term<T>, line: Line) -> Self {
        let grid = term.grid();
        let mut text = String::new();
        let mut columns = Vec::new();

        for col in 0..grid.columns() {
            let cell = &grid[line][Column(col)];
            // A wide character is stored in the first cell; the following
            // spacer cell must not add a second copy of the character.
            if cell.flags.intersects(WIDE_CHAR_SPACERS) {
                continue;
            }

            columns.push((text.len(), col));
            text.push(cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                for c in zerowidth {
                    columns.push((text.len(), col));
                    text.push(*c);
                }
            }
        }

        Self {
            text,
            columns,
            end_column: grid.columns(),
        }
    }

    fn column_at_or_after(&self, byte: usize) -> usize {
        match self
            .columns
            .binary_search_by_key(&byte, |(offset, _)| *offset)
        {
            Ok(index) => self.columns[index].1,
            Err(index) => self
                .columns
                .get(index)
                .map(|(_, column)| *column)
                .unwrap_or(self.end_column),
        }
    }

    /// The column of the last character that starts before `byte`. `byte` is
    /// an exclusive end offset, so it may fall in the middle of a multi-byte
    /// character; only character starts participate in the mapping.
    fn column_before(&self, byte: usize) -> usize {
        let index = self.columns.partition_point(|(offset, _)| *offset < byte);
        if index == 0 {
            self.columns.first().map(|(_, column)| *column).unwrap_or(0)
        } else {
            self.columns[index - 1].1
        }
    }

    fn range(&self, line: i32, start: usize, end: usize) -> Option<Range> {
        if start >= end || start >= self.text.len() {
            return None;
        }

        let start_column = self.column_at_or_after(start);
        let end_column = self.column_before(end).max(start_column);
        Some(Range::new(
            Point::new(line, start_column),
            Point::new(line, end_column),
        ))
    }
}

fn ranges_overlap(a: Range, b: Range) -> bool {
    a.start().line == b.start().line
        && a.start().column <= b.end().column
        && b.start().column <= a.end().column
}

fn push_hint(
    hints: &mut Vec<HintMatch>,
    text: impl Into<String>,
    range: Range,
    action: HintAction,
    kind: HintKind,
    label: Option<String>,
) {
    let text = text.into();
    if text.is_empty()
        || text.len() > 4096
        || hints.len() >= MAX_HINTS
        || hints
            .iter()
            .any(|existing| ranges_overlap(existing.range, range))
    {
        return;
    }

    hints.push(HintMatch {
        text,
        range,
        action,
        kind,
        label,
    });
}

fn trim_url_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let Some(c) = text[start..end].chars().next_back() else {
            break;
        };
        if matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '\'' | '"'
        ) {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// Collect hints visible in the current viewport. Built-in matchers run
/// first; user rules can add to them, and any later match overlapping an
/// existing hint is skipped so one piece of text is not offered twice.
pub(crate) fn find_visible_hints<T: EventListener>(
    term: &Term<T>,
    custom_rules: &[HintRule],
    path_hyperlink_regexes: &[String],
) -> Vec<HintMatch> {
    let grid = term.grid();
    let display_offset = grid.display_offset() as i32;
    let first_line = -display_offset;
    let last_line = grid.screen_lines() as i32 - 1 - display_offset;
    if last_line < first_line {
        return Vec::new();
    }

    let url_regex = Regex::new(URL_REGEX).ok();
    let uuid_regex = Regex::new(UUID_REGEX).ok();
    let ipv4_regex = Regex::new(IPV4_REGEX).ok();
    let git_hash_regex = Regex::new(GIT_HASH_REGEX).ok();
    let path_regexes: Vec<Regex> = path_hyperlink_regexes
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect();

    let mut hints = Vec::new();
    let mut seen_osc8 = HashSet::new();

    for line_ix in first_line..=last_line {
        let line = Line(line_ix);

        // OSC 8 hyperlinks take precedence over regex matches on the same
        // cells. Offer each distinct URI once.
        for col in 0..grid.columns() {
            let Some(link) = grid[line][Column(col)].hyperlink() else {
                continue;
            };
            let uri = link.uri().to_owned();
            if !seen_osc8.insert(uri.clone()) {
                continue;
            }

            let mut end_col = col;
            while end_col + 1 < grid.columns() {
                let Some(next) = grid[line][Column(end_col + 1)].hyperlink() else {
                    break;
                };
                if next.uri() != uri {
                    break;
                }
                end_col += 1;
            }

            push_hint(
                &mut hints,
                uri,
                Range::new(Point::new(line_ix, col), Point::new(line_ix, end_col)),
                HintAction::Open,
                HintKind::Url,
                None,
            );
        }

        let line_text = LineText::new(term, line);
        if line_text.text.is_empty() {
            continue;
        }
        let text = &line_text.text;

        if let Some(regex) = &url_regex {
            for m in regex.find_iter(text) {
                let end = trim_url_end(text, m.start(), m.end());
                if end <= m.start() {
                    continue;
                }
                if let Some(range) = line_text.range(line_ix, m.start(), end) {
                    push_hint(
                        &mut hints,
                        &text[m.start()..end],
                        range,
                        HintAction::Open,
                        HintKind::Url,
                        None,
                    );
                }
            }
        }

        for regex in &path_regexes {
            for captures in regex.captures_iter(text) {
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                let link = captures.name("link").unwrap_or(whole);
                let mut path = captures
                    .name("path")
                    .map(|capture| capture.as_str().to_owned())
                    .unwrap_or_else(|| link.as_str().to_owned());

                if let Some(line) = captures
                    .name("line")
                    .and_then(|capture| capture.as_str().parse::<u32>().ok())
                {
                    path.push_str(&format!(":{line}"));
                    if let Some(column) = captures
                        .name("column")
                        .and_then(|capture| capture.as_str().parse::<u32>().ok())
                    {
                        path.push_str(&format!(":{column}"));
                    }
                }

                if let Some(range) = line_text.range(line_ix, link.start(), link.end()) {
                    push_hint(
                        &mut hints,
                        path,
                        range,
                        HintAction::Open,
                        HintKind::Path,
                        None,
                    );
                }
            }
        }

        for rule in custom_rules {
            let Some(regex) = &rule.compiled else {
                continue;
            };
            for m in regex.find_iter(text) {
                if let Some(range) = line_text.range(line_ix, m.start(), m.end()) {
                    push_hint(
                        &mut hints,
                        m.as_str(),
                        range,
                        rule.action,
                        HintKind::Custom,
                        rule.name.clone(),
                    );
                }
            }
        }

        if let Some(regex) = &uuid_regex {
            for m in regex.find_iter(text) {
                if let Some(range) = line_text.range(line_ix, m.start(), m.end()) {
                    push_hint(
                        &mut hints,
                        m.as_str(),
                        range,
                        HintAction::Copy,
                        HintKind::Uuid,
                        None,
                    );
                }
            }
        }

        if let Some(regex) = &ipv4_regex {
            for m in regex.find_iter(text) {
                let address = m.as_str().split(':').next().unwrap_or_default();
                let octets: Vec<_> = address.split('.').collect();
                if octets.len() != 4 || octets.iter().any(|octet| octet.parse::<u8>().is_err()) {
                    continue;
                }
                if let Some(range) = line_text.range(line_ix, m.start(), m.end()) {
                    push_hint(
                        &mut hints,
                        m.as_str(),
                        range,
                        HintAction::Copy,
                        HintKind::Ip,
                        None,
                    );
                }
            }
        }

        if let Some(regex) = &git_hash_regex {
            for m in regex.find_iter(text) {
                // Require at least one hex letter so plain decimal numbers are
                // not offered as commit hashes.
                if !m.as_str().chars().any(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                if let Some(range) = line_text.range(line_ix, m.start(), m.end()) {
                    push_hint(
                        &mut hints,
                        m.as_str(),
                        range,
                        HintAction::Copy,
                        HintKind::GitHash,
                        None,
                    );
                }
            }
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_text_maps_wide_characters_to_columns() {
        let mut line = LineText {
            text: String::new(),
            columns: Vec::new(),
            end_column: 6,
        };
        line.columns.push((0, 0));
        line.text.push('a');
        line.columns.push((1, 1));
        line.text.push('界');
        line.columns.push((4, 3));
        line.text.push('b');

        assert_eq!(line.column_at_or_after(0), 0);
        assert_eq!(line.column_at_or_after(1), 1);
        assert_eq!(line.column_at_or_after(2), 3);
        assert_eq!(line.column_before(3), 1);
        assert_eq!(line.column_before(4), 1);
        assert_eq!(line.column_before(5), 3);

        let range = line.range(0, 0, 4).unwrap();
        assert_eq!(range.start(), Point::new(0, 0));
        assert_eq!(range.end(), Point::new(0, 1));
    }

    #[test]
    fn range_is_inclusive_of_the_last_matched_cell() {
        let line = LineText {
            text: "abcde".to_owned(),
            columns: vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            end_column: 5,
        };
        let range = line.range(7, 1, 4).unwrap();
        assert_eq!(range.start(), Point::new(7, 1));
        assert_eq!(range.end(), Point::new(7, 3));
    }

    #[test]
    fn url_trimming_removes_prose_punctuation() {
        let text = "see https://example.com/foo).";
        let start = text.find("https").unwrap();
        let end = trim_url_end(text, start, text.len());
        assert_eq!(&text[start..end], "https://example.com/foo");
    }
}
