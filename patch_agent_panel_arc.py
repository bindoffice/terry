with open("src/agent_panel.rs", "r") as f:
    text = f.read()

text = text.replace("use std::sync::Arc;\n", "")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
