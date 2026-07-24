with open('crates/project/src/terminals.rs', 'r') as f:
    content = f.read()

idx = content.rfind("let shell = match &remote_client")
if idx != -1:
    content = content[:idx] + content[idx:].replace("let shell = match &remote_client", "let shell = shell.unwrap_or_else(|| match &remote_client")
    idx2 = content.find("};", idx)
    content = content[:idx2] + "});" + content[idx2+2:]

with open('crates/project/src/terminals.rs', 'w') as f:
    f.write(content)
