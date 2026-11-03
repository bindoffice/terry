//! Long-lived interactive shell sessions, the `session_id` half of codex's
//! `exec_command`.
//!
//! tom's `bash` tool ran every command to completion or to its deadline. A
//! build that takes longer than the model is willing to wait had exactly two
//! outcomes: the output got truncated when the deadline fired, or the model
//! re-ran the whole command in the next turn and paid for the same output
//! twice. Codex's `exec_command` + `write_stdin` pair breaks that shape — run
//! for `yield_time_ms`, hand back a `session_id`, and let the model poll or
//! type into the same process.
//!
//! This module owns the process side of that. It is deliberately **not** part
//! of `ExecutionEnv`: a session outlives a single `Shell::exec` call, so it
//! cannot be expressed as a one-shot method. The tool keeps the registry; the
//! env keeps its one-shot `exec`.
//!
//! ## What is a PTY and what is a pipe
//!
//! A real terminal is what makes progress bars, `isatty`-gated colour, and
//! programs that refuse to read stdin from a pipe behave the way a human would
//! see them. It also comes with terminal semantics nobody wants implicitly:
//! `\r\n` line endings, echoed input, and a pager that waits for `q`. So a PTY
//! is **opt-in** (`tty: true`); the default keeps plain pipes, where
//! `cargo build` and `git status` behave exactly as they do today.
//!
//! ## Lifetime
//!
//! A session is killed when it is explicitly closed, when its owner drops the
//! registry, or when the idle reaper notices it has not been touched for
//! [`DEFAULT_IDLE_TIMEOUT`]. Without the reaper a model that starts a server and
//! forgets it leaves a process behind for the life of the editor.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::error::{ExecutionError, ExecutionErrorCode};

/// Sessions idle for longer than this are reaped. Long enough that a model can
/// think between two `write_stdin` calls, short enough that a forgotten
/// `npm run dev` does not outlive the session.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How much of a session's output is retained. The model reads from here, so
/// the cap is what stops a runaway build from filling the process heap; the
/// tool layer still applies its own token budget on top.
pub const MAX_SESSION_OUTPUT_BYTES: usize = 1024 * 1024;

/// Upper bound on concurrently live sessions. A model that opens one session
/// per command would otherwise leak a process tree per call.
pub const MAX_SESSIONS: usize = 16;

/// A handle to one live shell process.
///
/// Shared behind an `Arc` so `write_stdin` and `close` can hold it while the
/// session is still in the registry; the registry lock is never held across a
/// blocking read.
pub struct ShellSession {
    /// Stable id handed to the model.
    id: String,
    /// Command line, kept for the "what is session x" report.
    command: String,
    /// `true` when the process runs on a PTY (output then carries `\r\n`).
    tty: bool,
    /// Shared output buffer. The reader thread appends; every read drains the
    /// bytes it has not shown the model yet.
    output: Arc<Mutex<SessionOutput>>,
    /// Stdin of the child. `None` after the process exits or the writer fails.
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    /// Last observed liveness, refreshed by every interaction.
    last_used: Mutex<Instant>,
    /// Fires when the process is gone.
    exited: CancellationToken,
    /// Exit code, once known.
    exit_code: Mutex<Option<i32>>,
    /// Killed by [`ShellSession::close`] rather than exiting on its own.
    closed: std::sync::atomic::AtomicBool,
    /// Child pid, when known. Used to kill the whole tree on close.
    pid: AtomicU64,
    /// Highest `dropped` count already reported to the caller.
    dropped_reported: AtomicU64,
    /// Broadcast of output chunks, for a front end that wants to stream.
    events: broadcast::Sender<String>,
}

/// Debug is hand-written because the session holds a live child and a mutex:
/// deriving would print handles nobody can act on, and omit the facts that
/// matter when a session misbehaves.
impl std::fmt::Debug for ShellSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellSession")
            .field("id", &self.id)
            .field("command", &self.command)
            .field("tty", &self.tty)
            .field("pid", &self.pid())
            .field("exited", &self.has_exited())
            .field("exit_code", &self.exit_code())
            .field("closed", &self.was_closed())
            .finish()
    }
}

