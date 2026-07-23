import re

content = open("src/terminal_list_panel.rs").read()
# Clear unused warning
content = content.replace("    has_unread: bool,", "    pub has_unread: bool,")
open("src/terminal_list_panel.rs", "w").write(content)
