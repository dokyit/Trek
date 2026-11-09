//! Trek Driver: taps, swipes and keys over the USB cable while the iPhone is unlocked, with no
//! Bluetooth pairing and no iPhone Mirroring; or over Wi-Fi with no cable, once the phone was
//! paired by cable (usbmuxd and Xcode both reach it over the network then).
//!
//! The driver is a small XCUITest (`ios/TrekDriver`) that serves HTTP on the phone's loopback.
//! `ios/scripts/install-device.sh` builds it into Trek's own iPhone app (a free Apple ID installs
//! at most three apps on a phone, so it doesn't take a fourth) and leaves the signed app and the
//! `.xctestrun` that starts it in [`dir`]. Trek starts it on demand with
//! `xcodebuild test-without-building` (the one way to start an XCUITest on iOS 27: a runner opened
//! with `devicectl` never starts serving), reaches its port through usbmuxd as Xcode's port
//! forwarding does ([`device::exchange`]), and stops it when the agent lets go. Each start gets a
//! fresh token the driver checks on every request, so another app on the phone can't use it.
//!
//! Over Wi-Fi usbmuxd connects to the phone's address on the network rather than its loopback,
//! so a start over Wi-Fi also hands the driver this Mac's address there (`TREK_DRIVER_PEERS`):
//! it then listens on the local network and serves only that address, with the token. A driver
//! built before that (no `wifi` in its `features`) is offered over the cable only. Whether the
//! phone is locked, which the cable's black picture tells over USB, comes from CoreDevice over
//! Wi-Fi ([`passcode_required`]).
//!
//! Points: the driver speaks in screen points; the tools and the panel in pixels of the cable's
//! picture, or fractions of it ([`to_points`]).
//!
//! The picture: a driver with `stream` in its `features` pushes the screen as it changes down
//! one long answer ([`Driver::watch`], read by [`Mjpeg`]); one without is asked for a screenshot
//! at a time ([`Driver::screenshot`]), and [`after_stream`] says which follows a stream that
//! ended. What's on the screen as elements comes from one with `elements`
//! ([`Driver::elements`]).
//!
//! An action that went out without its answer coming back (the connection dropped, or the
//! answer was late) is not a failure: it may have happened. [`Transport::post`] on the driver
//! says so ([`UNKNOWN`]) and nothing here sends it again.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::device;

/// The phone port the driver serves on (WebDriverAgent's is 8100; this one keeps clear of it).
pub const PORT: u16 = 8117;
/// The driver's protocol version (`/status` says it): 2 added `/path`, and the arrows and
/// repeats in `/key`; 3 `/stream` (the screen pushed, [`Driver::watch`]) and `/elements` (what's
/// on it, [`Driver::elements`]). Its `features` say so. A driver from before is still used,
/// with what it has: Trek's iPhone app on the phone is replaced only by its updater.
pub const VERSION: u64 = 3;
/// The oldest driver Trek still talks to.
pub const OLDEST: u64 = 1;
/// How long a start may take: installing Trek's iPhone app the first time is most of it.
pub const START_LIMIT: Duration = Duration::from_secs(120);
/// The driver stops itself after this long without a request (Trek gone, or forgot it).
const PHONE_IDLE_SECS: u64 = 15 * 60;
/// The script that builds and installs it, as the user runs it.
pub const INSTALL: &str = "ios/scripts/install-device.sh";

/// Where `install-device.sh` leaves the driver: with the phone updater's state.
pub fn dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/dev.trek.phone-update/driver")
}

/// The driver as `install-device.sh` left it.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub dir: PathBuf,
    /// Trek's iPhone app with the driver inside, signed for the phone.
    pub app: PathBuf,
    pub xctestrun: PathBuf,
    /// Trek's iPhone app's bundle id and build number in it.
    pub bundle: String,
    pub build: String,
    /// The phone it was signed for.
    pub device: String,
    /// When its signing lapses (seconds since 1970), if known.
    pub expires: Option<i64>,
    /// It serves over Wi-Fi when asked (`build-driver.sh` wrote `wifi` in its `features`).
    pub wifi: bool,
}

impl Install {
    /// Read `dir`'s `info` (`key=value` lines) and check the app and run are there.
    pub fn read(dir: &Path) -> Option<Install> {
        let info = std::fs::read_to_string(dir.join("info")).ok()?;
        let get = |key: &str| {
            info.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        };
        let install = Install {
            dir: dir.to_path_buf(),
            app: dir.join("Trek.app"),
            xctestrun: dir.join("TrekDriver.xctestrun"),
            bundle: get("bundle"),
            build: get("build"),
            device: get("device"),
            expires: get("expires").parse().ok().filter(|&e| e > 0),
            wifi: std::fs::read_to_string(dir.join("features"))
                .is_ok_and(|f| f.lines().any(|l| l.trim() == "wifi")),
        };
        (install.app.is_dir() && install.xctestrun.is_file() && !install.bundle.is_empty())
            .then_some(install)
    }
}

/// Whether the driver can be used with the phone `udid`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Avail {
    /// Nothing says this Mac builds Trek's iPhone app: the USB route isn't offered.
    #[default]
    None,
    /// Trek's iPhone app was installed from this Mac with `install-device.sh`, but without the
    /// driver (an older script), or the driver is for another phone or lapsed: why.
    Install(String),
    Ready,
}

/// What [`Avail`] the driver has: `built_here` when `install-device.sh` installed Trek's iPhone
/// app on this phone from this Mac (the phone updater's `installed` says which device); `usb`
/// when the phone is on the cable (else usbmuxd sees it over Wi-Fi only).
pub fn availability(
    install: Option<&Install>,
    udid: &str,
    built_here: bool,
    now: i64,
    usb: bool,
) -> Avail {
    let Some(install) = install else {
        return if built_here {
            Avail::Install("it isn't installed yet".into())
        } else {
            Avail::None
        };
    };
    if !install.device.is_empty() && install.device != udid {
        return Avail::Install("it was built for another iPhone".into());
    }
    if install.expires.is_some_and(|e| e <= now) {
        return Avail::Install("its 7-day signing lapsed".into());
    }
    if !usb && !install.wifi {
        return Avail::Install("the one on the iPhone works over the cable only".into());
    }
    Avail::Ready
}

/// Whether `install-device.sh` installed Trek's iPhone app on `udid` from this Mac.
pub fn built_here(udid: &str) -> bool {
    let installed = dir().with_file_name("installed");
    std::fs::read_to_string(installed)
        .is_ok_and(|s| s.lines().any(|l| l.strip_prefix("device=") == Some(udid)))
}

/// Whether the phone `udid` needs its passcode now (locked: XCUITest can't start until it's
/// unlocked), as CoreDevice tells it over Wi-Fi as well as the cable: `xcrun devicectl device info
/// lockState`, about 1.5 s. A screen that went dark within the passcode's grace period isn't
/// locked by this measure (and the driver starts then). Blocks; `None` when it can't tell.
pub fn passcode_required(udid: &str) -> Option<bool> {
    let out = std::env::temp_dir().join(format!("trek-lock-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut child = Command::new("/usr/bin/xcrun")
        .args(["devicectl", "device", "info", "lockState", "--quiet"])
        .args(["--device", udid, "--json-output"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let json = std::fs::read(&out).ok();
    let _ = std::fs::remove_file(&out);
    lock_state(&json?)
}

/// `passcodeRequired` from devicectl's JSON answer.
pub fn lock_state(json: &[u8]) -> Option<bool> {
    let v: Value = serde_json::from_slice(json).ok()?;
    if v["info"]["outcome"]
        .as_str()
        .is_some_and(|o| o != "success")
    {
        return None;
    }
    v["result"]["passcodeRequired"].as_bool()
}

/// Where the driver is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Off,
    /// `xcodebuild` is installing and starting it.
    Starting,
    Ready,
    /// It didn't start, or stopped on its own: what to do about it.
    Failed(String),
}

/// The width in pixels to ask `/screenshot` for so the picture's longest side is `edge`, for a
/// screen of `screen` points (upright or on its side).
pub fn shot_width(screen: (f64, f64), edge: u32) -> u32 {
    let (w, h) = screen;
    let width = if h > w && h > 0. {
        (edge as f64 * w / h).round() as u32
    } else {
        edge
    };
    width.max(1)
}

/// The size in pixels of the picture `/screenshot?w=shot_width(screen, edge)` answers with,
/// for a screen of `screen` points at `scale` pixels a point: scaled to that width as the
/// driver does it, or the screen's own size when that is no wider.
pub fn shot_size(screen: (f64, f64), scale: f64, edge: u32) -> (u32, u32) {
    let native = ((screen.0 * scale).round(), (screen.1 * scale).round());
    let width = shot_width(screen, edge) as f64;
    if native.0 <= width || native.0 < 1. {
        return (native.0.max(1.) as u32, native.1.max(1.) as u32);
    }
    let factor = width / native.0;
    (
        (native.0 * factor).round().max(1.) as u32,
        (native.1 * factor).round().max(1.) as u32,
    )
}

/// A fraction of the screen (0…1 each way) in the driver's points.
pub fn to_points(fraction: (f64, f64), screen: (f64, f64)) -> (f64, f64) {
    let r = |v: f64| (v * 100.).round() / 100.;
    (
        r(fraction.0.clamp(0., 1.) * screen.0),
        r(fraction.1.clamp(0., 1.) * screen.1),
    )
}

/// One HTTP/1.1 request for the driver, `Connection: close`.
pub fn request(method: &str, path: &str, token: &str, body: Option<&Value>) -> Vec<u8> {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut out = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nX-Trek-Token: {token}\r\nConnection: close\r\n"
    );
    if !body.is_empty() {
        out += "Content-Type: application/json\r\n";
    }
    out += &format!("Content-Length: {}\r\n\r\n{body}", body.len());
    out.into_bytes()
}

/// The status code and body of a driver answer.
pub fn response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Trek Driver's answer was cut short.")?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("Trek Driver's answer wasn't HTTP.")?;
    let body = raw[end + 4..].to_vec();
    let length = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse::<usize>().ok())?
    });
    if length.is_some_and(|n| body.len() < n) {
        return Err("Trek Driver's answer was cut short.".into());
    }
    Ok((status, body))
}

