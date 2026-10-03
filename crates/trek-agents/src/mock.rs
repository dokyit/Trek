//! A scripted agent with no process and no network, for demos, UI tests and performance work.
//! Each prompt plays a realistic event stream chosen by a keyword in it:
//!
//! | keyword                      | plays                                                        |
//! | ---------------------------- | ------------------------------------------------------------ |
//! | (none)                       | an answer streamed token by token: markdown, code, paths     |
//! | `tools`                      | commands, reads, a search and an edit, with outputs          |
//! | `agents` / `subagents` [dur] | two background sub-agents that report back `dur` later        |
//! | `permission`                 | a command that needs approval                                |
//! | `question`                   | multiple-choice questions                                     |
//! | `plan` (or plan mode)        | a plan to approve before any change                          |
//! | `mock:long` [dur]            | a long build that runs `dur` (default 30s)                   |
//! | `mock:stream` [dur]          | one long answer streamed for `dur` (default 30s)             |
//! | `error`                      | a turn that fails                                             |
//! | `mock:limit` [dur]           | a usage limit that resets `dur` from now (default 5s)        |
//! | `mock:write`                 | adds a line to `NOTES.md` in the session's folder (for real)  |
//! | `recall`                     | the messages it remembers from this conversation             |
//!
//! Keywords may be written bare or as `mock:<keyword>`; durations look like `500ms`, `30s`, `2m`.
//! Every turn also reports context usage. A prompt sent mid-turn steers it. Once a session has
//! hit its limit, every prompt hits it again until it resets (or `lift_limit`).
//!
//! Like Claude Code and Codex, it keeps each session's history (in memory, for the process) and
//! can resume one partway or fork it (`SessionConfig::resume_at`, `fork`); its `mock-recap`
//! model can't, as ACP agents can't, so rewinds give it a recap instead.
//!
//! Selected by `AgentId::Direct("mock")`. Trek offers it only when `TREK_MOCK_AGENT=1` (and in
//! its own tests).

use crate::{AgentEvent, Billing, Command, Decision, Prompt, Question, SessionConfig};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use trek_core::HandHolding;

pub use trek_core::catalog::MOCK_PROVIDER as PROVIDER;

/// The mock's model that can't resume a session partway (see `crate::resumes_partway`).
pub const RECAP_MODEL: &str = "mock-recap";

/// Every mock session's history, by session id: each message with the mark of the turn it
/// belongs to, oldest first. The mock's stand-in for an agent's session files.
static HISTORY: LazyLock<Mutex<HashMap<String, Vec<(String, String)>>>> = LazyLock::new(Default::default);

fn new_id(kind: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("mock-{kind}-{nanos:x}-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Sessions at their usage limit: session id → when it resets (unix ms).
static LIMITS: LazyLock<Mutex<HashMap<String, i64>>> = LazyLock::new(Default::default);

/// Lift session `id`'s usage limit now, as if it had reset (tests run on a clock of their own).
pub fn lift_limit(id: &str) {
    LIMITS.lock().unwrap().remove(id);
}

/// The messages the mock remembers in session `id`, oldest first.
pub fn remembered(id: &str) -> Vec<String> {
    HISTORY.lock().unwrap().get(id).map(|h| h.iter().map(|(_, m)| m.clone()).collect()).unwrap_or_default()
}

/// The mark of the last turn in session `id` (see `crate::session_tail`).
pub fn last_mark(id: &str) -> Option<String> {
    HISTORY.lock().unwrap().get(id)?.last().map(|(mark, _)| mark.clone())
}

/// A message that came with a recap of the conversation: the messages the recap holds, and the
/// message itself.
fn split_recap(text: &str) -> (Vec<String>, &str) {
    let Some((head, message)) = text.split_once("</recap>") else { return (vec![], text) };
    let recap = head.split_once("<recap>").map_or("", |(_, r)| r);
    let said = recap.split("\n\n").filter_map(|e| e.trim().strip_prefix("User: ")).map(|e| e.trim().to_string()).collect();
    (said, message.trim_start())
}

/// The mock shows up in pickers when `TREK_MOCK_AGENT=1`.
pub fn enabled() -> bool {
    std::env::var("TREK_MOCK_AGENT").is_ok_and(|v| v == "1")
}

/// Pace in thousandths: 1000 plays at demo speed, 0 plays the script without delays.
static PACE: AtomicU32 = AtomicU32::new(1000);

/// Scale the script's built-in delays (`1.0` = demo speed, `0.0` = none). Durations written in
/// the prompt (`mock:long 30s`) are kept as written.
pub fn set_pace(pace: f32) {
    PACE.store((pace.max(0.) * 1000.) as u32, Ordering::Relaxed);
}

fn paced(ms: u64) -> Duration {
    Duration::from_millis(ms * PACE.load(Ordering::Relaxed) as u64 / 1000)
}

const WINDOW: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Script {
    Answer,
    Tools,
    Agents(Option<Duration>),
    Permission,
    Questions,
    Plan,
    Long(Duration),
    Stream(Duration),
    Error,
    Write,
    Recall,
    Limit(Duration),
}

impl Script {
    /// The script a prompt asks for. `plan` is the session's plan mode: every turn ends in a plan.
    fn parse(text: &str, plan: bool) -> Script {
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '-').to_lowercase())
            .collect();
        let duration_after = |i: usize| words.get(i + 1).and_then(|w| parse_duration(w));
        let found = words.iter().enumerate().find_map(|(i, w)| {
            let key = w.strip_prefix("mock:").unwrap_or(w);
            Some(match key {
                // Bare "long" and "stream" are too common to start a 30-second turn.
                "long" if w.starts_with("mock:") => Script::Long(duration_after(i).unwrap_or(Duration::from_secs(30))),
                "stream" if w.starts_with("mock:") => Script::Stream(duration_after(i).unwrap_or(Duration::from_secs(30))),
                // The one script that changes files: only when asked for by its full name.
                "write" if w.starts_with("mock:") => Script::Write,
                "limit" if w.starts_with("mock:") => Script::Limit(duration_after(i).unwrap_or(Duration::from_secs(5))),
                "error" => Script::Error,
                "permission" => Script::Permission,
                "question" | "questions" => Script::Questions,
                "plan" => Script::Plan,
                "agents" | "subagents" | "sub-agents" => Script::Agents(duration_after(i)),
                "tools" => Script::Tools,
                "recall" => Script::Recall,
                _ => return None,
            })
        });
        match found {
            Some(s) => s,
            None if plan => Script::Plan,
            None => Script::Answer,
        }
    }
}

