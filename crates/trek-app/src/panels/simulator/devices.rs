//! The Simulator tool's devices: "Add device…" (a device type, a runtime that runs it and a
//! name, then `simctl create`, boot and show), "Manage devices…" (rename, duplicate, erase,
//! delete, boot and shut down each simulator, and delete the unavailable ones), and "Get more
//! runtimes…" for a platform with none installed (`xcodebuild -downloadPlatform` in Trek's
//! terminal, the catalog read again until the runtime is in). macOS only: it's all `simctl`.
//!
//! The sheets are drawn as the composer's menus are (`composer::look`): the segmented control,
//! rows with a quiet highlight, a hairline round the list.

use super::{Device, Family, SimulatorPanel, pretty_runtime, run_simctl};
use crate::assets::Lucide;
use crate::composer::look;
use crate::workspace::WorkspaceEvent;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Confirm as Enter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often the catalog is read again while a runtime downloads in the terminal.
const DOWNLOAD_POLL: Duration = Duration::from_secs(10);
/// The sheets' padding.
const PAD: f32 = 20.;

// ---------------------------------------------------------------------------------------------
// What Xcode can make: platforms, device types and the runtimes that run them

/// A platform simulators run, as `xcodebuild -downloadPlatform` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Platform {
    Ios,
    WatchOs,
    TvOs,
    VisionOs,
}

impl Platform {
    const ALL: [Platform; 4] = [
        Platform::Ios,
        Platform::WatchOs,
        Platform::TvOs,
        Platform::VisionOs,
    ];

    /// A runtime's `platform` in `simctl list runtimes -j` (visionOS's is still "xrOS").
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "iOS" => Platform::Ios,
            "watchOS" => Platform::WatchOs,
            "tvOS" => Platform::TvOs,
            "xrOS" | "visionOS" => Platform::VisionOs,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Platform::Ios => "iOS",
            Platform::WatchOs => "watchOS",
            Platform::TvOs => "tvOS",
            Platform::VisionOs => "visionOS",
        }
    }

    /// The family to show in "Add device…" for this platform.
    fn family(self) -> Family {
        match self {
            Platform::Ios => Family::IPhone,
            Platform::WatchOs => Family::Watch,
            Platform::TvOs => Family::Tv,
            Platform::VisionOs => Family::Vision,
        }
    }

    /// What downloads and installs the newest runtime for it.
    fn download_command(self) -> String {
        format!("xcodebuild -downloadPlatform {}", self.name())
    }
}

impl Family {
    fn platform(self) -> Platform {
        match self {
            Family::IPhone | Family::IPad => Platform::Ios,
            Family::Watch => Platform::WatchOs,
            Family::Tv => Platform::TvOs,
            Family::Vision => Platform::VisionOs,
        }
    }

    /// A device type's `productFamily`.
    fn from_product(product: &str) -> Option<Self> {
        Some(match product {
            "iPhone" => Family::IPhone,
            "iPad" => Family::IPad,
            "Apple Watch" => Family::Watch,
            "Apple TV" => Family::Tv,
            "Apple Vision" => Family::Vision,
            _ => return None,
        })
    }

    /// Its label on the sheet's segmented control.
    fn short(self) -> &'static str {
        match self {
            Family::IPhone => "iPhone",
            Family::IPad => "iPad",
            Family::Watch => "Watch",
            Family::Tv => "TV",
            Family::Vision => "Vision",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DeviceType {
    id: String,
    name: String,
    family: Family,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Runtime {
    id: String,
    /// "iOS 27.0".
    name: String,
    platform: Platform,
    version: Vec<u32>,
    /// The device types it runs, in Apple's order (newest first).
    types: Vec<DeviceType>,
}

/// The runtimes installed and available, each with the device types it runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Catalog {
    runtimes: Vec<Runtime>,
}

impl Catalog {
    /// The runtimes that run a device of `family`, newest first.
    fn runtimes(&self, family: Family) -> Vec<&Runtime> {
        self.runtimes
            .iter()
            .filter(|r| r.types.iter().any(|t| t.family == family))
            .collect()
    }

    /// The device types of `family` runtime `runtime` runs.
    fn types(&self, family: Family, runtime: &str) -> Vec<&DeviceType> {
        self.runtimes
            .iter()
            .filter(|r| r.id == runtime)
            .flat_map(|r| r.types.iter())
            .filter(|t| t.family == family)
            .collect()
    }

    /// The platforms with no runtime installed.
    fn missing(&self) -> Vec<Platform> {
        Platform::ALL
            .into_iter()
            .filter(|p| !self.runtimes.iter().any(|r| r.platform == *p))
            .collect()
    }
}

fn version_parts(version: &str) -> Vec<u32> {
    version.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// `simctl list devicetypes runtimes -j`: the available runtimes, each with the device types it
/// supports (`supportedDeviceTypes`), so only combinations it can make are offered. A type's
/// name and family come from its entry there, or from the device type list when it leaves them
/// out.
pub(super) fn parse_catalog(json: &str) -> Result<Catalog, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("Unexpected simctl output: {e}"))?;
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let known: HashMap<String, (Option<String>, Option<Family>)> = v
        .get("devicetypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                str_of(t, "identifier")?,
                (
                    str_of(t, "name"),
                    str_of(t, "productFamily").and_then(|f| Family::from_product(&f)),
                ),
            ))
        })
        .collect();
    let mut runtimes = Vec::new();
    for r in v
        .get("runtimes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if r.get("isAvailable").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let (Some(id), Some(platform)) = (
            str_of(r, "identifier"),
            str_of(r, "platform").and_then(|p| Platform::parse(&p)),
        ) else {
            continue;
        };
        let version = str_of(r, "version").unwrap_or_default();
        let types: Vec<DeviceType> = r
            .get("supportedDeviceTypes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let id = str_of(t, "identifier")?;
                let (known_name, known_family) = known.get(&id).cloned().unwrap_or_default();
                let family = str_of(t, "productFamily")
                    .and_then(|f| Family::from_product(&f))
                    .or(known_family)?;
                let name = str_of(t, "name").or(known_name)?;
                (family.platform() == platform).then_some(DeviceType { id, name, family })
            })
            .collect();
        if types.is_empty() {
            continue;
        }
        runtimes.push(Runtime {
            name: format!("{} {version}", platform.name()),
            id,
            platform,
            version: version_parts(&version),
            types,
        });
    }
    runtimes.sort_by(|a, b| {
        let order = |p: Platform| Platform::ALL.iter().position(|x| *x == p);
        order(a.platform)
            .cmp(&order(b.platform))
            .then_with(|| b.version.cmp(&a.version))
    });
    Ok(Catalog { runtimes })
}

