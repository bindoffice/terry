import re

with open("crates/workspace/src/status_bar.rs", "r") as f:
    content = f.read()

toggle_code = """
        let toggle = IconButton::new("toggle-sidebar-left", if sidebar.open { ui::IconName::SidebarLeftOpen } else { ui::IconName::SidebarLeftClosed })
            .icon_size(IconSize::Small)
            .tooltip(|_, cx| Tooltip::for_action("Toggle Sidebar", &crate::ToggleWorkspaceSidebar, cx))
            .on_click(|_, window, cx| {
                if let Some(mut multi_workspace) = window.root::<crate::MultiWorkspace>().flatten() {
                    multi_workspace.update(cx, |multi_workspace, cx| {
                        multi_workspace.toggle_sidebar(window, cx);
                    });
                }
            });
"""

# inject into render_left_tools
new_method = """    fn render_left_tools(
        &self,
        sidebar: &SidebarStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
%s
        h_flex()
            .gap_1()
            .min_w_0()
            .overflow_x_hidden()
            .child(toggle)
            .children(self.left_items.iter().enumerate().map(|(index, item)| {
                render_hideable_item("status-bar-left", index, item.as_ref(), cx)
            }))
    }
""" % toggle_code

content = re.sub(
    r"    fn render_left_tools\(.+?-> impl IntoElement \{[\s\S]+?\}\n    \}",
    new_method,
    content
)

with open("crates/workspace/src/status_bar.rs", "w") as f:
    f.write(content)
