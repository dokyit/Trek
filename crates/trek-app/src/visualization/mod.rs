//! Native, non-executable visualizations embedded in an assistant answer.
//!
//! Agents emit a fenced `trek-viz` JSON block. The core crate owns and validates the bounded
//! wire format; this module only turns the parsed data into theme-aware GPUI elements. A block
//! still being written draws as far as it validates (its frame, then its marks as they arrive);
//! one that closed invalid shows its JSON under a note saying why it couldn't be drawn.
//!
//! Every artifact is built from the same three layers: the card (a hairline edge and the faint
//! sheen of Trek's glass panes), an inset surface the marks sit on, and floating readouts above
//! both. Marks wear theme and palette colours only; text always wears text colours.

mod charts;
mod diagrams;
mod layout;
mod mockup;

use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash as _, Hasher as _};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use trek_core::visualization::{MockNode, SemanticTone, TableCell, TimePoint, Visualization, VisualizationContent};

use crate::assets::Lucide;

const PLUGIN: &str = "trek-visualization";
/// How long marks take to arrive the first time a visualization is drawn.
pub(crate) const ENTER: Duration = Duration::from_millis(720);
/// How long a hovered mark takes to lift.
const LIFT: Duration = Duration::from_millis(160);
/// One sweep of the shimmer over a visualization still being written.
const SHIMMER: Duration = Duration::from_millis(1600);
/// The width assumed before the first frame measures the real one.
const ASSUMED_WIDTH: f32 = 620.;
/// Below this the artifact is in a side bar: tighter type, stacked legends.
pub(super) const NARROW: f32 = 460.;

/// A `trek-viz` block as parsed.
#[derive(Clone)]
enum Block {
    /// Closed and valid.
    Drawn { visualization: Visualization, ident: u64 },
    /// Its fence hasn't closed: what's been written so far.
    Arriving {
        kind: Option<String>,
        title: Option<String>,
        /// As far as it validates, in whole marks, with its identity.
        drawable: Option<(Visualization, u64)>,
        /// The JSON is all there; only the closing fence is missing.
        complete: bool,
        source: String,
    },
    /// Closed, but not something Trek can draw: why, and the block as written.
    Invalid { reason: String, source: String },
}

/// Identifies one visualization across re-renders, re-parses and the deltas of its stream (its
/// type and title, both written before its marks), for its one-time entrance.
fn ident(visualization: &Visualization) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (visualization.content.kind(), &visualization.title).hash(&mut hasher);
    hasher.finish()
}

/// `source`, a fenced block as Markdown found it, ends in its closing fence. An open fence runs to
/// the end of the answer, so while an agent is still writing the block its last line is JSON.
fn fence_closed(source: &str) -> bool {
    source.trim_end().rsplit_once('\n').is_some_and(|(_, last)| {
        let last = last.trim();
        last.len() >= 3 && last.bytes().all(|b| b == b'`')
    })
}

/// The assistant-answer Markdown extension: claims every `trek-viz` fence. `live`: the answer
/// is still being written, so a fence still open is one on its way rather than one never closed.
/// `size`: the answer's text size, for an invalid block's code.
#[derive(Clone, Copy)]
pub struct VisualizationPlugin {
    pub live: bool,
    pub size: Pixels,
}

impl MarkdownPlugin for VisualizationPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        PLUGIN
    }

    // Runs again on each delta while the block is the answer's last (the text view reparses
    // only its last block): one scan and a few bounded parses of at most 32 KiB, off the main
    // thread, so the cost is small and stays flat.
    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Code(code) = node else {
            return None;
        };
        if code.lang.as_deref() != Some("trek-viz") {
            return None;
        }
        let source = cx.node_source(node)?;
        let (block, text) = if fence_closed(source) {
            match trek_core::visualization::parse_fenced(source) {
                Ok(visualization) => {
                    let text = accessible_description(&visualization);
                    (Block::Drawn { ident: ident(&visualization), visualization }, text)
                }
                Err(error) => (Block::Invalid { reason: error.reason(), source: source.to_string() }, source.to_string()),
            }
        } else {
            let partial = trek_core::visualization::parse_partial(&code.value);
            let text = match &partial.drawable {
                Some(visualization) => accessible_description(visualization),
                None => format!("Visualization{}, being drawn", partial.title.as_deref().map(|t| format!(": {t}")).unwrap_or_default()),
            };
            let drawable = partial.drawable.map(|v| {
                let id = ident(&v);
                (v, id)
            });
            (Block::Arriving { kind: partial.kind, title: partial.title, drawable, complete: partial.complete, source: source.to_string() }, text)
        };
        Some(MarkdownNode::new(PLUGIN, block).text(text.clone()).markdown(source.to_string()).accessibility_label(text))
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = node.source_range().map(|range| range.start).unwrap_or(0);
        // Padding, not margin: a margin here collapses through the Markdown block it sits in,
        // and the answer would be measured shorter than it draws.
        let shown = match node.data::<Block>() {
            Some(Block::Drawn { visualization, ident }) => artifact(visualization, key, *ident, false, window, cx),
            Some(Block::Arriving { drawable: Some((visualization, ident)), complete, .. }) if self.live || *complete => {
                artifact(visualization, key, *ident, self.live && !complete, window, cx)
            }
            Some(Block::Arriving { kind, title, .. }) if self.live => pending(kind.as_deref(), title.as_deref(), key, window, cx),
            Some(Block::Arriving { source, .. }) => invalid("the answer ended before the block was finished", source, key, self.size, cx),
            Some(Block::Invalid { reason, source }) => invalid(reason, source, key, self.size, cx),
            None => return div().into_any_element(),
        };
        div().py_2().child(shown).into_any_element()
    }
}

// ---------- state ----------

