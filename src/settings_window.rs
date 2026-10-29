use fs::Fs;
use gpui::{
    Action, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    img, px,
};
use platform_title_bar::PlatformTitleBar;
use settings::{Settings, SettingsStore, UiLanguage, update_settings_file};
use strum::VariantArray;
use terminal::terminal_settings::{CursorShape as TerminalCursorShape, TerminalSettings};
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, DropdownMenu, IconPosition, Label,
    LabelSize, Switch, ToggleButtonGroup, ToggleButtonGroupStyle, ToggleButtonSimple, ToggleState,
    prelude::*,
};
use util::ResultExt;
use vim_mode_setting::VimModeSetting;

use crate::window_title_bar;

/// Must stay aligned with [`UiLanguage`] variant declaration order.
const LANGUAGE_LABELS: &[&str] = &[
    "System",
    "English",
    "简体中文",
    "繁體中文",
    "日本語",
    "한국어",
    "Español",
    "Français",
    "Deutsch",
    "Português (Brasil)",
    "Русский",
    "العربية",
    "हिन्दी",
    "Italiano",
    "Nederlands",
    "Türkçe",
    "Polski",
    "Tiếng Việt",
    "ไทย",
    "Bahasa Indonesia",
    "Українська",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Appearance,
    Editor,
    Terminal,
    Keyboard,
    Agent,
    About,
}

