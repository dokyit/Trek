//! Study: the screen under Notes whose composer starts a session with a tutor, and the session's
//! thread.

mod common;

use common::{data_dir, open, run};
use trek_app::workspace::{Route, Scope};
use trek_core::store::Item;
use trek_core::study;

/// The first message of thread `id`: its text and the images attached.
fn first_message(
    trek: &common::Trek,
    cx: &gpui_kit::TestAppContext,
    id: &str,
) -> (String, Vec<String>) {
    trek.items(cx, id)
        .into_iter()
        .find_map(|i| match i {
            Item::User { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("a first message")
}

#[test]
fn study_opens_from_the_sidebar_and_its_composer_starts_a_session_in_the_study_folder() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-study");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Study);
        assert!(trek.visible(cx, "study") && trek.visible(cx, "study-start"));
        // The starter is the main composer, composing a new thread: its + menu and send are
        // there, the project and plan controls aren't (a session has no project or plan mode).
        assert!(trek.read(cx, |ws, _| ws.is_draft_in(&Scope::Main)));
        assert!(trek.visible(cx, "attach") && trek.visible(cx, "send"));
        assert!(!trek.visible(cx, "plan-pill"));
        assert!(trek.visible(cx, ("study-kind", 0usize)));

        // The composer has the keys; Return starts learning what was typed.
        trek.type_text(cx, "the transformer architecture, math first");
        trek.press(cx, "enter");
        trek.render(cx);
        let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
            panic!("a session opened");
        };
        let t = trek
            .read(cx, |ws, _| ws.thread(&id).cloned())
            .expect("thread");
        assert_eq!(t.title, "Learn · the transformer architecture, math first");
        // It works in the study folder (made ready for it), in no project, with no chat folder.
        let dir = study::dir();
        assert_eq!(t.cwd.as_deref(), Some(dir.as_path()));
        assert!(t.project_id.is_none());
        assert!(dir.join("learner.md").exists() && study::map_dir(&dir).is_dir());
        // Remembered as a learning session, for the screen's list.
        let s = study::sessions(&dir);
        assert!(
            s.iter()
                .any(|s| s.thread == id && s.kind == study::Kind::Learn),
            "{s:?}"
        );
        // Its first message says what it is and names the guide Trek ships; the composer is empty.
        let guide = trek_core::skills::shipped(trek_core::skills::STUDY).unwrap();
        let (text, _) = first_message(&trek, cx, &id);
        assert!(text.starts_with("Study session (learn)."), "{text}");
        assert!(text.contains(&guide.display().to_string()));
        assert!(!text.contains("Sources I've added"));
        assert!(
            std::fs::read_to_string(guide)
                .unwrap()
                .contains("## The map format")
        );
        assert_eq!(trek.composer_text(cx), "");

        // Back on Study, the session is the one to continue, and Continue opens it.
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "study-continue") && !trek.visible(cx, "study-how"));
        assert!(trek.visible(
            cx,
            gpui_kit::SharedString::from(format!("study-session-{id}"))
        ));
        trek.click(cx, "study-continue");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
    });
}

/// The tutor asks in cards: with Interactive artifacts off the page says so, nothing starts, and
/// what was typed is kept.
#[test]
fn with_cards_off_study_says_so_and_does_not_start_a_session() {
    run(async |cx| {
        let trek = common::open_with(cx, |s| s.general.interactive_artifacts = false);
        let toasts = common::toasts(&trek, cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "study-cards-off"));
        trek.type_text(cx, "the transformer architecture");
        trek.press(cx, "enter");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Study);
        assert!(study::sessions(&study::dir()).is_empty());
        let seen = toasts.borrow();
        assert!(
            seen.iter()
                .any(|(m, _)| m.contains("Turn on Interactive artifacts")),
            "{seen:?}"
        );
    });
}

