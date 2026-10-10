//! Settings. A nav column replaces the thread sidebar (as in Codex). Pages are flat sections of
//! hairline-separated rows: a label and one-line explanation on the left, the control on the right.

mod connections;
mod mobile;
mod pages;

/// What the settings pages said, in words, since the last `take`: row titles and descriptions,
/// headings and notes. The test harness sees elements, not text, so a test asks this instead.
#[cfg(test)]
pub(crate) mod shown {
    use std::cell::RefCell;

    thread_local! {
        static SAID: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn text(text: &str) {
        SAID.with(|s| s.borrow_mut().push(text.to_string()));
    }

    /// A row's title (when it is plain text) and description.
    pub(super) fn title_and(title: &dyn std::any::Any, description: &str) {
        if let Some(t) = title.downcast_ref::<&str>() {
            text(t);
        } else if let Some(t) = title.downcast_ref::<String>() {
            text(t);
        }
        text(description);
    }

    pub(crate) fn take() -> Vec<String> {
        SAID.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}

use crate::palette;
use crate::ui;
use crate::workspace::{Route, SettingsPage, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_core::catalog::DIRECT_PROVIDERS;
use trek_core::detect::Availability;
use trek_core::import::{ImportedThread, Skip};
use trek_core::settings::{Settings, secrets};
use trek_core::AgentId;

/// What the iOS Simulator section says where Windows has nothing to show.
const SIMULATOR_NEEDS_A_MAC: &str = "The iOS Simulator needs macOS, so Trek doesn't offer it on Windows.";

pub(crate) fn page_icon(p: SettingsPage) -> Icon {
    match p {
        SettingsPage::Project => Icon::new(crate::assets::Lucide::FolderCog),
        SettingsPage::General => Icon::new(IconName::Settings2),
        SettingsPage::Appearance => Icon::new(IconName::Palette),
        SettingsPage::Notifications => Icon::new(IconName::Bell),
        SettingsPage::Snapshots => Icon::new(crate::assets::Lucide::Camera),
        SettingsPage::Skills => Icon::new(crate::assets::Lucide::Sparkle),
        SettingsPage::Shortcuts => Icon::new(crate::assets::Lucide::Keyboard),
        SettingsPage::Agents => Icon::new(IconName::Bot),
        SettingsPage::Tools => Icon::new(crate::assets::Lucide::Plug),
        SettingsPage::Mobile => Icon::new(crate::assets::Lucide::Smartphone),
        SettingsPage::ApiKeys => Icon::new(crate::assets::Lucide::Lock),
        SettingsPage::LocalModels => Icon::new(IconName::Cpu),
        SettingsPage::Permissions => Icon::new(crate::assets::Lucide::ShieldCheck),
        SettingsPage::Import => Icon::new(IconName::ArrowDown),
        SettingsPage::Updates => Icon::new(IconName::RefreshCw),
        SettingsPage::About => Icon::new(IconName::Info),
    }
}

const NAV_GROUPS: &[(&str, &[SettingsPage])] = &[
    ("Project", &[SettingsPage::Project]),
    ("App", &[SettingsPage::General, SettingsPage::Appearance, SettingsPage::Notifications, SettingsPage::Mobile, SettingsPage::Snapshots, SettingsPage::Shortcuts]),
    ("Agents", &[SettingsPage::Agents, SettingsPage::Skills, SettingsPage::ApiKeys, SettingsPage::LocalModels, SettingsPage::Tools]),
    ("Workflow", &[SettingsPage::Permissions, SettingsPage::Import]),
    ("Trek", &[SettingsPage::Updates, SettingsPage::About]),
];

/// Every settings page, in the order the navigation lists them.
pub(crate) fn pages() -> impl Iterator<Item = SettingsPage> {
    NAV_GROUPS.iter().flat_map(|(_, pages)| pages.iter().copied())
}

/// A settings page by its label, case-insensitive with dashes for spaces ("updates", "api-keys").
pub fn page_named(name: &str) -> Option<SettingsPage> {
    let name = name.trim().to_lowercase();
    pages().find(|p| p.label().to_lowercase().replace(' ', "-") == name)
}

pub(crate) fn page_blurb(p: SettingsPage) -> std::borrow::Cow<'static, str> {
    let w = crate::words::words();
    match p {
        SettingsPage::Snapshots => crate::keys::localize("Screenshots you attach from the composer’s + menu or with ⌘⇧S."),
        SettingsPage::Mobile => format!(
            "Keep your threads going from your iPhone: see what every agent is doing, answer approvals and questions, and steer or start work. The agents keep running on {}.",
            w.this_computer
        )
        .into(),
        SettingsPage::ApiKeys => w.api_keys_blurb.into(),
        SettingsPage::LocalModels => format!("Model servers running on {}: Ollama, LM Studio, and llama.cpp or MLX. Trek finds them on their usual ports.", w.this_computer).into(),
        SettingsPage::Import => format!("Trek reads, and never changes, the threads other agents keep on {}, so you can browse and continue them here.", w.this_computer).into(),
        SettingsPage::Project | SettingsPage::About => "".into(),
        SettingsPage::General => "What a new thread starts with, and how the composer behaves while an agent works.".into(),
        SettingsPage::Appearance => "Theme, text size, background art and motion.".into(),
        SettingsPage::Notifications => "How Trek tells you an agent finished or needs a decision.".into(),
        SettingsPage::Skills => "Instructions your agents can load on demand. Turn a skill off and every agent stops seeing it; turn it on to bring it back.".into(),
        SettingsPage::Shortcuts => "Every keyboard shortcut in Trek.".into(),
        SettingsPage::Agents => "Trek runs each vendor's own agent with the login you already have, so your subscriptions just work. Trek never reads or stores those credentials.".into(),
        // Windows has no iOS Simulator, so its blurb doesn't promise one.
        SettingsPage::Tools if crate::words::is_windows() => "Computer use, and the MCP servers, skills and plugins your agents can call.".into(),
        SettingsPage::Tools => "Computer use, the iOS Simulator, and the MCP servers, skills and plugins your agents can call.".into(),
        SettingsPage::Permissions => "How much each agent may do without asking.".into(),
        SettingsPage::Updates => "Trek checks for signed updates, gets them ready in the background, and installs them when you restart or quit.".into(),
    }
}

/// The settings navigation column, shown in place of the thread sidebar.
pub struct SettingsNav {
    workspace: Entity<Workspace>,
    _subscription: Subscription,
}

impl SettingsNav {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self { workspace, _subscription: sub }
    }
}

impl Render for SettingsNav {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = match self.workspace.read(cx).route {
            Route::Settings(p) => p,
            _ => SettingsPage::General,
        };
        let theme = cx.theme().clone();
        v_flex()
            .w(px(crate::root::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .px(px(10.))
            .pt(px(4.))
            .gap(px(1.))
            .when(!self.workspace.read(cx).see_through(), |el| el.bg(theme.sidebar))
            .child(
                h_flex()
                    .id("settings-back")
                    .px(px(10.))
                    .h(px(30.))
                    .gap(px(10.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.foreground.opacity(0.045)).text_color(theme.foreground))
                    .child(Icon::new(IconName::ArrowLeft).size(px(15.)))
                    .child("Back to threads")
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx)))),
            )
            .children(NAV_GROUPS.iter().map(|(group, pages)| {
                v_flex()
                    .pt(px(20.))
                    .gap(px(1.))
                    .child(div().px(px(10.)).pb(px(6.)).text_size(px(11.5)).font_medium().text_color(theme.muted_foreground.opacity(0.8)).child(*group))
                    .children(pages.iter().map(|&p| {
                        let label: &'static str = p.label();
                        // Agents: how many CLI updates are out (while Trek keeps checking: with checks
                        // off, what the last check found isn't fresh enough to badge).
                        let badge = (p == SettingsPage::Agents)
                            .then(|| self.workspace.read(cx))
                            .filter(|ws| ws.settings.updates.check_agents)
                            .map(|ws| ws.agent_updates.pending())
                            .filter(|n| *n > 0);
                        ui::nav_row(label, page_icon(p), label, None, p == current, cx)
                            .when_some(badge, |el, n| {
                                el.child(
                                    div()
                                        .id("agents-update-count")
                                        .test_support()
                                        .px(px(6.))
                                        .rounded_full()
                                        .bg(theme.foreground.opacity(0.08))
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(n.to_string()),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(p), cx))))
                    }))
            }))
    }
}

