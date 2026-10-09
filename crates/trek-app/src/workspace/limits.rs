//! Threads paused at a usage limit: the pause itself (kept on the thread, so it outlives Trek),
//! messages that wait for the reset, resuming at the reset, snoozing until it, and the handoff
//! to another agent.

use super::{Command, Workspace, WorkspaceEvent};
use gpui_kit::{Context, Task};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use trek_agents::{AgentStatus, LimitScope};
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
    /// Not the limit itself: the turn stopped ahead of it, as Trek asked (`Workspace::wrap_up`).
    pub wrapped: bool,
}

/// How full one of a plan's usage windows is (0–100), and when it resets (unix ms).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Window {
    pub scope: LimitScope,
    pub percent: f32,
    pub resets_at: Option<i64>,
}

/// The window a running turn was asked to wrap up ahead of.
pub(crate) type WrapUp = Window;

/// How full a window is when Trek starts re-reading an agent's usage more often for the turns
/// running on it (`Workspace::usage_every`).
const FILLING: f32 = 85.0;

/// How often a working agent's usage is re-read while a window fills up.
const FILLING_EVERY_MS: i64 = 2 * 60_000;

/// Of `windows`, the one to wrap up for: close to its limit, and the last of those to reset
/// (the thread waits for that one).
fn due(windows: impl IntoIterator<Item = Window>) -> Option<Window> {
    windows.into_iter().filter(|w| trek_core::limit::wrap_up_due(&w.scope, w.percent)).max_by_key(|w| w.resets_at)
}

/// A turn that resumes a thread after its usage limit: the message of the user's it sent (sent
/// again if the limit is still there), and how many resumes before it met the limit again.
#[derive(Debug, Clone, Default)]
pub(crate) struct Resuming {
    pub sent: Option<Queued>,
    pub tries: u32,
}

/// Asks an agent for its usage windows before a resume, answering on the channel (`None`: it
/// couldn't say). Claude Code and Codex report them; tests answer for any agent.
pub type UsageProbe = Rc<dyn Fn(&AgentId, &Path) -> async_channel::Receiver<Option<AgentStatus>>>;

