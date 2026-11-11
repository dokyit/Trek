//! Files from the phone (feature `uploads`; the wire is `trek_remote`'s `protocol/uploads.rs`): a
//! video from its photo library, or any file from the Files app, sent in pieces before the message
//! that takes it. Each piece is written after the one before it, off the main thread, into
//! `<upload id>.part` in the data folder's `uploads/`; the last one renames it
//! `iPhone <date> at <time> <name>`. A message names its files by upload id (`send.files`,
//! `new_thread.files`) and goes with an `@path` mention of each after its text, as the Mac's
//! composer mentions a file it doesn't attach: the agent reads it from there, and a video plays
//! under the message (`media::mentioned_videos`) on the Mac and the phone alike.
//!
//! What's part-way up is dropped once no piece came for an hour (the phone went), and `.part` files
//! a day old (a Trek that quit mid-upload) when the phone server starts. Finished files stay, as
//! the phone's photos do (`snapshots/`): a message names them.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};
use trek_remote as tr;

/// Where phones' files are kept.
pub fn uploads_dir() -> PathBuf {
    trek_core::paths::data_dir().join("uploads")
}

/// An upload no piece came for in this long was dropped by its phone.
const IDLE: Duration = Duration::from_secs(60 * 60);
/// A `.part` file this old when the server starts is a Trek that quit mid-upload; a finished
/// upload's id is forgotten this long after (its file stays).
pub const STALE: Duration = Duration::from_secs(24 * 60 * 60);
/// Uploads part-way up from every phone at once (each phone has
/// [`tr::MAX_UPLOADS_IN_FLIGHT`] of them).
const MAX_PARTIAL: usize = 16;
/// What an upload leaves free on the disk, at least.
const HEADROOM: u64 = 1 << 30;

struct Partial {
    name: String,
    total: u64,
    received: u64,
    part: PathBuf,
    touched: Instant,
    /// A piece is being written (with the table let go).
    writing: bool,
}

struct Finished {
    path: PathBuf,
    at: Instant,
}

#[derive(Default)]
struct Table {
    partial: HashMap<String, Partial>,
    finished: HashMap<String, Finished>,
}

impl Table {
    /// Uploads gone quiet are dropped (their `.part` with them); finished ones long ago forgotten.
    fn sweep(&mut self, now: Instant) {
        self.partial.retain(|_, p| {
            let keep = p.writing || now.duration_since(p.touched) < IDLE;
            if !keep {
                let _ = std::fs::remove_file(&p.part);
            }
            keep
        });
        self.finished
            .retain(|_, f| now.duration_since(f.at) < STALE);
    }
}

static UPLOADS: LazyLock<Mutex<Table>> = LazyLock::new(Default::default);

fn table() -> MutexGuard<'static, Table> {
    UPLOADS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `upload_part`, answered once the piece is on disk (decoded and written off the main thread).
pub(super) fn part(
    req: tr::UploadPart,
    reply: tr::Reply<tr::UploadProgress>,
    cx: &mut Context<Workspace>,
) {
    let dir = uploads_dir();
    cx.spawn(async move |_, cx| {
        let done = cx
            .background_executor()
            .spawn(async move { receive(&dir, req) })
            .await;
        let _ = reply.send(done);
    })
    .detach();
}

/// One piece of an upload, into `dir`. A piece refused (or that couldn't be written) ends its
/// upload: the phone sends the file again, under a new id.
fn receive(dir: &Path, req: tr::UploadPart) -> tr::HostResult<tr::UploadProgress> {
    let id = req.upload_id.to_ascii_lowercase();
    let done = req
        .decode()
        .map_err(tr::HostError::bad_request)
        .and_then(|piece| take(dir, piece));
    // A conflict is about another piece of it being written: that one carries on.
    if done
        .as_ref()
        .is_err_and(|e| e.code != tr::ErrorCode::Conflict)
    {
        let mut t = table();
        if t.partial.get(&id).is_some_and(|p| !p.writing)
            && let Some(p) = t.partial.remove(&id)
        {
            let _ = std::fs::remove_file(&p.part);
        }
    }
    done
}

