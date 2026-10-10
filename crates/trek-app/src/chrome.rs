//! What differs around a window's title bar between macOS and Windows, in one place.
//!
//! macOS draws the traffic lights itself, over the left of the title bar, and has a system menu
//! bar. On Windows gpui-component's `TitleBar` draws the caption buttons (minimize, maximize or
//! restore, close) at the right end of the row and Trek draws its menus in the window
//! (`menu_bar`).

/// Space the title bar leaves at its left before its content: the traffic lights on macOS, a small
/// margin on Windows. It is `TitleBar`'s own left padding, which is private to it: this is the
/// same number, for what Trek lines up with the title bar's content (the sidebar's width, Quick
/// Look's header).
pub const LEFT_INSET: f32 = if cfg!(target_os = "macos") { 80. } else { 12. };

/// How wide the caption buttons are, which `TitleBar` draws as a row of three squares as tall as
/// the bar: 0 on macOS, where the traffic lights are on the left.
pub const RIGHT_RESERVE: f32 = if cfg!(target_os = "macos") { 0. } else { 3. * TITLE_BAR_HEIGHT_PX };
// (Windows 11's own caption buttons are 46 px wide; gpui-component's are the bar's height.)

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_bar_height_is_the_one_the_title_bar_draws() {
        assert_eq!(gpui_kit::px(TITLE_BAR_HEIGHT_PX), gpui_kit::component::TITLE_BAR_HEIGHT);
    }
}
