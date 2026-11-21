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
            Install::Running(Some(Progress::Downloading {
                done,
                total: Some(total),
            })) if *total > 0 => format!("Downloading {}%", (done * 100 / total).min(100)),
            Install::Running(Some(Progress::Downloading { done, .. })) => {
                format!("Downloading {:.1} MB", *done as f64 / 1e6)
            }
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

pub enum Step {
    Progress(Progress),
    Done(Result<AddedAgent, String>),
}

impl Workspace {
    /// Tell the rest of Trek about the added agents in settings, then find which of them can
    /// start (looking for their programs only: nothing is run, so a test process may too).
    pub fn sync_added_agents(&mut self, cx: &mut gpui_kit::Context<Self>) {
        trek_core::catalog::set_added_agents(&self.settings.added_agents);
        self.prune_registry_installs();
        let list = self.settings.added_agents.clone();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx
                .send(
                    list.iter()
                        .map(trek_core::detect::added_agent)
                        .collect::<Vec<DetectedAgent>>(),
                )
                .await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(found) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                this.merge_added(found);
                // Ask the ones that can start what they offer (not from a test process).
                let ready: Vec<String> = this
                    .agents
                    .iter()
                    .filter(|a| a.availability == Availability::Ready && is_added(&a.agent))
                    .map(|a| a.agent.key())
                    .collect();
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
        self.agents
            .retain(|a| !matches!(&a.agent, AgentId::Acp(id) if !builtin(id)));
        let at = self
            .agents
            .iter()
            .position(|a| matches!(a.agent, AgentId::Direct(_)))
            .unwrap_or(self.agents.len());
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
        if !read_cache
            && !refresh
            && have.is_some_and(|at| trek_core::store::now_ms() - at < registry::STALE_AFTER_MS)
        {
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
            if refresh
                || at.is_none_or(|at| trek_core::store::now_ms() - at >= registry::STALE_AFTER_MS)
            {
                let _ = tx
                    .send(Loaded::Fetched(
                        registry::fetch().await.map_err(|e| format!("{e:#}")),
                    ))
                    .await;
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

    /// Install registry agent `id` and add it. One already added is updated to the registry's
    /// newer version instead, the way a check's update is: through the updates queue when a check
    /// has its row (after its running turns, its sessions ended first), else here, checked to
    /// start before it's used (`registry::update`). Never to an older version.
    pub fn install_registry_agent(&mut self, id: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(agent) = self
            .added_agents
            .registry
            .as_ref()
            .and_then(|r| r.agent(id))
            .cloned()
        else {
            return;
        };
        let key = AgentId::Acp(id.to_string()).key();
        if matches!(
            self.added_agents.installs.get(id),
            Some(Install::Running(_))
        ) || self.agent_updating(&key)
        {
            return;
        }
        let current = self
            .settings
            .added_agents
            .iter()
            .find(|a| a.id == id && a.source == registry::AgentSource::Registry)
            .cloned();
        if current.is_some() {
            let row = format!("registry:{id}");
            if self.agent_updates.request(&row) {
                self.pump_agent_updates(cx);
                cx.notify();
                return;
            }
            // Asked for already: not twice. Otherwise no check has listed it yet.
            if matches!(
                self.agent_updates.job(&row),
                Some(crate::agent_updates::Job::Queued | crate::agent_updates::Job::Running)
            ) {
                return;
            }
        }
        self.added_agents
            .installs
            .insert(id.to_string(), Install::Running(None));
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            let steps = tx.clone();
            let progress = move |p: Progress| {
                let _ = steps.try_send(Step::Progress(p));
            };
            let done = match &current {
                Some(current) => registry::update(current, &agent, &progress)
                    .await
                    .map(|(added, _)| added),
                None => registry::install(&agent, &progress).await,
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Step::Done(done)).await;
        });
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(step) = rx.recv().await {
                let _ = this.update(cx, |this, cx| this.install_step(&id, step, cx));
            }
        });
        self.keep(task);
        cx.notify();
    }

    /// What registry agent `id`'s install reported. One removed while it ran (an update's) stays
    /// removed: what the install left goes too.
    pub fn install_step(&mut self, id: &str, step: Step, cx: &mut gpui_kit::Context<Self>) {
        let Some(Install::Running(progress)) = self.added_agents.installs.get_mut(id) else {
            if let Step::Done(Ok(added)) = step {
                trek_core::runtime().spawn_blocking(move || registry::remove(&added));
            }
            return;
        };
        match step {
            Step::Progress(p) => *progress = Some(p),
            Step::Done(Ok(added)) => {
                self.added_agents.installs.remove(id);
                let name = added.name.clone();
                let updated = self.settings.added_agents.iter().any(|a| a.id == added.id);
                self.put_added(added, cx);
                cx.emit(WorkspaceEvent::Toast {
                    message: if updated {
                        format!("{name} is up to date")
                    } else {
                        format!("{name} is ready in Trek")
                    },
                    undo: None,
                });
            }
            Step::Done(Err(e)) => {
                self.added_agents
                    .installs
                    .insert(id.to_string(), Install::Failed(e));
            }
        }
        cx.notify();
    }

    /// Add an agent of the user's own. `secrets` go to the Keychain first: if one can't, nothing
    /// is added.
    pub fn add_custom_agent(
        &mut self,
        agent: AddedAgent,
        secrets: Vec<(String, String)>,
        cx: &mut gpui_kit::Context<Self>,
    ) -> anyhow::Result<()> {
        for (name, value) in &secrets {
            registry::set_secret(&agent.id, name, value).map_err(|e| {
                anyhow::anyhow!(
                    "Couldn't keep {name} in {}: {e}",
                    crate::words::words().your_credential_store
                )
            })?;
        }
        self.put_added(agent, cx);
        Ok(())
    }

    /// Save `agent` as added (replacing one with its id) and look for it. A registry agent's other
    /// versions go once none of its sessions runs (now, or at the next launch).
    pub(super) fn put_added(&mut self, agent: AddedAgent, cx: &mut gpui_kit::Context<Self>) {
        let key = AgentId::Acp(agent.id.clone()).key();
        match self
            .settings
            .added_agents
            .iter_mut()
            .find(|a| a.id == agent.id)
        {
            Some(old) => *old = agent,
            None => self.settings.added_agents.push(agent),
        }
        // A new version or command reports its models afresh: finding it again (`sync`) asks it.
        self.acp_info.remove(&key);
        let id = key.strip_prefix("acp:").unwrap_or(&key).to_string();
        trek_core::runtime().spawn_blocking(move || registry::forget_probe(&id));
        self.save_settings(cx);
        self.sync_added_agents(cx);
    }

    /// What `key`'s agent reported of itself (models, sign-in) is forgotten, in memory and in its
    /// probe cache, and asked again once it's found: after an update, a new command, a sign-in.
    pub fn forget_acp_info(&mut self, key: &str, cx: &mut gpui_kit::Context<Self>) {
        if !matches!(
            AgentId::from_key(key),
            AgentId::Acp(_) | AgentId::OpenCode | AgentId::Droid
        ) {
            return;
        }
        self.acp_info.remove(key);
        let id = key.strip_prefix("acp:").unwrap_or(key).to_string();
        let (tx, rx) = async_channel::bounded::<()>(1);
        trek_core::runtime().spawn_blocking(move || {
            registry::forget_probe(&id);
            let _ = tx.send_blocking(());
        });
        let key = key.to_string();
        let task = cx.spawn(async move |this, cx| {
            let _ = rx.recv().await;
            let _ = this.update(cx, |this, cx| this.probe_acp_agent(&key, cx));
        });
        self.keep(task);
    }

    /// A registry agent of `key` is being installed or updated from the Add agent sheet.
    pub fn registry_installing(&self, key: &str) -> bool {
        key.strip_prefix("acp:").is_some_and(|id| {
            matches!(
                self.added_agents.installs.get(id),
                Some(Install::Running(_))
            )
        })
    }

    /// Remove registry agents' versions that are no longer theirs, of those with no session
    /// running (a session keeps the version it started with until it ends).
    pub(super) fn prune_registry_installs(&self) {
        let idle: Vec<(String, String)> = self
            .settings
            .added_agents
            .iter()
            .filter(|a| a.source == registry::AgentSource::Registry)
            .filter(|a| {
                let key = AgentId::Acp(a.id.clone()).key();
                !self.registry_installing(&key)
                    && !self.agent_updating(&key)
                    && !self.threads.iter().any(|t| {
                        t.agent.key() == key
                            && self.live.get(&t.id).is_some_and(|l| l.commands.is_some())
                    })
            })
            .filter_map(|a| Some((a.id.clone(), a.version.clone()?)))
            .collect();
        if !idle.is_empty() {
            trek_core::runtime().spawn_blocking(move || {
                for (id, version) in idle {
                    registry::prune(&id, &version);
                }
            });
        }
    }

    /// Remove an added agent: from settings, and its download, icon and secrets from the Mac.
    pub fn remove_added_agent(&mut self, id: &str, cx: &mut gpui_kit::Context<Self>) {
        let Some(at) = self.settings.added_agents.iter().position(|a| a.id == id) else {
            return;
        };
        let agent = self.settings.added_agents.remove(at);
        let key = AgentId::Acp(agent.id.clone()).key();
        self.settings.disabled_agents.retain(|k| *k != key);
        self.acp_info.remove(&key);
        self.added_agents.installs.remove(id);
        self.save_settings(cx);
        let name = agent.name.clone();
        trek_core::runtime().spawn_blocking(move || registry::remove(&agent));
        self.sync_added_agents(cx);
        cx.emit(WorkspaceEvent::Toast {
            message: format!("Removed {name}"),
            undo: None,
        });
    }

    /// The registry's newer version of added agent `id`, when the index Trek has lists one.
    pub fn registry_update(&self, id: &str) -> Option<String> {
        let added = self
            .settings
            .added_agents
            .iter()
            .find(|a| a.id == id && a.source == registry::AgentSource::Registry)?;
        let listed = self.added_agents.registry.as_ref()?.agent(id)?;
        registry::newer_listed(&listed.version, added.version.as_deref().unwrap_or("0"))
            .then(|| listed.version.clone())
    }
}

fn builtin(id: &str) -> bool {
    trek_core::catalog::ACP_AGENTS.iter().any(|a| a.id == id)
}

/// An agent the user added rather than one Trek has built in.
pub fn is_added(agent: &AgentId) -> bool {
    matches!(agent, AgentId::Acp(id) if !builtin(id))
}
