//! Where an artifact's web view may show, and the seam to the native view.
//!
//! A web view is a native view over the window: GPUI can't clip it or draw over it. So each frame
//! Trek works out what part of the card a reader can actually see — the card's frame cut to the
//! transcript's visible area (`Window::content_mask`), with whatever GPUI drew over it taken out —
//! and shows the live view in exactly that rectangle (on macOS in a clipping container; the
//! WKWebView keeps its full size inside it, so its page doesn't reflow as it scrolls in). What
//! covers the card is read off the last frame's hit test: a menu, popover, dialog, toast or the
//! transcript's "Jump to latest" band all block the mouse there. A cover along the top or bottom
//! edge, the whole width across, is cut off; anything else over it (a menu in the middle) turns
//! the card into its last snapshot until it's gone, so the overlay is drawn on top as it should be.

use gpui_kit::*;

/// Where a live view goes, in window px: its whole frame (`body`, the size its page lays out at)
/// and the part of it that shows (`clip`, inside `body`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub body: Bounds<Pixels>,
    pub clip: Bounds<Pixels>,
    /// The card's bottom corners are in view: the clip rounds them as the card does.
    pub round_bottom: bool,
}

/// What a web view on screen can do, as the card needs it. The native one is in `native.rs`; tests
/// get a stand-in that records what it was asked.
pub trait Surface {
    /// Show it at `at`, or hide it (`None`).
    fn place(&mut self, at: Option<Placement>, window: &Window);
    /// Run `js` in the page (the theme's stylesheet swapped).
    fn eval(&self, js: &str);
    /// Load the document again.
    fn reload(&self);
    /// Its page as it looks now, to stand in for it while it's hidden or covered: an encoded image
    /// (TIFF or PNG, `native::decode`) at the device's pixels, delivered later. `None` when the
    /// platform can't.
    fn snapshot(&self) -> Option<async_channel::Receiver<Result<Vec<u8>, String>>>;
    /// Give the keyboard back to Trek (the page had it).
    fn release_keys(&self, window: &Window);
    /// The card's colour changed (a theme switch): what shows where the page draws nothing, on
    /// Windows (WebView2 can't be see-through over GPUI; WKWebView is).
    fn set_background(&self, rgb: (u8, u8, u8));
    #[cfg(test)]
    fn placed(&self) -> Option<Placement> {
        None
    }
}

/// A picture of a page: `width` × `height` device pixels, BGRA (as GPUI's sprites want them).
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What a reader sees of a card: the band of it to show live (`clip`, the full width of what's
/// visible, cut at the top and bottom past edge-to-edge covers), and whether something else still
/// covers part of that band (`covered`: show the snapshot instead).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coverage {
    pub clip: Option<Bounds<Pixels>>,
    pub covered: bool,
}

/// Columns sampled across a card (fewer when it's narrow).
const COLUMNS: usize = 7;
/// Rows are sampled this far apart.
const ROW_STEP: f32 = 32.;

