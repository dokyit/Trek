//! Plugins in the app: with none installed every place is empty; a plugin that's on draws in its
//! place and one that's off doesn't; a replacement stands in for Trek's surface until its plugin
//! stops, and then Trek's own is back; a page's web view exists only while the page is open, and
//! gives the keyboard back before it goes; a typed-in draft keeps taking keys after a page closes.
//! The examples do what they say: the focus walk's lengths, break and restart, the marmot's
//! moods from agents' turns, the sounds a plugin plays, Trailhead's prompts from its logic and
//! its page.

mod common;

use common::{open, run};
use gpui_kit::TestAppContext;
use gpui_kit::component::WindowExt as _;
use trek_app::workspace::Route;
use trek_core::plugins::{Slot, Surface};

/// Line weight, shadows and the theme are the process's: these tests take turns.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn start(cx: &mut TestAppContext) -> common::Trek {
    let trek = open(cx);
    cx.update(|cx| {
        trek_app::themes::init(&trek.ws, cx);
        trek_app::plugins::init(&trek.ws, cx);
    });
    trek
}

/// Install one of Trek's examples as `trek plugin install example:<name>` does, and have the
/// app read the folder.
fn install(name: &str, trek: &common::Trek, cx: &mut TestAppContext) {
    let dir = trek_core::plugins::dir();
    let mut out = Vec::new();
    let code = trek_core::plugins::cli::run(
        &["install".into(), format!("example:{name}")],
        &dir,
        &mut out,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
    cx.run_until_parked();
}

fn draft(trek: &common::Trek, cx: &mut TestAppContext) {
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

#[test]
fn no_plugins_nothing_drawn_and_a_plugin_draws_only_while_its_on() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        // Nothing installed: every slot is an empty list, and no surface is replaced.
        cx.update(|cx| {
            for slot in Slot::ALL {
                assert!(
                    trek_app::plugins::slot(slot, cx).is_empty(),
                    "{}",
                    slot.name()
                );
            }
            for surface in Surface::ALL {
                assert!(trek_app::plugins::replacement(surface, cx).is_none());
            }
            assert!(trek_app::plugins::palette_entries(cx).is_empty());
        });
        draft(&trek, cx);
        assert!(!trek.visible(cx, "focus-chip"));

        // The focus timer asks for nothing: installed, it's on, and its chip is on the composer.
        install("focus-timer", &trek, cx);
        cx.update(|cx| {
            assert!(trek_app::plugins::running("focus-timer", cx));
            assert_eq!(trek_app::plugins::slot(Slot::ComposerBorder, cx).len(), 1);
        });
        draft(&trek, cx);
        assert!(trek.visible(cx, "focus-chip"), "the timer's chip");

        // A session under way: time alone places the hiker, and stopping clears it.
        cx.update(|cx| trek_app::plugins::focus::jump(300., &trek.ws, cx));
        trek.render(cx);
        cx.update(|cx| {
            let f = cx.global::<trek_app::plugins::focus::Focus>();
            let now = trek_app::motion::now(cx);
            assert!(matches!(
                f.session,
                trek_app::plugins::focus::Session::Running { .. }
            ));
            let share = f.share(now);
            assert!((0.19..0.21).contains(&share), "5 of 25 minutes: {share}");
            assert_eq!(f.left(now), 20 * 60);
        });
        cx.update(|cx| trek_app::plugins::focus::control("stop", &trek.ws, cx));

        // Off: nothing of it is drawn.
        cx.update(|cx| trek_app::plugins::set_on("focus-timer", false, &trek.ws, cx));
        draft(&trek, cx);
        cx.update(|cx| assert!(trek_app::plugins::slot(Slot::ComposerBorder, cx).is_empty()));
        assert!(!trek.visible(cx, "focus-chip"));
    });
}

