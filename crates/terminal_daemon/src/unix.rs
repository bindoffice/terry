//! Unix implementation: a Unix-domain-socket daemon that owns PTYs.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{self, Pty, ToWinsize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use terminal_core::Emulator;
use tokio::sync::broadcast;

use crate::protocol::{
    DaemonInfo, Reply, Request, SessionInfo, TAG_INPUT, TAG_JSON, TAG_OUTPUT, read_frame,
    write_frame, write_json,
};
use crate::{Event, SessionSpec};

/// Output kept for replay when a client reattaches.
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
/// Read size from the PTY.
const READ_CHUNK: usize = 64 * 1024;
/// Environment variable the agent integrations read to identify their terminal.
///
/// Mirrors what the GUI injects, so `assets/agent-integrations/*` scripts work
/// unchanged against either host.
const AGENT_TOKEN_ENV: &str = "TERRY_TERMINAL_TOKEN";

/// Scrollback retained by each session's emulator.
///
/// The daemon never shows a scrollback: it only reads the screen tail for agent
/// detection and plain-text previews. Keeping this small bounds the memory a
/// long-lived session costs, since one emulator is alive per session.
const EMULATOR_SCROLLBACK_LINES: usize = 1_000;
/// Frame size used when replaying the buffer.
const REPLAY_CHUNK: usize = 64 * 1024;

type Registry = Arc<Mutex<HashMap<String, Arc<Session>>>>;

/// Shared state the socket server, the HTTP endpoint and the reaper all need.
struct DaemonState {
    registry: Registry,
    /// Agent identity and reported state per session.
    ///
    /// Lives outside `Registry` because it must exist even when the HTTP
    /// endpoint is off: the reaper reads it to avoid killing a session that is
    /// running an agent, and that reasoning has nothing to do with HTTP.
    agents: Arc<crate::agents::AgentRegistry>,
    /// Where this daemon listens, so a `Shutdown` handler can wake the
    /// blocked accept loop.
    socket: PathBuf,
    /// The optional HTTP endpoint, reported to clients that want to restart
    /// the daemon with the same options.
    http: Option<String>,
    token_file: Option<PathBuf>,
    /// Set by `Request::Shutdown`; the accept loop checks it after every
    /// accepted connection.
    shutdown: AtomicBool,
}

impl DaemonState {
    fn new(socket: PathBuf, options: &ServeOptions) -> Self {
        Self {
            registry: Arc::default(),
            agents: Arc::new(crate::agents::AgentRegistry::new()),
            socket,
            http: options.http.clone(),
            token_file: options.token_file.clone(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Currently live sessions, sorted by id and annotated with their agent.
    fn list(&self) -> Vec<SessionInfo> {
        let mut sessions: Vec<(SessionInfo, Arc<Session>)> = {
            let registry = self.registry.lock().expect("registry mutex poisoned");
            registry
                .values()
                .map(|session| (session.info(), session.clone()))
                .collect()
        };
        for (info, session) in &mut sessions {
            info.agent = self
                .agents
                .reported_for(&info.session)
                .and_then(|reported| reported.agent)
                .or_else(|| session.foreground_agent().map(str::to_string));
        }
        let mut sessions: Vec<SessionInfo> = sessions.into_iter().map(|(info, _)| info).collect();
        sessions.sort_by(|a, b| a.session.cmp(&b.session));
        sessions
    }

    /// Binds the terminal token a session was spawned with.
    fn bind_agent_token(&self, session: &str, token: &str) {
        self.agents.bind_session(session, token);
    }
}

enum SessionEvent {
    Output(Arc<[u8]>),
    Exited(i32),
}

struct Shared {
    buffer: VecDeque<u8>,
    subscribers: Vec<(u64, Sender<SessionEvent>)>,
    next_subscriber: u64,
    size: (u16, u16),
    exited: Option<i32>,
    /// Last time output was produced, for idle-based reaping.
    last_activity: std::time::Instant,
}

struct Session {
    id: String,
    /// Kept alive for as long as the session exists: dropping it hangs up the
    /// child and waits for it.
    pty: Mutex<Option<Pty>>,
    /// Clone of the PTY master used for input and resizes.
    writer: Mutex<Option<File>>,
    shared: Mutex<Shared>,
    /// VT state for this session.
    ///
    /// Kept here rather than rebuilt on demand: a client asking for the screen
    /// or for an agent state would otherwise have to replay the whole output
    /// buffer, which is up to `MAX_BUFFER_BYTES`. Its own lock so a slow reader
    /// never blocks the output fan-out.
    emulator: Mutex<Emulator>,
}

impl Session {
    fn info(&self) -> SessionInfo {
        let shared = self.shared.lock().expect("session mutex poisoned");
        SessionInfo {
            session: self.id.clone(),
            cols: shared.size.0,
            rows: shared.size.1,
            exited: shared.exited,
            buffered_bytes: shared.buffer.len(),
            attached: !shared.subscribers.is_empty(),
            idle_secs: shared.last_activity.elapsed().as_secs(),
            // Filled in by the registry, which owns the agent bookkeeping.
            agent: None,
        }
    }

    fn has_exited(&self) -> bool {
        self.shared
            .lock()
            .expect("session mutex poisoned")
            .exited
            .is_some()
    }

    /// The visible screen, for clients that repaint it outside the app.
    fn screen_snapshot(&self) -> terminal_core::ScreenSnapshot {
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .screen_snapshot()
    }

    /// The terminal (OSC) title.
    fn title(&self) -> String {
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .title()
    }

    /// Sends bytes to the PTY. Used by `POST /api/send_text`.
    fn send_text(&self, text: &str) -> Result<(), String> {
        if self.has_exited() {
            return Err("session has exited".to_string());
        }
        // A trailing newline is what "press enter" means over HTTP; the GUI's
        // `send_text` does the same for a phone typing a message.
        let normalized = terminal_core::keys::encode_text(text);
        if normalized.is_empty() {
            return Ok(());
        }
        // `write_input` swallows write errors; surface the one that matters.
        if self.writer.lock().expect("writer mutex poisoned").is_none() {
            return Err("session is not writable".to_string());
        }
        self.write_input(&normalized);
        Ok(())
    }

    /// Modes a client needs to encode keys against this session.
    fn key_modes(&self) -> terminal_core::keys::KeyModes {
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .key_modes()
    }

    /// Writes pre-encoded bytes to the PTY.
    fn write_bytes(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.write_input(bytes);
        }
    }

    /// The last `count` non-empty lines, oldest first.
    fn tail_lines(&self, count: usize) -> Vec<String> {
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .tail_lines(count)
    }

    /// The coding agent in the PTY's foreground process group, if the daemon
    /// can inspect it.
    ///
    /// Daemon-owned PTYs have no other process-level view: the GUI only receives
    /// bytes and a terminal token, and integrations are not installed for every
    /// agent. Reading the PTY's foreground group here makes direct launches of
    /// Codex, Pi and the rest of the catalogue visible just like local PTYs.
    fn foreground_agent(&self) -> Option<&'static str> {
        let pgid = {
            let writer = self.writer.lock().expect("writer mutex poisoned");
            let file = writer.as_ref()?;
            let pgid = unsafe { libc::tcgetpgrp(file.as_raw_fd()) };
            // Zero means no foreground group has been assigned yet; negative is
            // an error. Neither identifies a process we can inspect.
            (pgid > 0).then_some(pgid as u32)?
        };

        let pid = Pid::from_u32(pgid);
        let mut system = System::new();
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .without_tasks();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh_kind);
        let process = system.process(pid)?;
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .filter_map(|argument| argument.to_str().map(ToOwned::to_owned))
            .collect();

        agent_catalog::agent_for_argv(&argv)
            .or_else(|| {
                process
                    .name()
                    .to_str()
                    .and_then(agent_catalog::agent_for_basename)
            })
            .map(|agent| agent.id)
    }

