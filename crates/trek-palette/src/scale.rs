//! Trek's type scale and corner radii (Figma "08 Theme & motion", the Foundations sheet): seven
//! text sizes on whole points and five radii, in place of the two dozen sizes and radii that grew
//! up view by view. Every text size and corner in the app is one of these (`tests` hold the code
//! and the theme to it); code is set in the mono font at `LABEL` or `META`.
//!
//! The sizes are px at the default UI text size. A view that keeps its size whatever the user's UI
//! size uses `px(text::BODY)`; one that grows with it, as the kit's `text_sm()` sizes do, uses
//! `to_rems(text::BODY)`.
//!
//! A drawing (a device's frame, an app icon, a chart's marks, a glyph made of divs) keeps its own
//! proportions, and so does text sized from the transcript's size, which the user sets: a comment
//! on the line, or just above it, says `scale: ok` and why.

use gpui_kit::{Rems, rems};

/// Text sizes, smallest first.
pub mod text {
    /// Timestamps, counts, captions.
    pub const META: f32 = 11.;
    /// Chips, buttons, secondary rows.
    pub const LABEL: f32 = 12.;
    /// Rows, menus, the UI around the transcript.
    pub const BODY: f32 = 13.;
    /// Row titles, card headings.
    pub const TITLE: f32 = 14.;
    /// Section headings.
    pub const HEADING: f32 = 16.;
    /// Page titles in Settings.
    pub const DISPLAY_S: f32 = 20.;
    /// Basecamp and the welcome.
    pub const DISPLAY: f32 = 28.;

    pub const ALL: [f32; 7] = [META, LABEL, BODY, TITLE, HEADING, DISPLAY_S, DISPLAY];
}

/// Corner radii, smallest first. Windows draws controls at `TAG` and cards at `BUTTON` (see
/// trek-app's `fluent`).
pub mod radius {
    /// Tags and checkboxes.
    pub const TAG: f32 = 4.;
    /// Controls: inputs, chips, small buttons.
    pub const CONTROL: f32 = 6.;
    /// Buttons and rows.
    pub const BUTTON: f32 = 8.;
    /// Cards, menus and popovers.
    pub const CARD: f32 = 12.;
    /// Pills and dots: as round as the element's shorter side allows (`rounded_full()` says the
    /// same). A quad painted by hand isn't clamped to its size: give it half its side instead.
    pub const FULL: f32 = 9999.;

    pub const ALL: [f32; 5] = [TAG, CONTROL, BUTTON, CARD, FULL];
}

/// The UI text size the scale's px are at: the theme's rem when the user hasn't changed it.
pub const REM: f32 = trek_core::settings::DEFAULT_UI_FONT_SIZE;

