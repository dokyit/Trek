//! iPhone on a PC: the user's real iPhone beside the chat, its screen, apps and files, for them
//! and for agents. Windows reaches the phone's own services through Apple Mobile Device Service,
//! as Apple Devices does (`trek_phone::device`), and its screen and taps through CoreDevice's
//! services once its developer disk image is mounted (`trek_phone::coredevice`): the picture area
//! asks for the image's folder the first time, then shows the screen, a click on it a tap. Every
//! state's words are `trek_phone::pc`'s, which `phone_status` answers with too.
//!
//! While it's on screen, or an agent holds the phone, the tool looks for the phone every few
//! seconds (`device::probe`) and says the one thing to do until it's ready: install Apple Devices
//! (the service comes with it), plug the iPhone in, tap Trust, unlock it. Ready, it shows the
//! phone and its AssistiveTouch switch, its apps (and installs a .ipa), and its files: the media
//! folder or a file-sharing app's Documents, to copy into, save out of and remove from.
//!
//! Agents' `trek-mcp phone` calls come here one at a time (`RightPanel::phone_call`), as on a
//! Mac: one thread holds the phone at a time (`trek_phone::lease`), a strip says who with Stop to
//! take it back, and installing and changing files ask first by the thread's access level on its
//! approval card (`Workspace::browser_ask`). The Mac's iPhone tool (`panels/phone.rs`) has the
//! same rules and words for those; the tests here pin them. The tools a PC doesn't serve (the
//! screen and taps) are answered with where they are (`trek_ipc::phone::on_a_mac`).

use crate::assets::Lucide;
use crate::fluent;
use crate::scale::{radius, text, to_rems};
use crate::workspace::Workspace;
use crate::workspace::phone_use::PhoneCall;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_agents::Decision;
use trek_core::HandHolding;
use trek_ipc::phone::{self as tools, Sensitive};
use trek_phone::coredevice::{self, Gesture, Shot};
use trek_phone::device::{self, App as PhoneApp, Phone, Seen};
use trek_phone::files::{self, Entry};
use trek_phone::lease::{Lease, TOOK_OVER};
use trek_phone::pc;

const TICK: Duration = Duration::from_secs(1);
/// How often usbmuxd is asked again while the tool is wanted: a phone plugged in shows within it.
const LOOK_AGAIN: Duration = Duration::from_secs(3);
/// How long the strip stays after an agent's last call.
const STRIP_LINGER: Duration = Duration::from_secs(12);
/// How long an approval card waits for the user.
const ASK_WAIT: Duration = Duration::from_secs(15 * 60);

/// What needs the user's yes, by the thread's access level: installing an app unless the thread
/// has full access; writing or removing files unless it's on Auto or full access. (As on a Mac.)
pub fn needs_asking(kind: Sensitive, access: HandHolding) -> bool {
    match access {
        HandHolding::FullAccess => false,
        HandHolding::Auto => kind == Sensitive::Install,
        HandHolding::Supervised | HandHolding::AutoAcceptEdits => true,
    }
}

/// Where the tool's facts come from: the phone's own services, or a made-up phone (an isolated
/// capture run, and the tests), for which nothing real is touched. Every call blocks.
pub trait Source: Send + Sync {
    fn probe(&self) -> Seen;
    fn apps(&self, udid: &str) -> Result<Vec<PhoneApp>, String>;
    fn assistive(&self, udid: &str) -> Result<bool, String>;
    fn set_assistive(&self, udid: &str, on: bool) -> Result<(), String>;
    fn install(&self, udid: &str, package: &Path) -> Result<(), String>;
    fn list(&self, udid: &str, path: &str, app: Option<&str>) -> Result<Vec<Entry>, String>;
    fn push(
        &self,
        udid: &str,
        local: &Path,
        remote: &str,
        app: Option<&str>,
    ) -> Result<u64, String>;
    fn pull(
        &self,
        udid: &str,
        remote: &str,
        local: &Path,
        app: Option<&str>,
    ) -> Result<u64, String>;
    fn remove(&self, udid: &str, path: &str, app: Option<&str>) -> Result<(), String>;
    /// Whether the phone's developer disk image is mounted: its screen and taps need it.
    fn disk_image(&self, udid: &str) -> Result<bool, String>;
    /// Mount the developer disk image in `folder` (Xcode's, signed for the phone by Apple).
    fn mount_disk_image(&self, udid: &str, folder: &Path) -> Result<(), String>;
    /// The phone's screen as it is now.
    fn screenshot(&self, udid: &str) -> Result<Shot, String>;
    /// What's on the screen, as the phone's accessibility reads it (Xcode's Accessibility
    /// Inspector's way: `trek_phone::ax`).
    fn elements(&self, udid: &str) -> Result<trek_phone::ax::Screen, String>;
    /// A touch on the phone: a tap, a swipe, typing, or the Home Screen.
    fn touch(&self, udid: &str, gesture: Gesture) -> Result<(), String>;
    /// A tap on the screen at (`x`, `y`), fractions of its width and height.
    fn tap(&self, udid: &str, x: f32, y: f32) -> Result<(), String> {
        self.touch(udid, Gesture::Tap(x, y))
    }
    /// Open the app named `name` (as on the Home Screen, any case); its name as installed.
    fn open_app(&self, udid: &str, name: &str) -> Result<String, String>;
}

/// The phone's own services over usbmuxd: Apple Mobile Device Service.
struct Services;

impl Source for Services {
    fn probe(&self) -> Seen {
        device::probe()
    }
    fn apps(&self, udid: &str) -> Result<Vec<PhoneApp>, String> {
        device::apps(Some(udid))
    }
    fn assistive(&self, udid: &str) -> Result<bool, String> {
        device::assistive_touch(Some(udid))
    }
    fn set_assistive(&self, udid: &str, on: bool) -> Result<(), String> {
        device::set_assistive_touch(Some(udid), on)
    }
    fn install(&self, udid: &str, package: &Path) -> Result<(), String> {
        device::install(Some(udid), package)
    }
    fn list(&self, udid: &str, path: &str, app: Option<&str>) -> Result<Vec<Entry>, String> {
        device::list_dir(Some(udid), path, app)
    }
    fn push(
        &self,
        udid: &str,
        local: &Path,
        remote: &str,
        app: Option<&str>,
    ) -> Result<u64, String> {
        device::push(Some(udid), local, remote, app)
    }
    fn pull(
        &self,
        udid: &str,
        remote: &str,
        local: &Path,
        app: Option<&str>,
    ) -> Result<u64, String> {
        device::pull(Some(udid), remote, local, app)
    }
    fn remove(&self, udid: &str, path: &str, app: Option<&str>) -> Result<(), String> {
        device::remove(Some(udid), path, app)
    }
    fn disk_image(&self, udid: &str) -> Result<bool, String> {
        coredevice::mounted(udid)
    }
    fn mount_disk_image(&self, udid: &str, folder: &Path) -> Result<(), String> {
        coredevice::mount(udid, folder)
    }
    fn screenshot(&self, udid: &str) -> Result<Shot, String> {
        coredevice::screenshot(udid)
    }
    fn elements(&self, udid: &str) -> Result<trek_phone::ax::Screen, String> {
        coredevice::elements(udid)
    }
    fn touch(&self, udid: &str, gesture: Gesture) -> Result<(), String> {
        coredevice::touch_now(udid, gesture)
    }
    fn open_app(&self, udid: &str, name: &str) -> Result<String, String> {
        coredevice::open_app(udid, name)
    }
}

/// A made-up phone, all in memory: what usbmuxd shows, its apps, its files and its
/// AssistiveTouch. What was done to it is kept in `done`, in order.
#[cfg(any(test, feature = "test-support", feature = "shots"))]
#[derive(Default)]
pub struct MadeUp {
    pub seen: std::sync::Mutex<Option<Seen>>,
    pub apps: std::sync::Mutex<Vec<PhoneApp>>,
    /// (app's bundle id, or "" for the media folder; path; size, or `None` for a folder).
    pub files: std::sync::Mutex<Vec<(String, String, Option<u64>)>>,
    pub assistive: std::sync::Mutex<bool>,
    pub done: std::sync::Mutex<Vec<String>>,
    /// Its developer disk image is mounted: its screen shows, and takes taps.
    pub disk_image: std::sync::Mutex<bool>,
}

#[cfg(any(test, feature = "test-support", feature = "shots"))]
impl MadeUp {
    /// The phone it is.
    pub fn phone() -> Phone {
        Phone {
            udid: "00008140-MADEUP".into(),
            name: "Toby's iPhone".into(),
            ios: "27.2".into(),
            model: "iPhone17,2".into(),
            usb: true,
        }
    }

    /// Paired and answering, with a few apps and files.
    pub fn ready() -> Arc<MadeUp> {
        let app = |bundle: &str, name: &str, version: &str, file_sharing| PhoneApp {
            bundle: bundle.into(),
            name: name.into(),
            version: version.into(),
            build: "1".into(),
            file_sharing,
        };
        let made = MadeUp::default();
        *made.seen.lock().unwrap() = Some(Seen::Ready(Self::phone()));
        *made.apps.lock().unwrap() = vec![
            app("com.example.notes", "Field Notes", "2.4", true),
            app("dev.trek.Trek", "Trek", "0.4.49", false),
            app("com.example.weather", "Weather Lab", "1.0", false),
        ];
        let place = |app: &str, path: &str, size: Option<u64>| (app.into(), path.into(), size);
        *made.files.lock().unwrap() = vec![
            place("", "/Downloads", None),
            place("", "/Downloads/report.pdf", Some(1_400_000)),
            place("", "/DCIM", None),
            place("", "/DCIM/100APPLE", None),
            place("", "/DCIM/100APPLE/IMG_0001.HEIC", Some(2_100_000)),
            place("com.example.notes", "/Documents", None),
            place("com.example.notes", "/Documents/todo.txt", Some(320)),
        ];
        *made.disk_image.lock().unwrap() = true;
        Arc::new(made)
    }

    /// Ready, but its developer disk image not mounted yet: the screen asks for its folder.
    pub fn without_disk_image() -> Arc<MadeUp> {
        let made = Self::ready();
        *made.disk_image.lock().unwrap() = false;
        made
    }

    /// The taps it took, as fractions of its screen ("tap 0.50 0.25"), in order.
    pub fn taps(&self) -> Vec<String> {
        let done = self.done.lock().unwrap();
        done.iter()
            .filter(|d| d.starts_with("tap "))
            .cloned()
            .collect()
    }

    /// Its screen: a home screen of a few tiles, drawn here, as a PNG the size of a phone's in
    /// points.
    fn screen_png() -> Vec<u8> {
        let (w, h) = (390u32, 844u32);
        let picture = image::RgbaImage::from_fn(w, h, |x, y| {
            let tile = (x % 90 >= 22 && x % 90 < 82) && (y % 96 >= 70 && y % 96 < 130) && y < 560;
            if tile {
                let hue = ((x / 90) * 60 + (y / 96) * 40) as u8;
                image::Rgba([hue, 140, 255 - hue, 255])
            } else {
                let shade = 20 + (y * 40 / h) as u8;
                image::Rgba([shade, shade, shade + 30, 255])
            }
        });
        let mut png = std::io::Cursor::new(Vec::new());
        let _ = picture.write_to(&mut png, image::ImageFormat::Png);
        png.into_inner()
    }

    /// In `seen`'s state instead (no phone, Trust, …).
    pub fn showing(seen: Seen) -> Arc<MadeUp> {
        let made = MadeUp::default();
        *made.seen.lock().unwrap() = Some(seen);
        Arc::new(made)
    }

    fn note(&self, what: String) {
        self.done.lock().unwrap().push(what);
    }

    fn key(app: Option<&str>) -> String {
        app.unwrap_or_default().to_string()
    }
}

