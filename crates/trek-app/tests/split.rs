//! Two threads side by side in the main window: each pane a whole thread of its own, one with the
//! focus, and everything that means "the thread on screen" meaning the focused pane's.

mod common;

use common::{Trek, launch, open, run, toasts};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, point, px, size};
use std::path::PathBuf;
use trek_app::composer::Composer;
use trek_app::workspace::split::Pane;
use trek_app::workspace::{Route, Scope};
use trek_core::RunState;
use trek_core::store::Store;

/// Over to a new draft.
fn to_draft(trek: &Trek, cx: &mut TestAppContext) {
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| {
        ws.navigate(
            Route::Draft {
                project: Some(project),
            },
            cx,
        )
    });
    trek.render(cx);
}

/// A finished thread on `text`, from a new draft.
async fn thread(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    to_draft(trek, cx);
    let id = trek.send(cx, text);
    trek.wait_done(cx, &id, RunState::Idle).await;
    to_draft(trek, cx);
    id
}

/// The sidebar row of thread `id`, whichever kind it's drawn as.
fn row(trek: &Trek, cx: &mut TestAppContext, id: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    ["card", "live-line", "line"]
        .into_iter()
        .find_map(|kind| {
            let name = gpui_kit::SharedString::from(format!("{kind}-{id}"));
            trek.bounds(cx, name)
        })
        .expect("the thread's row")
        .center()
}

/// Threads `a` and `b`, side by side, with the focus on `b`.
async fn split(trek: &Trek, cx: &mut TestAppContext) -> (String, String) {
    let a = thread(trek, cx, "first thread").await;
    let b = thread(trek, cx, "second thread").await;
    open_beside(trek, cx, &a, &b);
    (a, b)
}

/// `a` on screen, `b` opened beside it.
fn open_beside(trek: &Trek, cx: &mut TestAppContext, a: &str, b: &str) {
    trek.update(cx, |ws, cx| {
        ws.navigate(Route::Thread(a.to_string()), cx);
        ws.open_beside(b, cx);
    });
    trek.render(cx);
}

fn right_composer(trek: &Trek, cx: &TestAppContext) -> Entity<Composer> {
    cx.read(|cx| {
        trek.root
            .read(cx)
            .beside
            .as_ref()
            .expect("a right pane")
            .composer
            .clone()
    })
}

fn left_text(trek: &Trek, cx: &TestAppContext) -> String {
    trek.composer_text(cx)
}

fn right_text(trek: &Trek, cx: &TestAppContext) -> String {
    let composer = right_composer(trek, cx);
    cx.read(|cx| composer.read(cx).text(cx))
}

fn focused(trek: &Trek, cx: &TestAppContext) -> Pane {
    trek.read(cx, |ws, _| ws.split.as_ref().expect("a split").focus)
}

#[test]
fn a_thread_never_split_has_none_of_it() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "one thread");
        trek.wait_done(cx, &a, RunState::Idle).await;
        trek.render(cx);
        assert!(cx.read(|cx| trek.root.read(cx).beside.is_none()));
        for id in [
            "split-view",
            "split-pane-left",
            "split-divider",
            "split-drop",
        ] {
            assert!(!trek.visible(cx, id), "{id}");
        }
        assert_eq!(trek.read(cx, |ws, _| ws.focused_scope()), Scope::Main);
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .thread_id_in(&Scope::Beside)
                .map(str::to_string)),
            None
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.layout.split.clone()),
            None
        );
    });
}