/// Nothing signed in: Study says so, with a way to Settings, and a message doesn't open a thread
/// that would fail or leave a session that never ran.
#[test]
fn with_no_agent_signed_in_study_says_so_and_starts_nothing() {
    run(async |cx| {
        let trek = open(cx);
        let toasts = common::toasts(&trek, cx);
        trek.update(cx, |ws, _| {
            ws.mock_agent = false;
            ws.agents.clear();
        });
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "study-no-agent"));
        trek.type_text(cx, "the transformer architecture");
        trek.press(cx, "enter");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Study);
        assert!(study::sessions(&study::dir()).is_empty());
        assert!(
            toasts
                .borrow()
                .iter()
                .any(|(m, _)| m.contains("Settings › Agents")),
            "{:?}",
            toasts.borrow()
        );
        trek.click(cx, "study-agents-settings");
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Settings(trek_app::workspace::SettingsPage::Agents)
        );
    });
}

#[test]
fn practice_is_a_choice_beside_the_composer() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        trek.render(cx);
        trek.click(cx, ("study-kind", 1usize));
        assert_eq!(trek.read(cx, |ws, _| ws.study.kind), study::Kind::Practice);
        trek.type_text(cx, "integration by parts");
        trek.press(cx, "enter");
        trek.render(cx);
        let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
            panic!("a session opened");
        };
        let title = trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone()));
        assert_eq!(title.as_deref(), Some("Practice · integration by parts"));
        let (text, _) = first_message(&trek, cx, &id);
        assert!(text.starts_with("Study session (practice)."), "{text}");
        assert!(
            text.ends_with("What I want to practise: integration by parts"),
            "{text}"
        );
        // The choice stays for the next session.
        assert_eq!(trek.read(cx, |ws, _| ws.study.kind), study::Kind::Practice);
    });
}

#[test]
fn attached_files_become_the_sessions_sources() {
    run(async |cx| {
        // A file named with `@` (what the + menu, a drop or a paste does for a non-image) and
        // an attached image.
        let notes = data_dir().join("attention-notes.md");
        std::fs::write(&notes, "# my notes").unwrap();
        let photo = data_dir().join("board.png");
        std::fs::write(&photo, b"png").unwrap();
        // A file already in sources/, ticked on the screen.
        let dir = study::dir();
        study::ensure(&dir).unwrap();
        let sheet = study::sources_dir(&dir).join("problem-sheet.md");
        std::fs::write(&sheet, "1. …").unwrap();

        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "study-source-0"));
        trek.click(cx, "study-source-0");
        assert_eq!(
            trek.read(cx, |ws, _| ws.study.picked.clone()),
            vec![sheet.clone()]
        );
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| c.attach_image(photo.clone(), cx));
        trek.type_text(
            cx,
            &format!("attention @{} and how it scales", notes.display()),
        );
        trek.press(cx, "enter");
        trek.render(cx);
        let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
            panic!("a session opened");
        };
        // The title is the learner's words, without the file.
        let title = trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone()));
        assert_eq!(
            title.as_deref(),
            Some("Learn · attention and how it scales")
        );
        // Both are copied into the topic's sources; the ticked one is used where it is; the first
        // message lists all three for the tutor, and the image stays attached.
        let into = study::sources_dir(&dir).join("attention-and-how-it-scales");
        assert_eq!(
            std::fs::read_to_string(into.join("attention-notes.md")).unwrap(),
            "# my notes"
        );
        assert!(into.join("board.png").is_file());
        let (text, images) = first_message(&trek, cx, &id);
        let sources = text
            .split("Sources I've added for this session (read them before you start):")
            .nth(1)
            .expect("sources listed");
        assert!(sources.contains(&into.join("attention-notes.md").display().to_string()));
        assert!(sources.contains(&into.join("board.png").display().to_string()));
        assert!(sources.contains(&sheet.display().to_string()), "{sources}");
        assert_eq!(images, vec![photo.display().to_string()]);
        // The ticks are spent.
        assert!(trek.read(cx, |ws, _| ws.study.picked.is_empty()));
    });
}

