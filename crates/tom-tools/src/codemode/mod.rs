//! Code mode: run model-written JavaScript in a QuickJS sandbox whose only
//! capability is calling injected tools.
//!
//! Port of pi's `packages/codemode` + `packages/coding-agent/src/extensions/
//! codemode`. The problem it solves is stated in `todo.md` §3: a model that has to
//! batch 200 reads, chain a search into an edit, or filter a large output
//! expresses that as many round trips, each carrying its whole result in the
//! context. A script expresses the same flow in one call, and only the script's
//! own output reaches the model.
//!
//! Layout:
//! - [`source`] — the `// @options:` line and its parsing.
//! - [`sandbox`] — the QuickJS VM, the host-call loop, deadlines and limits.
//! - [`prelude`] — the JavaScript the scripts actually see (`tools`, `text`,
//!   `exit`, `store`, …), evaluated inside the VM before the script.
//! - [`declarations`] — JSON Schema → the TypeScript declarations the model reads
//!   instead of prose.
//! - [`tool`] — the `codemode` tool itself: description, discovery globals,
//!   nested calls, and result formatting.
//!
//! Two things this module deliberately does **not** do:
//! - It does not re-implement tool execution. A nested call goes through the host
//!   ([`tool::CodemodeHostBinding`]), which runs it as an ordinary tool call, so
//!   validation, hooks and permission gates apply exactly once, in one place.
//! - It does not persist a script. A script is in-memory VM state; the
//!   `store()`/`load()` pair is the durable part, and it is small on purpose.
//!
//! ```
//! use std::sync::Arc;
//! use futures::future::BoxFuture;
//! use serde_json::{json, Value};
//!
//! struct Host;
//!
//! impl tom_tools::codemode::CodemodeHost for Host {
//!     fn call_tool(&self, _name: String, _args: Value) -> BoxFuture<'static, Result<Value, String>> {
//!         Box::pin(async { Ok(json!("contents")) })
//!     }
//!
//!     fn call_global(&self, _name: String, _args: Value) -> BoxFuture<'static, Result<Value, String>> {
//!         Box::pin(async { Ok(Value::Null) })
//!     }
//! }
//!
//! # async fn run() {
//! let outcome = tom_tools::codemode::run_codemode_script(
//!     Default::default(),
//!     "return 1 + 1".to_string(),
//!     Arc::new(Host),
//!     tokio_util::sync::CancellationToken::new(),
//! )
//! .await;
//! assert!(outcome.is_ok());
//! assert_eq!(outcome.value, Some(json!(2)));
//! # }
//! ```

pub mod declarations;
pub mod prelude;
pub mod sandbox;
pub mod source;
pub mod tool;

use std::time::Duration;

pub use declarations::{render_tool_sample, to_codemode_identifier, CodemodeDeclaration};
pub use sandbox::{
    run_codemode_script, CodemodeCallRecord, CodemodeCallStatus, CodemodeError, CodemodeErrorKind,
    CodemodeGlobalSpec, CodemodeHost, CodemodeOutcome, CodemodeOutputItem, CodemodeSandboxOptions,
    CodemodeToolSpec, DEFAULT_MEMORY_LIMIT_BYTES, MAX_OUTPUT_CHARS, MAX_OUTPUT_ITEMS,
    MAX_STORE_TOTAL_CHARS, MAX_STORE_VALUE_CHARS,
};
pub use source::{
    parse_codemode_source, CodemodeSourceError, CodemodeSourceOptions, ParsedCodemodeSource,
    CODEMODE_OPTIONS_PREFIX, CODEMODE_SOURCE_GRAMMAR,
};
pub use tool::{
    codemode_description, CodemodeHostBinding, CodemodeInput, CodemodeMode, CodemodeNamespace,
    CodemodeStore, CodemodeTool, CodemodeToolImpl, CodemodeToolOptions, NestedToolOutcome,
    NoCodemodeStore, CODEMODE_STORE_ENTRY_TYPE, CODEMODE_TOOL_NAME, DEFAULT_CODEMODE_INLINE_BUDGET,
    DEFAULT_MAX_OUTPUT_TOKENS,
};

/// Deadline applied to a script that does not set one.
///
/// A script is bounded by its own work, but "its own work" is model-controlled:
/// a `while (true)` or a `Promise.all` over a slow server would otherwise hold the
/// run open forever. Ten minutes is generous for a real batch and short enough
/// that a runaway script still reports instead of hanging the session.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
