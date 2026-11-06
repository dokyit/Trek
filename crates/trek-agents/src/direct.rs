//! Direct providers (API keys and local servers): Trek's own agent loop. A model request and what
//! comes back (`openai_turn`, `anthropic_turn`), its failures told apart (`failure`), tool calls
//! made sense of however a model sent them (`repair`), and the tools themselves with their
//! HandHolding permission gates (`execute_tool`, `edit`, `search`, `web`, `executor`).

pub mod edit;
pub mod executor;
pub mod failure;
pub mod mcp;
pub mod repair;
pub mod script;
pub mod search;
pub mod web;

use crate::rules::Ruling;
use crate::{AgentEvent, Billing, Command, Decision, SessionConfig, load_image};
use anyhow::{Context as _, Result, bail};
use executor::{ExecutorGone, LocalExecutor, ToolExecutor};
use failure::{FailureKind, RequestFailure};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_core::catalog::{DirectProvider, ModelCaps, ModelInfo, Wire, direct_provider};
use trek_core::permission_rules::{self, Call, PermissionRule, RuleKind};
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
    /// How long one `bash` command may run unless the model says otherwise (`None`: as long as it
    /// takes).
    pub command_timeout: Option<Duration>,
    /// No new model request, and no further tool call, after this: the one running finishes.
    pub deadline: Option<Instant>,
    /// The whole system prompt, in place of Trek's (`SessionConfig::instructions` still follow).
    pub system_prompt: Option<String>,
    pub observer: Option<Arc<dyn Observer>>,
    /// The model's context window in tokens, in place of what its provider's model list said
    /// (`ModelInfo::context`) or `catalog::context_window`'s guess.
    pub context_window: Option<u64>,
    /// Anthropic's message threads (the conversation held on the server between requests, so
    /// each request carries only what's new): `None` on Anthropic's own endpoint only,
    /// `Some(true)` on this endpoint too, `Some(false)` never.
    pub message_threads: Option<bool>,
}

impl Default for DirectOptions {
    fn default() -> Self {
        DirectOptions {
            api_key: None,
            provider: None,
            base_url: None,
            executor: Arc::new(LocalExecutor),
            script_log_dir: None,
            // The real guards are `LoopGuard` and the report at the end: 30 cut off five of six real
            // sub-agents mid-work, with nothing to show for it.
            max_turns: 200,
            command_timeout: Some(Duration::from_secs(300)),
            deadline: None,
            system_prompt: None,
            observer: None,
            context_window: None,
            message_threads: None,
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
    /// The model kept making the same calls and getting the same results.
    Stuck,
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

/// The error for a failed request, by what can be done about it (`failure`): one worth waiting out
/// is sent again by the session, a usage limit with a far-off reset is said with
/// `AgentEvent::LimitReached`.
fn request_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    message: String,
) -> anyhow::Error {
    let mut failed =
        failure::classify_status(status.as_u16(), &message, failure::retry_after(headers));
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let now = trek_core::store::now_ms();
        let limit = crate::limits::from_response(headers, message, now);
        let far = limit
            .resets_at
            .is_some_and(|at| at.saturating_sub(now) > failure::MAX_RETRY_AFTER.as_millis() as i64);
        if far || failed.kind == FailureKind::Fatal {
            return crate::limits::LimitError(limit).into();
        }
        failed.limit = Some(limit);
    }
    failed.into()
}

/// Trek's prompt: short, because a weak model does worse with a long one and a strong one doesn't
/// need it. What moves results is in the tools and their errors; this says how to use them.
/// `environment` and `project` are read once, when the session starts, so the prefix stays the same
/// for the provider's cache.
fn system_prompt(
    cwd: &Path,
    read_only: bool,
    environment: &str,
    project: Option<&str>,
    instructions: Option<&str>,
) -> String {
    let mut p = format!(
        "You are Trek, an autonomous coding agent working in `{}`.\n\n\
         Use your tools to do the work rather than describing it: read what you need, make the change, run it, read the result. Answer a plain question plainly.\n\
         - Find things with grep, glob and list_dir rather than guessing at paths.\n\
         - Read a file before editing it. Keep edits small and in the file's own style.\n\
         - Each command runs in a new shell in this folder. A failed one ends with `[exit code N]`. When something fails, work out why before trying again, and don't repeat a call that didn't work.\n\
         - Long output is cut, with the whole of it saved to a file you can read or grep.\n\
         - Keep going until the request is done or you're truly blocked, then say briefly what you did.\n",
        cwd.display()
    );
    if read_only {
        p.push_str("\nThis session is read-only: look and report, but change nothing. bash runs only commands that just read (git status, git log, git diff, ls, cat, grep and the like).\n");
    }
    if !environment.is_empty() {
        p.push('\n');
        p.push_str(environment);
        p.push('\n');
    }
    if let Some(project) = project {
        p.push('\n');
        p.push_str(project);
        p.push('\n');
    }
    if let Some(extra) = instructions {
        p.push('\n');
        p.push_str(extra);
        p.push('\n');
    }
    p
}

/// The facts a model can't work out without spending a call on each: the date, the OS, the shell
/// it's writing commands for, the branch.
async fn environment(exec: &dyn ToolExecutor, cwd: &Path) -> String {
    let limit = Some(Duration::from_secs(3));
    let os = if exec.is_local() {
        match std::env::consts::OS {
            "macos" => "macOS".to_string(),
            "windows" => "Windows".to_string(),
            "linux" => "Linux".to_string(),
            other => other.to_string(),
        }
    } else {
        exec.exec("uname -s", cwd, limit)
            .await
            .ok()
            .filter(|r| r.success())
            .map(|r| r.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Linux".to_string())
    };
    let shell = if exec.is_local() && cfg!(windows) {
        "cmd.exe (bash syntax won't work)"
    } else {
        "bash"
    };
    let mut out = format!(
        "Today's date: {}\nOS: {os}\nShell: {shell}",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let branch = exec
        .exec("git rev-parse --abbrev-ref HEAD", cwd, limit)
        .await
        .ok()
        .filter(|r| r.success())
        .map(|r| r.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(branch) = branch {
        out.push_str(&format!("\nGit branch: {branch}"));
    }
    out
}

/// The project's own instructions for agents: `AGENTS.md`, else `CLAUDE.md`, in the folder and in
/// each one up to the repository's root, outermost first.
async fn project_instructions(exec: &dyn ToolExecutor, cwd: &Path) -> Option<String> {
    const EACH: usize = 16_000;
    let root = exec
        .exec(
            "git rev-parse --show-toplevel",
            cwd,
            Some(Duration::from_secs(3)),
        )
        .await
        .ok()
        .filter(|r| r.success())
        .map(|r| PathBuf::from(r.stdout.trim()));
    let mut dirs = vec![cwd.to_path_buf()];
    if let Some(root) = &root {
        let mut upward = Vec::new();
        let mut at = Some(cwd);
        let mut reached = false;
        while let Some(dir) = at {
            upward.push(dir.to_path_buf());
            if dir == root {
                reached = true;
                break;
            }
            at = dir.parent();
        }
        if reached {
            dirs = upward;
            dirs.reverse();
        }
    }
    let mut found = Vec::new();
    for dir in &dirs {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = dir.join(name);
            let Ok(Some(bytes)) = exec.read(&path).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            if text.trim().is_empty() {
                continue;
            }
            found.push(format!(
                "Project instructions from {}:\n{}",
                path.display(),
                clip_chars(text.trim(), EACH)
            ));
            break;
        }
    }
    (!found.is_empty()).then(|| found.join("\n\n"))
}

static ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_id() -> String {
    let c = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}_{c}", trek_core::store::now_ms())
}

/// The tools a session offers: what a read-only session can't use isn't on offer, and `web_fetch`
/// is only for sessions on this machine (a benchmark's sandbox has no business on the web through
/// a side door). Then `script`, when it's on (see `script`).
fn tool_defs(script: bool, local: bool, read_only: bool) -> Vec<Value> {
    let mut tools: Vec<Value> = direct_tools()
        .into_iter()
        .filter(|t| {
            let name = t["function"]["name"].as_str().unwrap_or_default();
            match name {
                "write_file" | "edit_file" => !read_only,
                "web_fetch" => local,
                _ => true,
            }
        })
        .collect();
    if script && !read_only {
        tools.push(script::tool_def());
    }
    tools
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required
            }
        }
    })
}

fn direct_tools() -> Vec<Value> {
    vec![
        tool(
            "bash",
            "Run a shell command in the project folder and get its output. Each call is a new shell: cd doesn't carry over. A failed command ends with [exit code N]. Commands stop after 300 seconds unless timeout_s says otherwise.",
            json!({
                "command": { "type": "string", "description": "The command line to run." },
                "timeout_s": { "type": "integer", "description": "Seconds it may run, up to 3600. Set it for long builds and test runs." },
                "background": { "type": "boolean", "description": "Start it and return at once, with where its output goes. For servers and watchers." }
            }),
            &["command"],
        ),
        tool(
            "read_file",
            "Read a text file, with line numbers. Up to 2000 lines at a time: the end says how to read on.",
            json!({
                "path": { "type": "string", "description": "The file, relative to the project folder or absolute." },
                "start_line": { "type": "integer", "description": "First line to read, from 1." },
                "end_line": { "type": "integer", "description": "Last line to read." }
            }),
            &["path"],
        ),
        tool(
            "write_file",
            "Create a file, or replace all of one. For a change to an existing file, use edit_file.",
            json!({
                "path": { "type": "string", "description": "The file to write." },
                "content": { "type": "string", "description": "All of its new text." }
            }),
            &["path", "content"],
        ),
        tool(
            "edit_file",
            "Replace text in a file. old_str must match one place, so include enough lines to pick it out. The result shows the lines around the change.",
            json!({
                "path": { "type": "string", "description": "The file to edit." },
                "old_str": { "type": "string", "description": "The text to replace, copied from the file without line numbers." },
                "new_str": { "type": "string", "description": "What to put there." },
                "replace_all": { "type": "boolean", "description": "Replace every place old_str matches." }
            }),
            &["path", "old_str", "new_str"],
        ),
        tool(
            "grep",
            "Search file contents with a regular expression. Returns path:line:text, skipping what git ignores.",
            json!({
                "pattern": { "type": "string", "description": "The regular expression." },
                "path": { "type": "string", "description": "Folder or file to search. Default: the project folder." },
                "glob": { "type": "string", "description": "Only files matching this, such as *.rs." },
                "case_insensitive": { "type": "boolean" }
            }),
            &["pattern"],
        ),
        tool(
            "glob",
            "Find files by name, most recently changed first, skipping what git ignores.",
            json!({
                "pattern": { "type": "string", "description": "A pattern such as *.rs or src/**/*.ts." },
                "path": { "type": "string", "description": "Folder to search. Default: the project folder." }
            }),
            &["pattern"],
        ),
        tool(
            "list_dir",
            "List what's in a folder, folders first.",
            json!({
                "path": { "type": "string", "description": "The folder. Default: the project folder." }
            }),
            &[],
        ),
        tool(
            "web_fetch",
            "Get a web page or text file as text, for documentation and the like.",
            json!({
                "url": { "type": "string", "description": "The http or https address." }
            }),
            &["url"],
        ),
    ]
}

/// The two tools every MCP server's tools are reached through, so dozens of schemas don't ride
/// along on every request.
fn mcp_tools() -> Vec<Value> {
    vec![
        tool(
            "mcp_list",
            "List the tools on the MCP servers, or with server set, what each of that server's tools does and takes.",
            json!({
                "server": { "type": "string", "description": "A server's name. Leave out to list them all." }
            }),
            &[],
        ),
        tool(
            "mcp_call",
            "Run a tool on an MCP server.",
            json!({
                "server": { "type": "string", "description": "The server's name." },
                "tool": { "type": "string", "description": "The tool's name." },
                "arguments": { "type": "object", "description": "What the tool takes." }
            }),
            &["server", "tool"],
        ),
    ]
}

/// The thread's transcript, for a session that has earlier context to look back on: one that
/// started from a recap, or went on in a fresh context (`Trail::orientation`). They read the
/// log Trek keeps (`trek_core::session_log`).
fn transcript_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "transcript_search",
                "description": "Search this thread's whole transcript (earlier messages, tool calls, full results) for lines matching query. Returns the latest matches with line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Words all on one line (any case), or a regex with regex true." },
                        "regex": { "type": "boolean" },
                        "context": { "type": "integer", "description": "Lines around each match (default 2)." }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "transcript_read",
                "description": "Read transcript lines by number.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "start": { "type": "integer" },
                        "end": { "type": "integer" }
                    },
                    "required": ["start"]
                }
            }
        }),
    ]
}

