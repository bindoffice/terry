import re

content = open("src/terminal_list_panel.rs").read()

new_structs = """
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
struct PersistedTerminal {
    cwd: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedGroup {
    name: String,
    collapsed: bool,
    terminals: Vec<PersistedTerminal>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PersistedSession {
    groups: Vec<PersistedGroup>,
    active_group_index: usize,
}
"""

# add after "use workspace::{ItemHandle, Pane};"
content = content.replace("use workspace::{ItemHandle, Pane};", "use workspace::{ItemHandle, Pane};\n" + new_structs)

load_save_methods = """
    fn session_file_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ink/sessions/default.json")
    }

    pub fn save_session(&self, cx: &AppContext) {
        let mut session = PersistedSession {
            groups: Vec::new(),
            active_group_index: 0,
        };

        for (i, group) in self.groups.iter().enumerate() {
            if group.id == self.active_group_id {
                session.active_group_index = i;
            }

            let mut p_group = PersistedGroup {
                name: group.name.to_string(),
                collapsed: group.collapsed,
                terminals: Vec::new(),
            };

            for view_ent in &group.terminals {
                let cwd = view_ent.read(cx).terminal().read(cx).working_directory();
                p_group.terminals.push(PersistedTerminal { cwd });
            }

            session.groups.push(p_group);
        }

        let path = Self::session_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json = serde_json::to_string(&session).unwrap_or_default();
        std::fs::write(path, json).ok();
    }

    pub fn load_persisted_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let path = Self::session_file_path();
        let Ok(data) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(session): Result<PersistedSession, _> = serde_json::from_str(&data) else {
            return false;
        };

        if session.groups.is_empty() {
            return false;
        }

        for (i, p_group) in session.groups.into_iter().enumerate() {
            let id = self.new_group_id();
            let name = SharedString::from(p_group.name);
            self.groups.push(TerminalGroup {
                id,
                name,
                terminals: Vec::new(),
                collapsed: p_group.collapsed,
            });
            
            if i == session.active_group_index {
                self.active_group_id = id;
            }

            for p_term in p_group.terminals {
                self.spawn_terminal(id, p_term.cwd, window, cx);
            }
        }
        
        // After loading groups, switch to the active one
        self.switch_group(self.active_group_id, window, cx);
        true
    }
"""

content = content.replace("    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {\n        if !self.groups.is_empty() {\n            return;\n        }", "    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {\n        if !self.groups.is_empty() {\n            return;\n        }\n\n        if self.load_persisted_session(window, cx) {\n            return;\n        }")

# add the load methods
content = content.replace("    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {", load_save_methods + "\n    pub fn create_default_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {")

open("src/terminal_list_panel.rs", "w").write(content)