/// A copy of the installed `.xctestrun` that starts the driver: absolute paths (`__TESTROOT__` is
/// the folder it was in) and the driver's environment.
pub fn xctestrun(template: &[u8], root: &Path, env: &[(&str, String)]) -> Result<Vec<u8>, String> {
    let mut plist: plist::Value =
        plist::from_bytes(template).map_err(|e| format!("The driver's .xctestrun: {e}"))?;
    let root = root.to_string_lossy().to_string();
    let target = plist
        .as_dictionary_mut()
        .and_then(|d| d.get_mut("TrekDriver"))
        .and_then(|t| t.as_dictionary_mut())
        .ok_or("The driver's .xctestrun has no TrekDriver.")?;
    for (_, value) in target.iter_mut() {
        match value {
            plist::Value::String(s) => *s = s.replace("__TESTROOT__", &root),
            plist::Value::Array(items) => {
                for item in items {
                    if let plist::Value::String(s) = item {
                        *s = s.replace("__TESTROOT__", &root);
                    }
                }
            }
            _ => {}
        }
    }
    if !target.contains_key("TestingEnvironmentVariables") {
        let empty = plist::Value::Dictionary(plist::Dictionary::new());
        target.insert("TestingEnvironmentVariables".into(), empty);
    }
    let vars = target
        .get_mut("TestingEnvironmentVariables")
        .and_then(|v| v.as_dictionary_mut())
        .ok_or("The driver's .xctestrun has a strange environment.")?;
    for (k, v) in env {
        vars.insert((*k).to_string(), plist::Value::String(v.clone()));
    }
    let mut out = vec![];
    plist::to_writer_xml(&mut out, &plist).map_err(|e| e.to_string())?;
    Ok(out)
}

/// What `xcodebuild`'s log says went wrong, as the one thing to do.
pub fn why(log: &str) -> String {
    let has = |s: &str| log.contains(s);
    if has("Unlock") && has("to Continue") {
        "Unlock the iPhone: Trek Driver starts only while it's unlocked.".into()
    } else if has("Developer Mode") && (has("disabled") || has("not enabled") || has("turn on")) {
        "Turn on Developer Mode on the iPhone (Settings › Privacy & Security › Developer Mode)."
            .into()
    } else if has("UI Automation") || has("UI automation") {
        "Turn on Enable UI Automation on the iPhone (Settings › Developer), then try again.".into()
    } else if has("profile has expired") || has("provisioning profile") && has("expired") {
        format!("Trek Driver's 7-day signing lapsed: run {INSTALL} again.")
    } else if has("not been explicitly trusted")
        || has("Untrusted Developer")
        || has("invalid code signature")
    {
        "Trust the developer on the iPhone: Settings › General › VPN & Device Management.".into()
    } else if has("maximum number of installed apps") {
        "The iPhone has as many apps from a free Apple ID as it takes (three).".into()
    } else if has("Unable to find a destination matching") {
        // Seen when Xcode doesn't reach the phone: over Wi-Fi, it was never paired by cable, or
        // it's on another network.
        "Xcode can't reach the iPhone: plug it in once (Xcode then reaches it over Wi-Fi too), or put both on the same Wi-Fi.".into()
    } else {
        let line = log
            .lines()
            .rev()
            .find(|l| {
                let l = l.trim();
                l.starts_with("xcodebuild: error")
                    || l.contains("Error Domain=")
                    || l.starts_with("Failure Reason:")
                    || l.starts_with("Testing failed:")
            })
            .map(|l| l.trim().chars().take(240).collect::<String>());
        match line {
            Some(l) => format!("Trek Driver didn't start: {l}"),
            None => "Trek Driver didn't start (xcodebuild ended without saying why).".into(),
        }
    }
}

/// One step of a tool call over the driver.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Post(&'static str, Value),
    Wait(u64),
}

/// What carries steps to the driver (the real one, or a fake in tests).
pub trait Transport {
    fn post(&self, path: &str, body: &Value) -> Result<Value, String>;
}

/// Play `steps`, stopping at the first error.
pub fn run(transport: &dyn Transport, steps: &[Step]) -> Result<(), String> {
    for step in steps {
        match step {
            Step::Post(path, body) => {
                transport.post(path, body)?;
            }
            Step::Wait(ms) => std::thread::sleep(Duration::from_millis(*ms)),
        }
    }
    Ok(())
}

/// A key as the driver names it: the hardware buttons, Spotlight, or a key it types.
pub fn key_name(key: &str) -> String {
    let plain = key.trim().to_ascii_lowercase().replace(' ', "");
    match plain.as_str() {
        "cmd+space" | "command+space" | "meta+space" => "spotlight".into(),
        "return" => "enter".into(),
        "delete" => "backspace".into(),
        "esc" => "escape".into(),
        "volumeup" | "volume+" => "volume_up".into(),
        "volumedown" | "volume-" => "volume_down".into(),
        _ if key.chars().count() == 1 => key.to_string(),
        _ => plain,
    }
}

/// The steps for a phone tool over the driver. `point` turns a pair of the call's keys (pixels in
/// the latest screenshot) into screen points; `bundle` finds an app's bundle id by its name.
pub fn plan(
    method: &str,
    p: &Value,
    point: &dyn Fn(&str, &str) -> Result<(f64, f64), String>,
    bundle: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<Step>, String> {
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    Ok(match method {
        "phone_tap" => {
            let (x, y) = point("x", "y")?;
            let hold = p["hold_ms"].as_u64().unwrap_or(80).min(10_000);
            vec![Step::Post(
                "/tap",
                json!({ "x": x, "y": y, "hold_ms": hold }),
            )]
        }
        "phone_swipe" => {
            let (x1, y1) = point("x1", "y1")?;
            let (x2, y2) = point("x2", "y2")?;
            let ms = p["duration_ms"].as_u64().unwrap_or(300).clamp(60, 5000);
            vec![Step::Post(
                "/swipe",
                json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2, "duration_ms": ms }),
            )]
        }
        "phone_type" => {
            let text = s("text");
            let mut steps = vec![];
            if !text.is_empty() {
                steps.push(Step::Post("/type", json!({ "text": text })));
            }
            if p["submit"].as_bool() == Some(true) {
                steps.push(Step::Post("/key", json!({ "key": "enter" })));
            }
            steps
        }
        "phone_key" => vec![Step::Post("/key", json!({ "key": key_name(&s("key")) }))],
        "phone_home" => vec![Step::Post("/key", json!({ "key": "home" }))],
        "phone_open_app" => {
            let name = s("name");
            let name = name.trim();
            if name.is_empty() {
                return Err("Which app? Give its name.".into());
            }
            match bundle(name) {
                Some(id) => vec![Step::Post("/open", json!({ "bundle": id }))],
                // Not one the phone listed: as a person would, through Spotlight.
                None => vec![
                    Step::Post("/key", json!({ "key": "spotlight" })),
                    Step::Wait(700),
                    Step::Post("/type", json!({ "text": name })),
                    Step::Wait(1000),
                    Step::Post("/key", json!({ "key": "enter" })),
                ],
            }
        }
        other => return Err(format!("{other} doesn't go through Trek Driver.")),
    })
}

