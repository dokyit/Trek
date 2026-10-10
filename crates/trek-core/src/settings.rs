//! User settings, persisted as TOML. API keys live in the OS keychain, never in this file.

use crate::types::{Effort, HandHolding};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub appearance: Appearance,
    pub inbox: Inbox,
    pub permissions: Permissions,
    pub import: Import,
    pub updates: Updates,
    pub notifications: Notifications,
    pub onboarding: Onboarding,
    /// Providers the user enabled for direct API use (keys are in the keychain).
    pub api_providers: Vec<String>,
    /// Custom OpenAI-compatible endpoints.
    pub custom_endpoints: Vec<CustomEndpoint>,
    /// ACP agents the user added: from the ACP Registry, or a command of their own.
    pub added_agents: Vec<crate::registry::AddedAgent>,
    /// Folders the user added explicitly (always shown as projects).
    pub user_projects: Vec<String>,
    /// Installed agents the user switched off (`AgentId::key()`).
    pub disabled_agents: Vec<String>,
    pub tools: Tools,
    pub snapshots: Snapshots,
    pub layout: Layout,
    /// Per-project preferences, keyed by the project's folder path.
    pub projects: std::collections::BTreeMap<String, ProjectPrefs>,
    /// Projects removed from Trek (paths); adding the folder again brings one back.
    pub hidden_projects: Vec<String>,
    /// IDE mode: folders and loose files it opened recently.
    pub ide: Ide,
    /// Trek on your iPhone (Settings › Phone).
    pub mobile: Mobile,
    /// The sidebar's Usage card.
    pub usage: Usage,
    /// The built-in terminal panel.
    pub terminal: Terminal,
    /// What `load` couldn't read, and whether `save` may write the file.
    #[serde(skip)]
    pub guard: SaveGuard,
}

/// Kept alongside loaded settings: values from the file this build couldn't read (written back
/// unchanged on save), and whether saving is refused because the file couldn't be read at all.
/// Never part of a comparison.
#[derive(Debug, Clone, Default)]
pub struct SaveGuard {
    /// Why saves are refused, if they are.
    blocked: Option<String>,
    /// Values dropped on load: their path, the file's raw value, and what the default in their
    /// place serialized as when loaded.
    kept: Vec<(Vec<String>, toml::Value, Option<toml::Value>)>,
}

impl PartialEq for SaveGuard {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl SaveGuard {
    /// Refuse every save from now on, for `why`.
    pub fn block(&mut self, why: impl Into<String>) {
        self.blocked = Some(why.into());
    }

    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }
}

/// What went wrong reading the settings file, for a toast at launch.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadProblem {
    /// A copy of the file as it was, if one could be made.
    pub backup: Option<std::path::PathBuf>,
    /// Settings that couldn't be read (dotted paths): back at their defaults, and their values
    /// written back unchanged unless the user changes them.
    pub dropped: Vec<String>,
    /// The file isn't TOML at all: Trek runs on defaults and won't save over it.
    pub unreadable: bool,
    pub error: String,
}

impl LoadProblem {
    pub fn message(&self) -> String {
        let copy = self.backup.as_ref().map(|b| format!(" A copy is at {}.", crate::paths::tildify(b))).unwrap_or_default();
        if self.unreadable {
            format!(
                "settings.toml couldn't be read ({}). Trek is using its defaults and won't save changes until the file is fixed or removed and Trek restarted.{copy}",
                self.error.lines().next().unwrap_or("not TOML").trim()
            )
        } else {
            let mut names = self.dropped.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            if self.dropped.len() > 4 {
                names.push_str(&format!(" and {} more", self.dropped.len() - 4));
            }
            format!("Some settings couldn't be read and use their defaults for now: {names}. They're kept in the file unless you change them.{copy}")
        }
    }
}

/// The phone server: off until the user turns it on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mobile {
    pub enabled: bool,
    pub port: u16,
    /// Which of this Mac's addresses the pairing code points phones at.
    pub reach: Reach,
    /// Notifications on the phone, through ntfy (its free app and server): a thread needs you,
    /// finished or failed.
    pub push: bool,
    /// The ntfy server (`https://ntfy.sh`, or one of your own).
    pub push_server: String,
    /// The topic notifications go to: a long random name, as anyone who knows it can read them.
    /// Made when push is first turned on.
    pub push_topic: String,
    pub push_when: PushWhen,
    /// Notifications name the thread and its project. Off, they say only that a thread needs
    /// you, finished or failed: the ntfy server (someone else's, unless it's your own) reads
    /// whatever is sent through it.
    pub push_names: bool,
    /// The phone may allow a kind of request for the rest of a session, not just the one asked.
    /// Off, only this Mac can: Trek can't check from here who is holding the phone.
    pub session_approvals: bool,
}

impl Default for Mobile {
    fn default() -> Self {
        Self { enabled: false, port: 7420, reach: Reach::Wifi, push: false, push_server: "https://ntfy.sh".into(), push_topic: String::new(), push_when: PushWhen::Away, push_names: false, session_approvals: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushWhen {
    /// Only while you're away from the Mac: no keyboard or mouse for two minutes, or the screen
    /// is locked.
    Away,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// The Mac's address on the local network: the phone on the same Wi-Fi.
    Wifi,
    /// The Mac's Tailscale address: the phone anywhere on the tailnet.
    Tailscale,
}

/// A command a project can run from the title bar (build, test, dev server…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectAction {
    pub name: String,
    pub command: String,
}

/// What new threads in a project start with, its icon, and its actions. `None` = Trek's default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectPrefs {
    /// `lucide:<name>` or `file:<path>`; `None` shows the two-letter monogram.
    pub icon: Option<String>,
    /// The project's colour, as a hue in degrees (0–359): its badge and folder icons. `None`
    /// picks one from the name.
    pub color: Option<u16>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub hand_holding: Option<HandHolding>,
    pub actions: Vec<ProjectAction>,
    /// Where new threads run (git projects).
    pub run_in: RunIn,
    /// Files and folders new worktrees get a copy of from the project folder (paths relative to
    /// it): ignored ones a fresh checkout lacks, such as `.env`.
    pub worktree_copy: Vec<String>,
    /// The project's verification skill, once it has one (`crate::verification`).
    pub verification: Option<Verification>,
    /// The skill Trek last recorded, while it's gone from the project folder (a branch without it
    /// checked out): when it comes back, so do the user's choices for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_away: Option<Verification>,
}

/// Where a project's verification skill lives and how it's kept up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verification {
    /// The skill's folder.
    pub skill: String,
    /// Its name, as agents know it.
    pub name: String,
    /// How to run its CLI from the project folder, as the skill says.
    pub cli: Option<String>,
    /// When it was last maintained (ms): a Maintain run Trek saw finish, or a commit that changed it.
    pub maintained_at: Option<i64>,
    /// Remind the user once it's a week old.
    pub remind_weekly: bool,
    /// When the last reminder was given (ms).
    pub reminded_at: Option<i64>,
}

