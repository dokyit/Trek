//! App Snapshots through Snipping Tool (Windows): the composer opens the overlay, waits for the
//! image it leaves on the clipboard and attaches it as a paste would. A fake overlay and clipboard
//! stand in for Windows', so these run on every platform, on a mock clock.

use super::harness::{Trek, open, run};
use crate::screenclip::{Backend, Clip, Read, Source};
use crate::workspace::WorkspaceEvent;
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
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
        self.0.lock().unwrap().image.clone().map_or(Read::NotAnImage, Read::Image)
    }
}

/// The overlay and clipboard Trek uses from now on.
fn install(cx: &mut TestAppContext, timeout: Duration) -> Arc<Mutex<Pad>> {
    let pad = Arc::new(Mutex::new(Pad::default()));
    let fake = Arc::new(Fake { pad: pad.clone(), timeout });
    cx.update(|cx| crate::screenclip::install(fake, cx));
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
            if let WorkspaceEvent::Toast { message, .. } = event {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    seen
}

fn composer(trek: &Trek, cx: &TestAppContext) -> gpui_kit::Entity<crate::composer::Composer> {
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
        assert!(trek.visible(cx, "snapshot-wait"), "the wait is said in the strip");
        pass(cx, 5);
        assert!(attached(&trek, cx).is_empty(), "nothing was picked yet");
        assert!(trek.visible(cx, "snapshot-wait"));

        snip(&pad, [10, 120, 250]);
        until(cx, "the snip to attach", |cx| !attached(&trek, cx).is_empty());
        trek.render(cx);
        assert!(!trek.visible(cx, "snapshot-wait"), "the wait is over");
        let path = attached(&trek, cx)[0].clone();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(path.starts_with(trek_core::paths::data_dir().join("snapshots")), "saved with the others: {}", path.display());
        let picture = image::open(&path).expect("a PNG").to_rgb8();
        assert_eq!((picture.width(), picture.height(), picture.get_pixel(0, 0).0), (1, 1, [10, 120, 250]));

        // It goes out with the next message, as a pasted one does.
        trek.press(cx, "enter");
        let id = trek.thread_id(cx);
        let images = trek.items(cx, &id).into_iter().find_map(|i| if let Item::User { images, .. } = i { Some(images) } else { None }).expect("sent");
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
        assert_eq!(pad.lock().unwrap().looks, looks, "the clipboard isn't looked at any more");
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
        assert!(said[0].starts_with("Nothing was picked in a minute"), "{said:?}");
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
        assert_eq!(*seen.borrow(), ["Couldn't open the snipping tool: Windows answered 31."]);
        assert!(!trek.visible(cx, "snapshot-wait"));
        assert!(!composer(&trek, cx).read_with(cx, |c, _| c.snapshot_waiting()), "nothing is waiting");
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
        assert_eq!(pad.lock().unwrap().looks, after, "no one is looking at the clipboard once the window is gone");
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
        until(cx, "the snip to attach", |cx| !attached(&trek, cx).is_empty());
    });
}
