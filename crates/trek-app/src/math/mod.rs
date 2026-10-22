//! Math in answers, typeset natively: `$…$` and `\(…\)` in a line of text, `$$…$$` and `\[…\]`
//! centred on their own. Agents answer maths and statistics questions in LaTeX; shown raw it's
//! unreadable, and Markdown even eats the backslash of `\[`.
//!
//! `detect` finds the formulas in the Markdown source and masks them from Markdown, `parse` reads
//! the TeX, `layout` sets it in boxes by TeX's rules with the KaTeX fonts' metrics, and this
//! module draws the boxes with GPUI — glyphs in the embedded KaTeX fonts (tall delimiters and
//! radicals built from the fonts' own pieces, as TeX builds them) and rules on whole device
//! pixels — in the text's own colour, crisp at any scale. No web view, no JavaScript, no images,
//! and no paths (GPUI's path pipeline stalled the GPU when a transcript drew many of them).
//!
//! The text of every formula is its source, delimiters and all, so copying an answer gives the
//! TeX back. A formula still being written (its closing delimiter hasn't arrived) shows as plain
//! text until it closes.

mod detect;
pub mod layout;
mod parse;
mod tables;

use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    markdown_ast,
};
use gpui_kit::*;
use layout::{Face, Item, MBox, Measure, Shape};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Math is set this much larger than the text around it: the KaTeX fonts' x-height is
/// Computer Modern's, well below Inter's.
const SCALE: f32 = 1.21;

const INLINE: &str = "trek-math";
const DISPLAY: &str = "trek-math-display";

/// A formula as parsed: where it was, its TeX, and its tree (none while it's still open).
struct Formula {
    source: String,
    display: bool,
    tex: String,
    nodes: Option<Arc<Vec<parse::Node>>>,
}

impl Formula {
    fn of(source: &str) -> Option<Formula> {
        let region = detect::whole(source)?;
        let tex = region.tex(source);
        // Huge "formulas" are more likely a mistake than math: shown as written.
        let nodes = (region.closed && tex.len() <= 20_000).then(|| Arc::new(parse::parse(&tex)));
        Some(Formula {
            source: source.to_string(),
            display: region.delim.display(),
            tex,
            nodes,
        })
    }

    fn node(self, name: &'static str) -> MarkdownNode {
        let source = self.source.clone();
        let label = self.tex.trim().to_string();
        MarkdownNode::new(name, self)
            .text(source.clone())
            .markdown(source)
            .accessibility_label(label)
    }
}

/// A formula the mask made: inline math whose text Markdown read is all `detect::FILL`.
fn masked(node: &markdown_ast::Node) -> bool {
    matches!(node, markdown_ast::Node::InlineMath(m) if !m.value.is_empty() && m.value.bytes().all(|b| b == detect::FILL))
}

/// Inline math, and any formula in a table cell or heading (display math set inline there).
/// It also masks the source, so Markdown hands every formula over whole.
pub struct InlineMath;

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        INLINE
    }

    fn mask(&self, source: &str) -> Option<String> {
        detect::mask(source)
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        if !masked(node) {
            return None;
        }
        Some(Formula::of(cx.node_source(node)?)?.node(INLINE))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        window: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let formula = node.data::<Formula>()?;
        // Still open: the plain-text fallback, which is its source.
        let nodes = formula.nodes.as_ref()?;
        let em = context.font_size() * SCALE;
        let b = typeset(&formula.tex, nodes, false, em, window);
        let color = context.text_style().color;
        Some(InlineElement::new(draw(b.clone(), em, color)).with_baseline(em * b.h))
    }
}

/// Display math: a formula of its own, centred, scrolling sideways when it's wider than the
/// column. `size`: the text size; `ink`: the text colour.
pub struct DisplayMath {
    pub size: Pixels,
    pub ink: Hsla,
}

impl MarkdownPlugin for DisplayMath {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        DISPLAY
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let source = cx.node_source(node)?;
        match node {
            markdown_ast::Node::InlineMath(_) if masked(node) => {
                let formula = Formula::of(source)?;
                formula.display.then(|| formula.node(DISPLAY))
            }
            // A `$$` block Markdown found on its own: one that never closed. As written.
            markdown_ast::Node::Math(_) => {
                let formula = Formula::of(source).unwrap_or(Formula {
                    source: source.to_string(),
                    display: true,
                    tex: String::new(),
                    nodes: None,
                });
                Some(formula.node(DISPLAY))
            }
            _ => None,
        }
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, _: &mut App) -> impl IntoElement {
        let Some(formula) = node.data::<Formula>() else {
            return div().into_any_element();
        };
        let Some(nodes) = formula.nodes.as_ref() else {
            // Still being written: plain text until it closes.
            return div()
                .w_full()
                .text_color(self.ink)
                .text_size(self.size)
                .whitespace_normal()
                .child(formula.source.clone())
                .into_any_element();
        };
        let em = self.size * SCALE;
        let b = typeset(&formula.tex, nodes, true, em, window);
        let key = node.source_range().map_or(0, |r| r.start);
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id(SharedString::from(format!("math-{key}")))
                    .max_w_full()
                    .overflow_x_scroll()
                    // Room for the formula's ink past its box (an italic letter's tail).
                    .px(em * 0.1)
                    .py(em * 0.15)
                    .child(draw(b, em, self.ink)),
            )
            .into_any_element()
    }
}

/// The text system's widths for runs the font tables can't give.
struct Shaper<'a> {
    window: &'a Window,
    em: Pixels,
}

