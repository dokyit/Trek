//! The API cost estimate under the composer: its label as usage comes in, its breakdown on
//! hover, and the same figures after a relaunch.

use super::harness::{Trek, launch, mock, new_project, run, settings};
use crate::workspace::Route;
use gpui_kit::TestAppContext;
use std::time::Duration;
use trek_agents::{AgentEvent, Billing};
use trek_core::store::Store;
use trek_core::{AgentId, Effort, HandHolding, RunState, TokenUsage, UsageCost};

fn label(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<String> {
    trek.read(cx, |ws, _| {
        let t = ws.thread(id).cloned()?;
        crate::cost::label(ws.billing_of(&t).as_ref(), &ws.spend_of(id))
    })
}

fn hover(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let b = trek.bounds(cx, id.to_string()).expect("drawn");
    let at = gpui_kit::point(b.origin.x + b.size.width / 2., b.origin.y + b.size.height / 2.);
    trek.window(cx, |window, cx| {
        window.dispatch_event(gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx)
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    trek.render(cx);
}

#[test]
fn the_estimate_follows_usage_and_survives_a_relaunch() {
    run(async |cx| {
        let dir = new_project("cost");
        let db = dir.join("trek.sqlite");
        let project = new_project("project");
        let mut s = settings();
        s.user_projects.push(project.display().to_string());
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s.clone());
        let trek = Trek { ws, root, window, project: project.clone() };
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        trek.render(cx);
        // Nothing spent yet, nothing shown.
        assert!(!trek.visible(cx, "cost-estimate"));
        // The mock's turn on Claude Sonnet 5.5, billed per token: 2,400 × $2 + 1,850 × $10 +
        // 182,000 × $0.20 + 12,600 × $4 (1-hour cache writes), per million.
        let id = trek.send(cx, "mock:cost");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("$0.11"));
        assert!(trek.visible(cx, "cost-estimate"));
        hover(&trek, cx, "cost-estimate");
        assert!(trek.visible(cx, "cost-breakdown"), "the breakdown on hover");
        // On a plan the same tokens are an estimate at API prices.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::Billing(Billing::Plan(Some("Claude Max".into())))], cx));
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("≈ $0.11 at API prices"));
        // Streaming text changes nothing; the next usage report does.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("more".into())], cx));
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("≈ $0.11 at API prices"));
        let tokens = TokenUsage { input: 1_000, output: 2_000, cache_read: 0, cache_write: 0 };
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::Usage { model: Some("claude-opus-5-5".into()), tokens, cost: Some(UsageCost::reported(1.0)) }], cx));
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("≈ $1.11 at API prices"));
        // A consult under it adds its share.
        let child = trek.update(cx, |ws, cx| {
            let mut c = ws.store.create_thread(Some(&project), AgentId::Codex, Some("gpt-5.6-luna".into()), Effort::Low, HandHolding::Auto).unwrap();
            c.parent_id = Some(id.clone());
            ws.store.save_thread(&c).unwrap();
            ws.reload(cx);
            c.id
        });
        let used = TokenUsage { input: 100_000, output: 10_000, cache_read: 0, cache_write: 0 };
        trek.update(cx, |ws, cx| ws.apply_events(&child, vec![AgentEvent::Usage { model: None, tokens: used, cost: None }], cx));
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("≈ $1.14 at API prices"), "+ 100,000 × $0.20 + 10,000 × $1.20 per million");
        let b = trek.read(cx, |ws, _| crate::cost::breakdown(Some(&Billing::Plan(Some("Claude Max".into()))), &ws.spend_of(&id))).unwrap();
        assert_eq!(b.subs.as_deref(), Some("+ $0.03 from 1 consult"));
        assert_eq!(b.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Claude Opus 5.5", "Claude Sonnet 5.5"]);

        // Relaunched, it reads the same from the store.
        drop(trek);
        let (ws, root, window) = launch(cx, Store::open(&db).expect("store"), s);
        let trek = Trek { ws, root, window, project };
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        let spend = trek.read(cx, |ws, _| ws.spend_of(&id));
        assert!((spend.own.usd() + spend.subs.usd() - 1.1421).abs() < 1e-9, "{spend:?}");
        assert_eq!(spend.sub_threads, 1);
        // Once its session says how it's billed, the strip says it again.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::Billing(Billing::Plan(Some("Claude Max".into())))], cx));
        trek.render(cx);
        assert_eq!(label(&trek, cx, &id).as_deref(), Some("≈ $1.14 at API prices"));
        assert!(trek.visible(cx, "cost-estimate"));
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn local_models_show_nothing_and_unpriced_models_show_tokens() {
    run(async |cx| {
        let trek = super::harness::open(cx);
        // The mock runs "on this Mac": nothing is billed, so nothing is shown.
        let id = trek.send(cx, "explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert_eq!(label(&trek, cx, &id), None);
        assert!(!trek.visible(cx, "cost-estimate"));
        // An agent whose model has no known price: its tokens, and no guess.
        let t = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), AgentId::Acp("devin".into()), Some("fusion-claude-opus-5-5-high-sidekick-swe-2-medium".into()), Effort::Medium, HandHolding::Auto).unwrap();
            ws.reload(cx);
            t.id
        });
        let tokens = TokenUsage { input: 12_000, output: 300, cache_read: 0, cache_write: 0 };
        trek.update(cx, |ws, cx| ws.apply_events(&t, vec![AgentEvent::Usage { model: None, tokens, cost: None }], cx));
        assert_eq!(label(&trek, cx, &t).as_deref(), Some("12.3K tokens · price unknown"));
        let _ = mock();
    });
}
