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
                    // The manifest's `cmd` as the platform has it: Ctrl off the Mac.
                    (
                        "mine".to_string(),
                        Some(trek_app::keys::manifest_keystroke(
                            "cmd-alt-j",
                            trek_app::keys::is_mac()
                        ))
                    ),
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

// ---------- what an agent (or anyone) creates to customise Trek ----------
//
// An agent writes a folder into the plugins folder, as the trek-plugin skill tells it to. Trek
// notices without a restart, lists it, says it was added, and the user's switch turns it off and
// on at once; a look is on only while it's the one on screen; Delete takes its files.

use trek_core::plugins as core;

/// A throwaway agent at work in a thread of the harness project, as a mock agent would be.
fn agent_at_work(trek: &common::Trek, cx: &mut TestAppContext, title: &str) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(
                Some(&trek.project),
                common::mock(),
                None,
                trek_core::Effort::Medium,
                trek_core::HandHolding::Auto,
            )
            .expect("thread");
        t.title = title.into();
        t.run_state = trek_core::RunState::Working;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

/// Pip, as Codex wrote it: a marmot at the foot of the sidebar, no permissions.
fn write_pip(permissions: &str) {
    let dir = core::dir().join("pip");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "name = \"Pip\"\ndescription = \"A marmot at the foot of the sidebar.\"\nauthor = \"Codex\"\npermissions = {permissions}\n\n[[slot]]\nat = \"sidebar.foot\"\nui = {{ type = \"text\", text = \"pip\" }}\n"
        ),
    )
    .unwrap();
}

/// What the watch does on its tick: look at the folder, and read it again if it changed.
fn watch_tick(trek: &common::Trek, cx: &mut TestAppContext) {
    let dir = core::dir();
    let (print, asked) = trek_app::plugins::look(&dir);
    cx.update(|cx| trek_app::plugins::looked(print, asked, &trek.ws, cx));
    cx.run_until_parked();
}

fn state_on_disk() -> core::State {
    core::State::read(&core::dir())
}

fn listed(id: &str, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| cx.global::<trek_app::plugins::Plugins>().find(id).is_some())
}

/// A look's file, a dusky violet, as `trek-theme` writes one.
const DUSK: &str = "name = \"Dusk\"\ndescription = \"Violet evening light.\"\nmode = \"dark\"\n\n[dark]\naccent = \"#B58CFF\"\non_accent = \"#10081F\"\nwindow = \"#14101F\"\nsidebar = \"#171226\"\nsurface = \"#1E1830\"\nraised = \"#1A1529\"\ntext = \"#E6DFF5\"\nmuted = \"#9A8FB8\"\nborder = \"#2A2340\"\n";

/// A look as a plugin: the folder `trek-theme` says to write.
fn write_dusk_plugin() {
    let dir = core::dir().join("dusk");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "id = \"dusk\"\nname = \"Dusk\"\ndescription = \"Violet evening light.\"\ntrek = \">=0.4.55\"\n\n[[theme]]\nfile = \"look.toml\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("look.toml"), DUSK).unwrap();
}

fn enable_from_cli(id: &str) {
    let mut out = Vec::new();
    let code = core::cli::run(&["enable".into(), id.into()], &core::dir(), &mut out);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
}

