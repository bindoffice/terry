with open("crates/workspace/src/item.rs", "r") as f:
    text = f.read()

target = "    fn has_deleted_file(&self, cx: &App) -> bool {"
replacement = """    fn directory_for_new_file(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx).directory_for_new_file(cx)
    }

    fn has_deleted_file(&self, cx: &App) -> bool {"""
text = text.replace(target, replacement, 1)

with open("crates/workspace/src/item.rs", "w") as f:
    f.write(text)
