//! An interactive theme generator.
//!
//! The window is split in two: a live preview of the generated theme on the
//! left, and controls for picking a color for each theme role on the right.
//!
//! Generated themes are written to the user's theme directory and applied by
//! updating the `theme` setting, so they persist across restarts.

use std::collections::HashMap;
use std::sync::Arc;

use editor::Editor;
use fs::Fs;
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
    actions, canvas, div, hsla, prelude::*, px, relative, size,
};
use platform_title_bar::PlatformTitleBar;
use release_channel::ReleaseChannel;
use serde_json::json;
use settings::{SettingsStore, update_settings_file};
use theme::{Appearance, SystemAppearance, ThemeRegistry, try_parse_color};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Label, LabelSize, ToggleButtonGroup,
    ToggleButtonGroupStyle, ToggleButtonSimple, prelude::*,
};
use util::ResultExt;

use crate::window_title_bar::{new_window_title_bar, render_window_title_bar};

actions!(theme_generator, [OpenThemeGenerator]);

/// The palette roles that can be customized in the generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Role {
    Background,
    EditorBackground,
    PanelBackground,
    SurfaceBackground,
    Border,
    Text,
    TextMuted,
    Accent,
    Selection,
    Hover,
    TitleBar,
    TabBar,
    StatusBar,
    Success,
    Warning,
    Error,
    Info,
    Keyword,
    Str,
    Comment,
    Function,
    TypeName,
    Number,
    Constant,
    Variable,
    Operator,
    Property,
    Tag,
    Attribute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoleGroup {
    Interface,
    Window,
    Status,
    Syntax,
}

impl RoleGroup {
    const ALL: [RoleGroup; 4] = [
        RoleGroup::Interface,
        RoleGroup::Window,
        RoleGroup::Status,
        RoleGroup::Syntax,
    ];

    fn label(self) -> String {
        match self {
            RoleGroup::Interface => i18n::t("theme_generator_group_interface"),
            RoleGroup::Window => i18n::t("theme_generator_group_window"),
            RoleGroup::Status => i18n::t("theme_generator_group_status"),
            RoleGroup::Syntax => i18n::t("theme_generator_group_syntax"),
        }
    }
}

impl Role {
    const ALL: [Role; 29] = [
        Role::Background,
        Role::EditorBackground,
        Role::PanelBackground,
        Role::SurfaceBackground,
        Role::Border,
        Role::Text,
        Role::TextMuted,
        Role::Accent,
        Role::Selection,
        Role::Hover,
        Role::TitleBar,
        Role::TabBar,
        Role::StatusBar,
        Role::Success,
        Role::Warning,
        Role::Error,
        Role::Info,
        Role::Keyword,
        Role::Str,
        Role::Comment,
        Role::Function,
        Role::TypeName,
        Role::Number,
        Role::Constant,
        Role::Variable,
        Role::Operator,
        Role::Property,
        Role::Tag,
        Role::Attribute,
    ];

    fn key(self) -> &'static str {
        match self {
            Role::Background => "background",
            Role::EditorBackground => "editor.background",
            Role::PanelBackground => "panel.background",
            Role::SurfaceBackground => "surface.background",
            Role::Border => "border",
            Role::Text => "text",
            Role::TextMuted => "text.muted",
            Role::Accent => "text.accent",
            Role::Selection => "element.selection_background",
            Role::Hover => "element.hover",
            Role::TitleBar => "title_bar.background",
            Role::TabBar => "tab_bar.background",
            Role::StatusBar => "status_bar.background",
            Role::Success => "success",
            Role::Warning => "warning",
            Role::Error => "error",
            Role::Info => "info",
            Role::Keyword => "syntax.keyword",
            Role::Str => "syntax.string",
            Role::Comment => "syntax.comment",
            Role::Function => "syntax.function",
            Role::TypeName => "syntax.type",
            Role::Number => "syntax.number",
            Role::Constant => "syntax.constant",
            Role::Variable => "syntax.variable",
            Role::Operator => "syntax.operator",
            Role::Property => "syntax.property",
            Role::Tag => "syntax.tag",
            Role::Attribute => "syntax.attribute",
        }
    }

    fn group(self) -> RoleGroup {
        match self {
            Role::Background
            | Role::EditorBackground
            | Role::PanelBackground
            | Role::SurfaceBackground
            | Role::Border
            | Role::Text
            | Role::TextMuted
            | Role::Accent
            | Role::Selection
            | Role::Hover => RoleGroup::Interface,
            Role::TitleBar | Role::TabBar | Role::StatusBar => RoleGroup::Window,
            Role::Success | Role::Warning | Role::Error | Role::Info => RoleGroup::Status,
            Role::Keyword
            | Role::Str
            | Role::Comment
            | Role::Function
            | Role::TypeName
            | Role::Number
            | Role::Constant
            | Role::Variable
            | Role::Operator
            | Role::Property
            | Role::Tag
            | Role::Attribute => RoleGroup::Syntax,
        }
    }
}

/// The color channels that can be edited with the sliders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SliderKind {
    Hue,
    Saturation,
    Lightness,
    Alpha,
}

impl SliderKind {
    const ALL: [SliderKind; 4] = [
        SliderKind::Hue,
        SliderKind::Saturation,
        SliderKind::Lightness,
        SliderKind::Alpha,
    ];

    fn label(self) -> String {
        match self {
            SliderKind::Hue => i18n::t("theme_generator_hue"),
            SliderKind::Saturation => i18n::t("theme_generator_saturation"),
            SliderKind::Lightness => i18n::t("theme_generator_lightness"),
            SliderKind::Alpha => i18n::t("theme_generator_opacity"),
        }
    }

