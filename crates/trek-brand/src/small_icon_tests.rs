//! The art `assets/brand/trek_icon.py` writes, against the mark this crate draws: the mark's
//! SVGs are the same shapes in the same colours as [`MARK`], and the app icon's 16 and 32 px art
//! (`trek-icon-{night,paper,glass}-{16,32}.svg`, which script/bundle.sh and
//! script/windows-icons.py ship at those sizes) is drawn for its size as the title bar's small
//! mark is. Change the mark in both places, then rerun `python3 trek_icon.py` in assets/brand.

use super::{BOX, MARK, NIGHT, PAPER, Step};
use std::path::Path;

const ICONS: [&str; 3] = ["night", "paper", "glass"];

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brand")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (trek_icon.py writes it)", path.display()))
}

fn art(icon: &str, px: u32) -> String {
    read(&format!("trek-icon-{icon}-{px}.svg"))
}

/// The value of attribute `name` in `tag` (the text of one element, from its name on).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    // A space before the name, so `d` doesn't match inside `id`.
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    Some(&tag[start..start + tag[start..].find('"')?])
}

/// The mountain's four shapes in an SVG, as `Mountain.flat` writes them: the pole, the cloth,
/// the body with the trail cut through it, the shaded face. They're its last four paths, after
/// whatever tile it stands on.
fn mountain(svg: &str) -> [&str; 4] {
    let paths: Vec<&str> = svg
        .split("<path")
        .skip(1)
        .map(|tag| &tag[..tag.find("/>").unwrap()])
        .collect();
    paths[paths.len() - 4..].try_into().unwrap()
}

/// Every number in a path's `d`, in order (its commands' flags too).
fn numbers(d: &str) -> Vec<f32> {
    d.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or_else(|e| panic!("{s:?} in {d:?}: {e}")))
        .collect()
}

/// The numbers `steps` have as an SVG path whose round joins have radius `r`: a move or a line
/// is its point, an arc `r r 0 0 1` and its point.
fn path_numbers(steps: &[Step], r: f32) -> Vec<f32> {
    let mut out = vec![];
    for (i, step) in steps.iter().enumerate() {
        let (x, y) = step.point();
        match step {
            Step::Arc(_) if i > 0 => out.extend([r, r, 0., 0., 1., x, y]),
            _ => out.extend([x, y]),
        }
    }
    out
}

fn assert_close(svg: &[f32], drawn: &[f32], what: &str) {
    assert_eq!(svg.len(), drawn.len(), "{what}: {svg:?} against {drawn:?}");
    for (i, (a, b)) in svg.iter().zip(drawn).enumerate() {
        // The script writes two decimals.
        assert!(
            (a - b).abs() <= 0.0051,
            "{what}, number {i}: the SVG has {a}, the app draws {b}"
        );
    }
}

/// A path's subpaths: each `M…Z`.
fn subpaths(d: &str) -> Vec<&str> {
    d.split('Z').filter(|s| !s.is_empty()).collect()
}

/// The points a path goes through: where each move, line and arc ends.
fn points(d: &str) -> Vec<(f32, f32)> {
    let mut out = vec![];
    let mut rest = d;
    while let Some(at) = rest.find(['M', 'L', 'A']) {
        let after = &rest[at + 1..];
        let end = after
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let n = numbers(&after[..end]);
        out.push((n[n.len() - 2], n[n.len() - 1]));
        rest = &after[end..];
    }
    out
}

#[test]
fn the_marks_svgs_are_the_mark_the_app_draws() {
    for (name, colours) in [("trek-mark.svg", NIGHT), ("trek-mark-paper.svg", PAPER)] {
        let svg = read(name);
        let (x, y, w, h) = BOX;
        assert!(
            svg.contains(&format!("viewBox=\"{x} {y} {w} {h}\"")),
            "{name}'s box"
        );
        let [pole, cloth, body, shade] = mountain(&svg);
        let [lit, shaded, flag] = colours.map(|c| format!("#{c:06X}"));

        // The flag: a pole stroked its width with round ends, and the cloth.
        let (px, foot, top, wide) = MARK.pole;
        assert_close(
            &numbers(attr(pole, "d").unwrap()),
            &[px, foot, px, top],
            "the pole",
        );
        assert_eq!(attr(pole, "stroke-width"), Some(format!("{wide}").as_str()));
        assert_eq!(attr(pole, "stroke-linecap"), Some("round"));
        assert_eq!(attr(pole, "stroke"), Some(flag.as_str()), "{name}");
        let drawn: Vec<f32> = MARK
            .cloth
            .iter()
            .flat_map(|part| part.iter().flat_map(|p| [p.0, p.1]))
            .collect();
        assert_close(&numbers(attr(cloth, "d").unwrap()), &drawn, "the cloth");
        assert_eq!(attr(cloth, "fill"), Some(flag.as_str()), "{name}");

        // The body: the silhouette, then a hole a dot (two half circles, anticlockwise).
        let r = MARK.radius();
        let mut drawn = path_numbers(&MARK.silhouette(), r);
        for &(x, y, dot) in MARK.dots {
            drawn.extend([x - dot, y]);
            drawn.extend([dot, dot, 0., 1., 0., x + dot, y]);
            drawn.extend([dot, dot, 0., 1., 0., x - dot, y]);
        }
        assert_close(&numbers(attr(body, "d").unwrap()), &drawn, "the body");
        assert_eq!(
            attr(body, "fill-rule"),
            Some("evenodd"),
            "the dots are holes"
        );
        assert_eq!(attr(body, "fill"), Some(lit.as_str()), "{name}");

        // The shaded face: the same silhouette right of the ridge, no stroke of its own.
        assert_close(
            &numbers(attr(shade, "d").unwrap()),
            &path_numbers(&MARK.shade(), r),
            "the shaded face",
        );
        assert_eq!(attr(shade, "fill"), Some(shaded.as_str()), "{name}");
        assert_eq!(
            attr(shade, "stroke"),
            None,
            "a stroke would cover the summit"
        );
    }
}

