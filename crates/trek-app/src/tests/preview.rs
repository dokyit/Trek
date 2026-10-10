//! Attachments up close: the composer's thumbnails and a sent message's images open the in-app
//! preview, which steps through them, zooms, copies and takes them out of the outbox.

use super::harness::{Trek, open, run};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ClipboardEntry, TestAppContext, point, px};
use std::path::{Path, PathBuf};
use trek_core::RunState;
use trek_core::store::Item;

/// A `w`×`h` PNG named `name` in the project.
fn png(trek: &Trek, name: &str, w: u32, h: u32) -> PathBuf {
    let path = trek.project.join(name);
    image::RgbaImage::from_pixel(w, h, image::Rgba([90, 140, 200, 255])).save(&path).expect("png");
    path
}

/// Attach `paths` to the main composer, as a drop or paste does.
fn attach(trek: &Trek, cx: &mut TestAppContext, paths: &[PathBuf]) {
    let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
    composer.update(cx, |c, cx| paths.iter().for_each(|p| c.attach_image(p.clone(), cx)));
    trek.render(cx);
}

fn outbox(trek: &Trek, cx: &TestAppContext) -> Vec<PathBuf> {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).attached().0)
}

/// The image the preview shows and whether it's at actual pixels; `None` while it's closed.
fn showing(trek: &Trek, cx: &TestAppContext) -> Option<(PathBuf, bool)> {
    cx.read(|cx| trek.root.read(cx).preview.read(cx).current())
}

fn shown(trek: &Trek, cx: &TestAppContext) -> Option<PathBuf> {
    showing(trek, cx).map(|(p, _)| p)
}

#[test]
fn a_thumbnail_previews_the_outbox_from_that_image() {
    run(async |cx| {
        let trek = open(cx);
        let images = [png(&trek, "a.png", 40, 30), png(&trek, "b.png", 60, 40), png(&trek, "c.png", 20, 20)];
        attach(&trek, cx, &images);
        assert!(!trek.visible(cx, "attachment-preview"));

        trek.click(cx, ("attachment", 1usize));
        assert!(trek.visible(cx, "attachment-preview"));
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[1].as_path()));
        assert!(trek.visible(cx, "preview-remove"), "the outbox's images can be taken out from here");
        assert!(trek.visible(cx, ("preview-thumb", 2usize)), "a filmstrip for several");

        // → and ← step through them, round the ends; the filmstrip goes straight to one.
        trek.press(cx, "right");
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[2].as_path()));
        trek.press(cx, "right");
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[0].as_path()));
        trek.press(cx, "left");
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[2].as_path()));
        trek.click(cx, ("preview-thumb", 1usize));
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[1].as_path()));
        // The chevrons are up with the pointer over the image (not only beside it), and step too.
        trek.window(cx, |window, cx| window.hover("preview-image", cx));
        assert!(trek.visible(cx, "preview-next"));
        trek.click(cx, "preview-next");
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[2].as_path()));
        trek.click(cx, "preview-prev");
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[1].as_path()));
        assert!(trek.visible(cx, "attachment-preview"), "a click on a chevron isn't one beside the image");

        // Esc puts it away and the keyboard is back in the composer.
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "attachment-preview"));
        assert_eq!(shown(&trek, cx), None);
        trek.type_text(cx, "with these");
        assert_eq!(trek.composer_text(cx), "with these");
        assert_eq!(outbox(&trek, cx), images, "looking doesn't change what goes");
    });
}

#[test]
fn remove_in_the_preview_takes_it_out_of_the_outbox() {
    run(async |cx| {
        let trek = open(cx);
        let images = [png(&trek, "a.png", 40, 30), png(&trek, "b.png", 60, 40)];
        attach(&trek, cx, &images);
        trek.click(cx, ("attachment", 0usize));
        trek.click(cx, "preview-remove");
        assert_eq!(outbox(&trek, cx), [images[1].clone()]);
        assert_eq!(shown(&trek, cx).as_deref(), Some(images[1].as_path()), "the next one shows");
        trek.render(cx);
        assert!(!trek.visible(cx, ("attachment", 1usize)), "the strip has one thumbnail left");
        // The last one out closes it.
        trek.click(cx, "preview-remove");
        assert!(outbox(&trek, cx).is_empty());
        assert!(!trek.visible(cx, "attachment-preview"));
    });
}

#[test]
fn the_x_on_a_thumbnail_still_removes_without_previewing() {
    run(async |cx| {
        let trek = open(cx);
        let images = [png(&trek, "a.png", 40, 30), png(&trek, "b.png", 60, 40)];
        attach(&trek, cx, &images);
        // The × shows under the pointer.
        trek.window(cx, |window, cx| window.hover(("attachment", 0usize), cx));
        trek.click(cx, ("att-x", 0usize));
        assert_eq!(outbox(&trek, cx), [images[1].clone()]);
        assert!(!trek.visible(cx, "attachment-preview"));
    });
}

