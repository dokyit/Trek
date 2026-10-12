//! Where the main window was: its place and size, the display it was on, and whether it was
//! maximized, kept in the data folder (`window.json`) so the next launch opens it there.
//!
//! A window's bounds only mean something on their display (on Windows they're in that display's
//! logical pixels, on macOS relative to its screen), so the display goes with them, by the id
//! the system keeps for it across restarts (`PlatformDisplay::uuid`). The next launch puts the
//! window back only on that display, when it's still connected and enough of the window would be
//! on it to grab; otherwise the window opens centred, as on a first launch.

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILE: &str = "window.json";
/// A move or resize is saved this long after the last step of it (a drag reports every step).
const SETTLE: Duration = Duration::from_millis(500);
/// How much of the window must be on its display, and its top edge on it, for it to go back
/// there: enough of the title bar to grab it by.
const MIN_ON_SCREEN: (f32, f32) = (120., 40.);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// The display's id, as the system keeps it.
    pub display: String,
    #[serde(default)]
    pub maximized: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The display's size in the logical pixels these numbers are in (its width and height at
    /// the scale it had then): when it's not what it is now (the display's scale was changed,
    /// or its resolution), the numbers mean a different part of it. Files from before have none.
    #[serde(default)]
    pub area: Option<(f32, f32)>,
}

impl Placement {
    /// `window`'s placement now: its bounds when not maximized, and whether it is. A full-screen
    /// window is kept by the bounds it had before (the next launch opens it as a window).
    pub fn of(window: &Window, cx: &App) -> Option<Placement> {
        let on = window.display(cx)?;
        let display = on.uuid().ok()?.to_string();
        let area = on.bounds().size;
        let (b, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(b) | WindowBounds::Fullscreen(b) => (b, false),
            WindowBounds::Maximized(b) => (b, true),
        };
        Some(Placement {
            display,
            maximized,
            x: b.origin.x.as_f32(),
            y: b.origin.y.as_f32(),
            width: b.size.width.as_f32(),
            height: b.size.height.as_f32(),
            area: Some((area.width.as_f32(), area.height.as_f32())),
        })
    }
}

/// Where `saved` puts the window among `displays` (`(id as kept, id now, bounds)`): on its
/// display, no smaller than `min` nor larger than the display, as long as that display is still
/// there and the window's top edge and enough of it would be on it. `None`: open it centred.
pub fn restore(saved: &Placement, displays: &[(String, DisplayId, Bounds<Pixels>)], min: Size<Pixels>) -> Option<(DisplayId, WindowBounds)> {
    let (_, id, area) = displays.iter().find(|(kept, ..)| *kept == saved.display)?;
    if ![saved.x, saved.y, saved.width, saved.height].iter().all(|v| v.is_finite()) {
        return None;
    }
    let fit = |want: f32, least: Pixels, most: Pixels| px(want.max(least.as_f32()).min(most.as_f32().max(least.as_f32())));
    let mut bounds = Bounds::new(point(px(saved.x), px(saved.y)), size(fit(saved.width, min.width, area.size.width), fit(saved.height, min.height, area.size.height)));
    // On a display that isn't the size it was in logical pixels (its scale changed: 125 % to 200 %
    // makes it 1.6 times smaller), where the window was is another part of it, and its right and
    // bottom edges may have gone off. Put it wholly on, as far as it fits. (A window left partly
    // off the edge on purpose, on a display as it was, stays so.)
    if saved.area.is_some_and(|(w, h)| (w - area.size.width.as_f32()).abs() > 1. || (h - area.size.height.as_f32()).abs() > 1.) {
        let on = |at: f32, extent: f32, from: f32, to: f32| at.min(to - extent).max(from);
        bounds.origin = point(px(on(bounds.origin.x.as_f32(), bounds.size.width.as_f32(), area.left().as_f32(), area.right().as_f32())), px(on(bounds.origin.y.as_f32(), bounds.size.height.as_f32(), area.top().as_f32(), area.bottom().as_f32())));
    }
    let shown = bounds.intersect(area);
    let top_on_screen = bounds.top() >= area.top() && bounds.top() <= area.bottom() - px(MIN_ON_SCREEN.1);
    if !top_on_screen || shown.size.width < px(MIN_ON_SCREEN.0) || shown.size.height < px(MIN_ON_SCREEN.1) {
        return None;
    }
    Some((*id, if saved.maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) }))
}

fn file(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

pub fn load(dir: &Path) -> Option<Placement> {
    serde_json::from_slice(&std::fs::read(file(dir)).ok()?).ok()
}

fn save(dir: &Path, placement: &Placement) {
    let tmp = dir.join(format!("{FILE}.{}", std::process::id()));
    let written = serde_json::to_vec(placement).map_err(std::io::Error::other).and_then(|json| std::fs::write(&tmp, json)).and_then(|()| std::fs::rename(&tmp, file(dir)));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        tracing::warn!("save the window's place: {e}");
    }
}

