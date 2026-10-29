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
    AnyWindowHandle, App, AsyncApp, EntityId, Keystroke, SystemNotification, WindowHandle,
    WindowId, px,
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
        Some("send-text") => Some(match parse_targeted_text_args(&args[1..], "send-text") {
            Ok((terminal_id, text)) => post_command(
                "/api/send_text",
                &json!({ "terminal_id": terminal_id, "text": text }).to_string(),
            ),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        }),
        Some("run") => Some(match parse_targeted_text_args(&args[1..], "run") {
            Ok((terminal_id, text)) => post_command(
                "/api/run",
                &json!({ "terminal_id": terminal_id, "text": text }).to_string(),
            ),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        }),
        Some("session") => Some(run_session_cli(&args[1..])),
        Some("daemon") => Some(run_daemon_cli(&args[1..])),
        Some("agent-integration") => Some(run_agent_integration_cli(&args[1..])),
        Some("daemon-serve") => Some(run_daemon_serve(&args[1..])),
        Some("agents") => Some(post_command("/api/list_agent_states", "{}")),
        Some("wait") => Some(run_wait_cli(&args[1..], false)),
        Some("wait-output") => Some(run_wait_cli(&args[1..], true)),
        Some("events") => Some(stream_events()),
        Some("context") => Some(match parse_context_args(&args[1..]) {
            Ok(terminal_id) => post_command(
                "/api/get_context",
                &json!({ "terminal_id": terminal_id }).to_string(),
            ),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        }),
        Some("kitten" | "plugin") => Some(crate::kittens::run_cli_subcommand(&args[1..])),
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

fn parse_targeted_text_args(
    args: &[String],
    command: &str,
) -> Result<(Option<u64>, String), String> {
    let mut terminal_id = None;
    let mut text = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            text.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg == "--terminal" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--terminal 需要一个数字终端 id".to_string())?;
            terminal_id = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| "--terminal 需要一个数字终端 id".to_string())?,
            );
        } else if let Some(value) = arg.strip_prefix("--terminal=") {
            terminal_id = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| "--terminal 需要一个数字终端 id".to_string())?,
            );
        } else {
            text.push(arg.clone());
        }
        index += 1;
    }

    if text.is_empty() {
        return Err(format!(
            "{command} 需要文本参数，例如 terry {command} \"ls -la\""
        ));
    }
    Ok((terminal_id, text.join(" ")))
}

fn parse_context_args(args: &[String]) -> Result<Option<u64>, String> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--terminal" => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "--terminal 需要一个数字终端 id".to_string()),
        [arg] => arg
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("context 未知参数: {arg}")),
        _ => Err("context 用法: terry context [--terminal ID]".to_string()),
    }
}

