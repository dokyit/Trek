//! Two threads side by side in the main window. The route stays what it always was, the thread the
//! user is on, so it moves with the focus: the sidebar's selection, the title bar, the tools panel,
//! the terminal dock, the palette and the window's shortcuts follow the focused pane without
//! knowing there is a split. What a split changes is what the views are bound to: `Scope::Main` is
//! the left pane and holds its thread while the focus is across the divider, and `Scope::Beside`
//! is the right pane. With no split none of this runs, and `Scope::Main` is the route's as ever.
//!
//! On anything but a thread (a draft, Settings, Basecamp) the split waits: that screen takes the
//! whole main area, and coming back to a thread brings the two panes back. A thread that isn't in
//! the split takes the pane that had the focus. The layout is kept in `Layout::split`.

use super::{Mode, Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use trek_core::settings::SplitLayout;

/// Neither pane gets narrower than this: the composer's narrowest form, and a transcript that can
/// still be read. It's why the split won't open in less, and shows one pane if it comes to less.
pub const MIN_PANE: f32 = 340.;
/// The divider's width, a grab strip between the panes.
pub const DIVIDER: f32 = 9.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
}

impl Pane {
    pub fn other(self) -> Pane {
        match self {
            Pane::Left => Pane::Right,
            Pane::Right => Pane::Left,
        }
    }
}

/// The two threads sharing the main window. Never the same thread twice.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub left: String,
    pub right: String,
    /// The pane with the focus, kept while the split waits behind a draft or Settings: it's the
    /// one the next thread opens in.
    pub focus: Pane,
    /// The left pane's share of the width.
    pub ratio: f32,
}

impl Split {
    pub fn thread(&self, pane: Pane) -> &str {
        match pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    pub fn pane_of(&self, id: &str) -> Option<Pane> {
        if self.left == id {
            Some(Pane::Left)
        } else if self.right == id {
            Some(Pane::Right)
        } else {
            None
        }
    }

    fn place(&mut self, pane: Pane, id: &str) {
        match pane {
            Pane::Left => self.left = id.to_string(),
            Pane::Right => self.right = id.to_string(),
        }
    }
}

/// `room` (the main area's width) holds two panes and the divider.
pub fn fits(room: f32) -> bool {
    room >= MIN_PANE * 2. + DIVIDER
}

/// The left pane's width in `room`, at `ratio`: both panes keep their minimum whatever was left
/// stored, on a window that has grown or shrunk since.
pub fn left_width(ratio: f32, room: f32) -> f32 {
    let width = room - DIVIDER;
    (width * ratio).clamp(MIN_PANE, (width - MIN_PANE).max(MIN_PANE))
}

/// The share a drag leaves the left pane with.
pub fn ratio_at(left: f32, room: f32) -> f32 {
    let width = room - DIVIDER;
    if width <= 0. {
        return 0.5;
    }
    (left_width(left / width, room) / width).clamp(0., 1.)
}

impl Workspace {
    /// The split, while the main area shows it (on a thread): on a draft or a settings page it
    /// waits.
    pub fn split_live(&self) -> Option<&Split> {
        self.split
            .as_ref()
            .filter(|_| matches!(self.route, Route::Thread(_)))
    }

    /// The pane `id` is in, while the split is showing.
    pub fn pane_of(&self, id: &str) -> Option<Pane> {
        self.split_live().and_then(|s| s.pane_of(id))
    }

    /// The scope of the harness's composer with the focus: the right pane's in a split with the
    /// focus there, else the main window's.
    pub fn harness_scope(&self) -> Scope {
        match &self.route {
            Route::Thread(id) => self.scope_showing(id),
            _ => Scope::Main,
        }
    }

    /// The harness's scope showing `id`, if it shows at all.
    pub(super) fn scope_showing(&self, id: &str) -> Scope {
        match self.pane_of(id) {
            Some(Pane::Right) => Scope::Beside,
            _ => Scope::Main,
        }
    }

    /// `id` is drawn in a pane now. With no room for two, only the pane with the focus is.
    pub(super) fn split_shows(&self, id: &str) -> bool {
        self.pane_of(id).is_some() && self.split_fits()
    }

    /// The main area has room for both panes, as it was last drawn.
    pub fn split_fits(&self) -> bool {
        fits(self.split_room.get())
    }

