//! Trek IDE: the main window's Editor mode, a VS Code-shaped workbench over the same workspace.
//!
//! ```text
//! ┌ title bar (root.rs): [Agents | Editor]   ⌕ folder — Go to file (⌘P)        ◧ ⊟ ◨ ┐
//! ├ primary side bar ─┬ tabs / breadcrumbs ──────────────────┬ AI side bar ─────────┤
//! │ activity row      │ editor                                │ chat tabs  + 🕘 ⋯     │
//! │ Explorer · Search │                                       │ transcript            │
//! │ Source Control ·  ├ Problems · Output · Terminal · Tasks ─┤ composer              │
//! │ Agents            │                                       │                       │
//! ├ status bar ───────┴───────────────────────────────────────┴───────────────────────┤
//! ```
//!
//! Flat like the editors it borrows from: hairlines between regions, no cards. The regions
//! resize by their edges and hide with ⌘B, ⌥⌘B and ⌘J; the layout is kept in settings. Editor
//! tabs live here (the harness's lone `Route::Editor` borrows them too), the AI side bar is
//! `ai::AiPane` over `Scope::Ide`.

mod activity;
pub(crate) mod ai;
mod agents_view;
pub(crate) mod diff_view;
mod editor_group;
pub(crate) mod git;
mod layout;
mod panel;
pub(crate) mod scm;
mod status_bar;
mod watch;

pub use activity::SideView;
pub use editor_group::EditorTab;
pub use panel::{OutputChannel, PanelTab};

use crate::editor::EditorView;
use crate::panels::explorer::ExplorerPanel;
use crate::panels::ide_search::IdeSearch;
use crate::workspace::{Mode, Workspace};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::settings::IdeLayout;

pub struct IdeWorkbench {
    workspace: Entity<Workspace>,
    /// The editor group's tabs, in order.
    pub(crate) tabs: Vec<EditorTab>,
    pub(crate) active: usize,
    /// Tabs by when they were last in front (most recent last): closing the front one goes back
    /// to the one before it.
    mru: Vec<EntityId>,
    pub(crate) explorer: Entity<ExplorerPanel>,
    pub(crate) search: Entity<IdeSearch>,
    /// Source Control, made the first time it's shown (it reads git as it opens).
    pub(crate) scm: Option<Entity<scm::ScmView>>,
    pub(crate) ai: Entity<ai::AiPane>,
    status: Entity<status_bar::IdeStatus>,
    /// The bottom panel's terminals; one is made the first time the Terminal tab shows.
    terminals: Vec<Entity<crate::panels::terminal::TerminalPanel>>,
    terminal: usize,
    pub(crate) view: SideView,
    pub(crate) panel_tab: PanelTab,
    /// The Output panel's channel, when one was picked (else the chat in front's).
    pub(crate) output_channel: Option<OutputChannel>,
    /// Sizes and which regions show, as in settings.
    pub(crate) layout: IdeLayout,
    /// A region's edge being dragged.
    drag: Option<layout::Drag>,
    /// `Workspace::files_epoch` the open editors last reloaded at.
    files_seen: u64,
    /// `Workspace::agent_edits` the open editors last looked at their files at.
    edits_seen: u64,
    /// FSEvents on the IDE folder, while the editor is on screen.
    watch: Option<watch::FolderWatch>,
    /// Open files' paths with links resolved, as FSEvents names them (read once per file).
    canonical: std::collections::HashMap<PathBuf, PathBuf>,
    mode_seen: Mode,
    _subscriptions: Vec<Subscription>,
}

