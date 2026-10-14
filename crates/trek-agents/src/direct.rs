//! Direct providers (API keys and local servers). Phase 2 provides full native agent tool
//! execution (read_file, edit_file, write_file, bash) with HandHolding permission gates and
//! streaming tool-call accumulation, including fallback XML tool-call parsing for fine-tuned models.

use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, load_image};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;
use anyhow::{Context as _, Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use trek_core::catalog::{ModelInfo, Wire, direct_provider};
use trek_core::settings::secrets;
use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost};

/// The error for a failed request: a usage limit for a 429 (said with `AgentEvent::LimitReached`),
/// else `message` as it is.
fn request_error(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, message: String) -> anyhow::Error {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return crate::limits::LimitError(crate::limits::from_response(headers, message, trek_core::store::now_ms())).into();
    }
    anyhow::anyhow!(message)
}

fn system_prompt(cwd: &Path, read_only: bool, instructions: Option<&str>) -> String {
    let mut p = format!(
        "You are Trek, an autonomous coding agent working in `{}`.\n\
         You have access to tools to inspect files, make code edits, and execute terminal commands.\n\
         - bash: Run terminal commands (git, cargo, tests, etc.)\n\
         - read_file: View file contents with line numbers\n\
         - write_file: Create or overwrite files\n\
         - edit_file: Replace precise old_str snippets with new_str\n\
         Always examine files before editing them. Make edits minimal and correct.\n",
        cwd.display()
    );
    if read_only {
        p.push_str("\nIMPORTANT: This session is in read-only mode. File edits and mutating commands are prohibited.\n");
    }
    if let Some(extra) = instructions {
        p.push_str("\n");
        p.push_str(extra);
        p.push_str("\n");
    }
    p
}

static ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_id() -> String {
    let c = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}_{c}", trek_core::store::now_ms())
}

fn direct_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command in the project directory. Use this for git, cargo, running tests, listing files, etc.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The command line string to execute."
                        }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the contents of a file in the workspace. Returns the file content with line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The relative or absolute file path to read."
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "Optional 1-based start line number."
                        },
                        "end_line": {
                            "type": "integer",
                            "description": "Optional 1-based end line number."
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create a new file or completely overwrite an existing file with the specified content.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The file path to write to."
                        },
                        "content": {
                            "type": "string",
                            "description": "The complete text content to write."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace a specific snippet of text (old_str) with new text (new_str) in an existing file. old_str must uniquely appear in the file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file to edit."
                        },
                        "old_str": {
                            "type": "string",
                            "description": "The exact existing text to replace."
                        },
                        "new_str": {
                            "type": "string",
                            "description": "The replacement text."
                        }
                    },
                    "required": ["path", "old_str", "new_str"]
                }
            }
        }),
    ]
}

