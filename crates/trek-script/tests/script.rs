//! Programs against a fake host: no network, no shell, no files outside a temp dir for the log.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trek_script::{ScriptHost, ScriptLimits, ScriptResult, async_trait, run_script};

/// Records every call, keeps a little in-memory file system, and counts how many calls overlap.
#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<(String, Value)>>,
    files: Mutex<HashMap<String, String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    /// Set when a `hang` call's future is dropped before it finishes.
    hang_dropped: Arc<AtomicBool>,
}

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ScriptHost for FakeHost {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((tool.to_string(), args.clone()));
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let r = self.answer(tool, &args).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        r
    }
}

impl FakeHost {
    async fn answer(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let s = |k: &str| args[k].as_str().unwrap_or_default().to_string();
        match tool {
            "bash" => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(
                    json!({ "stdout": format!("ran {}\n", s("command")), "stderr": "", "exit_code": 0 }),
                )
            }
            "read" => match self.files.lock().unwrap().get(&s("path")) {
                Some(c) => Ok(json!({ "content": c })),
                None => Err(format!("no such file: {}", s("path"))),
            },
            "write" => {
                self.files.lock().unwrap().insert(s("path"), s("content"));
                Ok(json!({}))
            }
            "edit" => {
                let mut files = self.files.lock().unwrap();
                let f = files.get_mut(&s("path")).ok_or("no such file")?;
                if !f.contains(&s("old")) {
                    return Err("old text not found".into());
                }
                *f = f.replacen(&s("old"), &s("new"), 1);
                Ok(json!({}))
            }
            "big" => Ok(json!({ "stdout": format!("BEGIN{}END", "x".repeat(100_000)) })),
            "fail_big" => Err("E".repeat(10_000_000)),
            "hang" => {
                let _guard = SetOnDrop(self.hang_dropped.clone());
                tokio::time::sleep(Duration::from_secs(60)).await;
                std::mem::forget(_guard);
                Ok(json!({}))
            }
            _ => Err(format!("unknown tool {tool}")),
        }
    }
}

async fn run(code: &str, host: &Arc<FakeHost>, limits: ScriptLimits) -> ScriptResult {
    run_script(code, host.clone(), limits).await
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trek-script-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test]
async fn prints_and_returns() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        r#"print("hi", 1, {a: 1}); console.log([1, 2], null); return {done: true};"#,
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "hi 1 {\"a\":1}\n[1,2] null\n{\"done\":true}\n");
    assert!(!r.timed_out);
    assert_eq!(r.nested_calls, 0);
}

#[tokio::test]
async fn fifty_bash_calls_run_eight_at_a_time() {
    let host = Arc::new(FakeHost::default());
    let t = Instant::now();
    let r = run(
        r#"
        const cmds = Array.from({length: 50}, (_, i) => `echo ${i}`);
        const out = await Promise.all(cmds.map((c) => bash(c)));
        print(out.length, out[49].stdout.trim(), out.every((o) => o.exit_code === 0));
        "#,
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "50 ran echo 49 true\n");
    assert_eq!(r.nested_calls, 50);
    assert_eq!(r.calls_by_tool.get("bash"), Some(&50));
    assert_eq!(host.calls.lock().unwrap().len(), 50);
    assert_eq!(host.max_in_flight.load(Ordering::SeqCst), 8);
    // Seven rounds of 20 ms, not fifty.
    assert!(
        t.elapsed() < Duration::from_millis(700),
        "{:?}",
        t.elapsed()
    );
}

#[tokio::test]
async fn long_output_keeps_head_and_tail() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "for (let i = 0; i < 100000; i++) print(`line ${i}`);",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert!(r.truncated_bytes > 1_000_000, "{}", r.truncated_bytes);
    assert!(r.printed.len() <= 16 * 1024 + 200, "{}", r.printed.len());
    assert!(r.printed.starts_with("line 0\nline 1\n"));
    assert!(r.printed.ends_with("line 99999\n"));
    assert!(r.printed.contains("bytes of output dropped"));
}

#[tokio::test]
async fn an_uncaught_error_comes_back_with_the_output_so_far() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "print('before');\nawait bash('ls');\nthrow new TypeError('boom');",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.printed, "before\n");
    let e = r.error.unwrap();
    assert!(e.starts_with("TypeError: boom"), "{e}");
    assert!(e.contains("script:3"), "the line is the program's own: {e}");
    assert_eq!(e.matches("boom").count(), 1, "{e}");
    assert_eq!(r.nested_calls, 1);

    let r = run("print('x'); let = ;", &host, ScriptLimits::default()).await;
    assert!(r.error.unwrap().starts_with("SyntaxError"));
    assert_eq!(r.printed, "");

    let r = run("throw {code: 7}", &host, ScriptLimits::default()).await;
    assert_eq!(r.error.as_deref(), Some("Uncaught {\"code\":7}"));
}