impl Default for ProjectPrefs {
    fn default() -> Self {
        Self {
            icon: None,
            color: None,
            agent: None,
            model: None,
            effort: None,
            hand_holding: None,
            actions: vec![],
            run_in: RunIn::Local,
            worktree_copy: crate::worktree::default_copy(),
            verification: None,
            verification_away: None,
        }
    }
}

/// Where a thread runs: in the project folder, or in a worktree of its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunIn {
    #[default]
    Local,
    Worktree,
}

impl ProjectPrefs {
    pub fn is_empty(&self) -> bool {
        *self == ProjectPrefs::default()
    }
}

/// Window layout the user adjusted by hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Width of the right tools panel, in points.
    pub right_panel_width: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { right_panel_width: DEFAULT_RIGHT_PANEL_WIDTH }
    }
}

pub const DEFAULT_RIGHT_PANEL_WIDTH: f32 = 440.;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    Window,
    Area,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFormat {
    Png,
    Jpg,
}

/// App snapshots: screenshots taken from the composer (and ⌘⇧S) and attached to the message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshots {
    /// What ⌘⇧S captures.
    pub default_mode: SnapshotMode,
    /// Hide Trek while you pick, so it isn't in the shot.
    pub hide_trek: bool,
    /// Keep the window's drop shadow in window snapshots.
    pub window_shadow: bool,
    /// Play the camera shutter sound.
    pub sound: bool,
    pub format: SnapshotFormat,
    /// Delete snapshots older than this many days (0 = keep).
    pub keep_days: u32,
}

impl Default for Snapshots {
    fn default() -> Self {
        Self { default_mode: SnapshotMode::Window, hide_trek: true, window_shadow: false, sound: false, format: SnapshotFormat::Png, keep_days: 14 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tools {
    /// Give agents the computer-use MCP tools (screenshot, click, type).
    pub computer_use: bool,
    /// Give agents the iOS Simulator MCP tools.
    pub simulator: bool,
    /// Give agents Trek's sub-agent tools (`delegate_task` and friends): they can hand work to
    /// other agents and models, which Trek runs as child threads.
    pub orchestration: bool,
    /// Extra MCP servers passed to every session.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Give agents the Figma desktop app's Dev Mode MCP server (`FIGMA_DESKTOP_URL`), as
    /// `figma-desktop`. It answers only while Figma is open with the server turned on.
    pub figma_desktop: bool,
}

impl Default for Tools {
    fn default() -> Self {
        Self { computer_use: false, simulator: true, orchestration: true, mcp_servers: vec![], figma_desktop: false }
    }
}

/// The Figma desktop app's MCP server (Dev Mode): local, no login.
pub const FIGMA_DESKTOP_URL: &str = "http://127.0.0.1:3845/mcp";
/// The name Trek gives it in a session.
pub const FIGMA_DESKTOP_SERVER: &str = "figma-desktop";

/// One of the user's MCP servers: a command Trek starts (stdio), or a remote server at `url`
/// (streamable HTTP). Servers saved before HTTP was supported have only `command` and `args`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Sent with every request to `url`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<McpHeader>,
    /// The environment `command` is started with (tokens, mostly): names here, values in the
    /// Keychain (`secrets::mcp_env`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<McpEnvVar>,
    #[serde(default)]
    pub enabled: bool,
}

impl McpServerConfig {
    pub fn stdio(name: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self { name: name.into(), command: command.into(), args, url: None, headers: vec![], env: vec![], enabled: true }
    }

    pub fn http(name: impl Into<String>, url: impl Into<String>, headers: Vec<McpHeader>) -> Self {
        Self { name: name.into(), command: String::new(), args: vec![], url: Some(url.into()), headers, env: vec![], enabled: true }
    }

    /// What the add row was given, read as a server: an `http(s)://` URL is a remote server,
    /// anything else a command line, split as a shell would, with any `KEY=value` before the
    /// command as its environment. `None` when there's nothing to run or it doesn't split (an
    /// open quote). Values are inline; `stash_secrets` moves them to the Keychain.
    pub fn parse(name: &str, line: &str) -> Option<Self> {
        let (name, line) = (name.trim(), line.trim());
        if name.is_empty() || line.is_empty() {
            return None;
        }
        if is_mcp_url(line) {
            return Some(Self::http(name, line, vec![]));
        }
        let mut words = crate::mcp::split_words(line).ok()?.into_iter().peekable();
        // `env A=1 cmd` says the same as `A=1 cmd`.
        if words.peek().is_some_and(|w| w == "env") {
            words.next();
        }
        let mut env = vec![];
        while let Some((k, v)) = words.peek().and_then(|w| crate::mcp::assignment(w)) {
            env.push(McpEnvVar { name: k.to_string(), value: v.to_string(), secret: false });
            words.next();
        }
        let command = words.next()?;
        Some(Self { env, ..Self::stdio(name, command, words.collect()) })
    }

    pub fn is_http(&self) -> bool {
        self.url.is_some()
    }

    /// The command line, or the URL: what the server's row shows.
    pub fn summary(&self) -> String {
        match &self.url {
            Some(url) => url.clone(),
            None => std::iter::once(self.command.as_str()).chain(self.args.iter().map(String::as_str)).map(crate::mcp::quote).collect::<Vec<_>>().join(" "),
        }
    }

    /// What the add row shows when the server is edited: the URL, or the command line with its
    /// environment in front, a value kept in the Keychain as `KEPT` (left as it is, it stays).
    pub fn command_line(&self) -> String {
        let env = self.env.iter().map(|e| format!("{}={}", e.name, if e.secret { KEPT.to_string() } else { crate::mcp::quote(&e.value) }));
        env.chain(std::iter::once(self.summary())).collect::<Vec<_>>().join(" ")
    }

    /// Move every env and header value given inline to the Keychain, leaving its name here.
    /// `was` is the server's name before an edit: an env value left as `KEPT` is the one saved
    /// under it, and a renamed server's saved values move to the new name.
    pub fn stash_secrets(&mut self, was: Option<&str>) -> anyhow::Result<()> {
        let name = self.name.clone();
        let renamed = was.filter(|w| *w != name);
        for e in &mut self.env {
            if e.value == KEPT {
                e.value = was.and_then(|w| secrets::mcp_env(w, &e.name)).ok_or_else(|| anyhow::anyhow!("{} has no saved value to keep, so type it in", e.name))?;
                e.secret = false;
            } else if e.secret {
                if let Some(v) = renamed.and_then(|w| secrets::mcp_env(w, &e.name)) {
                    secrets::set_mcp_env(&name, &e.name, &v)?;
                }
                continue;
            }
            if !e.value.is_empty() {
                secrets::set_mcp_env(&name, &e.name, &e.value)?;
                (e.value, e.secret) = (String::new(), true);
            }
        }
        for h in &mut self.headers {
            if h.secret {
                if let Some(v) = renamed.and_then(|w| secrets::mcp_header(w, &h.name)) {
                    secrets::set_mcp_header(&name, &h.name, &v)?;
                }
            } else if !h.value.is_empty() {
                secrets::set_mcp_header(&name, &h.name, &h.value)?;
                (h.value, h.secret) = (String::new(), true);
            }
        }
        Ok(())
    }

    /// Delete what this server keeps in the Keychain (it's been removed), but for what `kept`
    /// (the server it was edited into) still uses.
    pub fn forget_secrets(&self, kept: Option<&McpServerConfig>) {
        let same = kept.filter(|k| k.name == self.name);
        for h in self.headers.iter().filter(|h| h.secret) {
            if !same.is_some_and(|k| k.headers.iter().any(|n| n.secret && n.name.eq_ignore_ascii_case(&h.name))) {
                let _ = secrets::delete_mcp_header(&self.name, &h.name);
            }
        }
        for e in self.env.iter().filter(|e| e.secret) {
            if !same.is_some_and(|k| k.env.iter().any(|n| n.secret && n.name == e.name)) {
                let _ = secrets::delete_mcp_env(&self.name, &e.name);
            }
        }
    }

    /// Its environment with the values read back from the Keychain; one whose value is gone
    /// isn't set.
    pub fn resolved_env(&self) -> Vec<(String, String)> {
        self.env.iter().filter_map(|e| Some((e.name.clone(), if e.secret { secrets::mcp_env(&self.name, &e.name)? } else { e.value.clone() }))).collect()
    }

    /// Its headers, the same way.
    pub fn resolved_headers(&self) -> Vec<(String, String)> {
        self.headers.iter().filter_map(|h| Some((h.name.clone(), if h.secret { secrets::mcp_header(&self.name, &h.name)? } else { h.value.clone() }))).collect()
    }
}

/// Stands for a value kept in the Keychain when a server's command line is shown for editing.
pub const KEPT: &str = "…";

/// An environment variable a stdio MCP server is started with. A `secret` one keeps its value
/// in the Keychain (`secrets::mcp_env`), never in the settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpEnvVar {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default)]
    pub secret: bool,
}

/// A line the user gave as an MCP server that is a remote server's address.
pub fn is_mcp_url(line: &str) -> bool {
    let l = line.trim();
    (l.starts_with("https://") || l.starts_with("http://")) && !l.contains(char::is_whitespace) && l.len() > "https://".len()
}

/// A header sent to a remote MCP server. A `secret` one (a token) keeps its value in the
/// Keychain (`secrets::mcp_header`), never in the settings file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpHeader {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default)]
    pub secret: bool,
}

