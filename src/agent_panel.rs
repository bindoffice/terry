use language_model::LanguageModelRegistry;
use gpui::{
    actions, App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, WeakEntity, Window, div, ParentElement, Styled,
};
use ui::{prelude::*, IconName, Label, LabelSize};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    Workspace,
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

        let mut provider_list = div().flex_col().gap_2();
        for provider in providers {
            let is_auth = provider.is_authenticated(cx);
            provider_list = provider_list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Label::new(provider.name().0.clone()).size(LabelSize::Default))
                    .child(Label::new(if is_auth { " (Ready)" } else { " (Needs Config)" }).size(LabelSize::Small).color(if is_auth { Color::Success } else { Color::Muted }))
            );
        }

        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .p_4()
            .child(Label::new("Agent Panel").size(LabelSize::Large))
            .child(
                div()
                    .mt_4()
                    .child(Label::new("LLM Providers").size(LabelSize::Default))
                    .child(
                        div()
                            .mt_2()
                            .p_2()
                            .bg(cx.theme().colors().surface_background)
                            .rounded_md()
                            .child(provider_list)
                            .child(
                                div().mt_4().child(
                                    ui::Button::new("open-settings", "Configure Providers").on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(zed_actions::OpenSettingsPage { page: "AI".to_string(), target: None }), cx);
                                        })
                                )
                            )
                    )
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
    fn set_position(&mut self, position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
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
    fn set_active_pane_item(&mut self, _: Option<&dyn workspace::ItemHandle>, _: &mut Window, _: &mut Context<Self>) {}
    fn hide_setting(&self, _: &App) -> Option<workspace::HideStatusItem> { None }
}
