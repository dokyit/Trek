//! The main window's composer: drafts kept per thread, delivery keys, the pills it shows.

mod common;

use common::*;
use trek_app::workspace::{Route, WorkspaceEvent};

/// What's typed stays with the thread (or new-thread draft) it was typed for: moving to another
/// thread shows that thread's own draft, and coming back brings the text back.
#[test]
fn drafts_stay_with_their_thread() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        trek.type_text(cx, "for the new thread");
        let a = trek.quiet_thread(cx);
        assert_eq!(
            trek.composer_text(cx),
            "",
            "a thread doesn't show the new thread's draft"
        );
        trek.type_text(cx, "for A");
        let b = trek.quiet_thread(cx);
        assert_eq!(trek.composer_text(cx), "", "B doesn't show A's draft");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a.clone()), cx));
        assert_eq!(trek.composer_text(cx), "for A");
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        assert_eq!(trek.composer_text(cx), "for the new thread");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert_eq!(trek.composer_text(cx), "");
    });
}

/// A message a rewind puts back lands in its own thread's composer and doesn't follow the user
/// to another thread.
#[test]
fn a_message_put_back_stays_in_its_thread() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.quiet_thread(cx);
        let b = trek.quiet_thread(cx);
        // Navigating and handing the message over in one go, as a fork does.
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Thread(a.clone()), cx);
            cx.emit(WorkspaceEvent::ComposeIn {
                scope: trek_app::workspace::Scope::Main,
                thread: a.clone(),
                text: "How does the app start up?".into(),
                images: vec![],
                edit: None,
            });
        });
        assert_eq!(trek.composer_text(cx), "How does the app start up?");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert_eq!(trek.composer_text(cx), "", "the message stays with A");
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a), cx));
        assert_eq!(trek.composer_text(cx), "How does the app start up?");
    });
}

/// While a turn runs, ↩ goes the way Settings › General says (steer, by default) and ⌥↩ the
/// other way: here, queued for after the turn.
#[test]
fn option_return_sends_the_other_way_mid_turn() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the turn to start", move |ws| {
            ws.live
                .get(&tid)
                .is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())
        })
        .await;
        trek.type_text(cx, "and then this");
        trek.press(cx, "alt-enter");
        assert_eq!(
            trek.read(cx, |ws, _| ws.queued(&id)),
            1,
            "queued behind the running turn"
        );
        assert_eq!(trek.composer_text(cx), "");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
    });
}

/// The send button says which keys do what, mid-turn too.
#[test]
fn the_send_hint_names_the_keys() {
    use trek_app::composer::delivery_hint;
    use trek_core::settings::FollowUp;
    trek_app::keys::with_mac(true, || {
        assert_eq!(
            delivery_hint(false, FollowUp::Steer, false),
            "Send ↩ · New line ⇧↩"
        );
        assert_eq!(
            delivery_hint(true, FollowUp::Steer, false),
            "Send ⌘↩ · New line ↩"
        );
        assert_eq!(
            delivery_hint(false, FollowUp::Steer, true),
            "↩ steers the running turn · ⌥↩ queues for after it"
        );
        assert_eq!(
            delivery_hint(false, FollowUp::Queue, true),
            "↩ queues for after the turn · ⌥↩ steers it now"
        );
    });
    trek_app::keys::with_mac(false, || {
        assert_eq!(
            delivery_hint(false, FollowUp::Steer, false),
            "Send Enter · New line Shift+Enter"
        );
        assert_eq!(
            delivery_hint(true, FollowUp::Steer, false),
            "Send Ctrl+Enter · New line Enter"
        );
        assert_eq!(
            delivery_hint(false, FollowUp::Steer, true),
            "Enter steers the running turn · Alt+Enter queues for after it"
        );
        assert_eq!(
            delivery_hint(false, FollowUp::Queue, true),
            "Enter queues for after the turn · Alt+Enter steers it now"
        );
    });
}

