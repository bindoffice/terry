//! The QuickJS sandbox that runs codemode scripts.
//!
//! A script's only capability is calling the tools and globals the host injects:
//! there is no file system, network, timer, or module access. Every execution
//! gets a fresh `rquickjs` runtime, so nothing a script does survives it.
//!
//! **Threading.** QuickJS is synchronous, so the VM owns a blocking thread and the
//! async side never touches it. Each call the script makes is spawned onto the
//! tokio runtime — so `Promise.all` over 200 reads really does run them
//! concurrently — and its answer is handed back through a channel the VM thread
//! blocks on. Between answers the VM thread drains the job queue, which is what
//! makes `await` work at all.
//!
//! **Limits.** The deadline and the caller's cancellation token both set a flag
//! the VM polls through QuickJS's interrupt handler, so `while (true) {}` ends
//! instead of hanging the run; the same flag is checked while waiting for an
//! answer, which the interrupt handler cannot cover. `memory_limit` caps the heap,
//! and the prelude caps script output and `store()` size.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{Context, Function, Object, Persistent, Promise, Runtime, Value as JsValue};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::prelude::CODEMODE_PRELUDE;

/// Default heap limit. A runaway script must not grow the host process; overruns
/// throw `InternalError: out of memory` inside the script.
pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// Characters of text and base64 image data one script may produce.
pub const MAX_OUTPUT_CHARS: usize = 16 * 1024 * 1024;
/// Items one script may produce.
pub const MAX_OUTPUT_ITEMS: usize = 100_000;
/// Largest single `store()` value, in JSON characters.
pub const MAX_STORE_VALUE_CHARS: usize = 256 * 1024;
/// Largest total `store()`, in JSON characters.
pub const MAX_STORE_TOTAL_CHARS: usize = 1024 * 1024;

/// How often the VM thread re-checks the deadline and the cancel token while it
/// waits for a tool answer.
const WAIT_POLL: Duration = Duration::from_millis(25);

/// One tool as a script sees it.
#[derive(Debug, Clone)]
pub struct CodemodeToolSpec {
    pub name: String,
    /// The JavaScript identifier scripts use (`tools.<js_name>`).
    pub js_name: String,
    pub description: String,
}

/// A host function scripts call as a top-level global (or `ns.member`).
#[derive(Debug, Clone)]
pub struct CodemodeGlobalSpec {
    pub name: String,
    /// Pass all call arguments as an array instead of the first one.
    pub spread: bool,
}

/// What one script produced.
#[derive(Debug, Clone, PartialEq)]
pub enum CodemodeOutputItem {
    /// `text(value)` or the returned value.
    Text(String),
    /// `console.*`.
    Console(String),
    Image {
        data: String,
        mime_type: String,
    },
}

/// A tool or global call the script made, for the result's details.
#[derive(Debug, Clone, PartialEq)]
pub struct CodemodeCallRecord {
    pub name: String,
    /// Compact JSON of the arguments, cut for display.
    pub args: String,
    pub status: CodemodeCallStatus,
    pub duration: Duration,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodemodeCallStatus {
    Ok,
    Error,
    Cancelled,
}

/// Why a script failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodemodeErrorKind {
    /// The script threw, or did not parse.
    Script,
    /// The deadline passed.
    Timeout,
    /// The caller cancelled.
    Aborted,
    /// The VM itself failed.
    Sandbox,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodemodeError {
    pub kind: CodemodeErrorKind,
    pub message: String,
}

/// The result of one script. `error: None` means the script completed.
#[derive(Debug, Clone, Default)]
pub struct CodemodeOutcome {
    pub output: Vec<CodemodeOutputItem>,
    pub calls: Vec<CodemodeCallRecord>,
    /// `store()` changes in write order; a `None` value deletes the key.
    pub store_writes: Vec<(String, Option<Value>)>,
    /// The script's returned value, when it returned one.
    pub value: Option<Value>,
    pub error: Option<CodemodeError>,
    pub elapsed: Duration,
}

impl CodemodeOutcome {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// The host side of a script's calls. Both entry points run on the async runtime
/// and may run concurrently, so an implementation must be `Send + Sync`. An `Err`
/// becomes a rejection inside the script.
pub trait CodemodeHost: Send + Sync {
    fn call_tool(
        &self,
        name: String,
        args: Value,
    ) -> futures::future::BoxFuture<'static, Result<Value, String>>;

