//! Interactive artifacts in answers: a finished `trek-html` block runs in a card (a stand-in web
//! view here), an unfinished one waits; the page sets its card's height within bounds, through
//! messages only its bootstrap can send; the view shows only where the card can be seen, and steps
//! aside for Trek's overlays and other threads; a theme switch reaches the page; the setting turns
//! it all off, instructions included.

use super::harness::{Trek, open, open_with, run};
use crate::artifact::{ArtifactReport, fake_web, placements};
use gpui_kit::{TestAppContext, px, size};
use trek_agents::AgentEvent;
use trek_core::RunState;
use trek_core::settings::ThemeChoice;

const PAGE: &str = "<!doctype html><html><head><title>Desk &amp; clamp</title></head><body><canvas></canvas></body></html>";

fn answer(page: &str) -> String {
    format!("Here it is:\n\n```trek-html\n{page}\n```\n\nDrag to turn it.")
}

/// A thread whose answer so far is `text` (still streaming unless `done`), on screen.
fn answered(trek: &Trek, cx: &mut TestAppContext, text: &str, done: bool) -> String {
    let id = trek.quiet_thread(cx);
    let mut events = vec![AgentEvent::TextDelta(text.into())];
    if done {
        events.extend([AgentEvent::TextDone(text.into()), AgentEvent::TurnComplete { error: None }]);
    }
    trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
    settle(trek, cx);
    id
}

/// A few frames: the first lays the card out, the next reads what covers it.
fn settle(trek: &Trek, cx: &mut TestAppContext) {
    trek.window(cx, |window, _| window.activate_window());
    for _ in 0..3 {
        trek.render(cx);
    }
}

/// The first element on screen whose id starts with `prefix`.
fn on_screen(trek: &Trek, cx: &mut TestAppContext, prefix: &str) -> Option<gpui_kit::SharedString> {
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window).into_iter().filter(|s| s.visible()).find_map(|s| match s.path().last()? {
            gpui_kit::ElementId::Name(n) if n.starts_with(prefix) => Some(n.clone()),
            _ => None,
        })
    })
}

fn cards(trek: &Trek, cx: &mut TestAppContext) -> Vec<ArtifactReport> {
    trek.window(cx, |window, cx| placements(window, cx))
}

fn one(trek: &Trek, cx: &mut TestAppContext) -> ArtifactReport {
    let all = cards(trek, cx);
    assert_eq!(all.len(), 1, "{all:?}");
    all.into_iter().next().unwrap()
}

#[test]
fn a_finished_block_runs_in_a_card_and_an_open_one_waits() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        // Still being written: a placeholder, and nothing runs.
        let open_fence = format!("Here it is:\n\n```trek-html\n{}", &PAGE[..60]);
        let id = answered(&trek, cx, &open_fence, false);
        assert!(on_screen(&trek, cx, "artifact-building-").is_some(), "a Building… card");
        assert_eq!(log.borrow().made, 0, "no view for a block still being written");
        assert!(cards(&trek, cx).is_empty());

        // Finished: a card, a view, placed where the card is.
        let text = answer(PAGE);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDone(text.clone()), AgentEvent::TurnComplete { error: None }], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        settle(&trek, cx);
        assert_eq!(log.borrow().made, 1);
        let card = one(&trek, cx);
        assert_eq!(card.title.as_deref(), Some("Desk & clamp"));
        assert!(card.has_view && card.live, "{card:?}");
        let (placed, body, mask) = (card.placed.unwrap(), card.body.unwrap(), card.mask.unwrap());
        assert_eq!(placed.body, body);
        assert!(mask.contains(&placed.clip.origin) && placed.clip.bottom() <= mask.bottom() && placed.clip.top() >= body.top(), "{placed:?} in {mask:?}");
        // Its bootstrap carries a secret of its own.
        let bootstrap = log.borrow().bootstraps[0].clone();
        assert!(!bootstrap.contains("__NONCE__") && bootstrap.contains("const N = '"));
    });
}

#[test]
fn the_page_sets_its_cards_height_within_bounds_and_only_the_page_can() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        assert_eq!(one(&trek, cx).height, trek_core::artifact::DEFAULT_HEIGHT);
        let post = |trek: &Trek, cx: &mut TestAppContext, body: &str| {
            trek.window(cx, |window, cx| crate::artifact::post(window, body, cx));
            settle(trek, cx);
        };
        post(&trek, cx, r#"{"n":"{nonce}","t":"h","v":480}"#);
        assert_eq!(one(&trek, cx).height, 480.);
        let body = one(&trek, cx).body.unwrap();
        assert_eq!(body.size.height, px(480.), "the card is as tall as the page says");
        post(&trek, cx, r#"{"n":"{nonce}","t":"h","v":9000}"#);
        assert_eq!(one(&trek, cx).height, trek_core::artifact::MAX_HEIGHT, "it scrolls inside past the most");
        post(&trek, cx, r#"{"n":"{nonce}","t":"h","v":12}"#);
        assert_eq!(one(&trek, cx).height, trek_core::artifact::MIN_HEIGHT);
        // The page's own scripts don't know the secret.
        post(&trek, cx, r#"{"n":"guess","t":"h","v":500}"#);
        post(&trek, cx, r#"{"t":"h","v":500}"#);
        assert_eq!(one(&trek, cx).height, trek_core::artifact::MIN_HEIGHT);
    });
}

#[test]
fn a_card_partly_scrolled_away_shows_only_what_the_transcript_shows() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        trek.window(cx, |window, _| window.resize(size(px(900.), px(560.))));
        answered(&trek, cx, &answer(PAGE), true);
        let card = one(&trek, cx);
        let (body, mask) = (card.body.unwrap(), card.mask.unwrap());
        assert!(body.size.height > mask.size.height || body.top() < mask.top() || body.bottom() > mask.bottom(), "the card should be cut: {body:?} in {mask:?}");
        let placed = card.placed.expect("live");
        assert_eq!(placed.clip, body.intersect(&mask), "the clip is what shows, the body keeps its size");
        assert!(placed.clip.size.height < body.size.height);
        assert!(!placed.round_bottom || placed.clip.bottom() == body.bottom());
    });
}

