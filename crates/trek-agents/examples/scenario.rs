//! Live checks of approvals, questions, plans, steering, resume and sub-agents through Trek's
//! own drivers, with tiny prompts on cheap models:
//! `cargo run -p trek-agents --example scenario -- <agent> <scenario>`
//!
//! codex:    approvals | steer | plan | plan-restart | resume | subagent | interrupt | lost-thread
//! opencode: approvals | resume | interrupt | plan | model
//! claude:   plan | keep-planning | question | cancel | interrupt | steer | lost-session
//! both:     rewind | fork (`claude` or `codex`)
//!
//! Each scenario works in its own folder under /tmp/trek-agents-e2e (or `$TREK_E2E_DIR`), keeps
//! what Trek would save there too, and exits non-zero when a check fails.
use std::path::{Path, PathBuf};
use std::time::Duration;
use trek_agents::{AgentEvent, Command, Decision, Prompt, SessionConfig, SessionHandle, start};
use trek_core::{AgentId, Effort, HandHolding};

const CODEX: &str = "gpt-5.6-luna";
const OPENCODE: &str = "opencode/mimo-v2.6-flash-free";
const CLAUDE: &str = "claude-haiku-4-5";

struct Session {
    h: SessionHandle,
    seen: Vec<AgentEvent>,
}

impl Session {
    fn start(agent: AgentId, cwd: &Path, model: &str, hand_holding: HandHolding, plan: bool, resume: Option<String>) -> Self {
        Self::open(agent, cwd, model, hand_holding, plan, resume, None, false)
    }

    /// `start`, resuming partway (`resume_at`) or as a fork.
    #[allow(clippy::too_many_arguments)]
    fn open(agent: AgentId, cwd: &Path, model: &str, hand_holding: HandHolding, plan: bool, resume: Option<String>, resume_at: Option<String>, fork: bool) -> Self {
        let h = start(SessionConfig {
            agent,
            cwd: cwd.to_path_buf(),
            model: Some(model.into()),
            effort: Effort::Low,
            hand_holding,
            plan,
            read_only: false,
            resume,
            resume_at,
            fork,
            recap: None,
            fast: None,
            mcp_servers: vec![],
        });
        Session { h, seen: vec![] }
    }

    /// The latest point the session can be taken back to.
    fn mark(&self) -> Option<String> {
        self.seen.iter().rev().find_map(|e| if let AgentEvent::Mark(m) = e { Some(m.clone()) } else { None })
    }

    async fn send(&self, cmd: Command) {
        println!("  >> {}", short(&format!("{cmd:?}")));
        self.h.commands.send(cmd).await.unwrap();
    }

    async fn prompt(&self, text: &str) {
        self.send(Command::Prompt { text: text.into(), images: vec![] }).await;
    }

