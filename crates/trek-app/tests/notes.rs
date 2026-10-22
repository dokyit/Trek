//! Pictures in notes: dropped (or picked) image files are kept in the notes folder and linked
//! where the cursor is, the preview draws them fit to its width, Write shows them as tiles, a
//! missing one is a placeholder, and deleting the note takes its pictures along to Deleted.

mod common;

use common::{Trek, open, run};
use gpui_kit::{Entity, TestAppContext};
use std::path::PathBuf;
use trek_app::notes::Mode;

/// A real PNG, `w`×`h` pixels.
fn png(path: &std::path::Path, w: u32, h: u32) {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([90, 140, 220, 255]));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

fn notes_view(trek: &Trek, cx: &mut TestAppContext) -> Entity<trek_app::notes::NotesView> {
    cx.read(|cx| trek.root.read(cx).notes.clone())
}

/// What the note holds on disk, once it's been saved.
fn saved(cx: &mut TestAppContext) -> String {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    trek_core::notes::list_in(&trek_core::notes::notes_dir())
        .first()
        .map(|n| n.body.clone())
        .unwrap_or_default()
}

/// Wait for pictures being copied in (off the main thread) to land in the note.
fn landed(cx: &mut TestAppContext, what: &str) -> String {
    for _ in 0..400 {
        let body = saved(cx);
        if body.contains(what) {
            return body;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{what} never landed: {:?}", saved(cx));
}

#[test]
fn a_pasted_screenshot_and_copied_files_go_in_as_pictures() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        let notes = notes_view(&trek, cx);
        // A screenshot on the clipboard (image data): kept as a PNG, linked at the cursor.
        let shot = trek_core::paths::data_dir().join("clip.png");
        png(&shot, 40, 20);
        let item = gpui_kit::ClipboardItem {
            entries: vec![gpui_kit::ClipboardEntry::Image(
                gpui_kit::Image::from_bytes(
                    gpui_kit::ImageFormat::Png,
                    std::fs::read(&shot).unwrap(),
                ),
            )],
        };
        let taken = trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.paste(&item, window, cx))
        });
        assert!(taken, "the text box doesn't paste it as text");
        let body = landed(cx, "![Pasted image](attachments/");
        assert!(body.trim_end().ends_with(".png)"), "{body:?}");
        // An SVG copied in Finder goes in too (the composer's paste leaves those to text).
        let svg = trek_core::paths::data_dir().join("diagram.svg");
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>",
        )
        .unwrap();
        let item = gpui_kit::ClipboardItem {
            entries: vec![gpui_kit::ClipboardEntry::ExternalPaths(
                gpui_kit::ExternalPaths(vec![svg].into()),
            )],
        };
        assert!(trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.paste(&item, window, cx))
        }));
        landed(cx, "![diagram](attachments/");
        // Plain text is the text box's.
        let text = gpui_kit::ClipboardItem::new_string("just words".into());
        assert!(!trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.paste(&text, window, cx))
        }));
    });
}

