//! Terminal color schemes ("palettes").
//!
//! A terminal color scheme only changes terminal colors — ANSI palette,
//! foreground/background, cursor and selection — leaving the rest of the UI
//! theme untouched. Profiles and the color-scheme picker refer to these by
//! name; `TerminalView` recognizes a palette name before falling back to a
//! regular theme name, so both kinds of name work in `color_scheme`.
//!
//! Schemes come from three places, in increasing priority:
//!
//! 1. the built-in library (`terminal_schemes()`),
//! 2. user files in `<config>/terminal_schemes/*.{json,toml,itermcolors}`,
//! 3. schemes imported at runtime (the import action writes a user file and
//!    inserts the parsed scheme immediately).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use gpui::{App, Global, Hsla, Rgba, SharedString};
use theme::Theme;

/// A terminal-only color scheme.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalColorScheme {
    pub name: String,
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
    pub selection: Option<Hsla>,
    /// ANSI colors 0-7 (black, red, green, yellow, blue, magenta, cyan, white).
    pub normal: [Hsla; 8],
    /// ANSI colors 8-15 (their bright variants).
    pub bright: [Hsla; 8],
}

impl TerminalColorScheme {
    /// Returns a copy of `base` with every terminal color replaced by this
    /// scheme. The rest of the theme (syntax, status colors, UI colors) is
    /// preserved, so a per-terminal scheme never changes the surrounding UI.
    pub fn to_theme(&self, base: &Theme) -> Theme {
        let mut theme = base.clone();
        theme.id = format!("terminal-scheme:{}", self.name);
        theme.name = SharedString::from(self.name.clone());

        let colors = &mut theme.styles.colors;
        colors.terminal_background = self.background;
        colors.terminal_ansi_background = self.background;
        colors.terminal_foreground = self.foreground;
        colors.terminal_bright_foreground = self.foreground;
        colors.terminal_dim_foreground = self.foreground.opacity(0.7);
        // The wrapper paints padding with `editor_background`, so keep it in
        // sync with the palette background instead of the UI theme's.
        colors.editor_background = self.background;
        colors.editor_foreground = self.foreground;

        let normal = &self.normal;
        let bright = &self.bright;
        colors.terminal_ansi_black = normal[0];
        colors.terminal_ansi_red = normal[1];
        colors.terminal_ansi_green = normal[2];
        colors.terminal_ansi_yellow = normal[3];
        colors.terminal_ansi_blue = normal[4];
        colors.terminal_ansi_magenta = normal[5];
        colors.terminal_ansi_cyan = normal[6];
        colors.terminal_ansi_white = normal[7];

        colors.terminal_ansi_bright_black = bright[0];
        colors.terminal_ansi_bright_red = bright[1];
        colors.terminal_ansi_bright_green = bright[2];
        colors.terminal_ansi_bright_yellow = bright[3];
        colors.terminal_ansi_bright_blue = bright[4];
        colors.terminal_ansi_bright_magenta = bright[5];
        colors.terminal_ansi_bright_cyan = bright[6];
        colors.terminal_ansi_bright_white = bright[7];

        let dim = |color: Hsla| color.opacity(0.65);
        colors.terminal_ansi_dim_black = dim(normal[0]);
        colors.terminal_ansi_dim_red = dim(normal[1]);
        colors.terminal_ansi_dim_green = dim(normal[2]);
        colors.terminal_ansi_dim_yellow = dim(normal[3]);
        colors.terminal_ansi_dim_blue = dim(normal[4]);
        colors.terminal_ansi_dim_magenta = dim(normal[5]);
        colors.terminal_ansi_dim_cyan = dim(normal[6]);
        colors.terminal_ansi_dim_white = dim(normal[7]);

        if let Some(local) = theme.styles.player.0.first_mut() {
            local.cursor = self.cursor;
            local.selection = self
                .selection
                .unwrap_or_else(|| self.foreground.opacity(0.25));
        }

        theme
    }

    /// A single ANSI color, indexed 0-15.
    pub fn ansi(&self, index: usize) -> Hsla {
        match index {
            0..=7 => self.normal[index],
            8..=15 => self.bright[index - 8],
            _ => self.foreground,
        }
    }
}