impl McpHeader {
    /// `Name: value`, as typed in Settings (`Authorization: Bearer …`).
    pub fn parse(line: &str) -> Option<(String, String)> {
        let (name, value) = line.split_once(':')?;
        let (name, value) = (name.trim(), value.trim());
        (!name.is_empty() && !value.is_empty() && !name.contains(char::is_whitespace)).then(|| (name.to_string(), value.to_string()))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: General::default(),
            appearance: Appearance::default(),
            inbox: Inbox::default(),
            permissions: Permissions::default(),
            import: Import::default(),
            updates: Updates::default(),
            notifications: Notifications::default(),
            onboarding: Onboarding::default(),
            api_providers: vec![],
            custom_endpoints: vec![],
            added_agents: vec![],
            user_projects: vec![],
            disabled_agents: vec![],
            tools: Tools::default(),
            snapshots: Snapshots::default(),
            layout: Layout::default(),
            projects: Default::default(),
            hidden_projects: vec![],
            ide: Ide::default(),
            mobile: Mobile::default(),
            usage: Usage::default(),
            terminal: Terminal::default(),
            guard: SaveGuard::default(),
        }
    }
}

/// The built-in terminal panel.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Terminal {
    /// The shell it opens: a path, or a name found on PATH (`pwsh`, `nu`). Empty, the automatic
    /// choice: `$SHELL` on macOS; on Windows PowerShell 7, else Windows PowerShell, else `%COMSPEC%`.
    pub shell: String,
}

/// How many providers the Usage card shows at once.
pub const USAGE_SHOWN_MAX: usize = 3;

/// The sidebar's Usage card.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// The providers it shows (`AgentId::key()`), up to `USAGE_SHOWN_MAX`, as the user picked
    /// them. Unset: the first few that have usage to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `AgentId::key()` of the default agent.
    pub default_agent: String,
    pub default_model: Option<String>,
    pub default_effort: Effort,
    pub hand_holding: HandHolding,
    /// What Enter does while the agent is running.
    pub follow_up: FollowUp,
    /// Send with ⌘↩ instead of ↩.
    pub send_with_cmd_enter: bool,
    /// Give new threads a short title with a small, fast model after the first message.
    pub auto_title: bool,
    pub prevent_sleep_while_running: bool,
    /// Favorite models as `agent-key/model-id`.
    pub favorite_models: Vec<String>,
    /// What a thread does when its agent hits a usage limit.
    pub on_usage_limit: OnUsageLimit,
    /// Ask a running agent to wrap up when its usage window is nearly used up, rather than
    /// let the limit cut it off mid-edit (`limit::wrap_up_prompt`).
    pub wrap_up_near_limit: bool,
    /// Ask agents to start an answer that ends work with a recap: Done, Still to do, Found
    /// (`changes::RECAP`).
    pub ask_recap: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            default_agent: "claude-code".into(),
            default_model: None,
            default_effort: Effort::High,
            hand_holding: HandHolding::AutoAcceptEdits,
            follow_up: FollowUp::Steer,
            send_with_cmd_enter: false,
            auto_title: true,
            prevent_sleep_while_running: true,
            favorite_models: vec![],
            on_usage_limit: OnUsageLimit::Ask,
            wrap_up_near_limit: true,
            ask_recap: true,
        }
    }
}

