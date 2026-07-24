with open('crates/project/src/terminals.rs', 'r') as f:
    content = f.read()

bad1 = """        let shell = shell.unwrap_or_else(|| match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        });"""

good1 = """        let shell = match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        };"""
content = content.replace(bad1, good1, 1) # Only replace the first occurrence

with open('crates/project/src/terminals.rs', 'w') as f:
    f.write(content)