/// The bundle id of the app called `name` (as on the Home Screen, any case), or `name` itself
/// when it's a bundle id the phone has.
pub fn find_app(apps: &[device::App], name: &str) -> Option<String> {
    let want = name.trim().to_lowercase();
    apps.iter()
        .find(|a| a.bundle.to_lowercase() == want)
        .or_else(|| apps.iter().find(|a| a.name.to_lowercase() == want))
        .map(|a| a.bundle.clone())
}

/// A fresh, unguessable-enough token for one start (std only: the hasher's random keys, the time
/// and the process).
fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out += &format!("{:016x}", h.finish());
    }
    out
}

struct Run {
    child: Child,
    udid: String,
    token: String,
    work: PathBuf,
    /// Started over Wi-Fi: it serves the local network too (over the cable, the loopback only).
    wifi: bool,
}

#[derive(Default)]
struct Inner {
    state: State,
    run: Option<Run>,
    /// The screen in points, from `/status`.
    screen: Option<(f64, f64)>,
    /// How many pixels a point is on it, from `/status` (3 on most iPhones).
    scale: f64,
    /// What the driver running says it does, from `/status` (`path`, `keys`, `stream`,
    /// `elements`).
    features: Vec<String>,
    used: Option<Instant>,
}

/// The driver on the phone, started and stopped from the Mac. Shared by the panel and the calls
/// under way (`Arc<Driver>`); every method blocks, so call them off the UI thread (except
/// [`Driver::state`], which only looks).
#[derive(Default)]
pub struct Driver {
    inner: Mutex<Inner>,
    /// Held while one start is under way: a second caller waits for it.
    starting: Mutex<()>,
}

