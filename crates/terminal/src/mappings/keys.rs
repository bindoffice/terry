use std::borrow::Cow;

/// The mappings defined in this file where created from reading the alacritty source
use gpui::Keystroke;

use crate::{KeyEventType, Modes};

#[derive(Debug, PartialEq, Eq)]
enum TerminalModifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl TerminalModifiers {
    fn new(ks: &Keystroke) -> Self {
        match (
            ks.modifiers.alt,
            ks.modifiers.control,
            ks.modifiers.shift,
            ks.modifiers.platform,
        ) {
            (false, false, false, false) => TerminalModifiers::None,
            (true, false, false, false) => TerminalModifiers::Alt,
            (false, true, false, false) => TerminalModifiers::Ctrl,
            (false, false, true, false) => TerminalModifiers::Shift,
            (false, true, true, false) => TerminalModifiers::CtrlShift,
            _ => TerminalModifiers::Other,
        }
    }

    fn any(&self) -> bool {
        match &self {
            TerminalModifiers::None => false,
            TerminalModifiers::Alt => true,
            TerminalModifiers::Ctrl => true,
            TerminalModifiers::Shift => true,
            TerminalModifiers::CtrlShift => true,
            TerminalModifiers::Other => true,
        }
    }
}

pub(crate) fn to_esc_str(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    to_esc_str_for_event(keystroke, mode, option_as_meta, KeyEventType::Press)
}

pub(crate) fn to_esc_str_for_event(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
    event_type: KeyEventType,
) -> Option<Cow<'static, str>> {
    // Legacy key events have no release form. Only report releases when the
    // application explicitly asked for event types.
    if event_type == KeyEventType::Release && !mode.contains(Modes::KITTY_REPORT_EVENT_TYPES) {
        return None;
    }

    // When the application has enabled any kitty keyboard enhancement, encode
    // keystrokes according to the flags it requested.
    if mode.intersects(Modes::KITTY_KEYBOARD_PROTOCOL) {
        return kitty_esc_str(keystroke, mode, option_as_meta, event_type);
    }

    legacy_esc_str(keystroke, mode, option_as_meta)
}

