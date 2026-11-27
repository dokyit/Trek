//! Interactive artifacts in answers: a finished `trek-html` block runs in a card (a stand-in web
//! view here), an unfinished one waits; the page sets its card's height within bounds, through
//! messages only its bootstrap can send; the view shows only where the card can be seen, and steps
//! aside for Trek's overlays and other threads; a theme switch reaches the page; the setting turns
//! it all off, instructions included.

mod common;

use common::{Trek, open, open_with, run};
use gpui_kit::{TestAppContext, px, size};
use trek_agents::AgentEvent;
use trek_app::artifact::{ArtifactReport, fake_web, placements};
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
        events.extend([
            AgentEvent::TextDone(text.into()),
            AgentEvent::TurnComplete { error: None },
        ]);
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
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible())
            .find_map(|s| match s.path().last()? {
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
        assert!(
            on_screen(&trek, cx, "artifact-building-").is_some(),
            "a Building… card"
        );
        assert_eq!(
            log.borrow().made,
            0,
            "no view for a block still being written"
        );
        assert!(cards(&trek, cx).is_empty());

        // Finished: a card, a view, placed where the card is.
        let text = answer(PAGE);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::TextDone(text.clone()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        trek.wait_done(cx, &id, RunState::Idle).await;
        settle(&trek, cx);
        assert_eq!(log.borrow().made, 1);
        let card = one(&trek, cx);
        assert_eq!(card.title.as_deref(), Some("Desk & clamp"));
        assert!(card.has_view && card.live, "{card:?}");
        let (placed, body, mask) = (card.placed.unwrap(), card.body.unwrap(), card.mask.unwrap());
        assert_eq!(placed.body, body);
        assert!(
            mask.contains(&placed.clip.origin)
                && placed.clip.bottom() <= mask.bottom()
                && placed.clip.top() >= body.top(),
            "{placed:?} in {mask:?}"
        );
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
            trek.window(cx, |window, cx| trek_app::artifact::post(window, body, cx));
            settle(trek, cx);
        };
        post(&trek, cx, r#"{"n":"{nonce}","t":"h","v":480}"#);
        assert_eq!(one(&trek, cx).height, 480.);
        let body = one(&trek, cx).body.unwrap();
        assert_eq!(
            body.size.height,
            px(480.),
            "the card is as tall as the page says"
        );
        post(&trek, cx, r#"{"n":"{nonce}","t":"h","v":9000}"#);
        assert_eq!(
            one(&trek, cx).height,
            trek_core::artifact::MAX_HEIGHT,
            "it scrolls inside past the most"
        );
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
        cx.simulate_window_resize(trek.window, size(px(900.), px(560.)));
        answered(&trek, cx, &answer(PAGE), true);
        let card = one(&trek, cx);
        let (body, mask) = (card.body.unwrap(), card.mask.unwrap());
        assert!(
            body.size.height > mask.size.height
                || body.top() < mask.top()
                || body.bottom() > mask.bottom(),
            "the card should be cut: {body:?} in {mask:?}"
        );
        let placed = card.placed.expect("live");
        assert_eq!(
            placed.clip,
            body.intersect(&mask),
            "the clip is what shows, the body keeps its size"
        );
        assert!(placed.clip.size.height < body.size.height);
        assert!(!placed.round_bottom || placed.clip.bottom() == body.bottom());
    });
}

#[test]
fn a_crowd_of_cards_near_the_screen_makes_each_view_once_not_every_frame() {
    // A thread of many players and pages (a video watched again and again): more cards on or
    // next to the screen than a window keeps views. The ones on screen and the nearest keep
    // theirs; the rest used to be made, taken away in the same frame for being past the most,
    // and made again in the next: a new web view every frame, which flashed white and black over
    // the cards as they scrolled past.
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        cx.simulate_window_resize(trek.window, size(px(1400.), px(1300.)));
        let blocks: Vec<String> = (0..12)
            .map(|i| {
                format!(
                    "Page {i}:\n\n```trek-html\n<!doctype html><html><body><p>{i}</p></body></html>\n```\n"
                )
            })
            .collect();
        answered(&trek, cx, &blocks.join("\n"), true);
        settle(&trek, cx);
        let showing = cards(&trek, cx)
            .iter()
            .filter(|c| {
                c.body
                    .zip(c.mask)
                    .is_some_and(|(b, m)| b.intersect(&m).size.height > px(0.))
            })
            .count();
        assert!(showing >= 2, "{showing} on screen");
        let with_view = cards(&trek, cx).iter().filter(|c| c.has_view).count();
        assert!(
            (showing..=showing.max(trek_app::artifact::MAX_LIVE)).contains(&with_view),
            "{with_view} views for {showing} showing"
        );
        let made = log.borrow().made;
        for _ in 0..30 {
            trek.render(cx);
        }
        assert_eq!(
            log.borrow().made,
            made,
            "views made again and again while nothing moved"
        );
        assert_eq!(
            cards(&trek, cx).iter().filter(|c| c.has_view).count(),
            with_view
        );
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

/// The composer has the keyboard.
fn composer_focused(trek: &Trek, cx: &mut TestAppContext) -> bool {
    let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
    trek.window(cx, |window, cx| composer.read(cx).is_focused(window, cx))
}

#[test]
fn a_page_with_the_keys_gives_them_back_before_its_card_hides_or_goes() {
    // A page clicked into (a watched video played) had the keys. Its view hidden or gone while it
    // still had them, the owner's Mac left them with the window: every key beeped, the composer
    // took no text even when clicked, and only dictation could write a message.
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        assert!(one(&trek, cx).live);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.focus(window, cx))
        });
        let take = |trek: &Trek, cx: &mut TestAppContext| {
            trek.window(cx, |window, cx| {
                trek_app::artifact::page_takes_keys(window, cx)
            })
            .expect("a live page");
            settle(trek, cx);
            assert!(log.borrow().page_has_keys && one(trek, cx).has_keys);
        };
        take(&trek, cx);
        assert!(!composer_focused(&trek, cx), "the card has GPUI's focus");
        // Hidden while it has them (a Trek surface over it; covered, or scrolled out of view).
        trek.update(cx, |ws, cx| {
            ws.overlay_open = true;
            cx.notify();
        });
        settle(&trek, cx);
        assert!(!one(&trek, cx).live);
        assert_eq!(log.borrow().hidden_with_keys, 0, "hidden with the keys");
        assert!(!log.borrow().page_has_keys && !one(&trek, cx).has_keys);
        assert!(composer_focused(&trek, cx), "back where they were");
        trek.update(cx, |ws, cx| {
            ws.overlay_open = false;
            cx.notify();
        });
        settle(&trek, cx);
        assert!(one(&trek, cx).live);
        // Gone while it has them: another thread on screen, and the card with its view goes.
        take(&trek, cx);
        trek.quiet_thread(cx);
        settle(&trek, cx);
        assert!(cards(&trek, cx).is_empty());
        assert_eq!(log.borrow().gone_with_keys, 0, "went with the keys");
        assert!(!log.borrow().page_has_keys);
        assert!(log.borrow().keys_back >= 2);
        // What's typed next reaches the composer.
        assert!(composer_focused(&trek, cx), "back where they were");
        trek.type_text(cx, "typed again");
        assert_eq!(trek.composer_text(cx), "typed again");
    });
}

