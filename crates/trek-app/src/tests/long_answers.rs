//! Long answers drawn in parts (`thread_view::answer_parts`): what the reader sees stays one
//! answer, while it streams in, scrolls, ends, errors or is taken back.

use super::flows::long_answer;
use super::harness::{Trek, mock, open, run, store_items};
use crate::workspace::Route;
use gpui_kit::{ElementId, TestAppContext};
use trek_agents::AgentEvent;
use trek_core::store::Item;

/// The id of part `part` of the answer at transcript index `ix`.
fn part(ix: usize, part: usize) -> ElementId {
    ElementId::NamedInteger("answer-part".into(), ((ix as u64) << 20) | part as u64)
}

fn answer_ix(trek: &Trek, cx: &TestAppContext, id: &str) -> usize {
    trek.item_ix(cx, id, |i| matches!(i, Item::Assistant { .. }))
}

/// What the documents drawn for the answer at `ix` hold, in order. Only the rows that were on
/// screen have one.
fn drawn(trek: &Trek, cx: &mut TestAppContext, ix: usize) -> Vec<String> {
    trek.drawn_markdown(cx)
        .into_iter()
        .filter(|(i, _)| *i == ix)
        .map(|(_, t)| t)
        .collect()
}

/// `text` as its parts.
fn parts_of(text: &str) -> Vec<&str> {
    let cuts = crate::thread_view::answer_parts(text);
    (0..=cuts.len())
        .filter_map(|n| crate::thread_view::part_of(text, &cuts, n))
        .collect()
}

/// Each document drawn for the answer at `ix` holds one part of `text`, in order, and the last
/// part (the one growing, at the end of the transcript) is among them.
fn assert_drawn_are_parts(trek: &Trek, cx: &mut TestAppContext, ix: usize, text: &str) {
    let held = drawn(trek, cx, ix);
    let parts = parts_of(text);
    let mut from = 0;
    for doc in &held {
        let n = parts[from..].iter().position(|p| p == doc).unwrap_or_else(|| panic!("a document holds {} bytes that aren't a part (in order) of the {}-byte answer: {:?}", doc.len(), text.len(), &doc[..doc.len().min(60)]));
        from += n + 1;
    }
    assert_eq!(
        held.last().cloned(),
        parts.last().map(|p| p.to_string()),
        "the last part is drawn, whole ({} parts)",
        parts.len()
    );
}

/// How many rows the answer is drawn in: 1 as one row, and each of its parts otherwise.
fn rows_of_answer(trek: &Trek, cx: &TestAppContext) -> usize {
    trek.rows(cx)
        .iter()
        .filter(|r| r.starts_with("assistant"))
        .count()
}

/// `text` streamed in a few characters at a time, each batch drawn before the next arrives.
async fn stream(trek: &Trek, cx: &mut TestAppContext, id: &str, text: &str, batch: usize) {
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(batch) {
        let delta: String = chunk.iter().collect();
        trek.update(cx, |ws, cx| {
            ws.apply_events(id, vec![AgentEvent::TextDelta(delta)], cx)
        });
        trek.render(cx);
    }
}

#[test]
fn an_answer_a_byte_either_side_of_six_kilobytes_reads_the_same() {
    run(async |cx| {
        let trek = open(cx);
        let full = long_answer();
        for len in [5_999, 6_001] {
            let id = trek.quiet_thread(cx);
            let text = &full[..len];
            let events = vec![
                AgentEvent::TextDelta(text.into()),
                AgentEvent::TextDone(text.into()),
                AgentEvent::TurnComplete { error: None },
            ];
            trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
            trek.render(cx);
            let ix = answer_ix(&trek, cx, &id);
            // The same words either way; the longer one is in parts.
            assert_eq!(trek.answers(cx, &id), text);
            assert_drawn_are_parts(&trek, cx, ix, text);
            if len < 6_000 {
                assert_eq!(trek.rows(cx), ["assistant", "end"]);
                assert_eq!(drawn(&trek, cx, ix), [text]);
                assert!(trek.visible(cx, ("answer-text", ix)));
            } else {
                assert!(rows_of_answer(&trek, cx) > 1, "{:?}", trek.rows(cx));
                // At the top, it begins with the answer's first words, in the document of the first part.
                let scroller = trek.thread_view(cx).read_with(cx, |v, _| v.scroller());
                scroller.update(cx, |s, cx| _ = s.scroll_to_item(0, cx));
                trek.render(cx);
                assert!(
                    trek.visible(cx, ("answer-text", ix)),
                    "the first part is the answer's top"
                );
                assert_eq!(drawn(&trek, cx, ix)[0], parts_of(text)[0]);
                assert!(text.starts_with(&drawn(&trek, cx, ix)[0]));
            }
        }
    });
}

