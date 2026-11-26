//! App Snapshots through Snipping Tool (Windows): the composer opens the overlay, waits for the
//! image it leaves on the clipboard and attaches it as a paste would. A fake overlay and clipboard
//! stand in for Windows', so these run on every platform, on a mock clock.
//!
//! And on a Linux desktop (`desktop`, below): through the desktop's screenshot portal or the X
//! server, by what the session has. A desktop of the tests' own stands in for both, so these too
//! run on every platform.

mod common;

use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use trek_app::screenclip::{Backend, Clip, Read, Source};
use trek_app::workspace::WorkspaceEvent;
use trek_core::RunState;
use trek_core::store::Item;

const INTERVAL: Duration = Duration::from_millis(5);

/// What the fake overlay did, and what its clipboard holds.
#[derive(Default)]
struct Pad {
    sequence: u32,
    image: Option<Clip>,
    /// Looks at the clipboard so far.
    looks: usize,
    launches: usize,
    launch_error: Option<String>,
}

struct Fake {
    pad: Arc<Mutex<Pad>>,
    timeout: Duration,
}

struct FakeClipboard(Arc<Mutex<Pad>>);

impl Backend for Fake {
    fn launch(&self) -> Result<(), String> {
        let mut pad = self.pad.lock().unwrap();
        pad.launches += 1;
        pad.launch_error.clone().map_or(Ok(()), Err)
    }
    fn source(&self) -> Box<dyn Source> {
        Box::new(FakeClipboard(self.pad.clone()))
    }
    fn interval(&self) -> Duration {
        INTERVAL
    }
    fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Source for FakeClipboard {
    fn sequence(&mut self) -> u32 {
        let mut pad = self.0.lock().unwrap();
        pad.looks += 1;
        pad.sequence
    }
    fn read(&mut self) -> Read {
        self.0
            .lock()
            .unwrap()
            .image
            .clone()
            .map_or(Read::NotAnImage, Read::Image)
    }
}

/// The overlay and clipboard Trek uses from now on.
fn install(cx: &mut TestAppContext, timeout: Duration) -> Arc<Mutex<Pad>> {
    let pad = Arc::new(Mutex::new(Pad::default()));
    let fake = Arc::new(Fake {
        pad: pad.clone(),
        timeout,
    });
    cx.update(|cx| trek_app::screenclip::install(fake, cx));
    pad
}

/// The user snipped: the clipboard holds a 1×1 picture of `rgb`.
fn snip(pad: &Mutex<Pad>, rgb: [u8; 3]) {
    let mut dib = Vec::new();
    for field in [40u32, 1, 1] {
        dib.extend_from_slice(&field.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&24u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    dib.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
    let mut pad = pad.lock().unwrap();
    pad.sequence += 1;
    pad.image = Some(Clip::Dib(dib));
}

/// Let `ticks` intervals pass, with the background work they start getting the time it takes.
fn pass(cx: &mut TestAppContext, ticks: usize) {
    for _ in 0..ticks {
        cx.executor().advance_clock(INTERVAL);
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(3));
    }
}

/// Pass intervals until `done`, up to a few seconds of real time.
fn until(cx: &mut TestAppContext, what: &str, done: impl Fn(&mut TestAppContext) -> bool) {
    for _ in 0..1000 {
        if done(cx) {
            return;
        }
        pass(cx, 1);
    }
    panic!("timed out waiting for {what}");
}

fn toasts(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
            if let WorkspaceEvent::Toast { message, .. } | WorkspaceEvent::Failure { message } =
                event
            {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    seen
}

fn composer(trek: &Trek, cx: &TestAppContext) -> gpui_kit::Entity<trek_app::composer::Composer> {
    cx.read(|cx| trek.root.read(cx).composer.clone())
}

fn attached(trek: &Trek, cx: &TestAppContext) -> Vec<PathBuf> {
    composer(trek, cx).read_with(cx, |c, _| c.attached()).0
}

fn start(trek: &Trek, cx: &mut TestAppContext) {
    composer(trek, cx).update(cx, |c, cx| c.snapshot_by_screenclip(cx));
    trek.render(cx);
}

#[test]
fn a_snip_that_arrives_attaches_like_a_paste_and_goes_with_the_message() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_secs(30));
        // A picture already on the clipboard before the overlay opens isn't the snip.
        snip(&pad, [1, 2, 3]);
        trek.type_text(cx, "what is wrong here");
        start(&trek, cx);
        assert_eq!(pad.lock().unwrap().launches, 1, "the overlay opened");
        assert!(
            trek.visible(cx, "snapshot-wait"),
            "the wait is said in the strip"
        );
        pass(cx, 5);
        assert!(attached(&trek, cx).is_empty(), "nothing was picked yet");
        assert!(trek.visible(cx, "snapshot-wait"));

        snip(&pad, [10, 120, 250]);
        until(cx, "the snip to attach", |cx| {
            !attached(&trek, cx).is_empty()
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "snapshot-wait"), "the wait is over");
        let path = attached(&trek, cx)[0].clone();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(
            path.starts_with(trek_core::paths::data_dir().join("snapshots")),
            "saved with the others: {}",
            path.display()
        );
        let picture = image::open(&path).expect("a PNG").to_rgb8();
        assert_eq!(
            (picture.width(), picture.height(), picture.get_pixel(0, 0).0),
            (1, 1, [10, 120, 250])
        );

        // It goes out with the next message, as a pasted one does.
        trek.press(cx, "enter");
        let id = trek.thread_id(cx);
        let images = trek
            .items(cx, &id)
            .into_iter()
            .find_map(|i| {
                if let Item::User { images, .. } = i {
                    Some(images)
                } else {
                    None
                }
            })
            .expect("sent");
        assert_eq!(images, [path.display().to_string()]);
        assert!(attached(&trek, cx).is_empty(), "the outbox empties on send");
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn asking_again_while_waiting_gives_up_quietly() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_secs(30));
        let seen = toasts(&trek, cx);
        start(&trek, cx);
        pass(cx, 3);
        assert!(trek.visible(cx, "snapshot-wait"));

