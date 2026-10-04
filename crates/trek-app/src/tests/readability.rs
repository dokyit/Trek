//! How answers read: markdown in both themes (reasoning included), project colours on folders,
//! and trail words where an agent is at work.

use super::harness::{Trek, open, run};
use crate::workspace::{PanelTool, Route};
use gpui_kit::{TestAppContext, px};
use trek_agents::AgentEvent;
use trek_core::settings::ThemeChoice;
use trek_core::store::Item;

const THOUGHT: &str = "**Mapping the startup path**\n\nThe flags come first, in `src/cli.rs`, so a *bad flag* never opens a window.";

const ANSWER: &str = "## How the app starts\n\nStartup lives in `src/main.rs`. It **parses the flags**, then **loads the settings**.\n\n### Settings\n\n- **Flags** come from `src/cli.rs`.\n  - Unknown ones are an error.\n- The folder `src/` holds it all.\n\n| Setting | Default |\n| --- | --- |\n| `theme` | `night` |\n\n```rust\nfn main() {}\n```\n\nSee [the spec](https://toml.io).";

/// A finished turn in a fresh thread: a thought, then `ANSWER`. Returns the thread.
fn answered(trek: &Trek, cx: &mut TestAppContext) -> String {
    std::fs::create_dir_all(trek.project.join("src")).unwrap();
    for f in ["main.rs", "cli.rs"] {
        std::fs::write(trek.project.join("src").join(f), "// startup\n").unwrap();
    }
    let id = trek.quiet_thread(cx);
    let events = vec![
        AgentEvent::ReasoningDelta(THOUGHT.into()),
        AgentEvent::TextDelta(ANSWER.into()),
        AgentEvent::TextDone(ANSWER.into()),
        AgentEvent::TurnComplete { error: None },
    ];
    trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
    trek.render(cx);
    id
}

#[test]
fn answers_and_reasoning_render_as_markdown_in_both_themes() {
    run(async |cx| {
        let trek = open(cx);
        let id = answered(&trek, cx);
        let answer = trek.item_ix(cx, &id, |i| matches!(i, Item::Assistant { .. }));
        let thought = trek.item_ix(cx, &id, |i| matches!(i, Item::Reasoning { .. }));
        for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            trek.render(cx);
            assert!(trek.visible(cx, ("answer", answer)), "{choice:?}");
            // Its path chips: a file, and a folder (tinted with the project's colour).
            assert!(trek.visible(cx, "path-src/main.rs"), "{choice:?}");
            assert!(trek.visible(cx, "path-src/"), "{choice:?}");
        }
        // The thought folds into a group; opened (a lone thought opens with it), it's markdown
        // too: its own document with the agent's text as written, not plain text.
        assert!(trek.drawn_markdown(cx).iter().all(|(ix, _)| *ix != thought));
        trek.click(cx, ("tool-group", thought));
        trek.render(cx);
        assert!(trek.visible(cx, ("reasoning", thought)));
        let docs = trek.drawn_markdown(cx);
        assert!(docs.iter().any(|(ix, text)| *ix == thought && text == THOUGHT), "{docs:?}");
        assert!(docs.iter().any(|(ix, text)| *ix == answer && text == ANSWER));
        // In Paper too.
        cx.update(|cx| crate::apply_theme(ThemeChoice::Paper, None, cx));
        trek.render(cx);
        assert!(trek.visible(cx, ("reasoning", thought)));
    });
}

