//! Sub-agents. An agent in Trek hands work to another agent and model through Trek's
//! orchestration tools (`trek-mcp orchestrate` → `ipc` → `handle_call`). Each sub-agent is a
//! thread of its own whose `parent_id` is the thread that asked: it runs in its parent's folder,
//! stays out of the inbox, shows as a row in its parent's transcript, and reports back with its
//! last message: to a caller still waiting, else in a message that wakes the parent.

use super::{Scope, Workspace};
use crate::ipc::{Call, Reply};
use gpui_kit::{Context, Task};
use serde_json::{Value, json};
use std::time::Duration;
use trek_agents::{AgentEvent, Command, Decision, McpServer};
use trek_core::catalog;
use trek_core::orchestrate::{self as orch, MAX_DEPTH, MAX_RUNNING, Mode, Outcome, Report};
use trek_core::store::{Item, Thread, ToolStatus, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState};

/// What Trek keeps about a sub-agent it started this run.
pub struct Delegation {
    pub parent: String,
    pub mode: Mode,
    /// The plan it offered, advising in plan mode: that's its advice.
    plan: Option<String>,
    /// Calls waiting for its answer (`delegate_task` with `wait`).
    waiters: Vec<async_channel::Sender<Reply>>,
    /// Stopped on its parent's behalf (Stop, `cancel_task`, archiving): its end isn't news.
    cancelled: bool,
    /// How it ended, once it has. Later turns (the user carrying on in it) don't report again.
    pub outcome: Option<Outcome>,
    /// Ends it for good if it hasn't stopped within `STOP_GRACE` of being asked to.
    _stop: Option<Task<()>>,
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
/// The per-call limit Codex gets for Trek's orchestration tools: longer than the longest wait.
const TOOL_TIMEOUT_SECS: u64 = 1900;

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
                        let _ = call.reply.try_send(Reply::Waiting(child));
                        return;
                    }
                    Ok(child) => {
                        let mut v = self.task_json(&child, false);
                        v["note"] = json!("It runs on its own. Trek sends you its answer in a message when it finishes: end your turn rather than polling.");
                        Ok(v)
                    }
                    Err(e) => Err(e),
                },
                "task_status" => self.task_status(&call.thread, &id()),
                "task_result" => self.task_result(&call.thread, &id()),
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
        let text = |k: &str| params.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let title = text("title").ok_or("delegate_task needs a title: a few words naming the task.")?;
        let prompt = text("prompt").ok_or("delegate_task needs a prompt: the sub-agent's whole task, complete on its own.")?;
        let mode = match text("mode") {
            None => Mode::Advise,
            Some(m) => Mode::parse(m).ok_or_else(|| format!("mode is \"advise\" or \"implement\", not “{m}”."))?,
        };
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
        if let Some(why) = self.at_limit(&agent) {
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
        self.delegations.insert(id.clone(), Delegation { parent: caller.to_string(), mode, plan: None, waiters: vec![], cancelled: false, outcome: None, _stop: None });
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

    /// Why `agent` can't take more work now: a usage limit it has used up.
    fn at_limit(&self, agent: &AgentId) -> Option<String> {
        let st = self.agent_status.get(&agent.key())?;
        let l = st.limits.iter().find(|l| l.percent >= 100.)?;
        let resets = l.resets_at.map(|r| format!(" (it resets in {})", crate::time::until(r))).unwrap_or_default();
        Some(format!("{} has used up its {}{resets}. Pick another agent.", agent.display_name(), l.label.to_lowercase()))
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
        let busy = self.live.get(child).is_some_and(|l| l.turn_started.is_some() || l.background > 0 || !l.queued.is_empty() || l.preparing);
        match t.run_state {
            RunState::NeedsYou => TaskState::NeedsYou,
            _ if busy => TaskState::Running,
            RunState::Working if self.delegations.contains_key(child) => TaskState::Running,
            RunState::Failed => TaskState::Failed,
            // Ended in an earlier run: its row says how.
            _ => match self.task_row_status(child) {
                Some(ToolStatus::Denied) => TaskState::Cancelled,
                Some(ToolStatus::Failed) => TaskState::Failed,
                _ => TaskState::Done,
            },
        }
    }

    fn task_row_status(&self, child: &str) -> Option<ToolStatus> {
        let parent = self.thread(child)?.parent_id.clone()?;
        let row = orch::task_row(child);
        self.live.get(&parent)?.items.iter().rev().find_map(|i| match i {
            Item::Tool { id, status, .. } if *id == row => Some(*status),
            _ => None,
        })
    }

    /// How long `child` has run (so far, or in all).
    pub fn task_elapsed(&self, child: &str) -> Duration {
        let Some(t) = self.thread(child) else { return Duration::ZERO };
        let end = if self.task_state(child).live() { now_ms() } else { t.updated_at };
        Duration::from_millis((end - t.created_at).max(0) as u64)
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
            TaskState::Failed => orch::last_error(&items),
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
            v["note"] = json!("Not finished yet. Trek sends you its answer when it is.");
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

    /// Stop every sub-agent of `parent` that's still at work (Stop on the parent).
    pub(super) fn stop_children(&mut self, parent: &str, cx: &mut Context<Self>) {
        let running: Vec<String> = self.running_children(parent).into_iter().map(|t| t.id.clone()).collect();
        for child in running {
            self.stop_task(&child, cx);
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
            if live.turn_started.take().is_some() || live.background > 0 {
                live.close_turn(false);
                live.streaming = None;
                live.reasoning = None;
                live.items.push(Item::Notice { text: "Stopped".into() });
            }
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
        let failed = self.thread(id).is_some_and(|t| t.run_state == RunState::Failed);
        let items = self.live.get(id).map(|l| l.items.to_vec()).unwrap_or_default();
        let outcome = if interrupted || d.cancelled {
            Outcome::Cancelled
        } else if failed {
            Outcome::Failed(orch::last_error(&items).unwrap_or_else(|| "It stopped without saying why.".into()))
        } else {
            Outcome::Done(orch::final_answer(&items, d.plan.as_deref()).unwrap_or_else(|| "(It finished without saying anything.)".into()))
        };
        self.finish_task(id, outcome, cx);
    }

    /// Sub-agent `child` ended as `outcome`: its row in the parent shows it, its session goes,
    /// and its answer reaches the parent: the calls waiting for it, else a message that wakes
    /// the parent (not when the parent's side stopped it).
    fn finish_task(&mut self, child: &str, outcome: Outcome, cx: &mut Context<Self>) {
        let Some(d) = self.delegations.get_mut(child).filter(|d| d.outcome.is_none()) else { return };
        d.outcome = Some(outcome.clone());
        d._stop = None;
        let (parent, waiters, silent) = (d.parent.clone(), std::mem::take(&mut d.waiters), d.cancelled);
        let (status, output) = match &outcome {
            Outcome::Done(answer) => (ToolStatus::Done, orch::cap(answer, orch::RESULT_CAP, "Open the sub-agent's thread for the rest.")),
            Outcome::Failed(why) => (ToolStatus::Failed, why.clone()),
            Outcome::Cancelled => (ToolStatus::Denied, String::new()),
        };
        self.settle_task_row(&parent, child, status, output, cx);
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
            self.wakes.entry(parent.clone()).or_default().push(report);
            self.deliver_wakes(&parent, cx);
        }
        cx.notify();
    }

    /// Wake `parent` with its sub-agents' reports, once it's free to take them: no turn running,
    /// nothing asked of the user, no message of theirs waiting.
    pub(super) fn deliver_wakes(&mut self, parent: &str, cx: &mut Context<Self>) {
        if self.thread(parent).is_none() {
            self.wakes.remove(parent);
            return;
        }
        let busy = self.live.get(parent).is_some_and(|l| {
            l.turn_started.is_some() || l.background > 0 || !l.permissions.is_empty() || !l.queued.is_empty() || !l.held.is_empty() || l.loading || l.preparing
        });
        if busy {
            return;
        }
        let Some(reports) = self.wakes.remove(parent).filter(|r| !r.is_empty()) else { return };
        self.send_to(parent, orch::wake_text(&reports), vec![], cx);
    }

    /// Put away `id`'s sub-agents with it (archiving): any still at work stop, quietly.
    pub(super) fn archive_children(&mut self, id: &str, cx: &mut Context<Self>) {
        for child in self.descendants(id).into_iter().filter(|t| t.archived_at.is_none()) {
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
