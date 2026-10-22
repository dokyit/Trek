//! Videos in a thread play in place: the one a `watch_video` call watched, under its row with
//! the frames the model saw (a frame clicked takes the player there); one an answer links; one
//! attached to a message. Stand-in web views here (`fake_web`).

mod common;

use common::{Trek, mock, open, open_with, run, store_items};
use gpui_kit::{ElementId, TestAppContext};
use trek_app::artifact::{fake_web, placements};
use trek_app::workspace::Route;
use trek_core::store::{Item, ToolStatus};

/// A scratch folder as `watch_video` leaves it: the video, two frames and the manifest. Returns
/// (video, manifest).
fn watched_folder(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = trek_core::paths::data_dir().join(format!("watched-{name}"));
    std::fs::create_dir_all(dir.join("frames")).unwrap();
    let video = dir.join("clip.mp4");
    std::fs::write(&video, b"not really a video").unwrap();
    for f in ["frame-001.jpg", "frame-002.jpg"] {
        std::fs::write(dir.join("frames").join(f), b"\xff\xd8").unwrap();
    }
    let manifest = dir.join(trek_core::media::MANIFEST);
    let body = serde_json::json!({
        "source": video, "video": video, "duration": 3.0, "width": 320, "height": 240, "start": 0.0, "end": 3.0,
        "frames": [{"t": 0.5, "file": "frames/frame-001.jpg"}, {"t": 1.5, "file": "frames/frame-002.jpg"}],
        "transcript": [],
    });
    std::fs::write(&manifest, body.to_string()).unwrap();
    (video, manifest)
}

/// A thread with `items`, on screen and drawn.
async fn thread_of(trek: &Trek, cx: &mut TestAppContext, items: Vec<Item>) -> String {
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
    trek.window(cx, |window, _| window.activate_window());
    for _ in 0..3 {
        trek.render(cx);
    }
    id
}

fn user(text: &str) -> Item {
    Item::User {
        text: text.into(),
        images: vec![],
        at: Some(1),
        resume: None,
        aside: false,
    }
}

fn watch_call(manifest: &std::path::Path, status: ToolStatus) -> Item {
    Item::Tool {
        id: "w1".into(),
        title: "mcp__trek-video__watch_video".into(),
        detail: "clip.mp4".into(),
        output: format!(
            "Watched clip.mp4\nManifest: {}\nVideo: 0:03.0 long",
            manifest.display()
        ),
        status,
    }
}

/// The frames of the strip on screen.
fn frames_shown(trek: &Trek, cx: &mut TestAppContext) -> usize {
    trek.window(cx, |window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|s| s.visible() && matches!(s.path().last(), Some(ElementId::NamedInteger(n, _)) if n == "watched-frame"))
            .count()
    })
}

#[test]
fn a_watched_video_plays_under_its_row_with_the_frames_the_model_saw() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        let (video, manifest) = watched_folder("row");
        thread_of(
            &trek,
            cx,
            vec![
                user("What's in the clip?"),
                watch_call(&manifest, ToolStatus::Done),
                Item::Assistant {
                    text: "A test pattern.".into(),
                },
            ],
        )
        .await;
        let cards = trek.window(cx, |window, cx| placements(window, cx));
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert_eq!(cards[0].title.as_deref(), Some("clip.mp4"));
        assert!(cards[0].has_view, "a view plays it");
        assert_eq!(
            frames_shown(&trek, cx),
            2,
            "the frames the model saw, under the player"
        );

        // A frame clicked takes the player there, once its page has loaded.
        let hash = trek_core::artifact::content_hash(&video.to_string_lossy());
        assert!(trek.window(cx, |window, cx| trek_app::artifact::seek(
            hash, 1.5, window, cx
        )));
        assert!(
            !log.borrow()
                .scripts
                .iter()
                .any(|s| s.contains("currentTime")),
            "not before it has loaded"
        );
        trek.window(cx, trek_app::artifact::load_all);
        assert!(
            log.borrow()
                .scripts
                .iter()
                .any(|s| s.contains("currentTime=1.500")),
            "{:?}",
            log.borrow().scripts
        );
        // The second frame of the row of item 1.
        log.borrow_mut().scripts.clear();
        trek.click(cx, ("watched-frame", 1001usize));
        assert!(
            log.borrow()
                .scripts
                .iter()
                .any(|s| s.contains("currentTime=1.500"))
        );
        assert!(
            !trek.window(cx, |window, cx| trek_app::artifact::seek(
                hash ^ 1,
                1.,
                window,
                cx
            )),
            "no card plays another video"
        );
    });
}

#[test]
fn a_running_or_failed_watch_has_no_player() {
    run(async |cx| {
        let _log = fake_web(true).unwrap();
        let trek = open(cx);
        let (_, manifest) = watched_folder("running");
        thread_of(
            &trek,
            cx,
            vec![user("Watch it"), watch_call(&manifest, ToolStatus::Failed)],
        )
        .await;
        assert!(
            trek.window(cx, |window, cx| placements(window, cx))
                .is_empty()
        );
        assert_eq!(frames_shown(&trek, cx), 0);
    });
}

#[test]
fn a_video_an_answer_links_or_a_message_attaches_plays_in_place() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open(cx);
        let (video, _) = watched_folder("linked");
        let answer = format!(
            "Here it is:\n\n[The test clip]({})\n\nAnd from the web:\n\n[Launch](https://cdn.example.com/launch.webm)",
            video.display()
        );
        let titles = |trek: &Trek, cx: &mut TestAppContext| {
            let mut t: Vec<String> = trek
                .window(cx, |window, cx| placements(window, cx))
                .into_iter()
                .filter_map(|c| c.title)
                .collect();
            t.sort();
            t
        };
        thread_of(
            &trek,
            cx,
            vec![user(&format!("Look at @{} please", video.display()))],
        )
        .await;
        assert_eq!(
            titles(&trek, cx),
            ["clip.mp4"],
            "attached to the message, it plays under it"
        );
        thread_of(
            &trek,
            cx,
            vec![user("Show me"), Item::Assistant { text: answer }],
        )
        .await;
        assert_eq!(
            titles(&trek, cx),
            ["Launch", "The test clip"],
            "a file and a web video the answer links"
        );
        assert_eq!(log.borrow().made, 3);
    });
}

#[test]
fn with_artifacts_off_no_video_plays() {
    run(async |cx| {
        let log = fake_web(true).unwrap();
        let trek = open_with(cx, |s| s.general.interactive_artifacts = false);
        let (video, manifest) = watched_folder("off");
        thread_of(
            &trek,
            cx,
            vec![
                user(&format!("@{}", video.display())),
                watch_call(&manifest, ToolStatus::Done),
                Item::Assistant {
                    text: format!("[clip]({})", video.display()),
                },
            ],
        )
        .await;
        assert!(
            trek.window(cx, |window, cx| placements(window, cx))
                .is_empty()
        );
        assert_eq!(log.borrow().made, 0);
        assert_eq!(
            frames_shown(&trek, cx),
            2,
            "the frames still show; a click opens one"
        );
    });
}