#[derive(Debug, Clone, Default)]
pub struct AccumulatedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct TurnResponse {
    pub text: String,
    pub content: Option<Value>,
    pub tool_calls: Vec<AccumulatedToolCall>,
}

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let AgentId::Direct(provider_id) = &config.agent else { unreachable!() };
    let provider = direct_provider(provider_id).with_context(|| format!("unknown provider {provider_id}"))?;
    let key = secrets::api_key(provider.id);
    if !provider.local && key.is_none() {
        bail!("No API key for {}. Add one in Settings → API Keys.", provider.name);
    }
    let mut model = config.model.clone().context("Pick a model first")?;
    let mut effort = config.effort;
    let mut hand_holding = config.hand_holding;
    let mut allowed_for_session = HashSet::new();
    let client = reqwest::Client::new();
    events
        .send(AgentEvent::Started { native_id: config.resume.clone().unwrap_or_default(), model: Some(model.clone()) })
        .await?;
    events.send(AgentEvent::Billing(if provider.local { Billing::Local } else { Billing::Metered })).await?;

    let system = system_prompt(&config.cwd, config.read_only, config.instructions.as_deref());
    let mut history: Vec<Value> = Vec::new();
    let mut kept = VecDeque::new();
    let mut shutdown = false;

    loop {
        let cmd = match kept.pop_front() {
            Some(cmd) => cmd,
            None => match commands.recv().await {
                Ok(cmd) => cmd,
                Err(_) => break,
            },
        };
        match cmd {
            Command::Prompt { text, images } => {
                let mut next = Some((text, images));
                let mut error = None;
                while let Some((text, images)) = next.take() {
                    let (message, skipped) = user_message(provider.wire, &text, &images);
                    for e in skipped {
                        events.send(AgentEvent::Notice(format!("Image left out: {e}"))).await?;
                    }
                    history.push(message);

                    // Multi-turn agent loop for tool calls
                    let mut turn_count = 0;
                    let mut turn_error = None;
                    while turn_count < 30 {
                        turn_count += 1;
                        let result = match provider.wire {
                            Wire::Anthropic => {
                                anthropic_turn(&client, ANTHROPIC_MESSAGES_URL, key.as_deref().unwrap_or_default(), &model, effort, &system, &history, &events, &commands, &mut kept, &mut shutdown).await
                            }
                            _ => {
                                let usage = REPORTS_USAGE.contains(&provider.id);
                                openai_turn(
                                    &client,
                                    &config.agent,
                                    provider.base_url,
                                    key.as_deref(),
                                    provider.local,
                                    usage,
                                    &model,
                                    effort,
                                    &system,
                                    &history,
                                    &events,
                                    &commands,
                                    &mut kept,
                                    &mut shutdown,
                                )
                                .await
                            }
                        };
                        match result {
                            Ok(resp) => {
                                if let Some(content) = resp.content {
                                    history.push(content);
                                    break;
                                }
                                if resp.tool_calls.is_empty() {
                                    history.push(json!({ "role": "assistant", "content": resp.text }));
                                    break;
                                }
                                let tool_calls_json: Vec<Value> = resp.tool_calls.iter().map(|c| json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": c.name,
                                        "arguments": c.arguments
                                    }
                                })).collect();
                                history.push(json!({
                                    "role": "assistant",
                                    "content": if resp.text.is_empty() { Value::Null } else { Value::String(resp.text.clone()) },
                                    "tool_calls": tool_calls_json
                                }));

                                let mut tool_interrupted = false;
                                for call in &resp.tool_calls {
                                    let (title, detail) = tool_display(&call.name, &call.arguments);
                                    events.send(AgentEvent::ToolStarted {
                                        id: call.id.clone(),
                                        title: title.to_string(),
                                        detail: detail.clone(),
                                    }).await?;

                                    let (output, ok, lines) = execute_tool(
                                        &call.name,
                                        &call.arguments,
                                        &config.cwd,
                                        config.read_only,
                                        hand_holding,
                                        &mut allowed_for_session,
                                        &events,
                                        &commands,
                                        &mut kept,
                                        &mut shutdown,
                                    ).await?;

                                    if let Some((added, removed)) = lines {
                                        events.send(AgentEvent::ToolLines {
                                            id: call.id.clone(),
                                            added,
                                            removed,
                                        }).await?;
                                    }

                                    events.send(AgentEvent::ToolFinished {
                                        id: call.id.clone(),
                                        output: output.clone(),
                                        ok,
                                    }).await?;

                                    history.push(json!({
                                        "role": "tool",
                                        "tool_call_id": call.id,
                                        "content": output
                                    }));

                                    if shutdown {
                                        tool_interrupted = true;
                                        break;
                                    }
                                }
                                if tool_interrupted || shutdown {
                                    break;
                                }
                            }
                            Err(e) => {
                                history.pop();
                                if let Some(crate::limits::LimitError(limit)) = e.downcast_ref() {
                                    events.send(limit.clone().event()).await?;
                                }
                                turn_error.get_or_insert_with(|| format!("{e:#}"));
                                break;
                            }
                        }
                    }
                    if let Some(err) = turn_error {
                        error.get_or_insert(err);
                    }

                    while let Some(cmd) = kept.pop_front() {
                        match cmd {
                            Command::Prompt { text, images } => {
                                next = Some((text, images));
                                break;
                            }
                            Command::SetModel { model: m, effort: e } => {
                                model = m;
                                effort = e;
                            }
                            Command::SetModes { effort: e, .. } => effort = e,
                            Command::SetHandHolding(h) => hand_holding = h,
                            Command::Shutdown => {
                                shutdown = true;
                                break;
                            }
                            Command::Answer { .. }
                            | Command::Interrupt
                            | Command::Respond { .. }
                            | Command::ReadTask { .. }
                            | Command::StopTask { .. } => {}
                        }
                    }
                    if shutdown {
                        break;
                    }
                }
                events.send(AgentEvent::TurnComplete { error }).await?;
                if shutdown {
                    break;
                }
            }
            Command::SetModel { model: m, effort: e } => {
                model = m;
                effort = e;
            }
            Command::SetModes { effort: e, .. } => effort = e,
            Command::SetHandHolding(h) => hand_holding = h,
            Command::Answer { .. } => {}
            Command::Shutdown => break,
            Command::Interrupt | Command::Respond { .. } | Command::ReadTask { .. } | Command::StopTask { .. } => {}
        }
    }
    Ok(())
}

fn is_safe_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.contains(';') || trimmed.contains('&') || trimmed.contains('|') || trimmed.contains('>') {
        return false;
    }
    let safe_prefixes = [
        "git status", "git diff", "git log", "git show", "git branch",
        "cargo check", "cargo test", "cargo clippy",
        "ls", "cat", "head", "tail", "grep", "rg", "find", "fd",
        "pwd", "which", "echo", "wc", "stat", "file"
    ];
    safe_prefixes.iter().any(|p| trimmed == *p || trimmed.starts_with(&format!("{p} ")))
}

fn tool_display(name: &str, args_json: &str) -> (&'static str, String) {
    let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    match name {
        "bash" => {
            let cmd = parsed.get("command").and_then(|c| c.as_str()).unwrap_or(args_json);
            ("Run command", cmd.to_string())
        }
        "read_file" => {
            let path = parsed.get("path").and_then(|p| p.as_str()).unwrap_or(args_json);
            ("Read", path.to_string())
        }
        "write_file" => {
            let path = parsed.get("path").and_then(|p| p.as_str()).unwrap_or(args_json);
            ("Write", path.to_string())
        }
        "edit_file" => {
            let path = parsed.get("path").and_then(|p| p.as_str()).unwrap_or(args_json);
            ("Edit", path.to_string())
        }
        _ => ("Tool", args_json.to_string()),
    }
}

