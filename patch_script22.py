import re

content = open("src/terminal_list_panel.rs").read()
search = """impl Render for TerminalListPanel {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
"""
replace = """impl Render for TerminalListPanel {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        for group in &mut self.groups {
            if group.id == self.active_group_id {
                group.has_unread = false;
            }
        }
"""
content = content.replace(search, replace)
open("src/terminal_list_panel.rs", "w").write(content)