#[test]
fn a_map_in_the_study_folder_is_listed_to_carry_on_with() {
    run(async |cx| {
        let dir = study::dir();
        study::ensure(&dir).unwrap();
        std::fs::write(
            study::map_dir(&dir).join("gradient-descent.md"),
            "# Gradient descent\n\n## Claims\n- **c1** a\n- **c2** b\n\n## Links\n- c1 → c2: c\n",
        )
        .unwrap();
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "study-map-0"));
        // Practice on it starts a practice session named after the map.
        trek.click(cx, "study-map-practice-0");
        trek.render(cx);
        let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
            panic!("a session opened");
        };
        let title = trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone()));
        assert_eq!(title.as_deref(), Some("Practice · Gradient descent"));
    });
}

#[test]
fn the_starter_offers_material_and_explains_a_session_until_there_is_one() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        // Files or folders under the composer; nothing in the study folder yet to tick, no maps
        // and nothing to continue: one quiet line says what the page will hold.
        assert!(trek.visible(cx, "study-context") && trek.visible(cx, "study-add-files"));
        assert!(!trek.visible(cx, "study-library") && !trek.visible(cx, "study-maps"));
        assert!(!trek.visible(cx, "study-continue"));
        assert!(trek.visible(cx, "study-how"));

        // Once there's a map it's listed, can be ticked to build on, and the line goes.
        let dir = study::dir();
        study::ensure(&dir).unwrap();
        let map = study::map_dir(&dir).join("softmax.md");
        std::fs::write(&map, "# Softmax\n\n## Claims\n- **c1** rows sum to 1\n").unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "study-map-0") && trek.visible(cx, "study-maps"));
        assert!(!trek.visible(cx, "study-how"));
        trek.click(cx, "study-use-map-0");
        assert_eq!(
            trek.read(cx, |ws, _| ws.study.picked.clone()),
            vec![map.clone()]
        );
    });
}

#[test]
fn a_session_takes_maps_notes_folders_and_pages_and_shows_what_was_asked() {
    run(async |cx| {
        let dir = study::dir();
        study::ensure(&dir).unwrap();
        let map = study::map_dir(&dir).join("softmax.md");
        std::fs::write(&map, "# Softmax\n\n## Claims\n- **c1** rows sum to 1\n").unwrap();
        // A Trek note, named by an id, and a folder of the learner's own.
        let notes = trek_core::notes::notes_dir();
        std::fs::create_dir_all(&notes).unwrap();
        let note = notes.join("0199a1b2c3d4.md");
        std::fs::write(&note, "# Lecture 4: attention\n\nQ, K and V.").unwrap();
        let course = data_dir().join("course");
        std::fs::create_dir_all(&course).unwrap();

        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        // A note chosen from the menu shows as a chip to take back off.
        trek.update(cx, |ws, cx| ws.toggle_study_source(note.clone(), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "study-note-0") && trek.visible(cx, "study-add-note"));
        trek.click(cx, "study-use-map-0");
        trek.type_text(
            cx,
            &format!(
                "attention from https://arxiv.org/abs/1706.03762 @{}",
                course.display()
            ),
        );
        trek.press(cx, "enter");
        trek.render(cx);
        let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
            panic!("a session opened");
        };
        let title = trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.title.clone()));
        assert_eq!(title.as_deref(), Some("Learn · attention from"));
        let (text, _) = first_message(&trek, cx, &id);
        // The note is copied under its title; the map is read where it is; the folder and the
        // page are named for the tutor.
        let copied = study::sources_dir(&dir)
            .join("attention-from")
            .join("lecture-4-attention.md");
        assert!(copied.is_file(), "{text}");
        assert!(
            text.contains(&copied.display().to_string())
                && text.contains(&map.display().to_string())
        );
        assert!(text.contains(&format!(
            "Folders to read where they are:\n- {}",
            course.display()
        )));
        assert!(
            text.ends_with("Pages to read:\n- https://arxiv.org/abs/1706.03762"),
            "{text}"
        );
        let intro = study::intro(&text).expect("read back");
        assert_eq!(intro.topic, "attention from");
        // In the thread it shows as what was asked, not as the tutor's instructions; the
        // composer is for thinking out loud, without a branch or plan mode.
        assert!(trek.visible(cx, "study-intro"));
        assert!(!trek.visible(cx, "plan-pill") && !trek.visible(cx, "branch-chip"));
        // Nor does the side panel treat it as a project: where it is and its context stay, the
        // Git group and project actions don't show, though the study folder is a repository.
        trek.update(cx, |ws, cx| ws.refresh_git_at(dir.clone(), cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "hub-card") && trek.visible(cx, "hub-where"));
        for id in ["hub-branch", "hub-pull", "hub-changes", "hub-run"] {
            assert!(!trek.visible(cx, id), "{id} in a study session");
        }
    });
}

