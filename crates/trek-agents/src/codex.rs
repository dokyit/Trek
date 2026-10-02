//! Codex via `codex app-server` (JSON-RPC over stdio, newline-delimited, no `jsonrpc` field).
//! Uses the user's own Codex login (ChatGPT plan or API key).

use crate::{AgentEvent, Command, Decision, SessionConfig, clip, diff_stat};
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};
use trek_core::{Effort, HandHolding, detect};

struct Rpc {
    stdin: ChildStdin,
    next_id: i64,
}

impl Rpc {
    async fn send(&mut self, v: &Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "id": id, "method": method, "params": params })).await?;
        Ok(id)
    }
}

/// Read until the response for `id` arrives, forwarding anything else.
async fn await_response(
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: i64,
    backlog: &mut Vec<Value>,
) -> Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["id"].as_i64() == Some(id) && v.get("method").is_none() {
            if let Some(err) = v.get("error") {
                bail!("codex: {}", err["message"].as_str().unwrap_or("request failed"));
            }
            return Ok(v["result"].clone());
        }
        backlog.push(v);
    }
    bail!("codex app-server exited")
}

fn sandbox_policy(h: HandHolding) -> Value {
    match h.codex_policy().0 {
        "read-only" => json!({ "type": "readOnly" }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        _ => json!({ "type": "workspaceWrite" }),
    }
}

fn effort_str(e: Effort) -> Option<&'static str> {
    // Codex app levels: low(Light) / medium / high / xhigh / max.
    match e {
        Effort::Off | Effort::Minimal | Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High => Some("high"),
        Effort::XHigh => Some("xhigh"),
        Effort::Max => Some("max"),
    }
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let bin = detect::which("codex").context("Codex isn't installed (npm i -g @openai/codex)")?;
    let mut child = tokio::process::Command::new(bin)
        .arg("app-server")
        .current_dir(&config.cwd)
        .env("PATH", detect::login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start codex app-server")?;
    let mut rpc = Rpc { stdin: child.stdin.take().unwrap(), next_id: 0 };
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut backlog = Vec::new();

    let id = rpc
        .request("initialize", json!({ "clientInfo": { "name": "trek", "title": "Trek", "version": trek_core::VERSION } }))
        .await?;
    await_response(&mut lines, id, &mut backlog).await?;
    rpc.send(&json!({ "method": "initialized" })).await?;

    let mut hand_holding = config.hand_holding;
    let (sandbox, approval, reviewer) = hand_holding.codex_policy();
    let cwd = config.cwd.display().to_string();
    let mut params = json!({
        "cwd": cwd, "sandbox": sandbox, "approvalPolicy": approval, "approvalsReviewer": reviewer,
    });
    if let Some(m) = &config.model {
        params["model"] = json!(m);
    }
    let result = match &config.resume {
        Some(thread_id) => {
            params["threadId"] = json!(thread_id);
            params["excludeTurns"] = json!(true);
            let id = rpc.request("thread/resume", params).await?;
            await_response(&mut lines, id, &mut backlog).await?
        }
        None => {
            let id = rpc.request("thread/start", params).await?;
            await_response(&mut lines, id, &mut backlog).await?
        }
    };
    let thread_id = result["thread"]["id"].as_str().context("no thread id")?.to_string();
    let mut model = result["model"].as_str().map(String::from).or(config.model.clone());
    let mut effort = config.effort;
    events.send(AgentEvent::Started { native_id: thread_id.clone(), model: model.clone() }).await?;

    let mut turn_id: Option<String> = None;
    // Approval requests awaiting the user: our request id → JSON-RPC id.
    let mut pending: HashMap<String, Value> = HashMap::new();

    for v in std::mem::take(&mut backlog) {
        handle_incoming(&v, &mut rpc, &events, &mut turn_id, &mut pending, hand_holding).await?;
    }

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt(text) => {
                        let (_, approval, reviewer) = hand_holding.codex_policy();
                        let mut p = json!({
                            "threadId": thread_id,
                            "input": [{ "type": "text", "text": text }],
                            "approvalPolicy": approval,
                            "approvalsReviewer": reviewer,
                            "sandboxPolicy": sandbox_policy(hand_holding),
                        });
                        if let Some(e) = effort_str(effort) { p["effort"] = json!(e); }
                        if let Some(m) = &model { p["model"] = json!(m); }
                        rpc.request("turn/start", p).await?;
                    }
                    Command::Interrupt => {
                        if let Some(t) = &turn_id {
                            rpc.request("turn/interrupt", json!({ "threadId": thread_id, "turnId": t })).await?;
                        }
                    }
                    Command::SetHandHolding(h) => hand_holding = h,
                    Command::SetModel { model: m, effort: e } => { model = Some(m); effort = e; }
                    Command::Respond { request_id, decision } => {
                        if let Some(rpc_id) = pending.remove(&request_id) {
                            let d = match decision {
                                Decision::Allow => "accept",
                                Decision::AllowForSession => "acceptForSession",
                                Decision::Deny => "decline",
                            };
                            rpc.send(&json!({ "id": rpc_id, "result": { "decision": d } })).await?;
                        }
                    }
                    Command::Shutdown => break,
                }
            }
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                handle_incoming(&v, &mut rpc, &events, &mut turn_id, &mut pending, hand_holding).await?;
            }
        }
    }
    let _ = child.start_kill();
    Ok(())
}