    /// Open `id` beside the thread on screen, with the focus on it. Already open in a pane, it
    /// just takes the focus. With a split open it takes the pane beside the one with the focus.
    pub fn open_beside(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else {
            return;
        };
        // They have places of their own, off the harness.
        if t.editor_chat || t.side_of.is_some() {
            return self.open_thread_here(id, cx);
        }
        if self.ide() {
            self.set_mode(Mode::Agents, cx);
        }
        let Route::Thread(front) = self.route.clone() else {
            // Nothing on screen to go beside: it opens as a thread does.
            return self.navigate(Route::Thread(id.to_string()), cx);
        };
        if front == id || self.pane_of(id).is_some() {
            return self.navigate(Route::Thread(id.to_string()), cx);
        }
        match self.split.as_mut() {
            Some(split) => split.place(split.focus.other(), id),
            None => {
                if !self.split_fits() {
                    cx.emit(WorkspaceEvent::Toast {
                        message: "Not enough room for two threads. Widen the window or close the tools panel.".into(),
                        undo: None,
                    });
                    return;
                }
                self.split = Some(Split {
                    left: front,
                    right: id.to_string(),
                    focus: Pane::Left,
                    ratio: 0.5,
                });
            }
        }
        self.navigate(Route::Thread(id.to_string()), cx);
        self.save_split(cx);
    }

    /// Show `id` in `pane` (a thread dropped on it), with the focus on it. One already in the
    /// other pane stays there and takes the focus: a thread isn't in both.
    pub fn open_in_pane(&mut self, pane: Pane, id: &str, cx: &mut Context<Self>) {
        let Some(split) = self.split.as_mut() else {
            return;
        };
        if split.pane_of(id).is_none() {
            split.place(pane, id);
            split.focus = pane;
        }
        self.navigate(Route::Thread(id.to_string()), cx);
        self.save_split(cx);
    }

    /// The focus moves to `pane`, without touching the keyboard (the click that moved it may have
    /// been on a text field of its own). Its thread becomes the route, so everything that follows
    /// the thread on screen follows the pane.
    pub fn focus_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        // A composer taking the keyboard on a draft says nothing about the split behind it.
        if self.split_live().is_none() {
            return;
        }
        let Some(split) = self.split.as_mut() else {
            return;
        };
        split.focus = pane;
        let id = split.thread(pane).to_string();
        if self.route == Route::Thread(id.clone()) {
            return;
        }
        self.route = Route::Thread(id.clone());
        self.refresh_git(cx);
        self.hand_back_queued(&id, cx);
        cx.notify();
    }

    /// The pane that hasn't the focus takes it, and the keyboard with it.
    pub fn focus_other_pane(&mut self, cx: &mut Context<Self>) {
        let Some(split) = self.split_live() else {
            return;
        };
        let id = split.thread(split.focus.other()).to_string();
        self.navigate(Route::Thread(id), cx);
    }

