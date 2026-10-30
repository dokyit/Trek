//! trek-bench end to end: the binary against a fake OpenAI-wire server, with tools run here or
//! by this test playing the sandbox over stdio.

use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A server that answers request n with `bodies[n]` as an SSE stream, one connection each, and
/// returns the request bodies it was sent.
fn serve(bodies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut seen = vec![];
        for body in bodies {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = vec![];
            let mut buf = [0u8; 8192];
            let start = loop {
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0, "the request ended early");
                request.extend_from_slice(&buf[..n]);
            };
            let head = String::from_utf8_lossy(&request[..start]).to_lowercase();
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse().unwrap());
            while request.len() < start + length {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0, "the request body ended early");
                request.extend_from_slice(&buf[..n]);
            }
            seen.push(serde_json::from_slice(&request[start..start + length]).unwrap());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
        }
        seen
    });
    (url, handle)
}

/// An SSE answer: `delta`, then a usage chunk (OpenRouter's, with `cost`), then the end.
fn answer(delta: Value, finish: &str, cost: Option<f64>) -> String {
    let mut usage = json!({ "prompt_tokens": 1000, "completion_tokens": 50, "prompt_tokens_details": { "cached_tokens": 600 } });
    if let Some(c) = cost {
        usage["cost"] = json!(c);
    }
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({ "choices": [{ "delta": delta, "finish_reason": finish }] }),
        json!({ "choices": [], "usage": usage }),
    )
}

