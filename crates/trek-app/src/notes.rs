//! Notes (sidebar › Notes): a place to jot things down beside the agents' work. The list of
//! notes on the left; on the right the note, written in markdown with a toolbar (and shortcuts)
//! for bold, italics, underline, strikethrough, headings, bullet, numbered and check lists,
//! quotes, code, text colour and highlights. Write, Split (writing beside how it reads) or
//! Preview. Saved as you type, as markdown files (see `trek_core::notes`).

use crate::workspace::Workspace;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;
use std::time::Duration;
use trek_core::notes::{self, Block, Edit, Note};

actions!(notes, [Bold, Italic, Underline, Strike, Bullets, Numbers, Checklist, Heading1, Heading2, Quote, Code, ToggleCheck, NewNote]);

const CONTEXT: &str = "NoteEditor";

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-b", Bold, Some(CONTEXT)),
        KeyBinding::new("cmd-i", Italic, Some(CONTEXT)),
        KeyBinding::new("cmd-u", Underline, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-x", Strike, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-8", Bullets, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-7", Numbers, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-9", Checklist, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-1", Heading1, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-2", Heading2, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-.", Quote, Some(CONTEXT)),
        KeyBinding::new("cmd-e", Code, Some(CONTEXT)),
        KeyBinding::new("cmd-enter", ToggleCheck, Some(CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some("Notes")),
    ]
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Write,
    Split,
    Preview,
}

