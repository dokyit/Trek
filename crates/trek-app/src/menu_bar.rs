//! Trek's menus in the window, for Windows, which has no system menu bar: the same `menus()` data
//! macOS hands to the system (`windows_menus` rearranges it for Windows), drawn as titles at the
//! left of the title bar with a dropdown under the one that's open.
//!
//! Alt tapped alone takes the keyboard for the bar, with the first title lit and no menu open; Alt
//! with a letter opens the menu that letter marks. Each mark is underlined while Alt is held or the
//! bar has the keyboard. With the keyboard: the left and right arrows light another title, Down or
//! Return opens the lit one, and Escape closes an open menu and then leaves the bar. In a menu, up
//! and down move through the items, a letter chooses the item it marks, and Return runs the one lit.
//! A click on a title opens its menu, hovering another title moves to it while one is open, and a
//! click outside leaves. Choosing an item hands focus back to where it was and dispatches the
//! item's action from there, as its shortcut would. The dropdown comes in and leaves on
//! `motion::SURFACE` (`Presence`), and just appears and goes under Reduce motion. The arrows, Return
//! and Escape are keymap actions in the bar's own context (`keys.rs`), so they're the bar's only
//! while it has focus.

use crate::motion::{Presence, SURFACE};
use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What Windows calls the item that quits: macOS's "Quit Trek" is under the app's name there.
const EXIT: &str = "Exit";

actions!(menu_bar, [MenuLeft, MenuRight, MenuUp, MenuDown, MenuEnter, MenuEscape]);

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
    mark_mnemonics(&mut menus);
    menus
}

/// Marks the letter Alt presses in each title and each menu's items, as Windows's own resource
/// strings do: the first letter of a label that no label before it in its row has marked, with a
/// `&` before it. The titles are one row, and each menu's items another.
fn mark_mnemonics(menus: &mut [Menu]) {
    let mut titles = Vec::new();
    for menu in menus.iter_mut() {
        menu.name = mark(menu.name.as_ref(), &mut titles);
    }
    for menu in menus.iter_mut() {
        let mut letters = Vec::new();
        for item in &mut menu.items {
            if let MenuItem::Action { name, .. } = item {
                *name = mark(name.as_ref(), &mut letters);
            }
        }
    }
}

/// `label` with its first unmarked letter (`taken` has the ones marked, lower case) marked.
fn mark(label: &str, taken: &mut Vec<char>) -> SharedString {
    for (at, c) in label.char_indices() {
        let lower = c.to_ascii_lowercase();
        if c.is_ascii_alphabetic() && !taken.contains(&lower) {
            taken.push(lower);
            return format!("{}&{}", &label[..at], &label[at..]).into();
        }
    }
    label.to_string().into()
}

/// A label without its `&`, split around the letter the mark is on: the text before it, the letter,
/// and the text after. A label with no mark is all text before.
fn split_mnemonic(label: &str) -> (&str, Option<char>, &str) {
    let Some((before, rest)) = label.split_once('&') else { return (label, None, "") };
    match rest.chars().next() {
        Some(c) => (before, Some(c), &rest[c.len_utf8()..]),
        None => (before, None, ""),
    }
}

/// The letter a label marks, lower case, as a key is matched.
fn mnemonic(label: &str) -> Option<char> {
    split_mnemonic(label).1.map(|c| c.to_ascii_lowercase())
}

/// The letter a key is, when it's one ASCII letter, in lower case.
fn letter_of(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_lowercase())
}

