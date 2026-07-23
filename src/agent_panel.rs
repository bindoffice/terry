use std::rc::Rc;
use std::sync::Arc;

use acp_thread::{
    AcpThread, AcpThreadEvent, AgentConnection, AgentModelId, AgentThreadEntry,
    AssistantMessageChunk, SelectedPermissionOutcome, ThreadStatus, ToolCallStatus,
};
use agent::{NativeAgent, NativeAgentConnection, Templates, ThreadStore};
use agent_client_protocol::schema::v1 as acp;
use editor::Editor;
use fs::Fs;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, WeakEntity, Window,
    actions, div, px,
};
use language_model::{ConfiguredModel, LanguageModelRegistry};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, ContextMenu, IconButton, IconName, Label, LabelSize,
    PopoverMenu, Tooltip, prelude::*,
};
use util::path_list::PathList;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

actions!(agent_panel, [ToggleFocus, NewThread, SendMessage]);

pub fn init(cx: &mut App) {
    ThreadStore::init_global(cx);
    prompt_store::init(cx);

    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<AgentPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &NewThread, window, cx| {
            if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                panel.update(cx, |panel, cx| panel.start_new_thread(window, cx));
            }
        });
        workspace.register_action(|workspace, _: &SendMessage, window, cx| {
            if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                panel.update(cx, |panel, cx| panel.send_message(window, cx));
            }
        });
    })
    .detach();
}

pub struct AgentPanel {
    #[allow(dead_code)]
    workspace: WeakEntity<Workspace>,
    project: Entity<project::Project>,
    fs: Arc<dyn Fs>,
    focus_handle: FocusHandle,
    position: DockPosition,
    connection: Option<Rc<dyn AgentConnection>>,
    thread: Option<Entity<AcpThread>>,
    prompt_editor: Entity<Editor>,
    model_label: SharedString,
    status_text: SharedString,
    error_text: Option<SharedString>,
    sending: bool,
    _thread_subscription: Option<Subscription>,
    _registry_subscription: Subscription,
}

impl AgentPanel {
    pub fn new(
        workspace: Entity<Workspace>,
        project: Entity<project::Project>,
        fs: Arc<dyn Fs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let prompt_editor = cx.new(|cx| {
            let mut editor = Editor::multi_line(window, cx);
            editor.set_placeholder_text("Message the agent…", window, cx);
            editor
        });

        let _registry_subscription =
            cx.subscribe(&LanguageModelRegistry::global(cx), |this, _, _, cx| {
                this.refresh_model_label(cx);
                cx.notify();
            });

        let mut panel = Self {
            workspace: workspace.downgrade(),
            project,
            fs,
            focus_handle,
            position: DockPosition::Right,
            connection: None,
            thread: None,
            prompt_editor,
            model_label: "No model".into(),
            status_text: "Starting…".into(),
            error_text: None,
            sending: false,
            _thread_subscription: None,
            _registry_subscription,
        };

        panel.refresh_model_label(cx);
        // Defer agent connect: panel is created inside a Workspace update, and
        // reading/updating the workspace (or starting sessions) here re-enters GPUI.
        let panel_handle = cx.weak_entity();
        cx.defer(move |cx| {
            if let Some(panel) = panel_handle.upgrade() {
                panel.update(cx, |panel, cx| {
                    panel.connect_and_start_thread(cx);
                });
            }
        });
        panel
    }