/// The context ring stays out of the way early in a thread and shows once there's something to
/// watch. The Plan pill is in a thread's composer as on a new thread's.
#[test]
fn the_context_ring_waits_and_plan_is_always_there() {
    run(async |cx| {
        let trek = open(cx);
        assert!(trek.visible(cx, "plan-pill"), "on a new thread");
        let id = trek.quiet_thread(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "plan-pill"), "in a thread under way");
        let set = |trek: &Trek, cx: &mut gpui_kit::TestAppContext, used: u64| {
            let id = id.clone();
            trek.update(cx, move |ws, cx| {
                ws.live.entry(id).or_default().context = Some((used, 200_000));
                cx.notify();
            });
            trek.render(cx);
        };
        set(&trek, cx, 20_000);
        assert!(!trek.visible(cx, "context-ring"), "10% isn't worth a ring");
        set(&trek, cx, 100_000);
        assert!(trek.visible(cx, "context-ring"), "50% is");
    });
}

/// After a toast offering an undo, there's time to reach it.
#[test]
fn undo_toasts_stay_long_enough_to_reach() {
    use std::time::Duration;
    use trek_app::root::toast_lifetime;
    assert!(toast_lifetime("Settled", true) >= Duration::from_secs(10));
    assert_eq!(toast_lifetime("Settled", false), Duration::from_secs(5));
    let long = "Titles are written with Claude Code, which isn't available. Used the first message instead.";
    assert!(
        toast_lifetime(long, false) > Duration::from_secs(5),
        "a long one stays longer"
    );
    assert!(toast_lifetime(&long.repeat(10), false) <= Duration::from_secs(12));
}

/// ⌘E settles the thread on screen and says which, with an undo.
#[test]
fn command_e_settles_with_an_undo() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.rename(&id, "Add a note".into(), cx));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
                if let WorkspaceEvent::Toast { message, undo } = event {
                    sink.borrow_mut().push((message.clone(), undo.is_some()));
                }
            })
            .detach()
        });
        // Trek in front, so the toast has a window to show in.
        cx.update(|cx| trek_app::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        trek.press(cx, "secondary-e");
        assert!(trek.read(cx, |ws, _| {
            ws.thread(&id).is_some_and(|t| t.settled_at.is_some())
        }));
        assert_eq!(*seen.borrow(), [("Settled “Add a note”".to_string(), true)]);
        // The toast is on screen, with its Undo.
        assert_eq!(
            trek.window(cx, |window, cx| trek_app::toast::count(window, cx)),
            1
        );
        std::thread::sleep(std::time::Duration::from_millis(450));
        trek.render(cx);
        trek.click(cx, "undo");
        assert!(
            trek.read(cx, |ws, _| ws
                .thread(&id)
                .is_some_and(|t| t.settled_at.is_none())),
            "Undo brings it back"
        );
        // Another one, left alone: it goes once its time is up.
        trek.update(cx, |ws, cx| ws.settle(&id, cx));
        assert!(trek.window(cx, |window, cx| trek_app::toast::count(window, cx)) >= 1);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(12));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(300));
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            trek.window(cx, |window, cx| trek_app::toast::count(window, cx)),
            0,
            "gone after its time"
        );
    });
}

/// A window too narrow for the sidebar, the chat and the tools panel: the sidebar steps aside
/// (at the smallest window the chat and panel then fit side by side); past that the panel floats.
#[test]
fn a_narrow_window_makes_room_for_the_tools_panel() {
    use trek_app::root::{SIDEBAR_WIDTH, overlay_panel, sidebar_yields};
    assert!(!sidebar_yields(1280., true, false));
    assert!(sidebar_yields(760., true, false));
    assert!(!sidebar_yields(760., false, false), "only for the panel");
    assert!(!sidebar_yields(760., true, true), "settings have no panel");
    assert!(
        !overlay_panel(760. - 8. - 16.),
        "the smallest window fits chat and panel once the sidebar steps aside"
    );
    assert!(overlay_panel(760. - SIDEBAR_WIDTH - 16.));
}

