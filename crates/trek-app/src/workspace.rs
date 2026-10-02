//! The application model: threads, live agent sessions, routing, updates. Views observe it.

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Task};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use trek_agents::{AcpInfo, AgentStatus, McpServer, SlashCommand, AgentEvent, Command, Decision, SessionConfig};
use trek_core::catalog::{self, ModelInfo};
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::import::ImportSummary;
use trek_core::settings::{FollowUp, Settings};
use trek_core::store::{Item, Project, Section, Store, Thread, ToolStatus, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState, ThreadSource};

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Composing a new thread in a project.
    Draft { project: Option<PathBuf> },
    Thread(String),
    Settings(SettingsPage),
    Onboarding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    General,
    Appearance,
    Notifications,
    Snapshots,
    Shortcuts,
    Agents,
    Skills,
    Tools,
    ApiKeys,
    LocalModels,
    Permissions,
    Import,
    Updates,
    About,
}

impl SettingsPage {
    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Notifications => "Notifications",
            SettingsPage::Snapshots => "App Snapshots",
            SettingsPage::Skills => "Skills",
            SettingsPage::Shortcuts => "Keyboard Shortcuts",
            SettingsPage::Agents => "Agents & Subscriptions",
            SettingsPage::Tools => "Tools & MCP",
            SettingsPage::ApiKeys => "API Keys",
            SettingsPage::LocalModels => "Local Models",
            SettingsPage::Permissions => "Permissions",
            SettingsPage::Import => "Import Threads",
            SettingsPage::Updates => "Updates",
            SettingsPage::About => "About",
        }
    }
}

/// Composer choices: agent, model, effort, hand-holding, plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Effort,
    pub hand_holding: HandHolding,
    pub plan: bool,
    pub fast: bool,
}

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: String,
    pub title: String,
    pub detail: String,
}

/// In-memory state of an open thread: transcript plus its live agent session, if any.
#[derive(Default)]
pub struct LiveThread {
    pub items: Vec<Item>,
    pub loaded: bool,
    pub loading: bool,
    /// Index of the assistant item currently streaming.
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub permissions: Vec<PendingPermission>,
    pub commands: Option<async_channel::Sender<Command>>,
    pub turn_started: Option<Instant>,
    pub plan: bool,
    pub fast: bool,
    pub cost_usd: f64,
    /// Tokens in the context window and the window size, as last reported by the agent.
    pub context: Option<(u64, u64)>,
    /// Bumped on every transcript change so views can resync cheaply.
    pub revision: u64,
    /// Follow-ups held while a turn runs (`FollowUp::Queue`), sent one per finished turn.
    pub queued: Vec<(String, Vec<PathBuf>)>,
    _events: Option<Task<()>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available { version: String, notes: String },
    Downloading { version: String, progress: f32 },
    Ready { version: String, path: PathBuf },
    /// Ready, waiting for running agents to finish.
    RestartPending { version: String, path: PathBuf },
    Failed(String),
}

/// Tools that open as tabs in the right panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTool {
    Git,
    Explorer,
    Terminal,
    Browser,
    SideChat,
    Simulator,
}

impl PanelTool {
    pub const ALL: [PanelTool; 6] = [PanelTool::Terminal, PanelTool::Browser, PanelTool::Simulator, PanelTool::Explorer, PanelTool::SideChat, PanelTool::Git];
    pub fn label(self) -> &'static str {
        match self {
            PanelTool::Git => "Git",
            PanelTool::Explorer => "Explorer",
            PanelTool::Terminal => "Terminal",
            PanelTool::Browser => "Browser",
            PanelTool::SideChat => "Side chat",
            PanelTool::Simulator => "Simulator",
        }
    }
}

pub enum WorkspaceEvent {
    /// Open (or focus) a right-panel tool.
    OpenTool(PanelTool),
    /// A toast-worthy message with an optional undo.
    Toast { message: String, undo: Option<UndoAction> },
    /// A thread needs the user or finished: an in-app toast when it's off screen, plus a system
    /// banner / sound per the notification settings. `viewing` = the thread is on screen.
    Attention { message: String, viewing: bool },
    FocusComposer,
    /// Run a shell command in a new terminal tab (agent install / sign in), then rescan agents.
    RunInTerminal(String),
    /// Insert text at the composer's cursor (e.g. an element picked in the browser).
    InsertIntoComposer(String),
    /// Attach an image to the composer (e.g. a browser screenshot).
    AttachImage(std::path::PathBuf),
}

#[derive(Debug, Clone)]
pub enum UndoAction {
    Unsettle(String),
    Unarchive(String),
}

/// Git state of the folder on screen, for the composer's branch chip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitInfo {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub changed: usize,
    pub ahead: u32,
    /// The remote's default branch (origin/HEAD), else main/master.
    pub default_branch: Option<String>,
    /// Local branches, most recently committed first.
    pub branches: Vec<String>,
    pub remote: Option<String>,
}

impl GitInfo {
    pub fn on_default(&self) -> bool {
        self.branch.is_some() && self.branch == self.default_branch
    }
}

