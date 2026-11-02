//! Encodes named keys and modifier combinations into PTY bytes.
//!
//! A phone has no physical keyboard, so its client sends *intentions* —
//! `ctrl-c`, `escape`, `up`, `enter` — rather than text. The desktop GUI maps
//! gpui `Keystroke`s onto terminals; a client that is not the GUI (the daemon's
//! HTTP endpoint, a native iOS/Android app) has no `Keystroke` to hand, so this
//! module is the equivalent mapping over plain strings.
//!
//! Two encodings matter, and the terminal decides which one applies:
//!
//! - **Legacy**: what `xterm` and `TERM=xterm-256color` expect, e.g. `ESC [ A`
//!   for `up`.
//! - **Application cursor keys** (`DECCKM`, set by vim, less, fzf, every TUI):
//!   the arrows become `ESC O A`. Getting this wrong makes the arrow keys insert
//!   literal `^[[A` into the prompt of whatever full-screen program is running,
//!   which is the single most common "the mobile client is broken" bug.
//!
//! The kitty keyboard protocol is deliberately **not** implemented here: it is a
//! desktop-grade feature (press/repeat/release events, alternate keys) and no TUI
//! on a phone depends on it. `send_keys` over HTTP therefore behaves like a
//! plain `xterm`, which is what a soft keyboard produces anyway.

/// The terminal modes that change key encoding.
///
/// Kept as a plain struct rather than the GUI's `Modes` bitfield so this crate
/// stays free of `terminal`'s gpui dependency.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModes {
    /// `DECCKM`: the application set cursor keys (`ESC O A` instead of `ESC [ A`).
    pub application_cursor: bool,
}

/// A key to send, as a client names it.
///
/// `Ctrl` is expressed in the key name (`"ctrl-c"`), not as a separate field:
/// that is what a phone keyboard bar sends and what the JSON API accepts.
pub fn encode(name: &str, modes: KeyModes) -> Option<Vec<u8>> {
    let normalized = normalize(name);
    let (modifiers, key) = split_modifiers(&normalized);

    if let Some(bytes) = encode_special(key, modifiers, modes) {
        return Some(bytes);
    }
    if let Some(bytes) = encode_control(key, modifiers) {
        return Some(bytes);
    }
    // A single character with Alt is the usual `ESC <char>` (terminal meta).
    if modifiers.alt && !modifiers.ctrl && key.chars().count() == 1 {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(key.as_bytes());
        return Some(bytes);
    }
    None
}

/// Sends literal text, untouched.
///
/// Separate from [`encode`] so a client typing `y` cannot accidentally hit a
/// key mapping, and so newlines become carriage returns the way a terminal
/// expects from `Enter`.
pub fn encode_text(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
}

#[derive(Clone, Copy, Debug, Default)]
struct Modifiers {
    ctrl: bool,
    alt: bool,
    shift: bool,
}

fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Splits `ctrl-shift-a` into modifiers plus the key.
fn split_modifiers(name: &str) -> (Modifiers, &str) {
    let mut modifiers = Modifiers::default();
    let mut rest = name;

    // Accept both `ctrl-x` and `c-x`, and either order, since clients differ.
    loop {
        let (flag, prefix) = if let Some(rest) = strip_prefix(rest, "ctrl-") {
            (&mut modifiers.ctrl, rest)
        } else if let Some(rest) = strip_prefix(rest, "control-") {
            (&mut modifiers.ctrl, rest)
        } else if let Some(rest) = strip_prefix(rest, "alt-") {
            (&mut modifiers.alt, rest)
        } else if let Some(rest) = strip_prefix(rest, "meta-") {
            (&mut modifiers.alt, rest)
        } else if let Some(rest) = strip_prefix(rest, "shift-") {
            (&mut modifiers.shift, rest)
        } else if let Some(rest) = strip_prefix(rest, "s-") {
            (&mut modifiers.shift, rest)
        } else if let Some(rest) = strip_prefix(rest, "c-") {
            (&mut modifiers.ctrl, rest)
        } else if let Some(rest) = strip_prefix(rest, "m-") {
            (&mut modifiers.alt, rest)
        } else {
            break;
        };
        *flag = true;
        rest = prefix;
    }

    (modifiers, rest)
}

