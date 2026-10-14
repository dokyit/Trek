//! The chat's terminal (⌃`): a dock along the bottom of the thread column, under the composer,
//! with shells in the thread's folder. The composer's status row toggles it, and it comes back
//! as it was left (shown or not, and how tall). The tools panel's Terminal tile still opens a
//! shell beside the thread; this one is under it, where T3 keeps its terminal.

use crate::panels::terminal::{Job, TerminalPanel};
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::{DEFAULT_TERMINAL_HEIGHT, MIN_TERMINAL_HEIGHT};

/// The dock takes at most this much of the window.
const MAX_SHARE: f32 = 0.6;
/// The header row (tabs and buttons).
const HEADER: f32 = 30.;

/// The dock's height as drawn in a window `viewport` tall: never under its minimum, never over
/// most of the window.
pub fn fit_height(wanted: f32, viewport: f32) -> f32 {
    let max = (viewport * MAX_SHARE).max(MIN_TERMINAL_HEIGHT);
    wanted.clamp(MIN_TERMINAL_HEIGHT, max)
}

pub struct TerminalDock {
    workspace: Entity<Workspace>,
    pub open: bool,
    /// Height in points (persisted in settings.layout); `fit_height` clamps what's drawn.
    pub height: f32,
    terminals: Vec<Entity<TerminalPanel>>,
    active: usize,
    /// Resize in progress: the pointer's y when the edge was grabbed, and the height then.
    drag: Option<(Pixels, f32)>,
}

impl TerminalDock {
    /// The dock as settings left it; a shell starts at once when it was left open.
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let layout = workspace.read(cx).settings.layout.clone();
        let mut this = Self { workspace, open: layout.terminal_open, height: layout.terminal_height.max(MIN_TERMINAL_HEIGHT), terminals: vec![], active: 0, drag: None };
        if this.open {
            this.new_terminal(cx);
        }
        this
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// ⌃` and the composer's Terminal: shown, or hidden when it's showing.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.hide(cx);
        } else {
            self.show(window, cx);
        }
    }

    /// Show the dock, a shell started if it has none, and give it the keys.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        if self.terminals.is_empty() {
            self.new_terminal(cx);
        }
        if let Some(t) = self.terminals.get(self.active) {
            t.read(cx).focus_handle().focus(window, cx);
        }
        self.save(cx);
        cx.notify();
    }

    /// Hide the dock; its shells keep running. The keys go back to the composer.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.save(cx);
        self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::FocusComposer));
        cx.notify();
    }

    /// Run a command (an agent install or sign-in, a project action) in a shell of its own in
    /// the dock; agents are looked for again when it exits.
    pub fn run_command(&mut self, job: Job, cwd: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cwd.or_else(|| self.workspace.read(cx).current_cwd());
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        let ws = self.workspace.downgrade();
        let term = cx.new(|cx| {
            let mut t = TerminalPanel::with_command(cwd, job, shell, cx);
            t.on_exit(move |cx| {
                let _ = ws.update(cx, |ws, cx| {
                    ws.acp_info.clear();
                    ws.detect_agents(cx);
                });
            });
            t
        });
        self.terminals.push(term);
        self.active = self.terminals.len() - 1;
        self.show(window, cx);
    }

    /// Set and persist the height (`fit_height` clamps what's drawn to the window).
    pub fn set_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.height = height.max(MIN_TERMINAL_HEIGHT);
        self.save(cx);
        cx.notify();
    }

    /// The edge, held, followed the pointer to `y` in a window `viewport` tall.
    pub fn drag_to(&mut self, y: Pixels, viewport: f32, cx: &mut Context<Self>) {
        let Some((y0, h0)) = self.drag else { return };
        self.height = fit_height(h0 + (y0 - y).as_f32(), viewport);
        cx.notify();
    }

    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            let h = self.height;
            self.set_height(h, cx);
        }
    }

    /// How many shells the dock holds (tests).
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.terminals.len()
    }

    fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let cwd = self.workspace.read(cx).current_cwd();
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        self.terminals.push(cx.new(|cx| TerminalPanel::new(cwd, shell, cx)));
        self.active = self.terminals.len() - 1;
        cx.notify();
    }

    /// Close the shell in front; the dock goes with the last one.
    fn kill_terminal(&mut self, cx: &mut Context<Self>) {
        if self.active < self.terminals.len() {
            self.terminals.remove(self.active);
        }
        self.active = self.active.min(self.terminals.len().saturating_sub(1));
        if self.terminals.is_empty() {
            self.hide(cx);
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (open, height) = (self.open, self.height);
        self.workspace.update(cx, |ws, cx| {
            if ws.settings.layout.terminal_open != open || ws.settings.layout.terminal_height != height {
                ws.settings.layout.terminal_open = open;
                ws.settings.layout.terminal_height = height;
                ws.save_settings(cx);
            }
        });
    }

    /// The grab strip along the dock's top edge: drag to resize, double-click for the default.
    fn handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.drag.is_some();
        div()
            .id("terminal-dock-resize")
            .absolute()
            .left_0()
            .right_0()
            .top(px(-2.))
            .h(px(5.))
            .cursor(CursorStyle::ResizeUpDown)
            .group("terminal-dock-resize")
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(1.))
                    .h(px(2.))
                    .when(active, |el| el.bg(crate::palette::ember(cx).opacity(0.7)))
                    .when(!active, |el| el.group_hover("terminal-dock-resize", |s| s.bg(theme.foreground.opacity(0.18)))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        this.set_height(DEFAULT_TERMINAL_HEIGHT, cx);
                        return;
                    }
                    this.drag = Some((e.position.y, this.height));
                    cx.notify();
                }),
            )
    }
}

