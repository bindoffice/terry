with open('crates/project/src/terminals.rs', 'r') as f:
    content = f.read()

# Add a `shell` parameter to create_terminal_shell
content = content.replace('pub fn create_terminal_shell(\n        &mut self,\n        cwd: Option<PathBuf>,\n        cx: &mut Context<Self>,\n    ) -> Task<Result<Entity<Terminal>>> {\n        self.create_terminal_shell_internal(cwd, false, cx)',
                          'pub fn create_terminal_shell(\n        &mut self,\n        cwd: Option<PathBuf>,\n        shell: Option<task::Shell>,\n        cx: &mut Context<Self>,\n    ) -> Task<Result<Entity<Terminal>>> {\n        self.create_terminal_shell_internal(cwd, shell, false, cx)')

content = content.replace('fn create_terminal_shell_internal(\n        &mut self,\n        cwd: Option<PathBuf>,\n        force_local: bool,\n        cx: &mut Context<Self>,\n    ) -> Task<Result<Entity<Terminal>>> {',
                          'fn create_terminal_shell_internal(\n        &mut self,\n        cwd: Option<PathBuf>,\n        shell: Option<task::Shell>,\n        force_local: bool,\n        cx: &mut Context<Self>,\n    ) -> Task<Result<Entity<Terminal>>> {')

# Find where `shell` was determined, and allow the passed-in shell to override
old_shell_logic = """        let shell = match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        };"""
new_shell_logic = """        let shell = shell.unwrap_or_else(|| match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        });"""
content = content.replace(old_shell_logic, new_shell_logic)

# In `local_terminal_action`, cwd is passed but not shell
content = content.replace('self.create_terminal_shell_internal(working_directory, true, cx)', 'self.create_terminal_shell_internal(working_directory, None, true, cx)')
# In `create_terminal_shell(cwd, cx)` replacing inside `terminals.rs`
content = content.replace('return self.create_terminal_shell(cwd, cx);', 'return self.create_terminal_shell(cwd, None, cx);')

with open('crates/project/src/terminals.rs', 'w') as f:
    f.write(content)