/// Where the main window opens: as it was left, else centred at `default`. `focus: false` (a
/// launch in the background) opens a window that was maximized at its normal size instead, as
/// maximizing a window on Windows also brings it to the front.
pub fn initial(default: Size<Pixels>, min: Size<Pixels>, focus: bool, cx: &App) -> (Option<DisplayId>, WindowBounds) {
    let displays: Vec<_> = cx.displays().iter().filter_map(|d| Some((d.uuid().ok()?.to_string(), d.id(), d.bounds()))).collect();
    match load(&trek_core::paths::data_dir()).and_then(|saved| restore(&saved, &displays, min)) {
        Some((id, WindowBounds::Maximized(b))) if !focus => (Some(id), WindowBounds::Windowed(b)),
        Some((id, bounds)) => (Some(id), bounds),
        None => (None, WindowBounds::centered(default, cx)),
    }
}

/// The latest placement waiting to be saved, and where.
#[derive(Default)]
struct Pending(Option<(PathBuf, Placement)>, bool);
impl Global for Pending {}

/// The main window moved or changed size: keep its placement, once it has settled.
pub fn remember(window: &Window, cx: &mut App) {
    let Some(placement) = Placement::of(window, cx) else { return };
    let dir = trek_core::paths::data_dir();
    let pending = cx.default_global::<Pending>();
    if pending.0.as_ref().is_some_and(|(d, p)| *d == dir && *p == placement) {
        return;
    }
    pending.0 = Some((dir, placement));
    if std::mem::replace(&mut pending.1, true) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SETTLE).await;
        let latest = cx.update(|cx| {
            let pending = cx.default_global::<Pending>();
            pending.1 = false;
            pending.0.clone()
        });
        if let Some((dir, placement)) = latest {
            save(&dir, &placement);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has a `test` attribute of its own.
    use super::{Placement, file, load, restore, save};
    use gpui_kit::{Bounds, DisplayId, Pixels, Size, WindowBounds, point, px, size};

    fn displays() -> Vec<(String, DisplayId, Bounds<Pixels>)> {
        vec![
            ("left".into(), DisplayId::new(1), Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)))),
            // On Windows, a second monitor's bounds in its own logical pixels (150 %).
            ("right".into(), DisplayId::new(2), Bounds::new(point(px(1280.), px(0.)), size(px(1707.), px(960.)))),
        ]
    }

    fn placed(display: &str, x: f32, y: f32, w: f32, h: f32) -> Placement {
        Placement { display: display.into(), maximized: false, x, y, width: w, height: h, area: None }
    }

    fn min_size() -> Size<Pixels> {
        size(px(760.), px(520.))
    }

    #[test]
    fn a_window_goes_back_where_it_was() {
        let saved = placed("right", 1400., 100., 1200., 800.);
        assert_eq!(restore(&saved, &displays(), min_size()), Some((DisplayId::new(2), WindowBounds::Windowed(Bounds::new(point(px(1400.), px(100.)), size(px(1200.), px(800.)))))));
        let max = Placement { maximized: true, ..placed("left", 10., 20., 1000., 700.) };
        assert_eq!(restore(&max, &displays(), min_size()), Some((DisplayId::new(1), WindowBounds::Maximized(Bounds::new(point(px(10.), px(20.)), size(px(1000.), px(700.)))))), "maximized, with the size it goes back to");
        // Partly off the edge, but plenty to grab: left as it was.
        assert!(restore(&placed("left", 1500., 300., 1000., 700.), &displays(), min_size()).is_some());
    }

    #[test]
    fn a_window_off_every_display_or_on_one_gone_opens_centred() {
        assert_eq!(restore(&placed("unplugged", 100., 100., 1000., 700.), &displays(), min_size()), None, "its display isn't connected");
        assert_eq!(restore(&placed("left", 5000., 100., 1000., 700.), &displays(), min_size()), None, "entirely off its display");
        assert_eq!(restore(&placed("left", 1850., 100., 1000., 700.), &displays(), min_size()), None, "a sliver on screen");
        assert_eq!(restore(&placed("left", 100., -300., 1000., 700.), &displays(), min_size()), None, "its title bar above the top");
        assert_eq!(restore(&placed("left", 100., 1060., 1000., 700.), &displays(), min_size()), None, "only its title bar's edge at the bottom");
        assert_eq!(restore(&placed("left", f32::NAN, 0., 1000., 700.), &displays(), min_size()), None);
    }

    #[test]
    fn a_saved_size_is_kept_within_the_minimum_and_the_display() {
        let Some((_, WindowBounds::Windowed(b))) = restore(&placed("left", 0., 0., 100., 100.), &displays(), min_size()) else { panic!() };
        assert_eq!(b.size, min_size());
        let Some((_, WindowBounds::Windowed(b))) = restore(&placed("right", 1280., 0., 4000., 3000.), &displays(), min_size()) else { panic!() };
        assert_eq!(b.size, size(px(1707.), px(960.)));
    }

    /// The same displays at every scale Windows offers: a 1080p one at 100 % is 1920×1080 logical
    /// pixels, at 200 % 960×540. A window left on it at 100 % comes back whole and on it at any,
    /// not hanging off its right and bottom edges where the numbers now point.
    #[test]
    fn a_window_saved_at_one_scale_comes_back_whole_on_the_display_at_another() {
        for (w, h) in [(1920., 1080.), (2560., 1440.), (3840., 2160.)] {
            let at_100 = (w, h);
            for scale in [1., 1.25, 1.5, 1.75, 2.] {
                let area = Bounds::new(point(px(0.), px(0.)), size(px(w / scale), px(h / scale)));
                let shown = vec![("d".to_string(), DisplayId::new(1), area)];
                // Right and low on the display at 100 %, as a window dragged there is.
                let saved = Placement { area: Some(at_100), ..placed("d", w - 1300., h - 840., 1280., 820.) };
                let Some((_, WindowBounds::Windowed(b))) = restore(&saved, &shown, min_size()) else { panic!("{w}x{h} at {scale}: opens centred") };
                let (right, bottom) = (b.right().as_f32(), b.bottom().as_f32());
                // Whole on the display, unless the display (in logical pixels) is smaller than the window's least.
                assert!(b.size.width <= area.size.width.max(min_size().width) && b.size.height <= area.size.height.max(min_size().height), "{w}x{h} at {scale}: {b:?}");
                assert!(b.origin.x.as_f32() >= -0.01 && b.origin.y.as_f32() >= -0.01, "{w}x{h} at {scale}: {b:?}");
                assert!(right <= area.size.width.as_f32() + 0.01 || b.size.width <= min_size().width, "{w}x{h} at {scale}: right edge at {right}");
                assert!(bottom <= area.size.height.as_f32() + 0.01 || b.size.height <= min_size().height, "{w}x{h} at {scale}: bottom edge at {bottom}");
                // At the scale it was saved at it's exactly where it was.
                if scale == 1. {
                    assert_eq!(b.origin, point(px(w - 1300.), px(h - 840.)));
                }
            }
        }
        // A window left half off the edge on purpose, with the display as it was, stays so; and a
        // file from before the size was kept (no area) is left as it is.
        let area = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let shown = vec![("d".to_string(), DisplayId::new(1), area)];
        for kept in [Some((1920., 1080.)), None] {
            let saved = Placement { area: kept, ..placed("d", 1500., 300., 1000., 700.) };
            let Some((_, WindowBounds::Windowed(b))) = restore(&saved, &shown, min_size()) else { panic!() };
            assert_eq!(b.origin, point(px(1500.), px(300.)));
        }
    }

    /// The smallest windows Windows machines have: its minimum doesn't fit, the window opens at
    /// its minimum anyway with its title bar on screen (the bar and the controls can be reached to
    /// move or close it).
    #[test]
    fn on_a_display_smaller_than_the_minimum_the_title_bar_is_still_on_it() {
        // 1366×768 at 150 % and 1280×720 at 175 %: 910×512 and 731×411 logical.
        for (w, h) in [(910., 512.), (731., 411.)] {
            let shown = vec![("d".to_string(), DisplayId::new(1), Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))))];
            let saved = Placement { area: Some((1366., 768.)), ..placed("d", 40., 40., 1280., 820.) };
            let Some((_, WindowBounds::Windowed(b))) = restore(&saved, &shown, min_size()) else { panic!("{w}x{h}: opens centred") };
            assert_eq!(b.origin, point(px(0.), px(0.)), "{w}x{h}");
            assert!(b.size.width.as_f32() >= min_size().width.as_f32() - 0.01, "{w}x{h}: {b:?}");
        }
    }

    #[test]
    fn a_placement_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("trek-place-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), None);
        let p = Placement { maximized: true, ..placed("{4c4c4544-0000}", -1280., 12.5, 1000., 700.) };
        save(&dir, &p);
        assert_eq!(load(&dir), Some(p));
        std::fs::write(file(&dir), "{not json").unwrap();
        assert_eq!(load(&dir), None, "a damaged file is as good as none");
        let _ = std::fs::remove_dir_all(dir);
    }
}
