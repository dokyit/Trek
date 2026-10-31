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

#[test]
fn the_starter_offers_material_and_explains_a_session_until_there_is_one() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        // Files or folders under the composer; nothing in the study folder yet to tick.
        assert!(trek.visible(cx, "study-context") && trek.visible(cx, "study-add-files"));
        assert!(!trek.visible(cx, "study-library"));
        // A first visit says how a session goes; the maps say what they'll hold.
        assert!(trek.visible(cx, "study-how") && trek.visible(cx, "study-maps"));

        // Once there's a map it's listed, can be ticked to build on, and the how-to goes.
        let dir = study::dir();
        study::ensure(&dir).unwrap();
        let map = study::map_dir(&dir).join("softmax.md");
        std::fs::write(&map, "# Softmax\n\n## Claims\n- **c1** rows sum to 1\n").unwrap();
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Study, cx));
        cx.run_until_parked();
        trek.render(cx);
        assert!(trek.visible(cx, "study-map-0") && trek.visible(cx, "study-library"));
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
