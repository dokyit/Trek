//! iPhone on a PC: the user's real iPhone beside the chat, its apps and files, for them and for
//! agents. Windows reaches the phone's own services through Apple Mobile Device Service, as Apple
//! Devices does (`trek_phone::device`); seeing its screen and tapping it need Trek on a Mac for
//! now, so the picture area says so. Every state's words are `trek_phone::pc`'s, which
//! `phone_status` answers with too.
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
        Arc::new(made)
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
    _tasks: Vec<Task<()>>,
}

impl PhonePanel {
    pub fn new(workspace: Entity<Workspace>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            return;
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
        let made_up = match state {
            "real" => {
                self.source = Arc::new(Services);
                None
            }
            "ready" | "apps" | "files" | "documents" | "working" | "agent" => Some(MadeUp::ready()),
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
                "phone: unknown state {other:?} (ready|apps|files|documents|working|agent|service|stopped|none|trust|locked|failed|real)"
            ),
        };
        if let Some(made_up) = made_up {
            self.source = made_up;
        }
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
            .child(
                h_flex()
                    .id("phone-screen-note")
                    .test_support()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(px(fluent::control(radius::CONTROL)))
                    .bg(theme.foreground.opacity(0.04))
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Lucide::Monitor).xsmall().flex_shrink_0())
                    .child(div().flex_1().min_w_0().child(pc::SCREEN)),
            )
            .into_any_element()
    }

    /// Apps and Files, and what each can do.
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
            Tab::Apps => Button::new("phone-install")
                .small()
                .outline()
                .icon(Lucide::PackagePlus)
                .label("Install an app…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.install_app(cx))),
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
                .on_click(cx.listener(|this, _, _, cx| this.add_file(cx))),
        };
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(crate::ui::segmented(
                "phone-tab",
                vec![(Tab::Apps, count), (Tab::Files, "Files".to_string())],
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
                let holder = self
                    .this
                    .update(self.cx, |p, _| {
                        p.lease.holder(Instant::now()).map(|h| h.title.clone())
                    })
                    .map_err(gone)?;
                text(pc::status(&seen, assistive, holder.as_deref()))
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
    use super::{HandHolding, Sensitive, ask_text, local_path, needs_asking, starting, tools};
    use serde_json::json;

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
}
