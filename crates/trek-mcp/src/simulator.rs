//! `trek-mcp simulator` — iOS Simulator control via `xcrun simctl`, with
//! touch, typing and hardware buttons through the AXe CLI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::outline::{self, Checked, Frame, Node, Refs, View};
use crate::rpc::{
    self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, opt_str, point_schema, text,
};
use crate::util::{self, MAX_IMAGE_SIDE, TempFile, fmt_num};

pub const AXE_MISSING: &str = "Touch input needs AXe. Turn on 'Simulator touch input' in Trek ▸ Settings ▸ Tools to install it.";

const INSTRUCTIONS: &str = "iOS Simulator control. Use `sim_list` to find devices and `sim_boot` to start one. Read \
the screen with `sim_elements` first: an outline whose tappable elements carry refs (e12) for `sim_tap` (a ref lasts \
while its element stays; read again after the screen changes); `sim_wait` waits for text or an element. Take a \
`sim_screenshot` when looks matter: it's sized in device points (downscaled further only if very large) and reports \
the mapping; x/y for sim_tap and sim_swipe are pixel coordinates in that screenshot, as frames are. Every tool takes \
an optional `udid` (default: the booted simulator). Run XCTest or Swift Testing suites with `sim_test` rather than \
`xcodebuild test` in a shell: the user watches each test pass or fail live in Trek's Simulator tool, and you get a \
short report of the failures.";

const SIMCTL_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a `sim_test` run may take before it's stopped.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(1800);
/// Run logs older than this are deleted when a new run starts.
const RUN_LOG_KEEP: Duration = Duration::from_secs(24 * 3600);
const AXE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub udid: String,
    pub runtime: String,
    pub state: String,
    pub device_type: String,
}

/// Screenshot pixel → device point mapping for one simulator.
#[derive(Debug, Clone, Copy)]
struct SimMapping {
    /// Device points per screenshot pixel.
    factor: f64,
}

#[derive(Default)]
pub struct Simulator {
    mappings: HashMap<String, SimMapping>,
    scale_cache: HashMap<String, f64>,
    /// The refs `sim_elements` has given out, per simulator.
    refs: HashMap<String, Refs>,
}

/// How often `sim_wait` reads the screen again.
const WAIT_POLL: Duration = Duration::from_millis(500);

/// A simulator's screen as AXe's `describe-ui` gives it (the accessibility tree as JSON, frames
/// in device points), as outline nodes numbered by where they sit.
pub fn parse_tree(json_text: &str) -> Result<Vec<Node>, String> {
    let v: Value = serde_json::from_str(json_text)
        .map_err(|e| format!("AXe's describe-ui answered with something that isn't JSON: {e}"))?;
    let mut roots: Vec<Node> = match &v {
        Value::Array(items) => items.iter().filter_map(element).collect(),
        other => element(other).into_iter().collect(),
    };
    outline::number(&mut roots);
    Ok(roots)
}

/// One element of AXe's tree. Keys as AXe 1.x writes them, with the names idb and
/// WebDriverAgent use for the same things read too.
fn element(v: &Value) -> Option<Node> {
    let o = v.as_object()?;
    let string = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let kind = string(&["type", "role"]).unwrap_or_default();
    let kind = kind
        .trim_start_matches("XCUIElementType")
        .trim_start_matches("AX");
    let subrole = string(&["subrole"]).unwrap_or_default();
    let secure = kind.to_lowercase().contains("secure") || subrole == "AXSecureTextField";
    let role = if subrole == "AXSearchField" {
        "search field".to_string()
    } else {
        outline::role_words(kind)
    };
    let value = string(&["AXValue", "value"]).filter(|_| !secure);
    let frame = o.get("frame").or_else(|| o.get("rect")).and_then(|f| {
        let n = |k: &str| f.get(k).and_then(Value::as_f64);
        Some(Frame {
            x: n("x")?,
            y: n("y")?,
            w: n("width")?,
            h: n("height")?,
        })
    });
    let children: Vec<Node> = o
        .get("children")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(element).collect())
        .unwrap_or_default();
    let has_actions = o
        .get("custom_actions")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    // What a tap does something to: a control, or a named thing that holds nothing else (a
    // custom view is often all a row is). Words and pictures aren't given a ref: their frame
    // says where they are.
    let control = matches!(
        role.as_str(),
        "button"
            | "link"
            | "text field"
            | "search field"
            | "text view"
            | "switch"
            | "toggle"
            | "slider"
            | "stepper"
            | "cell"
            | "tab"
            | "key"
            | "checkbox"
            | "radio button"
            | "menu item"
            | "picker wheel"
            | "segmented control"
            | "pop up button"
            | "disclosure triangle"
    );
    // Its label; with none, a control or a thing with nothing inside goes by the identifier its
    // app gave it (a container's identifier names nothing an agent reads).
    let name = string(&["AXLabel", "label", "title", "name"])
        .or_else(|| {
            string(&["AXUniqueId", "identifier"]).filter(|_| control || children.is_empty())
        })
        .unwrap_or_default();
    let leaf_thing = children.is_empty()
        && !name.is_empty()
        && matches!(role.as_str(), "other" | "group" | "generic");
    let checked = matches!(role.as_str(), "switch" | "toggle" | "checkbox")
        .then(|| Checked::of(value.as_deref()))
        .flatten();
    let off = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| o.get(*k))
            .is_some_and(|v| v.as_bool() == Some(false) || v.as_str() == Some("false"))
    };
    Some(Node {
        role,
        name,
        // A switch's 1 or 0 is its state, said as checked.
        value: value.filter(|_| checked.is_none()),
        secure,
        disabled: off(&["enabled", "isEnabled"]),
        checked,
        frame,
        acts: control || has_actions || leaf_thing,
        children,
        ..Node::default()
    })
}

/// The screen's size in device points: the application's own frame, which holds everything.
fn screen(roots: &[Node]) -> Option<Frame> {
    roots.iter().find_map(|n| n.frame)
}

/// Device points per `sim_screenshot` pixel for a screen of `points`, as `sim_screenshot` fits
/// it: a point a pixel, unless its long side passes `MAX_IMAGE_SIDE`.
fn fitted(points: Frame) -> f64 {
    let long = points.w.max(points.h).max(1.0);
    long / long.round().min(MAX_IMAGE_SIDE as f64)
}