#[test]
fn a_definition_at_the_end_gathers_an_answer_that_was_in_parts_into_one_document() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let body = long_answer();
        stream(&trek, cx, &id, &body, 250).await;
        let parts = rows_of_answer(&trek, cx);
        assert!(parts >= 5, "{:?}", trek.rows(cx));
        // `[the docs][1]` resolves only in the document that holds the definition.
        let tail = "See [the docs][1] for more.\n\n[1]: https://example.com/docs\n";
        stream(&trek, cx, &id, tail, 6).await;
        let full = format!("{body}{tail}");
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::TextDone(full.clone()),
                    AgentEvent::TurnComplete { error: None },
                ],
                cx,
            )
        });
        trek.render(cx);
        assert_eq!(trek.rows(cx), ["assistant", "end"]);
        let ix = answer_ix(&trek, cx, &id);
        assert_eq!(
            drawn(&trek, cx, ix),
            [full],
            "one document holds the answer and its definition"
        );
    });
}

#[test]
fn parts_follow_an_answer_through_fences_multibyte_text_and_an_error() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        // A fence opens (with blank lines in it) once cuts were made, and text with characters of
        // two, three and four bytes comes in batches that end between them.
        let text = format!(
            "{}\n\n```rust\nfn main() {{\n\n    println!(\"héllo — 🌍\");\n\n}}\n```\n\n{}",
            long_answer(),
            long_answer().replace("Section", "Sección — 🌍")
        );
        let chars: Vec<char> = text.chars().collect();
        let mut so_far = String::new();
        let mut rows = 0;
        for chunk in chars.chunks(331) {
            let delta: String = chunk.iter().collect();
            so_far.push_str(&delta);
            trek.update(cx, |ws, cx| {
                ws.apply_events(&id, vec![AgentEvent::TextDelta(delta)], cx)
            });
            trek.render(cx);
            // Cuts made stay made: the answer never has fewer rows than it had.
            let now = rows_of_answer(&trek, cx);
            assert!(
                now >= rows,
                "{rows} rows became {now} at {} bytes",
                so_far.len()
            );
            rows = now;
            let ix = answer_ix(&trek, cx, &id);
            assert_drawn_are_parts(&trek, cx, ix, &so_far);
        }
        assert!(rows >= 10, "{rows}");
        // The turn fails: what was said stays, in the same parts, with the error under it.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![AgentEvent::TurnComplete {
                    error: Some("boom".into()),
                }],
                cx,
            )
        });
        trek.render(cx);
        let ix = answer_ix(&trek, cx, &id);
        assert_drawn_are_parts(&trek, cx, ix, &text);
        assert_eq!(rows_of_answer(&trek, cx), rows);
        assert!(
            trek.items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::Error { .. })),
            "{:?}",
            trek.rows(cx)
        );
        // It was saved as the turn ended, whole.
        let stored = trek.read(cx, |ws, _| ws.store.items(&id)).expect("items");
        assert!(
            stored
                .iter()
                .any(|i| matches!(i, Item::Assistant { text: t } if *t == text))
        );
    });
}

#[test]
fn a_reader_scrolled_up_is_not_moved_as_parts_are_added_and_one_at_the_end_follows() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let full = long_answer();
        let (first, rest) = full.split_at(7_000);
        stream(&trek, cx, &id, first, 500).await;
        let ix = answer_ix(&trek, cx, &id);
        let scroller = trek.thread_view(cx).read_with(cx, |v, _| v.scroller());
        assert!(
            scroller.read_with(cx, |s, _| s.is_following_tail()),
            "a reader at the end follows"
        );
        let parts = rows_of_answer(&trek, cx);
        // At the end: the last part is on screen as it grows, and as parts are added.
        let (middle, last) = rest.split_at(rest.len() / 2);
        stream(&trek, cx, &id, middle, 400).await;
        let at_end = rows_of_answer(&trek, cx);
        assert!(at_end > parts);
        assert!(
            trek.visible(cx, part(ix, at_end - 1)),
            "the last part follows the stream"
        );
        // Scrolled back to the top, the top stays where it is while more parts come in below.
        scroller.update(cx, |s, cx| _ = s.scroll_to_item(0, cx));
        trek.render(cx);
        let before = trek
            .bounds(cx, ("answer-text", ix))
            .expect("the answer's top is on screen");
        stream(&trek, cx, &id, last, 400).await;
        assert!(
            rows_of_answer(&trek, cx) > at_end,
            "parts were added while scrolled up"
        );
        let after = trek
            .bounds(cx, ("answer-text", ix))
            .expect("the answer's top is still on screen");
        assert_eq!(after.origin, before.origin, "the view didn't jump");
        assert!(
            !scroller.read_with(cx, |s, _| s.is_following_tail()),
            "a reader scrolled up isn't dragged down"
        );
        // Back at the end, it follows again.
        scroller.update(cx, |s, cx| s.scroll_to_end(cx));
        let tail = "\n\n## One more\n\n".to_string() + &long_answer();
        stream(&trek, cx, &id, &tail, 600).await;
        assert!(
            trek.visible(cx, part(ix, rows_of_answer(&trek, cx) - 1)),
            "the last part follows once more"
        );
    });
}

