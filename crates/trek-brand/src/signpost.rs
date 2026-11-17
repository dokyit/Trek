//! The signpost's numbers, written by `assets/brand/trek_icon.py` (variant "c"): change
//! the drawing there and run it, not here.

use super::{Board, P};

/// The round joins' radius.
pub(super) const RADIUS: f32 = 1.26;
/// The post's outline, clockwise from its cap's low corner.
pub(super) const POST: &[P] = &[
    (42.50, 19.86),
    (50.06, 16.08),
    (50.06, 79.92),
    (42.50, 79.92),
];
/// The upper board, then the lower.
pub(super) const BOARDS: [Board; 2] = [
    Board {
        outline: &[
            (30.50, 26.52),
            (74.78, 21.87),
            (69.58, 36.77),
            (31.99, 40.72),
        ],
        blaze: &[
            (59.58, 26.00),
            (62.93, 25.65),
            (63.89, 34.83),
            (60.55, 35.19),
        ],
        pivot: (46.28, 32.04),
        angle: -6.00,
    },
    Board {
        outline: &[
            (21.22, 49.78),
            (56.99, 44.75),
            (58.86, 58.06),
            (23.10, 63.08),
            (27.15, 55.73),
        ],
        blaze: &[],
        pivot: (46.28, 53.04),
        angle: -8.00,
    },
];
/// The mark's own box: left, top, width, height (`trek-mark.svg`'s view box).
pub(super) const BOX: (f32, f32, f32, f32) = (13.00, 13.00, 70.00, 70.00);
/// The small mark in its 16 pt box, a point a pixel: `#` the post, `o` a board.
pub(super) const SMALL_1X: [&str; 16] = [
    "................",
    "........#.......",
    ".......##.......",
    "....ooooooooooo.",
    "....ooooooo.oo..",
    "....ooooooo.oo..",
    "....ooooooooo...",
    ".......##.......",
    ".oooooooooo.....",
    "..ooooooooo.....",
    "..ooooooooo.....",
    ".oooooooooo.....",
    ".......##.......",
    ".......##.......",
    ".......##.......",
    ".......##.......",
];
/// The same, two pixels a point.
pub(super) const SMALL_2X: [&str; 32] = [
    "................................",
    "................##..............",
    "...............###..............",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    ".......ooooooooooooooooooooooo..",
    ".......oooooooooooooooooooooo...",
    ".......oooooooooooooooo..oooo...",
    ".......oooooooooooooooo..ooo....",
    ".......oooooooooooooooo..ooo....",
    ".......oooooooooooooooo..oo.....",
    ".......oooooooooooooooooooo.....",
    ".......ooooooooooooooooooo......",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    "..oooooooooooooooooooo..........",
    "...ooooooooooooooooooo..........",
    "....oooooooooooooooooo..........",
    ".....ooooooooooooooooo..........",
    ".....ooooooooooooooooo..........",
    "....oooooooooooooooooo..........",
    "...ooooooooooooooooooo..........",
    "..oooooooooooooooooooo..........",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    "..............####..............",
    "..............####..............",
];
