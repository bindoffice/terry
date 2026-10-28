use std::path::PathBuf;

use collections::HashMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

use crate::{FontFamilyName, FontFeaturesContent, FontSize, FontWeightContent};

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ProjectTerminalSettingsContent {
    /// What shell to use when opening a terminal.
    ///
    /// Default: system
    pub shell: Option<Shell>,
    /// What working directory to use when launching the terminal
    ///
    /// Default: current_project_directory
    pub working_directory: Option<WorkingDirectory>,
    /// Any key-value pairs added to this list will be added to the terminal's
    /// environment. Use `:` to separate multiple values.
    ///
    /// Default: {}
    pub env: Option<HashMap<String, String>>,
    /// Activates the python virtual environment, if one is found, in the
    /// terminal's working directory (as resolved by the working_directory
    /// setting). Set this to "off" to disable this behavior.
    ///
    /// Default: on
    pub detect_venv: Option<VenvSettings>,
    /// Regexes used to identify paths for hyperlink navigation.
    ///
    /// Default: [
    ///   // Python-style diagnostics
    ///   "File \"(?<path>[^\"]+)\", line (?<line>[0-9]+)",
    ///   // Common path syntax with optional line, column, description, trailing punctuation, or
    ///   // surrounding symbols or quotes
    ///   [
    ///     "(?x)",
    ///     "# optionally starts with 0-2 opening prefix symbols",
    ///     "[({\\[<]{0,2}",
    ///     "# which may be followed by an opening quote",
    ///     "(?<quote>[\"'`])?",
    ///     "# `path` is the shortest sequence of any non-space character",
    ///     "(?<link>(?<path>[^ ]+?",
    ///     "    # which may end with a line and optionally a column,",
    ///     "    (?<line_column>:+[0-9]+(:[0-9]+)?|:?\\([0-9]+([,:][0-9]+)?\\))?",
    ///     "))",
    ///     "# which must be followed by a matching quote",
    ///     "(?(<quote>)\\k<quote>)",
    ///     "# and optionally a single closing symbol",
    ///     "[)}\\]>]?",
    ///     "# if line/column matched, may be followed by a description",
    ///     "(?(<line_column>):[^ 0-9][^ ]*)?",
    ///     "# which may be followed by trailing punctuation",
    ///     "[.,:)}\\]>]*",
    ///     "# and always includes trailing whitespace or end of line",
    ///     "([ ]+|$)"
    ///   ]
    /// ]
    pub path_hyperlink_regexes: Option<Vec<PathHyperlinkRegex>>,
    /// Timeout for hover and Cmd-click path hyperlink discovery in milliseconds.
    ///
    /// Default: 1
    pub path_hyperlink_timeout_ms: Option<u64>,
}

/// A named set of terminal launch options.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct TerminalProfileContent {
    /// Display name and lookup key of the profile.
    pub name: String,
    /// Shell to launch instead of the global `shell` setting.
    pub shell: Option<Shell>,
    /// Working directory to use instead of the global `working_directory`
    /// setting.
    pub working_directory: Option<WorkingDirectory>,
    /// Environment variables merged on top of the global terminal `env`.
    pub env: Option<HashMap<String, String>>,
    /// Command to write to the shell after it starts. The command is typed
    /// into the terminal, so it can use shell syntax and aliases.
    pub command: Option<String>,
    /// Name of the theme to use for this profile's terminals instead of the
    /// active UI theme. Only the colors are overridden: fonts, cursor shape,
    /// opacity and padding still come from the terminal settings.
    ///
    /// Example: `"One Dark"`.
    pub color_scheme: Option<String>,
}

