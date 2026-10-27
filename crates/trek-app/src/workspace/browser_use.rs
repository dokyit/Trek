//! Browser use, the workspace's part: the `trek-browser` server every session gets, its calls
//! (over Trek's local channel, as the sub-agent tools' are) handed to the main window's Browser
//! tool (`panels::browser::AgentCall`), and the approvals those calls ask for, which show on the
//! thread's approval card like an agent's own (`browser_ask`; `respond` answers them here).

use super::{Mode, PendingPermission, Workspace, WorkspaceEvent};
use crate::ipc::{Call, Reply};
use crate::panels::browser::AgentCall;
use gpui_kit::Context;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use trek_agents::{Decision, McpServer};
use trek_core::RunState;

/// How long a browser tool call may take: an approval waits for the user.
const TOOL_TIMEOUT_SECS: u64 = 1200;

/// Approval requests Trek's Browser raised: their ids start so, which keeps them apart from an
/// agent's own.
const ASK_PREFIX: &str = "trek-browser-ask-";

/// The answer each approval waits for, by its request id.
static ASKS: LazyLock<Mutex<HashMap<String, async_channel::Sender<Decision>>>> =
    LazyLock::new(Default::default);

/// Trek's browser server, for a session with the channel environment `env`.
pub(super) fn server(bin: String, env: Vec<(String, String)>) -> McpServer {
    McpServer {
        tool_timeout_secs: Some(TOOL_TIMEOUT_SECS),
        ..McpServer::stdio(trek_ipc::browser::SERVER, bin, vec!["browser".into()], env)
    }
}

/// The user answered approval `request_id`: whether it was the Browser's (and is now answered).
pub(super) fn answered(request_id: &str, decision: Decision) -> bool {
    if !request_id.starts_with(ASK_PREFIX) {
        return false;
    }
    if let Some(tx) = ASKS.lock().unwrap().remove(request_id) {
        let _ = tx.try_send(decision);
    }
    true
}

impl Workspace {
    /// Whether sessions get Trek's browser tools: on in settings, and a Browser to drive.
    pub(super) fn browser_tools(&self) -> bool {
        self.settings.tools.browser && cfg!(any(target_os = "macos", windows))
    }

    /// A browser tool call from `call.thread`'s agent: to the main window's Browser.
    pub(super) fn browser_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(&call.thread) else {
            let _ = call.reply.try_send(Reply::Done(Err(
                "This thread isn't in Trek any more.".into()
            )));
            return;
        };
        if !self.browser_tools() {
            let _ = call.reply.try_send(Reply::Done(Err(
                "Browser tools are off in Trek's settings (Tools & MCP).".into(),
            )));
            return;
        }
        if self.main_window.is_none() {
            let _ = call.reply.try_send(Reply::Done(Err(
                "Trek's main window is closed, and the Browser is in it: ask the user to open it."
                    .into(),
            )));
            return;
        }
        // Like the Simulator: it comes forward for the thread the user is looking at.
        let follow = self.settings.tools.browser_follow
            && self.mode == Mode::Agents
            && self.in_main(&call.thread);
        let agent = AgentCall::new(
            call.thread.clone(),
            if thread.title.trim().is_empty() {
                "A thread".to_string()
            } else {
                thread.title.clone()
            },
            call.method,
            call.params,
            thread.hand_holding,
            self.live.get(&call.thread).and_then(|l| l.turn_started),
            follow,
            call.reply,
        );
        cx.emit(WorkspaceEvent::BrowserUse(std::rc::Rc::new(
            std::cell::RefCell::new(Some(agent)),
        )));
    }

    /// Ask the user, on `thread`'s approval card, whether its agent may do what `title` says:
    /// the request's id, and where the answer comes.
    pub fn browser_ask(
        &mut self,
        thread: &str,
        title: String,
        detail: String,
        cx: &mut Context<Self>,
    ) -> (String, async_channel::Receiver<Decision>) {
        static N: AtomicU64 = AtomicU64::new(0);
        let id = format!("{ASK_PREFIX}{}", N.fetch_add(1, Ordering::Relaxed) + 1);
        let (tx, rx) = async_channel::bounded(1);
        let Some(live) = self.live.get_mut(thread) else {
            let _ = tx.try_send(Decision::Deny);
            return (id, rx);
        };
        ASKS.lock().unwrap().insert(id.clone(), tx);
        live.permissions.push(PendingPermission {
            request_id: id.clone(),
            title: title.clone(),
            detail,
            prompt: None,
            after_turn: false,
        });
        live.revision += 1;
        self.mutate_thread(thread, cx, |t| t.run_state = RunState::NeedsYou);
        cx.emit(WorkspaceEvent::Attention {
            message: format!("Needs your approval: {title}"),
            thread: thread.to_string(),
            button: None,
        });
        cx.notify();
        (id, rx)
    }

    /// Whether approval `request` is still on `thread`'s card (a turn that ends takes its card
    /// with it).
    pub fn browser_ask_pending(&self, thread: &str, request: &str) -> bool {
        self.live
            .get(thread)
            .is_some_and(|l| l.permissions.iter().any(|p| p.request_id == request))
    }

    /// The Browser no longer waits on approval `request`: off the card, and the thread back at
    /// work (it's still in its turn) unless something else waits on the user.
    pub fn browser_withdraw(&mut self, thread: &str, request: &str, cx: &mut Context<Self>) {
        ASKS.lock().unwrap().remove(request);
        let Some(live) = self.live.get_mut(thread) else {
            return;
        };
        let had = live.permissions.iter().any(|p| p.request_id == request);
        live.settle_prompt(request);
        live.revision += 1;
        let (waiting, running) = (!live.permissions.is_empty(), live.turn_started.is_some());
        if had && !waiting {
            let state = if running {
                RunState::Working
            } else {
                RunState::Idle
            };
            self.mutate_thread(thread, cx, |t| t.run_state = state);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_browsers_own_requests_are_answered_here() {
        assert!(!answered("toolu_123", Decision::Allow), "an agent's own");
        let (tx, rx) = async_channel::bounded(1);
        let id = format!("{ASK_PREFIX}test-1");
        ASKS.lock().unwrap().insert(id.clone(), tx);
        assert!(answered(&id, Decision::AllowForSession));
        assert_eq!(rx.try_recv().unwrap(), Decision::AllowForSession);
        // Withdrawn already: still the Browser's, nothing to send.
        assert!(answered(&id, Decision::Deny));
    }

    #[test]
    fn the_server_runs_trek_mcp_browser_with_time_for_an_approval() {
        let s = server(
            "/x/trek-mcp".into(),
            vec![("TREK_IPC_SESSION".into(), "k".into())],
        );
        assert_eq!(s.name, "trek-browser");
        assert_eq!(s.tool_timeout_secs, Some(TOOL_TIMEOUT_SECS));
        match &s.transport {
            trek_agents::McpTransport::Stdio { args, env, .. } => {
                assert_eq!(args, &["browser".to_string()]);
                assert_eq!(env[0].1, "k");
            }
            other => panic!("{other:?}"),
        }
    }
}