#[cfg(any(test, feature = "test-support", feature = "shots"))]
impl Source for MadeUp {
    fn probe(&self) -> Seen {
        self.seen.lock().unwrap().clone().unwrap_or(Seen::NoPhone)
    }
    fn apps(&self, _: &str) -> Result<Vec<PhoneApp>, String> {
        Ok(self.apps.lock().unwrap().clone())
    }
    fn assistive(&self, _: &str) -> Result<bool, String> {
        Ok(*self.assistive.lock().unwrap())
    }
    fn set_assistive(&self, _: &str, on: bool) -> Result<(), String> {
        *self.assistive.lock().unwrap() = on;
        self.note(format!("assistive {on}"));
        Ok(())
    }
    fn install(&self, _: &str, package: &Path) -> Result<(), String> {
        self.note(format!("install {}", package.display()));
        Ok(())
    }
    fn list(&self, _: &str, path: &str, app: Option<&str>) -> Result<Vec<Entry>, String> {
        let (key, dir) = (Self::key(app), files::normalize(path));
        let mut out: Vec<Entry> = self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|(a, p, _)| {
                *a == key
                    && p.rsplit_once('/').map(
                        |(parent, _)| {
                            if parent.is_empty() { "/" } else { parent }
                        },
                    ) == Some(dir.as_str())
            })
            .map(|(_, p, size)| Entry {
                name: p.rsplit('/').next().unwrap_or_default().to_string(),
                dir: size.is_none(),
                size: size.unwrap_or(0),
            })
            .collect();
        files::sort(&mut out);
        Ok(out)
    }
    fn push(&self, _: &str, local: &Path, remote: &str, app: Option<&str>) -> Result<u64, String> {
        let remote = files::check_remote(remote, app.is_some())?;
        let size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        self.files
            .lock()
            .unwrap()
            .push((Self::key(app), remote.clone(), Some(size)));
        self.note(format!("push {} {remote}", local.display()));
        Ok(size)
    }
    fn pull(&self, _: &str, remote: &str, local: &Path, _: Option<&str>) -> Result<u64, String> {
        self.note(format!("pull {remote} {}", local.display()));
        Ok(0)
    }
    fn remove(&self, _: &str, path: &str, app: Option<&str>) -> Result<(), String> {
        let path = files::check_remote(path, app.is_some())?;
        let key = Self::key(app);
        self.files
            .lock()
            .unwrap()
            .retain(|(a, p, _)| !(*a == key && *p == path));
        self.note(format!("remove {path}"));
        Ok(())
    }
    fn disk_image(&self, _: &str) -> Result<bool, String> {
        Ok(*self.disk_image.lock().unwrap())
    }
    fn mount_disk_image(&self, _: &str, folder: &Path) -> Result<(), String> {
        coredevice::disk_image_in(folder)?;
        *self.disk_image.lock().unwrap() = true;
        self.note(format!("mount {}", folder.display()));
        Ok(())
    }
    fn screenshot(&self, _: &str) -> Result<Shot, String> {
        if !*self.disk_image.lock().unwrap() {
            return Err("Its developer disk image isn't mounted.".into());
        }
        let png = Self::screen_png();
        let (width, height) = coredevice::png_size(&png).ok_or("No picture.")?;
        Ok(Shot { png, width, height })
    }
    fn touch(&self, _: &str, gesture: Gesture) -> Result<(), String> {
        if !*self.disk_image.lock().unwrap() {
            return Err("Its developer disk image isn't mounted.".into());
        }
        let done = match gesture {
            Gesture::Tap(x, y) => format!("tap {x:.2} {y:.2}"),
            Gesture::Swipe { from, to, ms } => format!(
                "swipe {:.2} {:.2} {:.2} {:.2} {ms}",
                from.0, from.1, to.0, to.1
            ),
            Gesture::Type(text) => {
                coredevice::keys_for(&text)?;
                format!("type {text:?}")
            }
            Gesture::Home => "home".into(),
            Gesture::Key { held, key } => format!("key {held:?} {key:#04x}"),
        };
        self.note(done);
        Ok(())
    }
    /// A Settings screen: the app, a row to tap and a switch, in a phone's points.
    fn elements(&self, _: &str) -> Result<trek_phone::ax::Screen, String> {
        use trek_phone::ax::{Element, Frame, Screen};
        let at = |role: &str, label: &str, depth: usize, (x, y, w, h)| Element {
            role: role.into(),
            label: label.into(),
            enabled: true,
            depth,
            frame: Some(Frame { x, y, w, h }),
            ..Element::default()
        };
        self.note("elements".into());
        Ok(Screen {
            size: Some((390., 844.)),
            elements: vec![
                at("application", "Settings", 0, (0., 0., 390., 844.)),
                at("button", "General", 1, (16., 200., 358., 44.)),
                Element {
                    value: Some("0".into()),
                    ..at("switch", "Airplane Mode", 1, (16., 260., 358., 44.))
                },
            ],
        })
    }
    fn open_app(&self, _: &str, name: &str) -> Result<String, String> {
        if !*self.disk_image.lock().unwrap() {
            return Err("Its developer disk image isn't mounted.".into());
        }
        let apps = [
            ("Settings", "com.apple.Preferences"),
            ("Calculator", "com.apple.calculator"),
            ("Notes", "com.apple.mobilenotes"),
        ];
        let (found, _) = coredevice::pick_app(&apps, name)?;
        self.note(format!("open {found}"));
        Ok(found.to_string())
    }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// The phone the tests' iPhone tools see (each GPUI test runs on a thread of its own).
    static PRETEND: std::cell::RefCell<Option<Arc<dyn Source>>> = const { std::cell::RefCell::new(None) };
}

/// Until the guard drops, iPhone tools made on this thread see `source` instead of the real
/// phone.
#[cfg(any(test, feature = "test-support"))]
pub fn pretend(source: Arc<dyn Source>) -> impl Drop {
    struct Restore(Option<Arc<dyn Source>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let before = self.0.take();
            PRETEND.with(|p| *p.borrow_mut() = before);
        }
    }
    Restore(PRETEND.with(|p| p.borrow_mut().replace(source)))
}

/// The real phone, or (in the tests, or an isolated capture run) a made-up one.
fn source() -> Arc<dyn Source> {
    // The tests never reach for a real phone: the one they pretend, or none.
    #[cfg(any(test, feature = "test-support"))]
    let tests = Some(PRETEND.with(|p| p.borrow().clone()).unwrap_or_else(|| {
        let none: Arc<dyn Source> = MadeUp::showing(Seen::NoPhone);
        none
    }));
    #[cfg(not(any(test, feature = "test-support")))]
    let tests: Option<Arc<dyn Source>> = None;
    if let Some(made_up) = tests {
        return made_up;
    }
    // An isolated capture run never reaches for the user's real phone, unless it's asked to
    // (`TREK_SHOT_PHONE=real`: checking the tool with a phone on the cable, everything else of
    // the run still isolated).
    #[cfg(feature = "shots")]
    if std::env::var_os("TREK_SHOT_DIR").is_some()
        && std::env::var("TREK_SHOT_PHONE").as_deref() != Ok("real")
    {
        return MadeUp::ready();
    }
    Arc::new(Services)
}

/// How often the screen's picture is taken again while it's on show (it's a still each time:
/// CoreDevice's screenshot service, over the cable).
const SCREEN_EVERY: Duration = Duration::from_millis(900);
/// How often while the screen stays still (`STILL_AFTER` pictures the same in a row): the phone
/// spends about a second on each picture, and a still one needs no looking at so often.
const SCREEN_STILL: Duration = Duration::from_secs(3);
const STILL_AFTER: u32 = 3;
/// An agent's picture's long side, unless it asks for another (`max_edge`), as on a Mac.
const SHOT_EDGE: u32 = 1200;
/// The screen's picture is drawn from a copy no bigger than this on its long side: the tab shows
/// it whole, and the phone's own picture (2868 pixels tall on a Pro Max) is 15 MB decoded.
const SCREEN_DRAWN: u32 = 1600;

/// The screen's picture as GPUI draws it (BGRA), scaled down to `SCREEN_DRAWN`. Decoded here,
/// off the UI thread, and let go of when the next comes: GPUI's own `img` of the PNG would
/// decode it on the way to the screen and keep every one for the run, a new one each second.
fn screen_picture(png: &[u8]) -> Result<Arc<RenderImage>, String> {
    let picture = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| format!("The screen's picture didn't read: {e}"))?;
    let picture = if picture.width().max(picture.height()) > SCREEN_DRAWN {
        picture.thumbnail(SCREEN_DRAWN, SCREEN_DRAWN)
    } else {
        picture
    };
    let mut pixels = picture.into_rgba8();
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// Where the tool notes the folder it last installed the developer disk image from: the phone
/// loses the image when it restarts, and the tool installs it from there again.
fn disk_image_note() -> PathBuf {
    trek_core::paths::data_dir().join("phone-disk-image")
}

/// The folder the developer disk image was last installed from, while it still holds one.
fn remembered_disk_image() -> Option<PathBuf> {
    let folder = PathBuf::from(std::fs::read_to_string(disk_image_note()).ok()?.trim());
    coredevice::disk_image_in(&folder).is_ok().then_some(folder)
}

/// The phone's screen in the picture area (CoreDevice's: `trek_phone::coredevice`).
#[derive(Clone)]
enum Screen {
    /// Not asked about yet (a phone newly ready).
    Unknown,
    /// Asked whether its developer disk image is mounted, or for the first picture.
    Looking,
    /// Its developer disk image isn't mounted: the user chooses its folder.
    NeedsImage,
    /// Mounting it.
    Mounting,
    /// On show: the latest picture (a copy no bigger than it's drawn), and the screen's size in
    /// pixels.
    Showing(Arc<RenderImage>, (u32, u32)),
    /// It couldn't be shown: why.
    Failed(SharedString),
}

/// What the strip shows: who's at it and what they did last.
struct Activity {
    title: String,
    action: String,
    at: Instant,
}

/// The tool's two lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Apps,
    Files,
    /// The phone's screen (`Screen`), a click on it a tap.
    Screen,
}

/// A folder the Files list shows: in the media folder, or in an app's Documents.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    app: Option<String>,
    path: String,
}

impl Place {
    fn media() -> Self {
        Place {
            app: None,
            path: "/".into(),
        }
    }

    fn documents(bundle: String) -> Self {
        Place {
            app: Some(bundle),
            path: "/Documents".into(),
        }
    }

    /// Its top: the media folder's root, or the app's Documents.
    fn top(&self) -> &'static str {
        if self.app.is_some() {
            "/Documents"
        } else {
            "/"
        }
    }
}

pub struct PhonePanel {
    workspace: Entity<Workspace>,
    source: Arc<dyn Source>,
    /// Bumped when the source changes (design review's made-up phones): answers that were on
    /// their way from the one before are dropped.
    generation: u64,
    visible: bool,
    /// What usbmuxd showed last (`None` before the first look).
    seen: Option<Seen>,
    looking: bool,
    looked: Option<Instant>,
    /// The phone's apps, with the phone they were read from.
    apps: Option<(String, Result<Vec<PhoneApp>, String>)>,
    apps_loading: bool,
    assistive: Option<bool>,
    assistive_setting: bool,
    tab: Tab,
    place: Place,
    /// The folder last listed, and what was in it.
    entries: Option<(Place, Result<Vec<Entry>, String>)>,
    entries_loading: bool,
    /// The file the user asked to remove, waiting for the second click that confirms it.
    removing: Option<String>,
    /// What the tool is doing for the user now ("Installing Notes.ipa…").
    working: Option<String>,
    notice: Option<SharedString>,
    lease: Lease,
    queue: VecDeque<PhoneCall>,
    running: bool,
    /// Bumped by Stop: a call under way checks it between steps.
    stops: u64,
    last: Option<Activity>,
    allowed: HashSet<(String, Sensitive)>,
    /// The phone's screen in the picture area, and the phone it's of.
    screen: Screen,
    screen_of: Option<String>,
    /// A picture on its way (one at a time), and when the last one came.
    shooting: bool,
    shot_at: Option<Instant>,
    /// The last picture's fingerprint (one the same isn't decoded again), and how many in a row
    /// came the same.
    last_frame: Option<u64>,
    still: u32,
    /// Pictures of the screen replaced since the last one came, let go of when the next does (a
    /// window may draw one until then).
    stale: Vec<Arc<RenderImage>>,
    /// Where a press on the screen's picture started (fractions of it), and when: let go, it's a
    /// tap or a swipe.
    pressed: Option<((f32, f32), Instant)>,
    /// What the user types to the iPhone: Enter sends it (Shift+Enter presses Return after).
    typing: Entity<InputState>,
    _typed: Subscription,
    /// The size of the last picture an agent was given, which its x/y are read in.
    agent_shot: Option<(u32, u32)>,
    /// Agents' pictures written so far (each its own file, which trek-mcp reads and removes).
    shots: u64,
    /// The made-up phone design review put in (`demo`), to read what it was asked.
    #[cfg(feature = "shots")]
    demo_phone: Option<Arc<MadeUp>>,
    _tasks: Vec<Task<()>>,
}

