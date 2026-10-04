//! IPC glue:
//! - the client behind the `terry new-tab` / `terry send-text` CLI
//!   subcommands, which talk to an already-running Terry over loopback HTTP;
//! - the GUI-side consumer that turns IPC requests into workspace actions.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use gpui::{AnyWindowHandle, App, AsyncApp, WindowHandle, WindowId};
use serde_json::{Value, json};
use session::ipc_server::{IpcRequest, IpcRequestKind};
use workspace::{AppState, OpenOptions, Workspace};

use crate::terminal_list_panel::TerminalListPanel;

// -------------------------------------------------------------------------
// CLI client
// -------------------------------------------------------------------------

/// Handles `terry` CLI subcommands before the GUI boots. Returns
/// `Some(exit_code)` when the process should exit early, `None` to continue
/// launching the GUI as usual.
pub fn run_cli_subcommand(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str) {
        Some("new-tab") => Some(match parse_new_tab_args(&args[1..]) {
            Ok(cwd) => post_command("/api/new_tab", &json!({ "cwd": cwd }).to_string()),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        }),
        Some("send-text") => Some(match parse_send_text_args(&args[1..]) {
            Ok(text) => post_command("/api/send_text", &json!({ "text": text }).to_string()),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        }),
        Some("help" | "--help" | "-h") => {
            print_cli_usage();
            Some(0)
        }
        _ => None,
    }
}

fn parse_new_tab_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut cwd = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg == "--cwd" {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err("--cwd 需要一个路径参数，例如 terry new-tab --cwd /tmp".to_string());
            };
            cwd = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--cwd=") {
            cwd = Some(PathBuf::from(value));
        } else {
            return Err(format!("new-tab 未知参数: {arg}"));
        }
        index += 1;
    }
    Ok(cwd)
}

fn parse_send_text_args(args: &[String]) -> Result<String, String> {
    let args = match args.first().map(String::as_str) {
        Some("--") => &args[1..],
        _ => args,
    };
    if args.is_empty() {
        return Err("send-text 需要文本参数，例如 terry send-text \"ls -la\"".to_string());
    }
    Ok(args.join(" "))
}

fn print_cli_usage() {
    println!("用法: terry [子命令]");
    println!();
    println!("不带子命令时启动 Terry 图形界面。");
    println!();
    println!("子命令:");
    println!("  new-tab [--cwd PATH]  在已运行的 Terry 实例中打开一个新的终端标签页");
    println!("  send-text TEXT        向当前活动的终端粘贴文本（换行会作为回车发送）");
    println!("  send-text -- TEXT     以 -- 分隔，原样发送后续文本");
    println!("  help                  显示本帮助");
}

#[derive(serde::Deserialize)]
struct IpcInfo {
    port: u16,
    token: String,
}

fn ipc_info_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("terry/ipc.json")
}

fn read_ipc_info() -> Option<IpcInfo> {
    let raw = std::fs::read_to_string(ipc_info_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Sends `body` to the running Terry instance and maps the outcome to an exit
/// code: 0 on 2xx, 1 on transport/app failure (2 is reserved for usage
/// errors). The reply body is echoed to stdout.
fn post_command(path: &str, body: &str) -> i32 {
    let Some(info) = read_ipc_info() else {
        eprintln!(
            "Terry 未运行或 IPC 未启用（无法读取 {}）",
            ipc_info_path().display()
        );
        return 1;
    };

    match http_post(info.port, path, &info.token, body) {
        Ok((status, response)) => {
            println!("{}", response.trim());
            if (200..300).contains(&status) { 0 } else { 1 }
        }
        Err(error) => {
            eprintln!("Terry 未运行或 IPC 未启用: {error}");
            1
        }
    }
}

/// Minimal HTTP/1.1 client over a raw TcpStream — keeps the GUI binary free
/// of an extra HTTP client dependency for scripting purposes.
fn http_post(port: u16, path: &str, token: &str, body: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok();

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let raw = String::from_utf8_lossy(&raw);

    let (head, response_body) = raw.split_once("\r\n\r\n").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed HTTP response")
    })?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing HTTP status line")
        })?;

    Ok((status, response_body.to_string()))
}

