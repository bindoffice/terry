//! Mirrors `packages/agent/src/harness/tools/bash.ts` — the built-in `bash`
//! `AgentTool`. Drives `execute_shell_with_capture` with a 2-second checkpoint
//! throttle, and converts ALL failures (non-zero exit, timeout, abort,
//! executionError) into `Err(AgentError)` — the agent loop encodes those into an
//! error `ToolResultMessage`.
//!
//! Throttle mirrors the TS `BASH_CHECKPOINT_INTERVAL_MS`: at most one
//! checkpoint update per 2-second window; a final flush always follows capture
//! resolution.

use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use tom_ai::types::{ConstrainedSamplingConfig, ConstrainedStrictness, Tool};

use crate::env::ExecutionEnv;
use crate::error::{ExecutionError, ExecutionErrorCode};
use crate::shell_output::{
    execute_shell_with_capture, ShellCaptureOptions, ShellCaptureProgress, ShellCaptureResult,
};
use crate::tools::tool_context::ExecutionToolContext;
use crate::truncate::{format_size, TruncationResult, DEFAULT_MAX_BYTES};

/// Throttle window for bash `on_update` flushes. Mirrors `BASH_UPDATE_THROTTLE_MS`.
const BASH_UPDATE_THROTTLE_MS: u64 = 2_000;

/// Default Bash deadline when neither the caller nor the host specifies one.
pub const DEFAULT_BASH_TIMEOUT_SECONDS: f64 = 120.0;

/// Input for the bash tool. Mirrors TS `BashToolInput`. `timeout` is in seconds.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct BashInput {
    /// Bash command to execute.
    pub command: String,
    /// Timeout in seconds (optional; uses the host default, otherwise 120 seconds).
    #[serde(default)]
    pub timeout: Option<f64>,
    /// Run the command in a long-lived session instead of to completion.
    ///
    /// When true, the command is started and the call returns after
    /// `yield_time_ms` with a `session_id` instead of waiting for it to finish.
    /// Use `write_stdin` with that id to poll for more output or to type input.
    /// This is what makes a five-minute build one tool call instead of five.
    #[serde(default)]
    pub session: Option<bool>,
    /// How long to wait for output before handing control back, in
    /// milliseconds. Only meaningful with `session: true`; defaults to
    /// [`DEFAULT_YIELD_MS`]. The command keeps running either way.
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    /// Allocate a pseudo-terminal for the session. A PTY is what makes progress
    /// bars, `isatty`-gated colour and programs that read stdin behave like a
    /// terminal. It also brings terminal semantics (`\r\n`, echo), so it is
    /// opt-in. Only meaningful with `session: true`.
    #[serde(default)]
    pub tty: Option<bool>,
}

/// Default `yield_time_ms` for a session command: long enough that a short
/// command finishes in the first call, short enough that a build hands control
/// back instead of blocking the turn.
pub const DEFAULT_YIELD_MS: u64 = 10_000;

/// Input for `bash_session`: poll, write to, or close a live session.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct BashSessionInput {
    /// The `session_id` returned by `bash` with `session: true`.
    pub session_id: String,
    /// Literal stdin to write. Omit to only poll for output.
    #[serde(default)]
    pub input: Option<String>,
    /// Append a newline after `input` (default true), so the model does not
    /// have to spell `\\n` for every command it types.
    #[serde(default)]
    pub newline: Option<bool>,
    /// How long to wait for output, in milliseconds. Defaults to
    /// [`DEFAULT_YIELD_MS`].
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    /// Terminate the session instead of writing to it.
    #[serde(default)]
    pub close: Option<bool>,
}

/// Structured details for the bash tool. Mirrors TS `BashToolDetails`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BashToolDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    /// Live session id, when the call opened or addressed one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether the session is still running, so the model knows to poll again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// Exit code, once the session has finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Mutable execution description passed to the optional prepare hook.
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub inherit_env: bool,
    /// Interpreter to run `command` with. `None` keeps the platform default
    /// shell; a `prepare` hook (or `shellPath`) can override it.
    pub shell: Option<crate::env::ShellOverride>,
}

/// Async hook called immediately before shell execution, matching native Pi's
/// `BashPrepare` extension point.
pub type BashPrepare =
    Arc<dyn Fn(&mut BashExecution) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Options for `create_bash_tool`. Mirrors TS `BashToolOptions`.