#[test]
fn keys_come_back_when_a_page_that_had_them_scrolls_away_and_its_view_is_let_go() {
    // The owner's case: he played a video inline, the thread went on, and the player scrolled far
    // up while it still had the keys. Its view was hidden, then let go once more cards came than
    // are kept live; the keys stayed with the window and every key beeped.
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        cx.simulate_window_resize(trek.window, size(px(900.), px(560.)));
        let id = answered(&trek, cx, &answer(PAGE), true);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.focus(window, cx))
        });
        trek.window(cx, |window, cx| {
            trek_app::artifact::page_takes_keys(window, cx)
        })
        .expect("a live page");
        settle(&trek, cx);
        assert!(log.borrow().page_has_keys && one(&trek, cx).has_keys);
        // More answers, each with a page of its own, push it up and out of view, then past the
        // views a window keeps.
        for n in 0..6 {
            let text = answer(&PAGE.replace("Desk", &format!("Desk {n}")));
            trek.update(cx, |ws, cx| {
                ws.apply_events(
                    &id,
                    vec![
                        AgentEvent::TextDelta(text.clone()),
                        AgentEvent::TextDone(text),
                        AgentEvent::TurnComplete { error: None },
                    ],
                    cx,
                )
            });
            settle(&trek, cx);
        }
        let all = cards(&trek, cx);
        assert!(
            !all.iter()
                .any(|c| c.live && c.title.as_deref() == Some("Desk & clamp")),
            "the first card scrolled away: {all:?}"
        );
        assert!(!all.iter().any(|c| c.has_keys), "{all:?}");
        let log = log.borrow();
        assert_eq!(log.hidden_with_keys, 0, "hidden with the keys");
        assert_eq!(log.gone_with_keys, 0, "let go with the keys");
        assert!(!log.page_has_keys && log.keys_back >= 1);
        drop(log);
        // Clicked into, the composer takes what's typed (the keys are Trek's again).
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| c.focus(window, cx))
        });
        trek.type_text(cx, "still typing");
        assert_eq!(trek.composer_text(cx), "still typing");
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
        let (other, scheme) = if dark {
            (ThemeChoice::Paper, "color-scheme:light")
        } else {
            (ThemeChoice::Night, "color-scheme:dark")
        };
        cx.update(|cx| trek_app::apply_theme(other, None, cx));
        settle(&trek, cx);
        let scripts = log.borrow().scripts.clone();
        assert!(
            scripts
                .iter()
                .any(|s| s.contains("trek-theme") && s.contains(scheme)),
            "{scripts:?}"
        );
        assert_eq!(log.borrow().reloads, 0, "restyled in place, not reloaded");
        cx.update(|cx| trek_app::apply_theme(ThemeChoice::Night, None, cx));
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
        assert!(
            !told.contains(trek_core::artifact::AGENT_INSTRUCTIONS),
            "{told}"
        );
        assert!(
            told.contains(trek_core::visualization::AGENT_INSTRUCTIONS),
            "visualizations stay on"
        );
    });
}