async fn handle_incoming(
    v: &Value,
    rpc: &mut Rpc,
    events: &async_channel::Sender<AgentEvent>,
    turn_id: &mut Option<String>,
    pending: &mut HashMap<String, Value>,
    hand_holding: HandHolding,
) -> Result<()> {
    let Some(method) = v["method"].as_str() else {
        // A response to one of our requests (turn/start etc.): surface errors only.
        if let Some(err) = v.get("error") {
            events.send(AgentEvent::Error(err["message"].as_str().unwrap_or("Codex error").into())).await?;
        }
        return Ok(());
    };
    let p = &v["params"];

    // Server → client requests (have an id).
    if let Some(rpc_id) = v.get("id").cloned() {
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" | "item/permissions/requestApproval" => {
                let is_edit = method == "item/fileChange/requestApproval";
                if is_edit && hand_holding != HandHolding::Supervised {
                    rpc.send(&json!({ "id": rpc_id, "result": { "decision": "accept" } })).await?;
                    return Ok(());
                }
                let request_id = format!("codex-{}", rpc_id);
                let (title, detail) = if is_edit {
                    ("Edit files".to_string(), p["reason"].as_str().unwrap_or("Apply the proposed changes").to_string())
                } else if method == "item/permissions/requestApproval" {
                    ("Grant permissions".to_string(), p["reason"].as_str().unwrap_or_default().to_string())
                } else {
                    ("Run command".to_string(), p["command"].as_str().unwrap_or_default().to_string())
                };
                pending.insert(request_id.clone(), rpc_id);
                events.send(AgentEvent::PermissionRequest { request_id, title, detail }).await?;
            }
            _ => {
                // Unsupported request types (user-input questions, MCP elicitation): decline politely.
                rpc.send(&json!({ "id": rpc_id, "error": { "code": -32601, "message": "not supported by Trek yet" } })).await?;
            }
        }
        return Ok(());
    }

    let ev = match method {
        "turn/started" => {
            *turn_id = p["turn"]["id"].as_str().map(String::from);
            None
        }
        "item/agentMessage/delta" => Some(AgentEvent::TextDelta(p["delta"].as_str().unwrap_or_default().into())),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            Some(AgentEvent::ReasoningDelta(p["delta"].as_str().unwrap_or_default().into()))
        }
        "item/started" => {
            let item = &p["item"];
            let id = item["id"].as_str().unwrap_or_default().to_string();
            match item["type"].as_str() {
                Some("commandExecution") => Some(AgentEvent::ToolStarted {
                    id,
                    title: "Run command".into(),
                    detail: item["command"].as_str().unwrap_or_default().into(),
                }),
                Some("fileChange") => {
                    let paths: Vec<&str> = item["changes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    Some(AgentEvent::ToolStarted { id, title: "Edit".into(), detail: paths.join(", ") })
                }
                Some("mcpToolCall") | Some("dynamicToolCall") => Some(AgentEvent::ToolStarted {
                    id,
                    title: item["tool"].as_str().unwrap_or("Tool").into(),
                    detail: String::new(),
                }),
                Some("webSearch") => Some(AgentEvent::ToolStarted {
                    id,
                    title: "Search the web".into(),
                    detail: item["query"].as_str().unwrap_or_default().into(),
                }),
                _ => None,
            }
        }
        "item/completed" => {
            let item = &p["item"];
            let id = item["id"].as_str().unwrap_or_default().to_string();
            match item["type"].as_str() {
                Some("agentMessage") => Some(AgentEvent::TextDone(item["text"].as_str().unwrap_or_default().into())),
                Some("commandExecution") => Some(AgentEvent::ToolFinished {
                    id,
                    output: clip(item["aggregatedOutput"].as_str().unwrap_or_default(), 8000),
                    ok: item["exitCode"].as_i64().unwrap_or(0) == 0,
                }),
                Some("fileChange") | Some("mcpToolCall") | Some("dynamicToolCall") | Some("webSearch") => {
                    Some(AgentEvent::ToolFinished { id, output: String::new(), ok: item["status"] != "failed" })
                }
                _ => None,
            }
        }
        "turn/diff/updated" => {
            let (additions, deletions) = diff_stat(p["diff"].as_str().unwrap_or_default());
            Some(AgentEvent::DiffStat { additions, deletions })
        }
        "turn/completed" => {
            *turn_id = None;
            let turn = &p["turn"];
            let error = match turn["status"].as_str() {
                Some("failed") => Some(turn["error"]["message"].as_str().unwrap_or("The turn failed.").to_string()),
                Some("interrupted") => Some("Interrupted".to_string()),
                _ => None,
            };
            Some(AgentEvent::TurnComplete { cost_usd: None, error })
        }
        "error" if p["willRetry"] != true => {
            Some(AgentEvent::Error(p["error"]["message"].as_str().unwrap_or("Codex error").into()))
        }
        _ => None,
    };
    if let Some(ev) = ev {
        events.send(ev).await?;
    }
    Ok(())
}
