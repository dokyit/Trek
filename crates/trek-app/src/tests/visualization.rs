//! Native visualizations in answers: every type draws with its stable ids, in both themes and
//! in a narrow column, and its Chart / Data views switch.

use super::harness::{Trek, open, run};
use gpui_kit::TestAppContext;
use trek_agents::AgentEvent;
use trek_core::settings::ThemeChoice;

/// A thread whose one answer is `text`, on screen.
fn answered(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    let id = trek.quiet_thread(cx);
    let events = vec![AgentEvent::TextDelta(text.into()), AgentEvent::TextDone(text.into()), AgentEvent::TurnComplete { error: None }];
    trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
    trek.render(cx);
    id
}

/// The ids of every element on screen, as `click` would name them.
fn on_screen(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    cx.run_until_parked();
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible())
            .filter_map(|s| match s.path().last()? {
                gpui_kit::ElementId::Name(n) => Some(n.to_string()),
                _ => None,
            })
            .collect()
    })
}

fn find(ids: &[String], prefix: &str) -> Option<String> {
    ids.iter().find(|id| id.starts_with(prefix)).cloned()
}

/// The body and a mark of each type, by id prefix.
const PARTS: &[(&str, &str, &str)] = &[
    ("bar", "viz-barchart-", "viz-bar-"),
    ("line", "viz-line-", "viz-lineplot-"),
    ("donut", "viz-donut-", "viz-part-"),
    ("stats", "viz-stats-", "viz-stat-"),
    ("table", "viz-table-", "viz-table-"),
    ("heatmap", "viz-heatmap-", "viz-cell-"),
    ("treemap", "viz-treemap-", "viz-tile-"),
    ("timeline", "viz-timeline-", "viz-event-"),
    ("flow", "viz-flow-", "viz-node-"),
    ("layers", "viz-layers-", "viz-layer-"),
    ("mockup", "viz-mockup-", "viz-frame-"),
];

#[test]
fn every_type_draws_natively_in_both_themes() {
    run(async |cx| {
        let trek = open(cx);
        assert_eq!(PARTS.len(), trek_core::visualization::TYPES.len());
        for (kind, answer) in trek_agents::mock::VIZ_GALLERY {
            let (_, body, mark) = PARTS.iter().find(|p| p.0 == *kind).expect("a type we know");
            answered(&trek, cx, answer);
            for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
                cx.update(|cx| crate::apply_theme(choice, None, cx));
                trek.render(cx);
                let ids = on_screen(&trek, cx);
                assert!(find(&ids, "trek-viz-").is_some(), "{kind} {choice:?}: no card in {ids:?}");
                assert!(find(&ids, body).is_some(), "{kind} {choice:?}: no {body} in {ids:?}");
                assert!(find(&ids, mark).is_some(), "{kind} {choice:?}: no {mark} in {ids:?}");
                // The Data view wherever there are values (when the header is in view).
                if find(&ids, "viz-json-").is_some() {
                    assert_eq!(find(&ids, "viz-data-").is_some(), !matches!(*kind, "mockup" | "table"), "{kind}: a table is its own data");
                }
            }
            cx.update(|cx| crate::apply_theme(ThemeChoice::Night, None, cx));
        }
    });
}