    fn call_global(
        &self,
        name: String,
        args: Value,
    ) -> futures::future::BoxFuture<'static, Result<Value, String>>;
}

/// How to run one script.
pub struct CodemodeSandboxOptions {
    pub tools: Vec<CodemodeToolSpec>,
    pub globals: Vec<CodemodeGlobalSpec>,
    pub store: BTreeMap<String, Value>,
    /// Deadline for the whole script, tool calls included.
    pub timeout: Option<Duration>,
    pub memory_limit: usize,
}

impl Default for CodemodeSandboxOptions {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            globals: Vec::new(),
            store: BTreeMap::new(),
            timeout: None,
            memory_limit: DEFAULT_MEMORY_LIMIT_BYTES,
        }
    }
}

/// Run `code` in a fresh sandbox.
pub async fn run_codemode_script(
    options: CodemodeSandboxOptions,
    code: String,
    host: Arc<dyn CodemodeHost>,
    signal: CancellationToken,
) -> CodemodeOutcome {
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle,
        Err(error) => {
            return CodemodeOutcome {
                error: Some(CodemodeError {
                    kind: CodemodeErrorKind::Sandbox,
                    message: format!("codemode needs a tokio runtime: {error}"),
                }),
                ..Default::default()
            }
        }
    };
    let started = Instant::now();
    // The VM gets its own OS thread rather than a runtime worker: QuickJS is
    // synchronous, so it must block *somewhere*, and blocking a worker would
    // starve the very tasks that answer its tool calls. A dedicated thread also
    // keeps this usable from a current-thread runtime, which `spawn_blocking`
    // would not.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("codemode-sandbox".to_string())
        .spawn(move || {
            let outcome = match Vm::new(options, host, &signal, &handle) {
                Some(vm) => vm.run(code, started),
                None => CodemodeOutcome {
                    error: Some(CodemodeError {
                        kind: CodemodeErrorKind::Sandbox,
                        message: "could not start the JavaScript runtime".to_string(),
                    }),
                    elapsed: Instant::now().saturating_duration_since(started),
                    ..Default::default()
                },
            };
            let _ = done_tx.send(outcome);
        });
    if let Err(error) = thread {
        return CodemodeOutcome {
            error: Some(CodemodeError {
                kind: CodemodeErrorKind::Sandbox,
                message: format!("could not start the codemode sandbox thread: {error}"),
            }),
            elapsed: started.elapsed(),
            ..Default::default()
        };
    }
    match done_rx.await {
        Ok(outcome) => outcome,
        Err(_) => CodemodeOutcome {
            error: Some(CodemodeError {
                kind: CodemodeErrorKind::Sandbox,
                message: "the codemode sandbox thread stopped without reporting".to_string(),
            }),
            elapsed: started.elapsed(),
            ..Default::default()
        },
    }
}

/// What the prelude reports through the bridge; recorded on the VM thread.
enum BridgeEvent {
    Call {
        id: u64,
        name: String,
        args: Value,
        global: bool,
    },
    Output(CodemodeOutputItem),
    Done {
        ok: bool,
        payload: String,
        writes: String,
    },
}

#[derive(Default)]
struct VmState {
    events: Vec<BridgeEvent>,
    outputs: Vec<CodemodeOutputItem>,
    done: Option<(bool, String, String)>,
}

/// One call's answer, on its way back to the VM.
struct CallReply {
    id: u64,
    result: Result<Value, String>,
    elapsed: Duration,
    cancelled: bool,
}

/// A pending call record plus the handle of the task that answers it.
struct PendingCall {
    /// Index into `calls`.
    index: usize,
    handle: JoinHandle<()>,
}

