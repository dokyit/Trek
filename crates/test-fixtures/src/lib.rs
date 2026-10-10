//! Binaries the test suite spawns where it once used Perl scripts and shell tools, so the same
//! tests run on macOS and Windows: `fake-acp` (an ACP agent), `fake-mcp` (a stdio MCP server)
//! and `fixture` (small utilities: `sleep`, `cat`, `echo`, `print`, `stderr`, `exit`, and the
//! two fakes as verbs). Tests get them through `bin`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub mod fake_acp;
pub mod fake_mcp;
pub mod fixture;

/// The built `name` fixture binary (`fake-acp`, `fake-mcp`, `fixture`), ready for
/// `std::process::Command::new` or `tokio::process::Command::new` on either platform.
///
/// Unit tests run from `target/<profile>/deps/`, so the binaries live two levels up in
/// `target/<profile>/`. `cargo test --workspace` builds them, but `cargo test -p <crate>`
/// doesn't build another crate's bins: if `name` isn't there it is built once per process
/// with `cargo build -p trek-test-fixtures` (CARGO_TARGET_DIR honoured through the
/// environment; a `--release` test run builds `--release`).
///
/// The path is rendered with `/` separators (it still names the same file on Windows) so a
/// test can hand it to code that spots a path by its slashes, e.g. `AddedAgent::resolve`.
pub fn bin(name: &str) -> PathBuf {
    let dir = profile_dir();
    let file = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if !file.is_file() {
        build(&dir);
    }
    assert!(
        file.is_file(),
        "the {name} test fixture isn't built and `cargo build -p trek-test-fixtures` didn't produce {}",
        file.display()
    );
    let path = file.to_string_lossy().into_owned();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path).replace('\\', "/");
    PathBuf::from(path)
}

/// `target/<profile>/` beside the running test binary (`…/<profile>/deps/<test>`).
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's own path");
    let deps = exe.parent().expect("the test binary has a folder");
    if deps.file_name().is_some_and(|d| d == "deps") {
        deps.parent().expect("deps has a profile folder").to_path_buf()
    } else {
        deps.to_path_buf()
    }
}

