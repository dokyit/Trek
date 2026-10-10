//! Source Control in the editor's side bar, as VS Code lays it out: the commit message and
//! Commit (what's staged, or everything when nothing is), then Staged Changes and Changes. A
//! click on a file opens its diff in an editor tab (the preview tab, as a click in the Explorer
//! does); its row has Stage or Unstage, and Discard (which asks first: tracked files go back to
//! the index's version, new ones to the Trash), and a context menu with the rest (Open File, Open
//! Changes, Copy Path, Reveal in Finder). Git runs off the main thread; once it's done the list,
//! the status bar's branch, the activity badge, the Explorer's marks and open diffs read again
//! at once (no waiting on a timer).

use super::IdeWorkbench;
use super::diff_view::DiffSource;
use crate::palette;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The largest untracked file whose lines are counted.
const COUNT_LIMIT: u64 = 4 << 20;

/// A changed file, in one of the two lists.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScmFile {
    /// Relative to the repository's top folder.
    pub rel: String,
    /// `M`, `A`, `D`, `R`, `U` (untracked) or `!` (in conflict).
    pub status: char,
    pub added: i64,
    pub removed: i64,
}

/// The repository's state, as read.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScmState {
    pub repo: bool,
    /// The repository's top folder (as the IDE folder names it).
    pub top: Option<PathBuf>,
    pub branch: String,
    /// (behind, ahead) its upstream.
    pub upstream: Option<(u32, u32)>,
    pub staged: Vec<ScmFile>,
    pub changes: Vec<ScmFile>,
}

fn numstat(top: &Path, cached: bool) -> HashMap<String, (i64, i64)> {
    let mut args = vec!["diff", "--numstat", "--no-renames", "--no-ext-diff"];
    if cached {
        args.push("--cached");
    }
    super::git::run(top, &args)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let (a, r, p) = (parts.next()?, parts.next()?, parts.next()?);
            Some((p.to_string(), (a.parse().unwrap_or(0), r.parse().unwrap_or(0))))
        })
        .collect()
}

/// Read the repository holding `root`: its branch and the two lists.
pub(crate) fn read(root: &Path) -> ScmState {
    let Some(top) = super::git::top(root) else { return ScmState::default() };
    let branch = super::git::run(&top, &["branch", "--show-current"]).map(|b| b.trim().to_string()).unwrap_or_default();
    let upstream = super::git::run(&top, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]).ok().and_then(|s| {
        let mut it = s.split_whitespace().filter_map(|n| n.parse::<u32>().ok());
        Some((it.next()?, it.next()?))
    });
    let out = super::git::command(&top).args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]).output().ok().filter(|o| o.status.success()).map(|o| o.stdout).unwrap_or_default();
    let (staged_n, changes_n) = (numstat(&top, true), numstat(&top, false));
    let mut state = ScmState { repo: true, top: Some(top.clone()), branch, upstream, ..Default::default() };
    let mut entries = out.split(|b| *b == 0).filter(|e| e.len() > 3);
    while let Some(e) = entries.next() {
        let (x, y) = (e[0] as char, e[1] as char);
        let rel = String::from_utf8_lossy(&e[3..]).to_string();
        // A rename's entry is followed by where it was.
        if x == 'R' || x == 'C' {
            entries.next();
        }
        let conflict = x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D');
        if conflict {
            state.changes.push(ScmFile { rel, status: '!', added: 0, removed: 0 });
            continue;
        }
        if x == '?' {
            let lines = std::fs::metadata(top.join(&rel))
                .ok()
                .filter(|m| m.len() <= COUNT_LIMIT)
                .and_then(|_| std::fs::read_to_string(top.join(&rel)).ok())
                .map_or(0, |t| t.lines().count() as i64);
            state.changes.push(ScmFile { rel, status: 'U', added: lines, removed: 0 });
            continue;
        }
        if x != ' ' {
            let (a, r) = staged_n.get(&rel).copied().unwrap_or_default();
            state.staged.push(ScmFile { rel: rel.clone(), status: if x == 'C' { 'A' } else { x }, added: a, removed: r });
        }
        if y != ' ' {
            let (a, r) = changes_n.get(&rel).copied().unwrap_or_default();
            state.changes.push(ScmFile { rel, status: y, added: a, removed: r });
        }
    }
    state
}

/// Which list a row is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Staged,
    Changes,
}

