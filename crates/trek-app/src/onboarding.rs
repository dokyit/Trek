//! Onboarding: a short intro (the cairn stacks itself while the hiker walks up to it), then one
//! step at a time: Appearance → Permissions (macOS) → Agents → API keys → Local models →
//! Approvals → Phone & machines → Finish.
//!
//! It shows on a first run, and once more for everyone when a new onboarding ships
//! (`settings.onboarding.version`, see `trek_core::settings::ONBOARDING_VERSION`). Every step is
//! optional: Continue moves on whatever was done, and Skip setup finishes from anywhere. Quit half
//! way and the next launch picks up on the same step. Settings › General and the command palette
//! show it again.
//!
//! One column, left-aligned like the rest of Trek; the footer never moves between steps, so
//! Continue stays under the pointer. Lists are cards of hairline rows with real logos, and the
//! accent colour is kept for the primary button, the progress and the selected choice.

mod helper;
mod steps;

pub(crate) use steps::node_setup_card;

use crate::permissions::{self, Permission, Status};
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use trek_agents::{AcpInfo, AgentStatus, Billing};
use trek_core::AgentId;
use trek_core::catalog::DIRECT_PROVIDERS;
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::settings::Settings;

/// The column steps are laid out in.
const COLUMN: f32 = 600.;

/// One onboarding step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Welcome,
    Appearance,
    Permissions,
    Agents,
    Providers,
    LocalModels,
    Approvals,
    Phone,
    Finish,
}

impl Step {
    pub const ALL: [Step; 9] = [
        Step::Welcome,
        Step::Appearance,
        Step::Permissions,
        Step::Agents,
        Step::Providers,
        Step::LocalModels,
        Step::Approvals,
        Step::Phone,
        Step::Finish,
    ];

    /// Its name in settings (`onboarding.step`) and in the verify driver (`onboarding <key>`).
    pub fn key(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::Appearance => "appearance",
            Step::Permissions => "permissions",
            Step::Agents => "agents",
            Step::Providers => "providers",
            Step::LocalModels => "local-models",
            Step::Approvals => "approvals",
            Step::Phone => "phone",
            Step::Finish => "finish",
        }
    }

    pub fn from_key(key: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.key() == key.trim())
    }

    /// What the progress says it is.
    pub fn label(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::Appearance => "Appearance",
            Step::Permissions => "Permissions",
            Step::Agents => "Agents",
            Step::Providers => "API keys",
            Step::LocalModels => "Local models",
            Step::Approvals => "Approvals",
            Step::Phone => "Phone & machines",
            Step::Finish => "Finish",
        }
    }
}

/// The steps shown, in order: Permissions only where the system has any to ask for (macOS).
pub fn steps(mac: bool) -> Vec<Step> {
    Step::ALL
        .into_iter()
        .filter(|s| mac || *s != Step::Permissions)
        .collect()
}

/// The steps on this platform.
pub fn steps_here() -> Vec<Step> {
    steps(cfg!(target_os = "macos"))
}

/// Where onboarding opens at launch: the step it was left on when one is due and saved (and shown
/// here), else the start.
pub fn start_step(onboarding: &trek_core::settings::Onboarding, here: &[Step]) -> Step {
    onboarding
        .due()
        .then(|| onboarding.step.as_deref().and_then(Step::from_key))
        .flatten()
        .filter(|s| here.contains(s))
        .unwrap_or(Step::Welcome)
}

/// The step after `step` (`None` after the last).
pub fn next_step(step: Step, here: &[Step]) -> Option<Step> {
    let i = here.iter().position(|s| *s == step)?;
    here.get(i + 1).copied()
}

/// The step before `step` (`None` at the start).
pub fn prev_step(step: Step, here: &[Step]) -> Option<Step> {
    let i = here.iter().position(|s| *s == step)?;
    i.checked_sub(1).and_then(|i| here.get(i)).copied()
}

/// Where an agent stands on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Installed and signed in (or needs no sign-in).
    Connected,
    NotSignedIn,
    NotInstalled,
}

/// How an agent that's signed in pays, where Trek can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    /// A subscription login: the plan's name when the agent says it ("Claude Max").
    Subscription(Option<String>),
    /// An API key or a cloud account, charged per token.
    ApiKey,
    Local,
    Unknown,
}