/// What of `visible` (a card cut to the transcript) is uncovered, given `open(p)`: nothing GPUI
/// drew over the card at `p`. Rows covered the whole way across are cut from the top and bottom
/// (to the pixel); a cover anywhere else in what's left makes it `covered`.
pub fn analyze(visible: Bounds<Pixels>, open: impl Fn(Point<Pixels>) -> bool) -> Coverage {
    let (w, h) = (
        f32::from(visible.size.width),
        f32::from(visible.size.height),
    );
    if w < 1. || h < 1. {
        return Coverage {
            clip: None,
            covered: false,
        };
    }
    let (left, top) = (f32::from(visible.origin.x), f32::from(visible.origin.y));
    let columns = COLUMNS.min((w / 24.).ceil().max(2.) as usize);
    let xs: Vec<f32> = (0..columns)
        .map(|i| left + 0.5 + (w - 1.) * i as f32 / (columns - 1) as f32)
        .collect();
    let shut = |y: f32| xs.iter().all(|x| !open(point(px(*x), px(y))));
    let rows = ((h / ROW_STEP).ceil() as usize).max(1);
    let ys: Vec<f32> = (0..=rows)
        .map(|i| (top + 0.5 + (h - 1.) * i as f32 / rows as f32).min(top + h - 0.5))
        .collect();
    let Some(first) = ys.iter().position(|y| !shut(*y)) else {
        return Coverage {
            clip: None,
            covered: false,
        };
    };
    let last = ys.iter().rposition(|y| !shut(*y)).unwrap_or(first);
    // The exact edge between a shut row and an open one, to the pixel.
    let edge = |mut shut_y: f32, mut open_y: f32| {
        while (open_y - shut_y).abs() > 1. {
            let mid = (open_y + shut_y) / 2.;
            if shut(mid) {
                shut_y = mid
            } else {
                open_y = mid
            }
        }
        open_y
    };
    let clip_top = if first == 0 {
        top
    } else {
        (edge(ys[first - 1], ys[first]) - 0.5).floor()
    };
    let clip_bottom = if last == ys.len() - 1 {
        top + h
    } else {
        (edge(ys[last + 1], ys[last]) + 0.5).ceil()
    };
    let clip_top = clip_top.max(top);
    let clip_bottom = clip_bottom.min(top + h);
    if clip_bottom - clip_top < 1. {
        return Coverage {
            clip: None,
            covered: false,
        };
    }
    let covered = ys
        .iter()
        .filter(|y| **y > clip_top && **y < clip_bottom)
        .any(|y| xs.iter().any(|x| !open(point(px(*x), px(*y)))));
    Coverage {
        clip: Some(Bounds::from_corners(
            point(visible.origin.x, px(clip_top)),
            point(visible.origin.x + visible.size.width, px(clip_bottom)),
        )),
        covered,
    }
}

/// How far `body` is from `viewport` up or down (0 when they overlap).
pub fn gap(body: Bounds<Pixels>, viewport: Bounds<Pixels>) -> f32 {
    let above = f32::from(viewport.top() - body.bottom());
    let below = f32::from(body.top() - viewport.bottom());
    above.max(below).max(0.)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has a `test` macro of its own.
    use super::{Coverage, analyze, gap};
    use gpui_kit::{Bounds, Pixels, point, px, size};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn nothing_over_it_shows_it_all() {
        let v = rect(100., 200., 600., 400.);
        assert_eq!(
            analyze(v, |_| true),
            Coverage {
                clip: Some(v),
                covered: false
            }
        );
    }

    #[test]
    fn a_band_across_its_foot_is_cut_off_to_the_pixel() {
        // The "Jump to latest" band: the whole width, its top at y = 547.
        let v = rect(100., 200., 600., 400.);
        let c = analyze(v, |p| f32::from(p.y) < 547.);
        let clip = c.clip.unwrap();
        assert!(!c.covered);
        assert_eq!(clip.top(), px(200.));
        assert!((f32::from(clip.bottom()) - 547.).abs() <= 1., "{clip:?}");
        assert_eq!(clip.size.width, px(600.));
        // And one across its head (a header that overlaps the transcript).
        let c = analyze(v, |p| f32::from(p.y) >= 233.);
        assert!(
            (f32::from(c.clip.unwrap().top()) - 233.).abs() <= 1.,
            "{c:?}"
        );
    }

    #[test]
    fn a_menu_over_its_middle_covers_it() {
        let v = rect(100., 200., 600., 400.);
        let menu = rect(300., 300., 200., 160.);
        let c = analyze(v, |p| !menu.contains(&p));
        assert!(c.covered);
        assert_eq!(
            c.clip,
            Some(v),
            "nothing is cut: the snapshot stands in for the whole card"
        );
    }

    #[test]
    fn covered_all_over_shows_nothing() {
        let v = rect(100., 200., 600., 400.);
        assert_eq!(analyze(v, |_| false).clip, None);
        assert_eq!(
            analyze(rect(0., 0., 0., 10.), |_| true).clip,
            None,
            "nothing visible"
        );
    }

    #[test]
    fn a_narrow_sliver_still_counts() {
        // The card's last 3 px peeking out at the top of the transcript.
        let v = rect(0., 100., 300., 3.);
        assert_eq!(analyze(v, |_| true).clip, Some(v));
    }

    #[test]
    fn gaps_measure_how_far_off_screen_it_is() {
        let vp = rect(0., 100., 800., 600.);
        assert_eq!(gap(rect(0., 300., 800., 200.), vp), 0.);
        assert_eq!(gap(rect(0., 900., 800., 200.), vp), 200.);
        assert_eq!(gap(rect(0., -300., 800., 200.), vp), 200.);
    }
}