fn strip_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value.strip_prefix(prefix)
}

fn encode_special(key: &str, modifiers: Modifiers, modes: KeyModes) -> Option<Vec<u8>> {
    // Modified special keys use the CSI `1;<mod>` form, where the modifier is
    // 1 + (shift:1 | alt:2 | ctrl:4). This is the xterm convention.
    let csi_modifier = 1 + (modifiers.shift as u8)
        + (modifiers.alt as u8) * 2
        + (modifiers.ctrl as u8) * 4;

    /// Arrows: SS3 (`ESC O A`) when the application set cursor keys and no
    /// modifier is held, CSI (`ESC [ A`) otherwise.
    fn arrow(code: char, modifiers: Modifiers, modes: KeyModes) -> Vec<u8> {
        if modifiers.ctrl || modifiers.alt || modifiers.shift {
            let csi_modifier = 1 + (modifiers.shift as u8)
                + (modifiers.alt as u8) * 2
                + (modifiers.ctrl as u8) * 4;
            format!("\x1b[1;{csi_modifier}{code}").into_bytes()
        } else if modes.application_cursor {
            format!("\x1bO{code}").into_bytes()
        } else {
            format!("\x1b[{code}").into_bytes()
        }
    }

    let bytes = match key {
        "escape" | "esc" => {
            if has_any(modifiers) {
                return None;
            }
            b"\x1b".to_vec()
        }
        "enter" | "return" | "cr" => {
            if modifiers.shift {
                b"\x0a".to_vec()
            } else if modifiers.alt {
                b"\x1b\x0d".to_vec()
            } else {
                b"\x0d".to_vec()
            }
        }
        "tab" => {
            if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\x09".to_vec()
            }
        }
        "backspace" | "bs" => match (modifiers.ctrl, modifiers.alt) {
            (true, _) => b"\x08".to_vec(),
            (_, true) => b"\x1b\x7f".to_vec(),
            _ => b"\x7f".to_vec(),
        },
        "space" => {
            if modifiers.ctrl {
                b"\x00".to_vec()
            } else {
                b" ".to_vec()
            }
        }
        "up" => arrow('A', modifiers, modes),
        "down" => arrow('B', modifiers, modes),
        "right" => arrow('C', modifiers, modes),
        "left" => arrow('D', modifiers, modes),
        "home" => {
            if modes.application_cursor && !has_any(modifiers) {
                b"\x1bOH".to_vec()
            } else {
                b"\x1b[H".to_vec()
            }
        }
        "end" => {
            if modes.application_cursor && !has_any(modifiers) {
                b"\x1bOF".to_vec()
            } else {
                b"\x1b[F".to_vec()
            }
        }
        "insert" => b"\x1b[2~".to_vec(),
        "delete" | "del" => b"\x1b[3~".to_vec(),
        "pageup" | "pgup" => b"\x1b[5~".to_vec(),
        "pagedown" | "pgdn" => b"\x1b[6~".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => b"\x1b[15~".to_vec(),
        "f6" => b"\x1b[17~".to_vec(),
        "f7" => b"\x1b[18~".to_vec(),
        "f8" => b"\x1b[19~".to_vec(),
        "f9" => b"\x1b[20~".to_vec(),
        "f10" => b"\x1b[21~".to_vec(),
        "f11" => b"\x1b[23~".to_vec(),
        "f12" => b"\x1b[24~".to_vec(),
        _ => return None,
    };
    let _ = csi_modifier;
    Some(bytes)
}

fn has_any(modifiers: Modifiers) -> bool {
    modifiers.ctrl || modifiers.alt || modifiers.shift
}