/// What a thread does when its agent hits a usage limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OnUsageLimit {
    /// Pause, and offer to resume at the reset, snooze, or switch agents.
    #[default]
    Ask,
    /// Pause, and resume on its own once the limit resets.
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FollowUp {
    Steer,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    System,
    Night,
    Paper,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeChoice,
    /// Base UI text size in points (the theme's rem). Read through [`Appearance::ui_font_size`].
    pub ui_font_size: f32,
    /// Message text in the transcript, in points. Read through [`Appearance::transcript_font_size`].
    pub transcript_font_size: f32,
    pub code_font_size: f32,
    pub reduce_motion: bool,
    /// Scales every animation duration (0 = instant, 1 = designed timing).
    pub motion_scale: f32,
    /// `builtin:<name>` or an absolute path to a copied image; `None` = no image (the default:
    /// art is opt-in, in Settings › Appearance).
    pub background: Option<String>,
    pub background_placement: BackgroundPlacement,
    /// How strongly the image is darkened/lightened under content, 0.0–0.9.
    pub background_dim: f32,
    /// Liquid glass: the window lets the desktop show through, blurred, under translucent chrome.
    pub glass: bool,
    /// How much of the theme's colour the glass keeps, 0.2 (clear) to 0.95 (frosted).
    pub glass_tint: f32,
    /// The Dock icon: the cairn on Trek orange, or one of its alternatives.
    pub app_icon: AppIcon,
}

/// The app icons Trek can show in the Dock (`assets/brand/trek_icon.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppIcon {
    /// Porcelain stones on a Trek-orange tile: the icon in the app bundle.
    #[default]
    Ember,
    /// Trek-orange stones on the Night theme's charcoal.
    Night,
    /// Frosted glass stones over a sunset.
    Glass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundPlacement {
    /// Behind the new-thread composer (Capy style).
    NewThread,
    /// Behind the whole window chrome.
    Everywhere,
}

/// The sizes Trek is designed at: the theme's 14 pt rem and 14.5 pt transcript text.
pub const DEFAULT_UI_FONT_SIZE: f32 = 14.0;
pub const DEFAULT_TRANSCRIPT_FONT_SIZE: f32 = 14.5;
/// Sizes outside this range are treated as unset (0 = default).
const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 9.0..=32.0;

fn font_size_or(v: f32, default: f32) -> f32 {
    if v.is_finite() && FONT_SIZE_RANGE.contains(&v) { v } else { default }
}

impl Appearance {
    /// The UI font size to apply: the stored value, or the default when unset or out of range.
    pub fn ui_font_size(&self) -> f32 {
        font_size_or(self.ui_font_size, DEFAULT_UI_FONT_SIZE)
    }

    /// The transcript font size to apply: the stored value, or the default when unset or out of range.
    pub fn transcript_font_size(&self) -> f32 {
        font_size_or(self.transcript_font_size, DEFAULT_TRANSCRIPT_FONT_SIZE)
    }

