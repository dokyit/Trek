//! Source control: branch, changed files with stats, a diff viewer, commit and push.

use crate::palette;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
struct FileChange {
    path: String,
    status: String,
    additions: i64,
    deletions: i64,
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    is_repo: bool,
    branch: String,
    upstream: Option<(u32, u32)>,
    files: Vec<FileChange>,
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Add,
    Del,
    Hunk,
    Meta,
    Ctx,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").args(args).current_dir(cwd).env("PATH", trek_core::detect::login_path()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn snapshot(cwd: &Path) -> Snapshot {
    if git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Snapshot::default();
    }
    let branch = git(cwd, &["branch", "--show-current"]).unwrap_or_default().trim().to_string();
    let upstream = git(cwd, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]).ok().and_then(|s| {
        let mut it = s.split_whitespace().filter_map(|n| n.parse::<u32>().ok());
        Some((it.next()?, it.next()?))
    });
    let mut stats = std::collections::HashMap::new();
    let numstat = git(cwd, &["diff", "HEAD", "--numstat"]).or_else(|_| git(cwd, &["diff", "--cached", "--numstat"])).unwrap_or_default();
    for line in numstat.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() == 3 {
            stats.insert(parts[2].to_string(), (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0)));
        }
    }
    let mut files = Vec::new();
    for line in git(cwd, &["status", "--porcelain=v1", "-uall"]).unwrap_or_default().lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let mut path = line[3..].to_string();
        if let Some((_, to)) = path.split_once(" -> ") {
            path = to.to_string();
        }
        let (additions, deletions) = match stats.get(&path) {
            Some(s) => *s,
            None if status == "??" => (std::fs::read_to_string(cwd.join(&path)).map(|s| s.lines().count() as i64).unwrap_or(0), 0),
            None => (0, 0),
        };
        files.push(FileChange { path, status, additions, deletions });
    }
    Snapshot { is_repo: true, branch, upstream, files }
}

fn file_diff(cwd: &Path, file: &FileChange) -> Vec<(LineKind, String)> {
    let text = if file.status == "??" {
        std::fs::read_to_string(cwd.join(&file.path)).map(|s| s.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n")).unwrap_or_else(|_| "Binary or unreadable file".into())
    } else {
        git(cwd, &["diff", "HEAD", "--", &file.path]).or_else(|_| git(cwd, &["diff", "--", &file.path])).unwrap_or_default()
    };
    text.lines()
        .take(5000)
        .map(|l| {
            let kind = if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") {
                LineKind::Meta
            } else if l.starts_with("@@") {
                LineKind::Hunk
            } else if l.starts_with('+') {
                LineKind::Add
            } else if l.starts_with('-') {
                LineKind::Del
            } else {
                LineKind::Ctx
            };
            (kind, l.to_string())
        })
        .filter(|(k, _)| *k != LineKind::Meta)
        .collect()
}

pub struct GitPanel {
    workspace: Entity<Workspace>,
    cwd: Option<PathBuf>,
    snap: Snapshot,
    loading: bool,
    selected: Option<String>,
    diff: Vec<(LineKind, String)>,
    message: Entity<InputState>,
    busy: Option<&'static str>,
    turns_seen: u64,
    _subscriptions: Vec<Subscription>,
    _task: Option<Task<()>>,
}

