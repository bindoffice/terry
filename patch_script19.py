import re

content = open("src/terminal_list_panel.rs").read()
content = content.replace("                  collapsed: p_group.collapsed,\n              });", "                  collapsed: p_group.collapsed,\n                  has_unread: false,\n              });")
open("src/terminal_list_panel.rs", "w").write(content)