#[tokio::test]
async fn a_tool_error_throws_and_can_be_caught() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        r#"
        try { await read("missing.txt"); } catch (e) { print("caught:", e.message); }
        await tool("nope", {});
        "#,
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.printed, "caught: read: no such file: missing.txt\n");
    assert!(r.error.unwrap().contains("nope: unknown tool nope"));
}

#[tokio::test]
async fn a_busy_loop_times_out_with_its_output() {
    let host = Arc::new(FakeHost::default());
    let limits = ScriptLimits {
        wall_clock: Duration::from_millis(500),
        ..ScriptLimits::default()
    };
    let t = Instant::now();
    let r = run("print('start'); while (true) {}", &host, limits.clone()).await;
    assert!(r.timed_out);
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "start\n");
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(r.to_model_text().contains("ran out of time"));

    // A catch can't swallow the interrupt.
    let r = run(
        "for (;;) { try { while (true) {} } catch (e) { print('caught'); } }",
        &host,
        limits.clone(),
    )
    .await;
    assert!(r.timed_out);
    assert_eq!(r.printed, "");

    // Waiting, not spinning: a promise that never settles, and a poll that never ends.
    let r = run(
        "print('a'); await new Promise(() => {});",
        &host,
        limits.clone(),
    )
    .await;
    assert!(r.timed_out);
    assert_eq!(r.printed, "a\n");
    let r = run(
        "let n = 0; while (true) { await sleep(50); print(n++); }",
        &host,
        limits,
    )
    .await;
    assert!(r.timed_out);
    assert!(r.printed.starts_with("0\n1\n"), "{}", r.printed);
}

#[tokio::test]
async fn a_timeout_aborts_calls_in_flight() {
    let host = Arc::new(FakeHost::default());
    let limits = ScriptLimits {
        wall_clock: Duration::from_millis(300),
        ..ScriptLimits::default()
    };
    let t = Instant::now();
    let r = run(
        "print('go'); await tool('hang', {}); print('never');",
        &host,
        limits,
    )
    .await;
    assert!(r.timed_out);
    assert_eq!(r.printed, "go\n");
    assert!(t.elapsed() < Duration::from_secs(3));
    // The abort lands on the host's runtime a moment later.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(host.hang_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_the_run_stops_the_program() {
    let host = Arc::new(FakeHost::default());
    let fut = run_script(
        "await tool('hang', {});",
        host.clone(),
        ScriptLimits::default(),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), fut)
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(host.hang_dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn read_edit_write_chain() {
    let host = Arc::new(FakeHost::default());
    host.files
        .lock()
        .unwrap()
        .insert("a.txt".into(), "hello world".into());
    let r = run(
        r#"
        print(await read("a.txt"));
        await edit("a.txt", "world", "there");
        await write("b.txt", (await read("a.txt")).toUpperCase());
        print(await read("b.txt"));
        "#,
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "hello world\nHELLO THERE\n");
    assert_eq!(host.files.lock().unwrap()["b.txt"], "HELLO THERE");
    let tools: Vec<String> = host
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.0.clone())
        .collect();
    assert_eq!(tools, ["read", "edit", "read", "write", "read"]);
    assert_eq!(
        host.calls.lock().unwrap()[1].1,
        json!({ "path": "a.txt", "old": "world", "new": "there" })
    );
}