#[test]
fn a_folder_an_agent_writes_is_picked_up_listed_announced_and_switches_live() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let toasts = common::toasts(&trek, cx);
        let _thread = agent_at_work(&trek, cx, "Add a marmot");

        // The agent writes the folder and turns it on in state.toml: it asks for nothing, so
        // that's all it takes. No restart, no click.
        write_pip("[]");
        std::fs::write(
            core::dir().join(core::STATE_FILE),
            "[plugins.pip]\non = true\n",
        )
        .unwrap();
        let before = cx.update(|cx| cx.global::<trek_app::plugins::Plugins>().generation);
        watch_tick(&trek, cx);
        assert!(listed("pip", cx), "it's in Settings › Plugins");
        assert!(cx.update(|cx| trek_app::plugins::running("pip", cx)));
        assert_eq!(
            cx.update(|cx| trek_app::plugins::slot(Slot::SidebarFoot, cx).len()),
            1
        );
        assert!(cx.update(|cx| cx.global::<trek_app::plugins::Plugins>().generation) > before);

        // The user hears of it, with where to manage it and which agent was at work.
        let told = toasts.borrow().clone();
        let (message, undo) = told
            .iter()
            .find(|(m, _)| m.contains("Pip was added"))
            .unwrap_or_else(|| panic!("no announcement in {told:?}"));
        assert!(message.contains("Settings › Plugins"), "{message}");
        assert!(message.contains("is on"), "{message}");
        assert!(
            message.contains(" by "),
            "the agent at work is named: {message}"
        );
        assert!(matches!(
            undo,
            Some(trek_app::workspace::UndoAction::OpenPlugins)
        ));
        // And it's noted for the row: when, and who.
        let added = state_on_disk().choice("pip").added.expect("noted");
        assert!(
            added.at > 0 && added.by.is_some() && added.title.as_deref() == Some("Add a marmot")
        );
        // A second look at the same folder says nothing more.
        let count = toasts.borrow().len();
        watch_tick(&trek, cx);
        assert_eq!(toasts.borrow().len(), count);

        // Off is gone at once, and the file says so; on is back at once, with no restart.
        cx.update(|cx| trek_app::plugins::set_on("pip", false, &trek.ws, cx));
        assert!(cx.update(|cx| trek_app::plugins::slot(Slot::SidebarFoot, cx).is_empty()));
        assert!(!state_on_disk().choice("pip").on);
        assert!(
            listed("pip", cx),
            "off is still listed, for turning on again"
        );
        cx.update(|cx| trek_app::plugins::set_on("pip", true, &trek.ws, cx));
        assert_eq!(
            cx.update(|cx| trek_app::plugins::slot(Slot::SidebarFoot, cx).len()),
            1
        );
        assert!(state_on_disk().choice("pip").on);

        // Delete takes the folder and what it kept.
        core::log("pip", "something");
        cx.update(|cx| trek_app::plugins::remove("pip", &trek.ws, cx))
            .unwrap();
        assert!(!listed("pip", cx));
        assert!(!core::dir().join("pip").exists());
        assert!(!core::data_dir("pip").exists());
        assert!(cx.update(|cx| trek_app::plugins::slot(Slot::SidebarFoot, cx).is_empty()));
    });
}

#[test]
fn the_plugins_page_lists_what_an_agent_made_with_its_switch_and_who_made_it() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let _thread = agent_at_work(&trek, cx, "Add a marmot");
        write_pip("[]");
        enable_from_cli("pip");
        watch_tick(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Settings(trek_app::workspace::SettingsPage::Plugins),
                cx,
            )
        });
        trek.render(cx);
        assert!(trek.visible(cx, "plugin-row-pip"));
        assert!(trek.visible(cx, "plugin-added-pip"), "when and by whom");
        // Its switch is on, and a click turns it off at once.
        assert!(cx.update(|cx| trek_app::plugins::running("pip", cx)));
        trek.click(cx, "plugin-on-pip");
        trek.render(cx);
        assert!(!cx.update(|cx| trek_app::plugins::running("pip", cx)));
        assert!(trek.visible(cx, "plugin-row-pip"), "still listed");
        trek.click(cx, "plugin-on-pip");
        assert!(cx.update(|cx| trek_app::plugins::running("pip", cx)));
    });
}

#[test]
fn a_plugin_that_asks_for_permissions_is_announced_and_waits_for_the_user() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let toasts = common::toasts(&trek, cx);
        // The agent writes a plugin that reads threads, and tries to grant itself by hand.
        write_pip("[\"threads:read\"]");
        std::fs::write(
            core::dir().join(core::STATE_FILE),
            "[plugins.pip]\non = true\ngranted = [\"threads:read\"]\nseal = \"abcdef\"\n",
        )
        .unwrap();
        watch_tick(&trek, cx);
        assert!(listed("pip", cx));
        assert!(
            !cx.update(|cx| trek_app::plugins::running("pip", cx)),
            "never granted by the agent"
        );
        assert!(cx.update(|cx| trek_app::plugins::slot(Slot::SidebarFoot, cx).is_empty()));
        let told = toasts.borrow().clone();
        assert!(
            told.iter()
                .any(|(m, _)| m.contains("Pip was added") && m.contains("asks for permissions")),
            "{told:?}"
        );
        // The user's Allow in Trek is what runs it.
        cx.update(|cx| trek_app::plugins::set_on("pip", true, &trek.ws, cx));
        assert!(cx.update(|cx| trek_app::plugins::running("pip", cx)));
    });
}

