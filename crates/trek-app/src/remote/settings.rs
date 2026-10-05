//! The Mac's settings a phone may see and change: what new threads start with, follow-ups,
//! notifications (the Mac's and the phone's through ntfy), auto-settle and the theme. Nothing
//! else: Full access's unlock, API keys and the phone server itself stay the Mac's to change, and
//! Full access as new threads' default needs the unlock like everywhere else.

use super::hand_holding;
use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::AgentId;
use trek_core::settings::{FollowUp, NotifyMode, PushWhen, ThemeChoice};
use trek_remote as tr;

impl Workspace {
    pub(super) fn remote_settings(&self) -> tr::MacSettings {
        let s = &self.settings;
        let m = &s.mobile;
        let (topic_url, subscribe_url) = tr::ntfy_links(&m.push_server, &m.push_topic);
        tr::MacSettings {
            default_agent: s.general.default_agent.clone(),
            default_model: s.general.default_model.clone(),
            default_effort: s.general.default_effort.as_str().to_string(),
            default_access: match s.general.hand_holding {
                trek_core::HandHolding::Supervised => tr::Access::Supervised,
                trek_core::HandHolding::AutoAcceptEdits => tr::Access::AutoAcceptEdits,
                trek_core::HandHolding::Auto => tr::Access::Auto,
                trek_core::HandHolding::FullAccess => tr::Access::FullAccess,
            },
            follow_up: match s.general.follow_up {
                FollowUp::Steer => tr::SendMode::Steer,
                FollowUp::Queue => tr::SendMode::Queue,
            },
            notifications: match s.notifications.mode {
                NotifyMode::Off => tr::NotifyMode::Off,
                NotifyMode::Banner => tr::NotifyMode::Banner,
                NotifyMode::Sound => tr::NotifyMode::Sound,
                NotifyMode::BannerAndSound => tr::NotifyMode::BannerAndSound,
            },
            push: tr::PushSettings {
                enabled: m.push,
                when: match m.push_when {
                    PushWhen::Away => tr::PushWhen::Away,
                    PushWhen::Always => tr::PushWhen::Always,
                },
                server: m.push_server.clone(),
                topic: m.push_topic.clone(),
                topic_url,
                subscribe_url,
            },
            auto_settle_days: s.inbox.auto_settle_days,
            theme: match s.appearance.theme {
                ThemeChoice::System => tr::Theme::System,
                ThemeChoice::Night => tr::Theme::Night,
                ThemeChoice::Paper => tr::Theme::Paper,
            },
            full_access: s.permissions.full_access_unlocked,
            session_approvals: m.session_approvals,
        }
    }

    /// Apply what the phone changed, all or nothing: one value the Mac won't take refuses the lot.
    pub(super) fn remote_set_settings(&mut self, change: tr::SettingsChange, cx: &mut Context<Self>) -> tr::HostResult<tr::MacSettings> {
        let mut s = self.settings.clone();
        if let Some(key) = &change.default_agent {
            let agent = AgentId::from_key(key);
            if !self.ready_agents().contains(&agent) {
                return Err(tr::HostError::not_found(format!("{} isn't set up on this Mac", agent.display_name())));
            }
            if s.general.default_agent != *key {
                s.general.default_agent = key.clone();
                // Its model was the old agent's.
                s.general.default_model = None;
            }
        }
        if let Some(model) = &change.default_model {
            let agent = AgentId::from_key(&s.general.default_agent);
            let models = self.models_for(&agent);
            if !model.is_empty() && !models.is_empty() && !models.iter().any(|m| &m.id == model) {
                return Err(tr::HostError::not_found(format!("{model} isn't one of {}'s models", agent.display_name())));
            }
            s.general.default_model = Some(model.clone()).filter(|m| !m.is_empty());
        }
        if let Some(effort) = &change.default_effort {
            s.general.default_effort = trek_core::Effort::parse(effort).ok_or_else(|| tr::HostError::bad_request(format!("No effort called {effort}")))?;
        }
        if let Some(access) = change.default_access {
            s.general.hand_holding = hand_holding(access, &self.settings)?;
        }
        if let Some(mode) = change.follow_up {
            s.general.follow_up = match mode {
                tr::SendMode::Steer => FollowUp::Steer,
                tr::SendMode::Queue => FollowUp::Queue,
            };
        }
        if let Some(mode) = change.notifications {
            s.notifications.mode = match mode {
                tr::NotifyMode::Off => NotifyMode::Off,
                tr::NotifyMode::Banner => NotifyMode::Banner,
                tr::NotifyMode::Sound => NotifyMode::Sound,
                tr::NotifyMode::BannerAndSound => NotifyMode::BannerAndSound,
            };
        }
        if let Some(on) = change.push {
            s.mobile.push = on;
            // A topic the first time, as the Mac's switch makes one.
            if on && s.mobile.push_topic.is_empty() {
                s.mobile.push_topic = crate::push::new_topic();
            }
        }
        if let Some(when) = change.push_when {
            s.mobile.push_when = match when {
                tr::PushWhen::Away => PushWhen::Away,
                tr::PushWhen::Always => PushWhen::Always,
            };
        }
        if let Some(server) = &change.push_server {
            let server = server.trim().trim_end_matches('/');
            let ok = (server.starts_with("https://") || server.starts_with("http://")) && server.len() <= 200 && !server.chars().any(|c| c.is_whitespace() || c.is_control());
            if !ok {
                return Err(tr::HostError::bad_request("The ntfy server is a web address: https://ntfy.sh, or your own"));
            }
            s.mobile.push_server = server.to_string();
        }
        if change.new_push_topic {
            s.mobile.push_topic = crate::push::new_topic();
        }
        if let Some(days) = change.auto_settle_days {
            if days > 365 {
                return Err(tr::HostError::bad_request("Settle after a year at most (0 never settles)"));
            }
            s.inbox.auto_settle_days = days;
        }
        let theme = change.theme.map(|t| match t {
            tr::Theme::System => ThemeChoice::System,
            tr::Theme::Night => ThemeChoice::Night,
            tr::Theme::Paper => ThemeChoice::Paper,
        });
        if let Some(t) = theme {
            s.appearance.theme = t;
        }
        if change.push_test && !s.mobile.push {
            return Err(tr::HostError::bad_request("Turn on notifications to your phone first"));
        }
        // Only these sections can have changed; the rest stays as the Mac has it.
        self.settings.general = s.general;
        self.settings.notifications = s.notifications;
        self.settings.mobile.push = s.mobile.push;
        self.settings.mobile.push_when = s.mobile.push_when;
        self.settings.mobile.push_server = s.mobile.push_server;
        self.settings.mobile.push_topic = s.mobile.push_topic;
        self.settings.inbox.auto_settle_days = s.inbox.auto_settle_days;
        self.settings.appearance.theme = s.appearance.theme;
        self.save_settings(cx);
        if change.push_test {
            self.test_push(cx);
        }
        if let Some(choice) = theme.filter(|_| !cfg!(test)) {
            cx.defer(move |cx| crate::apply_theme(choice, None, cx));
        }
        Ok(self.remote_settings())
    }
}
