//! Makes the built-in coding agent (`tom`) use the terminal's own colors.
//!
//! `tom` renders its TUI with its own palette, chosen by `--theme` (a built-in
//! preset name or a path to a pi theme JSON file). Terry writes a theme file
//! derived from the palette of the terminal that launched the agent, so the
//! agent's panel matches the terminal around it instead of arriving in an
//! unrelated color scheme.
//!
//! The file is regenerated on every launch and lives in Terry's data directory,
//! so it always reflects the current theme (including light/dark switches and
//! per-terminal color schemes) without the user configuring anything.

use std::path::PathBuf;

use gpui::{Hsla, Rgba};
use serde_json::{Value, json};

/// File name of the generated theme, under [`theme_dir`].
const THEME_FILE_NAME: &str = "terry-terminal.json";

/// Directory holding the generated theme.
fn theme_dir() -> PathBuf {
    paths::data_dir().join("coding_agent")
}

/// Path of the generated theme file.
pub fn theme_path() -> PathBuf {
    theme_dir().join(THEME_FILE_NAME)
}

/// Environment variable that carries the theme path to the agent.
pub const THEME_ENV: &str = "TERRY_AGENT_THEME";

/// The terminal palette, as the pieces the agent's theme needs.
///
/// Taken from the resolved terminal theme rather than the UI theme, so a
/// per-terminal color scheme is honored.
pub struct TerminalPalette {
    pub foreground: Hsla,
    pub background: Hsla,
    pub selection: Hsla,
    /// ANSI 0-7 (black, red, green, yellow, blue, magenta, cyan, white).
    pub normal: [Hsla; 8],
    /// ANSI 8-15 (their bright variants).
    pub bright: [Hsla; 8],
    /// Whether the palette is light-on-dark, which decides the `appearance`
    /// field the theme reports.
    pub is_dark: bool,
}

impl TerminalPalette {
    /// Reads the palette out of a resolved terminal theme.
    pub fn from_theme(theme: &theme::Theme) -> Self {
        let colors = &theme.styles.colors;
        Self {
            foreground: colors.terminal_foreground,
            background: colors.terminal_background,
            selection: theme
                .styles
                .player
                .0
                .first()
                .map(|player| player.selection)
                .unwrap_or_else(|| colors.terminal_foreground.opacity(0.25)),
            normal: [
                colors.terminal_ansi_black,
                colors.terminal_ansi_red,
                colors.terminal_ansi_green,
                colors.terminal_ansi_yellow,
                colors.terminal_ansi_blue,
                colors.terminal_ansi_magenta,
                colors.terminal_ansi_cyan,
                colors.terminal_ansi_white,
            ],
            bright: [
                colors.terminal_ansi_bright_black,
                colors.terminal_ansi_bright_red,
                colors.terminal_ansi_bright_green,
                colors.terminal_ansi_bright_yellow,
                colors.terminal_ansi_bright_blue,
                colors.terminal_ansi_bright_magenta,
                colors.terminal_ansi_bright_cyan,
                colors.terminal_ansi_bright_white,
            ],
            // `l` is perceptual lightness in gpui's Hsla, so the background is
            // the right thing to test.
            is_dark: colors.terminal_background.l < 0.5,
        }
    }

    /// Renders the palette as a pi theme JSON document.
    ///
    /// Only the tokens `tom-tui` actually consumes are set; the rest fall back
    /// to its defaults. Colors are emitted as `#rrggbb` (alpha is dropped: the
    /// theme format has no alpha channel, and the derived colors are opaque
    /// except the selection, which is blended onto the background instead).
    pub fn to_theme_json(&self) -> String {
        let fg = self.foreground;
        let bg = self.background;
        // Two families of derived tone, blended in opposite directions:
        //
        // - text tones start at the foreground and move *toward* the background
        //   (muted is dimmer than the foreground, dim dimmer still);
        // - surface tones start at the background and move slightly toward the
        //   foreground, so a panel is a subtle lift off the page rather than a
        //   near-foreground block.
        let muted = blend(fg, bg, 0.35);
        let dim = blend(fg, bg, 0.60);
        let surface = blend(bg, fg, 0.10);
        let border = blend(bg, fg, 0.28);
        let border_muted = blend(bg, fg, 0.16);

        let theme = json!({
            "name": "terry-terminal",
            "appearance": if self.is_dark { "dark" } else { "light" },
            "colors": {
                // Core text + chrome.
                "text": hex(fg),
                "muted": hex(muted),
                "dim": hex(dim),
                "accent": hex(self.bright[4]),      // bright blue
                "border": hex(border),
                "borderAccent": hex(self.bright[4]),
                "borderMuted": hex(border_muted),
                "selectedBg": hex(blend(bg, self.selection, 0.22)),
                "userMessageBg": hex(surface),

                // Semantic colors map onto the ANSI slots users already themed.
                "success": hex(self.normal[2]),     // green
                "error": hex(self.normal[1]),       // red
                "warning": hex(self.normal[3]),     // yellow

                // Reasoning blocks are de-emphasized, so they follow `muted`.
                "thinkingText": hex(muted),

                // Markdown, so agent prose matches the terminal's syntax colors.
                "mdHeading": hex(self.bright[3]),   // bright yellow
                "mdLink": hex(self.bright[4]),      // bright blue
                "mdLinkUrl": hex(dim),
                "mdCode": hex(self.bright[6]),      // bright cyan
                "mdCodeBlock": hex(fg),
                "mdCodeBlockBorder": hex(border_muted),
                "mdQuote": hex(muted),
                "mdQuoteBorder": hex(border),
                "mdHr": hex(border_muted),
                "mdListBullet": hex(self.normal[5]),// magenta

                // Tool output: the distinction users rely on when scanning a
                // transcript is success vs failure, so reuse those slots.
                "toolPendingBg": hex(surface),
                "toolSuccessBg": hex(blend(bg, self.normal[2], 0.14)),
                "toolErrorBg": hex(blend(bg, self.normal[1], 0.14)),
                "toolTitle": hex(fg),
                "toolOutput": hex(muted),

                // Diff view.
                "toolDiffAdded": hex(self.normal[2]),
                "toolDiffRemoved": hex(self.normal[1]),
                "toolDiffContext": hex(muted),

                // Custom (compaction notices and the like).
                "customMessageBg": hex(surface),
                "customMessageText": hex(fg),
                "customMessageLabel": hex(self.bright[4]),
            }
        });

        // Pretty-print: the file is small and being able to read it helps when
        // a palette produces an unexpected result.
        serde_json::to_string_pretty(&theme).unwrap_or_else(|_| theme.to_string())
    }
}