async fn ask_permission(
    title: &str,
    detail: &str,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<Decision> {
    let request_id = format!("direct-perm-{}", next_id());
    events
        .send(AgentEvent::PermissionRequest {
            request_id: request_id.clone(),
            title: title.to_string(),
            detail: detail.to_string(),
            prompt: None,
        })
        .await?;

    loop {
        let cmd = match kept.pop_front() {
            Some(cmd) => cmd,
            None => match commands.recv().await {
                Ok(cmd) => cmd,
                Err(_) => {
                    *shutdown = true;
                    bail!("Commands channel closed");
                }
            },
        };
        match cmd {
            Command::Respond { request_id: id, decision } if id == request_id => {
                events.send(AgentEvent::PermissionResolved { request_id }).await?;
                return Ok(decision);
            }
            Command::Interrupt => {
                events.send(AgentEvent::PermissionResolved { request_id }).await?;
                bail!("Interrupted");
            }
            Command::Shutdown => {
                events.send(AgentEvent::PermissionResolved { request_id }).await?;
                *shutdown = true;
                bail!("Shutdown");
            }
            other => kept.push_back(other),
        }
    }
}

async fn execute_tool(
    name: &str,
    args_json: &str,
    cwd: &Path,
    read_only: bool,
    hand_holding: HandHolding,
    allowed_for_session: &mut HashSet<String>,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    match name {
        "bash" => {
            if read_only {
                return Ok(("Permission denied: commands are disabled in read-only mode".into(), false, None));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or_else(|_| json!({ "command": args_json }));
            let command = parsed.get("command").and_then(|c| c.as_str()).unwrap_or(args_json).trim();
            if command.is_empty() {
                return Ok(("Error: empty command".into(), false, None));
            }
            let need_ask = match hand_holding {
                HandHolding::FullAccess => false,
                HandHolding::Auto => !is_safe_command(command) && !allowed_for_session.contains(command),
                HandHolding::AutoAcceptEdits | HandHolding::Supervised => !allowed_for_session.contains(command),
            };
            if need_ask {
                match ask_permission("Run command", command, events, commands, kept, shutdown).await? {
                    Decision::Allow => {},
                    Decision::AllowForSession => {
                        allowed_for_session.insert(command.to_string());
                    },
                    Decision::Deny => {
                        return Ok(("Permission denied by user".into(), false, None));
                    }
                }
            }
            let (out, ok) = run_bash_command(command, cwd, commands, kept, shutdown).await?;
            Ok((out, ok, None))
        }
        "read_file" => {
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed.get("path").and_then(|p| p.as_str()).unwrap_or_default();
            if path_str.is_empty() {
                return Ok(("Error: path parameter is required".into(), false, None));
            }
            let start = parsed.get("start_line").and_then(|s| s.as_u64()).map(|s| s as usize);
            let end = parsed.get("end_line").and_then(|e| e.as_u64()).map(|e| e as usize);
            let (out, ok) = run_read_file(path_str, start, end, cwd).await?;
            Ok((out, ok, None))
        }
        "write_file" => {
            if read_only {
                return Ok(("Permission denied: session is in read-only mode".into(), false, None));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed.get("path").and_then(|p| p.as_str()).unwrap_or_default();
            let content = parsed.get("content").and_then(|c| c.as_str()).unwrap_or_default();
            if path_str.is_empty() {
                return Ok(("Error: path parameter is required".into(), false, None));
            }
            let need_ask = match hand_holding {
                HandHolding::Supervised => !allowed_for_session.contains("write_file"),
                _ => false,
            };
            if need_ask {
                match ask_permission("Write file", path_str, events, commands, kept, shutdown).await? {
                    Decision::Allow => {},
                    Decision::AllowForSession => {
                        allowed_for_session.insert("write_file".into());
                    },
                    Decision::Deny => {
                        return Ok(("Permission denied by user".into(), false, None));
                    }
                }
            }
            run_write_file(path_str, content, cwd).await
        }
        "edit_file" => {
            if read_only {
                return Ok(("Permission denied: session is in read-only mode".into(), false, None));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed.get("path").and_then(|p| p.as_str()).unwrap_or_default();
            let old_str = parsed.get("old_str").and_then(|o| o.as_str()).unwrap_or_default();
            let new_str = parsed.get("new_str").and_then(|n| n.as_str()).unwrap_or_default();
            if path_str.is_empty() || old_str.is_empty() {
                return Ok(("Error: path and old_str parameters are required".into(), false, None));
            }
            let need_ask = match hand_holding {
                HandHolding::Supervised => !allowed_for_session.contains("edit_file"),
                _ => false,
            };
            if need_ask {
                match ask_permission("Edit file", path_str, events, commands, kept, shutdown).await? {
                    Decision::Allow => {},
                    Decision::AllowForSession => {
                        allowed_for_session.insert("edit_file".into());
                    },
                    Decision::Deny => {
                        return Ok(("Permission denied by user".into(), false, None));
                    }
                }
            }
            run_edit_file(path_str, old_str, new_str, cwd).await
        }
        unknown => {
            Ok((format!("Unknown tool: {unknown}. Available tools: bash, read_file, write_file, edit_file"), false, None))
        }
    }
}

async fn run_bash_command(
    command: &str,
    cwd: &Path,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool)> {
    #[cfg(unix)]
    let mut child = match trek_core::procs::async_command("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn() {
            Ok(c) => c,
            Err(e) => return Ok((format!("Failed to execute command: {e}"), false)),
        };

    #[cfg(windows)]
    let mut child = match trek_core::procs::async_command("cmd.exe")
        .arg("/c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn() {
            Ok(c) => c,
            Err(e) => return Ok((format!("Failed to execute command: {e}"), false)),
        };

    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();

    let read_out = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut stdout_buf);
    let read_err = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut stderr_buf);

    tokio::select! {
        res = async {
            tokio::try_join!(read_out, read_err)?;
            child.wait().await
        } => {
            let status = res?;
            let mut out = String::from_utf8_lossy(&stdout_buf).to_string();
            let err = String::from_utf8_lossy(&stderr_buf);
            if !err.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&err);
            }
            const MAX_OUT: usize = 50_000;
            if out.len() > MAX_OUT {
                out.truncate(MAX_OUT);
                out.push_str("\n... [output truncated]");
            }
            if out.trim().is_empty() {
                out = "(Command finished with no output)".to_string();
            }
            Ok((out, status.success()))
        }
        cmd = commands.recv() => {
            let _ = child.kill().await;
            keep_command(cmd, kept, shutdown)?;
            bail!("Interrupted")
        }
    }
}

