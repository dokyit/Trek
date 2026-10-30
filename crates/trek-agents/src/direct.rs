//! Direct providers (API keys and local servers). Phase 2 provides full native agent tool
//! execution (read_file, edit_file, write_file, bash) with HandHolding permission gates and
//! streaming tool-call accumulation, including fallback XML tool-call parsing for fine-tuned models.

pub mod executor;
pub mod script;

use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, load_image};
use anyhow::{Context as _, Result, bail};
use executor::{ExecutorGone, LocalExecutor, ToolExecutor};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_core::catalog::{DirectProvider, ModelInfo, Wire, direct_provider};
use trek_core::settings::secrets;
use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost};

/// How a Direct session runs beyond its `SessionConfig`. Trek's own sessions use the defaults
/// (`run`); `trek-bench` sets the rest (`run_with`).
pub struct DirectOptions {
    /// The key to send. `None`: the provider's saved key, from the keychain.
    pub api_key: Option<Option<String>>,
    /// A provider that isn't in the catalog (any OpenAI- or Anthropic-compatible endpoint), in
    /// place of looking `SessionConfig::agent`'s id up.
    pub provider: Option<DirectProvider>,
    /// Where requests go, in place of the provider's own endpoint: the base URL for the OpenAI
    /// wire (`…/v1`), the API's root or its messages endpoint for Anthropic's.
    pub base_url: Option<String>,
    /// Where the tools run.
    pub executor: Arc<dyn ToolExecutor>,
    /// Where each `script` run keeps its JSONL log of calls (`None`: no log). The tool itself
    /// is on with `SessionConfig::script`.
    pub script_log_dir: Option<PathBuf>,
    /// Model requests one prompt may take before the answer is cut off.
    pub max_turns: u32,
    /// How long one `bash` command may run (`None`: as long as it takes).
    pub command_timeout: Option<Duration>,
    /// No new model request, and no further tool call, after this: the one running finishes.
    pub deadline: Option<Instant>,
    /// The whole system prompt, in place of Trek's (`SessionConfig::instructions` still follow).
    pub system_prompt: Option<String>,
    pub observer: Option<Arc<dyn Observer>>,
}

impl Default for DirectOptions {
    fn default() -> Self {
        DirectOptions {
            api_key: None,
            provider: None,
            base_url: None,
            executor: Arc::new(LocalExecutor),
            script_log_dir: None,
            max_turns: 30,
            command_timeout: None,
            deadline: None,
            system_prompt: None,
            observer: None,
        }
    }
}

/// Why an answer to a prompt ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// The model ended its turn.
    EndTurn,
    /// `DirectOptions::max_turns` requests were made.
    MaxTurns,
    /// `DirectOptions::deadline` passed.
    TimeLimit,
    /// A request or the executor failed, or the turn was stopped.
    Error(String),
}

/// What a session did, told as it happens and in order (all from the session's own task), for a
/// benchmark's log. Every method does nothing unless implemented.
pub trait Observer: Send + Sync {
    /// Model request `n` (1-based, counted over the session) is about to be sent.
    fn request_start(&self, _n: u32) {}
    /// Request `n` ended: what it used and cost when the provider said (`None` for an `error`).
    fn request_end(
        &self,
        _n: u32,
        _usage: Option<&TokenUsage>,
        _cost: Option<UsageCost>,
        _elapsed: Duration,
        _error: Option<&str>,
    ) {
    }
    /// The model's text in one answer, when it wrote any.
    fn text(&self, _text: &str) {}
    /// The model called tool `name` with `args` (its JSON, or the raw text when it isn't).
    fn tool_call(&self, _name: &str, _args: &Value) {}
    /// That call's output, as the model is told it.
    fn tool_result(&self, _name: &str, _output: &str, _ok: bool) {}
    /// A `script` call's run (told before its `tool_result`): the calls it made, its output's
    /// size, how long it took.
    fn script_result(&self, _result: &trek_script::ScriptResult) {}
    /// The answer to a prompt ended, and why.
    fn stopped(&self, _why: &Stop) {}
}

/// The error for a failed request: a usage limit for a 429 (said with `AgentEvent::LimitReached`),
/// else `message` as it is.
fn request_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    message: String,
) -> anyhow::Error {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return crate::limits::LimitError(crate::limits::from_response(
            headers,
            message,
            trek_core::store::now_ms(),
        ))
        .into();
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

/// The tools a session offers: the four built in, then `script` when it's on (see `script`).
fn tool_defs(script: bool) -> Vec<Value> {
    let mut tools = direct_tools();
    if script {
        tools.push(script::tool_def());
    }
    tools
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
    /// OpenRouter's `reasoning_details` parts, kept to echo back with the next request: some
    /// providers route a turn through a model that wants its own thinking returned.
    pub reasoning_details: Vec<Value>,
    pub tool_calls: Vec<AccumulatedToolCall>,
    /// What the request used and cost, when the provider said (also sent as `AgentEvent::Usage`).
    pub usage: Option<(TokenUsage, Option<UsageCost>)>,
}

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    run_with(config, DirectOptions::default(), commands, events).await
}

/// `run`, with what `options` set: a key given rather than read from the keychain, another
/// endpoint, the tools run elsewhere, and so on.
pub async fn run_with(
    config: SessionConfig,
    options: DirectOptions,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let AgentId::Direct(provider_id) = &config.agent else {
        unreachable!()
    };
    let provider = match options.provider {
        Some(p) => p,
        None => *direct_provider(provider_id)
            .with_context(|| format!("unknown provider {provider_id}"))?,
    };
    let key = match &options.api_key {
        Some(given) => given.clone(),
        // Read quietly: a key an earlier copy of Trek saved is locked, and the way to it is
        // Settings (which says what macOS will ask), never a password prompt in the middle of
        // a send.
        None => match secrets::api_key_lookup(provider.id) {
            trek_core::keychain::Lookup::Found(k) => Some(k),
            trek_core::keychain::Lookup::Locked if !provider.local => bail!(
                "Your saved {} key is locked: an earlier copy of Trek saved it, and macOS only lets that copy read it. Open Settings → API Keys to allow access or paste it again.",
                provider.name
            ),
            _ => None,
        },
    };
    if !provider.local && key.is_none() {
        bail!(
            "No API key for {}. Add one in Settings → API Keys.",
            provider.name
        );
    }
    let url = endpoint(&provider, options.base_url.as_deref());
    session(config, &provider, &url, key, options, commands, events).await
}

/// Where `provider`'s requests go: the messages endpoint for Anthropic, the base URL for the
/// OpenAI wire. `base_url` replaces the provider's; for Anthropic it may be the API's root
/// (`/v1/messages` is added) or the messages endpoint itself.
pub fn endpoint(provider: &DirectProvider, base_url: Option<&str>) -> String {
    let Some(base) = base_url.map(|b| b.trim_end_matches('/')) else {
        return match provider.wire {
            Wire::Anthropic => ANTHROPIC_MESSAGES_URL.to_string(),
            _ => provider.base_url.to_string(),
        };
    };
    match provider.wire {
        Wire::Anthropic if base.ends_with("/messages") => base.to_string(),
        Wire::Anthropic if base.ends_with("/v1") => format!("{base}/messages"),
        Wire::Anthropic => format!("{base}/v1/messages"),
        _ => base.to_string(),
    }
}

