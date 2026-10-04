//! Sub-agents. An agent in Trek hands work to another agent and model through Trek's
//! orchestration tools (`trek-mcp orchestrate` → `ipc` → `handle_call`). Each sub-agent is a
//! thread of its own whose `parent_id` is the thread that asked: it runs in its parent's folder,
//! stays out of the inbox, shows as a row in its parent's transcript, and reports back with its
//! last message: to a caller still waiting, else in a message that wakes the parent.
//!
//! A report that's due goes in the store until the parent has it (`Store::hold_report`), so a
//! relaunch still delivers it; a parent free to take it gets it within `GATHER`, together with
//! any that came in alongside. While it waits on sub-agents (Trek's, or its agent's own working
//! in the background), a parent counts as working (`Workspace::waiting`).

use super::{Scope, Workspace};
use crate::ipc::{Call, Reply};
use gpui_kit::{Context, Task};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;
use trek_agents::{AgentEvent, Command, Decision, McpServer};
use trek_core::catalog;
use trek_core::limit::LimitScope;
use trek_core::orchestrate::{self as orch, MAX_DEPTH, MAX_PER_REQUEST, MAX_RUNNING, Mode, Outcome, Report};
use trek_core::store::{Item, Thread, ToolStatus, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState};

/// What Trek keeps about a sub-agent it started this run.
pub struct Delegation {
    pub parent: String,
    pub mode: Mode,
    /// The plan it offered, advising in plan mode: that's its advice.
    plan: Option<String>,
    /// Calls waiting for its answer (`delegate_task` or `task_result` with `wait`).
    waiters: Vec<async_channel::Sender<Reply>>,
    /// Stopped on its parent's behalf (Stop, `cancel_task`, archiving): its end isn't news.
    cancelled: bool,
    /// How it ended, once it has. Later turns (the user carrying on in it) don't report again.
    pub outcome: Option<Outcome>,
    /// When it began its current stretch of work (ms, on `Workspace::now`): when it started, or
    /// when it took its task back up after a usage limit.
    since: i64,
    /// Time it worked before a usage limit paused it (ms): the pause itself doesn't count.
    worked: i64,
    /// When it ended (ms): how long it ran doesn't change when the user carries on in it.
    ended: Option<i64>,
    /// It failed at a usage limit and its thread is paused until the limit resets: resumed, it
    /// takes its task back up and reports again (`task_resumed`).
    limited: bool,
    /// Ends it for good if it hasn't stopped within `STOP_GRACE` of being asked to.
    _stop: Option<Task<()>>,
}

impl Delegation {
    /// Stopped on its parent's behalf.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

/// Where a sub-agent stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    /// Waiting on the user (an approval, working with its parent's access level).
    NeedsYou,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn label(self) -> &'static str {
        match self {
            TaskState::Running => "Running",
            TaskState::NeedsYou => "Needs your approval",
            TaskState::Done => "Done",
            TaskState::Failed => "Failed",
            TaskState::Cancelled => "Stopped",
        }
    }

    fn key(self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::NeedsYou => "needs_approval",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }

    pub fn live(self) -> bool {
        matches!(self, TaskState::Running | TaskState::NeedsYou)
    }
}

/// How long Stop gives a sub-agent to end its turn before its session is shut down.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// The per-call limit Codex and Claude get for Trek's orchestration tools: longer than the
/// longest wait.
const TOOL_TIMEOUT_SECS: u64 = 1900;
/// The longest an ACP agent's call waits for a sub-agent. ACP gives Trek no way to set an MCP
/// call's time limit, and clients commonly give up after a minute; past this the call returns
/// "running", and the answer comes in a wake-up message instead.
const ACP_WAIT: Duration = Duration::from_secs(50);
/// How long a report waits for others before it wakes its parent: sub-agents that finish
/// together wake it once, with all their answers.
const GATHER: Duration = Duration::from_millis(120);
/// After a launch, reports held from before it wait this long: Trek finds its feet first.
const LAUNCH_WAKE: Duration = Duration::from_secs(if cfg!(test) { 0 } else { 3 });
/// How long a sub-agent whose agent's own sub-agents have all ended gets to take the turn their
/// reports start (Claude Code takes it within a second) before it's taken to be done without it.
const SELF_TURN_GRACE: Duration = Duration::from_secs(15);

/// Something a thread waits on: a sub-agent at work for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Waited {
    /// Whose logo it wears.
    pub agent: AgentId,
    /// "Sol: Review the cache", or the task an agent's own sub-agent was given.
    pub name: String,
    /// The model it runs ("Sol"), for one Trek runs: `None` for the agent's own.
    pub model: Option<String>,
    /// How long it has been at it.
    pub elapsed: Duration,
}

/// "Waiting on Sol", "Waiting on Sol and Opus 5.5", "Waiting on 3 sub-agents": who `waited` are,
/// by model for Trek's own and counted for an agent's.
pub fn waiting_label(waited: &[Waited]) -> String {
    let mut models: Vec<&str> = vec![];
    for m in waited.iter().filter_map(|w| w.model.as_deref()) {
        if !models.contains(&m) {
            models.push(m);
        }
    }
    let theirs = waited.iter().filter(|w| w.model.is_none()).count();
    let who = match (models.as_slice(), theirs) {
        ([], 0) => return String::new(),
        ([one], 0) if waited.len() == 1 => one.to_string(),
        ([a, b], 0) if waited.len() == 2 => format!("{a} and {b}"),
        ([one], 1) if waited.len() == 2 => format!("{one} and a sub-agent"),
        ([], 1) => "a sub-agent".to_string(),
        _ => format!("{} sub-agents", waited.len()),
    };
    format!("Waiting on {who}")
}

