//! The engine side: one QuickJS runtime per program, on a thread of its own with a current-thread
//! tokio runtime. Tool calls leave that thread for the caller's runtime, so a slow tool never
//! holds up the program's other calls, and a tight loop in the program never holds up Trek.

use crate::fit::fit;
use crate::output::Output;
use crate::{ScriptHost, ScriptLimits, ScriptResult};
use rquickjs::context::EvalOptions;
use rquickjs::prelude::Async;
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function, Promise, Value};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::{Semaphore, watch};
use tokio::task::AbortHandle;

const PRELUDE: &str = include_str!("prelude.js");

/// The program's stop signal: its time ran out, or the caller dropped `run_script`. The flag is
/// what QuickJS's interrupt handler reads between instructions; the channel wakes pending calls.
pub(crate) struct Cancel {
    flag: AtomicBool,
    tx: watch::Sender<bool>,
}

impl Cancel {
    pub(crate) fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            tx: watch::channel(false).0,
        }
    }

    pub(crate) fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.tx.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        let _ = self.tx.subscribe().wait_for(|c| *c).await;
    }
}

/// Aborts a host call's task when the program stops waiting for it.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What the program has done so far, owned by the script thread.
struct State {
    out: Output,
    calls: u32,
    by_tool: BTreeMap<String, u32>,
    log: Option<Log>,
    /// The program's uncaught error, as `show` in the prelude formats it.
    failure: Option<String>,
}

/// The JSONL record of a run: the code, every print and every call with its full result.
struct Log {
    path: PathBuf,
    file: BufWriter<File>,
}

impl Log {
    fn create(dir: &Path) -> Option<Self> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(dir).ok()?;
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("script-{ms}-{}-{seq}.jsonl", std::process::id()));
        let file = File::create(&path).ok()?;
        Some(Self {
            path,
            file: BufWriter::new(file),
        })
    }

    fn write(&mut self, record: &serde_json::Value) {
        let _ = serde_json::to_writer(&mut self.file, record);
        let _ = self.file.write_all(b"\n");
    }
}

/// Everything the natives need, shared by every call the program makes.
struct Env {
    state: Rc<RefCell<State>>,
    host: Arc<dyn ScriptHost>,
    spawner: Handle,
    permits: Arc<Semaphore>,
    cancel: Arc<Cancel>,
    max_calls: u32,
    max_result_bytes: usize,
}

/// Runs the program to its end on the current thread, which it blocks. `spawner` is where tool
/// calls run; without one they share this thread's runtime.
pub(crate) fn run_blocking(
    code: String,
    host: Arc<dyn ScriptHost>,
    limits: ScriptLimits,
    spawner: Option<Handle>,
    cancel: Arc<Cancel>,
    started: Instant,
) -> ScriptResult {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return ScriptResult::failed(format!("couldn't start the script runtime: {e}")),
    };
    let spawner = spawner.unwrap_or_else(|| rt.handle().clone());
    rt.block_on(run(code, host, limits, spawner, cancel, started))
}