/// `terry agent-integration install|uninstall|list [AGENT]`.
fn run_agent_integration_cli(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        eprintln!("用法: terry agent-integration install|uninstall|list [AGENT]");
        return 2;
    };

    match command {
        "list" => {
            println!("{}", crate::agent_integrations::SUPPORTED.join("\n"));
            0
        }
        "install" | "uninstall" => {
            let Some(agent) = args.get(1).map(String::as_str) else {
                eprintln!(
                    "{command} 需要一个 agent 名称（{}）",
                    crate::agent_integrations::SUPPORTED.join(", ")
                );
                return 2;
            };
            let result = if command == "install" {
                crate::agent_integrations::install(agent)
            } else {
                crate::agent_integrations::uninstall(agent)
            };
            match result {
                Ok(path) => {
                    println!("{command}ed {}", path.display());
                    if command == "install" {
                        let snippet = match agent {
                            "claude" => Some((
                                "~/.claude/settings.json",
                                crate::agent_integrations::claude_settings_snippet(&path),
                            )),
                            "kimi" => Some((
                                "~/.kimi-code/config.toml",
                                crate::agent_integrations::kimi_settings_snippet(&path),
                            )),
                            "qwen" => Some((
                                "~/.qwen/settings.json",
                                crate::agent_integrations::qwen_settings_snippet(&path),
                            )),
                            "codex" => Some((
                                "~/.codex/hooks.json",
                                crate::agent_integrations::codex_hooks_snippet(&path),
                            )),
                            _ => None,
                        };
                        if let Some((file, snippet)) = snippet {
                            println!("\n把下面的内容合并到 {file}：\n{snippet}");
                        }
                        if agent == "codex" {
                            println!(
                                "\n并在 ~/.codex/config.toml 里启用 hooks：\n{}",
                                crate::agent_integrations::codex_config_snippet()
                            );
                        }
                        println!("重启 {agent} 后生效。");
                    }
                    0
                }
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        other => {
            eprintln!("未知子命令: {other}");
            2
        }
    }
}

/// `terry daemon list|kill|serve`.
fn run_daemon_cli(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        eprintln!(
            "用法: terry daemon list | kill SESSION | prune [--dry-run] [--min-idle-secs N] | serve [--socket PATH]"
        );
        return 2;
    };

    match command {
        "list" => {
            let sessions = match connect_daemon() {
                Ok(mut control) => match control.list() {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        eprintln!("terry daemon: {error}");
                        return 1;
                    }
                },
                Err(message) => {
                    eprintln!("{message}");
                    return 1;
                }
            };
            match serde_json::to_string_pretty(&sessions) {
                Ok(json) => {
                    println!("{json}");
                    0
                }
                Err(error) => {
                    eprintln!("terry daemon: {error}");
                    1
                }
            }
        }
        "kill" => {
            let Some(session) = args.get(1) else {
                eprintln!("kill 需要一个 session id（用 `terry daemon list` 查看）");
                return 2;
            };
            let mut control = match connect_daemon() {
                Ok(control) => control,
                Err(message) => {
                    eprintln!("{message}");
                    return 1;
                }
            };
            match control.kill(session) {
                Ok(()) => {
                    println!("killed {session}");
                    0
                }
                Err(error) => {
                    eprintln!("terry daemon: {error}");
                    1
                }
            }
        }
        "serve" => run_daemon_serve(&args[1..]),
        "prune" => {
            let dry_run = args.iter().any(|arg| arg == "--dry-run");
            let min_idle_secs = args
                .iter()
                .position(|arg| arg == "--min-idle-secs")
                .and_then(|index| args.get(index + 1))
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(3600);
            let referenced = crate::persistent_sessions::referenced_sessions();
            match crate::persistent_sessions::prune(&referenced, min_idle_secs, dry_run) {
                Ok(pruned) if pruned.is_empty() => {
                    println!("没有可清理的会话");
                    0
                }
                Ok(pruned) => {
                    for session in pruned {
                        println!(
                            "{} {session}",
                            if dry_run { "would kill" } else { "killed" }
                        );
                    }
                    0
                }
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        other => {
            eprintln!("未知 daemon 子命令: {other}");
            eprintln!(
                "用法: terry daemon list | kill SESSION | prune [--dry-run] [--min-idle-secs N] | serve [--socket PATH]"
            );
            2
        }
    }
}

fn connect_daemon() -> Result<terminal_daemon::Control, String> {
    let socket = terminal_daemon::socket_path();
    terminal_daemon::Control::connect(&socket).map_err(|error| {
        format!(
            "无法连接终端守护进程（{}）：{error}\n开启「设置 → 终端 → 持久会话」后会自动启动。",
            socket.display()
        )
    })
}

/// Runs the terminal daemon in this process. Hidden: the GUI starts it
/// automatically when persistent sessions are enabled.
fn run_daemon_serve(args: &[String]) -> i32 {
    let mut socket = terminal_daemon::socket_path();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--socket" {
            if let Some(value) = args.get(index + 1) {
                socket = PathBuf::from(value);
                index += 1;
            }
        } else if let Some(value) = args[index].strip_prefix("--socket=") {
            socket = PathBuf::from(value);
        }
        index += 1;
    }

    match terminal_daemon::serve(&socket) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("terry daemon: {error}");
            1
        }
    }
}