/// A label as drawn, its marked letter underlined while `cues`. The underline is drawn over the
/// letter and takes no room, so the label lays out the same with it or without.
fn marked_label(marked: &str, cues: bool, id: (&'static str, usize), color: Hsla) -> AnyElement {
    let (before, letter, after) = split_mnemonic(marked);
    let Some(letter) = letter else { return div().whitespace_nowrap().child(before.to_string()).into_any_element() };
    h_flex()
        .whitespace_nowrap()
        .child(before.to_string())
        .child(
            div()
                .relative()
                .child(letter.to_string())
                .when(cues, |el| el.child(div().id(id).test_support().absolute().left_0().right_0().bottom(px(-2.)).h(px(1.)).bg(color))),
        )
        .child(after.to_string())
        .into_any_element()
}

/// A key as a menu shows it: "Ctrl+Shift+K" here, from the same formatter as every other label.
fn shortcut_label(keystroke: &gpui_kit::Keystroke) -> String {
    crate::keys::label_keystroke(keystroke)
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
    /// The row lit: under the pointer, or reached with the arrow keys.
    lit: Option<usize>,
}

pub struct MenuBar {
    workspace: Entity<Workspace>,
    menus: Vec<Menu>,
    /// Each menu's rows, made when the bar took the keyboard (`activate`): whether an item is on
    /// offer depends on where focus was then, not on the menu's own.
    entries: Vec<Vec<Entry>>,
    open: Presence<Open>,
    /// The title lit while the bar has the keyboard: the open menu's own, while one is open.
    title: usize,
    /// The bar has the keyboard: Alt was tapped, or a menu is open.
    active: bool,
    /// Alt is held, which underlines the mnemonics.
    alt: bool,
    /// A press came while Alt was held, so the release of Alt after it isn't a tap.
    pressed_with_alt: bool,
    /// The window the bar is in: keys in another window aren't its.
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    /// What had focus before the bar took it, to give it back.
    restore: Option<FocusHandle>,
    /// The menu a press outside it just closed. The press that closes a menu on its own title
    /// would otherwise open it again: the title hears the same press next.
    closed_by_press: Option<usize>,
    /// The keys that come before the focused element's (`intercept`).
    _keys: Subscription,
}

impl MenuBar {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        // A focused element only sees its own keys, and Alt has to reach the bar from anywhere in
        // its window: the window's keystrokes come here before anything else is done with them.
        let bar = cx.weak_entity();
        let keys = cx.intercept_keystrokes(move |ev, window, cx| {
            if let Some(bar) = bar.upgrade() {
                bar.update(cx, |bar, cx| bar.intercept(&ev.keystroke, window, cx));
            }
        });
        Self {
            workspace,
            menus: windows_menus(crate::menus()),
            entries: Vec::new(),
            open: Presence::new(SURFACE),
            title: 0,
            active: false,
            alt: false,
            pressed_with_alt: false,
            window: None,
            focus: cx.focus_handle(),
            restore: None,
            closed_by_press: None,
            _keys: keys,
        }
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

    /// The menu that's open, not one on its way out.
    fn open_menu(&self) -> Option<&Open> {
        if self.open.is_present() { self.open.item() } else { None }
    }

    /// Whether the mnemonics show: Alt is held, or the bar has the keyboard.
    fn cues(&self) -> bool {
        self.alt || self.active
    }

    fn first_title(&self) -> usize {
        self.menus.iter().position(|m| !m.disabled).unwrap_or(0)
    }

    /// Takes the keyboard for the bar, with the first title lit. The rows are made now, while the
    /// focus that had them is still in place, and stay as they are until the bar takes it again.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        self.restore = window.focused(cx).filter(|f| *f != self.focus);
        self.entries = (0..self.menus.len()).map(|ix| self.resolve(ix, window, cx)).collect();
        self.title = self.first_title();
        self.active = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Opens the menu of title `ix`, taking the keyboard for the bar if it hasn't it.
    fn open_title(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.menus.get(ix).is_none_or(|m| m.disabled) {
            return;
        }
        self.activate(window, cx);
        self.title = ix;
        let motion = self.motion(cx);
        self.open.enter(Open { menu: ix, lit: None }, motion, crate::motion::now(cx));
        cx.notify();
    }

    /// Puts the open menu away and keeps the keyboard: the next Escape leaves the bar.
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        let motion = self.motion(cx);
        self.open.exit(motion, crate::motion::now(cx));
        cx.notify();
    }

    /// Gives the keyboard up, and any menu with it. Focus goes back to what had it, unless something
    /// else has taken it since.
    fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.active = false;
        self.close_menu(cx);
        if window.focused(cx).is_none_or(|f| f == self.focus) {
            match self.restore.take() {
                Some(previous) => window.focus(&previous, cx),
                None => window.blur(cx),
            }
        }
        self.restore = None;
        cx.notify();
    }

    /// Light the title `by` places along (round the ends), or with a menu open, open that one.
    fn step_title(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.menus.len() as isize;
        for k in 1..=count {
            let ix = (self.title as isize + by * k).rem_euclid(count) as usize;
            if !self.menus[ix].disabled {
                if self.open.is_present() {
                    self.open_title(ix, window, cx);
                } else {
                    self.title = ix;
                    cx.notify();
                }
                return;
            }
        }
    }

    /// Light the next (or previous) item that can be chosen.
    fn step_item(&mut self, by: isize, cx: &mut Context<Self>) {
        if !self.open.is_present() {
            return;
        }
        let Some(open) = self.open.item_mut() else { return };
        let Some(entries) = self.entries.get(open.menu) else { return };
        let count = entries.len() as isize;
        let mut ix = open.lit.map_or(if by > 0 { -1 } else { count }, |l| l as isize);
        for _ in 0..count {
            ix = (ix + by).rem_euclid(count);
            if entries[ix as usize].enabled() {
                open.lit = Some(ix as usize);
                break;
            }
        }
        cx.notify();
    }

    fn light(&mut self, row: usize, cx: &mut Context<Self>) {
        if !self.open.is_present() {
            return;
        }
        let Some(open) = self.open.item_mut() else { return };
        let enabled = self.entries.get(open.menu).and_then(|rows| rows.get(row)).is_some_and(Entry::enabled);
        if open.lit != Some(row) && enabled {
            open.lit = Some(row);
            cx.notify();
        }
    }

    fn choose(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open_menu() else { return };
        let Some(Entry::Item { action, enabled: true, .. }) = self.entries.get(open.menu).and_then(|rows| rows.get(row)) else { return };
        let action = action.boxed_clone();
        self.leave(window, cx);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
    }

    /// Alt, from anywhere in the bar's window, before the focused element sees it: a tap of Alt
    /// (GPUI reports one as "alt" when nothing else was pressed while Alt was held), and Alt with
    /// a letter, which opens the menu that letter marks.
    fn intercept(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        if self.window != Some(window.window_handle()) {
            return;
        }
        let m = key.modifiers;
        if key.key == "alt" && m == Modifiers::none() {
            // A press with Alt held, a click say, isn't a tap of it.
            if std::mem::take(&mut self.pressed_with_alt) {
                return;
            }
            if self.active {
                self.leave(window, cx);
            } else {
                self.activate(window, cx);
            }
        } else if m.alt && !m.control && !m.shift && !m.platform && !m.function {
            if let Some(ix) = letter_of(&key.key).and_then(|letter| self.title_for(letter)) {
                self.open_title(ix, window, cx);
                cx.stop_propagation();
            }
        }
    }

    /// A letter with the bar's keyboard: in a menu, the item it marks is chosen; with none open,
    /// the menu of the title it marks opens.
    fn letter(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.modifiers.modified() {
            return;
        }
        let Some(letter) = letter_of(&ev.keystroke.key) else { return };
        if self.open.is_present() {
            if let Some(row) = self.row_for(letter) {
                self.choose(row, window, cx);
            }
        } else if let Some(ix) = self.title_for(letter) {
            self.open_title(ix, window, cx);
        }
        cx.stop_propagation();
    }

    /// The enabled title that `letter` marks.
    fn title_for(&self, letter: char) -> Option<usize> {
        self.menus.iter().position(|m| !m.disabled && mnemonic(&m.name) == Some(letter))
    }

    /// The enabled row of the open menu that `letter` marks.
    fn row_for(&self, letter: char) -> Option<usize> {
        let open = self.open_menu()?;
        self.entries.get(open.menu)?.iter().position(|e| matches!(e, Entry::Item { label, enabled: true, .. } if mnemonic(label) == Some(letter)))
    }

    /// The window's modifiers changed: Alt held underlines the mnemonics.
    pub fn modifiers_changed(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        if modifiers.alt && !self.alt {
            self.pressed_with_alt = false;
        }
        if self.alt != modifiers.alt {
            self.alt = modifiers.alt;
            cx.notify();
        }
    }

    /// A mouse button went down in the window: with Alt held, that's a press with Alt.
    pub fn mouse_pressed(&mut self) {
        if self.alt {
            self.pressed_with_alt = true;
        }
    }

    fn title(&self, ix: usize, lit: Option<usize>, cues: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let menu = &self.menus[ix];
        let on = lit == Some(ix);
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
            .child(marked_label(&menu.name, cues, ("mnemonic-title", ix), theme.foreground))
            // In the title bar's drag area, a press that isn't taken is a window move.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if this.closed_by_press.take() == Some(ix) {
                        return;
                    }
                    if this.open_menu().is_some_and(|o| o.menu == ix) {
                        this.leave(window, cx);
                    } else {
                        this.open_title(ix, window, cx);
                    }
                }),
            )
            // With one menu open, the pointer passing over another title opens that one.
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered && this.open_menu().is_some_and(|o| o.menu != ix) {
                    this.open_title(ix, window, cx);
                }
            }))
    }

    fn dropdown(&self, open: &Open, t: f32, leaving: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ix = open.menu;
        let cues = self.cues();
        let entries = self.entries.get(ix).map_or(&[][..], Vec::as_slice);
        let rows = entries.iter().enumerate().map(|(row, entry)| match entry {
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
                    .when(enabled, |el| el.text_color(theme.foreground).cursor_pointer().when(lit, |el| el.bg(theme.foreground.opacity(0.09))))
                    .child(marked_label(label, cues, ("mnemonic-item", row), theme.foreground))
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
                if this.open_menu().is_some_and(|o| o.menu == ix) {
                    this.closed_by_press = Some(ix);
                    this.leave(window, cx);
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
        self.window = Some(window.window_handle());
        let t = self.open.sample(crate::motion::now(cx), window);
        let leaving = !self.open.is_present();
        let lit = self.active.then_some(self.title);
        let cues = self.cues();
        // The bar has the keyboard but no menu is open: a press outside it gives the keyboard up.
        let idle = self.active && leaving;
        let mut bar = h_flex()
            .id("menu-bar")
            .test_support()
            .key_context("MenuBar")
            .track_focus(&self.focus)
            .when(idle, |el| el.on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.leave(window, cx))))
            .on_action(cx.listener(|this, _: &MenuLeft, window, cx| {
                this.step_title(-1, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &MenuRight, window, cx| {
                this.step_title(1, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &MenuUp, _, cx| {
                this.step_item(-1, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &MenuDown, window, cx| {
                if this.open.is_present() {
                    this.step_item(1, cx);
                } else {
                    let ix = this.title;
                    this.open_title(ix, window, cx);
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &MenuEnter, window, cx| {
                match this.open_menu().map(|o| o.lit) {
                    Some(Some(row)) => this.choose(row, window, cx),
                    Some(None) => {}
                    None => {
                        let ix = this.title;
                        this.open_title(ix, window, cx);
                    }
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &MenuEscape, window, cx| {
                if this.open.is_present() {
                    this.close_menu(cx);
                } else {
                    this.leave(window, cx);
                }
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.letter(ev, window, cx)))
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(1.));
        for ix in 0..self.menus.len() {
            let dropdown = match (t, self.open.item()) {
                (Some(t), Some(o)) if o.menu == ix => Some(self.dropdown(o, t, leaving, cx)),
                _ => None,
            };
            bar = bar.child(div().relative().flex_none().child(self.title(ix, lit, cues, cx)).children(dropdown));
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it carries gpui's `test` macro, which would take `#[test]`'s place.
    use super::{mnemonic, split_mnemonic, windows_menus};
    use gpui_kit::{Menu, MenuItem};

    /// A label as it reads, its mark taken out.
    fn names(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|i| match i {
                MenuItem::Separator => "-".to_string(),
                MenuItem::Action { name, .. } => name.replacen('&', "", 1),
                _ => "?".to_string(),
            })
            .collect()
    }

    /// The letter each item of a menu marks, in order.
    fn letters(menu: &Menu) -> Vec<Option<char>> {
        menu.items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { name, .. } => Some(mnemonic(name)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn windows_has_the_apps_items_under_file_and_help() {
        let menus = windows_menus(crate::menus());
        let titles: Vec<_> = menus.iter().map(|m| m.name.replacen('&', "", 1)).collect();
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

    #[test]
    fn each_title_and_each_items_row_marks_its_own_first_free_letters() {
        let menus = windows_menus(crate::menus());
        let titles: Vec<_> = menus.iter().map(|m| mnemonic(&m.name)).collect();
        assert_eq!(titles, [Some('f'), Some('t'), Some('v'), Some('w'), Some('h')]);
        // File: New Thread, Open Folder…, Settings…, Exit.
        assert_eq!(letters(&menus[0]), [Some('n'), Some('o'), Some('s'), Some('e')]);
        // Thread: Settle and Toggle take the letters Open and Cycle don't; Stop has S, T and O taken.
        assert_eq!(letters(&menus[1]), [Some('o'), Some('s'), Some('t'), Some('c'), Some('p')]);
        // View: Switch has S taken, so its w; Toggle Tools Panel has T taken, so its o.
        assert_eq!(letters(&menus[2]), [Some('s'), Some('g'), Some('w'), Some('b'), Some('t'), Some('o')]);
        assert_eq!(letters(&menus[3]), [Some('m'), Some('c')]);
        assert_eq!(letters(&menus[4]), [Some('a'), Some('c')]);
        for menu in &menus {
            let mut seen = letters(menu);
            let all = seen.len();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), all, "{}: a letter is marked twice", menu.name.replacen('&', "", 1));
        }
    }

    #[test]
    fn a_mark_comes_out_of_the_text_and_splits_it_around_its_letter() {
        assert_eq!(split_mnemonic("Sto&p Agent"), ("Sto", Some('p'), " Agent"));
        assert_eq!(split_mnemonic("&File"), ("", Some('F'), "ile"));
        assert_eq!(split_mnemonic("Exit"), ("Exit", None, ""));
        assert_eq!(mnemonic("&File"), Some('f'));
        assert_eq!(mnemonic("Exit"), None);
    }
}
