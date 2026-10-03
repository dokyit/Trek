//! Threads paused at a usage limit: the pause itself (kept on the thread, so it outlives Trek),
//! messages that wait for the reset, resuming at the reset, snoozing until it, and the handoff
//! to another agent.

use super::{Workspace, WorkspaceEvent};
use gpui_kit::{Context, Task};
use std::sync::Arc;
use std::time::Duration;
use trek_agents::LimitScope;
use trek_core::limit::{Pause, Queued};
use trek_core::settings::OnUsageLimit;
use trek_core::store::{Item, now_ms};
use trek_core::{AgentId, RunState};

/// Wall-clock time (unix ms) for usage limits. Tests run on a clock of their own.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> i64 + Send + Sync>);

impl Default for Clock {
    fn default() -> Self {
        Clock(Arc::new(now_ms))
    }
}

impl Clock {
    #[cfg(test)]
    pub fn new(f: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Clock(Arc::new(f))
    }

    pub fn now(&self) -> i64 {
        (self.0)()
    }
}

/// The longest the limit timer sleeps before it looks at the clock again: a Mac asleep through
/// a reset wakes to a timer that knows nothing of the time it lost.
const RECHECK: Duration = Duration::from_secs(30);

/// How long after launch overdue resumes wait: the windows are up and the agents found first.
pub(crate) const LAUNCH_DELAY_MS: i64 = 8_000;

/// A limit the running turn reported, applied when the turn ends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hit {
    pub message: String,
    pub resets_at: Option<i64>,
    pub scope: LimitScope,
}

impl Workspace {
    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    /// `id`'s pause, if a usage limit has it paused.
    pub fn pause(&self, id: &str) -> Option<&Pause> {
        self.thread(id).and_then(|t| t.paused.as_ref())
    }

    /// When the reset is unknown, the agent's usage windows may know it (`AgentStatus::limits`).
    fn reset_from_status(&self, agent: &AgentId, scope: &LimitScope) -> Option<i64> {
        let status = self.agent_status.get(&agent.key())?;
        trek_agents::limits::reset_from_usage(&status.limits, scope, self.now())
    }

