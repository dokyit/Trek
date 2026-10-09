//! Explorer: the project's file tree. A file opens in the in-app editor; a folder expands.
//!
//! Folders are read off the main thread and kept: a frame draws what was read last, and the
//! tree reads again when files changed under it (a turn finished, a checkout, a rewind) or the
//! user asks (Refresh); in the editor also when FSEvents says files moved (`ide::watch`). Files
//! git ignores are left out, as `.gitignore` (and the repository's excludes) say. In the editor
//! the tree also shows git status: a letter and a colour per changed file, a dot on folders
//! holding changes; and a row's context menu adds it to the AI side bar's chat.

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

/// Rows drawn at most: a tree opened that far is beyond reading anyway.
const MAX_ROWS: usize = 4000;

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
            !HIDDEN.contains(&n.as_ref()) && !n.ends_with(".nosync") && (dots || !n.starts_with('.'))
        })
        .map(|e| {
            // A link to a folder opens like one.
            let is_dir = e.file_type().is_some_and(|t| t.is_dir()) || (e.path_is_symlink() && e.path().is_dir());
            (e.into_path(), is_dir)
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.file_name().cmp(&b.0.file_name())));
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
        trek_core::git::read_only(root).args(args).output().ok().filter(|o| o.status.success()).map(|o| o.stdout)
    };
    // Where `root` sits in the repository: status paths are relative to its top.
    let Some(prefix) = run(&["rev-parse", "--show-prefix"]).map(|o| String::from_utf8_lossy(&o).trim().to_string()) else { return HashMap::new() };
    let Some(out) = run(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]) else { return HashMap::new() };
    let mut marks = HashMap::new();
    let mut entries = out.split(|b| *b == 0).filter(|e| e.len() > 3);
    while let Some(e) = entries.next() {
        let (x, y) = (e[0] as char, e[1] as char);
        let rel = String::from_utf8_lossy(&e[3..]).to_string();
        // A rename's entry is followed by its old path.
        if x == 'R' || x == 'C' {
            entries.next();
        }
        let Some(under) = rel.strip_prefix(prefix.as_str()) else { continue };
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

/// Why `name` can't be made or renamed to in `dir`, when it can't.
fn name_problem(dir: &Path, name: &str, nested: bool) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("A name is needed.".into());
    }
    if name.split('/').any(|p| p == "." || p == "..") || name.starts_with('/') || (!nested && name.contains('/')) {
        return Some(format!("“{name}” isn't a valid name here."));
    }
    dir.join(name).symlink_metadata().is_ok().then(|| format!("A file or folder “{name}” already exists here. Choose another name."))
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
    /// The editor's tree: compact rows, git marks, a single click opens a preview tab.
    ide: bool,
    git: HashMap<PathBuf, GitMark>,
    /// Folders holding changed files (each ancestor of one, up to the root).
    git_dirs: HashSet<PathBuf>,
    /// Reading folders in the background; a newer read replaces it.
    _read: Option<Task<()>>,
    _subscription: Subscription,
}