async fn run_read_file(
    path_str: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    cwd: &Path,
) -> Result<(String, bool)> {
    let path = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        cwd.join(path_str)
    };
    if !path.exists() {
        return Ok((format!("File not found: {path_str}"), false));
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Ok((format!("Error reading {path_str}: {e}"), false)),
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Ok(("(File is empty)".to_string(), true));
    }
    let start = start_line.unwrap_or(1).max(1) - 1;
    let end = end_line.unwrap_or(lines.len()).min(lines.len());
    if start >= lines.len() {
        return Ok((format!("Start line {} is beyond end of file ({} lines)", start + 1, lines.len()), false));
    }
    let mut out = String::new();
    for (idx, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:4} | {}\n", start + idx + 1, line));
    }
    Ok((out, true))
}

async fn run_write_file(path_str: &str, content: &str, cwd: &Path) -> Result<(String, bool, Option<(u32, u32)>)> {
    let path = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        cwd.join(path_str)
    };
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let added = content.lines().count() as u32;
    match tokio::fs::write(&path, content).await {
        Ok(_) => Ok((format!("Successfully wrote {added} lines to {path_str}"), true, Some((added, 0)))),
        Err(e) => Ok((format!("Error writing {path_str}: {e}"), false, None)),
    }
}

async fn run_edit_file(path_str: &str, old_str: &str, new_str: &str, cwd: &Path) -> Result<(String, bool, Option<(u32, u32)>)> {
    let path = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        cwd.join(path_str)
    };
    if !path.exists() {
        return Ok((format!("File not found: {path_str}"), false, None));
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Ok((format!("Error reading {path_str}: {e}"), false, None)),
    };
    let count = content.matches(old_str).count();
    if count == 0 {
        return Ok((format!("Error: old_str was not found in {path_str}. Please provide the exact text to match."), false, None));
    }
    if count > 1 {
        return Ok((format!("Error: old_str matched {count} times in {path_str}. Please provide more surrounding context to uniquely identify the location."), false, None));
    }
    let updated = content.replacen(old_str, new_str, 1);
    match tokio::fs::write(&path, &updated).await {
        Ok(_) => {
            let removed = old_str.lines().count() as u32;
            let added = new_str.lines().count() as u32;
            Ok((format!("Successfully edited {path_str}"), true, Some((added, removed))))
        }
        Err(e) => Ok((format!("Error writing edits to {path_str}: {e}"), false, None)),
    }
}

pub fn parse_xml_tool_calls(text: &str) -> (Vec<AccumulatedToolCall>, String) {
    let mut calls = Vec::new();
    let mut cleaned = String::new();
    let mut rest = text;

    while let Some(start_idx) = rest.find("<tool_call>") {
        cleaned.push_str(&rest[..start_idx]);
        let after_start = &rest[start_idx + "<tool_call>".len()..];
        let Some(end_idx) = after_start.find("</tool_call>") else {
            cleaned.push_str(&rest[start_idx..]);
            rest = "";
            break;
        };
        let body = after_start[..end_idx].trim();
        rest = &after_start[end_idx + "</tool_call>".len()..];

        if let Some(after_func) = body.strip_prefix("<function=") {
            let (name, args_part) = if let Some(bracket_end) = after_func.find('>') {
                let name = after_func[..bracket_end].trim();
                let remaining = after_func[bracket_end + 1..].trim();
                let args = remaining.strip_suffix("</function>").unwrap_or(remaining).trim();
                (name, args)
            } else {
                ("", "")
            };
            if !name.is_empty() {
                let formatted_args = if !args_part.starts_with('{') {
                    if name == "bash" {
                        json!({ "command": args_part }).to_string()
                    } else {
                        json!({ "path": args_part }).to_string()
                    }
                } else {
                    args_part.to_string()
                };
                calls.push(AccumulatedToolCall {
                    id: format!("call_xml_{}", next_id()),
                    name: name.to_string(),
                    arguments: formatted_args,
                });
                continue;
            }
        }

        if let Ok(v) = serde_json::from_str::<Value>(body) {
            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                let args_str = match v.get("arguments") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                calls.push(AccumulatedToolCall {
                    id: format!("call_xml_{}", next_id()),
                    name: name.to_string(),
                    arguments: args_str,
                });
                continue;
            }
        }
    }
    cleaned.push_str(rest);
    (calls, cleaned.trim().to_string())
}

/// OpenAI-style providers known to take `stream_options.include_usage` (a final chunk with the
/// request's usage; Google's OpenAI-compatible endpoint documents it too). Others aren't sent it
/// (Mistral turns down fields it doesn't know); usage they send anyway is still counted.
const REPORTS_USAGE: &[&str] = &["openai", "openrouter", "deepseek", "xai", "groq", "google"];

/// Take in what an Anthropic stream event says about usage: `message_start` has the input
/// side (and output so far), `message_delta` the running output count. `long_writes`: how many
/// of the cache writes were 1-hour ones (`cache_creation`), priced apart.
fn anthropic_usage(v: &Value, usage: &mut TokenUsage, long_writes: &mut u64) {
    let u = match v["type"].as_str() {
        Some("message_start") => &v["message"]["usage"],
        Some("message_delta") => &v["usage"],
        _ => return,
    };
    let set = |n: &Value, field: &mut u64| {
        if let Some(n) = n.as_u64() {
            *field = n;
        }
    };
    set(&u["input_tokens"], &mut usage.input);
    set(&u["output_tokens"], &mut usage.output);
    set(&u["cache_read_input_tokens"], &mut usage.cache_read);
    set(&u["cache_creation_input_tokens"], &mut usage.cache_write);
    set(&u["cache_creation"]["ephemeral_1h_input_tokens"], long_writes);
}

