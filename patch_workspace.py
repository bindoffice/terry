with open("crates/workspace/src/workspace.rs", "r") as f:
    text = f.read()

target = """    pub fn most_recent_active_path(&self, cx: &App) -> Option<PathBuf> {
        self.recent_navigation_history_iter(cx)"""
replacement = """    pub fn most_recent_active_path(&self, cx: &App) -> Option<PathBuf> {
        if let Some(dir) = self.active_item(cx).and_then(|item| item.directory_for_new_file(cx)) {
            return Some(dir);
        }
        self.recent_navigation_history_iter(cx)"""

text = text.replace(target, replacement, 1)

with open("crates/workspace/src/workspace.rs", "w") as f:
    f.write(text)