fn take(dir: &Path, piece: tr::UploadPiece) -> tr::HostResult<tr::UploadProgress> {
    let id = piece.upload_id.clone();
    let (part, fresh) = {
        let mut t = table();
        t.sweep(Instant::now());
        if t.finished.contains_key(&id) {
            return Err(tr::HostError::conflict("That file is on the Mac already"));
        }
        match t.partial.get_mut(&id) {
            Some(p) if p.writing => {
                return Err(tr::HostError::conflict(
                    "A piece of that file is still being written",
                ));
            }
            Some(p) if p.name != piece.name || p.total != piece.total => {
                return Err(tr::HostError::bad_request(
                    "That piece is of another file than the pieces before it",
                ));
            }
            // From the start again (the phone lost track of how far it got).
            Some(p) if piece.offset == 0 => {
                p.received = 0;
                p.writing = true;
                (p.part.clone(), true)
            }
            Some(p) if piece.offset != p.received => {
                return Err(tr::HostError::bad_request(format!(
                    "That piece starts at byte {}, but the next one starts at byte {}",
                    piece.offset, p.received
                )));
            }
            Some(p) => {
                p.writing = true;
                (p.part.clone(), false)
            }
            None if piece.offset != 0 => {
                return Err(tr::HostError::bad_request(
                    "That upload isn't on the Mac (it may have restarted since): send the file again",
                ));
            }
            None => {
                if t.partial.len() >= MAX_PARTIAL {
                    return Err(tr::HostError::new(
                        tr::ErrorCode::RateLimited,
                        "The Mac is taking in too many files at once: try again in a moment",
                    ));
                }
                std::fs::create_dir_all(dir)
                    .map_err(|e| tr::HostError::other(format!("Couldn't keep that file: {e}")))?;
                room_for(dir, piece.total)?;
                let part = dir.join(format!("{id}.part"));
                t.partial.insert(
                    id.clone(),
                    Partial {
                        name: piece.name.clone(),
                        total: piece.total,
                        received: 0,
                        part: part.clone(),
                        touched: Instant::now(),
                        writing: true,
                    },
                );
                (part, true)
            }
        }
    };
    // Written with the table let go: other phones' files carry on meanwhile.
    let wrote = write_at(&part, fresh, piece.offset, &piece.bytes);
    let mut t = table();
    let Some(p) = t.partial.get_mut(&id) else {
        return Err(tr::HostError::other("That upload was dropped"));
    };
    p.writing = false;
    p.touched = Instant::now();
    wrote.map_err(|e| tr::HostError::other(format!("Couldn't keep that file: {e}")))?;
    p.received = piece.offset + piece.bytes.len() as u64;
    let (received, total) = (p.received, p.total);
    if received < total {
        return Ok(tr::UploadProgress {
            upload_id: id,
            received,
            total,
            done: false,
        });
    }
    let p = t.partial.remove(&id).expect("just found");
    let path = keep(dir, &p.part, &p.name).map_err(|e| {
        let _ = std::fs::remove_file(&p.part);
        tr::HostError::other(format!("Couldn't keep that file: {e}"))
    })?;
    t.finished.insert(
        id.clone(),
        Finished {
            path,
            at: Instant::now(),
        },
    );
    Ok(tr::UploadProgress {
        upload_id: id,
        received,
        total,
        done: true,
    })
}

/// `bytes` at `offset` of `part` (emptied first when `fresh`).
fn write_at(part: &Path, fresh: bool, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek as _, SeekFrom, Write as _};
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(fresh)
        .open(part)?;
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(bytes)?;
    // What's past this piece (a file sent again from an earlier piece) isn't the file's.
    f.set_len(offset + bytes.len() as u64)
}

/// The whole file, from its `.part`, kept as `iPhone <date> at <time> <name>` (as the phone's
/// photos are, `save_uploads`), with ` 2`, ` 3`… before the extension when that's taken. Called
/// with the table held: no other upload takes the name meanwhile.
fn keep(dir: &Path, part: &Path, name: &str) -> std::io::Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    for n in 1u32.. {
        let file = if n == 1 {
            format!("iPhone {stamp} {name}")
        } else {
            format!("iPhone {stamp} {stem} {n}{ext}")
        };
        let path = dir.join(file);
        if std::fs::symlink_metadata(&path).is_err() {
            std::fs::rename(part, &path)?;
            return Ok(path);
        }
    }
    unreachable!("a free name comes first")
}