/// Writes an already-serialized theme, returning its path.
///
/// Separate from [`write_theme_file`] so a caller that keeps the serialized
/// form around (to skip rewrites) does not have to build the palette twice.
pub fn write_theme_json(json: &str) -> anyhow::Result<PathBuf> {
    let path = theme_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow::anyhow!("could not create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, json)
        .map_err(|error| anyhow::anyhow!("could not write {}: {error}", path.display()))?;
    Ok(path)
}

/// Writes the theme file for `palette`, returning its path.
///
/// A write failure is not fatal: the agent falls back to its built-in theme, so
/// the caller logs and carries on.
pub fn write_theme_file(palette: &TerminalPalette) -> anyhow::Result<PathBuf> {
    write_theme_json(&palette.to_theme_json())
}

/// `#rrggbb` for a color, dropping alpha.
fn hex(color: Hsla) -> String {
    let rgba: Rgba = color.into();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

/// Mixes `color` toward `toward` by `amount`, for derived tones.
fn blend(color: Hsla, toward: Hsla, amount: f32) -> Hsla {
    let amount = amount.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * amount;
    Hsla {
        h: color.h,
        s: lerp(color.s, toward.s),
        l: lerp(color.l, toward.l),
        a: color.a,
    }
}

/// The value to put in `TERRY_AGENT_THEME`, or `None` when no theme file exists.
pub fn theme_env_value() -> Option<String> {
    let path = theme_path();
    path.is_file().then(|| path.display().to_string())
}

/// Adds `TERRY_AGENT_THEME` to a terminal's environment when a theme exists.
///
/// Called wherever a terminal's env is assembled, so the agent launched from
/// that terminal finds the palette Terry is painting with.
pub fn insert_agent_theme_env(env: &mut collections::HashMap<String, String>) {
    if let Some(value) = theme_env_value() {
        env.insert(THEME_ENV.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::hsla;

    fn palette() -> TerminalPalette {
        TerminalPalette {
            foreground: hsla(0.0, 0.0, 0.9, 1.0),
            background: hsla(0.0, 0.0, 0.1, 1.0),
            selection: hsla(0.6, 0.8, 0.4, 1.0),
            normal: [
                hsla(0.0, 0.0, 0.1, 1.0),
                hsla(0.0, 0.8, 0.5, 1.0),
                hsla(0.33, 0.8, 0.5, 1.0),
                hsla(0.16, 0.8, 0.5, 1.0),
                hsla(0.6, 0.8, 0.5, 1.0),
                hsla(0.8, 0.8, 0.5, 1.0),
                hsla(0.5, 0.8, 0.5, 1.0),
                hsla(0.0, 0.0, 0.8, 1.0),
            ],
            bright: [
                hsla(0.0, 0.0, 0.3, 1.0),
                hsla(0.0, 0.9, 0.6, 1.0),
                hsla(0.33, 0.9, 0.6, 1.0),
                hsla(0.16, 0.9, 0.6, 1.0),
                hsla(0.6, 0.9, 0.6, 1.0),
                hsla(0.8, 0.9, 0.6, 1.0),
                hsla(0.5, 0.9, 0.6, 1.0),
                hsla(0.0, 0.0, 0.95, 1.0),
            ],
            is_dark: true,
        }
    }

    #[test]
    fn hex_drops_alpha_and_clamps() {
        assert_eq!(hex(hsla(0.0, 0.0, 1.0, 1.0)), "#ffffff");
        assert_eq!(hex(hsla(0.0, 0.0, 0.0, 0.5)), "#000000");
    }

    #[test]
    fn blend_moves_toward_the_target() {
        let dark = hsla(0.0, 0.0, 0.0, 1.0);
        let light = hsla(0.0, 0.0, 1.0, 1.0);
        let mid = blend(light, dark, 0.5);
        assert!((mid.l - 0.5).abs() < 0.001, "got {}", mid.l);
    }

    #[test]
    fn blend_is_clamped() {
        let dark = hsla(0.0, 0.0, 0.0, 1.0);
        let light = hsla(0.0, 0.0, 1.0, 1.0);
        assert!((blend(light, dark, 2.0).l - 0.0).abs() < 0.001);
        assert!((blend(light, dark, -1.0).l - 1.0).abs() < 0.001);
    }

    /// Parses the generated theme back into channel values for assertions.
    fn parsed_colors(json: &str) -> serde_json::Map<String, Value> {
        let value: Value = serde_json::from_str(json).expect("valid JSON");
        value["colors"].as_object().expect("colors object").clone()
    }

    fn luminance(hex: &str) -> u32 {
        let hex = hex.trim_start_matches('#');
        let channel = |range: std::ops::Range<usize>| {
            u32::from_str_radix(&hex[range], 16).expect("hex channel")
        };
        // Rough perceptual weight; enough to order light vs dark tones.
        (channel(0..2) * 30 + channel(2..4) * 59 + channel(4..6) * 11) / 100
    }

    #[test]
    fn surfaces_sit_closer_to_the_background_than_to_the_text() {
        // The bug this pins: a surface blended the wrong way becomes a bright
        // block on a dark theme, which is what `userMessageBg` was doing.
        let json = palette().to_theme_json();
        let colors = parsed_colors(&json);
        // The schema has no `background` token (tom paints the terminal's own),
        // so compare against the palette's background directly.
        let bg = luminance(&hex(palette().background));
        let fg = luminance(colors["text"].as_str().unwrap());
        for token in ["userMessageBg", "customMessageBg", "toolPendingBg"] {
            let surface = luminance(colors[token].as_str().unwrap());
            let to_bg = surface.abs_diff(bg);
            let to_fg = surface.abs_diff(fg);
            assert!(
                to_bg < to_fg,
                "{token} should be nearer the background ({bg}) than the text ({fg}), \
                 but was {surface}"
            );
        }
    }

    #[test]
    fn muted_text_is_dimmer_than_primary_text() {
        let json = palette().to_theme_json();
        let colors = parsed_colors(&json);
        let fg = luminance(colors["text"].as_str().unwrap());
        let muted = luminance(colors["muted"].as_str().unwrap());
        let dim = luminance(colors["dim"].as_str().unwrap());
        // On a dark theme "dimmer" means darker, so ordering is monotonic.
        assert!(fg > muted && muted > dim, "fg={fg} muted={muted} dim={dim}");
    }

    #[test]
    fn the_generated_theme_is_valid_pi_theme_json() {
        let text = palette().to_theme_json();
        // The strongest check: tom's own parser must accept it, otherwise the
        // agent would silently fall back to its default colors.
        let parsed = tom_tui::theme_json::ThemeJson::parse(&text)
            .expect("generated theme must parse as a pi theme");
        assert_eq!(parsed.name, "terry-terminal");
    }

    #[test]
    fn the_generated_theme_reports_the_right_appearance() {
        let dark = palette().to_theme_json();
        assert!(dark.contains("\"appearance\": \"dark\""));

        let mut light_palette = palette();
        light_palette.is_dark = false;
        assert!(
            light_palette
                .to_theme_json()
                .contains("\"appearance\": \"light\"")
        );
    }

    #[test]
    fn semantic_colors_come_from_the_ansi_slots() {
        let text = palette().to_theme_json();
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        let colors = value["colors"].as_object().expect("colors object");
        // `error` is ANSI red, which is distinct from `success` (green): a
        // palette that themed them apart must not collapse them.
        assert_ne!(colors["error"], colors["success"]);
        assert_ne!(colors["error"], colors["text"]);
    }

    #[test]
    fn every_emitted_color_is_a_hex_triplet() {
        let text = palette().to_theme_json();
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        for (token, color) in value["colors"].as_object().expect("colors object") {
            let text = color.as_str().expect("colors are strings");
            assert!(
                text.starts_with('#') && text.len() == 7,
                "token {token} is not #rrggbb: {text}"
            );
        }
    }

    #[test]
    fn the_theme_path_lives_under_the_data_dir() {
        let path = theme_path();
        assert!(path.starts_with(paths::data_dir()));
        assert!(path.ends_with(THEME_FILE_NAME));
    }

    #[test]
    fn writing_creates_the_file_and_the_env_value_points_at_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(THEME_FILE_NAME);
        std::fs::write(&path, palette().to_theme_json()).expect("write");
        assert!(path.is_file());
        // Sanity: the written file is parseable too.
        tom_tui::theme_json::ThemeJson::from_file(&path).expect("parse from file");
    }
}