#[tokio::test]
async fn the_log_has_every_call_and_print() {
    let host = Arc::new(FakeHost::default());
    let dir = temp_dir("log");
    let limits = ScriptLimits {
        log_dir: Some(dir.clone()),
        ..ScriptLimits::default()
    };
    let r = run(
        "await bash('ls', {timeout: 5}); print('one'); try { await read('x') } catch {}",
        &host,
        limits,
    )
    .await;
    let path = r.log_path.clone().expect("a log");
    assert!(path.starts_with(&dir));
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["start", "call", "print", "call", "end"]);
    assert!(lines[0]["code"].as_str().unwrap().starts_with("await bash"));
    let bash = &lines[1];
    assert_eq!(bash["tool"], "bash");
    assert_eq!(bash["args"], json!({ "command": "ls", "timeout_s": 5 }));
    assert_eq!(bash["ok"], true);
    assert_eq!(bash["result"]["stdout"], "ran ls\n");
    assert!(bash["bytes"].as_u64().unwrap() > 0);
    assert!(bash["ms"].as_u64().unwrap() >= 20);
    assert_eq!(lines[2]["text"], "one");
    assert_eq!(lines[3]["ok"], false);
    assert_eq!(lines[3]["error"], "no such file: x");
    assert_eq!(lines[4]["nested_calls"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_big_result_is_cut_for_the_program_and_logged_to_64_kib() {
    let host = Arc::new(FakeHost::default());
    let dir = temp_dir("big");
    let limits = ScriptLimits {
        max_result_bytes: 10_000,
        log_dir: Some(dir.clone()),
        ..ScriptLimits::default()
    };
    let r = run(
        "const {stdout} = await tool('big'); print(stdout.length < 10000, stdout.startsWith('BEGIN'), stdout.endsWith('END'));",
        &host,
        limits,
    )
    .await;
    assert_eq!(r.printed, "true true true\n");
    let log = std::fs::read_to_string(r.log_path.unwrap()).unwrap();
    let call: Value = serde_json::from_str(log.lines().nth(1).unwrap()).unwrap();
    // The log has more than the program got, and says how big the whole was.
    let logged = call["result"]["stdout"].as_str().unwrap();
    assert!(
        logged.len() > 60_000 && logged.len() <= 64 * 1024,
        "{}",
        logged.len()
    );
    assert!(logged.starts_with("BEGIN") && logged.ends_with("END"));
    assert!(call["bytes"].as_u64().unwrap() > 100_000);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_call_limit_holds() {
    let host = Arc::new(FakeHost::default());
    let limits = ScriptLimits {
        max_calls: 5,
        ..ScriptLimits::default()
    };
    let r = run(
        "for (let i = 0; i < 10; i++) { try { await bash('x') } catch (e) { print(i, e.message); break; } }",
        &host,
        limits,
    )
    .await;
    assert_eq!(r.nested_calls, 5);
    assert_eq!(host.calls.lock().unwrap().len(), 5);
    assert_eq!(
        r.printed,
        "5 bash: the script reached its limit of 5 tool calls\n"
    );
}

#[tokio::test]
async fn the_sandbox_has_no_way_out_but_the_tools() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        r#"
        print([typeof require, typeof process, typeof fetch, typeof std, typeof os,
               typeof __trek_call, typeof __trek_print, typeof __trek_run].join(" "));
        try { await import("fs"); print("imported"); } catch (e) { print("no import"); }
        "#,
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(
        r.printed,
        "undefined undefined undefined undefined undefined undefined undefined undefined\nno import\n"
    );
}

#[tokio::test]
async fn quick_unawaited_calls_finish_in_the_grace() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "write('a.txt', 'x'); bash('ls').then((o) => print(o.stdout.trim()));",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "ran ls\n");
    assert_eq!(r.detached, 0);
    assert_eq!(host.files.lock().unwrap()["a.txt"], "x");
}

#[tokio::test]
async fn slow_detached_work_is_stopped_after_the_grace_not_the_clock() {
    let host = Arc::new(FakeHost::default());
    let dir = temp_dir("detached");
    let limits = ScriptLimits {
        log_dir: Some(dir.clone()),
        ..ScriptLimits::default()
    };
    let t = Instant::now();
    let r = run(
        "setTimeout(() => print('late'), 300_000); tool('hang'); print('done');",
        &host,
        limits,
    )
    .await;
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    assert!(!r.timed_out, "the program returned");
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "done\n");
    assert_eq!(r.detached, 2);
    assert!(r.to_model_text().contains("2 detached calls or timers"));
    // The stopped call is logged, with how long it ran.
    let log = std::fs::read_to_string(r.log_path.unwrap()).unwrap();
    let hang: Value = log
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|l| l["tool"] == "hang")
        .expect("a record for the stopped call");
    assert_eq!(hang["ok"], false);
    assert!(hang["ms"].as_u64().unwrap() >= 1_500, "{hang}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(host.hang_dropped.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_cleared_timer_neither_fires_nor_holds_the_run() {
    let host = Arc::new(FakeHost::default());
    let t = Instant::now();
    let r = run(
        "const id = setTimeout(() => print('no'), 100); clearTimeout(id); setTimeout(() => print('yes'), 10); await sleep(200);",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.printed, "yes\n");
    assert_eq!(r.detached, 0);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[tokio::test]
async fn wide_chars_past_the_cap_keep_the_output() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "for (let i = 0; i < 2000; i++) print('€'.repeat(100));",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert!(r.truncated_bytes > 0);
    assert!(r.printed.starts_with('€') && r.printed.ends_with("€\n"));
}

#[tokio::test]
async fn a_huge_error_is_capped() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "throw new Error('A'.repeat(1e7))",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert!(r.error.as_ref().unwrap().starts_with("Error: AAAA"));
    assert!(
        r.to_model_text().len() < 8 * 1024,
        "{}",
        r.to_model_text().len()
    );

    let r = run("await tool('fail_big')", &host, ScriptLimits::default()).await;
    assert!(
        r.error
            .as_ref()
            .unwrap()
            .starts_with("Error: fail_big: EEEE")
    );
    assert!(
        r.to_model_text().len() < 8 * 1024,
        "{}",
        r.to_model_text().len()
    );
}

