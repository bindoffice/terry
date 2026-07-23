with open("crates/terminal_view/src/terminal_view.rs", "r") as f:
    text = f.read()

target = "    fn boxed_clone(&self) -> Box<dyn ItemHandle> {"
replacement = """    fn directory_for_new_file(&self, cx: &App) -> Option<PathBuf> {
        self.terminal().read(cx).working_directory()
    }

    fn boxed_clone(&self) -> Box<dyn ItemHandle> {"""

text = text.replace(target, replacement, 1)

with open("crates/terminal_view/src/terminal_view.rs", "w") as f:
    f.write(text)