/// All terminal color schemes known to the app.
#[derive(Default)]
pub struct ColorSchemeRegistry {
    schemes: RwLock<HashMap<String, TerminalColorScheme>>,
}

impl Global for ColorSchemeRegistry {}

impl ColorSchemeRegistry {
    /// The registry, if `init` has run.
    pub fn try_global(cx: &App) -> Option<&Self> {
        cx.try_global::<ColorSchemeRegistry>()
    }

    /// The registry. Panics if `init` has not run.
    pub fn global(cx: &App) -> &Self {
        cx.global::<ColorSchemeRegistry>()
    }

    /// Installs the registry with the built-in and user-provided schemes.
    /// Calling this more than once is a no-op.
    pub fn init(cx: &mut App) {
        if cx.try_global::<ColorSchemeRegistry>().is_some() {
            return;
        }
        let registry = ColorSchemeRegistry::default();
        for scheme in builtin_schemes() {
            registry.insert(scheme);
        }
        registry.load_user_schemes();
        cx.set_global(registry);
    }

    pub fn insert(&self, scheme: TerminalColorScheme) {
        self.schemes
            .write()
            .expect("color scheme registry poisoned")
            .insert(scheme.name.clone(), scheme);
    }

    pub fn get(&self, name: &str) -> Option<TerminalColorScheme> {
        self.schemes
            .read()
            .expect("color scheme registry poisoned")
            .get(name)
            .cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.schemes
            .read()
            .expect("color scheme registry poisoned")
            .contains_key(name)
    }

    /// Names of every registered scheme, sorted for stable pickers.
    pub fn names(&self) -> Vec<String> {
        let mut names = self
            .schemes
            .read()
            .expect("color scheme registry poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Loads every supported file from the user scheme directory. Files that
    /// cannot be parsed are skipped (a broken import must not break startup).
    pub fn load_user_schemes(&self) {
        let dir = user_scheme_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let Some(scheme) = parse_scheme_file(&path) else {
                log::warn!("skipping unrecognized terminal color scheme: {path:?}");
                continue;
            };
            self.insert(scheme);
        }
    }
}

/// Directory scanned for user-provided terminal color schemes.
pub fn user_scheme_dir() -> PathBuf {
    paths::config_dir().join("terminal_schemes")
}

/// Parses a scheme file by extension. The file stem is used as the name when
/// the format does not carry one.
pub fn parse_scheme_file(path: &Path) -> Option<TerminalColorScheme> {
    let text = fs::read_to_string(path).ok()?;
    let name = path
        .file_stem()
        .map(|name| name.to_string_lossy().to_string())?;
    match path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("json") => parse_json(&text, &name),
        Some("toml") => parse_alacritty(&text, &name),
        Some("itermcolors" | "plist") => parse_itermcolors(&text, &name),
        _ => None,
    }
}

/// Writes `scheme` into the user scheme directory, returning the file path.
pub fn save_user_scheme(scheme: &TerminalColorScheme) -> anyhow::Result<PathBuf> {
    let dir = user_scheme_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", sanitize_file_name(&scheme.name)));
    fs::write(&path, scheme.to_json())?;
    Ok(path)
}

fn sanitize_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "terminal-scheme".to_string()
    } else {
        trimmed
    }
}

