//! Files and pictures from this computer's composer going to a thread on another machine (feature
//! `machine_files`, Trek 0.4.61 on the machine).
//!
//! On send, every attachment is sent ahead of the message over the machine's own connection, as
//! the phone sends its files (`upload_part`, in pieces of 4 MiB, 2 GB a file at most): the strip's
//! pictures (as images) and files, and the files of this computer that what's typed mentions by an
//! absolute or `~/` path. The composer shows how far each got and holds the message until they're
//! all in (`composer::machine_upload`). The machine keeps them in its data folder's `uploads/`, as
//! `<this computer's name> <date> at <time> <name>`, and says where. The message then goes with
//! each mention of a file swapped for the machine's path to its copy, in place, and each picture
//! named by its upload id in `files`, so the machine attaches it as an image the model sees
//! (`shaped`). This computer remembers which copy there is which file here (`MachineEntry::sent`),
//! so the transcript shows its own: a video plays from the file here, a picture shows (`shown_here`).
//!
//! A folder goes as one zip of its files (`pack_folder`: what git ignores and `.git` left out),
//! named `<folder> folder.zip` so the agent there can tell what it is; here the message still
//! shows the folder. Until 0.4.67 a folder was refused ("Folders can't go to T-Book yet").
//!
//! What can't go says why, before anything is sent (`refusal`): the machine runs an older Trek, a
//! file over the limit or gone. A machine without `machine_files` still takes pictures the old
//! way, inline in the message (JPEG or PNG).

use super::{Machine, Workspace, split_id};
use crate::workspace::Scope;
use gpui_kit::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_remote as tr;
use trek_remote::client::Client;

/// What goes with a message: a picture on the strip, or a file (on the strip or mentioned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub path: PathBuf,
    /// Attached as an image (the strip's thumbnails), not mentioned.
    pub image: bool,
}

/// A file sent ahead of its message, waiting for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub upload_id: String,
    /// Where the machine keeps it, as it writes paths.
    pub there: String,
    pub image: bool,
}

/// Which file here a copy on the machine is (`machines.json` keeps the last [`SENT_KEPT`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentCopy {
    pub there: String,
    pub here: String,
}

/// How many copies sent to one machine are remembered, newest kept.
pub const SENT_KEPT: usize = 500;

/// How the attachments of a message to a machine go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Nothing to send first (none, or all sent already): the message goes now.
    Ready,
    /// Pictures alone, to a machine without `machine_files`: inline in the message, as before.
    Inline,
    /// Sent ahead of the message, these.
    Upload(Vec<Outgoing>),
    /// They can't go: why.
    Refuse(String),
}

/// This computer's path that a mention names (absolute, or `~/`), whatever is there.
fn mentioned_path(word: &str) -> Option<PathBuf> {
    if let Some(rest) = word.strip_prefix("~/") {
        Some(trek_core::paths::home().join(rest))
    } else if Path::new(word).is_absolute() {
        Some(PathBuf::from(word))
    } else {
        None
    }
}

/// This computer's file that a mention names (absolute, or `~/`), when it's a file.
pub fn mentioned_local(word: &str) -> Option<PathBuf> {
    mentioned_path(word).filter(|p| p.is_file())
}

/// The most files of one folder that go in its zip.
const FOLDER_FILES: usize = 20_000;

/// Why a folder couldn't be packed to go to a machine, as a toast says it after the folder's name.
#[derive(Debug, PartialEq, Eq)]
pub enum PackError {
    /// It holds no file that would go (empty, or everything in it is ignored by git).
    Empty,
    /// Its files come to more than one upload may be.
    TooBig(u64),
    /// More files than a zip of it takes.
    TooMany,
    Io(String),
}

impl PackError {
    pub fn words(&self, folder: &str, machine: &str) -> String {
        match self {
            PackError::Empty => format!(
                "{folder} has no files to send to {machine} (what git ignores stays behind)."
            ),
            PackError::TooBig(bytes) => format!(
                "{folder} is {}: folders of up to {} can go to {machine}.",
                crate::attachments::size_label(*bytes),
                tr::upload_size_words(tr::MAX_UPLOAD)
            ),
            PackError::TooMany => format!(
                "{folder} has more than {FOLDER_FILES} files: too many to send to {machine} at once."
            ),
            PackError::Io(why) => format!("Couldn't pack {folder} to send to {machine}: {why}"),
        }
    }
}

