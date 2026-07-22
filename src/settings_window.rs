use fs::Fs;
use gpui::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    px,
};
use settings::{Settings, SettingsStore, UiLanguage, update_settings_file};
use strum::VariantArray;
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, DropdownMenu, IconPosition, Label,
    LabelSize, prelude::*,
};
use util::ResultExt;

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

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    language_menu: Entity<ContextMenu>,
    last_language: Option<UiLanguage>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let language = Self::current_language(cx);
        let language_menu = build_language_menu(language, window, cx);
        cx.observe_global::<SettingsStore>(|_, cx| cx.notify())
            .detach();
        Self {
            focus_handle,
            language_menu,
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

        let language_label = Self::current_language_label(cx);

        div()
            .id("ink-settings")
            .key_context("SettingsWindow")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(
                div()
                    .flex_1()
                    .p_4()
                    .gap_4()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Label::new(i18n::t("ui_language")))
                            .child(
                                Label::new(i18n::t("ui_language_description"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                DropdownMenu::new(
                                    "ui-language",
                                    language_label,
                                    self.language_menu.clone(),
                                )
                                .style(ui::DropdownStyle::Outlined)
                                .trigger_size(ButtonSize::Medium),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Label::new(i18n::t("appearance")))
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
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(Label::new(i18n::t("custom_shortcuts")))
                            .child(
                                Button::new("open-keymaps", i18n::t("custom_shortcuts"))
                                    .style(ButtonStyle::Outlined)
                                    .size(ButtonSize::Medium)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            zed_actions::OpenKeymapFile.boxed_clone(),
                                            cx,
                                        );
                                    }),
                            ),
                    ),
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
        for (variant, label) in variants.iter().copied().zip(LANGUAGE_LABELS.iter().copied()) {
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
                    title: Some(format!("Ink — {}", i18n::t("settings")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(480.), px(360.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(360.),
                    height: px(240.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| SettingsWindow::new(window, cx)),
        )
        .log_err();
    });
}
