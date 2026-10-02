//! Claude Code via the user's own `claude` binary (stream-json + stdio control protocol).
//! Trek never reads Claude credentials; the CLI handles its own login.

use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, clip, load_image, mcp_servers_json};
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
        "TodoWrite" => ("Update plan".into(), String::new()),
        other => (other.to_string(), clip(&input.to_string(), 200)),
    }
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let bin = detect::which("claude").context("Claude Code isn't installed (npm i -g @anthropic-ai/claude-code)")?;
    let mode = if config.plan { "plan" } else { config.hand_holding.claude_mode() };
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args([
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
    ]);
    if let Some(model) = &config.model {
        cmd.args(["--model", model]);
    }
    if config.effort != Effort::Off {
        cmd.args(["--effort", config.effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]).as_str()]);
    }
    if let Some(id) = &config.resume {
        cmd.args(["--resume", id]);
    }
    if config.fast.is_some() {
        cmd.args(["--settings", r#"{"fastMode":true}"#]);
    }
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
    let stderr = child.stderr.take().unwrap();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            tracing::debug!("claude stderr: {l}");
        }
    });

    let mut next_id = 0u64;
    let mut req = |subtype: &str, extra: Value| {
        next_id += 1;
        let mut request = json!({ "subtype": subtype });
        if let (Some(r), Some(e)) = (request.as_object_mut(), extra.as_object()) {
            r.extend(e.clone());
        }
        json!({ "type": "control_request", "request_id": format!("trek-{next_id}"), "request": request })
    };
    let init = req("initialize", json!({}));
    let init_id = init["request_id"].as_str().unwrap_or_default().to_string();
    write_line(&mut stdin, &init).await?;
    // Outstanding `get_context_usage` requests; their responses become `Context` events.
    let mut context_requests: HashSet<String> = HashSet::new();

    // Inputs of pending permission requests, echoed back as `updatedInput` on allow.
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut streamed_text = false;

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt { text, images } => {
                        streamed_text = false;
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
                    Command::Interrupt => write_line(&mut stdin, &req("interrupt", json!({}))).await?,
                    Command::SetHandHolding(h) => {
                        write_line(&mut stdin, &req("set_permission_mode", json!({ "mode": h.claude_mode() }))).await?
                    }
                    Command::SetModel { model, .. } => {
                        write_line(&mut stdin, &req("set_model", json!({ "model": model }))).await?
                    }
                    Command::Respond { request_id, decision } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
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
                            Decision::Deny => json!({ "behavior": "deny", "message": "The user declined this action." }),
                        };
                        let msg = json!({
                            "type": "control_response",
                            "response": { "subtype": "success", "request_id": request_id, "response": response }
                        });
                        write_line(&mut stdin, &msg).await?;
                    }
                    Command::Answer { request_id, answers } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
                        let mut input = request["input"].clone();
                        // AskUserQuestion reads its answers from the input it gets back.
                        input["answers"] = Value::Object(answers.into_iter().map(|(q, a)| (q, Value::String(a))).collect());
                        let msg = json!({
                            "type": "control_response",
                            "response": { "subtype": "success", "request_id": request_id, "response": { "behavior": "allow", "updatedInput": input } }
                        });
                        write_line(&mut stdin, &msg).await?;
                    }
                    Command::Shutdown => break,
                }
            }
            line = stdout.next_line() => {
                let Some(line) = line? else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
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
                        let c = req("get_context_usage", json!({}));
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
                    if events.send(ev).await.is_err() {
                        return Ok(());
                    }
                }
                if v["type"] == "result" {
                    let c = req("get_context_usage", json!({}));
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
        }
        Some("result") => out.push(AgentEvent::TurnComplete {
            cost_usd: v["total_cost_usd"].as_f64(),
            error: (v["is_error"] == true).then(|| v["result"].as_str().unwrap_or("The turn failed.").to_string()),
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
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn mcp_config_shape() {
        let servers = [crate::McpServer { name: "fs".into(), command: "npx".into(), args: vec!["-y".into(), "srv".into()], env: vec![("K".into(), "v".into())] }];
        assert_eq!(
            json!({ "mcpServers": mcp_servers_json(&servers) }),
            json!({"mcpServers":{"fs":{"command":"npx","args":["-y","srv"],"env":{"K":"v"}}}})
        );
    }
}