// ---------------------------------------------------------------------------------------------
// The changes, as simctl commands

/// A change to the simulators, as one `simctl` command.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Op {
    Create {
        name: String,
        device_type: String,
        runtime: String,
    },
    Rename {
        udid: String,
        name: String,
    },
    Erase {
        udid: String,
    },
    Duplicate {
        udid: String,
        name: String,
    },
    Delete {
        udid: String,
    },
    DeleteUnavailable,
}

impl Op {
    /// Its `simctl` arguments.
    fn args(&self) -> Vec<String> {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect();
        match self {
            Op::Create {
                name,
                device_type,
                runtime,
            } => v(&["create", name, device_type, runtime]),
            Op::Rename { udid, name } => v(&["rename", udid, name]),
            Op::Erase { udid } => v(&["erase", udid]),
            Op::Duplicate { udid, name } => v(&["clone", udid, name]),
            Op::Delete { udid } => v(&["delete", udid]),
            Op::DeleteUnavailable => v(&["delete", "unavailable"]),
        }
    }

    /// The device it acts on.
    fn udid(&self) -> Option<&str> {
        match self {
            Op::Rename { udid, .. }
            | Op::Erase { udid }
            | Op::Duplicate { udid, .. }
            | Op::Delete { udid } => Some(udid),
            Op::Create { .. } | Op::DeleteUnavailable => None,
        }
    }

    /// What the device shows meanwhile.
    fn doing(&self) -> &'static str {
        match self {
            Op::Create { .. } => "Creating…",
            Op::Rename { .. } => "Renaming…",
            Op::Erase { .. } => "Erasing…",
            Op::Duplicate { .. } => "Duplicating…",
            Op::Delete { .. } | Op::DeleteUnavailable => "Deleting…",
        }
    }

    fn failed(&self) -> &'static str {
        match self {
            Op::Create { .. } => "Couldn't create the device",
            Op::Rename { .. } => "Couldn't rename it",
            Op::Erase { .. } => "Couldn't erase it",
            Op::Duplicate { .. } => "Couldn't duplicate it",
            Op::Delete { .. } => "Couldn't delete it",
            Op::DeleteUnavailable => "Couldn't delete the unavailable devices",
        }
    }
}

/// Run `op`; what simctl printed, trimmed (a new device's UDID).
fn run_op(op: &Op) -> Result<String, String> {
    let args = op.args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_simctl(&args).map(|out| out.trim().to_string())
}

/// Shut `udid` down, fine if it already is.
fn shut_down(udid: &str) -> Result<(), String> {
    match run_simctl(&["shutdown", udid]) {
        Err(e) if !e.contains("current state: Shutdown") => Err(e),
        _ => Ok(()),
    }
}

/// A duplicate's name: "iPhone 17 copy", then "iPhone 17 copy 2", … (a copy of a copy counts on
/// from its original's name rather than growing "copy copy").
pub(super) fn copy_name(name: &str, taken: &[&str]) -> String {
    let mut stem = name.trim();
    if let Some((head, tail)) = stem.rsplit_once(" copy ") {
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            stem = head;
        }
    }
    let stem = stem.strip_suffix(" copy").unwrap_or(stem);
    let base = format!("{stem} copy");
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base} {n}")))
        .find(|c| !taken.contains(&c.as_str()))
        .unwrap_or(base)
}

// ---------------------------------------------------------------------------------------------
// The panel's side

/// What the sheets work from, kept on the panel.
#[derive(Default)]
pub(super) struct Manage {
    catalog: Option<Result<Catalog, String>>,
    loading: bool,
    loaded_at: Option<Instant>,
    /// A platform downloading in the terminal: the catalog's read again until it's in.
    downloading: Option<Platform>,
    /// Devices whose runtime is gone ("Delete unavailable devices").
    pub(super) unavailable: usize,
    /// Why the last change failed, for the sheets.
    error: Option<String>,
}

impl SimulatorPanel {
    fn catalog(&self) -> Option<&Catalog> {
        self.manage.catalog.as_ref()?.as_ref().ok()
    }

    /// The platforms with no runtime, once the catalog's read ("Get more runtimes…").
    pub(super) fn missing_platforms(&self) -> Vec<Platform> {
        self.catalog().map(Catalog::missing).unwrap_or_default()
    }