impl Measure for Shaper<'_> {
    fn width(&self, text: &str, face: Face) -> Option<f32> {
        let run = TextRun {
            len: text.len(),
            font: font_of(face),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = self.window.text_system().shape_line(
            SharedString::from(text.to_string()),
            self.em,
            &[run],
            None,
        );
        Some(line.width / self.em)
    }
}

fn font_of(face: Face) -> Font {
    let mut f = font(face.family());
    f.weight = if face.bold() {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    f.style = if face.italic() {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    f
}

/// Laid-out formulas by TeX, display and size: drawing is every frame, setting needn't be.
type LaidOut = HashMap<(String, bool, u32), Arc<MBox>>;
static LAID_OUT: LazyLock<Mutex<LaidOut>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn typeset(
    tex: &str,
    nodes: &[parse::Node],
    display: bool,
    em: Pixels,
    window: &Window,
) -> Arc<MBox> {
    let key = (tex.to_string(), display, em.as_f32().to_bits());
    if let Some(b) = LAID_OUT.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return b.clone();
    }
    let b = Arc::new(layout::typeset(nodes, display, &Shaper { window, em }));
    let mut cache = LAID_OUT.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 2048 {
        cache.clear();
    }
    cache.insert(key, b.clone());
    b
}

/// An element `em` per em, drawing `b` in `color`.
/// A pixel deeper than the box: a formula with nothing below its baseline (`\vec{v}`) would
/// otherwise have its baseline on its bottom edge, where layout's rounding can push it outside,
/// and the text view would show its source instead.
fn draw(b: Arc<MBox>, em: Pixels, color: Hsla) -> impl IntoElement {
    let (w, h) = (em * b.w.max(0.01), em * (b.h + b.d).max(0.01) + px(1.));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| paint(&b, bounds.origin, em, color, window, cx),
    )
    .w(w)
    .h(h)
    .flex_none()
}

fn paint(
    b: &MBox,
    origin: Point<Pixels>,
    em: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    // Box coordinates are ems up from the baseline; the window's are pixels down from the top.
    let baseline = origin.y + em * b.h;
    let at = |x: f32, y: f32| point(origin.x + em * x, baseline - em * y);
    let snap = |v: Pixels| px((v.as_f32() * scale).round() / scale);
    // A rule at least a device pixel thick, on whole device pixels: crisp, never blurred away.
    let rule = |x: f32, y: f32, w: f32, h: f32, window: &mut Window| {
        let top = at(x, y + h);
        let thick = px(((em * h).as_f32() * scale).round().max(1.) / scale);
        let bounds = Bounds::new(point(top.x, snap(top.y)), size(em * w, thick));
        window.paint_quad(fill(bounds, color));
    };
    for item in &b.items {
        match item {
            Item::Glyph {
                x,
                y,
                text,
                face,
                size,
            } => {
                let font_size = em * *size;
                let run = TextRun {
                    len: text.len(),
                    font: font_of(*face),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(
                    SharedString::from(text.clone()),
                    font_size,
                    &[run],
                    None,
                );
                let p = at(*x, *y);
                let _ = line.paint(
                    point(p.x, p.y - line.ascent),
                    line.ascent + line.descent,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            Item::Rule { x, y, w, h } => rule(*x, *y, *w, *h, window),
            Item::Frame { x, y, w, h, t } => {
                rule(*x, *y, *w, *t, window);
                rule(*x, *y + *h - *t, *w, *t, window);
                let left = Bounds::new(
                    at(*x, *y + *h),
                    size(
                        px(((em * *t).as_f32() * scale).round().max(1.) / scale),
                        em * *h,
                    ),
                );
                window.paint_quad(fill(left, color));
                let right = Bounds::new(
                    at(*x + *w - *t, *y + *h),
                    size(
                        px(((em * *t).as_f32() * scale).round().max(1.) / scale),
                        em * *h,
                    ),
                );
                window.paint_quad(fill(right, color));
            }
            Item::Shape {
                x,
                y,
                w,
                h,
                t,
                shape: Shape::Strike { forward },
            } => {
                // A diagonal as a run of device-pixel squares: quads only, no paths (GPUI's
                // path pipeline stalls the GPU drawn this many times in a text view).
                let (x0, top) = (at(*x, *y + *h), em * *h);
                let width = em * *w;
                let thick = px(((em * *t).as_f32() * scale).round().max(1.) / scale);
                let steps = ((width.max(top).as_f32() * scale).ceil() as usize).clamp(1, 4000);
                for i in 0..steps {
                    let f = i as f32 / steps as f32;
                    let fy = if *forward { 1. - f } else { f };
                    let p = point(x0.x + width * f, x0.y + top * fy);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(p.x - thick / 2., p.y - thick / 2.),
                            size(thick, thick),
                        ),
                        color,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would shadow the standard one.
    use super::{DISPLAY, Formula};

    #[test]
    fn formulas_keep_their_source_as_text() {
        let f = Formula::of(r"\[ P(\theta=1\mid T=+) \]").unwrap();
        assert!(f.display && f.nodes.is_some());
        let node = f.node(DISPLAY);
        assert_eq!(node.as_text(), r"\[ P(\theta=1\mid T=+) \]");
        assert_eq!(node.as_markdown(), r"\[ P(\theta=1\mid T=+) \]");
        // Open, it has nothing to draw: its source shows as text.
        let open = Formula::of(r"\[ P(\theta").unwrap();
        assert!(open.nodes.is_none() && open.display);
        assert!(Formula::of("$5 and $").is_none());
    }
}