#[test]
fn pictures_go_into_a_note_and_show_in_every_mode() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        assert!(trek.visible(cx, "note-attach"), "Attach is on the toolbar");
        let notes = notes_view(&trek, cx);
        let outside = trek_core::paths::data_dir().join("from-finder");
        std::fs::create_dir_all(&outside).unwrap();
        let shot = outside.join("Whiteboard.png");
        png(&shot, 1600, 800);
        let readme = outside.join("README.txt");
        std::fs::write(&readme, "not a picture").unwrap();

        // Dropped (as the driver's `note image` does it) with the cursor after the first line.
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.set_text("Trip\n".into(), window, cx))
        });
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| {
                n.add_files(vec![shot.clone(), readme.clone()], window, cx)
            })
        });
        let body = landed(cx, "](attachments/");
        let image = trek_core::notes::images(&body)
            .pop()
            .expect("an image in the note");
        assert_eq!(image.alt, "Whiteboard");
        assert!(
            body.starts_with("Trip\n![Whiteboard](attachments/"),
            "{body:?}"
        );
        let kept = trek_core::notes::notes_dir().join(&image.link);
        assert_eq!(
            std::fs::read(&kept).unwrap(),
            std::fs::read(&shot).unwrap(),
            "a copy kept with the notes"
        );
        // A file that isn't a picture is attached too: a link to its copy, by its name.
        let files = trek_core::notes::files(&body);
        assert_eq!(files.len(), 1, "{body:?}");
        assert_eq!(files[0].name, "README.txt");
        assert!(files[0].link.ends_with(".txt"), "{}", files[0].link);
        assert_eq!(
            std::fs::read(trek_core::notes::notes_dir().join(&files[0].link)).unwrap(),
            b"not a picture"
        );

        // Split (the default): the preview draws it, fit to its width in its own proportions.
        trek.render(cx);
        let name = kept.file_name().unwrap().to_string_lossy().to_string();
        let drawn = trek
            .bounds(
                cx,
                gpui_kit::SharedString::from(format!("note-image-{name}")),
            )
            .expect("the picture in the preview");
        let preview = trek.bounds(cx, "note-preview").expect("the preview");
        assert!(
            drawn.size.width < preview.size.width,
            "{drawn:?} fits in {preview:?}"
        );
        assert!(
            drawn.size.width.as_f32() < 1600.,
            "a big picture is drawn smaller"
        );
        let aspect = drawn.size.width.as_f32() / drawn.size.height.as_f32();
        assert!((aspect - 2.).abs() < 0.05, "its proportions kept: {aspect}");

        // Write: the text shows its link, the picture is a tile under it.
        notes.update(cx, |n, cx| n.set_mode(Mode::Write, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "note-images") && trek.visible(cx, ("note-thumb", 0usize)));
        assert!(
            trek.visible(cx, ("note-file", 1usize)),
            "the attached file is a tile beside the picture"
        );
        assert!(!trek.visible(cx, "note-preview"));

        // A picture that isn't there: a placeholder that says so, nothing worse.
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| {
                n.set_text(format!("{body}![gone](attachments/nope.png)\n"), window, cx)
            })
        });
        notes.update(cx, |n, cx| n.set_mode(Mode::Preview, cx));
        trek.render(cx);
        assert!(trek.visible(
            cx,
            gpui_kit::SharedString::from(format!("note-image-{name}"))
        ));
        assert!(
            trek.visible(cx, "note-image-attachments/nope.png"),
            "a placeholder for the missing one"
        );

        // Deleted: its picture goes along to Deleted, where the note still shows it.
        trek.click(cx, "note-delete");
        cx.run_until_parked();
        assert!(!kept.exists(), "out of the notes' own attachments");
        let bin: PathBuf = trek_core::notes::notes_dir()
            .join("Deleted")
            .join(&image.link);
        assert!(bin.exists(), "kept beside the deleted note");
    });
}

/// A note holding `clip` attached from a made-up `.mp4`, shown as it reads.
async fn note_with_video(trek: &Trek, cx: &mut TestAppContext) -> String {
    trek.click(cx, "open-notes");
    trek.render(cx);
    let notes = notes_view(trek, cx);
    let clip = trek_core::paths::data_dir().join("Trip-clip.mp4");
    std::fs::write(&clip, b"not really a movie").unwrap();
    trek.window(cx, |window, cx| {
        notes.update(cx, |n, cx| n.set_text("Trip\n".into(), window, cx))
    });
    trek.window(cx, |window, cx| {
        notes.update(cx, |n, cx| n.add_files(vec![clip.clone()], window, cx))
    });
    let body = landed(cx, "](attachments/");
    assert!(
        body.contains("[Trip-clip.mp4](attachments/") && body.trim_end().ends_with(".mp4)"),
        "{body:?}"
    );
    notes.update(cx, |n, cx| n.set_mode(Mode::Preview, cx));
    trek.window(cx, |window, _| window.activate_window());
    for _ in 0..3 {
        trek.render(cx);
    }
    body
}

