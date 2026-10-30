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
//!
//! Limits: a program runs within the session's deadline as well as its own wall clock, and
//! makes no call once the deadline has passed. An executor that's gone (`ExecutorGone`) ends
//! the program and the turn, as it does for the model's own calls, instead of failing call
//! after call.

use super::executor::{ExecutorGone, ToolExecutor};
use super::tool_path;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
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

/// What a script's calls need of `execute_tool`, which waits on the program.
pub(crate) enum ToSession {
    Approval(Approval),
    /// The executor is gone (`ExecutorGone`): the program is ended and the turn with it.
    Gone(String),
}

/// The longest a command may be given when the session sets no limit of its own.
const LONGEST_COMMAND: Duration = Duration::from_secs(24 * 3600);

/// The script's tools (`bash`, `read`, `write`, `edit`), on the session's executor.
pub(crate) struct ScriptTools {
    executor: Arc<dyn ToolExecutor>,
    cwd: PathBuf,
    /// A command's limit when the script gives none, and the most it may give
    /// (`DirectOptions::command_timeout`).
    command_timeout: Option<Duration>,
    /// Whether `bash`, `write` and `edit` ask first (below Full access).
    ask: bool,
    /// No new call after this (`DirectOptions::deadline`).
    deadline: Option<Instant>,
    session: tokio::sync::mpsc::UnboundedSender<ToSession>,
    /// Why the executor went, once it has: every later call fails at once.
    gone: OnceLock<String>,
}

impl ScriptTools {
    pub(crate) fn new(
        executor: Arc<dyn ToolExecutor>,
        cwd: PathBuf,
        command_timeout: Option<Duration>,
        ask: bool,
        deadline: Option<Instant>,
        session: tokio::sync::mpsc::UnboundedSender<ToSession>,
    ) -> Self {
        ScriptTools {
            executor,
            cwd,
            command_timeout,
            ask,
            deadline,
            session,
            gone: OnceLock::new(),
        }
    }

    async fn approve(&self, tool: &'static str, target: &str) -> Result<(), String> {
        if !self.ask {
            return Ok(());
        }
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.session
            .send(ToSession::Approval(Approval {
                tool,
                target: target.to_string(),
                reply,
            }))
            .map_err(|_| "the turn was stopped".to_string())?;
        match answer.await {
            Ok(true) => Ok(()),
            Ok(false) => Err("Permission denied by user".into()),
            Err(_) => Err("the turn was stopped".into()),
        }
    }

    /// An executor error as the program gets it. One that says the executor is gone is told to
    /// the session too, which ends the program.
    fn failed(&self, e: anyhow::Error, what: &str) -> String {
        match e.downcast_ref::<ExecutorGone>() {
            Some(gone) => {
                let reason = gone.to_string();
                if self.gone.set(reason.clone()).is_ok() {
                    let _ = self.session.send(ToSession::Gone(reason.clone()));
                }
                reason
            }
            None => format!("{what}{e:#}"),
        }
    }

    async fn read_text(&self, path: &str) -> Result<String, String> {
        match self.executor.read(&tool_path(path, &self.cwd)).await {
            Ok(Some(bytes)) => {
                String::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8 text"))
            }
            Ok(None) => Err(format!("File not found: {path}")),
            Err(e) => Err(self.failed(e, "")),
        }
    }

    async fn write_text(&self, path: &str, text: &str) -> Result<(), String> {
        self.executor
            .write(&tool_path(path, &self.cwd), text.as_bytes())
            .await
            .map_err(|e| self.failed(e, ""))
    }

    /// The limit for a script's command: its own `timeout_s` (seconds, at most the session's
    /// command limit), else the session's.
    fn command_limit(&self, given: &Value) -> Result<Option<Duration>, String> {
        if given.is_null() {
            return Ok(self.command_timeout);
        }
        let most = self.command_timeout.unwrap_or(LONGEST_COMMAND);
        match given.as_f64() {
            Some(s) if s.is_finite() && s >= 0.0 && s <= most.as_secs_f64() => {
                Ok(Some(Duration::from_secs_f64(s)))
            }
            _ => Err(format!(
                "timeout must be a number of seconds from 0 to {}",
                most.as_secs()
            )),
        }
    }
}

/// Lines `start..=end` (1-based) of `content`, as `read_file` takes them: an end before the
/// start, or a start past the last line, is an error. The whole file keeps its last newline.
fn line_range(content: &str, start: Option<u64>, end: Option<u64>) -> Result<String, String> {
    if start.is_none() && end.is_none() {
        return Ok(content.to_string());
    }
    let lines: Vec<&str> = content.lines().collect();
    let start = start.unwrap_or(1).max(1) as usize;
    let end = end.map_or(lines.len(), |e| (e as usize).min(lines.len()));
    if lines.is_empty() {
        return Ok(String::new());
    }
    if start > lines.len() {
        return Err(format!(
            "Start line {start} is beyond end of file ({} lines)",
            lines.len()
        ));
    }
    if end < start {
        return Err(format!("End line {} is before start line {start}", end));
    }
    Ok(lines[start - 1..end].join("\n"))
}

