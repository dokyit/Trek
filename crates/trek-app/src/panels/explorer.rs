//! Explorer: the project's file tree. A file opens in the in-app editor; a folder expands.
//!
//! Folders are read off the main thread and kept: a frame draws what was read last, and the
//! tree reads again when files changed under it (a turn finished, a checkout, a rewind), when
//! the folder watcher says files came or went in a folder on show (made in a terminal, another
//! editor; in the editor the workbench's own watcher says it, `ide::watch`), or the user asks
//! (Refresh). Files git ignores are left out, as `.gitignore` (and the repository's excludes)
//! say. A row's context menu makes, renames and trashes files. In the editor the tree also shows
//! git status: a letter and a colour per changed file, a dot on folders holding changes; and the
//! menu adds a row to the AI side bar's chat.

use crate::fluent;
use crate::scale::{radius, text, to_rems};
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Never listed, ignored or not.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];

/// Rows drawn at most: a tree opened that far is beyond reading anyway. A last row says so.
const MAX_ROWS: usize = 4000;

/// A line of the tree: a file or folder, or a note in a folder (it's empty, it couldn't be read,
/// the tree stops here), at its depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Entry(PathBuf, bool, usize),
    Note(String, usize),
}

/// Why `dir` can't be listed, in a line (`None`: it can).
fn unreadable(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir).err().map(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "This folder isn't there any more.".to_string(),
        std::io::ErrorKind::PermissionDenied => "Trek isn't allowed to read this folder.".into(),
        _ => format!("Couldn't read this folder: {e}"),
    })
}

/// How long the folder watcher gathers changes before the tree reads again.
const GATHER: std::time::Duration = std::time::Duration::from_millis(150);

/// Whether a batch of paths the folder watcher names (by their real path under `real`, the
/// folder `given` as the tree knows it) changes what the tree shows: an entry come or gone in a
/// folder on show (`shown`). What happens inside `.git`, or in folders shut, doesn't.
pub fn changes_shown(
    paths: &[PathBuf],
    real: &Path,
    given: &Path,
    shown: impl Fn(&Path) -> bool,
) -> bool {
    paths.iter().any(|p| {
        let Some(rel) = p
            .strip_prefix(real)
            .ok()
            .or_else(|| p.strip_prefix(given).ok())
        else {
            return false;
        };
        if rel.as_os_str().is_empty() || rel.components().any(|c| c.as_os_str() == ".git") {
            return false;
        }
        given.join(rel).parent().is_some_and(&shown)
    })
}

/// The Agents-mode tree's watch on its folder.
struct Watch {
    root: PathBuf,
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
}

/// `dir`'s entries, folders first, leaving out what git ignores there (`.gitignore` files up to
/// the repository's top, its excludes and the user's) — outside a repository, everything. With
/// `dots` off, dotfiles go too (a tree rooted at the home folder, which is full of them).
fn children(dir: &Path, dots: bool) -> Vec<(PathBuf, bool)> {
    let walk = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .ignore(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .follow_links(false)
        .build();
    let mut v: Vec<(PathBuf, bool)> = walk
        .flatten()
        .filter(|e| e.depth() == 1)
        .filter(|e| {
            let n = e.file_name().to_string_lossy();
            !HIDDEN.contains(&n.as_ref())
                && !n.ends_with(".nosync")
                && (dots || !n.starts_with('.'))
        })
        .map(|e| {
            // A link to a folder opens like one.
            let is_dir = e.file_type().is_some_and(|t| t.is_dir())
                || (e.path_is_symlink() && e.path().is_dir());
            (e.into_path(), is_dir)
        })
        .collect();
    v.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
    });
    v
}

/// A changed file's state in git, as the tree marks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitMark {
    Modified,
    Added,
    Untracked,
    Deleted,
}

impl GitMark {
    pub fn letter(self) -> &'static str {
        match self {
            GitMark::Modified => "M",
            GitMark::Added => "A",
            GitMark::Untracked => "U",
            GitMark::Deleted => "D",
        }
    }

    pub fn color(self, cx: &App) -> Hsla {
        match self {
            GitMark::Modified => crate::palette::amber(cx),
            GitMark::Added | GitMark::Untracked => crate::palette::emerald(cx),
            GitMark::Deleted => crate::palette::red(cx),
        }
    }
}

