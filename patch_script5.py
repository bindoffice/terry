import re

content = open("src/terminal_list_panel.rs").read()

content = content.replace("cx.spawn(|this, cx| async move {", "cx.spawn(|this, mut cx| async move {")

open("src/terminal_list_panel.rs", "w").write(content)