#[test]
fn a_project_has_one_colour_wherever_its_folders_show() {
    run(async |cx| {
        let trek = open(cx);
        let id = answered(&trek, cx);
        let project = trek.project.clone();
        let name = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.name.clone())).expect("project");
        let tints = |trek: &Trek, cx: &mut TestAppContext| {
            trek.read(cx, |ws, cx| {
                let t = ws.thread(&id).expect("thread");
                (ws.thread_project_tint(t, cx), ws.current_project_tint(cx), ws.project_tint_at(&project, cx))
            })
        };
        // From the name until one's chosen: the badge's ink, the same for the thread, the
        // screen it's on and the folder.
        let dark = |cx: &mut TestAppContext| cx.read(|cx| gpui_kit::component::ActiveTheme::theme(cx).mode.is_dark());
        let auto = crate::ui::project_ink(crate::ui::project_hue(&name, None), dark(cx));
        assert_eq!(tints(&trek, cx), (Some(auto), Some(auto), Some(auto)));

        // Chosen in the project's settings: a swatch per colour, and every place follows.
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone())).expect("project");
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "project-color-auto"));
        trek.click(cx, "project-color-212");
        assert_eq!(trek.read(cx, |ws, _| ws.project_look(&project).color), Some(212));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        let blue = crate::ui::project_ink(212. / 360., dark(cx));
        assert_eq!(tints(&trek, cx), (Some(blue), Some(blue), Some(blue)));
        // Paper: the same hue, in Paper's ink.
        cx.update(|cx| crate::apply_theme(ThemeChoice::Paper, None, cx));
        trek.render(cx);
        let paper = crate::ui::project_ink(212. / 360., false);
        assert_eq!(tints(&trek, cx).0, Some(paper));
        assert!(trek.visible(cx, "path-src/"));
        // The Explorer draws the project's folders in it.
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Explorer, window, cx)));
        trek.render(cx);
        let explorer = |trek: &Trek, cx: &mut TestAppContext| {
            trek.render(cx);
            cx.read(|cx| panel.read(cx).explorer_tint(cx))
        };
        assert_eq!(explorer(&trek, cx), Some(Some(paper)));
        // Automatic again: back to the name's, the Explorer too.
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.color = None, cx));
        let named = crate::ui::project_ink(crate::ui::project_hue(&name, None), false);
        assert_eq!(tints(&trek, cx).0, Some(named));
        assert_eq!(explorer(&trek, cx), Some(Some(named)));
        // A draft in the project (its folder chip in the composer) shows it too.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        assert_eq!(tints(&trek, cx).1, Some(named));
        assert!(trek.read(cx, |ws, _| !ws.settings.projects.contains_key(&project.display().to_string())), "nothing left to keep");
    });
}

#[test]
fn a_side_chat_at_work_says_a_trail_word() {
    run(async |cx| {
        let trek = open(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::SideChat, window, cx)));
        trek.click(cx, "side-input");
        trek.type_live(cx, "mock:long 3s");
        trek.press_live(cx, "enter");
        let side = |ws: &crate::workspace::Workspace| ws.threads.iter().find(|t| t.side_of.is_some()).map(|t| t.id.clone());
        trek.wait(cx, "the side chat to start", |ws| side(ws).is_some_and(|id| ws.live.get(&id).is_some_and(|l| l.turn_started.is_some()))).await;
        trek.render(cx);
        assert!(trek.visible(cx, "side-working"), "a trail word while it works");
        let id = trek.read(cx, |ws, _| side(ws)).unwrap();
        trek.wait(cx, "the side chat to finish", |ws| !ws.turn_running(&id)).await;
        trek.render(cx);
        assert!(!trek.visible(cx, "side-working"));
        // Its answer reads as the transcript's do (Trek's markdown, path chips).
        let answer = trek.read(cx, |ws, _| ws.live[&id].items.iter().position(|i| matches!(i, Item::Assistant { text } if !text.is_empty()))).expect("an answer");
        assert!(trek.visible(cx, ("side-answer", answer)));
    });
}

/// A thread whose answer is a long paragraph, many turns deep (so the transcript scrolls).
fn long_answers(trek: &Trek, cx: &mut TestAppContext) -> (String, usize) {
    let id = trek.quiet_thread(cx);
    let para = "Startup lives in main.rs. It does three things, in order: it parses the flags, it loads the settings, and then it opens the window. The settings file is read once and watched for changes, so editing it while Trek runs applies the new values without a restart. ".repeat(3);
    for n in 0..6 {
        let text = format!("Answer {n}. {para}");
        let events = vec![AgentEvent::TextDelta(text.clone()), AgentEvent::TextDone(text), AgentEvent::TurnComplete { error: None }];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
    }
    trek.render(cx);
    let last = trek.read(cx, |ws, _| ws.live[&id].items.iter().rposition(|i| matches!(i, Item::Assistant { .. }))).unwrap();
    (id, last)
}