#[test]
fn a_plugin_that_asks_waits_and_a_replacement_gives_way_when_its_plugin_stops() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        // Now playing asks to read threads and open them: installed from the CLI it waits.
        install("now-playing", &trek, cx);
        cx.update(|cx| {
            assert!(!trek_app::plugins::running("now-playing", cx));
            assert!(trek_app::plugins::slot(Slot::Status, cx).is_empty());
        });
        // The user turns it on in Settings (allowing what it asks): its pill has its place.
        cx.update(|cx| trek_app::plugins::set_on("now-playing", true, &trek.ws, cx));
        cx.update(|cx| {
            assert!(trek_app::plugins::running("now-playing", cx));
            assert_eq!(trek_app::plugins::slot(Slot::Status, cx).len(), 1);
        });

        // Trailhead asks for nothing: it runs at once, and its screen is the new-thread screen
        // once it's chosen.
        install("trailhead", &trek, cx);
        cx.update(|cx| assert!(trek_app::plugins::running("trailhead", cx)));
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(
                Surface::NewThread,
                Some("trailhead"),
                &trek.ws,
                cx,
            )
        });
        draft(&trek, cx);
        assert!(trek.visible(cx, "plugin-surface-new-thread"));

        // Settings puts Trek's own back, and the plugin's again.
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(Surface::NewThread, None, &trek.ws, cx)
        });
        draft(&trek, cx);
        assert!(!trek.visible(cx, "plugin-surface-new-thread"));
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(
                Surface::NewThread,
                Some("trailhead"),
                &trek.ws,
                cx,
            )
        });
        draft(&trek, cx);
        assert!(trek.visible(cx, "plugin-surface-new-thread"));

        // It fails: only it stops, the screen is Trek's own again, and Settings says why.
        cx.update(|cx| trek_app::plugins::stop("trailhead", "TypeError: x", &trek.ws, cx));
        draft(&trek, cx);
        cx.update(|cx| {
            assert!(!trek_app::plugins::running("trailhead", cx));
            assert!(trek_app::plugins::replacement(Surface::NewThread, cx).is_none());
            assert!(trek_app::plugins::running("now-playing", cx));
            let p = cx.global::<trek_app::plugins::Plugins>();
            assert_eq!(
                p.state.choice("trailhead").stopped.as_deref(),
                Some("TypeError: x")
            );
        });
        assert!(!trek.visible(cx, "plugin-surface-new-thread"));
    });
}

#[test]
fn a_grant_written_into_the_file_is_ignored_and_an_edited_plugin_asks_again() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("now-playing", &trek, cx);
        let dir = trek_core::plugins::dir();
        let running =
            |cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running("now-playing", cx));
        let waiting = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                cx.global::<trek_app::plugins::Plugins>()
                    .state
                    .choice("now-playing")
                    .waiting
            })
        };
        // An agent writes the grant into state.toml itself: Trek doesn't take it.
        std::fs::write(
            dir.join(trek_core::plugins::STATE_FILE),
            "[plugins.now-playing]\non = true\ngranted = [\"threads:read\", \"threads:open\"]\nseal = \"0123abcd\"\n",
        )
        .unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(!running(cx), "a hand-written grant is no grant");
        assert!(waiting(cx), "it waits for the user");
        cx.update(|cx| assert!(trek_app::plugins::slot(Slot::Status, cx).is_empty()));

        // The user allows it in Settings: it runs, and the seal Trek wrote is good at the next
        // read of the folder.
        cx.update(|cx| trek_app::plugins::set_on("now-playing", true, &trek.ws, cx));
        assert!(running(cx));
        let kept = std::fs::read_to_string(dir.join(trek_core::plugins::STATE_FILE)).unwrap();
        assert!(kept.contains("seal = "), "{kept}");
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(running(cx), "sealed, it stays on");

        // Its files change (an update, or another plugin put in its place): it asks again.
        let pill = dir.join("now-playing/ui/status.json");
        let text = std::fs::read_to_string(&pill).unwrap();
        std::fs::write(&pill, text.replace("open it", "open it now")).unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(!running(cx), "the grant was for the plugin the user saw");
        assert!(waiting(cx));
        cx.update(|cx| assert!(trek_app::plugins::slot(Slot::Status, cx).is_empty()));
    });
}

