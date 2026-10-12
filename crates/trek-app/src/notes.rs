//! Notes (sidebar › Notes): a place to jot things down beside the agents' work. The list of
//! notes on the left; on the right the note, written in markdown with a toolbar (and shortcuts)
//! for bold, italics, underline, strikethrough, headings, bullet, numbered and check lists,
//! quotes, code, text colour and highlights. Write, Split (writing beside how it reads) or
//! Preview. Saved as you type, as markdown files (see `trek_core::notes`).
//!
//! Images go in by paste, by drop, or from the toolbar's picker: each is a file kept in the notes
//! folder's `assets/`, linked from the note as `![name](assets/file)` on a line of its own, and
//! drawn in the preview.

use crate::attachments::{self, Pasted};
use crate::workspace::Workspace;
use anyhow::{Context as _, Result};
use gpui_kit::component::input::{InputEvent, Redo, Textarea, TextareaState, Undo};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use trek_core::notes::{self, Block, Edit, Note};

actions!(notes, [Bold, Italic, Underline, Strike, Bullets, Numbers, Checklist, Heading1, Heading2, Quote, Code, ToggleCheck, NewNote]);

pub(crate) const CONTEXT: &str = "NoteEditor";

/// The notes' shortcuts, from the table in `keys.rs`.
pub fn key_bindings() -> Vec<KeyBinding> {
    crate::keys::bindings(crate::keys::Group::Notes)
}

/// Text colours and highlights a note can use: a name, the text colour, the highlight.
const COLORS: &[(&str, &str, &str)] = &[
    ("Red", "#ef4444", "#ef444440"),
    ("Orange", "#f97316", "#f9731640"),
    ("Yellow", "#eab308", "#facc1550"),
    ("Green", "#22c55e", "#22c55e40"),
    ("Teal", "#14b8a6", "#14b8a640"),
    ("Blue", "#3b82f6", "#3b82f640"),
    ("Purple", "#a855f7", "#a855f740"),
    ("Pink", "#ec4899", "#ec489940"),
];

/// Typing with pauses shorter than this is one step to undo.
const TYPING_BURST: Duration = Duration::from_millis(900);

/// The most steps kept to undo in one note.
const UNDO_STEPS: usize = 300;

/// A note's text and what was selected in it, at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
struct Snapshot {
    text: String,
    selection: Range<usize>,
}

/// What can be undone and redone in the note on screen (`NotesView::undo`). The note's own:
/// opening another starts afresh. Typing is taken a burst at a time; a formatting command, a
/// list carried on by Return, or a paste is a step each.
#[derive(Default)]
struct History {
    /// The note as it stands (as of the last change taken in).
    now: Snapshot,
    /// What it was before each step, oldest first.
    past: Vec<Snapshot>,
    /// What undoing took back, the next to redo last.
    future: Vec<Snapshot>,
    /// When the last change came from typing, while one more would join its step.
    typed: Option<Instant>,
}

impl History {
    fn starting_at(now: Snapshot) -> Self {
        History { now, ..Default::default() }
    }

    /// The note changed to `now`: by `typing` (joined to the burst under way), or in one go.
    fn record(&mut self, now: Snapshot, typing: bool) {
        if now.text == self.now.text {
            // Only the caret moved: an undo from here comes back to it.
            self.now.selection = now.selection;
            return;
        }
        let joins = typing && self.typed.is_some_and(|at| at.elapsed() < TYPING_BURST);
        if !joins {
            self.past.push(self.now.clone());
            if self.past.len() > UNDO_STEPS {
                self.past.remove(0);
            }
        }
        self.future.clear();
        self.typed = typing.then(Instant::now);
        self.now = now;
    }

    /// Step back: what the note goes back to.
    fn undo(&mut self) -> Option<Snapshot> {
        let back = self.past.pop()?;
        self.future.push(std::mem::replace(&mut self.now, back.clone()));
        self.typed = None;
        Some(back)
    }