/// The name a folder's zip goes under: `assets folder.zip`, so what receives it can tell it's a
/// folder to unpack.
pub fn folder_zip_name(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "folder".to_string());
    format!("{name} folder.zip")
}

/// Zip folder `dir` into a new folder under `into`, to go to a machine as one file: its files as
/// git would list them (what `.gitignore` leaves out stays behind, and `.git` itself), under the
/// folder's own name. Symbolic links aren't followed. Blocks on the disk.
pub fn pack_folder(dir: &Path, into: &Path) -> Result<PathBuf, PackError> {
    use std::io::Write as _;
    let io = |e: std::io::Error| PackError::Io(e.to_string());
    let top = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "folder".to_string());
    let walk = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .parents(true)
        .ignore(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        // A `.gitignore` in the folder counts whether or not the folder is in a repository.
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let mut files: Vec<(PathBuf, String, u64)> = vec![];
    let mut total = 0u64;
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let rel = crate::mentions::rel_string(rel);
        if rel.is_empty() || rel.ends_with(".DS_Store") {
            continue;
        }
        let len = entry.metadata().map_or(0, |m| m.len());
        total += len;
        if total > tr::MAX_UPLOAD {
            return Err(PackError::TooBig(total));
        }
        if files.len() == FOLDER_FILES {
            return Err(PackError::TooMany);
        }
        files.push((entry.path().to_path_buf(), format!("{top}/{rel}"), len));
    }
    if files.is_empty() {
        return Err(PackError::Empty);
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    std::fs::create_dir_all(into).map_err(io)?;
    let path = into.join(folder_zip_name(dir));
    let made = (|| -> std::io::Result<()> {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path)?);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        for (file, name, _) in &files {
            zip.start_file(name.as_str(), options)
                .map_err(std::io::Error::other)?;
            std::io::copy(&mut std::fs::File::open(file)?, &mut zip)?;
        }
        zip.finish().map_err(std::io::Error::other)?.flush()
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_file(&path);
        return Err(io(e));
    }
    let packed = std::fs::metadata(&path).map_err(io)?.len();
    if packed > tr::MAX_UPLOAD {
        let _ = std::fs::remove_file(&path);
        return Err(PackError::TooBig(packed));
    }
    Ok(path)
}

/// What goes with a message of `typed` words: the strip's `images` and `files`, then the files of
/// this computer `typed` mentions; each once, in that order.
pub fn outgoing(typed: &str, images: &[PathBuf], files: &[PathBuf]) -> Vec<Outgoing> {
    let mut out: Vec<Outgoing> = Vec::new();
    let mut add = |path: PathBuf, image: bool| {
        if !out.iter().any(|o| o.path == path) {
            out.push(Outgoing { path, image });
        }
    };
    for p in images {
        add(p.clone(), true);
    }
    for p in files {
        add(p.clone(), false);
    }
    let mut mentioned = vec![];
    if typed.contains('@') {
        trek_core::media::rewrite_mentions(typed, |w| {
            if let Some(p) = mentioned_local(w) {
                mentioned.push(p);
            }
            None
        });
    }
    for p in mentioned {
        add(p, false);
    }
    out
}

/// Why `items` can't go to the machine called `name` (`None`: they can). `serves`: it runs a Trek
/// that takes files (`feature::MACHINE_FILES`).
pub fn refusal(name: &str, serves: bool, items: &[Outgoing]) -> Option<String> {
    let label = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.display().to_string())
    };
    if !serves && items.iter().any(|o| !o.image) {
        return Some(format!(
            "{name} needs Trek {} or later to receive files. Update Trek there, or start the thread on {}.",
            tr::MACHINE_FILES_SINCE,
            crate::words::words().this_computer
        ));
    }
    for o in items {
        match std::fs::metadata(&o.path) {
            // A folder is sized as it's packed (`pack_folder`), off the main thread.
            Ok(m) if m.is_dir() => {}
            Err(_) => {
                return Some(format!(
                    "{} isn't there any more. Take it off and attach it again.",
                    label(&o.path)
                ));
            }
            Ok(m) if m.len() > tr::MAX_UPLOAD => {
                return Some(format!(
                    "{} is {}: files of up to {} can go to {name}.",
                    label(&o.path),
                    crate::attachments::size_label(m.len()),
                    tr::upload_size_words(tr::MAX_UPLOAD)
                ));
            }
            Ok(_) => {}
        }
    }
    None
}

