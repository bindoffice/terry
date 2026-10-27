use collections::HashMap;
use gpui::{FontFallbacks, FontFeatures, FontWeight, Pixels, px};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use settings::AlternateScroll;

use settings::{
    HintActionContent, HintRuleContent, IntoGpui, PathHyperlinkRegex, RegisterSetting,
    ShowScrollbar, SmartSelectionActionContent, SmartSelectionRuleContent, SshHostContent,
    TerminalBell, TerminalBlink, TerminalDockPosition, TerminalLineHeight, VenvSettings,
    WorkingDirectory, merge_from::MergeFrom,
};
use task::Shell;
use theme_settings::FontFamilyName;

/// Scrollback limit used when the user enables `unlimited_scroll_history`.
/// Large enough to be effectively unlimited while keeping the grid's
/// `max_scroll_limit + lines` arithmetic from overflowing.
pub(crate) const UNLIMITED_SCROLL_HISTORY_LINES: usize = 10_000_000;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Toolbar {
    pub breadcrumbs: bool,
}

/// A resolved smart-selection rule from settings.
#[derive(Clone, Debug, Deserialize)]
pub struct SmartSelectionRule {
    /// Regular expression matched against the clicked line.
    pub regex: String,
    /// The compiled regex, filled in when settings are loaded. `None` when the
    /// pattern was invalid, in which case the rule is skipped.
    #[serde(skip)]
    pub compiled: Option<Regex>,
    /// What to do with the matched text.
    pub action: SmartSelectionAction,
    /// Optional description, for documentation only.
    pub description: Option<String>,
}

/// What a smart-selection rule does with a match.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum SmartSelectionAction {
    /// Select the matched text.
    #[default]
    Select,
    /// Select the matched text and copy it to the clipboard.
    Copy,
}

impl From<SmartSelectionActionContent> for SmartSelectionAction {
    fn from(value: SmartSelectionActionContent) -> Self {
        match value {
            SmartSelectionActionContent::Select => SmartSelectionAction::Select,
            SmartSelectionActionContent::Copy => SmartSelectionAction::Copy,
        }
    }
}

impl From<SmartSelectionRuleContent> for SmartSelectionRule {
    fn from(value: SmartSelectionRuleContent) -> Self {
        let SmartSelectionRuleContent {
            regex,
            action,
            description,
        } = value;
        // Compile once here instead of on every double-click. Invalid patterns
        // are reported once and then skipped.
        let compiled = match Regex::new(&regex) {
            Ok(compiled) => Some(compiled),
            Err(error) => {
                log::warn!("ignoring invalid terminal smart_selection regex {regex:?}: {error}");
                None
            }
        };
        Self {
            regex,
            compiled,
            action: action.unwrap_or_default().into(),
            description,
        }
    }
}

/// What a hint does with a match.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HintAction {
    /// Open the match as a URL or path.
    Open,
    /// Copy the match to the clipboard.
    #[default]
    Copy,
    /// Paste the match into the terminal.
    Paste,
    /// Select the match in the terminal.
    Select,
}

impl From<HintActionContent> for HintAction {
    fn from(value: HintActionContent) -> Self {
        match value {
            HintActionContent::Open => HintAction::Open,
            HintActionContent::Copy => HintAction::Copy,
            HintActionContent::Paste => HintAction::Paste,
            HintActionContent::Select => HintAction::Select,
        }
    }
}

/// A resolved custom hint rule from settings.
#[derive(Clone, Debug, Deserialize)]
pub struct HintRule {
    /// Optional human-readable label shown in the hints picker.
    pub name: Option<String>,
    /// The regex source, kept for diagnostics.
    pub regex: String,
    /// The compiled regex. `None` when the pattern was invalid.
    #[serde(skip)]
    pub compiled: Option<Regex>,
    /// What to do with the matched text.
    pub action: HintAction,
}

