import re

content = open("src/terminal_list_panel.rs").read()

search = """                    })
                {
                    this.groups[group_index].terminals.push(terminal.clone());
"""
replace = """                    })
                {
                    this.groups[group_index].terminals.push(terminal.clone());

                    cx.subscribe(&terminal, |this, _terminal, event, cx| {
                        if let terminal::Event::TitleChanged = event {
                            this.save_session(cx);
                        }
                    }).detach();
"""

content = content.replace(search, replace)
open("src/terminal_list_panel.rs", "w").write(content)