/// Refuses a file the disk hasn't room for (with [`HEADROOM`] to spare).
fn room_for(dir: &Path, total: u64) -> tr::HostResult<()> {
    match free_bytes(dir) {
        Some(free) if free < total.saturating_add(HEADROOM) => {
            Err(tr::HostError::bad_request(format!(
                "{} has {} free: not enough to take a file of {}",
                upper_first(crate::words::words().this_computer),
                size_words(free),
                size_words(total)
            )))
        }
        _ => Ok(()),
    }
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)] // the field types differ between macOS and Linux
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is NUL-terminated, and `stats` is a `statvfs` for the call to fill in.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::statvfs(path.as_ptr(), &mut stats) } == 0;
    ok.then(|| u64::from(stats.f_bavail).saturating_mul(u64::from(stats.f_frsize)))
}

/// The bytes free to this user on `dir`'s volume (a quota counts), as Windows says.
#[cfg(windows)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path; the one count asked for is written to `free`,
    // and the other two may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0;
    ok.then_some(free)
}

#[cfg(not(any(unix, windows)))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// "820 MB", "1.5 GB".
fn size_words(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(1 << 20))
    }
}

/// The files `ids` name (each once, in order): every one sent whole and still on disk. Anything
/// else is a `bad_request` saying what, so nothing is sent without its files.
pub(super) fn finished(ids: &[String]) -> tr::HostResult<Vec<PathBuf>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let t = table();
    let mut out: Vec<PathBuf> = Vec::with_capacity(ids.len());
    for id in ids {
        if !tr::valid_upload_id(id) {
            return Err(tr::HostError::bad_request(format!(
                "{id:?} isn't an upload id (a UUID)"
            )));
        }
        let key = id.to_ascii_lowercase();
        let path = match t.finished.get(&key) {
            Some(f) if f.path.is_file() => f.path.clone(),
            Some(_) => {
                return Err(tr::HostError::bad_request(
                    "A file sent with this message isn't on the Mac any more: attach it again",
                ));
            }
            None if t.partial.contains_key(&key) => {
                return Err(tr::HostError::bad_request(
                    "A file sent with this message hasn't finished uploading",
                ));
            }
            None => {
                return Err(tr::HostError::bad_request(format!(
                    "No file was uploaded as {id} (the Mac may have restarted since): attach it again"
                )));
            }
        };
        if !out.contains(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

/// `text` with an `@path` mention of each of `files` after it, a space between, as the Mac's
/// composer adds a file it doesn't attach (`append_file_refs`).
pub(super) fn mentioning(text: &str, files: &[PathBuf]) -> String {
    if files.is_empty() {
        return text.to_string();
    }
    let refs: Vec<String> = files
        .iter()
        .map(|p| trek_core::media::mention_of(&p.display().to_string()))
        .collect();
    let text = text.trim_end();
    if text.is_empty() {
        refs.join(" ")
    } else {
        format!("{text} {}", refs.join(" "))
    }
}

/// Takes out the `.part` files in `dir` last written `older_than` ago or more: a Trek that quit,
/// or a phone that went, mid-upload. Returns how many went.
pub fn sweep_stale(dir: &Path, older_than: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "part"))
        .filter(|e| {
            e.metadata()
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| m.modified().ok())
                .is_some_and(|at| now.duration_since(at).unwrap_or_default() >= older_than)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_folder_says_how_much_room_it_has() {
        let free = super::free_bytes(&std::env::temp_dir());
        assert!(free.is_some_and(|f| f > 0), "{free:?}");
        // One that isn't there says nothing: the upload isn't held back on a guess.
        let gone = std::env::temp_dir().join(format!("trek-no-such-folder-{}", std::process::id()));
        assert_eq!(super::free_bytes(&gone), None);
    }
}