#[test]
fn each_pane_shows_its_own_thread_and_the_route_is_the_focused_one() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        for id in [
            "split-view",
            "split-pane-left",
            "split-pane-right",
            "split-divider",
        ] {
            assert!(trek.visible(cx, id), "{id}");
        }
        // The pane opened has the focus: the route, the sidebar's selection and the scope that
        // the window's commands use.
        assert_eq!(focused(&trek, cx), Pane::Right);
        assert_eq!(trek.thread_id(cx), b);
        assert!(trek.visible(cx, "split-focus-right"));
        assert!(!trek.visible(cx, "split-focus-left"));
        trek.read(cx, |ws, _| {
            assert_eq!(ws.thread_id_in(&Scope::Main), Some(a.as_str()));
            assert_eq!(ws.thread_id_in(&Scope::Beside), Some(b.as_str()));
            assert_eq!(ws.focused_thread(), Some(b.as_str()));
            assert_eq!(ws.focused_scope(), Scope::Beside);
            assert_eq!(ws.current_thread().map(|t| t.id.as_str()), Some(b.as_str()));
            assert_eq!(ws.shown_in(&a), Some(Scope::Main));
            assert_eq!(ws.shown_in(&b), Some(Scope::Beside));
        });
        // A press in the other pane moves the focus and the route with it.
        trek.click(cx, "split-pane-left");
        assert_eq!(focused(&trek, cx), Pane::Left);
        assert_eq!(trek.thread_id(cx), a);
        assert_eq!(trek.read(cx, |ws, _| ws.focused_scope()), Scope::Main);
        trek.render(cx);
        assert!(trek.visible(cx, "split-focus-left"));
        assert!(!trek.visible(cx, "split-focus-right"));
    });
}

#[test]
fn typing_goes_to_the_focused_pane_and_each_keeps_its_own_draft() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.type_text(cx, "for the right");
        assert_eq!(right_text(&trek, cx), "for the right");
        assert_eq!(left_text(&trek, cx), "");
        trek.click(cx, "split-pane-left");
        let left = cx.read(|cx| trek.root.read(cx).composer.clone());
        let right = right_composer(&trek, cx);
        let focus = trek.window(cx, |window, cx| {
            (
                left.read(cx).is_focused(window, cx),
                right.read(cx).is_focused(window, cx),
                window.focused(cx).is_some(),
            )
        });
        assert_eq!(focus, (true, false, true), "the keyboard came across");
        trek.type_text(cx, "for the left");
        assert_eq!(left_text(&trek, cx), "for the left");
        assert_eq!(right_text(&trek, cx), "for the right");
        // A message to one doesn't go to the other, with the focus on either.
        trek.press(cx, "enter");
        assert_eq!(left_text(&trek, cx), "");
        assert_eq!(right_text(&trek, cx), "for the right");
        trek.wait_done(cx, &a, RunState::Idle).await;
        assert!(trek.answers(cx, &a).contains("for the left") || trek.items(cx, &a).len() > 2);
        assert!(!format!("{:?}", trek.items(cx, &b)).contains("for the left"));
    });
}

#[test]
fn the_keyboard_and_the_focus_stay_together() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        let left = cx.read(|cx| trek.root.read(cx).composer.clone());
        let right = right_composer(&trek, cx);
        // Focus events reach a window in front.
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        // The keyboard landing in the other composer, however it got there, takes the focus.
        trek.window(cx, |window, cx| {
            left.update(cx, |c, cx| c.focus(window, cx))
        });
        trek.render(cx);
        assert_eq!(trek.thread_id(cx), a);
        assert_eq!(focused(&trek, cx), Pane::Left);
        // A composer focused while the split waits behind a draft leaves it alone.
        to_draft(&trek, cx);
        trek.window(cx, |window, cx| {
            right.update(cx, |c, cx| c.focus(window, cx))
        });
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| matches!(ws.route, Route::Draft { .. })));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        assert_eq!(focused(&trek, cx), Pane::Right);
        // Pressing the other pane's own composer moves both.
        let target = trek.bounds(cx, "split-pane-left").unwrap();
        let bottom = point(target.center().x, target.bottom() - px(40.));
        trek.window(cx, |window, cx| {
            window.drag(bottom, bottom, cx);
        });
        cx.run_until_parked();
        assert_eq!(focused(&trek, cx), Pane::Left);
        let on_left = trek.window(cx, |window, cx| left.read(cx).is_focused(window, cx));
        assert!(on_left);
    });
}