#[test]
fn agents_are_told_about_artifacts_by_default() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:told");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(
            trek.answers(cx, &id)
                .contains(trek_core::artifact::AGENT_INSTRUCTIONS)
        );
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
        assert!(
            titles.iter().any(|t| t == "Desk with a monitor clamp"),
            "{titles:?}"
        );
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
            trek.items(cx, &id)
                .into_iter()
                .filter_map(|i| {
                    if let trek_core::store::Item::User { text, .. } = i {
                        Some(text)
                    } else {
                        None
                    }
                })
                .collect()
        };
        let before = users(&trek, cx).len();
        let post = |trek: &Trek, cx: &mut TestAppContext, body: &str| {
            trek.window(cx, |window, cx| trek_app::artifact::post(window, body, cx));
            settle(trek, cx);
        };
        // Another secret: nothing is sent.
        post(
            &trek,
            cx,
            r#"{"n":"not-the-secret","t":"send","text":"forged"}"#,
        );
        assert_eq!(users(&trek, cx).len(), before);
        // A copy goes to the clipboard as it is.
        post(
            &trek,
            cx,
            r#"{"n":"{nonce}","t":"copy","text":"SELECT 1;\n"}"#,
        );
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("SELECT 1;\n")
        );
        // With the card's own secret, a send becomes the user's next message in its thread.
        assert!(one(&trek, cx).has_view);
        post(
            &trek,
            cx,
            r#"{"n":"{nonce}","t":"send","text":"  Resolve src/main.rs: keep mine  "}"#,
        );
        let after = users(&trek, cx);
        assert_eq!(after.len(), before + 1, "{after:?}");
        assert_eq!(
            after.last().map(|t| t.trim()),
            Some("Resolve src/main.rs: keep mine")
        );
    });
}

