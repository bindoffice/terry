with open("crates/workspace/src/item.rs", "r") as f:
    text = f.read()

if "std::path::PathBuf" not in text:
    text = "use std::path::PathBuf;\n" + text

with open("crates/workspace/src/item.rs", "w") as f:
    f.write(text)