/// A transcript tool's call, run on the log at `path`: its output, and whether it worked.
async fn transcript_tool(name: &str, args_json: &str, path: &Path) -> (String, bool) {
    let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    let path = path.to_path_buf();
    let name = name.to_string();
    let ran = tokio::task::spawn_blocking(move || match name.as_str() {
        "transcript_search" => trek_core::session_log::Query::from_args(&args)
            .and_then(|q| trek_core::session_log::search(&path, &q)),
        _ => {
            let n = |k: &str| args[k].as_u64().map(|v| v as usize);
            trek_core::session_log::read(&path, n("start").unwrap_or(1), n("end"))
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match ran {
        Ok(out) => (out, true),
        Err(e) => (e, false),
    }
}

/// Past this share of the model's context window, the next request starts a fresh context.
const CONTINUE_AT: f64 = 0.85;

/// What a fresh context's first user message says when it starts mid-answer.
const CONTINUE_MESSAGE: &str = "Your context was full, so it was started fresh: the system prompt says where things stand. Carry on with the current request from there.";

/// Roughly how many tokens `text` and `values` take together: a quarter of their characters, an
/// image's data counted as a picture's worth (about 1,500 tokens) rather than as text. An empty
/// `text` counts nothing. They're read where they are: the context check runs before every
/// request, and copying the history and the tools into one `Value` only to count them was work
/// thrown away.
fn estimate_tokens_of(text: &str, values: &[Value]) -> u64 {
    ((text_chars(text) + values.iter().map(estimate_chars).sum::<usize>()) / 4) as u64
}

fn text_chars(s: &str) -> usize {
    if s.len() > 6_000 && (s.starts_with("data:image") || !s.contains(' ')) {
        6_000
    } else {
        s.len()
    }
}

fn estimate_chars(v: &Value) -> usize {
    match v {
        Value::String(s) => text_chars(s),
        Value::Array(a) => a.iter().map(estimate_chars).sum(),
        Value::Object(o) => o.iter().map(|(k, v)| k.len() + estimate_chars(v)).sum(),
        other => other.to_string().len(),
    }
}

/// What a session has been doing, to tell a fresh context where things stand.
#[derive(Debug, Default)]
struct Trail {
    /// The session's first message (past any recap) and the one being answered.
    first: Option<String>,
    prompt: String,
    /// The latest tool calls in this answer: the call, whether it worked, its output.
    steps: VecDeque<(String, bool, String)>,
    /// The model's latest words.
    said: String,
    /// How many times the session started a fresh context.
    fresh: u32,
}

impl Trail {
    const STEPS: usize = 10;

    fn prompt(&mut self, text: &str) {
        let text = text.rsplit("</recap>").next().unwrap_or(text).trim();
        if self.first.is_none() {
            self.first = Some(text.to_string());
        }
        self.prompt = text.to_string();
        self.steps.clear();
    }

    /// A message that reached the model part way through an answer.
    fn steer(&mut self, text: &str) {
        self.prompt
            .push_str(&format!("\n\nThe user then added:\n{}", text.trim()));
    }

    fn step(&mut self, name: &str, args: &str, ok: bool, output: &str) {
        let (_, detail) = tool_display(name, args);
        self.steps.push_back((
            format!(
                "{name} {}",
                clip_chars(detail.lines().next().unwrap_or_default(), 160)
            ),
            ok,
            output.to_string(),
        ));
        while self.steps.len() > Self::STEPS {
            self.steps.pop_front();
        }
    }

    /// The orientation a fresh context gets in its system prompt: where the transcript is (and
    /// how to search it), the session's first message, the current request, the model's own notes
    /// when it managed to write them, what was done lately and its last words.
    fn orientation(&self, transcript: Option<&Path>, notes: Option<&str>) -> String {
        let mut out = format!(
            "<fresh-context>\nYour context window filled up, so this session went on in a fresh one{}. ",
            if self.fresh > 1 {
                format!(" ({} times now)", self.fresh)
            } else {
                String::new()
            }
        );
        match transcript {
            Some(path) => {
                let lines = std::fs::read(path)
                    .map(|b| b.iter().filter(|c| **c == b'\n').count() as u64)
                    .unwrap_or(0);
                out.push_str(&trek_core::session_log::retrieval(path, lines, None));
            }
            None => out.push_str(
                "Nothing from before is kept where you can search it: work from this, the files and your tools.",
            ),
        }
        let first = self.first.as_deref().unwrap_or_default();
        if !first.is_empty() && first != self.prompt {
            out.push_str(&format!(
                "\n\nThe session's first message:\n{}",
                clip_chars(first, 1_500)
            ));
        }
        if !self.prompt.is_empty() {
            out.push_str(&format!(
                "\n\nThe request you're answering:\n{}",
                // Whole, or near enough: a brief cut off half way sends the work off in the wrong
                // direction, and real ones run to a few thousand characters.
                clip_chars(&self.prompt, 12_000)
            ));
        }
        if let Some(notes) = notes {
            out.push_str(&format!(
                "\n\nThe notes you wrote for yourself just now:\n{notes}"
            ));
        }
        if !self.steps.is_empty() {
            // Cut short without a word, an output reads as all there was: a model once took a
            // file's first line for the file and re-read everything.
            out.push_str(
                "\n\nYour latest tool calls in it, oldest first, with the start of each output only (one ending in … went on: what isn't shown may still be in it):",
            );
            let n = self.steps.len();
            for (i, (call, ok, output)) in self.steps.iter().enumerate() {
                let shown = if i + 2 >= n {
                    clip_chars(output.trim(), 1_200)
                } else {
                    clip_chars(output.trim().lines().next().unwrap_or_default(), 160)
                };
                out.push_str(&format!(
                    "\n- {call} ({}): {shown}",
                    if *ok { "ok" } else { "failed" }
                ));
            }
        }
        if !self.said.trim().is_empty() {
            out.push_str(&format!(
                "\n\nYour last words:\n{}",
                clip_chars(self.said.trim(), 1_000)
            ));
        }
        out.push_str("\n</fresh-context>");
        out
    }
}

/// `s` cut to about `max` bytes (at a character boundary), with " …" when it was cut.
fn clip_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &s[..end])
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
    /// Reasoning as plain text, and the field it came in, for a provider that sends no details
    /// and wants it back on a tool call (DeepSeek's `reasoning_content`).
    pub reasoning: Option<(&'static str, String)>,
    /// Why the answer ended, as the provider said (`stop`, `tool_calls`, `length`). `length` is
    /// the same for Anthropic's `max_tokens`.
    pub finish: Option<String>,
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

/// What Trek knows about the model in use: its window and what it can take, from the provider's
/// model list (kept on disk by the app, fetched here when a session starts without one, as
/// `trek-bench` does), else the catalog's guess by its name. Asked before every request, so the
/// answer is kept until the model changes or the list is fetched again.
#[derive(Default)]
struct ModelFacts {
    known: Option<(
        String,
        Option<std::time::SystemTime>,
        u64,
        Option<ModelCaps>,
    )>,
    fetched: Vec<ModelInfo>,
}

impl ModelFacts {
    fn of(
        &mut self,
        provider_id: &str,
        window: Option<u64>,
        model: &str,
    ) -> (u64, Option<ModelCaps>) {
        let stamp = cached_direct_models_stamp(provider_id);
        if let Some((m, s, w, caps)) = &self.known {
            if m == model && *s == stamp {
                return (*w, caps.clone());
            }
        }
        let listed = if self.fetched.is_empty() {
            cached_direct_models(provider_id)
                .into_iter()
                .find(|m| m.id == model)
        } else {
            self.fetched.iter().find(|m| m.id == model).cloned()
        };
        let context = listed.as_ref().and_then(|m| m.context);
        let caps = listed.and_then(|m| m.caps);
        let window = window
            .or(context)
            .unwrap_or_else(|| trek_core::catalog::context_window(model));
        self.known = Some((model.to_string(), stamp, window, caps.clone()));
        (window, caps)
    }
}

/// What goes into one model request.
struct TurnRequest<'a> {
    client: &'a reqwest::Client,
    agent: &'a AgentId,
    provider: &'a DirectProvider,
    url: &'a str,
    key: Option<&'a str>,
    model: &'a str,
    effort: Effort,
    system: &'a str,
    history: &'a [Value],
    tools: &'a [Value],
    /// An answer in words only: the model is told it can't call anything (the tools stay in the
    /// request: providers refuse a history of tool calls with none declared).
    no_tools: bool,
    /// Most tokens the answer may use, when known.
    max_output: Option<u64>,
    caps: Option<&'a ModelCaps>,
    session_id: &'a str,
}

/// Sends `req`. Text it had streamed before failing is left in `partial`.
#[allow(clippy::too_many_arguments)]
async fn send_turn(
    req: &TurnRequest<'_>,
    thread: &mut Thread,
    quirks: &mut Quirks,
    partial: &mut String,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    match req.provider.wire {
        Wire::Anthropic => {
            anthropic_turn(req, thread, partial, events, commands, kept, shutdown).await
        }
        _ => openai_turn(req, quirks, partial, events, commands, kept, shutdown).await,
    }
}

/// Past this share of the window, old tool output is cut down to make room (`elide_old_outputs`).
const ELIDE_AT: f64 = 0.6;
/// What's kept whole by that: the latest outputs, up to this share of the window.
const ELIDE_KEEPS: f64 = 0.3;
/// A context smaller than this isn't worth a request to summarize.
const SUMMARY_MIN_WINDOW: u64 = 32_000;
/// How long a run of the same answer may be cut by the output limit before giving up on it.
const MAX_CUT_OFFS: u32 = 3;
/// How many empty answers in a row are asked for again.
const MAX_EMPTY: u32 = 2;

const SUMMARY_PROMPT: &str = "Your context is nearly full and is about to be cleared. Write the notes you'll need to carry on without it. Don't call any tools. Cover the goal and any constraints the user gave; what is done, and which files changed; what you found out that the files won't show; what you were in the middle of and the exact next steps. Be specific: paths, names, commands, error messages. At most 400 words.";
const CUT_OFF_PROMPT: &str = "Your answer was cut off by the output limit. Continue from exactly where it stopped, without repeating anything.";
const NUDGE_PROMPT: &str = "You ended your turn without calling a tool, but your last words say there's more to do. If there is, call the tool now. If the work is finished, say so in a sentence.";
const REPORT_PROMPT: &str = "You've reached the limit of tool rounds for this answer. Don't call any more tools. Say briefly what you finished, what's left, and anything the user should know.";
const STUCK_PROMPT: &str = "You've been repeating the same calls with the same results, so the tools are stopped for this answer. Don't call any more. Say briefly what you tried, what's blocking you, and what you'd need to get past it.";

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
    let local = options.executor.is_local();
    let mut tools = tool_defs(config.script, local, config.read_only);
    let hub = mcp::McpHub::new(config.mcp_servers.clone());
    if !hub.is_empty() {
        tools.extend(mcp_tools());
    }
    // A session with earlier context to look back on searches the thread's transcript.
    let mut searchable = false;
    if config.recap.is_some() && config.transcript.is_some() {
        tools.extend(transcript_tools());
        searchable = true;
    }
    let mut requests = 0u32;
    let mut model = config.model.clone().context("Pick a model first")?;
    let mut effort = config.effort;
    let mut hand_holding = config.hand_holding;
    let mut allowed_for_session = HashSet::new();
    let client = reqwest::Client::new();
    // Keeps one conversation on one upstream where the router can: its prompt cache is there.
    let session_id = format!("trek-{}", next_id());
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

    let mut facts = ModelFacts::default();
    // No list on disk (`trek-bench`, on a machine that never ran the app): OpenRouter says for free.
    if provider.id == "openrouter"
        && url == provider.base_url
        && !trek_core::paths::isolated()
        && cached_direct_models(provider.id).is_empty()
    {
        facts.fetched = direct_provider_models(provider.id, key.clone())
            .await
            .unwrap_or_default();
        cache_direct_models(provider.id, &facts.fetched);
    }

    let base_system = match &options.system_prompt {
        Some(own) => {
            let mut p = own.clone();
            if let Some(extra) = &config.instructions {
                p.push_str("\n\n");
                p.push_str(extra);
                p.push('\n');
            }
            p
        }
        None => {
            let exec = options.executor.as_ref();
            let (environment, project) = tokio::join!(
                environment(exec, &config.cwd),
                project_instructions(exec, &config.cwd)
            );
            let mut prompt = system_prompt(
                &config.cwd,
                config.read_only,
                &environment,
                project.as_deref(),
                config.instructions.as_deref(),
            );
            if !hub.is_empty() {
                prompt.push_str(&format!(
                    "\nMore tools are on these MCP servers: {}. mcp_list says what each has, and mcp_call runs one.\n",
                    hub.names().join(", ")
                ));
            }
            prompt
        }
    };
    let mut system = base_system.clone();
    let mut history: Vec<Value> = Vec::new();
    let mut thread = Thread {
        off: match options.message_threads {
            Some(on) => !on,
            None => url != ANTHROPIC_MESSAGES_URL,
        } || std::env::var_os("TREK_NO_MESSAGE_THREADS").is_some(),
        ..Thread::default()
    };
    let mut quirks = Quirks::default();
    let mut quirks_model = model.clone();
    let mut kept = VecDeque::new();
    let mut shutdown = false;
    // Process groups of commands the model left running: ended with the session.
    let mut background = BackgroundGroups {
        groups: Vec::new(),
        local,
    };
    // How full the context is: tokens in it at the last usage report, and `history`'s length
    // then (what came after is estimated).
    let mut used: Option<(u64, usize)> = None;
    let mut trail = Trail::default();

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
                    let (_, caps) = facts.of(provider.id, options.context_window, &model);
                    // Asked to compact: the model's notes and a fresh context, with no message of
                    // the user's to answer after.
                    let compacting = text.trim() == "/compact" && images.is_empty();
                    if compacting && history.is_empty() {
                        events
                            .send(AgentEvent::Notice(
                                "Nothing to compact yet: send a message first.".to_string(),
                            ))
                            .await?;
                        observe(&|o| o.stopped(&Stop::EndTurn));
                        continue;
                    }
                    // Where this message starts in `history`: a request refused outright takes it
                    // back out, so the next message doesn't carry one that can never be sent.
                    let mut turn_start = history.len();
                    if !compacting {
                        let (message, skipped) =
                            user_message(provider.wire, &text, &images, caps.as_ref());
                        for e in skipped {
                            events
                                .send(AgentEvent::Notice(format!("Image left out: {e}")))
                                .await?;
                        }
                        history.push(message);
                        trail.prompt(&text);
                    }

                    // Multi-turn agent loop for tool calls
                    let mut turn_count = 0;
                    let mut turn_error = None;
                    let mut why = None;
                    let mut guard = LoopGuard::default();
                    let mut empty_answers = 0u32;
                    let mut cut_offs = 0u32;
                    let mut nudged = false;
                    let mut overflowed = false;
                    let mut force_fresh = compacting;
                    let mut refused_history = false;
                    let mut whole_output = false;
                    while turn_count < options.max_turns {
                        if past_deadline() {
                            why = Some(Stop::TimeLimit);
                            break;
                        }
                        // What one model turned out not to take says nothing about the next.
                        if quirks_model != model {
                            quirks.forget_model();
                            quirks_model.clone_from(&model);
                        }
                        let (window, caps) = facts.of(provider.id, options.context_window, &model);
                        let estimate_now =
                            |history: &[Value], tools: &[Value], system: &str, used| match used {
                                Some((tokens, at)) if at <= history.len() => {
                                    tokens + estimate_tokens_of("", &history[at..])
                                }
                                _ => {
                                    estimate_tokens_of(system, tools)
                                        + estimate_tokens_of("", history)
                                }
                            };
                        let mut estimate = estimate_now(&history, &tools, &system, used);
                        // First make room cheaply, by cutting down old tool output.
                        if estimate as f64 > ELIDE_AT * window as f64
                            && elide_old_outputs(&mut history, window, &model)
                        {
                            used = None;
                            estimate = estimate_now(&history, &tools, &system, used);
                        }
                        // Near the context window's end: go on in a fresh context, oriented by the
                        // model's own notes and the trail, with the transcript to search for
                        // anything exact.
                        // Only when going on afresh frees room: a fresh context still holds the
                        // system prompt, the tools and an orientation (about 2,000 tokens).
                        let fresh_floor = estimate_tokens_of(&base_system, &tools) + 2_000;
                        if force_fresh
                            || (history.len() > 1
                                && estimate as f64 > CONTINUE_AT * window as f64
                                && fresh_floor < window / 2)
                        {
                            let mut notes = None;
                            let mut notes_failed = None;
                            if (compacting || (!force_fresh && window >= SUMMARY_MIN_WINDOW))
                                && estimate < window
                                && !past_deadline()
                            {
                                let mut asked = history.clone();
                                asked.push(json!({ "role": "user", "content": SUMMARY_PROMPT }));
                                requests += 1;
                                let n = requests;
                                observe(&|o| o.request_start(n));
                                let sent = Instant::now();
                                // Not for the screen: it's the model talking to itself.
                                let (sink, _keep) = async_channel::unbounded();
                                let mut own_thread = Thread {
                                    off: true,
                                    ..Thread::default()
                                };
                                let mut partial = String::new();
                                let answer = send_turn(
                                    &TurnRequest {
                                        client: &client,
                                        agent: &config.agent,
                                        provider,
                                        url,
                                        key: key.as_deref(),
                                        model: &model,
                                        effort: Effort::Off,
                                        system: &system,
                                        history: &asked,
                                        tools: &tools,
                                        no_tools: true,
                                        max_output: Some(4_000),
                                        caps: caps.as_ref(),
                                        session_id: &session_id,
                                    },
                                    &mut own_thread,
                                    &mut quirks,
                                    &mut partial,
                                    &sink,
                                    &commands,
                                    &mut kept,
                                    &mut shutdown,
                                )
                                .await;
                                match &answer {
                                    Ok(resp) => {
                                        let (tokens, cost) = match &resp.usage {
                                            Some((t, c)) => (Some(t), *c),
                                            None => (None, None),
                                        };
                                        observe(&|o| {
                                            o.request_end(n, tokens, cost, sent.elapsed(), None)
                                        });
                                        if let Some((tokens, cost)) = resp.usage {
                                            events
                                                .send(AgentEvent::Usage {
                                                    model: Some(model.clone()),
                                                    tokens,
                                                    cost,
                                                })
                                                .await?;
                                        }
                                        notes = Some(resp.text.trim().to_string())
                                            .filter(|t| !t.is_empty());
                                        if notes.is_none() {
                                            notes_failed =
                                                Some("the model wrote no notes".to_string());
                                        }
                                    }
                                    Err(e) => {
                                        let message = format!("{e:#}");
                                        observe(&|o| {
                                            o.request_end(
                                                n,
                                                None,
                                                None,
                                                sent.elapsed(),
                                                Some(&message),
                                            )
                                        });
                                        // A Stop while it wrote its notes is a Stop.
                                        if is_stop(e) {
                                            turn_error.get_or_insert(message);
                                            break;
                                        }
                                        notes_failed = Some(message);
                                    }
                                }
                                if shutdown {
                                    break;
                                }
                            }
                            // Asked to compact and the notes didn't come: the context stays as it
                            // was, rather than going with nothing to carry on from.
                            if compacting {
                                if let Some(why) = notes_failed.take().or_else(|| {
                                    (estimate >= window || past_deadline())
                                        .then(|| "the context is too full to summarize".to_string())
                                }) {
                                    turn_error.get_or_insert(format!(
                                        "Couldn't compact: {why}. The conversation is as it was."
                                    ));
                                    break;
                                }
                            }
                            trail.fresh += 1;
                            if compacting {
                                // There's no request in progress: what comes next is new.
                                trail.prompt.clear();
                                trail.steps.clear();
                            }
                            system = format!(
                                "{base_system}\n\n{}",
                                trail.orientation(config.transcript.as_deref(), notes.as_deref())
                            );
                            // At the start of an answer the message itself carries on; mid-answer,
                            // a line saying to.
                            let carried = (history.len() == turn_start + 1)
                                .then(|| history.pop())
                                .flatten();
                            history.clear();
                            if !compacting {
                                history.push(carried.unwrap_or_else(
                                    || json!({ "role": "user", "content": CONTINUE_MESSAGE }),
                                ));
                            }
                            turn_start = 0;
                            if !searchable && config.transcript.is_some() {
                                tools.extend(transcript_tools());
                                searchable = true;
                            }
                            used = None;
                            force_fresh = false;
                            if compacting {
                                events
                                    .send(AgentEvent::Context {
                                        used: estimate_tokens_of(&system, &tools),
                                        window,
                                    })
                                    .await?;
                                why = Some(Stop::EndTurn);
                                break;
                            }
                            let why_fresh = if refused_history {
                                "The provider refused the conversation as it stood".to_string()
                            } else {
                                format!(
                                    "The context was {}% full",
                                    (estimate * 100 / window.max(1)).min(100)
                                )
                            };
                            refused_history = false;
                            events
                                .send(AgentEvent::Notice(format!(
                                    "{why_fresh}, so this answer goes on in a fresh one{}.",
                                    if config.transcript.is_some() {
                                        ": the earlier conversation stays searchable in the transcript"
                                    } else {
                                        ""
                                    }
                                )))
                                .await?;
                            estimate = estimate_now(&history, &tools, &system, used);
                        }
                        turn_count += 1;
                        let max_output =
                            output_limit(provider, caps.as_ref(), window, estimate, whole_output);
                        let (mut attempt, mut limit_seen) = (0u32, None);
                        let mut partial = String::new();
                        let result = loop {
                            attempt += 1;
                            requests += 1;
                            let n = requests;
                            observe(&|o| o.request_start(n));
                            let sent = Instant::now();
                            partial.clear();
                            let result = send_turn(
                                &TurnRequest {
                                    client: &client,
                                    agent: &config.agent,
                                    provider,
                                    url,
                                    key: key.as_deref(),
                                    model: &model,
                                    effort,
                                    system: &system,
                                    history: &history,
                                    tools: &tools,
                                    no_tools: false,
                                    max_output,
                                    caps: caps.as_ref(),
                                    session_id: &session_id,
                                },
                                &mut thread,
                                &mut quirks,
                                &mut partial,
                                &events,
                                &commands,
                                &mut kept,
                                &mut shutdown,
                            )
                            .await;
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
                                    let message = format!("{e:#}");
                                    observe(&|o| {
                                        o.request_end(n, None, None, elapsed, Some(&message))
                                    });
                                }
                            }
                            let Err(e) = &result else { break result };
                            let Some(failed) = e.downcast_ref::<RequestFailure>() else {
                                break result;
                            };
                            if failed.limit.is_some() {
                                limit_seen = failed.limit.clone();
                            }
                            if !failed.is_retryable()
                                || attempt >= failure::MAX_ATTEMPTS
                                || past_deadline()
                            {
                                break result;
                            }
                            let wait = failure::backoff(attempt, failed.retry_after);
                            // A blip passes unmentioned; past that, say what's going on.
                            if attempt >= 2 {
                                events
                                    .send(AgentEvent::Notice(format!(
                                        "{}. Trying again in {} seconds (attempt {} of {}).",
                                        clip_chars(failed.message.trim_end_matches('.'), 200),
                                        wait.as_secs().max(1),
                                        attempt + 1,
                                        failure::MAX_ATTEMPTS
                                    )))
                                    .await?;
                            }
                            let wait = if cfg!(test) { wait / 500 } else { wait };
                            if let Err(stopped) = until_stopped(
                                tokio::time::sleep(wait),
                                &commands,
                                &mut kept,
                                &mut shutdown,
                            )
                            .await
                            {
                                break Err(stopped);
                            }
                        };
                        match result {
                            Ok(mut resp) => {
                                let anthropic = resp.content.is_some();
                                let cut_off = resp.finish.as_deref() == Some("length");
                                let in_context = resp.usage.as_ref().map(|(t, _)| {
                                    t.input + t.cache_read + t.cache_write + t.output
                                });
                                if let Some(tokens) = in_context {
                                    events
                                        .send(AgentEvent::Context {
                                            used: tokens,
                                            window,
                                        })
                                        .await?;
                                }
                                // Nothing said and nothing called: the model or its route had a
                                // bad moment. It's asked again; to the model an empty message in
                                // its history means nothing, and some providers refuse one.
                                if resp.text.trim().is_empty() && resp.tool_calls.is_empty() {
                                    whole_output |= cut_off;
                                    empty_answers += 1;
                                    if empty_answers <= MAX_EMPTY {
                                        continue;
                                    }
                                    let message = if cut_off {
                                        "The model used its whole output limit on reasoning and wrote nothing. Try a lower effort."
                                    } else {
                                        "The model returned an empty answer."
                                    };
                                    turn_error.get_or_insert_with(|| message.to_string());
                                    break;
                                }
                                empty_answers = 0;
                                if !resp.text.trim().is_empty() {
                                    trail.said = resp.text.clone();
                                }
                                // A server's tool called by its own name, as a note may have said to.
                                if !hub.is_empty() {
                                    if let Err(stopped) = until_stopped(
                                        route_to_servers(&mut resp.tool_calls, &tools, &hub),
                                        &commands,
                                        &mut kept,
                                        &mut shutdown,
                                    )
                                    .await
                                    {
                                        turn_error.get_or_insert_with(|| format!("{stopped:#}"));
                                        break;
                                    }
                                }
                                let problems = prepare_calls(&mut resp.tool_calls, &tools, cut_off);
                                match resp.content {
                                    Some(content) => history.push(content),
                                    None if resp.tool_calls.is_empty() => history
                                        .push(json!({ "role": "assistant", "content": resp.text })),
                                    None => history.push(assistant_tool_message(&resp)),
                                }
                                if let Some(tokens) = in_context {
                                    used = Some((tokens, history.len()));
                                }
                                cut_offs = if cut_off { cut_offs + 1 } else { 0 };
                                if cut_offs > MAX_CUT_OFFS {
                                    // The answer's calls are in history, and providers refuse a
                                    // call left unanswered.
                                    let results = resp
                                        .tool_calls
                                        .iter()
                                        .map(|c| {
                                            (
                                                c.id.clone(),
                                                "Not run: the answer kept hitting its output limit"
                                                    .to_string(),
                                                false,
                                            )
                                        })
                                        .collect();
                                    push_tool_results(&mut history, anthropic, results);
                                    turn_error.get_or_insert_with(|| {
                                        "The model keeps hitting its output limit, so the answer stopped here.".to_string()
                                    });
                                    break;
                                }
                                if resp.tool_calls.is_empty() {
                                    if cut_off {
                                        history.push(
                                            json!({ "role": "user", "content": CUT_OFF_PROMPT }),
                                        );
                                        continue;
                                    }
                                    // Weaker models announce the next step and stop. Once per
                                    // answer, and only part way through work.
                                    if !nudged && turn_count > 1 && announces_more(&resp.text) {
                                        nudged = true;
                                        history.push(
                                            json!({ "role": "user", "content": NUDGE_PROMPT }),
                                        );
                                        continue;
                                    }
                                    why = Some(Stop::EndTurn);
                                    break;
                                }

                                // Every call gets a result, even one a Stop cut short or never
                                // started: providers reject a history with a call left unanswered.
                                let mut results = Vec::new();
                                let mut stopped = None;
                                let mut timed_out = false;
                                let mut stuck = false;
                                for (i, call) in resp.tool_calls.iter().enumerate() {
                                    // An access level changed while the model was writing, or
                                    // while the last tool ran, holds from this call on.
                                    apply_settings(
                                        &mut kept,
                                        &mut model,
                                        &mut effort,
                                        &mut hand_holding,
                                        &mut history,
                                    );
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

                                    let ran = match (&problems[i], config.transcript.as_deref()) {
                                        // A call that can't be run as it stands: told why, and the
                                        // model calls it again.
                                        (Some(problem), _) => Ok((problem.clone(), false, None)),
                                        // Reading the thread's own transcript asks nothing.
                                        (None, Some(path))
                                            if searchable
                                                && (call.name == "transcript_search"
                                                    || call.name == "transcript_read") =>
                                        {
                                            let (out, ok) =
                                                transcript_tool(&call.name, &call.arguments, path)
                                                    .await;
                                            Ok((out, ok, None))
                                        }
                                        _ => {
                                            execute_tool(
                                                &call.name,
                                                &call.arguments,
                                                &config.cwd,
                                                config.read_only,
                                                hand_holding,
                                                &mut allowed_for_session,
                                                &options,
                                                config.script,
                                                &hub,
                                                &mut background.groups,
                                                &events,
                                                &commands,
                                                &mut kept,
                                                &mut shutdown,
                                            )
                                            .await
                                        }
                                    };
                                    let (mut output, ok, lines, stop) = match ran {
                                        Ok((output, ok, lines)) => (output, ok, lines, None),
                                        // A Stop (or shutdown) while the tool ran or waited for
                                        // permission: it ends this turn, not the session.
                                        Err(e) => (format!("{e:#}"), false, None, Some(e)),
                                    };
                                    observe(&|o| o.tool_result(&call.name, &output, ok));
                                    trail.step(&call.name, &call.arguments, ok, &output);

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
                                    // The same call with the same result, again and again: said to
                                    // the model first, then stopped.
                                    let repeats =
                                        guard.record(&call.name, &call.arguments, &output);
                                    if repeats >= LoopGuard::STOP_AT {
                                        stuck = true;
                                    } else if repeats >= LoopGuard::WARN_AT {
                                        output.push_str(&format!(
                                            "\n\n[You've made this same call {repeats} times and got the same result each time. It won't change by itself: try something different, or say what's blocking you.]"
                                        ));
                                    }
                                    let declined = output.starts_with("Permission denied by user");
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
                                    // The user declined this call and said something: what they said
                                    // reaches the model before anything else of this answer runs.
                                    // The app sends the "no" and then the words: they may still be on
                                    // their way.
                                    let mut stop_now = false;
                                    if declined {
                                        while let Ok(cmd) = commands.try_recv() {
                                            match cmd {
                                                Command::Interrupt => {
                                                    stop_now = true;
                                                    break;
                                                }
                                                Command::Shutdown => {
                                                    shutdown = true;
                                                    stop_now = true;
                                                    break;
                                                }
                                                other => kept.push_back(other),
                                            }
                                        }
                                    }
                                    if stop_now {
                                        for rest in &resp.tool_calls[i + 1..] {
                                            results.push((
                                                rest.id.clone(),
                                                "Not run: the turn was stopped".to_string(),
                                                false,
                                            ));
                                        }
                                        stopped = Some(anyhow::anyhow!("Interrupted"));
                                        break;
                                    }
                                    let replied = declined
                                        && kept.iter().any(|c| matches!(c, Command::Prompt { .. }));
                                    if replied && i + 1 < resp.tool_calls.len() {
                                        for rest in &resp.tool_calls[i + 1..] {
                                            results.push((
                                                rest.id.clone(),
                                                "Not run: the user declined the call before it and replied".to_string(),
                                                false,
                                            ));
                                        }
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
                                // Pictures a tool took (a browser's or a phone's screen).
                                let images = hub.take_images();
                                if !images.is_empty() {
                                    if sees_images(provider, caps.as_ref()) {
                                        let (usable, left_out) = usable_images(images);
                                        forget_old_tool_images(&mut history);
                                        history.push(tool_images_message(
                                            provider.wire,
                                            usable,
                                            left_out,
                                        ));
                                    } else {
                                        history.push(json!({
                                            "role": "user",
                                            "content": "A tool returned an image, but this model can't see images."
                                        }));
                                    }
                                }
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
                                if stuck {
                                    why = Some(Stop::Stuck);
                                    break;
                                }
                                // A message sent while the tools ran reaches the model now, not
                                // after it has finished all it planned.
                                apply_settings(
                                    &mut kept,
                                    &mut model,
                                    &mut effort,
                                    &mut hand_holding,
                                    &mut history,
                                );
                                while let Some(pos) = kept
                                    .iter()
                                    .position(|c| matches!(c, Command::Prompt { .. }))
                                {
                                    let Some(Command::Prompt { text, images }) = kept.remove(pos)
                                    else {
                                        continue;
                                    };
                                    let (message, skipped) =
                                        user_message(provider.wire, &text, &images, caps.as_ref());
                                    for e in skipped {
                                        events
                                            .send(AgentEvent::Notice(format!(
                                                "Image left out: {e}"
                                            )))
                                            .await?;
                                    }
                                    history.push(message);
                                    trail.steer(&text);
                                }
                            }
                            Err(e) => {
                                let interrupted = e.downcast_ref::<RequestFailure>().is_none();
                                let failed = e.downcast_ref::<RequestFailure>().cloned();
                                if let Some(failed) = &failed {
                                    // Too big for the window: make room and send it again, once.
                                    if failed.kind == FailureKind::ContextOverflow
                                        && !overflowed
                                        && history.len() > 1
                                    {
                                        overflowed = true;
                                        force_fresh = true;
                                        turn_count -= 1;
                                        continue;
                                    }
                                    // Something in the conversation the provider will never
                                    // accept: every later request would fail on it. Once, as for
                                    // an overflow, and the transcript still has it all.
                                    if failed.kind == FailureKind::Refused
                                        && !overflowed
                                        && history.len() > 1
                                        && failure::blames_history(&failed.message)
                                    {
                                        overflowed = true;
                                        force_fresh = true;
                                        refused_history = true;
                                        turn_count -= 1;
                                        continue;
                                    }
                                }
                                // Only if the limit is why this ended: not a Stop while waiting one
                                // out, and not a different failure after it.
                                let ended_by_limit =
                                    failed.as_ref().is_some_and(|f| f.is_retryable());
                                if let Some(limit) = limit_seen.filter(|_| ended_by_limit) {
                                    events.send(limit.event()).await?;
                                } else if let Some(crate::limits::LimitError(limit)) =
                                    e.downcast_ref()
                                {
                                    events.send(limit.clone().event()).await?;
                                }
                                // What the provider will refuse every time (too big, not allowed)
                                // comes back out, or every later request would fail on it too. One
                                // that failed for the moment stays, for "continue" to go on from.
                                let refused = failed.as_ref().is_some_and(|f| !f.is_retryable())
                                    || e.downcast_ref::<crate::limits::LimitError>().is_none()
                                        && failed.is_none()
                                        && !is_stop(&e);
                                if refused && history.len() == turn_start + 1 {
                                    history.pop();
                                }
                                // Words already on the screen when a Stop came stay in the
                                // conversation: the next message may well be about them.
                                if interrupted && is_stop(&e) && !partial.trim().is_empty() {
                                    history.push(
                                        json!({ "role": "assistant", "content": partial.clone() }),
                                    );
                                }
                                turn_error.get_or_insert_with(|| format!("{e:#}"));
                                break;
                            }
                        }
                    }
                    if why.is_none()
                        && turn_error.is_none()
                        && !shutdown
                        && turn_count >= options.max_turns
                    {
                        // The runaway guard; say so, or the answer looks like it just stopped
                        // mid-work.
                        events.send(AgentEvent::Notice(format!(
                            "Stopped at {} tool-call rounds in one answer. Send a message and it picks up where it left off.",
                            options.max_turns
                        ))).await?;
                        why = Some(Stop::MaxTurns);
                    }
                    // Ended by a guard rather than by the model: it still gets to say where
                    // things stand, or whoever asked gets no report at all.
                    let report = match why {
                        Some(Stop::MaxTurns) => Some(REPORT_PROMPT),
                        Some(Stop::Stuck) => Some(STUCK_PROMPT),
                        _ => None,
                    };
                    if let Some(prompt) = report.filter(|_| !past_deadline()) {
                        if why == Some(Stop::Stuck) {
                            events.send(AgentEvent::Notice(
                                "Stopped: the model kept making the same calls and getting the same results.".to_string(),
                            )).await?;
                        }
                        let (window, caps) = facts.of(provider.id, options.context_window, &model);
                        history.push(json!({ "role": "user", "content": prompt }));
                        let estimate = match used {
                            Some((tokens, at)) if at <= history.len() => {
                                tokens + estimate_tokens_of("", &history[at..])
                            }
                            _ => {
                                estimate_tokens_of(&system, &tools)
                                    + estimate_tokens_of("", &history)
                            }
                        };
                        requests += 1;
                        let n = requests;
                        observe(&|o| o.request_start(n));
                        let sent = Instant::now();
                        let mut own_thread = Thread {
                            off: true,
                            ..Thread::default()
                        };
                        let mut partial = String::new();
                        let answer = send_turn(
                            &TurnRequest {
                                client: &client,
                                agent: &config.agent,
                                provider,
                                url,
                                key: key.as_deref(),
                                model: &model,
                                effort,
                                system: &system,
                                history: &history,
                                tools: &tools,
                                no_tools: true,
                                max_output: output_limit(
                                    provider,
                                    caps.as_ref(),
                                    window,
                                    estimate,
                                    false,
                                ),
                                caps: caps.as_ref(),
                                session_id: &session_id,
                            },
                            &mut own_thread,
                            &mut quirks,
                            &mut partial,
                            &events,
                            &commands,
                            &mut kept,
                            &mut shutdown,
                        )
                        .await;
                        match answer {
                            Ok(resp) if !resp.text.trim().is_empty() => {
                                let (tokens, cost) = match &resp.usage {
                                    Some((t, c)) => (Some(t), *c),
                                    None => (None, None),
                                };
                                observe(&|o| o.request_end(n, tokens, cost, sent.elapsed(), None));
                                observe(&|o| o.text(&resp.text));
                                history.push(json!({ "role": "assistant", "content": resp.text }));
                            }
                            Ok(_) => {
                                observe(&|o| o.request_end(n, None, None, sent.elapsed(), None));
                                history.pop();
                            }
                            Err(e) => {
                                let e = format!("{e:#}");
                                observe(&|o| {
                                    o.request_end(n, None, None, sent.elapsed(), Some(&e))
                                });
                                history.pop();
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
                                if m != model {
                                    strip_reasoning(&mut history);
                                }
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
                if m != model {
                    strip_reasoning(&mut history);
                }
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
    // Servers and watchers the model started end with the session rather than outliving it. Here
    // only for a sandbox, which has no other way in; on this machine `BackgroundGroups` does it,
    // however the session ends.
    if !local && !background.groups.is_empty() {
        let groups: Vec<String> = background.groups.iter().map(|g| format!("-{g}")).collect();
        let _ = options
            .executor
            .exec(
                &format!("kill -- {} 2>/dev/null", groups.join(" ")),
                &config.cwd,
                Some(Duration::from_secs(5)),
            )
            .await;
    }
    Ok(())
}

/// Process groups a session's commands left running. Dropped, it ends those on this machine: a
/// session can end by a panic or by the app going away mid-turn as well as by being told to.
struct BackgroundGroups {
    groups: Vec<i32>,
    local: bool,
}

impl Drop for BackgroundGroups {
    fn drop(&mut self) {
        if !self.local {
            return;
        }
        for group in &self.groups {
            #[cfg(unix)]
            crate::signal_group(*group, libc::SIGTERM);
            trek_core::procs::unregister(*group);
        }
    }
}

/// Whether `e` is a Stop (or shutdown) rather than a failure.
fn is_stop(e: &anyhow::Error) -> bool {
    e.to_string() == "Interrupted"
}

/// Settings that arrived while an answer was going (a model, an effort, an access level) applied
/// now. What else is in `kept` stays there, in order.
fn apply_settings(
    kept: &mut VecDeque<Command>,
    model: &mut String,
    effort: &mut Effort,
    hand_holding: &mut HandHolding,
    history: &mut [Value],
) {
    let mut others = VecDeque::with_capacity(kept.len());
    while let Some(cmd) = kept.pop_front() {
        match cmd {
            Command::SetModel {
                model: m,
                effort: e,
            } => {
                if m != *model {
                    strip_reasoning(history);
                }
                *model = m;
                *effort = e;
            }
            Command::SetModes { effort: e, .. } => *effort = e,
            Command::SetHandHolding(h) => *hand_holding = h,
            other => others.push_back(other),
        }
    }
    *kept = others;
}

/// Reasoning is a model's own: another model can't read it, and providers refuse some of what they
/// didn't sign.
fn strip_reasoning(history: &mut [Value]) {
    for message in history {
        if let Some(fields) = message.as_object_mut() {
            fields.remove("reasoning_details");
            fields.remove("reasoning");
            fields.remove("reasoning_content");
        }
        if let Some(blocks) = message["content"].as_array_mut() {
            blocks.retain(|b| b["type"] != "thinking" && b["type"] != "redacted_thinking");
        }
    }
}

/// `resp`'s assistant message on the OpenAI wire: `content` null without text, the calls as they
/// were made sense of, and the reasoning the way it came (`reasoning_details` whole, and the plain
/// field it arrived in), since some models refuse a history that has lost it.
fn assistant_tool_message(resp: &TurnResponse) -> Value {
    let tool_calls: Vec<Value> = resp
        .tool_calls
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments }
            })
        })
        .collect();
    let mut message = json!({
        "role": "assistant",
        "content": if resp.text.trim().is_empty() { Value::Null } else { Value::String(resp.text.clone()) },
        "tool_calls": tool_calls
    });
    if !resp.reasoning_details.is_empty() {
        message["reasoning_details"] = json!(resp.reasoning_details);
    }
    // OpenRouter sends both forms of the same text; the details are what it wants back.
    if let (true, Some((field, text))) = (resp.reasoning_details.is_empty(), &resp.reasoning) {
        message[*field] = json!(text);
    }
    message
}