#[test]
fn the_data_view_shows_the_values_as_a_table_and_back() {
    run(async |cx| {
        let trek = open(cx);
        let line = trek_agents::mock::VIZ_GALLERY.iter().find(|(k, _)| *k == "line").unwrap().1;
        answered(&trek, cx, line);
        let ids = on_screen(&trek, cx);
        let data = find(&ids, "viz-data-").expect("a Data switch");
        assert!(find(&ids, "viz-json-").is_some() && find(&ids, "viz-copy-").is_some(), "copy as JSON and as a table");
        assert!(find(&ids, "viz-datatable-").is_none());
        // The legend switches a series off and back on; the chart stays.
        let legend = find(&ids, "viz-legend-").expect("a legend for three series");
        trek.click(cx, gpui_kit::SharedString::from(legend.clone()));
        trek.click(cx, gpui_kit::SharedString::from(legend));
        assert!(find(&on_screen(&trek, cx), "viz-lineplot-").is_some());

        trek.click(cx, gpui_kit::SharedString::from(data));
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "viz-datatable-").is_some(), "{ids:?}");
        assert!(find(&ids, "viz-lineplot-").is_none(), "the chart gives way to its table");
        trek.click(cx, gpui_kit::SharedString::from(find(&ids, "viz-chart-").expect("a Chart switch")));
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "viz-lineplot-").is_some() && find(&ids, "viz-datatable-").is_none());
    });
}

#[test]
fn a_narrow_window_still_draws_every_type() {
    run(async |cx| {
        let trek = open(cx);
        trek.window(cx, |window, _| window.resize(gpui_kit::size(gpui_kit::px(560.), gpui_kit::px(900.))));
        for (kind, answer) in trek_agents::mock::VIZ_GALLERY {
            let (_, body, _) = PARTS.iter().find(|p| p.0 == *kind).unwrap();
            answered(&trek, cx, answer);
            // A second frame: the first measures the column.
            trek.render(cx);
            let ids = on_screen(&trek, cx);
            assert!(find(&ids, body).is_some(), "{kind}: no {body} in {ids:?}");
        }
    });
}

/// The labels of elements on screen whose ids start with `prefix`.
fn labels(trek: &Trek, cx: &mut TestAppContext, prefix: &str) -> Vec<String> {
    cx.run_until_parked();
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible() && matches!(s.path().last(), Some(gpui_kit::ElementId::Name(n)) if n.starts_with(prefix)))
            .filter_map(|s| s.label().map(str::to_string))
            .collect()
    })
}

#[test]
fn an_invalid_block_says_why_over_its_json() {
    run(async |cx| {
        let trek = open(cx);
        answered(&trek, cx, "Here:\n\n```trek-viz\n{\"version\":1,\"title\":\"x\",\"summary\":\"y\",\"type\":\"bar\",\"bars\":[],\"html\":\"<b>\"}\n```\n");
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "trek-viz-").is_none(), "{ids:?}");
        assert!(find(&ids, "viz-invalid-").is_some(), "{ids:?}");
        // The JSON stays, as a code block (its copy button) under the note.
        assert!(ids.iter().any(|id| id == "copy"), "{ids:?}");
        let note = labels(&trek, cx, "viz-invalid-").concat();
        assert!(note.starts_with("Couldn't draw this visualization: unknown field `html`"), "{note}");
        // An over-limit block reads as such, not as a JSON error.
        let big = format!("```trek-viz\n{{\"version\":1,\"title\":\"{}\",\"summary\":\"y\",\"type\":\"bar\",\"bars\":[{{\"label\":\"a\",\"value\":1}}]}}\n```", "x".repeat(40_000));
        answered(&trek, cx, &big);
        let note = labels(&trek, cx, "viz-invalid-").concat();
        assert!(note.contains("over the 32 KiB limit"), "{note}");
    });
}

