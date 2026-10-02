//! Codex via `codex app-server` (JSON-RPC over stdio, newline-delimited, no `jsonrpc` field).
//! Uses the user's own Codex login (ChatGPT plan or API key).

use crate::{AgentEvent, Command, Decision, SessionConfig, clip, diff_stat, mcp_servers_json};
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use trek_core::catalog::ModelInfo;
use trek_core::{Effort, HandHolding, detect};

pub(crate) struct Rpc {
    pub(crate) stdin: ChildStdin,
    pub(crate) next_id: i64,
}

pub(crate) type RpcLines = Lines<BufReader<ChildStdout>>;

impl Rpc {
    pub(crate) async fn send(&mut self, v: &Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "id": id, "method": method, "params": params })).await?;
        Ok(id)
    }
}

/// Read until the response for `id` arrives, forwarding anything else.
pub(crate) async fn await_response(
    lines: &mut RpcLines,
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

/// Spawn `codex app-server` in `cwd` and complete the initialize handshake. Messages that
/// arrive before the handshake completes are left in `backlog`.
pub(crate) async fn start_app_server(cwd: &Path, backlog: &mut Vec<Value>) -> Result<(Child, Rpc, RpcLines)> {
    let bin = detect::which("codex").context("Codex isn't installed (npm i -g @openai/codex)")?;
    let mut child = tokio::process::Command::new(bin)
        .arg("app-server")
        .current_dir(cwd)
        .env("PATH", detect::login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start codex app-server")?;
    let mut rpc = Rpc { stdin: child.stdin.take().unwrap(), next_id: 0 };
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let id = rpc
        .request("initialize", json!({ "clientInfo": { "name": "trek", "title": "Trek", "version": trek_core::VERSION } }))
        .await?;
    await_response(&mut lines, id, backlog).await?;
    rpc.send(&json!({ "method": "initialized" })).await?;
    Ok((child, rpc, lines))
}

/// `turn/start` input: local images first, then the text.
fn user_input(text: &str, images: &[PathBuf]) -> Value {
    let mut input: Vec<Value> =
        images.iter().map(|p| json!({ "type": "localImage", "path": p.display().to_string() })).collect();
    input.push(json!({ "type": "text", "text": text }));
    Value::Array(input)
}

/// `Context` from a `thread/tokenUsage/updated` notification. Matches Codex's own estimate of
/// what's in the window: the last request's input (cached included) plus its visible output;
/// reasoning tokens are dropped from context between turns.
fn context_event(p: &Value) -> Option<AgentEvent> {
    let usage = &p["tokenUsage"];
    let window = usage["modelContextWindow"].as_u64()?;
    let last = &usage["last"];
    let total = last["totalTokens"].as_u64().unwrap_or_else(|| {
        last["inputTokens"].as_u64().unwrap_or(0) + last["outputTokens"].as_u64().unwrap_or(0)
    });
    let used = total.saturating_sub(last["reasoningOutputTokens"].as_u64().unwrap_or(0));
    Some(AgentEvent::Context { used, window })
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
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines) = start_app_server(&config.cwd, &mut backlog).await?;

    let mut hand_holding = config.hand_holding;
    let (sandbox, approval, reviewer) = hand_holding.codex_policy();
    let cwd = config.cwd.display().to_string();
    let mut params = json!({
        "cwd": cwd, "sandbox": sandbox, "approvalPolicy": approval, "approvalsReviewer": reviewer,
    });
    if let Some(m) = &config.model {
        params["model"] = json!(m);
    }
    if !config.mcp_servers.is_empty() {
        // Config overrides merge with the user's own `mcp_servers` (verified against 0.160).
        params["config"] = json!({ "mcp_servers": mcp_servers_json(&config.mcp_servers) });
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
                    Command::Prompt { text, images } => {
                        let (_, approval, reviewer) = hand_holding.codex_policy();
                        let mut p = json!({
                            "threadId": thread_id,
                            "input": user_input(&text, &images),
                            "approvalPolicy": approval,
                            "approvalsReviewer": reviewer,
                            "sandboxPolicy": sandbox_policy(hand_holding),
                        });
                        if let Some(e) = effort_str(effort) { p["effort"] = json!(e); }
                        p["serviceTier"] = json!(config.fast.clone().unwrap_or_else(|| "default".into()));
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
        "thread/tokenUsage/updated" => context_event(p),
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

/// Live model list from the user's Codex setup (includes custom providers), with efforts.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines) = start_app_server(&trek_core::paths::home(), &mut backlog).await?;
    let out = fetch_models(&mut rpc, &mut lines, &mut backlog).await;
    let _ = child.start_kill();
    out
}

/// `model/list` (all pages) on an initialized app-server.
pub(crate) async fn fetch_models(rpc: &mut Rpc, lines: &mut RpcLines, backlog: &mut Vec<Value>) -> Result<Vec<ModelInfo>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..8 {
        let mut params = json!({});
        if let Some(c) = &cursor {
            params["cursor"] = json!(c);
        }
        let id = rpc.request("model/list", params).await?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), await_response(lines, id, backlog)).await??;
        for m in result["data"].as_array().into_iter().flatten() {
            if m["hidden"] == true {
                continue;
            }
            let efforts: Vec<Effort> = m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["reasoningEffort"].as_str().and_then(|s| if s == "none" { Some(Effort::Off) } else { Effort::parse(s) }))
                .collect();
            let fast = m["serviceTiers"].as_array().into_iter().flatten().filter_map(|t| t["id"].as_str()).find(|t| *t != "default").map(String::from);
            let id = m["id"].as_str().or(m["model"].as_str()).unwrap_or_default().to_string();
            out.push(ModelInfo {
                name: m["displayName"].as_str().map(String::from).unwrap_or_else(|| id.clone()),
                id,
                efforts,
                tier: out.len().min(255) as u8,
                fast,
            });
        }
        cursor = result["nextCursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_becomes_context() {
        let p = json!({"threadId":"t","turnId":"u","tokenUsage":{
            "total":{"inputTokens":90000,"cachedInputTokens":5000,"outputTokens":900,"reasoningOutputTokens":400,"totalTokens":90900},
            "last":{"inputTokens":84099,"cachedInputTokens":3328,"outputTokens":105,"reasoningOutputTokens":100,"totalTokens":84204},
            "modelContextWindow":258400}});
        assert_eq!(context_event(&p), Some(AgentEvent::Context { used: 84104, window: 258400 }));
        let no_window = json!({"tokenUsage":{"last":{"totalTokens":1},"total":{},"modelContextWindow":null}});
        assert_eq!(context_event(&no_window), None);
    }

    #[test]
    fn user_input_lists_local_images_then_text() {
        assert_eq!(
            user_input("hi", &[PathBuf::from("/tmp/a.png")]),
            json!([{"type":"localImage","path":"/tmp/a.png"},{"type":"text","text":"hi"}])
        );
    }
}
