//! The follow-ups a thread holds for the phone: those sent while a turn runs (they go one a
//! finished turn), and those waiting for a usage limit to reset. They ride in the thread's row;
//! the phone drops one (it comes back for its composer) or changes its words in place.

use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::store::Thread;
use trek_remote as tr;

/// The most of a queued message a row carries.
const MAX_QUEUED_TEXT: usize = 16 << 10;

fn shown(text: &str) -> String {
    super::clip_text_to(text, MAX_QUEUED_TEXT)
}

impl Workspace {
    /// `t`'s queued follow-ups: after its running turn first, then those for its limit's reset.
    pub(super) fn remote_queued(&self, t: &Thread) -> Vec<tr::QueuedMessage> {
        let turn = self.live.get(&t.id).into_iter().flat_map(|l| &l.queued).map(|(text, images)| tr::QueuedMessage { text: shown(text), images: images.len() as u32, reset: false });
        let reset = t.paused.iter().flat_map(|p| &p.queued).map(|q| tr::QueuedMessage { text: shown(&q.text), images: q.images.len() as u32, reset: true });
        turn.chain(reset).collect()
    }

    /// Drop or change a queued follow-up, if it's still the one the phone shows. A dropped
    /// one's text comes back.
    pub(super) fn remote_queue_action(&mut self, req: tr::QueueActionRequest, cx: &mut Context<Self>) -> tr::HostResult<Option<String>> {
        let id = req.thread_id.as_str();
        if self.thread(id).is_none() {
            return Err(tr::HostError::not_found("No such thread"));
        }
        let ix = req.index as usize;
        let (text, images) = match req.reset {
            false => self.live.get(id).and_then(|l| l.queued.get(ix)).map(|(t, i)| (t.clone(), i.len())),
            true => self.pause(id).and_then(|p| p.queued.get(ix)).map(|q| (q.text.clone(), q.images.len())),
        }
        .filter(|(t, _)| shown(t) == req.text)
        .ok_or_else(|| tr::HostError::conflict("That message isn't queued any more"))?;
        let edit = match req.action {
            tr::QueueAction::Remove => None,
            tr::QueueAction::Edit { text } if text.trim().is_empty() && images == 0 => return Err(tr::HostError::bad_request("Write something, or remove it")),
            tr::QueueAction::Edit { text } => Some(text),
        };
        if req.reset {
            let Some(t) = self.threads.iter_mut().find(|t| t.id == id) else { return Err(tr::HostError::not_found("No such thread")) };
            if let Some(p) = t.paused.as_mut() {
                match &edit {
                    Some(new) => p.queued[ix].text = new.clone(),
                    None => {
                        p.queued.remove(ix);
                    }
                }
            }
            let _ = self.store.save_thread(t);
            if let Some(l) = self.live.get_mut(id) {
                l.revision += 1;
            }
            self.schedule_limits(cx);
        } else if let Some(l) = self.live.get_mut(id) {
            match &edit {
                Some(new) => l.queued[ix].0 = new.clone(),
                None => {
                    l.queued.remove(ix);
                }
            }
        }
        self.threads_gen += 1;
        cx.notify();
        Ok(edit.is_none().then_some(text))
    }
}
