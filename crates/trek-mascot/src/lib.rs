//! While an agent works: a trail word that changes every few seconds ("Breaking trail…") and a
//! little hiker walking back and forth along a dotted trail above the composer (`working_bar`).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// What Trek says an agent is doing while it works, instead of a plain "Working…". Each says
/// the agent is making its way: none of them has it resting, finishing or lost. The iPhone app
/// has the same list in the same order (`ios/Trek/Design/Hiker.swift`), so a thread says the
/// same word on both.
pub const WORDS: &[&str] = &[
    "Breaking trail",
    "Reading the map",
    "Route-finding",
    "Scouting ahead",
    "Checking the compass",
    "Marking the trail",
    "Following the cairns",
    "Gaining elevation",
    "Crossing the ridge",
    "Charting a course",
    "Stacking cairns",
    "Traversing",
    "Wayfinding",
    "Trailblazing",
];

/// How long each trail word stays before the next: long enough to read, and slower than the
/// clock beside it, so the line doesn't flicker.
pub const WORD_EVERY: u64 = 6;

/// The trail word for a turn that has run `secs` seconds ("Breaking trail"): it changes every
/// [`WORD_EVERY`] seconds, and `seed` (the thread) keeps threads from moving in lockstep.
pub fn word(seed: &str, secs: u64) -> &'static str {
    let n = WORDS.len() as u64;
    let h = seed.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    // Each thread strides through the list from its own start, by a stride that shares no factor
    // with its length: every word comes round once a lap, never twice in a row.
    let gcd = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let stride = (1..n)
        .map(|k| (h >> 7).wrapping_add(k) % n)
        .find(|&s| s > 1 && gcd(s, n) == 1)
        .unwrap_or(1);
    let i = (h % n + (secs / WORD_EVERY) % n * stride) % n;
    WORDS[i as usize]
}

/// Frames per second for the working animation. Pixel art reads fine at this rate; each frame
/// still redraws the window (the other views come from GPUI's cache), so 16 rather than 60 is
/// about a quarter of the frames — and the bar ticks at half this behind another window.
pub const FPS: u64 = 16;

/// Walking pace, in art pixels a second: one pixel a frame at [`FPS`], so the hiker glides
/// evenly instead of stepping one pixel, then two. On a Retina screen that's 24 pt a second.
pub const WALK: f32 = 16.;
/// Waiting on others it paces the same trail at twice the speed (two pixels a frame), and rests
/// less at the ends: restless where the working walk is steady.
pub const PACE: f32 = 32.;
/// How long it takes to get up to speed, and to slow to a stop: about a step each way.
const RAMP: f32 = 0.35;
/// Standing at an end before it sets off again, working and waiting. It turns halfway through.
const REST: f32 = 1.2;
const REST_PACING: f32 = 0.5;
/// Art pixels walked per pose: four poses a stride, so the feet stay planted as it goes.
const STEP: f32 = 4.;

/// One sprite pixel, in points. 1.5pt is 3 device pixels on Retina, so edges stay crisp.
const PX: f32 = 1.5;
const SPRITE_W: usize = 12;
const SPRITE_H: usize = 16;
/// Height of the trail with the hiker on it.
pub const HEIGHT: f32 = SPRITE_H as f32 * PX + 4.;