#[test]
fn a_sent_messages_image_previews_in_app() {
    run(async |cx| {
        let trek = open(cx);
        let image = png(&trek, "shot.png", 80, 50);
        attach(&trek, cx, std::slice::from_ref(&image));
        let id = trek.send(cx, "what's wrong here?");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Back up to the message, above the answer.
        trek.thread_view(cx).update(cx, |v, cx| v.scroll_to_top(cx));
        trek.render(cx);
        let ix = trek.item_ix(cx, &id, |i| matches!(i, Item::User { .. }));
        trek.click(cx, ("user-img", ix * 100));
        assert!(trek.visible(cx, "attachment-preview"), "it opens here, not in another app");
        assert_eq!(shown(&trek, cx).as_deref(), Some(image.as_path()));
        assert!(!trek.visible(cx, "preview-remove"), "a sent image isn't the outbox's to take out");
        assert!(!trek.visible(cx, ("preview-thumb", 0usize)), "no filmstrip for one");
        // ⌘W puts the preview away, not the thread's tab under it.
        trek.press(cx, "secondary-w");
        assert!(!trek.visible(cx, "attachment-preview"));
        assert_eq!(trek.thread_id(cx), id);
    });
}

#[test]
fn a_big_image_fits_and_zooms_to_actual_pixels() {
    run(async |cx| {
        let trek = open(cx);
        // Far bigger than the 1280×820 window at any scale.
        let big = png(&trek, "big.png", 4000, 2600);
        attach(&trek, cx, std::slice::from_ref(&big));
        trek.click(cx, ("attachment", 0usize));
        assert_eq!(showing(&trek, cx), Some((big.clone(), false)), "fit to the window first");
        let fit = trek.bounds(cx, "preview-image").expect("drawn");
        assert!(fit.size.width < px(1280.) && fit.size.height < px(820.));
        assert!(trek.visible(cx, "preview-name") && trek.visible(cx, "preview-meta"));

        trek.press(cx, "space");
        assert_eq!(showing(&trek, cx), Some((big.clone(), true)));
        trek.press(cx, "secondary-0");
        assert_eq!(showing(&trek, cx), Some((big.clone(), false)));
        trek.press(cx, "secondary-=");
        assert_eq!(showing(&trek, cx), Some((big.clone(), true)));
        trek.press(cx, "space");
        // A click on the image zooms too, and Esc closes from actual pixels.
        trek.click(cx, "preview-image");
        assert_eq!(showing(&trek, cx), Some((big.clone(), true)));
        trek.press(cx, "escape");
        assert_eq!(showing(&trek, cx), None);
    });
}

#[test]
fn a_small_image_isnt_blown_up_and_a_click_beside_it_closes() {
    run(async |cx| {
        let trek = open(cx);
        let icon = png(&trek, "icon.png", 16, 16);
        attach(&trek, cx, std::slice::from_ref(&icon));
        trek.click(cx, ("attachment", 0usize));
        let drawn = trek.bounds(cx, "preview-image").expect("drawn");
        assert!(drawn.size.width <= px(16.), "at most actual size: {drawn:?}");
        // Nothing to zoom to.
        trek.press(cx, "space");
        assert_eq!(showing(&trek, cx), Some((icon.clone(), false)));
        // A click on the image keeps it up; one beside it puts it away.
        trek.click(cx, "preview-image");
        assert_eq!(showing(&trek, cx), Some((icon, false)));
        trek.window(cx, |window, cx| window.click_at("attachment-preview", point(px(200.), px(400.)), cx));
        cx.run_until_parked();
        assert_eq!(showing(&trek, cx), None);
    });
}

#[test]
fn copy_puts_the_image_on_the_clipboard() {
    run(async |cx| {
        let trek = open(cx);
        let image = png(&trek, "a.png", 40, 30);
        attach(&trek, cx, std::slice::from_ref(&image));
        trek.click(cx, ("attachment", 0usize));
        trek.click(cx, "preview-copy");
        let item = cx.read_from_clipboard().expect("copied");
        let copied = item.entries().iter().find_map(|e| if let ClipboardEntry::Image(i) = e { Some(i.bytes().to_vec()) } else { None });
        assert_eq!(copied, Some(std::fs::read(&image).unwrap()));
    });
}

#[test]
fn files_trek_cant_draw_go_to_quick_look() {
    run(async |cx| {
        let trek = open(cx);
        let pdf = trek.project.join("spec.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        let png = png(&trek, "a.png", 40, 30);
        crate::image_preview::QUICK_LOOKED.with(|q| q.borrow_mut().clear());
        let list = vec![png.clone(), pdf.clone()];
        trek.window(cx, |window, cx| crate::image_preview::open(list.clone(), 1, window, cx));
        cx.run_until_parked();
        assert!(!trek.visible(cx, "attachment-preview"));
        assert_eq!(crate::image_preview::QUICK_LOOKED.with(|q| q.borrow().clone()), [pdf.clone()]);
        // From the image, the PDF isn't in the list to step to.
        trek.window(cx, |window, cx| crate::image_preview::open(list, 0, window, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "attachment-preview"));
        assert!(!trek.visible(cx, ("preview-thumb", 0usize)), "the image alone, no filmstrip");
        assert!(crate::image_preview::showable(&png) && !crate::image_preview::showable(Path::new("/nope.png")));
    });
}