impl From<HintRuleContent> for HintRule {
    fn from(value: HintRuleContent) -> Self {
        let HintRuleContent {
            name,
            regex,
            action,
        } = value;
        let compiled = match Regex::new(&regex) {
            Ok(compiled) => Some(compiled),
            Err(error) => {
                log::warn!("ignoring invalid terminal hints regex {regex:?}: {error}");
                None
            }
        };
        Self {
            name,
            regex,
            compiled,
            action: action.unwrap_or_default().into(),
        }
    }
}

/// A resolved SSH host offered by the SSH picker.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TerminalSshHost {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

/// A resolved terminal launch profile from settings.
#[derive(Clone, Debug, Deserialize)]
pub struct TerminalProfile {
    pub name: String,
    pub shell: Option<Shell>,
    pub working_directory: Option<WorkingDirectory>,
    pub env: HashMap<String, String>,
    pub command: Option<String>,
    /// Theme used for this profile's terminals instead of the UI theme.
    pub color_scheme: Option<String>,
}

#[derive(Clone, Debug, Deserialize, RegisterSetting)]
pub struct TerminalSettings {
    pub shell: Shell,
    pub working_directory: WorkingDirectory,
    pub font_size: Option<Pixels>, // todo(settings_refactor) can be non-optional...
    pub font_family: Option<FontFamilyName>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_features: Option<FontFeatures>,
    pub ligatures: bool,
    pub font_weight: Option<FontWeight>,
    pub line_height: TerminalLineHeight,
    pub env: HashMap<String, String>,
    pub cursor_shape: CursorShape,
    pub blinking: TerminalBlink,
    pub alternate_scroll: AlternateScroll,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    pub keep_selection_on_copy: bool,
    pub open_links_in_mouse_mode: bool,
    pub button: bool,
    pub dock: TerminalDockPosition,
    pub flexible: bool,
    pub default_width: Pixels,
    pub default_height: Pixels,
    pub detect_venv: VenvSettings,
    pub max_scroll_history_lines: Option<usize>,
    pub scroll_multiplier: f32,
    pub toolbar: Toolbar,
    pub scrollbar: ScrollbarSettings,
    pub minimum_contrast: f32,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
    pub show_count_badge: bool,
    pub bell: TerminalBell,
    /// Whether to render inline images (kitty graphics protocol / sixel).
    pub image_display: bool,
    /// Maximum bytes the image pool may hold.
    pub max_image_memory: usize,
    /// Whether Terry automatically injects its shell integration scripts into
    /// supported shells (zsh, fish, PowerShell, Nushell) so prompt marks
    /// (OSC 133) and working directory reports (OSC 7) work out of the box.
    /// bash needs the script to be sourced manually from
    /// `$TERRY_INTEGRATION_DIR`.
    pub shell_integration: bool,
    /// Allow applications to read the clipboard via OSC 52. Writing is always
    /// allowed; reading is opt-in because remote shells can use it to read
    /// local clipboard contents.
    pub clipboard_read: bool,
    /// Show a system notification when a command finishes while the terminal
    /// is not focused.
    pub notify_on_command_finish: bool,
    /// Whether to ask for confirmation before pasting text that contains more
    /// than one line.
    pub confirm_multiline_paste: bool,
    /// Terminal background opacity, `0.0` to `1.0`.
    pub background_opacity: f32,
    /// Blur the desktop behind a translucent terminal background.
    pub background_blur: bool,
    /// Horizontal padding inside the terminal element, in pixels.
    pub padding_x: Pixels,
    /// Vertical padding inside the terminal element, in pixels.
    pub padding_y: Pixels,
    /// Characters that terminate a word. `None` uses the terminal library's
    /// default set.
    pub word_separators: Option<String>,
    /// Cursor shape used in vi mode. `None` follows `cursor_shape`.
    pub vi_cursor_shape: Option<CursorShape>,
    /// Rules that refine double-click selection.
    pub smart_selection: Vec<SmartSelectionRule>,
    /// Custom hint rules for the "Show Hints" action.
    pub hints: Vec<HintRule>,
    /// User-defined SSH hosts for the SSH picker.
    pub ssh_hosts: Vec<TerminalSshHost>,
    /// Whether SSH launches reuse connections via ControlMaster.
    pub ssh_reuse_connections: bool,
    /// Extra arguments appended to SSH invocations.
    pub ssh_extra_args: Vec<String>,
    /// Default color scheme (terminal color scheme or theme) for terminals that
    /// do not override it.
    pub color_scheme: Option<String>,
    /// Named launch profiles.
    pub profiles: Vec<TerminalProfile>,
    /// Profile used for new terminals, if any.
    pub default_profile: Option<String>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ScrollbarSettings {
    /// When to show the scrollbar in the terminal.
    ///
    /// Default: inherits editor scrollbar settings
    pub show: Option<ShowScrollbar>,
}

fn settings_shell_to_task_shell(shell: settings::Shell) -> Shell {
    match shell {
        settings::Shell::System => Shell::System,
        settings::Shell::Program(program) => Shell::Program(program),
        settings::Shell::WithArguments {
            program,
            args,
            title_override,
        } => Shell::WithArguments {
            program,
            args,
            title_override,
        },
    }
}

impl settings::Settings for TerminalSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let user_content = content.terminal.clone().unwrap_or_default();
        // Note: we allow a subset of "terminal" settings in the project files.
        let mut project_content = user_content.project.clone();
        project_content.merge_from_option(content.project.terminal.as_ref());
        TerminalSettings {
            shell: settings_shell_to_task_shell(project_content.shell.unwrap_or_default()),
            working_directory: project_content.working_directory.unwrap_or_default(),
            font_size: user_content.font_size.map(|s| s.into_gpui()),
            font_family: user_content.font_family,
            font_fallbacks: user_content.font_fallbacks.map(|fallbacks| {
                FontFallbacks::from_fonts(
                    fallbacks
                        .into_iter()
                        .map(|family| family.0.to_string())
                        .collect(),
                )
            }),
            font_features: user_content.font_features.map(|f| f.into_gpui()),
            ligatures: user_content.ligatures.unwrap_or(false),
            font_weight: user_content.font_weight.map(|w| w.into_gpui()),
            line_height: user_content.line_height.unwrap_or_default(),
            env: project_content.env.unwrap_or_default(),
            cursor_shape: user_content.cursor_shape.unwrap_or_default().into(),
            blinking: user_content.blinking.unwrap_or_default(),
            alternate_scroll: user_content.alternate_scroll.unwrap_or_default(),
            option_as_meta: user_content.option_as_meta.unwrap_or_default(),
            copy_on_select: user_content.copy_on_select.unwrap_or_default(),
            keep_selection_on_copy: user_content.keep_selection_on_copy.unwrap_or_default(),
            open_links_in_mouse_mode: user_content.open_links_in_mouse_mode.unwrap_or_default(),
            button: user_content.button.unwrap_or_default(),
            dock: user_content.dock.unwrap_or_default(),
            default_width: px(user_content.default_width.unwrap_or_default()),
            default_height: px(user_content.default_height.unwrap_or_default()),
            flexible: user_content.flexible.unwrap_or_default(),
            detect_venv: project_content.detect_venv.unwrap_or_default(),
            scroll_multiplier: user_content.scroll_multiplier.unwrap_or_default(),
            max_scroll_history_lines: if user_content.unlimited_scroll_history.unwrap_or(false) {
                Some(UNLIMITED_SCROLL_HISTORY_LINES)
            } else {
                user_content.max_scroll_history_lines
            },
            toolbar: Toolbar {
                breadcrumbs: user_content
                    .toolbar
                    .unwrap_or_default()
                    .breadcrumbs
                    .unwrap_or_default(),
            },
            scrollbar: ScrollbarSettings {
                show: user_content.scrollbar.unwrap_or_default().show,
            },
            minimum_contrast: user_content.minimum_contrast.unwrap_or_default(),
            path_hyperlink_regexes: project_content
                .path_hyperlink_regexes
                .unwrap_or_default()
                .into_iter()
                .map(|regex| match regex {
                    PathHyperlinkRegex::SingleLine(regex) => regex,
                    PathHyperlinkRegex::MultiLine(regex) => regex.join("\n"),
                })
                .collect(),
            path_hyperlink_timeout_ms: project_content
                .path_hyperlink_timeout_ms
                .unwrap_or_default(),
            show_count_badge: user_content.show_count_badge.unwrap_or_default(),
            bell: user_content.bell.unwrap_or_default(),
            image_display: user_content.image_display.unwrap_or_default(),
            max_image_memory: user_content.max_image_memory.unwrap_or(512 * 1024 * 1024),
            shell_integration: user_content.shell_integration.unwrap_or(true),
            clipboard_read: user_content.clipboard_read.unwrap_or(false),
            notify_on_command_finish: user_content.notify_on_command_finish.unwrap_or(false),
            confirm_multiline_paste: user_content.confirm_multiline_paste.unwrap_or(true),
            background_opacity: user_content
                .background_opacity
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
            background_blur: user_content.background_blur.unwrap_or(false),
            padding_x: px(user_content.padding_x.unwrap_or(0.0).max(0.0)),
            padding_y: px(user_content.padding_y.unwrap_or(0.0).max(0.0)),
            word_separators: user_content
                .word_separators
                .filter(|separators| !separators.is_empty()),
            vi_cursor_shape: user_content.vi_cursor_shape.map(Into::into),
            smart_selection: user_content
                .smart_selection
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            hints: user_content
                .hints
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            ssh_hosts: user_content
                .ssh_hosts
                .unwrap_or_default()
                .into_iter()
                .map(|host| TerminalSshHost {
                    name: host.name.unwrap_or_else(|| host.host.clone()),
                    host: host.host,
                    user: host.user,
                    port: host.port,
                    identity_file: host.identity_file,
                })
                .collect(),
            ssh_reuse_connections: user_content.ssh_reuse_connections.unwrap_or(true),
            ssh_extra_args: user_content.ssh_extra_args.unwrap_or_default(),
            color_scheme: user_content.color_scheme,
            profiles: user_content
                .profiles
                .unwrap_or_default()
                .into_iter()
                .map(|profile| TerminalProfile {
                    name: profile.name,
                    shell: profile.shell.map(settings_shell_to_task_shell),
                    working_directory: profile.working_directory,
                    env: profile.env.unwrap_or_default(),
                    command: profile.command,
                    color_scheme: profile.color_scheme,
                })
                .collect(),
            default_profile: user_content.default_profile,
        }
    }
}