#[with_fallible_options]
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct TerminalSettingsContent {
    #[serde(flatten)]
    pub project: ProjectTerminalSettingsContent,
    /// Sets the terminal's font size.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font size.
    pub font_size: Option<FontSize>,
    /// Sets the terminal's font family.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font family.
    pub font_family: Option<FontFamilyName>,

    /// Sets the terminal's font fallbacks.
    ///
    /// If this option is not included,
    /// the terminal will default to matching the buffer's font fallbacks.
    #[schemars(extend("uniqueItems" = true))]
    pub font_fallbacks: Option<Vec<FontFamilyName>>,

    /// Sets the terminal's line height.
    ///
    /// Default: comfortable
    pub line_height: Option<TerminalLineHeight>,
    pub font_features: Option<FontFeaturesContent>,
    /// Whether to use programming ligatures (contextual alternates, `calt`)
    /// from the terminal font.
    ///
    /// Most programming fonts implement ligatures through `calt`; when this is
    /// disabled Terry turns that feature off so every glyph occupies exactly one
    /// cell. Enable it to render ligatures such as `->` or `=>`.
    ///
    /// Default: false
    pub ligatures: Option<bool>,
    /// Sets the terminal's font weight in CSS weight units 0-900.
    pub font_weight: Option<FontWeightContent>,
    /// Default cursor shape for the terminal.
    /// Can be "bar", "block", "underline", or "hollow".
    ///
    /// Default: "block"
    pub cursor_shape: Option<CursorShapeContent>,
    /// Sets the cursor blinking behavior in the terminal.
    ///
    /// Default: terminal_controlled
    pub blinking: Option<TerminalBlink>,
    /// Sets whether Alternate Scroll mode (code: ?1007) is active by default.
    /// Alternate Scroll mode converts mouse scroll events into up / down key
    /// presses when in the alternate screen (e.g. when running applications
    /// like vim or  less). The terminal can still set and unset this mode.
    ///
    /// Default: on
    pub alternate_scroll: Option<AlternateScroll>,
    /// Sets whether the option key behaves as the meta key.
    ///
    /// Default: false
    pub option_as_meta: Option<bool>,
    /// Whether or not selecting text in the terminal will automatically
    /// copy to the system clipboard.
    ///
    /// Default: false
    pub copy_on_select: Option<bool>,
    /// Whether to keep the text selection after copying it to the clipboard.
    ///
    /// Default: true
    pub keep_selection_on_copy: Option<bool>,
    /// Whether cmd-click (ctrl-click on Linux and Windows) opens hyperlinks even
    /// when the terminal application has enabled mouse reporting (e.g. vim with
    /// mouse=a, htop). When false, these clicks are forwarded to the application
    /// instead, and hyperlinks can still be opened with shift-cmd-click
    /// (shift-ctrl-click).
    ///
    /// Default: true
    pub open_links_in_mouse_mode: Option<bool>,
    /// Whether to show the terminal button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    pub dock: Option<TerminalDockPosition>,
    /// Whether the terminal panel should use flexible (proportional) sizing.
    ///
    /// Default: true
    pub flexible: Option<bool>,
    /// Default width when the terminal is docked to the left or right.
    ///
    /// Default: 640
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub default_width: Option<f32>,
    /// Default height when the terminal is docked to the bottom.
    ///
    /// Default: 320
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub default_height: Option<f32>,
    /// The maximum number of lines to keep in the scrollback history.
    /// 0 disables the scrolling.
    /// Existing terminals will not pick up this change until they are recreated.
    /// See <a href="https://github.com/alacritty/alacritty/blob/cb3a79dbf6472740daca8440d5166c1d4af5029e/extra/man/alacritty.5.scd?plain=1#L207-L213">Alacritty documentation</a> for more information.
    ///
    /// Default: 10_000
    pub max_scroll_history_lines: Option<usize>,
    /// Keep the entire scrollback history instead of evicting old lines once
    /// `max_scroll_history_lines` is reached.
    ///
    /// Memory usage grows with the amount of output the terminal produces, so
    /// this is intended for people who regularly need to scroll far back.
    /// `max_scroll_history_lines` is ignored when this is enabled.
    ///
    /// Default: false
    pub unlimited_scroll_history: Option<bool>,
    /// The multiplier for scrolling with the mouse wheel.
    ///
    /// Default: 1.0
    pub scroll_multiplier: Option<f32>,
    /// Toolbar related settings
    pub toolbar: Option<TerminalToolbarContent>,
    /// Scrollbar-related settings
    pub scrollbar: Option<ScrollbarSettingsContent>,
    /// The minimum APCA perceptual contrast between foreground and background colors.
    ///
    /// APCA (Accessible Perceptual Contrast Algorithm) is more accurate than WCAG 2.x,
    /// especially for dark mode. Values range from 0 to 106.
    ///
    /// Based on APCA Readability Criterion (ARC) Bronze Simple Mode:
    /// https://readtech.org/ARC/tests/bronze-simple-mode/
    /// - 0: No contrast adjustment
    /// - 45: Minimum for large fluent text (36px+)
    /// - 60: Minimum for other content text
    /// - 75: Minimum for body text
    /// - 90: Preferred for body text
    ///
    /// Default: 45
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub minimum_contrast: Option<f32>,
    /// Whether to show a badge on the terminal panel icon with the count of open terminals.
    ///
    /// Default: false
    pub show_count_badge: Option<bool>,
    /// What to do when the `BEL` character (`\a`) is printed to terminal.
    ///
    /// Default: "system"
    pub bell: Option<TerminalBell>,
    /// Whether to render inline images in the terminal (kitty graphics protocol
    /// and sixel). When disabled, image escape sequences are ignored.
    ///
    /// Default: true
    pub image_display: Option<bool>,
    /// Maximum bytes the terminal's image pool may hold for inline images.
    /// Older images are evicted once this budget is exceeded.
    ///
    /// Default: 536870912 (512 MiB)
    pub max_image_memory: Option<usize>,
    /// Whether to automatically set up Terry's shell integration for supported
    /// shells (zsh and fish). The injected scripts emit OSC 133 prompt marks
    /// and OSC 7 working directory reports, which enable prompt navigation
    /// (`ctrl-shift-up`). Bash users can source the script manually, see the
    /// documentation.
    ///
    /// Default: true
    pub shell_integration: Option<bool>,
    /// Whether terminal applications may read the system clipboard through
    /// OSC 52. Writing to the clipboard is always allowed, but reading lets a
    /// remote shell exfiltrate whatever you last copied, so it is opt-in.
    ///
    /// Default: false
    pub clipboard_read: Option<bool>,
    /// Whether to show a system notification when a shell command finishes
    /// while the terminal is not focused. Requires OSC 133 shell integration
    /// so Terry can detect command boundaries.
    ///
    /// Default: false
    pub notify_on_command_finish: Option<bool>,
    /// Whether to ask for confirmation before pasting text containing more
    /// than one line into the terminal. Choosing "Paste and Don't Ask Again"
    /// in the prompt sets this to `false`.
    ///
    /// Default: true
    pub confirm_multiline_paste: Option<bool>,
    /// Terminal background opacity, from `0.0` (fully transparent) to `1.0`
    /// (fully opaque).
    ///
    /// Values below 1 make the window translucent so the desktop shows
    /// through the terminal background. Combine with `background_blur` for a
    /// frosted-glass effect on platforms that support it (macOS, Windows 11).
    ///
    /// Default: 1.0
    #[schemars(range(min = 0.0, max = 1.0))]
    pub background_opacity: Option<f32>,
    /// Blurs the desktop behind a translucent terminal background. Only has an
    /// effect when `background_opacity` is below 1.0.
    ///
    /// Blur is a window-level effect, so enabling it also disables subpixel
    /// text antialiasing for the whole window.
    ///
    /// Default: false
    pub background_blur: Option<bool>,
    /// Horizontal padding between the terminal's edge and the character grid,
    /// in pixels.
    ///
    /// Default: 0
    pub padding_x: Option<f32>,
    /// Vertical padding between the terminal's edge and the character grid, in
    /// pixels.
    ///
    /// Default: 0
    pub padding_y: Option<f32>,
    /// Characters that terminate a word for semantic selection (double-click)
    /// and word-wise navigation in vi mode. Any character in this list
    /// separates words.
    ///
    /// Default: ",│`|:\"' ()[]{}<>\t"
    pub word_separators: Option<String>,
    /// Cursor shape used while vi mode is active. When unset, vi mode uses the
    /// regular `cursor_shape`.
    ///
    /// Can be "bar", "block", "underline", or "hollow".
    ///
    /// Default: null
    pub vi_cursor_shape: Option<CursorShapeContent>,
    /// Rules that refine what a double-click selects. The first rule whose
    /// regex matches the clicked cell wins; when no rule matches, Terry falls
    /// back to selecting the word around the click.
    ///
    /// Regexes are matched against the text of the clicked line, so anchor
    /// them only when needed. Named captures are ignored and the whole match is
    /// selected.
    ///
    /// Default: []
    pub smart_selection: Option<Vec<SmartSelectionRuleContent>>,
    /// Custom hint rules. Each rule is a regular expression matched against
    /// every visible line; matches are offered by the "Show Hints" action.
    ///
    /// Built-in hints cover URLs, filesystem paths, git hashes, UUIDs, and
    /// IP addresses. Rules here are evaluated after the URL and path hints,
    /// so a custom rule can refine other matches without hiding links.
    ///
    /// Default: []
    pub hints: Option<Vec<HintRuleContent>>,
    /// Extra SSH hosts offered by the "Connect to SSH Host…" picker, in
    /// addition to the hosts discovered in `~/.ssh/config`.
    ///
    /// Default: []
    pub ssh_hosts: Option<Vec<SshHostContent>>,
    /// Reuse SSH connections with `ControlMaster=auto` and `ControlPersist`, so
    /// opening a second terminal on the same host is instant (and does not ask
    /// for the password / passphrase again).
    ///
    /// Default: true
    pub ssh_reuse_connections: Option<bool>,
    /// Extra arguments appended to every SSH command Terry launches.
    ///
    /// Default: []
    pub ssh_extra_args: Option<Vec<String>>,
    /// Default terminal color scheme. Accepts a bundled or user-imported
    /// terminal color scheme (see `terminal_schemes/` in the config directory)
    /// or a UI theme name. Terminal color schemes only change terminal colors;
    /// a theme name uses that theme for the terminal.
    ///
    /// Profiles can override this per launch, and the color-scheme picker sets
    /// it per terminal.
    ///
    /// Default: null
    pub color_scheme: Option<String>,
    /// Named terminal profiles. Each profile can override the shell, working
    /// directory, environment, color scheme, and run a startup command after
    /// the shell is ready.
    ///
    /// Default: []
    pub profiles: Option<Vec<TerminalProfileContent>>,
    /// Name of the profile to use for new terminals. When unset, new terminals
    /// use the normal `shell` configuration.
    ///
    /// Default: null
    pub default_profile: Option<String>,
}