/// An OpenAI-style chunk's `usage` (the last chunk, when asked for), and its cost when the
/// provider says (OpenRouter's `cost`). Prompt tokens include cached ones and cache writes,
/// which are counted apart here; output is all the model wrote, reasoning included (providers
/// that leave reasoning out of `completion_tokens` still count it in `total_tokens`).
fn openai_usage(v: &Value) -> Option<(TokenUsage, Option<f64>)> {
    let u = v.get("usage").filter(|u| u.is_object())?;
    let prompt = u["prompt_tokens"].as_u64().unwrap_or(0);
    let details = &u["prompt_tokens_details"];
    let cached = details["cached_tokens"].as_u64().unwrap_or(0).min(prompt);
    let written = details["cache_write_tokens"].as_u64().unwrap_or(0).min(prompt - cached);
    let completion = u["completion_tokens"].as_u64().unwrap_or(0);
    let output = completion.max(u["total_tokens"].as_u64().unwrap_or(0).saturating_sub(prompt));
    let t = TokenUsage { input: prompt - cached - written, output, cache_read: cached, cache_write: written };
    (!t.is_empty()).then_some((t, u["cost"].as_f64()))
}

/// A user message for `wire`. Without images the content is a plain string; with images it's
/// Anthropic image blocks before the text, or OpenAI `image_url` data URLs after it.
/// Unreadable images are skipped and reported.
fn user_message(wire: Wire, text: &str, images: &[PathBuf]) -> (Value, Vec<String>) {
    let mut errors = Vec::new();
    let mut loaded = Vec::new();
    for path in images {
        match load_image(path) {
            Ok(img) => loaded.push(img),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if loaded.is_empty() {
        return (json!({ "role": "user", "content": text }), errors);
    }
    let content: Vec<Value> = match wire {
        Wire::Anthropic => loaded
            .into_iter()
            .map(|(media_type, data)| json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } }))
            .chain([json!({ "type": "text", "text": text })])
            .collect(),
        _ => [json!({ "type": "text", "text": text })]
            .into_iter()
            .chain(loaded.into_iter().map(|(media_type, data)| {
                json!({ "type": "image_url", "image_url": { "url": format!("data:{media_type};base64,{data}") } })
            }))
            .collect(),
    };
    (json!({ "role": "user", "content": content }), errors)
}

/// Split an SSE byte stream into `data:` payloads.
fn sse_events(buf: &mut Vec<u8>) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
        let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4));
        let Some((pos, end)) = lf.into_iter().chain(crlf).min_by_key(|(i, _)| *i) else { break };
        let block: Vec<u8> = buf.drain(..pos + end).collect();
        let text = String::from_utf8_lossy(&block[..pos]);
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s)))
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            out.push(data);
        }
    }
    out
}

fn keep_command(cmd: Result<Command, async_channel::RecvError>, kept: &mut VecDeque<Command>, shutdown: &mut bool) -> Result<()> {
    match cmd {
        Ok(Command::Interrupt) => bail!("Interrupted"),
        Ok(Command::Shutdown) | Err(_) => {
            *shutdown = true;
            bail!("Interrupted")
        }
        Ok(cmd) => {
            kept.push_back(cmd);
            Ok(())
        }
    }
}

async fn send_request(
    req: reqwest::RequestBuilder,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
    timeout: Duration,
) -> Result<reqwest::Response> {
    let send = req.send();
    tokio::pin!(send);
    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            response = &mut send => return Ok(response?),
            _ = &mut sleep => bail!("Timed out waiting for response headers after {} seconds", timeout.as_secs()),
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
}

fn anthropic_body(model: &str, effort: Effort, system: &str, history: &[Value]) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": 64000,
        "stream": true,
        "system": system,
        "messages": history,
    });
    if model.starts_with("claude-haiku-4") {
        if effort >= Effort::Low {
            let budget = match effort {
                Effort::Low => 2048,
                Effort::Medium => 8192,
                _ => 16000,
            };
            body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
        }
    } else {
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
        body["output_config"] = json!({ "effort": e.as_str() });
        body["fallbacks"] = json!("default");
    }
    body
}