fn run_session_cli(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        eprintln!("用法: terry session list | save NAME | load NAME | delete NAME");
        return 2;
    };

    let path = match command {
        "list" => "/api/list_session_templates",
        "save" => "/api/save_session_template",
        "load" => "/api/load_session_template",
        "delete" => "/api/delete_session_template",
        "help" | "--help" | "-h" => {
            print_cli_usage();
            return 0;
        }
        other => {
            eprintln!("未知 session 子命令: {other}");
            return 2;
        }
    };

    if command == "list" {
        return post_command(path, "{}");
    }

    let name = args[1..].join(" ");
    if name.trim().is_empty() {
        eprintln!("terry session {command} 需要一个名称");
        return 2;
    }
    post_command(path, &json!({ "name": name }).to_string())
}

/// Waits for an agent state, or for terminal output matching a substring or
/// regex.
///
/// `require_matcher` is used by the `wait-output` alias, which must not fall
/// back to waiting on agent state.
fn run_wait_cli(args: &[String], require_matcher: bool) -> i32 {
    let mut state = "blocked".to_string();
    let mut state_given = false;
    let mut terminal_id: Option<u64> = None;
    let mut timeout_seconds: Option<u64> = None;
    let mut substring: Option<String> = None;
    let mut regex: Option<String> = None;
    let mut tail_lines: Option<u64> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--state" => match next_cli_value(args, &mut index, "--state") {
                Some(value) => {
                    state = value.to_ascii_lowercase();
                    state_given = true;
                }
                None => return 2,
            },
            "--match-output" | "--match" => {
                match next_cli_value(args, &mut index, "--match-output") {
                    Some(value) => substring = Some(value.to_string()),
                    None => return 2,
                }
            }
            "--match-regex" | "--regex" => {
                match next_cli_value(args, &mut index, "--match-regex") {
                    Some(value) => regex = Some(value.to_string()),
                    None => return 2,
                }
            }
            "--terminal" => {
                match next_cli_value(args, &mut index, "--terminal")
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    Some(value) => terminal_id = Some(value),
                    None => {
                        eprintln!("--terminal 需要一个数字终端 id");
                        return 2;
                    }
                }
            }
            "--timeout" => {
                match next_cli_value(args, &mut index, "--timeout")
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    Some(value) => timeout_seconds = Some(value),
                    None => {
                        eprintln!("--timeout 需要秒数（0 表示不限时）");
                        return 2;
                    }
                }
            }
            "--tail" => {
                match next_cli_value(args, &mut index, "--tail")
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    Some(value) => tail_lines = Some(value),
                    None => {
                        eprintln!("--tail 需要行数（0 表示整个缓冲区）");
                        return 2;
                    }
                }
            }
            other => {
                eprintln!("wait 未知参数: {other}");
                return 2;
            }
        }
        index += 1;
    }

    if substring.is_some() && regex.is_some() {
        eprintln!("--match-output 与 --match-regex 只能选一个");
        return 2;
    }

    let has_matcher = substring.is_some() || regex.is_some();
    if require_matcher && !has_matcher {
        eprintln!("wait-output 需要 --match-output 或 --match-regex");
        return 2;
    }
    if has_matcher && state_given {
        eprintln!("--state 不能与 --match-output/--match-regex 同时使用");
        return 2;
    }

    let timeout_ms = timeout_seconds.map(|seconds| seconds.saturating_mul(1000));
    if has_matcher {
        return post_wait(
            "/api/wait_for_output",
            json!({
                "terminal_id": terminal_id,
                "match": substring,
                "regex": regex,
                "timeout_ms": timeout_ms,
                "tail_lines": tail_lines,
            }),
            timeout_ms,
        );
    }

    post_wait(
        "/api/wait_agent_state",
        json!({
            "state": state,
            "terminal_id": terminal_id,
            "timeout_ms": timeout_ms,
        }),
        timeout_ms,
    )
}

/// Returns the value following a CLI flag, advancing `index` past it.
fn next_cli_value<'a>(args: &'a [String], index: &mut usize, name: &str) -> Option<&'a str> {
    *index += 1;
    match args.get(*index) {
        Some(value) => Some(value.as_str()),
        None => {
            eprintln!("{name} 需要一个值");
            None
        }
    }
}