pub struct SettingsView {
    workspace: Entity<Workspace>,
    key_inputs: HashMap<&'static str, Entity<InputState>>,
    saved_keys: HashMap<&'static str, bool>,
    mcp_name: Entity<InputState>,
    mcp_command: Entity<InputState>,
    /// A header for a remote MCP server being added (shown once the command is a URL); the
    /// value goes to the Keychain.
    mcp_header_name: Entity<InputState>,
    mcp_header_value: Entity<InputState>,
    /// The MCP server loaded into the add row to be edited (its name before the edit).
    mcp_editing: Option<String>,
    /// Skills page: the last scan, filter, and new-skill fields.
    skills: Probe<Vec<trek_core::skills::Skill>>,
    skill_filter: Entity<InputState>,
    skill_name: Entity<InputState>,
    skill_desc: Entity<InputState>,
    skill_home: trek_core::skills::SkillHome,
    /// Project page: the name field (and which project it currently shows), and the new-action fields.
    project_name: Entity<InputState>,
    project_name_for: Option<String>,
    /// What new worktrees get a copy of, comma-separated (follows the project like the name).
    worktree_copy: Entity<InputState>,
    action_name: Entity<InputState>,
    action_command: Entity<InputState>,
    /// Import page: rules whose left-out sessions are listed.
    left_out_open: HashSet<Skip>,
    /// Appearance › Material: how frosted liquid glass is, 0 (clear) to 100 (frosted).
    pub(super) glass_slider: Entity<gpui_kit::component::slider::SliderState>,
    /// What the Tools and Snapshots pages show from the Mac's setup, and whether the project's
    /// image icon is still there, each read off the main thread.
    tools: Probe<ToolsProbe>,
    /// Whether the Figma desktop app's MCP server answers (Tools › Connections).
    figma_desktop: Probe<bool>,
    snapshots: Probe<SnapshotsProbe>,
    icon_file: Probe<(String, bool)>,
    /// Trek's data folder, found once (finding it makes sure it exists).
    pub(super) data_dir: std::path::PathBuf,
    _subscriptions: Vec<Subscription>,
}

/// Something a page shows that takes the disk, a process or a system call to learn. It's read on a
/// background thread when the page opens (and again when it may have changed); the page draws what
/// was last read, or a loading state before the first read. Never read from `render`.
pub(super) struct Probe<T> {
    pub(super) value: Option<T>,
    fresh: bool,
    task: Option<Task<()>>,
}

impl<T> Default for Probe<T> {
    fn default() -> Self {
        Self { value: None, fresh: false, task: None }
    }
}

impl<T> Probe<T> {
    /// Read again next time the page is drawn; a read in flight is dropped (it may be out of date).
    pub(super) fn invalidate(&mut self) {
        self.fresh = false;
        self.task = None;
    }

    pub(super) fn loading(&self) -> bool {
        self.task.is_some()
    }
}

