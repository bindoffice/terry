//! IPC glue:
//! - the client behind the `terry new-tab` / `terry send-text` CLI
//!   subcommands, which talk to an already-running Terry over loopback HTTP;
//! - the GUI-side consumer that turns IPC requests into workspace actions.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    AnyWindowHandle, App, AsyncApp, EntityId, Keystroke, SystemNotification, WindowHandle, WindowId,
};
use serde_json::{Value, json};
use session::ipc_server::{IpcRequest, IpcRequestKind};
use settings::Settings;
use terminal::terminal_settings::TerminalSettings;
use terminal_view::TerminalView;
use workspace::{AppState, MultiWorkspace, OpenOptions};

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
    println!();
    println!("选项:");
    println!("  --new-instance        即使已有实例在运行，也启动一个新实例");
}

#[derive(serde::Deserialize)]
struct IpcInfo {
    port: u16,
    token: String,
    /// Diagnostics written by newer builds; older `ipc.json` files omit them.
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    exe: Option<PathBuf>,
    #[serde(default)]
    arch: Option<String>,
}

fn ipc_info_path() -> PathBuf {
    paths::data_dir().join("ipc.json")
}

fn read_ipc_info() -> Option<IpcInfo> {
    let raw = std::fs::read_to_string(ipc_info_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

// -------------------------------------------------------------------------
// Single instance
// -------------------------------------------------------------------------

fn instance_lock_path() -> PathBuf {
    paths::data_dir().join("instance.lock")
}

fn try_create_instance_lock(path: &Path) -> bool {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            let _ = write!(file, "{}", std::process::id());
            true
        }
        Err(_) => false,
    }
}

fn read_lock_pid() -> Option<u32> {
    let contents = std::fs::read_to_string(instance_lock_path()).ok()?;
    contents.trim().parse().ok()
}

/// Details about the instance that accepted a forwarded launch.
///
/// Exiting after a handoff used to be completely silent, which is
/// indistinguishable from a binary that refuses to run — especially when the
/// instance holding the lock is an older build, or an x86 build under Rosetta.
pub struct RunningInstance {
    pid: Option<u32>,
    port: Option<u16>,
    executable: Option<PathBuf>,
    arch: Option<String>,
}

impl RunningInstance {
    fn load() -> Self {
        let info = read_ipc_info();
        let pid = info
            .as_ref()
            .and_then(|info| info.pid)
            .or_else(read_lock_pid);
        let port = info.as_ref().map(|info| info.port);
        let (executable, arch) = match info {
            Some(info) => (info.exe, info.arch),
            None => (None, None),
        };
        Self {
            pid,
            port,
            executable,
            arch,
        }
    }

    /// Explanation printed to stderr by the process that is about to exit.
    pub fn describe(&self) -> String {
        let mut details = Vec::new();
        if let Some(pid) = self.pid {
            details.push(format!("pid {pid}"));
        }
        if let Some(executable) = &self.executable {
            details.push(executable.display().to_string());
        }
        if let Some(arch) = &self.arch {
            details.push(arch.clone());
        }

        let mut message = if details.is_empty() {
            "terry: 已有 Terry 实例在运行，本次启动已转交给它，因此没有打开新窗口。\n".to_string()
        } else {
            format!(
                "terry: 已有 Terry 实例在运行（{}），本次启动已转交给它，因此没有打开新窗口。\n",
                details.join(", ")
            )
        };
        if let Some(port) = self.port {
            message.push_str(&format!("terry: 该实例的 IPC 端口为 {port}。\n"));
        }
        message.push_str("terry: 请先退出该实例，或使用 terry --new-instance 强制启动新实例。\n");
        message
    }
}

