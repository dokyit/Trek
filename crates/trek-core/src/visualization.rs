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

/// Instructions suitable for including verbatim in an agent capability prompt.
pub const AGENT_INSTRUCTIONS: &str = "Only when it materially clarifies the answer, add a native visualization as strict JSON in a fenced `trek-viz` block and explain its conclusion in prose. Every object has `version`:1, `title`, `summary`, and `type`. `bar` adds `bars` of {`label`,`value`,optional `tone`} and optional `x_label`,`y_label`; `heatmap` adds `x_labels`,`y_labels`,`values` (rectangular rows); `treemap` adds positive-weight `items` of {`label`,`weight`,optional `tone`}; `mockup` adds recursive `nodes` of {`kind`,optional `text`,`value`,`tone`,`children`}. `row`/`column` have only children, `card` may have text and children, `text`/`button`/`badge` require text, `metric` requires text and value, and `divider` needs neither; leaf kinds cannot have children. Tones: `neutral`,`accent`,`positive`,`warning`,`negative`,`info`. Use only these fields; never emit HTML, CSS, URLs, paths, scripts, or handlers. Limit JSON to 32 KiB, text to 256 characters, marks/nodes to 128, and mock depth to 6.";

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
pub struct MockNode {
    pub kind: MockNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<SemanticTone>,
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
}

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
                validate_node(node, 1, &mut count, &format!("nodes[{index}]"))?;
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
        return Err(VisualizationError::InvalidJson(format!(
            "unknown field `{field}`"
        )));
    }
    Ok(())
}

fn validate_node(
    node: &MockNode,
    depth: usize,
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

    let container = matches!(
        node.kind,
        MockNodeKind::Row | MockNodeKind::Column | MockNodeKind::Card
    );
    if !container && !node.children.is_empty() {
        return invalid(format!("{path} leaf node cannot have children"));
    }
    match node.kind {
        MockNodeKind::Row | MockNodeKind::Column => {
            if node.text.is_some() || node.value.is_some() {
                return invalid(format!("{path} row/column cannot have text or value"));
            }
        }
        MockNodeKind::Card => {
            if node.value.is_some() {
                return invalid(format!("{path} card cannot have a value"));
            }
        }
        MockNodeKind::Text | MockNodeKind::Button | MockNodeKind::Badge => {
            required(&node.text, &format!("{path}.text"))?
        }
        MockNodeKind::Metric => {
            required(&node.text, &format!("{path}.text"))?;
            required(&node.value, &format!("{path}.value"))?;
        }
        MockNodeKind::Divider => {
            if node.text.is_some() || node.value.is_some() {
                return invalid(format!("{path} divider cannot have text or value"));
            }
        }
    }
    for (index, child) in node.children.iter().enumerate() {
        validate_node(
            child,
            depth + 1,
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
            parse(&common("line", r#""items":[]"#))
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
}
