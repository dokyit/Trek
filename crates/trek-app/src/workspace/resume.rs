//! Chats a quit or a crash cut off (`trek_core::interrupted`): offered back at the next launch,
//! resumed when the user says so (or at launch, if they turned that on), with a visible state
//! until the agent is back and a Retry if it doesn't come. A thread stopped or closed on purpose
//! has nothing here: its turn ended before Trek did. One that was waiting on the user comes back
//! waiting: the card went with the session, so asking again is the user's call.
//!
//! What an agent can really do: Claude Code and Codex reopen their own session (`--resume`,
//! `thread/resume`); OpenCode, Droid and ACP agents reopen theirs if they advertise it
//! (`session/load`); direct models keep no session, so they start again from a recap. Any of them
//! whose session has gone starts again from the recap too. In every case the agent gets a message
//! and a note: tool calls aren't replayed, as Trek can't know how far one got.

use super::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::time::Duration;
use trek_core::interrupted::{self, Interrupted};
use trek_core::store::{Item, Thread};
use trek_core::{AgentId, RunState};

/// What's being done about a cut-off turn.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeState {
    /// Waiting for the user (or for the launch's automatic resume).
    Offered,
    /// A resume went, and the agent hasn't said anything yet.
    Resuming,
    /// The agent is back at it. Kept until its turn ends: a thread that goes down with Trek again
    /// has its tries counted.
    Going,
    /// The resume failed: why, and Retry.
    Failed(String),
}

/// A chat cut off, and where its offer stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub turn: Interrupted,
    pub state: ResumeState,
}

impl Offer {
    /// Waiting for the user's word.
    pub fn open(&self) -> bool {
        matches!(self.state, ResumeState::Offered | ResumeState::Failed(_))
    }
}

/// What the sidebar's notice lists, in the order the threads were cut off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InterruptedNotice {
    /// Cut off working: Resume all covers them.
    pub working: Vec<String>,
    /// Cut off waiting on the user: Resume all leaves them.
    pub waiting: Vec<String>,
}

impl InterruptedNotice {
    pub fn headline(&self) -> Option<String> {
        interrupted::headline(self.working.len(), self.waiting.len())
    }

    pub fn waiting_too(&self) -> Option<String> {
        interrupted::waiting_too(self.working.len(), self.waiting.len())
    }
}

/// How `t`'s agent comes back: what the user can expect, honestly.
pub fn how_it_resumes(t: &Thread) -> String {
    if t.native_id.is_none() {
        return "Starts again from a recap of this thread".into();
    }
    let name = t.agent.display_name();
    match &t.agent {
        AgentId::ClaudeCode | AgentId::Codex | AgentId::Direct(_) => {
            format!("Reopens its {name} session")
        }
        AgentId::OpenCode | AgentId::Droid | AgentId::Acp(_) => {
            format!("Reopens its {name} session if it can, or starts from a recap")
        }
    }
}

impl Workspace {
    /// At launch, once threads are in: what the last run left open, and any left from before it.
    /// A thread deleted since has nothing to come back to.
    pub(super) fn load_interrupted(&mut self, cx: &mut Context<Self>) {
        let turns = self.store.interrupted_turns().unwrap_or_else(|e| {
            tracing::warn!("read interrupted turns: {e:#}");
            vec![]
        });
        for turn in turns {
            if self.thread(&turn.thread).is_none() {
                let _ = self.store.drop_interrupted(&turn.thread);
                continue;
            }
            self.interrupted.insert(
                turn.thread.clone(),
                Offer {
                    turn,
                    state: ResumeState::Offered,
                },
            );
        }
        self.resume_at_launch(cx);
    }