/// What a reader has done to one artifact: kept by the window for as long as it's drawn.
#[derive(Default)]
pub(super) struct VizState {
    /// The mark under the pointer (a bar, slice, series, node, layer, tile…).
    hover: Option<usize>,
    hover_at: Option<Instant>,
    /// The line chart's crosshair, as an x index.
    cursor: Option<usize>,
    /// Series or parts switched off from the legend.
    hidden: BTreeSet<usize>,
    /// The Data view instead of the chart.
    data: bool,
    /// The body's width as last drawn.
    width: Option<f32>,
}

/// Bumped whenever an artifact's height changes after it was drawn (its width measured, the
/// Data view switched): transcripts that cache their rows' heights measure them again.
#[derive(Default)]
pub struct Relayout(pub u64);
impl Global for Relayout {}

fn relayout(cx: &mut App) {
    let n = cx.try_global::<Relayout>().map_or(0, |r| r.0);
    cx.set_global(Relayout(n + 1));
}

/// When each visualization first appeared, so its entrance plays once — not again on every
/// re-render, nor when a window drops and rebuilds its state.
static ENTERED: LazyLock<Mutex<HashMap<u64, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// How far the entrance of `hash` has played, 0 to 1 (1 at once without motion).
fn entrance(hash: u64, motion: bool, window: &mut Window) -> f32 {
    if !motion {
        return 1.;
    }
    let mut entered = ENTERED.lock().unwrap_or_else(|e| e.into_inner());
    if entered.len() > 4096 {
        entered.retain(|_, at| at.elapsed() < ENTER);
    }
    let at = *entered.entry(hash).or_insert_with(Instant::now);
    let t = at.elapsed().as_secs_f32() / ENTER.as_secs_f32();
    if t < 1. {
        window.request_animation_frame();
    }
    t.min(1.)
}

/// Motion is allowed: neither Trek's Reduce motion setting nor the system asks for less.
fn motion(cx: &App) -> bool {
    match cx.try_global::<crate::workspace::GlobalWorkspace>() {
        Some(ws) => ws.0.read(cx).motion(cx),
        None => !cx.reduce_motion(),
    }
}

/// The colours every part of an artifact draws with, from the theme.
#[derive(Clone)]
pub(super) struct Look {
    pub dark: bool,
    pub fg: Hsla,
    pub muted: Hsla,
    /// Hairline edges and rules.
    pub hairline: Hsla,
    /// Gridlines: a step quieter than the hairline.
    pub grid: Hsla,
    /// The inset surface marks sit on.
    pub inset: Hsla,
    /// What separates touching marks: the inset surface, opaque.
    pub gap: Hsla,
    pub popover: Hsla,
    pub mono: SharedString,
}

impl Look {
    fn of(cx: &App) -> Look {
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        Look {
            dark,
            fg: theme.foreground,
            muted: theme.muted_foreground,
            hairline: theme.foreground.opacity(if dark { 0.10 } else { 0.11 }),
            grid: theme.foreground.opacity(if dark { 0.06 } else { 0.07 }),
            inset: if dark { theme.background.opacity(0.6) } else { theme.muted.opacity(0.55) },
            gap: if dark { theme.background } else { theme.muted.blend(theme.background.opacity(0.45)) },
            popover: theme.popover,
            mono: theme.mono_font_family.clone(),
        }
    }
}

/// One frame of one artifact: its identity, the reader's state, and how far its entrance is.
#[derive(Clone)]
pub(super) struct Frame {
    pub key: usize,
    state: Entity<VizState>,
    pub hover: Option<usize>,
    /// How far the hovered mark has lifted, 0 to 1.
    pub lift: f32,
    pub cursor: Option<usize>,
    pub hidden: BTreeSet<usize>,
    pub width: f32,
    /// The entrance, 0 to 1, linear (marks ease and stagger it themselves).
    pub enter: f32,
    pub look: Look,
}

impl Frame {
    /// A stable element id for part `ix` of kind `part`: `viz-<part>-<key>-<ix>`.
    pub fn id(&self, part: &str, ix: usize) -> SharedString {
        SharedString::from(format!("viz-{part}-{}-{ix}", self.key))
    }

    pub fn narrow(&self) -> bool {
        self.width < NARROW
    }

    /// Mark `ix` of `n`'s share of the entrance, eased: they arrive one after another.
    pub fn arrive(&self, ix: usize, n: usize) -> f32 {
        arrive(self.enter, ix, n)
    }

    /// How present mark `ix` is while another one is hovered: the hovered one and, with nothing
    /// hovered, every one at full strength; the rest step back.
    pub fn presence(&self, ix: usize) -> f32 {
        match self.hover {
            Some(h) if h != ix => 1. - 0.62 * self.lift.max(0.6),
            _ => 1.,
        }
    }

    /// A hover listener that makes `ix` the hovered mark, and lets go of it on leaving.
    pub fn on_hover(&self, ix: usize) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let state = self.state.clone();
        move |hovered, _, cx| {
            let hovered = *hovered;
            state.update(cx, |s, cx| {
                if hovered && s.hover != Some(ix) {
                    s.hover = Some(ix);
                    s.hover_at = Some(Instant::now());
                    cx.notify();
                } else if !hovered && s.hover == Some(ix) {
                    s.hover = None;
                    s.hover_at = None;
                    cx.notify();
                }
            });
        }
    }

    /// Set the hovered mark directly (for marks found by pointer position, not their own box).
    pub fn set_hover(state: &Entity<VizState>, hover: Option<usize>, cx: &mut App) {
        state.update(cx, |s, cx| {
            if s.hover != hover {
                s.hover = hover;
                s.hover_at = hover.map(|_| Instant::now());
                cx.notify();
            }
        });
    }

    pub fn state(&self) -> Entity<VizState> {
        self.state.clone()
    }

    pub fn set_cursor(state: &Entity<VizState>, cursor: Option<usize>, cx: &mut App) {
        state.update(cx, |s, cx| {
            if s.cursor != cursor {
                s.cursor = cursor;
                cx.notify();
            }
        });
    }

    /// A click listener that switches series or part `ix` on or off — never all of them off.
    pub fn toggle(&self, ix: usize, count: usize) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let state = self.state.clone();
        move |_, _, cx| {
            state.update(cx, |s, cx| {
                if !s.hidden.remove(&ix) && s.hidden.len() + 1 < count {
                    s.hidden.insert(ix);
                }
                cx.notify();
            });
        }
    }
}

