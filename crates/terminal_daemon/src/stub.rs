//! Non-Unix stubs: the daemon is Unix-only for now, so the workspace still
//! compiles on Windows and every entry point reports the limitation.

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::protocol::SessionInfo;
use crate::{Event, SessionSpec};

const UNSUPPORTED: &str = "the terminal daemon is not supported on this platform yet";

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, UNSUPPORTED)
}

pub fn serve(_socket: &Path) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

pub fn ensure_daemon(_socket: &Path) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

pub struct Control;

impl Control {
    pub fn connect(_socket: &Path) -> anyhow::Result<Self> {
        anyhow::bail!(UNSUPPORTED)
    }

    pub fn spawn(&mut self, _spec: &SessionSpec) -> anyhow::Result<String> {
        anyhow::bail!(UNSUPPORTED)
    }

    pub fn list(&mut self) -> anyhow::Result<Vec<SessionInfo>> {
        anyhow::bail!(UNSUPPORTED)
    }

    pub fn kill(&mut self, _session: &str) -> anyhow::Result<()> {
        anyhow::bail!(UNSUPPORTED)
    }
}

pub struct AttachedWriter;

impl AttachedWriter {
    pub fn input(&self, _data: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn resize(&self, _cols: u16, _rows: u16) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn detach(&self) -> io::Result<()> {
        Err(unsupported())
    }
}

pub struct AttachedReader {
    session: String,
}

impl AttachedReader {
    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn read_event(&mut self) -> anyhow::Result<Option<Event>> {
        anyhow::bail!(UNSUPPORTED)
    }
}

pub fn attach(
    _socket: &Path,
    _session: &str,
    _cols: u16,
    _rows: u16,
) -> anyhow::Result<(AttachedReader, AttachedWriter)> {
    anyhow::bail!(UNSUPPORTED)
}