impl Driver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where it is now; a run whose `xcodebuild` ended is noticed here.
    pub fn state(&self) -> State {
        let mut inner = self.inner.lock().unwrap();
        let ended = inner
            .run
            .as_mut()
            .is_some_and(|r| !matches!(r.child.try_wait(), Ok(None)));
        if ended && inner.state != State::Starting {
            let run = inner.run.take();
            inner.screen = None;
            inner.state = match inner.state {
                // It stopped on its own (the phone locked, unplugged, or it sat idle).
                State::Ready => State::Off,
                ref other => other.clone(),
            };
            drop(inner);
            if let Some(run) = run {
                cleanup(&run);
            }
            return self.inner.lock().unwrap().state.clone();
        }
        inner.state.clone()
    }

    /// How long since a call last used it.
    pub fn idle(&self) -> Option<Duration> {
        self.inner.lock().unwrap().used.map(|t| t.elapsed())
    }

    /// The screen in points while it runs.
    pub fn screen(&self) -> Option<(f64, f64)> {
        self.inner.lock().unwrap().screen
    }

    /// Whether the driver running does `feature` (`path`: `/path`; `keys`: the arrows and repeats
    /// in `/key`), as its `/status` said.
    pub fn takes(&self, feature: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .features
            .iter()
            .any(|f| f == feature)
    }

    /// While it runs (or starts): whether it was started over Wi-Fi, so it answers there too. One
    /// started over the cable serves the phone's loopback only, which Wi-Fi can't reach.
    pub fn wifi(&self) -> Option<bool> {
        self.inner.lock().unwrap().run.as_ref().map(|r| r.wifi)
    }

    /// Running and answering on `udid`, started if need be (`limit` for a start): the screen in
    /// points.
    pub fn ensure(
        &self,
        install: &Install,
        udid: &str,
        limit: Duration,
    ) -> Result<(f64, f64), String> {
        let _one = self.starting.lock().unwrap();
        let link = device::link(udid)?
            .ok_or("No iPhone is connected: plug it in, unlock it and trust this Mac.")?; // words: ok, the iPhone tool is macOS only
        let wifi = matches!(link, device::Link::Network(_));
        if self.state() == State::Ready {
            // One started over Wi-Fi answers on the cable too (its loopback); not the other way.
            let same = self
                .inner
                .lock()
                .unwrap()
                .run
                .as_ref()
                .is_some_and(|r| r.udid == udid && (r.wifi || !wifi));
            if same && let Ok(screen) = self.status() {
                return Ok(screen);
            }
        }
        self.stop_now();
        self.start(install, udid, link, limit)
    }

    fn start(
        &self,
        install: &Install,
        udid: &str,
        link: device::Link,
        limit: Duration,
    ) -> Result<(f64, f64), String> {
        // Over Wi-Fi: this Mac's address on the phone's network, the one the driver serves.
        let peer = match link {
            device::Link::Usb => None,
            device::Link::Network(_) if !install.wifi => {
                return Err(self.fail(format!(
                    "Trek Driver on the iPhone works over the cable only: run {INSTALL} again to tap over Wi-Fi."
                )));
            }
            device::Link::Network(ip) => match device::local_address_toward(ip) {
                Some(peer) => Some(peer),
                None => {
                    return Err(self.fail(format!(
                        "Couldn't tell this Mac's address on the iPhone's network ({ip}): put both on the same Wi-Fi." // words: ok, the iPhone tool is macOS only
                    )));
                }
            },
        };
        // xcodebuild installs the driver's copy of Trek's iPhone app first: never an older one
        // over a newer (the phone updater's, from a source without the driver).
        if let Ok(apps) = device::apps(Some(udid))
            && let Some(app) = apps.iter().find(|a| a.bundle == install.bundle)
            && !install.build.is_empty()
            && app.build != install.build
        {
            return Err(self.fail(format!(
                "Trek's iPhone app is build {} but Trek Driver was built with {}: run {INSTALL} again.",
                app.build, install.build
            )));
        }
        let work = std::env::temp_dir().join(format!("trek-driver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)
            .map_err(|e| format!("Couldn't make {}: {e}", work.display()))?;
        let token = token();
        let template = std::fs::read(&install.xctestrun)
            .map_err(|e| format!("Couldn't read {}: {e}", install.xctestrun.display()))?;
        let mut env = vec![
            ("TREK_DRIVER", "1".into()),
            ("TREK_DRIVER_PORT", PORT.to_string()),
            ("TREK_DRIVER_TOKEN", token.clone()),
            ("TREK_DRIVER_IDLE", PHONE_IDLE_SECS.to_string()),
        ];
        if let Some(peer) = peer {
            env.push(("TREK_DRIVER_PEERS", peer.to_string()));
        }
        let run = xctestrun(&template, &install.dir, &env)?;
        let xctestrun_path = work.join("run.xctestrun");
        std::fs::write(&xctestrun_path, run).map_err(|e| e.to_string())?;
        let log = work.join("xcodebuild.log");
        let out = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let child = Command::new("/usr/bin/xcrun")
            .args(["xcodebuild", "test-without-building", "-xctestrun"])
            .arg(&xctestrun_path)
            .args(["-destination", &format!("id={udid}")])
            .arg("-only-testing:TrekDriver/Drive")
            .arg("-resultBundlePath")
            .arg(work.join("run.xcresult"))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .map_err(|e| {
                format!("Couldn't run xcodebuild (Xcode is needed for Trek Driver): {e}")
            })?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.state = State::Starting;
            inner.screen = None;
            inner.run = Some(Run {
                child,
                udid: udid.to_string(),
                token,
                work,
                wifi: peer.is_some(),
            });
        }
        let started = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let exited = {
                let mut inner = self.inner.lock().unwrap();
                inner
                    .run
                    .as_mut()
                    .map(|r| !matches!(r.child.try_wait(), Ok(None)))
                    .unwrap_or(true)
            };
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            // Locked: xcodebuild would wait for the unlock without end.
            let locked = text.contains("Unlock") && text.contains("to Continue");
            if exited || locked {
                return Err(self.fail(why(&text)));
            }
            if text.contains("TrekDriver: serving") || started.elapsed() > Duration::from_secs(3) {
                if let Ok(screen) = self.status() {
                    let mut inner = self.inner.lock().unwrap();
                    inner.state = State::Ready;
                    inner.used = Some(Instant::now());
                    return Ok(screen);
                }
            }
            if started.elapsed() > limit {
                return Err(self.fail(format!(
                    "Trek Driver didn't start within {} s; its log: {}",
                    limit.as_secs(),
                    log.display()
                )));
            }
        }
    }

    fn fail(&self, why: String) -> String {
        self.stop_now();
        self.inner.lock().unwrap().state = State::Failed(why.clone());
        why
    }

    /// `/status`: the screen in points.
    fn status(&self) -> Result<(f64, f64), String> {
        let v = self.send("GET", "/status", None, Duration::from_secs(5))?;
        if v["version"].as_u64().is_none_or(|n| n < OLDEST) {
            return Err(format!(
                "The iPhone has another Trek Driver (version {}): run {INSTALL} again.",
                v["version"]
            ));
        }
        let screen = (
            v["width"].as_f64().unwrap_or(0.),
            v["height"].as_f64().unwrap_or(0.),
        );
        if screen.0 < 1. || screen.1 < 1. {
            return Err("Trek Driver didn't say the screen's size.".into());
        }
        let mut inner = self.inner.lock().unwrap();
        inner.screen = Some(screen);
        inner.scale = v["scale"].as_f64().filter(|s| *s >= 1.).unwrap_or(3.);
        inner.features = features(&v);
        Ok(screen)
    }

    /// The size in pixels of the picture [`Driver::screenshot`] answers with when asked for a
    /// longest side of `edge` (what `phone_screenshot` does), while it runs: x/y of the tools
    /// are pixels of it.
    pub fn shot_size(&self, edge: u32) -> Option<(u32, u32)> {
        let inner = self.inner.lock().unwrap();
        Some(shot_size(inner.screen?, inner.scale, edge))
    }

    /// One request to the running driver; its JSON answer, or its error.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        limit: Duration,
    ) -> Result<Value, String> {
        let (status, body) = self.fetch(method, path, body, limit).map_err(|f| f.why)?;
        if status != 200 {
            return Err(Self::said(status, &body));
        }
        Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// `GET /screenshot`: the screen as a JPEG of `quality` percent, at most `max_width` pixels
    /// wide (0: as it is). An older driver answers with a full-size PNG; `image` reads both.
    pub fn screenshot(&self, max_width: u32, quality: u8) -> Result<Vec<u8>, String> {
        let path = format!("/screenshot?w={max_width}&q={quality}");
        let (status, body) = self
            .fetch("GET", &path, None, Duration::from_secs(15))
            .map_err(|f| f.why)?;
        if status != 200 {
            return Err(Self::said(status, &body));
        }
        if body.is_empty() {
            return Err("Trek Driver sent no picture.".into());
        }
        Ok(body)
    }

    /// One request to the running driver: the status code and body of its answer; or why none
    /// came, and whether the request had gone out by then.
    fn fetch(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        limit: Duration,
    ) -> Result<(u16, Vec<u8>), device::Failed> {
        let (udid, token) = self
            .running()
            .map_err(|why| device::Failed { sent: false, why })?;
        let raw = device::exchange_told(
            Some(&udid),
            PORT,
            &request(method, path, &token, body),
            limit,
        )?;
        // An answer that isn't whole came after the request went.
        let (status, body) = response(&raw).map_err(|why| device::Failed { sent: true, why })?;
        if status == 200 {
            self.inner.lock().unwrap().used = Some(Instant::now());
        }
        Ok((status, body))
    }

    /// The phone and token of the run under way.
    fn running(&self) -> Result<(String, String), String> {
        let inner = self.inner.lock().unwrap();
        let run = inner.run.as_ref().ok_or("Trek Driver isn't running.")?;
        Ok((run.udid.clone(), run.token.clone()))
    }

    /// `GET /elements`: what's on the screen, as the driver lists it (see
    /// `ios/TrekDriver/Driver/Elements.swift`): `elements` (each with `type`, `label`,
    /// `identifier`, `value`, `frame` in points as `[x, y, width, height]`, `enabled`,
    /// `selected`, `focused`, `depth`), the screen's `width` and `height` in points, the `app`
    /// in front, `more` (elements past `max`) and `partial` (its time, `limit`, ran out).
    pub fn elements(&self, max: u32, limit: Duration) -> Result<Value, String> {
        if !self.takes("elements") {
            return Err(NO_ELEMENTS.into());
        }
        let path = format!("/elements?max={max}&ms={}", limit.as_millis());
        self.send("GET", &path, None, limit + Duration::from_secs(10))
    }

    /// `GET /stream`: the screen as the driver pushes it, each frame a JPEG handed to `on` as it
    /// comes (and [`Picture::Still`] about once a second while the screen doesn't change, or
    /// more often while nothing comes at all), until `on` returns false. Blocks for as long;
    /// says how it ended. A driver without `stream` is [`StreamEnd::Refused`] at once.
    pub fn watch(&self, options: &StreamOptions, on: &mut dyn FnMut(Picture) -> bool) -> StreamEnd {
        if !self.takes("stream") {
            return StreamEnd::Refused(404, "This Trek Driver has no /stream.".into());
        }
        let (udid, token) = match self.running() {
            Ok(run) => run,
            Err(why) => return StreamEnd::Dropped(why),
        };
        let request = request("GET", &options.path(), &token, None);
        read_stream(
            |sink| device::stream(Some(&udid), PORT, &request, STREAM_QUIET, sink),
            &mut |picture| {
                // Watched is used: the Mac doesn't stop a driver somebody is looking through.
                self.inner.lock().unwrap().used = Some(Instant::now());
                on(picture)
            },
        )
    }

    /// What a driver answer other than 200 means.
    fn said(status: u16, body: &[u8]) -> String {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let msg = v["error"].as_str().unwrap_or("it failed").to_string();
        match status {
            401 => "Another Trek Driver answered (a different token): try again.".into(),
            409 => msg,
            _ => format!("Trek Driver: {msg}"),
        }
    }

    /// Stop it: asked nicely, then `xcodebuild` ended. Blocks a few seconds at most.
    pub fn stop(&self) {
        let _one = self.starting.lock().unwrap();
        self.stop_now();
    }

    fn stop_now(&self) {
        let has_run = self.inner.lock().unwrap().run.is_some();
        if has_run {
            let _ = self.send("POST", "/stop", None, Duration::from_secs(3));
        }
        let run = {
            let mut inner = self.inner.lock().unwrap();
            inner.screen = None;
            inner.features.clear();
            if inner.state != State::Off && !matches!(inner.state, State::Failed(_)) {
                inner.state = State::Off;
            }
            inner.run.take()
        };
        if let Some(mut run) = run {
            // The test returns after /stop and xcodebuild ends by itself in a moment.
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && matches!(run.child.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if matches!(run.child.try_wait(), Ok(None)) {
                let _ = run.child.kill();
                let _ = run.child.wait();
            }
            cleanup(&run);
        }
    }
}

/// What a driver's `/status` answer says it does: its `features`, and what its version implies
/// (version 2 came with `/path` and the keys).
pub fn features(status: &Value) -> Vec<String> {
    let mut out: Vec<String> = status["features"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if status["version"].as_u64().is_some_and(|v| v >= 2) {
        for f in ["path", "keys"] {
            if !out.iter().any(|o| o == f) {
                out.push(f.into());
            }
        }
    }
    out
}

/// How long the driver may take to answer `body`: the gesture's own length (a press held, a
/// path's last point), and each key typed, on top of the usual wait.
pub fn answer_limit(body: &Value) -> Duration {
    let chars = body["text"].as_str().map_or(0, |t| t.chars().count()) as u64;
    let keys = body["count"].as_u64().unwrap_or(1).min(100);
    let gesture = body["hold_ms"].as_u64().unwrap_or(0)
        + body["duration_ms"].as_u64().unwrap_or(0)
        + body["points"]
            .as_array()
            .and_then(|p| p.last())
            .and_then(|p| p["t_ms"].as_u64())
            .unwrap_or(0);
    Duration::from_millis(15_000 + chars * 150 + keys * 150 + gesture.min(60_000))
}

impl Transport for Driver {
    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        // Typing waits for each key; a long text gets longer, and so does a long press or drag.
        let (status, answer) = self
            .fetch("POST", path, Some(body), answer_limit(body))
            // Gone out but not answered: it may have happened. Said so, and never sent again
            // here: whoever asked looks at the screen and decides.
            .map_err(|f| if f.sent { unknown(path, &f.why) } else { f.why })?;
        if status != 200 {
            return Err(Self::said(status, &answer));
        }
        Ok(serde_json::from_slice(&answer).unwrap_or(Value::Null))
    }
}

/// How an error begins when a tap, swipe or key went out to the phone and only its answer was
/// lost (the connection dropped, or the answer didn't come in time): it may have happened.
pub const UNKNOWN: &str = "Outcome unknown:";

/// The error for `path`'s request that went out without an answer (`why`).
pub fn unknown(path: &str, why: &str) -> String {
    let what = match path {
        "/tap" => "The tap",
        "/swipe" | "/path" => "The swipe",
        "/type" => "The text",
        "/key" => "The key",
        "/open" => "Opening the app",
        _ => "The request",
    };
    format!(
        "{UNKNOWN} {what} went out to the iPhone but its answer didn't come back ({}). It may or may not have happened, and wasn't sent again: look at the screen (phone_elements or phone_screenshot) before deciding to repeat it.",
        why.trim_end_matches('.')
    )
}

/// What a driver without `/elements` (or a route without the driver) says to an agent asking
/// for elements.
pub const NO_ELEMENTS: &str = "Elements need a newer Trek Driver on the iPhone than the one running (it has no /elements): use phone_screenshot and x/y until Trek's iPhone app is updated.";

/// How long the stream may bring nothing (not even word that the screen is still, which the
/// driver sends every second) before the connection counts as dead.
pub const STREAM_QUIET: Duration = Duration::from_secs(5);

/// What the Mac asks of `/stream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamOptions {
    /// While the screen moves, frames at most this many pixels wide (0: the screen's own
    /// size); the driver gets there by halving, which costs it little.
    pub width: u32,
    /// The JPEG's quality, 1–100.
    pub quality: u8,
    /// Frames a second at most.
    pub fps: u32,
}