#[test]
fn attachments_go_to_the_focused_pane() {
    run(async |cx| {
        let trek = open(cx);
        let _ = split(&trek, cx).await;
        let image = PathBuf::from("/tmp/split-test.png");
        let path = image.clone();
        trek.update(cx, |_, cx| {
            cx.emit(trek_app::workspace::WorkspaceEvent::AttachImage(path))
        });
        let right = right_composer(&trek, cx);
        assert_eq!(
            cx.read(|cx| right.read(cx).attached().0.clone()),
            vec![image]
        );
        let left = cx.read(|cx| trek.root.read(cx).composer.clone());
        assert!(cx.read(|cx| left.read(cx).attached().0.is_empty()));
    });
}

#[test]
fn a_draft_follows_its_thread_from_pane_to_pane() {
    run(async |cx| {
        let trek = open(cx);
        let a = thread(&trek, cx, "first thread").await;
        let b = thread(&trek, cx, "second thread").await;
        let c = thread(&trek, cx, "third thread").await;
        // Typed for `b` in the main window, before there's any split.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        trek.type_text(cx, "kept for b");
        open_beside(&trek, cx, &a, &b);
        assert_eq!(right_text(&trek, cx), "kept for b");
        assert_eq!(left_text(&trek, cx), "");
        // `c` takes the right pane and `b` puts its message away...
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(c.clone()), cx));
        assert_eq!(right_text(&trek, cx), "");
        // ...to find it in the left pane.
        trek.click(cx, "split-pane-left");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        assert_eq!(left_text(&trek, cx), "kept for b");
    });
}

#[test]
fn closing_the_right_pane_gives_the_left_the_window() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.click(cx, "split-pane-left");
        trek.type_text(cx, "left's");
        let scrolled = cx.read(|cx| trek.root.read(cx).thread_view.entity_id());
        trek.click(cx, "split-close-right");
        trek.render(cx);
        assert_eq!(trek.thread_id(cx), a);
        assert_eq!(left_text(&trek, cx), "left's");
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert!(cx.read(|cx| trek.root.read(cx).beside.is_none()));
        assert!(!trek.visible(cx, "split-view"));
        assert!(!trek.visible(cx, "split-pane-left"));
        assert_eq!(
            cx.read(|cx| trek.root.read(cx).thread_view.entity_id()),
            scrolled
        );
        // The thread closed keeps its tab and goes on as it was.
        assert!(trek.read(cx, |ws, _| ws.tabs.contains(&b)));
        assert!(trek.read(cx, |ws, _| ws.thread(&b).is_some()));
    });
}

#[test]
fn closing_the_left_pane_moves_the_right_thread_with_its_message() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.type_text(cx, "right's");
        // Its place in the transcript goes with it: the scroller that holds it does.
        let scroller = cx.read(|cx| {
            let pane = trek.root.read(cx).beside.as_ref().expect("a right pane");
            pane.thread_view.read(cx).scroller().entity_id()
        });
        trek.click(cx, "split-close-left");
        trek.render(cx);
        assert_eq!(
            cx.read(|cx| trek
                .root
                .read(cx)
                .thread_view
                .read(cx)
                .scroller()
                .entity_id()),
            scroller
        );
        assert_eq!(trek.thread_id(cx), b);
        assert_eq!(left_text(&trek, cx), "right's");
        assert!(cx.read(|cx| trek.root.read(cx).beside.is_none()));
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .thread_id_in(&Scope::Main)
                .map(str::to_string)),
            Some(b.clone())
        );
        // Typing goes on where it was: the composer has the keys.
        trek.type_text(cx, " more");
        assert_eq!(left_text(&trek, cx), "right's more");
        // And the thread closed has its own message back if it had one.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a), cx));
        assert_eq!(left_text(&trek, cx), "");
    });
}

