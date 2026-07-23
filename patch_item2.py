with open("crates/workspace/src/item.rs", "r") as f:
    text = f.read()

text = text.replace("use std::{", "use std::path::PathBuf;\nuse std::{", 1)

with open("crates/workspace/src/item.rs", "w") as f:
    f.write(text)