impl PhonePanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let typing = cx.new(|cx| InputState::new(window, cx).placeholder(pc::SCREEN_TYPE));
        let typed = cx.subscribe_in(
            &typing,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    let text = input.read(cx).value().to_string();
                    if text.is_empty() && !*shift {
                        return;
                    }
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                    let text = if *shift { format!("{text}\n") } else { text };
                    this.touch_screen(Gesture::Type(text), cx);
                }
            },
        );
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            workspace,
            source: source(),
            generation: 0,
            visible: false,
            seen: None,
            looking: false,
            looked: None,
            apps: None,
            apps_loading: false,
            assistive: None,
            assistive_setting: false,
            tab: Tab::Apps,
            place: Place::media(),
            entries: None,
            entries_loading: false,
            removing: None,
            working: None,
            notice: None,
            lease: Lease::default(),
            queue: VecDeque::new(),
            running: false,
            stops: 0,
            last: None,
            allowed: HashSet::new(),
            screen: Screen::Unknown,
            screen_of: None,
            shooting: false,
            shot_at: None,
            last_frame: None,
            still: 0,
            stale: Vec::new(),
            pressed: None,
            typing,
            _typed: typed,
            agent_shot: None,
            shots: 0,
            #[cfg(feature = "shots")]
            demo_phone: None,
            _tasks: vec![tick],
        }
    }

    /// Shown or hidden by the right panel: it looks for the phone at once when it shows.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.look(cx);
        }
        cx.notify();
    }

    /// The tab is closing: calls waiting fail.
    pub fn release(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.visible = false;
        for call in self.queue.drain(..) {
            call.answer(Err("The user closed Trek's iPhone tool.".into()));
        }
    }

    fn busy(&self) -> bool {
        self.running || !self.queue.is_empty()
    }

    /// Whether the phone is wanted: on screen, or an agent's.
    fn wanted(&self) -> bool {
        self.visible || self.busy() || self.lease.holder(Instant::now()).is_some()
    }

    /// What usbmuxd showed last, as the tool shows it (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn seen(&self) -> Option<&Seen> {
        self.seen.as_ref()
    }

    /// The line the tool says over its lists, if any (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn notice(&self) -> Option<String> {
        self.notice.as_ref().map(|n| n.to_string())
    }

    /// The folder the Files list shows, and the app whose Documents it's in (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn files_at(&self) -> (Option<String>, String) {
        (self.place.app.clone(), self.place.path.clone())
    }

    /// The phone, once it's ready.
    fn phone(&self) -> Option<&Phone> {
        match &self.seen {
            Some(Seen::Ready(phone)) => Some(phone),
            _ => None,
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.wanted() && self.looked.is_none_or(|t| t.elapsed() >= LOOK_AGAIN) {
            self.look(cx);
        }
        // The screen's picture, again, while it's on show.
        if self.visible
            && self.tab == Tab::Screen
            && matches!(self.screen, Screen::Showing(..))
            && self.shot_at.is_none_or(|t| {
                t.elapsed()
                    >= if self.still >= STILL_AFTER {
                        SCREEN_STILL
                    } else {
                        SCREEN_EVERY
                    }
            })
        {
            self.shoot(cx);
        }
        // The strip goes a while after the last call.
        if self.last.as_ref().is_some_and(|a| {
            !self.busy() && a.at.elapsed() >= STRIP_LINGER && a.at.elapsed() < STRIP_LINGER + TICK
        }) {
            cx.notify();
        }
    }

    /// Run blocking work off the UI thread, then apply its result.
    fn background<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    /// Ask usbmuxd where the phone stands, and its AssistiveTouch once it's ready.
    fn look(&mut self, cx: &mut Context<Self>) {
        if self.looking {
            return;
        }
        self.looking = true;
        self.looked = Some(Instant::now());
        let (source, generation) = (self.source.clone(), self.generation);
        self.background(
            cx,
            move || {
                let seen = source.probe();
                let assistive = match &seen {
                    Seen::Ready(phone) => source.assistive(&phone.udid).ok(),
                    _ => None,
                };
                (seen, assistive)
            },
            move |this, (seen, assistive), cx| {
                if this.generation != generation {
                    return;
                }
                this.looking = false;
                this.looked = Some(Instant::now());
                this.saw(seen, assistive, cx);
            },
        );
    }

    /// What usbmuxd showed: a phone newly ready has its apps read (and its files, on that tab);
    /// one gone takes its lists with it.
    fn saw(&mut self, seen: Seen, assistive: Option<bool>, cx: &mut Context<Self>) {
        let udid = match &seen {
            Seen::Ready(phone) => Some(phone.udid.clone()),
            _ => None,
        };
        let changed = self.phone().map(|p| p.udid.clone()) != udid;
        if self.seen.as_ref() != Some(&seen) {
            self.seen = Some(seen);
            cx.notify();
        }
        if !self.assistive_setting && self.assistive != assistive {
            self.assistive = assistive;
            cx.notify();
        }
        if udid.is_none() {
            self.apps = None;
            self.entries = None;
            self.removing = None;
            self.set_screen(Screen::Unknown);
            self.screen_of = None;
            return;
        }
        if self.screen_of != udid {
            self.set_screen(Screen::Unknown);
            self.screen_of = udid.clone();
            self.agent_shot = None;
        }
        if matches!(self.screen, Screen::Unknown) {
            self.ask_screen(cx);
        }
        if changed || self.apps.as_ref().map(|(u, _)| u) != udid.as_ref() {
            self.place = Place::media();
            self.entries = None;
            self.load_apps(cx);
        }
        if self.tab == Tab::Files && self.entries.is_none() {
            self.load_entries(cx);
        }
    }

    /// Look again now (the header's button, or after the user acted).
    fn look_now(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        self.look(cx);
    }

    // MARK: screen

    /// Put the screen in `state`. A picture it showed waits in `stale` to be let go of.
    fn set_screen(&mut self, state: Screen) {
        if let Screen::Showing(picture, _) = std::mem::replace(&mut self.screen, state) {
            self.stale.push(picture);
        }
        // The next picture is decoded, whatever the last was.
        if !matches!(self.screen, Screen::Showing(..)) {
            self.last_frame = None;
        }
    }

    /// Whether the phone's developer disk image is mounted: then its picture. Else it's installed
    /// again from the folder it last came from (the phone loses it when it restarts), or its
    /// folder is asked for.
    fn ask_screen(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        self.set_screen(Screen::Looking);
        cx.notify();
        let (source, generation) = (self.source.clone(), self.generation);
        self.background(
            cx,
            move || {
                let mounted = source.disk_image(&udid)?;
                Ok::<_, String>((mounted, (!mounted).then(remembered_disk_image).flatten()))
            },
            move |this, mounted, cx| {
                if this.generation != generation {
                    return;
                }
                match mounted {
                    Ok((true, _)) => this.shoot(cx),
                    Ok((false, Some(folder))) => this.mount_disk_image(folder, cx),
                    Ok((false, None)) => this.set_screen(Screen::NeedsImage),
                    Err(why) => this.set_screen(Screen::Failed(why.into())),
                }
                cx.notify();
            },
        );
    }

    /// Take the screen's picture (one at a time).
    fn shoot(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        if self.shooting {
            return;
        }
        self.shooting = true;
        let (source, generation, last) = (self.source.clone(), self.generation, self.last_frame);
        self.background(
            cx,
            move || {
                let shot = source.screenshot(&udid)?;
                let print = fingerprint(&shot.png);
                // The same as the one on show: nothing to decode.
                if last == Some(print) {
                    return Ok::<_, String>((print, None));
                }
                Ok((
                    print,
                    Some((screen_picture(&shot.png)?, (shot.width, shot.height))),
                ))
            },
            move |this, shot, cx| {
                if this.generation != generation {
                    return;
                }
                this.shooting = false;
                this.shot_at = Some(Instant::now());
                match shot {
                    Ok((_, None)) => this.still += 1,
                    Ok((print, Some((picture, size)))) => {
                        // The ones replaced before the last: no window draws them any more.
                        for old in this.stale.drain(..) {
                            cx.drop_image(old, None);
                        }
                        this.set_screen(Screen::Showing(picture, size));
                        this.last_frame = Some(print);
                        this.still = 0;
                    }
                    // A picture on show stays, with why the next didn't come.
                    Err(why) if matches!(this.screen, Screen::Showing(..)) => {
                        this.notice = Some(why.into())
                    }
                    Err(why) => this.set_screen(Screen::Failed(why.into())),
                }
                cx.notify();
            },
        );
    }

    /// Ask for the developer disk image's folder, then mount it.
    fn choose_disk_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.mount_disk_image(folder, cx));
        })
        .detach();
    }

    /// Mount the developer disk image in `folder`, then show the screen; the folder is noted,
    /// for when the phone loses the image.
    fn mount_disk_image(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        self.set_screen(Screen::Mounting);
        cx.notify();
        let (source, generation) = (self.source.clone(), self.generation);
        self.background(
            cx,
            move || {
                source.mount_disk_image(&udid, &folder)?;
                let _ = std::fs::write(disk_image_note(), folder.to_string_lossy().as_bytes());
                Ok::<_, String>(())
            },
            move |this, mounted, cx| {
                if this.generation != generation {
                    return;
                }
                match mounted {
                    Ok(()) => {
                        this.set_screen(Screen::Looking);
                        this.shoot(cx);
                    }
                    Err(why) => this.set_screen(Screen::Failed(why.into())),
                }
                cx.notify();
            },
        );
    }

    /// The button let go over the screen (or off it): a tap where it was pressed if it barely
    /// moved, else a swipe from there to here, as long as it was held.
    fn let_go(
        &mut self,
        area: Bounds<Pixels>,
        size: (f32, f32),
        point: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((from, at)) = self.pressed.take() else {
            return;
        };
        let ms = (at.elapsed().as_millis() as u32).clamp(120, 1200);
        if let Some(gesture) = gesture_for(area, size, from, point, ms) {
            self.touch_screen(gesture, cx);
        }
    }

    /// A touch from the Screen tab: a tap, a swipe, typing or Home, then a fresh picture.
    fn touch_screen(&mut self, gesture: Gesture, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        let (source, generation) = (self.source.clone(), self.generation);
        self.background(
            cx,
            move || source.touch(&udid, gesture),
            move |this, touched, cx| {
                if this.generation != generation {
                    return;
                }
                match touched {
                    // A fresh picture at once, and quickly after: the screen's moving.
                    Ok(()) => {
                        this.shot_at = None;
                        this.still = 0;
                    }
                    Err(why) => this.notice = Some(why.into()),
                }
                cx.notify();
            },
        );
    }

    /// Mount the developer disk image in `folder`, as choosing it in the dialog does (the tests',
    /// which can't answer the dialog).
    #[cfg(any(test, feature = "test-support"))]
    pub fn mount_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.mount_disk_image(folder, cx);
    }

    /// Whether the screen is on show, and its picture's size (the tests').
    #[cfg(any(test, feature = "test-support", feature = "shots"))]
    pub fn screen_shown(&self) -> Option<(u32, u32)> {
        match &self.screen {
            Screen::Showing(_, size) => Some(*size),
            _ => None,
        }
    }

    /// The screen's picture again, as the tab takes one each second (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn shoot_again(&mut self, cx: &mut Context<Self>) {
        self.shoot(cx);
    }

    /// How many pictures in a row came the same as the one on show (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn screen_still(&self) -> u32 {
        self.still
    }

    /// The pictures of the screen the tool holds: the one on show, and the one before it until
    /// the next comes (the tests').
    #[cfg(any(test, feature = "test-support"))]
    pub fn screen_pictures(&self) -> usize {
        self.stale.len() + usize::from(matches!(self.screen, Screen::Showing(..)))
    }

    fn load_apps(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        if self.apps_loading {
            return;
        }
        self.apps_loading = true;
        let (source, generation) = (self.source.clone(), self.generation);
        let for_udid = udid.clone();
        self.background(
            cx,
            move || source.apps(&udid),
            move |this, apps, cx| {
                if this.generation != generation {
                    return;
                }
                this.apps_loading = false;
                this.apps = Some((for_udid, apps));
                cx.notify();
            },
        );
        cx.notify();
    }

    fn load_entries(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        self.entries_loading = true;
        self.removing = None;
        let (source, place) = (self.source.clone(), self.place.clone());
        let (listed, generation) = (place.clone(), self.generation);
        self.background(
            cx,
            move || source.list(&udid, &place.path, place.app.as_deref()),
            move |this, entries, cx| {
                // Only what's still asked for: the user may have moved on meanwhile.
                if this.place == listed && this.generation == generation {
                    this.entries_loading = false;
                    this.entries = Some((listed, entries));
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        // A fresh picture as the screen comes into view.
        if tab == Tab::Screen && matches!(self.screen, Screen::Showing(..)) {
            self.shoot(cx);
        }
        if tab == Tab::Files && self.entries.as_ref().is_none_or(|(p, _)| *p != self.place) {
            self.load_entries(cx);
        }
        cx.notify();
    }

    fn go_to(&mut self, place: Place, cx: &mut Context<Self>) {
        self.place = place;
        self.load_entries(cx);
    }

    fn open_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        let place = Place {
            app: self.place.app.clone(),
            path: files::join(&self.place.path, name),
        };
        self.go_to(place, cx);
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        if self.place.path == self.place.top() {
            return;
        }
        let parent = files::parents(&self.place.path)
            .pop()
            .unwrap_or_else(|| "/".to_string());
        let path = if self.place.app.is_some() && parent == "/" {
            "/Documents".to_string()
        } else {
            parent
        };
        let place = Place {
            app: self.place.app.clone(),
            path,
        };
        self.go_to(place, cx);
    }

    fn set_assistive(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        self.assistive_setting = true;
        self.assistive = Some(on);
        let source = self.source.clone();
        self.background(
            cx,
            move || source.set_assistive(&udid, on),
            move |this, done, cx| {
                this.assistive_setting = false;
                if let Err(e) = done {
                    this.assistive = Some(!on);
                    this.notice = Some(
                        format!(
                            "Couldn't turn AssistiveTouch {}: {e}",
                            if on { "on" } else { "off" }
                        )
                        .into(),
                    );
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The user's own install: a .ipa picked on this PC.
    fn install_app(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install".into()),
        });
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(package) = paths.into_iter().next() else { return };
            let name = shown_name(&package);
            if !package
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ipa"))
            {
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(format!("{name} isn't an app: pick a .ipa, signed for this iPhone.").into());
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.working = Some(format!("Installing {name}…"));
                this.notice = None;
                cx.notify();
            });
            let done = cx
                .background_executor()
                .spawn(async move { source.install(&udid, &package) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.working = None;
                this.notice = Some(match done {
                    Ok(()) => format!("Installed {name}. If it's development-signed, it opens once its developer is trusted on the iPhone (Settings › General › VPN & Device Management)."),
                    Err(e) => format!("Couldn't install {name}: {e}"),
                }.into());
                this.apps = None;
                this.load_apps(cx);
            });
        })
        .detach();
    }

    /// The user's own copy onto the phone, into the folder the Files list shows.
    fn add_file(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Copy to iPhone".into()),
        });
        let (source, place) = (self.source.clone(), self.place.clone());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(local) = paths.into_iter().next() else {
                return;
            };
            let name = shown_name(&local);
            let remote = files::join(&place.path, &name);
            let _ = this.update(cx, |this, cx| {
                this.working = Some(format!("Copying {name} to the iPhone…"));
                this.notice = None;
                cx.notify();
            });
            let done = cx
                .background_executor()
                .spawn(async move { source.push(&udid, &local, &remote, place.app.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.working = None;
                this.notice = Some(
                    match done {
                        Ok(n) => format!("Copied {name} ({}) to the iPhone.", files::human_size(n)),
                        Err(e) => format!("Couldn't copy {name}: {e}"),
                    }
                    .into(),
                );
                this.load_entries(cx);
            });
        })
        .detach();
    }

    /// The user's own copy off the phone, to where they choose on this PC.
    fn save_file(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        let home = trek_core::paths::home();
        let downloads = home.join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { home };
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let (source, place) = (self.source.clone(), self.place.clone());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(local))) = rx.await else {
                return;
            };
            let remote = files::join(&place.path, &name);
            let _ = this.update(cx, |this, cx| {
                this.working = Some(format!("Saving {name}…"));
                this.notice = None;
                cx.notify();
            });
            let shown = local.display().to_string();
            let done = cx
                .background_executor()
                .spawn(async move { source.pull(&udid, &remote, &local, place.app.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.working = None;
                this.notice = Some(
                    match done {
                        Ok(_) => format!("Saved {name} to {shown}."),
                        Err(e) => format!("Couldn't save {name}: {e}"),
                    }
                    .into(),
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// Remove a file: the first click asks, the second removes it.
    fn remove_file(&mut self, name: String, cx: &mut Context<Self>) {
        if self.removing.as_deref() != Some(name.as_str()) {
            self.removing = Some(name);
            cx.notify();
            return;
        }
        self.removing = None;
        let Some(udid) = self.phone().map(|p| p.udid.clone()) else {
            return;
        };
        let (source, place) = (self.source.clone(), self.place.clone());
        let remote = files::join(&place.path, &name);
        self.working = Some(format!("Removing {name}…"));
        self.background(
            cx,
            move || source.remove(&udid, &remote, place.app.as_deref()),
            move |this, done, cx| {
                this.working = None;
                this.notice = Some(
                    match done {
                        Ok(()) => format!("Removed {name} from the iPhone."),
                        Err(e) => format!("Couldn't remove {name}: {e}"),
                    }
                    .into(),
                );
                this.load_entries(cx);
            },
        );
        cx.notify();
    }

    /// An agent changed what's on the phone: the lists show it.
    fn changed(&mut self, method: &str, cx: &mut Context<Self>) {
        match method {
            "phone_install" => {
                self.apps = None;
                self.load_apps(cx);
            }
            "phone_push_file" | "phone_remove_file" if self.tab == Tab::Files => {
                self.load_entries(cx);
            }
            _ => {}
        }
    }

    // MARK: agents

    /// An agent's tool call: run after those before it.
    pub fn agent_call(&mut self, call: PhoneCall, _window: &mut Window, cx: &mut Context<Self>) {
        if let Err(why) = self
            .lease
            .claim(&call.thread, &call.title, call.turn, Instant::now())
        {
            call.answer(Err(why));
            return;
        }
        self.last = Some(Activity {
            title: call.title.clone(),
            action: starting(&call.method).into(),
            at: Instant::now(),
        });
        self.queue.push_back(call);
        cx.notify();
        if self.running {
            return;
        }
        self.running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |p, _| {
                    let next = p.queue.pop_front();
                    if next.is_none() {
                        p.running = false;
                    }
                    next.map(|c| (c, p.stops))
                });
                let Ok(Some((call, stops))) = next else { break };
                let method = call.method.clone();
                let result = Run {
                    this: this.clone(),
                    cx,
                    call: &call,
                    stops,
                }
                .go()
                .await;
                let done = match &result {
                    Ok(v) => v["text"]
                        .as_str()
                        .and_then(|t| t.lines().next())
                        .unwrap_or("Done")
                        .to_string(),
                    Err(e) => e.lines().next().unwrap_or_default().to_string(),
                };
                let ok = result.is_ok();
                call.answer(result);
                let ended = this.update(cx, |p, cx| {
                    if let Some(a) = p.last.as_mut() {
                        a.action = done;
                        a.at = Instant::now();
                    }
                    if ok {
                        p.changed(&method, cx);
                    }
                    cx.notify();
                });
                if ended.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The user takes the phone back: the call under way stops at its next step, those waiting
    /// fail, and the turn that held it gets no more.
    fn agent_stop(&mut self, cx: &mut Context<Self>) {
        self.stops += 1;
        self.lease.take_back();
        for call in self.queue.drain(..) {
            call.answer(Err(TOOK_OVER.into()));
        }
        self.last = None;
        cx.notify();
    }

    fn agent_hand_back(&mut self, cx: &mut Context<Self>) {
        self.lease.hand_back();
        cx.notify();
    }

    /// A made-up phone in one of the tool's situations, for design review (`control phone
    /// <state>` in an isolated capture run): ready, apps, files, documents, service, stopped,
    /// none, trust, locked, failed, working, agent; `real` for the user's real phone.
    #[cfg(feature = "shots")]
    pub fn demo(
        &mut self,
        state: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        // `tapped <n>`: the made-up phone took n taps (a click on its screen is one).
        if let Some(n) = state.strip_prefix("tapped ") {
            let n: usize = n.trim().parse()?;
            let taps = self
                .demo_phone
                .as_ref()
                .map(|p| p.taps())
                .unwrap_or_default();
            anyhow::ensure!(taps.len() == n, "phone tapped: {taps:?}, not {n}");
            return Ok(());
        }
        let made_up = match state {
            "real" => {
                self.source = Arc::new(Services);
                None
            }
            "ready" | "screen" | "apps" | "files" | "documents" | "working" | "agent" => {
                Some(MadeUp::ready())
            }
            "disk-image" => Some(MadeUp::without_disk_image()),
            "service" => Some(MadeUp::showing(Seen::NoService(pc::Service::Missing))),
            "stopped" => Some(MadeUp::showing(Seen::NoService(pc::Service::Stopped))),
            "none" => Some(MadeUp::showing(Seen::NoPhone)),
            "trust" => Some(MadeUp::showing(Seen::Untrusted(Phone {
                name: String::new(),
                ..MadeUp::phone()
            }))),
            "locked" => Some(MadeUp::showing(Seen::Locked(MadeUp::phone()))),
            "failed" => Some(MadeUp::showing(Seen::Failed(
                "Couldn't open lockdown: device socket io failed".into(),
            ))),
            other => anyhow::bail!(
                "phone: unknown state {other:?} (ready|screen|disk-image|apps|files|documents|working|agent|service|stopped|none|trust|locked|failed|real, or tapped <n>)"
            ),
        };
        self.demo_phone = made_up.clone();
        if let Some(made_up) = made_up {
            self.source = made_up;
        }
        self.set_screen(Screen::Unknown);
        self.screen_of = None;
        self.shooting = false;
        self.shot_at = None;
        self.agent_shot = None;
        // What was on its way from the phone before is the old phone's.
        self.generation += 1;
        self.looking = false;
        self.apps_loading = false;
        self.entries_loading = false;
        self.seen = None;
        self.apps = None;
        self.entries = None;
        self.place = Place::media();
        self.notice = None;
        self.working = None;
        self.last = None;
        self.tab = if matches!(state, "files" | "documents") {
            Tab::Files
        } else {
            Tab::Apps
        };
        if state == "documents" {
            self.place = Place::documents("com.example.notes".into());
        }
        if state == "working" {
            self.working = Some("Installing Field Notes.ipa…".into());
        }
        if state == "agent" {
            self.last = Some(Activity {
                title: "Ship the notes app".into(),
                action: "Installing an app…".into(),
                at: Instant::now(),
            });
            self.lease
                .claim("demo", "Ship the notes app", None, Instant::now())
                .map_err(anyhow::Error::msg)?;
        }
        self.look(cx);
        cx.notify();
        Ok(())
    }

    // MARK: drawing

    /// One line: the device glyph, its name, then iOS, the link and the model, muted.
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (name, detail) = match &self.seen {
            Some(Seen::Ready(p) | Seen::Untrusted(p) | Seen::Locked(p)) => {
                let (name, detail) = pc::describe(p);
                (name, Some(detail))
            }
            _ => ("No iPhone".to_string(), None),
        };
        h_flex()
            .id("phone-header")
            .test_support()
            .h(px(36.))
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(Lucide::Smartphone)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(to_rems(text::LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .when_some(detail, |el, d| {
                        el.child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(to_rems(text::META))
                                .text_color(theme.muted_foreground)
                                .child(d),
                        )
                    }),
            )
            .when(self.looking, |el| el.child(Spinner::new().xsmall()))
            .child(
                crate::ui::icon_button(
                    "phone-refresh",
                    Lucide::RefreshCw,
                    "Look for the iPhone again",
                )
                .xsmall()
                .on_click(cx.listener(|this, _, _, cx| this.look_now(cx))),
            )
            .into_any_element()
    }

    /// Who has the phone: an agent at it (with Stop), or the user who took it back.
    fn strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let row = |id: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .h(px(32.))
                .px(px(12.))
                .gap(px(8.))
                .text_size(to_rems(text::META))
                .border_b_1()
                .border_color(theme.border)
        };
        let busy = self.busy();
        let holder = self.lease.holder(Instant::now()).is_some();
        if let Some(a) = self
            .last
            .as_ref()
            .filter(|a| busy || holder || a.at.elapsed() < STRIP_LINGER)
        {
            return Some(
                row("phone-agent-strip")
                    .bg(ember.opacity(0.1))
                    .child(Icon::new(Lucide::Sparkles).xsmall().text_color(ember))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(if busy {
                                "Agent is using your iPhone"
                            } else {
                                "Agent has your iPhone"
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} · {}", a.title, a.action)),
                    )
                    .child(
                        Button::new("phone-agent-stop")
                            .xsmall()
                            .outline()
                            .icon(Lucide::Hand)
                            .label("Stop")
                            .tooltip(
                                "Take the iPhone back: the agent's iPhone tools stop for this turn",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.agent_stop(cx))),
                    )
                    .into_any_element(),
            );
        }
        self.lease.taken_over().then(|| {
            row("phone-agent-taken")
                .bg(theme.foreground.opacity(0.04))
                .child(
                    Icon::new(Lucide::Hand)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(theme.muted_foreground)
                        .child("You have the iPhone. The agent can use it again in its next turn."),
                )
                .child(
                    Button::new("phone-agent-hand-back")
                        .xsmall()
                        .ghost()
                        .label("Hand back")
                        .on_click(cx.listener(|this, _, _, cx| this.agent_hand_back(cx))),
                )
                .into_any_element()
        })
    }

    /// Before the phone is ready: the one thing to do, and what helps.
    fn state_card(&self, say: &pc::Say, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let icon = match &self.seen {
            Some(Seen::NoService(_)) => Icon::new(IconName::ArrowDown),
            Some(Seen::Untrusted(_)) => Icon::new(Lucide::ShieldCheck),
            Some(Seen::Locked(_)) => Icon::new(Lucide::Lock),
            Some(Seen::Failed(_)) => Icon::new(IconName::CircleAlert),
            _ => Icon::new(Lucide::Plug),
        };
        v_flex()
            .id("phone-state")
            .test_support()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .text_center()
            .child(
                div()
                    .size(px(40.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.foreground.opacity(0.06))
                    .child(icon.text_color(theme.muted_foreground)),
            )
            .child(
                div()
                    .id("phone-state-title")
                    .test_support()
                    .text_size(to_rems(text::BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .child(say.title),
            )
            .child(
                div()
                    .max_w(px(340.))
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground)
                    .child(say.detail.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .when(say.store, |el| {
                        el.child(
                            Button::new("phone-get-apple-devices")
                                .small()
                                .primary()
                                .icon(IconName::ExternalLink)
                                .label("Open the Microsoft Store")
                                .on_click(|_, _, cx| cx.open_url(pc::STORE_URL)),
                        )
                    })
                    .child(
                        Button::new("phone-look-again")
                            .small()
                            .outline()
                            .label("Look again")
                            .disabled(self.looking)
                            .on_click(cx.listener(|this, _, _, cx| this.look_now(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The phone: its picture's place (which says where the screen is), name, model and iOS, and
    /// AssistiveTouch.
    fn device_card(&self, phone: &Phone, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (name, detail) = pc::describe(phone);
        let assistive = self.assistive;
        v_flex()
            .id("phone-device")
            .test_support()
            .px_3()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    // The phone, without its picture: on a PC the screen isn't here yet.
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(36.))
                            .h(px(72.))
                            .rounded(px(9.)) // scale: ok, an iPhone's own corners at this size
                            .bg(rgb(0x0B0B0D))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(12.))
                                    .h(px(3.))
                                    .rounded_full()
                                    .bg(rgb(0xFFFFFF).opacity(0.18)),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(to_rems(text::TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(to_rems(text::META))
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(detail),
                            )
                            .child(
                                h_flex()
                                    .pt_2()
                                    .gap_2()
                                    .text_size(to_rems(text::LABEL))
                                    .child(div().flex_1().child("AssistiveTouch"))
                                    .child(
                                        Switch::new("phone-assistive")
                                            .small()
                                            .checked(assistive == Some(true))
                                            .disabled(assistive.is_none() || self.assistive_setting)
                                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                                this.set_assistive(*on, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The Screen tab: the phone's screen, a click on it a tap; or what it needs first.
    fn screen_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let note = |id: &'static str, icon: Icon, words: SharedString| {
            h_flex()
                .id(id)
                .test_support()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(fluent::control(radius::CONTROL)))
                .bg(theme.foreground.opacity(0.04))
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(icon.xsmall().flex_shrink_0())
                .child(div().flex_1().min_w_0().child(words))
        };
        match &self.screen {
            Screen::Showing(image, (width, height)) => {
                let (pw, ph) = ((*width).max(1) as f32, (*height).max(1) as f32);
                // Where the area was drawn this frame (its canvas measures it before a click lands).
                let area = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
                let measured = area.clone();
                let (over, out) = (area.clone(), area.clone());
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(
                        div()
                            .id("phone-screen")
                            .test_support()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .overflow_hidden()
                            .cursor_pointer()
                            // Laid over the area (a definite box), so the picture fits inside it.
                            .child(
                                img(image.clone())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                canvas(move |b, _, _| measured.set(b), |_, _, _, _| {})
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, _, _| {
                                    this.pressed = on_picture(area.get(), (pw, ph), e.position)
                                        .map(|at| (at, Instant::now()));
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseUpEvent, _, cx| {
                                    this.let_go(over.get(), (pw, ph), e.position, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseUpEvent, _, cx| {
                                    this.let_go(out.get(), (pw, ph), e.position, cx);
                                }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("phone-home")
                                    .small()
                                    .outline()
                                    .label(pc::SCREEN_HOME)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.touch_screen(Gesture::Home, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.typing).id("phone-type").small()),
                            ),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(pc::SCREEN_HINT),
                    )
                    .into_any_element()
            }
            Screen::NeedsImage => v_flex()
                .p_3()
                .gap_2()
                .child(note(
                    "phone-screen-image",
                    Icon::new(Lucide::Monitor),
                    pc::SCREEN_NEEDS_IMAGE.into(),
                ))
                .child(
                    h_flex().child(
                        Button::new("phone-screen-choose")
                            .small()
                            .outline()
                            .label(pc::SCREEN_CHOOSE)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_disk_image(cx))),
                    ),
                )
                .into_any_element(),
            Screen::Failed(why) => v_flex()
                .p_3()
                .gap_2()
                .child(note(
                    "phone-screen-failed",
                    Icon::new(IconName::TriangleAlert),
                    why.clone(),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("phone-screen-again")
                                .small()
                                .outline()
                                .label("Try again")
                                .on_click(cx.listener(|this, _, _, cx| this.ask_screen(cx))),
                        )
                        .child(
                            Button::new("phone-screen-choose")
                                .small()
                                .ghost()
                                .label(pc::SCREEN_CHOOSE)
                                .on_click(cx.listener(|this, _, _, cx| this.choose_disk_image(cx))),
                        ),
                )
                .into_any_element(),
            Screen::Mounting => div()
                .p_3()
                .child(note(
                    "phone-screen-mounting",
                    Icon::new(Lucide::Monitor),
                    pc::SCREEN_MOUNTING.into(),
                ))
                .into_any_element(),
            Screen::Looking | Screen::Unknown => div()
                .p_3()
                .child(note(
                    "phone-screen-looking",
                    Icon::new(Lucide::Monitor),
                    pc::SCREEN_LOOKING.into(),
                ))
                .into_any_element(),
        }
    }

    /// Apps, Files and the Screen, and what each can do.
    fn tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let count = match &self.apps {
            Some((_, Ok(apps))) => format!("Apps · {}", apps.len()),
            _ => "Apps".to_string(),
        };
        let panel = cx.entity().downgrade();
        let pick = move |tab: Tab, _: &mut Window, cx: &mut App| {
            let _ = panel.update(cx, |p, cx| p.show_tab(tab, cx));
        };
        let busy = self.working.is_some();
        // Files go into a folder of the media folder (not its top, nor the Camera Roll, which is
        // Photos' own), or into an app's Documents.
        let can_add = self.place.app.is_some()
            || (self.place.path != "/"
                && files::check_remote(&files::join(&self.place.path, "x"), false).is_ok());
        let action = match self.tab {
            Tab::Screen => div().into_any_element(),
            Tab::Apps => Button::new("phone-install")
                .small()
                .outline()
                .icon(Lucide::PackagePlus)
                .label("Install an app…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.install_app(cx)))
                .into_any_element(),
            Tab::Files => Button::new("phone-add-file")
                .small()
                .outline()
                .icon(Lucide::FilePlus)
                .label("Add a file…")
                .disabled(busy || !can_add)
                .tooltip(if can_add {
                    "Copy a file from this PC into this folder"
                } else {
                    "Open a folder to copy into first (Downloads, say; not the Camera Roll)"
                })
                .on_click(cx.listener(|this, _, _, cx| this.add_file(cx)))
                .into_any_element(),
        };
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(crate::ui::segmented(
                "phone-tab",
                vec![
                    (Tab::Apps, count),
                    (Tab::Files, "Files".to_string()),
                    (Tab::Screen, "Screen".to_string()),
                ],
                self.tab,
                pick,
                cx,
            ))
            .child(div().flex_1())
            .child(action)
            .into_any_element()
    }

    /// What the tool is doing for the user, under the tabs.
    fn working(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        self.working.clone().map(|what| {
            h_flex()
                .id("phone-working")
                .test_support()
                .px_3()
                .py(px(6.))
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(Spinner::new().xsmall())
                .child(what)
                .into_any_element()
        })
    }

    fn apps_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = v_flex()
            .id("phone-apps")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1();
        match &self.apps {
            None => list
                .items_center()
                .pt_6()
                .child(Spinner::new().small())
                .into_any_element(),
            Some((_, Err(e))) => list
                .px_3()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(format!("Couldn't list the apps: {e}"))
                .into_any_element(),
            Some((_, Ok(apps))) if apps.is_empty() => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child("No apps of yours on this iPhone yet. Install one signed for it with Install an app…")
                .into_any_element(),
            Some((_, Ok(apps))) => list
                .children(apps.iter().map(|a| {
                    let docs = a.bundle.clone();
                    h_flex()
                        .id(SharedString::from(format!("phone-app-{}", a.bundle)))
                        .test_support()
                        .mx_1()
                        .h(px(40.))
                        .px_2()
                        .gap_2()
                        .rounded(px(fluent::control(radius::CONTROL)))
                        .hover(|s| s.bg(theme.list_hover))
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(26.))
                                .rounded(px(radius::CONTROL))
                                .bg(theme.foreground.opacity(0.08))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(Lucide::AppWindow)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(to_rems(text::LABEL))
                                        .truncate()
                                        .child(a.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(to_rems(text::META))
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(if a.version.is_empty() {
                                            a.bundle.clone()
                                        } else {
                                            format!("{} · {}", a.version, a.bundle)
                                        }),
                                ),
                        )
                        .when(a.file_sharing, |el| {
                            el.child(
                                Button::new(SharedString::from(format!("phone-app-files-{}", a.bundle)))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Folder)
                                    .label("Files")
                                    .tooltip("This app takes files: open its Documents")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.tab = Tab::Files;
                                        this.go_to(Place::documents(docs.clone()), cx);
                                    })),
                            )
                        })
                }))
                .into_any_element(),
        }
    }

    /// Where the Files list is: the media folder or an app's Documents (a menu), the folder's
    /// path, and the way up.
    fn place_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let sharing: Vec<(String, String)> = match &self.apps {
            Some((_, Ok(apps))) => apps
                .iter()
                .filter(|a| a.file_sharing)
                .map(|a| (a.bundle.clone(), a.name.clone()))
                .collect(),
            _ => vec![],
        };
        let label = match &self.place.app {
            None => "iPhone".to_string(),
            Some(bundle) => sharing
                .iter()
                .find(|(b, _)| b == bundle)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| bundle.clone()),
        };
        let chosen = self.place.app.clone();
        let panel = cx.entity().downgrade();
        let places = Button::new("phone-files-place")
            .xsmall()
            .ghost()
            .icon(IconName::ChevronDown)
            .label(label)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                let go = |place: Place| {
                    let panel = panel.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let place = place.clone();
                        let _ = panel.update(cx, |p, cx| p.go_to(place, cx));
                    }
                };
                menu = menu.min_w(px(220.)).item(
                    PopupMenuItem::new("iPhone (its media folder)")
                        .icon(Lucide::Smartphone)
                        .checked(chosen.is_none())
                        .on_click(go(Place::media())),
                );
                for (bundle, name) in &sharing {
                    menu = menu.item(
                        PopupMenuItem::new(format!("{name}'s Documents"))
                            .icon(IconName::Folder)
                            .checked(chosen.as_deref() == Some(bundle.as_str()))
                            .on_click(go(Place::documents(bundle.clone()))),
                    );
                }
                menu
            });
        let at_top = self.place.path == self.place.top();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .text_size(to_rems(text::META))
            .border_b_1()
            .border_color(theme.border)
            .child(places)
            .child(
                crate::ui::icon_button("phone-files-up", IconName::ArrowUp, "Up a folder")
                    .xsmall()
                    .disabled(at_top)
                    .on_click(cx.listener(|this, _, _, cx| this.up(cx))),
            )
            .child(
                div()
                    .id("phone-files-path")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(self.place.path.clone()),
            )
            .into_any_element()
    }

    fn files_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = v_flex()
            .id("phone-files")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1();
        let here = self
            .entries
            .as_ref()
            .filter(|(place, _)| *place == self.place && !self.entries_loading);
        let list = match here {
            None => list.items_center().pt_6().child(Spinner::new().small()),
            Some((_, Err(e))) => list
                .px_3()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(format!("Couldn't list {}: {e}", self.place.path)),
            Some((_, Ok(entries))) if entries.is_empty() => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child("Nothing here yet."),
            Some((_, Ok(entries))) => {
                let camera_roll = self.place.app.is_none()
                    && files::check_remote(&files::join(&self.place.path, "x"), false).is_err();
                list.children(entries.iter().map(|e| self.file_row(e, camera_roll, cx)))
            }
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(self.place_bar(cx))
            .child(list)
            .into_any_element()
    }

    /// One file or folder: a folder opens; a file is saved to this PC, or removed (not in the
    /// Camera Roll, which is Photos' own).
    fn file_row(&self, e: &Entry, camera_roll: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let name = e.name.clone();
        let row_id = SharedString::from(format!("phone-file-{}", e.name));
        let row = h_flex()
            .id(row_id)
            .test_support()
            .mx_1()
            .h(px(28.))
            .px_2()
            .gap(px(6.))
            .rounded(px(fluent::control(radius::CONTROL)))
            .text_size(to_rems(text::LABEL))
            .hover(|s| s.bg(theme.list_hover))
            .child(if e.dir {
                crate::file_icon::folder(&e.name, false, px(14.), cx)
            } else {
                crate::file_icon::badge(&e.name, px(14.), cx)
            })
            .child(div().flex_1().min_w_0().truncate().child(e.name.clone()));
        if e.dir {
            return row
                .cursor_pointer()
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_folder(&name, cx)))
                .into_any_element();
        }
        let asking = self.removing.as_deref() == Some(e.name.as_str());
        let (save, remove) = (name.clone(), name);
        row.child(
            div()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(files::human_size(e.size)),
        )
        .child(
            crate::ui::icon_button(
                SharedString::from(format!("phone-file-save-{}", e.name)),
                Lucide::Download,
                "Save to this PC…",
            )
            .xsmall()
            .disabled(self.working.is_some())
            .on_click(cx.listener(move |this, _, _, cx| this.save_file(save.clone(), cx))),
        )
        .when(!camera_roll, |row| {
            row.child(
                Button::new(SharedString::from(format!("phone-file-remove-{}", e.name)))
                    .xsmall()
                    .when(asking, |b| b.danger().label("Remove?"))
                    .when(!asking, |b| b.ghost().icon(Lucide::Trash))
                    .tooltip(if asking {
                        "Click again to remove it from the iPhone"
                    } else {
                        "Remove from the iPhone"
                    })
                    .disabled(self.working.is_some())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.remove_file(remove.clone(), cx)),
                    ),
            )
        })
        .into_any_element()
    }
}

