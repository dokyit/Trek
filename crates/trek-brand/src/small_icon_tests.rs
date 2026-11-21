//! The art `assets/brand/trek_icon.py` writes, against the signpost this crate draws: the mark's
//! SVGs and the app icons hold the same shapes in the same colours as [`POST`] and [`BOARDS`],
//! and the app icon's 16 and 32 px art (`trek-icon-{night,paper,glass}-{16,32}.svg`, which
//! script/windows-icons.py ships at those sizes) is drawn on whole pixels,
//! one drawing in three colourings. Change the drawing in the script and rerun it (it writes
//! `signpost.rs` too).

use super::{BOARDS, BOX, NIGHT, PAPER, POST, RADIUS, Step, grown};
use std::path::Path;

const ICONS: [&str; 3] = ["night", "paper", "glass"];

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brand")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (trek_icon.py writes it)", path.display()))
}

/// The value of attribute `name` in `tag` (the text of one element, from its name on).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    // A space before the name, so `d` doesn't match inside `id`.
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    Some(&tag[start..start + tag[start..].find('"')?])
}

/// Every `<path` element's attributes in an SVG, in order.
fn paths(svg: &str) -> Vec<&str> {
    svg.split("<path")
        .skip(1)
        .map(|tag| &tag[..tag.find("/>").unwrap()])
        .collect()
}

/// The signpost's three shapes as `Drawn.flat` writes them: the post, the upper board, the
/// lower. They're the SVG's last three paths, after whatever tile it stands on.
fn signpost(svg: &str) -> [&str; 3] {
    let all = paths(svg);
    all[all.len() - 3..].try_into().unwrap()
}

/// Every number in a path's `d`, in order (its arcs' flags too).
fn numbers(d: &str) -> Vec<f32> {
    d.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or_else(|e| panic!("{s:?} in {d:?}: {e}")))
        .collect()
}

/// The numbers `steps` have as an SVG path whose round joins have radius `r`, each point scaled
/// by `k` and moved by `o`: a move or a line is its point, an arc `r r 0 0 1` and its point.
fn path_numbers(steps: &[Step], r: f32, k: f32, o: f32) -> Vec<f32> {
    let mut out = vec![];
    for (i, step) in steps.iter().enumerate() {
        let (x, y) = step.point();
        let (x, y) = (o + k * x, o + k * y);
        match step {
            Step::Arc(_) if i > 0 => out.extend([r * k, r * k, 0., 0., 1., x, y]),
            _ => out.extend([x, y]),
        }
    }
    out
}

/// The numbers of the shapes the app draws, as the SVG writes them at scale `k` from `o`: the
/// post, then each board with its blaze after it.
fn drawn(k: f32, o: f32) -> [Vec<f32>; 3] {
    let post = path_numbers(&grown(POST, RADIUS), RADIUS, k, o);
    let board = |i: usize| {
        let b = &BOARDS[i];
        let mut n = path_numbers(&grown(b.outline, RADIUS), RADIUS, k, o);
        n.extend(b.blaze.iter().flat_map(|p| [o + k * p.0, o + k * p.1]));
        n
    };
    [post, board(0), board(1)]
}

fn assert_close(svg: &[f32], drawn: &[f32], what: &str) {
    assert_eq!(svg.len(), drawn.len(), "{what}: {svg:?} against {drawn:?}");
    for (i, (a, b)) in svg.iter().zip(drawn).enumerate() {
        // The script writes two decimals.
        assert!(
            (a - b).abs() <= 0.006,
            "{what}, number {i}: the SVG has {a}, the app draws {b}"
        );
    }
}

#[test]
fn the_marks_svgs_are_the_mark_the_app_draws() {
    for (name, [board, post]) in [("trek-mark.svg", NIGHT), ("trek-mark-paper.svg", PAPER)] {
        let svg = read(name);
        let (x, y, w, h) = BOX;
        assert!(
            svg.contains(&format!("viewBox=\"{x} {y} {w} {h}\"")),
            "{name}'s box"
        );
        let shapes = signpost(&svg);
        let want = drawn(1., 0.);
        for (i, (shape, numbers_drawn)) in shapes.iter().zip(&want).enumerate() {
            assert_close(
                &numbers(attr(shape, "d").unwrap()),
                numbers_drawn,
                &format!("{name}, shape {i}"),
            );
            let colour = if i == 0 { post } else { board };
            assert_eq!(attr(shape, "fill"), Some(format!("#{colour:06X}").as_str()));
        }
        assert_eq!(
            attr(shapes[1], "fill-rule"),
            Some("evenodd"),
            "the blaze is a hole"
        );
    }
}

