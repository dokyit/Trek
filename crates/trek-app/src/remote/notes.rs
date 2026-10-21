//! Notes from the phone: the same markdown files the Notes screen keeps (`trek_core::notes`),
//! read and written whole. Deleting moves a note to the notes folder's `Deleted/`, as on the
//! Mac. The Notes screen reads them again when a phone changes one (`notes_epoch`). A note's
//! pictures stay on the Mac: the phone gets their markdown links (`![alt](attachments/…)`) as
//! text and saves them back untouched, and the list's titles and previews read an image as its
//! alt text. It asks for a picture to draw (`note_asset`, scaled down as a message's images
//! are), only ever from the notes' `attachments/`, and keeps a photo of its own there
//! (`add_note_image`) to link from a note as the Mac's Write does. Any other file kept there (a
//! PDF, a video) goes as it is, a mebibyte at a time (`note_file`), for the phone to keep and open.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::path::{Path, PathBuf};
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

/// The file (a picture, a PDF, a video) a note links as `path`, in the notes folder `dir`: only
/// `attachments/<name>` (or `./attachments/<name>`), never an absolute path, one with `..` in it,
/// or a symlink out of the folder.
fn asset_path(dir: &Path, path: &str) -> tr::HostResult<PathBuf> {
    let refused = || tr::HostError::bad_request("Only files kept with the notes can be sent");
    let prefix = format!("{}/", notes::ATTACHMENTS);
    let name = path.strip_prefix("./").unwrap_or(path).strip_prefix(prefix.as_str()).ok_or_else(refused)?;
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) || name.contains("..") {
        return Err(refused());
    }
    let folder = notes::attachments_dir(dir);
    let gone = || tr::HostError::not_found(format!("That file isn't on {} any more", crate::words::words().this_computer));
    let (folder, file) = (folder.canonicalize().map_err(|_| gone())?, folder.join(name).canonicalize().map_err(|_| gone())?);
    if !file.starts_with(&folder) || !file.is_file() {
        return Err(refused());
    }
    Ok(file)
}

/// `note_asset`: one of a note's pictures, as a JPEG, made off the main thread.
pub(super) fn asset(req: tr::NoteAssetRequest, reply: tr::Reply<tr::NoteAsset>, cx: &mut Context<Workspace>) {
    let file = find(&req.note_id).ok_or_else(|| tr::HostError::not_found("No such note")).and_then(|_| asset_path(&notes::notes_dir(), &req.path));
    let max = req.max_px.unwrap_or(tr::MAX_ATTACHMENT_PX).clamp(64, tr::MAX_ATTACHMENT_PX);
    cx.spawn(async move |_, cx| {
        let made = match file {
            Ok(file) => cx.background_executor().spawn(async move { super::attachments::jpeg(&file, max) }).await,
            Err(e) => Err(e),
        };
        let _ = reply.send(made.map(|(data, width, height)| tr::NoteAsset { note_id: req.note_id, path: req.path, mime: "image/jpeg".into(), data: tr::base64(&data), width, height }));
    })
    .detach();
}

/// `note_file`: a piece of one of the notes' files as it is on disk, read off the main thread.
pub(super) fn file(req: tr::NoteFileRequest, reply: tr::Reply<tr::NoteFileChunk>, cx: &mut Context<Workspace>) {
    let file = asset_path(&notes::notes_dir(), &req.path);
    cx.spawn(async move |_, cx| {
        let read = match file {
            Ok(file) => cx.background_executor().spawn(async move { chunk(&file, req) }).await,
            Err(e) => Err(e),
        };
        let _ = reply.send(read);
    })
    .detach();
}