impl Render for PhonePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let notice = self.notice.clone();
        let body = match self.seen.clone() {
            None => v_flex()
                .id("phone-looking")
                .test_support()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(to_rems(text::LABEL))
                .text_color(theme.muted_foreground)
                .child(Spinner::new().small())
                .child("Looking for the iPhone…")
                .into_any_element(),
            Some(Seen::Ready(phone)) => v_flex()
                .flex_1()
                .min_h_0()
                .child(self.device_card(&phone, cx))
                .child(self.tabs(cx))
                .children(self.working(cx))
                .child(match self.tab {
                    Tab::Apps => self.apps_list(cx),
                    Tab::Files => self.files_list(cx),
                    Tab::Screen => self.screen_view(cx),
                })
                .into_any_element(),
            Some(seen) => match pc::say(&seen) {
                Some(say) => self.state_card(&say, cx),
                None => div().into_any_element(),
            },
        };
        v_flex()
            .size_full()
            .child(self.header(cx))
            .children(self.strip(cx))
            .when_some(notice, |el, n| {
                el.child(
                    h_flex()
                        .id("phone-notice")
                        .test_support()
                        .px_3()
                        .py(px(6.))
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .bg(crate::palette::amber(cx).opacity(0.08))
                        .child(Icon::new(IconName::Info).xsmall())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(to_rems(text::META))
                                .child(n),
                        )
                        .child(
                            crate::ui::icon_button(
                                "phone-notice-close",
                                IconName::Close,
                                "Dismiss",
                            )
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            })),
                        ),
                )
            })
            .child(body)
    }
}