    /// Read the device types and runtimes again.
    pub(super) fn load_catalog(&mut self, cx: &mut Context<Self>) {
        if self.manage.loading {
            return;
        }
        self.manage.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    run_simctl(&["list", "devicetypes", "runtimes", "-j"])
                        .and_then(|json| parse_catalog(&json))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.manage.loading = false;
                this.manage.loaded_at = Some(Instant::now());
                // A failed re-read keeps what was read before.
                if result.is_ok() || this.catalog().is_none() {
                    this.manage.catalog = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// While a runtime downloads, read the catalog now and then; say so once it's in.
    pub(super) fn poll_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(platform) = self.manage.downloading else {
            return;
        };
        if self
            .catalog()
            .is_some_and(|c| !c.missing().contains(&platform))
        {
            self.manage.downloading = None;
            crate::toast::push(
                window,
                format!(
                    "The {} runtime is in: add a device from the device menu",
                    platform.name()
                ),
                cx,
            );
            cx.notify();
            return;
        }
        if self
            .manage
            .loaded_at
            .is_none_or(|t| t.elapsed() >= DOWNLOAD_POLL)
        {
            self.load_catalog(cx);
        }
    }

    /// Download `platform`'s runtime in Trek's terminal.
    fn download_runtime(&mut self, platform: Platform, cx: &mut Context<Self>) {
        self.manage.downloading = Some(platform);
        self.manage.loaded_at = Some(Instant::now());
        self.workspace.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::RunInTerminal {
                command: platform.download_command(),
                cwd: None,
            })
        });
        cx.notify();
    }

    /// The device this tool shows lets go of `udid` (it's about to shut down).
    fn let_go(&mut self, udid: &str) {
        if self.link.as_ref().is_some_and(|l| l.udid == udid) {
            self.drop_link();
        }
        if self.frame.as_ref().is_some_and(|f| f.udid == udid) {
            if let Some(f) = self.frame.take() {
                self.stale.push(f.image);
            }
        }
    }

    /// Run `op`, its device marked as busy meanwhile, and `then` with what it printed. With
    /// `shut_down_first` a booted device is shut down before (erase and clone need it) and
    /// booted again after.
    fn change(
        &mut self,
        op: Op,
        shut_down_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let udid = op.udid().map(str::to_string);
        if let Some(u) = &udid {
            if self.pending.contains_key(u) {
                return;
            }
            self.pending.insert(u.clone(), op.doing());
        }
        let reboot = shut_down_first
            && udid
                .as_ref()
                .is_some_and(|u| self.devices.iter().any(|d| &d.udid == u && d.booted()));
        if reboot {
            if let Some(u) = &udid {
                self.let_go(u);
            }
        }
        self.manage.error = None;
        cx.notify();
        let failed = op.failed();
        let work = move || {
            if reboot {
                if let Some(u) = op.udid() {
                    shut_down(u)?;
                }
            }
            run_op(&op)
        };
        self.background(window, cx, work, move |this, result, window, cx| {
            if let Some(u) = &udid {
                this.pending.remove(u);
            }
            match result {
                Ok(out) => then(this, out, window, cx),
                Err(e) => this.manage.error = Some(format!("{failed}: {e}")),
            }
            if reboot {
                if let Some(u) = udid {
                    this.start_boot(u, window, cx);
                }
            }
            this.refresh_devices(window, cx);
            cx.notify();
        });
    }

    fn device(&self, udid: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.udid == udid)
    }

    /// A device "Add device…" made: listed, booted and shown.
    fn created(&mut self, device: Device, window: &mut Window, cx: &mut Context<Self>) {
        let (udid, name) = (device.udid.clone(), device.name.clone());
        if self.device(&udid).is_none() {
            self.devices.push(device);
        }
        crate::toast::push(window, format!("Created {name}"), cx);
        self.boot(udid, window, cx);
    }

    fn rename(&mut self, udid: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() || self.device(&udid).is_some_and(|d| d.name == name) {
            return;
        }
        let op = Op::Rename {
            udid: udid.clone(),
            name: name.clone(),
        };
        self.change(op, false, window, cx, move |this, _, _, _| {
            if let Some(d) = this.devices.iter_mut().find(|d| d.udid == udid) {
                d.name = name;
            }
        });
    }

    fn duplicate(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.device(&udid) else { return };
        let taken: Vec<&str> = self.devices.iter().map(|d| d.name.as_str()).collect();
        let name = copy_name(&d.name, &taken);
        let op = Op::Duplicate {
            udid,
            name: name.clone(),
        };
        self.change(op, true, window, cx, move |_, _, window, cx| {
            crate::toast::push(window, format!("Made {name}"), cx);
        });
    }

    fn erase(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.device(&udid).map(|d| d.name.clone()) else {
            return;
        };
        self.change(
            Op::Erase { udid },
            true,
            window,
            cx,
            move |_, _, window, cx| {
                crate::toast::push(window, format!("Erased {name}"), cx);
            },
        );
    }

    /// Whether `udid` can be deleted: not while it's the device on show and running.
    fn deletable(&self, udid: &str) -> bool {
        !(self.selected.as_deref() == Some(udid) && self.device(udid).is_some_and(Device::booted))
    }

    fn delete(&mut self, udid: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.deletable(&udid) {
            return;
        }
        let Some(name) = self.device(&udid).map(|d| d.name.clone()) else {
            return;
        };
        self.let_go(&udid);
        let op = Op::Delete { udid: udid.clone() };
        self.change(op, false, window, cx, move |this, _, window, cx| {
            this.devices.retain(|d| d.udid != udid);
            this.dark.remove(&udid);
            if this.selected.as_deref() == Some(udid.as_str()) {
                this.selected = None;
            }
            crate::toast::push(window, format!("Deleted {name}"), cx);
        });
    }

    fn delete_unavailable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.manage.unavailable;
        self.change(
            Op::DeleteUnavailable,
            false,
            window,
            cx,
            move |this, _, window, cx| {
                this.manage.unavailable = 0;
                let what = if n == 1 { "device" } else { "devices" };
                crate::toast::push(window, format!("Deleted {n} unavailable {what}"), cx);
            },
        );
    }

    /// Below the device list: add one, or manage them.
    pub(super) fn add_device_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.entity();
        let manage = panel.clone();
        h_flex()
            .pt_2()
            .gap_1()
            .child(
                Button::new("sim-add-device")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add device…")
                    .on_click(move |_, window, cx| open_add(panel.clone(), None, window, cx)),
            )
            .child(
                Button::new("sim-manage-devices")
                    .ghost()
                    .small()
                    .icon(IconName::Settings2)
                    .label("Manage devices…")
                    .on_click(move |_, window, cx| open_manage(manage.clone(), window, cx)),
            )
            .into_any_element()
    }

    /// The panel with no simulators at all.
    pub(super) fn no_devices(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.entity();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .px_6()
            .gap_2()
            .text_center()
            .child(div().text_sm().font_medium().child("No simulators yet"))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Add an iPhone, an iPad or any device Xcode has a runtime for."),
            )
            .child(
                Button::new("sim-add-first")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add device…")
                    .on_click(move |_, window, cx| open_add(panel.clone(), None, window, cx)),
            )
            .into_any_element()
    }
}

