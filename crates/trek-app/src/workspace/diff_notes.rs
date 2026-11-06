//! Review notes: the comments waiting on a thread's diff and the files marked viewed (see
//! `diff_review`). Kept in the store as they change, so they outlast the panel and the app.

use super::{Scope, Workspace, WorkspaceEvent};
use crate::diff_review::{Draft, LineAnchor, Notes};
use gpui_kit::Context;

impl Workspace {
    /// A diff's comments go to `scope`'s chat as one message (a new chat's first, if it's on a
    /// new one). Not while its agent waits on a question, which would take them as the answer,
    /// nor on a secret's card, while no message goes: they stay, and a toast says what to do.
    /// A move to another agent waiting for the next message happens now, as the composer's
    /// next message would make it. Whether they went.
    pub fn send_comments_in(
        &mut self,
        scope: &Scope,
        text: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(id) = self.thread_id_in(scope).map(str::to_string) {
            let why = if self.asks_for_secret(&id) {
                Some(
                    "The agent is waiting for a secret. Give it in its card or decline it, then send the comments.",
                )
            } else if self.waits_on_question(&id) {
                Some(
                    "The agent is waiting on its question. Answer it first, then send the comments.",
                )
            } else {
                None
            };
            if let Some(why) = why {
                cx.emit(WorkspaceEvent::Toast {
                    message: why.into(),
                    undo: None,
                });
                return false;
            }
            self.commit_switch_in(scope, &text, cx);
        }
        self.send_in(scope, text, vec![], cx);
        true
    }

    /// The store keeps `key`'s notes as they are now, or forgets them when there's nothing left.
    fn review_notes_changed(&mut self, key: &str, cx: &mut Context<Self>) {
        self.review_notes_epoch += 1;
        let data = self
            .review_notes
            .get(key)
            .filter(|n| !n.drafts.is_empty() || !n.viewed.is_empty())
            .and_then(|n| serde_json::to_string(n).ok());
        if data.is_none() {
            self.review_notes.remove(key);
        }
        if let Err(e) = self.store.set_review_notes(key, data.as_deref()) {
            tracing::warn!("save the review notes of {key}: {e:#}");
        }
        cx.notify();
    }

    /// Notes left when the app last quit come back, except those of threads that are gone.
    pub(super) fn restore_review_notes(&mut self) {
        for (key, data) in self.store.review_notes().unwrap_or_default() {
            let Ok(notes) = serde_json::from_str::<Notes>(&data) else {
                continue;
            };
            if !key.starts_with("dir:") && self.thread(&key).is_none() {
                let _ = self.store.set_review_notes(&key, None);
                continue;
            }
            self.review_notes.insert(key, notes);
        }
    }

    /// A new comment; its id.
    pub fn add_comment(
        &mut self,
        key: &str,
        anchor: LineAnchor,
        body: String,
        cx: &mut Context<Self>,
    ) -> u64 {
        let notes = self.review_notes.entry(key.to_string()).or_default();
        notes.next_id += 1;
        let id = notes.next_id;
        notes.drafts.push(Draft {
            id,
            path: anchor.path,
            side: anchor.side,
            first: anchor.first,
            last: anchor.last,
            quote: anchor.quote,
            body,
        });
        self.review_notes_changed(key, cx);
        id
    }

    /// Comment `id` says `body` now; with nothing left in it, it goes.
    pub fn edit_comment(&mut self, key: &str, id: u64, body: String, cx: &mut Context<Self>) {
        if body.trim().is_empty() {
            return self.remove_comment(key, id, cx);
        }
        let Some(d) = self
            .review_notes
            .get_mut(key)
            .and_then(|n| n.drafts.iter_mut().find(|d| d.id == id))
        else {
            return;
        };
        d.body = body;
        self.review_notes_changed(key, cx);
    }

    pub fn remove_comment(&mut self, key: &str, id: u64, cx: &mut Context<Self>) {
        let Some(notes) = self.review_notes.get_mut(key) else {
            return;
        };
        notes.drafts.retain(|d| d.id != id);
        self.review_notes_changed(key, cx);
    }

    /// Take every comment off `key` (sent, or cleared); they come back to whoever keeps them.
    pub fn drop_comments(&mut self, key: &str, cx: &mut Context<Self>) -> Vec<Draft> {
        let Some(notes) = self.review_notes.get_mut(key) else {
            return vec![];
        };
        let drafts = std::mem::take(&mut notes.drafts);
        self.review_notes_changed(key, cx);
        drafts
    }

    /// Clear's undo: `drafts` go back, after any written since.
    pub(super) fn restore_comments(
        &mut self,
        key: &str,
        drafts: Vec<Draft>,
        cx: &mut Context<Self>,
    ) {
        let notes = self.review_notes.entry(key.to_string()).or_default();
        for mut d in drafts {
            notes.next_id += 1;
            d.id = notes.next_id;
            notes.drafts.push(d);
        }
        self.review_notes_changed(key, cx);
    }

    /// The diff moved under some comments: (comment, its first and last line now).
    pub fn move_comments(&mut self, key: &str, moved: &[(u64, u32, u32)], cx: &mut Context<Self>) {
        let Some(notes) = self.review_notes.get_mut(key) else {
            return;
        };
        let mut any = false;
        for (id, first, last) in moved {
            if let Some(d) = notes.drafts.iter_mut().find(|d| d.id == *id) {
                (d.first, d.last) = (*first, *last);
                any = true;
            }
        }
        if any {
            self.review_notes_changed(key, cx);
        }
    }

    pub fn is_viewed(&self, key: &str, name: &str) -> bool {
        self.review_notes
            .get(key)
            .is_some_and(|n| n.viewed.contains_key(name))
    }

    /// What `name`'s diff was when it was marked viewed.
    pub fn viewed_sig(&self, key: &str, name: &str) -> Option<&str> {
        self.review_notes
            .get(key)?
            .viewed
            .get(name)
            .map(String::as_str)
    }

    /// Mark `name` viewed, at the diff `sig` says, or not.
    pub fn set_viewed(
        &mut self,
        key: &str,
        name: &str,
        sig: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let notes = self.review_notes.entry(key.to_string()).or_default();
        match sig {
            Some(sig) => {
                notes.viewed.insert(name.to_string(), sig);
            }
            None => {
                notes.viewed.remove(name);
            }
        }
        self.review_notes_changed(key, cx);
    }
}
