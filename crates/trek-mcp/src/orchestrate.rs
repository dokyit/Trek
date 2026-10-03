//! `trek-mcp orchestrate`: lets an agent in Trek start and follow sub-agents (other agents and
//! models, run by Trek as child threads). Every call is passed to the Trek that started the
//! session over its local socket (`trek_ipc`); Trek does the work and the checking.
//!
//! Calls run side by side, each on a thread of its own: an agent that consults two models at
//! once waits on both together, and `ping` is answered while they run. A call the client
//! cancels (`notifications/cancelled`) gives up its connection and gets no answer.

use crate::rpc::{self, ToolDef, ToolSet};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

pub struct Orchestrate {
    client: Option<trek_ipc::Client>,
}

impl Orchestrate {
    pub fn from_env() -> Self {
        Self { client: trek_ipc::Client::from_env() }
    }
}

impl ToolSet for Orchestrate {
    fn family(&self) -> &'static str {
        "orchestrate"
    }

    fn instructions(&self) -> &'static str {
        "Trek runs other agents and models as sub-agents of this thread. Use list_models to see what's available, delegate_task to start one (advise: read-only review; implement: may change files), and task_status / task_result / cancel_task to follow it. Sub-agents see only the prompt you give them."
    }

    fn tools(&self) -> Vec<ToolDef> {
        trek_ipc::tools().into_iter().map(|(name, description, input_schema)| ToolDef { name, description, input_schema }).collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> rpc::ToolResult {
        call(self.client.as_ref(), name, args, None)
    }
}

/// Run one tool call against Trek. `handle` receives the connection's socket as soon as it's
/// open, so a cancel can shut it down.
fn call(client: Option<&trek_ipc::Client>, name: &str, args: &Value, handle: Option<&dyn Fn(std::os::unix::net::UnixStream)>) -> rpc::ToolResult {
    let client = client.ok_or("These tools work only in agents that Trek started (Trek's connection details are missing).")?;
    let mut conn = client.connect()?;
    if let (Some(handle), Ok(stream)) = (handle, conn.handle()) {
        handle(stream);
    }
    let result = conn.call(name, args)?;
    // Trek answers with JSON; agents read it best as indented text.
    let text = match &result {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    Ok(vec![rpc::text(text)])
}

/// Serve stdin/stdout like `main`, with tool calls on threads of their own.
pub fn serve(tools: Orchestrate) {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    // Calls in flight, by request id (as JSON): whether the client cancelled it, and its socket.
    let inflight: Arc<Mutex<HashMap<String, (bool, Option<std::os::unix::net::UnixStream>)>>> = Default::default();
    let client = Arc::new(tools.client.clone());
    let mut tools = tools;
    let mut server = rpc::Server::new();
    let write = |out: &Arc<Mutex<std::io::Stdout>>, v: &Value| {
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(o, "{v}").and_then(|_| o.flush());
    };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Option<Value> = serde_json::from_str(&line).ok();
        // A cancelled call: drop its connection; Trek stops waiting on its behalf.
        if let Some(m) = msg.as_ref().filter(|m| m["method"] == "notifications/cancelled") {
            let key = m["params"]["requestId"].to_string();
            if let Some((cancelled, stream)) = inflight.lock().unwrap().get_mut(&key) {
                *cancelled = true;
                if let Some(stream) = stream.take() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
            continue;
        }
        let call_of = msg.as_ref().filter(|m| m["method"] == "tools/call" && m.get("id").is_some()).map(|m| (m["id"].clone(), m["params"].clone()));
        let Some((id, params)) = call_of else {
            if let Some(response) = server.handle_line(&line, &mut tools) {
                let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
                if writeln!(o, "{response}").and_then(|_| o.flush()).is_err() {
                    break;
                }
            }
            continue;
        };
        let name = params["name"].as_str().unwrap_or_default().to_string();
        if !tools.tools().iter().any(|t| t.name == name) {
            write(&out, &rpc::error_response(id, rpc::INVALID_PARAMS, &format!("Unknown tool: {name}")));
            continue;
        }
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => {
                write(&out, &rpc::error_response(id, rpc::INVALID_PARAMS, "params.arguments must be an object"));
                continue;
            }
        };
        let key = id.to_string();
        inflight.lock().unwrap().insert(key.clone(), (false, None));
        let (out, inflight, client) = (out.clone(), inflight.clone(), client.clone());
        std::thread::spawn(move || {
            eprintln!("trek-mcp: tools/call {name}");
            let keep = |stream: std::os::unix::net::UnixStream| {
                match inflight.lock().unwrap().get_mut(&key) {
                    // Cancelled before it connected.
                    Some((true, _)) => _ = stream.shutdown(std::net::Shutdown::Both),
                    Some((false, slot)) => *slot = Some(stream),
                    None => {}
                }
            };
            let result = call(client.as_ref().as_ref(), &name, &args, Some(&keep));
            // A cancelled request gets no answer (MCP).
            if inflight.lock().unwrap().remove(&key).is_some_and(|(cancelled, _)| cancelled) {
                return;
            }
            let response = match result {
                Ok(content) => json!({ "jsonrpc": "2.0", "id": id, "result": { "content": content } }),
                Err(message) => {
                    eprintln!("trek-mcp: {name} failed");
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [rpc::text(message)], "isError": true } })
                }
            };
            write(&out, &response);
        });
    }
    eprintln!("trek-mcp: stdin closed, exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_shared_tools() {
        let tools = Orchestrate { client: None };
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        assert_eq!(names, ["list_models", "delegate_task", "task_status", "task_result", "cancel_task"]);
        assert!(tools.tools().iter().all(|t| t.to_json()["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn without_trek_calls_say_why() {
        let mut tools = Orchestrate { client: None };
        let err = tools.call("list_models", &json!({})).unwrap_err();
        assert!(err.contains("only in agents that Trek started"), "{err}");
    }

    #[test]
    fn calls_reach_trek_and_come_back_as_text() {
        use std::io::BufReader;
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("trek-mcp-orch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut w = stream;
            let _hello = trek_ipc::read_frame(&mut r, trek_ipc::MAX_FRAME).unwrap();
            w.write_all(trek_ipc::encode(&json!({"ok": true})).as_bytes()).unwrap();
            let req: Value = serde_json::from_str(&trek_ipc::read_frame(&mut r, trek_ipc::MAX_FRAME).unwrap().unwrap()).unwrap();
            assert_eq!(req["method"], "task_status");
            assert_eq!(req["params"]["id"], "abc");
            w.write_all(trek_ipc::encode(&trek_ipc::reply(&req["id"], Ok(json!({"id": "abc", "status": "done"})))).as_bytes()).unwrap();
        });
        let mut tools = Orchestrate { client: Some(trek_ipc::Client { socket: path, token: "t".into(), session: "s".into() }) };
        let out = tools.call("task_status", &json!({"id": "abc"})).unwrap();
        let text = out[0]["text"].as_str().unwrap();
        assert!(text.contains("\"status\": \"done\""), "{text}");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