/// The title a thread gets after its first turn: the mock names it after its script instead of
/// asking a model.
pub fn title(request: &str) -> String {
    match Script::parse(request, false) {
        Script::Answer => "How the app starts",
        Script::Tools => "Add a verbose flag",
        Script::Agents(_) => "Scout the routes and error handling",
        Script::Permission => "Apply the schema migrations",
        Script::Questions => "Choose a database",
        Script::Plan => "Require a session on every route",
        Script::Long(_) => "Run the full test suite",
        Script::Stream(_) => "Walk through the codebase",
        Script::Error => "Fix the failing build",
        Script::Write => "Add a note",
        Script::Recall => "What was said",
        Script::Limit(_) => "Refactor the parser",
    }
    .into()
}

/// The longest duration a prompt can ask for.
const MAX_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// `500ms`, `30s`, `2m`, at most a day.
fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.find(|c: char| !c.is_ascii_digit()).map(|i| s.split_at(i))?;
    let n: u64 = num.parse().ok()?;
    let d = match unit {
        "ms" => Duration::from_millis(n),
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n.saturating_mul(60)),
        _ => return None,
    };
    Some(d.min(MAX_DURATION))
}

/// Why a turn stopped early.
enum Stop {
    Interrupted,
    /// Shut down, or Trek stopped listening.
    Closed,
}

type Step<T = ()> = std::result::Result<T, Stop>;

/// What the user said to a pending request.
enum Reply {
    Decision(Decision),
    Answers(Vec<(String, String)>),
}

struct Session {
    /// The session's folder (`mock:write` writes there).
    cwd: std::path::PathBuf,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
    hand_holding: HandHolding,
    plan: bool,
    /// "Always allow" was chosen for commands this session.
    commands_allowed: bool,
    context: u64,
    /// What the session has "cost" so far; turns report it as a running total, as real agents do.
    cost: f64,
    next_id: u64,
    /// Prompts sent while a turn ran; the turn acknowledges them at its next step.
    steer: Vec<String>,
    /// Background sub-agents still out: (tool call id, description).
    background: Vec<(String, String)>,
    /// The session's id, under which its history is kept.
    native_id: String,
    /// The mark of the turn under way: messages sent during it share it.
    mark: String,
}

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let (native_id, resumed_at, recap) = open_session(&config);
    let commands = match recap {
        Some(r) => crate::recap_first(commands, r),
        None => commands,
    };
    let mut s = Session {
        cwd: config.cwd.clone(),
        commands,
        events,
        hand_holding: config.hand_holding,
        plan: config.plan,
        commands_allowed: false,
        context: 9_400,
        cost: 0.,
        next_id: 0,
        steer: vec![],
        background: vec![],
        native_id: native_id.clone(),
        mark: String::new(),
    };
    if s.emit(AgentEvent::Started { native_id, model: Some(config.model.clone().unwrap_or_else(|| "mock-swift".into())) }).await.is_err() {
        return Ok(());
    }
    if let Some(at) = resumed_at {
        let _ = s.emit(AgentEvent::Mark(at)).await;
    }
    // Nothing leaves the Mac, so nothing is billed.
    let _ = s.emit(AgentEvent::Billing(Billing::Local)).await;
    let _ = s.emit(AgentEvent::Context { used: s.context, window: WINDOW }).await;
    while let Ok(cmd) = s.commands.recv().await {
        match cmd {
            Command::Prompt { text, .. } => {
                if s.turn(&text).await.is_err() {
                    break;
                }
            }
            Command::SetHandHolding(h) => s.hand_holding = h,
            Command::Shutdown => break,
            // Nothing is running or pending between turns.
            Command::SetModel { .. } | Command::Interrupt | Command::Respond { .. } | Command::Answer { .. } => {}
        }
    }
    Ok(())
}

/// The session `config` asks for: its id, the point it was resumed at (if partway), and the
/// recap to use because it couldn't be (the history is gone, or the point isn't in it).
fn open_session(config: &SessionConfig) -> (String, Option<String>, Option<String>) {
    let mut sessions = HISTORY.lock().unwrap();
    let Some(from) = &config.resume else {
        let id = new_id("session");
        sessions.insert(id.clone(), vec![]);
        // A new session: the recap reaches it with the first message (`crate::start`).
        return (id, None, None);
    };
    let mut history = sessions.get(from).cloned().unwrap_or_default();
    let mut recap = None;
    if let Some(at) = &config.resume_at {
        match history.iter().rposition(|(mark, _)| mark == at) {
            Some(ix) => history.truncate(ix + 1),
            None => {
                history.clear();
                recap = config.recap.clone();
            }
        }
    }
    let id = if config.fork || recap.is_some() { new_id("session") } else { from.clone() };
    sessions.insert(id.clone(), history);
    let at = config.resume_at.clone().filter(|_| recap.is_none());
    (id, at, recap)
}

impl Session {
    async fn emit(&self, ev: AgentEvent) -> Step {
        self.events.send(ev).await.map_err(|_| Stop::Closed)
    }