impl IdeWorkbench {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = cx.new(|cx| ExplorerPanel::for_ide(workspace.clone(), cx));
        // A file renamed or trashed in the Explorer: its tabs follow it, or close.
        let me = cx.weak_entity();
        explorer.update(cx, |e, _| {
            e.on_moved(Box::new(move |from, to, window, cx| {
                let _ = me.update(cx, |this, cx| this.file_moved(&from, to.as_deref(), window, cx));
            }))
        });
        let search = cx.new(|cx| IdeSearch::new(workspace.clone(), window, cx));
        let ai = cx.new(|cx| ai::AiPane::new(workspace.clone(), window, cx));
        let me = cx.weak_entity();
        let status = cx.new(|cx| status_bar::IdeStatus::new(workspace.clone(), me, cx));
        let (layout, files_seen, edits_seen, mode_seen) = {
            let ws = workspace.read(cx);
            (ws.settings.ide.layout.clone(), ws.files_epoch, ws.agent_edits, ws.mode)
        };
        let subscriptions = vec![cx.observe_in(&workspace, window, |this, ws, window, cx| {
            let (files, edits, mode) = (ws.read(cx).files_epoch, ws.read(cx).agent_edits, ws.read(cx).mode);
            // Another branch checked out or a rewind: open files show what's on disk now.
            if files != this.files_seen {
                this.files_seen = files;
                for e in this.editors() {
                    e.update(cx, |e, cx| e.reload_from_disk(window, cx));
                }
            }
            // An agent finished a tool call: files it wrote show in their clean editors. Not
            // while the window shows Agents: the editors aren't on screen (and a lone harness
            // editor is the one in front, which `Route::Editor` reloads as it comes back).
            if edits != this.edits_seen && (mode == Mode::Editor || matches!(ws.read(cx).route, crate::workspace::Route::Editor { .. })) {
                this.edits_seen = edits;
                for e in this.editors() {
                    e.update(cx, |e, cx| e.reload_if_changed(window, cx));
                }
            }
            this.watch_root(window, cx);
            // The editors draw their own header in the harness only.
            if mode != this.mode_seen {
                this.mode_seen = mode;
                for e in this.editors() {
                    e.update(cx, |_, cx| cx.notify());
                }
            }
            cx.notify();
        }),
        // "Add to Chat" from the Explorer: a chip in the AI side bar, shown if it was hidden.
        cx.subscribe_in(&workspace, window, |this, ws, event: &crate::workspace::WorkspaceEvent, window, cx| {
            if let crate::workspace::WorkspaceEvent::AddToChat(chip) = event {
                if ws.read(cx).ide() {
                    this.add_chip(chip.clone(), window, cx);
                }
            }
        })];
        let mut this = Self {
            workspace,
            tabs: Vec::new(),
            active: 0,
            mru: Vec::new(),
            explorer,
            search,
            scm: None,
            ai,
            status,
            terminals: Vec::new(),
            terminal: 0,
            view: SideView::Explorer,
            panel_tab: PanelTab::Problems,
            output_channel: None,
            layout,
            drag: None,
            files_seen,
            edits_seen,
            watch: None,
            canonical: Default::default(),
            mode_seen,
            _subscriptions: subscriptions,
        };
        this.watch_root(window, cx);
        this
    }

    /// `from` (a file or folder) moved to `to`, or went to the Trash (`None`): clean tabs of
    /// files under it open at the new place (or close); a tab with unsaved edits keeps them, and
    /// saves to the new place.
    pub(crate) fn file_moved(&mut self, from: &std::path::Path, to: Option<&std::path::Path>, window: &mut Window, cx: &mut Context<Self>) {
        let front = self.tabs.get(self.active).map(EditorTab::id);
        for tab in self.tabs.clone() {
            let Some(e) = tab.editor().cloned() else { continue };
            let path = e.read(cx).path.clone();
            let Ok(rest) = path.strip_prefix(from) else { continue };
            let moved = to.map(|t| if rest.as_os_str().is_empty() { t.to_path_buf() } else { t.join(rest) });
            self.canonical.remove(&path);
            if tab.dirty(cx) {
                if let Some(m) = moved {
                    e.update(cx, |e, cx| {
                        e.path = m;
                        cx.notify();
                    });
                }
                continue;
            }
            let was_front = front == Some(tab.id());
            self.remove(tab.id(), cx);
            if let Some(m) = moved {
                let ix = self.open_tab(m, tab.preview, window, cx);
                if was_front {
                    self.activate(ix, cx);
                }
            }
        }
        cx.notify();
    }

    /// The editor in front, if a file is open (not a diff).
    pub fn active_editor(&self) -> Option<Entity<EditorView>> {
        self.tabs.get(self.active).and_then(|t| t.editor().cloned())
    }

    /// The open files' editors.
    pub(crate) fn editors(&self) -> Vec<Entity<EditorView>> {
        self.tabs.iter().filter_map(|t| t.editor().cloned()).collect()
    }

    /// Open `path` (a tab, or the one it has), on `line` when given. `preview`: in the preview
    /// tab, which the next preview replaces until it's edited or kept.
    pub fn open(&mut self, path: PathBuf, line: Option<u32>, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.open_tab(path.clone(), preview, window, cx);
        self.activate(ix, cx);
        self.workspace.update(cx, |ws, _| ws.ide_file_seen(&path));
        self.explorer.update(cx, |e, cx| e.reveal(&path, cx));
        let Some(view) = self.tabs[ix].editor().cloned() else { return };
        if let Some(line) = line {
            view.update(cx, |e, cx| e.goto_line(line, window, cx));
        }
        // A file opened on purpose takes the keys; a preview leaves them in the tree.
        if !preview {
            view.update(cx, |e, cx| e.focus(window, cx));
        }
        cx.notify();
    }

    /// The tab for `path` in the harness's lone editor screen (made if it has none), in front.
    pub fn editor_for(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorView> {
        let ix = self.open_tab(path.to_path_buf(), false, window, cx);
        if ix != self.active {
            self.activate(ix, cx);
        }
        self.tabs[ix].editor().cloned().expect("a file's tab is an editor")
    }

    /// Show `view` in the primary side bar (opening the bar if it was hidden).
    pub fn show_view(&mut self, view: SideView, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        if !self.layout.primary_open {
            self.layout.primary_open = true;
            self.save_layout(cx);
        }
        match view {
            SideView::Search => self.search.update(cx, |s, cx| s.focus(window, cx)),
            SideView::Scm if self.scm.is_none() => {
                let (ws, me) = (self.workspace.clone(), cx.weak_entity());
                self.scm = Some(cx.new(|cx| scm::ScmView::new(ws, me, window, cx)));
            }
            _ => {}
        }
        cx.notify();
    }

    /// ⌘B.
    pub fn toggle_primary(&mut self, cx: &mut Context<Self>) {
        self.layout.primary_open = !self.layout.primary_open;
        self.save_layout(cx);
    }

    /// ⌥⌘B.
    pub fn toggle_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.ai_open = !self.layout.ai_open;
        self.save_layout(cx);
        if self.layout.ai_open {
            self.ai.update(cx, |a, cx| a.focus(window, cx));
        }
    }

    /// ⌘J.
    pub fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.panel_open = !self.layout.panel_open;
        self.save_layout(cx);
        if self.layout.panel_open {
            self.show_panel(self.panel_tab, window, cx);
        }
    }

    /// The AI side bar takes the keys, shown if it was hidden.
    pub fn focus_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.layout.ai_open {
            self.layout.ai_open = true;
            self.save_layout(cx);
        }
        self.ai.update(cx, |a, cx| a.focus(window, cx));
    }

    /// ⌘⇧L, ⌘L: the editor's selected lines (the file, with none) go to the AI side bar as a
    /// chip, for the chat in front or a new one.
    pub fn add_selection(&mut self, new_chat: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.active_editor() else { return };
        let chip = {
            let e = view.read(cx);
            if e.scratch {
                return;
            }
            match e.selection(cx) {
                Some((lines, text)) => ai::context::ContextChip::Selection { path: e.path.clone(), lines, text },
                None => ai::context::ContextChip::File { path: e.path.clone() },
            }
        };
        if new_chat {
            self.workspace.update(cx, |ws, cx| ws.ide_new_chat(cx));
        }
        if !self.layout.ai_open {
            self.layout.ai_open = true;
            self.save_layout(cx);
        }
        self.ai.update(cx, |a, cx| a.add_chip(chip, window, cx));
    }

    /// A chip (a file from the Explorer, a problem, terminal output) for the AI side bar's
    /// input, the bar shown if it was hidden.
    pub fn add_chip(&mut self, chip: ai::context::ContextChip, window: &mut Window, cx: &mut Context<Self>) {
        if !self.layout.ai_open {
            self.layout.ai_open = true;
            self.save_layout(cx);
        }
        self.ai.update(cx, |a, cx| a.add_chip(chip, window, cx));
    }

    /// The layout as it is now goes to settings, for the next launch.
    fn save_layout(&mut self, cx: &mut Context<Self>) {
        let layout = self.layout.clone();
        self.workspace.update(cx, |ws, cx| {
            if ws.settings.ide.layout != layout {
                ws.settings.ide.layout = layout;
                ws.save_settings(cx);
            }
        });
        cx.notify();
    }

    /// A hairline between regions: the theme's, or the rim glass catches.
    fn line(glass: Option<f32>, cx: &App) -> Hsla {
        if glass.is_some() { crate::ui::panel_border(glass, cx) } else { cx.theme().border }
    }

    #[cfg(test)]
    pub fn tab_paths(&self, cx: &App) -> Vec<(PathBuf, bool)> {
        self.tabs.iter().filter_map(|t| t.editor().map(|e| (e.read(cx).path.clone(), t.preview))).collect()
    }
}

