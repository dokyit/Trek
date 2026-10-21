//! The thinking-process timeline in the thread view (`thread_view::thinking`): a header plus a
//! node per step, fanning out for parallel calls, dashing for dead ends, and collapsing to
//! "Thought for Ns" when the turn ends.

use super::harness::{Trek, open, run};
use gpui_kit::TestAppContext;
use trek_agents::AgentEvent;
use trek_core::store::{Item, ToolStatus};

/// The thinking timeline's header text ("Thinking Ns" while it runs, "Thought for Ns" once done).
fn header(trek: &Trek, cx: &TestAppContext) -> Option<String> {
    trek.thread_view(cx).read_with(cx, |v, cx| v.thinking_header(cx))
}

/// Open a quiet thread and put the user's message in it, so there's a turn for the timeline to
/// anchor on.
fn working_thread(trek: &Trek, cx: &mut TestAppContext) -> String {
    let id = trek.quiet_thread(cx);
    seed(trek, cx, &id, vec![Item::User { text: "go".into(), images: vec![], at: None, resume: None, aside: false }]);
    id
}

/// Append `items` straight into the open thread's transcript (the tests don't need the store).
fn seed(trek: &Trek, cx: &mut TestAppContext, id: &str, items: Vec<Item>) {
    trek.update(cx, |ws, _| {
        let live = ws.live.get_mut(id).expect("an open thread");
        for item in items {
            live.items.push(item);
        }
        live.revision += 1;
    });
}

#[test]
fn two_parallel_tool_calls_fan_out_and_merge() {
    run(async |cx| {
        let trek = open(cx);
        let id = working_thread(&trek, cx);
        // Two calls start before either finishes: they run in parallel.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolStarted { id: "a".into(), title: "Read".into(), detail: "src/a.rs".into() },
                    AgentEvent::ToolStarted { id: "b".into(), title: "Read".into(), detail: "src/b.rs".into() },
                ],
                cx,
            );
        });
        trek.render(cx);
        assert!(trek.visible(cx, "thinking-header"), "the header shows while it works");
        assert!(trek.visible(cx, "thinking-node-a"), "the first parallel call");
        assert!(trek.visible(cx, "thinking-node-b"), "the second parallel call");
        // Both finish: the fan-out closes into a hollow merge node.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolFinished { id: "a".into(), output: String::new(), ok: true },
                    AgentEvent::ToolFinished { id: "b".into(), output: String::new(), ok: true },
                ],
                cx,
            );
        });
        trek.render(cx);
        assert!(trek.visible(cx, "thinking-merge-a"), "the merge node after the parallel calls");
    });
}

#[test]
fn an_errored_tool_call_is_a_dashed_dead_end() {
    run(async |cx| {
        let trek = open(cx);
        let id = working_thread(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolStarted { id: "fail".into(), title: "Run command".into(), detail: "npm test".into() },
                    AgentEvent::ToolFinished { id: "fail".into(), output: "exit 1".into(), ok: false },
                ],
                cx,
            );
        });
        trek.render(cx);
        assert!(trek.visible(cx, "thinking-deadend-fail"), "the failed call is a dead end");
    });
}

#[test]
fn a_finished_turn_reads_thought_for_and_is_collapsed() {
    run(async |cx| {
        let trek = open(cx);
        let id = working_thread(&trek, cx);
        seed(
            &trek,
            cx,
            &id,
            vec![
                Item::Reasoning { text: "**Exploring the codebase**".into() },
                Item::Tool { id: "t".into(), title: "Read".into(), detail: "src/main.rs".into(), output: String::new(), status: ToolStatus::Done },
                Item::Assistant { text: "Done.".into() },
                Item::TurnEnd { at: 1, took_secs: 14 },
            ],
        );
        trek.render(cx);
        assert_eq!(header(&trek, cx).as_deref(), Some("Thought for 14s"));
        assert!(!trek.visible(cx, "thinking-body"), "the timeline collapses once the turn ends");
        // A click reopens the full timeline, with its closing node.
        trek.click(cx, "thinking-header");
        trek.render(cx);
        assert!(trek.visible(cx, "thinking-body"));
        assert!(trek.visible(cx, "thinking-final"));
    });
}

#[test]
fn long_turns_summarise_earlier_steps() {
    run(async |cx| {
        let trek = open(cx);
        let id = working_thread(&trek, cx);
        let n = 60;
        trek.update(cx, |ws, cx| {
            let mut events = Vec::with_capacity(n * 2);
            for i in 0..n {
                events.push(AgentEvent::ToolStarted { id: format!("t{i}"), title: "Read".into(), detail: format!("src/f{i}.rs") });
                events.push(AgentEvent::ToolFinished { id: format!("t{i}"), output: String::new(), ok: true });
            }
            ws.apply_events(&id, events, cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, "thinking-earlier"), "older steps fold into a summary");
    });
}