/// A thread on screen whose tutor asked `question` (a `trek-study` block, as the guide says),
/// and the card Trek draws for it.
fn asked(
    trek: &common::Trek,
    cx: &mut gpui_kit::TestAppContext,
    question: &str,
) -> (String, String) {
    let id = trek.quiet_thread(cx);
    let said = format!("First, a quick check:\n\n```trek-study\n{question}\n```");
    trek.update(cx, |ws, cx| {
        ws.apply_events(
            &id,
            vec![
                trek_agents::AgentEvent::TextDelta(said.clone()),
                trek_agents::AgentEvent::TextDone(said),
                trek_agents::AgentEvent::TurnComplete { error: None },
            ],
            cx,
        )
    });
    settle(trek, cx);
    (id, study::card(question).expect("a card"))
}

/// A few frames: the first lays a card out, the next reads what covers it.
fn settle(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext) {
    trek.window(cx, |window, _| window.activate_window());
    for _ in 0..3 {
        trek.render(cx);
    }
}

/// The card's page says `body` (`{nonce}`: its secret), as its bootstrap would.
fn post(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext, body: &str) {
    trek.window(cx, |window, cx| trek_app::artifact::post(window, body, cx));
    settle(trek, cx);
}

const QUESTION: &str = r#"{"kind": "question", "step": "", "prompt": "What sets long-run growth?", "options": ["Research share", "Researcher productivity", "Population growth $\\bar n$, scaled by $\\gamma$", "The saving rate"], "targets": []}"#;

/// The tutor writes only the question; Trek draws the card from it, and the card's answer comes
/// back as the learner's message, shown as a card's answer. Drawn again later (a new view), the
/// card is handed what was sent after it, so it shows the answer rather than a blank card.
#[test]
fn a_tutors_question_is_drawn_as_the_card_and_keeps_the_answer_it_got() {
    run(async |cx| {
        let log = trek_app::artifact::fake_web(true).unwrap();
        let trek = open(cx);
        let (id, html) = asked(&trek, cx, QUESTION);
        // The answer holds the card, not the question: the phone and every reader see the card.
        let answer = trek
            .items(cx, &id)
            .into_iter()
            .find_map(|i| match i {
                Item::Assistant { text } => Some(text),
                _ => None,
            })
            .unwrap();
        assert!(answer.contains(&format!("```trek-html\n{html}\n```")));
        assert!(!answer.contains("```trek-study"));
        assert_eq!(log.borrow().made, 1, "the card runs");
        assert!(log.borrow().bootstraps[0].contains("const replies = Object.freeze([]);"));

        let sent = "Question\nAnswer: C. Population growth $\\bar n$, scaled by $\\gamma$\nConfidence: Unsure (45%)\nWorking: I *think* so";
        let body = serde_json::json!({"n": "{nonce}", "t": "send", "text": sent}).to_string();
        post(&trek, cx, &body);
        let row = trek.rows(cx).iter().position(|r| r == "user").unwrap();
        trek.thread_view(cx)
            .update(cx, |v, cx| v.scroll_to_row(row, cx));
        settle(&trek, cx);
        // The learner's message reads as the card's answer, typeset.
        let shown = trek.window(cx, |window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .filter(|s| s.visible())
                .any(|s| {
                    matches!(s.path().last(), Some(gpui_kit::ElementId::NamedInteger(n, _)) if n == "study-sent")
                })
        });
        assert!(
            shown,
            "{:?} {:?}",
            trek.rows(cx),
            trek.window(cx, |window, _| {
                gpui_kit::base::test_support::registered_paths(window)
            })
            .lines()
            .filter(|l| l.contains("user"))
            .collect::<Vec<_>>()
        );
        trek.update(cx, |ws, _| {
            assert_eq!(ws.card_replies(&html), vec![sent.to_string()]);
        });

        // Drawn again (another page and back): the page is handed the answer it got.
        let before = log.borrow().made;
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        settle(&trek, cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.thread_view(cx).update(cx, |v, cx| v.scroll_to_top(cx));
        settle(&trek, cx);
        let replies = serde_json::to_string(&[sent]).unwrap();
        let handed = format!("const replies = Object.freeze({replies});");
        let log = log.borrow();
        assert!(log.made > before, "new views");
        assert!(
            log.bootstraps[before..].iter().any(|b| b.contains(&handed)),
            "the card made again is handed its answer"
        );
    });
}

