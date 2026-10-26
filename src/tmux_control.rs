//! tmux control mode (`tmux -CC`) protocol core.
//!
//! Terry currently attaches tmux sessions as a raw PTY, which means the whole
//! tmux UI is drawn inside one terminal. Native integration requires speaking
//! tmux's *control mode* protocol: tmux reports windows, panes, layouts and
//! pane output as line-oriented events, and accepts commands on stdin.
//!
//! This module implements that protocol as a standalone, testable foundation:
//!
//! * [`parse_line`] turns a control-mode line into a [`ControlEvent`].
//! * [`unescape_octal`] decodes the `%output` payload escaping.
//! * [`parse_layout`] turns a tmux layout string into a [`Layout`] tree.
//! * [`ControlClient`] spawns `tmux -CC`, writes commands and reads events.
//! * Helpers build the commands used to forward input and size changes.
//!
//! The UI wiring (mapping panes onto native panes) is intentionally not part of
//! this module yet, so nothing here is reachable from the app UI.
#![allow(dead_code)]

use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

/// A tmux pane id, rendered in commands as `%N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// A tmux window id, rendered in commands as `@N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

/// A tmux session id, rendered in commands as `$N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u32);

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${}", self.0)
    }
}

/// Split direction used by a tmux layout container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// Children are laid out side by side.
    LeftRight,
    /// Children are stacked vertically.
    TopBottom,
}

/// A parsed tmux layout tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Leaf {
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        /// Pane id when the dumped layout includes one.
        pane: Option<u32>,
    },
    Split {
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        orientation: Orientation,
        children: Vec<Layout>,
    },
}

/// One event emitted by the control client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    /// Pane output; the payload has already been unescaped.
    Output {
        pane: PaneId,
        data: Vec<u8>,
    },
    Begin {
        number: u64,
        flags: u32,
    },
    End {
        number: u64,
        flags: u32,
    },
    Error {
        number: u64,
        flags: u32,
    },
    /// A line of command output, emitted between `%begin` and `%end`.
    Response(String),
    WindowAdd(WindowId),
    WindowClose(WindowId),
    WindowRenamed {
        window: WindowId,
        name: String,
    },
    LayoutChange {
        window: WindowId,
        layout: Layout,
    },
    SessionChanged {
        session: SessionId,
        name: String,
    },
    /// tmux exited; the payload is the optional reason.
    Exit(Option<String>),
    /// A `%` line this module does not model yet, kept verbatim.
    Other(String),
}

fn parse_id<T>(token: &str, prefix: char, wrap: impl Fn(u32) -> T) -> Option<T> {
    let rest = token.strip_prefix(prefix)?;
    rest.parse::<u32>().ok().map(wrap)
}

/// Decodes the `\ooo` octal escaping used by `%output`.
pub fn unescape_octal(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\\' && index + 1 < bytes.len() {
            let next = bytes[index + 1];
            if next.is_ascii_digit() && index + 3 < bytes.len() {
                let digits = &input[index + 1..index + 4];
                if let Ok(value) = u8::from_str_radix(digits, 8) {
                    out.push(value);
                    index += 4;
                    continue;
                }
            }
            if next == b'\\' {
                out.push(b'\\');
                index += 2;
                continue;
            }
        }
        out.push(byte);
        index += 1;
    }
    out
}