    fn publish(&self, data: &[u8]) {
        // Feed the emulator before fanning out: a client that reads the screen
        // after receiving these bytes must already see them.
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .advance(data);

        let mut shared = self.shared.lock().expect("session mutex poisoned");
        shared.buffer.extend(data.iter().copied());
        shared.last_activity = std::time::Instant::now();
        while shared.buffer.len() > MAX_BUFFER_BYTES {
            shared.buffer.pop_front();
        }
        let payload: Arc<[u8]> = Arc::from(data);
        shared.subscribers.retain(|(_, subscriber)| {
            subscriber
                .send(SessionEvent::Output(payload.clone()))
                .is_ok()
        });
    }

    fn finish(&self, code: i32) {
        let mut shared = self.shared.lock().expect("session mutex poisoned");
        shared.exited = Some(code);
        for (_, subscriber) in shared.subscribers.drain(..) {
            let _ = subscriber.send(SessionEvent::Exited(code));
        }
    }

    fn write_input(&self, data: &[u8]) {
        let mut writer = self.writer.lock().expect("writer mutex poisoned");
        if let Some(file) = writer.as_mut() {
            let _ = file.write_all(data);
            let _ = file.flush();
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        {
            let mut shared = self.shared.lock().expect("session mutex poisoned");
            shared.size = (cols, rows);
        }
        // Both grids must agree: a snapshot whose size disagrees with the PTY
        // misplaces every cell after the first reflow.
        self.emulator
            .lock()
            .expect("emulator mutex poisoned")
            .resize(cols as usize, rows as usize);
        let writer = self.writer.lock().expect("writer mutex poisoned");
        if let Some(file) = writer.as_ref() {
            apply_resize(file, cols, rows);
        }
    }

    fn kill(&self) {
        let pty = self.pty.lock().expect("pty mutex poisoned").take();
        if let Some(pty) = pty {
            // The reader thread reaps the child when it exits. Signalling an
            // already-reaped pid could hit a recycled process, so only ask a
            // live session to hang up.
            if !self.has_exited() {
                let pid = pty.child().id() as i32;
                unsafe {
                    libc::kill(pid, libc::SIGHUP);
                }
            }
            // `Pty::drop` waits for the child. Keep that off the caller's
            // thread so a child that ignores SIGHUP cannot block the daemon.
            thread::spawn(move || drop(pty));
        }
        *self.writer.lock().expect("writer mutex poisoned") = None;
    }

    /// Registers a subscriber and snapshots the replay buffer atomically, so no
    /// output can be lost between the snapshot and the live stream.
    fn subscribe(
        &self,
    ) -> (
        u64,
        Vec<u8>,
        Option<i32>,
        (u16, u16),
        Receiver<SessionEvent>,
    ) {
        let (sender, receiver) = mpsc::channel();
        let mut shared = self.shared.lock().expect("session mutex poisoned");
        let id = shared.next_subscriber;
        shared.next_subscriber += 1;
        let replay: Vec<u8> = shared.buffer.iter().copied().collect();
        let exited = shared.exited;
        let size = shared.size;
        if exited.is_none() {
            shared.subscribers.push((id, sender));
        }
        (id, replay, exited, size, receiver)
    }

    /// Drops every attached client, leaving the process running.
    fn detach_all(&self) {
        self.shared
            .lock()
            .expect("session mutex poisoned")
            .subscribers
            .clear();
    }

    fn unsubscribe(&self, id: u64) {
        self.shared
            .lock()
            .expect("session mutex poisoned")
            .subscribers
            .retain(|(subscriber, _)| *subscriber != id);
    }
}

fn apply_resize(file: &File, cols: u16, rows: u16) {
    let window_size = WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 0,
        cell_height: 0,
    };
    let size = window_size.to_winsize();
    unsafe {
        libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &size);
    }
}

