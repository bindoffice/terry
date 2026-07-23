import re

content = open("src/terminal_list_panel.rs").read()
content = content.replace("cx.spawn(|this, mut cx: gpui::AsyncApp| async move {", "cx.spawn(|this: WeakEntity<Self>, mut cx: gpui::AsyncApp| async move {")

open("src/terminal_list_panel.rs", "w").write(content)