/// Mark `ix` of `n`'s share of an entrance `enter` of the way through (0 to 1), eased: they
/// arrive one after another, the last starting a third of the way in. Basecamp's bars too.
pub(crate) fn arrive(enter: f32, ix: usize, n: usize) -> f32 {
    if enter >= 1. {
        return 1.;
    }
    let delay = if n > 1 { ix as f32 / (n - 1) as f32 * 0.35 } else { 0. };
    ease_out(((enter - delay) / 0.65).clamp(0., 1.))
}

pub(super) fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

// ---------- colour ----------

/// A mark's colour for its semantic tone; no tone is the accent.
pub(super) fn tone(tone: Option<SemanticTone>, cx: &App) -> Hsla {
    match tone.unwrap_or(SemanticTone::Accent) {
        SemanticTone::Neutral => cx.theme().muted_foreground,
        SemanticTone::Accent => crate::palette::ember(cx),
        SemanticTone::Positive => crate::palette::emerald(cx),
        SemanticTone::Warning => crate::palette::amber(cx),
        SemanticTone::Negative => crate::palette::red(cx),
        SemanticTone::Info => crate::palette::sky(cx),
    }
}

/// Series `ix`'s colour: its tone when it has one, else the next categorical hue — by its
/// place in the data, so switching others off never repaints it.
pub(super) fn series_color(given: Option<SemanticTone>, ix: usize, cx: &App) -> Hsla {
    match given {
        Some(t) => tone(Some(t), cx),
        None => crate::palette::series(ix, cx),
    }
}

// ---------- numbers ----------

/// A number for reading: grouped thousands, compact from ten thousand, at most two decimals.
pub(super) fn format_number(value: f64) -> String {
    let abs = value.abs();
    let compact = |div: f64, suffix: &str| {
        let v = value / div;
        let s = if (v * 10.).round() % 10. == 0. { format!("{v:.0}") } else { format!("{v:.1}") };
        format!("{s}{suffix}")
    };
    if abs >= 1e12 {
        compact(1e12, "T")
    } else if abs >= 1e9 {
        compact(1e9, "B")
    } else if abs >= 1e6 {
        compact(1e6, "M")
    } else if abs >= 1e4 {
        compact(1e3, "K")
    } else {
        let decimals = if value.fract().abs() < 1e-9 {
            0
        } else if (value * 10.).fract().abs() < 1e-9 {
            1
        } else {
            2
        };
        group_thousands(&format!("{value:.decimals$}"))
    }
}

/// A number exactly as given, thousands grouped: for tables, where figures are looked up.
pub(super) fn format_exact(value: f64) -> String {
    if value.fract().abs() < 1e-9 && value.abs() < 1e15 {
        group_thousands(&format!("{value:.0}"))
    } else {
        let text = format!("{value}");
        if text.contains('e') { text } else { group_thousands(&text) }
    }
}

/// A tick label: as many decimals as the step needs, and no more.
pub(super) fn format_tick(value: f64, step: f64) -> String {
    if value.abs() >= 1e4 {
        return format_number(value);
    }
    // As many decimals as the step has: 0.25 needs two, 0.5 one, 200 none.
    let decimals = (0..6).find(|d| (step * 10f64.powi(*d as i32)).fract().abs() < 1e-6).unwrap_or(6);
    group_thousands(&format!("{value:.decimals$}"))
}

