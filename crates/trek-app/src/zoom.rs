//! Zooming the whole interface, as browsers and editors do: ⌘+ / ⌘− / ⌘0 (Ctrl on Windows), the
//! View menu, the palette, and Settings › Appearance › Interface, which all change one setting
//! (`Appearance::zoom`).
//!
//! The zoom is the window's (`Window::set_ui_zoom`, Trek's patch to gpui): every window is laid
//! out that many times smaller and drawn that many times larger, so text, controls, gaps, the
//! terminal's cells and the editor's lines grow together and stay sharp, the way a browser zooms
//! a page. The terminal's and the editor's own text sizes stay what they were; the zoom is on top
//! of them, as it is on top of the conversation's. A web page in the Browser grows with it too,
//! on top of its own zoom, which ⌘+ / ⌘− change instead while the page has the keys.
//!
//! The keys step through `ZOOM_STEPS` (Settings' four presets among them, and on past them both
//! ways) between 50% and 300%; ⌘0 is 100% from anywhere, so there's always a way back.

use crate::toast::{self, Toast};
use gpui_kit::*;
use std::time::Duration;
use trek_core::settings::{clamp_zoom, zoom_percent, zoom_step};

/// Which way a key, the menu or the palette zooms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Reset,
}

/// The zoom the interface has (1 with no workspace yet).
pub fn current(cx: &App) -> f32 {
    cx.try_global::<crate::workspace::GlobalWorkspace>()
        .map_or(1.0, |ws| ws.0.read(cx).settings.appearance.zoom())
}

/// Where `step` goes from `zoom`.
pub fn after(zoom: f32, step: Step) -> f32 {
    match step {
        Step::In => zoom_step(zoom, true),
        Step::Out => zoom_step(zoom, false),
        Step::Reset => 1.0,
    }
}

/// Zoom the interface a step (or back to 100%), save it, and say where it is in `window`, for a
/// moment.
pub fn step(step: Step, window: Option<&mut Window>, cx: &mut App) {
    let next = after(current(cx), step);
    set(next, cx);
    if let Some(window) = window {
        show(next, window, cx);
    }
}

/// Make `zoom` the interface's, and save it: every window takes it (`apply`, from `system::sync`).
pub fn set(zoom: f32, cx: &mut App) {
    let zoom = clamp_zoom(zoom);
    let Some(ws) = cx
        .try_global::<crate::workspace::GlobalWorkspace>()
        .map(|g| g.0.clone())
    else {
        return;
    };
    ws.update(cx, |ws, cx| {
        if ws.settings.appearance.zoom() != zoom || ws.settings.appearance.zoom != zoom {
            ws.settings.appearance.zoom = zoom;
            ws.save_settings(cx);
        }
    });
}

/// The key the zoom's toast goes up under: a new step replaces it rather than stacking.
pub const TOAST: &str = "zoom";

/// The least room, in the zoomed layout's points, a window keeps for Trek's layout: the sidebar,
/// a thread beside it and its composer. A zoom that would leave less is held at what fits (never
/// below 100% for that: a smaller window is the user's to make).
pub const ROOM: (f32, f32) = (720., 460.);

/// The zoom a window `size` points large (on screen) is drawn at for the chosen `zoom`.
pub fn fitted(zoom: f32, size: Size<Pixels>) -> f32 {
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    if w <= 0. || h <= 0. {
        return zoom;
    }
    let fits = (w / ROOM.0).min(h / ROOM.1).max(1.0);
    zoom.min(fits)
}

/// The window's size on screen, in points.
fn screen_size(window: &Window) -> Size<Pixels> {
    let z = window.ui_zoom();
    window.viewport_size().map(|d| d * z)
}

/// Give `window` the interface's zoom as it fits now: after it was resized, once the resize is
/// over (the zoom changes its layout's size, which the resize's own observers are hearing).
pub fn fit(window: &mut Window, cx: &mut App) {
    window.defer(cx, |window, cx| {
        let want = fitted(current(cx), screen_size(window));
        if (window.ui_zoom() - want).abs() > 1e-4 {
            window.set_ui_zoom(want, cx);
        }
    });
}