impl Workspace {
    /// Open Trek's end of the orchestration tools. Without it, sessions just don't get them.
    pub(super) fn start_ipc(&mut self, cx: &mut Context<Self>) {
        match crate::ipc::IpcServer::start(&trek_core::paths::data_dir().join("ipc")) {
            Ok((server, calls)) => {
                self.ipc = Some(server);
                self._ipc_calls = Some(cx.spawn(async move |this, cx| {
                    while let Ok(call) = calls.recv().await {
                        if this.update(cx, |ws, cx| ws.handle_call(call, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(e) => tracing::warn!("sub-agent tools are off this run: {e}"),
        }
    }

    /// MCP servers for a new session of `agent` in `thread` (`None`: a draft's, warmed up before
    /// its thread exists): `mcp_servers`, plus Trek's orchestration tools with a session key of
    /// their own. Returns the key too, to name the session's thread and to retire it.
    pub(super) fn session_mcp(&mut self, agent: &AgentId, thread: Option<&str>) -> (Vec<McpServer>, Option<String>) {
        let mut servers = self.mcp_servers();
        // The mock talks to Trek itself; a real agent needs the server binary.
        let mock = matches!(agent, AgentId::Direct(p) if p == catalog::MOCK_PROVIDER);
        let bin = super::trek_mcp_binary();
        let Some(ipc) = self.ipc.as_ref().filter(|_| self.settings.tools.orchestration && (mock || bin.is_some())) else { return (servers, None) };
        let key = ipc.open_session(thread);
        servers.push(McpServer {
            name: trek_agents::mock::ORCHESTRATE_SERVER.into(),
            command: bin.map(|b| b.display().to_string()).unwrap_or_else(|| "trek-mcp".into()),
            args: vec!["orchestrate".into()],
            env: ipc.env(&key),
            tool_timeout_secs: Some(TOOL_TIMEOUT_SECS),
        });
        (servers, Some(key))
    }

    /// Why an agent of `agent` can't consult other models (it wouldn't get `delegate_task`), for
    /// the composer: `None` when it can.
    pub fn consult_unavailable(&self, agent: &AgentId) -> Option<String> {
        let mock = matches!(agent, AgentId::Direct(p) if p == catalog::MOCK_PROVIDER);
        if !self.settings.tools.orchestration {
            Some("Sub-agent tools are off in Settings → Tools".into())
        } else if self.ipc.is_none() {
            Some("Trek couldn't open its sub-agent channel this run".into())
        } else if matches!(agent, AgentId::Direct(_)) && !mock {
            Some(format!("{} can't use Trek's tools: pick Claude Code, Codex or an ACP agent to consult", agent.display_name()))
        } else if !mock && super::trek_mcp_binary().is_none() {
            Some("Trek's tool server (trek-mcp) is missing from this build".into())
        } else {
            None
        }
    }

    /// `id`'s session now goes by `key`; the key of the session it replaces is retired.
    pub(super) fn adopt_ipc_session(&mut self, id: &str, key: Option<String>) {
        let old = std::mem::replace(&mut self.live.entry(id.to_string()).or_default().ipc_session, key.clone());
        let Some(ipc) = &self.ipc else { return };
        if let Some(old) = old.filter(|o| Some(o) != key.as_ref()) {
            ipc.close_session(&old);
        }
        if let Some(key) = &key {
            ipc.bind(key, id);
        }
    }

    /// `id`'s session ended: its orchestration key goes with it.
    pub(super) fn retire_ipc_session(&mut self, id: &str) {
        if let (Some(key), Some(ipc)) = (self.live.get_mut(id).and_then(|l| l.ipc_session.take()), &self.ipc) {
            ipc.close_session(&key);
        }
    }

    /// A tool call from an agent's session.
    pub(crate) fn handle_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let id = || call.params.get("id").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        let result = if self.thread(&call.thread).is_none() {
            Err("This thread isn't in Trek any more.".to_string())
        } else {
            match call.method.as_str() {
                "list_models" => Ok(self.list_models(&call.thread)),
                "delegate_task" => match self.delegate(&call.thread, &call.params, cx) {
                    Ok(child) if call.params.get("wait").and_then(Value::as_bool) == Some(true) => {
                        if let Some(d) = self.delegations.get_mut(&child) {
                            d.waiters.push(call.reply.clone());
                        }
                        let wait = crate::ipc::wait_for(&call.params).min(self.longest_wait(&call.thread));
                        let _ = call.reply.try_send(Reply::Waiting(child, wait));
                        return;
                    }
                    Ok(child) => {
                        let mut v = self.task_json(&child, false);
                        v["note"] = json!("It runs on its own. Trek sends you its answer in a message when it finishes: end your turn rather than polling, or collect it in this turn with task_result and wait true.");
                        Ok(v)
                    }
                    Err(e) => Err(e),
                },
                "task_status" => self.task_status(&call.thread, &id()),
                "task_result" => {
                    let child = id();
                    let wait = call.params.get("wait").and_then(Value::as_bool) == Some(true);
                    // Waiting on one still at work: its answer comes when it ends, as for a
                    // `delegate_task` that waits. Several started without waiting run side by
                    // side while the agent collects them one by one.
                    let live = self.own_task(&call.thread, &child).is_ok() && self.task_state(&child).live();
                    if let Some(d) = self.delegations.get_mut(&child).filter(|d| wait && live && d.outcome.is_none()) {
                        d.waiters.push(call.reply.clone());
                        let wait = crate::ipc::wait_for(&call.params).min(self.longest_wait(&call.thread));
                        let _ = call.reply.try_send(Reply::Waiting(child, wait));
                        return;
                    }
                    let result = self.task_result(&call.thread, &child);
                    // Read now, its answer needn't wake the agent later, nor after a restart.
                    if result.as_ref().is_ok_and(|_| !self.task_state(&child).live()) {
                        self.heard(&call.thread, &child);
                    }
                    result
                }
                "cancel_task" => self.cancel_task(&call.thread, &id(), cx),
                other => Err(format!("Trek has no tool called {other}.")),
            }
        };
        let _ = call.reply.try_send(Reply::Done(result));
    }

    /// What `list_models` answers: every agent the user can pick in Trek right now, with its
    /// models and efforts, and where the caller stands among sub-agents.
    pub fn list_models(&self, caller: &str) -> Value {
        let agents: Vec<Value> = self
            .ready_agents()
            .iter()
            .map(|a| {
                let models = self.models_for(a);
                json!({
                    "agent": a.key(),
                    "name": a.display_name(),
                    "default_model": crate::composer::default_model(&models).map(|m| m.id.clone()),
                    "models": models.iter().map(|m| json!({ "id": m.id, "name": m.name, "efforts": m.efforts.iter().map(|e| e.as_str()).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let me = self.thread(caller);
        let depth = self.depth(caller);
        json!({
            "agents": agents,
            "you": { "agent": me.map(|t| t.agent.key()), "model": me.and_then(|t| t.model.clone()), "effort": me.map(|t| t.effort.as_str()) },
            "sub_agents": {
                "depth": depth,
                "max_depth": MAX_DEPTH,
                "can_delegate": depth < MAX_DEPTH,
                "running": self.running_children(caller).len(),
                "max_running": MAX_RUNNING,
            },
        })
    }

    /// How long a call from `caller`'s agent can wait on a sub-agent before its client gives up.
    pub(crate) fn longest_wait(&self, caller: &str) -> Duration {
        match self.thread(caller).map(|t| &t.agent) {
            Some(AgentId::Acp(_) | AgentId::OpenCode | AgentId::Droid) => ACP_WAIT,
            _ => Duration::MAX,
        }
    }

    /// Whether `id` is a sub-agent that only advises, still on its task: nothing it does may
    /// change files.
    pub(crate) fn advising(&self, id: &str) -> bool {
        self.delegations.get(id).is_some_and(|d| d.mode == Mode::Advise && d.outcome.is_none())
    }

    /// Sub-agents `caller` started since the user's last message (wake-ups don't count).
    fn started_since_asked(&self, caller: &str) -> usize {
        let since = self.live.get(caller).and_then(|l| {
            l.items.iter().rev().find_map(|i| match i {
                Item::User { text, at, .. } if !orch::is_wake(text) => Some(at.unwrap_or(0)),
                _ => None,
            })
        });
        self.children(caller).iter().filter(|t| since.is_none_or(|s| t.created_at >= s)).count()
    }

    /// How many sub-agent levels `id` sits under.
    pub fn depth(&self, id: &str) -> usize {
        orch::depth(id, |t| self.thread(t).cloned().or_else(|| self.store.thread(t).ok().flatten()).and_then(|t| t.parent_id))
    }

    /// Start a sub-agent of `caller` as `params` ask. Returns its thread id.
    pub fn delegate(&mut self, caller: &str, params: &Value, cx: &mut Context<Self>) -> Result<String, String> {
        let parent = self.thread(caller).cloned().ok_or("This thread isn't in Trek any more.")?;
        if self.depth(caller) >= MAX_DEPTH {
            return Err(format!("You're a sub-agent {MAX_DEPTH} levels down, and Trek allows no deeper: do this part yourself."));
        }
        let running = self.running_children(caller).len();
        if running >= MAX_RUNNING {
            return Err(format!(
                "This thread already has {running} sub-agents running, the most Trek allows at once. Wait for one to finish (task_status), or stop one (cancel_task)."
            ));
        }
        if self.started_since_asked(caller) >= MAX_PER_REQUEST {
            return Err(format!(
                "You've started {MAX_PER_REQUEST} sub-agents since the user's last message, the most Trek allows. Report what you have and let the user decide what's next."
            ));
        }
        let text = |k: &str| params.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let title = text("title").ok_or("delegate_task needs a title: a few words naming the task.")?;
        let prompt = text("prompt").ok_or("delegate_task needs a prompt: the sub-agent's whole task, complete on its own.")?;
        let mode = match text("mode") {
            None => Mode::Advise,
            Some(m) => Mode::parse(m).ok_or_else(|| format!("mode is \"advise\" or \"implement\", not “{m}”."))?,
        };
        if mode == Mode::Implement && self.advising(caller) {
            return Err("You're advising, read-only, so a sub-agent you start can't change files either: use mode \"advise\".".into());
        }
        let agent = self.resolve_agent(text("agent"), &parent.agent)?;
        let models = self.models_for(&agent);
        let model = match text("model") {
            None => crate::composer::default_model(&models).map(|m| m.id.clone()),
            Some(m) => Some(
                models
                    .iter()
                    .find(|i| crate::composer::same_model(m, &i.id) || i.id.eq_ignore_ascii_case(m) || i.name.eq_ignore_ascii_case(m))
                    .map(|i| i.id.clone())
                    .or_else(|| models.is_empty().then(|| m.to_string()))
                    .ok_or_else(|| {
                        let list: Vec<String> = models.iter().map(|i| format!("{} ({})", i.id, i.name)).collect();
                        format!("{} has no model “{m}”. Its models: {}.", agent.display_name(), list.join(", "))
                    })?,
            ),
        };
        let effort = match text("effort") {
            None => parent.effort,
            Some(e) => Effort::parse(e).ok_or_else(|| format!("effort is one of off, minimal, low, medium, high, xhigh, max; not “{e}”."))?,
        };
        let efforts = model.as_ref().and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id))).map(|i| i.efforts.clone()).unwrap_or_default();
        let effort = if efforts.is_empty() { effort } else { effort.clamp_to(&efforts) };
        if let Some(why) = self.at_limit(&agent, model.as_deref()) {
            return Err(why);
        }
        // Advising, it can't change anything: nothing it asks to do is approved (`screen_advice`).
        let hand_holding = if mode == Mode::Advise { HandHolding::Supervised } else { parent.hand_holding };
        let mut child = self.store.create_thread(parent.cwd.as_deref(), agent, model, effort, hand_holding).map_err(|e| format!("Couldn't start the sub-agent: {e}"))?;
        child.title = title.chars().take(80).collect();
        // Its parent's folder, worktree and all.
        child.cwd = parent.cwd.clone();
        child.worktree = parent.worktree.clone();
        child.parent_id = Some(caller.to_string());
        self.store.save_thread(&child).map_err(|e| format!("Couldn't start the sub-agent: {e}"))?;
        let id = child.id.clone();
        self.threads.push(child);
        self.live.entry(id.clone()).or_default().loaded = true;
        let since = self.now();
        self.delegations.insert(
            id.clone(),
            Delegation { parent: caller.to_string(), mode, plan: None, waiters: vec![], cancelled: false, outcome: None, since, worked: 0, ended: None, limited: false, _stop: None },
        );
        // Its report reaches the parent one way or another, after a relaunch too.
        if let Err(e) = self.store.await_report(caller, &id) {
            tracing::warn!("keep the sub-agent's report due: {e:#}");
        }
        self.push_task_row(caller, &id, title, cx);
        self.send_to(&id, orch::child_prompt(mode, prompt), vec![], cx);
        Ok(id)
    }

    /// The agent `name` stands for, among those the user can pick (by key or name), else `default`.
    fn resolve_agent(&self, name: Option<&str>, default: &AgentId) -> Result<AgentId, String> {
        let ready = self.ready_agents();
        let Some(name) = name else {
            return Ok(if ready.contains(default) { default.clone() } else { ready.first().cloned().unwrap_or(default.clone()) });
        };
        let want = name.to_lowercase();
        ready
            .iter()
            .find(|a| {
                let key = a.key();
                key == want || a.display_name().to_lowercase() == want || key.split_once(':').is_some_and(|(_, id)| id == want) || (want == "claude" && **a == AgentId::ClaudeCode)
            })
            .cloned()
            .ok_or_else(|| format!("Trek can't run “{name}” now. These it can: {}.", ready.iter().map(|a| a.key()).collect::<Vec<_>>().join(", ")))
    }

    /// Why `agent` can't take more work on `model` now: a usage limit it has used up, as its usage
    /// windows say (those on another model don't count), or as a thread of it paused at one says.
    fn at_limit(&self, agent: &AgentId, model: Option<&str>) -> Option<String> {
        let now = self.now();
        let window = self.agent_status.get(&agent.key()).and_then(|st| {
            st.limits.iter().find(|l| l.percent >= 99.5 && trek_agents::limits::applies(l, &LimitScope::Other, model) && l.resets_at.is_none_or(|r| r > now))
        });
        if let Some(l) = window {
            let resets = l.resets_at.map(|r| format!(" (it resets {})", crate::time::reset_clock(r, now))).unwrap_or_default();
            return Some(format!("{} has used up its {}{resets}. Pick another agent.", agent.display_name(), l.label.to_lowercase()));
        }
        let paused = self
            .threads
            .iter()
            .filter(|t| t.agent == *agent)
            .filter_map(|t| t.paused.as_ref().map(|p| (t, p)))
            .filter(|(t, p)| match &p.scope {
                LimitScope::Model(_) => t.model.as_deref().zip(model).is_some_and(|(a, b)| crate::composer::same_model(a, b)),
                _ => true,
            })
            .filter_map(|(_, p)| p.resets_at.filter(|r| *r > now).map(|r| (r, p.scope.label())))
            .max_by_key(|(r, _)| *r);
        paused.map(|(r, label)| format!("{} has used up its {} (it resets {}). Pick another agent.", agent.display_name(), label.to_lowercase(), crate::time::reset_clock(r, now)))
    }

    /// The sub-agent's row in its parent's transcript, where the call was made.
    fn push_task_row(&mut self, parent: &str, child: &str, title: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(parent) else { return };
        let row = Item::Tool { id: orch::task_row(child), title: "Sub-agent".into(), detail: title.to_string(), output: String::new(), status: ToolStatus::Running };
        live.pending_rows.push_back(row);
        // Asked outside a turn, there's no call row to wait for.
        let idle = live.turn_started.is_none();
        live.place_task_rows(idle);
        self.persist_soon(parent, cx);
        cx.notify();
    }

    /// Settle the sub-agent's row in its parent: how it ended, and its answer.
    fn settle_task_row(&mut self, parent: &str, child: &str, status: ToolStatus, output: String, cx: &mut Context<Self>) {
        let row = orch::task_row(child);
        let is_row = |i: &Item| matches!(i, Item::Tool { id, .. } if *id == row);
        // Not placed yet (its call hasn't reached the transcript): it goes in settled.
        if let Some(Item::Tool { status: s, output: o, .. }) = self.live.get_mut(parent).and_then(|l| l.pending_rows.iter_mut().find(|i| is_row(i))) {
            *s = status;
            *o = output;
            return;
        }
        if let Some(live) = self.live.get_mut(parent).filter(|l| l.loaded && !l.loading) {
            if let Some(Item::Tool { status: s, output: o, .. }) = live.items.rfind_mut(is_row) {
                *s = status;
                *o = output;
            }
            live.revision += 1;
            self.persist_items(parent, cx);
        } else if let Some((item_id, mut item)) = self.store.items_with_ids(parent).ok().and_then(|rows| rows.into_iter().rev().find(|(_, i)| is_row(i))) {
            if let Item::Tool { status: s, output: o, .. } = &mut item {
                *s = status;
                *o = output;
            }
            let _ = self.store.update_item(&item_id, &item);
        }
        cx.notify();
    }

    /// `parent`'s sub-agents still in Trek's lists, oldest first.
    pub fn children(&self, parent: &str) -> Vec<&Thread> {
        let mut out: Vec<&Thread> = self.threads.iter().filter(|t| t.parent_id.as_deref() == Some(parent)).collect();
        out.sort_by_key(|t| t.created_at);
        out
    }

    /// `parent`'s sub-agents that are still at work (or waiting on the user).
    pub fn running_children(&self, parent: &str) -> Vec<&Thread> {
        self.children(parent).into_iter().filter(|t| self.task_state(&t.id).live()).collect()
    }

    /// Threads with a sub-agent under them (at any depth) waiting on the user's approval.
    /// Sub-agents stay out of the inbox, so the cards of those above them carry the request.
    pub fn waiting_on_sub_agents(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for t in self.threads.iter().filter(|t| t.parent_id.is_some() && t.run_state == RunState::NeedsYou) {
            let mut up = t.parent_id.clone();
            while let Some(p) = up.take() {
                if out.len() > self.threads.len() || !out.insert(p.clone()) {
                    break;
                }
                up = self.thread(&p).and_then(|p| p.parent_id.clone());
            }
        }
        out
    }

    /// Whether a sub-agent under `id` waits on the user's approval (`waiting_on_sub_agents`).
    pub fn sub_agent_needs_you(&self, id: &str) -> bool {
        self.waiting_on_sub_agents().contains(id)
    }

    /// Every sub-agent under `id`, theirs too, archived ones included.
    fn descendants(&self, id: &str) -> Vec<Thread> {
        let mut out = vec![];
        let mut queue = vec![id.to_string()];
        while let Some(at) = queue.pop() {
            for t in self.store.sub_agents(&at).unwrap_or_default() {
                queue.push(t.id.clone());
                out.push(t);
            }
            if out.len() > 1000 {
                break;
            }
        }
        out
    }

    /// Where sub-agent `child` stands.
    pub fn task_state(&self, child: &str) -> TaskState {
        if let Some(outcome) = self.delegations.get(child).and_then(|d| d.outcome.as_ref()) {
            return match outcome {
                Outcome::Done(_) => TaskState::Done,
                Outcome::Failed(_) => TaskState::Failed,
                Outcome::Cancelled => TaskState::Cancelled,
            };
        }
        let Some(t) = self.thread(child) else { return TaskState::Cancelled };
        let busy = self.live.get(child).is_some_and(|l| l.turn_started.is_some() || l.background_agents().next().is_some() || !l.queued.is_empty() || l.preparing);
        match t.run_state {
            RunState::NeedsYou => TaskState::NeedsYou,
            _ if busy => TaskState::Running,
            // Started this run and not ended: between turns, it waits on sub-agents of its own.
            _ if self.delegations.contains_key(child) => TaskState::Running,
            RunState::Failed => TaskState::Failed,
            // Ended in an earlier run: its row says how. One still "running" was cut off when
            // Trek quit (`settle_cut_off_rows`).
            _ => match self.task_row_status(child) {
                Some(ToolStatus::Denied) => TaskState::Cancelled,
                Some(ToolStatus::Failed | ToolStatus::Running) => TaskState::Failed,
                _ => TaskState::Done,
            },
        }
    }

    fn task_row_status(&self, child: &str) -> Option<ToolStatus> {
        self.task_row_of(child).map(|(status, _)| status)
    }

    /// The row of `child` in its parent's transcript (once that's loaded): status and output.
    fn task_row_of(&self, child: &str) -> Option<(ToolStatus, String)> {
        let parent = self.thread(child)?.parent_id.clone()?;
        let row = orch::task_row(child);
        self.live.get(&parent)?.items.iter().rev().find_map(|i| match i {
            Item::Tool { id, status, output, .. } if *id == row => Some((*status, output.clone())),
            _ => None,
        })
    }

    /// Rows of sub-agents still "running" in `parent`'s transcript, just read from the store,
    /// that no sub-agent of this run stands behind: Trek quit while they worked. They failed.
    pub(super) fn settle_cut_off_rows(&mut self, parent: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(parent) else { return };
        let mut changed = false;
        for ix in 0..live.items.len() {
            let cut_off = matches!(&live.items[ix], Item::Tool { id, status: ToolStatus::Running, .. } if orch::task_of_row(id).is_some_and(|c| !self.delegations.contains_key(c)));
            if let (true, Some(Item::Tool { status, output, .. })) = (cut_off, live.items.get_mut(ix)) {
                *status = ToolStatus::Failed;
                *output = orch::CUT_OFF.into();
                changed = true;
            }
        }
        if changed {
            live.revision += 1;
            self.persist_items(parent, cx);
        }
    }

    /// How long `child` has worked (so far, or in all), time paused at a usage limit aside.
    pub fn task_elapsed(&self, child: &str) -> Duration {
        let Some(t) = self.thread(child) else { return Duration::ZERO };
        let ms = match self.delegations.get(child) {
            Some(d) => d.worked + (d.ended.unwrap_or_else(|| self.now()) - d.since).max(0),
            None if self.task_state(child).live() => now_ms() - t.created_at,
            // Ended in an earlier run.
            None => t.updated_at - t.created_at,
        };
        Duration::from_millis(ms.max(0) as u64)
    }

    /// "Sol", "Opus 5.5": the short name of the model `t` runs.
    pub fn model_label(&self, t: &Thread) -> String {
        let models = self.models_for(&t.agent);
        let id = t.model.clone().or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone()));
        id.map(|m| models.iter().find(|i| crate::composer::same_model(&m, &i.id)).map(|i| i.name.clone()).unwrap_or(m)).unwrap_or_else(|| t.agent.display_name())
    }

    /// `child`'s answer as it stands: how it ended, else its last message so far.
    fn task_answer(&self, child: &str) -> Option<String> {
        match self.delegations.get(child).and_then(|d| d.outcome.as_ref()) {
            Some(Outcome::Done(a)) => return Some(a.clone()),
            Some(Outcome::Failed(e)) => return Some(e.clone()),
            Some(Outcome::Cancelled) => return None,
            None => {}
        }
        let items = match self.live.get(child).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(child).unwrap_or_default(),
        };
        match self.task_state(child) {
            // Cut off in an earlier run, its row says so.
            TaskState::Failed => orch::last_error(&items).or_else(|| self.task_row_of(child).map(|(_, why)| why).filter(|w| !w.is_empty())),
            _ => orch::final_answer(&items, self.delegations.get(child).and_then(|d| d.plan.as_deref())),
        }
    }

    /// A sub-agent as the tools describe it; with its answer (capped) when `result` and it's done.
    fn task_json(&self, child: &str, result: bool) -> Value {
        let Some(t) = self.thread(child) else { return json!({ "id": child, "status": "cancelled" }) };
        let state = self.task_state(child);
        let mut v = json!({
            "id": child,
            "title": t.title,
            "status": state.key(),
            "agent": t.agent.key(),
            "model": self.model_label(t),
            "effort": t.effort.as_str(),
            "elapsed_seconds": self.task_elapsed(child).as_secs(),
        });
        if let Some(d) = self.delegations.get(child) {
            v["mode"] = json!(d.mode.as_str());
        }
        if result && !state.live() {
            match (state, self.task_answer(child)) {
                (TaskState::Failed, why) => v["error"] = json!(why.unwrap_or_else(|| "It stopped without saying why.".into())),
                (TaskState::Cancelled, _) => {}
                (_, answer) => v["result"] = json!(orch::cap(&answer.unwrap_or_else(|| "(It finished without saying anything.)".into()), orch::RESULT_CAP, "That's as much as Trek passes on.")),
            }
        }
        v
    }

    /// `id`, if it's a sub-agent of `caller`.
    fn own_task(&self, caller: &str, id: &str) -> Result<(), String> {
        let parent = self.thread(id).cloned().or_else(|| self.store.thread(id).ok().flatten()).and_then(|t| t.parent_id);
        if id.is_empty() || parent.as_deref() != Some(caller) {
            return Err(format!("This thread has no sub-agent “{id}”. Use the id delegate_task returned."));
        }
        if self.thread(id).is_none() {
            return Err("That sub-agent's thread was archived or deleted.".into());
        }
        Ok(())
    }

    pub fn task_status(&self, caller: &str, id: &str) -> Result<Value, String> {
        self.own_task(caller, id)?;
        let mut v = self.task_json(id, false);
        match self.task_state(id) {
            TaskState::Done | TaskState::Failed => {
                if let Some(a) = self.task_answer(id) {
                    v["preview"] = json!(orch::preview(&a, orch::PREVIEW_CAP));
                }
            }
            TaskState::Running => {
                // What it's doing right now: its latest step.
                let step = self.live.get(id).and_then(|l| {
                    l.items.iter().rev().find_map(|i| match i {
                        Item::Tool { title, detail, .. } => Some(orch::preview(&format!("{title} {detail}"), 120)),
                        _ => None,
                    })
                });
                if let Some(s) = step {
                    v["latest_step"] = json!(s);
                }
            }
            _ => {}
        }
        Ok(v)
    }

    pub fn task_result(&self, caller: &str, id: &str) -> Result<Value, String> {
        self.own_task(caller, id)?;
        let mut v = self.task_json(id, true);
        if self.task_state(id).live() {
            v["note"] = json!("Not finished yet. Trek sends you its answer when it is, or call task_result with wait true to wait for it.");
        }
        Ok(v)
    }

    pub fn cancel_task(&mut self, caller: &str, id: &str, cx: &mut Context<Self>) -> Result<Value, String> {
        self.own_task(caller, id)?;
        if !self.task_state(id).live() {
            let mut v = self.task_json(id, false);
            v["note"] = json!("It had already ended.");
            return Ok(v);
        }
        self.stop_task(id, cx);
        Ok(json!({ "id": id, "status": "cancelled" }))
    }

    /// Stop sub-agent `child` on its parent's behalf: its turn is interrupted (its own sub-agents
    /// with it), and if it hasn't ended within `STOP_GRACE` its session is shut down.
    fn stop_task(&mut self, child: &str, cx: &mut Context<Self>) {
        if let Some(d) = self.delegations.get_mut(child) {
            d.cancelled = true;
        }
        self.interrupt(child, cx);
        // Between turns (waiting on its own sub-agents) there's no turn to end: it ends now.
        if self.live.get(child).is_none_or(|l| l.turn_started.is_none()) {
            self.end_task_now(child, cx);
            return;
        }
        let id = child.to_string();
        let stop = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STOP_GRACE).await;
            let _ = this.update(cx, |ws, cx| ws.end_task_now(&id, cx));
        });
        match self.delegations.get_mut(child) {
            Some(d) if d.outcome.is_none() => d._stop = Some(stop),
            Some(_) => {}
            None => self.keep(stop),
        }
    }

    /// Stop every sub-agent of `parent` that's still at work (Stop on the parent), and those
    /// further down still working under one that's done.
    pub(super) fn stop_children(&mut self, parent: &str, cx: &mut Context<Self>) {
        let kids: Vec<(String, bool)> = self.children(parent).into_iter().map(|t| (t.id.clone(), self.task_state(&t.id).live())).collect();
        for (child, live) in kids {
            if live {
                self.stop_task(&child, cx);
            } else {
                self.forget_limited(&child, cx);
                self.stop_children(&child, cx);
            }
        }
    }

    /// A sub-agent that didn't stop when asked: end its session and its turn here.
    fn end_task_now(&mut self, child: &str, cx: &mut Context<Self>) {
        if !self.task_state(child).live() {
            return;
        }
        if let Some(live) = self.live.get_mut(child) {
            if let Some(tx) = live.commands.take() {
                let _ = tx.try_send(Command::Shutdown);
            }
            live._events = None;
            live.held.clear();
            live.queued.clear();
            live.permissions.clear();
            if live.turn_started.take().is_some() {
                live.close_turn(false);
                live.streaming = None;
                live.reasoning = None;
                live.items.push(Item::Notice { text: "Stopped".into() });
            }
            live.lose_background();
            live.revision += 1;
        }
        self.retire_ipc_session(child);
        self.persist_items(child, cx);
        self.mutate_thread(child, cx, |t| {
            if matches!(t.run_state, RunState::Working | RunState::NeedsYou) {
                t.run_state = RunState::Idle;
            }
        });
        self.finish_task(child, Outcome::Cancelled, cx);
    }

    /// Requests an advising sub-agent makes are declined on the spot (it works read-only); a plan
    /// it offers is kept as its advice. The rest of `events` goes on as usual.
    pub(super) fn screen_advice(&mut self, id: &str, events: Vec<AgentEvent>) -> Vec<AgentEvent> {
        let tx = self.live.get(id).and_then(|l| l.commands.clone());
        let Some(d) = self.delegations.get_mut(id).filter(|d| d.mode == Mode::Advise && d.outcome.is_none()) else { return events };
        events
            .into_iter()
            .filter_map(|ev| match ev {
                AgentEvent::PermissionRequest { request_id, prompt, .. } => {
                    if let Some(trek_agents::Prompt::Plan(plan)) = prompt {
                        d.plan = Some(plan);
                    }
                    if let Some(tx) = &tx {
                        let _ = tx.try_send(Command::Respond { request_id, decision: Decision::Deny });
                    }
                    None
                }
                other => Some(other),
            })
            .collect()
    }

    /// A sub-agent's turn ended (`interrupted`: it was stopped): it reports how.
    pub(super) fn task_turn_ended(&mut self, id: &str, interrupted: bool, cx: &mut Context<Self>) {
        let Some(d) = self.delegations.get(id).filter(|d| d.outcome.is_none()) else { return };
        let (cancelled, plan) = (d.cancelled, d.plan.clone());
        let failed = self.thread(id).is_some_and(|t| t.run_state == RunState::Failed);
        // Paused at a usage limit: it can't go on until the limit resets.
        let paused = self.pause(id).map(|p| (p.scope.label(), p.resets_at));
        // Sub-agents of its own still at work (Trek's, or its agent's in the background), or
        // reports from them it hasn't had: it isn't done until the turn after its last wake-up.
        let awaiting = !self.running_children(id).is_empty()
            || self.wakes.get(id).is_some_and(|w| !w.is_empty())
            || self.live.get(id).is_some_and(|l| l.background_agents().next().is_some());
        if awaiting && !(interrupted || cancelled || failed || paused.is_some()) {
            return;
        }
        let items = self.live.get(id).map(|l| l.items.to_vec()).unwrap_or_default();
        let outcome = if interrupted || cancelled {
            Outcome::Cancelled
        } else if let Some((label, resets)) = paused {
            // Not for good: resumed at the reset, it takes the task back up (`task_resumed`).
            if let Some(d) = self.delegations.get_mut(id) {
                d.limited = true;
            }
            let when = resets.map(|r| format!(", which resets {}", crate::time::reset_clock(r, self.now()))).unwrap_or_default();
            Outcome::Failed(format!("It hit its {}{when}. Ask another model, or let it resume from its thread at the reset.", label.to_lowercase()))
        } else if failed {
            Outcome::Failed(orch::last_error(&items).unwrap_or_else(|| "It stopped without saying why.".into()))
        } else {
            Outcome::Done(orch::final_answer(&items, plan.as_deref()).unwrap_or_else(|| "(It finished without saying anything.)".into()))
        };
        self.finish_task(id, outcome, cx);
    }

    /// Sub-agent `id`'s session ended between turns while its agent's own sub-agents were still
    /// out: they went with it, and the turn their reports would have started never comes. Unless
    /// Trek's sub-agents will wake it yet, it's done, and failed.
    pub(super) fn background_agents_lost(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.delegations.get(id).is_none_or(|d| d.outcome.is_some()) || self.awaits_trek_reports(id) {
            return;
        }
        let why = "Its session ended while its own sub-agents were still at work. Its thread keeps what it did.";
        self.finish_task(id, Outcome::Failed(why.into()), cx);
    }

    /// Sub-agent `id`'s agent's own sub-agents have all ended with no turn of its own open. It
    /// normally takes one for their reports, and reports itself as that ends; if it doesn't
    /// within `SELF_TURN_GRACE` (they were stopped, say), it's done as it stands.
    pub(super) fn background_agents_gone(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.delegations.get(id).is_none_or(|d| d.outcome.is_some()) {
            return;
        }
        let id = id.to_string();
        let check = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SELF_TURN_GRACE).await;
            let _ = this.update(cx, |ws, cx| {
                let idle = ws.live.get(&id).is_none_or(|l| l.turn_started.is_none() && l.background_agents().next().is_none());
                if idle {
                    ws.task_turn_ended(&id, false, cx);
                }
            });
        });
        self.keep(check);
    }

    /// Reports from Trek's sub-agents of `id` are still to come (or to be heard).
    fn awaits_trek_reports(&self, id: &str) -> bool {
        !self.running_children(id).is_empty() || self.wakes.get(id).is_some_and(|w| !w.is_empty())
    }

    /// Sub-agent `id`, stopped by a usage limit, resumes at the reset (`send_resume`): it takes its
    /// task back up, its row runs again, and it reports to its parent when it's done.
    pub(super) fn task_resumed(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.now();
        let Some(d) = self.delegations.get_mut(id).filter(|d| d.limited && !d.cancelled) else { return };
        d.limited = false;
        d.outcome = None;
        // The clock picks up where it stopped: the wait for the reset isn't work.
        if let Some(ended) = d.ended.take() {
            d.worked += (ended - d.since).max(0);
        }
        d.since = now;
        let parent = d.parent.clone();
        let _ = self.store.await_report(&parent, id);
        self.settle_task_row(&parent, id, ToolStatus::Running, String::new(), cx);
    }

    /// A sub-agent of `parent`'s that a usage limit paused won't resume: its parent has moved
    /// on (Stop, archiving).
    fn forget_limited(&mut self, child: &str, cx: &mut Context<Self>) {
        let Some(d) = self.delegations.get_mut(child).filter(|d| d.limited) else { return };
        d.limited = false;
        d.cancelled = true;
        if self.pause(child).is_some() {
            self.mutate_thread(child, cx, |t| t.paused = None);
            self.schedule_limits(cx);
        }
    }

    /// Sub-agent `child` ended as `outcome`: its row in the parent shows it, its session goes,
    /// and its answer reaches the parent: the calls waiting for it, else a message that wakes
    /// the parent (not when the parent's side stopped it).
    fn finish_task(&mut self, child: &str, outcome: Outcome, cx: &mut Context<Self>) {
        let now = self.now();
        let Some(d) = self.delegations.get_mut(child).filter(|d| d.outcome.is_none()) else { return };
        d.outcome = Some(outcome.clone());
        d.ended = Some(now);
        d._stop = None;
        let (parent, waiters, silent) = (d.parent.clone(), std::mem::take(&mut d.waiters), d.cancelled);
        let (status, output) = match &outcome {
            Outcome::Done(answer) => (ToolStatus::Done, orch::cap(answer, orch::RESULT_CAP, "Open the sub-agent's thread for the rest.")),
            Outcome::Failed(why) => (ToolStatus::Failed, why.clone()),
            Outcome::Cancelled => (ToolStatus::Denied, String::new()),
        };
        self.settle_task_row(&parent, child, status, output, cx);
        // Failed or stopped, it has no use for sub-agents of its own still at work.
        self.stop_children(child, cx);
        // Its work is done: its session (150–250 MB) goes. A message from the user starts it again.
        if let Some(tx) = self.live.get_mut(child).and_then(|l| l.commands.take()) {
            let _ = tx.try_send(Command::Shutdown);
        }
        self.retire_ipc_session(child);
        let answer = self.task_json(child, true);
        let mut heard = false;
        for w in waiters {
            heard |= w.try_send(Reply::Done(Ok(answer.clone()))).is_ok();
        }
        if !heard && !silent {
            let report = Report {
                id: child.to_string(),
                title: self.thread(child).map(|t| t.title.clone()).unwrap_or_default(),
                model: self.thread(child).map(|t| self.model_label(t)).unwrap_or_default(),
                outcome,
            };
            if let Err(e) = self.store.hold_report(&parent, &report) {
                tracing::warn!("keep the sub-agent's report: {e:#}");
            }
            self.wakes.entry(parent.clone()).or_default().push(report);
            self.gather_wakes(&parent, cx);
        } else {
            let _ = self.store.drop_report(child);
            self.parent_may_be_done(&parent, cx);
        }
        cx.notify();
    }

    /// Wake `parent` with its reports once `GATHER` has passed: sub-agents that finish together
    /// reach it in one message.
    fn gather_wakes(&mut self, parent: &str, cx: &mut Context<Self>) {
        if self.gathering.contains_key(parent) {
            return;
        }
        let id = parent.to_string();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GATHER).await;
            let _ = this.update(cx, |ws, cx| {
                ws.gathering.remove(&id);
                ws.deliver_wakes(&id, cx);
            });
        });
        self.gathering.insert(parent.to_string(), task);
    }

    /// Reports a parent hadn't heard when Trek last quit, back in line for it: those held for it,
    /// and for each sub-agent the quit cut off, that it was. A parent whose own turn was cut off
    /// too (`cut_off`: it was waiting for an answer mid-turn) isn't woken: the user picks it up,
    /// and its reports go out once that turn is over (`parked`). Nor is a parent that's a
    /// sub-agent itself: its own answer would have nobody to go to (its parent hears it was cut
    /// off).
    pub(super) fn restore_wakes(&mut self, cut_off: &[String], cx: &mut Context<Self>) {
        let held = self.store.held_reports().unwrap_or_else(|e| {
            tracing::warn!("read held reports: {e:#}");
            vec![]
        });
        for (parent, child, report) in held {
            let alive = self.thread(&parent).is_some_and(|t| t.archived_at.is_none() && t.parent_id.is_none());
            let report = match report {
                Some(r) if alive => r,
                None if alive => {
                    let kid = self.thread(&child).cloned().or_else(|| self.store.thread(&child).ok().flatten());
                    let r = Report {
                        id: child.clone(),
                        title: kid.as_ref().map(|t| t.title.clone()).unwrap_or_default(),
                        model: kid.as_ref().map(|t| self.model_label(t)).unwrap_or_default(),
                        outcome: Outcome::Failed(format!("{} Its thread keeps what it did; start it again if you still need it.", orch::CUT_OFF)),
                    };
                    let _ = self.store.hold_report(&parent, &r);
                    r
                }
                _ => {
                    let _ = self.store.drop_report(&child);
                    continue;
                }
            };
            let line = if cut_off.contains(&parent) { &mut self.parked } else { &mut self.wakes };
            line.entry(parent).or_default().push(report);
        }
        // The quit ended its turn and the user will pick it up: its transcript says what's kept
        // for it, under the turn's "Interrupted".
        let parked: Vec<(String, usize)> = self.parked.iter().map(|(p, r)| (p.clone(), r.len())).collect();
        for (parent, n) in parked {
            self.ensure_loaded(&parent, cx);
            let agent = self.thread(&parent).map(|t| t.agent.display_name()).unwrap_or_default();
            if let Some(live) = self.live.get_mut(&parent) {
                live.items.push(Item::Notice { text: parked_notice(n, &agent) });
                live.revision += 1;
                self.persist_items(&parent, cx);
            }
        }
        if self.wakes.is_empty() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LAUNCH_WAKE).await;
            let _ = this.update(cx, |ws, cx| {
                let parents: Vec<String> = ws.wakes.keys().cloned().collect();
                for p in parents {
                    ws.deliver_wakes(&p, cx);
                }
            });
        });
        self.keep(task);
    }

    /// `parent`'s agent read `child`'s answer itself: no message brings it again, whether it's
    /// in line for the next wake-up or kept for after the user's turn (`parked`). The parked
    /// notice already in the transcript keeps its count.
    fn heard(&mut self, parent: &str, child: &str) {
        let mut had = false;
        for line in [&mut self.wakes, &mut self.parked] {
            if let Some(w) = line.get_mut(parent).filter(|w| w.iter().any(|r| r.id == child)) {
                w.retain(|r| r.id != child);
                had = true;
                if w.is_empty() {
                    line.remove(parent);
                }
            }
        }
        if had {
            let _ = self.store.drop_report(child);
        }
    }

    /// `id` had its turn cut off by the last quit with reports kept for it: a turn of the user's
    /// has ended there since, so they go out now.
    pub(super) fn unpark_wakes(&mut self, id: &str) {
        if let Some(reports) = self.parked.remove(id) {
            self.wakes.entry(id.to_string()).or_default().extend(reports);
        }
    }

    /// A sub-agent of `parent` ended without news for it: if `parent` is a sub-agent itself that
    /// was only waiting on it, idle with nothing left to hear, it's done too.
    fn parent_may_be_done(&mut self, parent: &str, cx: &mut Context<Self>) {
        let idle = self.live.get(parent).is_none_or(|l| l.turn_started.is_none() && l.background_agents().next().is_none() && l.queued.is_empty() && !l.preparing);
        let waiting = !self.running_children(parent).is_empty() || self.wakes.get(parent).is_some_and(|w| !w.is_empty());
        if idle && !waiting && self.delegations.get(parent).is_some_and(|d| d.outcome.is_none()) {
            self.task_turn_ended(parent, false, cx);
        }
    }

    /// Wake `parent` with its sub-agents' reports, once it's free to take them: no turn running,
    /// nothing asked of the user, no message of theirs on its way. What it runs in the background
    /// (a dev server, a browser) doesn't hold them back. Not loaded (Trek just started, say), it's
    /// read first; reports go out once they're in its transcript.
    pub(super) fn deliver_wakes(&mut self, parent: &str, cx: &mut Context<Self>) {
        if self.thread(parent).is_none_or(|t| t.archived_at.is_some()) {
            for r in self.wakes.remove(parent).unwrap_or_default() {
                let _ = self.store.drop_report(&r.id);
            }
            return;
        }
        if self.wakes.get(parent).is_none_or(|w| w.is_empty()) || self.gathering.contains_key(parent) {
            return;
        }
        self.ensure_loaded(parent, cx);
        let busy = self.live.get(parent).is_some_and(|l| l.turn_started.is_some() || !l.permissions.is_empty() || !l.held.is_empty() || l.loading)
            || self.holds_messages(parent);
        // Paused at a usage limit, it hears them once the limit lifts (`limits_due`, a resume).
        let paused = self.pause(parent).is_some_and(|p| p.resets_at.is_some());
        if busy || paused {
            return;
        }
        let Some(reports) = self.wakes.remove(parent).filter(|r| !r.is_empty()) else { return };
        for r in &reports {
            let _ = self.store.drop_report(&r.id);
        }
        // Follow-ups still queued here were left by a turn that failed or stopped.
        if let Some(l) = self.live.get_mut(parent).filter(|l| !l.queued.is_empty()) {
            l.hold_queue = true;
        }
        self.send_to(parent, orch::wake_text(&reports), vec![], cx);
    }

    /// The sub-agents `id` waits on while its agent has no turn of its own running: Trek's still
    /// at work for it (their reports wake it), and its agent's own working in the background (it
    /// takes a turn when they report). A turn blocked in a call that waits for one counts too,
    /// as does one whose only calls still running are its agent's own sub-agents (Claude's
    /// `Task`, run in the foreground).
    /// Empty when it waits on nothing, or on the user.
    pub fn waiting_on(&self, id: &str) -> Vec<Waited> {
        let Some(live) = self.live.get(id) else { return vec![] };
        let Some(t) = self.thread(id) else { return vec![] };
        if !live.permissions.is_empty() || t.run_state == RunState::NeedsYou {
            return vec![];
        }
        let turn = live.turn_started.is_some();
        let mut out: Vec<Waited> = self
            .running_children(id)
            .into_iter()
            // Mid-turn, only those it's blocked on: a call still waiting for their answer.
            .filter(|c| !turn || self.delegations.get(&c.id).is_some_and(|d| d.waiters.iter().any(|w| !w.is_closed())))
            .map(|c| {
                let model = self.model_label(c);
                Waited { agent: c.agent.clone(), name: format!("{model}: {}", c.title), model: Some(model), elapsed: self.task_elapsed(&c.id) }
            })
            .collect();
        if !turn {
            out.extend(live.background_agents().map(|b| Waited { agent: t.agent.clone(), name: b.task.title.clone(), model: None, elapsed: b.started.elapsed() }));
        } else if out.is_empty() {
            // Mid-turn, blocked on its agent's own: the calls still running are all sub-agents.
            out.extend(live.blocked_on_tasks().map(|k| Waited { agent: t.agent.clone(), name: k.description.clone(), model: None, elapsed: k.started.elapsed() }));
        }
        out
    }

    /// Whether `id` waits on sub-agents with no turn of its own running: it counts as working
    /// (the Working group, the working header), not as a thread that's done. Reports on their
    /// way to it count too.
    pub fn waiting(&self, id: &str) -> bool {
        let free = self.live.get(id).is_some_and(|l| l.turn_started.is_none() && l.permissions.is_empty());
        free && self.thread(id).is_some_and(|t| t.run_state == RunState::Idle && t.paused.is_none())
            && (!self.waiting_on(id).is_empty() || self.wakes.get(id).is_some_and(|w| !w.is_empty()))
    }

    /// Threads that `waiting` holds for, worked out together (for the sidebar's sections).
    pub fn waiting_threads(&self) -> HashSet<String> {
        let mut maybe: HashSet<&str> = self.delegations.values().filter(|d| d.outcome.is_none()).map(|d| d.parent.as_str()).collect();
        maybe.extend(self.live.iter().filter(|(_, l)| l.background_agents().next().is_some()).map(|(id, _)| id.as_str()));
        maybe.extend(self.wakes.iter().filter(|(_, w)| !w.is_empty()).map(|(id, _)| id.as_str()));
        maybe.into_iter().filter(|id| self.waiting(id)).map(str::to_string).collect()
    }

    /// `id` won't hear the reports held for it: it was stopped from above, or by the user.
    pub(super) fn forget_wakes(&mut self, id: &str) {
        for r in self.wakes.remove(id).unwrap_or_default() {
            let _ = self.store.drop_report(&r.id);
        }
        self.gathering.remove(id);
    }

    /// Put away `id`'s sub-agents with it (archiving): any still at work stop, quietly.
    pub(super) fn archive_children(&mut self, id: &str, cx: &mut Context<Self>) {
        for child in self.descendants(id).into_iter().filter(|t| t.archived_at.is_none()) {
            self.forget_limited(&child.id, cx);
            if let Some(d) = self.delegations.get_mut(&child.id) {
                d.cancelled = true;
            }
            if self.task_state(&child.id).live() {
                self.end_task_now(&child.id, cx);
            }
            if let Some(tx) = self.live.get_mut(&child.id).and_then(|l| l.commands.take()) {
                let _ = tx.try_send(Command::Shutdown);
            }
            let _ = self.store.update_thread(&child.id, |t| t.archived_at = Some(now_ms()));
            self.threads.retain(|t| t.id != child.id);
        }
    }

    /// Bring `id`'s sub-agents back with it (undoing an archive).
    pub(super) fn unarchive_children(&mut self, id: &str) {
        for child in self.descendants(id) {
            let _ = self.store.update_thread(&child.id, |t| t.archived_at = None);
        }
    }

    /// `id`'s sub-agents, all the way down, for deleting with it: their sessions stop.
    pub(super) fn drop_children(&mut self, id: &str, cx: &mut Context<Self>) -> Vec<Thread> {
        let all = self.descendants(id);
        for child in &all {
            if let Some(d) = self.delegations.get_mut(&child.id) {
                d.cancelled = true;
            }
            if self.task_state(&child.id).live() {
                self.end_task_now(&child.id, cx);
            }
            if let Some(tx) = self.live.get_mut(&child.id).and_then(|l| l.commands.take()) {
                let _ = tx.try_send(Command::Shutdown);
            }
            self.retire_ipc_session(&child.id);
            self.delegations.remove(&child.id);
        }
        all
    }

    /// Whether a sub-agent row of `scope`'s thread is still at work (it ticks).
    pub fn any_task_live_in(&self, scope: &Scope) -> bool {
        self.thread_id_in(scope).is_some_and(|id| !self.running_children(id).is_empty() || self.live.get(id).is_some_and(|l| l.active_tasks() > 0))
    }

    /// What archiving or deleting `id` does to its sub-agents, for the confirmation: `None`
    /// without any.
    pub fn sub_agents_note(&self, id: &str, verb: &str) -> Option<String> {
        let all = self.children(id);
        let running = all.iter().filter(|t| self.task_state(&t.id).live()).count();
        let n = all.len();
        let them = if n == 1 { "Its sub-agent is".to_string() } else { format!("Its {n} sub-agents are") };
        let stop = match running {
            0 => String::new(),
            1 if n == 1 => ", and stopped".into(),
            r => format!("; {r} still at work stop"),
        };
        (n > 0).then(|| format!("{them} {verb} with it{stop}."))
    }
}

