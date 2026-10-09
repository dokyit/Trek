//! The Usage card's data: what each agent says of its plan and limits, read off the main thread,
//! each agent on its own (one slow CLI doesn't hold the others back), again shortly after a turn
//! on it ends, and kept across launches so the card has numbers from the first frame (marked
//! with when they were read until they're read again). Also the tokens Trek recorded for each
//! agent today, for the agents with no plan limits to show, and which agents the card shows
//! (`Settings::usage`, up to three).

use super::{Workspace, devin_agent};
use gpui_kit::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use trek_agents::{AgentStatus, ResetCredit, UsageLimit};
use trek_core::AgentId;
use trek_core::basecamp::Range;
use trek_core::pricing::Spend;
use trek_core::settings::USAGE_SHOWN_MAX;

/// Reads an agent's account, plan, limits and commands, answering on the channel. Claude Code's
/// and Codex's own report (`claude_status`, `codex_status`) by default; tests answer for any agent.
pub type UsageFetch = Rc<dyn Fn(&AgentId, &Path) -> async_channel::Receiver<Result<AgentStatus, String>>>;

/// The shortest time between two reads of one agent's usage, unless one is asked for (`/usage`,
/// a limit hit with no reset known).
const READ_EVERY_MS: i64 = 30_000;

/// Claude Code's or Codex's own usage report, read on Trek's tokio runtime.
fn read_agent(agent: &AgentId, cwd: &Path) -> async_channel::Receiver<Result<AgentStatus, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let (agent, cwd) = (agent.clone(), cwd.to_path_buf());
    trek_core::runtime().spawn(async move {
        let status = match agent {
            AgentId::ClaudeCode => trek_agents::claude_status(&cwd).await,
            _ => trek_agents::codex_status(&cwd).await,
        };
        let _ = tx.send(status.map_err(|e| e.to_string())).await;
    });
    rx
}

/// What an agent last said of its plan and limits, and when: kept in `usage.json` in the data
/// folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub read_at: i64,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub limits: Vec<UsageLimit>,
    #[serde(default)]
    pub note: Option<String>,
}

impl Snapshot {
    pub(super) fn of(status: &AgentStatus, read_at: i64) -> Snapshot {
        Snapshot { read_at, plan: status.plan.clone(), limits: status.limits.clone(), note: status.note.clone() }
    }
}

fn snapshots_file() -> PathBuf {
    trek_core::paths::data_dir().join("usage.json")
}

