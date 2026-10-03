//! Claude Code via the user's own `claude` binary (stream-json + stdio control protocol).
//! Trek never reads Claude credentials; the CLI handles its own login.

use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, StderrTail, Step, clip, load_image, mcp_servers_json, plan_row, plan_title};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trek_core::{Effort, detect};

fn tool_title(name: &str, input: &Value) -> (String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match name {
        "Bash" => ("Run command".into(), s("command")),
        "Read" => ("Read".into(), s("file_path")),
        "Edit" | "MultiEdit" => ("Edit".into(), s("file_path")),
        "Write" => ("Write".into(), s("file_path")),
        "Grep" => ("Search".into(), s("pattern")),
        "Glob" => ("List files".into(), s("pattern")),
        "WebFetch" => ("Fetch".into(), s("url")),
        "WebSearch" => ("Search the web".into(), s("query")),
        "Agent" | "Task" => ("Subagent".into(), s("description")),
        "TodoWrite" => ("Update plan".into(), todo_detail(input)),
        "ExitPlanMode" => ("Plan".into(), plan_title(input["plan"].as_str().unwrap_or_default())),
        "AskUserQuestion" => ("Question".into(), input["questions"][0]["question"].as_str().unwrap_or_default().to_string()),
        other => (other.to_string(), clip(&input.to_string(), 200)),
    }
}

/// The step a TodoWrite call is on, like the plan rows of the other agents.
fn todo_detail(input: &Value) -> String {
    let steps: Vec<(String, Step)> = input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let step = match t["status"].as_str() {
                Some("completed") => Step::Done,
                Some("in_progress") => Step::Active,
                _ => Step::Pending,
            };
            (t["activeForm"].as_str().filter(|_| step == Step::Active).or(t["content"].as_str()).unwrap_or_default().to_string(), step)
        })
        .collect();
    if steps.is_empty() { String::new() } else { plan_row(&steps).0 }
}

/// Numbered control requests to the CLI.
struct Control {
    next_id: u64,
}

impl Control {
    fn request(&mut self, subtype: &str, extra: Value) -> Value {
        self.next_id += 1;
        let mut request = json!({ "subtype": subtype });
        if let (Some(r), Some(e)) = (request.as_object_mut(), extra.as_object()) {
            r.extend(e.clone());
        }
        json!({ "type": "control_request", "request_id": format!("trek-{}", self.next_id), "request": request })
    }
}

fn control_response(request_id: &str, response: Value) -> Value {
    json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": response } })
}

/// What Claude is told when the user says no, per tool.
fn deny_message(tool: &str) -> &'static str {
    match tool {
        "ExitPlanMode" => {
            "The user isn't ready to approve this plan and wants to keep planning. Stay in plan mode, don't change anything yet, and ask what they'd like to change."
        }
        "AskUserQuestion" => "The user skipped these questions. Carry on with your best judgment, or ask in plain words if you're truly stuck.",
        _ => "The user declined this action.",
    }
}

/// Messages answering the permission prompt `request`. Approving a plan also leaves plan mode
/// for `mode` (the thread's own level): on its own, Claude would drop to "default".
fn respond(ctl: &mut Control, request_id: &str, request: &Value, decision: Decision, mode: &str) -> Vec<Value> {
    let tool = request["tool_name"].as_str().unwrap_or_default();
    let input = request["input"].clone();
    let response = match decision {
        Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
        Decision::AllowForSession => {
            let mut ok = json!({ "behavior": "allow", "updatedInput": input });
            // Claude's own suggested rules (e.g. allow `gh issue list:*` in this
            // session); applying them stops the same prompt from coming back.
            if let Some(sug) = request["permission_suggestions"].as_array().filter(|a| !a.is_empty()) {
                ok["updatedPermissions"] = Value::Array(sug.clone());
            }
            ok
        }
        Decision::Deny => json!({ "behavior": "deny", "message": deny_message(tool) }),
    };
    let mut out = vec![control_response(request_id, response)];
    if tool == "ExitPlanMode" && decision != Decision::Deny {
        out.push(ctl.request("set_permission_mode", json!({ "mode": mode })));
    }
    out
}

/// Answers to an AskUserQuestion prompt: `(question, chosen labels or the user's own words)`.
fn answer(request_id: &str, request: &Value, answers: Vec<(String, String)>) -> Value {
    let mut input = request["input"].clone();
    // AskUserQuestion reads its answers from the input it gets back.
    input["answers"] = Value::Object(answers.into_iter().map(|(q, a)| (q, Value::String(a))).collect());
    control_response(request_id, json!({ "behavior": "allow", "updatedInput": input }))
}

