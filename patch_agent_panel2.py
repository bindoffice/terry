with open("src/agent_panel.rs", "r") as f:
    text = f.read()

text = text.replace("use language_model::LanguageModelRegistry;\nuse std::sync::Arc;", "")
# Oh wait, let's just make sure `use language_model::LanguageModelRegistry;` is back.
text = "use language_model::LanguageModelRegistry;\n" + text

text = text.replace("this._workspace.upgrade()", "this.workspace.upgrade()")
text = text.replace("Self { _workspace }", "Self { workspace }")

with open("src/agent_panel.rs", "w") as f:
    f.write(text)