fn read_git_info(cwd: &std::path::Path) -> GitInfo {
    let run = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(cwd).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if run(&["rev-parse", "--is-inside-work-tree"]).is_none() {
        return GitInfo::default();
    }
    GitInfo {
        is_repo: true,
        branch: run(&["branch", "--show-current"]).filter(|b| !b.is_empty()),
        changed: run(&["status", "--porcelain"]).map(|s| s.lines().count()).unwrap_or(0),
        ahead: run(&["rev-list", "--count", "@{u}..HEAD"]).and_then(|s| s.parse().ok()).unwrap_or(0),
        default_branch: run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .and_then(|s| s.split_once('/').map(|(_, b)| b.to_string()))
            .or_else(|| {
                let local = run(&["branch", "--format=%(refname:short)"]).unwrap_or_default();
                ["main", "master", "trunk"].iter().find(|b| local.lines().any(|l| l == **b)).map(|b| b.to_string())
            }),
        branches: run(&["for-each-ref", "--sort=-committerdate", "--count=20", "--format=%(refname:short)", "refs/heads"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
        remote: trek_core::store::git_remote(cwd),
    }
}

pub struct Workspace {
    pub store: Store,
    pub settings: Settings,
    pub threads: Vec<Thread>,
    pub projects: Vec<Project>,
    pub agents: Vec<DetectedAgent>,
    /// Live models from the user's Codex setup (custom providers included).
    pub codex_models: Vec<ModelInfo>,
    pub detecting: bool,
    pub importing: bool,
    pub import_summary: Option<ImportSummary>,
    pub live: HashMap<String, LiveThread>,
    pub route: Route,
    pub draft_prefs: Prefs,
    pub update: UpdateStatus,
    pub sidebar_collapsed: bool,
    pub settled_open: bool,
    pub search: String,
    pub project_filter: Option<String>,
    /// Bumped whenever any agent turn finishes (tools refresh on it).
    pub turns_finished: u64,
    pub git_info: HashMap<PathBuf, GitInfo>,
    /// Account, plan, usage limits and slash commands per vendor CLI, keyed by `AgentId::key()`.
    pub agent_status: HashMap<String, AgentStatus>,
    pub status_fetched_at: i64,
    /// What each installed ACP agent reported (models, login state), keyed by `AgentId::key()`.
    pub acp_info: HashMap<String, Result<AcpInfo, String>>,
    pub usage_loading: bool,
    /// A Trek menu is open over the window; native views (the browser) hide so they don't cover it.
    pub overlay_open: bool,
    /// Composer defaults as last copied into `draft_prefs` (agent, model, effort, hand-holding),
    /// so `save_settings` can tell when the user changed them.
    applied_defaults: (String, Option<String>, Effort, HandHolding),
    tasks: Vec<Task<()>>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let store = Store::open_default().unwrap_or_else(|e| {
            tracing::error!("database: {e}; using in-memory store");
            Store::in_memory().expect("in-memory store")
        });
        // TREK_ONBOARDING=1 replays onboarding without resetting anything (design review, support).
        let replay = std::env::var("TREK_ONBOARDING").is_ok_and(|v| v == "1");
        let route = if settings.onboarding.completed && !replay { Route::Draft { project: None } } else { Route::Onboarding };
        let applied_defaults = default_prefs_key(&settings);
        let draft_prefs = Prefs {
            agent: AgentId::from_key(&settings.general.default_agent),
            model: settings.general.default_model.clone(),
            effort: settings.general.default_effort,
            hand_holding: settings.general.hand_holding,
            plan: false,
            fast: false,
        };
        let mut this = Self {
            store,
            settings,
            threads: vec![],
            projects: vec![],
            agents: vec![],
            codex_models: vec![],
            detecting: false,
            importing: false,
            import_summary: None,
            live: HashMap::new(),
            route,
            draft_prefs,
            update: UpdateStatus::Idle,
            sidebar_collapsed: false,
            settled_open: false,
            search: String::new(),
            project_filter: None,
            turns_finished: 0,
            git_info: HashMap::new(),
            agent_status: HashMap::new(),
            status_fetched_at: 0,
            acp_info: HashMap::new(),
            usage_loading: false,
            overlay_open: false,
            applied_defaults,
            tasks: vec![],
        };
        this.reload(cx);
        if this.route == (Route::Draft { project: None }) {
            let first = this.workspace_projects().first().map(|p| p.path.clone()).or_else(|| this.projects.first().map(|p| p.path.clone()));
            this.route = Route::Draft { project: first };
        }
        this.detect_agents(cx);
        this.refresh_git(cx);
        if this.settings.onboarding.completed {
            this.import_threads(cx);
        }
        if this.settings.updates.auto_check {
            this.check_for_updates(false, cx);
        }
        this.start_housekeeping(cx);
        let keep = this.settings.snapshots.keep_days;
        cx.background_executor().spawn(async move { crate::mentions::prune_snapshots(keep) }).detach();
        this
    }

    // ---------- data ----------

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.threads = self.store.threads().unwrap_or_default();
        self.projects = self.store.projects().unwrap_or_default();
        cx.notify();
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    #[allow(dead_code)]
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn current_thread(&self) -> Option<&Thread> {
        match &self.route {
            Route::Thread(id) => self.thread(id),
            _ => None,
        }
    }

    fn mutate_thread(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Thread)) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == id) {
            f(t);
            if let Err(e) = self.store.save_thread(t) {
                tracing::warn!("save thread: {e}");
            }
            cx.notify();
        }
    }

    pub fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.settings.save() {
            tracing::warn!("save settings: {e}");
        }
        if default_prefs_key(&self.settings) != self.applied_defaults {
            self.apply_default_prefs(cx);
        }
        cx.notify();
    }

    /// Copy the default agent, model, effort and hand-holding from settings into the draft
    /// composer, so the next new thread starts with them. Only fields that changed in settings
    /// since the last apply are copied; the draft's other picks (plan, fast) are kept.
    /// `save_settings` calls this on its own when the defaults change.
    pub fn apply_default_prefs(&mut self, cx: &mut Context<Self>) {
        let (agent, model, effort, hand) = default_prefs_key(&self.settings);
        let (old_agent, old_model, old_effort, old_hand) = std::mem::replace(&mut self.applied_defaults, (agent.clone(), model.clone(), effort, hand));
        let p = &mut self.draft_prefs;
        let agent_id = AgentId::from_key(&agent);
        if agent != old_agent && p.agent != agent_id {
            p.agent = agent_id;
            p.model = model.clone();
        }
        if model != old_model {
            p.model = model;
        }
        if effort != old_effort {
            p.effort = effort;
        }
        if hand != old_hand {
            p.hand_holding = hand;
        }
        cx.notify();
    }

    /// Open Trek's data folder (settings, database, backgrounds) in Finder.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn reveal_data_folder(&self, cx: &App) {
        cx.open_with_system(&trek_core::paths::data_dir());
    }

    /// Plain-text report for "Copy diagnostics": versions, detected agents, where settings live.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn diagnostics(&self) -> String {
        let sw = |flag: &str| {
            std::process::Command::new("/usr/bin/sw_vers")
                .arg(flag)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".into())
        };
        let mut out = vec![
            format!("Trek {}", env!("CARGO_PKG_VERSION")),
            format!("macOS {} ({}) · {}", sw("-productVersion"), sw("-buildVersion"), std::env::consts::ARCH),
            format!("Settings: {}", trek_core::paths::settings_file().display()),
            format!("Data: {}", trek_core::paths::data_dir().display()),
            String::new(),
            "Agents:".into(),
        ];
        if self.agents.is_empty() {
            out.push(if self.detecting { "  (detecting…)".into() } else { "  (none detected)".into() });
        }
        for a in &self.agents {
            let state = match a.availability {
                Availability::Ready => "ready",
                Availability::NotInstalled => "not installed",
                Availability::NeedsLogin => "needs login",
                Availability::Offline => "offline",
            };
            let disabled = if self.settings.disabled_agents.contains(&a.agent.key()) { ", disabled" } else { "" };
            let version = a.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            let path = a.path.as_ref().map(|p| format!(" · {}", p.display())).unwrap_or_default();
            out.push(format!("  {}{version}: {state}{disabled}{path}", a.name));
        }
        out.join("\n")
    }

    /// Threads grouped into sidebar sections, filtered by search and project.
    pub fn sections(&self) -> Vec<(Section, Vec<&Thread>)> {
        let now = now_ms();
        let q = self.search.to_lowercase();
        let mut map: HashMap<Section, Vec<&Thread>> = HashMap::new();
        for t in &self.threads {
            if !q.is_empty() && !t.title.to_lowercase().contains(&q) {
                continue;
            }
            if let Some(p) = &self.project_filter {
                if t.project_id.as_ref() != Some(p) {
                    continue;
                }
            }
            if let Some(s) = t.section(now) {
                map.entry(s).or_default().push(t);
            }
        }
        let mut out: Vec<(Section, Vec<&Thread>)> = map.into_iter().collect();
        out.sort_by_key(|(s, _)| *s);
        for (s, list) in &mut out {
            match s {
                Section::Inbox => list.sort_by_key(|t| t.inbox_rank()),
                Section::Pinned => list.sort_by_key(|t| t.pinned_at),
                _ => list.sort_by_key(|t| -t.updated_at),
            }
        }
        out
    }

    /// Threads waiting on the user (approval or failure), excluding settled and archived ones.
    pub fn needs_you_count(&self) -> usize {
        self.threads.iter().filter(|t| t.needs_you() && t.settled_at.is_none() && t.archived_at.is_none()).count()
    }

    /// Follow-ups waiting for the running turn of `id` to finish (`FollowUp::Queue`).
    pub fn queued(&self, id: &str) -> usize {
        self.live.get(id).map_or(0, |l| l.queued.len())
    }

    /// Drop the follow-ups queued on `id` without sending them.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn clear_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(id) {
            if !live.queued.is_empty() {
                live.queued.clear();
                live.revision += 1;
                cx.notify();
            }
        }
    }

    pub fn any_working(&self) -> bool {
        self.threads.iter().any(|t| t.run_state == RunState::Working)
    }

    /// A thread is working with a live agent turn in this process. Unlike `any_working`, ignores
    /// threads left marked Working by an earlier run that quit mid-turn.
    pub fn any_turn_running(&self) -> bool {
        self.threads
            .iter()
            .any(|t| t.run_state == RunState::Working && self.live.get(&t.id).is_some_and(|l| l.turn_started.is_some()))
    }

    // ---------- navigation ----------

    /// Projects worth offering in pickers: repos and folders the user added.
    pub fn workspace_projects(&self) -> Vec<&Project> {
        self.projects.iter().filter(|p| p.is_workspace(&self.settings.user_projects)).collect()
    }

    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(cwd) = self.current_cwd() else { return };
        let task = cx.spawn(async move |this, cx| {
            let c = cwd.clone();
            let info = cx.background_executor().spawn(async move { read_git_info(&c) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.git_info.get(&cwd) != Some(&info) {
                    this.git_info.insert(cwd, info);
                    cx.notify();
                }
            });
        });
        self.tasks.push(task);
    }

    pub fn current_git(&self) -> Option<&GitInfo> {
        self.current_cwd().and_then(|c| self.git_info.get(&c))
    }

    /// `git switch <branch>` in the current folder.
    pub fn switch_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        let Some(cwd) = self.current_cwd() else { return };
        let task = cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move { std::process::Command::new("git").args(["switch", &branch]).current_dir(&cwd).output() })
                .await;
            let _ = this.update(cx, |this, cx| {
                match out {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).lines().find(|l| l.starts_with("error")).unwrap_or("git switch failed").to_string();
                        cx.emit(WorkspaceEvent::Toast { message: err, undo: None });
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("git: {e}"), undo: None }),
                }
                this.refresh_git(cx);
            });
        });
        self.tasks.push(task);
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Thread(id) = &route {
            let id = id.clone();
            self.mutate_thread(&id, cx, |t| t.last_seen_at = now_ms().max(t.updated_at));
            self.ensure_loaded(&id, cx);
        }
        self.route = route;
        self.refresh_git(cx);
        cx.emit(WorkspaceEvent::FocusComposer);
        cx.notify();
    }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        let project = match &self.route {
            Route::Thread(id) => self.thread(id).and_then(|t| t.cwd.clone()),
            Route::Draft { project } => project.clone(),
            _ => None,
        }
        .or_else(|| self.workspace_projects().first().map(|p| p.path.clone()));
        self.navigate(Route::Draft { project }, cx);
    }

    fn ensure_loaded(&mut self, id: &str, cx: &mut Context<Self>) {
        let thread = self.thread(id).cloned();
        let live = self.live.entry(id.to_string()).or_default();
        if live.loaded || live.loading {
            return;
        }
        let items = self.store.items(id).unwrap_or_default();
        if !items.is_empty() {
            live.items = items;
            live.loaded = true;
            live.revision += 1;
            return;
        }
        let Some(thread) = thread else { return };
        let (Some(native), true) = (thread.native_id.clone(), thread.source != ThreadSource::Trek) else {
            live.loaded = true;
            return;
        };
        live.loading = true;
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            let source = thread.source;
            let result = cx
                .background_executor()
                .spawn(async move { trek_core::import::load_transcript(source, &native) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let live = this.live.entry(id.clone()).or_default();
                live.loading = false;
                live.loaded = true;
                match result {
                    Ok(items) => live.items = items,
                    Err(e) => live.items = vec![Item::Error { text: format!("Couldn't load this thread: {e}") }],
                }
                live.revision += 1;
                cx.notify();
            });
        });
        self.tasks.push(task);
    }

    // ---------- composer prefs ----------

    pub fn prefs(&self) -> Prefs {
        match self.current_thread() {
            Some(t) => Prefs {
                agent: t.agent.clone(),
                model: t.model.clone(),
                effort: t.effort,
                hand_holding: t.hand_holding,
                plan: self.live.get(&t.id).is_some_and(|l| l.plan),
                fast: self.live.get(&t.id).is_some_and(|l| l.fast),
            },
            None => self.draft_prefs.clone(),
        }
    }

    pub fn set_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
        match self.route.clone() {
            Route::Thread(id) => {
                let before = self.thread(&id).cloned();
                self.mutate_thread(&id, cx, |t| {
                    t.model = prefs.model.clone();
                    t.effort = prefs.effort;
                    t.hand_holding = prefs.hand_holding;
                    // Switching agent on an existing thread forks it into a new session.
                    if t.agent != prefs.agent {
                        t.agent = prefs.agent.clone();
                        t.native_id = None;
                    }
                });
                let live = self.live.entry(id.clone()).or_default();
                let fast_changed = live.fast != prefs.fast;
                let plan_changed = live.plan != prefs.plan;
                live.plan = prefs.plan;
                live.fast = prefs.fast;
                if let (Some(before), Some(tx)) = (before, live.commands.clone()) {
                    // Claude reads effort, fast mode and plan at launch: restart idle sessions (they resume).
                    let relaunch = live.turn_started.is_none()
                        && (fast_changed || plan_changed || (prefs.agent == AgentId::ClaudeCode && before.effort != prefs.effort));
                    if before.agent != prefs.agent || relaunch {
                        let _ = tx.try_send(Command::Shutdown);
                        live.commands = None;
                    } else {
                        if before.hand_holding != prefs.hand_holding {
                            let _ = tx.try_send(Command::SetHandHolding(prefs.hand_holding));
                        }
                        if before.model != prefs.model || before.effort != prefs.effort {
                            if let Some(m) = prefs.model.clone() {
                                let _ = tx.try_send(Command::SetModel { model: m, effort: prefs.effort });
                            }
                        }
                    }
                }
            }
            _ => self.draft_prefs = prefs.clone(),
        }
        if let Route::Thread(id) = self.route.clone() {
            self.approve_covered_prompts(&id, prefs.hand_holding, cx);
        }
        cx.notify();
    }

    /// Models for an agent: live data for local servers, catalog otherwise.
    pub fn models_for(&self, agent: &AgentId) -> Vec<ModelInfo> {
        if let AgentId::Direct(p) = agent {
            if let Some(found) = self.agents.iter().find(|a| &a.agent == agent) {
                if !found.models.is_empty() {
                    return found.models.iter().map(|m| ModelInfo { id: m.clone(), name: m.clone(), efforts: vec![], tier: 0, fast: None }).collect();
                }
            }
            return match p.as_str() {
                "anthropic" => catalog::default_models(&AgentId::ClaudeCode),
                "openai" => catalog::default_models(&AgentId::Codex),
                _ => vec![],
            };
        }
        if *agent == AgentId::Codex && !self.codex_models.is_empty() {
            return self.codex_models.clone();
        }
        if let Some(Ok(info)) = self.acp_info.get(&agent.key()) {
            if !info.models.is_empty() {
                return info.models.clone();
            }
        }
        if *agent == AgentId::ClaudeCode {
            if let Some(st) = self.agent_status.get(&agent.key()).filter(|s| !s.models.is_empty()) {
                return st.models.clone();
            }
        }
        catalog::default_models(agent)
    }

    /// The window-wide backdrop image, when the user chose "Everywhere".
    pub fn backdrop(&self) -> Option<(String, f32)> {
        let a = &self.settings.appearance;
        (a.background_placement == trek_core::settings::BackgroundPlacement::Everywhere).then(|| a.background.clone().map(|b| (b, a.background_dim))).flatten()
    }

    /// Working directory for whatever is on screen: the thread's folder or the draft's project.
    pub fn current_cwd(&self) -> Option<PathBuf> {
        match &self.route {
            Route::Thread(id) => self.thread(id).and_then(|t| t.cwd.clone()),
            Route::Draft { project } => project.clone(),
            _ => None,
        }
    }

    /// Copy a user-picked image into Trek's data folder and use it as the background.
    pub fn set_background_image(&mut self, src: PathBuf, cx: &mut Context<Self>) {
        let dir = trek_core::paths::data_dir().join("backgrounds");
        let _ = std::fs::create_dir_all(&dir);
        let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "background".into());
        let dest = dir.join(name);
        match std::fs::copy(&src, &dest) {
            Ok(_) => {
                self.settings.appearance.background = Some(dest.display().to_string());
                self.save_settings(cx);
            }
            Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't use that image: {e}"), undo: None }),
        }
    }

    pub fn pick_background_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use as Background".into()) });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.set_background_image(p, cx));
                }
            }
        });
        self.tasks.push(task);
    }

    /// Start a side chat next to `parent` (or the current draft's project). Hidden from the sidebar.
    pub fn create_side_chat(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let cwd = self.current_cwd();
        let parent = match &self.route {
            Route::Thread(id) => id.clone(),
            _ => "draft".into(),
        };
        let p = self.prefs();
        let mut t = self.store.create_thread(cwd.as_deref(), p.agent, p.model, p.effort, HandHolding::Supervised).ok()?;
        t.title = "Side chat".into();
        t.side_of = Some(parent);
        let _ = self.store.save_thread(&t);
        let id = t.id.clone();
        self.threads.push(t);
        self.live.entry(id.clone()).or_default().loaded = true;
        cx.notify();
        Some(id)
    }

    /// Agents the user can pick right now.
    pub fn ready_agents(&self) -> Vec<AgentId> {
        let mut out: Vec<AgentId> = self
            .agents
            .iter()
            .filter(|a| a.availability == Availability::Ready)
            .filter(|a| match &a.agent {
                AgentId::ClaudeCode | AgentId::Codex => true,
                AgentId::Direct(_) => !a.models.is_empty(),
                // ACP agents are pickable once probed and signed in.
                other => self.acp_info.get(&other.key()).is_some_and(|i| i.as_ref().is_ok_and(|i| !i.needs_auth)),
            })
            .map(|a| a.agent.clone())
            .collect();
        for p in &self.settings.api_providers {
            let id = AgentId::Direct(p.clone());
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out.retain(|a| !self.settings.disabled_agents.contains(&a.key()));
        if out.is_empty() {
            out.push(AgentId::ClaudeCode);
        }
        out
    }

    // ---------- sending ----------

    pub fn send(&mut self, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        if matches!(self.route, Route::Draft { .. }) {
            let mut words = text.split_whitespace();
            if matches!(words.next(), Some("/permissions" | "/access" | "/mode")) {
                let arg = words.next().map(str::to_string);
                let reply = self.permissions_command(None, arg.as_deref(), cx);
                cx.emit(WorkspaceEvent::Toast { message: reply.replace("**", "").replace('`', ""), undo: None });
                return;
            }
        }
        let id = match self.route.clone() {
            Route::Thread(id) => id,
            Route::Draft { project } => {
                let Some(cwd) = project else {
                    cx.emit(WorkspaceEvent::Toast { message: "Pick a project folder first.".into(), undo: None });
                    return;
                };
                let p = self.draft_prefs.clone();
                let mut thread = match self.store.create_thread(Some(&cwd), p.agent, p.model, p.effort, p.hand_holding) {
                    Ok(t) => t,
                    Err(e) => {
                        cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't create thread: {e}"), undo: None });
                        return;
                    }
                };
                thread.title = trek_core::import_title(&text);
                let _ = self.store.save_thread(&thread);
                let id = thread.id.clone();
                self.reload(cx);
                let live = self.live.entry(id.clone()).or_default();
                live.loaded = true;
                live.plan = p.plan;
                self.route = Route::Thread(id.clone());
                id
            }
            _ => return,
        };
        self.send_to(&id, text, images, cx);
    }

    /// Send a prompt to a specific thread (main view or a side chat).
    pub fn send_to(&mut self, id: &str, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        let id = id.to_string();
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        if let Some(reply) = self.run_builtin_command(&id, &text, cx) {
            if reply.is_empty() {
                return;
            }
            let live = self.live.entry(id.clone()).or_default();
            live.items.push(Item::User { text: text.clone(), images: vec![] });
            live.items.push(Item::Notice { text: reply });
            live.revision += 1;
            self.persist_items(&id);
            cx.notify();
            return;
        }
        // Queue mode: hold follow-ups until the running turn finishes (see `apply_events`).
        let running = self.live.get(&id).is_some_and(|l| l.turn_started.is_some() && l.commands.is_some());
        if running && self.settings.general.follow_up == FollowUp::Queue {
            let live = self.live.entry(id.clone()).or_default();
            live.queued.push((text, images));
            live.revision += 1;
            cx.notify();
            return;
        }
        self.ensure_session(&id, cx);
        let live = self.live.entry(id.clone()).or_default();
        live.items.push(Item::User { text: text.clone(), images: images.iter().map(|p| p.display().to_string()).collect() });
        live.streaming = None;
        live.reasoning = None;
        live.turn_started = Some(Instant::now());
        live.revision += 1;
        if let Some(tx) = &live.commands {
            let _ = tx.try_send(Command::Prompt { text, images });
        }
        self.mutate_thread(&id, cx, |t| {
            t.run_state = RunState::Working;
            t.settled_at = None;
            t.snoozed_until = None;
            t.updated_at = now_ms();
            t.last_seen_at = t.updated_at;
        });
        self.persist_items(&id);
    }

    fn ensure_session(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id).cloned() else { return };
        if self.live.get(id).is_some_and(|l| l.commands.is_some()) {
            return;
        }
        let fast_on = self.live.get(id).is_some_and(|l| l.fast);
        let cwd = thread.cwd.clone().unwrap_or_else(trek_core::paths::home);
        let fast_tier = if fast_on {
            let models = self.models_for(&thread.agent);
            let m = thread.model.as_ref().and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id)));
            m.and_then(|m| m.fast.clone())
        } else {
            None
        };
        let mcp_servers = self.mcp_servers();
        let live = self.live.entry(id.to_string()).or_default();
        let handle = trek_agents::start(SessionConfig {
            agent: thread.agent.clone(),
            cwd,
            model: thread.model.clone(),
            effort: thread.effort,
            hand_holding: thread.hand_holding,
            plan: live.plan,
            resume: thread.native_id.clone(),
            fast: fast_tier,
            mcp_servers,
        });
        live.commands = Some(handle.commands);
        let events = handle.events;
        let id = id.to_string();
        live._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                // Batch everything already queued so a burst of tokens is one update.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.apply_events(&id, batch, cx)).is_err() {
                    break;
                }
                // Cap UI updates at ~60 Hz while streaming.
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
        }));
    }

    fn apply_events(&mut self, id: &str, events: Vec<AgentEvent>, cx: &mut Context<Self>) {
        let mut run_state: Option<RunState> = None;
        let mut native: Option<String> = None;
        let mut diff: Option<(i64, i64)> = None;
        let mut finished = false;
        // The turn ended cleanly: queued follow-ups may go out. After a stop or failure they go back
        // to the composer instead, so the user can rethink them.
        let mut continue_queue = false;
        let mut notify_text: Option<String> = None;
        {
            let live = self.live.entry(id.to_string()).or_default();
            for ev in events {
                match ev {
                    AgentEvent::Started { native_id, .. } => {
                        if !native_id.is_empty() {
                            native = Some(native_id);
                        }
                    }
                    AgentEvent::TextDelta(t) => {
                        let ix = match live.streaming {
                            Some(ix) => ix,
                            None => {
                                live.items.push(Item::Assistant { text: String::new() });
                                live.streaming = Some(live.items.len() - 1);
                                live.items.len() - 1
                            }
                        };
                        if let Some(Item::Assistant { text }) = live.items.get_mut(ix) {
                            text.push_str(&t);
                        }
                        live.reasoning = None;
                    }
                    AgentEvent::TextDone(t) => {
                        match live.streaming.take() {
                            Some(ix) => {
                                if let Some(Item::Assistant { text }) = live.items.get_mut(ix) {
                                    *text = t;
                                }
                            }
                            None if !t.trim().is_empty() => live.items.push(Item::Assistant { text: t }),
                            None => {}
                        }
                        live.reasoning = None;
                    }
                    AgentEvent::ReasoningDelta(t) => {
                        let ix = match live.reasoning {
                            Some(ix) => ix,
                            None => {
                                live.items.push(Item::Reasoning { text: String::new() });
                                live.reasoning = Some(live.items.len() - 1);
                                live.items.len() - 1
                            }
                        };
                        if let Some(Item::Reasoning { text }) = live.items.get_mut(ix) {
                            text.push_str(&t);
                        }
                    }
                    AgentEvent::ToolStarted { id: tid, title, detail } => {
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Tool { id: tid, title, detail, output: String::new(), status: ToolStatus::Running });
                    }
                    AgentEvent::ToolFinished { id: tid, output, ok } => {
                        if let Some(Item::Tool { output: o, status, .. }) =
                            live.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id, .. } if *id == tid))
                        {
                            *o = output;
                            *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                        }
                    }
                    AgentEvent::PermissionRequest { request_id, title, detail } => {
                        live.permissions.push(PendingPermission { request_id, title: title.clone(), detail });
                        run_state = Some(RunState::NeedsYou);
                        notify_text = Some(format!("Needs your approval: {title}"));
                    }
                    AgentEvent::DiffStat { additions, deletions } => diff = Some((additions, deletions)),
                    AgentEvent::Context { used, window } => live.context = Some((used, window)),
                    AgentEvent::TurnComplete { cost_usd, error } => {
                        // Models that hide their reasoning leave empty "Thought" rows behind.
                        live.items.retain(|i| !matches!(i, Item::Reasoning { text } if text.trim().is_empty()));
                        live.streaming = None;
                        live.reasoning = None;
                        live.turn_started = None;
                        live.permissions.clear();
                        if let Some(c) = cost_usd {
                            live.cost_usd += c;
                        }
                        if let Some(e) = error {
                            if e != "Interrupted" {
                                live.items.push(Item::Error { text: e });
                                run_state = Some(RunState::Failed);
                                continue_queue = false;
                            } else {
                                live.items.push(Item::Notice { text: "Interrupted".into() });
                                run_state = Some(RunState::Idle);
                                continue_queue = false;
                            }
                        } else {
                            run_state = Some(RunState::Idle);
                            continue_queue = true;
                        }
                        finished = true;
                    }
                    AgentEvent::Error(e) => {
                        live.items.push(Item::Error { text: e });
                        live.turn_started = None;
                        run_state = Some(RunState::Failed);
                        finished = true;
                        continue_queue = false;
                    }
                    AgentEvent::Exited => {
                        continue_queue = false;
                        live.commands = None;
                        if live.turn_started.take().is_some() {
                            run_state.get_or_insert(RunState::Failed);
                        }
                    }
                }
            }
            live.revision += 1;
        }
        let viewing = self.route == Route::Thread(id.to_string());
        self.mutate_thread(id, cx, |t| {
            if let Some(n) = native {
                t.native_id = Some(n);
            }
            if let Some(s) = run_state {
                t.run_state = s;
            }
            if let Some((a, d)) = diff {
                t.additions = a;
                t.deletions = d;
            }
            if finished {
                t.updated_at = now_ms();
                if viewing {
                    t.last_seen_at = t.updated_at;
                }
            }
        });
        if finished {
            self.turns_finished += 1;
            self.refresh_git(cx);
            self.persist_items(id);
            let next = if continue_queue {
                self.live.get_mut(id).and_then(|l| (!l.queued.is_empty()).then(|| l.queued.remove(0)))
            } else {
                None
            };
            if !continue_queue && viewing {
                self.restore_queued(id, cx);
            }
            if let Some((text, images)) = next {
                // The thread keeps going with the user's queued follow-up: not "finished" yet.
                self.send_to(id, text, images, cx);
            } else {
                if let Some(title) = self.thread(id).map(|t| t.title.clone()) {
                    notify_text.get_or_insert(format!("Finished: {title}"));
                }
                self.maybe_restart_for_update(cx);
            }
        }
        if let Some(message) = notify_text {
            cx.emit(WorkspaceEvent::Attention { message, viewing });
        }
        cx.notify();
    }

    /// Put queued follow-ups back into the composer (the thread must be on screen).
    fn restore_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(queued) = self.live.get_mut(id).map(|l| std::mem::take(&mut l.queued)) else { return };
        if queued.is_empty() {
            return;
        }
        let text = queued.iter().map(|(t, _)| t.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        if !text.is_empty() {
            cx.emit(WorkspaceEvent::InsertIntoComposer(text));
        }
        for path in queued.into_iter().flat_map(|(_, images)| images) {
            cx.emit(WorkspaceEvent::AttachImage(path));
        }
    }

    fn persist_items(&self, id: &str) {
        if let Some(live) = self.live.get(id) {
            let _ = self.store.set_items(id, &live.items);
        }
    }

    pub fn respond(&mut self, id: &str, request_id: &str, decision: Decision, cx: &mut Context<Self>) {
        let mut still_waiting = false;
        if let Some(live) = self.live.get_mut(id) {
            live.permissions.retain(|p| p.request_id != request_id);
            still_waiting = !live.permissions.is_empty();
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::Respond { request_id: request_id.to_string(), decision });
            }
            live.revision += 1;
        }
        if !still_waiting {
            self.mutate_thread(id, cx, |t| t.run_state = RunState::Working);
        }
        cx.notify();
    }

    pub fn interrupt(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(tx) = self.live.get(id).and_then(|l| l.commands.clone()) {
            let _ = tx.try_send(Command::Interrupt);
        }
        cx.notify();
    }

    // ---------- inbox lifecycle ----------

    pub fn settle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| {
            t.settled_at = Some(now_ms());
            t.pinned_at = None;
            t.snoozed_until = None;
            t.last_seen_at = t.updated_at.max(t.last_seen_at);
        });
        cx.emit(WorkspaceEvent::Toast { message: "Settled".into(), undo: Some(UndoAction::Unsettle(id.into())) });
    }

    pub fn unsettle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.settled_at = None);
    }

    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.pinned_at = if t.pinned_at.is_some() { None } else { Some(now_ms()) });
    }

    pub fn snooze(&mut self, id: &str, hours: i64, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.snoozed_until = Some(now_ms() + hours * 3_600_000));
        cx.emit(WorkspaceEvent::Toast { message: format!("Snoozed for {hours} h"), undo: None });
    }

    pub fn mark_unread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.last_seen_at = t.updated_at - 1);
    }

    pub fn archive(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.archived_at = Some(now_ms()));
        if self.route == Route::Thread(id.into()) {
            self.new_thread(cx);
        }
        self.threads.retain(|t| t.archived_at.is_none());
        cx.emit(WorkspaceEvent::Toast { message: "Archived".into(), undo: Some(UndoAction::Unarchive(id.into())) });
    }

    #[allow(dead_code)]
    pub fn rename(&mut self, id: &str, title: String, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.title = title);
    }

    pub fn undo(&mut self, action: UndoAction, cx: &mut Context<Self>) {
        match action {
            UndoAction::Unsettle(id) => self.unsettle(&id, cx),
            UndoAction::Unarchive(id) => {
                let _ = self.store.update_thread(&id, |t| t.archived_at = None);
                self.reload(cx);
            }
        }
    }

    /// Auto-settle and snooze wake-ups, once a minute.
    fn start_housekeeping(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            let alive = this.update(cx, |this, cx| {
                let now = now_ms();
                let days = this.settings.inbox.auto_settle_days;
                let due: Vec<String> =
                    this.threads.iter().filter(|t| t.should_auto_settle(now, days)).map(|t| t.id.clone()).collect();
                for id in due {
                    this.mutate_thread(&id, cx, |t| t.settled_at = Some(now));
                }
                // Snoozes that have expired fall back into the inbox on their own (section()).
                if now - this.status_fetched_at > 5 * 60_000 {
                    this.refresh_usage(cx);
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        });
        self.tasks.push(task);
    }

    // ---------- projects ----------

    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.add_project(path, cx));
                }
            }
        });
        self.tasks.push(task);
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.store.ensure_project(&path) {
            Ok(p) => {
                let key = p.path.display().to_string();
                if !self.settings.user_projects.contains(&key) {
                    self.settings.user_projects.push(key);
                    let _ = self.settings.save();
                }
                self.reload(cx);
                self.navigate(Route::Draft { project: Some(p.path) }, cx);
            }
            Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't open folder: {e}"), undo: None }),
        }
    }

    /// Clone `owner/repo` (or a URL) with the GitHub CLI, falling back to git.
    pub fn clone_repo(&mut self, spec: String, cx: &mut Context<Self>) {
        let spec = spec.trim().to_string();
        if spec.is_empty() {
            return;
        }
        let name = spec.trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or("repo").to_string();
        let dest = trek_core::paths::home().join("Developer").join(&name);
        cx.emit(WorkspaceEvent::Toast { message: format!("Cloning {spec}…"), undo: None });
        let task = cx.spawn(async move |this, cx| {
            let dest2 = dest.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _ = std::fs::create_dir_all(dest2.parent().unwrap());
                    let path = trek_core::detect::login_path();
                    let gh = trek_core::detect::which("gh");
                    let status = if let (Some(gh), false) = (gh, spec.contains("://")) {
                        std::process::Command::new(gh).args(["repo", "clone", &spec]).arg(&dest2).env("PATH", path).output()
                    } else {
                        let url = if spec.contains("://") || spec.starts_with("git@") { spec.clone() } else { format!("https://github.com/{spec}.git") };
                        std::process::Command::new("git").args(["clone", &url]).arg(&dest2).env("PATH", path).output()
                    };
                    match status {
                        Ok(o) if o.status.success() => Ok(()),
                        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.add_project(dest, cx),
                Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Clone failed: {e}"), undo: None }),
            });
        });
        self.tasks.push(task);
    }

    // ---------- discovery ----------

    pub fn detect_agents(&mut self, cx: &mut Context<Self>) {
        if self.detecting {
            return;
        }
        self.detecting = true;
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::detect::detect_all().await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(agents) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.agents = agents;
                    this.detecting = false;
                    if this.agents.iter().any(|a| a.agent == AgentId::Codex && a.availability == Availability::Ready) {
                        this.fetch_codex_models(cx);
                    }
                    this.status_fetched_at = 0;
                    this.refresh_usage(cx);
                    this.probe_acp_agents(cx);
                    // Default to an agent that's actually installed.
                    let ready = this.ready_agents();
                    if !ready.contains(&this.draft_prefs.agent) {
                        if let Some(a) = ready.first() {
                            this.draft_prefs.agent = a.clone();
                            this.draft_prefs.model = None;
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.tasks.push(task);
    }

    /// Re-read account, plan, usage limits and commands from the installed vendor CLIs. Free:
    /// no prompt is sent. Throttled to once every 30 seconds.
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_loading || now_ms() - self.status_fetched_at < 30_000 {
            return;
        }
        let ready = |id: AgentId| self.agents.iter().any(|a| a.agent == id && a.availability == Availability::Ready) && !self.settings.disabled_agents.contains(&id.key());
        let (claude, codex) = (ready(AgentId::ClaudeCode), ready(AgentId::Codex));
        if !claude && !codex {
            return;
        }
        self.usage_loading = true;
        let cwd = self.current_cwd().unwrap_or_else(trek_core::paths::home);
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let (a, b) = tokio::join!(
                async { if claude { Some(trek_agents::claude_status(&cwd).await) } else { None } },
                async { if codex { Some(trek_agents::codex_status(&cwd).await) } else { None } },
            );
            let _ = tx.send([(AgentId::ClaudeCode, a), (AgentId::Codex, b)]).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(results) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                for (agent, res) in results {
                    match res {
                        Some(Ok(st)) => {
                            if agent == AgentId::Codex && !st.models.is_empty() {
                                this.codex_models = st.models.clone();
                            }
                            this.agent_status.insert(agent.key(), st);
                        }
                        Some(Err(e)) => {
                            let st = this.agent_status.entry(agent.key()).or_default();
                            st.error = Some(e.to_string());
                        }
                        None => {}
                    }
                }
                this.usage_loading = false;
                this.status_fetched_at = now_ms();
                cx.notify();
            });
        });
        self.tasks.push(task);
        cx.notify();
    }

    /// Ask each installed ACP agent for its models and login state. Opens a session, sends no prompt.
    pub fn probe_acp_agents(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .agents
            .iter()
            .filter(|a| a.availability == Availability::Ready && matches!(a.agent, AgentId::OpenCode | AgentId::Droid | AgentId::Acp(_)))
            .map(|a| a.agent.key())
            .collect();
        for id in ids {
            let (tx, rx) = async_channel::bounded(1);
            let probe_id = id.strip_prefix("acp:").unwrap_or(&id).to_string();
            trek_core::runtime().spawn(async move {
                let r = trek_agents::acp_probe(&probe_id).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(r).await;
            });
            let task = cx.spawn(async move |this, cx| {
                if let Ok(r) = rx.recv().await {
                    let _ = this.update(cx, |this, cx| {
                        this.acp_info.insert(id, r);
                        cx.notify();
                    });
                }
            });
            self.tasks.push(task);
        }
    }

    /// Slash commands for an agent: Trek's own first, then the agent's commands and skills.
    pub fn slash_commands(&self, agent: &AgentId) -> Vec<SlashCommand> {
        let mut out: Vec<SlashCommand> = BUILTIN_COMMANDS
            .iter()
            .map(|(n, d)| SlashCommand { name: n.to_string(), description: d.to_string(), kind: trek_agents::CommandKind::Command })
            .collect();
        if let Some(st) = self.agent_status.get(&agent.key()) {
            for c in &st.commands {
                if !out.iter().any(|o| o.name == c.name) {
                    out.push(c.clone());
                }
            }
        }
        out
    }

    /// Commands Trek answers itself instead of sending to the agent.
    /// Prompts already on screen when the user raises the level: approve the ones the new level
    /// would never have asked about (everything for Full access, file edits for Auto-accept edits).
    fn approve_covered_prompts(&mut self, id: &str, level: HandHolding, cx: &mut Context<Self>) {
        let covered: Vec<String> = self
            .live
            .get(id)
            .map(|l| {
                l.permissions
                    .iter()
                    .filter(|p| match level {
                        HandHolding::FullAccess => true,
                        HandHolding::AutoAcceptEdits | HandHolding::Auto => matches!(p.title.as_str(), "Edit" | "Write"),
                        HandHolding::Supervised => false,
                    })
                    .map(|p| p.request_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for rid in covered {
            self.respond(id, &rid, Decision::Allow, cx);
        }
    }

    /// Change a thread's hand-holding (or the draft's), as the composer menu does.
    pub fn set_hand_holding(&mut self, id: Option<&str>, level: HandHolding, cx: &mut Context<Self>) -> Result<(), String> {
        if level == HandHolding::FullAccess && !self.settings.permissions.full_access_unlocked {
            return Err("Full access is off. Turn on “Allow Full access” in Settings → Permissions first.".into());
        }
        match id {
            Some(id) if self.route == Route::Thread(id.to_string()) => {
                let mut p = self.prefs();
                p.hand_holding = level;
                self.set_prefs(p, cx);
            }
            Some(id) => {
                self.mutate_thread(id, cx, |t| t.hand_holding = level);
                if let Some(tx) = self.live.get(id).and_then(|l| l.commands.clone()) {
                    let _ = tx.try_send(Command::SetHandHolding(level));
                }
                self.approve_covered_prompts(id, level, cx);
            }
            None => self.draft_prefs.hand_holding = level,
        }
        cx.notify();
        Ok(())
    }

    /// `/permissions [supervised|edits|auto|full]` — shows or changes the level.
    fn permissions_command(&mut self, id: Option<&str>, arg: Option<&str>, cx: &mut Context<Self>) -> String {
        let current = id.and_then(|i| self.thread(i)).map(|t| t.hand_holding).unwrap_or(self.draft_prefs.hand_holding);
        let level = match arg.map(|a| a.to_lowercase()) {
            None => {
                return format!(
                    "Hand-holding is **{}**. Switch with `/permissions supervised`, `/permissions edits`, `/permissions auto` or `/permissions full`.",
                    current.label()
                )
            }
            Some(a) => match a.as_str() {
                "supervised" | "ask" | "default" => HandHolding::Supervised,
                "edits" | "accept-edits" | "auto-accept" | "auto-accept-edits" | "acceptedits" => HandHolding::AutoAcceptEdits,
                "auto" => HandHolding::Auto,
                "full" | "full-access" | "bypass" | "yolo" => HandHolding::FullAccess,
                other => return format!("Unknown level “{other}”. Use supervised, edits, auto or full."),
            },
        };
        match self.set_hand_holding(id, level, cx) {
            Ok(()) => format!("Hand-holding set to **{}**. {}", level.label(), level.description()),
            Err(e) => e,
        }
    }

    fn run_builtin_command(&mut self, id: &str, text: &str, cx: &mut Context<Self>) -> Option<String> {
        let cmd = text.strip_prefix('/')?.split_whitespace().next()?;
        let agent = self.thread(id).map(|t| t.agent.clone())?;
        match cmd {
            "permissions" | "access" | "mode" => {
                let arg = text.split_whitespace().nth(1).map(str::to_string);
                Some(self.permissions_command(Some(id), arg.as_deref(), cx))
            }
            "clear" | "new" => {
                let project = self.thread(id).and_then(|t| t.cwd.clone());
                self.navigate(Route::Draft { project }, cx);
                Some(String::new())
            }
            "usage" => {
                self.status_fetched_at = 0;
                self.refresh_usage(cx);
                let st = self.agent_status.get(&agent.key())?;
                let mut lines = vec![format!("**{}** · {}", agent.display_name(), st.plan.clone().unwrap_or_else(|| "no plan reported".into()))];
                for l in &st.limits {
                    lines.push(format!("- {}: {:.0}% used{}", l.label, l.percent, l.resets_at.map(|r| format!(", resets {}", crate::time::until(r))).unwrap_or_default()));
                }
                Some(lines.join("\n"))
            }
            "context" => {
                let (used, window) = self.live.get(id).and_then(|l| l.context)?;
                Some(format!("{} of {} tokens in context ({:.0}%).", fmt_tokens(used), fmt_tokens(window), used as f64 / window.max(1) as f64 * 100.))
            }
            "cost" => {
                let c = self.live.get(id).map(|l| l.cost_usd).unwrap_or(0.0);
                Some(format!("This session has cost ${c:.2} so far (API-priced estimate; subscriptions aren't billed per token)."))
            }
            "model" => {
                let t = self.thread(id)?;
                Some(format!("{} · {}", agent.display_name(), t.model.clone().unwrap_or_else(|| "default model".into())))
            }
            _ => None,
        }
    }

    /// MCP servers handed to every new agent session: Trek's own tools plus the user's.
    pub fn mcp_servers(&self) -> Vec<McpServer> {
        let tools = &self.settings.tools;
        let mut out = vec![];
        if let Some(bin) = trek_mcp_binary() {
            let bin = bin.display().to_string();
            for (on, family) in [(tools.computer_use, "computer"), (tools.simulator, "simulator")] {
                if on {
                    out.push(McpServer { name: format!("trek-{family}"), command: bin.clone(), args: vec![family.into()], env: vec![] });
                }
            }
        }
        for s in tools.mcp_servers.iter().filter(|s| s.enabled) {
            out.push(McpServer { name: s.name.clone(), command: s.command.clone(), args: s.args.clone(), env: vec![] });
        }
        out
    }

    fn fetch_codex_models(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_agents::codex_models().await.unwrap_or_default()).await;
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(models) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.codex_models = models;
                    cx.notify();
                });
            }
        });
        self.tasks.push(task);
    }

    pub fn import_threads(&mut self, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        self.importing = true;
        cx.notify();
        let store = self.store.clone();
        let settings = self.settings.import.clone();
        let task = cx.spawn(async move |this, cx| {
            let summary = cx.background_executor().spawn(async move { trek_core::import::import_all(&store, &settings) }).await;
            let _ = this.update(cx, |this, cx| {
                this.importing = false;
                this.import_summary = Some(summary);
                this.reload(cx);
            });
        });
        self.tasks.push(task);
    }

    pub fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.settings.onboarding.completed = true;
        self.settings.general.hand_holding = self.draft_prefs.hand_holding;
        self.settings.general.default_agent = self.draft_prefs.agent.key();
        self.save_settings(cx);
        let project = self.projects.first().map(|p| p.path.clone());
        self.navigate(Route::Draft { project }, cx);
    }

    // ---------- updates ----------

    pub fn check_for_updates(&mut self, user_initiated: bool, cx: &mut Context<Self>) {
        if matches!(self.update, UpdateStatus::Checking | UpdateStatus::Downloading { .. }) {
            return;
        }
        self.update = UpdateStatus::Checking;
        cx.notify();
        let feed = trek_core::update::feed_url(&self.settings.updates);
        let auto_download = self.settings.updates.auto_download;
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::update::check(&feed).await.map_err(|e| e.to_string())).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                this.update = match result {
                    Ok(Some(u)) => {
                        let status = UpdateStatus::Available { version: u.version.to_string(), notes: u.notes.clone() };
                        if auto_download {
                            this.update = status;
                            this.download_update(u, cx);
                            return;
                        }
                        status
                    }
                    Ok(None) => UpdateStatus::UpToDate,
                    // A failed background check stays quiet; a manual one reports.
                    Err(e) if user_initiated => UpdateStatus::Failed(e),
                    Err(_) => UpdateStatus::Idle,
                };
                cx.notify();
            });
        });
        self.tasks.push(task);
    }

    fn download_update(&mut self, update: trek_core::update::AvailableUpdate, cx: &mut Context<Self>) {
        let version = update.version.to_string();
        self.update = UpdateStatus::Downloading { version: version.clone(), progress: 0.0 };
        let (tx, rx) = async_channel::unbounded::<Result<Option<PathBuf>, String>>();
        let (ptx, prx) = async_channel::unbounded::<f32>();
        trek_core::runtime().spawn(async move {
            let r = trek_core::update::download(&update, |p| {
                let _ = ptx.try_send(p);
            })
            .await;
            let _ = tx.send(r.map(Some).map_err(|e| e.to_string())).await;
        });
        let v2 = version.clone();
        let progress_task = cx.spawn(async move |this, cx| {
            while let Ok(p) = prx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if let UpdateStatus::Downloading { progress, .. } = &mut this.update {
                        *progress = p;
                        cx.notify();
                    }
                });
            }
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.update = match result {
                        Ok(Some(path)) => UpdateStatus::Ready { version: v2, path },
                        Ok(None) => UpdateStatus::Idle,
                        Err(e) => UpdateStatus::Failed(e),
                    };
                    cx.notify();
                });
            }
        });
        self.tasks.push(progress_task);
        self.tasks.push(task);
    }

    /// Install now if no agent is running; otherwise wait for them to finish.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateStatus::Ready { version, path } = self.update.clone() {
            if self.any_working() {
                self.update = UpdateStatus::RestartPending { version, path };
                cx.emit(WorkspaceEvent::Toast { message: "Trek will restart when your agents finish.".into(), undo: None });
                cx.notify();
            } else {
                self.install_update(path, cx);
            }
        }
    }

    fn maybe_restart_for_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateStatus::RestartPending { path, .. } = self.update.clone() {
            if !self.any_working() {
                self.install_update(path, cx);
            }
        }
    }

    fn install_update(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match trek_core::update::install_and_relaunch(&path) {
            Ok(()) => cx.quit(),
            Err(e) => {
                self.update = UpdateStatus::Failed(format!("{e:#}"));
                cx.notify();
            }
        }
    }

    pub fn shutdown_sessions(&mut self) {
        for live in self.live.values() {
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::Shutdown);
            }
        }
    }
}

