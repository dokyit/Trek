//! `trek-mcp simulator` — iOS Simulator control via `xcrun simctl`, with
//! touch, typing and hardware buttons through the AXe CLI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::rpc::{
    self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, opt_str, point_schema, text,
};
use crate::util::{self, MAX_IMAGE_SIDE, TempFile, fmt_num};

pub const AXE_MISSING: &str = "Touch input needs AXe. Turn on 'Simulator touch input' in Trek ▸ Settings ▸ Tools to install it.";

const INSTRUCTIONS: &str = "iOS Simulator control. Use `sim_list` to find devices and `sim_boot` to start one. \
`sim_screenshot` returns an image sized in device points (downscaled further only if very large) and reports the \
mapping; x/y for sim_tap and sim_swipe are pixel coordinates in that screenshot. Every tool takes an optional `udid` \
(default: the booted simulator). Take a fresh screenshot after acting to verify the result. Run XCTest or Swift \
Testing suites with `sim_test` rather than `xcodebuild test` in a shell: the user watches each test pass or fail live \
in Trek's Simulator tool, and you get a short report of the failures.";

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

    fn sim_tap(&self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        find_axe().ok_or(AXE_MISSING)?;
        let dev = self.resolve_booted(args)?;
        let (px, py) = self.to_points(&dev.udid, x, y);
        axe(
            &[
                "tap",
                "-x",
                &coord(px),
                "-y",
                &coord(py),
                "--udid",
                &dev.udid,
            ],
            None,
        )?;
        Ok(vec![text(format!(
            "Tapped ({}, {}) on {}.",
            fmt_num(x),
            fmt_num(y),
            dev.name
        ))])
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
                name: "sim_tap",
                description: "Tap at (x, y) in sim_screenshot pixel coordinates (device points). Requires AXe.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "x": {"type": "number"},
                        "y": {"type": "number"},
                        "udid": udid_prop(),
                    },
                    "required": ["x", "y"],
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
            "sim_tap" => self.sim_tap(args),
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
}
