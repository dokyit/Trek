//! An agent's own sign-in, run in a terminal Trek opens for it (Settings › Agents › Sign in,
//! "Switch account…", onboarding).
//!
//! Some logins end when they're done (`claude auth login`); others are the agent's whole TUI
//! (`droid`, `qwen`, `pi`), which stays open after it has signed in and never exits. So Trek
//! doesn't wait for the terminal: while one is open for an agent that wasn't signed in, it asks
//! that agent every few seconds, and the moment it is, Settings says so and tells the user the
//! terminal can be closed (in a toast, on the terminal's tab and in the dock, and on the agent's
//! row). When the terminal does end, what Trek knows of that agent is read again at once,
//! whatever was read a moment ago: it isn't left to the next launch.

use super::{Workspace, WorkspaceEvent};
use std::time::{Duration, Instant};
use trek_core::AgentId;
use trek_core::detect::Availability;

/// How often an agent whose login terminal is open is asked whether it's signed in.
pub const RECHECK: Duration = Duration::from_secs(4);
/// How long it's asked at most: a login that takes longer than this is the user's to finish.
const WATCH_FOR: Duration = Duration::from_secs(20 * 60);
/// How long after the terminal ended it's still asked (its last read is on its way).
const AFTER_EXIT: Duration = Duration::from_secs(30);
/// How long Settings keeps saying "Signed in".
pub const SAID_FOR: Duration = Duration::from_secs(10 * 60);

/// A login terminal that's open (or just ended) for an agent that wasn't signed in.
#[derive(Debug, Clone)]
pub struct LoginWatch {
    /// The terminal's command (its sign-in line).
    pub command: String,
    since: Instant,
    /// When the terminal ended, if it has.
    ended: Option<Instant>,
}

/// An agent that's signed in now, after Trek opened its login terminal.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub command: String,
    pub at: Instant,
    /// Whether the terminal had ended by then (nothing to close).
    pub ended: bool,
}

impl SignedIn {
    /// Settings' and the terminal's words, plain: it worked, and what's left to do.
    pub fn words(&self, name: &str) -> String {
        if self.ended {
            format!("Signed in to {name}.")
        } else {
            format!("Signed in to {name}. You can close the terminal.")
        }
    }
}

impl Workspace {
    /// Whether the agent keyed `key` is signed in as far as Trek knows (the same reading as
    /// Settings › Agents' account line): `None` before it's been read.
    pub fn agent_signed_in(&self, key: &str) -> Option<bool> {
        let status = self.agent_status.get(key);
        match (status, self.acp_info.get(key)) {
            (Some(st), _) => Some(st.logged_in || st.account.is_some()),
            (None, Some(Ok(info))) => Some(!info.needs_auth),
            (None, Some(Err(_))) => Some(false),
            (None, None) => self
                .agents
                .iter()
                .find(|a| a.agent.key() == key)
                .filter(|a| a.availability == Availability::NeedsLogin)
                .map(|_| false),
        }
    }

    /// The key of the agent whose sign-in `command` is, among the ones Trek found.
    fn login_agent(&self, command: &str) -> Option<String> {
        let keys: Vec<String> = trek_core::catalog::login_keys(command)
            .into_iter()
            .map(|k| AgentId::from_key(k).key())
            .collect();
        keys.iter()
            .find(|k| self.agents.iter().any(|a| a.agent.key() == **k))
            .or(keys.first())
            .cloned()
    }

    /// A Trek command is about to run in a terminal. When it's the sign-in of an agent that isn't
    /// signed in, the agent is asked every few seconds until it is (its terminal may never end).
    pub fn setup_started(&mut self, command: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(key) = self.login_agent(command) else {
            return;
        };
        self.signed_in.remove(&key);
        // Already signed in (this is "Switch account…") or not read yet: nothing to wait for.
        if self.agent_signed_in(&key) != Some(false) {
            cx.notify();
            return;
        }
        let running = self.login_watch.contains_key(&key);
        self.login_watch.insert(
            key.clone(),
            LoginWatch {
                command: command.trim().to_string(),
                since: Instant::now(),
                ended: None,
            },
        );
        cx.notify();
        if running {
            return;
        }
        let every = self.login_recheck;
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(every).await;
                let Ok(again) = this.update(cx, |this, cx| this.check_login(&key, cx)) else {
                    return;
                };
                if !again {
                    return;
                }
            }
        });
        self.keep(task);
    }

    /// One tick of a login's watch: signed in now (said so, and done), or asked again. `false`
    /// once there's nothing left to watch.
    fn check_login(&mut self, key: &str, cx: &mut gpui_kit::Context<Self>) -> bool {
        let Some(watch) = self.login_watch.get(key).cloned() else {
            return false;
        };
        if self.agent_signed_in(key) == Some(true) {
            self.login_watch.remove(key);
            let done = SignedIn {
                command: watch.command,
                at: Instant::now(),
                ended: watch.ended.is_some(),
            };
            let said = done.words(&AgentId::from_key(key).display_name());
            self.signed_in.insert(key.to_string(), done);
            cx.emit(WorkspaceEvent::Toast {
                message: said,
                undo: None,
            });
            cx.notify();
            return false;
        }
        let over = watch.since.elapsed() > WATCH_FOR
            || watch.ended.is_some_and(|at| at.elapsed() > AFTER_EXIT);
        if over {
            self.login_watch.remove(key);
            return false;
        }
        self.recheck_sign_in(key, cx);
        true
    }

    /// Ask the agent keyed `key` again whether it's signed in, whatever was read a moment ago.
    fn recheck_sign_in(&mut self, key: &str, cx: &mut gpui_kit::Context<Self>) {
        let agent = AgentId::from_key(key);
        match agent {
            // Its sign-in is part of finding it (`detect`).
            AgentId::Codex => {
                self.commands_read.remove(key);
                self.usage_fetched.remove(key);
                self.detect_agents(cx);
            }
            AgentId::ClaudeCode => self.recheck_status(agent, cx),
            // OpenCode, Droid and the ACP agents say it when asked to start a session.
            _ => self.probe_acp(Some(key), cx),
        }
    }

    /// A Trek command finished in a terminal: when it was an agent's sign-in, that agent is read
    /// again at once, and a login that took is said as soon as it shows.
    pub(super) fn login_finished(&mut self, command: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(key) = self.login_agent(command) else {
            return;
        };
        if let Some(watch) = self.login_watch.get_mut(&key) {
            watch.ended = Some(Instant::now());
        }
        self.recheck_sign_in(&key, cx);
        cx.notify();
    }

    /// What Settings says under an agent's row, and the dock beside its terminal, once Trek has
    /// seen it sign in: for a few minutes.
    pub fn signed_in_now(&self, key: &str) -> Option<&SignedIn> {
        self.signed_in.get(key).filter(|s| s.at.elapsed() < SAID_FOR)
    }

    /// What to say beside the terminal running `command`, if it signed an agent in.
    pub fn login_note(&self, command: &str) -> Option<String> {
        let command = command.trim();
        let (key, done) = self
            .signed_in
            .iter()
            .find(|(_, s)| s.command == command && s.at.elapsed() < SAID_FOR)?;
        Some(done.words(&AgentId::from_key(key).display_name()))
    }
}