/// What tutors were seen to write (Claude Code and Codex, 17 November) comes out as one card a
/// turn: Codex on GPT-5.6 Luna said its message twice in one turn (the same question card twice);
/// a turn that ended inside its block is drawn as far as it got; a tutor that still writes the
/// whole HTML card (a thread begun on an older Trek) gets today's card.
#[test]
fn a_turn_shows_one_card_however_the_tutor_wrote_it() {
    run(async |cx| {
        let _log = trek_app::artifact::fake_web(true).unwrap();
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let cards = |trek: &common::Trek, cx: &gpui_kit::TestAppContext| -> usize {
            trek.items(cx, &id)
                .iter()
                .map(|i| match i {
                    Item::Assistant { text } => text.matches("const DATA = ").count(),
                    _ => 0,
                })
                .sum()
        };
        let turn = |events: Vec<trek_agents::AgentEvent>, cx: &mut gpui_kit::TestAppContext| {
            let mut events = events;
            events.push(trek_agents::AgentEvent::TurnComplete { error: None });
            trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
            settle(&trek, cx);
        };
        use trek_agents::AgentEvent::{TextDelta, TextDone};
        // Codex, its message twice (raw, as it came).
        let said = "Let's locate the edge of what you already know.\n\n```trek-study\n{\"kind\":\"question\",\"step\":\"\",\"prompt\":\"What does $f'(x)$ tell you at a particular input $x$?\",\"options\":[\"The exact value of $f(x)$\",\"The instantaneous rate at which $f$ changes as $x$ changes\",\"The average value of $f$ over all inputs\",\"The input where $f$ is largest\"],\"targets\":[]}\n```";
        turn(
            vec![
                TextDelta(said.into()),
                TextDone(said.into()),
                TextDelta(said.into()),
                TextDone(said.into()),
            ],
            cx,
        );
        assert_eq!(cards(&trek, cx), 1, "{:?}", trek.items(cx, &id));
        // The turn ended inside the block.
        let cut = "Next.\n\n```trek-study\n{\"kind\": \"check\", \"prompt\": \"Why $\\frac{1}{2}$?\", \"options\": [\"a\", \"b\"";
        turn(vec![TextDelta(cut.into()), TextDone(cut.into())], cx);
        assert_eq!(cards(&trek, cx), 2);
        // A whole HTML card, as the 0.4.55 guide asked for.
        let old = format!(
            "Next one.\n\n```trek-html\n{}```",
            include_str!("../../trek-core/src/study_fixtures/old-card-opus-2026-10-29.html")
        );
        turn(vec![TextDelta(old.clone()), TextDone(old)], cx);
        assert_eq!(cards(&trek, cx), 3);
        let last = trek.answers(cx, &id);
        assert!(last.contains("T.replies"), "today's card");
    });
}

/// One question a live tutor asked, read back from a drawn card's page.
#[derive(Debug, Clone)]
struct Asked {
    kind: String,
    step: String,
    prompt: String,
    options: Vec<String>,
    targets: Vec<String>,
}

