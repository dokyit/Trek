//! Live pages in answers. The harness has no native window for a web view, so a page's card
//! draws as it does where none can be made: its header and actions, and a frame that says so.

use super::harness::{Trek, open, run};
use gpui_kit::TestAppContext;
use trek_agents::AgentEvent;
use trek_core::settings::ThemeChoice;
use trek_core::store::Item;

/// A thread whose one answer is `text`, on screen.
fn answered(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    let id = trek.quiet_thread(cx);
    let events = vec![AgentEvent::TextDelta(text.into()), AgentEvent::TextDone(text.into()), AgentEvent::TurnComplete { error: None }];
    trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
    trek.render(cx);
    id
}

/// The ids of every element on screen, as `click` would name them.
fn on_screen(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    cx.run_until_parked();
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible())
            .filter_map(|s| match s.path().last()? {
                gpui_kit::ElementId::Name(n) => Some(n.to_string()),
                _ => None,
            })
            .collect()
    })
}

fn find(ids: &[String], prefix: &str) -> Option<String> {
    ids.iter().find(|id| id.starts_with(prefix)).cloned()
}

/// The labels of elements on screen whose ids start with `prefix`.
fn labels(trek: &Trek, cx: &mut TestAppContext, prefix: &str) -> Vec<String> {
    cx.run_until_parked();
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible() && matches!(s.path().last(), Some(gpui_kit::ElementId::Name(n)) if n.starts_with(prefix)))
            .filter_map(|s| s.label().map(str::to_string))
            .collect()
    })
}

/// The page in `mock:page`, as the card holds it.
fn mock_html() -> String {
    let page = trek_agents::mock::PAGE;
    let body = page.split_once("```trek-html\n").unwrap().1;
    body.split_once("\n```").unwrap().0.to_string()
}

/// The card on screen (the transcript follows the end of the answer, so a long one may have its
/// header above the screen; tests that click it answer with just the page).
fn card_in_view(trek: &Trek, cx: &mut TestAppContext) -> String {
    for _ in 0..3 {
        trek.render(cx);
    }
    let ids = on_screen(trek, cx);
    find(&ids, "page-card-").unwrap_or_else(|| panic!("a page card on screen: {ids:?}"))
}

/// The ids of every element drawn in the last frame, on screen or scrolled off it.
fn drawn(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    cx.run_until_parked();
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|s| match s.path().last()? {
                gpui_kit::ElementId::Name(n) => Some(n.to_string()),
                _ => None,
            })
            .collect()
    })
}

/// An answer that's just the page, so its card's header is on screen.
fn just_the_page() -> String {
    format!("```trek-html\n{}\n```", mock_html())
}

#[test]
fn mock_page_shows_a_card_with_its_actions_and_no_code_block_in_both_themes() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:page");
        trek.wait_done(cx, &id, trek_core::RunState::Idle).await;
        let card = card_in_view(&trek, cx);
        let key = &card["page-card-".len()..];
        for choice in [ThemeChoice::Night, ThemeChoice::Paper] {
            cx.update(|cx| crate::apply_theme(choice, None, cx));
            trek.render(cx);
            // Its header may be above the screen (the transcript follows the answer's end).
            let ids = drawn(&trek, cx);
            for part in ["page-card-", "page-copy-", "page-open-", "page-expand-"] {
                assert_eq!(find(&ids, part), Some(format!("{part}{key}")), "{choice:?}: no {part} in {ids:?}");
            }
            // Not shown as code: no code block's copy button, no note, nothing still building; and
            // with no native window here for a view, the frame says so instead of staying blank.
            assert!(!ids.iter().any(|id| id == "copy"), "{choice:?}: a code block in {ids:?}");
            assert!(find(&ids, "page-hint-").is_some(), "{ids:?}");
            assert!(find(&ids, "page-invalid-").is_none() && find(&ids, "page-building-").is_none(), "{ids:?}");
            assert_eq!(labels(&trek, cx, "page-card-").concat(), "Page: Monitor arm: will the clamp fit?");
        }
        cx.update(|cx| crate::apply_theme(ThemeChoice::Night, None, cx));
        // The prose on both sides of it is in the answer as written.
        assert_eq!(trek.answers(cx, &id), trek_agents::mock::PAGE);
    });
}