impl SettingsTab {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Appearance,
        Self::Editor,
        Self::Terminal,
        Self::Keyboard,
        Self::Agent,
        Self::About,
    ];

    fn label(self) -> String {
        match self {
            Self::General => i18n::t("settings_tab_general"),
            Self::Appearance => i18n::t("appearance"),
            Self::Editor => i18n::t("settings_tab_editor"),
            Self::Terminal => i18n::t("terminal"),
            Self::Keyboard => i18n::t("settings_tab_keyboard"),
            Self::Agent => i18n::t("agent"),
            Self::About => i18n::t("settings_tab_about"),
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }
}

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    active_tab: SettingsTab,
    language_menu: Entity<ContextMenu>,
    font_size_menu: Entity<ContextMenu>,
    font_family_menu: Entity<ContextMenu>,
    last_language: Option<UiLanguage>,
    /// Drawn only on Linux when the window falls back to client-side
    /// decorations (e.g. GNOME on Wayland), which have to be provided by us.
    title_bar: Option<Entity<PlatformTitleBar>>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let language = Self::current_language(cx);
        let language_menu = build_language_menu(language, window, cx);
        let font_size_menu = build_font_size_menu(window, cx);
        let font_family_menu = build_font_family_menu(window, cx);
        let title_bar = window_title_bar::new_window_title_bar("terry-settings-title-bar", cx);
        cx.observe_global::<SettingsStore>(|_, cx| cx.notify())
            .detach();
        Self {
            focus_handle,
            active_tab: SettingsTab::General,
            language_menu,
            font_size_menu,
            font_family_menu,
            last_language: Some(language),
            title_bar,
        }
    }

    fn current_language(cx: &App) -> UiLanguage {
        i18n::UiLanguageSetting::get_global(cx).0
    }

    fn current_language_label(cx: &App) -> SharedString {
        let current = Self::current_language(cx);
        let variants = UiLanguage::VARIANTS;
        variants
            .iter()
            .position(|v| *v == current)
            .and_then(|i| LANGUAGE_LABELS.get(i).copied())
            .map(|label| {
                if current == UiLanguage::System {
                    i18n::t("language_system")
                } else {
                    label.to_string()
                }
            })
            .unwrap_or_else(|| i18n::t("language_system"))
            .into()
    }

    fn select_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;
            cx.notify();
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_tab;
        ToggleButtonGroup::single_row(
            "settings-tabs",
            [
                ToggleButtonSimple::new(
                    SettingsTab::General.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::General, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::Appearance.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Appearance, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::Editor.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Editor, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::Terminal.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Terminal, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::Keyboard.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Keyboard, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::Agent.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Agent, cx);
                    }),
                ),
                ToggleButtonSimple::new(
                    SettingsTab::About.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::About, cx);
                    }),
                ),
            ],
        )
        .selected_index(active.index())
        .style(ToggleButtonGroupStyle::Outlined)
        .label_size(LabelSize::Small)
        .auto_width()
        .into_any_element()
    }

    fn render_general_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let language_label = Self::current_language_label(cx);
        let (auto_update_enabled, reduce_motion, hide_mouse) = {
            let content = cx.global::<SettingsStore>().merged_settings();
            (
                content.auto_update.unwrap_or(true),
                matches!(content.reduce_motion, Some(settings::ReduceMotionMode::On)),
                content.hide_mouse.unwrap_or_default(),
            )
        };

        let hide_mouse_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("hide_mouse_never"), |content| {
                    content.hide_mouse = Some(settings::HideMouseMode::Never);
                }),
                setting_option(i18n::t("hide_mouse_on_typing"), |content| {
                    content.hide_mouse = Some(settings::HideMouseMode::OnTyping);
                }),
                setting_option(i18n::t("hide_mouse_on_typing_and_action"), |content| {
                    content.hide_mouse = Some(settings::HideMouseMode::OnTypingAndAction);
                }),
            ],
        );

        v_flex()
            .gap_4()
            .child(settings_section(
                i18n::t("ui_language"),
                i18n::t("ui_language_description"),
                DropdownMenu::new("ui-language", language_label, self.language_menu.clone())
                    .style(ui::DropdownStyle::Outlined)
                    .trigger_size(ButtonSize::Medium)
                    .into_any_element(),
            ))
            .child(settings_switch(
                "auto-update",
                i18n::t("auto_update"),
                i18n::t("auto_update_description"),
                auto_update_enabled,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.auto_update = Some(enabled);
                    });
                },
            ))
            .child(settings_switch(
                "reduce-motion",
                i18n::t("reduce_motion"),
                i18n::t("reduce_motion_description"),
                reduce_motion,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.reduce_motion = Some(if enabled {
                            settings::ReduceMotionMode::On
                        } else {
                            settings::ReduceMotionMode::Off
                        });
                    });
                },
            ))
            .child(settings_dropdown(
                "hide-mouse",
                i18n::t("hide_mouse"),
                i18n::t("hide_mouse_description"),
                hide_mouse_label(hide_mouse),
                hide_mouse_menu,
            ))
            .into_any_element()
    }

    fn render_appearance_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let current_font_size = theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx);
        let current_font_family_raw = theme_settings::ThemeSettings::get_global(cx)
            .buffer_font
            .family
            .clone();
        let current_line_height = theme_settings::ThemeSettings::get_global(cx).buffer_line_height;
        let current_font_family = if current_font_family_raw.as_ref() == ".SystemUIFont" {
            SharedString::from(i18n::t("font_system"))
        } else {
            current_font_family_raw
        };

        let line_height_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("line_height_comfortable"), |content| {
                    content.theme.buffer_line_height =
                        Some(settings::BufferLineHeight::Comfortable);
                }),
                setting_option(i18n::t("line_height_standard"), |content| {
                    content.theme.buffer_line_height = Some(settings::BufferLineHeight::Standard);
                }),
            ],
        );

        v_flex()
            .gap_4()
            .child(settings_section(
                i18n::t("appearance"),
                i18n::t("appearance_description"),
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Label::new(i18n::t("font_family"))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        DropdownMenu::new(
                                            "ui-font-family",
                                            current_font_family,
                                            self.font_family_menu.clone(),
                                        )
                                        .style(ui::DropdownStyle::Outlined)
                                        .trigger_size(ButtonSize::Medium),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        Label::new(i18n::t("font_size"))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        DropdownMenu::new(
                                            "ui-font-size",
                                            format!("{}px", f32::from(current_font_size)),
                                            self.font_size_menu.clone(),
                                        )
                                        .style(ui::DropdownStyle::Outlined)
                                        .trigger_size(ButtonSize::Medium),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("select-theme", i18n::t("select_theme"))
                                    .style(ButtonStyle::Outlined)
                                    .size(ButtonSize::Medium)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            zed_actions::theme_selector::Toggle::default()
                                                .boxed_clone(),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                Button::new("open-theme-generator", i18n::t("theme_generator"))
                                    .style(ButtonStyle::Outlined)
                                    .size(ButtonSize::Medium)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            crate::theme_generator::OpenThemeGenerator
                                                .boxed_clone(),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("line_height"),
                i18n::t("line_height_description"),
                DropdownMenu::new(
                    "ui-line-height",
                    line_height_label(current_line_height),
                    line_height_menu,
                )
                .style(ui::DropdownStyle::Outlined)
                .trigger_size(ButtonSize::Medium)
                .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_editor_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let editor_settings = editor::EditorSettings::get_global(cx);
        let cursor_blink = editor_settings.cursor_blink;
        let cursor_shape = editor_settings.cursor_shape;
        let relative_line_numbers = editor_settings.relative_line_numbers;
        let autosave = workspace::WorkspaceSettings::get_global(cx).autosave;
        let vim_enabled = VimModeSetting::is_enabled(cx);
        let helix_enabled = vim_mode_setting::HelixModeSetting::is_enabled(cx);
        let all_languages = language::language_settings::AllLanguageSettings::get_global(cx);
        let format_on_save = all_languages.defaults.format_on_save;
        let soft_wrap = all_languages.defaults.soft_wrap.clone();
        let show_whitespaces = all_languages.defaults.show_whitespaces;

        let cursor_shape_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("cursor_shape_bar"), |content| {
                    content.editor.cursor_shape = Some(settings::CursorShape::Bar);
                }),
                setting_option(i18n::t("cursor_shape_block"), |content| {
                    content.editor.cursor_shape = Some(settings::CursorShape::Block);
                }),
                setting_option(i18n::t("cursor_shape_underline"), |content| {
                    content.editor.cursor_shape = Some(settings::CursorShape::Underline);
                }),
                setting_option(i18n::t("cursor_shape_hollow"), |content| {
                    content.editor.cursor_shape = Some(settings::CursorShape::Hollow);
                }),
            ],
        );

        let relative_line_numbers_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("relative_line_numbers_off"), |content| {
                    content.editor.relative_line_numbers =
                        Some(settings::RelativeLineNumbers::Disabled);
                }),
                setting_option(i18n::t("relative_line_numbers_on"), |content| {
                    content.editor.relative_line_numbers =
                        Some(settings::RelativeLineNumbers::Enabled);
                }),
                setting_option(i18n::t("relative_line_numbers_wrapped"), |content| {
                    content.editor.relative_line_numbers =
                        Some(settings::RelativeLineNumbers::Wrapped);
                }),
            ],
        );

        let autosave_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("autosave_off"), |content| {
                    content.workspace.autosave = Some(settings::AutosaveSetting::Off);
                }),
                setting_option(i18n::t("autosave_on_focus_change"), |content| {
                    content.workspace.autosave = Some(settings::AutosaveSetting::OnFocusChange);
                }),
                setting_option(i18n::t("autosave_on_window_change"), |content| {
                    content.workspace.autosave = Some(settings::AutosaveSetting::OnWindowChange);
                }),
            ],
        );

        let format_on_save_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("format_on_save_off"), |content| {
                    content.project.all_languages.defaults.format_on_save =
                        Some(settings::FormatOnSave::Off);
                }),
                setting_option(i18n::t("format_on_save_on"), |content| {
                    content.project.all_languages.defaults.format_on_save =
                        Some(settings::FormatOnSave::On);
                }),
                setting_option(i18n::t("format_on_save_modifications"), |content| {
                    content.project.all_languages.defaults.format_on_save =
                        Some(settings::FormatOnSave::Modifications);
                }),
            ],
        );

        let soft_wrap_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("soft_wrap_off"), |content| {
                    content.project.all_languages.defaults.soft_wrap =
                        Some(settings::SoftWrap::None);
                }),
                setting_option(i18n::t("soft_wrap_editor_width"), |content| {
                    content.project.all_languages.defaults.soft_wrap =
                        Some(settings::SoftWrap::EditorWidth);
                }),
                setting_option(i18n::t("soft_wrap_bounded"), |content| {
                    content.project.all_languages.defaults.soft_wrap =
                        Some(settings::SoftWrap::Bounded);
                }),
            ],
        );

        let show_whitespaces_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("show_whitespaces_none"), |content| {
                    content.project.all_languages.defaults.show_whitespaces =
                        Some(settings::ShowWhitespaceSetting::None);
                }),
                setting_option(i18n::t("show_whitespaces_selection"), |content| {
                    content.project.all_languages.defaults.show_whitespaces =
                        Some(settings::ShowWhitespaceSetting::Selection);
                }),
                setting_option(i18n::t("show_whitespaces_boundary"), |content| {
                    content.project.all_languages.defaults.show_whitespaces =
                        Some(settings::ShowWhitespaceSetting::Boundary);
                }),
                setting_option(i18n::t("show_whitespaces_trailing"), |content| {
                    content.project.all_languages.defaults.show_whitespaces =
                        Some(settings::ShowWhitespaceSetting::Trailing);
                }),
                setting_option(i18n::t("show_whitespaces_all"), |content| {
                    content.project.all_languages.defaults.show_whitespaces =
                        Some(settings::ShowWhitespaceSetting::All);
                }),
            ],
        );

        let vim_state = if vim_enabled {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        };

        v_flex()
            .gap_4()
            .child(settings_section(
                i18n::t("vim_mode"),
                i18n::t("vim_mode_description"),
                Switch::new("vim-mode", vim_state)
                    .label(i18n::t("vim_mode"))
                    .on_click(|state, _window, cx| {
                        let enabled = *state == ToggleState::Selected;
                        let fs = <dyn Fs>::global(cx);
                        update_settings_file(fs, cx, move |content, _| {
                            content.vim_mode = Some(enabled);
                            if enabled {
                                content.helix_mode = Some(false);
                            }
                        });
                    })
                    .into_any_element(),
            ))
            .child(settings_switch(
                "helix-mode",
                i18n::t("helix_mode"),
                i18n::t("helix_mode_description"),
                helix_enabled,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.helix_mode = Some(enabled);
                        if enabled {
                            content.vim_mode = Some(false);
                        }
                    });
                },
            ))
            .child(settings_switch(
                "editor-cursor-blink",
                i18n::t("cursor_blink"),
                i18n::t("cursor_blink_description"),
                cursor_blink,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.editor.cursor_blink = Some(enabled);
                    });
                },
            ))
            .child(settings_dropdown(
                "editor-cursor-shape",
                i18n::t("cursor_shape"),
                i18n::t("cursor_shape_description"),
                editor_cursor_shape_label(cursor_shape),
                cursor_shape_menu,
            ))
            .child(settings_dropdown(
                "editor-relative-line-numbers",
                i18n::t("relative_line_numbers"),
                i18n::t("relative_line_numbers_description"),
                relative_line_numbers_label(relative_line_numbers),
                relative_line_numbers_menu,
            ))
            .child(settings_dropdown(
                "editor-autosave",
                i18n::t("autosave"),
                i18n::t("autosave_description"),
                autosave_label(autosave),
                autosave_menu,
            ))
            .child(settings_dropdown(
                "editor-format-on-save",
                i18n::t("format_on_save"),
                i18n::t("format_on_save_description"),
                format_on_save_label(format_on_save),
                format_on_save_menu,
            ))
            .child(settings_dropdown(
                "editor-soft-wrap",
                i18n::t("soft_wrap"),
                i18n::t("soft_wrap_description"),
                soft_wrap_label(soft_wrap),
                soft_wrap_menu,
            ))
            .child(settings_dropdown(
                "editor-show-whitespaces",
                i18n::t("show_whitespaces"),
                i18n::t("show_whitespaces_description"),
                show_whitespaces_label(show_whitespaces),
                show_whitespaces_menu,
            ))
            .into_any_element()
    }

    fn render_terminal_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let terminal = TerminalSettings::get_global(cx);
        let shell_integration = terminal.shell_integration;
        let persistent_sessions = terminal.persistent_sessions;
        let clipboard_read = terminal.clipboard_read;
        let notify_on_command_finish = terminal.notify_on_command_finish;
        let confirm_multiline_paste = terminal.confirm_multiline_paste;
        let copy_on_select = terminal.copy_on_select;
        let cursor_shape = terminal.cursor_shape;
        let blinking = terminal.blinking;
        let ligatures = terminal.ligatures;
        let background_opacity = terminal.background_opacity;
        let background_blur = terminal.background_blur;
        let image_display = terminal.image_display;
        let bell = terminal.bell;
        let unlimited_scroll_history = terminal.unlimited_scroll_history();
        let profiles = terminal.profiles.clone();
        let default_profile = terminal.default_profile.clone();

        let cursor_shape_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("cursor_shape_block"), |content| {
                    content.terminal.get_or_insert_default().cursor_shape =
                        Some(settings::CursorShapeContent::Block);
                }),
                setting_option(i18n::t("cursor_shape_underline"), |content| {
                    content.terminal.get_or_insert_default().cursor_shape =
                        Some(settings::CursorShapeContent::Underline);
                }),
                setting_option(i18n::t("cursor_shape_bar"), |content| {
                    content.terminal.get_or_insert_default().cursor_shape =
                        Some(settings::CursorShapeContent::Bar);
                }),
                setting_option(i18n::t("cursor_shape_hollow"), |content| {
                    content.terminal.get_or_insert_default().cursor_shape =
                        Some(settings::CursorShapeContent::Hollow);
                }),
            ],
        );

        let blinking_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option(i18n::t("terminal_blink_off"), |content| {
                    content.terminal.get_or_insert_default().blinking =
                        Some(settings::TerminalBlink::Off);
                }),
                setting_option(i18n::t("terminal_blink_follow"), |content| {
                    content.terminal.get_or_insert_default().blinking =
                        Some(settings::TerminalBlink::TerminalControlled);
                }),
                setting_option(i18n::t("terminal_blink_on"), |content| {
                    content.terminal.get_or_insert_default().blinking =
                        Some(settings::TerminalBlink::On);
                }),
            ],
        );

        let opacity_menu = build_option_menu(
            window,
            cx,
            vec![
                setting_option("100%", |content| {
                    content.terminal.get_or_insert_default().background_opacity = Some(1.0);
                }),
                setting_option("95%", |content| {
                    content.terminal.get_or_insert_default().background_opacity = Some(0.95);
                }),
                setting_option("90%", |content| {
                    content.terminal.get_or_insert_default().background_opacity = Some(0.9);
                }),
                setting_option("85%", |content| {
                    content.terminal.get_or_insert_default().background_opacity = Some(0.85);
                }),
                setting_option("80%", |content| {
                    content.terminal.get_or_insert_default().background_opacity = Some(0.8);
                }),
            ],
        );

        v_flex()
            .gap_4()
            .child(settings_section(
                i18n::t("shell_integration"),
                i18n::t("shell_integration_description"),
                Switch::new(
                    "shell-integration",
                    if shell_integration {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .label(i18n::t("shell_integration"))
                .on_click(|state, _window, cx| {
                    let enabled = *state == ToggleState::Selected;
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().shell_integration = Some(enabled);
                    });
                })
                .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("persistent_sessions"),
                i18n::t("persistent_sessions_description"),
                Switch::new(
                    "persistent-sessions",
                    if persistent_sessions {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .label(i18n::t("persistent_sessions"))
                .on_click(|state, _window, cx| {
                    let enabled = *state == ToggleState::Selected;
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().persistent_sessions =
                            Some(enabled);
                    });
                })
                .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("notify_on_command_finish"),
                i18n::t("notify_on_command_finish_description"),
                Switch::new(
                    "notify-on-command-finish",
                    if notify_on_command_finish {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .label(i18n::t("notify_on_command_finish"))
                .on_click(|state, _window, cx| {
                    let enabled = *state == ToggleState::Selected;
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content
                            .terminal
                            .get_or_insert_default()
                            .notify_on_command_finish = Some(enabled);
                    });
                })
                .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("confirm_multiline_paste"),
                i18n::t("confirm_multiline_paste_description"),
                Switch::new(
                    "confirm-multiline-paste",
                    if confirm_multiline_paste {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .label(i18n::t("confirm_multiline_paste"))
                .on_click(|state, _window, cx| {
                    let enabled = *state == ToggleState::Selected;
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content
                            .terminal
                            .get_or_insert_default()
                            .confirm_multiline_paste = Some(enabled);
                    });
                })
                .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("clipboard_read"),
                i18n::t("clipboard_read_description"),
                Switch::new(
                    "clipboard-read",
                    if clipboard_read {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .label(i18n::t("clipboard_read"))
                .on_click(|state, _window, cx| {
                    let enabled = *state == ToggleState::Selected;
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().clipboard_read = Some(enabled);
                    });
                })
                .into_any_element(),
            ))
            .child(settings_switch(
                "terminal-copy-on-select",
                i18n::t("terminal_copy_on_select"),
                i18n::t("terminal_copy_on_select_description"),
                copy_on_select,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().copy_on_select = Some(enabled);
                    });
                },
            ))
            .child(settings_dropdown(
                "terminal-cursor-shape",
                i18n::t("cursor_shape"),
                i18n::t("terminal_cursor_shape_description"),
                terminal_cursor_shape_label(cursor_shape),
                cursor_shape_menu,
            ))
            .child(settings_dropdown(
                "terminal-blinking",
                i18n::t("terminal_blinking"),
                i18n::t("terminal_blinking_description"),
                terminal_blink_label(blinking),
                blinking_menu,
            ))
            .child(settings_switch(
                "terminal-ligatures",
                i18n::t("terminal_ligatures"),
                i18n::t("terminal_ligatures_description"),
                ligatures,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().ligatures = Some(enabled);
                    });
                },
            ))
            .child(settings_dropdown(
                "terminal-background-opacity",
                i18n::t("terminal_background_opacity"),
                i18n::t("terminal_background_opacity_description"),
                format!("{}%", (background_opacity * 100.0).round() as i32),
                opacity_menu,
            ))
            .child(settings_switch(
                "terminal-background-blur",
                i18n::t("terminal_background_blur"),
                i18n::t("terminal_background_blur_description"),
                background_blur,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().background_blur = Some(enabled);
                    });
                },
            ))
            .child(settings_switch(
                "terminal-image-display",
                i18n::t("terminal_image_display"),
                i18n::t("terminal_image_display_description"),
                image_display,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().image_display = Some(enabled);
                    });
                },
            ))
            .child(settings_switch(
                "terminal-bell",
                i18n::t("terminal_bell"),
                i18n::t("terminal_bell_description"),
                matches!(bell, settings::TerminalBell::System),
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.terminal.get_or_insert_default().bell = Some(if enabled {
                            settings::TerminalBell::System
                        } else {
                            settings::TerminalBell::Off
                        });
                    });
                },
            ))
            .child(settings_switch(
                "terminal-unlimited-scroll-history",
                i18n::t("terminal_unlimited_scroll_history"),
                i18n::t("terminal_unlimited_scroll_history_description"),
                unlimited_scroll_history,
                |enabled, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content
                            .terminal
                            .get_or_insert_default()
                            .unlimited_scroll_history = Some(enabled);
                    });
                },
            ))
            .child(settings_section(
                i18n::t("terminal_profiles"),
                i18n::t("terminal_profiles_description"),
                v_flex()
                    .gap_2()
                    .children(if profiles.is_empty() {
                        vec![
                            Label::new(i18n::t("no_terminal_profiles"))
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .into_any_element(),
                        ]
                    } else {
                        profiles
                            .into_iter()
                            .map(|profile| {
                                let name = profile.name.clone();
                                let is_default = default_profile.as_deref() == Some(name.as_str());
                                let button_label = if is_default {
                                    i18n::t("clear_default_profile")
                                } else {
                                    i18n::t("set_as_default")
                                };
                                let mut row = h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(IconName::Terminal)
                                            .size(IconSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(Label::new(name.clone()).size(LabelSize::Small));
                                if is_default {
                                    row = row.child(
                                        Label::new(i18n::t("default_label"))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Accent),
                                    );
                                }
                                row.child(div().flex_grow(1.))
                                    .child(
                                        Button::new(
                                            SharedString::from(format!("profile-default-{name}")),
                                            button_label,
                                        )
                                        .style(ButtonStyle::Outlined)
                                        .size(ButtonSize::Compact)
                                        .on_click(
                                            move |_, _, cx| {
                                                let new_default = if is_default {
                                                    None
                                                } else {
                                                    Some(name.clone())
                                                };
                                                let fs = <dyn Fs>::global(cx);
                                                update_settings_file(fs, cx, move |content, _| {
                                                    content
                                                        .terminal
                                                        .get_or_insert_default()
                                                        .default_profile = new_default.clone();
                                                });
                                            },
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .collect()
                    })
                    .child(
                        Button::new("open-terminal-settings", i18n::t("edit_terminal_profiles"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Compact)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    zed_actions::OpenSettingsFile.boxed_clone(),
                                    cx,
                                );
                            }),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_keyboard_tab(&self, _cx: &App) -> AnyElement {
        settings_section(
            i18n::t("custom_shortcuts"),
            i18n::t("keymap_settings_description"),
            Button::new("open-keymaps", i18n::t("keymap_settings"))
                .style(ButtonStyle::Outlined)
                .size(ButtonSize::Medium)
                .on_click(|_, window, cx| {
                    window.dispatch_action(zed_actions::OpenKeymap.boxed_clone(), cx);
                })
                .into_any_element(),
        )
    }

    fn render_agent_tab(&self, _cx: &App) -> AnyElement {
        settings_section(
            i18n::t("llm_providers"),
            i18n::t("llm_providers_description"),
            Button::new("open-llm-providers", i18n::t("llm_providers"))
                .style(ButtonStyle::Outlined)
                .size(ButtonSize::Medium)
                .on_click(|_, window, cx| {
                    window.dispatch_action(
                        Box::new(crate::llm_provider_settings::OpenLlmProviderSettings),
                        cx,
                    );
                })
                .into_any_element(),
        )
    }

    fn render_about_tab(&self, _cx: &App) -> AnyElement {
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .py_2()
                    .child(
                        img("images/terry_logo.png")
                            .size(rems_from_px(72.))
                            .rounded_xl()
                            .overflow_hidden(),
                    )
                    .child(Label::new("Terry"))
                    .child(
                        div().max_w(px(320.)).text_center().child(
                            Label::new(i18n::t("about_terry_description"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    ),
            )
            .child(settings_section(
                i18n::t("about_version"),
                i18n::t("about_version_description"),
                Label::new(env!("TERRY_VERSION")).into_any_element(),
            ))
            .child(settings_section(
                i18n::t("about_website"),
                i18n::t("about_website_description"),
                Button::new("open-terry-website", "https://bindoffice.com")
                    .style(ButtonStyle::Outlined)
                    .size(ButtonSize::Medium)
                    .on_click(|_, _, cx| cx.open_url("https://bindoffice.com"))
                    .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("about_updates"),
                i18n::t("about_updates_description"),
                Button::new("about-check-for-updates", i18n::t("check_for_updates"))
                    .style(ButtonStyle::Outlined)
                    .size(ButtonSize::Medium)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(update_checker::CheckForUpdates.boxed_clone(), cx);
                    })
                    .into_any_element(),
            ))
            .child(settings_section(
                i18n::t("about_support"),
                i18n::t("about_support_description"),
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-config-folder", i18n::t("open_config_folder"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .on_click(|_, _, cx| cx.open_with_system(paths::config_dir())),
                    )
                    .child(
                        Button::new("open-log-folder", i18n::t("open_log_folder"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .on_click(|_, _, cx| {
                                let path = paths::log_file()
                                    .parent()
                                    .unwrap_or_else(|| paths::config_dir().as_path());
                                cx.open_with_system(path);
                            }),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.active_tab {
            SettingsTab::General => self.render_general_tab(window, cx),
            SettingsTab::Appearance => self.render_appearance_tab(window, cx),
            SettingsTab::Editor => self.render_editor_tab(window, cx),
            SettingsTab::Terminal => self.render_terminal_tab(window, cx),
            SettingsTab::Keyboard => self.render_keyboard_tab(cx),
            SettingsTab::Agent => self.render_agent_tab(cx),
            SettingsTab::About => self.render_about_tab(cx),
        }
    }
}

fn settings_section(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: AnyElement,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(Label::new(title))
        .child(
            Label::new(description)
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
        .child(control)
        .into_any_element()
}

type MenuHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// Builds a dropdown menu from a list of `(label, handler)` options.
fn build_option_menu(
    window: &mut Window,
    cx: &mut App,
    options: Vec<(SharedString, MenuHandler)>,
) -> Entity<ContextMenu> {
    ContextMenu::build(window, cx, move |mut menu, _, _| {
        for (label, handler) in options {
            menu = menu.entry(label, None, handler);
        }
        menu
    })
}

/// A menu option that applies a change to the user settings file.
fn setting_option(
    label: impl Into<SharedString>,
    update: fn(&mut settings::SettingsContent),
) -> (SharedString, MenuHandler) {
    (
        label.into(),
        Box::new(move |_window, cx| {
            let fs = <dyn Fs>::global(cx);
            update_settings_file(fs, cx, move |content, _| update(content));
        }),
    )
}

/// A labelled on/off setting row.
fn settings_switch(
    id: &'static str,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    enabled: bool,
    on_toggle: impl Fn(bool, &mut App) + 'static,
) -> AnyElement {
    let title = title.into();
    let state = if enabled {
        ToggleState::Selected
    } else {
        ToggleState::Unselected
    };
    settings_section(
        title.clone(),
        description,
        Switch::new(id, state)
            .label(title)
            .on_click(move |state, _window, cx| {
                on_toggle(*state == ToggleState::Selected, cx);
            })
            .into_any_element(),
    )
}

/// A labelled dropdown setting row.
fn settings_dropdown(
    id: &'static str,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    current: impl Into<SharedString>,
    menu: Entity<ContextMenu>,
) -> AnyElement {
    settings_section(
        title,
        description,
        DropdownMenu::new(id, current, menu)
            .style(ui::DropdownStyle::Outlined)
            .trigger_size(ButtonSize::Medium)
            .into_any_element(),
    )
}

fn editor_cursor_shape_label(shape: Option<language::CursorShape>) -> SharedString {
    match shape {
        Some(language::CursorShape::Block) => i18n::t("cursor_shape_block").into(),
        Some(language::CursorShape::Underline) => i18n::t("cursor_shape_underline").into(),
        Some(language::CursorShape::Hollow) => i18n::t("cursor_shape_hollow").into(),
        Some(language::CursorShape::Bar) | None => i18n::t("cursor_shape_bar").into(),
    }
}

fn terminal_cursor_shape_label(shape: TerminalCursorShape) -> SharedString {
    match shape {
        TerminalCursorShape::Block => i18n::t("cursor_shape_block").into(),
        TerminalCursorShape::Underline => i18n::t("cursor_shape_underline").into(),
        TerminalCursorShape::Bar => i18n::t("cursor_shape_bar").into(),
        TerminalCursorShape::Hollow => i18n::t("cursor_shape_hollow").into(),
    }
}

fn relative_line_numbers_label(value: settings::RelativeLineNumbers) -> SharedString {
    match value {
        settings::RelativeLineNumbers::Disabled => i18n::t("relative_line_numbers_off").into(),
        settings::RelativeLineNumbers::Enabled => i18n::t("relative_line_numbers_on").into(),
        settings::RelativeLineNumbers::Wrapped => i18n::t("relative_line_numbers_wrapped").into(),
    }
}

fn autosave_label(value: settings::AutosaveSetting) -> SharedString {
    match value {
        settings::AutosaveSetting::Off => i18n::t("autosave_off").into(),
        settings::AutosaveSetting::AfterDelay { .. } => i18n::t("autosave_after_delay").into(),
        settings::AutosaveSetting::OnFocusChange => i18n::t("autosave_on_focus_change").into(),
        settings::AutosaveSetting::OnWindowChange => i18n::t("autosave_on_window_change").into(),
    }
}

fn terminal_blink_label(value: settings::TerminalBlink) -> SharedString {
    match value {
        settings::TerminalBlink::Off => i18n::t("terminal_blink_off").into(),
        settings::TerminalBlink::TerminalControlled => i18n::t("terminal_blink_follow").into(),
        settings::TerminalBlink::On => i18n::t("terminal_blink_on").into(),
    }
}

fn line_height_label(value: theme_settings::BufferLineHeight) -> SharedString {
    match value {
        theme_settings::BufferLineHeight::Comfortable => i18n::t("line_height_comfortable").into(),
        theme_settings::BufferLineHeight::Standard => i18n::t("line_height_standard").into(),
        theme_settings::BufferLineHeight::Custom(_) => i18n::t("line_height_custom").into(),
    }
}

fn hide_mouse_label(value: settings::HideMouseMode) -> SharedString {
    match value {
        settings::HideMouseMode::Never => i18n::t("hide_mouse_never").into(),
        settings::HideMouseMode::OnTyping => i18n::t("hide_mouse_on_typing").into(),
        settings::HideMouseMode::OnTypingAndAction => {
            i18n::t("hide_mouse_on_typing_and_action").into()
        }
    }
}

fn format_on_save_label(value: settings::FormatOnSave) -> SharedString {
    match value {
        settings::FormatOnSave::On => i18n::t("format_on_save_on").into(),
        settings::FormatOnSave::Off => i18n::t("format_on_save_off").into(),
        settings::FormatOnSave::Modifications
        | settings::FormatOnSave::ModificationsIfAvailable => {
            i18n::t("format_on_save_modifications").into()
        }
    }
}

fn soft_wrap_label(value: settings::SoftWrap) -> SharedString {
    match value {
        settings::SoftWrap::None | settings::SoftWrap::PreferLine => {
            i18n::t("soft_wrap_off").into()
        }
        settings::SoftWrap::EditorWidth => i18n::t("soft_wrap_editor_width").into(),
        settings::SoftWrap::Bounded => i18n::t("soft_wrap_bounded").into(),
    }
}

fn show_whitespaces_label(value: settings::ShowWhitespaceSetting) -> SharedString {
    match value {
        settings::ShowWhitespaceSetting::None => i18n::t("show_whitespaces_none").into(),
        settings::ShowWhitespaceSetting::Selection => i18n::t("show_whitespaces_selection").into(),
        settings::ShowWhitespaceSetting::Boundary => i18n::t("show_whitespaces_boundary").into(),
        settings::ShowWhitespaceSetting::Trailing => i18n::t("show_whitespaces_trailing").into(),
        settings::ShowWhitespaceSetting::All => i18n::t("show_whitespaces_all").into(),
    }
}

/// Applies globally-scoped UI preferences that are read directly by GPUI.
fn apply_ui_preferences(cx: &mut App) {
    let (reduce_motion, hide_mouse) = {
        let content = cx.global::<SettingsStore>().merged_settings();
        (
            matches!(content.reduce_motion, Some(settings::ReduceMotionMode::On)),
            content.hide_mouse.unwrap_or_default(),
        )
    };
    cx.set_reduce_motion(reduce_motion);
    cx.set_cursor_hide_mode(match hide_mouse {
        settings::HideMouseMode::Never => gpui::CursorHideMode::Never,
        settings::HideMouseMode::OnTyping => gpui::CursorHideMode::OnTyping,
        settings::HideMouseMode::OnTypingAndAction => gpui::CursorHideMode::OnTypingAndAction,
    });
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = Self::current_language(cx);
        if self.last_language != Some(language) {
            self.language_menu = build_language_menu(language, window, cx);
            self.last_language = Some(language);
        }

        self.font_family_menu = build_font_family_menu(window, cx);
        self.font_size_menu = build_font_size_menu(window, cx);

        // When the window is client-decorated (e.g. GNOME on Wayland) the
        // compositor draws no title bar, so we have to provide one that
        // contains the window controls ourselves.
        let title_bar = window_title_bar::render_window_title_bar(
            &self.title_bar,
            i18n::t("settings"),
            window,
            cx,
        );

        div()
            .id("terry-settings")
            .key_context("SettingsWindow")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .children(title_bar)
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(self.render_tab_bar(cx)),
            )
            .child(
                div()
                    .id("terry-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .child(self.render_active_tab(window, cx)),
            )
    }
}

fn build_language_menu(
    current: UiLanguage,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    let variants = UiLanguage::VARIANTS;
    debug_assert_eq!(variants.len(), LANGUAGE_LABELS.len());

    ContextMenu::build(window, cx, move |mut menu, _, _| {
        for (variant, label) in variants
            .iter()
            .copied()
            .zip(LANGUAGE_LABELS.iter().copied())
        {
            let display = if variant == UiLanguage::System {
                i18n::t("language_system")
            } else {
                label.to_string()
            };
            let selected = variant == current;
            menu = menu.toggleable_entry(
                display,
                selected,
                IconPosition::Start,
                None,
                move |_window, cx| {
                    if variant == current {
                        return;
                    }
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file(fs, cx, move |content, _| {
                        content.ui_language = Some(variant);
                    });
                },
            );
        }
        menu
    })
}

fn build_font_size_menu(window: &mut Window, cx: &mut App) -> Entity<ContextMenu> {
    ContextMenu::build(window, cx, |mut menu, _, _| {
        for size in [11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0, 24.0] {
            let label = format!("{}", size);
            menu = menu.entry(label, None, move |_window, cx| {
                let fs = <dyn Fs>::global(cx);
                update_settings_file(fs, cx, move |content, _| {
                    content.theme.buffer_font_size = Some(settings::FontSize(size));
                    content.theme.ui_font_size = Some(settings::FontSize(size));
                    content.terminal.get_or_insert_default().font_size =
                        Some(settings::FontSize(size));
                });
            });
        }
        menu
    })
}

fn build_font_family_menu(window: &mut Window, cx: &mut App) -> Entity<ContextMenu> {
    ContextMenu::build(window, cx, |mut menu, _, _| {
        for family in [
            ".SystemUIFont",
            "Menlo",
            "Monaco",
            "Consolas",
            "Courier New",
            "Fira Code",
            "JetBrains Mono",
            "Hack",
        ] {
            let family_str = family.to_string();
            let label = if family == ".SystemUIFont" {
                i18n::t("font_system")
            } else {
                family.to_string()
            };
            menu = menu.entry(label, None, move |_window, cx| {
                let fs = <dyn Fs>::global(cx);
                let font_family = family_str.clone();
                update_settings_file(fs, cx, move |content, _| {
                    let name = settings::FontFamilyName(std::sync::Arc::from(font_family.as_str()));
                    content.theme.buffer_font_family = Some(name.clone());
                    content.theme.ui_font_family = Some(name.clone());
                    content.terminal.get_or_insert_default().font_family = Some(name);
                });
            });
        }
        menu
    })
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &zed_actions::OpenSettings, cx| {
        open_settings_window(cx);
    });

    cx.observe_new(|workspace: &mut workspace::Workspace, _, _| {
        workspace.register_action(|_, _: &zed_actions::OpenSettings, _window, cx| {
            open_settings_window(cx);
        });
    })
    .detach();

    apply_ui_preferences(cx);
    cx.observe_global::<SettingsStore>(|cx| apply_ui_preferences(cx))
        .detach();
}

fn open_settings_window(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<SettingsWindow>())
    {
        existing
            .update(cx, |_, window, _| {
                window.activate_window();
            })
            .log_err();
        return;
    }

    cx.defer(move |cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("settings")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(640.), px(560.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(480.),
                    height: px(420.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| SettingsWindow::new(window, cx)),
        )
        .log_err();
    });
}