// -------------------------------------------------------------------------
// GUI-side consumer
// -------------------------------------------------------------------------

/// Spawns the loop that receives requests from the IPC HTTP server (which
/// runs on its own thread) and applies them to the workspace. The gpui
/// `AsyncApp` is not `Send`, so the channel is the thread boundary; requests
/// sent before the GUI finishes booting simply buffer here.
pub fn init(mut request_rx: futures::channel::mpsc::UnboundedReceiver<IpcRequest>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(request) = request_rx.next().await {
            let IpcRequest { kind, reply } = request;
            let outcome = match kind {
                IpcRequestKind::NewTab { cwd } => new_tab(cx, cwd).await,
                IpcRequestKind::SendText { text } => send_text(cx, &text).await,
            };
            reply.send(outcome).ok();
        }
    })
    .detach();
}

/// Picks the workspace window to act on: the active window, else windows from
/// the platform focus stack, else any window. Returns `None` when no
/// workspace window exists yet.
async fn workspace_window(cx: &mut AsyncApp) -> Result<Option<WindowHandle<Workspace>>, String> {
    let candidates: Vec<AnyWindowHandle> = cx.update(|cx| {
        let mut candidates = Vec::new();
        if let Some(active) = cx.active_window() {
            candidates.push(active);
        }
        if let Some(stack) = cx.window_stack() {
            candidates.extend(stack);
        }
        candidates.extend(cx.windows());
        candidates
    });

    for handle in candidates {
        if let Some(window) = handle.downcast::<Workspace>() {
            return Ok(Some(window));
        }
    }
    Ok(None)
}

async fn new_tab(cx: &mut AsyncApp, cwd: Option<PathBuf>) -> Result<Value, String> {
    if let Some(cwd) = &cwd {
        if !cwd.is_dir() {
            return Err(format!("cwd 不存在: {}", cwd.display()));
        }
    }

    let window = match workspace_window(cx).await? {
        Some(window) => window,
        None => open_workspace_window(cx).await?,
    };
    let cwd_display = cwd.as_ref().map(|path| path.display().to_string());

    window
        .update(cx, move |workspace, window, cx| {
            let panel = workspace
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| {
                // open_shell_at no-ops when the active group is missing, so
                // make sure one exists first (create_default_group is
                // idempotent).
                panel.create_default_group(window, cx);
                match &cwd {
                    Some(cwd) => panel.open_shell_at(cwd.clone(), window, cx),
                    None => panel.new_terminal(window, cx),
                }
            });
            window.activate_window();
            Ok(json!({ "status": "ok", "action": "new_tab", "cwd": cwd_display }))
        })
        .map_err(|error| error.to_string())?
}

async fn send_text(cx: &mut AsyncApp, text: &str) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |workspace, _window, cx| {
            let panel = workspace
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = panel
                .read(cx)
                .active_terminal_view(cx)
                .ok_or_else(|| "no active terminal".to_string())?;
            view.update(cx, |view, cx| {
                view.terminal()
                    .update(cx, |terminal, _cx| terminal.paste(text));
            });
            Ok(json!({ "status": "ok", "action": "send_text" }))
        })
        .map_err(|error| error.to_string())?
}

/// Opens a fresh workspace window (used when Terry is running with zero
/// windows) and returns its handle.
async fn open_workspace_window(cx: &mut AsyncApp) -> Result<WindowHandle<Workspace>, String> {
    let existing: Vec<WindowId> = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .map(|handle| handle.window_id())
            .collect()
    });

    let app_state = cx.update(|cx| {
        AppState::try_global(cx).ok_or_else(|| "app state is not initialized yet".to_string())
    })?;

    let task = cx.update(|cx| {
        workspace::open_new(
            OpenOptions::default(),
            app_state,
            cx,
            |workspace, window, cx| crate::init_workspace(workspace, window, cx),
        )
    });
    task.await.map_err(|error| error.to_string())?;

    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find(|handle| !existing.contains(&handle.window_id()))
            .and_then(|handle| handle.downcast::<Workspace>())
            .ok_or_else(|| "new workspace window did not appear".to_string())
    })
}