async fn anthropic_turn(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    model: &str,
    effort: Effort,
    system: &str,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    let body = anthropic_body(model, effort, system, history);
    let mut req = client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if body.get("fallbacks").is_some() {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = send_request(req, commands, kept, shutdown, Duration::from_secs(60)).await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(request_error(status, &headers, format!("Anthropic API {status}: {msg}")));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut blocks: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut stop_reason = None;
    let mut terminal = false;
    let mut usage = TokenUsage::default();
    let mut long_writes = 0;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.extend_from_slice(&chunk?);
                for data in sse_events(&mut buf) {
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                    anthropic_usage(&v, &mut usage, &mut long_writes);
                    match v["type"].as_str() {
                        Some("content_block_start") => blocks.push(v["content_block"].clone()),
                        Some("content_block_delta") => {
                            let i = v["index"].as_u64().unwrap_or(0) as usize;
                            let d = &v["delta"];
                            match d["type"].as_str() {
                                Some("text_delta") => {
                                    let t = d["text"].as_str().unwrap_or_default();
                                    text.push_str(t);
                                    append(&mut blocks, i, "text", t);
                                    events.send(AgentEvent::TextDelta(t.into())).await?;
                                }
                                Some("thinking_delta") => {
                                    let t = d["thinking"].as_str().unwrap_or_default();
                                    append(&mut blocks, i, "thinking", t);
                                    events.send(AgentEvent::ReasoningDelta(t.into())).await?;
                                }
                                Some("signature_delta") => append(&mut blocks, i, "signature", d["signature"].as_str().unwrap_or_default()),
                                _ => {}
                            }
                        }
                        Some("message_delta") => stop_reason = v["delta"]["stop_reason"].as_str().map(String::from),
                        Some("message_stop") => terminal = true,
                        Some("error") => bail!("{}", v["error"]["message"].as_str().unwrap_or("stream error")),
                        _ => {}
                    }
                }
            }
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
    if !terminal {
        bail!("The connection closed before the answer finished");
    }
    if !usage.is_empty() {
        let cost = trek_core::pricing::request(model, &AgentId::Direct("anthropic".into()), &usage, long_writes, false);
        events.send(AgentEvent::Usage { model: Some(model.to_string()), tokens: usage, cost }).await?;
    }
    if stop_reason.as_deref() == Some("refusal") {
        bail!("The model declined this request.");
    }
    events.send(AgentEvent::TextDone(text.clone())).await?;
    Ok(TurnResponse {
        text,
        content: Some(json!({ "role": "assistant", "content": blocks })),
        tool_calls: vec![],
    })
}

fn append(blocks: &mut [Value], index: usize, field: &str, s: &str) {
    if let Some(b) = blocks.get_mut(index) {
        if let Some(Value::String(text)) = b.get_mut(field) {
            text.push_str(s);
        } else if let Value::Object(map) = b {
            map.insert(field.to_string(), Value::String(s.to_string()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn openai_turn(
    client: &reqwest::Client,
    agent: &AgentId,
    base_url: &str,
    key: Option<&str>,
    local: bool,
    include_usage: bool,
    model: &str,
    effort: Effort,
    system: &str,
    history: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(history.iter().cloned());
    let mut body = json!({
        "model": model,
        "messages": messages,
        "tools": direct_tools(),
        "stream": true
    });
    if include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !local && effort != Effort::Off {
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High]);
        body["reasoning_effort"] = json!(e.as_str());
    }
    let mut req = client.post(format!("{base_url}/chat/completions")).json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = send_request(req, commands, kept, shutdown, Duration::from_secs(60)).await?;
    let resp = if !resp.status().is_success() && resp.status() == reqwest::StatusCode::BAD_REQUEST {
        // Fallback: retry without tools if rejected
        body.as_object_mut().unwrap().remove("tools");
        let mut retry_req = client.post(format!("{base_url}/chat/completions")).json(&body);
        if let Some(k) = key {
            retry_req = retry_req.bearer_auth(k);
        }
        let retry_resp = send_request(retry_req, commands, kept, shutdown, Duration::from_secs(60)).await?;
        if retry_resp.status().is_success() {
            retry_resp
        } else {
            resp
        }
    } else {
        resp
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or(text);
        return Err(request_error(status, &headers, format!("{status}: {msg}")));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut tool_calls: Vec<AccumulatedToolCall> = Vec::new();
    let mut usage = None;
    let mut terminal = false;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                buf.extend_from_slice(&chunk?);
                for data in sse_events(&mut buf) {
                    if data == "[DONE]" {
                        terminal = true;
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                    usage = openai_usage(&v).or(usage);
                    terminal |= v["choices"].as_array().into_iter().flatten().any(|c| !c["finish_reason"].is_null());
                    let d = &v["choices"][0]["delta"];
                    if let Some(r) = d["reasoning_content"].as_str().or(d["reasoning"].as_str()) {
                        events.send(AgentEvent::ReasoningDelta(r.into())).await?;
                    }
                    if let Some(t) = d["content"].as_str() {
                        text.push_str(t);
                        events.send(AgentEvent::TextDelta(t.into())).await?;
                    }
                    if let Some(calls) = d.get("tool_calls").and_then(|t| t.as_array()) {
                        for call in calls {
                            let idx = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if tool_calls.len() <= idx {
                                tool_calls.resize_with(idx + 1, AccumulatedToolCall::default);
                            }
                            if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                                tool_calls[idx].id.push_str(id);
                            }
                            if let Some(f) = call.get("function") {
                                if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                                    tool_calls[idx].name.push_str(n);
                                }
                                if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
                                    tool_calls[idx].arguments.push_str(a);
                                }
                            }
                        }
                    }
                }
            }
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
    if !terminal {
        bail!("The connection closed before the answer finished");
    }
    if let Some((tokens, reported)) = usage {
        let cost = match reported {
            Some(usd) => Some(UsageCost::reported(usd)),
            None => trek_core::pricing::request(model, agent, &tokens, 0, false),
        };
        events.send(AgentEvent::Usage { model: Some(model.to_string()), tokens, cost }).await?;
    }

    // Fallback: if no native tool calls accumulated, check if text has XML tool calls
    if tool_calls.is_empty() && text.contains("<tool_call>") {
        let (extracted, cleaned) = parse_xml_tool_calls(&text);
        if !extracted.is_empty() {
            tool_calls = extracted;
            text = cleaned;
        }
    }

    for (i, call) in tool_calls.iter_mut().enumerate() {
        if call.id.is_empty() {
            call.id = format!("call_{}_{}", i, next_id());
        }
    }

    events.send(AgentEvent::TextDone(text.clone())).await?;
    Ok(TurnResponse {
        text,
        content: None,
        tool_calls,
    })
}

/// Query live models from a direct provider (OpenRouter, DeepSeek, Ollama, etc.).
pub async fn direct_provider_models(provider_id: &str, api_key: Option<String>) -> Result<Vec<ModelInfo>> {
    let provider = direct_provider(provider_id).with_context(|| format!("unknown provider {provider_id}"))?;
    let key = api_key.or_else(|| secrets::api_key(provider.id));

    if !provider.local && provider.id != "openrouter" && key.is_none() {
        return Ok(vec![]);
    }

    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;

    if provider.id == "ollama" {
        let url = provider.base_url.trim_end_matches("/v1").to_string() + "/api/tags";
        let res = client.get(url).send().await?;
        if !res.status().is_success() {
            bail!("ollama returned {}", res.status());
        }
        let v: Value = res.json().await?;
        let models = v["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["name"].as_str().map(|name| ModelInfo {
                id: name.to_string(),
                name: name.to_string(),
                efforts: vec![],
                tier: 0,
                fast: None,
            }))
            .collect();
        return Ok(models);
    }

    if provider.wire == Wire::Anthropic {
        let mut req = client.get("https://api.anthropic.com/v1/models");
        if let Some(ref k) = key {
            req = req.header("x-api-key", k);
        }
        req = req.header("anthropic-version", "2023-06-01");
        let res = req.send().await?;
        if !res.status().is_success() {
            bail!("{} returned {}", provider.name, res.status());
        }
        let v: Value = res.json().await?;
        return Ok(parse_direct_models(&v));
    }

    let mut req = client.get(format!("{}/models", provider.base_url));
    if let Some(ref k) = key {
        req = req.bearer_auth(k);
    }
    let res = req.send().await?;
    if !res.status().is_success() {
        bail!("{} returned {}", provider.name, res.status());
    }
    let v: Value = res.json().await?;
    Ok(parse_direct_models(&v))
}

