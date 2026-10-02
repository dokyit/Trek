//! Claude Code via the user's own `claude` binary (stream-json + stdio control protocol).
//! Trek never reads Claude credentials; the CLI handles its own login.

use crate::{AgentEvent, Command, Decision, SessionConfig, clip};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
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
    write_line(&mut stdin, &req("initialize", json!({}))).await?;

    // Inputs of pending permission requests, echoed back as `updatedInput` on allow.
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut streamed_text = false;

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt(text) => {
                        streamed_text = false;
                        let msg = json!({
                            "type": "user", "session_id": "",
                            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
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
                        let input = pending.remove(&request_id).unwrap_or(json!({}));
                        let response = match decision {
                            Decision::Allow | Decision::AllowForSession => json!({ "behavior": "allow", "updatedInput": input }),
                            Decision::Deny => json!({ "behavior": "deny", "message": "The user declined this action." }),
                        };
                        let msg = json!({
                            "type": "control_response",
                            "response": { "subtype": "success", "request_id": request_id, "response": response }
                        });
                        write_line(&mut stdin, &msg).await?;
                    }
                    Command::Shutdown => break,
                }
            }
            line = stdout.next_line() => {
                let Some(line) = line? else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                for ev in translate(&v, &mut pending, &mut streamed_text) {
                    if events.send(ev).await.is_err() {
                        return Ok(());
                    }
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

fn translate(v: &Value, pending: &mut HashMap<String, Value>, streamed_text: &mut bool) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => out.push(AgentEvent::Started {
            native_id: v["session_id"].as_str().unwrap_or_default().to_string(),
            model: v["model"].as_str().map(String::from),
        }),
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
        Some("user") => {
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
            pending.insert(request_id.clone(), r["input"].clone());
            out.push(AgentEvent::PermissionRequest {
                request_id,
                title: r["title"].as_str().map(String::from).unwrap_or(title),
                detail: r["description"].as_str().map(String::from).unwrap_or(detail),
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
}