    /// Events until `f` matches (returned), printing all but streamed text.
    async fn until(&mut self, secs: u64, f: impl Fn(&AgentEvent) -> bool) -> AgentEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let ev = match tokio::time::timeout_at(deadline, self.h.events.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => fail("session ended"),
                Err(_) => fail(&format!("timed out after {secs}s")),
            };
            if !matches!(ev, AgentEvent::TextDelta(_) | AgentEvent::ReasoningDelta(_)) {
                println!("  << {}", short(&format!("{ev:?}")));
            }
            self.seen.push(ev.clone());
            if f(&ev) {
                return ev;
            }
            if matches!(ev, AgentEvent::Exited) {
                fail("agent exited");
            }
        }
    }

    async fn turn(&mut self, secs: u64) -> Option<String> {
        match self.until(secs, |e| matches!(e, AgentEvent::TurnComplete { .. } | AgentEvent::Error(_))).await {
            AgentEvent::TurnComplete { error, .. } => error,
            AgentEvent::Error(e) => Some(e),
            _ => unreachable!(),
        }
    }

    async fn permission(&mut self, secs: u64) -> (String, String, String, Option<Prompt>) {
        match self.until(secs, |e| matches!(e, AgentEvent::PermissionRequest { .. })).await {
            AgentEvent::PermissionRequest { request_id, title, detail, prompt } => (request_id, title, detail, prompt),
            _ => unreachable!(),
        }
    }

    fn text(&self) -> String {
        self.seen.iter().filter_map(|e| if let AgentEvent::TextDone(t) = e { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    fn native_id(&self) -> String {
        self.seen.iter().find_map(|e| if let AgentEvent::Started { native_id, .. } = e { Some(native_id.clone()) } else { None }).unwrap_or_default()
    }

    fn count(&self, f: impl Fn(&AgentEvent) -> bool) -> usize {
        self.seen.iter().filter(|e| f(e)).count()
    }

    async fn stop(self) {
        let _ = self.h.commands.send(Command::Shutdown).await;
        while let Ok(ev) = self.h.events.recv().await {
            if matches!(ev, AgentEvent::Exited) {
                break;
            }
        }
    }
}

fn short(s: &str) -> String {
    let s = s.replace('\n', "⏎");
    if s.chars().count() > 220 { format!("{}…", s.chars().take(220).collect::<String>()) } else { s }
}

fn fail(why: &str) -> ! {
    println!("FAIL: {why}");
    std::process::exit(1)
}

fn check(ok: bool, what: &str) {
    if ok {
        println!("ok: {what}");
    } else {
        fail(what);
    }
}

fn base() -> PathBuf {
    PathBuf::from(std::env::var("TREK_E2E_DIR").unwrap_or_else(|_| "/tmp/trek-agents-e2e".into()))
}

fn folder(name: &str) -> PathBuf {
    let dir = base().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (agent, scenario) = match &args[..] {
        [a, s] => (a.as_str(), s.as_str()),
        _ => fail("usage: scenario <codex|opencode|claude> <scenario>"),
    };
    // What the drivers save (what ACP agents report, say) stays out of the user's Trek data.
    trek_core::paths::isolate(base().join("trek-data"));
    trek_core::runtime().block_on(async {
        match (agent, scenario) {
            ("codex", "approvals") => approvals(AgentId::Codex, CODEX, "codex-approvals").await,
            ("opencode", "approvals") => approvals(AgentId::OpenCode, OPENCODE, "opencode-approvals").await,
            ("codex", "steer") => codex_steer().await,
            ("codex", "plan") => codex_plan().await,
            ("codex", "plan-restart") => codex_plan_restart().await,
            ("codex", "resume") => resume(AgentId::Codex, CODEX, Some("gpt-6-luna"), "codex-resume").await,
            ("opencode", "resume") => resume(AgentId::OpenCode, OPENCODE, None, "opencode-resume").await,
            ("codex", "subagent") => codex_subagent().await,
            ("codex", "interrupt") => interrupt(AgentId::Codex, CODEX, "codex-interrupt").await,
            ("codex", "lost-thread") => codex_lost_thread().await,
            ("opencode", "interrupt") => interrupt(AgentId::OpenCode, OPENCODE, "opencode-interrupt").await,
            ("opencode", "plan") => opencode_plan().await,
            ("opencode", "model") => opencode_model().await,
            ("claude", "plan") => claude_plan(true).await,
            ("claude", "keep-planning") => claude_plan(false).await,
            ("claude", "question") => claude_question().await,
            ("claude", "cancel") => claude_cancel().await,
            ("claude", "interrupt") => interrupt(AgentId::ClaudeCode, CLAUDE, "claude-interrupt").await,
            ("claude", "steer") => claude_steer().await,
            ("claude", "lost-session") => claude_lost_session().await,
            ("claude", "rewind") => rewind(AgentId::ClaudeCode, CLAUDE, "claude-rewind").await,
            ("codex", "rewind") => rewind(AgentId::Codex, CODEX, "codex-rewind").await,
            ("claude", "fork") => fork(AgentId::ClaudeCode, CLAUDE, "claude-fork").await,
            ("codex", "fork") => fork(AgentId::Codex, CODEX, "codex-fork").await,
            _ => fail("unknown scenario"),
        }
    });
    println!("PASS");
}

const RECALL: &str = "List every word I asked you to remember, comma separated, nothing else.";

/// Two turns, each with a word to remember (APPLE, then BANANA). Returns the session and the
/// point just after the first turn.
async fn two_words(agent: &AgentId, model: &str, cwd: &Path) -> (String, String) {
    let mut s = Session::start(agent.clone(), cwd, model, HandHolding::Supervised, false, None);
    s.prompt("Remember the word APPLE. Reply with just OK.").await;
    check(s.turn(180).await.is_none(), "first turn");
    let first = s.mark().unwrap_or_else(|| fail("no point to take the session back to"));
    s.prompt("Also remember the word BANANA. Reply with just OK.").await;
    check(s.turn(180).await.is_none(), "second turn");
    check(s.mark().is_some_and(|m| m != first), "the second turn moved the point on");
    let id = s.native_id();
    s.stop().await;
    (id, first)
}

/// Ask what the session remembers: (its answer, uppercased; the session's id).
async fn words(s: &mut Session) -> (String, String) {
    s.prompt(RECALL).await;
    check(s.turn(180).await.is_none(), "recall turn");
    (s.text().to_uppercase(), s.native_id())
}

/// Cut a session back to just after its first turn, in place: it forgets the second.
async fn rewind(agent: AgentId, model: &str, name: &str) {
    let cwd = folder(name);
    let (id, first) = two_words(&agent, model, &cwd).await;
    let mut s = Session::open(agent.clone(), &cwd, model, HandHolding::Supervised, false, Some(id.clone()), Some(first.clone()), false);
    let (text, native) = words(&mut s).await;
    check(text.contains("APPLE") && !text.contains("BANANA"), &format!("cut back to the first turn: {text}"));
    check(native == id, "the same session");
    check(s.seen.iter().any(|e| *e == AgentEvent::Mark(first.clone())), "the cut is reported as the latest point");
    s.stop().await;
    // It stays cut back: resumed again, it still knows only the first word.
    let mut s = Session::start(agent, &cwd, model, HandHolding::Supervised, false, Some(id));
    s.prompt("Which words have I asked you to remember so far? Comma separated, nothing else.").await;
    check(s.turn(180).await.is_none(), "resumed turn");
    check(!s.text().to_uppercase().contains("BANANA"), "the cut held");
    s.stop().await;
}

/// Fork a session after its first turn, and whole: the copies know what they should, and the
/// original keeps everything.
async fn fork(agent: AgentId, model: &str, name: &str) {
    let cwd = folder(name);
    let (id, first) = two_words(&agent, model, &cwd).await;
    let mut s = Session::open(agent.clone(), &cwd, model, HandHolding::Supervised, false, Some(id.clone()), Some(first), true);
    let (text, native) = words(&mut s).await;
    check(text.contains("APPLE") && !text.contains("BANANA"), &format!("forked after the first turn: {text}"));
    check(!native.is_empty() && native != id, "a new session");
    s.stop().await;
    let mut s = Session::open(agent.clone(), &cwd, model, HandHolding::Supervised, false, Some(id.clone()), None, true);
    let (text, native) = words(&mut s).await;
    check(text.contains("APPLE") && text.contains("BANANA"), &format!("a whole fork: {text}"));
    check(native != id, "another new session");
    s.stop().await;
    let mut s = Session::start(agent, &cwd, model, HandHolding::Supervised, false, Some(id.clone()));
    let (text, native) = words(&mut s).await;
    check(text.contains("APPLE") && text.contains("BANANA"), &format!("the original is untouched: {text}"));
    check(native == id, "the original session");
    s.stop().await;
}

/// Supervised: three harmless writes, answered Allow, Deny, Allow for session.
async fn approvals(agent: AgentId, model: &str, name: &str) {
    let cwd = folder(name);
    let mut s = Session::start(agent.clone(), &cwd, model, HandHolding::Supervised, false, None);
    let decisions = [Decision::Allow, Decision::Deny, Decision::AllowForSession];
    for (i, file) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
        // OpenCode ends its turn when a call is rejected, so each write is its own turn.
        s.prompt(&format!("Run the shell command `touch {file}` (it needs your approval). If it's refused, don't retry; just reply with one word: done.")).await;
        let (rid, title, detail, _) = s.permission(180).await;
        check(title == "Run command" && detail.contains(file), &format!("asks to run `{detail}`"));
        s.send(Command::Respond { request_id: rid, decision: decisions[i] }).await;
        let err = s.turn(180).await;
        check(err.is_none(), "turn ends cleanly");
    }
    check(cwd.join("a.txt").exists() && !cwd.join("b.txt").exists() && cwd.join("c.txt").exists(), "allowed files exist, denied one doesn't");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { .. })) == 3, "one TurnComplete per turn");
    check(s.count(|e| matches!(e, AgentEvent::Context { .. })) > 0, "context usage reported");
    s.stop().await;
}

