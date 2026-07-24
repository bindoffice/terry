import re

with open('crates/terminal_view/src/persistence.rs', 'r') as f:
    content = f.read()

content = content.replace('SELECT working_directory\n             FROM terminals', 'SELECT working_directory_path\n             FROM terminals')

with open('crates/terminal_view/src/persistence.rs', 'w') as f:
    f.write(content)
