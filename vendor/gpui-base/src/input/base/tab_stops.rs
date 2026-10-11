//! Tabs in shaped text. A line of code is shaped as one string, and on Windows DirectWrite leaves
//! a tab out of the glyphs it hands back (the gap it makes is a jump of the next run's origin,
//! which GPUI's renderer doesn't carry over), so a tab-indented file (Go, a Makefile, much C# and
//! JavaScript) shows with its indentation gone. The editor knows its tab size: this puts a blank
//! glyph where each missing tab was, as wide as its tab stop, and moves the glyphs after it, so
//! the text, its byte offsets, the caret, the selection and hit-testing all keep agreeing (they
//! all read the glyphs' positions, and the tab now has its own).
//!
//! A platform that gives a tab a glyph of its own (CoreText) is left as it shaped it.

use gpui::{LineLayout, ShapedGlyph, ShapedLine, ShapedRun, TextRun, Window};
use std::sync::Arc;

/// `line` with a glyph, as wide as the tab stop it reaches, for each tab the shaper left out.
/// `tab_size` is in columns (spaces of the line's font).
pub fn with_tab_stops(mut line: ShapedLine, tab_size: usize, window: &Window) -> ShapedLine {
    if tab_size == 0 || !line.text.contains('\t') {
        return line;
    }
    let Some(font) = line.runs.first().and_then(|run| window.text_system().get_font_for_id(run.font_id)) else { return line };
    // A space of the line's font: the width of a column, and a blank glyph to stand for a tab.
    let space = window.text_system().shape_line(
        " ".into(),
        line.font_size,
        &[TextRun { len: 1, font, color: Default::default(), background_color: None, underline: None, strikethrough: None }],
        None,
    );
    let Some(blank) = space.runs.first().and_then(|run| run.glyphs.first()).map(|glyph| glyph.id) else { return line };
    let column = f32::from(space.width);
    if column <= 0. {
        return line;
    }
    let tab_width = column * tab_size as f32;
    let text = line.text.clone();
    let bytes = text.as_bytes();

    // The glyphs in the order they're laid out.
    let flat: Vec<(usize, usize)> = line.runs.iter().enumerate().flat_map(|(r, run)| (0..run.glyphs.len()).map(move |g| (r, g))).collect();
    let mut out: Vec<ShapedRun> = line.runs.iter().map(|run| ShapedRun { font_id: run.font_id, glyphs: Vec::with_capacity(run.glyphs.len()) }).collect();
    let mut shift = 0f32;
    let mut tabs = 0;
    // Where the last glyph's text ended: what lies between there and the next glyph's start was
    // left out by the shaper.
    let mut seen = 0usize;
    let mut last_run = 0usize;

    let mut tab_before = |out: &mut Vec<ShapedRun>, shift: &mut f32, run: usize, from: usize, to: usize, natural: f32, y: gpui::Pixels| {
        for (at, byte) in bytes.iter().enumerate().take(to.max(from)).skip(from) {
            if *byte != b'\t' {
                continue;
            }
            let start = natural + *shift;
            let stop = ((start / tab_width + 1e-3).floor() + 1.) * tab_width;
            out[run].glyphs.push(ShapedGlyph { id: blank, position: gpui::point(gpui::px(start), y), index: at, is_emoji: false });
            *shift += stop - start;
            tabs += 1;
        }
    };

    for &(r, g) in &flat {
        let glyph = &line.runs[r].glyphs[g];
        let natural = f32::from(glyph.position.x);
        tab_before(&mut out, &mut shift, if seen == 0 { r } else { last_run }, seen, glyph.index, natural, glyph.position.y);
        let mut moved = glyph.clone();
        moved.position.x = gpui::px(natural + shift);
        out[r].glyphs.push(moved);
        last_run = r;
        seen = glyph.index + text[glyph.index..].chars().next().map_or(1, char::len_utf8);
    }
    // Tabs after the last glyph.
    let end = f32::from(line.width);
    tab_before(&mut out, &mut shift, last_run, seen, bytes.len(), end, gpui::px(0.));
    if tabs == 0 {
        return line;
    }
    *line = Arc::new(LineLayout { font_size: line.font_size, width: gpui::px(end + shift), ascent: line.ascent, descent: line.descent, runs: out, len: line.len });
    line
}
