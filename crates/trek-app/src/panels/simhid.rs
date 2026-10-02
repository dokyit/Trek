//! A persistent link to one booted simulator through `trek-simhid`, a small Swift helper built on
//! the FBSimulatorControl frameworks that ship with AXe. One HID session stays open, so taps and
//! drags land immediately (each `axe` call pays 2–4 s of start-up), and the helper streams the
//! framebuffer as JPEG frames whenever the screen changes, up to 30 fps.

use gpui_kit::*;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

const SOURCE: &str = include_str!("../../simhid/main.swift");
const STUB_MAP: &str = include_str!("../../simhid/CoreSimulatorStub/module.modulemap");
const STUB_HEADER: &str = include_str!("../../simhid/CoreSimulatorStub/CoreSimulator.h");
/// Frames are encoded at most this wide (device pixels); plenty for a side-panel mirror.
pub const MAX_WIDTH: u32 = 900;

pub struct LinkFrame {
    pub image: Arc<RenderImage>,
    /// The device screen's real size in pixels (the image may be smaller).
    pub width: f32,
    pub height: f32,
}

pub enum LinkEvent {
    Ready,
    Frame(LinkFrame),
    Error(String),
    Exited,
}

pub struct SimLink {
    pub udid: String,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
}

impl SimLink {
    /// Start the helper for `udid`. Events arrive on the returned channel; frames are decoded on a
    /// background thread and only the newest one is kept while the UI is busy.
    pub fn start(helper: &Path, udid: &str, renderer: SvgRenderer) -> Result<(Arc<SimLink>, async_channel::Receiver<LinkEvent>), String> {
        let mut child = Command::new(helper)
            .args([udid, &MAX_WIDTH.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start the simulator link: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = async_channel::unbounded::<LinkEvent>();
        let latest: Arc<Mutex<Option<(f32, f32, Vec<u8>)>>> = Arc::default();
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);

        // Decoder: always works on the newest raw frame.
        let (frames_tx, slot) = (tx.clone(), latest.clone());
        let _ = std::thread::Builder::new().name("trek-simhid-decode".into()).spawn(move || {
            while wake_rx.recv_blocking().is_ok() {
                let Some((width, height, jpeg)) = slot.lock().unwrap().take() else { continue };
                let image = Image::from_bytes(ImageFormat::Jpeg, jpeg);
                if let Ok(decoded) = image.to_image_data(renderer.clone()) {
                    let frame = LinkFrame { image: decoded, width, height };
                    if frames_tx.send_blocking(LinkEvent::Frame(frame)).is_err() {
                        break;
                    }
                }
            }
        });

        // Reader: splits the framed stdout into frames and messages.
        let _ = std::thread::Builder::new().name("trek-simhid-read".into()).spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut header = [0u8; 5];
            while r.read_exact(&mut header).is_ok() {
                let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
                let mut payload = vec![0u8; len];
                if r.read_exact(&mut payload).is_err() {
                    break;
                }
                match header[0] {
                    1 if payload.len() > 4 => {
                        let w = u16::from_be_bytes([payload[0], payload[1]]) as f32;
                        let h = u16::from_be_bytes([payload[2], payload[3]]) as f32;
                        *latest.lock().unwrap() = Some((w, h, payload.split_off(4)));
                        let _ = wake_tx.try_send(());
                    }
                    2 => {
                        let text = String::from_utf8_lossy(&payload).to_string();
                        let event = match text.strip_prefix("error ") {
                            Some(e) => LinkEvent::Error(e.to_string()),
                            None if text == "ready" => LinkEvent::Ready,
                            None => continue,
                        };
                        if tx.send_blocking(event).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let _ = tx.send_blocking(LinkEvent::Exited);
        });

        Ok((Arc::new(SimLink { udid: udid.to_string(), stdin: Mutex::new(stdin), child: Mutex::new(child) }), rx))
    }

    /// Send one command line (see simhid/main.swift). Never blocks on the helper for long: the
    /// pipe has room for thousands of commands.
    pub fn send(&self, line: &str) {
        if let Ok(mut w) = self.stdin.lock() {
            let _ = w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")).and_then(|_| w.flush());
        }
    }

    pub fn touch(&self, phase: &str, x: f64, y: f64) {
        self.send(&format!("{phase} {:.1} {:.1}", x.max(0.0), y.max(0.0)));
    }
}

impl Drop for SimLink {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// AXe's framework folder (Homebrew keeps them next to its binary).
fn axe_frameworks() -> Option<PathBuf> {
    let opt = PathBuf::from("/opt/homebrew/opt/axe/libexec/Frameworks");
    if opt.join("FBSimulatorControl.framework").exists() {
        return Some(opt);
    }
    let axe = std::fs::canonicalize(crate::integrations::axe_path()?).ok()?;
    let dir = axe.parent()?.parent()?.join("libexec/Frameworks");
    dir.join("FBSimulatorControl.framework").exists().then_some(dir)
}

/// The helper binary: bundled with the app, else built once into Trek's data folder (needs Xcode's
/// Swift compiler and AXe; takes ~20 s the first time). Blocking; call off the UI thread.
pub fn ensure_helper() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let bundled = dir.join("trek-simhid");
        if bundled.exists() {
            return Ok(bundled);
        }
    }
    let frameworks = axe_frameworks().ok_or("AXe isn't installed")?;
    let stamp = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        SOURCE.hash(&mut h);
        frameworks.hash(&mut h);
        let bin = frameworks.join("FBSimulatorControl.framework/Versions/A/FBSimulatorControl");
        std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0).hash(&mut h);
        h.finish()
    };
    let out_dir = trek_core::paths::data_dir().join("bin");
    let out = out_dir.join(format!("trek-simhid-{stamp:016x}"));
    if out.exists() {
        return Ok(out);
    }
    let _ = std::fs::create_dir_all(&out_dir);
    build_helper(&frameworks, &out)?;
    // Older builds of the helper are dead weight now.
    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("trek-simhid-") && e.path() != out {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(out)
}

