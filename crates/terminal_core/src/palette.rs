//! Resolves the emulator's ANSI colors against a fixed palette.
//!
//! The GUI resolves colors against the active `theme::Theme`, which is why
//! `/api/screen` used to live inside the app. A host without a GUI — the
//! terminal daemon — has no theme, so this module provides the same shape of
//! answer from a plain table: the 16 ANSI colors, the 256-color cube, the
//! grayscale ramp, and the default foreground/background.
//!
//! The defaults match the classic xterm palette, which is what most terminal
//! clients use when they have no color scheme of their own.

use serde::{Deserialize, Serialize};
use vte::ansi::{Color, NamedColor};

/// A color as 8-bit RGB, which is all a client needs to paint a cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Formats the color as `#rrggbb`, the form the JSON endpoints report.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// The colors a host without a theme needs to paint a snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    /// ANSI 0-15: normal colors followed by their bright variants.
    pub ansi: [Rgb; 16],
    /// The 8 dim variants (`DimBlack` … `DimWhite`).
    pub dim: [Rgb; 8],
    pub foreground: Rgb,
    pub background: Rgb,
    /// `BrightForeground`, used for bold text in some schemes.
    pub bright_foreground: Rgb,
    /// `DimForeground`.
    pub dim_foreground: Rgb,
    pub cursor: Rgb,
}

impl Default for Palette {
    fn default() -> Self {
        Self::xterm()
    }
}

impl Palette {
    /// The classic xterm colors, used when no scheme is configured.
    pub fn xterm() -> Self {
        Self {
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xcd, 0x00, 0x00),
                Rgb::new(0x00, 0xcd, 0x00),
                Rgb::new(0xcd, 0xcd, 0x00),
                Rgb::new(0x00, 0x00, 0xee),
                Rgb::new(0xcd, 0x00, 0xcd),
                Rgb::new(0x00, 0xcd, 0xcd),
                Rgb::new(0xe5, 0xe5, 0xe5),
                Rgb::new(0x7f, 0x7f, 0x7f),
                Rgb::new(0xff, 0x00, 0x00),
                Rgb::new(0x00, 0xff, 0x00),
                Rgb::new(0xff, 0xff, 0x00),
                Rgb::new(0x5c, 0x5c, 0xff),
                Rgb::new(0xff, 0x00, 0xff),
                Rgb::new(0x00, 0xff, 0xff),
                Rgb::new(0xff, 0xff, 0xff),
            ],
            dim: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0x8b, 0x00, 0x00),
                Rgb::new(0x00, 0x8b, 0x00),
                Rgb::new(0x8b, 0x8b, 0x00),
                Rgb::new(0x00, 0x00, 0x9e),
                Rgb::new(0x8b, 0x00, 0x8b),
                Rgb::new(0x00, 0x8b, 0x8b),
                Rgb::new(0x9e, 0x9e, 0x9e),
            ],
            foreground: Rgb::new(0xd4, 0xd4, 0xd4),
            background: Rgb::new(0x1e, 0x1e, 0x1e),
            bright_foreground: Rgb::new(0xff, 0xff, 0xff),
            dim_foreground: Rgb::new(0x8a, 0x8a, 0x8a),
            cursor: Rgb::new(0xd4, 0xd4, 0xd4),
        }
    }

    /// Resolves one emulator color to RGB.
    pub fn resolve(&self, color: &Color) -> Rgb {
        match color {
            Color::Spec(rgb) => Rgb::new(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => self.indexed(*index),
            Color::Named(name) => self.named(*name),
        }
    }

    /// Resolves an 8-bit indexed color.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            // 0-15 are the named ANSI colors.
            0..=15 => self.ansi[index as usize],
            // 16-231 are a 6x6x6 RGB cube.
            16..=231 => {
                let (r, g, b) = rgb_cube_steps(index);
                Rgb::new(r, g, b)
            }
            // 232-255 are a 24-step grayscale ramp from (8,8,8) to (238,238,238).
            232..=255 => {
                let value = (index - 232) * 10 + 8;
                Rgb::new(value, value, value)
            }
        }
    }

    /// Resolves one of the emulator's named colors.
    pub fn named(&self, name: NamedColor) -> Rgb {
        match name {
            NamedColor::Black => self.ansi[0],
            NamedColor::Red => self.ansi[1],
            NamedColor::Green => self.ansi[2],
            NamedColor::Yellow => self.ansi[3],
            NamedColor::Blue => self.ansi[4],
            NamedColor::Magenta => self.ansi[5],
            NamedColor::Cyan => self.ansi[6],
            NamedColor::White => self.ansi[7],
            NamedColor::BrightBlack => self.ansi[8],
            NamedColor::BrightRed => self.ansi[9],
            NamedColor::BrightGreen => self.ansi[10],
            NamedColor::BrightYellow => self.ansi[11],
            NamedColor::BrightBlue => self.ansi[12],
            NamedColor::BrightMagenta => self.ansi[13],
            NamedColor::BrightCyan => self.ansi[14],
            NamedColor::BrightWhite => self.ansi[15],
            NamedColor::Foreground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimBlack => self.dim[0],
            NamedColor::DimRed => self.dim[1],
            NamedColor::DimGreen => self.dim[2],
            NamedColor::DimYellow => self.dim[3],
            NamedColor::DimBlue => self.dim[4],
            NamedColor::DimMagenta => self.dim[5],
            NamedColor::DimCyan => self.dim[6],
            NamedColor::DimWhite => self.dim[7],
            NamedColor::BrightForeground => self.bright_foreground,
            NamedColor::DimForeground => self.dim_foreground,
        }
    }
}

