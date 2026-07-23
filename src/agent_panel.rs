use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window, actions, div,
};
use language_model::{IconOrSvg, LanguageModelRegistry};
use ui::{Icon, IconName, Label, LabelSize, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

actions!(agent_panel, [ToggleFocus]);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<AgentPanel>(window, cx);
        });
    })
    .detach();
}

pub struct AgentPanel {
    #[allow(dead_code)]
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    position: DockPosition,
}

impl AgentPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self {
            workspace: workspace.downgrade(),
            focus_handle: cx.focus_handle(),
            position: DockPosition::Right,
        }
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl gpui::EventEmitter<PanelEvent> for AgentPanel {}

impl Render for AgentPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let registry = LanguageModelRegistry::read_global(cx);
        let providers = registry.visible_providers();
        let authenticated_count = providers.iter().filter(|p| p.is_authenticated(cx)).count();

        let mut provider_list = div().flex_col().gap_1();
        for provider in providers {
            let is_auth = provider.is_authenticated(cx);
            let icon = match provider.icon() {
                IconOrSvg::Svg(path) => Icon::from_external_svg(path),
                IconOrSvg::Icon(name) => Icon::new(name),
            }
            .size(IconSize::Small)
            .color(if is_auth {
                Color::Default
            } else {
                Color::Muted
            });

            provider_list = provider_list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().colors().surface_background)
                    .child(icon)
                    .child(
                        Label::new(provider.name().0.clone())
                            .size(LabelSize::Small)
                            .color(if is_auth {
                                Color::Default
                            } else {
                                Color::Muted
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Label::new(if is_auth { "Ready" } else { "Needs Config" })
                            .size(LabelSize::XSmall)
                            .color(if is_auth {
                                Color::Success
                            } else {
                                Color::Warning
                            }),
                    ),
            );
        }

        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(Label::new("Agent").size(LabelSize::Default)),
            )
            .child(
                div()
                    .id("agent-panel-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(Label::new("LLM Providers").size(LabelSize::Small))
                                    .child(
                                        Label::new(format!("{authenticated_count} ready"))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    ),
                            )
                            .child(provider_list),
                    )
                    .child(
                        ui::Button::new("open-llm-settings", "Configure Providers…")
                            .style(ui::ButtonStyle::Outlined)
                            .size(ui::ButtonSize::Medium)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    Box::new(crate::llm_provider_settings::OpenLlmProviderSettings),
                                    cx,
                                );
                            }),
                    ),
            )
    }
}

impl Panel for AgentPanel {
    fn persistent_name() -> &'static str {
        "AgentPanel"
    }
    fn panel_key() -> &'static str {
        "agent_panel"
    }
    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }
    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right | DockPosition::Left)
    }
    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.position = position;
    }
    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        gpui::px(300.)
    }
    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Sparkle)
    }
    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Agent Panel")
    }
    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }
    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn activation_priority(&self) -> u32 {
        4
    }
}

pub struct AgentPanelButton {
    workspace: WeakEntity<Workspace>,
}

impl AgentPanelButton {
    pub fn new(workspace: WeakEntity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self { workspace }
    }
}

impl Render for AgentPanelButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ui::IconButton::new("agent-panel-button", ui::IconName::Sparkle)
            .icon_size(ui::IconSize::Small)
            .tooltip(|_, cx| ui::Tooltip::for_action("Toggle Agent Panel", &ToggleFocus, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(workspace) = this.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.toggle_panel_focus::<AgentPanel>(window, cx);
                    });
                }
            }))
    }
}

impl workspace::StatusItemView for AgentPanelButton {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn workspace::ItemHandle>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn hide_setting(&self, _: &App) -> Option<workspace::HideStatusItem> {
        None
    }
}
