import re

content = open("src/terminal_list_panel.rs").read()

content = content.replace("cx: &AppContext", "cx: &App")
content = content.replace("cx.spawn(|this, mut cx|", "cx.spawn(|this, cx|")

open("src/terminal_list_panel.rs", "w").write(content)