        // The shortcut again: no new overlay, and the wait is over.
        start(&trek, cx);
        assert!(!trek.visible(cx, "snapshot-wait"));
        assert_eq!(pad.lock().unwrap().launches, 1);
        let looks = pad.lock().unwrap().looks;
        // A picture that lands afterwards is nobody's snapshot, and nothing was said of it.
        snip(&pad, [9, 9, 9]);
        pass(cx, 10);
        assert_eq!(
            pad.lock().unwrap().looks,
            looks,
            "the clipboard isn't looked at any more"
        );
        assert!(attached(&trek, cx).is_empty());
        assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());

        // And it can be taken again.
        start(&trek, cx);
        assert_eq!(pad.lock().unwrap().launches, 2);
        snip(&pad, [1, 1, 1]);
        until(cx, "the second snip", |cx| !attached(&trek, cx).is_empty());
    });
}

#[test]
fn nothing_picked_in_time_says_so() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_millis(60));
        let seen = toasts(&trek, cx);
        // Text copied while the overlay is up is not a snapshot either.
        start(&trek, cx);
        pad.lock().unwrap().sequence += 1;
        until(cx, "the wait to time out", |_| !seen.borrow().is_empty());
        let said = seen.borrow().clone();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with("Nothing was picked in a minute"),
            "{said:?}"
        );
        assert!(attached(&trek, cx).is_empty());
        trek.render(cx);
        assert!(!trek.visible(cx, "snapshot-wait"));
        // Free to try again.
        start(&trek, cx);
        assert_eq!(pad.lock().unwrap().launches, 2);
    });
}

#[test]
fn an_overlay_that_wont_open_is_said() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_secs(30));
        pad.lock().unwrap().launch_error = Some("Windows answered 31".into());
        let seen = toasts(&trek, cx);
        start(&trek, cx);
        assert_eq!(
            *seen.borrow(),
            ["Couldn't open the snipping tool: Windows answered 31."]
        );
        assert!(!trek.visible(cx, "snapshot-wait"));
        assert!(
            !composer(&trek, cx).read_with(cx, |c, _| c.snapshot_waiting()),
            "nothing is waiting"
        );
    });
}

