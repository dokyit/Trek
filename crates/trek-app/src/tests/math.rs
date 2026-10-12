//! Maths in answers draws as maths: the mock's worked example sets its formulas, inline and
//! displayed, in both themes, and the sentence with prices in it stays prose.

use super::harness::{Trek, open, run};
use gpui_kit::TestAppContext;
use trek_core::RunState;
use trek_core::settings::ThemeChoice;

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

#[test]
fn the_mock_answer_sets_its_maths_and_leaves_prices_alone() {
    run(async |cx| {
        let trek = open(cx);
        // Tall, so most of the answer is painted (its first lines may still sit above the view).
        trek.window(cx, |window, _| window.resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(2400.))));
        let id = trek.send(cx, "mock:math");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek_agents::mock::MATH;
        let at = |needle: &str| answer.find(needle).unwrap_or_else(|| panic!("{needle} in the answer"));
        for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            trek.render(cx);
            let ids = on_screen(&trek, cx);
            let maths: Vec<&String> = ids.iter().filter(|id| id.starts_with("math-")).collect();
            // Inline: `$p' = …$` and the `\( … \)` in part (b).
            for inline in ["$p' = 0.0162482$", "\\(P(\\theta=1 \\mid T_1"] {
                let id = format!("math-{}", at(inline));
                assert!(ids.contains(&id), "{choice:?}: no {id} for {inline} in {maths:?}");
            }
            // Displayed: the `\[` block, the one-line `\[ … \]` paragraph, and a `$$` paragraph.
            for display in ["\\[\nP(", "\\[ P(", "$$\\sum"] {
                let id = format!("math-block-{}", at(display));
                assert!(ids.contains(&id), "{choice:?}: no {id} for {display} in {maths:?}");
            }
            // "$5 and $10" pairs up as a math span, which nothing claims.
            let price = format!("math-{}", at("$5 and $10"));
            assert!(!ids.contains(&price), "{choice:?}: the prices drew as maths");
        }
        cx.update(|cx| crate::apply_theme(ThemeChoice::Night, None, cx));
    });
}
