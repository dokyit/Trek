//! A thread as the Mac's thread view shows it, for the phone: a turn's "Verified" badge, the
//! limit bar's buttons (feature `limit_actions`), the context ring's card and Compact now
//! (`context_card`), the confirmation's file list before an undo, retry or rewind
//! (`turn_preview`), and sending with consultants (`consult`).

use crate::workspace::{Scope, Workspace};
use gpui_kit::Context;
use trek_core::AgentId;
use trek_core::checkpoint::Change;
use trek_core::orchestrate::{Consult, Consultant, Style};
use trek_core::store::Item;
use trek_core::verification::Probe;
use trek_remote as tr;

/// How long one of a [`tr::TurnVerified`]'s commands may be, in characters.
const VERIFIED_COMMAND: usize = 200;

/// The turn ending at item `end` of `items` ran the project's verification CLI (`probe`): what
/// the Mac's badge under its turn end says. The turn is what follows the previous turn's end.
pub(super) fn verified(
    items: &[Item],
    end: usize,
    probe: Option<&Probe>,
) -> Option<tr::TurnVerified> {
    let probe = probe?;
    let end = end.min(items.len());
    let from = items[..end]
        .iter()
        .rposition(trek_core::rewind::ends_turn)
        .map_or(0, |b| b + 1);
    let v = trek_core::verification::verdict(&items[from.min(end)..end], probe)?;
    let skip = v.commands.len().saturating_sub(tr::MAX_VERIFIED);
    Some(tr::TurnVerified {
        passed: v.passed,
        commands: v.commands[skip..].iter().map(|c| clip(c)).collect(),
    })
}

fn clip(command: &str) -> String {
    if command.chars().count() <= VERIFIED_COMMAND {
        return command.to_string();
    }
    let cut: String = command.chars().take(VERIFIED_COMMAND - 1).collect();
    format!("{}…", cut.trim_end())
}

/// What restoring does to a file, as the confirmation says it.
fn preview_change(change: &Change) -> tr::PreviewChange {
    match change {
        Change::Modified => tr::PreviewChange::Revert,
        Change::Added => tr::PreviewChange::Delete,
        Change::Deleted => tr::PreviewChange::BringBack,
        Change::Nested => tr::PreviewChange::Nested,
        Change::Kept => tr::PreviewChange::Kept,
    }
}

/// What Compact now can do for thread `id`: wait for a turn or a pause, run (a `/compact` is the
/// turn running), or nothing (no message yet).
fn compact_state(ws: &Workspace, id: &str, items: &[Item]) -> tr::CompactState {
    let running = ws.turn_running(id);
    let last_message = items.iter().rev().find_map(|i| match i {
        Item::User { text, .. } => Some(text.trim()),
        _ => None,
    });
    if running && last_message == Some("/compact") {
        tr::CompactState::Running
    } else if running || ws.pause(id).is_some() {
        tr::CompactState::Busy
    } else if last_message.is_none() {
        tr::CompactState::Nothing
    } else {
        tr::CompactState::Ready
    }
}

