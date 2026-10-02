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
    pub prevent_sleep_while_running: bool,
    /// Favorite models as `agent-key/model-id`.
    pub favorite_models: Vec<String>,
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
            prevent_sleep_while_running: true,
            favorite_models: vec![],
        }
    }
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
    pub ui_font_size: f32,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundPlacement {
    /// Behind the new-thread composer (Capy style).
    NewThread,
    /// Behind the whole window chrome.
    Everywhere,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            ui_font_size: 13.0,
            transcript_font_size: 14.0,
            code_font_size: 12.5,
            reduce_motion: false,
            motion_scale: 1.0,
            background: Some("builtin:dawn".into()),
            background_placement: BackgroundPlacement::NewThread,
            background_dim: 0.35,
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
    /// Manifest URL; `{channel}` is substituted.
    pub feed_url: String,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            channel: Channel::Stable,
            auto_check: true,
            auto_download: true,
            feed_url: "https://github.com/trek-app/trek/releases/latest/download/{channel}.json".into(),
        }
    }
}

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
        std::fs::read_to_string(crate::paths::settings_file())
            .ok()
            .and_then(|s| toml::from_str(&s).map_err(|e| tracing::warn!("settings parse error: {e}")).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = crate::paths::settings_file();
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// API keys in the macOS Keychain (or platform secret store).
pub mod secrets {
    const SERVICE: &str = "dev.trek.Trek";

    pub fn set_api_key(provider: &str, key: &str) -> anyhow::Result<()> {
        keyring::Entry::new(SERVICE, provider)?.set_password(key)?;
        Ok(())
    }

    pub fn api_key(provider: &str) -> Option<String> {
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
}
