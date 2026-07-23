import re

content = open("src/terminal_list_panel.rs").read()
search = """                    if self.renaming_group_id == Some(group.id) {
"""
replace = """
                    let has_unread = group.has_unread;

                    if self.renaming_group_id == Some(group.id) {
"""
content = content.replace(search, replace)

search2 = """                                                .child(group.name.clone())
                                        )"""
replace2 = """                                                .child(group.name.clone())
                                        )
                                        .when(has_unread, |el| el.child(
                                            div().w_2().h_2().rounded_full().bg(gpui::red())
                                        ))"""

content = content.replace(search2, replace2)
open("src/terminal_list_panel.rs", "w").write(content)
