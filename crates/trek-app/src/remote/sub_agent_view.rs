//! A sub-agent opened from its row on the phone (feature `sub_agent_view`; the wire is
//! `trek_remote`'s `protocol/sub_agent_view.rs`). A tool item that stands for one carries what the
//! Mac's row says (`SubAgentRow`); one of the agent's own reads as a transcript of its own from
//! what its agent reported (`SubLog`), and is stopped or sent a message as its row on the Mac is
//! (`Workspace::stop_sub_agent`, `Workspace::message_sub_agent`).

use super::{agent_ref, clip_text, clip_text_to, instant_ms};
use crate::thread_view::SubAgentRow;
use crate::workspace::{LiveThread, TaskState, Workspace};
use gpui_kit::Context;
use trek_agents::SubAgentControl;
use trek_core::orchestrate as orch;
use trek_core::store::{Item, ToolStatus};
use trek_remote as tr;

/// The sub-agent `item` of `thread` stands for, as its row on the Mac shows it; `None` for any
/// other item.
pub(super) fn link(
    ws: &Workspace,
    thread: &str,
    live: &LiveThread,
    item: &Item,
) -> Option<tr::SubAgentLink> {
    let Item::Tool {
        id,
        title,
        detail,
        output,
        status,
    } = item
    else {
        return None;
    };
    if orch::task_of_row(id).is_none() && title != "Subagent" {
        return None;
    }
    let agent = ws.thread(thread)?.agent.clone();
    let sub = SubAgentRow::read(ws, thread, &agent, id, detail, output, *status);
    let own = live.tasks.iter().find(|t| t.id == *id);
    let (since, took_secs) = match own {
        Some(t) => (
            t.done.is_none().then(|| instant_ms(t.started)),
            t.ended
                .map(|e| e.saturating_duration_since(t.started).as_secs() as u32),
        ),
        // Trek's own: while it works, when its clock started, so the phone counts on from what
        // the Mac's row shows (a usage limit's pause aside); once it's over, what it worked.
        None => match sub.elapsed {
            Some(d) if sub.state.live() => (Some(started_ago(d)), None),
            Some(d) => (None, Some(d.as_secs() as u32)),
            None => (None, None),
        },
    };
    Some(tr::SubAgentLink {
        agent: agent_ref(sub.agent.as_ref().unwrap_or(&agent)),
        label: clip_text_to(&sub.label, 1 << 10),
        detail: clip_text_to(&sub.detail, 1 << 10),
        state: state(sub.state),
        since,
        took_secs,
        thread_id: sub.child,
    })
}

/// When (ms, to the second, as `instant_ms` gives it) a clock that has run for `worked` started:
/// the same from one read to the next while it runs, so its item doesn't change every time.
fn started_ago(worked: std::time::Duration) -> i64 {
    let ms = trek_core::store::now_ms() - worked.as_millis() as i64;
    (ms + 500).div_euclid(1000) * 1000
}

pub(super) fn state(s: TaskState) -> tr::SubAgentState {
    match s {
        TaskState::Running => tr::SubAgentState::Running,
        TaskState::NeedsYou => tr::SubAgentState::NeedsYou,
        TaskState::Done => tr::SubAgentState::Done,
        TaskState::Failed => tr::SubAgentState::Failed,
        TaskState::Cancelled => tr::SubAgentState::Stopped,
    }
}

