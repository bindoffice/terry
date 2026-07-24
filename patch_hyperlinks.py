import re
with open('crates/terminal/src/alacritty/hyperlinks.rs', 'r') as f:
    content = f.read()

content = content.replace('number.parse::<u32>().unwrap()', 'number.parse::<u32>().unwrap_or(0)')

with open('crates/terminal/src/alacritty/hyperlinks.rs', 'w') as f:
    f.write(content)
