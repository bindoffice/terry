use fs::Fs;
use gpui::{
    Action, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    px,
};
use settings::{Settings, SettingsStore, UiLanguage, update_settings_file};
use strum::VariantArray;
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, DropdownMenu, IconPosition, Label,
    LabelSize, Switch, ToggleButtonGroup, ToggleButtonGroupStyle, ToggleButtonSimple, ToggleState,
    prelude::*,
};
use util::ResultExt;
use vim_mode_setting::VimModeSetting;

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
    Keyboard,
    Agent,
}

impl SettingsTab {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Appearance,
        Self::Editor,
        Self::Keyboard,
        Self::Agent,
    ];

    fn label(self) -> String {
        match self {
            Self::General => i18n::t("settings_tab_general"),
            Self::Appearance => i18n::t("appearance"),
            Self::Editor => i18n::t("settings_tab_editor"),
            Self::Keyboard => i18n::t("settings_tab_keyboard"),
            Self::Agent => i18n::t("agent"),
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or(0)
    }
}

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    active_tab: SettingsTab,
    language_menu: Entity<ContextMenu>,
    font_size_menu: Entity<ContextMenu>,
    font_family_menu: Entity<ContextMenu>,
    last_language: Option<UiLanguage>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let language = Self::current_language(cx);
        let language_menu = build_language_menu(language, window, cx);
        let font_size_menu = build_font_size_menu(window, cx);
        let font_family_menu = build_font_family_menu(window, cx);
        cx.observe_global::<SettingsStore>(|_, cx| cx.notify())
            .detach();
        Self {
            focus_handle,
            active_tab: SettingsTab::General,
            language_menu,
            font_size_menu,
            font_family_menu,
            last_language: Some(language),
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
                ToggleButtonSimple::new(SettingsTab::General.label(), cx.listener(|this, _, _, cx| {
                    this.select_tab(SettingsTab::General, cx);
                })),
                ToggleButtonSimple::new(
                    SettingsTab::Appearance.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Appearance, cx);
                    }),
                ),
                ToggleButtonSimple::new(SettingsTab::Editor.label(), cx.listener(|this, _, _, cx| {
                    this.select_tab(SettingsTab::Editor, cx);
                })),
                ToggleButtonSimple::new(
                    SettingsTab::Keyboard.label(),
                    cx.listener(|this, _, _, cx| {
                        this.select_tab(SettingsTab::Keyboard, cx);
                    }),
                ),
                ToggleButtonSimple::new(SettingsTab::Agent.label(), cx.listener(|this, _, _, cx| {
                    this.select_tab(SettingsTab::Agent, cx);
                })),
            ],
        )
        .selected_index(active.index())
        .style(ToggleButtonGroupStyle::Outlined)
        .label_size(LabelSize::Small)
        .auto_width()
        .into_any_element()
    }

    fn render_general_tab(&self, cx: &App) -> AnyElement {
        let language_label = Self::current_language_label(cx);
        settings_section(
            i18n::t("ui_language"),
            i18n::t("ui_language_description"),
            DropdownMenu::new("ui-language", language_label, self.language_menu.clone())
                .style(ui::DropdownStyle::Outlined)
                .trigger_size(ButtonSize::Medium)
                .into_any_element(),
        )
    }

    fn render_appearance_tab(&self, cx: &App) -> AnyElement {
        let current_font_size = theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx);
        let current_font_family_raw = theme_settings::ThemeSettings::get_global(cx)
            .buffer_font
            .family
            .clone();
        let current_font_family = if current_font_family_raw.as_ref() == ".SystemUIFont" {
            SharedString::from(i18n::t("font_system"))
        } else {
            current_font_family_raw
        };

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
                        Button::new("select-theme", i18n::t("select_theme"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    zed_actions::theme_selector::Toggle::default().boxed_clone(),
                                    cx,
                                );
                            }),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_editor_tab(&self, cx: &App) -> AnyElement {
        let vim_enabled = VimModeSetting::is_enabled(cx);
        let toggle_state = if vim_enabled {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        };
        settings_section(
            i18n::t("vim_mode"),
            i18n::t("vim_mode_description"),
            Switch::new("vim-mode", toggle_state)
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
        )
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

    fn render_active_tab(&self, cx: &App) -> AnyElement {
        match self.active_tab {
            SettingsTab::General => self.render_general_tab(cx),
            SettingsTab::Appearance => self.render_appearance_tab(cx),
            SettingsTab::Editor => self.render_editor_tab(cx),
            SettingsTab::Keyboard => self.render_keyboard_tab(cx),
            SettingsTab::Agent => self.render_agent_tab(cx),
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

        div()
            .id("terry-settings")
            .key_context("SettingsWindow")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
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
                    .child(self.render_active_tab(cx)),
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
