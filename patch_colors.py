with open("src/terminal_list_panel.rs", "r") as f:
    text = f.read()

text = text.replace("let colors = theme.colors().clone();", "let _colors = theme.colors().clone();")

with open("src/terminal_list_panel.rs", "w") as f:
    f.write(text)
