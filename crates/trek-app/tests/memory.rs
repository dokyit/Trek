//! What a session keeps in memory: the transcript's parsed answers by what they weigh, and its
//! pictures as copies no bigger than they're drawn (`docs/PERFORMANCE.md`, "Memory over a
//! session").

mod common;

use common::{Trek, long_answer, mock, open, run, store_items};
use gpui_kit::TestAppContext;
use std::path::PathBuf;
use std::time::Duration;
use trek_app::pictures;
use trek_app::thread_view::MarkdownHeld;
use trek_app::workspace::Route;
use trek_core::store::Item;
use trek_core::{Effort, HandHolding};

/// A thread of these answers, one a turn, opened and drawn (at its end).
fn thread_of(trek: &Trek, cx: &mut TestAppContext, answers: Vec<String>) -> String {
    let id = trek.update(cx, |ws, cx| {
        let t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        let items = answers
            .into_iter()
            .enumerate()
            .flat_map(|(i, text)| {
                [
                    Item::User {
                        text: format!("Question {i}"),
                        images: vec![],
                        at: Some(1_759_400_000_000 + i as i64 * 60_000),
                        resume: None,
                        aside: false,
                    },
                    Item::Assistant { text },
                    Item::TurnEnd {
                        at: 1_759_400_000_000 + i as i64 * 60_000 + 30_000,
                        took_secs: 30,
                    },
                ]
            })
            .collect();
        store_items(&ws.store, &t.id, items);
        ws.reload(cx);
        ws.navigate(Route::Thread(t.id.clone()), cx);
        t.id
    });
    trek.render(cx);
    id
}

/// `n` answers of about 15 KB, each drawn in several parts.
fn long_answers(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("# Answer {i}\n\n{}", long_answer()))
        .collect()
}

/// An answer that's one code block of about `kb` KB: a single document however long (an answer
/// is never cut inside a fence).
fn code_answer(kb: usize) -> String {
    let line =
        "    let value = compute(input, 42).map_err(|e| Error::with_id(request.id(), e))?;\n";
    format!("```rust\n{}```\n", line.repeat(kb * 1024 / line.len()))
}

fn held(trek: &Trek, cx: &TestAppContext) -> MarkdownHeld {
    trek.thread_view(cx).read_with(cx, |v, _| v.markdown_held())
}

/// The transcript indexes that have a document.
fn documented(trek: &Trek, cx: &TestAppContext) -> Vec<usize> {
    let docs = trek
        .thread_view(cx)
        .read_with(cx, |v, cx| v.markdown_documents(cx));
    docs.into_iter().map(|(ix, _)| ix).collect()
}

/// Scroll to row `row` and draw, as a reader arriving there.
fn scroll_to(trek: &Trek, cx: &mut TestAppContext, row: usize) {
    trek.thread_view(cx)
        .update(cx, |v, cx| v.scroll_to_row(row, cx));
    trek.render(cx);
}

/// Up through the whole transcript a row at a time, every row drawn on the way.
fn read_up(trek: &Trek, cx: &mut TestAppContext) {
    for row in (0..trek.rows(cx).len()).rev() {
        scroll_to(trek, cx, row);
    }
}

#[test]
fn a_long_thread_read_through_keeps_its_documents_by_weight() {
    run(async |cx| {
        let trek = open(cx);
        // Forty answers of 15 KB: some 280 documents, about twice what the budget holds.
        thread_of(&trek, cx, long_answers(40));
        let rows = trek.rows(cx).len();
        let parts = trek
            .rows(cx)
            .iter()
            .filter(|r| r.starts_with("assistant"))
            .count();
        assert!(parts > 200, "{parts} parts in {rows} rows");
        let last_answer = 39 * 3 + 1;
        assert!(documented(&trek, cx).contains(&last_answer));

        read_up(&trek, cx);
        let h = held(&trek, cx);
        let (docs, weight) = h.on_screen;
        assert!(
            weight <= h.budget,
            "{weight} bytes of documents, where {} is the most",
            h.budget
        );
        assert!(
            docs < parts * 3 / 4 && docs > 40,
            "{docs} of {parts} documents kept"
        );
        // The ones drawn longest ago went: the end of the thread, not the top it's showing.
        let now = documented(&trek, cx);
        assert!(
            now.contains(&1),
            "the first answer, on screen, has its document"
        );
        assert!(
            !now.contains(&last_answer),
            "the last answer's documents went first"
        );

        // Back at the end, the last answer is parsed again and drawn, as when the thread opened.
        scroll_to(&trek, cx, rows - 1);
        assert!(documented(&trek, cx).contains(&last_answer));
        let text = trek.drawn_markdown(cx);
        let (_, end) = text
            .iter()
            .rev()
            .find(|(ix, _)| *ix == last_answer)
            .unwrap();
        assert!(
            end.trim_end()
                .ends_with("so the log lines up with the trace."),
            "{end:?}"
        );
        assert!(held(&trek, cx).on_screen.1 <= h.budget);
    });
}