    /// Step forward again: what an undo took back.
    fn redo(&mut self) -> Option<Snapshot> {
        let on = self.future.pop()?;
        self.past.push(std::mem::replace(&mut self.now, on.clone()));
        self.typed = None;
        Some(on)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Write,
    Split,
    Preview,
}

/// Toolbar widths below which it gives up room (see `NotesView::toolbar`): the view switcher's
/// words, then the formats of tier 2, then those of tier 1.
const TOOLBAR_FULL: f32 = 640.;
const TOOLBAR_SOME: f32 = 520.;
const TOOLBAR_FEW: f32 = 400.;

/// A formatting command on the toolbar. Tier 0 stays at any width; 1 and 2 move into the "More"
/// menu as the toolbar narrows. A divider goes between groups.
struct Format {
    id: &'static str,
    icon: crate::assets::Lucide,
    tip: &'static str,
    act: fn(&mut NotesView, &mut Window, &mut Context<NotesView>),
    tier: u8,
    group: u8,
}

// The tips are written for a Mac; `tool` and the "More formatting" menu localize them.
// keys: localized where used
const FORMATS: [Format; 12] = [
    Format { id: "note-bold", icon: crate::assets::Lucide::Bold, tip: "Bold (⌘B)", act: |t, w, cx| t.wrap("**", "**", w, cx), tier: 0, group: 0 },
    Format { id: "note-italic", icon: crate::assets::Lucide::Italic, tip: "Italic (⌘I)", act: |t, w, cx| t.wrap("*", "*", w, cx), tier: 0, group: 0 },
    Format { id: "note-underline", icon: crate::assets::Lucide::Underline, tip: "Underline (⌘U)", act: |t, w, cx| t.wrap("<u>", "</u>", w, cx), tier: 2, group: 0 },
    Format { id: "note-strike", icon: crate::assets::Lucide::Strikethrough, tip: "Strikethrough (⌘⇧X)", act: |t, w, cx| t.wrap("~~", "~~", w, cx), tier: 2, group: 0 },
    Format { id: "note-h1", icon: crate::assets::Lucide::Heading1, tip: "Heading (⌘⌥1)", act: |t, w, cx| t.block(Block::Heading(1), w, cx), tier: 1, group: 1 },
    Format { id: "note-h2", icon: crate::assets::Lucide::Heading2, tip: "Subheading (⌘⌥2)", act: |t, w, cx| t.block(Block::Heading(2), w, cx), tier: 2, group: 1 },
    Format { id: "note-bullets", icon: crate::assets::Lucide::List, tip: "Bullet list (⌘⇧8)", act: |t, w, cx| t.block(Block::Bullets, w, cx), tier: 1, group: 2 },
    Format { id: "note-numbers", icon: crate::assets::Lucide::ListOrdered, tip: "Numbered list (⌘⇧7)", act: |t, w, cx| t.block(Block::Numbers, w, cx), tier: 2, group: 2 },
    Format { id: "note-checks", icon: crate::assets::Lucide::ListTodo, tip: "Checklist (⌘⇧9) · tick with ⌘↩", act: |t, w, cx| t.block(Block::Checklist, w, cx), tier: 1, group: 2 },
    Format { id: "note-quote", icon: crate::assets::Lucide::Quote, tip: "Quote (⌘⇧.)", act: |t, w, cx| t.block(Block::Quote, w, cx), tier: 2, group: 2 },
    Format { id: "note-code", icon: crate::assets::Lucide::Code, tip: "Code (⌘E)", act: |t, w, cx| t.wrap("`", "`", w, cx), tier: 2, group: 2 },
    Format { id: "note-image", icon: crate::assets::Lucide::Image, tip: "Add an image (or paste or drop one)", act: |t, w, cx| t.pick_images(w, cx), tier: 1, group: 3 },
];
// keys: end

pub struct NotesView {
    workspace: Entity<Workspace>,
    notes: Vec<Note>,
    current: Option<String>,
    editor: Entity<TextareaState>,
    mode: Mode,
    colors_open: bool,
    /// The toolbar's width as last laid out (0 before): it gives up room below `TOOLBAR_*`.
    toolbar_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The preview's width as last laid out (0 before): images in it are drawn no wider.
    preview_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The note's text changed and isn't saved yet.
    dirty: bool,
    /// Undo and redo for the note on screen.
    history: History,
    /// The workspace's `notes_epoch` the list was read at.
    epoch: u64,
    _save: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl NotesView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Jot something down… Markdown works")
                // Return is ours: it carries a list on (see `newline`); ⇧Return is a plain new line.
                .submit_on_enter(true)
        });
        // What's typed in the moment before quitting is kept.
        cx.on_app_quit(|this: &mut Self, _| {
            this.flush();
            async {}
        })
        .detach();
        // And when the main window goes, which takes this view and its pending save with it: a
        // closed main window on macOS, or any quit on Windows, which closes the windows first.
        cx.on_release(|this: &mut Self, _| this.flush()).detach();
        let mut subscriptions = vec![cx.subscribe_in(&editor, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let (body, selection) = Self::read(state, cx);
                this.history.record(Snapshot { text: body.clone(), selection }, true);
                this.changed(body, cx);
            }
            InputEvent::PressEnter { shift: false, .. } => {
                let (text, sel) = Self::read(state, cx);
                this.apply(notes::newline(&text, sel), window, cx);
            }
            _ => {}
        })];
        // A phone changed the notes: read them again.
        subscriptions.push(cx.observe_in(&workspace, window, |this: &mut Self, ws, window, cx| {
            let epoch = ws.read(cx).notes_epoch;
            if epoch != this.epoch {
                this.epoch = epoch;
                this.reload(window, cx);
            }
        }));
        let epoch = workspace.read(cx).notes_epoch;
        let mut this = Self {
            workspace,
            notes: notes::list_in(&notes::notes_dir()),
            current: None,
            editor,
            mode: Mode::Split,
            colors_open: false,
            toolbar_width: Default::default(),
            preview_width: Default::default(),
            dirty: false,
            history: History::default(),
            epoch,
            _save: None,
            _subscriptions: subscriptions,
        };
        if let Some(first) = this.notes.first().map(|n| n.id.clone()) {
            this.open(&first, window, cx);
        }
        this
    }

    /// Where the keys go on the Notes screen: the note's text.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.focus_handle(cx);
        handle.focus(window, cx);
    }

    fn note(&self) -> Option<&Note> {
        self.notes.iter().find(|n| Some(&n.id) == self.current.as_ref())
    }

    fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.flush();
        self.current = Some(id.to_string());
        let body = self.note().map(|n| n.body.clone()).unwrap_or_default();
        self.editor.update(cx, |s, cx| {
            s.set_value(body.clone(), window, cx);
            s.set_selected_range(body.len()..body.len(), cx);
        });
        // Another note (or this one as the phone left it): nothing of the last is undone into it.
        self.history = History::starting_at(Snapshot { selection: body.len()..body.len(), text: body });
        self.dirty = false;
        cx.notify();
    }

    /// Read the notes again (a phone changed them). What's being typed here wins: a note with
    /// unsaved changes keeps them, and is saved over the phone's as it would have been.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            return;
        }
        self.notes = notes::list_in(&notes::notes_dir());
        match self.current.clone().filter(|id| self.notes.iter().any(|n| &n.id == id)).or_else(|| self.notes.first().map(|n| n.id.clone())) {
            Some(id) => {
                let body = self.notes.iter().find(|n| n.id == id).map(|n| n.body.clone()).unwrap_or_default();
                if self.current.as_deref() != Some(id.as_str()) || self.editor.read(cx).value() != body.as_str() {
                    self.open(&id, window, cx);
                }
            }
            None => {
                self.current = None;
                self.editor.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An empty note on top is reused rather than stacked.
        if let Some(empty) = self.notes.iter().find(|n| n.body.trim().is_empty()).map(|n| n.id.clone()) {
            self.open(&empty, window, cx);
        } else {
            match notes::create_in(&notes::notes_dir()) {
                Ok(note) => {
                    let id = note.id.clone();
                    self.notes.insert(0, note);
                    self.open(&id, window, cx);
                }
                Err(e) => self.workspace.update(cx, |_, cx| {
                    cx.emit(crate::workspace::WorkspaceEvent::Toast { message: format!("Couldn't make a note: {e}"), undo: None })
                }),
            }
        }
        if self.mode == Mode::Preview {
            self.mode = Mode::Split;
        }
        self.focus(window, cx);
    }

    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.notes.iter().position(|n| Some(&n.id) == self.current.as_ref()) else { return };
        let note = self.notes.remove(at);
        if let Err(e) = notes::delete(&note) {
            self.notes.insert(at, note);
            self.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: format!("Couldn't delete the note: {e}"), undo: None }));
            return;
        }
        self.dirty = false;
        self.current = None;
        match self.notes.get(at.min(self.notes.len().saturating_sub(1))).map(|n| n.id.clone()) {
            Some(next) => self.open(&next, window, cx),
            None => {
                self.editor.update(cx, |s, cx| s.set_value("", window, cx));
                cx.notify();
            }
        }
        let title = note.title();
        self.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: format!("Deleted “{title}” (kept in the notes folder's Deleted)"), undo: None }));
    }

    /// A new note on top of the list, in front, for what's about to be written in the editor
    /// when no note is open.
    fn start_note(&mut self) -> std::io::Result<()> {
        let note = notes::create_in(&notes::notes_dir())?;
        self.current = Some(note.id.clone());
        self.notes.insert(0, note);
        Ok(())
    }

    /// The note's text changed: keep it, and save it a moment after typing stops. Typing into
    /// no note at all starts one.
    fn changed(&mut self, body: String, cx: &mut Context<Self>) {
        if self.current.is_none() && (body.is_empty() || self.start_note().is_err()) {
            return;
        }
        let Some(note) = self.notes.iter_mut().find(|n| Some(&n.id) == self.current.as_ref()) else { return };
        if note.body == body {
            return;
        }
        note.body = body;
        note.modified = trek_core::store::now_ms();
        self.dirty = true;
        // The note just written in goes to the top.
        if let Some(at) = self.notes.iter().position(|n| Some(&n.id) == self.current.as_ref()).filter(|i| *i > 0) {
            let n = self.notes.remove(at);
            self.notes.insert(0, n);
        }
        self._save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, _| this.flush());
        }));
        cx.notify();
    }

    /// Save the note on screen if it changed.
    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(note) = self.note()
            && let Err(e) = notes::save(note)
        {
            tracing::warn!("note {}: {e}", note.id);
        }
        self.dirty = false;
    }

    fn read(state: &Entity<TextareaState>, cx: &App) -> (String, Range<usize>) {
        let s = state.read(cx);
        (s.value().to_string(), s.selected_range())
    }

    /// Put an edit's text in the editor and select what it says. One step to undo.
    fn apply(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.history.record(Snapshot { text: edit.text.clone(), selection: edit.selection.clone() }, false);
        self.show(Snapshot { text: edit.text, selection: edit.selection }, window, cx);
    }

    /// Put `to` in the editor (its text, and what was selected), and keep it as the note's.
    fn show(&mut self, to: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |s, cx| {
            s.set_value(to.text.clone(), window, cx);
            s.set_selected_range(to.selection.start.min(to.text.len())..to.selection.end.min(to.text.len()), cx);
        });
        self.changed(to.text, cx);
        self.focus(window, cx);
    }

    /// Take back the last step in this note: a burst of typing, or one command.
    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(back) = self.history.undo() {
            self.show(back, window, cx);
        }
    }

    /// Put back what the last undo took.
    fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(on) = self.history.redo() {
            self.show(on, window, cx);
        }
    }

    fn format(&mut self, f: impl FnOnce(&str, Range<usize>) -> Edit, window: &mut Window, cx: &mut Context<Self>) {
        let (text, sel) = Self::read(&self.editor, cx);
        self.apply(f(&text, sel), window, cx);
    }

    fn wrap(&mut self, open: &'static str, close: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.format(|t, s| notes::wrap(t, s, open, close), window, cx);
    }

    fn block(&mut self, block: Block, window: &mut Window, cx: &mut Context<Self>) {
        self.format(|t, s| notes::toggle_block(t, s, block), window, cx);
    }

    fn toggle_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, sel) = Self::read(&self.editor, cx);
        let line = text[..sel.start.min(text.len())].matches('\n').count();
        if let Some(new) = notes::toggle_check(&text, line) {
            self.apply(Edit { selection: sel.clone(), text: new }, window, cx);
        }
    }

    fn toast(&self, message: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message, undo: None }));
    }

    /// The toolbar's Image button: pick image files to add at the caret.
    fn pick_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Add".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| this.add_files(paths, true, window, cx));
            }
        })
        .detach();
    }

    /// Image files (dropped or picked) go into the note; any other file is left out. `say`:
    /// tell the user when none of them was an image (a pick can't be limited to images).
    fn add_files(&mut self, paths: Vec<PathBuf>, say: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (images, _) = attachments::split_files(paths);
        if images.is_empty() && say {
            self.toast("That isn't an image: notes take PNG, JPEG, GIF, WebP, HEIC, TIFF and BMP.".into(), cx);
        }
        self.add_images(images.into_iter().map(Source::File).collect(), window, cx);
    }

    /// A paste the editor handed over (see `attachments::pasted`): images go into the note. Other
    /// files copied along with them are left out, as a drop leaves them.
    fn paste_images(&mut self, pasted: Pasted, window: &mut Window, cx: &mut Context<Self>) {
        let sources = match pasted {
            Pasted::Files { images, .. } => images.into_iter().map(Source::File).collect(),
            Pasted::Images(images) => images.into_iter().map(Source::Data).collect(),
        };
        self.add_images(sources, window, cx);
    }

    /// Put `sources` in the notes folder's `assets/` off the main thread, then link them from the
    /// note, at the caret, as one step to undo. With no note open, one is started to hold them.
    fn add_images(&mut self, sources: Vec<Source>, window: &mut Window, cx: &mut Context<Self>) {
        if sources.is_empty() {
            return;
        }
        if self.current.is_none()
            && let Err(e) = self.start_note()
        {
            self.toast(format!("Couldn't make a note: {e}"), cx);
            return;
        }
        let Some(note) = self.current.clone() else { return };
        let assets = notes::assets_dir();
        cx.spawn_in(window, async move |this, cx| {
            let (id, dir) = (note.clone(), assets);
            let results = cx.background_executor().spawn(async move { sources.iter().map(|s| (s.label(), bring_in(s, &id, &dir))).collect::<Vec<_>>() }).await;
            let _ = this.update_in(cx, |this, window, cx| this.landed(&note, results, window, cx));
        })
        .detach();
    }

    /// Images are in `assets/`: link the ones that made it from note `id`. If the user has opened
    /// another note meanwhile, they're added at the end of the note they were meant for.
    fn landed(&mut self, id: &str, results: Vec<(String, Result<String>)>, window: &mut Window, cx: &mut Context<Self>) {
        let mut links = Vec::new();
        let mut failed = None;
        for (label, result) in results {
            match result {
                Ok(link) => links.push(link),
                Err(e) => {
                    failed.get_or_insert(format!("Couldn't add {label}: {e}."));
                }
            }
        }
        if let Some(message) = failed {
            self.toast(message, cx);
        }
        if links.is_empty() {
            return;
        }
        let link = links.join("\n");
        if self.current.as_deref() == Some(id) {
            let (text, sel) = Self::read(&self.editor, cx);
            self.apply(notes::insert_image(&text, sel, &link), window, cx);
        } else if let Some(at) = self.notes.iter().position(|n| n.id == id) {
            let mut note = self.notes.remove(at);
            note.body = notes::insert_image(&note.body, note.body.len()..note.body.len(), &link).text;
            note.modified = trek_core::store::now_ms();
            if let Err(e) = notes::save(&note) {
                tracing::warn!("note {}: {e}", note.id);
            }
            self.notes.insert(0, note);
            cx.notify();
        }
    }

    fn tool(&self, id: &'static str, icon: impl Into<Icon>, tip: &'static str, cx: &mut Context<Self>, f: fn(&mut Self, &mut Window, &mut Context<Self>)) -> AnyElement {
        crate::ui::icon_button(id, icon, crate::keys::shared(tip)).on_click(cx.listener(move |this, _, window, cx| f(this, window, cx))).into_any_element()
    }

    fn divider(cx: &App) -> AnyElement {
        div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(cx.theme().foreground.opacity(0.1)).into_any_element()
    }

    fn colors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let me = cx.entity();
        let swatch = |row: &'static str, i: usize, name: &'static str, css: &'static str, background: bool| {
            let me = me.clone();
            let color: Hsla = Rgba::try_from(css).map(Into::into).unwrap_or(theme.foreground);
            div()
                .id(SharedString::from(format!("swatch-{row}-{i}")))
                .size(px(22.))
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(theme.foreground.opacity(0.15))
                .bg(if background { color } else { color.opacity(1.) })
                .when(!background, |el| el.flex().items_center().justify_center().bg(gpui_kit::transparent_black()).child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(color).child("A")))
                .hover(|s| s.border_color(theme.foreground.opacity(0.5)))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(name).build(window, cx))
                .on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| {
                        this.colors_open = false;
                        this.format(|t, s| notes::color(t, s, css, background), window, cx);
                    })
                })
        };
        crate::ui::menu_surface(cx)
            .w(px(250.))
            .p(px(10.))
            .gap(px(8.))
            .child(div().text_xs().text_color(theme.muted_foreground).child("Text colour"))
            .child(h_flex().gap(px(6.)).children(COLORS.iter().enumerate().map(|(i, (n, c, _))| swatch("text", i, n, c, false))))
            .child(div().text_xs().text_color(theme.muted_foreground).child("Highlight"))
            .child(h_flex().gap(px(6.)).children(COLORS.iter().enumerate().map(|(i, (n, _, h))| swatch("mark", i, n, h, true))))
            .into_any_element()
    }

    fn toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let this = cx.entity();
        // Narrow (the tools panel open beside it, a small window), the toolbar gives up room
        // rather than run past its edge: first the view switcher's words (icons, named on hover),
        // then the less-used formats, then the rest, into a "More" menu.
        let width = self.toolbar_width.get();
        let narrow = |below: f32| width > 0. && width < below;
        let tier = if narrow(TOOLBAR_FEW) { 0 } else if narrow(TOOLBAR_SOME) { 1 } else { 2 };
        let shown = |t: u8| t <= tier;
        let colors = Popover::new("note-colors")
            .anchor(Anchor::TopLeft)
            .appearance(false)
            .open(self.colors_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.colors_open = *open;
                cx.notify();
            }))
            .trigger(crate::ui::icon_button("note-color", crate::assets::Lucide::Palette, "Colour and highlight"))
            .content(move |_, _, cx| this.update(cx, |this, cx| this.colors(cx)));
        let mode = self.mode;
        h_flex()
            .id("note-toolbar")
            .test_support()
            .relative()
            .overflow_hidden()
            .flex_none()
            .h(px(44.))
            .px(px(10.))
            .gap(px(1.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.06))
            .child(crate::ui::icon_button("note-undo", crate::assets::Lucide::Undo2, crate::keys::shared("Undo (⌘Z)")).disabled(self.history.past.is_empty()).on_click(cx.listener(|this, _, window, cx| this.undo(window, cx))))
            .child(crate::ui::icon_button("note-redo", crate::assets::Lucide::Redo2, crate::keys::shared("Redo (⇧⌘Z)")).disabled(self.history.future.is_empty()).on_click(cx.listener(|this, _, window, cx| this.redo(window, cx))))
            .children(FORMATS.iter().enumerate().flat_map(|(i, f)| {
                // A divider before each group that has a tool showing.
                let starts_group = i == 0 || FORMATS[i - 1].group != f.group;
                let group_shown = FORMATS.iter().any(|g| g.group == f.group && shown(g.tier));
                let divider = (starts_group && group_shown).then(|| Self::divider(cx));
                let tool = shown(f.tier).then(|| self.tool(f.id, f.icon, f.tip, cx, f.act));
                divider.into_iter().chain(tool)
            }))
            .when(tier < 2, |el| {
                let me = cx.entity().downgrade();
                let hidden: Vec<&'static Format> = FORMATS.iter().filter(|f| !shown(f.tier)).collect();
                el.child(crate::ui::icon_button("note-more", gpui_kit::component::IconName::Ellipsis, "More formatting").dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                    menu = menu.min_w(px(200.));
                    for f in &hidden {
                        let (me, act) = (me.clone(), f.act);
                        menu = menu.item(PopupMenuItem::new(crate::keys::shared(f.tip)).icon(Icon::new(f.icon)).on_click(move |_, window, cx| {
                            let _ = me.update(cx, |this, cx| act(this, window, cx));
                        }));
                    }
                    menu
                }))
            })
            .child(Self::divider(cx))
            .child(colors)
            .child(div().flex_1().min_w(px(6.)))
            .child(self.modes(mode, width > 0. && width < TOOLBAR_FULL, cx))
            .when(self.current.is_some(), |el| {
                el.child(div().w(px(6.))).child(
                    div().id("note-delete-zone").test_support().child(self.tool("note-delete", crate::assets::Lucide::Trash, "Delete note", cx, |t, w, cx| t.delete_current(w, cx))),
                )
            })
            .child({
                // Measured after layout: a width that crosses a step draws the toolbar again.
                let cell = self.toolbar_width.clone();
                canvas(
                    move |bounds, window, _| {
                        let w = f32::from(bounds.size.width);
                        let step = |w: f32| [TOOLBAR_FULL, TOOLBAR_SOME, TOOLBAR_FEW].iter().filter(|b| w > 0. && w < **b).count();
                        let before = cell.replace(w);
                        if step(before) != step(w) {
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            })
            .into_any_element()
    }

    /// Write / Split / Preview. Short of room, icons named on hover instead of words.
    fn modes(&self, mode: Mode, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity();
        let pick = move |m: Mode, window: &mut Window, cx: &mut App| {
            me.update(cx, |this, cx| {
                this.mode = m;
                if m != Mode::Preview {
                    this.focus(window, cx);
                }
                cx.notify();
            })
        };
        if !compact {
            return crate::ui::segmented("note-mode", vec![(Mode::Write, "Write"), (Mode::Split, "Split"), (Mode::Preview, "Preview")], mode, pick, cx);
        }
        let theme = cx.theme().clone();
        let options: [(Mode, Icon, &'static str); 3] = [
            (Mode::Write, Icon::new(crate::assets::Lucide::Pencil), "Write"),
            (Mode::Split, Icon::new(gpui_kit::component::IconName::PanelRight), "Split: writing beside how it reads"),
            (Mode::Preview, Icon::new(gpui_kit::component::IconName::Eye), "Preview"),
        ];
        h_flex()
            .flex_none()
            .h(px(30.))
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(theme.foreground.opacity(0.05))
            .border_1()
            .border_color(theme.foreground.opacity(0.06))
            .children(options.into_iter().enumerate().map(|(i, (value, icon, tip))| {
                let selected = value == mode;
                let pick = pick.clone();
                div()
                    .id(("note-mode", i))
                    .test_support()
                    .h_full()
                    .w(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.foreground.opacity(0.12)))
                    .child(icon.small().text_color(if selected { theme.foreground } else { theme.muted_foreground }))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
                    .on_click(move |_, window, cx| pick(value, window, cx))
            }))
            .into_any_element()
    }

    fn list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .w(px(250.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.foreground.opacity(0.06))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(44.))
                    .pl(px(16.))
                    .pr(px(8.))
                    .border_b_1()
                    .border_color(theme.foreground.opacity(0.06))
                    .child(div().flex_1().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child("Notes"))
                    .child(crate::ui::icon_button("note-new", crate::assets::Lucide::SquarePen, crate::keys::shared("New note (⌘N)")).on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx)))),
            )
            .child(
                v_flex()
                    .id("note-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(6.))
                    .gap(px(2.))
                    .when(self.notes.is_empty(), |el| {
                        el.child(div().p(px(12.)).text_xs().text_color(theme.muted_foreground).child(crate::keys::shared("No notes yet. Start typing, or press ⌘N.")))
                    })
                    .children(self.notes.iter().map(|n| {
                        let active = Some(&n.id) == self.current.as_ref();
                        let id = n.id.clone();
                        let preview = n.preview();
                        v_flex()
                            .id(SharedString::from(format!("note-{}", n.id)))
                            .test_support()
                            .px(px(10.))
                            .py(px(8.))
                            .gap(px(2.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(active, |el| el.bg(theme.foreground.opacity(0.075)))
                            .when(!active, |el| el.hover(|s| s.bg(theme.foreground.opacity(0.04))))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(n.title()))
                                    .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(crate::time::relative(n.modified))),
                            )
                            .when(!preview.is_empty(), |el| el.child(div().truncate().text_xs().text_color(theme.muted_foreground).child(preview)))
                            .on_click(cx.listener(move |this, _, window, cx| this.open(&id, window, cx)))
                    })),
            )
            .into_any_element()
    }
}

