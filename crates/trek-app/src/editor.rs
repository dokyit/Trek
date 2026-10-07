//! Editor: an in-app code surface. A project file opens here editable — syntax-highlighted,
//! line-numbered, searchable — and saves back with ⌘S, so a fix doesn't leave the app.
//! Deep links (`trek://edit?path=…&line=…`) land here from editor extensions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One changed line range vs `HEAD`, for the editor's diff fills.
enum DiffMark {
    Added,
    Changed,
    /// Content deleted at this position — a marker block on the line after it.
    Deleted,
}

/// `git diff -U0` hunks for `path`, as 0-based row ranges: (start_row, end_row, kind).
/// Untracked files count as all-added. Not a repo (or clean): empty.
fn diff_line_ranges(path: &Path) -> Vec<(u32, u32, DiffMark)> {
    let Some(dir) = path.parent() else { return vec![] };
    let status = std::process::Command::new("git")
        .args(["-C", &dir.display().to_string(), "status", "--porcelain", "--", &path.display().to_string()])
        .output()
        .ok();
    if let Some(out) = status.filter(|o| o.status.success()) {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.starts_with("??") {
            let n = std::fs::read_to_string(path).map(|t| t.lines().count() as u32).unwrap_or(0);
            return vec![(0, n, DiffMark::Added)];
        }
    }
    let Ok(out) = std::process::Command::new("git")
        .args(["-C", &dir.display().to_string(), "diff", "--no-color", "--unified=0", "--", &path.display().to_string()])
        .output()
    else {
        return vec![];
    };
    let mut marks = vec![];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        // @@ -old_start[,old_count] +new_start[,new_count] @@
        let Some(rest) = line.strip_prefix("@@ -") else { continue };
        let Some((old, rest)) = rest.split_once(' ') else { continue };
        let Some(new) = rest.strip_prefix('+').and_then(|r| r.split_once(' ')).map(|(n, _)| n) else { continue };
        let num = |s: &str| s.split(',').next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let cnt = |s: &str| s.split(',').nth(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
        let (_os, oc) = (num(old), cnt(old));
        let (ns, nc) = (num(new), cnt(new));
        let row = ns.saturating_sub(1);
        if nc == 0 {
            marks.push((row, row + 1, DiffMark::Deleted));
        } else {
            let kind = if oc > 0 { DiffMark::Changed } else { DiffMark::Added };
            marks.push((row, row + nc, kind));
        }
    }
    marks
}

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, TabSize};
use gpui_kit::base::input::{Point, RopeExt};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::palette;
use crate::workspace::{Workspace, WorkspaceEvent};

/// What a too-big or unreadable file gets instead of an editor: a note and a way out.
const MAX_FILE_BYTES: u64 = 2_000_000;

actions!(trek_editor, [SaveFile]);

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("cmd-s", SaveFile, Some("TrekEditor"))]
}

pub struct EditorView {
    workspace: Entity<Workspace>,
    pub(crate) path: PathBuf,
    state: Entity<EditorState>,
    /// Text as last loaded or saved — `value() != saved` means dirty.
    saved: String,
    pub(crate) dirty: bool,
    /// Why the file didn't load (too big, unreadable, gone). The view is read-only then.
    problem: Option<String>,
    /// The language server this doc is open on, when one covers its extension.
    lsp: Option<(Arc<crate::lsp_client::Client>, String)>,
    /// Keeps the diagnostics watcher alive.
    _diag_watch: Option<Task<()>>,
    /// Git-diff line fills (added/changed/deleted vs HEAD).
    diff_marks: Option<gpui_kit::base::input::RangeDecorationCollection>,
    _sub: Subscription,
}