#[test]
fn a_page_s_view_lives_while_its_open_and_gives_the_keys_back_before_it_goes() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("trailhead", &trek, cx);
        draft(&trek, cx);
        let has_view = |cx: &mut TestAppContext| {
            trek.root
                .read_with(cx, |r, cx| r.page_host.read(cx).has_view())
        };
        assert!(!has_view(cx), "no page open, no web view");
        let _ = trek_app::plugins::page::fake::take();

        // The page opens: its view is made and shown.
        cx.update(|cx| trek_app::plugins::open_page("trailhead", "prompts", &trek.ws, cx));
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| matches!(ws.route, Route::Page { .. })));
        assert!(has_view(cx));
        assert!(trek.visible(cx, "plugin-page"));
        let told = trek_app::plugins::page::fake::take();
        assert!(told.iter().any(|l| l == "shown true"), "{told:?}");

        // A menu over it hides it (a native view would cover the menu), and shows it again.
        trek.update(cx, |ws, cx| {
            ws.overlay_open = true;
            cx.notify();
        });
        assert_eq!(trek_app::plugins::page::fake::take(), ["shown false"]);
        trek.update(cx, |ws, cx| {
            ws.overlay_open = false;
            cx.notify();
        });
        assert_eq!(trek_app::plugins::page::fake::take(), ["shown true"]);

        // Leaving the page: the keys go back to Trek's view first, then the view is hidden and
        // dropped.
        draft(&trek, cx);
        assert!(!has_view(cx), "the view goes with the page");
        let told = trek_app::plugins::page::fake::take();
        assert_eq!(told, ["release-keys", "shown false"], "keys first");

        // And the composer takes what's typed.
        trek.type_text(cx, "still typing");
        let typed = trek
            .root
            .read_with(cx, |r, cx| r.composer.read(cx).text(cx));
        assert_eq!(typed, "still typing");

        // Its plugin turned off while the page is open closes the page's view too.
        cx.update(|cx| trek_app::plugins::open_page("trailhead", "prompts", &trek.ws, cx));
        trek.render(cx);
        assert!(has_view(cx));
        cx.update(|cx| trek_app::plugins::set_on("trailhead", false, &trek.ws, cx));
        trek.render(cx);
        assert!(!has_view(cx));
        // The page says why it's gone, with the way to Settings.
        let why = cx.update(|cx| trek_app::plugins::page::why_closed("trailhead", cx));
        assert_eq!(why, "Trailhead is off.");
        assert!(trek.visible(cx, "plugin-page-settings"));
        // Stopped by Trek, it says why.
        cx.update(|cx| trek_app::plugins::set_on("trailhead", true, &trek.ws, cx));
        trek.render(cx);
        assert!(has_view(cx));
        cx.update(|cx| {
            trek_app::plugins::stop("trailhead", "TypeError: x is undefined", &trek.ws, cx)
        });
        trek.render(cx);
        let why = cx.update(|cx| trek_app::plugins::page::why_closed("trailhead", cx));
        assert_eq!(why, "Trailhead was stopped: TypeError: x is undefined");
        trek.click(cx, "plugin-page-settings");
        assert!(trek.read(cx, |ws, _| matches!(
            ws.route,
            Route::Settings(trek_app::workspace::SettingsPage::Plugins)
        )));
    });
}

