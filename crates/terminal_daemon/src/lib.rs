//! Optional terminal daemon.
//!
//! The daemon owns PTYs so terminal sessions survive the GUI quitting, the
//! window closing, or an SSH connection dropping. It is deliberately optional:
//! the GUI only talks to it when `terminal.persistent_sessions` is enabled.
//!
//! Design notes:
//!
//! - The daemon never renders. It owns the PTY, keeps a bounded ring buffer of
//!   raw output for replay, and fans output out to attached clients. The GUI
//!   feeds those bytes into its own emulator, so no screen-diff protocol is
//!   needed.
//! - Clients use two sockets roles: a control connection for
//!   spawn/list/kill, and one attach connection per viewed session.
//! - Frames are `[u32 length][u8 tag][payload]`. Control messages are JSON;
//!   terminal bytes travel as raw payloads.
//!
//! This module is Unix-first (Unix domain sockets + PTY). Windows support
//! (named pipes + ConPTY) is not implemented yet; the non-Unix build exposes
//! stubs that return errors so the workspace still compiles.

pub mod protocol;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(not(unix))]
mod stub;
#[cfg(not(unix))]
pub use stub::*;

use std::path::PathBuf;

pub use protocol::SessionInfo;

/// File name of the daemon socket inside the app data directory.
pub const SOCKET_FILE_NAME: &str = "terminal-daemon.sock";

/// Default socket path shared by the daemon and its clients.
///
/// Unix sockets have a small path limit (`SUN_LEN`, 104 bytes on macOS), so the
/// socket lives in a short per-user directory under `/tmp` instead of the app
/// data directory. The directory is created with mode `0700` by the daemon.
pub fn socket_path() -> PathBuf {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/terry-{uid}")).join(SOCKET_FILE_NAME)
    }
    #[cfg(not(unix))]
    {
        paths::data_dir().join(SOCKET_FILE_NAME)
    }
}

/// Everything needed to start a session.
#[derive(Clone, Debug)]
pub struct SessionSpec {
    /// Stable id. Reusing an id reattaches instead of spawning a second PTY.
    pub session: String,
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

/// Something the daemon sends to an attached client.
#[derive(Debug)]
pub enum Event {
    /// Raw PTY output. On attach this starts with the replay buffer.
    Output(Vec<u8>),
    /// The child exited with this code (128 + signal when killed by a signal).
    Exited(i32),
}

/// Drops the leading part of a session's replay so a reattach only feeds the
/// emulator the tail it will actually keep.
///
/// The daemon buffers raw bytes (up to 8 MiB), but the emulator only keeps
/// `max_scroll_history_lines` (10k by default), so parsing far more than that
/// is wasted work. This skips the oldest bytes across chunk boundaries and
/// starts on a line boundary, so a cut escape sequence or half line does not
/// garble the top of the scrollback.
#[derive(Debug)]
pub struct ReplayTail {
    skip: usize,
    aligned: bool,
}

impl ReplayTail {
    /// `skip` is how many leading replay bytes to drop.
    pub fn new(skip: usize) -> Self {
        Self {
            skip,
            aligned: skip == 0,
        }
    }

    /// Returns the part of `bytes` that should be fed to the emulator.
    pub fn feed<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        if self.aligned {
            return bytes;
        }
        if bytes.len() <= self.skip {
            self.skip -= bytes.len();
            return &[];
        }

        let mut start = self.skip;
        self.skip = 0;
        self.aligned = true;
        if let Some(newline) = bytes[start..].iter().position(|&byte| byte == b'\n') {
            start += newline + 1;
        }
        &bytes[start..]
    }
}

#[cfg(test)]
mod replay_tail_tests {
    use super::ReplayTail;

    #[test]
    fn keeps_everything_without_a_skip() {
        let mut tail = ReplayTail::new(0);
        assert_eq!(tail.feed(b"abc"), b"abc");
        assert_eq!(tail.feed(b"def"), b"def");
    }

    #[test]
    fn skips_part_of_a_chunk() {
        let mut tail = ReplayTail::new(3);
        assert_eq!(tail.feed(b"abcdef"), b"def");
        assert_eq!(tail.feed(b"ghi"), b"ghi");
    }

    #[test]
    fn skips_across_chunks_and_starts_on_a_line() {
        let mut tail = ReplayTail::new(4);
        assert_eq!(tail.feed(b"abcd"), b"");
        assert_eq!(tail.feed(b"ef\ngh"), b"gh");
    }

    #[test]
    fn skips_more_than_was_received() {
        let mut tail = ReplayTail::new(10);
        assert_eq!(tail.feed(b"abcd"), b"");
        assert_eq!(tail.feed(b"efghij"), b"");
        assert_eq!(tail.feed(b"klmn"), b"klmn");
    }
}
