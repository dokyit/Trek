//! Trek's menus in the window, for Windows, which has no system menu bar: the same `menus()` data
//! macOS hands to the system (`windows_menus` rearranges it for Windows), drawn as titles at the
//! left of the title bar with a dropdown under the one that's open.
//!
//! A click on a title opens its menu; with one open, hovering another title or the left and right
//! arrows move to it; up and down move through the items, Return runs the one lit, Escape or a
//! click outside puts the menu away. Choosing an item hands focus back to where it was and
//! dispatches the item's action from there, as its shortcut would. The dropdown comes in and
//! leaves on `motion::SURFACE` (`Presence`), and just appears and goes under Reduce motion.

use crate::motion::{Presence, SURFACE};
use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What Windows calls the item that quits: macOS's "Quit Trek" is under the app's name there.
const EXIT: &str = "Exit";

/// `menus` (macOS's, which `main` also hands to the system) as Windows shows them: macOS puts the
/// app's own items under a menu named for the app; Windows has them under File and Help, and has
/// no Hide.
pub fn windows_menus(menus: Vec<Menu>) -> Vec<Menu> {
    let mut app = Vec::new();
    let mut rest = Vec::new();
    for menu in menus {
        if menu.name.as_ref() == "Trek" { app.extend(menu.items) } else { rest.push(menu) }
    }
    let (mut file, mut help) = (Vec::new(), Vec::new());
    for mut item in app {
        let MenuItem::Action { action, name, .. } = &mut item else { continue };
        if action.partial_eq(&crate::HideApp) {
            continue;
        }
        if action.partial_eq(&crate::About) || action.partial_eq(&crate::CheckForUpdates) {
            help.push(item);
        } else {
            if action.partial_eq(&crate::Quit) {
                *name = EXIT.into();
            }
            file.push(item);
        }
    }
    // File: its own items, then Settings, then Exit, each set off by a rule as the app menu had
    // them (Settings, then Quit).
    let (exit, settings): (Vec<_>, Vec<_>) = file.into_iter().partition(|i| matches!(i, MenuItem::Action { action, .. } if action.partial_eq(&crate::Quit)));
    let mut tail = [settings, exit];
    let mut menus = Vec::new();
    for mut menu in rest {
        if menu.name.as_ref() == "File" {
            for group in tail.iter_mut().map(std::mem::take).filter(|g| !g.is_empty()) {
                menu.items.push(MenuItem::separator());
                menu.items.extend(group);
            }
        }
        menus.push(menu);
    }
    if !help.is_empty() {
        menus.push(Menu { name: "Help".into(), items: help, disabled: false });
    }
    menus
}

/// A key as a menu shows it. keys.rs replaces this (the keystroke formatter lives there).
fn shortcut_label(keystroke: &gpui_kit::Keystroke) -> String {
    // keys.rs replaces this
    keystroke.to_string()
}

/// A row of an open menu, as it was when the menu opened: whether an item is on offer depends
/// on where focus was then, not on the menu's own.
enum Entry {
    Separator,
    Item { label: SharedString, action: Box<dyn Action>, shortcut: Option<String>, enabled: bool },
}

impl Entry {
    fn enabled(&self) -> bool {
        matches!(self, Entry::Item { enabled: true, .. })
    }
}

/// The menu that's open (or leaving).
struct Open {
    menu: usize,
    entries: Vec<Entry>,
    /// The row lit: under the pointer, or reached with the arrow keys.
    lit: Option<usize>,
}

pub struct MenuBar {
    workspace: Entity<Workspace>,
    menus: Vec<Menu>,
    open: Presence<Open>,
    focus: FocusHandle,
    /// What had focus before a menu opened, to give it back.
    restore: Option<FocusHandle>,
    /// The menu a press outside it just closed. The press that closes a menu on its own title
    /// would otherwise open it again: the title hears the same press next.
    closed_by_press: Option<usize>,
}