/// Parses a single control-mode line.
///
/// Lines that are not `%`-prefixed are treated as command output responses.
pub fn parse_line(line: &str) -> ControlEvent {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);

    let Some(rest) = line.strip_prefix('%') else {
        return ControlEvent::Response(line.to_string());
    };

    let (keyword, args) = match rest.split_once(' ') {
        Some((keyword, args)) => (keyword, args),
        None => (rest, ""),
    };

    match keyword {
        "output" => {
            let (pane, payload) = match args.split_once(' ') {
                Some((pane, payload)) => (pane, payload),
                None => (args, ""),
            };
            match parse_id(pane, '%', PaneId) {
                Some(pane) => ControlEvent::Output {
                    pane,
                    data: unescape_octal(payload),
                },
                None => ControlEvent::Other(line.to_string()),
            }
        }
        "begin" | "end" | "error" => {
            let mut parts = args.split(' ');
            // Format: `<timestamp> <command number> <flags>`.
            let _timestamp = parts.next();
            let number = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let flags = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            match keyword {
                "begin" => ControlEvent::Begin { number, flags },
                "end" => ControlEvent::End { number, flags },
                _ => ControlEvent::Error { number, flags },
            }
        }
        "window-add" => match parse_id(args, '@', WindowId) {
            Some(window) => ControlEvent::WindowAdd(window),
            None => ControlEvent::Other(line.to_string()),
        },
        "window-close" => match parse_id(args, '@', WindowId) {
            Some(window) => ControlEvent::WindowClose(window),
            None => ControlEvent::Other(line.to_string()),
        },
        "window-renamed" => match args.split_once(' ') {
            Some((window, name)) => match parse_id(window, '@', WindowId) {
                Some(window) => ControlEvent::WindowRenamed {
                    window,
                    name: name.to_string(),
                },
                None => ControlEvent::Other(line.to_string()),
            },
            None => ControlEvent::Other(line.to_string()),
        },
        "layout-change" => match args.split_once(' ') {
            Some((window, rest)) => match parse_id(window, '@', WindowId) {
                Some(window) => match rest.split_once(' ').map(|(layout, _)| layout) {
                    Some(layout) => match parse_layout(layout) {
                        Some(layout) => ControlEvent::LayoutChange { window, layout },
                        None => ControlEvent::Other(line.to_string()),
                    },
                    None => ControlEvent::Other(line.to_string()),
                },
                None => ControlEvent::Other(line.to_string()),
            },
            None => ControlEvent::Other(line.to_string()),
        },
        "session-changed" => match args.split_once(' ') {
            Some((session, name)) => match parse_id(session, '$', SessionId) {
                Some(session) => ControlEvent::SessionChanged {
                    session,
                    name: name.to_string(),
                },
                None => ControlEvent::Other(line.to_string()),
            },
            None => ControlEvent::Other(line.to_string()),
        },
        "exit" => {
            let reason = args.trim();
            ControlEvent::Exit((!reason.is_empty()).then(|| reason.to_string()))
        }
        _ => ControlEvent::Other(line.to_string()),
    }
}

struct Cursor<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn parse_number(&mut self) -> Option<u32> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return None;
        }
        self.input[start..self.pos].parse().ok()
    }

    fn parse_dims(&mut self) -> Option<(u32, u32)> {
        let width = self.parse_number()?;
        if !self.eat('x') {
            return None;
        }
        let height = self.parse_number()?;
        Some((width, height))
    }
}

/// Parses a tmux layout string such as
/// `b25d,80x24,0,0{40x24,0,0,0,39x24,41,0,1}`.
pub fn parse_layout(input: &str) -> Option<Layout> {
    let (_checksum, rest) = input.split_once(',')?;
    let mut cursor = Cursor::new(rest.trim());
    let layout = parse_layout_node(&mut cursor)?;
    Some(layout)
}

fn parse_layout_node(cursor: &mut Cursor<'_>) -> Option<Layout> {
    let (width, height) = cursor.parse_dims()?;
    if !cursor.eat(',') {
        return None;
    }
    let x = cursor.parse_number()?;
    if !cursor.eat(',') {
        return None;
    }
    let y = cursor.parse_number()?;

    // tmux encodes the split direction in the brackets: `[...]` lays children
    // out side by side, `{...}` stacks them. Children are comma separated.
    let (orientation, close) = match cursor.peek() {
        Some('[') => (Orientation::LeftRight, ']'),
        Some('{') => (Orientation::TopBottom, '}'),
        _ => {
            // A leaf may carry its pane id after the geometry: `WxH,X,Y,PANE`.
            let pane = if cursor.eat(',') {
                cursor.parse_number()
            } else {
                None
            };
            return Some(Layout::Leaf {
                width,
                height,
                x,
                y,
                pane,
            });
        }
    };

    cursor.pos += 1; // opening bracket
    let mut children = Vec::new();
    loop {
        children.push(parse_layout_node(cursor)?);
        if cursor.eat(close) {
            break;
        }
        // Accept both separators for robustness; orientation comes from the
        // brackets, not the separator.
        if !cursor.eat(',') && !cursor.eat('-') {
            return None;
        }
    }

    Some(Layout::Split {
        width,
        height,
        x,
        y,
        orientation,
        children,
    })
}