/// `Ctrl` + a letter/digit/symbol, as the single control byte a terminal wants.
fn encode_control(key: &str, modifiers: Modifiers) -> Option<Vec<u8>> {
    if !modifiers.ctrl {
        return None;
    }
    // `ctrl-c` interrupting a build is the single most used key on a phone, so
    // this path has to be right even for keys that have no legacy escape form.
    let mut chars = key.chars();
    let character = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    let byte = match character {
        'a'..='z' => character as u8 - b'a' + 1,
        'A'..='Z' => character as u8 - b'A' + 1,
        '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(vec![byte])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_str(name: &str) -> String {
        String::from_utf8(encode(name, KeyModes::default()).expect(name)).expect("utf8")
    }

    #[test]
    fn control_letters_become_control_bytes() {
        assert_eq!(encode_str("ctrl-c"), "\x03");
        assert_eq!(encode_str("ctrl-d"), "\x04");
        assert_eq!(encode_str("ctrl-l"), "\x0c");
        assert_eq!(encode_str("ctrl-z"), "\x1a");
        // Uppercase and the `c-` shorthand are accepted too.
        assert_eq!(encode_str("Ctrl-C"), "\x03");
        assert_eq!(encode_str("c-c"), "\x03");
    }

    #[test]
    fn the_common_phone_bar_keys_are_all_encodable() {
        // This is the key bar the PWA and the native clients render.
        for key in [
            "escape", "tab", "enter", "backspace", "up", "down", "left", "right", "home", "end",
            "pageup", "pagedown", "ctrl-c", "ctrl-d", "ctrl-z", "ctrl-l",
        ] {
            assert!(
                encode(key, KeyModes::default()).is_some(),
                "{key} must be encodable; a phone has no other way to send it"
            );
        }
    }

    #[test]
    fn arrows_follow_the_application_cursor_mode() {
        let legacy = KeyModes::default();
        let application = KeyModes {
            application_cursor: true,
        };

        assert_eq!(
            encode("up", legacy).unwrap(),
            b"\x1b[A",
            "legacy encoding"
        );
        assert_eq!(
            encode("up", application).unwrap(),
            b"\x1bOA",
            "vim/less/fzf set DECCKM and expect ESC O A; sending ESC [ A types ^[[A"
        );
    }

    #[test]
    fn modified_arrows_keep_the_csi_form() {
        // Ctrl+Left is word-jump in readline; it must not become an SS3 sequence
        // just because the application set cursor keys.
        let modes = KeyModes {
            application_cursor: true,
        };
        assert_eq!(encode("ctrl-left", modes).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode("shift-up", modes).unwrap(), b"\x1b[1;2A");
    }

    #[test]
    fn enter_and_shift_enter_differ() {
        assert_eq!(encode("enter", KeyModes::default()).unwrap(), b"\x0d");
        // Shift+Enter is a newline, which is what agent TUIs use for multi-line.
        assert_eq!(encode("shift-enter", KeyModes::default()).unwrap(), b"\x0a");
    }

    #[test]
    fn alt_and_a_character_is_a_meta_prefix() {
        assert_eq!(encode("alt-b", KeyModes::default()).unwrap(), b"\x1bb");
    }

    #[test]
    fn text_is_sent_verbatim_except_newlines() {
        assert_eq!(encode_text("y"), b"y");
        assert_eq!(
            encode_text("hello\nworld\n"),
            b"hello\rworld\r",
            "a terminal expects CR from Enter"
        );
        assert_eq!(encode_text("a\r\nb"), b"a\rb", "CRLF must not double up");
    }

    #[test]
    fn unknown_keys_are_refused_rather_than_guessed() {
        assert!(encode("not-a-key", KeyModes::default()).is_none());
        // A bare multi-character word is text, not a key: the client must use
        // send_text for that, so encoding it as a key would be a mistake.
        assert!(encode("hello", KeyModes::default()).is_none());
    }
}
