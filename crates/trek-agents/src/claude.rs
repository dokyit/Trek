//! Claude Code via the user's own `claude` binary (stream-json + stdio control protocol).
//! Trek never reads Claude credentials; the CLI handles its own login.

use crate::{AgentEvent, Billing, Command, Decision, GroupChild, SessionConfig, StderrTail, Step, clip, load_image, mcp_servers_json, plan_row, plan_title};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncWriteExt, BufReader};
use trek_core::{Effort, TokenUsage, UsageCost, detect};

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
        "Monitor" => ("Monitor".into(), if input["description"].is_string() { s("description") } else { s("command") }),
        other => (other.to_string(), clip(&input.to_string(), 200)),
    }
}

/// Lines an edit call will add and remove, from its input (refined by the result's patch).
fn edit_lines(name: &str, input: &Value) -> Option<(u32, u32)> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    match name {
        "Edit" => Some(crate::line_changes(&s(input, "old_string"), &s(input, "new_string"))),
        "MultiEdit" => Some(input["edits"].as_array().into_iter().flatten().fold((0, 0), |(a, r), e| {
            let (a2, r2) = crate::line_changes(&s(e, "old_string"), &s(e, "new_string"));
            (a + a2, r + r2)
        })),
        "Write" => Some((input["content"].as_str().unwrap_or_default().lines().count() as u32, 0)),
        _ => None,
    }
}

/// Lines an edit's result says it added and removed: its `structuredPatch`, or a new file's
/// content. Stream-json calls the field `tool_use_result`; session files, `toolUseResult`.
fn result_lines(v: &Value) -> Option<(u32, u32)> {
    let r = if v["tool_use_result"].is_object() { &v["tool_use_result"] } else { &v["toolUseResult"] };
    if let Some(hunks) = r["structuredPatch"].as_array().filter(|h| !h.is_empty()) {
        let lines = hunks.iter().flat_map(|h| h["lines"].as_array().into_iter().flatten()).filter_map(|l| l.as_str());
        return Some(lines.fold((0, 0), |(a, d), l| match l.chars().next() {
            Some('+') => (a + 1, d),
            Some('-') => (a, d + 1),
            _ => (a, d),
        }));
    }
    (r["type"] == "create").then(|| (r["content"].as_str().unwrap_or_default().lines().count() as u32, 0))
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
    // AskUserQuestion reads its answers from the input it gets back. Indexing a non-object to
    // set a key panics, so anything else is replaced by an object.
    let mut input = request["input"].as_object().cloned().unwrap_or_default();
    input.insert("answers".into(), Value::Object(answers.into_iter().map(|(q, a)| (q, Value::String(a))).collect()));
    let input = Value::Object(input);
    control_response(request_id, json!({ "behavior": "allow", "updatedInput": input }))
}

/// The answer to a request to use one of Trek's orchestration tools (`trek-orchestrate`): allowed.
fn trek_tool_allowed(v: &Value) -> Option<Value> {
    let r = &v["request"];
    let ours = v["type"] == "control_request" && r["subtype"] == "can_use_tool" && r["tool_name"].as_str()?.starts_with("mcp__trek-orchestrate__");
    ours.then(|| control_response(v["request_id"].as_str().unwrap_or_default(), json!({ "behavior": "allow", "updatedInput": r["input"] })))
}

/// Claude's name for effort `e`; `None` leaves it to Claude.
fn effort_level(e: Effort) -> Option<&'static str> {
    (e != Effort::Off).then(|| e.clamp_to(&[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]).as_str())
}

/// Requests putting a running session in plan mode, fast mode and effort `want`, from `had`
/// (each as `(plan, fast, effort)`); `mode` is the access level to go back to out of plan mode.
/// Plan mode is a permission mode, the others flag settings, all of which Claude takes mid-session.
fn modes_requests(ctl: &mut Control, had: (bool, bool, Effort), want: (bool, bool, Effort), mode: &str) -> Vec<Value> {
    let mut out = vec![];
    if had.0 != want.0 {
        out.push(ctl.request("set_permission_mode", json!({ "mode": if want.0 { "plan" } else { mode } })));
    }
    let mut flags = serde_json::Map::new();
    if had.1 != want.1 {
        flags.insert("fastMode".into(), if want.1 { Value::Bool(true) } else { Value::Null });
    }
    if had.2 != want.2 {
        flags.insert("effortLevel".into(), effort_level(want.2).map_or(Value::Null, |e| Value::String(e.into())));
    }
    if !flags.is_empty() {
        out.push(ctl.request("apply_flag_settings", json!({ "settings": flags })));
    }
    out
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
        // Each message is echoed as Claude takes it in: see `Turns`.
        "--replay-user-messages",
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
    if let Some(effort) = effort_level(config.effort) {
        flag(&mut args, "--effort", effort);
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
    let mut settings = serde_json::Map::new();
    if config.fast.is_some() {
        settings.insert("fastMode".into(), json!(true));
    }
    // Denied tools stay denied whatever the user's allow rules say; asking to run anything else
    // still comes to Trek, which declines it for a session that only advises.
    if config.read_only {
        flag(&mut args, "--disallowedTools", "Edit,MultiEdit,Write,NotebookEdit");
        settings.insert("sandbox".into(), read_only_sandbox(config));
    }
    if !settings.is_empty() {
        flag(&mut args, "--settings", &Value::Object(settings).to_string());
    }
    // Added to Claude Code's own system prompt, not in place of it, at every launch: a resumed
    // session hears it again without it piling up in the conversation.
    if let Some(notes) = config.instructions.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        flag(&mut args, "--append-system-prompt", notes);
    }
    for dir in config.read_dirs.iter().filter(|d| d.is_dir()) {
        flag(&mut args, "--add-dir", &dir.display().to_string());
    }
    args
}

