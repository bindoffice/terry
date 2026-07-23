import re

content = open("src/terminal_list_panel.rs").read()

content = content.replace("self.spawn_terminal(id, None, window, cx);", "self.spawn_terminal(id, None, window, cx);\n        self.save_session(cx);")
content = content.replace("self.spawn_terminal(self.active_group_id, None, window, cx);", "self.spawn_terminal(self.active_group_id, None, window, cx);\n        self.save_session(cx);")
content = content.replace("self.groups.remove(group_index);", "self.groups.remove(group_index);\n        self.save_session(cx);")

open("src/terminal_list_panel.rs", "w").write(content)
