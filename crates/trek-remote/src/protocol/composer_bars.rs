//! What the Mac's composer shows over its prompt, for the phone to show it too: the failing-checks
//! bar ("CI failed", with Send to agent and Dismiss), "Continue on <account>" in the limit bar, the
//! Compact chip once a large thread's prompt cache has gone, and the bar of a move to another agent
//! that waits for the next message (with what goes with it).
//!
//! Fields on rows are additive (sent only when there's something in them; an older Mac never sends
//! them, an older phone ignores them). The requests and actions are listed in `welcome`/`paired`
//! `features`: [`super::feature::FAILED_CHECKS`], [`super::feature::CONTINUE_ON_ACCOUNT`],
//! [`super::feature::COMPACT_OFFER`] and [`super::feature::SWITCH_PREVIEW`].

use super::AgentRef;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// `failed_checks`: a pull request the thread opened is failing
// ---------------------------------------------------------------------------------------------

/// A pull request the thread opened has failing checks the user hasn't dealt with
/// (`ThreadSummary::checks`). While the thread is idle (or its turn failed) the row's `needs` is
/// `failed` with the Mac's row words ("CI failed: clippy"), so an older phone lists it under Needs
/// you too.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FailingChecks {
    /// The pull request on GitHub.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// The checks failing, by name.
    #[serde(default)]
    pub failing: Vec<String>,
    /// The Mac's bar is up over the composer: the thread isn't at work or asking.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bar: bool,
    /// Sending by itself used up its tries ("CI still failing").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// The log is being read to send ("Reading the log…"): Send to agent waits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sending: bool,
    /// The thread's project sends failing checks by itself (`send_failed_checks`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

/// `set_settings.send_failed_checks`: turn a project's sending by itself on or off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSwitch {
    pub project_id: String,
    pub on: bool,
}

// ---------------------------------------------------------------------------------------------
// `continue_on_account`: the limit bar's "Continue on <account>"
// ---------------------------------------------------------------------------------------------

/// Another account of the paused thread's agent with room left (`LimitPause::other_account`), the
/// one the Mac's limit bar offers to carry on with.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OtherAccount {
    /// The account's id; `None`: the user's normal login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// What the Mac calls it ("Work", "Default").
    pub label: String,
}

// ---------------------------------------------------------------------------------------------
// `compact_offer`: the Compact chip
// ---------------------------------------------------------------------------------------------

/// A large idle thread compacting would help once its prompt cache has gone
/// (`ThreadSummary::compact_offer`): the next message would re-read the whole context at full
/// price. The phone shows the chip from `lapses_at` on, until it's used or dismissed (for that
/// `last_active`); `thread_action {kind: "compact"}` compacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactOffer {
    /// Tokens in the window, and its size.
    pub used: u64,
    pub window: u64,
    /// When the thread last did anything the provider's cache saw (ms).
    pub last_active: i64,
    /// When the cache goes (ms).
    pub lapses_at: i64,
    /// Why, in the Mac's words (its chip's tooltip).
    #[serde(default)]
    pub why: String,
}

// ---------------------------------------------------------------------------------------------
// `switch_preview`: a move to another agent waits for the next message
// ---------------------------------------------------------------------------------------------

/// The move to another agent (or another account of it) that `id`'s next message makes
/// (`ThreadSummary::pending_switch`), as the Mac's bar says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSwitch {
    /// The agent it moves to.
    pub agent: AgentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model's name ("GPT-5.6 Sol").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Where it goes, as the bar names it: "Codex", "Codex · Work", "the Work account".
    pub to: String,
    /// What it's on now, for "Stay on …".
    pub from: String,
}

/// The reply to `switch_preview`: what the next message carries to the new agent, as the Mac's
/// bar opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchPreview {
    pub thread_id: String,
    /// The handover as it would go now, secrets taken out.
    pub text: String,
    /// Its size (an estimate), and what it may take up.
    pub tokens: u64,
    pub budget: u64,
    /// What was left out, in the Mac's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_out: Option<String>,
}
