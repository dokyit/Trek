//! Safe, non-executable visualizations embedded in agent markdown.
//!
//! A visualization is JSON inside a `trek-viz` fenced block. The schema deliberately has no
//! escape hatch for HTML, CSS, URLs, file paths, scripts, or event handlers: renderers choose all
//! presentation from these semantic values.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const VERSION: u8 = 1;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024;
pub const MAX_TEXT_CHARS: usize = 256;
pub const MAX_MARKS: usize = 128;
pub const MAX_MOCK_DEPTH: usize = 6;
pub const MAX_ABS_VALUE: f64 = 1_000_000_000_000.0;
/// Short strings a renderer sets in tight places: stat values and deltas, units, ids, layer
/// items, timeline ticks.
pub const MAX_SHORT_CHARS: usize = 48;
/// Lines on one chart: past six, hues stop being told apart (fold the tail into "Other").
pub const MAX_SERIES: usize = 6;
pub const MAX_PARTS: usize = 12;
pub const MAX_TILES: usize = 12;
pub const MAX_TREND_POINTS: usize = 64;
pub const MAX_COLUMNS: usize = 12;
pub const MAX_ROWS: usize = 64;
pub const MAX_TABLE_CELLS: usize = 384;
pub const MAX_EVENTS: usize = 48;
pub const MAX_FLOW_NODES: usize = 24;
pub const MAX_FLOW_EDGES: usize = 48;
pub const MAX_FLOW_GROUPS: usize = 6;
pub const MAX_LAYERS: usize = 8;
pub const MAX_LAYER_ITEMS: usize = 8;
pub const MAX_TABS: usize = 8;

/// Every `type` the schema knows, in the order the instructions list them.
pub const TYPES: &[&str] = &["bar", "line", "donut", "stats", "table", "heatmap", "treemap", "timeline", "flow", "layers", "mockup"];

