# Inline visualizations

## What it does

Lets an agent add a native visual artifact to an answer with a fenced `trek-viz` JSON block. Version 1 supports bar charts, heatmaps, treemaps, and bounded UI mockups. Trek validates the schema and renders it with Night or Paper theme tokens; incomplete, invalid, or unsupported blocks stay ordinary readable code. Artifacts are inert: the format contains no HTML, CSS, URLs, scripts, filesystem access, or event handlers.

## Reach it as a user

Ask an agent to compare data visually, show a chart, or mock several interface treatments. The agent is told the supported schema automatically and should include the conclusion in prose. A valid artifact appears inline in its answer; use the copy button in the artifact header to copy its source JSON.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "mock:visualization"
./.agents/skills/verify-trek/scripts/trek-dev screenshot visualizations-night --output /tmp/trek-visualizations-night.png
./.agents/skills/verify-trek/scripts/trek-dev control theme paper
./.agents/skills/verify-trek/scripts/trek-dev screenshot visualizations-paper --output /tmp/trek-visualizations-paper.png
```

The mock response contains a labeled service-health heatmap and two side-by-side release-status treatments. It is deterministic and never invokes a real provider.

## Code

Schema and validation: `crates/trek-core/src/visualization.rs`. Native GPUI rendering and Markdown block integration: `crates/trek-app/src/visualization.rs`, `md.rs`, and `thread_view.rs`. Agent capability instructions start in `workspace.rs`; the deterministic path is `crates/trek-agents/src/mock.rs`.

## Verify

Both JSON fences render as native cards rather than code. The heatmap has weekday and service labels, each cell exposes its value, and the treatment cards keep their labels and values at narrow widths. Night and Paper both remain readable. Copying visualization data reports success. `logs --grep error` is empty, and an invalid or unfinished `trek-viz` fence remains a normal code block.
