with open("src/main.rs", "r") as f:
    text = f.read()

target = "        language_models::init(app_state.user_store.clone(), client.clone(), cx);"
replacement = """        language_models::init(app_state.user_store.clone(), client.clone(), cx);
        client::llm_token::RefreshLlmTokenListener::register(client.clone(), app_state.user_store.clone(), cx);"""

text = text.replace(target, replacement, 1)

with open("src/main.rs", "w") as f:
    f.write(text)
