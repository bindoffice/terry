use std::sync::Arc;

use anthropic::AnthropicModelMode;
use client::Client;
use editor::Editor;
use fs::Fs;
use futures::AsyncReadExt as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Render, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px,
};
use http_client::{AsyncBody, CustomHeaders, HttpClient, Method, Request as HttpRequest};
use language_model::{
    Event as LanguageModelRegistryEvent, LanguageModelProviderId, LanguageModelRegistry,
};
use language_models::AllLanguageModelSettings;
use serde::Deserialize;
use settings::{
    AnthropicCompatibleAvailableModel, AnthropicCompatibleModelCapabilities,
    AnthropicCompatibleSettingsContent, ModelMode, OpenAiCompatibleAvailableModel,
    OpenAiCompatibleModelCapabilities, OpenAiCompatibleSettingsContent, Settings, SettingsStore,
    update_settings_file, update_settings_file_with_completion,
};
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName, IconSize, Label, LabelSize,
    Tooltip, prelude::*,
};
use util::ResultExt;

actions!(llm_provider_settings, [OpenLlmProviderSettings]);

const OPENAI_DEFAULT_URL: &str = "https://api.openai.com/v1";
const CLAUDE_DEFAULT_URL: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_MAX_TOKENS: u64 = 128_000;
const DEFAULT_OPENAI_MAX_OUTPUT_TOKENS: u64 = 16_384;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    OpenAi,
    Claude,
}

impl ProviderKind {
    fn label(self) -> SharedString {
        match self {
            Self::OpenAi => SharedString::from("OpenAI"),
            Self::Claude => SharedString::from("Claude"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::OpenAi => IconName::AiOpenAi,
            Self::Claude => IconName::AiClaude,
        }
    }

    fn default_url(self) -> &'static str {
        match self {
            Self::OpenAi => OPENAI_DEFAULT_URL,
            Self::Claude => CLAUDE_DEFAULT_URL,
        }
    }
}

#[derive(Clone)]
struct ListedProvider {
    kind: ProviderKind,
    name: Arc<str>,
    api_url: String,
    model_count: usize,
}

