//! Another Trek driving this one (`docs/MULTI-MACHINE.md`): what this machine is
//! (`machine_info`), a repo cloned here and registered as a project (`clone_project`), and the
//! files git ignores copied into it (`put_files`).

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use trek_remote as tr;

/// How often a line git rewrites in place (`Receiving objects:  42%`, ended by `\r`) goes to the
/// machine that asked, at most: a large clone rewrites it thousands of times.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// This machine's name, platform, Trek's version and home folder. The name is asked of the
/// system (`scutil` on a Mac), off the main thread.
pub(super) fn machine_info(reply: tr::Reply<tr::MachineInfo>, cx: &mut Context<Workspace>) {
    cx.background_executor()
        .spawn(async move {
            let _ = reply.send(Ok(tr::MachineInfo {
                name: super::computer_name(),
                platform: std::env::consts::OS.to_string(),
                trek_version: env!("CARGO_PKG_VERSION").to_string(),
                home: trek_core::paths::home().display().to_string(),
            }));
        })
        .detach();
}

impl Workspace {
    /// `git clone req.url req.folder` on a thread of its own (a clone may take minutes), git's
    /// output going to `progress` as it comes; then the folder registered as a project named
    /// `req.name`, and its id the reply. A folder that exists and isn't empty is refused
    /// (`conflict`) before git runs; a clone that fails is `clone_failed` with what git said.
    pub(super) fn remote_clone_project(&mut self, req: tr::CloneProjectRequest, progress: tr::CloneProgress, reply: tr::Reply<String>, cx: &mut Context<Self>) {
        let Some(folder) = clone_folder(&req.folder) else {
            let _ = reply.send(Err(tr::HostError::bad_request("The folder must be absolute or under ~/")));
            return;
        };
        let (done, cloned) = tokio::sync::oneshot::channel();
        let (url, into) = (req.url, folder.clone());
        let spawned = std::thread::Builder::new().name("trek-clone".into()).spawn(move || {
            let _ = done.send(clone_into(&url, &into, &progress));
        });
        if let Err(e) = spawned {
            let _ = reply.send(Err(tr::HostError::other(format!("Couldn't start the clone: {e}"))));
            return;
        }
        let name = req.name;
        cx.spawn(async move |this, cx| {
            let answer = match cloned.await {
                Ok(Ok(())) => this
                    .update(cx, |ws, cx| ws.register_remote_project(&folder, &name, cx))
                    .unwrap_or_else(|_| Err(tr::HostError::other("Trek closed"))),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(tr::HostError::other("The clone stopped")),
            };
            let _ = reply.send(answer);
        })
        .detach();
    }

    /// `folder` as a project named `name`, among the user's own (as opening the folder makes
    /// it, without taking this Mac's window there); its id. Phones and machines hear of it with
    /// the next snapshot.
    pub(crate) fn register_remote_project(&mut self, folder: &Path, name: &str, cx: &mut Context<Self>) -> tr::HostResult<String> {
        let folder = crate::system::canonical(folder).unwrap_or_else(|| folder.to_path_buf());
        let project = self.store.ensure_project(&folder).map_err(|e| tr::HostError::other(format!("Couldn't add the project: {e}")))?;
        let name = name.trim();
        if !name.is_empty() && name != project.name {
            self.store.rename_project(&project.id, name).map_err(|e| tr::HostError::other(format!("Couldn't name the project: {e}")))?;
        }
        let key = project.path.display().to_string();
        let hidden = self.settings.hidden_projects.contains(&key);
        if hidden || !self.settings.user_projects.contains(&key) {
            self.settings.hidden_projects.retain(|h| *h != key);
            if !self.settings.user_projects.contains(&key) {
                self.settings.user_projects.push(key);
            }
            let _ = self.settings.save();
        }
        self.reload(cx);
        self.push_remote_changes(cx);
        Ok(project.id)
    }

    /// Write `files` under the project's folder, off the main thread: every path is checked
    /// before anything is written (inside the folder, no symbolic link on the way or at the end),
    /// then each file is written whole beside its place and moved there, with its mode.
    pub(super) fn remote_put_files(&mut self, project_id: &str, files: Vec<tr::FileUpload>, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        let Some(root) = self.project(project_id).map(|p| p.path.clone()) else {
            let _ = reply.send(Err(tr::HostError::not_found("No such project")));
            return;
        };
        cx.background_executor()
            .spawn(async move {
                let _ = reply.send(write_files(&root, &files));
            })
            .detach();
    }
}

/// Where a clone goes: `~/…` under the home folder, else the absolute path as given.
fn clone_folder(folder: &str) -> Option<PathBuf> {
    let path = match folder.strip_prefix("~/") {
        Some(rest) => trek_core::paths::home().join(rest),
        None => PathBuf::from(folder),
    };
    path.is_absolute().then_some(path)
}

