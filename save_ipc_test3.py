import re

content = open("src/main.rs").read()

bad_ipc = """
    // Start IPC Server
    let (ipc_port, ipc_token) = app.background_executor().block(session::ipc_server::start_ipc_server()).unwrap();
    let ipc_info = serde_json::json!({
        "port": ipc_port,
        "token": ipc_token
    });
    
    let ipc_path = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("ink/ipc.json");
    if let Some(parent) = ipc_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&ipc_path, serde_json::to_string(&ipc_info).unwrap()).ok();

    let app = Application::with_platform(gpui_platform::current_platform(false)).with_assets(Assets);
"""

good_ipc = """
    let app = Application::with_platform(gpui_platform::current_platform(false)).with_assets(Assets);

    // Start IPC Server
    let (ipc_port, ipc_token) = app.background_executor().block(session::ipc_server::start_ipc_server()).unwrap();
    let ipc_info = serde_json::json!({
        "port": ipc_port,
        "token": ipc_token
    });
    
    let ipc_path = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("ink/ipc.json");
    if let Some(parent) = ipc_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&ipc_path, serde_json::to_string(&ipc_info).unwrap()).ok();
"""

content = content.replace(bad_ipc, good_ipc)
open("src/main.rs", "w").write(content)
