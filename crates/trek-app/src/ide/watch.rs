//! FSEvents on the IDE folder: when files change under it (an agent writing, a build, another
//! editor), open files with no unsaved edits show them as they are now, the Explorer reads its
//! folders again, and diff tabs read their changes again. What git ignores is let go, as git
//! decides it: every `.gitignore` from the folder down to the file, `.git/info/exclude` and the
//! user's global excludes, read again when one of them changes. Changes inside `.git` are let go
//! too, except the index, HEAD and refs moving (a commit or checkout made elsewhere), which read
//! git's status again. Events are gathered for a moment, so a burst is one refresh, and sorted
//! off the main thread: a build's thousands of paths never reach it.
//!
//! The watcher runs while the window shows the editor; Agents mode drops it. Agents' own edits
//! don't wait for it: each finished tool call reloads clean files too (`Workspace::agent_edits`).
//! Tests run without the watcher, so frames depend on what the test does alone.

use super::IdeWorkbench;
use gpui_kit::*;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::Watcher as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long changes gather before they're acted on.
const GATHER: Duration = Duration::from_millis(150);

pub(super) struct FolderWatch {
    pub root: PathBuf,
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
}

/// What git ignores under a folder: the rules of each `.gitignore` (read as first needed), the
/// repository's `info/exclude` and the user's global excludes.
pub(crate) struct Ignores {
    root: PathBuf,
    /// The repository's own rules (`info/exclude`) and the global ones.
    base: Vec<Gitignore>,
    /// Each folder's `.gitignore`, by folder (`None`: it has none).
    dirs: HashMap<PathBuf, Option<Gitignore>>,
}

impl Ignores {
    pub(crate) fn new(root: &Path) -> Self {
        let mut base = vec![];
        let (global, _) = Gitignore::global();
        base.push(global);
        let top = super::git::top(root).unwrap_or_else(|| root.to_path_buf());
        let exclude = top.join(".git/info/exclude");
        if exclude.is_file() {
            let mut b = GitignoreBuilder::new(&top);
            b.add(exclude);
            base.extend(b.build().ok());
        }
        Self { root: root.to_path_buf(), base, dirs: HashMap::new() }
    }

    /// Forget the rules read, so they're read again (a `.gitignore` changed).
    fn forget(&mut self, dir: Option<&Path>) {
        match dir {
            Some(d) => _ = self.dirs.remove(d),
            None => self.dirs.clear(),
        }
    }

    fn rules(&mut self, dir: &Path) -> Option<&Gitignore> {
        self.dirs
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let file = dir.join(".gitignore");
                file.is_file().then(|| {
                    let mut b = GitignoreBuilder::new(dir);
                    b.add(file);
                    b.build().ok()
                })?
            })
            .as_ref()
    }

    /// Whether git ignores `path` (under the root): the nearest `.gitignore` with a say decides,
    /// a folder ignored ignoring what's in it.
    pub(crate) fn ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        // Each folder from the root down to the path, and the path itself.
        let mut at = self.root.clone();
        let parts: Vec<_> = rel.components().collect();
        for (i, part) in parts.iter().enumerate() {
            let here = at.join(part);
            let dir = i + 1 < parts.len() || is_dir;
            if self.decide(&at, &here, dir) {
                return true;
            }
            at = here;
        }
        false
    }

    /// `path` (directly under or below the folders up to `dir`) is ignored by the rules in force
    /// at `dir`: its own `.gitignore` and its parents', then the repository's and global ones.
    fn decide(&mut self, dir: &Path, path: &Path, is_dir: bool) -> bool {
        let mut d = Some(dir.to_path_buf());
        while let Some(cur) = d {
            if let Some(g) = self.rules(&cur) {
                let m = g.matched(path, is_dir);
                if m.is_ignore() {
                    return true;
                }
                if m.is_whitelist() {
                    return false;
                }
            }
            if cur == self.root {
                break;
            }
            d = cur.parent().map(Path::to_path_buf);
        }
        // `matched`, not `matched_path_or_any_parents`: that one panics on a path outside the
        // matcher's root (the global excludes are rooted at the process's folder, links
        // resolved). Folders are walked one by one above, so parents are decided already.
        self.base.iter().any(|g| g.matched(path, is_dir).is_ignore())
    }
}

/// A batch of changed paths, sorted: the files that matter (not in `.git`, not ignored), and
/// whether git's own state moved.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Batch {
    pub files: Vec<PathBuf>,
    pub git: bool,
}

/// Sort `paths` (as FSEvents names them, under `real`) into a `Batch` of paths under `given`.
pub(crate) fn sort(paths: Vec<PathBuf>, real: &Path, given: &Path, ignores: &mut Ignores) -> Batch {
    let mut batch = Batch::default();
    let mut paths: Vec<PathBuf> = paths.into_iter().filter_map(|p| p.strip_prefix(real).ok().or_else(|| p.strip_prefix(given).ok()).map(|r| given.join(r))).collect();
    paths.sort();
    paths.dedup();
    // Rules first: a `.gitignore` that changed decides about the rest of the batch.
    for p in &paths {
        if p.file_name().is_some_and(|n| n == ".gitignore") {
            ignores.forget(p.parent());
        }
        if p.ends_with(".git/info/exclude") {
            *ignores = Ignores::new(given);
        }
    }
    for p in paths {
        let rel = p.strip_prefix(given).unwrap_or(&p);
        if let Some(inside) = rel.components().position(|c| c.as_os_str() == ".git") {
            let under: PathBuf = rel.components().skip(inside + 1).collect();
            if under == Path::new("index") || under == Path::new("HEAD") || under.starts_with("refs") {
                batch.git = true;
            }
            continue;
        }
        if !ignores.ignored(&p, p.is_dir()) {
            batch.files.push(p);
        }
    }
    batch
}