async fn run(
    code: String,
    host: Arc<dyn ScriptHost>,
    limits: ScriptLimits,
    spawner: Handle,
    cancel: Arc<Cancel>,
    started: Instant,
) -> ScriptResult {
    let deadline = started + limits.wall_clock;
    let mut log = limits.log_dir.as_deref().and_then(Log::create);
    if let Some(log) = &mut log {
        log.write(&json!({ "kind": "start", "code": code }));
    }
    let state = Rc::new(RefCell::new(State {
        out: Output::new(limits.max_output_bytes),
        calls: 0,
        by_tool: BTreeMap::new(),
        log,
        failure: None,
    }));
    let env = Rc::new(Env {
        state: state.clone(),
        host,
        spawner,
        permits: Arc::new(Semaphore::new(limits.max_concurrency.max(1))),
        cancel: cancel.clone(),
        max_calls: limits.max_calls,
        max_result_bytes: limits.max_result_bytes,
    });

    let engine_error = engine(&code, env, &limits, deadline, &cancel).await.err();
    // Whatever stopped the program, nothing it started may outlive it.
    cancel.cancel();

    let timed_out = Instant::now() >= deadline;
    let mut st = state.borrow_mut();
    // An interrupt is how a timeout or a cancel stops running code; it isn't the program's error.
    let error = match (st.failure.take(), engine_error) {
        _ if timed_out => None,
        (Some(e), _) => Some(e),
        (None, e) => e,
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let log_path = st.log.as_ref().map(|l| l.path.clone());
    let (nested_calls, calls_by_tool) = (st.calls, std::mem::take(&mut st.by_tool));
    if let Some(log) = &mut st.log {
        log.write(&json!({
            "kind": "end",
            "elapsed_ms": elapsed_ms,
            "nested_calls": nested_calls,
            "timed_out": timed_out,
            "error": error,
        }));
        let _ = log.file.flush();
    }
    let out = std::mem::replace(&mut st.out, Output::new(0));
    let (printed, truncated_bytes) = out.finish(log_path.as_deref());
    ScriptResult {
        printed,
        truncated_bytes,
        nested_calls,
        calls_by_tool,
        elapsed_ms,
        timed_out,
        error,
        log_path,
    }
}

/// Runs the program, then whatever it left running (calls it didn't await), until both are done
/// or the deadline passes. `Err` is a failure to start or an interrupt, not the program's own
/// error (that lands in `State::failure`).
async fn engine(
    code: &str,
    env: Rc<Env>,
    limits: &ScriptLimits,
    deadline: Instant,
    cancel: &Arc<Cancel>,
) -> Result<(), String> {
    let qrt = AsyncRuntime::new().map_err(|e| format!("couldn't start QuickJS: {e}"))?;
    qrt.set_memory_limit(limits.max_memory_bytes).await;
    qrt.set_max_stack_size(STACK_BYTES / 2).await;
    let stop = cancel.clone();
    qrt.set_interrupt_handler(Some(Box::new(move || {
        stop.is_cancelled() || Instant::now() >= deadline
    })))
    .await;
    let ctx = AsyncContext::full(&qrt)
        .await
        .map_err(|e| format!("couldn't start QuickJS: {e}"))?;

    // The program runs as the body of an async function, so it can `await` at the top and
    // `return` a value (which is printed). On the same line as the opening brace, so its line
    // numbers in errors are its own.
    let source = format!("__trek_run(async () => {{{code}\n}})");
    let main = ctx.async_with(async move |ctx| -> Result<(), String> {
        install(&ctx, env).map_err(|e| describe(&ctx, e))?;
        let mut opts = EvalOptions::default();
        opts.strict = false;
        opts.filename = Some("script".into());
        let done: Promise = ctx
            .eval_with_options(source, opts)
            .map_err(|e| describe(&ctx, e))?;
        done.into_future::<()>()
            .await
            .map_err(|e| describe(&ctx, e))
    });

    let sleep = tokio::time::sleep_until(deadline.into());
    tokio::pin!(sleep);
    let result = tokio::select! {
        r = main => r,
        _ = &mut sleep => return Ok(()),
        _ = cancel.cancelled() => return Ok(()),
    };
    tokio::select! {
        _ = qrt.idle() => {}
        _ = &mut sleep => {}
        _ = cancel.cancelled() => {}
    }
    result
}

/// The script thread's stack; QuickJS gets half, so deep recursion is a JS error, not a crash.
pub(crate) const STACK_BYTES: usize = 16 << 20;

/// The natives the prelude wraps, then the prelude itself.
fn install<'js>(ctx: &Ctx<'js>, env: Rc<Env>) -> rquickjs::Result<()> {
    let g = ctx.globals();

    let e = env.clone();
    g.set(
        "__trek_print",
        Function::new(ctx.clone(), move |line: String| {
            let mut st = e.state.borrow_mut();
            if let Some(log) = &mut st.log {
                log.write(&json!({ "kind": "print", "text": line }));
            }
            st.out.push_line(&line);
        })?,
    )?;

    let e = env.clone();
    g.set(
        "__trek_fail",
        Function::new(ctx.clone(), move |error: String| {
            e.state.borrow_mut().failure = Some(error);
        })?,
    )?;

    let e = env.clone();
    g.set(
        "__trek_sleep",
        Function::new(
            ctx.clone(),
            Async(move |ms: f64| {
                let e = e.clone();
                async move {
                    let d = Duration::from_secs_f64((ms / 1000.0).clamp(0.0, 1e6));
                    tokio::select! {
                        _ = tokio::time::sleep(d) => {}
                        _ = e.cancel.cancelled() => {}
                    }
                    rquickjs::Result::Ok(())
                }
            }),
        )?,
    )?;

    let e = env;
    g.set(
        "__trek_call",
        Function::new(
            ctx.clone(),
            Async(move |name: String, args: String| {
                let e = e.clone();
                async move { rquickjs::Result::Ok(call(e, name, args).await) }
            }),
        )?,
    )?;

    ctx.eval::<(), _>(PRELUDE)?;
    // Defined after the prelude so its natives are already gone when the program starts.
    ctx.eval::<(), _>(RUN)?;
    Ok(())
}

