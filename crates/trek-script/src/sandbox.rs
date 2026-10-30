//! The engine side: one QuickJS runtime per program, on a thread of its own with a current-thread
//! tokio runtime. Tool calls leave that thread for the caller's runtime, so a slow tool never
//! holds up the program's other calls, and a tight loop in the program never holds up Trek.

use crate::fit::fit;
use crate::output::{Output, cap_text};
use crate::{ScriptHost, ScriptLimits, ScriptResult};
use rquickjs::context::EvalOptions;
use rquickjs::prelude::{Async, Opt};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function, Promise, Value};
use serde_json::json;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::hash::{Hash as _, Hasher as _};
use std::io::{BufWriter, Write as _};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::AbortHandle;

const PRELUDE: &str = include_str!("prelude.js");

/// How long work the program left running (calls it didn't await, timers) gets once the program
/// itself has returned. Then it's stopped and counted in `ScriptResult::detached`.
pub(crate) const DETACHED_GRACE: Duration = Duration::from_secs(2);
/// The program's error, and each warning, as the model is told it.
const ERROR_BYTES: usize = 4 * 1024;
const WARNING_BYTES: usize = 1024;
const MAX_WARNINGS: usize = 20;
/// One field of one log record (args, a result, a print).
const LOG_FIELD_BYTES: usize = 64 * 1024;
/// The longest wall clock taken as given; past it is a typo, not a plan.
const MAX_WALL_CLOCK: Duration = Duration::from_secs(24 * 3600);

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
    /// The program's body settled: it returned, or threw (then `failure` says what).
    ended: bool,
    failure: Option<String>,
    /// Tool calls and timers started and not yet settled.
    in_flight: u32,
    timers: u32,
    /// Rejections no handler has claimed yet, by promise (see `track_rejections`).
    unhandled: Vec<(u64, String)>,
}

/// The JSONL record of a run: the code, every print and every call with its result, each field
/// capped at `LOG_FIELD_BYTES`, and the whole at `ScriptLimits::max_log_bytes`.
struct Log {
    path: PathBuf,
    file: BufWriter<File>,
    written: u64,
    max: u64,
    capped: bool,
}

impl Log {
    fn create(dir: &Path, max: u64) -> Option<Self> {
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
            written: 0,
            max,
            capped: false,
        })
    }

    fn write(&mut self, mut record: serde_json::Value) {
        let end = record["kind"] == "end";
        if self.capped && !end {
            return;
        }
        if let Some(fields) = record.as_object_mut() {
            for v in fields.values_mut() {
                fit(v, LOG_FIELD_BYTES);
            }
        }
        let mut line = serde_json::to_vec(&record).unwrap_or_default();
        line.push(b'\n');
        if !end && self.written + line.len() as u64 > self.max {
            // Said once, and only the `end` record follows.
            self.capped = true;
            line = format!(
                "{}\n",
                json!({ "kind": "capped", "after_bytes": self.written })
            )
            .into_bytes();
        }
        if self.file.write_all(&line).is_ok() {
            self.written += line.len() as u64;
        }
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
    /// Pending timers by the prelude's id, so `clearTimeout` can end one early.
    timers: RefCell<HashMap<u64, Rc<Notify>>>,
}

/// Runs the program to its end on the current thread, which it blocks. `spawner` is where tool
/// calls run; without one they share this thread's runtime. A panic anywhere in here still
/// returns what the program printed and did, with the panic as the error.
pub(crate) fn run_blocking(
    code: String,
    host: Arc<dyn ScriptHost>,
    limits: ScriptLimits,
    spawner: Option<Handle>,
    cancel: Arc<Cancel>,
    started: Instant,
) -> ScriptResult {
    let deadline = started
        .checked_add(limits.wall_clock.min(MAX_WALL_CLOCK))
        .unwrap_or(started);
    let mut log = limits
        .log_dir
        .as_deref()
        .and_then(|d| Log::create(d, limits.max_log_bytes));
    if let Some(log) = &mut log {
        log.write(json!({ "kind": "start", "code": code }));
    }
    let state = Rc::new(RefCell::new(State {
        out: Output::new(limits.max_output_bytes),
        calls: 0,
        by_tool: BTreeMap::new(),
        log,
        ended: false,
        failure: None,
        in_flight: 0,
        timers: 0,
        unhandled: Vec::new(),
    }));

    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("couldn't start the script runtime: {e}"))?;
        let spawner = spawner.unwrap_or_else(|| rt.handle().clone());
        let env = Rc::new(Env {
            state: state.clone(),
            host,
            spawner,
            // Zero would never let a call through; it's taken as one.
            permits: Arc::new(Semaphore::new(limits.max_concurrency.max(1))),
            cancel: cancel.clone(),
            max_calls: limits.max_calls,
            max_result_bytes: limits.max_result_bytes,
            timers: RefCell::new(HashMap::new()),
        });
        rt.block_on(engine(&code, env, &limits, deadline, &cancel))
    }));
    // Whatever stopped the program, nothing it started may outlive it.
    cancel.cancel();
    let (engine_error, detached) = match ran {
        Ok(Ok(detached)) => (None, detached),
        Ok(Err(e)) => (Some(e), 0),
        Err(panic) => (Some(panic_error(panic)), 0),
    };
    finish(&state, engine_error, detached, started, deadline)
}

