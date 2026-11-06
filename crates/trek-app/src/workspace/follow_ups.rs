//! The follow-ups queued behind a running turn, one by one: the composer's rows move them, take
//! one back to change it, or remove it (`composer::queue`).

use super::{UndoAction, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::path::PathBuf;

impl Workspace {
    /// There's still something for `id`'s queue to wait behind: a turn, its agent's sub-agents, or
    /// whatever holds its messages. Otherwise the next message simply goes.
    pub fn queue_waits(&self, id: &str) -> bool {
        self.live
            .get(id)
            .is_some_and(|l| l.turn_started.is_some() || l.background_agents().next().is_some())
            || self.holds_queue(id)
    }

    fn queue_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(id) {
            live.revision += 1;
        }
        // The queue rides in the thread's row on the phone.
        self.threads_gen += 1;
        cx.notify();
    }

    /// Move queued message `ix` to place `to`.
    pub fn queue_move(&mut self, id: &str, ix: usize, to: usize, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id) else {
            return;
        };
        if ix == to || ix >= live.queued.len() || to >= live.queued.len() {
            return;
        }
        let message = live.queued.remove(ix);
        live.queued.insert(to, message);
        self.queue_changed(id, cx);
    }

    /// Take queued message `ix` off the queue.
    pub fn queue_take(
        &mut self,
        id: &str,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<(String, Vec<PathBuf>)> {
        let live = self.live.get_mut(id).filter(|l| ix < l.queued.len())?;
        let message = live.queued.remove(ix);
        self.queue_changed(id, cx);
        Some(message)
    }

    /// Put a message back at place `ix` (at the end, if the queue has since got shorter). Nothing
    /// left to wait behind, it goes back to the composer rather than sit there unsent: the one
    /// showing the thread, or the first to show it again (an undo from a toast, with the thread
    /// off screen, has no composer to take it).
    pub fn queue_put(
        &mut self,
        id: &str,
        ix: usize,
        text: String,
        images: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if !self.queue_waits(id) {
            if !self.on_screen(id) {
                let live = self.live.entry(id.to_string()).or_default();
                live.left_over.push((text, images));
                live.revision += 1;
                return;
            }
            cx.emit(WorkspaceEvent::RestoreQueued {
                thread: id.to_string(),
                text,
                images,
            });
            return;
        }
        let live = self.live.entry(id.to_string()).or_default();
        let at = ix.min(live.queued.len());
        live.queued.insert(at, (text, images));
        self.queue_changed(id, cx);
    }

    /// Remove queued message `ix`, with a toast to take that back.
    pub fn queue_remove(&mut self, id: &str, ix: usize, cx: &mut Context<Self>) {
        let Some((text, images)) = self.queue_take(id, ix, cx) else {
            return;
        };
        cx.emit(WorkspaceEvent::Toast {
            message: "Removed from the queue".into(),
            undo: Some(UndoAction::Requeue {
                thread: id.to_string(),
                ix,
                text,
                images,
            }),
        });
    }
}
