//! The bars over the Mac's composer, from a phone: the failing-checks bar (`workspace::pr_checks`),
//! the limit bar's "Continue on <account>" (`workspace::accounts`), the Compact chip
//! (`workspace::compact_offer`) and a move to another agent waiting for the next message
//! (`workspace::switching`). Each rides in the thread's row, and its buttons are thread actions.

use super::agent_ref;
use crate::workspace::{Scope, Workspace};
use gpui_kit::Context;
use trek_core::RunState;
use trek_core::store::Thread;
use trek_remote as tr;

impl Workspace {
    /// A pull request `t` opened failing, while that isn't dealt with.
    pub(super) fn remote_checks(&self, t: &Thread) -> Option<tr::FailingChecks> {
        let offer = self.pr_checks_offer(&t.id)?;
        Some(tr::FailingChecks {
            url: offer.url,
            number: offer.number,
            failing: offer.failing,
            bar: self.pr_checks_bar_up(&t.id),
            stopped: offer.stopped,
            sending: offer.sending,
            auto: self.pr_checks_auto_on(&t.id),
        })
    }

    /// What `t`'s row says it waits on when a pull request it opened is failing: the Mac's row
    /// words, once it's idle (at work, it may be on it already).
    pub(super) fn remote_checks_needs(&self, t: &Thread) -> Option<tr::Needs> {
        (t.run_state == RunState::Idle)
            .then(|| self.pr_checks_offer(&t.id))
            .flatten()
            .map(|offer| tr::Needs {
                kind: tr::NeedsKind::Failed,
                text: offer.row_words(),
            })
    }

    /// The account the limit bar offers `id` to carry on with.
    pub(super) fn remote_other_account(&self, id: &str) -> Option<tr::OtherAccount> {
        self.next_account(id).map(|c| tr::OtherAccount {
            account: c.id,
            label: c.label,
        })
    }

    /// The Compact chip's offer for `t`, lapsed or not (the phone shows it from `lapses_at`).
    pub(super) fn remote_compact_offer(&self, t: &Thread) -> Option<tr::CompactOffer> {
        let offer = self.compact_offer(&t.id, &Scope::Thread(t.id.clone()))?;
        Some(tr::CompactOffer {
            used: offer.used,
            window: offer.window,
            last_active: offer.last_active,
            lapses_at: offer.lapses_at(),
            why: offer.why(),
        })
    }

    /// The move waiting on `t`'s next message, as its bar says it.
    pub(super) fn remote_pending_switch(&self, t: &Thread) -> Option<tr::PendingSwitch> {
        let pending = self.pending_switch(&t.id)?;
        let (to, from) = self.pending_switch_names(&t.id)?;
        let models = self.models_for(&pending.agent);
        Some(tr::PendingSwitch {
            agent: agent_ref(&pending.agent),
            model: pending.model.clone(),
            model_label: pending
                .model
                .as_deref()
                .map(|m| crate::composer::model_name(&models, m)),
            effort: Some(pending.effort.as_str().to_string()),
            to,
            from,
        })
    }

    /// `switch_preview`: the handover the next message would carry now.
    pub(super) fn remote_switch_preview(&self, id: &str) -> tr::HostResult<tr::SwitchPreview> {
        if self.thread(id).is_none() {
            return Err(tr::HostError::not_found("No such thread"));
        }
        if self.pending_switch(id).is_none() {
            return Err(tr::HostError::not_found(
                "No switch waits for its next message",
            ));
        }
        let handover = self
            .pending_handover(id)
            .ok_or_else(|| tr::HostError::not_found("Its transcript isn't loaded"))?;
        Ok(tr::SwitchPreview {
            thread_id: id.to_string(),
            left_out: crate::workspace::switching::left_out_words(&handover),
            text: super::clip_text(&handover.text),
            tokens: handover.tokens,
            budget: handover.budget,
        })
    }

    /// The bars' buttons, from a phone.
    pub(super) fn remote_bar_action(
        &mut self,
        id: &str,
        action: tr::ThreadAction,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        match action {
            tr::ThreadAction::SendFailedChecks => {
                let offer = self
                    .pr_checks_offer(id)
                    .ok_or_else(|| tr::HostError::conflict("Nothing is failing now"))?;
                if offer.sending {
                    return Err(tr::HostError::conflict(
                        "Its log is being read to send already",
                    ));
                }
                self.pr_checks_send(id, cx);
            }
            tr::ThreadAction::DismissFailedChecks => {
                self.pr_checks_offer(id)
                    .ok_or_else(|| tr::HostError::conflict("Nothing is failing now"))?;
                self.pr_checks_dismiss(id, cx);
            }
            tr::ThreadAction::ContinueOnAccount { account } => {
                self.pause(id)
                    .ok_or_else(|| tr::HostError::conflict("It isn't paused at a usage limit"))?;
                let next = self.next_account(id).ok_or_else(|| {
                    tr::HostError::conflict("No other account of its agent has room now")
                })?;
                if next.id != account {
                    return Err(tr::HostError::conflict(format!(
                        "That account has no room now. {} has.",
                        next.label
                    )));
                }
                self.continue_on_account(id, account, cx);
            }
            tr::ThreadAction::StayOnAgent => {
                self.pending_switch(id).ok_or_else(|| {
                    tr::HostError::conflict("No switch waits for its next message")
                })?;
                self.stay_on_agent(id, cx);
            }
            _ => {
                return Err(tr::HostError::bad_request(
                    "This Mac can't do that to a thread",
                ));
            }
        }
        // The row says what changed with the next push, not in a few seconds.
        self.threads_gen += 1;
        Ok(())
    }
}
