import re

content = open("src/terminal_list_panel.rs").read()
# Let's just suppress the warning directly
content = content.replace("    pub has_unread: bool,", "    #[allow(dead_code)]\n    pub has_unread: bool,")
open("src/terminal_list_panel.rs", "w").write(content)

content2 = open("crates/session/src/ipc_server.rs").read()
content2 = content2.replace("Json(payload)", "Json(_payload)")
open("crates/session/src/ipc_server.rs", "w").write(content2)

import os
os.system("cargo clippy --workspace --exclude gpui --exclude terminal_view --exclude remote --exclude yawc --exclude zed-reqwest")
