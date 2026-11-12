//! Tabs in shaped text, to the editor's tab stops. A line of code is shaped as one string, and
//! neither platform's shaper knows the editor's tab size. On Windows DirectWrite leaves a tab out
//! of the glyphs it hands back (the gap it makes is a jump of the next run's origin, which GPUI's
//! renderer doesn't carry over), so a tab-indented file (Go, a Makefile, much C# and JavaScript)
//! shows with its indentation gone. On a Mac CoreText lays a tab out itself, to stops of its own
//! every 28 points (three and a bit columns of a 14 px code font, whatever the tab size).
//!
//! The editor knows its tab size: this puts a blank glyph of the line's font where each tab is,
//! as wide as its tab stop, and moves the glyphs after it, so the text, its byte offsets, the
//! caret, the selection and hit-testing all keep agreeing (they all read the glyphs' positions,
//! and the tab now has its own). A tab the shaper gave a glyph is replaced by the blank (its glyph
//! may be a fallback font's, after CJK or an emoji), and what follows goes from where the shaper
//! put it to this stop instead, so it isn't moved twice.

use gpui::{Font, LineLayout, Pixels, ShapedGlyph, ShapedLine, ShapedRun, TextRun, Window, point, px};
use std::sync::Arc;

/// `line` with each tab a blank glyph as wide as the tab stop it reaches. `tab_size` is in
/// columns: spaces of `font`, the line's own (its runs may be in others: a fallback font for an
/// emoji or CJK).
pub fn with_tab_stops(mut line: ShapedLine, tab_size: usize, font: &Font, window: &Window) -> ShapedLine {
    if tab_size == 0 || !line.text.contains('\t') {
        return line;
    }
    // A space of the line's font: the width of a column, and a blank glyph to stand for a tab.
    let space = window.text_system().shape_line(
        " ".into(),
        line.font_size,
        &[TextRun { len: 1, font: font.clone(), color: Default::default(), background_color: None, underline: None, strikethrough: None }],
        None,
    );
    let Some((blank_font, blank)) = space.runs.first().and_then(|run| Some((run.font_id, run.glyphs.first()?.id))) else { return line };
    let column = f32::from(space.width);
    if column <= 0. {
        return line;
    }
    let tab_width = column * tab_size as f32;
    let stop_after = |start: f32| ((start / tab_width + 1e-3).floor() + 1.) * tab_width;
    let text = line.text.clone();
    let bytes = text.as_bytes();

    // The glyphs in the order they're laid out, each in a run of the font it's from. A glyph id
    // names a glyph of one font only: the blank is the space's font's, and in a fallback font's
    // run it would draw whatever that font has under the same id.
    let mut out: Vec<ShapedRun> = Vec::with_capacity(line.runs.len() + 1);
    let mut shift = 0f32;
    // Where the last glyph's text ended: what lies between there and the next glyph's start was
    // left out by the shaper.
    let mut seen = 0usize;
    // The stop a tab the shaper laid out reaches: where the glyph after it goes.
    let mut after_tab: Option<f32> = None;

    // Tabs the shaper left out (DirectWrite): no width of their own, so they start where the
    // next glyph does.
    let tabs_before = |out: &mut Vec<ShapedRun>, shift: &mut f32, from: usize, to: usize, natural: f32, y: Pixels| {
        for (at, byte) in bytes.iter().enumerate().take(to).skip(from) {
            if *byte != b'\t' {
                continue;
            }
            let start = natural + *shift;
            push(out, blank_font, ShapedGlyph { id: blank, position: point(px(start), y), index: at, is_emoji: false });
            *shift += stop_after(start) - start;
        }
    };

    for run in line.runs.iter() {
        for glyph in &run.glyphs {
            let natural = f32::from(glyph.position.x);
            if let Some(stop) = after_tab.take() {
                shift = stop - natural;
            }
            tabs_before(&mut out, &mut shift, seen, glyph.index, natural, glyph.position.y);
            let placed = natural + shift;
            if bytes.get(glyph.index) == Some(&b'\t') {
                // A tab the shaper laid out (CoreText): the blank in its place, and what follows
                // at this stop rather than the shaper's.
                push(&mut out, blank_font, ShapedGlyph { id: blank, position: point(px(placed), glyph.position.y), index: glyph.index, is_emoji: false });
                after_tab = Some(stop_after(placed));
            } else {
                let mut moved = glyph.clone();
                moved.position.x = px(placed);
                push(&mut out, run.font_id, moved);
            }
            seen = seen.max(glyph.index + text[glyph.index..].chars().next().map_or(1, char::len_utf8));
        }
    }
    // Tabs after the last glyph.
    let end = f32::from(line.width);
    if let Some(stop) = after_tab.take() {
        shift = stop - end;
    }
    tabs_before(&mut out, &mut shift, seen, bytes.len(), end, px(0.));
    *line = Arc::new(LineLayout { font_size: line.font_size, width: px(end + shift), ascent: line.ascent, descent: line.descent, runs: out, len: line.len });
    line
}

/// `glyph`, of the font `font_id`, after the glyphs in `out`: in its last run when that's of the
/// same font (as the shaper makes them), else in a new one.
fn push(out: &mut Vec<ShapedRun>, font_id: gpui::FontId, glyph: ShapedGlyph) {
    match out.last_mut() {
        Some(run) if run.font_id == font_id => run.glyphs.push(glyph),
        _ => out.push(ShapedRun { font_id, glyphs: vec![glyph] }),
    }
}