impl Login {
    /// The badge on the agent's row.
    pub fn badge(&self) -> Option<String> {
        match self {
            Login::Subscription(Some(plan)) => Some(plan.clone()),
            Login::Subscription(None) => Some("Subscription".into()),
            Login::ApiKey => Some("API key".into()),
            Login::Local => Some("Local".into()),
            Login::Unknown => None,
        }
    }
}

/// One agent as the agents step lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedAgent {
    pub agent: AgentId,
    pub name: String,
    pub state: AgentState,
    pub login: Login,
    /// The account, its plan, or its version: what's known of it.
    pub detail: String,
}

/// Every coding agent Trek can run, in the order the agents step lists them: what detection
/// found, else what Trek knows of (detection doesn't run in an isolated process).
fn known_agents(detected: &[DetectedAgent]) -> Vec<(AgentId, String, Option<DetectedAgent>)> {
    let mut ids = vec![
        AgentId::ClaudeCode,
        AgentId::Codex,
        AgentId::OpenCode,
        AgentId::Droid,
    ];
    ids.extend(
        trek_core::catalog::ACP_AGENTS
            .iter()
            .map(|a| AgentId::Acp(a.id.into())),
    );
    for a in detected
        .iter()
        .filter(|a| !matches!(a.agent, AgentId::Direct(_)))
    {
        if !ids.contains(&a.agent) {
            ids.push(a.agent.clone());
        }
    }
    ids.into_iter()
        .map(|id| {
            let found = detected.iter().find(|a| a.agent == id).cloned();
            let name = found
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| id.display_name());
            (id, name, found)
        })
        .collect()
}

/// "2.1.287 (Claude Code)" / "grok 1.0.46 (…) [stable]" → "2.1.287" / "1.0.46".
fn clean_version(v: &str) -> Option<String> {
    v.split(|c: char| c.is_whitespace() || c == '(' || c == ',')
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
        .map(|t| t.trim_end_matches('.').to_string())
}

/// What the scan found of every agent: the ones installed first (signed in, then not), then the
/// rest. While a scan runs, one not found yet isn't listed as missing (it may still turn up).
pub fn scan_agents(
    detected: &[DetectedAgent],
    status: &HashMap<String, AgentStatus>,
    acp: &HashMap<String, Result<AcpInfo, String>>,
    detecting: bool,
) -> Vec<ScannedAgent> {
    let mut found = vec![];
    let mut missing = vec![];
    for (agent, name, d) in known_agents(detected) {
        let Some(d) = d.filter(|a| a.availability != Availability::NotInstalled) else {
            if !detecting {
                missing.push(ScannedAgent {
                    agent,
                    name,
                    state: AgentState::NotInstalled,
                    login: Login::Unknown,
                    detail: String::new(),
                });
            }
            continue;
        };
        let key = d.agent.key();
        let st = status.get(&key);
        let info = acp.get(&key);
        let needs_login = d.availability == Availability::NeedsLogin
            || st.is_some_and(|st| !st.logged_in && st.account.is_none())
            || info.is_some_and(|i| i.as_ref().map_or(true, |i| i.needs_auth));
        let login = match st {
            _ if needs_login => Login::Unknown,
            Some(st) => match &st.billing {
                Some(Billing::Plan(plan)) => Login::Subscription(plan.clone().or(st.plan.clone())),
                Some(Billing::Metered) => Login::ApiKey,
                Some(Billing::Local) => Login::Local,
                None if st.plan.is_some() => Login::Subscription(st.plan.clone()),
                None => Login::Unknown,
            },
            None => Login::Unknown,
        };
        let version = d
            .version
            .as_deref()
            .and_then(clean_version)
            .map(|v| format!("Version {v}"));
        let detail = match (st, info) {
            (Some(st), _) if st.account.is_some() => st.account.clone().unwrap_or_default(),
            (_, Some(Ok(i))) if !i.needs_auth && !i.models.is_empty() => {
                [Some(format!("{} models", i.models.len())), version]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
            (_, Some(Err(e))) => e.lines().next().unwrap_or_default().to_string(),
            _ => version.unwrap_or_default(),
        };
        found.push(ScannedAgent {
            agent,
            name,
            state: if needs_login {
                AgentState::NotSignedIn
            } else {
                AgentState::Connected
            },
            login,
            detail,
        });
    }
    // Ready to use first; the order within each is the catalog's.
    found.sort_by_key(|a| a.state != AgentState::Connected);
    found.extend(missing);
    found
}

/// A model server Trek can use, as the local models step shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalServer {
    pub id: &'static str,
    pub name: &'static str,
    pub running: bool,
    pub models: Vec<String>,
    /// Its command-line tool is installed (it can be started from here).
    pub installed: bool,
    /// Not switched off in Settings › Agents.
    pub enabled: bool,
}