    /// The glass's tint to draw with, when glass is on: the stored one, kept in its range.
    pub fn glass_tint(&self) -> Option<f32> {
        self.glass.then(|| if self.glass_tint.is_finite() { self.glass_tint.clamp(0.2, 0.95) } else { 0.6 })
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            transcript_font_size: DEFAULT_TRANSCRIPT_FONT_SIZE,
            code_font_size: 12.5,
            reduce_motion: false,
            motion_scale: 1.0,
            background: None,
            background_placement: BackgroundPlacement::NewThread,
            background_dim: 0.35,
            glass: false,
            glass_tint: 0.6,
            app_icon: AppIcon::Ember,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inbox {
    /// Settle finished threads after this many idle days (0 = never).
    pub auto_settle_days: u32,
    pub auto_settle_on_merge: bool,
    pub project_grouping: ProjectGrouping,
    pub show_working_shelf: bool,
}

impl Default for Inbox {
    fn default() -> Self {
        Self { auto_settle_days: 3, auto_settle_on_merge: true, project_grouping: ProjectGrouping::Repository, show_working_shelf: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectGrouping {
    Repository,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Permissions {
    /// Full access must be unlocked once before it appears in the composer.
    pub full_access_unlocked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Import {
    pub claude_code: bool,
    pub codex: bool,
    pub opencode: bool,
    /// Only index threads updated within this many days (0 = all).
    pub max_age_days: u32,
}

impl Default for Import {
    fn default() -> Self {
        Self { claude_code: true, codex: true, opencode: true, max_age_days: 90 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Stable,
    Beta,
    Nightly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    pub channel: Channel,
    pub auto_check: bool,
    pub auto_download: bool,
    /// Where releases come from: a GitHub releases URL, a manifest URL template with `{channel}`,
    /// or one manifest URL ending in `.json` (see `update::manifest_urls`).
    pub feed_url: String,
    /// The version whose "What's new" the user has seen. Behind the running version after an
    /// update, until they open it.
    pub seen_notes: String,
    /// Look for new versions of the agent CLIs (`agent_update`) at launch and every 12 hours.
    pub check_agents: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self { channel: Channel::Stable, auto_check: true, auto_download: true, feed_url: crate::update::OFFICIAL_FEED.into(), seen_notes: String::new(), check_agents: true }
    }
}

/// The placeholder feed earlier builds saved before Trek had a published home.
const RETIRED_FEED_PREFIX: &str = "https://github.com/trek-app/trek/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotifyMode {
    Off,
    Banner,
    Sound,
    BannerAndSound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub mode: NotifyMode,
    pub only_when_unfocused: bool,
    pub dock_badge: bool,
    pub menu_bar_icon: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { mode: NotifyMode::BannerAndSound, only_when_unfocused: true, dock_badge: true, menu_bar_icon: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Onboarding {
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
}

impl Settings {
    pub fn load() -> Settings {
        Self::load_checked().0
    }

    /// The settings file, read as well as it can be: a value this build can't read (a typo, or a
    /// choice a newer Trek added) falls back to its default alone, and a file that isn't TOML at
    /// all leaves every setting at its default with saving refused, so it's never written over.
    /// Either way a copy of the file is kept, and the problem returned for the user to see.
    pub fn load_checked() -> (Settings, Option<LoadProblem>) {
        let path = crate::paths::settings_file();
        let text = match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => return (Self::migrated(Settings::default()), None),
        };
        let (s, problem) = Self::parse(&text);
        let problem = problem.map(|mut p| {
            tracing::warn!("settings: {}; dropped: {:?}", p.error, p.dropped);
            p.backup = back_up(&path, &text);
            p
        });
        (Self::migrated(s), problem)
    }

    /// `text` as settings, salvaging what can be read (see `load_checked`). No copy is made.
    pub fn parse(text: &str) -> (Settings, Option<LoadProblem>) {
        let table = match toml::from_str::<toml::Table>(text) {
            Ok(t) => t,
            Err(e) => {
                let mut s = Settings::default();
                // Whoever wrote a settings file got past onboarding.
                s.onboarding.completed = !text.trim().is_empty();
                s.guard.block("settings.toml couldn't be read");
                return (s, Some(LoadProblem { backup: None, dropped: vec![], unreadable: true, error: e.to_string() }));
            }
        };
        let error = match toml::Value::Table(table.clone()).try_into::<Settings>() {
            Ok(s) => return (s, None),
            Err(e) => e.to_string(),
        };
        let mut kept = toml::Table::new();
        let mut dropped = vec![];
        salvage::<Settings>(&table, &mut vec![], &mut kept, &mut dropped);
        let mut s = toml::Value::Table(kept).try_into::<Settings>().unwrap_or_else(|_| {
            dropped = table.keys().map(|k| vec![k.clone()]).collect();
            Settings::default()
        });
        let defaults = toml::Value::try_from(&s).ok();
        s.guard.kept = dropped
            .iter()
            .filter_map(|p| Some((p.clone(), value_at(&toml::Value::Table(table.clone()), p)?.clone(), defaults.as_ref().and_then(|d| value_at(d, p)).cloned())))
            .collect();
        let dropped = dropped.iter().map(|p| p.join(".")).collect();
        (s, Some(LoadProblem { backup: None, dropped, unreadable: false, error }))
    }

    fn migrated(mut s: Settings) -> Settings {
        s.migrate();
        s
    }

    /// Bring older files up to date. Font sizes were saved (13 / 14) before Trek applied them; those
    /// untouched defaults become the sizes the app actually renders at, so nothing shrinks. The
    /// update feed pointed at a placeholder repository before releases were published.
    pub fn migrate(&mut self) {
        let a = &mut self.appearance;
        if a.ui_font_size == 13.0 && a.transcript_font_size == 14.0 {
            a.ui_font_size = DEFAULT_UI_FONT_SIZE;
            a.transcript_font_size = DEFAULT_TRANSCRIPT_FONT_SIZE;
        }
        let feed = self.updates.feed_url.trim();
        if feed.is_empty() || feed.starts_with(RETIRED_FEED_PREFIX) {
            self.updates.feed_url = crate::update::OFFICIAL_FEED.into();
        }
    }

    /// The file's text for these settings: values `load` couldn't read go back in unless the
    /// setting was changed since.
    pub fn to_toml(&self) -> anyhow::Result<String> {
        if self.guard.kept.is_empty() {
            return Ok(toml::to_string_pretty(self)?);
        }
        let mut value = toml::Value::try_from(self)?;
        for (path, raw, default) in &self.guard.kept {
            if value_at(&value, path) == default.as_ref() {
                set_at(&mut value, path, raw.clone());
            }
        }
        Ok(toml::to_string_pretty(&value)?)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        use std::io::Write as _;
        if let Some(why) = self.guard.blocked() {
            anyhow::bail!("not saved: {why}");
        }
        // A temp file of each save's own: two saves at once (two Treks, or tests side by side)
        // mustn't rename each other's half-written file into place.
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = crate::paths::settings_file();
        let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("toml.{}-{n}.tmp", std::process::id()));
        let text = self.to_toml()?;
        // On disk before the rename: after a power cut, an empty file would read as defaults.
        let saved = std::fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.sync_all()))
            .and_then(|_| std::fs::rename(&tmp, &path));
        if saved.is_err() {
            let _ = std::fs::remove_file(&tmp);
        } else if let Some(dir) = path.parent().and_then(|d| std::fs::File::open(d).ok()) {
            let _ = dir.sync_all();
        }
        Ok(saved?)
    }
}

/// Copy `text` (the file at `path`) to `settings.toml.bad-<time>` beside it, unless the latest
/// copy already holds it.
fn back_up(path: &std::path::Path, text: &str) -> Option<std::path::PathBuf> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let prefix = format!("{name}.bad-");
    let latest = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|f| f.to_string_lossy().starts_with(&prefix))).max();
    if let Some(latest) = latest.filter(|l| std::fs::read_to_string(l).is_ok_and(|t| t == text)) {
        return Some(latest);
    }
    let copy = dir.join(format!("{prefix}{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::write(&copy, text).ok().map(|_| copy)
}

/// Keep from `node` (the table at `path` of the file) into `kept` every value `T` reads on its
/// own; a table that doesn't is gone through key by key. What's left out goes in `dropped`.
fn salvage<T: serde::de::DeserializeOwned>(node: &toml::Table, path: &mut Vec<String>, kept: &mut toml::Table, dropped: &mut Vec<Vec<String>>) {
    for (k, v) in node {
        path.push(k.clone());
        let mut probe = toml::Value::Table(toml::Table::new());
        set_at(&mut probe, path, v.clone());
        if probe.try_into::<T>().is_ok() {
            let mut root = toml::Value::Table(std::mem::take(kept));
            set_at(&mut root, path, v.clone());
            if let toml::Value::Table(t) = root {
                *kept = t;
            }
        } else if let toml::Value::Table(t) = v {
            salvage::<T>(t, path, kept, dropped);
        } else {
            dropped.push(path.clone());
        }
        path.pop();
    }
}

fn value_at<'a>(v: &'a toml::Value, path: &[String]) -> Option<&'a toml::Value> {
    path.iter().try_fold(v, |v, k| v.as_table()?.get(k))
}

/// Put `value` at `path` in `root`, making the tables on the way.
fn set_at(root: &mut toml::Value, path: &[String], value: toml::Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = root;
    for k in parents {
        let Some(t) = node.as_table_mut() else { return };
        node = t.entry(k.clone()).or_insert_with(|| toml::Value::Table(toml::Table::new()));
    }
    if let Some(t) = node.as_table_mut() {
        t.insert(last.clone(), value);
    }
}

/// API keys and MCP server tokens in the macOS Keychain (or platform secret store).
pub mod secrets {
    const SERVICE: &str = "dev.trek.Trek";

    pub fn set_api_key(provider: &str, key: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!crate::paths::isolated(), "the Keychain is off in this process");
        keyring::Entry::new(SERVICE, provider)?.set_password(key)?;
        Ok(())
    }

    /// The Keychain account for header `header` of MCP server `server` (its own namespace, so
    /// it can't collide with a provider's API key).
    fn mcp_account(server: &str, header: &str) -> String {
        format!("mcp-header:{server}:{}", header.to_ascii_lowercase())
    }

    /// The same for its environment variable `var` (names are case-sensitive there).
    fn mcp_env_account(server: &str, var: &str) -> String {
        format!("mcp-env:{server}:{var}")
    }

    /// In an isolated process (tests) MCP server tokens are kept in memory instead, so adding,
    /// editing and removing a server goes the whole way without reaching the user's Keychain.
    static MEMORY: std::sync::Mutex<std::collections::BTreeMap<String, String>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

    fn memory() -> std::sync::MutexGuard<'static, std::collections::BTreeMap<String, String>> {
        MEMORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_mcp(account: &str, value: &str) -> anyhow::Result<()> {
        if crate::paths::isolated() {
            memory().insert(account.to_string(), value.to_string());
            return Ok(());
        }
        keyring::Entry::new(SERVICE, account)?.set_password(value)?;
        Ok(())
    }

    fn get_mcp(account: &str) -> Option<String> {
        if crate::paths::isolated() {
            return memory().get(account).cloned();
        }
        keyring::Entry::new(SERVICE, account).ok()?.get_password().ok()
    }

    fn delete_mcp(account: &str) -> anyhow::Result<()> {
        if crate::paths::isolated() {
            memory().remove(account);
            return Ok(());
        }
        keyring::Entry::new(SERVICE, account)?.delete_credential()?;
        Ok(())
    }

    pub fn set_mcp_header(server: &str, header: &str, value: &str) -> anyhow::Result<()> {
        set_mcp(&mcp_account(server, header), value)
    }

    pub fn mcp_header(server: &str, header: &str) -> Option<String> {
        get_mcp(&mcp_account(server, header))
    }

    pub fn delete_mcp_header(server: &str, header: &str) -> anyhow::Result<()> {
        delete_mcp(&mcp_account(server, header))
    }

    pub fn set_mcp_env(server: &str, var: &str, value: &str) -> anyhow::Result<()> {
        set_mcp(&mcp_env_account(server, var), value)
    }

    pub fn mcp_env(server: &str, var: &str) -> Option<String> {
        get_mcp(&mcp_env_account(server, var))
    }

    pub fn delete_mcp_env(server: &str, var: &str) -> anyhow::Result<()> {
        delete_mcp(&mcp_env_account(server, var))
    }

    pub fn api_key(provider: &str) -> Option<String> {
        if crate::paths::isolated() {
            return None;
        }
        if let Some(env) = crate::catalog::direct_provider(provider).and_then(|p| p.env_key) {
            if let Ok(v) = std::env::var(env) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        keyring::Entry::new(SERVICE, provider).ok()?.get_password().ok()
    }

    pub fn delete_api_key(provider: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!crate::paths::isolated(), "the Keychain is off in this process");
        keyring::Entry::new(SERVICE, provider)?.delete_credential()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        let partial: Settings = toml::from_str("[general]\nhand_holding = \"auto\"\n").unwrap();
        assert_eq!(partial.general.hand_holding, HandHolding::Auto);
        assert_eq!(partial.inbox.auto_settle_days, 3);
    }

    #[test]
    fn the_usage_card_picks_automatically_until_the_user_picks() {
        // Files from before the choice existed: automatic, and nothing is written for it.
        let old: Settings = toml::from_str("[general]\nhand_holding = \"auto\"\n").unwrap();
        assert_eq!(old.usage.shown, None);
        assert!(!toml::to_string_pretty(&old).unwrap().contains("shown"));
        let mut s = Settings::default();
        s.usage.shown = Some(vec!["claude-code".into(), "acp:devin".into()]);
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("[usage]"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().usage.shown, s.usage.shown);
        // Unchecking every one is a choice too (an empty card), not a return to automatic.
        s.usage.shown = Some(vec![]);
        assert_eq!(toml::from_str::<Settings>(&toml::to_string_pretty(&s).unwrap()).unwrap().usage.shown, Some(vec![]));
    }

    #[test]
    fn the_terminal_shell_is_automatic_until_one_is_named() {
        let old: Settings = toml::from_str("[general]\nhand_holding = \"auto\"\n").unwrap();
        assert_eq!(old.terminal.shell, "", "files from before the key: the automatic choice");
        let mut s = Settings::default();
        s.terminal.shell = r"C:\Program Files\PowerShell\7\pwsh.exe".into();
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("[terminal]"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().terminal.shell, s.terminal.shell);
        assert_eq!(toml::from_str::<Settings>("[terminal]\nshell = \"nu\"\n").unwrap().terminal.shell, "nu");
    }

    #[test]
    fn mcp_servers_from_before_http_load_unchanged_and_http_ones_round_trip() {
        // A settings file written before remote servers existed.
        let old = "[tools]\ncomputer_use = true\n\n[[tools.mcp_servers]]\nname = \"github\"\ncommand = \"npx\"\nargs = [\"-y\", \"@modelcontextprotocol/server-github\"]\nenabled = true\n";
        let s: Settings = toml::from_str(old).unwrap();
        assert!(s.tools.computer_use && !s.tools.figma_desktop, "the Figma desktop server is opt-in");
        let gh = &s.tools.mcp_servers[0];
        assert_eq!(*gh, McpServerConfig::stdio("github", "npx", vec!["-y".into(), "@modelcontextprotocol/server-github".into()]));
        assert!(!gh.is_http());
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::to_string(gh).unwrap(), "name = \"github\"\ncommand = \"npx\"\nargs = [\"-y\", \"@modelcontextprotocol/server-github\"]\nenabled = true\n", "a stdio server saves as it did");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);