    /// Resuming at launch turned on: the chats it covers go once the windows are up and the agents
    /// found (their CLIs may be mid-update, which messages wait for), with a word.
    fn resume_at_launch(&mut self, cx: &mut Context<Self>) {
        if !self.settings.general.resume_interrupted {
            return;
        }
        let now = self.now();
        let due: Vec<String> = self.resumable(now, |o| o.turn.may_auto_resume());
        if due.is_empty() {
            return;
        }
        let message = match due.len() {
            1 => "Resuming 1 interrupted thread.".to_string(),
            n => format!("Resuming {n} interrupted threads."),
        };
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(super::limits::LAUNCH_DELAY_MS as u64))
                .await;
            let _ = this.update(cx, |ws, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message,
                    undo: None,
                });
                for id in due {
                    ws.resume_interrupted(&id, cx);
                }
            });
        });
        self.keep(task);
    }

    /// The threads Resume all covers that also pass `and`, oldest first.
    fn resumable(&self, now: i64, and: impl Fn(&Offer) -> bool) -> Vec<String> {
        let mut offers: Vec<&Offer> = self
            .interrupted
            .values()
            .filter(|o| o.open() && !o.turn.waiting && and(o))
            .filter(|o| {
                self.thread(&o.turn.thread)
                    .is_some_and(|t| interrupted::listed(t, now))
            })
            .collect();
        offers.sort_by_key(|o| (o.turn.at, o.turn.thread.clone()));
        offers.into_iter().map(|o| o.turn.thread.clone()).collect()
    }

    /// `id`'s offer, if its last turn was cut off and that isn't dealt with yet.
    pub fn interrupted_offer(&self, id: &str) -> Option<&Offer> {
        self.interrupted.get(id)
    }

    /// What the sidebar's notice says. Threads being resumed have left it.
    pub fn interrupted_notice(&self) -> InterruptedNotice {
        let now = self.now();
        let mut waiting: Vec<&Offer> = self
            .interrupted
            .values()
            .filter(|o| o.turn.waiting && o.state == ResumeState::Offered)
            .filter(|o| {
                self.thread(&o.turn.thread)
                    .is_some_and(|t| interrupted::listed(t, now))
            })
            .collect();
        waiting.sort_by_key(|o| (o.turn.at, o.turn.thread.clone()));
        InterruptedNotice {
            working: self.resumable(now, |_| true),
            waiting: waiting.iter().map(|o| o.turn.thread.clone()).collect(),
        }
    }

    /// The message a resume sends: the user's own wording if they set one.
    pub fn resume_message(&self) -> String {
        match self.settings.general.resume_message.trim() {
            "" => interrupted::RESUME_MESSAGE.to_string(),
            text => text.to_string(),
        }
    }

    /// Resume, or Retry: the agent's session reopens where it can (`ensure_session`, with the
    /// recap to fall back on) and it's sent the resume message.
    pub fn resume_interrupted(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.interrupted.get(id).is_some_and(Offer::open) {
            return;
        }
        // Picked up some other way meanwhile: nothing more to send.
        if self.turn_running(id) {
            self.forget_interrupted(id);
            cx.notify();
            return;
        }
        // No folder to run in: it would wait where nothing says for what.
        let missing = self
            .thread(id)
            .is_some_and(|t| t.worktree.as_ref().is_some_and(|w| w.is_missing()));
        if missing {
            if let Some(o) = self.interrupted.get_mut(id) {
                o.state = ResumeState::Failed(
                    "Its worktree is missing. Restore it, then try again.".into(),
                );
            }
            cx.notify();
            return;
        }
        if let Some(o) = self.interrupted.get_mut(id) {
            o.state = ResumeState::Resuming;
            o.turn.tries += 1;
        }
        if let Err(e) = self.store.interrupted_tried(id) {
            tracing::warn!("note a resume of {id}: {e:#}");
        }
        let text = self.resume_message();
        self.send_to(id, text, vec![], cx);
        self.resumed_work_changed(id, cx);
    }

    /// Resume all: every chat cut off working. One waiting on the user stays waiting.
    pub fn resume_all_interrupted(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        for id in self.resumable(now, |_| true) {
            self.resume_interrupted(&id, cx);
        }
    }

    /// The user is leaving `id` as it is: it stays stopped.
    pub fn dismiss_interrupted(&mut self, id: &str, cx: &mut Context<Self>) {
        self.forget_interrupted(id);
        cx.notify();
    }

    /// The sidebar notice's ✕: every chat it lists stays as it is.
    pub fn dismiss_all_interrupted(&mut self, cx: &mut Context<Self>) {
        let notice = self.interrupted_notice();
        for id in notice.working.iter().chain(&notice.waiting) {
            self.forget_interrupted(id);
        }
        cx.notify();
    }

    /// The offer's over, whatever became of it.
    pub(super) fn forget_interrupted(&mut self, id: &str) {
        if self.interrupted.remove(id).is_some()
            && let Err(e) = self.store.drop_interrupted(id)
        {
            tracing::warn!("drop interrupted turn of {id}: {e:#}");
        }
    }

    /// The first message after a cut-off turn goes with a note for the agent (the transcript has
    /// the message as sent). Whoever sends it, the offer has been taken up.
    pub(super) fn resume_prompt(&mut self, id: &str, text: String) -> String {
        // A command (`/compact`) goes as typed, or the agent wouldn't know it for one: the note
        // waits for the next message, and the offer stays open.
        if text.trim_start().starts_with('/') {
            return text;
        }
        let Some(offer) = self.interrupted.get_mut(id) else {
            return text;
        };
        match offer.state {
            ResumeState::Going => text,
            ResumeState::Resuming => interrupted::prompt(&text, offer.turn.waiting),
            ResumeState::Offered | ResumeState::Failed(_) => {
                offer.state = ResumeState::Going;
                interrupted::prompt(&text, offer.turn.waiting)
            }
        }
    }

    /// The agent has said something since it was resumed: it's back.
    pub(super) fn resume_took(&mut self, id: &str) {
        if let Some(o) = self
            .interrupted
            .get_mut(id)
            .filter(|o| o.state == ResumeState::Resuming)
        {
            o.state = ResumeState::Going;
        }
    }

    /// A turn of `id`'s ended. One that followed a resume: done, or stopped by the user (who is
    /// then leaving it stopped), or stopped at a usage limit (its pause takes it from here), the
    /// offer's over; failed, it stays, with why and a Retry.
    pub(super) fn resume_turn_ended(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self
            .interrupted
            .get(id)
            .is_some_and(|o| matches!(o.state, ResumeState::Resuming | ResumeState::Going))
        {
            return;
        }
        if self.thread(id).map(|t| t.run_state) == Some(RunState::Failed) {
            let why = self
                .live
                .get(id)
                .and_then(|l| {
                    l.items.iter().rev().find_map(|i| match i {
                        Item::Error { text } => Some(text.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_else(|| "The agent stopped unexpectedly.".into());
            if let Some(o) = self.interrupted.get_mut(id) {
                o.state = ResumeState::Failed(why);
            }
        } else {
            self.forget_interrupted(id);
        }
        self.resumed_work_changed(id, cx);
    }

    fn resumed_work_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(l) = self.live.get_mut(id) {
            l.revision += 1;
        }
        cx.notify();
    }

    /// As Trek quits: a turn the user asked to stop, and the agent hadn't yet, was stopped on
    /// purpose: it ends here rather than be offered back. One whose turn is over with its agent's
    /// own sub-agents still out was at work all the same: it's noted, and says what stopped.
    pub(super) fn settle_turns_for_quit(&mut self) {
        let now = self.now();
        let mut stopped = vec![];
        let mut out = vec![];
        for (id, live) in self
            .live
            .iter_mut()
            .filter(|(id, _)| !crate::machines::is_remote(id))
        {
            if live.turn_started.is_some() && live.turn_started == live.stop_asked {
                live.turn_started = None;
                live.close_turn(false);
                live.streaming = None;
                live.reasoning = None;
                live.items.push(Item::Notice {
                    text: "Interrupted".into(),
                });
                stopped.push(id.clone());
            } else if live.turn_started.is_none()
                && live.commands.is_some()
                && live.background_agents().next().is_some()
            {
                live.lose_background();
                out.push(id.clone());
            }
        }
        for id in stopped {
            if let Err(e) = self
                .store
                .update_thread(&id, |t| t.run_state = RunState::Idle)
            {
                tracing::warn!("settle stopped turn of {id}: {e:#}");
            }
        }
        for id in out {
            let top_level = self.thread(&id).is_some_and(|t| t.parent_id.is_none());
            if top_level && let Err(e) = self.store.note_interrupted(&id, false, None, now) {
                tracing::warn!("note interrupted {id}: {e:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::{Effort, HandHolding, ThreadSource};

    fn thread(agent: AgentId, native: Option<&str>) -> Thread {
        let store = trek_core::store::Store::in_memory().unwrap();
        let mut t = store
            .create_thread(None, agent, None, Effort::Medium, HandHolding::Auto)
            .unwrap();
        t.native_id = native.map(str::to_string);
        t.source = ThreadSource::Trek;
        t
    }

    #[test]
    fn each_agent_says_only_what_it_can_do() {
        let says = |agent, native| how_it_resumes(&thread(agent, native));
        assert_eq!(
            says(AgentId::ClaudeCode, Some("s")),
            "Reopens its Claude Code session"
        );
        assert_eq!(says(AgentId::Codex, Some("s")), "Reopens its Codex session");
        assert_eq!(
            says(AgentId::OpenCode, Some("s")),
            "Reopens its OpenCode session if it can, or starts from a recap",
            "ACP agents only if they advertise it"
        );
        assert_eq!(
            says(AgentId::Direct("anthropic".into()), None),
            "Starts again from a recap of this thread",
            "direct models keep no session"
        );
        assert_eq!(
            says(AgentId::ClaudeCode, None),
            "Starts again from a recap of this thread",
            "a thread that changed agent left its session"
        );
    }
}
