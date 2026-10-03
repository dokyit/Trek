//! Claude Code via the user's own `claude` binary (stream-json + stdio control protocol).
//! Trek never reads Claude credentials; the CLI handles its own login.

use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, StderrTail, Step, clip, load_image, mcp_servers_json, plan_row, plan_title};
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trek_core::{Effort, TokenUsage, detect};

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

/// The CLI's answer to `--resume` with a session it doesn't have (deleted by hand, or cleaned up
/// after `cleanupPeriodDays`): an error result before the session starts, then it exits.
fn session_missing(v: &Value) -> bool {
    v["type"] == "result"
        && v["is_error"] == true
        && v["errors"].as_array().into_iter().flatten().any(|e| e.as_str().is_some_and(|e| e.starts_with("No conversation found")))
}

/// A running `claude` process.
struct Cli {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: StderrTail,
}

impl Cli {
    fn spawn(bin: &std::path::Path, config: &SessionConfig, mcp_file: Option<&TempFile>) -> Result<Cli> {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(cli_args(config));
        if let Some(file) = mcp_file {
            cmd.arg("--mcp-config").arg(&file.0);
        }
        cmd.current_dir(&config.cwd)
            .env("PATH", detect::login_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().context("failed to start claude")?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let stderr = StderrTail::capture(child.stderr.take().unwrap(), "claude");
        Ok(Cli { child, stdin, stdout, stderr })
    }

    /// Send the `initialize` control request; its id, to know the response.
    async fn initialize(&mut self, ctl: &mut Control) -> Result<String> {
        let init = ctl.request("initialize", json!({}));
        write_line(&mut self.stdin, &init).await?;
        Ok(init["request_id"].as_str().unwrap_or_default().to_string())
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
    /// A result held back for unread messages: its cost, and when to stop waiting for Claude to
    /// take them in (they may never come, and the turn mustn't hang).
    waiting: Option<(Option<f64>, tokio::time::Instant)>,
    /// The limit Claude's last `rate_limit_event` said was hit: when it resets, and which it is.
    rejected: Option<(Option<i64>, crate::LimitScope)>,
    /// This turn has reported its limit (`AgentEvent::LimitReached`).
    limited: bool,
    /// Each model's tokens as the last `result` counted them: `modelUsage` runs for the whole
    /// session, so a turn's share is how far it moved.
    models: HashMap<String, TokenUsage>,
    /// The session was resumed (or forked) into this process: its first `modelUsage` carries
    /// the session's earlier turns, with nothing here to tell them apart from this one's.
    resumed: bool,
    /// The model the session said it runs (`system init`).
    model: Option<String>,
}

/// How long a held result waits for Claude to start on the messages after it. It starts within
/// a fraction of a second.
const STEER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

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
            return vec![];
        }
        let mut out = translate(v, pending, streamed_text);
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
                self.waiting = Some((v["total_cost_usd"].as_f64(), tokio::time::Instant::now() + STEER_WAIT));
                out.retain(|e| !matches!(e, AgentEvent::TurnComplete { .. }));
            }
        }
        out
    }

    /// What a `result` says its turn used, per model (sub-agents and Claude Code's own helper
    /// calls may run on another one). Without `modelUsage`, the turn's `usage` (its main model).
    fn usage(&mut self, v: &Value) -> Vec<AgentEvent> {
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let own = TokenUsage { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_write: n("cache_creation_input_tokens") };
        let Some(models) = v["modelUsage"].as_object() else {
            return if own.is_empty() { vec![] } else { vec![AgentEvent::Usage { model: None, tokens: own }] };
        };
        let totals: Vec<(&String, TokenUsage)> = models
            .iter()
            .map(|(model, u)| {
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                (model, TokenUsage { input: n("inputTokens"), output: n("outputTokens"), cache_read: n("cacheReadInputTokens"), cache_write: n("cacheCreationInputTokens") })
            })
            .collect();
        if std::mem::take(&mut self.resumed) {
            // The first result of a resumed session: its totals are the baseline from here on,
            // and the turn's own `usage` (its main model's) is all that's known of this turn.
            // Helper and sub-agent calls in this one turn go uncounted rather than overcounted.
            let main = self.model.as_deref().filter(|m| models.contains_key(*m)).map(String::from).or_else(|| {
                // Which entry the turn ran on: one that holds at least the turn, the busiest.
                let holds = |t: &TokenUsage| t.input >= own.input && t.output >= own.output && t.cache_read >= own.cache_read && t.cache_write >= own.cache_write;
                totals.iter().filter(|(_, t)| holds(t)).max_by_key(|(_, t)| t.output).map(|(m, _)| m.to_string())
            });
            self.models = totals.into_iter().map(|(m, t)| (m.clone(), t)).collect();
            return if own.is_empty() { vec![] } else { vec![AgentEvent::Usage { model: main, tokens: own }] };
        }
        let mut out = vec![];
        for (model, total) in totals {
            let tokens = total.since(&self.models.get(model).copied().unwrap_or_default());
            self.models.insert(model.clone(), total);
            if !tokens.is_empty() {
                out.push(AgentEvent::Usage { model: Some(model.clone()), tokens });
            }
        }
        out
    }

    /// Claude never took in what it was sent after the held result: the turn ends with it.
    fn give_up(&mut self) -> Option<AgentEvent> {
        let (cost_usd, _) = self.waiting.take()?;
        tracing::warn!("claude: {} message(s) sent mid-turn were never taken in", self.unread.len());
        self.unread.clear();
        Some(AgentEvent::TurnComplete { cost_usd, error: None })
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
        Some(TempFile::write("mcp", &serde_json::to_string(&json!({ "mcpServers": mcp_servers_json(&config.mcp_servers) }))?)?)
    };
    let mut cli = Cli::spawn(&bin, &config, mcp_file.as_ref())?;
    let mut ctl = Control { next_id: 0 };
    let mut init_id = cli.initialize(&mut ctl).await?;
    // Outstanding `get_context_usage` requests; their responses become `Context` events.
    let mut context_requests: HashSet<String> = HashSet::new();

    // Inputs of pending permission requests, echoed back as `updatedInput` on allow.
    let mut pending: HashMap<String, Value> = HashMap::new();
    let mut streamed_text = false;
    let mut hand_holding = config.hand_holding;
    // In plan mode (as Claude last reported it): access changes wait until the plan is approved.
    let mut planning = config.plan;
    let mut in_turn = false;
    let mut turns = Turns { resumed: config.resume.is_some(), ..Default::default() };
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
                            if events.send(AgentEvent::TurnComplete { cost_usd: None, error: Some("Interrupted".into()) }).await.is_err() {
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
                    Command::Shutdown => break,
                }
            }
            _ = tokio::time::sleep_until(turns.waiting.map_or_else(tokio::time::Instant::now, |(_, at)| at)), if turns.waiting.is_some() => {
                if let Some(ev) = turns.give_up() {
                    in_turn = false;
                    if events.send(ev).await.is_err() {
                        return Ok(());
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
                    let _ = cli.child.start_kill();
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
    let _ = cli.child.start_kill();
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
        assert!(matches!(ev.last(), Some(AgentEvent::TurnComplete { cost_usd: Some(c), error: None }) if *c > 0.0));
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
        assert_eq!(turns.give_up(), Some(AgentEvent::TurnComplete { cost_usd: Some(0.5), error: None }));
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
        assert_eq!(ev[end], AgentEvent::TurnComplete { cost_usd: Some(0.0), error: Some(message.into()) });

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
        assert!(turns.step(&v, &mut HashMap::new(), &mut false).is_empty());
        assert_eq!(turns.rejected, None);
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
            ev.into_iter().filter_map(|e| if let AgentEvent::Usage { model, tokens } = e { Some((model, tokens)) } else { None }).collect()
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
    fn a_resumed_session_counts_only_its_own_turns() {
        let mut pending = HashMap::new();
        let mut streamed = false;
        let usage = |ev: Vec<AgentEvent>| -> Vec<(Option<String>, TokenUsage)> {
            ev.into_iter().filter_map(|e| if let AgentEvent::Usage { model, tokens } = e { Some((model, tokens)) } else { None }).collect()
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
}
