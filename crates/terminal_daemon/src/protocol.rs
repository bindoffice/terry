//! Wire protocol between the GUI (or a script) and the terminal daemon.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Payload is UTF-8 JSON.
pub const TAG_JSON: u8 = 1;
/// Payload is raw PTY output (daemon -> client).
pub const TAG_OUTPUT: u8 = 2;
/// Payload is raw keyboard input (client -> daemon).
pub const TAG_INPUT: u8 = 3;

/// Upper bound for a single frame. Output is chunked below this size.
pub const MAX_FRAME_BYTES: u32 = 4 * 1024 * 1024;

/// Writes one length-prefixed frame.
pub fn write_frame<W: Write>(writer: &mut W, tag: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame exceeds MAX_FRAME_BYTES",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&[tag])?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Writes one JSON control frame.
pub fn write_json<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(message)?;
    write_frame(writer, TAG_JSON, &payload)
}

/// Reads one length-prefixed frame.
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length);
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds MAX_FRAME_BYTES",
        ));
    }
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag)?;
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    Ok((tag[0], payload))
}

/// A session as reported by `List`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session: String,
    pub cols: u16,
    pub rows: u16,
    /// Exit code once the child is gone.
    pub exited: Option<i32>,
    /// Bytes available for replay on the next attach.
    pub buffered_bytes: usize,
    /// Whether a client is currently attached.
    #[serde(default)]
    pub attached: bool,
    /// Seconds since the last output. Used to avoid reaping live sessions.
    #[serde(default)]
    pub idle_secs: u64,
    /// The coding agent running in this session, when one was reported.
    ///
    /// The reaper uses this: an agent that is thinking (or waiting for a human)
    /// produces no output, so it looks exactly like an abandoned session. Killing
    /// it loses work the user cannot get back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Client -> daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Start a session, or reattach to an existing one with the same id.
    Spawn {
        session: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        program: Option<String>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        cols: u16,
        rows: u16,
    },
    /// Switch this connection into attach mode for `session`.
    Attach {
        session: String,
        cols: u16,
        rows: u16,
        /// Never resize the PTY from this connection, not even through `Resize`.
        ///
        /// A phone watching a session that the desktop is also displaying must
        /// not impose its own grid: the two would fight, and whichever attached
        /// last would reflow the program the other one is using. A mirror client
        /// renders at the session's size instead (see the `Attached` reply) and
        /// simply scales or scrolls.
        #[serde(default)]
        readonly: bool,
    },
    List,
    Kill {
        session: String,
    },
    /// Attach mode only.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Attach mode only: leave the session running and close this connection.
    Detach,
    /// Read a session's screen without attaching to it.
    ///
    /// `lines: Some(n)` returns the last `n` non-empty lines as plain text
    /// (`Reply::Text`); `None` returns the styled viewport (`Reply::Screen`).
    /// Reading needs no attach, so a client that only follows agent state never
    /// takes over the input side or touches the PTY size.
    ReadScreen {
        session: String,
        #[serde(default)]
        lines: Option<usize>,
    },
}

/// Daemon -> client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Reply {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Attached {
        session: String,
        cols: u16,
        rows: u16,
        replayed_bytes: usize,
    },
    Exited {
        code: i32,
    },
    /// A styled viewport, encoded exactly like the GUI's `/api/screen` payload
    /// (`terminal_core::screen_json`), so a client can talk to either host.
    Screen {
        screen: serde_json::Value,
    },
    /// The last non-empty lines of a session, oldest first.
    Text {
        lines: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, TAG_OUTPUT, b"hello").unwrap();
        write_json(&mut buffer, &Request::List).unwrap();

        let mut cursor = std::io::Cursor::new(buffer);
        let (tag, payload) = read_frame(&mut cursor).unwrap();
        assert_eq!(tag, TAG_OUTPUT);
        assert_eq!(payload, b"hello");

        let (tag, payload) = read_frame(&mut cursor).unwrap();
        assert_eq!(tag, TAG_JSON);
        let request: Request = serde_json::from_slice(&payload).unwrap();
        assert!(matches!(request, Request::List));
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut buffer = Vec::new();
        let payload = vec![0u8; MAX_FRAME_BYTES as usize + 1];
        assert!(write_frame(&mut buffer, TAG_OUTPUT, &payload).is_err());
    }

    #[test]
    fn attach_readonly_defaults_to_false() {
        let request: Request =
            serde_json::from_str(r#"{"op":"attach","session":"s","cols":80,"rows":24}"#).unwrap();
        match request {
            Request::Attach { readonly, .. } => assert!(
                !readonly,
                "an old client that does not send the flag must still be able to resize"
            ),
            other => panic!("expected Attach, got {other:?}"),
        }
    }

    #[test]
    fn spawn_request_uses_defaults_for_omitted_fields() {
        let request: Request =
            serde_json::from_str(r#"{"op":"spawn","session":"s","cols":80,"rows":24}"#).unwrap();
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
                assert_eq!(session, "s");
                assert_eq!(cwd, None);
                assert_eq!(program, None);
                assert!(args.is_empty());
                assert!(env.is_empty());
                assert_eq!((cols, rows), (80, 24));
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
    }
}
