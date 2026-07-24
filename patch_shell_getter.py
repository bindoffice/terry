with open('crates/terminal/src/terminal.rs', 'r') as f:
    content = f.read()

getter = """
    pub fn shell(&self) -> &task::Shell {
        &self.template.shell
    }
"""

content = content.replace('pub fn working_directory(&self) -> Option<PathBuf> {', 'pub fn working_directory(&self) -> Option<PathBuf> {\n' + getter)

with open('crates/terminal/src/terminal.rs', 'w') as f:
    f.write(content)
