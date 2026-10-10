//! Notes from the phone: the same markdown files the Notes screen keeps (`trek_core::notes`),
//! read and written whole. Deleting moves a note to the notes folder's `Deleted/`, as on the
//! Mac. The Notes screen reads them again when a phone changes one (`notes_epoch`).

use crate::workspace::Workspace;
use gpui_kit::Context;
use trek_core::notes;
use trek_remote as tr;

/// The longest note a phone may write (a megabyte of markdown is a lot of notes).
const MAX_BODY: usize = 1 << 20;

/// Every note, newest first.
pub(super) fn list() -> Vec<tr::NoteSummary> {
    notes::list_in(&notes::notes_dir()).into_iter().map(|n| tr::NoteSummary { title: n.title(), preview: n.preview(), id: n.id, modified: n.modified }).collect()
}

/// The note called `id`, if it's one of the list's (an id is never a path).
fn find(id: &str) -> Option<notes::Note> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    notes::list_in(&notes::notes_dir()).into_iter().find(|n| n.id == id)
}

fn whole(n: notes::Note) -> tr::Note {
    tr::Note { title: n.title(), id: n.id, body: n.body, modified: n.modified }
}

pub(super) fn get(id: &str) -> tr::HostResult<tr::Note> {
    find(id).map(whole).ok_or_else(|| tr::HostError::not_found("No such note"))
}

fn too_big(body: &str) -> tr::HostResult<()> {
    if body.len() > MAX_BODY {
        return Err(tr::HostError::bad_request("That note is too long (a megabyte at most)"));
    }
    Ok(())
}

impl Workspace {
    pub(super) fn remote_create_note(&mut self, body: String, cx: &mut Context<Self>) -> tr::HostResult<tr::Note> {
        too_big(&body)?;
        let mut note = notes::create_in(&notes::notes_dir()).map_err(|e| tr::HostError::other(format!("Couldn't make a note: {e}")))?;
        if !body.is_empty() {
            note.body = body;
            notes::save(&note).map_err(|e| tr::HostError::other(format!("Couldn't save the note: {e}")))?;
        }
        self.notes_changed(cx);
        get(&note.id)
    }

    pub(super) fn remote_save_note(&mut self, req: tr::SaveNoteRequest, cx: &mut Context<Self>) -> tr::HostResult<tr::Note> {
        too_big(&req.body)?;
        let mut note = find(&req.note_id).ok_or_else(|| tr::HostError::not_found("No such note"))?;
        // Edited on the Mac since the phone read it: the phone's copy would undo that.
        if req.modified.is_some_and(|m| m != note.modified) {
            return Err(tr::HostError::conflict(format!("This note changed on {} since you opened it. Open it again to see the change.", crate::words::words().the_computer)));
        }
        if note.body != req.body {
            note.body = req.body;
            notes::save(&note).map_err(|e| tr::HostError::other(format!("Couldn't save the note: {e}")))?;
            self.notes_changed(cx);
        }
        get(&note.id)
    }

    pub(super) fn remote_delete_note(&mut self, id: &str, cx: &mut Context<Self>) -> tr::HostResult<()> {
        let note = find(id).ok_or_else(|| tr::HostError::not_found("No such note"))?;
        notes::delete(&note).map_err(|e| tr::HostError::other(format!("Couldn't delete the note: {e}")))?;
        self.notes_changed(cx);
        Ok(())
    }

    /// A phone changed the notes: the Notes screen reads them again.
    fn notes_changed(&mut self, cx: &mut Context<Self>) {
        self.notes_epoch += 1;
        cx.notify();
    }
}