/// The device menu's own entries, above its devices.
pub(super) fn menu_entries(
    menu: PopupMenu,
    panel: &WeakEntity<SimulatorPanel>,
    missing: &[Platform],
) -> PopupMenu {
    let add = panel.clone();
    let mut menu = menu.item(
        PopupMenuItem::new("Add device…")
            .icon(IconName::Plus)
            .on_click(move |_, window, cx| {
                if let Some(p) = add.upgrade() {
                    open_add(p, None, window, cx);
                }
            }),
    );
    if let Some(platform) = missing.first().copied() {
        let more = panel.clone();
        menu = menu.item(
            PopupMenuItem::new("Get more runtimes…")
                .icon(Lucide::Download)
                .on_click(move |_, window, cx| {
                    if let Some(p) = more.upgrade() {
                        open_add(p, Some(platform.family()), window, cx);
                    }
                }),
        );
    }
    let manage = panel.clone();
    menu.item(
        PopupMenuItem::new("Manage devices…")
            .icon(IconName::Settings2)
            .on_click(move |_, window, cx| {
                if let Some(p) = manage.upgrade() {
                    open_manage(p, window, cx);
                }
            }),
    )
}

// ---------------------------------------------------------------------------------------------
// Pieces of both sheets

fn title(text: &'static str) -> Div {
    div().text_size(px(16.)).font_semibold().child(text)
}

fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.5))
        .line_height(relative(1.5))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn label(text: &'static str, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn error_line(id: &'static str, text: String, cx: &App) -> AnyElement {
    h_flex()
        .id(id)
        .test_support()
        .gap(px(6.))
        .text_size(px(12.5))
        .text_color(crate::palette::red(cx))
        .child(Icon::new(IconName::CircleAlert).xsmall())
        .child(div().flex_1().min_w_0().child(text))
        .into_any_element()
}

/// A list on a sheet: the composer menus' card, inset 4.
fn list_card(id: &'static str, cx: &App) -> Stateful<Div> {
    v_flex()
        .id(id)
        .p(px(4.))
        .rounded(px(12.))
        .border_1()
        .border_color(look::hairline(cx))
        .bg(look::card_fill(cx))
        .overflow_y_scroll()
}

fn footer(cx: &App) -> Div {
    h_flex()
        .gap(px(8.))
        .pt(px(12.))
        .border_t_1()
        .border_color(look::hairline(cx))
}

// ---------------------------------------------------------------------------------------------
// Add device…

/// Open "Add device…", on `family` when given.
fn open_add(
    panel: Entity<SimulatorPanel>,
    family: Option<Family>,
    window: &mut Window,
    cx: &mut App,
) {
    panel.update(cx, |p, cx| p.load_catalog(cx));
    let sheet = cx.new(|cx| AddSheet::new(panel, family, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(500.)).p(px(0.)).child(sheet.clone())
    });
}

struct AddSheet {
    panel: Entity<SimulatorPanel>,
    family: Family,
    runtime: Option<String>,
    device_type: Option<String>,
    name: Entity<InputState>,
    /// The name last filled in from the device type: until it's typed over, it follows the type.
    default_name: String,
    creating: bool,
    error: Option<String>,
    /// What of the panel this sheet last drew (it redraws only when that changes).
    seen: (Option<Instant>, bool, Option<Platform>),
    _subscriptions: Vec<Subscription>,
}

