//! Right panel (Synara-style): tabs of tools — Terminal, Browser, Simulator, Explorer, Side chat, Git.

mod browser;
pub mod explorer;
pub mod ide_search;
pub(crate) mod git;
mod side_chat;
mod simhid;
mod simulator;
pub(crate) mod terminal;

use crate::ui;
use crate::workspace::{PanelTool, Workspace};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub fn tool_icon(tool: PanelTool) -> Icon {
    match tool {
        PanelTool::Terminal => Icon::new(IconName::SquareTerminal),
        PanelTool::Browser => Icon::new(IconName::Globe),
        PanelTool::Explorer => Icon::new(IconName::FolderOpen),
        PanelTool::SideChat => Icon::new(crate::assets::Lucide::MessageSquare),
        PanelTool::Git => Icon::new(crate::assets::Lucide::GitBranch),
        PanelTool::Simulator => Icon::new(crate::assets::Lucide::Smartphone),
    }
}

/// What a tool is for, in a line: the empty panel's grid says it under the tool's name.
pub fn tool_blurb(tool: PanelTool) -> &'static str {
    match tool {
        PanelTool::Terminal => "A shell in the project's folder",
        PanelTool::Browser => "Preview the app or any page",
        PanelTool::Simulator => "Run and drive an iOS simulator",
        PanelTool::Explorer => "Browse the project's files",
        PanelTool::SideChat => "Ask on the side, off the thread",
        PanelTool::Git => "Changes, commits and branches",
    }
}

/// Quiet centered message for empty tool states.
pub fn empty(text: impl Into<SharedString>, cx: &App) -> Div {
    v_flex().size_full().items_center().justify_center().px_6().text_sm().text_center().text_color(cx.theme().muted_foreground).child(text.into())
}

enum View {
    Git(Entity<git::GitPanel>),
    Explorer(Entity<explorer::ExplorerPanel>),
    Terminal(Entity<terminal::TerminalPanel>),
    Browser(Entity<browser::BrowserPanel>),
    SideChat(Entity<side_chat::SideChatPanel>),
    Simulator(Entity<simulator::SimulatorPanel>),
}

struct Tab {
    id: u64,
    tool: PanelTool,
    view: View,
}

pub const MIN_PANEL: f32 = 320.;
/// A labelled tab's width beside its label (padding, icon, close button), and a label character's
/// rough width: enough to tell whether the labels fit before they're laid out.
const TAB_CHROME: f32 = 58.;
const TAB_CHAR: f32 = 7.4;
pub const WIDE_PANEL: f32 = 720.;

pub struct RightPanel {
    workspace: Entity<Workspace>,
    pub open: bool,
    /// Width in points (persisted in settings.layout); the root clamps it to the window.
    pub width: f32,
    tabs: Vec<Tab>,
    active: Option<u64>,
    next_id: u64,
    launcher_open: bool,
    /// Where the tab strip is scrolled to, and what it was drawn with: overflowing, cut off at
    /// its start, at its end (see `watch_overflow`).
    tab_scroll: ScrollHandle,
    drawn: std::rc::Rc<std::cell::Cell<(bool, bool, bool)>>,
    /// The tab last scrolled into view.
    scrolled_to: Option<u64>,
}

impl RightPanel {
    pub fn new(workspace: Entity<Workspace>) -> Self {
        Self {
            workspace,
            open: false,
            width: trek_core::settings::DEFAULT_RIGHT_PANEL_WIDTH,
            tabs: vec![],
            active: None,
            next_id: 1,
            launcher_open: false,
            tab_scroll: ScrollHandle::new(),
            drawn: Default::default(),
            scrolled_to: None,
        }
    }

