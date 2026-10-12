//! Notes take images: pasted, dropped or picked, they're kept in the notes folder's `assets/`,
//! linked from the note on a line of their own, and drawn in the preview.

use super::harness::{Trek, new_project, open, run};
use crate::notes::{Source, bring_in, file_safe, unused_name};
use gpui_kit::{ClipboardItem, ExternalPaths, FileDropEvent, Image, ImageFormat, PlatformInput, TestAppContext};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use trek_core::notes;

/// A 2×1 image encoded as `format`.
fn encoded(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), format).unwrap();
    out
}

fn png() -> Vec<u8> {
    encoded(image::ImageFormat::Png)
}

fn open_notes(trek: &Trek, cx: &mut TestAppContext) {
    trek.click(cx, "open-notes");
    trek.render(cx);
}

/// What the first note holds on disk, once what was done to it has been saved.
fn saved(cx: &mut TestAppContext) -> String {
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    notes::list_in(&notes::notes_dir()).first().map(|n| n.body.clone()).unwrap_or_default()
}

/// The note's text once an image has been linked from it (it's saved off the main thread first).
async fn linked(cx: &mut TestAppContext) -> String {
    let deadline = Instant::now() + super::harness::REAL_TIME_LIMIT;
    loop {
        let body = saved(cx);
        if body.contains("](assets/") {
            return body;
        }
        assert!(Instant::now() < deadline, "timed out waiting for an image in the note: {body:?}");
        cx.background_executor.timer(Duration::from_millis(5)).await;
    }
}

/// The alt text and file name of the first image link in `body`.
fn link_in(body: &str) -> (String, String) {
    let alt = body.split("![").nth(1).and_then(|s| s.split("](").next()).expect("an image link");
    let name = body.split("](assets/").nth(1).and_then(|s| s.split(')').next()).expect("a link into assets");
    (alt.to_string(), name.to_string())
}

/// Where the preview draws the image in `assets/<name>`.
fn drawn(trek: &Trek, cx: &mut TestAppContext, name: &str) -> bool {
    trek.render(cx);
    trek.render(cx);
    trek.visible(cx, format!("md-image-{}", notes::notes_dir().join(format!("assets/{name}")).display()))
}

fn assets() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(notes::assets_dir()).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    names.sort();
    names
}

/// Drop `paths` on the note, as the platform does: the pointer comes over it, the files enter
/// with it, and are let go.
fn drop_on_note(trek: &Trek, cx: &mut TestAppContext, paths: Vec<PathBuf>) {
    let at = trek.bounds(cx, "note-pane").expect("the note").center();
    trek.window(cx, |window, cx| {
        // Typing had the window in keyboard mode, where nothing counts as hovered until the
        // pointer moves; a real drag moves it first.
        window.dispatch_event(PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx);
    });
    trek.render(cx);
    trek.window(cx, |window, cx| {
        window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position: at, paths: ExternalPaths(paths.into()) }), cx);
    });
    trek.render(cx);
    trek.window(cx, |window, cx| {
        window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Submit { position: at }), cx);
    });
    cx.run_until_parked();
}

#[test]
fn a_pasted_screenshot_is_kept_linked_and_shown() {
    run(async |cx| {
        let trek = open(cx);
        open_notes(&trek, cx);
        trek.type_text(cx, "Login bug");
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png())));
        trek.press(cx, "secondary-v");
        let body = linked(cx).await;

        // On a line of its own after what was written, the caret on the line below.
        let (alt, name) = link_in(&body);
        assert_eq!(alt, "Pasted image");
        assert_eq!(body, format!("Login bug\n![Pasted image](assets/{name})\n"), "links are written with `/` on every platform");
        // The file is in the notes folder's assets, named for its note, and is what was pasted.
        let note = notes::list_in(&notes::notes_dir()).remove(0);
        assert!(name.starts_with(&note.id) && name.ends_with(".png"), "{name}");
        assert_eq!(assets(), [name.clone()]);
        assert_eq!(std::fs::read(notes::assets_dir().join(&name)).unwrap(), png());
        // The preview draws it; the list names it by its alt text, not its link.
        assert!(drawn(&trek, cx, &name), "the preview shows the image");
        assert_eq!((note.title(), note.preview()), ("Login bug".to_string(), "Pasted image".to_string()));
        assert!(trek.visible(cx, format!("note-{}", note.id)));
        // One step to undo: the line goes, the words stay. The image stays on disk, for a redo.
        trek.press(cx, "secondary-z");
        assert_eq!(saved(cx), "Login bug");
        assert_eq!(assets(), [name]);
    });
}

