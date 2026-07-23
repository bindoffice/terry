with open("src/main.rs", "r") as f:
    text = f.read()

target = """        language_models::init(app_state.user_store.clone(), client.clone(), cx);
        client::llm_token::RefreshLlmTokenListener::register(client.clone(), app_state.user_store.clone(), cx);        terminal_list_panel::init(cx);"""
replacement = """        client::llm_token::RefreshLlmTokenListener::register(client.clone(), app_state.user_store.clone(), cx);
        language_models::init(app_state.user_store.clone(), client.clone(), cx);
        terminal_list_panel::init(cx);"""

text = text.replace(target, replacement, 1)

with open("src/main.rs", "w") as f:
    f.write(text)