/// Compile main.swift against a scratch copy of AXe's frameworks. Their Swift interfaces qualify
/// types with module names that are also class names (FBSimulatorControl.FBSimulatorControl,
/// IOSurface.IOSurface), which newer compilers can't resolve, so the copy has those prefixes
/// stripped; CoreSimulator, a private framework without headers, gets a four-class stub.
pub fn build_helper(frameworks: &Path, out: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("trek-simhid-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let fw_copy = tmp.join("Frameworks");
    let stub = tmp.join("CoreSimulatorStub");
    std::fs::create_dir_all(&stub).map_err(|e| e.to_string())?;
    std::fs::write(stub.join("module.modulemap"), STUB_MAP).map_err(|e| e.to_string())?;
    std::fs::write(stub.join("CoreSimulator.h"), STUB_HEADER).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join("main.swift"), SOURCE).map_err(|e| e.to_string())?;
    let copied = Command::new("/bin/cp").arg("-R").arg(frameworks).arg(&fw_copy).status().map_err(|e| e.to_string())?;
    if !copied.success() {
        return Err("Couldn't copy AXe's frameworks".into());
    }
    for module in ["FBSimulatorControl", "FBControlCore", "XCTestBootstrap"] {
        let dir = fw_copy.join(format!("{module}.framework/Versions/A/Modules/{module}.swiftmodule"));
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.ends_with(".swiftmodule") || name.ends_with(".private.swiftinterface") {
                let _ = std::fs::remove_file(&path);
            } else if name.ends_with(".swiftinterface") {
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                std::fs::write(&path, unqualify(&text, module)).map_err(|e| e.to_string())?;
            }
        }
    }
    let result = Command::new("/usr/bin/xcrun")
        .args(["swiftc", "-O", "-suppress-warnings", "-I"])
        .arg(&stub)
        .arg("-F")
        .arg(&fw_copy)
        .args(["-framework", "FBSimulatorControl", "-framework", "FBControlCore", "-Xlinker", "-rpath", "-Xlinker"])
        .arg(frameworks)
        .arg(tmp.join("main.swift"))
        .arg("-o")
        .arg(out)
        .output()
        .map_err(|e| format!("Couldn't run the Swift compiler: {e}"))?;
    let _ = std::fs::remove_dir_all(&tmp);
    if result.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&result.stderr);
        let line = err.lines().find(|l| l.contains("error:")).unwrap_or("swiftc failed").trim().to_string();
        Err(format!("Couldn't build the simulator link: {line}"))
    }
}

/// Drop `Module.` / `IOSurface.` qualifiers outside import lines.
fn unqualify(text: &str, module: &str) -> String {
    let strip = |line: &str, prefix: &str| -> String {
        let needle = format!("{prefix}.");
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(i) = rest.find(&needle) {
            let before_ok = rest[..i].chars().last().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let after = &rest[i + needle.len()..];
            let after_ok = after.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
            out.push_str(&rest[..i]);
            if !(before_ok && after_ok) {
                out.push_str(&needle);
            }
            rest = after;
        }
        out.push_str(rest);
        out
    };
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("import ") {
                line.to_string()
            } else {
                strip(line, module).replace("IOSurface.IOSurface", "IOSurface")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::unqualify;

    #[test]
    fn strips_module_qualifiers() {
        let src = "import FBSimulatorControl\nfunc a(_ s: FBSimulatorControl.FBSimulator) -> IOSurface.IOSurface? // MyFBSimulatorControl.x";
        let out = unqualify(src, "FBSimulatorControl");
        assert_eq!(out, "import FBSimulatorControl\nfunc a(_ s: FBSimulator) -> IOSurface? // MyFBSimulatorControl.x");
    }
}