#[test]
fn an_image_is_put_at_the_caret_and_text_pastes_as_text() {
    run(async |cx| {
        let trek = open(cx);
        open_notes(&trek, cx);
        trek.type_text(cx, "before after");
        for _ in 0..6 {
            trek.press(cx, "left");
        }
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png())));
        trek.press(cx, "secondary-v");
        let body = linked(cx).await;
        let (_, name) = link_in(&body);
        assert_eq!(body, format!("before\n![Pasted image](assets/{name})\n after"));
        // Text on the clipboard still pastes as text.
        cx.write_to_clipboard(ClipboardItem::new_string("!".into()));
        trek.press(cx, "secondary-v");
        assert_eq!(saved(cx), format!("before\n![Pasted image](assets/{name})\n! after"));
        assert_eq!(assets().len(), 1);
    });
}

#[test]
fn a_dropped_image_file_is_copied_and_other_files_are_left() {
    run(async |cx| {
        let trek = open(cx);
        open_notes(&trek, cx);
        trek.type_text(cx, "Design");
        trek.render(cx);
        let shot = trek.project.join("Home screen.PNG");
        let text = trek.project.join("readme.md");
        std::fs::write(&shot, png()).unwrap();
        std::fs::write(&text, "words").unwrap();
        drop_on_note(&trek, cx, vec![text.clone(), shot.clone()]);
        let body = linked(cx).await;

        let (alt, name) = link_in(&body);
        assert_eq!(alt, "Home screen");
        assert!(name.ends_with("-Home-screen.png"), "{name}");
        assert_eq!(body, format!("Design\n![Home screen](assets/{name})\n"));
        assert_eq!(assets(), [name.clone()], "the text file isn't taken");
        assert_eq!(std::fs::read(notes::assets_dir().join(&name)).unwrap(), png());
        assert_eq!(std::fs::read(&shot).unwrap(), png(), "copied, not moved");
        assert!(drawn(&trek, cx, &name));

        // Only a text file dropped: nothing happens.
        drop_on_note(&trek, cx, vec![text]);
        assert_eq!(saved(cx), body);
        assert_eq!(assets().len(), 1);

        // Back out: the line goes, and the original is where it was.
        trek.press(cx, "secondary-z");
        assert_eq!(saved(cx), "Design");
        assert!(shot.is_file());
    });
}

#[test]
fn the_image_button_picks_images_only_into_the_note() {
    run(async |cx| {
        let trek = open(cx);
        open_notes(&trek, cx);
        assert!(trek.visible(cx, "note-image"), "an Image tool beside the others");
        let a = trek.project.join("a.png");
        let b = trek.project.join("b.png");
        let other = trek.project.join("c.txt");
        for p in [&a, &b] {
            std::fs::write(p, png()).unwrap();
        }
        std::fs::write(&other, "x").unwrap();
        // No note yet: one is made to hold them.
        trek.click(cx, "note-image");
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files && !options.directories && options.multiple);
            Some(vec![a.clone(), other.clone(), b.clone()])
        });
        let body = linked(cx).await;
        // Both images, a line each, in a single step to undo.
        let names: Vec<&str> = body.lines().map(|l| l.split("](assets/").nth(1).and_then(|s| s.strip_suffix(')')).expect("a link per line")).collect();
        assert_eq!(names.len(), 2, "{body:?}");
        assert!(names[0].ends_with("-a.png") && names[1].ends_with("-b.png"), "{names:?}");
        assert_eq!(assets().len(), 2);
        trek.press(cx, "secondary-z");
        assert_eq!(saved(cx), "");
        // Only a text file picked: a word about it, nothing added.
        trek.click(cx, "note-image");
        cx.simulate_path_prompt_response(|_| Some(vec![other.clone()]));
        cx.run_until_parked();
        assert_eq!(saved(cx), "");
        assert_eq!(assets().len(), 2);
    });
}