pub struct ScmView {
    workspace: Entity<Workspace>,
    workbench: WeakEntity<IdeWorkbench>,
    root: Option<PathBuf>,
    pub(crate) state: ScmState,
    loading: bool,
    /// Git at work: what's running (its button spins).
    busy: Option<&'static str>,
    message: Entity<InputState>,
    /// The file whose diff is open, by (list, path).
    selected: Option<(bool, String)>,
    /// `files_epoch` + `turns_finished` as last read.
    seen: u64,
    run: u64,
    _read: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ScmView {
    pub fn new(workspace: Entity<Workspace>, workbench: WeakEntity<IdeWorkbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder(crate::keys::localize("Message (⌘↩ to commit)").into_owned()));
        let subscriptions = vec![
            cx.observe(&workspace, |this, ws, cx| {
                let (root, seen) = {
                    let ws = ws.read(cx);
                    (ws.ide_root.clone(), ws.files_epoch + ws.turns_finished + ws.agent_edits)
                };
                if root != this.root || seen != this.seen {
                    this.seen = seen;
                    this.refresh(cx);
                }
            }),
            cx.subscribe_in(&message, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { secondary: true, .. } = event {
                    this.commit(window, cx);
                }
            }),
        ];
        let mut this = Self {
            workspace,
            workbench,
            root: None,
            state: ScmState::default(),
            loading: false,
            busy: None,
            message,
            selected: None,
            seen: 0,
            run: 0,
            _read: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// Read the repository again (off the main thread).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.root = self.workspace.read(cx).ide_root.clone();
        let Some(root) = self.root.clone() else {
            self.state = ScmState::default();
            cx.notify();
            return;
        };
        self.run += 1;
        let run = self.run;
        self.loading = true;
        cx.notify();
        self._read = Some(cx.spawn(async move |this, cx| {
            let state = cx.background_executor().spawn(async move { read(&root) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.run == run {
                    this.loading = false;
                    this.state = state;
                    cx.notify();
                }
            });
        }));
    }

    /// Run `op` (git, off the main thread) with `label`'s button spinning; then everything that
    /// shows git's state reads it again, at once.
    fn run_op(&mut self, label: &'static str, op: impl FnOnce(&Path) -> Result<(), String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        self.run_then(label, op, |_, _, _| {}, window, cx);
    }