fn set_blocking(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
    }
}

fn wait_for_exit(pid: i32) -> i32 {
    let mut status: libc::c_int = 0;
    loop {
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            if libc::WIFEXITED(status) {
                return libc::WEXITSTATUS(status);
            }
            if libc::WIFSIGNALED(status) {
                return 128 + libc::WTERMSIG(status);
            }
            return -1;
        }
        if result < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return -1;
        }
    }
}

fn spawn_reader(session: Arc<Session>, mut reader: File, pid: i32) {
    thread::spawn(move || {
        set_blocking(reader.as_raw_fd());
        let mut buffer = vec![0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => session.publish(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let code = wait_for_exit(pid);
        session.finish(code);
    });
}

fn spawn_session(
    registry: &Registry,
    session: String,
    cwd: Option<String>,
    program: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cols: u16,
    rows: u16,
) -> anyhow::Result<String> {
    if let Some(existing) = registry
        .lock()
        .expect("registry mutex poisoned")
        .get(&session)
    {
        if !existing.has_exited() {
            return Ok(session);
        }
    }

    let program = program
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());
    let options = tty::Options {
        shell: Some(tty::Shell::new(program, args)),
        working_directory: cwd.map(PathBuf::from),
        drain_on_exit: true,
        env: env.into_iter().collect(),
        child_signal_mask: None,
    };

    let cols = cols.max(1);
    let rows = rows.max(1);
    let window_size = WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 0,
        cell_height: 0,
    };
    let pty = tty::new(&options, window_size, 0)?;
    let pid = pty.child().id() as i32;
    let reader = pty.file().try_clone()?;
    let writer = pty.file().try_clone()?;

    let state = Arc::new(Session {
        id: session.clone(),
        pty: Mutex::new(Some(pty)),
        writer: Mutex::new(Some(writer)),
        emulator: Mutex::new(Emulator::new(
            cols as usize,
            rows as usize,
            EMULATOR_SCROLLBACK_LINES,
        )),
        shared: Mutex::new(Shared {
            buffer: VecDeque::new(),
            subscribers: Vec::new(),
            next_subscriber: 0,
            size: (cols, rows),
            exited: None,
            last_activity: std::time::Instant::now(),
        }),
    });

    registry
        .lock()
        .expect("registry mutex poisoned")
        .insert(session.clone(), state.clone());
    spawn_reader(state, reader, pid);
    Ok(session)
}

/// Reads a session's screen without attaching to it.
///
/// `lines: Some(n)` is the cheap path: plain text for a preview or for agent
/// detection. `None` returns the styled viewport, encoded with the same helper
/// the GUI's `/api/screen` uses.
fn read_screen(registry: &Registry, id: &str, lines: Option<usize>) -> Reply {
    let session = registry
        .lock()
        .expect("registry mutex poisoned")
        .get(id)
        .cloned();
    let Some(session) = session else {
        return Reply::Error {
            message: format!("unknown session {id}"),
        };
    };

    match lines {
        Some(count) => Reply::Text {
            lines: session.tail_lines(count.max(1)),
        },
        None => {
            let snapshot = session.screen_snapshot();
            // The daemon has no theme, so it reports the stock palette. A
            // client that wants the desktop theme gets it from the GUI host.
            let palette = terminal_core::palette::Palette::default();
            // Session ids are `t-<uuid>`; the numeric id in the payload is the
            // GUI's `EntityId`, which the daemon does not have. Clients talking
            // to the daemon address sessions by string, so 0 is honest here.
            let screen = terminal_core::screen_json::screen_payload(0, &snapshot, &palette);
            Reply::Screen { screen }
        }
    }
}

fn list_sessions(state: &DaemonState) -> Vec<SessionInfo> {
    state.list()
}

