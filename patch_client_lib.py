with open("crates/client/src/client.rs", "r") as f:
    text = f.read()

text = text.replace("mod llm_token;", "pub mod llm_token;")

with open("crates/client/src/client.rs", "w") as f:
    f.write(text)