/// `req.length` bytes (a mebibyte at most) of `file` from `req.offset`.
fn chunk(file: &Path, req: tr::NoteFileRequest) -> tr::HostResult<tr::NoteFileChunk> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let failed = |e: std::io::Error| tr::HostError::other(format!("Couldn't read that file: {e}"));
    let mut f = std::fs::File::open(file).map_err(failed)?;
    let total = f.metadata().map_err(failed)?.len();
    if req.offset > total {
        return Err(tr::HostError::bad_request(format!("That file is only {total} bytes long")));
    }
    let length = req.length.unwrap_or(tr::MAX_NOTE_FILE_CHUNK).min(tr::MAX_NOTE_FILE_CHUNK).min(total - req.offset);
    f.seek(SeekFrom::Start(req.offset)).map_err(failed)?;
    let mut bytes = Vec::with_capacity(length as usize);
    f.take(length).read_to_end(&mut bytes).map_err(failed)?;
    Ok(tr::NoteFileChunk { mime: mime(file).into(), path: req.path, offset: req.offset, total, bytes_b64: tr::base64(&bytes) })
}

/// What a file kept with the notes is, from its extension.
fn mime(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("pdf") => "application/pdf",
        Some("mp4") => "video/mp4",
        Some("m4v") => "video/x-m4v",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("heic") => "image/heic",
        Some("webp") => "image/webp",
        Some("txt" | "md") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The most a photo from the phone may weigh, as for a message's.
const MAX_IMAGE: usize = 10 << 20;

/// `add_note_image`: keep a photo with the notes, as the Mac's Write keeps one dropped in, and
/// say how to link it. The note itself is the phone's to change.
pub(super) fn add_image(req: tr::AddNoteImageRequest) -> tr::HostResult<tr::NoteImage> {
    find(&req.note_id).ok_or_else(|| tr::HostError::not_found("No such note"))?;
    let upload = tr::ImageUpload { mime: "image/jpeg".into(), data: req.data };
    let (bytes, ext) = upload.decode().map_err(|e| tr::HostError::bad_request(format!("That photo couldn't be read: {e}")))?;
    if bytes.len() > MAX_IMAGE {
        return Err(tr::HostError::bad_request("That photo is too big (10 MB at most)"));
    }
    let path = notes::add_image(&notes::notes_dir(), &bytes, ext).map_err(|e| tr::HostError::other(format!("Couldn't keep the photo: {e}")))?;
    let markdown = notes::image_markdown(req.name.as_deref().unwrap_or_default(), &path);
    Ok(tr::NoteImage { note_id: req.note_id, path, markdown })
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

#[cfg(test)]
mod tests {
    use super::asset_path;
    use trek_remote::ErrorCode;

    #[test]
    fn only_the_notes_own_pictures_are_sent() {
        let dir = std::env::temp_dir().join(format!("trek-note-assets-{}", uuid::Uuid::new_v4()));
        let pictures = dir.join("attachments");
        std::fs::create_dir_all(&pictures).unwrap();
        std::fs::write(pictures.join("a1.png"), b"png").unwrap();
        std::fs::write(dir.join("secret.md"), b"no").unwrap();
        let found = asset_path(&dir, "attachments/a1.png").unwrap();
        assert_eq!(found, pictures.join("a1.png").canonicalize().unwrap());
        assert_eq!(asset_path(&dir, "./attachments/a1.png").unwrap(), found, "as Write links them too");
        let code = |path: &str| asset_path(&dir, path).unwrap_err().code;
        for path in [
            "attachments/../secret.md",
            "attachments/..",
            "../attachments/a1.png",
            "/etc/passwd",
            "attachments//etc/passwd",
            "attachments/sub/a1.png",
            "attachments\\..\\secret.md",
            "secret.md",
            "attachments/",
            "attachments/.hidden",
            "~/attachments/a1.png",
            "file:///etc/passwd",
        ] {
            assert_eq!(code(path), ErrorCode::BadRequest, "{path}");
        }
        assert_eq!(code("attachments/gone.png"), ErrorCode::NotFound);
        // A link out of the folder is refused like a path out of it.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("secret.md"), pictures.join("sneaky.png")).unwrap();
            assert_eq!(code("attachments/sneaky.png"), ErrorCode::BadRequest);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
