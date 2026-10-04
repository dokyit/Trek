//! Checking for and installing agent CLI updates (`crate::agent_updates` holds the state). A
//! check runs at launch and every 12 hours on Trek's I/O runtime, never on the UI's; updates run
//! one at a time, each once none of its agent's threads is mid-turn. While one runs, its agent
//! is left alone: its idle sessions are shut down as it starts, and no session starts and no
//! message goes to it (they wait, as for a worktree being made) until it's done.

use super::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use trek_agents::Command;
use trek_core::AgentId;
use trek_core::agent_update::{Outcome, Snapshot};
use trek_core::store::now_ms;

/// Why an agent's update hasn't started yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    Turn,
    Background,
}

impl Workspace {
    /// Look for new versions of the agent CLIs. A background check (`user: false`) runs only
    /// with the setting on. Not in a test process: that would run the user's own CLIs.
    pub fn check_agent_updates(&mut self, user: bool, cx: &mut Context<Self>) {
        let u = &mut self.agent_updates;
        if u.checking || u.mock || (!user && !self.settings.updates.check_agents) || trek_core::paths::isolated() {
            return;
        }
        u.checking = true;
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::agent_update::check_all().await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(fresh) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                this.agent_updates.checked(fresh, now_ms());
                this.save_agent_versions(cx);
                if this.agent_updates.start_at_launch() {
                    this.update_all_agents(cx);
                }
                // Updates asked for before it came in go now, on what it found.
                this.pump_agent_updates(cx);
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Cache what's known for the next launch: the last check, with updates installed since.
    /// Never the made-up agents.
    fn save_agent_versions(&self, cx: &mut Context<Self>) {
        let u = &self.agent_updates;
        if u.mock {
            return;
        }
        let snapshot = Snapshot { checked_at: u.checked_at, agents: u.found.clone() };
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = snapshot.save() {
                    tracing::warn!("save agent versions: {e:#}");
                }
            })
            .detach();
    }

    /// From housekeeping: check when the last check is 12 hours old, and start updates whose
    /// agents' turns have ended.
    pub(super) fn maybe_check_agent_updates(&mut self, cx: &mut Context<Self>) {
        if self.agent_updates.due(self.settings.updates.check_agents, now_ms()) {
            self.check_agent_updates(false, cx);
        }
        self.pump_agent_updates(cx);
    }

    /// A turn is under way in one of `agent`'s threads (sub-agents and side chats included), or
    /// one of its sessions has work running in the background (a dev server, a browser, its own
    /// sub-agents): replacing the CLI would end its session and that work with it.
    pub fn agent_busy(&self, agent: &str) -> bool {
        self.agent_hold(agent).is_some()
    }

    /// What `agent`'s update waits for, if anything: a turn, or background work.
    pub fn agent_hold(&self, agent: &str) -> Option<Hold> {
        let lives = || self.threads.iter().filter(|t| t.agent.key() == agent).filter_map(|t| self.live.get(&t.id));
        if lives().any(|l| l.turn_started.is_some()) {
            Some(Hold::Turn)
        } else if lives().any(|l| l.commands.is_some() && !l.background.is_empty()) {
            Some(Hold::Background)
        } else {
            None
        }
    }

    /// `agent`'s CLI is being replaced: its sessions and messages wait.
    pub fn agent_updating(&self, agent: &str) -> bool {
        self.agent_updates.updating(agent)
    }

    /// Update row `id` (`AgentVersion::id`): now, or once its agent's running turns end.
    pub fn update_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.agent_updates.request(id) {
            self.pump_agent_updates(cx);
            cx.notify();
        }
    }

    /// Update every agent with a new version out, one after another.
    pub fn update_all_agents(&mut self, cx: &mut Context<Self>) {
        for id in self.agent_updates.updatable() {
            self.agent_updates.request(&id);
        }
        self.pump_agent_updates(cx);
        cx.notify();
    }

    /// Start the next update that can start. Called as updates are asked for, as one finishes,
    /// as turns end, and from housekeeping.
    pub(super) fn pump_agent_updates(&mut self, cx: &mut Context<Self>) {
        // A check may have found the agent gone since its update was asked for.
        self.agent_updates.drop_orphans();
        if !self.agent_updates.queued() {
            return;
        }
        // The rows are an earlier launch's: check first (whatever the setting, the update was
        // asked for), and start on what that finds.
        if self.agent_updates.stale {
            self.check_agent_updates(true, cx);
            return;
        }
        let Some(id) = self.agent_updates.next(|a| self.agent_busy(a)) else { return };
        let Some(version) = self.agent_updates.start(&id) else { return };
        // None of its turns is running: every session it has is idle, and goes now, before its
        // files are replaced under it.
        self.stop_sessions_of(&version.agent);
        cx.notify();
        let runner = self.agent_updates.runner.clone();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(runner.run(version).await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| this.agent_updated(&id, outcome, cx));
        });
        self.keep(task);
    }

    fn agent_updated(&mut self, id: &str, outcome: Outcome, cx: &mut Context<Self>) {
        // Its row is kept through checks while it runs; were it gone anyway, the job still ends,
        // or it would hold up every update after it (and Trek's own).
        let row = self.agent_updates.found.iter().find(|v| v.id == id).map(|v| (v.agent.clone(), v.name.clone()));
        let (agent, name) = row.clone().unwrap_or_else(|| {
            let h = trek_core::agent_update::harness(id);
            (h.map(|h| h.agent.to_string()).unwrap_or_default(), h.map(|h| h.name()).unwrap_or_else(|| id.to_string()))
        });
        let dry = self.agent_updates.pretend();
        let message = match &outcome {
            Outcome::Updated { version, .. } if dry => format!("Dry run: {name} would be at {version} now. Nothing was changed."),
            Outcome::Updated { version, .. } => {
                // Settings and pickers show the version the agent's CLI now reports (an adapter's
                // isn't the agent's).
                let adapter = trek_core::agent_update::harness(id).is_some_and(|h| h.adapter);
                if let Some(a) = self.agents.iter_mut().find(|a| a.agent.key() == agent).filter(|_| !adapter) {
                    a.version = Some(version.clone());
                }
                format!("{name} updated to {version}.")
            }
            Outcome::Failed { summary, .. } => format!("Couldn't update {name}: {summary}"),
        };
        let installed = matches!(outcome, Outcome::Updated { .. });
        self.agent_updates.finish(id, outcome);
        // Its new version is cached too: a later launch doesn't offer it again, with checks off
        // (none runs to say so) or before its check is in.
        if installed && !dry {
            self.save_agent_versions(cx);
        }
        cx.emit(WorkspaceEvent::Toast { message, undo: None });
        if !self.agent_updating(&agent) {
            self.agent_released(&agent, cx);
        }
        self.pump_agent_updates(cx);
        // A Trek update held back for this one may go now.
        self.maybe_restart_for_update(cx);
        cx.notify();
    }

    /// `agent`'s CLI is no longer being replaced: what waited for it goes now, to a session of
    /// the new version. Only messages held for the update: follow-ups a failed turn left queued
    /// are the user's to hand back or send.
    fn agent_released(&mut self, agent: &str, cx: &mut Context<Self>) {
        let held: Vec<String> = self.threads.iter().filter(|t| t.agent.key() == agent && self.agent_updates.held.contains(&t.id)).map(|t| t.id.clone()).collect();
        for t in held {
            self.agent_updates.held.remove(&t);
            self.send_queued(&t, cx);
        }
        // Parents whose sub-agents reported meanwhile hear from them (after those messages'
        // turns, if any).
        let woken: Vec<String> = self.threads.iter().filter(|t| t.agent.key() == agent && self.wakes.contains_key(&t.id)).map(|t| t.id.clone()).collect();
        for t in woken {
            self.deliver_wakes(&t, cx);
        }
        // Claude Code writes the titles; Claude and Codex report usage.
        if agent == AgentId::ClaudeCode.key() {
            for (id, announce) in std::mem::take(&mut self.agent_updates.titles) {
                self.regenerate_title(&id, announce, cx);
            }
        }
        if agent == AgentId::ClaudeCode.key() || agent == AgentId::Codex.key() {
            self.refresh_usage(cx);
        }
    }

    /// Before `agent` is updated, its sessions go (the next message starts the new version,
    /// resuming the conversation): idle ones now, any still busy once their turn ends, and a
    /// pre-warmed draft session too.
    fn stop_sessions_of(&mut self, agent: &str) {
        for t in self.threads.iter().filter(|t| t.agent.key() == agent) {
            let Some(live) = self.live.get_mut(&t.id) else { continue };
            if !live.free_to_relaunch() {
                live.relaunch = true;
            } else if let Some(tx) = live.commands.take() {
                let _ = tx.try_send(Command::Shutdown);
            }
        }
        if self.warm.as_ref().is_some_and(|(key, _, _)| key.0.key() == agent) {
            self.warm = None;
            if let (Some(key), Some(ipc)) = (self.warm_ipc.take(), &self.ipc) {
                ipc.close_session(&key);
            }
        }
    }
}