impl Workspace {
    /// `sub_agent`: what the agent's own sub-agent on call `call_id` has done so far.
    pub(super) fn remote_sub_agent(
        &self,
        req: &tr::SubAgentRequest,
    ) -> tr::HostResult<tr::SubAgentView> {
        let thread = self
            .thread(&req.thread_id)
            .ok_or_else(|| tr::HostError::not_found("No such thread"))?;
        let live = self
            .live
            .get(&req.thread_id)
            .ok_or_else(|| tr::HostError::not_found("That thread isn't open on the Mac"))?;
        let row = live
            .items
            .iter()
            .rev()
            .find(|i| matches!(i, Item::Tool { id, .. } if *id == req.call_id))
            .ok_or_else(|| tr::HostError::not_found("No such sub-agent"))?;
        let link = link(self, &req.thread_id, live, row)
            .ok_or_else(|| tr::HostError::not_found("That row isn't a sub-agent"))?;
        if link.thread_id.is_some() {
            return Err(tr::HostError::bad_request(
                "Trek's own sub-agents open as threads (`subscribe`)",
            ));
        }
        let cwd = thread.cwd.as_deref();
        let log = live.sub_logs.get(&req.call_id);
        let items = log
            .map(|l| {
                l.items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| {
                        let n = l.dropped + i;
                        Some(tr::Item {
                            id: format!("s{n}"),
                            seq: n as u64,
                            at: None,
                            body: body(item, cwd)?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let running = live
            .tasks
            .iter()
            .any(|t| t.id == req.call_id && t.done.is_none());
        let control = SubAgentControl::of(&thread.agent);
        let in_background = live
            .background_agents()
            .any(|b| b.task.call.as_deref() == Some(req.call_id.as_str()) && b.task.stoppable);
        let answer = match row {
            Item::Tool { output, .. } if !running && !output.trim().is_empty() => {
                Some(clip_text(output))
            }
            _ => None,
        };
        Ok(tr::SubAgentView {
            thread_id: req.thread_id.clone(),
            call_id: req.call_id.clone(),
            prompt: log.and_then(|l| l.prompt.as_deref()).map(clip_text),
            earlier: log.map_or(0, |l| l.dropped as u32),
            gone: log.is_none() && !running,
            answer,
            can_stop: running && live.commands.is_some() && (control.stop || in_background),
            can_message: running && live.commands.is_some() && control.message,
            no_message: (!control.message).then(|| SubAgentControl::no_message(&thread.agent)),
            asking: running && !live.permissions.is_empty(),
            items,
            link,
        })
    }

    /// `sub_agent_action`: stop it, or send it a message; then where it stands.
    pub(super) fn remote_sub_agent_action(
        &mut self,
        req: tr::SubAgentActionRequest,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<tr::SubAgentView> {
        let done = match req.action {
            tr::SubAgentAction::Stop => self.stop_sub_agent(&req.thread_id, &req.call_id, cx),
            tr::SubAgentAction::Message => self.message_sub_agent(
                &req.thread_id,
                &req.call_id,
                req.text.as_deref().unwrap_or_default(),
                cx,
            ),
        };
        done.map_err(tr::HostError::bad_request)?;
        self.remote_sub_agent(&tr::SubAgentRequest {
            thread_id: req.thread_id,
            call_id: req.call_id,
        })
    }
}

/// One item of a sub-agent's transcript as the phone shows it.
fn body(item: &Item, cwd: Option<&std::path::Path>) -> Option<tr::ItemBody> {
    Some(match item {
        Item::User { text, .. } => tr::ItemBody::User {
            text: clip_text(text),
            images: 0,
            aside: false,
            videos: vec![],
        },
        Item::Assistant { text } => tr::ItemBody::Assistant {
            text: clip_text(text),
            streaming: false,
        },
        Item::Tool {
            id,
            title,
            detail,
            output,
            status,
        } => {
            let op = crate::activity::op(title, detail, cwd);
            tr::ItemBody::Tool {
                call_id: id.clone(),
                tool: tr::ToolKind::from_title(title),
                title: clip_text_to(if op.verb.is_empty() { title } else { &op.verb }, 1 << 10),
                detail: clip_text_to(if op.text.is_empty() { detail } else { &op.text }, 16 << 10),
                status: match status {
                    ToolStatus::Running => tr::ToolStatus::Running,
                    ToolStatus::Done => tr::ToolStatus::Done,
                    ToolStatus::Failed => tr::ToolStatus::Failed,
                    ToolStatus::Denied => tr::ToolStatus::Denied,
                },
                output: tr::truncate_output(output).to_string(),
                added: None,
                removed: None,
                watch: None,
                sub_agent: None,
            }
        }
        _ => return None,
    })
}