/// `git status` for the repository holding `root`: each changed path under `root` (as a path
/// under `root` as given, which may go through a symlink git resolves) and its mark. Not a
/// repository: empty.
pub fn git_marks(root: &Path) -> HashMap<PathBuf, GitMark> {
    let run = |args: &[&str]| {
        trek_core::git::read_only(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };
    // Where `root` sits in the repository: status paths are relative to its top.
    let Some(prefix) = run(&["rev-parse", "--show-prefix"])
        .map(|o| String::from_utf8_lossy(&o).trim().to_string())
    else {
        return HashMap::new();
    };
    let Some(out) = run(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]) else {
        return HashMap::new();
    };
    let mut marks = HashMap::new();
    // Not filtered by length: a rename's old path can be as short as `a.c`, and must be there to
    // be skipped. (An entry itself is at least `XY p`.)
    let mut entries = out.split(|b| *b == 0).filter(|e| !e.is_empty());
    while let Some(e) = entries.next() {
        if e.len() < 4 {
            continue;
        }
        let (x, y) = (e[0] as char, e[1] as char);
        let rel = String::from_utf8_lossy(&e[3..]).to_string();
        // A rename's entry is followed by its old path.
        if x == 'R' || x == 'C' {
            entries.next();
        }
        let Some(under) = rel.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let path = root.join(under);
        let mark = match (x, y) {
            ('?', '?') => GitMark::Untracked,
            ('A', _) => GitMark::Added,
            ('D', _) | (_, 'D') => GitMark::Deleted,
            _ => GitMark::Modified,
        };
        marks.insert(path, mark);
    }
    marks
}

/// What a name typed in the tree makes.
#[derive(Debug, Clone, PartialEq)]
enum EditKind {
    NewFile,
    NewFolder,
    Rename(PathBuf),
}

/// A name being typed in the tree (a new file or folder in `dir`, or a rename), in place.
struct Edit {
    kind: EditKind,
    dir: PathBuf,
    input: Entity<InputState>,
    _sub: Subscription,
}

/// Told when a file or folder moved (`Some(to)`) or went to the Trash (`None`).
pub type OnMoved = Box<dyn Fn(PathBuf, Option<PathBuf>, &mut Window, &mut App)>;

/// `to` is `from` itself under another case: on a case-insensitive volume (macOS's, Windows's)
/// the new spelling "exists" because it is the file being renamed.
fn is_case_change(from: &Path, to: &Path) -> bool {
    let (Some(a), Some(b)) = (from.file_name(), to.file_name()) else {
        return false;
    };
    let (a, b) = (a.to_string_lossy(), b.to_string_lossy());
    if a == b || a.to_lowercase() != b.to_lowercase() {
        return false;
    }
    match (from.symlink_metadata(), to.symlink_metadata()) {
        (Ok(f), Ok(t)) => same_file(&f, &t),
        _ => false,
    }
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// Windows can't tell from the metadata alone; the names already differ only in case.
#[cfg(not(unix))]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// Why `name` can't be made or renamed to in `dir`, when it can't. `renaming` is the path being
/// renamed, whose own name in another case isn't a clash.
fn name_problem(dir: &Path, name: &str, nested: bool, renaming: Option<&Path>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("A name is needed.".into());
    }
    if invalid_name(name, nested, cfg!(windows)) {
        return Some(format!("“{name}” isn't a valid name here."));
    }
    let to = dir.join(name);
    if renaming.is_some_and(|from| is_case_change(from, &to)) {
        return None;
    }
    to.symlink_metadata()
        .is_ok()
        .then(|| format!("A file or folder “{name}” already exists here. Choose another name."))
}

/// `name` (for a new item, or a rename) can't be one inside the folder: it climbs out (`..`) or
/// starts elsewhere (`/`), or is nested where it can't be (`nested`: `a/b.md` makes the folder).
/// On Windows (`windows`, a value so either platform's tests check both) its own rules too: `\`
/// separates as `/` does, a drive (`C:`) or a share (`\\server`) is elsewhere, a `:` names a
/// hidden stream of the file before it, `<>"|?*` and control characters are refused, and so are
/// names Windows keeps for devices (`CON`, `NUL`, `COM1`…) and ones ending in a dot or a space.
fn invalid_name(name: &str, nested: bool, windows: bool) -> bool {
    let separators: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let parts: Vec<&str> = name.split(separators).collect();
    if parts.iter().any(|p| *p == "." || *p == "..")
        || name.starts_with(separators)
        || (!nested && parts.len() > 1)
    {
        return true;
    }
    windows
        && parts.iter().any(|p| {
            p.is_empty()
                || p.ends_with(['.', ' '])
                || p.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
                || kept_for_devices(p)
        })
}