/// OpenCode 2's effort levels are each model's own. Once a session says which its model offers,
/// the effort menu shows those, and a thread on a level the model doesn't have moves to the one
/// the session is on (the model's default); a level it has stays.
#[test]
fn a_models_own_efforts_replace_the_shared_ones() {
    use trek_agents::{AcpInfo, AgentEvent};
    use trek_core::catalog::ModelInfo;
    use trek_core::{AgentId, Effort, HandHolding};
    run(async |cx| {
        let trek = open(cx);
        let shared = vec![Effort::Low, Effort::Medium, Effort::High];
        let id = trek.update(cx, |ws, cx| {
            let model = |id: &str| ModelInfo {
                id: id.into(),
                name: id.into(),
                efforts: shared.clone(),
                tier: 0,
                fast: None,
            };
            ws.acp_info.insert(
                "opencode".into(),
                Ok(AcpInfo {
                    models: vec![model("opencode/a"), model("opencode/b")],
                    ..Default::default()
                }),
            );
            let t = ws
                .store
                .create_thread(
                    Some(&trek.project),
                    AgentId::OpenCode,
                    Some("opencode/b".into()),
                    Effort::Low,
                    HandHolding::Auto,
                )
                .unwrap();
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        let said = |effort| AgentEvent::Efforts {
            model: "opencode/b".into(),
            efforts: vec![Effort::Medium, Effort::High],
            effort,
        };
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![said(Some(Effort::Medium))], cx)
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().effort),
            Effort::Medium,
            "B has no Low: its default"
        );
        let efforts = |cx: &mut gpui_kit::TestAppContext, m: &str| {
            trek.read(cx, |ws, _| {
                ws.models_for(&AgentId::OpenCode)
                    .into_iter()
                    .find(|i| i.id == m)
                    .unwrap()
                    .efforts
            })
        };
        assert_eq!(
            efforts(cx, "opencode/b"),
            vec![Effort::Medium, Effort::High]
        );
        assert_eq!(
            efforts(cx, "opencode/a"),
            shared,
            "another model's are as they were"
        );
        trek.render(cx);
        trek.click(cx, "model-pill");
        trek.render(cx);
        trek.click(cx, "mm-effort");
        trek.render(cx);
        assert!(
            trek.visible(cx, "eff-medium")
                && trek.visible(cx, "eff-high")
                && !trek.visible(cx, "eff-low")
        );
        // Picked from those, it stays whatever level the session reports next.
        trek.click(cx, "eff-high");
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![said(Some(Effort::Medium))], cx)
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).unwrap().effort),
            Effort::High
        );
    });
}

#[test]
fn the_model_picker_calls_its_rail_an_agent_not_a_provider() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        trek.click(cx, "model-pill");
        trek.render(cx);
        trek.click(cx, "mm-model");
        trek.render(cx);
        assert!(trek.visible(cx, "model-source"));
    });
}

#[test]
fn a_first_message_whose_thread_couldnt_be_made_goes_back_to_the_composer() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        // No folder can be made for a thread in no project: a file has the place they go.
        let chats = trek_core::paths::chats_dir();
        let _ = std::fs::remove_dir_all(&chats);
        std::fs::create_dir_all(chats.parent().unwrap()).unwrap();
        std::fs::write(&chats, "").unwrap();
        let image = trek.project.join("shot.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 140, 200, 255]))
            .save(&image)
            .unwrap();
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| c.attach_image(image.clone(), cx));
        trek.type_text(cx, "a long prompt");
        trek.press(cx, "enter");
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft { project: None },
            "no thread"
        );
        assert_eq!(trek.composer_text(cx), "a long prompt");
        assert_eq!(cx.read(|cx| composer.read(cx).attached().0), [image]);
        std::fs::remove_file(&chats).unwrap();
    });
}

/// The picker's trigger is only recomputed as text changes. Enter with the caret moved out of
/// the token must not replace what lies between the caret and the old `@`.
#[test]
fn a_pick_with_the_caret_gone_from_the_token_replaces_nothing() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("alpha_only.rs"), "").unwrap();
        let picks = |trek: &Trek, cx: &gpui_kit::TestAppContext| {
            cx.read(|cx| trek.root.read(cx).composer.read(cx).picks(cx))
        };
        trek.type_text(cx, "fix the bug in @alpha");
        for _ in 0..2000 {
            cx.run_until_parked();
            if picks(&trek, cx).contains(&"alpha_only.rs".to_string()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            picks(&trek, cx).contains(&"alpha_only.rs".to_string()),
            "{:?}",
            picks(&trek, cx)
        );
        trek.press(cx, "home");
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "fix the bug in @alpha");
        assert!(
            picks(&trek, cx).is_empty(),
            "the picker closed with the caret out of the token"
        );
    });
}

/// "Add photos & files" appends `@path` references and leaves the caret after them.
#[test]
fn attached_files_leave_the_caret_after_their_references() {
    run(async |cx| {
        let trek = open(cx);
        trek.type_text(cx, "please review");
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        let file = trek.project.join("notes.md");
        trek.window(cx, |window, cx| {
            composer.update(cx, |c, cx| {
                c.append_file_refs(std::slice::from_ref(&file), window, cx)
            })
        });
        let want = format!("please review @{} ", file.display());
        assert_eq!(trek.composer_text(cx), want);
        trek.type_text(cx, "thanks");
        assert_eq!(trek.composer_text(cx), format!("{want}thanks"));
    });
}