/// A caught panic as the run's error.
pub(crate) fn panic_error(panic: Box<dyn std::any::Any + Send>) -> String {
    let what = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into());
    format!("the script engine failed: {what}")
}

/// The result from what the run left in `state`.
fn finish(
    state: &Rc<RefCell<State>>,
    engine_error: Option<String>,
    detached: u32,
    started: Instant,
    deadline: Instant,
) -> ScriptResult {
    let mut st = state.borrow_mut();
    // Only a program that never returned can have run out of time.
    let timed_out = !st.ended && Instant::now() >= deadline;
    let error = match (st.failure.take(), engine_error) {
        (Some(e), _) => Some(e),
        // An interrupt is how the clock stops running code; it isn't the program's error.
        (None, Some(e)) if timed_out && e.contains("interrupted") => None,
        (None, e) => e,
    }
    .map(|e| cap_text(&e, ERROR_BYTES));
    let mut warnings: Vec<String> = st
        .unhandled
        .drain(..)
        .map(|(_, w)| format!("unhandled rejection: {w}"))
        .collect();
    if warnings.len() > MAX_WARNINGS {
        let more = warnings.len() - MAX_WARNINGS;
        warnings.truncate(MAX_WARNINGS);
        warnings.push(format!("... and {more} more unhandled rejections"));
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let log_path = st.log.as_ref().map(|l| l.path.clone());
    let (nested_calls, calls_by_tool) = (st.calls, std::mem::take(&mut st.by_tool));
    if let Some(log) = &mut st.log {
        log.write(json!({
            "kind": "end",
            "elapsed_ms": elapsed_ms,
            "nested_calls": nested_calls,
            "timed_out": timed_out,
            "detached": detached,
            "error": error,
            "warnings": warnings,
        }));
        let _ = log.file.flush();
    }
    let out = std::mem::replace(&mut st.out, Output::new(0));
    // Counts and the log path survive even a bug in putting the text together.
    let (printed, truncated_bytes, error) =
        match std::panic::catch_unwind(AssertUnwindSafe(|| out.finish(log_path.as_deref()))) {
            Ok((printed, dropped)) => (printed, dropped, error),
            Err(panic) => (String::new(), 0, error.or(Some(panic_error(panic)))),
        };
    ScriptResult {
        printed,
        truncated_bytes,
        nested_calls,
        calls_by_tool,
        elapsed_ms,
        timed_out,
        error,
        log_path,
        detached,
        warnings,
    }
}

/// Runs the program until its body settles, then gives what it left running `DETACHED_GRACE`
/// to finish. `Ok` is how many calls and timers were still running when that ran out; `Err` is
/// a failure to start or an interrupt, not the program's own error (that lands in `State`).
async fn engine(
    code: &str,
    env: Rc<Env>,
    limits: &ScriptLimits,
    deadline: Instant,
    cancel: &Arc<Cancel>,
) -> Result<u32, String> {
    let qrt = AsyncRuntime::new().map_err(|e| format!("couldn't start QuickJS: {e}"))?;
    qrt.set_memory_limit(limits.max_memory_bytes).await;
    qrt.set_max_stack_size(STACK_BYTES / 2).await;
    let stop = cancel.clone();
    qrt.set_interrupt_handler(Some(Box::new(move || {
        stop.is_cancelled() || Instant::now() >= deadline
    })))
    .await;
    track_rejections(&qrt, env.state.clone()).await;
    let ctx = AsyncContext::full(&qrt)
        .await
        .map_err(|e| format!("couldn't start QuickJS: {e}"))?;

    // The program runs as the body of an async function, so it can `await` at the top and
    // `return` a value (which is printed). On the same line as the opening brace, so its line
    // numbers in errors are its own.
    let source = format!("__trek_run(async () => {{{code}\n}})");
    let state = env.state.clone();
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
        _ = &mut sleep => return Ok(0),
        _ = cancel.cancelled() => return Ok(0),
    };
    if !state.borrow().ended {
        // Stopped by an interrupt or a startup error, not by its own end.
        return result.map(|()| 0);
    }
    let grace = tokio::time::sleep(DETACHED_GRACE);
    tokio::select! {
        _ = qrt.idle() => {}
        _ = grace => {}
        _ = &mut sleep => {}
        _ = cancel.cancelled() => {}
    }
    let st = state.borrow();
    Ok(st.in_flight + st.timers)
}