#[test]
fn a_plugins_look_is_off_when_another_look_is_chosen_and_comes_back_when_it_is_turned_on() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        let running =
            |cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running("dusk", cx));
        let shown = |cx: &mut TestAppContext| cx.update(|cx| trek_app::themes::shown_id(cx));
        let chosen = |cx: &mut TestAppContext| {
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone())
        };

        // The agent writes the plugin that carries the look, turns it on and asks for it.
        write_dusk_plugin();
        enable_from_cli("dusk");
        std::fs::create_dir_all(trek_core::themes::dir()).unwrap();
        std::fs::write(
            trek_core::themes::dir().join(trek_core::themes::APPLY_FILE),
            "dusk\n",
        )
        .unwrap();
        // The two folders are looked at on one tick, the themes first: the request waits for
        // the plugin that brings the look instead of being refused.
        cx.update(|cx| trek_app::themes::reread(&trek.ws, cx));
        assert_eq!(shown(cx), "trek", "not known yet");
        watch_tick(&trek, cx);
        assert!(running(cx));
        assert_eq!(shown(cx), "dusk", "the look the agent asked for is up");
        assert_eq!(chosen(cx), "dusk");
        assert!(cx.update(|cx| {
            trek_app::plugins::look_in_use(
                &cx.global::<trek_app::plugins::Plugins>()
                    .find("dusk")
                    .unwrap()
                    .manifest,
                cx,
            )
        }));

        // The user picks a built-in theme: it shows, and the plugin's look is off, not fighting.
        cx.update(|cx| trek_app::themes::switch_to("console", &trek.ws, cx));
        assert_eq!(shown(cx), "console");
        assert!(!running(cx), "one look on at a time");
        assert!(!state_on_disk().choice("dusk").on);
        assert_eq!(state_on_disk().choice("dusk").look.as_deref(), Some("dusk"));
        assert!(
            cx.update(|cx| trek_app::themes::find("dusk", cx)).is_none(),
            "an off plugin's look isn't on offer"
        );
        assert!(
            listed("dusk", cx),
            "but the plugin is still there to turn on"
        );

        // Settings › Plugins says so, and turning it back on brings the look back.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Settings(trek_app::workspace::SettingsPage::Plugins),
                cx,
            )
        });
        trek.render(cx);
        assert!(
            trek.visible(cx, "plugin-look-dusk"),
            "its look is shown as off"
        );
        cx.update(|cx| trek_app::plugins::set_on("dusk", true, &trek.ws, cx));
        assert!(running(cx));
        assert_eq!(shown(cx), "dusk");
        assert_eq!(chosen(cx), "dusk");
        assert_eq!(state_on_disk().choice("dusk").look, None);

        // Another plugin's look, or turning it off, leaves nothing half on.
        cx.update(|cx| trek_app::plugins::set_on("dusk", false, &trek.ws, cx));
        assert_eq!(shown(cx), "trek", "Trek's own is back when the look goes");
        assert!(!running(cx));

        // Delete: the folder goes, and Trek's own stays.
        cx.update(|cx| trek_app::plugins::set_on("dusk", true, &trek.ws, cx));
        assert_eq!(shown(cx), "dusk");
        cx.update(|cx| trek_app::plugins::remove("dusk", &trek.ws, cx))
            .unwrap();
        assert!(!core::dir().join("dusk").exists());
        assert_eq!(shown(cx), "trek");
    });
}