enum Page {
    List,
    Add,
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenLlmProviderSettings, cx| open_llm_provider_settings(cx));

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
                    title: Some(format!("Terry — {}", i18n::t("llm_providers")).into()),
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
    page: Page,
    add_kind: ProviderKind,
    name_editor: Entity<Editor>,
    base_url_editor: Entity<Editor>,
    api_key_editor: Entity<Editor>,
    status: Option<SharedString>,
    error: Option<SharedString>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl LlmProviderSettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let registry = LanguageModelRegistry::global(cx);

        let subscriptions = vec![
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.subscribe(&registry, |_, _, event, cx| {
                if matches!(
                    event,
                    LanguageModelRegistryEvent::ProviderStateChanged(_)
                        | LanguageModelRegistryEvent::ProvidersChanged
                        | LanguageModelRegistryEvent::AddedProvider(_)
                        | LanguageModelRegistryEvent::RemovedProvider(_)
                ) {
                    cx.notify();
                }
            }),
            cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
        ];

        let name_editor = new_single_line_editor(i18n::t("provider_name_placeholder").as_str(), "", window, cx);
        let base_url_editor =
            new_single_line_editor(OPENAI_DEFAULT_URL, OPENAI_DEFAULT_URL, window, cx);
        let api_key_editor =
            new_single_line_editor(i18n::t("paste_api_key").as_str(), "", window, cx);

        Self {
            focus_handle,
            page: Page::List,
            add_kind: ProviderKind::OpenAi,
            name_editor,
            base_url_editor,
            api_key_editor,
            status: None,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    fn listed_providers(&self, cx: &App) -> Vec<ListedProvider> {
        let settings = AllLanguageModelSettings::get_global(cx);
        let mut providers = Vec::new();

        for (name, config) in &settings.openai_compatible {
            providers.push(ListedProvider {
                kind: ProviderKind::OpenAi,
                name: name.clone(),
                api_url: config.api_url.clone(),
                model_count: config.available_models.len(),
            });
        }
        for (name, config) in &settings.anthropic_compatible {
            if settings.openai_compatible.contains_key(name) {
                continue;
            }
            providers.push(ListedProvider {
                kind: ProviderKind::Claude,
                name: name.clone(),
                api_url: config.api_url.clone(),
                model_count: config.available_models.len(),
            });
        }

        providers.sort_by(|a, b| a.name.cmp(&b.name));
        providers
    }

    fn show_add_form(&mut self, kind: ProviderKind, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Page::Add;
        self.add_kind = kind;
        self.error = None;
        self.status = None;
        self.name_editor.update(cx, |editor, cx| {
            editor.set_text("", window, cx);
        });
        self.base_url_editor.update(cx, |editor, cx| {
            editor.set_placeholder_text(kind.default_url(), window, cx);
            editor.set_text(kind.default_url(), window, cx);
        });
        self.api_key_editor.update(cx, |editor, cx| {
            editor.set_text("", window, cx);
        });
        cx.notify();
    }

    fn cancel_add(&mut self, cx: &mut Context<Self>) {
        self.page = Page::List;
        self.error = None;
        self.status = None;
        self.busy = false;
        cx.notify();
    }

    fn save_new_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }

        let kind = self.add_kind;
        let name = self.name_editor.read(cx).text(cx).trim().to_string();
        let api_url = self.base_url_editor.read(cx).text(cx).trim().to_string();
        let api_key = self.api_key_editor.read(cx).text(cx).trim().to_string();

        if name.is_empty() {
            self.error = Some(i18n::t("provider_name_required").into());
            cx.notify();
            return;
        }
        if api_url.is_empty() {
            self.error = Some(i18n::t("base_url_required").into());
            cx.notify();
            return;
        }
        if api_key.is_empty() {
            self.error = Some(i18n::t("api_key_required").into());
            cx.notify();
            return;
        }

        let registry = LanguageModelRegistry::read_global(cx);
        let provider_id = LanguageModelProviderId(name.clone().into());
        if registry.provider(&provider_id).is_some() {
            self.error = Some(i18n::t("provider_name_taken").into());
            cx.notify();
            return;
        }

        self.busy = true;
        self.error = None;
        self.status = Some(i18n::t("fetching_models").into());
        cx.notify();

        let http = cx.http_client();
        let fs = <dyn Fs>::global(cx);

        cx.spawn_in(window, async move |this, cx| {
            let fetch_result = match kind {
                ProviderKind::OpenAi => fetch_openai_models(http.as_ref(), &api_url, &api_key)
                    .await
                    .map(FetchedModels::OpenAi),
                ProviderKind::Claude => fetch_claude_models(http.as_ref(), &api_url, &api_key)
                    .await
                    .map(FetchedModels::Claude),
            };

            let models = match fetch_result {
                Ok(models) if models.is_empty() => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = None;
                        this.error = Some(i18n::t("no_models_found").into());
                        cx.notify();
                    })?;
                    return anyhow::Ok(());
                }
                Ok(models) => models,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = None;
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    })?;
                    return anyhow::Ok(());
                }
            };

            let model_count = models.len();
            let provider_name = name.clone();
            let settings_update = cx.update(|_window, cx| {
                update_settings_file_with_completion(fs, cx, move |settings, _cx| {
                    let language_models = settings.language_models.get_or_insert_default();
                    match models {
                        FetchedModels::OpenAi(available_models) => {
                            language_models
                                .openai_compatible
                                .get_or_insert_default()
                                .insert(
                                    Arc::from(provider_name.as_str()),
                                    OpenAiCompatibleSettingsContent {
                                        api_url: api_url.clone(),
                                        available_models,
                                        custom_headers: None,
                                    },
                                );
                        }
                        FetchedModels::Claude(available_models) => {
                            language_models
                                .anthropic_compatible
                                .get_or_insert_default()
                                .insert(
                                    Arc::from(provider_name.as_str()),
                                    AnthropicCompatibleSettingsContent {
                                        api_url: api_url.clone(),
                                        available_models,
                                        custom_headers: None,
                                    },
                                );
                        }
                    }
                })
            })?;

            settings_update
                .await
                .map_err(|_| anyhow::anyhow!("Settings update was canceled"))??;

            let provider_id = LanguageModelProviderId(name.into());
            let set_key = cx.update(|_window, cx| {
                let provider = LanguageModelRegistry::read_global(cx)
                    .provider(&provider_id)
                    .ok_or_else(|| anyhow::anyhow!("Provider was not registered"))?;
                anyhow::Ok(provider.set_api_key(Some(api_key), cx))
            })??;
            set_key.await?;

            this.update(cx, |this, cx| {
                this.busy = false;
                this.status = None;
                this.error = None;
                this.page = Page::List;
                this.status = Some(
                    i18n::t("provider_added_with_models")
                        .replace("{count}", &model_count.to_string())
                        .into(),
                );
                cx.notify();
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn refresh_models(
        &mut self,
        provider: ListedProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }

        self.busy = true;
        self.error = None;
        self.status = Some(i18n::t("fetching_models").into());
        cx.notify();

        let http = cx.http_client();
        let fs = <dyn Fs>::global(cx);
        let typed_key = self.api_key_editor.read(cx).text(cx).trim().to_string();
        let kind = provider.kind;
        let name = provider.name.clone();
        let api_url = provider.api_url.clone();

        cx.spawn_in(window, async move |this, cx| {
            let api_key = match resolve_api_key(&api_url, &typed_key, cx).await {
                Ok(key) => key,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = None;
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    })?;
                    return anyhow::Ok(());
                }
            };

            let fetch_result = match kind {
                ProviderKind::OpenAi => fetch_openai_models(http.as_ref(), &api_url, &api_key)
                    .await
                    .map(FetchedModels::OpenAi),
                ProviderKind::Claude => fetch_claude_models(http.as_ref(), &api_url, &api_key)
                    .await
                    .map(FetchedModels::Claude),
            };

            let models = match fetch_result {
                Ok(models) if models.is_empty() => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = None;
                        this.error = Some(i18n::t("no_models_found").into());
                        cx.notify();
                    })?;
                    return anyhow::Ok(());
                }
                Ok(models) => models,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = None;
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    })?;
                    return anyhow::Ok(());
                }
            };

            let model_count = models.len();
            let settings_update = cx.update(|_window, cx| {
                update_settings_file_with_completion(fs, cx, move |settings, _cx| {
                    let language_models = settings.language_models.get_or_insert_default();
                    match models {
                        FetchedModels::OpenAi(available_models) => {
                            if let Some(entry) = language_models
                                .openai_compatible
                                .get_or_insert_default()
                                .get_mut(name.as_ref())
                            {
                                entry.available_models = available_models;
                            }
                        }
                        FetchedModels::Claude(available_models) => {
                            if let Some(entry) = language_models
                                .anthropic_compatible
                                .get_or_insert_default()
                                .get_mut(name.as_ref())
                            {
                                entry.available_models = available_models;
                            }
                        }
                    }
                })
            })?;

            settings_update
                .await
                .map_err(|_| anyhow::anyhow!("Settings update was canceled"))??;

            this.update(cx, |this, cx| {
                this.busy = false;
                this.error = None;
                this.status = Some(
                    i18n::t("models_refreshed")
                        .replace("{count}", &model_count.to_string())
                        .into(),
                );
                cx.notify();
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn remove_provider(&mut self, provider: ListedProvider, cx: &mut Context<Self>) {
        let fs = <dyn Fs>::global(cx);
        let name = provider.name.clone();
        update_settings_file(fs, cx, move |settings, _| {
            let Some(language_models) = settings.language_models.as_mut() else {
                return;
            };
            if let Some(providers) = language_models.openai_compatible.as_mut() {
                providers.remove(name.as_ref());
            }
            if let Some(providers) = language_models.anthropic_compatible.as_mut() {
                providers.remove(name.as_ref());
            }
        });
        self.status = Some(i18n::t("provider_removed").into());
        self.error = None;
        cx.notify();
    }
}

impl Focusable for LlmProviderSettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LlmProviderSettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terry-llm-provider-settings")
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
                            .gap_4()
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
                            .children(self.status.clone().map(|status| {
                                Label::new(status).size(LabelSize::Small).color(Color::Success)
                            }))
                            .children(self.error.clone().map(|error| {
                                Label::new(error).size(LabelSize::Small).color(Color::Error)
                            }))
                            .child(match self.page {
                                Page::List => self.render_list(window, cx),
                                Page::Add => self.render_add_form(cx),
                            }),
                    ),
            )
    }
}

