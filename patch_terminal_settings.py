import re

with open('crates/terminal/src/terminal_settings.rs', 'r') as f:
    lines = f.readlines()

for i in range(80, 140):
    lines[i] = lines[i].replace('.unwrap()', '.unwrap_or_default()')

with open('crates/terminal/src/terminal_settings.rs', 'w') as f:
    f.writelines(lines)
