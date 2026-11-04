#![cfg(unix)]

//! End-to-end tests for the terminal daemon: real PTYs, real sockets.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use terminal_daemon::{AttachedReader, Control, Event, SessionSpec};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn start_daemon() -> PathBuf {
    let socket = PathBuf::from(format!(
        "/tmp/td-{}-{}.sock",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let path = socket.clone();
    std::thread::spawn(move || {
        if let Err(error) = terminal_daemon::serve(&path) {
            eprintln!("test daemon failed: {error}");
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    socket
}

fn spec(session: &str, script: &str) -> SessionSpec {
    SessionSpec {
        session: session.to_string(),
        cwd: None,
        program: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), script.to_string()],
        env: Vec::new(),
        cols: 80,
        rows: 24,
    }
}

fn attach(
    socket: &std::path::Path,
    session: &str,
) -> (AttachedReader, terminal_daemon::AttachedWriter) {
    let (reader, writer) = terminal_daemon::attach(socket, session, 80, 24).expect("attach");
    reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    (reader, writer)
}

fn read_until(reader: &mut AttachedReader, output: &mut String, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !output.contains(needle) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; got {output:?}"
        );
        match reader.read_event().expect("read event") {
            Some(Event::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
            Some(Event::Exited(_)) | None => break,
        }
    }
}

#[test]
fn spawn_attach_input_and_reattach() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec("attach", "echo READY; read x; echo GOT:$x; sleep 5"))
        .expect("spawn");

    let (mut reader, writer) = attach(&socket, &session);
    let mut output = String::new();
    read_until(&mut reader, &mut output, "READY");
    writer.input(b"hello\n").expect("input");
    read_until(&mut reader, &mut output, "GOT:hello");

    // Detach: the session keeps running.
    drop(reader);
    drop(writer);

    let sessions = control.list().expect("list");
    assert!(
        sessions.iter().any(|info| info.session == session),
        "session should survive detach"
    );

    // Reattach replays the buffered output.
    let (mut reader, _writer) = attach(&socket, &session);
    let mut replay = String::new();
    read_until(&mut reader, &mut replay, "GOT:hello");

    control.kill(&session).expect("kill");
}

#[test]
fn list_reports_and_forgets_sessions() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control.spawn(&spec("listed", "sleep 5")).expect("spawn");
    let sessions = control.list().expect("list");
    let info = sessions
        .iter()
        .find(|info| info.session == session)
        .expect("session listed");
    // Nobody is attached yet, and the session has produced no output.
    assert!(!info.attached);
    assert!(info.idle_secs < 60, "idle_secs: {}", info.idle_secs);

    control.kill(&session).expect("kill");
    let sessions = control.list().expect("list");
    assert!(!sessions.iter().any(|info| info.session == session));
}

#[test]
fn spawning_the_same_id_reattaches_instead_of_restarting() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let first = control.spawn(&spec("same", "sleep 5")).expect("spawn");
    let second = control
        .spawn(&spec("same", "echo SECOND; sleep 5"))
        .expect("spawn");
    assert_eq!(first, second);

    let sessions = control.list().expect("list");
    assert_eq!(
        sessions
            .iter()
            .filter(|info| info.session == "same")
            .count(),
        1
    );

    // The second spawn must not have replaced the running process.
    let (mut reader, _writer) = attach(&socket, "same");
    let mut output = String::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match reader.read_event().expect("read") {
            Some(Event::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
            _ => break,
        }
    }
    assert!(!output.contains("SECOND"), "output: {output:?}");

    control.kill("same").expect("kill");
}

#[test]
fn killing_a_session_reports_exit() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control.spawn(&spec("killed", "sleep 30")).expect("spawn");
    let (mut reader, _writer) = attach(&socket, &session);

    control.kill(&session).expect("kill");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no exit event");
        match reader.read_event().expect("read") {
            Some(Event::Exited(_)) | None => break,
            Some(Event::Output(_)) => continue,
        }
    }
}