    /// Set and persist the width (the root clamps what's drawn to the window).
    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.width = width.max(MIN_PANEL);
        let w = self.width;
        self.workspace.update(cx, |ws, cx| {
            ws.settings.layout.right_panel_width = w;
            ws.save_settings(cx);
        });
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        self.sync_native(cx);
        cx.notify();
    }

    /// Open a tool (reusing an existing tab except for terminals and side chats, which stack).
    pub fn open_tool(&mut self, tool: PanelTool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.launcher_open = false;
        let reuse = !matches!(tool, PanelTool::Terminal | PanelTool::SideChat);
        if reuse {
            if let Some(t) = self.tabs.iter().find(|t| t.tool == tool) {
                self.active = Some(t.id);
                self.after_activate(window, cx);
                return;
            }
        }
        let ws = self.workspace.clone();
        let view = match tool {
            PanelTool::Git => View::Git(cx.new(|cx| git::GitPanel::new(ws, window, cx))),
            PanelTool::Explorer => View::Explorer(cx.new(|cx| explorer::ExplorerPanel::new(ws, cx))),
            PanelTool::Terminal => {
                let (cwd, shell) = (ws.read(cx).current_cwd(), ws.read(cx).settings.terminal.shell.clone());
                View::Terminal(cx.new(|cx| terminal::TerminalPanel::new(cwd, shell, cx)))
            }
            PanelTool::Browser => View::Browser(cx.new(|cx| browser::BrowserPanel::new(ws, window, cx))),
            PanelTool::SideChat => View::SideChat(cx.new(|cx| side_chat::SideChatPanel::new(ws, window, cx))),
            PanelTool::Simulator => View::Simulator(cx.new(|cx| simulator::SimulatorPanel::new(ws, window, cx))),
        };
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab { id, tool, view });
        self.active = Some(id);
        self.after_activate(window, cx);
    }

    /// Open the Git tool on the changes of the turn ending at `end` (by item id) of `thread`,
    /// `path`'s diff open.
    pub fn show_turn_diff(&mut self, thread: String, end: String, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(PanelTool::Git, window, cx);
        if let Some(Tab { view: View::Git(g), .. }) = self.active_tab() {
            g.update(cx, |g, cx| g.show_turn(thread, end, path, cx));
        }
    }

    /// Run a setup command (install / sign in) or a project action in a fresh terminal tab, then
    /// rescan agents.
    pub fn run_command(&mut self, job: terminal::Job, cwd: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.launcher_open = false;
        let cwd = cwd.or_else(|| self.workspace.read(cx).current_cwd());
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        let ws = self.workspace.downgrade();
        let view = cx.new(|cx| {
            let mut t = terminal::TerminalPanel::with_command(cwd, job, shell, cx);
            t.on_exit(move |cx| {
                let _ = ws.update(cx, |ws, cx| ws.detect_agents(cx));
            });
            t
        });
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab { id, tool: PanelTool::Terminal, view: View::Terminal(view) });
        self.active = Some(id);
        self.after_activate(window, cx);
    }

    fn after_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_native(cx);
        match self.active_tab().map(|t| &t.view) {
            Some(View::Terminal(t)) => {
                let handle = t.read(cx).focus_handle();
                handle.focus(window, cx);
            }
            // A side chat is there to be typed in: the keys go to its box at once.
            Some(View::SideChat(s)) => {
                let handle = s.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The Browser tool, if it's open (the shots harness's `browser` command).
    #[cfg(any(test, feature = "shots"))]
    pub fn browser(&self) -> Option<Entity<browser::BrowserPanel>> {
        self.tabs.iter().find_map(|t| match &t.view {
            View::Browser(b) => Some(b.clone()),
            _ => None,
        })
    }

    /// The tab in front, by its id.
    #[cfg(test)]
    pub(crate) fn active_id(&self) -> Option<u64> {
        self.active
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    /// What the Git tool's commit message box holds, while it's the tab on show.
    #[cfg(test)]
    pub(crate) fn git_message(&self, cx: &App) -> Option<String> {
        match self.active_tab().map(|t| &t.view) {
            Some(View::Git(g)) => Some(g.read(cx).message_text(cx)),
            _ => None,
        }
    }

    /// The turn the Git tool shows (its files) and the file selected, while it's the tab on show.
    #[cfg(test)]
    pub(crate) fn git_turn(&self, cx: &App) -> Option<(Vec<String>, Option<String>)> {
        match self.active_tab().map(|t| &t.view) {
            Some(View::Git(g)) => g.read(cx).turn_shown(),
            _ => None,
        }
    }

    /// The Git tool's diff scroll offset, while it's the tab on show.
    #[cfg(test)]
    pub(crate) fn git_diff_offset(&self, cx: &App) -> Option<Point<Pixels>> {
        match self.active_tab().map(|t| &t.view) {
            Some(View::Git(g)) => Some(g.read(cx).diff_offset()),
            _ => None,
        }
    }

    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        match self.tabs.iter().find(|t| t.id == id).map(|t| &t.view) {
            Some(View::Browser(b)) => b.update(cx, |b, cx| b.set_visible(false, cx)),
            Some(View::Simulator(s)) => s.update(cx, |s, cx| s.release(window, cx)),
            _ => {}
        }
        let ix = self.tabs.iter().position(|t| t.id == id);
        self.tabs.retain(|t| t.id != id);
        if self.active == Some(id) {
            self.active = ix.and_then(|i| self.tabs.get(i.saturating_sub(1)).or(self.tabs.first())).map(|t| t.id);
        }
        self.after_activate(window, cx);
    }

    /// Native web views draw above GPUI, so only the visible Browser tab may show.
    /// The Simulator only mirrors (and polls the device) while its tab is on screen.
    pub fn sync_native(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let open = self.open;
        let menu_open = self.launcher_open || self.workspace.read(cx).overlay_open;
        for t in &self.tabs {
            match &t.view {
                View::Browser(b) => {
                    // Hidden while the launcher menu is open, or it would cover the menu.
                    let show = open && active == Some(t.id) && !menu_open;
                    b.update(cx, |b, cx| b.set_visible(show, cx));
                }
                View::Simulator(s) => {
                    let show = open && active == Some(t.id);
                    s.update(cx, |s, cx| s.set_visible(show, cx));
                }
                _ => {}
            }
        }
    }

    /// The + menu's rows: a tool per row.
    fn launcher_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        PanelTool::offered()
            .into_iter()
            .map(|tool| {
                h_flex()
                    .id(SharedString::from(format!("launch-{}", tool.label())))
                    .gap(px(12.))
                    .px(px(14.))
                    .h(px(34.))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_sm()
                    .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                    .child(tool_icon(tool).small().text_color(theme.muted_foreground))
                    .child(tool.label())
                    .on_click(cx.listener(move |this, _, window, cx| this.open_tool(tool, window, cx)))
                    .into_any_element()
            })
            .collect()
    }

    /// The empty panel: the tools in a compact grid, each with what it's for. One column when
    /// the panel is too narrow for two.
    fn tool_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let cols = if self.width >= 420. { 2 } else { 1 };
        let tiles = PanelTool::offered().into_iter().map(|tool| {
            h_flex()
                .id(SharedString::from(format!("tool-tile-{}", tool.label())))
                .test_support()
                .min_w_0()
                .items_start()
                .gap(px(10.))
                .px(px(10.))
                .py(px(9.))
                .rounded(px(10.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                .child(
                    div()
                        .flex_none()
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.))
                        .bg(theme.foreground.opacity(0.06))
                        .child(tool_icon(tool).small().text_color(theme.muted_foreground)),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .gap(px(1.))
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(theme.foreground).child(tool.label()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(tool_blurb(tool))),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.open_tool(tool, window, cx)))
        });
        v_flex()
            .id("tool-grid")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .px_4()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(if cols == 2 { 520. } else { 300. }))
                    .gap(px(10.))
                    .child(div().px(px(10.)).text_xs().text_color(theme.muted_foreground).child("Open a tool"))
                    .child(div().grid().grid_cols(cols).gap(px(4.)).children(tiles)),
            )
            .into_any_element()
    }
}

