//! A file dragged in from another app reaches the composer even when the keyboard was the last
//! thing used: typing a message and then fetching a file from Finder used to leave the drop with
//! no target, because nothing in the window counted as under the pointer.

mod common;

use common::{Trek, open, run};
use gpui_kit::{
    ExternalPaths, FileDropEvent, Pixels, PlatformInput, Point, TestAppContext, point, px,
};
use std::path::PathBuf;

fn file(trek: &Trek, name: &str) -> PathBuf {
    let path = trek.project.join(name);
    std::fs::write(&path, b"%PDF-1.4 a file").expect("file");
    path
}

/// A drag of `paths` from another app that comes in at the window's edge, moves to `at` and is
/// let go there, as the platform reports it.
fn drop_at(trek: &Trek, cx: &mut TestAppContext, paths: &[PathBuf], at: Point<Pixels>) {
    let paths = ExternalPaths(paths.to_vec().into());
    trek.window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Entered {
                position: point(px(1.), at.y),
                paths,
            }),
            cx,
        );
    });
    cx.run_until_parked();
    trek.window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Pending { position: at }),
            cx,
        );
    });
    cx.run_until_parked();
    trek.window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::FileDrop(FileDropEvent::Submit { position: at }),
            cx,
        );
    });
    cx.run_until_parked();
}

fn files(trek: &Trek, cx: &TestAppContext) -> Vec<PathBuf> {
    cx.read(|cx| trek.root.read(cx).composer.read(cx).attached_files())
}

#[test]
fn a_drop_lands_after_typing() {
    run(async |cx| {
        let trek = open(cx);
        let _id = trek.quiet_thread(cx);
        trek.render(cx);
        // Typing leaves the keyboard the last input: the drop used to find nothing under the
        // pointer then, so the card never took the file.
        trek.type_live(cx, "hello");
        let f = file(&trek, "a.pdf");
        let card = trek
            .bounds(cx, "composer-card")
            .expect("the composer card is drawn");
        let at = card.center();
        drop_at(&trek, cx, std::slice::from_ref(&f), at);
        assert_eq!(files(&trek, cx), vec![f]);
    });
}