/// Shell configuration to open the terminal with.
#[derive(
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// Use the system's default terminal configuration in /etc/passwd
    #[default]
    System,
    /// Use a specific program with no arguments.
    Program(String),
    /// Use a specific program with arguments.
    WithArguments {
        /// The program to run.
        program: String,
        /// The arguments to pass to the program.
        args: Vec<String>,
        /// An optional string to override the title of the terminal tab
        title_override: Option<String>,
    },
}

#[derive(
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkingDirectory {
    #[default]
    /// Use the current file's directory, falling back to the project directory,
    /// then the first project in the workspace.
    CurrentFileDirectory,
    /// Use the current file's project directory. Fallback to the
    /// first project directory strategy if unsuccessful.
    CurrentProjectDirectory,
    /// Use the first project in this workspace's directory. Fallback to using
    /// this platform's home directory.
    FirstProjectDirectory,
    /// Always use this platform's home directory (if it can be found).
    AlwaysHome,
    /// Always use a specific directory. This value will be shell expanded.
    /// If this path is not a valid directory the terminal will default to
    /// this platform's home directory  (if it can be found).
    Always { directory: String },
}

#[with_fallible_options]
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq, Default,
)]
pub struct ScrollbarSettingsContent {
    /// When to show the scrollbar in the terminal.
    ///
    /// Default: inherits editor scrollbar settings
    pub show: Option<ShowScrollbar>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, MergeFrom, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight {
    /// Use a line height that's comfortable for reading, 1.618
    #[default]
    Comfortable,
    /// Use a standard line height, 1.3. This option is useful for TUIs,
    /// particularly if they use box characters
    Standard,
    /// Use a custom line height.
    Custom(#[serde(serialize_with = "crate::serialize_f32_with_two_decimal_places")] f32),
}

impl TerminalLineHeight {
    pub fn value(&self) -> f32 {
        match self {
            TerminalLineHeight::Comfortable => 1.618,
            TerminalLineHeight::Standard => 1.3,
            TerminalLineHeight::Custom(line_height) => f32::max(*line_height, 1.),
        }
    }
}

/// When to show the scrollbar.
///
/// Default: auto
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowScrollbar {
    /// Show the scrollbar if there's important information or
    /// follow the system's configured behavior.
    #[default]
    Auto,
    /// Match the system's configured behavior.
    System,
    /// Always show the scrollbar.
    Always,
    /// Never show the scrollbar.
    Never,
}

/// A smart-selection rule: a regex and what double-clicking a match does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct SmartSelectionRuleContent {
    /// Regular expression matched against the clicked line.
    pub regex: String,
    /// What to do with the matched text.
    ///
    /// Default: "select"
    pub action: Option<SmartSelectionActionContent>,
    /// Optional human-readable description, for documentation only.
    pub description: Option<String>,
}

/// What a smart-selection rule does with a match.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum SmartSelectionActionContent {
    /// Select the matched text.
    #[default]
    Select,
    /// Select the matched text and copy it to the clipboard.
    Copy,
}