struct Vm {
    options: CodemodeSandboxOptions,
    // Drop order is declaration order, so the context must go first: freeing the
    // runtime while a context still references it trips QuickJS's own assertion.
    context: Context,
    runtime: Runtime,
    /// `settle()` and `stalled()` of the prelude API object.
    api: Option<Persistent<Object<'static>>>,
    state: Rc<RefCell<VmState>>,
    replies: Receiver<CallReply>,
    reply_tx: std::sync::mpsc::Sender<CallReply>,
    host: Arc<dyn CodemodeHost>,
    signal: CancellationToken,
    /// Child of `signal` for the script: cancelling it stops the calls a finished
    /// script left running.
    calls_signal: CancellationToken,
    handle: tokio::runtime::Handle,
    deadline: Option<Instant>,
    expired: Arc<AtomicBool>,
    calls: Vec<CodemodeCallRecord>,
    pending: BTreeMap<u64, PendingCall>,
    elapsed: Duration,
}

impl Vm {
    fn new(
        options: CodemodeSandboxOptions,
        host: Arc<dyn CodemodeHost>,
        signal: &CancellationToken,
        handle: &tokio::runtime::Handle,
    ) -> Option<Self> {
        let runtime = Runtime::new().ok()?;
        runtime.set_memory_limit(options.memory_limit);
        let context = Context::full(&runtime).ok()?;
        let (reply_tx, replies) = channel();
        let expired = Arc::new(AtomicBool::new(false));
        let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
        {
            let expired = Arc::clone(&expired);
            let signal = signal.clone();
            runtime.set_interrupt_handler(Some(Box::new(move || {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    expired.store(true, Ordering::SeqCst);
                }
                expired.load(Ordering::SeqCst) || signal.is_cancelled()
            })));
        }
        Some(Self {
            calls_signal: signal.child_token(),
            signal: signal.clone(),
            options,
            context,
            runtime,
            api: None,
            state: Rc::new(RefCell::new(VmState::default())),
            replies,
            reply_tx,
            host,
            handle: handle.clone(),
            deadline,
            expired,
            calls: Vec::new(),
            pending: BTreeMap::new(),
            elapsed: Duration::ZERO,
        })
    }

    fn run(mut self, code: String, started: Instant) -> CodemodeOutcome {
        if let Err(error) = self.start(code) {
            // A script that never awaits runs entirely inside `start`, so the
            // interrupt handler fires there: report the deadline or the abort
            // rather than calling it a sandbox failure.
            let error = self.classify_interrupt(error);
            return self.finish(Some(error), started);
        }
        let interrupt = self.drive().map(|kind| CodemodeError {
            kind,
            message: match kind {
                CodemodeErrorKind::Timeout => format!(
                    "Execution timed out after {:.1} seconds",
                    self.options.timeout.unwrap_or_default().as_secs_f64()
                ),
                CodemodeErrorKind::Aborted => "Execution aborted".to_string(),
                _ => "The sandbox failed".to_string(),
            },
        });
        self.finish(interrupt, started)
    }

    /// Turn an interrupt (deadline or cancellation) into its own error kind.
    fn classify_interrupt(&self, error: CodemodeError) -> CodemodeError {
        if self.expired.load(Ordering::SeqCst) {
            return CodemodeError {
                kind: CodemodeErrorKind::Timeout,
                message: format!(
                    "Execution timed out after {:.1} seconds",
                    self.options.timeout.unwrap_or_default().as_secs_f64()
                ),
            };
        }
        if self.signal.is_cancelled() {
            return CodemodeError {
                kind: CodemodeErrorKind::Aborted,
                message: "Execution aborted".to_string(),
            };
        }
        error
    }

    /// Install the bridge and the prelude, then start the script.
    fn start(&mut self, code: String) -> Result<(), CodemodeError> {
        let config = self.config_json();
        let state = Rc::clone(&self.state);
        let api_slot: Rc<RefCell<Option<Persistent<Object<'static>>>>> =
            Rc::new(RefCell::new(None));
        let api_out = Rc::clone(&api_slot);
        let context = &self.context;
        context.with(move |ctx| -> Result<(), CodemodeError> {
            let bridge = Function::new(
                ctx.clone(),
                move |kind: String, a: JsValue<'_>, b: JsValue<'_>, c: JsValue<'_>| {
                    let mut state = state.borrow_mut();
                    match kind.as_str() {
                        "call" | "global" => {
                            let id = a.as_number().unwrap_or_default() as u64;
                            let name = string_of(&b);
                            let args = serde_json::from_str(&string_of(&c)).unwrap_or(Value::Null);
                            state.events.push(BridgeEvent::Call {
                                id,
                                name,
                                args,
                                global: kind == "global",
                            });
                        }
                        "output" => {
                            let item = output_item(&string_of(&a), string_of(&b), string_of(&c));
                            state.events.push(BridgeEvent::Output(item));
                        }
                        "done" => state.events.push(BridgeEvent::Done {
                            ok: a.as_bool().unwrap_or(false),
                            payload: string_of(&b),
                            writes: string_of(&c),
                        }),
                        _ => {}
                    }
                },
            )
            .map_err(|error| sandbox_error(error.to_string()))?;
            let factory: Function<'_> = ctx
                .eval(CODEMODE_PRELUDE)
                .map_err(|error| exception_error(&ctx, error))?;
            let api_object: Object<'_> = factory
                .call((bridge, config))
                .map_err(|error| exception_error(&ctx, error))?;
            let settle: Function<'_> = api_object
                .get("settle")
                .map_err(|error| sandbox_error(error.to_string()))?;
            let track: Function<'_> = api_object
                .get("track")
                .map_err(|error| sandbox_error(error.to_string()))?;
            *api_out.borrow_mut() = Some(Persistent::save(&ctx, api_object));
            drop(settle);
            // The script is compiled here, as an async function body, so top-level
            // `await` and `return` work and a stack trace names the line the model
            // wrote. Evaluating the wrapped source directly (rather than through
            // `new Function`) keeps the prelude's own lines out of the count.
            let promise: Promise<'_> = ctx.eval(script_source(&code)).map_err(|error| {
                // A script that does not compile is the script's error, not the
                // sandbox's: the model wrote something it has to fix.
                let mut error = exception_error(&ctx, error);
                error.kind = CodemodeErrorKind::Script;
                error
            })?;
            track
                .call::<_, ()>((promise,))
                .map_err(|error| exception_error(&ctx, error))
        })?;
        self.api = api_slot.borrow_mut().take();
        Ok(())
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "tools": self.options.tools.iter().map(|tool| serde_json::json!({
                "name": tool.name,
                "jsName": tool.js_name,
                "description": tool.description,
            })).collect::<Vec<_>>(),
            "globals": self.options.globals.iter().map(|global| serde_json::json!({
                "name": global.name,
                "spread": global.spread,
            })).collect::<Vec<_>>(),
            "store": self.options.store,
            "limits": {
                "maxOutputChars": MAX_OUTPUT_CHARS,
                "maxOutputItems": MAX_OUTPUT_ITEMS,
                "maxStoreValueChars": MAX_STORE_VALUE_CHARS,
                "maxStoreTotalChars": MAX_STORE_TOTAL_CHARS,
            },
        })
        .to_string()
    }

    /// Drain JS jobs, dispatch the calls the script issued, and hand answers back
    /// until the script settles. Returns the reason it was interrupted, if it was.
    fn drive(&mut self) -> Option<CodemodeErrorKind> {
        loop {
            self.drain_jobs();
            if self.expired.load(Ordering::SeqCst) {
                return Some(CodemodeErrorKind::Timeout);
            }
            if self.signal.is_cancelled() {
                return Some(CodemodeErrorKind::Aborted);
            }
            self.dispatch();
            if self.state.borrow().done.is_some() {
                return None;
            }
            if self.pending.is_empty() {
                // Nothing can settle the script: no call in flight and no timers.
                // The prelude reports the stall itself, which lands in `done`.
                let stalled = self.call_api_bool("stalled");
                self.drain_jobs();
                self.dispatch();
                if stalled || self.state.borrow().done.is_some() {
                    return None;
                }
                return None;
            }
            match self.wait_for_reply() {
                Ok(reply) => self.settle(reply),
                Err(kind) => return Some(kind),
            }
        }
    }

    fn drain_jobs(&self) {
        while let Ok(true) = self.runtime.execute_pending_job() {}
    }

    /// Forward the calls the script issued to the host.
    fn dispatch(&mut self) {
        let events = std::mem::take(&mut self.state.borrow_mut().events);
        for event in events {
            match event {
                BridgeEvent::Call {
                    id,
                    name,
                    args,
                    global,
                } => {
                    let index = self.calls.len();
                    self.calls.push(CodemodeCallRecord {
                        name: name.clone(),
                        args: preview_args(&args),
                        status: CodemodeCallStatus::Ok,
                        duration: Duration::ZERO,
                        error: None,
                    });
                    let reply_tx = self.reply_tx.clone();
                    let host = Arc::clone(&self.host);
                    let signal = self.calls_signal.child_token();
                    let call_signal = signal.clone();
                    let join = self.handle.spawn(async move {
                        let started = Instant::now();
                        let call = if global {
                            host.call_global(name, args)
                        } else {
                            host.call_tool(name, args)
                        };
                        let result = tokio::select! {
                            biased;
                            _ = call_signal.cancelled() => {
                                Err("the script ended before this call answered".to_string())
                            }
                            result = call => result,
                        };
                        let _ = reply_tx.send(CallReply {
                            id,
                            result,
                            elapsed: started.elapsed(),
                            cancelled: call_signal.is_cancelled(),
                        });
                    });
                    self.pending.insert(
                        id,
                        PendingCall {
                            index,
                            handle: join,
                        },
                    );
                }
                BridgeEvent::Output(item) => self.state.borrow_mut().outputs.push(item),
                BridgeEvent::Done {
                    ok,
                    payload,
                    writes,
                } => {
                    self.state.borrow_mut().done = Some((ok, payload, writes));
                }
            }
        }
    }

    fn wait_for_reply(&self) -> Result<CallReply, CodemodeErrorKind> {
        loop {
            if let Some(deadline) = self.deadline {
                let now = Instant::now();
                if now >= deadline {
                    self.expired.store(true, Ordering::SeqCst);
                    return Err(CodemodeErrorKind::Timeout);
                }
                let remaining = deadline - now;
                match self.replies.recv_timeout(remaining.min(WAIT_POLL)) {
                    Ok(reply) => return Ok(reply),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return Err(CodemodeErrorKind::Sandbox),
                }
            }
            match self.replies.recv_timeout(WAIT_POLL) {
                Ok(reply) => return Ok(reply),
                Err(RecvTimeoutError::Timeout) => {
                    if self.expired.load(Ordering::SeqCst) {
                        return Err(CodemodeErrorKind::Timeout);
                    }
                    if self.signal.is_cancelled() {
                        return Err(CodemodeErrorKind::Aborted);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(CodemodeErrorKind::Sandbox),
            }
        }
    }

    fn settle(&mut self, reply: CallReply) {
        let Some(pending) = self.pending.remove(&reply.id) else {
            return;
        };
        let record = &mut self.calls[pending.index];
        record.duration = reply.elapsed;
        match &reply.result {
            Ok(_) => record.status = CodemodeCallStatus::Ok,
            Err(message) => {
                record.status = if reply.cancelled {
                    CodemodeCallStatus::Cancelled
                } else {
                    CodemodeCallStatus::Error
                };
                record.error = Some(truncate(message, 500));
            }
        }
        let payload = match &reply.result {
            Ok(value) => value.to_string(),
            Err(message) => message.clone(),
        };
        let ok = reply.result.is_ok();
        let id = reply.id;
        self.context.with(|ctx| {
            let Some(api) = self.api.clone() else {
                return;
            };
            let Ok(api) = api.restore(&ctx) else {
                return;
            };
            let Ok(settle) = api.get::<_, Function<'_>>("settle") else {
                return;
            };
            let _ = settle.call::<_, ()>((id, ok, payload.as_str(), ""));
        });
    }

    fn call_api_bool(&self, name: &str) -> bool {
        let Some(api) = self.api.clone() else {
            return false;
        };
        self.context.with(|ctx| {
            let Ok(api) = api.restore(&ctx) else {
                return false;
            };
            let Ok(found) = api.get::<_, Function<'_>>(name) else {
                return false;
            };
            found.call::<_, bool>(()).unwrap_or(false)
        })
    }

    fn finish(mut self, interrupt: Option<CodemodeError>, started: Instant) -> CodemodeOutcome {
        // Calls still in flight belong to a script that ended or was stopped; stop
        // them instead of letting them run on after the result is returned.
        self.calls_signal.cancel();
        for (_, pending) in std::mem::take(&mut self.pending) {
            pending.handle.abort();
        }
        for record in &mut self.calls {
            if record.status == CodemodeCallStatus::Ok && record.duration == Duration::ZERO {
                record.status = CodemodeCallStatus::Cancelled;
            }
        }
        self.elapsed = started.elapsed();
        // The prelude's API object holds a JS reference that must be released while
        // its context is still alive; `Persistent` has no drop hook of its own.
        if let Some(api) = self.api.take() {
            self.context.with(move |_ctx| drop(api));
        }
        let state = std::mem::take(&mut *self.state.borrow_mut());
        let (value, store_writes, script_error) = match &state.done {
            Some((true, payload, writes)) => (
                serde_json::from_str(payload).ok(),
                parse_writes(writes),
                None,
            ),
            Some((false, payload, _)) => (
                None,
                Vec::new(),
                Some(CodemodeError {
                    kind: CodemodeErrorKind::Script,
                    message: parse_error(payload),
                }),
            ),
            None => (
                None,
                Vec::new(),
                Some(CodemodeError {
                    kind: CodemodeErrorKind::Sandbox,
                    message: "the script ended without reporting a result".to_string(),
                }),
            ),
        };
        CodemodeOutcome {
            output: state.outputs,
            calls: self.calls,
            store_writes,
            value,
            error: interrupt.or(script_error),
            elapsed: self.elapsed,
        }
    }
}