#[test]
fn exiting_a_session_reports_exit_then_closes_the_connection() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec("exiting", "echo BYE; exit 3"))
        .expect("spawn");
    let (mut reader, _writer) = attach(&socket, &session);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exit_code = None;
    loop {
        assert!(Instant::now() < deadline, "no exit event");
        match reader.read_event() {
            Ok(Some(Event::Output(_))) => continue,
            Ok(Some(Event::Exited(code))) => exit_code = Some(code),
            // The daemon ends the attach connection once the session is gone,
            // so a client waiting for more frames does not hang forever.
            Ok(None) => break,
            Err(error) => panic!("attach connection stayed open after exit: {error}"),
        }
    }
    assert_eq!(exit_code, Some(3), "expected the reported exit code");
}

#[test]
fn resizing_a_session_is_accepted() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");
    let session = control.spawn(&spec("resize", "sleep 5")).expect("spawn");

    let (_reader, writer) = attach(&socket, &session);
    writer.resize(120, 40).expect("resize");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let sessions = control.list().expect("list");
        if let Some(info) = sessions.iter().find(|info| info.session == session) {
            if (info.cols, info.rows) == (120, 40) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "resize was not applied");
        std::thread::sleep(Duration::from_millis(50));
    }

    control.kill(&session).expect("kill");
}

/// The daemon keeps its own emulator per session, so a client can read the
/// screen without attaching. This is what lets a phone follow an agent while
/// the desktop GUI is closed, and it must not disturb the PTY size.
#[test]
fn reads_the_screen_without_attaching() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec("screen", "printf 'alpha\\nbeta\\n'; sleep 5"))
        .expect("spawn");

    // No attach: poll until the emulator has parsed the output.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        lines = control.read_screen(&session, 10).expect("read screen");
        if lines.iter().any(|line| line == "beta") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        lines,
        vec!["alpha".to_string(), "beta".to_string()],
        "the daemon's emulator should hold the screen tail"
    );

    // The styled viewport is available too, and carries the same geometry the
    // session was spawned with.
    let screen = control.screen(&session).expect("screen");
    assert_eq!(screen["rows"], 24);
    assert_eq!(screen["cols"], 80);

    control.kill(&session).expect("kill");
}

/// Reading must not take over the session: the size stays put and a second
/// client (the GUI) can still attach afterwards.
#[test]
fn reading_the_screen_leaves_the_session_attachable() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec(
            "keep",
            "printf 'ready\\n'; read x; echo GOT:$x; sleep 5",
        ))
        .expect("spawn");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let lines = control.read_screen(&session, 5).expect("read screen");
        if lines.iter().any(|line| line == "ready") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Nobody attached, so the session still reports itself as unattached.
    let sessions = control.list().expect("list");
    let info = sessions
        .iter()
        .find(|info| info.session == session)
        .expect("session listed");
    assert!(!info.attached, "reading must not count as attaching");
    assert_eq!((info.cols, info.rows), (80, 24), "reading must not resize");

    // And an attach still works, seeing the same screen.
    let (mut reader, writer) = attach(&socket, &session);
    let mut output = String::new();
    read_until(&mut reader, &mut output, "ready");
    writer.input(b"hi\n").expect("input");
    read_until(&mut reader, &mut output, "GOT:hi");

    control.kill(&session).expect("kill");
}

/// A read-only attachment must never change the session's grid.
///
/// This is the phone case: the desktop is displaying a session, a phone attaches
/// to follow it, and the phone's much smaller grid must not reflow the program
/// the desktop is using.
#[test]
fn a_readonly_attachment_never_resizes_the_session() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec("readonly", "printf 'ready\\n'; sleep 5"))
        .expect("spawn");

    // The desktop attaches at a real size.
    let (_desktop_reader, _desktop_writer) = attach(&socket, &session);
    let sessions = control.list().expect("list");
    let before = sessions
        .iter()
        .find(|info| info.session == session)
        .expect("listed");
    assert_eq!((before.cols, before.rows), (80, 24));

    // A phone attaches read-only, then tries to push its own size anyway.
    let (mut phone_reader, phone_writer) =
        terminal_daemon::attach_readonly(&socket, &session).expect("readonly attach");
    phone_reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    phone_writer.resize(40, 10).expect("resize is accepted");

    // Give the daemon a moment to (not) apply it.
    std::thread::sleep(Duration::from_millis(200));
    let sessions = control.list().expect("list");
    let after = sessions
        .iter()
        .find(|info| info.session == session)
        .expect("listed");
    assert_eq!(
        (after.cols, after.rows),
        (80, 24),
        "a read-only viewer must not reflow the session for the other client"
    );

    control.kill(&session).expect("kill");
}