#[test]
fn the_full_icons_hold_the_signpost_the_app_draws() {
    // On the Mac the 96-unit square is the tile's box (824 of 1024, from 100); on iOS it's the
    // whole canvas.
    for (name, k, o) in [
        ("trek-icon-night.svg", 824. / 96., 100.),
        ("trek-icon-paper.svg", 824. / 96., 100.),
        ("trek-icon-night-ios.svg", 1024. / 96., 0.),
        ("trek-icon-paper-ios.svg", 1024. / 96., 0.),
        ("trek-icon-dark-ios.svg", 1024. / 96., 0.),
        ("trek-icon-tinted-ios.svg", 1024. / 96., 0.),
    ] {
        let svg = read(name);
        for (i, (shape, want)) in signpost(&svg).iter().zip(drawn(k, o)).enumerate() {
            assert_close(
                &numbers(attr(shape, "d").unwrap()),
                &want,
                &format!("{name}, shape {i}"),
            );
        }
    }
}

#[test]
fn the_icon_composer_layers_hold_the_signpost_the_app_draws() {
    // The Mac's AppIcon.icon: full bleed like iOS (the system masks it), the post behind the
    // boards, in Night's colours.
    let svg = read("AppIcon.icon/Assets/post.svg") + &read("AppIcon.icon/Assets/boards.svg");
    let [board, post] = NIGHT;
    for (i, (shape, want)) in signpost(&svg)
        .iter()
        .zip(drawn(1024. / 96., 0.))
        .enumerate()
    {
        assert_close(
            &numbers(attr(shape, "d").unwrap()),
            &want,
            &format!("AppIcon.icon, shape {i}"),
        );
        let colour = if i == 0 { post } else { board };
        assert_eq!(attr(shape, "fill"), Some(format!("#{colour:06X}").as_str()));
    }
    let doc = read("AppIcon.icon/icon.json");
    assert!(
        doc.contains("\"image-name\": \"boards.svg\"")
            && doc.contains("\"image-name\": \"post.svg\"")
    );
}

#[test]
fn the_small_icons_are_on_whole_pixels_inside_their_tile() {
    for icon in ICONS {
        for (px, inside) in [(16., 2.0..=14.0), (32., 4.0..=28.0)] {
            let svg = read(&format!("trek-icon-{icon}-{px}.svg"));
            let shapes = paths(&svg);
            // The post's pixels, then the boards': the last two paths.
            for shape in &shapes[shapes.len() - 2..] {
                let n = numbers(attr(shape, "d").unwrap());
                assert!(n.iter().all(|v| v.fract() == 0.0), "{icon} {px}: {n:?}");
            }
            let post = numbers(attr(shapes[shapes.len() - 2], "d").unwrap());
            // The post's runs (x y w 1 -w each) start inside the tile and well clear of its corners.
            for p in post.chunks(5) {
                assert!(
                    inside.contains(&p[0]) && inside.contains(&p[1]),
                    "{icon} {px}: {p:?}"
                );
            }
        }
    }
}

#[test]
fn the_three_small_icons_differ_only_in_colour() {
    // One drawing in three colourings, so Night, Paper and Glass read as one app at every size.
    for px in [16, 32] {
        let shapes = |icon: &str| {
            let svg = read(&format!("trek-icon-{icon}-{px}.svg"));
            let all: Vec<String> = paths(&svg)
                .iter()
                .map(|p| attr(p, "d").unwrap().to_string())
                .collect();
            all[all.len() - 2..].to_vec()
        };
        let night = shapes("night");
        for icon in ["paper", "glass"] {
            assert_eq!(shapes(icon), night, "{icon} at {px} px");
        }
    }
}

#[test]
fn the_menu_bar_glyphs_share_one_signpost() {
    let idle = read("menubar-idle.svg");
    let attention = read("menubar-attention.svg");
    let working = read("menubar-working.svg");
    // Attention is idle with a dot; working draws the same boards as outlines.
    assert!(attention.starts_with(&idle[..idle.len() - "</svg>".len()]));
    assert!(attention.contains("<circle"));
    let boards = |svg: &str| {
        paths(svg)
            .iter()
            .filter_map(|p| attr(p, "d"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert!(boards(&working).contains(&boards(&idle)[1]));
    assert!(working.contains("fill=\"none\""));
    // Each glyph is on whole units: whole pixels at 44 px, the @2x template.
    for svg in [&idle, &working] {
        for d in boards(svg) {
            assert!(numbers(&d).iter().all(|v| v % 1.0 == 0.0), "{d}");
        }
    }
}