impl GitPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        let sub = cx.observe(&workspace, |this, ws, cx| {
            let (cwd, turns) = {
                let ws = ws.read(cx);
                (ws.current_cwd(), ws.turns_finished)
            };
            if cwd != this.cwd || turns != this.turns_seen {
                this.turns_seen = turns;
                this.refresh(cx);
            }
        });
        let mut this = Self {
            workspace,
            cwd: None,
            snap: Snapshot::default(),
            loading: false,
            selected: None,
            diff: vec![],
            message,
            busy: None,
            turns_seen: 0,
            _subscriptions: vec![sub],
            _task: None,
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.cwd = self.workspace.read(cx).current_cwd();
        let Some(cwd) = self.cwd.clone() else {
            self.snap = Snapshot::default();
            cx.notify();
            return;
        };
        self.loading = true;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let c = cwd.clone();
            let snap = cx.background_executor().spawn(async move { snapshot(&c) }).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if this.selected.as_ref().is_some_and(|s| !snap.files.iter().any(|f| &f.path == s)) {
                    this.selected = None;
                    this.diff.clear();
                }
                this.snap = snap;
                if let Some(sel) = this.selected.clone() {
                    this.select(sel, cx);
                }
                cx.notify();
            });
        }));
    }

    fn select(&mut self, path: String, cx: &mut Context<Self>) {
        let (Some(cwd), Some(file)) = (self.cwd.clone(), self.snap.files.iter().find(|f| f.path == path).cloned()) else { return };
        self.selected = Some(path);
        cx.spawn(async move |this, cx| {
            let lines = cx.background_executor().spawn(async move { file_diff(&cwd, &file) }).await;
            let _ = this.update(cx, |this, cx| {
                this.diff = lines;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run(&mut self, label: &'static str, steps: Vec<Vec<String>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cwd) = self.cwd.clone() else { return };
        self.busy = Some(label);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut last = String::new();
                    for step in steps {
                        let args: Vec<&str> = step.iter().map(String::as_str).collect();
                        last = git(&cwd, &args)?;
                    }
                    Ok::<String, String>(last)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(_) => window.push_notification(format!("{label} done"), cx),
                    Err(e) => window.push_notification(gpui_kit::component::notification::Notification::error(e), cx),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().trim().to_string();
        if msg.is_empty() {
            window.push_notification("Write a commit message first", cx);
            return;
        }
        self.message.update(cx, |s, cx| s.set_value("", window, cx));
        self.run("Commit", vec![vec!["add".into(), "-A".into()], vec!["commit".into(), "-m".into(), msg]], window, cx);
    }

    fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let step = if self.snap.upstream.is_some() { vec!["push".to_string()] } else { vec!["push".into(), "-u".into(), "origin".into(), "HEAD".into()] };
        self.run("Push", vec![step], window, cx);
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        if self.cwd.is_none() {
            return super::empty("Open a project to see its changes.", cx).into_any_element();
        }
        if !self.snap.is_repo && !self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_sm().text_color(muted).child("This folder isn't a Git repository."))
                .child(Button::new("git-init").small().outline().label("Initialize repository").on_click(cx.listener(|this, _, window, cx| {
                    this.run("Initialize", vec![vec!["init".into()]], window, cx)
                })))
                .into_any_element();
        }
        let (ahead, behind) = self.snap.upstream.map(|(b, a)| (a, b)).unwrap_or((0, 0));
        let total_add: i64 = self.snap.files.iter().map(|f| f.additions).sum();
        let total_del: i64 = self.snap.files.iter().map(|f| f.deletions).sum();
        let selected = self.selected.clone();

        let header = h_flex()
            .px_3()
            .h(px(40.))
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(Icon::new(crate::assets::Lucide::GitBranch).small().text_color(muted))
            .child(div().font_medium().child(if self.snap.branch.is_empty() { "detached".to_string() } else { self.snap.branch.clone() }))
            .when(ahead > 0, |el| el.child(div().text_xs().text_color(muted).child(format!("↑{ahead}"))))
            .when(behind > 0, |el| el.child(div().text_xs().text_color(muted).child(format!("↓{behind}"))))
            .child(div().flex_1())
            .when(self.loading, |el| el.child(Spinner::new().xsmall().color(muted)))
            .child(crate::ui::icon_button("git-refresh", IconName::RefreshCw, "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh(cx))));

        let files = v_flex()
            .id("git-files")
            .max_h(px(260.))
            .overflow_y_scroll()
            .py_1()
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{} changed", self.snap.files.len()))
                    .child(div().flex_1())
                    .child(div().text_color(palette::emerald(cx)).child(format!("+{total_add}")))
                    .child(div().pl_1().text_color(palette::red(cx)).child(format!("−{total_del}"))),
            )
            .children(self.snap.files.iter().map(|f| {
                let color = match f.status.as_str() {
                    "??" | "A" => palette::emerald(cx),
                    "D" => palette::red(cx),
                    _ => palette::amber(cx),
                };
                let letter = if f.status == "??" { "U".to_string() } else { f.status.chars().next().unwrap_or('M').to_string() };
                let path = f.path.clone();
                let is_sel = selected.as_ref() == Some(&f.path);
                let (dir, name) = match f.path.rsplit_once('/') {
                    Some((d, n)) => (format!("{d}/"), n.to_string()),
                    None => (String::new(), f.path.clone()),
                };
                h_flex()
                    .id(SharedString::from(format!("gf-{}", f.path)))
                    .mx_1()
                    .px_2()
                    .h(px(28.))
                    .gap_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_sel, |el| el.bg(theme.list_active))
                    .when(!is_sel, |el| el.hover(|s| s.bg(theme.list_hover)))
                    .child(div().w(px(12.)).text_xs().font_semibold().text_color(color).child(letter))
                    .child(h_flex().flex_1().min_w_0().overflow_hidden().child(div().flex_none().child(name)).child(div().pl_1().truncate().text_xs().text_color(muted).child(dir)))
                    .when(f.additions > 0, |el| el.child(div().text_xs().text_color(palette::emerald(cx)).child(format!("+{}", f.additions))))
                    .when(f.deletions > 0, |el| el.child(div().text_xs().text_color(palette::red(cx)).child(format!("−{}", f.deletions))))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), cx)))
            }));

        let mono = theme.mono_font_family.clone();
        let diff_lines = self.diff.clone();
        let add_bg = palette::emerald(cx).opacity(0.12);
        let del_bg = palette::red(cx).opacity(0.12);
        let diff = if selected.is_none() {
            super::empty(if self.snap.files.is_empty() { "No changes. The working tree is clean." } else { "Select a file to view its diff." }, cx).into_any_element()
        } else {
            uniform_list("git-diff", diff_lines.len(), move |range, _, cx| {
                let theme = cx.theme();
                range
                    .map(|i| {
                        let (kind, text) = &diff_lines[i];
                        div()
                            .px_3()
                            .h(px(19.))
                            .whitespace_nowrap()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .when(*kind == LineKind::Add, |el| el.bg(add_bg))
                            .when(*kind == LineKind::Del, |el| el.bg(del_bg))
                            .when(*kind == LineKind::Hunk, |el| el.text_color(theme.muted_foreground).bg(theme.muted))
                            .child(text.clone())
                    })
                    .collect()
            })
            .size_full()
            .into_any_element()
        };

        let commit = v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(Input::new(&self.message).small())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("git-commit")
                            .small()
                            .primary()
                            .flex_1()
                            .loading(self.busy == Some("Commit"))
                            .disabled(self.snap.files.is_empty())
                            .label("Commit all")
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    )
                    .child(
                        Button::new("git-push")
                            .small()
                            .outline()
                            .loading(self.busy == Some("Push"))
                            .icon(IconName::ArrowUp)
                            .label("Push")
                            .on_click(cx.listener(|this, _, window, cx| this.push(window, cx))),
                    ),
            );

        v_flex()
            .size_full()
            .child(header)
            .child(files)
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme.border).child(diff))
            .child(commit)
            .into_any_element()
    }
}