/// Posts a request whose server-side handler may block for a while.
fn post_wait(path: &str, body: Value, timeout_ms: Option<u64>) -> i32 {
    let Some(info) = read_ipc_info() else {
        eprintln!(
            "Terry 未运行或 IPC 未启用（无法读取 {}）",
            ipc_info_path().display()
        );
        return 1;
    };

    // The client must outlive the server-side wait, which polls the GUI.
    let server_timeout_ms = timeout_ms.unwrap_or(120_000);
    let read_timeout = if server_timeout_ms == 0 {
        Duration::from_secs(86_400)
    } else {
        Duration::from_millis(server_timeout_ms) + Duration::from_secs(10)
    };

    match http_post_with_timeout(
        info.port,
        path,
        &info.token,
        &body.to_string(),
        Duration::from_secs(5),
        Duration::from_secs(5),
        read_timeout,
    ) {
        Ok((status, response)) => {
            println!("{}", response.trim());
            if (200..300).contains(&status) { 0 } else { 1 }
        }
        Err(error) => {
            eprintln!("请求失败: {error}");
            1
        }
    }
}

/// Streams `/api/events` (server-sent events) until interrupted.
fn stream_events() -> i32 {
    let Some(info) = read_ipc_info() else {
        eprintln!(
            "Terry 未运行或 IPC 未启用（无法读取 {}）",
            ipc_info_path().display()
        );
        return 1;
    };

    let address = SocketAddr::from(([127, 0, 0, 1], info.port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("无法连接 Terry IPC: {error}");
            return 1;
        }
    };

    let request = format!(
        "GET /api/events HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Authorization: Bearer {}\r\n\
         Accept: text/event-stream\r\n\
         Connection: close\r\n\
         \r\n",
        info.port, info.token
    );
    if let Err(error) = stream.write_all(request.as_bytes()) {
        eprintln!("无法发送请求: {error}");
        return 1;
    }
    let _ = stream.flush();

    let reader = std::io::BufReader::new(stream);
    let mut first_line = true;
    for line in std::io::BufRead::lines(reader) {
        let Ok(line) = line else {
            break;
        };
        let line = line.trim_end_matches('\r');

        if first_line {
            first_line = false;
            if !line.contains(" 200 ") {
                eprintln!("events 请求失败: {line}");
                return 1;
            }
            continue;
        }

        // Keep SSE fields and comments; drop headers and chunk framing.
        if line.starts_with("event:") || line.starts_with("data:") || line.starts_with(':') {
            println!("{line}");
            let _ = std::io::stdout().flush();
        }
    }
    0
}