#[test]
fn prose_uses_the_whole_column_and_the_column_grows_with_the_window() {
    run(async |cx| {
        let trek = open(cx);
        // A big window: the column at its widest, the prose across all of it, the composer and
        // the transcript on one edge.
        cx.simulate_window_resize(trek.window, gpui_kit::size(px(1900.), px(1000.)));
        let (_, last) = long_answers(&trek, cx);
        let column = trek.read(cx, |ws, _| ws.column());
        assert_eq!(column, px(870.));
        let text = trek.bounds(cx, ("answer-text", last)).expect("the answer");
        assert!((text.size.width - column).abs() < px(1.), "{text:?}");
        let composer = trek.bounds(cx, "send").expect("composer");
        assert!(composer.right() <= text.right() + px(16.) && composer.right() >= text.right() - px(16.), "{composer:?} vs {text:?}");
        // A larger text size widens it in step.
        trek.update(cx, |ws, cx| {
            ws.settings.appearance.transcript_font_size = 17.5;
            ws.save_settings(cx);
        });
        trek.render(cx);
        let wider = trek.bounds(cx, ("answer-text", last)).expect("the answer");
        assert!((wider.size.width - px(17.5 * crate::md::COLUMN)).abs() < px(1.), "{wider:?}");
        // A small window: the column takes what's left, with room at the sides.
        cx.simulate_window_resize(trek.window, gpui_kit::size(px(900.), px(800.)));
        trek.render(cx);
        let narrow = trek.bounds(cx, ("answer-text", last)).expect("the answer");
        assert!(narrow.size.width < px(870.));
        let view = trek.bounds(cx, ("answer", last)).expect("the row");
        // (The row itself sits in the scroller's own inset.)
        let (left, right) = (narrow.left() - view.left(), view.right() - narrow.right());
        assert!(left >= px(20.) && (left - right).abs() < px(1.), "{narrow:?} in {view:?}");
    });
}

#[test]
fn the_last_line_clears_the_composer_and_jump_to_latest_sits_below_the_text() {
    run(async |cx| {
        let trek = open(cx);
        let (_, last) = long_answers(&trek, cx);
        // At the live edge: the last answer ends clear above the composer, and no jump button.
        let text = trek.bounds(cx, ("answer-text", last)).expect("the answer");
        let composer = trek.bounds(cx, "send").expect("composer");
        assert!(text.bottom() < composer.top() - px(40.), "{text:?} vs {composer:?}");
        assert!(!trek.visible(cx, "jump-to-latest"));
        // Scrolled up: the button shows in the band at the transcript's foot, under every line
        // still drawn above it.
        let view = trek.thread_view(cx);
        view.update(cx, |v, cx| v.scroll_to_top(cx));
        trek.render(cx);
        let jump = trek.bounds(cx, "jump-to-latest").expect("jump to latest");
        // It sits in the band's solid part, under the fade the text above melts into: no line
        // runs behind it.
        let band = trek.bounds(cx, "transcript-foot").expect("the band");
        assert!(jump.top() >= band.top() + px(crate::thread_view::JUMP_FADE) && jump.bottom() <= band.bottom(), "{jump:?} in {band:?}");
        assert!(jump.bottom() <= composer.top() - px(16.), "in the transcript, above the working bar and composer");
        // Back down.
        trek.click(cx, "jump-to-latest");
        trek.render(cx);
        assert!(!trek.visible(cx, "jump-to-latest"));
    });
}

#[test]
fn a_thread_window_redraws_its_folders_in_a_new_colour() {
    run(async |cx| {
        let trek = open(cx);
        let id = answered(&trek, cx);
        let own = trek.open_thread_window(cx, &id);
        assert!(trek.visible_in(cx, own, "path-src/"));
        // The main window moves on, so only the thread window shows the thread.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        super::take_renders();
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.color = Some(135), cx));
        cx.run_until_parked();
        assert!(super::take_renders().get("ThreadView").is_some_and(|n| *n > 0), "its transcript redrew for the colour");
    });
}
