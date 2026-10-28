//! Study: the screen under Notes that starts a session with a tutor, and the session's thread.

mod common;

use common::{open, run};
use trek_app::workspace::Route;
use trek_core::study;

#[test]
fn study_opens_from_the_sidebar_and_starts_a_session_in_the_study_folder() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-study");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Study);
        assert!(trek.visible(cx, "study") && trek.visible(cx, "study-start"));

        // The topic field has the keys; Return starts learning it.
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
        // Its first message names the guide Trek ships.
        let guide = trek_core::skills::shipped(trek_core::skills::STUDY).unwrap();
        assert!(
            std::fs::read_to_string(guide)
                .unwrap()
                .contains("## The map format")
        );
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
