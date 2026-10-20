//! The chat's terminal (⌃`): a dock along the bottom of the thread column, under the composer,
//! with shells in the thread's folder. Each thread (and the draft) has its own: its shells, and
//! whether its dock is shown. Another thread's shells keep running while it isn't on screen, and
//! the draft's go with it when it's sent and becomes a thread. The composer's status row toggles
//! it, and it comes back as it was left (shown or not for the thread on screen at launch, and how
//! tall, which all threads share). The tools panel's Terminal tile still opens a shell beside the
//! thread; this one is under it, where T3 keeps its terminal.

use crate::panels::terminal::{Job, TerminalPanel, TitleChanged};
use crate::workspace::{Route, Workspace, WorkspaceEvent};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
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

/// Whose shells: a thread's, or the draft's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Owner {
    Thread(String),
    Draft,
}

/// The owner `route` shows a dock for; none off threads and drafts (settings, Basecamp, notes).
fn owner(route: &Route) -> Option<Owner> {
    match route {
        Route::Thread(id) => Some(Owner::Thread(id.clone())),
        Route::Draft { .. } => Some(Owner::Draft),
        _ => None,
    }
}

/// One thread's dock: its shells, the one in front, and whether it's shown.
#[derive(Default)]
struct Shells {
    open: bool,
    terminals: Vec<Entity<TerminalPanel>>,
    active: usize,
}

pub struct TerminalDock {
    workspace: Entity<Workspace>,
    /// Height in points (persisted in settings.layout); `fit_height` clamps what's drawn.
    pub height: f32,
    /// Each thread's shells (and the draft's), kept while another is on screen.
    docks: HashMap<Owner, Shells>,
    /// The owner last on screen: what the dock acts on off threads and drafts (Basecamp, notes).
    shown: Owner,
    /// Settings left the dock open and no thread or draft has been shown yet: the first one
    /// shown gets it, a shell started.
    launch_open: bool,
    /// Resize in progress: the pointer's y when the edge was grabbed, and the height then.
    drag: Option<(Pixels, f32)>,
    _route: Subscription,
}