    /// `run_op`, then `done` when it worked.
    fn run_then(
        &mut self,
        label: &'static str,
        op: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(top) = self.state.top.clone() else { return };
        self.busy = Some(label);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let t = top.clone();
            let result = cx.background_executor().spawn(async move { op(&t) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(()) => done(this, window, cx),
                    Err(e) => this.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: format!("{label} failed: {e}"), undo: None })),
                }
                this.git_moved(window, cx);
            });
        })
        .detach();
    }

    /// Git's state moved: the list, the status bar and badge, the Explorer and open diffs.
    fn git_moved(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh(cx);
        if let Some(root) = self.root.clone() {
            self.workspace.update(cx, |ws, cx| ws.refresh_git_at(root, cx));
        }
        if let Some(wb) = self.workbench.upgrade() {
            wb.update(cx, |wb, cx| {
                wb.explorer.update(cx, |e, cx| e.refresh(cx));
                wb.reload_diffs(cx);
            });
        }
    }

    pub(crate) fn stage(&mut self, rels: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if rels.is_empty() {
            return;
        }
        self.run_op(
            "Stage",
            move |top| {
                let mut args = vec!["add", "-A", "--"];
                args.extend(rels.iter().map(String::as_str));
                super::git::run(top, &args).map(|_| ())
            },
            window,
            cx,
        );
    }

    pub(crate) fn unstage(&mut self, rels: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if rels.is_empty() {
            return;
        }
        self.run_op(
            "Unstage",
            move |top| {
                let paths: Vec<&str> = rels.iter().map(String::as_str).collect();
                let mut args = vec!["restore", "--staged", "--"];
                args.extend(&paths);
                // Before the first commit there's no HEAD to restore from: take them out of the index.
                super::git::run(top, &args).or_else(|_| {
                    let mut args = vec!["rm", "--cached", "-r", "-q", "--"];
                    args.extend(&paths);
                    super::git::run(top, &args)
                })?;
                Ok(())
            },
            window,
            cx,
        );
    }

    /// Discard the Changes of `files`, after asking: tracked files go back to the index's
    /// version, new ones to the Trash.
    pub(crate) fn discard(&mut self, files: Vec<ScmFile>, window: &mut Window, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        let new = files.iter().filter(|f| f.status == 'U').count();
        let title = match files.as_slice() {
            [f] if f.status == 'U' => format!("Move {} to the Trash?", name(&f.rel)),
            [f] => format!("Discard your changes to {}?", name(&f.rel)),
            _ => format!("Discard the changes to {} files?", files.len()),
        };
        let detail = match (new, files.len()) {
            (0, _) => "They go back to how they are in the index. This can't be undone.",
            (n, total) if n == total => "They're new to git, so there's nothing to go back to: they go to the Trash.",
            _ => "Changed files go back to how they are in the index (this can't be undone); new ones go to the Trash.",
        };
        let answer = window.prompt(PromptLevel::Warning, &title, Some(detail), &["Discard", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_op(
                    "Discard",
                    move |top| {
                        let (untracked, tracked): (Vec<ScmFile>, Vec<ScmFile>) = files.into_iter().partition(|f| f.status == 'U');
                        if !tracked.is_empty() {
                            let mut args = vec!["restore", "--worktree", "--"];
                            args.extend(tracked.iter().map(|f| f.rel.as_str()));
                            super::git::run(top, &args)?;
                        }
                        for f in untracked {
                            crate::system::trash(&top.join(&f.rel)).map_err(|e| format!("{}: {e:#}", f.rel))?;
                        }
                        Ok(())
                    },
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Commit what's staged, or everything when nothing is.
    pub(crate) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().trim().to_string();
        if msg.is_empty() {
            self.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: "Write a commit message first.".into(), undo: None }));
            return;
        }
        if self.state.staged.is_empty() && self.state.changes.is_empty() {
            return;
        }
        let all = self.state.staged.is_empty();
        let sent = msg.clone();
        self.run_then(
            "Commit",
            move |top| {
                if all {
                    super::git::run(top, &["add", "-A"])?;
                }
                super::git::run(top, &["commit", "-q", "-m", &msg]).map(|_| ())
            },
            // The box empties once the commit is in (unless something else was typed meanwhile).
            move |this, window, cx| {
                if this.message.read(cx).value().trim() == sent {
                    this.message.update(cx, |s, cx| s.set_value("", window, cx));
                }
            },
            window,
            cx,
        );
    }

    fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let upstream = self.state.upstream.is_some();
        self.run_op("Push", move |top| super::git::run(top, if upstream { &["push"] } else { &["push", "-u", "origin", "HEAD"] }).map(|_| ()), window, cx);
    }

    /// Open `file`'s diff (from `list`) in an editor tab.
    pub(crate) fn open_changes(&mut self, file: &ScmFile, staged: bool, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(top) = self.state.top.clone() else { return };
        self.selected = Some((staged, file.rel.clone()));
        let source = DiffSource::Git { top, rel: file.rel.clone(), staged };
        if let Some(wb) = self.workbench.upgrade() {
            window.defer(cx, move |window, cx| {
                wb.update(cx, |wb, cx| {
                    wb.open_diff_view(source, preview, window, cx);
                });
            });
        }
        cx.notify();
    }

    fn open_file(&self, rel: &str, cx: &mut Context<Self>) {
        let Some(top) = self.state.top.clone() else { return };
        let path = top.join(rel);
        self.workspace.update(cx, |ws, cx| ws.open_editor(path, None, cx));
    }

    fn row(&self, file: &ScmFile, list: List, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let staged = list == List::Staged;
        let color = match file.status {
            'A' | 'U' => palette::emerald(cx),
            'D' | '!' => palette::red(cx),
            _ => palette::amber(cx),
        };
        let (dir, name) = match file.rel.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), file.rel.clone()),
        };
        let on = self.selected.as_ref() == Some(&(staged, file.rel.clone()));
        let key = format!("{}-{}", if staged { "staged" } else { "changes" }, file.rel);
        let group = SharedString::from(format!("scm-row-{key}"));
        let busy = self.busy.is_some();
        let action = |id: String, icon: Icon, tip: &'static str| crate::ui::icon_button(SharedString::from(id), icon, tip).xsmall().disabled(busy);
        let (f1, f2, f3, f4) = (file.clone(), file.clone(), file.clone(), file.clone());
        let (rel, rel2) = (file.rel.clone(), file.rel.clone());
        let me = cx.weak_entity();
        let top = self.state.top.clone().unwrap_or_default();
        h_flex()
            .id(SharedString::from(format!("scm-file-{key}")))
            .test_support()
            .group(group.clone())
            .h(px(22.))
            .pl(px(18.))
            .pr(px(8.))
            .gap(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .when(on, |el| el.bg(theme.list_active))
            .when(!on, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(crate::file_icon::badge(&name, px(14.), cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(div().flex_none().when(file.status == 'D', |el| el.line_through()).child(name))
                    .child(div().min_w_0().truncate().text_size(px(11.5)).text_color(muted).child(dir)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(1.))
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .child(action(format!("scm-open-{key}"), Icon::new(IconName::File), "Open File").on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_file(&rel, cx);
                    })))
                    .when(!staged, |el| {
                        el.child(action(format!("scm-discard-{key}"), Icon::new(crate::assets::Lucide::Undo2), "Discard Changes").on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.discard(vec![f1.clone()], window, cx);
                        })))
                    })
                    .child(if staged {
                        action(format!("scm-unstage-{key}"), Icon::new(IconName::Minus), "Unstage Changes").on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.unstage(vec![rel2.clone()], window, cx);
                        }))
                    } else {
                        action(format!("scm-stage-{key}"), Icon::new(IconName::Plus), "Stage Changes").on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.stage(vec![rel2.clone()], window, cx);
                        }))
                    }),
            )
            .child(div().w(px(12.)).flex_none().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(file.status.to_string()))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| this.open_changes(&f2, staged, e.click_count() < 2, window, cx)))
            .context_menu(move |menu, _, _| {
                let me1 = me.clone();
                let act = move |label: &'static str, f: Box<dyn Fn(&mut ScmView, &mut Window, &mut Context<ScmView>)>| {
                    let me = me1.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let _ = me.update(cx, |this, cx| f(this, window, cx));
                    })
                };
                let (open, changes, stage, discard) = (f3.clone(), f3.clone(), f4.clone(), f4.clone());
                let path = top.join(&f3.rel);
                let (copy, reveal) = (path.display().to_string(), path.clone());
                let menu = menu
                    .min_w(px(200.))
                    .item(act("Open File", Box::new(move |this, _, cx| this.open_file(&open.rel, cx))))
                    .item(act("Open Changes", Box::new(move |this, window, cx| this.open_changes(&changes, staged, false, window, cx))))
                    .separator();
                let menu = if staged {
                    menu.item(act("Unstage Changes", Box::new(move |this, window, cx| this.unstage(vec![stage.rel.clone()], window, cx))))
                } else {
                    menu.item(act("Stage Changes", Box::new(move |this, window, cx| this.stage(vec![stage.rel.clone()], window, cx))))
                        .item(act("Discard Changes", Box::new(move |this, window, cx| this.discard(vec![discard.clone()], window, cx))))
                };
                menu.separator()
                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                    .item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(&reveal)))
            })
    }

    fn section(&self, list: List, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = match list {
            List::Staged => self.state.staged.clone(),
            List::Changes => self.state.changes.clone(),
        };
        if files.is_empty() && list == List::Staged {
            return None;
        }
        let theme = cx.theme().clone();
        let busy = self.busy.is_some();
        let title = match list {
            List::Staged => "Staged Changes",
            List::Changes => "Changes",
        };
        let id = match list {
            List::Staged => "staged",
            List::Changes => "changes",
        };
        let rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
        let all = files.clone();
        let header = h_flex()
            .id(SharedString::from(format!("scm-section-{id}")))
            .test_support()
            .group(SharedString::from(format!("scm-section-{id}")))
            .h(px(24.))
            .px(px(8.))
            .gap(px(4.))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .child(Icon::new(IconName::ChevronDown).size(px(11.)))
            .child(div().flex_1().child(title.to_uppercase()))
            .child(
                h_flex()
                    .gap(px(1.))
                    .invisible()
                    .group_hover(SharedString::from(format!("scm-section-{id}")), |s| s.visible())
                    .when(list == List::Changes && !files.is_empty(), |el| {
                        el.child(crate::ui::icon_button("scm-discard-all", crate::assets::Lucide::Undo2, "Discard All Changes").xsmall().disabled(busy).on_click(cx.listener(move |this, _, window, cx| this.discard(all.clone(), window, cx))))
                            .child(crate::ui::icon_button("scm-stage-all", IconName::Plus, "Stage All Changes").xsmall().disabled(busy).on_click(cx.listener({
                                let rels = rels.clone();
                                move |this, _, window, cx| this.stage(rels.clone(), window, cx)
                            })))
                    })
                    .when(list == List::Staged, |el| {
                        el.child(crate::ui::icon_button("scm-unstage-all", IconName::Minus, "Unstage All Changes").xsmall().disabled(busy).on_click(cx.listener({
                            let rels = rels.clone();
                            move |this, _, window, cx| this.unstage(rels.clone(), window, cx)
                        })))
                    }),
            )
            .child(div().px(px(5.)).rounded_full().bg(theme.foreground.opacity(0.1)).text_size(px(10.)).child(files.len().to_string()));
        let rows: Vec<AnyElement> = files.iter().map(|f| self.row(f, list, cx).into_any_element()).collect();
        Some(v_flex().child(header).children(rows).into_any_element())
    }
}

