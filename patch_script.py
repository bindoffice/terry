import re

content = open("src/terminal_list_panel.rs").read()

content = content.replace("fn spawn_terminal(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {", "fn spawn_terminal(&mut self, group_id: GroupId, cwd: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {")
content = content.replace("let working_directory = std::env::current_dir().ok();", "let working_directory = cwd.or_else(|| std::env::current_dir().ok());")
content = content.replace("self.spawn_terminal(id, window, cx);", "self.spawn_terminal(id, None, window, cx);")
content = content.replace("self.spawn_terminal(self.active_group_id, window, cx);", "self.spawn_terminal(self.active_group_id, None, window, cx);")

open("src/terminal_list_panel.rs", "w").write(content)