/// Rolling output buffer with a "seen" cursor.
#[derive(Default)]
struct SessionOutput {
    /// Everything retained, capped at [`MAX_SESSION_OUTPUT_BYTES`].
    buffer: String,
    /// How many bytes of `buffer` the model has already been shown.
    seen: usize,
    /// Total bytes ever produced, including bytes dropped by the cap. Without
    /// this a caller cannot tell "no new output" from "output was dropped".
    total: u64,
    /// Bytes dropped from the front of `buffer` by the cap.
    dropped: u64,
}

impl SessionOutput {
    /// Append a chunk, trimming the front once the cap is exceeded.
    fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.total += chunk.len() as u64;
        if self.buffer.len() > MAX_SESSION_OUTPUT_BYTES {
            let excess = self.buffer.len() - MAX_SESSION_OUTPUT_BYTES;
            // Cut on a char boundary: slicing a UTF-8 string at a byte offset
            // panics, and build output is full of non-ASCII.
            let cut = floor_char_boundary(&self.buffer, excess);
            self.buffer.drain(..cut);
            self.dropped += cut as u64;
            // The cursor points into the old buffer; rebase it so the caller
            // never re-reads bytes that were trimmed away.
            self.seen = self.seen.saturating_sub(cut);
        }
    }
}

/// Largest byte offset <= `at` that is a UTF-8 char boundary.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut i = at;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Output returned by a poll or a write.
#[derive(Debug, Clone)]
pub struct SessionOutputChunk {
    /// Bytes produced since the previous call.
    pub output: String,
    /// The process has exited (or was closed).
    pub exited: bool,
    /// Exit code, when the process has exited normally.
    pub exit_code: Option<i32>,
    /// Bytes of output dropped by the retention cap before this call.
    pub dropped_bytes: u64,
}

impl SessionOutputChunk {
    pub fn is_empty(&self) -> bool {
        self.output.is_empty() && !self.exited
    }
}

/// How `ShellSession::start` builds the child.
pub struct SessionStart {
    pub command: String,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub inherit_env: bool,
    pub shell: Option<crate::env::ShellOverride>,
    /// Allocate a PTY instead of pipes. Opt-in: see the module docs.
    pub tty: bool,
    /// Row/column for a PTY. Ignored when `tty` is false.
    pub rows: u16,
    pub cols: u16,
}

impl ShellSession {
    /// Spawn a session and start its reader threads.
    ///
    /// The reader threads own the child's stdout/stderr (or PTY master) and
    /// append into the shared buffer; the child itself is reaped on a third
    /// thread so `exit_code` becomes available without anyone polling it.
    pub fn start(id: String, start: SessionStart) -> Result<Arc<Self>, ExecutionError> {
        let (events, _) = broadcast::channel(256);
        let session = Arc::new(Self {
            id,
            command: start.command.clone(),
            tty: start.tty,
            output: Arc::new(Mutex::new(SessionOutput::default())),
            stdin: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            exited: CancellationToken::new(),
            exit_code: Mutex::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
            pid: AtomicU64::new(0),
            dropped_reported: AtomicU64::new(0),
            events,
        });

        let shell = start.shell.clone();
        if start.tty {
            session.spawn_pty(start)?;
        } else {
            session.spawn_piped(start, shell)?;
        }
        Ok(session)
    }