/// The hiker's poses, facing right. Its colours: `h` hat, `s` skin, `o` eye, `K` near side
/// (shirt and the leg nearest), `k` far leg, `b` boots, `E` the ember pack and `e` its shade.
/// Every pose is 16 rows: on contact the legs are spread and the body sits a row lower, which
/// is the bob of a walk.
const POSES: [[&str; SPRITE_H]; 5] = [
    // Standing.
    [
        "....hhhh....",
        "..hhhhhhhh..",
        "....ssss....",
        "....ssos....",
        ".EE..ss.....",
        "EEEEKKKK....",
        "EEEEKKKKK...",
        "EeEEKKKKs...",
        "EeEEKKKK....",
        ".EEEKKKK....",
        "....KKKK....",
        "....KKK.....",
        "....KKK.....",
        "....KKK.....",
        "....KKK.....",
        "....bbbb....",
    ],
    // Contact: near leg forward, its arm swung back.
    [
        "............",
        "....hhhh....",
        "..hhhhhhhh..",
        "....ssss....",
        "....ssos....",
        ".EE..ss.....",
        "EEEEKKKK....",
        "EEEEKKKK....",
        "EeEEKKKK....",
        "EeEEsKKK....",
        ".EEEKKKK....",
        "....KKKK....",
        "...kkKKKK...",
        "..kk...KK...",
        ".kk.....KK..",
        ".b......bbb.",
    ],
    // Passing: near leg under the body, the far one swinging through.
    [
        "....hhhh....",
        "..hhhhhhhh..",
        "....ssss....",
        "....ssos....",
        ".EE..ss.....",
        "EEEEKKKK....",
        "EEEEKKKKK...",
        "EeEEKKKKs...",
        "EeEEKKKK....",
        ".EEEKKKK....",
        "....KKKK....",
        "....KKkk....",
        "....KK.kk...",
        "....KK..k...",
        "....KK..bb..",
        "....bbb.....",
    ],
    // Contact: far leg forward, the near arm swung forward.
    [
        "............",
        "....hhhh....",
        "..hhhhhhhh..",
        "....ssss....",
        "....ssos....",
        ".EE..ss.....",
        "EEEEKKKK....",
        "EEEEKKKKK...",
        "EeEEKKKKKs..",
        "EeEEKKKK....",
        ".EEEKKKK....",
        "....KKKK....",
        "...KKkkkk...",
        "..KK...kk...",
        ".KK.....kk..",
        ".b......bbb.",
    ],
    // Passing: far leg under the body, the near one swinging through.
    [
        "....hhhh....",
        "..hhhhhhhh..",
        "....ssss....",
        "....ssos....",
        ".EE..ss.....",
        "EEEEKKKK....",
        "EEEEKKKKK...",
        "EeEEKKKKs...",
        "EeEEKKKK....",
        ".EEEKKKK....",
        "....kkKK....",
        "....kkKK....",
        "....kk.KK...",
        "....kk..K...",
        "....kk..bb..",
        "....bbb.....",
    ],
];
/// The standing pose's index in [`POSES`]; the walk is the four after it, in order.
pub const STAND: usize = 0;

/// The hiker's colours on Night and Paper, `(night, paper)`: eight in all, the pack in Trek's
/// ember. The near and far sides of the body are the theme's ink at two strengths, so the
/// hiker reads on either background and its legs read apart.
fn color(c: char, dark: bool) -> Option<u32> {
    let (night, paper) = match c {
        'h' => (0xC9A47C, 0xB58A5C),
        's' => (0xEBC6A0, 0xE9BF95),
        'o' => (0x1A1A1C, 0x1A1A1C),
        'K' => (0x9A9AA2, 0x55555B),
        'k' => (0x5C5C63, 0x9C9A95),
        'b' => (0x8A5A2E, 0x6B4423),
        'E' => (0xFF7A3D, 0xE85D1F),
        'e' => (0xC9551F, 0xB8461A),
        _ => return None,
    };
    Some(if dark { night } else { paper })
}

/// The hiker's walk along its trail, advanced a frame at a time by whoever draws it (the
/// working bar). It walks at a steady pace whatever the trail's width, rests and turns at each
/// end, and takes a pose every [`STEP`] pixels it covers, so its feet don't slide. Its place is
/// kept as a share of the trail, so a trail that grows or shrinks (a resize, the sidebar going)
/// stretches under it instead of moving it, and a change of pace never moves it either.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walk {
    /// Where it is, 0 at the trail's left end to 1 at its right.
    pub at: f32,
    /// It heads right next (or is heading right now).
    pub right: bool,
    /// How fast it's going now, art pixels a second.
    pub speed: f32,
    /// Resting at an end: how much of the rest is left, 1 to 0. It faces the way it came until
    /// halfway, then the way it's going.
    pub rest: f32,
    /// Art pixels covered since it last set off: its legs' place in the stride.
    pub walked: f32,
}

