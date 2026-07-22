use gpui::{
    Action, AppContext as _, Context, IntoElement, ParentElement, Render, Window, div,
};
use platform_title_bar::PlatformTitleBar;
use ui::{Color, IconButton, IconName, IconSize, Label, LabelSize, Tooltip, prelude::*};
use workspace::Workspace;

pub struct AppTitleBar {
    platform_titlebar: gpui::Entity<PlatformTitleBar>,
}

impl AppTitleBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            platform_titlebar: cx.new(|cx| PlatformTitleBar::new("title-bar", cx)),
        }
    }
}

impl Render for AppTitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.platform_titlebar.update(cx, |titlebar, _cx| {
            titlebar.set_children([div()
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                .pr_2()
                .child(
                    Label::new("Ink")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    IconButton::new("open-settings", IconName::Settings)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(i18n::t("settings")))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                zed_actions::OpenSettings.boxed_clone(),
                                cx,
                            );
                        }),
                )
                .into_any_element()]);
        });
        self.platform_titlebar.clone().into_any_element()
    }
}

pub fn init(cx: &mut gpui::App) {
    PlatformTitleBar::init(cx);

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let item = cx.new(AppTitleBar::new);
        workspace.set_titlebar_item(item.into(), window, cx);
    })
    .detach();
}