/// A session with `provider`, its requests sent to `url` (the messages endpoint for Anthropic,
/// the base URL for the OpenAI wire).
async fn session(
    config: SessionConfig,
    provider: &DirectProvider,
    url: &str,
    key: Option<String>,
    options: DirectOptions,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let observer = options.observer.clone();
    let observe = |f: &dyn Fn(&dyn Observer)| {
        if let Some(o) = &observer {
            f(o.as_ref());
        }
    };
    let past_deadline = || options.deadline.is_some_and(|d| Instant::now() >= d);
    let tools = tool_defs(config.script);
    let mut requests = 0u32;
    let mut model = config.model.clone().context("Pick a model first")?;
    let mut effort = config.effort;
    let mut hand_holding = config.hand_holding;
    let mut allowed_for_session = HashSet::new();
    let client = reqwest::Client::new();
    events
        .send(AgentEvent::Started {
            native_id: config.resume.clone().unwrap_or_default(),
            model: Some(model.clone()),
        })
        .await?;
    events
        .send(AgentEvent::Billing(if provider.local {
            Billing::Local
        } else {
            Billing::Metered
        }))
        .await?;

    let system = match &options.system_prompt {
        Some(own) => {
            let mut p = own.clone();
            if let Some(extra) = &config.instructions {
                p.push_str("\n\n");
                p.push_str(extra);
                p.push('\n');
            }
            p
        }
        None => system_prompt(
            &config.cwd,
            config.read_only,
            config.instructions.as_deref(),
        ),
    };
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
                        events
                            .send(AgentEvent::Notice(format!("Image left out: {e}")))
                            .await?;
                    }
                    // Where this message starts in `history`: a failed first request takes it back
                    // out, so the next message doesn't carry one that was never answered.
                    let turn_start = history.len();
                    history.push(message);

                    // Multi-turn agent loop for tool calls
                    let mut turn_count = 0;
                    let mut turn_error = None;
                    let mut why = None;
                    while turn_count < options.max_turns {
                        if past_deadline() {
                            why = Some(Stop::TimeLimit);
                            break;
                        }
                        turn_count += 1;
                        requests += 1;
                        let n = requests;
                        observe(&|o| o.request_start(n));
                        let sent = Instant::now();
                        let result = match provider.wire {
                            Wire::Anthropic => {
                                anthropic_turn(
                                    &client,
                                    url,
                                    key.as_deref().unwrap_or_default(),
                                    &model,
                                    effort,
                                    &system,
                                    &history,
                                    &tools,
                                    &events,
                                    &commands,
                                    &mut kept,
                                    &mut shutdown,
                                )
                                .await
                            }
                            _ => {
                                let usage = REPORTS_USAGE.contains(&provider.id);
                                openai_turn(
                                    &client,
                                    &config.agent,
                                    url,
                                    key.as_deref(),
                                    provider.local,
                                    usage,
                                    &model,
                                    effort,
                                    &system,
                                    &history,
                                    &tools,
                                    &events,
                                    &commands,
                                    &mut kept,
                                    &mut shutdown,
                                )
                                .await
                            }
                        };
                        let elapsed = sent.elapsed();
                        match &result {
                            Ok(resp) => {
                                let (tokens, cost) = match &resp.usage {
                                    Some((t, c)) => (Some(t), *c),
                                    None => (None, None),
                                };
                                observe(&|o| o.request_end(n, tokens, cost, elapsed, None));
                                if !resp.text.is_empty() {
                                    observe(&|o| o.text(&resp.text));
                                }
                            }
                            Err(e) => {
                                let e = format!("{e:#}");
                                observe(&|o| o.request_end(n, None, None, elapsed, Some(&e)));
                            }
                        }
                        match result {
                            Ok(resp) => {
                                let anthropic = resp.content.is_some();
                                match resp.content {
                                    Some(content) => history.push(content),
                                    None if resp.tool_calls.is_empty() => history
                                        .push(json!({ "role": "assistant", "content": resp.text })),
                                    None => {
                                        let tool_calls_json: Vec<Value> = resp
                                            .tool_calls
                                            .iter()
                                            .map(|c| {
                                                json!({
                                                    "id": c.id,
                                                    "type": "function",
                                                    "function": {
                                                        "name": c.name,
                                                        "arguments": c.arguments
                                                    }
                                                })
                                            })
                                            .collect();
                                        let mut message = json!({
                                            "role": "assistant",
                                            "content": if resp.text.is_empty() { Value::Null } else { Value::String(resp.text.clone()) },
                                            "tool_calls": tool_calls_json
                                        });
                                        if !resp.reasoning_details.is_empty() {
                                            message["reasoning_details"] =
                                                json!(resp.reasoning_details);
                                        }
                                        history.push(message);
                                    }
                                }
                                if resp.tool_calls.is_empty() {
                                    why = Some(Stop::EndTurn);
                                    break;
                                }

                                // Every call gets a result, even one a Stop cut short or never
                                // started: providers reject a history with a call left unanswered.
                                let mut results = Vec::new();
                                let mut stopped = None;
                                let mut timed_out = false;
                                for (i, call) in resp.tool_calls.iter().enumerate() {
                                    let (title, detail) = tool_display(&call.name, &call.arguments);
                                    events
                                        .send(AgentEvent::ToolStarted {
                                            id: call.id.clone(),
                                            title: title.to_string(),
                                            detail: detail.clone(),
                                        })
                                        .await?;
                                    if observer.is_some() {
                                        let args = serde_json::from_str(&call.arguments)
                                            .unwrap_or_else(|_| {
                                                Value::String(call.arguments.clone())
                                            });
                                        observe(&|o| o.tool_call(&call.name, &args));
                                    }

                                    let ran = execute_tool(
                                        &call.name,
                                        &call.arguments,
                                        &config.cwd,
                                        config.read_only,
                                        hand_holding,
                                        &mut allowed_for_session,
                                        &options,
                                        config.script,
                                        &events,
                                        &commands,
                                        &mut kept,
                                        &mut shutdown,
                                    )
                                    .await;
                                    let (output, ok, lines, stop) = match ran {
                                        Ok((output, ok, lines)) => (output, ok, lines, None),
                                        // A Stop (or shutdown) while the tool ran or waited for
                                        // permission: it ends this turn, not the session.
                                        Err(e) => (format!("{e:#}"), false, None, Some(e)),
                                    };
                                    observe(&|o| o.tool_result(&call.name, &output, ok));

                                    if let Some((added, removed)) = lines {
                                        events
                                            .send(AgentEvent::ToolLines {
                                                id: call.id.clone(),
                                                added,
                                                removed,
                                            })
                                            .await?;
                                    }

                                    events
                                        .send(AgentEvent::ToolFinished {
                                            id: call.id.clone(),
                                            output: output.clone(),
                                            ok,
                                        })
                                        .await?;
                                    results.push((call.id.clone(), output, ok));

                                    if let Some(e) = stop {
                                        for rest in &resp.tool_calls[i + 1..] {
                                            results.push((
                                                rest.id.clone(),
                                                "Not run: the turn was stopped".to_string(),
                                                false,
                                            ));
                                        }
                                        stopped = Some(e);
                                        break;
                                    }
                                    // Past the time limit, the call that ran is the last.
                                    if past_deadline() && i + 1 < resp.tool_calls.len() {
                                        for rest in &resp.tool_calls[i + 1..] {
                                            results.push((
                                                rest.id.clone(),
                                                "Not run: the time limit was reached".to_string(),
                                                false,
                                            ));
                                        }
                                        timed_out = true;
                                        break;
                                    }
                                }
                                push_tool_results(&mut history, anthropic, results);
                                if let Some(e) = stopped {
                                    turn_error.get_or_insert_with(|| format!("{e:#}"));
                                    break;
                                }
                                if shutdown {
                                    break;
                                }
                                if timed_out {
                                    why = Some(Stop::TimeLimit);
                                    break;
                                }
                                if turn_count >= options.max_turns {
                                    // The runaway guard ends the loop next check; say so, or the
                                    // answer looks like it just stopped mid-work.
                                    events.send(AgentEvent::Notice(format!(
                                        "Stopped at {} tool-call rounds in one answer — send a message and it picks up where it left off.",
                                        options.max_turns
                                    ))).await?;
                                    why = Some(Stop::MaxTurns);
                                    break;
                                }
                            }
                            Err(e) => {
                                // Only the message itself: once tools have run, the history ends
                                // with their results, a whole exchange to keep.
                                if history.len() == turn_start + 1 {
                                    history.pop();
                                }
                                if let Some(crate::limits::LimitError(limit)) = e.downcast_ref() {
                                    events.send(limit.clone().event()).await?;
                                }
                                turn_error.get_or_insert_with(|| format!("{e:#}"));
                                break;
                            }
                        }
                    }
                    let why = match (&turn_error, why) {
                        (Some(e), _) => Stop::Error(e.clone()),
                        (None, Some(why)) => why,
                        (None, None) => Stop::Error("Interrupted".into()),
                    };
                    observe(&|o| o.stopped(&why));
                    if let Some(err) = turn_error {
                        error.get_or_insert(err);
                    }

                    while let Some(cmd) = kept.pop_front() {
                        match cmd {
                            Command::Prompt { text, images } => {
                                next = Some((text, images));
                                break;
                            }
                            Command::SetModel {
                                model: m,
                                effort: e,
                            } => {
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
            Command::SetModel {
                model: m,
                effort: e,
            } => {
                model = m;
                effort = e;
            }
            Command::SetModes { effort: e, .. } => effort = e,
            Command::SetHandHolding(h) => hand_holding = h,
            Command::Answer { .. } => {}
            Command::Shutdown => break,
            Command::Interrupt
            | Command::Respond { .. }
            | Command::ReadTask { .. }
            | Command::StopTask { .. } => {}
        }
    }
    Ok(())
}

/// The results of one answer's tool calls, `(id, output, ok)`, in the shape the wire wants:
/// one user message of `tool_result` blocks for Anthropic, a `tool` message each otherwise.
fn push_tool_results(
    history: &mut Vec<Value>,
    anthropic: bool,
    results: Vec<(String, String, bool)>,
) {
    if anthropic {
        let blocks: Vec<Value> = results
            .into_iter()
            .map(|(id, output, ok)| json!({ "type": "tool_result", "tool_use_id": id, "content": output, "is_error": !ok }))
            .collect();
        history.push(json!({ "role": "user", "content": blocks }));
    } else {
        for (id, output, _) in results {
            history.push(json!({ "role": "tool", "tool_call_id": id, "content": output }));
        }
    }
}

/// Whether Auto mode may run `cmd` without asking: a read-only command from a short list, run
/// as it is. The shell runs whatever else the line holds, so anything that chains, substitutes or
/// redirects is out, and so are the flags that make a listed tool write, delete or run commands.
fn is_safe_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.contains([';', '&', '|', '>', '<', '`', '\n', '\r']) || trimmed.contains("$(") {
        return false;
    }
    let safe_prefixes = [
        "git status",
        "git diff",
        "git log",
        "git show",
        "git branch",
        "cargo check",
        "cargo test",
        "cargo clippy",
        "ls",
        "cat",
        "head",
        "tail",
        "grep",
        "rg",
        "find",
        "fd",
        "pwd",
        "which",
        "echo",
        "wc",
        "stat",
        "file",
    ];
    let Some(prefix) = safe_prefixes
        .iter()
        .find(|p| trimmed == **p || trimmed.starts_with(&format!("{p} ")))
    else {
        return false;
    };
    // The arguments as the shell will see them, near enough: quotes and backslashes go, so
    // `'-delete'` or `-de''lete` reads as `-delete`.
    let args: Vec<String> = trimmed[prefix.len()..]
        .split_whitespace()
        .map(|a| a.replace(['\'', '"', '\\'], ""))
        .collect();
    // A cluster of short flags (`-df`) holding any of `letters`.
    let short = |a: &str, letters: &str| {
        a.starts_with('-') && !a.starts_with("--") && a.chars().skip(1).any(|c| letters.contains(c))
    };
    !args.iter().any(|a| {
        let a = a.as_str();
        match *prefix {
            "find" => matches!(
                a,
                "-delete"
                    | "-exec"
                    | "-execdir"
                    | "-ok"
                    | "-okdir"
                    | "-fprint"
                    | "-fprint0"
                    | "-fprintf"
                    | "-fls"
            ),
            "fd" => short(a, "xX") || a.starts_with("--exec"),
            // Listing only: a name creates a branch, and these flags delete, move, copy or
            // retarget one.
            "git branch" => {
                !a.starts_with('-')
                    || short(a, "dDmMcCfu")
                    || [
                        "--delete",
                        "--move",
                        "--copy",
                        "--force",
                        "--set-upstream",
                        "--unset-upstream",
                        "--edit-description",
                        "--track",
                        "--no-track",
                    ]
                    .iter()
                    .any(|f| a.starts_with(f))
            }
            "git diff" | "git log" | "git show" => a.starts_with("--output"),
            "rg" => a.starts_with("--pre"),
            "cargo check" | "cargo test" | "cargo clippy" => a == "--fix",
            _ => false,
        }
    })
}

