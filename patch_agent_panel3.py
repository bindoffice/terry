with open("src/agent_panel.rs", "r") as f:
    text = f.read()

text = text.replace("            _workspace: workspace.weak_handle(),\n", "            workspace: workspace.weak_handle(),\n")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