/// The snapshots kept last time (none when there's no file, or it can't be read).
pub(crate) fn load_snapshots() -> HashMap<String, Snapshot> {
    std::fs::read(snapshots_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// One provider's row on the Usage card, as it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub agent: AgentId,
    pub plan: Option<String>,
    pub limits: Vec<UsageLimit>,
    /// It said it has no plan limits (signed in, nothing to show).
    pub no_limits: bool,
    pub error: Option<String>,
    pub note: Option<String>,
    pub resets: Vec<ResetCredit>,
    /// Shown from what was read at this time (unix ms), in an earlier run: not read again yet.
    pub as_of: Option<i64>,
    /// Tokens Trek recorded for it today, and what they cost.
    pub today: Option<(u64, Spend)>,
}

impl Workspace {
    /// Every provider the Usage card can show, in the agent picker's order: the agents ready to
    /// use, then any others something is known of (kept from an earlier run, or used today),
    /// Claude Code and Codex first. Agents turned off in Settings aren't offered.
    pub fn usage_providers(&self) -> Vec<AgentId> {
        let mut out = self.ready_agents();
        let mut known: Vec<&String> = self.agent_status.keys().chain(self.usage_cached.keys()).chain(self.usage_today.keys()).collect();
        let rank = |k: &str| match k {
            "claude-code" => 0,
            "codex" => 1,
            _ => 2,
        };
        known.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
        for key in known {
            let agent = AgentId::from_key(key);
            if !out.contains(&agent) {
                out.push(agent);
            }
        }
        out.retain(|a| !self.settings.disabled_agents.contains(&a.key()));
        out
    }

    /// Whether there's anything to show of `agent`'s usage.
    fn has_usage(&self, agent: &AgentId) -> bool {
        let key = agent.key();
        self.agent_status.contains_key(&key) || self.usage_cached.contains_key(&key) || self.usage_today.get(&key).is_some_and(|(n, _)| *n > 0)
    }

    /// The providers the Usage card shows: the ones picked (`Settings::usage`), else the first
    /// few with usage to show. Never more than `USAGE_SHOWN_MAX`.
    pub fn usage_shown(&self) -> Vec<AgentId> {
        let all = self.usage_providers();
        match &self.settings.usage.shown {
            Some(keys) => all.into_iter().filter(|a| keys.contains(&a.key())).take(USAGE_SHOWN_MAX).collect(),
            None => all.into_iter().filter(|a| self.has_usage(a)).take(USAGE_SHOWN_MAX).collect(),
        }
    }

    /// The user picked the providers rather than leaving it to Trek.
    pub fn usage_picked(&self) -> bool {
        self.settings.usage.shown.is_some()
    }

    /// Show `agent` on the Usage card, or stop showing it. Showing a fourth is refused (false):
    /// one goes first. What's on the card now becomes the pick.
    pub fn toggle_usage_shown(&mut self, agent: &AgentId, cx: &mut Context<Self>) -> bool {
        let mut keys: Vec<String> = self.usage_shown().iter().map(AgentId::key).collect();
        let key = agent.key();
        match keys.iter().position(|k| *k == key) {
            Some(i) => {
                keys.remove(i);
            }
            None if keys.len() >= USAGE_SHOWN_MAX => return false,
            None => keys.push(key),
        }
        self.settings.usage.shown = Some(keys);
        self.save_settings(cx);
        // Devin's plan is read only while something shows it.
        if *agent == devin_agent() && self.usage_shown().contains(agent) {
            self.refresh_devin_usage(cx);
        }
        true
    }

    /// Leave it to Trek again: the first few providers with usage to show.
    pub fn show_usage_automatically(&mut self, cx: &mut Context<Self>) {
        self.settings.usage.shown = None;
        self.save_settings(cx);
    }

    /// The Usage card's rows, for the providers it shows. What an agent said this run is shown
    /// as it is; what it said in an earlier run is shown with when (windows that have reset
    /// since are shown empty again) until it's read again.
    pub fn usage_rows(&self) -> Vec<UsageRow> {
        let now = self.now();
        self.usage_shown()
            .into_iter()
            .map(|agent| {
                let key = agent.key();
                let today = self.usage_today.get(&key).filter(|(n, _)| *n > 0).cloned();
                if let Some(st) = self.agent_status.get(&key) {
                    return UsageRow {
                        no_limits: st.limits.is_empty() && st.error.is_none(),
                        plan: st.plan.clone(),
                        limits: st.limits.clone(),
                        error: st.error.clone(),
                        note: st.note.clone(),
                        resets: st.resets.clone(),
                        as_of: None,
                        today,
                        agent,
                    };
                }
                match self.usage_cached.get(&key) {
                    Some(s) => {
                        let limits = s
                            .limits
                            .iter()
                            .map(|l| match l.resets_at {
                                Some(at) if at <= now => UsageLimit { percent: 0., resets_at: None, ..l.clone() },
                                _ => l.clone(),
                            })
                            .collect();
                        UsageRow { no_limits: s.limits.is_empty(), plan: s.plan.clone(), limits, error: None, note: s.note.clone(), resets: vec![], as_of: Some(s.read_at), today, agent }
                    }
                    None => UsageRow { agent, plan: None, limits: vec![], no_limits: false, error: None, note: None, resets: vec![], as_of: None, today },
                }
            })
            .collect()
    }

    /// Whether `agent`'s usage can be read on its own (`fetch_usage`): Claude Code's and Codex's,
    /// or any agent's a test answers for. Devin's is read apart (`refresh_devin_usage`).
    fn usage_readable(&self, agent: &AgentId) -> bool {
        self.usage_fetch.is_some() || matches!(agent, AgentId::ClaudeCode | AgentId::Codex)
    }

    /// Re-read account, plan, usage limits and commands from the installed vendor CLIs, each on
    /// its own and at once, off the main thread. Free: no prompt is sent. Each agent at most
    /// every 30 seconds. Devin is asked on its own (`refresh_devin_usage`).
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        self.read_usage(false, cx);
    }

    /// `refresh_usage` now, however recently the agents were read (`/usage`, a reset used, a
    /// limit with no reset known).
    pub fn refresh_usage_now(&mut self, cx: &mut Context<Self>) {
        self.read_usage(true, cx);
    }

    fn read_usage(&mut self, force: bool, cx: &mut Context<Self>) {
        let mut agents = vec![AgentId::ClaudeCode, AgentId::Codex];
        if self.usage_fetch.is_some() {
            let more: Vec<AgentId> = self.usage_providers().into_iter().filter(|a| !agents.contains(a) && *a != devin_agent()).collect();
            agents.extend(more);
        }
        for agent in agents {
            self.fetch_usage(agent, force, cx);
        }
    }

    /// Read `agent`'s usage, unless it was read in the last 30 seconds (or is being read) and
    /// `force` isn't set. Its answer lands when it comes, whatever the others are doing.
    pub(super) fn fetch_usage(&mut self, agent: AgentId, force: bool, cx: &mut Context<Self>) {
        let key = agent.key();
        let fetch: UsageFetch = match &self.usage_fetch {
            Some(f) => f.clone(),
            None if trek_core::paths::isolated() => return,
            None => Rc::new(read_agent),
        };
        if !self.usage_readable(&agent) || (!force && (self.usage_inflight.contains(&key) || self.now() - self.usage_fetched.get(&key).copied().unwrap_or(0) < READ_EVERY_MS)) {
            return;
        }
        // Not of a CLI being replaced: asked again once it's back (`agent_released`). Before any
        // agent is found (at launch), the ones that answered last time are asked straight away.
        let ready = match &self.usage_fetch {
            Some(_) => self.ready_agents().contains(&agent),
            None => self.agent_ready(&agent) || (self.agents.is_empty() && self.usage_cached.contains_key(&key) && !self.settings.disabled_agents.contains(&key)),
        };
        if !ready || self.agent_updating(&key) {
            return;
        }
        self.usage_inflight.insert(key.clone());
        self.usage_loading = true;
        let folder = self.current_cwd().unwrap_or_else(trek_core::paths::home);
        let rx = fetch(&agent, &folder);
        let task = cx.spawn(async move |this, cx| {
            let res = rx.recv().await.unwrap_or_else(|_| Err("the read was dropped".into()));
            let _ = this.update(cx, |this, cx| this.usage_read(agent, folder, res, cx));
        });
        self.keep(task);
        cx.notify();
    }

    /// `agent` said what its usage is (or failed to): the card, the limits Trek keeps to and the
    /// commands follow, and what it said is kept for the next launch.
    fn usage_read(&mut self, agent: AgentId, folder: PathBuf, res: Result<AgentStatus, String>, cx: &mut Context<Self>) {
        let key = agent.key();
        let now = self.now();
        match res {
            Ok(st) => {
                if agent == AgentId::Codex && !st.models.is_empty() {
                    self.codex_models = st.models.clone();
                }
                // Its commands include the folder's own (project commands, skills).
                self.agent_commands.insert((key.clone(), folder), st.commands.clone());
                self.keep_snapshot(&key, Snapshot::of(&st, now), cx);
                self.agent_status.insert(key.clone(), st);
            }
            Err(e) => {
                let st = self.agent_status.entry(key.clone()).or_default();
                st.error = Some(e);
            }
        }
        self.usage_inflight.remove(&key);
        self.usage_fetched.insert(key, now);
        if self.usage_inflight.is_empty() {
            self.usage_loading = false;
            self.status_fetched_at = now;
        }
        self.fill_unknown_resets(cx);
        self.wrap_up_where_due(&agent, cx);
        cx.notify();
    }

    /// Keep what `key`'s agent said of its plan for the next launch (written off the main thread).
    pub(super) fn keep_snapshot(&mut self, key: &str, snapshot: Snapshot, cx: &mut Context<Self>) {
        if snapshot.plan.is_none() && snapshot.limits.is_empty() && snapshot.note.is_none() {
            return;
        }
        self.usage_cached.insert(key.to_string(), snapshot);
        let (path, data) = (snapshots_file(), serde_json::to_vec(&self.usage_cached).unwrap_or_default());
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = std::fs::write(&path, data) {
                    tracing::warn!("keep usage: {e}");
                }
            })
            .detach();
    }

    /// A turn on `agent` ended: what it used shows on the card shortly. Its plan's usage is read
    /// again (at most every 30 seconds; Devin's, only while the card shows it, at most every ten
    /// minutes), and today's tokens are summed again.
    pub(super) fn usage_after_turn(&mut self, agent: &AgentId, cx: &mut Context<Self>) {
        if self.usage_readable(agent) && *agent != devin_agent() {
            self.fetch_usage(agent.clone(), false, cx);
        } else if *agent == devin_agent() && self.usage_shown().contains(agent) {
            self.refresh_devin_usage(cx);
        }
        self.refresh_usage_today(cx);
    }

    /// Sum the tokens Trek recorded for each agent today, off the main thread. One sum at a
    /// time; asked again while one runs, it runs once more after.
    pub fn refresh_usage_today(&mut self, cx: &mut Context<Self>) {
        if self.usage_today_reading {
            self.usage_today_again = true;
            return;
        }
        self.usage_today_reading = true;
        let store = self.store.clone();
        let since = Range::Today.window(&chrono::Local::now()).start;
        let work = cx.background_executor().spawn(async move {
            let rows = store.usage_since_by_agent(since).unwrap_or_else(|e| {
                tracing::warn!("usage today: {e:#}");
                vec![]
            });
            let mut by: HashMap<String, (u64, Spend)> = HashMap::new();
            for r in &rows {
                let (n, spend) = by.entry(r.agent.key()).or_default();
                *n += r.tokens.total();
                spend.add(&r.agent, r.model.as_deref(), &r.tokens, r.cost, r.at);
            }
            by
        });
        let task = cx.spawn(async move |this, cx| {
            let by = work.await;
            let _ = this.update(cx, |this, cx| {
                this.usage_today = by;
                this.usage_today_reading = false;
                if std::mem::take(&mut this.usage_today_again) {
                    this.refresh_usage_today(cx);
                }
                cx.notify();
            });
        });
        self.keep(task);
    }
}