/// Voice mode, driven without a mic: the take opens as `ask_to_dictate` opens it once both
/// permissions are in, and `heard` stands in for the recognizer.
fn open_voice(
    trek: &Trek,
    cx: &mut gpui_kit::TestAppContext,
    live: bool,
) -> async_channel::Sender<trek_app::dictate::Heard> {
    let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
    let (heard, results) = async_channel::unbounded();
    trek.window(cx, |window, cx| {
        composer.update(cx, |c, cx| c.begin_voice(live, results, window, cx))
    });
    trek.render(cx);
    heard
}

fn voice_preview(trek: &Trek, cx: &gpui_kit::TestAppContext) -> (String, std::ops::Range<usize>) {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).voice_preview_text())
}

fn voice_phase(trek: &Trek, cx: &gpui_kit::TestAppContext) -> trek_app::composer::voice::Phase {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).voice_phase())
}

/// What the recognizer hears shows at the caret, styled as interim and revised in place, but
/// the message itself holds only what was typed: a draft put away mid-take keeps no unconfirmed
/// words, and the take ends there.
#[test]
fn interim_dictation_never_lands_in_the_draft() {
    use trek_app::composer::voice::Phase;
    use trek_app::dictate::Heard;
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        trek.type_text(cx, "Please");
        let heard = open_voice(&trek, cx, true);
        assert!(
            trek.visible(cx, "voice-row")
                && trek.visible(cx, "voice-confirm")
                && trek.visible(cx, "voice-cancel")
        );
        assert!(
            !trek.visible(cx, "send"),
            "the waveform row replaces the toolbar"
        );
        heard.try_send(Heard::Partial("it is can".into())).unwrap();
        trek.render(cx);
        assert_eq!(
            voice_preview(&trek, cx),
            ("Please it is can".to_string(), 7..16)
        );
        heard
            .try_send(Heard::Partial("it is. Can you make the button".into()))
            .unwrap();
        trek.render(cx);
        let (text, interim) = voice_preview(&trek, cx);
        assert_eq!(
            &text[interim], "it is. Can you make the button",
            "revised in place"
        );
        assert_eq!(
            trek.composer_text(cx),
            "Please",
            "interim text is not in the message"
        );
        // Another thread on screen: the draft goes away as typed and the take ends.
        trek.quiet_thread(cx);
        assert_eq!(voice_phase(&trek, cx), Phase::Idle);
        assert_eq!(trek.composer_text(cx), "");
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project),
                },
                cx,
            )
        });
        assert_eq!(
            trek.composer_text(cx),
            "Please",
            "the draft holds no unconfirmed words"
        );
        // A result arriving after the take ended changes nothing.
        let _ = heard.try_send(Heard::Final("late".into()));
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "Please");
    });
}

/// ✓ (and ↩) puts the text in as typed: the recognizer's final text when it settles in time,
/// else what was on screen once the grace runs out. The toolbar comes back with the caret
/// after the words.
#[test]
fn confirming_dictation_commits_the_text() {
    use trek_app::composer::voice::{GRACE, Phase};
    use trek_app::dictate::Heard;
    run(async |cx| {
        let trek = open(cx);
        trek.type_text(cx, "Note:");
        let heard = open_voice(&trek, cx, true);
        heard
            .try_send(Heard::Partial("make the buttons bigger".into()))
            .unwrap();
        trek.render(cx);
        trek.click(cx, "voice-confirm");
        assert_eq!(voice_phase(&trek, cx), Phase::Finishing);
        assert_eq!(
            trek.composer_text(cx),
            "Note:",
            "nothing goes in until it settles"
        );
        heard
            .try_send(Heard::Final(
                "make the buttons bigger, but keep the padding.".into(),
            ))
            .unwrap();
        trek.render(cx);
        assert_eq!(voice_phase(&trek, cx), Phase::Idle);
        assert_eq!(
            trek.composer_text(cx),
            "Note: make the buttons bigger, but keep the padding."
        );
        assert!(
            trek.visible(cx, "send") && !trek.visible(cx, "voice-row"),
            "the toolbar is back"
        );
        trek.type_text(cx, " Thanks");
        assert_eq!(
            trek.composer_text(cx),
            "Note: make the buttons bigger, but keep the padding. Thanks",
            "the caret is after the words"
        );

        // ↩, and a recognizer that doesn't settle: the shown text goes in after the grace.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        let heard = open_voice(&trek, cx, true);
        heard.try_send(Heard::Partial("all right".into())).unwrap();
        trek.render(cx);
        trek.press(cx, "enter");
        assert_eq!(voice_phase(&trek, cx), Phase::Finishing);
        cx.executor().advance_clock(GRACE);
        trek.render(cx);
        assert_eq!(trek.composer_text(cx), "all right");
        assert_eq!(voice_phase(&trek, cx), Phase::Idle);
    });
}