#[test]
fn a_block_still_arriving_shows_its_frame_then_its_chart() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let answer = trek_agents::mock::VIZ_GALLERY.iter().find(|(k, _)| *k == "bar").unwrap().1;
        let mut sent = 0;
        let mut send = |trek: &Trek, cx: &mut TestAppContext, upto: &str| {
            let end = answer.find(upto).unwrap() + upto.len();
            trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta(answer[sent..end].into())], cx));
            sent = end;
            trek.render(cx);
            on_screen(trek, cx)
        };
        // The type and title are in, no bar is whole: the card's frame, and no raw JSON.
        let ids = send(&trek, cx, r#""bars":[{"label":"trek-app","val"#);
        assert!(find(&ids, "viz-pending-").is_some(), "{ids:?}");
        assert!(find(&ids, "trek-viz-").is_none() && !ids.iter().any(|id| id == "copy"), "no code block while it's written: {ids:?}");
        assert_eq!(labels(&trek, cx, "viz-pending-").concat(), "Visualization being drawn: Build time by crate");
        // Two bars whole: drawn already, still marked as arriving, nothing to copy yet.
        let ids = send(&trek, cx, r#"{"label":"trek-core","value":48},"#);
        assert!(find(&ids, "trek-viz-").is_some() && find(&ids, "viz-drawing-").is_some(), "{ids:?}");
        assert!(find(&ids, "viz-pending-").is_none() && find(&ids, "viz-json-").is_none(), "{ids:?}");
        assert_eq!(ids.iter().filter(|id| id.starts_with("viz-bar-")).count(), 2, "{ids:?}");
        // The rest, and the message is done: the whole chart, with its actions.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta(answer[sent..].into()), AgentEvent::TextDone(answer.into()), AgentEvent::TurnComplete { error: None }], cx)
        });
        trek.render(cx);
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "trek-viz-").is_some() && find(&ids, "viz-json-").is_some(), "{ids:?}");
        assert!(find(&ids, "viz-drawing-").is_none() && find(&ids, "viz-pending-").is_none(), "{ids:?}");
        assert_eq!(ids.iter().filter(|id| id.starts_with("viz-bar-")).count(), 5, "{ids:?}");
    });
}

#[test]
fn a_block_left_open_when_the_answer_ends_says_so() {
    run(async |cx| {
        let trek = open(cx);
        answered(&trek, cx, "Here:\n\n```trek-viz\n{\"version\":1,\"title\":\"x\",\"summary\":\"y\",\"type\":\"bar\",\"bars\":[{\"label\":\"a\",\"value\":1}");
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "viz-pending-").is_none() && find(&ids, "viz-drawing-").is_none(), "nothing still arriving: {ids:?}");
        assert!(labels(&trek, cx, "viz-invalid-").concat().contains("ended before the block was finished"));
    });
}

#[test]
fn an_answer_with_visualizations_keeps_its_rows_apart() {
    run(async |cx| {
        let trek = open(cx);
        // With motion, as on screen: the marks arrive over the first frames.
        cx.update(|cx| cx.set_reduce_motion(false));
        trek.window(cx, |window, _| window.resize(gpui_kit::size(gpui_kit::px(1440.), gpui_kit::px(1000.))));
        let id = trek.send(cx, "mock:visualization");
        trek.wait_done(cx, &id, trek_core::RunState::Idle).await;
        for _ in 0..3 {
            trek.render(cx);
        }
        std::thread::sleep(std::time::Duration::from_millis(900));
        for _ in 0..3 {
            trek.render(cx);
        }
        let answer = trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::Assistant { .. }));
        let end = trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::TurnEnd { .. }));
        let text = trek.bounds(cx, ("answer-text", answer)).expect("the answer on screen");
        let copy = trek.bounds(cx, ("copy-turn", end)).expect("its footer on screen");
        assert!(text.bottom() <= copy.top(), "the answer ends at {:?}, its footer starts at {:?}", text.bottom(), copy.top());
        // The last artifact, then the closing sentence, both inside the answer's box.
        let ids = on_screen(&trek, cx);
        let last = ids.iter().filter(|i| i.starts_with("trek-viz-")).max_by_key(|i| i["trek-viz-".len()..].parse::<usize>().unwrap_or(0)).expect("a card").clone();
        let card = trek.bounds(cx, gpui_kit::SharedString::from(last)).expect("the last card");
        assert!(card.bottom() + gpui_kit::px(24.) <= text.bottom(), "the card ends at {:?}, the answer at {:?}: no room for the sentence after it", card.bottom(), text.bottom());
    });
}