/// A message's `text` and `images` as the machine takes them, with what was sent ahead in
/// `pending`: each mention of a sent file becomes a mention of its copy there, in place; each sent
/// picture goes by its upload id (the `files` of `send` and `new_thread`), the others inline as
/// before. Returns the text, those ids, and the pictures to send inline.
pub fn shaped(
    text: &str,
    images: &[PathBuf],
    pending: &HashMap<PathBuf, Sent>,
) -> (String, Vec<String>, Vec<PathBuf>) {
    let text = if pending.is_empty() || !text.contains('@') {
        text.to_string()
    } else {
        // A folder's mention too: its zip there stands for it.
        trek_core::media::rewrite_mentions(text, |w| {
            mentioned_path(w)
                .and_then(|p| pending.get(&p))
                .filter(|s| !s.image)
                .map(|s| s.there.clone())
        })
    };
    let mut ids = vec![];
    let mut inline = vec![];
    for p in images {
        match pending.get(p).filter(|s| s.image) {
            Some(s) => ids.push(s.upload_id.clone()),
            None => inline.push(p.clone()),
        }
    }
    (text, ids, inline)
}

/// `text` from the machine with each mention of a copy this computer sent it swapped back for
/// the file here, while that's still here.
pub fn shown_here(text: &str, sent: &[SentCopy]) -> String {
    if sent.is_empty() || !text.contains('@') {
        return text.to_string();
    }
    trek_core::media::rewrite_mentions(text, |w| here_of(w, sent))
}

/// The file here that the machine's copy `there` is, while it's still here.
pub fn here_of(there: &str, sent: &[SentCopy]) -> Option<String> {
    sent.iter()
        .rev()
        .find(|c| c.there == there)
        .map(|c| c.here.clone())
        .filter(|h| Path::new(h).exists())
}

impl Machine {
    /// Remember that the files of `sent` are on it now, to send their message with, and which
    /// file here each copy is.
    fn took(&mut self, sent: Vec<(PathBuf, Sent)>) {
        for (here, s) in sent {
            let there = s.there.clone();
            self.pending.insert(here.clone(), s);
            self.entry.sent.retain(|c| c.there != there);
            self.entry.sent.push(SentCopy {
                there,
                here: here.display().to_string(),
            });
        }
        let extra = self.entry.sent.len().saturating_sub(SENT_KEPT);
        self.entry.sent.drain(..extra);
    }
}

impl Workspace {
    /// The machine (its host id) a message sent from `scope` goes to, when it isn't this
    /// computer: the thread's own machine, or the one the main window's draft starts on.
    pub fn machine_host_for_send(&self, scope: &Scope) -> Option<String> {
        let host = match self.thread_id_in(scope) {
            Some(id) => split_id(id).map(|(host, _)| host.to_string()),
            None if matches!(scope, Scope::Main) => match &self.route {
                crate::workspace::Route::Draft { project } => {
                    self.draft_machine(project.as_deref())
                }
                _ => None,
            },
            None => None,
        }?;
        self.machines.get(&host).map(|m| m.id().to_string())
    }

