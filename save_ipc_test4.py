import re
content = open("src/main.rs").read()
content = content.replace("app.background_executor().block(session::ipc_server::start_ipc_server())", "app.background_executor().block(app.background_executor().spawn(session::ipc_server::start_ipc_server()))")
open("src/main.rs", "w").write(content)