#[test]
fn the_switch_asks_before_it_allows_what_a_plugin_asks_for() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("now-playing", &trek, cx);
        install("focus-timer", &trek, cx);
        let running =
            |id: &str, cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running(id, cx));
        let ws = trek.ws.clone();

        // Now playing asks to read and open threads: its switch opens a dialog and turns
        // nothing on.
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("now-playing", true, &ws, window, cx)
        });
        cx.run_until_parked();
        assert!(!running("now-playing", cx), "nothing allowed yet");
        assert!(trek.window(cx, |window, cx| window.has_active_dialog(cx)));
        trek.window(cx, |window, cx| window.close_all_dialogs(cx));

        // The timer asks for nothing: its switch is at once, off and on.
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("focus-timer", false, &ws, window, cx)
        });
        assert!(!running("focus-timer", cx));
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("focus-timer", true, &ws, window, cx)
        });
        assert!(running("focus-timer", cx));
        assert!(!trek.window(cx, |window, cx| window.has_active_dialog(cx)));

        // Allowed once, its switch is at once too.
        cx.update(|cx| trek_app::plugins::set_on("now-playing", true, &ws, cx));
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("now-playing", false, &ws, window, cx)
        });
        assert!(!running("now-playing", cx));
    });
}

#[test]
fn a_plugin_too_big_to_seal_says_so_and_waits() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let dir = trek_core::plugins::dir().join("big");
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"Big\"\npermissions = [\"threads:read\"]\n",
        )
        .unwrap();
        for i in 0..=trek_core::plugins::MAX_FILES {
            std::fs::write(dir.join(format!("ui/{i}.json")), "{}").unwrap();
        }
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.update(|cx| trek_app::plugins::set_on("big", true, &trek.ws, cx));
        cx.update(|cx| {
            assert!(!trek_app::plugins::running("big", cx));
            let p = cx.global::<trek_app::plugins::Plugins>();
            let plugin = p.find("big").expect("read");
            assert!(p.state.choice("big").waiting);
            let said = trek_app::plugins::cant_allow(plugin);
            assert!(said.contains("more than 500 files"), "{said}");
            assert!(!said.contains("key"), "{said}");
        });
    });
}

#[test]
fn an_allowed_plugin_s_logic_edited_since_doesn_t_run_before_it_asks_again() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let dir = trek_core::plugins::dir().join("nosy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"Nosy\"\npermissions = [\"threads:read\"]\nmain = \"main.js\"\n\
             [[command]]\nid = \"go\"\ntitle = \"Go\"\nrun = \"go\"\n",
        )
        .unwrap();
        let main = |v: u32| format!("trek.command(\"go\", () => trek.state.set({{ v: {v} }}));\n");
        std::fs::write(dir.join("main.js"), main(1)).unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.update(|cx| trek_app::plugins::set_on("nosy", true, &trek.ws, cx));
        let ws = trek.ws.clone();
        let go = |cx: &mut TestAppContext| {
            trek.window(cx, |window, cx| {
                trek_app::plugins::run_command("nosy", "go", &ws, window, cx)
            });
            cx.run_until_parked();
            cx.update(|cx| {
                cx.global::<trek_app::plugins::Plugins>()
                    .states
                    .get("nosy")
                    .and_then(|s| s.get("v"))
                    .and_then(|v| v.as_u64())
            })
        };
        assert_eq!(go(cx), Some(1), "as allowed, it runs");

        // Its logic is rewritten before Trek looks at the folder again: it doesn't run.
        std::fs::write(dir.join("main.js"), main(2)).unwrap();
        assert_eq!(go(cx), Some(1), "the edited logic didn't run");
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.update(|cx| assert!(!trek_app::plugins::running("nosy", cx), "it asks again"));
    });
}