/// With motion, a switch's frames can come late (Linux CI draws in software): the first after
/// the start may find the switch over. The plugin's look still goes into the settings, another
/// look chosen over it still turns it off, and so does one chosen before its switch's middle.
#[test]
fn a_plugins_look_keeps_its_settings_and_its_switch_when_the_frames_come_late() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        cx.update(|cx| cx.set_reduce_motion(false));
        let running =
            |cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running("dusk", cx));
        let shown = |cx: &mut TestAppContext| cx.update(|cx| trek_app::themes::shown_id(cx));
        let chosen = |cx: &mut TestAppContext| {
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone())
        };
        let late = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                trek_app::themes::frame_late(std::time::Duration::from_secs(2), &trek.ws, cx)
            });
            cx.run_until_parked();
        };
        let on_disk = || trek_core::settings::Settings::load().appearance.trek_theme;

        write_dusk_plugin();
        watch_tick(&trek, cx);
        assert!(listed("dusk", cx));

        // Turned on: its look comes up, and the settings keep it.
        cx.update(|cx| trek_app::plugins::set_on("dusk", true, &trek.ws, cx));
        late(cx);
        assert_eq!(shown(cx), "dusk");
        assert_eq!(chosen(cx), "dusk", "the settings name the look on screen");
        assert_eq!(on_disk(), "dusk");

        // Another look over it: the plugin is off, the other look kept.
        cx.update(|cx| trek_app::themes::switch_to("console", &trek.ws, cx));
        late(cx);
        assert_eq!(shown(cx), "console");
        assert_eq!(chosen(cx), "console");
        assert!(
            !running(cx),
            "a plugin's look is on only while it's on screen"
        );
        assert_eq!(state_on_disk().choice("dusk").look.as_deref(), Some("dusk"));

        // Back on, then another look before its switch's middle: off again, nothing half on.
        cx.update(|cx| trek_app::plugins::set_on("dusk", true, &trek.ws, cx));
        assert!(running(cx));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        cx.update(|cx| trek_app::themes::switch_to("console", &trek.ws, cx));
        late(cx);
        assert_eq!(shown(cx), "console");
        assert_eq!(chosen(cx), "console");
        assert_eq!(on_disk(), "console");
        assert!(!running(cx), "its look never got on screen");
    });
}

#[test]
fn a_loose_theme_file_in_the_themes_folder_is_listed_switches_and_deletes_like_a_plugin() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        // A "Dusk" an agent made last week, before looks travelled in plugins.
        let themes = trek_core::themes::dir();
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("dusk-old.toml"), DUSK).unwrap();
        cx.update(|cx| trek_app::themes::reread(&trek.ws, cx));
        let loose = cx.update(|cx| trek_app::themes::loose_looks(cx));
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].id, "dusk-old");

        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Settings(trek_app::workspace::SettingsPage::Plugins),
                cx,
            )
        });
        trek.render(cx);
        assert!(
            trek.visible(cx, "look-row-dusk-old"),
            "listed beside the plugins"
        );
        // Its switch is its look being on screen: on is the look, off is Trek's own.
        trek.click(cx, "look-on-dusk-old");
        assert_eq!(cx.update(|cx| trek_app::themes::shown_id(cx)), "dusk-old");
        trek.render(cx);
        trek.click(cx, "look-on-dusk-old");
        assert_eq!(cx.update(|cx| trek_app::themes::shown_id(cx)), "trek");

        // Delete takes the file; one on screen gives way to Trek's own.
        cx.update(|cx| trek_app::themes::switch_to("dusk-old", &trek.ws, cx));
        cx.update(|cx| trek_app::themes::delete_look("dusk-old", &trek.ws, cx))
            .unwrap();
        assert!(!themes.join("dusk-old.toml").exists());
        assert_eq!(cx.update(|cx| trek_app::themes::shown_id(cx)), "trek");
        assert!(cx.update(|cx| trek_app::themes::loose_looks(cx)).is_empty());
        // A built-in one isn't deletable.
        assert!(
            cx.update(|cx| trek_app::themes::delete_look("console", &trek.ws, cx))
                .is_err()
        );
    });
}

#[test]
fn the_agents_are_told_trek_s_exact_folder_and_not_to_use_claude_codes() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // The note every request carries, and the skill as written for this computer.
    let note = core::agent_note();
    let plugins = core::dir().display().to_string();
    assert!(note.contains(&plugins), "{note}");
    assert!(
        note.contains("~/.claude") && note.contains("dev-mods"),
        "{note}"
    );
    let skill = trek_core::skills::shipped(core::SKILL).unwrap();
    let text = std::fs::read_to_string(skill).unwrap();
    let data = trek_core::paths::data_dir().display().to_string();
    assert!(
        !text.contains("<data>") && !text.contains("<trek>"),
        "filled in"
    );
    assert!(
        text.contains(&format!("{data}/plugins/<id>/")),
        "the exact folder"
    );
    assert!(text.contains("plugin enable <id>"), "the exact command");
    assert!(
        text.contains("Never write into `~/.claude`"),
        "and where not to write"
    );
    // The theme skill sends looks to the same place.
    let theme =
        std::fs::read_to_string(trek_core::skills::shipped(trek_core::themes::SKILL).unwrap())
            .unwrap();
    assert!(
        theme.contains(&format!("{data}/plugins/<id>/look.toml")),
        "{theme}"
    );
}