#[test]
fn closing_the_window_ends_the_wait() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_secs(30));
        start(&trek, cx);
        pass(cx, 3);
        let looks = pad.lock().unwrap().looks;
        assert!(looks > 1, "it was looking");
        // The composer itself may well live on (it is only let go with its last reference); the
        // wait must end with the window all the same.
        trek.window(cx, |window, _| window.remove_window());
        cx.run_until_parked();
        pass(cx, 5);
        let after = pad.lock().unwrap().looks;
        pass(cx, 20);
        assert_eq!(
            pad.lock().unwrap().looks,
            after,
            "no one is looking at the clipboard once the window is gone"
        );
    });
}

/// Ctrl+Shift+S on Windows opens the overlay (on a Mac it runs `screencapture`, so not here).
#[cfg(windows)]
#[test]
fn the_shortcut_opens_the_overlay() {
    run(async |cx| {
        let trek = open(cx);
        let pad = install(cx, Duration::from_secs(30));
        trek.press(cx, "secondary-shift-s");
        assert_eq!(pad.lock().unwrap().launches, 1);
        assert!(trek.visible(cx, "snapshot-wait"));
        snip(&pad, [5, 6, 7]);
        until(cx, "the snip to attach", |cx| {
            !attached(&trek, cx).is_empty()
        });
    });
}

/// A Linux desktop's snapshots: the + menu, the shortcut and Settings offer what the session can
/// take (a runtime value), and what is taken is attached. The portal and the X server are the
/// tests' own (`screenclip::desktop::pretend`), on whichever platform this runs.
mod desktop {
    use super::*;
    use gpui_kit::{AnyWindowHandle, App};
    use trek_app::composer::Composer;
    use trek_app::screenclip::desktop::{
        Asked, Capture, Frame, Mode, NOTHING_TO_CAPTURE_WITH, Session, pretend,
    };
    use trek_app::screenclip::{Row, Tool};
    use trek_app::settings_view::shown;
    use trek_app::workspace::{Route, SettingsPage};
    use trek_core::settings::{SnapshotFormat, SnapshotMode};

    const SETTLE: Duration = Duration::from_millis(20);

    /// What the desktop was asked, and what it answers.
    #[derive(Default)]
    struct Said {
        /// "pick", "portal screen" (the portal without its picker) and "x screen", in order.
        asked: Vec<&'static str>,
        /// Times Trek's windows stepped aside, and came back.
        aside: usize,
        back: usize,
        portal: Option<Asked>,
    }

    struct Desktop {
        session: Session,
        said: Arc<Mutex<Said>>,
    }

    impl Capture for Desktop {
        fn session(&self) -> Session {
            self.session
        }
        fn ask_portal(&self, interactive: bool) -> Asked {
            let mut said = self.said.lock().unwrap();
            said.asked
                .push(if interactive { "pick" } else { "portal screen" });
            said.portal
                .clone()
                .unwrap_or(Asked::Failed("no portal".into()))
        }
        fn screen(&self) -> Result<Frame, String> {
            self.said.lock().unwrap().asked.push("x screen");
            // A 4×2 screen, all one colour.
            Ok(Frame {
                width: 4,
                height: 2,
                rgb: [10, 120, 250].repeat(8),
            })
        }
        fn step_aside(&self, _: &mut App) {
            self.said.lock().unwrap().aside += 1;
        }
        fn come_back(&self, _: AnyWindowHandle, _: &mut App) {
            self.said.lock().unwrap().back += 1;
        }
        fn settle(&self) -> Duration {
            SETTLE
        }
    }