impl Render for TerminalDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("TerminalDock");
        let theme = cx.theme().clone();
        let glass = self.workspace.read(cx).glass();
        let line = if glass.is_some() { crate::ui::panel_border(glass, cx) } else { theme.border };
        let height = fit_height(self.height, window.viewport_size().height.as_f32());
        let shell = crate::panels::terminal::shell_name(&self.workspace.read(cx).settings.terminal.shell);
        let ember = crate::palette::ember(cx);
        let fill = StyleRefinement::default().size_full();
        let many = self.terminals.len() > 1;

        let header = h_flex()
            .h(px(HEADER))
            .flex_none()
            .px(px(10.))
            .gap(px(4.))
            .items_center()
            .child(
                h_flex()
                    .relative()
                    .h_full()
                    .px(px(8.))
                    .items_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("TERMINAL")
                    .when(!many, |el| el.child(div().absolute().bottom(px(4.)).left(px(8.)).right(px(8.)).h(px(1.5)).bg(ember))),
            )
            .when(many, |el| {
                // More than one shell: a tab each, the one in front underlined.
                el.children(self.terminals.iter().enumerate().map(|(ix, _)| {
                    let on = ix == self.active;
                    h_flex()
                        .id(("terminal-dock-tab", ix))
                        .test_support()
                        .relative()
                        .h_full()
                        .px(px(8.))
                        .gap(px(5.))
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(if on { theme.foreground } else { theme.muted_foreground })
                        .when(!on, |el| el.hover(|s| s.text_color(theme.foreground)))
                        .child(Icon::new(IconName::SquareTerminal).xsmall())
                        .child(format!("{shell} {}", ix + 1))
                        .when(on, |el| el.child(div().absolute().bottom(px(4.)).left(px(8.)).right(px(8.)).h(px(1.5)).bg(ember)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.active = ix;
                            if let Some(t) = this.terminals.get(ix) {
                                t.read(cx).focus_handle().focus(window, cx);
                            }
                            cx.notify();
                        }))
                }))
            })
            .child(div().flex_1())
            .child(crate::ui::icon_button("terminal-dock-new", IconName::Plus, "New terminal").on_click(cx.listener(|this, _, window, cx| {
                this.new_terminal(cx);
                this.show(window, cx);
            })))
            .child(crate::ui::icon_button("terminal-dock-kill", crate::assets::Lucide::Trash, "Kill terminal").on_click(cx.listener(|this, _, _, cx| this.kill_terminal(cx))))
            .child(
                crate::ui::icon_button("terminal-dock-close", IconName::Close, format!("Hide terminal ({})", crate::keys::hint(crate::keys::Id::ToggleTerminal)))
                    .on_click(cx.listener(|this, _, _, cx| this.hide(cx))),
            );

        let body: AnyElement = match self.terminals.get(self.active) {
            Some(t) => div().size_full().pl(px(10.)).child(t.clone().cached(fill)).into_any_element(),
            None => crate::panels::empty("No terminal. + starts one in this folder.", cx).into_any_element(),
        };

        v_flex()
            .id("terminal-dock")
            .test_support()
            .relative()
            .w_full()
            .h(px(height))
            .flex_none()
            .border_t_1()
            .border_color(line)
            .bg(crate::ui::chrome_bg(glass, cx))
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .child(self.handle(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::fit_height;
    use trek_core::settings::MIN_TERMINAL_HEIGHT;

    #[test]
    fn the_dock_stays_inside_the_window() {
        assert_eq!(fit_height(240., 820.), 240.);
        assert_eq!(fit_height(20., 820.), MIN_TERMINAL_HEIGHT);
        assert_eq!(fit_height(5000., 820.), 820. * 0.6);
        // A tiny window: the minimum wins over the share.
        assert_eq!(fit_height(5000., 100.), MIN_TERMINAL_HEIGHT);
    }
}