/// Claude Code's, Codex's and Devin's own usage report (`claude_status`, `codex_status`,
/// `devin_status`), off the main thread.
fn ask_agent(agent: &AgentId, cwd: &Path) -> async_channel::Receiver<Option<AgentStatus>> {
    let (tx, rx) = async_channel::bounded(1);
    let (agent, cwd) = (agent.clone(), cwd.to_path_buf());
    trek_core::runtime().spawn(async move {
        let status = match agent {
            AgentId::ClaudeCode => trek_agents::claude_status(&cwd).await,
            AgentId::Acp(_) => trek_agents::devin_status().await,
            _ => trek_agents::codex_status(&cwd).await,
        };
        let _ = tx.send(status.ok()).await;
    });
    rx
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
    fn reset_from_status(&self, agent: &AgentId, model: Option<&str>, scope: &LimitScope) -> Option<i64> {
        let status = self.agent_status.get(&agent.key())?;
        trek_agents::limits::reset_from_usage(&status.limits, scope, model, self.now())
    }

    /// The turn of `id` ended at a usage limit: pause the thread until it resets. Messages queued
    /// behind the turn wait for the reset too. A thread that hits the limit again keeps the
    /// user's choices (`Pause::renewed`); one that hit it while resuming resumes again, quietly,
    /// with what it had sent.
    pub(super) fn pause_at_limit(&mut self, id: &str, hit: Hit, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id).cloned() else { return };
        // A side chat has no bar to wait behind: its panel shows the limit row, and what's typed
        // there next goes to the agent as usual.
        if thread.side_of.is_some() {
            if let Some(l) = self.live.get_mut(id) {
                l.resuming = None;
            }
            return;
        }
        let now = self.now();
        let (resuming, queued) = match self.live.get_mut(id) {
            Some(l) => (l.resuming.take(), std::mem::take(&mut l.queued)),
            None => (None, vec![]),
        };
        let tries = resuming.as_ref().map_or(0, |r| r.tries + 1);
        let reported = hit.resets_at.or_else(|| self.reset_from_status(&thread.agent, thread.model.as_deref(), &hit.scope));
        let resets_at = trek_core::limit::reset_ahead(reported, now, tries);
        let auto = self.settings.general.on_usage_limit == OnUsageLimit::Resume || resuming.is_some();
        let mut fresh = Pause::new(hit.message, resets_at, hit.scope, now, auto);
        fresh.tries = tries;
        fresh.wrapped = hit.wrapped;
        let mut pause = match thread.paused {
            Some(old) => old.renewed(fresh),
            None => fresh,
        };
        // What the resume sent of the user's met the limit too: it goes again, first.
        pause.queued.extend(resuming.as_ref().and_then(|r| r.sent.clone()));
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
        // Read whether or not the Usage card shows it.
        if unknown && matches!(thread.agent, AgentId::ClaudeCode | AgentId::Codex) {
            self.fetch_usage(thread.agent.clone(), true, cx);
        } else if unknown && thread.agent == super::devin_agent() {
            self.devin_status_at = 0;
            self.refresh_devin_usage(cx);
        }
        // A resume that met the limit again moves on without a word: the bar and the card say it,
        // and the alert for the limit went when it was first hit.
        // A sub-agent's limit goes to its parent, as its failure (`task_turn_ended`), not to the user.
        // A turn that wrapped up has its own alert, as any finished turn: its answer says where it stopped.
        if resuming.is_none() && thread.parent_id.is_none() && !hit.wrapped {
            cx.emit(WorkspaceEvent::Attention { message: notice, thread: id.to_string() });
        }
        self.schedule_limits(cx);
    }

    /// `id`'s agent said how full its plan's windows are (`AgentEvent::LimitUsed`): the usage
    /// Trek shows follows, and a turn running close to a limit is asked to wrap up.
    pub(super) fn windows_used(&mut self, id: &str, windows: Vec<Window>, cx: &mut Context<Self>) {
        let Some(agent) = self.thread(id).map(|t| t.agent.key()) else { return };
        if let Some(status) = self.agent_status.get_mut(&agent) {
            for w in &windows {
                let length = match w.scope {
                    LimitScope::Session => "5h",
                    LimitScope::Weekly => "7d",
                    _ => continue,
                };
                // The account's own window of that length, not a model's ("Weekly · Fable").
                if let Some(l) = status.limits.iter_mut().find(|l| l.window == length && !l.label.contains('·') && !l.label.contains("(scoped)")) {
                    l.percent = w.percent;
                    l.resets_at = w.resets_at.or(l.resets_at);
                }
            }
        }
        if let Some(w) = due(windows) {
            self.wrap_up(id, w, cx);
        }
    }

    /// `agent`'s usage was just read (`refresh_usage`, `refresh_devin_usage`): the turns running
    /// on it that a window close to its limit applies to are asked to wrap up.
    pub(crate) fn wrap_up_where_due(&mut self, agent: &AgentId, cx: &mut Context<Self>) {
        let Some(limits) = self.agent_status.get(&agent.key()).map(|s| s.limits.clone()).filter(|l| !l.is_empty()) else { return };
        let running: Vec<(String, Option<String>)> =
            self.threads.iter().filter(|t| t.agent == *agent && self.turn_running(&t.id)).map(|t| (t.id.clone(), t.model.clone())).collect();
        for (id, model) in running {
            let applying = limits.iter().filter(|l| trek_agents::limits::applies(l, &LimitScope::Other, model.as_deref()));
            if let Some(w) = due(applying.map(|l| Window { scope: trek_agents::limits::window_scope(l), percent: l.percent, resets_at: l.resets_at })) {
                self.wrap_up(&id, w, cx);
            }
        }
    }

    /// Ask `id`'s running turn to wrap up: window `w` is nearly used up, and the limit would
    /// cut the agent off wherever it happens to be. Once a turn; the message steers the turn
    /// like one the user sent, and a note in the transcript says it went. A turn that then ends
    /// by itself pauses the thread until the reset (`pause_wrapped`).
    pub(super) fn wrap_up(&mut self, id: &str, w: Window, cx: &mut Context<Self>) {
        let now = self.now();
        if !self.settings.general.wrap_up_near_limit || !trek_core::limit::wrap_up_due(&w.scope, w.percent) || w.resets_at.is_some_and(|at| at <= now) {
            return;
        }
        let Some(live) = self.live.get_mut(id) else { return };
        if live.turn_started.is_none() || live.commands.is_none() || live.wrap_up.is_some() || live.limit.is_some() {
            return;
        }
        let reset = w.resets_at.map(|at| crate::time::reset_clock(at, now));
        let prompt = trek_core::limit::wrap_up_prompt(&w.scope, w.percent, reset.as_deref());
        let note = match &reset {
            Some(at) => format!("{} {}% used (resets {at}): asked the agent to wrap up", w.scope.label(), w.percent.floor() as u32),
            None => format!("{} {}% used: asked the agent to wrap up", w.scope.label(), w.percent.floor() as u32),
        };
        live.wrap_up = Some(w);
        live.streaming = None;
        live.items.push(Item::Notice { text: note });
        live.revision += 1;
        self.dispatch(id, Command::Prompt { text: prompt, images: vec![] });
        self.persist_soon(id, cx);
        cx.notify();
    }

    /// `id`'s turn ended by itself after it was asked to wrap up ahead of `w`'s limit: the thread
    /// pauses until the reset, like one the limit stopped (what's queued waits, and it resumes
    /// or asks, as set). Not a sub-agent (its answer goes to its parent as it is) or a side
    /// chat, and not without a reset still ahead to wait for.
    pub(super) fn pause_wrapped(&mut self, id: &str, w: WrapUp, cx: &mut Context<Self>) -> bool {
        let Some(t) = self.thread(id) else { return false };
        if t.side_of.is_some() || t.parent_id.is_some() || !w.resets_at.is_some_and(|at| at > self.now()) {
            return false;
        }
        let message = trek_core::limit::wrapped_message(&w.scope, w.percent);
        self.pause_at_limit(id, Hit { message, resets_at: w.resets_at, scope: w.scope, wrapped: true }, cx);
        true
    }

    /// Whether a turn is running on `agent` while one of its windows fills up (wrap-ups on).
    fn filling(&self, agent: &AgentId) -> bool {
        self.settings.general.wrap_up_near_limit
            && self.agent_status.get(&agent.key()).is_some_and(|s| s.limits.iter().any(|l| (FILLING..100.0).contains(&l.percent)))
            && self.threads.iter().any(|t| t.agent == *agent && self.turn_running(&t.id))
    }

    /// How long Claude Code's and Codex's usage is left unread (unix ms): five minutes, or two
    /// while a turn runs on one whose window is filling up, so a wrap-up comes in time.
    pub(super) fn usage_every(&self) -> i64 {
        if self.filling(&AgentId::ClaudeCode) || self.filling(&AgentId::Codex) { FILLING_EVERY_MS } else { 5 * 60_000 }
    }

    /// Devin reports no usage as it works, and reading it costs a few seconds of its terminal
    /// UI (`refresh_devin_usage`, at most every ten minutes): worth it only while a turn runs on
    /// it and its quota is filling up, or isn't known yet.
    pub(super) fn watches_devin_usage(&self) -> bool {
        let devin = super::devin_agent();
        self.filling(&devin) || (self.settings.general.wrap_up_near_limit && self.devin_status_at == 0 && self.threads.iter().any(|t| t.agent == devin && self.turn_running(&t.id)))
    }

    /// Fill in resets the agents' usage windows now know (after `refresh_usage`).
    pub(super) fn fill_unknown_resets(&mut self, cx: &mut Context<Self>) {
        let unknown: Vec<(String, AgentId, Option<String>, LimitScope)> = self
            .threads
            .iter()
            .filter_map(|t| t.paused.as_ref().filter(|p| p.resets_at.is_none()).map(|p| (t.id.clone(), t.agent.clone(), t.model.clone(), p.scope.clone())))
            .collect();
        let mut any = false;
        for (id, agent, model, scope) in unknown {
            if let Some(at) = self.reset_from_status(&agent, model.as_deref(), &scope) {
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
            self.send_resume(id, &pause, cx);
        } else {
            self.hand_back(id, pause.queued, cx);
        }
        self.schedule_limits(cx);
    }

    /// `text` was typed while `id` is paused. With a reset to wait for, it waits for it, after
    /// what's queued already. With none, the user is trying again: the pause ends and what was
    /// queued goes first. Handed back when it's to go the usual way.
    pub(super) fn send_while_paused(&mut self, id: &str, text: String, images: Vec<std::path::PathBuf>, cx: &mut Context<Self>) -> Option<(String, Vec<std::path::PathBuf>)> {
        let Some(pause) = self.pause(id).cloned() else { return Some((text, images)) };
        if pause.resets_at.is_some() {
            self.update_pause(id, cx, |p| {
                p.queued.push(Queued { text, images });
                p.resume = true;
            });
            return None;
        }
        self.mutate_thread(id, cx, |t| t.paused = None);
        self.schedule_limits(cx);
        if pause.queued.is_empty() {
            return Some((text, images));
        }
        let mut pause = pause;
        pause.queued.push(Queued { text, images });
        pause.tries = 0;
        self.send_resume(id, &pause, cx);
        None
    }

    /// Set the timer for the next pause to end (one timer for all threads), or none. A thread whose
    /// agent is being asked about its limit waits for the answer (`resume_when_clear`).
    pub(crate) fn schedule_limits(&mut self, cx: &mut Context<Self>) {
        let checking = &self.limit_checks;
        let next = trek_core::limit::next_end(self.threads.iter().filter(|t| !checking.contains(&t.id)).filter_map(|t| t.paused.as_ref()));
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
            self.threads
                .iter()
                .filter(|t| !self.limit_checks.contains(&t.id))
                .filter_map(|t| t.paused.as_ref().filter(|p| p.is_over(now)).map(|p| (t.id.clone(), p.resume)))
                .collect()
        };
        for (id, resume) in due {
            if resume {
                self.resume_when_clear(&id, cx);
            } else {
                self.mutate_thread(&id, cx, |t| t.paused = None);
                // Sub-agents that reported while it was paused wake it now.
                self.deliver_wakes(&id, cx);
            }
        }
        self.schedule_limits(cx);
    }

    /// Resume `id`, after making sure the limit has really gone where the agent can say (Claude
    /// Code, Codex and Devin report their usage windows): still used up, the pause moves to the
    /// new reset.
    fn resume_when_clear(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id).cloned() else { return };
        let probe: Option<UsageProbe> = match &self.usage_probe {
            Some(p) => Some(p.clone()),
            None if (matches!(t.agent, AgentId::ClaudeCode | AgentId::Codex) || t.agent == super::devin_agent()) && !trek_core::paths::isolated() => Some(Rc::new(ask_agent)),
            None => None,
        };
        let Some(probe) = probe else {
            self.resume_now(id, cx);
            return;
        };
        if !self.limit_checks.insert(id.to_string()) {
            return;
        }
        let rx = probe(&t.agent, &t.cwd.clone().unwrap_or_else(trek_core::paths::home));
        let id = id.to_string();
        let task: Task<()> = cx.spawn(async move |this, cx| {
            let status = rx.recv().await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.limit_checks.remove(&id);
                let now = this.now();
                // The user called the resume off (or moved on) meanwhile.
                let Some(pause) = this.pause(&id).filter(|p| p.resume).cloned() else {
                    this.schedule_limits(cx);
                    return;
                };
                let model = this.thread(&id).and_then(|t| t.model.clone());
                let still = status.as_ref().and_then(|s| trek_agents::limits::limited_until(&s.limits, &pause.scope, model.as_deref(), now));
                if let Some(s) = status {
                    this.agent_status.insert(t.agent.key(), s);
                }
                match still {
                    Some(at) => this.update_pause(&id, cx, |p| p.resets_at = Some(at)),
                    None => this.resume_now(&id, cx),
                }
                this.schedule_limits(cx);
            });
        });
        self.keep(task);
    }

    /// The limit has reset: send what waited for it (or "continue"), in order, and say so (once:
    /// not again for each try after a resume that met the limit again).
    fn resume_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pause) = self.pause(id).cloned() else { return };
        // A turn running (the user carried on by hand): what's queued follows it.
        self.mutate_thread(id, cx, |t| {
            t.paused = None;
            t.snoozed_until = None;
        });
        self.send_resume(id, &pause, cx);
        let title = self.thread(id).map(|t| t.title.clone()).unwrap_or_default();
        if pause.tries == 0 && self.thread(id).is_some_and(|t| t.side_of.is_none() && t.parent_id.is_none()) {
            cx.emit(WorkspaceEvent::Attention { message: format!("Resumed: {title}"), thread: id.to_string() });
        }
    }

    /// Send what `pause` held for `id`: the first message starts a turn, the rest queue behind it.
    /// The thread's history is read first, so the agent (or the recap it starts from) has it.
    fn send_resume(&mut self, id: &str, pause: &Pause, cx: &mut Context<Self>) {
        self.ensure_loaded(id, cx);
        // A sub-agent takes its task back up, before its session starts (an advising one's is
        // read-only).
        self.task_resumed(id, cx);
        let mut messages = pause.messages().into_iter();
        let Some(first) = messages.next() else { return };
        let running = self.turn_running(id);
        let live = self.live.entry(id.to_string()).or_default();
        if running {
            live.queued.push((first.text, first.images));
        } else {
            live.resuming = Some(Resuming { sent: (!pause.queued.is_empty()).then(|| first.clone()), tries: pause.tries });
            self.send_to(id, first.text, first.images, cx);
        }
        let live = self.live.entry(id.to_string()).or_default();
        live.queued.extend(messages.map(|q| (q.text, q.images)));
        live.revision += 1;
        cx.notify();
    }

    /// At launch: resumes that came due while Trek was closed go once it's up, with a word.
    pub(super) fn resume_overdue(&mut self, cx: &mut Context<Self>) {
        self.drop_lost_resumes(cx);
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

    /// At launch: a sub-agent paused at its limit when Trek quit has lost its task with the run
    /// that started it. Its parent heard it failed, and nothing would carry an answer back, so it
    /// isn't resumed on its own: at the reset its pause just lifts. What the user queued in its
    /// thread still goes, and "Resume at reset" there still works: that's them carrying on in it.
    fn drop_lost_resumes(&mut self, cx: &mut Context<Self>) {
        let lost: Vec<String> = self
            .threads
            .iter()
            .filter(|t| t.parent_id.is_some() && !self.delegations.contains_key(&t.id))
            .filter(|t| t.paused.as_ref().is_some_and(|p| p.resume && p.queued.is_empty()))
            .map(|t| t.id.clone())
            .collect();
        for id in lost {
            self.mutate_thread(&id, cx, |t| {
                if let Some(p) = t.paused.as_mut() {
                    p.resume = false;
                }
            });
        }
    }

    /// The thread moved from one agent to another mid-conversation: a divider says so (the new
    /// agent starts from a recap). Switching back and forth before sending leaves one divider,
    /// or none once it's back where it started.
    pub(super) fn note_handoff(&mut self, id: &str, from: (&AgentId, Option<String>), to: (&AgentId, Option<String>)) -> bool {
        // Each side as it is now: the model it ran (the default, if none was picked) and its name.
        let side = |agent: &AgentId, model: Option<String>| {
            let models = self.models_for(agent);
            let model = model.or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone()));
            let name = crate::thread_view::handoff_name(agent, model.as_deref().map(|m| crate::composer::model_name(&models, m)).as_deref());
            (agent.key(), model, name)
        };
        let (mut from, to) = (side(from.0, from.1), side(to.0, to.1));
        let Some(live) = self.live.get_mut(id) else { return false };
        if !live.items.iter().any(|i| matches!(i, Item::User { aside: false, .. })) {
            return false;
        }
        if let Some(Item::Handoff { from: key, from_model, from_name, .. }) = live.items.last() {
            from = (key.clone(), from_model.clone(), from_name.clone().unwrap_or_default());
            let last = live.items.len() - 1;
            live.items.truncate(last);
        }
        if from.0 != to.0 {
            live.items.push(Item::Handoff { from: from.0, from_model: from.1, to: to.0, to_model: to.1, from_name: Some(from.2), to_name: Some(to.2) });
        }
        live.revision += 1;
        true
    }
}
