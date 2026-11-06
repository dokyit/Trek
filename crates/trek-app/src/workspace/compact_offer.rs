//! When compacting would save money: a provider's prompt cache lasts a set time after a thread's
//! last activity (about an hour on a subscription, five minutes on an API key). Past that, the
//! next message re-reads the whole context at full price, so on a large thread it's worth
//! compacting first. On a small one the re-read costs little: there's nothing to offer there.

use super::{Scope, Workspace};
use trek_agents::Billing;
use trek_core::AgentId;
use trek_core::store::Item;

/// From this share of its window a thread counts as large.
pub const LARGE: f32 = 0.4;
/// How long a subscription's prompt cache lasts.
const PLAN_CACHE_MS: i64 = 60 * 60 * 1_000;
/// How long an API key's prompt cache lasts.
const KEY_CACHE_MS: i64 = 5 * 60 * 1_000;

/// How long the prompt cache lasts for what's billed like this; `None` where there's none (a model
/// on this computer). Not known yet, the longer one: better late than nagging.
fn cache_life(billing: Option<&Billing>) -> Option<i64> {
    match billing {
        Some(Billing::Local) => None,
        Some(Billing::Metered) => Some(KEY_CACHE_MS),
        Some(Billing::Plan(_)) | None => Some(PLAN_CACHE_MS),
    }
}

/// A large idle thread on an agent that can compact, and how long its cache lasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOffer {
    pub used: u64,
    pub window: u64,
    /// When the thread last did anything the provider's cache would have seen (unix ms).
    pub last_active: i64,
    life: i64,
}

impl CompactOffer {
    pub fn lapses_at(&self) -> i64 {
        self.last_active + self.life
    }

    /// The cache has gone by `now`.
    pub fn lapsed(&self, now: i64) -> bool {
        now >= self.lapses_at()
    }

    /// The chip's tooltip: why now, and what compacting does.
    pub fn why(&self) -> String {
        let tokens = crate::context_popover::fmt_tokens(self.used);
        if self.life == KEY_CACHE_MS {
            format!(
                "The prompt cache has expired: it lasts about 5 minutes on an API key.\nYour next message re-reads all {tokens} tokens at full price.\nCompact first to send less."
            )
        } else {
            format!(
                "The prompt cache has expired: it lasts about an hour on a subscription.\nYour next message re-reads all {tokens} tokens, and they count against your limits.\nCompact first to send less."
            )
        }
    }
}

/// When `items` last did anything: the end of its last turn, or a message that has no end yet.
fn last_active(items: &[Item]) -> Option<i64> {
    items.iter().rev().find_map(|i| match i {
        Item::TurnEnd { at, .. } => Some(*at),
        Item::User { at, .. } => *at,
        _ => None,
    })
}

impl Workspace {
    /// Whether `/compact` means something to `agent`: Claude Code's own, Codex's request and the
    /// Direct harness's own, and any other agent that offers it among its commands.
    pub fn can_compact(&self, scope: &Scope, agent: &AgentId) -> bool {
        match agent {
            AgentId::ClaudeCode | AgentId::Codex | AgentId::Direct(_) => true,
            _ => self
                .slash_commands(scope, agent)
                .iter()
                .any(|c| c.name == "compact"),
        }
    }

    /// `id` as a thread compacting could help, once its cache has gone: large (`LARGE`), idle, not
    /// waiting on anything, on an agent that can compact and has a cache to lose. `None` otherwise.
    pub fn compact_offer(&self, id: &str, scope: &Scope) -> Option<CompactOffer> {
        if crate::machines::is_remote(id) {
            return None;
        }
        let thread = self.thread(id)?;
        let live = self.live.get(id)?;
        let (used, window) = live.context?;
        if window == 0 || (used as f32) < window as f32 * LARGE {
            return None;
        }
        if self.turn_running(id)
            || self.pause(id).is_some()
            || live.pending_switch.is_some()
            || !live.permissions.is_empty()
            || !self.can_compact(scope, &thread.agent)
        {
            return None;
        }
        // The Direct harness compacts the session it has. With none (after a restart, the idle
        // shutdown or a plan toggle) the next message starts one from a recap, small already.
        if matches!(thread.agent, AgentId::Direct(_)) && live.commands.is_none() {
            return None;
        }
        Some(CompactOffer {
            used,
            window,
            last_active: last_active(&live.items)?,
            life: cache_life(self.billing_of(thread).as_ref())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_lasts_by_how_its_billed() {
        assert_eq!(cache_life(Some(&Billing::Local)), None);
        assert_eq!(cache_life(Some(&Billing::Metered)), Some(KEY_CACHE_MS));
        assert_eq!(cache_life(Some(&Billing::Plan(None))), Some(PLAN_CACHE_MS));
        assert_eq!(cache_life(None), Some(PLAN_CACHE_MS));
    }

    #[test]
    fn the_last_activity_is_the_last_turn_or_message() {
        let turn_end = Item::TurnEnd {
            at: 100,
            took_secs: 1,
        };
        let said = Item::Assistant { text: "hi".into() };
        let user = |at: Option<i64>| Item::User {
            text: "a".into(),
            images: vec![],
            at,
            resume: None,
            aside: false,
        };
        assert_eq!(last_active(&[user(Some(5)), said.clone()]), Some(5));
        assert_eq!(
            last_active(&[user(Some(5)), turn_end.clone(), said.clone()]),
            Some(100)
        );
        assert_eq!(
            last_active(&[turn_end, user(Some(200)), said.clone()]),
            Some(200)
        );
        assert_eq!(last_active(&[user(None), said.clone()]), None);
        assert_eq!(last_active(&[said]), None);
    }

    #[test]
    fn the_tooltip_says_why_by_how_its_billed() {
        let offer = |life| CompactOffer {
            used: 84_000,
            window: 200_000,
            last_active: 0,
            life,
        };
        assert!(
            offer(KEY_CACHE_MS)
                .why()
                .contains("5 minutes on an API key")
        );
        assert!(
            offer(KEY_CACHE_MS)
                .why()
                .contains("all 84k tokens at full price")
        );
        assert!(
            offer(PLAN_CACHE_MS)
                .why()
                .contains("an hour on a subscription")
        );
        assert!(!offer(KEY_CACHE_MS).lapsed(KEY_CACHE_MS - 1));
        assert!(offer(KEY_CACHE_MS).lapsed(KEY_CACHE_MS));
    }
}