/// Said in a thread whose turn a quit cut off while it waited on `n` sub-agents' reports: its
/// agent hears them after the user's next message there.
fn parked_notice(n: usize, agent: &str) -> String {
    match n {
        1 => format!("A sub-agent's report is kept for this thread: {agent} hears it once your next message here is answered."),
        _ => format!("{n} sub-agents' reports are kept for this thread: {agent} hears them once your next message here is answered."),
    }
}

#[cfg(test)]
impl Workspace {
    /// `parent` has reports in line that a delivery already tried and held back.
    pub(crate) fn wakes_held(&self, parent: &str) -> bool {
        !self.gathering.contains_key(parent) && self.wakes.get(parent).is_some_and(|w| !w.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::{Waited, waiting_label};
    use std::time::Duration;
    use trek_core::AgentId;

    fn on(model: Option<&str>) -> Waited {
        Waited { agent: AgentId::Codex, name: String::new(), model: model.map(str::to_string), elapsed: Duration::ZERO }
    }

    #[test]
    fn the_wait_names_who_by_model_and_counts_the_rest() {
        assert_eq!(waiting_label(&[on(Some("Sol"))]), "Waiting on Sol");
        assert_eq!(waiting_label(&[on(Some("Sol")), on(Some("Opus 5.5"))]), "Waiting on Sol and Opus 5.5");
        assert_eq!(waiting_label(&[on(Some("Sol")), on(Some("Sol"))]), "Waiting on 2 sub-agents", "two on one model aren't one");
        assert_eq!(waiting_label(&[on(Some("Sol")), on(None)]), "Waiting on Sol and a sub-agent");
        assert_eq!(waiting_label(&[on(None)]), "Waiting on a sub-agent");
        assert_eq!(waiting_label(&[on(None), on(None), on(Some("Sol"))]), "Waiting on 3 sub-agents");
        assert_eq!(waiting_label(&[]), "");
    }
}