#[test]
fn a_document_too_big_to_keep_is_drawn_and_let_go_once_off_screen() {
    run(async |cx| {
        let trek = open(cx);
        // The first answer is a 40 KB file: one document, over the most one may weigh.
        let mut answers = vec![code_answer(40)];
        answers.extend(long_answers(3));
        thread_of(&trek, cx, answers);
        let rows = trek.rows(cx).len();
        assert!(!documented(&trek, cx).contains(&1), "not on screen yet");

        // Scrolled to: it's there, whatever it weighs.
        scroll_to(&trek, cx, 1);
        let h = held(&trek, cx);
        assert!(documented(&trek, cx).contains(&1));
        assert!(h.on_screen.1 > h.cap, "{} bytes with it", h.on_screen.1);
        assert!(trek.visible(cx, ("answer", 1usize)));

        // Off screen again: it isn't kept, though there's room in the budget for it.
        scroll_to(&trek, cx, rows - 1);
        trek.render(cx);
        trek.render(cx);
        let h = held(&trek, cx);
        assert!(
            !documented(&trek, cx).contains(&1),
            "let go of once off screen"
        );
        assert!(h.on_screen.0 > 0 && h.on_screen.1 + 2 * h.cap < h.budget);

        // And it's drawn again when scrolled back to.
        scroll_to(&trek, cx, 1);
        assert!(documented(&trek, cx).contains(&1));
        assert!(trek.visible(cx, ("answer", 1usize)));
    });
}

#[test]
fn a_thread_off_screen_keeps_the_documents_it_was_showing() {
    run(async |cx| {
        let trek = open(cx);
        let first = thread_of(&trek, cx, long_answers(12));
        // Read some way up: dozens of documents.
        let rows = trek.rows(cx).len();
        for row in (rows - 40..rows).rev() {
            scroll_to(&trek, cx, row);
        }
        let reading = held(&trek, cx).on_screen.0;
        assert!(reading > 25, "{reading} documents after reading up 40 rows");
        let showing = documented(&trek, cx);

        // Another thread on screen: the first is parked with what it was showing, no more.
        thread_of(&trek, cx, long_answers(2));
        let h = held(&trek, cx);
        let (_, parked, weight) = h
            .parked
            .iter()
            .find(|(id, ..)| *id == first)
            .unwrap()
            .clone();
        assert!(
            (1..=12).contains(&parked),
            "{parked} documents parked of the {reading} it had"
        );
        assert!(weight < h.parked_budget);

        // Back: where it was, drawn from the documents it kept (none parsed for the frame).
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(first.clone()), cx));
        let back = held(&trek, cx);
        assert!(
            back.on_screen.0 >= parked,
            "its documents came back with it"
        );
        trek.render(cx);
        let now = documented(&trek, cx);
        assert!(
            now.iter().all(|ix| showing.contains(ix)),
            "showing the answers it was: {now:?}"
        );
        assert_eq!(trek.rows(cx).len(), rows);
    });
}

#[test]
fn parked_threads_give_up_their_documents_longest_left_first() {
    run(async |cx| {
        let trek = open(cx);
        // Each thread ends on a 20 KB file: 3 MB of document on screen as it's left.
        let threads: Vec<String> = (0..12)
            .map(|_| thread_of(&trek, cx, vec![code_answer(4), code_answer(20)]))
            .collect();
        let h = held(&trek, cx);
        assert_eq!(h.parked.len(), 11, "every thread left is parked");
        let total: usize = h.parked.iter().map(|(_, _, w)| w).sum();
        assert!(
            total <= h.parked_budget,
            "{total} bytes parked, where {} is the most",
            h.parked_budget
        );
        // The ones left longest ago gave theirs up; the latest still have theirs.
        let (oldest, latest) = (&h.parked[0], h.parked.last().unwrap());
        assert_eq!((oldest.0.as_str(), oldest.1), (threads[0].as_str(), 0));
        assert_eq!(latest.0, threads[10]);
        assert!(latest.1 > 0 && latest.2 > 2_000_000, "{latest:?}");
        let emptied = h
            .parked
            .iter()
            .take_while(|(_, docs, _)| *docs == 0)
            .count();
        assert!(
            h.parked[emptied..].iter().all(|(_, docs, _)| *docs > 0),
            "only the oldest: {:?}",
            h.parked
        );

        // The oldest opens where it was all the same, its answer parsed again.
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Thread(threads[0].clone()), cx)
        });
        trek.render(cx);
        assert!(documented(&trek, cx).contains(&4));
        assert!(
            trek.visible(cx, ("copy-turn", 5usize)),
            "at its end, as it was left"
        );
    });
}

