//! Opt-in persistent terminal sessions backed by `terminal_daemon`.
//!
//! When `terminal.persistent_sessions` is enabled, terminals are owned by a
//! background daemon instead of this process, so they survive quitting Terry.
//! The GUI keeps rendering locally: it attaches to the daemon, feeds the raw
//! bytes into its own emulator, and forwards input and resizes back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use collections::HashMap;
use gpui::App;
use settings::Settings as _;
use terminal::terminal_settings::TerminalSettings;
use terminal_daemon::{AttachedReader, AttachedWriter, Control, Event, SessionSpec, attach};

/// One daemon connection, shared by every window and workspace.
fn daemon_slot() -> &'static Mutex<Option<Control>> {
    static SLOT: OnceLock<Mutex<Option<Control>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Whether persistent sessions are enabled on this platform.
pub fn is_enabled(cx: &App) -> bool {
    cfg!(unix) && TerminalSettings::get_global(cx).persistent_sessions
}

/// A stable, process-independent session id.
pub fn new_session_id() -> String {
    format!("t-{}", uuid::Uuid::new_v4())
}

/// An attached session: the read half feeds the emulator, the write half
/// carries input and resizes back to the daemon.
pub struct AttachedSession {
    pub reader: AttachedReader,
    pub writer: Arc<AttachedWriter>,
}

/// Spawns (or reattaches to) a daemon session and attaches to it.
///
/// Blocking: call this from a background executor.
pub fn connect_and_spawn(spec: SessionSpec) -> anyhow::Result<AttachedSession> {
    with_control(|control| {
        let session = control.spawn(&spec)?;
        let (reader, writer) = attach(
            &terminal_daemon::socket_path(),
            &session,
            spec.cols,
            spec.rows,
        )?;
        Ok(AttachedSession {
            reader,
            writer: Arc::new(writer),
        })
    })
}

/// Ends a daemon session and its process.
pub fn kill(session: &str) -> anyhow::Result<()> {
    with_control(|control| control.kill(session))
}

/// Sessions currently owned by the daemon.
pub fn list() -> anyhow::Result<Vec<terminal_daemon::SessionInfo>> {
    with_control(|control| control.list())
}

/// Runs `f` against the shared control connection, starting the daemon on
/// first use and reconnecting once if the connection went stale.
fn with_control<R>(f: impl FnOnce(&mut Control) -> anyhow::Result<R>) -> anyhow::Result<R> {
    let mut slot = daemon_slot().lock().expect("daemon mutex poisoned");
    if slot.is_none() {
        *slot = Some(start_daemon()?);
    }
    let result = f(slot.as_mut().expect("daemon connection checked above"));
    if result.is_err() {
        // Force a reconnect on the next call.
        *slot = None;
    }
    result
}