impl Default for Walk {
    /// At the trail's start, facing along it, half a rest before it sets off.
    fn default() -> Self {
        Walk {
            at: 0.,
            right: true,
            speed: 0.,
            rest: 0.5,
            walked: 0.,
        }
    }
}

impl Walk {
    /// `dt` seconds on, along a trail `travel` art pixels long (the hiker's own width left out),
    /// pacing while it waits on others. `dt` is taken in whole frames at [`FPS`] (a frame at
    /// least, a few at most), so each one moves it whole pixels at full speed.
    pub fn step(&mut self, dt: f32, travel: f32, pacing: bool) {
        let frames = (dt * FPS as f32).round().clamp(0., 4.);
        if frames == 0. {
            return;
        }
        let dt = frames / FPS as f32;
        let (top, rest) = if pacing {
            (PACE, REST_PACING)
        } else {
            (WALK, REST)
        };
        if self.rest > 0. {
            self.rest = (self.rest - dt / rest).max(0.);
            // Rested, it sets off in the same frame: it never stands at an end in a stride.
            if self.rest > 0. {
                return;
            }
        }
        let travel = travel.max(1.);
        let left = if self.right { 1. - self.at } else { self.at } * travel;
        // Up to speed over the ramp, and down again so it stops right at the end: never faster
        // than it could stop from in the trail that's left.
        let accel = top / RAMP;
        let v = (self.speed + accel * dt)
            .min(top)
            .min((2. * accel * left).sqrt());
        let d = (v * dt).max(0.25);
        if d >= left {
            self.at = if self.right { 1. } else { 0. };
            self.right = !self.right;
            self.speed = 0.;
            self.rest = 1.;
            self.walked = 0.;
        } else {
            self.at += if self.right { d } else { -d } / travel;
            self.speed = v;
            self.walked += d;
        }
    }

    /// Its pose ([`POSES`]): standing while it rests, else one of the four of the walk, a pose
    /// every [`STEP`] pixels from where it set off.
    pub fn pose(&self) -> usize {
        if self.rest > 0. {
            STAND
        } else {
            1 + (self.walked / STEP) as usize % 4
        }
    }

    /// It faces right: the way it's going, or while it rests, the way it came until halfway.
    pub fn facing_right(&self) -> bool {
        if self.rest > 0.5 {
            !self.right
        } else {
            self.right
        }
    }
}

/// The trail's length the hiker walks, in art pixels, as its last frame drew it: the working
/// bar reads it to step the walk. A first frame (nothing drawn yet) says a typical trail.
#[derive(Clone)]
pub struct Travel(Rc<Cell<f32>>);

impl Default for Travel {
    fn default() -> Self {
        Travel(Rc::new(Cell::new(240.)))
    }
}

impl Travel {
    pub fn get(&self) -> f32 {
        self.0.get()
    }
}

/// `TREK_FORCE_ACTIVE=1` runs the working animation as if the window were frontmost, and keeps
/// drawing the window while macOS hides it (covered, or on another Space), so the animation's full
/// cost, drawing included, can be measured while Trek stays in the background. A measurement aid,
/// not a setting.
pub fn force_active() -> bool {
    static FORCE: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("TREK_FORCE_ACTIVE").is_ok_and(|v| v == "1"));
    *FORCE
}

