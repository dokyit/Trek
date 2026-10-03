//! Basecamp: opening and leaving it, the fresh-install invitation, a populated recap in both
//! themes, reviewing from it, and the token usage a turn records for it.

use super::harness::{Trek, mock, open, run, store_items};
use crate::workspace::Route;
use gpui_kit::TestAppContext;
use trek_core::settings::ThemeChoice;
use trek_core::store::{Item, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState, TokenUsage};

/// A "Ready for review" row's id.
fn row(thread: &str) -> gpui_kit::SharedString {
    format!("review-{thread}").into()
}

fn theme(cx: &mut TestAppContext, choice: ThemeChoice) {
    cx.update(|cx| crate::apply_theme(choice, None, cx));
}

/// Wait until Basecamp has a recap computed from what the workspace holds now.
fn recap(trek: &Trek, cx: &mut TestAppContext) -> trek_core::basecamp::Recap {
    let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
    for _ in 0..500 {
        cx.run_until_parked();
        if let Some(r) = basecamp.read_with(cx, |b, _| b.recap().cloned()) {
            return r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no recap");
}

#[test]
fn a_fresh_install_is_invited_to_start_and_esc_goes_back() {
    run(async |cx| {
        let trek = open(cx);
        let draft = trek.read(cx, |ws, _| ws.route.clone());
        trek.click(cx, "open-basecamp");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Basecamp);
        assert!(recap(&trek, cx).is_empty());
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            assert!(trek.visible(cx, "basecamp-empty"), "the invitation to start");
            assert!(!trek.visible(cx, "basecamp-narrative"));
        }
        trek.click(cx, "basecamp-new-thread");
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { .. }));
        // ⌘⇧H opens it with the sidebar folded away too; Esc goes back where it was opened from.
        trek.update(cx, |ws, _| ws.sidebar_collapsed = true);
        trek.press(cx, "cmd-shift-h");
        trek.render(cx);
        assert!(trek.visible(cx, "basecamp"));
        assert!(!trek.visible(cx, "open-basecamp"), "the sidebar stays folded");
        trek.press(cx, "escape");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), draft);
    });
}

#[test]
fn a_day_of_work_is_recapped_and_reviewed() {
    run(async |cx| {
        let trek = open(cx);
        // Kept inside today, even a minute after midnight.
        let today = trek_core::basecamp::Range::Today.window(&chrono::Local::now()).start;
        let now = now_ms().max(today + 61_000);
        let (done, failed, quiet) = trek.update(cx, |ws, cx| {
            let mut ids = vec![];
            for (title, model) in [("Ship the parser", "mock-deep"), ("Fix the flaky test", "mock-swift"), ("Read the docs", "mock-swift")] {
                let t = ws.store.create_thread(Some(&trek.project), mock(), Some(model.into()), Effort::Medium, HandHolding::Auto).unwrap();
                let items = vec![
                    Item::User { text: title.into(), images: vec![], at: Some(now - 60_000), resume: None, aside: false },
                    Item::Assistant { text: "Done.".into() },
                    Item::TurnEnd { at: now - 1_000, took_secs: 59 },
                ];
                store_items(&ws.store, &t.id, items);
                let tokens = TokenUsage { input: 1_000, output: 400, cache_read: if model == "mock-deep" { 30_000 } else { 5_000 }, cache_write: 0 };
                ws.store.record_usage(&t.id, now - 1_000, &mock(), Some(model), &tokens).unwrap();
                ids.push(t.id);
            }
            ws.store.update_thread(&ids[0], |t| {
                t.last_seen_at = t.updated_at - 1;
                t.additions = 372;
                t.deletions = 15;
            })
            .unwrap();
            ws.store.update_thread(&ids[1], |t| t.run_state = RunState::Failed).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Basecamp, cx);
            (ids[0].clone(), ids[1].clone(), ids[2].clone())
        });
        let r = recap(&trek, cx);
        assert_eq!((r.prompts, r.threads, r.turns, r.failed), (3, 3, 3, 1));
        assert_eq!(r.tokens.total(), 3 * 1_400 + 40_000);
        assert_eq!(r.best_model().and_then(|m| m.model.clone()).as_deref(), Some("mock-deep"));
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            for id in ["basecamp-narrative", "basecamp-profile"] {
                assert!(trek.visible(cx, id), "{id} in {choice:?}");
            }
            // The failure waits on the user, the unread thread is ready; the read one isn't listed.
            assert!(trek.visible(cx, row(&failed)));
            assert!(trek.visible(cx, row(&done)));
            assert!(!trek.visible(cx, row(&quiet)));
        }
        // Hovering the profile picks out the stretch under the pointer.
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        let (bounds, _) = basecamp.read_with(cx, |b, _| b.profile_hover());
        let b = bounds.expect("the profile was drawn");
        let at = gpui_kit::point(b.origin.x + b.size.width * 0.52, b.origin.y + b.size.height / 2.);
        trek.window(cx, |window, cx| {
            window.dispatch_event(gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx)
        });
        cx.run_until_parked();
        assert_eq!(basecamp.read_with(cx, |b, _| b.profile_hover().1), Some((0.52 * r.buckets.len() as f32) as usize), "the stretch from noon");
        let order: Vec<String> = trek.read(cx, |ws, _| ws.ready_for_review().iter().map(|t| t.id.clone()).collect());
        assert_eq!(order, [failed.clone(), done.clone()], "what needs you comes first");
        // Mark all read: the finished one is reviewed; the failure still needs the user.
        trek.click(cx, "basecamp-mark-read");
        trek.render(cx);
        assert!(!trek.visible(cx, row(&done)));
        assert!(trek.visible(cx, row(&failed)));
        // A row opens its thread.
        trek.click(cx, row(&failed));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(failed));
    });
}

#[test]
fn a_turn_records_the_tokens_its_agent_reported() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "explain the project");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let rows = trek.read(cx, |ws, _| ws.store.usage_between(0, i64::MAX).unwrap());
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!((rows[0].thread_id.as_str(), &rows[0].agent, rows[0].model.as_deref()), (id.as_str(), &mock(), Some("mock-swift")));
        assert!(rows[0].tokens.output > 0);
        // Basecamp counts it.
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let r = recap(&trek, cx);
        assert_eq!((r.prompts, r.turns, r.tokens.total()), (1, 1, rows[0].tokens.total()));
        assert_eq!(r.models.first().map(|m| m.agent.clone()), Some(AgentId::Direct("mock".into())));
    });
}