/// Ensure only one GUI process owns the session files. If another instance is
/// already running, ask it to focus and return details about it so the caller
/// can explain why it is exiting.
///
/// Returns `None` when this process may start the GUI. A crashed process can
/// leave a stale lock; in that case the lock is reclaimed after the focus
/// attempts fail. If the lock cannot be created at all (permissions), startup
/// proceeds anyway rather than blocking the user.
pub fn acquire_single_instance() -> Option<RunningInstance> {
    let path = instance_lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    if try_create_instance_lock(&path) {
        return None;
    }

    // Give an instance that is still booting time to publish ipc.json, then
    // ask it to focus. Retry a few times before treating the lock as stale.
    let mut ipc_attempts = 0;
    for _ in 0..20 {
        if focus_existing_instance() {
            return Some(RunningInstance::load());
        }
        if read_ipc_info().is_some() {
            ipc_attempts += 1;
            if ipc_attempts >= 3 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let _ = std::fs::remove_file(&path);
    if try_create_instance_lock(&path) {
        return None;
    }
    // Another process won the race after the stale lock was removed.
    if focus_existing_instance() {
        return Some(RunningInstance::load());
    }
    None
}

/// Remove the lock file on a clean quit, but only if this process still owns
/// it (it may have been reclaimed after a crash).
pub fn release_instance_lock() {
    let path = instance_lock_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    if contents.trim().parse::<u32>().ok() == Some(std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

fn focus_existing_instance() -> bool {
    let Some(info) = read_ipc_info() else {
        return false;
    };
    matches!(
        http_post_with_timeout(
            info.port,
            "/api/focus",
            &info.token,
            "{}",
            Duration::from_millis(500),
            Duration::from_secs(1),
            Duration::from_secs(1),
        ),
        Ok((status, body))
            if (200..300).contains(&status) && body.contains("\"action\":\"focus\"")
    )
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
    http_post_with_timeout(
        port,
        path,
        token,
        body,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(15),
    )
}

fn http_post_with_timeout(
    port: u16,
    path: &str,
    token: &str,
    body: &str,
    connect_timeout: Duration,
    write_timeout: Duration,
    read_timeout: Duration,
) -> std::io::Result<(u16, String)> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, connect_timeout)?;
    stream.set_write_timeout(Some(write_timeout)).ok();
    stream.set_read_timeout(Some(read_timeout)).ok();

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
                IpcRequestKind::Focus => focus(cx).await,
                IpcRequestKind::FocusTerminal { terminal_id } => {
                    focus_terminal(cx, terminal_id).await
                }
                IpcRequestKind::ListSessions => list_sessions(cx).await,
                IpcRequestKind::SendKeys { terminal_id, keys } => {
                    send_keys(cx, terminal_id, &keys).await
                }
                IpcRequestKind::ReadScreen { terminal_id } => read_screen(cx, terminal_id).await,
                IpcRequestKind::GetContext { terminal_id } => get_context(cx, terminal_id).await,
                IpcRequestKind::Notify { title, message } => notify(cx, title, &message).await,
            };
            reply.send(outcome).ok();
        }
    })
    .detach();
}

/// Picks the workspace window to act on: the active window, else windows from
/// the platform focus stack, else any window. Returns `None` when no
/// workspace window exists yet.
///
/// Workspace windows are rooted at `MultiWorkspace`, not `Workspace`: the
/// multi-workspace view owns the active workspace entity, so downcasting to
/// `Workspace` never matches (and would make every IPC call open a new window).
async fn workspace_window(
    cx: &mut AsyncApp,
) -> Result<Option<WindowHandle<MultiWorkspace>>, String> {
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
        if let Some(window) = handle.downcast::<MultiWorkspace>() {
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

    // Capture the current terminal count/active id, then wait for the spawn to
    // finish. Returning before the shell exists made `new-tab && send-text`
    // race against the old active terminal, and spawn failures looked like
    // successes.
    let before = window
        .update(cx, move |multi_workspace, window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                let before = {
                    let panel = panel.read(cx);
                    (panel.terminal_count(), panel.focused_terminal_id(cx))
                };
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
                Ok::<_, String>(before)
            })
        })
        .map_err(|error| error.to_string())??;

    let mut observed_new = false;
    let mut new_terminal_id = None;
    let mut polls_after_spawn = 0;
    for _ in 0..50 {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        let state = window
            .update(cx, |multi_workspace, _window, cx| {
                let workspace = multi_workspace.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    workspace.panel::<TerminalListPanel>(cx).map(|panel| {
                        let panel = panel.read(cx);
                        (panel.terminal_count(), panel.focused_terminal_id(cx))
                    })
                })
            })
            .map_err(|error| error.to_string())?;
        let Some((count, focused)) = state else {
            continue;
        };
        if count > before.0 {
            if focused != before.1 && focused.is_some() {
                new_terminal_id = focused;
                observed_new = true;
                break;
            }
            if !observed_new {
                observed_new = true;
                new_terminal_id = focused;
            }
            polls_after_spawn += 1;
            if polls_after_spawn >= 5 {
                break;
            }
        }
    }

    if !observed_new {
        return Err("timed out waiting for the new terminal to open".to_string());
    }

    Ok(json!({
        "status": "ok",
        "action": "new_tab",
        "cwd": cwd_display,
        "terminal_id": new_terminal_id.map(|id| id.as_u64()),
    }))
}

async fn send_text(cx: &mut AsyncApp, text: &str) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
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
        })
        .map_err(|error| error.to_string())?
}

/// Brings the running workspace window to the front, opening one if Terry is
/// alive but currently has no workspace window (macOS keeps the app running
/// after the last window closes).
async fn focus(cx: &mut AsyncApp) -> Result<Value, String> {
    match workspace_window(cx).await? {
        Some(window) => {
            window
                .update(cx, |_multi_workspace, window, _cx| {
                    window.activate_window();
                })
                .map_err(|error| error.to_string())?;
            cx.update(|cx| cx.activate(true));
        }
        None => {
            open_workspace_window(cx).await?;
        }
    }
    Ok(json!({ "status": "ok", "action": "focus" }))
}