#[test]
fn a_thread_is_never_in_both_panes() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        let c = thread(&trek, cx, "third thread").await;
        // The draft waited in front of the split; `c` took the pane with the focus.
        trek.update(cx, |ws, cx| ws.close_focused_pane(cx));
        open_beside(&trek, cx, &a, &b);
        // Already open: it takes the focus, wherever asked from.
        trek.update(cx, |ws, cx| ws.open_beside(&a, cx));
        assert_eq!(focused(&trek, cx), Pane::Left);
        assert_eq!(trek.thread_id(cx), a);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        assert_eq!(focused(&trek, cx), Pane::Right);
        trek.update(cx, |ws, cx| ws.open_in_pane(Pane::Left, &b, cx));
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (a.as_str(), b.as_str())
            );
        });
        // Beside itself is no split at all.
        trek.update(cx, |ws, cx| ws.close_focused_pane(cx));
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        trek.update(cx, |ws, cx| ws.open_beside(&a, cx));
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        // A thread not in the split takes the pane with the focus, as a sidebar click.
        open_beside(&trek, cx, &a, &b);
        trek.click(cx, "split-pane-left");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(c.clone()), cx));
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (c.as_str(), b.as_str())
            );
            assert_eq!(ws.route, Route::Thread(c.clone()));
        });
        // "Open beside" takes the pane beside the focus.
        trek.update(cx, |ws, cx| ws.open_beside(&a, cx));
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (c.as_str(), a.as_str())
            );
            assert_eq!(s.focus, Pane::Right);
        });
    });
}

#[test]
fn a_thread_visible_in_either_pane_is_seen() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        // A turn ends in the pane without the focus: the user is looking at it.
        let left = cx.read(|cx| trek.root.read(cx).composer.clone());
        trek.window(cx, |window, cx| {
            left.update(cx, |c, cx| {
                c.set_text("a turn behind the focus", window, cx)
            });
            left.update(cx, |c, cx| c.send_now(window, cx));
        });
        cx.run_until_parked();
        assert_eq!(trek.thread_id(cx), b, "the focus didn't move");
        trek.wait_done(cx, &a, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| !ws.thread(&a).unwrap().is_unseen()));
        assert!(trek.read(cx, |ws, _| ws.on_screen(&a) && ws.on_screen(&b)));
        // Closed, it's as any thread off screen: its next answer is unread.
        trek.update(cx, |ws, cx| ws.close_pane(Pane::Left, cx));
        assert!(trek.read(cx, |ws, _| !ws.on_screen(&a)));
        trek.update(cx, |ws, cx| ws.send_to(&a, "off screen".into(), vec![], cx));
        trek.wait_done(cx, &a, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.thread(&a).unwrap().is_unseen()));
    });
}

#[test]
fn streaming_in_one_pane_leaves_the_other_alone() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        let rendered = |cx: &mut TestAppContext, into: &str| {
            cx.run_until_parked();
            trek_app::test_support::take_renders();
            for n in 0..40 {
                let delta = format!("A long answer, batch {n}, with enough words to wrap. ");
                trek.ws.update(cx, |ws, cx| {
                    ws.apply_events(into, vec![trek_agents::AgentEvent::TextDelta(delta)], cx)
                });
                cx.run_until_parked();
            }
            let counts = trek_app::test_support::take_renders();
            (
                counts.get("ThreadView").copied().unwrap_or(0),
                counts.get("ThreadView:beside").copied().unwrap_or(0),
            )
        };
        let (left, right) = rendered(cx, &b);
        assert_eq!(left, 0, "the left pane redrew for the right one's answer");
        assert!(right > 0, "the right pane drew its own answer");
        let (left, right) = rendered(cx, &a);
        assert_eq!(right, 0, "the right pane redrew for the left one's answer");
        assert!(left > 0, "the left pane drew its own answer");
    });
}

