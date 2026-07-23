with open("crates/workspace/src/workspace.rs", "r") as f:
    text = f.read()

target = "    pub(crate) modal_layer: Entity<ModalLayer>,"
replacement = """    pub(crate) modal_layer: Entity<ModalLayer>,
    last_active_directory: Option<PathBuf>,"""

text = text.replace(target, replacement, 1)

target2 = "            last_active_view_id: None,"
replacement2 = """            last_active_view_id: None,
            last_active_directory: None,"""
text = text.replace(target2, replacement2, 1)

target3 = "        cx.emit(Event::ActiveItemChanged);"
replacement3 = """        cx.emit(Event::ActiveItemChanged);
        if let Some(dir) = self.active_item(cx).and_then(|item| item.directory_for_new_file(cx)) {
            self.last_active_directory = Some(dir);
        }"""
text = text.replace(target3, replacement3, 1)

with open("crates/workspace/src/workspace.rs", "w") as f:
    f.write(text)
