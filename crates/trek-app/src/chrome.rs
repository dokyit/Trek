//! What differs around a window's title bar between macOS and Windows, in one place.
//!
//! macOS draws the traffic lights itself, over the left of the title bar, and has a system menu
//! bar. On Windows gpui-component's `TitleBar` draws the caption buttons (minimize, maximize or
//! restore, close) at the right end of the row and Trek draws its menus in the window
//! (`menu_bar`).

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
pub const RIGHT_RESERVE: f32 = if cfg!(target_os = "macos") { 0. } else { 3. * TITLE_BAR_HEIGHT_PX };

/// `TitleBar`'s height, as a plain number.
pub const TITLE_BAR_HEIGHT_PX: f32 = 34.;

/// Whether Trek's menus are drawn in the window (`menu_bar`): there is no system menu bar on
/// Windows. macOS keeps the native one (`App::set_menus`).
pub const IN_WINDOW_MENUS: bool = cfg!(windows);

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
        let (hover, active, fg) = if close { (theme.danger, theme.danger_active, theme.danger_foreground) } else { (theme.secondary_hover, theme.secondary_active, theme.secondary_foreground) };
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
            .when(supported.minimize, |el| el.child(button("caption-minimize", IconName::WindowMinimize, WindowControlArea::Min)))
            .when(supported.maximize, |el| {
                el.child(button("caption-maximize", if window.is_maximized() { IconName::WindowRestore } else { IconName::WindowMaximize }, WindowControlArea::Max))
            })
            .child(button("caption-close", IconName::WindowClose, WindowControlArea::Close))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it carries gpui's `test` macro, which would take `#[test]`'s place.
    use super::TITLE_BAR_HEIGHT_PX;

    #[test]
    fn the_title_bar_height_is_the_one_the_title_bar_draws() {
        assert_eq!(gpui_kit::px(TITLE_BAR_HEIGHT_PX), gpui_kit::component::TITLE_BAR_HEIGHT);
    }
}