#[test]
fn a_cards_send_waits_while_its_thread_asks_for_a_secret() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        let id = answered(&trek, cx, &answer(PAGE), true);
        let (tx, _rx) = async_channel::bounded(1);
        trek.update(cx, |ws, cx| {
            ws.ask_for_secret(&id, "GITHUB_TOKEN", "To open the pull request.", tx, cx)
        });
        assert!(one(&trek, cx).has_view);
        trek.window(cx, |window, cx| {
            trek_app::artifact::post(
                window,
                r#"{"n":"{nonce}","t":"send","text":"ghp_typed-in-the-page"}"#,
                cx,
            )
        });
        settle(&trek, cx);
        let items = format!("{:?}", trek.items(cx, &id));
        assert!(!items.contains("ghp_typed-in-the-page"), "{items}");
    });
}

/// The hole this closes: a page's `window.open` (or a link it clicked itself) went straight to
/// the user's browser, with whatever the page put in the address. Now the page's asking is one
/// thing and the user's click another, and the click is the app's own to see.
#[test]
fn a_page_opens_a_link_only_on_a_click_the_app_saw() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        let wants = |trek: &Trek, cx: &mut TestAppContext, url: &str, engine: Option<bool>| {
            trek.window(cx, |window, cx| {
                trek_app::artifact::wants(window, url, engine, cx)
            });
        };
        // As the page loads, with no click anywhere: a window, with what the agent read in its
        // address.
        wants(&trek, cx, "https://evil.example.org/?secret=hunter2", None);
        assert_eq!(cx.opened_url(), None, "no click, no window");
        // The page's own word that there was a click is no click: no message opens a link,
        // even one with the secret only Trek's script has.
        trek.window(cx, |window, cx| {
            trek_app::artifact::post(
                window,
                r#"{"n":"{nonce}","t":"open","u":"https://evil.example.org/said-so"}"#,
                cx,
            )
        });
        assert_eq!(cx.opened_url(), None);
        // The user clicks a link in the page: the system's click, then the page's asking.
        log.borrow_mut().click();
        wants(&trek, cx, "https://threejs.org/docs", None);
        // threejs.org is no host a page may load from: a click alone doesn't open it, the
        // card's own confirm does.
        assert_eq!(cx.opened_url(), None);
        settle(&trek, cx);
        let open = on_screen(&trek, cx, "artifact-link-open-").expect("the card asks");
        trek.click(cx, open);
        settle(&trek, cx);
        assert_eq!(cx.opened_url().as_deref(), Some("https://threejs.org/docs"));
        // One click opens one address: a second riding on it goes nowhere.
        wants(&trek, cx, "https://evil.example.org/rides-along", None);
        assert_eq!(cx.opened_url().as_deref(), Some("https://threejs.org/docs"));
        // Where the engine vouches for each request (Windows): its "no" is no, a "yes" opens
        // web and mail addresses only, and no faster than one a second.
        wants(&trek, cx, "https://example.org/too-soon", Some(true));
        assert_eq!(cx.opened_url().as_deref(), Some("https://threejs.org/docs"));
        std::thread::sleep(std::time::Duration::from_millis(1050));
        wants(&trek, cx, "https://evil.example.org/by-script", Some(false));
        wants(&trek, cx, "file:///etc/passwd", Some(true));
        assert_eq!(cx.opened_url().as_deref(), Some("https://threejs.org/docs"));
        wants(&trek, cx, "https://example.org/clicked", Some(true));
        // The engine's gesture vouches for a click, not for the host: the card confirms first.
        assert_eq!(cx.opened_url().as_deref(), Some("https://threejs.org/docs"));
        settle(&trek, cx);
        let open = on_screen(&trek, cx, "artifact-link-open-").expect("the card asks");
        trek.click(cx, open);
        settle(&trek, cx);
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://example.org/clicked")
        );
    });
}

