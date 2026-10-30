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