impl EditorView {
    pub fn new(workspace: Entity<Workspace>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (text, problem) = load(&path);
        let lang = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
        let saved = text.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .language(lang.clone())
                .line_number(true)
                .searchable(true)
                .tab_size(TabSize { tab_size: 4, hard_tabs: false });
            s.set_value(text.clone(), window, cx);
            if problem.is_some() {
                s.set_readonly(true, cx);
            }
            s
        });

        // A language server covers this file? Providers on the state, the doc opened, and a
        // watcher folding publishDiagnostics into the gutter.
        tracing::info!("lsp: {} lang={lang} problem={problem:?}", path.display());
        let lsp = problem.is_none().then(|| {
            crate::lsp_client::uri_for(&path).and_then(|uri| {
                workspace.update(cx, |ws, _| ws.lsp_for(&path, &lang)).map(|client| {
                    state.update(cx, |s, _| *s.lsp_mut() = client.lsp_for(uri.clone(), workspace.clone()));
                    client.did_open(&uri, &text);
                    (client, uri)
                })
            })
        }).flatten();
        let diag_watch = lsp.as_ref().map(|(client, uri)| {
            let rx = client.watch_diagnostics();
            let uri = uri.clone();
            let state = state.clone();
            cx.spawn(async move |_, cx| {
                while let Ok((u, diags)) = rx.recv().await {
                    if u != uri {
                        continue;
                    }
                    cx.update_entity(&state, |s, cx| {
                        let rope = s.text().clone();
                        if let Some(set) = s.diagnostics_mut() {
                            set.reset(&rope);
                            set.extend(diags.iter().cloned().map(gpui_kit::base::input::Diagnostic::from));
                        }
                        cx.notify();
                    });
                }
            })
        });

        let sub = cx.subscribe(&state, |this, s, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = s.read(cx).value().to_string();
                if let Some((client, uri)) = &this.lsp {
                    client.did_change(uri, &text);
                }
                let dirty = text.as_str() != this.saved.as_str();
                if dirty != this.dirty {
                    this.dirty = dirty;
                    cx.notify();
                }
            }
        });
        let mut this = Self { workspace, path, state, saved, dirty: false, problem, lsp, _diag_watch: diag_watch, diff_marks: None, _sub: sub };
        this.refresh_diff(cx);
        this
    }

    /// Paint changed lines vs HEAD — added/modified fills, a marker where lines went away.
    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let lines = diff_line_ranges(&self.path);
        // The collection and the ranges are computed inside the state's update; the set
        // itself is another update, so it happens outside.
        let diff_marks = &mut self.diff_marks;
        let (decs, marks) = self.state.update(cx, |s, cx| {
            if diff_marks.is_none() {
                *diff_marks = Some(s.create_range_decorations_collection(vec![], cx));
            }
            let text = s.text().clone();
            let last_row = text.offset_to_point(text.len()).row as u32;
            let theme = cx.theme().clone();
            let decs: Vec<_> = lines
                .into_iter()
                .filter(|(a, _, _)| *a <= last_row)
                .map(|(a, b, kind)| {
                    let b = b.min(last_row + 1);
                    let start = text.point_to_offset(Point::new(a as usize, 0));
                    let end = if matches!(kind, DiffMark::Deleted) {
                        start + 4.min(text.len().saturating_sub(start))
                    } else {
                        text.point_to_offset(Point::new(b as usize, 0)).max(start)
                    };
                    let color = match kind {
                        DiffMark::Added => theme.success.opacity(0.12),
                        DiffMark::Changed => theme.info.opacity(0.10),
                        DiffMark::Deleted => theme.danger.opacity(0.35),
                    };
                    gpui_kit::base::input::RangeDecoration::new(start..end)
                        .with_style(gpui_kit::base::input::RangeDecorationStyle::Fill)
                        .with_color(color)
                })
                .collect();
            (decs, diff_marks.clone().unwrap())
        });
        marks.set(decs, cx);
    }

    /// Put the caret on a line (deep links point here) and take the focus.
    pub fn goto_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.set_cursor_position(Position::new(line.saturating_sub(1), 0), window, cx));
    }

    /// Take the window's focus (the deferred focus call lands here).
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.focus(window, cx));
    }

    fn save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
    }

    /// Write the buffer back to its file. No-op when clean.
    pub(crate) fn save_now(&mut self, cx: &mut Context<Self>) {
        if !self.dirty {
            return;
        }
        let text = self.state.read(cx).value().to_string();
        match std::fs::write(&self.path, &text) {
            Ok(()) => {
                if let Some((client, uri)) = &self.lsp {
                    client.did_save(uri, &text);
                }
                self.saved = text;
                self.dirty = false;
                cx.notify();
                self.refresh_diff(cx);
            }
            Err(e) => self.workspace.update(cx, |ws, cx| {
                cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't save {}: {e}", self.path.display()).into(), undo: None });
                let _ = ws;
            }),
        }
    }
}

/// Read the file; a reason for read-only otherwise.
fn load(path: &Path) -> (String, Option<String>) {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_FILE_BYTES => return (String::new(), Some("Too big to edit — open it in your editor.".into())),
        Err(e) => return (String::new(), Some(format!("Can't read it: {e}"))),
        _ => {}
    }
    match std::fs::read_to_string(path) {
        Ok(t) => (t, None),
        Err(_) => (String::new(), Some("Not text — open it in another app.".into())),
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        if let Some((client, uri)) = &self.lsp {
            client.did_close(uri);
        }
    }
}

#[cfg(test)]
impl EditorView {
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn text_state(&self) -> Entity<EditorState> {
        self.state.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // ~ for home, since headers are tight.
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        let dir = self.path.parent().map(|p| {
            let p = p.strip_prefix(&home).unwrap_or(p);
            format!("~/{}", p.display()).trim_end_matches('/').to_string()
        }).unwrap_or_default();
        let dirty = self.dirty;
        let path = self.path.clone();

        v_flex()
            .size_full()
            .key_context("TrekEditor")
            .on_action(cx.listener(Self::save))
            .child(
                h_flex()
                    .px_3()
                    .h(px(40.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(Icon::new(crate::assets::Lucide::FilePen).small().text_color(theme.muted_foreground))
                    .child(div().min_w_0().truncate().font_medium().child(name))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(dir))
                    .when(dirty, |el| el.child(div().text_color(palette::ember(cx)).child("●")))
                    .when_some(self.problem.clone(), |el, p| {
                        el.child(div().text_xs().text_color(palette::red(cx)).child(p))
                    })
                    .child(
                        Button::new("editor-save").ghost().small().label("Save").tooltip("Save (⌘S)")
                            .when(!dirty, |b| b.disabled(true))
                            .on_click(cx.listener(|this, _, window, cx| this.save(&SaveFile, window, cx))),
                    )
                    .child(crate::ui::icon_button("editor-open-default", IconName::ExternalLink, "Open in default app").on_click(move |_, _, cx| {
                        cx.open_with_system(&path);
                    })),
            )
            .child(div().flex_1().min_h_0().child(Editor::new(&self.state).appearance(false).bordered(false).h(relative(1.))))
    }
}