impl LocalServer {
    /// The command that starts the server, where one does without more to say.
    pub fn start_command(&self) -> Option<&'static str> {
        match self.id {
            "ollama" => Some("ollama serve"),
            "lmstudio" => Some("lms server start"),
            _ => None,
        }
    }

    /// Where to get it.
    pub fn download(&self) -> &'static str {
        match self.id {
            "ollama" => "https://ollama.com/download",
            "lmstudio" => "https://lmstudio.ai/download",
            _ => "https://github.com/ggml-org/llama.cpp#quick-start",
        }
    }
}

/// The CLI each local server is found by.
pub fn local_cli(id: &str) -> &'static [&'static str] {
    match id {
        "ollama" => &["ollama"],
        "lmstudio" => &["lms"],
        "llamacpp" => &["llama-server", "mlx_lm.server"],
        _ => &[],
    }
}

/// The local model servers: running ones (with the models they serve) from the scan, the rest
/// as installed or not by their CLIs (`clis`: the ids whose CLI was found).
pub fn scan_local(
    detected: &[DetectedAgent],
    clis: &HashSet<String>,
    disabled: &[String],
) -> Vec<LocalServer> {
    DIRECT_PROVIDERS
        .iter()
        .filter(|p| p.local)
        .map(|p| {
            let id = AgentId::Direct(p.id.into());
            let found = detected.iter().find(|a| a.agent == id);
            let running = found.is_some_and(|a| a.availability == Availability::Ready);
            LocalServer {
                id: p.id,
                name: p.name,
                running,
                models: found.map(|a| a.models.clone()).unwrap_or_default(),
                installed: running || clis.contains(p.id),
                enabled: !disabled.contains(&id.key()),
            }
        })
        .collect()
}

/// Where an API provider's key stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// Saved in Trek (in the platform's secret store).
    Saved,
    /// Not saved, but the shell has it (`$VAR`).
    FromEnv(&'static str),
    NotSet,
}

/// What Trek knows of provider `id`'s key without reading the secret store: the providers it
/// saved keys for are in settings, and the shell's variables are read as they are.
pub fn key_state(id: &str, settings: &Settings, env: impl Fn(&str) -> bool) -> KeyState {
    if settings.api_providers.iter().any(|p| p == id) {
        return KeyState::Saved;
    }
    DIRECT_PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.env_key)
        .filter(|k| env(k))
        .map_or(KeyState::NotSet, KeyState::FromEnv)
}

/// The providers shown before "More providers": the ones most people have a key for.
pub const FEATURED_PROVIDERS: [&str; 4] = ["openrouter", "anthropic", "openai", "google"];

/// Where to get a key for provider `id`.
pub fn key_page(id: &str) -> Option<&'static str> {
    Some(match id {
        "anthropic" => "https://console.anthropic.com/settings/keys",
        "openai" => "https://platform.openai.com/api-keys",
        "google" => "https://aistudio.google.com/apikey",
        "openrouter" => "https://openrouter.ai/settings/keys",
        "deepseek" => "https://platform.deepseek.com/api_keys",
        "xai" => "https://console.x.ai",
        "mistral" => "https://console.mistral.ai/api-keys",
        "groq" => "https://console.groq.com/keys",
        _ => return None,
    })
}