    /// How `items` go with a message from `scope` to machine `host`. A new thread in a project
    /// the machine has no copy of is refused here, before anything is sent for it.
    pub fn machine_files_plan(&self, scope: &Scope, host: &str, items: &[Outgoing]) -> Plan {
        if self.thread_id_in(scope).is_none()
            && matches!(scope, Scope::Main)
            && let crate::workspace::Route::Draft {
                project: Some(project),
            } = &self.route
            && let Some(why) = self.machine_lacks_project(host, project)
        {
            return Plan::Refuse(why);
        }
        let Some(m) = self.machines.get(host) else {
            return Plan::Refuse("That machine isn't paired any more.".into());
        };
        let items: Vec<Outgoing> = items
            .iter()
            .filter(|o| !m.pending.contains_key(&o.path))
            .cloned()
            .collect();
        if items.is_empty() {
            return Plan::Ready;
        }
        let serves = m.entry.paired.serves(tr::feature::MACHINE_FILES);
        if let Some(why) = refusal(&m.entry.name, serves, &items) {
            return Plan::Refuse(why);
        }
        if !serves {
            return Plan::Inline;
        }
        if let Some(why) = self.machine_unavailable_why(host) {
            return Plan::Refuse(why);
        }
        Plan::Upload(items)
    }

    /// The connection files go to machine `host` on, and its name.
    pub fn machine_client(&self, host: &str) -> Option<(Arc<Client>, String)> {
        let m = self.machines.get(host)?;
        Some((m.client.clone()?, m.entry.name.clone()))
    }

    /// The files of `sent` are on machine `host` now: its next message takes them.
    pub fn machine_took_files(
        &mut self,
        host: &str,
        sent: Vec<(PathBuf, Sent)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self.machines.get_mut(host) {
            m.took(sent);
            self.machines.save();
            cx.notify();
        }
    }