    fn id(&mut self, kind: &str) -> String {
        self.next_id += 1;
        format!("mock-{kind}-{}", self.next_id)
    }

    /// Keep a message in the session's history, with the turn under way.
    fn remember(&self, text: &str) {
        let (recapped, message) = split_recap(text);
        let mut sessions = HISTORY.lock().unwrap();
        let history = sessions.entry(self.native_id.clone()).or_default();
        history.extend(recapped.into_iter().map(|m| ("recap".to_string(), m)));
        history.push((self.mark.clone(), message.to_string()));
    }

    /// One prompt, start to finish (including an early stop). Errs only when the session is over.
    async fn turn(&mut self, text: &str) -> Step {
        self.mark = new_id("turn");
        self.remember(text);
        // The turn is in the history now: cutting the session back to its mark keeps it whole.
        self.emit(AgentEvent::Mark(self.mark.clone())).await?;
        let text = split_recap(text).1.to_string();
        let text = text.as_str();
        self.context += 1_200 + text.len() as u64 / 3;
        self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await?;
        let mut script = Script::parse(text, self.plan);
        // Until the limit resets, every prompt meets it.
        if let Some(until) = LIMITS.lock().unwrap().get(&self.native_id).copied().filter(|u| *u > trek_core::store::now_ms()) {
            script = Script::Limit(Duration::from_millis((until - trek_core::store::now_ms()) as u64));
        }
        match self.play(script).await {
            Ok(()) => {}
            Err(Stop::Interrupted) => {
                self.steer.clear();
                for (id, _) in std::mem::take(&mut self.background) {
                    self.emit(AgentEvent::Task { id, description: None, activity: None, tool_uses: None, done: Some(false) }).await?;
                }
                self.emit(AgentEvent::Background(0)).await?;
                self.emit(AgentEvent::TurnComplete { cost_usd: None, error: Some("Interrupted".into()) }).await?;
            }
            Err(Stop::Closed) => return Err(Stop::Closed),
        }
        self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await
    }

    async fn play(&mut self, script: Script) -> Step {
        match script {
            Script::Answer => {
                self.think("The user wants an overview. I'll keep it short and point at the files that matter.").await?;
                self.say(ANSWER).await?;
            }
            Script::Tools => self.tools().await?,
            Script::Agents(after) => return self.agents(after).await,
            Script::Permission => self.permission().await?,
            Script::Questions => self.questions().await?,
            Script::Plan => self.plan_turn().await?,
            Script::Long(total) => self.long(total).await?,
            Script::Stream(total) => self.stream(total).await?,
            Script::Write => self.write().await?,
            Script::Recall => {
                let mut said = remembered(&self.native_id);
                said.pop();
                let text = if said.is_empty() { "I don't remember anything from before this message.".to_string() } else { format!("I remember: {}", said.join(" | ")) };
                self.say(&text).await?;
            }
            Script::Limit(after) => return self.limit(after).await,
            Script::Error => {
                self.think("Let me check the build first.").await?;
                let id = self.id("tool");
                self.tool_start(&id, "Run command", "cargo build").await?;
                self.pause(paced(400)).await?;
                self.emit(AgentEvent::ToolFinished { id, output: "error[E0425]: cannot find value `cfg` in this scope\n --> src/main.rs:14:9".into(), ok: false }).await?;
                let cost_usd = Some(self.spend(0.01));
                return self.emit(AgentEvent::TurnComplete { cost_usd, error: Some("The mock agent hit an error: the build failed and the session ended (exit code 101).".into()) }).await;
            }
        }
        self.finish().await
    }

    async fn finish(&mut self) -> Step {
        self.acknowledge_steer().await?;
        let cost_usd = Some(self.spend(0.02));
        self.emit(AgentEvent::TurnComplete { cost_usd, error: None }).await
    }

    /// A usage limit hit partway through, resetting `after` from now. Like Claude Code: the
    /// limit's own message instead of an answer, and the turn fails with it.
    async fn limit(&mut self, after: Duration) -> Step {
        let resets_at = {
            let mut limits = LIMITS.lock().unwrap();
            *limits.entry(self.native_id.clone()).or_insert_with(|| trek_core::store::now_ms() + after.as_millis() as i64)
        };
        self.think("Picking up the parser refactor where it stood.").await?;
        self.tool("Read", "src/parser.rs", "pub fn parse(input: &str) -> Result<Ast> { … }", 150).await?;
        let at = chrono::DateTime::from_timestamp_millis(resets_at).map(|d| d.with_timezone(&chrono::Local).format("%-I:%M%P").to_string()).unwrap_or_default();
        let message = format!("You've hit your session limit · resets {at}");
        self.emit(AgentEvent::LimitReached { message: message.clone(), resets_at: Some(resets_at), scope: crate::LimitScope::Session }).await?;
        let cost_usd = Some(self.spend(0.0));
        self.emit(AgentEvent::TurnComplete { cost_usd, error: Some(message) }).await
    }

    /// Add `usd` to the session's spend; returns the new total.
    fn spend(&mut self, usd: f64) -> f64 {
        self.cost += usd;
        self.cost
    }