/// Wrap a script as the body of an async function.
///
/// The wrapper shares the script's first line on purpose: `(async function(){` is
/// written on line 1 before the script's own text, so line numbers in a stack
/// trace match the input. Only column numbers on line 1 shift.
fn script_source(code: &str) -> String {
    format!("(async function(){{\"use strict\";{code}\n}})()")
}

fn sandbox_error(message: String) -> CodemodeError {
    CodemodeError {
        kind: CodemodeErrorKind::Sandbox,
        message,
    }
}

fn output_item(kind: &str, data: String, mime_type: String) -> CodemodeOutputItem {
    match kind {
        "console" => CodemodeOutputItem::Console(data),
        "image" => CodemodeOutputItem::Image { data, mime_type },
        _ => CodemodeOutputItem::Text(data),
    }
}

fn exception_error(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> CodemodeError {
    let thrown: JsValue<'_> = ctx.catch();
    let message = if let Some(exception) = thrown.as_exception() {
        exception.message().unwrap_or_else(|| exception.to_string())
    } else if let Some(string) = thrown.as_string() {
        string.to_string().unwrap_or_default()
    } else {
        error.to_string()
    };
    sandbox_error(message)
}

fn string_of(value: &JsValue<'_>) -> String {
    match value.as_string() {
        Some(string) => string.to_string().unwrap_or_default(),
        None => match value.as_number() {
            Some(number) => format!("{number}"),
            None => String::new(),
        },
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{cut}...")
}

fn preview_args(args: &Value) -> String {
    truncate(&serde_json::to_string(args).unwrap_or_default(), 200)
}

/// A script error arrives as `{ name, message, stack }`; the stack already starts
/// with `Name: message`.
fn parse_error(payload: &str) -> String {
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => value
            .get("stack")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                let message = value.get("message").and_then(Value::as_str)?;
                Some(match value.get("name").and_then(Value::as_str) {
                    Some(name) => format!("{name}: {message}"),
                    None => message.to_string(),
                })
            })
            .unwrap_or_else(|| payload.to_string()),
        Err(_) => payload.to_string(),
    }
}

/// `store()` writes arrive as `[[key], [key, json], ...]`; a missing value deletes.
fn parse_writes(payload: &str) -> Vec<(String, Option<Value>)> {
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let mut parts = entry.as_array()?.iter();
            let key = parts.next()?.as_str()?.to_string();
            let value = parts.next().and_then(|value| {
                value
                    .as_str()
                    .and_then(|json| serde_json::from_str::<Value>(json).ok())
            });
            Some((key, value))
        })
        .collect()
}
