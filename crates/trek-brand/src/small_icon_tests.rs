//! The app icon's 16 and 32 px art (`assets/brand/trek-icon-{ember,night,glass}-{16,32}.svg`,
//! from `trek_icon.py`), which script/bundle.sh and script/windows-icons.py ship at those sizes,
//! is drawn from the title bar's small mark. These keep the two in step: change the mark, rerun
//! `python3 trek_icon.py` in assets/brand.

use super::SMALL_STONES;
use std::path::Path;

const ICONS: [&str; 3] = ["ember", "night", "glass"];

fn art(icon: &str, px: u32) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/brand")
        .join(format!("trek-icon-{icon}-{px}.svg"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (trek_icon.py writes it)", path.display()))
}

/// The stones in an icon's SVG, as drawn: left, top, width, height, bottom first.
fn stones(svg: &str) -> Vec<(f32, f32, f32, f32)> {
    svg.split("<rect ")
        .skip(1)
        .map(|rect| {
            // A space before each name, so `x` doesn't match inside `rx`.
            let rect = format!(" {rect}");
            let attr = |name: &str| -> f32 {
                let key = format!(" {name}=\"");
                let start = rect
                    .find(&key)
                    .unwrap_or_else(|| panic!("no {name} in{rect}"));
                let value = &rect[start + key.len()..];
                value[..value.find('"').unwrap()].parse().unwrap()
            };
            (attr("x"), attr("y"), attr("width"), attr("height"))
        })
        .collect()
}

#[test]
fn the_32_px_icon_holds_the_small_mark_a_pixel_a_point() {
    // The mark's 16 pt box sits in the middle of the 32 px tile.
    let want: Vec<_> = SMALL_STONES
        .iter()
        .map(|&(x, y, w, h)| (x + 8., y + 8., w, h))
        .collect();
    for icon in ICONS {
        assert_eq!(stones(&art(icon, 32)), want, "trek-icon-{icon}-32.svg");
    }
}

#[test]
fn the_16_px_icon_puts_every_flat_edge_on_a_whole_pixel() {
    for icon in ICONS {
        let drawn = stones(&art(icon, 16));
        assert_eq!(drawn.len(), SMALL_STONES.len(), "{icon}: four stones");
        for &(x, y, w, h) in &drawn {
            assert!(
                [x, y, w, h].iter().all(|v| v.fract() == 0.0),
                "{icon}: {x} {y} {w} {h}"
            );
            // Inside the tile (1 to 15), clear of its rounded corners.
            assert!(x >= 2.0 && x + w <= 14.0 && y >= 2.0 && y + h <= 14.0);
        }
        // Stacked bottom up a pixel apart, each smaller than the one under it, as the mark's are.
        for pair in drawn.windows(2) {
            let (below, above) = (pair[0], pair[1]);
            assert_eq!(
                above.1 + above.3 + 1.0,
                below.1,
                "{icon}: {above:?} on {below:?}"
            );
            assert!(above.2 < below.2, "{icon}: {above:?} on {below:?}");
        }
    }
}

#[test]
fn the_three_small_icons_differ_only_in_colour() {
    // One drawing in three colourings, so Ember, Night and Glass read as one app at every size.
    for px in [16, 32] {
        let ember = stones(&art("ember", px));
        for icon in ["night", "glass"] {
            assert_eq!(stones(&art(icon, px)), ember, "{icon} at {px} px");
        }
    }
}
