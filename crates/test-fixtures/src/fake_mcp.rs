//! A stand-in MCP server (stdio) for tests. It logs a line to stdout before the protocol starts
//! (as some real servers do), asks the client to `ping` before answering initialize, and lists
//! its tools in two pages. Its first argument picks how it behaves:
//!   ok      answer normally; a tool `env_ok` is listed when FAKE_TOKEN is "abc"
//!   crash   print an error to stderr and exit before answering
//!   silent  read everything and never answer
//!   refuse  answer initialize with a JSON-RPC error

use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub fn run(mode: Option<String>) {
    let mode = mode.as_deref().unwrap_or("ok");
    if mode == "crash" {
        eprintln!("npm warn exec The following package was not found and will be installed");
        eprintln!("Error: GITHUB_PERSONAL_ACCESS_TOKEN environment variable is required");
        std::process::exit(1);
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    writeln!(out, "fake-mcp starting up").unwrap();
    out.flush().unwrap();

    let mut lines = stdin.lock().split(b'\n');
    while let Some(Ok(line)) = lines.next() {
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&line) else { continue };
        if mode == "silent" {
            continue;
        }
        let method = msg["method"].as_str().unwrap_or_default();
        let id = msg["id"].clone();
        if method == "initialize" {
            if mode == "refuse" {
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "Unsupported protocol version" } }));
                continue;
            }
            // A request of the server's own first; the client must answer it and carry on.
            say(&mut out, json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "ping" }));
            let reply = lines
                .next()
                .and_then(|l| l.ok())
                .and_then(|l| serde_json::from_slice::<Value>(&l).ok())
                .unwrap_or_else(|| json!({}));
            assert!(reply["id"] == json!("srv-1") && truthy(&reply["result"]), "no ping reply");
            say(
                &mut out,
                json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": msg["params"]["protocolVersion"],
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "fake-mcp", "version": "1.0" },
                } }),
            );
        } else if method == "notifications/initialized" {
            // Nothing to say.
        } else if method == "tools/list" {
            if msg["params"]["cursor"].is_null() {
                say(&mut out, json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": { "level": "info", "data": "listing" } }));
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [{ "name": "echo" }, { "name": "add" }], "nextCursor": "page2" } }));
            } else {
                let mut tools = vec![json!({ "name": "get_time" })];
                if std::env::var("FAKE_TOKEN").as_deref() == Ok("abc") {
                    tools.push(json!({ "name": "env_ok" }));
                }
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } }));
            }
        } else if !id.is_null() {
            say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } }));
        }
    }
}

fn say(out: &mut impl Write, msg: Value) {
    writeln!(out, "{msg}").unwrap();
    out.flush().unwrap();
}

/// What `if $v` answers in Perl: undef, 0, "0", "" and false are falsy.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !(s.is_empty() || s == "0"),
        _ => true,
    }
}