/// A name Windows keeps for a device, with or without an extension: `nul`, `CON.txt`, `com1`
/// (`COM0` to `COM9` and `LPT0` to `LPT9`, as its naming rules list them).
fn kept_for_devices(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

pub struct ExplorerPanel {
    workspace: Entity<Workspace>,
    edit: Option<Edit>,
    on_moved: Option<OnMoved>,
    root: Option<PathBuf>,
    empty_hint: &'static str,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    /// `turns_finished` + `files_epoch` as last read: agents and checkouts add and remove files.
    files_seen: u64,
    /// Each folder's entries as last read, for the root and the folders open under it.
    entries: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    /// The folders shown that couldn't be read, and why.
    unreadable: HashMap<PathBuf, String>,
    /// The editor's tree: compact rows, git marks, a single click opens a preview tab.
    ide: bool,
    git: HashMap<PathBuf, GitMark>,
    /// Folders holding changed files (each ancestor of one, up to the root).
    git_dirs: HashSet<PathBuf>,
    /// Reading folders in the background; a newer read replaces it.
    _read: Option<Task<()>>,
    /// Agents mode's watch on the folder (Editor mode's workbench watches its own).
    watch: Option<Watch>,
    _subscription: Subscription,
}

impl ExplorerPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(
            workspace,
            |ws| ws.current_cwd(),
            "Open a project to browse its files.",
            false,
            cx,
        )
    }

    /// The IDE's tree: roots at `ide_root` instead of the route's folder, so it stays put while
    /// the AI side bar moves between chats.
    pub fn for_ide(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(
            workspace,
            |ws| ws.ide_root.clone(),
            "Open a folder, or a file, to start.",
            true,
            cx,
        )
    }

    fn from(
        workspace: Entity<Workspace>,
        root: impl Fn(&Workspace) -> Option<PathBuf> + 'static,
        empty_hint: &'static str,
        ide: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = root(&workspace.read(cx));
        let files_seen = Self::files_now(&workspace.read(cx));
        let sub = cx.observe(&workspace, move |this, ws, cx| {
            let root = root(&ws.read(cx));
            let files = Self::files_now(&ws.read(cx));
            if root != this.root {
                this.root = root;
                this.expanded.clear();
                this.entries.clear();
                this.git.clear();
                this.git_dirs.clear();
                this.selected = None;
                this.files_seen = files;
                this.refresh(cx);
                this.watch(cx);
            } else if files != this.files_seen {
                this.files_seen = files;
                this.refresh(cx);
            }
        });
        let mut this = Self {
            workspace,
            edit: None,
            on_moved: None,
            root: initial,
            empty_hint,
            expanded: HashSet::new(),
            selected: None,
            files_seen,
            entries: HashMap::new(),
            unreadable: HashMap::new(),
            ide,
            git: HashMap::new(),
            git_dirs: HashSet::new(),
            _read: None,
            watch: None,
            _subscription: sub,
        };
        this.refresh(cx);
        this.watch(cx);
        this
    }

    /// Watch the folder (again, when it moved), so files made or removed there outside Trek's
    /// agents (in a terminal, another editor) show without waiting for a turn. Not in the editor,
    /// whose workbench watches its folder itself, nor in tests, whose frames depend on what the
    /// test does alone.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.ide || cfg!(any(test, feature = "test-support")) {
            return;
        }
        if self.watch.as_ref().map(|w| &w.root) == self.root.as_ref() {
            return;
        }
        self.watch = None;
        let Some(root) = self.root.clone() else {
            return;
        };
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
        if let Err(e) =
            notify::Watcher::watch(&mut watcher, &root, notify::RecursiveMode::Recursive)
        {
            return tracing::warn!("watch {}: {e}", root.display());
        }
        // FSEvents names files by their real path (`/private/var/…` for `/var/…`).
        let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let given = root.clone();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut paths = first;
                cx.background_executor().timer(GATHER).await;
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                let gone = this
                    .update(cx, |this, cx| {
                        let entries = &this.entries;
                        if changes_shown(&paths, &real, &given, |d| entries.contains_key(d)) {
                            this.refresh(cx);
                        }
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        });
        self.watch = Some(Watch {
            root,
            _watcher: watcher,
            _task: task,
        });
    }

    fn files_now(ws: &Workspace) -> u64 {
        ws.turns_finished + ws.files_epoch
    }

    /// Read the root and every open folder again (and git status, in the editor).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let dirs: Vec<PathBuf> = std::iter::once(root.clone())
            .chain(
                self.expanded
                    .iter()
                    .filter(|d| d.starts_with(&root))
                    .cloned(),
            )
            .collect();
        self.read(dirs, true, cx);
    }

    /// Fold every folder back up.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        cx.notify();
    }

    /// Point the tree at `path` (the open editor's file): its folders open, its row selected.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }
        let missing: Vec<PathBuf> = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&root) && *a != root)
            .map(Path::to_path_buf)
            .filter(|a| self.expanded.insert(a.clone()))
            .collect();
        self.selected = Some(path.to_path_buf());
        if !missing.is_empty() {
            self.read(missing, false, cx);
        }
        cx.notify();
    }

    /// Read `dirs` off the main thread into the cache; `whole`: they're everything shown, so
    /// what was cached for other folders goes (and git status is read again).
    fn read(&mut self, dirs: Vec<PathBuf>, whole: bool, cx: &mut Context<Self>) {
        let git_root = (whole && self.ide).then(|| self.root.clone()).flatten();
        // Rooted at the home folder, its dotfiles (`.Trash`, `.cargo`, …) would bury the rest.
        let dots = self.root.as_deref() != Some(trek_core::paths::home().as_path());
        let task = cx.spawn(async move |this, cx| {
            let (read, git) = cx
                .background_executor()
                .spawn(async move {
                    let read: Vec<(PathBuf, Vec<(PathBuf, bool)>, Option<String>)> = dirs
                        .into_iter()
                        .map(|d| {
                            let why = unreadable(&d);
                            let c = children(&d, dots);
                            (d, c, why)
                        })
                        .collect();
                    (read, git_root.map(|r| git_marks(&r)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if whole {
                    // What's shown now: the root and the folders open (one opened since this read
                    // started keeps what its own read brought).
                    let root = this.root.clone();
                    let expanded = &this.expanded;
                    this.entries
                        .retain(|d, _| root.as_ref() == Some(d) || expanded.contains(d));
                }
                for (dir, entries, why) in read {
                    match why {
                        Some(why) => this.unreadable.insert(dir.clone(), why),
                        None => this.unreadable.remove(&dir),
                    };
                    this.entries.insert(dir, entries);
                }
                if let Some(git) = git {
                    let root = this.root.clone().unwrap_or_default();
                    this.git_dirs = git
                        .keys()
                        .flat_map(|p| {
                            p.ancestors()
                                .skip(1)
                                .take_while(|a| a.starts_with(&root))
                                .map(Path::to_path_buf)
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    this.git = git;
                }
                cx.notify();
            });
        });
        // A part read (a folder opened) doesn't call off a whole one under way.
        if whole || self._read.is_none() {
            self._read = Some(task);
        } else {
            task.detach();
        }
    }

    /// The git mark of `path` (the editor's tabs show it too).
    pub fn mark(&self, path: &Path) -> Option<GitMark> {
        self.git.get(path).copied()
    }

    /// The tree's lines under `root`: what's in it and in each folder open, a note in a folder
    /// that's empty or couldn't be read, and a last one where the tree stops at `MAX_ROWS`.
    pub fn rows(&self, root: &Path) -> Vec<Row> {
        let mut out = vec![];
        if !self.rows_in(root, 0, &mut out) {
            out.push(Row::Note(
                format!("Showing the first {MAX_ROWS} items. Close a folder to see the rest."),
                0,
            ));
        }
        out
    }

    /// `rows` from `dir` down; `false` once the cap is reached.
    fn rows_in(&self, dir: &Path, depth: usize, out: &mut Vec<Row>) -> bool {
        if let Some(why) = self.unreadable.get(dir) {
            out.push(Row::Note(why.clone(), depth));
            return true;
        }
        let Some(entries) = self.entries.get(dir) else {
            return true;
        };
        if entries.is_empty() {
            let empty = if depth == 0 {
                "This folder is empty."
            } else {
                "Empty"
            };
            out.push(Row::Note(empty.into(), depth));
        }
        for (path, is_dir) in entries {
            if out.len() >= MAX_ROWS {
                return false;
            }
            // Open once its entries are in: never an open folder with nothing under it.
            let open = *is_dir && self.expanded.contains(path) && self.entries.contains_key(path);
            out.push(Row::Entry(path.clone(), *is_dir, depth));
            if open && !self.rows_in(path, depth + 1, out) {
                return false;
            }
        }
        true
    }

    /// A note's line in the tree, indented to its folder's entries.
    fn note_row(&self, text: String, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let step = if self.ide { 12. } else { 14. };
        div()
            .id(SharedString::from(format!("explorer-note-{depth}-{text}")))
            .test_support()
            .when(!self.ide, |el| el.mx_1().text_size(to_rems(text::LABEL)))
            .when(self.ide, |el| el.text_size(px(text::BODY)))
            .py(px(4.))
            .pl(px(8. + depth as f32 * step + 18.))
            .pr_2()
            .text_color(theme.muted_foreground)
            .child(text)
            .into_any_element()
    }

    /// Who's told when a file or folder moves or goes (the editor, for its tabs).
    pub fn on_moved(&mut self, f: OnMoved) {
        self.on_moved = Some(f);
    }

    /// The folder new files go in from the header's buttons: the selected folder, the selected
    /// file's, or the root.
    pub fn target_dir(&self) -> Option<PathBuf> {
        let root = self.root.clone()?;
        let dir = match &self.selected {
            Some(p) if p.is_dir() => p.clone(),
            Some(p) => p.parent().map(Path::to_path_buf).unwrap_or(root.clone()),
            None => root.clone(),
        };
        Some(if dir.starts_with(&root) { dir } else { root })
    }

    /// Start typing a new file's (or folder's) name in `dir`, its row in the tree.
    pub fn begin_new(
        &mut self,
        dir: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.root.as_ref().is_some_and(|r| dir != *r) && self.expanded.insert(dir.clone()) {
            self.read(vec![dir.clone()], false, cx);
        }
        self.begin(
            if folder {
                EditKind::NewFolder
            } else {
                EditKind::NewFile
            },
            dir,
            "",
            window,
            cx,
        );
    }

    /// Start renaming `path`, its name in an input in its row (the stem picked).
    pub fn begin_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.begin(EditKind::Rename(path), dir, &name, window, cx);
    }

    fn begin(
        &mut self,
        kind: EditKind,
        dir: PathBuf,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            EditKind::NewFile => "File name",
            EditKind::NewFolder => "Folder name",
            EditKind::Rename(_) => "New name",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let stem = match value.rfind('.') {
            Some(at) if at > 0 => at,
            _ => value.len(),
        };
        input.update(cx, |s, cx| {
            s.set_value(value.to_string(), window, cx);
            s.set_selected_range(0..stem, cx);
            s.focus(window, cx);
        });
        let sub =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
                    InputEvent::Blur => this.cancel_edit(cx),
                    _ => {}
                },
            );
        self.edit = Some(Edit {
            kind,
            dir,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.edit.take().is_some() {
            cx.notify();
        }
    }

    fn toast(&self, message: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        });
    }

    /// Something that went wrong, as a toast with the error face.
    fn failure(&self, message: String, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |_, cx| cx.emit(WorkspaceEvent::Failure { message }));
    }

    /// The name typed goes: the file or folder is made (a new file opens), or renamed. A name
    /// that won't do keeps the input up, saying why.
    pub fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else { return };
        let name = edit.input.read(cx).value().trim().to_string();
        let (kind, dir) = (edit.kind.clone(), edit.dir.clone());
        if let EditKind::Rename(from) = &kind {
            if from
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == name)
            {
                return self.cancel_edit(cx);
            }
        }
        if let Some(why) = name_problem(
            &dir,
            &name,
            !matches!(kind, EditKind::Rename(_)),
            if let EditKind::Rename(from) = &kind {
                Some(from)
            } else {
                None
            },
        ) {
            return self.toast(why, cx);
        }
        let to = dir.join(&name);
        let made = match &kind {
            EditKind::NewFile => to
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&to)
                        .map(|_| ())
                }),
            EditKind::NewFolder => std::fs::create_dir_all(&to),
            EditKind::Rename(from) => std::fs::rename(from, &to),
        };
        if let Err(e) = made {
            return self.failure(
                format!(
                    "Couldn't {} “{name}”: {e}",
                    if matches!(kind, EditKind::Rename(_)) {
                        "rename to"
                    } else {
                        "make"
                    }
                ),
                cx,
            );
        }
        self.edit = None;
        self.selected = Some(to.clone());
        match kind {
            EditKind::NewFile => self
                .workspace
                .update(cx, |ws, cx| ws.open_editor(to.clone(), None, cx)),
            EditKind::NewFolder => _ = self.expanded.insert(to.clone()),
            EditKind::Rename(from) => {
                // Folders open under the old name stay open under the new one.
                let moved: Vec<PathBuf> = self
                    .expanded
                    .iter()
                    .filter(|d| d.starts_with(&from))
                    .cloned()
                    .collect();
                for d in moved {
                    self.expanded.remove(&d);
                    if let Ok(rest) = d.strip_prefix(&from) {
                        self.expanded.insert(to.join(rest));
                    }
                }
                if let Some(f) = &self.on_moved {
                    f(from, Some(to.clone()), window, cx);
                }
            }
        }
        self.refresh(cx);
        cx.notify();
    }

    /// Move `path` to the Trash, after asking. Its clean editor tabs close.
    pub fn delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let what = if path.is_dir() { "folder" } else { "file" };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Move “{name}” to the Trash?"),
            Some(&format!("The {what} can be put back from the Trash.")),
            &["Move to Trash", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                match crate::system::trash(&path) {
                    Ok(()) => {
                        if let Some(f) = &this.on_moved {
                            f(path.clone(), None, window, cx);
                        }
                        if this.selected.as_ref().is_some_and(|s| s.starts_with(&path)) {
                            this.selected = None;
                        }
                    }
                    Err(e) => {
                        this.failure(format!("Couldn't move “{name}” to the Trash: {e:#}"), cx)
                    }
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// The input row of a name being typed, at `depth`.
    fn edit_row(&self, depth: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.edit.as_ref()?;
        let folder = edit.kind == EditKind::NewFolder
            || matches!(&edit.kind, EditKind::Rename(p) if p.is_dir());
        let icon = if folder {
            crate::file_icon::folder("", false, px(14.), cx)
        } else {
            crate::file_icon::badge(&edit.input.read(cx).value().to_string(), px(14.), cx)
        };
        Some(
            h_flex()
                .id("explorer-edit")
                .test_support()
                .h(px(24.))
                .pl(px(8. + depth as f32 * 12. + 18.))
                .pr_2()
                .gap(px(6.))
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&edit.input).xsmall()),
                )
                .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                    cx.stop_propagation();
                    this.cancel_edit(cx);
                }))
                .into_any_element(),
        )
    }

    fn toggle_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&dir) {
            self.expanded.insert(dir.clone());
            // Read each time it opens: files come and go while it's shut.
            self.read(vec![dir], false, cx);
        }
        cx.notify();
    }
}