#[test]
fn a_thread_window_on_the_same_thread_draws_the_parts_too() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let window = trek.open_thread_window(cx, &id);
        stream(&trek, cx, &id, &long_answer(), 300).await;
        let ix = answer_ix(&trek, cx, &id);
        let parts = rows_of_answer(&trek, cx);
        assert!(parts >= 5);
        // The main window and the thread window each keep their own documents.
        assert!(trek.visible(cx, part(ix, parts - 1)));
        assert!(trek.visible_in(cx, window, part(ix, parts - 1)));
    });
}

#[test]
fn an_answer_in_parts_is_taken_back_with_the_message_before_it() {
    run(async |cx| {
        let trek = open(cx);
        // Two turns, the first answer long (stored as if it had been saved there).
        let user = |t: &str| Item::User {
            text: t.into(),
            images: vec![],
            at: Some(1_759_400_000_000),
            resume: None,
            aside: false,
        };
        let items = vec![
            user("one"),
            Item::Assistant {
                text: long_answer(),
            },
            Item::TurnEnd {
                at: 1_759_400_030_000,
                took_secs: 3,
            },
            user("two"),
            Item::Assistant {
                text: "Short.".into(),
            },
            Item::TurnEnd {
                at: 1_759_400_060_000,
                took_secs: 3,
            },
        ];
        let id = trek.update(cx, |ws, cx| {
            let t = ws
                .store
                .create_thread(
                    Some(&trek.project),
                    mock(),
                    None,
                    trek_core::Effort::Medium,
                    trek_core::HandHolding::Auto,
                )
                .expect("thread");
            store_items(&ws.store, &t.id, items);
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        trek.wait(cx, "the history", |ws| {
            ws.live.get(&id).is_some_and(|l| l.loaded && !l.loading)
        })
        .await;
        trek.render(cx);
        let long_rows = rows_of_answer(&trek, cx) - 1;
        assert!(long_rows >= 5, "{:?}", trek.rows(cx));
        let ix = answer_ix(&trek, cx, &id);
        assert!(trek.visible(cx, ("answer-text", ix)) || trek.visible(cx, part(ix, long_rows - 1)));
        // Back to "two": the long answer stays, in its parts.
        let two = trek.read(cx, |ws, _| ws.live[&id].items.ids()[3].clone());
        assert!(
            trek.update(cx, |ws, cx| ws.rewind(&id, &two, false, cx))
                .is_some()
        );
        trek.render(cx);
        assert_eq!(rows_of_answer(&trek, cx), long_rows);
        assert!(
            !trek
                .items(cx, &id)
                .iter()
                .any(|i| matches!(i, Item::User { text, .. } if text == "two")),
            "{:?}",
            trek.rows(cx)
        );
        // Back to "one": everything after it goes, whatever number of rows it was drawn in.
        let one = trek.read(cx, |ws, _| ws.live[&id].items.ids()[0].clone());
        assert!(
            trek.update(cx, |ws, cx| ws.rewind(&id, &one, false, cx))
                .is_some()
        );
        trek.render(cx);
        assert_eq!(rows_of_answer(&trek, cx), 0, "{:?}", trek.rows(cx));
        assert!(
            trek.drawn_markdown(cx).is_empty(),
            "no document keeps a part of what's gone"
        );
    });
}

#[test]
fn a_very_long_answer_is_saved_later_but_whole_when_the_turn_ends() {
    run(async |cx| {
        let trek = open(cx);
        let store = trek.read(cx, |ws, _| ws.store.clone());
        let id = trek.quiet_thread(cx);
        let stored = |cx: &TestAppContext| {
            trek.read(cx, |ws, _| ws.store.items(&id))
                .expect("items")
                .iter()
                .map(|i| {
                    if let Item::Assistant { text } = i {
                        text.len()
                    } else {
                        0
                    }
                })
                .sum::<usize>()
        };
        let (small, big) = ("a ".repeat(500), "b".repeat(200 * 1024));
        // A short answer is saved after a second, as ever.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta(small.clone())], cx)
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1_100));
        cx.run_until_parked();
        assert_eq!(stored(cx), small.len());
        // One of 200 KB waits three: not saved after one and a half seconds, saved after three.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta(big.clone())], cx)
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1_500));
        cx.run_until_parked();
        assert_eq!(stored(cx), small.len(), "not yet");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1_800));
        cx.run_until_parked();
        assert_eq!(stored(cx), small.len() + big.len());
        // More, and the turn ends before the wait is over: it's saved at once.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta("tail".into())], cx)
        });
        assert_eq!(stored(cx), small.len() + big.len());
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: None }], cx)
        });
        assert_eq!(stored(cx), small.len() + big.len() + 4);
        // And whatever came since is saved when Trek quits, as it is for any answer.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![AgentEvent::TextDelta("more".into())], cx)
        });
        cx.quit();
        assert_eq!(
            store
                .items(&id)
                .expect("items")
                .iter()
                .map(|i| if let Item::Assistant { text } = i {
                    text.len()
                } else {
                    0
                })
                .sum::<usize>(),
            small.len() + big.len() + 8
        );
    });
}