/// The Tools page's view of the Mac: privacy permissions, AXe, Trek's MCP server, and what each
/// agent already loads.
#[derive(Clone, Default)]
pub(super) struct ToolsProbe {
    accessibility: bool,
    screen_recording: bool,
    axe: bool,
    mcp_bundled: bool,
    agent_mcp: Vec<(&'static str, Vec<String>)>,
    plugins: Vec<String>,
    /// Claude Code's and Codex's link to Figma's remote server.
    figma: Vec<(AgentId, connections::FigmaLink)>,
}

impl ToolsProbe {
    fn read() -> Self {
        let agent_mcp = crate::integrations::agent_mcp_servers();
        let plugins = crate::integrations::claude_plugins();
        let figma = connections::figma_links(&agent_mcp, &plugins, |a| trek_core::detect::which(if *a == AgentId::Codex { "codex" } else { "claude" }).is_some());
        Self {
            accessibility: crate::integrations::accessibility_allowed(),
            screen_recording: crate::integrations::screen_recording_allowed(),
            axe: crate::integrations::axe_path().is_some(),
            mcp_bundled: crate::workspace::trek_mcp_binary().is_some(),
            agent_mcp,
            plugins,
            figma,
        }
    }
}

/// The Snapshots page: how many are kept and their size, and the Screen Recording permission.
#[derive(Clone, Default)]
pub(super) struct SnapshotsProbe {
    pub(super) count: usize,
    pub(super) bytes: u64,
    pub(super) screen_recording: bool,
}

impl SnapshotsProbe {
    fn read() -> Self {
        let (count, bytes) = crate::mentions::snapshot_usage();
        Self { count, bytes, screen_recording: crate::integrations::screen_recording_allowed() }
    }
}

/// Liquid glass's tint at frost `percent` (the slider), and back.
pub(crate) fn tint_at(percent: f32) -> f32 {
    0.2 + percent.clamp(0., 100.) / 100. * 0.75
}

pub(crate) fn frost_of(tint: f32) -> f32 {
    ((tint - 0.2) / 0.75 * 100.).clamp(0., 100.)
}

impl SettingsView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut key_inputs = HashMap::new();
        let mut saved_keys = HashMap::new();
        for p in DIRECT_PROVIDERS.iter().filter(|p| !p.local) {
            key_inputs.insert(p.id, cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Paste key")));
            saved_keys.insert(p.id, secrets::api_key(p.id).is_some());
        }
        let subs = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        let mcp_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, e.g. github"));
        let mcp_command = cx.new(|cx| InputState::new(window, cx).placeholder("A command, a URL, or a server's JSON config"));
        let mcp_header_name = cx.new(|cx| InputState::new(window, cx).placeholder("Header name"));
        let mcp_header_value = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(format!("Value, kept in {} (optional)", crate::words::words().your_credential_store)));
        let skill_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter skills"));
        let skill_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, e.g. Review pull requests"));
        let skill_desc = cx.new(|cx| InputState::new(window, cx).placeholder("When should an agent use it?"));
        let mut subs = subs;
        subs.push(cx.observe(&skill_filter, |_, _, cx| cx.notify()));
        // A URL shows the header fields.
        subs.push(cx.observe(&mcp_command, |_, _, cx| cx.notify()));
        // Return in the add row adds (or saves) the server.
        for input in [&mcp_name, &mcp_command, &mcp_header_value] {
            subs.push(cx.subscribe_in(input, window, |this: &mut Self, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::PressEnter { .. }) {
                    this.add_mcp_server(window, cx);
                }
            }));
        }
        // Each page reads the Mac again whenever it's opened.
        subs.push(cx.observe(&workspace, |this: &mut Self, ws, cx| {
            let route = ws.read(cx).route.clone();
            if route != Route::Settings(SettingsPage::Skills) {
                this.skills.invalidate();
            }
            if route != Route::Settings(SettingsPage::Tools) {
                this.tools.invalidate();
                this.figma_desktop.invalidate();
            }
            if route != Route::Settings(SettingsPage::Snapshots) {
                this.snapshots.invalidate();
            }
            if route != Route::Settings(SettingsPage::Project) {
                this.icon_file.invalidate();
            }
            // Registry agents' rows offer the registry's newer versions: its index is read once.
            if route == Route::Settings(SettingsPage::Agents) {
                ws.update(cx, |ws, cx| {
                    let listed = ws.settings.added_agents.iter().any(|a| a.source == trek_core::registry::AgentSource::Registry);
                    if listed && ws.added_agents.registry.is_none() && ws.added_agents.error.is_none() {
                        ws.load_registry(false, cx);
                    }
                });
            }
        }));
        // Permissions are granted in System Settings: coming back from there reads them again.
        subs.push(cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.tools.invalidate();
                this.figma_desktop.invalidate();
                this.snapshots.invalidate();
                cx.notify();
            }
        }));
        let project_name = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        let action_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, e.g. Test"));
        let worktree_copy = cx.new(|cx| InputState::new(window, cx).placeholder("Nothing is copied"));
        subs.push(cx.subscribe_in(&worktree_copy, window, |this: &mut Self, input, event: &gpui_kit::component::input::InputEvent, _, cx| {
            if matches!(event, gpui_kit::component::input::InputEvent::PressEnter { .. } | gpui_kit::component::input::InputEvent::Blur) {
                let list: Vec<String> = input.read(cx).value().split(',').map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
                let Some(id) = this.project_name_for.clone() else { return };
                this.workspace.update(cx, |ws, cx| {
                    let Some(path) = ws.project(&id).map(|p| p.path.clone()) else { return };
                    if ws.project_prefs(&path).worktree_copy != list {
                        ws.update_project_prefs(&path, |pr| pr.worktree_copy = list, cx);
                    }
                });
            }
        }));
        let action_command = cx.new(|cx| InputState::new(window, cx).placeholder("Command, e.g. cargo test"));
        let frost = frost_of(workspace.read(cx).settings.appearance.glass_tint);
        let glass_slider = cx.new(|_| gpui_kit::component::slider::SliderState::new().min(0.).max(100.).step(1.).default_value(frost));
        // The glass follows the slider as it moves; the setting is saved when it's let go.
        subs.push(cx.subscribe(&glass_slider, |this: &mut Self, _, event: &gpui_kit::component::slider::SliderEvent, cx| {
            let (gpui_kit::component::slider::SliderEvent::Change(v) | gpui_kit::component::slider::SliderEvent::Release(v)) = event;
            let gpui_kit::component::slider::SliderValue::Single(percent) = *v else { return };
            let release = matches!(event, gpui_kit::component::slider::SliderEvent::Release(_));
            this.workspace.update(cx, |ws, cx| {
                ws.settings.appearance.glass_tint = tint_at(percent);
                if release {
                    ws.save_settings(cx);
                } else {
                    cx.notify();
                }
            });
        }));
        subs.push(cx.subscribe_in(&project_name, window, |this: &mut Self, input, event: &gpui_kit::component::input::InputEvent, _, cx| {
            if matches!(event, gpui_kit::component::input::InputEvent::PressEnter { .. } | gpui_kit::component::input::InputEvent::Blur) {
                let name = input.read(cx).value().to_string();
                if let Some(id) = this.project_name_for.clone() {
                    this.workspace.update(cx, |ws, cx| {
                        if ws.project(&id).is_some_and(|p| p.name != name.trim()) {
                            ws.rename_project(&id, &name, cx);
                        }
                    });
                }
            }
        }));
        Self {
            workspace,
            key_inputs,
            saved_keys,
            mcp_name,
            mcp_command,
            mcp_header_name,
            mcp_header_value,
            mcp_editing: None,
            project_name,
            project_name_for: None,
            worktree_copy,
            action_name,
            action_command,
            skills: Probe::default(),
            skill_filter,
            skill_name,
            skill_desc,
            skill_home: trek_core::skills::SkillHome::ClaudeCode,
            left_out_open: HashSet::new(),
            glass_slider,
            tools: Probe::default(),
            figma_desktop: Probe::default(),
            snapshots: Probe::default(),
            icon_file: Probe::default(),
            data_dir: trek_core::paths::data_dir(),
            _subscriptions: subs,
        }
    }

    /// Read `slot` on a background thread unless it's fresh or already being read; the page
    /// redraws when it's in.
    fn probe<T: Send + 'static>(&mut self, slot: fn(&mut Self) -> &mut Probe<T>, read: impl FnOnce() -> T + Send + 'static, cx: &mut Context<Self>) {
        let p = slot(self);
        if p.fresh || p.task.is_some() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let value = cx.background_executor().spawn(async move { read() }).await;
            let _ = this.update(cx, |this, cx| {
                let p = slot(this);
                p.value = Some(value);
                p.fresh = true;
                p.task = None;
                cx.notify();
            });
        });
        slot(self).task = Some(task);
    }

    /// Start reading what `page` shows from the Mac, if it isn't fresh.
    fn probe_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        match page {
            SettingsPage::Tools => {
                self.probe(|this| &mut this.tools, ToolsProbe::read, cx);
                self.probe_figma_desktop(cx);
            }
            SettingsPage::Snapshots => self.probe(|this| &mut this.snapshots, SnapshotsProbe::read, cx),
            SettingsPage::Skills => {
                let project = self.workspace.read(cx).current_cwd();
                self.probe(|this| &mut this.skills, move || trek_core::skills::discover(project.as_deref()), cx)
            }
            _ => {}
        }
    }

    /// Whether the project's image icon (`file:` spec) is still on disk; `None` until it's known.
    pub(super) fn icon_file_exists(&mut self, spec: &str, cx: &mut Context<Self>) -> Option<bool> {
        let file = spec.strip_prefix("file:")?.to_string();
        if self.icon_file.value.as_ref().is_some_and(|(s, _)| s != spec) {
            self.icon_file = Probe::default();
        }
        let spec = spec.to_string();
        self.probe(|this| &mut this.icon_file, move || (spec, std::path::Path::new(&file).exists()), cx);
        self.icon_file.value.as_ref().map(|(_, exists)| *exists)
    }

    fn page(&self, cx: &App) -> SettingsPage {
        match self.workspace.read(cx).route {
            Route::Settings(p) => p,
            _ => SettingsPage::General,
        }
    }

    /// Closure that edits settings and saves.
    fn setter<T: 'static>(&self, f: fn(&mut Settings, T)) -> impl Fn(T, &mut Window, &mut App) + Clone + 'static {
        let ws = self.workspace.clone();
        move |v: T, _: &mut Window, cx: &mut App| {
            ws.update(cx, |ws, cx| {
                f(&mut ws.settings, v);
                ws.save_settings(cx);
            })
        }
    }

    fn row(title: impl IntoElement + 'static, description: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> AnyElement {
        Self::row_with(title, description, None, control, cx)
    }

    /// `row`, with a line of its own under the description (a server's status).
    fn row_with(title: impl IntoElement + 'static, description: impl Into<SharedString>, extra: Option<AnyElement>, control: impl IntoElement, cx: &App) -> AnyElement {
        let description: SharedString = description.into();
        #[cfg(test)]
        shown::title_and(&title, &description);
        h_flex()
            .w_full()
            .min_h(px(52.))
            .gap(px(32.))
            .py(px(13.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().text_size(px(13.5)).font_medium().child(title))
                    .when(!description.is_empty(), |el| {
                        el.child(div().max_w(px(460.)).text_size(px(12.5)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground).child(description))
                    })
                    .children(extra),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    fn switch(&self, id: &'static str, on: bool, f: fn(&mut Settings, bool)) -> Switch {
        let set = self.setter(f);
        Switch::new(id).checked(on).on_click(move |v: &bool, window, cx| set(*v, window, cx))
    }

    /// Section heading: more space above than below (rhythm), real weight instead of an eyebrow.
    fn heading(text: &str, cx: &App) -> AnyElement {
        #[cfg(test)]
        shown::text(text);
        div().pt(px(36.)).pb(px(10.)).text_size(px(13.)).font_semibold().text_color(cx.theme().foreground).child(text.to_string()).into_any_element()
    }

    fn note(text: &str, cx: &App) -> AnyElement {
        #[cfg(test)]
        shown::text(text);
        div().pb(px(16.)).max_w(px(560.)).text_size(px(13.)).line_height(relative(1.55)).text_color(cx.theme().muted_foreground).child(text.to_string()).into_any_element()
    }

    /// What checking MCP server `i` found: its tools, or why it didn't answer.
    fn mcp_status(i: usize, check: &crate::workspace::McpCheck, cx: &App) -> AnyElement {
        use crate::workspace::McpCheck;
        let muted = cx.theme().muted_foreground;
        let line = h_flex().id(SharedString::from(format!("mcp-status-{i}"))).test_support().max_w(px(460.)).gap(px(7.)).pt(px(2.)).text_size(px(12.5));
        let dot = |color: Hsla| div().flex_none().size(px(6.)).rounded_full().bg(color);
        match check {
            McpCheck::Checking(_) => line.text_color(muted).child(dot(muted.opacity(0.5))).child("Checking…").into_any_element(),
            McpCheck::Works(tools) => {
                let count = match tools.len() {
                    0 => "Works, no tools".to_string(),
                    1 => "Works · 1 tool".to_string(),
                    n => format!("Works · {n} tools"),
                };
                let all: SharedString = tools.join(", ").into();
                line.child(dot(palette::emerald(cx)))
                    .child(div().flex_none().child(count))
                    .when(!tools.is_empty(), |el| el.child(div().min_w_0().text_color(muted).text_ellipsis().whitespace_nowrap().overflow_hidden().child(all.clone())))
                    .when(!tools.is_empty(), |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(all.clone()).build(window, cx)))
                    .into_any_element()
            }
            McpCheck::Failed(why) => line.items_start().child(div().mt(px(6.)).child(dot(palette::red(cx)))).child(div().min_w_0().line_height(relative(1.5)).child(why.clone())).into_any_element(),
        }
    }

    fn status_dot(color: Hsla, label: &'static str) -> AnyElement {
        h_flex().gap(px(7.)).text_size(px(12.5)).child(div().size(px(6.)).rounded_full().bg(color)).child(label).into_any_element()
    }

    /// Thumbnails: None, the built-in art, the user's own image, and "Choose image…".
    fn background_gallery(&self, s: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = s.appearance.background.clone();
        let mut options: Vec<(Option<String>, &'static str)> =
            vec![(None, "None"), (Some("builtin:dawn".into()), "Dawn"), (Some("builtin:night".into()), "Night"), (Some("builtin:paper".into()), "Paper")];
        if let Some(c) = current.clone().filter(|c| !c.starts_with("builtin:")) {
            options.push((Some(c), "Yours"));
        }
        let tile = |id: SharedString, selected: bool| {
            v_flex().id(id).gap(px(6.)).cursor_pointer().child(
                div()
                    .w(px(100.))
                    .h(px(66.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if selected { theme.foreground } else { theme.border })
                    .border_dashed()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Plus).text_color(theme.muted_foreground)),
            )
        };
        h_flex()
            .flex_wrap()
            .gap(px(12.))
            .px(px(4.))
            .children(options.into_iter().map(|(spec, label)| {
                let selected = spec == current;
                let preview = spec.clone();
                let ws = self.workspace.clone();
                v_flex()
                    .id(SharedString::from(format!("bg-{label}")))
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(
                        div()
                            .relative()
                            .w(px(100.))
                            .h(px(66.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if selected { theme.foreground } else { theme.border })
                            .bg(theme.background)
                            .when_some(preview, |el, spec| el.child(img(ui::background_source(&spec)).size_full().object_fit(ObjectFit::Cover))),
                    )
                    .child(div().text_xs().text_color(if selected { theme.foreground } else { theme.muted_foreground }).child(label))
                    .on_click(move |_, _, cx| {
                        let spec = spec.clone();
                        ws.update(cx, |ws, cx| {
                            ws.settings.appearance.background = spec;
                            ws.save_settings(cx);
                        })
                    })
            }))
            .child(
                tile("bg-choose".into(), false)
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Choose image…"))
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.pick_background_image(cx)))),
            )
            .into_any_element()
    }

    /// One row per agent: logo, account and plan, enable switch, and Sign in or Install.
    fn agents_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ws = self.workspace.read(cx);
        let detecting = ws.detecting;
        let muted = cx.theme().muted_foreground;
        let disabled = ws.settings.disabled_agents.clone();
        let agents: Vec<_> = ws.agents.iter().filter(|a| !matches!(a.agent, AgentId::Direct(_))).cloned().collect();
        let (installed, missing): (Vec<_>, Vec<_>) = agents.into_iter().partition(|a| a.availability != Availability::NotInstalled);
        let mut out: Vec<AnyElement> = vec![];
        let setup_key = |a: &AgentId| match a {
            AgentId::Acp(id) => id.clone(),
            other => other.key(),
        };
        let mut rows = vec![];
        for a in installed {
            let key = a.agent.key();
            let status = ws.agent_status.get(&key);
            let acp = ws.acp_info.get(&key);
            let on = !disabled.contains(&key);
            let setup = trek_core::catalog::agent_setup(&setup_key(&a.agent));
            // Account line: who is signed in and on which plan.
            let (account, needs_login): (String, bool) = match (&a.agent, status, acp) {
                (_, Some(st), _) if st.logged_in || st.account.is_some() => {
                    let parts: Vec<String> = [st.account.clone(), st.plan.clone()].into_iter().flatten().collect();
                    (if parts.is_empty() { "Signed in".into() } else { parts.join(" · ") }, false)
                }
                (_, Some(_), _) => ("Not signed in".into(), true),
                (_, _, Some(Ok(info))) if info.needs_auth => ("Sign in to use this agent".into(), true),
                (_, _, Some(Ok(info))) => {
                    let n = info.models.len();
                    (if n > 0 { format!("Signed in · {n} models") } else { "Signed in".into() }, false)
                }
                (_, _, Some(Err(e))) => (e.lines().next().unwrap_or("Couldn't start").to_string(), true),
                // Agents that are off aren't asked (asking starts them).
                (_, None, None) if !on => ("Off".into(), false),
                // One the user added says where it came from until it's been asked.
                (agent, None, None) if crate::workspace::is_added(agent) => (crate::add_agent::source_line(agent).unwrap_or_default(), false),
                (AgentId::ClaudeCode | AgentId::Codex, None, None) if ws.usage_loading => ("Checking account…".into(), false),
                (_, None, None) if a.availability == Availability::NeedsLogin => ("Not signed in".into(), true),
                _ => ("Checking account…".into(), false),
            };
            // "2.1.287 (Claude Code)", "grok 1.0.46 (…) [stable]" → "2.1.287", "1.0.46".
            let version = a.version.as_deref().and_then(|v| {
                v.split(|c: char| c.is_whitespace() || c == '(' || c == ',').find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
            });
            let account = match version {
                Some(v) => format!("{account} · v{}", v.trim_end_matches('.')),
                None => account,
            };
            let title = h_flex()
                .gap(px(10.))
                .child(ui::agent_logo(&a.agent, px(22.), cx))
                .child(v_flex().child(div().text_size(px(14.)).font_medium().child(a.name.clone())).child(
                    div().text_size(px(12.5)).text_color(if needs_login { palette::amber(cx) } else { muted }).child(account),
                ));
            let sign_in = setup.filter(|_| needs_login).map(|s| {
                let cmd = s.login.to_string();
                Button::new(SharedString::from(format!("login-{key}"))).small().outline().label("Sign in").on_click(cx.listener(move |this, _, _, cx| {
                    let cmd = cmd.clone();
                    this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command: cmd, cwd: None }))
                }))
            });
            let more = setup.map(|s| {
                let (login, url) = (s.login.to_string(), s.account_url);
                let ws = self.workspace.clone();
                Button::new(SharedString::from(format!("more-{key}")))
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::Ellipsis).text_color(muted))
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let (login, ws) = (login.clone(), ws.clone());
                        menu.min_w(px(180.))
                            .item(PopupMenuItem::new("Switch account…").on_click(move |_, _, cx| {
                                let cmd = login.clone();
                                ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command: cmd, cwd: None }))
                            }))
                            .item(PopupMenuItem::new("Manage plan").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(url)))
                    })
            });
            let more = match more {
                Some(b) => Some(b.into_any_element()),
                None => self.added_menu(&a.agent, cx),
            };
            let k2 = key.clone();
            let toggle = Switch::new(SharedString::from(format!("enable-{key}"))).checked(on).on_click(cx.listener(move |this, v: &bool, _, cx| {
                let (k, v) = (k2.clone(), *v);
                this.workspace.update(cx, |ws, cx| {
                    ws.settings.disabled_agents.retain(|d| *d != k);
                    if !v {
                        ws.settings.disabled_agents.push(k.clone());
                    }
                    ws.save_settings(cx);
                    // Disabled agents aren't probed; find out what one offers once it's back on.
                    if v {
                        ws.probe_acp_agent(&k, cx);
                    }
                });
            }));
            let controls = h_flex().gap(px(6.)).children(sign_in).children(more).child(div().pl(px(4.)).child(toggle));
            rows.push(Self::row(title, "", controls, cx));
        }
        // Updates to install come first; otherwise the setting waits under the agents.
        let first = self.workspace.read(cx).agent_updates.pending() > 0 && self.workspace.read(cx).settings.updates.check_agents;
        let updates = self.agent_updates_section(first, cx);
        // Nothing installed: the guidance still shows, under it the CLIs a scan can install.
        let none = rows.is_empty();
        let empty = Self::note(
            &if detecting {
                format!("Looking for agents on {}…", crate::words::words().this_computer)
            } else {
                "No agents found. Install an agent's CLI — Claude Code, Codex or another — and press Scan again; it joins with the login it already has. Or add one from the ACP Registry.".to_string()
            },
            cx,
        );
        if first {
            out.extend(updates);
            if none {
                out.push(empty);
            } else {
                out.push(Self::heading("Installed", cx));
                out.push(ui::group(rows, cx));
            }
        } else {
            if none {
                out.push(empty);
            } else {
                out.push(ui::group(rows, cx));
            }
            out.extend(updates);
        }
        if !missing.is_empty() {
            out.push(Self::heading("Not installed", cx));
            let rows = missing
                .into_iter()
                .map(|a| {
                    let key = a.agent.key();
                    let install = trek_core::catalog::agent_setup(&setup_key(&a.agent)).map(|s| s.install.to_string()).or(a.install_hint.clone()).unwrap_or_default();
                    let title = h_flex().gap(px(10.)).child(ui::agent_logo(&a.agent, px(22.), cx)).child(div().text_size(px(14.)).font_medium().child(a.name.clone()));
                    // One the user added: what's missing, and its menu (update, remove).
                    if let Some(menu) = self.added_menu(&a.agent, cx) {
                        return Self::row(title, install, menu, cx);
                    }
                    let cmd = install.clone();
                    let button = Button::new(SharedString::from(format!("install-{key}"))).small().outline().icon(IconName::ArrowDown).label("Install").on_click(
                        cx.listener(move |this, _, _, cx| {
                            let cmd = cmd.clone();
                            this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command: cmd, cwd: None }))
                        }),
                    );
                    Self::row(title, install, button, cx)
                })
                .collect();
            out.push(ui::group(rows, cx));
        }
        out.push(
            h_flex()
                .pt_3()
                .gap_2()
                .child(Button::new("add-agent").small().outline().icon(IconName::Plus).label("Add agent…").on_click(cx.listener(|this, _, window, cx| {
                    crate::add_agent::open(this.workspace.clone(), crate::add_agent::Tab::Registry, window, cx)
                })))
                .child(
                    Button::new("rescan")
                        .small()
                        .outline()
                        .loading(detecting)
                        .icon(IconName::RefreshCw)
                        .label("Scan again")
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.detect_agents(cx)))),
                )
                .into_any_element(),
        );
        out
    }

    /// The ⋯ menu of an agent the user added: the registry's newer version, its website, and
    /// Remove (which asks first). `None` for built-in agents.
    fn added_menu(&self, agent: &AgentId, cx: &App) -> Option<AnyElement> {
        let AgentId::Acp(id) = agent else { return None };
        let added = self.workspace.read(cx).settings.added_agents.iter().find(|a| a.id == *id).cloned()?;
        let update = self.workspace.read(cx).registry_update(id);
        let muted = cx.theme().muted_foreground;
        let ws = self.workspace.clone();
        let menu = Button::new(SharedString::from(format!("more-{}", agent.key())))
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis).text_color(muted))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = menu.min_w(px(180.));
                if let Some(v) = update.clone() {
                    let (ws, id) = (ws.clone(), added.id.clone());
                    menu = menu.item(PopupMenuItem::new(format!("Update to {v}")).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.install_registry_agent(&id, cx))));
                }
                if let Some(url) = added.website.clone() {
                    menu = menu.item(PopupMenuItem::new("Website").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&url)));
                }
                let (ws, added) = (ws.clone(), added.clone());
                menu.item(PopupMenuItem::new("Remove…").on_click(move |_, window, cx| confirm_remove(ws.clone(), &added, window, cx)))
            });
        Some(menu.into_any_element())
    }

    /// Agent CLI updates: the setting, when Trek last looked, and each update out.
    /// `first`: it opens the page, right under the blurb's own space.
    fn agent_updates_section(&mut self, first: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ws = self.workspace.read(cx);
        let (on, u) = (ws.settings.updates.check_agents, &ws.agent_updates);
        let (checking, checked_at, listed, missed) = (u.checking, u.checked_at, !u.listed().is_empty(), u.unchecked_note());
        // "Up to date" only when every agent was compared: an offline check proves nothing.
        let last = match (checking, checked_at, missed) {
            (true, ..) => "Checking now…".to_string(),
            (false, 0, _) => "Not checked yet.".to_string(),
            (false, at, Some(missed)) => format!("Checked {}. {missed}", relative_ago(at)),
            (false, at, None) if !listed => format!("Checked {}: every agent is up to date.", relative_ago(at)),
            (false, at, None) => format!("Checked {}.", relative_ago(at)),
        };
        let description = format!(
            "At launch and every 12 hours, Trek checks each agent CLI against where it came from: npm, {} or its maker. Updates wait for running turns to end. {last}",
            crate::words::words().package_manager
        );
        let controls = h_flex()
            .gap(px(10.))
            .child(
                Button::new("check-agent-updates")
                    .small()
                    .outline()
                    .loading(checking)
                    .label("Check now")
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.check_agent_updates(true, cx)))),
            )
            .child(self.switch("agent-updates-auto", on, |s, v| s.updates.check_agents = v));
        let mut rows = vec![Self::row("Check for agent updates", description, controls, cx)];
        rows.extend(crate::agent_updates::rows(&self.workspace, cx));
        let heading = if first {
            div().pb(px(10.)).text_size(px(13.)).font_semibold().child("Updates").into_any_element()
        } else {
            Self::heading("Updates", cx)
        };
        let mut out = vec![heading, ui::group(rows, cx)];
        if listed {
            out.push(
                h_flex()
                    .pt_3()
                    .gap(px(12.))
                    .child(div().flex_1().text_size(px(12.5)).text_color(cx.theme().muted_foreground).child(crate::agent_updates::WHY))
                    .children(crate::agent_updates::update_all(&self.workspace, cx))
                    .into_any_element(),
            );
        }
        out
    }

    /// Add what's in the add row: a command line (with any `KEY=value` before it), a remote
    /// server's URL with an optional header, or a pasted JSON config or `claude mcp add` line,
    /// which may hold several servers. Tokens go to the Keychain, and each new server is checked.
    /// While a server is being edited, it's replaced instead.
    fn add_mcp_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use trek_core::settings::McpHeader;
        let name = self.mcp_name.read(cx).value().trim().to_string();
        let text = self.mcp_command.read(cx).value().to_string();
        let mut servers = match trek_core::mcp::read_servers(&name, &text) {
            Ok(servers) => servers,
            Err(e) => return crate::toast::push(window, SharedString::from(e), cx),
        };
        let header = self.mcp_header_name.read(cx).value().trim().trim_end_matches(':').to_string();
        let value = self.mcp_header_value.read(cx).value().trim().to_string();
        if let [server] = &mut servers[..]
            && server.is_http()
            && !header.is_empty()
            && !value.is_empty()
        {
            if header.contains(char::is_whitespace) {
                return crate::toast::push(window, "A header name has no spaces, e.g. Authorization.", cx);
            }
            server.headers.retain(|h| !h.name.eq_ignore_ascii_case(&header));
            server.headers.push(McpHeader { name: header, value, secret: false });
        }
        let existing = self.workspace.read(cx).settings.tools.mcp_servers.clone();
        let mut notes: Vec<String> = vec![];
        for s in &servers {
            let stand_ins = trek_core::mcp::placeholders(s);
            if !stand_ins.is_empty() {
                notes.push(format!("{}: {} still holds the README's stand-in, not a real value. Edit the server to put yours in.", s.name, stand_ins.join(", ")));
            }
        }
        let mut added = vec![];
        if let Some(was) = self.mcp_editing.clone() {
            let Some(at) = existing.iter().position(|m| m.name == was) else {
                self.mcp_editing = None;
                return crate::toast::push(window, SharedString::from(format!("{was} was removed meanwhile; nothing was saved.")), cx);
            };
            if servers.len() != 1 {
                return crate::toast::push(window, SharedString::from(format!("That's {} servers. Edit one at a time, or cancel and add them.", servers.len())), cx);
            }
            let mut server = servers.remove(0);
            let old = &existing[at];
            if server.name != was && existing.iter().any(|m| m.name == server.name) {
                return crate::toast::push(window, SharedString::from(format!("There's already a server named {}.", server.name)), cx);
            }
            server.enabled = old.enabled;
            // A header not typed again stays as it was (its token in the Keychain).
            if server.is_http() && old.is_http() {
                for h in &old.headers {
                    if !server.headers.iter().any(|n| n.name.eq_ignore_ascii_case(&h.name)) {
                        server.headers.push(h.clone());
                    }
                }
            }
            if let Err(e) = server.stash_secrets(Some(&was)) {
                return crate::toast::push(window, SharedString::from(format!("Not saved: {e}.")), cx);
            }
            old.forget_secrets(Some(&server));
            let checked = server.name.clone();
            self.workspace.update(cx, |ws, cx| {
                ws.mcp_checks.remove(&was);
                if let Some(slot) = ws.settings.tools.mcp_servers.get_mut(at) {
                    *slot = server;
                }
                ws.save_settings(cx);
            });
            added.push(checked);
        } else {
            let mut taken: HashSet<String> = existing.iter().map(|m| m.name.clone()).collect();
            let mut skipped = vec![];
            for mut server in servers {
                if !taken.insert(server.name.clone()) {
                    skipped.push(server.name);
                    continue;
                }
                if let Err(e) = server.stash_secrets(None) {
                    notes.push(format!("{} wasn't added: its tokens couldn't be saved to {} ({e}).", server.name, crate::words::words().your_credential_store));
                    continue;
                }
                added.push(server.name.clone());
                self.workspace.update(cx, |ws, _| ws.settings.tools.mcp_servers.push(server));
            }
            if !skipped.is_empty() {
                let why = if skipped.len() == 1 { "there's already a server by that name" } else { "there are already servers by those names" };
                notes.insert(0, format!("Skipped {}: {why}. Edit or remove it to change it.", skipped.join(", ")));
            }
            if added.len() > 1 {
                notes.insert(0, format!("Added {} servers: {}.", added.len(), added.join(", ")));
            }
            self.workspace.update(cx, |ws, cx| ws.save_settings(cx));
        }
        for note in notes {
            crate::toast::push(window, SharedString::from(note), cx);
        }
        if added.is_empty() {
            return;
        }
        self.mcp_editing = None;
        for input in [&self.mcp_name, &self.mcp_command, &self.mcp_header_name, &self.mcp_header_value] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        // Each one is tried straight away, so a missing token or a typo shows now, not mid-task.
        self.workspace.update(cx, |ws, cx| {
            for name in &added {
                ws.check_mcp_server(name, cx);
            }
        });
    }

    /// Load server `name` into the add row to be edited: its command line with its environment
    /// in front (saved values shown as …, kept unless typed over), or its URL.
    fn edit_mcp_server(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.workspace.read(cx).settings.tools.mcp_servers.iter().find(|m| m.name == name).cloned() else { return };
        self.mcp_editing = Some(server.name.clone());
        self.mcp_name.update(cx, |s, cx| s.set_value(server.name.clone(), window, cx));
        self.mcp_command.update(cx, |s, cx| s.set_value(server.command_line(), window, cx));
        for input in [&self.mcp_header_name, &self.mcp_header_value] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    fn cancel_mcp_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mcp_editing = None;
        for input in [&self.mcp_name, &self.mcp_command, &self.mcp_header_name, &self.mcp_header_value] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    fn tools_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.workspace.read(cx).settings.clone();
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let mut out = vec![];
        // Read off the main thread when the page opens; until then each check says so.
        let probe = self.tools.value.clone();
        let checking = |id: &str| div().id(SharedString::from(format!("{id}-checking"))).test_support().text_size(px(12.5)).text_color(muted).child("Checking…").into_any_element();
        let bundled = probe.as_ref().is_none_or(|p| p.mcp_bundled);
        let permission = |id: &'static str, label: &'static str, ok: Option<bool>, pane: &'static str, cx: &mut Context<Self>| -> AnyElement {
            match ok {
                None => checking(id),
                Some(true) => Self::status_dot(palette::emerald(cx), "Allowed"),
                Some(false) => Button::new(id).small().outline().label(format!("Allow {label}")).on_click(move |_, _, cx| cx.open_url(pane)).into_any_element(),
            }
        };
        let ax = probe.as_ref().map(|p| p.accessibility);
        let sr = probe.as_ref().map(|p| p.screen_recording);
        out.extend(self.connections(cx));
        out.push(Self::heading("Computer use", cx));
        out.push(ui::group(
            vec![
                Self::row(
                    "Computer use tools",
                    "Off until you turn it on. Agents can then see the screen, click, type and switch apps through Trek's MCP tools, following your hand-holding level.",
                    self.switch("computer-use", s.tools.computer_use, |s, v| s.tools.computer_use = v),
                    cx,
                ),
                Self::row("Accessibility", "Lets Trek click and type for the agent.", permission("ax-perm", "Accessibility", ax, crate::integrations::ACCESSIBILITY_PANE, cx), cx),
                Self::row(
                    "Screen Recording",
                    "Lets Trek take screenshots of other apps.",
                    permission("sr-perm", "Screen Recording", sr, crate::integrations::SCREEN_RECORDING_PANE, cx),
                    cx,
                ),
            ],
            cx,
        ));
        out.push(Self::heading("iOS Simulator", cx));
        if crate::words::is_windows() {
            // Xcode's tools are all it has to run on: nothing to switch, install or allow.
            out.push(Self::note(SIMULATOR_NEEDS_A_MAC, cx));
        } else {
            let axe_control: AnyElement = match probe.as_ref().map(|p| p.axe) {
                None => checking("install-axe"),
                Some(true) => Self::status_dot(palette::emerald(cx), "Installed"),
                Some(false) => Button::new("install-axe")
                    .small()
                    .outline()
                    .icon(IconName::ArrowDown)
                    .label("Install AXe")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command: crate::integrations::AXE_INSTALL.into(), cwd: None }))
                    }))
                    .into_any_element(),
            };
            out.push(ui::group(
                vec![
                    Self::row(
                        "Simulator tools",
                        "Agents can boot simulators, install and launch your app, take screenshots and tap through it.",
                        self.switch("sim-tools", s.tools.simulator, |s, v| s.tools.simulator = v),
                        cx,
                    ),
                    Self::row("Touch input (AXe)", "Taps, swipes and typing in the simulator, for you and for agents.", axe_control, cx),
                ],
                cx,
            ));
        }
        out.push(Self::heading("Sub-agents", cx));
        out.push(ui::group(
            vec![Self::row(
                "Sub-agent tools",
                "Agents can hand work to other agents and models you have in Trek, to review or to build, and get their answers back. Each shows inline in its thread with its logo; open it to follow along.",
                self.switch("orchestration", s.tools.orchestration, |s, v| s.tools.orchestration = v),
                cx,
            )],
            cx,
        ));
        if !bundled {
            out.push(Self::note("Trek's MCP server (trek-mcp) isn't next to this build. Run cargo build -p trek-mcp, or use the bundled app.", cx));
        }

        out.push(Self::heading("Your MCP servers", cx));
        out.push(Self::note(
            "Passed to every new session, on top of what each agent already loads. Type a command Trek starts or a remote server's URL, or paste the JSON config or claude mcp add line from a server's README.",
            cx,
        ));
        let checks = self.workspace.read(cx).mcp_checks.clone();
        let editing = self.mcp_editing.clone();
        let mut rows: Vec<AnyElement> = s
            .tools
            .mcp_servers
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = m.name.clone();
                let controls = h_flex()
                    .gap_1()
                    .child(ui::icon_button(SharedString::from(format!("mcp-check-{i}")), IconName::RefreshCw, "Check it works").on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.check_mcp_server(&name, cx))
                    })))
                    .child(ui::icon_button(SharedString::from(format!("mcp-edit-{i}")), crate::assets::Lucide::Pencil, "Edit").on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, window, cx| this.edit_mcp_server(&name, window, cx)
                    })))
                    .child(ui::icon_button(SharedString::from(format!("mcp-del-{i}")), IconName::Delete, "Remove").on_click(cx.listener(move |this, _, _, cx| {
                        if this.mcp_editing.as_deref() == Some(name.as_str()) {
                            this.mcp_editing = None;
                        }
                        this.workspace.update(cx, |ws, cx| {
                            if i < ws.settings.tools.mcp_servers.len() {
                                let gone = ws.settings.tools.mcp_servers.remove(i);
                                // Its tokens go with it.
                                gone.forget_secrets(None);
                                ws.mcp_checks.remove(&gone.name);
                            }
                            ws.save_settings(cx);
                        })
                    })))
                    .child(div().pl_2().child(Switch::new(SharedString::from(format!("mcp-on-{i}"))).checked(m.enabled).on_click(cx.listener(move |this, v: &bool, _, cx| {
                        let v = *v;
                        this.workspace.update(cx, |ws, cx| {
                            if let Some(m) = ws.settings.tools.mcp_servers.get_mut(i) {
                                m.enabled = v;
                            }
                            ws.save_settings(cx);
                        })
                    }))));
                let transport = if m.is_http() { "Remote" } else { "Command" };
                let title = h_flex()
                    .gap(px(8.))
                    .child(m.name.clone())
                    .child(div().id(SharedString::from(format!("mcp-transport-{i}"))).test_support().px(px(6.)).rounded_full().bg(cx.theme().foreground.opacity(0.07)).text_size(px(11.)).font_normal().text_color(muted).child(transport))
                    .when(editing.as_deref() == Some(m.name.as_str()), |el| el.child(div().text_size(px(11.)).font_normal().text_color(muted).child("Editing below")));
                let mut line = m.summary();
                let headers: Vec<&str> = m.headers.iter().map(|h| h.name.as_str()).collect();
                if !headers.is_empty() {
                    line = format!("{line} · sends {}", headers.join(", "));
                }
                let env: Vec<&str> = m.env.iter().map(|e| e.name.as_str()).collect();
                if !env.is_empty() {
                    line = format!("{line} · with {}", env.join(", "));
                }
                let status = checks.get(&m.name).map(|c| Self::mcp_status(i, c, cx));
                Self::row_with(title, line, status, controls, cx)
            })
            .collect();
        let remote = trek_core::settings::is_mcp_url(&self.mcp_command.read(cx).value());
        let add_label = if editing.is_some() { "Save" } else { "Add" };
        rows.push(
            v_flex()
                .w_full()
                .py(px(12.))
                .gap_2()
                .when_some(editing.clone(), |el, name| {
                    el.child(
                        div()
                            .id("mcp-editing")
                            .test_support()
                            .text_size(px(12.5))
                            .text_color(muted)
                            .child(format!("Editing {name}. A value shown as … stays as saved unless you type over it.")),
                    )
                })
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(div().w(px(150.)).child(Input::new(&self.mcp_name).id("mcp-name").small()))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.mcp_command).id("mcp-command").small()))
                        .when(editing.is_some(), |el| {
                            el.child(Button::new("mcp-cancel").small().ghost().label("Cancel").on_click(cx.listener(|this, _, window, cx| this.cancel_mcp_edit(window, cx))))
                        })
                        .child(
                            Button::new("mcp-add")
                                .small()
                                .outline()
                                .when(editing.is_none(), |b| b.icon(IconName::Plus))
                                .label(add_label)
                                .on_click(cx.listener(|this, _, window, cx| this.add_mcp_server(window, cx))),
                        ),
                )
                // A remote server may want a token: its header goes along, its value to the Keychain.
                .when(remote, |el| {
                    el.child(
                        h_flex()
                            .id("mcp-header-row")
                            .test_support()
                            .w_full()
                            .gap_2()
                            .child(div().w(px(150.)).child(Input::new(&self.mcp_header_name).small()))
                            .child(div().flex_1().min_w_0().child(Input::new(&self.mcp_header_value).small()))
                            // Lines the fields up with the ones above.
                            .when(editing.is_some(), |el| el.child(div().invisible().child(Button::new("mcp-cancel-spacer").small().ghost().label("Cancel"))))
                            .child(div().invisible().child(Button::new("mcp-add-spacer").small().outline().when(editing.is_none(), |b| b.icon(IconName::Plus)).label(add_label))),
                    )
                })
                .into_any_element(),
        );
        out.push(ui::group(rows, cx));

        out.push(Self::heading("Already set up in your agents", cx));
        let ws = self.workspace.read(cx);
        let mut rows = vec![];
        if probe.is_none() {
            rows.push(div().id("tools-reading").test_support().py(px(16.)).text_size(px(12.5)).text_color(muted).child("Reading your agents' setup…").into_any_element());
        }
        for (agent, names) in probe.as_ref().map(|p| p.agent_mcp.clone()).unwrap_or_default() {
            rows.push(Self::row(
                format!("{agent} MCP servers"),
                if names.is_empty() { "None".to_string() } else { names.join(", ") },
                div().text_xs().text_color(muted).child(names.len().to_string()),
                cx,
            ));
        }
        for agent in [AgentId::ClaudeCode, AgentId::Codex] {
            if let Some(st) = ws.agent_status.get(&agent.key()) {
                let skills: Vec<String> = st.commands.iter().filter(|c| c.kind == trek_agents::CommandKind::Skill).map(|c| c.name.clone()).collect();
                if !skills.is_empty() {
                    rows.push(Self::row(
                        format!("{} skills", agent.display_name()),
                        format!("Type $ in the composer to use one. {}", skills.iter().take(12).cloned().collect::<Vec<_>>().join(", ")),
                        div().text_xs().text_color(muted).child(skills.len().to_string()),
                        cx,
                    ));
                }
            }
        }
        if let Some(plugins) = probe.map(|p| p.plugins) {
            rows.push(Self::row(
                "Claude Code plugins",
                if plugins.is_empty() { "None installed".to_string() } else { plugins.join(", ") },
                div().font_family(mono).text_xs().text_color(muted).child(plugins.len().to_string()),
                cx,
            ));
        }
        out.push(ui::group(rows, cx));
        out.push(
            h_flex()
                .pt_3()
                .child(
                    Button::new("tools-refresh")
                        .small()
                        .outline()
                        .loading(self.tools.loading())
                        .icon(IconName::RefreshCw)
                        .label("Check again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tools.invalidate();
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        out
    }

    fn content(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let page = self.page(cx);
        self.probe_page(page, cx);
        let s = self.workspace.read(cx).settings.clone();
        let muted = cx.theme().muted_foreground;
        let mut out: Vec<AnyElement> = vec![];
        match page {
            SettingsPage::Project => out.extend(self.project_page(cx)),
            SettingsPage::General => out.extend(self.general_page(&s, cx)),
            SettingsPage::Appearance => out.extend(self.appearance_page(&s, cx)),
            SettingsPage::Notifications => out.extend(self.notifications_page(&s, cx)),
            SettingsPage::Shortcuts => out.extend(self.shortcuts_page(cx)),
            SettingsPage::Agents => out.extend(self.agents_page(cx)),
            SettingsPage::Tools => out.extend(self.tools_page(cx)),
            SettingsPage::Mobile => out.extend(self.mobile_page(&s, cx)),
            SettingsPage::Skills => out.extend(self.skills_page(cx)),
            SettingsPage::Snapshots => out.extend(self.snapshots_page(&s, cx)),
            SettingsPage::ApiKeys => out.extend(self.api_keys_page(cx)),
            SettingsPage::LocalModels => {
                let ws = self.workspace.read(cx);
                let detecting = ws.detecting;
                let locals: Vec<_> = ws.agents.iter().filter(|a| matches!(a.agent, AgentId::Direct(_))).cloned().collect();
                // Nothing up yet: say how servers get here, under it the ones found offline.
                let up = locals.iter().any(|a| a.availability == Availability::Ready);
                if !up {
                    out.push(Self::note(
                        if detecting {
                            "Looking for model servers…"
                        } else {
                            "No model servers running. Ollama, LM Studio, and llama.cpp or MLX show up here on their own once they're running on their usual ports — there's nothing to set up."
                        },
                        cx,
                    ));
                }
                if !locals.is_empty() {
                    let rows = locals
                        .into_iter()
                        .map(|a| {
                            let (status, detail) = match a.availability {
                                Availability::Ready if a.models.is_empty() => {
                                    (Self::status_dot(palette::amber(cx), "No models"), "Running. Pull a model, e.g. ollama pull qwen3-coder".to_string())
                                }
                                Availability::Ready => (Self::status_dot(palette::emerald(cx), "Ready"), a.models.join(", ")),
                                _ => (Self::status_dot(muted.opacity(0.5), "Not running"), String::new()),
                            };
                            let title = h_flex().gap(px(10.)).child(ui::agent_logo(&a.agent, px(18.), cx)).child(div().text_size(px(13.5)).font_medium().child(a.name.clone()));
                            Self::row(title, detail, status, cx)
                        })
                        .collect();
                    out.push(ui::group(rows, cx));
                }
                if !up {
                    out.push(
                        h_flex()
                            .child(
                                Button::new("rescan-local")
                                    .small()
                                    .outline()
                                    .loading(detecting)
                                    .icon(IconName::RefreshCw)
                                    .label("Scan again")
                                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.detect_agents(cx)))),
                            )
                            .into_any_element(),
                    );
                }
            }
            SettingsPage::Permissions => out.extend(self.permissions_page(&s, cx)),
            SettingsPage::Import => {
                let ws = self.workspace.read(cx);
                let importing = ws.importing;
                let summary = ws.import_summary.clone();
                out.push(ui::group(
                    vec![
                        Self::row(h_flex().gap(px(10.)).child(ui::agent_logo(&AgentId::ClaudeCode, px(18.), cx)).child(div().text_size(px(13.5)).font_medium().child("Claude Code")), "~/.claude/projects", self.switch("imp-claude", s.import.claude_code, |s, v| s.import.claude_code = v), cx),
                        Self::row(h_flex().gap(px(10.)).child(ui::agent_logo(&AgentId::Codex, px(18.), cx)).child(div().text_size(px(13.5)).font_medium().child("Codex")), "~/.codex", self.switch("imp-codex", s.import.codex, |s, v| s.import.codex = v), cx),
                        Self::row(h_flex().gap(px(10.)).child(ui::agent_logo(&AgentId::OpenCode, px(18.), cx)).child(div().text_size(px(13.5)).font_medium().child("OpenCode")), "~/.local/share/opencode", self.switch("imp-opencode", s.import.opencode, |s, v| s.import.opencode = v), cx),
                        Self::row(
                            "How far back",
                            "",
                            ui::segmented(
                                "max-age",
                                vec![(30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "All")],
                                s.import.max_age_days,
                                self.setter(|s, v| s.import.max_age_days = v),
                                cx,
                            ),
                            cx,
                        ),
                    ],
                    cx,
                ));
                out.push(
                    h_flex()
                        .pt_3()
                        .gap_3()
                        .child(
                            Button::new("import-now")
                                .small()
                                .outline()
                                .loading(importing)
                                .label("Import now")
                                .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.import_threads(cx)))),
                        )
                        .when_some(summary, |el, s| {
                            let mut line = format!("{} Claude Code · {} Codex · {} OpenCode", s.claude_code, s.codex, s.opencode);
                            // Title generators, test runs and sub-agents never become threads.
                            match s.left_out.len() {
                                0 => {}
                                1 => line.push_str(" · 1 helper session left out"),
                                n => line.push_str(&format!(" · {n} helper sessions left out")),
                            }
                            el.child(div().text_sm().text_color(muted).child(line))
                        })
                        .into_any_element(),
                );
                out.extend(self.left_out_section(cx));
            }
            SettingsPage::Updates => out.extend(self.updates_page(&s, cx)),
            SettingsPage::About => out.extend(self.about_page(&s, cx)),
        }
        out
    }
}