/// ✕ (and esc) drops what was heard: the message is as it was, caret included.
#[test]
fn cancelling_dictation_restores_the_message() {
    use trek_app::composer::voice::Phase;
    use trek_app::dictate::Heard;
    run(async |cx| {
        let trek = open(cx);
        trek.type_text(cx, "keep this");
        let heard = open_voice(&trek, cx, true);
        heard
            .try_send(Heard::Partial("noise noise".into()))
            .unwrap();
        trek.render(cx);
        trek.click(cx, "voice-cancel");
        assert_eq!(voice_phase(&trek, cx), Phase::Idle);
        assert_eq!(trek.composer_text(cx), "keep this");
        assert!(trek.visible(cx, "send"));
        let heard = open_voice(&trek, cx, true);
        heard.try_send(Heard::Partial("more noise".into())).unwrap();
        trek.render(cx);
        trek.press(cx, "escape");
        assert_eq!(voice_phase(&trek, cx), Phase::Idle);
        assert_eq!(trek.composer_text(cx), "keep this");
        trek.type_text(cx, "!");
        assert_eq!(
            trek.composer_text(cx),
            "keep this!",
            "focus and caret are back in the message"
        );
    });
}

/// The waveform's levels: silence stays a row of dots, a sound rises within a tick and falls
/// back over a few, readings out of range are held to 0 to 1, the newest is last, and the
/// history is capped. Meter readings map from dBFS.
#[test]
fn waveform_levels_rise_fast_and_fall_slowly() {
    use trek_app::composer::voice::{KEPT, Levels};
    use trek_app::dictate::{loudness, loudness_db};
    let mut levels = Levels::default();
    for _ in 0..5 {
        levels.push(0.);
    }
    assert!(levels.bars().all(|b| b == 0.), "silence is dots");
    levels.push(1.);
    let peak = levels.bars().last().unwrap();
    assert!(peak >= 0.65, "a burst shows at once: {peak}");
    levels.push(0.);
    let after = levels.bars().last().unwrap();
    assert!(
        after > 0.3 && after < peak,
        "and falls back gradually: {after}"
    );
    for _ in 0..30 {
        levels.push(0.);
    }
    assert_eq!(levels.bars().last(), Some(0.), "back to a dot");
    levels.push(f32::NAN);
    levels.push(7.);
    assert!(levels.bars().all(|b| (0. ..=1.).contains(&b)));
    for _ in 0..KEPT + 50 {
        levels.push(0.5);
    }
    assert_eq!(levels.bars().len(), KEPT, "the history is capped");
    assert_eq!(loudness_db(-160.), 0.);
    assert_eq!(loudness_db(0.), 1.);
    assert_eq!(loudness_db(f32::NEG_INFINITY), 0.);
    assert!(
        loudness_db(-30.) > 0.3 && loudness_db(-30.) < 0.7,
        "speech sits mid-strip"
    );
    assert_eq!(loudness(&[]), 0.);
    assert_eq!(loudness(&[0.; 64]), 0.);
    assert!(loudness(&[0.5, -0.5, 0.5, -0.5]) > 0.9);
}

