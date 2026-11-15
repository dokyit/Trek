//! A Trek plugin's logic (`main.js`): one call of it, in QuickJS, with nothing of the machine in
//! reach. The plugin's file is evaluated in a fresh context, the handler for what happened is
//! called (a hook's listeners, or a command's function), and what it asked for comes back as
//! data: its state as it left it, and effects (a toast, a thread to open, a message to send) for
//! Trek to carry out or refuse by the plugin's permissions. Nothing lives between calls but the
//! state, so a plugin can't leak, hang on to a timer, or be left half-broken by an error.
//!
//! A call has a time budget and a memory cap; past either QuickJS stops it and the call fails.

use rquickjs::{Context, Runtime};
use serde_json::Value;
use std::time::{Duration, Instant};

/// What to call in the plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum Handler {
    /// Every listener `trek.on(<event>, …)` registered.
    Event(String),
    /// The function `trek.command(<name>, …)` registered, or a global function of that name.
    Command(String),
}

/// One call of a plugin's logic.
#[derive(Debug, Clone)]
pub struct PluginCall {
    /// The plugin's `main.js`.
    pub source: String,
    pub handler: Handler,
    /// What happened (`{thread: {…}}`), handed to the handler.
    pub arg: Value,
    /// The plugin's state before the call (an object).
    pub state: Value,
    /// The plugin's own storage (an object).
    pub storage: Value,
    /// What its permissions let it read: `{threads, usage, theme, project}`.
    pub data: Value,
}

/// How far a call may go.
#[derive(Debug, Clone, Copy)]
pub struct PluginLimits {
    pub time: Duration,
    pub memory: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            time: Duration::from_millis(250),
            memory: 32 * 1024 * 1024,
        }
    }
}

/// What a call asked Trek to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Toast(String),
    Open(String),
    NewThread,
    Send {
        thread: String,
        text: String,
    },
    Page(String),
    /// Keep this as the plugin's storage.
    Storage(Value),
}

/// How a call went.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginRun {
    /// The state after the call, when the call changed it.
    pub state: Option<Value>,
    pub effects: Vec<Effect>,
    /// What it printed (`console.log`).
    pub logs: Vec<String>,
    /// Why it failed: an exception, the time budget, the memory cap.
    pub error: Option<String>,
}

const PRELUDE: &str = r#"
const __out = { state: null, effects: [], logs: [] };
const __handlers = Object.create(null), __commands = Object.create(null);
const __text = (v) => { try { return typeof v === 'string' ? v : JSON.stringify(v); } catch (_) { return String(v); } };
const trek = Object.freeze({
  state: Object.freeze({
    get: () => __state,
    set: (patch) => { if (patch && typeof patch === 'object') { Object.assign(__state, patch); __out.state = __state; } },
  }),
  storage: Object.freeze({
    get: () => __storage,
    set: (value) => { if (value && typeof value === 'object') { __storage = value; __out.effects.push({ t: 'storage', value }); } },
  }),
  on: (event, f) => { if (typeof f === 'function') (__handlers[event] ||= []).push(f); },
  command: (name, f) => { if (typeof f === 'function') __commands[name] = f; },
  toast: (text) => { __out.effects.push({ t: 'toast', text: String(text) }); },
  open: (thread) => { __out.effects.push({ t: 'open', thread: String(thread) }); },
  newThread: () => { __out.effects.push({ t: 'new-thread' }); },
  send: (thread, text) => { __out.effects.push({ t: 'send', thread: String(thread), text: String(text) }); },
  page: (id) => { __out.effects.push({ t: 'page', page: String(id) }); },
  threads: () => __data.threads || [],
  usage: () => __data.usage || null,
  project: () => __data.project || null,
  theme: () => __data.theme || null,
  log: (...a) => { __out.logs.push(a.map(__text).join(' ')); },
});
const console = Object.freeze({ log: trek.log, info: trek.log, warn: trek.log, error: trek.log });
"#;

const DISPATCH: &str = r#"
(() => {
  if (__kind === 'event') {
    for (const f of (__handlers[__name] || [])) f(__arg);
  } else {
    const f = __commands[__name] || (typeof globalThis[__name] === 'function' ? globalThis[__name] : null);
    if (!f) throw new Error('main.js has no function "' + __name + '" (define it, or trek.command("' + __name + '", …))');
    f(__arg);
  }
  return JSON.stringify(__out);
})()
"#;

/// Run one call of a plugin's logic. Blocking, for a background thread: it takes at most
/// `limits.time`.
pub fn run_plugin(call: &PluginCall, limits: PluginLimits) -> PluginRun {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(call, limits))) {
        Ok(run) => run,
        Err(panic) => PluginRun {
            error: Some(crate::sandbox::panic_error(panic)),
            ..Default::default()
        },
    }
}