pub struct NotesView {
    workspace: Entity<Workspace>,
    notes: Vec<Note>,
    current: Option<String>,
    editor: Entity<TextareaState>,
    mode: Mode,
    colors_open: bool,
    /// The note's text changed and isn't saved yet.
    dirty: bool,
    _save: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl NotesView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Jot something down… Markdown works: **bold**, *italic*, - lists, - [ ] to-dos")
                // Return is ours: it carries a list on (see `newline`); ⇧Return is a plain new line.
                .submit_on_enter(true)
        });
        // What's typed in the moment before quitting is kept.
        cx.on_app_quit(|this: &mut Self, _| {
            this.flush();
            async {}
        })
        .detach();
        let subscriptions = vec![cx.subscribe_in(&editor, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let body = state.read(cx).value().to_string();
                this.changed(body, cx);
            }
            InputEvent::PressEnter { shift: false, .. } => {
                let (text, sel) = Self::read(state, cx);
                this.apply(notes::newline(&text, sel), window, cx);
            }
            _ => {}
        })];
        let mut this = Self {
            workspace,
            notes: notes::list_in(&notes::notes_dir()),
            current: None,
            editor,
            mode: Mode::Split,
            colors_open: false,
            dirty: false,
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
        self.dirty = false;
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

    /// The note's text changed: keep it, and save it a moment after typing stops. Typing into
    /// no note at all starts one.
    fn changed(&mut self, body: String, cx: &mut Context<Self>) {
        if self.current.is_none() {
            if body.is_empty() {
                return;
            }
            match notes::create_in(&notes::notes_dir()) {
                Ok(note) => {
                    self.current = Some(note.id.clone());
                    self.notes.insert(0, note);
                }
                Err(_) => return,
            }
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

    /// Put an edit's text in the editor and select what it says.
    fn apply(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |s, cx| {
            s.set_value(edit.text.clone(), window, cx);
            s.set_selected_range(edit.selection.clone(), cx);
        });
        self.changed(edit.text, cx);
        self.focus(window, cx);
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

    fn tool(&self, id: &'static str, icon: impl Into<Icon>, tip: &'static str, cx: &mut Context<Self>, f: fn(&mut Self, &mut Window, &mut Context<Self>)) -> AnyElement {
        crate::ui::icon_button(id, icon, tip).on_click(cx.listener(move |this, _, window, cx| f(this, window, cx))).into_any_element()
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
        let me = cx.entity();
        h_flex()
            .id("note-toolbar")
            .test_support()
            .flex_none()
            .h(px(44.))
            .px(px(10.))
            .gap(px(1.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.06))
            .child(self.tool("note-bold", crate::assets::Lucide::Bold, "Bold (⌘B)", cx, |t, w, cx| t.wrap("**", "**", w, cx)))
            .child(self.tool("note-italic", crate::assets::Lucide::Italic, "Italic (⌘I)", cx, |t, w, cx| t.wrap("*", "*", w, cx)))
            .child(self.tool("note-underline", crate::assets::Lucide::Underline, "Underline (⌘U)", cx, |t, w, cx| t.wrap("<u>", "</u>", w, cx)))
            .child(self.tool("note-strike", crate::assets::Lucide::Strikethrough, "Strikethrough (⌘⇧X)", cx, |t, w, cx| t.wrap("~~", "~~", w, cx)))
            .child(Self::divider(cx))
            .child(self.tool("note-h1", crate::assets::Lucide::Heading1, "Heading (⌘⌥1)", cx, |t, w, cx| t.block(Block::Heading(1), w, cx)))
            .child(self.tool("note-h2", crate::assets::Lucide::Heading2, "Subheading (⌘⌥2)", cx, |t, w, cx| t.block(Block::Heading(2), w, cx)))
            .child(Self::divider(cx))
            .child(self.tool("note-bullets", crate::assets::Lucide::List, "Bullet list (⌘⇧8)", cx, |t, w, cx| t.block(Block::Bullets, w, cx)))
            .child(self.tool("note-numbers", crate::assets::Lucide::ListOrdered, "Numbered list (⌘⇧7)", cx, |t, w, cx| t.block(Block::Numbers, w, cx)))
            .child(self.tool("note-checks", crate::assets::Lucide::ListTodo, "Checklist (⌘⇧9) · tick with ⌘↩", cx, |t, w, cx| t.block(Block::Checklist, w, cx)))
            .child(self.tool("note-quote", crate::assets::Lucide::Quote, "Quote (⌘⇧.)", cx, |t, w, cx| t.block(Block::Quote, w, cx)))
            .child(self.tool("note-code", crate::assets::Lucide::Code, "Code (⌘E)", cx, |t, w, cx| t.wrap("`", "`", w, cx)))
            .child(Self::divider(cx))
            .child(colors)
            .child(div().flex_1())
            .child(crate::ui::segmented(
                "note-mode",
                vec![(Mode::Write, "Write"), (Mode::Split, "Split"), (Mode::Preview, "Preview")],
                mode,
                move |m, window, cx| {
                    me.update(cx, |this, cx| {
                        this.mode = m;
                        if m != Mode::Preview {
                            this.focus(window, cx);
                        }
                        cx.notify();
                    })
                },
                cx,
            ))
            .when(self.current.is_some(), |el| {
                el.child(div().w(px(6.))).child(
                    div().id("note-delete-zone").test_support().child(self.tool("note-delete", crate::assets::Lucide::Trash, "Delete note", cx, |t, w, cx| t.delete_current(w, cx))),
                )
            })
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
                    .child(crate::ui::icon_button("note-new", crate::assets::Lucide::SquarePen, "New note (⌘N)").on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx)))),
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
                        el.child(div().p(px(12.)).text_xs().text_color(theme.muted_foreground).child("No notes yet. Start typing, or press ⌘N."))
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
            .child(Textarea::new(&self.editor).appearance(false).h_full());
        let preview = div().id("note-preview").flex_1().min_w_0().h_full().overflow_y_scroll().px(px(32.)).py(px(22.)).child(if body.trim().is_empty() {
            div().text_color(theme.muted_foreground).text_size(size).child("Nothing to show yet.").into_any_element()
        } else {
            crate::md::keyed(SharedString::from(format!("note-md-{id}")), body, None, None, size, cx).into_any_element()
        });
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
                    .key_context(CONTEXT)
                    .flex_1()
                    .min_w_0()
                    .h_full()
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