/// A row's context menu: New File and New Folder (in the folder, or beside the file), Rename and
/// Delete, then Finder and the clipboard. In the editor (`ide`) it starts with Add to Chat, for
/// the AI side bar's chat.
fn row_menu(
    menu: PopupMenu,
    me: WeakEntity<ExplorerPanel>,
    ws: &Entity<Workspace>,
    path: &Path,
    is_dir: bool,
    root: &Path,
    ide: bool,
) -> PopupMenu {
    let copy = |label: &'static str, text: String| {
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
        })
    };
    let relative = path
        .strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string());
    let (ws, chip, reveal) = (ws.clone(), path.to_path_buf(), path.to_path_buf());
    let dir = if is_dir {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf())
    };
    let act =
        |label: &'static str,
         f: Box<dyn Fn(&mut ExplorerPanel, &mut Window, &mut Context<ExplorerPanel>)>| {
            let me = me.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = me.update(cx, |this, cx| f(this, window, cx));
            })
        };
    let (d1, d2, rename, delete) = (dir.clone(), dir, path.to_path_buf(), path.to_path_buf());
    let at_root = path == root;
    let menu = menu.min_w(px(200.));
    let menu = if ide {
        menu.item(PopupMenuItem::new("Add to Chat").on_click(move |_, _, cx| {
            let chip = crate::ide::ai::context::ContextChip::File { path: chip.clone() };
            ws.update(cx, |_, cx| {
                cx.emit(crate::workspace::WorkspaceEvent::AddToChat(chip))
            });
        }))
        .separator()
    } else {
        menu
    };
    let menu = menu
        .item(act(
            "New File…",
            Box::new(move |this, window, cx| this.begin_new(d1.clone(), false, window, cx)),
        ))
        .item(act(
            "New Folder…",
            Box::new(move |this, window, cx| this.begin_new(d2.clone(), true, window, cx)),
        ))
        .separator();
    let menu = if at_root {
        menu
    } else {
        menu.item(act(
            "Rename…",
            Box::new(move |this, window, cx| this.begin_rename(rename.clone(), window, cx)),
        ))
        .item(act(
            "Delete",
            Box::new(move |this, window, cx| this.delete(delete.clone(), window, cx)),
        ))
        .separator()
    };
    menu.item(
        PopupMenuItem::new(crate::words::words().reveal_in_file_manager)
            .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
    )
    .item(copy("Copy Path", path.display().to_string()))
    .item(copy("Copy Relative Path", relative))
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(root) = self.root.clone() else {
            return super::empty(self.empty_hint, cx).into_any_element();
        };
        let rows = self.rows(&root);
        let selected = self.selected.clone();
        let ide = self.ide;
        // A name being typed: a new entry's row first in its folder, a rename in place.
        let (new_in, renaming) = match self.edit.as_ref().map(|e| (&e.kind, e.dir.clone())) {
            Some((EditKind::Rename(p), _)) => (None, Some(p.clone())),
            Some((_, dir)) => (Some(dir), None),
            None => (None, None),
        };
        let mut lead = vec![];
        if new_in.as_ref() == Some(&root) {
            lead.extend(self.edit_row(0, cx));
        }
        let mut elements: Vec<AnyElement> = vec![];
        for row in rows {
            let (path, is_dir, depth) = match row {
                Row::Entry(path, is_dir, depth) => (path, is_dir, depth),
                Row::Note(text, depth) => {
                    // A new entry being named in an empty folder stands for its note.
                    if new_in.is_none() || depth > 0 {
                        elements.push(self.note_row(text, depth, cx));
                    }
                    continue;
                }
            };
            if renaming.as_ref() == Some(&path) {
                elements.extend(self.edit_row(depth, cx));
                continue;
            }
            let opens_here = new_in.as_ref() == Some(&path);
            elements.push(self.tree_row(path, is_dir, depth, &root, &selected, ide, cx));
            if opens_here {
                elements.extend(self.edit_row(depth + 1, cx));
            }
        }
        let tree = v_flex()
            .id("explorer-tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1()
            .children(lead)
            .children(elements);

        // In the editor the side bar's section header has these (`ide::activity`).
        let header = (!ide).then(|| self.header(&root, cx));
        v_flex()
            .size_full()
            .children(header)
            .child(tree)
            .into_any_element()
    }
}