fn name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

impl Render for ScmView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        if self.root.is_none() {
            return crate::panels::empty("Open a folder to see its changes.", cx).into_any_element();
        }
        if !self.state.repo {
            if self.loading {
                return div().size_full().into_any_element();
            }
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_sm().text_color(muted).child("This folder isn't a Git repository."))
                .child(Button::new("scm-init").small().outline().label("Initialize Repository").on_click(cx.listener(|this, _, window, cx| {
                    let Some(root) = this.root.clone() else { return };
                    this.state.top = Some(root);
                    this.run_op("Initialize", |top| super::git::run(top, &["init", "-q"]).map(|_| ()), window, cx);
                })))
                .into_any_element();
        }
        let staged = !self.state.staged.is_empty();
        let nothing = !staged && self.state.changes.is_empty();
        let busy = self.busy;
        let (behind, ahead) = self.state.upstream.unwrap_or((0, 0));
        let branch = if self.state.branch.is_empty() { "detached".to_string() } else { self.state.branch.clone() };
        let commit_label = if staged { "Commit" } else { "Commit All" };
        let head = h_flex()
            .h(px(28.))
            .px(px(12.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(muted)
            .child(Icon::new(crate::assets::Lucide::GitBranch).size(px(12.)))
            .child(div().min_w_0().truncate().text_color(theme.foreground.opacity(0.85)).child(branch))
            .when(ahead + behind > 0, |el| el.child(div().flex_none().child(format!("↓{behind} ↑{ahead}"))))
            .child(div().flex_1())
            .when(self.loading, |el| el.child(Spinner::new().xsmall().color(muted)))
            .child(crate::ui::icon_button("scm-refresh", IconName::RefreshCw, "Refresh").xsmall().on_click(cx.listener(|this, _, window, cx| this.git_moved(window, cx))));
        let commit = v_flex()
            .px(px(10.))
            .pb(px(8.))
            .gap(px(6.))
            .child(div().id("scm-message").test_support().child(Input::new(&self.message).small()))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Button::new("scm-commit")
                            .small()
                            .primary()
                            .flex_1()
                            .icon(IconName::Check)
                            .label(commit_label)
                            .loading(busy == Some("Commit"))
                            .disabled(nothing || busy.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    )
                    .child(
                        Button::new("scm-push")
                            .small()
                            .outline()
                            .icon(IconName::ArrowUp)
                            .label(if ahead > 0 { format!("Push {ahead}") } else { "Push".into() })
                            .loading(busy == Some("Push"))
                            .disabled(busy.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.push(window, cx))),
                    ),
            );
        let staged_section = self.section(List::Staged, cx);
        let changes_section = self.section(List::Changes, cx);
        v_flex()
            .id("scm")
            .test_support()
            .size_full()
            .child(head)
            .child(commit)
            .child(
                v_flex()
                    .id("scm-lists")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(px(8.))
                    .children(staged_section)
                    .children(changes_section)
                    .when(nothing && !self.loading, |el| el.child(div().px(px(18.)).py(px(6.)).text_size(px(12.)).text_color(muted).child("No changes. The working tree is clean."))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
impl ScmView {
    /// The two lists: "S M a.rs", "C U b.rs".
    pub(crate) fn describe(&self) -> Vec<String> {
        self.state.staged.iter().map(|f| format!("S {} {}", f.status, f.rel)).chain(self.state.changes.iter().map(|f| format!("C {} {}", f.status, f.rel))).collect()
    }

    pub(crate) fn idle(&self) -> bool {
        !self.loading && self.busy.is_none()
    }

    pub(crate) fn set_message(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
    }

    pub(crate) fn file(&self, staged: bool, rel: &str) -> Option<ScmFile> {
        if staged { self.state.staged.iter() } else { self.state.changes.iter() }.find(|f| f.rel == rel).cloned()
    }
}