#[tokio::test]
async fn unhandled_rejections_become_warnings() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "setTimeout(() => { throw new Error('late'); }, 10); tool('nope'); await sleep(100); print('ok');",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "ok\n");
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("Error: late")));
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("nope: unknown tool nope"))
    );
    assert!(r.to_model_text().contains("[warning] unhandled rejection"));

    // One handled later, after it failed, isn't a warning.
    let r = run(
        "const p = tool('nope'); await sleep(50); try { await p } catch (e) { print('caught') }",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.printed, "caught\n");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[tokio::test]
async fn a_polluted_prototype_doesnt_break_calls() {
    let host = Arc::new(FakeHost::default());
    host.files
        .lock()
        .unwrap()
        .insert("a.txt".into(), "hi".into());
    let r = run(
        "Object.prototype.e = 1; Object.prototype.content = 'wrong'; JSON.parse = () => ({}); print(await read('a.txt'), (await bash('x')).exit_code);",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert_eq!(r.printed, "hi 0\n");
}

#[tokio::test]
async fn bad_arguments_throw() {
    let host = Arc::new(FakeHost::default());
    let r = run(
        "await tool('bash', () => 1)",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert!(r.error.unwrap().starts_with("TypeError: bash: arguments"));
    assert_eq!(r.nested_calls, 0);
}

#[tokio::test]
async fn the_log_is_capped() {
    let host = Arc::new(FakeHost::default());
    let dir = temp_dir("logcap");
    let limits = ScriptLimits {
        log_dir: Some(dir.clone()),
        max_log_bytes: 300_000,
        ..ScriptLimits::default()
    };
    let r = run(
        "await write('big.txt', 'w'.repeat(200_000)); for (let i = 0; i < 100_000; i++) print(`line ${i}`);",
        &host,
        limits,
    )
    .await;
    assert_eq!(r.error, None);
    let path = r.log_path.unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() < 310_000);
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // The write's 200 KB argument is kept to 64 KiB, its ends intact.
    let content = lines[1]["args"]["content"].as_str().unwrap();
    assert!(content.len() <= 64 * 1024 && content.starts_with("www") && content.ends_with("www"));
    let kinds: Vec<&str> = lines
        .iter()
        .rev()
        .take(2)
        .map(|l| l["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["end", "capped"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_engine_holds_up_under_abuse() {
    let host = Arc::new(FakeHost::default());
    // What's there: no shared memory (Atomics.wait would block past the clock), no wasm.
    let r = run(
        "print(typeof SharedArrayBuffer, typeof Atomics, typeof WebAssembly, typeof WeakRef, typeof FinalizationRegistry, typeof queueMicrotask)",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(
        r.printed,
        "undefined undefined undefined function function function\n"
    );

    let limits = ScriptLimits {
        wall_clock: Duration::from_millis(500),
        ..ScriptLimits::default()
    };
    let t = Instant::now();
    let r = run(
        "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0)",
        &host,
        limits.clone(),
    )
    .await;
    assert!(r.error.unwrap().starts_with("ReferenceError"));
    assert!(t.elapsed() < Duration::from_secs(1));

    // A huge allocation is a catchable error, not an abort.
    let r = run(
        "try { new ArrayBuffer(2 ** 30); print('allocated') } catch (e) { print('caught', e.name) } print('alive')",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert!(
        r.printed.starts_with("caught") && r.printed.ends_with("alive\n"),
        "{}",
        r.printed
    );

    // Deep recursion is a catchable error.
    let r = run(
        "const f = (n) => f(n + 1) + 1; try { f(0) } catch (e) { print('caught', e.name) }",
        &host,
        ScriptLimits::default(),
    )
    .await;
    assert_eq!(r.error, None);
    assert!(r.printed.starts_with("caught"), "{}", r.printed);

    // Catastrophic backtracking still answers to the clock.
    let t = Instant::now();
    let r = run(
        "print('start'); /(a+)+$/.test('a'.repeat(40) + 'b'); print('done')",
        &host,
        limits,
    )
    .await;
    assert!(r.timed_out);
    assert_eq!(r.printed, "start\n");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn runs_without_a_caller_runtime() {
    // A plain thread with no tokio context: calls run on the script thread's own runtime.
    let host = Arc::new(FakeHost::default());
    let fut = run_script(
        "print((await bash('pwd')).stdout.trim())",
        host,
        ScriptLimits::default(),
    );
    assert_eq!(block_on(fut).printed, "ran pwd\n");
}

/// A minimal executor, so the test above polls `run_script` with no tokio context at all.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}