fn call(id: &str, name: &str, args: Value) -> Value {
    json!({ "tool_calls": [{ "index": 0, "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } }] })
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trek_bench_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prompt.txt"), "Do the task.").unwrap();
    std::fs::write(dir.join("key.txt"), "sk-test\n").unwrap();
    dir
}

fn bench(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_trek-bench"));
    cmd.arg("--prompt-file")
        .arg(dir.join("prompt.txt"))
        .arg("--out")
        .arg(dir.join("out/result.json"))
        .args(args)
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("TREK_BENCH_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    std::fs::create_dir_all(dir.join("out")).unwrap();
    cmd
}

fn lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

fn types(lines: &[Value]) -> Vec<&str> {
    lines.iter().map(|l| l["type"].as_str().unwrap()).collect()
}

#[test]
fn a_local_run_reports_each_request_and_the_totals() {
    let dir = scratch("local");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let (url, server) = serve(vec![
        answer(
            call("call_1", "bash", json!({ "command": "echo hi > out.txt" })),
            "tool_calls",
            Some(0.001),
        ),
        answer(json!({ "content": "finished" }), "stop", Some(0.002)),
    ]);
    let run = bench(
        &dir,
        &[
            "--cwd",
            work.to_str().unwrap(),
            "--model",
            "anthropic/claude-sonnet-5-5",
            "--base-url",
            &url,
            "--api-key-file",
            dir.join("key.txt").to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    let out = lines(&run.stdout);
    assert_eq!(run.status.code(), Some(0), "{out:#?}");
    assert_eq!(
        types(&out),
        [
            "request_start",
            "request_end",
            "tool_call",
            "tool_result",
            "request_start",
            "request_end",
            "text",
            "done"
        ]
    );
    assert_eq!(
        out[1]["usage"],
        json!({ "input": 400, "cache_read": 600, "cache_write": 0, "output": 50 })
    );
    assert_eq!(out[1]["cost_usd"], 0.001);
    assert_eq!(out[2]["args"], json!({ "command": "echo hi > out.txt" }));
    assert_eq!(out[3]["ok"], true);
    let done = &out[7];
    assert_eq!(done["requests"], 2);
    assert_eq!(done["turns"], 2);
    assert_eq!(done["tool_calls"], 1);
    assert_eq!(done["script_calls"], 0);
    assert_eq!(
        done["usage"],
        json!({ "input": 800, "cache_read": 1200, "cache_write": 0, "output": 100 })
    );
    assert!((done["cost_usd"].as_f64().unwrap() - 0.003).abs() < 1e-9);
    assert_eq!(done["cost_source"], "reported");
    assert_eq!(done["stopped"], "end_turn");
    assert_eq!(done["final_text"], "finished");
    assert_eq!(done["model"], "anthropic/claude-sonnet-5-5");
    assert_eq!(done["provider"], "openrouter");
    assert_eq!(done["base_url"], url);
    assert_eq!(
        std::fs::read_to_string(work.join("out.txt")).unwrap(),
        "hi\n"
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("out/result.json")).unwrap())
            .unwrap();
    assert_eq!(&written, done);

    // The key went with the requests, and the model and tools as given.
    let sent = server.join().unwrap();
    assert_eq!(sent[0]["model"], "anthropic/claude-sonnet-5-5");
    assert_eq!(sent[0]["tools"].as_array().unwrap().len(), 4);
    assert_eq!(sent[0]["reasoning"], json!({ "effort": "high" }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stdio_run_asks_the_sandbox_for_every_tool() {
    let dir = scratch("stdio");
    let (url, server) = serve(vec![
        answer(
            call(
                "call_1",
                "write_file",
                json!({ "path": "a.txt", "content": "x\n" }),
            ),
            "tool_calls",
            None,
        ),
        answer(
            call("call_2", "bash", json!({ "command": "cat a.txt" })),
            "tool_calls",
            None,
        ),
        answer(json!({ "content": "ok" }), "stop", None),
    ]);
    // A provider Trek doesn't know, with no key: an OpenAI-compatible local server.
    let mut child = bench(
        &dir,
        &[
            "--cwd",
            "/app",
            "--model",
            "qwen",
            "--provider",
            "litellm",
            "--wire",
            "openai",
            "--base-url",
            &url,
            "--executor",
            "stdio",
            "--command-timeout-s",
            "90",
        ],
    )
    .stdin(Stdio::piped())
    .spawn()
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut seen = vec![];
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let v: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let reply = match v["type"].as_str().unwrap() {
            "write" => Some(json!({ "type": "result", "id": v["id"] })),
            "exec" => Some(
                json!({ "type": "result", "id": v["id"], "stdout": "x\n", "stderr": "", "exit_code": 0, "timed_out": false }),
            ),
            _ => None,
        };
        if let Some(reply) = reply {
            writeln!(stdin, "{reply}").unwrap();
        }
        let done = v["type"] == "done";
        seen.push(v);
        if done {
            break;
        }
    }
    assert_eq!(child.wait().unwrap().code(), Some(0), "{seen:#?}");
    assert_eq!(
        types(&seen),
        [
            "request_start",
            "request_end",
            "tool_call",
            "write",
            "tool_result",
            "request_start",
            "request_end",
            "tool_call",
            "exec",
            "tool_result",
            "request_start",
            "request_end",
            "text",
            "done"
        ]
    );
    assert_eq!(seen[3]["path"], "/app/a.txt");
    assert_eq!(seen[3]["content_b64"], "eAo=");
    assert_eq!(seen[3]["mode"], "0644");
    assert_eq!(
        (&seen[8]["command"], &seen[8]["cwd"], &seen[8]["timeout_s"]),
        (&json!("cat a.txt"), &json!("/app"), &json!(90))
    );
    assert_eq!(seen[9]["chars"], 2);
    let done = seen.last().unwrap();
    assert_eq!(done["tool_calls"], 2);
    assert_eq!(done["provider"], "litellm");
    assert_eq!(done["executor"], "stdio");
    // No cost from the server and no price for the model: none is made up.
    assert_eq!(done["cost_usd"], Value::Null);
    assert_eq!(done["requests_without_cost"], 3);
    // A local server isn't sent a reasoning effort.
    let sent = server.join().unwrap();
    assert!(sent[0].get("reasoning").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_script_run_counts_its_nested_calls() {
    let dir = scratch("script");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let code = "await write('n.txt', 'one\\ntwo\\n'); const t = await read('n.txt'); \
                const r = await bash('wc -l < n.txt'); print(t.length, r.stdout.trim());";
    let (url, server) = serve(vec![
        answer(
            call("call_1", "script", json!({ "code": code })),
            "tool_calls",
            Some(0.01),
        ),
        answer(json!({ "content": "done" }), "stop", Some(0.01)),
    ]);
    let run = bench(
        &dir,
        &[
            "--cwd",
            work.to_str().unwrap(),
            "--model",
            "m",
            "--base-url",
            &url,
            "--api-key-file",
            dir.join("key.txt").to_str().unwrap(),
            "--script",
            "on",
            "--effort",
            "off",
        ],
    )
    .output()
    .unwrap();
    let out = lines(&run.stdout);
    assert_eq!(run.status.code(), Some(0), "{out:#?}");
    let script = out.iter().find(|l| l["type"] == "script_result").unwrap();
    assert_eq!(script["nested_calls"], 3);
    assert_eq!(
        script["calls_by_tool"],
        json!({ "bash": 1, "read": 1, "write": 1 })
    );
    assert_eq!(script["timed_out"], false);
    let done = out.last().unwrap();
    assert_eq!(done["script_calls"], 1);
    assert_eq!(done["nested_calls"], 3);
    assert_eq!(done["tool_calls"], 1);
    assert_eq!(done["script"], true);
    assert!(dir.join("out/script-logs").is_dir());
    let sent = server.join().unwrap();
    let tools = sent[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    // The program's printed line went back to the model as the tool result.
    let result = sent[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(result["content"].as_str().unwrap().trim(), "8 2");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_key_is_an_error_with_a_done() {
    let dir = scratch("nokey");
    let run = bench(&dir, &["--cwd", "/tmp", "--model", "m"])
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(2));
    let out = lines(&run.stdout);
    assert_eq!(types(&out), ["done"]);
    assert_eq!(out[0]["stopped"], "error");
    assert!(
        out[0]["error"]
            .as_str()
            .unwrap()
            .contains("OPENROUTER_API_KEY"),
        "{}",
        out[0]
    );
    assert_eq!(out[0]["requests"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_turn_cap_ends_the_run_with_its_own_code() {
    let dir = scratch("cap");
    let (url, _server) = serve(vec![answer(
        call("call_1", "bash", json!({ "command": "true" })),
        "tool_calls",
        None,
    )]);
    let run = bench(
        &dir,
        &[
            "--cwd",
            dir.to_str().unwrap(),
            "--model",
            "m",
            "--base-url",
            &url,
            "--api-key-file",
            dir.join("key.txt").to_str().unwrap(),
            "--max-turns",
            "1",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(run.status.code(), Some(3));
    let out = lines(&run.stdout);
    let done = out.last().unwrap();
    assert_eq!(done["stopped"], "max_turns");
    assert_eq!(done["error"], Value::Null);
    assert_eq!(done["requests"], 1);
    assert_eq!(done["tool_calls"], 1);
    let _ = std::fs::remove_dir_all(dir);
}