#[test]
fn a_video_plays_in_place_in_a_card_of_the_notes_preview() {
    run(async |cx| {
        let log = trek_app::artifact::fake_web(true).unwrap();
        let trek = open(cx);
        let body = note_with_video(&trek, cx).await;
        let cards = trek.window(cx, |window, cx| trek_app::artifact::placements(window, cx));
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert_eq!(cards[0].title.as_deref(), Some("Trip-clip.mp4"));
        assert!(
            cards[0].has_view && log.borrow().made == 1,
            "a web view plays it"
        );
        // The view is the player's page for the file, which is the only thing it loads.
        assert!(body.contains("attachments/"));
        let link = trek_core::notes::files(&body).pop().unwrap().link;
        assert!(trek_core::notes::notes_dir().join(link).is_file());
        // Another kind of file stays a chip.
        let notes = notes_view(&trek, cx);
        let pdf = trek_core::paths::data_dir().join("Itinerary.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.add_files(vec![pdf], window, cx))
        });
        landed(cx, ".pdf)");
        for _ in 0..3 {
            trek.render(cx);
        }
        assert_eq!(
            trek.window(cx, |window, cx| trek_app::artifact::placements(window, cx))
                .len(),
            1,
            "only the video is a card"
        );
    });
}

#[test]
fn with_artifacts_off_a_video_is_a_chip_that_opens_in_quick_look() {
    run(async |cx| {
        let log = trek_app::artifact::fake_web(true).unwrap();
        let trek = common::open_with(cx, |s| s.general.interactive_artifacts = false);
        note_with_video(&trek, cx).await;
        assert!(
            trek.window(cx, |window, cx| trek_app::artifact::placements(window, cx))
                .is_empty()
        );
        assert_eq!(log.borrow().made, 0);
    });
}

#[test]
fn a_phones_changes_that_arrive_mid_typing_are_read_without_losing_the_typing() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        let notes = notes_view(&trek, cx);
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.set_text("mine".into(), window, cx))
        });
        assert_eq!(
            cx.read(|cx| notes.read(cx).listed()),
            (vec!["mine".to_string()], true),
            "unsaved"
        );
        // The phone adds a note while it is.
        let theirs = trek_core::notes::create_in(&trek_core::notes::notes_dir()).unwrap();
        std::fs::write(&theirs.path, "from the phone").unwrap();
        trek.update(cx, |ws, cx| {
            ws.notes_epoch += 1;
            cx.notify();
        });
        assert_eq!(
            cx.read(|cx| notes.read(cx).listed()),
            (vec!["mine".to_string(), "from the phone".to_string()], true)
        );
        // The save lands, the typing intact, and the phone's note is still listed.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        assert_eq!(
            cx.read(|cx| notes.read(cx).listed()),
            (
                vec!["mine".to_string(), "from the phone".to_string()],
                false
            )
        );
    });
}

#[cfg(unix)]
#[test]
fn a_note_that_could_not_be_saved_stays_unsaved_and_is_tried_again() {
    use std::os::unix::fs::PermissionsExt;
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        let notes = notes_view(&trek, cx);
        let dir = trek_core::notes::notes_dir();
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.set_text("first".into(), window, cx))
        });
        saved(cx);
        // The folder turns read-only (a sync conflict, a full disk): the next save fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::write(dir.join("probe"), "").is_ok() {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            return; // permissions don't bind this user
        }
        trek.window(cx, |window, cx| {
            notes.update(cx, |n, cx| n.set_text("second".into(), window, cx))
        });
        saved(cx);
        assert!(cx.read(|cx| notes.read(cx).listed().1), "still unsaved");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert_eq!(saved(cx), "second");
        assert!(!cx.read(|cx| notes.read(cx).listed().1));
    });
}
