use std::collections::HashMap;
use std::sync::Arc;

use editor::Editor;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Render, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px,
};
use language_model::{
    CreateProviderSettingsView, IconOrSvg, InlineDescription, LanguageModelProvider,
    LanguageModelProviderId, LanguageModelRegistry, ProviderSettingsView,
};
use theme::ActiveTheme;
use ui::{
    Button, ButtonLink, ButtonSize, ButtonStyle, Color, ConfiguredApiCard, Icon, Label, LabelSize,
    prelude::*,
};
use util::ResultExt;

actions!(llm_provider_settings, [OpenLlmProviderSettings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenLlmProviderSettings, cx| open_llm_provider_settings(cx));

    // Route Zed-style "open the AI settings page" requests to our window so
    // existing call sites (e.g. the agent panel) keep working.
    cx.on_action(|action: &zed_actions::OpenSettingsPage, cx| {
        if action.page.eq_ignore_ascii_case("AI")
            || action.page.eq_ignore_ascii_case("LLM Providers")
        {
            open_llm_provider_settings(cx);
        }
    });
}

fn open_llm_provider_settings(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<LlmProviderSettingsWindow>())
    {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .log_err();
        return;
    }

    cx.defer(|cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Ink — {}", i18n::t("llm_providers")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(640.), px(720.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(480.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| LlmProviderSettingsWindow::new(window, cx)),
        )
        .log_err();
    });
}

pub struct LlmProviderSettingsWindow {
    focus_handle: FocusHandle,
    _registry_subscription: Subscription,
    /// Cached configuration views for `Inline`/`SubPage` providers, keyed by
    /// provider id so they survive re-renders.
    provider_views: HashMap<LanguageModelProviderId, AnyView>,
    /// Single-line editors used to type API keys for `ApiKey` providers.
    api_key_editors: HashMap<LanguageModelProviderId, Entity<Editor>>,
}

impl LlmProviderSettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let registry = LanguageModelRegistry::global(cx);
        let _registry_subscription = cx.observe(&registry, |_, _, cx| cx.notify());
        let _ = window;
        Self {
            focus_handle,
            _registry_subscription,
            provider_views: HashMap::default(),
            api_key_editors: HashMap::default(),
        }
    }

    fn api_key_editor(
        &mut self,
        provider_id: &LanguageModelProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        if let Some(editor) = self.api_key_editors.get(provider_id) {
            return editor.clone();
        }
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Paste your API key…", window, cx);
            editor
        });
        self.api_key_editors
            .insert(provider_id.clone(), editor.clone());
        editor
    }

    fn provider_config_view(
        &mut self,
        provider_id: &LanguageModelProviderId,
        create_view: &CreateProviderSettingsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyView {
        if let Some(view) = self.provider_views.get(provider_id) {
            return view.clone();
        }
        let view = create_view(window, cx);
        self.provider_views
            .insert(provider_id.clone(), view.clone());
        view
    }
}

impl Focusable for LlmProviderSettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LlmProviderSettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let providers = LanguageModelRegistry::read_global(cx).visible_providers();

        let mut sections = Vec::new();
        for provider in providers {
            sections.push(self.render_provider_section(provider, window, cx));
        }

        div()
            .id("ink-llm-provider-settings")
            .key_context("LlmProviderSettings")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(
                div()
                    .flex_1()
                    .id("llm-provider-scroll")
                    .overflow_y_scroll()
                    .px_6()
                    .pb_10()
                    .child(
                        v_flex()
                            .w_full()
                            .child(
                                v_flex()
                                    .pt_4()
                                    .pb_2()
                                    .gap_0p5()
                                    .child(
                                        Label::new(i18n::t("llm_providers")).size(LabelSize::Large),
                                    )
                                    .child(
                                        Label::new(i18n::t("llm_providers_description"))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                            )
                            .children(sections),
                    ),
            )
    }
}

impl LlmProviderSettingsWindow {
    fn render_provider_section(
        &mut self,
        provider: Arc<dyn LanguageModelProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let provider_id = provider.id();
        let provider_name = provider.name().0.clone();

        let body = match provider.settings_view(cx) {
            Some(ProviderSettingsView::ApiKey(config)) => {
                self.render_api_key_body(&provider, config, window, cx)
            }
            Some(ProviderSettingsView::Inline(settings)) => {
                let view =
                    self.provider_config_view(&provider_id, &settings.create_view, window, cx);
                render_inline_body(
                    provider_name.clone(),
                    settings.title,
                    settings.description,
                    view,
                )
            }
            Some(ProviderSettingsView::SubPage(settings)) => {
                let view =
                    self.provider_config_view(&provider_id, &settings.create_view, window, cx);
                render_inline_body(provider_name.clone(), None, settings.description, view)
            }
            None => div().into_any_element(),
        };

        v_flex()
            .min_w_0()
            .w_full()
            .pt_4()
            .gap_1p5()
            .child(render_provider_header(provider_name, provider.icon(), cx))
            .child(body)
            .into_any_element()
    }