/// A custom hint rule: a regex and what selecting a match does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct HintRuleContent {
    /// Optional human-readable label shown in the hints picker.
    pub name: Option<String>,
    /// Regular expression matched against each visible terminal line.
    pub regex: String,
    /// What to do with the matched text.
    ///
    /// Default: "copy"
    pub action: Option<HintActionContent>,
}

/// What a hint rule does with a match.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum HintActionContent {
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

/// A user-defined SSH host offered by the terminal's SSH picker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct SshHostContent {
    /// Display name / picker label. Defaults to `host`.
    pub name: Option<String>,
    /// Hostname or `~/.ssh/config` alias passed to `ssh`.
    pub host: String,
    /// User to log in as.
    pub user: Option<String>,
    /// Port to connect to.
    pub port: Option<u16>,
    /// Identity file, passed as `-i`.
    pub identity_file: Option<String>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
// todo() -> combine with CursorShape
pub enum CursorShapeContent {
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

#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TerminalBlink {
    /// Never blink the cursor, ignoring the terminal mode.
    Off,
    /// Default the cursor blink to off, but allow the terminal to
    /// set blinking.
    #[default]
    TerminalControlled,
    /// Always blink the cursor, ignoring the terminal mode.
    On,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AlternateScroll {
    #[default]
    On,
    Off,
}

// Toolbar related settings
#[with_fallible_options]
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq)]
pub struct TerminalToolbarContent {
    /// Whether to display the terminal title in breadcrumbs inside the terminal pane.
    /// Only shown if the terminal title is not empty.
    ///
    /// The shell running in the terminal needs to be configured to emit the title.
    /// Example: `echo -e "\e]2;New Title\007";`
    ///
    /// Default: true
    pub breadcrumbs: Option<bool>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBell {
    /// Play an OS-specific alert sound.
    #[default]
    System,
    /// Do not play any sound.
    Off,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, MergeFrom,
)]
#[serde(rename_all = "snake_case")]
pub enum CondaManager {
    /// Automatically detect the conda manager
    #[default]
    Auto,
    /// Use conda
    Conda,
    /// Use mamba
    Mamba,
    /// Use micromamba
    Micromamba,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum VenvSettings {
    #[default]
    Off,
    On {
        /// Default directories to search for virtual environments, relative
        /// to the current working directory. We recommend overriding this
        /// in your project's settings, rather than globally.
        activate_script: Option<ActivateScript>,
        venv_name: Option<String>,
        directories: Option<Vec<PathBuf>>,
        /// Preferred Conda manager to use when activating Conda environments.
        ///
        /// Default: auto
        conda_manager: Option<CondaManager>,
    },
}
#[with_fallible_options]
pub struct VenvSettingsContent<'a> {
    pub activate_script: ActivateScript,
    pub venv_name: &'a str,
    pub directories: &'a [PathBuf],
    pub conda_manager: CondaManager,
}