/// Build the fixtures once, for `cargo test -p <one crate>` runs where they weren't a target.
fn build(profile_dir: &Path) {
    static BUILD: OnceLock<()> = OnceLock::new();
    BUILD.get_or_init(|| {
        // This crate's manifest is <workspace>/crates/test-fixtures.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("the workspace root");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = std::process::Command::new(cargo);
        cmd.args(["build", "-p", "trek-test-fixtures", "--locked"]).current_dir(root);
        if profile_dir.file_name().is_some_and(|d| d == "release") {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("couldn't run cargo to build the test fixtures");
        assert!(status.success(), "`cargo build -p trek-test-fixtures --locked` failed");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::time::Duration;

    fn fixture(args: &[&str]) -> Command {
        let mut c = Command::new(bin("fixture"));
        c.args(args);
        c
    }

    #[test]
    fn the_binaries_are_found() {
        for name in ["fixture", "fake-acp", "fake-mcp"] {
            assert!(bin(name).is_file(), "{name}");
        }
    }

    #[test]
    fn the_fixture_covers_the_shell_tools() {
        let out = fixture(&["echo", "hi", "there"]).output().unwrap();
        assert_eq!(out.stdout, b"hi there\n");
        let out = fixture(&["print", "/usr/bin"]).output().unwrap();
        assert_eq!(out.stdout, b"/usr/bin");
        let mut cat = fixture(&["cat", "--stderr", "err"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        cat.stdin.take().unwrap().write_all(b"hello").unwrap();
        let out = cat.wait_with_output().unwrap();
        assert_eq!((out.stdout.as_slice(), String::from_utf8_lossy(&out.stderr).trim()), (b"hello".as_slice(), "err"));
        let started = std::time::Instant::now();
        let out = fixture(&["cat", "--delay", "0.2"]).output().unwrap();
        assert_eq!(out.stdout, b"");
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(fixture(&["exit"]).status().unwrap().success());
        assert_eq!(fixture(&["exit", "7"]).status().unwrap().code(), Some(7));
        assert_eq!(fixture(&["bogus"]).status().unwrap().code(), Some(2));
        let started = std::time::Instant::now();
        fixture(&["sleep", "0.2"]).status().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200) && started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn ready_then_eof_says_so_and_exits_when_its_stdin_ends() {
        let mut child = fixture(&["ready-then-eof"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        assert!(child.try_wait().unwrap().is_none(), "it waits for the end of stdin");
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }

    /// Send `input`, one message a line, then collect every line the fake agent answered.
    fn ask(dir: &Path, input: &[Value]) -> Vec<Value> {
        let mut child = Command::new(bin("fake-acp"))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for m in input {
            writeln!(stdin, "{m}").unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn the_fake_agent_speaks_acp() {
        let dir = std::env::temp_dir().join(format!("trek-fixture-acp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let server = json!({ "name": "x", "command": "x", "args": [], "env": [{ "name": "A", "value": "b" }] });
        let answers = ask(
            &dir,
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": { "cwd": "/", "mcpServers": [server] } }),
                json!({ "jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": { "sessionId": "fake-1", "prompt": [{ "type": "text", "text": "hi" }] } }),
            ],
        );
        assert_eq!(answers[0]["result"]["protocolVersion"], json!(1));
        assert_eq!(answers[0]["result"]["agentCapabilities"]["loadSession"], json!(true));
        assert_eq!(answers[1]["result"]["sessionId"], json!("fake-1"));
        // The update precedes the prompt's own reply.
        assert_eq!(answers[2]["params"]["update"]["content"]["text"], json!("heard 1"));
        assert_eq!(answers[3]["result"]["stopReason"], json!("end_turn"));
        // Every request was logged, the notification too.
        let log = std::fs::read_to_string(dir.join("acp-log.jsonl")).unwrap();
        let methods: Vec<String> = log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["method"].as_str().unwrap().to_string()).collect();
        assert_eq!(methods, ["initialize", "notifications/initialized", "session/new", "session/prompt"]);

        // A bad mcpServers is turned down; a held prompt ends when a "refuse" is refused.
        let answers = ask(
            &dir,
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": { "mcpServers": { "x": {} } } }),
                json!({ "jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": { "sessionId": "s", "prompt": [{ "type": "text", "text": "hold" }] } }),
                json!({ "jsonrpc": "2.0", "id": 4, "method": "session/prompt", "params": { "sessionId": "s", "prompt": [{ "type": "text", "text": "refuse" }] } }),
            ],
        );
        assert!(answers[1]["error"]["message"].as_str().unwrap().contains("must be an array"), "{answers:?}");
        assert!(answers[2]["error"]["message"].as_str().unwrap().contains("a prompt is already running"));
        assert_eq!(answers[2]["id"], json!(4));
        assert_eq!((answers[3]["id"].clone(), answers[3]["result"]["stopReason"].clone()), (json!(3), json!("end_turn")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fake_agent_serves_its_config_options() {
        let dir = std::env::temp_dir().join(format!("trek-fixture-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fake-acp-config.json"), r#"{"models":{"b":["high","default"],"a":["low"],"c":[]},"current":"c"}"#).unwrap();
        let answers = ask(
            &dir,
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": { "cwd": "/", "mcpServers": [] } }),
                json!({ "jsonrpc": "2.0", "id": 3, "method": "session/set_config_option", "params": { "configId": "model", "value": "b" } }),
                json!({ "jsonrpc": "2.0", "id": 4, "method": "session/set_config_option", "params": { "configId": "effort", "value": "high" } }),
                json!({ "jsonrpc": "2.0", "id": 5, "method": "session/set_config_option", "params": { "configId": "model", "value": "nope" } }),
            ],
        );
        // session/new answers with the options, then announces them again.
        let options = &answers[1]["result"]["configOptions"];
        assert_eq!(options[0]["options"][0], json!({ "value": "a", "name": "A" }));
        assert_eq!(options[0]["currentValue"], json!("c"));
        let update = &answers[2]["params"]["update"];
        assert_eq!(update["sessionUpdate"], json!("config_option_update"));
        // c has no levels, so there is no effort select at all.
        assert_eq!(options.as_array().unwrap().len(), 1, "{options}");
        // Switching to b resets the effort to its default; setting high keeps it.
        let options = &answers[3]["result"]["configOptions"];
        assert_eq!((options[0]["currentValue"].clone(), options[1]["currentValue"].clone()), (json!("b"), json!("default")));
        assert_eq!(answers[4]["result"]["configOptions"][1]["currentValue"], json!("high"));
        assert!(answers[5]["error"]["message"].as_str().unwrap().contains("no model nope"), "{answers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake-mcp child, its stdin and a line reader on its stdout ("fake-mcp starting up" read already).
    fn mcp(mode: &str, token: Option<&str>) -> (Child, ChildStdin, BufReader<std::process::ChildStdout>) {
        let mut cmd = Command::new(bin("fake-mcp"));
        cmd.arg(mode).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(t) = token {
            cmd.env("FAKE_TOKEN", t);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        lines.read_line(&mut first).unwrap();
        assert_eq!(first.trim_end(), "fake-mcp starting up");
        (child, stdin, lines)
    }

    fn next_line(lines: &mut BufReader<std::process::ChildStdout>) -> Value {
        let mut line = String::new();
        assert!(lines.read_line(&mut line).unwrap() > 0, "the server stopped answering");
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn the_fake_mcp_server_speaks_mcp() {
        let (_child, mut stdin, mut lines) = mcp("ok", Some("abc"));
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#).unwrap();
        let ping = next_line(&mut lines);
        assert_eq!((ping["id"].clone(), ping["method"].clone()), (json!("srv-1"), json!("ping")));
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":"srv-1","result":{{}}}}"#).unwrap();
        let init = next_line(&mut lines);
        assert_eq!(init["result"]["serverInfo"]["name"], json!("fake-mcp"));
        writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#).unwrap();
        let note = next_line(&mut lines);
        assert_eq!(note["method"], json!("notifications/message"));
        let page1 = next_line(&mut lines);
        assert_eq!(page1["result"]["nextCursor"], json!("page2"));
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{{"cursor":"page2"}}}}"#).unwrap();
        let page2 = next_line(&mut lines);
        let names: Vec<&str> = page2["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["get_time", "env_ok"], "env_ok only with FAKE_TOKEN=abc");
    }

    #[test]
    fn the_fake_mcp_server_crashes_and_refuses() {
        let out = Command::new(bin("fake-mcp")).arg("crash").output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("GITHUB_PERSONAL_ACCESS_TOKEN"));

        let (_child, mut stdin, mut lines) = mcp("refuse", None);
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#).unwrap();
        let answer = next_line(&mut lines);
        assert_eq!(answer["error"]["code"], json!(-32602));
    }

    #[test]
    fn the_fake_mcp_server_in_silent_mode_never_answers() {
        let (mut child, mut stdin, _lines) = mcp("silent", None);
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(child.try_wait().unwrap().is_none(), "still running, still silent");
        child.kill().unwrap();
    }
}
  