#[test]
fn copy_puts_the_page_s_source_on_the_clipboard() {
    run(async |cx| {
        let trek = open(cx);
        answered(&trek, cx, &just_the_page());
        let card = card_in_view(&trek, cx);
        let key = &card["page-card-".len()..];
        trek.click(cx, gpui_kit::SharedString::from(format!("page-copy-{key}")));
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(mock_html().as_str()));
    });
}

#[test]
fn expanding_a_page_makes_it_taller_and_collapsing_puts_it_back() {
    run(async |cx| {
        let trek = open(cx);
        answered(&trek, cx, &just_the_page());
        let card = card_in_view(&trek, cx);
        let key = card["page-card-".len()..].to_string();
        let height = |trek: &Trek, cx: &mut TestAppContext| f32::from(trek.bounds(cx, gpui_kit::SharedString::from(format!("page-card-{key}"))).expect("the card").size.height);
        let collapsed = height(&trek, cx);
        // The page's 420 under its header.
        assert!(collapsed > 420. && collapsed < 520., "{collapsed}");
        trek.click(cx, gpui_kit::SharedString::from(format!("page-expand-{key}")));
        trek.render(cx);
        // No view to ask its height: as tall as a page may be, 80 % of the window.
        let window = trek.window(cx, |window, _| f32::from(window.viewport_size().height));
        let expanded = height(&trek, cx);
        assert!((expanded - collapsed - (window * 0.8 - 420.)).abs() <= 1., "{collapsed} → {expanded} in {window}");
        // Taller than the transcript now, which follows its end: a wheel over the page brings the
        // header back.
        let header = gpui_kit::SharedString::from(format!("page-expand-{key}"));
        for _ in 0..20 {
            if trek.visible(cx, header.clone()) {
                break;
            }
            use gpui_kit::test::TestWindowExt as _;
            let step = gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(120.)));
            let over = gpui_kit::SharedString::from(format!("page-hint-{key}"));
            trek.window(cx, |window, cx| window.scroll(over, step, cx));
            trek.render(cx);
        }
        trek.click(cx, header);
        trek.render(cx);
        assert!((height(&trek, cx) - collapsed).abs() <= 1.);
    });
}

#[test]
fn open_saves_the_page_and_opens_the_browser_tool() {
    run(async |cx| {
        let trek = open(cx);
        answered(&trek, cx, &just_the_page());
        let card = card_in_view(&trek, cx);
        let key = &card["page-card-".len()..];
        trek.click(cx, gpui_kit::SharedString::from(format!("page-open-{key}")));
        trek.render(cx);
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        assert!(cx.read(|cx| panel.read(cx).open && panel.read(cx).browser().is_some()), "the Browser tool opened");
        // The page, as a file in the data folder the Browser can open.
        let pages = trek_core::paths::data_dir().join("pages");
        let saved: Vec<String> = std::fs::read_dir(&pages).expect("the pages folder").flatten().filter_map(|e| std::fs::read_to_string(e.path()).ok()).collect();
        assert!(saved.contains(&mock_html()), "{pages:?}");
    });
}

#[test]
fn a_page_still_being_written_shows_its_placeholder_then_its_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let answer = trek_agents::mock::PAGE;
        let mut sent = 0;
        let mut send = |trek: &Trek, cx: &mut TestAppContext, upto: &str| {
            let end = answer.find(upto).unwrap() + upto.len();
            trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta(answer[sent..end].into())], cx));
            sent = end;
            trek.render(cx);
            on_screen(trek, cx)
        };
        // The fence is open and the title isn't in: the placeholder, no code, no card.
        let ids = send(&trek, cx, "<meta charset=\"utf-8\">");
        assert!(find(&ids, "page-building-").is_some(), "{ids:?}");
        assert!(find(&ids, "page-card-").is_none() && !ids.iter().any(|id| id == "copy"), "no code block while it's written: {ids:?}");
        assert_eq!(labels(&trek, cx, "page-building-").concat(), "Building a page… 1 KB");
        // Further in: the size counts up and the title shows.
        send(&trek, cx, "<canvas id=\"cube\"");
        let ids = drawn(&trek, cx);
        assert!(find(&ids, "page-progress-").is_some(), "{ids:?}");
        let label = labels(&trek, cx, "page-building-").concat();
        assert!(label.starts_with("Building a page… ") && label != "Building a page… 1 KB", "{label}");
        // The rest, and the message is done: the card.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta(answer[sent..].into()), AgentEvent::TextDone(answer.into()), AgentEvent::TurnComplete { error: None }], cx)
        });
        let card = card_in_view(&trek, cx);
        let ids = on_screen(&trek, cx);
        assert!(find(&ids, "page-building-").is_none(), "{ids:?}");
        assert!(card.starts_with("page-card-"));
    });
}

