with open('crates/project/src/terminals.rs', 'r') as f:
    content = f.read()

# Fix in create_terminal_task
bad_shell = """        let remote_client = self.remote_client.clone();
        let shell = shell.unwrap_or_else(|| match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        });"""
good_shell = """        let remote_client = self.remote_client.clone();
        let shell = match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        };"""
content = content.replace(bad_shell, good_shell)

# Fix in create_terminal_shell_internal where `shell` IS defined as a parameter
old_shell_internal = """        let shell = match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        };"""
new_shell_internal = """        let shell = shell.unwrap_or_else(|| match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        });"""
content = content.replace(old_shell_internal, new_shell_internal)

with open('crates/project/src/terminals.rs', 'w') as f:
    f.write(content)
