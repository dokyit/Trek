//! Native, non-executable visualizations embedded in an assistant answer.
//!
//! Agents emit a fenced `trek-viz` JSON block. The core crate owns and validates the bounded
//! wire format; this module only turns the parsed data into theme-aware GPUI elements. Invalid
//! or incomplete blocks are left to Markdown's ordinary code-block renderer.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::visualization::{
    MockNode, MockNodeKind, SemanticTone, Visualization, VisualizationContent,
};

const PLUGIN: &str = "trek-visualization";

#[derive(Clone)]
struct VisualizationNode {
    visualization: Visualization,
}

/// The assistant-answer Markdown extension. It claims only complete, valid `trek-viz` fences.
#[derive(Clone, Copy)]
pub struct VisualizationPlugin;

impl MarkdownPlugin for VisualizationPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        PLUGIN
    }

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
        let visualization = trek_core::visualization::parse_fenced(source).ok()?;
        let accessible = accessible_description(&visualization);
        Some(
            MarkdownNode::new(
                PLUGIN,
                VisualizationNode { visualization },
            )
            .text(accessible.clone())
            .markdown(source.to_string())
            .accessibility_label(accessible),
        )
    }

    fn render(&self, node: &MarkdownNode, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        match node.data::<VisualizationNode>() {
            Some(data) => {
                let key = node.source_range().map(|range| range.start).unwrap_or(0);
                artifact(&data.visualization, key, cx)
            }
            None => div().into_any_element(),
        }
    }
}

fn tone(tone: Option<SemanticTone>, cx: &App) -> Hsla {
    match tone.unwrap_or(SemanticTone::Accent) {
        SemanticTone::Neutral => cx.theme().muted_foreground,
        SemanticTone::Accent => crate::palette::ember(cx),
        SemanticTone::Positive => crate::palette::emerald(cx),
        SemanticTone::Warning => crate::palette::amber(cx),
        SemanticTone::Negative => crate::palette::red(cx),
        SemanticTone::Info => crate::palette::sky(cx),
    }
}

/// A text equivalent for VoiceOver and transcript selection. Visual marks always carry visible
/// labels too; this adds their values so a heatmap never communicates through color alone.
fn accessible_description(viz: &Visualization) -> String {
    let mut out = format!("Visualization: {}. {}", viz.title, viz.summary);
    let mut push = |text: String| {
        if out.chars().count() < 4_096 {
            out.push_str("; ");
            out.push_str(&text);
        }
    };
    match &viz.content {
        VisualizationContent::Bar { bars, .. } => {
            for bar in bars {
                push(format!("{} {}", bar.label, format_number(bar.value)));
            }
        }
        VisualizationContent::Heatmap {
            x_labels,
            y_labels,
            values,
        } => {
            for (row_ix, row) in values.iter().enumerate() {
                for (col_ix, value) in row.iter().enumerate() {
                    push(format!(
                        "{} {} {}",
                        y_labels.get(row_ix).map(String::as_str).unwrap_or(""),
                        x_labels.get(col_ix).map(String::as_str).unwrap_or(""),
                        format_number(*value)
                    ));
                }
            }
        }
        VisualizationContent::Treemap { items } => {
            for item in items {
                push(format!("{} {}", item.label, format_number(item.weight)));
            }
        }
        VisualizationContent::Mockup { nodes } => {
            fn visit(nodes: &[MockNode], out: &mut Vec<String>) {
                for node in nodes {
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
    }
    out.chars().take(4_096).collect()
}

fn artifact(viz: &Visualization, key: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let border = theme.foreground.opacity(0.10);
    let body = match &viz.content {
        VisualizationContent::Bar {
            bars,
            x_label,
            y_label,
        } => bars_view(bars, x_label.as_deref(), y_label.as_deref(), key, cx),
        VisualizationContent::Heatmap {
            x_labels,
            y_labels,
            values,
        } => heatmap_view(x_labels, y_labels, values, key, cx),
        VisualizationContent::Treemap { items } => treemap_view(items, key, cx),
        VisualizationContent::Mockup { nodes } => mockup_view(nodes, cx),
    };
    let source_for_copy = serde_json::to_string_pretty(viz).unwrap_or_default();
    v_flex()
        .id(SharedString::from(format!("trek-viz-{key}")))
        .test_support()
        .w_full()
        .my_2()
        .rounded(px(12.))
        .border_1()
        .border_color(border)
        .bg(theme.foreground.opacity(0.025))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .px_4()
                .pt_3()
                .pb_2()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_semibold()
                                .text_color(theme.foreground)
                                .child(viz.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .line_height(relative(1.45))
                                .text_color(muted)
                                .child(viz.summary.clone()),
                        ),
                )
                .child(
                    gpui_kit::component::button::Button::new(SharedString::from(format!("copy-viz-{key}")))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Copy).text_color(muted))
                        .tooltip("Copy visualization data")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                source_for_copy.clone(),
                            ));
                            gpui_kit::component::WindowExt::push_notification(
                                window,
                                "Visualization data copied",
                                cx,
                            );
                        }),
                ),
        )
        .child(div().w_full().h(px(1.)).bg(border))
        .child(div().w_full().max_h(px(460.)).overflow_y_scrollbar().id(("viz-scroll", key)).p_4().child(body))
        .into_any_element()
}