#[derive(Clone, Default)]
pub struct BashToolOptions {
    /// Prepended to the command (separated by `\n`) before execution.
    pub command_prefix: Option<String>,
    /// Fallback timeout in seconds applied when the model doesn't pass one.
    /// `None` uses `DEFAULT_BASH_TIMEOUT_SECONDS`, not an unlimited deadline.
    /// A model-supplied timeout still wins.
    pub default_timeout: Option<f64>,
    /// Interpreter used when neither `prepare` nor the caller sets one.
    /// This is how `settings.json`'s `shellPath` reaches the tool.
    pub shell: Option<crate::env::ShellOverride>,
    /// Optional async hook that can adjust command, cwd, and environment.
    pub prepare: Option<BashPrepare>,
    /// Model-specific output budget. `None` keeps the 50 KB / 2000 line default.
    pub truncation_policy: Option<crate::truncate::TruncationPolicy>,
}

/// The built-in `bash` tool. Holds an `Arc<dyn ExecutionEnv>` (read-only view
/// suffices — bash does not touch the mutation queue).
pub struct BashTool {
    schema: Tool,
    env: Arc<dyn ExecutionEnv>,
    options: BashToolOptions,
    /// Live sessions opened by `session: true`. Owned here so a session lives
    /// exactly as long as the tool, which lives as long as the agent session.
    sessions: Arc<crate::pty_session::SessionRegistry>,
}

impl BashTool {
    fn schema(default_timeout: f64, policy: Option<crate::truncate::TruncationPolicy>) -> Tool {
        let mut params = serde_json::to_value(schemars::schema_for!(BashInput)).unwrap_or_default();
        params["properties"]["timeout"]["description"] =
            format!("Timeout in seconds (optional, defaults to {default_timeout} seconds).").into();
        let options = crate::truncate::TruncationOptions::from_policy(policy);
        let budget = match policy {
            Some(policy) => policy.describe(),
            None => format!("{}KB", DEFAULT_MAX_BYTES / 1024),
        };
        Tool {
            name: "bash".to_string(),
            description: format!(
                "Execute a Bash command in the current working directory using Bash/POSIX syntax, even on Windows. PowerShell cmdlets (Get-ChildItem, Select-Object, Select-String) are not valid here; use ls, find, rg, and POSIX pipelines instead. Returns stdout and \
                 stderr. Output is truncated to last {} lines or {budget} \
                 (whichever is hit first). If truncated, full output is saved to a temp file. \
                 Defaults to a {default_timeout}-second timeout; provide timeout in seconds to override it. \
                 For anything that may outlive the timeout (a build, a test suite, a server), pass \
                 `session: true`: the call returns after `yield_time_ms` with a `session_id` and the \
                 command keeps running, so you poll it with `bash_session` instead of re-running it. \
                 Add `tty: true` when the program needs a terminal (progress bars, colour, stdin).",
                options.max_lines_or_default()
            ),
            parameters: tom_ai::types::Schema::new(params),
            constrained_sampling: Some(ConstrainedSamplingConfig::JsonSchema {
                strict: ConstrainedStrictness::Prefer,
            }),
        }
    }
}

/// Build the `bash` tool from a context. Mirrors TS `createBashTool`.
pub fn create_bash_tool(
    context: &ExecutionToolContext,
    options: Option<BashToolOptions>,
) -> Arc<dyn AgentTool> {
    create_bash_tool_with_sessions(
        context,
        options,
        Arc::new(crate::pty_session::SessionRegistry::new()),
    )
}

/// Build the `bash` tool on a caller-owned session registry.
///
/// A front end that also wants the companion `bash_session` tool passes the
/// registry in, so both tools address the same sessions. `create_bash_tool`
/// makes a private one, which is right when nothing else needs to poll.
pub fn create_bash_tool_with_sessions(
    context: &ExecutionToolContext,
    options: Option<BashToolOptions>,
    sessions: Arc<crate::pty_session::SessionRegistry>,
) -> Arc<dyn AgentTool> {
    let options = options.unwrap_or_default();
    Arc::new(BashTool {
        schema: BashTool::schema(
            options
                .default_timeout
                .unwrap_or(DEFAULT_BASH_TIMEOUT_SECONDS),
            options.truncation_policy,
        ),
        env: context.env().clone(),
        options,
        sessions,
    })
}