impl ExplorerPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(workspace, |ws| ws.current_cwd(), "Open a project to browse its files.", false, cx)
    }

    /// The IDE's tree: roots at `ide_root` instead of the route's folder, so it stays put while
    /// the AI side bar moves between chats.
    pub fn for_ide(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(workspace, |ws| ws.ide_root.clone(), "Open a folder, or a file, to start.", true, cx)
    }

    fn from(workspace: Entity<Workspace>, root: impl Fn(&Workspace) -> Option<PathBuf> + 'static, empty_hint: &'static str, ide: bool, cx: &mut Context<Self>) -> Self {
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
            ide,
            git: HashMap::new(),
            git_dirs: HashSet::new(),
            _read: None,
            _subscription: sub,
        };
        this.refresh(cx);
        this
    }

    fn files_now(ws: &Workspace) -> u64 {
        ws.turns_finished + ws.files_epoch
    }

    /// Read the root and every open folder again (and git status, in the editor).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let dirs: Vec<PathBuf> = std::iter::once(root.clone()).chain(self.expanded.iter().filter(|d| d.starts_with(&root)).cloned()).collect();
        self.read(dirs, true, cx);
    }

    /// Fold every folder back up.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        cx.notify();
    }

    /// Point the tree at `path` (the open editor's file): its folders open, its row selected.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        if !path.starts_with(&root) {
            return;
        }
        let missing: Vec<PathBuf> = path.ancestors().skip(1).take_while(|a| a.starts_with(&root) && *a != root).map(Path::to_path_buf).filter(|a| self.expanded.insert(a.clone())).collect();
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
                    let read: Vec<(PathBuf, Vec<(PathBuf, bool)>)> = dirs.into_iter().map(|d| {
                        let c = children(&d, dots);
                        (d, c)
                    }).collect();
                    (read, git_root.map(|r| git_marks(&r)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if whole {
                    // What's shown now: the root and the folders open (one opened since this read
                    // started keeps what its own read brought).
                    let root = this.root.clone();
                    let expanded = &this.expanded;
                    this.entries.retain(|d, _| root.as_ref() == Some(d) || expanded.contains(d));
                }
                this.entries.extend(read);
                if let Some(git) = git {
                    let root = this.root.clone().unwrap_or_default();
                    this.git_dirs = git.keys().flat_map(|p| p.ancestors().skip(1).take_while(|a| a.starts_with(&root)).map(Path::to_path_buf).collect::<Vec<_>>()).collect();
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

    fn rows(&self, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, bool, usize)>) {
        for (path, is_dir) in self.entries.get(dir).into_iter().flatten() {
            if out.len() >= MAX_ROWS {
                return;
            }
            // Open once its entries are in: never an open folder with nothing under it.
            let open = *is_dir && self.expanded.contains(path) && self.entries.contains_key(path);
            out.push((path.clone(), *is_dir, depth));
            if open {
                self.rows(path, depth + 1, out);
            }
        }
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
    pub fn begin_new(&mut self, dir: PathBuf, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.as_ref().is_some_and(|r| dir != *r) && self.expanded.insert(dir.clone()) {
            self.read(vec![dir.clone()], false, cx);
        }
        self.begin(if folder { EditKind::NewFolder } else { EditKind::NewFile }, dir, "", window, cx);
    }

    /// Start renaming `path`, its name in an input in its row (the stem picked).
    pub fn begin_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.begin(EditKind::Rename(path), dir, &name, window, cx);
    }

    fn begin(&mut self, kind: EditKind, dir: PathBuf, value: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
            InputEvent::Blur => this.cancel_edit(cx),
            _ => {}
        });
        self.edit = Some(Edit { kind, dir, input, _sub: sub });
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.edit.take().is_some() {
            cx.notify();
        }
    }

    fn toast(&self, message: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
    }

    /// The name typed goes: the file or folder is made (a new file opens), or renamed. A name
    /// that won't do keeps the input up, saying why.
    pub(crate) fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else { return };
        let name = edit.input.read(cx).value().trim().to_string();
        let (kind, dir) = (edit.kind.clone(), edit.dir.clone());
        if let EditKind::Rename(from) = &kind {
            if from.file_name().is_some_and(|n| n.to_string_lossy() == name) {
                return self.cancel_edit(cx);
            }
        }
        if let Some(why) = name_problem(&dir, &name, !matches!(kind, EditKind::Rename(_))) {
            return self.toast(why, cx);
        }
        let to = dir.join(&name);
        let made = match &kind {
            EditKind::NewFile => to.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&to).map(|_| ())),
            EditKind::NewFolder => std::fs::create_dir_all(&to),
            EditKind::Rename(from) => std::fs::rename(from, &to),
        };
        if let Err(e) = made {
            return self.toast(format!("Couldn't {} “{name}”: {e}", if matches!(kind, EditKind::Rename(_)) { "rename to" } else { "make" }), cx);
        }
        self.edit = None;
        self.selected = Some(to.clone());
        match kind {
            EditKind::NewFile => self.workspace.update(cx, |ws, cx| ws.open_editor(to.clone(), None, cx)),
            EditKind::NewFolder => _ = self.expanded.insert(to.clone()),
            EditKind::Rename(from) => {
                // Folders open under the old name stay open under the new one.
                let moved: Vec<PathBuf> = self.expanded.iter().filter(|d| d.starts_with(&from)).cloned().collect();
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
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
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
                    Err(e) => this.toast(format!("Couldn't move “{name}” to the Trash: {e:#}"), cx),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// The input row of a name being typed, at `depth`.
    fn edit_row(&self, depth: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.edit.as_ref()?;
        let folder = edit.kind == EditKind::NewFolder || matches!(&edit.kind, EditKind::Rename(p) if p.is_dir());
        let icon = if folder { crate::file_icon::folder("", false, px(14.), cx) } else { crate::file_icon::badge(&edit.input.read(cx).value().to_string(), px(14.), cx) };
        Some(
            h_flex()
                .id("explorer-edit")
                .test_support()
                .h(px(24.))
                .pl(px(8. + depth as f32 * 12. + 18.))
                .pr_2()
                .gap(px(6.))
                .child(icon)
                .child(div().flex_1().min_w_0().child(Input::new(&edit.input).xsmall()))
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

/// A row's context menu in the editor: Add to Chat, New File and New Folder (in the folder, or
/// beside the file), Rename and Delete, then Finder and the clipboard.
fn row_menu(menu: PopupMenu, me: WeakEntity<ExplorerPanel>, ws: &Entity<Workspace>, path: &Path, is_dir: bool, root: &Path) -> PopupMenu {
    let copy = |label: &'static str, text: String| PopupMenuItem::new(label).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
    let relative = path.strip_prefix(root).map(|p| p.display().to_string()).unwrap_or_else(|_| path.display().to_string());
    let (ws, chip, reveal) = (ws.clone(), path.to_path_buf(), path.to_path_buf());
    let dir = if is_dir { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf()) };
    let act = |label: &'static str, f: Box<dyn Fn(&mut ExplorerPanel, &mut Window, &mut Context<ExplorerPanel>)>| {
        let me = me.clone();
        PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = me.update(cx, |this, cx| f(this, window, cx));
        })
    };
    let (d1, d2, rename, delete) = (dir.clone(), dir, path.to_path_buf(), path.to_path_buf());
    let at_root = path == root;
    let menu = menu
        .min_w(px(200.))
        .item(PopupMenuItem::new("Add to Chat").on_click(move |_, _, cx| {
            let chip = crate::ide::ai::context::ContextChip::File { path: chip.clone() };
            ws.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::AddToChat(chip)));
        }))
        .separator()
        .item(act("New File…", Box::new(move |this, window, cx| this.begin_new(d1.clone(), false, window, cx))))
        .item(act("New Folder…", Box::new(move |this, window, cx| this.begin_new(d2.clone(), true, window, cx))))
        .separator();
    let menu = if at_root {
        menu
    } else {
        menu.item(act("Rename…", Box::new(move |this, window, cx| this.begin_rename(rename.clone(), window, cx))))
            .item(act("Delete", Box::new(move |this, window, cx| this.delete(delete.clone(), window, cx))))
            .separator()
    };
    menu.item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(&reveal)))
        .item(copy("Copy Path", path.display().to_string()))
        .item(copy("Copy Relative Path", relative))
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(root) = self.root.clone() else {
            return super::empty(self.empty_hint, cx).into_any_element();
        };
        let mut rows = Vec::new();
        self.rows(&root, 0, &mut rows);
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
        for (path, is_dir, depth) in rows {
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
        let tree = v_flex().id("explorer-tree").flex_1().min_h_0().overflow_y_scroll().py_1().children(lead).children(elements);

        v_flex().size_full().child(tree).into_any_element()
    }
}