fn group_thousands(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

/// `text` with its unit: currency before, a percent sign tight after, anything else spaced.
pub(super) fn with_unit(text: String, unit: Option<&str>) -> String {
    match unit {
        None => text,
        Some(u @ ("$" | "€" | "£" | "¥")) => match text.strip_prefix('-') {
            Some(rest) => format!("-{u}{rest}"),
            None => format!("{u}{text}"),
        },
        Some("%") => format!("{text}%"),
        Some(u) => format!("{text} {u}"),
    }
}

// ---------- the artifact ----------

fn kind_icon(content: &VisualizationContent) -> Lucide {
    kind_icon_named(content.kind())
}

/// A type's icon by its wire name; one not known (or not yet written) gets a plain chart.
fn kind_icon_named(kind: &str) -> Lucide {
    match kind {
        "bar" => Lucide::ChartBarBig,
        "line" => Lucide::ChartSpline,
        "donut" => Lucide::ChartPie,
        "stats" => Lucide::Gauge,
        "table" => Lucide::Table2,
        "heatmap" => Lucide::Grid3x3,
        "treemap" => Lucide::LayoutDashboard,
        "timeline" => Lucide::ChartGantt,
        "flow" => Lucide::Workflow,
        "layers" => Lucide::Layers,
        "mockup" => Lucide::AppWindow,
        _ => Lucide::ChartNoAxesColumn,
    }
}

/// The card every artifact sits in: a hairline edge, raised a step off the column, and the
/// light a glass pane catches along its top. (Callers add `test_support`, which wraps it.)
fn shell(id: SharedString, look: &Look, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let sheen_top = gpui_kit::white().opacity(if look.dark { 0.07 } else { 0.85 });
    let sheen = gpui_kit::white().opacity(if look.dark { 0.022 } else { 0.0 });
    v_flex()
        .id(id)
        .relative()
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_color(look.hairline)
        // Raised off the column: a step lighter at night, the paper itself by day.
        .bg(if look.dark { theme.foreground.opacity(0.022) } else { theme.background })
        .when(!look.dark, |el| {
            el.shadow(vec![BoxShadow { color: gpui_kit::black().opacity(0.04), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
        })
        .overflow_hidden()
        // The light a glass pane catches: a bright rim and a sheen fading down from it.
        .child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(sheen_top))
        .child(div().absolute().top_0().left_0().right_0().h(px(64.)).bg(linear_gradient(180., linear_color_stop(sheen, 0.), linear_color_stop(sheen.opacity(0.), 1.))))
}

/// The title row: what it is, its name, and what can be done with it; the summary runs the full
/// width beneath, so a narrow column doesn't squeeze it beside the actions.
fn header(icon: Lucide, title: AnyElement, actions: AnyElement, summary: AnyElement, look: &Look) -> Div {
    v_flex()
        .w_full()
        .gap(px(4.))
        .pl_4()
        .pr_3()
        .pt(px(12.))
        .pb(px(12.))
        .child(
            h_flex()
                .w_full()
                .gap(px(10.))
                .child(
                    div()
                        .flex_none()
                        .size(px(22.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(look.hairline)
                        .bg(look.inset)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size(px(12.)).text_color(look.muted)),
                )
                .child(div().flex_1().min_w_0().child(title))
                .child(actions),
        )
        .child(div().pl(px(32.)).child(summary))
}

fn title_text(title: impl Into<SharedString>, look: &Look) -> AnyElement {
    div().line_clamp(2).text_size(px(13.5)).line_height(relative(1.3)).font_semibold().text_color(look.fg).child(title.into()).into_any_element()
}

fn summary_text(summary: impl Into<SharedString>, look: &Look) -> AnyElement {
    div().text_size(px(12.)).line_height(relative(1.45)).text_color(look.muted).child(summary.into()).into_any_element()
}

/// 0 to 1 and round again, once per `SHIMMER`, for everything still being drawn; it keeps
/// frames coming while anything asks for it. Without motion it holds still at the middle.
fn shimmer(motion: bool, window: &mut Window) -> f32 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    if !motion {
        return 0.5;
    }
    window.request_animation_frame();
    (START.elapsed().as_secs_f32() % SHIMMER.as_secs_f32()) / SHIMMER.as_secs_f32()
}

/// "Drawing…", breathing with the shimmer: in a card's corner while its marks are arriving.
fn drawing_label(key: usize, phase: f32, look: &Look) -> AnyElement {
    let breath = 0.55 + 0.45 * (phase * std::f32::consts::TAU).cos().abs();
    h_flex()
        .id(SharedString::from(format!("viz-drawing-{key}")))
        .test_support()
        .flex_none()
        .h(px(24.))
        .pr_1()
        .text_size(px(11.))
        .text_color(look.muted)
        .opacity(breath)
        .child("Drawing…")
        .into_any_element()
}

fn artifact(viz: &Visualization, key: usize, ident: u64, drawing: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state(SharedString::from(format!("trek-viz-state-{key}")), cx, |_, _| VizState::default());
    let motion = motion(cx);
    let enter = entrance(ident ^ key as u64, motion, window);
    let (hover, hover_at, cursor, hidden, showing_data, width) = {
        let s = state.read(cx);
        (s.hover, s.hover_at, s.cursor, s.hidden.clone(), s.data, s.width)
    };
    let lift = match hover_at {
        Some(at) if motion => {
            let t = at.elapsed().as_secs_f32() / LIFT.as_secs_f32();
            if t < 1. {
                window.request_animation_frame();
            }
            ease_out(t)
        }
        _ => 1.,
    };
    let look = Look::of(cx);
    let frame = Frame { key, state: state.clone(), hover, lift, cursor, hidden, width: width.unwrap_or(ASSUMED_WIDTH), enter, look: look.clone() };
    let data = data_table(viz);
    let body = if showing_data && let Some(data) = &data {
        charts::table_view(&data.columns, &data.rows.iter().map(|r| (r.clone(), None)).collect::<Vec<_>>(), &frame, "datatable", cx)
    } else {
        match &viz.content {
            VisualizationContent::Bar { bars, x_label, y_label } => charts::bars(bars, x_label.as_deref(), y_label.as_deref(), &frame, cx),
            VisualizationContent::Line { x_labels, series, area, y_label, unit } => charts::line(x_labels, series, *area, y_label.as_deref(), unit.as_deref(), &frame, cx),
            VisualizationContent::Donut { parts, unit } => charts::donut(parts, unit.as_deref(), &frame, cx),
            VisualizationContent::Stats { tiles } => charts::stats(tiles, &frame, cx),
            VisualizationContent::Table { columns, rows } => {
                let rows: Vec<(Vec<TableCell>, Option<SemanticTone>)> = rows.iter().map(|r| (r.cells().to_vec(), r.tone())).collect();
                charts::table_view(columns, &rows, &frame, "table", cx)
            }
            VisualizationContent::Heatmap { x_labels, y_labels, values } => charts::heatmap(x_labels, y_labels, values, &frame, cx),
            VisualizationContent::Treemap { items } => charts::treemap(items, &frame, cx),
            VisualizationContent::Timeline { events, ticks, unit } => diagrams::timeline(events, ticks.as_deref(), unit.as_deref(), &frame, cx),
            VisualizationContent::Flow { nodes, edges } => diagrams::flow(nodes, edges, &frame, cx),
            VisualizationContent::Layers { layers } => diagrams::layers(layers, &frame, cx),
            VisualizationContent::Mockup { nodes } => mockup::mockup(nodes, &frame, cx),
        }
    };

    let group = SharedString::from(format!("trek-viz-group-{key}"));
    // The body's width, for layouts that change with it (labels thinned, legends stacked).
    let probe = {
        let state = state.clone();
        let known = width;
        canvas(
            move |bounds, window, _| {
                let w = f32::from(bounds.size.width);
                if known.is_none_or(|k| (k - w).abs() > 1.5) {
                    window.on_next_frame(move |_, cx| {
                        state.update(cx, |s, cx| {
                            s.width = Some(w);
                            cx.notify();
                        });
                        relayout(cx);
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(1.))
    };
    // While its marks are still arriving there's nothing whole to copy or tabulate yet.
    let actions = if drawing {
        drawing_label(key, shimmer(motion, window), &look)
    } else {
        let json = serde_json::to_string_pretty(viz).unwrap_or_default();
        let markdown = data.as_ref().map(|d| d.markdown());
        h_flex()
            .flex_none()
            .gap(px(2.))
            .opacity(0.72)
            .group_hover(group.clone(), |s| s.opacity(1.))
            // A table is its own data: no second view of it.
            .when(data.is_some() && !matches!(viz.content, VisualizationContent::Table { .. }), |el| el.child(view_switch(&frame, showing_data, cx)))
            .when_some(markdown, |el, markdown| el.child(copy_button(frame.id("copy", 0), Lucide::Sheet, "Copy as Markdown table", markdown, "Table copied", None, &look)))
            .child(copy_button(frame.id("json", 0), Lucide::Braces, "Copy data as JSON", json, "Visualization data copied", None, &look))
            .into_any_element()
    };
    let csv = data.as_ref().map(|d| d.csv());

    shell(SharedString::from(format!("trek-viz-{key}")), &look, cx)
        .test_support()
        .group(group.clone())
        .child(header(kind_icon(&viz.content), title_text(viz.title.clone(), &look), actions, summary_text(viz.summary.clone(), &look), &look))
        .child(
            div()
                // No scroll of its own: the transcript scrolls, and a box that scrolls inside
                // one that scrolls traps the wheel (every type is bounded by the schema).
                .id(("viz-body", key))
                .relative()
                .w_full()
                .px_4()
                .pb_4()
                .child(probe)
                .child(body)
                .when(showing_data && !drawing, |el| {
                    el.when_some(csv, |el, csv| {
                        el.child(
                            h_flex().pt_2().justify_end().child(copy_button(frame.id("csv", 0), Lucide::Copy, "Copy as CSV", csv, "CSV copied", Some("CSV"), &look)),
                        )
                    })
                }),
        )
        .into_any_element()
}

/// Roughly how tall a type's body draws, so the frame shown while it's written doesn't jump
/// when the marks replace it.
fn expected_height(kind: Option<&str>) -> f32 {
    match kind {
        Some("stats") => 96.,
        Some("bar") => 170.,
        Some("heatmap" | "timeline") => 170.,
        Some("donut") => 200.,
        Some("table") => 200.,
        Some("line" | "treemap" | "flow") => 230.,
        Some("layers") => 250.,
        Some("mockup") => 320.,
        _ => 180.,
    }
}

/// A visualization still being written with nothing whole to draw yet: its card, with its type
/// and title as soon as they're written, and a quiet skeleton of its marks where they will go.
fn pending(kind: Option<&str>, title: Option<&str>, key: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let look = Look::of(cx);
    let motion = motion(cx);
    let phase = shimmer(motion, window);
    let stub = look.fg.opacity(if look.dark { 0.07 } else { 0.06 });
    let bar = |w: f32, h: f32| div().w(relative(w)).h(px(h)).rounded(px(h / 2.)).bg(stub);
    let label = match title {
        Some(t) => format!("Visualization being drawn: {t}"),
        None => "Visualization being drawn".into(),
    };
    let title = match title {
        Some(t) => title_text(t.to_string(), &look),
        None => div().h(px(17.)).flex().items_center().child(bar(0.42, 10.)).into_any_element(),
    };
    let summary = div().h(px(17.)).flex().items_center().child(bar(0.6, 8.)).into_any_element();
    let height = expected_height(kind);
    // A hint of the marks to come, in the type's own shape.
    let marks = match kind {
        Some("bar" | "timeline" | "layers" | "table") => v_flex()
            .size_full()
            .p_3()
            .gap(px(12.))
            .children([0.82, 0.58, 0.7, 0.36, 0.5].into_iter().map(|w| h_flex().gap(px(10.)).child(div().w(px(64.)).flex_none().child(bar(1., 8.))).child(div().flex_1().child(bar(w, 12.)))))
            .into_any_element(),
        Some("stats") => h_flex()
            .size_full()
            .p_3()
            .gap_3()
            .children((0..3).map(|_| v_flex().flex_1().gap(px(10.)).child(bar(0.5, 8.)).child(bar(0.7, 16.)).child(bar(0.35, 8.))))
            .into_any_element(),
        _ => div().into_any_element(),
    };
    // A soft band of light crossing the surface, left to right: thin slices brightening to its
    // middle and fading out again (a gradient to transparent draws with hard edges here).
    const SLICES: usize = 16;
    let band = look.fg.opacity(if look.dark { 0.05 } else { 0.055 });
    let sweep = h_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(phase * 1.6 - 0.4))
        .w(relative(0.4))
        .children((0..SLICES).map(|i| {
            let k = ((i as f32 + 0.5) / SLICES as f32 * std::f32::consts::PI).sin();
            div().flex_1().h_full().bg(band.opacity(k * k))
        }));
    shell(SharedString::from(format!("viz-pending-{key}")), &look, cx)
        .aria_label(label)
        .test_support()
        .child(header(kind_icon_named(kind.unwrap_or("")), title, drawing_label(key, phase, &look), summary, &look))
        .child(div().w_full().px_4().pb_4().child(inset(&look).relative().overflow_hidden().w_full().h(px(height)).child(marks).when(motion, |el| el.child(sweep))))
        .into_any_element()
}

/// A block that closed but can't be drawn: a short note saying why, over the JSON as written.
fn invalid(reason: &str, source: &str, key: usize, size: Pixels, cx: &App) -> AnyElement {
    let look = Look::of(cx);
    let note = format!("Couldn't draw this visualization: {reason}");
    v_flex()
        .id(SharedString::from(format!("viz-invalid-{key}")))
        .aria_label(note.clone())
        .test_support()
        .w_full()
        .gap(px(6.))
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .border_1()
                .border_color(look.hairline)
                .bg(crate::palette::amber(cx).opacity(if look.dark { 0.06 } else { 0.07 }))
                .text_size(px(12.))
                .line_height(relative(1.45))
                .child(div().flex_none().h(px(12. * 1.45)).flex().items_center().child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(crate::palette::amber(cx))))
                .child(div().flex_1().min_w_0().text_color(look.muted).child(note)),
        )
        .child(crate::md::keyed(SharedString::from(format!("viz-raw-{key}")), source.to_string(), None, None, size, false, false, cx))
        .into_any_element()
}

/// Chart | Data: the two views of the same values, as a segmented control.
fn view_switch(frame: &Frame, showing_data: bool, cx: &App) -> AnyElement {
    let look = &frame.look;
    let raised = cx.theme().background;
    let segment = |label: &'static str, active: bool, data: bool| {
        let state = frame.state();
        div()
            .id(frame.id(if data { "data" } else { "chart" }, 0))
            .test_support()
            .px(px(8.))
            .h(px(20.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .text_size(px(11.))
            .cursor_pointer()
            .when(active, |el| {
                el.bg(raised).text_color(look.fg).font_medium().shadow(vec![BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.4 } else { 0.08 }), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
            })
            .when(!active, |el| el.text_color(look.muted).hover(|s| s.text_color(look.fg)))
            .on_click(move |_, _, cx| {
                state.update(cx, |s, cx| {
                    s.data = data;
                    s.hover = None;
                    cx.notify();
                });
                relayout(cx);
            })
            .child(label)
    };
    h_flex()
        .mr_1()
        .p(px(2.))
        .gap(px(1.))
        .rounded(px(7.))
        .bg(look.fg.opacity(0.055))
        .child(segment("Chart", !showing_data, false))
        .child(segment("Data", showing_data, true))
        .into_any_element()
}

fn copy_button(id: SharedString, icon: Lucide, tip: &'static str, text: String, done: &'static str, label: Option<&'static str>, look: &Look) -> AnyElement {
    let muted = look.muted;
    let hover = look.fg.opacity(0.06);
    h_flex()
        .id(id)
        .test_support()
        .h(px(24.))
        .min_w(px(24.))
        .px(px(5.))
        .gap(px(4.))
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(hover))
        .child(Icon::new(icon).size(px(13.)).text_color(muted))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
        .on_click(move |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            crate::toast::push(window, done, cx);
        })
        .when_some(label, |el, label| el.child(div().text_size(px(11.)).text_color(muted).child(label)))
        .into_any_element()
}

/// A floating readout: the top layer, over the plot.
pub(super) fn readout(look: &Look) -> Div {
    v_flex()
        .gap(px(3.))
        .px(px(10.))
        .py(px(7.))
        .rounded(px(8.))
        .border_1()
        .border_color(look.fg.opacity(if look.dark { 0.14 } else { 0.10 }))
        .bg(look.popover)
        .shadow(vec![
            BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.45 } else { 0.10 }), offset: point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(-4.), inset: false },
            BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.3 } else { 0.05 }), offset: point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false },
        ])
        .text_size(px(11.5))
}