/// Keeps the reasons of rejected promises nobody handles. A promise that gets its handler later
/// (awaited after it failed) is dropped from the list again; what's left at the end is a warning.
async fn track_rejections(qrt: &AsyncRuntime, state: Rc<RefCell<State>>) {
    qrt.set_host_promise_rejection_tracker(Some(Box::new(move |ctx, promise, reason, handled| {
        let key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            promise.hash(&mut h);
            h.finish()
        };
        let Ok(mut st) = state.try_borrow_mut() else {
            return;
        };
        if handled {
            st.unhandled.retain(|(k, _)| *k != key);
        } else if !reason.is_uncatchable_error() && st.unhandled.len() < 1_000 {
            let text = cap_text(&show(&ctx, reason), WARNING_BYTES);
            st.unhandled.push((key, text));
        }
    })))
    .await;
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
                log.write(json!({ "kind": "print", "text": line }));
            }
            st.out.push_line(&line);
        })?,
    )?;

    let e = env.clone();
    g.set(
        "__trek_end",
        Function::new(ctx.clone(), move |error: Opt<String>| {
            let mut st = e.state.borrow_mut();
            st.ended = true;
            st.failure = error.0.map(|e| cap_text(&e, ERROR_BYTES));
        })?,
    )?;

    // A timer: resolves `true` when its time is up, `false` when cleared or stopped. It's
    // registered before this returns, so a `clearTimeout` right after `setTimeout` finds it.
    let e = env.clone();
    g.set(
        "__trek_sleep",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, ms: f64, id: f64| -> rquickjs::Result<Promise<'js>> {
                let timer = Timer::start(&e, id as u64);
                let cancel = e.cancel.clone();
                let d = Duration::from_secs_f64((ms / 1000.0).clamp(0.0, 1e6));
                Promise::wrap_future(&ctx, async move {
                    let fired = tokio::select! {
                        _ = tokio::time::sleep(d) => true,
                        _ = timer.woken.notified() => false,
                        _ = cancel.cancelled() => false,
                    };
                    drop(timer);
                    rquickjs::Result::Ok(fired)
                })
            },
        )?,
    )?;

    let e = env.clone();
    g.set(
        "__trek_wake",
        Function::new(ctx.clone(), move |id: f64| {
            if let Some(n) = e.timers.borrow_mut().remove(&(id as u64)) {
                n.notify_one();
            }
        })?,
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
    Ok(())
}

/// A pending timer: counted while it waits, and wakeable by its id.
struct Timer {
    env: Rc<Env>,
    id: u64,
    woken: Rc<Notify>,
}