/// The `bash_session` companion tool: poll, write to, or close a live session.
pub struct BashSessionTool {
    schema: Tool,
    sessions: Arc<crate::pty_session::SessionRegistry>,
}

/// Build the companion `bash_session` tool over the same registry as a `bash`
/// tool.
pub fn create_bash_session_tool(
    sessions: Arc<crate::pty_session::SessionRegistry>,
) -> Arc<dyn AgentTool> {
    Arc::new(BashSessionTool {
        schema: BashSessionTool::schema(),
        sessions,
    })
}

impl BashSessionTool {
    fn schema() -> Tool {
        let params =
            serde_json::to_value(schemars::schema_for!(BashSessionInput)).unwrap_or_default();
        Tool {
            name: crate::tools::tool_context::BASH_SESSION_TOOL_NAME.to_string(),
            description: "Poll, write to, or close a session started with `bash` and \
                          `session: true`. Omit `input` to just read new output; pass `input` to \
                          type into the process (a `y` to a prompt, more stdin to a REPL); set \
                          `close` to terminate it. Returns whatever output arrived within \
                          `yield_time_ms` plus whether the process is still running."
                .to_string(),
            parameters: tom_ai::types::Schema::new(params),
            constrained_sampling: Some(ConstrainedSamplingConfig::JsonSchema {
                strict: ConstrainedStrictness::Prefer,
            }),
        }
    }
}

#[async_trait]
impl AgentTool for BashSessionTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        crate::tools::tool_context::BASH_SESSION_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: BashSessionInput = serde_json::from_value(params)
            .map_err(|e| AgentError::Validation(format!("bash_session input invalid: {e}")))?;
        let session = self.sessions.get(&input.session_id).ok_or_else(|| {
            AgentError::Validation(format!(
                "no live session {:?}; it may have exited and been reaped",
                input.session_id
            ))
        })?;

        if input.close.unwrap_or(false) {
            self.sessions.close(&input.session_id);
            return Ok(AgentToolResult {
                content: vec![TextContentOrImage::text(format!(
                    "Closed session {}.",
                    input.session_id
                ))],
                details: serde_json::json!({
                    "session_id": input.session_id,
                    "closed": true,
                }),
                ..Default::default()
            });
        }

        if let Some(text) = input.input.as_deref() {
            session
                .write(text, input.newline.unwrap_or(true))
                .map_err(|error| AgentError::Tool(error.to_string()))?;
        }

        let wait = Some(Duration::from_millis(
            input.yield_time_ms.unwrap_or(DEFAULT_YIELD_MS),
        ));
        let chunk = session.read(wait).await;
        let running = !session.has_exited() && !session.was_closed();
        let mut text = chunk.output.clone();
        if text.is_empty() {
            text.push_str("(no new output)");
        }
        if chunk.dropped_bytes > 0 {
            text.push_str(&format!(
                "\n\n[{} of earlier output was dropped: the session buffer is capped]",
                format_size(chunk.dropped_bytes as usize)
            ));
        }
        if running {
            text.push_str(&format!(
                "\n\n[session {} still running; call bash_session again to poll]",
                input.session_id
            ));
        } else {
            let code = chunk
                .exit_code
                .map(|code| format!(" with exit code {code}"))
                .unwrap_or_default();
            text.push_str(&format!("\n\n[session {} finished{code}]", input.session_id));
        }
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text(text)],
            details: serde_json::json!({
                "session_id": input.session_id,
                "running": running,
                "exit_code": chunk.exit_code,
                "dropped_bytes": chunk.dropped_bytes,
            }),
            ..Default::default()
        })
    }
}