impl Render for IdeWorkbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("IdeWorkbench");
        let glass = self.workspace.read(cx).glass();
        let line = Self::line(glass, cx);
        let chrome = crate::ui::chrome_bg(glass, cx);
        let viewport = window.viewport_size();
        let layout = self.layout.clone();
        // What's left for the editor decides how wide the side bars may be drawn.
        let (primary_w, ai_w) = layout::fit_widths(&layout, viewport.width.as_f32());
        let panel_h = layout::fit_panel(&layout, viewport.height.as_f32());
        let dragging = self.drag.is_some();
        let fill = || StyleRefinement::default().size_full();

        let center = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(crate::ui::panel_bg(glass, cx))
            .child(self.editor_area(glass, cx))
            .when(layout.panel_open, |el| el.child(self.bottom_panel(panel_h, glass, window, cx)));

        v_flex()
            .id("ide-workbench")
            .when(dragging, |el| self.drag_listeners(el, cx))
            .test_support()
            .key_context("TrekIde")
            .on_action(cx.listener(|this, _: &crate::AddSelectionToChat, window, cx| this.add_selection(false, window, cx)))
            .on_action(cx.listener(|this, _: &crate::AddSelectionToNewChat, window, cx| this.add_selection(true, window, cx)))
            .size_full()
            .border_t_1()
            .border_color(line)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(layout.primary_open, |el| {
                        el.child(
                            v_flex()
                                .id("ide-primary")
            .test_support()
                                .relative()
                                .w(px(primary_w))
                                .h_full()
                                .flex_none()
                                .bg(chrome)
                                .border_r_1()
                                .border_color(line)
                                .child(self.activity_row(cx))
                                .child(self.side_view(fill, cx))
                                .child(self.handle(layout::Split::Primary, cx)),
                        )
                    })
                    .child(center)
                    .when(layout.ai_open, |el| {
                        el.child(
                            v_flex()
                                .id("ide-ai")
            .test_support()
                                .relative()
                                .w(px(ai_w))
                                .h_full()
                                .flex_none()
                                .bg(chrome)
                                .border_l_1()
                                .border_color(line)
                                .child(self.ai.clone().cached(fill()))
                                .child(self.handle(layout::Split::Ai, cx)),
                        )
                    }),
            )
            .child(self.status.clone().cached(StyleRefinement::default().w_full().flex_none().h(px(status_bar::HEIGHT))))
    }
}