/// `size`, from the scale, in rems: it grows and shrinks with the user's UI text size.
pub const fn to_rems(size: f32) -> Rems {
    rems(size / REM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn seven_sizes_on_whole_points_and_five_radii() {
        assert!(text::ALL.windows(2).all(|w| w[0] < w[1]));
        assert!(text::ALL.iter().all(|s| s.fract() == 0.));
        assert!(radius::ALL.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(to_rems(text::TITLE), rems(1.));
    }

    /// The theme's own sizes, which the kit's components draw with: its text size is on the
    /// scale, and so are its two radii.
    #[test]
    fn the_theme_s_text_size_and_radii_are_on_the_scale() {
        let themes: serde_json::Value =
            serde_json::from_str(include_str!("../../trek-assets/assets/themes/trek.json"))
                .unwrap();
        let themes = themes["themes"].as_array().unwrap();
        assert_eq!(themes.len(), 2, "Night and Paper");
        for theme in themes {
            let name = &theme["name"];
            let size = theme["font.size"].as_f64().unwrap() as f32;
            assert!(text::ALL.contains(&size), "{name} font.size {size}");
            for key in ["radius", "radius.lg"] {
                let r = theme[key].as_f64().unwrap() as f32;
                assert!(radius::ALL.contains(&r), "{name} {key} {r}");
            }
        }
    }

    /// The Rust files of the crates that draw Trek's UI, test modules left out.
    fn ui_sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = vec![];
        for krate in ["trek-app", "trek-line-breakdown", "trek-motion"] {
            walk(&crates.join(krate).join("src"), &mut files);
        }
        files.sort();
        files
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                let end = lines
                    .iter()
                    .zip(lines.iter().skip(1))
                    .position(|(line, next)| {
                        line.trim_start().starts_with("#[cfg(test)]")
                            && next.trim_start().starts_with("mod ")
                    })
                    .unwrap_or(lines.len());
                (path, lines[..end].join("\n"))
            })
            .collect()
    }

    /// `line` without its comment (a `//` inside a string, as in a URL, isn't one).
    fn code(line: &str) -> &str {
        match line.find("//") {
            Some(at) if line[..at].matches('"').count() % 2 == 0 => &line[..at],
            _ => line,
        }
    }

    /// The numbers written out in `code`, other than 0 (no corner): `px(12.5)` has 12.5;
    /// `text::META`, `Grid3x3` and `pair.0` have none.
    fn numbers(code: &str) -> Vec<f32> {
        let b = code.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            let after_word =
                i > 0 && (b[i - 1].is_ascii_alphanumeric() || b"_.".contains(&b[i - 1]));
            if !b[i].is_ascii_digit() || after_word {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            let literal = code[start..i].trim_end_matches('.').replace('_', "");
            if let Ok(n) = literal.parse::<f32>()
                && n != 0.
            {
                out.push(n);
            }
            // A suffix (`4.5f32`) belongs to the number.
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        }
        out
    }

    /// From `open`, the byte after an opening bracket, to its closing one; or after a field's
    /// colon, to the comma (or bracket) that ends its value.
    fn argument(code: &str, open: usize) -> &str {
        let field = code[..open].ends_with(": ");
        let mut depth = 0;
        for (i, c) in code[open..].char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => return &code[open..open + i],
                ')' | ']' | '}' => depth -= 1,
                ',' | ';' if field && depth == 0 => return &code[open..open + i],
                _ => {}
            }
        }
        &code[open..]
    }

    /// The names a value in `arg` is read from: `look::ROW_RADIUS` and `size`, not a method
    /// (`.chars()`), a call, or the scale's own tokens.
    fn names(arg: &str) -> Vec<&str> {
        let b = arg.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            let starts = (b[i].is_ascii_alphabetic() || b[i] == b'_')
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'));
            if !starts {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_:".contains(&b[i])) {
                i += 1;
            }
            let after_dot = start > 0 && b[start - 1] == b'.';
            let called = arg[i..].trim_start().starts_with(['(', '!']);
            let mut path = arg[start..i].rsplit("::");
            let name = path.next().unwrap_or("");
            let module = path.next().unwrap_or("");
            if !after_dot && !called && !name.is_empty() && !matches!(module, "text" | "radius") {
                out.push(name);
            }
        }
        out
    }

    /// Where a text size or a corner radius is given: `px(12.5)` in `.text_size(px(12.5))` or
    /// `font_size: px(12.5),`, `6.` in `fluent::control(6.)`.
    const SIZES_AND_CORNERS: [&str; 17] = [
        ".text_size(",
        ".font_size(",
        "text_size: ",
        "font_size: ",
        ".rounded(",
        ".rounded_t(",
        ".rounded_b(",
        ".rounded_l(",
        ".rounded_r(",
        ".rounded_tl(",
        ".rounded_tr(",
        ".rounded_bl(",
        ".rounded_br(",
        "corner_radii(",
        "Corners::all(",
        "fluent::control(",
        "fluent::surface(",
    ];

    /// The kit's own sizes, in rems of the UI size: `text_xs()` is 10.5 px, `rounded_md()` 5.25.
    const KIT_SIZES: [&str; 7] = ["xs", "sm", "base", "lg", "xl", "2xl", "3xl"];

    /// What a line that keeps its own numbers says, with why, in a comment on it or just above it.
    const OK: &str = "scale: ok";

    /// A UI source file: its code with the comments taken out, and which lines say [`OK`].
    struct Source {
        short: String,
        code: String,
        ok: Vec<bool>,
    }

    impl Source {
        fn line_of(&self, at: usize) -> usize {
            self.code[..at].matches('\n').count()
        }
    }

    /// No text size or corner radius is written out in the UI's code: each is a token, a constant
    /// set from one, `rounded_full()`, or the theme's. A number in its place, one of the kit's
    /// sizes, or a constant or local set to a number that a size or corner is read from, is
    /// reported, unless its line says [`OK`].
    #[test]
    fn every_text_size_and_corner_comes_from_the_scale() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let sources: Vec<Source> = ui_sources()
            .into_iter()
            .map(|(path, text)| {
                let lines: Vec<&str> = text.lines().collect();
                // The line that says so, or the line under a comment that does.
                let says = |l: &str| l.contains(OK) && l.trim_start().starts_with("//");
                let ok = (0..lines.len())
                    .map(|i| lines[i].contains(OK) || (i > 0 && says(lines[i - 1])))
                    .collect();
                Source {
                    short: path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                    code: lines.iter().map(|l| code(l)).collect::<Vec<_>>().join("\n"),
                    ok,
                }
            })
            .collect();
        assert!(sources.len() > 100, "found {} files", sources.len());
        let mut found = vec![];
        // (file, name, where it's read): the names sizes and corners are read from.
        let mut named: Vec<(usize, &str, String)> = vec![];
        for (ix, s) in sources.iter().enumerate() {
            let kit_calls = KIT_SIZES.iter().flat_map(|kind| {
                let corners = ["", "_t", "_b", "_l", "_r", "_tl", "_tr", "_bl", "_br"]
                    .map(|side| format!(".rounded{side}_{kind}()"));
                std::iter::once(format!(".text_{kind}()")).chain(corners)
            });
            for call in kit_calls {
                for (at, _) in s.code.match_indices(&call) {
                    if !s.ok[s.line_of(at)] {
                        found.push(format!("{}:{}: {call}", s.short, s.line_of(at) + 1));
                    }
                }
            }
            for call in SIZES_AND_CORNERS {
                for (at, _) in s.code.match_indices(call) {
                    let open = at + call.len();
                    let arg = argument(&s.code, open);
                    let (first, last) = (s.line_of(at), s.line_of(open + arg.len()));
                    if (first..=last).any(|l| s.ok[l]) {
                        continue;
                    }
                    let given = numbers(arg);
                    if !given.is_empty() {
                        found.push(format!(
                            "{}:{}: {call}{arg}): {given:?}",
                            s.short,
                            first + 1
                        ));
                    }
                    for name in names(arg) {
                        named.push((ix, name, format!("{}:{}", s.short, first + 1)));
                    }
                }
            }
        }
        for (ix, s) in sources.iter().enumerate() {
            // A constant a size or corner is read from is set from the scale, not to a number.
            for (at, _) in s.code.match_indices("const ") {
                let rest = &s.code[at + "const ".len()..];
                let rest = &rest[..rest.find(';').unwrap_or(rest.len())];
                let Some((name, rhs)) = rest.split_once(": f32 =") else {
                    continue;
                };
                let name = name.trim();
                let Some((_, _, read)) = named.iter().find(|(_, n, _)| *n == name) else {
                    continue;
                };
                let (given, line) = (numbers(rhs), s.line_of(at));
                if !given.is_empty() && !s.ok[line] {
                    found.push(format!(
                        "{}:{}: const {name} = {}: {given:?} (read at {read})",
                        s.short,
                        line + 1,
                        rhs.trim()
                    ));
                }
            }
            // So is a local, in the file that reads it.
            for (n, line) in s.code.lines().enumerate() {
                let Some((name, rhs)) = line
                    .trim_start()
                    .strip_prefix("let ")
                    .and_then(|l| l.split_once('='))
                else {
                    continue;
                };
                let name = name.trim_start_matches("mut ").split(':').next();
                let name = name.unwrap_or("").trim();
                let read = named.iter().any(|(file, n, _)| *file == ix && *n == name);
                let given = numbers(rhs);
                if read && !given.is_empty() && !s.ok[n] {
                    found.push(format!(
                        "{}:{}: let {name} = {}: {given:?}",
                        s.short,
                        n + 1,
                        rhs.trim()
                    ));
                }
            }
        }
        assert!(
            found.is_empty(),
            "text sizes and corners off the scale (use trek_palette::scale's tokens, or say why \
             with `{OK}` on the line):\n{}",
            found.join("\n")
        );
    }
}