async fn codex_steer() {
    let cwd = folder("codex-steer");
    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::FullAccess, false, None);
    s.prompt("Run the shell command `sleep 6 && echo slept`, then reply with one short sentence.").await;
    s.until(120, |e| matches!(e, AgentEvent::ToolStarted { title, .. } if title == "Run command")).await;
    s.prompt("Also end your reply with the word BANANA.").await;
    check(s.turn(180).await.is_none(), "steered turn finishes cleanly");
    check(s.text().contains("BANANA"), "the steer reached the running turn");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { .. })) == 1, "steering didn't start a second turn");
    // The next message is a new turn: the old turn id was cleared.
    s.prompt("Reply with just the word: second").await;
    check(s.turn(120).await.is_none(), "next turn starts and finishes");
    check(s.text().to_lowercase().contains("second"), "second turn answered");
    check(!s.seen.iter().any(|e| matches!(e, AgentEvent::Error(_))), "no errors");
    s.stop().await;
}

async fn codex_plan() {
    let cwd = folder("codex-plan");
    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::Supervised, true, None);
    s.prompt("Plan adding a file hello.txt to this folder. Before writing the plan, ask me one short multiple-choice question about what the file should contain.").await;
    let (rid, _, _, prompt) = s.permission(180).await;
    let Some(Prompt::Questions(q)) = prompt else { fail("expected a question") };
    check(!q.is_empty() && !q[0].options.is_empty(), "question with options");
    // "Other": the user's own words rather than an option.
    s.send(Command::Answer { request_id: rid, answers: vec![(q[0].question.clone(), "Exactly this line: hi from Trek".into())] }).await;
    let (rid, _, _, prompt) = s.permission(180).await;
    let Some(Prompt::Plan(plan)) = prompt else { fail("expected a plan") };
    check(plan.contains("hi from Trek"), "the plan uses the typed answer");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { error: None, .. })) == 1, "plan offered after the turn ended");
    check(!cwd.join("hello.txt").exists(), "nothing written while planning");
    s.send(Command::Respond { request_id: rid, decision: Decision::Allow }).await;
    // Implementing in Supervised: the write needs approval.
    loop {
        match s.until(240, |e| matches!(e, AgentEvent::PermissionRequest { .. } | AgentEvent::TurnComplete { .. })).await {
            AgentEvent::PermissionRequest { request_id, .. } => s.send(Command::Respond { request_id, decision: Decision::Allow }).await,
            AgentEvent::TurnComplete { error, .. } => {
                check(error.is_none(), "implementation turn finishes");
                break;
            }
            _ => unreachable!(),
        }
    }
    let text = std::fs::read_to_string(cwd.join("hello.txt")).unwrap_or_default();
    check(text.contains("hi from Trek"), "approved plan was implemented");
    s.stop().await;
}