fn print_cli_usage() {
    println!("用法: terry [子命令]");
    println!();
    println!("不带子命令时启动 Terry 图形界面。");
    println!();
    println!("子命令:");
    println!("  new-tab [--cwd PATH]  在已运行的 Terry 实例中打开一个新的终端标签页");
    println!("  send-text [--terminal ID] TEXT");
    println!("                        向指定终端（默认活动终端）粘贴文本（不执行）");
    println!("  run [--terminal ID] TEXT");
    println!("                        向指定终端粘贴文本并回车执行");
    println!("  send-text -- TEXT     以 -- 分隔，原样发送后续文本");
    println!("  session list          列出已保存的 session 模板");
    println!("  session save NAME     把当前分组保存为 session 模板");
    println!("  session load NAME     从模板创建一个新的分组");
    println!("  session delete NAME   删除 session 模板");
    println!("  agents                列出正在运行的 agent 终端及其状态（JSON）");
    println!(
        "  wait [--state S | --match-output TEXT | --match-regex PAT] [--terminal ID] [--timeout 秒] [--tail 行]"
    );
    println!(
        "                        阻塞直到 agent 进入指定状态或终端最近的输出匹配（默认 --state blocked）"
    );
    println!(
        "  wait-output           等同于 wait --match-output/--match-regex（--tail 0 表示整个缓冲区）"
    );
    println!("  events                订阅 agent 状态变化（SSE，Ctrl-C 退出）");
    println!("  daemon list           列出持久会话（JSON）");
    println!("  daemon kill SESSION   结束一个持久会话及其进程");
    println!("  daemon prune [--dry-run]  清理不再被任何会话文件引用的孤儿会话");
    println!("  agent-integration install|uninstall|list [AGENT]");
    println!("                        安装/卸载 agent 状态上报集成（目前支持 pi）");
    println!("  context [--terminal ID] 输出终端的标题、cwd、命令与 agent 状态（JSON）");
    println!("  kitten list           列出可用的 kitten 插件");
    println!("  kitten run NAME       运行 kitten 插件（也可直接 kitten NAME）");
    println!("  kitten path           显示 kitten 搜索目录");
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
                IpcRequestKind::SendText { terminal_id, text } => {
                    send_text(cx, terminal_id, &text).await
                }
                IpcRequestKind::Focus => focus(cx).await,
                IpcRequestKind::FocusTerminal { terminal_id } => {
                    focus_terminal(cx, terminal_id).await
                }
                IpcRequestKind::ListSessions => list_sessions(cx).await,
                IpcRequestKind::SendKeys { terminal_id, keys } => {
                    send_keys(cx, terminal_id, &keys).await
                }
                IpcRequestKind::ReadScreen { terminal_id, lines } => {
                    read_screen(cx, terminal_id, lines).await
                }
                IpcRequestKind::GetContext { terminal_id } => get_context(cx, terminal_id).await,
                IpcRequestKind::Notify { title, message } => notify(cx, title, &message).await,
                IpcRequestKind::CloseSession { terminal_id } => {
                    close_session(cx, terminal_id).await
                }
                IpcRequestKind::DetachSession { terminal_id } => {
                    detach_session(cx, terminal_id).await
                }
                IpcRequestKind::ReattachSession { terminal_id } => {
                    reattach_session(cx, terminal_id).await
                }
                IpcRequestKind::ListDetached => list_detached(cx).await,
                IpcRequestKind::RenameSession { terminal_id, title } => {
                    rename_session(cx, terminal_id, title).await
                }
                IpcRequestKind::FocusGroup { group_id } => focus_group(cx, group_id).await,
                IpcRequestKind::NewGroup { name, cwd } => new_group(cx, name, cwd).await,
                IpcRequestKind::DeleteGroup { group_id } => delete_group(cx, group_id).await,
                IpcRequestKind::RenameGroup { group_id, name } => {
                    rename_group(cx, group_id, name).await
                }
                IpcRequestKind::MoveSession {
                    terminal_id,
                    group_id,
                } => move_session(cx, terminal_id, group_id).await,
                IpcRequestKind::SetGroupLayout { group_id, layout } => {
                    set_group_layout(cx, group_id, layout).await
                }
                IpcRequestKind::SetBroadcast { group_id, enabled } => {
                    set_broadcast(cx, group_id, enabled).await
                }
                IpcRequestKind::Scroll { terminal_id, to } => scroll(cx, terminal_id, to).await,
                IpcRequestKind::SetFontSize { terminal_id, size } => {
                    set_font_size(cx, terminal_id, size).await
                }
                IpcRequestKind::SetColorScheme { terminal_id, name } => {
                    set_color_scheme(cx, terminal_id, name).await
                }
                IpcRequestKind::Run {
                    terminal_id,
                    text,
                    enter,
                } => run_command(cx, terminal_id, text, enter).await,
                IpcRequestKind::NewWindow => new_window(cx).await,
                IpcRequestKind::ListSessionTemplates => list_session_templates(cx).await,
                IpcRequestKind::SaveSessionTemplate { name } => {
                    save_session_template(cx, name).await
                }
                IpcRequestKind::LoadSessionTemplate { name } => {
                    load_session_template(cx, name).await
                }
                IpcRequestKind::DeleteSessionTemplate { name } => {
                    delete_session_template(cx, name).await
                }
                IpcRequestKind::ListAgentStates => list_agent_states(cx).await,
                IpcRequestKind::ReportAgentState {
                    token,
                    state,
                    message,
                    agent,
                } => report_agent_state(cx, token, state, message, agent).await,
                IpcRequestKind::ReportAgentSession {
                    token,
                    agent,
                    session_path,
                    session_id,
                } => report_agent_session(cx, token, agent, session_path, session_id).await,
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
            let panel = multi_workspace
                .workspace()
                .read(cx)
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
                multi_workspace
                    .workspace()
                    .read(cx)
                    .panel::<TerminalListPanel>(cx)
                    .map(|panel| {
                        let panel = panel.read(cx);
                        (panel.terminal_count(), panel.focused_terminal_id(cx))
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

async fn send_text(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    text: &str,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id().as_u64();
            view.update(cx, |view, cx| {
                view.terminal()
                    .update(cx, |terminal, _cx| terminal.paste(text));
            });
            Ok(json!({
                "status": "ok",
                "action": "send_text",
                "terminal_id": id,
            }))
        })
        .map_err(|error| error.to_string())?
}

/// Brings the running workspace window to the front, opening one if Terry is
/// alive but currently has no workspace window (macOS keeps the app running
/// after the last window closes).
async fn close_session(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let id = match terminal_id {
                Some(id) => EntityId::from(id),
                None => resolve_terminal(&panel, None, cx)?.entity_id(),
            };
            let closed = panel.update(cx, |panel, cx| panel.close_session_by_id(id, window, cx));
            if !closed {
                return Err(format!("no terminal with id {}", id.as_u64()));
            }
            Ok(json!({
                "status": "ok",
                "action": "close_session",
                "terminal_id": id.as_u64(),
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn detach_session(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let id = match terminal_id {
                Some(id) => EntityId::from(id),
                None => resolve_terminal(&panel, None, cx)?.entity_id(),
            };
            let detached =
                panel.update(cx, |panel, cx| panel.detach_terminal_by_id(id, window, cx));
            if !detached {
                return Err(format!("no terminal with id {}", id.as_u64()));
            }
            Ok(json!({
                "status": "ok",
                "action": "detach_session",
                "terminal_id": id.as_u64(),
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn reattach_session(cx: &mut AsyncApp, terminal_id: u64) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let attached = panel.update(cx, |panel, cx| {
                panel.reattach_terminal_by_id(EntityId::from(terminal_id), window, cx)
            });
            if !attached {
                return Err(format!("no detached terminal with id {terminal_id}"));
            }
            Ok(json!({
                "status": "ok",
                "action": "reattach_session",
                "terminal_id": terminal_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn list_detached(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            Ok(panel.read(cx).detached_sessions_json(cx))
        })
        .map_err(|error| error.to_string())?
}

async fn rename_session(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    title: Option<String>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id();
            view.update(cx, move |view, cx| view.set_custom_title(title, cx));
            Ok(json!({
                "status": "ok",
                "action": "rename_session",
                "terminal_id": id.as_u64(),
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn focus_group(cx: &mut AsyncApp, group_id: u64) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let focused = panel.update(cx, |panel, cx| {
                panel.focus_group_by_id(group_id, window, cx)
            });
            if !focused {
                return Err(format!("no group with id {group_id}"));
            }
            window.activate_window();
            Ok(json!({
                "status": "ok",
                "action": "focus_group",
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn new_group(
    cx: &mut AsyncApp,
    name: Option<String>,
    cwd: Option<PathBuf>,
) -> Result<Value, String> {
    if let Some(cwd) = &cwd {
        if !cwd.is_dir() {
            return Err(format!("cwd 不存在: {}", cwd.display()));
        }
    }

    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let group_id = panel.update(cx, |panel, cx| {
                panel.create_group_from_api(name, cwd, window, cx)
            });
            window.activate_window();
            Ok(json!({
                "status": "ok",
                "action": "new_group",
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn delete_group(cx: &mut AsyncApp, group_id: u64) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| {
                panel.delete_group_by_id(group_id, window, cx)
            })?;
            Ok(json!({
                "status": "ok",
                "action": "delete_group",
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn rename_group(cx: &mut AsyncApp, group_id: u64, name: String) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let renamed =
                panel.update(cx, |panel, cx| panel.rename_group_by_id(group_id, name, cx));
            if !renamed {
                return Err(format!("no group with id {group_id}"));
            }
            Ok(json!({
                "status": "ok",
                "action": "rename_group",
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn move_session(cx: &mut AsyncApp, terminal_id: u64, group_id: u64) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let moved = panel.update(cx, |panel, cx| {
                panel.move_terminal_to_group_by_id(
                    EntityId::from(terminal_id),
                    group_id,
                    window,
                    cx,
                )
            });
            if !moved {
                return Err(format!("no terminal {terminal_id} or group {group_id}"));
            }
            Ok(json!({
                "status": "ok",
                "action": "move_session",
                "terminal_id": terminal_id,
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn set_group_layout(
    cx: &mut AsyncApp,
    group_id: u64,
    layout: String,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| {
                panel.set_group_layout_by_id(group_id, &layout, window, cx)
            })?;
            Ok(json!({
                "status": "ok",
                "action": "set_group_layout",
                "group_id": group_id,
                "layout": layout,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn set_broadcast(
    cx: &mut AsyncApp,
    group_id: Option<u64>,
    enabled: bool,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let affected = panel.update(cx, |panel, cx| {
                panel.set_broadcast_input_by_id(group_id, enabled, cx)
            })?;
            Ok(json!({
                "status": "ok",
                "action": "set_broadcast",
                "group_id": affected,
                "enabled": enabled,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn scroll(cx: &mut AsyncApp, terminal_id: Option<u64>, to: String) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id();
            let terminal = view.read(cx).terminal().clone();
            let applied = terminal.update(cx, |terminal, _| match to.as_str() {
                "top" => {
                    terminal.scroll_to_top();
                    true
                }
                "bottom" => {
                    terminal.scroll_to_bottom();
                    true
                }
                "page_up" => {
                    terminal.scroll_page_up();
                    true
                }
                "page_down" => {
                    terminal.scroll_page_down();
                    true
                }
                "line_up" => {
                    terminal.scroll_line_up();
                    true
                }
                "line_down" => {
                    terminal.scroll_line_down();
                    true
                }
                _ => false,
            });
            if !applied {
                return Err(format!("unknown scroll target {to:?}"));
            }
            Ok(json!({
                "status": "ok",
                "action": "scroll",
                "terminal_id": id.as_u64(),
                "to": to,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn set_font_size(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    size: Option<f32>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id();
            view.update(cx, move |view, cx| {
                view.set_font_size_override(size.map(px), cx);
            });
            Ok(json!({
                "status": "ok",
                "action": "set_font_size",
                "terminal_id": id.as_u64(),
                "size": size,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn set_color_scheme(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    name: Option<String>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id();
            let display_name = name.clone();
            view.update(cx, move |view, cx| view.set_color_scheme(name, cx));
            Ok(json!({
                "status": "ok",
                "action": "set_color_scheme",
                "terminal_id": id.as_u64(),
                "name": display_name,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn run_command(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    text: String,
    enter: bool,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            // Read (do not update) the workspace: `resolve_terminal` reads it
            // back, and gpui aborts on a re-entrant read inside a live update.
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id();
            let terminal = view.read(cx).terminal().clone();
            terminal.update(cx, move |terminal, _| {
                terminal.paste(&text);
                if enter {
                    terminal.input(b"\r".to_vec());
                }
            });
            Ok(json!({
                "status": "ok",
                "action": "run",
                "terminal_id": id.as_u64(),
                "enter": enter,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn list_session_templates(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            Ok(panel.read(cx).list_session_templates())
        })
        .map_err(|error| error.to_string())?
}

async fn save_session_template(cx: &mut AsyncApp, name: String) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;
    let saved_name = name.clone();

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| panel.save_session_template(name, cx))?;
            Ok(json!({
                "status": "ok",
                "action": "save_session_template",
                "name": saved_name,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn load_session_template(cx: &mut AsyncApp, name: String) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let group_id = panel.update(cx, |panel, cx| {
                panel.load_session_template(&name, window, cx)
            })?;
            window.activate_window();
            Ok(json!({
                "status": "ok",
                "action": "load_session_template",
                "name": name,
                "group_id": group_id,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn delete_session_template(cx: &mut AsyncApp, name: String) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;
    let deleted_name = name.clone();

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, _cx| panel.delete_session_template(&name))?;
            Ok(json!({
                "status": "ok",
                "action": "delete_session_template",
                "name": deleted_name,
            }))
        })
        .map_err(|error| error.to_string())?
}

async fn new_window(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = open_workspace_window(cx).await?;
    let window_id = window.window_id().as_u64();
    window
        .update(cx, |_multi_workspace, window, _cx| {
            window.activate_window();
        })
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "status": "ok",
        "action": "new_window",
        "window_id": window_id,
    }))
}

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
            let panel = multi_workspace
                .workspace()
                .read(cx)
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
        .map_err(|error| error.to_string())?
}

async fn list_sessions(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            Ok(panel.read(cx).session_info(cx))
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
            .view_by_id_any(EntityId::from(id))
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
            let panel = multi_workspace
                .workspace()
                .read(cx)
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
        .map_err(|error| error.to_string())?
}

async fn read_screen(
    cx: &mut AsyncApp,
    terminal_id: Option<u64>,
    lines: Option<u32>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            // Read rather than update the workspace: panel helpers read it back
            // for the active pane, and gpui panics on a re-entrant read inside a
            // live update.
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            let view = resolve_terminal(&panel, terminal_id, cx)?;
            let id = view.entity_id().as_u64();
            let terminal = view.read(cx).terminal().read(cx);
            let content = match lines {
                Some(lines) if lines > 0 => {
                    terminal.last_n_non_empty_lines(lines as usize).join("\n")
                }
                _ => terminal.get_content(),
            };
            Ok(json!({ "terminal_id": id, "content": content }))
        })
        .map_err(|error| error.to_string())?
}

async fn report_agent_state(
    cx: &mut AsyncApp,
    token: String,
    state: String,
    message: Option<String>,
    agent: Option<String>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| {
                panel.apply_reported_agent_state(&token, &state, message, agent.as_deref(), cx)
            })
        })
        .map_err(|error| error.to_string())?
}

async fn report_agent_session(
    cx: &mut AsyncApp,
    token: String,
    agent: String,
    session_path: Option<String>,
    session_id: Option<String>,
) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel.update(cx, |panel, cx| {
                panel.apply_reported_agent_session(&token, &agent, session_path, session_id, cx)
            })
        })
        .map_err(|error| error.to_string())?
}

/// Environment that lets an in-terminal integration reach Terry's IPC.
pub(crate) fn connection_env() -> Vec<(String, String)> {
    match read_ipc_info() {
        Some(info) => vec![
            (
                "TERRY_IPC_URL".to_string(),
                format!("http://127.0.0.1:{}", info.port),
            ),
            ("TERRY_IPC_TOKEN".to_string(), info.token),
        ],
        None => Vec::new(),
    }
}

async fn list_agent_states(cx: &mut AsyncApp) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            Ok(panel.read(cx).agent_states_json(cx))
        })
        .map_err(|error| error.to_string())?
}

async fn get_context(cx: &mut AsyncApp, terminal_id: Option<u64>) -> Result<Value, String> {
    let window = workspace_window(cx)
        .await?
        .ok_or_else(|| "no Terry window is open".to_string())?;

    window
        .update(cx, move |multi_workspace, _window, cx| {
            // See `read_screen`: panel helpers read the workspace back, so this
            // must not run inside `workspace.update`.
            let panel = multi_workspace
                .workspace()
                .read(cx)
                .panel::<TerminalListPanel>(cx)
                .ok_or_else(|| "terminal panel is not initialized".to_string())?;
            panel
                .read(cx)
                .terminal_context(terminal_id.map(EntityId::from), cx)
                .ok_or_else(|| "no active terminal".to_string())
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