fn legacy_esc_str(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = TerminalModifiers::new(keystroke);

    // Manual Bindings including modifiers
    let manual_esc_str: Option<&'static str> = match (keystroke.key.as_ref(), &modifiers) {
        //Basic special keys
        ("tab", TerminalModifiers::None) => Some("\x09"),
        ("escape", TerminalModifiers::None) => Some("\x1b"),
        ("enter", TerminalModifiers::None) => Some("\x0d"),
        ("enter", TerminalModifiers::Shift) => Some("\x0a"),
        ("enter", TerminalModifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", TerminalModifiers::None) => Some("\x7f"),
        //Interesting escape codes
        ("tab", TerminalModifiers::Shift) => Some("\x1b[Z"),
        ("backspace", TerminalModifiers::Ctrl) => Some("\x08"),
        ("backspace", TerminalModifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", TerminalModifiers::Shift) => Some("\x7f"),
        ("space", TerminalModifiers::Ctrl) => Some("\x00"),
        ("home", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOH"),
        ("home", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[H"),
        ("end", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOF"),
        ("end", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[F"),
        ("up", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOA"),
        ("up", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[A"),
        ("down", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOB"),
        ("down", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[B"),
        ("right", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOC"),
        ("right", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[C"),
        ("left", TerminalModifiers::None) if mode.contains(Modes::APP_CURSOR) => Some("\x1bOD"),
        ("left", TerminalModifiers::None) if !mode.contains(Modes::APP_CURSOR) => Some("\x1b[D"),
        ("back", TerminalModifiers::None) => Some("\x7f"),
        ("insert", TerminalModifiers::None) => Some("\x1b[2~"),
        ("delete", TerminalModifiers::None) => Some("\x1b[3~"),
        ("pageup", TerminalModifiers::None) => Some("\x1b[5~"),
        ("pagedown", TerminalModifiers::None) => Some("\x1b[6~"),
        ("f1", TerminalModifiers::None) => Some("\x1bOP"),
        ("f2", TerminalModifiers::None) => Some("\x1bOQ"),
        ("f3", TerminalModifiers::None) => Some("\x1bOR"),
        ("f4", TerminalModifiers::None) => Some("\x1bOS"),
        ("f5", TerminalModifiers::None) => Some("\x1b[15~"),
        ("f6", TerminalModifiers::None) => Some("\x1b[17~"),
        ("f7", TerminalModifiers::None) => Some("\x1b[18~"),
        ("f8", TerminalModifiers::None) => Some("\x1b[19~"),
        ("f9", TerminalModifiers::None) => Some("\x1b[20~"),
        ("f10", TerminalModifiers::None) => Some("\x1b[21~"),
        ("f11", TerminalModifiers::None) => Some("\x1b[23~"),
        ("f12", TerminalModifiers::None) => Some("\x1b[24~"),
        ("f13", TerminalModifiers::None) => Some("\x1b[25~"),
        ("f14", TerminalModifiers::None) => Some("\x1b[26~"),
        ("f15", TerminalModifiers::None) => Some("\x1b[28~"),
        ("f16", TerminalModifiers::None) => Some("\x1b[29~"),
        ("f17", TerminalModifiers::None) => Some("\x1b[31~"),
        ("f18", TerminalModifiers::None) => Some("\x1b[32~"),
        ("f19", TerminalModifiers::None) => Some("\x1b[33~"),
        ("f20", TerminalModifiers::None) => Some("\x1b[34~"),
        // NumpadEnter, Action::Esc("\n".into());
        //Mappings for caret notation keys
        ("a", TerminalModifiers::Ctrl) => Some("\x01"), //1
        ("A", TerminalModifiers::CtrlShift) => Some("\x01"), //1
        ("b", TerminalModifiers::Ctrl) => Some("\x02"), //2
        ("B", TerminalModifiers::CtrlShift) => Some("\x02"), //2
        ("c", TerminalModifiers::Ctrl) => Some("\x03"), //3
        ("C", TerminalModifiers::CtrlShift) => Some("\x03"), //3
        ("d", TerminalModifiers::Ctrl) => Some("\x04"), //4
        ("D", TerminalModifiers::CtrlShift) => Some("\x04"), //4
        ("e", TerminalModifiers::Ctrl) => Some("\x05"), //5
        ("E", TerminalModifiers::CtrlShift) => Some("\x05"), //5
        ("f", TerminalModifiers::Ctrl) => Some("\x06"), //6
        ("F", TerminalModifiers::CtrlShift) => Some("\x06"), //6
        ("g", TerminalModifiers::Ctrl) => Some("\x07"), //7
        ("G", TerminalModifiers::CtrlShift) => Some("\x07"), //7
        ("h", TerminalModifiers::Ctrl) => Some("\x08"), //8
        ("H", TerminalModifiers::CtrlShift) => Some("\x08"), //8
        ("i", TerminalModifiers::Ctrl) => Some("\x09"), //9
        ("I", TerminalModifiers::CtrlShift) => Some("\x09"), //9
        ("j", TerminalModifiers::Ctrl) => Some("\x0a"), //10
        ("J", TerminalModifiers::CtrlShift) => Some("\x0a"), //10
        ("k", TerminalModifiers::Ctrl) => Some("\x0b"), //11
        ("K", TerminalModifiers::CtrlShift) => Some("\x0b"), //11
        ("l", TerminalModifiers::Ctrl) => Some("\x0c"), //12
        ("L", TerminalModifiers::CtrlShift) => Some("\x0c"), //12
        ("m", TerminalModifiers::Ctrl) => Some("\x0d"), //13
        ("M", TerminalModifiers::CtrlShift) => Some("\x0d"), //13
        ("n", TerminalModifiers::Ctrl) => Some("\x0e"), //14
        ("N", TerminalModifiers::CtrlShift) => Some("\x0e"), //14
        ("o", TerminalModifiers::Ctrl) => Some("\x0f"), //15
        ("O", TerminalModifiers::CtrlShift) => Some("\x0f"), //15
        ("p", TerminalModifiers::Ctrl) => Some("\x10"), //16
        ("P", TerminalModifiers::CtrlShift) => Some("\x10"), //16
        ("q", TerminalModifiers::Ctrl) => Some("\x11"), //17
        ("Q", TerminalModifiers::CtrlShift) => Some("\x11"), //17
        ("r", TerminalModifiers::Ctrl) => Some("\x12"), //18
        ("R", TerminalModifiers::CtrlShift) => Some("\x12"), //18
        ("s", TerminalModifiers::Ctrl) => Some("\x13"), //19
        ("S", TerminalModifiers::CtrlShift) => Some("\x13"), //19
        ("t", TerminalModifiers::Ctrl) => Some("\x14"), //20
        ("T", TerminalModifiers::CtrlShift) => Some("\x14"), //20
        ("u", TerminalModifiers::Ctrl) => Some("\x15"), //21
        ("U", TerminalModifiers::CtrlShift) => Some("\x15"), //21
        ("v", TerminalModifiers::Ctrl) => Some("\x16"), //22
        ("V", TerminalModifiers::CtrlShift) => Some("\x16"), //22
        ("w", TerminalModifiers::Ctrl) => Some("\x17"), //23
        ("W", TerminalModifiers::CtrlShift) => Some("\x17"), //23
        ("x", TerminalModifiers::Ctrl) => Some("\x18"), //24
        ("X", TerminalModifiers::CtrlShift) => Some("\x18"), //24
        ("y", TerminalModifiers::Ctrl) => Some("\x19"), //25
        ("Y", TerminalModifiers::CtrlShift) => Some("\x19"), //25
        ("z", TerminalModifiers::Ctrl) => Some("\x1a"), //26
        ("Z", TerminalModifiers::CtrlShift) => Some("\x1a"), //26
        ("@", TerminalModifiers::Ctrl) => Some("\x00"), //0
        ("[", TerminalModifiers::Ctrl) => Some("\x1b"), //27
        ("\\", TerminalModifiers::Ctrl) => Some("\x1c"), //28
        ("]", TerminalModifiers::Ctrl) => Some("\x1d"), //29
        ("^", TerminalModifiers::Ctrl) => Some("\x1e"), //30
        ("_", TerminalModifiers::Ctrl) => Some("\x1f"), //31
        ("?", TerminalModifiers::Ctrl) => Some("\x7f"), //127
        _ => None,
    };
    if let Some(esc_str) = manual_esc_str {
        return Some(Cow::Borrowed(esc_str));
    }

    // Automated bindings applying modifiers
    if modifiers.any() {
        let modifier_code = modifier_code(keystroke);
        let modified_esc_str = match keystroke.key.as_ref() {
            "up" => Some(format!("\x1b[1;{}A", modifier_code)),
            "down" => Some(format!("\x1b[1;{}B", modifier_code)),
            "right" => Some(format!("\x1b[1;{}C", modifier_code)),
            "left" => Some(format!("\x1b[1;{}D", modifier_code)),
            "f1" => Some(format!("\x1b[1;{}P", modifier_code)),
            "f2" => Some(format!("\x1b[1;{}Q", modifier_code)),
            "f3" => Some(format!("\x1b[1;{}R", modifier_code)),
            "f4" => Some(format!("\x1b[1;{}S", modifier_code)),
            "F5" => Some(format!("\x1b[15;{}~", modifier_code)),
            "f6" => Some(format!("\x1b[17;{}~", modifier_code)),
            "f7" => Some(format!("\x1b[18;{}~", modifier_code)),
            "f8" => Some(format!("\x1b[19;{}~", modifier_code)),
            "f9" => Some(format!("\x1b[20;{}~", modifier_code)),
            "f10" => Some(format!("\x1b[21;{}~", modifier_code)),
            "f11" => Some(format!("\x1b[23;{}~", modifier_code)),
            "f12" => Some(format!("\x1b[24;{}~", modifier_code)),
            "f13" => Some(format!("\x1b[25;{}~", modifier_code)),
            "f14" => Some(format!("\x1b[26;{}~", modifier_code)),
            "f15" => Some(format!("\x1b[28;{}~", modifier_code)),
            "f16" => Some(format!("\x1b[29;{}~", modifier_code)),
            "f17" => Some(format!("\x1b[31;{}~", modifier_code)),
            "f18" => Some(format!("\x1b[32;{}~", modifier_code)),
            "f19" => Some(format!("\x1b[33;{}~", modifier_code)),
            "f20" => Some(format!("\x1b[34;{}~", modifier_code)),
            "insert" => Some(format!("\x1b[2;{}~", modifier_code)),
            "pageup" => Some(format!("\x1b[5;{}~", modifier_code)),
            "pagedown" => Some(format!("\x1b[6;{}~", modifier_code)),
            "end" => Some(format!("\x1b[1;{}F", modifier_code)),
            "home" => Some(format!("\x1b[1;{}H", modifier_code)),
            _ => None,
        };
        if let Some(esc_str) = modified_esc_str {
            return Some(Cow::Owned(esc_str));
        }
    }

    if !cfg!(target_os = "macos") || option_as_meta {
        let is_alt_lowercase_ascii =
            modifiers == TerminalModifiers::Alt && keystroke.key.is_ascii();
        let is_alt_uppercase_ascii =
            keystroke.modifiers.alt && keystroke.modifiers.shift && keystroke.key.is_ascii();
        if is_alt_lowercase_ascii || is_alt_uppercase_ascii {
            let key = if is_alt_uppercase_ascii {
                &keystroke.key.to_ascii_uppercase()
            } else {
                &keystroke.key
            };
            return Some(Cow::Owned(format!("\x1b{}", key)));
        }
    }

    None
}

///   Code     Modifiers
/// ---------+---------------------------
///    2     | Shift
///    3     | Alt
///    4     | Shift + Alt
///    5     | Control
///    6     | Shift + Control
///    7     | Alt + Control
///    8     | Shift + Alt + Control
/// ---------+---------------------------
/// from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
fn modifier_code(keystroke: &Keystroke) -> u32 {
    let mut modifier_code = 0;
    if keystroke.modifiers.shift {
        modifier_code |= 1;
    }
    if keystroke.modifiers.alt {
        modifier_code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        modifier_code |= 1 << 2;
    }
    modifier_code + 1
}

/// Flags from the kitty keyboard protocol.
///
/// https://sw.kovidgoyal.net/kitty/keyboard-protocol/
#[derive(Clone, Copy, Default)]
struct KittyFlags {
    disambiguate: bool,
    report_events: bool,
    report_alternate: bool,
    report_all: bool,
    report_text: bool,
}

impl KittyFlags {
    fn from_mode(mode: Modes) -> Self {
        Self {
            disambiguate: mode.contains(Modes::KITTY_DISAMBIGUATE_ESC_CODES),
            report_events: mode.contains(Modes::KITTY_REPORT_EVENT_TYPES),
            report_alternate: mode.contains(Modes::KITTY_REPORT_ALTERNATE_KEYS),
            report_all: mode.contains(Modes::KITTY_REPORT_ALL_KEYS_AS_ESC),
            report_text: mode.contains(Modes::KITTY_REPORT_ASSOCIATED_TEXT),
        }
    }

    /// Flags 1, 2 and 8 change the encoding form. Flags 4 and 16 are pure
    /// enhancements to events that are already encoded as escape codes.
    fn changes_encoding(self) -> bool {
        self.disambiguate || self.report_events || self.report_all
    }
}

#[derive(Clone, Copy)]
enum KittyKey {
    /// `CSI code ; modifiers u`
    CsiU(u32),
    /// `CSI number ; modifiers <final>`
    Csi { number: u32, final_char: char },
}

/// Functional keys from the kitty protocol's table.
fn kitty_functional_key(key: &str) -> Option<KittyKey> {
    Some(match key {
        "enter" => KittyKey::CsiU(13),
        "tab" => KittyKey::CsiU(9),
        "backspace" | "back" => KittyKey::CsiU(127),
        "escape" => KittyKey::CsiU(27),
        "insert" => KittyKey::Csi {
            number: 2,
            final_char: '~',
        },
        "delete" => KittyKey::Csi {
            number: 3,
            final_char: '~',
        },
        "pageup" => KittyKey::Csi {
            number: 5,
            final_char: '~',
        },
        "pagedown" => KittyKey::Csi {
            number: 6,
            final_char: '~',
        },
        "up" => KittyKey::Csi {
            number: 1,
            final_char: 'A',
        },
        "down" => KittyKey::Csi {
            number: 1,
            final_char: 'B',
        },
        "right" => KittyKey::Csi {
            number: 1,
            final_char: 'C',
        },
        "left" => KittyKey::Csi {
            number: 1,
            final_char: 'D',
        },
        "home" => KittyKey::Csi {
            number: 1,
            final_char: 'H',
        },
        "end" => KittyKey::Csi {
            number: 1,
            final_char: 'F',
        },
        "f1" => KittyKey::Csi {
            number: 1,
            final_char: 'P',
        },
        "f2" => KittyKey::Csi {
            number: 1,
            final_char: 'Q',
        },
        // The original kitty spec used `CSI R` for F3, but that conflicts
        // with Cursor Position Report, so the table now uses `CSI 13~`.
        "f3" => KittyKey::Csi {
            number: 13,
            final_char: '~',
        },
        "f4" => KittyKey::Csi {
            number: 1,
            final_char: 'S',
        },
        "f5" => KittyKey::Csi {
            number: 15,
            final_char: '~',
        },
        "f6" => KittyKey::Csi {
            number: 17,
            final_char: '~',
        },
        "f7" => KittyKey::Csi {
            number: 18,
            final_char: '~',
        },
        "f8" => KittyKey::Csi {
            number: 19,
            final_char: '~',
        },
        "f9" => KittyKey::Csi {
            number: 20,
            final_char: '~',
        },
        "f10" => KittyKey::Csi {
            number: 21,
            final_char: '~',
        },
        "f11" => KittyKey::Csi {
            number: 23,
            final_char: '~',
        },
        "f12" => KittyKey::Csi {
            number: 24,
            final_char: '~',
        },
        "f13" => KittyKey::CsiU(57376),
        "f14" => KittyKey::CsiU(57377),
        "f15" => KittyKey::CsiU(57378),
        "f16" => KittyKey::CsiU(57379),
        "f17" => KittyKey::CsiU(57380),
        "f18" => KittyKey::CsiU(57381),
        "f19" => KittyKey::CsiU(57382),
        "f20" => KittyKey::CsiU(57383),
        "f21" => KittyKey::CsiU(57384),
        "f22" => KittyKey::CsiU(57385),
        "f23" => KittyKey::CsiU(57386),
        "f24" => KittyKey::CsiU(57387),
        "f25" => KittyKey::CsiU(57388),
        "f26" => KittyKey::CsiU(57389),
        "f27" => KittyKey::CsiU(57390),
        "f28" => KittyKey::CsiU(57391),
        "f29" => KittyKey::CsiU(57392),
        "f30" => KittyKey::CsiU(57393),
        "f31" => KittyKey::CsiU(57394),
        "f32" => KittyKey::CsiU(57395),
        "f33" => KittyKey::CsiU(57396),
        "f34" => KittyKey::CsiU(57397),
        "f35" => KittyKey::CsiU(57398),
        "capslock" | "caps_lock" => KittyKey::CsiU(57358),
        "scrolllock" | "scroll_lock" => KittyKey::CsiU(57359),
        "numlock" | "num_lock" => KittyKey::CsiU(57360),
        "printscreen" | "print_screen" => KittyKey::CsiU(57361),
        "pause" => KittyKey::CsiU(57362),
        "menu" => KittyKey::CsiU(57363),
        "kp_0" | "kp0" => KittyKey::CsiU(57399),
        "kp_1" | "kp1" => KittyKey::CsiU(57400),
        "kp_2" | "kp2" => KittyKey::CsiU(57401),
        "kp_3" | "kp3" => KittyKey::CsiU(57402),
        "kp_4" | "kp4" => KittyKey::CsiU(57403),
        "kp_5" | "kp5" => KittyKey::CsiU(57404),
        "kp_6" | "kp6" => KittyKey::CsiU(57405),
        "kp_7" | "kp7" => KittyKey::CsiU(57406),
        "kp_8" | "kp8" => KittyKey::CsiU(57407),
        "kp_9" | "kp9" => KittyKey::CsiU(57408),
        "kp_decimal" | "kpdecimal" => KittyKey::CsiU(57409),
        "kp_divide" | "kpdivide" => KittyKey::CsiU(57410),
        "kp_multiply" | "kpmultiply" => KittyKey::CsiU(57411),
        "kp_subtract" | "kpsubtract" => KittyKey::CsiU(57412),
        "kp_add" | "kpadd" => KittyKey::CsiU(57413),
        "kp_enter" | "kpenter" => KittyKey::CsiU(57414),
        "kp_equal" | "kpequal" => KittyKey::CsiU(57415),
        "kp_separator" | "kpseparator" => KittyKey::CsiU(57416),
        "kp_left" | "kpleft" => KittyKey::CsiU(57417),
        "kp_right" | "kpright" => KittyKey::CsiU(57418),
        "kp_up" | "kpup" => KittyKey::CsiU(57419),
        "kp_down" | "kpdown" => KittyKey::CsiU(57420),
        "kp_page_up" | "kppageup" => KittyKey::CsiU(57421),
        "kp_page_down" | "kppagedown" => KittyKey::CsiU(57422),
        "kp_home" | "kphome" => KittyKey::CsiU(57423),
        "kp_end" | "kpend" => KittyKey::CsiU(57424),
        "kp_insert" | "kpinsert" => KittyKey::CsiU(57425),
        "kp_delete" | "kpdelete" => KittyKey::CsiU(57426),
        "kp_begin" | "kpbegin" => KittyKey::CsiU(57427),
        "media_play" | "mediaplay" => KittyKey::CsiU(57428),
        "media_pause" | "mediapause" => KittyKey::CsiU(57429),
        "media_play_pause" | "mediaplaypause" => KittyKey::CsiU(57430),
        "media_reverse" | "mediareverse" => KittyKey::CsiU(57431),
        "media_stop" | "mediastop" => KittyKey::CsiU(57432),
        "media_fast_forward" | "mediafastforward" => KittyKey::CsiU(57433),
        "media_rewind" | "mediarewind" => KittyKey::CsiU(57434),
        "media_track_next" | "mediatracknext" => KittyKey::CsiU(57435),
        "media_track_previous" | "mediatrackprevious" => KittyKey::CsiU(57436),
        "media_record" | "mediarecord" => KittyKey::CsiU(57437),
        "lower_volume" | "volumedown" => KittyKey::CsiU(57438),
        "raise_volume" | "volumeup" => KittyKey::CsiU(57439),
        "mute_volume" | "volumemute" | "mute" => KittyKey::CsiU(57440),
        "shift" => KittyKey::CsiU(57441),
        "control" => KittyKey::CsiU(57442),
        "alt" => KittyKey::CsiU(57443),
        "platform" | "super" | "cmd" | "win" => KittyKey::CsiU(57444),
        "hyper" => KittyKey::CsiU(57445),
        "meta" => KittyKey::CsiU(57446),
        "right_shift" => KittyKey::CsiU(57447),
        "right_control" => KittyKey::CsiU(57448),
        "right_alt" => KittyKey::CsiU(57449),
        "right_platform" | "right_super" => KittyKey::CsiU(57450),
        "right_hyper" => KittyKey::CsiU(57451),
        "right_meta" => KittyKey::CsiU(57452),
        "iso_level3_shift" => KittyKey::CsiU(57453),
        "iso_level5_shift" => KittyKey::CsiU(57454),
        _ => return None,
    })
}

fn kitty_key_is_modifier(key: &str) -> bool {
    matches!(
        key,
        "shift"
            | "control"
            | "alt"
            | "platform"
            | "super"
            | "cmd"
            | "win"
            | "hyper"
            | "meta"
            | "right_shift"
            | "right_control"
            | "right_alt"
            | "right_platform"
            | "right_super"
            | "right_hyper"
            | "right_meta"
            | "iso_level3_shift"
            | "iso_level5_shift"
    )
}

/// Modifier bits for the kitty keyboard protocol: the value is 1 plus the
/// sum of modifier weights (shift = 1, alt = 2, ctrl = 4, super = 8).
fn kitty_modifiers(keystroke: &Keystroke) -> u32 {
    let mut mods = 0;
    if keystroke.modifiers.shift || keystroke.key == "shift" {
        mods |= 1;
    }
    if keystroke.modifiers.alt || keystroke.key == "alt" {
        mods |= 2;
    }
    if keystroke.modifiers.control || keystroke.key == "control" {
        mods |= 4;
    }
    if keystroke.modifiers.platform
        || matches!(keystroke.key.as_str(), "platform" | "super" | "cmd" | "win")
    {
        mods |= 8;
    }
    mods + 1
}

fn kitty_printable_char(keystroke: &Keystroke) -> Option<char> {
    if keystroke.key == "space" {
        return Some(' ');
    }
    let mut chars = keystroke.key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(c),
        _ => None,
    }
}

fn single_printable_char(text: Option<&str>) -> Option<char> {
    let mut chars = text?.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(c),
        _ => None,
    }
}

fn unshift_char(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(lower), None) => lower,
        _ => c,
    }
}

/// Best-effort shifted form for US-layout ASCII keys when the platform did
/// not provide `key_char` (notably synthesized keystrokes in tests and
/// `send-keys`-style automation).
fn shifted_ascii_char(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '`' => '~',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        _ => c,
    }
}

/// The unshifted Unicode code point for a printable key. For non-Latin
/// layouts GPUI puts the typed character in `key_char` and the PC-101
/// equivalent in `key`.
fn kitty_unicode_code(keystroke: &Keystroke) -> Option<u32> {
    let key_char = single_printable_char(keystroke.key_char.as_deref());
    let key = kitty_printable_char(keystroke);

    if let Some(c) = key_char {
        if !c.is_ascii() || key.is_none() {
            let c = if keystroke.modifiers.shift {
                unshift_char(c)
            } else {
                c
            };
            return Some(c as u32);
        }
    }

    key.map(|c| c as u32)
}

fn kitty_shifted_key(keystroke: &Keystroke, unicode_code: u32) -> Option<u32> {
    if !keystroke.modifiers.shift {
        return None;
    }

    let shifted = single_printable_char(keystroke.key_char.as_deref()).or_else(|| {
        let key = kitty_printable_char(keystroke)?;
        Some(shifted_ascii_char(key))
    })?;
    let code = shifted as u32;
    (code != unicode_code).then_some(code)
}

fn kitty_base_layout_key(keystroke: &Keystroke, unicode_code: u32) -> Option<u32> {
    if unicode_code == 0 {
        return None;
    }
    let key = kitty_printable_char(keystroke)?;
    if key == ' ' {
        return None;
    }
    let code = key as u32;
    (code != unicode_code).then_some(code)
}

fn kitty_text(keystroke: &Keystroke, alt_is_meta: bool) -> Option<String> {
    if keystroke.modifiers.control || keystroke.modifiers.platform || alt_is_meta {
        return None;
    }

    let text = match keystroke.key_char.as_deref() {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => {
            let key = kitty_printable_char(keystroke)?;
            if keystroke.modifiers.shift {
                shifted_ascii_char(key).to_string()
            } else {
                key.to_string()
            }
        }
    };
    if text.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(text)
}

fn alt_consumed_text(keystroke: &Keystroke, alt_is_meta: bool) -> bool {
    if keystroke.modifiers.control
        || keystroke.modifiers.platform
        || !keystroke.modifiers.alt
        || alt_is_meta
    {
        return false;
    }
    let Some(key_char) = keystroke.key_char.as_deref() else {
        return false;
    };
    if key_char.is_empty() || key_char.chars().any(|c| c.is_control()) {
        return false;
    }
    match kitty_printable_char(keystroke) {
        Some(key) => key_char != key.to_string(),
        None => true,
    }
}

fn kitty_text_codepoints(text: &str) -> String {
    text.chars()
        .map(|c| (c as u32).to_string())
        .collect::<Vec<_>>()
        .join(":")
}

fn modifier_field(mods: u32, event_type: KeyEventType, report_events: bool) -> String {
    if report_events && event_type != KeyEventType::Press {
        format!("{mods}:{}", event_type.kitty_code())
    } else {
        mods.to_string()
    }
}

fn encode_functional(
    key: KittyKey,
    mods: u32,
    mods_field: &str,
    event_type: KeyEventType,
    report_events: bool,
) -> String {
    let has_mods_or_event = mods != 1 || (report_events && event_type != KeyEventType::Press);
    match key {
        KittyKey::CsiU(code) => {
            if has_mods_or_event {
                format!("\x1b[{code};{mods_field}u")
            } else {
                format!("\x1b[{code}u")
            }
        }
        KittyKey::Csi { number, final_char } => {
            if has_mods_or_event {
                format!("\x1b[{number};{mods_field}{final_char}")
            } else if number == 1 {
                format!("\x1b[{final_char}")
            } else {
                format!("\x1b[{number}{final_char}")
            }
        }
    }
}

fn encode_csi_u(
    code: u32,
    shifted: Option<u32>,
    base: Option<u32>,
    mods: u32,
    mods_field: &str,
    event_type: KeyEventType,
    report_events: bool,
    text: Option<&str>,
) -> String {
    let mut key = code.to_string();
    if shifted.is_some() || base.is_some() {
        key.push(':');
        if let Some(shifted) = shifted {
            key.push_str(&shifted.to_string());
        }
        if let Some(base) = base {
            key.push(':');
            key.push_str(&base.to_string());
        }
    }

    let has_mods_or_event = mods != 1 || (report_events && event_type != KeyEventType::Press);
    let mut out = String::from("\x1b[");
    out.push_str(&key);
    if let Some(text) = text {
        out.push(';');
        if has_mods_or_event {
            out.push_str(mods_field);
        }
        out.push(';');
        out.push_str(text);
        out.push('u');
    } else if has_mods_or_event {
        out.push(';');
        out.push_str(mods_field);
        out.push('u');
    } else {
        out.push('u');
    }
    out
}

/// Encode a key event using the kitty keyboard protocol.
///
/// Format: `CSI unicode-key-code:alternate-keys ; modifiers:event-type ; text u`
fn kitty_esc_str(
    keystroke: &Keystroke,
    mode: Modes,
    option_as_meta: bool,
    event_type: KeyEventType,
) -> Option<Cow<'static, str>> {
    let flags = KittyFlags::from_mode(mode);
    if !flags.changes_encoding() {
        // Alternate-key and associated-text reporting are pure enhancements:
        // without one of flags 1, 2 or 8 the legacy encoding is unchanged.
        return legacy_esc_str(keystroke, mode, option_as_meta);
    }

    let key = keystroke.key.as_str();
    // On non-macOS alt is always the meta/escape modifier. On macOS it is
    // only meta when the user opted in via `option_as_meta`; otherwise it
    // composes text (e.g. option-a -> å).
    let alt_is_meta = keystroke.modifiers.alt && (!cfg!(target_os = "macos") || option_as_meta);
    let has_meta_modifier =
        keystroke.modifiers.control || keystroke.modifiers.platform || alt_is_meta;
    let text = kitty_text(keystroke, alt_is_meta);
    let printable = kitty_printable_char(keystroke);
    let functional = kitty_functional_key(key);
    let modifier_key = kitty_key_is_modifier(key);
    let legacy_exception = matches!(key, "enter" | "tab" | "backspace" | "back");

    let should_encode = if legacy_exception {
        // Enter/Tab/Backspace keep their legacy bytes unless every key is
        // reported as an escape code, so users can always type `reset`.
        flags.report_all
    } else if key == "escape" {
        flags.disambiguate || flags.report_all
    } else if modifier_key {
        flags.report_all
    } else if functional.is_some() {
        true
    } else if printable.is_some() {
        flags.report_all || (flags.disambiguate && has_meta_modifier)
    } else {
        flags.disambiguate
    };

    if event_type == KeyEventType::Release {
        // Legacy text events have no release form. Enter/Tab/Backspace also
        // stay release-less unless flag 8 is active.
        if !flags.report_events || !should_encode {
            return None;
        }
        if legacy_exception && !flags.report_all {
            return None;
        }
    }

    if !should_encode {
        if event_type == KeyEventType::Release {
            return None;
        }
        if let Some(text) = &text {
            return Some(Cow::Owned(text.clone()));
        }
        return legacy_esc_str(keystroke, mode, option_as_meta);
    }

    // A macOS option-composed character is delivered by the OS as pure text;
    // the spec encodes that as key code 0 with no modifiers.
    let pure_text = alt_consumed_text(keystroke, alt_is_meta) && flags.report_all;
    let mods = if pure_text {
        1
    } else {
        kitty_modifiers(keystroke)
    };
    let mods_field = modifier_field(mods, event_type, flags.report_events);
    let text_field = if flags.report_text && flags.report_all && event_type != KeyEventType::Release
    {
        text.as_deref().map(kitty_text_codepoints)
    } else {
        None
    };

    if let Some(functional) = functional {
        return Some(Cow::Owned(encode_functional(
            functional,
            mods,
            &mods_field,
            event_type,
            flags.report_events,
        )));
    }

    let code = if pure_text {
        0
    } else {
        kitty_unicode_code(keystroke)?
    };
    let shifted = if flags.report_alternate && !pure_text {
        kitty_shifted_key(keystroke, code)
    } else {
        None
    };
    let base = if flags.report_alternate && !pure_text {
        kitty_base_layout_key(keystroke, code)
    } else {
        None
    };

    Some(Cow::Owned(encode_csi_u(
        code,
        shifted,
        base,
        mods,
        &mods_field,
        event_type,
        flags.report_events,
        text_field.as_deref(),
    )))
}

#[cfg(test)]
mod test {
    use gpui::Modifiers;

    use super::*;

    #[test]
    fn test_plain_inputs() {
        let ks = Keystroke {
            modifiers: Modifiers {
                control: false,
                alt: false,
                shift: false,
                platform: false,
                function: false,
            },
            key: "🖖🏻".to_string(), //2 char string
            key_char: None,
        };
        assert_eq!(to_esc_str(&ks, Modes::NONE, false), None);
    }

    #[test]
    fn test_application_mode() {
        let app_cursor = Modes::APP_CURSOR;
        let none = Modes::NONE;

        let up = Keystroke::parse("up").unwrap();
        let down = Keystroke::parse("down").unwrap();
        let left = Keystroke::parse("left").unwrap();
        let right = Keystroke::parse("right").unwrap();

        assert_eq!(to_esc_str(&up, none, false), Some("\x1b[A".into()));
        assert_eq!(to_esc_str(&down, none, false), Some("\x1b[B".into()));
        assert_eq!(to_esc_str(&right, none, false), Some("\x1b[C".into()));
        assert_eq!(to_esc_str(&left, none, false), Some("\x1b[D".into()));

        assert_eq!(to_esc_str(&up, app_cursor, false), Some("\x1bOA".into()));
        assert_eq!(to_esc_str(&down, app_cursor, false), Some("\x1bOB".into()));
        assert_eq!(to_esc_str(&right, app_cursor, false), Some("\x1bOC".into()));
        assert_eq!(to_esc_str(&left, app_cursor, false), Some("\x1bOD".into()));

        let home = Keystroke::parse("home").unwrap();
        let end = Keystroke::parse("end").unwrap();
        assert_eq!(to_esc_str(&home, none, false), Some("\x1b[H".into()));
        assert_eq!(to_esc_str(&end, none, false), Some("\x1b[F".into()));
        assert_eq!(to_esc_str(&home, app_cursor, false), Some("\x1bOH".into()));
        assert_eq!(to_esc_str(&end, app_cursor, false), Some("\x1bOF".into()));

        let shift_up = Keystroke::parse("shift-up").unwrap();
        let shift_down = Keystroke::parse("shift-down").unwrap();
        let shift_home = Keystroke::parse("shift-home").unwrap();
        let shift_end = Keystroke::parse("shift-end").unwrap();
        assert_eq!(to_esc_str(&shift_up, none, false), Some("\x1b[1;2A".into()));
        assert_eq!(
            to_esc_str(&shift_down, none, false),
            Some("\x1b[1;2B".into())
        );
        assert_eq!(
            to_esc_str(&shift_home, none, false),
            Some("\x1b[1;2H".into())
        );
        assert_eq!(
            to_esc_str(&shift_end, none, false),
            Some("\x1b[1;2F".into())
        );
    }

    #[test]
    fn test_kitty_keyboard_all_flags() {
        let kitty = Modes::KITTY_KEYBOARD_PROTOCOL;

        // Flag 8 + 16: plain text becomes CSI u with associated text.
        let a = Keystroke::parse("a").unwrap();
        assert_eq!(to_esc_str(&a, kitty, false), Some("\x1b[97;;97u".into()));

        // Ctrl+letter uses CSI u with ctrl modifier bit (4) + 1.
        let ctrl_c = Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(to_esc_str(&ctrl_c, kitty, false), Some("\x1b[99;5u".into()));

        // Flag 4 adds the shifted key for ctrl+shift+a.
        let ctrl_shift_a = Keystroke::parse("ctrl-shift-a").unwrap();
        assert_eq!(
            to_esc_str(&ctrl_shift_a, kitty, false),
            Some("\x1b[97:65;6u".into())
        );

        // Alt+key with option-as-meta uses the alt modifier bit (2) + 1.
        let alt_x = Keystroke::parse("alt-x").unwrap();
        assert_eq!(to_esc_str(&alt_x, kitty, true), Some("\x1b[120;3u".into()));

        // Functional keys use their protocol form, not PUA key codes.
        let up = Keystroke::parse("up").unwrap();
        assert_eq!(to_esc_str(&up, kitty, false), Some("\x1b[A".into()));
        let shift_up = Keystroke::parse("shift-up").unwrap();
        assert_eq!(
            to_esc_str(&shift_up, kitty, false),
            Some("\x1b[1;2A".into())
        );
        let shift_tab = Keystroke::parse("shift-tab").unwrap();
        assert_eq!(
            to_esc_str(&shift_tab, kitty, false),
            Some("\x1b[9;2u".into())
        );
        let ctrl_f5 = Keystroke::parse("ctrl-f5").unwrap();
        assert_eq!(
            to_esc_str(&ctrl_f5, kitty, false),
            Some("\x1b[15;5~".into())
        );

        // Flag 8 also escapes Enter/Tab/Backspace/Escape.
        let enter = Keystroke::parse("enter").unwrap();
        assert_eq!(to_esc_str(&enter, kitty, false), Some("\x1b[13u".into()));
        let shift_enter = Keystroke::parse("shift-enter").unwrap();
        assert_eq!(
            to_esc_str(&shift_enter, kitty, false),
            Some("\x1b[13;2u".into())
        );
        let escape = Keystroke::parse("escape").unwrap();
        assert_eq!(to_esc_str(&escape, kitty, false), Some("\x1b[27u".into()));
        let ctrl_escape = Keystroke::parse("ctrl-escape").unwrap();
        assert_eq!(
            to_esc_str(&ctrl_escape, kitty, false),
            Some("\x1b[27;5u".into())
        );

        // Legacy behavior without the protocol.
        assert_eq!(to_esc_str(&ctrl_c, Modes::NONE, false), Some("\x03".into()));
        assert_eq!(to_esc_str(&up, Modes::NONE, false), Some("\x1b[A".into()));
    }

    #[test]
    fn test_kitty_disambiguate_only() {
        let mode = Modes::KITTY_DISAMBIGUATE_ESC_CODES;

        // Plain text and shift-only text stay as text.
        assert_eq!(
            to_esc_str(&Keystroke::parse("a").unwrap(), mode, false),
            Some("a".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-a").unwrap(), mode, false),
            Some("A".into())
        );

        // Ambiguous combinations become CSI u.
        assert_eq!(
            to_esc_str(&Keystroke::parse("ctrl-c").unwrap(), mode, false),
            Some("\x1b[99;5u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("ctrl-shift-a").unwrap(), mode, false),
            Some("\x1b[97;6u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("alt-x").unwrap(), mode, true),
            Some("\x1b[120;3u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("escape").unwrap(), mode, false),
            Some("\x1b[27u".into())
        );

        // Enter/Tab/Backspace keep their legacy bytes with flag 1 only.
        assert_eq!(
            to_esc_str(&Keystroke::parse("enter").unwrap(), mode, false),
            Some("\r".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-tab").unwrap(), mode, false),
            Some("\x1b[Z".into())
        );
    }

    #[test]
    fn test_kitty_event_types() {
        let mode = Modes::KITTY_DISAMBIGUATE_ESC_CODES | Modes::KITTY_REPORT_EVENT_TYPES;
        let ctrl_c = Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(
            to_esc_str_for_event(&ctrl_c, mode, false, KeyEventType::Press),
            Some("\x1b[99;5u".into())
        );
        assert_eq!(
            to_esc_str_for_event(&ctrl_c, mode, false, KeyEventType::Repeat),
            Some("\x1b[99;5:2u".into())
        );
        assert_eq!(
            to_esc_str_for_event(&ctrl_c, mode, false, KeyEventType::Release),
            Some("\x1b[99;5:3u".into())
        );

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(
            to_esc_str_for_event(&up, mode, false, KeyEventType::Press),
            Some("\x1b[A".into())
        );
        assert_eq!(
            to_esc_str_for_event(&up, mode, false, KeyEventType::Repeat),
            Some("\x1b[1;1:2A".into())
        );
        assert_eq!(
            to_esc_str_for_event(&up, mode, false, KeyEventType::Release),
            Some("\x1b[1;1:3A".into())
        );

        // Text events have no release form without flag 8. Repeats still
        // produce text, as they do in a legacy terminal.
        let a = Keystroke::parse("a").unwrap();
        assert_eq!(
            to_esc_str_for_event(&a, mode, false, KeyEventType::Repeat),
            Some("a".into())
        );
        assert_eq!(
            to_esc_str_for_event(&a, mode, false, KeyEventType::Release),
            None
        );

        // Enter/Tab/Backspace do not get release events without flag 8.
        let enter = Keystroke::parse("enter").unwrap();
        assert_eq!(
            to_esc_str_for_event(&enter, mode, false, KeyEventType::Release),
            None
        );

        // Without flag 2 a release must never be translated into a legacy
        // key press; repeats still behave like presses.
        assert_eq!(
            to_esc_str_for_event(&a, Modes::NONE, false, KeyEventType::Release),
            None
        );
        assert_eq!(
            to_esc_str_for_event(
                &a,
                Modes::KITTY_REPORT_ALL_KEYS_AS_ESC,
                false,
                KeyEventType::Release
            ),
            None
        );
        // Without the kitty protocol, plain text repeats are left to GPUI's
        // text-input path (the encoder returns None for character keys).
        assert_eq!(
            to_esc_str_for_event(&a, Modes::NONE, false, KeyEventType::Repeat),
            None
        );
    }

    #[test]
    fn test_kitty_report_all_and_text() {
        let all = Modes::KITTY_REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            to_esc_str(&Keystroke::parse("a").unwrap(), all, false),
            Some("\x1b[97u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-a").unwrap(), all, false),
            Some("\x1b[97;2u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("enter").unwrap(), all, false),
            Some("\x1b[13u".into())
        );

        let with_text = all | Modes::KITTY_REPORT_ASSOCIATED_TEXT;
        assert_eq!(
            to_esc_str(&Keystroke::parse("a").unwrap(), with_text, false),
            Some("\x1b[97;;97u".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-a").unwrap(), with_text, false),
            Some("\x1b[97;2;65u".into())
        );
        // Ctrl combinations do not produce text.
        assert_eq!(
            to_esc_str(&Keystroke::parse("ctrl-c").unwrap(), with_text, false),
            Some("\x1b[99;5u".into())
        );

        // All-keys reporting also gives text keys release events.
        let mode = with_text | Modes::KITTY_REPORT_EVENT_TYPES;
        assert_eq!(
            to_esc_str_for_event(
                &Keystroke::parse("a").unwrap(),
                mode,
                false,
                KeyEventType::Release
            ),
            Some("\x1b[97;1:3u".into())
        );
        assert_eq!(
            to_esc_str_for_event(
                &Keystroke::parse("enter").unwrap(),
                mode,
                false,
                KeyEventType::Release
            ),
            Some("\x1b[13;1:3u".into())
        );

        // Modifier keys are reported too, and a release still carries its
        // own modifier bit.
        assert_eq!(
            to_esc_str_for_event(
                &Keystroke::parse("shift").unwrap(),
                mode,
                false,
                KeyEventType::Release
            ),
            Some("\x1b[57441;2:3u".into())
        );
    }

    #[test]
    fn test_kitty_alternate_keys() {
        let mode = Modes::KITTY_DISAMBIGUATE_ESC_CODES | Modes::KITTY_REPORT_ALTERNATE_KEYS;

        // Flag 4 is a pure enhancement: plain text is still text.
        assert_eq!(
            to_esc_str(&Keystroke::parse("a").unwrap(), mode, false),
            Some("a".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("ctrl-shift-a").unwrap(), mode, false),
            Some("\x1b[97:65;6u".into())
        );

        // Non-Latin layout: unicode key is the typed character, base layout
        // key is the PC-101 equivalent in `key`.
        let mut cyrillic = Keystroke::parse("ctrl-c").unwrap();
        cyrillic.key_char = Some("ф".to_string());
        assert_eq!(
            to_esc_str(&cyrillic, mode, false),
            Some("\x1b[1092::99;5u".into())
        );

        let mut shifted_cyrillic = Keystroke::parse("ctrl-shift-c").unwrap();
        shifted_cyrillic.key_char = Some("Ф".to_string());
        assert_eq!(
            to_esc_str(&shifted_cyrillic, mode, false),
            Some("\x1b[1092:1060:99;6u".into())
        );
    }

    #[test]
    fn test_kitty_prefers_key_char() {
        // A dead-key composition reports the physical key in `key` and the
        // composed character in `key_char`; the composed character must win.
        let mut composed = Keystroke::parse("a").unwrap();
        composed.key_char = Some("å".to_string());
        assert_eq!(
            to_esc_str(&composed, Modes::KITTY_DISAMBIGUATE_ESC_CODES, false),
            Some("å".into())
        );
    }

    #[test]
    fn test_ctrl_codes() {
        let letters_lower = 'a'..='z';
        let letters_upper = 'A'..='Z';
        let mode = Modes::APP_CURSOR;

        for (lower, upper) in letters_lower.zip(letters_upper) {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-shift-{}", lower)).unwrap(),
                    mode,
                    false
                ),
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-{}", upper)).unwrap(),
                    mode,
                    false
                ),
                "On letter: {}/{}",
                lower,
                upper
            )
        }
    }

    #[test]
    fn alt_is_meta() {
        let ascii_printable = ' '..='~';
        for character in ascii_printable {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", character)).unwrap(),
                    Modes::NONE,
                    true
                )
                .unwrap(),
                format!("\x1b{}", character)
            );
        }

        let gpui_keys = [
            "up", "down", "right", "left", "f1", "f2", "f3", "f4", "F5", "f6", "f7", "f8", "f9",
            "f10", "f11", "f12", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20", "insert",
            "pageup", "pagedown", "end", "home",
        ];

        for key in gpui_keys {
            assert_ne!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", key)).unwrap(),
                    Modes::NONE,
                    true
                )
                .unwrap(),
                format!("\x1b{}", key)
            );
        }
    }

    #[test]
    fn test_shift_enter_newline() {
        let shift_enter = Keystroke::parse("shift-enter").unwrap();
        let regular_enter = Keystroke::parse("enter").unwrap();
        let mode = Modes::NONE;

        // Shift-enter should send line feed (newline)
        assert_eq!(to_esc_str(&shift_enter, mode, false), Some("\x0a".into()));

        // Regular enter should still send carriage return
        assert_eq!(to_esc_str(&regular_enter, mode, false), Some("\x0d".into()));
    }

    #[test]
    fn test_modifier_code_calc() {
        //   Code     Modifiers
        // ---------+---------------------------
        //    2     | Shift
        //    3     | Alt
        //    4     | Shift + Alt
        //    5     | Control
        //    6     | Shift + Control
        //    7     | Alt + Control
        //    8     | Shift + Alt + Control
        // ---------+---------------------------
        // from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
        assert_eq!(2, modifier_code(&Keystroke::parse("shift-a").unwrap()));
        assert_eq!(3, modifier_code(&Keystroke::parse("alt-a").unwrap()));
        assert_eq!(4, modifier_code(&Keystroke::parse("shift-alt-a").unwrap()));
        assert_eq!(5, modifier_code(&Keystroke::parse("ctrl-a").unwrap()));
        assert_eq!(6, modifier_code(&Keystroke::parse("shift-ctrl-a").unwrap()));
        assert_eq!(7, modifier_code(&Keystroke::parse("alt-ctrl-a").unwrap()));
        assert_eq!(
            8,
            modifier_code(&Keystroke::parse("shift-ctrl-alt-a").unwrap())
        );
    }
}
