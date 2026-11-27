//! Taking back, branching, snoozing and deleting a thread that runs on another machine, from
//! here: rewind, undo, retry, edit and send again, fork (`turn_action`), what the confirmation
//! says first (`turn_preview`), and the thread menu's Snooze and Delete… (`thread_action`).
//!
//! The machine does the work and keeps the thread: this computer asks, and shows what the machine
//! answers. Until 0.4.67 every one of these said "Do that in Trek on <machine>".

use super::split_id;
use crate::workspace::{ForkAt, Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, Context, Entity, Window};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use trek_core::checkpoint::{Change, FileChange};
use trek_core::store::Item;
use trek_remote as tr;
use trek_remote::client::ClientError;

/// What a machine says an undo, retry or rewind would do (`turn_preview`).
pub struct TurnPreview {
    /// Its agent can't take its own session back to there: it carries on from a recap.
    pub recap: bool,
    /// The files restoring would change, or why none can be put back.
    pub files: Result<Vec<FileChange>, String>,
}

/// A machine's refusal as a toast says it. A machine on a Trek that doesn't know the request
/// ("Unknown message type", or the "This Mac can't …" its host answers by default) is said to
/// need a newer Trek, with the version when it's known (`since`), never "do it there".
pub fn refused(e: &ClientError, name: &str, what: &str, since: Option<&str>) -> String {
    match e {
        ClientError::Server { code, message }
            if *code == tr::ErrorCode::BadRequest
                && (message.starts_with("Unknown message type")
                    || message.starts_with("This Mac can't")) =>
        {
            needs_newer(name, what, since)
        }
        ClientError::Server { message, .. } => format!("Couldn't {what} on {name}: {message}"),
        other => format!("Couldn't {what} on {name}: {other}"),
    }
}

/// "T-Book needs Trek 0.4.68 or later to rewind from here."
pub fn needs_newer(name: &str, what: &str, since: Option<&str>) -> String {
    match since {
        Some(since) => format!("{name} needs Trek {since} or later to {what} from here."),
        None => format!("{name} needs a newer Trek to {what} from here."),
    }
}

fn change_of(change: tr::PreviewChange) -> Change {
    match change {
        tr::PreviewChange::Revert => Change::Modified,
        tr::PreviewChange::Delete => Change::Added,
        tr::PreviewChange::BringBack => Change::Deleted,
        tr::PreviewChange::Nested => Change::Nested,
        tr::PreviewChange::Kept => Change::Kept,
    }
}

/// What a `turn_action` answered: the new thread (a fork), and the message taken back.
type Done = (Option<String>, Option<String>);