/// Ask before removing an added agent: its download goes, and its threads can't continue.
fn confirm_remove(ws: Entity<Workspace>, agent: &trek_core::registry::AddedAgent, window: &mut Window, cx: &mut App) {
    let (id, name) = (agent.id.clone(), agent.name.clone());
    let what = match agent.source {
        trek_core::registry::AgentSource::Registry if agent.distribution.as_deref() == Some("binary") => "Its download is deleted. ",
        _ => "",
    };
    let body = format!("{what}Its threads stay in the sidebar, and continue once it's added again.");
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (ws, id) = (ws.clone(), id.clone());
        use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
        alert.title(format!("Remove {name}?")).description(body.clone()).footer(
            DialogFooter::new().child(DialogClose::new().child(Button::new("remove-agent-cancel").outline().label("Cancel"))).child(DialogAction::new().child(
                Button::new("remove-agent-ok").with_variant(gpui_kit::component::button::ButtonVariant::Danger).label("Remove").on_click(move |_, _, cx| {
                    ws.update(cx, |ws, cx| ws.remove_added_agent(&id, cx))
                }),
            )),
        )
    });
}

/// When something happened, in a sentence: "just now", "3h ago", "on Sep 12".
fn relative_ago(ms: i64) -> String {
    match crate::time::relative(ms) {
        r if r == "now" => "just now".into(),
        r if r.ends_with(|c: char| c.is_ascii_alphabetic()) && r.starts_with(|c: char| c.is_ascii_digit()) => format!("{r} ago"),
        r => format!("on {r}"),
    }
}