#[async_trait]
impl trek_script::ScriptHost for ScriptTools {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        if let Some(reason) = self.gone.get() {
            return Err(reason.clone());
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err("the time limit was reached; no more calls".into());
        }
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
                let timeout = self.command_limit(&args["timeout_s"])?;
                self.approve("bash", &command).await?;
                let ran = self
                    .executor
                    .exec(&command, &self.cwd, timeout)
                    .await
                    .map_err(|e| self.failed(e, "Failed to execute command: "))?;
                Ok(json!({
                    "stdout": ran.stdout,
                    "stderr": ran.stderr,
                    "exit_code": ran.exit_code,
                    "timed_out": ran.timed_out,
                }))
            }
            "read" => {
                let content = self.read_text(&text("path")?).await?;
                let content = line_range(
                    &content,
                    args["start_line"].as_u64(),
                    args["end_line"].as_u64(),
                )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct::executor::{ExecOutput, LocalExecutor};
    use futures::future::BoxFuture;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use trek_script::ScriptHost as _;

    fn tools(
        executor: Arc<dyn ToolExecutor>,
        command_timeout: Option<Duration>,
        deadline: Option<Instant>,
    ) -> (ScriptTools, tokio::sync::mpsc::UnboundedReceiver<ToSession>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cwd = std::env::temp_dir();
        (
            ScriptTools::new(executor, cwd, command_timeout, false, deadline, tx),
            rx,
        )
    }

    /// Every call fails as a sandbox that's gone does; counts the calls that reached it.
    #[derive(Default)]
    struct Gone(AtomicU32);

    impl ToolExecutor for Gone {
        fn exec<'a>(
            &'a self,
            _: &'a str,
            _: &'a Path,
            _: Option<Duration>,
        ) -> BoxFuture<'a, anyhow::Result<ExecOutput>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ExecutorGone("the sandbox hung up".into()).into()) })
        }
        fn read<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, anyhow::Result<Option<Vec<u8>>>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ExecutorGone("the sandbox hung up".into()).into()) })
        }
        fn write<'a>(&'a self, _: &'a Path, _: &'a [u8]) -> BoxFuture<'a, anyhow::Result<()>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ExecutorGone("the sandbox hung up".into()).into()) })
        }
    }

    #[tokio::test]
    async fn a_scripts_command_timeout_is_checked() {
        let (t, _rx) = tools(Arc::new(LocalExecutor), Some(Duration::from_secs(60)), None);
        for bad in [json!(-1), json!(61), json!("10"), json!(1e300)] {
            let e = t
                .call("bash", json!({ "command": "true", "timeout_s": bad }))
                .await
                .unwrap_err();
            assert_eq!(
                e, "timeout must be a number of seconds from 0 to 60",
                "{bad}"
            );
        }
        assert_eq!(
            t.command_limit(&json!(2.5)).unwrap(),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(
            t.command_limit(&Value::Null).unwrap(),
            Some(Duration::from_secs(60))
        );
        #[cfg(unix)]
        {
            let ran = t
                .call("bash", json!({ "command": "echo ok", "timeout_s": 5 }))
                .await
                .unwrap();
            assert_eq!(ran["stdout"], "ok\n");
        }
    }

    #[test]
    fn a_scripts_read_range_is_checked_like_read_file() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(line_range(text, None, None).unwrap(), text);
        assert_eq!(line_range(text, Some(2), None).unwrap(), "two\nthree");
        assert_eq!(line_range(text, Some(2), Some(2)).unwrap(), "two");
        assert_eq!(line_range(text, None, Some(9)).unwrap(), "one\ntwo\nthree");
        assert_eq!(
            line_range(text, Some(3), Some(2)).unwrap_err(),
            "End line 2 is before start line 3"
        );
        assert_eq!(
            line_range(text, Some(5), None).unwrap_err(),
            "Start line 5 is beyond end of file (3 lines)"
        );
        assert_eq!(line_range("", Some(2), None).unwrap(), "");
    }

    #[tokio::test]
    async fn a_gone_executor_is_told_once_and_ends_every_later_call() {
        let gone = Arc::new(Gone::default());
        let (t, mut rx) = tools(gone.clone(), None, None);
        let e = t
            .call("bash", json!({ "command": "ls" }))
            .await
            .unwrap_err();
        assert_eq!(e, "the sandbox hung up");
        assert!(matches!(rx.try_recv(), Ok(ToSession::Gone(r)) if r == "the sandbox hung up"));
        for _ in 0..5 {
            assert!(t.call("read", json!({ "path": "a" })).await.is_err());
        }
        // Only the first call reached the executor, and the session heard once.
        assert_eq!(gone.0.load(Ordering::SeqCst), 1);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn no_call_starts_past_the_deadline() {
        let gone = Arc::new(Gone::default());
        let (t, _rx) = tools(gone.clone(), None, Some(Instant::now()));
        let e = t
            .call("bash", json!({ "command": "ls" }))
            .await
            .unwrap_err();
        assert!(e.contains("time limit"), "{e}");
        assert_eq!(gone.0.load(Ordering::SeqCst), 0);
    }
}
