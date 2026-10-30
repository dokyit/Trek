//! The `script` tool: programmatic tool calling for Trek's own agent harness. Instead of calling
//! tools one at a time, the model writes a JavaScript program that calls them (hundreds of calls
//! in one model request), and only what the program prints goes back into its context. Tool
//! results stay in the program's variables, and every call is logged to a JSONL file for later
//! search. Modelled on Factory's Droid `Script` tool.
//!
//! The program runs in QuickJS with no file, network or system access of its own: the tools the
//! harness hands in through [`ScriptHost`] are its only way out. Independent calls run at the same
//! time (`Promise.all`), bounded by [`ScriptLimits::max_concurrency`].

mod definition;
mod fit;
mod output;
mod sandbox;

pub use async_trait::async_trait;
pub use definition::{anthropic_tool_definition, tool_definition};

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The harness's tools, as the program sees them. `call` may run concurrently, up to
/// [`ScriptLimits::max_concurrency`] at once, on the tokio runtime that called [`run_script`].
///
/// The prelude's wrappers call these names: `bash` with `{command, timeout_s?}` returning
/// `{stdout, stderr, exit_code}`; `read` with `{path, start_line?, end_line?}` returning
/// `{content}`; `write` with `{path, content}`; `edit` with `{path, old, new}`. `tool(name, args)`
/// in the program passes any other name through as it is. An `Err` is thrown in the program as an
/// `Error` whose message starts with the tool's name.
///
/// Implement it with `#[trek_script::async_trait]`.
#[async_trait]
pub trait ScriptHost: Send + Sync {
    async fn call(&self, tool: &str, args: serde_json::Value) -> Result<serde_json::Value, String>;
}

/// How far one program may go. `Default` is what [`tool_definition`] tells the model.
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    /// Wall clock for the whole run (taken as at most a day); after it the output so far comes
    /// back with `timed_out`. Once the program returns, what it left running (calls it didn't
    /// await, timers) gets 2 more seconds within this, then is stopped (`ScriptResult::detached`).
    pub wall_clock: Duration,
    /// Printed text returned to the model; beyond it the head and tail are kept.
    pub max_output_bytes: usize,
    /// Tool calls in one run; past it a call throws.
    pub max_calls: u32,
    /// One result's JSON as the program receives it; longer strings in it lose their middle.
    /// The log keeps up to 64 KiB of each field.
    pub max_result_bytes: usize,
    /// Tool calls in flight at once; 0 is taken as 1.
    pub max_concurrency: usize,
    /// QuickJS heap; past it the program gets an out-of-memory error.
    pub max_memory_bytes: usize,
    /// Where the run's JSONL log goes (one file per run, the directory is created). `None` keeps
    /// no log.
    pub log_dir: Option<PathBuf>,
    /// One run's log; past it a `capped` record, then only the `end` record.
    pub max_log_bytes: u64,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(300),
            max_output_bytes: 16 * 1024,
            max_calls: 1_000,
            max_result_bytes: 2 * 1024 * 1024,
            max_concurrency: 8,
            max_memory_bytes: 256 * 1024 * 1024,
            log_dir: None,
            max_log_bytes: 64 * 1024 * 1024,
        }
    }
}

/// How a run went. `printed` is what the model should see; the rest is for the harness and the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptResult {
    /// Everything the program printed, capped (see `truncated_bytes`).
    pub printed: String,
    /// Bytes dropped from the middle of `printed`.
    pub truncated_bytes: u64,
    /// Tool calls the program made.
    pub nested_calls: u32,
    pub calls_by_tool: BTreeMap<String, u32>,
    pub elapsed_ms: u64,
    /// The wall clock ran out before the program returned; `printed` is the output up to then.
    /// Never set for a program that returned (see `detached`).
    pub timed_out: bool,
    /// The program's uncaught error (name, message and stack, at most about 4 KiB), or a
    /// failure to run it at all.
    pub error: Option<String>,
    /// The run's JSONL log: a `start` record with the code, then `print` and `call` records in
    /// order (a call with its args, `bytes`, `ms` from call to answer, and its `result` or
    /// `error`), then `end`. Each field is kept to 64 KiB; see `ScriptLimits::max_log_bytes`.
    pub log_path: Option<PathBuf>,
    /// Calls and timers the program left running when it returned that were still running 2 s
    /// later, and were stopped.
    pub detached: u32,
    /// Things the model should hear about that didn't stop the program: rejected promises no
    /// one handled.
    pub warnings: Vec<String>,
}

impl ScriptResult {
    fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }

    /// The tool result for the model: the printed text, then a line each for a timeout, work
    /// left running, warnings and an error.
    pub fn to_model_text(&self) -> String {
        let mut text = self.printed.clone();
        let mut note = |line: String| {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&line);
            text.push('\n');
        };
        if self.timed_out {
            note(format!(
                "[the script ran out of time after {} s; output so far is above]",
                self.elapsed_ms / 1000
            ));
        }
        if self.detached > 0 {
            note(format!(
                "[the script ended with {} detached calls or timers still running; they were stopped]",
                self.detached
            ));
        }
        for w in &self.warnings {
            note(format!("[warning] {w}"));
        }
        if let Some(e) = &self.error {
            note(format!("[the script failed] {e}"));
        }
        if text.is_empty() {
            text = format!(
                "[the script printed nothing; {} tool calls]",
                self.nested_calls
            );
        }
        text
    }
}

/// Runs `code` with `host`'s tools until it ends or `limits.wall_clock` passes.
///
/// The program runs on a thread of its own (QuickJS isn't `Send`, and a busy loop mustn't take a
/// tokio worker); the returned future only waits for it, so it is `Send` and can be spawned.
/// Tool calls are spawned on the tokio runtime this is called from (or, outside one, on the
/// script thread's). Dropping the future stops the program and aborts its calls in flight.
pub async fn run_script(
    code: &str,
    host: Arc<dyn ScriptHost>,
    limits: ScriptLimits,
) -> ScriptResult {
    let started = Instant::now();
    let cancel = Arc::new(sandbox::Cancel::new());
    // Cancelling a finished run does nothing, so this needn't be disarmed.
    let _stop = StopOnDrop(cancel.clone());
    let spawner = tokio::runtime::Handle::try_current().ok();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let code = code.to_owned();
    let spawned = std::thread::Builder::new()
        .name("trek-script".into())
        .stack_size(sandbox::STACK_BYTES)
        .spawn(move || {
            // `run_blocking` catches its own panics; this is the last line, for one in between.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sandbox::run_blocking(code, host, limits, spawner, cancel, started)
            }));
            let _ = tx.send(r.unwrap_or_else(|p| ScriptResult::failed(sandbox::panic_error(p))));
        });
    if let Err(e) = spawned {
        return ScriptResult::failed(format!("couldn't start the script thread: {e}"));
    }
    rx.await.unwrap_or_else(|_| {
        ScriptResult::failed("the script engine stopped unexpectedly".to_string())
    })
}

/// Stops the program when `run_script`'s future is dropped before it ends.
struct StopOnDrop(Arc<sandbox::Cancel>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