/// The plan waits while the app-server restarts (Trek relaunches idle sessions when plan mode or
/// fast mode changes): a new session, still started in plan mode, takes the approval.
async fn codex_plan_restart() {
    let cwd = folder("codex-plan-restart");
    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::FullAccess, true, None);
    s.prompt("Plan adding a file note.txt to this folder containing the single word: restarted. Don't ask me anything; keep the plan to three lines.").await;
    let (rid, _, _, prompt) = s.permission(240).await;
    check(matches!(prompt, Some(Prompt::Plan(_))), "plan offered after the turn");
    check(s.count(|e| matches!(e, AgentEvent::ToolStarted { title, .. } if title == "Plan")) == 1, "the plan shows on a Plan row");
    check(!s.seen.iter().any(|e| matches!(e, AgentEvent::TextDone(t) if t.contains("note.txt"))), "and not again as the reply");
    let thread = s.native_id();
    s.stop().await;

    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::FullAccess, true, Some(thread.clone()));
    s.until(120, |e| matches!(e, AgentEvent::Started { .. })).await;
    check(s.native_id() == thread, "resumed the thread");
    s.send(Command::Respond { request_id: rid, decision: Decision::Allow }).await;
    check(s.turn(240).await.is_none(), "implementation turn finishes");
    let text = std::fs::read_to_string(cwd.join("note.txt")).unwrap_or_default();
    check(text.contains("restarted"), "the plan was implemented, out of plan mode");
    s.stop().await;
}

