//! Live shell sessions: the `session_id` / `yield_time_ms` path.
//!
//! The point of a session is that the process outlives the tool call, so every
//! test here has to observe a *second* call reaching the *same* process. That
//! is what a one-shot `bash` call cannot do, and it is the whole reason the
//! feature exists.

use std::sync::Arc;
use std::time::Duration;

use tom_tools::pty_session::{SessionRegistry, SessionStart};

fn registry() -> SessionRegistry {
    // No idle reaper: these tests control lifetime explicitly.
    SessionRegistry::without_reaper()
}

fn start(command: &str) -> SessionStart {
    SessionStart {
        command: command.to_string(),
        cwd: std::env::current_dir().unwrap(),
        env: Default::default(),
        inherit_env: true,
        shell: None,
        tty: false,
        rows: 24,
        cols: 80,
    }
}

/// Poll until the predicate holds, bounded so a hang fails instead of blocking.
async fn wait_for_output(
    session: &Arc<tom_tools::pty_session::ShellSession>,
    needle: &str,
) -> String {
    let mut all = String::new();
    for _ in 0..100 {
        let chunk = session.read(Some(Duration::from_millis(100))).await;
        all.push_str(&chunk.output);
        if all.contains(needle) {
            return all;
        }
        if chunk.exited {
            break;
        }
    }
    all
}

#[tokio::test]
async fn a_session_outlives_the_call_that_started_it() {
    let registry = registry();
    let session = registry
        .open(start("echo first; sleep 0.3; echo second"))
        .expect("session should start");

    // First read sees the early output while the process is still running.
    let early = session.read(Some(Duration::from_millis(1000))).await;
    assert!(!early.exited, "process should still be running");

    // The same session later yields the output that had not been produced yet.
    // A one-shot exec would have had to re-run the whole command for this.
    let all = wait_for_output(&session, "second").await;
    assert!(all.contains("second"), "got {all:?}");
}

#[tokio::test]
async fn writes_reach_the_running_process() {
    let registry = registry();
    let session = registry
        .open(start("read line; echo \"got:$line\""))
        .expect("session should start");

    session.write("hello", true).expect("write should work");

    let all = wait_for_output(&session, "got:hello").await;
    assert!(all.contains("got:hello"), "got {all:?}");
}

#[tokio::test]
async fn output_is_drained_exactly_once() {
    let registry = registry();
    let session = registry.open(start("echo once")).expect("start");

    let first = wait_for_output(&session, "once").await;
    assert!(first.contains("once"));
    // Drain until the process has exited, so the assertion below is about the
    // cursor rather than about how fast the shell finished.
    let mut rest = String::new();
    for _ in 0..100 {
        let chunk = session.read(Some(Duration::from_millis(50))).await;
        rest.push_str(&chunk.output);
        if chunk.exited && chunk.output.is_empty() {
            break;
        }
    }
    // Once drained, nothing is replayed: the model already paid for those
    // tokens once.
    assert!(
        !rest.contains("once"),
        "output was replayed: {rest:?}"
    );
}

#[tokio::test]
async fn a_finished_session_reports_its_exit_code() {
    let registry = registry();
    let session = registry.open(start("exit 7")).expect("start");

    for _ in 0..100 {
        let chunk = session.read(Some(Duration::from_millis(100))).await;
        if chunk.exited {
            assert_eq!(chunk.exit_code, Some(7));
            return;
        }
    }
    panic!("session never reported exit");
}

#[tokio::test]
async fn closing_a_session_removes_it_from_the_registry() {
    let registry = registry();
    let session = registry.open(start("sleep 5")).expect("start");
    let id = session.id().to_string();
    assert!(registry.get(&id).is_some());

    assert!(registry.close(&id));
    assert!(registry.get(&id).is_none());
    // Closing twice is not an error, it is just a no-op.
    assert!(!registry.close(&id));
}

#[tokio::test]
async fn the_registry_refuses_to_open_past_its_limit() {
    let registry = registry();
    let mut opened = Vec::new();
    for _ in 0..tom_tools::MAX_SESSIONS {
        opened.push(registry.open(start("sleep 5")).expect("within limit"));
    }
    let error = registry
        .open(start("sleep 5"))
        .expect_err("should refuse past the cap");
    assert!(
        error.to_string().contains("too many live shell sessions"),
        "got {error}"
    );
    for session in opened {
        registry.close(session.id());
    }
}

#[tokio::test]
async fn a_utf8_boundary_is_never_split_by_the_retention_cap() {
    // Multi-byte output larger than the cap would panic if the trim sliced at a
    // raw byte offset.
    let registry = registry();
    let session = registry
        .open(start("yes 中文 | head -c 200000"))
        .expect("start");

    let mut total = 0usize;
    for _ in 0..200 {
        let chunk = session.read(Some(Duration::from_millis(50))).await;
        total += chunk.output.len();
        if chunk.exited && chunk.output.is_empty() {
            break;
        }
    }
    assert!(total > 0, "session produced no output at all");
}

#[cfg(unix)]
#[tokio::test]
async fn a_pty_session_sees_a_terminal() {
    let registry = registry();
    let mut start = start("if [ -t 1 ]; then echo tty=yes; else echo tty=no; fi");
    start.tty = true;
    let session = registry.open(start).expect("pty session should start");

    let all = wait_for_output(&session, "tty=").await;
    assert!(
        all.contains("tty=yes"),
        "a tty session must report a terminal, got {all:?}"
    );
}