/// The toolbar row's state machine: mic → asking → listening → ✓ finishing → idle, with ✕,
/// failures, backgrounding and the file fallback each ending it their own way.
#[test]
fn the_voice_toolbar_moves_through_its_states() {
    use trek_app::composer::voice::{Effect, Event, Phase, Voice};
    let started = |live| Event::Started {
        live,
        value: "Hi".into(),
        cursor: 2,
    };
    let mut v = Voice::default();
    assert!(!v.active());
    assert_eq!(
        v.step(Event::Mic),
        Effect {
            ask: true,
            ..Default::default()
        }
    );
    assert_eq!((v.phase, v.take), (Phase::Asking, 1));
    assert!(!v.active(), "the toolbar stays while macOS asks");
    assert_eq!(
        v.step(Event::Background),
        Effect::default(),
        "a permission dialog takes focus: ignored"
    );
    assert_eq!(
        v.step(Event::Mic),
        Effect::default(),
        "the spinning mic does nothing"
    );
    v.step(started(true));
    assert!(v.active() && v.phase == Phase::Listening);
    v.step(Event::Partial("there".into()));
    assert_eq!(v.preview(), ("Hi there".to_string(), 3..8));
    assert_eq!(
        v.step(Event::Confirm),
        Effect {
            finish: true,
            ..Default::default()
        }
    );
    assert_eq!(v.phase, Phase::Finishing);
    assert_eq!(v.step(Event::Confirm), Effect::default(), "✓ once");
    assert_eq!(
        v.step(Event::Background),
        Effect::default(),
        "settling goes on in the background"
    );
    v.step(Event::Partial("there you".into()));
    assert_eq!(
        v.step(Event::GraceOver),
        Effect {
            end: true,
            commit: Some("there you".into()),
            ..Default::default()
        }
    );
    assert_eq!(v.phase, Phase::Idle);
    assert_eq!(
        v.step(Event::Final("late".into())),
        Effect::default(),
        "a result after the take ends is ignored"
    );

    // Refused: back to the toolbar with why.
    v.step(Event::Mic);
    assert_eq!(
        v.step(Event::Failed("no mic".into())),
        Effect {
            end: true,
            toast: Some("no mic".into()),
            ..Default::default()
        }
    );
    assert_eq!(v.take, 2);

    // ✕ drops the words; the mic while listening is ✓.
    v.step(Event::Mic);
    v.step(started(true));
    v.step(Event::Partial("scratch that".into()));
    assert_eq!(
        v.step(Event::Cancel),
        Effect {
            end: true,
            ..Default::default()
        }
    );
    v.step(Event::Mic);
    v.step(started(true));
    assert_eq!(
        v.step(Event::Mic),
        Effect {
            finish: true,
            ..Default::default()
        }
    );
    assert_eq!(
        v.step(Event::Final("  ".into())),
        Effect {
            end: true,
            toast: Some("Nothing heard.".into()),
            ..Default::default()
        }
    );

    // Backgrounded or failing mid-take: what was heard stays, with a toast.
    v.step(Event::Mic);
    v.step(started(true));
    v.step(Event::Partial("half a thought".into()));
    let e = v.step(Event::Background);
    assert_eq!((e.end, e.commit.as_deref()), (true, Some("half a thought")));
    assert!(e.toast.is_some_and(|t| t.contains("background")));
    v.step(Event::Mic);
    v.step(started(true));
    let e = v.step(Event::Failed("the mic went away".into()));
    assert_eq!((e.end, e.commit), (true, None));
    assert_eq!(
        e.toast.as_deref(),
        Some("Dictation stopped: the mic went away")
    );

    // After ✓, a failure keeps what was shown, quietly.
    v.step(Event::Mic);
    v.step(started(true));
    v.step(Event::Partial("ship it".into()));
    v.step(Event::Confirm);
    assert_eq!(
        v.step(Event::Failed("No speech detected".into())),
        Effect {
            end: true,
            commit: Some("ship it".into()),
            ..Default::default()
        }
    );

    // The recorded-file fallback: no partials, and ✓ waits for Speech however long it takes.
    v.step(Event::Mic);
    v.step(started(false));
    v.step(Event::Confirm);
    assert_eq!(
        v.step(Event::GraceOver),
        Effect::default(),
        "no grace for a file being transcribed"
    );
    assert_eq!(
        v.step(Event::Final("from the file".into())),
        Effect {
            end: true,
            commit: Some("from the file".into()),
            ..Default::default()
        }
    );

    // The preview splices at the caret with a space either side as needed, on a char boundary.
    let mut v = Voice::default();
    v.step(Event::Started {
        live: true,
        value: "héllo world".into(),
        cursor: 2,
    });
    assert_eq!(
        v.preview(),
        ("héllo world".to_string(), 1..1),
        "nothing heard yet: no interim range"
    );
    let mut v = Voice::default();
    v.step(Event::Started {
        live: true,
        value: "ab".into(),
        cursor: 1,
    });
    v.step(Event::Partial(" x ".into()));
    assert_eq!(v.preview(), ("a x b".to_string(), 2..3));
}