/// Instructions suitable for including verbatim in an agent capability prompt. Every session
/// carries them, so they stay terse (`instructions_stay_compact`).
pub const AGENT_INSTRUCTIONS: &str = "Only when a picture clarifies the answer, add a native visualization: strict JSON in a fenced `trek-viz` block, then state its takeaway in prose. Every object: `version`:1, `title`, `summary`, `type`, plus its fields: \
bar `bars`[{label,value}], `x_label`?, `y_label`?; \
line `x_labels`, `series`[{label,values}] (≤6, one value per x label), `area`?:bool, `y_label`?, `unit`?; \
donut `parts`[{label,value>0}], `unit`?; \
stats `tiles`[{label,value:string,delta?:string,trend?:[numbers]}]; \
table `columns`, `rows` of [string|number] or {cells,tone}; \
heatmap `x_labels`, `y_labels`, `values` (one row per y label); \
treemap `items`[{label,weight>0}]; \
timeline `events`[{label,start,end?,lane?}], positions numbers, or strings from `ticks`; \
flow `nodes`[{id,label,detail?,group?}], `edges`[{from,to,label?}]; \
layers `layers`[{label,detail?,items?:[string]}], top first; \
mockup `nodes` tree of {kind,text?,value?,children?}: containers row, column, card, list, nav, tabs (text children; value = active), frame with `device` phone|browser|window|tablet; leaves with `text`: text, button, badge, metric (+value), input (value?), toggle (value on|off), avatar; image (placeholder; value wide|square|tall), progress (value 0-100), divider. \
Marks may set `tone`: neutral, accent, positive, warning, negative, info. Never emit HTML, CSS, URLs, paths or scripts. Limits: 32 KiB, text 256 chars (ids, units, stat values 48), 128 marks, mockup depth 6.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visualization {
    pub version: u8,
    pub title: String,
    pub summary: String,
    #[serde(flatten)]
    pub content: VisualizationContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VisualizationContent {
    Bar {
        bars: Vec<BarMark>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_label: Option<String>,
    },
    Heatmap {
        x_labels: Vec<String>,
        y_labels: Vec<String>,
        values: Vec<Vec<f64>>,
    },
    Treemap {
        items: Vec<TreemapItem>,
    },
    Mockup {
        nodes: Vec<MockNode>,
    },
    /// Values over an ordered x axis, one line per series.
    Line {
        x_labels: Vec<String>,
        series: Vec<LineSeries>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        area: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Part-to-whole at a glance.
    Donut {
        parts: Vec<DonutPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Headline figures: when the number is the chart.
    Stats {
        tiles: Vec<StatTile>,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<TableRow>,
    },
    /// Plans, phases, a gantt: spans and milestones along one axis.
    Timeline {
        events: Vec<TimelineEvent>,
        /// Ordinal axis labels; string positions name one of them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ticks: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// A node-and-edge diagram, laid out by the renderer.
    Flow {
        nodes: Vec<FlowNode>,
        #[serde(default)]
        edges: Vec<FlowEdge>,
    },
    /// An exploded stack of planes, top first: architecture tiers, z-order, design layers.
    Layers {
        layers: Vec<Layer>,
    },
}

impl VisualizationContent {
    /// The wire name of this variant's `type`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bar { .. } => "bar",
            Self::Heatmap { .. } => "heatmap",
            Self::Treemap { .. } => "treemap",
            Self::Mockup { .. } => "mockup",
            Self::Line { .. } => "line",
            Self::Donut { .. } => "donut",
            Self::Stats { .. } => "stats",
            Self::Table { .. } => "table",
            Self::Timeline { .. } => "timeline",
            Self::Flow { .. } => "flow",
            Self::Layers { .. } => "layers",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarMark {
    pub label: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreemapItem {
    pub label: String,
    pub weight: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineSeries {
    pub label: String,
    pub values: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DonutPart {
    pub label: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatTile {
    pub label: String,
    /// Already formatted by the agent ("1,284", "99.94%", "$4.2M").
    pub value: String,
    /// Signed change against a named period ("+12% vs last week").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    /// A sparkline of recent values, oldest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TableCell {
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TonedRow {
    pub cells: Vec<TableCell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

/// A table row: its cells, or its cells with a tone that tints the whole row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TableRow {
    Cells(Vec<TableCell>),
    Toned(TonedRow),
}

impl TableRow {
    pub fn cells(&self) -> &[TableCell] {
        match self {
            Self::Cells(cells) => cells,
            Self::Toned(row) => &row.cells,
        }
    }

    pub fn tone(&self) -> Option<SemanticTone> {
        match self {
            Self::Cells(_) => None,
            Self::Toned(row) => row.tone,
        }
    }
}

/// A position on a timeline: a number on a numeric axis, or one of the `ticks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimePoint {
    Number(f64),
    Tick(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEvent {
    pub label: String,
    pub start: TimePoint,
    /// None: a milestone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TimePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MockNode {
    pub kind: MockNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
    /// Only on a `frame`: the chrome drawn around it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<MockDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MockNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTone {
    Neutral,
    Accent,
    Positive,
    Warning,
    Negative,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockNodeKind {
    Row,
    Column,
    Card,
    Text,
    Metric,
    Button,
    Badge,
    Divider,
    Input,
    Toggle,
    Progress,
    Avatar,
    Image,
    List,
    Tabs,
    Nav,
    Frame,
}

impl MockNodeKind {
    /// Kinds that hold children.
    pub fn is_container(self) -> bool {
        matches!(self, Self::Row | Self::Column | Self::Card | Self::List | Self::Tabs | Self::Nav | Self::Frame)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockDevice {
    Phone,
    Browser,
    Window,
    Tablet,
}

/// A progress node's `value` ("64", "64%") as a fraction, when it is one.
pub fn progress_fraction(value: &str) -> Option<f64> {
    let n: f64 = value.trim().trim_end_matches('%').trim().parse().ok()?;
    (n.is_finite() && (0.0..=100.0).contains(&n)).then_some(n / 100.0)
}

/// Placeholder shapes an `image` node may take.
pub const IMAGE_SHAPES: &[&str] = &["wide", "square", "tall"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisualizationError {
    PayloadTooLarge { bytes: usize, max: usize },
    InvalidFence,
    InvalidJson(String),
    UnsupportedVersion(u8),
    Invalid(String),
}

impl fmt::Display for VisualizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { bytes, max } => write!(
                f,
                "visualization payload is {bytes} bytes; maximum is {max}"
            ),
            Self::InvalidFence => write!(f, "expected exactly one fenced `trek-viz` block"),
            Self::InvalidJson(error) => write!(f, "invalid visualization JSON: {error}"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "unsupported visualization schema version {version}; expected {VERSION}"
            ),
            Self::Invalid(error) => write!(f, "invalid visualization: {error}"),
        }
    }
}

impl std::error::Error for VisualizationError {}

impl VisualizationError {
    /// What went wrong, without the "invalid visualization" preamble: for a reader who's
    /// already been told that much ("Couldn't draw this visualization: …").
    pub fn reason(&self) -> String {
        match self {
            Self::PayloadTooLarge { bytes, max } => format!(
                "it's {:.1} KiB, over the {} KiB limit",
                *bytes as f64 / 1024.0,
                max / 1024
            ),
            Self::InvalidFence => "it isn't one ```trek-viz block with JSON inside".into(),
            Self::InvalidJson(error) | Self::Invalid(error) => error.clone(),
            Self::UnsupportedVersion(version) => {
                format!("schema version {version} isn't supported (expected {VERSION})")
            }
        }
    }
}

/// What can be read from a `trek-viz` block still being written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partial {
    /// `type` and `title`, once each has been written in full.
    pub kind: Option<String>,
    pub title: Option<String>,
    /// The visualization as far as it's written, in whole marks: the text cut back to the last
    /// complete element of an open array and closed, when that validates.
    pub drawable: Option<Visualization>,
    /// The top-level object has closed: only the fence is missing.
    pub complete: bool,
}

/// One open object or array while scanning partial JSON.
struct Open {
    array: bool,
    /// Just past its last complete member (just past the bracket while it has none).
    good: usize,
    /// An object is past a key's `:` and so reading a value.
    value: bool,
}

/// Read what can be drawn from `payload`, a `trek-viz` block's JSON as far as it's arrived.
///
/// One linear scan finds the type and title and where each open array last ended an element;
/// then the text is cut back to one of those and closed, innermost array first, until it
/// validates (a few tries at most, each one ordinary `parse`). Marks still being written are
/// left out rather than guessed at, so a value never shows as `12` on its way to `128`.
pub fn parse_partial(payload: &str) -> Partial {
    let bytes = payload.as_bytes();
    let mut partial = Partial::default();
    let mut stack: Vec<Open> = Vec::new();
    let (mut in_string, mut escaped, mut string_start) = (false, false, 0);
    // The top-level key whose value is being read.
    let mut key: Option<(usize, usize)> = None;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
                let Some(open) = stack.last_mut() else { break };
                let is_value = open.array || open.value;
                if is_value {
                    open.good = i + 1;
                }
                if stack.len() == 1 {
                    if !is_value {
                        key = Some((string_start, i + 1));
                    } else if let Some((ks, ke)) = key.take() {
                        let text = || serde_json::from_str::<String>(&payload[string_start..=i]).ok();
                        match &payload[ks..ke] {
                            "\"type\"" => partial.kind = text(),
                            "\"title\"" => partial.title = text(),
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }
        match b {
            b'"' if !stack.is_empty() => {
                in_string = true;
                string_start = i;
            }
            b'{' | b'[' => {
                if stack.len() == 1 {
                    key = None;
                }
                stack.push(Open { array: b == b'[', good: i + 1, value: false });
            }
            b'}' | b']' if !stack.is_empty() => {
                stack.pop();
                match stack.last_mut() {
                    Some(parent) => parent.good = i + 1,
                    None => {
                        end = Some(i + 1);
                        break;
                    }
                }
            }
            b':' => {
                if let Some(open) = stack.last_mut() {
                    open.value = true;
                }
            }
            b',' => {
                if let Some(open) = stack.last_mut() {
                    open.good = i;
                    open.value = false;
                }
                if stack.len() == 1 {
                    key = None;
                }
            }
            _ if stack.is_empty() && !b.is_ascii_whitespace() => break,
            _ => {}
        }
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return partial;
    }
    if let Some(end) = end {
        partial.complete = true;
        partial.drawable = parse(&payload[..end]).ok();
        return partial;
    }
    // Where to cut: each open array, innermost first, then the top-level object itself.
    let cuts = (0..stack.len()).rev().filter(|&k| k == 0 || stack[k].array).take(4);
    for k in cuts {
        let mut text = payload[..stack[k].good].to_string();
        for open in stack[..=k].iter().rev() {
            text.push(if open.array { ']' } else { '}' });
        }
        if let Ok(visualization) = parse(&text) {
            partial.drawable = Some(visualization);
            break;
        }
    }
    partial
}

/// Parse and validate a visualization JSON payload (without its markdown fence).
pub fn parse(payload: &str) -> Result<Visualization, VisualizationError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(VisualizationError::PayloadTooLarge {
            bytes: payload.len(),
            max: MAX_PAYLOAD_BYTES,
        });
    }
    reject_unknown_top_level_fields(payload)?;
    let visualization: Visualization = serde_json::from_str(payload)
        .map_err(|error| VisualizationError::InvalidJson(error.to_string()))?;
    validate(&visualization)?;
    Ok(visualization)
}

/// Parse a string containing exactly one `trek-viz` fenced block.
pub fn parse_fenced(block: &str) -> Result<Visualization, VisualizationError> {
    let trimmed = block.trim();
    let info = trimmed
        .strip_prefix("```trek-viz")
        .ok_or(VisualizationError::InvalidFence)?
        .trim_start_matches([' ', '\t']);
    let payload = info
        .strip_prefix('\n')
        .or_else(|| info.strip_prefix("\r\n"))
        .ok_or(VisualizationError::InvalidFence)?
        .strip_suffix("```")
        .ok_or(VisualizationError::InvalidFence)?
        .trim_end_matches(['\r', '\n']);
    if payload.contains("```") {
        return Err(VisualizationError::InvalidFence);
    }
    parse(payload)
}

/// Validate a programmatically constructed visualization.
pub fn validate(visualization: &Visualization) -> Result<(), VisualizationError> {
    if visualization.version != VERSION {
        return Err(VisualizationError::UnsupportedVersion(
            visualization.version,
        ));
    }
    text("title", &visualization.title, true)?;
    text("summary", &visualization.summary, true)?;

    match &visualization.content {
        VisualizationContent::Bar {
            bars,
            x_label,
            y_label,
        } => {
            dataset("bar", bars.len())?;
            optional_text("x_label", x_label)?;
            optional_text("y_label", y_label)?;
            for (index, bar) in bars.iter().enumerate() {
                text(&format!("bars[{index}].label"), &bar.label, true)?;
                number(&format!("bars[{index}].value"), bar.value)?;
            }
        }
        VisualizationContent::Heatmap {
            x_labels,
            y_labels,
            values,
        } => {
            if x_labels.is_empty() || y_labels.is_empty() {
                return invalid("heatmap axes must be nonempty");
            }
            if x_labels.len().saturating_mul(y_labels.len()) > MAX_MARKS {
                return invalid(format!("heatmap has more than {MAX_MARKS} cells"));
            }
            if values.len() != y_labels.len()
                || values.iter().any(|row| row.len() != x_labels.len())
            {
                return invalid(
                    "heatmap values must have one row per y label and one value per x label",
                );
            }
            for (axis, labels) in [("x_labels", x_labels), ("y_labels", y_labels)] {
                for (index, label) in labels.iter().enumerate() {
                    text(&format!("{axis}[{index}]"), label, true)?;
                }
            }
            for (row, values) in values.iter().enumerate() {
                for (column, value) in values.iter().enumerate() {
                    number(&format!("values[{row}][{column}]"), *value)?;
                }
            }
        }
        VisualizationContent::Treemap { items } => {
            dataset("treemap", items.len())?;
            for (index, item) in items.iter().enumerate() {
                text(&format!("items[{index}].label"), &item.label, true)?;
                number(&format!("items[{index}].weight"), item.weight)?;
                if item.weight <= 0.0 {
                    return invalid(format!("items[{index}].weight must be positive"));
                }
            }
        }
        VisualizationContent::Mockup { nodes } => {
            if nodes.is_empty() {
                return invalid("mockup nodes must be nonempty");
            }
            let mut count = 0;
            for (index, node) in nodes.iter().enumerate() {
                validate_node(node, 1, false, &mut count, &format!("nodes[{index}]"))?;
            }
        }
        VisualizationContent::Line {
            x_labels,
            series,
            y_label,
            unit,
            ..
        } => {
            if x_labels.len() < 2 {
                return invalid("line needs at least two x labels");
            }
            dataset("line x_labels", x_labels.len())?;
            labels("x_labels", x_labels, false)?;
            optional_text("y_label", y_label)?;
            short_opt("unit", unit)?;
            if series.is_empty() || series.len() > MAX_SERIES {
                return invalid(format!("line needs 1 to {MAX_SERIES} series"));
            }
            for (index, line) in series.iter().enumerate() {
                text(&format!("series[{index}].label"), &line.label, true)?;
                if line.values.len() != x_labels.len() {
                    return invalid(format!("series[{index}] must have one value per x label"));
                }
                for (point, value) in line.values.iter().enumerate() {
                    number(&format!("series[{index}].values[{point}]"), *value)?;
                }
            }
        }
        VisualizationContent::Donut { parts, unit } => {
            if parts.is_empty() || parts.len() > MAX_PARTS {
                return invalid(format!("donut needs 1 to {MAX_PARTS} parts"));
            }
            short_opt("unit", unit)?;
            for (index, part) in parts.iter().enumerate() {
                text(&format!("parts[{index}].label"), &part.label, true)?;
                number(&format!("parts[{index}].value"), part.value)?;
                if part.value <= 0.0 {
                    return invalid(format!("parts[{index}].value must be positive"));
                }
            }
        }
        VisualizationContent::Stats { tiles } => {
            if tiles.is_empty() || tiles.len() > MAX_TILES {
                return invalid(format!("stats needs 1 to {MAX_TILES} tiles"));
            }
            for (index, tile) in tiles.iter().enumerate() {
                text(&format!("tiles[{index}].label"), &tile.label, true)?;
                short(&format!("tiles[{index}].value"), &tile.value)?;
                short_opt(&format!("tiles[{index}].delta"), &tile.delta)?;
                if let Some(trend) = &tile.trend {
                    if trend.len() < 2 || trend.len() > MAX_TREND_POINTS {
                        return invalid(format!("tiles[{index}].trend needs 2 to {MAX_TREND_POINTS} values"));
                    }
                    for (point, value) in trend.iter().enumerate() {
                        number(&format!("tiles[{index}].trend[{point}]"), *value)?;
                    }
                }
            }
        }
        VisualizationContent::Table { columns, rows } => {
            if columns.is_empty() || columns.len() > MAX_COLUMNS {
                return invalid(format!("table needs 1 to {MAX_COLUMNS} columns"));
            }
            labels("columns", columns, false)?;
            if rows.is_empty() || rows.len() > MAX_ROWS {
                return invalid(format!("table needs 1 to {MAX_ROWS} rows"));
            }
            if rows.len() * columns.len() > MAX_TABLE_CELLS {
                return invalid(format!("table has more than {MAX_TABLE_CELLS} cells"));
            }
            for (index, row) in rows.iter().enumerate() {
                if row.cells().len() != columns.len() {
                    return invalid(format!("rows[{index}] must have one cell per column"));
                }
                for (column, cell) in row.cells().iter().enumerate() {
                    let field = format!("rows[{index}][{column}]");
                    match cell {
                        TableCell::Number(value) => number(&field, *value)?,
                        TableCell::Text(value) => text(&field, value, false)?,
                    }
                }
            }
        }
        VisualizationContent::Timeline { events, ticks, unit } => {
            if events.is_empty() || events.len() > MAX_EVENTS {
                return invalid(format!("timeline needs 1 to {MAX_EVENTS} events"));
            }
            short_opt("unit", unit)?;
            if let Some(ticks) = ticks {
                if ticks.is_empty() || ticks.len() > MAX_MARKS {
                    return invalid(format!("timeline ticks need 1 to {MAX_MARKS} labels"));
                }
                labels("ticks", ticks, true)?;
                if let Some(dup) = ticks.iter().enumerate().find(|(i, t)| ticks[..*i].contains(t)) {
                    return invalid(format!("ticks[{}] repeats `{}`", dup.0, dup.1));
                }
            }
            let mut lanes = Vec::new();
            for (index, event) in events.iter().enumerate() {
                text(&format!("events[{index}].label"), &event.label, true)?;
                if let Some(lane) = &event.lane {
                    short(&format!("events[{index}].lane"), lane)?;
                    if !lanes.contains(lane) {
                        lanes.push(lane.clone());
                    }
                }
                let start = time_point(&format!("events[{index}].start"), &event.start, ticks.as_deref())?;
                if let Some(end) = &event.end {
                    let end = time_point(&format!("events[{index}].end"), end, ticks.as_deref())?;
                    if end < start {
                        return invalid(format!("events[{index}] ends before it starts"));
                    }
                }
            }
            if lanes.len() > MAX_LAYERS {
                return invalid(format!("timeline has more than {MAX_LAYERS} lanes"));
            }
        }
        VisualizationContent::Flow { nodes, edges } => {
            if nodes.is_empty() || nodes.len() > MAX_FLOW_NODES {
                return invalid(format!("flow needs 1 to {MAX_FLOW_NODES} nodes"));
            }
            if edges.len() > MAX_FLOW_EDGES {
                return invalid(format!("flow has more than {MAX_FLOW_EDGES} edges"));
            }
            let mut groups = Vec::new();
            for (index, node) in nodes.iter().enumerate() {
                let path = format!("nodes[{index}]");
                identifier(&format!("{path}.id"), &node.id)?;
                if nodes[..index].iter().any(|other| other.id == node.id) {
                    return invalid(format!("{path}.id `{}` is not unique", node.id));
                }
                text(&format!("{path}.label"), &node.label, true)?;
                optional_text(&format!("{path}.detail"), &node.detail)?;
                if let Some(group) = &node.group {
                    short(&format!("{path}.group"), group)?;
                    if !groups.contains(group) {
                        groups.push(group.clone());
                    }
                }
            }
            if groups.len() > MAX_FLOW_GROUPS {
                return invalid(format!("flow has more than {MAX_FLOW_GROUPS} groups"));
            }
            for (index, edge) in edges.iter().enumerate() {
                for (end, id) in [("from", &edge.from), ("to", &edge.to)] {
                    if !nodes.iter().any(|node| &node.id == id) {
                        return invalid(format!("edges[{index}].{end} names no node `{id}`"));
                    }
                }
                if edge.from == edge.to {
                    return invalid(format!("edges[{index}] connects `{}` to itself", edge.from));
                }
                if edges[..index].iter().any(|other| other.from == edge.from && other.to == edge.to) {
                    return invalid(format!("edges[{index}] repeats `{}` -> `{}`", edge.from, edge.to));
                }
                short_opt(&format!("edges[{index}].label"), &edge.label)?;
            }
        }
        VisualizationContent::Layers { layers } => {
            if layers.is_empty() || layers.len() > MAX_LAYERS {
                return invalid(format!("layers needs 1 to {MAX_LAYERS} layers"));
            }
            for (index, layer) in layers.iter().enumerate() {
                text(&format!("layers[{index}].label"), &layer.label, true)?;
                optional_text(&format!("layers[{index}].detail"), &layer.detail)?;
                if layer.items.len() > MAX_LAYER_ITEMS {
                    return invalid(format!("layers[{index}] has more than {MAX_LAYER_ITEMS} items"));
                }
                labels(&format!("layers[{index}].items"), &layer.items, true)?;
            }
        }
    }
    Ok(())
}

fn reject_unknown_top_level_fields(payload: &str) -> Result<(), VisualizationError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| VisualizationError::InvalidJson(error.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| VisualizationError::InvalidJson("top level must be an object".into()))?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| VisualizationError::InvalidJson("missing or non-string `type`".into()))?;
    let variant = match kind {
        "bar" => &[
            "version", "title", "summary", "type", "bars", "x_label", "y_label",
        ][..],
        "heatmap" => &[
            "version", "title", "summary", "type", "x_labels", "y_labels", "values",
        ][..],
        "treemap" => &["version", "title", "summary", "type", "items"][..],
        "mockup" => &["version", "title", "summary", "type", "nodes"][..],
        "line" => &[
            "version", "title", "summary", "type", "x_labels", "series", "area", "y_label", "unit",
        ][..],
        "donut" => &["version", "title", "summary", "type", "parts", "unit"][..],
        "stats" => &["version", "title", "summary", "type", "tiles"][..],
        "table" => &["version", "title", "summary", "type", "columns", "rows"][..],
        "timeline" => &["version", "title", "summary", "type", "events", "ticks", "unit"][..],
        "flow" => &["version", "title", "summary", "type", "nodes", "edges"][..],
        "layers" => &["version", "title", "summary", "type", "layers"][..],
        other => {
            return Err(VisualizationError::InvalidJson(format!(
                "unsupported visualization type `{other}`"
            )));
        }
    };
    if let Some(field) = object
        .keys()
        .find(|field| !variant.contains(&field.as_str()))
    {
        // Name what the type does take, so the mistake reads at a glance.
        let takes = variant[4..].iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
        return Err(VisualizationError::InvalidJson(format!(
            "unknown field `{field}` (a {kind} takes {takes})"
        )));
    }
    Ok(())
}

fn validate_node(
    node: &MockNode,
    depth: usize,
    in_frame: bool,
    count: &mut usize,
    path: &str,
) -> Result<(), VisualizationError> {
    if depth > MAX_MOCK_DEPTH {
        return invalid(format!(
            "{path} exceeds maximum mockup depth {MAX_MOCK_DEPTH}"
        ));
    }
    *count += 1;
    if *count > MAX_MARKS {
        return invalid(format!("mockup has more than {MAX_MARKS} nodes"));
    }
    optional_text(&format!("{path}.text"), &node.text)?;
    optional_text(&format!("{path}.value"), &node.value)?;

    if !node.kind.is_container() && !node.children.is_empty() {
        return invalid(format!("{path} leaf node cannot have children"));
    }
    if node.device.is_some() != (node.kind == MockNodeKind::Frame) {
        return invalid(format!("{path} `device` belongs on a frame, and every frame needs one"));
    }
    let no_value = |what: &str| -> Result<(), VisualizationError> {
        if node.value.is_some() {
            return invalid(format!("{path} {what} cannot have a value"));
        }
        Ok(())
    };
    match node.kind {
        MockNodeKind::Row | MockNodeKind::Column => {
            if node.text.is_some() || node.value.is_some() {
                return invalid(format!("{path} row/column cannot have text or value"));
            }
        }
        MockNodeKind::Card | MockNodeKind::List | MockNodeKind::Nav => no_value("container")?,
        MockNodeKind::Frame => {
            no_value("frame")?;
            if in_frame {
                return invalid(format!("{path} frames cannot nest"));
            }
        }
        MockNodeKind::Tabs => {
            if node.text.is_some() {
                return invalid(format!("{path} tabs take their labels from text children"));
            }
            if node.children.is_empty() || node.children.len() > MAX_TABS {
                return invalid(format!("{path} tabs need 1 to {MAX_TABS} text children"));
            }
            if node.children.iter().any(|child| child.kind != MockNodeKind::Text) {
                return invalid(format!("{path} tabs children must be text nodes"));
            }
            if let Some(active) = &node.value
                && !node.children.iter().any(|child| child.text.as_ref() == Some(active))
            {
                return invalid(format!("{path}.value must name one of its tabs"));
            }
        }
        MockNodeKind::Text | MockNodeKind::Button | MockNodeKind::Badge => {
            required(&node.text, &format!("{path}.text"))?
        }
        MockNodeKind::Metric => {
            required(&node.text, &format!("{path}.text"))?;
            required(&node.value, &format!("{path}.value"))?;
        }
        MockNodeKind::Input => required(&node.text, &format!("{path}.text"))?,
        MockNodeKind::Toggle => {
            required(&node.text, &format!("{path}.text"))?;
            if node.value.as_deref().is_some_and(|v| v != "on" && v != "off") {
                return invalid(format!("{path}.value must be `on` or `off`"));
            }
        }
        MockNodeKind::Progress => {
            if node.value.as_deref().and_then(progress_fraction).is_none() {
                return invalid(format!("{path}.value must be a percentage from 0 to 100"));
            }
        }
        MockNodeKind::Avatar => {
            required(&node.text, &format!("{path}.text"))?;
            no_value("avatar")?;
        }
        MockNodeKind::Image => {
            // A placeholder only: its value picks a shape, never a source.
            if node.value.as_deref().is_some_and(|v| !IMAGE_SHAPES.contains(&v)) {
                return invalid(format!("{path}.value must be one of {}", IMAGE_SHAPES.join(", ")));
            }
        }
        MockNodeKind::Divider => {
            if node.text.is_some() || node.value.is_some() {
                return invalid(format!("{path} divider cannot have text or value"));
            }
        }
    }
    let in_frame = in_frame || node.kind == MockNodeKind::Frame;
    for (index, child) in node.children.iter().enumerate() {
        validate_node(
            child,
            depth + 1,
            in_frame,
            count,
            &format!("{path}.children[{index}]"),
        )?;
    }
    Ok(())
}

fn dataset(name: &str, len: usize) -> Result<(), VisualizationError> {
    if len == 0 {
        return invalid(format!("{name} dataset must be nonempty"));
    }
    if len > MAX_MARKS {
        return invalid(format!(
            "{name} dataset has {len} marks; maximum is {MAX_MARKS}"
        ));
    }
    Ok(())
}

fn text(field: &str, value: &str, required: bool) -> Result<(), VisualizationError> {
    let length = value.chars().count();
    if required && value.trim().is_empty() {
        return invalid(format!("{field} must be nonempty"));
    }
    if length > MAX_TEXT_CHARS {
        return invalid(format!(
            "{field} is {length} characters; maximum is {MAX_TEXT_CHARS}"
        ));
    }
    if value.chars().any(char::is_control) {
        return invalid(format!("{field} contains a control character"));
    }
    Ok(())
}

fn optional_text(field: &str, value: &Option<String>) -> Result<(), VisualizationError> {
    if let Some(value) = value {
        text(field, value, true)?;
    }
    Ok(())
}

/// A short required string: stat values, units, ids, lane and group names.
fn short(field: &str, value: &str) -> Result<(), VisualizationError> {
    text(field, value, true)?;
    let length = value.chars().count();
    if length > MAX_SHORT_CHARS {
        return invalid(format!(
            "{field} is {length} characters; maximum is {MAX_SHORT_CHARS}"
        ));
    }
    Ok(())
}

fn short_opt(field: &str, value: &Option<String>) -> Result<(), VisualizationError> {
    if let Some(value) = value {
        short(field, value)?;
    }
    Ok(())
}

/// Every label in `values` nonempty and bounded; `brief` ones to `MAX_SHORT_CHARS`.
fn labels(field: &str, values: &[String], brief: bool) -> Result<(), VisualizationError> {
    for (index, value) in values.iter().enumerate() {
        let field = format!("{field}[{index}]");
        if brief { short(&field, value)? } else { text(&field, value, true)? }
    }
    Ok(())
}

/// Flow node ids: short words a renderer never shows, so letters, digits, `-` and `_` only.
fn identifier(field: &str, value: &str) -> Result<(), VisualizationError> {
    short(field, value)?;
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return invalid(format!("{field} may use only letters, digits, `-` and `_`"));
    }
    Ok(())
}

/// A timeline position as a number on its axis: itself, or its tick's index. Numbers and
/// ticks don't mix, and a tick must be one of `ticks`.
fn time_point(field: &str, point: &TimePoint, ticks: Option<&[String]>) -> Result<f64, VisualizationError> {
    match (point, ticks) {
        (TimePoint::Number(value), None) => {
            number(field, *value)?;
            Ok(*value)
        }
        (TimePoint::Tick(tick), Some(ticks)) => match ticks.iter().position(|t| t == tick) {
            Some(index) => Ok(index as f64),
            None => invalid(format!("{field} `{tick}` is not one of the ticks")),
        },
        (TimePoint::Number(_), Some(_)) => invalid(format!("{field} must name a tick when `ticks` is set")),
        (TimePoint::Tick(_), None) => invalid(format!("{field} is a string, so the timeline needs `ticks`")),
    }
}

fn required(value: &Option<String>, field: &str) -> Result<(), VisualizationError> {
    if value.as_ref().is_none_or(|value| value.trim().is_empty()) {
        return invalid(format!("{field} is required"));
    }
    Ok(())
}

fn number(field: &str, value: f64) -> Result<(), VisualizationError> {
    if !value.is_finite() || value.abs() > MAX_ABS_VALUE {
        return invalid(format!(
            "{field} must be finite and between -{MAX_ABS_VALUE} and {MAX_ABS_VALUE}"
        ));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, VisualizationError> {
    Err(VisualizationError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn common(kind: &str, body: &str) -> String {
        format!(
            r#"{{"version":1,"title":"Demo","summary":"Useful summary","type":"{kind}",{body}}}"#
        )
    }

    #[test]
    fn parses_every_visualization_variant() {
        let bar = parse(&common(
            "bar",
            r#""bars":[{"label":"Build","value":42,"tone":"positive"}],"y_label":"seconds""#,
        ))
        .unwrap();
        assert!(matches!(bar.content, VisualizationContent::Bar { .. }));

        let heatmap = parse(&common(
            "heatmap",
            r#""x_labels":["Mon","Tue"],"y_labels":["API"],"values":[[2,4]]"#,
        ))
        .unwrap();
        assert!(matches!(
            heatmap.content,
            VisualizationContent::Heatmap { .. }
        ));

        let treemap = parse(&common(
            "treemap",
            r#""items":[{"label":"Core","weight":3},{"label":"UI","weight":2,"tone":"accent"}]"#,
        ))
        .unwrap();
        assert!(matches!(
            treemap.content,
            VisualizationContent::Treemap { .. }
        ));

        let mockup = parse_fenced(&format!("```trek-viz\n{}\n```", common("mockup", r#""nodes":[{"kind":"card","text":"Status","children":[{"kind":"metric","text":"Passing","value":"98%","tone":"positive"}]}]"#))).unwrap();
        assert!(matches!(
            mockup.content,
            VisualizationContent::Mockup { .. }
        ));
    }

    #[test]
    fn rejects_bad_shapes_values_and_fields() {
        assert!(
            parse(&common(
                "heatmap",
                r#""x_labels":["A","B"],"y_labels":["C"],"values":[[1]]"#
            ))
            .unwrap_err()
            .to_string()
            .contains("one value per x label")
        );
        assert!(
            parse(&common(
                "treemap",
                r#""items":[{"label":"none","weight":0}]"#
            ))
            .unwrap_err()
            .to_string()
            .contains("must be positive")
        );
        assert!(
            parse(&common(
                "bar",
                r#""bars":[{"label":"huge","value":1000000000001}]"#
            ))
            .is_err()
        );
        assert!(
            parse(&common(
                "bar",
                r#""bars":[{"label":"x","value":1}],"script":"alert(1)""#
            ))
            .unwrap_err()
            .to_string()
            .contains("unknown field `script`")
        );
        assert!(
            parse(&common(
                "bar",
                r##""bars":[{"label":"x","value":1,"color":"#fff"}]"##
            ))
            .unwrap_err()
            .to_string()
            .contains("unknown field `color`")
        );
        assert!(
            parse(&common("scatter", r#""items":[]"#))
                .unwrap_err()
                .to_string()
                .contains("unsupported visualization type")
        );
    }

    #[test]
    fn enforces_payload_text_count_and_depth_bounds() {
        let oversized = " ".repeat(MAX_PAYLOAD_BYTES + 1);
        assert!(matches!(
            parse(&oversized),
            Err(VisualizationError::PayloadTooLarge { .. })
        ));

        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        let payload = format!(
            r#"{{"version":1,"title":"{long}","summary":"s","type":"bar","bars":[{{"label":"x","value":1}}]}}"#
        );
        assert!(
            parse(&payload)
                .unwrap_err()
                .to_string()
                .contains("maximum is 256")
        );

        let bars = (0..=MAX_MARKS)
            .map(|i| format!(r#"{{"label":"{i}","value":1}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            parse(&common("bar", &format!(r#""bars":[{bars}]"#)))
                .unwrap_err()
                .to_string()
                .contains("maximum is 128")
        );

        let mut node = r#"{"kind":"text","text":"leaf"}"#.to_string();
        for _ in 0..MAX_MOCK_DEPTH {
            node = format!(r#"{{"kind":"card","children":[{node}]}}"#);
        }
        assert!(
            parse(&common("mockup", &format!(r#""nodes":[{node}]"#)))
                .unwrap_err()
                .to_string()
                .contains("maximum mockup depth")
        );
    }

    #[test]
    fn rejects_wrong_version_and_malformed_fences() {
        let version = common("bar", r#""bars":[{"label":"x","value":1}]"#).replacen(
            "\"version\":1",
            "\"version\":2",
            1,
        );
        assert_eq!(
            parse(&version),
            Err(VisualizationError::UnsupportedVersion(2))
        );
        assert_eq!(
            parse_fenced("```json\n{}\n```"),
            Err(VisualizationError::InvalidFence)
        );
        assert_eq!(
            parse_fenced("before\n```trek-viz\n{}\n```"),
            Err(VisualizationError::InvalidFence)
        );
        assert_eq!(
            parse_fenced("```trek-viz-extra\n{}\n```"),
            Err(VisualizationError::InvalidFence)
        );
        let spaced = common("bar", r#""bars":[{"label":"x","value":1}]"#);
        assert!(parse_fenced(&format!("```trek-viz \n{spaced}\n```")).is_ok());
    }

    fn err(kind: &str, body: &str) -> String {
        parse(&common(kind, body)).unwrap_err().to_string()
    }

    #[test]
    fn instructions_stay_compact_and_cover_every_type() {
        assert!(AGENT_INSTRUCTIONS.chars().count() < 2_000, "{} chars", AGENT_INSTRUCTIONS.chars().count());
        for kind in TYPES {
            assert!(AGENT_INSTRUCTIONS.contains(&format!("{kind} `")) || AGENT_INSTRUCTIONS.contains(&format!("{kind} ")), "{kind}");
        }
        for kind in ["input", "toggle", "progress", "avatar", "image", "list", "tabs", "nav", "frame", "phone", "browser", "window", "tablet"] {
            assert!(AGENT_INSTRUCTIONS.contains(kind), "{kind}");
        }
    }

    #[test]
    fn every_type_has_a_wire_name_and_known_fields() {
        let bodies = [
            ("bar", r#""bars":[{"label":"a","value":1}]"#),
            ("line", r#""x_labels":["a","b"],"series":[{"label":"s","values":[1,2]}]"#),
            ("donut", r#""parts":[{"label":"a","value":1}]"#),
            ("stats", r#""tiles":[{"label":"a","value":"1"}]"#),
            ("table", r#""columns":["a"],"rows":[["x"]]"#),
            ("heatmap", r#""x_labels":["a"],"y_labels":["b"],"values":[[1]]"#),
            ("treemap", r#""items":[{"label":"a","weight":1}]"#),
            ("timeline", r#""events":[{"label":"a","start":1}]"#),
            ("flow", r#""nodes":[{"id":"a","label":"A"}]"#),
            ("layers", r#""layers":[{"label":"a"}]"#),
            ("mockup", r#""nodes":[{"kind":"divider"}]"#),
        ];
        assert_eq!(bodies.len(), TYPES.len());
        for ((kind, body), listed) in bodies.iter().zip(TYPES) {
            assert_eq!(kind, listed);
            let viz = parse(&common(kind, body)).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert_eq!(viz.content.kind(), *kind);
            // What a renderer copies parses back to the same thing.
            assert_eq!(parse(&serde_json::to_string(&viz).unwrap()).unwrap(), viz);
            assert!(err(kind, &format!(r#"{body},"html":"<b>""#)).contains("unknown field `html`"), "{kind}");
        }
    }

    #[test]
    fn line_charts_line_up_with_their_axis() {
        let viz = parse(&common(
            "line",
            r#""x_labels":["Mon","Tue","Wed"],"series":[{"label":"p50","values":[1,2,3],"tone":"info"},{"label":"p95","values":[4,5,6]}],"area":true,"y_label":"latency","unit":"ms""#,
        ))
        .unwrap();
        let VisualizationContent::Line { series, area, unit, .. } = viz.content else { panic!() };
        assert_eq!((series.len(), area, unit.as_deref()), (2, true, Some("ms")));
        assert!(err("line", r#""x_labels":["a"],"series":[{"label":"s","values":[1]}]"#).contains("at least two x labels"));
        assert!(err("line", r#""x_labels":["a","b"],"series":[{"label":"s","values":[1]}]"#).contains("one value per x label"));
        assert!(err("line", r#""x_labels":["a","b"],"series":[]"#).contains("1 to 6 series"));
        let seven = (0..7).map(|i| format!(r#"{{"label":"{i}","values":[1,2]}}"#)).collect::<Vec<_>>().join(",");
        assert!(err("line", &format!(r#""x_labels":["a","b"],"series":[{seven}]"#)).contains("1 to 6 series"));
        assert!(err("line", r#""x_labels":["a","b"],"series":[{"label":"s","values":[1,2]}],"unit":"this unit is far too long to sit beside a number on an axis""#).contains("maximum is 48"));
        assert!(err("line", r#""x_labels":["a","b"],"series":[{"label":"s","values":[1,2],"color":"red"}]"#).contains("unknown field `color`"));
    }

    #[test]
    fn donuts_and_stats_stay_small() {
        assert!(parse(&common("donut", r#""parts":[{"label":"Rust","value":62,"tone":"accent"},{"label":"Other","value":38}],"unit":"%""#)).is_ok());
        assert!(err("donut", r#""parts":[{"label":"none","value":0}]"#).contains("must be positive"));
        assert!(err("donut", r#""parts":[]"#).contains("1 to 12 parts"));
        let many = (0..13).map(|i| format!(r#"{{"label":"{i}","value":1}}"#)).collect::<Vec<_>>().join(",");
        assert!(err("donut", &format!(r#""parts":[{many}]"#)).contains("1 to 12 parts"));

        let viz = parse(&common("stats", r#""tiles":[{"label":"Success","value":"99.94%","delta":"+0.2 pts","trend":[1,2,3],"tone":"positive"}]"#)).unwrap();
        let VisualizationContent::Stats { tiles } = viz.content else { panic!() };
        assert_eq!(tiles[0].trend.as_deref(), Some(&[1., 2., 3.][..]));
        assert!(err("stats", r#""tiles":[{"label":"a","value":"1","trend":[1]}]"#).contains("2 to 64 values"));
        assert!(err("stats", r#""tiles":[{"label":"a","value":1}]"#).contains("invalid"), "values are preformatted strings");
        assert!(err("stats", &format!(r#""tiles":[{{"label":"a","value":"{}"}}]"#, "9".repeat(49))).contains("maximum is 48"));
        assert!(err("stats", r#""tiles":[]"#).contains("1 to 12 tiles"));
    }

    #[test]
    fn tables_mix_text_and_numbers_in_rectangular_rows() {
        let viz = parse(&common("table", r#""columns":["Service","p95"],"rows":[["API",120.5],{"cells":["Workers",940],"tone":"warning"}]"#)).unwrap();
        let VisualizationContent::Table { rows, .. } = viz.content else { panic!() };
        assert_eq!(rows[0].cells(), &[TableCell::Text("API".into()), TableCell::Number(120.5)]);
        assert_eq!(rows[1].tone(), Some(SemanticTone::Warning));
        assert!(err("table", r#""columns":["a","b"],"rows":[["x"]]"#).contains("one cell per column"));
        assert!(err("table", r#""columns":["a"],"rows":[]"#).contains("1 to 64 rows"));
        assert!(err("table", r#""columns":["a"],"rows":[[true]]"#).contains("invalid"));
        assert!(err("table", r#""columns":["a"],"rows":[{"cells":["x"],"href":"y"}]"#).contains("invalid"));
        let columns = (0..12).map(|i| format!(r#""c{i}""#)).collect::<Vec<_>>().join(",");
        let row = format!("[{}]", (0..12).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
        let rows = vec![row; 33].join(",");
        assert!(err("table", &format!(r#""columns":[{columns}],"rows":[{rows}]"#)).contains("more than 384 cells"));
    }

    #[test]
    fn timelines_take_numbers_or_their_own_ticks() {
        assert!(parse(&common("timeline", r#""events":[{"label":"Design","start":0,"end":2,"lane":"UI"},{"label":"Ship","start":4}],"unit":"wk""#)).is_ok());
        assert!(parse(&common("timeline", r#""ticks":["Oct","Nov","Dec"],"events":[{"label":"Beta","start":"Oct","end":"Nov","tone":"info"},{"label":"GA","start":"Dec"}]"#)).is_ok());
        assert!(err("timeline", r#""events":[{"label":"x","start":3,"end":1}]"#).contains("ends before it starts"));
        assert!(err("timeline", r#""ticks":["Oct","Nov"],"events":[{"label":"x","start":"Nov","end":"Oct"}]"#).contains("ends before it starts"));
        assert!(err("timeline", r#""ticks":["Oct"],"events":[{"label":"x","start":"Jan"}]"#).contains("not one of the ticks"));
        assert!(err("timeline", r#""events":[{"label":"x","start":"Jan"}]"#).contains("needs `ticks`"));
        assert!(err("timeline", r#""ticks":["Oct"],"events":[{"label":"x","start":1}]"#).contains("must name a tick"));
        assert!(err("timeline", r#""ticks":["Oct","Oct"],"events":[{"label":"x","start":"Oct"}]"#).contains("repeats"));
        assert!(err("timeline", r#""events":[]"#).contains("1 to 48 events"));
        let lanes = (0..9).map(|i| format!(r#"{{"label":"e","start":1,"lane":"l{i}"}}"#)).collect::<Vec<_>>().join(",");
        assert!(err("timeline", &format!(r#""events":[{lanes}]"#)).contains("more than 8 lanes"));
    }

    #[test]
    fn flows_reference_only_their_own_nodes() {
        let viz = parse(&common("flow", r#""nodes":[{"id":"ui","label":"Composer","group":"App"},{"id":"core","label":"Workspace","detail":"routes turns","tone":"accent"}],"edges":[{"from":"ui","to":"core","label":"send"}]"#)).unwrap();
        let VisualizationContent::Flow { nodes, edges } = viz.content else { panic!() };
        assert_eq!((nodes.len(), edges.len()), (2, 1));
        assert!(parse(&common("flow", r#""nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"a"}]"#)).is_ok(), "cycles are drawn, not refused");
        assert!(err("flow", r#""nodes":[{"id":"a","label":"A"}],"edges":[{"from":"a","to":"ghost"}]"#).contains("names no node `ghost`"));
        assert!(err("flow", r#""nodes":[{"id":"a","label":"A"},{"id":"a","label":"B"}]"#).contains("not unique"));
        assert!(err("flow", r#""nodes":[{"id":"a b","label":"A"}]"#).contains("letters, digits"));
        assert!(err("flow", r#""nodes":[{"id":"a","label":"A"}],"edges":[{"from":"a","to":"a"}]"#).contains("to itself"));
        assert!(err("flow", r#""nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"},{"from":"a","to":"b"}]"#).contains("repeats"));
        assert!(err("flow", r#""nodes":[]"#).contains("1 to 24 nodes"));
        let nodes = (0..25).map(|i| format!(r#"{{"id":"n{i}","label":"N"}}"#)).collect::<Vec<_>>().join(",");
        assert!(err("flow", &format!(r#""nodes":[{nodes}]"#)).contains("1 to 24 nodes"));
        let nodes = (0..10).map(|i| format!(r#"{{"id":"n{i}","label":"N"}}"#)).collect::<Vec<_>>().join(",");
        let edges = (0..10).flat_map(|a| (0..10).filter(move |b| *b != a).map(move |b| format!(r#"{{"from":"n{a}","to":"n{b}"}}"#))).take(49).collect::<Vec<_>>().join(",");
        assert!(err("flow", &format!(r#""nodes":[{nodes}],"edges":[{edges}]"#)).contains("more than 48 edges"));
        let grouped = (0..7).map(|i| format!(r#"{{"id":"n{i}","label":"N","group":"g{i}"}}"#)).collect::<Vec<_>>().join(",");
        assert!(err("flow", &format!(r#""nodes":[{grouped}]"#)).contains("more than 6 groups"));
    }

    #[test]
    fn layers_are_a_short_stack_of_short_items() {
        assert!(parse(&common("layers", r#""layers":[{"label":"UI","detail":"GPUI views","items":["Sidebar","Transcript"],"tone":"accent"},{"label":"Core"}]"#)).is_ok());
        assert!(err("layers", r#""layers":[]"#).contains("1 to 8 layers"));
        let items = (0..9).map(|i| format!(r#""i{i}""#)).collect::<Vec<_>>().join(",");
        assert!(err("layers", &format!(r#""layers":[{{"label":"x","items":[{items}]}}]"#)).contains("more than 8 items"));
        assert!(err("layers", r#""layers":[{"label":"x","items":[""]}]"#).contains("nonempty"));
    }

    #[test]
    fn mockup_v2_kinds_and_device_frames() {
        let tree = r#""nodes":[{"kind":"frame","device":"phone","text":"Release","children":[
            {"kind":"nav","text":"Trek","children":[{"kind":"avatar","text":"Ada Lovelace"}]},
            {"kind":"tabs","value":"Health","children":[{"kind":"text","text":"Health"},{"kind":"text","text":"Deploys"}]},
            {"kind":"input","text":"Search services","value":"workers"},
            {"kind":"toggle","text":"Auto-rollback","value":"on"},
            {"kind":"progress","text":"Rollout","value":"64%","tone":"info"},
            {"kind":"image","text":"Latency chart","value":"wide"},
            {"kind":"list","children":[{"kind":"text","text":"API"},{"kind":"text","text":"Workers"}]}]}]"#;
        let viz = parse(&common("mockup", &tree.replace('\n', ""))).unwrap();
        let VisualizationContent::Mockup { nodes } = viz.content else { panic!() };
        assert_eq!(nodes[0].device, Some(MockDevice::Phone));
        for device in ["browser", "window", "tablet"] {
            assert!(parse(&common("mockup", &format!(r#""nodes":[{{"kind":"frame","device":"{device}"}}]"#))).is_ok());
        }
        let bad = [
            (r#"{"kind":"frame"}"#, "every frame needs one"),
            (r#"{"kind":"card","device":"phone"}"#, "belongs on a frame"),
            (r#"{"kind":"frame","device":"watch"}"#, "invalid"),
            (r#"{"kind":"frame","device":"phone","children":[{"kind":"frame","device":"browser"}]}"#, "cannot nest"),
            (r#"{"kind":"tabs","children":[{"kind":"button","text":"x"}]}"#, "must be text nodes"),
            (r#"{"kind":"tabs","value":"Z","children":[{"kind":"text","text":"A"}]}"#, "name one of its tabs"),
            (r#"{"kind":"tabs"}"#, "1 to 8 text children"),
            (r#"{"kind":"toggle","text":"x","value":"maybe"}"#, "`on` or `off`"),
            (r#"{"kind":"progress","value":"140%"}"#, "0 to 100"),
            (r#"{"kind":"progress"}"#, "0 to 100"),
            (r#"{"kind":"image","value":"https://example.com/a.png"}"#, "must be one of"),
            (r#"{"kind":"input"}"#, "text is required"),
            (r#"{"kind":"avatar","text":"A","value":"x"}"#, "cannot have a value"),
            (r#"{"kind":"input","text":"x","children":[{"kind":"divider"}]}"#, "cannot have children"),
            (r#"{"kind":"image","src":"x"}"#, "unknown field `src`"),
        ];
        for (node, message) in bad {
            let e = err("mockup", &format!(r#""nodes":[{node}]"#));
            assert!(e.contains(message), "{node}: {e}");
        }
        assert_eq!(progress_fraction(" 64 % "), Some(0.64));
    }

    #[test]
    fn a_block_still_arriving_draws_its_whole_marks() {
        let full = common("bar", r#""bars":[{"label":"a","value":12},{"label":"b","value":128},{"label":"c \"q\"","value":7}],"x_label":"s""#);
        let at = |needle: &str| &full[..full.find(needle).unwrap() + needle.len()];
        // Nothing yet but the type and title: something to frame, nothing to draw.
        let early = parse_partial(at(r#""type":"bar""#));
        assert_eq!((early.kind.as_deref(), early.title.as_deref(), early.drawable.is_none()), (Some("bar"), Some("Demo"), true));
        assert!(parse_partial(r#"{"version":1,"title":"De"#).title.is_none(), "only a title written in full");
        // A number still arriving is left out, not drawn as 12 on its way to 128.
        let mid = parse_partial(at(r#""value":12"#));
        assert!(mid.drawable.is_none(), "{mid:?}");
        let one = parse_partial(at(r#""value":12},{"label":"b","value":12"#));
        let VisualizationContent::Bar { bars, .. } = one.drawable.unwrap().content else { panic!() };
        assert_eq!(bars.iter().map(|b| b.value).collect::<Vec<_>>(), vec![12.0]);
        let two = parse_partial(at(r#"{"label":"c \"q"#));
        let VisualizationContent::Bar { bars, .. } = two.drawable.unwrap().content else { panic!() };
        assert_eq!(bars.len(), 2);
        // Closed, with the fence still to come; and every prefix reads without a panic.
        let done = parse_partial(&format!("{full}\n``"));
        assert!(done.complete && done.drawable == parse(&full).ok());
        for end in 0..=full.len() {
            let _ = parse_partial(&full[..end]);
        }
        // A row of a table only once it's whole; a line's series only once it has every value.
        // (`common` closes the object; a block still arriving hasn't.)
        let open = |kind: &str, body: &str| common(kind, body).strip_suffix('}').unwrap().to_string();
        let table = open("table", r#""columns":["a","b"],"rows":[["x",1],["y","#);
        let VisualizationContent::Table { rows, .. } = parse_partial(&table).drawable.unwrap().content else { panic!() };
        assert_eq!(rows.len(), 1);
        let line = open("line", r#""x_labels":["a","b"],"series":[{"label":"s","values":[1,2]},{"label":"t","values":[3"#);
        let VisualizationContent::Line { series, .. } = parse_partial(&line).drawable.unwrap().content else { panic!() };
        assert_eq!(series.len(), 1);
    }

    #[test]
    fn errors_read_plainly_after_the_notice() {
        let unknown = parse(&common("bar", r#""bars":[{"label":"x","value":1}],"html":"<b>""#)).unwrap_err();
        assert_eq!(unknown.reason(), "unknown field `html` (a bar takes `bars`, `x_label`, `y_label`)");
        let big = VisualizationError::PayloadTooLarge { bytes: 40 * 1024, max: MAX_PAYLOAD_BYTES };
        assert_eq!(big.reason(), "it's 40.0 KiB, over the 32 KiB limit");
        assert!(!VisualizationError::Invalid("bars dataset must be nonempty".into()).reason().contains("invalid visualization"));
    }
}
