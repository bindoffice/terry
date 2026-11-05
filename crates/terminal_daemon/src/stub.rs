//! Non-Unix stubs: the daemon is Unix-only for now, so the workspace still
//! compiles on Windows and every entry point reports the limitation.

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::protocol::SessionInfo;
use crate::{DaemonInfo, Event, SessionSpec};

const UNSUPPORTED: &str = "the terminal daemon is not supported on this platform yet";

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, UNSUPPORTED)
}

pub fn serve(_socket: &Path) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

/// Extra listeners to start alongside the daemon socket.
///
/// Unix-only in practice; the stub keeps the type so `terry daemon` compiles
/// and reports the platform limitation instead of failing to build.
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    pub http: Option<String>,
    pub token_file: Option<std::path::PathBuf>,
}

impl ServeOptions {
    pub fn with_http(
        bind: impl Into<String>,
        token_file: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            http: Some(bind.into()),
            token_file: Some(token_file.into()),
        }
    }
}

pub fn serve_with_options(_socket: &Path, _options: ServeOptions) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

pub fn serve_with_options_and_port(
    _socket: &Path,
    _options: ServeOptions,
    _on_http_port: impl FnOnce(u16),
) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

pub fn ensure_daemon(_socket: &Path) -> anyhow::Result<()> {
    anyhow::bail!(UNSUPPORTED)
}

pub fn is_daemon_running(_socket: &Path) -> bool {
    false
}

pub fn wait_until_stopped(_socket: &Path, _timeout: Duration) -> bool {
    false
}

pub fn wait_until_running(_socket: &Path, _timeout: Duration) -> bool {
    false
}

pub fn terminate_daemon(_socket: &Path) -> anyhow::Result<Vec<u32>> {
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

    pub fn info(&mut self) -> anyhow::Result<DaemonInfo> {
        anyhow::bail!(UNSUPPORTED)
    }

    pub fn shutdown(&mut self) -> anyhow::Result<DaemonInfo> {
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
    replayed_bytes: usize,
}

impl AttachedReader {
    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn replayed_bytes(&self) -> usize {
        self.replayed_bytes
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