        let mut s = Settings::default();
        s.tools.figma_desktop = true;
        s.tools.mcp_servers.push(McpServerConfig::http(
            "linear",
            "https://mcp.linear.app/mcp",
            vec![McpHeader { name: "Authorization".into(), value: String::new(), secret: true }, McpHeader { name: "X-Team".into(), value: "core".into(), secret: false }],
        ));
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(!toml::to_string(&s.tools.mcp_servers[0]).unwrap().contains("command"), "an HTTP server has no command");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
    }

    #[test]
    fn an_mcp_servers_tokens_go_to_the_keychain_and_follow_it() {
        // Values typed in are moved out of the settings file; only the names are saved.
        let mut gh = McpServerConfig::parse("gh-keys", "GITHUB_TOKEN=ghp_1 LOG=debug npx -y @modelcontextprotocol/server-github").unwrap();
        gh.headers.push(McpHeader { name: "X-Key".into(), value: "k".into(), secret: false });
        gh.stash_secrets(None).unwrap();
        assert!(gh.env.iter().all(|e| e.secret && e.value.is_empty()) && gh.headers[0].secret);
        let saved = toml::to_string(&gh).unwrap();
        assert!(!saved.contains("ghp_1") && saved.contains("GITHUB_TOKEN"), "{saved}");
        assert_eq!(toml::from_str::<McpServerConfig>(&saved).unwrap(), gh);
        assert_eq!(gh.resolved_env(), [("GITHUB_TOKEN".to_string(), "ghp_1".to_string()), ("LOG".into(), "debug".into())]);

        // Edited and renamed: a value left as it was moves with it, a new one replaces it, and
        // what the old name kept is let go of.
        let mut edited = McpServerConfig::parse("gh-keys-2", &gh.command_line().replace("LOG=…", "LOG=info")).unwrap();
        edited.headers = gh.headers.clone();
        edited.stash_secrets(Some("gh-keys")).unwrap();
        gh.forget_secrets(Some(&edited));
        assert_eq!(edited.resolved_env(), [("GITHUB_TOKEN".to_string(), "ghp_1".to_string()), ("LOG".into(), "info".into())]);
        assert_eq!(edited.resolved_headers(), [("X-Key".to_string(), "k".to_string())]);
        assert!(gh.resolved_env().is_empty() && gh.resolved_headers().is_empty());
        // A kept value with nothing saved behind it is an error, not an empty token.
        assert!(McpServerConfig::parse("new", "A=… run").unwrap().stash_secrets(Some("nothing")).is_err());

        // Edited in place: what it still uses stays.
        let mut again = McpServerConfig::parse("gh-keys-2", &edited.command_line()).unwrap();
        again.stash_secrets(Some("gh-keys-2")).unwrap();
        edited.forget_secrets(Some(&again));
        assert_eq!(again.resolved_env().len(), 2);
        again.forget_secrets(None);
        assert!(again.resolved_env().is_empty(), "removed, its tokens go too");
    }

    #[test]
    fn the_add_row_tells_a_url_from_a_command() {
        let http = McpServerConfig::parse("linear", " https://mcp.linear.app/mcp ").unwrap();
        assert_eq!(http.url.as_deref(), Some("https://mcp.linear.app/mcp"));
        assert_eq!(http.summary(), "https://mcp.linear.app/mcp");
        assert!(McpServerConfig::parse("figma", "http://127.0.0.1:3845/mcp").unwrap().is_http());
        let stdio = McpServerConfig::parse("gh", "npx -y @modelcontextprotocol/server-github").unwrap();
        assert_eq!((stdio.command.as_str(), stdio.args.len(), stdio.is_http()), ("npx", 2, false));
        assert_eq!(stdio.summary(), "npx -y @modelcontextprotocol/server-github");
        assert_eq!(McpServerConfig::parse("", "npx srv"), None, "a server needs a name");
        assert_eq!(McpServerConfig::parse("x", "  "), None);
        assert!(!is_mcp_url("https://"), "a scheme alone isn't an address");
        assert!(!is_mcp_url("httpie get"), "a command that starts like a scheme is a command");
        assert_eq!(McpHeader::parse("Authorization: Bearer abc:def"), Some(("Authorization".into(), "Bearer abc:def".into())));
        assert_eq!(McpHeader::parse("Bearer abc"), None);
        assert_eq!(McpHeader::parse("X Y: z"), None);
    }

    #[test]
    fn background_art_is_opt_in_and_a_chosen_one_is_kept() {
        assert_eq!(Settings::default().appearance.background, None, "a new install has a plain new-thread screen");
        // A settings file that names the art (saved before it was opt-in, or chosen) keeps it.
        let s: Settings = toml::from_str("[appearance]\nbackground = \"builtin:dawn\"\n").unwrap();
        assert_eq!(s.appearance.background.as_deref(), Some("builtin:dawn"));
        // None survives a save and a load (TOML leaves it out; the default fills it back in).
        let back: Settings = toml::from_str(&toml::to_string_pretty(&Settings::default()).unwrap()).unwrap();
        assert_eq!(back.appearance.background, None);
    }

    #[test]
    fn projects_from_before_worktrees_run_locally_and_copy_env_files() {
        let s: Settings = toml::from_str("[projects.\"/code/app\"]\nicon = \"lucide:rocket\"\n").unwrap();
        let p = &s.projects["/code/app"];
        assert_eq!(p.run_in, RunIn::Local);
        assert_eq!(p.worktree_copy, [".env", ".env.local"]);
        assert!(!p.is_empty());
        let mut back = p.clone();
        back.icon = None;
        assert!(back.is_empty(), "defaults alone don't keep a project's entry");
        back.run_in = RunIn::Worktree;
        let text = toml::to_string(&back).unwrap();
        assert_eq!(toml::from_str::<ProjectPrefs>(&text).unwrap(), back);
    }

    #[test]
    fn a_project_colour_is_kept_and_alone_keeps_the_entry() {
        let s: Settings = toml::from_str("[projects.\"/code/app\"]\ncolor = 210\n").unwrap();
        let p = &s.projects["/code/app"];
        assert_eq!(p.color, Some(210));
        assert!(!p.is_empty(), "a chosen colour is worth keeping");
        assert_eq!(toml::from_str::<ProjectPrefs>(&toml::to_string(p).unwrap()).unwrap(), *p);
        assert_eq!(ProjectPrefs::default().color, None, "colours come from the name until chosen");
    }

    #[test]
    fn font_sizes_fall_back_and_migrate() {
        let mut s: Settings = toml::from_str("[appearance]\nui_font_size = 0.0\ntranscript_font_size = 16.0\n").unwrap();
        assert_eq!(s.appearance.ui_font_size(), DEFAULT_UI_FONT_SIZE);
        assert_eq!(s.appearance.transcript_font_size(), 16.0);
        s.appearance.ui_font_size = 13.0;
        s.appearance.transcript_font_size = 14.0;
        s.migrate();
        assert_eq!((s.appearance.ui_font_size, s.appearance.transcript_font_size), (DEFAULT_UI_FONT_SIZE, DEFAULT_TRANSCRIPT_FONT_SIZE));
        s.appearance.ui_font_size = 13.0;
        s.migrate();
        assert_eq!(s.appearance.ui_font_size, 13.0);
    }

    #[test]
    fn computer_use_is_opt_in_and_a_choice_to_have_it_stays() {
        assert!(!Settings::default().tools.computer_use);
        let s: Settings = toml::from_str("[tools]\nsimulator = true\n").unwrap();
        assert!(!s.tools.computer_use);
        let s: Settings = toml::from_str("[tools]\ncomputer_use = true\n").unwrap();
        assert!(s.tools.computer_use);
    }

    #[test]
    fn one_bad_value_falls_back_alone_and_is_written_back() {
        let text = "[general]\nsend_with_cmd_enter = true\n\n[appearance]\ntheme = \"aurora\"\nui_font_size = 16.0\n\n[onboarding]\ncompleted = true\n";
        let (s, problem) = Settings::parse(text);
        let problem = problem.expect("a problem");
        assert!(!problem.unreadable);
        assert_eq!(problem.dropped, ["appearance.theme"]);
        assert!(problem.message().contains("appearance.theme"));
        // Everything else survives.
        assert!(s.general.send_with_cmd_enter && s.onboarding.completed);
        assert_eq!(s.appearance.ui_font_size, 16.0);
        assert_eq!(s.appearance.theme, ThemeChoice::System);
        assert!(s.guard.blocked().is_none());
        // The newer value goes back in the file while the user leaves the theme alone…
        let back: toml::Table = toml::from_str(&s.to_toml().unwrap()).unwrap();
        assert_eq!(back["appearance"]["theme"].as_str(), Some("aurora"));
        // …and gives way once they pick one.
        let mut s = s;
        s.appearance.theme = ThemeChoice::Paper;
        let back: toml::Table = toml::from_str(&s.to_toml().unwrap()).unwrap();
        assert_eq!(back["appearance"]["theme"].as_str(), Some("paper"));
    }

    #[test]
    fn bad_values_in_nested_tables_and_maps_fall_back_alone() {
        let text = "[projects.\"/code/app\"]\nicon = \"lucide:rocket\"\nrun_in = \"cloud\"\n\n[mobile]\nport = \"x\"\nenabled = true\nreach = \"tailscale\"\n";
        let (s, problem) = Settings::parse(text);
        let mut dropped = problem.unwrap().dropped;
        dropped.sort();
        assert_eq!(dropped, ["mobile.port", "projects./code/app.run_in"]);
        assert_eq!(s.projects["/code/app"].icon.as_deref(), Some("lucide:rocket"));
        assert_eq!(s.projects["/code/app"].run_in, RunIn::Local);
        assert!(s.mobile.enabled);
        assert_eq!((s.mobile.port, s.mobile.reach), (7420, Reach::Tailscale));
    }

    #[test]
    fn a_file_that_isnt_toml_is_never_saved_over() {
        let (s, problem) = Settings::parse("[general\nsend_with_cmd_enter = true\n");
        let problem = problem.unwrap();
        assert!(problem.unreadable);
        assert!(problem.message().contains("won't save"));
        assert!(s.onboarding.completed, "someone who has a settings file has been through onboarding");
        assert!(s.save().is_err());
        assert_eq!(Settings::parse("").1, None);
    }

    #[test]
    fn a_problem_keeps_one_copy_of_the_file() {
        let dir = std::env::temp_dir().join(format!("trek-settings-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        let first = back_up(&path, "bad = [").unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "bad = [");
        assert!(first.file_name().unwrap().to_string_lossy().starts_with("settings.toml.bad-"));
        assert_eq!(back_up(&path, "bad = [").unwrap(), first, "the same text isn't copied twice");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_update_feed_moves_to_the_published_one() {
        let mut s: Settings =
            toml::from_str("[updates]\nchannel = \"beta\"\nfeed_url = \"https://github.com/trek-app/trek/releases/latest/download/{channel}.json\"\n").unwrap();
        s.migrate();
        assert_eq!(s.updates.feed_url, crate::update::OFFICIAL_FEED);
        assert_eq!(s.updates.channel, Channel::Beta);
        // A feed the user chose stays.
        s.updates.feed_url = "http://127.0.0.1:8765/{channel}.json".into();
        s.migrate();
        assert_eq!(s.updates.feed_url, "http://127.0.0.1:8765/{channel}.json");
    }
}

/// The editor (Trek IDE): recent picks, the workbench's layout, and how it follows the agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ide {
    /// Folders opened as the IDE's workspace, most recent first.
    pub recent_folders: Vec<String>,
    /// Loose files opened in the editor (from a folder or on their own), most recent first.
    pub recent_files: Vec<String>,
    pub layout: IdeLayout,
    /// Going back to Agents opens the AI side bar's chat there.
    pub follow_active_chat: bool,
}

impl Default for Ide {
    fn default() -> Self {
        Self { recent_folders: vec![], recent_files: vec![], layout: IdeLayout::default(), follow_active_chat: true }
    }
}

/// The workbench's regions as the user left them: sizes in points, and which are shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdeLayout {
    pub primary_width: f32,
    pub ai_width: f32,
    pub panel_height: f32,
    pub primary_open: bool,
    pub ai_open: bool,
    pub panel_open: bool,
}

impl IdeLayout {
    pub const PRIMARY: std::ops::RangeInclusive<f32> = 180.0..=480.0;
    pub const AI: std::ops::RangeInclusive<f32> = 320.0..=720.0;
    pub const MIN_PANEL: f32 = 120.;
}

impl Default for IdeLayout {
    fn default() -> Self {
        Self { primary_width: 260., ai_width: 400., panel_height: 240., primary_open: true, ai_open: true, panel_open: false }
    }
}

impl Ide {
    pub fn remember_folder(&mut self, path: &str) {
        self.recent_folders.retain(|f| f != path);
        self.recent_folders.insert(0, path.to_string());
        self.recent_folders.truncate(15);
    }

    pub fn remember_file(&mut self, path: &str) {
        self.recent_files.retain(|f| f != path);
        self.recent_files.insert(0, path.to_string());
        self.recent_files.truncate(15);
    }
}