/// A file's name, as the tool says it.
fn shown_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// "Installing an app…" for the strip while a call of `method` runs. (As on a Mac, for the
/// tools a PC serves.)
fn starting(method: &str) -> &'static str {
    match method {
        "phone_screenshot" => "Looking at the screen…",
        "phone_elements" => "Reading the screen…",
        "phone_tap" => "Tapping…",
        "phone_swipe" => "Swiping…",
        "phone_type" => "Typing…",
        "phone_key" => "Pressing a key…",
        "phone_home" => "Going home…",
        "phone_open_app" => "Opening an app…",
        "phone_install" => "Installing an app…",
        "phone_push_file" | "phone_pull_file" => "Copying a file…",
        "phone_remove_file" => "Removing a file…",
        "phone_files" => "Looking at its files…",
        "phone_apps" => "Listing its apps…",
        "phone_release" => "Letting go…",
        _ => "Checking the iPhone…",
    }
}

/// The file name of a path, for approvals.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// What an approval card says for a sensitive call. (As on a Mac.)
fn ask_text(method: &str, p: &Value) -> (String, String) {
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    let place = |remote: String| match p["app"].as_str() {
        Some(app) => format!("{remote} in {app}"),
        None => remote,
    };
    match method {
        "phone_install" => (
            format!("Install {} on your iPhone", file_name(&s("path"))),
            s("path"),
        ),
        "phone_remove_file" => (
            format!("Remove {} from your iPhone", file_name(&s("path"))),
            place(s("path")),
        ),
        _ => (
            format!("Copy {} to your iPhone", file_name(&s("local"))),
            format!("{} → {}", s("local"), place(s("remote"))),
        ),
    }
}