/// Quotes a single argument for a control-mode command.
pub fn quote_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '\'' | '"' | ';' | '$' | '\\'));
    if !needs_quotes {
        return arg.to_string();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Command that forwards literal text to a pane.
pub fn send_keys_literal(pane: PaneId, text: &str) -> String {
    format!("send-keys -t {} -l -- {}\n", pane, quote_arg(text))
}

/// Command that forwards a single named key (for example `Enter`, `C-c`).
pub fn send_key(pane: PaneId, key: &str) -> String {
    format!("send-keys -t {} -- {}\n", pane, quote_arg(key))
}

/// Command that tells tmux the client's viewport size.
pub fn refresh_client_size(columns: u16, rows: u16) -> String {
    format!("refresh-client -C {}x{}\n", columns, rows)
}

/// Command listing panes with the fields the UI needs.
pub const LIST_PANES: &str = "list-panes -a -F '#{session_id} #{window_id} #{pane_id} #{pane_width}x#{pane_height} #{pane_current_path} #{pane_title}'\n";

/// A running `tmux -CC` client.
pub struct ControlClient {
    child: Child,
    stdin: ChildStdin,
}

impl ControlClient {
    /// Attaches to an existing session in control mode, creating it if needed.
    pub fn attach(target: &str) -> io::Result<Self> {
        let mut child = Command::new("tmux")
            .arg("-CC")
            .arg("attach-session")
            .arg("-t")
            .arg(target)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("tmux control mode: no stdin"))?;
        Ok(Self { child, stdin })
    }

    /// Takes the stdout reader, which yields control-mode lines.
    pub fn take_reader(&mut self) -> Option<BufReader<std::process::ChildStdout>> {
        self.child.stdout.take().map(BufReader::new)
    }

    /// Sends a raw command. The caller is responsible for the trailing newline.
    pub fn send(&mut self, command: &str) -> io::Result<()> {
        self.stdin.write_all(command.as_bytes())?;
        self.stdin.flush()
    }

    /// Reads the next event, or `None` at end of stream.
    pub fn next_event(&mut self, reader: &mut impl BufRead) -> io::Result<Option<ControlEvent>> {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(parse_line(&line)))
    }

    /// Kills the client and reaps the child process.
    pub fn shutdown(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_octal_decodes_control_bytes() {
        assert_eq!(unescape_octal(r"a\033[31mred"), b"a\x1b[31mred");
        assert_eq!(unescape_octal(r"back\\slash"), b"back\\slash");
        assert_eq!(unescape_octal("plain"), b"plain");
        // A truncated escape is passed through unchanged.
        assert_eq!(unescape_octal(r"tail\03"), b"tail\\03");
    }

    #[test]
    fn parses_pane_output() {
        let event = parse_line("%output %3 hello\\040world\r\n");
        assert_eq!(
            event,
            ControlEvent::Output {
                pane: PaneId(3),
                data: b"hello world".to_vec(),
            }
        );
    }

    #[test]
    fn parses_command_framing() {
        assert_eq!(
            parse_line("%begin 1700000000 12 0\n"),
            ControlEvent::Begin {
                number: 12,
                flags: 0
            }
        );
        assert_eq!(
            parse_line("%end 1700000000 12 0\n"),
            ControlEvent::End {
                number: 12,
                flags: 0
            }
        );
        assert_eq!(
            parse_line("%error 1700000000 13 0\n"),
            ControlEvent::Error {
                number: 13,
                flags: 0
            }
        );
        assert_eq!(
            parse_line("80x24\n"),
            ControlEvent::Response("80x24".to_string())
        );
    }

    #[test]
    fn parses_window_and_session_events() {
        assert_eq!(
            parse_line("%window-add @4"),
            ControlEvent::WindowAdd(WindowId(4))
        );
        assert_eq!(
            parse_line("%window-close @4"),
            ControlEvent::WindowClose(WindowId(4))
        );
        assert_eq!(
            parse_line("%window-renamed @2 editor"),
            ControlEvent::WindowRenamed {
                window: WindowId(2),
                name: "editor".to_string()
            }
        );
        assert_eq!(
            parse_line("%session-changed $1 work"),
            ControlEvent::SessionChanged {
                session: SessionId(1),
                name: "work".to_string()
            }
        );
        assert_eq!(parse_line("%exit"), ControlEvent::Exit(None));
        assert_eq!(
            parse_line("%exit server exited"),
            ControlEvent::Exit(Some("server exited".to_string()))
        );
    }

    #[test]
    fn parses_layout_change_event() {
        let event = parse_line(
            "%layout-change @0 b25d,80x24,0,0[40x24,0,0,0,39x24,41,0,1] \
             80x24,0,0[40x24,0,0,0,39x24,41,0,1] *",
        );
        let ControlEvent::LayoutChange { window, layout } = event else {
            panic!("expected a layout change, got {event:?}");
        };
        assert_eq!(window, WindowId(0));
        let Layout::Split {
            orientation,
            children,
            ..
        } = layout
        else {
            panic!("expected a split layout");
        };
        assert_eq!(orientation, Orientation::LeftRight);
        assert_eq!(children.len(), 2);
        assert_eq!(
            children[0],
            Layout::Leaf {
                width: 40,
                height: 24,
                x: 0,
                y: 0,
                pane: Some(0),
            }
        );
        assert_eq!(
            children[1],
            Layout::Leaf {
                width: 39,
                height: 24,
                x: 41,
                y: 0,
                pane: Some(1),
            }
        );
    }

    #[test]
    fn parses_single_leaf_layout() {
        assert_eq!(
            parse_layout("b25d,80x24,0,0,7"),
            Some(Layout::Leaf {
                width: 80,
                height: 24,
                x: 0,
                y: 0,
                pane: Some(7),
            })
        );
    }

    #[test]
    fn parses_nested_top_bottom_layout() {
        // Two stacked rows ({{...}}), the first row split into two columns.
        let layout =
            parse_layout("0000,80x24,0,0{80x12,0,0[40x12,0,0,0,39x12,41,0,1],80x11,0,13,2}")
                .expect("layout should parse");
        let Layout::Split {
            orientation,
            children,
            ..
        } = layout
        else {
            panic!("expected a split layout");
        };
        assert_eq!(orientation, Orientation::TopBottom);
        assert_eq!(children.len(), 2);
        let Layout::Split {
            orientation: inner,
            children: inner_children,
            ..
        } = &children[0]
        else {
            panic!("expected the first row to be split");
        };
        assert_eq!(*inner, Orientation::LeftRight);
        assert_eq!(inner_children.len(), 2);
        assert_eq!(
            children[1],
            Layout::Leaf {
                width: 80,
                height: 11,
                x: 0,
                y: 13,
                pane: Some(2),
            }
        );
    }

    #[test]
    fn quotes_arguments_for_commands() {
        assert_eq!(quote_arg("simple"), "simple");
        assert_eq!(quote_arg("two words"), "'two words'");
        assert_eq!(quote_arg("it's"), "'it'\\''s'");
        assert_eq!(quote_arg(""), "''");
        assert_eq!(quote_arg("a$b"), "'a$b'");
    }

    #[test]
    fn builds_forwarding_commands() {
        assert_eq!(
            send_keys_literal(PaneId(1), "ls -la"),
            "send-keys -t %1 -l -- 'ls -la'\n"
        );
        assert_eq!(send_key(PaneId(2), "Enter"), "send-keys -t %2 -- Enter\n");
        assert_eq!(refresh_client_size(120, 40), "refresh-client -C 120x40\n");
    }
}
