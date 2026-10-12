//! Pictures in notes: dropped (or picked) image files are kept in the notes folder and linked
//! where the cursor is, the preview draws them fit to its width, Write shows them as tiles, a
//! missing one is a placeholder, and deleting the note takes its pictures along to Deleted.

use super::harness::{Trek, open, run};
use crate::notes::Mode;
use gpui_kit::{Entity, TestAppContext};
use std::path::PathBuf;

/// A real PNG, `w`×`h` pixels.
fn png(path: &std::path::Path, w: u32, h: u32) {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([90, 140, 220, 255]));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

fn notes_view(trek: &Trek, cx: &mut TestAppContext) -> Entity<crate::notes::NotesView> {
    cx.read(|cx| trek.root.read(cx).notes.clone())
}

/// What the note holds on disk, once it's been saved.
fn saved(cx: &mut TestAppContext) -> String {
    cx.executor().advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    trek_core::notes::list_in(&trek_core::notes::notes_dir()).first().map(|n| n.body.clone()).unwrap_or_default()
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
        let item = gpui_kit::ClipboardItem { entries: vec![gpui_kit::ClipboardEntry::Image(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, std::fs::read(&shot).unwrap()))] };
        let taken = trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.paste(&item, window, cx)));
        assert!(taken, "the text box doesn't paste it as text");
        let body = landed(cx, "![Pasted image](attachments/");
        assert!(body.trim_end().ends_with(".png)"), "{body:?}");
        // An SVG copied in Finder goes in too (the composer's paste leaves those to text).
        let svg = trek_core::paths::data_dir().join("diagram.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>").unwrap();
        let item = gpui_kit::ClipboardItem { entries: vec![gpui_kit::ClipboardEntry::ExternalPaths(gpui_kit::ExternalPaths(vec![svg].into()))] };
        assert!(trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.paste(&item, window, cx))));
        landed(cx, "![diagram](attachments/");
        // Plain text is the text box's.
        let text = gpui_kit::ClipboardItem::new_string("just words".into());
        assert!(!trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.paste(&text, window, cx))));
    });
}

#[test]
fn pictures_go_into_a_note_and_show_in_every_mode() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        assert!(trek.visible(cx, "note-image"), "Add image is on the toolbar");
        let notes = notes_view(&trek, cx);
        let outside = trek_core::paths::data_dir().join("from-finder");
        std::fs::create_dir_all(&outside).unwrap();
        let shot = outside.join("Whiteboard.png");
        png(&shot, 1600, 800);
        let readme = outside.join("README.txt");
        std::fs::write(&readme, "not a picture").unwrap();

        // Dropped (as the driver's `note image` does it) with the cursor after the first line.
        trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.set_text("Trip\n".into(), window, cx)));
        trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.add_files(vec![shot.clone(), readme.clone()], window, cx)));
        let body = landed(cx, "](attachments/");
        let image = trek_core::notes::images(&body).pop().expect("an image in the note");
        assert_eq!(image.alt, "Whiteboard");
        assert!(body.starts_with("Trip\n![Whiteboard](attachments/"), "{body:?}");
        let kept = trek_core::notes::notes_dir().join(&image.link);
        assert_eq!(std::fs::read(&kept).unwrap(), std::fs::read(&shot).unwrap(), "a copy kept with the notes");
        assert!(!body.contains("README"), "only pictures go in");

        // Split (the default): the preview draws it, fit to its width in its own proportions.
        trek.render(cx);
        let name = kept.file_name().unwrap().to_string_lossy().to_string();
        let drawn = trek.bounds(cx, gpui_kit::SharedString::from(format!("note-image-{name}"))).expect("the picture in the preview");
        let preview = trek.bounds(cx, "note-preview").expect("the preview");
        assert!(drawn.size.width < preview.size.width, "{drawn:?} fits in {preview:?}");
        assert!(drawn.size.width.as_f32() < 1600., "a big picture is drawn smaller");
        let aspect = drawn.size.width.as_f32() / drawn.size.height.as_f32();
        assert!((aspect - 2.).abs() < 0.05, "its proportions kept: {aspect}");

        // Write: the text shows its link, the picture is a tile under it.
        notes.update(cx, |n, cx| n.set_mode(Mode::Write, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "note-images") && trek.visible(cx, ("note-thumb", 0usize)));
        assert!(!trek.visible(cx, "note-preview"));

        // A picture that isn't there: a placeholder that says so, nothing worse.
        trek.window(cx, |window, cx| notes.update(cx, |n, cx| n.set_text(format!("{body}![gone](attachments/nope.png)\n"), window, cx)));
        notes.update(cx, |n, cx| n.set_mode(Mode::Preview, cx));
        trek.render(cx);
        assert!(trek.visible(cx, gpui_kit::SharedString::from(format!("note-image-{name}"))));
        assert!(trek.visible(cx, "note-image-attachments/nope.png"), "a placeholder for the missing one");

        // Deleted: its picture goes along to Deleted, where the note still shows it.
        trek.click(cx, "note-delete");
        cx.run_until_parked();
        assert!(!kept.exists(), "out of the notes' own attachments");
        let bin: PathBuf = trek_core::notes::notes_dir().join("Deleted").join(&image.link);
        assert!(bin.exists(), "kept beside the deleted note");
    });
}