/// The inset surface marks sit on.
pub(super) fn inset(look: &Look) -> Div {
    div().rounded(px(9.)).border_1().border_color(look.fg.opacity(if look.dark { 0.05 } else { 0.06 })).bg(look.inset)
}

// ---------- data: the table behind every chart ----------

/// A chart's values as rows of cells: the Data view, and what the copy buttons copy.
pub(super) struct DataTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<TableCell>>,
}

impl DataTable {
    fn cell_text(cell: &TableCell) -> String {
        match cell {
            TableCell::Number(n) => {
                if n.fract() == 0. && n.abs() < 1e15 { format!("{n:.0}") } else { n.to_string() }
            }
            TableCell::Text(t) => t.clone(),
        }
    }

    pub fn markdown(&self) -> String {
        let escape = |s: String| s.replace('|', "\\|");
        let mut out = format!("| {} |\n", self.columns.iter().cloned().map(escape).collect::<Vec<_>>().join(" | "));
        let numeric: Vec<bool> = (0..self.columns.len()).map(|c| self.rows.iter().all(|r| matches!(r.get(c), Some(TableCell::Number(_))))).collect();
        out.push_str(&format!("| {} |\n", numeric.iter().map(|n| if *n { "---:" } else { "---" }).collect::<Vec<_>>().join(" | ")));
        for row in &self.rows {
            out.push_str(&format!("| {} |\n", row.iter().map(|c| escape(Self::cell_text(c))).collect::<Vec<_>>().join(" | ")));
        }
        out
    }

