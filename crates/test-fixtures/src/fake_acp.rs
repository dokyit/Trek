//! A stand-in ACP agent for tests: answers initialize, session/new, session/load and
//! session/prompt (saying how many text blocks it got), and logs every message it's sent to
//! acp-log.jsonl in its working folder. A prompt "hold" isn't answered until the next prompt, a
//! "refuse" that comes while one is held is turned down, and the held one then ends.
//! session/new and session/load turn down `mcpServers` that aren't what the spec says: an array
//! of stdio servers {name, command, args, env: [{name, value}]}, plus {type: "http", name, url,
//! headers: [{name, value}]} only when the agent said it takes HTTP. It says so when its working
//! folder has a fake-acp-mcp.json, which becomes its `mcpCapabilities` (e.g. {"http": true}).
//! With a fake-acp-config.json ({"models": {id: [effort values]}, "current": id}) it offers a
//! model select and the current model's effort select as OpenCode 2 does: a model switch brings
//! that model's levels (none: no effort select), on "default" if it has one, else its first.
//! Like OpenCode 1.x, it also sends them as a `config_option_update` after `session/new`'s
//! answer.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub fn run() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("acp-log.jsonl")
        .unwrap_or_else(|e| panic!("log: {e}"));
    let mut sessions = 0u64;
    let mut held: Option<Value> = None;
    let caps = read_json("fake-acp-mcp.json").unwrap_or_else(|| json!({}));
    let mut config = read_json("fake-acp-config.json");
    let mut effort = config.as_ref().and_then(|c| default_effort(c));

    let mut lines = stdin.lock().split(b'\n');
    while let Some(Ok(line)) = lines.next() {
        let Ok(m) = serde_json::from_slice::<Value>(&line) else { continue };
        let Some(method) = m.get("method").filter(|m| !m.is_null()) else { continue };
        let entry = json!({ "method": method, "params": m["params"] });
        writeln!(log, "{entry}").unwrap();
        log.flush().unwrap();
        let Some(id) = m.get("id").filter(|i| !i.is_null()).cloned() else { continue };
        let method = method.as_str().unwrap_or_default();
        let mut result = json!({});
        if method == "initialize" {
            result = json!({ "protocolVersion": 1, "agentCapabilities": { "loadSession": true, "mcpCapabilities": caps.clone() } });
        } else if matches!(method, "session/new" | "session/load") {
            if let Some(why) = bad_mcp(&m["params"]["mcpServers"], &caps) {
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("Invalid params: {why}") } }));
                continue;
            }
            if method == "session/new" {
                sessions += 1;
                result = json!({ "sessionId": format!("fake-{sessions}") });
                if let Some(c) = &config {
                    result["configOptions"] = config_options(c, effort.as_ref());
                }
            }
        } else if method == "session/set_config_option" && config.is_some() {
            let c = config.as_mut().unwrap();
            let config_id = m["params"]["configId"].as_str().unwrap_or_default();
            let value = m["params"]["value"].as_str().unwrap_or_default();
            if config_id == "model" && c["models"].as_object().is_some_and(|m| m.contains_key(value)) {
                c["current"] = json!(value);
                effort = default_effort(c);
            } else if config_id == "effort" && levels_of(c).iter().any(|l| l.as_str() == Some(value)) {
                effort = Some(json!(value));
            } else {
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("Invalid params: no {config_id} {value}") } }));
                continue;
            }
            result = json!({ "configOptions": config_options(c, effort.as_ref()) });
        } else if method == "session/prompt" {
            let texts: Vec<&Value> = m["params"]["prompt"]
                .as_array()
                .map(|p| p.iter().filter(|b| b["type"].as_str() == Some("text")).collect())
                .unwrap_or_default();
            let text = texts.first().and_then(|t| t["text"].as_str()).unwrap_or_default();
            if text == "hold" {
                held = Some(id);
                continue;
            }
            if text == "refuse" && held.is_some() {
                say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "a prompt is already running" } }));
                say(&mut out, json!({ "jsonrpc": "2.0", "id": held.take().unwrap(), "result": { "stopReason": "end_turn" } }));
                continue;
            }
            let update = json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": format!("heard {}", texts.len()) } });
            say(&mut out, json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": m["params"]["sessionId"], "update": update } }));
            result = json!({ "stopReason": "end_turn" });
        }
        say(&mut out, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        if method == "session/new" && let Some(c) = &config {
            let update = json!({ "sessionUpdate": "config_option_update", "configOptions": config_options(c, effort.as_ref()) });
            say(&mut out, json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": result["sessionId"].clone(), "update": update } }));
        }
    }
}