impl BashTool {
    /// Start a command on a live session and hand control back after
    /// `yield_time_ms`.
    ///
    /// The command is *not* run to completion: whatever output arrived in the
    /// window is returned, along with the session id the model uses to poll.
    /// The process keeps running in the background, which is the whole point —
    /// a five-minute build costs one tool call, not five.
    async fn execute_in_session(
        &self,
        input: BashInput,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let mut execution = BashExecution {
            command: match &self.options.command_prefix {
                Some(p) => format!("{p}\n{}", input.command),
                None => input.command,
            },
            cwd: self.env.cwd().to_path_buf(),
            env: HashMap::new(),
            inherit_env: true,
            shell: self.options.shell.clone(),
        };
        if let Some(prepare) = &self.options.prepare {
            prepare(&mut execution).await.map_err(AgentError::Tool)?;
        }

        let start = crate::pty_session::SessionStart {
            command: execution.command,
            cwd: execution.cwd,
            env: execution.env,
            inherit_env: execution.inherit_env,
            shell: execution.shell,
            tty: input.tty.unwrap_or(false),
            rows: 24,
            cols: 80,
        };
        let session = self
            .sessions
            .open(start)
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        let session_id = session.id().to_string();

        emit_update(&on_update, String::new(), None, None);
        let wait = Duration::from_millis(input.yield_time_ms.unwrap_or(DEFAULT_YIELD_MS));
        let chunk = session.read(Some(wait)).await;
        let running = !session.has_exited() && !session.was_closed();

        let mut text = chunk.output.clone();
        if text.is_empty() {
            text.push_str(if running {
                "(no output yet)"
            } else {
                "(no output)"
            });
        }
        if chunk.dropped_bytes > 0 {
            text.push_str(&format!(
                "\n\n[{} of earlier output was dropped: the session buffer is capped]",
                format_size(chunk.dropped_bytes as usize)
            ));
        }
        if running {
            text.push_str(&format!(
                "\n\n[session {session_id} still running after {}ms; call bash_session with \
                 this session_id to read more output or write input]",
                wait.as_millis()
            ));
        } else {
            let code = chunk
                .exit_code
                .map(|code| format!(" with exit code {code}"))
                .unwrap_or_default();
            text.push_str(&format!("\n\n[session {session_id} finished{code}]"));
        }

        // Cancellation closes the session instead of leaving an orphan: the
        // signal outlives the call, and a cancelled run must not leak a process.
        if signal.is_cancelled() {
            self.sessions.close(&session_id);
            return Err(AgentError::Tool("Command aborted".to_string()));
        }

        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text(text)],
            details: serde_json::to_value(BashToolDetails {
                truncation: None,
                full_output_path: None,
                session_id: Some(session_id),
                running: Some(running),
                exit_code: chunk.exit_code,
            })
            .unwrap_or(serde_json::Value::Null),
            ..Default::default()
        })
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "bash"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: BashInput = serde_json::from_value(params)
            .map_err(|e| AgentError::Validation(format!("bash input invalid: {e}")))?;
        // A session command is a different lifetime from a one-shot command:
        // it returns after `yield_time_ms` and keeps running. Split here so the
        // one-shot path below stays exactly as it was.
        if input.session.unwrap_or(false) {
            return self.execute_in_session(input, signal, on_update).await;
        }
        let timeout = Some(resolve_timeout(
            input.timeout,
            self.options.default_timeout,
        )?);
        let mut execution = BashExecution {
            command: match &self.options.command_prefix {
                Some(p) => format!("{p}\n{}", input.command),
                None => input.command,
            },
            cwd: self.env.cwd().to_path_buf(),
            env: HashMap::new(),
            inherit_env: true,
            shell: self.options.shell.clone(),
        };
        if let Some(prepare) = &self.options.prepare {
            prepare(&mut execution).await.map_err(AgentError::Tool)?;
        }
        let command = execution.command;

        // Initial empty update (mirrors TS `onUpdate?.({ content: [], details: undefined })`).
        emit_update(&on_update, String::new(), None, None);

        // The throttle state. Shared between the on_chunk callback (which marks
        // dirty + schedules a flush) and the post-capture final flush.
        let throttle = Arc::new(Mutex::new(ThrottleState {
            dirty: false,
            last_flush: Instant::now(),
            latest_output: String::new(),
            latest_truncation: None,
            latest_full_path: None,
        }));

        let throttle_for_cb = throttle.clone();
        let on_update_for_cb = on_update.clone();
        let on_chunk: Box<dyn FnMut(&str, &dyn Fn() -> ShellCaptureProgress) + Send> = Box::new(
            move |_chunk, get_progress: &dyn Fn() -> ShellCaptureProgress| {
                let progress = get_progress();
                // Snapshot the fields we need from the progress before touching
                // the async mutex — the on_chunk callback runs on whatever thread
                // the env's stdout/stderr callback is invoked from, which may be
                // a runtime worker. Use try_lock so we never block (and never
                // panic on a held lock); a missed flush is fine — the final
                // post-capture flush always emits the terminal state.
                let new_output = progress.output.clone();
                let new_truncation = if progress.truncation.truncated {
                    Some(progress.truncation.clone())
                } else {
                    None
                };
                let new_full_path = progress
                    .full_output_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned());
                let mut st = match throttle_for_cb.try_lock() {
                    Ok(g) => g,
                    Err(_) => return, // busy — skip this update; final flush covers it
                };
                st.latest_output = new_output;
                st.latest_truncation = new_truncation;
                st.latest_full_path = new_full_path;
                st.dirty = true;
                let elapsed = st.last_flush.elapsed();
                let throttle_duration = Duration::from_millis(BASH_UPDATE_THROTTLE_MS);
                if elapsed >= throttle_duration {
                    flush_now(&mut st, &on_update_for_cb);
                } else {
                    // Schedule a delayed flush (mirrors TS setTimeout)
                    let remaining = throttle_duration - elapsed;
                    let throttle_clone = throttle_for_cb.clone();
                    let on_update_clone = on_update_for_cb.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(remaining).await;
                        let mut st = throttle_clone.lock().await;
                        flush_now(&mut st, &on_update_clone);
                    });
                }
            },
        );

        let cwd = execution.cwd;
        let env = self.env.clone();
        let capture_opts = ShellCaptureOptions {
            shell: execution.shell,
            cwd: Some(cwd),
            env: Some(execution.env),
            inherit_env: execution.inherit_env,
            timeout,
            truncation: crate::truncate::TruncationOptions::from_policy(
                self.options.truncation_policy,
            ),
            cancel: Some(&signal),
            on_chunk: Some(on_chunk),
            return_execution_errors: true,
        };

        // execute_shell_with_capture wants `&Arc<dyn ExecutionEnv>`.
        let capture = execute_shell_with_capture(&env, &command, capture_opts)
            .await
            .map_err(exec_err_to_agent)?;

        // Final flush + emit the terminal output via on_update.
        {
            let mut st = throttle.lock().await;
            st.latest_output = capture.output.clone();
            st.latest_truncation = if capture.truncated {
                Some(capture.truncation.clone())
            } else {
                None
            };
            st.latest_full_path = capture
                .full_output_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            st.dirty = true;
            flush_now(&mut st, &on_update);
        }

        // Build the terminal output text + details, applying truncation notices.
        let (output_text, details) = build_bash_output(&capture);
        let details_value = serde_json::to_value(details).unwrap_or(serde_json::Value::Null);

        // Resolution: cancelled → aborted; timeout → timeout; other exec error;
        // non-zero exit → Tool error. All become Err(AgentError).
        let append_status = |status: String| -> String {
            if output_text.is_empty() {
                status
            } else {
                format!("{output_text}\n\n{status}")
            }
        };
        if capture.cancelled {
            return Err(AgentError::Tool(append_status("Command aborted".into())));
        }
        if let Some(ee) = &capture.execution_error {
            if ee.code == ExecutionErrorCode::Timeout {
                // `timeout` is `Option<f64>` in seconds; `{timeout:?}` used to render
                // `Some(2.0)`, so the message read "timed out after Some(2.0) seconds".
                let limit = match timeout {
                    Some(secs) => format!("after {secs}s"),
                    None => "".to_string(),
                };
                return Err(AgentError::Tool(append_status(format!(
                    "Command timed out {limit}"
                ))));
            }
            return Err(AgentError::Tool(ee.to_string()));
        }
        if let Some(code) = capture.exit_code {
            if code != 0 {
                return Err(AgentError::Tool(append_status(format!(
                    "Command exited with code {code}"
                ))));
            }
        }
        let text = if output_text.is_empty() {
            "(no output)".to_string()
        } else {
            output_text
        };
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text(text)],
            details: details_value,
            ..Default::default()
        })
    }
}