impl TerminalColorScheme {
    /// Canonical JSON form used for user files (and re-import).
    pub fn to_json(&self) -> String {
        let file = SchemeFile {
            name: self.name.clone(),
            foreground: hex_string(self.foreground),
            background: hex_string(self.background),
            cursor: Some(hex_string(self.cursor)),
            selection: self.selection.map(hex_string),
            ansi: Some(
                self.normal
                    .iter()
                    .chain(self.bright.iter())
                    .map(|color| hex_string(*color))
                    .collect(),
            ),
            normal: None,
            bright: None,
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }
}

fn hex_string(color: Hsla) -> String {
    let rgba: Rgba = color.into();
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SchemeFile {
    name: String,
    foreground: String,
    background: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selection: Option<String>,
    /// Either 16 colors (normal then bright) or an 8 + 8 `normal`/`bright`
    /// split. Both forms are accepted on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ansi: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    normal: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bright: Option<Vec<String>>,
}

fn parse_json(text: &str, fallback_name: &str) -> Option<TerminalColorScheme> {
    let file: SchemeFile = serde_json::from_str(text).ok()?;
    let name = if file.name.trim().is_empty() {
        fallback_name.to_string()
    } else {
        file.name
    };
    let background = parse_color(&file.background)?;
    let foreground = parse_color(&file.foreground)?;
    let mut ansi: Vec<Hsla> = Vec::with_capacity(16);
    if let Some(colors) = file.ansi {
        ansi.extend(colors.iter().filter_map(|color| parse_color(color)));
    }
    if let Some(normal) = file.normal {
        ansi.extend(normal.iter().filter_map(|color| parse_color(color)));
    }
    if let Some(bright) = file.bright {
        ansi.extend(bright.iter().filter_map(|color| parse_color(color)));
    }
    if ansi.len() != 16 {
        return None;
    }
    Some(TerminalColorScheme {
        name,
        foreground,
        background,
        cursor: file
            .cursor
            .as_deref()
            .and_then(parse_color)
            .unwrap_or(foreground),
        selection: file.selection.as_deref().and_then(parse_color),
        normal: ansi[..8].try_into().ok()?,
        bright: ansi[8..16].try_into().ok()?,
    })
}

/// Alacritty's `[colors.*]` table. Alacritty declares ANSI colors as named
/// tables, which we map onto indexes 0-7 and 8-15.
fn parse_alacritty(text: &str, name: &str) -> Option<TerminalColorScheme> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let colors = value.get("colors")?;

    let get = |table: &toml::Value, key: &str| -> Option<Hsla> {
        table.get(key)?.as_str().and_then(parse_color)
    };

    let primary = colors.get("primary");
    let fallback = default_palette();
    let background = primary
        .and_then(|primary| get(primary, "background"))
        .unwrap_or(fallback.background);
    let foreground = primary
        .and_then(|primary| get(primary, "foreground"))
        .unwrap_or(fallback.foreground);

    let normal_table = colors.get("normal");
    let bright_table = colors.get("bright");
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];

    let mut normal = fallback.normal;
    let mut bright = fallback.bright;
    for (index, key) in names.iter().enumerate() {
        if let Some(color) = normal_table.and_then(|table| get(table, key)) {
            normal[index] = color;
        }
        if let Some(color) = bright_table.and_then(|table| get(table, key)) {
            bright[index] = color;
        }
    }

    let cursor = colors
        .get("cursor")
        .and_then(|cursor| get(cursor, "cursor"))
        .unwrap_or(foreground);
    let selection = colors
        .get("selection")
        .and_then(|selection| get(selection, "background"));

    Some(TerminalColorScheme {
        name: name.to_string(),
        foreground,
        background,
        cursor,
        selection,
        normal,
        bright,
    })
}