/// Calls made ready to run: names repaired, arguments parsed and fitted to the tool's schema and
/// written back as valid JSON (history must never echo what a provider would refuse to parse).
/// Each gets `None`, or what to tell the model instead of running it.
fn prepare_calls(
    calls: &mut [AccumulatedToolCall],
    tools: &[Value],
    cut_off: bool,
) -> Vec<Option<String>> {
    let known: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    calls
        .iter_mut()
        .map(|call| {
            let raw = std::mem::take(&mut call.arguments);
            let Some(name) = repair::repair_tool_name(&call.name, &known) else {
                call.arguments = match repair::parse_args(&raw).value() {
                    Some(v) => v.to_string(),
                    None => "{}".to_string(),
                };
                let problem = if call.name.trim().is_empty() {
                    format!(
                        "Error: this tool call has no name. Available tools: {}",
                        known.join(", ")
                    )
                } else {
                    format!(
                        "Error: tool '{}' does not exist. Available tools: {}",
                        call.name,
                        known.join(", ")
                    )
                };
                // The name goes back in history, and providers refuse one outside these characters.
                let safe: String = call
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .take(64)
                    .collect();
                call.name = if safe.is_empty() {
                    "unknown_tool".to_string()
                } else {
                    safe
                };
                return Some(problem);
            };
            call.name = name;
            let parsed = repair::parse_args(&raw);
            let args = match parsed {
                // Hitting the limit, anything repaired was finished by a guess.
                repair::Args::Repaired(_) if cut_off => None,
                repair::Args::Clean(v) | repair::Args::Repaired(v) => Some(v),
                repair::Args::Truncated | repair::Args::Invalid(_) => None,
            };
            let Some(mut args) = args else {
                call.arguments = "{}".to_string();
                return Some(match repair::parse_args(&raw) {
                    repair::Args::Invalid(why) if !cut_off => {
                        repair::invalid_args_message(&call.name, &raw, &why)
                    }
                    _ => repair::truncated_args_message(&call.name),
                });
            };
            let schema = tools
                .iter()
                .find(|t| t["function"]["name"] == call.name.as_str())
                .map(|t| &t["function"]["parameters"]);
            let mut problem = None;
            if let Some(schema) = schema {
                repair::coerce_args(&call.name, &mut args, schema);
                let missing = repair::missing_required(&args, schema);
                if !missing.is_empty() {
                    problem = Some(format!(
                        "Error: this call to {} was not run because it's missing {}. Call it again with {}.",
                        call.name,
                        missing.join(" and "),
                        if missing.len() == 1 { "it" } else { "them" }
                    ));
                }
            }
            call.arguments = args.to_string();
            problem
        })
        .collect()
}

/// Calls to a tool a session's servers have, by that tool's own name (Trek's notes tell agents to
/// call `watch_video`), turned into the `mcp_call` that runs it. A server's own name comes before
/// any guess at what a name was meant to be. Arguments that can't be read are left for
/// `prepare_calls` to report: wrapping them would turn half a call into a clean one.
async fn route_to_servers(calls: &mut [AccumulatedToolCall], tools: &[Value], hub: &mcp::McpHub) {
    let known: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    for call in calls {
        if known.contains(&call.name.as_str()) {
            continue;
        }
        let Some(arguments) = repair::parse_args(&call.arguments).value().cloned() else {
            continue;
        };
        let Some(server) = hub.find(&call.name).await else {
            continue;
        };
        call.arguments =
            json!({ "server": server, "tool": call.name, "arguments": arguments }).to_string();
        call.name = "mcp_call".to_string();
    }
}

/// Whether to send this model pictures: where its list says it can see them, or a vendor whose
/// models can. Anything else would be refused outright.
fn sees_images(provider: &DirectProvider, caps: Option<&ModelCaps>) -> bool {
    match caps.and_then(|c| c.images) {
        Some(known) => known,
        None => matches!(provider.id, "anthropic" | "openai" | "google"),
    }
}

const TOOL_IMAGES_NOTE: &str = "The images the tool calls above returned:";
/// Providers refuse an image over about 5 MB, which is this much base64.
const MAX_TOOL_IMAGE: usize = 6_500_000;
/// Most images kept from one round of calls.
const MAX_TOOL_IMAGES: usize = 4;

/// The images a provider will take, in a kind it reads and a size it allows, and how many are left
/// out. Sent anyway, one refused image would fail every later request too.
fn usable_images(images: Vec<mcp::ToolImage>) -> (Vec<mcp::ToolImage>, usize) {
    let total = images.len();
    let usable: Vec<mcp::ToolImage> = images
        .into_iter()
        .filter(|(kind, data)| {
            matches!(
                kind.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) && data.len() <= MAX_TOOL_IMAGE
        })
        .take(MAX_TOOL_IMAGES)
        .collect();
    let left_out = total - usable.len();
    (usable, left_out)
}

/// Pictures from earlier tool calls, gone from the conversation: only the latest screen matters,
/// each costs as much as a page of text, and providers cap how many one request may hold.
fn forget_old_tool_images(history: &mut [Value]) {
    for message in history.iter_mut() {
        let from_tools = message["content"]
            .as_array()
            .and_then(|parts| parts.first())
            .is_some_and(|part| part["text"] == TOOL_IMAGES_NOTE);
        if from_tools {
            message["content"] =
                json!("[Images from earlier tool calls were removed to save context.]");
        }
    }
}

/// Pictures from tool calls, as a user message: a tool's result can only hold text on most wires.
fn tool_images_message(wire: Wire, images: Vec<mcp::ToolImage>, left_out: usize) -> Value {
    let note = json!({ "type": "text", "text": TOOL_IMAGES_NOTE });
    let mut parts: Vec<Value> = std::iter::once(note)
        .chain(images.into_iter().map(|(kind, data)| match wire {
            Wire::Anthropic => json!({
                "type": "image",
                "source": { "type": "base64", "media_type": kind, "data": data }
            }),
            _ => json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{kind};base64,{data}") }
            }),
        }))
        .collect();
    if left_out > 0 {
        parts.push(json!({
            "type": "text",
            "text": format!("{left_out} more couldn't be included: too large, too many, or of a kind this model doesn't read.")
        }));
    }
    json!({ "role": "user", "content": parts })
}

/// Whether `text`, an answer with no tool call in it, ends by promising more.
fn announces_more(text: &str) -> bool {
    let Some(last) = text.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
        return false;
    };
    let line = last.to_lowercase();
    if line.starts_with("let me know") {
        return false;
    }
    line.ends_with(':')
        || [
            "let me ",
            "i'll ",
            "i will ",
            "now i'll ",
            "now i will ",
            "now let me ",
            "next, i",
            "next i",
            "i'm going to ",
            "i am going to ",
        ]
        .iter()
        .any(|start| line.starts_with(start))
}

/// Notices a model making the same call over and over and getting the same answer.
#[derive(Default)]
struct LoopGuard {
    recent: VecDeque<u64>,
}

impl LoopGuard {
    /// How far back a repeat counts.
    const WINDOW: usize = 16;
    const WARN_AT: usize = 4;
    const STOP_AT: usize = 8;

    /// Takes in a finished call: how many of the last few calls were this very one, it included.
    fn record(&mut self, name: &str, args: &str, output: &str) -> usize {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        args.hash(&mut hasher);
        output.hash(&mut hasher);
        let hash = hasher.finish();
        self.recent.push_back(hash);
        while self.recent.len() > Self::WINDOW {
            self.recent.pop_front();
        }
        self.recent.iter().filter(|h| **h == hash).count()
    }
}

/// Old tool output cut down to its first lines, and old reasoning dropped, when that frees enough
/// to be worth changing what the provider has cached (a tenth of the window): the latest outputs
/// stay whole. Whether anything changed.
fn elide_old_outputs(history: &mut [Value], window: u64, model: &str) -> bool {
    const STUB_ABOVE: usize = 1_000;
    const STUB_KEEPS: usize = 300;
    let keep_chars = (window as f64 * ELIDE_KEEPS * 4.0) as usize;
    let needed = (window as f64 * 0.1 * 4.0) as usize;
    // Where "the latest" starts, walking back from the end.
    let mut seen = 0usize;
    let mut cutoff = 0usize;
    for (i, message) in history.iter().enumerate().rev() {
        seen += estimate_chars(message);
        if seen > keep_chars {
            cutoff = i + 1;
            break;
        }
    }
    let output_of = |m: &Value| -> Vec<usize> {
        if m["role"] == "tool" {
            return vec![m["content"].as_str().map_or(0, str::len)];
        }
        m["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .map(|b| b["content"].as_str().map_or(0, str::len))
            .collect()
    };
    let mut freeable = 0usize;
    for message in &history[..cutoff] {
        freeable += output_of(message)
            .iter()
            .filter(|n| **n > STUB_ABOVE)
            .map(|n| n - STUB_KEEPS)
            .sum::<usize>();
    }
    // Reasoning from before the latest message: some models refuse a history without theirs
    // however old, so theirs stays.
    let keeps_reasoning = ["deepseek", "kimi", "moonshot"]
        .iter()
        .any(|m| model.to_lowercase().contains(m));
    let last_user = history
        .iter()
        .rposition(|m| m["role"] == "user" && m["content"].is_string())
        .unwrap_or(0);
    if !keeps_reasoning {
        for message in &history[..last_user] {
            for field in ["reasoning_details", "reasoning", "reasoning_content"] {
                freeable += message.get(field).map_or(0, estimate_chars);
            }
        }
    }
    if freeable < needed {
        return false;
    }
    let stub = |text: &str| -> String {
        format!(
            "{}\n[… {} bytes of this output were removed to save context. Run the call again if you need them.]",
            clip_chars(text, STUB_KEEPS),
            text.len().saturating_sub(STUB_KEEPS)
        )
    };
    for message in &mut history[..cutoff] {
        if message["role"] == "tool" {
            let text = message["content"].as_str().unwrap_or_default();
            if text.len() > STUB_ABOVE {
                message["content"] = json!(stub(text));
            }
        } else if let Some(blocks) = message["content"].as_array_mut() {
            for block in blocks.iter_mut().filter(|b| b["type"] == "tool_result") {
                let text = block["content"].as_str().unwrap_or_default();
                if text.len() > STUB_ABOVE {
                    block["content"] = json!(stub(text));
                }
            }
        }
    }
    if !keeps_reasoning {
        for message in &mut history[..last_user] {
            if let Some(fields) = message.as_object_mut() {
                fields.remove("reasoning_details");
                fields.remove("reasoning");
                fields.remove("reasoning_content");
            }
        }
    }
    true
}

/// Most tokens to let an answer use: what the model allows, and what's left of its window. Without
/// a limit OpenRouter reserves the model's whole maximum against the account's credit and refuses
/// requests it could easily afford. `whole_output`: the last answer spent its limit on reasoning.
/// Only sent where it's safe to: other providers refuse a limit above their own, which isn't known.
fn output_limit(
    provider: &DirectProvider,
    caps: Option<&ModelCaps>,
    window: u64,
    estimate: u64,
    whole_output: bool,
) -> Option<u64> {
    const USUAL: u64 = 32_000;
    const MOST: u64 = 64_000;
    const LEAST: u64 = 1_024;
    let model_max = caps.and_then(|c| c.max_output);
    if provider.local || (model_max.is_none() && provider.id != "openrouter") {
        return None;
    }
    let limit = match (model_max, whole_output) {
        (Some(max), true) => max,
        (Some(max), false) => max.min(MOST),
        (None, true) => MOST,
        (None, false) => USUAL,
    };
    let room = window.saturating_sub(estimate).saturating_sub(LEAST);
    Some(limit.min(room.max(LEAST)))
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
    // The shell expands these into flags the checks below never see: `find . ${x:--delete}`.
    if *prefix != "echo" && args.iter().any(|a| a.contains(['$', '{'])) {
        return false;
    }
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
            "cargo check" | "cargo clippy" => a == "--fix",
            // Follows forever.
            "tail" => short(a, "fF") || a.starts_with("--follow") || a == "--retry",
            _ => false,
        }
    })
}

