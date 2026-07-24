with open('crates/project/src/terminals.rs', 'r') as f:
    content = f.read()

bad2 = """        let shell = match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        };"""
good2 = """        let shell = shell.unwrap_or_else(|| match &remote_client {
            Some(remote_client) => remote_client
                .read(cx)
                .shell()
                .unwrap_or_else(get_default_system_shell),
            None => get_system_shell(),
        });"""
content = "".join(content.rsplit(bad2, 1)).strip() + "}\n" # Just simple replacement string
content = content.replace(bad2, good2) # It was the second one

with open('crates/project/src/terminals.rs', 'w') as f:
    f.write(content)