impl TerminalSettings {
    /// Whether the scrollback history is configured to be unlimited.
    pub fn unlimited_scroll_history(&self) -> bool {
        self.max_scroll_history_lines == Some(UNLIMITED_SCROLL_HISTORY_LINES)
    }

    /// Looks up a profile by its exact name.
    pub fn profile(&self, name: &str) -> Option<&TerminalProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// Resolves the configured default profile, if it still exists.
    pub fn default_profile_config(&self) -> Option<&TerminalProfile> {
        self.default_profile
            .as_deref()
            .and_then(|name| self.profile(name))
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Cursor is a block like `█`.
    #[default]
    Block,
    /// Cursor is an underscore like `_`.
    Underline,
    /// Cursor is a vertical bar like `⎸`.
    Bar,
    /// Cursor is a hollow box like `▯`.
    Hollow,
}

impl From<settings::CursorShapeContent> for CursorShape {
    fn from(value: settings::CursorShapeContent) -> Self {
        match value {
            settings::CursorShapeContent::Block => CursorShape::Block,
            settings::CursorShapeContent::Underline => CursorShape::Underline,
            settings::CursorShapeContent::Bar => CursorShape::Bar,
            settings::CursorShapeContent::Hollow => CursorShape::Hollow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::{RootUserSettings as _, Settings as _, SettingsContent};

    #[test]
    fn transparency_padding_and_text_options_are_parsed() {
        let content = SettingsContent::parse_json_with_comments("{}").unwrap();
        let settings = TerminalSettings::from_settings(&content);
        // Transparency and padding default to "off" rather than zeroing out.
        assert_eq!(settings.background_opacity, 1.0);
        assert!(!settings.background_blur);
        assert_eq!(settings.padding_x, px(0.0));
        assert_eq!(settings.padding_y, px(0.0));
        assert_eq!(settings.word_separators, None);
        assert_eq!(settings.vi_cursor_shape, None);
        assert!(settings.smart_selection.is_empty());

        let content = SettingsContent::parse_json_with_comments(
            r#"{
                "terminal": {
                    "background_opacity": 0.85,
                    "background_blur": true,
                    "padding_x": 6,
                    "padding_y": 2,
                    "word_separators": " \t",
                    "vi_cursor_shape": "underline",
                    "smart_selection": [
                        { "regex": "https?://\\S+", "action": "copy" },
                        { "regex": "\\bTODO\\b" }
                    ]
                }
            }"#,
        )
        .unwrap();
        let settings = TerminalSettings::from_settings(&content);
        assert_eq!(settings.background_opacity, 0.85);
        assert!(settings.background_blur);
        assert_eq!(settings.padding_x, px(6.0));
        assert_eq!(settings.padding_y, px(2.0));
        assert_eq!(settings.word_separators.as_deref(), Some(" \t"));
        assert_eq!(settings.vi_cursor_shape, Some(CursorShape::Underline));
        assert_eq!(settings.smart_selection.len(), 2);
        assert_eq!(
            settings.smart_selection[0].action,
            SmartSelectionAction::Copy
        );
        assert_eq!(
            settings.smart_selection[1].action,
            SmartSelectionAction::Select
        );
    }

    #[test]
    fn background_opacity_is_clamped() {
        let content = SettingsContent::parse_json_with_comments(
            r#"{ "terminal": { "background_opacity": 2.0 } }"#,
        )
        .unwrap();
        assert_eq!(
            TerminalSettings::from_settings(&content).background_opacity,
            1.0
        );

        let content = SettingsContent::parse_json_with_comments(
            r#"{ "terminal": { "background_opacity": -1.0 } }"#,
        )
        .unwrap();
        assert_eq!(
            TerminalSettings::from_settings(&content).background_opacity,
            0.0
        );
    }

    #[test]
    fn parses_terminal_profiles_and_default() {
        let content = SettingsContent::parse_json_with_comments(
            r#"{
                "terminal": {
                    "profiles": [
                        {
                            "name": "tmux",
                            "command": "tmux new -A -s main",
                            "env": { "TMUX_PROFILE": "1" }
                        },
                        {
                            "name": "venv",
                            "working_directory": "current_project_directory",
                            "color_scheme": "One Dark"
                        }
                    ],
                    "default_profile": "tmux",
                    "notify_on_command_finish": true
                }
            }"#,
        )
        .unwrap();

        let settings = TerminalSettings::from_settings(&content);
        assert!(settings.notify_on_command_finish);
        assert_eq!(settings.profiles.len(), 2);
        assert_eq!(settings.profiles[0].name, "tmux");
        assert_eq!(
            settings.profiles[0].command.as_deref(),
            Some("tmux new -A -s main")
        );
        assert_eq!(
            settings.profiles[0]
                .env
                .get("TMUX_PROFILE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            settings
                .default_profile_config()
                .map(|profile| profile.name.as_str()),
            Some("tmux")
        );
        assert_eq!(
            settings
                .profile("venv")
                .and_then(|profile| profile.working_directory.clone()),
            Some(WorkingDirectory::CurrentProjectDirectory)
        );
        assert_eq!(
            settings
                .profile("venv")
                .and_then(|profile| profile.color_scheme.as_deref()),
            Some("One Dark")
        );
    }

    #[test]
    fn parses_ssh_settings_and_default_color_scheme() {
        let content = SettingsContent::parse_json_with_comments(
            r#"{
                "terminal": {
                    "color_scheme": "Nord",
                    "ssh_reuse_connections": false,
                    "ssh_extra_args": ["-o", "ServerAliveInterval=30"],
                    "ssh_hosts": [
                        { "host": "prod.example.com" },
                        {
                            "name": "Staging",
                            "host": "staging.example.com",
                            "user": "deploy",
                            "port": 2222,
                            "identity_file": "~/.ssh/id_staging"
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let settings = TerminalSettings::from_settings(&content);
        assert_eq!(settings.color_scheme.as_deref(), Some("Nord"));
        assert!(!settings.ssh_reuse_connections);
        assert_eq!(
            settings.ssh_extra_args,
            vec!["-o".to_string(), "ServerAliveInterval=30".to_string()]
        );
        assert_eq!(settings.ssh_hosts.len(), 2);
        assert_eq!(settings.ssh_hosts[0].name, "prod.example.com");
        assert_eq!(settings.ssh_hosts[0].user, None);
        assert_eq!(settings.ssh_hosts[1].name, "Staging");
        assert_eq!(settings.ssh_hosts[1].user.as_deref(), Some("deploy"));
        assert_eq!(settings.ssh_hosts[1].port, Some(2222));
        assert_eq!(
            settings.ssh_hosts[1].identity_file.as_deref(),
            Some("~/.ssh/id_staging")
        );
    }

    #[test]
    fn ligatures_are_off_by_default_and_can_be_enabled() {
        let content = SettingsContent::parse_json_with_comments("{}").unwrap();
        assert!(!TerminalSettings::from_settings(&content).ligatures);

        let content =
            SettingsContent::parse_json_with_comments(r#"{ "terminal": { "ligatures": true } }"#)
                .unwrap();
        assert!(TerminalSettings::from_settings(&content).ligatures);
    }

    #[test]
    fn parses_custom_hint_rules() {
        let content = SettingsContent::parse_json_with_comments(
            r##"{
                "terminal": {
                    "hints": [
                        { "name": "Issues", "regex": "#\\d+", "action": "copy" },
                        { "regex": "JIRA-\\d+", "action": "open" },
                        { "regex": "(", "action": "select" }
                    ]
                }
            }"##,
        )
        .unwrap();

        let settings = TerminalSettings::from_settings(&content);
        assert_eq!(settings.hints.len(), 3);
        assert_eq!(settings.hints[0].name.as_deref(), Some("Issues"));
        assert_eq!(settings.hints[0].action, HintAction::Copy);
        assert!(settings.hints[0].compiled.is_some());
        assert_eq!(settings.hints[1].action, HintAction::Open);
        // Invalid regexes are kept as rules but skipped by the scanner.
        assert_eq!(settings.hints[2].action, HintAction::Select);
        assert!(settings.hints[2].compiled.is_none());
    }

    #[test]
    fn unlimited_scroll_history_overrides_the_line_cap() {
        let content = SettingsContent::parse_json_with_comments(
            r#"{ "terminal": { "max_scroll_history_lines": 5000 } }"#,
        )
        .unwrap();
        assert_eq!(
            TerminalSettings::from_settings(&content).max_scroll_history_lines,
            Some(5000)
        );

        let content = SettingsContent::parse_json_with_comments(
            r#"{
                "terminal": {
                    "max_scroll_history_lines": 5000,
                    "unlimited_scroll_history": true
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            TerminalSettings::from_settings(&content).max_scroll_history_lines,
            Some(UNLIMITED_SCROLL_HISTORY_LINES)
        );
    }
}