/// A `w`×`h` PNG named `name` in the project.
fn png(trek: &Trek, name: &str, w: u32, h: u32) -> PathBuf {
    let path = trek.project.join(name);
    image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, 140])
    })
    .save(&path)
    .expect("png");
    path
}

/// A thread whose turn `i` attaches `attached[i]` to its message and shows `shown[i]` in its
/// answer, opened and drawn, its pictures read.
fn picture_thread(
    trek: &Trek,
    cx: &mut TestAppContext,
    attached: &[Vec<PathBuf>],
    shown: &[PathBuf],
) -> String {
    let id = trek.update(cx, |ws, cx| {
        let t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        let items = shown
            .iter()
            .enumerate()
            .flat_map(|(i, picture)| {
                [
                    Item::User {
                        text: format!("Screenshot {i}: what's off?"),
                        images: (attached.get(i).into_iter().flatten())
                            .map(|p| p.display().to_string())
                            .collect(),
                        at: Some(1_759_400_000_000 + i as i64 * 60_000),
                        resume: None,
                        aside: false,
                    },
                    Item::Assistant {
                        text: format!(
                            "After the fix it looks like this:\n\n![after]({})\n\nThe rows line up.",
                            picture.display()
                        ),
                    },
                    Item::TurnEnd {
                        at: 1_759_400_000_000 + i as i64 * 60_000 + 30_000,
                        took_secs: 30,
                    },
                ]
            })
            .collect();
        store_items(&ws.store, &t.id, items);
        ws.reload(cx);
        ws.navigate(Route::Thread(t.id.clone()), cx);
        t.id
    });
    draw_pictures(trek, cx);
    id
}

/// Draw, let the pictures asked for be read (off the main thread), and draw them.
fn draw_pictures(trek: &Trek, cx: &mut TestAppContext) {
    trek.render(cx);
    cx.run_until_parked();
    trek.render(cx);
}

/// Up to the thread's first message (it opens at its end), its pictures read and drawn.
fn to_top(trek: &Trek, cx: &mut TestAppContext) {
    trek.thread_view(cx).update(cx, |v, cx| v.scroll_to_top(cx));
    draw_pictures(trek, cx);
}

fn kept(cx: &TestAppContext) -> pictures::Kept {
    cx.read(pictures::kept)
}

#[test]
fn a_threads_pictures_are_small_copies_and_the_viewers_whole_one_goes_as_it_closes() {
    run(async |cx| {
        let trek = open(cx);
        // Two screenshots, 5.8 MB of pixels each: both attached, the first also in the answer.
        let shots = [
            png(&trek, "one.png", 1440, 1000),
            png(&trek, "two.png", 1440, 1000),
        ];
        picture_thread(&trek, cx, &[shots.to_vec()], &shots[..1]);
        to_top(&trek, cx);
        assert!(trek.visible(cx, ("user-img", 0usize)) && trek.visible(cx, ("user-img", 1usize)));
        let k = kept(cx);
        // Two tiles (346×240) and the answer's picture (922×640): 3 MB, where the two whole
        // were 11.5 and their textures as much again.
        assert_eq!((k.small.0, k.whole.0, k.loads), (3, 0, 3));
        assert_eq!(k.small.1, 2 * 346 * 240 * 4 + 922 * 640 * 4);
        assert!(k.small.1 < 2 * 1440 * 1000 * 4 / 3);

        // Opened in the viewer: that one whole, over its small copy while it's read.
        trek.click(cx, ("user-img", 1usize));
        assert!(trek.visible(cx, "attachment-preview"));
        draw_pictures(&trek, cx);
        let k = kept(cx);
        assert_eq!(k.whole, (1, 1440 * 1000 * 4));
        // (The filmstrip's thumbnails are the tiles' copies: nothing more was read for them.)
        assert_eq!((k.small.0, k.loads), (3, 4));

        // Put away: the whole one goes, the transcript's copies stay.
        trek.press(cx, "escape");
        cx.executor().advance_clock(Duration::from_secs(1));
        draw_pictures(&trek, cx);
        assert!(!trek.visible(cx, "attachment-preview"));
        let k = kept(cx);
        assert_eq!((k.small.0, k.whole), (3, (0, 0)));
    });
}