    /// From now until the guard goes, this thread is on a Linux desktop with `portal` and `x11`
    /// (its words Linux's too, as Settings read them).
    fn desktop(portal: bool, x11: bool) -> (Arc<Mutex<Said>>, impl Drop) {
        struct Both<A, B> {
            _desktop: A,
            _words: B,
        }
        impl<A, B> Drop for Both<A, B> {
            fn drop(&mut self) {}
        }
        let said = Arc::new(Mutex::new(Said::default()));
        let _words = trek_app::words::pretend_os(trek_app::words::Os::Linux);
        let _desktop = pretend(Arc::new(Desktop {
            session: Session { portal, x11 },
            said: said.clone(),
        }));
        (said, Both { _desktop, _words })
    }

    /// The desktop's portal saved a 1×1 screenshot of `rgb` among the user's pictures, and names
    /// it as GNOME's does: by a URI, the space in its name encoded.
    fn portal_saved(said: &Mutex<Said>, rgb: [u8; 3]) -> PathBuf {
        let dir = trek_core::paths::data_dir().join("Pictures");
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = dir.join("Screenshot From 2026-11-26.png");
        image::RgbImage::from_pixel(1, 1, image::Rgb(rgb))
            .save(&theirs)
            .unwrap();
        let path = theirs
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20");
        let uri = if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        };
        said.lock().unwrap().portal = Some(Asked::Saved(uri));
        theirs
    }

    /// Time for the windows to step aside, then the capture, as long as it takes.
    fn until_attached(trek: &Trek, cx: &mut TestAppContext, n: usize) -> PathBuf {
        cx.executor().advance_clock(SETTLE);
        until(cx, "the snapshot to attach", |cx| {
            attached(trek, cx).len() >= n
        });
        attached(trek, cx)[n - 1].clone()
    }

    /// Time enough for a snapshot that attaches nothing to be over.
    fn settle(cx: &mut TestAppContext) {
        cx.executor().advance_clock(SETTLE);
        pass(cx, 20);
    }

    fn said_on(trek: &Trek, cx: &mut TestAppContext, page: SettingsPage) -> Vec<String> {
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(page), cx));
        shown::take();
        trek.render(cx);
        shown::take()
    }

    fn stepped(said: &Mutex<Said>) -> (usize, usize) {
        let said = said.lock().unwrap();
        (said.aside, said.back)
    }

    #[test]
    fn a_pick_in_the_desktops_tool_attaches_what_the_portal_saved() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(true, true);
            let theirs = portal_saved(&said, [10, 120, 250]);
            let seen = toasts(&trek, cx);
            trek.type_text(cx, "what is wrong here");
            // Settings start on "a window", which the desktop's own picker decides: ⌘⇧S asks it.
            trek.press(cx, "secondary-shift-s");
            let path = until_attached(&trek, cx, 1);
            assert_eq!(said.lock().unwrap().asked, ["pick"]);
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
            assert!(
                path.starts_with(trek_core::paths::data_dir().join("snapshots")),
                "saved with the others: {}",
                path.display()
            );
            let picture = image::open(&path).expect("a PNG").to_rgb8();
            assert_eq!(
                (picture.width(), picture.height(), picture.get_pixel(0, 0).0),
                (1, 1, [10, 120, 250])
            );
            assert!(
                theirs.exists(),
                "the desktop's own copy is left where it is"
            );
            // Hide Trek while capturing is on unless turned off: aside before, back after.
            assert_eq!(stepped(&said), (1, 1));
            assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());

            // It goes out with the next message.
            trek.press(cx, "enter");
            let id = trek.thread_id(cx);
            let images = trek
                .items(cx, &id)
                .into_iter()
                .find_map(|i| {
                    if let Item::User { images, .. } = i {
                        Some(images)
                    } else {
                        None
                    }
                })
                .expect("sent");
            assert_eq!(images, [path.display().to_string()]);
            trek.wait_done(cx, &id, RunState::Idle).await;
        });
    }

    #[test]
    fn the_screen_comes_from_the_x_server_in_the_format_settings_ask_for() {
        run(async |cx| {
            let trek = common::open_with(cx, |s| {
                s.snapshots.format = SnapshotFormat::Jpg;
                s.snapshots.hide_trek = false;
                s.snapshots.default_mode = SnapshotMode::Screen;
            });
            // A portal is there too: the screen still comes straight from the X server.
            let (said, _desktop) = desktop(true, true);
            trek.press(cx, "secondary-shift-s");
            let path = until_attached(&trek, cx, 1);
            assert_eq!(said.lock().unwrap().asked, ["x screen"]);
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("jpg"));
            assert_eq!(
                image::guess_format(&std::fs::read(&path).unwrap()).unwrap(),
                image::ImageFormat::Jpeg
            );
            let picture = image::open(&path).expect("a JPEG").to_rgb8();
            assert_eq!((picture.width(), picture.height()), (4, 2));
            assert_eq!(stepped(&said), (0, 0), "Hide Trek is off");
        });
    }

    #[test]
    fn a_wayland_sessions_screen_is_the_portals_without_its_picker() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(true, false);
            portal_saved(&said, [1, 2, 3]);
            composer(&trek, cx).update(cx, |c, cx| c.snapshot_by_desktop(Mode::Screen, cx));
            until_attached(&trek, cx, 1);
            assert_eq!(said.lock().unwrap().asked, ["portal screen"]);
        });
    }

    #[test]
    fn a_bare_x11_session_takes_the_screen_whatever_the_shortcut_is_set_to() {
        run(async |cx| {
            // "A window" (the default): nothing picks one here, so the screen it is.
            let trek = open(cx);
            let (said, _desktop) = desktop(false, true);
            assert_eq!(Composer::snapshot_tool().rows(), [Row::Screen]);
            trek.press(cx, "secondary-shift-s");
            let path = until_attached(&trek, cx, 1);
            assert_eq!(said.lock().unwrap().asked, ["x screen"]);
            let picture = image::open(&path).expect("a PNG").to_rgb8();
            assert_eq!(
                (picture.width(), picture.height(), picture.get_pixel(3, 1).0),
                (4, 2, [10, 120, 250])
            );
        });
    }

    #[test]
    fn the_plus_menu_offers_what_the_session_can_take() {
        run(async |cx| {
            let trek = open(cx);
            for (portal, x11, pick, screen) in [
                (true, true, true, true),
                (true, false, true, true),
                (false, true, false, true),
                (false, false, false, false),
            ] {
                let (_said, _desktop) = desktop(portal, x11);
                trek.click(cx, "attach");
                trek.render(cx);
                assert!(trek.visible(cx, "plus-mention"), "the menu is open");
                assert_eq!(
                    trek.visible(cx, "plus-snapshot"),
                    pick,
                    "Take a snapshot, portal {portal}, X11 {x11}"
                );
                assert_eq!(
                    trek.visible(cx, "plus-snapshot-screen"),
                    screen,
                    "Snapshot the screen, portal {portal}, X11 {x11}"
                );
                // A Mac's rows, which `screencapture` takes, aren't a desktop's.
                assert!(!trek.visible(cx, "plus-snapshot-window"));
                assert!(!trek.visible(cx, "plus-snapshot-area"));
                trek.press(cx, "escape");
                trek.render(cx);
                assert!(!trek.visible(cx, "plus-mention"), "the menu is shut");
            }
        });
    }

    #[test]
    fn the_menus_rows_take_what_they_say() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(true, true);
            portal_saved(&said, [7, 8, 9]);
            trek.click(cx, "attach");
            trek.render(cx);
            trek.click(cx, "plus-snapshot-screen");
            until_attached(&trek, cx, 1);
            assert_eq!(said.lock().unwrap().asked, ["x screen"]);
            trek.render(cx);
            if !trek.visible(cx, "plus-snapshot") {
                trek.click(cx, "attach");
                trek.render(cx);
            }
            trek.click(cx, "plus-snapshot");
            until_attached(&trek, cx, 2);
            assert_eq!(said.lock().unwrap().asked, ["x screen", "pick"]);
        });
    }

    #[test]
    fn settings_offer_what_applies_on_a_desktop() {
        run(async |cx| {
            let trek = open(cx);
            {
                let (_said, _desktop) = desktop(true, true);
                assert!(trek_app::settings_view::pages().any(|p| p == SettingsPage::Snapshots));
                let snapshots = said_on(&trek, cx, SettingsPage::Snapshots);
                for row in [
                    "The + menu in the composer offers both.",
                    "Hide Trek while capturing",
                    "Format",
                    "Storage",
                    "Keep snapshots",
                    "Snapshot folder",
                ] {
                    assert!(snapshots.iter().any(|s| s == row), "{row}: {snapshots:?}");
                }
                assert!(
                    snapshots.iter().any(|s| s.contains("does the picking")),
                    "{snapshots:?}"
                );
                // `screencapture`'s alone, and a permission no desktop asks Trek for.
                for gone in ["Window shadow", "Shutter sound", "Permission"] {
                    assert!(
                        !snapshots.iter().any(|s| s == gone),
                        "{gone}: {snapshots:?}"
                    );
                }
            }
            {
                // A bare X11 session: the screen is all there is, and the page says why.
                let (_said, _desktop) = desktop(false, true);
                let snapshots = said_on(&trek, cx, SettingsPage::Snapshots);
                assert!(
                    snapshots
                        .iter()
                        .any(|s| s.contains("takes the whole screen")
                            && s.contains("needs the desktop's portal")),
                    "{snapshots:?}"
                );
                assert!(
                    !snapshots.iter().any(|s| s.contains("offers both")),
                    "nothing to choose between: {snapshots:?}"
                );
                for row in ["Hide Trek while capturing", "Format", "Keep snapshots"] {
                    assert!(snapshots.iter().any(|s| s == row), "{row}: {snapshots:?}");
                }
            }
            // A session that can take none has no page for them.
            let (_said, _desktop) = desktop(false, false);
            assert_eq!(Composer::snapshot_tool(), Tool::Desktop(Session::default()));
            assert!(!Composer::snapshots_offered());
            assert!(trek_app::settings_view::pages().all(|p| p != SettingsPage::Snapshots));
        });
    }

    #[test]
    fn a_closed_picker_attaches_nothing_and_says_nothing() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(true, true);
            said.lock().unwrap().portal = Some(Asked::Cancelled);
            let seen = toasts(&trek, cx);
            composer(&trek, cx).update(cx, |c, cx| c.snapshot_by_desktop(Mode::Pick, cx));
            settle(cx);
            assert_eq!(said.lock().unwrap().asked, ["pick"]);
            assert!(attached(&trek, cx).is_empty());
            assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
            assert_eq!(stepped(&said), (1, 1), "Trek is back all the same");
            // And it can be asked again.
            portal_saved(&said, [4, 5, 6]);
            composer(&trek, cx).update(cx, |c, cx| c.snapshot_by_desktop(Mode::Pick, cx));
            until_attached(&trek, cx, 1);
        });
    }

    #[test]
    fn a_portal_that_fails_is_said() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(true, false);
            said.lock().unwrap().portal = Some(Asked::Failed(
                "Portal request didn't succeed with no information".into(),
            ));
            let seen = toasts(&trek, cx);
            composer(&trek, cx).update(cx, |c, cx| c.snapshot_by_desktop(Mode::Pick, cx));
            cx.executor().advance_clock(SETTLE);
            until(cx, "the failure to be said", |_| !seen.borrow().is_empty());
            assert_eq!(
                *seen.borrow(),
                [
                    "Couldn't attach the snapshot: the desktop's screenshot portal said \"Portal request didn't succeed with no information\"."
                ]
            );
            assert!(attached(&trek, cx).is_empty());
        });
    }

    /// The shortcut is bound on every platform; where nothing can take a snapshot it used to do
    /// nothing at all, without a word.
    #[test]
    fn the_shortcut_says_why_where_nothing_can_take_a_snapshot() {
        run(async |cx| {
            let trek = open(cx);
            let (said, _desktop) = desktop(false, false);
            let seen = toasts(&trek, cx);
            trek.press(cx, "secondary-shift-s");
            settle(cx);
            assert_eq!(*seen.borrow(), [NOTHING_TO_CAPTURE_WITH]);
            assert!(said.lock().unwrap().asked.is_empty());
            assert!(attached(&trek, cx).is_empty());
        });
    }
}