#[test]
fn a_plugin_key_that_is_trek_s_own_stays_trek_s() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let dir = trek_core::plugins::dir().join("keys");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"Keys\"\n\
             [[command]]\nid = \"mine\"\ntitle = \"Mine\"\nkey = \"cmd-alt-j\"\naction = \"set:hits=1\"\n\
             [[command]]\nid = \"theirs\"\ntitle = \"Theirs\"\nkey = \"cmd-n\"\naction = \"set:hits=2\"\n",
        )
        .unwrap();
        std::fs::write(
            trek_core::plugins::dir().join(trek_core::plugins::STATE_FILE),
            "[plugins.keys]\non = true\n",
        )
        .unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.update(|cx| {
            assert!(trek_app::plugins::running("keys", cx));
            // cmd-n is Trek's new thread: the plugin's command keeps its palette row, keyless.
            let keys: Vec<(String, Option<String>)> = trek_app::plugins::palette_entries(cx)
                .into_iter()
                .filter_map(|e| match e {
                    trek_app::plugins::PaletteEntry::Command { command, key, .. } => {
                        Some((command, key))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                keys,
                [
                    ("mine".to_string(), Some("cmd-alt-j".to_string())),
                    ("theirs".to_string(), None)
                ]
            );
            assert!(trek_app::plugins::free_key("p", "c", Some("cmd-n"), cx).is_none());
            assert!(trek_app::plugins::free_key("p", "c", Some("cmd-alt-j"), cx).is_some());
        });
    });
}

#[test]
fn a_plugin_s_hiker_walks_while_an_agent_works_and_stands_when_none_does() {
    // `trek:hiker` is "the hiker on a dotted trail, walking while an agent works". It was drawn as
    // a hiker standing at the middle of a short trail, whatever the agent did.
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        use std::time::Duration;
        use trek_agents::AgentEvent;
        let trek = start(cx);
        let dir = trek_core::plugins::dir().join("stroll");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "name = \"Stroll\"\n[[slot]]\nat = \"sidebar.top\"\nui = { type = \"trek:hiker\" }\n",
        )
        .unwrap();
        std::fs::write(
            trek_core::plugins::dir().join(trek_core::plugins::STATE_FILE),
            "[plugins.stroll]\non = true\n",
        )
        .unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.update(|cx| {
            assert!(trek_app::plugins::running("stroll", cx));
            cx.set_reduce_motion(false)
        });
        trek.window(cx, |window, _| window.activate_window());
        let id = trek.quiet_thread(cx);
        trek.render(cx);
        let walks = |cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::hiker::walks(cx));
        // The clock moves in frames, a render after each, as the window draws them.
        let watch = |trek: &common::Trek, cx: &mut TestAppContext, secs: u32| {
            let mut at = Vec::new();
            for _ in 0..secs * 16 {
                cx.executor().advance_clock(Duration::from_millis(62));
                trek.render(cx);
                at.push(walks(cx).last().copied().expect("a hiker is drawn"));
            }
            at
        };

        // Nothing is working: it stands where it set off.
        let idle = watch(&trek, cx, 3);
        assert!(
            idle.iter().all(|a| *a == 0.),
            "it moved with no one working: {idle:?}"
        );

        // An agent works: it sets off, and keeps going for as long as it does.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![AgentEvent::ToolStarted {
                    id: "t1".into(),
                    title: "Run command".into(),
                    detail: "cargo build".into(),
                }],
                cx,
            )
        });
        let working = watch(&trek, cx, 6);
        let moved = |walk: &[f32]| walk.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(moved(&working) > 30, "it should walk: {working:?}");
        let again = watch(&trek, cx, 2);
        assert!(moved(&again) > 10, "and carry on: {again:?}");

        // The turn is over: it stands where it is, and stays there.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolFinished {
                        id: "t1".into(),
                        output: String::new(),
                        ok: true,
                    },
                    AgentEvent::TextDone("Done.".into()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        trek.render(cx);
        let stopped = watch(&trek, cx, 2);
        assert_eq!(
            moved(&stopped),
            0,
            "it stands once the work is done: {stopped:?}"
        );
    });
}

/// A plugin's state, as its trees read it.
fn state_of(id: &str, cx: &mut TestAppContext) -> serde_json::Value {
    cx.update(|cx| {
        cx.global::<trek_app::plugins::Plugins>()
            .states
            .get(id)
            .cloned()
            .unwrap_or_default()
    })
}

