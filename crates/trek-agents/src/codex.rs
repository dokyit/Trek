//! Codex via `codex app-server` (JSON-RPC over stdio, newline-delimited, no `jsonrpc` field).
//! Uses the user's own Codex login (ChatGPT plan or API key).

use crate::{AgentEvent, BatchHandOff, Billing, Command, Decision, GroupChild, Prompt, Question, SessionConfig, StderrTail, Step, clip, diff_stat, mcp_servers_json, plan_row, plan_title};
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use trek_core::catalog::ModelInfo;
use trek_core::{Effort, HandHolding, TokenUsage, UsageCost, detect};

pub(crate) struct Rpc {
    pub(crate) stdin: ChildStdin,
    pub(crate) next_id: i64,
}

pub(crate) type RpcLines = crate::ProtocolLines<BufReader<ChildStdout>>;

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
pub(crate) async fn await_response<R: AsyncBufRead + Unpin>(lines: &mut crate::ProtocolLines<R>, id: i64, backlog: &mut Vec<Value>) -> Result<Value> {
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

const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
struct ResponseTimeout {
    method: String,
    seconds: u64,
}

impl std::fmt::Display for ResponseTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codex app-server didn't answer {} in {}s", self.method, self.seconds)
    }
}

impl std::error::Error for ResponseTimeout {}

async fn response_with_timeout<R: AsyncBufRead + Unpin>(
    lines: &mut crate::ProtocolLines<R>,
    id: i64,
    backlog: &mut Vec<Value>,
    method: &str,
    timeout: std::time::Duration,
) -> Result<Value> {
    match tokio::time::timeout(timeout, await_response(lines, id, backlog)).await {
        Ok(response) => response,
        Err(_) => Err(ResponseTimeout { method: method.to_string(), seconds: timeout.as_secs() }.into()),
    }
}

async fn startup_response<R: AsyncBufRead + Unpin>(lines: &mut crate::ProtocolLines<R>, id: i64, backlog: &mut Vec<Value>, method: &str) -> Result<Value> {
    response_with_timeout(lines, id, backlog, method, RESPONSE_TIMEOUT).await
}

/// Notifications a session never reads. Opting out keeps streamed output Trek doesn't show off
/// the pipe. Commands' output it reads: a command can outlive its turn in a background terminal,
/// and its latest lines show while it runs.
const UNUSED_NOTIFICATIONS: &[&str] = &[
    "item/fileChange/outputDelta",
    "command/exec/outputDelta",
    "process/outputDelta",
    "hook/started",
    "hook/completed",
    "mcpServer/startupStatus/updated",
    "thread/status/changed",
];

/// End an app-server: its stdin first, the cue to exit and on Windows the only gentle one (see
/// `GroupChild::terminate`), then the group.
async fn stop(mut child: GroupChild, rpc: Rpc) {
    drop(rpc);
    child.terminate().await;
}