/// A dotted trail the width of its parent with the hiker on it, where `walk` has it. `still`
/// (reduced motion), it stands where it is, facing along the trail. The caller re-renders at
/// [`FPS`] while it wants motion, stepping `walk` by `travel` (see `WorkingBar`), which this
/// keeps up to date with the trail as drawn.
pub fn trail(walk: Walk, travel: &Travel, still: bool, cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    let travel = travel.0.clone();
    let (pose, right) = if still {
        (STAND, walk.right)
    } else {
        (walk.pose(), walk.facing_right())
    };
    div()
        .h(px(HEIGHT))
        .w_full()
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let cell = cell_size(window.scale_factor());
                    travel.set(travel_of(b, cell) / cell);
                    paint(b, walk.at, pose, right, ink, window)
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// The welcome screen's trail: the hiker `progress` of the way along it (0 to 1), in stride while
/// `walking`, else standing. The caller eases `progress`; the legs follow the ground covered.
pub fn crossing(progress: f32, walking: bool, cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    let progress = progress.clamp(0., 1.);
    div()
        .h(px(HEIGHT))
        .w_full()
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let cell = cell_size(window.scale_factor());
                    let walked = progress * travel_of(b, cell) / cell;
                    let pose = if walking {
                        1 + (walked / STEP) as usize % 4
                    } else {
                        STAND
                    };
                    paint(b, progress, pose, true, ink, window)
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// The colours a trail is drawn in: the theme's mode, and its dots.
#[derive(Clone, Copy)]
struct Ink {
    dark: bool,
    dots: Hsla,
}

impl Ink {
    fn of(cx: &App) -> Ink {
        Ink {
            dark: cx.theme().mode.is_dark(),
            dots: cx.theme().foreground.opacity(0.16),
        }
    }
}

/// `v` points moved to the nearest whole device pixel at `scale` (device pixels per point).
pub fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// One sprite pixel, in points, at `scale`: a whole number of device pixels, so no edge falls
/// between two (1.5 pt is 1.875 device pixels at 125%, and every cell would blur). It's the whole
/// number nearest [`PX`] (ties down), kept short enough that the sprite fits its trail's height
/// ([`HEIGHT`], less the trail's own line). On a Retina display that's 3 device pixels, 1.5 pt, as
/// it always was; at 125% and 150% it's 2 (1.6 and 1.33 pt).
pub fn cell_size(scale: f32) -> f32 {
    let fits = ((HEIGHT - 1.) * scale / SPRITE_H as f32).floor();
    let nearest = (PX * scale - 0.5).ceil();
    nearest.min(fits).max(1.) / scale
}

/// Where the sprite's pixel grid sits on a trail whose ground is at `ground` and whose hiker is
/// at `x`, in points at `scale`: its left edge, its feet, and a pixel's size — every one a whole
/// number of device pixels, the left edge on the sprite's own grid so it steps pixel by pixel.
pub fn grid(x: f32, ground: f32, scale: f32) -> (f32, f32, f32) {
    let cell = cell_size(scale);
    let left = (x / cell).round() * cell;
    (snap(left, scale), snap(ground, scale), cell)
}

/// How far the hiker can go along a trail drawn in `b`, in points: its width less the sprite's
/// and a margin at each end.
fn travel_of(b: Bounds<Pixels>, cell: f32) -> f32 {
    (b.size.width.as_f32() - SPRITE_W as f32 * cell - 8.).max(0.)
}

/// The trail in `b`, with the hiker `share` of the way along it (0 to 1).
fn paint(b: Bounds<Pixels>, share: f32, pose: usize, right: bool, ink: Ink, window: &mut Window) {
    let scale = window.scale_factor();
    let w = b.size.width.as_f32();
    let ground = snap(b.origin.y.as_f32() + HEIGHT - 2., scale);
    // Dotted trail: dots a whole number of device pixels across, each on a device pixel.
    let dot = ((2. * scale).round() / scale).max(1. / scale);
    let mut x = b.origin.x.as_f32() + 2.;
    while x < b.origin.x.as_f32() + w - 2. {
        window.paint_quad(
            fill(
                Bounds::new(
                    point(px(snap(x, scale)), px(snap(ground - 1., scale))),
                    size(px(dot), px(dot)),
                ),
                ink.dots,
            )
            .corner_radii(px(dot / 2.)),
        );
        x += 7.;
    }
    let cell = cell_size(scale);
    let x = b.origin.x.as_f32() + 4. + travel_of(b, cell) * share.clamp(0., 1.);
    // The sprite's pixels sit on whole device pixels, so none blurs between two; its boots
    // stand on the dots.
    let (left, feet, cell) = grid(x, ground - 1., scale);
    sprite(left, feet, cell, pose, right, ink.dark, window);
}

fn sprite(
    left: f32,
    ground: f32,
    cell_px: f32,
    pose: usize,
    right: bool,
    dark: bool,
    window: &mut Window,
) {
    let top = ground - SPRITE_H as f32 * cell_px;
    for (y, row) in POSES[pose].iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let Some(rgb) = color(ch, dark) else {
                continue;
            };
            let col = if right { x } else { SPRITE_W - 1 - x };
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        px(left + col as f32 * cell_px),
                        px(top + y as f32 * cell_px),
                    ),
                    size(px(cell_px), px(cell_px)),
                ),
                gpui_kit::rgb(rgb),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{POSES, STAND, STEP, WALK, WORD_EVERY, WORDS, Walk, word};

    #[test]
    fn trail_words_hold_then_move_on() {
        // The same word for the whole step, from its first second to its last.
        assert_eq!(word("thread-a", 0), word("thread-a", WORD_EVERY - 1));
        assert_eq!(word("thread-a", 12), word("thread-a", 12 + WORD_EVERY - 1));
        // Over a few minutes it changes often, and never stays on one word for long.
        let steps: Vec<&str> = (0..60).map(|i| word("thread-a", i * WORD_EVERY)).collect();
        let changes = steps.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(changes, 59, "a new word every step: {steps:?}");
        // The whole list comes round in a lap, and threads don't move in lockstep.
        let mut lap: Vec<&str> = steps[..WORDS.len()].to_vec();
        lap.sort();
        lap.dedup();
        assert_eq!(lap.len(), WORDS.len());
        let other: Vec<&str> = (0..60).map(|i| word("thread-b", i * WORD_EVERY)).collect();
        assert_ne!(steps, other);
        // Deterministic: a redraw shows the same word.
        assert_eq!(word("thread-a", 123), word("thread-a", 123));
    }

    #[test]
    fn trail_words_are_plain() {
        // Short, sentence case, no jokes about resting or getting lost: each one says the agent
        // is under way.
        for w in WORDS {
            assert!(w.len() <= 20, "{w}");
            assert!(w.chars().next().is_some_and(char::is_uppercase), "{w}");
            assert!(!w.contains('!') && !w.ends_with('…'), "{w}");
            assert_eq!(w[1..], w[1..].to_lowercase(), "sentence case: {w}");
        }
        let mut all = WORDS.to_vec();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), WORDS.len(), "no word twice");
    }

    /// The walk a frame at a time at 16 a second, `n` frames, on a trail `travel` pixels long.
    fn frames(walk: &mut Walk, n: usize, travel: f32, pacing: bool) -> Vec<Walk> {
        (0..n)
            .map(|_| {
                walk.step(1. / 16., travel, pacing);
                *walk
            })
            .collect()
    }

    #[test]
    fn the_hiker_sets_off_walks_rests_turns_and_comes_back() {
        let travel = 200.;
        let mut walk = Walk::default();
        assert_eq!(
            (walk.at, walk.pose(), walk.facing_right()),
            (0., STAND, true),
            "it starts standing at the trail's start, facing along it"
        );
        let mut seen_right_end = false;
        let mut back_home = false;
        for w in frames(&mut walk, 16 * 60, travel, false) {
            assert!((0. ..=1.).contains(&w.at), "on the trail: {w:?}");
            if w.at == 1. {
                seen_right_end = true;
                assert_eq!(w.pose(), STAND, "it stands at the end");
            }
            if seen_right_end && w.at == 0. {
                back_home = true;
            }
        }
        assert!(
            seen_right_end && back_home,
            "there and back within a minute"
        );
    }

    #[test]
    fn at_full_speed_it_moves_one_pixel_a_frame() {
        // Mid-trail, the walk is even: a whole pixel each frame, never one then two.
        let travel = 400.;
        let mut walk = Walk {
            rest: 0.,
            ..Walk::default()
        };
        let path = frames(&mut walk, 160, travel, false);
        let mids: Vec<f32> = path
            .windows(2)
            .filter(|w| (0.1..0.9).contains(&w[0].at) && w[1].speed == WALK)
            .map(|w| (w[1].at - w[0].at) * travel)
            .collect();
        assert!(mids.len() > 40, "{} frames at full speed", mids.len());
        for d in mids {
            assert!((d - 1.).abs() < 1e-3, "{d} px in a frame");
        }
    }

    #[test]
    fn its_feet_stay_planted() {
        // A pose for every few pixels covered: slowing down at the end, the legs slow with it,
        // instead of churning at full cadence while it barely moves.
        let mut walk = Walk {
            rest: 0.,
            ..Walk::default()
        };
        let path = frames(&mut walk, 16 * 20, 120., false);
        let mut last = (path[0].pose(), path[0].walked);
        for w in &path[1..] {
            if w.pose() != last.0 && w.pose() != STAND && last.0 != STAND {
                assert!(
                    w.walked - last.1 >= STEP - 1.01,
                    "a new pose after {} px",
                    w.walked - last.1
                );
                last = (w.pose(), w.walked);
            }
            if w.pose() == STAND {
                last = (STAND, 0.);
            } else if last.0 == STAND {
                last = (w.pose(), w.walked);
            }
        }
        // The walk goes through all four poses, in order.
        let mut walk = Walk {
            rest: 0.,
            ..Walk::default()
        };
        let poses: Vec<usize> = frames(&mut walk, 60, 400., false)
            .iter()
            .map(Walk::pose)
            .collect();
        let mut order = poses.clone();
        order.dedup();
        for w in order.windows(2) {
            assert_eq!(w[1], w[0] % 4 + 1, "{order:?}");
        }
    }

    #[test]
    fn it_turns_halfway_through_its_rest() {
        let mut walk = Walk {
            at: 0.999,
            rest: 0.,
            speed: 1.,
            ..Walk::default()
        };
        walk.step(1. / 16., 100., false);
        assert_eq!((walk.at, walk.pose()), (1., STAND));
        assert!(walk.facing_right(), "it arrives facing the way it came");
        let path = frames(&mut walk, 30, 100., false);
        let turned = path.iter().position(|w| !w.facing_right()).unwrap();
        assert!(
            (8..12).contains(&turned),
            "turns about halfway: frame {turned}"
        );
        assert!(path.iter().skip(turned).all(|w| !w.facing_right()));
    }

    #[test]
    fn a_new_pace_or_a_new_width_never_moves_it() {
        // Working to waiting and back, or the trail resized, at awkward points: it carries on
        // from where it stood.
        for frames_in in [5, 40, 90, 200, 333] {
            let mut walk = Walk::default();
            frames(&mut walk, frames_in, 300., false);
            let before = walk;
            walk.step(1. / 16., 300., true);
            assert!(
                (walk.at - before.at).abs() * 300. <= 2.01
                    && walk.facing_right() == before.facing_right()
                    || walk.rest > 0.,
                "{before:?} -> {walk:?}"
            );
            let before = walk;
            walk.step(1. / 16., 600., false);
            assert!(
                (walk.at - before.at).abs() * 600. <= 1.01 || walk.rest > 0.,
                "a wider trail: {before:?} -> {walk:?}"
            );
        }
    }

    #[test]
    fn a_pause_in_drawing_picks_up_where_it_left_off() {
        // No frame at all moves nothing; a long gap counts as a few frames, not a leap.
        let mut walk = Walk {
            rest: 0.,
            at: 0.5,
            speed: WALK,
            ..Walk::default()
        };
        let before = walk;
        walk.step(0., 300., false);
        assert_eq!(walk, before);
        walk.step(30., 300., false);
        assert!((walk.at - before.at) * 300. <= 4.01, "{walk:?}");
    }

    /// Within a thousandth of a device pixel of a whole number.
    fn whole(points: f32, scale: f32) -> bool {
        let dev = points * scale;
        (dev - dev.round()).abs() < 1e-3
    }

    #[test]
    fn the_sprite_keeps_its_size_on_a_retina_screen() {
        assert_eq!(super::cell_size(2.), 1.5, "1.5 pt is 3 device pixels at 2x");
        let (left, feet, cell) = super::grid(40.2, 25., 2.);
        assert_eq!(
            (left, feet, cell),
            (40.5, 25., 1.5),
            "steps by 1.5 pt, as before"
        );
    }

    #[test]
    fn the_sprite_lands_on_whole_device_pixels_at_every_scale() {
        for scale in [1., 1.25, 1.5, 1.75, 2., 2.25, 2.5, 3.] {
            let cell = super::cell_size(scale);
            assert!(
                whole(cell, scale) && cell * scale >= 1.,
                "{scale}: a pixel of the art is {} device px",
                cell * scale
            );
            // It fits the trail it's drawn on, and isn't far from the art's size.
            assert!(
                cell * super::SPRITE_H as f32 <= super::HEIGHT,
                "{scale}: {} pt tall in a {} pt trail",
                cell * super::SPRITE_H as f32,
                super::HEIGHT
            );
            assert!(
                cell >= super::PX * 0.6 && cell <= super::PX * 1.4,
                "{scale}: a pixel of {cell} pt for {} pt",
                super::PX
            );
            // Wherever the hiker is on the trail, edge and feet are on device pixels too.
            for i in 0..200 {
                let (left, feet, c) = super::grid(4. + i as f32 * 1.37, 25.3, scale);
                assert!(
                    whole(left, scale) && whole(feet, scale) && c == cell,
                    "{scale}: left {left} feet {feet}"
                );
                // Each of the art's pixels begins on one.
                assert!(
                    whole(left + 11. * c, scale) && whole(feet - 16. * c, scale),
                    "{scale}: art pixels at {left}, {feet}"
                );
            }
        }
    }

    #[test]
    fn snapping_goes_to_the_nearest_device_pixel() {
        assert_eq!(super::snap(10.1, 1.25), 10.4, "13 device pixels is 10.4 pt");
        assert_eq!(super::snap(3., 2.), 3.);
        assert!(whole(super::snap(7.77, 1.5), 1.5));
    }

    #[test]
    fn every_pose_is_on_the_grid_with_its_boots_on_the_ground() {
        for (i, pose) in POSES.iter().enumerate() {
            for row in pose {
                assert_eq!(row.chars().count(), super::SPRITE_W, "pose {i}: {row}");
                assert!(
                    row.chars()
                        .all(|c| c == '.' || super::color(c, true).is_some()),
                    "pose {i}: {row}"
                );
            }
            assert!(
                pose[super::SPRITE_H - 1].contains('b'),
                "pose {i} stands on the trail"
            );
        }
        // Eight colours, each with a Night and a Paper value.
        let used: std::collections::BTreeSet<char> = POSES
            .iter()
            .flatten()
            .flat_map(|r| r.chars())
            .filter(|c| *c != '.')
            .collect();
        assert_eq!(used.len(), 8, "{used:?}");
    }
}