/// Where ref `r` is tapped on a screen read afresh as `roots`: the centre of the element it
/// names, in device points, once it's found again and proven the same; and what it is, for the
/// reply. Refused when it changed, is disabled, or isn't on screen.
fn tap_point(refs: &Refs, r: &str, roots: &[Node]) -> Result<(String, (f64, f64)), String> {
    let (seen, now) = refs.find(r, roots)?;
    let what = seen.describe();
    if now.disabled {
        return Err(format!("{what} is disabled, so nothing was tapped."));
    }
    let Some(frame) = now.frame else {
        return Err(format!(
            "{what} doesn't say where it is, so there's nowhere to tap. Nothing was done."
        ));
    };
    let centre = frame.centre();
    if screen(roots).is_some_and(|s| !s.holds(centre)) {
        return Err(format!(
            "{what} is off the screen (scrolled out of view), so there's nowhere to tap. Nothing was done: swipe it into view and read the elements again."
        ));
    }
    Ok((what, centre))
}

/// What a wait is for, from a tool's arguments: text (in a name or value) or an element (role
/// or name), to show or to go.
struct Awaited<'a> {
    needle: &'a str,
    by_text: bool,
    gone: bool,
}

impl Awaited<'_> {
    fn what(&self) -> String {
        if self.by_text {
            format!("the text {:?}", self.needle)
        } else {
            format!("an element matching {:?}", self.needle)
        }
    }

    /// Whether `roots` is the screen it waits for.
    fn met(&self, roots: &[Node]) -> bool {
        let there = if self.by_text {
            outline::has_text(roots, self.needle)
        } else {
            outline::has_element(roots, self.needle)
        };
        there != self.gone
    }
}

fn simctl(args: &[&str]) -> Result<String, String> {
    let mut full = vec!["simctl"];
    full.extend_from_slice(args);
    util::run_ok("/usr/bin/xcrun", &full, None, SIMCTL_TIMEOUT)
        .map_err(|e| format!("simctl {}: {e}", args.join(" ")))
}

/// "com.apple.CoreSimulator.SimRuntime.iOS-26-0" → "iOS 26.0"
pub fn pretty_runtime(id: &str) -> String {
    let tail = id.rsplit('.').next().unwrap_or(id);
    match tail.split_once('-') {
        Some((os, ver)) => format!("{os} {}", ver.replace('-', ".")),
        None => tail.to_string(),
    }
}