impl AddSheet {
    fn new(
        panel: Entity<SimulatorPanel>,
        family: Option<Family>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Device name"));
        let subs = vec![
            cx.observe_in(&panel, window, |this: &mut Self, panel, window, cx| {
                let m = &panel.read(cx).manage;
                let seen = (m.loaded_at, m.loading, m.downloading);
                if seen != this.seen {
                    this.seen = seen;
                    this.sync(window, cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &name,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.create(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            panel,
            family: family.unwrap_or(Family::IPhone),
            runtime: None,
            device_type: None,
            name,
            default_name: String::new(),
            creating: false,
            error: None,
            seen: (None, false, None),
            _subscriptions: subs,
        };
        this.sync(window, cx);
        this
    }

    /// Keep the choice one the catalog can make: a runtime of the family (the newest), a device
    /// type it runs (the first), and the name following the type until it's typed over.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (runtime, device_type, type_name) = {
            let panel = self.panel.read(cx);
            let Some(catalog) = panel.catalog() else {
                return;
            };
            let runtimes = catalog.runtimes(self.family);
            let runtime = self
                .runtime
                .clone()
                .filter(|r| runtimes.iter().any(|x| &x.id == r))
                .or_else(|| runtimes.first().map(|r| r.id.clone()));
            let types = runtime
                .as_deref()
                .map(|r| catalog.types(self.family, r))
                .unwrap_or_default();
            let device_type = self
                .device_type
                .clone()
                .filter(|t| types.iter().any(|x| &x.id == t))
                .or_else(|| types.first().map(|t| t.id.clone()));
            let type_name = types
                .iter()
                .find(|t| Some(&t.id) == device_type.as_ref())
                .map(|t| t.name.clone())
                .unwrap_or_default();
            (runtime, device_type, type_name)
        };
        self.runtime = runtime;
        self.device_type = device_type;
        let current = self.name.read(cx).value().to_string();
        if (current.is_empty() || current == self.default_name) && current != type_name {
            let value = type_name.clone();
            self.name.update(cx, |i, cx| i.set_value(value, window, cx));
        }
        self.default_name = type_name;
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        let (Some(runtime), Some(device_type)) = (self.runtime.clone(), self.device_type.clone())
        else {
            return;
        };
        if name.is_empty() {
            self.error = Some("Give the device a name.".into());
            cx.notify();
            return;
        }
        self.creating = true;
        self.error = None;
        cx.notify();
        let op = Op::Create {
            name: name.clone(),
            device_type: device_type.clone(),
            runtime: runtime.clone(),
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { run_op(&op) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating = false;
                match result {
                    Ok(udid) if !udid.is_empty() => {
                        let device = Device {
                            name,
                            udid,
                            runtime: pretty_runtime(&runtime),
                            state: "Shutdown".into(),
                            device_type,
                        };
                        this.panel.update(cx, |p, cx| p.created(device, window, cx));
                        window.close_dialog(cx);
                    }
                    Ok(_) => this.error = Some("simctl didn't say which device it made.".into()),
                    Err(e) => this.error = Some(format!("Couldn't create the device: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runtime, device type and name, for a family that has a runtime.
    fn chooser(&self, catalog: &Catalog, cx: &mut Context<Self>) -> AnyElement {
        let runtimes = catalog.runtimes(self.family);
        let runtime_ids: Vec<String> = runtimes.iter().map(|r| r.id.clone()).collect();
        let current = runtime_ids
            .iter()
            .position(|r| Some(r) == self.runtime.as_ref())
            .unwrap_or(0);
        let view = cx.entity();
        let pick_runtime = move |ix: usize, window: &mut Window, cx: &mut App| {
            let id = runtime_ids.get(ix).cloned();
            view.update(cx, |this, cx| {
                this.runtime = id;
                this.sync(window, cx);
                cx.notify();
            });
        };
        let os: AnyElement = if runtimes.len() > 1 {
            look::segmented(
                "sim-add-runtime",
                runtimes
                    .iter()
                    .enumerate()
                    .map(|(i, r)| (i, SharedString::from(r.name.clone())))
                    .collect(),
                current,
                pick_runtime,
                cx,
            )
        } else {
            div()
                .text_size(px(13.))
                .child(runtimes.first().map(|r| r.name.clone()).unwrap_or_default())
                .into_any_element()
        };
        let types = self
            .runtime
            .as_deref()
            .map(|r| catalog.types(self.family, r))
            .unwrap_or_default();
        let muted = cx.theme().muted_foreground;
        let rows = types.into_iter().enumerate().map(|(ix, t)| {
            let selected = self.device_type.as_ref() == Some(&t.id);
            let id = t.id.clone();
            look::row(("sim-add-type", ix), selected, cx)
                .test_support()
                .child(t.family.icon().small().text_color(muted))
                .child(div().flex_1().min_w_0().truncate().child(t.name.clone()))
                .when(selected, |el| el.child(Icon::new(IconName::Check).xsmall()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.device_type = Some(id.clone());
                    this.error = None;
                    this.sync(window, cx);
                    cx.notify();
                }))
        });
        v_flex()
            .gap(px(12.))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(label("Runs", cx))
                    .child(h_flex().child(os)),
            )
            .child(
                v_flex().gap(px(6.)).child(label("Device", cx)).child(
                    list_card("sim-add-types", cx)
                        .max_h(px(232.))
                        .test_support()
                        .children(rows),
                ),
            )
            .child(
                v_flex().gap(px(6.)).child(label("Name", cx)).child(
                    div()
                        .id("sim-add-name")
                        .test_support()
                        .child(Input::new(&self.name)),
                ),
            )
            .into_any_element()
    }

    /// For a family with no runtime: what's missing, and the download.
    fn get_more(
        &self,
        platform: Platform,
        downloading: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("sim-add-runtimes")
            .test_support()
            .p(px(14.))
            .gap(px(8.))
            .rounded(px(12.))
            .bg(look::chip_fill(cx))
            .child(
                h_flex()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .child(
                        Icon::new(Lucide::Download)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .font_medium()
                            .child(format!("No {} runtime installed", platform.name())),
                    ),
            )
            .child(note(
                format!(
                    "{} simulators run on the {} simulator runtime, which Xcode downloads and installs: several gigabytes, so it can take a while. Its progress shows in Trek's terminal, and this list picks it up once it's in.",
                    self.family.label(),
                    platform.name()
                ),
                cx,
            ))
            .child(
                div()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(7.))
                    .bg(look::card_fill(cx))
                    .border_1()
                    .border_color(look::hairline(cx))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(12.))
                    .child(platform.download_command()),
            )
            .when(downloading, |el| {
                el.child(
                    h_flex()
                        .id("sim-add-downloading")
                        .test_support()
                        .gap(px(8.))
                        .child(Spinner::new().xsmall())
                        .child(note(
                            "Downloading in the terminal. This list looks again every few seconds.",
                            cx,
                        )),
                )
            })
            .into_any_element()
    }
}

impl Render for AddSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (catalog, loading, downloading) = {
            let m = &self.panel.read(cx).manage;
            (m.catalog.clone(), m.loading, m.downloading)
        };
        let platform = self.family.platform();
        let view = cx.entity();
        let pick_family = move |family: Family, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.family = family;
                this.error = None;
                this.sync(window, cx);
                cx.notify();
            })
        };
        let has_runtime = catalog
            .as_ref()
            .and_then(|c| c.as_ref().ok())
            .is_some_and(|c| !c.runtimes(self.family).is_empty());
        let body: AnyElement = match &catalog {
            None => h_flex()
                .gap(px(8.))
                .child(Spinner::new().xsmall())
                .child(note("Reading Xcode's simulators…", cx))
                .into_any_element(),
            Some(Err(e)) => error_line(
                "sim-add-catalog-error",
                format!("Couldn't read Xcode's simulators: {e}"),
                cx,
            ),
            Some(Ok(c)) if has_runtime => self.chooser(c, cx),
            Some(Ok(_)) => self.get_more(platform, downloading == Some(platform), cx),
        };
        let primary: AnyElement = if has_runtime {
            Button::new("sim-add-create")
                .primary()
                .label(if self.creating {
                    "Creating…"
                } else {
                    "Create"
                })
                .loading(self.creating)
                .disabled(self.creating || self.device_type.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))
                .into_any_element()
        } else if catalog.as_ref().is_some_and(|c| c.is_ok()) && downloading != Some(platform) {
            let panel = self.panel.clone();
            Button::new("sim-add-download")
                .primary()
                .icon(Lucide::Download)
                .label(format!("Download {}", platform.name()))
                .on_click(move |_, window, cx| {
                    panel.update(cx, |p, cx| p.download_runtime(platform, cx));
                    // The terminal's under the sheet: out of its way.
                    window.close_dialog(cx);
                    crate::toast::push(
                        window,
                        format!(
                            "Downloading {} in the terminal: Trek says when it's in",
                            platform.name()
                        ),
                        cx,
                    );
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };
        v_flex()
            .id("sim-add-sheet")
            .test_support()
            // Return creates (the dialog would only close).
            .on_action(cx.listener(|this, _: &Enter, window, cx| {
                if this.device_type.is_some() {
                    this.create(window, cx);
                }
            }))
            .w_full()
            .p(px(PAD))
            .gap(px(14.))
            .child(title("Add a simulator"))
            .child(note(
                "Pick a device and the OS it runs. Trek creates it, boots it and shows it here.",
                cx,
            ))
            .child(h_flex().child(look::segmented(
                "sim-add-family",
                Family::ALL.iter().map(|f| (*f, f.short())).collect(),
                self.family,
                pick_family,
                cx,
            )))
            .child(body)
            .when(loading && catalog.is_some(), |el| {
                el.child(
                    h_flex()
                        .gap(px(8.))
                        .child(Spinner::new().xsmall())
                        .child(note("Looking for new runtimes…", cx)),
                )
            })
            .when_some(self.error.clone(), |el, e| {
                el.child(error_line("sim-add-error", e, cx))
            })
            .child(
                footer(cx)
                    .justify_end()
                    .child(
                        Button::new("sim-add-cancel")
                            .outline()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(primary),
            )
    }
}

// ---------------------------------------------------------------------------------------------
// Manage devices…

/// Open "Manage devices…".
fn open_manage(panel: Entity<SimulatorPanel>, window: &mut Window, cx: &mut App) {
    panel.update(cx, |p, cx| {
        p.manage.error = None;
        p.last_list = None;
        cx.notify();
    });
    let sheet = cx.new(|cx| ManageSheet::new(panel, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(560.)).p(px(0.)).child(sheet.clone())
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Confirm {
    Erase,
    Delete,
}

/// What of the panel the manage sheet draws.
#[derive(Clone, PartialEq, Default)]
struct Seen {
    devices: Vec<Device>,
    pending: Vec<(String, &'static str)>,
    selected: Option<String>,
    unavailable: usize,
    error: Option<String>,
}

impl Seen {
    fn of(p: &SimulatorPanel) -> Self {
        let mut pending: Vec<_> = p.pending.iter().map(|(k, v)| (k.clone(), *v)).collect();
        pending.sort();
        Self {
            devices: p.devices.clone(),
            pending,
            selected: p.selected.clone(),
            unavailable: p.manage.unavailable,
            error: p.manage.error.clone(),
        }
    }
}

struct ManageSheet {
    panel: Entity<SimulatorPanel>,
    /// The device being renamed, and its field.
    renaming: Option<(String, Entity<InputState>)>,
    /// The device whose erase or delete waits on a yes.
    confirming: Option<(String, Confirm)>,
    seen: Seen,
    _subscriptions: Vec<Subscription>,
}

impl ManageSheet {
    fn new(panel: Entity<SimulatorPanel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe_in(&panel, window, |this: &mut Self, panel, _, cx| {
                // The panel redraws with every frame of the mirror; this only when a device did.
                let seen = Seen::of(panel.read(cx));
                if seen != this.seen {
                    this.seen = seen;
                    cx.notify();
                }
            }),
        ];
        let seen = Seen::of(panel.read(cx));
        Self {
            panel,
            renaming: None,
            confirming: None,
            seen,
            _subscriptions: subs,
        }
    }

    fn start_rename(
        &mut self,
        udid: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Device name"));
        input.update(cx, |s, cx| s.set_value(name, window, cx));
        let id = udid.clone();
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_rename(&id, window, cx);
                }
            },
        ));
        let focus = input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| {
                s.focus(window, cx);
                let len = s.value().len();
                s.set_selected_range(0..len, cx);
            })
        });
        self.confirming = None;
        self.renaming = Some((udid, input));
        cx.notify();
    }

    fn save_rename(&mut self, udid: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming.take().filter(|(id, _)| id == udid) else {
            return;
        };
        let name = input.read(cx).value().to_string();
        self.panel
            .update(cx, |p, cx| p.rename(id, name, window, cx));
        cx.notify();
    }

    fn row(&self, ix: usize, d: &Device, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let p = self.panel.read(cx);
        let pending = p.pending.get(&d.udid).copied();
        let shown = p.selected.as_deref() == Some(d.udid.as_str());
        let deletable = p.deletable(&d.udid);
        let booted = d.booted();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(id, _)| *id == d.udid)
            .map(|(_, input)| input.clone());
        let confirming = self
            .confirming
            .as_ref()
            .filter(|(id, _)| *id == d.udid)
            .map(|(_, c)| *c);
        let udid = d.udid.clone();
        let state = h_flex()
            .gap(px(5.))
            .text_size(px(12.))
            .text_color(muted)
            .child(d.runtime.clone())
            .child("·")
            .when(booted, |el| {
                el.child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(crate::palette::emerald(cx)),
                )
            })
            .child(if booted { "Booted" } else { "Shut down" })
            .when(shown, |el| el.child("·").child("On show"));
        let name: AnyElement = match &renaming {
            Some(input) => div()
                .id(("sim-m-name-input", ix))
                .test_support()
                .child(Input::new(input).small())
                .into_any_element(),
            None => div()
                .text_size(px(13.))
                .truncate()
                .child(d.name.clone())
                .into_any_element(),
        };
        let actions: AnyElement = if let Some(label) = pending {
            h_flex()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(muted)
                .child(Spinner::new().xsmall())
                .child(label)
                .into_any_element()
        } else if renaming.is_some() {
            let save = udid.clone();
            h_flex()
                .gap(px(6.))
                .child(
                    Button::new(("sim-m-rename-cancel", ix))
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.renaming = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(("sim-m-rename-save", ix))
                        .primary()
                        .small()
                        .label("Save")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.save_rename(&save, window, cx)
                        })),
                )
                .into_any_element()
        } else {
            let (power, ren, dup, era, del) = (
                udid.clone(),
                (udid.clone(), d.name.clone()),
                udid.clone(),
                udid.clone(),
                udid.clone(),
            );
            h_flex()
                .gap(px(2.))
                .child(
                    crate::ui::icon_button(
                        ("sim-m-power", ix),
                        Lucide::Power,
                        if booted { "Shut down" } else { "Boot" },
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let udid = power.clone();
                        this.panel.update(cx, |p, cx| {
                            if booted {
                                p.shutdown(udid, window, cx)
                            } else {
                                p.boot(udid, window, cx)
                            }
                        });
                    })),
                )
                .child(
                    crate::ui::icon_button(("sim-m-rename", ix), Lucide::Pencil, "Rename")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_rename(ren.0.clone(), ren.1.clone(), window, cx)
                        })),
                )
                .child(
                    crate::ui::icon_button(
                        ("sim-m-duplicate", ix),
                        IconName::Copy,
                        if booted {
                            "Duplicate (shuts it down while it's copied)"
                        } else {
                            "Duplicate"
                        },
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let udid = dup.clone();
                        this.panel.update(cx, |p, cx| p.duplicate(udid, window, cx));
                    })),
                )
                .child(
                    crate::ui::icon_button(("sim-m-erase", ix), Lucide::Eraser, "Erase…").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.renaming = None;
                            this.confirming = Some((era.clone(), Confirm::Erase));
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    crate::ui::icon_button(
                        ("sim-m-delete", ix),
                        Lucide::Trash,
                        if deletable {
                            "Delete…"
                        } else {
                            "Shut it down to delete it: it's the one on show"
                        },
                    )
                    .disabled(!deletable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.renaming = None;
                        this.confirming = Some((del.clone(), Confirm::Delete));
                        cx.notify();
                    })),
                )
                .into_any_element()
        };
        let confirm = confirming.filter(|_| pending.is_none()).map(|what| {
            let (question, verb) = match what {
                Confirm::Erase => (
                    format!(
                        "Erase “{}”? Its apps, data and settings are wiped, as on a new device.{}",
                        d.name,
                        if booted {
                            " It shuts down for it and starts again after."
                        } else {
                            ""
                        }
                    ),
                    "Erase",
                ),
                Confirm::Delete => (
                    format!(
                        "Delete “{}”? The simulator and everything on it go. This can't be undone.",
                        d.name
                    ),
                    "Delete",
                ),
            };
            let target = udid.clone();
            h_flex()
                .id(("sim-m-confirm", ix))
                .test_support()
                .mt(px(4.))
                .ml(px(30.))
                .gap(px(8.))
                .child(div().flex_1().min_w_0().child(note(question, cx)))
                .child(
                    Button::new(("sim-m-confirm-cancel", ix))
                        .ghost()
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(("sim-m-confirm-yes", ix))
                        .danger()
                        .small()
                        .label(verb)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.confirming = None;
                            let udid = target.clone();
                            this.panel.update(cx, |p, cx| match what {
                                Confirm::Erase => p.erase(udid, window, cx),
                                Confirm::Delete => p.delete(udid, window, cx),
                            });
                            cx.notify();
                        })),
                )
        });
        let fill = look::chip_fill(cx);
        v_flex()
            .id(("sim-m-row", ix))
            .test_support()
            .px(px(9.))
            .py(px(6.))
            .rounded(px(7.))
            .when(confirming.is_some(), |el| el.bg(fill))
            .hover(move |s| s.bg(fill))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(d.icon().small().text_color(muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(name)
                            .child(state),
                    )
                    .child(actions),
            )
            .children(confirm)
            .into_any_element()
    }
}

