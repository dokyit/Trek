//! The app's one motion policy, over `trek_motion` (springs, presence, FLIP, the named times and
//! curves, re-exported whole so `crate::motion::X` is everything that moves).
//!
//! Whether anything may move is `enabled`: neither Trek's Reduce motion nor the system's asks for
//! less, and the motion scale isn't 0 (a "Still" Trek theme). Every animation asks it, or reads
//! GPUI's flag, which `system` keeps in step with it. How long a fixed-time animation takes comes
//! from `timed` (the named times at the motion scale). `tests/motion.rs` holds every other file
//! to that: no `Animation::new` and no direct read of the reduce-motion flag outside here.

pub use trek_motion::*;

use gpui_kit::App;

/// Motion is allowed: Trek's Reduce motion is off, so is the system's, and the motion scale isn't
/// 0. Out of a workspace (a test of one element), the system's alone.
pub fn enabled(cx: &App) -> bool {
    match cx.try_global::<crate::workspace::GlobalWorkspace>() {
        Some(ws) => ws.0.read(cx).motion(cx),
        None => !reduced(cx),
    }
}

/// A motion scale that stills everything: 0 (what isn't a number runs as designed, as `set_pace`
/// has it).
pub fn stills(scale: f32) -> bool {
    scale <= 0.
}

/// GPUI's reduce-motion flag: the system's preference or Trek's (`system::reduce_motion_as`),
/// which GPUI's own animations and the components' read. Only `enabled` and the system module
/// read or write it.
pub(crate) fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}