/// Throttle state, shared between the on_chunk callback and the final flush.
struct ThrottleState {
    dirty: bool,
    last_flush: Instant,
    latest_output: String,
    latest_truncation: Option<TruncationResult>,
    latest_full_path: Option<String>,
}

/// Flush a dirty throttle state by emitting an `on_update`. Mirrors
/// `emitOutputUpdate`.
fn flush_now(st: &mut ThrottleState, on_update: &Arc<dyn Fn(ToolResultPartial) + Send + Sync>) {
    if !st.dirty {
        return;
    }
    st.dirty = false;
    st.last_flush = Instant::now();
    emit_update(
        on_update,
        st.latest_output.clone(),
        st.latest_truncation.clone(),
        st.latest_full_path.clone(),
    );
}

/// Emit a single `on_update` payload built from the given capture snapshot.
/// Mirrors the TS `onUpdate({ content:[{text:progress.output}], details:{...} })`.
fn emit_update(
    on_update: &Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    output: String,
    truncation: Option<TruncationResult>,
    full_output_path: Option<String>,
) {
    let details = match (&truncation, &full_output_path) {
        (None, None) => serde_json::Value::Null,
        _ => {
            let details = BashToolDetails {
                truncation,
                full_output_path,
                session_id: None,
                running: None,
                exit_code: None,
            };
            serde_json::to_value(details).unwrap_or(serde_json::Value::Null)
        }
    };
    on_update(AgentToolResult {
        content: vec![TextContentOrImage::text(output)],
        details,
        ..Default::default()
    });
}