pub fn parse_devices(json_text: &str) -> Result<Vec<Device>, String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("Bad simctl JSON: {e}"))?;
    let mut out = Vec::new();
    if let Some(map) = v.get("devices").and_then(Value::as_object) {
        for (runtime, list) in map {
            for d in list.as_array().into_iter().flatten() {
                if d.get("isAvailable").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                let s = |k: &str| {
                    d.get(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                out.push(Device {
                    name: s("name"),
                    udid: s("udid"),
                    runtime: pretty_runtime(runtime),
                    state: s("state"),
                    device_type: s("deviceTypeIdentifier"),
                });
            }
        }
    }
    // Booted first, then newest runtime, then name.
    out.sort_by(|a, b| {
        (b.state == "Booted")
            .cmp(&(a.state == "Booted"))
            .then_with(|| version_key(&b.runtime).cmp(&version_key(&a.runtime)))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

fn version_key(runtime: &str) -> (String, Vec<u32>) {
    let (os, ver) = runtime.split_once(' ').unwrap_or((runtime, ""));
    (
        os.to_string(),
        ver.split('.').filter_map(|p| p.parse().ok()).collect(),
    )
}

fn devices() -> Result<Vec<Device>, String> {
    parse_devices(&simctl(&["list", "devices", "available", "-j"])?)
}

fn find_axe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TREK_AXE_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    util::find_executable("axe", &["/opt/homebrew/bin", "/usr/local/bin"])
}

fn axe(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let bin = find_axe().ok_or(AXE_MISSING)?;
    let bin = bin.to_str().ok_or("AXe path is not UTF-8")?;
    util::run_ok(bin, args, stdin, AXE_TIMEOUT)
        .map_err(|e| format!("axe {}: {e}", args.first().unwrap_or(&"")))
}

fn coord(v: f64) -> String {
    format!("{:.1}", v.max(0.0))
}

impl Simulator {
    /// Resolve `udid` (default/"booted" → first booted device).
    fn resolve(&self, args: &Value) -> Result<Device, String> {
        let want = opt_str(args, "udid").map(str::trim);
        let all = devices()?;
        match want {
            None | Some("booted") => all.into_iter().find(|d| d.state == "Booted").ok_or_else(|| {
                "No simulator is booted. Use sim_list to pick a device and sim_boot to start it.".to_string()
            }),
            Some(u) => all
                .into_iter()
                .find(|d| d.udid.eq_ignore_ascii_case(u))
                .ok_or_else(|| format!("No available simulator with UDID {u}. Use sim_list to see devices.")),
        }
    }

    fn resolve_booted(&self, args: &Value) -> Result<Device, String> {
        let d = self.resolve(args)?;
        if d.state != "Booted" {
            return Err(format!(
                "{} ({}) is {}; boot it with sim_boot first.",
                d.name, d.udid, d.state
            ));
        }
        Ok(d)
    }

    /// Screen scale (points → pixels) for a device type, from its CoreSimulator profile.
    fn screen_scale(&mut self, device_type: &str) -> Option<f64> {
        if let Some(s) = self.scale_cache.get(device_type) {
            return Some(*s);
        }
        let types: Value =
            serde_json::from_str(&simctl(&["list", "devicetypes", "-j"]).ok()?).ok()?;
        let bundle = types
            .get("devicetypes")?
            .as_array()?
            .iter()
            .find(|t| t.get("identifier").and_then(Value::as_str) == Some(device_type))?
            .get("bundlePath")?
            .as_str()?
            .to_string();
        let res = PathBuf::from(bundle).join("Contents/Resources");
        let attempts = [
            (
                "capabilities.plist",
                "capabilities.ScreenDimensionsCapability.main-screen-scale",
            ),
            ("profile.plist", "mainScreenScale"),
        ];
        let scale = attempts.iter().find_map(|(file, key)| {
            let path = res.join(file);
            let out = util::run_ok(
                "/usr/bin/plutil",
                &["-extract", key, "raw", "-o", "-", path.to_str()?],
                None,
                Duration::from_secs(10),
            )
            .ok()?;
            out.trim().parse::<f64>().ok().filter(|s| *s >= 1.0)
        })?;
        self.scale_cache.insert(device_type.to_string(), scale);
        Some(scale)
    }

    fn to_points(&self, udid: &str, x: f64, y: f64) -> (f64, f64) {
        let f = self.mappings.get(udid).map(|m| m.factor).unwrap_or(1.0);
        (x * f, y * f)
    }

    fn sim_list(&self) -> ToolResult {
        let list = devices()?;
        let items: Vec<Value> = list
            .iter()
            .map(
                |d| json!({"name": d.name, "udid": d.udid, "runtime": d.runtime, "state": d.state}),
            )
            .collect();
        let booted = list.iter().filter(|d| d.state == "Booted").count();
        Ok(vec![text(format!(
            "{} available simulators ({booted} booted). AXe touch input: {}.\n{}",
            items.len(),
            if find_axe().is_some() {
                "installed"
            } else {
                "not installed"
            },
            items
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ))])
    }

    fn sim_boot(&self, args: &Value) -> ToolResult {
        let udid = arg_str(args, "udid")?.trim();
        match simctl(&["boot", udid]) {
            Ok(_) => {}
            Err(e) if e.contains("current state: Booted") => {
                return Ok(vec![text(format!("{udid} is already booted."))]);
            }
            Err(e) => return Err(e),
        }
        // Wait until SpringBoard is up so screenshots/taps work right away.
        let _ = util::run(
            "/usr/bin/xcrun",
            &["simctl", "bootstatus", udid],
            None,
            Duration::from_secs(180),
        );
        Ok(vec![text(format!(
            "Booted {udid}. It runs headless; use sim_screenshot to see it (open the Simulator app only if a human wants to watch)."
        ))])
    }

    fn sim_shutdown(&self, args: &Value) -> ToolResult {
        let udid = arg_str(args, "udid")?.trim();
        match simctl(&["shutdown", udid]) {
            Ok(_) => Ok(vec![text(format!("Shut down {udid}."))]),
            Err(e) if e.contains("current state: Shutdown") => {
                Ok(vec![text(format!("{udid} is already shut down."))])
            }
            Err(e) => Err(e),
        }
    }

    fn sim_screenshot(&mut self, args: &Value) -> ToolResult {
        let dev = self.resolve_booted(args)?;
        let tmp = TempFile::new("png");
        simctl(&["io", &dev.udid, "screenshot", "--type=png", tmp.path_str()])?;
        let (_, pw, ph) = util::load_png(&tmp.0)?;
        let scale = self
            .screen_scale(&dev.device_type)
            .unwrap_or(if pw.min(ph) >= 1000 { 3.0 } else { 2.0 });
        let (pt_w, pt_h) = (pw as f64 / scale, ph as f64 / scale);
        let target = pt_w.max(pt_h).round().min(MAX_IMAGE_SIDE as f64) as u32;
        if pw.max(ph) > target {
            util::sips_fit(tmp.path_str(), target)?;
        }
        let (b64, iw, ih) = util::load_png(&tmp.0)?;
        let factor = pt_w / iw.max(1) as f64;
        self.mappings
            .insert(dev.udid.clone(), SimMapping { factor });
        let mapping = if (factor - 1.0).abs() < 0.01 {
            "Screenshot pixels equal device points, so pass coordinates from this image directly to sim_tap/sim_swipe."
                .to_string()
        } else {
            format!(
                "1 screenshot px = {} points; pass coordinates from this image to sim_tap/sim_swipe and trek-mcp converts them.",
                fmt_num(factor)
            )
        };
        Ok(vec![
            rpc::image_png(b64),
            text(format!(
                "{} ({}, {}) screenshot: {iw}x{ih} px. Screen: {}x{} points @{}x ({pw}x{ph} pixels). {mapping}",
                dev.name,
                dev.runtime,
                dev.udid,
                fmt_num(pt_w),
                fmt_num(pt_h),
                fmt_num(scale),
            )),
        ])
    }

    fn sim_open_url(&self, args: &Value) -> ToolResult {
        let url = arg_str(args, "url")?;
        let dev = self.resolve_booted(args)?;
        simctl(&["openurl", &dev.udid, url])?;
        Ok(vec![text(format!("Opened {url} on {}.", dev.name))])
    }

    fn sim_install(&self, args: &Value) -> ToolResult {
        let path = arg_str(args, "app_path")?;
        if !std::path::Path::new(path).exists() {
            return Err(format!(
                "{path} does not exist (expected a built .app bundle, e.g. from DerivedData)"
            ));
        }
        let dev = self.resolve_booted(args)?;
        simctl(&["install", &dev.udid, path])?;
        Ok(vec![text(format!("Installed {path} on {}.", dev.name))])
    }

    fn sim_launch(&self, args: &Value) -> ToolResult {
        let bundle = arg_str(args, "bundle_id")?;
        let dev = self.resolve_booted(args)?;
        let out = simctl(&["launch", "--terminate-running-process", &dev.udid, bundle])?;
        Ok(vec![text(format!(
            "Launched {bundle} on {}. {}",
            dev.name,
            out.trim()
        ))])
    }

    /// `xcodebuild test` on a simulator, its output written line by line to a run log that Trek's
    /// Simulator tool follows (see `trek_ipc::simtest`), its result read back as a short report.
    fn sim_test(&self, args: &Value) -> ToolResult {
        use std::io::{BufRead as _, Write as _};
        use trek_ipc::simtest::{FOOTER, RunHeader, TestRun};

        let scheme = arg_str(args, "scheme")?.trim().to_string();
        let dev = self.resolve(args)?;
        let path = PathBuf::from(opt_str(args, "path").unwrap_or("."));
        let only: Vec<String> = args
            .get("only_testing")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let (cwd, mut cmd_args) = test_target(&path)?;
        cmd_args.extend([
            "test".to_string(),
            "-scheme".into(),
            scheme.clone(),
            "-destination".into(),
            format!("platform=iOS Simulator,id={}", dev.udid),
        ]);
        if let Some(plan) = opt_str(args, "test_plan") {
            cmd_args.extend(["-testPlan".into(), plan.to_string()]);
        }
        cmd_args.extend(only.iter().map(|t| format!("-only-testing:{t}")));

        let dir = std::env::var_os(trek_ipc::simtest::ENV_RUNS_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("trek-sim-runs"));
        let _ = std::fs::create_dir_all(&dir);
        prune_run_logs(&dir);
        let started_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default();
        let log_path = dir.join(format!("{started_ms}-{}.log", std::process::id()));
        let mut log = std::fs::File::create(&log_path)
            .map_err(|e| format!("Couldn't write the run log {}: {e}", log_path.display()))?;
        let header = RunHeader {
            label: scheme.clone(),
            udid: dev.udid.clone(),
            started_ms,
        };
        let _ = log.write_all(header.line().as_bytes());

        let mut child = std::process::Command::new("/usr/bin/xcodebuild") // spawn: ok
            .args(&cmd_args)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Couldn't run xcodebuild: {e}"))?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let readers: Vec<_> = [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|pipe| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
        drop(tx);

        let mut run = TestRun::new(&scheme);
        let mut tail: std::collections::VecDeque<String> = Default::default();
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        let mut timed_out = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    let _ = writeln!(log, "{line}");
                    run.feed(&line);
                    tail.push_back(line);
                    if tail.len() > 40 {
                        tail.pop_front();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if std::time::Instant::now() >= deadline {
                        timed_out = true;
                        let _ = child.kill();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        for r in readers {
            let _ = r.join();
        }
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        let _ = writeln!(log, "{FOOTER}{code}");
        run.finish(code == 0);

        let mut out = run.report();
        if timed_out {
            out.push_str(&format!(
                "\nStopped after {} minutes.",
                TEST_TIMEOUT.as_secs() / 60
            ));
        }
        if run.cases.is_empty() {
            out.push_str("\nLast lines:\n");
            out.push_str(&Vec::from(tail).join("\n"));
        }
        out.push_str(&format!("\nFull log: {}", log_path.display()));
        Ok(vec![text(out)])
    }

    /// The screen of `udid` now, read through AXe.
    fn describe(udid: &str) -> Result<Vec<Node>, String> {
        parse_tree(&axe(&["describe-ui", "--udid", udid], None)?)
    }

    /// Device points per `sim_screenshot` pixel: the latest screenshot's, else what one of this
    /// screen would get.
    fn factor(&self, udid: &str, roots: &[Node]) -> f64 {
        self.mappings
            .get(udid)
            .map(|m| m.factor)
            .or_else(|| screen(roots).map(fitted))
            .unwrap_or(1.0)
    }

    /// What `sim_elements` answers for a screen read as `roots`: whose it is and how a frame
    /// reads, then the outline, with the simulator's refs given out.
    fn outlined(
        &mut self,
        dev: &Device,
        roots: &[Node],
        filter: Option<&str>,
        interactive_only: bool,
        max: usize,
    ) -> String {
        let view = View {
            filter,
            interactive_only,
            max,
            scale: self.factor(&dev.udid, roots),
            origin: (0.0, 0.0),
            shows: screen(roots),
        };
        let out = self
            .refs
            .entry(dev.udid.clone())
            .or_insert_with(|| Refs::new("sim_elements"))
            .outline(roots, &view);
        let mut said = format!(
            "Elements of {} ({}). frame=x,y,width,height in sim_screenshot pixels.\n",
            dev.name, dev.runtime
        );
        if let Some(focused) = &out.focused {
            said.push_str(&format!("Focused: {focused}\n"));
        }
        said.push('\n');
        match (out.shown + out.more, filter) {
            (0, Some(filter)) => said.push_str(&format!(
                "(No element's role or name contains {filter:?}.)"
            )),
            (0, None) => said.push_str(
                "(Nothing here to read or tap: the app may draw everything itself. sim_screenshot shows it.)",
            ),
            _ => said.push_str(out.text.trim_end()),
        }
        said
    }

    fn sim_elements(&mut self, args: &Value) -> ToolResult {
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        let roots = Self::describe(&dev.udid)?;
        let max = opt_f64(args, "max")?.map_or(outline::DEFAULT_MAX, |m| {
            (m.max(1.0) as usize).min(outline::MOST)
        });
        let interactive_only = args
            .get("interactive_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(vec![text(self.outlined(
            &dev,
            &roots,
            opt_str(args, "filter"),
            interactive_only,
            max,
        ))])
    }

    /// Tap at (`px`, `py`) in device points. `Ok(false)`: AXe didn't come back in time, so
    /// whether the tap reached the simulator isn't known.
    fn tap_at(udid: &str, px: f64, py: f64) -> Result<bool, String> {
        match axe(
            &["tap", "-x", &coord(px), "-y", &coord(py), "--udid", udid],
            None,
        ) {
            Ok(_) => Ok(true),
            Err(e) if e.contains("timed out or killed") => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn sim_tap(&mut self, args: &Value) -> ToolResult {
        let by_ref = opt_str(args, "ref");
        let at = ["x", "y"]
            .iter()
            .any(|k| args.get(k).is_some_and(|v| !v.is_null()));
        if by_ref.is_some() && at {
            return Err("Give ref, or x and y, not both.".into());
        }
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        let (what, (px, py)) = match by_ref {
            // By ref: the screen is read again, and the element found there and proven the same.
            Some(r) => {
                let roots = Self::describe(&dev.udid)?;
                let refs = self
                    .refs
                    .entry(dev.udid.clone())
                    .or_insert_with(|| Refs::new("sim_elements"));
                let (what, (px, py)) = tap_point(refs, r, &roots)?;
                let f = self.factor(&dev.udid, &roots);
                (
                    format!(
                        "{what} at its centre ({}, {})",
                        fmt_num((px / f).round()),
                        fmt_num((py / f).round())
                    ),
                    (px, py),
                )
            }
            None => {
                let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
                (
                    format!("({}, {})", fmt_num(x), fmt_num(y)),
                    self.to_points(&dev.udid, x, y),
                )
            }
        };
        if Self::tap_at(&dev.udid, px, py)? {
            Ok(vec![text(format!("Tapped {what} on {}.", dev.name))])
        } else {
            // Not an error and not retried: a second tap could do the thing twice.
            Ok(vec![text(format!(
                "Outcome unknown: the tap on {what} was sent to {}, but AXe didn't answer in time, so it may or may not have landed. It was not tried again: read the elements, or take a screenshot, to see what happened before tapping again.",
                dev.name
            ))])
        }
    }

    /// `sim_wait`: for a time, or until text or an element shows (or goes) on the screen,
    /// answered with the screen's fresh elements either way.
    fn sim_wait(&mut self, args: &Value) -> ToolResult {
        let awaited = match (opt_str(args, "text"), opt_str(args, "element")) {
            (None, None) => {
                let ms = arg_f64(args, "ms").map_err(|_| {
                    "Wait for what? Give text or element (or ms, to just pause).".to_string()
                })?;
                if !(0.0..=10_000.0).contains(&ms) {
                    return Err("ms must be between 0 and 10000".into());
                }
                std::thread::sleep(Duration::from_millis(ms as u64));
                return Ok(vec![text(format!("Waited {} ms.", ms as u64))]);
            }
            (Some(_), Some(_)) => return Err("Give text or element, not both.".into()),
            (text, element) => Awaited {
                needle: text.or(element).unwrap_or_default(),
                by_text: text.is_some(),
                gone: args.get("gone").and_then(Value::as_bool).unwrap_or(false),
            },
        };
        let within = opt_f64(args, "timeout_ms")?.unwrap_or(10_000.0);
        if !(0.0..=60_000.0).contains(&within) {
            return Err("timeout_ms must be between 0 and 60000".into());
        }
        let within = Duration::from_millis(within as u64);
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        let what = awaited.what();
        let started = std::time::Instant::now();
        loop {
            let roots = match Self::describe(&dev.udid) {
                Ok(roots) => roots,
                // A reading AXe couldn't make (it times out now and then) has shown nothing
                // either way: it's asked again until the time is up.
                Err(_) if started.elapsed() < within => {
                    std::thread::sleep(WAIT_POLL);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let (met, waited) = (awaited.met(&roots), started.elapsed());
            if met || waited >= within {
                let outlined = self.outlined(&dev, &roots, None, false, outline::DEFAULT_MAX);
                return if met {
                    Ok(vec![text(format!(
                        "{} after {:.1} s.\n\n{outlined}",
                        if awaited.gone {
                            format!("Gone: {what}")
                        } else {
                            format!("Found {what}")
                        },
                        waited.as_secs_f32()
                    ))])
                } else {
                    Err(format!(
                        "Waited {:.0} s, and {what} {}.\n\n{outlined}",
                        within.as_secs_f32(),
                        if awaited.gone {
                            "is still there"
                        } else {
                            "never showed"
                        }
                    ))
                };
            }
            std::thread::sleep(WAIT_POLL);
        }
    }

    fn sim_swipe(&self, args: &Value) -> ToolResult {
        let (fx, fy) = arg_point(args, "from")?;
        let (tx, ty) = arg_point(args, "to")?;
        let duration = opt_f64(args, "duration")?;
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        let (ax, ay) = self.to_points(&dev.udid, fx, fy);
        let (bx, by) = self.to_points(&dev.udid, tx, ty);
        let (ax, ay, bx, by) = (coord(ax), coord(ay), coord(bx), coord(by));
        let mut a = vec![
            "swipe",
            "--start-x",
            &ax,
            "--start-y",
            &ay,
            "--end-x",
            &bx,
            "--end-y",
            &by,
        ];
        let dur;
        if let Some(d) = duration {
            dur = format!("{:.2}", d.clamp(0.05, 10.0));
            a.extend(["--duration", &dur]);
        }
        a.extend(["--udid", &dev.udid]);
        axe(&a, None)?;
        Ok(vec![text(format!(
            "Swiped from ({}, {}) to ({}, {}) on {}.",
            fmt_num(fx),
            fmt_num(fy),
            fmt_num(tx),
            fmt_num(ty),
            dev.name
        ))])
    }

    fn sim_type(&self, args: &Value) -> ToolResult {
        let s = arg_str(args, "text")?;
        if s.is_empty() {
            return Err("text is empty".into());
        }
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        axe(&["type", "--stdin", "--udid", &dev.udid], Some(s))?;
        Ok(vec![text(format!(
            "Typed {} characters on {}.",
            s.chars().count(),
            dev.name
        ))])
    }

    fn sim_button(&self, args: &Value) -> ToolResult {
        let name = arg_str(args, "name")?.trim().to_lowercase();
        let button = match name.as_str() {
            "home" => "home",
            "lock" | "power" => "lock",
            "siri" => "siri",
            "side" | "side-button" => "side-button",
            other => {
                return Err(format!(
                    "Unknown button {other:?}; use home, lock, siri or side-button"
                ));
            }
        };
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        axe(&["button", button, "--udid", &dev.udid], None)?;
        Ok(vec![text(format!("Pressed {button} on {}.", dev.name))])
    }
}

/// Where `xcodebuild` runs and how it's told the project: a `.xcworkspace` or `.xcodeproj` by
/// flag, a folder (with one of them, or a `Package.swift`) by running in it.
fn test_target(path: &std::path::Path) -> Result<(PathBuf, Vec<String>), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let flag = match ext {
        "xcworkspace" => Some("-workspace"),
        "xcodeproj" => Some("-project"),
        _ => None,
    };
    match flag {
        Some(flag) => {
            let cwd = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            Ok((cwd, vec![flag.to_string(), path.display().to_string()]))
        }
        None if path.is_dir() => Ok((path.to_path_buf(), vec![])),
        None => Err(format!(
            "{} isn't a .xcworkspace, a .xcodeproj or a folder with one (or a Package.swift)",
            path.display()
        )),
    }
}

/// Delete run logs nobody will look at again.
fn prune_run_logs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|d| d > RUN_LOG_KEEP));
        if old && e.path().extension().is_some_and(|x| x == "log") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn udid_prop() -> Value {
    json!({"type": "string", "description": "From sim_list (default: the booted simulator)"})
}

impl rpc::ToolSet for Simulator {
    fn family(&self) -> &'static str {
        "simulator"
    }

    fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "sim_list",
                description: "List available iOS/iPadOS/watchOS/tvOS simulators with name, UDID, runtime and state (booted first).",
                input_schema: json!({"type": "object", "properties": {}}),
            },
            ToolDef {
                name: "sim_boot",
                description: "Boot a simulator by UDID and wait until it has finished starting. Runs headless; use sim_screenshot to see it.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"udid": {"type": "string", "description": "Simulator UDID from sim_list"}},
                    "required": ["udid"],
                }),
            },
            ToolDef {
                name: "sim_shutdown",
                description: "Shut down a simulator by UDID.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"udid": {"type": "string", "description": "Simulator UDID from sim_list"}},
                    "required": ["udid"],
                }),
            },
            ToolDef {
                name: "sim_screenshot",
                description: "Screenshot a booted simulator. The image is sized in device points (longest side ≤ 1568), \
so its pixel coordinates are what sim_tap and sim_swipe expect. The text block reports the point size and scale.",
                input_schema: json!({"type": "object", "properties": {"udid": udid_prop()}}),
            },
            ToolDef {
                name: "sim_open_url",
                description: "Open a URL (web link or custom URL scheme / deep link) in a booted simulator.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"url": {"type": "string"}, "udid": udid_prop()},
                    "required": ["url"],
                }),
            },
            ToolDef {
                name: "sim_install",
                description: "Install a built .app bundle (e.g. from Xcode DerivedData/Build/Products/Debug-iphonesimulator) on a booted simulator.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "app_path": {"type": "string", "description": "Absolute path to the .app bundle"},
                        "udid": udid_prop(),
                    },
                    "required": ["app_path"],
                }),
            },
            ToolDef {
                name: "sim_launch",
                description: "Launch (or relaunch) an installed app by bundle identifier on a booted simulator.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "bundle_id": {"type": "string", "description": "e.g. com.example.MyApp or com.apple.mobilesafari"},
                        "udid": udid_prop(),
                    },
                    "required": ["bundle_id"],
                }),
            },
            ToolDef {
                name: "sim_elements",
                description: "A booted simulator's screen as an outline, one element a line: role, name, value, state and frame (x,y,width,height in sim_screenshot pixels), with a ref like [ref=e12] on each one you can tap. Read it before a screenshot: names and places are exact. Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "filter": {"type": "string", "description": "Only elements whose role or name contains this"},
                        "interactive_only": {"type": "boolean", "description": "Only those you can tap"},
                        "max": {"type": "integer", "description": "Most elements shown (default 150)"},
                        "udid": udid_prop(),
                    },
                }),
            },
            ToolDef {
                name: "sim_tap",
                description: "Tap the element `ref` names (refused if it changed since you read it), or at (x, y) in sim_screenshot pixel coordinates. Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "ref": {"type": "string", "description": "From sim_elements, e.g. \"e12\""},
                        "x": {"type": "number"},
                        "y": {"type": "number"},
                        "udid": udid_prop(),
                    },
                }),
            },
            ToolDef {
                name: "sim_wait",
                description: "Wait until `text` or an `element` shows on the screen (`gone`: until it goes), answered with the screen's fresh elements; or pause for `ms` milliseconds (max 10000). Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "In an element's name or value"},
                        "element": {"type": "string", "description": "In an element's role or name"},
                        "gone": {"type": "boolean"},
                        "timeout_ms": {"type": "integer", "description": "Default 10000, at most 60000"},
                        "ms": {"type": "integer", "minimum": 0, "maximum": 10000},
                        "udid": udid_prop(),
                    },
                }),
            },
            ToolDef {
                name: "sim_swipe",
                description: "Swipe from `from` to `to` in sim_screenshot pixel coordinates (e.g. scroll a list by swiping up). Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "from": point_schema("Start point"),
                        "to": point_schema("End point"),
                        "duration": {"type": "number", "description": "Seconds (optional, default ~0.5)"},
                        "udid": udid_prop(),
                    },
                    "required": ["from", "to"],
                }),
            },
            ToolDef {
                name: "sim_type",
                description: "Type text into the focused field of a booted simulator (tap the field first). Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}, "udid": udid_prop()},
                    "required": ["text"],
                }),
            },
            ToolDef {
                name: "sim_button",
                description: "Press a hardware button on a booted simulator: home, lock, siri or side-button. Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "enum": ["home", "lock", "siri", "side-button"]},
                        "udid": udid_prop(),
                    },
                    "required": ["name"],
                }),
            },
            ToolDef {
                name: "sim_test",
                description: "Run a scheme's tests (XCTest, Swift Testing, UI tests) on a simulator with `xcodebuild test`. \
The user watches them live in Trek's Simulator tool: the device's screen and each test passing or failing. Returns how \
the run ended, the counts and every failure with its message, and the path of the full log.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "scheme": {"type": "string", "description": "The scheme to test"},
                        "path": {"type": "string", "description": "A .xcworkspace, a .xcodeproj, or a folder with one or with a Package.swift (default: the working directory)"},
                        "only_testing": {"type": "array", "items": {"type": "string"}, "description": "Limit the run: Target, Target/Class or Target/Class/testMethod"},
                        "test_plan": {"type": "string", "description": "A test plan of the scheme"},
                        "udid": udid_prop(),
                    },
                    "required": ["scheme"],
                }),
            },
        ]
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        match name {
            "sim_list" => self.sim_list(),
            "sim_boot" => self.sim_boot(args),
            "sim_shutdown" => self.sim_shutdown(args),
            "sim_screenshot" => self.sim_screenshot(args),
            "sim_open_url" => self.sim_open_url(args),
            "sim_install" => self.sim_install(args),
            "sim_launch" => self.sim_launch(args),
            "sim_elements" => self.sim_elements(args),
            "sim_tap" => self.sim_tap(args),
            "sim_wait" => self.sim_wait(args),
            "sim_swipe" => self.sim_swipe(args),
            "sim_type" => self.sim_type(args),
            "sim_button" => self.sim_button(args),
            "sim_test" => self.sim_test(args),
            other => Err(format!("Unknown tool {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_names() {
        assert_eq!(
            pretty_runtime("com.apple.CoreSimulator.SimRuntime.iOS-26-0"),
            "iOS 26.0"
        );
        assert_eq!(
            pretty_runtime("com.apple.CoreSimulator.SimRuntime.watchOS-11-2"),
            "watchOS 11.2"
        );
    }

    #[test]
    fn device_parsing_and_order() {
        let j = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"name":"iPhone 16","udid":"A","state":"Shutdown","isAvailable":true,"deviceTypeIdentifier":"t1"}],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[
                {"name":"iPhone 17","udid":"B","state":"Shutdown","isAvailable":true},
                {"name":"iPad","udid":"C","state":"Booted","isAvailable":true},
                {"name":"Broken","udid":"D","state":"Shutdown","isAvailable":false}]}}"#;
        let d = parse_devices(j).unwrap();
        let ids: Vec<_> = d.iter().map(|d| d.udid.as_str()).collect();
        assert_eq!(ids, ["C", "B", "A"]);
        assert_eq!(d[2].runtime, "iOS 18.0");
        assert_eq!(d[2].device_type, "t1");
    }

    /// The Settings app's first screen as AXe 1.8 describes it (iOS 27, an iPhone 17e), cut down
    /// to a few rows and with a switch, a disabled row, a row scrolled off the screen, a custom
    /// view and a passcode field put in.
    const SETTINGS: &str = r#"[{"AXFrame":"{{0, 0}, {390, 844}}","AXUniqueId":null,"frame":{"y":0,"x":0,"width":390,"height":844},
      "role_description":"application","AXLabel":"Settings","content_required":false,"type":"Application","title":null,"help":null,
      "custom_actions":[],"AXValue":null,"enabled":true,"role":"AXApplication","subrole":null,"pid":64278,"traits":[],"children":[
      {"type":"Group","role":"AXGroup","AXLabel":null,"AXUniqueId":"com.apple.settings.sidebar.collectionView","subrole":null,
       "frame":{"x":0,"y":0,"width":390,"height":844},"enabled":true,"AXValue":null,"custom_actions":[],"traits":[],"children":[
        {"type":"Heading","role":"AXHeading","AXLabel":"Settings","AXUniqueId":null,"frame":{"x":16,"y":105,"width":133,"height":41},"enabled":true,"AXValue":null,"custom_actions":[],"children":[]},
        {"type":"Button","role":"AXButton","AXLabel":"General","AXUniqueId":"com.apple.settings.general","role_description":"button",
         "frame":{"height":52,"width":358,"x":16,"y":365.33333333333337},"AXFrame":"{{16, 365.33333333333337}, {358, 52}}","enabled":true,"AXValue":null,"custom_actions":[],"children":[
          {"type":"Image","role":"AXImage","AXLabel":null,"AXUniqueId":null,"frame":{"x":30,"y":377.3333435058594,"width":28,"height":28},"enabled":true,"AXValue":null,"children":[]},
          {"type":"StaticText","role":"AXStaticText","AXLabel":"General","AXUniqueId":null,"frame":{"height":28,"width":101.33332824707031,"x":70,"y":377.3333435058594},"enabled":true,"AXValue":null,"children":[]},
          {"type":"Image","role":"AXImage","AXLabel":"chevron.forward","AXUniqueId":null,"frame":{"x":349,"y":385,"width":7,"height":12},"enabled":true,"AXValue":null,"children":[]}]},
        {"type":"Button","role":"AXButton","AXLabel":"Accessibility","AXUniqueId":"com.apple.settings.accessibility","frame":{"height":52,"width":358,"x":16,"y":417.33333333333337},"enabled":true,"AXValue":null,"custom_actions":[],"children":[]},
        {"type":"Switch","role":"AXSwitch","AXLabel":"Airplane Mode","AXUniqueId":null,"frame":{"x":16,"y":469,"width":358,"height":52},"enabled":true,"AXValue":"0","custom_actions":[],"children":[]},
        {"type":"Button","role":"AXButton","AXLabel":"Software Update","AXUniqueId":null,"frame":{"x":16,"y":521,"width":358,"height":52},"enabled":false,"AXValue":null,"custom_actions":[],"children":[]},
        {"type":"Button","role":"AXButton","AXLabel":"StandBy","AXUniqueId":null,"frame":{"x":16,"y":885,"width":358,"height":52},"enabled":true,"AXValue":null,"custom_actions":[],"children":[]},
        {"type":"Other","role":"AXGroup","AXLabel":null,"AXUniqueId":"com.example.banner","frame":{"x":16,"y":580,"width":358,"height":40},"enabled":true,"AXValue":null,"custom_actions":[],"children":[]}]},
      {"type":"Group","role":"AXGroup","AXLabel":"Toolbar","AXUniqueId":null,"frame":{"x":0,"y":730,"width":390,"height":114},"enabled":true,"AXValue":null,"custom_actions":[],"children":[
        {"AXValue":"Search","AXFrame":"{{28, 778}, {334, 28}}","content_required":false,"pid":64278,"AXUniqueId":null,"subrole":"AXSearchField","AXLabel":"Search","enabled":true,
         "frame":{"x":28,"width":334,"y":778,"height":28},"title":null,"traits":[],"role":"AXTextField","type":"TextField","custom_actions":[],"help":null,"role_description":"search text field","children":[]},
        {"type":"SecureTextField","role":"AXTextField","AXLabel":"Passcode","AXUniqueId":null,"frame":{"x":28,"y":740,"width":334,"height":28},"enabled":true,"AXValue":"••••","custom_actions":[],"children":[]}]}]}]"#;

    fn iphone() -> Device {
        Device {
            name: "iPhone 17e".into(),
            udid: "U".into(),
            runtime: "iOS 27.0".into(),
            state: "Booted".into(),
            device_type: String::new(),
        }
    }

    /// A simulator whose Settings screen has been read once.
    fn read_settings() -> (Simulator, Vec<Node>, String) {
        let mut sim = Simulator::default();
        let roots = parse_tree(SETTINGS).unwrap();
        let read = sim.outlined(&iphone(), &roots, None, false, outline::DEFAULT_MAX);
        (sim, roots, read)
    }

    #[test]
    fn the_screen_reads_as_an_outline_from_axes_tree() {
        let (mut sim, roots, read) = read_settings();
        assert_eq!(
            read,
            "Elements of iPhone 17e (iOS 27.0). frame=x,y,width,height in sim_screenshot pixels.

- application \"Settings\" [frame=0,0,390,844]:
  - heading \"Settings\" [frame=16,105,133,41]
  - button \"General\" [ref=e1] [frame=16,365,358,52]:
    - image \"chevron.forward\" [frame=349,385,7,12]
  - button \"Accessibility\" [ref=e2] [frame=16,417,358,52]
  - switch \"Airplane Mode\" [ref=e3] [unchecked] [frame=16,469,358,52]
  - button \"Software Update\" [ref=e4] [disabled] [frame=16,521,358,52]
  - button \"StandBy\" [ref=e5] [offscreen] [frame=16,885,358,52]
  - other \"com.example.banner\" [ref=e6] [frame=16,580,358,40]
  - group \"Toolbar\" [frame=0,730,390,114]:
    - search field \"Search\" [ref=e7] [frame=28,778,334,28]
    - text field \"Passcode\" [ref=e8] [secure] [frame=28,740,334,28]"
        );
        assert!(
            !read.contains('•'),
            "a passcode field's value is never read"
        );
        // Read again, every element has the ref it had; narrowed, the same refs.
        assert_eq!(
            sim.outlined(&iphone(), &roots, None, false, outline::DEFAULT_MAX),
            read
        );
        let narrowed = sim.outlined(&iphone(), &roots, Some("access"), true, 5);
        assert!(
            narrowed.ends_with("\n\n- button \"Accessibility\" [ref=e2] [frame=16,417,358,52]"),
            "{narrowed}"
        );
        assert!(
            sim.outlined(&iphone(), &roots, Some("wallet"), false, 5)
                .ends_with("(No element's role or name contains \"wallet\".)")
        );
        // Frames are in the latest screenshot's pixels: here one that's two points to a pixel.
        sim.mappings.insert("U".into(), SimMapping { factor: 2.0 });
        let halved = sim.outlined(&iphone(), &roots, Some("general"), false, 5);
        assert!(
            halved.contains("button \"General\" [ref=e1] [frame=8,183,179,26]"),
            "{halved}"
        );
        // With no screenshot yet, the pixels are those one would have: a point each, unless
        // the screen's long side passes the largest image.
        assert_eq!(
            fitted(Frame {
                x: 0.0,
                y: 0.0,
                w: 390.0,
                h: 844.0
            }),
            1.0
        );
        assert_eq!(
            fitted(Frame {
                x: 0.0,
                y: 0.0,
                w: 1024.0,
                h: 1366.0
            }),
            1.0
        );
        assert_eq!(
            fitted(Frame {
                x: 0.0,
                y: 0.0,
                w: 2000.0,
                h: 3136.0
            }),
            2.0
        );
        // Something that isn't AXe's JSON says so; an empty screen reads as empty.
        assert!(
            parse_tree("Error: no simulator")
                .unwrap_err()
                .contains("isn't JSON")
        );
        let mut empty = Simulator::default();
        assert!(
            empty
                .outlined(&iphone(), &parse_tree("[]").unwrap(), None, false, 5)
                .ends_with("(Nothing here to read or tap: the app may draw everything itself. sim_screenshot shows it.)")
        );
    }

    #[test]
    fn a_tap_by_ref_goes_to_the_element_found_again_and_proven_the_same() {
        let (sim, roots, _) = read_settings();
        let refs = &sim.refs["U"];
        // As it was read: its centre, in device points.
        let (what, (x, y)) = tap_point(refs, "e1", &roots).unwrap();
        assert_eq!(what, "button \"General\"");
        assert!(
            (x - 195.0).abs() < 0.01 && (y - 391.33).abs() < 0.01,
            "{x}, {y}"
        );
        let again =
            "Nothing was done. Read the elements again (sim_elements) and use the ref it has now.";
        fn rows(roots: &mut [Node]) -> &mut Vec<Node> {
            &mut roots[0].children[0].children
        }
        // Another screen's row at its place.
        let mut renamed = roots.clone();
        rows(&mut renamed)[1].name = "About".into();
        assert_eq!(
            tap_point(refs, "e1", &renamed).unwrap_err(),
            format!("Where button \"General\" was there's now button \"About\". {again}")
        );
        // The list moved under it: the same name at the same place, among other rows.
        let mut scrolled = roots.clone();
        rows(&mut scrolled)[2].name = "Action Button".into();
        assert!(
            tap_point(refs, "e1", &scrolled)
                .unwrap_err()
                .starts_with("What's around button \"General\" has changed since you read it")
        );
        let mut moved = roots.clone();
        rows(&mut moved)[1].frame.as_mut().unwrap().y += 60.0;
        assert!(
            tap_point(refs, "e1", &moved)
                .unwrap_err()
                .contains("has moved since you read it")
        );
        let mut gone = roots.clone();
        rows(&mut gone).truncate(1);
        assert!(
            tap_point(refs, "e1", &gone)
                .unwrap_err()
                .contains("isn't where it was any more")
        );
        // A row that can't be tapped, or isn't on the screen.
        assert_eq!(
            tap_point(refs, "e4", &roots).unwrap_err(),
            "button \"Software Update\" is disabled, so nothing was tapped."
        );
        assert!(
            tap_point(refs, "e5", &roots)
                .unwrap_err()
                .starts_with("button \"StandBy\" is off the screen (scrolled out of view)")
        );
        let err = tap_point(refs, "e99", &roots).unwrap_err();
        assert!(
            err.contains("No element with ref \"e99\"") && err.contains("sim_elements"),
            "{err}"
        );
        // After the screen changed and was read again, a ref names what's there now.
        let mut sim = sim;
        sim.outlined(&iphone(), &renamed, None, false, outline::DEFAULT_MAX);
        assert!(
            tap_point(&sim.refs["U"], "e1", &renamed).is_err(),
            "the old row's ref stays the old row's"
        );
        let about = sim.outlined(&iphone(), &renamed, Some("about"), false, 5);
        let r = about
            .split("[ref=")
            .nth(1)
            .unwrap()
            .split(']')
            .next()
            .unwrap();
        assert_eq!(
            tap_point(&sim.refs["U"], r, &renamed).unwrap().0,
            "button \"About\""
        );
    }

    #[test]
    fn a_wait_is_for_text_or_an_element_to_show_or_go() {
        use crate::rpc::ToolSet as _;
        let roots = parse_tree(SETTINGS).unwrap();
        let wait = |needle, by_text, gone| Awaited {
            needle,
            by_text,
            gone,
        };
        assert!(wait("airplane mode", true, false).met(&roots));
        assert!(!wait("Wi-Fi", true, false).met(&roots) && wait("Wi-Fi", true, true).met(&roots));
        assert!(
            wait("switch", false, false).met(&roots) && !wait("switch", false, true).met(&roots)
        );
        assert!(
            !wait("••", true, false).met(&roots),
            "nor is a passcode waited on"
        );
        assert_eq!(wait("General", true, false).what(), "the text \"General\"");
        assert_eq!(
            wait("button", false, true).what(),
            "an element matching \"button\""
        );
        // What's asked wrongly is refused before the simulator is touched.
        let mut sim = Simulator::default();
        let mut refused = |tool: &str, args: Value| sim.call(tool, &args).unwrap_err();
        assert_eq!(
            refused("sim_wait", json!({"text": "a", "element": "b"})),
            "Give text or element, not both."
        );
        assert!(refused("sim_wait", json!({})).starts_with("Wait for what?"));
        assert!(refused("sim_wait", json!({"ms": 20_000})).contains("between 0 and 10000"));
        assert!(
            refused("sim_wait", json!({"text": "a", "timeout_ms": 90_000}))
                .contains("between 0 and 60000")
        );
        assert_eq!(
            refused("sim_tap", json!({"ref": "e1", "x": 1, "y": 2})),
            "Give ref, or x and y, not both."
        );
        assert_eq!(
            sim.call("sim_wait", &json!({"ms": 0})),
            Ok(vec![text("Waited 0 ms.")])
        );
    }

    #[test]
    fn the_tools_offer_elements_before_screenshots() {
        use crate::rpc::ToolSet as _;
        let sim = Simulator::default();
        let tools = sim.tools();
        let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap();
        for name in ["sim_elements", "sim_tap", "sim_wait"] {
            let schema = &tool(name).input_schema;
            assert_eq!(schema["type"], "object", "{name}");
            assert!(
                schema.get("required").is_none(),
                "{name} needs a ref or a point, text or a time"
            );
            assert!(schema["properties"]["udid"].is_object(), "{name}");
        }
        assert!(tool("sim_tap").input_schema["properties"]["ref"].is_object());
        assert!(tool("sim_elements").description.contains("[ref=e12]"));
        let said = sim.instructions();
        let (elements, screenshot) = (
            said.find("sim_elements").unwrap(),
            said.find("sim_screenshot").unwrap(),
        );
        assert!(elements < screenshot, "{said}");
    }
}
