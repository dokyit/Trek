//! A thread's sub-agents at work, as every view shows them: the sidebar's badge and tooltip, the
//! working bar's count and its "Waiting on 2 sub-agents" list, the hub's row, the phone's group
//! and the transcript's live rows all read [`Workspace::sub_agents_at_work`], so they can't
//! disagree.
//!
//! Two kinds: Trek's own (`delegate_task`), each a thread of its own whose row in its parent is
//! `orchestrate::task_row`; and the agent's own (Claude Code's `Agent`, Codex's agents), one
//! `SubTask` per run, keyed by the call it works under. One woken again (`SendMessage`) works under
//! the call that woke it, on that call's row (`LiveThread::resume_task`); one the agent lists in
//! the background has a task and a row too (`LiveThread::set_background`). So each sub-agent at
//! work is one entry here and one live row in its thread.

use super::{LiveThread, Workspace};
use std::time::{Duration, Instant};
use trek_core::AgentId;
use trek_core::orchestrate as orch;
use trek_core::store::{Item, ToolStatus};

/// One sub-agent at work for a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct AtWork {
    /// Its row in the thread's transcript: the call it works under (the agent's own), or
    /// `orchestrate::task_row` of its thread (Trek's).
    pub row: String,
    /// Whose logo it wears.
    pub agent: AgentId,
    /// "Sol: Review the cache", or what the agent's own was sent to do.
    pub name: String,
    /// The model it runs ("Sol"), for one Trek runs.
    pub model: Option<String>,
    /// Its thread, for one Trek runs.
    pub child: Option<String>,
    /// When it started (this run, for one woken again), as this Mac saw it.
    pub started: Instant,
    /// How long it has worked (a usage limit's pause aside, for Trek's own).
    pub elapsed: Duration,
    /// The agent lists it in the background: it outlives the turn that started it, and the
    /// agent takes a turn when it reports.
    pub background: bool,
}

impl LiveThread {
    /// Its agent's own sub-agents at work, oldest first: those it reported (`SubTask`), then
    /// rows of this turn's still running for one it hasn't said more about yet (the moment
    /// before Claude's `task_started`; an agent that never says more than its call).
    pub fn own_at_work(&self, agent: &AgentId) -> Vec<AtWork> {
        let mut out: Vec<AtWork> = self
            .tasks
            .iter()
            .filter(|t| t.done.is_none())
            .map(|t| AtWork {
                row: t.id.clone(),
                agent: agent.clone(),
                name: t.description.clone(),
                model: None,
                child: None,
                started: t.started,
                elapsed: t.started.elapsed(),
                background: self
                    .background_agents()
                    .any(|b| b.task.call.as_deref() == Some(t.id.as_str())),
            })
            .collect();
        if let Some(since) = self.turn_started {
            let unreported: Vec<AtWork> = self
                .items
                .iter()
                .rev()
                .take_while(|i| !matches!(i, Item::User { .. }))
                .filter_map(|i| match i {
                    Item::Tool {
                        id,
                        title,
                        detail,
                        status: ToolStatus::Running,
                        ..
                    } if title == "Subagent" && !self.tasks.iter().any(|t| t.id == *id) => {
                        Some(AtWork {
                            row: id.clone(),
                            agent: agent.clone(),
                            name: detail.clone(),
                            model: None,
                            child: None,
                            started: since,
                            elapsed: since.elapsed(),
                            background: false,
                        })
                    }
                    _ => None,
                })
                .collect();
            out.extend(unreported.into_iter().rev());
        }
        out
    }
}

impl Workspace {
    /// Open the row of the sub-agent at work under `row` in thread `id`'s transcript, and scroll
    /// to it (the working bar's list, the hub): what it's doing shows there.
    pub fn reveal_sub_agent(&mut self, id: &str, row: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(live) = self.live.get_mut(id) else {
            return;
        };
        let n = live.revealed.as_ref().map_or(0, |(_, n)| n + 1);
        live.revealed = Some((row.to_string(), n));
        cx.emit(super::WorkspaceEvent::Transcript {
            id: id.to_string(),
            appended: false,
        });
        cx.notify();
    }

    /// Thread `id`'s sub-agents at work: Trek's own, then its agent's own.
    pub fn sub_agents_at_work(&self, id: &str) -> Vec<AtWork> {
        self.sub_agents_of(id, true)
    }

