//! Checking for and installing agent CLI updates (`crate::agent_updates` holds the state). A
//! check runs at launch and every 12 hours on Trek's I/O runtime, never on the UI's; updates run
//! one at a time, each once none of its agent's threads is mid-turn.

use super::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use trek_agents::Command;
use trek_core::agent_update::{Outcome, Snapshot};
use trek_core::store::now_ms;

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
                if this.agent_updates.start_at_launch() {
                    this.update_all_agents(cx);
                }
                let snapshot = Snapshot { checked_at: this.agent_updates.checked_at, agents: this.agent_updates.found.clone() };
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = snapshot.save() {
                            tracing::warn!("save agent versions: {e:#}");
                        }
                    })
                    .detach();
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// From housekeeping: check when the last check is 12 hours old, and start updates whose
    /// agents' turns have ended.
    pub(super) fn maybe_check_agent_updates(&mut self, cx: &mut Context<Self>) {
        if self.agent_updates.due(self.settings.updates.check_agents, now_ms()) {
            self.check_agent_updates(false, cx);
        }
        self.pump_agent_updates(cx);
    }

    /// A turn is under way in one of `agent`'s threads (sub-agents and side chats included).
    pub fn agent_busy(&self, agent: &str) -> bool {
        self.threads.iter().any(|t| t.agent.key() == agent && self.turn_running(&t.id))
    }

    /// Update `agent`: now, or once its running turns end.
    pub fn update_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.agent_updates.request(agent) {
            self.pump_agent_updates(cx);
            cx.notify();
        }
    }

    /// Update every agent with a new version out, one after another.
    pub fn update_all_agents(&mut self, cx: &mut Context<Self>) {
        for agent in self.agent_updates.updatable() {
            self.agent_updates.request(&agent);
        }
        self.pump_agent_updates(cx);
        cx.notify();
    }

    /// Start the next update that can start. Called as updates are asked for, as one finishes,
    /// as turns end, and from housekeeping.
    pub(super) fn pump_agent_updates(&mut self, cx: &mut Context<Self>) {
        if !self.agent_updates.queued() {
            return;
        }
        let Some(agent) = self.agent_updates.next(|a| self.agent_busy(a)) else { return };
        let Some(version) = self.agent_updates.start(&agent) else { return };
        cx.notify();
        let runner = self.agent_updates.runner.clone();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(runner.run(version).await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| this.agent_updated(&agent, outcome, cx));
        });
        self.keep(task);
    }

    fn agent_updated(&mut self, agent: &str, outcome: Outcome, cx: &mut Context<Self>) {
        let name = trek_core::AgentId::from_key(agent).display_name();
        let dry = matches!(self.agent_updates.runner, crate::agent_updates::Runner::DryRun);
        let message = match &outcome {
            Outcome::Updated { version, .. } if dry => format!("Dry run: {name} would be at {version} now. Nothing was changed."),
            Outcome::Updated { version, .. } => {
                // Settings and pickers show the version the agent now reports.
                if let Some(a) = self.agents.iter_mut().find(|a| a.agent.key() == agent) {
                    a.version = Some(version.clone());
                }
                self.restart_sessions_of(agent);
                format!("{name} updated to {version}.")
            }
            Outcome::Failed { summary, .. } => format!("Couldn't update {name}: {summary}"),
        };
        self.agent_updates.finish(agent, outcome);
        cx.emit(WorkspaceEvent::Toast { message, undo: None });
        self.pump_agent_updates(cx);
        cx.notify();
    }

    /// After `agent` was updated, its sessions still run the old CLI: idle ones shut down (the
    /// next message starts the new one, resuming the conversation), busy ones once their turn
    /// ends, and a pre-warmed draft session goes.
    fn restart_sessions_of(&mut self, agent: &str) {
        for t in self.threads.iter().filter(|t| t.agent.key() == agent) {
            let Some(live) = self.live.get_mut(&t.id) else { continue };
            if live.turn_started.is_some() || live.background > 0 {
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