impl IdeWorkbench {
    /// Watch the IDE folder (again, when it moved) while the editor is on screen; none without
    /// one, or in Agents mode.
    pub(super) fn watch_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let ws = self.workspace.read(cx);
        let root = ws.ide_root.clone().filter(|_| ws.ide());
        if self.watch.as_ref().map(|w| &w.root) == root.as_ref() {
            return;
        }
        self.watch = None;
        let Some(root) = root else { return };
        let (tx, rx) = async_channel::unbounded::<Vec<PathBuf>>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(e) = event {
                let _ = tx.try_send(e.paths);
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => return tracing::warn!("watch {}: {e}", root.display()),
        };
        if let Err(e) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            return tracing::warn!("watch {}: {e}", root.display());
        }
        // FSEvents names files by their real path (`/private/var/…` for `/var/…`).
        let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let given = root.clone();
        let ignores = Arc::new(Mutex::new(None::<Ignores>));
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut paths = first;
                cx.background_executor().timer(GATHER).await;
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                let (real, given, ignores) = (real.clone(), given.clone(), ignores.clone());
                let batch = cx
                    .background_executor()
                    .spawn(async move {
                        let mut guard = ignores.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        // A panic on the dispatch queue would abort the app: a batch that can't be
                        // sorted is let through whole instead (rules read again next time).
                        let all = paths.clone();
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let ignores = guard.get_or_insert_with(|| Ignores::new(&given));
                            sort(paths, &real, &given, ignores)
                        }))
                        .unwrap_or_else(|_| {
                            *guard = None;
                            Batch { files: all.into_iter().filter_map(|p| p.strip_prefix(&real).ok().map(|r| given.join(r))).collect(), git: true }
                        })
                    })
                    .await;
                if batch.files.is_empty() && !batch.git {
                    continue;
                }
                if this.update_in(cx, |this, window, cx| this.files_changed(&batch, window, cx)).is_err() {
                    break;
                }
            }
        });
        self.watch = Some(FolderWatch { root, _watcher: watcher, _task: task });
    }

    /// Files changed on disk: their clean editors read them again, the Explorer its folders,
    /// diff tabs their changes; git's status is read again when it moved.
    fn files_changed(&mut self, batch: &Batch, window: &mut Window, cx: &mut Context<Self>) {
        for e in self.editors() {
            let path = e.read(cx).path.clone();
            let canon = self.canonical.entry(path.clone()).or_insert_with(|| std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone())).clone();
            if batch.files.iter().any(|p| *p == path || (p.file_name() == path.file_name() && std::fs::canonicalize(p).ok().as_ref() == Some(&canon))) {
                e.update(cx, |e, cx| e.reload_if_changed(window, cx));
            }
        }
        if !batch.files.is_empty() {
            self.explorer.update(cx, |e, cx| e.refresh(cx));
        }
        self.reload_diffs(cx);
        if let Some(root) = self.workspace.read(cx).ide_root.clone() {
            if batch.git || !batch.files.is_empty() {
                self.workspace.update(cx, |ws, cx| ws.refresh_git_at(root, cx));
            }
        }
        if let Some(scm) = &self.scm {
            scm.update(cx, |s, cx| s.refresh(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Ignores, sort};

    #[test]
    fn nested_gitignores_and_git_state_sort_a_batch() {
        let dir = std::env::temp_dir().join(format!("trek-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("web/node_modules/x")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(dir.join("web/.gitignore"), "node_modules/\n!keep.log\n").unwrap();
        let mut ig = Ignores::new(&dir);
        // Paths outside the matchers' roots (the global one is rooted elsewhere) never panic.
        assert!(!ig.ignored(std::path::Path::new("/elsewhere/a.rs"), false));
        assert!(!ig.decide(&dir, std::path::Path::new("/elsewhere/a.rs"), false));
        let paths = ["web/node_modules/x/a.js", "web/app.js", "debug.log", "web/keep.log", "src/main.rs", ".git/index", ".git/objects/ab/cd"].iter().map(|p| dir.join(p)).collect();
        let batch = sort(paths, &dir, &dir, &mut ig);
        let rel: Vec<String> = batch.files.iter().map(|p| p.strip_prefix(&dir).unwrap().display().to_string()).collect();
        assert_eq!(rel, ["src/main.rs", "web/app.js", "web/keep.log"]);
        assert!(batch.git, "the index moved");
        // A .gitignore that changes counts at once.
        std::fs::write(dir.join("web/.gitignore"), "app.js\n").unwrap();
        let batch = sort(vec![dir.join("web/.gitignore"), dir.join("web/app.js")], &dir, &dir, &mut ig);
        let rel: Vec<String> = batch.files.iter().map(|p| p.strip_prefix(&dir).unwrap().display().to_string()).collect();
        assert_eq!(rel, ["web/.gitignore"]);
        assert!(!batch.git);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