    /// The same, when the caller knows whether `id` has threads of Trek's under it at all
    /// (`trek`; the sidebar does, and looks for them only then).
    pub fn sub_agents_of(&self, id: &str, trek: bool) -> Vec<AtWork> {
        if crate::machines::is_remote(id) {
            return self.machine_sub_agents(id);
        }
        let now = Instant::now();
        let children = if trek {
            self.running_children(id)
        } else {
            vec![]
        };
        let mut out: Vec<AtWork> = children
            .into_iter()
            .map(|c| {
                let model = self.model_label(c);
                let elapsed = self.task_elapsed(&c.id);
                AtWork {
                    row: orch::task_row(&c.id),
                    agent: c.agent.clone(),
                    name: format!("{model}: {}", c.title),
                    model: Some(model),
                    child: Some(c.id.clone()),
                    started: now.checked_sub(elapsed).unwrap_or(now),
                    elapsed,
                    background: false,
                }
            })
            .collect();
        if let (Some(l), Some(t)) = (self.live.get(id), self.thread(id)) {
            out.extend(l.own_at_work(&t.agent));
        }
        out
    }

    /// A thread on another machine: its sub-agents at work as its row there lists them (the
    /// machine's own `sub_agents_at_work`, which its sidebar badge here reads too), each by the
    /// row it has in the transcript as the machine sent it. None while it can't be reached.
    fn machine_sub_agents(&self, id: &str) -> Vec<AtWork> {
        let Some((host, _)) = crate::machines::split_id(id) else {
            return vec![];
        };
        let Some(row) = self.machines.reachable_row(id) else {
            return vec![];
        };
        let now = Instant::now();
        let ms = trek_core::store::now_ms();
        row.sub_agents
            .iter()
            .filter(|k| {
                matches!(
                    k.state,
                    trek_remote::SubAgentState::Running | trek_remote::SubAgentState::NeedsYou
                )
            })
            .map(|k| {
                let elapsed = Duration::from_millis(k.since.map_or(0, |s| (ms - s).max(0) as u64));
                AtWork {
                    row: match (&k.thread_id, &k.call_id) {
                        (Some(child), _) => orch::task_row(child),
                        (None, Some(call)) => call.clone(),
                        (None, None) => String::new(),
                    },
                    agent: AgentId::from_key(&k.agent.key),
                    name: match &k.model {
                        Some(model) => format!("{model}: {}", k.title),
                        None => k.title.clone(),
                    },
                    model: k.model.clone(),
                    child: k
                        .thread_id
                        .as_deref()
                        .map(|c| crate::machines::local_id(host, c)),
                    started: now.checked_sub(elapsed).unwrap_or(now),
                    elapsed,
                    background: false,
                }
            })
            .collect()
    }

    /// `waiting_on` for a thread on another machine, from its row and transcript there: with its
    /// turn over, every sub-agent it has out; mid-turn, those its only calls still running are
    /// (a turn blocked on them). Nothing while something waits on the user.
    pub(super) fn machine_waiting_on(&self, id: &str) -> Vec<super::orchestrate::Waited> {
        let Some(row) = self.machines.reachable_row(id) else {
            return vec![];
        };
        if row.needs.is_some()
            || row.run_state == trek_remote::RunState::NeedsYou
            || self.live.get(id).is_some_and(|l| !l.permissions.is_empty())
        {
            return vec![];
        }
        let at_work = self.machine_sub_agents(id);
        let at_work: Vec<AtWork> = if row.run_state == trek_remote::RunState::Working {
            let Some(live) = self.live.get(id) else {
                return vec![];
            };
            // The calls of this turn still running, when they're all sub-agents.
            let running: Vec<&str> = live
                .items
                .iter()
                .rev()
                .take_while(|i| !matches!(i, Item::User { .. }))
                .filter_map(|i| match i {
                    Item::Tool {
                        id,
                        status: ToolStatus::Running,
                        ..
                    } => Some(id.as_str()),
                    _ => None,
                })
                .collect();
            let blocked = !running.is_empty()
                && live.streaming.is_none()
                && running.iter().all(|r| at_work.iter().any(|a| a.row == *r));
            if !blocked {
                return vec![];
            }
            at_work
                .into_iter()
                .filter(|a| running.contains(&a.row.as_str()))
                .collect()
        } else {
            at_work
        };
        at_work
            .into_iter()
            .map(|a| super::orchestrate::Waited {
                agent: a.agent,
                name: a.name,
                model: a.model,
                elapsed: a.elapsed,
            })
            .collect()
    }
}
