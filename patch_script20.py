import re

content = open("src/terminal_list_panel.rs").read()
search = """                terminals: Vec::new(),
                collapsed: p_group.collapsed,
            });"""
replace = """                terminals: Vec::new(),
                collapsed: p_group.collapsed,
                has_unread: false,
            });"""
content = content.replace(search, replace)
open("src/terminal_list_panel.rs", "w").write(content)
