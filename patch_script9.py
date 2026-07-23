import re

content = open("src/terminal_list_panel.rs").read()
content = content.replace("cx.spawn(|this: WeakEntity<Self>, mut cx: gpui::AsyncAppContext| async move {", "cx.spawn(|this, mut cx| async move {")
content = content.replace("}).ok();", "});")

open("src/terminal_list_panel.rs", "w").write(content)
