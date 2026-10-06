//! The sidebar's live area: threads that need eyes on them keep their cards; the quiet rest
//! group by project as compact lines, capped until opened.

use super::harness::{Trek, mock, open, run};
use crate::workspace::Route;
use gpui_kit::TestAppContext;
use trek_core::store::now_ms;
use trek_core::{Effort, HandHolding, RunState};

/// A seen, unsettled thread in the harness project, `older_by` ms old.
fn quiet(trek: &Trek, cx: &mut TestAppContext, title: &str, older_by: i64) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
        t.title = title.into();
        t.updated_at = now_ms() - older_by;
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

fn project_of(trek: &Trek, cx: &TestAppContext, id: &str) -> String {
    trek.read(cx, |ws, _| ws.thread(id).unwrap().project_id.clone().unwrap())
}

#[test]
fn quiet_threads_fold_into_a_project_group_capped_at_three() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000)).collect();
        trek.render(cx);
        let pid = project_of(&trek, cx, &ids[0]);
        // The newest three stay out; the rest wait behind "Show 2 more". None is a card.
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(trek.visible(cx, format!("live-line-{id}")), i < 3, "quiet {i}");
            assert!(!trek.visible(cx, format!("card-{id}")), "quiet {i} keeps no card");
        }
        assert!(trek.visible(cx, format!("live-proj-head-{pid}")));
        assert!(trek.visible(cx, format!("live-more-{pid}")));
        trek.click(cx, format!("live-more-{pid}"));
        trek.render(cx);
        for id in &ids {
            assert!(trek.visible(cx, format!("live-line-{id}")));
        }
        trek.click(cx, format!("live-less-{pid}"));
        trek.render(cx);
        assert!(!trek.visible(cx, format!("live-line-{}", ids[4])));
    });
}

#[test]
fn what_needs_you_keeps_its_card_over_quiet_lines() {
    run(async |cx| {
        let trek = open(cx);
        for i in 0..4 {
            quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000);
        }
        let asking = quiet(&trek, cx, "Asking", 10_000);
        let working = quiet(&trek, cx, "Working", 20_000);
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&asking, |t| t.run_state = RunState::NeedsYou).unwrap();
            ws.store.update_thread(&working, |t| t.run_state = RunState::Working).unwrap();
            ws.reload(cx);
        });
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-{asking}")), "the approval keeps its card");
        assert!(trek.visible(cx, format!("card-{working}")), "the running turn keeps its card");
        assert!(!trek.visible(cx, format!("live-line-{asking}")));
        assert!(!trek.visible(cx, format!("live-line-{working}")));
    });
}

#[test]
fn unread_and_open_lines_are_never_hidden() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = (0..5).map(|i| quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000)).collect();
        // The oldest has news: unread lines always show, wherever they'd sort.
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&ids[4], |t| t.last_seen_at = t.updated_at - 1).unwrap();
            ws.reload(cx);
        });
        // The open thread stays out even though it's seen and old.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[3].clone()), cx));
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-{}", ids[4])), "unread is never capped away");
        assert!(trek.visible(cx, format!("live-line-unseen-{}", ids[4])));
        assert!(trek.visible(cx, format!("live-line-{}", ids[3])), "the open thread stays");
        assert!(!trek.visible(cx, format!("live-line-{}", ids[2])), "a seen old one hides");
    });
}

#[test]
fn opening_a_live_group_doesnt_open_the_settled_one() {
    run(async |cx| {
        let trek = open(cx);
        for i in 0..4 {
            quiet(&trek, cx, &format!("Quiet {i}"), i as i64 * 60_000);
        }
        let settled = quiet(&trek, cx, "Settled", 5 * 60_000);
        for i in 0..5 {
            let id = quiet(&trek, cx, &format!("Settled {i}"), (6 + i) as i64 * 60_000);
            trek.update(cx, |ws, cx| {
                ws.store.update_thread(&id, |t| t.settled_at = Some(t.updated_at)).unwrap();
                ws.reload(cx);
            });
        }
        trek.update(cx, |ws, cx| {
            ws.store.update_thread(&settled, |t| t.settled_at = Some(t.updated_at)).unwrap();
            ws.reload(cx);
            ws.settled_open = true;
            cx.notify();
        });
        let pid = project_of(&trek, cx, &settled);
        trek.render(cx);
        // Settled shows its five; opening the live group leaves it capped.
        assert!(trek.visible(cx, format!("live-more-{pid}")));
        trek.click(cx, format!("live-more-{pid}"));
        trek.render(cx);
        let settled_ids = trek.read(cx, |ws, _| {
            ws.threads.iter().filter(|t| t.settled_at.is_some() && t.project_id.as_deref() == Some(pid.as_str())).map(|t| t.id.clone()).collect::<Vec<_>>()
        });
        let shown = settled_ids.iter().filter(|id| trek.visible(cx, format!("line-{id}"))).count();
        assert_eq!(shown, 5, "the settled group still caps at five");
    });
}