pub struct Onboarding {
    workspace: Entity<Workspace>,
    step: Step,
    here: Vec<Step>,
    /// Bumped each time onboarding starts over, so the intro plays again.
    run: usize,
    /// The project the first thread starts in.
    project: Option<std::path::PathBuf>,
    last_step_change: std::time::Instant,
    /// The agents step lists every agent that isn't installed, not just the first few.
    all_missing: bool,
    /// The API keys step lists every provider, not just the featured ones.
    all_providers: bool,
    key_inputs: HashMap<&'static str, Entity<InputState>>,
    glass_slider: Entity<gpui_kit::component::slider::SliderState>,
    /// What each permission was when last looked (every second while the step is open).
    perms: HashMap<Permission, Status>,
    /// Permissions whose prompt was shown this session: their button opens the pane next time.
    asked: HashSet<Permission>,
    _poll: Option<Task<()>>,
    /// Local servers whose CLI is installed (`None` until looked for).
    local_clis: Option<HashSet<String>>,
    /// The floating window with Trek's icon to drag into a pane's list.
    helper: Option<(Permission, AnyWindowHandle)>,
    /// When the intro started playing.
    intro_at: std::time::Instant,
    _subscriptions: Vec<Subscription>,
}

/// How many agents that aren't installed the agents step lists before "Show more".
const MISSING_SHOWN: usize = 4;