/// `mcpServers` for `--mcp-config`, with a per-server `timeout` (ms) where a server's calls may
/// run long: it lifts both Claude Code's limit on one call and its idle limit (half an hour).
fn claude_mcp_servers(servers: &[crate::McpServer]) -> Value {
    let mut out = mcp_servers_json(servers);
    for s in servers {
        if let (Some(secs), Some(entry)) = (s.tool_timeout_secs, out.get_mut(&s.name)) {
            entry["timeout"] = json!(secs * 1000);
        }
    }
    out
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

/// The shell of a session that only advises, shut in Claude Code's own sandbox: the system
/// refuses its commands' writes to the folder it works in (and the ones it may read), whatever
/// they are. Turning the edit tools off isn't enough: a command the user's settings allow
/// (`npm run …`, `git commit`) runs without asking Trek, and a shell can write anything. With
/// no way round (`allowUnsandboxedCommands`), a repository's settings can't loosen it either.
fn read_only_sandbox(config: &SessionConfig) -> Value {
    let folders: Vec<String> = std::iter::once(&config.cwd).chain(config.read_dirs.iter()).map(|d| d.display().to_string()).collect();
    json!({ "enabled": true, "allowUnsandboxedCommands": false, "filesystem": { "denyWrite": folders } })
}

/// The CLI's answer to `--resume` with a session it doesn't have (deleted by hand, or cleaned up
/// after `cleanupPeriodDays`): an error result before the session starts, then it exits.
fn session_missing(v: &Value) -> bool {
    v["type"] == "result"
        && v["is_error"] == true
        && v["errors"].as_array().into_iter().flatten().any(|e| e.as_str().is_some_and(|e| e.starts_with("No conversation found")))
}

/// A running `claude` process.
struct Cli {
    child: GroupChild,
    stdin: tokio::process::ChildStdin,
    stdout: crate::ProtocolLines<BufReader<tokio::process::ChildStdout>>,
    stderr: StderrTail,
    /// The notes' file, when they went in one (`notes_in_file`): read when it likes, so kept.
    _notes: Option<TempFile>,
}

/// Claude Code from npm is `claude.cmd`, which Windows runs through cmd.exe, and cmd.exe can't be
/// given an argument with a line break (std won't start it with one; see `detect::batch_args_problem`).
/// The user's notes often have them, so for `claude.cmd` they go in a file, as
/// `--append-system-prompt-file`; the same words reach Claude either way. The file is returned to
/// be kept as long as the process.
fn notes_in_file(args: &mut [String]) -> Result<Option<TempFile>> {
    let Some(at) = args.iter().position(|a| a == "--append-system-prompt") else { return Ok(None) };
    let file = TempFile::write("notes", &args[at + 1])?;
    args[at] = "--append-system-prompt-file".into();
    args[at + 1] = file.0.display().to_string();
    Ok(Some(file))
}

impl Cli {
    fn spawn(bin: &std::path::Path, config: &SessionConfig, mcp_file: Option<&TempFile>) -> Result<Cli> {
        let mut args = cli_args(config);
        let notes = if detect::runs_through_cmd(bin) { notes_in_file(&mut args)? } else { None };
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(args);
        if let Some(file) = mcp_file {
            cmd.arg("--mcp-config").arg(&file.0);
        }
        if let Some(problem) = detect::batch_args_problem(bin, cmd.as_std().get_args()) {
            anyhow::bail!("Claude Code can't start: {problem}");
        }
        cmd.current_dir(&config.cwd)
            .env("PATH", detect::login_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = crate::spawn_group(&mut cmd).context("failed to start claude")?;
        let stdin = child.stdin.take().unwrap();
        let stdout = crate::ProtocolLines::new(BufReader::new(child.stdout.take().unwrap()));
        let stderr = StderrTail::capture(child.stderr.take().unwrap(), "claude");
        Ok(Cli { child, stdin, stdout, stderr, _notes: notes })
    }

    /// Send the `initialize` control request; its id, to know the response.
    async fn initialize(&mut self, ctl: &mut Control) -> Result<String> {
        let init = ctl.request("initialize", json!({}));
        write_line(&mut self.stdin, &init).await?;
        Ok(init["request_id"].as_str().unwrap_or_default().to_string())
    }

    /// End the process: its stdin first, the cue to exit and on Windows the only gentle one
    /// (see `GroupChild::terminate`), then the group.
    async fn stop(self) {
        let Cli { mut child, stdin, .. } = self;
        drop(stdin);
        child.terminate().await;
    }
}

/// Which `result` ends the turn the app sees. A message sent while a turn runs joins that turn
/// (Claude takes it in at its next step), or runs after it as a turn of its own with its own
/// `result` when the turn was already ending. Claude echoes each message as it takes it in
/// (`--replay-user-messages`), so a result that leaves a message unread isn't the end yet.
#[derive(Default)]
struct Turns {
    /// Messages written that Claude hasn't echoed yet, and whether each was sent while a turn
    /// ran. Slash commands aren't echoed (some never reach the model), so they aren't counted.
    unread: Vec<(String, bool)>,
    /// A result held back for unread messages: when to stop waiting for Claude to take them in
    /// (they may never come, and the turn mustn't hang).
    waiting: Option<tokio::time::Instant>,
    /// The limit Claude's last `rate_limit_event` said was hit: when it resets, and which it is.
    rejected: Option<(Option<i64>, crate::LimitScope)>,
    /// This turn has reported its limit (`AgentEvent::LimitReached`).
    limited: bool,
    /// Each model's tokens and `costUSD` as the last `result` counted them: `modelUsage` runs for
    /// the whole session (across resumes too), so a turn's share is how far it moved.
    models: HashMap<String, (TokenUsage, f64)>,
    /// The session was resumed (or forked) into this process: its first `modelUsage` may carry
    /// the session's earlier turns, which the ledger tells apart from this one's.
    resumed: bool,
    /// The session resumed (`--resume`), whose totals the ledger may hold under its id when a
    /// fork reports a new one.
    resumed_from: Option<String>,
    /// Where each session's `models` are kept between processes (`UsageLedger`); none in tests
    /// that don't ask for one.
    ledger: Option<PathBuf>,
    /// The model the session said it runs (`system init`).
    model: Option<String>,
    /// What runs in the background, and what started it.
    background: BackgroundTasks,
}

/// Claude's background tasks as Trek shows them: the set `background_tasks_changed` reports
/// (all of it, each time), with what `task_started` said about each: the call that started it
/// (a `Monitor` call starts a shell task as `Bash` does) and whether it was detached from the
/// start (a sub-agent run in the background, whose answer comes in its notification).
#[derive(Default)]
struct BackgroundTasks {
    /// The live set as last reported: (task id, task type, description), ambient ones left out.
    live: Vec<(String, String, String)>,
    /// What `task_started` said, by task id: the call, and whether it's a sub-agent that started
    /// detached. Kept until the task's notification (which comes after it has left the live set).
    started: HashMap<String, (String, bool)>,
    /// `Monitor` calls, by id.
    monitors: HashSet<String>,
}

impl BackgroundTasks {
    /// The live set (`background_tasks_changed`).
    fn set(&mut self, v: &Value) {
        self.live = v["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            // Housekeeping (watchers Claude Code starts itself) isn't work the user would see.
            .filter(|t| t["ambient"] != true)
            .filter_map(|t| Some((t["task_id"].as_str()?.to_string(), t["task_type"].as_str().unwrap_or_default().to_string(), t["description"].as_str().unwrap_or_default().to_string())))
            .collect();
    }

    /// A task started; `true` when it's in the live set (its kind may read differently now).
    fn started(&mut self, v: &Value) -> bool {
        let (Some(id), Some(call)) = (v["task_id"].as_str(), v["tool_use_id"].as_str()) else { return false };
        // A sub-agent's own commands run inside it, not for the session.
        if v["owned_by_subagent"] == true {
            return false;
        }
        self.started.insert(id.to_string(), (call.to_string(), v["is_backgrounded"] == true && v["task_type"] == "local_agent"));
        self.live.iter().any(|(t, ..)| t == id)
    }

    /// The call a sub-agent that ran detached was started by, once it has ended (`task_id`).
    fn detached_agent(&self, task: &str) -> Option<&str> {
        self.started.get(task).filter(|(_, detached)| *detached).map(|(call, _)| call.as_str())
    }

    fn list(&self) -> Vec<crate::BackgroundTask> {
        self.live
            .iter()
            .map(|(id, kind, description)| {
                let call = self.started.get(id).map(|(c, _)| c.clone());
                let kind = match kind.as_str() {
                    "local_bash" if call.as_ref().is_some_and(|c| self.monitors.contains(c)) => crate::BackgroundKind::Monitor,
                    "local_bash" => crate::BackgroundKind::Shell,
                    "local_agent" | "remote_agent" | "in_process_teammate" => crate::BackgroundKind::Agent,
                    _ => crate::BackgroundKind::Other,
                };
                let readable = matches!(kind, crate::BackgroundKind::Shell | crate::BackgroundKind::Monitor);
                crate::BackgroundTask { id: id.clone(), kind, title: description.clone(), call, readable, stoppable: true }
            })
            .collect()
    }
}

/// How long a held result waits for Claude to start on the messages after it. It starts within
/// a fraction of a second.
const STEER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Each Claude session's running per-model totals (`Turns::models`) as its last `result` left
/// them, one small file per session under Trek's data folder. A resumed process starts from
/// them, so its first turn is counted from Claude Code's own figures, sub-agents included.
struct UsageLedger;

impl UsageLedger {
    /// Files of sessions untouched this long are dropped: they're unlikely to be resumed, and a
    /// resume without one only prices its first turn itself.
    const KEEP: std::time::Duration = std::time::Duration::from_secs(90 * 86_400);

    fn dir() -> PathBuf {
        trek_core::paths::data_dir().join("claude-usage")
    }

    fn path(dir: &std::path::Path, session: &str) -> PathBuf {
        let name: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        dir.join(format!("{name}.json"))
    }

    fn load(dir: &std::path::Path, session: &str) -> Option<HashMap<String, (TokenUsage, f64)>> {
        Self::prune(dir);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(Self::path(dir, session)).ok()?).ok()?;
        let n = |m: &Value, k: &str| m[k].as_u64().unwrap_or(0);
        Some(
            v.as_object()?
                .iter()
                .map(|(model, m)| {
                    let tokens = TokenUsage { input: n(m, "input"), output: n(m, "output"), cache_read: n(m, "cache_read"), cache_write: n(m, "cache_write") };
                    (model.clone(), (tokens, m["cost"].as_f64().unwrap_or(0.0)))
                })
                .collect(),
        )
    }

    fn save(dir: &std::path::Path, session: &str, models: &HashMap<String, (TokenUsage, f64)>) {
        let v: serde_json::Map<String, Value> = models
            .iter()
            .map(|(model, (t, cost))| (model.clone(), json!({ "input": t.input, "output": t.output, "cache_read": t.cache_read, "cache_write": t.cache_write, "cost": cost })))
            .collect();
        if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(Self::path(dir, session), Value::Object(v).to_string())) {
            tracing::warn!("claude: couldn't keep the session's usage totals: {e}");
        }
    }

    fn prune(dir: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let stale = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Self::KEEP);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// How full each of the plan's windows is, from a `rate_limit_event` that lets the turn go on:
/// every window Claude lists (`unifiedWindows`), else the one the event is about. None once
/// the limit is hit (the turn's failure says that) or while overage pays for what's past it.
fn windows_used(info: &Value) -> Vec<AgentEvent> {
    if info["status"] == "rejected" || info["isUsingOverage"] == true {
        return vec![];
    }
    let used = |kind: &str, w: &Value| {
        let percent = (w["utilization"].as_f64()? * 100.0) as f32;
        Some(AgentEvent::LimitUsed { scope: crate::limits::claude_scope(kind), percent, resets_at: w["resetsAt"].as_i64().map(|s| s * 1000) })
    };
    match info["unifiedWindows"].as_object() {
        Some(windows) => windows.iter().filter_map(|(kind, w)| used(kind, w)).collect(),
        None => used(info["rateLimitType"].as_str().unwrap_or_default(), info).into_iter().collect(),
    }
}

impl Turns {
    fn sent(&mut self, text: &str, mid_turn: bool) {
        if !text.trim_start().starts_with('/') {
            self.unread.push((text.to_string(), mid_turn));
        }
    }

    /// The text of a message Claude echoed.
    fn echo_text(v: &Value) -> String {
        match &v["message"]["content"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        }
    }

    /// Events for one line of the CLI's output: `translate`'s, less a `TurnComplete` that
    /// messages sent since still belong to.
    fn step(&mut self, v: &Value, pending: &mut HashMap<String, Value>, streamed_text: &mut bool) -> Vec<AgentEvent> {
        if v["type"] == "user" && v["isReplay"] == true {
            let text = Self::echo_text(v);
            if let Some(i) = self.unread.iter().position(|(t, _)| *t == text) {
                self.unread.remove(i);
                // Claude is on it: the turn goes on until its result.
                self.waiting = None;
            }
            return vec![];
        }
        if v["type"] == "system" && v["subtype"] == "init" {
            // The next turn has begun.
            self.waiting = None;
            if let Some(m) = v["model"].as_str().filter(|m| !m.is_empty()) {
                self.model = Some(m.to_string());
            }
        }
        if v["type"] == "rate_limit_event" {
            let info = &v["rate_limit_info"];
            // Past the limit but carrying on, paid as overage: no limit to stop at.
            let rejected = info["status"] == "rejected" && info["isUsingOverage"] != true;
            self.rejected = rejected.then(|| (info["resetsAt"].as_i64().map(|s| s * 1000), crate::limits::claude_scope(info["rateLimitType"].as_str().unwrap_or_default())));
            return windows_used(info);
        }
        if v["type"] == "system" && v["subtype"] == "background_tasks_changed" {
            self.background.set(v);
            return vec![AgentEvent::Background(self.background.list())];
        }
        // A sub-agent at work: the tools it calls are its row's activity.
        if v["type"] == "assistant"
            && let Some(task) = v["parent_tool_use_id"].as_str()
        {
            return v["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| {
                    let (title, detail) = tool_title(b["name"].as_str().unwrap_or("tool"), &b["input"]);
                    AgentEvent::TaskStep { task: task.to_string(), title, detail }
                })
                .collect();
        }
        if v["type"] == "assistant" {
            for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use" && b["name"] == "Monitor") {
                self.background.monitors.insert(b["id"].as_str().unwrap_or_default().to_string());
            }
        }
        let mut out = translate(v, pending, streamed_text);
        if v["type"] == "system" && v["subtype"] == "task_started" && self.background.started(v) {
            out.push(AgentEvent::Background(self.background.list()));
        }
        // A sub-agent that ran detached answers in its notification: that's its row's output.
        if v["type"] == "system" && v["subtype"] == "task_notification" {
            let task = v["task_id"].as_str().unwrap_or_default();
            if let Some(call) = self.background.detached_agent(task).map(str::to_string) {
                out.push(AgentEvent::ToolFinished { id: call, output: clip(v["summary"].as_str().unwrap_or_default(), 8000), ok: v["status"] == "completed" });
            }
            if let Some((call, _)) = self.background.started.remove(task) {
                self.background.monitors.remove(&call);
            }
            // Over, whether or not a new live set follows: a task that's still listed after its
            // notification would show (and hold an update) as running for good.
            if self.background.live.iter().any(|(t, ..)| t == task) {
                self.background.live.retain(|(t, ..)| t != task);
                out.push(AgentEvent::Background(self.background.list()));
            }
        }
        // The limit message gets the reset and window Claude reported for it, if it did.
        for ev in out.iter_mut() {
            if let AgentEvent::LimitReached { resets_at, scope, .. } = ev {
                self.limited = true;
                if let Some((at, kind)) = self.rejected.clone() {
                    *resets_at = at.or(*resets_at);
                    if *scope == crate::LimitScope::Other {
                        *scope = kind;
                    }
                }
            }
        }
        if v["type"] == "result" {
            // A turn that failed at a limit without saying so in a message of its own.
            if v["is_error"] == true && !std::mem::take(&mut self.limited) && result_error(v) != "Interrupted" {
                let text = result_error(v);
                let limit = match self.rejected.clone() {
                    Some((resets_at, scope)) => Some(crate::Limit { message: text.clone(), resets_at: resets_at.or_else(|| crate::limits::reset_from_text(&text, trek_core::store::now_ms())), scope }),
                    None => crate::Limit::from_text(&text, trek_core::store::now_ms()),
                };
                if let Some(limit) = limit {
                    let at = out.iter().position(|e| matches!(e, AgentEvent::TurnComplete { .. })).unwrap_or(out.len());
                    out.insert(at, limit.event());
                }
            }
            self.limited = false;
            // Tokens the turn used go out first, whether or not the result ends the turn here.
            let used = self.usage(v);
            out.splice(0..0, used);
            // A stopped or failed turn ends here whatever was sent: Claude may drop what it hadn't
            // read yet, and a turn that waited on it would never end.
            if v["is_error"] == true || !self.unread.iter().any(|(_, mid_turn)| *mid_turn) {
                self.unread.clear();
                self.waiting = None;
            } else {
                self.waiting = Some(tokio::time::Instant::now() + STEER_WAIT);
                out.retain(|e| !matches!(e, AgentEvent::TurnComplete { .. }));
            }
        }
        out
    }

    /// What a `result` says its turn used, per model (sub-agents and Claude Code's own helper
    /// calls may run on another one), and what that cost by Claude Code's own pricing
    /// (`costUSD`, which knows its cache tiers and fast mode). Without `modelUsage`, the turn's
    /// `usage` (its main model), priced here.
    fn usage(&mut self, v: &Value) -> Vec<AgentEvent> {
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let own = TokenUsage { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_write: n("cache_creation_input_tokens") };
        // Claude Code writes the prompt cache for an hour on subscriptions; `cache_creation` says.
        let own_1h = u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0);
        let fast = u["speed"] == "fast" || v["fast_mode_state"] == "on";
        let priced = |model: Option<&str>, tokens: &TokenUsage, long_writes: u64| {
            model.and_then(|m| trek_core::pricing::request(m, &trek_core::AgentId::ClaudeCode, tokens, long_writes, fast))
        };
        let Some(models) = v["modelUsage"].as_object() else {
            let cost = priced(self.model.as_deref(), &own, own_1h);
            return if own.is_empty() { vec![] } else { vec![AgentEvent::Usage { model: None, tokens: own, cost }] };
        };
        let totals: Vec<(&String, TokenUsage, Option<f64>)> = models
            .iter()
            .map(|(model, u)| {
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                (model, TokenUsage { input: n("inputTokens"), output: n("outputTokens"), cache_read: n("cacheReadInputTokens"), cache_write: n("cacheCreationInputTokens") }, u["costUSD"].as_f64())
            })
            .collect();
        if std::mem::take(&mut self.resumed) {
            let session = v["session_id"].as_str().filter(|s| !s.is_empty());
            let ledger = self.ledger.as_deref();
            let saved = ledger.and_then(|dir| session.and_then(|s| UsageLedger::load(dir, s)).or_else(|| self.resumed_from.as_deref().and_then(|s| UsageLedger::load(dir, s))));
            match saved {
                // Claude Code carries a session's totals over only when it resumes the folder's
                // latest session; otherwise they start again with this turn. Carried over, the
                // main model's total holds the saved one and this turn's `usage` on top of it.
                Some(saved) => {
                    let covers = |a: &TokenUsage, b: &TokenUsage| a.input >= b.input && a.output >= b.output && a.cache_read >= b.cache_read && a.cache_write >= b.cache_write;
                    let main = self.model.as_deref().and_then(|m| totals.iter().find(|(name, _, _)| name.as_str() == m));
                    let carried = match main.and_then(|(m, total, _)| saved.get(m.as_str()).map(|(before, _)| (total, before))) {
                        Some((total, before)) => {
                            let mut at_least = *before;
                            at_least.add(&own);
                            covers(total, &at_least)
                        }
                        None => totals.iter().all(|(m, total, _)| saved.get(m.as_str()).is_none_or(|(before, _)| covers(total, before))),
                    };
                    self.models = if carried { saved } else { HashMap::new() };
                }
                None => return self.first_resumed(v, &own, own_1h, totals, priced),
            }
        }
        let mut out = vec![];
        for (model, total, total_cost) in totals {
            let (before, before_cost) = self.models.get(model).copied().unwrap_or_default();
            let tokens = total.since(&before);
            let restarted = tokens == total && before != TokenUsage::default();
            let cost = match total_cost {
                Some(c) => Some(UsageCost::reported(if restarted || c < before_cost { c } else { c - before_cost })),
                // The turn's own `usage` says how its main model wrote the cache.
                None => priced(Some(model), &tokens, if tokens == own { own_1h } else { 0 }),
            };
            self.models.insert(model.clone(), (total, total_cost.unwrap_or(0.0)));
            if !tokens.is_empty() {
                out.push(AgentEvent::Usage { model: Some(model.clone()), tokens, cost });
            }
        }
        self.remember(v);
        out
    }

    /// The first result of a resumed session no ledger knows (resumed outside Trek, or before it
    /// kept one): its totals are the baseline from here on, and the turn's own `usage` (its main
    /// model's) is all that's known of this turn, so it's priced here. Helper and sub-agent calls
    /// in this one turn go uncounted rather than overcounted.
    fn first_resumed(
        &mut self,
        v: &Value,
        own: &TokenUsage,
        own_1h: u64,
        totals: Vec<(&String, TokenUsage, Option<f64>)>,
        priced: impl Fn(Option<&str>, &TokenUsage, u64) -> Option<UsageCost>,
    ) -> Vec<AgentEvent> {
        let main = self.model.as_deref().filter(|m| totals.iter().any(|(name, _, _)| name.as_str() == *m)).map(String::from).or_else(|| {
            // Which entry the turn ran on: one that holds at least the turn, the busiest.
            let holds = |t: &TokenUsage| t.input >= own.input && t.output >= own.output && t.cache_read >= own.cache_read && t.cache_write >= own.cache_write;
            totals.iter().filter(|(_, t, _)| holds(t)).max_by_key(|(_, t, _)| t.output).map(|(m, _, _)| m.to_string())
        });
        self.models = totals.into_iter().map(|(m, t, c)| (m.clone(), (t, c.unwrap_or(0.0)))).collect();
        self.remember(v);
        let cost = priced(main.as_deref(), own, own_1h);
        if own.is_empty() { vec![] } else { vec![AgentEvent::Usage { model: main, tokens: *own, cost }] }
    }

    /// Keep the session's totals for the process that resumes it next.
    fn remember(&self, v: &Value) {
        if let (Some(dir), Some(session)) = (self.ledger.as_deref(), v["session_id"].as_str().filter(|s| !s.is_empty())) {
            UsageLedger::save(dir, session, &self.models);
        }
    }

    /// Claude never took in what it was sent after the held result: the turn ends with it.
    fn give_up(&mut self) -> Option<Vec<AgentEvent>> {
        self.waiting.take()?;
        let missed: Vec<String> = self.unread.iter().filter(|(_, mid_turn)| *mid_turn).map(|(text, _)| format!("“{}”", clip(text, 120))).collect();
        tracing::warn!("claude: {} message(s) sent mid-turn were never taken in", missed.len());
        self.unread.clear();
        let message = format!("Claude Code didn't take up {}. Send it again.", missed.join(", "));
        Some(vec![AgentEvent::Notice(message), AgentEvent::TurnComplete { error: None }])
    }
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let bin = detect::which("claude").context("Claude Code isn't installed (npm i -g @anthropic-ai/claude-code)")?;
    let (mut config, mut recap) = check_resume_point(config).await;
    if recap.is_some() {
        let _ = events
            .send(AgentEvent::Notice("Claude Code couldn't take its session back to that point, so it continues in a new session with a recap of this conversation.".into()))
            .await;
    }
    // Lives as long as the session; removed on drop.
    let mcp_file = if config.mcp_servers.is_empty() {
        None
    } else {
        Some(TempFile::write("mcp", &serde_json::to_string(&json!({ "mcpServers": claude_mcp_servers(&config.mcp_servers) }))?)?)
    };
    let mut cli = Cli::spawn(&bin, &config, mcp_file.as_ref())?;
    let mut ctl = Control { next_id: 0 };
    let mut init_id = cli.initialize(&mut ctl).await?;
    // Outstanding `get_context_usage` requests; their responses become `Context` events.
    let mut context_requests: HashSet<String> = HashSet::new();
    // Outstanding reads of background tasks' output (request id → task id), and stops.
    let mut output_requests: HashMap<String, String> = HashMap::new();
    let mut stop_requests: HashSet<String> = HashSet::new();

    // Inputs of pending permission requests, echoed back as `updatedInput` on allow.
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut streamed_text = false;
    let mut hand_holding = config.hand_holding;
    // In plan mode (as Claude last reported it): access changes wait until the plan is approved.
    let mut planning = config.plan;
    // Fast mode and effort as the session last took them.
    let (mut fast, mut effort) = (config.fast.is_some(), config.effort);
    let mut in_turn = false;
    let mut turns = Turns { resumed: config.resume.is_some(), resumed_from: config.resume.clone(), ledger: Some(UsageLedger::dir()), ..Default::default() };
    // Resumed partway: once the session has said which it is, that message is its latest point.
    let mut resumed_at = config.resume_at.clone();
    // The session has started (`system init`); until then, the messages sent so far, to send
    // again if the session to resume is gone and a new one starts instead.
    let mut started = false;
    let mut unstarted: Vec<(String, Vec<PathBuf>)> = Vec::new();

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    Command::Prompt { text, images } => {
                        streamed_text = false;
                        let mid_turn = std::mem::replace(&mut in_turn, true);
                        if !started {
                            unstarted.push((text.clone(), images.clone()));
                        }
                        let text = match recap.take() {
                            Some(r) => crate::recap_prompt(&r, &text),
                            None => text,
                        };
                        let (msg, skipped) = user_message(&text, &images);
                        for e in skipped {
                            let _ = events.send(AgentEvent::Notice(e)).await;
                        }
                        turns.sent(&text, mid_turn);
                        write_line(&mut cli.stdin, &msg).await?;
                    }
                    Command::Interrupt => {
                        write_line(&mut cli.stdin, &ctl.request("interrupt", json!({}))).await?;
                        // Between a held result and the turn for what was sent after it, there may
                        // be nothing for Claude to stop: the turn ends here.
                        if turns.waiting.take().is_some() {
                            turns.unread.clear();
                            in_turn = false;
                            if events.send(AgentEvent::TurnComplete { error: Some("Interrupted".into()) }).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Command::SetHandHolding(h) => {
                        hand_holding = h;
                        if !planning {
                            write_line(&mut cli.stdin, &ctl.request("set_permission_mode", json!({ "mode": h.claude_mode() }))).await?
                        }
                    }
                    Command::SetModel { model, .. } => {
                        write_line(&mut cli.stdin, &ctl.request("set_model", json!({ "model": model }))).await?
                    }
                    Command::SetModes { plan, fast: f, effort: e } => {
                        for msg in modes_requests(&mut ctl, (planning, fast, effort), (plan, f.is_some(), e), hand_holding.claude_mode()) {
                            write_line(&mut cli.stdin, &msg).await?;
                        }
                        (planning, fast, effort) = (plan, f.is_some(), e);
                    }
                    Command::Respond { request_id, decision } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
                        for msg in respond(&mut ctl, &request_id, &request, decision, hand_holding.claude_mode()) {
                            write_line(&mut cli.stdin, &msg).await?;
                        }
                        if request["tool_name"] == "ExitPlanMode" && decision != Decision::Deny {
                            planning = false;
                        }
                    }
                    Command::Answer { request_id, answers } => {
                        let request = pending.remove(&request_id).unwrap_or(json!({}));
                        write_line(&mut cli.stdin, &answer(&request_id, &request, answers)).await?;
                    }
                    Command::ReadTask { id } => {
                        let r = ctl.request("get_task_output", json!({ "task_id": id }));
                        output_requests.insert(r["request_id"].as_str().unwrap_or_default().to_string(), id);
                        write_line(&mut cli.stdin, &r).await?;
                    }
                    Command::StopTask { id } => {
                        let r = ctl.request("stop_task", json!({ "task_id": id }));
                        stop_requests.insert(r["request_id"].as_str().unwrap_or_default().to_string());
                        write_line(&mut cli.stdin, &r).await?;
                    }
                    Command::Shutdown => break,
                }
            }
            _ = tokio::time::sleep_until(turns.waiting.unwrap_or_else(tokio::time::Instant::now)), if turns.waiting.is_some() => {
                if let Some(out) = turns.give_up() {
                    in_turn = false;
                    for ev in out {
                        if events.send(ev).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
            line = cli.stdout.next_line() => {
                let Some(line) = line? else {
                    if in_turn {
                        return Err(cli.stderr.exited("Claude Code"));
                    }
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if !started && config.resume.is_some() && session_missing(&v) {
                    // Start over in a new session, with what was sent so far.
                    tracing::warn!("claude: session {:?} is gone; starting a new one", config.resume);
                    config.resume = None;
                    config.resume_at = None;
                    config.fork = false;
                    resumed_at = None;
                    cli.stop().await;
                    cli = Cli::spawn(&bin, &config, mcp_file.as_ref())?;
                    init_id = cli.initialize(&mut ctl).await?;
                    context_requests.clear();
                    recap = config.recap.take();
                    let notice = match recap {
                        Some(_) => AgentEvent::Notice("Claude Code couldn't reopen this conversation, so it continues in a new session with a recap of it.".into()),
                        None => crate::lost_session("Claude Code"),
                    };
                    let _ = events.send(notice).await;
                    turns = Turns::default();
                    for (i, (text, images)) in unstarted.iter().enumerate() {
                        let text = match recap.take() {
                            Some(r) => crate::recap_prompt(&r, text),
                            None => text.clone(),
                        };
                        turns.sent(&text, i > 0);
                        write_line(&mut cli.stdin, &user_message(&text, images).0).await?;
                    }
                    continue;
                }
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
                        write_line(&mut cli.stdin, &c).await?;
                    } else if context_requests.remove(id) {
                        if let Some(ev) = context_event(r) {
                            if events.send(ev).await.is_err() {
                                return Ok(());
                            }
                        }
                    } else if let Some(task) = output_requests.remove(id) {
                        // A task whose output can't be read (it ended long ago) just shows none.
                        if r["subtype"] == "success" {
                            if events.send(AgentEvent::TaskOutput { id: task, output: task_output(r) }).await.is_err() {
                                return Ok(());
                            }
                        }
                    } else if stop_requests.remove(id) {
                        if r["subtype"] == "error" {
                            let msg = r["error"].as_str().unwrap_or("it didn't say why");
                            if events.send(AgentEvent::Notice(format!("Claude Code couldn't stop the background task: {msg}"))).await.is_err() {
                                return Ok(());
                            }
                        }
                    } else if r["subtype"] == "error" {
                        // A setting Claude wouldn't change: the turn (if any) goes on.
                        let msg = r["error"].as_str().unwrap_or("Claude Code rejected the change.").to_string();
                        tracing::warn!("claude control request {id} failed: {msg}");
                        if events.send(AgentEvent::Notice(format!("Claude Code didn't apply the change: {msg}"))).await.is_err() {
                            return Ok(());
                        }
                    }
                    continue;
                }
                // Trek's own sub-agent tools need no approval: what a sub-agent does is asked
                // for in its own thread, at the level Trek gives it.
                if let Some(answer) = trek_tool_allowed(&v) {
                    write_line(&mut cli.stdin, &answer).await?;
                    continue;
                }
                for ev in turns.step(&v, &mut pending, &mut streamed_text) {
                    let begun = matches!(ev, AgentEvent::Started { .. });
                    if begun {
                        started = true;
                        unstarted.clear();
                    }
                    if events.send(ev).await.is_err() {
                        return Ok(());
                    }
                    if let Some(at) = resumed_at.take_if(|_| begun) {
                        let _ = events.send(AgentEvent::Mark(at)).await;
                    }
                }
                if v["type"] == "result" {
                    // Held back: the turn goes on.
                    in_turn = turns.waiting.is_some();
                    let c = ctl.request("get_context_usage", json!({}));
                    context_requests.insert(c["request_id"].as_str().unwrap_or_default().to_string());
                    write_line(&mut cli.stdin, &c).await?;
                }
            }
        }
    }
    cli.stop().await;
    Ok(())
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, v: &Value) -> Result<()> {
    let mut s = serde_json::to_string(v)?;
    s.push('\n');
    stdin.write_all(s.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// A user message for a prompt: image blocks first, then the text. Unreadable images are left
/// out, and said why.
fn user_message(text: &str, images: &[PathBuf]) -> (Value, Vec<String>) {
    let mut content = Vec::new();
    let mut skipped = Vec::new();
    for path in images {
        match load_image(path) {
            Ok((media_type, data)) => content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": media_type, "data": data }
            })),
            Err(e) => skipped.push(format!("Image left out: {e:#}")),
        }
    }
    content.push(json!({ "type": "text", "text": text }));
    let msg = json!({
        "type": "user", "session_id": "",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null
    });
    (msg, skipped)
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

