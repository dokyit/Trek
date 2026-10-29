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