/// "Zoom 110%", small and brief, where toasts go; with what this window is drawn at when it's too
/// small for the whole of it.
pub fn show(zoom: f32, window: &mut Window, cx: &mut App) {
    let here = fitted(zoom, screen_size(window));
    let text = if here < zoom - 1e-4 {
        format!(
            "Zoom {} · {} fits this window",
            zoom_percent(zoom),
            zoom_percent(here)
        )
    } else {
        format!("Zoom {}", zoom_percent(zoom))
    };
    toast::push(
        window,
        Toast::new(text)
            .key(TOAST.into())
            .lifetime(Duration::from_millis(1400)),
        cx,
    );
}

thread_local! {
    /// The zoom of the window being drawn (`drawing`), for what's drawn without the window at
    /// hand (the title bar's room for the traffic lights). Windows draw one at a time.
    static DRAWING: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// Note the zoom of the window about to be drawn: its root view says so first.
pub fn drawing(window: &Window) {
    DRAWING.with(|d| d.set(window.ui_zoom()));
}

/// The zoom of the window being drawn.
pub fn now() -> f32 {
    DRAWING.with(|d| d.get())
}

/// Put `zoom` on every window, and on the ones opened from now on.
pub fn apply(zoom: f32, cx: &mut App) {
    Window::set_default_ui_zoom(zoom);
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, cx| {
            let here = fitted(zoom, screen_size(window));
            window.set_ui_zoom(here, cx)
        });
    }
}

/// A window's own ⌘+ / ⌘− / ⌘0: the Browser's page while it has the keys (`browser`, the
/// window's Browser if it has one), else the interface.
pub fn key(
    step: Step,
    browser: Option<Entity<crate::panels::browser::BrowserPanel>>,
    window: &mut Window,
    cx: &mut App,
) {
    if crate::artifact::page_has_keys(window)
        && let Some(browser) = browser
        && browser.update(cx, |b, cx| b.zoom_page(step, cx))
    {
        return;
    }
    self::step(step, Some(window), cx);
}

/// The app's own ⌘+ / ⌘− / ⌘0 (the menu, with no window to say it in): the interface, the
/// toast in the window in front.
pub fn register(cx: &mut App) {
    cx.on_action(|_: &crate::ZoomIn, cx| from_app(Step::In, cx));
    cx.on_action(|_: &crate::ZoomOut, cx| from_app(Step::Out, cx));
    cx.on_action(|_: &crate::ResetZoom, cx| from_app(Step::Reset, cx));
}

fn from_app(s: Step, cx: &mut App) {
    step(s, None, cx);
    let zoom = current(cx);
    if let Some(window) = cx.active_window() {
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| show(zoom, window, cx));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, after, fitted};
    use gpui_kit::px;

    #[test]
    fn the_keys_step_and_come_back() {
        assert_eq!(after(1.0, Step::In), 1.1);
        assert_eq!(after(1.1, Step::Out), 1.0);
        assert_eq!(after(3.0, Step::In), 3.0, "the top stays the top");
        assert_eq!(after(0.5, Step::Out), 0.5);
        assert_eq!(after(2.5, Step::Reset), 1.0);
        assert_eq!(after(0.5, Step::Reset), 1.0);
    }

    #[test]
    fn a_window_too_small_for_the_zoom_is_drawn_at_what_fits() {
        let size = |w: f32, h: f32| gpui_kit::size(px(w), px(h));
        // 1440 × 920 points: room for twice the layout, not three times.
        assert_eq!(fitted(3.0, size(1440., 920.)), 2.0);
        assert_eq!(fitted(1.5, size(1440., 920.)), 1.5);
        // A large display has room for all of it.
        assert_eq!(fitted(3.0, size(2560., 1600.)), 3.0);
        // Zooming out is never held back, and a small window doesn't zoom below 100% by itself.
        assert_eq!(fitted(0.5, size(400., 300.)), 0.5);
        assert_eq!(fitted(1.25, size(400., 300.)), 1.0);
    }
}
