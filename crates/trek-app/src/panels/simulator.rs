//! iOS Simulator: a live mirror of a booted simulator with touch, typing and hardware buttons.
//!
//! With AXe installed, a resident helper (panels/simhid.rs) streams the framebuffer at up to 30 fps
//! and delivers touches, keys and buttons over one open HID session. Without it (or while the
//! helper is being built), frames come from `simctl io screenshot` and input from the AXe CLI.

use crate::workspace::{Route, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::assets::Lucide;
use super::simhid::{LinkEvent, SimLink};

/// Mirror cadence while the screen is changing and once it has settled. One `simctl` screenshot
/// takes ~0.35 s end to end, so while active two captures overlap (staggered) for ~5–6 fps;
/// once settled it's one at a time.
const FAST_FRAME: Duration = Duration::from_millis(125);
const IDLE_FRAME: Duration = Duration::from_millis(500);
const MAX_IN_FLIGHT: u32 = 2;
/// Unchanged frames in a row before dropping to the idle cadence.
const SETTLE_AFTER: u32 = 12;
const TICK: Duration = Duration::from_millis(40);
const DEVICE_REFRESH: Duration = Duration::from_secs(5);
const TYPE_DEBOUNCE: Duration = Duration::from_millis(220);
const SCROLL_DEBOUNCE: Duration = Duration::from_millis(140);
/// Pointer travel (window px) below which a press is a tap, not a swipe.
const TAP_SLOP: f32 = 6.;
const LONG_PRESS: Duration = Duration::from_millis(450);

const NEEDS_AXE: &str = "Needs AXe for touch input — install it from the banner";

// ---------------------------------------------------------------------------------------------
// simctl / AXe plumbing (all of it runs on background threads)

#[derive(Clone, Debug, PartialEq)]
struct Device {
    name: String,
    udid: String,
    runtime: String,
    state: String,
    device_type: String,
}

impl Device {
    fn booted(&self) -> bool {
        self.state == "Booted"
    }

    fn is_ipad(&self) -> bool {
        self.name.starts_with("iPad") || self.device_type.contains(".iPad")
    }

    fn icon(&self) -> Icon {
        Icon::new(if self.is_ipad() { Lucide::Tablet } else { Lucide::Smartphone })
    }
}

/// `simctl` itself (resolved once through xcrun so each frame skips the lookup).
fn simctl() -> Command {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    let path = PATH.get_or_init(|| {
        let out = Command::new("/usr/bin/xcrun").args(["--find", "simctl"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        (out.status.success() && p.is_file()).then_some(p)
    });
    let mut cmd = match path {
        Some(p) => Command::new(p),
        None => {
            let mut c = Command::new("/usr/bin/xcrun");
            c.arg("simctl");
            c
        }
    };
    cmd.stdin(Stdio::null());
    cmd
}

/// The useful line of a simctl / AXe failure.
fn clean_error(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines
        .iter()
        .find(|l| !l.starts_with("An error was encountered") && !l.starts_with("Underlying error") && !l.starts_with("Detected file type"))
        .or(lines.first())
        .map(|l| l.to_string())
        .unwrap_or_else(|| fallback.to_string());
    let mut line = line.trim_start_matches("Error: ").to_string();
    if line.chars().count() > 220 {
        line = line.chars().take(220).collect::<String>() + "…";
    }
    line
}

fn run_simctl(args: &[&str]) -> Result<String, String> {
    let out = simctl().args(args).output().map_err(|e| format!("Couldn't run simctl (is Xcode installed?): {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(clean_error(&out.stderr, &format!("simctl {} failed", args.first().unwrap_or(&""))))
    }
}

/// "com.apple.CoreSimulator.SimRuntime.iOS-27-0" → "iOS 27.0" (same as trek-mcp's simulator tools).
fn pretty_runtime(id: &str) -> String {
    let tail = id.rsplit('.').next().unwrap_or(id);
    match tail.split_once('-') {
        Some((os, ver)) => format!("{os} {}", ver.replace('-', ".")),
        None => tail.to_string(),
    }
}

fn version_key(runtime: &str) -> Vec<u32> {
    runtime.split_once(' ').map(|(_, v)| v).unwrap_or("").split('.').filter_map(|p| p.parse().ok()).collect()
}

/// Available iOS / iPadOS simulators: newest runtime first, then by name.
fn parse_devices(json_text: &str) -> Result<Vec<Device>, String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("Unexpected simctl output: {e}"))?;
    let mut out = Vec::new();
    for (runtime, list) in v.get("devices").and_then(Value::as_object).into_iter().flatten() {
        let runtime = pretty_runtime(runtime);
        if !runtime.starts_with("iOS") {
            continue;
        }
        for d in list.as_array().into_iter().flatten() {
            if d.get("isAvailable").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            out.push(Device { name: s("name"), udid: s("udid"), runtime: runtime.clone(), state: s("state"), device_type: s("deviceTypeIdentifier") });
        }
    }
    out.sort_by(|a, b| version_key(&b.runtime).cmp(&version_key(&a.runtime)).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

fn list_devices() -> Result<Vec<Device>, String> {
    parse_devices(&run_simctl(&["list", "devices", "available", "-j"])?)
}

/// Screen scale (points → pixels) for a device type, from its CoreSimulator profile.
/// Mirrors `Simulator::screen_scale` in crates/trek-mcp/src/simulator.rs.
fn screen_scale(device_type: &str) -> Option<f64> {
    let types: Value = serde_json::from_str(&run_simctl(&["list", "devicetypes", "-j"]).ok()?).ok()?;
    let bundle = types
        .get("devicetypes")?
        .as_array()?
        .iter()
        .find(|t| t.get("identifier").and_then(Value::as_str) == Some(device_type))?
        .get("bundlePath")?
        .as_str()?
        .to_string();
    let res = PathBuf::from(bundle).join("Contents/Resources");
    let attempts = [("capabilities.plist", "capabilities.ScreenDimensionsCapability.main-screen-scale"), ("profile.plist", "mainScreenScale")];
    attempts.iter().find_map(|(file, key)| {
        let path = res.join(file);
        let out = Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(&path).stdin(Stdio::null()).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().filter(|s| *s >= 1.0 && out.status.success())
    })
}

/// `Some(true)` dark, `Some(false)` light, `None` when the runtime can't switch.
fn read_appearance(udid: &str) -> Option<bool> {
    match run_simctl(&["ui", udid, "appearance"]).ok()?.trim() {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

struct Frame {
    udid: String,
    /// Hash of the encoded bytes, so an unchanged screen isn't decoded again.
    id: u64,
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
}

/// One mirror frame, decoded here rather than on the UI thread. JPEG because simctl encodes it
/// faster than PNG; it goes through a temp file since this simctl writes `-` as a file name.
fn grab_frame(udid: &str, seq: u64, prev: Option<u64>, renderer: SvgRenderer) -> Result<Option<Frame>, String> {
    let path = std::env::temp_dir().join(format!("trek-sim-{}-{seq}.jpg", std::process::id()));
    let out = simctl()
        .args(["io", udid, "screenshot", "--type=jpeg"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Couldn't run simctl: {e}"))?;
    let bytes = std::fs::read(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    if !out.status.success() || bytes.is_empty() {
        return Err(clean_error(&out.stderr, "The simulator didn't return a frame"));
    }
    let image = Image::from_bytes(ImageFormat::Jpeg, bytes);
    let id = image.id();
    if prev == Some(id) {
        return Ok(None);
    }
    let decoded = image.to_image_data(renderer).map_err(|e| format!("Couldn't decode the simulator frame: {e}"))?;
    let size = decoded.size(0);
    Ok(Some(Frame { udid: udid.to_string(), id, image: decoded, width: size.width.0 as f32, height: size.height.0 as f32 }))
}

struct AxeJob {
    bin: PathBuf,
    args: Vec<String>,
    stdin: Option<String>,
}

fn run_axe(job: &AxeJob) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = Command::new(&job.bin)
        .args(&job.args)
        .stdin(if job.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run AXe: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (&job.stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("AXe failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let what = job.args.first().map(String::as_str).unwrap_or("command");
        Err(format!("AXe {what}: {}", clean_error(&out.stderr, "failed")))
    }
}

fn coord(v: f64) -> String {
    format!("{:.1}", v.max(0.0))
}

/// "apple.com" → https://apple.com, "localhost:3000" → http://…; deep links (myapp://, tel:) pass through.
fn normalize_url(input: &str) -> String {
    let t = input.trim();
    const BARE: &[&str] = &["tel:", "sms:", "mailto:", "facetime:", "maps:", "data:"];
    if t.contains("://") || BARE.iter().any(|p| t.starts_with(p)) {
        t.to_string()
    } else if t.starts_with("localhost") || t.starts_with("127.0.0.1") {
        format!("http://{t}")
    } else {
        format!("https://{t}")
    }
}

fn bundle_id(app: &Path) -> Option<String> {
    let plist = app.join("Info.plist");
    let out = Command::new("/usr/bin/plutil").args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"]).arg(&plist).stdin(Stdio::null()).output().ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !id.is_empty()).then_some(id)
}

// ---------------------------------------------------------------------------------------------
// Layout of the mirrored screen inside the panel

#[derive(Clone, Copy)]
struct ScreenLayout {
    /// The screen image, relative to the mirror area.
    screen: Bounds<Pixels>,
    bezel: Pixels,
    radius: Pixels,
}

/// Fit the screen plus a thin bezel into `area`, keeping the aspect ratio and never growing past
/// the device's size in points.
fn fit(area: Size<Pixels>, w: f32, h: f32, max_w: f32, ipad: bool) -> Option<ScreenLayout> {
    let margin = 20.;
    let (aw, ah) = (area.width.as_f32() - 2. * margin, area.height.as_f32() - 2. * margin);
    if aw < 60. || ah < 60. || w < 1. || h < 1. {
        return None;
    }
    let k = if ipad { 0.025 } else { 0.035 };
    let s = (aw / (w * (1. + 2. * k))).min(ah / (h + 2. * k * w)).min(max_w / w);
    let (sw, sh) = (w * s, h * s);
    let x = (area.width.as_f32() - sw) / 2.;
    let y = (area.height.as_f32() - sh) / 2.;
    let corner = if ipad { 0.03 } else { 0.125 };
    Some(ScreenLayout { screen: Bounds { origin: point(px(x), px(y)), size: size(px(sw), px(sh)) }, bezel: px(k * sw), radius: px(corner * sw.min(sh)) })
}

/// A window position over the mirrored screen → device points, clamped to the screen.
fn window_to_points(screen: Bounds<Pixels>, per_px: f64, pts: Size<f64>, p: Point<Pixels>) -> (f64, f64) {
    let x = ((p.x - screen.origin.x).as_f32() as f64 * per_px).clamp(0.0, pts.width);
    let y = ((p.y - screen.origin.y).as_f32() as f64 * per_px).clamp(0.0, pts.height);
    (x, y)
}

struct Gesture {
    start: Point<Pixels>,
    last: Point<Pixels>,
    at: Instant,
}

// ---------------------------------------------------------------------------------------------

pub struct SimulatorPanel {
    workspace: Entity<Workspace>,
    devices: Vec<Device>,
    /// First device listing has come back.
    listed: bool,
    listing: bool,
    last_list: Option<Instant>,
    list_error: Option<String>,
    selected: Option<String>,
    /// Boots / shutdowns in flight, by UDID.
    pending: HashMap<String, &'static str>,
    /// A header action in flight (install, screenshot, …).
    busy: Option<&'static str>,
    error: Option<SharedString>,
    axe: Option<PathBuf>,
    axe_checking: bool,
    /// Screen scale per device type (0 = unknown, use a heuristic).
    scales: HashMap<String, f64>,
    /// Appearance per UDID (`None` = not supported by the runtime).
    dark: HashMap<String, Option<bool>>,
    info_pending: HashSet<String>,
    visible: bool,
    frame: Option<Frame>,
    /// Replaced frames whose GPU textures still need releasing.
    stale: Vec<Arc<RenderImage>>,
    /// Screenshots running right now, the sequence number of the next one and of the frame on
    /// screen (an older capture finishing late is dropped).
    in_flight: u32,
    next_seq: u64,
    shown_seq: u64,
    last_capture: Instant,
    unchanged: u32,
    capture_error: Option<String>,
    /// Mirror area bounds in window coordinates, recorded at prepaint.
    area: Rc<Cell<Bounds<Pixels>>>,
    gesture: Option<Gesture>,
    typed: String,
    type_flush: Option<Task<()>>,
    scroll: Option<(Point<Pixels>, Point<Pixels>)>,
    scroll_flush: Option<Task<()>>,
    url_input: Entity<InputState>,
    url_open: bool,
    focus: FocusHandle,
    axe_tx: async_channel::Sender<AxeJob>,
    /// The resident helper: its binary once built, the open link, and whether streaming is paused.
    helper: Option<Result<std::path::PathBuf, String>>,
    helper_building: bool,
    link: Option<Arc<SimLink>>,
    link_ready: bool,
    link_paused: bool,
    link_retry: Option<Instant>,
    _link_events: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SimulatorPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url_input = cx.new(|cx| InputState::new(window, cx).placeholder("https://example.com or myapp://path"));
        let sub = cx.subscribe_in(&url_input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.open_url(window, cx);
            }
        });

        // AXe calls run one at a time on their own thread, so taps and keystrokes keep their order.
        let (axe_tx, axe_rx) = async_channel::unbounded::<AxeJob>();
        let (err_tx, err_rx) = async_channel::unbounded::<String>();
        let _ = std::thread::Builder::new().name("trek-axe".into()).spawn(move || {
            while let Ok(job) = axe_rx.recv_blocking() {
                if let Err(e) = run_axe(&job) {
                    let _ = err_tx.send_blocking(e);
                }
            }
        });
        let errors = cx.spawn(async move |this, cx| {
            while let Ok(e) = err_rx.recv().await {
                if this.update(cx, |this, cx| this.fail(e, cx)).is_err() {
                    break;
                }
            }
        });
        // Frames left behind by a capture that outlived its process.
        cx.background_executor()
            .spawn(async move {
                let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
                for e in entries.flatten() {
                    let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d > Duration::from_secs(60)));
                    if old && e.file_name().to_string_lossy().starts_with("trek-sim-") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            })
            .detach();
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() {
                    break;
                }
            }
        });

        Self {
            workspace,
            devices: vec![],
            listed: false,
            listing: false,
            last_list: None,
            list_error: None,
            selected: None,
            pending: HashMap::new(),
            busy: None,
            error: None,
            axe: None,
            axe_checking: false,
            scales: HashMap::new(),
            dark: HashMap::new(),
            info_pending: HashSet::new(),
            visible: false,
            frame: None,
            stale: vec![],
            in_flight: 0,
            next_seq: 1,
            shown_seq: 0,
            last_capture: Instant::now(),
            unchanged: 0,
            capture_error: None,
            area: Rc::new(Cell::new(Bounds::default())),
            gesture: None,
            typed: String::new(),
            type_flush: None,
            scroll: None,
            scroll_flush: None,
            url_input,
            url_open: false,
            focus: cx.focus_handle(),
            axe_tx,
            helper: None,
            helper_building: false,
            link: None,
            link_ready: false,
            link_paused: false,
            link_retry: None,
            _link_events: None,
            _tasks: vec![errors, poll],
            _subscriptions: vec![sub],
        }
    }

    /// Called by the right panel when the tab is shown or hidden; polling only runs while shown.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            // Pick up AXe installed from the banner, and device changes made elsewhere.
            self.check_axe(cx);
            self.last_list = None;
            self.unchanged = 0;
        }
        cx.notify();
    }

    /// The tab is closing: give the frame textures back.
    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        self.drop_link();
        for image in self.stale.drain(..).chain(self.frame.take().map(|f| f.image)) {
            cx.drop_image(image, Some(&mut *window));
        }
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn current(&self) -> Option<&Device> {
        let id = self.selected.as_ref()?;
        self.devices.iter().find(|d| &d.udid == id)
    }

    /// The selected device when it's booted and ready for input.
    fn live(&self) -> Option<&Device> {
        self.current().filter(|d| d.booted() && !self.pending.contains_key(&d.udid))
    }

    fn on_screen(&self, window: &Window, cx: &App) -> bool {
        self.visible && window.is_visible() && !matches!(self.workspace.read(cx).route, Route::Settings(_) | Route::Onboarding)
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.on_screen(window, cx) {
            if let Some(link) = self.link.as_ref().filter(|_| !self.link_paused) {
                link.send("fps 0");
                self.link_paused = true;
            }
            return;
        }
        self.manage_link(window, cx);
        if self.link_ready {
            if self.link_paused {
                if let Some(link) = &self.link {
                    link.send("fps 30");
                }
                self.link_paused = false;
            }
            if !self.listing && self.last_list.is_none_or(|t| t.elapsed() >= DEVICE_REFRESH) {
                self.refresh_devices(window, cx);
            }
            return;
        }
        if !self.listing && self.last_list.is_none_or(|t| t.elapsed() >= DEVICE_REFRESH) {
            self.refresh_devices(window, cx);
        }
        if self.gesture.is_some() {
            return;
        }
        let Some(udid) = self.live().map(|d| d.udid.clone()) else { return };
        let settled = self.unchanged >= SETTLE_AFTER;
        let (interval, max) = if settled { (IDLE_FRAME, 1) } else { (FAST_FRAME, MAX_IN_FLIGHT) };
        if self.in_flight < max && self.last_capture.elapsed() >= interval {
            self.capture(udid, window, cx);
        }
    }

    /// Run blocking work off the UI thread, then apply its result.
    fn background<R: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| done(this, result, window, cx));
        })
        .detach();
    }

    fn check_axe(&mut self, cx: &mut Context<Self>) {
        if self.axe_checking {
            return;
        }
        self.axe_checking = true;
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { crate::integrations::axe_path() }).await;
            let _ = this.update(cx, |this, cx| {
                this.axe_checking = false;
                if this.axe != found {
                    this.axe = found;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn refresh_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.listing = true;
        self.background(window, cx, list_devices, |this, result, window, cx| {
            this.listing = false;
            this.listed = true;
            this.last_list = Some(Instant::now());
            match result {
                Ok(devices) => {
                    this.list_error = None;
                    if devices != this.devices {
                        this.devices = devices;
                        cx.notify();
                    }
                }
                Err(e) => {
                    this.list_error = Some(e);
                    cx.notify();
                }
            }
            let valid = this.selected.as_ref().is_some_and(|id| this.devices.iter().any(|d| &d.udid == id));
            if !valid {
                this.selected = this.devices.iter().find(|d| d.booted()).map(|d| d.udid.clone());
                cx.notify();
            }
            this.load_device_info(window, cx);
        });
    }

    /// Screen scale and appearance for the selected booted device, fetched once.
    fn load_device_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.live().cloned() else { return };
        if !self.scales.contains_key(&d.device_type) && self.info_pending.insert(d.device_type.clone()) {
            let dt = d.device_type.clone();
            self.background(
                window,
                cx,
                move || screen_scale(&dt),
                move |this, scale, _, cx| {
                    this.info_pending.remove(&d.device_type);
                    this.scales.insert(d.device_type.clone(), scale.unwrap_or(0.0));
                    cx.notify();
                },
            );
        }
        let Some(d) = self.live().cloned() else { return };
        if !self.dark.contains_key(&d.udid) && self.info_pending.insert(d.udid.clone()) {
            let udid = d.udid.clone();
            self.background(
                window,
                cx,
                move || read_appearance(&udid),
                move |this, dark, _, cx| {
                    this.info_pending.remove(&d.udid);
                    this.dark.insert(d.udid.clone(), dark);
                    cx.notify();
                },
            );
        }
    }

    fn select(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&udid) {
            self.selected = Some(udid);
            self.unchanged = 0;
            self.capture_error = None;
            self.load_device_info(window, cx);
        }
        cx.notify();
    }

    /// Put a decoded frame on screen, releasing the texture it replaces.
    fn show_frame(&mut self, frame: Frame, window: &mut Window, cx: &mut Context<Self>) {
        for old in self.stale.drain(..) {
            cx.drop_image(old, Some(&mut *window));
        }
        if let Some(old) = self.frame.replace(frame) {
            self.stale.push(old.image);
        }
        self.unchanged = 0;
        self.capture_error = None;
        cx.notify();
    }

    /// Build the helper once, then keep one link open to the live device.
    fn manage_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.live().map(|d| d.udid.clone());
        if self.link.as_ref().is_some_and(|l| Some(&l.udid) != live.as_ref()) {
            self.drop_link();
        }
        let Some(udid) = live else { return };
        if self.axe.is_none() || self.link.is_some() || self.link_retry.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        match &self.helper {
            None if !self.helper_building => {
                self.helper_building = true;
                self.background(window, cx, super::simhid::ensure_helper, |this, result, _, cx| {
                    this.helper_building = false;
                    if let Err(e) = &result {
                        tracing::warn!("simulator link unavailable: {e}");
                    }
                    this.helper = Some(result);
                    cx.notify();
                });
            }
            Some(Ok(path)) => {
                let path = path.clone();
                match SimLink::start(&path, &udid, cx.svg_renderer()) {
                    Ok((link, events)) => {
                        self.link = Some(link);
                        self.link_ready = false;
                        self.link_paused = false;
                        self._link_events = Some(cx.spawn_in(window, async move |this, cx| {
                            while let Ok(event) = events.recv().await {
                                let done = matches!(event, LinkEvent::Exited);
                                let _ = this.update_in(cx, |this, window, cx| this.link_event(event, window, cx));
                                if done {
                                    break;
                                }
                            }
                        }));
                    }
                    Err(e) => {
                        self.link_retry = Some(Instant::now());
                        tracing::warn!("{e}");
                    }
                }
            }
            _ => {}
        }
    }

    fn link_event(&mut self, event: LinkEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(udid) = self.link.as_ref().map(|l| l.udid.clone()) else { return };
        match event {
            LinkEvent::Ready => self.link_ready = true,
            LinkEvent::Frame(f) => {
                if self.selected.as_ref() != Some(&udid) {
                    cx.drop_image(f.image, Some(window));
                    return;
                }
                self.link_ready = true;
                let id = self.next_seq;
                self.next_seq += 1;
                self.show_frame(Frame { udid, id, image: f.image, width: f.width, height: f.height }, window, cx);
            }
            LinkEvent::Error(e) => {
                // Input errors are transient (e.g. the device is mid-boot); show them briefly.
                self.fail(format!("Simulator: {e}"), cx);
            }
            LinkEvent::Exited => {
                self.drop_link();
                self.link_retry = Some(Instant::now());
                cx.notify();
            }
        }
    }

    fn drop_link(&mut self) {
        self.link = None;
        self.link_ready = false;
        self._link_events = None;
    }

    /// The open link when it's ready for input.
    fn ready_link(&self) -> Option<Arc<SimLink>> {
        self.link.clone().filter(|_| self.link_ready)
    }

    fn capture(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        self.in_flight += 1;
        self.last_capture = Instant::now();
        let seq = self.next_seq;
        self.next_seq += 1;
        let prev = self.frame.as_ref().filter(|f| f.udid == udid).map(|f| f.id);
        let renderer = cx.svg_renderer();
        self.background(
            window,
            cx,
            move || grab_frame(&udid, seq, prev, renderer),
            move |this, result, window, cx| {
                this.in_flight = this.in_flight.saturating_sub(1);
                match result {
                    Ok(Some(frame)) => {
                        if this.selected.as_ref() != Some(&frame.udid) || seq < this.shown_seq {
                            cx.drop_image(frame.image, Some(window));
                            return;
                        }
                        this.shown_seq = seq;
                        this.show_frame(frame, window, cx);
                    }
                    Ok(None) => this.unchanged = this.unchanged.saturating_add(1),
                    Err(e) => {
                        // Usually the device was shut down elsewhere; the next listing will show it.
                        this.last_list = None;
                        if this.frame.is_none() {
                            this.capture_error = Some(e);
                            cx.notify();
                        }
                    }
                }
            },
        );
    }

    fn boot(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.pending.insert(udid.clone(), "Booting…");
        self.select(udid.clone(), window, cx);
        let id = udid.clone();
        // `bootstatus -b` boots if needed and returns once SpringBoard is up.
        self.background(
            window,
            cx,
            move || run_simctl(&["bootstatus", &id, "-b"]).map(|_| ()),
            move |this, result, window, cx| {
                this.pending.remove(&udid);
                if let Err(e) = result {
                    this.error = Some(format!("Couldn't boot: {e}").into());
                }
                this.refresh_devices(window, cx);
                cx.notify();
            },
        );
    }

    fn shutdown(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.pending.insert(udid.clone(), "Shutting down…");
        if self.link.as_ref().is_some_and(|l| l.udid == udid) {
            self.drop_link();
        }
        cx.notify();
        let id = udid.clone();
        self.background(
            window,
            cx,
            move || match run_simctl(&["shutdown", &id]) {
                Err(e) if !e.contains("current state: Shutdown") => Err(e),
                _ => Ok(()),
            },
            move |this, result, window, cx| {
                this.pending.remove(&udid);
                if let Err(e) = result {
                    this.error = Some(format!("Couldn't shut down: {e}").into());
                }
                if this.frame.as_ref().is_some_and(|f| f.udid == udid) {
                    if let Some(f) = this.frame.take() {
                        this.stale.push(f.image);
                    }
                }
                this.refresh_devices(window, cx);
                cx.notify();
            },
        );
    }

    /// A one-off simctl action on the live device, with a notification on success.
    fn action(
        &mut self,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(String) -> Result<String, String> + Send + 'static,
    ) {
        let Some(udid) = self.live().map(|d| d.udid.clone()) else { return };
        self.busy = Some(label);
        self.error = None;
        cx.notify();
        self.background(
            window,
            cx,
            move || work(udid),
            move |this, result, window, cx| {
                this.busy = None;
                this.unchanged = 0;
                match result {
                    Ok(msg) if !msg.is_empty() => window.push_notification(msg, cx),
                    Ok(_) => {}
                    Err(e) => this.error = Some(format!("{label}: {e}").into()),
                }
                cx.notify();
            },
        );
    }

    fn toggle_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(udid) = self.live().map(|d| d.udid.clone()) else { return };
        let Some(Some(dark)) = self.dark.get(&udid).copied() else { return };
        self.dark.insert(udid.clone(), Some(!dark));
        let mode = if dark { "light" } else { "dark" };
        self.action("Appearance", window, cx, move |u| run_simctl(&["ui", &u, "appearance", mode]).map(|_| String::new()));
    }

    fn screenshot_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.downgrade();
        let Some(udid) = self.live().map(|d| d.udid.clone()) else { return };
        self.busy = Some("Screenshot");
        self.error = None;
        cx.notify();
        let work = move || {
            let dir = trek_core::paths::data_dir().join("snapshots");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("simulator-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")));
            run_simctl(&["io", &udid, "screenshot", "--type=png", &path.to_string_lossy()])?;
            Ok::<PathBuf, String>(path)
        };
        self.background(window, cx, work, move |this, result, window, cx| {
            this.busy = None;
            match result {
                Ok(path) => {
                    let _ = ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::AttachImage(path)));
                    window.push_notification("Screenshot attached to your next message", cx);
                }
                Err(e) => this.error = Some(format!("Screenshot: {e}").into()),
            }
            cx.notify();
        });
    }

    fn open_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.url_input.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return;
        }
        let url = normalize_url(&raw);
        self.url_open = false;
        self.action("Open URL", window, cx, move |u| run_simctl(&["openurl", &u, &url]).map(|_| String::new()));
    }

    fn install_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: false, prompt: Some("Install".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(app) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if app.extension().and_then(|e| e.to_str()) != Some("app") || !app.is_dir() {
                    this.fail("Install: pick a built .app bundle (e.g. DerivedData/…/Debug-iphonesimulator/MyApp.app)", cx);
                    return;
                }
                this.action("Install", window, cx, move |u| {
                    let path = app.to_string_lossy().to_string();
                    run_simctl(&["install", &u, &path])?;
                    let name = app.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    match bundle_id(&app) {
                        Some(id) => {
                            run_simctl(&["launch", "--terminate-running-process", &u, &id])?;
                            Ok(format!("Installed and launched {name}"))
                        }
                        None => Ok(format!("Installed {name}")),
                    }
                });
            });
        })
        .detach();
    }

    fn open_simulator_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let udid = self.current().map(|d| d.udid.clone());
        self.background(
            window,
            cx,
            move || {
                let mut cmd = Command::new("/usr/bin/open");
                cmd.args(["-a", "Simulator"]);
                if let Some(u) = udid {
                    cmd.args(["--args", "-CurrentDeviceUDID", &u]);
                }
                cmd.stdin(Stdio::null())
                    .output()
                    .map_err(|e| e.to_string())
                    .and_then(|o| if o.status.success() { Ok(()) } else { Err(clean_error(&o.stderr, "open failed")) })
            },
            |this, result, _, cx| {
                if let Err(e) = result {
                    this.fail(format!("Couldn't open Simulator.app: {e}"), cx);
                }
            },
        );
    }

    // --- AXe input ---------------------------------------------------------------------------

    fn axe_send(&mut self, args: Vec<String>, stdin: Option<String>) {
        let (Some(bin), Some(udid)) = (self.axe.clone(), self.live().map(|d| d.udid.clone())) else { return };
        let mut args = args;
        args.extend(["--udid".to_string(), udid]);
        self.unchanged = 0;
        let _ = self.axe_tx.try_send(AxeJob { bin, args, stdin });
    }

    fn button(&mut self, name: &str) {
        self.flush_typed();
        if let Some(link) = self.ready_link() {
            link.send(&format!("button {}", if name == "side-button" { "side" } else { name }));
            return;
        }
        self.axe_send(vec!["button".into(), name.into()], None);
    }

    fn flush_typed(&mut self) {
        self.type_flush = None;
        if !self.typed.is_empty() {
            let text = std::mem::take(&mut self.typed);
            self.axe_send(vec!["type".into(), "--stdin".into()], Some(text));
        }
    }

    fn queue_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(link) = self.ready_link() {
            for line in text.split_inclusive('\n') {
                let (body, newline) = line.strip_suffix('\n').map_or((line, false), |b| (b, true));
                if !body.is_empty() {
                    link.send(&format!("text {body}"));
                }
                if newline {
                    link.send("key 40");
                }
            }
            return;
        }
        self.typed.push_str(text);
        self.type_flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPE_DEBOUNCE).await;
            let _ = this.update(cx, |this, _| this.flush_typed());
        }));
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if k.key == "escape" && !m.modified() {
            window.blur(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if (self.axe.is_none() && self.ready_link().is_none()) || self.live().is_none() {
            return;
        }
        if m.platform {
            if k.key == "v" {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.queue_text(&text, cx);
                }
                cx.stop_propagation();
            }
            return; // other ⌘ shortcuts stay with Trek
        }
        if m.control || m.function {
            return;
        }
        // HID usage codes for the keys `axe type` can't express.
        let code = match k.key.as_str() {
            "enter" => Some(40),
            "backspace" => Some(42),
            "tab" => Some(43),
            "delete" => Some(76),
            "right" => Some(79),
            "left" => Some(80),
            "down" => Some(81),
            "up" => Some(82),
            _ => None,
        };
        if let Some(code) = code {
            self.flush_typed();
            if let Some(link) = self.ready_link() {
                link.send(&format!("key {code}"));
                cx.stop_propagation();
                return;
            }
            self.axe_send(vec!["key".into(), code.to_string()], None);
            cx.stop_propagation();
            return;
        }
        let text = match k.key.as_str() {
            "space" => Some(" ".to_string()),
            _ => k.key_char.clone().filter(|s| !s.is_empty() && !s.chars().any(char::is_control)),
        };
        if let Some(text) = text {
            self.queue_text(&text, cx);
            cx.stop_propagation();
        }
    }

    /// The device's screen in window coordinates, and device points per window pixel.
    fn screen_map(&self) -> Option<(Bounds<Pixels>, f64, Size<f64>)> {
        let frame = self.frame.as_ref()?;
        let d = self.live().filter(|d| d.udid == frame.udid)?;
        let scale = self.scale_for(d, frame);
        let area = self.area.get();
        let l = fit(area.size, frame.width, frame.height, frame.width / scale as f32, d.is_ipad())?;
        let screen = Bounds { origin: area.origin + l.screen.origin, size: l.screen.size };
        let per_px = frame.width as f64 / scale / l.screen.size.width.as_f32() as f64;
        Some((screen, per_px, size(frame.width as f64 / scale, frame.height as f64 / scale)))
    }

    fn scale_for(&self, d: &Device, frame: &Frame) -> f64 {
        self.scales.get(&d.device_type).copied().filter(|s| *s >= 1.0).unwrap_or(if frame.width.min(frame.height) >= 1000. { 3.0 } else { 2.0 })
    }

    fn to_points(&self, p: Point<Pixels>) -> Option<(f64, f64)> {
        let (screen, per_px, pts) = self.screen_map()?;
        Some(window_to_points(screen, per_px, pts, p))
    }

    fn pointer_down(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if self.axe.is_none() {
            return;
        }
        if self.screen_map().is_some_and(|(screen, _, _)| screen.contains(&p)) {
            self.flush_typed();
            self.gesture = Some(Gesture { start: p, last: p, at: Instant::now() });
            if let (Some(link), Some((x, y))) = (self.ready_link(), self.to_points(p)) {
                link.touch("down", x, y);
            }
        }
        cx.notify();
    }

    /// Live drag: the finger follows the pointer while the link is open.
    fn pointer_move(&mut self, p: Point<Pixels>) {
        let Some(g) = self.gesture.as_mut() else { return };
        g.last = p;
        if let (Some(link), Some((x, y))) = (self.ready_link(), self.to_points(p)) {
            link.touch("move", x, y);
        }
    }

    fn pointer_up(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut g) = self.gesture.take() else { return };
        g.last = p;
        if let Some(link) = self.ready_link() {
            if let Some((x, y)) = self.to_points(p) {
                link.touch("up", x, y);
            }
            cx.notify();
            return;
        }
        let (Some((ax, ay)), Some((bx, by))) = (self.to_points(g.start), self.to_points(g.last)) else { return };
        let held = g.at.elapsed();
        let moved = (g.last.x - g.start.x).as_f32().hypot((g.last.y - g.start.y).as_f32());
        let args: Vec<String> = if moved < TAP_SLOP {
            if held >= LONG_PRESS {
                let delay = format!("{:.2}", held.as_secs_f64().min(5.0));
                vec!["touch".into(), "-x".into(), coord(ax), "-y".into(), coord(ay), "--down".into(), "--up".into(), "--delay".into(), delay]
            } else {
                vec!["tap".into(), "-x".into(), coord(ax), "-y".into(), coord(ay)]
            }
        } else {
            let duration = format!("{:.2}", held.as_secs_f64().clamp(0.1, 2.0));
            vec![
                "swipe".into(),
                "--start-x".into(),
                coord(ax),
                "--start-y".into(),
                coord(ay),
                "--end-x".into(),
                coord(bx),
                "--end-y".into(),
                coord(by),
                "--duration".into(),
                duration,
            ]
        };
        self.axe_send(args, None);
        cx.notify();
    }

    /// Trackpad / wheel scrolling over the screen becomes a swipe once the scroll pauses.
    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if (self.axe.is_none() && self.ready_link().is_none()) || !self.screen_map().is_some_and(|(s, _, _)| s.contains(&event.position)) {
            return;
        }
        let delta = event.delta.pixel_delta(px(20.));
        let first = self.scroll.is_none();
        let (anchor, total) = self.scroll.unwrap_or((event.position, Point::default()));
        self.scroll = Some((anchor, total + delta));
        // With the link, scrolling is a finger dragging in real time; it lifts once scrolling stops.
        if let Some(link) = self.ready_link() {
            let to = |p: Point<Pixels>| self.to_points(p);
            if first {
                if let Some((x, y)) = to(anchor) {
                    link.touch("down", x, y);
                }
            }
            if let Some((x, y)) = to(anchor + total + delta) {
                link.touch("move", x, y);
            }
        }
        self.scroll_flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCROLL_DEBOUNCE).await;
            let _ = this.update(cx, |this, _| this.flush_scroll());
        }));
        cx.stop_propagation();
    }

    fn flush_scroll(&mut self) {
        self.scroll_flush = None;
        let Some((anchor, total)) = self.scroll.take() else { return };
        if let Some(link) = self.ready_link() {
            if let Some((x, y)) = self.to_points(anchor + total) {
                link.touch("up", x, y);
            }
            return;
        }
        let Some((screen, _, _)) = self.screen_map() else { return };
        // Start mid-screen so short screens and edges still leave room to travel.
        let start = point(anchor.x, screen.origin.y + screen.size.height / 2.);
        let end = point(start.x + total.x, start.y + total.y);
        let (Some((ax, ay)), Some((bx, by))) = (self.to_points(start), self.to_points(end)) else { return };
        if (bx - ax).hypot(by - ay) < 4.0 {
            return;
        }
        let args = ["swipe", "--start-x", &coord(ax), "--start-y", &coord(ay), "--end-x", &coord(bx), "--end-y", &coord(by), "--duration", "0.25"];
        self.axe_send(args.iter().map(|s| s.to_string()).collect(), None);
    }

    // --- rendering ---------------------------------------------------------------------------

    fn device_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current().cloned();
        let label: SharedString = current.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "Choose a device".into()).into();
        let icon = current.as_ref().map(|d| d.icon()).unwrap_or_else(|| Icon::new(Lucide::Smartphone));
        let devices = self.devices.clone();
        let selected = self.selected.clone();
        let entity = cx.entity().downgrade();
        let green = crate::palette::emerald(cx);
        Button::new("sim-device")
            .ghost()
            .small()
            .icon(icon)
            .label(label)
            .dropdown_caret(true)
            .disabled(self.devices.is_empty())
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                menu = menu.min_w(px(260.)).max_h(px(420.)).scrollable(true);
                for (group, ipad) in [("iPhone", false), ("iPad", true)] {
                    let list: Vec<&Device> = devices.iter().filter(|d| d.is_ipad() == ipad).collect();
                    if list.is_empty() {
                        continue;
                    }
                    menu = menu.label(group);
                    for d in list {
                        let (name, runtime, booted, udid) = (d.name.clone(), d.runtime.clone(), d.booted(), d.udid.clone());
                        let entity = entity.clone();
                        menu = menu.item(
                            PopupMenuItem::element(move |_, cx| {
                                let muted = cx.theme().muted_foreground;
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                                    .child(div().text_xs().text_color(muted).child(runtime.clone()))
                                    .child(div().size(px(6.)).rounded_full().when(booted, |el| el.bg(green)))
                            })
                            .checked(selected.as_ref() == Some(&d.udid))
                            .on_click(move |_, window, cx| {
                                let udid = udid.clone();
                                let _ = entity.update(cx, |this, cx| this.select(udid, window, cx));
                            }),
                        );
                    }
                }
                menu
            })
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.current().cloned();
        let pending = current.as_ref().and_then(|d| self.pending.get(&d.udid).copied());
        let power: AnyElement = match (&current, pending) {
            (Some(_), Some(label)) => {
                h_flex().gap_1().px_2().text_xs().text_color(theme.muted_foreground).child(Spinner::new().xsmall()).child(label).into_any_element()
            }
            (Some(d), None) if d.booted() => {
                let udid = d.udid.clone();
                Button::new("sim-shutdown")
                    .ghost()
                    .small()
                    .icon(Lucide::Power)
                    .label("Shut down")
                    .on_click(cx.listener(move |this, _, window, cx| this.shutdown(udid.clone(), window, cx)))
                    .into_any_element()
            }
            (Some(d), None) => {
                let udid = d.udid.clone();
                Button::new("sim-boot")
                    .primary()
                    .small()
                    .icon(Lucide::Power)
                    .label("Boot")
                    .on_click(cx.listener(move |this, _, window, cx| this.boot(udid.clone(), window, cx)))
                    .into_any_element()
            }
            (None, _) => div().into_any_element(),
        };
        h_flex()
            .px_2()
            .h(px(40.))
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(div().min_w_0().child(self.device_picker(cx)))
            .child(div().flex_1())
            .child(power)
            .child(
                crate::ui::icon_button("sim-open-app", Lucide::AppWindow, "Open in Simulator.app")
                    .on_click(cx.listener(|this, _, window, cx| this.open_simulator_app(window, cx))),
            )
            .into_any_element()
    }

    /// Hardware buttons and device actions for the live device.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let has_axe = self.axe.is_some();
        let udid = self.live().map(|d| d.udid.clone()).unwrap_or_default();
        let dark = self.dark.get(&udid).copied();
        let busy = self.busy;
        let url_input = self.url_input.clone();
        let entity = cx.entity().downgrade();
        let url = Popover::new("sim-url")
            .anchor(Anchor::TopLeft)
            .appearance(false)
            .open(self.url_open)
            .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                this.url_open = *open;
                if *open {
                    let handle = this.url_input.read(cx).focus_handle(cx);
                    handle.focus(window, cx);
                }
                cx.notify();
            }))
            .trigger(crate::ui::icon_button("sim-url-btn", Lucide::Link, "Open URL or deep link").selected(self.url_open))
            .content(move |_, _, cx| {
                let entity = entity.clone();
                crate::ui::menu_surface(cx)
                    .w(px(300.))
                    .p_2()
                    .gap_2()
                    .child(div().px_1().text_xs().text_color(cx.theme().muted_foreground).child("Open a web link or your app's URL scheme"))
                    .child(h_flex().gap_1().child(div().flex_1().child(Input::new(&url_input).small())).child(
                        Button::new("sim-url-go").small().primary().label("Open").on_click(move |_, window, cx| {
                            let _ = entity.update(cx, |this, cx| this.open_url(window, cx));
                        }),
                    ))
            });
        let appearance = match dark {
            Some(Some(is_dark)) => crate::ui::icon_button(
                "sim-appearance",
                if is_dark { Lucide::Moon } else { Lucide::Sun },
                if is_dark { "Switch to light appearance" } else { "Switch to dark appearance" },
            )
            .on_click(cx.listener(|this, _, window, cx| this.toggle_appearance(window, cx))),
            Some(None) => crate::ui::icon_button("sim-appearance", Lucide::Sun, "This runtime can't switch appearance").disabled(true),
            None => crate::ui::icon_button("sim-appearance", Lucide::Sun, "Reading appearance…").disabled(true),
        };
        h_flex()
            .px_2()
            .h(px(36.))
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(crate::ui::icon_button("sim-home", Lucide::House, if has_axe { "Home" } else { NEEDS_AXE }).disabled(!has_axe).on_click(cx.listener(
                |this, _, _, cx| {
                    this.button("home");
                    cx.notify();
                },
            )))
            .child(crate::ui::icon_button("sim-lock", Lucide::Lock, if has_axe { "Lock (side button)" } else { NEEDS_AXE }).disabled(!has_axe).on_click(
                cx.listener(|this, _, _, cx| {
                    this.button("lock");
                    cx.notify();
                }),
            ))
            .child(appearance)
            .child(crate::ui::divider(cx))
            .child(
                crate::ui::icon_button("sim-shot", Lucide::Camera, "Screenshot to chat")
                    .loading(busy == Some("Screenshot"))
                    .on_click(cx.listener(|this, _, window, cx| this.screenshot_to_chat(window, cx))),
            )
            .child(url)
            .child(
                crate::ui::icon_button("sim-install", Lucide::PackagePlus, "Install a .app and launch it")
                    .loading(busy == Some("Install"))
                    .on_click(cx.listener(|this, _, window, cx| this.install_app(window, cx))),
            )
            .child(div().flex_1())
            .when(self.focus.is_focused(window), |el| {
                el.child(h_flex().gap_1().pr_1().text_xs().text_color(theme.muted_foreground).child(Icon::new(Lucide::Keyboard).xsmall()).child(if has_axe {
                    "Typing goes to the simulator · esc"
                } else {
                    "Typing needs AXe"
                }))
            })
            .into_any_element()
    }

    fn mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let focused = self.focus.is_focused(window);
        let area = self.area.clone();
        let entity = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let before = area.get();
                area.set(bounds);
                if before.size != bounds.size {
                    let entity = entity.clone();
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let device = self.live().cloned();
        let frame = self.frame.as_ref().filter(|f| device.as_ref().is_some_and(|d| d.udid == f.udid));
        let layout = match (frame, &device) {
            (Some(f), Some(d)) => {
                fit(self.area.get().size, f.width, f.height, f.width / self.scale_for(d, f) as f32, d.is_ipad()).map(|l| (l, f.image.clone()))
            }
            _ => None,
        };
        let has_axe = self.axe.is_some();
        let waiting = frame.is_none();
        let capture_error = self.capture_error.clone();

        div()
            .id("sim-mirror")
            .key_context("Simulator")
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .when(has_axe, |el| el.cursor_pointer())
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| this.pointer_down(e.position, window, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                if e.pressed_button == Some(MouseButton::Left) {
                    this.pointer_move(e.position);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| this.pointer_up(e.position, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| this.pointer_up(e.position, cx)))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.scroll(e, cx)))
            .child(measure)
            .when_some(layout, |el, (l, image)| {
                let s = l.screen;
                el.child(
                    div()
                        .absolute()
                        .left(s.origin.x - l.bezel)
                        .top(s.origin.y - l.bezel)
                        .w(s.size.width + l.bezel * 2.)
                        .h(s.size.height + l.bezel * 2.)
                        .rounded(l.radius + l.bezel)
                        .bg(rgb(0x0B0B0D))
                        .border_1()
                        .border_color(if focused { theme.ring.opacity(0.7) } else { theme.border })
                        .shadow_lg(),
                )
                .child(img(image).absolute().left(s.origin.x).top(s.origin.y).w(s.size.width).h(s.size.height).rounded(l.radius))
            })
            .when(waiting, |el| {
                el.child(
                    v_flex().absolute().top_0().left_0().size_full().items_center().justify_center().gap_2().text_sm().text_color(theme.muted_foreground).map(
                        |el| match capture_error {
                            Some(e) => el.child(div().px_6().text_center().child(format!("Can't mirror this device: {e}"))),
                            None => el.child(Spinner::new().small()).child("Connecting to the display…"),
                        },
                    ),
                )
            })
            .into_any_element()
    }

    fn axe_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_none()
            .mx_2()
            .mt_2()
            .px_3()
            .py(px(6.))
            .gap_2()
            .rounded(px(10.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .text_sm()
            .child(Icon::new(Lucide::Hand).small().text_color(theme.muted_foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().child("Touch input needs AXe"))
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Taps, swipes, typing and hardware buttons")),
            )
            .child(Button::new("sim-install-axe").small().primary().label("Install AXe").on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal(crate::integrations::AXE_INSTALL.into())));
            })))
            .into_any_element()
    }

    fn device_card(&self, d: &Device, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected.as_ref() == Some(&d.udid);
        let pending = self.pending.get(&d.udid).copied();
        let udid = d.udid.clone();
        let status: AnyElement = match pending {
            Some(label) => h_flex().gap_1().text_xs().text_color(theme.muted_foreground).child(Spinner::new().xsmall()).child(label).into_any_element(),
            None if d.booted() => {
                let udid = udid.clone();
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(div().size(px(6.)).rounded_full().bg(crate::palette::emerald(cx)))
                            .child("Booted"),
                    )
                    .child(Button::new(SharedString::from(format!("sim-show-{udid}"))).small().outline().label("Show").on_click(cx.listener(
                        move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.select(udid.clone(), window, cx);
                        },
                    )))
                    .into_any_element()
            }
            None => {
                let udid = udid.clone();
                Button::new(SharedString::from(format!("sim-boot-{udid}")))
                    .small()
                    .outline()
                    .icon(Lucide::Power)
                    .label("Boot")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.boot(udid.clone(), window, cx);
                    }))
                    .into_any_element()
            }
        };
        h_flex()
            .id(SharedString::from(format!("sim-card-{udid}")))
            .px_3()
            .h(px(52.))
            .gap_3()
            .rounded(px(10.))
            .border_1()
            .border_color(if selected { theme.ring.opacity(0.5) } else { theme.border })
            .bg(theme.foreground.opacity(if selected { 0.06 } else { 0.025 }))
            .hover(|s| s.bg(theme.foreground.opacity(0.06)))
            .cursor_pointer()
            .child(d.icon().small().text_color(theme.muted_foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(d.name.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(d.runtime.clone())),
            )
            .child(status)
            .on_click(cx.listener(move |this, _, window, cx| this.select(udid.clone(), window, cx)))
            .into_any_element()
    }

    fn device_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut col = v_flex().id("sim-devices").size_full().overflow_y_scroll().px_3().py_4().gap_2().child(
            v_flex().px_1().pb_2().gap_1().child(div().text_sm().font_medium().child("iOS Simulator")).child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Boot a device to mirror it here. Click to tap, drag to swipe, and type straight into the app."),
            ),
        );
        for (group, ipad) in [("iPhone", false), ("iPad", true)] {
            let list: Vec<Device> = self.devices.iter().filter(|d| d.is_ipad() == ipad).cloned().collect();
            if list.is_empty() {
                continue;
            }
            col = col.child(div().px_1().pt_2().text_xs().font_medium().text_color(theme.muted_foreground).child(group));
            for d in &list {
                col = col.child(self.device_card(d, cx));
            }
        }
        col.into_any_element()
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.listed {
            return v_flex().size_full().items_center().justify_center().child(Spinner::new().small()).into_any_element();
        }
        if let Some(e) = &self.list_error {
            let text = format!("Couldn't list simulators. {e}");
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .px_6()
                .text_sm()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element();
        }
        if self.devices.is_empty() {
            return super::empty("No iOS simulators found. Add an iOS runtime in Xcode ▸ Settings ▸ Components.", cx).into_any_element();
        }
        let Some(d) = self.current().cloned() else { return self.device_list(cx) };
        if self.pending.get(&d.udid).is_some_and(|l| l.starts_with("Boot")) {
            let muted = cx.theme().muted_foreground;
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new().small())
                .child(div().text_sm().child(format!("Booting {}…", d.name)))
                .child(div().text_xs().text_color(muted).child("The first boot of a device can take a minute."))
                .into_any_element();
        }
        if !d.booted() {
            return self.device_list(cx);
        }
        let banner = self.axe.is_none().then(|| self.axe_banner(cx));
        v_flex().size_full().child(self.toolbar(window, cx)).children(banner).child(self.mirror(window, cx)).into_any_element()
    }
}