/// Wait (on the test's clock, a few tenths of a second at a time) until `until` holds of plugin
/// `id`'s state: its logic runs after Trek's events settle, off the UI thread.
async fn until_state(
    id: &str,
    what: &str,
    cx: &mut TestAppContext,
    until: impl Fn(&serde_json::Value) -> bool,
) {
    for _ in 0..200 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.run_until_parked();
        if until(&state_of(id, cx)) {
            return;
        }
        cx.background_executor
            .timer(std::time::Duration::from_millis(5))
            .await;
    }
    panic!("{id}: timed out waiting for {what}: {}", state_of(id, cx));
}

/// The focus walk: its time steps through the lengths before a walk, its key starts, pauses and
/// carries on, a walk ends offering a break, and a walk under way outlasts a restart by the
/// wall clock (and one whose time ran out while Trek was closed is done).
#[test]
fn the_focus_walk_steps_its_length_pauses_offers_a_break_and_outlasts_a_restart() {
    use std::time::Duration;
    use trek_app::plugins::focus::{Focus, Mode, Session, control};
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("focus-timer", &trek, cx);
        draft(&trek, cx);
        assert!(trek.visible(cx, "focus-chip"));
        assert!(trek.visible(cx, "focus-length"), "the time is a control");
        let f = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let f = cx.global::<Focus>();
                (f.minutes, f.mode, f.session)
            })
        };
        // 25 → 45 → 60 → 15 → 25, before a walk.
        let mut lengths = vec![];
        for _ in 0..4 {
            trek.click(cx, "focus-length");
            lengths.push(f(cx).0);
        }
        assert_eq!(lengths, [45., 60., 15., 25.]);

        // The key's command: start, pause, carry on.
        cx.update(|cx| control("toggle", &trek.ws, cx));
        assert!(matches!(f(cx).2, Session::Running { .. }));
        cx.update(|cx| control("toggle", &trek.ws, cx));
        assert!(matches!(f(cx).2, Session::Paused { .. }));
        cx.update(|cx| control("toggle", &trek.ws, cx));
        assert!(matches!(f(cx).2, Session::Running { .. }));

        // Time's up: done, with a break on offer; the break is a walk of its own, in its mode.
        cx.update(|cx| trek_app::plugins::focus::jump(25. * 60. - 1., &trek.ws, cx));
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(f(cx).2, Session::Done);
        trek.render(cx);
        assert!(trek.visible(cx, "focus-break"));
        trek.click(cx, "focus-break");
        assert!(matches!(f(cx), (_, Mode::Break, Session::Running { .. })));
        cx.executor().advance_clock(Duration::from_secs(5 * 60 + 1));
        cx.run_until_parked();
        assert_eq!(f(cx).1, Mode::Focus);
        assert_eq!(
            f(cx).2,
            Session::Ready,
            "the break over, ready for the next walk"
        );

        // Kept by the wall clock: ten minutes in at quit, back two minutes later, twelve in.
        let now = cx.update(|cx| trek_app::motion::now(cx));
        let mut walk = Focus::new(25., 5.);
        walk.take_up(
            &serde_json::json!({"mode": "focus", "minutes": 25, "state": "running", "walked_ms": 600_000, "at_ms": 1_000_000}),
            now,
            1_120_000,
        );
        assert_eq!(walk.left(now), 13 * 60);
        // Paused stays paused, wherever the clock got to.
        walk.take_up(
            &serde_json::json!({"mode": "focus", "minutes": 45, "state": "paused", "walked_ms": 60_000, "at_ms": 0}),
            now,
            9_000_000,
        );
        assert_eq!(
            walk.session,
            Session::Paused {
                walked: Duration::from_secs(60)
            }
        );
        assert_eq!(walk.minutes, 45.);
        // Its time ran out while Trek was closed: done.
        walk.take_up(
            &serde_json::json!({"mode": "focus", "minutes": 25, "state": "running", "walked_ms": 600_000, "at_ms": 0}),
            now,
            3_600_000,
        );
        assert_eq!(walk.session, Session::Done);
        // And what's kept reads back the same.
        let kept = walk.kept(now, 42);
        walk.take_up(&kept, now, 42);
        assert_eq!(walk.session, Session::Done);
    });
}