fn info_reply(state: &DaemonState) -> DaemonInfo {
    DaemonInfo {
        version: crate::version().to_string(),
        build_id: crate::BUILD_ID.to_string(),
        pid: std::process::id(),
        http: state.http.clone(),
        token_file: state
            .token_file
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

fn kill_session(registry: &Registry, id: &str) -> anyhow::Result<()> {
    let removed = registry.lock().expect("registry mutex poisoned").remove(id);
    let Some(session) = removed else {
        anyhow::bail!("unknown session {id}");
    };
    session.kill();
    Ok(())
}

/// Extra listeners to start alongside the Unix socket.
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// `host:port` to serve HTTP on. `None` keeps the daemon socket-only, which
    /// is the default: the HTTP endpoint can spawn arbitrary programs.
    pub http: Option<String>,
    /// Where the bearer token lives.
    ///
    /// A path rather than the value: the endpoint re-reads it per request, so
    /// rotating the token revokes access on a running daemon instead of at the
    /// next restart. Required when `http` is set.
    pub token_file: Option<std::path::PathBuf>,
}

impl ServeOptions {
    /// Options for a daemon that also answers HTTP requests, reading its token
    /// from `token_file`.
    pub fn with_http(bind: impl Into<String>, token_file: impl Into<std::path::PathBuf>) -> Self {
        Self {
            http: Some(bind.into()),
            token_file: Some(token_file.into()),
        }
    }
}

/// Bridges the daemon's registry to the HTTP layer.
struct HttpSessions {
    state: Arc<DaemonState>,
    events: broadcast::Sender<crate::http::SessionEvent>,
    /// Base URL handed to spawned sessions so their agent integrations can post
    /// state back to this daemon.
    http_url: Option<String>,
}

impl HttpSessions {
    fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.state
            .registry
            .lock()
            .expect("registry mutex poisoned")
            .get(id)
            .cloned()
    }

    /// Announces a change to SSE subscribers.
    fn publish(&self, kind: &'static str, session: &str, payload: serde_json::Value) {
        let _ = self.events.send(crate::http::SessionEvent {
            kind,
            session: session.to_string(),
            payload,
        });
    }
}

impl crate::http::Sessions for HttpSessions {
    fn list(&self) -> Vec<crate::http::SessionSummary> {
        let entries: Vec<(SessionInfo, Arc<Session>)> = {
            let registry = self.state.registry.lock().expect("registry mutex poisoned");
            let mut entries: Vec<_> = registry
                .iter()
                .map(|(_, session)| (session.info(), session.clone()))
                .collect();
            entries.sort_by(|a, b| a.0.session.cmp(&b.0.session));
            entries
        };

        entries
            .into_iter()
            .map(|(info, session)| {
                let title = session.title();
                let lines = session.tail_lines(agent_detect::SCREEN_LINES);
                let reported = self.state.agents.reported_for(&info.session);
                let process_agent = session.foreground_agent();
                let agent_hint = reported
                    .as_ref()
                    .and_then(|reported| reported.agent.as_deref())
                    .or(process_agent);
                let application_cursor = session.key_modes().application_cursor;
                crate::http::summarize(
                    &info,
                    title,
                    None,
                    reported.as_ref(),
                    agent_hint,
                    &lines,
                    application_cursor,
                )
            })
            .collect()
    }

    fn screen(&self, session: &str) -> Option<serde_json::Value> {
        let session = self.session(session)?;
        let snapshot = session.screen_snapshot();
        let palette = terminal_core::palette::Palette::default();
        Some(terminal_core::screen_json::screen_payload(
            0, &snapshot, &palette,
        ))
    }

    fn read_screen(&self, session: &str, count: usize) -> Option<Vec<String>> {
        Some(self.session(session)?.tail_lines(count))
    }

    fn send_text(&self, session: &str, text: &str) -> Result<(), String> {
        let session = self
            .session(session)
            .ok_or_else(|| "unknown session".to_string())?;
        session.send_text(text)
    }

    fn send_keys(&self, session: &str, keys: &[String]) -> Result<(), String> {
        let session = self
            .session(session)
            .ok_or_else(|| "unknown session".to_string())?;
        let modes = session.key_modes();
        let mut bytes = Vec::new();
        for key in keys {
            match terminal_core::keys::encode(key, modes) {
                Some(encoded) => bytes.extend_from_slice(&encoded),
                None => return Err(format!("unknown key {key:?}")),
            }
        }
        session.write_bytes(&bytes);
        Ok(())
    }

    fn key_modes(&self, session: &str) -> Option<terminal_core::keys::KeyModes> {
        Some(self.session(session)?.key_modes())
    }