impl Render for NotesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let size = px(self.workspace.read(cx).settings.appearance.transcript_font_size());
        let body = self.note().map(|n| n.body.clone()).unwrap_or_default();
        let id = self.current.clone().unwrap_or_default();
        let write = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(28.))
            .py(px(20.))
            .text_size(size)
            .line_height(relative(1.6))
            .child(
                Textarea::new(&self.editor)
                    .appearance(false)
                    // A screenshot (or an image file) on the clipboard goes into the note as an image;
                    // anything else pastes as text, as before.
                    .on_paste({
                        let me = cx.entity().downgrade();
                        move |item, window, cx| match attachments::pasted(item) {
                            Some(p) => me.update(cx, |this, cx| this.paste_images(p, window, cx)).is_ok(),
                            None => false,
                        }
                    })
                    .h_full(),
            );
        // The notes folder is where `assets/…` links start from: the images in them are drawn,
        // and open in the image preview when clicked. Beside the text (Split) the column is
        // narrower than an image is drawn at: its width, measured after layout, bounds them (a
        // width that changes draws the preview again, as the toolbar's does).
        const PREVIEW_PAD: f32 = 32.;
        let measured = self.preview_width.get();
        let images_within = (measured > 0.).then(|| px(measured - 2. * PREVIEW_PAD));
        let preview_width = self.preview_width.clone();
        let preview = div()
            .id("note-preview")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(PREVIEW_PAD))
            .py(px(22.))
            .child(
                canvas(
                    move |bounds, window, _| {
                        let w = f32::from(bounds.size.width);
                        if (preview_width.replace(w) - w).abs() > 1. {
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_0(),
            )
            .child(if body.trim().is_empty() {
                div().text_color(theme.muted_foreground).text_size(size).child("Nothing to show yet.").into_any_element()
            } else {
                crate::md::keyed(SharedString::from(format!("note-md-{id}")), body, Some(notes::notes_dir()), None, size, false, false, images_within, cx).into_any_element()
            });
        let drop_tint = crate::palette::ember(cx).opacity(0.08);
        let main = match self.mode {
            Mode::Write => write.into_any_element(),
            Mode::Preview => preview.into_any_element(),
            Mode::Split => h_flex()
                .flex_1()
                .min_h_0()
                .size_full()
                .child(write)
                .child(div().w(px(1.)).h_full().bg(theme.foreground.opacity(0.06)))
                .child(preview)
                .into_any_element(),
        };
        h_flex()
            .id("notes")
            .test_support()
            .key_context("Notes")
            .size_full()
            .on_action(cx.listener(|this, _: &NewNote, window, cx| this.new_note(window, cx)))
            .child(self.list(cx))
            .child(
                v_flex()
                    .id("note-pane")
                    .test_support()
                    .key_context(CONTEXT)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    // Image files dropped anywhere on the note go into it; other files are left.
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_tint))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.add_files(paths.paths().to_vec(), false, window, cx)))
                    // Before the text box's own undo gets them: that one knows nothing of what
                    // the toolbar did, and the two together would lose steps.
                    .capture_action(cx.listener(|t, _: &Undo, w, cx| {
                        cx.stop_propagation();
                        t.undo(w, cx);
                    }))
                    .capture_action(cx.listener(|t, _: &Redo, w, cx| {
                        cx.stop_propagation();
                        t.redo(w, cx);
                    }))
                    .on_action(cx.listener(|t, _: &Bold, w, cx| t.wrap("**", "**", w, cx)))
                    .on_action(cx.listener(|t, _: &Italic, w, cx| t.wrap("*", "*", w, cx)))
                    .on_action(cx.listener(|t, _: &Underline, w, cx| t.wrap("<u>", "</u>", w, cx)))
                    .on_action(cx.listener(|t, _: &Strike, w, cx| t.wrap("~~", "~~", w, cx)))
                    .on_action(cx.listener(|t, _: &Code, w, cx| t.wrap("`", "`", w, cx)))
                    .on_action(cx.listener(|t, _: &Bullets, w, cx| t.block(Block::Bullets, w, cx)))
                    .on_action(cx.listener(|t, _: &Numbers, w, cx| t.block(Block::Numbers, w, cx)))
                    .on_action(cx.listener(|t, _: &Checklist, w, cx| t.block(Block::Checklist, w, cx)))
                    .on_action(cx.listener(|t, _: &Heading1, w, cx| t.block(Block::Heading(1), w, cx)))
                    .on_action(cx.listener(|t, _: &Heading2, w, cx| t.block(Block::Heading(2), w, cx)))
                    .on_action(cx.listener(|t, _: &Quote, w, cx| t.block(Block::Quote, w, cx)))
                    .on_action(cx.listener(|t, _: &ToggleCheck, w, cx| t.toggle_check(w, cx)))
                    .child(self.toolbar(cx))
                    .child(div().flex_1().min_h_0().flex().child(main)),
            )
    }
}