    /// Close `pane`: the thread leaves the split, keeps its tab, and goes on running. The other
    /// pane's thread gets the full width and the focus.
    pub fn close_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let Some(split) = self.split.take() else {
            return;
        };
        self.save_split(cx);
        if matches!(self.route, Route::Thread(_)) {
            self.navigate(Route::Thread(split.thread(pane.other()).to_string()), cx);
        } else {
            cx.notify();
        }
    }

    /// Close the pane with the focus. `false` with no split showing.
    pub fn close_focused_pane(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.split_live().map(|s| s.focus) else {
            return false;
        };
        self.close_pane(pane, cx);
        true
    }

    /// `id` is going (archived, deleted): its pane goes first, so the other thread comes to the
    /// full width rather than the route falling back to a draft.
    pub(super) fn split_lost(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(pane) = self.split.as_ref().and_then(|s| s.pane_of(id)) {
            self.close_pane(pane, cx);
        }
    }

    /// A thread that went some other way (with its project, its parent, its machine) leaves its
    /// pane too.
    pub fn split_prune(&mut self, cx: &mut Context<Self>) {
        let Some(split) = self.split.as_ref() else {
            return;
        };
        let gone = |id: &str| self.thread(id).is_none_or(|t| t.archived_at.is_some());
        let (left, right) = (gone(&split.left), gone(&split.right));
        match (left, right) {
            (false, false) => {}
            (true, false) => self.close_pane(Pane::Left, cx),
            (false, true) => self.close_pane(Pane::Right, cx),
            (true, true) => {
                self.split = None;
                self.save_split(cx);
                if matches!(self.route, Route::Thread(_)) {
                    self.navigate(Route::Draft { project: None }, cx);
                }
            }
        }
    }

    /// The route is about to be `route`: the focus follows a thread in the split, and one that
    /// isn't in it takes the pane with the focus. Whether the panes changed.
    pub(super) fn split_follow(&mut self, route: &Route) -> bool {
        let (Some(split), Route::Thread(id)) = (self.split.as_mut(), route) else {
            return false;
        };
        match split.pane_of(id) {
            Some(pane) => {
                split.focus = pane;
                false
            }
            None => {
                split.place(split.focus, id);
                true
            }
        }
    }

    /// The agent in `from`'s pane handed off to `to`: the pane follows it. `true` when the pane
    /// hadn't the focus and stays without it (`false`: it opens as any thread does).
    pub(super) fn split_hand_off(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.pane_of(from) else {
            return false;
        };
        if self.route == Route::Thread(from.to_string()) {
            return false;
        }
        if let Some(split) = self.split.as_mut() {
            split.place(pane, to);
        }
        self.open_tab(to);
        self.pane_came_on_screen(to, cx);
        self.save_split(cx);
        true
    }

    /// `id` is on screen in the pane without the focus, as `navigate` has it for the other: looked
    /// at, and loaded.
    fn pane_came_on_screen(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.thread(id).is_some_and(|t| t.is_unseen()) {
            self.mutate_thread(id, cx, |t| {
                t.last_seen_at = trek_core::store::now_ms().max(t.updated_at)
            });
        }
        self.load_for_view(id, cx);
    }

    /// The divider came to rest.
    pub fn set_split_ratio(&mut self, ratio: f32, cx: &mut Context<Self>) {
        if let Some(split) = self.split.as_mut() {
            split.ratio = ratio;
            self.save_split(cx);
        }
    }

    pub(crate) fn save_split(&mut self, cx: &mut Context<Self>) {
        let layout = self.split.as_ref().map(|s| SplitLayout {
            left: s.left.clone(),
            right: s.right.clone(),
            right_focused: s.focus == Pane::Right,
            ratio: s.ratio,
        });
        if self.settings.layout.split != layout {
            self.settings.layout.split = layout;
            self.save_settings(cx);
        }
    }

    /// At launch: the split as it was left, when both its threads are still here.
    pub(super) fn restore_split(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.settings.layout.split.clone() else {
            return;
        };
        // A machine's threads aren't known yet: its pane would close as soon as it's looked at.
        let here = |id: &str| {
            !crate::machines::is_remote(id)
                && self.thread(id).is_some_and(|t| t.archived_at.is_none())
        };
        if saved.left == saved.right || !here(&saved.left) || !here(&saved.right) {
            self.settings.layout.split = None;
            return;
        }
        let focus = if saved.right_focused {
            Pane::Right
        } else {
            Pane::Left
        };
        let split = Split {
            left: saved.left,
            right: saved.right,
            focus,
            ratio: saved.ratio,
        };
        let other = split.thread(focus.other()).to_string();
        let front = split.thread(focus).to_string();
        self.split = Some(split);
        self.open_tab(&other);
        self.pane_came_on_screen(&other, cx);
        self.navigate(Route::Thread(front), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split() -> Split {
        Split {
            left: "a".into(),
            right: "b".into(),
            focus: Pane::Left,
            ratio: 0.5,
        }
    }

    #[test]
    fn a_thread_is_found_in_its_pane() {
        let s = split();
        assert_eq!(s.pane_of("a"), Some(Pane::Left));
        assert_eq!(s.pane_of("b"), Some(Pane::Right));
        assert_eq!(s.pane_of("c"), None);
        assert_eq!(s.thread(Pane::Right), "b");
        assert_eq!(Pane::Left.other(), Pane::Right);
    }

    #[test]
    fn two_panes_need_their_minimum_each_and_the_divider() {
        assert!(!fits(MIN_PANE * 2.));
        assert!(fits(MIN_PANE * 2. + DIVIDER));
    }

    #[test]
    fn neither_pane_goes_under_its_minimum() {
        let room = 1000.;
        assert_eq!(left_width(0.5, room), (room - DIVIDER) / 2.);
        assert_eq!(left_width(0.01, room), MIN_PANE);
        assert_eq!(left_width(0.99, room), room - DIVIDER - MIN_PANE);
        // A window that shrank since: never a clamp the wrong way round.
        assert_eq!(left_width(0.5, 400.), MIN_PANE);
    }

    #[test]
    fn a_drag_past_the_edge_stops_at_the_minimum() {
        let room = 1000.;
        let width = room - DIVIDER;
        assert_eq!(ratio_at(0., room), MIN_PANE / width);
        assert_eq!(ratio_at(room, room), (width - MIN_PANE) / width);
        assert_eq!(ratio_at(width / 2., room), 0.5);
    }
}
