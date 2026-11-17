//! Plugins in the app: with none installed every place is empty; a plugin that's on draws in its
//! place and one that's off doesn't; a replacement stands in for Trek's surface until its plugin
//! stops, and then Trek's own is back; a page's web view exists only while the page is open, and
//! gives the keyboard back before it goes; a typed-in draft keeps taking keys after a page closes.

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
        // The board asks to read threads and open them: installed from the CLI it waits.
        install("trail-board", &trek, cx);
        cx.update(|cx| {
            assert!(!trek_app::plugins::running("trail-board", cx));
            assert!(trek_app::plugins::replacement(Surface::ThreadList, cx).is_none());
        });
        draft(&trek, cx);
        assert!(!trek.visible(cx, "plugin-surface-thread-list"));

        // The user turns it on in Settings (allowing what it asks): its board is the thread list.
        cx.update(|cx| trek_app::plugins::set_on("trail-board", true, &trek.ws, cx));
        draft(&trek, cx);
        cx.update(|cx| {
            assert!(trek_app::plugins::running("trail-board", cx));
            assert!(trek_app::plugins::replacement(Surface::ThreadList, cx).is_some());
        });
        assert!(trek.visible(cx, "plugin-surface-thread-list"));

        // Settings puts Trek's own back, and the plugin's again.
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(Surface::ThreadList, None, &trek.ws, cx)
        });
        draft(&trek, cx);
        assert!(!trek.visible(cx, "plugin-surface-thread-list"));
        cx.update(|cx| {
            trek_app::plugins::choose_replacement(
                Surface::ThreadList,
                Some("trail-board"),
                &trek.ws,
                cx,
            )
        });
        draft(&trek, cx);
        assert!(trek.visible(cx, "plugin-surface-thread-list"));

        // It fails: only it stops, the list is Trek's own again, and Settings says why.
        cx.update(|cx| trek_app::plugins::stop("trail-board", "TypeError: x", &trek.ws, cx));
        draft(&trek, cx);
        cx.update(|cx| {
            assert!(!trek_app::plugins::running("trail-board", cx));
            assert!(trek_app::plugins::replacement(Surface::ThreadList, cx).is_none());
            let p = cx.global::<trek_app::plugins::Plugins>();
            assert_eq!(
                p.state.choice("trail-board").stopped.as_deref(),
                Some("TypeError: x")
            );
        });
        assert!(!trek.visible(cx, "plugin-surface-thread-list"));
    });
}

#[test]
fn a_grant_written_into_the_file_is_ignored_and_an_edited_plugin_asks_again() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("trail-board", &trek, cx);
        let dir = trek_core::plugins::dir();
        let running =
            |cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running("trail-board", cx));
        let waiting = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                cx.global::<trek_app::plugins::Plugins>()
                    .state
                    .choice("trail-board")
                    .waiting
            })
        };
        // An agent writes the grant into state.toml itself: Trek doesn't take it.
        std::fs::write(
            dir.join(trek_core::plugins::STATE_FILE),
            "[plugins.trail-board]\non = true\ngranted = [\"threads:read\", \"threads:open\"]\nseal = \"0123abcd\"\n\n[replace]\nthread-list = \"trail-board\"\n",
        )
        .unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(!running(cx), "a hand-written grant is no grant");
        assert!(waiting(cx), "it waits for the user");
        draft(&trek, cx);
        assert!(!trek.visible(cx, "plugin-surface-thread-list"));

        // The user allows it in Settings: it runs, and the seal Trek wrote is good at the next
        // read of the folder.
        cx.update(|cx| trek_app::plugins::set_on("trail-board", true, &trek.ws, cx));
        assert!(running(cx));
        let kept = std::fs::read_to_string(dir.join(trek_core::plugins::STATE_FILE)).unwrap();
        assert!(kept.contains("seal = "), "{kept}");
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(running(cx), "sealed, it stays on");

        // Its files change (an update, or another plugin put in its place): it asks again.
        let board = dir.join("trail-board/ui/board.json");
        let text = std::fs::read_to_string(&board).unwrap();
        std::fs::write(&board, text.replace("Needs you", "Needs you now")).unwrap();
        cx.update(|cx| trek_app::plugins::reread(&trek.ws, cx));
        cx.run_until_parked();
        assert!(!running(cx), "the grant was for the plugin the user saw");
        assert!(waiting(cx));
        draft(&trek, cx);
        assert!(!trek.visible(cx, "plugin-surface-thread-list"));
    });
}

#[test]
fn a_page_s_view_lives_while_its_open_and_gives_the_keys_back_before_it_goes() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = start(cx);
        install("summit-log", &trek, cx);
        cx.update(|cx| trek_app::plugins::set_on("summit-log", true, &trek.ws, cx));
        draft(&trek, cx);
        let has_view = |cx: &mut TestAppContext| {
            trek.root
                .read_with(cx, |r, cx| r.page_host.read(cx).has_view())
        };
        assert!(!has_view(cx), "no page open, no web view");
        let _ = trek_app::plugins::page::fake::take();

        // The page opens: its view is made and shown.
        cx.update(|cx| trek_app::plugins::open_page("summit-log", "summit-log", &trek.ws, cx));
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
        cx.update(|cx| trek_app::plugins::open_page("summit-log", "summit-log", &trek.ws, cx));
        trek.render(cx);
        assert!(has_view(cx));
        cx.update(|cx| trek_app::plugins::set_on("summit-log", false, &trek.ws, cx));
        trek.render(cx);
        assert!(!has_view(cx));
        // The page says why it's gone, with the way to Settings.
        let why = cx.update(|cx| trek_app::plugins::page::why_closed("summit-log", cx));
        assert_eq!(why, "Summit log is off.");
        assert!(trek.visible(cx, "plugin-page-settings"));
        // Stopped by Trek, it says why.
        cx.update(|cx| trek_app::plugins::set_on("summit-log", true, &trek.ws, cx));
        trek.render(cx);
        assert!(has_view(cx));
        cx.update(|cx| {
            trek_app::plugins::stop("summit-log", "TypeError: x is undefined", &trek.ws, cx)
        });
        trek.render(cx);
        let why = cx.update(|cx| trek_app::plugins::page::why_closed("summit-log", cx));
        assert_eq!(why, "Summit log was stopped: TypeError: x is undefined");
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
        install("trail-board", &trek, cx);
        install("focus-timer", &trek, cx);
        let running =
            |id: &str, cx: &mut TestAppContext| cx.update(|cx| trek_app::plugins::running(id, cx));
        let ws = trek.ws.clone();

        // The board asks to read and open threads: its switch opens a dialog and turns
        // nothing on.
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("trail-board", true, &ws, window, cx)
        });
        cx.run_until_parked();
        assert!(!running("trail-board", cx), "nothing allowed yet");
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

        // Allowed once, the board's switch is at once too.
        cx.update(|cx| trek_app::plugins::set_on("trail-board", true, &ws, cx));
        trek.window(cx, |window, cx| {
            trek_app::plugins::switch("trail-board", false, &ws, window, cx)
        });
        assert!(!running("trail-board", cx));
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
            assert!(trek_app::plugins::free_key("cmd-n", cx).is_none());
            assert!(trek_app::plugins::free_key("cmd-alt-j", cx).is_some());
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
