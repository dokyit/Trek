//! Editor: an in-app code surface. A project file opens here editable — syntax-highlighted,
//! line-numbered, searchable — and saves back with ⌘S, so a fix doesn't leave the app.
//! Deep links (`trek://edit?path=…&line=…`) land here from editor extensions.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, TabSize};
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
    _sub: Subscription,
}

impl EditorView {
    pub fn new(workspace: Entity<Workspace>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (text, problem) = load(&path);
        let lang = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
        let saved = text.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .language(lang)
                .line_number(true)
                .searchable(true)
                .tab_size(TabSize { tab_size: 4, hard_tabs: false });
            s.set_value(text, window, cx);
            if problem.is_some() {
                s.set_readonly(true, cx);
            }
            s
        });
        let sub = cx.subscribe(&state, |this, s, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let dirty = s.read(cx).value().as_str() != this.saved.as_str();
                if dirty != this.dirty {
                    this.dirty = dirty;
                    cx.notify();
                }
            }
        });
        Self { workspace, path, state, saved, dirty: false, problem, _sub: sub }
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
                self.saved = text;
                self.dirty = false;
                cx.notify();
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
