//! Trek: one scale for every corner the app paints, so a theme can make the whole window squarer
//! or softer without each view knowing. Quads, their shadows and images go through
//! [`scaled_corners`] as they're painted. A corner that rounds off the shorter side (a pill, a
//! circle: a status dot, an avatar, a spinner) keeps its shape at any scale; only the corners of
//! cards, buttons, rows and the like change. The scale is the process's, 1 (as designed) until
//! [`set_corner_scale`] says otherwise.

use crate::{Corners, Pixels, Size};
use std::sync::atomic::{AtomicU32, Ordering};

static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0

/// The largest scale taken.
pub const MAX_CORNER_SCALE: f32 = 2.0;

/// Scale every corner the app paints from the next frame on: 0 squares them, 1 is as each element
/// asks, up to [`MAX_CORNER_SCALE`]. Pills and circles stay round. A value that isn't a number
/// is taken as 1.
pub fn set_corner_scale(scale: f32) {
    let scale = if scale.is_finite() {
        scale.clamp(0., MAX_CORNER_SCALE)
    } else {
        1.
    };
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// The scale corners are painted at (see [`set_corner_scale`]).
pub fn corner_scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

static BORDERS: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0
static PILLS_FOLLOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Trek: whether pills and circles take the corner scale too (a theme with square everything),
/// rather than keeping their shape (as designed).
pub fn set_pills_follow(follow: bool) {
    PILLS_FOLLOW.store(follow, Ordering::Relaxed);
}
static SHADOWS: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0

/// The heaviest border scale taken.
pub const MAX_BORDER_SCALE: f32 = 3.0;

fn finite_in(v: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(0., hi) } else { 1. }
}

/// Trek: scale every border the app paints from the next frame on, as a theme's line weight: 0
/// leaves borders out, 1 is as each element asks, up to [`MAX_BORDER_SCALE`]. A border scaled
/// below a device pixel but above none still takes one (`snap_stroke`).
pub fn set_border_scale(scale: f32) {
    BORDERS.store(finite_in(scale, MAX_BORDER_SCALE).to_bits(), Ordering::Relaxed);
}

/// The scale borders are painted at (see [`set_border_scale`]).
pub fn border_scale() -> f32 {
    f32::from_bits(BORDERS.load(Ordering::Relaxed))
}

/// Trek: how strong every shadow the app paints is from the next frame on, 0 (none, a flat look)
/// to 1 (as each element asks).
pub fn set_shadow_scale(scale: f32) {
    SHADOWS.store(finite_in(scale, 1.).to_bits(), Ordering::Relaxed);
}

/// The strength shadows are painted at (see [`set_shadow_scale`]).
pub fn shadow_scale() -> f32 {
    f32::from_bits(SHADOWS.load(Ordering::Relaxed))
}

/// `radii`, for a shape of `size`, at the current corner scale: a corner as round as half the
/// shorter side (or rounder) is a pill's or a circle's and is left alone.
pub(crate) fn scaled_corners(radii: Corners<Pixels>, size: Size<Pixels>) -> Corners<Pixels> {
    if PILLS_FOLLOW.load(Ordering::Relaxed) {
        return scaled_all_at(corner_scale(), radii, size);
    }
    scaled_corners_at(corner_scale(), radii, size)
}

/// `radii` at `scale`, pills and circles too: each no rounder than half the shorter side first.
fn scaled_all_at(scale: f32, radii: Corners<Pixels>, size: Size<Pixels>) -> Corners<Pixels> {
    if scale == 1. {
        return radii;
    }
    let half = size.width.min(size.height) / 2.;
    let one = |r: Pixels| (r.min(half) * scale).min(half);
    Corners {
        top_left: one(radii.top_left),
        top_right: one(radii.top_right),
        bottom_right: one(radii.bottom_right),
        bottom_left: one(radii.bottom_left),
    }
}

fn scaled_corners_at(scale: f32, radii: Corners<Pixels>, size: Size<Pixels>) -> Corners<Pixels> {
    if scale == 1. {
        return radii;
    }
    let half = size.width.min(size.height) / 2.;
    let one = |r: Pixels| {
        if r >= half - Pixels(0.5) {
            r
        } else {
            (r * scale).min(half)
        }
    };
    Corners {
        top_left: one(radii.top_left),
        top_right: one(radii.top_right),
        bottom_right: one(radii.bottom_right),
        bottom_left: one(radii.bottom_left),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{px, size};

    #[test]
    fn cards_follow_the_scale_and_pills_keep_their_shape() {
        let card = size(px(300.), px(120.));
        let r = Corners::all(px(12.));
        assert_eq!(scaled_corners_at(1., r, card), r);
        assert_eq!(scaled_corners_at(0., r, card), Corners::all(px(0.)));
        assert_eq!(scaled_corners_at(0.5, r, card), Corners::all(px(6.)));
        // A softer scale never rounds past half the shorter side.
        let chip = size(px(80.), px(20.));
        assert_eq!(
            scaled_corners_at(2., Corners::all(px(8.)), chip),
            Corners::all(px(10.))
        );
        // A pill (radius at or past half its height) and a dot stay round.
        let pill = Corners::all(px(9999.));
        assert_eq!(scaled_corners_at(0., pill, chip), pill);
        let dot = size(px(8.), px(8.));
        assert_eq!(
            scaled_corners_at(0., Corners::all(px(4.)), dot),
            Corners::all(px(4.))
        );
    }

    #[test]
    fn the_scale_is_kept_in_range() {
        set_corner_scale(f32::NAN);
        assert_eq!(corner_scale(), 1.);
        set_corner_scale(9.);
        assert_eq!(corner_scale(), MAX_CORNER_SCALE);
        set_corner_scale(1.);
    }
}