fn tool_display(name: &str, args_json: &str) -> (&'static str, String) {
    let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    match name {
        "bash" => {
            let cmd = parsed
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or(args_json);
            ("Run command", cmd.to_string())
        }
        "read_file" => {
            let path = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or(args_json);
            ("Read", path.to_string())
        }
        "write_file" => {
            let path = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or(args_json);
            ("Write", path.to_string())
        }
        "edit_file" => {
            let path = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or(args_json);
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

    // What arrives while the card is open (a steering message, a mode change) waits in `later`
    // and goes behind what was already kept once this is answered. Commands already in `kept`
    // can't answer this request, so only the channel is read here.
    let mut later = Vec::new();
    let answer = loop {
        let cmd = match commands.recv().await {
            Ok(cmd) => cmd,
            Err(_) => {
                *shutdown = true;
                kept.extend(later);
                bail!("Commands channel closed");
            }
        };
        match cmd {
            Command::Respond {
                request_id: id,
                decision,
            } if id == request_id => break Ok(decision),
            Command::Interrupt => break Err("Interrupted"),
            Command::Shutdown => {
                *shutdown = true;
                break Err("Shutdown");
            }
            other => later.push(other),
        }
    };
    kept.extend(later);
    events
        .send(AgentEvent::PermissionResolved { request_id })
        .await?;
    answer.map_err(|e| anyhow::anyhow!(e))
}

/// Whether `write_file`/`edit_file` on `path_str` needs an answer first, and the key that
/// "allow for the session" is remembered under. In the project folder only Supervised asks; a
/// path outside it asks below Full access (a write there, like `~/.zshrc`, runs code later and
/// would get round the prompt `bash` has), and an allowance covers that one path only.
fn edit_permission(
    tool: &str,
    hand_holding: HandHolding,
    path_str: &str,
    cwd: &Path,
    allowed_for_session: &HashSet<String>,
) -> (bool, String) {
    let path = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        cwd.join(path_str)
    };
    if crate::acp::within(&path, cwd) {
        let ask = hand_holding == HandHolding::Supervised && !allowed_for_session.contains(tool);
        return (ask, tool.to_string());
    }
    let key = format!("{tool}:outside:{}", path.display());
    let ask = hand_holding != HandHolding::FullAccess && !allowed_for_session.contains(&key);
    (ask, key)
}

/// Whether `tool` (`bash`, `write_file` or `edit_file`) may go ahead on `target` (the command,
/// or the path): asks the user when `hand_holding` says to, and remembers "allow for the
/// session". `Ok(false)`: denied.
#[allow(clippy::too_many_arguments)]
async fn permitted(
    tool: &str,
    target: &str,
    cwd: &Path,
    hand_holding: HandHolding,
    allowed_for_session: &mut HashSet<String>,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<bool> {
    let (need_ask, title, session_key) = match tool {
        "bash" => {
            let need_ask = match hand_holding {
                HandHolding::FullAccess => false,
                HandHolding::Auto => {
                    !is_safe_command(target) && !allowed_for_session.contains(target)
                }
                HandHolding::AutoAcceptEdits | HandHolding::Supervised => {
                    !allowed_for_session.contains(target)
                }
            };
            (need_ask, "Run command", target.to_string())
        }
        _ => {
            let (need_ask, key) =
                edit_permission(tool, hand_holding, target, cwd, allowed_for_session);
            let title = if tool == "write_file" {
                "Write file"
            } else {
                "Edit file"
            };
            (need_ask, title, key)
        }
    };
    if !need_ask {
        return Ok(true);
    }
    Ok(
        match ask_permission(title, target, events, commands, kept, shutdown).await? {
            Decision::Allow => true,
            Decision::AllowForSession => {
                allowed_for_session.insert(session_key);
                true
            }
            Decision::Deny => false,
        },
    )
}

async fn execute_tool(
    name: &str,
    args_json: &str,
    cwd: &Path,
    read_only: bool,
    hand_holding: HandHolding,
    allowed_for_session: &mut HashSet<String>,
    options: &DirectOptions,
    script: bool,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    let exec = options.executor.as_ref();
    match name {
        "bash" => {
            if read_only {
                return Ok((
                    "Permission denied: commands are disabled in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let parsed: Value =
                serde_json::from_str(args_json).unwrap_or_else(|_| json!({ "command": args_json }));
            let command = parsed
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or(args_json)
                .trim();
            if command.is_empty() {
                return Ok(("Error: empty command".into(), false, None));
            }
            if !permitted(
                "bash",
                command,
                cwd,
                hand_holding,
                allowed_for_session,
                events,
                commands,
                kept,
                shutdown,
            )
            .await?
            {
                return Ok(("Permission denied by user".into(), false, None));
            }
            let (out, ok) = run_bash_command(
                exec,
                command,
                cwd,
                options.command_timeout,
                commands,
                kept,
                shutdown,
            )
            .await?;
            Ok((out, ok, None))
        }
        "read_file" => {
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_default();
            if path_str.is_empty() {
                return Ok(("Error: path parameter is required".into(), false, None));
            }
            let start = parsed
                .get("start_line")
                .and_then(|s| s.as_u64())
                .map(|s| s as usize);
            let end = parsed
                .get("end_line")
                .and_then(|e| e.as_u64())
                .map(|e| e as usize);
            let (out, ok) = run_read_file(exec, path_str, start, end, cwd).await?;
            Ok((out, ok, None))
        }
        "write_file" => {
            if read_only {
                return Ok((
                    "Permission denied: session is in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_default();
            let content = parsed
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            if path_str.is_empty() {
                return Ok(("Error: path parameter is required".into(), false, None));
            }
            if !permitted(
                "write_file",
                path_str,
                cwd,
                hand_holding,
                allowed_for_session,
                events,
                commands,
                kept,
                shutdown,
            )
            .await?
            {
                return Ok(("Permission denied by user".into(), false, None));
            }
            run_write_file(exec, path_str, content, cwd).await
        }
        "edit_file" => {
            if read_only {
                return Ok((
                    "Permission denied: session is in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let path_str = parsed
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_default();
            let old_str = parsed
                .get("old_str")
                .and_then(|o| o.as_str())
                .unwrap_or_default();
            let new_str = parsed
                .get("new_str")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if path_str.is_empty() || old_str.is_empty() {
                return Ok((
                    "Error: path and old_str parameters are required".into(),
                    false,
                    None,
                ));
            }
            if !permitted(
                "edit_file",
                path_str,
                cwd,
                hand_holding,
                allowed_for_session,
                events,
                commands,
                kept,
                shutdown,
            )
            .await?
            {
                return Ok(("Permission denied by user".into(), false, None));
            }
            run_edit_file(exec, path_str, old_str, new_str, cwd).await
        }
        // The fifth tool, when it's on (see `script`): a program whose calls run on the same
        // executor. Its approvals are asked here while it waits; a Stop drops it like it does a
        // command, and the calls it has in flight with it.
        "script" if script => {
            if read_only {
                return Ok((
                    "Permission denied: scripts are disabled in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
            let code = parsed["code"].as_str().unwrap_or_default();
            if code.trim().is_empty() {
                return Ok(("Error: code parameter is required".into(), false, None));
            }
            let (approvals_tx, mut approvals) = tokio::sync::mpsc::unbounded_channel();
            let tools = Arc::new(script::ScriptTools {
                executor: options.executor.clone(),
                cwd: cwd.to_path_buf(),
                command_timeout: options.command_timeout,
                approvals: (hand_holding != HandHolding::FullAccess).then_some(approvals_tx),
            });
            let limits = trek_script::ScriptLimits {
                log_dir: options.script_log_dir.clone(),
                ..Default::default()
            };
            let run = trek_script::run_script(code, tools, limits);
            tokio::pin!(run);
            let result = loop {
                tokio::select! {
                    result = &mut run => break result,
                    Some(ask) = approvals.recv() => {
                        let ok = permitted(
                            ask.tool,
                            &ask.target,
                            cwd,
                            hand_holding,
                            allowed_for_session,
                            events,
                            commands,
                            kept,
                            shutdown,
                        )
                        .await?;
                        let _ = ask.reply.send(ok);
                    }
                    cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
                }
            };
            if let Some(o) = &options.observer {
                o.script_result(&result);
            }
            let ok = result.error.is_none() && !result.timed_out;
            Ok((result.to_model_text(), ok, None))
        }
        unknown => Ok((
            format!(
                "Unknown tool: {unknown}. Available tools: bash, read_file, write_file, edit_file"
            ),
            false,
            None,
        )),
    }
}

/// `run` to its end, unless a Stop (or shutdown) comes first: then it's dropped (a command and
/// what it started end with it) and this is the error. A message, a model or an access level
/// sent meanwhile waits in `kept`.
async fn until_stopped<T>(
    run: impl std::future::Future<Output = T>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<T> {
    tokio::pin!(run);
    loop {
        tokio::select! {
            res = &mut run => return Ok(res),
            cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
        }
    }
}

/// Most of a command's output the model is told, in bytes.
const MAX_OUT: usize = 50_000;

async fn run_bash_command(
    exec: &dyn ToolExecutor,
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool)> {
    let ran =
        match until_stopped(exec.exec(command, cwd, timeout), commands, kept, shutdown).await? {
            Ok(ran) => ran,
            Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => return Err(e),
            Err(e) => return Ok((format!("Failed to execute command: {e:#}"), false)),
        };
    let ok = ran.success();
    let mut out = ran.stdout;
    if !ran.stderr.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&ran.stderr);
    }
    if out.len() > MAX_OUT {
        let mut cut = MAX_OUT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n... [output truncated]");
    }
    if ran.timed_out {
        let limit = timeout.map_or(0, |t| t.as_secs());
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "[Command timed out after {limit} seconds and was stopped]"
        ));
    }
    if out.trim().is_empty() {
        out = "(Command finished with no output)".to_string();
    }
    Ok((out, ok))
}

/// `path_str` as the executor gets it: relative paths are in `cwd`.
fn tool_path(path_str: &str, cwd: &Path) -> PathBuf {
    if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        cwd.join(path_str)
    }
}

/// A file's text through `exec`: `Err` is what to tell the model (the file isn't there or isn't
/// text), and the outer `Err` an executor that's gone.
async fn read_text(
    exec: &dyn ToolExecutor,
    path: &Path,
    path_str: &str,
) -> Result<std::result::Result<String, String>> {
    match exec.read(path).await {
        Ok(Some(bytes)) => Ok(String::from_utf8(bytes)
            .map_err(|_| format!("Error reading {path_str}: stream did not contain valid UTF-8"))),
        Ok(None) => Ok(Err(format!("File not found: {path_str}"))),
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => Err(e),
        Err(e) => Ok(Err(format!("Error reading {path_str}: {e:#}"))),
    }
}

async fn run_read_file(
    exec: &dyn ToolExecutor,
    path_str: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    cwd: &Path,
) -> Result<(String, bool)> {
    let path = tool_path(path_str, cwd);
    let content = match read_text(exec, &path, path_str).await? {
        Ok(c) => c,
        Err(message) => return Ok((message, false)),
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Ok(("(File is empty)".to_string(), true));
    }
    let start = start_line.unwrap_or(1).max(1) - 1;
    let end = end_line.unwrap_or(lines.len()).min(lines.len());
    if start >= lines.len() {
        return Ok((
            format!(
                "Start line {} is beyond end of file ({} lines)",
                start + 1,
                lines.len()
            ),
            false,
        ));
    }
    if end <= start {
        return Ok((
            format!(
                "End line {} is before start line {}",
                end_line.unwrap_or(lines.len()),
                start + 1
            ),
            false,
        ));
    }
    let mut out = String::new();
    for (idx, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:4} | {}\n", start + idx + 1, line));
    }
    Ok((out, true))
}

async fn run_write_file(
    exec: &dyn ToolExecutor,
    path_str: &str,
    content: &str,
    cwd: &Path,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    let path = tool_path(path_str, cwd);
    let added = content.lines().count() as u32;
    match exec.write(&path, content.as_bytes()).await {
        Ok(()) => Ok((
            format!("Successfully wrote {added} lines to {path_str}"),
            true,
            Some((added, 0)),
        )),
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => Err(e),
        Err(e) => Ok((format!("Error writing {path_str}: {e:#}"), false, None)),
    }
}

async fn run_edit_file(
    exec: &dyn ToolExecutor,
    path_str: &str,
    old_str: &str,
    new_str: &str,
    cwd: &Path,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    let path = tool_path(path_str, cwd);
    let content = match read_text(exec, &path, path_str).await? {
        Ok(c) => c,
        Err(message) => return Ok((message, false, None)),
    };
    let count = content.matches(old_str).count();
    if count == 0 {
        return Ok((
            format!(
                "Error: old_str was not found in {path_str}. Please provide the exact text to match."
            ),
            false,
            None,
        ));
    }
    if count > 1 {
        return Ok((
            format!(
                "Error: old_str matched {count} times in {path_str}. Please provide more surrounding context to uniquely identify the location."
            ),
            false,
            None,
        ));
    }
    let updated = content.replacen(old_str, new_str, 1);
    match exec.write(&path, updated.as_bytes()).await {
        Ok(()) => {
            let removed = old_str.lines().count() as u32;
            let added = new_str.lines().count() as u32;
            Ok((
                format!("Successfully edited {path_str}"),
                true,
                Some((added, removed)),
            ))
        }
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => Err(e),
        Err(e) => Ok((
            format!("Error writing edits to {path_str}: {e:#}"),
            false,
            None,
        )),
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
                let args = remaining
                    .strip_suffix("</function>")
                    .unwrap_or(remaining)
                    .trim();
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
    set(
        &u["cache_creation"]["ephemeral_1h_input_tokens"],
        long_writes,
    );
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
    let written = details["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0)
        .min(prompt - cached);
    let completion = u["completion_tokens"].as_u64().unwrap_or(0);
    let output = completion.max(
        u["total_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(prompt),
    );
    let t = TokenUsage {
        input: prompt - cached - written,
        output,
        cache_read: cached,
        cache_write: written,
    };
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
        let crlf = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| (i, 4));
        let Some((pos, end)) = lf.into_iter().chain(crlf).min_by_key(|(i, _)| *i) else {
            break;
        };
        let block: Vec<u8> = buf.drain(..pos + end).collect();
        let text = String::from_utf8_lossy(&block[..pos]);
        let data = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|s| s.strip_prefix(' ').unwrap_or(s))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !data.is_empty() {
            out.push(data);
        }
    }
    out
}

fn keep_command(
    cmd: Result<Command, async_channel::RecvError>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<()> {
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

/// `tools` (`tool_defs`) in the shape the Messages API takes them.
fn anthropic_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let f = &t["function"];
            json!({ "name": f["name"], "description": f["description"], "input_schema": f["parameters"] })
        })
        .collect()
}

fn anthropic_body(
    model: &str,
    effort: Effort,
    system: &str,
    history: &[Value],
    tools: &[Value],
) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": 64000,
        "stream": true,
        "system": system,
        "messages": history,
        "tools": anthropic_tools(tools),
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
        let e = effort.clamp_to(&[
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ]);
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
    tools: &[Value],
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    let body = anthropic_body(model, effort, system, history, tools);
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
        return Err(request_error(
            status,
            &headers,
            format!("Anthropic API {status}: {msg}"),
        ));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut blocks: Vec<Value> = Vec::new();
    // A tool call's input, streamed as pieces of JSON, by block index.
    let mut inputs: HashMap<usize, String> = HashMap::new();
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
                                Some("input_json_delta") => inputs.entry(i).or_default().push_str(d["partial_json"].as_str().unwrap_or_default()),
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
    let mut reported = None;
    if !usage.is_empty() {
        let cost = trek_core::pricing::request(
            model,
            &AgentId::Direct("anthropic".into()),
            &usage,
            long_writes,
            false,
        );
        reported = Some((usage, cost));
        events
            .send(AgentEvent::Usage {
                model: Some(model.to_string()),
                tokens: usage,
                cost,
            })
            .await?;
    }
    if stop_reason.as_deref() == Some("refusal") {
        bail!("The model declined this request.");
    }
    events.send(AgentEvent::TextDone(text.clone())).await?;
    let mut tool_calls = Vec::new();
    for (i, block) in blocks.iter_mut().enumerate() {
        if block["type"] != "tool_use" {
            continue;
        }
        // Sent back as an object, as the API gave it; the tool takes it as JSON text.
        if let Some(input) = inputs.get(&i).filter(|s| !s.trim().is_empty()) {
            block["input"] = serde_json::from_str(input).unwrap_or_else(|_| json!({}));
        }
        tool_calls.push(AccumulatedToolCall {
            id: block["id"].as_str().unwrap_or_default().to_string(),
            name: block["name"].as_str().unwrap_or_default().to_string(),
            arguments: block["input"].to_string(),
        });
    }
    Ok(TurnResponse {
        text,
        content: Some(json!({ "role": "assistant", "content": blocks })),
        reasoning_details: vec![],
        tool_calls,
        usage: reported,
    })
}

/// A delta's reasoning as text: the `reasoning_content`/`reasoning` strings most providers
/// send, or OpenRouter's `reasoning_details` array (`reasoning.text` parts carry `text`,
/// summaries carry `summary`). Anything else can't be shown.
fn reasoning_text(delta: &Value) -> Option<String> {
    if let Some(s) = delta["reasoning_content"]
        .as_str()
        .or(delta["reasoning"].as_str())
    {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let parts = delta["reasoning_details"].as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|p| p["text"].as_str().or(p["summary"].as_str()))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// A delta's `content` as text: a string for most providers, an array of parts
/// (`[{"type":"text","text":"…"}]`) for a few.
fn content_text(delta: &Value) -> Option<String> {
    let v = &delta["content"];
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    let parts = v.as_array()?;
    let text: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
    (!text.is_empty()).then_some(text)
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
/// The error for a failed request whose body is `text`: the message the API gave, else the text.
fn response_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    text: String,
) -> anyhow::Error {
    let msg = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or(text);
    request_error(status, headers, format!("{status}: {msg}"))
}

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
    tools: &[Value],
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
        "tools": tools,
        "stream": true
    });
    if include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !local && effort != Effort::Off {
        let e = effort.clamp_to(&[Effort::Low, Effort::Medium, Effort::High]);
        if matches!(agent, AgentId::Direct(p) if p == "openrouter") {
            // OpenRouter's unified switch; a raw `reasoning_effort` only reaches providers
            // that take it themselves, others ignore or reject it.
            body["reasoning"] = json!({ "effort": e.as_str() });
        } else {
            body["reasoning_effort"] = json!(e.as_str());
        }
    }
    let mut req = client
        .post(format!("{base_url}/chat/completions"))
        .json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    let resp = send_request(req, commands, kept, shutdown, Duration::from_secs(60)).await?;
    let status = resp.status();
    let resp = if status == reqwest::StatusCode::BAD_REQUEST
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
    {
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        // A model that takes no reasoning effort says so by name: send the request again
        // without it. Any other 400 may be the tools: send it again without those.
        let object = body.as_object_mut().unwrap();
        let sent_effort =
            object.contains_key("reasoning_effort") || object.contains_key("reasoning");
        let retried = if sent_effort && text.to_lowercase().contains("reasoning") {
            object.remove("reasoning_effort");
            object.remove("reasoning");
            true
        } else if status == reqwest::StatusCode::BAD_REQUEST {
            object.remove("tools").is_some()
        } else {
            false
        };
        let mut retry_resp = None;
        if retried {
            let mut retry_req = client
                .post(format!("{base_url}/chat/completions"))
                .json(&body);
            if let Some(k) = key {
                retry_req = retry_req.bearer_auth(k);
            }
            let r =
                send_request(retry_req, commands, kept, shutdown, Duration::from_secs(60)).await?;
            if r.status().is_success() {
                retry_resp = Some(r);
            }
        }
        match retry_resp {
            Some(r) => r,
            None => return Err(response_error(status, &headers, text)),
        }
    } else {
        resp
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        return Err(response_error(status, &headers, text));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut tool_calls: Vec<AccumulatedToolCall> = Vec::new();
    let mut reasoning_details: Vec<Value> = Vec::new();
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
                    // A mid-stream `{"error": …}` frame carries no choices: without this it is
                    // skipped and the real reason dies as "connection closed early".
                    if let Some(e) = v.get("error") {
                        let msg = e["message"].as_str().unwrap_or("the provider reported an error");
                        bail!("{msg}");
                    }
                    usage = openai_usage(&v).or(usage);
                    terminal |= v["choices"].as_array().into_iter().flatten().any(|c| !c["finish_reason"].is_null());
                    let d = &v["choices"][0]["delta"];
                    if let Some(details) = d.get("reasoning_details").and_then(|r| r.as_array()) {
                        reasoning_details.extend(details.iter().cloned());
                    }
                    if let Some(r) = reasoning_text(d) {
                        events.send(AgentEvent::ReasoningDelta(r)).await?;
                    }
                    // Empty content deltas say nothing but still break a reasoning span on the
                    // other end of this channel (MiMo sends one between reasoning deltas), and a
                    // few providers send content parts, not a string.
                    if let Some(t) = content_text(d).filter(|t| !t.is_empty()) {
                        text.push_str(&t);
                        events.send(AgentEvent::TextDelta(t)).await?;
                    }
                    if let Some(calls) = d.get("tool_calls").and_then(|t| t.as_array()) {
                        for (pos, call) in calls.iter().enumerate() {
                            // `index` missing: a single-call chunk is pos 0; several in one chunk
                            // keep their order rather than all merging into slot 0. A call with
                            // an id other than the one in that slot is a new call (one per chunk,
                            // each without an index), so it opens a slot of its own.
                            let mut idx = call.get("index").and_then(|i| i.as_u64()).map(|i| i as usize).unwrap_or(pos);
                            if call.get("index").is_none() {
                                let id = call.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                                if !id.is_empty()
                                    && tool_calls
                                        .get(idx)
                                        .is_some_and(|t| !t.id.is_empty() && !t.id.ends_with(id))
                                {
                                    idx = tool_calls.len();
                                }
                            }
                            if tool_calls.len() <= idx {
                                tool_calls.resize_with(idx + 1, AccumulatedToolCall::default);
                            }
                            // id and name may stream in pieces or repeat whole each chunk:
                            // `ends_with` takes either without doubling the repeat.
                            if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                                if !tool_calls[idx].id.ends_with(id) {
                                    tool_calls[idx].id.push_str(id);
                                }
                            }
                            if let Some(f) = call.get("function") {
                                if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                                    if !tool_calls[idx].name.ends_with(n) {
                                        tool_calls[idx].name.push_str(n);
                                    }
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
    let mut request_usage = None;
    if let Some((tokens, reported)) = usage {
        let cost = match reported {
            Some(usd) => Some(UsageCost::reported(usd)),
            None => trek_core::pricing::request(model, agent, &tokens, 0, false),
        };
        request_usage = Some((tokens, cost));
        events
            .send(AgentEvent::Usage {
                model: Some(model.to_string()),
                tokens,
                cost,
            })
            .await?;
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
        reasoning_details,
        usage: request_usage,
    })
}

/// Query live models from a direct provider (OpenRouter, DeepSeek, Ollama, etc.).
pub async fn direct_provider_models(
    provider_id: &str,
    api_key: Option<String>,
) -> Result<Vec<ModelInfo>> {
    let provider =
        direct_provider(provider_id).with_context(|| format!("unknown provider {provider_id}"))?;
    let key = api_key.or_else(|| secrets::api_key(provider.id));

    if !provider.local && provider.id != "openrouter" && key.is_none() {
        return Ok(vec![]);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

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
            .filter_map(|m| {
                m["name"].as_str().map(|name| ModelInfo {
                    id: name.to_string(),
                    name: name.to_string(),
                    efforts: vec![],
                    tier: 0,
                    fast: None,
                })
            })
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
    let file = trek_core::paths::data_dir()
        .join("direct-models")
        .join(format!("{provider_id}.json"));
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
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
        });
        url
    }

    #[test]
    fn a_429_is_a_limit_and_other_failures_are_not() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        let e = request_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            "429 Too Many Requests: Rate limit exceeded".into(),
        );
        let Some(crate::limits::LimitError(limit)) = e.downcast_ref() else {
            panic!("{e:#}")
        };
        assert!(
            limit
                .resets_at
                .is_some_and(|at| (at - trek_core::store::now_ms() - 30_000).abs() < 5_000)
        );
        assert_eq!(
            format!("{e:#}"),
            "429 Too Many Requests: Rate limit exceeded"
        );
        let e = request_error(
            reqwest::StatusCode::BAD_REQUEST,
            &headers,
            "400 Bad Request: no such model".into(),
        );
        assert!(e.downcast_ref::<crate::limits::LimitError>().is_none());
    }

    #[test]
    fn sse_splits_complete_events_with_either_line_ending() {
        let mut buf = b"data: {\"a\":1}\n\ndata: [DONE]\r\n\r\ndata: {\"b\"".to_vec();
        assert_eq!(
            sse_events(&mut buf),
            vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]
        );
        assert_eq!(buf, b"data: {\"b\"");
        buf.extend_from_slice(b":\"\xc3");
        assert!(sse_events(&mut buf).is_empty());
        buf.extend_from_slice(b"\xa9\"}\r\ndata: second line\r\n\r\n");
        assert_eq!(
            sse_events(&mut buf),
            vec!["{\"b\":\"\u{e9}\"}\nsecond line".to_string()]
        );
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
            Command::SetModel {
                model: "next".into(),
                effort: Effort::Medium,
            },
            Command::SetModes {
                plan: true,
                fast: Some("fast".into()),
                effort: Effort::High,
            },
            Command::Prompt {
                text: "steer".into(),
                images: vec![],
            },
        ] {
            keep_command(Ok(command), &mut kept, &mut shutdown).unwrap();
        }
        assert!(matches!(
            kept.pop_front(),
            Some(Command::SetHandHolding(trek_core::HandHolding::Supervised))
        ));
        assert!(
            matches!(kept.pop_front(), Some(Command::SetModel { model, .. }) if model == "next")
        );
        assert!(matches!(
            kept.pop_front(),
            Some(Command::SetModes {
                effort: Effort::High,
                ..
            })
        ));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));
        assert!(!shutdown && kept.is_empty());
    }

    #[tokio::test]
    async fn a_locked_key_is_explained_at_send_never_prompted_for() {
        // Saved by "another build" (isolated: the in-memory store).
        trek_core::keychain::plant_locked("groq", "gsk-from-before");
        let config = SessionConfig {
            agent: AgentId::Direct("groq".into()),
            cwd: std::env::temp_dir(),
            model: Some("m".into()),
            effort: Effort::Off,
            hand_holding: HandHolding::Auto,
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
            script: false,
        };
        let (_commands, commands_rx) = async_channel::unbounded();
        let (events_tx, _events) = async_channel::unbounded();
        let err = run(config, commands_rx, events_tx).await.unwrap_err();
        assert!(
            err.to_string().contains("locked") && err.to_string().contains("Settings → API Keys"),
            "{err}"
        );
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
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let (tx, rx) = async_channel::unbounded();
        tx.send(Command::SetModes {
            plan: true,
            fast: None,
            effort: Effort::High,
        })
        .await
        .unwrap();
        tx.send(Command::Prompt {
            text: "steer".into(),
            images: vec![],
        })
        .await
        .unwrap();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        send_request(
            reqwest::Client::new().get(&url),
            &rx,
            &mut kept,
            &mut shutdown,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            kept.pop_front(),
            Some(Command::SetModes {
                effort: Effort::High,
                ..
            })
        ));
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { text, .. }) if text == "steer"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let e = send_request(
            reqwest::Client::new().get(&url),
            &rx,
            &mut kept,
            &mut shutdown,
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("Timed out waiting for response headers"),
            "{e:#}"
        );
    }

    #[tokio::test]
    async fn openai_stream_requires_a_terminal_marker() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let complete = sse_server(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
        )
        .await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &complete,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "ok");

        let incomplete = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n").await;
        let e = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &incomplete,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("connection closed before the answer finished"),
            "{e:#}"
        );
    }

    #[tokio::test]
    async fn openai_stream_skips_empty_content_and_reads_reasoning() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"think\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\" more\"}],\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
        );
        let url = sse_server(body).await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &url,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "hi");
        assert_eq!(answer.reasoning_details.len(), 1);
        let (mut reasoning, mut text_deltas) = (String::new(), Vec::new());
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AgentEvent::ReasoningDelta(t) => reasoning.push_str(&t),
                AgentEvent::TextDelta(t) => text_deltas.push(t),
                _ => {}
            }
        }
        assert_eq!(reasoning, "think more");
        assert_eq!(text_deltas, ["hi"]);
    }

    #[tokio::test]
    async fn openai_stream_reads_content_parts_and_reports_stream_errors() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let parts = sse_server("data: {\"choices\":[{\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"arr\"},{\"type\":\"text\",\"text\":\"ay\"}]},\"finish_reason\":\"stop\"}]}\n\n").await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &parts,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "array");

        let failed = sse_server("data: {\"choices\":[{\"delta\":{\"content\":\"part\"}}]}\n\ndata: {\"error\":{\"message\":\"provider exploded\"}}\n\n").await;
        let e = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &failed,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("provider exploded"), "{e:#}");
    }

    #[tokio::test]
    async fn openai_stream_keeps_indexless_tool_calls_apart() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"a\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}},{\"id\":\"b\",\"function\":{\"name\":\"bash\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"bash\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        );
        let url = sse_server(body).await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &url,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.tool_calls.len(), 2);
        assert_eq!(answer.tool_calls[0].id, "a");
        assert_eq!(answer.tool_calls[0].name, "read_file");
        // The resent full id/name doesn't double; its arguments stay the first chunk's.
        assert_eq!(answer.tool_calls[1].id, "b");
        assert_eq!(answer.tool_calls[1].name, "bash");
        assert_eq!(answer.tool_calls[1].arguments, "{}");
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
        let answer = anthropic_turn(
            &client,
            &complete,
            "key",
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
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
        let temp_dir =
            std::env::temp_dir().join(format!("trek_test_direct_{}", trek_core::store::now_ms()));
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
        })
        .to_string();

        let (_out, ok, lines) = execute_tool(
            "write_file",
            &write_args,
            cwd,
            false,
            HandHolding::Auto,
            &mut allowed,
            &DirectOptions::default(),
            false,
            &tx_events,
            &rx_cmd,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert!(ok);
        assert_eq!(lines, Some((3, 0)));

        // 2. read_file with line range
        let read_args = json!({
            "path": "test.txt",
            "start_line": 2,
            "end_line": 3
        })
        .to_string();

        let (out, ok, _) = execute_tool(
            "read_file",
            &read_args,
            cwd,
            false,
            HandHolding::Auto,
            &mut allowed,
            &DirectOptions::default(),
            false,
            &tx_events,
            &rx_cmd,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert!(ok);
        assert!(out.contains("line 2"));
        assert!(!out.contains("line 1"));

        // 3. edit_file
        let edit_args = json!({
            "path": "test.txt",
            "old_str": "line 2",
            "new_str": "line two replacement"
        })
        .to_string();

        let (_out, ok, lines) = execute_tool(
            "edit_file",
            &edit_args,
            cwd,
            false,
            HandHolding::Auto,
            &mut allowed,
            &DirectOptions::default(),
            false,
            &tx_events,
            &rx_cmd,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
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
        assert!(is_safe_command("find . -name '*.rs' -type f"));
        assert!(is_safe_command("git branch -vv"));
        assert!(is_safe_command("git branch --show-current"));
        assert!(is_safe_command("echo $HOME"));
        assert!(is_safe_command("rg -n 'fn main' src"));
        // Commands the shell runs from inside a listed one.
        for unsafe_command in [
            "echo $(rm -rf ~)",
            "cat `curl -s https://example.com/x`",
            "ls .\nrm -rf ~",
            "ls .\r\nrm -rf ~",
            "cat <(rm -rf ~)",
            "echo $((1)) $(touch x)",
        ] {
            assert!(!is_safe_command(unsafe_command), "{unsafe_command:?}");
        }
        // Listed tools with flags that delete, write or run commands.
        for unsafe_command in [
            "find . -name '*.rs' -delete",
            "find . '-delete'",
            "find . -de''lete",
            "find . -exec rm -rf {} +",
            "find . -execdir rm {} ;",
            "find . -fprint out.txt",
            "fd -x rm",
            "fd -HIx rm",
            "fd --exec-batch rm",
            "git branch -D main",
            "git branch -d main",
            "git branch --delete main",
            "git branch -m main old",
            "git branch new-branch",
            "git branch -f main HEAD~3",
            "git diff --output=src/main.rs",
            "git log --output src/main.rs",
            "rg --pre ./script x",
            "rg --pre=sh x",
            "cargo clippy --fix",
        ] {
            assert!(!is_safe_command(unsafe_command), "{unsafe_command:?}");
        }
    }

    #[test]
    fn anthropic_requests_carry_the_tools() {
        let body = anthropic_body(
            "claude-sonnet-4-5",
            Effort::Medium,
            "sys",
            &[],
            &direct_tools(),
        );
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["bash", "read_file", "write_file", "edit_file"]);
        assert_eq!(
            body["tools"][1]["input_schema"]["required"],
            json!(["path"])
        );
        assert!(
            body["tools"][0].get("function").is_none() && body["tools"][0].get("type").is_none()
        );
    }

    #[tokio::test]
    async fn anthropic_tool_use_blocks_become_tool_calls() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let url = sse_server(concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Looking.\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \\\"src/\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"main.rs\\\"}\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ))
        .await;
        let answer = anthropic_turn(
            &client,
            &url,
            "key",
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "Looking.");
        assert_eq!(answer.tool_calls.len(), 1);
        assert_eq!(
            (
                answer.tool_calls[0].id.as_str(),
                answer.tool_calls[0].name.as_str()
            ),
            ("toolu_1", "read_file")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&answer.tool_calls[0].arguments).unwrap(),
            json!({ "path": "src/main.rs" })
        );
        let content = answer.content.unwrap();
        assert_eq!(
            content["content"][1]["input"],
            json!({ "path": "src/main.rs" })
        );
    }

    /// A server that answers request n with `responses[n]` (status, SSE body), one connection
    /// each, and keeps every request body it was sent.
    async fn script_server(
        responses: Vec<(u16, String)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = bodies.clone();
        tokio::spawn(async move {
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                let mut buf = [0u8; 4096];
                let body_start = loop {
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "the request ended early");
                    request.extend_from_slice(&buf[..n]);
                };
                let head = String::from_utf8_lossy(&request[..body_start]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse().unwrap());
                while request.len() < body_start + length {
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "the request body ended early");
                    request.extend_from_slice(&buf[..n]);
                }
                seen.lock().unwrap().push(
                    serde_json::from_slice(&request[body_start..body_start + length]).unwrap(),
                );
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            }
        });
        (url, bodies)
    }

    fn openai_text(text: &str) -> (u16, String) {
        (
            200,
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({ "choices": [{ "delta": { "content": text }, "finish_reason": "stop" }] })
            ),
        )
    }

    fn openai_tool_call(id: &str, name: &str, arguments: Value) -> (u16, String) {
        let call = json!({ "index": 0, "id": id, "type": "function", "function": { "name": name, "arguments": arguments.to_string() } });
        (
            200,
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({ "choices": [{ "delta": { "tool_calls": [call] }, "finish_reason": "tool_calls" }] })
            ),
        )
    }

    struct TestSession {
        commands: async_channel::Sender<Command>,
        events: async_channel::Receiver<AgentEvent>,
        bodies: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
        cwd: PathBuf,
        task: tokio::task::JoinHandle<Result<()>>,
    }

    impl TestSession {
        async fn start(
            provider: &'static str,
            hand_holding: HandHolding,
            responses: Vec<(u16, String)>,
        ) -> TestSession {
            Self::start_with(
                provider,
                hand_holding,
                responses,
                DirectOptions::default(),
                false,
            )
            .await
        }

        async fn start_with(
            provider: &'static str,
            hand_holding: HandHolding,
            responses: Vec<(u16, String)>,
            options: DirectOptions,
            script: bool,
        ) -> TestSession {
            let (url, bodies) = script_server(responses).await;
            let cwd = std::env::temp_dir().join(format!("trek_test_direct_session_{}", next_id()));
            std::fs::create_dir_all(&cwd).unwrap();
            let config = SessionConfig {
                agent: AgentId::Direct(provider.into()),
                cwd: cwd.clone(),
                model: Some("m".into()),
                effort: Effort::Off,
                hand_holding,
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
                script,
            };
            let (commands, commands_rx) = async_channel::unbounded();
            let (events_tx, events) = async_channel::unbounded();
            let provider = direct_provider(provider).unwrap();
            let task = tokio::spawn(async move {
                session(
                    config,
                    provider,
                    &url,
                    Some("key".into()),
                    options,
                    commands_rx,
                    events_tx,
                )
                .await
            });
            TestSession {
                commands,
                events,
                bodies,
                cwd,
                task,
            }
        }

        async fn send(&self, command: Command) {
            self.commands.send(command).await.unwrap();
        }

        async fn prompt(&self, text: &str) {
            self.send(Command::Prompt {
                text: text.into(),
                images: vec![],
            })
            .await;
        }

        /// The next event `pick` takes, failing on an error or the session's end on the way.
        async fn until<T>(&self, mut pick: impl FnMut(&AgentEvent) -> Option<T>) -> T {
            loop {
                let event = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
                    .await
                    .expect("an event in time")
                    .expect("the session is running");
                if let Some(found) = pick(&event) {
                    return found;
                }
                assert!(
                    !matches!(event, AgentEvent::Error(_) | AgentEvent::Exited),
                    "{event:?}"
                );
            }
        }

        async fn turn_complete(&self) -> Option<String> {
            self.until(|e| match e {
                AgentEvent::TurnComplete { error } => Some(error.clone()),
                _ => None,
            })
            .await
        }

        async fn tool_finished(&self) -> (String, bool) {
            self.until(|e| match e {
                AgentEvent::ToolFinished { output, ok, .. } => Some((output.clone(), *ok)),
                _ => None,
            })
            .await
        }

        fn body(&self, n: usize) -> Value {
            self.bodies.lock().unwrap()[n].clone()
        }

        async fn finish(self) {
            drop(self.commands);
            tokio::time::timeout(Duration::from_secs(10), self.task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let _ = std::fs::remove_dir_all(&self.cwd);
        }
    }

    /// Every OpenAI-wire assistant message with tool calls is followed by a reply to each call.
    fn assert_tool_calls_answered(messages: &[Value]) {
        for (i, m) in messages.iter().enumerate() {
            for call in m["tool_calls"].as_array().into_iter().flatten() {
                let id = &call["id"];
                let answered = messages[i + 1..]
                    .iter()
                    .take_while(|r| r["role"] == "tool")
                    .any(|r| &r["tool_call_id"] == id);
                assert!(answered, "call {id} has no reply in {messages:#?}");
            }
        }
    }

    fn user_texts(messages: &[Value]) -> Vec<&str> {
        messages
            .iter()
            .filter(|m| m["role"] == "user")
            .filter_map(|m| m["content"].as_str())
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_during_a_command_ends_the_turn_and_what_it_started() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call(
                    "call_1",
                    "bash",
                    json!({ "command": "sleep 30 & echo $! > bg.pid; wait" }),
                ),
                openai_text("next"),
            ],
        )
        .await;
        s.prompt("one").await;
        s.until(|e| matches!(e, AgentEvent::ToolStarted { .. }).then_some(()))
            .await;
        let pid_file = s.cwd.join("bg.pid");
        let started = std::time::Instant::now();
        let pid: i32 = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|p| p.trim().parse().ok())
            {
                break pid;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the command didn't start"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        s.send(Command::Interrupt).await;
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("Interrupted"), "{output}");
        assert_eq!(s.turn_complete().await.as_deref(), Some("Interrupted"));
        // The background `sleep` went with the shell. SAFETY: signal 0 only checks the pid.
        let started = std::time::Instant::now();
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the command's background process outlived the Stop"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // The session goes on, with the cut-short call answered.
        s.prompt("two").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        assert_eq!(user_texts(&messages), ["one", "two"]);
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_at_a_permission_card_ends_the_turn() {
        let s = TestSession::start(
            "ollama",
            HandHolding::Supervised,
            vec![
                openai_tool_call("call_1", "bash", json!({ "command": "touch made" })),
                openai_text("next"),
            ],
        )
        .await;
        s.prompt("one").await;
        s.until(|e| matches!(e, AgentEvent::PermissionRequest { .. }).then_some(()))
            .await;
        s.send(Command::Interrupt).await;
        let (_, ok) = s.tool_finished().await;
        assert!(!ok);
        assert_eq!(s.turn_complete().await.as_deref(), Some("Interrupted"));
        assert!(!s.cwd.join("made").exists());
        s.prompt("two").await;
        assert_eq!(s.turn_complete().await, None);
        assert_tool_calls_answered(s.body(1)["messages"].as_array().unwrap());
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_message_sent_while_a_command_runs_waits_for_it() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call(
                    "call_1",
                    "bash",
                    json!({ "command": "sleep 1; echo finished" }),
                ),
                openai_text("a"),
                openai_text("b"),
            ],
        )
        .await;
        s.prompt("one").await;
        s.until(|e| matches!(e, AgentEvent::ToolStarted { .. }).then_some(()))
            .await;
        s.send(Command::SetHandHolding(HandHolding::Supervised))
            .await;
        s.prompt("steer").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("finished"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(2)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        assert_eq!(user_texts(&messages), ["one", "steer"]);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_failed_request_after_a_tool_keeps_the_history_whole() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                (500, "boom".into()),
                openai_tool_call("call_1", "read_file", json!({ "path": "missing.txt" })),
                (500, "boom".into()),
                openai_text("ok"),
            ],
        )
        .await;
        // A first request that fails takes its message back out.
        s.prompt("zero").await;
        assert!(s.turn_complete().await.is_some_and(|e| e.contains("500")));
        // One that fails after a tool ran keeps the call and its result.
        s.prompt("one").await;
        assert!(s.turn_complete().await.is_some_and(|e| e.contains("500")));
        s.prompt("two").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(3)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        assert_eq!(user_texts(&messages), ["one", "two"]);
        assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 1);
        s.finish().await;
    }

    #[tokio::test]
    async fn anthropic_sessions_run_tools() {
        let tool_use = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"notes.txt\\\"}\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let text = concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let s = TestSession::start(
            "anthropic",
            HandHolding::FullAccess,
            vec![(200, tool_use.into()), (200, text.into())],
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("read it").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("hello from notes"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert!(s.body(0)["tools"].as_array().is_some_and(|t| t.len() == 4));
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert_eq!(
            messages[1]["content"][0]["input"],
            json!({ "path": "notes.txt" })
        );
        let result = &messages[2]["content"][0];
        assert_eq!(
            (
                messages[2]["role"].as_str(),
                result["type"].as_str(),
                result["tool_use_id"].as_str()
            ),
            (Some("user"), Some("tool_result"), Some("toolu_1"))
        );
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .contains("hello from notes")
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn thirty_tool_rounds_say_why_the_answer_stopped() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            (0..30)
                .map(|i| {
                    openai_tool_call(
                        &format!("call_{i}"),
                        "read_file",
                        json!({ "path": "notes.txt" }),
                    )
                })
                .collect(),
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("keep reading").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("30 tool-call rounds"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 30);
        let messages = s.body(29)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        s.finish().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_permission_card_waits_through_other_commands() {
        let (tx, commands) = async_channel::unbounded();
        let (events, rx) = async_channel::unbounded();
        let task = tokio::spawn(async move {
            let mut kept = VecDeque::from([Command::Prompt {
                text: "steer".into(),
                images: vec![],
            }]);
            let mut shutdown = false;
            let decision = ask_permission(
                "Run command",
                "ls",
                &events,
                &commands,
                &mut kept,
                &mut shutdown,
            )
            .await;
            (decision, kept)
        });
        let Ok(AgentEvent::PermissionRequest { request_id, .. }) = rx.recv().await else {
            panic!("no permission request")
        };
        tx.send(Command::SetHandHolding(HandHolding::Supervised))
            .await
            .unwrap();
        tx.send(Command::Respond {
            request_id,
            decision: Decision::AllowForSession,
        })
        .await
        .unwrap();
        let (decision, mut kept) = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the permission wait never ended")
            .unwrap();
        assert_eq!(decision.unwrap(), Decision::AllowForSession);
        assert!(matches!(
            rx.recv().await,
            Ok(AgentEvent::PermissionResolved { .. })
        ));
        // What was typed meanwhile is kept in order, behind what was already there.
        assert!(matches!(kept.pop_front(), Some(Command::Prompt { .. })));
        assert!(matches!(
            kept.pop_front(),
            Some(Command::SetHandHolding(HandHolding::Supervised))
        ));
        assert!(kept.is_empty());
    }

    #[tokio::test]
    async fn read_file_with_the_end_before_the_start_says_so() {
        let dir = std::env::temp_dir().join(format!("trek_read_range_{}", next_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lines: Vec<String> = (1..=100).map(|n| format!("line {n}")).collect();
        std::fs::write(dir.join("a.txt"), lines.join("\n")).unwrap();
        let (out, ok) = run_read_file(&LocalExecutor, "a.txt", Some(50), Some(10), &dir)
            .await
            .unwrap();
        assert!(!ok);
        assert!(out.contains("before start line 50"), "{out}");
        let (_, ok) = run_read_file(&LocalExecutor, "a.txt", Some(5), Some(0), &dir)
            .await
            .unwrap();
        assert!(!ok);
        let (out, ok) = run_read_file(&LocalExecutor, "a.txt", Some(10), Some(10), &dir)
            .await
            .unwrap();
        assert!(
            ok && out.contains("line 10") && !out.contains("line 11"),
            "{out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn long_command_output_is_cut_at_a_character() {
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        // One ASCII byte, then three-byte characters: byte 50000 is inside one.
        let (out, ok) = run_bash_command(
            &LocalExecutor,
            "printf a; yes '✓' | head -n 20000 | tr -d '\\n'",
            Path::new("."),
            None,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert!(ok);
        assert!(out.ends_with("\n... [output truncated]"));
        assert!(out.starts_with("a✓✓"));
        assert!(out.len() < 50_100);
    }

    #[test]
    fn edits_outside_the_project_ask_below_full_access() {
        let cwd = std::env::temp_dir().join("trek_edit_permission");
        let none = HashSet::new();
        for tool in ["write_file", "edit_file"] {
            // In the folder: only Supervised asks.
            for (h, asks) in [
                (HandHolding::Supervised, true),
                (HandHolding::AutoAcceptEdits, false),
                (HandHolding::Auto, false),
                (HandHolding::FullAccess, false),
            ] {
                assert_eq!(edit_permission(tool, h, "src/a.rs", &cwd, &none).0, asks);
            }
            // Outside it, by absolute path or by `..`: everything but Full access asks.
            for path in ["/etc/trek-test", "../../outside.txt"] {
                for (h, asks) in [
                    (HandHolding::Supervised, true),
                    (HandHolding::AutoAcceptEdits, true),
                    (HandHolding::Auto, true),
                    (HandHolding::FullAccess, false),
                ] {
                    assert_eq!(
                        edit_permission(tool, h, path, &cwd, &none).0,
                        asks,
                        "{path}"
                    );
                }
            }
        }
        // An allowance for the session covers that path, not the others.
        let (_, key) = edit_permission(
            "write_file",
            HandHolding::Auto,
            "/etc/trek-test",
            &cwd,
            &none,
        );
        let allowed = HashSet::from([key]);
        assert!(
            !edit_permission(
                "write_file",
                HandHolding::Auto,
                "/etc/trek-test",
                &cwd,
                &allowed
            )
            .0
        );
        assert!(
            edit_permission(
                "write_file",
                HandHolding::Auto,
                "/etc/other",
                &cwd,
                &allowed
            )
            .0
        );
        assert!(
            !edit_permission(
                "write_file",
                HandHolding::Supervised,
                "a.rs",
                &cwd,
                &HashSet::from(["write_file".to_string()])
            )
            .0
        );
    }

    #[tokio::test]
    async fn a_write_outside_the_project_waits_for_an_answer() {
        let dir = std::env::temp_dir().join(format!("trek_write_outside_{}", next_id()));
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let target = dir.join("outside.txt");
        let (events, rx) = async_channel::unbounded();
        let (tx, commands) = async_channel::unbounded();
        tx.send(Command::Interrupt).await.unwrap();
        let (mut kept, mut shutdown, mut allowed) = (VecDeque::new(), false, HashSet::new());
        let args = json!({ "path": target.to_str().unwrap(), "content": "x" }).to_string();
        let e = execute_tool(
            "write_file",
            &args,
            &project,
            false,
            HandHolding::AutoAcceptEdits,
            &mut allowed,
            &DirectOptions::default(),
            false,
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("Interrupted"), "{e:#}");
        assert!(matches!(
            rx.recv().await,
            Ok(AgentEvent::PermissionRequest { .. })
        ));
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn openai_stream_splits_indexless_tool_calls_sent_one_per_chunk() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"a\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"x\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"b\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        );
        let url = sse_server(body).await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &url,
            None,
            true,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.tool_calls.len(), 2);
        assert_eq!(answer.tool_calls[0].id, "a");
        assert_eq!(answer.tool_calls[0].name, "read_file");
        assert_eq!(answer.tool_calls[0].arguments, "{\"path\":\"x\"}");
        assert_eq!(answer.tool_calls[1].id, "b");
        assert_eq!(answer.tool_calls[1].name, "bash");
        assert_eq!(answer.tool_calls[1].arguments, "{\"command\":\"ls\"}");
    }

    /// Answers each request with the next of `replies` (status line, body), and gives back the
    /// request bodies it saw.
    async fn scripted_server(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                let (head_end, length) = loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    data.extend_from_slice(&buf[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..at]).to_lowercase();
                        let length = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (at + 4, length);
                    }
                    if n == 0 {
                        return;
                    }
                };
                while data.len() < head_end + length {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&data[head_end..]).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn a_model_that_rejects_reasoning_effort_is_asked_again_without_it() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let (url, seen) = scripted_server(vec![
            (
                "400 Bad Request",
                "{\"error\":{\"message\":\"Unrecognized request argument supplied: reasoning_effort\"}}",
            ),
            (
                "200 OK",
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            ),
        ])
        .await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("openai".into()),
            &url,
            Some("key"),
            false,
            false,
            "gpt-4.1",
            Effort::High,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "ok");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let first: Value = serde_json::from_str(&seen[0]).unwrap();
        let second: Value = serde_json::from_str(&seen[1]).unwrap();
        assert_eq!(first["reasoning_effort"], "high");
        assert!(second.get("reasoning_effort").is_none());
        // The tools stay: only the rejected field went.
        assert!(second.get("tools").is_some());
    }

    #[tokio::test]
    async fn a_400_about_something_else_still_retries_without_the_tools() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let (url, seen) = scripted_server(vec![
            ("400 Bad Request", "{\"error\":{\"message\":\"tools are not supported\"}}"),
            (
                "200 OK",
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            ),
        ])
        .await;
        let answer = openai_turn(
            &client,
            &AgentId::Direct("openai".into()),
            &url,
            None,
            false,
            false,
            "m",
            Effort::High,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert_eq!(answer.text, "ok");
        let seen = seen.lock().unwrap();
        let second: Value = serde_json::from_str(&seen[1]).unwrap();
        assert!(second.get("tools").is_none());
    }

    #[tokio::test]
    async fn a_failed_retry_reports_the_first_error() {
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let (url, _seen) = scripted_server(vec![
            (
                "400 Bad Request",
                "{\"error\":{\"message\":\"no such model\"}}",
            ),
            (
                "500 Internal Server Error",
                "{\"error\":{\"message\":\"boom\"}}",
            ),
        ])
        .await;
        let e = openai_turn(
            &client,
            &AgentId::Direct("openai".into()),
            &url,
            None,
            false,
            false,
            "m",
            Effort::Off,
            "sys",
            &[],
            &direct_tools(),
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("no such model"), "{e:#}");
    }

    /// Files in a map and commands written down: proves every tool goes through the executor.
    #[derive(Default)]
    struct MemoryExecutor {
        files: std::sync::Mutex<HashMap<PathBuf, Vec<u8>>>,
        ran: std::sync::Mutex<Vec<(String, PathBuf, Option<Duration>)>>,
    }

    impl ToolExecutor for MemoryExecutor {
        fn exec<'a>(
            &'a self,
            command: &'a str,
            cwd: &'a Path,
            timeout: Option<Duration>,
        ) -> futures::future::BoxFuture<'a, Result<executor::ExecOutput>> {
            Box::pin(async move {
                self.ran
                    .lock()
                    .unwrap()
                    .push((command.into(), cwd.into(), timeout));
                match command {
                    "gone" => Err(ExecutorGone("peer closed".into()).into()),
                    "broken" => Err(anyhow::anyhow!("no shell here")),
                    "slow" => Ok(executor::ExecOutput {
                        stdout: "partial".into(),
                        timed_out: true,
                        ..Default::default()
                    }),
                    _ => Ok(executor::ExecOutput {
                        stdout: format!("ran {command}"),
                        stderr: "note".into(),
                        exit_code: Some(0),
                        timed_out: false,
                    }),
                }
            })
        }

        fn read<'a>(
            &'a self,
            path: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<Option<Vec<u8>>>> {
            Box::pin(async move { Ok(self.files.lock().unwrap().get(path).cloned()) })
        }

        fn write<'a>(
            &'a self,
            path: &'a Path,
            bytes: &'a [u8],
        ) -> futures::future::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.files
                    .lock()
                    .unwrap()
                    .insert(path.into(), bytes.to_vec());
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn every_tool_goes_through_the_executor() {
        let memory = Arc::new(MemoryExecutor::default());
        let options = DirectOptions {
            executor: memory.clone(),
            command_timeout: Some(Duration::from_secs(7)),
            ..Default::default()
        };
        // A folder that doesn't exist here: the sandbox's.
        let cwd = Path::new("/sandbox/app");
        let (events, _rx) = async_channel::unbounded();
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown, mut allowed) = (VecDeque::new(), false, HashSet::new());
        let mut tool = async |name: &str, args: Value| {
            execute_tool(
                name,
                &args.to_string(),
                cwd,
                false,
                HandHolding::FullAccess,
                &mut allowed,
                &options,
                false,
                &events,
                &commands,
                &mut kept,
                &mut shutdown,
            )
            .await
        };

        let (out, ok, lines) = tool("read_file", json!({ "path": "a.txt" })).await.unwrap();
        assert_eq!((out.as_str(), ok), ("File not found: a.txt", false));
        assert_eq!(lines, None);
        let (_, ok, lines) = tool(
            "write_file",
            json!({ "path": "a.txt", "content": "one\ntwo\n" }),
        )
        .await
        .unwrap();
        assert!(ok);
        assert_eq!(lines, Some((2, 0)));
        let (out, ok, _) = tool("read_file", json!({ "path": "/sandbox/app/a.txt" }))
            .await
            .unwrap();
        assert!(ok && out.contains("   2 | two"), "{out}");
        let (_, ok, lines) = tool(
            "edit_file",
            json!({ "path": "a.txt", "old_str": "two", "new_str": "2\n3" }),
        )
        .await
        .unwrap();
        assert!(ok);
        assert_eq!(lines, Some((2, 1)));
        let (out, ok, _) = tool("bash", json!({ "command": "make" })).await.unwrap();
        assert_eq!((out.as_str(), ok), ("ran make\nnote", true));
        let (out, ok, _) = tool("bash", json!({ "command": "slow" })).await.unwrap();
        assert_eq!(
            (out.as_str(), ok),
            (
                "partial\n[Command timed out after 7 seconds and was stopped]",
                false
            )
        );
        let (out, ok, _) = tool("bash", json!({ "command": "broken" })).await.unwrap();
        assert_eq!(
            (out.as_str(), ok),
            ("Failed to execute command: no shell here", false)
        );
        // An executor that's gone ends the turn instead of telling the model.
        let e = tool("bash", json!({ "command": "gone" }))
            .await
            .unwrap_err();
        assert!(e.downcast_ref::<ExecutorGone>().is_some());
        // The script tool isn't offered without a host.
        let (out, ok, _) = tool("script", json!({ "code": "1" })).await.unwrap();
        assert!(!ok && out.starts_with("Unknown tool: script"), "{out}");

        assert_eq!(
            memory.files.lock().unwrap()[Path::new("/sandbox/app/a.txt")],
            b"one\n2\n3\n"
        );
        let ran = memory.ran.lock().unwrap();
        assert_eq!(ran[0].0, "make");
        assert_eq!(ran[0].1, cwd);
        assert_eq!(ran[0].2, Some(Duration::from_secs(7)));
    }

    /// Everything an `Observer` is told, one line each.
    #[derive(Default)]
    struct Log(std::sync::Mutex<Vec<String>>);

    impl Log {
        fn push(&self, line: String) {
            self.0.lock().unwrap().push(line);
        }

        fn lines(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Observer for Log {
        fn request_start(&self, n: u32) {
            self.push(format!("start {n}"));
        }
        fn request_end(
            &self,
            n: u32,
            usage: Option<&TokenUsage>,
            cost: Option<UsageCost>,
            _elapsed: Duration,
            error: Option<&str>,
        ) {
            let usage = usage.map(|u| (u.input, u.cache_read, u.output));
            let cost = cost.map(|c| (c.usd, c.reported));
            self.push(format!("end {n} {usage:?} {cost:?} {error:?}"));
        }
        fn text(&self, text: &str) {
            self.push(format!("text {text}"));
        }
        fn tool_call(&self, name: &str, args: &Value) {
            self.push(format!("call {name} {args}"));
        }
        fn tool_result(&self, name: &str, output: &str, ok: bool) {
            self.push(format!("result {name} {ok} {}", output.trim()));
        }
        fn script_result(&self, r: &trek_script::ScriptResult) {
            self.push(format!(
                "script {} {:?} {}",
                r.nested_calls, r.calls_by_tool, r.timed_out
            ));
        }
        fn stopped(&self, why: &Stop) {
            self.push(format!("stopped {why:?}"));
        }
    }

    /// `response` with a usage chunk (prompt 100, 40 of it cached, output 5) before its end, and
    /// OpenRouter's `cost` when given.
    fn with_usage((status, body): (u16, String), cost: Option<f64>) -> (u16, String) {
        let mut usage = json!({ "prompt_tokens": 100, "completion_tokens": 5, "prompt_tokens_details": { "cached_tokens": 40 } });
        if let Some(cost) = cost {
            usage["cost"] = json!(cost);
        }
        let chunk = format!("data: {}\n\n", json!({ "choices": [], "usage": usage }));
        (
            status,
            body.replace("data: [DONE]", &format!("{chunk}data: [DONE]")),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_observer_sees_each_request_and_tool_in_order() {
        let log = Arc::new(Log::default());
        let s = TestSession::start_with(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                with_usage(
                    openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                    Some(0.0012),
                ),
                with_usage(openai_text("all done"), None),
            ],
            DirectOptions {
                observer: Some(log.clone()),
                ..Default::default()
            },
            false,
        )
        .await;
        s.prompt("go").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(
            log.lines(),
            [
                "start 1",
                "end 1 Some((60, 40, 5)) Some((0.0012, true)) None",
                "call bash {\"command\":\"echo hi\"}",
                "result bash true hi",
                "start 2",
                "end 2 Some((60, 40, 5)) None None",
                "text all done",
                "stopped EndTurn",
            ]
        );
        assert_eq!(s.body(0)["tools"].as_array().unwrap().len(), 4);
        s.finish().await;
    }

    #[tokio::test]
    async fn the_turn_cap_and_the_deadline_end_an_answer() {
        let log = Arc::new(Log::default());
        let s = TestSession::start_with(
            "ollama",
            HandHolding::FullAccess,
            (0..2)
                .map(|i| {
                    openai_tool_call(
                        &format!("call_{i}"),
                        "read_file",
                        json!({ "path": "notes.txt" }),
                    )
                })
                .collect(),
            DirectOptions {
                observer: Some(log.clone()),
                max_turns: 2,
                ..Default::default()
            },
            false,
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello").unwrap();
        s.prompt("keep reading").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("2 tool-call rounds"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 2);
        assert_eq!(log.lines().last().unwrap(), "stopped MaxTurns");
        s.finish().await;

        // A deadline already past: no request at all.
        let log = Arc::new(Log::default());
        let s = TestSession::start_with(
            "ollama",
            HandHolding::FullAccess,
            vec![],
            DirectOptions {
                observer: Some(log.clone()),
                deadline: Some(Instant::now()),
                ..Default::default()
            },
            false,
        )
        .await;
        s.prompt("anything").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(log.lines(), ["stopped TimeLimit"]);
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_script_runs_its_calls_through_the_executor() {
        let log = Arc::new(Log::default());
        let code = r#"
await write("out/a.txt", "one\ntwo\n");
const text = await read("out/a.txt", {start: 2});
const {stdout, exit_code} = await bash("wc -l < out/a.txt");
print(`line 2: ${text}; lines: ${stdout.trim()}; exit ${exit_code}`);
"#;
        let s = TestSession::start_with(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call("call_1", "script", json!({ "code": code })),
                openai_text("done"),
            ],
            DirectOptions {
                observer: Some(log.clone()),
                system_prompt: Some("You are a benchmark agent.".into()),
                ..Default::default()
            },
            true,
        )
        .await;
        s.prompt("go").await;
        let (out, ok) = s.tool_finished().await;
        assert!(ok, "{out}");
        assert_eq!(out.trim(), "line 2: two; lines: 2; exit 0");
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(
            std::fs::read_to_string(s.cwd.join("out/a.txt")).unwrap(),
            "one\ntwo\n"
        );
        let lines = log.lines();
        assert!(
            lines.contains(&r#"script 3 {"bash": 1, "read": 1, "write": 1} false"#.to_string()),
            "{lines:#?}"
        );
        // The four tools stay, and the script comes after them.
        let tools = s.body(0)["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 5);
        assert_eq!(tools[4]["function"]["name"], "script");
        // The system prompt given replaces Trek's.
        assert_eq!(
            s.body(0)["messages"][0]["content"],
            "You are a benchmark agent."
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn a_scripts_writes_ask_like_the_models_own() {
        let s = TestSession::start_with(
            "ollama",
            HandHolding::Supervised,
            vec![
                openai_tool_call(
                    "call_1",
                    "script",
                    json!({ "code": "await write('x.txt', '1'); print('wrote')" }),
                ),
                openai_text("done"),
            ],
            DirectOptions::default(),
            true,
        )
        .await;
        s.prompt("go").await;
        let (request_id, title) = s
            .until(|e| match e {
                AgentEvent::PermissionRequest {
                    request_id, title, ..
                } => Some((request_id.clone(), title.clone())),
                _ => None,
            })
            .await;
        assert_eq!(title, "Write file");
        s.send(Command::Respond {
            request_id,
            decision: Decision::Deny,
        })
        .await;
        let (out, ok) = s.tool_finished().await;
        assert!(!ok, "{out}");
        assert!(out.contains("Permission denied by user"), "{out}");
        assert!(!out.contains("wrote"), "{out}");
        assert_eq!(s.turn_complete().await, None);
        assert!(!s.cwd.join("x.txt").exists());
        // On, the script is offered beside the four.
        assert_eq!(s.body(0)["tools"].as_array().unwrap().len(), 5);
        s.finish().await;
    }

    #[test]
    fn a_base_url_replaces_the_endpoint() {
        let openrouter = direct_provider("openrouter").unwrap();
        let anthropic = direct_provider("anthropic").unwrap();
        assert_eq!(endpoint(openrouter, None), "https://openrouter.ai/api/v1");
        assert_eq!(
            endpoint(openrouter, Some("http://localhost:4200/v1/")),
            "http://localhost:4200/v1"
        );
        assert_eq!(endpoint(anthropic, None), ANTHROPIC_MESSAGES_URL);
        for base in [
            "https://proxy.example",
            "https://proxy.example/v1",
            "https://proxy.example/v1/messages",
        ] {
            assert_eq!(
                endpoint(anthropic, Some(base)),
                "https://proxy.example/v1/messages"
            );
        }
    }
}