    /// Wait `d`, handling whatever the user sends meanwhile.
    async fn pause(&mut self, d: Duration) -> Step {
        if d.is_zero() {
            loop {
                match self.commands.try_recv() {
                    Ok(cmd) => self.handle_midturn(Ok(cmd))?,
                    Err(async_channel::TryRecvError::Empty) => break,
                    Err(async_channel::TryRecvError::Closed) => return Err(Stop::Closed),
                }
            }
            tokio::task::yield_now().await;
            return Ok(());
        }
        let sleep = tokio::time::sleep(d);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return Ok(()),
                cmd = self.commands.recv() => self.handle_midturn(cmd)?,
            }
        }
    }

    /// Commands that arrive while a turn runs and no request is pending.
    fn handle_midturn(&mut self, cmd: std::result::Result<Command, async_channel::RecvError>) -> Step {
        match cmd {
            Ok(Command::Prompt { text, .. }) => {
                self.remember(&text);
                self.steer.push(text);
            }
            Ok(Command::Interrupt) => return Err(Stop::Interrupted),
            Ok(Command::SetHandHolding(h)) => self.hand_holding = h,
            Ok(Command::Shutdown) | Err(_) => return Err(Stop::Closed),
            Ok(Command::SetModel { .. } | Command::Respond { .. } | Command::Answer { .. }) => {}
        }
        Ok(())
    }

    /// Block until `request_id` is answered.
    async fn wait_reply(&mut self, request_id: &str) -> Step<Reply> {
        loop {
            match self.commands.recv().await {
                Ok(Command::Respond { request_id: r, decision }) if r == request_id => return Ok(Reply::Decision(decision)),
                Ok(Command::Answer { request_id: r, answers }) if r == request_id => return Ok(Reply::Answers(answers)),
                other => self.handle_midturn(other)?,
            }
        }
    }

    /// A prompt sent mid-turn is folded into the running turn, as Claude Code does.
    async fn acknowledge_steer(&mut self) -> Step {
        for text in std::mem::take(&mut self.steer) {
            self.say(&format!("Noted — {}. I've taken that into account.", text.trim().trim_end_matches('.'))).await?;
        }
        Ok(())
    }

    async fn think(&mut self, text: &str) -> Step {
        for chunk in tokens(text) {
            self.emit(AgentEvent::ReasoningDelta(chunk.into())).await?;
            self.pause(paced(10)).await?;
        }
        Ok(())
    }

    /// Stream `text` a token at a time, then send it whole (as agents do at the end of a message).
    async fn say(&mut self, text: &str) -> Step {
        for chunk in tokens(text) {
            self.emit(AgentEvent::TextDelta(chunk.into())).await?;
            self.pause(paced(14)).await?;
        }
        self.emit(AgentEvent::TextDone(text.into())).await
    }

    async fn tool_start(&mut self, id: &str, title: &str, detail: &str) -> Step {
        self.emit(AgentEvent::ToolStarted { id: id.into(), title: title.into(), detail: detail.into() }).await
    }

    async fn tool(&mut self, title: &str, detail: &str, output: &str, ms: u64) -> Step {
        let id = self.id("tool");
        self.tool_start(&id, title, detail).await?;
        self.pause(paced(ms)).await?;
        self.emit(AgentEvent::ToolFinished { id, output: output.into(), ok: true }).await
    }

    async fn tools(&mut self) -> Step {
        self.think("I should look around before changing anything.").await?;
        self.tool("Run command", "ls -la", LS_OUTPUT, 250).await?;
        self.tool("Read", "src/main.rs", MAIN_RS, 180).await?;
        self.tool("Search", "parse_args", "src/main.rs:3:    let cfg = parse_args();\nsrc/cli.rs:12:pub fn parse_args() -> Config {", 150).await?;
        self.acknowledge_steer().await?;
        self.say("The flag parsing lives in `src/cli.rs`; I'll add the `--verbose` flag there and wire it through `src/main.rs`.").await?;
        self.tool("Edit", "src/cli.rs", "Applied 1 edit to src/cli.rs", 220).await?;
        self.tool("Edit", "src/main.rs", "Applied 2 edits to src/main.rs", 200).await?;
        self.emit(AgentEvent::DiffStat { additions: 14, deletions: 3 }).await?;
        self.tool("Run command", "cargo test", TEST_OUTPUT, 600).await?;
        self.say("Added a `--verbose` flag:\n\n- `src/cli.rs` parses it into `Config::verbose`\n- `src/main.rs` raises the log level when it's set\n\nAll 14 tests pass.").await
    }

    /// Add a line to `NOTES.md` in the session's folder: a real change, for worktree reviews.
    async fn write(&mut self) -> Step {
        let path = self.cwd.join("NOTES.md");
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let n = before.lines().filter(|l| l.starts_with("- ")).count() + 1;
        let text = if before.is_empty() { format!("# Notes\n\n- Note {n}\n") } else { format!("{before}- Note {n}\n") };
        let id = self.id("tool");
        self.tool_start(&id, "Edit", "NOTES.md").await?;
        self.pause(paced(150)).await?;
        let (output, ok) = match std::fs::write(&path, &text) {
            Ok(()) => (format!("Wrote note {n} to NOTES.md"), true),
            Err(e) => (format!("Couldn't write NOTES.md: {e}"), false),
        };
        self.emit(AgentEvent::ToolFinished { id, output, ok }).await?;
        let added = (text.lines().count() - before.lines().count()) as i64;
        self.emit(AgentEvent::DiffStat { additions: added, deletions: 0 }).await?;
        self.say(&format!("Added note {n} to `NOTES.md`.")).await
    }

    async fn agents(&mut self, after: Option<Duration>) -> Step {
        let after = after.unwrap_or_else(|| paced(6_000));
        self.say("I'll send two scouts ahead: one maps the HTTP routes, the other audits error handling.").await?;
        let scouts = [("Map the HTTP routes", "Reading src/routes.rs"), ("Audit error handling", "Searching for unwrap()")];
        for (description, _) in scouts {
            let id = self.id("agent");
            self.tool_start(&id, "Subagent", description).await?;
            self.emit(AgentEvent::Task { id: id.clone(), description: Some(description.into()), activity: None, tool_uses: None, done: None }).await?;
            self.background.push((id, description.into()));
        }
        self.emit(AgentEvent::Background(self.background.len())).await?;
        for (id, _) in self.background.clone() {
            self.emit(AgentEvent::ToolFinished { id, output: "Async agent launched successfully.".into(), ok: true }).await?;
        }
        self.say("Both scouts are out. I'll pull their findings together when they report back.").await?;
        // The turn ends, but the thread keeps working until the background agents are back.
        let cost_usd = Some(self.spend(0.01));
        self.emit(AgentEvent::TurnComplete { cost_usd, error: None }).await?;
        let agents = self.background.clone();
        for (i, (id, _)) in agents.iter().enumerate() {
            self.pause(after / 3).await?;
            let activity = scouts[i].1;
            self.emit(AgentEvent::Task { id: id.clone(), description: None, activity: Some(activity.into()), tool_uses: Some(2 + i as u64 * 3), done: None }).await?;
        }
        for (id, _) in agents {
            self.pause(after / 3).await?;
            self.background.retain(|(b, _)| *b != id);
            self.emit(AgentEvent::Task { id, description: None, activity: None, tool_uses: None, done: Some(true) }).await?;
            self.emit(AgentEvent::Background(self.background.len())).await?;
        }
        self.say("Both scouts reported back:\n\n1. **Routes** — 12 handlers in `src/routes.rs`, two of them unauthenticated.\n2. **Errors** — 7 `unwrap()` calls on request input; I'd turn those into `400`s.").await?;
        self.finish().await
    }

    async fn permission(&mut self) -> Step {
        self.say("The schema change needs the migration script to run.").await?;
        let tool = self.id("tool");
        let command = "./scripts/migrate.sh --apply";
        self.tool_start(&tool, "Run command", command).await?;
        let ask = !self.commands_allowed && self.hand_holding != HandHolding::FullAccess;
        let decision = if ask {
            let request_id = self.id("permission");
            self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "Run command".into(), detail: command.into(), prompt: None }).await?;
            match self.wait_reply(&request_id).await? {
                Reply::Decision(d) => d,
                Reply::Answers(_) => Decision::Allow,
            }
        } else {
            Decision::Allow
        };
        if decision == Decision::AllowForSession {
            self.commands_allowed = true;
        }
        if decision == Decision::Deny {
            self.emit(AgentEvent::ToolFinished { id: tool, output: "The user declined this action.".into(), ok: false }).await?;
            return self.say("Okay, I won't run it. The migration is in `scripts/migrate.sh` if you want to apply it yourself.").await;
        }
        self.pause(paced(500)).await?;
        self.emit(AgentEvent::ToolFinished { id: tool, output: "Applying 3 migrations…\n✓ 0007_add_users_email\n✓ 0008_index_sessions\n✓ 0009_drop_legacy_tokens".into(), ok: true }).await?;
        self.say("Migrations applied: the `users` table has an `email` column and sessions are indexed.").await
    }

    async fn questions(&mut self) -> Step {
        self.think("Two choices only the user can make.").await?;
        let request_id = self.id("question");
        let questions = vec![
            Question {
                question: "Which database should the service use?".into(),
                header: "Database".into(),
                options: vec![
                    ("SQLite".into(), "One file next to the binary, zero setup.".into()),
                    ("Postgres".into(), "A server; better for many concurrent writers.".into()),
                ],
                multi: false,
                secret: false,
            },
            Question {
                question: "What should ship with it?".into(),
                header: "Extras".into(),
                options: vec![("Migrations".into(), String::new()), ("Seed data".into(), String::new()), ("Backups".into(), "Nightly, kept for a week.".into())],
                multi: true,
                secret: false,
            },
        ];
        self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "AskUserQuestion".into(), detail: String::new(), prompt: Some(Prompt::Questions(questions)) })
            .await?;
        match self.wait_reply(&request_id).await? {
            Reply::Answers(answers) => {
                let summary = answers.iter().map(|(q, a)| format!("- {q} **{a}**")).collect::<Vec<_>>().join("\n");
                self.say(&format!("Got it:\n\n{summary}\n\nSetting that up now.")).await
            }
            Reply::Decision(_) => self.say("No problem — I'll go with SQLite and migrations, the simplest setup.").await,
        }
    }

    async fn plan_turn(&mut self) -> Step {
        self.think("Read-only first: understand the layout, then propose a plan.").await?;
        self.tool("Read", "src/routes.rs", "pub fn routes() -> Router { … }", 200).await?;
        self.tool("Search", "fn handler", "src/routes.rs: 12 matches", 150).await?;
        let request_id = self.id("plan");
        self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "ExitPlanMode".into(), detail: String::new(), prompt: Some(Prompt::Plan(PLAN.into())) }).await?;
        match self.wait_reply(&request_id).await? {
            Reply::Decision(Decision::Deny) => self.say("Okay — I'll keep refining the plan. What should change?").await,
            _ => {
                self.plan = false;
                self.tool("Edit", "src/auth.rs", "Applied 1 edit to src/auth.rs", 250).await?;
                self.tool("Edit", "src/routes.rs", "Applied 2 edits to src/routes.rs", 250).await?;
                self.emit(AgentEvent::DiffStat { additions: 31, deletions: 6 }).await?;
                self.tool("Run command", "cargo test", TEST_OUTPUT, 500).await?;
                self.say("Implemented the plan: every route now goes through `require_session`, and the tests pass.").await
            }
        }
    }

    /// A long build: one command that runs for most of `total`, with a context update every ten
    /// seconds at demo pace (none at pace zero: tests would otherwise see them land depending on
    /// how fast the machine runs them). Steering is acknowledged within a quarter second.
    async fn long(&mut self, total: Duration) -> Step {
        let started = tokio::time::Instant::now();
        self.think("This needs the full test suite; it takes a while.").await?;
        let id = self.id("tool");
        self.tool_start(&id, "Run command", "cargo test --workspace").await?;
        let end = started + total;
        let every = paced(10_000);
        let mut next_context = if every.is_zero() { end } else { started + every };
        loop {
            let now = tokio::time::Instant::now();
            if now >= end {
                break;
            }
            self.pause((end - now).min(Duration::from_millis(250))).await?;
            self.acknowledge_steer().await?;
            if tokio::time::Instant::now() >= next_context && next_context < end {
                next_context += every;
                self.context += 800;
                self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await?;
            }
        }
        self.emit(AgentEvent::ToolFinished { id, output: TEST_OUTPUT.into(), ok: true }).await?;
        self.say(&format!("The full suite passed after {} — nothing to fix.", humanize(total))).await
    }

    /// One long answer, streamed a token at a time (as `say` does) for `total`: section after
    /// section of markdown with lists, code and paths.
    async fn stream(&mut self, total: Duration) -> Step {
        let end = tokio::time::Instant::now() + total;
        let mut text = String::new();
        let mut part = 0;
        loop {
            part += 1;
            let section = format!("{}## Part {part}\n\n{}\n\n", if part == 1 { "" } else { "\n" }, if part % 2 == 1 { ANSWER } else { PLAN });
            for chunk in tokens(&section) {
                self.emit(AgentEvent::TextDelta(chunk.into())).await?;
                text.push_str(chunk);
                if tokio::time::Instant::now() >= end {
                    return self.emit(AgentEvent::TextDone(text)).await;
                }
                // Real time, not the pace: the prompt asked for this long.
                self.pause(Duration::from_millis(14)).await?;
            }
        }
    }
}