#[test]
fn deleting_a_note_keeps_its_images() {
    run(async |cx| {
        let trek = open(cx);
        open_notes(&trek, cx);
        trek.type_text(cx, "keep");
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png())));
        trek.press(cx, "secondary-v");
        let (_, name) = link_in(&linked(cx).await);
        trek.click(cx, "note-delete");
        assert!(notes::list_in(&notes::notes_dir()).is_empty());
        assert!(notes::assets_dir().join(&name).is_file(), "a note brought back from Deleted/ has its images");
        let kept = std::fs::read_dir(notes::notes_dir().join("Deleted")).unwrap().flatten().next().expect("the note in Deleted");
        assert!(std::fs::read_to_string(kept.path()).unwrap().contains(&name));
    });
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    names.sort();
    names
}

#[test]
fn images_are_named_for_their_note_and_never_replace_one() {
    let dir = new_project("note-images");
    let assets = dir.join("assets");
    let from = dir.join("My Shot (1).PNG");
    std::fs::write(&from, png()).unwrap();
    let link = bring_in(&Source::File(from.clone()), "0192-ab", &assets).unwrap();
    // The name says what it is; the file's name needs no escaping in a link.
    let name = link.strip_prefix("![My Shot (1)](assets/").and_then(|l| l.strip_suffix(')')).unwrap_or_else(|| panic!("{link}"));
    assert!(name.starts_with("0192-ab-") && name.ends_with("-My-Shot-1.png"), "{name}");
    assert_eq!(names_in(&assets), [name.to_string()]);
    assert_eq!(std::fs::read(assets.join(name)).unwrap(), png());
    assert!(from.is_file());

    // A name already taken gets a number.
    std::fs::write(assets.join("x.png"), b"first").unwrap();
    assert_eq!(unused_name(&assets, "x", "png"), "x-2.png");
    std::fs::write(assets.join("x-2.png"), b"second").unwrap();
    assert_eq!(unused_name(&assets, "x", "png"), "x-3.png");
    assert_eq!(unused_name(&assets, "y", "png"), "y.png");

    // Whatever the names, a link has nothing in it to escape, and a name has some stem.
    assert_eq!(file_safe("a b(c)[d]/e\\f.g"), "a-b-c-d-e-f-g");
    assert_eq!(file_safe("../.."), "image");
    assert_eq!(file_safe(&"x".repeat(200)).len(), 60);
    assert_eq!(file_safe("snapshot_é"), "snapshot_é");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn image_data_and_converted_files_become_images_in_assets() {
    let dir = new_project("note-images-convert");
    let assets = dir.join("assets");
    // Clipboard data in a format the notes can't show is saved as PNG, as the composer does.
    let link = bring_in(&Source::Data(Image::from_bytes(ImageFormat::Tiff, encoded(image::ImageFormat::Tiff))), "n1", &assets).unwrap();
    assert!(link.starts_with("![Pasted image](assets/n1-pasted-") && link.ends_with(".png)"), "{link}");
    // A BMP file is copied as a PNG (macOS's sips, or the image crate where there is none).
    let bmp = dir.join("paint.bmp");
    std::fs::write(&bmp, encoded(image::ImageFormat::Bmp)).unwrap();
    let link = bring_in(&Source::File(bmp), "n1", &assets).unwrap();
    let name = link.strip_prefix("![paint](assets/").and_then(|l| l.strip_suffix(')')).unwrap_or_else(|| panic!("{link}"));
    assert!(name.ends_with("-paint.png"), "{name}");
    let back = image::open(assets.join(name)).unwrap().to_rgba8();
    assert_eq!((back.width(), back.height()), (2, 1));
    // What can't be read leaves nothing behind but what was there.
    let before = names_in(&assets);
    let broken = dir.join("broken.heic");
    std::fs::write(&broken, b"not an image").unwrap();
    assert_eq!(bring_in(&Source::File(broken), "n1", &assets).unwrap_err().to_string(), "it couldn't be converted to PNG");
    assert!(bring_in(&Source::Data(Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec())), "n1", &assets).is_err());
    assert_eq!(names_in(&assets), before);
    // A note id with odd characters (a file someone put there by hand) is still a safe name.
    let link = bring_in(&Source::File(dir.join("paint.bmp")), "my note (old)", &assets).unwrap();
    assert!(link.contains("](assets/my-note-old-"), "{link}");
    let _ = std::fs::remove_dir_all(dir);
}
