import re

with open('crates/settings_content/src/terminal.rs', 'r') as f:
    content = f.read()

# WorkingDirectory
content = content.replace('pub enum WorkingDirectory {', '#[derive(Default)]\npub enum WorkingDirectory {\n    #[default]')

# TerminalBlink
content = content.replace('pub enum TerminalBlink {', '#[derive(Default)]\npub enum TerminalBlink {')
content = content.replace('TerminalControlled,', '#[default]\n    TerminalControlled,')

# AlternateScroll
content = content.replace('pub enum AlternateScroll {', '#[derive(Default)]\npub enum AlternateScroll {\n    #[default]')

# TerminalDockPosition
content = content.replace('pub enum TerminalDockPosition {', '#[derive(Default)]\npub enum TerminalDockPosition {')
content = content.replace('Bottom,\n    Right,', '#[default]\n    Bottom,\n    Right,')


with open('crates/settings_content/src/terminal.rs', 'w') as f:
    f.write(content)