/// Pip the marmot hears agents' turns (by thread id alone: it asks for no permissions): it keeps
/// watch while one works and cheers when the turn is over; Soft sounds plays its own sound for
/// the same moment.
#[test]
fn the_marmot_and_soft_sounds_hear_an_agent_work_and_finish() {
    use trek_agents::AgentEvent;
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("marmot", &trek, cx);
        install("soft-sounds", &trek, cx);
        cx.update(|cx| {
            assert!(trek_app::plugins::running("marmot", cx));
            assert!(trek_app::plugins::running("soft-sounds", cx));
            assert_eq!(trek_app::plugins::slot(Slot::SidebarFoot, cx).len(), 1);
        });
        until_state("marmot", "a nap at launch", cx, |s| s["mood"] == "asleep").await;
        let sounds = || trek_app::system::SOUNDS.with(|n| n.get());
        let before = sounds();

        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![AgentEvent::ToolStarted {
                    id: "t1".into(),
                    title: "Run command".into(),
                    detail: "cargo build".into(),
                }],
                cx,
            )
        });
        until_state("marmot", "keeping watch", cx, |s| s["mood"] == "working").await;
        assert_eq!(state_of("marmot", cx)["line"], "Keeping watch over 1 agent");

        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::ToolFinished {
                        id: "t1".into(),
                        output: String::new(),
                        ok: true,
                    },
                    AgentEvent::TextDone("Done.".into()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        until_state("marmot", "a cheer", cx, |s| s["mood"] == "cheering").await;
        let log = trek_core::plugins::log_tail("soft-sounds", 5).join("\n");
        assert!(log.contains("played sounds/chime.wav"), "{log}");
        assert!(sounds() > before, "the finish sound");

        // A pat settles it down.
        trek.window(cx, |window, cx| {
            trek_app::plugins::run_command("marmot", "pat", &trek.ws, window, cx)
        });
        until_state("marmot", "a nap after a pat", cx, |s| s["mood"] == "asleep").await;
    });
}

/// Trailhead's prompts: its logic puts the kept ones in its state when Trek starts, and its
/// page's `trek.state.set` changes what the screen's chips show. Its greeting follows the hour.
#[test]
fn trailheads_prompts_come_from_its_logic_and_its_page() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        trek_core::plugins::set_storage(
            "trailhead",
            serde_json::json!({"prompts": ["Plan my day", "Review my last commit"]})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        install("trailhead", &trek, cx);
        until_state("trailhead", "the kept prompts", cx, |s| {
            s["prompts"] == serde_json::json!(["Plan my day", "Review my last commit"])
        })
        .await;
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(
                Surface::NewThread,
                Some("trailhead"),
                &trek.ws,
                cx,
            )
        });
        draft(&trek, cx);
        assert!(trek.visible(cx, "plugin-surface-new-thread"));
        // As its page sets it (the bridge's state.set merges in the same way).
        cx.update(|cx| {
            trek_app::plugins::set_state(
                "trailhead",
                serde_json::json!({"prompts": ["Only this one"]}),
                &trek.ws,
                cx,
            )
        });
        assert_eq!(
            state_of("trailhead", cx)["prompts"],
            serde_json::json!(["Only this one"])
        );
        let part = |h: u32| {
            trek_app::plugins::render::now(
                chrono::NaiveDate::from_ymd_opt(2026, 11, 17)
                    .unwrap()
                    .and_hms_opt(h, 0, 0)
                    .unwrap(),
            )["part"]
                .clone()
        };
        assert_eq!(
            [part(6), part(13), part(19), part(23), part(3)],
            ["morning", "afternoon", "evening", "night", "night"]
        );
    });
}