impl Render for SimulatorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error = self.error.clone();
        v_flex()
            .size_full()
            .child(self.header(cx))
            .when_some(error, |el, e| {
                el.child(
                    h_flex()
                        .px_3()
                        .py(px(6.))
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .bg(crate::palette::red(cx).opacity(0.08))
                        .child(Icon::new(IconName::CircleAlert).xsmall().text_color(crate::palette::red(cx)))
                        .child(div().flex_1().min_w_0().text_xs().child(e))
                        .child(crate::ui::icon_button("sim-error-close", IconName::Close, "Dismiss").xsmall().on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        }))),
                )
            })
            .child(div().flex_1().min_h_0().child(self.body(window, cx)))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob exports its own `test` attribute.
    use super::{clean_error, fit, normalize_url, parse_devices, window_to_points};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn devices_are_ios_only_newest_first() {
        let j = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[{"name":"iPhone 16","udid":"A","state":"Shutdown","isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-16"}],
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0":[
                {"name":"iPhone 17","udid":"B","state":"Booted","isAvailable":true},
                {"name":"iPad Air","udid":"C","state":"Shutdown","isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Air"},
                {"name":"Broken","udid":"D","state":"Shutdown","isAvailable":false}],
            "com.apple.CoreSimulator.SimRuntime.watchOS-12-0":[{"name":"Watch","udid":"E","state":"Shutdown","isAvailable":true}]}}"#;
        let d = parse_devices(j).unwrap();
        let ids: Vec<_> = d.iter().map(|d| d.udid.as_str()).collect();
        assert_eq!(ids, ["C", "B", "A"]);
        assert!(d[0].is_ipad() && !d[1].is_ipad());
        assert!(d[1].booted());
        assert_eq!(d[2].runtime, "iOS 26.0");
    }

    #[test]
    fn urls() {
        assert_eq!(normalize_url("apple.com"), "https://apple.com");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("myapp://home"), "myapp://home");
        assert_eq!(normalize_url("tel:123"), "tel:123");
    }

    #[test]
    fn fit_keeps_aspect_and_caps_at_points() {
        let l = fit(size(px(400.), px(800.)), 1206., 2622., 402., false).unwrap();
        let ratio = l.screen.size.width.as_f32() / l.screen.size.height.as_f32();
        assert!((ratio - 1206. / 2622.).abs() < 0.001);
        assert!(l.screen.size.width.as_f32() <= 402.);
        let big = fit(size(px(4000.), px(8000.)), 1206., 2622., 402., false).unwrap();
        assert!((big.screen.size.width.as_f32() - 402.).abs() < 0.01);
    }

    #[test]
    fn clicks_map_to_device_points() {
        // iPhone 17: 1206x2622 px @3x = 402x874 pt, shown 201 px wide (half size).
        let screen = Bounds { origin: point(px(100.), px(50.)), size: size(px(201.), px(437.)) };
        let per_px = 1206. / 3. / 201.;
        let pts = size(402., 874.);
        let (x, y) = window_to_points(screen, per_px, pts, point(px(200.5), px(268.5)));
        assert!((x - 201.).abs() < 0.01 && (y - 437.).abs() < 0.01);
        assert_eq!(window_to_points(screen, per_px, pts, point(px(0.), px(9999.))), (0., 874.));
    }

    #[test]
    fn errors_pick_the_useful_line() {
        let e = b"An error was encountered processing the command (domain=com.apple.CoreSimulator.SimError, code=405):\nUnable to boot device in current state: Booted\n";
        assert_eq!(clean_error(e, "x"), "Unable to boot device in current state: Booted");
        assert_eq!(clean_error(b"", "fallback"), "fallback");
    }
}