#[test]
fn commands_act_on_the_focused_pane() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        // Model and access are each pane's: asked of the focus, set on the focus.
        trek.update(cx, |ws, cx| {
            let mut prefs = ws.prefs();
            prefs.effort = trek_core::Effort::Low;
            ws.set_prefs(prefs, cx);
        });
        let effort = |trek: &Trek, cx: &TestAppContext, id: &str| {
            trek.read(cx, |ws, _| ws.thread(id).unwrap().effort)
        };
        assert_eq!(effort(&trek, cx, &b), trek_core::Effort::Low);
        assert_ne!(effort(&trek, cx, &a), trek_core::Effort::Low);
        // ⌘E settles the focused pane's thread, ⌘. and its neighbours go by the same route.
        trek.press(cx, "cmd-e");
        assert!(trek.read(cx, |ws, _| ws.thread(&b).unwrap().settled_at.is_some()));
        assert!(trek.read(cx, |ws, _| ws.thread(&a).unwrap().settled_at.is_none()));
        // A pane sends to its own thread whichever has the focus.
        trek.update(cx, |ws, cx| {
            ws.send_in(&Scope::Main, "to the left".into(), vec![], cx)
        });
        trek.wait_done(cx, &a, RunState::Idle).await;
        assert!(format!("{:?}", trek.items(cx, &a)).contains("to the left"));
        assert!(!format!("{:?}", trek.items(cx, &b)).contains("to the left"));
        assert_eq!(trek.thread_id(cx), b);
        // ⌘W closes the pane, not its tab.
        trek.press(cx, "cmd-w");
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert_eq!(trek.thread_id(cx), a);
        assert!(trek.read(cx, |ws, _| ws.tabs.contains(&b)));
    });
}

#[test]
fn the_shortcut_picks_a_thread_then_crosses_the_split() {
    run(async |cx| {
        let trek = open(cx);
        let a = thread(&trek, cx, "first thread").await;
        let b = thread(&trek, cx, "second thread").await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        trek.render(cx);
        trek.press(cx, "cmd-\\");
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        assert!(cx.read(|cx| palette.read(cx).open && palette.read(cx).beside_only));
        let rows = cx.read(|cx| palette.read(cx).entries(cx));
        assert_eq!(rows.len(), 1, "every thread but the one on screen");
        trek.render(cx);
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(cx.read(|cx| !palette.read(cx).open));
        assert_eq!(focused(&trek, cx), Pane::Right);
        assert_eq!(trek.thread_id(cx), b);
        trek.press(cx, "cmd-\\");
        assert_eq!(trek.thread_id(cx), a);
        // The keys came with it.
        trek.type_text(cx, "typed");
        assert_eq!(left_text(&trek, cx), "typed");
        trek.press(cx, "cmd-\\");
        assert_eq!(trek.thread_id(cx), b);
    });
}

#[test]
fn the_palette_offers_the_split_commands() {
    run(async |cx| {
        let trek = open(cx);
        let a = thread(&trek, cx, "first thread").await;
        let b = thread(&trek, cx, "second thread").await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        let labels = |cx: &TestAppContext| -> Vec<String> {
            cx.read(|cx| {
                palette
                    .read(cx)
                    .entries(cx)
                    .into_iter()
                    .map(|e| e.label.to_string())
                    .collect()
            })
        };
        assert!(
            labels(cx)
                .iter()
                .any(|l| l == "Open a thread beside this one")
        );
        open_beside(&trek, cx, &a, &b);
        let on_split = labels(cx);
        assert!(on_split.iter().any(|l| l == "Focus the other pane"));
        assert!(on_split.iter().any(|l| l == "Close this pane"));
        assert!(
            !on_split
                .iter()
                .any(|l| l == "Open a thread beside this one")
        );
    });
}

#[test]
fn the_split_waits_behind_a_draft_and_a_new_thread_takes_the_focused_pane() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.type_text(cx, "kept");
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        trek.render(cx);
        // Full width, with the split waiting.
        assert!(!trek.visible(cx, "split-view"));
        assert!(trek.read(cx, |ws, _| ws.split.is_some() && ws.split_live().is_none()));
        assert_eq!(trek.read(cx, |ws, _| ws.focused_scope()), Scope::Main);
        // Sent, the new thread takes the pane that had the focus.
        let c = trek.send(cx, "a new thread");
        trek.wait_done(cx, &c, RunState::Idle).await;
        trek.render(cx);
        assert!(trek.visible(cx, "split-view"));
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (a.as_str(), c.as_str())
            );
        });
        // `b` left the pane with its message.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert_eq!(right_text(&trek, cx), "kept");
    });
}