impl StreamOptions {
    /// Over the cable: every frame at the screen's own size (the phone encodes it once and
    /// nothing more; the cable carries it easily).
    pub const CABLE: StreamOptions = StreamOptions {
        width: 0,
        quality: 70,
        fps: 30,
    };
    /// Over Wi-Fi: half-size frames while the screen moves (a third of the bytes), then a
    /// full-size one when it rests.
    pub const WIFI: StreamOptions = StreamOptions {
        width: 900,
        quality: 60,
        fps: 24,
    };

    /// The request path for them.
    pub fn path(&self) -> String {
        format!(
            "/stream?w={}&q={}&fps={}",
            self.width, self.quality, self.fps
        )
    }
}

/// What the panel does about its picture once a stream has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Nothing more: it was stopped.
    Stop,
    /// Ask for one screenshot at a time, as before `/stream`: for this long, then try the stream
    /// again; for good when this driver hasn't one.
    Poll(Option<Duration>),
    /// Connect again after this long (the driver checked first, and started if it went away).
    Again(Duration),
    /// It keeps dropping without a frame: ask for one screenshot, to tell a stream that brings
    /// nothing on this iPhone (screenshots work: use them for [`stream_rest`]) from an iPhone
    /// that shows nothing now (locked, gone: wait and connect again).
    Doubt,
}

/// How long screenshots are asked for while as many as the driver takes watch its stream
/// already, before the stream is tried again.
const STREAM_FULL_WAIT: Duration = Duration::from_secs(10);

/// What follows a stream that ended as `end`, after `drops` drops in a row without a frame
/// between them: a driver that hasn't `/stream` is asked for screenshots instead (one whose
/// stream is full, for a while), and a connection that dropped (the phone locked, the cable
/// came out, the Wi-Fi address changed) is made again, a little later each time it fails,
/// until [`STREAM_GIVE_UP`] in a row put the stream itself in doubt.
pub fn after_stream(end: &StreamEnd, drops: u32) -> Next {
    match end {
        StreamEnd::Stopped => Next::Stop,
        StreamEnd::Refused(503, _) => Next::Poll(Some(STREAM_FULL_WAIT)),
        StreamEnd::Refused(..) => Next::Poll(None),
        StreamEnd::Dropped(_) if drops + 1 >= STREAM_GIVE_UP => Next::Doubt,
        StreamEnd::Dropped(_) => {
            Next::Again(Duration::from_millis((250u64 << drops.min(4)).min(3000)))
        }
    }
}

/// This many drops in a row with no frame between them, and the stream itself is in doubt
/// ([`Next::Doubt`]): the tool mustn't go without a picture for a stream that connects and
/// brings nothing, when screenshots (how the picture came before `/stream`) still work.
pub const STREAM_GIVE_UP: u32 = 4;

/// How long screenshots stand in for a stream that brought nothing while they worked, the
/// `times`th time in a row: a minute, then ten, then an hour, before it's tried again.
pub fn stream_rest(times: u32) -> Duration {
    Duration::from_secs(match times {
        0 | 1 => 60,
        2 => 600,
        _ => 3600,
    })
}

/// One thing the stream brought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    /// The screen changed: a JPEG of it.
    Frame(Vec<u8>),
    /// Nothing new: the screen is as the last frame showed it (or nothing has come for a
    /// moment).
    Still,
}

/// How a stream ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEnd {
    /// Whoever read it stopped it.
    Stopped,
    /// The driver wouldn't serve it: its status and what it said (404 from a driver without
    /// `/stream`, 503 when enough are watching already).
    Refused(u16, String),
    /// The connection couldn't be made, dropped, went quiet, or brought something that isn't
    /// the stream: why. Worth trying again.
    Dropped(String),
}

/// The driver's `/stream` answer, read as it arrives: `multipart/x-mixed-replace`, each part a
/// JPEG with its `Content-Length` (an empty part says the screen is still). Bytes go in as the
/// connection gives them, in pieces of any size; whole parts come out.
#[derive(Debug, Default)]
pub struct Mjpeg {
    buf: Vec<u8>,
    /// The answer's headers are read: what follows is parts.
    started: bool,
}

/// The most one frame may be: more is not a frame.
const MAX_FRAME: usize = 32 << 20;

impl Mjpeg {
    pub fn new() -> Self {
        Self::default()
    }