/// A call being run.
struct Run<'a> {
    this: WeakEntity<PhonePanel>,
    cx: &'a mut AsyncApp,
    call: &'a PhoneCall,
    /// `PhonePanel::stops` when it started: Stop since means the user took over.
    stops: u64,
}

fn gone<E>(_: E) -> String {
    "Trek's iPhone tool closed.".to_string()
}

fn text(t: impl Into<String>) -> Result<Value, String> {
    Ok(json!({ "text": t.into() }))
}

/// A picture of `size` drawn fitted and centred in `area`: its left, top, width and height.
fn fitted(area: Bounds<Pixels>, size: (f32, f32)) -> Option<(f32, f32, f32, f32)> {
    let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
    if aw <= 0. || ah <= 0. || size.0 <= 0. || size.1 <= 0. {
        return None;
    }
    let scale = (aw / size.0).min(ah / size.1);
    let (w, h) = (size.0 * scale, size.1 * scale);
    let left = f32::from(area.origin.x) + (aw - w) / 2.;
    let top = f32::from(area.origin.y) + (ah - h) / 2.;
    Some((left, top, w, h))
}

/// Where `point` falls on a picture of `size` drawn fitted and centred in `area`, as fractions of
/// its width and height; `None` off the picture (in the bars beside it).
fn on_picture(area: Bounds<Pixels>, size: (f32, f32), point: Point<Pixels>) -> Option<(f32, f32)> {
    let (left, top, w, h) = fitted(area, size)?;
    let (x, y) = (
        (f32::from(point.x) - left) / w,
        (f32::from(point.y) - top) / h,
    );
    ((0. ..=1.).contains(&x) && (0. ..=1.).contains(&y)).then_some((x, y))
}

/// An element's role as the driver names its type on a Mac (`Button`, `StaticText`), which
/// trek-mcp reads back into the same words (`outline::role_words`).
fn driver_type(role: &str, secure: bool) -> String {
    match role {
        "text" => "StaticText".into(),
        "text field" if secure => "SecureTextField".into(),
        _ => role
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect(),
    }
}

