import re

content = open("src/terminal_list_panel.rs").read()
search = """                    cx.subscribe(&terminal, |this, _terminal, event, cx| {
                        if let terminal::Event::TitleChanged = event {
                            this.save_session(cx);
                        }
                    }).detach();"""

replace = """                    cx.subscribe(&terminal, move |this, _terminal, event, cx| {
                        match event {
                            terminal::Event::TitleChanged => this.save_session(cx),
                            terminal::Event::Wakeup => {
                                // Background output detected -> mark group as unread
                                if let Some(group) = this.groups.iter_mut().find(|g| g.id == group_id) {
                                    group.has_unread = true;
                                    cx.notify();
                                }
                            },
                            _ => {}
                        }
                    }).detach();"""

content = content.replace(search, replace)
open("src/terminal_list_panel.rs", "w").write(content)