impl LlmProviderSettingsWindow {
    fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let providers = self.listed_providers(cx);
        let busy = self.busy;

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new("add-openai-provider", i18n::t("add_openai_provider"))
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Medium)
                            .start_icon(Icon::new(IconName::AiOpenAi).size(IconSize::Small))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_add_form(ProviderKind::OpenAi, window, cx);
                            })),
                    )
                    .child(
                        Button::new("add-claude-provider", i18n::t("add_claude_provider"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .start_icon(Icon::new(IconName::AiClaude).size(IconSize::Small))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_add_form(ProviderKind::Claude, window, cx);
                            })),
                    ),
            )
            .when(providers.is_empty(), |this| {
                this.child(
                    Label::new(i18n::t("no_providers_yet"))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .children(providers.into_iter().map(|provider| {
                self.render_provider_card(provider, busy, window, cx)
            }))
            .into_any_element()
    }

    fn render_provider_card(
        &mut self,
        provider: ListedProvider,
        busy: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = provider.name.clone();
        let kind = provider.kind;
        let api_url = provider.api_url.clone();
        let model_count = provider.model_count;

        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .min_w_0()
                            .child(Icon::new(kind.icon()).color(Color::Muted))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(Label::new(name.to_string()))
                                    .child(
                                        Label::new(kind.label())
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                IconButton::new(
                                    SharedString::from(format!("refresh-{}", name)),
                                    IconName::RotateCw,
                                )
                                .tooltip(Tooltip::text(i18n::t("refresh_models")))
                                .disabled(busy)
                                .on_click({
                                    let provider = provider.clone();
                                    cx.listener(move |this, _, window, cx| {
                                        this.refresh_models(provider.clone(), window, cx);
                                    })
                                }),
                            )
                            .child(
                                IconButton::new(
                                    SharedString::from(format!("remove-{}", name)),
                                    IconName::Trash,
                                )
                                .tooltip(Tooltip::text(i18n::t("remove_provider")))
                                .disabled(busy)
                                .on_click({
                                    let provider = provider.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove_provider(provider.clone(), cx);
                                    })
                                }),
                            ),
                    ),
            )
            .child(
                Label::new(api_url)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new(
                    i18n::t("model_count").replace("{count}", &model_count.to_string()),
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .into_any_element()
    }

    fn render_add_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.add_kind;
        let busy = self.busy;

        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_1p5()
                    .child(Icon::new(kind.icon()).color(Color::Muted))
                    .child(Label::new(
                        i18n::t("add_provider_title").replace("{kind}", kind.label().as_ref()),
                    )),
            )
            .child(div().h_px().w_full().bg(cx.theme().colors().border_variant))
            .child(self.render_field(
                SharedString::from(i18n::t("provider_name")),
                self.name_editor.clone(),
                cx,
            ))
            .child(self.render_field(
                SharedString::from(i18n::t("base_url")),
                self.base_url_editor.clone(),
                cx,
            ))
            .child(self.render_field(
                SharedString::from(i18n::t("api_key")),
                self.api_key_editor.clone(),
                cx,
            ))
            .child(
                Label::new(i18n::t("models_fetched_on_save"))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-add-provider", i18n::t("cancel"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_add(cx);
                            })),
                    )
                    .child(
                        Button::new("save-add-provider", i18n::t("save_and_fetch_models"))
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Medium)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_new_provider(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_field(
        &self,
        label: SharedString,
        editor: Entity<Editor>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
            .child(
                div()
                    .w_full()
                    .h_5()
                    .px_1()
                    .rounded_md()
                    .bg(cx.theme().colors().editor_background)
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .child(editor),
            )
    }
}