fn run(call: &PluginCall, limits: PluginLimits) -> PluginRun {
    let fail = |error: String| PluginRun {
        error: Some(error),
        ..Default::default()
    };
    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return fail(format!("couldn't start QuickJS: {e}")),
    };
    rt.set_memory_limit(limits.memory);
    rt.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + limits.time;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let ctx = match Context::full(&rt) {
        Ok(ctx) => ctx,
        Err(e) => return fail(format!("couldn't start QuickJS: {e}")),
    };
    let (kind, name) = match &call.handler {
        Handler::Event(e) => ("event", e),
        Handler::Command(c) => ("command", c),
    };
    let object = |v: &Value| {
        if v.is_object() {
            v.to_string()
        } else {
            "{}".to_string()
        }
    };
    // What the call is given, as literals: JSON is JavaScript.
    let inputs = format!(
        "let __state = {}; let __storage = {}; const __data = {}; const __arg = {}; const __kind = {}; const __name = {};",
        object(&call.state),
        object(&call.storage),
        object(&call.data),
        call.arg,
        Value::String(kind.to_string()),
        Value::String(name.clone()),
    );
    let out: Result<String, String> = ctx.with(|ctx| {
        let step = |code: &str| -> Result<(), String> {
            ctx.eval::<(), _>(code)
                .map_err(|e| crate::sandbox::describe(&ctx, e))
        };
        step(&inputs)?;
        step(PRELUDE)?;
        step(&call.source)?;
        ctx.eval::<String, _>(DISPATCH)
            .map_err(|e| crate::sandbox::describe(&ctx, e))
    });
    let timed_out = Instant::now() > deadline;
    let text = match out {
        Ok(text) => text,
        Err(_) if timed_out => {
            return fail(format!(
                "it ran past its {} ms: a plugin's function has to return quickly",
                limits.time.as_millis()
            ));
        }
        Err(e) => return fail(e),
    };
    let Ok(out) = serde_json::from_str::<Value>(&text) else {
        return fail("its result couldn't be read".into());
    };
    let text_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let effects = out["effects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(match e.get("t")?.as_str()? {
                "toast" => Effect::Toast(text_of(e, "text")?),
                "open" => Effect::Open(text_of(e, "thread")?),
                "new-thread" => Effect::NewThread,
                "send" => Effect::Send {
                    thread: text_of(e, "thread")?,
                    text: text_of(e, "text")?,
                },
                "page" => Effect::Page(text_of(e, "page")?),
                "storage" => Effect::Storage(e.get("value")?.clone()),
                _ => return None,
            })
        })
        .take(64)
        .collect();
    PluginRun {
        state: out.get("state").filter(|s| s.is_object()).cloned(),
        effects,
        logs: out["logs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_string))
            .take(200)
            .collect(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(source: &str, handler: Handler) -> PluginCall {
        PluginCall {
            source: source.to_string(),
            handler,
            arg: json!({"thread": {"id": "t1", "title": "Fix the build"}}),
            state: json!({"count": 2}),
            storage: json!({}),
            data: json!({"threads": [{"id": "t1", "state": "working"}]}),
        }
    }

    #[test]
    fn a_hook_sets_state_and_asks_for_effects() {
        let run = run_plugin(
            &call(
                r#"
                trek.on("turn.end", (e) => {
                  trek.state.set({ count: trek.state.get().count + 1, last: e.thread.title });
                  trek.toast(`Done: ${e.thread.title}`);
                  console.log("threads", trek.threads().length);
                });
                trek.on("turn.start", () => trek.toast("not this one"));
                "#,
                Handler::Event("turn.end".into()),
            ),
            PluginLimits::default(),
        );
        assert_eq!(run.error, None);
        assert_eq!(
            run.state,
            Some(json!({"count": 3, "last": "Fix the build"}))
        );
        assert_eq!(
            run.effects,
            vec![Effect::Toast("Done: Fix the build".into())]
        );
        assert_eq!(run.logs, vec!["threads 1"]);
    }

    #[test]
    fn a_command_is_a_registered_or_a_global_function() {
        let registered = run_plugin(
            &call(
                r#"trek.command("go", () => trek.send("t1", "carry on"));"#,
                Handler::Command("go".into()),
            ),
            PluginLimits::default(),
        );
        assert_eq!(
            registered.effects,
            vec![Effect::Send {
                thread: "t1".into(),
                text: "carry on".into()
            }]
        );
        assert_eq!(registered.state, None, "state untouched isn't sent back");
        let global = run_plugin(
            &call(
                "function hello() { trek.storage.set({ seen: true }); trek.open('t1'); }",
                Handler::Command("hello".into()),
            ),
            PluginLimits::default(),
        );
        assert_eq!(
            global.effects,
            vec![
                Effect::Storage(json!({"seen": true})),
                Effect::Open("t1".into())
            ]
        );
        let missing = run_plugin(
            &call("", Handler::Command("nope".into())),
            PluginLimits::default(),
        );
        assert!(missing.error.unwrap().contains("no function \"nope\""));
    }

    #[test]
    fn an_exception_a_runaway_and_a_hog_fail_the_call_alone() {
        let thrown = run_plugin(
            &call(
                r#"trek.on("turn.end", () => { undefinedThing.x = 1; });"#,
                Handler::Event("turn.end".into()),
            ),
            PluginLimits::default(),
        );
        assert!(
            thrown.error.as_deref().unwrap().contains("undefinedThing"),
            "{thrown:?}"
        );
        let limits = PluginLimits {
            time: Duration::from_millis(60),
            ..Default::default()
        };
        let started = Instant::now();
        let runaway = run_plugin(&call("for (;;) {}", Handler::Event("x".into())), limits);
        assert!(runaway.error.unwrap().contains("ran past"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let hog = run_plugin(
            &call(
                "const a = []; for (;;) a.push(new Array(100000).fill(7));",
                Handler::Event("x".into()),
            ),
            PluginLimits {
                time: Duration::from_secs(5),
                memory: 8 * 1024 * 1024,
            },
        );
        assert!(hog.error.is_some());
        // There's nothing of the machine to reach.
        let reach = run_plugin(
            &call(
                r#"trek.on("x", () => trek.log(typeof require, typeof process, typeof fetch, typeof Deno));"#,
                Handler::Event("x".into()),
            ),
            PluginLimits::default(),
        );
        assert_eq!(reach.logs, vec!["undefined undefined undefined undefined"]);
    }
}
