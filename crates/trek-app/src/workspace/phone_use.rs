//! iPhone use, the workspace's part: the `trek-phone` server every session gets on macOS, its
//! calls (over Trek's local channel, as the browser tools' are) handed to the main window's iPhone
//! tool (`panels::phone::PhoneCall`). What needs the user's approval goes to the thread's card
//! through the same plumbing as the Browser's (`Workspace::browser_ask`).

use super::{Mode, Workspace, WorkspaceEvent};
use crate::ipc::{Call, Reply};
use gpui_kit::Context;
use trek_agents::McpServer;

/// How long a phone tool call may take: an install, or an approval that waits for the user.
const TOOL_TIMEOUT_SECS: u64 = 1200;

/// One of the phone tools' calls, on its way to the main window's iPhone tool.
pub struct PhoneCall {
    pub thread: String,
    /// The thread's title, for the strip and for other agents told who holds the phone.
    pub title: String,
    pub method: String,
    pub params: serde_json::Value,
    /// The thread's access level: what needs asking.
    pub access: trek_core::HandHolding,
    /// When the thread's current turn started: the user takes the phone back from a turn.
    pub turn: Option<std::time::Instant>,
    /// The iPhone tool may come forward to show it.
    pub follow: bool,
    reply: Option<async_channel::Sender<Reply>>,
}

impl PhoneCall {
    pub fn answer(mut self, result: Result<serde_json::Value, String>) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.try_send(Reply::Done(result));
        }
    }
}

impl Drop for PhoneCall {
    fn drop(&mut self) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.try_send(Reply::Done(Err(
                "Trek's iPhone tool didn't get to this call (its window or the tool closed). Try again.".into(),
            )));
        }
    }
}

/// A call for the main window's iPhone tool, which takes it out (`WorkspaceEvent`s go to every
/// window).
pub type PhoneCallSlot = std::rc::Rc<std::cell::RefCell<Option<PhoneCall>>>;

/// Trek's phone server, for a session with the channel environment `env`.
pub(super) fn server(bin: String, env: Vec<(String, String)>) -> McpServer {
    McpServer {
        tool_timeout_secs: Some(TOOL_TIMEOUT_SECS),
        ..McpServer::stdio(trek_ipc::phone::SERVER, bin, vec!["phone".into()], env)
    }
}

impl Workspace {
    /// Whether sessions get Trek's iPhone tools: on in settings, on a Mac.
    pub(super) fn phone_tools(&self) -> bool {
        self.settings.tools.phone && cfg!(target_os = "macos")
    }

    /// A phone tool call from `call.thread`'s agent: to the main window's iPhone tool.
    pub(super) fn phone_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let fail = |call: Call, why: &str| {
            let _ = call.reply.try_send(Reply::Done(Err(why.to_string())));
        };
        let Some(thread) = self.thread(&call.thread) else {
            return fail(call, "This thread isn't in Trek any more.");
        };
        if !self.phone_tools() {
            return fail(
                call,
                "The iPhone tools are off in Trek's settings (Tools & MCP), or this isn't a Mac.",
            );
        }
        if self.main_window.is_none() {
            return fail(
                call,
                "Trek's main window is closed, and the iPhone tool is in it: ask the user to open it.",
            );
        }
        let follow = self.settings.tools.phone_follow
            && self.mode == Mode::Agents
            && self.in_main(&call.thread);
        let title = if thread.title.trim().is_empty() {
            "A thread".to_string()
        } else {
            thread.title.clone()
        };
        let phone = PhoneCall {
            thread: call.thread.clone(),
            title,
            method: call.method,
            params: call.params,
            access: thread.hand_holding,
            turn: self.live.get(&call.thread).and_then(|l| l.turn_started),
            follow,
            reply: Some(call.reply),
        };
        cx.emit(WorkspaceEvent::PhoneUse(std::rc::Rc::new(
            std::cell::RefCell::new(Some(phone)),
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_runs_trek_mcp_phone_with_time_for_an_install() {
        let s = server(
            "/x/trek-mcp".into(),
            vec![("TREK_IPC_SESSION".into(), "k".into())],
        );
        assert_eq!(s.name, "trek-phone");
        assert_eq!(s.tool_timeout_secs, Some(TOOL_TIMEOUT_SECS));
        match &s.transport {
            trek_agents::McpTransport::Stdio { args, env, .. } => {
                assert_eq!(args, &["phone".to_string()]);
                assert_eq!(env[0].1, "k");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unanswered_call_says_so() {
        let (tx, rx) = async_channel::bounded(1);
        drop(PhoneCall {
            thread: "t".into(),
            title: "T".into(),
            method: "phone_tap".into(),
            params: serde_json::json!({}),
            access: trek_core::HandHolding::Auto,
            turn: None,
            follow: false,
            reply: Some(tx),
        });
        match rx.try_recv().unwrap() {
            Reply::Done(Err(e)) => assert!(e.contains("didn't get to this call")),
            other => panic!("{other:?}"),
        }
    }
}
