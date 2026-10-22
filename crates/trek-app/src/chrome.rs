//! What differs around a window's title bar between macOS and Windows, in one place.
//!
//! macOS draws the traffic lights itself, over the left of the title bar, and has a system menu
//! bar. On Windows gpui-component's `TitleBar` draws the caption buttons (minimize, maximize or
//! restore, close) at the right end of the row, and there is no menu bar: the command palette,
//! Settings, the context menus and the shortcuts reach every command.
//!
//! On Windows the title bar is also what Windows asks "is this the window's handle?" of, before
//! every press (`WM_NCHITTEST`): a press on the handle moves the window instead of reaching what's
//! under it. `TitleBar` answers yes for its whole row, so each control in it says it takes the
//! pointer for itself (`no_drag`).

use gpui_kit::*;

/// Space the title bar leaves at its left before its content: the traffic lights on macOS, a small
/// margin on Windows. It is `TitleBar`'s own left padding, which is private to it: this is the
/// same number, for what Trek lines up with the title bar's content (the sidebar's width, Quick
/// Look's header).
pub const LEFT_INSET: f32 = if cfg!(target_os = "macos") { 80. } else { 12. };

/// How wide the caption buttons are, which `TitleBar` draws as a row of three squares as tall as
/// the bar (34 px, 102 in all; Windows 11's own are 46 wide): 0 on macOS, where the traffic lights
/// are on the left. `TitleBar` puts its content left of them by itself; this is for what Trek
/// draws outside a `TitleBar` (Quick Look's header).
pub const RIGHT_RESERVE: f32 = if cfg!(target_os = "macos") {
    0.
} else {
    3. * TITLE_BAR_HEIGHT_PX
};

/// `TitleBar`'s height, as a plain number.
pub const TITLE_BAR_HEIGHT_PX: f32 = 34.;

/// The pointer as 0.4.1-beta.1 handled it: the title bar's controls Windows took for the window's
/// handle (a press on the Editor toggle began a window move), and a pointer that left the window
/// kept the sidebar's rows held. Only a capture build can turn it on (`shots`' `legacy-pointer`),
/// so the beta's failures can be shown beside the fixes.
#[cfg(feature = "shots")]
pub static LEGACY_POINTER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the pointer is handled as the beta did (`LEGACY_POINTER`); never in a release build.
pub fn legacy_pointer() -> bool {
    #[cfg(feature = "shots")]
    {
        LEGACY_POINTER.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(feature = "shots"))]
    {
        false
    }
}

/// A capture build's cursor: the machine's isn't where a script can put it (a shots window is
/// driven by events), so `realmove` and `realleave` say where it is. `true` is off the window.
#[cfg(all(windows, feature = "shots"))]
pub static SCRIPTED_CURSOR_AWAY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the cursor is over the window with handle `hwnd`, as the system has it (`None` where
/// it can't say: a window with no handle). Never from inside an update: asking
/// (`WindowFromPoint`) sends the window under the cursor `WM_NCHITTEST` there and then, and when
/// that's this window GPUI answers it by updating the window, which it can't while the app is
/// being updated (it logs "RefCell already borrowed" instead).
#[cfg(any(windows, test, feature = "test-support"))]
fn cursor_over(hwnd: Option<isize>) -> Option<bool> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(ask) = CURSOR_IN_TESTS.with(|c| c.borrow().clone()) {
        return ask();
    }
    #[cfg(all(windows, feature = "shots"))]
    if std::env::var_os("TREK_SHOT_DIR").is_some() {
        return Some(!SCRIPTED_CURSOR_AWAY.load(std::sync::atomic::Ordering::Relaxed));
    }
    #[cfg(windows)]
    return hwnd.map(crate::winsys::cursor_over_window);
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// The tests' stand-in for the system's answer (`cursor_over`), which may do what asking Windows
/// does: have the window's own callbacks update it.
#[cfg(any(test, feature = "test-support"))]
pub type AskCursor = std::rc::Rc<dyn Fn() -> Option<bool>>;
#[cfg(any(test, feature = "test-support"))]
thread_local!(pub static CURSOR_IN_TESTS: std::cell::RefCell<Option<AskCursor>> = const { std::cell::RefCell::new(None) });

// The windows whose cursor question is out (`ask_after_draw`): one at a time each.
#[cfg(any(windows, test, feature = "test-support"))]
thread_local!(static ASKING: std::cell::RefCell<std::collections::HashSet<WindowId>> = std::cell::RefCell::default());

/// Ask the system where the cursor is once this draw is over, outside any update
/// (`system::after_update`), and move GPUI's pointer off `window` if it's over another window or
/// none (`should_let_go`, asked again then: the window may have heard from the pointer since).
#[cfg(any(windows, test, feature = "test-support"))]
fn ask_after_draw(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    if !ASKING.with(|a| a.borrow_mut().insert(id)) {
        return;
    }
    #[cfg(windows)]
    let hwnd = crate::winsys::hwnd_of(window);
    #[cfg(not(windows))]
    let hwnd = None;
    let handle = window.window_handle();
    crate::system::after_update(cx, move |cx| {
        let over = cursor_over(hwnd);
        ASKING.with(|a| a.borrow_mut().remove(&id));
        if over == Some(false) {
            crate::logging::ran(handle.update(cx, |_, window, cx| {
                if should_let_go(pointer_inside(window), window.is_window_hovered(), || over) {
                    pointer_off_window(window, cx);
                }
            }));
        }
    });
}

/// Moves GPUI's pointer off the window, as an event past its edge would: nothing is under it, so
/// what was hovered isn't (its highlight, its tooltip, a list holding still for it).
#[cfg_attr(not(any(windows, test, feature = "test-support")), allow(dead_code))]
pub fn pointer_off_window(window: &mut Window, cx: &mut App) {
    let modifiers = window.modifiers();
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(-1.), px(-1.)),
            pressed_button: None,
            modifiers,
        }),
        cx,
    );
}