pub(crate) fn parse_direct_models(v: &Value) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    let items = v["data"].as_array().or_else(|| v["models"].as_array());
    for m in items.into_iter().flatten() {
        if let Some(raw_id) = m["id"].as_str().or_else(|| m["name"].as_str()) {
            let id = raw_id.strip_prefix("models/").unwrap_or(raw_id).to_string();
            let name = m["display_name"]
                .as_str()
                .or_else(|| m["name"].as_str())
                .map(|n| n.strip_prefix("models/").unwrap_or(n))
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(&id)
                .to_string();
            models.push(ModelInfo {
                id,
                name,
                efforts: vec![],
                tier: models.len().min(255) as u8,
                fast: None,
            });
        }
    }
    models
}

pub fn cache_direct_models(provider_id: &str, models: &[ModelInfo]) {
    if trek_core::paths::isolated() {
        return;
    }
    let cache_dir = trek_core::paths::data_dir().join("direct-models");
    let _ = std::fs::create_dir_all(&cache_dir);
    let file = cache_dir.join(format!("{provider_id}.json"));
    let tmp = cache_dir.join(format!("{provider_id}.json.tmp"));
    if let Ok(json) = serde_json::to_string(models) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
}

pub fn cached_direct_models(provider_id: &str) -> Vec<ModelInfo> {
    if trek_core::paths::isolated() {
        return vec![];
    }
    let file = trek_core::paths::data_dir().join("direct-models").join(format!("{provider_id}.json"));
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn load_all_cached_direct_models() -> HashMap<String, Vec<ModelInfo>> {
    let mut map = HashMap::new();
    for provider in trek_core::catalog::DIRECT_PROVIDERS {
        let models = cached_direct_models(provider.id);
        if !models.is_empty() {
            map.insert(provider.id.to_string(), models);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_request_head(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt as _;
        let mut head = vec![];
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
    }

    async fn sse_server(body: &'static str) -> String {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request_head(&mut socket).await;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
        });
        url
    }

    #[test]
    fn a_429_is_a_limit_and_other_failures_are_not() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        let e = request_error(reqwest::StatusCode::TOO_MANY_REQUESTS, &headers, "429 Too Many Requests: Rate limit exceeded".into());
        let Some(crate::limits::LimitError(limit)) = e.downcast_ref() else { panic!("{e:#}") };
        assert!(limit.resets_at.is_some_and(|at| (at - trek_core::store::now_ms() - 30_000).abs() < 5_000));
        assert_eq!(format!("{e:#}"), "429 Too Many Requests: Rate limit exceeded");
        let e = request_error(reqwest::StatusCode::BAD_REQUEST, &headers, "400 Bad Request: no such model".into());
        assert!(e.downcast_ref::<crate::limits::LimitError>().is_none());
    }

    #[test]
    fn sse_splits_complete_events_with_either_line_ending() {
        let mut buf = b"data: {\"a\":1}\n\ndata: [DONE]\r\n\r\ndata: {\"b\"".to_vec();
        assert_eq!(sse_events(&mut buf), vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert_eq!(buf, b"data: {\"b\"");
        buf.extend_from_slice(b":\"\xc3");
        assert!(sse_events(&mut buf).is_empty());
        buf.extend_from_slice(b"\xa9\"}\r\ndata: second line\r\n\r\n");
        assert_eq!(sse_events(&mut buf), vec!["{\"b\":\"\u{e9}\"}\nsecond line".to_string()]);
    }

    #[test]
    fn appending_deltas_updates_the_existing_string() {
        let mut blocks = vec![json!({"type":"text","text":"one"})];
        append(&mut blocks, 0, "text", " two");
        append(&mut blocks, 0, "text", " three");
        assert_eq!(blocks[0]["text"], "one two three");

        append(&mut blocks, 0, "signature", "sig_xyz");
        assert_eq!(blocks[0]["signature"], "sig_xyz");
        append(&mut blocks, 0, "signature", "_more");
        assert_eq!(blocks[0]["signature"], "sig_xyz_more");
    }

    #[test]
    fn streaming_keeps_non_control_commands_in_order() {
        let mut kept = VecDeque::new();
        let mut shutdown = false;
        for command in [
            Command::SetHandHolding(trek_core::HandHolding::Supervised),
            Command::SetModel { model: "next".into(), effort: Effort::Medium },
            Command::SetModes { plan: true, fast: Some("fast".into()), effort: Effort::High },
            Command::Prompt { text: "steer".into(), images: vec![] },
        ] {
            keep_command(Ok(command), &mut kept, &mut shutdown).unwrap();
        }
        assert!(matches!(kept.pop_front(), Some(Command::SetHandHolding(trek_core::HandHolding::Supervised))));
        assert!(matches!(kept.pop_front(), Some(Command::SetModel { model, .. }) if model == "next"));
        assert!(matches!(kept.pop_front(), Some(Command::SetModes { effort: Effort::High, .. })));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));
        assert!(!shutdown && kept.is_empty());
    }

    #[tokio::test]
    async fn response_headers_keep_commands_and_time_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request_head(&mut socket).await;
            tokio::time::sleep(Duration::from_millis(30)).await;
            use tokio::io::AsyncWriteExt as _;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let (tx, rx) = async_channel::unbounded();
        tx.send(Command::SetModes { plan: true, fast: None, effort: Effort::High }).await.unwrap();
        tx.send(Command::Prompt { text: "steer".into(), images: vec![] }).await.unwrap();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        send_request(reqwest::Client::new().get(&url), &rx, &mut kept, &mut shutdown, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(kept.pop_front(), Some(Command::SetModes { effort: Effort::High, .. })));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let e = send_request(reqwest::Client::new().get(&url), &rx, &mut kept, &mut shutdown, Duration::from_millis(20)).await.unwrap_err();
        assert!(e.to_string().contains("Timed out waiting for response headers"), "{e:#}");
    }

    #[tokio::test]
    async fn openai_stream_requires_a_terminal_marker() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let complete = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n").await;
        let answer = openai_turn(&client, &AgentId::Direct("local".into()), &complete, None, true, false, "m", Effort::Off, "sys", &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap();
        assert_eq!(answer.text, "ok");

        let incomplete = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n").await;
        let e = openai_turn(&client, &AgentId::Direct("local".into()), &incomplete, None, true, false, "m", Effort::Off, "sys", &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap_err();
        assert!(e.to_string().contains("connection closed before the answer finished"), "{e:#}");
    }

    #[tokio::test]
    async fn anthropic_stream_requires_message_stop() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let complete = sse_server(
            "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
        )
        .await;
        let answer = anthropic_turn(&client, &complete, "key", "m", Effort::Off, "sys", &[], &events, &commands, &mut kept, &mut shutdown).await.unwrap();
        assert_eq!(answer.text, "ok");
    }

    #[test]
    fn parses_xml_tool_calls_format() {
        let text = "I will check git status.\n<tool_call>\n<function=bash>\n{\"command\": \"git status\"} \n</tool_call>\nDone.";
        let (calls, cleaned) = parse_xml_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].arguments.contains("git status"));
        assert!(!cleaned.contains("<tool_call>"));
        assert!(cleaned.contains("I will check git status."));
    }

    #[tokio::test]
    async fn tool_execution_file_ops() {
        let temp_dir = std::env::temp_dir().join(format!("trek_test_direct_{}", trek_core::store::now_ms()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cwd = &temp_dir;

        let (tx_events, _rx_events) = async_channel::unbounded();
        let (_tx_cmd, rx_cmd) = async_channel::unbounded();
        let mut kept = VecDeque::new();
        let mut shutdown = false;
        let mut allowed = HashSet::new();

        // 1. write_file
        let write_args = json!({
            "path": "test.txt",
            "content": "line 1\nline 2\nline 3"
        }).to_string();

        let (_out, ok, lines) = execute_tool("write_file", &write_args, cwd, false, HandHolding::Auto, &mut allowed, &tx_events, &rx_cmd, &mut kept, &mut shutdown).await.unwrap();
        assert!(ok);
        assert_eq!(lines, Some((3, 0)));

        // 2. read_file with line range
        let read_args = json!({
            "path": "test.txt",
            "start_line": 2,
            "end_line": 3
        }).to_string();

        let (out, ok, _) = execute_tool("read_file", &read_args, cwd, false, HandHolding::Auto, &mut allowed, &tx_events, &rx_cmd, &mut kept, &mut shutdown).await.unwrap();
        assert!(ok);
        assert!(out.contains("line 2"));
        assert!(!out.contains("line 1"));

        // 3. edit_file
        let edit_args = json!({
            "path": "test.txt",
            "old_str": "line 2",
            "new_str": "line two replacement"
        }).to_string();

        let (_out, ok, lines) = execute_tool("edit_file", &edit_args, cwd, false, HandHolding::Auto, &mut allowed, &tx_events, &rx_cmd, &mut kept, &mut shutdown).await.unwrap();
        assert!(ok);
        assert_eq!(lines, Some((1, 1)));

        let updated = std::fs::read_to_string(cwd.join("test.txt")).unwrap();
        assert!(updated.contains("line two replacement"));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn safe_command_checks() {
        assert!(is_safe_command("git status"));
        assert!(is_safe_command("git log -n 5"));
        assert!(is_safe_command("cargo check"));
        assert!(is_safe_command("ls -la"));
        assert!(!is_safe_command("rm -rf /"));
        assert!(!is_safe_command("git status; rm -rf ."));
        assert!(!is_safe_command("git status && rm -rf ."));
    }
}
