//! The editor group: tabs over the open files, breadcrumbs, and the editor in front.
//!
//! A single click in the Explorer opens the *preview* tab (italic), which the next preview
//! replaces; editing it or double-clicking keeps it. A tab with unsaved edits shows a dot where
//! its × goes, and closing it asks first (Save, Don't Save, Cancel).

use super::IdeWorkbench;
use super::diff_view::{DiffSource, DiffView};
use crate::editor::EditorView;
use crate::keys::{self, Id};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};

const TAB_HEIGHT: f32 = 34.;

#[derive(Clone)]
pub struct EditorTab {
    pub view: TabView,
    /// The preview tab: replaced by the next preview until it's edited or kept.
    pub preview: bool,
    /// Redraws the tab row when the file's dirty state changes (and keeps an edited preview).
    _watch: std::rc::Rc<Subscription>,
}

/// What a tab shows: a file in the editor, or a diff (a Review, Source Control's changes).
#[derive(Clone, PartialEq)]
pub enum TabView {
    Editor(Entity<EditorView>),
    Diff(Entity<DiffView>),
}

impl EditorTab {
    /// The file's editor, when this tab is one.
    pub fn editor(&self) -> Option<&Entity<EditorView>> {
        match &self.view {
            TabView::Editor(e) => Some(e),
            TabView::Diff(_) => None,
        }
    }

    pub fn diff(&self) -> Option<&Entity<DiffView>> {
        match &self.view {
            TabView::Diff(d) => Some(d),
            TabView::Editor(_) => None,
        }
    }

    pub fn id(&self) -> EntityId {
        match &self.view {
            TabView::Editor(e) => e.entity_id(),
            TabView::Diff(d) => d.entity_id(),
        }
    }

    /// Edits not saved yet (a diff has none).
    pub fn dirty(&self, cx: &App) -> bool {
        self.editor().is_some_and(|e| e.read(cx).dirty)
    }

