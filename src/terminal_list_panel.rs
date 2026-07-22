use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, WeakEntity, Window, div, px,
};
use terminal_view::TerminalView;
use ui::{IconButton, IconName, Label, LabelSize, Tooltip, prelude::*};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::ToggleLeftDock;

pub struct TerminalListPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    position: DockPosition,
    _workspace_subscription: Subscription,
}

impl TerminalListPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _workspace_subscription =
            cx.subscribe(&workspace, |_, _, _, cx| cx.notify());
        Self {
            workspace: workspace.downgrade(),
            focus_handle,
            position: DockPosition::Left,
            _workspace_subscription,
        }
    }

    fn collect_terminals(
        &self,
        cx: &App,
    ) -> Vec<(Entity<TerminalView>, SharedString, bool)> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let workspace = workspace.read(cx);
        for pane in workspace.panes() {
            let pane = pane.read(cx);
            let active_in_pane = pane.active_item().map(|item| item.item_id());
            for item in pane.items() {
                if let Some(terminal_view) = item.act_as::<TerminalView>(cx) {
                    let title = item.tab_content_text(0, cx);
                    let is_active = Some(item.item_id()) == active_in_pane;
                    result.push((terminal_view, title, is_active));
                }
            }
        }
        result
    }

    fn focus_terminal(
        &mut self,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let target_id = terminal_view.entity_id();
        workspace.update(cx, |workspace, cx| {
            for pane in workspace.panes().to_vec() {
                let index = pane
                    .read(cx)
                    .items()
                    .position(|item| item.item_id() == target_id);
                if let Some(index) = index {
                    pane.update(cx, |pane, cx| {
                        pane.activate_item(index, true, true, window, cx)
                    });
                    break;
                }
            }
        });
    }

    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            let working_directory = std::env::current_dir().ok();
            terminal_view::terminal_panel::TerminalPanel::add_center_terminal(
                workspace,
                window,
                cx,
                move |project, cx| project.create_terminal_shell(working_directory, cx),
            )
            .detach_and_log_err(cx);
        });
    }
}

impl Focusable for TerminalListPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TerminalListPanel {}

impl Render for TerminalListPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminals = self.collect_terminals(cx);
        let theme = cx.theme().clone();

        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .items_center()
                    .justify_between()
                    .child(Label::new("Terminals").size(LabelSize::Small))
                    .child(
                        IconButton::new("new-terminal", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("New Terminal"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_terminal(window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("terminal-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(terminals.into_iter().enumerate().map(|(index, (terminal_view, title, is_active))| {
                        let colors = theme.colors().clone();
                        div()
                            .id(index)
                            .px_2()
                            .py_1()
                            .mx_1()
                            .rounded_md()
                            .cursor_pointer()
                            .when(is_active, |this| this.bg(colors.element_selected))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(IconName::Terminal)
                                            .size(IconSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(Label::new(title).size(LabelSize::Small).truncate()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.focus_terminal(terminal_view.clone(), window, cx);
                            }))
                    })),
            )
    }
}

impl Panel for TerminalListPanel {
    fn persistent_name() -> &'static str {
        "TerminalListPanel"
    }

    fn panel_key() -> &'static str {
        "terminal_list_panel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(&mut self, position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
        self.position = position;
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        px(240.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Terminal)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Terminal List")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleLeftDock)
    }

    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        true
    }

    fn activation_priority(&self) -> u32 {
        3
    }
}