impl Render for RightPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("RightPanel");
        let theme = cx.theme().clone();
        let active = self.active;
        // Labels while they fit; past that the tabs not in front show just their icon (named on
        // hover), and if even those don't fit the strip scrolls, fades at a cut edge and lists
        // every tab in a menu. No tab is ever cut to a sliver.
        let room = match self.tab_scroll.bounds().size.width {
            w if w > px(0.) => f32::from(w),
            _ => self.width - 140.,
        };
        let labelled: f32 = self.tabs.iter().map(|t| TAB_CHROME + TAB_CHAR * t.tool.label().len() as f32 + 4.).sum();
        let compact = labelled > room;
        if active != self.scrolled_to {
            self.scrolled_to = active;
            if let Some(ix) = self.tabs.iter().position(|t| Some(t.id) == active) {
                self.tab_scroll.scroll_to_item(ix);
            }
        }
        let (over, cut_start, cut_end) = tab_overflow(&self.tab_scroll);
        self.drawn.set((over, cut_start, cut_end));
        let tabs = h_flex()
            .id("panel-tabs")
            .test_support()
            .size_full()
            .gap_1()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll)
            .children(self.tabs.iter().map(|t| {
                let id = t.id;
                let is_active = Some(id) == active;
                let icon_only = compact && !is_active;
                let label = t.tool.label();
                h_flex()
                    .id(("panel-tab", id as usize))
                    .test_support()
                    .group("panel-tab")
                    .h(px(28.))
                    .pl(px(if icon_only { 8. } else { 10. }))
                    .pr(px(if icon_only { 8. } else { 4. }))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .flex_none()
                    .cursor_pointer()
                    .text_sm()
                    .when(is_active, |el| el.bg(theme.foreground.opacity(0.08)))
                    .when(!is_active, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.foreground.opacity(0.05))))
                    .when(icon_only, |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)))
                    .child(tool_icon(t.tool).xsmall())
                    .when(!icon_only, |el| el.child(label))
                    .when(!icon_only, |el| el.child(
                        div()
                            .id(("panel-tab-close", id as usize))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .when(!is_active, |el| el.invisible().group_hover("panel-tab", |s| s.visible()))
                            .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                            .child(Icon::new(IconName::Close).xsmall())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(id, window, cx);
                            })),
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = Some(id);
                        this.after_activate(window, cx);
                    }))
            }));
        let bg = crate::ui::panel_bg(self.workspace.read(cx).glass(), cx);
        let fade = |id: &'static str, left: bool| {
            let (from, to) = if left { (bg, bg.opacity(0.)) } else { (bg.opacity(0.), bg) };
            div()
                .id(id)
                .test_support()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(24.))
                .when(left, |el| el.left_0())
                .when(!left, |el| el.right_0())
                .bg(linear_gradient(90., linear_color_stop(from, 0.), linear_color_stop(to, 1.)))
        };
        let tabs = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(28.))
            .child(tabs)
            .when(cut_start, |el| el.child(fade("panel-tabs-fade-start", true)))
            .when(cut_end, |el| el.child(fade("panel-tabs-fade-end", false)))
            .child(watch_overflow(self.tab_scroll.clone(), self.drawn.clone(), self.tabs.iter().position(|t| Some(t.id) == active)));
        let all_tabs = over.then(|| {
            let items: Vec<(u64, PanelTool, bool)> = self.tabs.iter().map(|t| (t.id, t.tool, Some(t.id) == active)).collect();
            let panel = cx.entity().downgrade();
            ui::icon_button("panel-tabs-all", IconName::ChevronDown, "All tabs").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                menu = menu.min_w(px(180.));
                for (id, tool, on) in &items {
                    let (panel, id) = (panel.clone(), *id);
                    menu = menu.item(PopupMenuItem::new(tool.label()).icon(tool_icon(*tool)).checked(*on).on_click(move |_, window, cx| {
                        let _ = panel.update(cx, |p, cx| {
                            p.active = Some(id);
                            p.after_activate(window, cx);
                        });
                    }));
                }
                menu
            })
        });

        let launcher_open = self.launcher_open;
        let entity = cx.entity();
        let launcher = Popover::new("panel-launcher")
            .anchor(Anchor::TopRight)
            .appearance(false)
            .open(launcher_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.launcher_open = *open;
                this.sync_native(cx);
                cx.notify();
            }))
            .trigger(ui::icon_button("panel-add", IconName::Plus, "Open a tool"))
            .content(move |_, _, cx| {
                let rows = entity.update(cx, |p, cx| p.launcher_rows(cx));
                ui::menu_surface(cx).w(px(220.)).children(rows)
            });

        let header = h_flex()
            .h(px(44.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(tabs)
            .children(all_tabs)
            .child(launcher)
            .child({
                let wide = self.width >= WIDE_PANEL - 1.;
                ui::icon_button("panel-wide", if wide { IconName::Minimize } else { IconName::Maximize }, if wide { "Narrower" } else { "Wider" }).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let w = if wide { trek_core::settings::DEFAULT_RIGHT_PANEL_WIDTH } else { WIDE_PANEL };
                        this.set_width(w, cx);
                    },
                ))
            })
            .child(ui::icon_button("panel-close", IconName::PanelRightClose, "Hide panel (⌘J)").on_click(cx.listener(|this, _, _, cx| this.toggle(cx))));

        let body = match self.active_tab() {
            Some(t) => match &t.view {
                View::Git(v) => v.clone().into_any_element(),
                View::Explorer(v) => v.clone().into_any_element(),
                View::Terminal(v) => v.clone().into_any_element(),
                View::Browser(v) => v.clone().into_any_element(),
                View::SideChat(v) => v.clone().into_any_element(),
                View::Simulator(v) => v.clone().into_any_element(),
            },
            None => self.tool_grid(cx),
        };

        v_flex().size_full().child(header).child(div().flex_1().min_h_0().child(body))
    }
}