impl ExplorerPanel {
    /// One row of the tree.
    #[allow(clippy::too_many_arguments)]
    fn tree_row(&self, path: PathBuf, is_dir: bool, depth: usize, root: &Path, selected: &Option<PathBuf>, ide: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let open = self.expanded.contains(&path) && self.entries.contains_key(&path);
            let is_sel = selected.as_ref() == Some(&path);
            let mark = self.git.get(&path).copied();
            let changed_dir = is_dir && self.git_dirs.contains(&path);
            let p = path.clone();
            let row = h_flex()
                .id(SharedString::from(path.display().to_string()))
                .test_support()
                .when(!ide, |el| el.mx_1().h(px(26.)).rounded(px(6.)).text_sm())
                .when(ide, |el| el.h(px(22.)).text_size(px(13.)))
                .pl(px(8. + depth as f32 * if ide { 12. } else { 14. }))
                .pr_2()
                .gap(px(6.))
                .cursor_pointer()
                .when(is_sel, |el| el.bg(theme.list_active))
                .when(!is_sel, |el| el.hover(|s| s.bg(theme.list_hover)))
                .child(if is_dir {
                    Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().text_color(theme.muted_foreground).into_any_element()
                } else {
                    div().w(px(12.)).into_any_element()
                })
                .child(if is_dir {
                    crate::file_icon::folder(&name, open, px(14.), cx)
                } else {
                    crate::file_icon::badge(&name, px(14.), cx)
                })
                .child(div().flex_1().min_w_0().truncate().when_some(mark, |el, m| el.text_color(m.color(cx))).child(name))
                .when_some(mark, |el, m| el.child(div().flex_none().text_xs().font_weight(FontWeight::MEDIUM).text_color(m.color(cx)).child(m.letter())))
                .when(changed_dir, |el| el.child(div().flex_none().size(px(5.)).rounded_full().bg(crate::palette::amber(cx).opacity(0.8))))
                .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    if is_dir {
                        this.toggle_dir(p.clone(), cx);
                    } else {
                        // A file opens in the in-app editor; it highlights here while open. In
                        // the editor a single click previews it, a double click keeps it.
                        this.selected = Some(p.clone());
                        let preview = this.ide && e.click_count() < 2;
                        this.workspace.update(cx, |ws, cx| if preview { ws.preview_editor(p.clone(), cx) } else { ws.open_editor(p.clone(), None, cx) });
                        cx.notify();
                    }
                }));
            if !ide {
                return row.into_any_element();
            }
            let (ws, root, me) = (self.workspace.clone(), root.to_path_buf(), cx.weak_entity());
            row.context_menu(move |menu, _, _| row_menu(menu, me.clone(), &ws, &path, is_dir, &root)).into_any_element()
        }
    }
}