    /// The turn of `id` ended at a usage limit: pause the thread until it resets. Messages queued
    /// behind the turn wait for the reset too. A thread that hits the limit again keeps the
    /// user's choices (`Pause::renewed`); one that hit it while resuming resumes again.
    pub(super) fn pause_at_limit(&mut self, id: &str, hit: Hit, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id).cloned() else { return };
        let now = self.now();
        let resets_at = trek_core::limit::reset_ahead(hit.resets_at.or_else(|| self.reset_from_status(&thread.agent, &hit.scope)), now);
        let (resuming, queued) = match self.live.get_mut(id) {
            Some(l) => (std::mem::take(&mut l.resuming), std::mem::take(&mut l.queued)),
            None => (false, vec![]),
        };
        let auto = self.settings.general.on_usage_limit == OnUsageLimit::Resume || resuming;
        let fresh = Pause::new(hit.message, resets_at, hit.scope, now, auto);
        let mut pause = match thread.paused {
            Some(old) => old.renewed(fresh),
            None => fresh,
        };
        pause.queued.extend(queued.into_iter().map(|(text, images)| Queued { text, images }));
        // Something the user typed waits for the reset: that's asking for the resume.
        pause.resume |= !pause.queued.is_empty();
        let notice = match pause.resets_at {
            Some(at) if pause.resume => format!("Paused until {}: {}", crate::time::reset_clock(at, now), thread.title),
            Some(at) => format!("Usage limit reached, resets {}: {}", crate::time::reset_clock(at, now), thread.title),
            None => format!("Usage limit reached: {}", thread.title),
        };
        let unknown = pause.resets_at.is_none();
        self.mutate_thread(id, cx, |t| {
            t.paused = Some(pause);
            t.run_state = RunState::Idle;
        });
        if let Some(l) = self.live.get_mut(id) {
            l.revision += 1;
        }
        // The agent's usage windows may say when it resets: ask them now rather than in minutes.
        if unknown && matches!(thread.agent, AgentId::ClaudeCode | AgentId::Codex) {
            self.status_fetched_at = 0;
            self.refresh_usage(cx);
        }
        if thread.side_of.is_none() {
            cx.emit(WorkspaceEvent::Attention { message: notice, thread: id.to_string() });
        }
        self.schedule_limits(cx);
    }

    /// Fill in resets the agents' usage windows now know (after `refresh_usage`).
    pub(super) fn fill_unknown_resets(&mut self, cx: &mut Context<Self>) {
        let unknown: Vec<(String, AgentId, LimitScope)> =
            self.threads.iter().filter_map(|t| t.paused.as_ref().filter(|p| p.resets_at.is_none()).map(|p| (t.id.clone(), t.agent.clone(), p.scope.clone()))).collect();
        let mut any = false;
        for (id, agent, scope) in unknown {
            if let Some(at) = self.reset_from_status(&agent, &scope) {
                self.mutate_thread(&id, cx, |t| {
                    if let Some(p) = t.paused.as_mut() {
                        p.resets_at = Some(at);
                    }
                });
                any = true;
            }
        }
        if any {
            self.schedule_limits(cx);
        }
    }

    /// Change `id`'s pause and save it.
    fn update_pause(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Pause)) {
        self.mutate_thread(id, cx, |t| {
            if let Some(p) = t.paused.as_mut() {
                f(p);
            }
        });
        if let Some(l) = self.live.get_mut(id) {
            l.revision += 1;
        }
        self.schedule_limits(cx);
    }

    /// "Resume at reset": send "continue" (or what's queued) shortly after the limit resets.
    pub fn resume_at_reset(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_pause(id, cx, |p| p.resume = true);
    }

    /// Call the scheduled resume off. What was queued for it goes back to the composer.
    pub fn cancel_resume(&mut self, id: &str, cx: &mut Context<Self>) {
        let queued = self.pause(id).map(|p| p.queued.clone()).unwrap_or_default();
        self.update_pause(id, cx, |p| {
            p.resume = false;
            p.queued.clear();
        });
        self.hand_back(id, queued, cx);
    }

    /// Take one message off the queue for the reset; it goes back to the composer.
    pub fn unqueue_for_reset(&mut self, id: &str, ix: usize, cx: &mut Context<Self>) {
        let Some(q) = self.pause(id).and_then(|p| p.queued.get(ix).cloned()) else { return };
        self.update_pause(id, cx, |p| {
            p.queued.remove(ix);
        });
        self.hand_back(id, vec![q], cx);
    }

    fn hand_back(&mut self, id: &str, queued: Vec<Queued>, cx: &mut Context<Self>) {
        if queued.is_empty() {
            return;
        }
        let text = queued.iter().map(|q| q.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        let images = queued.into_iter().flat_map(|q| q.images).collect();
        cx.emit(WorkspaceEvent::RestoreQueued { thread: id.to_string(), text, images });
    }

    /// "Snooze until reset": out of the inbox until the limit resets.
    pub fn snooze_until_reset(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(at) = self.pause(id).and_then(|p| p.resets_at) else { return };
        let label = crate::time::reset_clock(at, self.now());
        self.mutate_thread(id, cx, |t| {
            t.snoozed_until = Some(at);
            t.settled_at = None;
            t.branch = None;
        });
        cx.emit(WorkspaceEvent::Toast { message: format!("Snoozed until {label}"), undo: None });
    }

    /// The pause is over now (the user switched agents, or tries again by hand): what was queued
    /// goes back to the composer, unless `send` sends it now.
    pub fn end_pause(&mut self, id: &str, send: bool, cx: &mut Context<Self>) {
        let Some(pause) = self.pause(id).cloned() else { return };
        self.mutate_thread(id, cx, |t| t.paused = None);
        if let Some(l) = self.live.get_mut(id) {
            l.revision += 1;
        }
        if send {
            self.send_resume(id, pause.messages(), cx);
        } else {
            self.hand_back(id, pause.queued, cx);
        }
        self.schedule_limits(cx);
    }

    /// Queue `text` to go when `id`'s limit resets (it was typed while the thread is paused).
    pub(super) fn queue_for_reset(&mut self, id: &str, text: String, images: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.update_pause(id, cx, |p| {
            p.queued.push(Queued { text, images });
            p.resume = true;
        });
    }

    /// Set the timer for the next pause to end (one timer for all threads), or none.
    pub(crate) fn schedule_limits(&mut self, cx: &mut Context<Self>) {
        let next = trek_core::limit::next_end(self.threads.iter().filter_map(|t| t.paused.as_ref()));
        let Some(next) = next else {
            self.limit_timer = None;
            return;
        };
        let wait = (next.max(self.resumes_from) - self.now()).clamp(0, RECHECK.as_millis() as i64);
        self.limit_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait as u64)).await;
            let _ = this.update(cx, |this, cx| this.limits_due(cx));
        }));
    }

    /// End the pauses whose time has come: resume the ones that asked for it, and simply lift the
    /// rest (the limit has reset; the bar goes).
    fn limits_due(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let due: Vec<(String, bool)> = if now < self.resumes_from {
            vec![]
        } else {
            self.threads.iter().filter_map(|t| t.paused.as_ref().filter(|p| p.is_over(now)).map(|p| (t.id.clone(), p.resume))).collect()
        };
        for (id, resume) in due {
            if resume {
                self.resume_when_clear(&id, cx);
            } else {
                self.mutate_thread(&id, cx, |t| t.paused = None);
            }
        }
        self.schedule_limits(cx);
    }

    /// Resume `id`, after making sure the limit has really gone where the agent can say (Claude
    /// Code and Codex report their usage windows): still used up, the pause moves to the new reset.
    fn resume_when_clear(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id).cloned() else { return };
        let checkable = matches!(t.agent, AgentId::ClaudeCode | AgentId::Codex) && !trek_core::paths::isolated();
        if !checkable {
            self.resume_now(id, cx);
            return;
        }
        if !self.limit_checks.insert(id.to_string()) {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        let (agent, cwd) = (t.agent.clone(), t.cwd.clone().unwrap_or_else(trek_core::paths::home));
        trek_core::runtime().spawn(async move {
            let status = match agent {
                AgentId::ClaudeCode => trek_agents::claude_status(&cwd).await,
                _ => trek_agents::codex_status(&cwd).await,
            };
            let _ = tx.send(status).await;
        });
        let id = id.to_string();
        let task: Task<()> = cx.spawn(async move |this, cx| {
            let status = rx.recv().await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                this.limit_checks.remove(&id);
                let now = this.now();
                let still = status.as_ref().and_then(|s| s.limits.iter().filter(|l| l.percent >= 99.5).filter_map(|l| l.resets_at).filter(|at| *at > now).max());
                match still {
                    Some(at) => this.update_pause(&id, cx, |p| p.resets_at = Some(at)),
                    None => this.resume_now(&id, cx),
                }
                if let Some(s) = status {
                    this.agent_status.insert(t.agent.key(), s);
                }
            });
        });
        self.keep(task);
    }

    /// The limit has reset: send what waited for it (or "continue"), in order, and say so.
    fn resume_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pause) = self.pause(id).cloned() else { return };
        // A turn running (the user carried on by hand): what's queued follows it.
        self.mutate_thread(id, cx, |t| {
            t.paused = None;
            t.snoozed_until = None;
        });
        self.send_resume(id, pause.messages(), cx);
        let title = self.thread(id).map(|t| t.title.clone()).unwrap_or_default();
        if self.thread(id).is_some_and(|t| t.side_of.is_none()) {
            cx.emit(WorkspaceEvent::Attention { message: format!("Resumed: {title}"), thread: id.to_string() });
        }
    }

    /// Send `messages` to `id`: the first starts a turn, the rest queue behind it.
    fn send_resume(&mut self, id: &str, messages: Vec<Queued>, cx: &mut Context<Self>) {
        let mut messages = messages.into_iter();
        let Some(first) = messages.next() else { return };
        let running = self.turn_running(id);
        let live = self.live.entry(id.to_string()).or_default();
        if running {
            live.queued.push((first.text, first.images));
        } else {
            live.resuming = true;
            self.send_to(id, first.text, first.images, cx);
        }
        let live = self.live.entry(id.to_string()).or_default();
        live.queued.extend(messages.map(|q| (q.text, q.images)));
        live.revision += 1;
        cx.notify();
    }

    /// At launch: resumes that came due while Trek was closed go once it's up, with a word.
    pub(super) fn resume_overdue(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let overdue = self.threads.iter().filter(|t| t.paused.as_ref().is_some_and(|p| p.resume && p.is_over(now))).count();
        if overdue > 0 {
            self.resumes_from = now + LAUNCH_DELAY_MS;
            let message = if overdue == 1 {
                "A usage limit reset while Trek was closed: its thread resumes in a moment.".to_string()
            } else {
                format!("Usage limits reset while Trek was closed: {overdue} threads resume in a moment.")
            };
            // Spawned so it lands after the window has subscribed to workspace events.
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            })
            .detach();
        }
        self.schedule_limits(cx);
    }

    /// The thread moved from one agent to another mid-conversation: a divider says so (the new
    /// agent starts from a recap). Switching back and forth before sending leaves one divider,
    /// or none once it's back where it started.
    pub(super) fn note_handoff(&mut self, id: &str, from: (&AgentId, Option<String>), to: (&AgentId, Option<String>)) -> bool {
        let Some(live) = self.live.get_mut(id) else { return false };
        if !live.items.iter().any(|i| matches!(i, Item::User { aside: false, .. })) {
            return false;
        }
        let (mut from_key, mut from_model) = (from.0.key(), from.1);
        if let Some(Item::Handoff { from, from_model: m, .. }) = live.items.last() {
            (from_key, from_model) = (from.clone(), m.clone());
            let last = live.items.len() - 1;
            live.items.truncate(last);
        }
        if from_key != to.0.key() {
            live.items.push(Item::Handoff { from: from_key, from_model, to: to.0.key(), to_model: to.1 });
        }
        live.revision += 1;
        true
    }
}