impl Workspace {
    /// The limit bar's buttons and the context card's Compact now, from a phone.
    pub(super) fn remote_thread_parity_action(
        &mut self,
        id: &str,
        action: tr::ThreadAction,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        let not_paused = || tr::HostError::conflict("It isn't paused at a usage limit");
        match action {
            tr::ThreadAction::ResumeAtReset => {
                self.pause(id).ok_or_else(not_paused)?;
                self.resume_at_reset(id, cx);
            }
            tr::ThreadAction::CancelResume => {
                self.pause(id).ok_or_else(not_paused)?;
                self.cancel_resume(id, cx);
            }
            tr::ThreadAction::TryAgain => {
                self.pause(id).ok_or_else(not_paused)?;
                self.end_pause(id, true, cx);
            }
            tr::ThreadAction::SnoozeUntilReset => {
                let pause = self.pause(id).ok_or_else(not_paused)?;
                if pause.resets_at.is_none() {
                    return Err(tr::HostError::conflict(
                        "When the limit resets isn't known yet",
                    ));
                }
                self.snooze_until_reset(id, cx);
            }
            tr::ThreadAction::Compact => {
                // As the card's Compact now: not while a turn runs or the thread waits on a limit.
                if self.turn_running(id) {
                    return Err(tr::HostError::conflict("Stop the running turn to compact."));
                }
                if self.pause(id).is_some() {
                    return Err(tr::HostError::conflict(
                        "The thread is paused at a usage limit.",
                    ));
                }
                let said = self
                    .live
                    .get(id)
                    .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { .. })));
                if !said {
                    return Err(tr::HostError::conflict(
                        "Nothing to compact yet: send a message first.",
                    ));
                }
                self.send_to(id, "/compact".to_string(), vec![], cx);
            }
            _ => {
                return Err(tr::HostError::bad_request(
                    "This Mac can't do that to a thread",
                ));
            }
        }
        Ok(())
    }

    /// `context_card`: the card the context ring opens, as the Mac would draw it now.
    pub(super) fn remote_context_card(&self, id: &str) -> tr::HostResult<tr::ContextCard> {
        let t = self
            .thread(id)
            .ok_or_else(|| tr::HostError::not_found("No such thread"))?;
        let live = self.live.get(id);
        let Some((used, window)) = live.and_then(|l| l.context).filter(|(_, w)| *w > 0) else {
            return Err(tr::HostError::not_found(
                "The thread hasn't said how full its context is yet",
            ));
        };
        let scope = Scope::Thread(id.to_string());
        let card = crate::context_popover::Card::of(
            self,
            &scope,
            Some(t),
            used,
            window,
            crate::context_popover::Compact::Idle,
        );
        let shown = card.shown();
        let items = live.map(|l| &l.items[..]).unwrap_or_default();
        Ok(tr::ContextCard {
            thread_id: id.to_string(),
            used: shown.used,
            window: shown.window,
            reserved: shown.reserved,
            parts: shown
                .parts
                .into_iter()
                .map(|p| tr::ContextPart {
                    kind: p.kind.to_string(),
                    label: p.label.to_string(),
                    tokens: p.tokens,
                    detail: p
                        .detail
                        .into_iter()
                        .map(|(label, value)| tr::ContextDetail { label, value })
                        .collect(),
                    note: p.note,
                })
                .collect(),
            compact: compact_state(self, id, items),
        })
    }

    /// `turn_preview`: what the confirmation before `req.action` on `req.item_id` says. Which
    /// files restoring would change is worked out off the main thread, as the Mac's card does.
    pub(super) fn remote_turn_preview(
        &mut self,
        req: tr::TurnPreviewRequest,
        reply: tr::Reply<tr::TurnPreview>,
        cx: &mut Context<Self>,
    ) {
        let (found, plan) = tokio::sync::oneshot::channel();
        let ask = req.clone();
        self.when_loaded(req.thread_id.clone(), found, cx, move |ws, _| {
            ws.turn_preview_plan(&ask)
        });
        cx.spawn(async move |_, cx| {
            let made = match plan.await {
                Ok(Ok((recap, Ok(files)))) => {
                    let changes = cx
                        .background_executor()
                        .spawn(async move { files.changes() })
                        .await;
                    let (files, unavailable) = match changes {
                        Ok(changes) => (
                            Some(
                                changes
                                    .iter()
                                    .map(|f| tr::PreviewFile {
                                        path: f.path.clone(),
                                        change: preview_change(&f.change),
                                    })
                                    .collect(),
                            ),
                            None,
                        ),
                        Err(e) => (None, Some(format!("Couldn't check the files: {e:#}"))),
                    };
                    Ok(tr::TurnPreview {
                        thread_id: req.thread_id,
                        item_id: req.item_id,
                        recap,
                        files,
                        unavailable,
                    })
                }
                Ok(Ok((recap, Err(why)))) => Ok(tr::TurnPreview {
                    thread_id: req.thread_id,
                    item_id: req.item_id,
                    recap,
                    files: None,
                    unavailable: Some(why),
                }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(tr::HostError::other("Trek is closing")),
            };
            let _ = reply.send(made);
        })
        .detach();
    }

    /// For `turn_preview`: whether the agent carries on from a recap, and the files to look at
    /// (or why there are none), for the message the action goes back to.
    #[allow(clippy::type_complexity)]
    fn turn_preview_plan(
        &self,
        req: &tr::TurnPreviewRequest,
    ) -> tr::HostResult<(bool, Result<crate::workspace::RewindFiles, String>)> {
        let id = req.thread_id.as_str();
        let live = self
            .live
            .get(id)
            .ok_or_else(|| tr::HostError::not_found("No such thread"))?;
        let ix = req
            .item_id
            .strip_prefix('i')
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|ix| *ix < live.items.len())
            .ok_or_else(|| tr::HostError::not_found("No such item"))?;
        let item = &live.items[ix];
        let key = live.items.id_at(ix).unwrap_or_default().to_string();
        let what = match req.action {
            tr::TurnAction::Undo => "undo",
            tr::TurnAction::Retry => "retry",
            tr::TurnAction::Rewind => "rewind",
            tr::TurnAction::Fork => {
                return Err(tr::HostError::bad_request(
                    "A fork asks nothing first: it keeps this thread as it is",
                ));
            }
        };
        let message = if req.action == tr::TurnAction::Rewind {
            if !matches!(item, Item::User { aside: false, .. }) {
                return Err(tr::HostError::bad_request(
                    "Only your own messages can be rewound to",
                ));
            }
            key
        } else {
            if !trek_core::rewind::ends_turn(item) {
                return Err(tr::HostError::bad_request("That item doesn't end a turn"));
            }
            self.turn_start_item(id, &key).ok_or_else(|| {
                tr::HostError::bad_request("This turn didn't start from a message of yours")
            })?
        };
        if self.turn_running(id) {
            return Err(tr::HostError::conflict(format!(
                "Stop the running turn to {what}"
            )));
        }
        let recap = self.rewind_plan(id, &message) == Some(trek_core::rewind::Reopen::Recap);
        let files = self.rewind_files(id, &message).ok_or_else(|| {
            self.no_checkpoint(id, &message)
                .unwrap_or(crate::workspace::NoCheckpoint::Missing)
                .explain()
        });
        Ok((recap, files))
    }

    /// `text` for thread `t` wrapped with `consult`'s instructions, as the Mac's composer sends
    /// a message with consultants: every key must name a model of an agent set up here.
    pub(super) fn consulted(
        &self,
        thread: &str,
        agent: &AgentId,
        text: &str,
        consult: &tr::ConsultRequest,
    ) -> tr::HostResult<String> {
        if let Some(why) = self.consult_unavailable(agent) {
            return Err(tr::HostError::bad_request(format!("Can't consult: {why}.")));
        }
        let ready = self.ready_agents();
        let pick = |key: &str| -> tr::HostResult<Consultant> {
            let c = Consultant::from_key(key).ok_or_else(|| {
                tr::HostError::bad_request(format!("“{key}” isn't a model to consult"))
            })?;
            if !ready.contains(&c.agent) {
                return Err(tr::HostError::bad_request(format!(
                    "{} isn't set up on {}",
                    c.agent.display_name(),
                    crate::words::words().this_computer
                )));
            }
            let models = self.models_for(&c.agent);
            if !models.is_empty()
                && !models
                    .iter()
                    .any(|m| crate::composer::same_model(&c.model, &m.id))
            {
                return Err(tr::HostError::bad_request(format!(
                    "{} isn't one of {}'s models",
                    c.model,
                    c.agent.display_name()
                )));
            }
            Ok(c)
        };
        let consultants = consult
            .with
            .iter()
            .map(|k| pick(k))
            .collect::<tr::HostResult<Vec<_>>>()?;
        if consultants.is_empty() {
            return Err(tr::HostError::bad_request("Name a model to consult"));
        }
        let consult = Consult {
            consultants,
            style: match consult.style {
                tr::ConsultStyle::Advise => Style::Advise,
                tr::ConsultStyle::Discuss => Style::Discuss,
                tr::ConsultStyle::Arena => Style::Arena,
            },
            implement: consult.then == tr::ConsultThen::Implement,
            judge: consult.judge.as_deref().map(pick).transpose()?,
        };
        crate::ide::ai::commands::consult_prompt(
            self,
            &Scope::Thread(thread.to_string()),
            text,
            &consult,
        )
        .map_err(tr::HostError::bad_request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::store::ToolStatus;

    fn run(command: &str, status: ToolStatus) -> Item {
        Item::Tool {
            id: format!("t-{command}"),
            title: "Run".into(),
            detail: command.into(),
            output: String::new(),
            status,
        }
    }

    #[test]
    fn a_turn_that_ran_the_check_carries_its_verdict() {
        let probe = Probe {
            needle: "verify-trek".into(),
            main: None,
        };
        let items = vec![
            Item::User {
                text: "first".into(),
                images: vec![],
                aside: false,
                resume: None,
                at: None,
            },
            run("verify-trek check", ToolStatus::Done),
            Item::TurnEnd {
                at: 0,
                took_secs: 3,
            },
            Item::User {
                text: "second".into(),
                images: vec![],
                aside: false,
                resume: None,
                at: None,
            },
            run("verify-trek check", ToolStatus::Failed),
            run("cargo test", ToolStatus::Done),
            Item::TurnEnd {
                at: 0,
                took_secs: 3,
            },
        ];
        let first = verified(&items, 2, Some(&probe)).expect("ran it");
        assert!(first.passed);
        assert_eq!(first.commands, ["verify-trek check"]);
        let second = verified(&items, 6, Some(&probe)).expect("ran it");
        assert!(!second.passed, "only this turn's runs count");
        assert_eq!(second.commands, ["verify-trek check"]);
        assert_eq!(verified(&items, 6, None), None, "no verification skill");
        let long = "verify-trek check ".to_string() + &"x".repeat(400);
        let many: Vec<Item> = (0..8).map(|_| run(&long, ToolStatus::Done)).collect();
        let v = verified(&many, many.len(), Some(&probe)).unwrap();
        assert_eq!(v.commands.len(), tr::MAX_VERIFIED);
        assert!(
            v.commands
                .iter()
                .all(|c| c.chars().count() <= VERIFIED_COMMAND)
        );
    }
}