#[test]
fn pictures_scrolled_far_past_are_let_go_and_read_again_when_scrolled_back_to() {
    run(async |cx| {
        let trek = open(cx);
        // Thirty-two answers with a picture each, 2.3 MB a copy: more than the budget holds.
        let shots: Vec<PathBuf> = (0..32)
            .map(|i| png(&trek, &format!("shot-{i}.png"), 1000, 700))
            .collect();
        let each = 914 * 640 * 4;
        assert!(shots.len() * each > pictures::BUDGETS.0);
        picture_thread(&trek, cx, &[], &shots);
        let rows = trek.rows(cx).len();
        let on_open = kept(cx).loads;
        assert!(
            (1..6).contains(&on_open),
            "{on_open} read for the screen it opens on"
        );

        // Up through all of it, a moment at each row (longer than a picture counts as on
        // screen for, or nothing could go).
        for row in (0..rows).rev() {
            trek.thread_view(cx)
                .update(cx, |v, cx| v.scroll_to_row(row, cx));
            draw_pictures(&trek, cx);
            cx.executor().advance_clock(Duration::from_secs(2));
        }
        let k = kept(cx);
        assert_eq!(k.loads, 32, "each read once on the way");
        assert!(k.small.1 <= pictures::BUDGETS.0, "{} bytes kept", k.small.1);
        assert!(
            k.small.0 < 32 && k.small.0 >= 20,
            "{} of 32 kept: the budget's worth",
            k.small.0
        );

        // Scrolling about near the top reads nothing again: those are kept.
        for row in [3, 9, 0, 6] {
            trek.thread_view(cx)
                .update(cx, |v, cx| v.scroll_to_row(row, cx));
            draw_pictures(&trek, cx);
        }
        assert_eq!(kept(cx).loads, 32);

        // Back at the end, the pictures that went are read again and drawn.
        cx.executor().advance_clock(Duration::from_secs(2));
        trek.thread_view(cx)
            .update(cx, |v, cx| v.scroll_to_row(rows - 1, cx));
        draw_pictures(&trek, cx);
        let k = kept(cx);
        assert!(k.loads > 32, "the last answer's picture was read again");
        assert!(k.small.1 <= pictures::BUDGETS.0 + 2 * each);
        let last = shots.last().unwrap();
        assert!(trek.visible(
            cx,
            gpui_kit::SharedString::from(format!("md-image-{}", last.display()))
        ));
    });
}

#[test]
fn a_thread_no_longer_shown_gives_its_pictures_back_after_a_while() {
    run(async |cx| {
        let trek = open(cx);
        let shots = [
            png(&trek, "one.png", 1440, 1000),
            png(&trek, "two.png", 1440, 1000),
        ];
        let id = picture_thread(&trek, cx, &[shots.to_vec()], &shots[..1]);
        to_top(&trek, cx);
        assert_eq!(kept(cx).small.0, 3);
        // What housekeeping does once a minute.
        let minute = |cx: &mut TestAppContext| {
            cx.executor().advance_clock(Duration::from_secs(60));
            cx.update(pictures::forget_idle);
            cx.run_until_parked();
        };

        // On screen and still for a quarter of an hour, nothing in it moving: its pictures stay,
        // and none is read again.
        for _ in 0..15 {
            minute(cx);
        }
        assert_eq!((kept(cx).small.0, kept(cx).loads), (3, 3));
        assert!(trek.visible(cx, ("user-img", 0usize)));

        // Another thread on screen, for a couple of minutes: still there to come back to.
        thread_of(&trek, cx, long_answers(1));
        for _ in 0..2 {
            minute(cx);
        }
        assert_eq!(kept(cx).small.0, 3, "a short while away costs nothing");
        // Left for longer: they're let go of.
        for _ in 0..6 {
            minute(cx);
        }
        assert_eq!(
            kept(cx).small,
            (0, 0),
            "not kept for a thread nobody is looking at"
        );

        // Shown again: read again, and drawn.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        to_top(&trek, cx);
        assert_eq!((kept(cx).small.0, kept(cx).loads), (3, 6));
        assert!(trek.visible(cx, ("user-img", 1usize)));
    });
}