/// The settings that seed a new thread's composer.
fn default_prefs_key(s: &Settings) -> (String, Option<String>, Effort, HandHolding) {
    let g = &s.general;
    (g.default_agent.clone(), g.default_model.clone(), g.default_effort, g.hand_holding)
}

/// Convenience for views.
pub fn workspace_global(cx: &App) -> Entity<Workspace> {
    cx.global::<GlobalWorkspace>().0.clone()
}

pub struct GlobalWorkspace(pub Entity<Workspace>);
impl gpui_kit::Global for GlobalWorkspace {}

pub fn init(cx: &mut App) -> Entity<Workspace> {
    let ws = cx.new(Workspace::new);
    cx.set_global(GlobalWorkspace(ws.clone()));
    ws
}

/// Commands Trek handles itself, shown first in the `/` menu.
pub const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("new", "Start a new thread in this project"),
    ("usage", "Show plan usage and reset times"),
    ("context", "Show how much of the context window is used"),
    ("cost", "Show this session's estimated cost"),
    ("model", "Show the model this thread uses"),
    ("permissions", "Show or change how much the agent asks first"),
    ("permissions supervised", "Ask before every edit and command"),
    ("permissions edits", "Apply file edits; ask before commands"),
    ("permissions auto", "Work on its own; check before risky actions"),
    ("permissions full", "No prompts and no sandbox"),
];

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0}K", n as f64 / 1_000.),
        _ => {
            let m = n as f64 / 1_000_000.;
            if m.fract() < 0.05 { format!("{m:.0}M") } else { format!("{m:.1}M") }
        }
    }
}

/// The bundled `trek-mcp` server: next to the app binary, else a dev build.
pub fn trek_mcp_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("trek-mcp"), dir.join("../Resources/trek-mcp")].into_iter().find(|p| p.exists())
}
