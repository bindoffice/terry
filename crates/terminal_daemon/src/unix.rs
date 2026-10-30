//! Unix implementation: a Unix-domain-socket daemon that owns PTYs.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{self, Pty, ToWinsize};

use crate::protocol::{
    Reply, Request, SessionInfo, TAG_INPUT, TAG_JSON, TAG_OUTPUT, read_frame, write_frame,
    write_json,
};
use crate::{Event, SessionSpec};

/// Output kept for replay when a client reattaches.
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
/// Read size from the PTY.
const READ_CHUNK: usize = 64 * 1024;
/// Frame size used when replaying the buffer.
const REPLAY_CHUNK: usize = 64 * 1024;

type Registry = Arc<Mutex<HashMap<String, Arc<Session>>>>;

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
        }
    }

    fn has_exited(&self) -> bool {
        self.shared
            .lock()
            .expect("session mutex poisoned")
            .exited
            .is_some()
    }

    fn publish(&self, data: &[u8]) {
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
        let writer = self.writer.lock().expect("writer mutex poisoned");
        if let Some(file) = writer.as_ref() {
            apply_resize(file, cols, rows);
        }
    }

    fn kill(&self) {
        let pty = self.pty.lock().expect("pty mutex poisoned").take();
        if let Some(pty) = pty {
            let pid = pty.child().id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGHUP);
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

fn list_sessions(registry: &Registry) -> Vec<SessionInfo> {
    let registry = registry.lock().expect("registry mutex poisoned");
    let mut sessions: Vec<SessionInfo> = registry.values().map(|session| session.info()).collect();
    sessions.sort_by(|a, b| a.session.cmp(&b.session));
    sessions
}

fn kill_session(registry: &Registry, id: &str) -> anyhow::Result<()> {
    let removed = registry.lock().expect("registry mutex poisoned").remove(id);
    let Some(session) = removed else {
        anyhow::bail!("unknown session {id}");
    };
    session.kill();
    Ok(())
}

/// Runs the daemon until the process is killed.
pub fn serve(socket: &Path) -> anyhow::Result<()> {
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

    let registry: Registry = Arc::default();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let registry = registry.clone();
                thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, registry) {
                        log::debug!("terminal daemon connection closed: {error}");
                    }
                });
            }
            Err(error) => log::warn!("terminal daemon accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, registry: Registry) -> anyhow::Result<()> {
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
                        sessions: list_sessions(&registry),
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
            } => return attach_connection(reader, writer, registry, session, cols, rows),
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
    if cols > 0 && rows > 0 {
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
                        break;
                    }
                }
            }
        });

        let read_result = loop {
            match read_frame(&mut reader) {
                Ok((TAG_INPUT, payload)) => session.write_input(&payload),
                Ok((TAG_JSON, payload)) => match serde_json::from_slice::<Request>(&payload) {
                    Ok(Request::Resize { cols, rows }) => session.resize(cols, rows),
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
    let stream = UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    write_json(
        &mut writer,
        &Request::Attach {
            session: session.to_string(),
            cols,
            rows,
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