async fn resume(agent: AgentId, model: &str, switch_to: Option<&str>, name: &str) {
    let cwd = folder(name);
    let mut s = Session::start(agent.clone(), &cwd, model, HandHolding::Supervised, false, None);
    s.prompt("Remember the code word MANGO. Reply with just: ok").await;
    check(s.turn(180).await.is_none(), "first turn");
    let id = s.native_id();
    check(!id.is_empty(), &format!("native id {id}"));
    s.stop().await;

    let mut s = Session::start(agent, &cwd, model, HandHolding::Supervised, false, Some(id.clone()));
    s.until(120, |e| matches!(e, AgentEvent::Started { .. })).await;
    check(s.native_id() == id, "resumed the same session");
    if let Some(m) = switch_to {
        s.send(Command::SetModel { model: m.into(), effort: Effort::Medium }).await;
    }
    s.prompt("What was the code word I asked you to remember? Reply with just the word.").await;
    check(s.turn(180).await.is_none(), "resumed turn");
    check(s.text().contains("MANGO"), "remembers the earlier turn");
    check(s.count(|e| matches!(e, AgentEvent::Context { .. })) > 0, "context usage reported");
    s.stop().await;
    println!("thread {id}");
}

/// A thread Codex no longer has (its rollout is gone) opens as a new one instead of failing.
/// Sends no prompt.
async fn codex_lost_thread() {
    let cwd = folder("codex-lost-thread");
    let gone = "01a0fe4e-0000-7000-8000-000000000000";
    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::Supervised, false, Some(gone.into()));
    s.until(60, |e| matches!(e, AgentEvent::Started { .. })).await;
    check(!s.native_id().is_empty() && s.native_id() != gone, "started a new thread");
    let AgentEvent::Notice(why) = s.until(10, |e| matches!(e, AgentEvent::Notice(_))).await else { unreachable!() };
    check(why.contains("without the earlier context"), "the user is told the earlier context is gone");
    s.stop().await;
}

async fn codex_subagent() {
    let cwd = folder("codex-subagent");
    let mut s = Session::start(AgentId::Codex, &cwd, CODEX, HandHolding::FullAccess, false, None);
    s.prompt("Spawn exactly one sub-agent whose task is: 'Reply with just the word pong.' Wait for it to finish, then tell me in one line what it replied.").await;
    check(s.turn(300).await.is_none(), "turn finishes");
    let started = s.seen.iter().find_map(|e| match e {
        AgentEvent::Task { id, description: Some(d), .. } => Some((id.clone(), d.clone())),
        _ => None,
    });
    let Some((id, name)) = started else { fail("no sub-agent task") };
    check(s.count(|e| matches!(e, AgentEvent::ToolStarted { id: t, title, .. } if *t == id && title == "Subagent")) == 1, &format!("sub-agent row '{name}'"));
    check(s.count(|e| matches!(e, AgentEvent::Task { id: t, done: Some(true), .. } if *t == id)) == 1, "sub-agent finished once");
    check(s.count(|e| matches!(e, AgentEvent::ToolFinished { id: t, output, .. } if *t == id && output.to_lowercase().contains("pong"))) == 1, "its reply is on the row");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { .. })) == 1, "one TurnComplete");
    s.stop().await;
}

