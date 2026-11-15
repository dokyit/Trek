//! Plugins in the app: with none installed every place is empty; a plugin that's on draws in its
//! place and one that's off doesn't; a replacement stands in for Trek's surface until its plugin
//! stops, and then Trek's own is back; a page's web view exists only while the page is open, and
//! gives the keyboard back before it goes; a typed-in draft keeps taking keys after a page closes.

mod common;

use common::{open, run};
use gpui_kit::TestAppContext;
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
    });
}