/// Each card drawn in `text` (an answer), as the question it asks.
fn cards_in(text: &str) -> Vec<Asked> {
    let mut out = vec![];
    for part in text.split("```trek-html\n").skip(1) {
        let Some(at) = part.find("const DATA = ") else {
            continue;
        };
        let line = &part[at + "const DATA = ".len()..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        let json = line.trim_end().trim_end_matches(';').replace("<\\/", "</");
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let l = |k: &str| -> Vec<String> {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        out.push(Asked {
            kind: s("kind"),
            step: s("step"),
            prompt: s("prompt"),
            options: l("options"),
            targets: l("targets"),
        });
    }
    out
}

/// What the card for `q` sends when the learner picks the second option (or works a problem), a
/// little unsure: the same text the card's own script writes.
fn answer_to(q: &Asked, turn: usize) -> String {
    let label = match q.kind.as_str() {
        "predict" => "Predict",
        "check" => "Check",
        "problem" => "Problem",
        _ => "Question",
    };
    let mut head = [label, q.step.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ");
    if !q.targets.is_empty() {
        head.push_str(&format!(" [{}]", q.targets.join(", ")));
    }
    let working = [
        "I think it's this because the outer part changes with the inner one, so you multiply the rates.",
        "Not sure. I tried a small case with $x = 2$ and it seemed to work out.",
        "My guess: differentiate the outside, keep the inside, then times the inside's derivative.",
        "I worked it through: $\\frac{d}{dx}(3x+1)^2 = 2(3x+1)\\cdot 3 = 18x + 6$.",
    ][turn % 4];
    if q.kind == "problem" {
        return format!("{head}\nStatus: I think I've solved it\nWorking: {working}");
    }
    let answer = match q.options.get(1).or(q.options.first()) {
        Some(o) => format!(
            "Answer: {}. {o}",
            if q.options.len() > 1 { 'B' } else { 'A' }
        ),
        None => "Answer: (in my thinking)".to_string(),
    };
    format!("{head}\n{answer}\nConfidence: Fairly sure (70%)\nWorking: {working}")
}

/// Study with real tutors: for each `agent:model` in `TREK_STUDY_MODELS` (default Claude Code on
/// Haiku 4.5), a Learn and a Practice session on a small topic with maths, answered card by card
/// as the card would (`TREK_STUDY_TURNS` questions each, default 4). Prints, for every turn,
/// whether exactly one card was drawn, how long it took, and what the tutor wrote when it wasn't.
/// Not run by default (it costs what the turns cost):
/// `TREK_LIVE_AGENT=study cargo test -p trek-app --test study live_study -- --ignored --nocapture`.
#[test]
#[ignore = "live: runs real agents"]
fn live_study_sessions_ask_one_card_a_turn() {
    use std::time::{Duration, Instant};
    use trek_agents::Decision;
    use trek_core::{AgentId, RunState};
    let models = std::env::var("TREK_STUDY_MODELS")
        .unwrap_or_else(|_| "claude-code:claude-haiku-4-5".into());
    let turns: usize = std::env::var("TREK_STUDY_TURNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(4);
    let kinds = std::env::var("TREK_STUDY_KINDS").unwrap_or_else(|_| "learn,practice".into());
    let mut report: Vec<String> = vec![];
    let mut bad = 0;
    for spec in models.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (agent, model) = spec.split_once(':').expect("agent:model");
        let agent = AgentId::from_key(agent);
        for kind in kinds.split(',').map(str::trim) {
            let kind = if kind == "practice" {
                study::Kind::Practice
            } else {
                study::Kind::Learn
            };
            let topic = match kind {
                study::Kind::Learn => "the chain rule for derivatives, with the maths",
                study::Kind::Practice => {
                    "differentiating with the chain rule, e.g. sin(x^2) and (3x+1)^5"
                }
            };
            let (agent, model) = (agent.clone(), model.to_string());
            run(async |cx| {
                let trek = common::open_with(cx, |s| {
                    s.general.default_agent = agent.key();
                    s.general.default_model = Some(model.clone());
                    s.general.hand_holding = trek_core::HandHolding::AutoAcceptEdits;
                });
                trek.update(cx, |ws, cx| {
                    ws.agents = [
                        (AgentId::ClaudeCode, "Claude Code"),
                        (AgentId::Codex, "Codex"),
                    ]
                    .into_iter()
                    .map(|(a, n)| trek_core::detect::DetectedAgent {
                        agent: a,
                        name: n.into(),
                        path: None,
                        version: None,
                        availability: trek_core::detect::Availability::Ready,
                        models: vec![],
                        install_hint: None,
                    })
                    .collect();
                    ws.draft_prefs.agent = agent.clone();
                    ws.draft_prefs.model = Some(model.clone());
                    ws.navigate(Route::Study, cx);
                    ws.study.kind = kind;
                    ws.start_study(kind, topic.to_string(), None, cx);
                    // An older guide in its place (`TREK_STUDY_GUIDE`), before the tutor reads it:
                    // how a release's tutors asked.
                    if let Ok(old) = std::env::var("TREK_STUDY_GUIDE") {
                        let guide = trek_core::skills::shipped(trek_core::skills::STUDY).unwrap();
                        std::fs::write(guide, std::fs::read_to_string(old).unwrap()).unwrap();
                    }
                });
                let Route::Thread(id) = trek.read(cx, |ws, _| ws.route.clone()) else {
                    panic!("a session opened");
                };
                // Until the turn is over, allowing what it asks to do.
                let done = async |cx: &mut gpui_kit::TestAppContext| -> Result<(), String> {
                    let deadline = Instant::now() + Duration::from_secs(360);
                    loop {
                        cx.run_until_parked();
                        let asking = trek.read(cx, |ws, _| {
                            ws.pending_request(&id).map(|p| p.request_id.clone())
                        });
                        if let Some(rid) = asking {
                            trek.update(cx, |ws, cx| ws.respond(&id, &rid, Decision::Allow, cx));
                        }
                        let state = trek.read(cx, |ws, _| {
                            (ws.thread(&id).map(|t| t.run_state), ws.turn_running(&id))
                        });
                        match state {
                            (Some(RunState::Idle), false) => return Ok(()),
                            (Some(RunState::Failed), _) => return Err("the turn failed".into()),
                            _ => {}
                        }
                        if Instant::now() > deadline {
                            return Err("timed out".into());
                        }
                        cx.background_executor
                            .timer(Duration::from_millis(100))
                            .await;
                    }
                };
                let mut asked = 0;
                for turn in 0..turns {
                    let began = Instant::now();
                    let outcome = done(cx).await;
                    let secs = began.elapsed().as_secs_f32();
                    // What the tutor wrote this turn: the answers after the last message.
                    let items = trek.items(cx, &id);
                    let from = items
                        .iter()
                        .rposition(|i| matches!(i, Item::User { .. }))
                        .unwrap_or(0);
                    let said: Vec<String> = items[from..]
                        .iter()
                        .filter_map(|i| match i {
                            Item::Assistant { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect();
                    let all = said.join("\n\n");
                    let cards = cards_in(&all);
                    let raw = all.matches("```trek-study").count();
                    let last = said.last().cloned().unwrap_or_default();
                    let after = last
                        .rfind("\n```")
                        .map(|at| last[at + 4..].trim().to_string())
                        .unwrap_or_default();
                    let plan = all.contains("```trek-viz");
                    let ok = outcome.is_ok() && cards.len() == 1 && raw == 0;
                    if cards.len() > 1 {
                        for (n, s) in said.iter().enumerate() {
                            println!(
                                "  answer {n}: {} bytes, {} cards, starts {:?}",
                                s.len(),
                                cards_in(s).len(),
                                s.chars().take(60).collect::<String>()
                            );
                        }
                    }
                    let what = match (&outcome, cards.len(), raw) {
                        (Err(e), _, _) => e.clone(),
                        (_, 1, 0) => format!(
                            "card {} ({} options){}: {}",
                            cards[0].kind,
                            cards[0].options.len(),
                            if after.is_empty() {
                                ""
                            } else {
                                ", prose after it"
                            },
                            cards[0].prompt.chars().take(70).collect::<String>()
                        ),
                        (_, 0, 0) if plan => "plan, no card".into(),
                        (_, n, r) => format!("{n} cards drawn, {r} not drawn"),
                    };
                    let line = format!(
                        "{spec} {} turn {} · {secs:.0}s · {what}",
                        kind.label(),
                        turn + 1
                    );
                    println!("{line}");
                    if !ok && !(plan && cards.is_empty() && raw == 0) {
                        bad += 1;
                        println!("--- raw ---\n{}\n--- end ---", {
                            // The drawn cards' pages are long: shown as their question.
                            let mut t = all.clone();
                            while let Some(at) = t.find("```trek-html\n<!doctype") {
                                let end = t[at..]
                                    .find("</html>\n```")
                                    .map_or(t.len(), |e| at + e + 11);
                                t.replace_range(at..end, "[card]");
                            }
                            t
                        });
                    }
                    report.push(line);
                    if outcome.is_err() {
                        break;
                    }
                    asked += cards.len();
                    let reply = match cards.last() {
                        Some(q) => answer_to(q, turn),
                        None if plan => "The plan looks good. Go ahead.".to_string(),
                        // Typed in the composer, as a learner does when there's no card to answer in.
                        None => "Not sure how to write it out. Is it the outer derivative times the inner one?".to_string(),
                    };
                    if turn + 1 < turns {
                        trek.update(cx, |ws, cx| ws.send_to(&id, reply, vec![], cx));
                    }
                }
                println!("{spec} {}: {asked} cards", kind.label());
            });
        }
    }
    println!("\n=== live study ===\n{}", report.join("\n"));
    println!("turns without exactly one card (not a plan): {bad}");
}

/// The thinking box's mic is Trek's own dictation: the page asks for a take (only after the
/// learner's click), what's heard so far goes to the page as it's revised, and the mic again
/// ends the take with its words kept.
#[test]
fn a_question_cards_mic_takes_trek_dictation_into_the_card() {
    use trek_app::dictate::{Heard, fake};
    run(async |cx| {
        let dictation = fake::install();
        let log = trek_app::artifact::fake_web(true).unwrap();
        let trek = open(cx);
        asked(&trek, cx, QUESTION);
        assert!(log.borrow().bootstraps[0].contains("canDictate: true"));
        let heard = |log: &std::rc::Rc<std::cell::RefCell<trek_app::artifact::FakeLog>>| {
            log.borrow()
                .scripts
                .iter()
                .filter(|s| s.contains("'trekdictation'"))
                .cloned()
                .collect::<Vec<_>>()
        };
        post(&trek, cx, r#"{"n":"{nonce}","t":"dictate","on":true}"#);
        cx.run_until_parked();
        assert!(dictation.listening(), "a take is open");
        dictation.hear(Heard::Partial("population".into()));
        cx.run_until_parked();
        dictation.hear(Heard::Partial("population growth".into()));
        cx.run_until_parked();
        let so_far = heard(&log);
        assert!(
            so_far
                .iter()
                .any(|s| s.contains(r#""state":"listening","text":"population growth""#)),
            "{so_far:?}"
        );
        // The mic again: the take settles, and its words go to the page to keep.
        post(&trek, cx, r#"{"n":"{nonce}","t":"dictate","on":false}"#);
        cx.run_until_parked();
        assert!(dictation.finished());
        dictation.hear(Heard::Final("population growth times gamma".into()));
        cx.run_until_parked();
        let last = heard(&log).last().cloned().unwrap();
        assert!(
            last.contains(r#""state":"done","text":"population growth times gamma""#),
            "{last}"
        );
        assert!(!dictation.mic_open(), "the mic is let go");
        // The words were the card's, not the composer's.
        assert_eq!(trek.composer_text(cx), "");
    });
}
