import re

content = open("src/terminal_list_panel.rs").read()
content = content.replace("gpui::WeakView<Self>", "WeakEntity<Self>")
content = content.replace("gpui::AsyncWindowContext", "gpui::AsyncAppContext")

open("src/terminal_list_panel.rs", "w").write(content)