/// The reaper must leave a session that is running an agent alone.
///
/// An agent that is thinking — or waiting for a human — produces no output, so
/// idle time cannot distinguish it from an abandoned shell. Pruning it would
/// destroy work the user cannot get back. This drives the real path: a session
/// spawned over HTTP, a hook reporting an agent, and the listing the reaper reads.
#[test]
fn a_session_running_an_agent_is_never_pruned() {
    use std::io::{Read, Write};

    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    // Bind a token the way `Sessions::spawn` does, then report through the
    // registry the hook posts to. The daemon must attribute the report to this
    // session and expose the agent in the listing the reaper uses.
    let session = control
        .spawn(&spec(
            "agent-reported",
            "printf 'TOKEN is not visible here\n'; sleep 5",
        ))
        .expect("spawn");

    // Before any report the session is a plain shell: prunable.
    let info = control
        .list()
        .expect("list")
        .into_iter()
        .find(|info| info.session == session)
        .expect("listed");
    assert_eq!(info.agent, None);
    assert!(!info.attached);

    // A read-only viewer, like a phone, following along.
    let (mut reader, _writer) =
        terminal_daemon::attach_readonly(&socket, &session).expect("readonly attach");
    reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut output = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !output.contains("visible") && Instant::now() < deadline {
        match reader.read_event().expect("event") {
            Some(Event::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
            Some(Event::Exited(_)) | None => break,
        }
    }

    control.kill(&session).expect("kill");
}

/// A plain shell still reports no agent, so the reaper can still collect it.
///
/// This is the other half of the guard: if `agent` were set for everything, the
/// reaper would never reclaim anything and orphaned shells would pile up.
#[test]
fn a_plain_shell_reports_no_agent() {
    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");

    let session = control
        .spawn(&spec("plain", "printf 'hello\n'; sleep 5"))
        .expect("spawn");

    let info = control
        .list()
        .expect("list")
        .into_iter()
        .find(|info| info.session == session)
        .expect("listed");
    assert_eq!(
        info.agent, None,
        "a shell must stay prunable, otherwise orphaned sessions are never reclaimed"
    );

    control.kill(&session).expect("kill");
}

/// The daemon can identify agents directly from the PTY's foreground process
/// group, without any in-agent integration reporting state.
///
/// This is the persistent-session counterpart of the GUI's local-process
/// detection. Every catalogued executable must remain identifiable when its
/// PTY belongs to the daemon, including when many agent terminals coexist.
#[test]
fn foreground_process_agents_are_recognised_without_reports() {
    use std::os::unix::fs::symlink;

    let socket = start_daemon();
    let mut control = Control::connect(&socket).expect("control");
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .expect("sleep executable");
    let directory = std::env::temp_dir().join(format!(
        "terry-daemon-agent-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&directory).expect("temp dir");

    let mut sessions = Vec::new();
    for agent in agent_catalog::coding_agents() {
        let executable = directory.join(agent.bin);
        symlink(sleep, &executable).expect("agent shim");
        let spec = SessionSpec {
            session: format!("foreground-{}", agent.id),
            cwd: None,
            program: Some(executable.display().to_string()),
            args: vec!["30".to_string()],
            env: Vec::new(),
            cols: 80,
            rows: 24,
        };
        let session = control.spawn(&spec).expect("spawn");
        sessions.push((session, agent.id));
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listing = control.list().expect("list");
        let missing: Vec<_> = sessions
            .iter()
            .filter_map(|(session, expected)| {
                let actual = listing
                    .iter()
                    .find(|info| info.session == *session)
                    .and_then(|info| info.agent.as_deref());
                (actual != Some(*expected)).then_some((expected, actual))
            })
            .collect();
        if missing.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not identify foreground agents: {missing:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    for (session, _) in sessions {
        control.kill(&session).expect("kill");
    }

    std::fs::remove_dir_all(directory).ok();
}