/// The xterm 6x6x6 color cube, as the three channel steps for `index`.
fn rgb_cube_steps(index: u8) -> (u8, u8, u8) {
    let index = index - 16;
    let r = (index - (index % 36)) / 36;
    let g = ((index % 36) - (index % 6)) / 6;
    let b = (index % 36) % 6;
    // A zero step is black; the other steps are 55 + 40 * n.
    (
        if r == 0 { 0 } else { r * 40 + 55 },
        if g == 0 { 0 } else { g * 40 + 55 },
        if b == 0 { 0 } else { b * 40 + 55 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_map_to_the_ansi_table() {
        let palette = Palette::xterm();

        assert_eq!(palette.named(NamedColor::Black), palette.ansi[0]);
        assert_eq!(palette.named(NamedColor::Red), palette.ansi[1]);
        assert_eq!(palette.named(NamedColor::BrightWhite), palette.ansi[15]);
        assert_eq!(palette.named(NamedColor::Foreground), palette.foreground);
        assert_eq!(palette.named(NamedColor::Background), palette.background);
    }

    #[test]
    fn indexed_colors_cover_all_three_regions() {
        let palette = Palette::xterm();

        // 0-15 alias the named colors.
        assert_eq!(palette.indexed(1), palette.ansi[1]);
        // 16 is the darkest corner of the cube: black.
        assert_eq!(palette.indexed(16), Rgb::new(0, 0, 0));
        // 231 is the brightest corner: white.
        assert_eq!(palette.indexed(231), Rgb::new(255, 255, 255));
        // 232 starts the grayscale ramp, 255 ends it.
        assert_eq!(palette.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(palette.indexed(255), Rgb::new(238, 238, 238));
    }

    #[test]
    fn true_color_passes_through_untouched() {
        let palette = Palette::xterm();
        let spec = vte::ansi::Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        };

        assert_eq!(
            palette.resolve(&Color::Spec(spec)),
            Rgb::new(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn hex_formatting_is_lowercase_and_padded() {
        assert_eq!(Rgb::new(0x0a, 0xbc, 0xde).to_hex(), "#0abcde");
        assert_eq!(Rgb::new(0, 0, 0).to_hex(), "#000000");
    }
}