/// Commands use the input's atomic token editing, but retain their source spelling on send.
#[test]
fn slash_ghost_tab_chip_and_send() {
    run(async |cx| {
        let trek = open(cx);
        let id = slash_commands_for_test(&trek, cx);
        trek.type_text(cx, "/imp");
        assert_eq!(trek.composer_text(cx), "/imp");
        assert_eq!(
            slash_completion(&trek, cx),
            (Some("rove-codebase-architecture".into()), 0, 4)
        );
        trek.press(cx, "tab");
        assert_eq!(trek.composer_text(cx), "/improve-codebase-architecture ");
        assert_eq!(slash_completion(&trek, cx), (None, 1, 31));
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "");
        assert_eq!(slash_completion(&trek, cx).1, 0);
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if text.trim() == "/improve-codebase-architecture")));
    });
}

#[test]
fn slash_chip_backspace_and_arrows_are_atomic() {
    run(async |cx| {
        let trek = open(cx);
        slash_commands_for_test(&trek, cx);
        trek.type_text(cx, "/imp");
        trek.press(cx, "tab");
        trek.press(cx, "left"); // Before the trailing space, after the chip.
        assert_eq!(slash_completion(&trek, cx).2, 30);
        trek.press(cx, "left");
        assert_eq!(slash_completion(&trek, cx).2, 0);
        trek.press(cx, "right");
        assert_eq!(slash_completion(&trek, cx).2, 30);
        trek.press(cx, "backspace");
        assert_eq!(trek.composer_text(cx), " ");
        assert_eq!(slash_completion(&trek, cx).1, 0);
    });
}

#[test]
fn slash_ghost_escape_and_saved_draft_keep_only_typed_text() {
    run(async |cx| {
        let trek = open(cx);
        let id = slash_commands_for_test(&trek, cx);
        trek.type_text(cx, "/imp");
        assert!(slash_completion(&trek, cx).0.is_some());
        trek.press(cx, "escape");
        assert_eq!(trek.composer_text(cx), "/imp");
        assert_eq!(slash_completion(&trek, cx).0, None);
        // Reopen a live suggestion, then put this draft away without accepting it.
        trek.type_text(cx, "r");
        assert!(slash_completion(&trek, cx).0.is_some());
        trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx));
        assert_eq!(trek.composer_text(cx), "/impr");
        assert_eq!(slash_completion(&trek, cx).0, None);
    });
}

fn slash_commands_for_test(trek: &Trek, cx: &mut gpui_kit::TestAppContext) -> String {
    let id = trek.quiet_thread(cx);
    let commands = ["improve-codebase-architecture", "improve-tests"]
        .into_iter()
        .map(|name| trek_agents::SlashCommand {
            name: name.into(),
            description: "Improve this project".into(),
            kind: trek_agents::CommandKind::Skill,
        })
        .collect();
    trek.update(cx, |ws, cx| {
        ws.apply_events(&id, vec![trek_agents::AgentEvent::Commands(commands)], cx)
    });
    id
}

fn slash_completion(trek: &Trek, cx: &gpui_kit::TestAppContext) -> (Option<String>, usize, usize) {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).slash_completion(cx))
}

#[test]
fn slash_enter_picks_highlight_but_tab_accepts_first_ghost() {
    run(async |cx| {
        let trek = open(cx);
        slash_commands_for_test(&trek, cx);
        trek.type_text(cx, "/imp");
        trek.press(cx, "down");
        assert_eq!(
            slash_completion(&trek, cx).0.as_deref(),
            Some("rove-codebase-architecture")
        );
        trek.press(cx, "enter");
        assert_eq!(trek.composer_text(cx), "/improve-tests ");
        assert_eq!(slash_completion(&trek, cx).1, 1);
        trek.press(cx, "backspace"); // The delimiter.
        trek.press(cx, "backspace"); // The token.
        trek.type_text(cx, "/imp");
        trek.press(cx, "down");
        trek.press(cx, "tab");
        assert_eq!(trek.composer_text(cx), "/improve-codebase-architecture ");
        assert_eq!(slash_completion(&trek, cx).1, 1);
    });
}
