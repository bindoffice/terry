import sys
with open('crates/workspace/src/item.rs', 'r') as f:
    text = f.read()

target = "    fn has_deleted_file(&self, _: &App) -> bool {"
replacement = """    fn directory_for_new_file(&self, cx: &App) -> Option<PathBuf> {
        None
    }

    fn has_deleted_file(&self, _: &App) -> bool {"""

text = text.replace(target, replacement, 1)

target2 = "    fn can_save_as(&self, cx: &App) -> bool;"
replacement2 = """    fn directory_for_new_file(&self, cx: &App) -> Option<PathBuf>;
    fn can_save_as(&self, cx: &App) -> bool;"""
text = text.replace(target2, replacement2, 1)

with open('crates/workspace/src/item.rs', 'w') as f:
    f.write(text)
