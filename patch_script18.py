import re

content = open("src/terminal_list_panel.rs").read()

search_group = """struct TerminalGroup {
    id: GroupId,
    name: SharedString,
    terminals: Vec<Entity<TerminalView>>,
    collapsed: bool,
}"""

replace_group = """struct TerminalGroup {
    id: GroupId,
    name: SharedString,
    terminals: Vec<Entity<TerminalView>>,
    collapsed: bool,
    has_unread: bool,
}"""

content = content.replace(search_group, replace_group)

search_push1 = """        self.groups.push(TerminalGroup {
            id,
            name: SharedString::from(i18n::t("terminals")),
            terminals: Vec::new(),
            collapsed: false,
        });"""

replace_push1 = """        self.groups.push(TerminalGroup {
            id,
            name: SharedString::from(i18n::t("terminals")),
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
        });"""

content = content.replace(search_push1, replace_push1)

search_push2 = """        self.groups.push(TerminalGroup {
            id,
            name,
            terminals: Vec::new(),
            collapsed: false,
        });"""

replace_push2 = """        self.groups.push(TerminalGroup {
            id,
            name,
            terminals: Vec::new(),
            collapsed: false,
            has_unread: false,
        });"""

content = content.replace(search_push2, replace_push2)
content = content.replace("            collapsed: p_group.collapsed,\n        });", "            collapsed: p_group.collapsed,\n            has_unread: false,\n        });")

open("src/terminal_list_panel.rs", "w").write(content)