impl Render for ManageSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (devices, unavailable, error) = {
            let p = self.panel.read(cx);
            (
                p.devices.clone(),
                p.manage.unavailable,
                p.manage.error.clone(),
            )
        };
        // A device that went (deleted here or elsewhere) takes its open question with it.
        if self
            .confirming
            .as_ref()
            .is_some_and(|(id, _)| !devices.iter().any(|d| &d.udid == id))
        {
            self.confirming = None;
        }
        let mut list = list_card("sim-m-list", cx).max_h(px(380.));
        let mut ix = 0;
        for family in Family::ALL {
            let group: Vec<&Device> = devices.iter().filter(|d| d.family() == family).collect();
            if group.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .px(px(9.))
                    .pt(px(6.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .font_medium()
                    .text_color(theme.muted_foreground)
                    .child(family.label()),
            );
            for d in group {
                list = list.child(self.row(ix, d, cx));
                ix += 1;
            }
        }
        if devices.is_empty() {
            list = list.child(div().p(px(10.)).child(note("No simulators yet.", cx)));
        }
        let panel = self.panel.clone();
        let add = self.panel.clone();
        v_flex()
            .id("sim-manage-sheet")
            .test_support()
            // Return saves a rename (the dialog would only close); otherwise it closes.
            .on_action(cx.listener(|this, _: &Enter, window, cx| {
                match this.renaming.as_ref().map(|(id, _)| id.clone()) {
                    Some(id) => this.save_rename(&id, window, cx),
                    None => cx.propagate(),
                }
            }))
            .w_full()
            .p(px(PAD))
            .gap(px(14.))
            .child(title("Simulators"))
            .child(note(
                "Every simulator Xcode has, including ones made outside Trek. Changes here act on the simulator itself, as Xcode's Devices window does.",
                cx,
            ))
            .child(list)
            .when_some(error, |el, e| el.child(error_line("sim-m-error", e, cx)))
            .child(
                footer(cx)
                    .child(
                        Button::new("sim-m-unavailable")
                            .ghost()
                            .small()
                            .icon(Lucide::Trash)
                            .label(match unavailable {
                                0 => "No unavailable devices".to_string(),
                                1 => "Delete 1 unavailable device".to_string(),
                                n => format!("Delete {n} unavailable devices"),
                            })
                            .disabled(unavailable == 0)
                            .tooltip("Devices whose runtime is no longer installed")
                            .on_click(move |_, window, cx| {
                                panel.update(cx, |p, cx| p.delete_unavailable(window, cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("sim-m-add")
                            .outline()
                            .icon(IconName::Plus)
                            .label("Add device…")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let add = add.clone();
                                window.defer(cx, move |window, cx| {
                                    open_add(add, None, window, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new("sim-m-done")
                            .primary()
                            .label("Done")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Catalog, Family, Op, Platform, copy_name, parse_catalog};

    /// Trimmed from Xcode 27's `simctl list devicetypes runtimes -j`: iOS 27 and 26 (the
    /// newer runs the iPhone 18 Pro the older can't), an unavailable watchOS, and a tvOS
    /// runtime whose supported types leave out their family.
    const CATALOG: &str = r#"{
      "devicetypes": [
        {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro", "name": "iPhone 18 Pro", "productFamily": "iPhone"},
        {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17", "name": "iPhone 17", "productFamily": "iPhone"},
        {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M4", "name": "iPad Air 11-inch (M4)", "productFamily": "iPad"},
        {"identifier": "com.apple.CoreSimulator.SimDeviceType.Apple-TV-4K-3rd-generation-4K", "name": "Apple TV 4K (3rd generation)", "productFamily": "Apple TV"},
        {"identifier": "com.apple.CoreSimulator.SimDeviceType.Apple-Watch-Series-12-46mm", "name": "Apple Watch Series 12 (46mm)", "productFamily": "Apple Watch"}
      ],
      "runtimes": [
        {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-0", "platform": "iOS", "version": "26.0", "isAvailable": true, "name": "iOS 26.0",
         "supportedDeviceTypes": [
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17", "name": "iPhone 17", "productFamily": "iPhone"},
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M4", "name": "iPad Air 11-inch (M4)", "productFamily": "iPad"}
         ]},
        {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-27-0", "platform": "iOS", "version": "27.0", "isAvailable": true, "name": "iOS 27.0",
         "supportedDeviceTypes": [
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro", "name": "iPhone 18 Pro", "productFamily": "iPhone"},
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17", "name": "iPhone 17", "productFamily": "iPhone"},
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M4", "name": "iPad Air 11-inch (M4)", "productFamily": "iPad"},
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.Apple-TV-4K-3rd-generation-4K", "name": "Apple TV 4K (3rd generation)", "productFamily": "Apple TV"}
         ]},
        {"identifier": "com.apple.CoreSimulator.SimRuntime.watchOS-13-0", "platform": "watchOS", "version": "13.0", "isAvailable": false,
         "supportedDeviceTypes": [
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.Apple-Watch-Series-12-46mm", "name": "Apple Watch Series 12 (46mm)", "productFamily": "Apple Watch"}
         ]},
        {"identifier": "com.apple.CoreSimulator.SimRuntime.tvOS-27-0", "platform": "tvOS", "version": "27.0", "isAvailable": true,
         "supportedDeviceTypes": [
           {"identifier": "com.apple.CoreSimulator.SimDeviceType.Apple-TV-4K-3rd-generation-4K"}
         ]}
      ]
    }"#;

    fn catalog() -> Catalog {
        parse_catalog(CATALOG).unwrap()
    }

    #[test]
    fn runtimes_are_available_ones_newest_first() {
        let c = catalog();
        let ios: Vec<_> = c
            .runtimes(Family::IPhone)
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(ios, ["iOS 27.0", "iOS 26.0"]);
        // The watchOS runtime is unavailable: none to offer, so it's one to download.
        assert!(c.runtimes(Family::Watch).is_empty());
        assert_eq!(c.missing(), [Platform::WatchOs, Platform::VisionOs]);
    }

    #[test]
    fn only_types_a_runtime_supports_are_offered() {
        let c = catalog();
        let names = |family, runtime| -> Vec<String> {
            c.types(family, runtime)
                .iter()
                .map(|t| t.name.clone())
                .collect()
        };
        let ios27 = "com.apple.CoreSimulator.SimRuntime.iOS-27-0";
        let ios26 = "com.apple.CoreSimulator.SimRuntime.iOS-26-0";
        assert_eq!(names(Family::IPhone, ios27), ["iPhone 18 Pro", "iPhone 17"]);
        assert_eq!(names(Family::IPhone, ios26), ["iPhone 17"]);
        assert_eq!(names(Family::IPad, ios26), ["iPad Air 11-inch (M4)"]);
        // An iOS runtime listing a TV type doesn't make it an iOS device.
        assert!(
            c.runtimes(Family::Tv)
                .iter()
                .all(|r| r.platform == Platform::TvOs)
        );
        // tvOS's entry left the name and family out: the device type list fills them in.
        assert_eq!(
            names(Family::Tv, "com.apple.CoreSimulator.SimRuntime.tvOS-27-0"),
            ["Apple TV 4K (3rd generation)"]
        );
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(parse_catalog("not json").is_err());
        assert_eq!(parse_catalog("{}").unwrap(), Catalog::default());
    }

    #[test]
    fn commands() {
        let args = |op: Op| op.args().join(" | ");
        assert_eq!(
            args(Op::Create {
                name: "My Phone".into(),
                device_type: "com.apple.CoreSimulator.SimDeviceType.iPhone-17".into(),
                runtime: "com.apple.CoreSimulator.SimRuntime.iOS-27-0".into(),
            }),
            "create | My Phone | com.apple.CoreSimulator.SimDeviceType.iPhone-17 | com.apple.CoreSimulator.SimRuntime.iOS-27-0"
        );
        assert_eq!(
            args(Op::Rename {
                udid: "U".into(),
                name: "New name".into()
            }),
            "rename | U | New name"
        );
        assert_eq!(args(Op::Erase { udid: "U".into() }), "erase | U");
        assert_eq!(
            args(Op::Duplicate {
                udid: "U".into(),
                name: "X copy".into()
            }),
            "clone | U | X copy"
        );
        assert_eq!(args(Op::Delete { udid: "U".into() }), "delete | U");
        assert_eq!(args(Op::DeleteUnavailable), "delete | unavailable");
        assert_eq!(
            Platform::WatchOs.download_command(),
            "xcodebuild -downloadPlatform watchOS"
        );
        assert_eq!(
            Platform::VisionOs.download_command(),
            "xcodebuild -downloadPlatform visionOS"
        );
    }

    #[test]
    fn copies_are_numbered() {
        assert_eq!(copy_name("iPhone 17", &["iPhone 17"]), "iPhone 17 copy");
        assert_eq!(
            copy_name("iPhone 17", &["iPhone 17", "iPhone 17 copy"]),
            "iPhone 17 copy 2"
        );
        assert_eq!(
            copy_name("iPhone 17 copy", &["iPhone 17", "iPhone 17 copy"]),
            "iPhone 17 copy 2"
        );
        assert_eq!(
            copy_name("iPhone 17 copy 2", &["iPhone 17 copy", "iPhone 17 copy 2"]),
            "iPhone 17 copy 3"
        );
        // A name that only ends in a number keeps it.
        assert_eq!(copy_name("Pixel 9", &[]), "Pixel 9 copy");
    }
}