    fn value(self, color: Hsla) -> f32 {
        match self {
            SliderKind::Hue => color.h,
            SliderKind::Saturation => color.s,
            SliderKind::Lightness => color.l,
            SliderKind::Alpha => color.a,
        }
    }

    fn display(self, value: f32) -> String {
        match self {
            SliderKind::Hue => format!("{:.0}°", value * 360.0),
            _ => format!("{:.0}%", value * 100.0),
        }
    }

    /// The color shown at position `t` of this slider's track.
    fn sample(self, color: Hsla, t: f32) -> Hsla {
        match self {
            SliderKind::Hue => hsla(t, color.s.max(0.55), color.l.clamp(0.35, 0.7), 1.0),
            SliderKind::Saturation => hsla(color.h, t, color.l, 1.0),
            SliderKind::Lightness => hsla(color.h, color.s, t, 1.0),
            SliderKind::Alpha => hsla(color.h, color.s, color.l, t),
        }
    }
}

fn parse_color(hex: &str) -> Hsla {
    try_parse_color(hex).unwrap_or_else(|_| hsla(0.0, 0.0, 0.0, 1.0))
}

fn hex_string(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let r = (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8;
    let a = (rgba.a * 255.0).round().clamp(0.0, 255.0) as u8;

    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    hsla(color.h, color.s, color.l, alpha.clamp(0.0, 1.0))
}

fn shift_lightness(color: Hsla, delta: f32) -> Hsla {
    hsla(color.h, color.s, (color.l + delta).clamp(0.0, 1.0), color.a)
}

/// Maps a syntax token from the theme schema to the role that drives its color.
fn syntax_role(token: &str) -> Role {
    match token {
        "boolean" | "number" => Role::Number,
        "comment" | "comment.doc" => Role::Comment,
        "constructor" | "function" => Role::Function,
        "constant" | "variant" | "enum" => Role::Constant,
        "keyword" | "preproc" => Role::Keyword,
        "label" | "link_text" | "link_uri" => Role::Accent,
        "namespace" | "type" => Role::TypeName,
        "operator" => Role::Operator,
        "property" => Role::Property,
        "tag" | "selector" | "selector.pseudo" => Role::Tag,
        "attribute" => Role::Attribute,
        "text.literal" | "string" => Role::Str,
        "diff.plus" => Role::Success,
        "diff.minus" => Role::Error,
        "hint" | "predictive" => Role::TextMuted,
        _ if token.starts_with("string") => Role::Str,
        _ if token.starts_with("variable") => Role::Variable,
        _ if token.starts_with("punctuation") => Role::TextMuted,
        _ => Role::Text,
    }
}

/// Syntax tokens covered by generated themes. Mirrors the token set used by the
/// bundled themes so that every language highlights consistently.
const SYNTAX_TOKENS: [&str; 46] = [
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constructor",
    "embedded",
    "emphasis",
    "emphasis.strong",
    "enum",
    "function",
    "hint",
    "keyword",
    "label",
    "link_text",
    "link_uri",
    "namespace",
    "number",
    "operator",
    "predictive",
    "preproc",
    "primary",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.list_marker",
    "punctuation.markup",
    "punctuation.special",
    "selector",
    "selector.pseudo",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.literal",
    "title",
    "type",
    "variable",
    "variable.parameter",
    "variable.special",
    "variant",
    "diff.plus",
    "diff.minus",
];

/// A set of colors for a single theme.
#[derive(Clone)]
struct Palette {
    appearance: Appearance,
    colors: HashMap<Role, Hsla>,
}

impl Palette {
    fn new(appearance: Appearance) -> Self {
        let mut colors = HashMap::default();
        let entries: &[(Role, &str)] = match appearance {
            Appearance::Dark => &[
                (Role::Background, "#1e1e2e"),
                (Role::EditorBackground, "#1e1e2e"),
                (Role::PanelBackground, "#181825"),
                (Role::SurfaceBackground, "#313244"),
                (Role::Border, "#45475a"),
                (Role::Text, "#cdd6f4"),
                (Role::TextMuted, "#a6adc8"),
                (Role::Accent, "#89b4fa"),
                (Role::Selection, "#585b70"),
                (Role::Hover, "#313244"),
                (Role::TitleBar, "#181825"),
                (Role::TabBar, "#181825"),
                (Role::StatusBar, "#181825"),
                (Role::Success, "#a6e3a1"),
                (Role::Warning, "#f9e2af"),
                (Role::Error, "#f38ba8"),
                (Role::Info, "#89dceb"),
                (Role::Keyword, "#cba6f7"),
                (Role::Str, "#a6e3a1"),
                (Role::Comment, "#6c7086"),
                (Role::Function, "#89b4fa"),
                (Role::TypeName, "#f9e2af"),
                (Role::Number, "#fab387"),
                (Role::Constant, "#f2cdcd"),
                (Role::Variable, "#cdd6f4"),
                (Role::Operator, "#89dceb"),
                (Role::Property, "#b4befe"),
                (Role::Tag, "#f38ba8"),
                (Role::Attribute, "#f9e2af"),
            ],
            Appearance::Light => &[
                (Role::Background, "#eff1f5"),
                (Role::EditorBackground, "#eff1f5"),
                (Role::PanelBackground, "#e6e9ef"),
                (Role::SurfaceBackground, "#ccd0da"),
                (Role::Border, "#bcc0cc"),
                (Role::Text, "#4c4f69"),
                (Role::TextMuted, "#6c6f85"),
                (Role::Accent, "#1e66f5"),
                (Role::Selection, "#acb0be"),
                (Role::Hover, "#dce0e8"),
                (Role::TitleBar, "#e6e9ef"),
                (Role::TabBar, "#e6e9ef"),
                (Role::StatusBar, "#e6e9ef"),
                (Role::Success, "#40a02b"),
                (Role::Warning, "#df8e1d"),
                (Role::Error, "#d20f39"),
                (Role::Info, "#04a5e5"),
                (Role::Keyword, "#8839ef"),
                (Role::Str, "#40a02b"),
                (Role::Comment, "#9ca0b0"),
                (Role::Function, "#1e66f5"),
                (Role::TypeName, "#df8e1d"),
                (Role::Number, "#fe640b"),
                (Role::Constant, "#d20f39"),
                (Role::Variable, "#4c4f69"),
                (Role::Operator, "#04a5e5"),
                (Role::Property, "#7287fd"),
                (Role::Tag, "#d20f39"),
                (Role::Attribute, "#df8e1d"),
            ],
        };

        for (role, hex) in entries {
            colors.insert(*role, parse_color(hex));
        }

        Self { appearance, colors }
    }

    fn color(&self, role: Role) -> Hsla {
        self.colors
            .get(&role)
            .copied()
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.0, 1.0))
    }

    fn hex(&self, role: Role) -> String {
        hex_string(self.color(role))
    }

    fn set_color(&mut self, role: Role, color: Hsla) {
        self.colors.insert(role, color);
    }

    /// Serializes the palette into a Zed theme file.
    fn to_theme_json(&self, name: &str) -> serde_json::Value {
        let color = |role: Role| self.color(role);
        let hex = |role: Role| self.hex(role);
        let translucent = |role: Role, alpha: f32| hex_string(with_alpha(color(role), alpha));
        let ansi = |role: Role| {
            (
                hex_string(color(role)),
                hex_string(shift_lightness(color(role), 0.12)),
                hex_string(shift_lightness(color(role), -0.12)),
            )
        };

        let status = |role: Role| {
            (
                hex_string(color(role)),
                hex_string(with_alpha(color(role), 0.15)),
                hex_string(with_alpha(color(role), 0.35)),
            )
        };

        let (success, success_bg, success_border) = status(Role::Success);
        let (warning, warning_bg, warning_border) = status(Role::Warning);
        let (error, error_bg, error_border) = status(Role::Error);
        let (info, info_bg, info_border) = status(Role::Info);
        let (conflict, conflict_bg, conflict_border) = status(Role::Warning);
        let (created, created_bg, created_border) = status(Role::Success);
        let (modified, modified_bg, modified_border) = status(Role::Warning);
        let (deleted, deleted_bg, deleted_border) = status(Role::Error);
        let (hidden, hidden_bg, hidden_border) = status(Role::TextMuted);
        let (hint, hint_bg, hint_border) = status(Role::Info);
        let (ignored, ignored_bg, ignored_border) = status(Role::TextMuted);
        let (predictive, predictive_bg, predictive_border) = status(Role::Info);
        let (renamed, renamed_bg, renamed_border) = status(Role::Accent);
        let (unreachable, unreachable_bg, unreachable_border) = status(Role::TextMuted);

        let (red, bright_red, dim_red) = ansi(Role::Error);
        let (green, bright_green, dim_green) = ansi(Role::Success);
        let (yellow, bright_yellow, dim_yellow) = ansi(Role::Warning);
        let (blue, bright_blue, dim_blue) = ansi(Role::Accent);
        let (magenta, bright_magenta, dim_magenta) = ansi(Role::Keyword);
        let (cyan, bright_cyan, dim_cyan) = ansi(Role::Info);
        let (white, bright_white, dim_white) = ansi(Role::Text);

        let mut syntax = serde_json::Map::new();
        for token in SYNTAX_TOKENS {
            let mut entry = serde_json::Map::new();
            entry.insert("color".to_string(), json!(hex(syntax_role(token))));
            if token.starts_with("comment") {
                entry.insert("font_style".to_string(), json!("italic"));
            }
            syntax.insert(token.to_string(), serde_json::Value::Object(entry));
        }

        let player_roles = [
            Role::Accent,
            Role::Success,
            Role::Warning,
            Role::Error,
            Role::Info,
            Role::Keyword,
            Role::Function,
            Role::TypeName,
        ];
        let players: Vec<serde_json::Value> = player_roles
            .iter()
            .map(|role| {
                let color = color(*role);
                json!({
                    "cursor": hex_string(color),
                    "background": hex_string(color),
                    "selection": hex_string(with_alpha(color, 0.35)),
                })
            })
            .collect();
        let accents: Vec<String> = player_roles.iter().map(|role| hex(*role)).collect();

        let style = json!({
            "background.appearance": "opaque",
            "background": hex(Role::Background),
            "editor.background": hex(Role::EditorBackground),
            "editor.foreground": hex(Role::Text),
            "editor.gutter.background": hex(Role::EditorBackground),
            "editor.subheader.background": hex(Role::PanelBackground),
            "editor.active_line.background": translucent(Role::Hover, 0.5),
            "editor.highlighted_line.background": translucent(Role::Hover, 0.35),
            "editor.line_number": hex(Role::TextMuted),
            "editor.active_line_number": hex(Role::Text),
            "editor.hover_line_number": hex(Role::TextMuted),
            "editor.invisible": translucent(Role::TextMuted, 0.5),
            "editor.wrap_guide": hex(Role::Border),
            "editor.active_wrap_guide": hex(Role::TextMuted),
            "editor.document_highlight.read_background": translucent(Role::Accent, 0.15),
            "editor.document_highlight.write_background": translucent(Role::Accent, 0.25),
            "surface.background": hex(Role::SurfaceBackground),
            "elevated_surface.background": hex(Role::SurfaceBackground),
            "panel.background": hex(Role::PanelBackground),
            "panel.focused_border": hex(Role::Accent),
            "pane.focused_border": hex(Role::Border),
            "element.background": hex(Role::PanelBackground),
            "element.hover": hex(Role::Hover),
            "element.active": hex(Role::Selection),
            "element.selected": hex(Role::Selection),
            "element.disabled": hex(Role::PanelBackground),
            "element.selection_background": hex(Role::Selection),
            "drop_target.background": translucent(Role::Accent, 0.25),
            "ghost_element.background": "#00000000",
            "ghost_element.hover": hex(Role::Hover),
            "ghost_element.active": hex(Role::Selection),
            "ghost_element.selected": hex(Role::Selection),
            "ghost_element.disabled": "#00000000",
            "border": hex(Role::Border),
            "border.variant": translucent(Role::Border, 0.6),
            "border.focused": hex(Role::Accent),
            "border.selected": hex(Role::Accent),
            "border.transparent": "#00000000",
            "border.disabled": translucent(Role::Border, 0.5),
            "text": hex(Role::Text),
            "text.muted": hex(Role::TextMuted),
            "text.placeholder": hex(Role::TextMuted),
            "text.disabled": translucent(Role::TextMuted, 0.6),
            "text.accent": hex(Role::Accent),
            "icon": hex(Role::Text),
            "icon.muted": hex(Role::TextMuted),
            "icon.disabled": translucent(Role::TextMuted, 0.6),
            "icon.placeholder": hex(Role::TextMuted),
            "icon.accent": hex(Role::Accent),
            "title_bar.background": hex(Role::TitleBar),
            "title_bar.inactive_background": hex(Role::TitleBar),
            "status_bar.background": hex(Role::StatusBar),
            "toolbar.background": hex(Role::TabBar),
            "tab_bar.background": hex(Role::TabBar),
            "tab.inactive_background": hex(Role::TabBar),
            "tab.active_background": hex(Role::Background),
            "search.match_background": translucent(Role::Warning, 0.35),
            "search.active_match_background": translucent(Role::Warning, 0.55),
            "scrollbar.thumb.background": translucent(Role::TextMuted, 0.4),
            "scrollbar.thumb.hover_background": translucent(Role::TextMuted, 0.6),
            "scrollbar.thumb.border": "#00000000",
            "scrollbar.track.background": "#00000000",
            "scrollbar.track.border": "#00000000",
            "link_text.hover": hex(Role::Accent),
            "terminal.background": hex(Role::EditorBackground),
            "terminal.foreground": hex(Role::Text),
            "terminal.bright_foreground": hex(Role::Text),
            "terminal.dim_foreground": hex(Role::TextMuted),
            "terminal.ansi.black": hex(Role::PanelBackground),
            "terminal.ansi.bright_black": hex(Role::Border),
            "terminal.ansi.dim_black": hex(Role::TextMuted),
            "terminal.ansi.red": red,
            "terminal.ansi.bright_red": bright_red,
            "terminal.ansi.dim_red": dim_red,
            "terminal.ansi.green": green,
            "terminal.ansi.bright_green": bright_green,
            "terminal.ansi.dim_green": dim_green,
            "terminal.ansi.yellow": yellow,
            "terminal.ansi.bright_yellow": bright_yellow,
            "terminal.ansi.dim_yellow": dim_yellow,
            "terminal.ansi.blue": blue,
            "terminal.ansi.bright_blue": bright_blue,
            "terminal.ansi.dim_blue": dim_blue,
            "terminal.ansi.magenta": magenta,
            "terminal.ansi.bright_magenta": bright_magenta,
            "terminal.ansi.dim_magenta": dim_magenta,
            "terminal.ansi.cyan": cyan,
            "terminal.ansi.bright_cyan": bright_cyan,
            "terminal.ansi.dim_cyan": dim_cyan,
            "terminal.ansi.white": white,
            "terminal.ansi.bright_white": bright_white,
            "terminal.ansi.dim_white": dim_white,
            "version_control.added": hex(Role::Success),
            "version_control.modified": hex(Role::Warning),
            "version_control.word_added": hex(Role::Success),
            "version_control.word_deleted": hex(Role::Error),
            "version_control.deleted": hex(Role::Error),
            "version_control.conflict_marker.ours": hex(Role::Success),
            "version_control.conflict_marker.theirs": hex(Role::Info),
            "conflict": conflict,
            "conflict.background": conflict_bg,
            "conflict.border": conflict_border,
            "created": created,
            "created.background": created_bg,
            "created.border": created_border,
            "modified": modified,
            "modified.background": modified_bg,
            "modified.border": modified_border,
            "deleted": deleted,
            "deleted.background": deleted_bg,
            "deleted.border": deleted_border,
            "hidden": hidden,
            "hidden.background": hidden_bg,
            "hidden.border": hidden_border,
            "hint": hint,
            "hint.background": hint_bg,
            "hint.border": hint_border,
            "ignored": ignored,
            "ignored.background": ignored_bg,
            "ignored.border": ignored_border,
            "predictive": predictive,
            "predictive.background": predictive_bg,
            "predictive.border": predictive_border,
            "renamed": renamed,
            "renamed.background": renamed_bg,
            "renamed.border": renamed_border,
            "unreachable": unreachable,
            "unreachable.background": unreachable_bg,
            "unreachable.border": unreachable_border,
            "success": success,
            "success.background": success_bg,
            "success.border": success_border,
            "warning": warning,
            "warning.background": warning_bg,
            "warning.border": warning_border,
            "error": error,
            "error.background": error_bg,
            "error.border": error_border,
            "info": info,
            "info.background": info_bg,
            "info.border": info_border,
            "accents": accents,
            "players": players,
            "syntax": serde_json::Value::Object(syntax),
        });

        json!({
            "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
            "name": name,
            "author": "Terry",
            "themes": [{
                "name": name,
                "appearance": match self.appearance {
                    Appearance::Dark => "dark",
                    Appearance::Light => "light",
                },
                "style": style,
            }],
        })
    }
}

