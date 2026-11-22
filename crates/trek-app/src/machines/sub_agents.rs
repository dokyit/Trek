//! A thread on another machine at work beyond its own turn, as the thread here shows one: its
//! sub-agents' rows (what the machine says of each on its call's item, `SubAgentLink`, kept in
//! `LiveThread::machine_subs`), the working bar's "Waiting on 2 sub-agents" and its list
//! (`Workspace::sub_agents_at_work`, from the row's `sub_agents`, as the sidebar's badge), and
//! what its agent runs in the background (the row's `background_tasks`, as the strip above the
//! composer). Steering any of them goes to the machine: a sub-agent of Trek's is its own thread
//! there (stopped as one, `interrupt`), one of the agent's own is stopped or sent a message by
//! the call it works under (`sub_agent_action`, feature `sub_agent_view`), and a background task
//! is read and stopped by its id (`background_output`, `stop_background`).

use super::split_id;
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use trek_core::orchestrate as orch;
use trek_remote as tr;

impl Workspace {
    /// `stop_sub_agent` for a thread on another machine.
    pub(crate) fn machine_stop_sub_agent(
        &mut self,
        thread: &str,
        row: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (host, _) = split_id(thread).ok_or("No such thread.")?;
        let host = host.to_string();
        if let Some(why) = self.machine_unavailable_why(&host) {
            return Err(why);
        }
        if let Some(child) = orch::task_of_row(row) {
            // Trek's own is a thread of its own there: stopped as any thread is.
            let child = super::local_id(&host, child);
            self.machine_interrupt(&child, cx);
            return Ok(());
        }
        self.machine_sub_agent_action(thread, row, tr::SubAgentAction::Stop, None, cx)
    }

    /// `message_sub_agent` for a thread on another machine.
    pub(crate) fn machine_message_sub_agent(
        &mut self,
        thread: &str,
        row: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if orch::task_of_row(row).is_some() {
            return Err("Write to its thread instead.".into());
        }
        let agent = self
            .thread(thread)
            .map(|t| t.agent.clone())
            .ok_or("No such thread.")?;
        if !trek_agents::SubAgentControl::of(&agent).message {
            return Err(trek_agents::SubAgentControl::no_message(&agent));
        }
        self.machine_sub_agent_action(
            thread,
            row,
            tr::SubAgentAction::Message,
            Some(text.to_string()),
            cx,
        )
    }

    /// `sub_agent_action` on the agent's own sub-agent working under call `row` of machine thread
    /// `thread`; what the machine refuses is said in a toast.
    fn machine_sub_agent_action(
        &mut self,
        thread: &str,
        row: &str,
        action: tr::SubAgentAction,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (host, tid) = split_id(thread).ok_or("No such thread.")?;
        let m = self.machines.get(host).ok_or("No such thread.")?;
        if let Some(why) = self.machine_unavailable_why(host) {
            return Err(why);
        }
        if !m.entry.paired.serves(tr::feature::SUB_AGENT_VIEW) {
            return Err(format!(
                "{} needs a newer Trek to steer its sub-agents from here.",
                m.entry.name
            ));
        }
        if !self
            .live
            .get(thread)
            .and_then(|l| l.machine_subs.get(row))
            .is_some_and(|s| s.state == tr::SubAgentState::Running)
        {
            return Err("It has already finished.".into());
        }
        let what = match action {
            tr::SubAgentAction::Stop => "stop the sub-agent",
            tr::SubAgentAction::Message => "send the sub-agent a message",
        };
        let req = tr::SubAgentActionRequest {
            thread_id: tid.to_string(),
            call_id: row.to_string(),
            action,
            text,
        };
        self.machine_request(
            thread,
            what,
            move |_| tr::ClientMessage::SubAgentAction(req),
            cx,
        );
        Ok(())
    }

    /// `read_background` for a thread on another machine: the end of the task's output, asked of
    /// the machine, shows in the strip's popover once it's in.
    pub(crate) fn machine_read_background(
        &mut self,
        thread: &str,
        task: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((host, tid)) = split_id(thread) else {
            return;
        };
        let readable = self.live.get(thread).is_some_and(|l| {
            l.background
                .iter()
                .any(|b| b.task.id == task && b.task.readable)
        });
        if !readable || self.machine_unavailable_why(host).is_some() {
            return;
        }
        let Some(client) = self.machines.get(host).and_then(|m| m.client.clone()) else {
            return;
        };
        let msg = tr::ClientMessage::BackgroundOutput {
            thread_id: tid.to_string(),
            task_id: task.to_string(),
        };
        let (thread, task) = (thread.to_string(), task.to_string());
        cx.spawn(async move |this, cx| {
            let Ok(tr::ServerMessage::BackgroundOutput(out)) = client.request(msg).await else {
                return;
            };
            let _ = this.update(cx, |ws, cx| {
                let Some(b) = ws
                    .live
                    .get_mut(&thread)
                    .and_then(|l| l.background.iter_mut().find(|b| b.task.id == task))
                else {
                    return;
                };
                if let Some(output) = out.output {
                    b.output = Some(output);
                }
                b.output_rev += 1;
                cx.emit(WorkspaceEvent::Background { id: thread.clone() });
                cx.notify();
            });
        })
        .detach();
    }

    /// `stop_background` for a thread on another machine: asked of the machine.
    pub(crate) fn machine_stop_background(
        &mut self,
        thread: &str,
        task: String,
        cx: &mut Context<Self>,
    ) {
        self.machine_request(
            thread,
            "stop it",
            move |thread_id| tr::ClientMessage::StopBackground {
                thread_id,
                task_id: task,
            },
            cx,
        );
    }
}