impl MenuBar {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self { workspace, menus: windows_menus(crate::menus()), open: Presence::new(SURFACE), focus: cx.focus_handle(), restore: None, closed_by_press: None }
    }

    fn motion(&self, cx: &App) -> bool {
        self.workspace.read(cx).motion(cx)
    }

    fn resolve(&self, ix: usize, window: &Window, cx: &mut App) -> Vec<Entry> {
        let mut entries = Vec::new();
        for item in &self.menus[ix].items {
            match item {
                MenuItem::Separator => {
                    // No rule at the top, at the bottom or twice in a row.
                    if !matches!(entries.last(), None | Some(Entry::Separator)) {
                        entries.push(Entry::Separator);
                    }
                }
                MenuItem::Action { name, action, disabled, .. } => {
                    let shortcut = window.highest_precedence_binding_for_action(action.as_ref()).map(|b| b.keystrokes().iter().map(|k| shortcut_label(k.inner())).collect::<Vec<_>>().join(" "));
                    let enabled = !*disabled && (window.is_action_available(action.as_ref(), cx) || cx.is_action_available(action.as_ref()));
                    entries.push(Entry::Item { label: name.clone(), action: action.boxed_clone(), shortcut, enabled });
                }
                MenuItem::Submenu(_) | MenuItem::SystemMenu(_) => {}
            }
        }
        while matches!(entries.last(), Some(Entry::Separator)) {
            entries.pop();
        }
        entries
    }

    fn open_menu_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.menus.get(ix).is_none_or(|m| m.disabled) {
            return;
        }
        if !self.open.is_present() {
            self.restore = window.focused(cx).filter(|f| *f != self.focus);
        }
        let entries = self.resolve(ix, window, cx);
        let motion = self.motion(cx);
        self.open.enter(Open { menu: ix, entries, lit: None }, motion, crate::motion::now(cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open.is_present() {
            return;
        }
        let motion = self.motion(cx);
        self.open.exit(motion, crate::motion::now(cx));
        // Focus goes back to what had it, unless something else has taken it since.
        if window.focused(cx).is_none_or(|f| f == self.focus) {
            match self.restore.take() {
                Some(previous) => window.focus(&previous, cx),
                None => window.blur(cx),
            }
        }
        self.restore = None;
        cx.notify();
    }

    /// Move to the menu `by` places along (round the ends).
    fn step_menu(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.open.item().map(|o| o.menu) else { return };
        let count = self.menus.len() as isize;
        for k in 1..=count {
            let ix = (from as isize + by * k).rem_euclid(count) as usize;
            if !self.menus[ix].disabled {
                self.open_menu_at(ix, window, cx);
                return;
            }
        }
    }

    /// Light the next (or previous) item that can be chosen.
    fn step_item(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(open) = self.open.item_mut() else { return };
        let count = open.entries.len() as isize;
        let mut ix = open.lit.map_or(if by > 0 { -1 } else { count }, |l| l as isize);
        for _ in 0..count {
            ix = (ix + by).rem_euclid(count);
            if open.entries[ix as usize].enabled() {
                open.lit = Some(ix as usize);
                break;
            }
        }
        cx.notify();
    }

    fn light(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.item_mut()
            && open.lit != Some(row)
            && open.entries.get(row).is_some_and(Entry::enabled)
        {
            open.lit = Some(row);
            cx.notify();
        }
    }

    fn choose(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::Item { action, enabled: true, .. }) = self.open.item().and_then(|o| o.entries.get(row)) else { return };
        let action = action.boxed_clone();
        self.close(window, cx);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open.is_present() || ev.keystroke.modifiers.modified() {
            return;
        }
        match ev.keystroke.key.as_str() {
            "escape" => self.close(window, cx),
            "left" => self.step_menu(-1, window, cx),
            "right" => self.step_menu(1, window, cx),
            "down" => self.step_item(1, cx),
            "up" => self.step_item(-1, cx),
            "enter" => {
                if let Some(row) = self.open.item().and_then(|o| o.lit) {
                    self.choose(row, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn title(&self, ix: usize, open: Option<usize>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let menu = &self.menus[ix];
        let on = open == Some(ix);
        div()
            .id(("menu-title", ix))
            .test_support()
            .h(px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(px(12.5))
            .when(menu.disabled, |el| el.text_color(theme.muted_foreground.opacity(0.5)))
            .when(!menu.disabled, |el| {
                el.cursor_pointer()
                    .when(on, |el| el.bg(theme.foreground.opacity(0.09)).text_color(theme.foreground))
                    .when(!on, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.foreground.opacity(0.05)).text_color(theme.foreground)))
            })
            .child(menu.name.clone())
            // In the title bar's drag area, a press that isn't taken is a window move.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if this.closed_by_press.take() == Some(ix) {
                        return;
                    }
                    if this.open.is_present() && this.open.item().is_some_and(|o| o.menu == ix) {
                        this.close(window, cx);
                    } else {
                        this.open_menu_at(ix, window, cx);
                    }
                }),
            )
            // With one menu open, the pointer passing over another title opens that one.
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered && this.open.is_present() && this.open.item().is_some_and(|o| o.menu != ix) {
                    this.open_menu_at(ix, window, cx);
                }
            }))
    }

    fn dropdown(&self, open: &Open, t: f32, leaving: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ix = open.menu;
        let rows = open.entries.iter().enumerate().map(|(row, entry)| match entry {
            Entry::Separator => div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(theme.foreground.opacity(0.08)).into_any_element(),
            Entry::Item { label, shortcut, enabled, .. } => {
                let (enabled, lit) = (*enabled, open.lit == Some(row));
                h_flex()
                    .id(("menu-item", row))
                    .test_support()
                    .min_h(px(28.))
                    .px(px(10.))
                    .gap(px(24.))
                    .justify_between()
                    .rounded(px(7.))
                    .text_size(px(12.5))
                    .when(!enabled, |el| el.text_color(theme.foreground.opacity(0.35)))
                    .when(enabled, |el| el.text_color(theme.foreground).cursor_pointer().when(lit, |el| el.bg(theme.list_active)))
                    .child(div().whitespace_nowrap().child(label.clone()))
                    .when_some(shortcut.clone(), |el, s| el.child(div().whitespace_nowrap().text_size(px(11.5)).text_color(theme.muted_foreground).child(s)))
                    .when(enabled, |el| {
                        el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.light(row, cx);
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| this.choose(row, window, cx)))
                    })
                    .into_any_element()
            }
        });
        let surface = crate::ui::menu_surface(cx)
            .id("menu-dropdown")
            .test_support()
            .min_w(px(220.))
            .p(px(4.))
            .rounded(px(10.))
            .opacity(t)
            .mt(px(2. - 4. * (1. - t)))
            .occlude()
            .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                // Only the menu this was drawn for: by the time it runs, a title's press may
                // have moved on to another.
                if this.open.is_present() && this.open.item().is_some_and(|o| o.menu == ix) {
                    this.closed_by_press = Some(ix);
                    this.close(window, cx);
                    // Good for this press alone.
                    let this = cx.entity();
                    window.defer(cx, move |_, cx| this.update(cx, |this, _| this.closed_by_press = None));
                }
            }))
            .children(rows);
        let surface = if leaving { crate::motion::inert(surface).into_any_element() } else { surface.into_any_element() };
        deferred(div().absolute().top(relative(1.)).left_0().child(surface)).with_priority(1).into_any_element()
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.open.sample(crate::motion::now(cx), window);
        let leaving = !self.open.is_present();
        let open = self.open.item().map(|o| o.menu);
        let mut bar = h_flex()
            .id("menu-bar")
            .key_context("MenuBar")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.key(ev, window, cx)))
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(1.));
        for ix in 0..self.menus.len() {
            let dropdown = match (t, self.open.item()) {
                (Some(t), Some(o)) if o.menu == ix => Some(self.dropdown(o, t, leaving, cx)),
                _ => None,
            };
            bar = bar.child(div().relative().flex_none().child(self.title(ix, open.filter(|_| !leaving), cx)).children(dropdown));
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it carries gpui's `test` macro, which would take `#[test]`'s place.
    use super::windows_menus;
    use gpui_kit::{Menu, MenuItem};

    fn names(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|i| match i {
                MenuItem::Separator => "-".to_string(),
                MenuItem::Action { name, .. } => name.to_string(),
                _ => "?".to_string(),
            })
            .collect()
    }

    #[test]
    fn windows_has_the_apps_items_under_file_and_help() {
        let menus = windows_menus(crate::menus());
        let titles: Vec<_> = menus.iter().map(|m| m.name.to_string()).collect();
        assert_eq!(titles, ["File", "Thread", "View", "Window", "Help"]);
        let file = names(&menus[0]);
        assert_eq!(file, ["New Thread", "Open Folder…", "-", "Settings…", "-", "Exit"]);
        assert_eq!(names(&menus[4]), ["About Trek", "Check for Updates…"]);
        let all: Vec<_> = menus.iter().flat_map(names).collect();
        assert!(!all.iter().any(|n| n.contains("Hide")), "no Hide on Windows: {all:?}");
    }

    #[test]
    fn nothing_is_lost_or_added_but_what_macos_has() {
        // Every action in macOS's menus is in Windows's, bar Hide.
        let count = |menus: &[Menu]| menus.iter().flat_map(|m| &m.items).filter(|i| matches!(i, MenuItem::Action { .. })).count();
        assert_eq!(count(&windows_menus(crate::menus())), count(&crate::menus()) - 1);
    }
}