fn new_single_line_editor(
    placeholder: &str,
    text: &str,
    window: &mut Window,
    cx: &mut Context<LlmProviderSettingsWindow>,
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

enum FetchedModels {
    OpenAi(Vec<OpenAiCompatibleAvailableModel>),
    Claude(Vec<AnthropicCompatibleAvailableModel>),
}

impl FetchedModels {
    fn is_empty(&self) -> bool {
        match self {
            Self::OpenAi(models) => models.is_empty(),
            Self::Claude(models) => models.is_empty(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::OpenAi(models) => models.len(),
            Self::Claude(models) => models.len(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiListModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

async fn fetch_openai_models(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
) -> anyhow::Result<Vec<OpenAiCompatibleAvailableModel>> {
    let api_url = api_url.trim_end_matches('/');
    let uri = format!("{api_url}/models");
    let request = HttpRequest::builder()
        .method(Method::GET)
        .uri(&uri)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .body(AsyncBody::default())?;

    let mut response = client.send(request).await?;
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    anyhow::ensure!(
        response.status().is_success(),
        "failed to list OpenAI models: {} {}",
        response.status(),
        body,
    );

    let parsed: OpenAiListModelsResponse = serde_json::from_str(&body)?;
    Ok(parsed
        .data
        .into_iter()
        .filter(|entry| !entry.id.trim().is_empty())
        .map(|entry| OpenAiCompatibleAvailableModel {
            name: entry.id,
            display_name: None,
            max_tokens: DEFAULT_OPENAI_MAX_TOKENS,
            max_output_tokens: Some(DEFAULT_OPENAI_MAX_OUTPUT_TOKENS),
            max_completion_tokens: Some(DEFAULT_OPENAI_MAX_OUTPUT_TOKENS),
            reasoning_effort: None,
            capabilities: OpenAiCompatibleModelCapabilities::default(),
        })
        .collect())
}

async fn fetch_claude_models(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
) -> anyhow::Result<Vec<AnthropicCompatibleAvailableModel>> {
    let models = anthropic::list_models(
        client,
        api_url.trim_end_matches('/'),
        api_key,
        &CustomHeaders::default(),
    )
    .await?;

    Ok(models
        .into_iter()
        .map(|model| AnthropicCompatibleAvailableModel {
            name: model.id,
            display_name: Some(model.display_name),
            max_tokens: model.max_input_tokens,
            tool_override: model.tool_override,
            max_output_tokens: Some(model.max_output_tokens),
            default_temperature: Some(model.default_temperature),
            extra_beta_headers: model.extra_beta_headers,
            mode: Some(match model.mode {
                AnthropicModelMode::Default => ModelMode::Default,
                AnthropicModelMode::Thinking { budget_tokens } => {
                    ModelMode::Thinking { budget_tokens }
                }
                AnthropicModelMode::AdaptiveThinking => ModelMode::Adaptive,
            }),
            capabilities: AnthropicCompatibleModelCapabilities {
                tools: true,
                images: model.supports_images,
                prompt_caching: false,
            },
        })
        .collect())
}

async fn resolve_api_key(
    api_url: &str,
    typed_key: &str,
    cx: &mut gpui::AsyncWindowContext,
) -> anyhow::Result<String> {
    if !typed_key.is_empty() {
        return Ok(typed_key.to_string());
    }

    let credentials_provider =
        cx.update(|_window, cx| Client::global(cx).credentials_provider())?;
    let credentials = credentials_provider
        .read_credentials(api_url, cx)
        .await?
        .ok_or_else(|| anyhow::anyhow!(i18n::t("api_key_required_for_refresh")))?;
    let key = String::from_utf8(credentials.1)
        .map_err(|_| anyhow::anyhow!(i18n::t("api_key_required_for_refresh")))?;
    if key.trim().is_empty() {
        anyhow::bail!(i18n::t("api_key_required_for_refresh"));
    }
    Ok(key)
}