/// iTerm2 `.itermcolors` files are plists describing each ANSI color as a
/// separate `<dict>` of RGB components. Parsing is done with a small scanner
/// instead of pulling in a plist dependency.
fn parse_itermcolors(text: &str, name: &str) -> Option<TerminalColorScheme> {
    let mut parsed: HashMap<String, Hsla> = HashMap::new();
    let mut rest = text;
    while let Some(key_start) = rest.find("<key>") {
        rest = &rest[key_start + "<key>".len()..];
        let Some(key_end) = rest.find("</key>") else {
            break;
        };
        let key = rest[..key_end].trim().to_string();
        rest = &rest[key_end + "</key>".len()..];
        if !(key.ends_with(" Color") || key.ends_with(" Color sRGB")) {
            continue;
        }
        let Some(dict_end) = rest.find("</dict>") else {
            break;
        };
        let dict = &rest[..dict_end];
        rest = &rest[dict_end + "</dict>".len()..];
        let Some(red) = plist_component(dict, "Red Component") else {
            continue;
        };
        let Some(green) = plist_component(dict, "Green Component") else {
            continue;
        };
        let Some(blue) = plist_component(dict, "Blue Component") else {
            continue;
        };
        let alpha = plist_component(dict, "Alpha Component").unwrap_or(1.0);
        parsed.insert(key, rgba(red, green, blue, alpha));
    }

    let fallback = default_palette();
    // A plist with no recognizable color entries is not a scheme; reject it so
    // broken imports are reported instead of silently becoming "Default".
    if parsed.is_empty() {
        return None;
    }
    let background = parsed
        .get("Background Color")
        .copied()
        .unwrap_or(fallback.background);
    let foreground = parsed
        .get("Foreground Color")
        .copied()
        .unwrap_or(fallback.foreground);
    let cursor = parsed.get("Cursor Color").copied().unwrap_or(foreground);
    let selection = parsed
        .get("Selection Color")
        .copied()
        .or(fallback.selection);

    let mut normal = fallback.normal;
    let mut bright = fallback.bright;
    for index in 0..8 {
        if let Some(color) = parsed.get(&format!("Ansi {index} Color")) {
            normal[index] = *color;
        }
        if let Some(color) = parsed.get(&format!("Ansi {} Color", index + 8)) {
            bright[index] = *color;
        }
    }

    Some(TerminalColorScheme {
        name: name.to_string(),
        foreground,
        background,
        cursor,
        selection,
        normal,
        bright,
    })
}

fn plist_component(dict: &str, key: &str) -> Option<f32> {
    let needle = format!("<key>{key}</key>");
    let start = dict.find(&needle)? + needle.len();
    let rest = &dict[start..];
    let open = rest.find("<real>")? + "<real>".len();
    let close = rest[open..].find("</real>")?;
    rest[open..open + close].trim().parse().ok()
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> Hsla {
    Hsla::from(Rgba {
        r: r.clamp(0.0, 1.0),
        g: g.clamp(0.0, 1.0),
        b: b.clamp(0.0, 1.0),
        a: a.clamp(0.0, 1.0),
    })
}

fn parse_color(text: &str) -> Option<Hsla> {
    Rgba::try_from(text).ok().map(Hsla::from)
}

fn default_palette() -> TerminalColorScheme {
    scheme(
        "Default",
        "#282828",
        "#ebdbb2",
        "#ebdbb2",
        Some("#504945"),
        [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    )
}

fn scheme(
    name: &str,
    background: &str,
    foreground: &str,
    cursor: &str,
    selection: Option<&str>,
    ansi: [&str; 16],
) -> TerminalColorScheme {
    let color = |hex: &str| {
        parse_color(hex).unwrap_or_else(|| {
            log::error!("invalid built-in terminal color {hex}");
            Hsla::from(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            })
        })
    };
    let mut colors = [Hsla::from(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    }); 16];
    for (slot, hex) in colors.iter_mut().zip(ansi) {
        *slot = color(hex);
    }
    TerminalColorScheme {
        name: name.to_string(),
        foreground: color(foreground),
        background: color(background),
        cursor: color(cursor),
        selection: selection.map(color),
        normal: colors[..8].try_into().expect("8 normal colors"),
        bright: colors[8..].try_into().expect("8 bright colors"),
    }
}