const PRESET_COLORS: [&str; 16] = [
    "#11111b", "#1e1e2e", "#313244", "#45475a", "#6c7086", "#a6adc8", "#cdd6f4", "#f5e0dc",
    "#f38ba8", "#fab387", "#f9e2af", "#a6e3a1", "#94e2d5", "#89b4fa", "#cba6f7", "#b4befe",
];

pub struct ThemeGenerator {
    focus_handle: FocusHandle,
    title_bar: Option<Entity<PlatformTitleBar>>,
    name_editor: Entity<Editor>,
    palette: Palette,
    selected: Role,
    dragging: Option<SliderKind>,
    status: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl ThemeGenerator {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = SystemAppearance::global(cx).0;
        let palette = Palette::new(appearance);
        let name_editor = new_single_line_editor(
            i18n::t("theme_generator_name_placeholder").as_str(),
            "",
            window,
            cx,
        );
        let title_bar = new_window_title_bar("terry-theme-generator-title-bar", cx);

        name_editor.update(cx, |editor, cx| {
            editor.focus_handle(cx).focus(window, cx);
        });

        let subscriptions = vec![cx.observe_global::<SettingsStore>(|_, cx| cx.notify())];

        Self {
            focus_handle: cx.focus_handle(),
            title_bar,
            name_editor,
            palette,
            selected: Role::Background,
            dragging: None,
            status: None,
            _subscriptions: subscriptions,
        }
    }

    fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if self.palette.appearance != appearance {
            self.palette = Palette::new(appearance);
            cx.notify();
        }
    }

    fn select_role(&mut self, role: Role, cx: &mut Context<Self>) {
        self.selected = role;
        cx.notify();
    }

    fn update_slider_color(
        palette: &mut Palette,
        role: Role,
        kind: SliderKind,
        x: Pixels,
        bounds: Bounds<Pixels>,
    ) {
        let width = bounds.size.width;
        if width <= px(0.0) {
            return;
        }

        let t = ((x - bounds.origin.x) / width).clamp(0.0, 1.0);
        let color = palette.color(role);
        let updated = match kind {
            SliderKind::Hue => hsla(t, color.s, color.l, color.a),
            SliderKind::Saturation => hsla(color.h, t, color.l, color.a),
            SliderKind::Lightness => hsla(color.h, color.s, t, color.a),
            SliderKind::Alpha => hsla(color.h, color.s, color.l, t),
        };
        palette.set_color(role, updated);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name_editor.read(cx).text(cx).trim().to_string();
        if name.is_empty() {
            self.status = Some(i18n::t("theme_generator_empty_name").into());
            cx.notify();
            return;
        }

        let json = self.palette.to_theme_json(&name);
        let bytes = match serde_json::to_vec_pretty(&json) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::error!("failed to serialize theme: {error}");
                self.status = Some(i18n::t("theme_generator_save_failed").into());
                cx.notify();
                return;
            }
        };

        let themes_dir = paths::themes_dir();
        let path = themes_dir.join(format!("{}.json", slugify(&name)));
        let write_result =
            std::fs::create_dir_all(themes_dir).and_then(|_| std::fs::write(&path, &bytes));
        if let Err(error) = write_result {
            log::error!("failed to write theme to {}: {error}", path.display());
            self.status = Some(i18n::t("theme_generator_save_failed").into());
            cx.notify();
            return;
        }

        let registry = ThemeRegistry::global(cx);
        if let Err(error) = theme_settings::load_user_theme(&registry, &bytes) {
            log::error!("failed to load generated theme: {error}");
            self.status = Some(i18n::t("theme_generator_save_failed").into());
            cx.notify();
            return;
        }

        let theme_name: Arc<str> = name.as_str().into();
        let appearance = self.palette.appearance;
        let system_appearance = SystemAppearance::global(cx).0;
        let fs = <dyn Fs>::global(cx);
        update_settings_file(fs, cx, move |settings, _| {
            theme_settings::set_theme(settings, theme_name, appearance, system_appearance);
        });

        self.status =
            Some(format!("{} {}", i18n::t("theme_generator_saved"), path.display()).into());
        cx.notify();
    }

    fn color(&self, role: Role) -> Hsla {
        self.palette.color(role)
    }

    fn render_preview(&self) -> AnyElement {
        let color = |role: Role| self.color(role);
        let muted = color(Role::TextMuted);

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_6()
            .gap_4()
            .bg(color(Role::Background))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(muted)
                            .child(i18n::t("theme_generator_preview")),
                    )
                    .child(div().text_size(px(11.0)).text_color(muted).child(
                        match self.palette.appearance {
                            Appearance::Dark => "dark",
                            Appearance::Light => "light",
                        },
                    )),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .border_1()
                    .border_color(color(Role::Border))
                    .bg(color(Role::Background))
                    .child(self.render_preview_title_bar())
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_preview_sidebar())
                            .child(self.render_preview_editor()),
                    )
                    .child(self.render_preview_status_bar()),
            )
            .child(self.render_preview_components())
            .into_any_element()
    }

    fn render_preview_title_bar(&self) -> AnyElement {
        let color = |role: Role| self.color(role);
        let dot = |role: Role| {
            div()
                .size(px(9.0))
                .rounded_full()
                .bg(color(role))
                .into_any_element()
        };

        h_flex()
            .h(px(34.0))
            .px_3()
            .items_center()
            .justify_between()
            .bg(color(Role::TitleBar))
            .child(
                h_flex()
                    .gap_1p5()
                    .child(dot(Role::Error))
                    .child(dot(Role::Warning))
                    .child(dot(Role::Success)),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(color(Role::TextMuted))
                    .child("main.rs — preview"),
            )
            .child(div().w(px(40.0)))
            .into_any_element()
    }

    fn render_preview_sidebar(&self) -> AnyElement {
        let color = |role: Role| self.color(role);

        v_flex()
            .w(px(168.0))
            .h_full()
            .flex_none()
            .py_2()
            .bg(color(Role::PanelBackground))
            .border_r_1()
            .border_color(color(Role::Border))
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .text_size(px(10.0))
                    .text_color(color(Role::TextMuted))
                    .child("EXPLORER"),
            )
            .child(self.preview_tree_row("src", false))
            .child(self.preview_tree_row("main.rs", true))
            .child(self.preview_tree_row("theme.rs", false))
            .child(self.preview_tree_row("lib.rs", false))
            .child(self.preview_tree_row("Cargo.toml", false))
            .into_any_element()
    }

    fn preview_tree_row(&self, name: &str, selected: bool) -> AnyElement {
        let color = |role: Role| self.color(role);
        let foreground = if selected {
            color(Role::Text)
        } else {
            color(Role::TextMuted)
        };

        h_flex()
            .h(px(22.0))
            .px_3()
            .gap_2()
            .items_center()
            .when(selected, |this| this.bg(color(Role::Selection)))
            .child(div().size(px(7.0)).rounded_sm().bg(if selected {
                color(Role::Accent)
            } else {
                color(Role::TextMuted)
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(foreground)
                    .child(name.to_string()),
            )
            .into_any_element()
    }

    fn render_preview_editor(&self) -> AnyElement {
        let color = |role: Role| self.color(role);

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(color(Role::EditorBackground))
            .child(
                h_flex()
                    .h(px(30.0))
                    .bg(color(Role::TabBar))
                    .border_b_1()
                    .border_color(color(Role::Border))
                    .child(
                        h_flex()
                            .h_full()
                            .px_3()
                            .items_center()
                            .bg(color(Role::Background))
                            .border_t_1()
                            .border_color(color(Role::Accent))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(color(Role::Text))
                                    .child("main.rs"),
                            ),
                    )
                    .child(
                        h_flex().h_full().px_3().items_center().child(
                            div()
                                .text_size(px(11.5))
                                .text_color(color(Role::TextMuted))
                                .child("theme.rs"),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_0p5()
                    .font_family("Lilex")
                    .child(self.preview_code_line(
                        1,
                        &[
                            ("use", Role::Keyword),
                            ("std::collections::HashMap;", Role::Text),
                        ],
                        None,
                    ))
                    .child(self.preview_code_line(2, &[], None))
                    .child(self.preview_code_line(
                        3,
                        &[("// Build a theme from a palette", Role::Comment)],
                        None,
                    ))
                    .child(self.preview_code_line(
                        4,
                        &[
                            ("fn", Role::Keyword),
                            ("main", Role::Function),
                            ("()", Role::TextMuted),
                            ("{", Role::TextMuted),
                        ],
                        None,
                    ))
                    .child(self.preview_code_line(
                        5,
                        &[
                            ("let", Role::Keyword),
                            ("accent", Role::Variable),
                            ("=", Role::Operator),
                            ("\"#89b4fa\"", Role::Str),
                            (";", Role::TextMuted),
                        ],
                        Some(Role::Selection),
                    ))
                    .child(self.preview_code_line(
                        6,
                        &[
                            ("let", Role::Keyword),
                            ("count", Role::Variable),
                            ("=", Role::Operator),
                            ("42", Role::Number),
                            (";", Role::TextMuted),
                        ],
                        Some(Role::Hover),
                    ))
                    .child(self.preview_code_line(
                        7,
                        &[
                            ("println!", Role::Function),
                            ("(", Role::TextMuted),
                            ("\"theme: {}\"", Role::Str),
                            (",", Role::TextMuted),
                            ("accent", Role::Variable),
                            (")", Role::TextMuted),
                            (";", Role::TextMuted),
                        ],
                        None,
                    ))
                    .child(self.preview_code_line(8, &[("}", Role::TextMuted)], None)),
            )
            .into_any_element()
    }

    fn preview_code_line(
        &self,
        number: usize,
        spans: &[(&str, Role)],
        background: Option<Role>,
    ) -> AnyElement {
        let color = |role: Role| self.color(role);

        h_flex()
            .h(px(20.0))
            .gap_1()
            .px_2()
            .rounded_sm()
            .when_some(background, |this, role| this.bg(color(role)))
            .child(
                div()
                    .w(px(20.0))
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(color(Role::TextMuted))
                    .child(number.to_string()),
            )
            .children(spans.iter().map(|(text, role)| {
                let mut span = div()
                    .text_size(px(12.5))
                    .text_color(color(*role))
                    .child((*text).to_string());
                if *role == Role::Comment {
                    span = span.italic();
                }
                span.into_any_element()
            }))
            .into_any_element()
    }

    fn render_preview_status_bar(&self) -> AnyElement {
        let color = |role: Role| self.color(role);
        let muted = color(Role::TextMuted);

        h_flex()
            .h(px(24.0))
            .px_3()
            .gap_3()
            .items_center()
            .bg(color(Role::StatusBar))
            .border_t_1()
            .border_color(color(Role::Border))
            .child(div().size(px(7.0)).rounded_full().bg(color(Role::Success)))
            .child(div().text_size(px(10.5)).text_color(muted).child("main"))
            .child(div().flex_1())
            .child(div().text_size(px(10.5)).text_color(muted).child("Rust"))
            .child(div().text_size(px(10.5)).text_color(muted).child("UTF-8"))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(muted)
                    .child("Ln 7, Col 12"),
            )
            .into_any_element()
    }

    fn render_preview_components(&self) -> AnyElement {
        let color = |role: Role| self.color(role);

        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(color(Role::Accent))
                    .text_size(px(11.0))
                    .text_color(color(Role::Background))
                    .child("Button"),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(color(Role::SurfaceBackground))
                    .border_1()
                    .border_color(color(Role::Border))
                    .text_size(px(11.0))
                    .text_color(color(Role::TextMuted))
                    .child("Placeholder"),
            )
            .child(self.preview_pill("Success", Role::Success))
            .child(self.preview_pill("Warning", Role::Warning))
            .child(self.preview_pill("Error", Role::Error))
            .child(self.preview_pill("Info", Role::Info))
            .into_any_element()
    }

    fn preview_pill(&self, label: &str, role: Role) -> AnyElement {
        let color = self.color(role);
        div()
            .px_2()
            .py_0p5()
            .rounded_full()
            .bg(with_alpha(color, 0.18))
            .text_size(px(10.0))
            .text_color(color)
            .child(label.to_string())
            .into_any_element()
    }

    fn render_controls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;
        let panel = colors.panel_background;

        v_flex()
            .w(px(420.0))
            .h_full()
            .flex_none()
            .bg(panel)
            .border_l_1()
            .border_color(border)
            .child(self.render_controls_header(cx))
            .child(self.render_controls_name(cx))
            .child(self.render_current_color(cx))
            .child(self.render_role_list(cx))
            .into_any_element()
    }

    fn render_controls_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;

        h_flex()
            .px_4()
            .py_3()
            .gap_3()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .child(Label::new(i18n::t("theme_generator_colors")).size(LabelSize::Large))
            .child(
                ToggleButtonGroup::single_row(
                    "theme-generator-appearance",
                    [
                        ToggleButtonSimple::new(
                            i18n::t("theme_generator_dark"),
                            cx.listener(|this, _, _, cx| {
                                this.set_appearance(Appearance::Dark, cx);
                            }),
                        ),
                        ToggleButtonSimple::new(
                            i18n::t("theme_generator_light"),
                            cx.listener(|this, _, _, cx| {
                                this.set_appearance(Appearance::Light, cx);
                            }),
                        ),
                    ],
                )
                .selected_index(match self.palette.appearance {
                    Appearance::Dark => 0,
                    Appearance::Light => 1,
                })
                .style(ToggleButtonGroupStyle::Outlined)
                .label_size(LabelSize::Small)
                .auto_width(),
            )
            .into_any_element()
    }

    fn render_controls_name(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;

        v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(
                Label::new(i18n::t("theme_generator_name"))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                div()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .child(self.name_editor.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("save-theme", i18n::t("theme_generator_save"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .children(self.status.clone().map(|status| {
                        Label::new(status)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                    })),
            )
            .into_any_element()
    }

    fn render_current_color(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;
        let selected = self.selected;
        let color = self.palette.color(selected);
        let hex = self.palette.hex(selected);

        v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .size(px(28.0))
                            .rounded_md()
                            .bg(color)
                            .border_1()
                            .border_color(border),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new(selected.key()).size(LabelSize::Small))
                            .child(Label::new(hex).size(LabelSize::XSmall).color(Color::Muted)),
                    ),
            )
            .children(
                SliderKind::ALL
                    .into_iter()
                    .map(|kind| self.render_slider_row(kind, cx)),
            )
            .child(
                Label::new(i18n::t("theme_generator_presets"))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(PRESET_COLORS.iter().enumerate().map(|(ix, hex)| {
                        let color = parse_color(hex);
                        div()
                            .id(("theme-preset", ix))
                            .size(px(18.0))
                            .rounded_sm()
                            .bg(color)
                            .border_1()
                            .border_color(border)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.palette.set_color(this.selected, color);
                                cx.notify();
                            }))
                            .into_any_element()
                    })),
            )
            .into_any_element()
    }

    fn render_slider_row(&self, kind: SliderKind, cx: &mut Context<Self>) -> AnyElement {
        let value = kind.value(self.palette.color(self.selected));
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(kind.label())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(kind.display(value))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(self.render_slider(kind, value, cx))
            .into_any_element()
    }

    fn render_slider(&self, kind: SliderKind, value: f32, cx: &mut Context<Self>) -> AnyElement {
        let color = self.palette.color(self.selected);
        let value = value.clamp(0.0, 1.0);
        let samples: Vec<Hsla> = (0..32)
            .map(|index| kind.sample(color, index as f32 / 31.0))
            .collect();
        let entity = cx.entity().downgrade();

        let bar = h_flex()
            .id(("theme-slider", kind as usize))
            .relative()
            .h(px(16.0))
            .w_full()
            .rounded_full()
            .overflow_hidden()
            .cursor_pointer()
            .children(
                samples
                    .into_iter()
                    .map(|sample| div().flex_1().h_full().bg(sample)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(value.min(0.98)))
                    .w(px(3.0))
                    .bg(gpui::white())
                    .border_1()
                    .border_color(gpui::black()),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        register_slider_listeners(window, entity.clone(), kind, bounds);
                    },
                )
                .absolute()
                .size_full(),
            );

        bar.into_any_element()
    }

    fn render_role_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let text_muted = cx.theme().colors().text_muted;
        let mut groups = Vec::with_capacity(RoleGroup::ALL.len());

        for group in RoleGroup::ALL {
            let rows: Vec<AnyElement> = Role::ALL
                .into_iter()
                .filter(|role| role.group() == group)
                .map(|role| self.render_role_row(role, cx))
                .collect();

            groups.push(
                v_flex()
                    .w_full()
                    .child(
                        div()
                            .px_4()
                            .py_1()
                            .text_size(px(10.5))
                            .text_color(text_muted)
                            .child(group.label()),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }

        v_flex()
            .id("theme-role-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_2()
            .children(groups)
            .into_any_element()
    }

    fn render_role_row(&self, role: Role, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;
        let hover_background = colors.element_hover;
        let selected = self.selected == role;
        let selected_background = colors.element_selected;
        let swatch = self.palette.color(role);
        let hex = self.palette.hex(role);

        h_flex()
            .id(("theme-role", role as usize))
            .w_full()
            .px_4()
            .py_1p5()
            .gap_3()
            .items_center()
            .cursor_pointer()
            .when(selected, |this| this.bg(selected_background))
            .hover(move |this| this.bg(hover_background))
            .child(
                div()
                    .size(px(16.0))
                    .rounded_sm()
                    .bg(swatch)
                    .border_1()
                    .border_color(border),
            )
            .child(Label::new(role.key()).size(LabelSize::Small))
            .child(div().flex_1())
            .child(Label::new(hex).size(LabelSize::XSmall).color(Color::Muted))
            .on_click(cx.listener(move |this, _, _, cx| this.select_role(role, cx)))
            .into_any_element()
    }
}

fn register_slider_listeners(
    window: &mut Window,
    entity: WeakEntity<ThemeGenerator>,
    kind: SliderKind,
    bounds: Bounds<Pixels>,
) {
    let down_entity = entity.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
        if phase.bubble() && event.button == MouseButton::Left && bounds.contains(&event.position) {
            down_entity
                .update(cx, |this, cx| {
                    this.dragging = Some(kind);
                    ThemeGenerator::update_slider_color(
                        &mut this.palette,
                        this.selected,
                        kind,
                        event.position.x,
                        bounds,
                    );
                    cx.notify();
                })
                .ok();
        }
    });

    let move_entity = entity.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if !phase.bubble() {
            return;
        }
        move_entity
            .update(cx, |this, cx| {
                if this.dragging != Some(kind) {
                    return;
                }
                if event.pressed_button.is_none() {
                    this.dragging = None;
                    cx.notify();
                    return;
                }
                ThemeGenerator::update_slider_color(
                    &mut this.palette,
                    this.selected,
                    kind,
                    event.position.x,
                    bounds,
                );
                cx.notify();
            })
            .ok();
    });

    let up_entity = entity;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if !phase.bubble() || event.button != MouseButton::Left {
            return;
        }
        up_entity
            .update(cx, |this, cx| {
                if this.dragging == Some(kind) {
                    this.dragging = None;
                    cx.notify();
                }
            })
            .ok();
    });
}