    fn spawn(&self, spec: SessionSpec) -> Result<String, String> {
        let id = spec.session.clone();
        let mut env = spec.env.clone();

        // Bind a terminal token before spawning, and hand it to the child. The
        // agent integration scripts read `TERRY_TERMINAL_TOKEN` and send it back
        // with every report, which is how a session in the daemon learns which
        // agent is running in it: the daemon has no `/proc` view of the PTY's
        // foreground process the way the GUI does.
        let terminal_token = uuid::Uuid::new_v4().to_string();
        env.retain(|(key, _)| key != AGENT_TOKEN_ENV);
        env.push((AGENT_TOKEN_ENV.to_string(), terminal_token.clone()));
        if let Some(http) = self.http_url.as_deref() {
            env.retain(|(key, _)| key != "TERRY_IPC_URL");
            env.push(("TERRY_IPC_URL".to_string(), http.to_string()));

            // The hook scripts guard on all three variables and exit silently
            // when any is missing, so a session without this one reports
            // nothing at all - a failure that looks like "the agent has no
            // state" rather than "the environment is wrong".
            //
            // The per-terminal token doubles as the endpoint token: each
            // script already proves its identity with `TERRY_TERMINAL_TOKEN`,
            // and a distinct shared secret per terminal would buy nothing while
            // making the scripts need a second variable.
            env.retain(|(key, _)| key != "TERRY_IPC_TOKEN");
            env.push(("TERRY_IPC_TOKEN".to_string(), terminal_token.clone()));
        }

        spawn_session(
            &self.state.registry,
            spec.session,
            spec.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            spec.program,
            spec.args,
            env,
            spec.cols,
            spec.rows,
        )
        .map_err(|error| error.to_string())?;

        self.state.bind_agent_token(&id, &terminal_token);
        self.publish("terminal_added", &id, serde_json::json!({ "session": id }));
        Ok(id)
    }

    fn close(&self, session: &str) -> Result<(), String> {
        kill_session(&self.state.registry, session).map_err(|error| error.to_string())?;
        self.state.agents.forget_session(session);
        self.publish(
            "terminal_removed",
            session,
            serde_json::json!({ "session": session }),
        );
        Ok(())
    }

    fn detach(&self, session: &str) -> Result<(), String> {
        // Detach means "stop watching, keep the process", which is exactly the
        // process-internal behaviour of dropping every subscriber.
        let session = self
            .session(session)
            .ok_or_else(|| "unknown session".to_string())?;
        session.detach_all();
        Ok(())
    }

    fn report_agent_state(
        &self,
        token: &str,
        agent: Option<String>,
        state: &str,
        message: Option<String>,
    ) -> Result<(), String> {
        self.state.agents.report(token, agent, state, message)?;
        if let Some(session) = self.session_for_token(token) {
            self.publish(
                "agent_state",
                &session,
                serde_json::json!({ "state": state }),
            );
        }
        Ok(())
    }

    fn report_agent_session(&self, token: &str, agent: &str) -> Result<(), String> {
        // Announcing the agent must not overwrite the state its hook already
        // reported; only the identity is new here.
        self.state.agents.set_agent(token, agent)?;
        if let Some(session) = self.session_for_token(token) {
            self.publish(
                "agent_state",
                &session,
                serde_json::json!({ "agent": agent }),
            );
        }
        Ok(())
    }
}

impl HttpSessions {
    /// The session a terminal token was bound to.
    fn session_for_token(&self, token: &str) -> Option<String> {
        self.state.agents.session_for_token(token)
    }
}

/// Runs the daemon until the process is killed.
pub fn serve(socket: &Path) -> anyhow::Result<()> {
    serve_with_options(socket, ServeOptions::default())
}

/// Runs the daemon, optionally with an HTTP listener for companion clients.
///
/// Blocks until the process is killed. When `options.http` is set and the port
/// was `0`, `on_http_port` is called with the port the OS actually chose; that
/// is how a test (or a supervisor printing a URL) learns where to connect.
pub fn serve_with_options(socket: &Path, options: ServeOptions) -> anyhow::Result<()> {
    serve_with_options_and_port(socket, options, |_| {})
}

/// [`serve_with_options`], reporting the bound HTTP port before serving.
pub fn serve_with_options_and_port(
    socket: &Path,
    options: ServeOptions,
    on_http_port: impl FnOnce(u16),
) -> anyhow::Result<()> {
    if UnixStream::connect(socket).is_ok() {
        anyhow::bail!(
            "a terminal daemon is already listening at {}",
            socket.display()
        );
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    log::info!("terminal daemon listening on {}", socket.display());

    let state = Arc::new(DaemonState::new(socket.to_path_buf(), &options));

    // The HTTP listener is opt-in: it can spawn arbitrary programs, so it must
    // be something the user asked for, not a side effect of starting the daemon.
    if let Some(bind) = options.http.as_deref() {
        let token_file = options
            .token_file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--http requires --token-file"))?;
        if !token_file.is_file() {
            anyhow::bail!(
                "the token file {} does not exist; create one with `terry daemon rotate-token --token-file {}`",
                token_file.display(),
                token_file.display()
            );
        }
        let token = crate::http::AuthToken::from_file(token_file);
        let (events, _) = broadcast::channel(256);
        let sessions = Arc::new(HttpSessions {
            state: state.clone(),
            events: events.clone(),
            http_url: Some(format!("http://{bind}")),
        });
        let server = crate::http::serve(bind, token, sessions, events.subscribe())
            .map_err(|error| anyhow::anyhow!(error))?;
        log::info!(
            "terminal daemon http listening on {}:{}",
            bind,
            server.port()
        );
        on_http_port(server.port());
        // Kept for the lifetime of the process; dropping it would shut the
        // listener down.
        std::mem::forget(server);
    }

    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) => {
                log::warn!("terminal daemon accept failed: {error}");
                continue;
            }
        };
        // `Shutdown` wakes this loop by connecting to the socket itself. Once
        // the flag is visible, drop that connection and stop listening.
        if state.shutdown.load(Ordering::SeqCst) {
            break;
        }
        // On some platforms an accepted socket inherits the listener's flags.
        let _ = stream.set_nonblocking(false);
        let state = state.clone();
        thread::spawn(move || {
            if let Err(error) = handle_connection(stream, state) {
                log::debug!("terminal daemon connection closed: {error}");
            }
        });
    }

    // Leave no socket behind: clients detect a stopped daemon by `connect`
    // failing, and the next daemon binds a fresh file.
    let _ = std::fs::remove_file(socket);
    Ok(())
}

