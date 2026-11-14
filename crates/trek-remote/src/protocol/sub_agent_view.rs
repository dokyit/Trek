//! A sub-agent opened from its row in a thread (feature `sub_agent_view`).
//!
//! A tool item that stands for a sub-agent carries [`SubAgentLink`] (`sub_agent`): who it is,
//! where it stands, and how it opens. One Trek runs (`delegate_task`) has a thread of its own
//! (`thread_id`): it opens like any other (`subscribe`), and is sent to, answered and stopped by
//! its id (`send`, `answer`, `interrupt`). One of the agent's own (Claude Code's Task, Codex's
//! agents) lives in its parent's session: `sub_agent` reads what it has done so far (its prompt,
//! what it said, its calls and their output), again as often as the phone likes while it works,
//! and `sub_agent_action` stops it or sends it a message where its agent can take one
//! ([`SubAgentView::can_stop`], [`SubAgentView::can_message`]; [`SubAgentView::no_message`] says
//! why not, in the words the Mac uses). What it asks for (an approval, a question) waits on its
//! parent's thread, as on the Mac, and is answered there.
//!
//! An older Mac sends no `sub_agent` on tool items and lists no `sub_agent_view`: the phone shows
//! the row as before. An older phone ignores the field.

use super::{AgentRef, Item, SubAgentState};
use serde::{Deserialize, Serialize};

/// On a tool item: the sub-agent it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentLink {
    /// Whose logo it wears: the agent running it.
    pub agent: AgentRef,
    /// "Sol: Review the cache design", or what the agent's own was sent to do.
    pub label: String,
    /// Where it stands, and what it's doing or why it failed ("Reading src/auth.rs · 4 steps").
    #[serde(default)]
    pub detail: String,
    pub state: SubAgentState,
    /// When it started (ms), while this Mac has seen it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// How long it worked, once it's done (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took_secs: Option<u32>,
    /// Its thread, for one Trek runs: opened with `subscribe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// `sub_agent`: what the agent's own sub-agent started by call `call_id` of thread `thread_id`
/// has done so far.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubAgentRequest {
    pub thread_id: String,
    pub call_id: String,
}

/// The reply to `sub_agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentView {
    /// As asked.
    pub thread_id: String,
    pub call_id: String,
    /// As its row has it.
    pub link: SubAgentLink,
    /// What it was told to do, whole, when its agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// What it has done, oldest first: `assistant` for what it said, `tool` for each call (with
    /// its output once it has some). Ids are the Mac's (`s0`, `s1`…) and stay put while it works.
    #[serde(default)]
    pub items: Vec<Item>,
    /// Earlier ones left out (the Mac keeps the latest few hundred).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub earlier: u32,
    /// Its answer, once it's done (what its parent got back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// `sub_agent_action` `stop` works now.
    #[serde(default)]
    pub can_stop: bool,
    /// `sub_agent_action` `message` works now.
    #[serde(default)]
    pub can_message: bool,
    /// Why there's no message to send it, when there isn't ("Claude Code doesn't take messages
    /// for its sub-agents while they run. Write to the thread, and its agent can pass it on.").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_message: Option<String>,
    /// It waits on the user: its request is on its parent's thread, answered there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
    /// The Mac no longer has it (its session ended, or Trek restarted since): `items` is empty,
    /// and its row's answer is all there is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `sub_agent_action`: stop the agent's own sub-agent, or send it a message. Answered with
/// `sub_agent`, as it stands after.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubAgentActionRequest {
    pub thread_id: String,
    pub call_id: String,
    pub action: SubAgentAction,
    /// The message, for `message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentAction {
    #[default]
    Stop,
    Message,
}
