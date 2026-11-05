//! Trek's components on Windows: the same anatomy, states and wording as the approved designs, set
//! in Windows 11's visual language. Surfaces are opaque (Windows has no glass in Trek), controls
//! have 4 px corners, cards and flyouts 8 px, resting cards sit flat on a 1 px stroke, and only what
//! floats casts a shadow. A pill stays a pill on the Mac; a circle stays a circle everywhere, as
//! Windows draws status marks and progress rings round too.
//!
//! Each helper takes what the Mac draws, or leaves it alone there, so a component states its Mac
//! look once and macOS can't change by way of this module.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// Whether this build draws Windows' look.
pub const WINDOWS: bool = cfg!(windows);

/// A control's corners: buttons, chips, inputs, menu rows, tags and tiles.
pub const CONTROL_RADIUS: f32 = 4.;
/// A segmented control's thumb, inside its track's 4 px corners.
pub const THUMB_RADIUS: f32 = 3.;
/// A card's, flyout's or dialog's corners.
pub const SURFACE_RADIUS: f32 = 8.;
/// The selection bar's size on a selected row (see [`selection_bar`]).
pub const BAR_WIDTH: f32 = 3.;
pub const BAR_HEIGHT: f32 = 16.;

/// `windows` on Windows, `mac` elsewhere.
pub const fn pick(windows: f32, mac: f32) -> f32 {
    if WINDOWS { windows } else { mac }
}

/// A control's corner radius, given the Mac's.
pub const fn control(mac: f32) -> f32 {
    pick(CONTROL_RADIUS, mac)
}

/// A card's or flyout's corner radius, given the Mac's.
pub const fn surface(mac: f32) -> f32 {
    pick(SURFACE_RADIUS, mac)
}

/// What a fully rounded chip becomes: a pill on the Mac, a 4 px control on Windows. Not for dots,
/// status marks, spinners or avatars, which stay `rounded_full` everywhere.
pub trait PillExt: Styled + Sized {
    fn rounded_pill(self) -> Self {
        if WINDOWS {
            self.rounded(px(CONTROL_RADIUS))
        } else {
            self.rounded_full()
        }
    }
}

impl<T: Styled> PillExt for T {}

fn dark(cx: &App) -> bool {
    cx.theme().mode.is_dark()
}

fn shadow(alpha: f32, y: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// A resting card's shadow: none on Windows, where it's the stroke that sets it apart.
pub fn card_shadow(mac: Vec<BoxShadow>) -> Vec<BoxShadow> {
    if WINDOWS { Vec::new() } else { mac }
}

/// The shadow of what floats over the window (a flyout, a menu, the context card): Windows 11's
/// flyout elevation, 0 8 16.
pub fn flyout_shadow(mac: Vec<BoxShadow>, cx: &App) -> Vec<BoxShadow> {
    if WINDOWS {
        vec![shadow(if dark(cx) { 0.37 } else { 0.14 }, 8., 16.)]
    } else {
        mac
    }
}

/// A tooltip's shadow: Windows 11's lower elevation, 0 4 8.
pub fn tooltip_shadow(mac: Vec<BoxShadow>, cx: &App) -> Vec<BoxShadow> {
    if WINDOWS {
        vec![shadow(if dark(cx) { 0.28 } else { 0.14 }, 4., 8.)]
    } else {
        mac
    }
}

/// The 1 px stroke round a card or flyout, given the Mac's: Windows' are firmer, as they stand in
/// for a resting card's shadow.
pub fn stroke(mac: Hsla, cx: &App) -> Hsla {
    if WINDOWS {
        if dark(cx) {
            hsla(0., 0., 1., 0.0837)
        } else {
            hsla(0., 0., 0., 0.0803)
        }
    } else {
        mac
    }
}

/// Windows 11's mark on a selected row: a short accent bar at its left edge, for the row to lay
/// over itself (the row needs `relative()`). `None` on the Mac, where the fill alone says it.
pub fn selection_bar(selected: bool, cx: &App) -> Option<Div> {
    (WINDOWS && selected).then(|| {
        div()
            .absolute()
            .left(px(0.))
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(BAR_WIDTH))
                    .h(px(BAR_HEIGHT))
                    .rounded(px(BAR_WIDTH / 2.))
                    .bg(crate::palette::send(cx)),
            )
    })
}

/// A verdict, as Windows 11's info bars colour one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Success,
    Caution,
    Critical,
    Informational,
}

/// A verdict's solid ink and tint, from Windows 11's status colours; the tint is opaque, so a badge
/// reads the same on any surface.
pub fn verdict(kind: Verdict, cx: &App) -> (Hsla, Hsla) {
    let (ink, tint) = verdict_rgb(kind, dark(cx));
    (rgb(ink).into(), rgb(tint).into())
}

/// [`verdict`] as numbers.
pub const fn verdict_rgb(kind: Verdict, dark: bool) -> (u32, u32) {
    match (kind, dark) {
        (Verdict::Success, false) => (0x0F7B0F, 0xDFF6DD),
        // Greener than Windows' own olive #393D1B, which reads as a different hue from the design's.
        (Verdict::Success, true) => (0x6CCB5F, 0x17351F),
        (Verdict::Caution, false) => (0x9D5D00, 0xFFF4CE),
        (Verdict::Caution, true) => (0xFCE100, 0x433519),
        (Verdict::Critical, false) => (0xC42B1C, 0xFDE7E9),
        (Verdict::Critical, true) => (0xFF99A4, 0x442726),
        (Verdict::Informational, false) => (0x005FB7, 0xE5F1FB),
        (Verdict::Informational, true) => (0x60CDFF, 0x1F3A4D),
    }
}

#[cfg(test)]
mod tests {
    // Named, as a glob would bring in the kit's own `test` attribute over the standard one.
    use super::{CONTROL_RADIUS, SURFACE_RADIUS, Verdict, WINDOWS, control, surface, verdict_rgb};

    /// WCAG relative luminance of a 0xRRGGBB colour.
    fn luminance(c: u32) -> f32 {
        let channel = |shift: u32| {
            let v = ((c >> shift) & 0xFF) as f32 / 255.;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn verdict_ink_reads_on_its_tint_in_both_themes() {
        for kind in [
            Verdict::Success,
            Verdict::Caution,
            Verdict::Critical,
            Verdict::Informational,
        ] {
            for dark in [false, true] {
                let (ink, tint) = verdict_rgb(kind, dark);
                let ratio = contrast(ink, tint);
                assert!(ratio >= 4.5, "{kind:?} dark={dark}: {ratio}");
            }
        }
    }

    #[test]
    fn the_mac_keeps_what_it_passes_in() {
        if !WINDOWS {
            assert_eq!(control(15.), 15.);
            assert_eq!(surface(14.), 14.);
        } else {
            assert_eq!(control(15.), CONTROL_RADIUS);
            assert_eq!(surface(14.), SURFACE_RADIUS);
        }
    }
}