impl Onboarding {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let here = steps_here();
        let ws = workspace.read(cx);
        let step = if ws.route == Route::Onboarding {
            start_step(&ws.settings.onboarding, &here)
        } else {
            Step::Welcome
        };
        let frost = crate::settings_view::frost_of(ws.settings.appearance.glass_tint);
        let mut key_inputs = HashMap::new();
        for p in DIRECT_PROVIDERS.iter().filter(|p| !p.local) {
            key_inputs.insert(
                p.id,
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder("Paste key")
                }),
            );
        }
        let glass_slider = cx.new(|_| {
            gpui_kit::component::slider::SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(frost)
        });
        let mut subs = vec![cx.observe(&workspace, |this: &mut Self, ws, cx| {
            if let Some(step) = ws.update(cx, |ws, _| ws.onboarding_restart.take()) {
                this.restart(step, cx);
            }
            cx.notify()
        })];
        // The glass follows the slider as it moves; the setting is saved when it's let go.
        subs.push(cx.subscribe(
            &glass_slider,
            |this: &mut Self, _, event: &gpui_kit::component::slider::SliderEvent, cx| {
                use gpui_kit::component::slider::{SliderEvent, SliderValue};
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
                let SliderValue::Single(percent) = *v else {
                    return;
                };
                let release = matches!(event, SliderEvent::Release(_));
                this.workspace.update(cx, |ws, cx| {
                    ws.settings.appearance.glass_tint = crate::settings_view::tint_at(percent);
                    if release {
                        ws.save_settings(cx);
                    } else {
                        cx.notify();
                    }
                });
            },
        ));
        let mut this = Self {
            workspace,
            step,
            here,
            run: 0,
            project: None,
            last_step_change: std::time::Instant::now() - Duration::from_secs(1),
            all_missing: false,
            all_providers: false,
            key_inputs,
            glass_slider,
            perms: HashMap::new(),
            asked: HashSet::new(),
            _poll: None,
            local_clis: None,
            helper: None,
            intro_at: std::time::Instant::now(),
            _subscriptions: subs,
        };
        this.entered(cx);
        this
    }

    /// The step on screen.
    pub fn step(&self) -> Step {
        self.step
    }

    /// Start over at `step` (shown again from Settings, the palette or the verify driver).
    fn restart(&mut self, step: Step, cx: &mut Context<Self>) {
        self.run += 1;
        self.intro_at = std::time::Instant::now();
        self.last_step_change = std::time::Instant::now() - Duration::from_secs(1);
        self.show(step, cx);
    }

    /// Move to `step` from a click: bursts while a step is still animating in are ignored.
    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        if self.last_step_change.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_step_change = std::time::Instant::now();
        self.show(step, cx);
    }

    fn show(&mut self, step: Step, cx: &mut Context<Self>) {
        // A step this platform doesn't show (Permissions on Windows): the one after it.
        let step = if self.here.contains(&step) {
            step
        } else {
            Step::ALL
                .into_iter()
                .skip_while(|s| *s != step)
                .find(|s| self.here.contains(s))
                .unwrap_or(Step::Finish)
        };
        if self.step != step {
            self.left(cx);
        }
        self.step = step;
        self.workspace
            .update(cx, |ws, cx| ws.remember_onboarding_step(step.key(), cx));
        self.entered(cx);
        cx.notify();
    }

    /// Leaving the step on screen: what it keeps running stops.
    fn left(&mut self, cx: &mut Context<Self>) {
        if self.step == Step::Permissions {
            self._poll = None;
            self.close_helper(cx);
        }
    }

    /// Arrived on the step on screen: start what it needs.
    fn entered(&mut self, cx: &mut Context<Self>) {
        match self.step {
            Step::Permissions => self.watch_permissions(cx),
            Step::LocalModels if self.local_clis.is_none() => self.find_local_clis(cx),
            Step::Finish if self.project.is_none() => {
                self.project = self
                    .workspace
                    .read(cx)
                    .workspace_projects()
                    .first()
                    .map(|p| p.path.clone());
            }
            // Someone new: their other agents' threads come over while they set up.
            Step::Appearance => {
                let ws = self.workspace.read(cx);
                if !ws.settings.onboarding.completed && ws.import_summary.is_none() && !ws.importing
                {
                    self.workspace.update(cx, |ws, cx| ws.import_threads(cx));
                }
            }
            _ => {}
        }
    }

    /// Continue: the next step, or out at the end.
    fn advance(&mut self, cx: &mut Context<Self>) {
        match next_step(self.step, &self.here) {
            Some(next) => self.go(next, cx),
            None => self.finish(cx),
        }
    }

    /// Finished (or skipped): saved so it doesn't show again until a new version, and on to a
    /// new thread in the project picked.
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        self.left(cx);
        let project = self.project.clone();
        self.workspace
            .update(cx, |ws, cx| ws.finish_onboarding(project, cx));
    }

    /// Look at every permission now and every second while the step is open: the rows change as
    /// soon as one is granted in System Settings.
    fn watch_permissions(&mut self, cx: &mut Context<Self>) {
        self.read_permissions(cx);
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| this.read_permissions(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn read_permissions(&mut self, cx: &mut Context<Self>) {
        let now: HashMap<Permission, Status> = Permission::ALL
            .into_iter()
            .map(|p| (p, permissions::status(p)))
            .collect();
        if now != self.perms {
            // The one being dragged in was granted: the helper's job is done.
            if let Some((p, _)) = self.helper
                && now.get(&p) == Some(&Status::Granted)
            {
                self.close_helper(cx);
            }
            self.perms = now;
            cx.notify();
        }
    }

    /// The row's button: the system's prompt where there is one (once), else the pane, with
    /// Trek's icon floating beside it to drag into the list.
    fn grant(&mut self, p: Permission, window: &mut Window, cx: &mut Context<Self>) {
        let status = self.perms.get(&p).copied().unwrap_or(Status::NotYet);
        match p.grant(status, self.asked.contains(&p)) {
            permissions::Grant::Prompt => {
                self.asked.insert(p);
                permissions::prompt(p);
            }
            permissions::Grant::Pane => {
                permissions::open_pane(p, cx);
                if p.takes_a_drop() {
                    self.open_helper(p, window, cx);
                }
            }
        }
        self.read_permissions(cx);
    }

    fn open_helper(&mut self, p: Permission, _: &mut Window, cx: &mut Context<Self>) {
        self.close_helper(cx);
        let this = cx.entity().downgrade();
        if let Some(handle) = helper::open(p, this, cx) {
            self.helper = Some((p, handle));
        }
    }

    pub(crate) fn close_helper(&mut self, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.helper.take() {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    /// Which local servers' CLIs are installed, looked for off the main thread (it reads PATH).
    fn find_local_clis(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move {
            DIRECT_PROVIDERS
                .iter()
                .filter(|p| p.local)
                .filter(|p| {
                    local_cli(p.id)
                        .iter()
                        .any(|bin| trek_core::detect::which(bin).is_some())
                })
                .map(|p| p.id.to_string())
                .collect::<HashSet<String>>()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                this.local_clis = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    /// A settings setter, as Settings' controls use: change, then save.
    fn setter<T: 'static>(
        &self,
        f: fn(&mut Settings, T),
    ) -> impl Fn(T, &mut Window, &mut App) + Clone + 'static {
        let ws = self.workspace.clone();
        move |v: T, _: &mut Window, cx: &mut App| {
            ws.update(cx, |ws, cx| {
                f(&mut ws.settings, v);
                ws.save_settings(cx);
            })
        }
    }

    /// The thin bars along the top: one a step (the intro aside), done ones filled, this one in
    /// the accent. Each takes you to its step.
    fn progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let shown: Vec<Step> = self
            .here
            .iter()
            .copied()
            .filter(|s| *s != Step::Welcome)
            .collect();
        let at = shown.iter().position(|s| *s == self.step).unwrap_or(0);
        let ember = crate::palette::ember(cx);
        v_flex()
            .w(px(COLUMN))
            .gap(px(10.))
            .child(
                h_flex()
                    .gap(px(4.))
                    .children(shown.iter().enumerate().map(|(i, s)| {
                        let s = *s;
                        let color = if i == at {
                            ember
                        } else if i < at {
                            theme.foreground.opacity(0.45)
                        } else {
                            theme.foreground.opacity(0.1)
                        };
                        div()
                            .id(SharedString::from(format!("ob-progress-{}", s.key())))
                            .test_support()
                            .flex_1()
                            .py(px(6.))
                            .cursor_pointer()
                            .child(div().h(px(3.)).rounded_full().bg(color))
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(s.label())
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.go(s, cx)))
                    })),
            )
            .child(
                h_flex()
                    .text_size(px(12.5))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · {} of {}",
                                self.step.label(),
                                at + 1,
                                shown.len()
                            )),
                    )
                    .when(self.step != Step::Finish, |el| {
                        el.child(
                            Button::new("ob-skip")
                                .ghost()
                                .xsmall()
                                .label("Skip setup")
                                .tooltip("Finish now; everything here is in Settings too")
                                .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                        )
                    }),
            )
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let last = self.step == Step::Finish;
        h_flex()
            .w(px(COLUMN))
            .h(px(64.))
            .border_t_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(h_flex().flex_1().when_some(
                prev_step(self.step, &self.here).filter(|s| *s != Step::Welcome),
                |el, prev| {
                    el.child(
                        Button::new("ob-back")
                            .ghost()
                            .label("Back")
                            .on_click(cx.listener(move |this, _, _, cx| this.go(prev, cx))),
                    )
                },
            ))
            .child(
                Button::new("ob-next")
                    .primary()
                    .label(if last { "Start a thread" } else { "Continue" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if last {
                            this.finish(cx)
                        } else {
                            this.advance(cx)
                        }
                    })),
            )
            .into_any_element()
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let reduce = !self.workspace.read(cx).motion(cx);
        let glass = self.workspace.read(cx).glass();
        let bg = if glass.is_some() {
            crate::ui::chrome_bg(glass, cx)
        } else {
            theme.background
        };
        if self.step == Step::Welcome {
            return div()
                .id("onboarding")
                .test_support()
                .size_full()
                .bg(bg)
                .child(self.welcome(reduce, window, cx))
                .into_any_element();
        }
        let content = match self.step {
            Step::Welcome => unreachable!("drawn above"),
            Step::Appearance => self.appearance_step(cx),
            Step::Permissions => self.permissions_step(cx),
            Step::Agents => self.agents_step(cx),
            Step::Providers => self.providers_step(cx),
            Step::LocalModels => self.local_step(cx),
            Step::Approvals => self.approvals_step(cx),
            Step::Phone => self.phone_step(cx),
            Step::Finish => self.finish_step(reduce, cx),
        };
        let animated = div().w_full().child(content).with_animation(
            (
                SharedString::from(format!("ob-step-{}-{}", self.step.key(), self.run)),
                0usize,
            ),
            Animation::new(Duration::from_millis(if reduce { 160 } else { 320 }))
                .with_easing(ease_out_quint()),
            move |el, t| {
                if reduce {
                    el.opacity(t)
                } else {
                    el.opacity(t).mt(px(10. * (1. - t)))
                }
            },
        );
        // The footer is pinned under the scrolling body, so Continue never moves between steps.
        v_flex()
            .id("onboarding")
            .test_support()
            .size_full()
            .bg(bg)
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .justify_center()
                    .pt(px(28.))
                    .child(self.progress(cx)),
            )
            .child(
                div()
                    .id("onboarding-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        h_flex().w_full().justify_center().child(
                            v_flex()
                                .w(px(COLUMN))
                                .pt(px(36.))
                                .pb(px(32.))
                                .child(animated),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .justify_center()
                    .pb(px(16.))
                    .child(self.footer(cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{
        AgentState, DIRECT_PROVIDERS, FEATURED_PROVIDERS, KeyState, Login, Step, key_page,
        key_state, next_step, prev_step, scan_agents, scan_local, start_step, steps,
    };
    use std::collections::{HashMap, HashSet};
    use trek_agents::{AcpInfo, AgentStatus, Billing};
    use trek_core::AgentId;
    use trek_core::detect::{Availability, DetectedAgent};
    use trek_core::settings::Onboarding as Saved;
    use trek_core::settings::Settings;

    fn detected(agent: AgentId, availability: Availability) -> DetectedAgent {
        DetectedAgent {
            name: agent.display_name(),
            agent,
            path: None,
            version: Some("2.1.287 (Claude Code)".into()),
            availability,
            models: vec![],
            install_hint: None,
        }
    }

    #[test]
    fn the_steps_are_the_platforms_and_resume_where_they_were_left() {
        let mac = steps(true);
        let windows = steps(false);
        assert_eq!(mac.first(), Some(&Step::Welcome));
        assert_eq!(mac.last(), Some(&Step::Finish));
        assert!(mac.contains(&Step::Permissions));
        assert!(
            !windows.contains(&Step::Permissions),
            "Windows asks for none"
        );
        assert_eq!(next_step(Step::Appearance, &windows), Some(Step::Agents));
        assert_eq!(next_step(Step::Appearance, &mac), Some(Step::Permissions));
        assert_eq!(prev_step(Step::Agents, &windows), Some(Step::Appearance));
        assert_eq!(next_step(Step::Finish, &mac), None);
        assert_eq!(prev_step(Step::Welcome, &mac), None);
        for s in Step::ALL {
            assert_eq!(Step::from_key(s.key()), Some(s));
        }

        // A first run starts at the beginning.
        let mut saved = Saved::default();
        assert_eq!(start_step(&saved, &mac), Step::Welcome);
        // Quit on a step: the next launch is back on it.
        saved.step = Some("providers".into());
        assert_eq!(start_step(&saved, &mac), Step::Providers);
        // A step this platform doesn't show, or one this build doesn't know, starts over.
        saved.step = Some("permissions".into());
        assert_eq!(start_step(&saved, &windows), Step::Welcome);
        saved.step = Some("somewhere-new".into());
        assert_eq!(start_step(&saved, &mac), Step::Welcome);
        // Finished: nothing to resume (and it isn't shown at launch at all).
        saved.step = Some("providers".into());
        saved.finish();
        assert_eq!(start_step(&saved, &mac), Step::Welcome);
    }

    #[test]
    fn the_scan_says_where_each_agent_stands_and_how_it_pays() {
        let mut status = HashMap::new();
        status.insert(
            AgentId::ClaudeCode.key(),
            AgentStatus {
                account: Some("ada@example.com".into()),
                plan: Some("Claude Max".into()),
                logged_in: true,
                billing: Some(Billing::Plan(None)),
                ..Default::default()
            },
        );
        status.insert(
            AgentId::OpenCode.key(),
            AgentStatus {
                account: Some("API key".into()),
                logged_in: true,
                billing: Some(Billing::Metered),
                ..Default::default()
            },
        );
        let mut acp = HashMap::new();
        acp.insert(
            AgentId::Acp("cursor".into()).key(),
            Ok(AcpInfo {
                needs_auth: true,
                ..Default::default()
            }),
        );
        let found = vec![
            detected(AgentId::ClaudeCode, Availability::Ready),
            detected(AgentId::Codex, Availability::NeedsLogin),
            detected(AgentId::OpenCode, Availability::Ready),
            detected(AgentId::Acp("cursor".into()), Availability::Ready),
            detected(AgentId::Droid, Availability::NotInstalled),
        ];
        let scan = scan_agents(&found, &status, &acp, false);
        let by = |id: AgentId| scan.iter().find(|a| a.agent == id).cloned().unwrap();

        let claude = by(AgentId::ClaudeCode);
        assert_eq!(claude.state, AgentState::Connected);
        assert_eq!(claude.login, Login::Subscription(Some("Claude Max".into())));
        assert_eq!(claude.login.badge().as_deref(), Some("Claude Max"));
        assert_eq!(claude.detail, "ada@example.com");

        assert_eq!(by(AgentId::OpenCode).login, Login::ApiKey);
        assert_eq!(
            by(AgentId::OpenCode).login.badge().as_deref(),
            Some("API key")
        );

        let codex = by(AgentId::Codex);
        assert_eq!(codex.state, AgentState::NotSignedIn);
        assert_eq!(codex.detail, "Version 2.1.287");
        assert_eq!(
            by(AgentId::Acp("cursor".into())).state,
            AgentState::NotSignedIn,
            "an ACP agent that refuses a session until signed in"
        );
        assert_eq!(by(AgentId::Droid).state, AgentState::NotInstalled);

        // Ready ones first, then the ones to sign in to, then the ones to install.
        let states: Vec<AgentState> = scan.iter().map(|a| a.state).collect();
        let mut sorted = states.clone();
        sorted.sort_by_key(|s| match s {
            AgentState::Connected => 0,
            AgentState::NotSignedIn => 1,
            AgentState::NotInstalled => 2,
        });
        assert_eq!(states, sorted);

        // While the scan runs, nothing is called missing yet.
        let scanning = scan_agents(&[], &status, &acp, true);
        assert!(scanning.is_empty());
        // Nothing found at all: every agent Trek knows is listed to install.
        let none = scan_agents(&[], &HashMap::new(), &HashMap::new(), false);
        assert!(none.len() >= 4);
        assert!(none.iter().all(|a| a.state == AgentState::NotInstalled));
    }

    #[test]
    fn local_servers_are_running_installed_or_to_get() {
        let mut ollama = detected(AgentId::Direct("ollama".into()), Availability::Ready);
        ollama.models = vec!["qwen3-coder".into(), "llama3.2".into()];
        let lm = detected(AgentId::Direct("lmstudio".into()), Availability::Offline);
        let clis: HashSet<String> = ["lmstudio".to_string()].into();
        let disabled = vec![AgentId::Direct("ollama".into()).key()];
        let servers = scan_local(&[ollama, lm], &clis, &disabled);
        let by = |id: &str| servers.iter().find(|s| s.id == id).cloned().unwrap();
        let o = by("ollama");
        assert!(o.running && o.installed && !o.enabled);
        assert_eq!(o.models, ["qwen3-coder", "llama3.2"]);
        let l = by("lmstudio");
        assert!(!l.running && l.installed && l.enabled);
        assert_eq!(l.start_command(), Some("lms server start"));
        let c = by("llamacpp");
        assert!(!c.running && !c.installed);
        assert!(c.download().starts_with("https://"));
    }

    #[test]
    fn a_key_is_saved_from_the_shell_or_not_set_without_reading_the_keychain() {
        let mut s = Settings::default();
        s.api_providers.push("openrouter".into());
        assert_eq!(key_state("openrouter", &s, |_| false), KeyState::Saved);
        assert_eq!(
            key_state("openai", &s, |k| k == "OPENAI_API_KEY"),
            KeyState::FromEnv("OPENAI_API_KEY")
        );
        assert_eq!(key_state("openai", &s, |_| false), KeyState::NotSet);
        for id in FEATURED_PROVIDERS {
            assert!(
                DIRECT_PROVIDERS.iter().any(|p| p.id == id && !p.local),
                "{id}"
            );
            assert!(key_page(id).is_some(), "{id} says where to get a key");
        }
    }
}