    fn render_api_key_body(
        &mut self,
        provider: &Arc<dyn LanguageModelProvider>,
        config: language_model::ApiKeyConfiguration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let provider_id = provider.id();
        let provider_name = provider.name().0.clone();
        let has_key = config.has_key;
        let is_from_env_var = config.is_from_env_var;
        let env_var_name = config.env_var_name;
        let api_key_url = config.api_key_url;

        if has_key {
            let configured_label = if is_from_env_var {
                "API key set in environment variable"
            } else {
                "API key configured"
            };
            let card = ConfiguredApiCard::new(
                SharedString::from(format!("reset-api-key-{}", provider_id.0)),
                configured_label,
            )
            .button_label("Reset Key")
            .disabled(is_from_env_var)
            .when(is_from_env_var, |this| {
                this.tooltip_label(format!(
                    "To reset your API key, unset the {env_var_name} environment variable."
                ))
            })
            .on_click({
                let provider = provider.clone();
                move |_, _, cx| {
                    provider.set_api_key(None, cx).detach_and_log_err(cx);
                }
            })
            .into_any_element();

            return v_flex().gap_2().child(card).into_any_element();
        }

        let editor = self.api_key_editor(&provider_id, window, cx);

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .flex_wrap()
                    .gap_0p5()
                    .child(Label::new("Visit the").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        ButtonLink::new(
                            SharedString::from(format!("{provider_name} dashboard")),
                            api_key_url.to_string(),
                        )
                        .no_icon(true)
                        .label_size(LabelSize::Small)
                        .label_color(Color::Muted),
                    )
                    .child(
                        Label::new("to generate an API key.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(
                Label::new(format!(
                    "Or set the {env_var_name} environment variable and restart for it to take effect."
                ))
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .h_5()
                            .px_1()
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .on_action({
                                let provider = provider.clone();
                                let editor = editor.clone();
                                move |_: &menu::Confirm, _window, cx| {
                                    let key = editor.read(cx).text(cx);
                                    if !key.trim().is_empty() {
                                        provider
                                            .set_api_key(Some(key), cx)
                                            .detach_and_log_err(cx);
                                    }
                                }
                            })
                            .child(editor.clone()),
                    )
                    .child(
                        Button::new(
                            SharedString::from(format!("save-api-key-{}", provider_id.0)),
                            "Save",
                        )
                        .style(ButtonStyle::Filled)
                        .size(ButtonSize::Medium)
                        .on_click({
                            let provider = provider.clone();
                            let editor = editor.clone();
                            move |_, _window, cx| {
                                let key = editor.read(cx).text(cx);
                                if !key.trim().is_empty() {
                                    provider
                                        .set_api_key(Some(key), cx)
                                        .detach_and_log_err(cx);
                                }
                            }
                        }),
                    ),
            )
            .into_any_element()
    }
}

fn render_provider_header(
    provider_name: SharedString,
    icon: IconOrSvg,
    cx: &mut Context<LlmProviderSettingsWindow>,
) -> impl IntoElement {
    let icon = match icon {
        IconOrSvg::Svg(path) => Icon::from_external_svg(path),
        IconOrSvg::Icon(name) => Icon::new(name),
    }
    .color(Color::Muted);

    v_flex()
        .w_full()
        .gap_1p5()
        .child(
            h_flex()
                .gap_1p5()
                .child(icon)
                .child(Label::new(provider_name).size(LabelSize::Default)),
        )
        .child(div().h_px().w_full().bg(cx.theme().colors().border_variant))
}

fn render_inline_body(
    _provider_name: SharedString,
    title: Option<SharedString>,
    description: Option<InlineDescription>,
    view: AnyView,
) -> AnyElement {
    v_flex()
        .pt_1()
        .w_full()
        .min_w_0()
        .gap_2()
        .when_some(title, |this, title| this.child(Label::new(title)))
        .when_some(description, |this, description| {
            this.child(render_inline_description(description))
        })
        .child(view)
        .into_any_element()
}

fn render_inline_description(description: InlineDescription) -> AnyElement {
    match description {
        InlineDescription::ApiKeyUrl(url) => h_flex()
            .gap_0p5()
            .child(
                Label::new("To find an API key, visit the")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                ButtonLink::new(SharedString::from("provider dashboard."), url.to_string())
                    .label_size(LabelSize::Small),
            )
            .into_any_element(),
        InlineDescription::Text(text) => Label::new(text)
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element(),
    }
}
