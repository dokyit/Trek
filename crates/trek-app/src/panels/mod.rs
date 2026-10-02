//! Right panel (Synara-style): tabs of tools — Terminal, Browser, Explorer, Side chat, Git.

mod browser;
mod explorer;
mod git;
mod side_chat;
mod terminal;

use crate::ui;
use crate::workspace::{PanelTool, Workspace};
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
    }
}

/// Quiet centered message for empty tool states.
pub fn empty(text: &'static str, cx: &App) -> Div {
    v_flex().size_full().items_center().justify_center().px_6().text_sm().text_center().text_color(cx.theme().muted_foreground).child(text)
}

enum View {
    Git(Entity<git::GitPanel>),
    Explorer(Entity<explorer::ExplorerPanel>),
    Terminal(Entity<terminal::TerminalPanel>),
    Browser(Entity<browser::BrowserPanel>),
    SideChat(Entity<side_chat::SideChatPanel>),
}

struct Tab {
    id: u64,
    tool: PanelTool,
    view: View,
}

pub struct RightPanel {
    workspace: Entity<Workspace>,
    pub open: bool,
    pub wide: bool,
    tabs: Vec<Tab>,
    active: Option<u64>,
    next_id: u64,
    launcher_open: bool,
}

impl RightPanel {
    pub fn new(workspace: Entity<Workspace>) -> Self {
        Self { workspace, open: false, wide: false, tabs: vec![], active: None, next_id: 1, launcher_open: false }
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
                let cwd = ws.read(cx).current_cwd();
                View::Terminal(cx.new(|cx| terminal::TerminalPanel::new(cwd, cx)))
            }
            PanelTool::Browser => View::Browser(cx.new(|cx| browser::BrowserPanel::new(window, cx))),
            PanelTool::SideChat => View::SideChat(cx.new(|cx| side_chat::SideChatPanel::new(ws, window, cx))),
        };
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab { id, tool, view });
        self.active = Some(id);
        self.after_activate(window, cx);
    }

    /// Run a setup command (install / sign in) in a fresh terminal tab, then rescan agents.
    pub fn run_command(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(PanelTool::Terminal, window, cx);
        let ws = self.workspace.downgrade();
        if let Some(Tab { view: View::Terminal(t), .. }) = self.active_tab() {
            t.update(cx, |t, _| {
                t.run_once(&command, move |cx| {
                    let _ = ws.update(cx, |ws, cx| {
                        ws.detect_agents(cx);
                    });
                })
            });
        }
    }

    fn after_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_native(cx);
        if let Some(Tab { view: View::Terminal(t), .. }) = self.active_tab() {
            let handle = t.read(cx).focus_handle();
            handle.focus(window, cx);
        }
        cx.notify();
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Tab { view: View::Browser(b), .. }) = self.tabs.iter().find(|t| t.id == id) {
            b.update(cx, |b, cx| b.set_visible(false, cx));
        }
        let ix = self.tabs.iter().position(|t| t.id == id);
        self.tabs.retain(|t| t.id != id);
        if self.active == Some(id) {
            self.active = ix.and_then(|i| self.tabs.get(i.saturating_sub(1)).or(self.tabs.first())).map(|t| t.id);
        }
        self.after_activate(window, cx);
    }

    /// Native web views draw above GPUI, so only the visible Browser tab may show.
    fn sync_native(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let open = self.open;
        let menu_open = self.launcher_open;
        for t in &self.tabs {
            if let View::Browser(b) = &t.view {
                // Hidden while the launcher menu is open, or it would cover the menu.
                let show = open && active == Some(t.id) && !menu_open;
                b.update(cx, |b, cx| b.set_visible(show, cx));
            }
        }
    }

    fn launcher_rows(&self, large: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        PanelTool::ALL
            .into_iter()
            .map(|tool| {
                let row = h_flex()
                    .id(SharedString::from(format!("launch-{}-{}", tool.label(), large)))
                    .gap(px(12.))
                    .px(px(14.))
                    .h(px(if large { 42. } else { 34. }))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .text_sm()
                    .when(large, |el| el.bg(theme.foreground.opacity(0.04)).border_1().border_color(theme.border))
                    .hover(|s| s.bg(theme.foreground.opacity(0.08)))
                    .child(tool_icon(tool).small().text_color(theme.muted_foreground))
                    .child(tool.label())
                    .on_click(cx.listener(move |this, _, window, cx| this.open_tool(tool, window, cx)));
                row.into_any_element()
            })
            .collect()
    }
}

impl Render for RightPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active;
        let tabs = h_flex()
            .id("panel-tabs")
            .flex_1()
            .min_w_0()
            .gap_1()
            .overflow_x_scroll()
            .children(self.tabs.iter().map(|t| {
                let id = t.id;
                let is_active = Some(id) == active;
                h_flex()
                    .id(("panel-tab", id as usize))
                    .group("panel-tab")
                    .h(px(28.))
                    .pl(px(10.))
                    .pr(px(4.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .flex_none()
                    .cursor_pointer()
                    .text_sm()
                    .when(is_active, |el| el.bg(theme.foreground.opacity(0.08)))
                    .when(!is_active, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.foreground.opacity(0.05))))
                    .child(tool_icon(t.tool).xsmall())
                    .child(t.tool.label())
                    .child(
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
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = Some(id);
                        this.after_activate(window, cx);
                    }))
            }));

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
                let rows = entity.update(cx, |p, cx| p.launcher_rows(false, cx));
                ui::menu_surface(cx).w(px(220.)).children(rows)
            });

        let header = h_flex()
            .h(px(44.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(tabs)
            .child(launcher)
            .child(ui::icon_button("panel-wide", if self.wide { IconName::Minimize } else { IconName::Maximize }, "Wider").on_click(cx.listener(|this, _, _, cx| {
                this.wide = !this.wide;
                cx.notify();
            })))
            .child(ui::icon_button("panel-close", IconName::PanelRightClose, "Hide panel (⌘J)").on_click(cx.listener(|this, _, _, cx| this.toggle(cx))));

        let body = match self.active_tab() {
            Some(t) => match &t.view {
                View::Git(v) => v.clone().into_any_element(),
                View::Explorer(v) => v.clone().into_any_element(),
                View::Terminal(v) => v.clone().into_any_element(),
                View::Browser(v) => v.clone().into_any_element(),
                View::SideChat(v) => v.clone().into_any_element(),
            },
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(v_flex().w(px(280.)).gap_2().children(self.launcher_rows(true, cx)))
                .into_any_element(),
        };

        v_flex().size_full().child(header).child(div().flex_1().min_h_0().child(body))
    }
}