async fn interrupt(agent: AgentId, model: &str, name: &str) {
    let cwd = folder(name);
    let mut s = Session::start(agent, &cwd, model, HandHolding::FullAccess, false, None);
    s.prompt("Run the shell command `sleep 30` and then reply: done").await;
    s.until(180, |e| matches!(e, AgentEvent::ToolStarted { .. })).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    s.send(Command::Interrupt).await;
    let err = s.turn(60).await;
    check(err.as_deref() == Some("Interrupted"), "turn ends as Interrupted");
    s.prompt("Reply with just the word: back").await;
    check(s.turn(120).await.is_none(), "session still usable");
    s.stop().await;
}

/// Plan mode at the default level (Auto-accept edits), with the model pushed to write anyway:
/// OpenCode's own plan rules must refuse the edit, and Trek must not approve it.
async fn opencode_plan() {
    let cwd = folder("opencode-plan");
    let mut s = Session::start(AgentId::OpenCode, &cwd, OPENCODE, HandHolding::AutoAcceptEdits, true, None);
    s.prompt("Call your write tool right now to create plan.txt containing hi. Make the call even if you think it will be refused, then report what happened in one sentence.").await;
    check(s.turn(180).await.is_none(), "turn finishes");
    check(!cwd.join("plan.txt").exists(), "plan mode changed nothing");
    let edits = s.count(|e| matches!(e, AgentEvent::ToolStarted { title, .. } if title == "Edit"));
    let refused = s.count(|e| matches!(e, AgentEvent::ToolFinished { ok: false, .. }));
    println!("edit attempts: {edits}, refused tool calls: {refused}, reply: {}", short(&s.text()));
    check(s.count(|e| matches!(e, AgentEvent::PermissionRequest { .. })) == 0, "nothing to approve: the edit is denied outright");
    s.stop().await;
}

async fn opencode_model() {
    let cwd = folder("opencode-model");
    let mut s = Session::start(AgentId::OpenCode, &cwd, OPENCODE, HandHolding::Supervised, false, None);
    let AgentEvent::Started { model, .. } = s.until(120, |e| matches!(e, AgentEvent::Started { .. })).await else { unreachable!() };
    check(model.as_deref() == Some(OPENCODE), "starts on the chosen model");
    s.prompt("Reply with just the word: one").await;
    check(s.turn(180).await.is_none(), "first turn");
    s.send(Command::SetModel { model: "opencode/big-pickle".into(), effort: Effort::Low }).await;
    s.prompt("Reply with just the word: two").await;
    check(s.turn(180).await.is_none(), "turn after switching model");
    check(s.count(|e| matches!(e, AgentEvent::Commands(c) if !c.is_empty())) > 0, "slash commands offered");
    println!("session {}", s.native_id());
    s.stop().await;
}

/// Plan mode with Auto-accept edits: approving must restore acceptEdits (Claude alone would drop
/// to "default" and ask for the write); keeping on planning must change nothing.
async fn claude_plan(approve: bool) {
    let cwd = folder(if approve { "claude-plan" } else { "claude-keep-planning" });
    let mut s = Session::start(AgentId::ClaudeCode, &cwd, CLAUDE, HandHolding::AutoAcceptEdits, true, None);
    s.prompt("Create a file named plan_test.txt in the current folder containing the single word ok. Keep the plan to one line, then exit plan mode.").await;
    let (rid, _, _, prompt) = s.permission(180).await;
    check(matches!(prompt, Some(Prompt::Plan(_))), "plan offered");
    let decision = if approve { Decision::Allow } else { Decision::Deny };
    s.send(Command::Respond { request_id: rid, decision }).await;
    let asked = s.count(|e| matches!(e, AgentEvent::PermissionRequest { .. }));
    check(s.turn(240).await.is_none(), "turn finishes");
    let more = s.count(|e| matches!(e, AgentEvent::PermissionRequest { .. })) - asked;
    if approve {
        check(more == 0, "no prompt for the write: acceptEdits restored");
        check(std::fs::read_to_string(cwd.join("plan_test.txt")).unwrap_or_default().contains("ok"), "file written");
    } else {
        check(!cwd.join("plan_test.txt").exists(), "nothing written");
        println!("reply: {}", short(&s.text()));
    }
    s.stop().await;
}