fn bars_view(
    bars: &[trek_core::visualization::BarMark],
    x_label: Option<&str>,
    y_label: Option<&str>,
    key: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let max = bars
        .iter()
        .map(|bar| bar.value.abs())
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let rows = bars.iter().enumerate().map(|(ix, bar)| {
        let width = (bar.value.abs() / max) as f32;
        let color = tone(bar.tone, cx);
        h_flex()
            .id(SharedString::from(format!("viz-bar-{key}-{ix}")))
            .test_support()
            .w_full()
            .gap_3()
            .child(
                div()
                    .w(px(112.))
                    .flex_none()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(muted)
                    .child(bar.label.clone()),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(px(10.))
                    .rounded_full()
                    .bg(theme.foreground.opacity(0.07))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(width.clamp(0., 1.)))
                            .rounded_full()
                            .bg(color.opacity(0.82)),
                    ),
            )
            .child(
                div()
                    .w(px(68.))
                    .flex_none()
                    .text_right()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(11.5))
                    .text_color(theme.foreground)
                    .child(format_number(bar.value)),
            )
    });
    v_flex()
        .w_full()
        .gap(px(9.))
        .when_some(y_label.map(str::to_string), |el, label| {
            el.child(div().text_xs().font_medium().text_color(muted).child(label))
        })
        .children(rows)
        .when_some(x_label.map(str::to_string), |el, label| {
            el.child(
                div()
                    .pt_1()
                    .text_center()
                    .text_xs()
                    .text_color(muted)
                    .child(label),
            )
        })
        .into_any_element()
}

fn heatmap_view(
    x_labels: &[String],
    y_labels: &[String],
    values: &[Vec<f64>],
    key: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let accent = crate::palette::sky(cx);
    let (min, max) = values
        .iter()
        .flatten()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
            (lo.min(*value), hi.max(*value))
        });
    let span = (max - min).max(f64::EPSILON);
    let header = h_flex()
        .w_full()
        .gap(px(4.))
        .child(div().w(px(76.)).flex_none())
        .children(x_labels.iter().map(|label| {
            div()
                .flex_1()
                .min_w_0()
                .text_center()
                .truncate()
                .text_size(px(10.))
                .text_color(muted)
                .child(label.clone())
        }));
    let rows = values.iter().enumerate().map(|(row_ix, row)| {
        let label = y_labels.get(row_ix).cloned().unwrap_or_default();
        h_flex()
            .w_full()
            .gap(px(4.))
            .child(
                div()
                    .w(px(76.))
                    .flex_none()
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(muted)
                    .child(label.clone()),
            )
            .children(row.iter().enumerate().map(|(col_ix, value)| {
                let strength = ((*value - min) / span) as f32;
                let tip = format!(
                    "{} · {}: {}",
                    label,
                    x_labels.get(col_ix).map(String::as_str).unwrap_or(""),
                    format_number(*value)
                );
                div()
                    .id(SharedString::from(format!("viz-cell-{key}-{}", row_ix * x_labels.len() + col_ix)))
                    .test_support()
                    .flex_1()
                    .min_w(px(8.))
                    .h(px(20.))
                    .rounded(px(3.))
                    .bg(accent.opacity(0.12 + strength.clamp(0., 1.) * 0.76))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
            }))
    });
    v_flex()
        .w_full()
        .gap(px(5.))
        .child(header)
        .children(rows)
        .into_any_element()
}