/// Whether a read-only session may run `cmd`: what Auto would run unasked, less the builds, which
/// run the project's own code.
fn is_read_only_command(cmd: &str) -> bool {
    !cmd.trim_start().starts_with("cargo") && is_safe_command(cmd)
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
        "grep" => (
            "Search",
            parsed["pattern"].as_str().unwrap_or(args_json).to_string(),
        ),
        "glob" => (
            "Find files",
            parsed["pattern"].as_str().unwrap_or(args_json).to_string(),
        ),
        "list_dir" => ("List", parsed["path"].as_str().unwrap_or(".").to_string()),
        "web_fetch" => (
            "Fetch",
            parsed["url"].as_str().unwrap_or(args_json).to_string(),
        ),
        "mcp_list" => (
            "List tools",
            parsed["server"]
                .as_str()
                .unwrap_or("all servers")
                .to_string(),
        ),
        "mcp_call" => (
            "Use tool",
            format!(
                "{} {}",
                parsed["server"].as_str().unwrap_or_default(),
                parsed["tool"].as_str().unwrap_or_default()
            ),
        ),
        // The model's own few words on what the program does, else its first line.
        "script" => {
            let described = parsed["description"]
                .as_str()
                .map(str::trim)
                .filter(|d| !d.is_empty());
            let first_line = || {
                parsed["code"]
                    .as_str()
                    .and_then(|c| c.lines().map(str::trim).find(|l| !l.is_empty()))
            };
            let line = described.or_else(first_line).unwrap_or(args_json);
            ("Run script", line.to_string())
        }
        "transcript_search" => (
            trek_core::session_log::SEARCH_TITLE,
            parsed["query"].as_str().unwrap_or(args_json).to_string(),
        ),
        "transcript_read" => (
            trek_core::session_log::READ_TITLE,
            match (parsed["start"].as_u64(), parsed["end"].as_u64()) {
                (Some(a), Some(b)) => format!("lines {a}-{b}"),
                (Some(a), None) => format!("from line {a}"),
                _ => args_json.to_string(),
            },
        ),
        _ => ("Tool", args_json.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn ask_permission(
    title: &str,
    detail: &str,
    call: Option<Call>,
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
            call,
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
            // A message while the card is up is the user's answer to it: no, and why. (Trek's
            // queue key holds a message back in the app, so one that arrives here was sent.)
            prompt @ Command::Prompt { .. } => {
                later.push(prompt);
                break Ok(Decision::Deny);
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

/// Whether a thread's level, by itself, asks before `tool` (`bash`, `mcp`, `web_fetch`,
/// `write_file` or `edit_file`) acts on `target`: and the card's title, and the key "allow for the
/// session" is remembered under.
fn level_asks(
    tool: &str,
    target: &str,
    cwd: &Path,
    hand_holding: HandHolding,
    allowed_for_session: &HashSet<String>,
) -> (bool, &'static str, String) {
    match tool {
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
        // A tool on an MCP server that doesn't say it only reads (those don't come here below
        // Supervised). Auto trusts Trek's own servers, which have approvals of their own, and asks
        // about the rest, as it does for an ACP agent.
        "mcp" => {
            let server = target.split(' ').next().unwrap_or_default();
            let need_ask = match hand_holding {
                HandHolding::FullAccess => false,
                HandHolding::Auto => {
                    !crate::is_trek_server(server) && !allowed_for_session.contains(target)
                }
                HandHolding::AutoAcceptEdits | HandHolding::Supervised => {
                    !allowed_for_session.contains(target)
                }
            };
            (need_ask, "Use tool", target.to_string())
        }
        // Public pages are like reading a file. An address inside this network isn't, and
        // Supervised asks about everything; an allowance covers the one address.
        "web_fetch" => {
            let need_ask = match hand_holding {
                HandHolding::FullAccess => false,
                HandHolding::Supervised => !allowed_for_session.contains(target),
                HandHolding::Auto | HandHolding::AutoAcceptEdits => {
                    web::is_private(target) && !allowed_for_session.contains(target)
                }
            };
            (need_ask, "Fetch page", target.to_string())
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
    }
}

/// Whether a Direct session at `level` would ask about `call` with no rule to go by, nothing
/// allowed for the session yet: the Settings page's rule tester shows this.
pub fn level_would_ask(call: &Call, cwd: &Path, level: HandHolding) -> bool {
    let tool = match call.kind {
        RuleKind::Command => "bash",
        RuleKind::Edit => "write_file",
        RuleKind::Fetch => "web_fetch",
        RuleKind::Tool => "mcp",
    };
    level_asks(tool, &call.target, cwd, level, &HashSet::new()).0
}

/// The answer to `permitted`.
#[derive(Debug, PartialEq)]
enum Permit {
    Go,
    /// No: what the model is told.
    No(String),
}

/// Whether a rule has something to say that a session's own leave to skip asking mustn't get
/// round: a deny or an ask, in a read-only session whose reads don't ask.
fn rule_speaks(tool: &str, target: &str, cwd: &Path) -> bool {
    let call = crate::rules::direct_call(tool, target);
    matches!(
        crate::rules::ruling(&permission_rules::current(), &[call], cwd, false),
        Ruling::Refuse { .. } | Ruling::Ask
    )
}

/// Whether `tool` (`bash`, `write_file` or `edit_file`) may go ahead on `target` (the command,
/// or the path): the user's rules first, whatever the level; then asks the user when
/// `hand_holding` says to, and remembers "allow for the session".
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
) -> Result<Permit> {
    permitted_under(
        &permission_rules::current(),
        tool,
        target,
        cwd,
        hand_holding,
        allowed_for_session,
        events,
        commands,
        kept,
        shutdown,
    )
    .await
}

/// `permitted`, by `rules`.
#[allow(clippy::too_many_arguments)]
async fn permitted_under(
    rules: &[PermissionRule],
    tool: &str,
    target: &str,
    cwd: &Path,
    hand_holding: HandHolding,
    allowed_for_session: &mut HashSet<String>,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<Permit> {
    let call = crate::rules::direct_call(tool, target);
    let (level_ask, title, session_key) =
        level_asks(tool, target, cwd, hand_holding, allowed_for_session);
    let need_ask = match crate::rules::ruling(rules, std::slice::from_ref(&call), cwd, true) {
        Ruling::Refuse { message, note } => {
            events.send(AgentEvent::Notice(note)).await?;
            return Ok(Permit::No(message));
        }
        Ruling::Allow => false,
        // Every time: "allow for the session" doesn't get past a rule that says to ask.
        Ruling::Ask => true,
        Ruling::Level => level_ask,
    };
    if !need_ask {
        return Ok(Permit::Go);
    }
    Ok(
        match ask_permission(title, target, Some(call), events, commands, kept, shutdown).await? {
            Decision::Allow => Permit::Go,
            Decision::AllowForSession => {
                allowed_for_session.insert(session_key);
                Permit::Go
            }
            Decision::Deny => Permit::No("Permission denied by user".into()),
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
    hub: &mcp::McpHub,
    background: &mut Vec<i32>,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    let exec = options.executor.as_ref();
    // Valid JSON, and already fitted to the tool's schema (`prepare_calls`).
    let parsed: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    let text_of = |key: &str| parsed.get(key).and_then(Value::as_str).unwrap_or_default();
    match name {
        "bash" => {
            let command = text_of("command").trim();
            if command.is_empty() {
                return Ok(("Error: empty command".into(), false, None));
            }
            // Read-only sessions can still look around: what can't change anything runs.
            if read_only && !is_read_only_command(command) {
                return Ok((
                    "Not run: this session is read-only, so bash only runs single commands that just read (git status, git log, git diff, git show, ls, cat, head, tail, grep, rg, find, wc, stat, file, which, pwd, echo), with no pipes or redirects. Use read_file, grep, glob and list_dir for the rest.".into(),
                    false,
                    None,
                ));
            }
            let asks = !read_only
                || hand_holding == HandHolding::Supervised
                || rule_speaks("bash", command, cwd);
            if asks
                && let Permit::No(why) = permitted(
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
                return Ok((why, false, None));
            }
            if !read_only && parsed["background"].as_bool() == Some(true) {
                let (out, ok) =
                    start_in_background(exec, command, cwd, background, commands, kept, shutdown)
                        .await?;
                return Ok((out, ok, None));
            }
            let timeout = parsed["timeout_s"]
                .as_u64()
                .map(|s| Duration::from_secs(s.clamp(1, MAX_COMMAND_TIMEOUT_S)))
                .or(options.command_timeout);
            // A limit the session has to keep holds for the command too.
            let timeout = match (timeout, options.deadline) {
                (Some(limit), Some(deadline)) => {
                    Some(limit.min(deadline.saturating_duration_since(Instant::now())))
                }
                (limit, _) => limit,
            };
            let (out, ok) =
                run_bash_command(exec, command, cwd, timeout, commands, kept, shutdown).await?;
            Ok((out, ok, None))
        }
        "read_file" => {
            let start = parsed["start_line"].as_u64().map(|s| s as usize);
            let end = parsed["end_line"].as_u64().map(|e| e as usize);
            let (out, ok) = run_read_file(
                exec,
                text_of("path"),
                start,
                end,
                cwd,
                &mut Waiting {
                    commands,
                    kept: &mut *kept,
                    shutdown: &mut *shutdown,
                },
            )
            .await?;
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
            let path_str = text_of("path");
            if let Permit::No(why) = permitted(
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
                return Ok((why, false, None));
            }
            run_write_file(
                exec,
                path_str,
                text_of("content"),
                cwd,
                &mut Waiting {
                    commands,
                    kept: &mut *kept,
                    shutdown: &mut *shutdown,
                },
            )
            .await
        }
        "edit_file" => {
            if read_only {
                return Ok((
                    "Permission denied: session is in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let path_str = text_of("path");
            if text_of("old_str").is_empty() {
                return Ok((
                    "Error: old_str is empty. To create a file or replace all of one, use write_file.".into(),
                    false,
                    None,
                ));
            }
            if let Permit::No(why) = permitted(
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
                return Ok((why, false, None));
            }
            run_edit_file(
                exec,
                path_str,
                text_of("old_str"),
                text_of("new_str"),
                parsed["replace_all"].as_bool() == Some(true),
                cwd,
                &mut Waiting {
                    commands,
                    kept: &mut *kept,
                    shutdown: &mut *shutdown,
                },
            )
            .await
        }
        "grep" | "glob" | "list_dir" => {
            let mut waiting = Waiting {
                commands,
                kept: &mut *kept,
                shutdown: &mut *shutdown,
            };
            let (out, ok) = run_search(exec, name, &parsed, cwd, &mut waiting).await?;
            Ok((out, ok, None))
        }
        "web_fetch" => {
            let url = text_of("url").trim();
            if let Permit::No(why) = permitted(
                "web_fetch",
                url,
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
                return Ok((why, false, None));
            }
            // Every hop of a redirect answers to the user's rules, as the address asked for did.
            let rules = permission_rules::current();
            let cwd_owned = cwd.to_path_buf();
            let hop_allowed: web::HopCheck = std::sync::Arc::new(move |hop: &str| {
                let call = crate::rules::direct_call("web_fetch", hop);
                matches!(
                    crate::rules::ruling(&rules, &[call], &cwd_owned, true),
                    Ruling::Allow | Ruling::Level
                )
            });
            let fetched =
                until_stopped(web::fetch(url, hop_allowed), commands, kept, shutdown).await?;
            let (text, ok) = match fetched {
                Ok(text) => (text, true),
                Err(message) => (message, false),
            };
            let mut waiting = Waiting {
                commands,
                kept: &mut *kept,
                shutdown: &mut *shutdown,
            };
            Ok((fit_output(exec, text, false, &mut waiting).await?, ok, None))
        }
        "mcp_list" => {
            let server = parsed["server"].as_str();
            let (text, ok) = until_stopped(hub.listing(server), commands, kept, shutdown).await?;
            let mut waiting = Waiting {
                commands,
                kept: &mut *kept,
                shutdown: &mut *shutdown,
            };
            Ok((fit_output(exec, text, false, &mut waiting).await?, ok, None))
        }
        "mcp_call" => {
            let (server, tool_name) = (text_of("server"), text_of("tool"));
            let arguments = match &parsed["arguments"] {
                Value::Null => json!({}),
                Value::String(raw) => match repair::parse_args(raw).value() {
                    Some(v) => v.clone(),
                    None => {
                        return Ok((
                            "Error: arguments must be a JSON object.".to_string(),
                            false,
                            None,
                        ));
                    }
                },
                other => other.clone(),
            };
            let found =
                until_stopped(hub.tool(server, tool_name), commands, kept, shutdown).await?;
            let info = match found {
                Ok(info) => info,
                Err(message) => return Ok((message, false, None)),
            };
            if read_only && !info.read_only {
                return Ok((
                    format!(
                        "Not run: this session is read-only, and {server} doesn't say that {tool_name} only reads."
                    ),
                    false,
                    None,
                ));
            }
            let target = format!("{server} {tool_name}");
            let asks = !info.read_only
                || hand_holding == HandHolding::Supervised
                || rule_speaks("mcp", &target, cwd);
            if asks
                && let Permit::No(why) = permitted(
                    "mcp",
                    &target,
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
                return Ok((why, false, None));
            }
            let (text, ok) = until_stopped(
                hub.call(server, tool_name, arguments),
                commands,
                kept,
                shutdown,
            )
            .await?;
            let mut waiting = Waiting {
                commands,
                kept: &mut *kept,
                shutdown: &mut *shutdown,
            };
            Ok((fit_output(exec, text, false, &mut waiting).await?, ok, None))
        }
        // A tool the session has on offer: a program whose calls run on the same executor. Its
        // approvals are asked here while it waits; a Stop drops it like it does a command, and the
        // calls it has in flight with it.
        "script" if script => {
            if read_only {
                return Ok((
                    "Permission denied: scripts are disabled in read-only mode".into(),
                    false,
                    None,
                ));
            }
            let code = parsed["code"].as_str().unwrap_or_default();
            if code.trim().is_empty() {
                return Ok(("Error: code parameter is required".into(), false, None));
            }
            let (to_session, mut from_script) = tokio::sync::mpsc::unbounded_channel();
            let tools = Arc::new(script::ScriptTools::new(
                options.executor.clone(),
                cwd.to_path_buf(),
                options.command_timeout,
                hand_holding != HandHolding::FullAccess || !permission_rules::current().is_empty(),
                options.deadline,
                to_session,
            ));
            let mut limits = trek_script::ScriptLimits {
                log_dir: options.script_log_dir.clone(),
                ..Default::default()
            };
            // Within the session's time limit too: the program stops with it.
            if let Some(deadline) = options.deadline {
                limits.wall_clock = limits
                    .wall_clock
                    .min(deadline.saturating_duration_since(Instant::now()));
            }
            let run = trek_script::run_script(code, tools, limits);
            tokio::pin!(run);
            let result = loop {
                tokio::select! {
                    result = &mut run => break result,
                    Some(said) = from_script.recv() => {
                        let ask = match said {
                            script::ToSession::Approval(ask) => ask,
                            // The executor is gone: the program ends here (`run` dropped), and
                            // the turn with it, as it would for one of the model's own calls.
                            script::ToSession::Gone(reason) => {
                                return Err(ExecutorGone(reason).into());
                            }
                        };
                        let answer = permitted(
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
                        let _ = ask.reply.send(answer == Permit::Go);
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
            format!("Error: tool '{unknown}' does not exist in this session."),
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

/// Most of a tool's output the model is told, in bytes. The rest is saved where it can look.
const MAX_OUT: usize = 30_000;
/// Longest a command may ask to run for.
const MAX_COMMAND_TIMEOUT_S: u64 = 3_600;

/// `text`, or if it's too long, its start and end around a note saying where all of it is. A
/// command's end matters most (that's where the build error is); a listing's or a page's, its start.
async fn fit_output(
    exec: &dyn ToolExecutor,
    text: String,
    keep_end: bool,
    waiting: &mut Waiting<'_>,
) -> Result<String> {
    if text.len() <= MAX_OUT {
        return Ok(text);
    }
    let (head, tail) = if keep_end {
        (MAX_OUT / 5, MAX_OUT * 4 / 5)
    } else {
        (MAX_OUT * 4 / 5, MAX_OUT / 5)
    };
    let mut head_end = head;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - tail;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let path = exec
        .temp_dir()
        .join("trek-output")
        .join(format!("{}.txt", next_id()));
    // The file outlives the session's secrets: what a command printed of one doesn't go in it.
    let mut kept_text = text.clone();
    trek_core::session_secrets::scrub_string(&mut kept_text, &trek_core::session_secrets::known());
    let saved = waiting.on(exec.write(&path, kept_text.as_bytes())).await?;
    let cut = tail_start - head_end;
    let note = match saved {
        Ok(()) => format!(
            "[… {cut} bytes cut here. All of it is in {}: read_file pages through it and grep searches it.]",
            path.display()
        ),
        Err(_) => format!("[… {cut} bytes cut here.]"),
    };
    Ok(format!(
        "{}\n\n{note}\n\n{}",
        &text[..head_end],
        &text[tail_start..]
    ))
}

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
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&ran.stderr);
    }
    let mut waiting = Waiting {
        commands,
        kept,
        shutdown,
    };
    let mut out = fit_output(exec, out, true, &mut waiting).await?;
    let mut ending = |line: String| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
    };
    if ran.timed_out {
        ending(match timeout {
            Some(limit) => format!(
                "[Command timed out after {} seconds and was stopped. If it needs longer, run it again with a larger timeout_s (up to {MAX_COMMAND_TIMEOUT_S}), or with background set to true.]",
                limit.as_secs()
            ),
            None => "[Command timed out and was stopped]".to_string(),
        });
    } else {
        // Without this a failure can read as success: `which x` says nothing at all.
        match ran.exit_code {
            Some(0) => {}
            Some(code) => ending(format!("[exit code {code}]")),
            None => ending("[ended by a signal]".to_string()),
        }
    }
    if out.trim().is_empty() {
        out = "(Command finished with no output)".to_string();
    }
    Ok((out, ok))
}

/// Starts `command` and doesn't wait for it: its output goes to a file, and it stays running until
/// the model stops it or the session ends.
async fn start_in_background(
    exec: &dyn ToolExecutor,
    command: &str,
    cwd: &Path,
    background: &mut Vec<i32>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<(String, bool)> {
    let log = exec
        .temp_dir()
        .join("trek-output")
        .join(format!("background-{}.log", next_id()));
    let shown = log.display().to_string();
    let mut waiting = Waiting {
        commands,
        kept,
        shutdown,
    };
    if let Err(e) = waiting.on(exec.write(&log, b"")).await? {
        return Ok((format!("Failed to start in the background: {e:#}"), false));
    }
    let windows = exec.is_local() && cfg!(windows);
    let wrapped = if windows {
        format!("start \"\" /b cmd /c \"{command} > \"{shown}\" 2>&1\"")
    } else {
        format!(
            "( {command}\n) > {} 2>&1 < /dev/null &\necho \"$$\"",
            search::quote(&shown)
        )
    };
    let ran = match waiting
        .on(exec.exec(&wrapped, cwd, Some(Duration::from_secs(10))))
        .await?
    {
        Ok(ran) => ran,
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => return Err(e),
        Err(e) => return Ok((format!("Failed to start in the background: {e:#}"), false)),
    };
    if !ran.success() {
        return Ok((
            format!(
                "Failed to start in the background: {}{}",
                ran.stdout, ran.stderr
            ),
            false,
        ));
    }
    let Some(group) = ran.stdout.trim().parse::<i32>().ok() else {
        return Ok((
            format!(
                "Started in the background. Its output goes to {shown}: read it with read_file."
            ),
            true,
        ));
    };
    background.push(group);
    // Trek ending, or a later launch finding it left over, ends it too.
    if exec.is_local() {
        trek_core::procs::register(group);
    }
    Ok((
        format!(
            "Started in the background as process group {group}. Its output goes to {shown}: read it with read_file. To stop it: kill -- -{group}"
        ),
        true,
    ))
}

/// A tool's way to hear a Stop while it waits on the executor (see `until_stopped`): a read or
/// write the sandbox never answers mustn't hold the turn.
struct Waiting<'a> {
    commands: &'a async_channel::Receiver<Command>,
    kept: &'a mut VecDeque<Command>,
    shutdown: &'a mut bool,
}

impl Waiting<'_> {
    async fn on<T>(&mut self, run: impl std::future::Future<Output = T>) -> Result<T> {
        until_stopped(run, self.commands, self.kept, self.shutdown).await
    }
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
    waiting: &mut Waiting<'_>,
) -> Result<std::result::Result<String, String>> {
    match waiting.on(exec.read(path)).await? {
        Ok(Some(bytes)) => Ok(String::from_utf8(bytes).map_err(|_| {
            format!(
                "Error reading {path_str}: stream did not contain valid UTF-8. It's a binary file, so use a command that understands its format."
            )
        })),
        Ok(None) => Ok(Err(format!("File not found: {path_str}"))),
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => Err(e),
        Err(e) => Ok(Err(format!("Error reading {path_str}: {e:#}"))),
    }
}

/// Most lines one `read_file` returns.
const READ_LINES: usize = 2_000;
/// Most bytes it returns, whatever the lines.
const READ_BYTES: usize = 50_000;
/// A longer line is cut: one minified line can fill a context by itself.
const READ_LINE: usize = 2_000;

async fn run_read_file(
    exec: &dyn ToolExecutor,
    path_str: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    cwd: &Path,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool)> {
    let path = tool_path(path_str, cwd);
    // Models read folders all the time. It isn't worth an error to say what they wanted to know.
    if exec.is_local() && path.is_dir() {
        let (listing, ok) = list_folder(exec, &path, path_str, cwd, waiting).await?;
        return Ok((format!("{path_str} is a folder. It holds:\n{listing}"), ok));
    }
    let content = match read_text(exec, &path, path_str, waiting).await? {
        Ok(c) => c,
        Err(message) if message.to_lowercase().contains("is a directory") => {
            let (listing, ok) = list_folder(exec, &path, path_str, cwd, waiting).await?;
            return Ok((format!("{path_str} is a folder. It holds:\n{listing}"), ok));
        }
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
    let mut shown_to = start;
    let mut clipped = false;
    for (idx, line) in lines[start..end].iter().enumerate() {
        if idx == READ_LINES || out.len() >= READ_BYTES {
            break;
        }
        let number = start + idx + 1;
        if line.len() > READ_LINE {
            clipped = true;
            out.push_str(&format!("{number:4} | {}\n", clip_chars(line, READ_LINE)));
        } else {
            out.push_str(&format!("{number:4} | {line}\n"));
        }
        shown_to = number;
    }
    if shown_to < end {
        out.push_str(&format!(
            "(Showing lines {}-{shown_to} of {}. To read on, call read_file with start_line {}.)\n",
            start + 1,
            lines.len(),
            shown_to + 1
        ));
    } else if end == lines.len() && start > 0 {
        out.push_str(&format!("(End of file: {} lines.)\n", lines.len()));
    } else if end < lines.len() {
        out.push_str(&format!("(The file has {} lines.)\n", lines.len()));
    }
    if clipped {
        out.push_str("(Lines ending in … were cut. Use grep or bash to see the rest of them.)\n");
    }
    Ok((out, true))
}

/// What's in a folder, for `read_file` on one and for `list_dir`.
async fn list_folder(
    exec: &dyn ToolExecutor,
    path: &Path,
    path_str: &str,
    cwd: &Path,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool)> {
    let (listing, ok) = if exec.is_local() {
        let path = path.to_path_buf();
        match waiting
            .on(tokio::task::spawn_blocking(move || search::list(&path)))
            .await?
        {
            Ok(Ok(text)) => (text, true),
            Ok(Err(message)) => (message, false),
            Err(e) => (format!("Error listing {path_str}: {e}"), false),
        }
    } else {
        let command = search::list_command(&path.display().to_string());
        remote_listing(exec, &command, cwd, "(Empty folder)", waiting).await?
    };
    Ok((listing, ok))
}

/// A search's command run in a sandbox: its lines, `empty` when there are none.
async fn remote_listing(
    exec: &dyn ToolExecutor,
    command: &str,
    cwd: &Path,
    empty: &str,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool)> {
    let ran = match waiting
        .on(exec.exec(command, cwd, Some(Duration::from_secs(60))))
        .await?
    {
        Ok(ran) => ran,
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => return Err(e),
        Err(e) => return Ok((format!("Error: {e:#}"), false)),
    };
    if ran.stdout.trim().is_empty() {
        return Ok(if ran.stderr.trim().is_empty() {
            (empty.to_string(), true)
        } else {
            (format!("Error: {}", ran.stderr.trim()), false)
        });
    }
    let lines = ran.stdout.lines().count();
    let mut out = ran.stdout;
    if lines > search::MAX_RESULTS {
        out.push_str("(There are more: narrow it down.)\n");
    }
    Ok((out, true))
}

/// `grep`, `glob` or `list_dir`.
async fn run_search(
    exec: &dyn ToolExecutor,
    tool: &str,
    args: &Value,
    cwd: &Path,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool)> {
    let path_str = args["path"].as_str().unwrap_or(".").trim();
    let root = match path_str {
        "" | "." => cwd.to_path_buf(),
        given => tool_path(given, cwd),
    };
    let pattern = args["pattern"].as_str().unwrap_or_default().to_string();
    if tool != "list_dir" && pattern.is_empty() {
        return Ok((format!("Error: {tool} needs a pattern."), false));
    }
    if exec.is_local() && !root.exists() {
        return Ok((format!("Error: {path_str} doesn't exist."), false));
    }
    if tool == "list_dir" {
        return list_folder(exec, &root, path_str, cwd, waiting).await;
    }
    let glob_filter = args["glob"].as_str().map(str::to_string);
    let case_insensitive = args["case_insensitive"].as_bool() == Some(true);
    let (out, ok) = if exec.is_local() {
        let tool = tool.to_string();
        let done = waiting
            .on(tokio::task::spawn_blocking(move || {
                if tool == "grep" {
                    search::grep(&root, &pattern, glob_filter.as_deref(), case_insensitive)
                } else {
                    search::glob(&root, &pattern)
                }
            }))
            .await?;
        match done {
            Ok(Ok(text)) => (text, true),
            Ok(Err(message)) => (message, false),
            Err(e) => (format!("Error: the search failed: {e}"), false),
        }
    } else {
        let root = root.display().to_string();
        let (command, empty) = if tool == "grep" {
            (
                search::grep_command(&root, &pattern, glob_filter.as_deref(), case_insensitive),
                "No matches.".to_string(),
            )
        } else {
            (
                search::glob_command(&root, &pattern),
                format!("No files match {pattern}."),
            )
        };
        remote_listing(exec, &command, cwd, &empty, waiting).await?
    };
    Ok((fit_output(exec, out, false, waiting).await?, ok))
}

async fn run_write_file(
    exec: &dyn ToolExecutor,
    path_str: &str,
    content: &str,
    cwd: &Path,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    let path = tool_path(path_str, cwd);
    let added = content.lines().count() as u32;
    match waiting.on(exec.write(&path, content.as_bytes())).await? {
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
    replace_all: bool,
    cwd: &Path,
    waiting: &mut Waiting<'_>,
) -> Result<(String, bool, Option<(u32, u32)>)> {
    if old_str == new_str {
        return Ok((
            "Error: old_str and new_str are the same, so there's nothing to change.".to_string(),
            false,
            None,
        ));
    }
    let path = tool_path(path_str, cwd);
    let content = match read_text(exec, &path, path_str, waiting).await? {
        Ok(c) => c,
        Err(message) => return Ok((message, false, None)),
    };
    let found = match edit::locate(path_str, &content, old_str, new_str, replace_all) {
        edit::Outcome::Found(found) => found,
        edit::Outcome::Ambiguous { count, lines } => {
            let places: Vec<String> = lines.iter().map(usize::to_string).collect();
            return Ok((
                format!(
                    "Error: old_str matched {count} times in {path_str} (lines {}). Add the lines around it so it matches one place, or set replace_all to true.",
                    places.join(", ")
                ),
                false,
                None,
            ));
        }
        edit::Outcome::NotFound(message) => return Ok((message, false, None)),
    };
    let updated = edit::apply(&content, &found);
    match waiting.on(exec.write(&path, updated.as_bytes())).await? {
        Ok(()) => {
            let places = found.ranges.len() as u32;
            let removed = old_str.lines().count() as u32 * places;
            let added = new_str.lines().count() as u32 * places;
            let mut message = format!("Successfully edited {path_str}");
            if places > 1 {
                message.push_str(&format!(" ({places} places)"));
            }
            if let Some(how) = found.how {
                message.push_str(&format!(
                    ". old_str only matched after allowing for {how}: check the result"
                ));
            }
            let (first, _) = found.ranges[0];
            message.push_str(":\n");
            message.push_str(&edit::snippet(&updated, first, found.replacements[0].len()));
            Ok((message, true, Some((added, removed))))
        }
        Err(e) if e.downcast_ref::<ExecutorGone>().is_some() => Err(e),
        Err(e) => Ok((
            format!("Error writing edits to {path_str}: {e:#}"),
            false,
            None,
        )),
    }
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
fn user_message(
    wire: Wire,
    text: &str,
    images: &[PathBuf],
    caps: Option<&ModelCaps>,
) -> (Value, Vec<String>) {
    let mut errors = Vec::new();
    let mut loaded = Vec::new();
    // Sent anyway, the whole request would be refused or the picture silently ignored.
    if !images.is_empty() && caps.and_then(|c| c.images) == Some(false) {
        let names: Vec<String> = images
            .iter()
            .map(|p| format!("{}: this model can't see images", p.display()))
            .collect();
        return (json!({ "role": "user", "content": text }), names);
    }
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
            response = &mut send => {
                return response.map_err(|e| {
                    // A server on this machine that isn't running won't start by being asked again.
                    let here = e
                        .url()
                        .and_then(|u| u.host_str())
                        .is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
                    if e.is_connect() && here {
                        return RequestFailure {
                            kind: FailureKind::Fatal,
                            message: format!("Nothing is answering at {}. Is the server running?", e.url().map_or("", |u| u.as_str())),
                            retry_after: None,
                            limit: None,
                        }
                        .into();
                    }
                    RequestFailure::transient(format!("Couldn't reach the provider: {e}")).into()
                });
            }
            _ = &mut sleep => {
                return Err(RequestFailure::transient(format!(
                    "Timed out waiting for response headers after {} seconds",
                    timeout.as_secs()
                )).into());
            }
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

/// Anthropic's message thread for the session's conversation (beta `message-threads-2026-08-12`):
/// the server keeps the conversation, system prompt and tools for five minutes after each
/// request, so a request within that time carries only the messages since the last response.
/// Fewer bytes to upload and tokenize, so the first token comes sooner, most of all in a long
/// run of tool calls. `history` here stays the truth: when the server no longer holds the
/// conversation as it is here (the thread expired, a message was taken back, the context went on
/// afresh, the model or system prompt changed), the next request sends all of it again and starts
/// a new thread; a 404 for a thread the server lost is replayed the same way, once.
#[derive(Default)]
struct Thread {
    /// The response the server's copy ends with, to continue from.
    previous: Option<String>,
    /// How many of `history`'s messages the server holds, and a hash of them: an edit means a
    /// new thread.
    held: usize,
    held_hash: u64,
    /// The fixed fields (model, effort, system prompt, tools) the thread was created with: a
    /// change means a new thread.
    fixed: u64,
    last_use: Option<Instant>,
    /// No threads this session: another endpoint than Anthropic's, the kill switch, or the API
    /// refused them.
    off: bool,
}

/// A thread expires 5 minutes after its last request; a little under, so a request sent right
/// at the end isn't a wasted round trip.
const THREAD_KEEPS: Duration = Duration::from_secs(4 * 60 + 40);
const THREADS_BETA: &str = "message-threads-2026-08-12";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// The error codes of a continue request the server can't honour: the conversation is replayed
/// in a new thread.
const THREAD_LOST: [&str; 4] = [
    "thread_not_found",
    "thread_already_continued",
    "thread_continue_in_flight",
    "thread_fingerprint_mismatch",
];

/// What a request says about the thread.
#[derive(Clone, Debug, PartialEq)]
enum ThreadRequest {
    /// No thread: the whole conversation, system prompt and tools, every request.
    None,
    /// Start one with the whole conversation.
    Create,
    /// Go on from the response `previous`: only the messages after the `held` the server has,
    /// no system prompt or tools.
    Continue { previous: String, held: usize },
}

impl Thread {
    /// The request to make now: a continue while the server holds this conversation as it is
    /// here, under the same fixed fields, with something new to send.
    fn request(&self, fixed: u64, history: &[Value], now: Instant) -> ThreadRequest {
        if self.off {
            return ThreadRequest::None;
        }
        match (&self.previous, self.last_use) {
            (Some(previous), Some(at))
                if self.fixed == fixed
                    && now.saturating_duration_since(at) < THREAD_KEEPS
                    && self.held < history.len()
                    && hash_messages(history[..self.held].iter()) == self.held_hash =>
            {
                ThreadRequest::Continue {
                    previous: previous.clone(),
                    held: self.held,
                }
            }
            _ => ThreadRequest::Create,
        }
    }

    /// The server stored `answer` (sent in reply to `history`) as response `id`.
    fn stored(&mut self, id: String, fixed: u64, history: &[Value], answer: &Value) {
        self.previous = Some(id);
        self.held = history.len() + 1;
        self.held_hash = hash_messages(history.iter().chain(std::iter::once(answer)));
        self.fixed = fixed;
        self.last_use = Some(Instant::now());
    }

    /// The server's copy can't be continued: the next request starts a thread anew.
    fn forget(&mut self) {
        self.previous = None;
    }
}

fn hash_messages<'a>(messages: impl Iterator<Item = &'a Value>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for message in messages {
        message.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// The request fields a thread fixes at its creation, as one hash.
fn fixed_fields(model: &str, effort: Effort, system: &str, tools: &[Value]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    model.hash(&mut hasher);
    effort.as_str().hash(&mut hasher);
    system.hash(&mut hasher);
    // The same text as `Value::Array(tools.to_vec()).to_string()`, without copying the tools.
    serde_json::to_string(tools)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

fn anthropic_body(
    model: &str,
    effort: Effort,
    system: &str,
    history: &[Value],
    tools: &[Value],
    thread: &ThreadRequest,
) -> Value {
    // Prompt caching: the tools and system prompt (the first breakpoint covers both: tools come
    // first), and the conversation up to its last message, so each request in a run of tool
    // calls reads the one before it from the cache. On a thread the top-level `cache_control`
    // marks the last message instead (a fixed field, the same on every request of the thread),
    // and a continue request leaves the system prompt and tools out: the server has them.
    let mut tools = anthropic_tools(tools);
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }
    let mut messages = match thread {
        ThreadRequest::Continue { held, .. } => history[(*held).min(history.len())..].to_vec(),
        _ => history.to_vec(),
    };
    if *thread == ThreadRequest::None {
        if let Some(last) = messages.last_mut() {
            if let Some(text) = last["content"].as_str() {
                last["content"] = json!([{ "type": "text", "text": text }]);
            }
            if let Some(block) = last["content"]
                .as_array_mut()
                .and_then(|blocks| blocks.last_mut())
            {
                block["cache_control"] = json!({ "type": "ephemeral" });
            }
        }
    }
    let mut body = json!({
        "model": model,
        "max_tokens": 64000,
        "stream": true,
        "messages": messages,
    });
    if !matches!(thread, ThreadRequest::Continue { .. }) {
        body["system"] =
            json!([{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }]);
        body["tools"] = Value::Array(tools);
    }
    match thread {
        ThreadRequest::None => {}
        ThreadRequest::Create => {
            body["thread"] = json!({ "type": "create" });
            body["cache_control"] = json!({ "type": "ephemeral" });
        }
        ThreadRequest::Continue { previous, .. } => {
            body["thread"] = json!({ "type": "continue", "previous_message_id": previous });
            body["cache_control"] = json!({ "type": "ephemeral" });
        }
    }
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
    req: &TurnRequest<'_>,
    thread: &mut Thread,
    partial: &mut String,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    let (model, effort, system, history, tools) =
        (req.model, req.effort, req.system, req.history, req.tools);
    let fixed = fixed_fields(model, effort, system, tools);
    let mut request = thread.request(fixed, history, Instant::now());
    // Once: a thread the server lost is replayed as a new one; a thread it refuses, left out.
    let mut retried = false;
    let resp = loop {
        let mut body = anthropic_body(model, effort, system, history, tools, &request);
        if req.no_tools {
            body["tool_choice"] = json!({ "type": "none" });
        }
        if let Some(limit) = req.max_output {
            body["max_tokens"] = json!(limit);
        }
        let mut betas = Vec::new();
        if body.get("fallbacks").is_some() {
            betas.push(FALLBACK_BETA);
        }
        if request != ThreadRequest::None {
            betas.push(THREADS_BETA);
        }
        let mut post = req
            .client
            .post(req.url)
            .header("x-api-key", req.key.unwrap_or_default())
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        if !betas.is_empty() {
            post = post.header("anthropic-beta", betas.join(","));
        }
        let resp = send_request(post, commands, kept, shutdown, HEADERS_WAIT).await?;
        if resp.status().is_success() {
            break resp;
        }
        let status = resp.status();
        let headers = resp.headers().clone();
        let text = until_stopped(resp.text(), commands, kept, shutdown)
            .await?
            .unwrap_or_default();
        let error = serde_json::from_str::<Value>(&text).ok();
        let code = error
            .as_ref()
            .and_then(|v| v["error"]["details"]["error_code"].as_str())
            .unwrap_or_default()
            .to_string();
        if !retried && request != ThreadRequest::None && code.starts_with("thread_") {
            retried = true;
            thread.forget();
            request = match request {
                ThreadRequest::Continue { .. } if THREAD_LOST.contains(&code.as_str()) => {
                    ThreadRequest::Create
                }
                _ => {
                    thread.off = true;
                    ThreadRequest::None
                }
            };
            continue;
        }
        let msg = error
            .as_ref()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(request_error(
            status,
            &headers,
            format!("Anthropic API {status}: {msg}"),
        ));
    };
    let mut blocks: Vec<Value> = Vec::new();
    // A tool call's input, streamed as pieces of JSON, by block index.
    let mut inputs: HashMap<usize, String> = HashMap::new();
    let mut text = String::new();
    let mut stop_reason = None;
    let mut terminal = false;
    let mut usage = TokenUsage::default();
    let mut long_writes = 0;
    let mut id = None;
    let mut fell_back = false;
    let streamed: Result<()> = async {
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        let idle = tokio::time::sleep(STREAM_IDLE);
        tokio::pin!(idle);
        let mut ended = false;
        loop {
            tokio::select! {
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buf.extend_from_slice(&bytes);
                            idle.as_mut().reset(tokio::time::Instant::now() + STREAM_IDLE);
                        }
                        Some(Err(e)) => {
                            return Err(RequestFailure::transient(format!(
                                "The connection dropped while the answer was coming: {e}"
                            )).into());
                        }
                        None => {
                            ended = true;
                            if !buf.is_empty() {
                                buf.extend_from_slice(b"\n\n");
                            }
                        }
                    }
                }
                _ = &mut idle => {
                    return Err(RequestFailure::transient(format!(
                        "The provider sent nothing for {} seconds",
                        STREAM_IDLE.as_secs()
                    )).into());
                }
                cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
            }
            for data in sse_events(&mut buf) {
                let Ok(v) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                anthropic_usage(&v, &mut usage, &mut long_writes);
                match v["type"].as_str() {
                    Some("message_start") => id = v["message"]["id"].as_str().map(String::from),
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
                            Some("signature_delta") => append(
                                &mut blocks,
                                i,
                                "signature",
                                d["signature"].as_str().unwrap_or_default(),
                            ),
                            Some("input_json_delta") => inputs
                                .entry(i)
                                .or_default()
                                .push_str(d["partial_json"].as_str().unwrap_or_default()),
                            _ => {}
                        }
                    }
                    Some("message_delta") => {
                        stop_reason = v["delta"]["stop_reason"].as_str().map(String::from);
                        // A fallback model's answer: the thread keeps one model, so the
                        // server doesn't store it.
                        fell_back |= v["usage"]["iterations"].as_array().is_some_and(|its| {
                            its.iter().any(|it| {
                                it["type"] == "fallback_message"
                                    || it.get("fallback_message").is_some()
                            })
                        });
                    }
                    Some("message_stop") => terminal = true,
                    Some("error") => return Err(failure::classify_stream_error(&v["error"]).into()),
                    _ => {}
                }
            }
            if ended || terminal {
                return Ok(());
            }
        }
    }
    .await;
    if let Err(e) = streamed {
        thread.forget();
        partial.clone_from(&text);
        if !text.is_empty() && !is_stop(&e) {
            events.send(AgentEvent::TextDone(text.clone())).await?;
        }
        return Err(e);
    }
    if !terminal {
        thread.forget();
        partial.clone_from(&text);
        if !text.is_empty() {
            events.send(AgentEvent::TextDone(text.clone())).await?;
        }
        return Err(
            RequestFailure::transient("The connection closed before the answer finished").into(),
        );
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
        // The server may hold the refused turn, which a continue can't take back.
        thread.forget();
        return Err(RequestFailure {
            kind: FailureKind::Fatal,
            message: "The model declined this request.".to_string(),
            retry_after: None,
            limit: None,
        }
        .into());
    }
    events.send(AgentEvent::TextDone(text.clone())).await?;
    let mut tool_calls = Vec::new();
    for (i, block) in blocks.iter_mut().enumerate() {
        if block["type"] != "tool_use" {
            continue;
        }
        // Sent back as an object, as the API gave it; the tool takes it as JSON text. What can't
        // be read goes back as an empty object, and the session tells the model what happened.
        let raw = inputs.get(&i).filter(|s| !s.trim().is_empty()).cloned();
        if let Some(input) = &raw {
            block["input"] = repair::parse_args(input)
                .value()
                .cloned()
                .unwrap_or_else(|| json!({}));
        }
        tool_calls.push(AccumulatedToolCall {
            id: block["id"].as_str().unwrap_or_default().to_string(),
            name: block["name"].as_str().unwrap_or_default().to_string(),
            arguments: raw.unwrap_or_else(|| block["input"].to_string()),
        });
    }
    if req.no_tools {
        tool_calls.clear();
    }
    let answer = json!({ "role": "assistant", "content": blocks });
    if request != ThreadRequest::None {
        // Not stored: a fallback model's answer, or one `max_tokens` cut off mid tool call.
        match id {
            Some(id) if !fell_back && stop_reason.as_deref() != Some("max_tokens") => {
                thread.stored(id, fixed, history, &answer)
            }
            _ => thread.forget(),
        }
    }
    Ok(TurnResponse {
        text,
        content: Some(answer),
        reasoning_details: vec![],
        reasoning: None,
        finish: stop_reason.map(|r| match r.as_str() {
            "max_tokens" | "model_context_window_exceeded" => "length".to_string(),
            _ => r,
        }),
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

/// The message in a failed response's body, else the body.
fn error_message(text: &str) -> String {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let error = parsed.as_ref().map(|v| &v["error"]);
    let message = error.and_then(|e| e["message"].as_str().or(e.as_str()));
    // OpenRouter puts what the upstream really said under `metadata`, and "Provider returned
    // error" on top of it.
    let raw = error.and_then(|e| e["metadata"]["raw"].as_str());
    match (message, raw) {
        (Some(message), Some(raw)) if message != raw => format!("{message}: {raw}"),
        (Some(message), _) => message.to_string(),
        _ => text.to_string(),
    }
}

/// How long a provider may take to start answering. A cold local model or a queued route is slow,
/// not broken.
const HEADERS_WAIT: Duration = Duration::from_secs(300);
/// How long a stream may go quiet before it's given up on. OpenRouter's keep-alive comments count
/// as life; a model thinking in silence for this long has stalled.
const STREAM_IDLE: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(300)
};
/// How long to wait for the rest of a stream (its usage, its end) after the answer finished.
const AFTER_FINISH: Duration = Duration::from_secs(5);

/// What a provider turned out not to take, learned from its errors and remembered for the rest of
/// the session, so one refusal doesn't become one refused request per round.
#[derive(Default)]
struct Quirks {
    no_reasoning: bool,
    no_output_limit: bool,
    /// It wants `max_completion_tokens`, not `max_tokens`.
    completion_field: bool,
    no_routing: bool,
    no_stream_options: bool,
    no_tools: bool,
    no_session_id: bool,
    no_cache_key: bool,
    /// All the credit will cover (OpenRouter: "can only afford N").
    afford: Option<u64>,
    /// It refuses reasoning sent back to it, though it sent it.
    no_reasoning_echo: bool,
    /// Tool call ids handed out so far: one never repeats in a conversation.
    seen_ids: HashSet<String>,
}

impl Quirks {
    /// Another model is in use: what was learned about this one no longer holds. What's about
    /// the account and the conversation stays.
    fn forget_model(&mut self) {
        *self = Quirks {
            afford: self.afford,
            no_session_id: self.no_session_id,
            no_cache_key: self.no_cache_key,
            seen_ids: std::mem::take(&mut self.seen_ids),
            ..Quirks::default()
        };
    }

    /// Whether what a provider refused can be fixed by changing `body`, and if so does it. Anything
    /// else is left to fail with its own message: sending the same conversation without its tools,
    /// or without some part nobody can name, gets a confident answer to the wrong question.
    fn adapt(&mut self, status: u16, message: &str, body: &mut Value) -> bool {
        let m = message.to_lowercase();
        if failure::is_context_overflow(&m) {
            return false;
        }
        let Some(object) = body.as_object_mut() else {
            return false;
        };
        if status == 402 {
            let afford = m
                .split("can only afford")
                .nth(1)
                .and_then(|rest| {
                    let digits: String = rest
                        .trim_start()
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    digits.parse::<u64>().ok()
                })
                .filter(|n| *n >= 256);
            let Some(afford) = afford else { return false };
            // A little under: the estimate behind that figure moves as other requests settle.
            let limit = afford - afford / 10;
            self.afford = Some(limit);
            let field = if object.contains_key("max_completion_tokens") {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            if object.get(field).and_then(Value::as_u64) == Some(limit) {
                return false;
            }
            object.insert(field.to_string(), json!(limit));
            return true;
        }
        if !matches!(status, 400 | 404 | 422) {
            return false;
        }
        let mentions = |words: &[&str]| words.iter().any(|w| m.contains(w));
        if (object.contains_key("reasoning") || object.contains_key("reasoning_effort"))
            && mentions(&["reasoning", "thinking"])
        {
            object.remove("reasoning");
            object.remove("reasoning_effort");
            self.no_reasoning = true;
            return true;
        }
        // Still about reasoning, and it isn't the setting: it's what's being sent back.
        if !self.no_reasoning_echo && mentions(&["reasoning", "thinking"]) {
            let mut stripped = false;
            for message in object
                .get_mut("messages")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(fields) = message.as_object_mut() {
                    for field in ["reasoning_details", "reasoning", "reasoning_content"] {
                        stripped |= fields.remove(field).is_some();
                    }
                }
            }
            if stripped {
                self.no_reasoning_echo = true;
                return true;
            }
        }
        if object.contains_key("provider")
            && (status == 404 || mentions(&["no endpoints", "require_parameters"]))
        {
            object.remove("provider");
            self.no_routing = true;
            return true;
        }
        if object.contains_key("max_tokens") && m.contains("max_completion_tokens") {
            if let Some(limit) = object.remove("max_tokens") {
                object.insert("max_completion_tokens".to_string(), limit);
            }
            self.completion_field = true;
            return true;
        }
        if (object.contains_key("max_tokens") || object.contains_key("max_completion_tokens"))
            && mentions(&[
                "max_tokens",
                "max_completion_tokens",
                "maximum output",
                "output tokens",
            ])
        {
            object.remove("max_tokens");
            object.remove("max_completion_tokens");
            self.no_output_limit = true;
            return true;
        }
        if object.contains_key("stream_options") && m.contains("stream_options") {
            object.remove("stream_options");
            self.no_stream_options = true;
            return true;
        }
        if object.contains_key("session_id") && m.contains("session_id") {
            object.remove("session_id");
            self.no_session_id = true;
            return true;
        }
        if object.contains_key("prompt_cache_key") && m.contains("prompt_cache_key") {
            object.remove("prompt_cache_key");
            self.no_cache_key = true;
            return true;
        }
        // About the setting that keeps a summary or a report to words, not the tools themselves.
        if object.contains_key("tool_choice") && m.contains("tool_choice") {
            object.remove("tool_choice");
            return true;
        }
        if object.contains_key("tools") && failure::says_no_tools(&m) {
            object.remove("tools");
            object.remove("tool_choice");
            self.no_tools = true;
            return true;
        }
        false
    }
}

/// The reasoning setting for OpenRouter, as this model takes it: the nearest level it offers at or
/// above what was asked (`none` for Off, where it can be turned off), nothing for a model without
/// reasoning. With no facts about the model, the three levels every reasoning model takes.
fn reasoning_level(effort: Effort, caps: Option<&ModelCaps>) -> Option<String> {
    if let Some(caps) = caps {
        if caps.reasoning == Some(false) {
            return None;
        }
        if !caps.efforts.is_empty() {
            if effort == Effort::Off {
                return caps
                    .efforts
                    .iter()
                    .find(|e| *e == "none")
                    .map(|e| e.to_string());
            }
            const ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
            let rank = |name: &str| ORDER.iter().position(|o| *o == name);
            let want = rank(effort.as_str()).unwrap_or(2);
            let mut offered: Vec<(usize, &String)> = caps
                .efforts
                .iter()
                .filter_map(|e| rank(e).map(|r| (r, e)))
                .collect();
            offered.sort();
            return offered
                .iter()
                .find(|(r, _)| *r >= want)
                .or_else(|| offered.last())
                .map(|(_, e)| e.to_string());
        }
    }
    (effort != Effort::Off).then(|| {
        effort
            .clamp_to(&[Effort::Low, Effort::Medium, Effort::High])
            .as_str()
            .to_string()
    })
}

/// `text` as a content part the provider may cache up to.
fn cached_part(text: &str) -> Value {
    json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }])
}

/// The request for `req`, shaped for its provider and for what it's been found not to take.
fn openai_body(req: &TurnRequest<'_>, quirks: &Quirks) -> Value {
    let openrouter = req.provider.id == "openrouter";
    // Only these take explicit breakpoints; on the others they cost more than they save.
    let breakpoints =
        openrouter && (req.model.starts_with("anthropic/") || req.model.starts_with("qwen/"));
    let mut messages: Vec<Value> = Vec::with_capacity(req.history.len() + 1);
    messages.push(if breakpoints {
        json!({ "role": "system", "content": cached_part(req.system) })
    } else {
        json!({ "role": "system", "content": req.system })
    });
    for message in req.history {
        let role = message["role"].as_str().unwrap_or_default();
        // Some providers refuse an empty assistant message.
        if role == "assistant"
            && message.get("tool_calls").is_none()
            && message["content"]
                .as_str()
                .is_some_and(|c| c.trim().is_empty())
        {
            continue;
        }
        // And some refuse two user messages in a row, which a message left by a failed request,
        // or sent while a turn ran, can make.
        if role == "user" {
            if let (Some(text), Some(last)) = (message["content"].as_str(), messages.last_mut()) {
                if last["role"] == "user" {
                    if let Some(before) = last["content"].as_str() {
                        last["content"] = json!(format!("{before}\n\n{text}"));
                        continue;
                    }
                }
            }
        }
        let mut message = message.clone();
        if quirks.no_reasoning_echo {
            if let Some(fields) = message.as_object_mut() {
                for field in ["reasoning_details", "reasoning", "reasoning_content"] {
                    fields.remove(field);
                }
            }
        }
        // DeepSeek's own API refuses a tool call whose message has no `reasoning_content` while
        // thinking is on, even from before thinking was. Any text does.
        if req.provider.id == "deepseek"
            && role == "assistant"
            && message.get("tool_calls").is_some()
            && message.get("reasoning_content").is_none()
        {
            message["reasoning_content"] = json!(" ");
        }
        messages.push(message);
    }
    if breakpoints {
        if let Some(last) = messages.last_mut() {
            if let Some(text) = last["content"].as_str().map(str::to_string) {
                last["content"] = cached_part(&text);
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true
    });
    if !quirks.no_tools {
        body["tools"] = json!(req.tools);
        if req.no_tools {
            body["tool_choice"] = json!("none");
        }
    }
    let reports_usage =
        REPORTS_USAGE.contains(&req.provider.id) || direct_provider(req.provider.id).is_none();
    if reports_usage && !quirks.no_stream_options {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !req.provider.local && !quirks.no_reasoning {
        if openrouter {
            // OpenRouter's unified switch; a raw `reasoning_effort` only reaches providers
            // that take it themselves, others ignore or reject it.
            if let Some(level) = reasoning_level(req.effort, req.caps) {
                body["reasoning"] = json!({ "effort": level });
            }
        } else if req.effort != Effort::Off && req.caps.and_then(|c| c.reasoning) != Some(false) {
            let e = req
                .effort
                .clamp_to(&[Effort::Low, Effort::Medium, Effort::High]);
            body["reasoning_effort"] = json!(e.as_str());
        }
    }
    if let Some(limit) = req.max_output.filter(|_| !quirks.no_output_limit) {
        let limit = quirks.afford.map_or(limit, |a| limit.min(a));
        let field = if req.provider.id == "openai" || quirks.completion_field {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[field] = json!(limit);
    }
    if openrouter {
        if !quirks.no_session_id {
            body["session_id"] = json!(clip_chars(req.session_id, 256));
        }
        // Only routes that honour everything in the request: a route that drops `tools` or
        // `reasoning` without saying is how a model comes to look broken.
        if !quirks.no_routing {
            body["provider"] = json!({ "require_parameters": true });
        }
    }
    if req.provider.id == "openai" && !quirks.no_cache_key {
        body["prompt_cache_key"] = json!(clip_chars(req.session_id, 64));
    }
    body
}

fn openai_post(req: &TurnRequest<'_>, body: &Value) -> reqwest::RequestBuilder {
    let mut post = req
        .client
        .post(format!("{}/chat/completions", req.url))
        .json(body);
    if let Some(key) = req.key {
        post = post.bearer_auth(key);
    }
    if req.provider.id == "openrouter" {
        post = post
            .header("HTTP-Referer", "https://github.com/dokyit/Trek")
            .header("X-OpenRouter-Title", "Trek")
            .header("X-Title", "Trek");
    }
    post
}

/// OpenRouter's reasoning deltas put back together: a part arrives in pieces carrying its `index`,
/// and the whole is what must be echoed, unchanged in type and order.
fn merge_reasoning_details(parts: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for part in parts {
        let same = match part.get("index") {
            Some(index) => merged
                .iter()
                .rposition(|m| m.get("index") == Some(index) && m["type"] == part["type"]),
            None => merged
                .last()
                .filter(|m| m.get("index").is_none() && m["type"] == part["type"])
                .map(|_| merged.len() - 1),
        };
        let Some(at) = same else {
            merged.push(part);
            continue;
        };
        let target = &mut merged[at];
        let Some(fields) = part.as_object() else {
            continue;
        };
        for (key, value) in fields {
            match (key.as_str(), value) {
                ("text" | "summary" | "data", Value::String(piece)) => {
                    let joined = format!(
                        "{}{piece}",
                        target[key.as_str()].as_str().unwrap_or_default()
                    );
                    target[key.as_str()] = json!(joined);
                }
                (_, Value::Null) => {}
                _ => target[key.as_str()] = value.clone(),
            }
        }
    }
    merged
}

async fn openai_turn(
    req: &TurnRequest<'_>,
    quirks: &mut Quirks,
    partial: &mut String,
    events: &async_channel::Sender<AgentEvent>,
    commands: &async_channel::Receiver<Command>,
    kept: &mut VecDeque<Command>,
    shutdown: &mut bool,
) -> Result<TurnResponse> {
    let model = req.model;
    let mut body = openai_body(req, quirks);
    let mut tries = 0;
    let resp = loop {
        let resp = send_request(
            openai_post(req, &body),
            commands,
            kept,
            shutdown,
            HEADERS_WAIT,
        )
        .await?;
        let status = resp.status();
        if status.is_success() {
            break resp;
        }
        let headers = resp.headers().clone();
        let text = until_stopped(resp.text(), commands, kept, shutdown)
            .await?
            .unwrap_or_default();
        let message = error_message(&text);
        tries += 1;
        let had_tools = !quirks.no_tools;
        if tries < 6 && quirks.adapt(status.as_u16(), &message, &mut body) {
            if had_tools && quirks.no_tools {
                events
                    .send(AgentEvent::Notice(
                        "This model can't use tools, so it can only answer in words.".to_string(),
                    ))
                    .await?;
            }
            continue;
        }
        return Err(request_error(
            status,
            &headers,
            format!("{status}: {message}"),
        ));
    };
    let mut text = String::new();
    let mut tool_calls: Vec<AccumulatedToolCall> = Vec::new();
    let mut details: Vec<Value> = Vec::new();
    let mut plain_reasoning: Option<(&'static str, String)> = None;
    let mut usage = None;
    let mut terminal = false;
    let mut finish: Option<String> = None;
    let streamed: Result<()> = async {
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        let idle = tokio::time::sleep(STREAM_IDLE);
        tokio::pin!(idle);
        let mut ended = false;
        'stream: loop {
            tokio::select! {
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buf.extend_from_slice(&bytes);
                            let quiet = if terminal { AFTER_FINISH } else { STREAM_IDLE };
                            idle.as_mut().reset(tokio::time::Instant::now() + quiet);
                        }
                        Some(Err(e)) => {
                            return Err(RequestFailure::transient(format!(
                                "The connection dropped while the answer was coming: {e}"
                            )).into());
                        }
                        None => {
                            ended = true;
                            // A last event with no blank line after it is still an event.
                            if !buf.is_empty() {
                                buf.extend_from_slice(b"\n\n");
                            }
                        }
                    }
                }
                _ = &mut idle => {
                    if terminal {
                        break 'stream;
                    }
                    return Err(RequestFailure::transient(format!(
                        "The provider sent nothing for {} seconds",
                        STREAM_IDLE.as_secs()
                    )).into());
                }
                cmd = commands.recv() => keep_command(cmd, kept, shutdown)?,
            }
            for data in sse_events(&mut buf) {
                if data == "[DONE]" {
                    terminal = true;
                    break 'stream;
                }
                let Ok(v) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                // A mid-stream `{"error": …}` frame carries no choices: without this it is
                // skipped and the real reason dies as "connection closed early".
                let error = v.get("error").or_else(|| v["choices"][0].get("error"));
                if let Some(e) = error.filter(|e| !e.is_null()) {
                    return Err(failure::classify_stream_error(e).into());
                }
                usage = openai_usage(&v).or(usage);
                for choice in v["choices"].as_array().into_iter().flatten() {
                    if !choice["finish_reason"].is_null() {
                        terminal = true;
                        idle.as_mut()
                            .reset(tokio::time::Instant::now() + AFTER_FINISH);
                    }
                    if let Some(f) = choice["finish_reason"].as_str() {
                        finish = Some(match f {
                            "model_context_window_exceeded" => "length".to_string(),
                            other => other.to_string(),
                        });
                    }
                }
                let d = &v["choices"][0]["delta"];
                if let Some(parts) = d.get("reasoning_details").and_then(|r| r.as_array()) {
                    details.extend(parts.iter().cloned());
                }
                // Kept in the field it came in: that's the field its provider wants back.
                for field in ["reasoning_content", "reasoning"] {
                    if let Some(piece) = d[field].as_str().filter(|s| !s.is_empty()) {
                        match &mut plain_reasoning {
                            Some((_, all)) => all.push_str(piece),
                            None => plain_reasoning = Some((field, piece.to_string())),
                        }
                        break;
                    }
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
                        let mut idx = call
                            .get("index")
                            .and_then(|i| i.as_u64())
                            .map(|i| i as usize)
                            .unwrap_or(pos);
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
                            // Text, or for a few providers the object itself.
                            match f.get("arguments") {
                                Some(Value::String(a)) => tool_calls[idx].arguments.push_str(a),
                                Some(a @ Value::Object(_)) => {
                                    tool_calls[idx].arguments = a.to_string();
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            if ended {
                break;
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        partial.clone_from(&text);
        // What's on the screen is closed off, so a retry starts a message of its own rather than
        // running on from the middle of a sentence.
        if !text.is_empty() && !is_stop(&e) {
            events.send(AgentEvent::TextDone(text.clone())).await?;
        }
        return Err(e);
    }
    let failed_finish = match finish.as_deref() {
        Some("error") => Some(RequestFailure::transient("The provider reported an error")),
        Some("content_filter" | "safety" | "prohibited_content") => Some(RequestFailure {
            kind: FailureKind::Fatal,
            message: "The provider's content filter stopped this answer.".to_string(),
            retry_after: None,
            limit: None,
        }),
        _ if !terminal => Some(RequestFailure::transient(
            "The connection closed before the answer finished",
        )),
        _ => None,
    };
    if let Some(failed) = failed_finish {
        partial.clone_from(&text);
        if !text.is_empty() {
            events.send(AgentEvent::TextDone(text.clone())).await?;
        }
        return Err(failed.into());
    }
    let mut request_usage = None;
    if let Some((tokens, reported)) = usage {
        let cost = match reported {
            Some(usd) => Some(UsageCost::reported(usd)),
            None => trek_core::pricing::request(model, req.agent, &tokens, 0, false),
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

    // A slot opened by an index that was skipped holds nothing to run.
    tool_calls.retain(|c| !(c.name.is_empty() && c.arguments.trim().is_empty()));
    if req.no_tools {
        tool_calls.clear();
    }
    // No structured calls: some families write theirs as text, when their route's template doesn't
    // turn them into calls.
    if tool_calls.is_empty() && !req.no_tools && !text.is_empty() {
        let known: Vec<&str> = req
            .tools
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        let (found, cleaned) = repair::parse_text_calls(&text, &known);
        if !found.is_empty() {
            tool_calls = found
                .into_iter()
                .map(|(name, arguments)| AccumulatedToolCall {
                    id: String::new(),
                    name,
                    arguments,
                })
                .collect();
            text = cleaned;
        }
    }
    repair::normalize_ids(&mut tool_calls, &mut quirks.seen_ids, |i| {
        format!("call_{i}_{}", next_id())
    });

    events.send(AgentEvent::TextDone(text.clone())).await?;
    Ok(TurnResponse {
        text,
        content: None,
        tool_calls,
        reasoning_details: merge_reasoning_details(details),
        reasoning: plain_reasoning,
        finish,
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

    // OpenRouter's list is a large download.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
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
                    context: None,
                    caps: None,
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

/// What a provider's model list says a model can take. `None` when it says nothing, as most do.
fn model_caps(m: &Value) -> Option<ModelCaps> {
    let parameters = m["supported_parameters"].as_array();
    let takes = |name: &str| parameters.map(|p| p.iter().any(|v| v == name));
    let caps = ModelCaps {
        // OpenRouter's, then Anthropic's and Gemini's own lists.
        max_output: m["top_provider"]["max_completion_tokens"]
            .as_u64()
            .or_else(|| m["max_tokens"].as_u64())
            .or_else(|| m["outputTokenLimit"].as_u64())
            .filter(|n| *n > 0),
        tools: takes("tools"),
        reasoning: takes("reasoning"),
        efforts: m["reasoning"]["supported_efforts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        images: m["architecture"]["input_modalities"]
            .as_array()
            .map(|a| a.iter().any(|v| v == "image")),
    };
    (caps != ModelCaps::default()).then_some(caps)
}

pub(crate) fn parse_direct_models(v: &Value) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    let items = v["data"].as_array().or_else(|| v["models"].as_array());
    for m in items.into_iter().flatten() {
        let caps = model_caps(m);
        // Every request has tools in it, so a model that takes none could only fail.
        if caps.as_ref().and_then(|c| c.tools) == Some(false) {
            continue;
        }
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
                // OpenRouter and most OpenAI-style lists, Anthropic's, Gemini's.
                context: ["context_length", "max_input_tokens", "inputTokenLimit"]
                    .iter()
                    .find_map(|k| m[*k].as_u64())
                    .or_else(|| m["top_provider"]["context_length"].as_u64())
                    .filter(|n| *n > 0),
                caps,
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

/// Where `cache_direct_models` keeps a provider's list (none in an isolated Trek).
fn cached_direct_models_file(provider_id: &str) -> Option<std::path::PathBuf> {
    (!trek_core::paths::isolated()).then(|| {
        trek_core::paths::data_dir()
            .join("direct-models")
            .join(format!("{provider_id}.json"))
    })
}

pub fn cached_direct_models(provider_id: &str) -> Vec<ModelInfo> {
    cached_direct_models_file(provider_id)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// When the cached list was last written: a stat, for telling whether it changed without
/// reading it.
fn cached_direct_models_stamp(provider_id: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(cached_direct_models_file(provider_id)?)
        .and_then(|m| m.modified())
        .ok()
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

    /// One request as these tests want to make it: the parts that vary, the rest from defaults.
    #[allow(clippy::too_many_arguments)]
    async fn openai_turn(
        client: &reqwest::Client,
        agent: &AgentId,
        base_url: &str,
        key: Option<&str>,
        local: bool,
        _include_usage: bool,
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
        let id = match agent {
            AgentId::Direct(p) if p == "openai" => "openai",
            AgentId::Direct(p) if p == "openrouter" => "openrouter",
            AgentId::Direct(p) if p == "deepseek" => "deepseek",
            _ => "local",
        };
        let provider = DirectProvider {
            id,
            name: "test",
            base_url: "",
            wire: Wire::OpenAiChat,
            env_key: None,
            local,
        };
        super::openai_turn(
            &TurnRequest {
                client,
                agent,
                provider: &provider,
                url: base_url,
                key,
                model,
                effort,
                system,
                history,
                tools,
                no_tools: false,
                max_output: None,
                caps: None,
                session_id: "test",
            },
            &mut Quirks::default(),
            &mut String::new(),
            events,
            commands,
            kept,
            shutdown,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn anthropic_turn(
        client: &reqwest::Client,
        url: &str,
        key: &str,
        model: &str,
        effort: Effort,
        system: &str,
        history: &[Value],
        tools: &[Value],
        thread: &mut Thread,
        events: &async_channel::Sender<AgentEvent>,
        commands: &async_channel::Receiver<Command>,
        kept: &mut VecDeque<Command>,
        shutdown: &mut bool,
    ) -> Result<TurnResponse> {
        let agent = AgentId::Direct("anthropic".into());
        let provider = DirectProvider {
            id: "anthropic",
            name: "test",
            base_url: "",
            wire: Wire::Anthropic,
            env_key: None,
            local: false,
        };
        super::anthropic_turn(
            &TurnRequest {
                client,
                agent: &agent,
                provider: &provider,
                url,
                key: Some(key),
                model,
                effort,
                system,
                history,
                tools,
                no_tools: false,
                max_output: None,
                caps: None,
                session_id: "test",
            },
            thread,
            &mut String::new(),
            events,
            commands,
            kept,
            shutdown,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
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
        super::execute_tool(
            name,
            args_json,
            cwd,
            read_only,
            hand_holding,
            allowed_for_session,
            options,
            script,
            &mcp::McpHub::new(Vec::new()),
            &mut Vec::new(),
            events,
            commands,
            kept,
            shutdown,
        )
        .await
    }

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
        // A short wait is waited out, with the limit kept in case the tries run out.
        let e = request_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            "429 Too Many Requests: Rate limit exceeded".into(),
        );
        let failed = e.downcast_ref::<RequestFailure>().expect("a failure");
        assert!(failed.is_retryable());
        assert_eq!(failed.retry_after, Some(Duration::from_secs(30)));
        assert!(
            failed
                .limit
                .as_ref()
                .and_then(|l| l.resets_at)
                .is_some_and(|at| (at - trek_core::store::now_ms() - 30_000).abs() < 5_000)
        );
        // A long one is a usage limit, said at once.
        let mut far = reqwest::header::HeaderMap::new();
        far.insert("retry-after", "3600".parse().unwrap());
        let e = request_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &far,
            "429 Too Many Requests: Rate limit exceeded".into(),
        );
        assert!(e.downcast_ref::<crate::limits::LimitError>().is_some());
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
            transcript: None,
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
            &mut Thread::default(),
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
        let text = "I will check git status.\n<tool_call>\n<function=bash>\n{\"command\": \"git status\"} \n</tool_call>\n";
        let (calls, cleaned) = repair::parse_text_calls(text, &["bash"]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bash");
        assert!(calls[0].1.contains("git status"));
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

    /// `permitted_under` for `tool` on `target` with nobody to answer: what it says, and whether it
    /// asked.
    async fn rule_outcome(
        rules: &[PermissionRule],
        tool: &str,
        target: &str,
        level: HandHolding,
        allowed: &[&str],
    ) -> (Option<Permit>, Vec<AgentEvent>) {
        let (events, seen) = async_channel::unbounded();
        let (_keep, commands) = async_channel::unbounded();
        let mut allowed: HashSet<String> = allowed.iter().map(|a| a.to_string()).collect();
        let mut kept = VecDeque::new();
        let mut shutdown = false;
        let cwd = std::env::temp_dir().join("trek_rule_outcome");
        let run = permitted_under(
            rules,
            tool,
            target,
            &cwd,
            level,
            &mut allowed,
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        );
        // Asking waits for an answer nobody gives: the card is what's being looked for.
        let answer = tokio::time::timeout(Duration::from_millis(50), run)
            .await
            .ok()
            .map(|r| r.unwrap());
        drop(events);
        let mut out = vec![];
        while let Ok(e) = seen.recv().await {
            out.push(e);
        }
        (answer, out)
    }

    fn command_rule(
        pattern: &str,
        action: trek_core::permission_rules::RuleAction,
    ) -> PermissionRule {
        PermissionRule::new(RuleKind::Command, pattern, action)
    }

    #[tokio::test]
    async fn a_deny_rule_holds_at_every_level_and_tells_both_the_model_and_the_user() {
        use trek_core::permission_rules::RuleAction::Deny;
        for level in HandHolding::ALL {
            for line in ["rm x", "echo y; rm x", "FOO=1 \\rm x", "/bin/rm x", "$X y"] {
                let (answer, events) =
                    rule_outcome(&[command_rule("rm", Deny)], "bash", line, level, &[]).await;
                let Some(Permit::No(why)) = answer else {
                    panic!("{level:?} {line}: {answer:?}");
                };
                assert!(why.contains("Deny command `rm`"), "{why}");
                assert!(
                    matches!(&events[..], [AgentEvent::Notice(n)] if n.contains("Deny command `rm`")),
                    "{events:?}"
                );
            }
        }
        // "Allow for the session" doesn't get past it either.
        let (answer, _) = rule_outcome(
            &[command_rule("rm", Deny)],
            "bash",
            "rm x",
            HandHolding::Supervised,
            &["rm x"],
        )
        .await;
        assert!(matches!(answer, Some(Permit::No(_))));
    }

    #[tokio::test]
    async fn an_ask_rule_asks_at_full_access_and_past_a_session_allowance() {
        use trek_core::permission_rules::RuleAction::Ask;
        for (level, allowed) in [
            (HandHolding::FullAccess, vec![]),
            (HandHolding::Supervised, vec!["git push"]),
            (HandHolding::Auto, vec!["git push"]),
        ] {
            let (answer, events) = rule_outcome(
                &[command_rule("git push", Ask)],
                "bash",
                "git push",
                level,
                &allowed,
            )
            .await;
            assert_eq!(answer, None, "{level:?}: still waiting for the answer");
            assert!(
                matches!(
                    &events[..],
                    [AgentEvent::PermissionRequest { title, call: Some(c), .. }]
                        if title == "Run command" && c.target == "git push"
                ),
                "{events:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_allow_rule_skips_the_card_only_for_what_it_covers() {
        use trek_core::permission_rules::RuleAction::Allow;
        let rules = [command_rule("git push", Allow)];
        let (answer, events) = rule_outcome(
            &rules,
            "bash",
            "git push origin",
            HandHolding::Supervised,
            &[],
        )
        .await;
        assert_eq!(answer, Some(Permit::Go));
        assert!(events.is_empty(), "{events:?}");
        for line in ["git push; rm x", "git push $(rm x)", "git pull"] {
            let (answer, events) =
                rule_outcome(&rules, "bash", line, HandHolding::Supervised, &[]).await;
            assert_eq!(answer, None, "{line}");
            assert!(matches!(
                &events[..],
                [AgentEvent::PermissionRequest { .. }]
            ));
        }
    }

    #[tokio::test]
    async fn rules_read_the_other_tools_too() {
        use trek_core::permission_rules::RuleAction::{Allow, Deny};
        let (answer, _) = rule_outcome(
            &[PermissionRule::new(RuleKind::Edit, "*.lock", Deny)],
            "write_file",
            "Cargo.lock",
            HandHolding::FullAccess,
            &[],
        )
        .await;
        assert!(matches!(answer, Some(Permit::No(_))));
        let (answer, _) = rule_outcome(
            &[PermissionRule::new(RuleKind::Edit, "~/.ssh/**", Allow)],
            "edit_file",
            "src/a.rs",
            HandHolding::FullAccess,
            &[],
        )
        .await;
        assert_eq!(answer, Some(Permit::Go), "no rule: the level's");
        let (answer, _) = rule_outcome(
            &[PermissionRule::new(RuleKind::Fetch, "evil.com", Deny)],
            "web_fetch",
            "https://a.evil.com/x",
            HandHolding::FullAccess,
            &[],
        )
        .await;
        assert!(matches!(answer, Some(Permit::No(_))));
        let (answer, _) = rule_outcome(
            &[PermissionRule::new(RuleKind::Tool, "mcp__github__*", Deny)],
            "mcp",
            "github create_issue",
            HandHolding::FullAccess,
            &[],
        )
        .await;
        assert!(matches!(answer, Some(Permit::No(_))));
    }

    #[test]
    fn the_testers_level_column_is_what_the_session_does() {
        let cwd = std::env::temp_dir();
        let ask = |call: Call, level| level_would_ask(&call, &cwd, level);
        let ls = || Call::command("bash", "ls -la");
        let rm = || Call::command("bash", "rm x");
        assert!(ask(ls(), HandHolding::Supervised));
        assert!(!ask(ls(), HandHolding::Auto));
        assert!(ask(rm(), HandHolding::Auto));
        assert!(!ask(rm(), HandHolding::FullAccess));
        assert!(ask(Call::edit("Write", "a.rs"), HandHolding::Supervised));
        assert!(!ask(Call::edit("Write", "a.rs"), HandHolding::Auto));
        assert!(ask(
            Call::edit("Write", "/elsewhere/a.rs"),
            HandHolding::Auto
        ));
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
            &ThreadRequest::None,
        );
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "bash",
                "read_file",
                "write_file",
                "edit_file",
                "grep",
                "glob",
                "list_dir",
                "web_fetch"
            ]
        );
        assert_eq!(
            body["tools"][1]["input_schema"]["required"],
            json!(["path"])
        );
        assert!(
            body["tools"][0].get("function").is_none() && body["tools"][0].get("type").is_none()
        );
        // Cached: the tools and system prompt, and the conversation so far.
        assert_eq!(body["system"][0]["text"], "sys");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        let last = direct_tools().len() - 1;
        assert_eq!(body["tools"][last]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());
        let history = [
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "yes" }] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t", "content": "ok" }, { "type": "text", "text": "and" }] }),
        ];
        let body = anthropic_body(
            "claude-sonnet-4-5",
            Effort::Medium,
            "sys",
            &history,
            &[],
            &ThreadRequest::None,
        );
        assert_eq!(
            body["messages"][0]["content"], "hi",
            "only the last message"
        );
        assert_eq!(
            body["messages"][2]["content"][1]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(
            body["messages"][2]["content"][0]
                .get("cache_control")
                .is_none()
        );
        let body = anthropic_body(
            "claude-sonnet-4-5",
            Effort::Medium,
            "sys",
            &history[..1],
            &[],
            &ThreadRequest::None,
        );
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }])
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
            &mut Thread::default(),
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
            Self::start_configured(provider, hand_holding, responses, options, |c| {
                c.script = script
            })
            .await
        }

        /// `start_with`, the session's config changed by `tweak` first.
        async fn start_configured(
            provider: &'static str,
            hand_holding: HandHolding,
            responses: Vec<(u16, String)>,
            options: DirectOptions,
            tweak: impl FnOnce(&mut SessionConfig),
        ) -> TestSession {
            let (url, bodies) = script_server(responses).await;
            let cwd = std::env::temp_dir().join(format!("trek_test_direct_session_{}", next_id()));
            std::fs::create_dir_all(&cwd).unwrap();
            let mut config = SessionConfig {
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
                script: false,
                transcript: None,
            };
            tweak(&mut config);
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
    async fn a_message_sent_while_a_command_runs_reaches_the_next_request() {
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
        // It reaches the model on the very next request, after the tool's result.
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        assert_eq!(user_texts(&messages), ["one", "steer"]);
        assert_eq!(messages.last().unwrap()["content"], "steer");
        assert_eq!(s.bodies.lock().unwrap().len(), 2);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_failed_request_after_a_tool_keeps_the_history_whole() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                (400, "boom".into()),
                openai_tool_call("call_1", "read_file", json!({ "path": "missing.txt" })),
                (400, "boom".into()),
                openai_text("ok"),
            ],
        )
        .await;
        // A first request that's refused takes its message back out.
        s.prompt("zero").await;
        assert!(s.turn_complete().await.is_some_and(|e| e.contains("400")));
        // One that fails after a tool ran keeps the call and its result.
        s.prompt("one").await;
        assert!(s.turn_complete().await.is_some_and(|e| e.contains("400")));
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
        assert_eq!(
            s.body(0)["tools"].as_array().map(Vec::len),
            Some(direct_tools().len())
        );
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

    /// An Anthropic stream with the response `id` the thread goes on from.
    fn anthropic_stream(id: &str, events: &str) -> (u16, String) {
        (
            200,
            format!(
                "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"input_tokens\":5,\"output_tokens\":1}}}}}}\n\n{events}"
            ),
        )
    }

    const ANTHROPIC_TOOL_USE: &str = concat!(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"notes.txt\\\"}\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    const ANTHROPIC_TEXT: &str = concat!(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    fn thread_error(status: u16, code: &str) -> (u16, String) {
        (
            status,
            json!({ "type": "error", "error": { "type": "not_found_error", "message": "no thread", "details": { "error_code": code } } })
                .to_string(),
        )
    }
    fn threaded() -> DirectOptions {
        DirectOptions {
            message_threads: Some(true),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn anthropic_tool_rounds_and_prompts_continue_the_thread() {
        let s = TestSession::start_with(
            "anthropic",
            HandHolding::FullAccess,
            vec![
                anthropic_stream("msg_1", ANTHROPIC_TOOL_USE),
                anthropic_stream("msg_2", ANTHROPIC_TEXT),
                anthropic_stream("msg_3", ANTHROPIC_TEXT),
            ],
            threaded(),
            false,
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("read it").await;
        s.tool_finished().await;
        assert_eq!(s.turn_complete().await, None);
        // The first request starts the thread with everything; the top-level cache_control
        // marks the last message, so no block carries a breakpoint of its own.
        let first = s.body(0);
        assert_eq!(first["thread"], json!({ "type": "create" }));
        assert_eq!(first["cache_control"]["type"], "ephemeral");
        assert_eq!(
            first["system"][0]["text"].as_str().map(|t| t.is_empty()),
            Some(false)
        );
        assert_eq!(
            first["tools"].as_array().map(|t| t.len()),
            Some(direct_tools().len())
        );
        assert_eq!(first["messages"].as_array().map(|m| m.len()), Some(1));
        assert!(first["messages"][0].to_string().contains("read it"));
        assert!(!first["messages"][0].to_string().contains("cache_control"));
        // The tool round goes on from the stored response, with the results only.
        let second = s.body(1);
        assert_eq!(
            second["thread"],
            json!({ "type": "continue", "previous_message_id": "msg_1" })
        );
        assert!(
            second.get("system").is_none() && second.get("tools").is_none(),
            "{second}"
        );
        assert_eq!(second["cache_control"]["type"], "ephemeral");
        let messages = second["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert_eq!(messages[0]["content"][0]["tool_use_id"], "toolu_1");
        // The fixed fields stay as they were.
        assert_eq!(second["model"], first["model"]);
        assert_eq!(second["thinking"], first["thinking"]);
        assert_eq!(second["output_config"], first["output_config"]);
        // So does the next prompt, within the thread's five minutes.
        s.prompt("and again").await;
        assert_eq!(s.turn_complete().await, None);
        let third = s.body(2);
        assert_eq!(
            third["thread"],
            json!({ "type": "continue", "previous_message_id": "msg_2" })
        );
        let messages = third["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert_eq!(messages[0]["role"], "user");
        assert!(messages[0].to_string().contains("and again"));
        s.finish().await;
    }

    #[tokio::test]
    async fn a_lost_thread_is_replayed_as_a_new_one() {
        let s = TestSession::start_with(
            "anthropic",
            HandHolding::FullAccess,
            vec![
                anthropic_stream("msg_1", ANTHROPIC_TOOL_USE),
                thread_error(404, "thread_not_found"),
                anthropic_stream("msg_2", ANTHROPIC_TEXT),
                anthropic_stream("msg_3", ANTHROPIC_TEXT),
            ],
            threaded(),
            false,
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("read it").await;
        s.tool_finished().await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.body(1)["thread"]["type"], "continue");
        let replay = s.body(2);
        assert_eq!(replay["thread"], json!({ "type": "create" }));
        assert!(replay.get("system").is_some() && replay.get("tools").is_some());
        assert_eq!(replay["messages"].as_array().map(|m| m.len()), Some(3));
        assert_tool_calls_answered(replay["messages"].as_array().unwrap());
        // The new thread goes on from the replay's response.
        s.prompt("more").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(
            s.body(3)["thread"],
            json!({ "type": "continue", "previous_message_id": "msg_2" })
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn a_fallback_models_answer_is_not_continued_from() {
        let fallen_back = ANTHROPIC_TOOL_USE.replace(
            "\"usage\":{\"output_tokens\":3}",
            "\"usage\":{\"output_tokens\":3,\"iterations\":[{\"type\":\"fallback_message\",\"model\":\"other\"}]}",
        );
        assert_ne!(fallen_back, ANTHROPIC_TOOL_USE);
        let s = TestSession::start_with(
            "anthropic",
            HandHolding::FullAccess,
            vec![
                anthropic_stream("msg_1", &fallen_back),
                anthropic_stream("msg_2", ANTHROPIC_TEXT),
            ],
            threaded(),
            false,
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("read it").await;
        s.tool_finished().await;
        assert_eq!(s.turn_complete().await, None);
        let second = s.body(1);
        assert_eq!(second["thread"], json!({ "type": "create" }));
        assert_eq!(second["messages"].as_array().map(|m| m.len()), Some(3));
        s.finish().await;
    }

    #[tokio::test]
    async fn a_refused_thread_is_left_out_for_the_session() {
        let s = TestSession::start_with(
            "anthropic",
            HandHolding::FullAccess,
            vec![
                thread_error(400, "thread_unsupported_request"),
                anthropic_stream("msg_1", ANTHROPIC_TEXT),
                anthropic_stream("msg_2", ANTHROPIC_TEXT),
            ],
            threaded(),
            false,
        )
        .await;
        s.prompt("hi").await;
        assert_eq!(s.turn_complete().await, None);
        s.prompt("again").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.body(0)["thread"]["type"], "create");
        for n in 1..3 {
            let body = s.body(n);
            assert!(body.get("thread").is_none(), "{body}");
            assert!(body.get("system").is_some());
            // Without a thread, the last message block carries the cache breakpoint.
            assert!(body.get("cache_control").is_none());
        }
        assert_eq!(s.body(2)["messages"].as_array().map(|m| m.len()), Some(3));
        s.finish().await;
    }

    #[test]
    fn a_thread_goes_on_only_while_the_server_holds_the_same_conversation() {
        let history = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "yes" }] }),
            json!({ "role": "user", "content": "more" }),
        ];
        let mut thread = Thread::default();
        let now = Instant::now();
        assert_eq!(thread.request(1, &history, now), ThreadRequest::Create);
        thread.stored("msg_1".into(), 1, &history[..1], &history[1]);
        let goes_on = ThreadRequest::Continue {
            previous: "msg_1".into(),
            held: 2,
        };
        assert_eq!(thread.request(1, &history, now), goes_on);
        // Nothing new to send, a changed fixed field, an edited message, or time: a new thread.
        assert_eq!(thread.request(1, &history[..2], now), ThreadRequest::Create);
        assert_eq!(thread.request(2, &history, now), ThreadRequest::Create);
        let mut edited = history.clone();
        edited[0] = json!({ "role": "user", "content": "hello" });
        assert_eq!(thread.request(1, &edited, now), ThreadRequest::Create);
        assert_eq!(
            thread.request(1, &history, now + THREAD_KEEPS + Duration::from_secs(1)),
            ThreadRequest::Create
        );
        thread.forget();
        assert_eq!(thread.request(1, &history, now), ThreadRequest::Create);
        thread.off = true;
        assert_eq!(thread.request(1, &history, now), ThreadRequest::None);
    }

    #[test]
    fn thread_requests_carry_only_whats_new() {
        let history = [
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "yes" }] }),
            json!({ "role": "user", "content": "more" }),
        ];
        let create = anthropic_body(
            "claude-sonnet-4-5",
            Effort::Medium,
            "sys",
            &history,
            &direct_tools(),
            &ThreadRequest::Create,
        );
        assert_eq!(create["thread"], json!({ "type": "create" }));
        assert_eq!(create["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(create["messages"].as_array().map(|m| m.len()), Some(3));
        assert_eq!(
            create["messages"][2]["content"], "more",
            "no breakpoint of its own"
        );
        assert_eq!(create["system"][0]["cache_control"]["type"], "ephemeral");
        let last = direct_tools().len() - 1;
        assert_eq!(create["tools"][last]["cache_control"]["type"], "ephemeral");
        let go_on = anthropic_body(
            "claude-sonnet-4-5",
            Effort::Medium,
            "sys",
            &history,
            &direct_tools(),
            &ThreadRequest::Continue {
                previous: "msg_9".into(),
                held: 2,
            },
        );
        assert_eq!(
            go_on["thread"],
            json!({ "type": "continue", "previous_message_id": "msg_9" })
        );
        assert_eq!(
            go_on["messages"],
            json!([{ "role": "user", "content": "more" }])
        );
        assert!(go_on.get("system").is_none() && go_on.get("tools").is_none());
        assert_eq!(go_on["cache_control"], json!({ "type": "ephemeral" }));
        assert_eq!(go_on["output_config"], create["output_config"]);
        assert_eq!(go_on["fallbacks"], create["fallbacks"]);
    }

    #[tokio::test]
    async fn the_round_limit_says_why_and_asks_for_a_report() {
        let mut script: Vec<(u16, String)> = (0..5)
            .map(|i| {
                openai_tool_call(
                    &format!("call_{i}"),
                    "bash",
                    json!({ "command": format!("echo {i}") }),
                )
            })
            .collect();
        script.push(openai_text("Did five of them. The rest is left."));
        let s = TestSession::start_configured(
            "ollama",
            HandHolding::FullAccess,
            script,
            DirectOptions {
                max_turns: 5,
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("keep going").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("5 tool-call rounds"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        // One more request, for the report: tools off, and the history still whole.
        assert_eq!(s.bodies.lock().unwrap().len(), 6);
        let last = s.body(5);
        assert_eq!(last["tool_choice"], "none");
        let messages = last["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        assert!(
            user_texts(&messages)
                .last()
                .unwrap()
                .contains("limit of tool rounds")
        );
        s.finish().await;
    }

    /// A message sent while a card is up is the answer to it.
    #[tokio::test]
    async fn a_message_while_a_card_is_up_declines_it_and_is_kept() {
        let (tx, commands) = async_channel::unbounded();
        let (events, rx) = async_channel::unbounded();
        let task = tokio::spawn(async move {
            let (mut kept, mut shutdown) = (VecDeque::new(), false);
            let decision = ask_permission(
                "Run command",
                "ls",
                None,
                &events,
                &commands,
                &mut kept,
                &mut shutdown,
            )
            .await;
            (decision, kept)
        });
        let Ok(AgentEvent::PermissionRequest { .. }) = rx.recv().await else {
            panic!("no permission request")
        };
        tx.send(Command::Prompt {
            text: "no, use git status".into(),
            images: vec![],
        })
        .await
        .unwrap();
        let (decision, kept) = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the card never closed")
            .unwrap();
        assert_eq!(decision.unwrap(), Decision::Deny);
        assert!(
            matches!(kept.front(), Some(Command::Prompt { text, .. }) if text == "no, use git status")
        );
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
                None,
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
        let (out, ok) = run_read_file(
            &LocalExecutor,
            "a.txt",
            Some(50),
            Some(10),
            &dir,
            &mut waiting(),
        )
        .await
        .unwrap();
        assert!(!ok);
        assert!(out.contains("before start line 50"), "{out}");
        let (_, ok) = run_read_file(
            &LocalExecutor,
            "a.txt",
            Some(5),
            Some(0),
            &dir,
            &mut waiting(),
        )
        .await
        .unwrap();
        assert!(!ok);
        let (out, ok) = run_read_file(
            &LocalExecutor,
            "a.txt",
            Some(10),
            Some(10),
            &dir,
            &mut waiting(),
        )
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
        assert!(out.contains("bytes cut here"), "{}", &out[..100]);
        assert!(out.starts_with("a✓✓"));
        assert!(out.ends_with("✓"));
        assert!(out.len() < MAX_OUT + 300);
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
                "partial\n[Command timed out after 7 seconds and was stopped. If it needs longer, run it again with a larger timeout_s (up to 3600), or with background set to true.]",
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
        assert!(!ok && out.contains("'script' does not exist"), "{out}");

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
    /// OpenRouter's `cost` when given. Its tests run a real shell, so unix only, as they are.
    #[cfg(unix)]
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

    /// `response`, reporting `prompt` tokens in and 5 out.
    fn using(prompt: u64, (status, body): (u16, String)) -> (u16, String) {
        let usage = json!({ "prompt_tokens": prompt, "completion_tokens": 5 });
        let chunk = format!("data: {}\n\n", json!({ "choices": [], "usage": usage }));
        (
            status,
            body.replace("data: [DONE]", &format!("{chunk}data: [DONE]")),
        )
    }

    fn tool_names(body: &Value) -> Vec<String> {
        body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                t["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[cfg(unix)] // used by the continuation tests, which are unix-only
    /// A transcript log like the one Trek keeps, in a folder of its own.
    fn transcript_file(text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek_test_direct_log_{}", next_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thread.log");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_full_context_goes_on_fresh_with_the_transcript_to_search() {
        let log = transcript_file(
            "### #1 L1 turn 1 2026-10-31 10:00:00 tool Run command (failed)\nargs: cargo build\nresult:\nerror[E0599]: no method named `frobnicate` found for struct `Parser`\n\n",
        );
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                using(
                    18_000,
                    openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                ),
                using(
                    3_000,
                    openai_tool_call("call_2", "transcript_search", json!({ "query": "E0599" })),
                ),
                using(3_200, openai_text("Found it: frobnicate.")),
            ],
            DirectOptions {
                context_window: Some(20_000),
                ..Default::default()
            },
            |c| c.transcript = Some(log.clone()),
        )
        .await;
        s.prompt("fix the build").await;
        let first = s
            .until(|e| match e {
                AgentEvent::Context { used, window } => Some((*used, *window)),
                _ => None,
            })
            .await;
        assert_eq!(
            first,
            (18_005, 20_000),
            "the provider's count against the window"
        );
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .await;
        assert!(
            notice.contains("% full, so this answer goes on in a fresh one: the earlier conversation stays searchable"),
            "{notice}"
        );
        assert_eq!(s.turn_complete().await, None);

        // The first request: the plain prompt, the tools.
        assert_eq!(
            tool_names(&s.body(0))[..4],
            ["bash", "read_file", "write_file", "edit_file"]
        );
        // The second starts afresh: system prompt and orientation, then a line to carry on.
        let fresh = s.body(1);
        let messages = fresh["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "{messages:#?}");
        let system = messages[0]["content"].as_str().unwrap();
        assert!(
            system.starts_with("You are Trek"),
            "the system prompt stays"
        );
        assert!(
            system.contains("<fresh-context>")
                && system.contains(&format!("\"{}\" (5 lines)", log.display()))
                && system.contains("transcript_search")
                && system.contains("The request you're answering:\nfix the build")
                && system.contains("- bash echo hi (ok): hi"),
            "{system}"
        );
        assert_eq!(messages[1]["content"], CONTINUE_MESSAGE);
        assert_eq!(
            tool_names(&fresh),
            [
                "bash",
                "read_file",
                "write_file",
                "edit_file",
                "grep",
                "glob",
                "list_dir",
                "web_fetch",
                "transcript_search",
                "transcript_read"
            ]
        );
        // The search reads the log.
        let found = s.body(2)["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let found = found["content"].as_str().unwrap();
        assert!(
            found.contains("1 matching line") && found.contains("no method named `frobnicate`"),
            "{found}"
        );
        s.finish().await;
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_message_that_would_overfill_the_context_starts_a_fresh_one() {
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                using(18_000, openai_text("Done with the first.")),
                using(500, openai_text("On it.")),
            ],
            DirectOptions {
                context_window: Some(20_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("first").await;
        assert_eq!(s.turn_complete().await, None);
        s.prompt("second").await;
        assert_eq!(s.turn_complete().await, None);
        let fresh = s.body(1);
        let messages = fresh["messages"].as_array().unwrap();
        assert_eq!(
            user_texts(messages),
            ["second"],
            "the message itself carries on"
        );
        let system = messages[0]["content"].as_str().unwrap();
        assert!(
            system.contains("The session's first message:\nfirst")
                && system.contains("The request you're answering:\nsecond")
                && system.contains("Your last words:\nDone with the first.")
                && system.contains("Nothing from before is kept where you can search it"),
            "{system}"
        );
        assert_eq!(
            tool_names(&fresh).len(),
            direct_tools().len(),
            "no transcript to search"
        );
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_session_from_a_recap_can_search_the_transcript_from_the_start() {
        let log = transcript_file("### #1 L1 turn 1 user\nhello\n\n");
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![openai_text("Hi again.")],
            DirectOptions::default(),
            |c| {
                c.transcript = Some(log.clone());
                c.recap = Some("User: hello".into());
            },
        )
        .await;
        s.prompt("again").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(tool_names(&s.body(0)).len(), direct_tools().len() + 2);
        s.finish().await;
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    /// The estimate of one `Value`, as it was counted before the parts were read in place.
    fn estimate_tokens(v: &Value) -> u64 {
        (estimate_chars(v) / 4) as u64
    }

    #[test]
    fn context_estimates_count_text_and_images_apart() {
        assert_eq!(estimate_tokens(&json!("x".repeat(400))), 100);
        // Counted in place, the same as counted as one array.
        let system = "You are a helpful agent. ".repeat(40);
        let parts = vec![
            json!({ "role": "user", "content": "x".repeat(7_000) }),
            json!({ "name": "bash", "parameters": { "type": "object", "n": 3, "ok": true } }),
            json!(null),
        ];
        assert_eq!(
            estimate_tokens_of(&system, &parts),
            estimate_tokens(&json!([system, parts]))
        );
        assert_eq!(
            estimate_tokens_of("", &parts[1..]),
            estimate_tokens(&Value::Array(parts[1..].to_vec()))
        );
        assert_eq!(
            serde_json::to_string(&parts[..]).unwrap(),
            Value::Array(parts.to_vec()).to_string()
        );
        let image = json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", "A".repeat(2_000_000)) } });
        assert!(
            estimate_tokens(&image) < 2_000,
            "a picture's worth, not its bytes"
        );
        assert_eq!(
            trek_core::catalog::context_window("deepseek/deepseek-v4-flash"),
            128_000
        );
        assert_eq!(
            trek_core::catalog::context_window("claude-opus-5-5"),
            200_000
        );
        let models = parse_direct_models(&json!({ "data": [
            { "id": "deepseek/deepseek-v4-flash", "context_length": 1_048_576 },
            { "id": "claude-x", "max_input_tokens": 200_000 },
            { "id": "plain" },
        ] }));
        let windows: Vec<Option<u64>> = models.iter().map(|m| m.context).collect();
        assert_eq!(windows, [Some(1_048_576), Some(200_000), None]);
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
        assert_eq!(
            s.body(0)["tools"].as_array().unwrap().len(),
            direct_tools().len()
        );
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
        assert_eq!(tools.len(), direct_tools().len() + 1);
        assert_eq!(tools.last().unwrap()["function"]["name"], "script");
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
        // On, the script is offered beside the others.
        assert_eq!(
            s.body(0)["tools"].as_array().unwrap().len(),
            direct_tools().len() + 1
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn a_scripts_commands_ask_like_the_models_own() {
        let s = TestSession::start_with(
            "ollama",
            HandHolding::Supervised,
            vec![
                openai_tool_call(
                    "call_1",
                    "script",
                    json!({ "code": "await bash('  touch ran.txt  '); print('ran')" }),
                ),
                openai_text("done"),
            ],
            DirectOptions::default(),
            true,
        )
        .await;
        s.prompt("go").await;
        let (request_id, title, detail) = s
            .until(|e| match e {
                AgentEvent::PermissionRequest {
                    request_id,
                    title,
                    detail,
                    ..
                } => Some((request_id.clone(), title.clone(), detail.clone())),
                _ => None,
            })
            .await;
        assert_eq!(title, "Run command");
        // Trimmed, as the model's own command is, so a session allowance would match.
        assert_eq!(detail, "touch ran.txt");
        s.send(Command::Respond {
            request_id,
            decision: Decision::Deny,
        })
        .await;
        let (out, ok) = s.tool_finished().await;
        assert!(!ok && out.contains("Permission denied by user"), "{out}");
        assert!(!s.cwd.join("ran.txt").exists());
        s.finish().await;
    }

    #[test]
    fn a_script_shows_its_description() {
        let args = json!({ "code": "\n  const x = 1;\nprint(x)", "description": "Count notes" });
        assert_eq!(
            tool_display("script", &args.to_string()),
            ("Run script", "Count notes".to_string())
        );
        let args = json!({ "code": "\n  const x = 1;\nprint(x)" });
        assert_eq!(
            tool_display("script", &args.to_string()),
            ("Run script", "const x = 1;".to_string())
        );
    }

    /// A `Waiting` no Stop ever reaches, for calling the file tools directly.
    fn waiting() -> Waiting<'static> {
        let (tx, rx) = async_channel::unbounded::<Command>();
        std::mem::forget(tx);
        Waiting {
            commands: Box::leak(Box::new(rx)),
            kept: Box::leak(Box::new(VecDeque::new())),
            shutdown: Box::leak(Box::new(false)),
        }
    }

    #[tokio::test]
    async fn a_stop_ends_a_read_the_sandbox_never_answers() {
        let (out, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (_to_exec, replies) = tokio::io::duplex(1 << 16);
        let options = DirectOptions {
            executor: Arc::new(executor::StdioExecutor::new(
                out,
                replies,
                Duration::from_secs(600),
            )),
            ..Default::default()
        };
        let (events, _rx) = async_channel::unbounded();
        let (tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown, mut allowed) = (VecDeque::new(), false, HashSet::new());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            tx.send(Command::Interrupt).await.unwrap();
            // Kept open: a closed channel would read as a shutdown.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let started = Instant::now();
        let e = execute_tool(
            "read_file",
            &json!({ "path": "a.txt" }).to_string(),
            Path::new("/app"),
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
        .unwrap_err();
        assert!(e.to_string().contains("Interrupted"), "{e:#}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // The peer is told it needn't answer.
        let read: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        let cancel: Value = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        assert_eq!(read["type"], "read");
        assert_eq!(cancel, json!({ "type": "cancel", "id": read["id"] }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_script_stops_at_the_sessions_deadline() {
        let log = Arc::new(Log::default());
        let options = DirectOptions {
            deadline: Some(Instant::now() + Duration::from_millis(1500)),
            observer: Some(log.clone()),
            ..Default::default()
        };
        let (events, _rx) = async_channel::unbounded();
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown, mut allowed) = (VecDeque::new(), false, HashSet::new());
        let code = "for (let i = 0; i < 100; i++) { await bash('sleep 0.2'); } print('all done')";
        let started = Instant::now();
        let (out, ok, _) = execute_tool(
            "script",
            &json!({ "code": code }).to_string(),
            &std::env::temp_dir(),
            false,
            HandHolding::FullAccess,
            &mut allowed,
            &options,
            true,
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "{out}");
        assert!(!ok && !out.contains("all done"), "{out}");
        let lines = log.lines();
        let script = lines.iter().find(|l| l.starts_with("script ")).unwrap();
        // Well short of a hundred calls.
        let calls: u32 = script.split(' ').nth(1).unwrap().parse().unwrap();
        assert!(calls < 20, "{script}");
    }

    #[tokio::test]
    async fn a_gone_sandbox_ends_a_script_and_its_turn() {
        let memory = Arc::new(MemoryExecutor::default());
        let options = DirectOptions {
            executor: memory.clone(),
            ..Default::default()
        };
        let (events, _rx) = async_channel::unbounded();
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown, mut allowed) = (VecDeque::new(), false, HashSet::new());
        // A program that would swallow every failure and go on.
        let code = "for (let i = 0; i < 500; i++) { try { await bash('gone'); } catch (e) {} }";
        let e = execute_tool(
            "script",
            &json!({ "code": code }).to_string(),
            Path::new("/sandbox/app"),
            false,
            HandHolding::FullAccess,
            &mut allowed,
            &options,
            true,
            &events,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap_err();
        assert!(e.downcast_ref::<ExecutorGone>().is_some(), "{e:#}");
        assert_eq!(memory.ran.lock().unwrap().len(), 1);
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

    fn anthropic_tool(id: &str, name: &str, input: Value) -> String {
        format!(
            "data: {}\n\ndata: {}\n\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}}}}\n\ndata: {{\"type\":\"message_stop\"}}\n\n",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": input.to_string() } }),
        )
    }

    /// A call whose arguments are `raw` exactly, as the stream gave them.
    fn raw_tool_call(id: &str, name: &str, raw: &str, finish: &str) -> (u16, String) {
        let call = json!({ "index": 0, "id": id, "type": "function", "function": { "name": name, "arguments": raw } });
        (
            200,
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({ "choices": [{ "delta": { "tool_calls": [call] }, "finish_reason": finish }] })
            ),
        )
    }

    fn finish_with(text: &str, finish: &str) -> (u16, String) {
        (
            200,
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({ "choices": [{ "delta": { "content": text }, "finish_reason": finish }] })
            ),
        )
    }

    #[tokio::test]
    async fn a_server_error_is_tried_again_until_it_works() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                (500, "boom".into()),
                (503, "busy".into()),
                openai_text("worked"),
            ],
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 3);
        // The same request each time.
        assert_eq!(s.body(0)["messages"], s.body(2)["messages"]);
        s.finish().await;
    }

    #[tokio::test]
    async fn an_error_inside_the_stream_is_tried_again() {
        let s = TestSession::start(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                (
                    200,
                    "data: {\"error\":{\"code\":502,\"message\":\"Provider returned error\"},\"choices\":[{\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n".into(),
                ),
                finish_with("partial", "error"),
                (200, "data: {\"choices\":[{\"delta\":{\"content\":\"cut\"}}]}\n\n".into()),
                openai_text("worked"),
            ],
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 4);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_is_given_up_on_and_tried_again() {
        // Headers, then silence on an open connection.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let held = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let keep = held.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            tokio::io::AsyncWriteExt::write_all(
                &mut socket,
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"hm\"}}]}\n\n",
            )
            .await
            .unwrap();
            keep.lock().unwrap().push(socket);
        });
        let client = reqwest::Client::new();
        let (_tx, commands) = async_channel::unbounded();
        let (events, _rx) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let e = openai_turn(
            &client,
            &AgentId::Direct("local".into()),
            &format!("http://{addr}"),
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
        let failed = e.downcast_ref::<RequestFailure>().expect("a failure");
        assert!(failed.is_retryable(), "{failed}");
        assert!(failed.message.contains("sent nothing"), "{failed}");
    }

    #[tokio::test]
    async fn a_400_about_something_else_keeps_the_tools_and_fails() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![(
                400,
                "{\"error\":{\"message\":\"messages: bad history\"}}".into(),
            )],
        )
        .await;
        s.prompt("hello").await;
        let error = s.turn_complete().await.unwrap();
        assert!(error.contains("bad history"), "{error}");
        assert_eq!(s.bodies.lock().unwrap().len(), 1);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_model_without_tools_says_so_and_the_tools_go() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                (
                    400,
                    "{\"error\":{\"message\":\"registry.ollama.ai/x does not support tools\"}}"
                        .into(),
                ),
                openai_text("words only"),
                openai_text("still words"),
            ],
        )
        .await;
        s.prompt("hello").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("can't use tools"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        assert!(s.body(1).get("tools").is_none());
        // Remembered: no refused request per round.
        s.prompt("again").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 3);
        assert!(s.body(2).get("tools").is_none());
        s.finish().await;
    }

    #[tokio::test]
    async fn a_full_context_refused_by_the_provider_goes_on_fresh() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                (400, "{\"error\":{\"message\":\"This model's maximum context length is 8192 tokens\"}}".into()),
                openai_text("carried on"),
            ],
        )
        .await;
        s.prompt("start").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 3);
        let fresh = s.body(2);
        let messages = fresh["messages"].as_array().unwrap();
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("<fresh-context>")
        );
        assert_eq!(messages.len(), 2, "{messages:#?}");
        s.finish().await;
    }

    #[tokio::test]
    async fn a_call_cut_off_by_the_limit_is_not_run() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                raw_tool_call(
                    "call_1",
                    "write_file",
                    r#"{"path":"cut.txt","content":"half of a fi"#,
                    "length",
                ),
                openai_text("smaller then"),
            ],
        )
        .await;
        s.prompt("write it").await;
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("cut off"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert!(!s.cwd.join("cut.txt").exists());
        // History never echoes what a provider couldn't parse.
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        let call = &messages[2]["tool_calls"][0]["function"];
        assert_eq!(call["arguments"], "{}");
        s.finish().await;
    }

    #[tokio::test]
    async fn an_answer_cut_off_by_the_limit_is_asked_to_go_on() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![finish_with("first ha", "length"), openai_text("lf.")],
        )
        .await;
        s.prompt("tell me").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_eq!(messages[2]["content"], "first ha");
        assert_eq!(messages[3]["content"], CUT_OFF_PROMPT);
        s.finish().await;
    }

    #[tokio::test]
    async fn an_empty_answer_is_asked_for_again_then_reported() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                finish_with("", "stop"),
                openai_text("there"),
                finish_with("", "stop"),
                finish_with("", "stop"),
                finish_with("", "stop"),
            ],
        )
        .await;
        s.prompt("one").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 2);
        s.prompt("two").await;
        let error = s.turn_complete().await.unwrap();
        assert!(error.contains("empty answer"), "{error}");
        s.finish().await;
    }

    #[tokio::test]
    async fn arguments_with_a_second_value_after_them_still_run() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                raw_tool_call(
                    "call_1",
                    "read_file",
                    r#"{"path":"notes.txt"}{}"#,
                    "tool_calls",
                ),
                openai_text("read"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("read").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("hello from notes"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"notes.txt"}"#
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn arguments_that_arent_json_never_become_a_command() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                raw_tool_call("call_1", "bash", "touch owned}", "tool_calls"),
                openai_text("sorry"),
            ],
        )
        .await;
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("aren't valid JSON"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert!(!s.cwd.join("owned").exists());
        s.finish().await;
    }

    #[tokio::test]
    async fn a_missing_field_is_named_and_nothing_is_written() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                raw_tool_call(
                    "call_1",
                    "write_file",
                    r#"{"path":"keep.txt"}"#,
                    "tool_calls",
                ),
                raw_tool_call(
                    "call_2",
                    "edit_file",
                    r#"{"path":"keep.txt","old_str":"precious"}"#,
                    "tool_calls",
                ),
                openai_text("ok"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("keep.txt"), "precious").unwrap();
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("missing content"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("missing new_str"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(
            std::fs::read_to_string(s.cwd.join("keep.txt")).unwrap(),
            "precious"
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn other_harnesses_names_for_tools_and_fields_work() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                raw_tool_call(
                    "call_1",
                    "Read",
                    r#"{"file_path":"notes.txt","start_line":"1"}"#,
                    "tool_calls",
                ),
                raw_tool_call("call_2", "launch_rocket", "{}", "tool_calls"),
                openai_text("done"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("hello from notes"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(
            !ok && output.contains("does not exist") && output.contains("read_file"),
            "{output}"
        );
        assert_eq!(s.turn_complete().await, None);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_call_written_as_text_runs() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_text("Looking.\n<function=read_file><parameter=path>notes.txt</parameter></function>"),
                openai_text("done"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("hello from notes"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_eq!(messages[2]["content"], "Looking.");
        assert_tool_calls_answered(&messages);
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_command_says_its_exit_code() {
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let run = |command: &'static str, kept: &mut VecDeque<Command>, shutdown: &mut bool| {
            let commands = commands.clone();
            let mut kept_here = std::mem::take(kept);
            let mut shutdown_here = *shutdown;
            async move {
                let out = run_bash_command(
                    &LocalExecutor,
                    command,
                    Path::new("."),
                    None,
                    &commands,
                    &mut kept_here,
                    &mut shutdown_here,
                )
                .await
                .unwrap();
                out
            }
        };
        let (out, ok) = run("exit 3", &mut kept, &mut shutdown).await;
        assert!(!ok);
        assert_eq!(out, "[exit code 3]");
        let (out, ok) = run("echo hi; false", &mut kept, &mut shutdown).await;
        assert!(!ok);
        assert_eq!(out, "hi\n[exit code 1]");
        let (out, ok) = run("true", &mut kept, &mut shutdown).await;
        assert!(ok);
        assert_eq!(out, "(Command finished with no output)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn long_output_keeps_its_end_and_saves_the_whole() {
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let (out, ok) = run_bash_command(
            &LocalExecutor,
            "seq 1 40000; echo 'error: the real failure' >&2; exit 1",
            Path::new("."),
            None,
            &commands,
            &mut kept,
            &mut shutdown,
        )
        .await
        .unwrap();
        assert!(!ok);
        assert!(out.starts_with("1\n2\n"), "{}", &out[..20]);
        assert!(
            out.contains("error: the real failure\n[exit code 1]"),
            "{}",
            &out[out.len() - 200..]
        );
        let path = out
            .split("All of it is in ")
            .nth(1)
            .and_then(|r| r.split(": read_file").next())
            .expect("a path");
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("\n20000\n"));
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn something_left_running_doesnt_hold_the_answer() {
        let started = Instant::now();
        let ran = LocalExecutor
            .exec("sleep 30 & echo started", Path::new("."), None)
            .await
            .unwrap();
        assert!(ran.success());
        assert_eq!(ran.stdout, "started\n");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commands_get_the_login_path_and_bash() {
        let ran = LocalExecutor
            .exec(
                "echo \"$PATH\"; echo \"${BASH_VERSION:+bash}\"",
                Path::new("."),
                None,
            )
            .await
            .unwrap();
        let mut lines = ran.stdout.lines();
        assert_eq!(lines.next(), Some(trek_core::detect::login_path()));
        assert_eq!(lines.next(), Some("bash"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_model_can_start_something_in_the_background_and_set_a_timeout() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call(
                    "call_1",
                    "bash",
                    json!({ "command": "echo from-background; sleep 30", "background": true }),
                ),
                openai_tool_call(
                    "call_2",
                    "bash",
                    json!({ "command": "sleep 30", "timeout_s": 1 }),
                ),
                openai_text("ok"),
            ],
        )
        .await;
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("process group"), "{output}");
        let log = output
            .split("output goes to ")
            .nth(1)
            .and_then(|r| r.split(": read it").next())
            .expect("a log path")
            .to_string();
        let (output, ok) = s.tool_finished().await;
        assert!(
            !ok && output.contains("timed out after 1 seconds"),
            "{output}"
        );
        assert_eq!(s.turn_complete().await, None);
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("from-background")
        );
        let group: i32 = s.body(1)["messages"][3]["content"]
            .as_str()
            .and_then(|o| o.split("process group ").nth(1))
            .and_then(|r| r.split('.').next())
            .and_then(|g| g.parse().ok())
            .expect("a group");
        s.finish().await;
        // Ended with the session.
        let alive = unsafe { libc::kill(-group, 0) } == 0;
        assert!(!alive, "group {group} outlived the session");
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn read_file_pages_through_a_long_file_and_lists_a_folder() {
        let (_tx, commands) = async_channel::unbounded();
        let (mut kept, mut shutdown) = (VecDeque::new(), false);
        let dir = std::env::temp_dir().join(format!("trek_read_{}", next_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (1..=3000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("long.txt"), body).unwrap();
        std::fs::write(dir.join("wide.txt"), "x".repeat(10_000)).unwrap();
        let mut waiting = Waiting {
            commands: &commands,
            kept: &mut kept,
            shutdown: &mut shutdown,
        };
        let (out, ok) = run_read_file(&LocalExecutor, "long.txt", None, None, &dir, &mut waiting)
            .await
            .unwrap();
        assert!(ok);
        assert!(
            out.contains("2000 | line 2000"),
            "{}",
            &out[out.len() - 200..]
        );
        assert!(!out.contains("2001 | line"));
        assert!(
            out.contains("Showing lines 1-2000 of 3000"),
            "{}",
            &out[out.len() - 200..]
        );
        assert!(out.contains("start_line 2001"));
        let (out, _) = run_read_file(
            &LocalExecutor,
            "long.txt",
            Some(2990),
            None,
            &dir,
            &mut waiting,
        )
        .await
        .unwrap();
        assert!(
            out.contains("3000 | line 3000") && out.contains("End of file: 3000 lines"),
            "{out}"
        );
        let (out, _) = run_read_file(&LocalExecutor, "wide.txt", None, None, &dir, &mut waiting)
            .await
            .unwrap();
        assert!(out.len() < READ_LINE + 200, "{}", out.len());
        assert!(out.contains("were cut"), "{out}");
        let (out, ok) = run_read_file(&LocalExecutor, ".", None, None, &dir, &mut waiting)
            .await
            .unwrap();
        assert!(
            ok && out.contains("long.txt") && out.contains("It holds"),
            "{out}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn edits_forgive_what_models_get_wrong_and_say_so() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call(
                    "call_1",
                    "edit_file",
                    json!({ "path": "a.rs", "old_str": "let a = 1;", "new_str": "let a = 2;" }),
                ),
                openai_tool_call(
                    "call_2",
                    "edit_file",
                    json!({ "path": "a.rs", "old_str": "nope", "new_str": "x" }),
                ),
                openai_tool_call(
                    "call_3",
                    "edit_file",
                    json!({ "path": "b.txt", "old_str": "x", "new_str": "y", "replace_all": true }),
                ),
                openai_text("ok"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("a.rs"), "fn f() {\n    let a = 1;   \n}\n").unwrap();
        std::fs::write(s.cwd.join("b.txt"), "x x\n").unwrap();
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok, "{output}");
        assert!(
            output.contains("let a = 2;") && output.contains("|"),
            "{output}"
        );
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("not found"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("2 places"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(
            std::fs::read_to_string(s.cwd.join("b.txt")).unwrap(),
            "y y\n"
        );
        s.finish().await;
    }

    #[tokio::test]
    async fn search_tools_work_read_only_and_writes_arent_on_offer() {
        let s = TestSession::start_configured(
            "ollama",
            HandHolding::Auto,
            vec![
                openai_tool_call("call_1", "grep", json!({ "pattern": "needle" })),
                openai_tool_call("call_2", "glob", json!({ "pattern": "*.txt" })),
                openai_tool_call("call_3", "list_dir", json!({})),
                openai_tool_call("call_4", "bash", json!({ "command": "ls" })),
                openai_tool_call("call_5", "bash", json!({ "command": "touch nope" })),
                openai_text("found it"),
            ],
            DirectOptions::default(),
            |c| c.read_only = true,
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "a needle here\n").unwrap();
        s.prompt("find it").await;
        let (output, ok) = s.tool_finished().await;
        assert!(
            ok && output.contains("notes.txt:1:a needle here"),
            "{output}"
        );
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("notes.txt"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("notes.txt"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("notes.txt"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("read-only"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert!(!s.cwd.join("nope").exists());
        let names = tool_names(&s.body(0));
        assert!(names.contains(&"grep".to_string()));
        assert!(
            !names.contains(&"write_file".to_string()) && !names.contains(&"edit_file".to_string())
        );
        s.finish().await;
    }

    #[test]
    fn shell_expansion_cant_smuggle_a_flag_past_the_check() {
        for command in [
            "find . ${x:--delete}",
            "find . {-delete,}",
            "git diff ${x:---output=a.rs}",
            "cat $HOME/.ssh/id_rsa",
        ] {
            assert!(!is_safe_command(command), "{command}");
            assert!(!is_read_only_command(command), "{command}");
        }
        assert!(is_safe_command("echo $HOME"));
        assert!(is_safe_command("find . -name a.rs"));
    }

    #[tokio::test]
    async fn the_fourth_cut_off_in_a_row_leaves_the_history_valid() {
        let cut = || {
            raw_tool_call(
                "c",
                "write_file",
                r#"{"path":"big.txt","content":"half of"#,
                "length",
            )
        };
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![cut(), cut(), cut(), cut(), openai_text("next one works")],
        )
        .await;
        s.prompt("write it").await;
        let error = s.turn_complete().await.unwrap();
        assert!(error.contains("output limit"), "{error}");
        // The session goes on: what's in history is something a provider can read.
        s.prompt("smaller then").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(4)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        s.finish().await;
    }

    #[tokio::test]
    async fn a_conversation_the_provider_refuses_goes_on_fresh_once() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call("c1", "bash", json!({ "command": "echo hi" })),
                (
                    400,
                    "{\"error\":{\"message\":\"messages.3: image too large\"}}".into(),
                ),
                openai_text("carried on"),
                openai_text("and the next"),
            ],
        )
        .await;
        s.prompt("go").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("refused the conversation"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        let fresh = s.body(2);
        assert!(
            fresh["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("<fresh-context>")
        );
        s.prompt("more").await;
        assert_eq!(s.turn_complete().await, None);
        s.finish().await;
    }

    #[test]
    fn invalid_names_dont_go_back_in_history() {
        let tools = tool_defs(false, true, false);
        let mut calls = vec![
            AccumulatedToolCall {
                id: "1".into(),
                name: "repo_browser.print_tree".into(),
                arguments: "{}".into(),
            },
            AccumulatedToolCall {
                id: "2".into(),
                name: "".into(),
                arguments: "{}".into(),
            },
        ];
        let problems = prepare_calls(&mut calls, &tools, false);
        assert_eq!(calls[0].name, "repo_browser_print_tree");
        assert_eq!(calls[1].name, "unknown_tool");
        assert!(
            problems[0]
                .as_ref()
                .unwrap()
                .contains("repo_browser.print_tree")
        );
        assert!(problems[1].as_ref().unwrap().contains("no name"));
    }

    #[test]
    fn only_usable_images_go_to_a_provider_and_old_ones_go_away() {
        let (usable, left_out) = usable_images(vec![
            ("image/png".into(), "aGk=".into()),
            ("image/svg+xml".into(), "aGk=".into()),
            ("image/png".into(), "x".repeat(MAX_TOOL_IMAGE + 1)),
        ]);
        assert_eq!(usable.len(), 1);
        assert_eq!(left_out, 2);
        let message = tool_images_message(Wire::OpenAiChat, usable, left_out);
        assert_eq!(message["content"][1]["type"], "image_url");
        assert!(
            message["content"][2]["text"]
                .as_str()
                .unwrap()
                .contains("2 more")
        );
        let mut history = vec![
            message,
            json!({ "role": "user", "content": [{ "type": "text", "text": "mine" }, { "type": "image_url", "image_url": { "url": "x" } }] }),
        ];
        forget_old_tool_images(&mut history);
        assert!(history[0]["content"].as_str().unwrap().contains("removed"));
        assert!(
            history[1]["content"].is_array(),
            "the user's own picture stays"
        );
    }

    #[test]
    fn what_one_model_refused_is_forgotten_for_the_next() {
        let mut quirks = Quirks {
            no_tools: true,
            no_reasoning: true,
            afford: Some(100),
            ..Quirks::default()
        };
        quirks.seen_ids.insert("a".into());
        quirks.forget_model();
        assert!(!quirks.no_tools && !quirks.no_reasoning);
        assert_eq!(quirks.afford, Some(100));
        assert!(quirks.seen_ids.contains("a"));
    }

    #[test]
    fn a_complaint_about_tool_choice_leaves_the_tools() {
        let mut quirks = Quirks::default();
        let mut body = json!({ "tools": [1], "tool_choice": "none", "messages": [] });
        assert!(quirks.adapt(400, "tool_choice is not supported by this model", &mut body));
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("tools").is_some());
        assert!(!quirks.no_tools);
    }

    #[test]
    fn a_provider_that_refuses_its_own_reasoning_stops_getting_it() {
        let mut quirks = Quirks::default();
        let mut body = json!({
            "reasoning_effort": "high",
            "messages": [{ "role": "assistant", "reasoning_content": "x", "tool_calls": [] }]
        });
        let message = "reasoning_content is not allowed here";
        assert!(quirks.adapt(400, message, &mut body));
        assert!(body.get("reasoning_effort").is_none());
        assert!(body["messages"][0].get("reasoning_content").is_some());
        assert!(quirks.adapt(400, message, &mut body));
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert!(quirks.no_reasoning_echo);
        assert!(!quirks.adapt(400, message, &mut body));
    }

    #[tokio::test]
    async fn arguments_sent_as_an_object_are_read() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                (
                    200,
                    format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "c", "function": { "name": "read_file", "arguments": { "path": "notes.txt" } } }] }, "finish_reason": "tool_calls" }] })
                    ),
                ),
                openai_text("ok"),
            ],
        )
        .await;
        std::fs::write(s.cwd.join("notes.txt"), "hello from notes").unwrap();
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("hello from notes"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        s.finish().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reading_something_that_isnt_a_file_says_so_and_doesnt_hang() {
        let e = LocalExecutor
            .read(Path::new("/dev/zero"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("regular file"), "{e}");
        assert!(
            LocalExecutor
                .read(Path::new("/definitely/not/here"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_only_commands_exclude_builds_and_follows() {
        assert!(is_read_only_command("git status"));
        assert!(is_read_only_command("ls -la"));
        assert!(!is_read_only_command("cargo check"));
        assert!(!is_read_only_command("rm x"));
        assert!(!is_safe_command("cargo test"));
        assert!(!is_safe_command("tail -f log"));
        assert!(is_safe_command("tail -n 5 log"));
    }

    #[tokio::test]
    async fn the_same_call_again_and_again_is_warned_about_then_stopped() {
        let mut script: Vec<(u16, String)> = (0..LoopGuard::STOP_AT)
            .map(|i| {
                openai_tool_call(
                    &format!("call_{i}"),
                    "read_file",
                    json!({ "path": "notes.txt" }),
                )
            })
            .collect();
        script.push(openai_text("I was stuck on notes.txt."));
        let s = TestSession::start("ollama", HandHolding::FullAccess, script).await;
        std::fs::write(s.cwd.join("notes.txt"), "same").unwrap();
        s.prompt("go").await;
        let notice = s
            .until(|e| match e {
                AgentEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(notice.contains("same calls"), "{notice}");
        assert_eq!(s.turn_complete().await, None);
        let bodies = s.bodies.lock().unwrap().len();
        assert_eq!(bodies, LoopGuard::STOP_AT + 1);
        let messages = s.body(bodies - 1)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        let tool_outputs: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"] == "tool")
            .filter_map(|m| m["content"].as_str())
            .collect();
        assert!(!tool_outputs[0].contains("same call"));
        assert!(tool_outputs[LoopGuard::WARN_AT - 1].contains("same call 4 times"));
        assert_eq!(s.body(bodies - 1)["tool_choice"], "none");
        s.finish().await;
    }

    #[tokio::test]
    async fn a_servers_tools_are_reached_through_two_small_tools() {
        let server = crate::McpServer::stdio(
            "fake",
            trek_test_fixtures::bin("fake-mcp").display().to_string(),
            vec!["tools".into()],
            vec![],
        );
        let s = TestSession::start_configured(
            "anthropic",
            HandHolding::FullAccess,
            vec![
                (200, anthropic_tool("t1", "mcp_list", json!({ "server": "fake" }))),
                (200, anthropic_tool("t2", "mcp_call", json!({ "server": "fake", "tool": "echo", "arguments": { "text": "hello" } }))),
                // By its own name, as Trek's notes say to call `watch_video`.
                (200, anthropic_tool("t3", "picture", json!({}))),
                (200, ANTHROPIC_TEXT.to_string()),
            ],
            DirectOptions::default(),
            |c| c.mcp_servers = vec![server],
        )
        .await;
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("echo"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output == "hello", "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output.contains("[image]"), "{output}");
        assert_eq!(s.turn_complete().await, None);
        let body = s.body(0);
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(
            names.contains(&"mcp_list") && names.contains(&"mcp_call"),
            "{names:?}"
        );
        assert!(!names.contains(&"echo"), "{names:?}");
        assert!(
            body["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains("MCP servers: fake")
        );
        // The picture went to the model.
        let last = s.body(3);
        let messages = last["messages"].as_array().unwrap();
        let parts = messages.last().unwrap()["content"].as_array().unwrap();
        assert_eq!(parts[1]["type"], "image");
        s.finish().await;
    }

    #[tokio::test]
    async fn read_only_sessions_only_run_tools_that_say_they_only_read() {
        let server = crate::McpServer::stdio(
            "fake",
            trek_test_fixtures::bin("fake-mcp").display().to_string(),
            vec!["ok".into()],
            vec![],
        );
        let s = TestSession::start_configured(
            "ollama",
            HandHolding::Auto,
            vec![
                openai_tool_call(
                    "c1",
                    "mcp_call",
                    json!({ "server": "fake", "tool": "echo", "arguments": { "text": "x" } }),
                ),
                openai_tool_call(
                    "c2",
                    "mcp_call",
                    json!({ "server": "fake", "tool": "get_time" }),
                ),
                openai_text("ok"),
            ],
            DirectOptions::default(),
            |c| {
                c.read_only = true;
                c.mcp_servers = vec![server];
            },
        )
        .await;
        s.prompt("go").await;
        let (output, ok) = s.tool_finished().await;
        assert!(!ok && output.contains("read-only"), "{output}");
        let (output, ok) = s.tool_finished().await;
        assert!(ok, "{output}");
        assert_eq!(s.turn_complete().await, None);
        s.finish().await;
    }

    /// Every request carries this, so what it costs is decided here. A weak model does worse with
    /// a long prompt and a long tool list: docs/PROMPT_BUDGET.md has what each piece is for.
    #[test]
    fn the_harness_prompt_stays_small() {
        let prompt = system_prompt(
            Path::new("/Users/someone/project"),
            false,
            "Today's date: 2026-11-05\nOS: macOS\nShell: bash\nGit branch: main",
            None,
            None,
        );
        let tools = serde_json::to_string(&tool_defs(false, true, false)).unwrap();
        let tokens = |text: &str| text.len() / 4;
        println!(
            "system {} tools {} mcp {}",
            tokens(&prompt),
            tokens(&tools),
            tokens(&serde_json::to_string(&mcp_tools()).unwrap())
        );
        assert!(tokens(&prompt) <= 300, "{}", tokens(&prompt));
        assert!(tokens(&tools) <= 900, "{}", tokens(&tools));
    }

    /// Two calls in one answer; the first is declined with a reply, the way the app sends it: the
    /// "no", then the words. The second doesn't run, and the words reach the model next.
    #[tokio::test]
    async fn a_reply_that_declines_a_call_stops_the_rest_of_the_answer() {
        let two = {
            let calls = json!([
                { "index": 0, "id": "c1", "function": { "name": "bash", "arguments": "{\"command\":\"touch first\"}" } },
                { "index": 1, "id": "c2", "function": { "name": "bash", "arguments": "{\"command\":\"touch second\"}" } }
            ]);
            (
                200,
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({ "choices": [{ "delta": { "tool_calls": calls }, "finish_reason": "tool_calls" }] })
                ),
            )
        };
        let s = TestSession::start(
            "ollama",
            HandHolding::Supervised,
            vec![two, openai_text("Understood.")],
        )
        .await;
        s.prompt("make two files").await;
        let request_id = s
            .until(|e| match e {
                AgentEvent::PermissionRequest { request_id, .. } => Some(request_id.clone()),
                _ => None,
            })
            .await;
        s.send(Command::Respond {
            request_id,
            decision: Decision::Deny,
        })
        .await;
        s.prompt("no, don't make files").await;
        let (first, _) = s.tool_finished().await;
        assert!(first.contains("denied"), "{first}");
        assert_eq!(s.turn_complete().await, None);
        assert!(!s.cwd.join("second").exists());
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        // Not run either way: skipped when the words were already there, declined by them when
        // they came while its own card was up.
        let second = messages
            .iter()
            .find(|m| m["tool_call_id"] == "c2")
            .and_then(|m| m["content"].as_str())
            .unwrap();
        assert!(
            second.contains("declined the call before it") || second.contains("denied"),
            "{second}"
        );
        assert_eq!(messages.last().unwrap()["content"], "no, don't make files");
        s.finish().await;
    }

    #[tokio::test]
    async fn a_compact_whose_notes_fail_leaves_the_conversation_as_it_was() {
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                openai_text("Hello."),
                (400, "{\"error\":{\"message\":\"out of credit\"}}".into()),
                openai_text("Still here."),
            ],
            DirectOptions {
                context_window: Some(40_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("hi").await;
        assert_eq!(s.turn_complete().await, None);
        s.prompt("/compact").await;
        let error = s.turn_complete().await.unwrap();
        assert!(
            error.contains("Couldn't compact") && error.contains("as it was"),
            "{error}"
        );
        s.prompt("next").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(2)["messages"].as_array().unwrap().clone();
        assert_eq!(user_texts(&messages), ["hi", "next"]);
        s.finish().await;
    }

    #[tokio::test]
    async fn at_auto_a_tool_on_someone_elses_server_that_may_change_things_asks() {
        let server = crate::McpServer::stdio(
            "fake",
            trek_test_fixtures::bin("fake-mcp").display().to_string(),
            vec!["ok".into()],
            vec![],
        );
        let s = TestSession::start_configured(
            "ollama",
            HandHolding::Auto,
            vec![
                openai_tool_call(
                    "c1",
                    "mcp_call",
                    json!({ "server": "fake", "tool": "get_time" }),
                ),
                openai_tool_call(
                    "c2",
                    "mcp_call",
                    json!({ "server": "fake", "tool": "echo", "arguments": { "text": "x" } }),
                ),
                openai_text("ok"),
            ],
            DirectOptions::default(),
            |c| c.mcp_servers = vec![server],
        )
        .await;
        s.prompt("go").await;
        // A tool that says it only reads: unasked.
        let (_, ok) = s.tool_finished().await;
        assert!(ok);
        // One that doesn't: asked.
        let request_id = s
            .until(|e| match e {
                AgentEvent::PermissionRequest {
                    request_id, detail, ..
                } => {
                    assert!(detail.contains("fake echo"), "{detail}");
                    Some(request_id.clone())
                }
                _ => None,
            })
            .await;
        s.send(Command::Respond {
            request_id,
            decision: Decision::Allow,
        })
        .await;
        let (output, ok) = s.tool_finished().await;
        assert!(ok && output == "x", "{output}");
        assert_eq!(s.turn_complete().await, None);
        assert!(crate::is_trek_server("trek-browser"));
        assert!(!crate::is_trek_server("fake"));
        s.finish().await;
    }

    #[test]
    fn calls_that_vary_arent_a_loop() {
        let mut guard = LoopGuard::default();
        assert_eq!(guard.record("bash", "a", "x"), 1);
        assert_eq!(guard.record("bash", "a", "x"), 2);
        assert_eq!(guard.record("bash", "a", "y"), 1, "a new result");
        assert_eq!(guard.record("bash", "b", "x"), 1, "new arguments");
    }

    #[tokio::test]
    async fn a_promise_with_no_call_is_nudged_once() {
        let s = TestSession::start(
            "ollama",
            HandHolding::FullAccess,
            vec![
                openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                openai_text("Now let me fix the failing test:"),
                openai_tool_call("call_2", "bash", json!({ "command": "echo fixed" })),
                openai_text("All done. Let me know if you want more."),
            ],
        )
        .await;
        s.prompt("go").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 4);
        let messages = s.body(2)["messages"].as_array().unwrap().clone();
        assert_eq!(messages.last().unwrap()["content"], NUDGE_PROMPT);
        s.finish().await;
    }

    #[test]
    fn promises_are_told_from_endings() {
        assert!(announces_more("Let me check the file."));
        assert!(announces_more("Here's what's next:"));
        assert!(announces_more("Done with that.\n\nI'll now run the tests."));
        assert!(!announces_more("All done."));
        assert!(!announces_more("Let me know if you need anything."));
        assert!(!announces_more(""));
    }

    #[tokio::test]
    async fn openrouter_requests_say_what_they_need() {
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![openai_text("hi")],
            DirectOptions {
                context_window: Some(100_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        let body = s.body(0);
        assert_eq!(body["provider"]["require_parameters"], true);
        assert!(
            body["session_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 256)
        );
        // An output limit, so OpenRouter doesn't reserve the model's whole maximum.
        assert_eq!(body["max_tokens"], 32_000);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("temperature").is_none() && body.get("top_p").is_none());
        s.finish().await;
    }

    #[tokio::test]
    async fn the_session_id_stays_the_same_and_the_prefix_too() {
        let s = TestSession::start(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                openai_text("hi"),
            ],
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        let (a, b) = (s.body(0), s.body(1));
        assert_eq!(a["session_id"], b["session_id"]);
        assert_eq!(a["messages"][0], b["messages"][0]);
        assert_eq!(a["tools"], b["tools"]);
        s.finish().await;
    }

    #[test]
    fn other_providers_get_a_limit_only_when_theirs_is_known() {
        let provider = |id: &'static str, local: bool| DirectProvider {
            id,
            name: "t",
            base_url: "",
            wire: Wire::OpenAiChat,
            env_key: None,
            local,
        };
        let known = ModelCaps {
            max_output: Some(8_192),
            ..Default::default()
        };
        assert_eq!(
            output_limit(&provider("groq", false), None, 100_000, 0, false),
            None
        );
        assert_eq!(
            output_limit(&provider("ollama", true), Some(&known), 100_000, 0, false),
            None
        );
        assert_eq!(
            output_limit(&provider("groq", false), Some(&known), 100_000, 0, false),
            Some(8_192)
        );
        // What's left of the window, and never so little that nothing can be said.
        assert_eq!(
            output_limit(&provider("openrouter", false), None, 100_000, 90_000, false),
            Some(8_976)
        );
        assert_eq!(
            output_limit(&provider("openrouter", false), None, 100_000, 99_900, false),
            Some(1_024)
        );
    }

    #[tokio::test]
    async fn too_little_credit_for_the_limit_lowers_the_limit() {
        let s = TestSession::start(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                (402, "{\"error\":{\"message\":\"You requested up to 32000 tokens, but can only afford 18354\"}}".into()),
                openai_text("hi"),
                openai_text("again"),
            ],
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.body(1)["max_tokens"], 16_519);
        s.prompt("next").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.body(2)["max_tokens"], 16_519);
        s.finish().await;
    }

    #[tokio::test]
    async fn no_routes_for_the_request_drops_the_routing_rule() {
        let s = TestSession::start(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                (404, "{\"error\":{\"message\":\"No endpoints found that can handle the requested parameters\"}}".into()),
                openai_text("hi"),
            ],
        )
        .await;
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        assert!(s.body(1).get("provider").is_none());
        s.finish().await;
    }

    #[test]
    fn reasoning_levels_come_from_what_the_model_offers() {
        let offers = |efforts: &[&str]| ModelCaps {
            reasoning: Some(true),
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
            ..Default::default()
        };
        let flash = offers(&["xhigh", "high", "none"]);
        let level = |e, c: &ModelCaps| reasoning_level(e, Some(c));
        assert_eq!(level(Effort::Low, &flash).as_deref(), Some("high"));
        assert_eq!(level(Effort::Max, &flash).as_deref(), Some("xhigh"));
        assert_eq!(level(Effort::Off, &flash).as_deref(), Some("none"));
        let cant_stop = offers(&["low", "high"]);
        assert_eq!(level(Effort::Off, &cant_stop), None);
        let none = ModelCaps {
            reasoning: Some(false),
            ..Default::default()
        };
        assert_eq!(level(Effort::High, &none), None);
        // Nothing known: the three everyone takes.
        assert_eq!(reasoning_level(Effort::Max, None).as_deref(), Some("high"));
        assert_eq!(reasoning_level(Effort::Off, None), None);
    }

    #[test]
    fn the_model_list_keeps_what_models_can_do_and_leaves_out_what_cant_use_tools() {
        let list = json!({ "data": [
            {
                "id": "deepseek/deepseek-v4-flash",
                "name": "DeepSeek V4 Flash",
                "context_length": 1_048_576,
                "supported_parameters": ["tools", "reasoning", "max_tokens"],
                "top_provider": { "max_completion_tokens": 131_072 },
                "architecture": { "input_modalities": ["text"] },
                "reasoning": { "supported_efforts": ["xhigh", "high", "none"] }
            },
            {
                "id": "vendor/vision",
                "supported_parameters": ["tools"],
                "architecture": { "input_modalities": ["text", "image"] }
            },
            {
                "id": "tencent/no-tools",
                "supported_parameters": ["max_tokens"]
            },
            { "id": "plain" }
        ]});
        let models = parse_direct_models(&list);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["deepseek/deepseek-v4-flash", "vendor/vision", "plain"]
        );
        let flash = models[0].caps.as_ref().unwrap();
        assert_eq!(flash.max_output, Some(131_072));
        assert_eq!(flash.tools, Some(true));
        assert_eq!(flash.reasoning, Some(true));
        assert_eq!(flash.images, Some(false));
        assert_eq!(flash.efforts, ["xhigh", "high", "none"]);
        let vision = models[1].caps.as_ref().unwrap();
        assert_eq!(vision.images, Some(true));
        assert_eq!(vision.reasoning, Some(false));
        assert_eq!(models[2].caps, None);
    }

    #[test]
    fn images_arent_sent_to_a_model_that_cant_see() {
        let blind = ModelCaps {
            images: Some(false),
            ..Default::default()
        };
        let path = PathBuf::from("/tmp/a.png");
        let (message, skipped) = user_message(
            Wire::OpenAiChat,
            "look",
            std::slice::from_ref(&path),
            Some(&blind),
        );
        assert_eq!(message["content"], "look");
        assert!(skipped[0].contains("can't see images"), "{skipped:?}");
    }

    #[test]
    fn reasoning_pieces_are_put_back_together() {
        let merged = merge_reasoning_details(vec![
            json!({ "type": "reasoning.text", "index": 0, "text": "one " }),
            json!({ "type": "reasoning.text", "index": 0, "text": "two", "signature": "sig" }),
            json!({ "type": "reasoning.encrypted", "index": 1, "data": "abc" }),
            json!({ "type": "reasoning.encrypted", "index": 1, "data": "def" }),
            json!({ "type": "reasoning.summary", "summary": "a" }),
            json!({ "type": "reasoning.summary", "summary": "b" }),
        ]);
        assert_eq!(
            merged,
            vec![
                json!({ "type": "reasoning.text", "index": 0, "text": "one two", "signature": "sig" }),
                json!({ "type": "reasoning.encrypted", "index": 1, "data": "abcdef" }),
                json!({ "type": "reasoning.summary", "summary": "ab" }),
            ]
        );
    }

    #[tokio::test]
    async fn plain_reasoning_goes_back_in_the_field_it_came_in() {
        let call = json!({ "index": 0, "id": "c1", "function": { "name": "bash", "arguments": "{\"command\":\"echo hi\"}" } });
        let s = TestSession::start(
            "deepseek",
            HandHolding::FullAccess,
            vec![
                (
                    200,
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({ "choices": [{ "delta": { "reasoning_content": "thinking" } }] }),
                        json!({ "choices": [{ "delta": { "tool_calls": [call] }, "finish_reason": "tool_calls" }] }),
                    ),
                ),
                openai_text("done"),
            ],
        )
        .await;
        s.prompt("go").await;
        assert_eq!(s.turn_complete().await, None);
        let messages = s.body(1)["messages"].as_array().unwrap().clone();
        assert_eq!(messages[2]["reasoning_content"], "thinking");
        s.finish().await;
    }

    #[test]
    fn deepseek_gets_reasoning_on_every_tool_call_and_empty_messages_go() {
        let provider = DirectProvider {
            id: "deepseek",
            name: "t",
            base_url: "",
            wire: Wire::OpenAiChat,
            env_key: None,
            local: false,
        };
        let agent = AgentId::Direct("deepseek".into());
        let client = reqwest::Client::new();
        let history = [
            json!({ "role": "user", "content": "a" }),
            json!({ "role": "assistant", "content": "  " }),
            json!({ "role": "user", "content": "b" }),
            json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": "x", "type": "function", "function": { "name": "bash", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "x", "content": "out" }),
        ];
        let body = openai_body(
            &TurnRequest {
                client: &client,
                agent: &agent,
                provider: &provider,
                url: "",
                key: None,
                model: "deepseek-v4",
                effort: Effort::High,
                system: "sys",
                history: &history,
                tools: &[],
                no_tools: false,
                max_output: None,
                caps: None,
                session_id: "s",
            },
            &Quirks::default(),
        );
        let messages = body["messages"].as_array().unwrap();
        // Empty assistant message gone, and the two user messages made one.
        assert_eq!(messages[1]["content"], "a\n\nb");
        assert_eq!(messages[2]["reasoning_content"], " ");
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn breakpoints_are_only_for_models_that_use_them() {
        let provider = DirectProvider {
            id: "openrouter",
            name: "t",
            base_url: "",
            wire: Wire::OpenAiChat,
            env_key: None,
            local: false,
        };
        let agent = AgentId::Direct("openrouter".into());
        let client = reqwest::Client::new();
        let history = [json!({ "role": "user", "content": "hi" })];
        let body_for = |model: &'static str| {
            openai_body(
                &TurnRequest {
                    client: &client,
                    agent: &agent,
                    provider: &provider,
                    url: "",
                    key: None,
                    model,
                    effort: Effort::Off,
                    system: "sys",
                    history: &history,
                    tools: &[],
                    no_tools: false,
                    max_output: None,
                    caps: None,
                    session_id: "s",
                },
                &Quirks::default(),
            )
        };
        let claude = body_for("anthropic/claude-sonnet-5");
        assert_eq!(
            claude["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            claude["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        let other = body_for("deepseek/deepseek-v4-flash");
        assert_eq!(other["messages"][0]["content"], "sys");
        assert_eq!(other["messages"][1]["content"], "hi");
    }

    #[tokio::test]
    async fn the_system_prompt_says_where_it_is_and_reads_the_projects_instructions() {
        let s =
            TestSession::start("ollama", HandHolding::FullAccess, vec![openai_text("hi")]).await;
        std::fs::write(s.cwd.join("AGENTS.md"), "Always use tabs.").unwrap();
        s.prompt("hello").await;
        assert_eq!(s.turn_complete().await, None);
        let system = s.body(0)["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            system.contains("Today's date:") && system.contains("OS:"),
            "{system}"
        );
        assert!(system.contains("[exit code N]"), "{system}");
        // Read when the session starts, which here is before the file was written.
        s.finish().await;

        let dir = std::env::temp_dir().join(format!("trek_notes_{}", next_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Always use tabs.").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "Not this one.").unwrap();
        let notes = project_instructions(&LocalExecutor, &dir).await.unwrap();
        assert!(
            notes.contains("Always use tabs.") && !notes.contains("Not this one."),
            "{notes}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn old_output_is_cut_down_before_the_context_fills() {
        let big = "x".repeat(60_000);
        let s = TestSession::start_configured(
            "ollama",
            HandHolding::FullAccess,
            vec![
                using(
                    100,
                    openai_tool_call(
                        "call_1",
                        "bash",
                        json!({ "command": format!("printf '%s' {big}") }),
                    ),
                ),
                using(
                    100,
                    openai_tool_call("call_2", "bash", json!({ "command": "echo two" })),
                ),
                openai_tool_call(
                    "call_3",
                    "bash",
                    json!({ "command": format!("printf '%s' {}", "y".repeat(40_000)) }),
                ),
                openai_tool_call("call_4", "bash", json!({ "command": "echo four" })),
                openai_text("done"),
            ],
            DirectOptions {
                context_window: Some(30_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("go").await;
        assert_eq!(s.turn_complete().await, None);
        let last = s.bodies.lock().unwrap().len() - 1;
        let messages = s.body(last)["messages"].as_array().unwrap().clone();
        assert_tool_calls_answered(&messages);
        s.finish().await;
    }

    #[test]
    fn eliding_keeps_the_latest_and_doesnt_bother_for_little() {
        let long = "a ".repeat(20_000);
        let mut history = vec![
            json!({ "role": "user", "content": "go" }),
            json!({ "role": "tool", "tool_call_id": "1", "content": long }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "2", "content": "b ".repeat(20_000) }] }),
            json!({ "role": "tool", "tool_call_id": "3", "content": "c ".repeat(10_000) }),
        ];
        assert!(elide_old_outputs(&mut history, 20_000, "m"));
        let first = history[1]["content"].as_str().unwrap();
        assert!(
            first.len() < 1_000 && first.contains("removed to save context"),
            "{first}"
        );
        let second = history[2]["content"][0]["content"].as_str().unwrap();
        assert!(second.len() < 1_000, "{}", second.len());
        assert_eq!(
            history[3]["content"].as_str().unwrap().len(),
            20_000,
            "the latest stays"
        );
        // Done again, there's nothing left worth changing the cache for.
        assert!(!elide_old_outputs(&mut history, 20_000, "m"));
    }

    #[test]
    fn eliding_leaves_reasoning_some_models_insist_on() {
        let with_reasoning = || {
            vec![
                json!({ "role": "user", "content": "a" }),
                json!({ "role": "assistant", "content": "x", "reasoning_details": [{ "type": "reasoning.text", "text": "r ".repeat(20_000) }] }),
                json!({ "role": "user", "content": "b" }),
            ]
        };
        let mut history = with_reasoning();
        assert!(elide_old_outputs(&mut history, 20_000, "vendor/some-model"));
        assert!(history[1].get("reasoning_details").is_none());
        let mut history = with_reasoning();
        elide_old_outputs(&mut history, 20_000, "deepseek/deepseek-v4-flash");
        assert!(history[1].get("reasoning_details").is_some());
    }

    #[test]
    fn switching_models_drops_the_last_ones_reasoning() {
        let mut history = vec![
            json!({ "role": "assistant", "content": null, "reasoning_details": [1], "reasoning_content": "x" }),
            json!({ "role": "assistant", "content": [{ "type": "thinking", "thinking": "t" }, { "type": "text", "text": "hi" }] }),
        ];
        strip_reasoning(&mut history);
        assert!(history[0].get("reasoning_details").is_none());
        assert!(history[0].get("reasoning_content").is_none());
        assert_eq!(history[1]["content"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_stop_keeps_the_message_and_what_was_said_of_the_answer() {
        // The stream starts, then stays open: only a Stop ends it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = bodies.clone();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 65536];
                loop {
                    let n = tokio::io::AsyncReadExt::read(&mut socket, &mut buf)
                        .await
                        .unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(at) = text.find("\r\n\r\n") {
                        let length: usize = text
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= at + 4 + length {
                            seen.lock().unwrap().push(text[at + 4..].to_string());
                            break;
                        }
                    }
                }
                let first = seen.lock().unwrap().len() == 1;
                let reply = if first {
                    "data: {\"choices\":[{\"delta\":{\"content\":\"So far \"}}]}\n\n".to_string()
                } else {
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string()
                };
                let head = if first {
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n".to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        reply.len()
                    )
                };
                tokio::io::AsyncWriteExt::write_all(
                    &mut socket,
                    format!("{head}{reply}").as_bytes(),
                )
                .await
                .unwrap();
                held.push(socket);
            }
        });
        let cwd = std::env::temp_dir().join(format!("trek_test_stop_{}", next_id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let config = SessionConfig {
            agent: AgentId::Direct("ollama".into()),
            cwd: cwd.clone(),
            model: Some("m".into()),
            effort: Effort::Off,
            hand_holding: HandHolding::FullAccess,
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
            transcript: None,
        };
        let (tx, commands) = async_channel::unbounded();
        let (events, rx) = async_channel::unbounded();
        let provider = *direct_provider("ollama").unwrap();
        let url = format!("http://{addr}");
        let task = tokio::spawn(async move {
            session(
                config,
                &provider,
                &url,
                None,
                DirectOptions::default(),
                commands,
                events,
            )
            .await
        });
        tx.send(Command::Prompt {
            text: "first".into(),
            images: vec![],
        })
        .await
        .unwrap();
        loop {
            if matches!(rx.recv().await.unwrap(), AgentEvent::TextDelta(_)) {
                break;
            }
        }
        tx.send(Command::Interrupt).await.unwrap();
        loop {
            if let AgentEvent::TurnComplete { error } = rx.recv().await.unwrap() {
                assert_eq!(error.as_deref(), Some("Interrupted"));
                break;
            }
        }
        tx.send(Command::Prompt {
            text: "second".into(),
            images: vec![],
        })
        .await
        .unwrap();
        loop {
            if let AgentEvent::TurnComplete { error } = rx.recv().await.unwrap() {
                assert_eq!(error, None);
                break;
            }
        }
        let second: Value = serde_json::from_str(&bodies.lock().unwrap()[1]).unwrap();
        let messages = second["messages"].as_array().unwrap();
        assert_eq!(messages[1]["content"], "first");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "So far ");
        assert_eq!(messages[3]["content"], "second");
        tx.send(Command::Shutdown).await.unwrap();
        let _ = task.await;
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn compact_writes_notes_and_starts_fresh_with_nothing_more_to_answer() {
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                openai_text("Hello."),
                openai_text("Notes: we said hello."),
                openai_text("Hello again."),
            ],
            DirectOptions {
                context_window: Some(40_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        // Nothing to compact yet: no request, and it says so.
        s.prompt("/compact").await;
        assert_eq!(s.turn_complete().await, None);
        assert_eq!(s.bodies.lock().unwrap().len(), 0);
        s.prompt("hi").await;
        assert_eq!(s.turn_complete().await, None);
        s.prompt("/compact").await;
        let used = s
            .until(|e| match e {
                AgentEvent::Context { used, .. } => Some(*used),
                _ => None,
            })
            .await;
        assert!(used > 0);
        assert_eq!(s.turn_complete().await, None);
        let asked = s.body(1);
        assert_eq!(asked["tool_choice"], "none");
        // The next message goes alone, with the notes in the system prompt.
        s.prompt("next").await;
        assert_eq!(s.turn_complete().await, None);
        let fresh = s.body(2);
        let messages = fresh["messages"].as_array().unwrap();
        assert_eq!(user_texts(messages), ["next"]);
        let system = messages[0]["content"].as_str().unwrap();
        assert!(system.contains("Notes: we said hello."), "{system}");
        assert!(!system.contains("The request you're answering"), "{system}");
        s.finish().await;
    }

    #[tokio::test]
    async fn a_nearly_full_context_writes_notes_for_itself() {
        let s = TestSession::start_configured(
            "openrouter",
            HandHolding::FullAccess,
            vec![
                using(
                    38_000,
                    openai_tool_call("call_1", "bash", json!({ "command": "echo hi" })),
                ),
                using(38_200, openai_text("My notes: fixing the parser in a.rs.")),
                using(500, openai_text("Carried on.")),
            ],
            DirectOptions {
                context_window: Some(40_000),
                ..Default::default()
            },
            |_| {},
        )
        .await;
        s.prompt("do the work").await;
        assert_eq!(s.turn_complete().await, None);
        let asked = s.body(1);
        assert_eq!(asked["tool_choice"], "none");
        let asked_messages = asked["messages"].as_array().unwrap();
        assert_eq!(asked_messages.last().unwrap()["content"], SUMMARY_PROMPT);
        let fresh = s.body(2);
        let system = fresh["messages"][0]["content"].as_str().unwrap();
        assert!(
            system.contains("My notes: fixing the parser in a.rs."),
            "{system}"
        );
        assert_eq!(
            user_texts(fresh["messages"].as_array().unwrap()),
            [CONTINUE_MESSAGE]
        );
        s.finish().await;
    }
}
