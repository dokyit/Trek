//! Agents the user adds (Settings › Agents › Add agent): the ACP Registry's index, installing
//! from it, a command of the user's own, and removing either. What's added lives in settings
//! (`added_agents`) and in `catalog`, where launching, detection and names find it.

use super::{Workspace, WorkspaceEvent};
use std::collections::HashMap;
use trek_core::AgentId;
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::registry::{self, AddedAgent, Progress, Registry};

/// The registry as Trek last read it, and the installs under way.
#[derive(Default)]
pub struct AddedAgents {
    /// The index: the cached one at first, then a fresh one.
    pub registry: Option<Registry>,
    pub loading: bool,
    /// Why the last fetch failed (the cached index, if any, is still shown).
    pub error: Option<String>,
    /// The cache on disk has been read.
    cache_read: bool,
    /// Registry agents being installed or updated, by registry id, and the last failure of each.
    pub installs: HashMap<String, Install>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Install {
    Running(Option<Progress>),
    Failed(String),
}

impl Install {
    /// What a row says while it runs: "Downloading 42%", "Unpacking…".
    pub fn label(&self) -> String {
        match self {
            Install::Running(None) => "Starting…".into(),
            Install::Running(Some(Progress::Downloading { done, total: Some(total) })) if *total > 0 => format!("Downloading {}%", (done * 100 / total).min(100)),
            Install::Running(Some(Progress::Downloading { done, .. })) => format!("Downloading {:.1} MB", *done as f64 / 1e6),
            Install::Running(Some(Progress::Unpacking)) => "Unpacking…".into(),
            Install::Running(Some(Progress::Fetching)) => "Fetching with npx…".into(),
            Install::Failed(e) => e.clone(),
        }
    }
}

enum Loaded {
    Cached(Option<Registry>),
    Fetched(Result<Registry, String>),
}

enum Step {
    Progress(Progress),
    Done(Result<AddedAgent, String>),
}

impl Workspace {
    /// Tell the rest of Trek about the added agents in settings, then find which of them can
    /// start (looking for their programs only: nothing is run, so a test process may too).
    pub fn sync_added_agents(&mut self, cx: &mut gpui_kit::Context<Self>) {
        trek_core::catalog::set_added_agents(&self.settings.added_agents);
        let list = self.settings.added_agents.clone();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(list.iter().map(trek_core::detect::added_agent).collect::<Vec<DetectedAgent>>()).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(found) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                this.merge_added(found);
                // Ask the ones that can start what they offer (not from a test process).
                let ready: Vec<String> = this.agents.iter().filter(|a| a.availability == Availability::Ready && is_added(&a.agent)).map(|a| a.agent.key()).collect();
                for key in ready {
                    this.probe_acp_agent(&key, cx);
                }
                // A draft on an agent that was just removed moves to one that's here.
                let ready = this.ready_agents();
                for p in [&mut this.draft_prefs, &mut this.ide_chat.draft_prefs] {
                    if is_added(&p.agent) && !ready.contains(&p.agent) {
                        if let Some(a) = ready.first() {
                            p.agent = a.clone();
                            p.model = None;
                        }
                    }
                }
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Put what was found of the added agents in the agent list: after the built-in CLI agents,
    /// before the local model servers. Entries of agents no longer added go.
    fn merge_added(&mut self, found: Vec<DetectedAgent>) {
        self.agents.retain(|a| !matches!(&a.agent, AgentId::Acp(id) if !builtin(id)));
        let at = self.agents.iter().position(|a| matches!(a.agent, AgentId::Direct(_))).unwrap_or(self.agents.len());
        self.agents.splice(at..at, found);
    }

    /// Read the cached index (once), and fetch a fresh one when it's older than a few hours, or
    /// always when `refresh`.
    pub fn load_registry(&mut self, refresh: bool, cx: &mut gpui_kit::Context<Self>) {
        let a = &mut self.added_agents;
        if a.loading {
            return;
        }
        let read_cache = !a.cache_read;
        let have = a.registry.as_ref().map(|r| r.fetched_at);
        if !read_cache && !refresh && have.is_some_and(|at| trek_core::store::now_ms() - at < registry::STALE_AFTER_MS) {
            return;
        }
        a.loading = true;
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            let mut at = have;
            if read_cache {
                let cached = registry::load_cached();
                at = at.or(cached.as_ref().map(|r| r.fetched_at));
                let _ = tx.send(Loaded::Cached(cached)).await;
            }
            if refresh || at.is_none_or(|at| trek_core::store::now_ms() - at >= registry::STALE_AFTER_MS) {
                let _ = tx.send(Loaded::Fetched(registry::fetch().await.map_err(|e| format!("{e:#}")))).await;
            }
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(loaded) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    let a = &mut this.added_agents;
                    match loaded {
                        Loaded::Cached(r) => {
                            a.cache_read = true;
                            if a.registry.is_none() {
                                a.registry = r;
                            }
                        }
                        Loaded::Fetched(Ok(r)) => {
                            a.registry = Some(r);
                            a.error = None;
                        }
                        Loaded::Fetched(Err(e)) => a.error = Some(e),
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |this, cx| {
                this.added_agents.loading = false;
                cx.notify();
            });
        });
        self.keep(task);
        cx.notify();
    }