/// The built-in scheme library. Only ANSI palettes live here; names are stable
/// because profiles and session files refer to them.
pub fn builtin_schemes() -> Vec<TerminalColorScheme> {
    vec![
        scheme(
            "Dracula",
            "#282a36",
            "#f8f8f2",
            "#f8f8f2",
            Some("#44475a"),
            [
                "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd",
                "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df",
                "#a4ffff", "#ffffff",
            ],
        ),
        scheme(
            "Nord",
            "#2e3440",
            "#d8dee9",
            "#d8dee9",
            Some("#434c5e"),
            [
                "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead",
                "#8fbcbb", "#eceff4",
            ],
        ),
        scheme(
            "Solarized Dark",
            "#002b36",
            "#839496",
            "#93a1a1",
            Some("#073642"),
            [
                "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                "#93a1a1", "#fdf6e3",
            ],
        ),
        scheme(
            "Solarized Light",
            "#fdf6e3",
            "#657b83",
            "#586e75",
            Some("#eee8d5"),
            [
                "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                "#93a1a1", "#fdf6e3",
            ],
        ),
        scheme(
            "Gruvbox Dark",
            "#282828",
            "#ebdbb2",
            "#ebdbb2",
            Some("#504945"),
            [
                "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a",
                "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b",
                "#8ec07c", "#ebdbb2",
            ],
        ),
        scheme(
            "Gruvbox Light",
            "#fbf1c7",
            "#3c3836",
            "#3c3836",
            Some("#d5c4a1"),
            [
                "#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a",
                "#7c6f64", "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71",
                "#427b58", "#3c3836",
            ],
        ),
        scheme(
            "One Dark",
            "#282c34",
            "#abb2bf",
            "#528bff",
            Some("#3e4451"),
            [
                "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2",
                "#abb2bf", "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
                "#56b6c2", "#ffffff",
            ],
        ),
        scheme(
            "Monokai",
            "#272822",
            "#f8f8f2",
            "#f8f8f0",
            Some("#49483e"),
            [
                "#272822", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4",
                "#f8f8f2", "#75715e", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff",
                "#a1efe4", "#f9f8f5",
            ],
        ),
        scheme(
            "Tokyo Night",
            "#1a1b26",
            "#c0caf5",
            "#c0caf5",
            Some("#33467c"),
            [
                "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff",
                "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7",
                "#7dcfff", "#c0caf5",
            ],
        ),
        scheme(
            "Catppuccin Mocha",
            "#1e1e2e",
            "#cdd6f4",
            "#f5e0dc",
            Some("#585b70"),
            [
                "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5",
                "#bac2de", "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7",
                "#94e2d5", "#a6adc8",
            ],
        ),
        scheme(
            "GitHub Dark",
            "#0d1117",
            "#c9d1d9",
            "#c9d1d9",
            Some("#3b5070"),
            [
                "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf",
                "#b1bac4", "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff",
                "#56d4dd", "#f0f6fc",
            ],
        ),
        scheme(
            "GitHub Light",
            "#ffffff",
            "#24292f",
            "#24292f",
            Some("#b6c8d9"),
            [
                "#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83",
                "#6e7781", "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9",
                "#3192aa", "#8c959f",
            ],
        ),
        scheme(
            "Rosé Pine",
            "#191724",
            "#e0def4",
            "#e0def4",
            Some("#403d52"),
            [
                "#26233a", "#eb6f92", "#9ccfd8", "#f6c177", "#31748f", "#c4a7e7", "#ebbcba",
                "#e0def4", "#6e6a86", "#eb6f92", "#9ccfd8", "#f6c177", "#31748f", "#c4a7e7",
                "#ebbcba", "#e0def4",
            ],
        ),
        scheme(
            "Everforest Dark",
            "#2d353b",
            "#d3c6aa",
            "#d3c6aa",
            Some("#475258"),
            [
                "#475258", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092",
                "#d3c6aa", "#7a8478", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6",
                "#83c092", "#f2efdf",
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use theme::ActiveTheme;

    #[test]
    fn builtin_library_is_complete_and_unique() {
        let schemes = builtin_schemes();
        assert!(schemes.len() >= 10, "expected a usable library");
        let mut names = schemes
            .iter()
            .map(|scheme| scheme.name.clone())
            .collect::<Vec<_>>();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "scheme names must be unique");
        for scheme in &schemes {
            assert_ne!(
                scheme.foreground, scheme.background,
                "{} is invisible",
                scheme.name
            );
        }
    }

    #[test]
    fn parses_alacritty_toml() {
        let text = r##"
            [colors.primary]
            background = "#101010"
            foreground = "#f0f0f0"

            [colors.cursor]
            cursor = "#ff0000"

            [colors.selection]
            background = "#333333"

            [colors.normal]
            black = "#000000"
            red = "#cc0000"
            green = "#00cc00"
            yellow = "#cccc00"
            blue = "#0000cc"
            magenta = "#cc00cc"
            cyan = "#00cccc"
            white = "#cccccc"

            [colors.bright]
            black = "#555555"
            red = "#ff5555"
            green = "#55ff55"
            yellow = "#ffff55"
            blue = "#5555ff"
            magenta = "#ff55ff"
            cyan = "#55ffff"
            white = "#ffffff"
        "##;
        let scheme = parse_alacritty(text, "Alacritty Test").unwrap();
        assert_eq!(scheme.name, "Alacritty Test");
        assert_eq!(scheme.foreground, parse_color("#f0f0f0").unwrap());
        assert_eq!(scheme.background, parse_color("#101010").unwrap());
        assert_eq!(scheme.cursor, parse_color("#ff0000").unwrap());
        assert_eq!(scheme.normal[1], parse_color("#cc0000").unwrap());
        assert_eq!(scheme.bright[7], parse_color("#ffffff").unwrap());
    }

    #[test]
    fn parses_itermcolors_plist() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Ansi 0 Color</key>
    <dict>
        <key>Red Component</key><real>0.0</real>
        <key>Green Component</key><real>0.0</real>
        <key>Blue Component</key><real>0.0</real>
    </dict>
    <key>Ansi 1 Color</key>
    <dict>
        <key>Red Component</key><real>1.0</real>
        <key>Green Component</key><real>0.0</real>
        <key>Blue Component</key><real>0.0</real>
    </dict>
    <key>Background Color</key>
    <dict>
        <key>Red Component</key><real>0.1</real>
        <key>Green Component</key><real>0.1</real>
        <key>Blue Component</key><real>0.1</real>
    </dict>
    <key>Foreground Color</key>
    <dict>
        <key>Red Component</key><real>0.9</real>
        <key>Green Component</key><real>0.9</real>
        <key>Blue Component</key><real>0.9</real>
    </dict>
</dict>
</plist>"#;
        let scheme = parse_itermcolors(text, "iTerm Test").unwrap();
        assert_eq!(scheme.background, rgba(0.1, 0.1, 0.1, 1.0));
        assert_eq!(scheme.foreground, rgba(0.9, 0.9, 0.9, 1.0));
        assert_eq!(scheme.normal[1], rgba(1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn json_round_trip() {
        let scheme = builtin_schemes()
            .into_iter()
            .find(|scheme| scheme.name == "Nord")
            .unwrap();
        let parsed = parse_json(&scheme.to_json(), "ignored").unwrap();
        assert_eq!(parsed.name, "Nord");
        assert_eq!(parsed.foreground, scheme.foreground);
        assert_eq!(parsed.background, scheme.background);
        assert_eq!(parsed.normal, scheme.normal);
        assert_eq!(parsed.bright, scheme.bright);
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(parse_json("{}", "x").is_none());
        assert!(parse_alacritty("not toml", "x").is_none());
        assert!(parse_itermcolors("<plist></plist>", "x").is_none());
    }

    #[gpui::test]
    fn registry_installs_the_builtin_library(cx: &mut TestAppContext) {
        cx.update(ColorSchemeRegistry::init);
        cx.update(|cx| {
            let registry = ColorSchemeRegistry::global(cx);
            assert!(registry.contains("Dracula"));
            assert!(registry.contains("Nord"));
            assert!(registry.get("Solarized Dark").is_some());
        });
    }

    #[gpui::test]
    fn scheme_overrides_terminal_colors_only(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = settings::SettingsStore::test(cx);
            cx.set_global(store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let scheme = builtin_schemes()
            .into_iter()
            .find(|scheme| scheme.name == "Nord")
            .unwrap();
        let base = cx.update(|cx| cx.theme().clone());
        let themed = scheme.to_theme(&base);

        assert_eq!(themed.colors().terminal_ansi_background, scheme.background);
        assert_eq!(themed.colors().terminal_foreground, scheme.foreground);
        assert_eq!(themed.colors().terminal_ansi_red, scheme.normal[1]);
        assert_eq!(themed.colors().terminal_ansi_bright_cyan, scheme.bright[6]);
        assert_eq!(themed.players().local().cursor, scheme.cursor);
        // The terminal element's own background and padding use these.
        assert_eq!(themed.colors().editor_background, scheme.background);
        // UI colors must be untouched.
        assert_eq!(themed.colors().border, base.colors().border);
        assert_eq!(themed.colors().text, base.colors().text);
    }
}
