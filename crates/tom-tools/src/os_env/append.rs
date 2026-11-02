//! Reusable append handles must follow the file currently named by the path.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::env::AppendFile;
use crate::error::{io_to_file_error, FileError, FileErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    volume: u64,
    index: u128,
}

pub(super) struct OsAppendFile {
    path: PathBuf,
    state: Option<AppendState>,
    buffer: Vec<u8>,
    pending: Option<tokio::task::JoinHandle<AppendOutcome>>,
}

struct AppendState {
    file: File,
    identity: Option<FileIdentity>,
}

type AppendOutcome = (Option<AppendState>, Vec<u8>, io::Result<()>);

fn open_current(path: &Path) -> io::Result<AppendState> {
    if let Some(parent) = path.parent() {
        // Match append_file: the actual open decides whether the path is
        // writable, even if the caller cannot inspect an existing parent.
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // Some filesystems cannot provide stable IDs. Reopen on every append there,
    // preserving write-only access instead of requiring metadata permissions.
    let identity = identity_from_file(&file).ok();
    Ok(AppendState { file, identity })
}

impl OsAppendFile {
    pub(super) async fn open(path: PathBuf) -> Result<Self, FileError> {
        let worker_path = path.clone();
        let state = tokio::task::spawn_blocking(move || open_current(&worker_path))
            .await
            .map_err(join_error)?
            .map_err(|error| io_to_file_error(error, Some(&path.to_string_lossy())))?;
        Ok(Self {
            path,
            state: Some(state),
            buffer: Vec::new(),
            pending: None,
        })
    }

    async fn finish_pending(&mut self) -> Result<(), FileError> {
        let Some(pending) = self.pending.as_mut() else {
            return Ok(());
        };
        // Await by reference: cancellation must leave the task owned by this
        // writer. The next append joins it before starting another write.
        let outcome = pending.await;
        self.pending = None;
        let (state, mut buffer, result) = outcome.map_err(join_error)?;
        buffer.clear();
        // Do not retain a large image/tool payload for the lifetime of a session.
        self.buffer = if buffer.capacity() <= 64 * 1024 {
            buffer
        } else {
            Vec::new()
        };
        self.state = if result.is_ok() { state } else { None };
        result.map_err(|error| io_to_file_error(error, Some(&self.path.to_string_lossy())))
    }
}

fn join_error(error: tokio::task::JoinError) -> FileError {
    FileError::new(FileErrorCode::Unknown, error.to_string()).with_source(Box::new(error))
}

#[async_trait]
impl AppendFile for OsAppendFile {
    async fn append(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.finish_pending().await?;
        let path = self.path.clone();
        let mut state = self.state.take();
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.extend_from_slice(bytes);
        self.pending = Some(tokio::task::spawn_blocking(move || {
            // Metadata, write and visibility flush share one blocking dispatch.
            // Splitting them into separate async fs operations wakes workers
            // repeatedly for every recovery frame.
            let result = (|| {
                let current = state.as_ref().and_then(|state| state.identity);
                if current.is_none() || identity_at_path(&path).ok() != current {
                    state = Some(open_current(&path)?);
                }
                let file = &mut state.as_mut().expect("current append file was opened").file;
                file.write_all(&buffer)?;
                file.flush()
            })();
            (state, buffer, result)
        }));
        self.finish_pending().await
    }
}

#[cfg(unix)]
fn identity_from_file(file: &File) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(FileIdentity {
        volume: metadata.dev(),
        index: metadata.ino().into(),
    })
}

#[cfg(unix)]
fn identity_at_path(path: &Path) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    Ok(FileIdentity {
        volume: metadata.dev(),
        index: metadata.ino().into(),
    })
}

// Stable std has no safe Windows file-ID API. Keep this crate free of unsafe
// code and follow the current path on every append on unsupported platforms.
#[cfg(not(unix))]
fn identity_from_file(_file: &File) -> io::Result<FileIdentity> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(not(unix))]
fn identity_at_path(_path: &Path) -> io::Result<FileIdentity> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_identity_reopens_after_external_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let mut writer = OsAppendFile::open(path.clone()).await.unwrap();
        writer.append(b"before\n").await.unwrap();
        writer.state.as_mut().unwrap().identity = None;
        let replacement = directory.path().join("replacement");
        std::fs::write(&replacement, b"copied\n").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        writer.append(b"after\n").await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"copied\nafter\n");
    }

    #[tokio::test]
    async fn missing_path_is_recreated_instead_of_writing_to_removed_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let backup = directory.path().join("backup");
        let mut writer = OsAppendFile::open(path.clone()).await.unwrap();
        writer.append(b"before\n").await.unwrap();
        std::fs::rename(&path, &backup).unwrap();
        writer.append(b"after\n").await.unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), b"before\n");
        assert_eq!(std::fs::read(path).unwrap(), b"after\n");
    }

    #[tokio::test]
    async fn failed_reopen_does_not_acknowledge_a_write_to_the_old_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let backup = directory.path().join("backup");
        let mut writer = OsAppendFile::open(path.clone()).await.unwrap();
        writer.append(b"before\n").await.unwrap();
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(writer.append(b"after\n").await.is_err());
        assert_eq!(std::fs::read(backup).unwrap(), b"before\n");
    }

    #[tokio::test]
    async fn cancelled_append_wait_keeps_the_pending_writer_ordered() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let mut writer = OsAppendFile::open(path.clone()).await.unwrap();
        let mut state = writer.state.take().unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        writer.pending = Some(tokio::spawn(async move {
            wait.await.unwrap();
            let result = state.file.write_all(b"pending\n");
            (Some(state), Vec::new(), result)
        }));
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            writer.append(b"cancelled\n")
        )
        .await
        .is_err());
        assert!(writer.pending.is_some());
        release.send(()).unwrap();
        writer.append(b"next\n").await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"pending\nnext\n");
    }
}
