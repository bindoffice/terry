#![cfg(unix)]

//! Manual smoke test: what a phone client would do against the daemon.
//! Run with `cargo test -p terminal_daemon --test manual_smoke -- --nocapture`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use terminal_daemon::{Control, SessionSpec};

#[test]
fn agent_question_round_trip() {
    let socket = PathBuf::from(format!("/tmp/terry-smoke-{}.sock", std::process::id()));
    let path = socket.clone();
    std::thread::spawn(move || {
        let _ = terminal_daemon::serve(&path);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut control = Control::connect(&socket).expect("control");
    let session = control
        .spawn(&SessionSpec {
            session: "t-agent".into(),
            cwd: None,
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "printf 'Do you want to proceed? (y/n)\\n'; read a; echo ANSWER:$a; sleep 5"
                    .into(),
            ],
            env: Vec::new(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn");

    // 1. Poll the screen tail without attaching (what the phone list does).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        lines = control.read_screen(&session, 5).expect("read");
        if lines.iter().any(|l| l.contains("proceed")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("--- screen tail (no attach) ---");
    for line in &lines {
        println!("{line}");
    }
    assert!(lines.iter().any(|l| l.contains("proceed? (y/n)")));

    // 2. The styled viewport, same encoding the GUI's /api/screen returns.
    let screen = control.screen(&session).expect("screen");
    println!("--- styled viewport ---");
    println!(
        "rows={} cols={} default_bg={}",
        screen["rows"], screen["cols"], screen["default_bg"]
    );
    println!("line 0: {}", screen["lines"][0]);
    assert_eq!(screen["rows"], 24);
    assert_eq!(screen["cols"], 80);

    // 3. Answer it, with an attach that must not resize the PTY (0,0).
    let (_reader, writer) = terminal_daemon::attach(&socket, &session, 0, 0).expect("attach");
    writer.input(b"y\n").expect("input");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut after = Vec::new();
    while Instant::now() < deadline {
        after = control.read_screen(&session, 5).expect("read");
        if after.iter().any(|l| l.contains("ANSWER:y")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("--- after answering ---");
    for line in &after {
        println!("{line}");
    }
    assert!(after.iter().any(|l| l.contains("ANSWER:y")));

    // 4. Reading never resized the session.
    let info = control
        .list()
        .expect("list")
        .into_iter()
        .find(|i| i.session == session)
        .expect("listed");
    println!("--- session info ---");
    println!("size={}x{} attached={}", info.cols, info.rows, info.attached);
    assert_eq!((info.cols, info.rows), (80, 24));

    control.kill(&session).expect("kill");
}