    /// The editor's file (a diff tab has none of its own).
    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        self.editor().filter(|e| !e.read(cx).scratch).map(|e| e.read(cx).path.clone())
    }

    /// What the tab says.
    fn title(&self, cx: &App) -> String {
        match &self.view {
            TabView::Editor(e) => file_name(&e.read(cx).path),
            TabView::Diff(d) => d.read(cx).title(cx),
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        match &self.view {
            TabView::Editor(e) => e.update(cx, |e, cx| e.focus(window, cx)),
            TabView::Diff(d) => {
                let handle = d.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
        }
    }
}

impl IdeWorkbench {
    /// The tab for `path`, made if there's none: in place of the preview tab when `preview`,
    /// else after the one in front. Returns its index.
    pub(super) fn open_tab(&mut self, path: PathBuf, preview: bool, window: &mut Window, cx: &mut Context<Self>) -> usize {
        if let Some(ix) = self.tabs.iter().position(|t| t.editor().is_some_and(|e| e.read(cx).path == path)) {
            // Opened on purpose (double click, ⌘P, a link): it stays.
            if !preview {
                self.tabs[ix].preview = false;
            }
            return ix;
        }
        let view = cx.new(|cx| EditorView::new(self.workspace.clone(), path, window, cx));
        let watch = cx.observe(&view, |this, view, cx| {
            if view.read(cx).dirty {
                if let Some(t) = this.tabs.iter_mut().find(|t| t.editor() == Some(&view) && t.preview) {
                    t.preview = false;
                }
            }
            cx.notify();
        });
        let tab = EditorTab { view: TabView::Editor(view), preview, _watch: std::rc::Rc::new(watch) };
        self.place(tab, preview, cx)
    }

    /// Put `tab` in place of the preview tab when `preview` (and there's a clean one), else
    /// after the one in front. Returns its index.
    fn place(&mut self, tab: EditorTab, preview: bool, cx: &mut Context<Self>) -> usize {
        let replace = preview.then(|| self.tabs.iter().position(|t| t.preview && !t.dirty(cx))).flatten();
        match replace {
            Some(ix) => {
                let old = std::mem::replace(&mut self.tabs[ix], tab);
                self.forget_file(&old, cx);
                ix
            }
            None => {
                let at = if self.tabs.is_empty() { 0 } else { (self.active + 1).min(self.tabs.len()) };
                self.tabs.insert(at, tab);
                at
            }
        }
    }

    /// Bring tab `ix` to the front. Its file goes with the AI side bar's messages.
    pub(super) fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        self.active = ix;
        let id = tab.id();
        self.mru.retain(|m| *m != id);
        self.mru.push(id);
        let editor = tab.editor().cloned();
        let file = tab.path(cx);
        self.ai.update(cx, |a, cx| a.set_current_file(file, cx));
        self.status.update(cx, |s, cx| s.set_editor(editor, cx));
        cx.notify();
    }

    /// Show `text` (a patch) in a read-only plain text tab named `title`: the one already
    /// there, or a new one (a diff's "Open as patch").
    pub fn open_diff(&mut self, title: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self.tabs.iter().position(|t| t.editor().is_some_and(|e| e.read(cx).scratch && e.read(cx).path.to_string_lossy() == title));
        let ix = match existing {
            Some(ix) => {
                if let Some(e) = self.tabs[ix].editor().cloned() {
                    e.update(cx, |e, cx| e.set_scratch_text(&text, window, cx));
                }
                ix
            }
            None => {
                let view = cx.new(|cx| EditorView::scratch(self.workspace.clone(), &title, &text, "diff", window, cx));
                let watch = cx.observe(&view, |_, _, cx| cx.notify());
                self.place(EditorTab { view: TabView::Editor(view), preview: false, _watch: std::rc::Rc::new(watch) }, false, cx)
            }
        };
        self.activate(ix, cx);
        cx.notify();
    }

    /// Show `source` in a diff tab: the one already showing it, or a new one (a Source Control
    /// file's diff in the preview tab, as a click in VS Code opens it).
    pub fn open_diff_view(&mut self, source: DiffSource, preview: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<DiffView> {
        let existing = self.tabs.iter().position(|t| t.diff().is_some_and(|d| d.read(cx).source == source));
        let ix = match existing {
            Some(ix) => {
                if !preview {
                    self.tabs[ix].preview = false;
                }
                ix
            }
            None => {
                let me = cx.weak_entity();
                let view = cx.new(|cx| DiffView::new(self.workspace.clone(), me, source, cx));
                let watch = cx.observe(&view, |_, _, cx| cx.notify());
                self.place(EditorTab { view: TabView::Diff(view), preview, _watch: std::rc::Rc::new(watch) }, preview, cx)
            }
        };
        self.activate(ix, cx);
        let view = self.tabs[ix].diff().cloned().expect("a diff tab");
        let handle = view.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        cx.notify();
        view
    }

    /// The Review of `thread`'s pending changes, in a diff tab.
    pub fn open_review(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_diff_view(DiffSource::Review { thread }, false, window, cx);
    }

    /// Diff tabs of files in `top` read their files again (Source Control changed them).
    pub fn reload_diffs(&mut self, cx: &mut Context<Self>) {
        for d in self.tabs.iter().filter_map(|t| t.diff().cloned()).collect::<Vec<_>>() {
            d.update(cx, |d, cx| d.reload(cx));
        }
    }

    /// ⌃Tab and ⌃⇧Tab in the editor: the next or previous tab.
    pub fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let ix = (self.active as isize + step).rem_euclid(self.tabs.len() as isize) as usize;
        self.activate(ix, cx);
        self.tabs[ix].clone().focus(window, cx);
    }

    /// ⌘W in the editor: close the tab in front (asking first when it has unsaved edits).
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active) {
            let id = tab.id();
            self.close(id, window, cx);
        }
    }

    /// Close the tab of `id`. With unsaved edits it asks: Save (closing once the save lands),
    /// Don't Save, or Cancel.
    pub fn close(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id() == id).cloned() else { return };
        let Some(view) = tab.editor().filter(|_| tab.dirty(cx)).cloned() else {
            self.remove(id, cx);
            return;
        };
        let name = file_name(&view.read(cx).path);
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Do you want to save the changes you made to {name}?"),
            Some("Your changes will be lost if you don't save them."),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else { return };
            let _ = this.update(cx, |this, cx| match choice {
                0 => {
                    if view.update(cx, |v, cx| v.save_for_close(cx)) {
                        this.remove(id, cx);
                    }
                }
                1 => this.remove(id, cx),
                _ => {}
            });
        })
        .detach();
    }

    /// Close every tab but `keep`'s, or those right of it; tabs with unsaved edits stay (closing
    /// them one by one asks about each).
    fn close_many(&mut self, keep: EntityId, right_only: bool, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t.id() == keep) else { return };
        let gone: Vec<EntityId> = self.tabs.iter().enumerate().filter(|(ix, t)| *ix != at && (!right_only || *ix > at) && !t.dirty(cx)).map(|(_, t)| t.id()).collect();
        for id in gone {
            self.remove(id, cx);
        }
    }

    /// Close the tabs with nothing unsaved.
    fn close_saved(&mut self, cx: &mut Context<Self>) {
        let gone: Vec<EntityId> = self.tabs.iter().filter(|t| !t.dirty(cx)).map(|t| t.id()).collect();
        for id in gone {
            self.remove(id, cx);
        }
    }

    /// Take the tab of `id` away, unsaved edits and all; the one in front before it comes back.
    pub(super) fn remove(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t.id() == id) else { return };
        let tab = self.tabs.remove(ix);
        self.forget_file(&tab, cx);
        self.mru.retain(|m| *m != id);
        let front = self.mru.last().and_then(|m| self.tabs.iter().position(|t| t.id() == *m));
        match front {
            Some(f) => self.activate(f, cx),
            None => {
                self.active = 0;
                self.ai.update(cx, |a, cx| a.set_current_file(None, cx));
                self.status.update(cx, |s, cx| s.set_editor(None, cx));
            }
        }
        cx.notify();
    }

    /// A file's tab went: what its language server said about it goes from Problems.
    fn forget_file(&mut self, tab: &EditorTab, cx: &mut Context<Self>) {
        let Some(path) = tab.editor().map(|e| e.read(cx).path.clone()) else { return };
        self.workspace.update(cx, |ws, cx| ws.clear_diagnostics(&path, cx));
    }

    /// The row of tabs over the editor.
    fn tab_bar(&self, glass: Option<f32>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let line = Self::line(glass, cx);
        let chrome = crate::ui::chrome_bg(glass, cx);
        let editor_bg = crate::ui::panel_bg(glass, cx);
        let ember = crate::palette::ember(cx);
        let root = self.workspace.read(cx).ide_root.clone();
        let me = cx.weak_entity();
        let ws = self.workspace.read(cx);
        // ✦: an agent changed the file and the change isn't kept or undone yet.
        let edited: Vec<bool> = self.tabs.iter().map(|t| t.path(cx).is_some_and(|p| ws.agent_edited(&p))).collect();
        h_flex()
            .id("ide-tabs")
            .w_full()
            .h(px(TAB_HEIGHT))
            .flex_none()
            .bg(chrome)
            .border_b_1()
            .border_color(line)
            .overflow_x_scroll()
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let path = tab.path(cx);
                let name = tab.title(cx);
                let on = ix == self.active;
                let dirty = tab.dirty(cx);
                let preview = tab.preview;
                let mark = path.as_ref().and_then(|p| self.explorer.read(cx).mark(p));
                let agent = edited.get(ix).copied().unwrap_or(false);
                let id = tab.id();
                let diff = tab.diff().is_some();
                let group = SharedString::from(format!("ide-tab-{ix}"));
                let me = me.clone();
                let root = root.clone();
                h_flex()
                    .id(("ide-tab", ix))
                    .test_support()
                    .group(group.clone())
                    .relative()
                    .h_full()
                    .flex_none()
                    .pl(px(12.))
                    .pr(px(6.))
                    .gap(px(6.))
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .border_r_1()
                    .border_color(line)
                    .when(on, |el| {
                        el.bg(editor_bg).text_color(theme.foreground).child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(ember))
                    })
                    .when(!on, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.list_hover)))
                    .child(if diff { Icon::new(crate::assets::Lucide::FileDiff).size(px(13.)).text_color(crate::palette::ember(cx)).into_any_element() } else { crate::file_icon::badge(&name, px(13.), cx) })
                    .child(div().when(preview, |el| el.italic()).when_some(mark, |el, m| el.text_color(m.color(cx))).child(name))
                    .when(agent, |el| {
                        el.child(
                            div()
                                .id(("ide-tab-agent", ix))
                                .test_support()
                                .text_size(px(11.))
                                .text_color(ember)
                                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Changed by an agent: keep or undo it in the AI side bar").build(window, cx))
                                .child("✦"),
                        )
                    })
                    .when_some(mark, |el, m| el.child(div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(m.color(cx)).child(m.letter())))
                    .child(
                        // The dot of unsaved edits sits where × goes; × shows on hover.
                        div()
                            .id(("ide-tab-close", ix))
                            .test_support()
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                            .when(dirty, |el| el.child(div().group_hover(group.clone(), |s| s.invisible()).size(px(7.)).rounded_full().bg(theme.foreground.opacity(0.7))))
                            .child(
                                div()
                                    .when(dirty, |el| el.absolute())
                                    .when(!on || dirty, |el| el.invisible().group_hover(group.clone(), |s| s.visible()))
                                    .child(Icon::new(IconName::Close).size(px(12.)).text_color(theme.muted_foreground)),
                            )
                            // The × closes only: the tab under it isn't brought forward first.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(id, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        // A double click keeps a preview tab.
                        if e.click_count() >= 2 {
                            if let Some(t) = this.tabs.get_mut(ix) {
                                t.preview = false;
                            }
                        }
                        this.activate(ix, cx);
                        if let Some(t) = this.tabs.get(ix).cloned() {
                            t.focus(window, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close(id, window, cx)))
                    .context_menu(move |menu, _, _| {
                        let act = |label: &'static str, f: fn(&mut IdeWorkbench, EntityId, &mut Window, &mut Context<IdeWorkbench>)| {
                            let me = me.clone();
                            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let _ = me.update(cx, |this, cx| f(this, id, window, cx));
                            })
                        };
                        let copy = |label: &'static str, text: String| PopupMenuItem::new(label).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
                        let menu = menu.min_w(px(200.))
                            .item(act("Close", |this, id, window, cx| this.close(id, window, cx)))
                            .item(act("Close Others", |this, id, _, cx| this.close_many(id, false, cx)))
                            .item(act("Close to the Right", |this, id, _, cx| this.close_many(id, true, cx)))
                            .item(act("Close Saved", |this, _, _, cx| this.close_saved(cx)))
                            .separator()
                            .item(act("Keep Open", |this, id, _, cx| {
                                if let Some(t) = this.tabs.iter_mut().find(|t| t.id() == id) {
                                    t.preview = false;
                                }
                                cx.notify();
                            }));
                        let Some(path) = path.clone() else { return menu };
                        let relative = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).map(|p| p.display().to_string()).unwrap_or_else(|| path.display().to_string());
                        let (reveal, open) = (path.clone(), path.clone());
                        menu.separator()
                            .item(copy("Copy Path", path.display().to_string()))
                            .item(copy("Copy Relative Path", relative))
                            .separator()
                            .item(PopupMenuItem::new(crate::words::words().reveal_in_file_manager).on_click(move |_, _, cx| cx.reveal_path(&reveal)))
                            .item(PopupMenuItem::new("Open in Default App").on_click(move |_, _, cx| cx.open_with_system(&open)))
                    })
            }))
    }

    /// `crates › trek-app › src › editor.rs`: where the file in front is, under the IDE folder.
    fn breadcrumbs(&self, view: &Entity<EditorView>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let path = view.read(cx).path.clone();
        let root = self.workspace.read(cx).ide_root.clone();
        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(trek_core::paths::tildify(&path)));
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).filter(|p| !p.is_empty() && p != "/").collect();
        let last = parts.len().saturating_sub(1);
        h_flex()
            .id("ide-breadcrumbs")
            .test_support()
            .h(px(24.))
            .flex_none()
            .px(px(14.))
            .gap(px(5.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .overflow_hidden()
            .children(parts.into_iter().enumerate().map(move |(ix, part)| {
                h_flex()
                    .flex_none()
                    .gap(px(5.))
                    .when(ix > 0, |el| el.child(Icon::new(IconName::ChevronRight).size(px(11.)).text_color(theme.muted_foreground.opacity(0.6))))
                    .when(ix == last, |el| el.child(crate::file_icon::badge(&part, px(12.), cx)).text_color(theme.foreground.opacity(0.85)))
                    .child(part)
            }))
    }

    /// The editor area: tabs, breadcrumbs and the file in front; the welcome screen when no
    /// folder is open, shortcuts when no file is.
    pub(super) fn editor_area(&self, glass: Option<f32>, cx: &mut Context<Self>) -> AnyElement {
        let fill = || StyleRefinement::default().size_full();
        match self.tabs.get(self.active).map(|t| t.view.clone()) {
            Some(TabView::Editor(view)) => v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.tab_bar(glass, cx))
                .child(self.breadcrumbs(&view, cx))
                .child(div().flex_1().min_h_0().child(view.cached(fill())))
                .into_any_element(),
            Some(TabView::Diff(view)) => v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.tab_bar(glass, cx))
                .child(div().flex_1().min_h_0().child(view.cached(fill())))
                .into_any_element(),
            None if self.workspace.read(cx).ide_root.is_none() => div().flex_1().min_h_0().w_full().child(self.welcome(cx)).into_any_element(),
            None => div().flex_1().min_h_0().w_full().child(self.watermark(cx)).into_any_element(),
        }
    }

    /// No file open: the shortcuts worth knowing, quietly, as Cursor and VS Code show them.
    fn watermark(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row = |label: &'static str, keys: String| {
            h_flex()
                .gap(px(18.))
                .child(div().w(px(150.)).text_right().child(label))
                .child(div().min_w(px(110.)).text_color(theme.muted_foreground.opacity(0.75)).child(keys))
        };
        v_flex()
            .id("ide-watermark")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .text_size(px(12.5))
            .text_color(theme.muted_foreground)
            .child(div().pb(px(14.)).opacity(0.5).child(crate::brand::logo_mark(px(56.))))
            .child(row("Go to File", keys::hint(Id::QuickOpen)))
            .child(row("New Chat", keys::hint(Id::NewThread)))
            .child(row("Keep / Undo a Change", format!("{} / {}", keys::hint(Id::KeepHunk), keys::hint(Id::UndoHunk))))
            .child(row("Show Terminal", keys::hint(Id::ToggleRightPanel)))
            .child(row("Toggle AI Side Bar", keys::hint(Id::ToggleAiBar)))
            .child(row("Switch to Agents", keys::hint(Id::SwitchMode)))
    }

    /// No folder open: what to do, then recent folders and loose files.
    fn welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let folders: Vec<String> = ws.settings.ide.recent_folders.iter().chain(ws.settings.user_projects.iter()).cloned().fold(Vec::new(), |mut v, f| {
            if !v.contains(&f) {
                v.push(f);
            }
            v
        });
        let files: Vec<String> = ws.settings.ide.recent_files.iter().take(8).cloned().collect();
        // Capped at 340 but shrinks with the column: a narrower welcome must not clip rows.
        let mut list = v_flex().w_full().max_w(px(340.)).gap_1();
        let row = |id: SharedString, icon: AnyElement, name: String, path: &Path| {
            h_flex()
                .id(id)
                .test_support()
                .w_full()
                .px_2()
                .h(px(28.))
                .gap_2()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(icon)
                // The name stays whole; the path gives way first (`~/Code/…`).
                .child(div().flex_none().max_w(relative(0.7)).truncate().text_sm().child(name))
                .child(div().flex_1().min_w_0().text_xs().text_color(theme.muted_foreground).truncate().child(trek_core::paths::tildify(path)))
        };
        if !folders.is_empty() {
            list = list.child(div().pt_4().pb_1().text_xs().text_color(theme.muted_foreground).child("Recent folders"));
            for f in folders.iter().take(6) {
                let p = PathBuf::from(f);
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                let icon = crate::file_icon::folder(&name, false, px(15.), cx);
                list = list.child(row(SharedString::from(format!("ide-recent-folder-{f}")), icon, name, &p).on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |ws, cx| ws.set_ide_root(p.clone(), cx));
                })));
            }
        }
        if !files.is_empty() {
            list = list.child(div().pt_3().pb_1().text_xs().text_color(theme.muted_foreground).child("Recent files"));
            for f in &files {
                let p = PathBuf::from(f);
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                let icon = crate::file_icon::badge(&name, px(15.), cx);
                list = list.child(row(SharedString::from(format!("ide-recent-file-{f}")), icon, name, &p).on_click(cx.listener(move |this, _, window, cx| {
                    this.open(p.clone(), None, false, window, cx);
                })));
            }
        }
        div().id("ide-welcome").size_full().overflow_y_scroll().child(
            v_flex()
                .w_full()
                .min_h(relative(1.))
                .items_center()
                .justify_center()
                .py(px(40.))
                .gap_4()
                .child(crate::brand::logo_mark(px(52.)))
                .child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(div().text_lg().font_medium().child("Trek IDE"))
                        .child(div().text_sm().text_color(theme.muted_foreground).child("A folder, a file, and the agents already signed in.")),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .justify_center()
                        .flex_wrap()
                        .child(
                            Button::new("ide-open-folder")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Open folder")
                                .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.open_ide_folder(cx)))),
                        )
                        .child(Button::new("ide-open-file").outline().icon(IconName::File).label("Open file").on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.open_ide_file(cx))))),
                )
                .child(list),
        )
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}