async fn focus_terminal(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = match workspace_window(cx).await? {
        Some(window) => window,
        None => open_workspace_window(cx).await?,
    };

    window
        .update(cx, move |multi_workspace, window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                if let Some(id) = terminal_id {
                    let focused = panel.update(cx, |panel, cx| {
                        panel.focus_terminal_by_id(EntityId::from(id), window, cx)
                    });
                    if !focused {
                        return Err(format!("no terminal with id {id}"));
                    }
                }
                window.activate_window();
                Ok(json!({ "status": "ok", "action": "focus_session" }))
            })
        })
        .map_err(|error| error.to_string())?
}

async fn list_sessions(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, |multi_workspace, _window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                Ok(panel.read(cx).session_info(cx))
            })
        })
        .map_err(|error| error.to_string())?
}

fn resolve_terminal(
    panel: &gpui::Entity<TerminalListPanel>,
    terminal_id: Option<u64>,
    cx: &App,
) -> Result<gpui::Entity<TerminalView>, String> {
    match terminal_id {
        Some(id) => panel
            .read(cx)
            .terminal_view_by_id(EntityId::from(id))
            .ok_or_else(|| format!("no terminal with id {id}")),
        None => panel
            .read(cx)
            .active_terminal_view(cx)
            .ok_or_else(|| "no active terminal".to_string()),
    }
}

async fn send_keys(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    keys: &str,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;
    let keys = keys.to_string();

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                let view = resolve_terminal(&panel, terminal_id, cx)?;
                let option_as_meta = TerminalSettings::get_global(cx).option_as_meta;
                view.update(cx, move |view, cx| {
                    view.terminal().update(cx, |terminal, _cx| {
                        let handled = Keystroke::parse(&keys).ok().is_some_and(|keystroke| {
                            terminal.try_keystroke(&keystroke, option_as_meta)
                        });
                        if !handled {
                            terminal.input(keys.into_bytes());
                        }
                    });
                });
                Ok(json!({ "status": "ok", "action": "send_keys" }))
            })
        })
        .map_err(|error| error.to_string())?
}

async fn read_screen(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                let view = resolve_terminal(&panel, terminal_id, cx)?;
                let id = view.entity_id().as_u64();
                let content = view.read(cx).terminal().read(cx).get_content();
                Ok(json!({ "terminal_id": id, "content": content }))
            })
        })
        .map_err(|error| error.to_string())?
}

async fn get_context(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let workspace = multi_workspace.workspace().clone();
            workspace.update(cx, move |workspace, cx| {
                let panel = workspace
                    .panel::<TerminalListPanel>(cx)
                    .ok_or_else(|| "terminal panel is not initialized".to_string())?;
                panel
                    .read(cx)
                    .terminal_context(terminal_id.map(EntityId::from), cx)
                    .ok_or_else(|| "no active terminal".to_string())
            })
        })
        .map_err(|error| error.to_string())?
}

async fn notify(cx: &mut AsyncApp, title: Option<String>, message: &str) -> Result<Value, String> {
    let title = title.unwrap_or_else(|| "Terry".to_string());
    let body = message.to_string();
    cx.update(|cx| {
        cx.show_system_notification(SystemNotification {
            tag: "terry-ipc".into(),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        });
    });
    Ok(json!({ "status": "ok", "action": "notify" }))
}

/// Opens a fresh workspace window (used when Terry is running with zero
/// workspace windows) and returns its handle.
async fn open_workspace_window(cx: &mut AsyncApp) -> Result<WindowHandle<MultiWorkspace>, String> {
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
            .and_then(|handle| handle.downcast::<MultiWorkspace>())
            .ok_or_else(|| "new workspace window did not appear".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::RunningInstance;
    use std::path::PathBuf;

    #[test]
    fn describe_reports_the_instance_that_received_the_launch() {
        let instance = RunningInstance {
            pid: Some(42),
            port: Some(1234),
            executable: Some(PathBuf::from(
                "/Applications/Terry.app/Contents/MacOS/terry",
            )),
            arch: Some("aarch64".to_string()),
        };

        let message = instance.describe();
        assert!(message.contains("pid 42"), "{message}");
        assert!(
            message.contains("/Applications/Terry.app/Contents/MacOS/terry"),
            "{message}"
        );
        assert!(message.contains("aarch64"), "{message}");
        assert!(message.contains("1234"), "{message}");
        assert!(message.contains("--new-instance"), "{message}");
    }

    #[test]
    fn describe_still_explains_the_exit_without_details() {
        let instance = RunningInstance {
            pid: None,
            port: None,
            executable: None,
            arch: None,
        };

        let message = instance.describe();
        assert!(message.contains("已有 Terry 实例在运行"), "{message}");
        assert!(message.contains("--new-instance"), "{message}");
        assert!(!message.contains("IPC"), "{message}");
    }
}