fn say(out: &mut impl Write, msg: Value) {
    writeln!(out, "{msg}").unwrap();
    out.flush().unwrap();
}

fn read_json(file: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// The current model's effort levels.
fn levels_of(config: &Value) -> Vec<Value> {
    let Some(current) = config["current"].as_str() else { return vec![] };
    config["models"][current].as_array().cloned().unwrap_or_default()
}

/// The model's levels, on "default" if it has one, else its first.
fn default_effort(config: &Value) -> Option<Value> {
    let levels = levels_of(config);
    levels.iter().find(|l| *l == "default").or_else(|| levels.first()).cloned()
}

fn ucfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn config_options(config: &Value, effort: Option<&Value>) -> Value {
    let mut models: Vec<&String> = config["models"].as_object().map(|m| m.keys().collect()).unwrap_or_default();
    models.sort();
    let mut options = vec![json!({
        "id": "model", "category": "model", "type": "select", "currentValue": config["current"],
        "options": models.iter().map(|v| json!({ "value": v, "name": v.to_uppercase() })).collect::<Vec<_>>(),
    })];
    let levels = levels_of(config);
    if !levels.is_empty() {
        options.push(json!({
            "id": "effort", "category": "thought_level", "type": "select", "currentValue": effort,
            "options": levels.iter().map(|l| json!({ "value": l, "name": ucfirst(l.as_str().unwrap_or_default()) })).collect::<Vec<_>>(),
        }));
    }
    json!(options)
}

/// JSON values Perl's `!ref` allows where a scalar is wanted: strings, numbers, booleans.
fn scalar(v: &Value) -> bool {
    !v.is_null() && !v.is_object() && !v.is_array()
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

/// A value the way Perl's `"$v"` interpolation writes it.
fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Why `servers` isn't a valid `mcpServers`, or None.
fn bad_mcp(servers: &Value, caps: &Value) -> Option<String> {
    let Some(list) = servers.as_array() else { return Some("mcpServers must be an array".into()) };
    let pairs = |v: &Value| v.as_array().is_some_and(|l| l.iter().all(|p| p.is_object() && scalar(&p["name"]) && scalar(&p["value"])));
    for s in list {
        if !s.is_object() {
            return Some("a server must be an object".into());
        }
        if !scalar(&s["name"]) {
            return Some("a server needs a name".into());
        }
        let name = shown(&s["name"]);
        let kind = match &s["type"] {
            Value::Null => "stdio".to_string(),
            Value::String(t) => t.clone(),
            other => shown(other),
        };
        match kind.as_str() {
            "stdio" => {
                if !(scalar(&s["command"]) && s["args"].is_array() && pairs(&s["env"])) {
                    return Some(format!("{name}: stdio needs command, args and env"));
                }
            }
            "http" | "sse" => {
                if caps.get(kind.as_str()).is_none_or(|v| !truthy(v)) {
                    return Some(format!("{name}: {kind} isn't supported"));
                }
                if !(scalar(&s["url"]) && pairs(&s["headers"])) {
                    return Some(format!("{name}: {kind} needs url and headers"));
                }
            }
            _ => return Some(format!("{name}: unknown type {kind}")),
        }
    }
    None
}