    /// `text` and `images` for machine `host`, shaped by what was sent ahead (`shaped`), which
    /// they now take with them.
    pub(super) fn machine_shaped(
        &mut self,
        host: &str,
        text: &str,
        images: &[PathBuf],
    ) -> (String, Vec<String>, Vec<PathBuf>) {
        let Some(m) = self.machines.get_mut(host) else {
            return (text.to_string(), vec![], images.to_vec());
        };
        let shaped = shaped(text, images, &m.pending);
        m.pending.clear();
        shaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("trek-machine-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn what_goes_is_the_strip_then_the_files_typed_each_once() {
        let d = dir("outgoing");
        let (shot, clip, notes) = (
            d.join("shot.png"),
            d.join("clip.mov"),
            d.join("My notes.txt"),
        );
        for p in [&shot, &clip, &notes] {
            std::fs::write(p, b"x").unwrap();
        }
        let typed = format!(
            "look at @{} and @\"{}\", and @src/main.rs and @{}/gone.txt",
            clip.display(),
            notes.display(),
            d.display()
        );
        let out = outgoing(
            &typed,
            std::slice::from_ref(&shot),
            std::slice::from_ref(&clip),
        );
        assert_eq!(
            out,
            [
                Outgoing {
                    path: shot,
                    image: true
                },
                Outgoing {
                    path: clip,
                    image: false
                },
                Outgoing {
                    path: notes,
                    image: false
                },
            ],
            "a relative mention is the project's, and one that's gone is left as typed"
        );
    }

    #[test]
    fn an_older_trek_a_folder_a_file_too_big_or_gone_say_why() {
        let d = dir("refusal");
        let mov = d.join("724640981761510973.mov");
        std::fs::write(&mov, vec![0u8; 2_600_000]).unwrap();
        let png = d.join("shot.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let file = |p: &PathBuf| Outgoing {
            path: p.clone(),
            image: false,
        };
        let pic = |p: &PathBuf| Outgoing {
            path: p.clone(),
            image: true,
        };
        assert_eq!(refusal("T-Book", true, &[file(&mov), pic(&png)]), None);
        assert_eq!(
            refusal("T-Book", false, &[file(&mov)]).as_deref(),
            Some(&*format!(
                "T-Book needs Trek 0.4.61 or later to receive files. Update Trek there, or start the thread on {}.",
                crate::words::words().this_computer
            ))
        );
        // Pictures alone still go to an older Trek, inline as they always did.
        assert_eq!(refusal("T-Book", false, &[pic(&png)]), None);
        // A folder goes (packed as it's sent); to a Trek that takes no files it's said as a
        // file is.
        assert_eq!(refusal("T-Book", true, &[file(&d)]), None);
        assert!(
            refusal("T-Book", false, &[file(&d)])
                .unwrap()
                .starts_with("T-Book needs Trek 0.4.61 or later to receive files.")
        );
        let gone = refusal("T-Book", true, &[file(&d.join("gone.mov"))]).unwrap();
        assert!(gone.starts_with("gone.mov isn't there any more"), "{gone}");
        // Over 2 GB: a sparse file, so the disk isn't asked for it.
        let big = d.join("big.mov");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(tr::MAX_UPLOAD + 1).unwrap();
        assert_eq!(
            refusal("T-Book", true, &[file(&big)]).as_deref(),
            Some("big.mov is 2.0 GB: files of up to 2 GB can go to T-Book.")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A folder packs into `<folder> folder.zip`: its files under its own name, what its
    /// `.gitignore` leaves out left behind (in a repository or not), `.git` never; one with
    /// nothing to send says so.
    #[test]
    fn a_folder_packs_into_a_zip_of_what_git_would_list() {
        let d = dir("pack");
        let assets = d.join("assets");
        std::fs::create_dir_all(assets.join("icons")).unwrap();
        std::fs::create_dir_all(assets.join("build")).unwrap();
        std::fs::create_dir_all(assets.join(".git")).unwrap();
        std::fs::write(assets.join("readme.txt"), "what these are").unwrap();
        std::fs::write(assets.join("icons/a.svg"), "<svg/>").unwrap();
        std::fs::write(assets.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(assets.join("build/out.bin"), [7u8; 64]).unwrap();
        std::fs::write(assets.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        let zip = pack_folder(&assets, &d.join("out")).unwrap();
        assert_eq!(
            zip.file_name().and_then(|n| n.to_str()),
            Some("assets folder.zip")
        );
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "assets/.gitignore",
                "assets/icons/a.svg",
                "assets/readme.txt"
            ]
        );
        let mut read = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("assets/readme.txt").unwrap(),
            &mut read,
        )
        .unwrap();
        assert_eq!(read, "what these are");
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(pack_folder(&empty, &d.join("out")), Err(PackError::Empty));
        assert_eq!(
            PackError::Empty.words("empty", "T-Book"),
            "empty has no files to send to T-Book (what git ignores stays behind)."
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The message names each sent file by the machine's path, in place (a Windows machine's with
    /// its backslashes, quoted for its spaces); a sent picture goes by its id, one not sent inline.
    #[test]
    fn the_message_names_the_copies_there_and_sends_pictures_by_id() {
        let d = dir("shaped");
        let (clip, shot, old) = (d.join("clip.mov"), d.join("shot.png"), d.join("old.jpg"));
        for p in [&clip, &shot, &old] {
            std::fs::write(p, b"x").unwrap();
        }
        let there = r"C:\Users\T\AppData\Roaming\Trek\uploads\Mac 2026-11-20 at 10.00.00 clip.mov";
        let pending = HashMap::from([
            (
                clip.clone(),
                Sent {
                    upload_id: "u-clip".into(),
                    there: there.into(),
                    image: false,
                },
            ),
            (
                shot.clone(),
                Sent {
                    upload_id: "u-shot".into(),
                    there: "/Users/t/Library/Application Support/Trek/uploads/Mac shot.png".into(),
                    image: true,
                },
            ),
        ]);
        let text = crate::composer::with_files("what happens here?", std::slice::from_ref(&clip));
        let (sent, ids, inline) = shaped(&text, &[shot.clone(), old.clone()], &pending);
        assert_eq!(sent.trim_end(), format!("what happens here? @\"{there}\""));
        assert_eq!(ids, ["u-shot"]);
        assert_eq!(inline, [old]);
        // Shown here, it's this computer's file again.
        let copies = vec![SentCopy {
            there: there.into(),
            here: clip.display().to_string(),
        }];
        assert_eq!(shown_here(&sent, &copies).trim_end(), text.trim_end());
        let _ = std::fs::remove_dir_all(&d);
        // Its file gone from here, the machine's path stays.
        assert_eq!(shown_here(&sent, &copies), sent);
    }
}