/// The end of a background task's output from Claude's answer to `get_task_output`: a dev
/// server's log can run to megabytes, and only its end is shown.
fn task_output(response: &Value) -> String {
    let mut output = String::new();
    crate::keep_tail(&mut output, response["response"]["output"].as_str().unwrap_or_default());
    output
}

/// How the login is billed, from the `account` in the `initialize` response; `None` when it
/// doesn't say. Claude Code names the subscription only while its login is the one in use, so a
/// subscription wins over a key that's merely present. Without one, a key is what pays.
pub(crate) fn account_billing(account: &Value) -> Option<Billing> {
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

/// A temp file removed on drop. Only its owner can read it (it can hold a socket token), and
/// it's always a new file: never one someone else put there first.
struct TempFile(PathBuf);

impl TempFile {
    fn write(tag: &str, contents: &str) -> Result<Self> {
        Self::write_in(&std::env::temp_dir(), tag, contents)
    }

    fn write_in(dir: &Path, tag: &str, contents: &str) -> Result<Self> {
        use std::io::Write as _;
        let mut tries = 0;
        loop {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let path = dir.join(format!("trek-{tag}-{}-{nanos}-{tries}.json", std::process::id()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            // Windows has no mode bits: the file inherits the ACL of the user's temp folder.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            match options.open(&path) {
                Ok(mut file) => {
                    // Removed on failure too.
                    let made = Self(path);
                    file.write_all(contents.as_bytes()).with_context(|| format!("writing {}", made.0.display()))?;
                    return Ok(made);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 16 => tries += 1,
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        }
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
        // Claude's own message for a usage limit ("You've hit your session limit · resets 7:40pm
        // (America/New_York)"): the limit, not something the model said.
        Some("assistant") if v["parent_tool_use_id"].is_null() && v["error"] == "rate_limit" => {
            let text: Vec<&str> = v["message"]["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
            let text = text.join("\n");
            let now = trek_core::store::now_ms();
            let limit = crate::Limit::from_text(&text, now).unwrap_or(crate::Limit { resets_at: crate::limits::reset_from_text(&text, now), scope: crate::limits::scope_of(&text), message: text });
            out.push(limit.event());
            out.extend(mark(v));
        }
        Some("assistant") if v["parent_tool_use_id"].is_null() => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        out.push(AgentEvent::TextDone(block["text"].as_str().unwrap_or_default().into()));
                        *streamed_text = false;
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        let id: String = block["id"].as_str().unwrap_or_default().into();
                        let (title, detail) = tool_title(name, &block["input"]);
                        out.push(AgentEvent::ToolStarted { id: id.clone(), title, detail });
                        if let Some((added, removed)) = edit_lines(name, &block["input"]) {
                            out.push(AgentEvent::ToolLines { id, added, removed });
                        }
                    }
                    _ => {}
                }
            }
            out.extend(mark(v));
        }
        // The user's own message, echoed back (see `Turns`).
        Some("user") if v["isReplay"] == true => {}
        Some("user") if v["parent_tool_use_id"].is_null() => {
            let blocks = v["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default();
            // The result's patch describes the message's one tool call.
            let patched = (blocks.iter().filter(|b| b["type"] == "tool_result").count() == 1).then(|| result_lines(v)).flatten();
            for block in blocks {
                if block["type"] == "tool_result" {
                    let output = match &block["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    let id: String = block["tool_use_id"].as_str().unwrap_or_default().into();
                    let ok = block["is_error"] != true;
                    // A failed or denied edit changed nothing, whatever its input estimated.
                    if let Some((added, removed)) = if ok { patched } else { Some((0, 0)) } {
                        out.push(AgentEvent::ToolLines { id: id.clone(), added, removed });
                    }
                    out.push(AgentEvent::ToolFinished { id, output: clip(&output, 8000), ok });
                }
            }
            out.extend(mark(v));
        }
        Some("result") => out.push(AgentEvent::TurnComplete { error: (v["is_error"] == true).then(|| result_error(v)) }),
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
    fn answers_go_into_any_input() {
        for input in [json!({ "questions": [] }), json!("text"), Value::Null, json!([1])] {
            let msg = answer("r1", &json!({ "input": input }), vec![("Color?".into(), "teal".into())]);
            let updated = &msg["response"]["response"]["updatedInput"];
            assert_eq!(updated["answers"]["Color?"], "teal", "{input}");
        }
    }

    #[test]
    fn mcp_config_is_private_and_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("trek-mcp-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = TempFile::write_in(&dir, "mcp", "{\"token\":1}").unwrap();
        let b = TempFile::write_in(&dir, "mcp", "{}").unwrap();
        assert_ne!(a.0, b.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&a.0).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_to_string(&a.0).unwrap(), "{\"token\":1}");
        let path = a.0.clone();
        drop(a);
        assert!(!path.exists());
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_report_the_lines_they_change() {
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let edit = json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/p/a.rs","old_string":"fn a() {}\n","new_string":"fn a() {\n    b();\n}\n"}}]}});
        let ev = translate(&edit, &mut pending, &mut streamed);
        assert_eq!(ev[1], AgentEvent::ToolLines { id: "e1".into(), added: 3, removed: 1 });
        // The result's patch is exact, and replaces the estimate.
        let result = json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"e1","content":"ok"}]},
            "tool_use_result":{"filePath":"/p/a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":3,"lines":["-fn a() {}","+fn a() {","+    b();","+}"," fn c() {}"]}]}});
        let ev = translate(&result, &mut pending, &mut streamed);
        assert_eq!(ev[0], AgentEvent::ToolLines { id: "e1".into(), added: 3, removed: 1 });
        assert!(matches!(&ev[1], AgentEvent::ToolFinished { id, ok: true, .. } if id == "e1"));
        let write = json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"/p/n.md","content":"# N\n\nhi\n"}}]}});
        assert_eq!(translate(&write, &mut pending, &mut streamed)[1], AgentEvent::ToolLines { id: "w1".into(), added: 3, removed: 0 });
        // A denied or failed edit changed nothing.
        let denied = json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"e1","content":"The user doesn't want to proceed","is_error":true}]}});
        let ev = translate(&denied, &mut pending, &mut streamed);
        assert_eq!(ev[0], AgentEvent::ToolLines { id: "e1".into(), added: 0, removed: 0 });
        assert!(matches!(&ev[1], AgentEvent::ToolFinished { ok: false, .. }));
        // Other tools have none.
        let read = json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/p/a.rs"}}]}});
        assert!(!translate(&read, &mut pending, &mut streamed).iter().any(|e| matches!(e, AgentEvent::ToolLines { .. })));
    }

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
            vec![AgentEvent::TurnComplete { error: None }]
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
        let (msg, skipped) = user_message("look", &[png, dir.join("missing.png"), dir.join("x.bmp")]);
        assert_eq!(skipped.len(), 2);
        assert!(skipped[0].starts_with("Image left out: can't read image"), "{skipped:?}");
        let content = msg["message"]["content"].as_array().unwrap();
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
    fn plan_fast_and_effort_change_mid_session() {
        let mut ctl = Control { next_id: 0 };
        assert!(modes_requests(&mut ctl, (false, false, Effort::High), (false, false, Effort::High), "acceptEdits").is_empty());
        let on = modes_requests(&mut ctl, (false, false, Effort::High), (true, true, Effort::Max), "acceptEdits");
        assert_eq!(on[0]["request"], json!({ "subtype": "set_permission_mode", "mode": "plan" }));
        assert_eq!(on[1]["request"], json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": true, "effortLevel": "max" } }));
        // Out of plan mode, back to the thread's own access; fast mode off and effort left to Claude.
        let off = modes_requests(&mut ctl, (true, true, Effort::Max), (false, false, Effort::Off), "acceptEdits");
        assert_eq!(off[0]["request"], json!({ "subtype": "set_permission_mode", "mode": "acceptEdits" }));
        assert_eq!(off[1]["request"], json!({ "subtype": "apply_flag_settings", "settings": { "fastMode": null, "effortLevel": null } }));
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
            vec![AgentEvent::TurnComplete { error: Some("No conversation found with session ID: 0b0b0b0b-0000-4000-8000-000000000000".into()) }]
        );
        assert_eq!(result_error(&json!({"is_error":true,"result":"API Error: overloaded"})), "API Error: overloaded");
        assert_eq!(result_error(&json!({"is_error":true})), "The turn failed.");
    }

    /// Events for `lines` through `Turns`, with `first` sent before them and `steer` sent mid-turn.
    fn steered(lines: &[Value], first: &str, steer: &str) -> (Vec<AgentEvent>, Turns) {
        let mut turns = Turns::default();
        turns.sent(first, false);
        turns.sent(steer, true);
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let ev = lines.iter().flat_map(|v| turns.step(v, &mut pending, &mut streamed)).collect();
        (ev, turns)
    }

    fn turn_ends(ev: &[AgentEvent]) -> usize {
        ev.iter().filter(|e| matches!(e, AgentEvent::TurnComplete { .. })).count()
    }

    #[test]
    fn a_steer_run_as_its_own_turn_ends_the_turn_once() {
        // Recorded (Claude Code 2.1.288, claude-haiku-4-5): a story, then "Now reply with just
        // the word BANANA." while it streamed. Claude runs the second message after the first,
        // with a result each.
        let lines = fixture(include_str!("../fixtures/claude-steer-turn.jsonl"));
        let first = "Write a 200 word story about a fox.";
        let (ev, turns) = steered(&lines, first, "Now reply with just the word BANANA.");
        assert_eq!(turn_ends(&ev), 1, "{ev:?}");
        assert!(matches!(ev.last(), Some(AgentEvent::TurnComplete { error: None })));
        assert!(ev.contains(&AgentEvent::TextDone("BANANA".into())));
        assert!(turns.unread.is_empty() && turns.waiting.is_none());
        // The echoes aren't points to resume at.
        let echoes: Vec<&str> = lines.iter().filter(|v| v["isReplay"] == true).filter_map(|v| v["uuid"].as_str()).collect();
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Mark(m) if echoes.contains(&m.as_str()))));

        // Nothing sent mid-turn: each result ends its turn, as before.
        let mut turns = Turns::default();
        turns.sent(first, false);
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let ev: Vec<AgentEvent> = lines.iter().flat_map(|v| turns.step(v, &mut pending, &mut streamed)).collect();
        assert_eq!(turn_ends(&ev), 2);
    }

    #[test]
    fn a_steer_taken_into_the_turn_ends_it_once() {
        // Recorded: "Also end your reply with the word BANANA." sent while `sleep 6` ran. Claude
        // takes it in after the tool call, and the turn has a single result.
        let lines = fixture(include_str!("../fixtures/claude-steer-merged.jsonl"));
        let (ev, turns) = steered(&lines, "Run the bash command 'sleep 6' and then say DONE.", "Also end your reply with the word BANANA.");
        assert_eq!(turn_ends(&ev), 1, "{ev:?}");
        assert!(matches!(ev.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
        assert!(turns.waiting.is_none());
    }

    #[test]
    fn a_held_result_gives_up_on_messages_claude_never_takes_in() {
        let ok = json!({"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.5});
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let mut turns = Turns::default();
        turns.sent("first", false);
        turns.sent("steer", true);
        assert!(turns.step(&ok, &mut pending, &mut streamed).is_empty());
        let gave_up = turns.give_up().unwrap();
        assert!(matches!(&gave_up[..], [AgentEvent::Notice(n), AgentEvent::TurnComplete { error: None }] if n.contains("“steer”") && n.contains("Send it again")), "{gave_up:?}");
        assert!(turns.unread.is_empty() && turns.give_up().is_none());

        // A stopped turn ends whatever is unread.
        let stopped = json!({"type":"result","is_error":true,"terminal_reason":"aborted_streaming","total_cost_usd":0.5});
        turns.sent("steer", true);
        let ev = turns.step(&stopped, &mut pending, &mut streamed);
        assert!(matches!(&ev[..], [AgentEvent::TurnComplete { error: Some(e), .. }] if e == "Interrupted"));
        assert!(turns.unread.is_empty() && turns.waiting.is_none());

        // Slash commands aren't echoed, so they never hold a turn open.
        turns.sent("/context", true);
        assert_eq!(turn_ends(&turns.step(&ok, &mut pending, &mut streamed)), 1);
    }

    #[test]
    fn a_missing_session_is_recognised() {
        // Recorded: `claude --resume <unknown id>` exits at once with this result.
        let v: Value = serde_json::from_str(r#"{"type":"result","subtype":"error_during_execution","duration_ms":0,"is_error":true,"num_turns":0,"stop_reason":null,"session_id":"0b0b0b0b-0000-4000-8000-000000000000","total_cost_usd":0,"permission_denials":[],"errors":["No conversation found with session ID: 0b0b0b0b-0000-4000-8000-000000000000"]}"#).unwrap();
        assert!(session_missing(&v));
        assert!(!session_missing(&json!({"type":"result","is_error":true,"errors":["API Error: overloaded"]})));
        assert!(!session_missing(&json!({"type":"result","is_error":false,"result":"No conversation found"})));
    }

    #[test]
    fn a_usage_limit_is_one_limit_event_with_its_reset() {
        // The stream as Claude Code 2.1.287 sends it at a session limit: a rejected
        // `rate_limit_event`, its own message (no model behind it), and an error result. The
        // message and the reset are recorded ones (the reset was 1790984400, 7:40pm in New York).
        let lines = fixture(include_str!("../fixtures/claude-limit.jsonl"));
        let mut turns = Turns::default();
        turns.sent("keep going", false);
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let ev: Vec<AgentEvent> = lines.iter().flat_map(|v| turns.step(v, &mut pending, &mut streamed)).collect();
        let message = "You've hit your session limit · resets 7:40pm (America/New_York)";
        let limits: Vec<&AgentEvent> = ev.iter().filter(|e| matches!(e, AgentEvent::LimitReached { .. })).collect();
        assert_eq!(limits, [&AgentEvent::LimitReached { message: message.into(), resets_at: Some(1_790_984_400_000), scope: crate::LimitScope::Session }]);
        // Not an answer from the model.
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::TextDone(_) | AgentEvent::TextDelta(_))), "{ev:?}");
        // The turn ends after it, failed.
        let limit = ev.iter().position(|e| matches!(e, AgentEvent::LimitReached { .. })).unwrap();
        let end = ev.iter().position(|e| matches!(e, AgentEvent::TurnComplete { .. })).unwrap();
        assert!(limit < end);
        assert_eq!(ev[end], AgentEvent::TurnComplete { error: Some(message.into()) });

        // Without the rate-limit event, the result alone is enough; and an ordinary failure isn't one.
        let mut turns = Turns::default();
        let ev = turns.step(&lines[3], &mut pending, &mut streamed);
        assert!(matches!(&ev[..], [AgentEvent::LimitReached { scope: crate::LimitScope::Session, resets_at: Some(_), .. }, AgentEvent::TurnComplete { .. }]), "{ev:?}");
        let overloaded = json!({"type":"result","is_error":true,"result":"API Error: 529 overloaded"});
        assert!(!turns.step(&overloaded, &mut pending, &mut streamed).iter().any(|e| matches!(e, AgentEvent::LimitReached { .. })));
    }

    #[test]
    fn a_rejected_rate_limit_makes_no_limit_of_an_interrupt_or_overage() {
        let rejected = |overage: bool| json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1790984400,"rateLimitType":"five_hour","isUsingOverage":overage}});
        let (mut pending, mut streamed) = (HashMap::new(), false);
        // The user stopped the turn: an interrupt, whatever the last rate-limit event said.
        let mut turns = Turns::default();
        turns.sent("keep going", false);
        turns.step(&rejected(false), &mut pending, &mut streamed);
        let interrupt = json!({"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_tools","result":""});
        let ev = turns.step(&interrupt, &mut pending, &mut streamed);
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::LimitReached { .. })), "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::TurnComplete { error: Some(e), .. } if e == "Interrupted")), "{ev:?}");
        // Paid overage carries the turn past the limit: a later failure is just a failure.
        let mut turns = Turns::default();
        turns.sent("keep going", false);
        turns.step(&rejected(true), &mut pending, &mut streamed);
        assert_eq!(turns.rejected, None);
        let failed = json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"Tool crashed"});
        assert!(!turns.step(&failed, &mut pending, &mut streamed).iter().any(|e| matches!(e, AgentEvent::LimitReached { .. })));
        // Without overage, the failure is the limit.
        let mut turns = Turns::default();
        turns.sent("keep going", false);
        turns.step(&rejected(false), &mut pending, &mut streamed);
        let ev = turns.step(&failed, &mut pending, &mut streamed);
        assert!(ev.contains(&AgentEvent::LimitReached { message: "Tool crashed".into(), resets_at: Some(1_790_984_400_000), scope: crate::LimitScope::Session }), "{ev:?}");
    }

    #[test]
    fn rate_limit_events_that_allow_the_turn_change_nothing() {
        // Recorded (Claude Code 2.1.288): the event every turn starts with, a warning near the weekly limit.
        let v: Value = serde_json::from_str(include_str!("../fixtures/claude-rate-limit-allowed.json")).unwrap();
        let mut turns = Turns::default();
        // No limit, and nothing for the transcript: only how full the plan's windows are.
        let used = |scope, percent, at: i64| AgentEvent::LimitUsed { scope, percent, resets_at: Some(at * 1000) };
        assert_eq!(turns.step(&v, &mut HashMap::new(), &mut false), [used(crate::LimitScope::Session, 1.0, 1791058200), used(crate::LimitScope::Weekly, 59.0, 1791471600)]);
        assert_eq!(turns.rejected, None);
        // An older Claude Code names one window; overage, or the limit itself, none to wrap up for.
        let one = json!({"status":"allowed_warning","resetsAt":50,"rateLimitType":"seven_day_opus","utilization":0.985});
        assert_eq!(windows_used(&one), [AgentEvent::LimitUsed { scope: crate::LimitScope::Model("Opus".into()), percent: 98.5, resets_at: Some(50_000) }]);
        assert!(windows_used(&json!({"status":"allowed","isUsingOverage":true,"rateLimitType":"five_hour","utilization":0.99})).is_empty());
        assert!(windows_used(&json!({"status":"rejected","rateLimitType":"five_hour","utilization":1})).is_empty());
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
            read_only: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
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
    fn trek_s_own_tools_are_allowed_without_asking() {
        let ask = |tool: &str| json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":tool,"input":{"title":"x"}}});
        let ok = trek_tool_allowed(&ask("mcp__trek-orchestrate__delegate_task")).unwrap();
        assert_eq!(ok["response"]["request_id"], "r1");
        assert_eq!(ok["response"]["response"], json!({"behavior":"allow","updatedInput":{"title":"x"}}));
        assert_eq!(trek_tool_allowed(&ask("mcp__other__delete_everything")), None);
        assert_eq!(trek_tool_allowed(&ask("Bash")), None);
    }

    #[test]
    fn mcp_config_shape() {
        let servers = [crate::McpServer::stdio("fs", "npx", vec!["-y".into(), "srv".into()], vec![("K".into(), "v".into())])];
        assert_eq!(
            json!({ "mcpServers": mcp_servers_json(&servers) }),
            json!({"mcpServers":{"fs":{"command":"npx","args":["-y","srv"],"env":{"K":"v"}}}})
        );
        // A remote server: Claude Code's own `--transport http` shape.
        let figma = crate::McpServer::http("figma-desktop", "http://127.0.0.1:3845/mcp", vec![]);
        let linear = crate::McpServer::http("linear", "https://mcp.linear.app/mcp", vec![("Authorization".into(), "Bearer t".into())]);
        assert_eq!(
            claude_mcp_servers(&[figma, linear]),
            json!({
                "figma-desktop": {"type":"http","url":"http://127.0.0.1:3845/mcp","headers":{}},
                "linear": {"type":"http","url":"https://mcp.linear.app/mcp","headers":{"Authorization":"Bearer t"}},
            })
        );
        let trek = crate::McpServer { tool_timeout_secs: Some(1900), ..crate::McpServer::stdio("trek-orchestrate", "trek-mcp", vec![], vec![]) };
        let out = claude_mcp_servers(&[trek, servers[0].clone()]);
        assert_eq!(out["trek-orchestrate"]["timeout"], 1_900_000, "Trek's tools may wait long on a sub-agent");
        assert!(out["fs"].get("timeout").is_none(), "others keep Claude Code's default");
    }

    #[test]
    fn notes_for_a_cmd_go_in_a_file() {
        let notes = "Verify with ./app check.\nThen say \"done\" & stop at 100%.";
        let mut args = cli_args(&SessionConfig { instructions: Some(notes.into()), ..config() });
        let file = notes_in_file(&mut args).unwrap().expect("a file for the notes");
        assert!(has(&args, &["--append-system-prompt-file", &file.0.display().to_string()]), "{args:?}");
        assert!(!args.iter().any(|a| a == "--append-system-prompt" || a.contains("Verify")), "{args:?}");
        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), notes);
        // No notes, no file, and nothing else changes.
        let mut plain = cli_args(&config());
        assert!(notes_in_file(&mut plain).unwrap().is_none());
        assert_eq!(plain, cli_args(&config()));
    }

    /// Through a stand-in `claude.cmd` (npm's shim, see `tests_cmd_args`), notes with line breaks,
    /// quotes and `%` reach Claude whole, from the file; the rest of the arguments arrive as given.
    #[cfg(windows)]
    #[tokio::test]
    async fn notes_with_line_breaks_reach_a_claude_cmd() {
        let shims = crate::tests_cmd_args::Shims::new("claude");
        let notes = "First line\r\nSecond \"quoted\" line & 100% of %PATH%\n";
        let config = SessionConfig { instructions: Some(notes.into()), cwd: std::env::temp_dir(), ..config() };
        let mut cli = Cli::spawn(&shims.npm(), &config, None).unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(30), cli.stdout.next_line()).await.unwrap().unwrap().expect("the stand-in's argv");
        let argv: Vec<String> = serde_json::from_str(&line).unwrap();
        let mut expected = cli_args(&config);
        let at = expected.iter().position(|a| a == "--append-system-prompt").unwrap();
        let file = PathBuf::from(&argv[2 + at + 1]);
        expected[at] = "--append-system-prompt-file".into();
        expected[at + 1] = file.display().to_string();
        assert_eq!(argv[2..], expected[..], "{argv:?}");
        // Trimmed, as the flag would have had them.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), notes.trim());
        cli.stop().await;
        assert!(!file.exists(), "the notes' file goes with the process");
    }

    #[test]
    fn project_notes_join_the_system_prompt() {
        assert!(!cli_args(&config()).iter().any(|a| a == "--append-system-prompt"));
        let notes = cli_args(&SessionConfig { instructions: Some("Verify with ./app check.".into()), ..config() });
        assert!(has(&notes, &["--append-system-prompt", "Verify with ./app check."]), "{notes:?}");
        // Trek's guides are read without asking; a folder that isn't there isn't passed.
        let guides = std::env::temp_dir();
        let dirs = cli_args(&SessionConfig { read_dirs: vec![guides.clone(), "/no/such/dir".into()], ..config() });
        assert!(has(&dirs, &["--add-dir", &guides.display().to_string()]) && !dirs.iter().any(|a| a == "/no/such/dir"), "{dirs:?}");
    }

    #[test]
    fn a_read_only_session_has_no_editing_tools() {
        let args = cli_args(&SessionConfig { read_only: true, ..config() });
        assert!(has(&args, &["--disallowedTools", "Edit,MultiEdit,Write,NotebookEdit"]), "{args:?}");
        assert!(!cli_args(&config()).iter().any(|a| a == "--disallowedTools"));
    }

    #[test]
    fn a_read_only_sessions_shell_cannot_write_where_it_works() {
        let settings = |c: &SessionConfig| -> Option<Value> {
            let args = cli_args(c);
            assert!(args.iter().filter(|a| *a == "--settings").count() <= 1, "one --settings: {args:?}");
            args.iter().position(|a| a == "--settings").map(|i| serde_json::from_str(&args[i + 1]).unwrap())
        };
        // The sandbox is on with no way round it, and its folder (and what it may read) is
        // write-denied: checked against Claude Code 2.1.289, where an allow-listed `touch`
        // then fails with "Operation not permitted".
        let guides = std::env::temp_dir();
        let s = settings(&SessionConfig { read_only: true, read_dirs: vec![guides.clone()], ..config() }).unwrap();
        assert_eq!(s["sandbox"]["enabled"], true);
        assert_eq!(s["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(s["sandbox"]["filesystem"]["denyWrite"], json!(["/tmp", guides.display().to_string()]));
        // With Fast on too, both go in the one settings argument.
        let both = settings(&SessionConfig { read_only: true, fast: Some("fast".into()), ..config() }).unwrap();
        assert!(both["fastMode"] == true && both["sandbox"]["enabled"] == true, "{both}");
        // A session that may change things isn't sandboxed by Trek.
        assert_eq!(settings(&config()), None);
        assert_eq!(settings(&SessionConfig { fast: Some("fast".into()), ..config() }), Some(json!({"fastMode": true})));
    }

    #[test]
    fn results_report_what_each_turn_used_per_model() {
        let mut pending = HashMap::new();
        let mut streamed = false;
        let mut turns = Turns::default();
        // Two real results from one session (trimmed): `usage` is the turn's, `modelUsage` runs
        // from the start of the process. The dated Haiku is Claude Code's own helper call.
        let first = json!({"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.0191803,
            "usage":{"input_tokens":10,"cache_creation_input_tokens":8324,"cache_read_input_tokens":13803,"output_tokens":41},
            "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":897,"outputTokens":8,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"costUSD":0.000937},
                "claude-haiku-4-5":{"inputTokens":10,"outputTokens":41,"cacheReadInputTokens":13803,"cacheCreationInputTokens":8324,"costUSD":0.0182433}}});
        let second = json!({"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.023611,
            "usage":{"input_tokens":10,"cache_creation_input_tokens":1004,"cache_read_input_tokens":22127,"output_tokens":40},
            "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":897,"outputTokens":8,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"costUSD":0.000937},
                "claude-haiku-4-5":{"inputTokens":20,"outputTokens":81,"cacheReadInputTokens":35930,"cacheCreationInputTokens":9328,"costUSD":0.022674}}});
        let usage = |ev: Vec<AgentEvent>| -> Vec<(Option<String>, TokenUsage)> {
            assert!(matches!(ev.last(), Some(AgentEvent::TurnComplete { .. })), "{ev:?}");
            ev.into_iter().filter_map(|e| if let AgentEvent::Usage { model, tokens, .. } = e { Some((model, tokens)) } else { None }).collect()
        };
        let mut got = usage(turns.step(&first, &mut pending, &mut streamed));
        got.sort_by_key(|(m, _)| m.clone());
        assert_eq!(
            got,
            vec![
                (Some("claude-haiku-4-5".into()), TokenUsage { input: 10, output: 41, cache_read: 13803, cache_write: 8324 }),
                (Some("claude-haiku-4-5-20251001".into()), TokenUsage { input: 897, output: 8, cache_read: 0, cache_write: 0 }),
            ]
        );
        // The second turn: only what moved, which is the turn's own `usage`.
        assert_eq!(usage(turns.step(&second, &mut pending, &mut streamed)), vec![(Some("claude-haiku-4-5".into()), TokenUsage { input: 10, output: 40, cache_read: 22127, cache_write: 1004 })]);
        // Without `modelUsage`, the turn's `usage` stands for the session's model.
        let mut fresh = Turns::default();
        let bare = json!({"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":3,"output_tokens":4}});
        assert_eq!(usage(fresh.step(&bare, &mut pending, &mut streamed)), vec![(None, TokenUsage { input: 3, output: 4, ..Default::default() })]);
    }

    #[test]
    fn turn_costs_add_up_to_claude_codes_total_across_a_resume() {
        // Recorded (Claude Code 2.1.288, claude-haiku-4-5, low effort): a turn, then a second
        // in a new process with `--resume`. `total_cost_usd` and `modelUsage` carry on across
        // the resume (0.0206883, then 0.0233464), so summing them per process would count the
        // first turn twice.
        let lines = fixture(include_str!("../fixtures/claude-cost-resume.jsonl"));
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let costs = |ev: Vec<AgentEvent>| -> Vec<(Option<String>, UsageCost)> {
            ev.into_iter().filter_map(|e| if let AgentEvent::Usage { model, cost, .. } = e { Some((model, cost.expect("priced"))) } else { None }).collect()
        };
        let mut first = Turns::default();
        let mut one: Vec<(Option<String>, UsageCost)> = lines[..2].iter().flat_map(|v| costs(first.step(v, &mut pending, &mut streamed))).collect();
        one.sort_by(|a, b| a.0.cmp(&b.0));
        // Claude Code's own figures, per model: the turn, and its title call on the dated id.
        assert_eq!(one.iter().map(|(m, c)| (m.as_deref().unwrap(), c.reported)).collect::<Vec<_>>(), [("claude-haiku-4-5", true), ("claude-haiku-4-5-20251001", true)]);
        assert!((one[0].1.usd - 0.0197463).abs() < 1e-12 && (one[1].1.usd - 0.000942).abs() < 1e-12, "{one:?}");
        let mut resumed = Turns { resumed: true, ..Default::default() };
        let two: Vec<(Option<String>, UsageCost)> = lines[2..].iter().flat_map(|v| costs(resumed.step(v, &mut pending, &mut streamed))).collect();
        // Priced here from the turn's own usage, its cache writes 1-hour ones:
        // 10 × $1 + 37 × $5 + 22,871 × $0.10 + 88 × $2, per million.
        assert_eq!(two.len(), 1);
        assert!(!two[0].1.reported);
        let total: f64 = one.iter().chain(&two).map(|(_, c)| c.usd).sum();
        assert!((total - lines[3]["total_cost_usd"].as_f64().unwrap()).abs() < 1e-9, "{total}");
    }

    #[test]
    fn a_resume_counts_from_the_ledger_with_claude_codes_own_figures() {
        // The same two real processes, each with the ledger: the resumed turn's cost is Claude
        // Code's own (its `costUSD` moved), and the two add up to its total.
        let dir = std::env::temp_dir().join(format!("trek-claude-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lines = fixture(include_str!("../fixtures/claude-cost-resume.jsonl"));
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let costs = |ev: Vec<AgentEvent>| -> Vec<UsageCost> { ev.into_iter().filter_map(|e| if let AgentEvent::Usage { cost, .. } = e { cost } else { None }).collect() };
        let mut first = Turns { ledger: Some(dir.clone()), ..Default::default() };
        let one: Vec<UsageCost> = lines[..2].iter().flat_map(|v| costs(first.step(v, &mut pending, &mut streamed))).collect();
        let mut resumed = Turns { resumed: true, resumed_from: Some("eb3d266a-3587-4222-9ec1-fce8b7b3d23c".into()), ledger: Some(dir.clone()), ..Default::default() };
        let two: Vec<UsageCost> = lines[2..].iter().flat_map(|v| costs(resumed.step(v, &mut pending, &mut streamed))).collect();
        assert_eq!(two.len(), 1);
        assert!(two[0].reported, "Claude Code's own figure");
        assert!((two[0].usd - (0.0224044 - 0.0197463)).abs() < 1e-12, "{two:?}");
        let total: f64 = one.iter().chain(&two).map(|c| c.usd).sum();
        assert!((total - lines[3]["total_cost_usd"].as_f64().unwrap()).abs() < 1e-12, "{total}");

        // A sub-agent on another model in the resumed turn is counted too.
        let mut resumed = Turns { resumed: true, ledger: Some(dir.clone()), ..Default::default() };
        let mut with_task = lines[3].clone();
        with_task["modelUsage"]["claude-sonnet-5-5"] = json!({"inputTokens":40,"outputTokens":300,"cacheReadInputTokens":1000,"cacheCreationInputTokens":2000,"costUSD":0.0123});
        let ev: Vec<(Option<String>, UsageCost)> = [&lines[2], &with_task]
            .into_iter()
            .flat_map(|v| resumed.step(v, &mut pending, &mut streamed))
            .filter_map(|e| if let AgentEvent::Usage { model, cost, .. } = e { Some((model, cost.unwrap())) } else { None })
            .collect();
        assert!(ev.contains(&(Some("claude-sonnet-5-5".into()), UsageCost::reported(0.0123))), "{ev:?}");

        // Claude Code started the totals over (another session in the folder ran since): all of
        // the turn's figures are new.
        let mut elsewhere = Turns { resumed: true, ledger: Some(dir.clone()), model: Some("claude-haiku-4-5".into()), ..Default::default() };
        let mut fresh = lines[3].clone();
        fresh["modelUsage"] = json!({"claude-haiku-4-5":{"inputTokens":10,"outputTokens":37,"cacheReadInputTokens":22871,"cacheCreationInputTokens":88,"costUSD":0.0026581}});
        let ev = costs(elsewhere.step(&fresh, &mut pending, &mut streamed));
        assert_eq!(ev, vec![UsageCost::reported(0.0026581)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resumed_session_counts_only_its_own_turns() {
        let mut pending = HashMap::new();
        let mut streamed = false;
        let usage = |ev: Vec<AgentEvent>| -> Vec<(Option<String>, TokenUsage)> {
            ev.into_iter().filter_map(|e| if let AgentEvent::Usage { model, tokens, .. } = e { Some((model, tokens)) } else { None }).collect()
        };
        // A real second turn in a new process (`--resume`): `modelUsage` carries the first
        // turn's tokens, the helper call included, restored from the session; `usage` is the turn's.
        let mut turns = Turns { resumed: true, ..Default::default() };
        let init = json!({"type":"system","subtype":"init","session_id":"s","model":"claude-haiku-4-5"});
        turns.step(&init, &mut pending, &mut streamed);
        let resumed = json!({"type":"result","subtype":"success","is_error":false,
            "usage":{"input_tokens":10,"cache_creation_input_tokens":1003,"cache_read_input_tokens":22021,"output_tokens":41},
            "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":897,"outputTokens":8,"cacheReadInputTokens":0,"cacheCreationInputTokens":0},
                "claude-haiku-4-5":{"inputTokens":20,"outputTokens":80,"cacheReadInputTokens":39901,"cacheCreationInputTokens":5144}}});
        let own = TokenUsage { input: 10, output: 41, cache_read: 22021, cache_write: 1003 };
        assert_eq!(usage(turns.step(&resumed, &mut pending, &mut streamed)), vec![(Some("claude-haiku-4-5".into()), own)]);
        // From there on, what moved.
        let next = json!({"type":"result","subtype":"success","is_error":false,
            "usage":{"input_tokens":10,"cache_creation_input_tokens":500,"cache_read_input_tokens":23000,"output_tokens":30},
            "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":897,"outputTokens":8,"cacheReadInputTokens":0,"cacheCreationInputTokens":0},
                "claude-haiku-4-5":{"inputTokens":30,"outputTokens":110,"cacheReadInputTokens":62901,"cacheCreationInputTokens":5644}}});
        assert_eq!(
            usage(turns.step(&next, &mut pending, &mut streamed)),
            vec![(Some("claude-haiku-4-5".into()), TokenUsage { input: 10, output: 30, cache_read: 23000, cache_write: 500 })]
        );
        // Without the session's model among the totals (an alias), the entry that holds the turn.
        let mut aliased = Turns { resumed: true, model: Some("haiku".into()), ..Default::default() };
        assert_eq!(usage(aliased.step(&resumed, &mut pending, &mut streamed)), vec![(Some("claude-haiku-4-5".into()), own)]);
    }

    #[test]
    fn a_background_task_s_output_keeps_its_end() {
        let log = format!("{}ready on :5173\n", "GET /\n".repeat(10_000));
        let out = task_output(&json!({ "subtype": "success", "response": { "output": log } }));
        assert!(out.len() <= crate::OUTPUT_TAIL && out.ends_with("ready on :5173\n"), "{}", out.len());
        assert_eq!(task_output(&json!({ "subtype": "success", "response": {} })), "");
    }

    #[test]
    fn background_tasks_are_tracked_with_what_started_them() {
        // Recorded (Claude Code 2.1.289, claude-haiku-4-5): a `Monitor`, a background `Bash` and a
        // sub-agent run in the background, each outliving the turn that started it.
        let mut turns = Turns::default();
        let (mut pending, mut streamed) = (HashMap::new(), false);
        let mut step = |v: Value| turns.step(&v, &mut pending, &mut streamed);
        let monitor = json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_m","name":"Monitor","input":{"command":"for i in 1 2; do sleep 4; echo tick $i; done","description":"tick counter","timeout_ms":30000}}]}});
        assert!(matches!(&step(monitor)[..], [AgentEvent::ToolStarted { title, detail, .. }] if title == "Monitor" && detail == "tick counter"));
        let changed = |tasks: Value| json!({"type":"system","subtype":"background_tasks_changed","tasks":tasks});
        // The set comes before `task_started` names the call: a shell until then.
        let ev = step(changed(json!([{"task_id":"brc5","task_type":"local_bash","description":"tick counter"}])));
        assert!(matches!(&ev[..], [AgentEvent::Background(b)] if b[0].kind == crate::BackgroundKind::Shell && b[0].call.is_none()));
        let ev = step(json!({"type":"system","subtype":"task_started","task_id":"brc5","tool_use_id":"toolu_m","description":"tick counter","is_backgrounded":true,"task_type":"local_bash"}));
        let Some(AgentEvent::Background(b)) = ev.last() else { panic!("{ev:?}") };
        assert_eq!(
            b[..],
            [crate::BackgroundTask { id: "brc5".into(), kind: crate::BackgroundKind::Monitor, title: "tick counter".into(), call: Some("toolu_m".into()), readable: true, stoppable: true }]
        );
        // A sub-agent detached from the start, alongside; housekeeping tasks aren't shown.
        step(json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_a","name":"Agent","input":{"description":"Background sleep task","prompt":"…","run_in_background":true}}]}}));
        let ev = step(changed(json!([
            {"task_id":"brc5","task_type":"local_bash","description":"tick counter"},
            {"task_id":"ae33","task_type":"local_agent","description":"Background sleep task"},
            {"task_id":"w1","task_type":"local_bash","description":"watch settings","ambient":true}
        ])));
        let Some(AgentEvent::Background(b)) = ev.last() else { panic!("{ev:?}") };
        assert_eq!(b.iter().map(|t| (t.id.as_str(), t.kind)).collect::<Vec<_>>(), [("brc5", crate::BackgroundKind::Monitor), ("ae33", crate::BackgroundKind::Agent)]);
        let ev = step(json!({"type":"system","subtype":"task_started","task_id":"ae33","tool_use_id":"toolu_a","description":"Background sleep task","subagent_type":"general-purpose","is_backgrounded":true,"spawn_depth":1,"task_type":"local_agent"}));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Task { id, description: Some(_), .. } if id == "toolu_a")));
        assert!(!b[1].readable, "a sub-agent's output isn't a shell's");
        // What the sub-agent does is its row's activity; its own commands aren't the session's tasks.
        let ev = step(json!({"type":"assistant","parent_tool_use_id":"toolu_a","message":{"content":[{"type":"tool_use","id":"toolu_k","name":"Bash","input":{"command":"sleep 6","description":"Sleep for 6 seconds"}}]}}));
        assert_eq!(ev, [AgentEvent::TaskStep { task: "toolu_a".into(), title: "Run command".into(), detail: "sleep 6".into() }]);
        let ev = step(json!({"type":"system","subtype":"task_started","task_id":"bgvn","owned_by_subagent":true,"tool_use_id":"toolu_k","description":"Sleep for 6 seconds","is_backgrounded":false,"task_type":"local_bash"}));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Background(_))), "{ev:?}");
        // Done: the set empties first, then the notification brings the sub-agent's answer.
        let ev = step(changed(json!([])));
        assert_eq!(ev, [AgentEvent::Background(vec![])]);
        let ev = step(json!({"type":"system","subtype":"task_notification","task_id":"ae33","tool_use_id":"toolu_a","status":"completed","output_file":"/tmp/x.output","summary":"kid done."}));
        assert_eq!(
            ev,
            [
                AgentEvent::Task { id: "toolu_a".into(), description: None, activity: None, tool_uses: None, done: Some(true) },
                AgentEvent::ToolFinished { id: "toolu_a".into(), output: "kid done.".into(), ok: true }
            ]
        );
        // A shell's notification has no answer to give.
        let ev = step(json!({"type":"system","subtype":"task_notification","task_id":"brc5","tool_use_id":"toolu_m","status":"completed","summary":"Monitor \"tick counter\" stream ended"}));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::ToolFinished { .. })), "{ev:?}");
        assert!(turns.background.started.is_empty() && turns.background.monitors.is_empty(), "nothing kept once they're over");
    }

    #[test]
    fn a_task_that_reports_its_end_leaves_the_set_without_a_new_one() {
        let mut turns = Turns::default();
        let mut step = |v: Value| turns.step(&v, &mut Default::default(), &mut Default::default());
        let changed = |tasks: Value| json!({"type":"system","subtype":"background_tasks_changed","tasks":tasks});
        step(changed(json!([{"task_id":"ag1","task_type":"local_agent","description":"Turn-end card"},{"task_id":"sh1","task_type":"local_bash","description":"wait loop"}])));
        step(json!({"type":"system","subtype":"task_started","task_id":"ag1","tool_use_id":"toolu_a","description":"Turn-end card","is_backgrounded":true,"task_type":"local_agent"}));
        // Its notification comes, and no new set after it.
        let ev = step(json!({"type":"system","subtype":"task_notification","task_id":"ag1","tool_use_id":"toolu_a","status":"completed","summary":"done"}));
        let left: Vec<String> = ev.iter().find_map(|e| match e {
            AgentEvent::Background(list) => Some(list.iter().map(|t| t.id.clone()).collect()),
            _ => None,
        }).expect("the set again");
        assert_eq!(left, ["sh1"], "the finished agent is gone; the shell still runs");
    }
}