    /// Install registry agent `id` (or update it to the registry's version) and add it.
    pub fn install_registry_agent(&mut self, id: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(agent) = self.added_agents.registry.as_ref().and_then(|r| r.agent(id)).cloned() else { return };
        if matches!(self.added_agents.installs.get(id), Some(Install::Running(_))) {
            return;
        }
        self.added_agents.installs.insert(id.to_string(), Install::Running(None));
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            let steps = tx.clone();
            let progress = move |p: Progress| {
                let _ = steps.try_send(Step::Progress(p));
            };
            let done = registry::install(&agent, &progress).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Step::Done(done)).await;
        });
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(step) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match step {
                        Step::Progress(p) => {
                            this.added_agents.installs.insert(id.clone(), Install::Running(Some(p)));
                        }
                        Step::Done(Ok(added)) => {
                            this.added_agents.installs.remove(&id);
                            let name = added.name.clone();
                            let updated = this.settings.added_agents.iter().any(|a| a.id == added.id);
                            this.put_added(added, cx);
                            cx.emit(WorkspaceEvent::Toast { message: if updated { format!("{name} is up to date") } else { format!("{name} is ready in Trek") }, undo: None });
                        }
                        Step::Done(Err(e)) => {
                            this.added_agents.installs.insert(id.clone(), Install::Failed(e));
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.keep(task);
        cx.notify();
    }

    /// Add an agent of the user's own. `secrets` go to the Keychain first: if one can't, nothing
    /// is added.
    pub fn add_custom_agent(&mut self, agent: AddedAgent, secrets: Vec<(String, String)>, cx: &mut gpui_kit::Context<Self>) -> anyhow::Result<()> {
        for (name, value) in &secrets {
            registry::set_secret(&agent.id, name, value).map_err(|e| anyhow::anyhow!("Couldn't keep {name} in your Keychain: {e}"))?;
        }
        self.put_added(agent, cx);
        Ok(())
    }

    /// Save `agent` as added (replacing one with its id) and look for it.
    fn put_added(&mut self, agent: AddedAgent, cx: &mut gpui_kit::Context<Self>) {
        let agent_id = agent.id.clone();
        let key = AgentId::Acp(agent.id.clone()).key();
        match self.settings.added_agents.iter_mut().find(|a| a.id == agent.id) {
            Some(old) => *old = agent,
            None => self.settings.added_agents.push(agent),
        }
        // A new version or command reports its models afresh.
        self.acp_info.remove(&key);
        let id = agent_id.clone();
        trek_core::runtime().spawn_blocking(move || registry::forget_probe(&id));
        self.save_settings(cx);
        self.sync_added_agents(cx);
    }

    /// Remove an added agent: from settings, and its download, icon and secrets from the Mac.
    pub fn remove_added_agent(&mut self, id: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(at) = self.settings.added_agents.iter().position(|a| a.id == id) else { return };
        let agent = self.settings.added_agents.remove(at);
        let key = AgentId::Acp(agent.id.clone()).key();
        self.settings.disabled_agents.retain(|k| *k != key);
        self.acp_info.remove(&key);
        self.added_agents.installs.remove(id);
        self.save_settings(cx);
        let name = agent.name.clone();
        trek_core::runtime().spawn_blocking(move || registry::remove(&agent));
        self.sync_added_agents(cx);
        cx.emit(WorkspaceEvent::Toast { message: format!("Removed {name}"), undo: None });
    }

    /// The registry's newer version of added agent `id`, when the index Trek has lists one.
    pub fn registry_update(&self, id: &str) -> Option<String> {
        let added = self.settings.added_agents.iter().find(|a| a.id == id && a.source == registry::AgentSource::Registry)?;
        let listed = self.added_agents.registry.as_ref()?.agent(id)?;
        (added.version.as_deref() != Some(listed.version.as_str()) && trek_core::agent_update::is_newer(&listed.version, added.version.as_deref().unwrap_or("0"))).then(|| listed.version.clone())
    }
}

fn builtin(id: &str) -> bool {
    trek_core::catalog::ACP_AGENTS.iter().any(|a| a.id == id)
}

/// An agent the user added rather than one Trek has built in.
pub fn is_added(agent: &AgentId) -> bool {
    matches!(agent, AgentId::Acp(id) if !builtin(id))
}
