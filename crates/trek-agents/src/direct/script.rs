//! The fifth Direct tool, `script` (the `trek-script` crate): a JavaScript program the model
//! writes that calls the other tools itself, so many calls take one model request and only what
//! it prints goes back. A session offers it when `SessionConfig::script` is on; `tool_defs` lists
//! its definition after the four built in (they stay), and `execute_tool` runs each call with
//! `trek_script::run_script` on a [`ScriptTools`] built on the session's own [`ToolExecutor`],
//! so a script's calls run where the model's do (this machine, or the benchmark's sandbox).
//!
//! Approvals: a script's `bash`, `write` and `edit` go through the session's `HandHolding` like
//! the model's own calls do. `ScriptTools` can't ask the user itself (its calls run on tasks of
//! their own), so it sends each to `execute_tool`, which asks while it waits on the program:
//! one approval per command for now, and "allow for the session" covers later ones as usual.
//! Reads, as with `read_file`, aren't asked about.

use super::executor::{ExecutorGone, ToolExecutor};
use super::tool_path;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use trek_script::async_trait;

/// The tool's definition in the OpenAI function shape `direct_tools` uses (it's turned into
/// the Messages API's shape, `trek_script::anthropic_tool_definition`, for Anthropic).
pub fn tool_def() -> Value {
    json!({ "type": "function", "function": trek_script::tool_definition() })
}

/// A call a script wants to make that needs the user's go-ahead first: `tool` is `bash`,
/// `write_file` or `edit_file`, `target` the command or the path. The answer is whether it may.
pub(crate) struct Approval {
    pub tool: &'static str,
    pub target: String,
    pub reply: tokio::sync::oneshot::Sender<bool>,
}

/// The script's tools (`bash`, `read`, `write`, `edit`), on the session's executor.
pub(crate) struct ScriptTools {
    pub executor: Arc<dyn ToolExecutor>,
    pub cwd: PathBuf,
    /// A command's limit when the script gives none (`DirectOptions::command_timeout`).
    pub command_timeout: Option<Duration>,
    /// Where to ask for approval; `None` when the session has Full access.
    pub approvals: Option<tokio::sync::mpsc::UnboundedSender<Approval>>,
}

impl ScriptTools {
    async fn approve(&self, tool: &'static str, target: &str) -> Result<(), String> {
        let Some(approvals) = &self.approvals else {
            return Ok(());
        };
        let (reply, answer) = tokio::sync::oneshot::channel();
        approvals
            .send(Approval {
                tool,
                target: target.to_string(),
                reply,
            })
            .map_err(|_| "the turn was stopped".to_string())?;
        match answer.await {
            Ok(true) => Ok(()),
            Ok(false) => Err("Permission denied by user".into()),
            Err(_) => Err("the turn was stopped".into()),
        }
    }

    async fn read_text(&self, path: &str) -> Result<String, String> {
        match self.executor.read(&tool_path(path, &self.cwd)).await {
            Ok(Some(bytes)) => {
                String::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8 text"))
            }
            Ok(None) => Err(format!("File not found: {path}")),
            Err(e) => Err(format!("{e:#}")),
        }
    }

    async fn write_text(&self, path: &str, text: &str) -> Result<(), String> {
        self.executor
            .write(&tool_path(path, &self.cwd), text.as_bytes())
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

#[async_trait]
impl trek_script::ScriptHost for ScriptTools {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        let text = |k: &str| {
            args[k]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{k} is required"))
        };
        match tool {
            "bash" => {
                // Trimmed as the model's own `bash` is, so "allow for the session" keys match.
                let command = text("command")?.trim().to_string();
                if command.is_empty() {
                    return Err("empty command".into());
                }
                self.approve("bash", &command).await?;
                let timeout = args["timeout_s"]
                    .as_f64()
                    .map(Duration::from_secs_f64)
                    .or(self.command_timeout);
                let ran = self
                    .executor
                    .exec(&command, &self.cwd, timeout)
                    .await
                    .map_err(|e| match e.downcast_ref::<ExecutorGone>() {
                        Some(gone) => gone.to_string(),
                        None => format!("Failed to execute command: {e:#}"),
                    })?;
                Ok(json!({
                    "stdout": ran.stdout,
                    "stderr": ran.stderr,
                    "exit_code": ran.exit_code,
                    "timed_out": ran.timed_out,
                }))
            }
            "read" => {
                let content = self.read_text(&text("path")?).await?;
                let start = args["start_line"].as_u64().unwrap_or(1).max(1) as usize;
                let end = args["end_line"].as_u64().map_or(usize::MAX, |e| e as usize);
                let lines: Vec<&str> = content
                    .lines()
                    .skip(start - 1)
                    .take(end.saturating_sub(start - 1))
                    .collect();
                // The whole file keeps its last newline; a slice is its lines.
                let content = if start == 1 && end == usize::MAX {
                    content.clone()
                } else {
                    lines.join("\n")
                };
                Ok(json!({ "content": content }))
            }
            "write" => {
                let path = text("path")?;
                self.approve("write_file", &path).await?;
                self.write_text(&path, &text("content")?).await?;
                Ok(json!({}))
            }
            "edit" => {
                let path = text("path")?;
                let (old, new) = (text("old")?, text("new")?);
                if old.is_empty() {
                    return Err("old is required".into());
                }
                self.approve("edit_file", &path).await?;
                let content = self.read_text(&path).await?;
                match content.matches(old.as_str()).count() {
                    1 => {
                        self.write_text(&path, &content.replacen(&old, &new, 1))
                            .await?
                    }
                    0 => return Err(format!("old text was not found in {path}")),
                    n => {
                        return Err(format!(
                            "old text matched {n} times in {path}; give more context"
                        ));
                    }
                }
                Ok(json!({}))
            }
            other => Err(format!(
                "no tool named {other}; a script has bash, read, write and edit"
            )),
        }
    }
}