impl Timer {
    fn start(env: &Rc<Env>, id: u64) -> Self {
        let woken = Rc::new(Notify::new());
        env.timers.borrow_mut().insert(id, woken.clone());
        if let Ok(mut st) = env.state.try_borrow_mut() {
            st.timers += 1;
        }
        Self {
            env: env.clone(),
            id,
            woken,
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Ok(mut t) = self.env.timers.try_borrow_mut() {
            t.remove(&self.id);
        }
        if let Ok(mut st) = self.env.state.try_borrow_mut() {
            st.timers = st.timers.saturating_sub(1);
        }
    }
}

/// A tool call in flight: counted, and logged however it ends, including dropped unanswered
/// when the run stops.
struct Pending {
    env: Rc<Env>,
    name: String,
    args: serde_json::Value,
    started: Instant,
    logged: bool,
}

impl Pending {
    fn log(&mut self, outcome: Result<&serde_json::Value, &str>) {
        self.logged = true;
        let Ok(mut st) = self.env.state.try_borrow_mut() else {
            return;
        };
        let Some(log) = &mut st.log else { return };
        let ms = self.started.elapsed().as_millis() as u64;
        let args = std::mem::take(&mut self.args);
        log.write(match outcome {
            Ok(value) => json!({
                "kind": "call", "tool": self.name, "args": args, "ok": true,
                "bytes": serde_json::to_vec(value).map(|b| b.len()).unwrap_or(0),
                "ms": ms, "result": value,
            }),
            Err(e) => json!({
                "kind": "call", "tool": self.name, "args": args, "ok": false,
                "bytes": 0, "ms": ms, "error": e,
            }),
        });
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.logged {
            self.log(Err("stopped: the script ended before the call did"));
        }
        if let Ok(mut st) = self.env.state.try_borrow_mut() {
            st.in_flight = st.in_flight.saturating_sub(1);
        }
    }
}

/// One tool call from the program. The reply is JSON the prelude unpacks: `{"r": result}` or
/// `{"e": message}`, which it throws. `ms` in the log is from the call to its answer, waiting
/// for a free slot included.
async fn call(env: Rc<Env>, name: String, args: String) -> String {
    let reply_err = |e: &str| json!({ "e": e }).to_string();
    let args: serde_json::Value = match serde_json::from_str(&args) {
        Ok(a) => a,
        Err(e) => return reply_err(&format!("its arguments aren't JSON: {e}")),
    };
    {
        let mut st = env.state.borrow_mut();
        if st.calls >= env.max_calls {
            return reply_err(&format!(
                "the script reached its limit of {} tool calls",
                env.max_calls
            ));
        }
        st.calls += 1;
        st.in_flight += 1;
        *st.by_tool.entry(name.clone()).or_default() += 1;
    }
    let mut pending = Pending {
        env: env.clone(),
        name: name.clone(),
        args: args.clone(),
        started: Instant::now(),
        logged: false,
    };

    let task = env.spawner.spawn({
        let (host, permits) = (env.host.clone(), env.permits.clone());
        async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return Err("the script was stopped".to_string());
            };
            host.call(&name, args).await
        }
    });
    let _abort = AbortOnDrop(task.abort_handle());
    let result = tokio::select! {
        r = task => match r {
            Ok(done) => done,
            Err(e) if e.is_panic() => Err(format!("the {} tool panicked", pending.name)),
            Err(_) => Err("cancelled".to_string()),
        },
        _ = env.cancel.cancelled() => {
            let e = "cancelled: the script was stopped";
            pending.log(Err(e));
            return reply_err(e);
        }
    };

    match result {
        Ok(mut value) => {
            pending.log(Ok(&value));
            fit(&mut value, env.max_result_bytes);
            json!({ "r": value }).to_string()
        }
        Err(e) => {
            pending.log(Err(&e));
            reply_err(&e)
        }
    }
}

/// A thrown value as text: `Name: message` for an error, else its JSON or string form.
fn show<'js>(ctx: &Ctx<'js>, v: Value<'js>) -> String {
    if let Some(ex) = v.as_exception() {
        let name: Option<String> = ex.get("name").ok();
        return format!(
            "{}: {}",
            name.as_deref().unwrap_or("Error"),
            ex.message().unwrap_or_default()
        );
    }
    if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
        return s;
    }
    match ctx.json_stringify(v.clone()) {
        Ok(Some(s)) => s.to_string().unwrap_or_default(),
        _ => format!("{v:?}"),
    }
}

/// An error from QuickJS as text: the pending exception's name, message and stack where there is
/// one (a syntax error, an interrupt), else the error itself.
fn describe(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    if !matches!(e, rquickjs::Error::Exception) {
        return e.to_string();
    }
    let v: Value = ctx.catch();
    let head = show(ctx, v.clone());
    match v
        .as_exception()
        .and_then(|ex| ex.stack())
        .filter(|s| !s.trim().is_empty())
    {
        Some(stack) => format!("{head}\n{}", stack.trim_end()),
        None => head,
    }
}