impl ExplorerPanel {
    /// The tools panel's header: the folder's name, then New File, New Folder, Refresh and
    /// Collapse folders.
    fn header(&self, root: &Path, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.display().to_string());
        h_flex()
            .flex_none()
            .pl_3()
            .pr_1()
            .h(px(36.))
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .text_size(to_rems(text::LABEL))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(name),
            )
            .child(
                crate::ui::icon_button(
                    "explorer-new-file",
                    crate::assets::Lucide::FilePlus,
                    "New File…",
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(dir) = this.target_dir() {
                        this.begin_new(dir, false, window, cx);
                    }
                })),
            )
            .child(
                crate::ui::icon_button(
                    "explorer-new-folder",
                    crate::assets::Lucide::FolderPlus,
                    "New Folder…",
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(dir) = this.target_dir() {
                        this.begin_new(dir, true, window, cx);
                    }
                })),
            )
            .child(
                crate::ui::icon_button("explorer-refresh", IconName::RefreshCw, "Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                crate::ui::icon_button(
                    "explorer-collapse",
                    crate::assets::Lucide::ChevronsDownUp,
                    "Collapse folders",
                )
                .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
            )
    }

    /// One row of the tree.
    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &self,
        path: PathBuf,
        is_dir: bool,
        depth: usize,
        root: &Path,
        selected: &Option<PathBuf>,
        ide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let open = self.expanded.contains(&path) && self.entries.contains_key(&path);
            let is_sel = selected.as_ref() == Some(&path);
            let mark = self.git.get(&path).copied();
            let changed_dir = is_dir && self.git_dirs.contains(&path);
            let p = path.clone();
            let row = h_flex()
                .id(SharedString::from(path.display().to_string()))
                .test_support()
                .relative()
                .when(!ide, |el| {
                    el.mx_1()
                        .h(px(26.))
                        .rounded(px(fluent::control(radius::CONTROL)))
                        .text_size(to_rems(text::LABEL))
                })
                .when(ide, |el| el.h(px(22.)).text_size(px(text::BODY)))
                .pl(px(8. + depth as f32 * if ide { 12. } else { 14. }))
                .pr_2()
                .gap(px(6.))
                .cursor_pointer()
                .when(is_sel, |el| el.bg(theme.list_active))
                .when(!is_sel, |el| el.hover(|s| s.bg(theme.list_hover)))
                .children(fluent::selection_bar(is_sel, cx))
                .child(if is_dir {
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .text_color(theme.muted_foreground)
                    .into_any_element()
                } else {
                    div().w(px(12.)).into_any_element()
                })
                .child(if is_dir {
                    crate::file_icon::folder(&name, open, px(14.), cx)
                } else {
                    crate::file_icon::badge(&name, px(14.), cx)
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when_some(mark, |el, m| el.text_color(m.color(cx)))
                        .child(name),
                )
                .when_some(mark, |el, m| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(to_rems(text::META))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(m.color(cx))
                            .child(m.letter()),
                    )
                })
                .when(changed_dir, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .size(px(5.))
                            .rounded_full()
                            .bg(crate::palette::amber(cx).opacity(0.8)),
                    )
                })
                .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    if is_dir {
                        this.toggle_dir(p.clone(), cx);
                    } else {
                        // A file opens in the in-app editor; it highlights here while open. In
                        // the editor a single click previews it, a double click keeps it.
                        this.selected = Some(p.clone());
                        let preview = this.ide && e.click_count() < 2;
                        this.workspace.update(cx, |ws, cx| {
                            if preview {
                                ws.preview_editor(p.clone(), cx)
                            } else {
                                ws.open_editor(p.clone(), None, cx)
                            }
                        });
                        cx.notify();
                    }
                }));
            let (ws, root, me) = (self.workspace.clone(), root.to_path_buf(), cx.weak_entity());
            row.context_menu(move |menu, _, _| {
                row_menu(menu, me.clone(), &ws, &path, is_dir, &root, ide)
            })
            .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GitMark, changes_shown, git_marks, invalid_name, name_problem};
    use std::path::{Path, PathBuf};

    /// A new item or a rename stays in its folder under either platform's rules, and the Windows
    /// ones are checked on the Mac's CI too: `..\x` and `C:\x` would land outside the folder,
    /// `todo:v2.md` would write a hidden stream of `todo`, and `nul` is a device.
    #[test]
    fn names_stay_in_the_folder_by_each_platforms_rules() {
        for windows in [false, true] {
            assert!(invalid_name("../x", true, windows));
            assert!(invalid_name("/x", true, windows));
            assert!(
                invalid_name("a/b", false, windows),
                "nested only where allowed"
            );
            assert!(!invalid_name("a/b.md", true, windows));
            assert!(!invalid_name("notes.md", false, windows));
        }
        for bad in [
            r"..\x",
            r"C:\x",
            "c:x.md",
            r"\\server\share",
            r"src\..\..\x",
            "todo:v2.md",
            "nul",
            "CON.txt",
            "com1",
            "lpt9.log",
            "name.",
            "name ",
            "a<b",
            "a|b",
            "a?b",
            "a*b",
            "a\"b",
            "tab\there",
            r"a\\b",
            "com0",
            "LPT0.txt",
        ] {
            assert!(invalid_name(bad, true, true), "{bad:?} on Windows");
        }
        for fine in [
            "con-notes.md",
            "console.md",
            "com10.txt",
            r"src\main.rs",
            "a.b.c",
        ] {
            assert!(!invalid_name(fine, true, true), "{fine:?} on Windows");
        }
        assert!(
            invalid_name(r"src\main.rs", false, true),
            "a folder, where nesting isn't allowed"
        );
        // What only Windows refuses is a valid name on a Mac.
        for fine in ["todo:v2.md", "name.", "nul", r"back\slash"] {
            assert!(!invalid_name(fine, true, false), "{fine:?} on a Mac");
        }
    }

    #[test]
    fn a_rename_that_only_changes_case_is_not_a_clash_with_itself() {
        let dir =
            std::env::temp_dir().join(format!("trek-explorer-case-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("readme.md");
        std::fs::write(&from, "x").unwrap();
        std::fs::write(dir.join("other.md"), "y").unwrap();
        let case_insensitive = dir.join("README.md").symlink_metadata().is_ok();
        // Whether or not the volume folds case, renaming to another case of its own name is fine.
        assert_eq!(name_problem(&dir, "README.md", false, Some(&from)), None);
        // A different file's name still clashes, and so does a new file's.
        assert!(name_problem(&dir, "other.md", false, Some(&from)).is_some());
        assert!(name_problem(&dir, "readme.md", true, None).is_some());
        if case_insensitive {
            assert!(
                name_problem(&dir, "README.md", true, None).is_some(),
                "a new file can't take the spelling of an existing one"
            );
            std::fs::rename(&from, dir.join("README.md")).unwrap();
            assert!(
                std::fs::read_dir(&dir)
                    .unwrap()
                    .any(|e| e.unwrap().file_name() == "README.md")
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_from_a_short_path_does_not_hide_the_next_changed_file() {
        let dir =
            std::env::temp_dir().join(format!("trek-explorer-marks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run =
            |args: &[&str]| assert!(crate::panels::git::git(&dir, args).is_ok(), "git {args:?}");
        run(&["init", "-q"]);
        std::fs::write(dir.join("a.c"), "x\n").unwrap();
        std::fs::write(dir.join("zzzz.txt"), "y\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "first",
        ]);
        run(&["mv", "a.c", "b.c"]);
        std::fs::write(dir.join("zzzz.txt"), "y\nz\n").unwrap();
        let marks = git_marks(&dir);
        let modified = marks
            .iter()
            .any(|(p, m)| p.ends_with("zzzz.txt") && *m == GitMark::Modified);
        assert!(modified, "{marks:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The watcher's paths, by their real path or the tree's, change the tree when an entry came
    /// or went in a folder on show; not inside `.git`, in a folder shut, or outside the folder.
    #[test]
    fn the_watcher_reads_again_only_for_folders_on_show() {
        let (real, given) = (Path::new("/private/var/p"), Path::new("/var/p"));
        let shown = |d: &Path| d == Path::new("/var/p") || d == Path::new("/var/p/src");
        let changed = |paths: &[&str]| {
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            changes_shown(&paths, real, given, shown)
        };
        assert!(changed(&["/private/var/p/new.txt"]), "by its real path");
        assert!(changed(&["/var/p/src/main.rs"]), "by the tree's");
        assert!(!changed(&["/private/var/p/.git/index"]));
        assert!(
            !changed(&["/private/var/p/target/debug/x.o"]),
            "a folder shut"
        );
        assert!(!changed(&["/private/var/p"]), "the folder itself");
        assert!(!changed(&["/elsewhere/a.txt"]));
        assert!(changed(&["/private/var/p/target/x", "/private/var/p/b.rs"]));
    }
}
