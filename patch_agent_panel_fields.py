with open("src/agent_panel.rs", "r") as f:
    text = f.read()

# restore fields to workspace
text = text.replace("    _workspace: WeakEntity<Workspace>,", "    workspace: WeakEntity<Workspace>,")
text = text.replace("            workspace: workspace.downgrade(),", "            workspace: workspace.downgrade(),")
text = text.replace("        Self { workspace }", "        Self { workspace }")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