/// A picture's fingerprint: one the same as the last isn't decoded again.
fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// How far a press may move on the drawn picture and still be a tap.
const DRAG_PX: f32 = 8.;

/// A press at `from` (fractions of the picture) let go at `point`: a tap there when it moved less
/// than `DRAG_PX` on the drawn picture, else a swipe to where it was let go (held to the
/// picture's edges), over `ms`.
fn gesture_for(
    area: Bounds<Pixels>,
    size: (f32, f32),
    from: (f32, f32),
    point: Point<Pixels>,
    ms: u32,
) -> Option<Gesture> {
    let (left, top, w, h) = fitted(area, size)?;
    let to = (
        ((f32::from(point.x) - left) / w).clamp(0., 1.),
        ((f32::from(point.y) - top) / h).clamp(0., 1.),
    );
    let moved = ((to.0 - from.0) * w).hypot((to.1 - from.1) * h);
    Some(if moved < DRAG_PX {
        Gesture::Tap(from.0, from.1)
    } else {
        Gesture::Swipe { from, to, ms }
    })
}

/// What an agent hears when the screen can't be had yet.
const NO_DISK_IMAGE: &str = pc::SCREEN_ADVICE;

/// `shot` as a PNG whose long side is at most `edge` pixels (an agent's picture), and its size.
fn shrink(shot: Shot, edge: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let long = shot.width.max(shot.height);
    if long <= edge {
        return Ok((shot.png, shot.width, shot.height));
    }
    let picture = image::load_from_memory_with_format(&shot.png, image::ImageFormat::Png)
        .map_err(|e| format!("The screenshot didn't read: {e}"))?;
    let scale = edge as f32 / long as f32;
    let width = ((shot.width as f32 * scale).round() as u32).max(1);
    let height = ((shot.height as f32 * scale).round() as u32).max(1);
    let small = picture.resize_exact(width, height, image::imageops::FilterType::Triangle);
    let mut png = std::io::Cursor::new(Vec::new());
    small
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("The screenshot didn't write: {e}"))?;
    Ok((png.into_inner(), width, height))
}

/// A path on this PC an agent gave: a whole one (a relative one would land in Trek's own folder,
/// not the agent's).
fn local_path(p: &Value, key: &str) -> Result<PathBuf, String> {
    let given = p[key].as_str().unwrap_or_default().trim();
    let path = PathBuf::from(given);
    if given.is_empty() || !path.is_absolute() {
        return Err(format!(
            "{key} must be a full path on this PC (like C:\\Users\\you\\Downloads\\file.txt), not {given:?}."
        ));
    }
    Ok(path)
}