impl TerminalDock {
    /// The dock as settings left it; a shell starts at once when it was left open.
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let layout = workspace.read(cx).settings.layout.clone();
        let _route = cx.observe(&workspace, |this, ws, cx| this.follow_route(&ws, cx));
        let mut this = Self {
            workspace: workspace.clone(),
            height: layout.terminal_height.max(MIN_TERMINAL_HEIGHT),
            docks: HashMap::new(),
            shown: Owner::Draft,
            launch_open: layout.terminal_open,
            drag: None,
            _route,
        };
        this.follow_route(&workspace, cx);
        this
    }

    /// The route moved (or anything else in the workspace changed): the thread now on screen
    /// shows its own dock. The draft's shells go to the thread it became; the first thread or
    /// draft shown gets the dock settings left open.
    fn follow_route(&mut self, ws: &Entity<Workspace>, cx: &mut Context<Self>) {
        let Some(now) = owner(&ws.read(cx).route) else { return };
        if now == self.shown && !self.launch_open {
            return;
        }
        if let (Owner::Draft, Owner::Thread(id)) = (&self.shown, &now) {
            if ws.read(cx).draft_became.as_deref() == Some(id.as_str()) && !self.docks.contains_key(&now) {
                if let Some(draft) = self.docks.remove(&Owner::Draft) {
                    self.docks.insert(now.clone(), draft);
                }
            }
        }
        self.shown = now;
        if std::mem::take(&mut self.launch_open) {
            let dock = self.docks.entry(self.shown.clone()).or_default();
            dock.open = true;
            if dock.terminals.is_empty() {
                self.new_terminal(cx);
            }
        }
        self.save(cx);
        cx.notify();
    }

    /// The owner the dock acts on: the thread (or draft) on screen, else the one last shown.
    fn current(&self, cx: &App) -> Owner {
        owner(&self.workspace.read(cx).route).unwrap_or_else(|| self.shown.clone())
    }

    fn dock(&self, cx: &App) -> Option<&Shells> {
        self.docks.get(&self.current(cx))
    }

    fn dock_mut(&mut self, cx: &App) -> &mut Shells {
        let owner = self.current(cx);
        self.docks.entry(owner).or_default()
    }

    /// Whether the thread on screen has its dock shown.
    pub fn is_open(&self, cx: &App) -> bool {
        self.dock(cx).is_some_and(|d| d.open)
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// ⌃` and the composer's Terminal: shown, or hidden when it's showing.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_open(cx) {
            self.hide(cx);
        } else {
            self.show(window, cx);
        }
    }

    /// Show the dock, a shell started if it has none, and give it the keys.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock_mut(cx).open = true;
        if self.dock_mut(cx).terminals.is_empty() {
            self.new_terminal(cx);
        }
        if let Some(t) = self.front(cx) {
            t.read(cx).focus_handle().focus(window, cx);
        }
        self.save(cx);
        cx.notify();
    }

    /// Hide the dock; its shells keep running. The keys go back to the composer.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.dock_mut(cx).open = false;
        self.save(cx);
        self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::FocusComposer));
        cx.notify();
    }

    /// Run a command (an agent install or sign-in, a project action) in a shell of its own in
    /// the dock of the thread on screen; agents are looked for again when it exits.
    pub fn run_command(&mut self, job: Job, cwd: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cwd.or_else(|| self.workspace.read(cx).current_cwd());
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        let ws = self.workspace.downgrade();
        let term = cx.new(|cx| {
            let mut t = TerminalPanel::with_command(ws.clone(), cwd, job, shell, cx);
            t.on_exit(move |cx| {
                let _ = ws.update(cx, |ws, cx| {
                    ws.acp_info.clear();
                    ws.detect_agents(cx);
                });
            });
            t
        });
        Self::follow_title(&term, cx);
        let dock = self.dock_mut(cx);
        dock.terminals.push(term);
        dock.active = dock.terminals.len() - 1;
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

    /// The shells of the thread on screen (tests).
    #[cfg(test)]
    pub fn terminals(&self, cx: &App) -> Vec<Entity<TerminalPanel>> {
        self.dock(cx).map(|d| d.terminals.clone()).unwrap_or_default()
    }

    /// How many shells the thread on screen has (tests).
    #[cfg(test)]
    pub fn count(&self, cx: &App) -> usize {
        self.dock(cx).map_or(0, |d| d.terminals.len())
    }

    /// The shell in front in the dock, while the dock is shown (the shots harness's `terminal`).
    #[cfg(feature = "shots")]
    pub fn shown(&self, cx: &App) -> Option<Entity<TerminalPanel>> {
        self.is_open(cx).then(|| self.front(cx)).flatten()
    }

    /// The shell in front in the thread on screen's dock.
    fn front(&self, cx: &App) -> Option<Entity<TerminalPanel>> {
        self.dock(cx).and_then(|d| d.terminals.get(d.active).cloned())
    }

    fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let cwd = self.workspace.read(cx).current_cwd();
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        let ws = self.workspace.downgrade();
        let term = cx.new(|cx| TerminalPanel::new(ws, cwd, shell, cx));
        Self::follow_title(&term, cx);
        let dock = self.dock_mut(cx);
        dock.terminals.push(term);
        dock.active = dock.terminals.len() - 1;
        cx.notify();
    }

    /// Redraw the tabs when `term`'s title changes.
    fn follow_title(term: &Entity<TerminalPanel>, cx: &mut Context<Self>) {
        cx.subscribe(term, |_, _, _: &TitleChanged, cx| cx.notify()).detach();
    }

    /// Close the shell in front; the dock goes with the last one.
    fn kill_terminal(&mut self, cx: &mut Context<Self>) {
        let dock = self.dock_mut(cx);
        if dock.active < dock.terminals.len() {
            dock.terminals.remove(dock.active);
        }
        dock.active = dock.active.min(dock.terminals.len().saturating_sub(1));
        if dock.terminals.is_empty() {
            self.hide(cx);
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (open, height) = (self.is_open(cx), self.height);
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
        let (count, active) = self.dock(cx).map_or((0, 0), |d| (d.terminals.len(), d.active));
        let many = count > 1;
        // What each shell is called: the title its shell or program set, else the shell's name.
        let titles: Vec<Option<String>> = self.dock(cx).map_or(vec![], |d| d.terminals.iter().map(|t| t.read(cx).title().map(str::to_string)).collect());

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
            .when(!many, |el| {
                el.children(titles.first().cloned().flatten().map(|title| {
                    div().id("terminal-dock-title").test_support().min_w_0().max_w(px(360.)).truncate().text_size(px(11.5)).text_color(theme.muted_foreground).child(title)
                }))
            })
            .when(many, |el| {
                // More than one shell: a tab each, the one in front underlined.
                el.children((0..count).map(|ix| {
                    let on = ix == active;
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
                        .child(div().max_w(px(180.)).truncate().child(titles.get(ix).cloned().flatten().unwrap_or_else(|| format!("{shell} {}", ix + 1))))
                        .when(on, |el| el.child(div().absolute().bottom(px(4.)).left(px(8.)).right(px(8.)).h(px(1.5)).bg(ember)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.dock_mut(cx).active = ix;
                            if let Some(t) = this.front(cx) {
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

        let body: AnyElement = match self.front(cx) {
            Some(t) => div().size_full().pl(px(10.)).child(t.cached(fill)).into_any_element(),
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
