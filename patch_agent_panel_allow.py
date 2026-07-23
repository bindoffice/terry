with open("src/agent_panel.rs", "r") as f:
    text = f.read()

text = text.replace("    workspace: WeakEntity<Workspace>,\n    focus_handle: FocusHandle,", "    #[allow(dead_code)]\n    workspace: WeakEntity<Workspace>,\n    focus_handle: FocusHandle,")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