    /// Pipes: a plain child, exactly like `Shell::exec` but long-lived.
    fn spawn_piped(
        self: &Arc<Self>,
        start: SessionStart,
        shell: Option<crate::env::ShellOverride>,
    ) -> Result<(), ExecutionError> {
        let mut cmd = std::process::Command::new(
            shell
                .as_ref()
                .map(|s| s.program.clone())
                .unwrap_or_else(default_shell_program),
        );
        cmd.current_dir(&start.cwd);
        cmd.env_clear();
        if start.inherit_env {
            for (k, v) in std::env::vars() {
                if !crate::os_env::is_host_private_env(&k) {
                    cmd.env(k, v);
                }
            }
        }
        for (k, v) in &start.env {
            cmd.env(k, v);
        }
        match &shell {
            Some(shell) => {
                cmd.args(&shell.args);
            }
            None => {
                // No override: the interpreter needs the `-c` that a default
                // shell invocation implies. Without it bash treats the command
                // as a script filename and fails with exit 127.
                cmd.arg("-c");
            }
        }
        cmd.arg(&start.command);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|error| {
            ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                format!("failed to spawn shell: {error}"),
            )
        })?;

        // Record the pid so `close` can kill the whole group.
        self.pid.store(child.id() as u64, Ordering::Relaxed);
        if let Some(stdin) = child.stdin.take() {
            *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(stdin));
        }
        // stdout and stderr are two pipes into one buffer: the model asked for
        // "the output", not for a stream-separated transcript.
        if let Some(stdout) = child.stdout.take() {
            self.spawn_reader(stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            self.spawn_reader(stderr);
        }
        // The child is blocked in `wait` on this thread; the session's own
        // reader threads keep draining its pipes meanwhile.
        let me = Arc::downgrade(self);
        std::thread::Builder::new()
            .name(format!("tom-session-wait-{}", self.id))
            .spawn(move || {
                let code = child.wait().ok().and_then(|status| status.code());
                if let Some(me) = me.upgrade() {
                    *me.exit_code.lock().unwrap_or_else(|e| e.into_inner()) = code;
                    me.exited.cancel();
                }
            })
            .ok();
        Ok(())
    }

    /// PTY: one master handles both directions and the child sees a terminal.
    #[cfg(unix)]
    fn spawn_pty(self: &Arc<Self>, start: SessionStart) -> Result<(), ExecutionError> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: start.rows.max(1),
                cols: start.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| {
                ExecutionError::new(
                    ExecutionErrorCode::SpawnError,
                    format!("failed to open a pty: {error}"),
                )
            })?;

        let mut builder = CommandBuilder::new(
            start
                .shell
                .as_ref()
                .map(|s| s.program.to_string_lossy().into_owned())
                .unwrap_or_else(|| default_shell_program().to_string_lossy().into_owned()),
        );
        match &start.shell {
            Some(shell) => {
                builder.args(&shell.args);
            }
            None => {
                builder.arg("-c");
            }
        }
        builder.arg(&start.command);
        builder.cwd(start.cwd.clone());
        if start.inherit_env {
            for (k, v) in std::env::vars() {
                if !crate::os_env::is_host_private_env(&k) {
                    builder.env(k, v);
                }
            }
        }
        for (k, v) in &start.env {
            builder.env(k, v);
        }
        // A terminal session must not inherit the host's TERM: a TUI would then
        // try to draw into a capture buffer.
        builder.env("TERM", "dumb");

        let child = pair.slave.spawn_command(builder).map_err(|error| {
            ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                format!("failed to spawn on a pty: {error}"),
            )
        })?;
        // The slave side belongs to the child; holding it open in the parent
        // means the master never reports EOF.
        drop(pair.slave);

        if let Ok(writer) = pair.master.take_writer() {
            *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(writer);
        }
        if let Ok(mut reader) = pair.master.try_clone_reader() {
            let output = Arc::clone(&self.output);
            let events = self.events.clone();
            let exited = self.exited.clone();
            std::thread::Builder::new()
                .name(format!("tom-pty-{}", self.id))
                .spawn(move || {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                                output
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(&chunk);
                                let _ = events.send(chunk);
                            }
                        }
                    }
                    exited.cancel();
                })
                .ok();
        }
        // portable-pty's `Child` is blocking; reap on a plain thread.
        if let Some(pid) = child.process_id() {
            self.pid.store(pid as u64, Ordering::Relaxed);
        }
        let me = Arc::downgrade(self);
        let mut child = child;
        std::thread::Builder::new()
            .name(format!("tom-pty-wait-{}", self.id))
            .spawn(move || {
                let code = child.wait().ok().map(|status| status.exit_code() as i32);
                if let Some(me) = me.upgrade() {
                    *me.exit_code.lock().unwrap_or_else(|e| e.into_inner()) = code;
                    me.exited.cancel();
                }
            })
            .ok();
        Ok(())
    }

    /// PTY on non-Unix: `portable-pty` has a Win32 backend, but it is not
    /// exercised by CI here and a half-tested ConPTY path is worse than an
    /// honest refusal.
    #[cfg(not(unix))]
    fn spawn_pty(self: &Arc<Self>, _start: SessionStart) -> Result<(), ExecutionError> {
        let _ = self;
        Err(ExecutionError::new(
            ExecutionErrorCode::SpawnError,
            "tty sessions are only supported on Unix",
        ))
    }

    /// Pump a blocking pipe reader into the shared buffer.
    fn spawn_reader<R>(self: &Arc<Self>, mut reader: R)
    where
        R: Read + Send + 'static,
    {
        let output = Arc::clone(&self.output);
        let events = self.events.clone();
        std::thread::Builder::new()
            .name(format!("tom-session-{}", self.id))
            .spawn(move || {
                let mut buf = vec![0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                            output
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(&chunk);
                            let _ = events.send(chunk);
                        }
                    }
                }
            })
            .ok();
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn is_tty(&self) -> bool {
        self.tty
    }

    pub fn has_exited(&self) -> bool {
        self.exited.is_cancelled()
    }

    pub fn exit_code(&self) -> Option<i32> {
        *self.exit_code.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn was_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Subscribe to live chunks, for a front end that streams a session.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// Drain output produced since the last call.
    ///
    /// `wait` bounds how long to wait for *new* output when none is pending.
    /// This is the `yield_time_ms` of `exec_command`: the caller decides how
    /// long it is willing to block before getting control back.
    pub async fn read(&self, wait: Option<Duration>) -> SessionOutputChunk {
        if let Some(chunk) = self.drain() {
            if !chunk.output.is_empty() {
                return chunk;
            }
        }
        if let Some(wait) = wait {
            if !self.has_exited() && !wait.is_zero() {
                let mut rx = self.events.subscribe();
                // Wait for whichever comes first: new output, the process
                // exiting, or the caller's window elapsing. Any chunk is enough
                // — the caller asked "did anything happen yet", not "did it all
                // happen yet", and the process keeps running either way.
                tokio::select! {
                    biased;
                    _ = self.exited.cancelled() => {}
                    _ = tokio::time::sleep(wait) => {}
                    _ = rx.recv() => {}
                }
            }
        }
        self.drain().unwrap_or_else(|| SessionOutputChunk {
            output: String::new(),
            exited: self.has_exited(),
            exit_code: self.exit_code(),
            dropped_bytes: 0,
        })
    }

    /// Take everything new from the buffer. `None` when there is nothing.
    fn drain(&self) -> Option<SessionOutputChunk> {
        self.touch();
        let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        if output.seen >= output.buffer.len() && !self.has_exited() {
            return None;
        }
        let fresh = output.buffer[output.seen..].to_string();
        output.seen = output.buffer.len();
        let dropped_delta = output.dropped.saturating_sub(self.dropped_reported());
        if dropped_delta > 0 {
            self.dropped_reported.store(output.dropped, Ordering::Relaxed);
        }
        drop(output);
        Some(SessionOutputChunk {
            output: fresh,
            exited: self.has_exited(),
            exit_code: self.exit_code(),
            dropped_bytes: dropped_delta,
        })
    }

    fn dropped_reported(&self) -> u64 {
        self.dropped_reported.load(Ordering::Relaxed)
    }

    /// Write bytes to the session's stdin.
    ///
    /// Returns `false` when the process is gone or its stdin was closed, which
    /// the tool turns into "the session is no longer writable" rather than an
    /// error — a model polling a finished session should not see a failure.
    pub fn write(&self, input: &str, newline: bool) -> Result<(), ExecutionError> {
        self.touch();
        let mut guard = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stdin) = guard.as_mut() else {
            return Err(ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                "session stdin is closed".to_string(),
            ));
        };
        stdin
            .write_all(input.as_bytes())
            .and_then(|_| {
                if newline {
                    stdin.write_all(b"\n")
                } else {
                    Ok(())
                }
            })
            .and_then(|_| stdin.flush())
            .map_err(|error| {
                // A broken pipe is the normal way a finished process reports
                // itself; drop the handle so later writes fail consistently.
                *guard = None;
                ExecutionError::new(
                    ExecutionErrorCode::SpawnError,
                    format!("failed to write to the session: {error}"),
                )
            })
    }

    /// Terminate the process tree and mark the session closed.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.exited.cancel();
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // The pid is not retained on every platform; killing through the
        // process group needs it. `spawn_piped` sets the group leader, so the
        // registry's reaper below covers the pipe case, and the PTY backend
        // gets the same treatment through the child handle.
        if let Some(pid) = self.pid() {
            crate::os_env::kill_process_tree_blocking(pid);
        }
    }

    /// Child pid, when the backend exposes one.
    fn pid(&self) -> Option<u32> {
        match self.pid.load(Ordering::Relaxed) {
            0 => None,
            pid => u32::try_from(pid).ok(),
        }
    }
}