fn handle_connection(stream: UnixStream, state: Arc<DaemonState>) -> anyhow::Result<()> {
    let registry = state.registry.clone();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    loop {
        let (tag, payload) = read_frame(&mut reader)?;
        if tag != TAG_JSON {
            continue;
        }
        let request: Request = serde_json::from_slice(&payload)?;
        match request {
            Request::Spawn {
                session,
                cwd,
                program,
                args,
                env,
                cols,
                rows,
            } => {
                let reply =
                    match spawn_session(&registry, session, cwd, program, args, env, cols, rows) {
                        Ok(session) => Reply::Ok {
                            session: Some(session),
                        },
                        Err(error) => Reply::Error {
                            message: error.to_string(),
                        },
                    };
                write_json(&mut writer, &reply)?;
            }
            Request::List => {
                write_json(
                    &mut writer,
                    &Reply::Sessions {
                        sessions: list_sessions(&state),
                    },
                )?;
            }
            Request::Kill { session } => {
                let reply = match kill_session(&registry, &session) {
                    Ok(()) => Reply::Ok {
                        session: Some(session),
                    },
                    Err(error) => Reply::Error {
                        message: error.to_string(),
                    },
                };
                write_json(&mut writer, &reply)?;
            }
            Request::Attach {
                session,
                cols,
                rows,
                readonly,
            } => {
                return attach_connection(reader, writer, registry, session, cols, rows, readonly);
            }
            Request::ReadScreen { session, lines } => {
                let reply = read_screen(&registry, &session, lines);
                write_json(&mut writer, &reply)?;
            }
            Request::Info => {
                write_json(&mut writer, &Reply::Info(info_reply(&state)))?;
            }
            Request::Shutdown => {
                // Answer first: the caller may wait for this before starting
                // the replacement daemon.
                write_json(&mut writer, &Reply::Info(info_reply(&state)))?;

                // End the sessions explicitly. A `serve` embedded in another
                // process (tests, `terry daemon serve` in the foreground) keeps
                // running after this function returns, and the caller asked
                // for the sessions to be gone.
                let sessions: Vec<String> = state
                    .registry
                    .lock()
                    .expect("registry mutex poisoned")
                    .keys()
                    .cloned()
                    .collect();
                for session in sessions {
                    let _ = kill_session(&registry, &session);
                }

                state.shutdown.store(true, Ordering::SeqCst);
                // Wake the accept loop, which is blocked in `accept`.
                let _ = UnixStream::connect(&state.socket);
                return Ok(());
            }
            Request::Resize { .. } | Request::Detach => {
                write_json(
                    &mut writer,
                    &Reply::Error {
                        message: "not attached to a session".to_string(),
                    },
                )?;
            }
        }
    }
}

fn attach_connection(
    mut reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    registry: Registry,
    session_id: String,
    cols: u16,
    rows: u16,
    readonly: bool,
) -> anyhow::Result<()> {
    let session = registry
        .lock()
        .expect("registry mutex poisoned")
        .get(&session_id)
        .cloned();
    let Some(session) = session else {
        write_json(
            &mut writer,
            &Reply::Error {
                message: format!("unknown session {session_id}"),
            },
        )?;
        return Ok(());
    };

    // A client that does not know the real grid size yet (a reattach at startup,
    // before the view has been laid out) passes 0 and lets the view push the
    // true size through `Resize`. Resizing here would clobber a session that is
    // still running at its previous size.
    //
    // A read-only attachment never resizes at all: it renders the session at
    // whatever size the writing client chose, so two viewers cannot reflow each
    // other's program.
    if !readonly && cols > 0 && rows > 0 {
        session.resize(cols, rows);
    }
    let (subscriber_id, replay, exited, size, events) = session.subscribe();

    let result = (|| -> anyhow::Result<()> {
        write_json(
            &mut writer,
            &Reply::Attached {
                session: session_id.clone(),
                cols: size.0,
                rows: size.1,
                replayed_bytes: replay.len(),
            },
        )?;
        for chunk in replay.chunks(REPLAY_CHUNK) {
            write_frame(&mut writer, TAG_OUTPUT, chunk)?;
        }
        if let Some(code) = exited {
            write_json(&mut writer, &Reply::Exited { code })?;
            return Ok(());
        }

        let mut event_writer = writer.try_clone()?;
        let event_thread = thread::spawn(move || {
            while let Ok(event) = events.recv() {
                match event {
                    SessionEvent::Output(data) => {
                        if write_frame(&mut event_writer, TAG_OUTPUT, &data).is_err() {
                            break;
                        }
                    }
                    SessionEvent::Exited(code) => {
                        let _ = write_json(&mut event_writer, &Reply::Exited { code });
                        // The session is gone, so nothing else will ever be sent.
                        // Shut the connection down so the client's blocking
                        // reader sees EOF and ends, instead of waiting forever
                        // for frames that will never arrive.
                        let _ = event_writer.shutdown(Shutdown::Both);
                        break;
                    }
                }
            }
        });

        let read_result = loop {
            match read_frame(&mut reader) {
                Ok((TAG_INPUT, payload)) => session.write_input(&payload),
                Ok((TAG_JSON, payload)) => match serde_json::from_slice::<Request>(&payload) {
                    // A read-only attachment ignores resizes rather than
                    // erroring: a client that renders at the session's size may
                    // still send one while it is figuring that out.
                    Ok(Request::Resize { cols, rows }) => {
                        if !readonly {
                            session.resize(cols, rows);
                        }
                    }
                    Ok(Request::Detach) => break Ok(()),
                    Ok(_) => {}
                    Err(error) => break Err(error.into()),
                },
                Ok(_) => {}
                Err(error) => break Err(error.into()),
            }
        };

        // Dropping the subscriber ends the event thread.
        session.unsubscribe(subscriber_id);
        let _ = event_thread.join();
        read_result
    })();

    session.unsubscribe(subscriber_id);
    result
}