/// The CLI's arguments for a session (MCP servers aside).
fn cli_args(config: &SessionConfig) -> Vec<String> {
    let mode = if config.plan { "plan" } else { config.hand_holding.claude_mode() };
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        mode,
        // Lets Trek switch a running session to Full access; it doesn't change the starting mode.
        "--allow-dangerously-skip-permissions",
    ]
    .map(String::from)
    .to_vec();
    let flag = |args: &mut Vec<String>, flag: &str, value: &str| args.extend([flag.to_string(), value.to_string()]);
    if let Some(model) = &config.model {
        flag(&mut args, "--model", model);
    }
    if config.effort != Effort::Off {
        flag(&mut args, "--effort", config.effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]).as_str());
    }
    if let Some(id) = &config.resume {
        flag(&mut args, "--resume", id);
        // Keeps the conversation up to that message; what came after it is gone for the model.
        if let Some(at) = &config.resume_at {
            flag(&mut args, "--resume-session-at", at);
        }
        if config.fork {
            args.push("--fork-session".into());
        }
    }
    if config.fast.is_some() {
        flag(&mut args, "--settings", r#"{"fastMode":true}"#);
    }
    args
}

/// A session to cut back that isn't on disk with that message (deleted, or the message is from
/// a session Claude Code no longer has) starts afresh instead, with the recap.
async fn check_resume_point(mut config: SessionConfig) -> (SessionConfig, Option<String>) {
    let (Some(id), Some(at)) = (config.resume.clone(), config.resume_at.clone()) else { return (config, None) };
    let found = tokio::task::spawn_blocking(move || trek_core::import::claude::has_message(&id, &at)).await.unwrap_or(false);
    if found {
        return (config, None);
    }
    tracing::warn!("claude: session {:?} has no message {:?} to resume at; starting afresh", config.resume, config.resume_at);
    config.resume = None;
    config.resume_at = None;
    config.fork = false;
    let recap = config.recap.take();
    (config, recap)
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let bin = detect::which("claude").context("Claude Code isn't installed (npm i -g @anthropic-ai/claude-code)")?;
    let (config, mut recap) = check_resume_point(config).await;
    if recap.is_some() {
        let _ = events
            .send(AgentEvent::Notice("Claude Code couldn't take its session back to that point, so it continues in a new session with a recap of this conversation.".into()))
            .await;
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(cli_args(&config));
    // Lives as long as the session; removed on drop.
    let _mcp_file = if config.mcp_servers.is_empty() {
        None
    } else {
        let file = TempFile::write(
            "mcp",
            &serde_json::to_string(&json!({ "mcpServers": mcp_servers_json(&config.mcp_servers) }))?,
        )?;
        cmd.arg("--mcp-config").arg(&file.0);
        Some(file)
    };
    cmd.current_dir(&config.cwd)
        .env("PATH", detect::login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("failed to start claude")?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr = StderrTail::capture(child.stderr.take().unwrap(), "claude");

    let mut ctl = Control { next_id: 0 };
    let init = ctl.request("initialize", json!({}));
    let init_id = init["request_id"].as_str().unwrap_or_default().to_string();
    write_line(&mut stdin, &init).await?;
    // Outstanding `get_context_usage` requests; their responses become `Context` events.
    let mut context_requests: HashSet<String> = HashSet::new();

    // Inputs of pending permission requests, echoed back as `updatedInput` on allow.
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut streamed_text = false;
    let mut hand_holding = config.hand_holding;
    // In plan mode (as Claude last reported it): access changes wait until the plan is approved.
    let mut planning = config.plan;
    let mut in_turn = false;
    // Resumed partway: once the session has said which it is, that message is its latest point.
    let mut resumed_at = config.resume_at.clone();

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt { text, images } => {
                        streamed_text = false;
                        in_turn = true;
                        let text = match recap.take() {
                            Some(r) => crate::recap_prompt(&r, &text),
                            None => text,
                        };
                        let (content, errors) = user_content(&text, &images);
                        for e in errors {
                            let _ = events.send(AgentEvent::Error(e)).await;
                        }
                        let msg = json!({
                            "type": "user", "session_id": "",
                            "message": { "role": "user", "content": content },
                            "parent_tool_use_id": null
                        });
                        write_line(&mut stdin, &msg).await?;
                    }
                    Command::Interrupt => write_line(&mut stdin, &ctl.request("interrupt", json!({}))).await?,
                    Command::SetHandHolding(h) => {
                        hand_holding = h;
                        if !planning {
                            write_line(&mut stdin, &ctl.request("set_permission_mode", json!({ "mode": h.claude_mode() }))).await?
                        }
                    }
                    Command::SetModel { model, .. } => {
                        write_line(&mut stdin, &ctl.request("set_model", json!({ "model": model }))).await?
                    }
                    Command::Respond { request_id, decision } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
                        for msg in respond(&mut ctl, &request_id, &request, decision, hand_holding.claude_mode()) {
                            write_line(&mut stdin, &msg).await?;
                        }
                        if request["tool_name"] == "ExitPlanMode" && decision != Decision::Deny {
                            planning = false;
                        }
                    }
                    Command::Answer { request_id, answers } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
                        write_line(&mut stdin, &answer(&request_id, &request, answers)).await?;
                    }
                    Command::Shutdown => break,
                }
            }
            line = stdout.next_line() => {
                let Some(line) = line? else {
                    if in_turn {
                        return Err(stderr.exited("Claude Code"));
                    }
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(mode) = permission_mode(&v) {
                    planning = mode == "plan";
                }
                if v["type"] == "control_response" {
                    let r = &v["response"];
                    let id = r["request_id"].as_str().unwrap_or_default();
                    if id == init_id {
                        if let Some(b) = account_billing(&r["response"]["account"]) {
                            if events.send(AgentEvent::Billing(b)).await.is_err() {
                                return Ok(());
                            }
                        }
                        // Ask for context usage up front so the UI has data before the first prompt.
                        let c = ctl.request("get_context_usage", json!({}));
                        context_requests.insert(c["request_id"].as_str().unwrap_or_default().to_string());
                        write_line(&mut stdin, &c).await?;
                    } else if context_requests.remove(id) {
                        if let Some(ev) = context_event(r) {
                            if events.send(ev).await.is_err() {
                                return Ok(());
                            }
                        }
                    } else if r["subtype"] == "error" {
                        let msg = r["error"].as_str().unwrap_or("Claude Code rejected the change.").to_string();
                        tracing::warn!("claude control request {id} failed: {msg}");
                        if events.send(AgentEvent::Error(msg)).await.is_err() {
                            return Ok(());
                        }
                    }
                    continue;
                }
                for ev in translate(&v, &mut pending, &mut streamed_text) {
                    let started = matches!(ev, AgentEvent::Started { .. });
                    if events.send(ev).await.is_err() {
                        return Ok(());
                    }
                    if let Some(at) = resumed_at.take_if(|_| started) {
                        let _ = events.send(AgentEvent::Mark(at)).await;
                    }
                }
                if v["type"] == "result" {
                    in_turn = false;
                    let c = ctl.request("get_context_usage", json!({}));
                    context_requests.insert(c["request_id"].as_str().unwrap_or_default().to_string());
                    write_line(&mut stdin, &c).await?;
                }
            }
        }
    }
    let _ = child.start_kill();
    Ok(())
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, v: &Value) -> Result<()> {
    let mut s = serde_json::to_string(v)?;
    s.push('\n');
    stdin.write_all(s.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Message content for a prompt: image blocks first, then the text. Unreadable images are
/// skipped and reported.
fn user_content(text: &str, images: &[PathBuf]) -> (Vec<Value>, Vec<String>) {
    let mut content = Vec::new();
    let mut errors = Vec::new();
    for path in images {
        match load_image(path) {
            Ok((media_type, data)) => content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": media_type, "data": data }
            })),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    content.push(json!({ "type": "text", "text": text }));
    (content, errors)
}

/// Why a turn failed: its result text, else the CLI's own errors (e.g. a session that can't be
/// resumed).
fn result_error(v: &Value) -> String {
    // A stopped turn (Trek's interrupt) ends with an internal diagnostic as its only "error".
    if matches!(v["terminal_reason"].as_str(), Some("aborted_streaming" | "aborted_tools")) {
        return "Interrupted".to_string();
    }
    let errors: Vec<&str> = v["errors"].as_array().into_iter().flatten().filter_map(|e| e.as_str()).filter(|e| !e.starts_with("[ede_diagnostic]")).collect();
    v["result"]
        .as_str()
        .filter(|r| !r.trim().is_empty())
        .map(String::from)
        .or_else(|| (!errors.is_empty()).then(|| errors.join("\n")))
        .unwrap_or_else(|| "The turn failed.".to_string())
}

/// The permission mode Claude reports in its `init` and `status` messages.
fn permission_mode(v: &Value) -> Option<&str> {
    (v["type"] == "system" && matches!(v["subtype"].as_str(), Some("init" | "status"))).then(|| v["permissionMode"].as_str()).flatten()
}

/// How the login is billed, from the `account` in the `initialize` response; `None` when it
/// doesn't say. Claude Code names the subscription only while its login is the one in use, so a
/// subscription wins over a key that's merely present. Without one, a key is what pays.
fn account_billing(account: &Value) -> Option<Billing> {
    if let Some(plan) = account["subscriptionType"].as_str().filter(|s| !s.is_empty()) {
        let name = if plan.starts_with("Claude") { plan.to_string() } else { format!("Claude {}", crate::status::capitalize(plan)) };
        return Some(Billing::Plan(Some(name)));
    }
    // Bedrock, Vertex, Foundry and gateways bill the cloud account per token.
    if account["apiProvider"].as_str().is_some_and(|p| p != "firstParty") || account["apiKeySource"].is_string() {
        return Some(Billing::Metered);
    }
    match account["tokenSource"].as_str()? {
        // A bearer token for a proxy or gateway (ANTHROPIC_AUTH_TOKEN), or a key from a helper script.
        "ANTHROPIC_AUTH_TOKEN" | "apiKeyHelper" => Some(Billing::Metered),
        // `claude setup-token` and hosted sessions: a subscription login whose plan isn't named.
        "CLAUDE_CODE_OAUTH_TOKEN" | "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR" | "CCR_OAUTH_TOKEN_FILE" => Some(Billing::Plan(None)),
        _ => None,
    }
}

/// `Context` from a `get_context_usage` control response (`{subtype, request_id, response}`).
fn context_event(r: &Value) -> Option<AgentEvent> {
    if r["subtype"] != "success" {
        return None;
    }
    let body = &r["response"];
    Some(AgentEvent::Context { used: body["totalTokens"].as_u64()?, window: body["maxTokens"].as_u64()? })
}

/// A temp file removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn write(tag: &str, contents: &str) -> Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("trek-{tag}-{}-{nanos}.json", std::process::id()));
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A message of the conversation is a point `--resume-session-at` can cut it back to.
fn mark(v: &Value) -> Option<AgentEvent> {
    v["uuid"].as_str().filter(|u| !u.is_empty()).map(|u| AgentEvent::Mark(u.to_string()))
}

fn translate(v: &Value, pending: &mut HashMap<String, Value>, streamed_text: &mut bool) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            out.push(AgentEvent::Started {
                native_id: v["session_id"].as_str().unwrap_or_default().to_string(),
                model: v["model"].as_str().map(String::from),
            });
        }
        Some("system") if v["subtype"] == "task_started" && v["task_type"] == "local_agent" => out.push(AgentEvent::Task {
            id: v["tool_use_id"].as_str().unwrap_or_default().to_string(),
            description: v["description"].as_str().map(String::from),
            activity: None,
            tool_uses: None,
            done: None,
        }),
        Some("system") if v["subtype"] == "task_progress" => out.push(AgentEvent::Task {
            id: v["tool_use_id"].as_str().unwrap_or_default().to_string(),
            description: None,
            activity: v["description"].as_str().map(String::from),
            tool_uses: v["usage"]["tool_uses"].as_u64(),
            done: None,
        }),
        Some("system") if v["subtype"] == "task_notification" => out.push(AgentEvent::Task {
            id: v["tool_use_id"].as_str().unwrap_or_default().to_string(),
            description: None,
            activity: None,
            tool_uses: None,
            done: Some(v["status"] == "completed"),
        }),
        Some("system") if v["subtype"] == "background_tasks_changed" => out.push(AgentEvent::Background(
            v["tasks"].as_array().map(|t| t.iter().filter(|t| t["task_type"] == "local_agent").count()).unwrap_or(0),
        )),
        Some("stream_event") => {
            let e = &v["event"];
            if e["type"] == "content_block_delta" && v["parent_tool_use_id"].is_null() {
                match e["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        *streamed_text = true;
                        out.push(AgentEvent::TextDelta(e["delta"]["text"].as_str().unwrap_or_default().into()));
                    }
                    Some("thinking_delta") => {
                        out.push(AgentEvent::ReasoningDelta(e["delta"]["thinking"].as_str().unwrap_or_default().into()))
                    }
                    _ => {}
                }
            }
        }
        Some("assistant") if v["parent_tool_use_id"].is_null() => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        out.push(AgentEvent::TextDone(block["text"].as_str().unwrap_or_default().into()));
                        *streamed_text = false;
                    }
                    Some("tool_use") => {
                        let (title, detail) = tool_title(block["name"].as_str().unwrap_or("tool"), &block["input"]);
                        out.push(AgentEvent::ToolStarted {
                            id: block["id"].as_str().unwrap_or_default().into(),
                            title,
                            detail,
                        });
                    }
                    _ => {}
                }
            }
            out.extend(mark(v));
        }
        Some("user") if v["parent_tool_use_id"].is_null() => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result" {
                    let output = match &block["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    out.push(AgentEvent::ToolFinished {
                        id: block["tool_use_id"].as_str().unwrap_or_default().into(),
                        output: clip(&output, 8000),
                        ok: block["is_error"] != true,
                    });
                }
            }
            out.extend(mark(v));
        }
        Some("result") => out.push(AgentEvent::TurnComplete {
            cost_usd: v["total_cost_usd"].as_f64(),
            error: (v["is_error"] == true).then(|| result_error(v)),
        }),
        Some("control_request") if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            let request_id = v["request_id"].as_str().unwrap_or_default().to_string();
            let tool = r["tool_name"].as_str().unwrap_or("tool");
            let (title, detail) = tool_title(tool, &r["input"]);
            pending.insert(request_id.clone(), r.clone());
            let prompt = match tool {
                "AskUserQuestion" => Some(crate::Prompt::Questions(
                    r["input"]["questions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|q| crate::Question {
                            question: q["question"].as_str().unwrap_or_default().to_string(),
                            header: q["header"].as_str().unwrap_or_default().to_string(),
                            options: q["options"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|o| (o["label"].as_str().unwrap_or_default().to_string(), o["description"].as_str().unwrap_or_default().to_string()))
                                .collect(),
                            multi: q["multiSelect"] == true,
                            secret: false,
                        })
                        .collect(),
                )),
                "ExitPlanMode" => Some(crate::Prompt::Plan(r["input"]["plan"].as_str().unwrap_or_default().to_string())),
                _ => None,
            };
            out.push(AgentEvent::PermissionRequest {
                request_id,
                title: r["title"].as_str().map(String::from).unwrap_or(title),
                detail: r["description"].as_str().map(String::from).unwrap_or(detail),
                prompt,
            });
        }
        // Claude dropped a prompt it was waiting on (an interrupt, or a hook answered it).
        Some("control_cancel_request") => {
            if let Some(id) = v["request_id"].as_str().filter(|id| pending.remove(*id).is_some()) {
                out.push(AgentEvent::PermissionResolved { request_id: id.to_string() });
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::HandHolding;

    #[test]
    fn translates_core_stream_messages() {
        let mut pending = HashMap::new();
        let mut streamed = false;
        let init = json!({"type":"system","subtype":"init","session_id":"abc","model":"claude-opus-5-5"});
        assert_eq!(
            translate(&init, &mut pending, &mut streamed),
            vec![AgentEvent::Started { native_id: "abc".into(), model: Some("claude-opus-5-5".into()) }]
        );
        let delta = json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}});
        assert_eq!(translate(&delta, &mut pending, &mut streamed), vec![AgentEvent::TextDelta("Hi".into())]);
        let perm = json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}});
        let ev = translate(&perm, &mut pending, &mut streamed);
        assert!(matches!(&ev[0], AgentEvent::PermissionRequest { request_id, detail, .. } if request_id == "r1" && detail == "ls"));
        assert!(pending.contains_key("r1"));
        let result = json!({"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.12});
        assert_eq!(
            translate(&result, &mut pending, &mut streamed),
            vec![AgentEvent::TurnComplete { cost_usd: Some(0.12), error: None }]
        );
    }

    #[test]
    fn billing_from_account_and_init() {
        let max = json!({"email":"me@example.com","subscriptionType":"Claude Max","apiProvider":"firstParty"});
        assert_eq!(account_billing(&max), Some(Billing::Plan(Some("Claude Max".into()))));
        assert_eq!(account_billing(&json!({"subscriptionType":"pro"})), Some(Billing::Plan(Some("Claude Pro".into()))));
        // A key in the environment doesn't matter while a subscription is signed in.
        let both = json!({"subscriptionType":"Claude Max","apiKeySource":"ANTHROPIC_API_KEY","apiProvider":"firstParty"});
        assert_eq!(account_billing(&both), Some(Billing::Plan(Some("Claude Max".into()))));
        assert_eq!(account_billing(&json!({"tokenSource":"none","apiKeySource":"ANTHROPIC_API_KEY","apiProvider":"firstParty"})), Some(Billing::Metered));
        assert_eq!(account_billing(&json!({"apiProvider":"bedrock"})), Some(Billing::Metered));
        assert_eq!(account_billing(&json!({"apiProvider":"firstParty"})), None);
        assert_eq!(account_billing(&Value::Null), None);


        // A proxy or gateway token (LiteLLM, OpenRouter): the init message says apiKeySource "none",
        // but this is per-token usage, not a subscription.
        let gateway = json!({"tokenSource":"ANTHROPIC_AUTH_TOKEN","apiProvider":"firstParty"});
        assert_eq!(account_billing(&gateway), Some(Billing::Metered));
        assert_eq!(account_billing(&json!({"tokenSource":"apiKeyHelper","apiKeySource":"apiKeyHelper","apiProvider":"firstParty"})), Some(Billing::Metered));
        // A long-lived subscription token from `claude setup-token`, unless a key is the one in use.
        let oauth = json!({"tokenSource":"CLAUDE_CODE_OAUTH_TOKEN","apiProvider":"firstParty"});
        assert_eq!(account_billing(&oauth), Some(Billing::Plan(None)));
        let oauth_and_key = json!({"tokenSource":"CLAUDE_CODE_OAUTH_TOKEN","apiKeySource":"ANTHROPIC_API_KEY","apiProvider":"firstParty"});
        assert_eq!(account_billing(&oauth_and_key), Some(Billing::Metered));
        // No login at all, or one that isn't in use: unknown.
        assert_eq!(account_billing(&json!({"tokenSource":"none","apiProvider":"firstParty"})), None);
        assert_eq!(account_billing(&json!({"tokenSource":"claude.ai","apiProvider":"firstParty"})), None);

        // The init message alone never claims a subscription: "none" is also what a gateway token reports.
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let init = json!({"type":"system","subtype":"init","session_id":"s","model":"m","apiKeySource":"none"});
        assert_eq!(translate(&init, &mut pending, &mut streamed).len(), 1);
    }

    #[test]
    fn context_usage_response_becomes_context_event() {
        let r = json!({"subtype":"success","request_id":"trek-2","response":{"totalTokens":15568,"maxTokens":1000000,"percentage":2}});
        assert_eq!(context_event(&r), Some(AgentEvent::Context { used: 15568, window: 1_000_000 }));
        assert_eq!(context_event(&json!({"subtype":"error","request_id":"x","error":"nope"})), None);
    }

    #[test]
    fn user_content_puts_images_before_text() {
        let dir = std::env::temp_dir().join(format!("trek-img-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G']).unwrap();
        let (content, errors) = user_content("look", &[png, dir.join("missing.png"), dir.join("x.bmp")]);
        assert_eq!(errors.len(), 2);
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], "iVBORw==");
        assert_eq!(content[1], json!({"type":"text","text":"look"}));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Recorded stream-json output (Claude Code 2.1.287, claude-haiku-4-5), one message per line.
    fn fixture(text: &str) -> Vec<Value> {
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn approving_a_plan_restores_the_threads_access_level() {
        let lines = fixture(include_str!("../fixtures/claude-plan-approval.jsonl"));
        let mut pending = HashMap::new();
        let mut streamed = false;
        let row = translate(&lines[0], &mut pending, &mut streamed);
        assert!(matches!(&row[0], AgentEvent::ToolStarted { title, detail, .. } if title == "Plan" && detail.starts_with("Create plan_test.txt")));
        let ev = translate(&lines[1], &mut pending, &mut streamed);
        let AgentEvent::PermissionRequest { request_id, prompt: Some(crate::Prompt::Plan(plan)), .. } = &ev[0] else { panic!("{ev:?}") };
        assert!(plan.starts_with("Create plan_test.txt"));
        // Claude reports "default" after the approval; Trek sets the thread's own level.
        assert_eq!(permission_mode(&lines[2]), Some("default"));

        let request = pending.remove(request_id).unwrap();
        let mut ctl = Control { next_id: 4 };
        let msgs = respond(&mut ctl, request_id, &request, Decision::Allow, HandHolding::FullAccess.claude_mode());
        assert_eq!(msgs[0]["response"]["request_id"], request_id.as_str());
        assert_eq!(msgs[0]["response"]["response"]["behavior"], "allow");
        assert_eq!(msgs[0]["response"]["response"]["updatedInput"]["plan"], request["input"]["plan"]);
        assert_eq!(msgs[1], json!({"type":"control_request","request_id":"trek-5","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}));
    }

    #[test]
    fn keep_planning_stays_in_plan_mode_with_a_reason() {
        let lines = fixture(include_str!("../fixtures/claude-plan-approval.jsonl"));
        let request = &lines[1]["request"];
        let msgs = respond(&mut Control { next_id: 0 }, "r", request, Decision::Deny, "default");
        assert_eq!(msgs.len(), 1, "no mode change");
        let r = &msgs[0]["response"]["response"];
        assert_eq!(r["behavior"], "deny");
        assert!(r["message"].as_str().unwrap().contains("keep planning"));
    }

    #[test]
    fn other_tools_keep_their_suggested_rules() {
        let lines = fixture(include_str!("../fixtures/claude-plan-approval.jsonl"));
        let request = &lines[3]["request"];
        let msgs = respond(&mut Control { next_id: 0 }, "w", request, Decision::AllowForSession, "default");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["response"]["response"]["updatedPermissions"], json!([{"type":"setMode","mode":"acceptEdits","destination":"session"}]));
        let denied = respond(&mut Control { next_id: 0 }, "w", request, Decision::Deny, "default");
        assert_eq!(denied[0]["response"]["response"]["message"], "The user declined this action.");
    }

    #[test]
    fn questions_accept_the_users_own_words() {
        let lines = fixture(include_str!("../fixtures/claude-question.jsonl"));
        let mut pending = HashMap::new();
        let ev = translate(&lines[0], &mut pending, &mut false);
        let AgentEvent::PermissionRequest { request_id, prompt: Some(crate::Prompt::Questions(q)), .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(q[0].options.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), vec!["Red", "Blue"]);
        // "Other": typed text that matches no option goes back as the answer, verbatim.
        let msg = answer(request_id, &pending[request_id], vec![(q[0].question.clone(), "teal with a hint of orange".into())]);
        let input = &msg["response"]["response"]["updatedInput"];
        assert_eq!(input["answers"], json!({"Which color do you like?":"teal with a hint of orange"}));
        assert_eq!(input["questions"], lines[0]["request"]["input"]["questions"]);
        let skipped = respond(&mut Control { next_id: 0 }, request_id, &pending[request_id], Decision::Deny, "default");
        assert!(skipped[0]["response"]["response"]["message"].as_str().unwrap().contains("skipped"));
    }

    #[test]
    fn interrupting_an_open_prompt_withdraws_it_and_ends_the_turn_as_interrupted() {
        // Recorded: Trek's interrupt while Claude waited on a Bash approval.
        let lines = fixture(include_str!("../fixtures/claude-interrupt.jsonl"));
        let mut pending = HashMap::new();
        let ev: Vec<AgentEvent> = lines.iter().flat_map(|v| translate(v, &mut pending, &mut false)).collect();
        let request_id = lines[0]["request_id"].as_str().unwrap().to_string();
        assert!(matches!(&ev[0], AgentEvent::PermissionRequest { request_id: r, .. } if *r == request_id));
        assert!(ev.contains(&AgentEvent::PermissionResolved { request_id: request_id.clone() }));
        assert!(pending.is_empty());
        let Some(AgentEvent::TurnComplete { error, .. }) = ev.last() else { panic!("{ev:?}") };
        assert_eq!(error.as_deref(), Some("Interrupted"));
        // Already answered: nothing to take down.
        assert!(translate(&lines[1], &mut pending, &mut false).is_empty());
        // Claude's internal diagnostics never reach the user as the reason.
        assert_eq!(result_error(&json!({"is_error":true,"errors":["[ede_diagnostic] result_type=user"]})), "The turn failed.");
    }

    #[test]
    fn failed_results_say_why() {
        // Recorded: `claude --resume <unknown id>` answers with a result, not a turn.
        let v: Value = serde_json::from_str(r#"{"type":"result","subtype":"error_during_execution","duration_ms":0,"is_error":true,"num_turns":0,"stop_reason":null,"session_id":"0b0b0b0b-0000-4000-8000-000000000000","total_cost_usd":0,"permission_denials":[],"errors":["No conversation found with session ID: 0b0b0b0b-0000-4000-8000-000000000000"]}"#).unwrap();
        assert_eq!(
            translate(&v, &mut HashMap::new(), &mut false),
            vec![AgentEvent::TurnComplete { cost_usd: Some(0.0), error: Some("No conversation found with session ID: 0b0b0b0b-0000-4000-8000-000000000000".into()) }]
        );
        assert_eq!(result_error(&json!({"is_error":true,"result":"API Error: overloaded"})), "API Error: overloaded");
        assert_eq!(result_error(&json!({"is_error":true})), "The turn failed.");
    }

    #[test]
    fn todo_rows_name_the_active_step() {
        let input = json!({"todos":[{"content":"Read code","status":"completed","activeForm":"Reading code"},{"content":"Fix bug","status":"in_progress","activeForm":"Fixing the bug"}]});
        assert_eq!(tool_title("TodoWrite", &input), ("Update plan".into(), "Fixing the bug".into()));
    }

    fn config() -> SessionConfig {
        SessionConfig {
            agent: trek_core::AgentId::ClaudeCode,
            cwd: "/tmp".into(),
            model: Some("claude-haiku-4-5".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::Supervised,
            plan: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
        }
    }

    /// `flag value` pairs and lone flags in `args`.
    fn has(args: &[String], want: &[&str]) -> bool {
        args.windows(want.len()).any(|w| w.iter().zip(want).all(|(a, b)| a == b))
    }

    #[test]
    fn resuming_partway_and_forking_pass_the_cli_flags() {
        let mut c = config();
        c.resume = Some("s1".into());
        c.resume_at = Some("m7".into());
        c.fork = true;
        let args = cli_args(&c);
        assert!(has(&args, &["--resume", "s1"]) && has(&args, &["--resume-session-at", "m7"]) && has(&args, &["--fork-session"]), "{args:?}");
        // In place: the same session, cut back.
        c.fork = false;
        let args = cli_args(&c);
        assert!(has(&args, &["--resume-session-at", "m7"]) && !has(&args, &["--fork-session"]));
        // A whole fork.
        c.resume_at = None;
        c.fork = true;
        let args = cli_args(&c);
        assert!(has(&args, &["--resume", "s1"]) && has(&args, &["--fork-session"]) && !args.iter().any(|a| a == "--resume-session-at"));
        // Nothing to resume: neither applies.
        let args = cli_args(&config());
        assert!(!args.iter().any(|a| a.starts_with("--resume") || a == "--fork-session"));
    }

    #[test]
    fn every_message_is_a_point_to_resume_at() {
        // Recorded: `--resume <id> --resume-session-at <the first answer> --fork-session`, then
        // "list every word I asked you to remember" (claude-haiku-4-5): the copy knows only the
        // first turn's word.
        let lines = fixture(include_str!("../fixtures/claude-fork-at.jsonl"));
        let ev: Vec<AgentEvent> = lines.iter().flat_map(|v| translate(v, &mut HashMap::new(), &mut false)).collect();
        assert!(matches!(&ev[0], AgentEvent::Started { native_id, .. } if native_id == "02c2cd16-37a1-48fa-967e-cc54689d7ac7"));
        let marks: Vec<&str> = ev.iter().filter_map(|e| if let AgentEvent::Mark(m) = e { Some(m.as_str()) } else { None }).collect();
        assert_eq!(marks, ["d4baf339-bf60-4409-88f1-7f71f5b0ed8f", "079acfb9-e65d-471d-ad28-b5603f1d9e57"]);
        assert!(ev.contains(&AgentEvent::TextDone("APPLE".into())));
        // The mark follows its message's events, so the latest is the answer itself.
        let done = ev.iter().position(|e| *e == AgentEvent::TextDone("APPLE".into())).unwrap();
        assert_eq!(ev[done + 1], AgentEvent::Mark("079acfb9-e65d-471d-ad28-b5603f1d9e57".into()));
    }

    #[test]
    fn mcp_config_shape() {
        let servers = [crate::McpServer { name: "fs".into(), command: "npx".into(), args: vec!["-y".into(), "srv".into()], env: vec![("K".into(), "v".into())] }];
        assert_eq!(
            json!({ "mcpServers": mcp_servers_json(&servers) }),
            json!({"mcpServers":{"fs":{"command":"npx","args":["-y","srv"],"env":{"K":"v"}}}})
        );
    }
}