/// Starts the program and reports its end: a returned value is printed, an uncaught error is
/// formatted like a print and handed to `__trek_fail`.
const RUN: &str = r#"
(() => {
  const fail = globalThis.__trek_fail;
  delete globalThis.__trek_fail;
  globalThis.__trek_run = (main) => {
    delete globalThis.__trek_run;
    return main().then(
      (v) => { if (v !== undefined) print(v); },
      (e) => {
        if (e instanceof Error) fail(e.stack ? `${e.name}: ${e.message}\n${e.stack}`.trimEnd() : `${e.name}: ${e.message}`);
        else { try { fail(`Uncaught ${JSON.stringify(e) ?? String(e)}`); } catch { fail(`Uncaught ${String(e)}`); } }
      },
    );
  };
})();
"#;

/// One tool call from the program. The reply is JSON the prelude unpacks: `{"r": result}` or
/// `{"e": message}`, which it throws.
async fn call(env: Rc<Env>, name: String, args: String) -> String {
    let reply_err = |e: String| json!({ "e": e }).to_string();
    let args: serde_json::Value = serde_json::from_str(&args).unwrap_or_default();
    {
        let mut st = env.state.borrow_mut();
        if st.calls >= env.max_calls {
            return reply_err(format!(
                "the script reached its limit of {} tool calls",
                env.max_calls
            ));
        }
        st.calls += 1;
        *st.by_tool.entry(name.clone()).or_default() += 1;
    }

    let task = env.spawner.spawn({
        let (host, permits) = (env.host.clone(), env.permits.clone());
        let (name, args) = (name.clone(), args.clone());
        async move {
            let _permit = permits.acquire_owned().await;
            let t = Instant::now();
            let r = host.call(&name, args).await;
            (r, t.elapsed())
        }
    });
    let _abort = AbortOnDrop(task.abort_handle());
    let (result, took) = tokio::select! {
        r = task => match r {
            Ok(done) => done,
            Err(e) if e.is_panic() => (Err(format!("the {name} tool panicked")), Duration::ZERO),
            Err(_) => (Err("cancelled".to_string()), Duration::ZERO),
        },
        _ = env.cancel.cancelled() => return reply_err("cancelled: the script was stopped".into()),
    };

    let mut st = env.state.borrow_mut();
    match result {
        Ok(mut value) => {
            if let Some(log) = &mut st.log {
                let bytes = serde_json::to_vec(&value).map(|b| b.len()).unwrap_or(0);
                log.write(&json!({
                    "kind": "call", "tool": name, "args": args, "ok": true,
                    "bytes": bytes, "ms": took.as_millis() as u64, "result": value,
                }));
            }
            fit(&mut value, env.max_result_bytes);
            json!({ "r": value }).to_string()
        }
        Err(e) => {
            if let Some(log) = &mut st.log {
                log.write(&json!({
                    "kind": "call", "tool": name, "args": args, "ok": false,
                    "bytes": 0, "ms": took.as_millis() as u64, "error": e,
                }));
            }
            reply_err(e)
        }
    }
}

/// An error from QuickJS as text: the pending exception's name, message and stack where there is
/// one (a syntax error, an interrupt), else the error itself.
fn describe(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    if !matches!(e, rquickjs::Error::Exception) {
        return e.to_string();
    }
    let v: Value = ctx.catch();
    if let Some(ex) = v.as_exception() {
        let name: Option<String> = ex.get("name").ok();
        let head = format!(
            "{}: {}",
            name.as_deref().unwrap_or("Error"),
            ex.message().unwrap_or_default()
        );
        return match ex.stack().filter(|s| !s.trim().is_empty()) {
            Some(stack) => format!("{head}\n{}", stack.trim_end()),
            None => head,
        };
    }
    match v.as_string().and_then(|s| s.to_string().ok()) {
        Some(s) => s,
        None => format!("{v:?}"),
    }
}