/// Starts the daemon if it is not already listening.
pub fn ensure_daemon(socket: &Path) -> anyhow::Result<()> {
    if UnixStream::connect(socket).is_ok() {
        return Ok(());
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    let exe = std::env::current_exe()?;
    let daemon = exe.with_file_name("terry-daemon");
    let program = if daemon.exists() { daemon } else { exe };

    let mut command = std::process::Command::new(program);
    command.arg("serve").arg("--socket").arg(socket);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()?;

    for _ in 0..100 {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("terminal daemon did not start at {}", socket.display())
}

/// Whether a daemon is listening on `socket`.
pub fn is_daemon_running(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// Waits until nothing is listening on `socket`.
pub fn wait_until_stopped(socket: &Path, timeout: Duration) -> bool {
    wait_for_socket(socket, timeout, false)
}

/// Waits until something is listening on `socket`.
pub fn wait_until_running(socket: &Path, timeout: Duration) -> bool {
    wait_for_socket(socket, timeout, true)
}

fn wait_for_socket(socket: &Path, timeout: Duration, running: bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_daemon_running(socket) == running {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Signals daemons that are too old to understand `Shutdown`.
///
/// Such a daemon cannot identify itself either, so the only way to retire it is
/// to find the process serving this socket by its command line. Both the daemon
/// entry point and the exact socket path must match, so an unrelated process
/// that merely mentions the path is left alone.
pub fn terminate_daemon(socket: &Path) -> anyhow::Result<Vec<u32>> {
    let socket_arg = socket.display().to_string();
    let mut system = System::new();
    // `refresh_processes` refreshes no command lines at all, and the argv is
    // the only way to recognize a daemon this old.
    let refresh_kind = ProcessRefreshKind::nothing()
        .with_cmd(UpdateKind::Always)
        .without_tasks();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    let mut signalled = Vec::new();
    for (pid, process) in system.processes() {
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .filter_map(|argument| argument.to_str().map(ToOwned::to_owned))
            .collect();
        if !argv.iter().any(|argument| argument == &socket_arg) {
            continue;
        }
        let gui_host = argv.iter().any(|argument| argument == "daemon-serve");
        let standalone = argv
            .first()
            .is_some_and(|exe| exe.ends_with("terry-daemon"))
            && argv.iter().any(|argument| argument == "serve");
        if !gui_host && !standalone {
            continue;
        }

        let pid = pid.as_u32();
        if pid == std::process::id() {
            continue;
        }
        // The daemon installs no signal handlers; SIGTERM is enough.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        signalled.push(pid);
    }
    Ok(signalled)
}

/// Control connection: spawn, list, kill.
pub struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Control {
    pub fn connect(socket: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            reader,
            writer: stream,
        })
    }

    fn request(&mut self, request: &Request) -> anyhow::Result<Reply> {
        write_json(&mut self.writer, request)?;
        loop {
            let (tag, payload) = read_frame(&mut self.reader)?;
            if tag == TAG_JSON {
                return Ok(serde_json::from_slice(&payload)?);
            }
        }
    }

    /// Starts a session (or reattaches when the id is already alive).
    pub fn spawn(&mut self, spec: &SessionSpec) -> anyhow::Result<String> {
        let reply = self.request(&Request::Spawn {
            session: spec.session.clone(),
            cwd: spec.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            program: spec.program.clone(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            cols: spec.cols,
            rows: spec.rows,
        })?;
        match reply {
            Reply::Ok { session } => Ok(session.unwrap_or_else(|| spec.session.clone())),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    pub fn list(&mut self) -> anyhow::Result<Vec<SessionInfo>> {
        match self.request(&Request::List)? {
            Reply::Sessions { sessions } => Ok(sessions),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    pub fn kill(&mut self, session: &str) -> anyhow::Result<()> {
        match self.request(&Request::Kill {
            session: session.to_string(),
        })? {
            Reply::Ok { .. } => Ok(()),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    /// The last `count` non-empty lines of a session, oldest first.
    ///
    /// Does not attach: the session keeps its size and its input stream, so a
    /// client can poll this while another client (the desktop GUI) is showing
    /// the same terminal.
    pub fn read_screen(&mut self, session: &str, count: usize) -> anyhow::Result<Vec<String>> {
        match self.request(&Request::ReadScreen {
            session: session.to_string(),
            lines: Some(count),
        })? {
            Reply::Text { lines } => Ok(lines),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    /// The styled viewport of a session, encoded like `/api/screen`.
    pub fn screen(&mut self, session: &str) -> anyhow::Result<serde_json::Value> {
        match self.request(&Request::ReadScreen {
            session: session.to_string(),
            lines: None,
        })? {
            Reply::Screen { screen } => Ok(screen),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    /// The daemon's build identity.
    ///
    /// A daemon older than the handshake cannot parse this request; it drops
    /// the connection, so callers should treat the error as "unknown build"
    /// rather than as a broken daemon.
    pub fn info(&mut self) -> anyhow::Result<DaemonInfo> {
        match self.request(&Request::Info)? {
            Reply::Info(info) => Ok(info),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }

    /// Asks the daemon to stop and end every session it owns.
    ///
    /// Returns the identity it reported on the way out, so a caller can say
    /// which build it just retired.
    pub fn shutdown(&mut self) -> anyhow::Result<DaemonInfo> {
        match self.request(&Request::Shutdown)? {
            Reply::Info(info) => Ok(info),
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }
}

/// Write half of an attached session: input, resize, detach.
pub struct AttachedWriter {
    writer: Mutex<UnixStream>,
}

impl AttachedWriter {
    pub fn input(&self, data: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().expect("attach writer poisoned");
        write_frame(&mut *writer, TAG_INPUT, data)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let mut writer = self.writer.lock().expect("attach writer poisoned");
        write_json(&mut *writer, &Request::Resize { cols, rows })
    }

    pub fn detach(&self) -> io::Result<()> {
        let mut writer = self.writer.lock().expect("attach writer poisoned");
        write_json(&mut *writer, &Request::Detach)
    }
}

/// Read half of an attached session.
pub struct AttachedReader {
    reader: BufReader<UnixStream>,
    session: String,
    replayed_bytes: usize,
}

impl AttachedReader {
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Bytes of replay the daemon sent before live output. Used to drop the
    /// oldest part when the emulator cannot keep that much scrollback.
    pub fn replayed_bytes(&self) -> usize {
        self.replayed_bytes
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)
    }

    /// Blocks for the next event. `Ok(None)` means the connection closed.
    pub fn read_event(&mut self) -> anyhow::Result<Option<Event>> {
        loop {
            match read_frame(&mut self.reader) {
                Ok((TAG_OUTPUT, payload)) => return Ok(Some(Event::Output(payload))),
                Ok((TAG_JSON, payload)) => match serde_json::from_slice::<Reply>(&payload)? {
                    Reply::Exited { code } => return Ok(Some(Event::Exited(code))),
                    Reply::Error { message } => anyhow::bail!(message),
                    _ => continue,
                },
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// Attaches to a session, returning its read and write halves.
pub fn attach(
    socket: &Path,
    session: &str,
    cols: u16,
    rows: u16,
) -> anyhow::Result<(AttachedReader, AttachedWriter)> {
    attach_with_options(socket, session, cols, rows, false)
}

/// Attaches without ever resizing the session.
///
/// Use this for a viewer that is not the primary display — a phone following a
/// session the desktop is driving. The `Attached` reply carries the session's
/// real size, so the client can render at that size instead of imposing its own.
pub fn attach_readonly(
    socket: &Path,
    session: &str,
) -> anyhow::Result<(AttachedReader, AttachedWriter)> {
    attach_with_options(socket, session, 0, 0, true)
}

fn attach_with_options(
    socket: &Path,
    session: &str,
    cols: u16,
    rows: u16,
    readonly: bool,
) -> anyhow::Result<(AttachedReader, AttachedWriter)> {
    let stream = UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    write_json(
        &mut writer,
        &Request::Attach {
            session: session.to_string(),
            cols,
            rows,
            readonly,
        },
    )?;

    loop {
        let (tag, payload) = read_frame(&mut reader)?;
        if tag != TAG_JSON {
            continue;
        }
        match serde_json::from_slice::<Reply>(&payload)? {
            Reply::Attached {
                session,
                replayed_bytes,
                ..
            } => {
                return Ok((
                    AttachedReader {
                        reader,
                        session,
                        replayed_bytes,
                    },
                    AttachedWriter {
                        writer: Mutex::new(writer),
                    },
                ));
            }
            Reply::Error { message } => anyhow::bail!(message),
            other => anyhow::bail!("unexpected reply: {other:?}"),
        }
    }
}