/// A page starts with the open sources and no other host. What its policy stops, the card asks
/// about; the user's answer is for exactly the hosts named.
#[test]
fn a_host_the_policy_stopped_is_the_users_to_allow() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        answered(&trek, cx, &answer(PAGE), true);
        let post = |trek: &Trek, cx: &mut TestAppContext, body: &str| {
            trek.window(cx, |window, cx| trek_app::artifact::post(window, body, cx));
            settle(trek, cx);
        };
        let hosts = |trek: &Trek, cx: &mut TestAppContext| {
            trek.update(cx, |ws, _| ws.settings.general.artifact_hosts.clone())
        };
        let policy = |log: &std::rc::Rc<std::cell::RefCell<trek_app::artifact::FakeLog>>| {
            log.borrow().policies.last().cloned().unwrap_or_default()
        };
        assert_eq!(policy(&log), trek_core::artifact::net::page_csp(&[]));
        assert!(!policy(&log).contains("jsdelivr"));
        assert!(on_screen(&trek, cx, "artifact-allow-once-").is_none());

        // The engine stopped a script from jsDelivr.
        post(
            &trek,
            cx,
            r#"{"n":"{nonce}","t":"blocked","u":"https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js"}"#,
        );
        // A report without the secret is the page's own, and an address on this machine is
        // nobody's to allow: neither is asked about.
        post(
            &trek,
            cx,
            r#"{"t":"blocked","u":"https://evil.example.org/x"}"#,
        );
        post(
            &trek,
            cx,
            r#"{"n":"{nonce}","t":"blocked","u":"https://127.0.0.1:8443/x"}"#,
        );
        let once = on_screen(&trek, cx, "artifact-allow-once-").expect("the card asks");
        assert_eq!(log.borrow().reloads, 0, "nothing loads until the user says");
        // A click this soon after the drawn hosts changed is ignored: the strip may be one
        // redraw behind what the queue holds.
        std::thread::sleep(std::time::Duration::from_millis(350));
        trek.click(cx, once);
        settle(&trek, cx);
        assert_eq!(log.borrow().reloads, 1, "allowed, the page loads again");
        let allowed = policy(&log);
        assert_eq!(
            allowed,
            trek_core::artifact::net::page_csp(&["cdn.jsdelivr.net".to_string()])
        );
        assert!(!allowed.contains("evil") && !allowed.contains("127.0.0.1"));
        assert!(hosts(&trek, cx).is_empty(), "once is for this page alone");
        assert!(on_screen(&trek, cx, "artifact-allow-once-").is_none());

        // Another host, allowed for good: it's kept in settings.
        post(
            &trek,
            cx,
            r#"{"n":"{nonce}","t":"blocked","u":"https://api.github.com/repos/x/y"}"#,
        );
        let always = on_screen(&trek, cx, "artifact-allow-always-").expect("the card asks");
        std::thread::sleep(std::time::Duration::from_millis(350));
        trek.click(cx, always);
        settle(&trek, cx);
        assert_eq!(hosts(&trek, cx), ["api.github.com"]);
        assert_eq!(log.borrow().reloads, 2);
        assert!(
            policy(&log).contains(" https://api.github.com https://cdn.jsdelivr.net;")
                || policy(&log).contains(" https://cdn.jsdelivr.net https://api.github.com;"),
            "{}",
            policy(&log)
        );

        // A third, refused: nothing changes, and the card doesn't ask about it again.
        let tracker = r#"{"n":"{nonce}","t":"blocked","u":"https://tracker.example.org/p.gif"}"#;
        post(&trek, cx, tracker);
        let deny = on_screen(&trek, cx, "artifact-deny-").expect("the card asks");
        std::thread::sleep(std::time::Duration::from_millis(350));
        trek.click(cx, deny);
        settle(&trek, cx);
        assert_eq!(log.borrow().reloads, 2);
        assert!(!policy(&log).contains("tracker"));
        post(&trek, cx, tracker);
        assert!(on_screen(&trek, cx, "artifact-deny-").is_none());
        assert_eq!(hosts(&trek, cx), ["api.github.com"]);
    });
}