/// A pointer that goes off a window's edge leaves what it was over still hovered: Windows says so
/// to the window alone (`WM_MOUSELEAVE`), with no mouse move for GPUI to take it off the last
/// element, so that stays lit, its tooltip stays up, and the sidebar's rows keep holding still for a
/// pointer that's gone, until it comes back. Called as the window is drawn: where GPUI still has the
/// pointer inside and the system has the cursor over another window or none, it moves the pointer
/// off (`pointer_off_window`). Asking the system, not the window's own notion of it, which is told
/// as the cursor comes and goes and so can lag a draw either way. A press with the button down
/// keeps the cursor with the window (it's captured), so a drag past the edge is left alone. The
/// system is asked after the draw (`ask_after_draw`), never during it.
pub fn let_go_of_hover(window: &mut Window, cx: &mut App) {
    #[cfg(any(windows, test, feature = "test-support"))]
    {
        // Only when GPUI has been told the pointer left: most frames cost a comparison.
        if legacy_pointer()
            || !should_let_go(pointer_inside(window), window.is_window_hovered(), || {
                Some(false)
            })
        {
            return;
        }
        ask_after_draw(window, cx);
    }
    #[cfg(not(any(windows, test, feature = "test-support")))]
    let _ = (window, cx);
}

/// Whether GPUI has the pointer inside the window's viewport.
#[cfg_attr(not(any(windows, test, feature = "test-support")), allow(dead_code))]
pub fn pointer_inside(window: &Window) -> bool {
    Bounds::new(point(px(0.), px(0.)), window.viewport_size()).contains(&window.mouse_position())
}

/// `let_go_of_hover`'s question: GPUI has the pointer `inside`, the window has been told it left
/// (`!window_hovered`), and the system has the cursor over another window or none
/// (`cursor_over() == Some(false)`; `Some(true)` is this window's own frame or a web view in it).
/// Once the pointer is moved off, it isn't inside, so the move is made once.
#[cfg_attr(not(any(windows, test, feature = "test-support")), allow(dead_code))]
pub fn should_let_go(
    inside: bool,
    window_hovered: bool,
    cursor_over: impl FnOnce() -> Option<bool>,
) -> bool {
    inside && !window_hovered && cursor_over() == Some(false)
}

/// Whether a control in the title bar's row asks Windows to keep its press from moving the window.
fn takes_pointer() -> bool {
    cfg!(windows) && !legacy_pointer()
}

