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
        AgentEvent::TurnComplete { cost_usd: None, error: None },
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

#[test]
fn a_line_of_prose_holds_about_seventy_characters() {
    run(async |cx| {
        let trek = open(cx);
        trek.window(cx, |window, cx| {
            let family = gpui_kit::component::ActiveTheme::theme(cx).font_family.clone();
            let text = "Startup lives in main.rs. It does three things, in order: it parses the flags, it loads the settings, and then it opens the window. The settings file is read once and watched for changes, so editing it while Trek runs applies the new values without a restart.";
            // Laid out in the transcript's font, as CoreText sets it: the same count at any size.
            for size in [px(14.5), px(18.)] {
                let run = gpui_kit::TextRun { len: text.len(), font: gpui_kit::font(family.clone()), color: gpui_kit::black(), background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().shape_line(text.into(), size, &[run], None);
                let per_line = crate::md::measure(size) / (line.width / text.chars().count() as f32);
                assert!((65. ..=78.).contains(&per_line), "{per_line} characters a line at {size:?}");
            }
        });
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