fn treemap_view(items: &[trek_core::visualization::TreemapItem], key: usize, cx: &App) -> AnyElement {
    let max = items.iter().map(|item| item.weight).fold(0.0_f64, f64::max).max(f64::EPSILON);
    h_flex()
        .w_full()
        .items_stretch()
        .flex_wrap()
        .gap(px(4.))
        .children(items.iter().enumerate().map(|(ix, item)| {
            let color = tone(item.tone, cx);
            let share = (item.weight / max) as f32;
            v_flex()
                .id(SharedString::from(format!("viz-tile-{key}-{ix}")))
                .test_support()
                .h(px(78.))
                .w(px(82. + 138. * share.clamp(0., 1.)))
                .flex_grow(1.)
                .justify_between()
                .gap_1()
                .p_2()
                .rounded(px(7.))
                .border_1()
                .border_color(color.opacity(0.38))
                .bg(color.opacity(0.13))
                .overflow_hidden()
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(px(11.5))
                        .font_medium()
                        .text_color(cx.theme().foreground)
                        .child(item.label.clone()),
                )
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(color)
                        .child(format_number(item.weight)),
                )
        }))
        .into_any_element()
}

fn mockup_view(nodes: &[MockNode], cx: &App) -> AnyElement {
    v_flex()
        .w_full()
        .gap_3()
        .p_3()
        .rounded(px(10.))
        .border_1()
        .border_color(cx.theme().foreground.opacity(0.08))
        .bg(cx.theme().background.opacity(0.65))
        .children(nodes.iter().map(|node| mock_node(node, cx)))
        .into_any_element()
}

fn mock_node(node: &MockNode, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = tone(node.tone, cx);
    let children = || node.children.iter().map(|child| mock_node(child, cx));
    match node.kind {
        MockNodeKind::Row => h_flex()
            .w_full()
            .items_stretch()
            .gap_2()
            .flex_wrap()
            .children(children())
            .into_any_element(),
        MockNodeKind::Column => v_flex()
            .w_full()
            .gap_2()
            .children(children())
            .into_any_element(),
        MockNodeKind::Card => v_flex()
            .flex_1()
            .min_w(px(180.))
            .gap_2()
            .p_3()
            .rounded(px(9.))
            .border_1()
            .border_color(theme.foreground.opacity(0.10))
            .bg(theme.foreground.opacity(0.025))
            .when_some(node.text.clone(), |el, text| {
                el.child(
                    div()
                        .font_medium()
                        .text_size(px(12.5))
                        .text_color(theme.foreground)
                        .child(text),
                )
            })
            .when_some(node.value.clone(), |el, value| {
                el.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(relative(1.4))
                        .text_color(theme.muted_foreground)
                        .child(value),
                )
            })
            .children(children())
            .into_any_element(),
        MockNodeKind::Text => div()
            .text_size(px(12.))
            .line_height(relative(1.45))
            .text_color(theme.foreground.opacity(0.88))
            .child(node.text.clone().unwrap_or_default())
            .into_any_element(),
        MockNodeKind::Metric => v_flex()
            .flex_1()
            .min_w(px(112.))
            .gap(px(2.))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(node.text.clone().unwrap_or_default()),
            )
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(17.))
                    .font_semibold()
                    .text_color(color)
                    .child(node.value.clone().unwrap_or_default()),
            )
            .children(children())
            .into_any_element(),
        MockNodeKind::Button => h_flex()
            .flex_none()
            .justify_center()
            .px_3()
            .h(px(28.))
            .rounded(px(7.))
            .border_1()
            .border_color(color.opacity(0.42))
            .bg(color.opacity(0.12))
            .text_size(px(11.5))
            .font_medium()
            .text_color(color)
            .child(node.text.clone().unwrap_or_default())
            .into_any_element(),
        MockNodeKind::Badge => h_flex()
            .flex_none()
            .px_2()
            .h(px(20.))
            .rounded_full()
            .bg(color.opacity(0.13))
            .text_size(px(10.5))
            .font_medium()
            .text_color(color)
            .child(node.text.clone().unwrap_or_default())
            .into_any_element(),
        MockNodeKind::Divider => div()
            .w_full()
            .h(px(1.))
            .bg(theme.foreground.opacity(0.08))
            .into_any_element(),
    }
}

fn format_number(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000_000. {
        format!("{:.1}T", value / 1_000_000_000_000.)
    } else if abs >= 1_000_000_000. {
        format!("{:.1}B", value / 1_000_000_000.)
    } else if abs >= 1_000_000. {
        format!("{:.1}M", value / 1_000_000.)
    } else if abs >= 1_000. {
        format!("{:.1}K", value / 1_000.)
    } else if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_number;

    #[test]
    fn values_read_compactly() {
        assert_eq!(format_number(85_000.), "85.0K");
        assert_eq!(format_number(24_000_000.), "24.0M");
        assert_eq!(format_number(5_000_000_000.), "5.0B");
        assert_eq!(format_number(1_000_000_000_000.), "1.0T");
        assert_eq!(format_number(-18.), "-18");
        assert_eq!(format_number(3.25), "3.2");
    }
}