fn start_daemon() -> anyhow::Result<Control> {
    let socket = terminal_daemon::socket_path();
    if let Ok(control) = Control::connect(&socket) {
        return Ok(control);
    }

    // The GUI binary hosts the daemon itself (`terry daemon-serve`), so no
    // second executable has to be shipped.
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("daemon-serve")
        .arg("--socket")
        .arg(&socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
        .spawn()
        .map_err(|error| anyhow::anyhow!("failed to start the terminal daemon: {error}"))?;

    for _ in 0..100 {
        if let Ok(control) = Control::connect(&socket) {
            return Ok(control);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("terminal daemon did not start at {}", socket.display())
}

/// Environment for a daemon-spawned shell.
///
/// Mirrors what `TerminalBuilder::new` prepares, because the daemon spawns the
/// shell and the GUI never sees it. Shell integration matters most: without
/// `TERRY_INTEGRATION_DIR`, prompt marks (OSC 133) and cwd tracking (OSC 7)
/// would not work in persistent terminals.
pub fn spawn_env(
    program: &str,
    extra: &HashMap<String, String>,
    cx: &App,
) -> HashMap<String, String> {
    let settings = TerminalSettings::get_global(cx);
    let version = release_channel::AppVersion::global(cx);

    let mut env = HashMap::default();
    terminal::insert_zed_terminal_env(&mut env, &version);
    for (key, value) in &settings.env {
        env.insert(key.clone(), value.clone());
    }
    for (key, value) in extra {
        env.insert(key.clone(), value.clone());
    }

    // The daemon falls back to `$SHELL`; pass it through so the integration
    // matches the shell that will actually run.
    if !env.contains_key("SHELL") {
        if let Some(shell) = std::env::var("SHELL").ok() {
            env.insert("SHELL".to_string(), shell);
        }
    }

    if settings.shell_integration {
        // Activation commands for PowerShell/Nushell are ignored for now.
        if let Err(error) =
            terminal::shell_integration::inject_shell_integration_env(&mut env, program)
        {
            log::warn!("persistent session: shell integration unavailable: {error}");
        }
    }

    env.remove("SHLVL");
    env
}

/// Re-exported so the panel does not need to depend on the daemon crate.

/// One-time notice shown when the first persistent terminal is detached.
pub fn should_notice_detach() -> bool {
    static NOTICED: AtomicBool = AtomicBool::new(false);
    !NOTICED.swap(true, Ordering::SeqCst)
}

/// Convenience wrapper for building a [`SessionSpec`].
pub fn session_spec(
    session: String,
    cwd: Option<PathBuf>,
    program: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> SessionSpec {
    SessionSpec {
        session,
        cwd,
        program,
        args,
        env: env.into_iter().collect(),
        cols: 80,
        rows: 24,
    }
}

/// Session ids referenced by any persisted Terry session file.
///
/// Walks the JSON generically so it keeps working if the session schema
/// changes: every `daemon_session` string anywhere in the file counts.
pub fn referenced_sessions() -> collections::HashSet<String> {
    let mut referenced = collections::HashSet::default();
    let directory = paths::data_dir().join("sessions");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return referenced;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        collect_daemon_sessions(&value, &mut referenced);
    }
    referenced
}

fn collect_daemon_sessions(
    value: &serde_json::Value,
    referenced: &mut collections::HashSet<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "daemon_session" {
                    if let Some(session) = value.as_str() {
                        referenced.insert(session.to_string());
                    }
                } else {
                    collect_daemon_sessions(value, referenced);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_daemon_sessions(item, referenced);
            }
        }
        _ => {}
    }
}

/// Kills daemon sessions that no session file references, that nobody is
/// attached to, and that have been idle for at least `min_idle_secs`.
///
/// The attached and idle guards keep a session alive while a window is still
/// restoring, or while an agent is thinking without producing output.
pub fn prune(
    referenced: &collections::HashSet<String>,
    min_idle_secs: u64,
    dry_run: bool,
) -> anyhow::Result<Vec<String>> {
    with_control(|control| {
        let sessions = control.list()?;
        let mut pruned = Vec::new();
        for info in sessions {
            if info.attached || info.idle_secs < min_idle_secs || referenced.contains(&info.session)
            {
                continue;
            }
            if !dry_run {
                control.kill(&info.session)?;
            }
            pruned.push(info.session);
        }
        Ok(pruned)
    })
}

/// True the first time it is called in this process, so the startup sweep runs
/// once even with several windows open.
pub fn claim_startup_prune() -> bool {
    static CLAIMED: AtomicBool = AtomicBool::new(false);
    !CLAIMED.swap(true, Ordering::SeqCst)
}

/// Read the next event from an attached session, ignoring daemon errors.
pub fn next_event(reader: &mut AttachedReader) -> Option<Event> {
    match reader.read_event() {
        Ok(event) => event,
        Err(error) => {
            log::debug!("persistent session: {error}");
            None
        }
    }
}

/// Feeds an attached daemon session into a display-only terminal: output goes
/// into the emulator, input and resizes go back to the daemon.
pub fn wire(
    terminal: &gpui::Entity<terminal::Terminal>,
    attached: AttachedSession,
    cx: &mut gpui::AsyncWindowContext,
) {
    let AttachedSession { mut reader, writer } = attached;

    terminal.update(cx, |terminal, _cx| {
        let input = writer.clone();
        terminal.set_input_sink(Some(Arc::new(move |bytes: &[u8]| {
            let _ = input.input(bytes);
        })));
        let resize = writer.clone();
        terminal.set_resize_sink(Some(Arc::new(move |columns: u16, rows: u16| {
            let _ = resize.resize(columns, rows);
        })));
    });

    // Reading blocks, so it runs on its own thread and hands bytes to the
    // GUI thread through a channel.
    let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Event>();
    std::thread::spawn(move || {
        while let Some(event) = next_event(&mut reader) {
            if sender.unbounded_send(event).is_err() {
                break;
            }
        }
    });

    let terminal = terminal.clone();
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        // A command that prints a lot reaches us as one daemon frame per line:
        // the PTY is line-buffered, so each write there becomes a read here.
        // Every `write_output` repaints the terminal and refreshes the sidebar
        // and tab bar, so handling them one by one makes burst output cost
        // ~1ms per line. Drain whatever is already queued into a single write
        // instead; the in-process PTY path coalesces the same way (it batches
        // events on a 4ms timer before touching the UI).
        let mut pending: Vec<u8> = Vec::new();
        let mut exit_code = None;
        while let Some(event) = receiver.next().await {
            match event {
                Event::Output(bytes) => pending.extend_from_slice(&bytes),
                Event::Exited(code) => exit_code = Some(code),
            }

            while let Ok(event) = receiver.try_recv() {
                match event {
                    Event::Output(bytes) => pending.extend_from_slice(&bytes),
                    Event::Exited(code) => {
                        exit_code = Some(code);
                        break;
                    }
                }
            }

            if !pending.is_empty() {
                terminal.update(cx, |terminal, cx| terminal.write_output(&pending, cx));
                pending.clear();
            }
            if let Some(code) = exit_code.take() {
                terminal.update(cx, |terminal, cx| terminal.mark_exited(code, cx));
                break;
            }
        }
    })
    .detach();
}