#[test]
fn mock_page_stream_ends_in_the_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:page stream");
        trek.wait_done(cx, &id, trek_core::RunState::Idle).await;
        card_in_view(&trek, cx);
        assert_eq!(trek.answers(cx, &id), trek_agents::mock::PAGE);
    });
}

#[test]
fn a_block_that_isn_t_a_page_stays_code_under_a_note_saying_why() {
    run(async |cx| {
        let trek = open(cx);
        let cases = [
            // Left open when the answer ended.
            ("Here:\n\n```trek-html\n<!doctype html><title>x</title><p>half".to_string(), "the answer ended before the page was finished"),
            // A fragment, not a document.
            ("Here:\n\n```trek-html\n<div>just a part</div>\n```\n".to_string(), "isn't a whole HTML document"),
            // Over the limit.
            (format!("```trek-html\n<!doctype html>{}\n```", "x".repeat(trek_core::page::MAX_BYTES)), "over the 200 KiB limit"),
        ];
        for (text, why) in cases {
            answered(&trek, cx, &text);
            let ids = on_screen(&trek, cx);
            assert!(find(&ids, "page-card-").is_none() && find(&ids, "page-building-").is_none(), "{why}: {ids:?}");
            // The block as code: its copy button (drawn, if above the screen for a long one).
            let all = drawn(&trek, cx);
            assert!(all.iter().any(|id| id == "copy"), "{why}: the block as code: {all:?}");
            let note = labels(&trek, cx, "page-invalid-").concat();
            assert!(note.starts_with("Couldn't show this page: ") && note.contains(why), "{note}");
        }
    });
}

#[test]
fn reasoning_and_tool_output_never_get_a_page() {
    run(async |cx| {
        let trek = open(cx);
        let block = format!("```trek-html\n{}\n```", mock_html());
        let no_page = |trek: &Trek, cx: &mut TestAppContext, what: &str| {
            let ids = on_screen(trek, cx);
            for part in ["page-card-", "page-building-", "page-invalid-"] {
                assert!(find(&ids, part).is_none(), "{what}: no page outside an answer: {ids:?}");
            }
        };
        // Reasoning, opened: its markdown, with the block in it as code.
        let id = trek.quiet_thread(cx);
        let thought = format!("Drafting the page:\n\n{block}");
        let events = vec![AgentEvent::ReasoningDelta(thought.clone()), AgentEvent::TextDelta("Done.".into()), AgentEvent::TextDone("Done.".into()), AgentEvent::TurnComplete { error: None }];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
        trek.render(cx);
        let ix = trek.item_ix(cx, &id, |i| matches!(i, Item::Reasoning { .. }));
        trek.click(cx, ("tool-group", ix));
        trek.render(cx);
        assert!(trek.drawn_markdown(cx).iter().any(|(i, text)| *i == ix && *text == thought), "the reasoning is drawn");
        no_page(&trek, cx, "reasoning");
        // A tool's output.
        let id = trek.quiet_thread(cx);
        let events = vec![
            AgentEvent::ToolStarted { id: "t1".into(), title: "Read".into(), detail: "page.md".into() },
            AgentEvent::ToolFinished { id: "t1".into(), output: block.clone(), ok: true },
            AgentEvent::TextDelta("Done.".into()),
            AgentEvent::TextDone("Done.".into()),
            AgentEvent::TurnComplete { error: None },
        ];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
        trek.render(cx);
        let ix = trek.item_ix(cx, &id, |i| matches!(i, Item::Tool { .. }));
        if trek.visible(cx, ("tool-group", ix)) {
            trek.click(cx, ("tool-group", ix));
            trek.render(cx);
        }
        no_page(&trek, cx, "tool output");
    });
}