impl Run<'_> {
    /// A file of its own for an agent's picture, in the data folder (trek-mcp reads it, then
    /// removes it).
    fn shot_path(&mut self) -> Result<PathBuf, String> {
        let n = self
            .this
            .update(self.cx, |p, _| {
                p.shots += 1;
                p.shots
            })
            .map_err(gone)?;
        let dir = trek_core::paths::data_dir().join("phone-shots");
        let _ = std::fs::create_dir_all(&dir);
        Ok(dir.join(format!("{}-{n}.png", std::process::id())))
    }

    async fn sleep(&mut self, d: Duration) {
        self.cx.background_executor().timer(d).await;
    }

    fn check(&mut self) -> Result<(), String> {
        let stops = self.this.update(self.cx, |p, _| p.stops).map_err(gone)?;
        if stops != self.stops {
            return Err(TOOK_OVER.into());
        }
        Ok(())
    }

    /// Blocking work (the phone's services) off the UI thread.
    async fn blocking<R: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> R {
        self.cx
            .background_executor()
            .spawn(async move { work() })
            .await
    }

    fn source(&mut self) -> Result<Arc<dyn Source>, String> {
        self.this
            .update(self.cx, |p, _| p.source.clone())
            .map_err(gone)
    }

    /// Where the phone stands now (the tool shows it too).
    async fn look(&mut self) -> Result<(Seen, Option<bool>), String> {
        let source = self.source()?;
        let (seen, assistive) = self
            .blocking(move || {
                let seen = source.probe();
                let assistive = match &seen {
                    Seen::Ready(phone) => source.assistive(&phone.udid).ok(),
                    _ => None,
                };
                (seen, assistive)
            })
            .await;
        let shown = seen.clone();
        self.this
            .update(self.cx, |p, cx| p.saw(shown, assistive, cx))
            .map_err(gone)?;
        Ok((seen, assistive))
    }

    /// The phone's udid once it's ready; else what the user must do first.
    async fn ready(&mut self) -> Result<String, String> {
        match self.look().await? {
            (Seen::Ready(phone), _) => Ok(phone.udid),
            (seen, _) => Err(pc::say(&seen)
                .map(|s| s.advice)
                .unwrap_or_else(|| "The iPhone isn't ready.".into())),
        }
    }

    async fn go(mut self) -> Result<Value, String> {
        let method = self.call.method.clone();
        let p = self.call.params.clone();
        // The screen and taps are the Mac's for now.
        if !tools::pc_serves(&method) {
            return Err(tools::on_a_mac(&method));
        }
        if let Some(kind) = tools::sensitive(&method) {
            self.ask(kind, &method, &p).await?;
        }
        self.check()?;
        let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        let app = p["app"].as_str().map(str::to_string);
        match method.as_str() {
            "phone_status" => {
                let (seen, assistive) = self.look().await?;
                let screen = match &seen {
                    Seen::Ready(phone) => {
                        let (source, udid) = (self.source()?, phone.udid.clone());
                        self.blocking(move || source.disk_image(&udid).ok()).await
                    }
                    _ => None,
                };
                let holder = self
                    .this
                    .update(self.cx, |p, _| {
                        p.lease.holder(Instant::now()).map(|h| h.title.clone())
                    })
                    .map_err(gone)?;
                text(pc::status(&seen, screen, assistive, holder.as_deref()))
            }
            "phone_screenshot" => {
                let udid = self.ready().await?;
                let source = self.source()?;
                let mounted = {
                    let (source, udid) = (source.clone(), udid.clone());
                    self.blocking(move || source.disk_image(&udid)).await?
                };
                if !mounted {
                    return Err(NO_DISK_IMAGE.into());
                }
                let edge = p["max_edge"]
                    .as_u64()
                    .map(|e| (e as u32).clamp(200, 2000))
                    .unwrap_or(SHOT_EDGE);
                let path = self.shot_path()?;
                let file = path.clone();
                let (width, height) = self
                    .blocking(move || -> Result<(u32, u32), String> {
                        let (png, width, height) = shrink(source.screenshot(&udid)?, edge)?;
                        std::fs::write(&file, png).map_err(|e| e.to_string())?;
                        Ok((width, height))
                    })
                    .await?;
                self.this
                    .update(self.cx, |p, _| p.agent_shot = Some((width, height)))
                    .map_err(gone)?;
                Ok(json!({
                    "text": format!("The iPhone's screen, {width}×{height}. x/y for phone_tap are pixels in this picture."),
                    "image": path.display().to_string(),
                }))
            }
            "phone_tap" => {
                let udid = self.ready().await?;
                let (Some(x), Some(y)) = (p["x"].as_f64(), p["y"].as_f64()) else {
                    return Err(
                        "phone_tap takes x and y, pixels in the latest phone_screenshot.".into(),
                    );
                };
                let (width, height) = self
                    .this
                    .update(self.cx, |p, _| p.agent_shot)
                    .map_err(gone)?
                    .ok_or("Take a phone_screenshot first: x and y are pixels in it.")?;
                let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
                if !(0. ..=1.).contains(&fx) || !(0. ..=1.).contains(&fy) {
                    return Err(format!(
                        "({x}, {y}) is off the screen: the latest phone_screenshot is {width}×{height}."
                    ));
                }
                let source = self.source()?;
                self.blocking(move || source.tap(&udid, fx, fy)).await?;
                text(format!(
                    "Tapped ({x}, {y}). Take a screenshot to see what it did."
                ))
            }
            // The screen as the driver lists it on a Mac (trek-mcp makes the outline, its refs and
            // phone_wait from this): read through the phone's accessibility, its frames in points,
            // and the picture taps are read in: the agent's latest screenshot, else the points.
            "phone_elements" => {
                let udid = self.ready().await?;
                let source = self.source()?;
                let screen = self.blocking(move || source.elements(&udid)).await?;
                let (w, h) = screen.size.ok_or(
                    "The iPhone didn't say its screen's size, so its elements can't be placed: use phone_screenshot and tap by x/y.",
                )?;
                let picture = self
                    .this
                    .update(self.cx, |p, _| {
                        *p.agent_shot
                            .get_or_insert((w.round() as u32, h.round() as u32))
                    })
                    .map_err(gone)?;
                let elements: Vec<Value> = screen
                    .elements
                    .iter()
                    .map(|e| {
                        json!({
                            "type": driver_type(&e.role, e.secure),
                            "label": e.label,
                            "identifier": e.identifier,
                            "value": e.value,
                            "depth": e.depth,
                            "enabled": e.enabled,
                            "focused": e.focused,
                            "selected": e.selected,
                            "secure": e.secure,
                            "frame": e.frame.map(|f| json!([f.x, f.y, f.w, f.h])),
                        })
                    })
                    .collect();
                let count = elements.len();
                Ok(json!({
                    "elements": elements,
                    "width": w,
                    "height": h,
                    "picture": { "width": picture.0, "height": picture.1 },
                    "text": format!("Read the screen: {count} element{}.", if count == 1 { "" } else { "s" }),
                }))
            }
            // trek-mcp's own: it asks for phone_elements until what's waited for shows.
            "phone_wait" => Err(
                "phone_wait is answered by trek-mcp, from phone_elements: call that here.".into(),
            ),
            "phone_swipe" => {
                let udid = self.ready().await?;
                let at = |k: &str| p[k].as_f64();
                let (Some(x1), Some(y1), Some(x2), Some(y2)) =
                    (at("x1"), at("y1"), at("x2"), at("y2"))
                else {
                    return Err(
                        "phone_swipe takes x1, y1, x2 and y2, pixels in the latest phone_screenshot."
                            .into(),
                    );
                };
                let (width, height) = self
                    .this
                    .update(self.cx, |p, _| p.agent_shot)
                    .map_err(gone)?
                    .ok_or("Take a phone_screenshot first: x and y are pixels in it.")?;
                let fraction = |x: f64, y: f64| (x as f32 / width as f32, y as f32 / height as f32);
                let (from, to) = (fraction(x1, y1), fraction(x2, y2));
                if [from.0, from.1, to.0, to.1]
                    .iter()
                    .any(|v| !(0. ..=1.).contains(v))
                {
                    return Err(format!(
                        "The swipe goes off the screen: the latest phone_screenshot is {width}×{height}."
                    ));
                }
                let ms = p["duration_ms"].as_u64().unwrap_or(300).clamp(50, 5000) as u32;
                let source = self.source()?;
                self.blocking(move || source.touch(&udid, Gesture::Swipe { from, to, ms }))
                    .await?;
                text(format!(
                    "Swiped from ({x1}, {y1}) to ({x2}, {y2}). Take a screenshot to see what it did."
                ))
            }
            "phone_type" => {
                let udid = self.ready().await?;
                let Some(typed) = p["text"].as_str() else {
                    return Err("phone_type takes text.".into());
                };
                let submit = p["submit"].as_bool().unwrap_or(false);
                let keys = if submit {
                    format!("{typed}\n")
                } else {
                    typed.to_string()
                };
                let source = self.source()?;
                self.blocking(move || source.touch(&udid, Gesture::Type(keys)))
                    .await?;
                text(if submit {
                    "Typed it and pressed Return."
                } else {
                    "Typed it."
                })
            }
            "phone_key" => {
                let udid = self.ready().await?;
                let Some(combo) = p["key"].as_str() else {
                    return Err("phone_key takes key, e.g. enter or cmd+space.".into());
                };
                let (held, key) = coredevice::key_combo(combo)?;
                let source = self.source()?;
                self.blocking(move || source.touch(&udid, Gesture::Key { held, key }))
                    .await?;
                text(format!("Pressed {combo}."))
            }
            "phone_home" => {
                let udid = self.ready().await?;
                let source = self.source()?;
                self.blocking(move || source.touch(&udid, Gesture::Home))
                    .await?;
                text("Went to the Home Screen.")
            }
            "phone_open_app" => {
                let udid = self.ready().await?;
                let Some(name) = p["name"].as_str().map(str::to_string) else {
                    return Err(
                        "phone_open_app takes name, the app's name as on the Home Screen.".into(),
                    );
                };
                let source = self.source()?;
                let opened = self.blocking(move || source.open_app(&udid, &name)).await?;
                text(format!("Opened {opened}. Take a screenshot to see it."))
            }
            "phone_release" => {
                let thread = self.call.thread.clone();
                self.this
                    .update(self.cx, |p, cx| {
                        p.lease.release(&thread);
                        p.last = None;
                        cx.notify();
                    })
                    .map_err(gone)?;
                text("Let go of the iPhone.")
            }
            "phone_apps" => {
                let udid = self.ready().await?;
                let source = self.source()?;
                let apps = self.blocking(move || source.apps(&udid)).await?;
                let lines: Vec<String> = apps
                    .iter()
                    .map(|a| {
                        format!(
                            "{} — {} {}{}",
                            a.name,
                            a.bundle,
                            a.version,
                            if a.file_sharing {
                                " [file sharing]"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect();
                text(format!("{} apps:\n{}", lines.len(), lines.join("\n")))
            }
            "phone_install" => {
                let path = local_path(&p, "path")?;
                let udid = self.ready().await?;
                let source = self.source()?;
                let shown = path.display().to_string();
                self.blocking(move || source.install(&udid, &path)).await?;
                text(format!(
                    "Installed {shown}. A development-signed app opens only once its developer is trusted on the iPhone (Settings › General › VPN & Device Management)."
                ))
            }
            "phone_files" => {
                let path = p["path"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| if app.is_some() { "/Documents" } else { "/" }.to_string());
                let udid = self.ready().await?;
                let source = self.source()?;
                let shown = path.clone();
                let entries = self
                    .blocking(move || source.list(&udid, &path, app.as_deref()))
                    .await?;
                let lines: Vec<String> = entries
                    .iter()
                    .map(|e| {
                        if e.dir {
                            format!("{}/", e.name)
                        } else {
                            format!("{}  {}", e.name, files::human_size(e.size))
                        }
                    })
                    .collect();
                text(if lines.is_empty() {
                    format!("{shown} is empty.")
                } else {
                    format!("{shown}:\n{}", lines.join("\n"))
                })
            }
            "phone_push_file" => {
                let (local, remote) = (local_path(&p, "local")?, s("remote"));
                let udid = self.ready().await?;
                let source = self.source()?;
                let shown = remote.clone();
                let n = self
                    .blocking(move || source.push(&udid, &local, &remote, app.as_deref()))
                    .await?;
                text(format!("Copied {} to {shown}.", files::human_size(n)))
            }
            "phone_pull_file" => {
                let (remote, local) = (s("remote"), local_path(&p, "local")?);
                let udid = self.ready().await?;
                let source = self.source()?;
                let shown = local.display().to_string();
                let n = self
                    .blocking(move || source.pull(&udid, &remote, &local, app.as_deref()))
                    .await?;
                text(format!("Copied {} to {shown}.", files::human_size(n)))
            }
            "phone_remove_file" => {
                let path = s("path");
                let udid = self.ready().await?;
                let source = self.source()?;
                let shown = path.clone();
                self.blocking(move || source.remove(&udid, &path, app.as_deref()))
                    .await?;
                text(format!("Removed {shown}."))
            }
            other => Err(format!("Unknown iPhone tool {other}")),
        }
    }

    /// Ask the user, on the thread's card, when the access level says to.
    async fn ask(&mut self, kind: Sensitive, method: &str, p: &Value) -> Result<(), String> {
        let thread = self.call.thread.clone();
        if !needs_asking(kind, self.call.access) {
            return Ok(());
        }
        let (allowed, workspace) = self
            .this
            .update(self.cx, |p, _| {
                (
                    p.allowed.contains(&(thread.clone(), kind)),
                    p.workspace.clone(),
                )
            })
            .map_err(gone)?;
        if allowed {
            return Ok(());
        }
        let (title, detail) = ask_text(method, p);
        let (request, rx) = workspace.update(self.cx, |ws, cx| {
            ws.browser_ask(&thread, title.clone(), detail, cx)
        });
        let started = Instant::now();
        let decision = loop {
            match rx.try_recv() {
                Ok(d) => break d,
                Err(async_channel::TryRecvError::Closed) => break Decision::Deny,
                Err(async_channel::TryRecvError::Empty) => {}
            }
            let pending =
                workspace.update(self.cx, |ws, _| ws.browser_ask_pending(&thread, &request));
            let took_over = self.check().is_err();
            if !pending || took_over || started.elapsed() > ASK_WAIT {
                workspace.update(self.cx, |ws, cx| ws.browser_withdraw(&thread, &request, cx));
                if took_over {
                    return Err(TOOK_OVER.into());
                }
                break Decision::Deny;
            }
            self.sleep(Duration::from_millis(200)).await;
        };
        match decision {
            Decision::Allow => Ok(()),
            Decision::AllowForSession => {
                let _ = self
                    .this
                    .update(self.cx, |p, _| p.allowed.insert((thread, kind)));
                Ok(())
            }
            Decision::Deny => Err(format!(
                "The user didn't allow it ({title}). Don't try another way round; ask them if it's needed."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would come with it.
    use super::{
        Gesture, HandHolding, SCREEN_DRAWN, Sensitive, ask_text, driver_type, gesture_for,
        local_path, needs_asking, on_picture, screen_picture, starting, tools,
    };
    use gpui_kit::{Bounds, point, px, size};
    use serde_json::json;

    /// The roles read from the phone's accessibility, named as the driver names its types on a
    /// Mac, so trek-mcp reads them back into the same words.
    #[test]
    fn roles_are_named_as_the_driver_names_them() {
        assert_eq!(driver_type("button", false), "Button");
        assert_eq!(driver_type("text", false), "StaticText");
        assert_eq!(driver_type("text field", false), "TextField");
        assert_eq!(driver_type("text field", true), "SecureTextField");
        assert_eq!(driver_type("tab bar", false), "TabBar");
        assert_eq!(driver_type("application", false), "Application");
    }

    /// A Pro Max's screen (1320×2868) is drawn from a copy no taller than the tab needs, in
    /// GPUI's colour order (blue first).
    #[test]
    fn the_screen_is_drawn_from_a_copy_no_bigger_than_it_needs() {
        let picture = image::RgbaImage::from_pixel(1320, 2868, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let drawn = screen_picture(&png).unwrap();
        let size = drawn.size(0);
        assert_eq!(size.height.0, SCREEN_DRAWN as i32);
        assert!((735..=737).contains(&size.width.0), "{size:?}");
        assert_eq!(&drawn.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        // A small one stays its size.
        let small = image::RgbaImage::from_pixel(390, 844, image::Rgba([0, 0, 0, 255]));
        let mut png = Vec::new();
        small
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let size = screen_picture(&png).unwrap().size(0);
        assert_eq!((size.width.0, size.height.0), (390, 844));
        assert!(screen_picture(b"not a picture").is_err());
    }

    /// The same rules as the Mac's iPhone tool (`panels/phone.rs`).
    #[test]
    fn approvals_follow_the_access_level() {
        use HandHolding::*;
        assert!(!needs_asking(Sensitive::Install, FullAccess));
        assert!(!needs_asking(Sensitive::Files, FullAccess));
        assert!(needs_asking(Sensitive::Install, Auto));
        assert!(!needs_asking(Sensitive::Files, Auto));
        for level in [Supervised, AutoAcceptEdits] {
            assert!(needs_asking(Sensitive::Install, level));
            assert!(needs_asking(Sensitive::Files, level));
        }
    }

    #[test]
    fn approval_cards_name_the_file_and_where_it_goes() {
        let (title, detail) = ask_text(
            "phone_push_file",
            &json!({"local": "C:\\Users\\me\\a.txt", "remote": "/Documents/a.txt", "app": "com.x"}),
        );
        assert_eq!(title, "Copy a.txt to your iPhone");
        assert_eq!(detail, "C:\\Users\\me\\a.txt → /Documents/a.txt in com.x");
        let (title, _) = ask_text("phone_install", &json!({"path": "C:\\b\\Notes.ipa"}));
        assert_eq!(title, "Install Notes.ipa on your iPhone");
        let (title, _) = ask_text("phone_remove_file", &json!({"path": "/Downloads/a.txt"}));
        assert_eq!(title, "Remove a.txt from your iPhone");
    }

    #[test]
    fn every_tool_a_pc_serves_has_a_strip_word() {
        for t in tools::PC_TOOL_NAMES {
            assert!(!starting(t).is_empty());
        }
        assert_eq!(starting("phone_install"), "Installing an app…");
    }

    /// A relative path would land in Trek's own folder, not the agent's project.
    #[test]
    fn an_agents_paths_on_this_pc_are_whole_ones() {
        assert!(local_path(&json!({"local": "C:\\x\\a.txt"}), "local").is_ok());
        let err = local_path(&json!({"local": "a.txt"}), "local").unwrap_err();
        assert!(err.contains("full path on this PC"), "{err}");
        assert!(local_path(&json!({}), "local").is_err());
    }

    /// Let go where it was pressed (within a few pixels), a press is a tap there; dragged, a
    /// swipe to where it was let go, held to the picture's edges.
    #[test]
    fn a_press_let_go_is_a_tap_or_a_swipe() {
        let area = Bounds::new(point(px(10.), px(20.)), size(px(300.), px(400.)));
        // 100×200 fits as 200×400: 60 to 260 across, 20 to 420 down.
        let at = |x: f32, y: f32| point(px(x), px(y));
        assert_eq!(
            gesture_for(area, (100., 200.), (0.5, 0.5), at(163., 222.), 150),
            Some(Gesture::Tap(0.5, 0.5))
        );
        assert_eq!(
            gesture_for(area, (100., 200.), (0.5, 0.75), at(160., 120.), 300),
            Some(Gesture::Swipe {
                from: (0.5, 0.75),
                to: (0.5, 0.25),
                ms: 300
            })
        );
        assert_eq!(
            gesture_for(area, (100., 200.), (0.5, 0.5), at(500., 220.), 300),
            Some(Gesture::Swipe {
                from: (0.5, 0.5),
                to: (1., 0.5),
                ms: 300
            })
        );
        assert_eq!(
            gesture_for(Bounds::default(), (100., 200.), (0.5, 0.5), at(0., 0.), 300),
            None
        );
    }

    /// A click on the Screen tab is a tap where it falls on the picture, drawn fitted and centred:
    /// in the bars beside it, it's none.
    #[test]
    fn a_click_is_a_place_on_the_fitted_picture() {
        let area = Bounds::new(point(px(10.), px(20.)), size(px(300.), px(400.)));
        // 100×200 fits as 200×400, 50 in from the left.
        assert_eq!(
            on_picture(area, (100., 200.), point(px(160.), px(220.))),
            Some((0.5, 0.5))
        );
        assert_eq!(
            on_picture(area, (100., 200.), point(px(60.), px(20.))),
            Some((0., 0.))
        );
        assert_eq!(
            on_picture(area, (100., 200.), point(px(30.), px(220.))),
            None
        );
        assert_eq!(
            on_picture(Bounds::default(), (100., 200.), point(px(0.), px(0.))),
            None
        );
    }
}