/// Spawn `codex app-server` in `cwd` and complete the initialize handshake. Messages that
/// arrive before the handshake completes are left in `backlog`. The experimental API is on
/// for Plan mode (`collaborationMode`).
pub(crate) async fn start_app_server(
    cwd: &Path,
    opt_out: &[&str],
    backlog: &mut Vec<Value>,
) -> Result<(GroupChild, Rpc, RpcLines, StderrTail)> {
    let bin = detect::which("codex").context("Codex isn't installed (npm i -g @openai/codex)")?;
    let mut command = tokio::process::Command::new(bin);
    command
        .arg("app-server")
        .current_dir(cwd)
        .env("PATH", detect::login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::spawn_group(&mut command).context("failed to start codex app-server")?;
    let stderr = StderrTail::capture(child.stderr.take().unwrap(), "codex");
    let mut rpc = Rpc { stdin: child.stdin.take().unwrap(), next_id: 0 };
    let mut lines = crate::ProtocolLines::new(BufReader::new(child.stdout.take().unwrap()));
    let id = rpc
        .request(
            "initialize",
            json!({
                "clientInfo": { "name": "trek", "title": "Trek", "version": trek_core::VERSION },
                "capabilities": { "experimentalApi": true, "optOutNotificationMethods": opt_out },
            }),
        )
        .await?;
    if let Err(e) = startup_response(&mut lines, id, backlog, "initialize").await {
        stop(child, rpc).await;
        return Err(if e.to_string().contains("exited") { stderr.exited("Codex") } else { e });
    }
    rpc.send(&json!({ "method": "initialized" })).await?;
    Ok((child, rpc, lines, stderr))
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

/// A `TurnError` for a usage limit (or a rate limit Codex gave up retrying).
fn is_limit_error(error: &Value) -> bool {
    matches!(error["codexErrorInfo"].as_str(), Some("usageLimitExceeded" | "rateLimitExceeded"))
}

/// Copy what `update` reports over `into`, leaving what it leaves out (recursively for objects).
fn merge(into: &mut Value, update: &Value) {
    match (into.as_object_mut(), update.as_object()) {
        (Some(base), Some(new)) => {
            for (k, v) in new {
                if v.is_null() {
                    continue;
                }
                match base.get_mut(k) {
                    Some(old) if old.is_object() && v.is_object() => merge(old, v),
                    _ => {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        _ if !update.is_null() => *into = update.clone(),
        _ => {}
    }
}

/// The id of the account's main limit (`limitId`); snapshots without one are its.
const MAIN_LIMIT: &str = "codex";

/// When the usage limit a turn failed at resets, from the rate-limit snapshots by limit id: the
/// one for the session's `model` if that's used up (a quota of its own), else the account's main
/// one, else any used up. And which limit that is.
fn limit_reset(snapshots: &BTreeMap<String, Value>, model: Option<&str>) -> (Option<i64>, Option<crate::LimitScope>) {
    let own = snapshots.iter().filter(|(id, s)| *id != MAIN_LIMIT && model.is_some_and(|m| s["normalModelSlug"].as_str() == Some(m)));
    let main = snapshots.get_key_value(MAIN_LIMIT);
    let others = snapshots.iter().filter(|(id, _)| *id != MAIN_LIMIT);
    own.chain(main).chain(others).find_map(|(id, s)| snapshot_reset(s, (id != MAIN_LIMIT).then_some(id))).map_or((None, None), |(at, scope)| (Some(at), Some(scope)))
}

/// When one rate-limit snapshot's used-up windows reset (the latest of them), and which limit that
/// is: a limit of its own (`id`, not the account's main one) by its name, else by its window.
fn snapshot_reset(snapshot: &Value, id: Option<&str>) -> Option<(i64, crate::LimitScope)> {
    let (at, mins) = ["primary", "secondary"]
        .iter()
        .map(|k| &snapshot[*k])
        .filter(|w| w["usedPercent"].as_f64().is_some_and(|p| p >= 100.))
        .filter_map(|w| Some((w["resetsAt"].as_i64()? * 1000, w["windowDurationMins"].as_i64())))
        .max_by_key(|(at, _)| *at)?;
    let scope = match (id, mins) {
        (Some(id), _) => crate::LimitScope::Model(snapshot["limitName"].as_str().unwrap_or(id).to_string()),
        (None, Some(300)) => crate::LimitScope::Session,
        (None, Some(10080)) => crate::LimitScope::Weekly,
        _ => crate::LimitScope::Other,
    };
    Some((at, scope))
}

/// How full the windows of one rate-limit snapshot are, for a session on `model`: the account's
/// main limit counts for every model, a limit of its own (`id`) only for the model it names.
/// Windows still without a percentage are left out.
fn windows_used(snapshot: &Value, id: &str, model: Option<&str>) -> Vec<AgentEvent> {
    let own = id != MAIN_LIMIT;
    if own && !model.is_some_and(|m| snapshot["normalModelSlug"].as_str() == Some(m)) {
        return vec![];
    }
    ["primary", "secondary"]
        .iter()
        .map(|k| &snapshot[*k])
        .filter_map(|w| {
            let percent = w["usedPercent"].as_f64()? as f32;
            let scope = match (own, w["windowDurationMins"].as_i64()) {
                (true, _) => crate::LimitScope::Model(snapshot["limitName"].as_str().unwrap_or(id).to_string()),
                (false, Some(300)) => crate::LimitScope::Session,
                (false, Some(10080)) => crate::LimitScope::Weekly,
                _ => crate::LimitScope::Other,
            };
            Some(AgentEvent::LimitUsed { scope, percent, resets_at: w["resetsAt"].as_i64().map(|s| s * 1000) })
        })
        .collect()
}

/// Codex passes provider errors through as raw JSON; pull out the sentence a person can read.
fn readable_error(msg: &str) -> String {
    serde_json::from_str::<Value>(msg)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or(v["message"].as_str()).map(String::from))
        .unwrap_or_else(|| msg.to_string())
}

/// `/bin/zsh -lc 'touch a.txt'` → `touch a.txt`: the command as the model wrote it. On Windows
/// Codex wraps in `powershell.exe -NoProfile -Command <script>` (or `pwsh`, or `cmd.exe /c`).
fn unwrap_shell(cmd: &str) -> String {
    if let Some(script) = unwrap_windows_shell(cmd) {
        return script;
    }
    for flag in [" -lc ", " -c "] {
        let Some(i) = cmd.find(flag) else { continue };
        if !matches!(cmd[..i].rsplit('/').next(), Some("sh" | "bash" | "zsh")) {
            continue;
        }
        let rest = cmd[i + flag.len()..].trim();
        if rest.len() >= 2 && rest.starts_with('\'') && rest.ends_with('\'') {
            return rest[1..rest.len() - 1].replace("'\\''", "'").replace("'\"'\"'", "'");
        }
        if rest.len() >= 2 && rest.starts_with('"') && rest.ends_with('"') {
            return rest[1..rest.len() - 1].replace("\\\"", "\"");
        }
        return rest.to_string();
    }
    cmd.to_string()
}

/// What Codex puts ahead of a PowerShell script so its output comes back as UTF-8: not part of the
/// command the model wrote.
const POWERSHELL_UTF8_PRELUDES: [&str; 2] = ["[Console]::OutputEncoding=[System.Text.Encoding]::UTF8", "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}"];

/// `powershell.exe -NoProfile -Command 'Get-ChildItem'` → `Get-ChildItem`, and likewise for `pwsh`
/// and `cmd.exe /c`: the program may be a path (quoted when it has spaces), in any case, with or
/// without `.exe`. `None` when `cmd` isn't one of those wrappers.
fn unwrap_windows_shell(cmd: &str) -> Option<String> {
    let (program, mut rest) = next_word(cmd)?;
    let name = program.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    let powershell = matches!(name, "powershell" | "pwsh");
    if !powershell && name != "cmd" {
        return None;
    }
    let script = loop {
        let (word, after) = next_word(rest)?;
        match word.to_ascii_lowercase().as_str() {
            "-command" | "-c" if powershell => break after,
            "/c" if !powershell => break after,
            "-noprofile" | "-nologo" | "-noninteractive" | "-executionpolicybypass" | "-noexit" if powershell => rest = after,
            "-executionpolicy" | "-ep" if powershell => rest = next_word(after)?.1,
            "/d" | "/s" | "/q" if !powershell => rest = after,
            _ => return None,
        }
    };
    let script = script.trim();
    if script.is_empty() {
        return None;
    }
    let script = if script.len() >= 2 && script.starts_with('\'') && script.ends_with('\'') {
        script[1..script.len() - 1].replace("'\"'\"'", "'").replace("''", "'")
    } else if script.len() >= 2 && script.starts_with('"') && script.ends_with('"') {
        script[1..script.len() - 1].replace("\\\"", "\"").replace("`\"", "\"").replace("\"\"", "\"")
    } else {
        script.to_string()
    };
    let mut script = script.as_str();
    if powershell {
        for prelude in POWERSHELL_UTF8_PRELUDES {
            if let Some(after) = script.strip_prefix(prelude) {
                script = after.trim_start_matches([';', ' ', '\r', '\n']);
            }
        }
    }
    Some(script.to_string())
}

/// The first word of `s` (a quoted one without its quotes) and what follows it.
fn next_word(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if let Some(inner) = s.strip_prefix(['\'', '"']) {
        let end = inner.find(&s[..1])?;
        return Some((&inner[..end], &inner[end + 1..]));
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (end > 0).then(|| (&s[..end], &s[end..]))
}

fn command_text(v: &Value) -> String {
    unwrap_shell(v["command"].as_str().unwrap_or_default())
}

fn change_paths(item: &Value) -> Vec<String> {
    item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).map(String::from).collect()
}

/// Lines a `fileChange` added and removed: a new file's content counts as added, a deleted
/// file's as removed, an update's unified diff line by line.
fn change_lines(item: &Value) -> (u32, u32) {
    item["changes"].as_array().into_iter().flatten().fold((0, 0), |(a, r), c| {
        let diff = c["diff"].as_str().unwrap_or_default();
        let n = diff.lines().count() as u32;
        match c["kind"]["type"].as_str() {
            Some("add") => (a + n, r),
            Some("delete") => (a, r + n),
            _ => {
                let (a2, r2) = diff_stat(diff);
                (a + a2 as u32, r + r2 as u32)
            }
        }
    })
}

/// A message's text, to name it ("an image" when it's only images).
fn input_text(input: &Value) -> String {
    let text: Vec<&str> = input.as_array().into_iter().flatten().filter_map(|i| i["text"].as_str()).collect();
    let text = text.join(" ");
    if text.trim().is_empty() { "an image".into() } else { format!("“{}”", clip(text.trim(), 80)) }
}

/// A sub-agent's name from its path: `/root/pong_agent` → "pong agent".
fn agent_name(path: &str) -> String {
    path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("Subagent").replace('_', " ")
}

/// What a sub-agent is doing, from one of its own items.
/// A tool item as its row is titled: (title, detail). `None` for items that aren't tool calls.
fn tool_row(item: &Value) -> Option<(String, String)> {
    Some(match item["type"].as_str()? {
        "commandExecution" => ("Run command".to_string(), command_text(item)),
        "fileChange" => ("Edit".to_string(), change_paths(item).join(", ")),
        "mcpToolCall" | "dynamicToolCall" => (mcp_title(item), String::new()),
        "webSearch" => ("Search the web".to_string(), item["query"].as_str().unwrap_or_default().to_string()),
        "imageView" => ("Read".to_string(), item["path"].as_str().unwrap_or_default().to_string()),
        _ => return None,
    })
}

fn activity(item: &Value) -> Option<String> {
    let s = match item["type"].as_str()? {
        "commandExecution" => format!("Running {}", command_text(item)),
        "fileChange" => format!("Editing {}", change_paths(item).join(", ")),
        "mcpToolCall" | "dynamicToolCall" => item["tool"].as_str()?.to_string(),
        "webSearch" => "Searching the web".to_string(),
        _ => return None,
    };
    Some(clip(&s, 80))
}

/// An MCP tool row's title: the tool's name, or for Trek's own server Claude's full name for it
/// (`mcp__trek-orchestrate__delegate_task`), so Trek tells its tools from another server's.
fn mcp_title(item: &Value) -> String {
    let tool = item["tool"].as_str().unwrap_or("Tool");
    match item["server"].as_str() {
        Some(server) if server == trek_core::orchestrate::SERVER => format!("mcp__{server}__{tool}"),
        _ => tool.to_string(),
    }
}

/// `mcp_servers` for Codex's config: the shared shape for stdio servers, `url` and
/// `http_headers` for remote ones, with a tool timeout where a server needs longer than Codex's
/// default minute.
///
/// Along with a notice for each server that can't be handed to Codex (left out).
fn codex_mcp_servers(servers: &[crate::McpServer]) -> (Value, Vec<String>) {
    codex_mcp_servers_with(servers, mcp_servers_json(servers, BatchHandOff::ResolvedPath))
}

/// `codex_mcp_servers` on top of the servers' JSON as handed over (see `mcp_servers_json_with`).
fn codex_mcp_servers_with(servers: &[crate::McpServer], (mut out, left_out): (Value, Vec<String>)) -> (Value, Vec<String>) {
    for s in servers {
        let Some(entry) = out.get_mut(&s.name) else { continue };
        if let crate::McpTransport::Http { url, headers } = &s.transport {
            let headers: serde_json::Map<String, Value> = headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            *entry = json!({ "url": url });
            if !headers.is_empty() {
                entry["http_headers"] = Value::Object(headers);
            }
        }
        if let Some(secs) = s.tool_timeout_secs {
            entry["tool_timeout_sec"] = json!(secs);
        }
    }
    (out, left_out)
}

fn tool_output(item: &Value) -> String {
    let text: Vec<&str> = item["result"]["content"]
        .as_array()
        .or(item["contentItems"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect();
    let out = if text.is_empty() { item["error"]["message"].as_str().unwrap_or_default().to_string() } else { text.join("\n") };
    clip(&out, 8000)
}

/// One line per requested grant, then the reason.
fn permissions_summary(p: &Value) -> String {
    let perms = &p["permissions"];
    let fs = &perms["fileSystem"];
    let mut lines = vec![];
    if perms["network"]["enabled"] == true {
        lines.push("Network access".to_string());
    }
    for (key, label) in [("write", "Write"), ("read", "Read")] {
        let paths: Vec<&str> = fs[key].as_array().into_iter().flatten().filter_map(|p| p.as_str()).collect();
        if !paths.is_empty() {
            lines.push(format!("{label}: {}", paths.join(", ")));
        }
    }
    for e in fs["entries"].as_array().into_iter().flatten() {
        let path = &e["path"];
        let target = path["path"].as_str().or(path["pattern"].as_str()).map(String::from).unwrap_or_else(|| path["value"]["kind"].as_str().unwrap_or("files").replace('_', " "));
        let access = crate::status::capitalize(e["access"].as_str().unwrap_or("read"));
        lines.push(format!("{access}: {target}"));
    }
    if let Some(r) = p["reason"].as_str().filter(|r| !r.is_empty()) {
        lines.push(r.to_string());
    }
    lines.join("\n")
}

fn approval_decision(d: Decision) -> &'static str {
    match d {
        Decision::Allow => "accept",
        Decision::AllowForSession => "acceptForSession",
        Decision::Deny => "decline",
    }
}

/// `item/permissions/requestApproval` answer: grant what was asked (this turn or the whole
/// session), or nothing.
fn permissions_grant(requested: &Value, d: Decision) -> Value {
    match d {
        Decision::Allow => json!({ "permissions": requested, "scope": "turn" }),
        Decision::AllowForSession => json!({ "permissions": requested, "scope": "session" }),
        Decision::Deny => json!({ "permissions": {} }),
    }
}

/// `{id: {answers: [..]}}`. Trek answers by question text; answers that name no question are
/// taken in order. Questions left unanswered are left out.
fn question_answers(ids: &[(String, String)], answers: &[(String, String)]) -> Value {
    let mut out = serde_json::Map::new();
    for (i, (question, id)) in ids.iter().enumerate() {
        let by_order = || answers.get(i).filter(|(q, _)| !ids.iter().any(|(known, _)| known == q));
        if let Some((_, a)) = answers.iter().find(|(q, _)| q == question).or_else(by_order).filter(|(_, a)| !a.is_empty()) {
            out.insert(id.clone(), json!({ "answers": [a] }));
        }
    }
    Value::Object(out)
}

/// Request ids of plans offered after their turn: `codex-plan-<turn id>`.
const PLAN_REQUEST: &str = "codex-plan-";

/// Trek's id for a server request.
fn request_key(rpc_id: &Value) -> String {
    format!("codex-{}", rpc_id.as_str().map(String::from).unwrap_or_else(|| rpc_id.to_string()))
}

/// What one of our requests was, so its response can be acted on.
enum Call {
    Start,
    /// A steer into turn `turn`; `retried` once it has gone out a second time.
    Steer { input: Value, turn: String, retried: bool },
    /// `account/read`, sent before the session starts: how the login is billed.
    Account,
    /// `thread/backgroundTerminals/terminate` for the command run by item `0`.
    Terminate(String),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
enum Turn {
    Idle,
    /// `turn/start` is out; its id isn't known yet.
    Starting,
    Running(String),
}

/// Something waiting on the user.
enum Pending {
    Approval(Value),
    Permissions { rpc_id: Value, requested: Value },
    /// `(question, id)` pairs, to key the answers.
    Questions { rpc_id: Value, ids: Vec<(String, String)> },
    /// The plan a Plan-mode turn proposed; approving it starts the work.
    Plan,
}

struct SubAgent {
    /// The tool row (and task id) it shows on. Each run of the same agent gets its own.
    row: String,
    runs: u32,
    tool_uses: u64,
    /// Its latest message: the row's output once it finishes.
    last: String,
    done: bool,
    /// The model it was spawned on, when not the session's.
    model: Option<String>,
}

/// What handling one message produced: events for the UI, messages for Codex.
#[derive(Default, Debug)]
struct Out {
    events: Vec<AgentEvent>,
    send: Vec<Value>,
}

/// One Codex thread: app-server messages become events, commands become requests.
struct Session {
    thread_id: String,
    model: Option<String>,
    effort: Effort,
    hand_holding: HandHolding,
    fast: Option<String>,
    /// Trek's plan mode: turns run in Codex's Plan collaboration mode.
    plan: bool,
    /// The collaboration mode Codex last reported, so leaving Plan mode is said explicitly.
    mode: Option<String>,
    turn: Turn,
    /// A turn has been started in this session.
    started: bool,
    /// Messages sent while `turn/start` was in flight; they steer the turn once its id is known.
    held: Vec<Value>,
    /// Steers Codex refused because the turn was ending: they start the next turn.
    after_turn: Vec<Value>,
    /// Interrupt the starting turn as soon as it has an id.
    interrupt: bool,
    calls: HashMap<i64, Call>,
    next_id: i64,
    /// Questions and approvals by Trek request id.
    pending: HashMap<String, Pending>,
    /// The running Plan-mode turn's proposed plan.
    proposed: Option<String>,
    /// The plan as it streams: `(item id, text so far, row shown)`. Its row appears once the
    /// first line (the title) is in.
    drafting: Option<(String, String, bool)>,
    /// A fatal error reported before its `turn/completed`.
    turn_error: Option<String>,
    /// `fileChange` item id → its paths, for the approval card.
    edits: HashMap<String, Vec<String>>,
    /// The thread's folder: edits inside it go ahead at Auto-accept edits.
    cwd: PathBuf,
    /// A recap of the conversation for the first message: the thread couldn't be taken back to
    /// where the conversation now ends, so this is a new one.
    recap: Option<String>,
    /// Sub-agents by their thread id.
    agents: HashMap<String, SubAgent>,
    plan_updates: u32,
    /// The account's rate limits as Codex last reported them (`account/rateLimits/updated`), by
    /// limit id: when a usage limit resets.
    rate_limits: BTreeMap<String, Value>,
    /// The running turn failed at a usage limit (`usageLimitExceeded`).
    limited: bool,
    /// Each Codex thread's token total as last reported (this one's and its sub-agents').
    token_totals: HashMap<String, TokenUsage>,
    /// Tokens the running turn has used so far, by model, and what they cost (priced per
    /// request), sent as `Usage` when it ends.
    turn_tokens: Vec<(Option<String>, TokenUsage, Option<UsageCost>)>,
    /// Commands running in this thread, by item id: the command and the end of its output so far.
    commands: HashMap<String, (String, String)>,
    /// Commands that outlived the turn that started them (Codex's background terminals), in the
    /// order they were left running.
    background: Vec<String>,
    /// Codex's own ids for the terminals commands run in, by item id: what stopping one takes.
    processes: HashMap<String, String>,
    /// Windows (a field so both platforms' behaviour is tested on either).
    windows: bool,
    /// The user has been told Codex's Windows sandbox isn't set up.
    sandbox_told: bool,
}

/// A command Codex's Windows sandbox couldn't start: its sandbox hasn't had the one-time setup,
/// and every command under Supervised or Auto fails before it starts, with an output that says
/// the sandbox's "setup refresh had errors" (Codex 0.162.0).
fn sandbox_not_set_up(item: &Value, windows: bool) -> bool {
    windows
        && item["type"] == "commandExecution"
        && item["status"] == "failed"
        && item["aggregatedOutput"].as_str().is_some_and(|o| o.contains("setup refresh had errors"))
}

/// What the user is told when `sandbox_not_set_up`.
const SANDBOX_SETUP_NOTICE: &str = "Codex's Windows sandbox needs its one-time setup, so its commands fail before they start. Run codex once in a terminal and accept the sandbox setup it asks for, or set this thread's access to Full access.";

impl Session {
    fn new(thread_id: String, config: &SessionConfig, opened: &Value, next_id: i64) -> Self {
        Session {
            thread_id,
            model: opened["model"].as_str().map(String::from).or(config.model.clone()),
            effort: config.effort,
            hand_holding: config.hand_holding,
            fast: config.fast.clone(),
            plan: config.plan,
            mode: opened["collaborationMode"]["mode"].as_str().map(String::from),
            turn: Turn::Idle,
            started: false,
            held: vec![],
            after_turn: vec![],
            interrupt: false,
            calls: HashMap::new(),
            next_id,
            pending: HashMap::new(),
            proposed: None,
            drafting: None,
            turn_error: None,
            edits: HashMap::new(),
            cwd: config.cwd.clone(),
            recap: None,
            agents: HashMap::new(),
            plan_updates: 0,
            rate_limits: BTreeMap::new(),
            limited: false,
            token_totals: HashMap::new(),
            turn_tokens: vec![],
            commands: HashMap::new(),
            background: vec![],
            processes: HashMap::new(),
            windows: cfg!(windows),
            sandbox_told: false,
        }
    }

    /// The commands left running in the background, as `AgentEvent::Background` lists them.
    /// Those Codex said which terminal they run in can be stopped (`thread/backgroundTerminals/terminate`).
    fn background_event(&self) -> AgentEvent {
        let tasks = self
            .background
            .iter()
            .filter_map(|id| {
                let (command, _) = self.commands.get(id)?;
                let stoppable = self.processes.contains_key(id);
                Some(crate::BackgroundTask { id: id.clone(), kind: crate::BackgroundKind::Shell, title: command.clone(), call: Some(id.clone()), readable: true, stoppable })
            })
            .collect();
        AgentEvent::Background(tasks)
    }

    /// Command `id` is no longer running: off the list of those in the background.
    fn command_gone(&mut self, id: &str, out: &mut Out) {
        self.commands.remove(id);
        self.processes.remove(id);
        if let Some(ix) = self.background.iter().position(|b| b == id) {
            self.background.remove(ix);
            out.events.push(self.background_event());
        }
    }

    /// Command `id`, stopped on request, has ended: its row gets the output it had.
    fn command_ended(&mut self, id: &str, out: &mut Out) {
        let Some((_, output)) = self.commands.get(id).cloned() else { return };
        self.command_gone(id, out);
        out.events.push(AgentEvent::ToolFinished { id: id.to_string(), output, ok: false });
    }

    fn busy(&self) -> bool {
        self.turn != Turn::Idle
    }

    fn request(&mut self, method: &str, params: Value, call: Call) -> Value {
        self.next_id += 1;
        self.calls.insert(self.next_id, call);
        json!({ "id": self.next_id, "method": method, "params": params })
    }

    fn turn_start(&mut self, input: Value) -> Value {
        let (_, approval, reviewer) = self.hand_holding.codex_policy();
        let mut p = json!({
            "threadId": self.thread_id,
            "input": input,
            "approvalPolicy": approval,
            "approvalsReviewer": reviewer,
            "sandboxPolicy": sandbox_policy(self.hand_holding),
            "serviceTier": self.fast.clone().unwrap_or_else(|| "default".into()),
        });
        if let Some(e) = effort_str(self.effort) {
            p["effort"] = json!(e);
        }
        if let Some(m) = &self.model {
            p["model"] = json!(m);
        }
        // Plan mode is a collaboration mode, and it sticks: leaving it is said explicitly too.
        if self.plan || self.mode.as_deref() == Some("plan") {
            let mode = if self.plan { "plan" } else { "default" };
            p["collaborationMode"] = json!({
                "mode": mode,
                "settings": { "model": self.model.clone().unwrap_or_default(), "reasoning_effort": effort_str(self.effort), "developer_instructions": null },
            });
            self.mode = Some(mode.into());
        }
        self.turn = Turn::Starting;
        self.started = true;
        self.turn_error = None;
        self.proposed = None;
        self.drafting = None;
        // A plan offered after the last turn is answered by this message instead.
        self.pending.retain(|_, p| !matches!(p, Pending::Plan));
        self.request("turn/start", p, Call::Start)
    }

    /// A user message: starts a turn, or steers the running one.
    fn prompt(&mut self, input: Value, retried: bool, out: &mut Out) {
        match self.turn.clone() {
            Turn::Idle => {
                let m = self.turn_start(input);
                out.send.push(m);
            }
            Turn::Starting => self.held.push(input),
            Turn::Running(id) => {
                let params = json!({ "threadId": self.thread_id, "expectedTurnId": id, "input": input });
                let m = self.request("turn/steer", params, Call::Steer { input, turn: id, retried });
                out.send.push(m);
            }
        }
    }

    /// Codex refused a steer. The message is never dropped: it goes into whatever turn is next.
    fn steer_failed(&mut self, input: Value, turn: String, retried: bool, out: &mut Out) {
        match self.turn.clone() {
            // The turn ended first ("no active turn to steer"): the message starts the next one.
            Turn::Idle => self.prompt(input, true, out),
            Turn::Starting => self.held.push(input),
            // A newer turn is running: steer into that one instead.
            Turn::Running(id) if id != turn && !retried => self.prompt(input, true, out),
            // Codex is ending this turn and its `turn/completed` hasn't arrived yet (or the retry
            // was refused too): the message starts the next turn.
            Turn::Running(_) => self.after_turn.push(input),
        }
    }

    /// The turn has an id: send what was held for it.
    fn running(&mut self, id: String, out: &mut Out) {
        self.turn = Turn::Running(id.clone());
        for input in std::mem::take(&mut self.held) {
            self.prompt(input, false, out);
        }
        if std::mem::take(&mut self.interrupt) {
            let m = self.request("turn/interrupt", json!({ "threadId": self.thread_id, "turnId": id }), Call::Other);
            out.send.push(m);
        }
    }

    fn command(&mut self, cmd: Command) -> Out {
        let mut out = Out::default();
        match cmd {
            Command::Prompt { text, images } => {
                let text = match self.recap.take() {
                    Some(r) => crate::recap_prompt(&r, &text),
                    None => text,
                };
                self.prompt(user_input(&text, &images), false, &mut out)
            }
            Command::Interrupt => match self.turn.clone() {
                Turn::Running(id) => {
                    let m = self.request("turn/interrupt", json!({ "threadId": self.thread_id, "turnId": id }), Call::Other);
                    out.send.push(m);
                }
                Turn::Starting => self.interrupt = true,
                Turn::Idle => {}
            },
            Command::SetHandHolding(h) => self.hand_holding = h,
            Command::SetModel { model, effort } => {
                self.model = Some(model);
                self.effort = effort;
            }
            // Read per turn (`turn/start`), so the next one runs with them.
            Command::SetModes { plan, fast, effort } => {
                self.plan = plan;
                self.fast = fast;
                self.effort = effort;
            }
            Command::Respond { request_id, decision } => self.respond(&request_id, decision, &mut out),
            Command::Answer { request_id, answers } => match self.pending.remove(&request_id) {
                Some(Pending::Questions { rpc_id, ids }) => {
                    out.send.push(json!({ "id": rpc_id, "result": { "answers": question_answers(&ids, &answers) } }));
                }
                Some(other) => {
                    self.pending.insert(request_id, other);
                }
                None => {}
            },
            Command::ReadTask { id } => {
                if let Some((_, output)) = self.commands.get(&id) {
                    out.events.push(AgentEvent::TaskOutput { id, output: output.clone() });
                }
            }
            Command::StopTask { id } => {
                if let Some(process) = self.processes.get(&id).cloned() {
                    let m = self.request("thread/backgroundTerminals/terminate", json!({ "threadId": self.thread_id, "processId": process }), Call::Terminate(id));
                    out.send.push(m);
                }
            }
            Command::Shutdown => {}
        }
        out
    }

    fn respond(&mut self, request_id: &str, decision: Decision, out: &mut Out) {
        let pending = match self.pending.remove(request_id) {
            Some(p) => p,
            // A plan offered by this thread's previous session (it restarted while the plan
            // waited). Nothing has run here since, so it's still the latest plan.
            None if request_id.starts_with(PLAN_REQUEST) && !self.started => Pending::Plan,
            None => return,
        };
        match pending {
            Pending::Approval(rpc_id) => out.send.push(json!({ "id": rpc_id, "result": { "decision": approval_decision(decision) } })),
            Pending::Permissions { rpc_id, requested } => {
                out.send.push(json!({ "id": rpc_id, "result": permissions_grant(&requested, decision) }))
            }
            // Skipped: no answers, and the model carries on without them.
            Pending::Questions { rpc_id, .. } => out.send.push(json!({ "id": rpc_id, "result": { "answers": {} } })),
            // Codex's own "Implement this plan?": leave Plan mode and start the work. Keeping
            // on planning needs nothing; the user's next message goes to Plan mode.
            Pending::Plan if decision != Decision::Deny => {
                self.plan = false;
                // The plan came from a Plan-mode turn, so the thread is in Plan mode whatever
                // a resumed session reported: leave it explicitly.
                self.mode = Some("plan".into());
                self.prompt(user_input("Implement the plan.", &[]), false, out);
            }
            Pending::Plan => {}
        }
    }

    fn incoming(&mut self, v: &Value) -> Out {
        let mut out = Out::default();
        let Some(method) = v["method"].as_str() else {
            self.response(v, &mut out);
            return out;
        };
        let p = &v["params"];
        if let Some(rpc_id) = v.get("id") {
            self.server_request(method, rpc_id.clone(), p, &mut out);
            return out;
        }
        match p["threadId"].as_str() {
            // Requests are Trek's by request id, whichever thread raised them: a sub-agent's
            // approval that Codex settles must go too.
            _ if method == "serverRequest/resolved" => self.notification(method, p, &mut out),
            Some(t) if t != self.thread_id && method == "thread/tokenUsage/updated" => self.count_tokens(t, p, &mut out),
            Some(t) if t != self.thread_id => self.sub_agent(t, method, p, &mut out),
            _ => self.notification(method, p, &mut out),
        }
        out
    }

    fn response(&mut self, v: &Value, out: &mut Out) {
        let Some(call) = v["id"].as_i64().and_then(|id| self.calls.remove(&id)) else { return };
        let error = v.get("error").map(|e| readable_error(e["message"].as_str().unwrap_or("Codex error")));
        match (call, error) {
            (Call::Start, None) => {
                if let Some(id) = v["result"]["turn"]["id"].as_str()
                    && self.turn == Turn::Starting
                {
                    self.running(id.to_string(), out);
                }
            }
            // The turn never started: it ends here, failed (an `Error` alone doesn't end a turn).
            (Call::Start, Some(e)) => {
                self.turn = Turn::Idle;
                self.interrupt = false;
                // Messages sent while the turn was starting were waiting for it: say which didn't go.
                let unsent: Vec<String> = self.held.drain(..).map(|i| input_text(&i)).collect();
                let e = if unsent.is_empty() { e } else { format!("{e}\nNot sent to Codex: {}", unsent.join(", ")) };
                out.events.push(AgentEvent::TurnComplete { error: Some(e) });
            }
            (Call::Steer { input, turn, retried }, Some(e)) => {
                tracing::debug!("codex refused a steer: {e}");
                self.steer_failed(input, turn, retried, out);
            }
            (Call::Account, None) => out.events.extend(account_billing(&v["result"]).map(AgentEvent::Billing)),
            // Stopped: it's over now, whether or not Codex says the command completed after.
            (Call::Terminate(id), None) if v["result"]["terminated"] == true => self.command_ended(&id, out),
            (Call::Terminate(_), None) => out.events.push(AgentEvent::Notice("Codex couldn't stop the background command: it had no terminal running it".into())),
            (Call::Terminate(_), Some(e)) => out.events.push(AgentEvent::Notice(format!("Codex couldn't stop the background command: {e}"))),
            (_, Some(e)) => tracing::debug!("codex request failed: {e}"),
            _ => {}
        }
    }

    fn server_request(&mut self, method: &str, rpc_id: Value, p: &Value, out: &mut Out) {
        let request_id = request_key(&rpc_id);
        let mut prompt = None;
        let (title, detail) = match method {
            "item/commandExecution/requestApproval" => {
                self.pending.insert(request_id.clone(), Pending::Approval(rpc_id));
                match p["networkApprovalContext"]["host"].as_str() {
                    Some(host) => ("Network access".to_string(), host.to_string()),
                    None => ("Run command".to_string(), command_text(p)),
                }
            }
            "item/fileChange/requestApproval" => {
                // Codex only asks for writes its sandbox blocks: outside the project's writable
                // roots, or with more room (`grantRoot`). Above Supervised, edits inside the
                // project go ahead, as with Claude's acceptEdits; the rest are the user's call
                // below Full access.
                let paths = p["itemId"].as_str().and_then(|i| self.edits.get(i)).cloned().unwrap_or_default();
                let in_project = p["grantRoot"].is_null() && !paths.is_empty() && paths.iter().all(|f| crate::acp::within(Path::new(f), &self.cwd));
                let auto = match self.hand_holding {
                    HandHolding::FullAccess => true,
                    HandHolding::Supervised => false,
                    HandHolding::AutoAcceptEdits | HandHolding::Auto => in_project,
                };
                if auto {
                    out.send.push(json!({ "id": rpc_id, "result": { "decision": "accept" } }));
                    return;
                }
                self.pending.insert(request_id.clone(), Pending::Approval(rpc_id));
                let detail = Some(paths.join(", "))
                    .filter(|d| !d.is_empty())
                    .or(p["grantRoot"].as_str().map(String::from))
                    .or(p["reason"].as_str().map(String::from))
                    .unwrap_or_default();
                ("Edit".to_string(), detail)
            }
            "item/permissions/requestApproval" => {
                self.pending.insert(request_id.clone(), Pending::Permissions { rpc_id, requested: p["permissions"].clone() });
                ("Grant permissions".to_string(), permissions_summary(p))
            }
            "item/tool/requestUserInput" => {
                let qs: Vec<&Value> = p["questions"].as_array().into_iter().flatten().collect();
                let ids = qs.iter().map(|q| (q["question"].as_str().unwrap_or_default().to_string(), q["id"].as_str().unwrap_or_default().to_string())).collect();
                let questions: Vec<Question> = qs
                    .iter()
                    .map(|q| Question {
                        question: q["question"].as_str().unwrap_or_default().to_string(),
                        header: q["header"].as_str().unwrap_or_default().to_string(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|o| (o["label"].as_str().unwrap_or_default().to_string(), o["description"].as_str().unwrap_or_default().to_string()))
                            .collect(),
                        multi: false,
                        secret: q["isSecret"] == true,
                    })
                    .collect();
                let detail = questions.first().map(|q| q.question.clone()).unwrap_or_default();
                self.pending.insert(request_id.clone(), Pending::Questions { rpc_id, ids });
                prompt = Some(Prompt::Questions(questions));
                ("Question".to_string(), detail)
            }
            // MCP servers asking for input mid-call: Trek has no form for it, so decline.
            "mcpServer/elicitation/request" => {
                // Trek's own sub-agent server asks nothing itself: what comes in its name is
                // Codex checking a call to it, which needs no approval (the sub-agent's own
                // actions are approved in its thread).
                let result = if p["serverName"] == "trek-orchestrate" { json!({ "action": "accept", "content": {} }) } else { json!({ "action": "decline" }) };
                out.send.push(json!({ "id": rpc_id, "result": result }));
                return;
            }
            "currentTime/read" => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                out.send.push(json!({ "id": rpc_id, "result": { "currentTimeAt": now } }));
                return;
            }
            _ => {
                out.send.push(json!({ "id": rpc_id, "error": { "code": -32601, "message": format!("Trek doesn't handle {method}") } }));
                return;
            }
        };
        out.events.push(AgentEvent::PermissionRequest { request_id, title, detail, prompt });
    }

    fn notification(&mut self, method: &str, p: &Value, out: &mut Out) {
        let ev = match method {
            "turn/started" => {
                if let Some(id) = p["turn"]["id"].as_str() {
                    self.running(id.to_string(), out);
                }
                None
            }
            "turn/completed" => {
                self.turn_completed(&p["turn"], out);
                None
            }
            "item/agentMessage/delta" => Some(AgentEvent::TextDelta(p["delta"].as_str().unwrap_or_default().into())),
            "item/commandExecution/outputDelta" => {
                if let Some((_, output)) = p["itemId"].as_str().and_then(|id| self.commands.get_mut(id)) {
                    crate::keep_tail(output, p["delta"].as_str().unwrap_or_default());
                }
                None
            }
            "item/plan/delta" => {
                self.plan_delta(p["itemId"].as_str().unwrap_or_default(), p["delta"].as_str().unwrap_or_default(), out);
                None
            }
            // Codex settled a request itself (auto-resolved, reviewed, or its turn moved on). Ones
            // Trek answered are already gone.
            "serverRequest/resolved" => {
                let request_id = request_key(&p["requestId"]);
                self.pending.remove(&request_id).map(|_| AgentEvent::PermissionResolved { request_id })
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                Some(AgentEvent::ReasoningDelta(p["delta"].as_str().unwrap_or_default().into()))
            }
            "item/reasoning/summaryPartAdded" if p["summaryIndex"].as_i64().unwrap_or(0) > 0 => Some(AgentEvent::ReasoningDelta("\n\n".into())),
            "item/started" => {
                self.item_started(&p["item"], out);
                None
            }
            "item/completed" => {
                self.item_completed(&p["item"], out);
                None
            }
            "turn/plan/updated" => {
                self.plan_updated(p, out);
                None
            }
            "thread/tokenUsage/updated" => {
                let main = self.thread_id.clone();
                self.count_tokens(&main, p, out);
                context_event(p)
            }
            "turn/diff/updated" => {
                let (additions, deletions) = diff_stat(p["diff"].as_str().unwrap_or_default());
                Some(AgentEvent::DiffStat { additions, deletions })
            }
            "thread/settings/updated" => {
                if let Some(m) = p["threadSettings"]["collaborationMode"]["mode"].as_str() {
                    self.mode = Some(m.into());
                }
                None
            }
            // Sparse updates, one limit at a time: what's missing stays as last reported.
            "account/rateLimits/updated" => {
                let snapshot = &p["rateLimits"];
                let id = snapshot["limitId"].as_str().unwrap_or(MAIN_LIMIT).to_string();
                let merged = self.rate_limits.entry(id.clone()).or_insert(Value::Null);
                merge(merged, snapshot);
                out.events.extend(windows_used(merged, &id, self.model.as_deref()));
                None
            }
            "error" if p["willRetry"] != true => {
                self.limited |= is_limit_error(&p["error"]);
                let msg = readable_error(p["error"]["message"].as_str().unwrap_or("Codex error"));
                // A turn's fatal error also ends the turn, and `turn/completed` reports it.
                if self.busy() {
                    self.turn_error = Some(msg);
                    None
                } else {
                    Some(AgentEvent::Error(msg))
                }
            }
            _ => None,
        };
        out.events.extend(ev);
    }

    /// A `thread/tokenUsage/updated` for `thread` (this one or a sub-agent's): what it used is how
    /// far its running total moved since the last report (a resumed thread's total carries on
    /// from its history; the first report's `last` is its own request). Counted toward the
    /// running turn, or reported at once when none runs (a background sub-agent).
    fn count_tokens(&mut self, thread: &str, p: &Value, out: &mut Out) {
        let usage = &p["tokenUsage"];
        let total = trek_core::import::codex::tokens(&usage["total"]);
        let used = match self.token_totals.get(thread) {
            Some(before) => total.since(before),
            None => trek_core::import::codex::tokens(&usage["last"]),
        };
        self.token_totals.insert(thread.to_string(), total);
        // A sub-agent spawned on another model counts toward that one.
        let model = self.agents.get(thread).and_then(|a| a.model.clone()).or_else(|| self.model.clone());
        if used.is_empty() {
            return;
        }
        // Each report is one request: its prompt's size sets its tier, and Fast costs more.
        let cost = model.as_deref().and_then(|m| trek_core::pricing::request(m, &trek_core::AgentId::Codex, &used, 0, self.fast.is_some()));
        if self.busy() {
            match self.turn_tokens.iter_mut().find(|(m, _, _)| *m == model) {
                Some((_, t, c)) => {
                    t.add(&used);
                    *c = c.zip(cost).map(|(a, b)| UsageCost::priced(a.usd + b.usd));
                }
                None => self.turn_tokens.push((model, used, cost)),
            }
        } else {
            out.events.push(AgentEvent::Usage { model, tokens: used, cost });
        }
    }

    fn turn_completed(&mut self, turn: &Value, out: &mut Out) {
        self.turn = Turn::Idle;
        self.interrupt = false;
        // Codex resolves whatever it was still asking when the turn ends.
        self.pending.clear();
        self.edits.clear();
        let error = match turn["status"].as_str() {
            Some("failed") => Some(
                turn["error"]["message"].as_str().map(readable_error).or(self.turn_error.take()).unwrap_or_else(|| "The turn failed.".into()),
            ),
            Some("interrupted") => Some("Interrupted".to_string()),
            _ => None,
        };
        self.turn_error = None;
        let limited = std::mem::take(&mut self.limited) || is_limit_error(&turn["error"]);
        if let Some(message) = error.as_ref().filter(|e| limited || (*e != "Interrupted" && crate::limits::looks_like_limit(e))) {
            let (resets_at, scope) = limit_reset(&self.rate_limits, self.model.as_deref());
            let now = trek_core::store::now_ms();
            let scope = scope.unwrap_or_else(|| crate::limits::scope_of(message));
            let resets_at = resets_at.or_else(|| crate::limits::reset_from_text(message, now));
            out.events.push(AgentEvent::LimitReached { message: message.clone(), resets_at, scope });
        }
        let failed = error.is_some();
        // Whatever became of it, the turn is in the thread's history: one to cut back to.
        if let Some(id) = turn["id"].as_str().filter(|id| !id.is_empty()) {
            out.events.push(AgentEvent::Mark(id.to_string()));
        }
        for (model, tokens, cost) in std::mem::take(&mut self.turn_tokens) {
            out.events.push(AgentEvent::Usage { model, tokens, cost });
        }
        // Commands still running when their turn ends carry on in the background.
        let left: Vec<String> = self.commands.keys().filter(|id| !self.background.contains(id)).cloned().collect();
        if !left.is_empty() {
            self.background.extend(left);
            out.events.push(self.background_event());
        }
        out.events.push(AgentEvent::TurnComplete { error });
        // Messages that missed this turn start the next one, and answer its plan.
        let mut late = std::mem::take(&mut self.after_turn).into_iter();
        if let Some(first) = late.next() {
            let m = self.turn_start(first);
            out.send.push(m);
            self.held.extend(late);
            return;
        }
        // After the turn, like Codex's own "Implement this plan?" prompt.
        if let Some(plan) = self.proposed.take().filter(|p| self.plan && !failed && !p.trim().is_empty()) {
            let request_id = format!("{PLAN_REQUEST}{}", turn["id"].as_str().unwrap_or_default());
            self.pending.insert(request_id.clone(), Pending::Plan);
            out.events.push(AgentEvent::PermissionRequest { request_id, title: "Plan".into(), detail: String::new(), prompt: Some(Prompt::Plan(plan)) });
        }
    }

    fn item_started(&mut self, item: &Value, out: &mut Out) {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let (title, detail) = match item["type"].as_str() {
            Some("commandExecution") => {
                self.commands.insert(id.clone(), (command_text(item), String::new()));
                if let Some(process) = item["processId"].as_str().filter(|p| !p.is_empty()) {
                    self.processes.insert(id.clone(), process.to_string());
                }
                ("Run command".to_string(), command_text(item))
            }
            Some("fileChange") => {
                let paths = change_paths(item);
                let detail = paths.join(", ");
                self.edits.insert(id.clone(), paths);
                ("Edit".to_string(), detail)
            }
            Some("mcpToolCall" | "dynamicToolCall") => {
                let args = &item["arguments"];
                let detail = if args.as_object().is_some_and(|o| !o.is_empty()) { clip(&args.to_string(), 200) } else { String::new() };
                (mcp_title(item), detail)
            }
            Some("webSearch") => ("Search the web".to_string(), item["query"].as_str().unwrap_or_default().to_string()),
            Some("imageView") => ("Read".to_string(), item["path"].as_str().unwrap_or_default().to_string()),
            Some("collabAgentToolCall") if item["tool"] == "spawnAgent" => {
                ("Subagent".to_string(), clip(item["prompt"].as_str().unwrap_or_default(), 200))
            }
            Some("subAgentActivity") => return self.sub_agent_activity(item, out),
            Some("plan") => {
                self.drafting = Some((id, String::new(), false));
                return;
            }
            _ => return,
        };
        out.events.push(AgentEvent::ToolStarted { id, title, detail });
    }

    /// The proposed plan shows as a "Plan" row, like Claude's: its title as the detail, the
    /// whole plan as the output, and the plan card to approve it once the turn ends.
    fn plan_delta(&mut self, item: &str, delta: &str, out: &mut Out) {
        if self.drafting.as_ref().is_none_or(|(id, ..)| id != item) {
            self.drafting = Some((item.to_string(), String::new(), false));
        }
        let Some((id, text, shown)) = self.drafting.as_mut() else { return };
        text.push_str(delta);
        if !*shown && text.trim_start().contains('\n') {
            *shown = true;
            out.events.push(AgentEvent::ToolStarted { id: id.clone(), title: "Plan".into(), detail: plan_title(text) });
        }
    }

    fn item_completed(&mut self, item: &Value, out: &mut Out) {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let status = item["status"].as_str().unwrap_or_default();
        let declined = || if status == "declined" { "Declined".to_string() } else { String::new() };
        let ev = match item["type"].as_str() {
            Some("agentMessage") => AgentEvent::TextDone(item["text"].as_str().unwrap_or_default().into()),
            Some("plan") => {
                // The completed item is authoritative; the deltas may not add up to it.
                let text = item["text"].as_str().unwrap_or_default().to_string();
                let shown = self.drafting.take().is_some_and(|(row, _, shown)| row == id && shown);
                if !shown {
                    out.events.push(AgentEvent::ToolStarted { id: id.clone(), title: "Plan".into(), detail: plan_title(&text) });
                }
                self.proposed = Some(text.clone());
                AgentEvent::ToolFinished { id, output: text, ok: true }
            }
            Some("commandExecution") => {
                self.command_gone(&id, out);
                let output = item["aggregatedOutput"].as_str().map(|o| clip(o, 8000)).unwrap_or_else(declined);
                let ok = status == "completed" && item["exitCode"].as_i64().unwrap_or(0) == 0;
                out.events.push(AgentEvent::ToolFinished { id, output, ok });
                // Said once after the first command it hit, not at every one.
                if sandbox_not_set_up(item, self.windows) && !std::mem::replace(&mut self.sandbox_told, true) {
                    out.events.push(AgentEvent::Notice(SANDBOX_SETUP_NOTICE.into()));
                }
                return;
            }
            Some("fileChange") => {
                let (added, removed) = change_lines(item);
                if status == "completed" && added + removed > 0 {
                    out.events.push(AgentEvent::ToolLines { id: id.clone(), added, removed });
                }
                AgentEvent::ToolFinished { id, output: declined(), ok: status == "completed" }
            }
            Some("mcpToolCall" | "dynamicToolCall") => {
                AgentEvent::ToolFinished { id, output: tool_output(item), ok: status != "failed" && item["success"] != false }
            }
            Some("webSearch" | "imageView") => AgentEvent::ToolFinished { id, output: String::new(), ok: true },
            Some("collabAgentToolCall") => return self.collab_completed(item, out),
            Some("subAgentActivity") => return self.sub_agent_activity(item, out),
            _ => return,
        };
        out.events.push(ev);
    }

    /// `update_plan` (the agent's to-do list) as a tool row, like Claude's TodoWrite.
    fn plan_updated(&mut self, p: &Value, out: &mut Out) {
        let steps: Vec<(String, Step)> = p["plan"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                let step = match s["status"].as_str() {
                    Some("completed") => Step::Done,
                    Some("inProgress") => Step::Active,
                    _ => Step::Pending,
                };
                (s["step"].as_str().unwrap_or_default().to_string(), step)
            })
            .collect();
        if steps.is_empty() {
            return;
        }
        self.plan_updates += 1;
        let id = format!("plan-{}-{}", p["turnId"].as_str().unwrap_or_default(), self.plan_updates);
        let (detail, mut output) = plan_row(&steps);
        if let Some(why) = p["explanation"].as_str().filter(|e| !e.trim().is_empty()) {
            output = format!("{}\n\n{output}", why.trim());
        }
        out.events.push(AgentEvent::ToolStarted { id: id.clone(), title: "Update plan".into(), detail });
        out.events.push(AgentEvent::ToolFinished { id, output, ok: true });
    }

    /// Multi-agent v2: a sub-agent started, was given more work, finished or was interrupted.
    fn sub_agent_activity(&mut self, item: &Value, out: &mut Out) {
        let Some(thread) = item["agentThreadId"].as_str() else { return };
        match item["kind"].as_str() {
            // A finished agent that's given more work runs again, on a row of its own.
            Some("started" | "interacted") if self.agents.get(thread).is_none_or(|a| a.done) => {
                let name = agent_name(item["agentPath"].as_str().unwrap_or_default());
                let runs = self.agents.get(thread).map_or(0, |a| a.runs) + 1;
                let row = if runs == 1 { thread.to_string() } else { format!("{thread}-{runs}") };
                let model = item["model"].as_str().filter(|m| !m.is_empty()).map(String::from).or_else(|| self.agents.get(thread).and_then(|a| a.model.clone()));
                self.agents.insert(thread.into(), SubAgent { row: row.clone(), runs, tool_uses: 0, last: String::new(), done: false, model });
                out.events.push(AgentEvent::ToolStarted { id: row.clone(), title: "Subagent".into(), detail: name.clone() });
                out.events.push(AgentEvent::Task { id: row, description: Some(name), activity: None, tool_uses: None, done: None });
            }
            Some("completed") => self.finish_agent(thread, true, out),
            Some("interrupted") => self.finish_agent(thread, false, out),
            _ => {}
        }
    }

    /// Multi-agent v1: `spawnAgent` names the new threads; any collab call reports agent states.
    fn collab_completed(&mut self, item: &Value, out: &mut Out) {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let spawn = item["tool"] == "spawnAgent";
        if spawn {
            let description = Some(clip(item["prompt"].as_str().unwrap_or("Subagent"), 200));
            for t in item["receiverThreadIds"].as_array().into_iter().flatten().filter_map(|t| t.as_str()) {
                if !self.agents.contains_key(t) {
                    let model = item["model"].as_str().filter(|m| !m.is_empty()).map(String::from);
                    self.agents.insert(t.into(), SubAgent { row: id.clone(), runs: 1, tool_uses: 0, last: String::new(), done: false, model });
                    out.events.push(AgentEvent::Task { id: id.clone(), description: description.clone(), activity: None, tool_uses: None, done: None });
                }
            }
        }
        for (t, state) in item["agentsStates"].as_object().into_iter().flatten() {
            if let Some(m) = state["message"].as_str()
                && let Some(a) = self.agents.get_mut(t)
            {
                a.last = m.to_string();
            }
            match state["status"].as_str() {
                Some("completed" | "shutdown") => self.finish_agent(t, true, out),
                Some("errored" | "interrupted" | "notFound") => self.finish_agent(t, false, out),
                _ => {}
            }
        }
        if spawn {
            out.events.push(AgentEvent::ToolFinished { id, output: String::new(), ok: item["status"] != "failed" });
        }
    }

    /// Events from a sub-agent's own thread: progress on its row.
    fn sub_agent(&mut self, thread: &str, method: &str, p: &Value, out: &mut Out) {
        let Some(a) = self.agents.get_mut(thread) else { return };
        match method {
            "item/started" => {
                if let Some(activity) = activity(&p["item"]) {
                    a.tool_uses += 1;
                    out.events.push(AgentEvent::Task { id: a.row.clone(), description: None, activity: Some(activity), tool_uses: Some(a.tool_uses), done: None });
                }
                if let Some((title, detail)) = tool_row(&p["item"]) {
                    out.events.push(AgentEvent::TaskStep { task: a.row.clone(), title, detail });
                }
            }
            "item/completed" if p["item"]["type"] == "agentMessage" => a.last = p["item"]["text"].as_str().unwrap_or_default().to_string(),
            "turn/completed" => {
                let ok = p["turn"]["status"] == "completed";
                self.finish_agent(thread, ok, out);
            }
            _ => {}
        }
    }

    fn finish_agent(&mut self, thread: &str, ok: bool, out: &mut Out) {
        let Some(a) = self.agents.get_mut(thread).filter(|a| !a.done) else { return };
        a.done = true;
        out.events.push(AgentEvent::Task { id: a.row.clone(), description: None, activity: None, tool_uses: None, done: Some(ok) });
        out.events.push(AgentEvent::ToolFinished { id: a.row.clone(), output: clip(&a.last, 8000), ok });
    }
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let mut backlog = Vec::new();
    let (child, mut rpc, mut lines, stderr) = start_app_server(&config.cwd, UNUSED_NOTIFICATIONS, &mut backlog).await?;
    // Which login the session uses (ChatGPT plan or API key); answered alongside thread/start.
    let account_req = rpc.request("account/read", json!({})).await?;

    let (sandbox, approval, reviewer) = config.hand_holding.codex_policy();
    let mut params = json!({
        "cwd": config.cwd.display().to_string(), "sandbox": sandbox, "approvalPolicy": approval, "approvalsReviewer": reviewer,
    });
    if let Some(m) = &config.model {
        params["model"] = json!(m);
    }
    if !config.mcp_servers.is_empty() {
        // Config overrides merge with the user's own `mcp_servers` (verified against 0.160).
        let (servers, left_out) = codex_mcp_servers(&config.mcp_servers);
        for notice in left_out {
            let _ = events.send(AgentEvent::Notice(notice)).await;
        }
        params["config"] = json!({ "mcp_servers": servers });
    }
    let mut lost = false;
    // The thread couldn't be cut back or forked where asked: a new one, with the recap.
    let mut cut_off = false;
    let resumed = match opening(&config, &params) {
        Some((method, p)) => {
            let id = rpc.request(method, p).await?;
            match startup_response(&mut lines, id, &mut backlog, method).await {
                Ok(r) if method == "thread/resume" && config.resume_at.is_some() => {
                    let (thread, at) = (r["thread"]["id"].as_str().unwrap_or_default().to_string(), config.resume_at.clone().unwrap_or_default());
                    match cut_back(&mut rpc, &mut lines, &mut backlog, &thread, &at).await {
                        Ok(()) => Some(r),
                        Err(e) if e.downcast_ref::<ResponseTimeout>().is_some() => {
                            stop(child, rpc).await;
                            return Err(e);
                        }
                        Err(e) => {
                            tracing::warn!("codex couldn't cut thread {thread} back to turn {at}, starting a new one: {e:#}");
                            cut_off = true;
                            None
                        }
                    }
                }
                Ok(r) => Some(r),
                Err(e) if e.downcast_ref::<ResponseTimeout>().is_some() => {
                    stop(child, rpc).await;
                    return Err(e);
                }
                // Codex no longer has the thread (its rollout was deleted): carry on in a new one.
                Err(e) if e.to_string().contains("no rollout found") => {
                    tracing::warn!("codex resume failed, starting a new thread: {e:#}");
                    lost = true;
                    None
                }
                Err(e) if method == "thread/fork" || config.resume_at.is_some() => {
                    tracing::warn!("codex fork failed, starting a new thread: {e:#}");
                    cut_off = true;
                    None
                }
                Err(e) => return Err(e),
            }
        }
        None => None,
    };
    let opened = match resumed {
        Some(r) => r,
        None => {
            let id = rpc.request("thread/start", params).await?;
            match startup_response(&mut lines, id, &mut backlog, "thread/start").await {
                Ok(r) => r,
                Err(e) => {
                    stop(child, rpc).await;
                    return Err(e);
                }
            }
        }
    };
    let thread_id = opened["thread"]["id"].as_str().context("no thread id")?.to_string();
    let mut s = Session::new(thread_id.clone(), &config, &opened, rpc.next_id);
    s.calls.insert(account_req, Call::Account);
    events.send(AgentEvent::Started { native_id: thread_id, model: s.model.clone() }).await?;
    if (lost || cut_off) && config.recap.is_some() {
        s.recap = config.recap.clone();
        events
            .send(AgentEvent::Notice("Codex couldn't take its thread back to that point, so it continues in a new thread with a recap of this conversation.".into()))
            .await?;
    } else if lost {
        events.send(crate::lost_session("Codex")).await?;
    } else if let Some(at) = config.resume_at.clone().filter(|_| config.resume.is_some()) {
        // Taken back (or forked) to just after that turn: it's the latest point now.
        events.send(AgentEvent::Mark(at)).await?;
    }

    let mut out = Out::default();
    for v in std::mem::take(&mut backlog) {
        let o = s.incoming(&v);
        out.events.extend(o.events);
        out.send.extend(o.send);
    }
    loop {
        for msg in std::mem::take(&mut out.send) {
            rpc.send(&msg).await?;
        }
        for ev in std::mem::take(&mut out.events) {
            if events.send(ev).await.is_err() {
                stop(child, rpc).await;
                return Ok(());
            }
        }
        tokio::select! {
            cmd = commands.recv() => {
                match cmd {
                    Ok(Command::Shutdown) | Err(_) => break,
                    Ok(cmd) => out = s.command(cmd),
                }
            }
            line = lines.next_line() => {
                let Some(line) = line? else {
                    if s.busy() {
                        return Err(stderr.exited("Codex"));
                    }
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                out = s.incoming(&v);
            }
        }
    }
    stop(child, rpc).await;
    Ok(())
}

/// The request that opens an existing thread: a fork (through `resume_at` when given), or a
/// resume. `None` for a new thread. `params`: the session's settings, as `thread/start` takes them.
fn opening(config: &SessionConfig, params: &Value) -> Option<(&'static str, Value)> {
    let thread = config.resume.as_ref()?;
    let mut p = params.clone();
    p["threadId"] = json!(thread);
    p["excludeTurns"] = json!(true);
    if !config.fork {
        return Some(("thread/resume", p));
    }
    if let Some(at) = &config.resume_at {
        p["lastTurnId"] = json!(at);
    }
    Some(("thread/fork", p))
}

/// Scanning a thread's turns newest first (one `thread/turns/list` page at a time) for turn `at`:
/// `Some(next)` once it's found, with the turn that followed it (`None`: it's the latest).
/// `newer` carries the last turn seen from page to page.
fn turn_after(page: &Value, at: &str, newer: &mut Option<String>) -> Option<Option<String>> {
    for turn in page["data"].as_array().into_iter().flatten() {
        let id = turn["id"].as_str().unwrap_or_default();
        if id == at {
            return Some(newer.clone());
        }
        *newer = Some(id.to_string());
    }
    None
}

/// Take `thread` back to just after turn `at`: the turns after it leave its history (files are
/// Trek's business, not Codex's).
async fn cut_back(rpc: &mut Rpc, lines: &mut RpcLines, backlog: &mut Vec<Value>, thread: &str, at: &str) -> Result<()> {
    let mut cursor: Option<String> = None;
    let mut newer = None;
    let next = loop {
        let mut p = json!({ "threadId": thread, "limit": 50, "sortDirection": "desc", "itemsView": "notLoaded" });
        if let Some(c) = &cursor {
            p["cursor"] = json!(c);
        }
        let id = rpc.request("thread/turns/list", p).await?;
        let page = startup_response(lines, id, backlog, "thread/turns/list").await?;
        if let Some(next) = turn_after(&page, at, &mut newer) {
            break next;
        }
        cursor = page["nextCursor"].as_str().map(String::from);
        if cursor.is_none() {
            bail!("turn {at} isn't in thread {thread}");
        }
    };
    // `at` is the latest turn: nothing to drop.
    let Some(next) = next else { return Ok(()) };
    let id = rpc.request("thread/revert", json!({ "threadId": thread, "beforeTurnId": next })).await?;
    startup_response(lines, id, backlog, "thread/revert").await.map(|_| ())
}

/// How the login is billed, from an `account/read` result. Older Codex builds without the
/// method answer with an error, which leaves billing unknown.
pub(crate) fn account_billing(r: &Value) -> Option<Billing> {
    let a = &r["account"];
    match a["type"].as_str()? {
        "chatgpt" => Some(Billing::Plan(a["planType"].as_str().map(crate::status::codex_plan_name))),
        "apiKey" | "amazonBedrock" => Some(Billing::Metered),
        _ => None,
    }
}

/// Live model list from the user's Codex setup (includes custom providers), with efforts.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let mut backlog = Vec::new();
    let (mut child, mut rpc, mut lines, _) = start_app_server(&trek_core::paths::home(), &[], &mut backlog).await?;
    let out = fetch_models(&mut rpc, &mut lines, &mut backlog).await;
    // A probe that opened no thread: Codex has nothing to save.
    child.kill_now().await;
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
            // Codex in Trek means OpenAI's models; `provider/model` ids come from custom routing.
            if m["hidden"] == true || m["id"].as_str().or(m["model"].as_str()).is_some_and(|id| id.contains('/')) {
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
    use trek_core::AgentId;

    /// Recorded app-server output (codex-cli 0.160, gpt-5.6-luna), one message per line.
    fn fixture(text: &str) -> Vec<Value> {
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn session(thread: &str, plan: bool) -> Session {
        let config = SessionConfig {
            agent: AgentId::Codex,
            cwd: "/tmp/trek-agents-e2e".into(),
            model: Some("gpt-5.6-luna".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::Supervised,
            plan,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        Session::new(thread.into(), &config, &json!({ "model": "gpt-5.6-luna" }), 2)
    }

    fn feed(s: &mut Session, lines: &[Value]) -> Out {
        let mut out = Out::default();
        for v in lines {
            let o = s.incoming(v);
            out.events.extend(o.events);
            out.send.extend(o.send);
        }
        out
    }

    fn prompt(text: &str) -> Command {
        Command::Prompt { text: text.into(), images: vec![] }
    }

    fn turn_completes(events: &[AgentEvent]) -> usize {
        events.iter().filter(|e| matches!(e, AgentEvent::TurnComplete { .. })).count()
    }

    /// `codex_mcp_servers` where nothing is handed over differently: not Windows.
    fn as_written(servers: &[crate::McpServer]) -> Value {
        let (out, left_out) = codex_mcp_servers_with(servers, crate::mcp_servers_json_with(servers, BatchHandOff::ResolvedPath, false, |_| Some("C:\\n\\npx.cmd".into())));
        assert!(left_out.is_empty());
        out
    }

    #[test]
    fn slow_mcp_tools_get_a_longer_timeout() {
        let server = |name: &str, timeout| crate::McpServer { tool_timeout_secs: timeout, ..crate::McpServer::stdio(name, "trek-mcp", vec![], vec![]) };
        let out = as_written(&[server("trek-orchestrate", Some(1900)), server("fs", None)]);
        assert_eq!(out["trek-orchestrate"]["tool_timeout_sec"], 1900);
        assert!(out["fs"].get("tool_timeout_sec").is_none(), "others keep Codex's default");
    }

    #[test]
    fn remote_mcp_servers_use_codexs_url_keys() {
        let out = as_written(&[
            crate::McpServer::http("figma-desktop", "http://127.0.0.1:3845/mcp", vec![]),
            crate::McpServer::http("linear", "https://mcp.linear.app/mcp", vec![("Authorization".into(), "Bearer t".into())]),
            // A command's environment (its tokens) goes in `env`, as Codex's config has it.
            crate::McpServer::stdio("fs", "npx", vec!["srv".into()], vec![("GITHUB_TOKEN".into(), "ghp_1".into())]),
        ]);
        assert_eq!(
            out,
            json!({
                "figma-desktop": {"url":"http://127.0.0.1:3845/mcp"},
                "linear": {"url":"https://mcp.linear.app/mcp","http_headers":{"Authorization":"Bearer t"}},
                "fs": {"command":"npx","args":["srv"],"env":{"GITHUB_TOKEN":"ghp_1"}},
            })
        );
    }

    #[test]
    fn permission_access_names_are_unicode_safe() {
        let request = |access: &str| json!({"permissions":{"fileSystem":{"entries":[{"path":{"path":"/tmp/x"},"access":access}]}}});
        assert_eq!(permissions_summary(&request("")), ": /tmp/x");
        assert_eq!(permissions_summary(&request("écrire")), "Écrire: /tmp/x");
    }

    #[tokio::test]
    async fn startup_responses_have_a_timeout() {
        let (_write, read) = tokio::io::duplex(64);
        let mut lines = crate::ProtocolLines::new(BufReader::new(read));
        let e = response_with_timeout(&mut lines, 1, &mut vec![], "thread/start", std::time::Duration::from_millis(10)).await.unwrap_err();
        assert!(e.to_string().contains("didn't answer thread/start"), "{e:#}");
        assert!(e.downcast_ref::<ResponseTimeout>().is_some(), "timeouts must not fall back to a new thread");
    }

    #[test]
    fn background_commands_are_listed_before_the_turn_closes() {
        let mut s = session("t", false);
        s.turn = Turn::Running("u".into());
        s.commands.insert("c".into(), ("sleep 300".into(), String::new()));
        let out = s.incoming(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"completed"}}}));
        let background = out.events.iter().position(|e| matches!(e, AgentEvent::Background(_))).unwrap();
        let complete = out.events.iter().position(|e| matches!(e, AgentEvent::TurnComplete { .. })).unwrap();
        assert!(background < complete, "{:?}", out.events);
    }

    #[test]
    fn trek_s_own_tools_are_named_with_their_server() {
        assert_eq!(mcp_title(&json!({ "server": "trek-orchestrate", "tool": "delegate_task" })), "mcp__trek-orchestrate__delegate_task");
        assert_eq!(mcp_title(&json!({ "server": "t3-code", "tool": "delegate_task" })), "delegate_task", "another server's tool keeps its name");
        assert!(!trek_core::orchestrate::is_delegate_call(&mcp_title(&json!({ "server": "t3-code", "tool": "delegate_task" }))));
    }

    #[test]
    fn rewinds_and_forks_open_the_thread_where_asked() {
        let params = json!({ "cwd": "/tmp/x", "sandbox": "read-only", "approvalPolicy": "on-request", "approvalsReviewer": "user", "model": "gpt-5.6-luna" });
        let mut config = session_config(None);
        assert_eq!(opening(&config, &params), None, "a new thread starts");
        config.resume = Some("t1".into());
        let (method, p) = opening(&config, &params).unwrap();
        assert_eq!(method, "thread/resume");
        assert_eq!((p["threadId"].as_str(), p["model"].as_str(), p["excludeTurns"].as_bool()), (Some("t1"), Some("gpt-5.6-luna"), Some(true)));
        // Cut back in place: resumed, then reverted (`cut_back`).
        config.resume_at = Some("turn-1".into());
        assert_eq!(opening(&config, &params).unwrap().0, "thread/resume");
        // A fork through a turn keeps the session's settings.
        config.fork = true;
        let (method, p) = opening(&config, &params).unwrap();
        assert_eq!(method, "thread/fork");
        assert_eq!((p["threadId"].as_str(), p["lastTurnId"].as_str(), p["sandbox"].as_str(), p["cwd"].as_str()), (Some("t1"), Some("turn-1"), Some("read-only"), Some("/tmp/x")));
        config.resume_at = None;
        assert!(opening(&config, &params).unwrap().1.get("lastTurnId").is_none(), "a whole fork");
    }

    #[test]
    fn the_turn_after_a_point_is_found_page_by_page() {
        // Recorded `thread/turns/list` (newest first, two a page) over three turns, and the
        // thread after `thread/revert` dropped the last two.
        let pages = fixture(include_str!("../fixtures/codex-turns-pages.jsonl"));
        let (first, second, third) = ("01a100af-e7ff-7e13-9471-146425462502", "01a100b0-190f-7d31-9329-a1b44ce180d0", "01a100b0-28bc-7fb2-8882-ec579ddc4ccd");
        // The first turn is on the second page; the turn after it is on the first.
        let mut newer = None;
        assert_eq!(turn_after(&pages[0]["result"], first, &mut newer), None);
        assert!(pages[0]["result"]["nextCursor"].is_string());
        assert_eq!(turn_after(&pages[1]["result"], first, &mut newer), Some(Some(second.to_string())));
        // The latest turn has nothing after it.
        assert_eq!(turn_after(&pages[0]["result"], third, &mut None), Some(None));
        assert_eq!(turn_after(&pages[0]["result"], second, &mut None), Some(Some(third.to_string())));
        // Reverting before the second turn left only the first.
        let left: Vec<&str> = pages[2]["result"]["data"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
        assert_eq!(left, [first]);
    }

    #[test]
    fn a_thread_that_couldnt_be_cut_back_gets_the_recap_with_its_first_message() {
        let mut s = session("t", false);
        s.recap = Some("User: remember APPLE\n\nAssistant: OK".into());
        let first = s.command(prompt("which word?"));
        let text = first.send[0]["params"]["input"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
        assert!(text.contains("<recap>\nUser: remember APPLE") && text.ends_with("which word?"), "{text}");
        s.incoming(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"turn-1","status":"completed","items":[]}}}));
        let next = s.command(prompt("and now?"));
        assert_eq!(next.send[0]["params"]["input"].as_array().unwrap().last().unwrap()["text"], "and now?", "only the first message carries it");
    }

    fn session_config(resume: Option<&str>) -> SessionConfig {
        SessionConfig {
            agent: AgentId::Codex,
            cwd: "/tmp/x".into(),
            model: Some("gpt-5.6-luna".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::Supervised,
            plan: false,
            read_only: false,
            resume: resume.map(String::from),
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        }
    }

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
    fn billing_from_account_read() {
        let plus = json!({"account":{"type":"chatgpt","email":"me@example.com","planType":"plus"},"requiresOpenaiAuth":true});
        assert_eq!(account_billing(&plus), Some(Billing::Plan(Some("ChatGPT Plus".into()))));
        assert_eq!(account_billing(&json!({"account":{"type":"chatgpt","email":null}})), Some(Billing::Plan(None)));
        assert_eq!(account_billing(&json!({"account":{"type":"apiKey"}})), Some(Billing::Metered));
        assert_eq!(account_billing(&json!({"account":null,"requiresOpenaiAuth":true})), None);
        assert_eq!(account_billing(&Value::Null), None);

        // The answer reaches the session by its request id, whenever it arrives; a server request
        // that reuses the number isn't mistaken for it.
        let mut s = session("t", false);
        s.calls.insert(1, Call::Account);
        let reused = feed(&mut s, &[json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"threadId":"t","itemId":"c","command":"ls"}})]);
        assert!(!reused.events.iter().any(|e| matches!(e, AgentEvent::Billing(_))));
        let out = feed(&mut s, &[json!({"id":1,"result":plus})]);
        assert_eq!(out.events, vec![AgentEvent::Billing(Billing::Plan(Some("ChatGPT Plus".into())))]);
        // Older builds without the method: an error, and billing stays unknown.
        s.calls.insert(2, Call::Account);
        assert!(feed(&mut s, &[json!({"id":2,"error":{"code":-32601,"message":"unknown method"}})]).events.is_empty());
    }

    #[test]
    fn user_input_lists_local_images_then_text() {
        assert_eq!(
            user_input("hi", &[PathBuf::from("/tmp/a.png")]),
            json!([{"type":"localImage","path":"/tmp/a.png"},{"type":"text","text":"hi"}])
        );
    }

    #[test]
    fn shell_wrappers_are_unwrapped() {
        assert_eq!(unwrap_shell("/bin/zsh -lc 'touch a.txt'"), "touch a.txt");
        assert_eq!(unwrap_shell("/bin/zsh -lc ls"), "ls");
        assert_eq!(unwrap_shell("bash -c \"echo \\\"hi\\\"\""), "echo \"hi\"");
        assert_eq!(unwrap_shell("/bin/zsh -lc 'echo '\\''x'\\'''"), "echo 'x'");
        assert_eq!(unwrap_shell("ls -c foo"), "ls -c foo");
    }

    #[test]
    fn windows_shell_wrappers_are_unwrapped() {
        // Codex's own flags: `-NoProfile -Command`, the script as one argument.
        assert_eq!(unwrap_shell("powershell.exe -NoProfile -Command 'Get-ChildItem'"), "Get-ChildItem");
        assert_eq!(unwrap_shell("powershell.exe -Command Get-ChildItem -Force"), "Get-ChildItem -Force");
        assert_eq!(unwrap_shell("pwsh -NoLogo -NoProfile -Command \"Get-Content a.txt\""), "Get-Content a.txt");
        assert_eq!(unwrap_shell("PowerShell -c 'ls'"), "ls");
        assert_eq!(unwrap_shell("powershell -ExecutionPolicy Bypass -NoProfile -Command 'ls'"), "ls");
        // A full path, quoted when it has spaces.
        assert_eq!(unwrap_shell(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile -Command 'dir'"), "dir");
        assert_eq!(unwrap_shell(r"'C:\Program Files\PowerShell\7\pwsh.exe' -NoProfile -Command 'dir'"), "dir");
        assert_eq!(unwrap_shell(r#""C:\Program Files\PowerShell\7\pwsh.exe" -Command dir"#), "dir");
        // Quotes inside: `''` in single quotes (and the POSIX spelling), `\"` and `""` in double quotes.
        assert_eq!(unwrap_shell("powershell.exe -Command 'Write-Output ''hi'''"), "Write-Output 'hi'");
        assert_eq!(unwrap_shell("powershell.exe -Command 'Write-Output '\"'\"'hi'\"'\"''"), "Write-Output 'hi'");
        assert_eq!(unwrap_shell(r#"powershell.exe -Command "Write-Output \"hi\"""#), "Write-Output \"hi\"");
        assert_eq!(unwrap_shell(r#"powershell.exe -Command "Write-Output ""hi""""#), "Write-Output \"hi\"");
        // The UTF-8 line Codex puts ahead of the script isn't the model's.
        assert_eq!(unwrap_shell("powershell.exe -NoProfile -Command '[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;\nGet-Date'"), "Get-Date");
        assert_eq!(unwrap_shell("powershell.exe -Command '[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Get-Date'"), "Get-Date");
        // cmd.exe.
        assert_eq!(unwrap_shell("cmd.exe /c dir /b"), "dir /b");
        assert_eq!(unwrap_shell("cmd /d /s /c \"echo hi\""), "echo hi");
        assert_eq!(unwrap_shell(r"C:\Windows\System32\cmd.exe /C type a.txt"), "type a.txt");
        // Not wrappers: another program, or PowerShell running a file.
        assert_eq!(unwrap_shell("powershell.exe -File a.ps1"), "powershell.exe -File a.ps1");
        assert_eq!(unwrap_shell("cargo test -c foo"), "cargo test -c foo");
        assert_eq!(unwrap_shell("powershell.exe"), "powershell.exe");
        assert_eq!(unwrap_shell("cmd.exe /c"), "cmd.exe /c");
    }

    #[test]
    fn turn_start_carries_policy_model_and_effort() {
        let mut s = session("t", false);
        s.command(Command::SetModel { model: "gpt-6-luna".into(), effort: Effort::High });
        s.command(Command::SetHandHolding(HandHolding::AutoAcceptEdits));
        let out = s.command(prompt("hi"));
        let p = &out.send[0]["params"];
        assert_eq!(out.send[0]["method"], "turn/start");
        assert_eq!((p["model"].as_str(), p["effort"].as_str()), (Some("gpt-6-luna"), Some("high")));
        assert_eq!(p["sandboxPolicy"], json!({"type":"workspaceWrite"}));
        assert_eq!(p["approvalPolicy"], "on-request");
        // Not in Plan mode, never was: no collaboration mode is sent.
        assert!(p.get("collaborationMode").is_none());
    }

    #[test]
    fn plan_turn_asks_a_question_streams_the_plan_and_offers_to_implement_it() {
        let lines = fixture(include_str!("../fixtures/codex-plan-question.jsonl"));
        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        let start = s.command(prompt("Plan adding a file hello.txt")).send;
        assert_eq!(start[0]["params"]["collaborationMode"]["mode"], "plan");
        assert_eq!(start[0]["params"]["collaborationMode"]["settings"]["model"], "gpt-5.6-luna");

        // The question becomes a Questions prompt; the answer goes back keyed by question id.
        let out = feed(&mut s, &lines[..2]);
        let AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Questions(qs)), .. } = &out.events[0] else { panic!("{:?}", out.events) };
        assert_eq!(request_id, "codex-0");
        assert_eq!((qs[0].question.as_str(), qs[0].header.as_str(), qs[0].options.len()), ("What should hello.txt contain?", "File content", 3));
        assert_eq!(qs[0].options[0].0, "Hello, world! (Recommended)");
        let answer = s.command(Command::Answer { request_id: "codex-0".into(), answers: vec![(qs[0].question.clone(), qs[0].options[0].0.clone())] });
        assert_eq!(answer.send, vec![json!({"id":0,"result":{"answers":{"content":{"answers":["Hello, world! (Recommended)"]}}}})]);

        // The plan shows as a "Plan" row once its title is in (not as the reply too), then waits
        // for approval once the turn is over.
        let out = feed(&mut s, &lines[2..]);
        let row = "01a0fe4b-3b8c-7841-b156-902ccc923692-plan";
        assert!(!out.events.iter().any(|e| matches!(e, AgentEvent::TextDelta(_) | AgentEvent::TextDone(_))));
        assert_eq!(out.events[0], AgentEvent::ToolStarted { id: row.into(), title: "Plan".into(), detail: "Add `hello.txt`".into() });
        let n = out.events.len();
        assert_eq!(n, 5);
        assert!(matches!(&out.events[n - 4], AgentEvent::ToolFinished { id, output, ok: true } if id == row && output.starts_with("# Add `hello.txt`\n\n## Summary")));
        assert!(matches!(&out.events[n - 3], AgentEvent::Mark(_)), "the finished turn is a point to cut back to");
        assert_eq!(out.events[n - 2], AgentEvent::TurnComplete { error: None });
        let AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Plan(plan)), .. } = &out.events[n - 1] else { panic!() };
        assert!(plan.contains("Hello, world!"));

        // Approving leaves Plan mode explicitly and starts the work.
        let go = s.command(Command::Respond { request_id: request_id.clone(), decision: Decision::Allow }).send;
        assert_eq!(go[0]["method"], "turn/start");
        assert_eq!(go[0]["params"]["collaborationMode"]["mode"], "default");
        assert_eq!(go[0]["params"]["input"][0]["text"], "Implement the plan.");
    }

    #[test]
    fn keep_planning_stays_in_plan_mode() {
        let lines = fixture(include_str!("../fixtures/codex-plan-question.jsonl"));
        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        s.command(prompt("Plan it"));
        let out = feed(&mut s, &lines[2..]);
        let Some(AgentEvent::PermissionRequest { request_id, .. }) = out.events.last() else { panic!() };
        assert!(s.command(Command::Respond { request_id: request_id.clone(), decision: Decision::Deny }).send.is_empty());
        let next = s.command(prompt("Make it say hi instead")).send;
        assert_eq!(next[0]["params"]["collaborationMode"]["mode"], "plan");
    }

    #[test]
    fn a_resumed_plan_mode_thread_is_taken_out_of_plan_mode() {
        let config = SessionConfig {
            agent: AgentId::Codex,
            cwd: "/tmp".into(),
            model: None,
            effort: Effort::Medium,
            hand_holding: HandHolding::Supervised,
            plan: false,
            read_only: false,
            resume: Some("t".into()),
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let opened = json!({"model":"gpt-5.6-luna","collaborationMode":{"mode":"plan","settings":{"model":"gpt-5.6-luna"}}});
        let mut s = Session::new("t".into(), &config, &opened, 2);
        let p = &s.command(prompt("go")).send[0]["params"];
        assert_eq!(p["collaborationMode"]["mode"], "default");
        assert_eq!(p["collaborationMode"]["settings"], json!({"model":"gpt-5.6-luna","reasoning_effort":"medium","developer_instructions":null}));
        // Said once; Codex keeps it from there.
        s.turn = Turn::Idle;
        assert!(s.command(prompt("again")).send[0]["params"].get("collaborationMode").is_none());
    }

    #[test]
    fn questions_take_free_text_and_can_be_skipped() {
        let lines = fixture(include_str!("../fixtures/codex-plan-question.jsonl"));
        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        feed(&mut s, &lines[..2]);
        let typed = s.command(Command::Answer { request_id: "codex-0".into(), answers: vec![("What should hello.txt contain?".into(), "Just: hi there".into())] });
        assert_eq!(typed.send[0]["result"]["answers"]["content"]["answers"], json!(["Just: hi there"]));

        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        feed(&mut s, &lines[..2]);
        let skipped = s.command(Command::Respond { request_id: "codex-0".into(), decision: Decision::Deny });
        assert_eq!(skipped.send, vec![json!({"id":0,"result":{"answers":{}}})]);
    }

    #[test]
    fn command_approvals_map_each_decision() {
        let lines = fixture(include_str!("../fixtures/codex-approvals.jsonl"));
        let mut s = session("01a0fe4d-09c7-77c0-b896-912482b6fdd8", false);
        let mut sent = vec![];
        let mut events = vec![];
        let mut decisions = vec![Decision::Allow, Decision::Deny, Decision::AllowForSession].into_iter();
        for v in &lines {
            let out = s.incoming(v);
            for ev in &out.events {
                if let AgentEvent::PermissionRequest { request_id, title, detail, .. } = ev {
                    assert_eq!(title, "Run command");
                    assert!(detail.starts_with("touch "), "{detail}");
                    sent.extend(s.command(Command::Respond { request_id: request_id.clone(), decision: decisions.next().unwrap() }).send);
                }
            }
            events.extend(out.events);
        }
        assert_eq!(
            sent,
            vec![
                json!({"id":0,"result":{"decision":"accept"}}),
                json!({"id":1,"result":{"decision":"decline"}}),
                json!({"id":2,"result":{"decision":"acceptForSession"}}),
            ]
        );
        let started: Vec<&str> = events.iter().filter_map(|e| if let AgentEvent::ToolStarted { detail, .. } = e { Some(detail.as_str()) } else { None }).collect();
        assert_eq!(started, vec!["touch a.txt", "touch b.txt", "touch c.txt"]);
        let finished: Vec<bool> = events.iter().filter_map(|e| if let AgentEvent::ToolFinished { ok, .. } = e { Some(*ok) } else { None }).collect();
        assert_eq!(finished, vec![true, false, true]);
        assert!(events.contains(&AgentEvent::ToolFinished { id: "exec-cb1e614a-b651-40ef-ae77-d085904383ce".into(), output: "Declined".into(), ok: false }));
        assert_eq!(turn_completes(&events), 1);
    }

    #[test]
    fn file_changes_count_their_lines() {
        let item = json!({"changes":[
            {"path":"/p/new.md","kind":{"type":"add"},"diff":"# New\n\nhi\n"},
            {"path":"/p/a.rs","kind":{"type":"update","move_path":null},"diff":"@@ -1,2 +1,2 @@\n-old\n+new\n same\n"},
            {"path":"/p/gone.txt","kind":{"type":"delete"},"diff":"x\ny\n"}
        ]});
        assert_eq!(change_lines(&item), (4, 3));
        assert_eq!(change_lines(&json!({})), (0, 0));
    }

    #[test]
    fn file_change_approval_names_the_files() {
        let lines = fixture(include_str!("../fixtures/codex-file-change.jsonl"));
        let mut s = session("01a0fe61-b5ad-7aa2-ade8-be4ef1741959", false);
        let out = feed(&mut s, &lines[..3]);
        assert_eq!(out.events[0], AgentEvent::ToolStarted { id: "exec-0fe3013c-9d31-44b2-936e-e82ea3758de3".into(), title: "Edit".into(), detail: "/tmp/trek-agents-e2e/d.txt".into() });
        assert_eq!(
            out.events[1],
            AgentEvent::PermissionRequest { request_id: "codex-0".into(), title: "Edit".into(), detail: "/tmp/trek-agents-e2e/d.txt".into(), prompt: None }
        );
        let ok = s.command(Command::Respond { request_id: "codex-0".into(), decision: Decision::Allow });
        assert_eq!(ok.send, vec![json!({"id":0,"result":{"decision":"accept"}})]);
        let out = feed(&mut s, &lines[3..]);
        assert!(out.events.contains(&AgentEvent::ToolFinished { id: "exec-0fe3013c-9d31-44b2-936e-e82ea3758de3".into(), output: String::new(), ok: true }));
        // A new one-line file.
        assert!(out.events.contains(&AgentEvent::ToolLines { id: "exec-0fe3013c-9d31-44b2-936e-e82ea3758de3".into(), added: 1, removed: 0 }));
        assert!(out.events.contains(&AgentEvent::DiffStat { additions: 1, deletions: 0 }));

        // Above Supervised, Codex's sandbox decides; Trek doesn't ask again.
        let mut s = session("01a0fe61-b5ad-7aa2-ade8-be4ef1741959", false);
        s.command(Command::SetHandHolding(HandHolding::AutoAcceptEdits));
        let out = feed(&mut s, &lines[..3]);
        assert!(!out.events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })));
        assert_eq!(out.send, vec![json!({"id":0,"result":{"decision":"accept"}})]);

        // A write outside the project (or one asking for more room) is the user's call below
        // Full access: Codex asks only because its sandbox blocks it.
        let outside: Vec<Value> = lines[..3].iter().map(|v| serde_json::from_str(&v.to_string().replace("/tmp/trek-agents-e2e/d.txt", "/Users/someone/.zshrc")).unwrap()).collect();
        let mut wider = lines[..3].to_vec();
        wider[2]["params"]["grantRoot"] = json!("/Users/someone");
        for (h, ask) in [(HandHolding::AutoAcceptEdits, true), (HandHolding::Auto, true), (HandHolding::FullAccess, false)] {
            for lines in [&outside, &wider] {
                let mut s = session("01a0fe61-b5ad-7aa2-ade8-be4ef1741959", false);
                s.command(Command::SetHandHolding(h));
                let out = feed(&mut s, lines);
                assert_eq!(out.events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })), ask, "{h:?}");
                assert_eq!(out.send.is_empty(), ask, "{h:?}");
            }
        }
    }

    #[test]
    fn a_sub_agents_settled_request_goes() {
        let mut s = session("main", false);
        let ask = json!({"method":"item/commandExecution/requestApproval","id":7,"params":{"threadId":"kid","turnId":"k","itemId":"e1","command":"ls"}});
        assert!(matches!(&s.incoming(&ask).events[..], [AgentEvent::PermissionRequest { request_id, .. }] if request_id == "codex-7"));
        let resolved = json!({"method":"serverRequest/resolved","params":{"threadId":"kid","requestId":7}});
        assert_eq!(s.incoming(&resolved).events, vec![AgentEvent::PermissionResolved { request_id: "codex-7".into() }]);
        // Answering it now sends nothing.
        assert!(s.command(Command::Respond { request_id: "codex-7".into(), decision: Decision::Allow }).send.is_empty());
    }

    #[test]
    fn permission_requests_grant_what_was_asked() {
        let lines = fixture(include_str!("../fixtures/codex-permissions.jsonl"));
        let reply = |d| {
            let mut s = session("01a0fe60-583d-7771-b22d-9a4adf57fb28", false);
            let out = feed(&mut s, &lines);
            assert_eq!(
                out.events,
                vec![AgentEvent::PermissionRequest {
                    request_id: "codex-0".into(),
                    title: "Grant permissions".into(),
                    detail: "Network access\nThe requested workflow asks me to request network access.".into(),
                    prompt: None,
                }]
            );
            s.command(Command::Respond { request_id: "codex-0".into(), decision: d }).send.remove(0)
        };
        let asked = json!({"network":{"enabled":true},"fileSystem":null});
        assert_eq!(reply(Decision::Allow), json!({"id":0,"result":{"permissions":asked,"scope":"turn"}}));
        assert_eq!(reply(Decision::AllowForSession), json!({"id":0,"result":{"permissions":asked,"scope":"session"}}));
        assert_eq!(reply(Decision::Deny), json!({"id":0,"result":{"permissions":{}}}));
    }

    #[test]
    fn sub_agents_show_progress_on_their_row() {
        let lines = fixture(include_str!("../fixtures/codex-subagents.jsonl"));
        let main = "01a0fe4e-092b-70a0-9bf1-acaf9895b554";
        let child = "01a0fe4e-1f2d-7f20-a1e8-b5a58b5a5126";
        let mut s = session(main, false);
        let out = feed(&mut s, &lines);
        let ev = &out.events;
        assert_eq!(ev[0], AgentEvent::ToolStarted { id: child.into(), title: "Subagent".into(), detail: "pong agent".into() });
        assert_eq!(ev[1], AgentEvent::Task { id: child.into(), description: Some("pong agent".into()), activity: None, tool_uses: None, done: None });
        // Finished once, with its reply on the row.
        let done: Vec<&AgentEvent> = ev.iter().filter(|e| matches!(e, AgentEvent::Task { done: Some(_), .. })).collect();
        assert_eq!(done, vec![&AgentEvent::Task { id: child.into(), description: None, activity: None, tool_uses: None, done: Some(true) }]);
        assert!(ev.contains(&AgentEvent::ToolFinished { id: child.into(), output: "pong".into(), ok: true }));
        // The sub-agent's own text, context and turn stay off the main transcript.
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::TextDelta(_))).count(), 1);
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Context { used, .. } if *used > 23000 && *used < 23200)));
        assert_eq!(turn_completes(ev), 1);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Context { .. })));
    }

    #[test]
    fn sub_agent_commands_count_as_steps() {
        let mut s = session("main", false);
        let start = json!({"method":"item/started","params":{"threadId":"main","turnId":"t","item":{"type":"subAgentActivity","id":"c1","kind":"started","agentThreadId":"kid","agentPath":"/root/sleep_task"}}});
        let cmd = json!({"method":"item/started","params":{"threadId":"kid","turnId":"k","item":{"type":"commandExecution","id":"e1","command":"/bin/zsh -lc 'sleep 15'","cwd":"/tmp","status":"inProgress","commandActions":[]}}});
        let out = feed(&mut s, &[start, cmd]);
        assert_eq!(
            out.events[out.events.len() - 2..],
            [
                AgentEvent::Task { id: "kid".into(), description: None, activity: Some("Running sleep 15".into()), tool_uses: Some(1), done: None },
                AgentEvent::TaskStep { task: "kid".into(), title: "Run command".into(), detail: "sleep 15".into() },
            ]
        );
        assert!(!out.events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { id, .. } if id == "e1")));
    }

    #[test]
    fn steer_follows_the_running_turn_and_never_drops_a_message() {
        let lines = fixture(include_str!("../fixtures/codex-steer.jsonl"));
        let turn = "01a0fe4d-9675-7061-935d-d8b249eb41d6";
        let mut s = session("01a0fe4d-9485-79e2-8d9e-a4847ef074fc", false);
        let first = s.command(prompt("Run sleep 5")).send;
        assert_eq!((first[0]["id"].as_i64(), first[0]["method"].as_str()), (Some(3), Some("turn/start")));
        // Typed before the turn has an id: held, then steered into it.
        assert!(s.command(prompt("Also say BANANA")).send.is_empty());
        let out = feed(&mut s, &lines[..1]);
        assert_eq!(out.send[0]["method"], "turn/steer");
        assert_eq!(out.send[0]["params"]["expectedTurnId"], turn);
        assert_eq!(out.send[0]["params"]["input"][0]["text"], "Also say BANANA");
        assert_eq!(out.send[0]["id"], 4);
        // Codex refuses the steer before this turn's `turn/completed` is read (it clears the
        // active turn first): the message waits, then starts the next turn.
        let out = feed(&mut s, &lines[1..3]);
        assert!(out.send.is_empty() && out.events.is_empty());
        let out = feed(&mut s, &lines[4..5]);
        assert_eq!(turn_completes(&out.events), 1);
        assert_eq!(out.send[0]["method"], "turn/start");
        assert_eq!(out.send[0]["params"]["input"][0]["text"], "Also say BANANA");
        assert_eq!(s.turn, Turn::Starting);

        // The other order: the turn ended while the steer was in flight.
        let mut s = session("01a0fe4d-9485-79e2-8d9e-a4847ef074fc", false);
        s.next_id = 5;
        s.turn = Turn::Running(turn.into());
        assert_eq!(s.command(prompt("late")).send[0]["id"], 6);
        feed(&mut s, &lines[4..5]);
        let out = feed(&mut s, &lines[5..]);
        assert_eq!(out.send[0]["method"], "turn/start");
        assert_eq!(out.send[0]["params"]["input"][0]["text"], "late");
        assert!(out.events.is_empty());

        // A newer turn is already running: the refused steer goes into that one, once.
        let mut s = session("t", false);
        s.turn = Turn::Running("old".into());
        let id = s.command(prompt("more")).send[0]["id"].clone();
        s.turn = Turn::Running("new".into());
        let refused = |id: &Value| json!({"id":id,"error":{"code":-32600,"message":"expected active turn id `old` but found `new`"}});
        let out = s.incoming(&refused(&id));
        assert_eq!((out.send[0]["method"].as_str(), out.send[0]["params"]["expectedTurnId"].as_str()), (Some("turn/steer"), Some("new")));
        // Refused again: it starts the next turn rather than being lost.
        let out = s.incoming(&refused(&out.send[0]["id"]));
        assert!(out.send.is_empty() && out.events.is_empty());
        let out = s.incoming(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"new","status":"completed","items":[]}}}));
        assert_eq!(out.send[0]["params"]["input"][0]["text"], "more");
    }

    #[test]
    fn interrupt_waits_for_the_turn_id() {
        let mut s = session("t", false);
        s.command(prompt("go"));
        assert!(s.command(Command::Interrupt).send.is_empty());
        let out = s.incoming(&json!({"id":3,"result":{"turn":{"id":"turn-1","status":"inProgress","items":[]}}}));
        assert_eq!(out.send, vec![json!({"id":4,"method":"turn/interrupt","params":{"threadId":"t","turnId":"turn-1"}})]);
        let done = s.incoming(&json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"turn-1","status":"interrupted","items":[]}}}));
        assert_eq!(done.events, vec![AgentEvent::Mark("turn-1".into()), AgentEvent::TurnComplete { error: Some("Interrupted".into()) }]);
    }

    #[test]
    fn a_usage_limit_ends_the_turn_with_one_limit_event() {
        // Shapes per the app-server schema (codex-cli 0.160.0: `TurnError.codexErrorInfo`,
        // `account/rateLimits/updated`), in the envelope of a recorded failed turn; the message
        // as the CLI words it.
        let lines = fixture(include_str!("../fixtures/codex-limit.jsonl"));
        let mut s = session("01a0fe5b-ce3d-7420-b4af-d01f2ab43c6b", false);
        s.command(prompt("keep going"));
        let out = feed(&mut s, &lines);
        let message = lines[2]["params"]["error"]["message"].as_str().unwrap().to_string();
        // The reset comes from the used-up window of the snapshot, not the words. (The snapshot
        // itself says how full each window is, before the limit does.)
        let used = |scope, percent, at: i64| AgentEvent::LimitUsed { scope, percent, resets_at: Some(at) };
        assert_eq!(
            out.events,
            vec![
                used(crate::LimitScope::Session, 100.0, 1_791_007_800_000),
                used(crate::LimitScope::Weekly, 61.0, 1_791_065_371_000),
                AgentEvent::LimitReached { message: message.clone(), resets_at: Some(1_791_007_800_000), scope: crate::LimitScope::Session },
                AgentEvent::Mark("01a0fe5c-0a11-7c2e-9d41-5b7f0e2d1a90".into()),
                AgentEvent::TurnComplete { error: Some(message) },
            ]
        );
        // The next turn starts clean: an ordinary failure is no limit.
        s.command(prompt("again"));
        let other = fixture(include_str!("../fixtures/codex-turn-error.jsonl"));
        assert!(!feed(&mut s, &other).events.iter().any(|e| matches!(e, AgentEvent::LimitReached { .. })));
    }

    #[test]
    fn rate_limit_updates_are_merged_and_read() {
        let mut v = json!({"primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":10},"planType":"plus"});
        merge(&mut v, &json!({"primary":{"usedPercent":100},"secondary":{"usedPercent":100,"windowDurationMins":10080,"resetsAt":99},"planType":null}));
        assert_eq!(v["primary"], json!({"usedPercent":100,"windowDurationMins":300,"resetsAt":10}));
        assert_eq!(v["planType"], "plus");
        // Both windows used up: the later reset, the weekly one.
        let one = |v: Value| BTreeMap::from([(MAIN_LIMIT.to_string(), v)]);
        assert_eq!(limit_reset(&one(v), None), (Some(99_000), Some(crate::LimitScope::Weekly)));
        assert_eq!(limit_reset(&one(json!({"primary":{"usedPercent":40,"resetsAt":10}})), None), (None, None));
    }

    #[test]
    fn each_limit_keeps_its_own_snapshot() {
        // As the app-server reports them: the account's main limit, and a model's quota of its own.
        let update = |s: &mut Session, snapshot: Value| {
            let line = json!({"method":"account/rateLimits/updated","params":{"rateLimits":snapshot}});
            feed(s, &[line]);
        };
        let mut s = session("01a0fe5b-ce3d-7420-b4af-d01f2ab43c6b", false);
        update(&mut s, json!({"limitId":"codex","limitName":null,"primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":50},"secondary":{"usedPercent":30,"windowDurationMins":10080,"resetsAt":900}}));
        update(&mut s, json!({"limitId":"base_model_inference","limitName":"gpt-reserve","normalModelSlug":"gpt-5.6-luna","primary":{"usedPercent":100,"windowDurationMins":10080,"resetsAt":700}}));
        // A sparse update without an id is the main limit's, and leaves the other alone.
        update(&mut s, json!({"limitId":null,"primary":{"usedPercent":100}}));
        assert_eq!(s.rate_limits["codex"]["primary"], json!({"usedPercent":100,"windowDurationMins":300,"resetsAt":50}));
        assert_eq!(s.rate_limits["base_model_inference"]["limitName"], "gpt-reserve");
        assert!(s.rate_limits["codex"]["limitName"].is_null());
        // The main limit's reset, as the main limit, for any model but the one with a quota of its own.
        assert_eq!(limit_reset(&s.rate_limits, Some("gpt-5.6-sol")), (Some(50_000), Some(crate::LimitScope::Session)));
        assert_eq!(limit_reset(&s.rate_limits, Some("gpt-5.6-luna")), (Some(700_000), Some(crate::LimitScope::Model("gpt-reserve".into()))));
        // Main limit clear: the used-up one is the other's.
        update(&mut s, json!({"limitId":"codex","primary":{"usedPercent":10}}));
        assert_eq!(limit_reset(&s.rate_limits, None), (Some(700_000), Some(crate::LimitScope::Model("gpt-reserve".into()))));
    }

    #[test]
    fn rate_limit_updates_say_how_full_the_windows_are() {
        let update = |s: &mut Session, snapshot: Value| feed(s, &[json!({"method":"account/rateLimits/updated","params":{"rateLimits":snapshot}})]).events;
        let used = |scope, percent, at: i64| AgentEvent::LimitUsed { scope, percent, resets_at: Some(at) };
        let mut s = session("01a0fe5b-ce3d-7420-b4af-d01f2ab43c6b", false);
        let ev = update(&mut s, json!({"limitId":"codex","limitName":null,"primary":{"usedPercent":94,"windowDurationMins":300,"resetsAt":50},"secondary":{"usedPercent":30,"windowDurationMins":10080,"resetsAt":900}}));
        assert_eq!(ev, [used(crate::LimitScope::Session, 94.0, 50_000), used(crate::LimitScope::Weekly, 30.0, 900_000)]);
        // A sparse update: the window it's about is the one remembered.
        let ev = update(&mut s, json!({"limitId":null,"primary":{"usedPercent":96}}));
        assert_eq!(ev[0], used(crate::LimitScope::Session, 96.0, 50_000));
        // A model's own quota is news only to a session on that model.
        let reserve = json!({"limitId":"base_model_inference","limitName":"gpt-reserve","normalModelSlug":"gpt-5.6-luna","primary":{"usedPercent":99,"windowDurationMins":10080,"resetsAt":700}});
        s.model = Some("gpt-5.6-sol".into());
        assert!(update(&mut s, reserve.clone()).is_empty());
        s.model = Some("gpt-5.6-luna".into());
        assert_eq!(update(&mut s, reserve), [used(crate::LimitScope::Model("gpt-reserve".into()), 99.0, 700_000)]);
    }

    #[test]
    fn turn_errors_are_reported_once_and_readable() {
        let lines = fixture(include_str!("../fixtures/codex-turn-error.jsonl"));
        let mut s = session("01a0fe5b-ce3d-7420-b4af-d01f2ab43c6b", false);
        s.command(prompt("hi"));
        let out = feed(&mut s, &lines);
        assert_eq!(
            out.events,
            vec![AgentEvent::Mark("01a0fe5b-cec1-72d0-a342-0bdf8326949a".into()), AgentEvent::TurnComplete {
                error: Some("The 'gpt-nope-9' model is not supported when using Codex with a ChatGPT account.".into())
            }]
        );
        // With no turn to end, the error is reported on its own.
        let out = s.incoming(&lines[1]);
        assert!(matches!(&out.events[..], [AgentEvent::Error(e)] if e.starts_with("The 'gpt-nope-9' model")));
        // A refused turn/start ends that turn, failed, and the session can start the next one.
        let mut s = session("t", false);
        s.command(prompt("hi"));
        let out = s.incoming(&json!({"id":3,"error":{"code":-32600,"message":"thread not loaded"}}));
        assert_eq!(out.events, vec![AgentEvent::TurnComplete { error: Some("thread not loaded".into()) }]);
        assert_eq!(s.command(prompt("again")).send[0]["method"], "turn/start");
        // Messages sent while it was starting waited for it: the error names them.
        s.command(prompt("and this"));
        s.command(Command::Prompt { text: String::new(), images: vec![PathBuf::from("/tmp/a.png")] });
        let out = s.incoming(&json!({"id":4,"error":{"code":-32600,"message":"thread not loaded"}}));
        assert_eq!(
            out.events,
            vec![AgentEvent::TurnComplete { error: Some("thread not loaded\nNot sent to Codex: “and this”, an image".into()) }]
        );
        assert!(s.held.is_empty());
    }

    #[test]
    fn plan_updates_become_checklist_rows() {
        // Shape from the app-server schema (TurnPlanUpdatedNotification).
        let mut s = session("t", false);
        let out = s.incoming(&json!({"method":"turn/plan/updated","params":{"threadId":"t","turnId":"u","explanation":null,"plan":[
            {"step":"List files","status":"completed"},{"step":"Count files","status":"inProgress"},{"step":"Report","status":"pending"}]}}));
        assert_eq!(
            out.events,
            vec![
                AgentEvent::ToolStarted { id: "plan-u-1".into(), title: "Update plan".into(), detail: "Count files".into() },
                AgentEvent::ToolFinished { id: "plan-u-1".into(), output: "✓ List files\n→ Count files\n○ Report".into(), ok: true },
            ]
        );
    }

    #[test]
    fn unhandled_server_requests_get_an_answer() {
        let mut s = session("t", false);
        let out = s.incoming(&json!({"id":7,"method":"mcpServer/elicitation/request","params":{"threadId":"t"}}));
        assert_eq!(out.send, vec![json!({"id":7,"result":{"action":"decline"}})]);
        let out = s.incoming(&json!({"id":9,"method":"mcpServer/elicitation/request","params":{"threadId":"t","serverName":"trek-orchestrate"}}));
        assert_eq!(out.send, vec![json!({"id":9,"result":{"action":"accept","content":{}}})]);
        let out = s.incoming(&json!({"id":8,"method":"item/tool/call","params":{"threadId":"t"}}));
        assert_eq!(out.send[0]["error"]["code"], -32601);
        assert!(out.events.is_empty());
    }

    #[test]
    fn a_plan_offered_by_the_previous_session_can_still_be_approved() {
        // The app-server restarted while the plan card waited (plan mode toggled, or it quit);
        // the new session resumes the thread and was started with plan on. Leaving Plan mode is
        // said even if the resumed thread didn't report its mode.
        let config = SessionConfig {
            agent: AgentId::Codex,
            cwd: "/tmp".into(),
            model: Some("gpt-5.6-luna".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::Supervised,
            plan: true,
            read_only: false,
            resume: Some("t".into()),
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        };
        let opened = json!({"model":"gpt-5.6-luna"});
        let mut s = Session::new("t".into(), &config, &opened, 2);
        let go = s.command(Command::Respond { request_id: "codex-plan-turn-1".into(), decision: Decision::Allow }).send;
        assert_eq!(go[0]["params"]["collaborationMode"]["mode"], "default");
        assert_eq!(go[0]["params"]["input"][0]["text"], "Implement the plan.");
        // Keep planning needs nothing.
        let mut s = Session::new("t".into(), &config, &opened, 2);
        assert!(s.command(Command::Respond { request_id: "codex-plan-turn-1".into(), decision: Decision::Deny }).send.is_empty());
    }

    #[test]
    fn a_newer_message_answers_the_plan_instead() {
        let lines = fixture(include_str!("../fixtures/codex-plan-question.jsonl"));
        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        s.command(prompt("Plan it"));
        let out = feed(&mut s, &lines[2..]);
        let Some(AgentEvent::PermissionRequest { request_id, .. }) = out.events.last() else { panic!() };
        // The user wrote back instead of approving: a new Plan-mode turn.
        let next = s.command(prompt("Make it say hi instead")).send;
        assert_eq!(next[0]["params"]["collaborationMode"]["mode"], "plan");
        // The old card can't start the work any more (it would steer into the planning turn).
        assert!(s.command(Command::Respond { request_id: request_id.clone(), decision: Decision::Allow }).send.is_empty());
        assert!(s.plan);
    }

    #[test]
    fn requests_codex_settles_itself_lose_their_card() {
        let lines = fixture(include_str!("../fixtures/codex-plan-question.jsonl"));
        let mut s = session("01a0fe4b-3a5d-7cd2-bda4-95b0523e3695", true);
        let out = feed(&mut s, &lines[..3]);
        assert_eq!(out.events.last(), Some(&AgentEvent::PermissionResolved { request_id: "codex-0".into() }));
        // A late answer goes nowhere.
        assert!(s.command(Command::Answer { request_id: "codex-0".into(), answers: vec![("q".into(), "a".into())] }).send.is_empty());
        // Ones Trek answered itself are already settled: no event.
        let lines = fixture(include_str!("../fixtures/codex-approvals.jsonl"));
        let mut s = session("01a0fe4d-09c7-77c0-b896-912482b6fdd8", false);
        let asked = feed(&mut s, &lines[..5]);
        assert!(matches!(asked.events.last(), Some(AgentEvent::PermissionRequest { .. })));
        s.command(Command::Respond { request_id: "codex-0".into(), decision: Decision::Allow });
        assert!(feed(&mut s, &lines[5..6]).events.is_empty());
    }

    #[test]
    fn secret_and_free_text_questions() {
        // Shape from the app-server schema (ToolRequestUserInputParams).
        let mut s = session("t", false);
        let out = s.incoming(&json!({"id":4,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"u","itemId":"i","isBlocking":true,"questions":[
            {"id":"name","header":"Name","question":"What should the service be called?","options":null},
            {"id":"token","header":"Token","question":"Paste the deploy token","isSecret":true,"options":null}]}}));
        let AgentEvent::PermissionRequest { prompt: Some(Prompt::Questions(qs)), .. } = &out.events[0] else { panic!("{:?}", out.events) };
        assert_eq!(qs.iter().map(|q| (q.options.len(), q.secret)).collect::<Vec<_>>(), vec![(0, false), (0, true)]);
        let answer = s.command(Command::Answer { request_id: "codex-4".into(), answers: vec![(qs[1].question.clone(), "s3cr3t".into())] }).send;
        assert_eq!(answer, vec![json!({"id":4,"result":{"answers":{"token":{"answers":["s3cr3t"]}}}})]);
    }

    #[test]
    fn current_time_and_network_approvals() {
        let mut s = session("t", false);
        let out = s.incoming(&json!({"id":"r1","method":"currentTime/read","params":{"threadId":"t"}}));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(out.send[0]["id"], "r1");
        assert!(out.send[0]["result"]["currentTimeAt"].as_u64().is_some_and(|t| t.abs_diff(now) < 5));
        // Shape from CommandExecutionRequestApprovalParams.
        let out = s.incoming(&json!({"id":5,"method":"item/commandExecution/requestApproval","params":{"threadId":"t","turnId":"u","itemId":"e",
            "command":null,"networkApprovalContext":{"host":"pypi.org","protocol":"https"}}}));
        assert_eq!(out.events, vec![AgentEvent::PermissionRequest { request_id: "codex-5".into(), title: "Network access".into(), detail: "pypi.org".into(), prompt: None }]);
        let ok = s.command(Command::Respond { request_id: "codex-5".into(), decision: Decision::AllowForSession }).send;
        assert_eq!(ok, vec![json!({"id":5,"result":{"decision":"acceptForSession"}})]);
    }

    #[test]
    fn settings_updates_are_followed() {
        // Plan mode turned on outside Trek's control (shape from ThreadSettingsUpdatedNotification):
        // Trek's next turn, with plan mode off, says so.
        let mut s = session("t", false);
        s.incoming(&json!({"method":"thread/settings/updated","params":{"threadId":"t","threadSettings":{"collaborationMode":{"mode":"plan","settings":{"model":"gpt-5.6-luna"}},
            "approvalPolicy":"on-request","approvalsReviewer":"user","cwd":"/tmp","model":"gpt-5.6-luna","modelProvider":"openai","sandboxPolicy":{"type":"workspaceWrite"}}}}));
        assert_eq!(s.command(prompt("go")).send[0]["params"]["collaborationMode"]["mode"], "default");
    }

    #[test]
    fn image_views_are_read_rows() {
        let mut s = session("t", false);
        let item = json!({"type":"imageView","id":"img1","path":"/tmp/shot.png"});
        let out = feed(&mut s, &[
            json!({"method":"item/started","params":{"threadId":"t","turnId":"u","item":item}}),
            json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","item":item}}),
        ]);
        assert_eq!(
            out.events,
            vec![
                AgentEvent::ToolStarted { id: "img1".into(), title: "Read".into(), detail: "/tmp/shot.png".into() },
                AgentEvent::ToolFinished { id: "img1".into(), output: String::new(), ok: true },
            ]
        );
    }

    #[test]
    fn multi_agent_v1_spawns_show_on_the_spawn_row() {
        // Shapes from the app-server schema (collabAgentToolCall, CollabAgentState).
        let mut s = session("main", false);
        let spawn = |status: &str, states: Value| json!({"type":"collabAgentToolCall","id":"call1","tool":"spawnAgent","status":status,
            "senderThreadId":"main","receiverThreadIds":["kid"],"prompt":"Count the files","agentsStates":states});
        let out = feed(&mut s, &[
            json!({"method":"item/started","params":{"threadId":"main","turnId":"u","item":spawn("inProgress", json!({}))}}),
            json!({"method":"item/completed","params":{"threadId":"main","turnId":"u","item":spawn("completed", json!({"kid":{"status":"running","message":null}}))}}),
            json!({"method":"item/started","params":{"threadId":"kid","turnId":"k","item":{"type":"commandExecution","id":"e1","command":"ls","cwd":"/tmp","status":"inProgress","commandActions":[]}}}),
        ]);
        assert_eq!(
            out.events,
            vec![
                AgentEvent::ToolStarted { id: "call1".into(), title: "Subagent".into(), detail: "Count the files".into() },
                AgentEvent::Task { id: "call1".into(), description: Some("Count the files".into()), activity: None, tool_uses: None, done: None },
                AgentEvent::ToolFinished { id: "call1".into(), output: String::new(), ok: true },
                AgentEvent::Task { id: "call1".into(), description: None, activity: Some("Running ls".into()), tool_uses: Some(1), done: None },
                AgentEvent::TaskStep { task: "call1".into(), title: "Run command".into(), detail: "ls".into() },
            ]
        );
        // A later collab call (here `wait`) reports it finished, with its reply.
        let wait = json!({"type":"collabAgentToolCall","id":"call2","tool":"wait","status":"completed","senderThreadId":"main","receiverThreadIds":["kid"],
            "agentsStates":{"kid":{"status":"completed","message":"3 files"}}});
        let out = s.incoming(&json!({"method":"item/completed","params":{"threadId":"main","turnId":"u","item":wait}}));
        assert_eq!(
            out.events,
            vec![
                AgentEvent::Task { id: "call1".into(), description: None, activity: None, tool_uses: None, done: Some(true) },
                AgentEvent::ToolFinished { id: "call1".into(), output: "3 files".into(), ok: true },
            ]
        );
    }

    #[test]
    fn a_sub_agent_given_more_work_gets_a_new_row() {
        let mut s = session("main", false);
        let activity = |kind: &str| json!({"method":"item/completed","params":{"threadId":"main","turnId":"u","item":{"type":"subAgentActivity","id":format!("a-{kind}"),"kind":kind,"agentThreadId":"kid","agentPath":"/root/pong_agent"}}});
        feed(&mut s, &[activity("started"), activity("completed")]);
        let out = feed(&mut s, &[activity("interacted")]);
        assert_eq!(
            out.events,
            vec![
                AgentEvent::ToolStarted { id: "kid-2".into(), title: "Subagent".into(), detail: "pong agent".into() },
                AgentEvent::Task { id: "kid-2".into(), description: Some("pong agent".into()), activity: None, tool_uses: None, done: None },
            ]
        );
        let out = feed(&mut s, &[activity("completed")]);
        assert_eq!(out.events[0], AgentEvent::Task { id: "kid-2".into(), description: None, activity: None, tool_uses: None, done: Some(true) });
        // While it's still running, more work is the same run.
        feed(&mut s, &[activity("interacted")]);
        assert!(feed(&mut s, &[activity("interacted")]).events.is_empty());
    }

    #[test]
    fn a_turn_reports_the_tokens_it_and_its_sub_agents_used() {
        // A real turn: the main thread reports three times (running totals), its sub-agent once.
        let lines = fixture(include_str!("../fixtures/codex-subagents.jsonl"));
        let mut s = session("01a0fe4e-092b-70a0-9bf1-acaf9895b554", false);
        let out = feed(&mut s, &lines);
        let used: Vec<&AgentEvent> = out.events.iter().filter(|e| matches!(e, AgentEvent::Usage { .. })).collect();
        let tokens = TokenUsage { input: (51982 - 28928) + (23158 - 6912), output: 90 + 5, cache_read: 28928 + 6912, cache_write: 0 };
        // Priced request by request; none was long, so it's the turn's tokens at standard rates.
        let (luna, _) = trek_core::pricing::price("gpt-5.6-luna", &AgentId::Codex).unwrap();
        let [AgentEvent::Usage { model, tokens: got, cost: Some(cost) }] = used[..] else { panic!("{used:?}") };
        assert_eq!((model.as_deref(), *got), (Some("gpt-5.6-luna"), tokens));
        assert!((cost.usd - luna.rates.cost(&tokens, 0)).abs() < 1e-12 && !cost.reported);
        // Reported as the turn ends, just before it.
        let at = out.events.iter().position(|e| matches!(e, AgentEvent::Usage { .. })).unwrap();
        assert!(matches!(out.events[at + 1], AgentEvent::TurnComplete { .. }));
        // A background sub-agent reporting between turns is counted at once.
        let late = json!({"method":"thread/tokenUsage/updated","params":{"threadId":"01a0fe4e-1f2d-7f20-a1e8-b5a58b5a5126","tokenUsage":{"total":{"inputTokens":24158,"cachedInputTokens":6912,"outputTokens":15},"last":{},"modelContextWindow":258400}}});
        let out = feed(&mut s, &[late]);
        // 1,000 × $0.20 + 10 × $1.20, per million.
        assert_eq!(
            out.events,
            vec![AgentEvent::Usage { model: Some("gpt-5.6-luna".into()), tokens: TokenUsage { input: 1000, output: 10, cache_read: 0, cache_write: 0 }, cost: Some(UsageCost::priced(0.000212)) }]
        );
    }

    #[test]
    fn fast_requests_cost_what_fast_processing_does() {
        let mut s = session("main", false);
        s.model = Some("gpt-5.6-luna".into());
        s.fast = Some("priority".into());
        let report = json!({"method":"thread/tokenUsage/updated","params":{"threadId":"main","tokenUsage":{"total":{"inputTokens":1000,"cachedInputTokens":0,"outputTokens":10},"last":{"inputTokens":1000,"cachedInputTokens":0,"outputTokens":10}}}});
        let out = feed(&mut s, &[report]);
        // Between turns it's reported at once: twice (1,000 × $0.20 + 10 × $1.20) per million.
        let [AgentEvent::Usage { cost: Some(c), .. }] = &out.events[..] else { panic!("{:?}", out.events) };
        assert!((c.usd - 2.0 * 0.000212).abs() < 1e-12);
    }

    #[test]
    fn a_sub_agent_on_another_model_counts_toward_it() {
        let mut s = session("main", false);
        s.model = Some("gpt-5.6-luna".into());
        let spawn = json!({"type":"collabAgentToolCall","id":"call1","tool":"spawnAgent","status":"completed","senderThreadId":"main",
            "receiverThreadIds":["kid"],"prompt":"Count the files","model":"gpt-5.6-mini","agentsStates":{"kid":{"status":"running","message":null}}});
        let tokens = |thread: &str, input: u64| json!({"method":"thread/tokenUsage/updated","params":{"threadId":thread,"tokenUsage":{"total":{"inputTokens":input,"cachedInputTokens":0,"outputTokens":5},"last":{"inputTokens":input,"cachedInputTokens":0,"outputTokens":5}}}});
        let out = feed(&mut s, &[
            json!({"method":"turn/started","params":{"threadId":"main","turn":{"id":"u","items":[],"status":"inProgress"}}}),
            json!({"method":"item/completed","params":{"threadId":"main","turnId":"u","item":spawn}}),
            tokens("kid", 300),
            tokens("main", 1000),
            json!({"method":"turn/completed","params":{"threadId":"main","turn":{"id":"u","items":[],"status":"completed","error":null}}}),
        ]);
        let used: Vec<(Option<String>, u64)> = out.events.iter().filter_map(|e| if let AgentEvent::Usage { model, tokens, .. } = e { Some((model.clone(), tokens.input)) } else { None }).collect();
        assert_eq!(used, vec![(Some("gpt-5.6-mini".into()), 300), (Some("gpt-5.6-luna".into()), 1000)]);
    }

    #[test]
    fn commands_that_outlive_their_turn_run_in_the_background() {
        // Recorded (Codex 0.160.0, gpt-5.6-luna): a command started in a background terminal; the
        // turn ends while it runs, and it completes on its own later.
        let mut s = session("t", false);
        let command = "/bin/zsh -lc 'for i in 1 2 3; do sleep 5; echo tick $i; done'";
        let item = |method: &str, status: &str, output: Value| {
            let exit = if output.is_null() { Value::Null } else { json!(0) };
            json!({"method":method,"params":{"threadId":"t","turnId":"u","item":{"type":"commandExecution","id":"exec-1","command":command,"cwd":"/tmp","processId":"46444","source":"unifiedExecStartup","status":status,"commandActions":[],"aggregatedOutput":output,"exitCode":exit}}})
        };
        let delta = |d: &str| json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"t","turnId":"u","itemId":"exec-1","delta":d}});
        feed(&mut s, &[json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u"}}}), item("item/started", "inProgress", Value::Null), delta("tick 1\r\n")]);
        let out = feed(&mut s, &[json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"completed","items":[]}}})]);
        let Some(AgentEvent::Background(b)) = out.events.get(out.events.len() - 2) else { panic!("{:?}", out.events) };
        assert!(matches!(out.events.last(), Some(AgentEvent::TurnComplete { error: None })));
        let title = command_text(&json!({ "command": command }));
        assert_eq!(b[..], [crate::BackgroundTask { id: "exec-1".into(), kind: crate::BackgroundKind::Shell, title, call: Some("exec-1".into()), readable: true, stoppable: true }]);
        // Its output so far can be read while it runs, without a turn.
        feed(&mut s, &[delta("tick 2\r\n")]);
        let read = s.command(Command::ReadTask { id: "exec-1".into() });
        assert_eq!(read.events, [AgentEvent::TaskOutput { id: "exec-1".into(), output: "tick 1\r\ntick 2\r\n".into() }]);
        assert!(read.send.is_empty(), "nothing asked of Codex");
        // It ends: off the list, and its row gets its output.
        let out = feed(&mut s, &[item("item/completed", "completed", json!("tick 1\r\ntick 2\r\ntick 3\r\n"))]);
        assert_eq!(out.events[0], AgentEvent::Background(vec![]));
        assert!(matches!(&out.events[1], AgentEvent::ToolFinished { id, ok: true, .. } if id == "exec-1"));
        assert!(s.commands.is_empty() && s.background.is_empty());
    }

    #[test]
    fn a_windows_sandbox_without_its_setup_is_explained_once() {
        // Recorded (Codex 0.162.0, Windows, Auto): every command fails before it starts.
        let said = "Failed to create unified exec process: helper_unknown_error: setup refresh had errors";
        let done = |id: &str, status: &str, output: &str| {
            json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","item":{"type":"commandExecution","id":id,"command":"Get-Content -Raw -LiteralPath .\\hello.txt","cwd":"C:\\p","status":status,"commandActions":[],"aggregatedOutput":output,"exitCode":-1}}})
        };
        let notices = |out: &Out| out.events.iter().filter(|e| matches!(e, AgentEvent::Notice(_))).count();
        let mut s = session("t", false);
        s.windows = true;
        let out = feed(&mut s, &[done("e1", "failed", said)]);
        assert!(matches!(&out.events[..], [AgentEvent::ToolFinished { ok: false, .. }, AgentEvent::Notice(n)] if n == SANDBOX_SETUP_NOTICE && n.contains("one-time setup") && n.contains("Full access")), "{:?}", out.events);
        // Once per session, however many commands hit it.
        let out = feed(&mut s, &[done("e2", "failed", said)]);
        assert!(matches!(&out.events[..], [AgentEvent::ToolFinished { ok: false, .. }]), "{:?}", out.events);
        // Narrowly: other failures, and other output, are Codex's own business.
        let mut s = session("t", false);
        s.windows = true;
        let out = feed(&mut s, &[done("e3", "failed", "command not found"), done("e4", "completed", "setup refresh had errors"), done("e5", "declined", "")]);
        assert_eq!(notices(&out), 0, "{:?}", out.events);
        // Not the same words from another item, nor on macOS and Linux.
        assert!(!sandbox_not_set_up(&json!({"type":"agentMessage","status":"failed","aggregatedOutput":said}), true));
        let item = done("e6", "failed", said)["params"]["item"].clone();
        assert!(sandbox_not_set_up(&item, true) && !sandbox_not_set_up(&item, false));
        let mut s = session("t", false);
        s.windows = false;
        assert_eq!(notices(&feed(&mut s, &[done("e7", "failed", said)])), 0);
    }

    #[test]
    fn a_background_command_is_stopped_through_its_terminal() {
        let mut s = session("t", false);
        let started = json!({"method":"item/started","params":{"threadId":"t","turnId":"u","item":{"type":"commandExecution","id":"exec-1","command":"npm run dev","cwd":"/tmp","processId":"46444","source":"unifiedExecStartup","status":"inProgress","commandActions":[]}}});
        let delta = json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"t","turnId":"u","itemId":"exec-1","delta":"ready on :5173\n"}});
        feed(&mut s, &[json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u"}}}), started, delta]);
        feed(&mut s, &[json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"completed","items":[]}}})]);
        let stop = s.command(Command::StopTask { id: "exec-1".into() });
        let [req] = &stop.send[..] else { panic!("{:?}", stop.send) };
        assert_eq!(req["method"], "thread/backgroundTerminals/terminate");
        assert_eq!(req["params"], json!({ "threadId": "t", "processId": "46444" }));
        // Codex says it's stopped: off the list, its row ends with the output it had.
        let out = feed(&mut s, &[json!({ "id": req["id"], "result": { "terminated": true } })]);
        assert_eq!(out.events[0], AgentEvent::Background(vec![]));
        assert_eq!(out.events[1], AgentEvent::ToolFinished { id: "exec-1".into(), output: "ready on :5173\n".into(), ok: false });
        assert!(s.commands.is_empty() && s.processes.is_empty());
        // Its completion, if Codex reports one after, finds nothing left to take off.
        let done = json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","item":{"type":"commandExecution","id":"exec-1","command":"npm run dev","cwd":"/tmp","processId":"46444","status":"failed","commandActions":[],"aggregatedOutput":"ready on :5173\n","exitCode":143}}});
        assert!(!feed(&mut s, &[done]).events.iter().any(|e| matches!(e, AgentEvent::Background(_))));
        // One that had ended by then: Codex says so, and Trek says it couldn't.
        let mut s = session("t", false);
        s.processes.insert("exec-2".into(), "7".into());
        let stop = s.command(Command::StopTask { id: "exec-2".into() });
        let out = feed(&mut s, &[json!({ "id": stop.send[0]["id"], "result": { "terminated": false } })]);
        assert!(matches!(&out.events[..], [AgentEvent::Notice(n)] if n.contains("couldn't stop")), "{:?}", out.events);
    }

    #[test]
    fn a_sub_agent_s_tool_calls_are_its_steps() {
        let mut s = session("t", false);
        s.agents.insert("kid".into(), SubAgent { row: "kid".into(), runs: 1, tool_uses: 0, last: String::new(), done: false, model: None });
        let out = feed(&mut s, &[json!({"method":"item/started","params":{"threadId":"kid","turnId":"k","item":{"type":"commandExecution","id":"e1","command":"ls","cwd":"/tmp","status":"inProgress","commandActions":[]}}})]);
        assert!(out.events.contains(&AgentEvent::TaskStep { task: "kid".into(), title: "Run command".into(), detail: "ls".into() }), "{:?}", out.events);
        // The sub-agent's commands aren't the session's: they never run in its background.
        assert!(s.commands.is_empty());
    }
}