impl Focusable for ThemeGenerator {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ThemeGenerator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar =
            render_window_title_bar(&self.title_bar, i18n::t("theme_generator"), window, cx);

        v_flex()
            .key_context("ThemeGenerator")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .children(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_preview())
                    .child(self.render_controls(cx)),
            )
    }
}

pub fn init(cx: &mut App) {
    if load_user_themes(cx) {
        theme_settings::reload_theme(cx);
    }

    cx.on_action(|_: &OpenThemeGenerator, cx| open(cx));
}

pub fn open(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ThemeGenerator>())
    {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .log_err();
        return;
    }

    let app_id = ReleaseChannel::global(cx).app_id();
    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("theme_generator")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                app_id: Some(app_id.to_owned()),
                window_bounds: Some(WindowBounds::centered(size(px(1180.0), px(760.0)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(960.0),
                    height: px(620.0),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ThemeGenerator::new(window, cx)),
        )
        .log_err();
    });
}

/// Loads themes generated by the theme generator from the user's theme
/// directory. Returns whether any theme was loaded.
fn load_user_themes(cx: &mut App) -> bool {
    let themes_dir = paths::themes_dir();
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return false;
    };

    let registry = ThemeRegistry::global(cx);
    let mut loaded = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match theme_settings::load_user_theme(&registry, &bytes) {
                Ok(()) => loaded = true,
                Err(error) => log::error!("failed to load theme at {}: {error}", path.display()),
            },
            Err(error) => log::error!("failed to read theme at {}: {error}", path.display()),
        }
    }

    loaded
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "custom-theme".to_string()
    } else {
        slug
    }
}

fn new_single_line_editor<T: 'static>(
    placeholder: &str,
    text: &str,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<Editor> {
    let placeholder = placeholder.to_string();
    let text = text.to_string();
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_placeholder_text(placeholder.as_str(), window, cx);
        if !text.is_empty() {
            editor.set_text(text, window, cx);
        }
        editor
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_themes_deserialize() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let palette = Palette::new(appearance);
            let json = palette.to_theme_json("Generated Theme");
            let bytes = serde_json::to_vec(&json).expect("theme should serialize");

            let family: theme_settings::ThemeFamilyContent =
                serde_json::from_slice(&bytes).expect("generated theme should deserialize");
            assert_eq!(family.name, "Generated Theme");
            assert_eq!(family.themes.len(), 1);
            assert_eq!(family.themes[0].name, "Generated Theme");

            // Refining the theme parses every color and catches invalid values.
            let _ = theme_settings::refine_theme_family(family);
        }
    }

    #[test]
    fn slugify_produces_safe_file_names() {
        assert_eq!(slugify("My Theme"), "my-theme");
        assert_eq!(slugify("  Rosé / Pine  "), "rosé-pine");
        assert_eq!(slugify("///"), "custom-theme");
    }
}