async fn claude_question() {
    let cwd = folder("claude-question");
    let mut s = Session::start(AgentId::ClaudeCode, &cwd, CLAUDE, HandHolding::Supervised, false, None);
    s.prompt("Use the AskUserQuestion tool to ask me which color I like (offer red and blue). Then repeat my answer back verbatim in one sentence.").await;
    let (rid, _, _, prompt) = s.permission(180).await;
    let Some(Prompt::Questions(q)) = prompt else { fail("expected a question") };
    s.send(Command::Answer { request_id: rid, answers: vec![(q[0].question.clone(), "teal with a hint of orange".into())] }).await;
    check(s.turn(180).await.is_none(), "turn finishes");
    check(s.text().contains("teal with a hint of orange"), "free-text answer reached Claude");
    s.stop().await;
}

/// Interrupting while Claude waits on an approval: the prompt is withdrawn (its card must go)
/// and the turn ends as Interrupted.
async fn claude_cancel() {
    let cwd = folder("claude-cancel");
    let mut s = Session::start(AgentId::ClaudeCode, &cwd, CLAUDE, HandHolding::Supervised, false, None);
    s.prompt("Run the shell command `touch cancel.txt`, then reply: done").await;
    let (rid, title, _, _) = s.permission(180).await;
    check(title == "Run command", "asks to run the command");
    s.send(Command::Interrupt).await;
    let err = s.turn(60).await;
    check(err.as_deref() == Some("Interrupted"), "turn ends as Interrupted");
    check(s.count(|e| matches!(e, AgentEvent::PermissionResolved { request_id } if *request_id == rid)) == 1, "the prompt was withdrawn");
    check(!cwd.join("cancel.txt").exists(), "nothing ran");
    s.stop().await;
}

/// Messages sent mid-turn: one while text streams (Claude runs it after the turn, with a result
/// of its own) and one while a command runs (Claude takes it into the turn). Either way the
/// turn ends once.
async fn claude_steer() {
    let cwd = folder("claude-steer");
    let mut s = Session::start(AgentId::ClaudeCode, &cwd, CLAUDE, HandHolding::FullAccess, false, None);
    s.prompt("Write a 150 word story about a fox.").await;
    s.until(120, |e| matches!(e, AgentEvent::TextDelta(_))).await;
    s.prompt("Now reply with just the word BANANA.").await;
    check(s.turn(180).await.is_none(), "steered turn finishes cleanly");
    check(s.text().contains("BANANA"), "the steer was answered");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { .. })) == 1, "one TurnComplete for the steered turn");
    s.prompt("Run the shell command `sleep 6`, then reply with one short sentence.").await;
    s.until(120, |e| matches!(e, AgentEvent::ToolStarted { title, .. } if title == "Run command")).await;
    s.prompt("Also end your reply with the word CHERRY.").await;
    check(s.turn(180).await.is_none(), "second steered turn finishes cleanly");
    check(s.text().contains("CHERRY"), "the steer reached the running turn");
    check(s.count(|e| matches!(e, AgentEvent::TurnComplete { .. })) == 2, "one TurnComplete per turn");
    check(!s.seen.iter().any(|e| matches!(e, AgentEvent::Error(_))), "no errors");
    s.stop().await;
}

/// A session Claude Code no longer has (its transcript was cleaned up) starts a new one instead
/// of failing every message.
async fn claude_lost_session() {
    let cwd = folder("claude-lost-session");
    let gone = "0b0b0b0b-0000-4000-8000-000000000000";
    let mut s = Session::start(AgentId::ClaudeCode, &cwd, CLAUDE, HandHolding::Supervised, false, Some(gone.into()));
    s.prompt("Reply with just the word: hello").await;
    let AgentEvent::Notice(why) = s.until(60, |e| matches!(e, AgentEvent::Notice(_))).await else { unreachable!() };
    check(why.contains("without the earlier context"), "the user is told the earlier context is gone");
    check(s.turn(120).await.is_none(), "the message is answered in the new session");
    check(!s.native_id().is_empty() && s.native_id() != gone, "started a new session");
    check(s.text().to_lowercase().contains("hello"), "the message reached it");
    s.stop().await;
}