/// Against the real Codex: a message sent mid-turn steers the running turn. Costs a few cents of
/// the user's plan, so it only runs on request:
/// `cargo test -p trek-agents codex_live -- --ignored --nocapture` (Codex must be signed in;
/// `TREK_LIVE_CODEX_ACCESS=full` runs it without Codex's sandbox).
#[cfg(test)]
mod live {
    use crate::{AgentEvent, Command, SessionConfig, start};
    use std::time::Duration;
    use trek_core::{AgentId, Effort, HandHolding};

    #[test]
    #[ignore = "talks to the real Codex"]
    fn codex_live_steer_joins_the_running_turn() {
        let cwd = std::env::temp_dir().join("trek-verify-e2e");
        std::fs::create_dir_all(&cwd).unwrap();
        let session = start(SessionConfig {
            agent: AgentId::Codex,
            cwd,
            model: Some(std::env::var("TREK_LIVE_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.6-luna".into())),
            effort: Effort::Low,
            // Codex's sandbox on Windows needs its own one-time setup; where that hasn't been done
            // every command fails before it starts ("setup refresh had errors"). `full` skips it.
            hand_holding: if std::env::var("TREK_LIVE_CODEX_ACCESS").as_deref() == Ok("full") { HandHolding::FullAccess } else { HandHolding::Auto },
            plan: false,
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        });
        let events = trek_core::runtime().block_on(async {
            let prompt = |text: &str| Command::Prompt { text: text.into(), images: vec![] };
            session.commands.send(prompt("Run the shell command `sleep 6` and then reply with exactly the word: done")).await.unwrap();
            let mut seen = vec![];
            let mut steered = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            loop {
                let ev = tokio::time::timeout_at(deadline, session.events.recv()).await.expect("in time").expect("open");
                println!("{ev:?}");
                // Once the command is running, steer.
                if !steered && matches!(ev, AgentEvent::ToolStarted { .. }) {
                    steered = true;
                    session.commands.send(prompt("After that, also add the word banana on its own line.")).await.unwrap();
                }
                let done = matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Error(_) | AgentEvent::Exited);
                seen.push(ev);
                if done {
                    break;
                }
            }
            let _ = session.commands.send(Command::Shutdown).await;
            seen
        });
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { .. })), "the command ran");
        let turns: Vec<&AgentEvent> = events.iter().filter(|e| matches!(e, AgentEvent::TurnComplete { .. })).collect();
        assert!(matches!(turns[..], [AgentEvent::TurnComplete { error: None, .. }]), "one turn, finished: {turns:?}");
        let said: String = events.iter().filter_map(|e| if let AgentEvent::TextDone(t) = e { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n");
        assert!(said.to_lowercase().contains("banana"), "the steer reached the running turn: {said}");
    }
}