    /// More bytes of the answer: the parts they complete, in order. `Err` when the answer isn't
    /// a stream (`Refused`, once its body is whole) or stops making sense (`Dropped`).
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Picture>, StreamEnd> {
        self.buf.extend_from_slice(bytes);
        let mut out = vec![];
        if !self.started {
            let Some(end) = find(&self.buf, b"\r\n\r\n") else {
                return if self.buf.len() > 64 << 10 {
                    Err(StreamEnd::Dropped(
                        "Trek Driver's answer wasn't HTTP.".into(),
                    ))
                } else {
                    Ok(out)
                };
            };
            let head = String::from_utf8_lossy(&self.buf[..end]).to_string();
            let status = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .ok_or_else(|| StreamEnd::Dropped("Trek Driver's answer wasn't HTTP.".into()))?;
            if status != 200 {
                // An ordinary answer saying why not: whole by its Content-Length.
                if !device::answer_complete(&self.buf) {
                    return Ok(out);
                }
                let said = Driver::said(status, &self.buf[end + 4..]);
                return Err(StreamEnd::Refused(status, said));
            }
            if !header(&head, "content-type").is_some_and(|t| t.contains("multipart/")) {
                return Err(StreamEnd::Dropped(
                    "Trek Driver answered /stream with something else.".into(),
                ));
            }
            self.buf.drain(..end + 4);
            self.started = true;
        }
        loop {
            // Between parts: the line break that ended the last one.
            let skip = self
                .buf
                .iter()
                .take_while(|b| **b == b'\r' || **b == b'\n')
                .count();
            let rest = &self.buf[skip..];
            let Some(end) = find(rest, b"\r\n\r\n") else {
                if rest.len() > 16 << 10 {
                    return Err(StreamEnd::Dropped("The stream lost its place.".into()));
                }
                break;
            };
            let head = String::from_utf8_lossy(&rest[..end]);
            if !head.starts_with("--") {
                return Err(StreamEnd::Dropped("The stream lost its place.".into()));
            }
            let length = header(&head, "content-length")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n <= MAX_FRAME)
                .ok_or_else(|| StreamEnd::Dropped("A frame came without its size.".into()))?;
            let body = end + 4;
            if rest.len() < body + length {
                break;
            }
            out.push(if length == 0 {
                Picture::Still
            } else {
                Picture::Frame(rest[body..body + length].to_vec())
            });
            self.buf.drain(..skip + body + length);
        }
        Ok(out)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A header's value in `head` (the lines before the blank one), by its name in any case.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Read a stream: `pieces` runs the connection, handing each piece it reads to the sink it's
/// given (as [`device::stream`] does) until the sink returns false; each frame goes to `on`,
/// which returns whether to go on. A piece without bytes is a tick: `on` hears
/// [`Picture::Still`], so it can stop a stream that brings nothing.
pub fn read_stream(
    pieces: impl FnOnce(&mut dyn FnMut(&[u8]) -> bool) -> Result<(), String>,
    on: &mut dyn FnMut(Picture) -> bool,
) -> StreamEnd {
    let mut parser = Mjpeg::new();
    let mut end = None;
    let ran = pieces(&mut |bytes| {
        if bytes.is_empty() {
            if !on(Picture::Still) {
                end = Some(StreamEnd::Stopped);
            }
            return end.is_none();
        }
        match parser.push(bytes) {
            Ok(pictures) => {
                for picture in pictures {
                    if !on(picture) {
                        end = Some(StreamEnd::Stopped);
                        break;
                    }
                }
            }
            Err(e) => end = Some(e),
        }
        end.is_none()
    });
    match (end, ran) {
        (Some(end), _) => end,
        (None, Err(why)) => StreamEnd::Dropped(why),
        (None, Ok(())) => StreamEnd::Stopped,
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// The run's files go; its log stays in the system's temporary folder until the next start.
fn cleanup(run: &Run) {
    let _ = std::fs::remove_dir_all(run.work.join("run.xcresult"));
    let _ = std::fs::remove_file(run.work.join("run.xctestrun"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn fractions_become_points_of_the_screen() {
        // The cable's picture is 1320×2868 pixels; the screen 440×956 points (3×).
        let frac = (660. / 1320., 2868. / 2868.);
        assert_eq!(to_points(frac, (440., 956.)), (220., 956.));
        assert_eq!(to_points((0.25, 0.5), (440., 956.)), (110., 478.));
        // Outside the picture: clamped to its edge.
        assert_eq!(to_points((-0.1, 1.2), (440., 956.)), (0., 956.));
    }

    #[test]
    fn requests_and_answers_are_plain_http() {
        let raw = request("POST", "/tap", "abc", Some(&json!({"x": 1})));
        let text = String::from_utf8(raw).unwrap();
        assert!(text.starts_with("POST /tap HTTP/1.1\r\n"));
        assert!(text.contains("X-Trek-Token: abc\r\n"));
        assert!(text.contains("Content-Length: 7\r\n\r\n{\"x\":1}"));
        let (code, body) =
            response(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}").unwrap();
        assert_eq!((code, body.as_slice()), (200, &b"{\"ok\":true}"[..]));
        assert!(response(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{}").is_err());
        assert!(response(b"garbage").is_err());
    }

    #[test]
    fn a_drivers_status_says_what_it_does() {
        // The one on the phone before /path: version 1, no features.
        assert!(features(&json!({"ok": true, "version": 1, "width": 440})).is_empty());
        let now = features(&json!({"version": 2, "features": ["path", "keys"]}));
        assert_eq!(now, ["path", "keys"]);
        // A later one that names more keeps them.
        let later = features(&json!({"version": 3, "features": ["path", "keys", "pinch"]}));
        assert!(later.contains(&"pinch".to_string()));
        // The stream and the elements come only with a driver that names them: a version
        // alone promises neither.
        assert!(!now.iter().any(|f| f == "stream" || f == "elements"));
        assert!(!later.iter().any(|f| f == "stream" || f == "elements"));
        let v3 =
            features(&json!({"version": 3, "features": ["path", "keys", "stream", "elements"]}));
        assert!(v3.contains(&"stream".to_string()) && v3.contains(&"elements".to_string()));
    }

    #[test]
    fn the_driver_and_the_mac_agree_on_the_version() {
        let actions = include_str!("../../../ios/TrekDriver/Driver/Actions.swift");
        assert!(
            actions.contains(&format!("static let version = {VERSION}\n")),
            "Actions.version must be driver::VERSION"
        );
        for f in ["path", "keys", "stream", "elements"] {
            assert!(actions.contains(&format!("\"{f}\"")), "{f} in its features");
        }
    }

    /// A driver as its `/status` left it, with no run behind it.
    fn driver_taking(features: &[&str]) -> Driver {
        let d = Driver::new();
        d.inner.lock().unwrap().features = features.iter().map(|f| f.to_string()).collect();
        d
    }

    #[test]
    fn an_older_driver_is_asked_for_neither_the_stream_nor_elements() {
        // Version 2, as on a phone the updater hasn't reached yet: nothing is sent to it.
        let old = driver_taking(&["path", "keys"]);
        let mut frames = 0;
        let end = old.watch(&StreamOptions::CABLE, &mut |_| {
            frames += 1;
            true
        });
        assert!(matches!(end, StreamEnd::Refused(404, _)), "{end:?}");
        assert_eq!(frames, 0);
        // …so the picture is polled, as before, for good.
        assert_eq!(after_stream(&end, 0), Next::Poll(None));
        assert_eq!(
            old.elements(300, Duration::from_secs(5)).unwrap_err(),
            NO_ELEMENTS
        );
        // One that has them is asked (here it isn't running, which is all it can say).
        let new = driver_taking(&["path", "keys", "stream", "elements"]);
        let end = new.watch(&StreamOptions::CABLE, &mut |_| true);
        assert_eq!(end, StreamEnd::Dropped("Trek Driver isn't running.".into()));
        assert_eq!(
            new.elements(300, Duration::from_secs(5)).unwrap_err(),
            "Trek Driver isn't running."
        );
    }

    #[test]
    fn a_stream_that_ended_is_stopped_polled_or_made_again() {
        assert_eq!(after_stream(&StreamEnd::Stopped, 0), Next::Stop);
        // No such endpoint (or another driver's token): screenshots one at a time, for good.
        for status in [404, 401, 501] {
            let refused = StreamEnd::Refused(status, "no".into());
            assert_eq!(after_stream(&refused, 0), Next::Poll(None));
        }
        // Enough watching already: screenshots for a while, then the stream is tried again.
        let full = StreamEnd::Refused(503, "2 are watching the screen already".into());
        assert!(
            matches!(after_stream(&full, 0), Next::Poll(Some(wait)) if wait >= Duration::from_secs(5))
        );
        // Dropped: again, soon, then less eagerly.
        let dropped = StreamEnd::Dropped("The iPhone closed the connection.".into());
        let waits: Vec<u64> = (0..STREAM_GIVE_UP - 1)
            .map(|n| match after_stream(&dropped, n) {
                Next::Again(d) => d.as_millis() as u64,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(waits, [250, 500, 1000]);
        // And once it has dropped that often with no frame, the stream is in doubt: a
        // screenshot says whether it's the stream (screenshots stand in, longer each time) or
        // the iPhone.
        for n in STREAM_GIVE_UP - 1..STREAM_GIVE_UP + 4 {
            assert_eq!(after_stream(&dropped, n), Next::Doubt);
        }
        assert_eq!(
            [1, 2, 3, 9].map(|times| stream_rest(times).as_secs()),
            [60, 600, 3600, 3600]
        );
        assert_eq!(StreamOptions::CABLE.path(), "/stream?w=0&q=70&fps=30");
    }

    const STREAM_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=trekframe\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";

    /// One part, as `ScreenStream.part` writes it.
    fn part(jpeg: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "--trekframe\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            jpeg.len()
        )
        .into_bytes();
        out.extend_from_slice(jpeg);
        out.extend_from_slice(b"\r\n");
        out
    }

    const STILL: &[u8] =
        b"--trekframe\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n\r\n";

    /// The driver's ordinary answer saying no, as `Response.error` writes it.
    fn refusal(status: u16, error: &str) -> Vec<u8> {
        let body = json!({"error": error, "ok": false}).to_string();
        format!(
            "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// Bytes that look like the stream's own punctuation, to make sure a frame is taken by its
    /// length and not by what's in it.
    fn tricky() -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(b"\r\n\r\n--trekframe\r\nContent-Length: 3\r\n\r\nabc\r\n");
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn frames_come_out_whole_however_the_bytes_were_split() {
        let (a, b) = (tricky(), vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]);
        let mut answer = STREAM_HEAD.to_vec();
        answer.extend(part(&a));
        answer.extend_from_slice(STILL);
        answer.extend(part(&b));
        let want = [
            Picture::Frame(a.clone()),
            Picture::Still,
            Picture::Frame(b.clone()),
        ];
        // All at once.
        assert_eq!(Mjpeg::new().push(&answer).unwrap(), want);
        // A byte at a time.
        let mut parser = Mjpeg::new();
        let mut got = vec![];
        for byte in &answer {
            got.extend(parser.push(std::slice::from_ref(byte)).unwrap());
        }
        assert_eq!(got, want);
        // In two pieces, cut at every place: in the headers, inside a frame, between parts.
        for cut in 0..=answer.len() {
            let mut parser = Mjpeg::new();
            let mut got = parser.push(&answer[..cut]).unwrap();
            got.extend(parser.push(&answer[cut..]).unwrap());
            assert_eq!(got, want, "cut at {cut}");
        }
        // Part of a frame is no frame yet.
        let mut parser = Mjpeg::new();
        let half = STREAM_HEAD.len() + part(&a).len() - 10;
        assert_eq!(parser.push(&answer[..half]).unwrap(), []);
    }

    #[test]
    fn an_answer_that_isnt_the_stream_ends_it() {
        // A driver from before /stream: an ordinary 404, whole by its length.
        let no = refusal(404, "no /stream");
        let mut parser = Mjpeg::new();
        assert_eq!(parser.push(&no[..no.len() - 5]).unwrap(), []);
        assert_eq!(
            parser.push(&no[no.len() - 5..]).unwrap_err(),
            StreamEnd::Refused(404, "Trek Driver: no /stream".into())
        );
        // Enough are watching.
        let busy = refusal(503, "2 are watching the screen already");
        let end = Mjpeg::new().push(&busy).unwrap_err();
        assert!(matches!(&end, StreamEnd::Refused(503, why) if why.contains("watching")));
        // Not multipart, not HTTP, or a part without its size: dropped, to be tried again.
        let plain = b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: 2\r\n\r\nhi";
        assert!(matches!(
            Mjpeg::new().push(plain),
            Err(StreamEnd::Dropped(_))
        ));
        assert!(matches!(
            Mjpeg::new().push(b"SSH-2.0 hello\r\n\r\n"),
            Err(StreamEnd::Dropped(_))
        ));
        let mut sizeless = STREAM_HEAD.to_vec();
        sizeless.extend_from_slice(b"--trekframe\r\nContent-Type: image/jpeg\r\n\r\nabc");
        assert!(matches!(
            Mjpeg::new().push(&sizeless),
            Err(StreamEnd::Dropped(_))
        ));
        let mut lost = STREAM_HEAD.to_vec();
        lost.extend_from_slice(b"garbage\r\n\r\n");
        assert!(matches!(
            Mjpeg::new().push(&lost),
            Err(StreamEnd::Dropped(_))
        ));
    }

    /// A far end that takes the request, writes `script`'s pieces a moment apart and hangs up.
    fn serve(pieces: Vec<Vec<u8>>) -> std::net::SocketAddr {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            for piece in pieces {
                if s.write_all(&piece).is_err() {
                    return;
                }
                let _ = s.flush();
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        addr
    }

    fn watch_at(addr: std::net::SocketAddr, on: &mut dyn FnMut(Picture) -> bool) -> StreamEnd {
        let request = request("GET", "/stream?w=0&q=70&fps=30", "t", None);
        read_stream(
            |sink| device::stream_tcp(addr, &request, Duration::from_secs(5), sink),
            on,
        )
    }

    #[test]
    fn a_dropped_connection_ends_the_stream_after_the_frames_it_brought() {
        let (a, b) = (vec![0xFF, 0xD8, 7, 7, 0xFF, 0xD9], tricky());
        // The first frame split across two writes, the second cut off by the hang-up.
        let first = part(&a);
        let second = part(&b);
        let mut one = STREAM_HEAD.to_vec();
        one.extend_from_slice(&first[..20]);
        let mut two = first[20..].to_vec();
        two.extend_from_slice(&second[..second.len() - 8]);
        let addr = serve(vec![one, two]);
        let mut frames = vec![];
        let end = watch_at(addr, &mut |p| {
            if let Picture::Frame(jpeg) = p {
                frames.push(jpeg);
            }
            true
        });
        assert_eq!(frames, [a], "the whole frame, and not the half one");
        assert_eq!(
            end,
            StreamEnd::Dropped("The iPhone closed the connection.".into())
        );
        // Dropped is tried again.
        assert!(matches!(after_stream(&end, 0), Next::Again(_)));
    }

    #[test]
    fn the_reader_stops_a_stream_and_a_refusal_comes_through() {
        let mut answer = STREAM_HEAD.to_vec();
        for n in 0..5u8 {
            answer.extend(part(&[0xFF, 0xD8, n, 0xFF, 0xD9]));
        }
        let addr = serve(vec![answer, vec![0; 1]]);
        let mut seen = 0;
        let end = watch_at(addr, &mut |_| {
            seen += 1;
            seen < 2
        });
        assert_eq!((end, seen), (StreamEnd::Stopped, 2));
        let end = watch_at(serve(vec![refusal(404, "no /stream")]), &mut |_| true);
        assert_eq!(
            end,
            StreamEnd::Refused(404, "Trek Driver: no /stream".into())
        );
    }

    #[test]
    fn a_screenshots_size_is_known_before_its_taken() {
        // An iPhone 17: 402×874 points at 3 pixels a point; phone_screenshot's longest side is
        // 1200 unless asked otherwise. (What its driver answered in a simulator: 552×1200.)
        assert_eq!(shot_width((402., 874.), 1200), 552);
        assert_eq!(shot_size((402., 874.), 3., 1200), (552, 1200));
        // A Pro Max, where rounding leaves the long side a pixel short.
        assert_eq!(shot_size((440., 956.), 3., 1200), (552, 1199));
        // Asked for more than the screen has: the screen's own pixels.
        assert_eq!(shot_size((402., 874.), 3., 4000), (1206, 2622));
        // On its side, the width is the long one.
        assert_eq!(shot_size((874., 402.), 3., 1200), (1200, 552));
        // A point of the screen and the same point of that picture are one place: an element's
        // box goes out in the picture's pixels, and a tap comes back as a fraction of it.
        let (screen, picture) = ((402., 874.), (552., 1200.));
        let pixel = (201. * picture.0 / screen.0, 406.3 * picture.1 / screen.1);
        let back = to_points((pixel.0 / picture.0, pixel.1 / picture.1), screen);
        assert_eq!(back, (201., 406.3));
        // While it runs, the driver says its own.
        let d = driver_taking(&["elements"]);
        assert_eq!(d.shot_size(1200), None);
        {
            let mut inner = d.inner.lock().unwrap();
            inner.screen = Some((402., 874.));
            inner.scale = 3.;
        }
        assert_eq!(d.shot_size(1200), Some((552, 1200)));
    }

    #[test]
    fn an_action_that_went_out_unanswered_is_unknown_and_stops_what_follows() {
        let said = unknown("/tap", "The iPhone's port 8117 didn't answer in time.");
        assert!(said.starts_with(UNKNOWN), "{said}");
        assert!(said.contains("The tap went out"), "{said}");
        assert!(said.contains("wasn't sent again"), "{said}");
        assert!(said.contains("(The iPhone's port 8117 didn't answer in time)"));
        assert!(unknown("/path", "x").contains("The swipe"));
        assert!(unknown("/type", "x").contains("The text"));

        /// A driver whose `/type` goes out and is never answered.
        struct Lost(RefCell<Vec<String>>);
        impl Transport for Lost {
            fn post(&self, path: &str, _: &Value) -> Result<Value, String> {
                self.0.borrow_mut().push(path.to_string());
                match path {
                    "/type" => Err(unknown(path, "Couldn't read from the iPhone: reset")),
                    _ => Ok(json!({"ok": true})),
                }
            }
        }
        let lost = Lost(RefCell::default());
        // Typing then Return: the text's outcome is unknown, so Return isn't pressed on top,
        // and the text isn't typed a second time.
        let steps = [
            Step::Post("/type", json!({"text": "hello"})),
            Step::Post("/key", json!({"key": "enter"})),
        ];
        let err = run(&lost, &steps).unwrap_err();
        assert!(err.starts_with(UNKNOWN), "{err}");
        assert_eq!(*lost.0.borrow(), ["/type"]);
    }

    #[test]
    fn long_gestures_get_as_long_to_answer() {
        let base = answer_limit(&json!({"x": 1, "y": 2})).as_millis();
        assert_eq!(
            answer_limit(&json!({"hold_ms": 3000})).as_millis(),
            base + 3000
        );
        let path = json!({"points": [{"x": 1, "y": 1, "t_ms": 0}, {"x": 2, "y": 2, "t_ms": 9000}]});
        assert_eq!(answer_limit(&path).as_millis(), base + 9000);
        assert!(
            answer_limit(&json!({"key": "backspace", "count": 20}))
                > answer_limit(&json!({"key": "backspace"}))
        );
    }

    #[test]
    fn the_run_gets_absolute_paths_and_the_driver_environment() {
        let template = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>TrekDriver</key><dict>
<key>TestHostPath</key><string>__TESTROOT__/Trek.app</string>
<key>DependentProductPaths</key><array><string>__TESTROOT__/Trek.app</string></array>
<key>TestingEnvironmentVariables</key><dict><key>XCInjectBundleInto</key><string>unused</string></dict>
</dict></dict></plist>"#;
        let out = xctestrun(template, Path::new("/d"), &[("TREK_DRIVER", "1".into())]).unwrap();
        let v: plist::Value = plist::from_bytes(&out).unwrap();
        let t = v.as_dictionary().unwrap()["TrekDriver"]
            .as_dictionary()
            .unwrap();
        assert_eq!(t["TestHostPath"].as_string(), Some("/d/Trek.app"));
        assert_eq!(
            t["DependentProductPaths"].as_array().unwrap()[0].as_string(),
            Some("/d/Trek.app")
        );
        let env = t["TestingEnvironmentVariables"].as_dictionary().unwrap();
        assert_eq!(env["TREK_DRIVER"].as_string(), Some("1"));
        assert_eq!(env["XCInjectBundleInto"].as_string(), Some("unused"));
    }

    #[test]
    fn availability_says_what_to_install() {
        let install = Install {
            dir: "/d".into(),
            app: "/d/Trek.app".into(),
            xctestrun: "/d/TrekDriver.xctestrun".into(),
            bundle: "dev.trek.TrekMobile".into(),
            build: "938".into(),
            device: "PHONE".into(),
            expires: Some(1000),
            wifi: false,
        };
        assert_eq!(availability(None, "PHONE", false, 0, true), Avail::None);
        assert!(matches!(
            availability(None, "PHONE", true, 0, true),
            Avail::Install(_)
        ));
        assert_eq!(
            availability(Some(&install), "PHONE", true, 999, true),
            Avail::Ready
        );
        assert_eq!(
            availability(Some(&install), "PHONE", true, 1000, true),
            Avail::Install("its 7-day signing lapsed".into())
        );
        assert_eq!(
            availability(Some(&install), "OTHER", true, 0, true),
            Avail::Install("it was built for another iPhone".into())
        );
        // Over Wi-Fi: a driver built before it could serve there is offered over the cable only.
        assert_eq!(
            availability(Some(&install), "PHONE", true, 999, false),
            Avail::Install("the one on the iPhone works over the cable only".into())
        );
        let wifi = Install {
            wifi: true,
            ..install
        };
        assert_eq!(
            availability(Some(&wifi), "PHONE", true, 999, false),
            Avail::Ready
        );
    }

    #[test]
    fn a_driver_built_for_wifi_says_so_in_its_features() {
        let dir = std::env::temp_dir().join(format!("trek-driver-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Trek.app")).unwrap();
        std::fs::write(dir.join("TrekDriver.xctestrun"), "").unwrap();
        std::fs::write(dir.join("info"), "bundle=dev.trek.TrekMobile\nbuild=958\n").unwrap();
        assert!(
            !Install::read(&dir).unwrap().wifi,
            "no features: the cable only"
        );
        std::fs::write(dir.join("features"), "wifi\n").unwrap();
        assert!(Install::read(&dir).unwrap().wifi);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_comes_from_devicectls_answer() {
        // As devicectl answered for the user's iPhone over Wi-Fi.
        let seen = br#"{"info":{"commandType":"devicectl.device.info.lockState","outcome":"success"},
            "result":{"deviceIdentifier":"1392C405","passcodeRequired":false,"unlockedSinceBoot":true}}"#;
        assert_eq!(lock_state(seen), Some(false));
        let locked = br#"{"info":{"outcome":"success"},"result":{"passcodeRequired":true}}"#;
        assert_eq!(lock_state(locked), Some(true));
        assert_eq!(lock_state(br#"{"info":{"outcome":"failed"}}"#), None);
        assert_eq!(lock_state(b"not json"), None);
    }

    #[test]
    fn xcodebuild_logs_become_the_thing_to_do() {
        assert!(
            why("xcodebuild: error: Unable to find a destination matching the provided destination specifier:")
                .starts_with("Xcode can't reach the iPhone")
        );
        assert!(
            why(
                "Error Domain=com.apple.dt.deviceprep Code=-3 \"Unlock tobyj’s iPhone to Continue\""
            )
            .starts_with("Unlock the iPhone")
        );
        assert!(why("This device has reached the maximum number of installed apps using a free developer profile").contains("three"));
        assert!(why("xcodebuild: error: Failed to build workspace.").contains("xcodebuild: error"));
        assert!(why("").contains("without saying why"));
    }

    /// A driver that records what it's asked.
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<(String, Value)>>,
        fail: Option<&'static str>,
    }

    impl Transport for Fake {
        fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
            self.calls
                .borrow_mut()
                .push((path.to_string(), body.clone()));
            match self.fail {
                Some(p) if p == path => {
                    Err("Nothing has the keyboard on the iPhone: tap a text field first.".into())
                }
                _ => Ok(json!({"ok": true})),
            }
        }
    }

    fn tool(fake: &Fake, method: &str, p: Value) -> Result<(), String> {
        // Screenshot 660×1434 (half the cable's 1320×2868); screen 440×956 points.
        let point = |kx: &str, ky: &str| {
            let (x, y) = (p[kx].as_f64().unwrap(), p[ky].as_f64().unwrap());
            Ok(to_points((x / 660., y / 1434.), (440., 956.)))
        };
        let apps = vec![device::App {
            bundle: "dev.trek.TrekMobile".into(),
            name: "Trek".into(),
            version: String::new(),
            build: String::new(),
            file_sharing: false,
        }];
        let steps = plan(method, &p, &point, &|n| find_app(&apps, n))?;
        run(fake, &steps)
    }

    #[test]
    fn tools_map_onto_the_driver() {
        let fake = Fake::default();
        tool(&fake, "phone_tap", json!({"x": 330, "y": 717})).unwrap();
        tool(
            &fake,
            "phone_tap",
            json!({"x": 66, "y": 143.4, "hold_ms": 700}),
        )
        .unwrap();
        tool(
            &fake,
            "phone_swipe",
            json!({"x1": 330, "y1": 1000, "x2": 330, "y2": 400}),
        )
        .unwrap();
        tool(&fake, "phone_type", json!({"text": "q", "submit": true})).unwrap();
        tool(&fake, "phone_key", json!({"key": "backspace"})).unwrap();
        tool(&fake, "phone_key", json!({"key": "cmd+space"})).unwrap();
        tool(&fake, "phone_home", json!({})).unwrap();
        tool(&fake, "phone_open_app", json!({"name": "trek"})).unwrap();
        let calls = fake.calls.borrow();
        let want = [
            ("/tap", json!({"x": 220.0, "y": 478.0, "hold_ms": 80})),
            ("/tap", json!({"x": 44.0, "y": 95.6, "hold_ms": 700})),
            (
                "/swipe",
                json!({"x1": 220.0, "y1": 666.67, "x2": 220.0, "y2": 266.67, "duration_ms": 300}),
            ),
            ("/type", json!({"text": "q"})),
            ("/key", json!({"key": "enter"})),
            ("/key", json!({"key": "backspace"})),
            ("/key", json!({"key": "spotlight"})),
            ("/key", json!({"key": "home"})),
            ("/open", json!({"bundle": "dev.trek.TrekMobile"})),
        ];
        assert_eq!(calls.len(), want.len());
        for ((path, body), (wp, wb)) in calls.iter().zip(want.iter()) {
            assert_eq!((path.as_str(), body), (*wp, wb));
        }
    }

    #[test]
    fn an_unknown_app_opens_through_spotlight_and_errors_stop_the_steps() {
        let fake = Fake::default();
        tool(&fake, "phone_open_app", json!({"name": "Calculator"})).unwrap();
        let paths: Vec<_> = fake.calls.borrow().iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(paths, ["/key", "/type", "/key"]);
        let fake = Fake {
            fail: Some("/type"),
            ..Fake::default()
        };
        let err = tool(&fake, "phone_type", json!({"text": "q", "submit": true})).unwrap_err();
        assert!(err.starts_with("Nothing has the keyboard"));
        assert_eq!(
            fake.calls.borrow().len(),
            1,
            "Return isn't pressed after a failed type"
        );
        assert!(tool(&fake, "phone_open_app", json!({"name": " "})).is_err());
    }
}