fn default_shell_program() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from("bash.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/bash")
    }
}

/// Registry of live sessions, keyed by the id handed to the model.
///
/// One registry belongs to the `bash` tool instance, which belongs to the
/// session; dropping it closes every child.
#[derive(Default)]
pub struct SessionRegistry {
    inner: Mutex<HashMap<String, Arc<ShellSession>>>,
    next: AtomicU64,
    idle_timeout: Option<Duration>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
        }
    }

    /// Registry that never reaps by age, for tests that need determinism.
    pub fn without_reaper() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            idle_timeout: None,
        }
    }

    /// Open a session, reaping idle ones first so a forgetful model cannot
    /// accumulate processes.
    pub fn open(&self, start: SessionStart) -> Result<Arc<ShellSession>, ExecutionError> {
        self.reap_idle();
        let mut sessions = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if sessions.len() >= MAX_SESSIONS {
            return Err(ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                format!(
                    "too many live shell sessions (limit {MAX_SESSIONS}); close one with \
                     bash_session before starting another"
                ),
            ));
        }
        let id = format!("shell-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let session = ShellSession::start(id.clone(), start)?;
        sessions.insert(id, Arc::clone(&session));
        Ok(session)
    }

    pub fn get(&self, id: &str) -> Option<Arc<ShellSession>> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// Remove and close a session. Returns whether it existed.
    pub fn close(&self, id: &str) -> bool {
        let session = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        match session {
            Some(session) => {
                session.close();
                true
            }
            None => false,
        }
    }

    /// Sessions currently registered.
    pub fn live(&self) -> Vec<Arc<ShellSession>> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Close sessions that have exited or been idle past the timeout.
    pub fn reap_idle(&self) {
        let Some(idle_timeout) = self.idle_timeout else {
            return;
        };
        let mut sessions = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let stale: Vec<String> = sessions
            .iter()
            .filter(|(_, session)| {
                // An exited session still holds its output for one last poll,
                // so only reap it once the caller has had a chance to read it
                // (a session that exited and stays idle for five minutes).
                session.has_exited() && session.idle_for() > Duration::from_secs(300)
                    || session.idle_for() > idle_timeout
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(session) = sessions.remove(&id) {
                session.close();
            }
        }
    }
}

impl Drop for SessionRegistry {
    fn drop(&mut self) {
        let sessions: Vec<Arc<ShellSession>> = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for session in sessions {
            session.close();
        }
    }
}