#[test]
fn no_room_for_two_says_why_and_a_narrowed_window_shows_the_focused_pane() {
    run(async |cx| {
        let trek = open(cx);
        let a = thread(&trek, cx, "first thread").await;
        let b = thread(&trek, cx, "second thread").await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        let shown = toasts(&trek, cx);
        cx.simulate_window_resize(trek.window, size(px(760.), px(700.)));
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.open_beside(&b, cx));
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert!(
            shown
                .borrow()
                .iter()
                .any(|(m, _)| m.starts_with("Not enough room for two threads")),
            "{:?}",
            shown.borrow()
        );
        assert_eq!(trek.thread_id(cx), a);
        // Room again, it opens. Narrowed after, the focused pane alone is drawn, and the other
        // counts as off screen until there's room for it.
        cx.simulate_window_resize(trek.window, size(px(1280.), px(820.)));
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.open_beside(&b, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "split-view"));
        assert!(trek.read(cx, |ws, _| ws.on_screen(&a)));
        cx.simulate_window_resize(trek.window, size(px(760.), px(700.)));
        trek.render(cx);
        assert!(!trek.visible(cx, "split-view"));
        assert!(trek.visible(cx, "split-pane-right"));
        assert!(!trek.visible(cx, "split-pane-left"));
        assert!(trek.read(cx, |ws, _| !ws.on_screen(&a) && ws.on_screen(&b)));
        // ⌘\ reaches the other one.
        trek.press(cx, "cmd-\\");
        trek.render(cx);
        assert_eq!(trek.thread_id(cx), a);
        assert!(trek.visible(cx, "split-pane-left"));
        assert!(!trek.visible(cx, "split-pane-right"));
        cx.simulate_window_resize(trek.window, size(px(1280.), px(820.)));
        trek.render(cx);
        assert!(trek.visible(cx, "split-view"));
    });
}

#[test]
fn the_divider_drags_within_the_minimum_and_double_click_evens_it() {
    run(async |cx| {
        let trek = open(cx);
        let _ = split(&trek, cx).await;
        let width = |trek: &Trek, cx: &mut TestAppContext| {
            trek.bounds(cx, "split-pane-left")
                .expect("left pane")
                .size
                .width
        };
        let before = width(&trek, cx);
        let at = trek.bounds(cx, "split-divider").expect("divider").center();
        trek.window(cx, |window, cx| {
            window.drag(at, point(at.x + px(60.), at.y), cx)
        });
        trek.render(cx);
        let after = width(&trek, cx);
        assert!(
            (after - before - px(60.)).abs() < px(1.),
            "{before:?} {after:?}"
        );
        let kept = trek.read(cx, |ws, _| ws.settings.layout.split.clone().unwrap().ratio);
        assert!(kept > 0.5);
        // Dragged far past the edge it stops at the minimum, and so does the right pane.
        let at = trek.bounds(cx, "split-divider").expect("divider").center();
        trek.window(cx, |window, cx| window.drag(at, point(px(1270.), at.y), cx));
        trek.render(cx);
        let right = trek.bounds(cx, "split-pane-right").unwrap().size.width;
        assert!(
            (right - px(trek_app::workspace::split::MIN_PANE)).abs() < px(2.),
            "{right:?}"
        );
        trek.window(cx, |window, cx| window.double_click("split-divider", cx));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.split.as_ref().unwrap().ratio), 0.5);
        // The native views get out of the way only for as long as the drag.
        assert!(trek.read(cx, |ws, _| !ws.overlay_open));
    });
}