    fn connect_and_start_thread(&mut self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let fs = self.fs.clone();

        let thread_store = ThreadStore::global(cx);
        let agent = NativeAgent::new(thread_store, Templates::new(), fs, cx);
        let connection: Rc<dyn AgentConnection> = Rc::new(NativeAgentConnection(agent));
        self.connection = Some(connection.clone());
        self.status_text = "Creating thread…".into();

        let task = connection.new_session(project, PathList::new(&[] as &[&str]), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(thread) => {
                    this.attach_thread(thread, cx);
                    this.status_text = "Ready".into();
                    this.error_text = None;
                }
                Err(err) => {
                    this.status_text = "Failed to start".into();
                    this.error_text = Some(err.to_string().into());
                }
            })
            .ok();
        })
        .detach();
    }

    fn attach_thread(&mut self, thread: Entity<AcpThread>, cx: &mut Context<Self>) {
        self._thread_subscription = Some(cx.subscribe(&thread, |this, thread, event, cx| {
            match event {
                AcpThreadEvent::StatusChanged
                | AcpThreadEvent::NewEntry
                | AcpThreadEvent::EntryUpdated(_)
                | AcpThreadEvent::EntriesRemoved(_)
                | AcpThreadEvent::TitleUpdated
                | AcpThreadEvent::Stopped(_) => {
                    this.sending = matches!(thread.read(cx).status(), ThreadStatus::Generating);
                    this.status_text = if this.sending {
                        "Generating…".into()
                    } else {
                        "Ready".into()
                    };
                    cx.notify();
                }
                AcpThreadEvent::Error => {
                    this.sending = false;
                    this.status_text = "Error".into();
                    this.error_text = Some("Agent turn failed".into());
                    cx.notify();
                }
                AcpThreadEvent::ToolAuthorizationRequested(tool_call_id) => {
                    this.auto_allow_tool_if_possible(thread, tool_call_id.clone(), cx);
                }
                _ => cx.notify(),
            }
        }));
        self.thread = Some(thread);
        self.refresh_model_label(cx);
        cx.notify();
    }

    /// Minimal panel: auto-allow once when a tool asks, so a first chat turn can finish.
    fn auto_allow_tool_if_possible(
        &mut self,
        thread: Entity<AcpThread>,
        tool_call_id: acp::ToolCallId,
        cx: &mut Context<Self>,
    ) {
        thread.update(cx, |thread, cx| {
            let option = thread.entries().iter().find_map(|entry| {
                let AgentThreadEntry::ToolCall(call) = entry else {
                    return None;
                };
                if call.id != tool_call_id {
                    return None;
                }
                let ToolCallStatus::WaitingForConfirmation { options, .. } = &call.status else {
                    return None;
                };
                options
                    .first_option_of_kind(acp::PermissionOptionKind::AllowOnce)
                    .map(|option| (option.option_id.clone(), option.kind))
            });

            if let Some((option_id, kind)) = option {
                thread.authorize_tool_call(
                    tool_call_id,
                    SelectedPermissionOutcome::new(option_id, kind),
                    cx,
                );
            }
        });
    }

    fn start_new_thread(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            self.connect_and_start_thread(cx);
            return;
        };
        let project = self.project.clone();

        self.sending = false;
        self.error_text = None;
        self.status_text = "Creating thread…".into();
        self.thread = None;
        self._thread_subscription = None;

        let task = connection.new_session(project, PathList::new(&[] as &[&str]), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(thread) => {
                    this.attach_thread(thread, cx);
                    this.status_text = "Ready".into();
                }
                Err(err) => {
                    this.status_text = "Failed to start".into();
                    this.error_text = Some(err.to_string().into());
                }
            })
            .ok();
        })
        .detach();
    }

    fn refresh_model_label(&mut self, cx: &App) {
        let registry = LanguageModelRegistry::read_global(cx);
        self.model_label = registry
            .default_model()
            .map(|configured| {
                format!(
                    "{} / {}",
                    configured.provider.name().0,
                    configured.model.name().0
                )
                .into()
            })
            .unwrap_or_else(|| "No model".into());
    }

    fn select_model(&mut self, model: ConfiguredModel, cx: &mut Context<Self>) {
        let model_id = AgentModelId::new(format!(
            "{}/{}",
            model.model.provider_id().0,
            model.model.id().0
        ));

        LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
            registry.set_default_model(Some(model.clone()), cx);
        });
        self.refresh_model_label(cx);

        if let (Some(connection), Some(thread)) = (&self.connection, &self.thread) {
            let session_id = thread.read(cx).session_id().clone();
            if let Some(selector) = connection.model_selector(&session_id) {
                selector.select_model(model_id, cx).detach();
            }
        }

        cx.notify();
    }

    fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else {
            self.error_text = Some("Thread not ready".into());
            cx.notify();
            return;
        };

        if LanguageModelRegistry::read_global(cx).default_model().is_none() {
            self.error_text = Some("Configure a model first".into());
            cx.notify();
            return;
        }

        let text = self.prompt_editor.read(cx).text(cx);
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }

        self.prompt_editor.update(cx, |editor, cx| {
            editor.clear(window, cx);
        });
        self.sending = true;
        self.status_text = "Generating…".into();
        self.error_text = None;

        let future = thread.update(cx, |thread, cx| thread.send(vec![text.into()], cx));
        cx.spawn(async move |this, cx| {
            let result = future.await;
            this.update(cx, |this, cx| {
                this.sending = false;
                match result {
                    Ok(_) => {
                        this.status_text = "Ready".into();
                    }
                    Err(err) => {
                        this.status_text = "Error".into();
                        this.error_text = Some(err.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    fn cancel_generation(&mut self, cx: &mut Context<Self>) {
        if let Some(thread) = &self.thread {
            thread.update(cx, |thread, cx| {
                thread.cancel(cx).detach();
            });
        }
        self.sending = false;
        self.status_text = "Cancelled".into();
        cx.notify();
    }

    fn entry_body(entry: &AgentThreadEntry, cx: &App) -> (SharedString, String, Color) {
        match entry {
            AgentThreadEntry::UserMessage(message) => (
                "You".into(),
                message.content.to_markdown(cx).to_string(),
                Color::Accent,
            ),
            AgentThreadEntry::AssistantMessage(message) => {
                let body = message
                    .chunks
                    .iter()
                    .map(|chunk| match chunk {
                        AssistantMessageChunk::Message { block, .. } => {
                            block.to_markdown(cx).to_string()
                        }
                        AssistantMessageChunk::Thought { block, .. } => {
                            format!("thinking: {}", block.to_markdown(cx))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ("Agent".into(), body, Color::Default)
            }
            AgentThreadEntry::ToolCall(call) => {
                let name = call
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".into());
                let status = match &call.status {
                    ToolCallStatus::WaitingForConfirmation { .. } => " (waiting…)",
                    ToolCallStatus::InProgress | ToolCallStatus::Pending => " (running…)",
                    ToolCallStatus::Completed => " ✓",
                    ToolCallStatus::Failed => " ✗",
                    ToolCallStatus::Rejected => " rejected",
                    ToolCallStatus::Canceled => " canceled",
                };
                ("Tool".into(), format!("{name}{status}"), Color::Muted)
            }
            AgentThreadEntry::Elicitation(_) => {
                ("Input".into(), "Agent requested input".into(), Color::Warning)
            }
            AgentThreadEntry::CompletedPlan(_) => {
                ("Plan".into(), "Plan updated".into(), Color::Muted)
            }
            AgentThreadEntry::ContextCompaction(_) => {
                ("System".into(), "Context compacted".into(), Color::Muted)
            }
        }
    }

    fn render_entries(&self, cx: &App) -> Div {
        let mut list = div().flex_col().gap_3().p_3();

        let Some(thread) = &self.thread else {
            return list.child(
                Label::new("Starting agent session…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        };

        let entries = thread.read(cx).entries();
        if entries.is_empty() {
            return list
                .child(
                    Label::new("Send a message to start chatting.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new("Configure a provider if the model menu is empty.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
        }

        for entry in entries {
            let (role, body, color) = Self::entry_body(entry, cx);
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Label::new(role).size(LabelSize::XSmall).color(color))
                    .child(Label::new(body).size(LabelSize::Small).color(Color::Default)),
            );
        }

        list
    }

    fn render_model_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.weak_entity();
        let models: Vec<ConfiguredModel> = LanguageModelRegistry::read_global(cx)
            .available_models(cx)
            .filter_map(|model| {
                let provider =
                    LanguageModelRegistry::read_global(cx).provider(&model.provider_id())?;
                Some(ConfiguredModel { provider, model })
            })
            .collect();

        PopoverMenu::new("agent-model-menu")
            .trigger_with_tooltip(
                Button::new("model-selector", self.model_label.clone())
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Compact)
                    .label_size(LabelSize::XSmall),
                Tooltip::text("Select model"),
            )
            .menu(move |window, cx| {
                let models = models.clone();
                let panel = panel.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    if models.is_empty() {
                        return menu.header("No authenticated models");
                    }
                    for configured in models {
                        let label = format!(
                            "{} / {}",
                            configured.provider.name().0,
                            configured.model.name().0
                        );
                        let panel = panel.clone();
                        menu = menu.entry(label, None, move |_, cx| {
                            let model = configured.clone();
                            if let Some(panel) = panel.upgrade() {
                                panel.update(cx, |panel, cx| {
                                    panel.select_model(model, cx);
                                });
                            }
                        });
                    }
                    menu
                }))
            })
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
        let authenticated = LanguageModelRegistry::read_global(cx)
            .visible_providers()
            .iter()
            .any(|p| p.is_authenticated(cx));

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
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Label::new("Agent").size(LabelSize::Default))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("new-thread", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("New Thread"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_new_thread(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border_variant)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.render_model_menu(cx))
                    .child(div().flex_1())
                    .child(
                        Label::new(self.status_text.clone())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                div()
                    .id("agent-entries")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(self.render_entries(cx)),
            )
            .when_some(self.error_text.clone(), |this, error| {
                this.child(
                    div().px_3().py_1().child(
                        Label::new(error)
                            .size(LabelSize::XSmall)
                            .color(Color::Error),
                    ),
                )
            })
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .h(px(72.))
                            .w_full()
                            .border_1()
                            .border_color(cx.theme().colors().border_variant)
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .child(self.prompt_editor.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("open-llm-settings", "Providers")
                                    .style(ButtonStyle::Subtle)
                                    .size(ButtonSize::Compact)
                                    .label_size(LabelSize::XSmall)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(
                                                crate::llm_provider_settings::OpenLlmProviderSettings,
                                            ),
                                            cx,
                                        );
                                    }),
                            )
                            .child(div().flex_1())
                            .when(self.sending, |this| {
                                this.child(
                                    Button::new("cancel", "Cancel")
                                        .style(ButtonStyle::Subtle)
                                        .size(ButtonSize::Compact)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_generation(cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new(
                                    "send",
                                    if self.sending { "Sending…" } else { "Send" },
                                )
                                .style(ButtonStyle::Filled)
                                .size(ButtonSize::Compact)
                                .disabled(
                                    self.sending || !authenticated || self.thread.is_none(),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.send_message(window, cx);
                                })),
                            ),
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
        px(380.)
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
        IconButton::new("agent-panel-button", IconName::Sparkle)
            .icon_size(IconSize::Small)
            .tooltip(|_, cx| Tooltip::for_action("Toggle Agent Panel", &ToggleFocus, cx))
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