/// Whether the tool tabs overflow their strip, and whether some are cut off at its start and end.
fn tab_overflow(scroll: &ScrollHandle) -> (bool, bool, bool) {
    let max = scroll.max_offset().x;
    let at = -scroll.offset().x;
    let over = max > px(0.5);
    (over, over && at > px(0.5), over && at < max - px(0.5))
}

/// Draws the panel again when its tab strip, once laid out, isn't what it was drawn as (tabs came
/// to overflow it, or it scrolled to an edge), so the fades and the menu keep up; and scrolls the
/// tab in front into view once it has been laid out (the panel is a cached view: no other frame
/// would get to it).
fn watch_overflow(scroll: ScrollHandle, drawn: std::rc::Rc<std::cell::Cell<(bool, bool, bool)>>, front: Option<usize>) -> impl IntoElement {
    canvas(
        move |_, window, _| {
            let view = scroll.bounds();
            let hidden = front.and_then(|ix| scroll.bounds_for_item(ix)).is_some_and(|b| {
                let at = scroll.offset().x;
                b.left() + at < view.left() - px(0.5) || b.right() + at > view.right() + px(0.5)
            });
            if hidden {
                scroll.scroll_to_item(front.unwrap_or_default());
            }
            if hidden || tab_overflow(&scroll) != drawn.get() {
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_0()
}