#[test]
fn archiving_a_thread_in_a_pane_gives_the_other_the_window() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        // With the focus on it.
        trek.update(cx, |ws, cx| ws.archive(&b, cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert_eq!(trek.thread_id(cx), a);
        // Without the focus on it.
        let c = thread(&trek, cx, "third thread").await;
        open_beside(&trek, cx, &a, &c);
        assert_eq!(focused(&trek, cx), Pane::Right);
        trek.update(cx, |ws, cx| ws.delete_thread(&a, cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert_eq!(trek.thread_id(cx), c);
    });
}

#[test]
fn the_editor_and_back_keeps_the_split() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.focused_scope()), Scope::Ide);
        assert!(!trek.visible(cx, "split-view"));
        assert!(trek.read(cx, |ws, _| !ws.on_screen(&a) && !ws.on_screen(&b)));
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        assert!(trek.visible(cx, "split-view"));
        assert_eq!(trek.read(cx, |ws, _| ws.focused_scope()), Scope::Beside);
        assert_eq!(trek.thread_id(cx), b);
        // The editor's Open beside comes back to the harness for it.
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.update(cx, |ws, cx| ws.open_beside(&a, cx));
        assert!(trek.read(cx, |ws, _| !ws.ide()));
        assert_eq!(trek.thread_id(cx), a);
    });
}

#[test]
fn a_handoff_in_either_pane_stays_in_that_pane() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        // The pane without the focus follows its agent's handoff, and the focus stays where it is.
        let first = trek
            .update(cx, |ws, cx| {
                ws.handoff(&a, &serde_json::json!({ "brief": "Goal: x." }), cx)
            })
            .unwrap();
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (first.as_str(), b.as_str())
            );
            assert_eq!(s.focus, Pane::Right);
            assert_eq!(ws.route, Route::Thread(b.clone()));
            assert!(ws.tabs.contains(&first));
        });
        // The one with the focus takes the user along, as a single thread does.
        let second = trek
            .update(cx, |ws, cx| {
                ws.handoff(&b, &serde_json::json!({ "brief": "Goal: y." }), cx)
            })
            .unwrap();
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (first.as_str(), second.as_str())
            );
            assert_eq!(ws.route, Route::Thread(second.clone()));
        });
        trek.wait_done(cx, &first, RunState::Idle).await;
        trek.wait_done(cx, &second, RunState::Idle).await;
    });
}

#[test]
fn the_picker_lists_a_threads_sub_agents_first_and_doesnt_stick() {
    run(async |cx| {
        let trek = open(cx);
        let other = thread(&trek, cx, "another thread").await;
        let parent = trek.send(cx, "mock:delegate look into it");
        trek.wait_done(cx, &parent, RunState::Idle).await;
        let child = trek.read(cx, |ws, _| {
            ws.children(&parent)
                .first()
                .map(|t| t.title.clone())
                .expect("a sub-agent")
        });
        trek.render(cx);
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        trek.press(cx, "cmd-\\");
        let rows: Vec<String> = cx.read(|cx| {
            palette
                .read(cx)
                .entries(cx)
                .into_iter()
                .map(|e| e.label.to_string())
                .collect()
        });
        assert_eq!(rows.first(), Some(&child), "{rows:?}");
        assert!(trek.read(cx, |ws, _| ws.thread(&other).is_some()));
        assert_eq!(
            rows.len(),
            2,
            "the sub-agent and the other thread: {rows:?}"
        );
        // By name: nothing else comes along.
        trek.type_text(cx, "no thread is called this");
        assert!(cx.read(|cx| palette.read(cx).entries(cx).is_empty()));
        trek.press(cx, "escape");
        assert!(cx.read(|cx| !palette.read(cx).open));
        // The palette opened another way is the whole palette again.
        trek.window(cx, |window, cx| {
            palette.update(cx, |p, cx| p.open(window, cx))
        });
        cx.run_until_parked();
        assert!(cx.read(|cx| !palette.read(cx).beside_only));
    });
}

