with open("src/agent_panel.rs", "r") as f:
    text = f.read()

text = text.replace("use language_model::LanguageModelRegistry;\nuse std::sync::Arc;", "")
text = text.replace("    workspace: WeakEntity<Workspace>,\n", "    _workspace: WeakEntity<Workspace>,\n")
text = text.replace("            workspace: workspace.weak_handle(),\n", "            _workspace: workspace.weak_handle(),\n")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
