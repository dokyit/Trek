//! What the sidebar's thread menu does, from a phone: snooze (the Mac's presets, or until a time)
//! and wake, mark unread, regenerate the title, and delete, which the Mac confirms first: asked
//! without `confirmed`, it answers `conflict` with what its own confirmation would say (a
//! worktree's uncommitted work, sub-agents that go too), and deletes only when asked again.

use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::ThreadSource;
use trek_core::store::now_ms;
use trek_remote as tr;

impl Workspace {
    /// The thread actions besides pinning, settling, archiving and renaming (deleting aside).
    pub(super) fn remote_more_thread_action(
        &mut self,
        id: &str,
        action: tr::ThreadAction,
        cx: &mut Context<Self>,
    ) -> tr::HostResult<()> {
        match action {
            tr::ThreadAction::Snooze {
                preset: Some(preset),
                ..
            } => match preset {
                tr::SnoozePreset::Hour => self.snooze(id, 1, cx),
                tr::SnoozePreset::ThreeHours => self.snooze(id, 3, cx),
                tr::SnoozePreset::Tomorrow => self.snooze_until_morning(id, 1, cx),
                tr::SnoozePreset::NextWeek => self.snooze_until_morning(id, 7, cx),
            },
            tr::ThreadAction::Snooze {
                until: Some(at), ..
            } if at <= now_ms() => return Err(tr::HostError::bad_request("That time has passed")),
            tr::ThreadAction::Snooze {
                until: Some(at), ..
            } => self.snooze_until(id, at, cx),
            tr::ThreadAction::Snooze { .. } => {
                return Err(tr::HostError::bad_request("Snooze until when?"));
            }
            tr::ThreadAction::Wake => {
                if self
                    .thread(id)
                    .is_some_and(|t| t.snoozed_until.is_some_and(|at| at > now_ms()))
                {
                    self.unsnooze(id, cx);
                }
            }
            tr::ThreadAction::MarkUnread => self.mark_unread(id, cx),
            tr::ThreadAction::RegenerateTitle => {
                if self.title_inputs(id).is_none() {
                    return Err(tr::HostError::bad_request(
                        "There's nothing in it to name it after yet",
                    ));
                }
                self.regenerate_title(id, true, cx);
            }
            _ => {
                return Err(tr::HostError::bad_request(
                    "This Mac can't do that to a thread",
                ));
            }
        }
        Ok(())
    }

    /// Delete a thread as the sidebar's Delete… does once confirmed: a thread in a worktree of its
    /// own takes the worktree with it (and stays if the worktree can't go). Unconfirmed, nothing
    /// happens: the answer is the confirmation's words.
    pub(super) fn remote_delete(
        &mut self,
        id: String,
        confirmed: bool,
        reply: tr::Reply<()>,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.thread(&id).cloned() else {
            return drop(reply.send(Err(tr::HostError::not_found("No such thread"))));
        };
        let sub_agents = self.sub_agents_note(&id, "deleted");
        let sharers = self.worktree_sharers(&id);
        let Some(wt) = t.worktree.clone().filter(|_| sharers.is_empty()) else {
            if !confirmed {
                let mut words = vec![if t.source != ThreadSource::Trek {
                    "It leaves Trek for good. The original stays in the agent's own history."
                        .to_string()
                } else {
                    "The thread and its transcript are deleted from Trek. This can't be undone."
                        .to_string()
                }];
                if let Some(first) = sharers.first().and_then(|s| self.thread(s)) {
                    let others = match sharers.len() {
                        1 => format!("“{}”", first.title),
                        n => format!("“{}” and {} more", first.title, n - 1),
                    };
                    words.push(format!("Its worktree stays: {others} works in it too."));
                }
                words.extend(sub_agents);
                return drop(reply.send(Err(tr::HostError::conflict(words.join(" ")))));
            }
            self.delete_thread(&id, cx);
            return drop(reply.send(Ok(())));
        };
        let check = self.worktree_removal(&id, cx);
        cx.spawn(async move |this, cx| {
            let Some(r) = check.await else { return drop(reply.send(Err(tr::HostError::other("Couldn't look at its worktree")))) };
            if !confirmed {
                let mut words = vec!["The thread and its transcript are deleted from Trek, and its worktree with them. This can't be undone.".to_string()];
                words.extend(sub_agents);
                words.extend(crate::worktree_ui::losses(&wt, &r, None).into_iter().map(|(line, _)| line));
                return drop(reply.send(Err(tr::HostError::conflict(words.join(" ")))));
            }
            // No more uncommitted changes go than were counted just now.
            let removed = this.update(cx, |ws, cx| ws.remove_worktree(&id, r.uncommitted, false, cx));
            let result = match removed {
                Ok(task) => match task.await {
                    Ok(_) => this.update(cx, |ws, cx| ws.delete_thread(&id, cx)).map_err(|_| tr::HostError::other("Trek is closing")),
                    Err(e) => Err(tr::HostError::conflict(format!("Couldn't remove its worktree, so the thread stays: {e:#}"))),
                },
                Err(_) => Err(tr::HostError::other("Trek is closing")),
            };
            let _ = reply.send(result);
        })
        .detach();
    }
}