#[test]
fn a_tab_closed_takes_its_pane() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        trek.update(cx, |ws, cx| ws.close_tab(&a, cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert_eq!(trek.thread_id(cx), b);
        assert!(trek.read(cx, |ws, _| !ws.tabs.contains(&a)));
    });
}

#[test]
fn a_row_dragged_to_the_main_area_opens_beside_and_a_slipped_click_is_a_click() {
    run(async |cx| {
        let trek = open(cx);
        let a = thread(&trek, cx, "first thread").await;
        let b = thread(&trek, cx, "second thread").await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        trek.render(cx);
        let at = row(&trek, cx, &b);
        // A press that moves a few points and lets go on its own row is a click.
        trek.window(cx, |window, cx| {
            window.drag(at, point(at.x + px(4.), at.y), cx)
        });
        trek.render(cx);
        assert_eq!(trek.thread_id(cx), b);
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        // Dragged to the main area's left half, it does nothing.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        trek.render(cx);
        let at = row(&trek, cx, &b);
        let (x, y) = (px(272. + 60.), px(400.));
        trek.window(cx, |window, cx| window.drag(at, point(x, y), cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.split.is_none()));
        assert_eq!(trek.thread_id(cx), a);
        // To its right half, it opens beside.
        let at = row(&trek, cx, &b);
        trek.window(cx, |window, cx| window.drag(at, point(px(1200.), y), cx));
        trek.render(cx);
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().expect("opened beside");
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (a.as_str(), b.as_str())
            );
        });
        assert!(trek.visible(cx, "split-view"));
        // Over a split, the pane it's let go on takes it. (The new thread took `b`'s pane; `b`
        // comes back to it for the drag to have something to replace.)
        let c = thread(&trek, cx, "third thread").await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b.clone()), cx));
        trek.render(cx);
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (a.as_str(), b.as_str())
            );
        });
        let at = row(&trek, cx, &c);
        let target = trek
            .bounds(cx, "split-pane-left")
            .expect("left pane")
            .center();
        trek.window(cx, |window, cx| window.drag(at, target, cx));
        trek.render(cx);
        trek.read(cx, |ws, _| {
            let s = ws.split.as_ref().unwrap();
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (c.as_str(), b.as_str())
            );
        });
    });
}

#[test]
fn the_layout_comes_back_at_the_next_launch() {
    run(async |cx| {
        let trek = open(cx);
        let (a, b) = split(&trek, cx).await;
        let at = trek.bounds(cx, "split-divider").expect("divider").center();
        trek.window(cx, |window, cx| {
            window.drag(at, point(at.x - px(40.), at.y), cx)
        });
        trek.render(cx);
        let (store, settings) = trek.read(cx, |ws, _| (ws.store.clone(), ws.settings.clone()));
        let saved = settings.layout.split.clone().expect("kept");
        assert_eq!(
            (saved.left.as_str(), saved.right.as_str()),
            (a.as_str(), b.as_str())
        );
        assert!(saved.ratio < 0.5);
        let (ws, root, window) = launch(cx, store.clone(), settings.clone());
        cx.run_until_parked();
        let again = Trek {
            ws,
            root,
            window,
            project: trek.project.clone(),
        };
        again.render(cx);
        again.read(cx, |ws, _| {
            let s = ws.split.as_ref().expect("restored");
            assert_eq!(
                (s.left.as_str(), s.right.as_str()),
                (a.as_str(), b.as_str())
            );
            assert_eq!(s.ratio, saved.ratio);
            assert_eq!(s.focus, Pane::Right);
            assert_eq!(ws.route, Route::Thread(b.clone()));
        });
        assert!(again.visible(cx, "split-view"));
        // A layout whose thread has gone is dropped.
        let mut gone = settings.clone();
        gone.layout.split.as_mut().unwrap().left = "no-such-thread".into();
        let fresh: Store = store;
        let (ws, _, _) = launch(cx, fresh, gone);
        cx.run_until_parked();
        ws.read_with(cx, |ws, _| {
            assert!(ws.split.is_none());
            assert_eq!(ws.route, Route::Draft { project: None });
        });
    });
}
