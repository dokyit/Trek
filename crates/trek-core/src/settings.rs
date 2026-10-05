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
}

impl Default for Tools {
    fn default() -> Self {
        Self { computer_use: true, simulator: true, orchestration: true, mcp_servers: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
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
            user_projects: vec![],
            disabled_agents: vec![],
            tools: Tools::default(),
            snapshots: Snapshots::default(),
            layout: Layout::default(),
            projects: Default::default(),
            hidden_projects: vec![],
        }
    }
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
    /// `builtin:<name>` or an absolute path to a copied image; `None` = no image.
    pub background: Option<String>,
    pub background_placement: BackgroundPlacement,
    /// How strongly the image is darkened/lightened under content, 0.0–0.9.
    pub background_dim: f32,
    /// Liquid glass: the window lets the desktop show through, blurred, under translucent chrome.
    pub glass: bool,
    /// How much of the theme's colour the glass keeps, 0.3 (clear) to 0.9 (frosted).
    pub glass_tint: f32,
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
        self.glass.then(|| if self.glass_tint.is_finite() { self.glass_tint.clamp(0.3, 0.9) } else { 0.6 })
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
            background: Some("builtin:dawn".into()),
            background_placement: BackgroundPlacement::NewThread,
            background_dim: 0.35,
            glass: false,
            glass_tint: 0.6,
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
        let mut s: Settings = std::fs::read_to_string(crate::paths::settings_file())
            .ok()
            .and_then(|s| toml::from_str(&s).map_err(|e| tracing::warn!("settings parse error: {e}")).ok())
            .unwrap_or_default();
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

    pub fn save(&self) -> anyhow::Result<()> {
        // A temp file of each save's own: two saves at once (two Treks, or tests side by side)
        // mustn't rename each other's half-written file into place.
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = crate::paths::settings_file();
        let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("toml.{}-{n}.tmp", std::process::id()));
        let saved = std::fs::write(&tmp, toml::to_string_pretty(self)?).and_then(|_| std::fs::rename(&tmp, path));
        if saved.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        Ok(saved?)
    }
}

/// API keys in the macOS Keychain (or platform secret store).
pub mod secrets {
    const SERVICE: &str = "dev.trek.Trek";

    pub fn set_api_key(provider: &str, key: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!crate::paths::isolated(), "the Keychain is off in this process");
        keyring::Entry::new(SERVICE, provider)?.set_password(key)?;
        Ok(())
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