fn humanize(d: Duration) -> String {
    match d.as_secs() {
        0 => format!("{} ms", d.as_millis()),
        s @ 1..=59 => format!("{s} s"),
        s => format!("{} min {} s", s / 60, s % 60),
    }
}

/// Split text the way a model streams it: short runs of a few characters, ending at word breaks.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        if (c.is_whitespace() && end - start >= 3) || end - start >= 12 {
            out.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

const ANSWER: &str = "## How the app starts\n\nStartup happens in `src/main.rs`: it parses the flags, loads the config and opens the window.\n\n- **Flags** are parsed in `src/cli.rs` into a `Config`.\n- **Settings** come from `config.toml`; anything missing falls back to the defaults.\n- The **window** is created last, so a bad config fails fast.\n\n```rust\nfn main() -> anyhow::Result<()> {\n    let cfg = cli::parse_args();\n    let settings = Settings::load(&cfg.config_path)?;\n    app::run(settings)\n}\n```\n\nIf you want to change the startup order, `app::run` in `src/app.rs` is the place.";

const PLAN: &str = "## Require a session on every route\n\n1. Add `require_session` middleware in `src/auth.rs`.\n2. Wrap the router in `src/routes.rs` with it, keeping `/health` public.\n3. Return `401` with a JSON body when the session is missing or expired.\n4. Add tests for an authenticated and an anonymous request.\n\nNo database changes.";

