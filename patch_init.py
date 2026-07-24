with open('crates/terminal/src/terminal.rs', 'r') as f:
    lines = f.readlines()

for i, line in enumerate(lines):
    if 'let terminal = Terminal {' in line:
        lines.insert(i + 1, '            content_dirty: true,\n')

with open('crates/terminal/src/terminal.rs', 'w') as f:
    f.writelines(lines)