#[test]
fn trek_overlays_and_other_threads_hide_the_view() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        assert!(one(&trek, cx).live);
        // A Trek surface over the window (the palette, a picker): the view steps aside.
        trek.update(cx, |ws, cx| {
            ws.overlay_open = true;
            cx.notify();
        });
        settle(&trek, cx);
        let card = one(&trek, cx);
        assert!(!card.live && card.placed.is_none(), "{card:?}");
        trek.update(cx, |ws, cx| {
            ws.overlay_open = false;
            cx.notify();
        });
        settle(&trek, cx);
        assert!(one(&trek, cx).live);
        // Another thread: the card isn't drawn, and its view goes with it.
        trek.quiet_thread(cx);
        settle(&trek, cx);
        assert!(cards(&trek, cx).is_empty(), "{:?}", cards(&trek, cx));
    });
}

#[test]
fn a_theme_switch_reaches_the_running_page() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        assert!(log.borrow().scripts.is_empty());
        // Whichever theme the test starts in, the other.
        let dark = cx.update(|cx| gpui_kit::component::ActiveTheme::theme(cx).mode.is_dark());
        let (other, scheme) = if dark { (ThemeChoice::Paper, "color-scheme:light") } else { (ThemeChoice::Night, "color-scheme:dark") };
        cx.update(|cx| crate::apply_theme(other, None, cx));
        settle(&trek, cx);
        let scripts = log.borrow().scripts.clone();
        assert!(scripts.iter().any(|s| s.contains("trek-theme") && s.contains(scheme)), "{scripts:?}");
        assert_eq!(log.borrow().reloads, 0, "restyled in place, not reloaded");
        cx.update(|cx| crate::apply_theme(ThemeChoice::Night, None, cx));
    });
}

#[test]
fn reload_loads_the_document_again() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        let reload = on_screen(&trek, cx, "artifact-reload-").expect("a Reload button");
        trek.click(cx, reload);
        assert_eq!(log.borrow().reloads, 1);
    });
}

#[test]
fn without_a_web_view_the_card_says_so() {
    run(async |cx| {
        fake_web(false);
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        let card = one(&trek, cx);
        assert!(!card.has_view && !card.live);
    });
}

#[test]
fn with_artifacts_off_the_block_is_code_and_agents_arent_told() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open_with(cx, |s| s.general.interactive_artifacts = false);
        answered(&trek, cx, &answer(PAGE), true);
        assert!(cards(&trek, cx).is_empty());
        assert_eq!(log.borrow().made, 0);
        let id = trek.send(cx, "mock:told");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let told = trek.answers(cx, &id);
        assert!(!told.contains(trek_core::artifact::AGENT_INSTRUCTIONS), "{told}");
        assert!(told.contains(trek_core::visualization::AGENT_INSTRUCTIONS), "visualizations stay on");
    });
}

#[test]
fn agents_are_told_about_artifacts_by_default() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:told");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.answers(cx, &id).contains(trek_core::artifact::AGENT_INSTRUCTIONS));
    });
}

#[test]
fn the_mock_answers_with_two_artifacts() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        let id = trek.send(cx, "mock:interactive");
        trek.wait_done(cx, &id, RunState::Idle).await;
        settle(&trek, cx);
        let all = cards(&trek, cx);
        let titles: Vec<_> = all.iter().filter_map(|c| c.title.clone()).collect();
        assert!(titles.iter().any(|t| t == "Desk with a monitor clamp"), "{titles:?}");
    });
}

#[test]
fn a_cards_send_goes_to_its_thread_and_copy_reaches_the_clipboard() {
    // (One send a second per page is `trek_core::artifact`'s `a_page_sends_at_most_once_a_second`.)
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        let id = answered(&trek, cx, &answer(PAGE), true);
        let users = |trek: &Trek, cx: &mut TestAppContext| -> Vec<String> {
            trek.items(cx, &id).into_iter().filter_map(|i| if let trek_core::store::Item::User { text, .. } = i { Some(text) } else { None }).collect()
        };
        let before = users(&trek, cx).len();
        let post = |trek: &Trek, cx: &mut TestAppContext, body: &str| {
            trek.window(cx, |window, cx| crate::artifact::post(window, body, cx));
            settle(trek, cx);
        };
        // Another secret: nothing is sent.
        post(&trek, cx, r#"{"n":"not-the-secret","t":"send","text":"forged"}"#);
        assert_eq!(users(&trek, cx).len(), before);
        // A copy goes to the clipboard as it is.
        post(&trek, cx, r#"{"n":"{nonce}","t":"copy","text":"SELECT 1;\n"}"#);
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("SELECT 1;\n"));
        // With the card's own secret, a send becomes the user's next message in its thread.
        assert!(one(&trek, cx).has_view);
        post(&trek, cx, r#"{"n":"{nonce}","t":"send","text":"  Resolve src/main.rs: keep mine  "}"#);
        let after = users(&trek, cx);
        assert_eq!(after.len(), before + 1, "{after:?}");
        assert_eq!(after.last().map(|t| t.trim()), Some("Resolve src/main.rs: keep mine"));
    });
}