impl Workspace {
    /// The machine's name and connection for its thread `id`, when it can be asked now. Said in
    /// a toast when it can't (offline, not connected).
    fn machine_for(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<(String, String, std::sync::Arc<tr::client::Client>)> {
        let (host, tid) = split_id(id)?;
        if self.machine_unavailable(host, cx) {
            return None;
        }
        let m = self.machines.get(host)?;
        Some((m.entry.name.clone(), tid.to_string(), m.client.clone()?))
    }

    /// What the confirmation before `action` on item `item` of a machine's thread says, asked of
    /// the machine (`None`: `id` isn't a machine's thread, or the machine isn't connected). A
    /// machine that can't be asked, or doesn't serve `turn_preview`, restores no files: why is
    /// in `files`.
    pub fn machine_turn_preview(
        &self,
        id: &str,
        item: &str,
        action: tr::TurnAction,
    ) -> Option<Pin<Box<dyn Future<Output = TurnPreview>>>> {
        let (host, tid) = split_id(id)?;
        let m = self.machines.get(host)?;
        let name = m.entry.name.clone();
        let client = m.client.clone()?;
        let serves = self.machines.serves(id, tr::feature::TURN_PREVIEW);
        let req = tr::TurnPreviewRequest {
            thread_id: tid.to_string(),
            item_id: item.to_string(),
            action,
        };
        Some(Box::pin(async move {
            if !serves {
                return TurnPreview {
                    recap: false,
                    files: Err(format!(
                        "{name} needs a newer Trek to say which files it would put back, so they stay as they are."
                    )),
                };
            }
            match client.request(tr::ClientMessage::TurnPreview(req)).await {
                Ok(tr::ServerMessage::TurnPreview(p)) => TurnPreview {
                    recap: p.recap,
                    files: match (p.files, p.unavailable) {
                        (Some(files), _) => Ok(files
                            .into_iter()
                            .map(|f| FileChange {
                                path: f.path,
                                change: change_of(f.change),
                            })
                            .collect()),
                        (None, Some(why)) => Err(why),
                        (None, None) => Err(format!("{name} has no checkpoint of the files.")),
                    },
                },
                Ok(other) => TurnPreview {
                    recap: false,
                    files: Err(format!(
                        "Couldn't check the files on {name}: unexpected reply ({})",
                        super::tr_kind(&other)
                    )),
                },
                // What the machine says stands in the way, in its own words ("Stop the running
                // turn to rewind").
                Err(ClientError::Server {
                    code: tr::ErrorCode::Conflict,
                    message,
                }) => TurnPreview {
                    recap: false,
                    files: Err(message),
                },
                Err(e) => TurnPreview {
                    recap: false,
                    files: Err(refused(&e, &name, "check the files", None)),
                },
            }
        }))
    }

    /// Send a `turn_action` for a machine's thread and hand what it answered to `then` (the
    /// refusal, worded for a toast, when it refused). False when nothing was sent: the machine
    /// can't be asked now (said in a toast).
    #[allow(clippy::too_many_arguments)]
    fn machine_turn_action(
        &mut self,
        id: &str,
        item: &str,
        action: tr::TurnAction,
        model: Option<String>,
        restore: bool,
        what: &'static str,
        then: impl FnOnce(&mut Self, Result<Done, String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((name, thread_id, client)) = self.machine_for(id, cx) else {
            return false;
        };
        let req = tr::TurnActionRequest {
            thread_id,
            item_id: item.to_string(),
            action,
            model,
            restore_files: restore,
        };
        cx.spawn(async move |this, cx| {
            let done = match client.request(tr::ClientMessage::TurnAction(req)).await {
                Ok(tr::ServerMessage::Ack {
                    thread_id, text, ..
                }) => Ok((thread_id, text)),
                Ok(_) => Ok((None, None)),
                Err(e) => Err(refused(&e, &name, what, None)),
            };
            let _ = this.update(cx, |ws, cx| then(ws, done, cx));
        })
        .detach();
        true
    }

    /// The text and pictures of message `item` of a machine's thread, as this computer shows
    /// them (its own copies of what it sent), for a composer.
    fn machine_said(&self, id: &str, item: &str) -> Option<(String, Vec<PathBuf>)> {
        let live = self.live.get(id)?;
        match &live.items[live.items.position(item)?] {
            Item::User {
                text,
                images,
                aside: false,
                ..
            } => Some((text.clone(), images.iter().map(PathBuf::from).collect())),
            _ => None,
        }
    }

    /// A turn is running on the machine's thread: said in a toast. Nothing can be taken back yet.
    fn machine_busy(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if !self.turn_running(id) {
            return false;
        }
        cx.emit(WorkspaceEvent::Toast {
            message: "Stop the running turn first.".into(),
            undo: None,
        });
        true
    }

    /// Rewind a machine's thread to just before message `item`, or (`undo`) take back the turn
    /// that item `item` ends. The message goes back into the composer of `scope` once the
    /// machine has done it.
    pub fn machine_take_back(
        &mut self,
        id: &str,
        item: &str,
        undo: bool,
        restore: bool,
        scope: Scope,
        cx: &mut Context<Self>,
    ) {
        if self.machine_busy(id, cx) {
            return;
        }
        let message = if undo {
            self.turn_start_item(id, item)
        } else {
            Some(item.to_string())
        };
        let back = message.and_then(|m| self.machine_said(id, &m));
        let (action, what) = if undo {
            (tr::TurnAction::Undo, "undo the turn")
        } else {
            (tr::TurnAction::Rewind, "rewind")
        };
        let thread = id.to_string();
        self.machine_turn_action(
            id,
            item,
            action,
            None,
            restore,
            what,
            move |_, done, cx| match done {
                Ok((_, text)) => {
                    // As this computer shows the message (its own files); else as the machine
                    // sent it back.
                    let (text, images) = back.unwrap_or((text.unwrap_or_default(), vec![]));
                    cx.emit(WorkspaceEvent::ComposeIn {
                        scope,
                        thread,
                        text,
                        images,
                        edit: None,
                    });
                }
                Err(message) => cx.emit(WorkspaceEvent::Failure { message }),
            },
            cx,
        );
    }

    /// Take back the turn item `end` closes on a machine's thread and send its message again
    /// there, with `model` if given.
    pub fn machine_retry(
        &mut self,
        id: &str,
        end: &str,
        model: Option<String>,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if self.machine_busy(id, cx) {
            return;
        }
        self.machine_turn_action(
            id,
            end,
            tr::TurnAction::Retry,
            model,
            restore,
            "retry",
            |_, done, cx| {
                if let Err(message) = done {
                    cx.emit(WorkspaceEvent::Failure { message });
                }
            },
            cx,
        );
    }

    /// Send `text` in place of message `item` of a machine's thread: the machine rewinds to just
    /// before it, then takes the new message (with what's attached here, sent ahead as any
    /// message's is). False when nothing was asked. If the machine refuses the rewind, or
    /// rewinds and then doesn't take the message (the link dropped between the two, it refused
    /// it: `machine_send` gives it back), the edited words and pictures go back into the
    /// composer.
    pub fn machine_edit_and_resend(
        &mut self,
        id: &str,
        item: &str,
        text: String,
        images: Vec<PathBuf>,
        restore: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.machine_busy(id, cx) {
            return false;
        }
        let thread = id.to_string();
        self.machine_turn_action(
            id,
            item,
            tr::TurnAction::Rewind,
            None,
            restore,
            "send the edited message",
            move |ws, done, cx| match done {
                Ok(_) => ws.machine_send(&thread, text, images, None, cx),
                Err(message) => {
                    cx.emit(WorkspaceEvent::Failure { message });
                    cx.emit(WorkspaceEvent::InsertIntoComposer(text));
                    for image in images {
                        cx.emit(WorkspaceEvent::AttachImage(image));
                    }
                }
            },
            cx,
        )
    }

    /// Fork a machine's thread at `at`: the new thread is the machine's too, and opens here as
    /// a fork of a thread of this computer does (a fork from before a message has that message
    /// waiting in its composer).
    pub fn machine_fork(&mut self, id: &str, at: ForkAt, scope: &Scope, cx: &mut Context<Self>) {
        let Some((host, _)) = split_id(id).map(|(h, t)| (h.to_string(), t.to_string())) else {
            return;
        };
        let (item, back) = match &at {
            ForkAt::Before(item) => (Some(item.clone()), self.machine_said(id, item)),
            ForkAt::After(end) => (Some(end.clone()), None),
            // The whole conversation: up to where its last turn ended.
            ForkAt::End => {
                if self.turn_running(id) {
                    cx.emit(WorkspaceEvent::Toast {
                        message: "Wait for the turn to finish, or fork from an earlier message."
                            .into(),
                        undo: None,
                    });
                    return;
                }
                let end = self.live.get(id).and_then(|l| {
                    let ix = l.items.iter().rposition(trek_core::rewind::ends_turn)?;
                    l.items.id_at(ix).map(str::to_string)
                });
                (end, None)
            }
        };
        let Some(item) = item else {
            cx.emit(WorkspaceEvent::Toast {
                message: "There's no finished turn to fork from yet. Open the thread and try again once it has answered.".into(),
                undo: None,
            });
            return;
        };
        let scope = scope.clone();
        self.machine_turn_action(
            id,
            &item,
            tr::TurnAction::Fork,
            None,
            false,
            "fork the thread",
            move |ws, done, cx| match done {
                Ok((Some(fork), text)) => {
                    let message = back.or(text.map(|t| (t, vec![])));
                    ws.open_machine_fork(&host, &fork, scope, message, cx);
                }
                Ok((None, _)) => cx.emit(WorkspaceEvent::Failure {
                    message: "The fork was made, but its machine didn't say which thread it is."
                        .into(),
                }),
                Err(message) => cx.emit(WorkspaceEvent::Failure { message }),
            },
            cx,
        );
    }

    /// Open thread `fork` of machine `host` once its row is here (the machine sends it as the
    /// fork is made, a moment either side of its answer), with `message` in its composer.
    fn open_machine_fork(
        &mut self,
        host: &str,
        fork: &str,
        scope: Scope,
        message: Option<(String, Vec<PathBuf>)>,
        cx: &mut Context<Self>,
    ) {
        let local = super::local_id(host, fork);
        cx.spawn(async move |this, cx| {
            for _ in 0..60 {
                let here = this
                    .read_with(cx, |ws, _| ws.thread(&local).is_some())
                    .unwrap_or(true);
                if here {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
            let _ = this.update(cx, |ws, cx| {
                if ws.thread(&local).is_none() {
                    cx.emit(WorkspaceEvent::Failure {
                        message: "The fork was made, but hasn't shown up here yet. It's in the machine's list."
                            .into(),
                    });
                    return;
                }
                let into = match scope {
                    Scope::Ide | Scope::Side => scope.clone(),
                    _ => Scope::Main,
                };
                match &scope {
                    Scope::Main | Scope::Beside => ws.navigate(Route::Thread(local.clone()), cx),
                    Scope::Thread(_) => ws.show_in_main(Route::Thread(local.clone()), cx),
                    Scope::Ide | Scope::Side => ws.open_chat_in(&scope, &local, cx),
                }
                if let Some((text, images)) = message {
                    cx.emit(WorkspaceEvent::ComposeIn {
                        scope: into,
                        thread: local.clone(),
                        text,
                        images,
                        edit: None,
                    });
                }
            });
        })
        .detach();
    }

    /// Snooze a machine's thread until `at` (ms), or (`None`) wake it now.
    pub fn machine_snooze(&mut self, id: &str, at: Option<i64>, cx: &mut Context<Self>) {
        let Some((host, _)) = split_id(id) else {
            return;
        };
        if !self.machines.serves(id, tr::feature::SNOOZE) {
            let name = self
                .machines
                .get(host)
                .map_or_else(String::new, |m| m.entry.name.clone());
            cx.emit(WorkspaceEvent::Toast {
                message: needs_newer(&name, "snooze its threads", None),
                undo: None,
            });
            return;
        }
        let action = match at {
            Some(at) => tr::ThreadAction::Snooze {
                until: Some(at),
                preset: None,
            },
            None => tr::ThreadAction::Wake,
        };
        self.machine_thread_action(id, action, cx);
    }
}

/// The thread menu's Delete… for a thread on another machine. The machine words the
/// confirmation (it knows what goes with the thread: a worktree's uncommitted work, sub-agents):
/// asked without `confirmed` it deletes nothing and answers with those words, which the dialog
/// here shows; Delete then asks again, confirmed.
pub fn confirm_delete(
    ws: Entity<Workspace>,
    id: String,
    title: String,
    window: &mut Window,
    cx: &mut App,
) {
    let asked = ws.update(cx, |ws, cx| {
        let (name, thread_id, client) = ws.machine_for(&id, cx)?;
        if !ws.machines.serves(&id, tr::feature::DELETE) {
            cx.emit(WorkspaceEvent::Toast {
                message: needs_newer(&name, "delete its threads", None),
                undo: None,
            });
            return None;
        }
        Some((name, thread_id, client))
    });
    let Some((name, thread_id, client)) = asked else {
        return;
    };
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let req = tr::ClientMessage::ThreadAction(tr::ThreadActionRequest {
            thread_id,
            action: tr::ThreadAction::Delete { confirmed: false },
        });
        let words = match client.request(req).await {
            Err(ClientError::Server {
                code: tr::ErrorCode::Conflict,
                message,
            }) => message,
            // Every Trek that runs threads for another asks first; one that didn't has deleted it.
            Ok(_) => return,
            Err(e) => {
                let message = refused(&e, &name, "delete the thread", None);
                let _ = ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::Failure { message }));
                return;
            }
        };
        let _ = handle.update(cx, |_, window, cx| {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (ws, id) = (ws.clone(), id.clone());
                alert
                    .title(format!("Delete “{title}” on {name}?"))
                    .description(words.clone())
                    .confirm()
                    .ok_text("Delete")
                    .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        ws.update(cx, |ws, cx| {
                            ws.machine_thread_action(
                                &id,
                                tr::ThreadAction::Delete { confirmed: true },
                                cx,
                            )
                        });
                        true
                    })
            });
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_too_old_for_a_request_is_said_to_need_a_newer_trek_not_to_be_gone_to() {
        let unknown = ClientError::Server {
            code: tr::ErrorCode::BadRequest,
            message: "Unknown message type \"turn_preview\"".into(),
        };
        assert_eq!(
            refused(&unknown, "T-Book", "rewind", Some("0.4.68")),
            "T-Book needs Trek 0.4.68 or later to rewind from here."
        );
        assert_eq!(
            refused(&unknown, "T-Book", "rewind", None),
            "T-Book needs a newer Trek to rewind from here."
        );
        let default = ClientError::Server {
            code: tr::ErrorCode::BadRequest,
            message: "This Mac can't do that to a thread".into(),
        };
        assert!(refused(&default, "T-Book", "fork the thread", None).contains("newer Trek"));
        // What the machine did say is passed on in its own words, without the code.
        let busy = ClientError::Server {
            code: tr::ErrorCode::BadRequest,
            message: "Stop the running turn to rewind".into(),
        };
        assert_eq!(
            refused(&busy, "T-Book", "rewind", None),
            "Couldn't rewind on T-Book: Stop the running turn to rewind"
        );
        assert!(
            refused(&ClientError::Disconnected, "T-Book", "rewind", None)
                .starts_with("Couldn't rewind on T-Book: the connection dropped")
        );
    }

    #[test]
    fn a_previews_changes_read_as_the_local_confirmations_do() {
        assert_eq!(change_of(tr::PreviewChange::Revert), Change::Modified);
        assert_eq!(change_of(tr::PreviewChange::Delete), Change::Added);
        assert_eq!(change_of(tr::PreviewChange::BringBack), Change::Deleted);
        assert_eq!(change_of(tr::PreviewChange::Nested), Change::Nested);
        assert_eq!(change_of(tr::PreviewChange::Kept), Change::Kept);
    }
}