/// A control in a title bar's row, which takes the pointer for itself: Windows asks, before every
/// press, whether the pointer is on the window's handle (`WM_NCHITTEST`), and `TitleBar` says yes
/// for its whole row, unless what's under the pointer blocks it. A press on a control that
/// doesn't is a window move: the click never arrives. The area is the control and nothing more
/// (its gaps, the title and the empty strip stay the handle: a press there moves the window and a
/// double click maximises it). `id` names the control; the area is `no-drag-<id>`. macOS keeps
/// what it does (its title bar's own press-and-move).
pub fn no_drag(id: &'static str, control: impl IntoElement) -> impl IntoElement + Styled {
    use gpui_kit::prelude::FluentBuilder as _;
    div()
        .id(SharedString::from(format!("no-drag-{id}")))
        .test_support()
        .flex_none()
        .when(takes_pointer(), |el| el.occlude())
        .child(control)
}

/// Width of the title bar's left cluster (the sidebar toggle and Trek's mark) so that what follows
/// it starts where the sidebar ends: the sidebar's width less what `TitleBar` leaves before it.
pub const fn left_cluster(sidebar_width: f32) -> f32 {
    sidebar_width - LEFT_INSET
}

/// The caption buttons once more, last in the window so they're over everything: the title bar's
/// own are under whatever fills the window (Quick Look, the palette's backdrop), which hides them
/// and takes their clicks, so a window with one of those up couldn't be minimized or closed.
/// Drawn only while something covers the title bar. `None` where the system draws the controls.
pub fn caption_over_overlays(window: &Window, cx: &App) -> Option<AnyElement> {
    use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
    use gpui_kit::prelude::FluentBuilder as _;
    if !cfg!(windows) {
        return None;
    }
    let theme = cx.theme().clone();
    let button = |id: &'static str, icon: IconName, area: WindowControlArea| {
        let close = area == WindowControlArea::Close;
        let (hover, active, fg) = if close {
            (theme.danger, theme.danger_active, theme.danger_foreground)
        } else {
            (
                theme.secondary_hover,
                theme.secondary_active,
                theme.secondary_foreground,
            )
        };
        div()
            .id(id)
            .test_support()
            .flex()
            .w(px(TITLE_BAR_HEIGHT_PX))
            .h_full()
            .flex_none()
            .justify_center()
            .items_center()
            .text_color(theme.foreground)
            .hover(move |s| s.bg(hover).text_color(fg))
            .active(move |s| s.bg(active).text_color(fg))
            .window_control_area(area)
            .child(Icon::new(icon).small())
    };
    let supported = window.window_controls();
    Some(
        h_flex()
            .id("caption-over-overlays")
            .absolute()
            .top_0()
            .right_0()
            .h(px(TITLE_BAR_HEIGHT_PX))
            .when(supported.minimize, |el| {
                el.child(button(
                    "caption-minimize",
                    IconName::WindowMinimize,
                    WindowControlArea::Min,
                ))
            })
            .when(supported.maximize, |el| {
                el.child(button(
                    "caption-maximize",
                    if window.is_maximized() {
                        IconName::WindowRestore
                    } else {
                        IconName::WindowMaximize
                    },
                    WindowControlArea::Max,
                ))
            })
            .child(button(
                "caption-close",
                IconName::WindowClose,
                WindowControlArea::Close,
            ))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it carries gpui's `test` macro, which would take `#[test]`'s place.
    use super::{TITLE_BAR_HEIGHT_PX, should_let_go};

    #[test]
    fn the_title_bar_height_is_the_one_the_title_bar_draws() {
        assert_eq!(
            gpui_kit::px(TITLE_BAR_HEIGHT_PX),
            gpui_kit::component::TITLE_BAR_HEIGHT
        );
    }

    #[test]
    fn hover_is_let_go_only_for_a_pointer_gone_to_another_window() {
        // Told it left, and the cursor is over another window or the desktop: let go.
        assert!(should_let_go(true, false, || Some(false)));
        // Moved off already (the move it makes puts the pointer at -1, -1): once, never again.
        assert!(!should_let_go(false, false, || Some(false)));
        // Still hovered (a drag with the button held keeps the window's pointer captured).
        assert!(!should_let_go(true, true, || Some(false)));
        // Over this window still: its frame, or a web view inside it.
        assert!(!should_let_go(true, false, || Some(true)));
        // A window with no handle can't say: nothing moves.
        assert!(!should_let_go(true, false, || None));
        // The system is only asked when GPUI has been told the pointer left.
        assert!(!should_let_go(true, true, || panic!(
            "asked the system on a hovered window"
        )));
        assert!(!should_let_go(false, false, || panic!(
            "asked the system with the pointer outside"
        )));
    }
}