/// An image on its way into a note.
pub(crate) enum Source {
    /// Image data off the clipboard (a screenshot).
    Data(Image),
    /// An image file, dropped, picked or copied. It's copied: the original stays where it was.
    File(PathBuf),
}

impl Source {
    /// How a failure names it.
    fn label(&self) -> String {
        match self {
            Source::Data(_) => "the pasted image".into(),
            Source::File(path) => path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string()),
        }
    }
}

/// `name` as the stem of a file a markdown link can point at without escaping: letters, digits,
/// `-` and `_`, the rest becoming `-`.
pub(crate) fn file_safe(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' }).collect();
    let stem: String = cleaned.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-").chars().take(60).collect();
    if stem.is_empty() { "image".into() } else { stem }
}

/// `<stem>.<ext>` in `dir`, or `<stem>-2.<ext>`… if that's taken: an image never replaces another.
pub(crate) fn unused_name(dir: &Path, stem: &str, ext: &str) -> String {
    (1usize..)
        .map(|n| if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") })
        .find(|name| !dir.join(name).exists())
        .unwrap_or_else(|| format!("{stem}.{ext}"))
}

/// Make a PNG of `path` at `to`, for an image agents and the preview can't read as it is: with
/// the same conversion the composer uses (macOS's `sips`, which also reads HEIC), else, where
/// there is none, with the image crate (TIFF and BMP).
fn convert_to(path: &Path, to: &Path, assets: &Path) -> Result<()> {
    match attachments::convert_file(path, assets) {
        Ok(made) => std::fs::rename(&made, to)
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&made);
            })
            .with_context(|| format!("{} couldn't be written", to.display())),
        Err(e) => {
            let decoded = image::open(path).map_err(|_| e)?;
            decoded.save_with_format(to, image::ImageFormat::Png).context("it couldn't be converted to PNG")
        }
    }
}