impl VenvSettings {
    pub fn as_option(&self) -> Option<VenvSettingsContent<'_>> {
        match self {
            VenvSettings::Off => None,
            VenvSettings::On {
                activate_script,
                venv_name,
                directories,
                conda_manager,
            } => Some(VenvSettingsContent {
                activate_script: activate_script.unwrap_or(ActivateScript::Default),
                venv_name: venv_name.as_deref().unwrap_or(""),
                directories: directories.as_deref().unwrap_or(&[]),
                conda_manager: conda_manager.unwrap_or(CondaManager::Auto),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, MergeFrom)]
#[serde(untagged)]
pub enum PathHyperlinkRegex {
    SingleLine(String),
    MultiLine(Vec<String>),
}

#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TerminalDockPosition {
    Left,
    #[default]
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub enum ActivateScript {
    #[default]
    Default,
    Csh,
    Fish,
    Nushell,
    PowerShell,
    Pyenv,
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use crate::{ProjectSettingsContent, Shell};

    #[test]
    #[ignore]
    fn test_project_settings() {
        let project_content =
            json!({"terminal": {"shell": {"program": "/bin/project"}}, "option_as_meta": true});

        let _user_content =
            json!({"terminal": {"shell": {"program": "/bin/user"}}, "option_as_meta": false});

        let project_settings =
            serde_json::from_value::<ProjectSettingsContent>(project_content).unwrap();

        assert_eq!(
            project_settings.terminal.unwrap().shell,
            Some(Shell::Program("/bin/project".to_owned()))
        );
    }
}