const LS_OUTPUT: &str = "total 48\ndrwxr-xr-x  8 me  staff   256 Oct  2 09:14 .\n-rw-r--r--  1 me  staff  1184 Oct  2 09:14 Cargo.toml\n-rw-r--r--  1 me  staff   912 Oct  2 09:14 README.md\ndrwxr-xr-x  5 me  staff   160 Oct  2 09:14 src\ndrwxr-xr-x  3 me  staff    96 Oct  2 09:14 tests";

const MAIN_RS: &str = "mod cli;\n\nfn main() {\n    let cfg = cli::parse_args();\n    app::run(cfg);\n}";

const TEST_OUTPUT: &str = "running 14 tests\n..............\ntest result: ok. 14 passed; 0 failed; 0 ignored; finished in 0.42s";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scripts_follow_keywords() {
        assert_eq!(Script::parse("explain the startup", false), Script::Answer);
        assert_eq!(Script::parse("Run the tools please", false), Script::Tools);
        assert_eq!(Script::parse("mock:long 2s", false), Script::Long(Duration::from_secs(2)));
        assert_eq!(Script::parse("mock:long", false), Script::Long(Duration::from_secs(30)));
        assert_eq!(Script::parse("how long is it?", false), Script::Answer, "bare `long` is just a word");
        assert_eq!(Script::parse("mock:stream 2m", false), Script::Stream(Duration::from_secs(120)));
        assert_eq!(Script::parse("stream the logs", false), Script::Answer);
        assert_eq!(Script::parse("send subagents 300ms", false), Script::Agents(Some(Duration::from_millis(300))));
        assert_eq!(Script::parse("ask a question", false), Script::Questions);
        assert_eq!(Script::parse("mock:permission", false), Script::Permission);
        assert_eq!(Script::parse("anything", true), Script::Plan);
        assert_eq!(Script::parse("Error!", false), Script::Error);
        assert_eq!(Script::parse("mock:limit 5s", false), Script::Limit(Duration::from_secs(5)));
        assert_eq!(Script::parse("mock:limit", false), Script::Limit(Duration::from_secs(5)));
        assert_eq!(Script::parse("the rate limit", false), Script::Answer, "bare `limit` is just a word");
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("10"), None);
        // Huge values are capped rather than overflowing.
        assert_eq!(parse_duration("999999999999999999m"), Some(MAX_DURATION));
        assert_eq!(parse_duration("99999999999s"), Some(MAX_DURATION));
        assert_eq!(parse_duration("99999999999999999999999s"), None, "not a u64");
    }

    #[test]
    fn titles_follow_the_script() {
        assert_eq!(title("explain the startup"), "How the app starts");
        assert_eq!(title("mock:long 5s"), "Run the full test suite");
        assert_eq!(title("ask a question"), "Choose a database");
        assert_eq!(title("mock:write"), "Add a note");
        assert_eq!(title("write it down"), "How the app starts", "only `mock:write` writes");
    }

    #[test]
    fn tokens_rebuild_the_text() {
        for text in [ANSWER, PLAN, "short", "ünïcödé words stream fine"] {
            assert_eq!(tokens(text).concat(), text);
            assert!(tokens(text).iter().all(|t| !t.is_empty()));
        }
    }

    struct Live {
        commands: async_channel::Sender<Command>,
        events: async_channel::Receiver<AgentEvent>,
    }

    impl Live {
        fn start(hand_holding: HandHolding, plan: bool) -> Live {
            set_pace(0.);
            let config = SessionConfig {
                agent: trek_core::AgentId::Direct(PROVIDER.into()),
                cwd: PathBuf::from("/tmp"),
                model: None,
                effort: trek_core::Effort::Low,
                hand_holding,
                plan,
                resume: None,
                resume_at: None,
                fork: false,
                recap: None,
                fast: None,
                mcp_servers: vec![],
            };
            let h = crate::start(config);
            Live { commands: h.commands, events: h.events }
        }

        async fn send(&self, cmd: Command) {
            self.commands.send(cmd).await.unwrap();
        }

        async fn prompt(&self, text: &str) {
            self.send(Command::Prompt { text: text.into(), images: vec![] }).await;
        }

        /// Events up to and including the first one `stop` accepts.
        async fn until(&self, stop: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
            let mut out = vec![];
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(10), self.events.recv()).await.expect("mock stalled").expect("mock exited");
                let done = stop(&ev);
                out.push(ev);
                if done {
                    return out;
                }
            }
        }

        async fn turn(&self) -> Vec<AgentEvent> {
            self.until(|e| matches!(e, AgentEvent::TurnComplete { .. })).await
        }
    }

    fn text(events: &[AgentEvent]) -> String {
        events.iter().filter_map(|e| if let AgentEvent::TextDone(t) = e { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    fn config(resume: Option<&str>, at: Option<&str>, fork: bool, recap: Option<&str>) -> SessionConfig {
        SessionConfig {
            agent: trek_core::AgentId::Direct(PROVIDER.into()),
            cwd: PathBuf::from("/tmp"),
            model: None,
            effort: trek_core::Effort::Low,
            hand_holding: HandHolding::Auto,
            plan: false,
            resume: resume.map(String::from),
            resume_at: at.map(String::from),
            fork,
            recap: recap.map(String::from),
            fast: None,
            mcp_servers: vec![],
        }
    }

    #[test]
    fn sessions_resume_partway_fork_or_fall_back_to_a_recap() {
        let said = |m: &str, t: &str| (m.to_string(), t.to_string());
        HISTORY.lock().unwrap().insert("s".into(), vec![said("m1", "one"), said("m2", "two"), said("m2", "steered"), said("m3", "three")]);
        // A fork through a turn copies the history up to it; the session is left as it was.
        let (fork, at, recap) = open_session(&config(Some("s"), Some("m2"), true, Some("recap")));
        assert!(fork != "s" && at.as_deref() == Some("m2") && recap.is_none());
        assert_eq!(remembered(&fork), ["one", "two", "steered"]);
        assert_eq!(remembered("s").len(), 4);
        // In place: the same session, cut back.
        let (id, at, _) = open_session(&config(Some("s"), Some("m1"), false, None));
        assert_eq!((id.as_str(), at.as_deref()), ("s", Some("m1")));
        assert_eq!(remembered("s"), ["one"]);
        // A point it doesn't have: a new session, with the recap to go with the first message.
        let (id, at, recap) = open_session(&config(Some("s"), Some("gone"), false, Some("User: one")));
        assert!(id != "s" && at.is_none() && recap.as_deref() == Some("User: one"));
        assert!(remembered(&id).is_empty());
        // Resumed whole, and new.
        assert_eq!(open_session(&config(Some("s"), None, false, None)).0, "s");
        assert!(open_session(&config(None, None, false, None)).0.starts_with("mock-session-"));
    }

    #[test]
    fn a_session_taken_back_remembers_only_what_was_kept() {
        trek_core::runtime().block_on(async {
            set_pace(0.);
            let open = |c: SessionConfig| {
                let h = crate::start(c);
                Live { commands: h.commands, events: h.events }
            };
            let m = open(config(None, None, false, None));
            let started = m.until(|e| matches!(e, AgentEvent::Started { .. })).await;
            let Some(AgentEvent::Started { native_id, .. }) = started.last() else { panic!() };
            let mark = |events: &[AgentEvent]| events.iter().find_map(|e| if let AgentEvent::Mark(m) = e { Some(m.clone()) } else { None }).unwrap();
            m.prompt("apple").await;
            let first = mark(&m.turn().await);
            m.prompt("banana").await;
            m.turn().await;
            m.prompt("recall").await;
            assert_eq!(text(&m.turn().await), "I remember: apple | banana");
            m.send(Command::Shutdown).await;

            // Cut back to the first turn: the second is forgotten.
            let back = open(config(Some(native_id), Some(&first), false, None));
            let started = back.until(|e| matches!(e, AgentEvent::Mark(_))).await;
            assert!(started.contains(&AgentEvent::Started { native_id: native_id.clone(), model: Some("mock-swift".into()) }));
            back.prompt("recall").await;
            assert_eq!(text(&back.turn().await), "I remember: apple");

            // A new session primed with a recap remembers what the recap says.
            let recapped = open(config(None, None, false, Some("User: apple\n\nAssistant: OK\n\nUser: cherry")));
            recapped.prompt("recall").await;
            assert_eq!(text(&recapped.turn().await), "I remember: apple | cherry");
        });
    }

    #[test]
    fn plays_an_answer_with_streamed_text() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Supervised, false);
            let start = m.until(|e| matches!(e, AgentEvent::Context { .. })).await;
            assert!(matches!(&start[0], AgentEvent::Started { native_id, .. } if native_id.starts_with("mock-")));
            m.prompt("explain").await;
            let events = m.turn().await;
            let streamed: String = events.iter().filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
            assert_eq!(streamed, ANSWER);
            assert!(events.iter().filter(|e| matches!(e, AgentEvent::TextDelta(_))).count() > 20, "streams token by token");
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ReasoningDelta(_))));
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
        });
    }

    #[test]
    fn cost_is_a_running_total_that_nobody_pays() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let start = m.until(|e| matches!(e, AgentEvent::Context { .. })).await;
            assert!(start.contains(&AgentEvent::Billing(crate::Billing::Local)), "{start:?}");
            let cost = |events: Vec<AgentEvent>| match events.last() {
                Some(AgentEvent::TurnComplete { cost_usd: Some(c), .. }) => *c,
                other => panic!("{other:?}"),
            };
            m.prompt("explain").await;
            let first = cost(m.turn().await);
            m.prompt("and again").await;
            let second = cost(m.turn().await);
            assert!(second > first && first > 0., "{first} then {second}");
        });
    }

    #[test]
    fn permission_waits_for_the_user() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Supervised, false);
            m.prompt("mock:permission").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: None, .. }) = events.last() else { panic!("{events:?}") };
            m.send(Command::Respond { request_id: request_id.clone(), decision: Decision::Deny }).await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: false, .. })));
            assert!(text(&events).contains("won't run it"));
            // Full access: no prompt at all.
            m.send(Command::SetHandHolding(HandHolding::FullAccess)).await;
            m.prompt("permission again").await;
            let events = m.turn().await;
            assert!(!events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })));
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: true, .. })));
        });
    }

    #[test]
    fn questions_take_answers() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("a question for you").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Questions(q)), .. }) = events.last() else { panic!("{events:?}") };
            assert_eq!(q.len(), 2);
            assert!(q[1].multi);
            m.send(Command::Answer { request_id: request_id.clone(), answers: vec![(q[0].question.clone(), "Postgres".into())] }).await;
            assert!(text(&m.turn().await).contains("**Postgres**"));
        });
    }

    #[test]
    fn plan_mode_ends_in_a_plan() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, true);
            m.prompt("add auth").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Plan(plan)), .. }) = events.last() else { panic!("{events:?}") };
            assert!(plan.contains("require_session"));
            m.send(Command::Respond { request_id: request_id.clone(), decision: Decision::Allow }).await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::DiffStat { .. })));
            // Approving leaves plan mode: the next prompt is answered normally.
            m.prompt("thanks").await;
            assert!(!m.turn().await.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })));
        });
    }

    #[test]
    fn background_agents_finish_after_the_turn() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("subagents 30ms").await;
            let first = m.turn().await;
            assert!(first.contains(&AgentEvent::Background(2)));
            let tasks = first.iter().filter(|e| matches!(e, AgentEvent::Task { description: Some(_), .. })).count();
            assert_eq!(tasks, 2);
            let rest = m.turn().await;
            assert!(rest.contains(&AgentEvent::Background(0)));
            assert_eq!(rest.iter().filter(|e| matches!(e, AgentEvent::Task { done: Some(true), .. })).count(), 2);
            assert!(text(&rest).contains("reported back"));
        });
    }

    #[test]
    fn long_turns_stop_on_interrupt_and_take_steering() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:long 300ms").await;
            m.prompt("use tabs").await;
            let events = m.turn().await;
            assert!(text(&events).contains("Noted — use tabs"));
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
            // As long as a prompt can ask for: the turn runs (no overflow) until stopped.
            m.prompt("mock:long 99999999999s").await;
            m.until(|e| matches!(e, AgentEvent::ToolStarted { .. })).await;
            m.send(Command::Interrupt).await;
            let events = m.turn().await;
            assert_eq!(events.last(), Some(&AgentEvent::TurnComplete { cost_usd: None, error: Some("Interrupted".into()) }));
        });
    }

    #[test]
    fn streams_one_answer_for_as_long_as_asked() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let start = std::time::Instant::now();
            m.prompt("mock:stream 300ms").await;
            let events = m.turn().await;
            assert!(start.elapsed() >= Duration::from_millis(300));
            let streamed: String = events.iter().filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
            assert!(streamed.starts_with("## Part 1\n\n## How the app starts"), "{streamed}");
            assert_eq!(text(&events), streamed, "one message, sent whole at the end");
            assert!(events.iter().filter(|e| matches!(e, AgentEvent::TextDelta(_))).count() > 10);
        });
    }

    #[test]
    fn errors_fail_the_turn() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:error").await;
            let events = m.turn().await;
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: Some(_), .. })));
        });
    }

    #[test]
    fn a_limit_holds_until_it_resets() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let started = m.until(|e| matches!(e, AgentEvent::Started { .. })).await;
            let Some(AgentEvent::Started { native_id, .. }) = started.last() else { panic!() };
            let before = trek_core::store::now_ms();
            m.prompt("mock:limit 5s").await;
            let events = m.turn().await;
            let Some(AgentEvent::LimitReached { message, resets_at: Some(at), scope }) = events.iter().find(|e| matches!(e, AgentEvent::LimitReached { .. })) else { panic!("{events:?}") };
            assert!(*at >= before + 5_000 && *at < before + 7_000);
            assert_eq!(*scope, crate::LimitScope::Session);
            assert!(message.starts_with("You've hit your session limit · resets "));
            assert_eq!(events.last(), Some(&AgentEvent::TurnComplete { cost_usd: Some(0.0), error: Some(message.clone()) }));
            // Anything else meets the same limit until it resets.
            m.prompt("explain the startup").await;
            let again = m.turn().await;
            assert!(again.contains(&AgentEvent::LimitReached { message: message.clone(), resets_at: Some(*at), scope: crate::LimitScope::Session }));
            lift_limit(native_id);
            m.prompt("explain the startup").await;
            assert!(matches!(m.turn().await.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
        });
    }
}