    pub fn csv(&self) -> String {
        let field = |s: String| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s };
        let mut out = self.columns.iter().cloned().map(field).collect::<Vec<_>>().join(",");
        out.push('\n');
        for row in &self.rows {
            out.push_str(&row.iter().map(|c| field(Self::cell_text(c))).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
        out
    }
}

fn text(s: &str) -> TableCell {
    TableCell::Text(s.to_string())
}

fn time_text(point: &TimePoint) -> TableCell {
    match point {
        TimePoint::Number(n) => TableCell::Number(*n),
        TimePoint::Tick(t) => text(t),
    }
}

/// The values behind `viz` as a table, for every type that has values (a mockup has none).
pub(super) fn data_table(viz: &Visualization) -> Option<DataTable> {
    use VisualizationContent as V;
    let (columns, rows): (Vec<String>, Vec<Vec<TableCell>>) = match &viz.content {
        V::Bar { bars, x_label, y_label } => (
            vec![y_label.clone().unwrap_or_else(|| "Label".into()), x_label.clone().unwrap_or_else(|| "Value".into())],
            bars.iter().map(|b| vec![text(&b.label), TableCell::Number(b.value)]).collect(),
        ),
        V::Line { x_labels, series, .. } => (
            std::iter::once(String::new()).chain(series.iter().map(|s| s.label.clone())).collect(),
            x_labels.iter().enumerate().map(|(i, x)| std::iter::once(text(x)).chain(series.iter().map(|s| TableCell::Number(s.values[i]))).collect()).collect(),
        ),
        V::Donut { parts, .. } => {
            let total: f64 = parts.iter().map(|p| p.value).sum();
            (
                vec!["Part".into(), "Value".into(), "Share".into()],
                parts.iter().map(|p| vec![text(&p.label), TableCell::Number(p.value), text(&format!("{:.1}%", p.value / total * 100.))]).collect(),
            )
        }
        V::Stats { tiles } => (
            vec!["Figure".into(), "Value".into(), "Change".into()],
            tiles.iter().map(|t| vec![text(&t.label), text(&t.value), text(t.delta.as_deref().unwrap_or(""))]).collect(),
        ),
        V::Table { columns, rows } => (columns.clone(), rows.iter().map(|r| r.cells().to_vec()).collect()),
        V::Heatmap { x_labels, y_labels, values } => (
            std::iter::once(String::new()).chain(x_labels.iter().cloned()).collect(),
            y_labels.iter().zip(values).map(|(y, row)| std::iter::once(text(y)).chain(row.iter().map(|v| TableCell::Number(*v))).collect()).collect(),
        ),
        V::Treemap { items } => {
            let total: f64 = items.iter().map(|i| i.weight).sum();
            (
                vec!["Item".into(), "Weight".into(), "Share".into()],
                items.iter().map(|i| vec![text(&i.label), TableCell::Number(i.weight), text(&format!("{:.1}%", i.weight / total * 100.))]).collect(),
            )
        }
        V::Timeline { events, .. } => {
            let lanes = events.iter().any(|e| e.lane.is_some());
            let mut columns = vec!["Event".to_string()];
            if lanes {
                columns.push("Lane".into());
            }
            columns.extend(["Start".into(), "End".into()]);
            (
                columns,
                events
                    .iter()
                    .map(|e| {
                        let mut row = vec![text(&e.label)];
                        if lanes {
                            row.push(text(e.lane.as_deref().unwrap_or("")));
                        }
                        row.push(time_text(&e.start));
                        row.push(e.end.as_ref().map(time_text).unwrap_or_else(|| text("—")));
                        row
                    })
                    .collect(),
            )
        }
        V::Flow { nodes, edges } => {
            let label = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| n.label.clone()).unwrap_or_default();
            let grouped = nodes.iter().any(|n| n.group.is_some());
            let mut columns = vec!["Step".to_string()];
            if grouped {
                columns.push("Group".into());
            }
            columns.extend(["Leads to".into(), "Detail".into()]);
            (
                columns,
                nodes
                    .iter()
                    .map(|n| {
                        let next = edges
                            .iter()
                            .filter(|e| e.from == n.id)
                            .map(|e| match &e.label {
                                Some(l) => format!("{} ({l})", label(&e.to)),
                                None => label(&e.to),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        let mut row = vec![text(&n.label)];
                        if grouped {
                            row.push(text(n.group.as_deref().unwrap_or("")));
                        }
                        row.push(text(&next));
                        row.push(text(n.detail.as_deref().unwrap_or("")));
                        row
                    })
                    .collect(),
            )
        }
        V::Layers { layers } => (
            vec!["Layer".into(), "Detail".into(), "Contains".into()],
            layers.iter().map(|l| vec![text(&l.label), text(l.detail.as_deref().unwrap_or("")), text(&l.items.join(", "))]).collect(),
        ),
        V::Mockup { .. } => return None,
    };
    Some(DataTable { columns, rows })
}

/// A text equivalent for VoiceOver and transcript selection. Visual marks always carry visible
/// labels too; this adds their values so nothing is communicated through colour alone.
fn accessible_description(viz: &Visualization) -> String {
    let mut out = format!("Visualization: {}. {}", viz.title, viz.summary);
    let mut push = |text: String| {
        if out.chars().count() < 4_096 {
            out.push_str("; ");
            out.push_str(&text);
        }
    };
    match &viz.content {
        VisualizationContent::Mockup { nodes } => {
            fn visit(nodes: &[MockNode], out: &mut Vec<String>) {
                for node in nodes {
                    if let Some(device) = node.device {
                        out.push(format!("{device:?} frame").to_lowercase());
                    }
                    if let Some(text) = &node.text {
                        out.push(text.clone());
                    }
                    if let Some(value) = &node.value {
                        out.push(value.clone());
                    }
                    visit(&node.children, out);
                }
            }
            let mut words = Vec::new();
            visit(nodes, &mut words);
            for word in words {
                push(word);
            }
        }
        VisualizationContent::Flow { edges, nodes } => {
            let label = |id: &str| nodes.iter().find(|n| n.id == id).map(|n| n.label.as_str()).unwrap_or("");
            for node in nodes {
                push(match (&node.group, &node.detail) {
                    (Some(g), Some(d)) => format!("{} ({g}): {d}", node.label),
                    (Some(g), None) => format!("{} ({g})", node.label),
                    (None, Some(d)) => format!("{}: {d}", node.label),
                    (None, None) => node.label.clone(),
                });
            }
            for edge in edges {
                push(match &edge.label {
                    Some(l) => format!("{} to {} ({l})", label(&edge.from), label(&edge.to)),
                    None => format!("{} to {}", label(&edge.from), label(&edge.to)),
                });
            }
        }
        _ => {
            if let Some(data) = data_table(viz) {
                for row in &data.rows {
                    let cells: Vec<String> = row
                        .iter()
                        .zip(&data.columns)
                        .map(|(cell, column)| {
                            let value = match cell {
                                TableCell::Number(n) => format_number(*n),
                                TableCell::Text(t) => t.clone(),
                            };
                            if column.is_empty() { value } else { format!("{column} {value}") }
                        })
                        .collect();
                    push(cells.join(", "));
                }
            }
        }
    }
    out.chars().take(4_096).collect()
}

#[cfg(test)]
mod tests {
    use super::{Visualization, TableCell, accessible_description, data_table, fence_closed, format_exact, format_number, format_tick, ident, with_unit};

    #[test]
    fn a_block_keeps_its_identity_while_it_arrives() {
        // Open while the agent writes it; closed by a fence line of its own.
        assert!(!fence_closed("```trek-viz"));
        assert!(!fence_closed("```trek-viz\n{\"a\":1}"));
        assert!(!fence_closed("```trek-viz\n{\"a\":1}\n``"));
        assert!(fence_closed("```trek-viz\n{\"a\":1}\n```"));
        assert!(fence_closed("```trek-viz\n{\"a\":1}\n````\n"));
        // The entrance is keyed on type and title, written before any mark: the first marks
        // drawn and the finished chart are the same visualization, so it plays once.
        let full = trek_agents::mock::VIZ_GALLERY.iter().find(|(k, _)| *k == "bar").unwrap().1;
        let json = &full[full.find('{').unwrap()..full.rfind('}').unwrap() + 1];
        let early = trek_core::visualization::parse_partial(&json[..json.find("trek-agents").unwrap()]).drawable.expect("two bars drawn");
        assert_eq!(ident(&early), ident(&trek_core::visualization::parse(json).unwrap()));
    }

    #[test]
    fn values_read_compactly() {
        assert_eq!(format_number(85_000.), "85K");
        assert_eq!(format_number(12_500.), "12.5K");
        assert_eq!(format_number(24_000_000.), "24M");
        assert_eq!(format_number(5_000_000_000.), "5B");
        assert_eq!(format_number(1_000_000_000_000.), "1T");
        assert_eq!(format_number(-18.), "-18");
        assert_eq!(format_number(3.25), "3.25");
        assert_eq!(format_number(1284.), "1,284");
        assert_eq!(format_number(-1284.5), "-1,284.5");
        assert_eq!(format_exact(1_284_000.), "1,284,000");
        assert_eq!(format_exact(-0.25), "-0.25");
        assert_eq!(format_tick(0.5, 0.25), "0.50");
        assert_eq!(format_tick(2000., 500.), "2,000");
        assert_eq!(with_unit("12".into(), Some("ms")), "12 ms");
        assert_eq!(with_unit("-4".into(), Some("$")), "-$4");
        assert_eq!(with_unit("40".into(), Some("%")), "40%");
    }

    fn viz(json: &str) -> Visualization {
        trek_core::visualization::parse(json).unwrap()
    }

    #[test]
    fn every_chart_has_a_table_and_a_complete_description() {
        let line = viz(r#"{"version":1,"title":"Latency","summary":"p95 falls","type":"line","x_labels":["Mon","Tue"],"series":[{"label":"p95","values":[120,80]}],"unit":"ms"}"#);
        let data = data_table(&line).unwrap();
        assert_eq!(data.markdown(), "|  | p95 |\n| --- | ---: |\n| Mon | 120 |\n| Tue | 80 |\n");
        assert_eq!(data.csv(), ",p95\nMon,120\nTue,80\n");
        assert!(accessible_description(&line).contains("Mon, p95 120; Tue, p95 80"));

        let flow = viz(r#"{"version":1,"title":"Send","summary":"s","type":"flow","nodes":[{"id":"a","label":"Composer"},{"id":"b","label":"Agent","detail":"runs the turn"}],"edges":[{"from":"a","to":"b","label":"prompt"}]}"#);
        let described = accessible_description(&flow);
        assert!(described.contains("Agent: runs the turn") && described.contains("Composer to Agent (prompt)"), "{described}");
        assert_eq!(data_table(&flow).unwrap().rows[0][1], TableCell::Text("Agent (prompt)".into()));

        let table = viz(r#"{"version":1,"title":"T","summary":"s","type":"table","columns":["a|b","n"],"rows":[["x, \"y\"",1.5]]}"#);
        let data = data_table(&table).unwrap();
        assert!(data.markdown().contains("a\\|b"));
        assert!(data.csv().contains("\"x, \"\"y\"\"\",1.5"));

        let mock = viz(r#"{"version":1,"title":"M","summary":"s","type":"mockup","nodes":[{"kind":"frame","device":"phone","children":[{"kind":"button","text":"Deploy"}]}]}"#);
        assert!(data_table(&mock).is_none());
        assert!(accessible_description(&mock).contains("phone frame; Deploy"));
    }
}