#[test]
fn the_16_px_icon_has_no_trail_and_keeps_its_flag_and_base_on_whole_pixels() {
    for icon in ICONS {
        let svg = art(icon, 16);
        let [pole, cloth, body, shade] = mountain(&svg).map(|p| attr(p, "d").unwrap());
        // No dots: the body is the silhouette alone.
        assert_eq!(subpaths(body).len(), 1, "{icon}: {body}");
        let points = points(body);
        // The base is on a pixel's edge, inside the tile (1 to 15) and clear of its corners.
        let base = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(
            base.fract() == 0.0 && base <= 13.0,
            "{icon}: base at {base}"
        );
        assert!(points.iter().all(|p| p.0 >= 2.0 && p.0 <= 14.0));
        // The summit is in the middle of the pole's pixel column.
        let summit = *points.iter().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let pole = numbers(pole);
        assert_eq!(
            pole[0], summit.0,
            "{icon}: the pole stands behind the summit"
        );
        assert_eq!(pole[0].fract(), 0.5, "{icon}: a pole one pixel wide");
        assert!(pole[1].fract() == 0.0 && pole[3].fract() == 0.0);
        // The cloth is a rectangle of whole pixels above the summit, hanging from the pole's top.
        let cloth = numbers(cloth);
        assert!(cloth.iter().all(|v| v.fract() == 0.0), "{icon}: {cloth:?}");
        assert!(cloth.chunks(2).all(|p| p[1] >= 2.0 && p[1] < summit.1));
        assert_eq!((cloth[0], cloth[1]), (pole[0] + 0.5, pole[3]));
        // The shaded face starts on a pixel's edge under the summit's pixel.
        let ridge: Vec<f32> = numbers(shade)
            .chunks(2)
            .filter(|p| p[1] > summit.1 && p[0] < 13.0)
            .map(|p| p[0])
            .collect();
        assert!(
            ridge.len() >= 2 && ridge.iter().all(|x| *x == summit.0 + 0.5),
            "{icon}: {ridge:?}"
        );
    }
}

#[test]
fn the_32_px_icon_has_a_shorter_trail_of_larger_dots() {
    for icon in ICONS {
        let svg = art(icon, 32);
        let [pole, cloth, body, _] = mountain(&svg).map(|p| attr(p, "d").unwrap());
        let parts = subpaths(body);
        let dots = &parts[1..];
        assert!(
            !dots.is_empty() && dots.len() < MARK.dots.len(),
            "{icon}: {} dots",
            dots.len()
        );
        // Each dot is centred on a pixel, and larger for the mountain's width than the mark's.
        let xs = points(parts[0]).into_iter().map(|p| p.0);
        let width = xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min);
        for dot in dots {
            let n = numbers(dot);
            let (left, y, r) = (n[0], n[1], n[2]);
            assert!(
                ((left + r).fract() - 0.5).abs() < 0.001 && y.fract() == 0.5,
                "{icon}: {dot}"
            );
            assert!(r / width > MARK.dots[0].2 / BOX.2, "{icon}: a dot of {r}");
        }
        // The flag is on whole pixels here too.
        let (pole, cloth) = (numbers(pole), numbers(cloth));
        assert_eq!(pole[0].fract(), 0.5, "{icon}: a pole one pixel wide");
        assert!(cloth.iter().all(|v| v.fract() == 0.0), "{icon}: {cloth:?}");
    }
}

#[test]
fn the_three_small_icons_differ_only_in_colour() {
    // One drawing in three colourings, so Night, Paper and Glass read as one app at every size.
    for px in [16, 32] {
        let night = art("night", px);
        let shapes = mountain(&night).map(|p| attr(p, "d").unwrap());
        for icon in ["paper", "glass"] {
            let other = art(icon, px);
            assert_eq!(
                mountain(&other).map(|p| attr(p, "d").unwrap()),
                shapes,
                "{icon} at {px} px"
            );
        }
    }
}

#[test]
fn the_full_icons_hold_the_mark_as_large_as_the_tile_allows() {
    // On the Mac the mark's 96-unit square is the tile's box (824 of 1024, from 100); on iOS
    // it's the whole canvas. So a point of the mark is at 100 + 824/96 of it, or 1024/96.
    for (name, k, o) in [
        ("trek-icon-night.svg", 824. / 96., 100.),
        ("trek-icon-paper.svg", 824. / 96., 100.),
        ("trek-icon-night-ios.svg", 1024. / 96., 0.),
        ("trek-icon-paper-ios.svg", 1024. / 96., 0.),
        ("trek-icon-dark-ios.svg", 1024. / 96., 0.),
        ("trek-icon-tinted-ios.svg", 1024. / 96., 0.),
    ] {
        let svg = read(name);
        let [pole, _, _, shade] = mountain(&svg).map(|p| attr(p, "d").unwrap());
        let (x, foot, top, _) = MARK.pole;
        let want = [x, foot, x, top].map(|v| o + k * v);
        for (a, b) in numbers(pole).iter().zip(want) {
            assert!((a - b).abs() <= 0.0051, "{name}: the pole at {a}, not {b}");
        }
        // The shaded face ends on the summit itself.
        let n = numbers(shade);
        let summit = MARK.summit();
        let end = (n[n.len() - 2], n[n.len() - 1]);
        assert!(
            (end.0 - (o + k * summit.0)).abs() <= 0.0051
                && (end.1 - (o + k * summit.1)).abs() <= 0.0051,
            "{name}: {end:?}"
        );
    }
}
