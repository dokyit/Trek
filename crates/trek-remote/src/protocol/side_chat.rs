//! A side chat from the phone (feature `side_chat`): a quick question beside a thread, in the same
//! folder and a session of its own, which doesn't disturb the thread's agent. On the Mac it's the
//! Side Chat panel's; on the phone a sheet over the thread. A side chat is a thread no list holds:
//! `side_chat` finds a thread's (or starts one with its first message) and answers with its row;
//! from there it's read, sent to, answered and stopped by its id like any other thread
//! (`subscribe`, `send`, `answer`, `interrupt`), and its row follows as `unlisted` while a phone
//! has it open.

use super::{ImageUpload, ThreadSummary};
use serde::{Deserialize, Serialize};

/// `side_chat`: the side chat beside `thread_id`. Without `text`, the newest one there is (none:
/// the reply has no `side`). With `text`, a new one starts beside the thread with that as its
/// first message, on the thread's agent and model, asking before every edit and command as a side
/// chat started on the Mac does.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SideChatRequest {
    /// The thread it's beside.
    pub thread_id: String,
    /// The first message of a new side chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Photos with that message, as `send.images`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageUpload>,
}

/// The reply to `side_chat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideChat {
    /// The thread it's beside, as asked.
    pub thread_id: String,
    /// The side chat's row: the one found, or the one just started. Missing when the thread has
    /// none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<ThreadSummary>,
}