/// Put `source` in `assets` as an image of note `note`, named `<note id>-<timestamp>-<name>.<ext>`
/// so a note's images can be found by its id, and give the markdown that shows it:
/// `![name](assets/file)`, written with `/` on every platform. Nothing is written outside `assets`.
pub(crate) fn bring_in(source: &Source, note: &str, assets: &Path) -> Result<String> {
    std::fs::create_dir_all(assets).with_context(|| format!("{} couldn't be created", assets.display()))?;
    let id = file_safe(note);
    let (alt, name) = match source {
        Source::Data(image) => {
            // Saved as the composer saves a pasted image (the same formats, the same conversions),
            // then named for its note.
            let saved = attachments::save_image(image, assets)?;
            let name = format!("{id}-{}", saved.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            std::fs::rename(&saved, assets.join(&name)).with_context(|| format!("{} couldn't be written", assets.join(&name).display()))?;
            ("Pasted image".to_string(), name)
        }
        Source::File(path) => {
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let convert = attachments::needs_conversion(path);
            let ext = if convert { "png".to_string() } else { path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default() };
            let name = unused_name(assets, &format!("{id}-{}-{}", attachments::timestamp(), file_safe(&stem)), &ext);
            let to = assets.join(&name);
            if convert {
                convert_to(path, &to, assets)?;
            } else {
                std::fs::copy(path, &to).with_context(|| format!("it couldn't be copied to {}", to.display()))?;
            }
            let alt = stem.replace(['[', ']', '\r', '\n'], " ").trim().to_string();
            (if alt.is_empty() { "Image".to_string() } else { alt }, name)
        }
    };
    Ok(format!("![{alt}](assets/{name})"))
}

#[cfg(test)]
mod tests {
    use super::{History, Snapshot, UNDO_STEPS};

    fn at(text: &str) -> Snapshot {
        Snapshot { text: text.into(), selection: text.len()..text.len() }
    }

    #[test]
    fn typing_undoes_a_burst_at_a_time_and_a_command_by_itself() {
        let mut h = History::starting_at(at(""));
        // Keys in quick succession are one step.
        for text in ["m", "mi", "mil", "milk"] {
            h.record(at(text), true);
        }
        assert_eq!(h.past.len(), 1);
        // A command (bold, a list, a colour) is a step of its own, and ends the burst.
        h.record(at("**milk**"), false);
        h.record(at("**milk** and"), true);
        assert_eq!(h.past.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["", "milk", "**milk**"]);
        // Back through them, and forward again.
        assert_eq!(h.undo().map(|s| s.text).as_deref(), Some("**milk**"));
        assert_eq!(h.undo().map(|s| s.text).as_deref(), Some("milk"));
        assert_eq!(h.redo().map(|s| s.text).as_deref(), Some("**milk**"));
        assert_eq!(h.undo().map(|s| s.text).as_deref(), Some("milk"));
        assert_eq!(h.undo().map(|s| s.text).as_deref(), Some(""));
        assert_eq!(h.undo(), None, "nothing before the note as it was opened");
        // Something new after an undo: what was taken back can't be redone any more.
        assert_eq!(h.redo().map(|s| s.text).as_deref(), Some("milk"));
        h.record(at("milk, eggs"), true);
        assert_eq!(h.redo(), None);
        assert_eq!(h.undo().map(|s| s.text).as_deref(), Some("milk"));
    }

    #[test]
    fn the_caret_moving_is_no_step_but_an_undo_comes_back_to_it() {
        let mut h = History::starting_at(at("one two"));
        h.record(Snapshot { text: "one two".into(), selection: 0..3 }, true);
        assert!(h.past.is_empty());
        h.record(at("**one** two"), false);
        assert_eq!(h.undo(), Some(Snapshot { text: "one two".into(), selection: 0..3 }));
    }

    #[test]
    fn only_so_many_steps_are_kept() {
        let mut h = History::starting_at(at("0"));
        for n in 1..=UNDO_STEPS + 20 {
            h.record(at(&n.to_string()), false);
        }
        assert_eq!(h.past.len(), UNDO_STEPS);
        assert_eq!(h.past[0].text, "20");
    }
}