/// Build the terminal `(output_text, details)` from a capture, appending the
/// truncation continuation notice (mirrors the TS block).
fn build_bash_output(capture: &ShellCaptureResult) -> (String, BashToolDetails) {
    let mut output_text = capture.output.clone();
    let details = if capture.truncated {
        let t = &capture.truncation;
        let start_line = t.total_lines.saturating_sub(t.output_lines) + 1;
        let end_line = t.total_lines;
        let path_str = capture
            .full_output_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        if t.last_line_partial {
            let last_line_size = format_size(capture.last_line_bytes);
            output_text.push_str(&format!(
                "\n\n[Showing last {} of line {end_line} (line is {last_line_size}). Full output: {}]",
                format_size(t.output_bytes),
                path_str.as_deref().unwrap_or("?")
            ));
        } else if matches!(
            t.truncated_by,
            Some(crate::truncate::TruncationLimit::Lines)
        ) {
            output_text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {}. Full output: {}]",
                t.total_lines,
                path_str.as_deref().unwrap_or("?")
            ));
        } else {
            output_text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit). Full output: {}]",
                t.total_lines,
                format_size(t.max_bytes),
                path_str.as_deref().unwrap_or("?")
            ));
        }
        BashToolDetails {
            truncation: Some(capture.truncation.clone()),
            full_output_path: path_str,
            session_id: None,
            running: None,
            exit_code: None,
        }
    } else {
        BashToolDetails {
            truncation: None,
            full_output_path: None,
            session_id: None,
            running: None,
            exit_code: None,
        }
    };
    (output_text, details)
}

/// Resolve and validate the effective deadline before preparing a command.
fn resolve_timeout(timeout: Option<f64>, default_timeout: Option<f64>) -> Result<f64, AgentError> {
    let secs = timeout
        .or(default_timeout)
        .unwrap_or(DEFAULT_BASH_TIMEOUT_SECONDS);
    if !secs.is_finite() || secs <= 0.0 {
        return Err(AgentError::Tool(
            "Invalid timeout: must be a finite number of seconds".into(),
        ));
    }
    let max = 2_147_483_647.0 / 1000.0;
    if secs > max {
        return Err(AgentError::Tool(format!(
            "Invalid timeout: maximum is {max} seconds"
        )));
    }
    Ok(secs)
}

fn exec_err_to_agent(e: ExecutionError) -> AgentError {
    AgentError::Tool(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_and_null_timeout_use_tool_default() {
        for params in [
            serde_json::json!({ "command": "noop" }),
            serde_json::json!({ "command": "noop", "timeout": null }),
        ] {
            let input: BashInput = serde_json::from_value(params).unwrap();
            assert_eq!(resolve_timeout(input.timeout, None).unwrap(), 120.0);
        }
    }

    #[test]
    fn explicit_timeout_overrides_host_default() {
        assert_eq!(resolve_timeout(None, Some(30.0)).unwrap(), 30.0);
        assert_eq!(resolve_timeout(Some(300.0), Some(30.0)).unwrap(), 300.0);
        assert_eq!(resolve_timeout(Some(300.0), None).unwrap(), 300.0);
    }

    #[test]
    fn effective_timeout_must_be_valid() {
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, 2_147_484.0] {
            assert!(resolve_timeout(Some(invalid), None).is_err());
            assert!(resolve_timeout(None, Some(invalid)).is_err());
            assert_eq!(resolve_timeout(Some(10.0), Some(invalid)).unwrap(), 10.0);
        }
    }
}