/// `git clone --progress -- <url> <folder>`, its output (stderr) to `progress` line by line.
pub(crate) fn clone_into(url: &str, folder: &Path, progress: &tr::CloneProgress) -> tr::HostResult<()> {
    match std::fs::symlink_metadata(folder) {
        Ok(meta) if meta.is_dir() => {
            let mut entries = std::fs::read_dir(folder).map_err(|e| tr::HostError::other(format!("Couldn't read {}: {e}", folder.display())))?;
            if entries.next().is_some() {
                return Err(tr::HostError::conflict(format!("{} already exists and isn't empty", folder.display())));
            }
        }
        Ok(_) => return Err(tr::HostError::conflict(format!("{} already exists and isn't a folder", folder.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(tr::HostError::other(format!("Couldn't look at {}: {e}", folder.display()))),
    }
    let mut command = trek_core::procs::command("git");
    command
        .args(["clone", "--progress", "--", url])
        .arg(folder)
        .env("PATH", trek_core::detect::login_path())
        // Nobody is at this machine to type a password: a repo that needs one fails, saying so.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| tr::HostError::clone_failed(format!("Couldn't run git: {e}")))?;
    let mut output = GitOutput::new(progress);
    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => output.feed(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
    output.finish();
    let status = child.wait().map_err(|e| tr::HostError::clone_failed(format!("git: {e}")))?;
    if status.success() { Ok(()) } else { Err(tr::HostError::clone_failed(output.failure().unwrap_or_else(|| format!("git clone failed ({status})")))) }
}

/// git's stderr, cut into lines (`\n`, and the `\r` its progress counters rewrite themselves
/// with) for `clone_progress`, keeping the last few to say why it failed.
struct GitOutput<'a> {
    progress: &'a tr::CloneProgress,
    partial: Vec<u8>,
    /// When a rewritten line last went out.
    sent_at: Option<Instant>,
    /// The last whole lines.
    tail: std::collections::VecDeque<String>,
}

impl<'a> GitOutput<'a> {
    fn new(progress: &'a tr::CloneProgress) -> Self {
        Self { progress, partial: Vec::new(), sent_at: None, tail: Default::default() }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                let line = std::mem::take(&mut self.partial);
                self.line(&line, b == b'\n');
            } else {
                self.partial.push(b);
            }
        }
    }

    fn finish(&mut self) {
        let line = std::mem::take(&mut self.partial);
        self.line(&line, true);
    }

    fn line(&mut self, bytes: &[u8], whole: bool) {
        let text = String::from_utf8_lossy(bytes).trim().to_string();
        if text.is_empty() {
            return;
        }
        if whole {
            self.progress.line(text.clone());
            self.tail.push_back(text);
            if self.tail.len() > 20 {
                self.tail.pop_front();
            }
        } else if self.sent_at.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY) {
            self.sent_at = Some(Instant::now());
            self.progress.line(text);
        }
    }

    /// What git said went wrong: its `fatal:` and `error:` lines, else its last few.
    fn failure(&self) -> Option<String> {
        let said: Vec<&str> = self.tail.iter().map(String::as_str).filter(|l| l.starts_with("fatal:") || l.starts_with("error:")).collect();
        let lines: Vec<&str> = if said.is_empty() { self.tail.iter().rev().take(5).rev().map(String::as_str).collect() } else { said };
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// Every file checked, then every file written; the first that can't be is the error.
pub(crate) fn write_files(root: &Path, files: &[tr::FileUpload]) -> tr::HostResult<()> {
    let root = crate::system::canonical(root).filter(|r| r.is_dir()).ok_or_else(|| tr::HostError::not_found("The project's folder is gone"))?;
    for file in files {
        check_target(&root, &file.path)?;
    }
    for file in files {
        write_file(&root, file).map_err(|e| tr::HostError::other(format!("Couldn't write {}: {e}", file.path)))?;
    }
    Ok(())
}

/// Refuse `rel` when it isn't a path inside the project, or when a folder on its way or the file
/// itself is a symbolic link (or a file where a folder should be, or the other way round).
fn check_target(root: &Path, rel: &str) -> tr::HostResult<()> {
    if !tr::safe_relative_path(rel) {
        return Err(tr::HostError::bad_request(format!("{rel:?} isn't a path inside the project")));
    }
    let parts: Vec<&str> = rel.split(['/', '\\']).collect();
    let mut at = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        at.push(part);
        let last = i + 1 == parts.len();
        let shown = parts[..=i].join("/");
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(tr::HostError::conflict(format!("{shown} is a symbolic link: Trek doesn't write through one")));
            }
            Ok(meta) if !last && !meta.is_dir() => return Err(tr::HostError::conflict(format!("{shown} is a file, not a folder"))),
            Ok(meta) if last && meta.is_dir() => return Err(tr::HostError::conflict(format!("{shown} is a folder"))),
            Ok(_) => {}
            // The rest is made as it's written.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(tr::HostError::other(format!("Couldn't look at {shown}: {e}"))),
        }
    }
    Ok(())
}

/// One file, its folders made one at a time (none of them a symbolic link), written whole to a
/// new file beside it and moved over it: a rename replaces whatever is there, never writing
/// through it.
fn write_file(root: &Path, file: &tr::FileUpload) -> std::io::Result<()> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let parts: Vec<&str> = file.path.split(['/', '\\']).collect();
    let Some((name, folders)) = parts.split_last() else { return Err(std::io::Error::other("no file name")) };
    let mut dir = root.to_path_buf();
    for part in folders {
        dir.push(part);
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = std::fs::symlink_metadata(&dir)?;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(std::io::Error::other(format!("{} isn't a folder of the project's", dir.display())));
                }
            }
            Err(e) => return Err(e),
        }
    }
    let target = dir.join(name);
    let temp = dir.join(format!(".trek-upload-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut out = options.open(&temp)?;
        out.write_all(&file.bytes)?;
        #[cfg(unix)]
        out.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(file.mode))?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}