/// How Settings names and explains each kind of session the import leaves out.
fn left_out_rule(rule: Skip) -> (&'static str, &'static str) {
    match rule {
        Skip::Trek => ("Started by Trek", "Already in the sidebar as Trek's own threads."),
        Skip::Subagent => ("Sub-agents", "Work a sub-agent did for another thread."),
        Skip::UntouchedFork => ("Untouched forks", "Copies of a session nothing was added to. The original is imported."),
        Skip::TitleGenerator => ("Title generators", "Other apps asking an agent to name one of their threads."),
        Skip::TempDir => ("Temp folder runs", "Started by a program, or a single prompt, in a system temp folder."),
        Skip::OneShotRun => ("One-shot runs", "A single prompt run from the command line, with codex exec or opencode run."),
        Skip::NoUserMessage => ("Empty sessions", "Nothing typed and nothing answered, or only commands like /clear."),
    }
}

impl SettingsView {
    /// Sessions the last import left out, grouped by rule, each one can be brought back.
    fn left_out_section(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(summary) = self.workspace.read(cx).import_summary.clone() else { return vec![] };
        if summary.left_out.is_empty() {
            return vec![];
        }
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_active;
        let mut rows = vec![];
        for rule in Skip::ALL {
            let sessions: Vec<&ImportedThread> = summary.left_out.iter().filter(|t| t.skip == Some(rule)).collect();
            if sessions.is_empty() {
                continue;
            }
            let open = self.left_out_open.contains(&rule);
            let (title, about) = left_out_rule(rule);
            let toggle = h_flex()
                .id(SharedString::from(format!("left-out-{}", rule.key())))
                .gap(px(6.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .text_size(px(12.5))
                .text_color(muted)
                .child(sessions.len().to_string())
                .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.left_out_open.remove(&rule) {
                        this.left_out_open.insert(rule);
                    }
                    cx.notify();
                }));
            rows.push(Self::row(title, about, toggle, cx));
            if open {
                rows.extend(sessions.into_iter().map(|t| self.left_out_row(t, cx)));
            }
        }
        vec![
            Self::heading("Left out", cx),
            Self::note("Sessions that aren't your conversations don't become threads. If one was misjudged, bring it back; imports leave it in the sidebar from then on.", cx),
            ui::group(rows, cx),
        ]
    }

    fn left_out_row(&self, t: &ImportedThread, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let place = t.cwd.as_deref().map(trek_core::paths::tildify).unwrap_or_default();
        // Long temp-folder paths are cut; when it happened stays readable.
        let meta = h_flex()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(muted)
            .when(!place.is_empty(), |el| el.child(div().min_w_0().truncate().child(place)).child("·"))
            .child(div().flex_none().child(crate::time::relative(t.updated_at)));
        let session = t.clone();
        h_flex()
            .w_full()
            .min_h(px(44.))
            .gap(px(12.))
            .py(px(8.))
            .pl(px(12.))
            .child(ui::agent_logo(&t.source.agent().unwrap_or(AgentId::ClaudeCode), px(14.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).truncate().child(t.title.clone()))
                    .child(meta),
            )
            .child(
                Button::new(SharedString::from(format!("left-out-show-{}-{}", t.source.key(), t.native_id)))
                    .small()
                    .outline()
                    .label("Show in sidebar")
                    .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.show_left_out(&session, cx)))),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page(cx);
        if page == SettingsPage::Project {
            self.sync_project_name(window, cx);
        }
        let body = self.content(cx);
        div().id("settings-content").size_full().overflow_y_scroll().child(
            h_flex().w_full().justify_center().child(
                v_flex()
                    .w_full()
                    .max_w(px(720.))
                    .px(px(48.))
                    .pt(px(44.))
                    .pb(px(80.))
                    .child(div().text_size(px(20.)).font_semibold().child(page.label()))
                    .when(!page_blurb(page).is_empty(), |el| {
                        el.child(div().pt(px(6.)).max_w(px(560.)).text_size(px(13.)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground).child(page_blurb(page).into_owned()))
                    })
                    .child(div().h(px(28.)))
                    .children(body),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{SettingsPage, page_named};

    #[test]
    fn the_legal_note_reads_as_one_paragraph() {
        assert!(!super::pages::LEGAL.contains("  ") && !super::pages::LEGAL.contains('\n'), "{}", super::pages::LEGAL);
    }

    #[test]
    fn pages_resolve_by_label() {
        assert_eq!(page_named("updates"), Some(SettingsPage::Updates));
        assert_eq!(page_named(" API-Keys "), Some(SettingsPage::ApiKeys));
        assert_eq!(page_named("agents-&-subscriptions"), Some(SettingsPage::Agents));
        assert_eq!(page_named("nowhere"), None);
    }
}
