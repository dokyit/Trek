//! The helper process (helper/main.swift): the iPhone's screen over USB and the Mac's Bluetooth
//! keyboard and mouse for it. One runs at a time, owned by Trek's iPhone tool; this starts it,
//! sends it commands and splits what it prints into events.
//!
//! It's bundled in Trek.app; a build from source compiles it once into Trek's data folder with the
//! Swift compiler of the Command Line Tools (`xcode-select --install`; full Xcode isn't needed).
//! Trek, its parent, is the process macOS asks about the camera (the phone's screen counts as
//! one) and Bluetooth, so Trek's Info.plist says why it uses them.

use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::hid::{self, Step};

pub const SOURCE: &str = include_str!("../helper/main.swift");
/// The helper's own Info.plist, linked in: the usage strings macOS shows when it runs by itself
/// (under Trek, Trek's own are the ones shown).
const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.trek.phone-helper</string>
<key>CFBundleName</key><string>Trek iPhone</string>
<key>NSCameraUsageDescription</key><string>Trek shows your iPhone's screen, which comes over the USB cable as a camera does.</string>
<key>NSBluetoothAlwaysUsageDescription</key><string>Trek is a Bluetooth keyboard and mouse for your iPhone, so agents can tap and type on it.</string>
</dict></plist>
"#;
/// The bundled helper's name, next to Trek's executable.
pub const BUNDLED: &str = "trek-phone-helper";

/// What the helper reports about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// "granted", "denied" or "undetermined".
    pub camera: String,
    /// The capture device's name ("tobyj's iPhone"); empty when there's none.
    pub screen: String,
    /// The newest frame's size (the screen's pixels); 0 before the first.
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    /// Seconds since the newest frame (negative: none yet).
    pub frame_age: f64,
    /// "starting", "advertising", "connected", "off", "not allowed", …
    pub bluetooth: String,
    /// Phones subscribed to the keyboard and mouse.
    pub hosts: u32,
}

impl Status {
    /// Whether a phone listens to the keyboard and mouse.
    pub fn input_ready(&self) -> bool {
        self.hosts > 0
    }
}

/// A screenshot the helper wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    /// The picture's size, and the screen's.
    pub width: u32,
    pub height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    /// How old the frame was, in seconds.
    pub age: f64,
}

pub enum Event {
    /// A JPEG of the screen (at most the width the helper was started with) and the screen's size.
    Frame {
        jpeg: Vec<u8>,
        width: u32,
        height: u32,
    },
    Status(Status),
    Error(String),
    Exited,
}

/// One message line from the helper, as an event (shots and input go to their waiters).
enum Message {
    Event(Event),
    Shot(Result<Shot, String>),
    Hid { sent: u64, missed: u64 },
    Ignored,
}

fn parse_message(payload: &[u8]) -> Message {
    let Ok(v) = serde_json::from_slice::<Value>(payload) else {
        return Message::Ignored;
    };
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    match v["event"].as_str() {
        Some("status") => Message::Event(Event::Status(Status {
            camera: s("camera"),
            screen: s("screen"),
            width: n("width") as u32,
            height: n("height") as u32,
            frames: n("frames"),
            frame_age: v["frame_age"].as_f64().unwrap_or(-1.),
            bluetooth: s("bluetooth"),
            hosts: n("hosts") as u32,
        })),
        Some("shot") => Message::Shot(match v["error"].as_str() {
            Some(e) => Err(e.to_string()),
            None => Ok(Shot {
                path: PathBuf::from(s("path")),
                width: n("width") as u32,
                height: n("height") as u32,
                screen_width: n("screen_width") as u32,
                screen_height: n("screen_height") as u32,
                age: v["age"].as_f64().unwrap_or(0.),
            }),
        }),
        Some("hid") => Message::Hid {
            sent: n("sent"),
            missed: n("missed"),
        },
        Some("error") => Message::Event(Event::Error(s("message"))),
        _ => Message::Ignored,
    }
}

/// A frame payload: the screen's width and height, then the JPEG.
fn parse_frame(mut payload: Vec<u8>) -> Option<Event> {
    if payload.len() <= 4 {
        return None;
    }
    let width = u16::from_be_bytes([payload[0], payload[1]]) as u32;
    let height = u16::from_be_bytes([payload[2], payload[3]]) as u32;
    Some(Event::Frame {
        jpeg: payload.split_off(4),
        width,
        height,
    })
}

type Waiters<T> = Mutex<VecDeque<async_channel::Sender<T>>>;

/// The running helper.
pub struct Link {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    shots: Arc<Waiters<Result<Shot, String>>>,
    inputs: Arc<Waiters<(u64, u64)>>,
}

impl Link {
    /// Start `helper`, frames at most `max_width` wide. Events come on the returned channel; only
    /// the newest frame waits while the reader is busy.
    pub fn start(
        helper: &Path,
        max_width: u32,
    ) -> Result<(Arc<Link>, async_channel::Receiver<Event>), String> {
        let map: String = hid::report_map()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut child = Command::new(helper)
            .args([map, max_width.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start the iPhone helper: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = async_channel::unbounded::<Event>();
        let shots: Arc<Waiters<Result<Shot, String>>> = Arc::default();
        let inputs: Arc<Waiters<(u64, u64)>> = Arc::default();
        let (shot_waiters, input_waiters) = (shots.clone(), inputs.clone());
        let _ = std::thread::Builder::new()
            .name("trek-phone-read".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                let mut header = [0u8; 5];
                while r.read_exact(&mut header).is_ok() {
                    let len =
                        u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
                    let mut payload = vec![0u8; len];
                    if r.read_exact(&mut payload).is_err() {
                        break;
                    }
                    let event = match header[0] {
                        // Frames pile up only if nobody reads: drop this one then.
                        1 if tx.len() < 4 => parse_frame(payload),
                        2 => match parse_message(&payload) {
                            Message::Event(e) => Some(e),
                            Message::Shot(result) => {
                                if let Some(w) = shot_waiters.lock().unwrap().pop_front() {
                                    let _ = w.try_send(result);
                                }
                                None
                            }
                            Message::Hid { sent, missed } => {
                                if let Some(w) = input_waiters.lock().unwrap().pop_front() {
                                    let _ = w.try_send((sent, missed));
                                }
                                None
                            }
                            Message::Ignored => None,
                        },
                        _ => None,
                    };
                    if let Some(e) = event
                        && tx.send_blocking(e).is_err()
                    {
                        break;
                    }
                }
                // Whoever waits hears it's gone (their senders drop).
                shot_waiters.lock().unwrap().clear();
                input_waiters.lock().unwrap().clear();
                let _ = tx.send_blocking(Event::Exited);
            });
        Ok((
            Arc::new(Link {
                stdin: Mutex::new(stdin),
                child: Mutex::new(child),
                shots,
                inputs,
            }),
            rx,
        ))
    }

    fn send(&self, line: &str) {
        if let Ok(mut w) = self.stdin.lock() {
            let _ = w
                .write_all(line.as_bytes())
                .and_then(|_| w.write_all(b"\n"))
                .and_then(|_| w.flush());
        }
    }

    /// Stream frames (`fps` a second at most) or stop.
    pub fn frames(&self, on: bool, fps: u32) {
        self.send(&format!("frames {} {fps}", if on { "on" } else { "off" }));
    }

    pub fn ask_status(&self) {
        self.send("status");
    }

    /// Send reports; the answer says how many went out and how many found no phone listening,
    /// once the last pause is over.
    pub fn hid(&self, steps: &[Step]) -> async_channel::Receiver<(u64, u64)> {
        let (tx, rx) = async_channel::bounded(1);
        // In the order the helper answers: it runs them one after another.
        let mut waiters = self.inputs.lock().unwrap();
        waiters.push_back(tx);
        self.send(&hid::encode(steps));
        rx
    }

    /// The newest frame as PNG at `path`, its long side at most `max_edge`.
    pub fn shot(
        &self,
        path: &Path,
        max_edge: u32,
    ) -> async_channel::Receiver<Result<Shot, String>> {
        let (tx, rx) = async_channel::bounded(1);
        let mut waiters = self.shots.lock().unwrap();
        waiters.push_back(tx);
        self.send(&format!("shot {max_edge} {}", path.display()));
        rx
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.send("quit");
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The helper: bundled next to `exe_dir`'s executable, else built into `bin_dir` once (needs the
/// Command Line Tools' Swift compiler; ~30 s). Blocking: call off the UI thread.
pub fn ensure(exe_dir: Option<&Path>, bin_dir: &Path) -> Result<PathBuf, String> {
    if let Some(bundled) = exe_dir.map(|d| d.join(BUNDLED)).filter(|p| p.exists()) {
        return Ok(bundled);
    }
    let stamp = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        SOURCE.hash(&mut h);
        INFO_PLIST.hash(&mut h);
        h.finish()
    };
    let out = bin_dir.join(format!("{BUNDLED}-{stamp:016x}"));
    if out.exists() {
        return Ok(out);
    }
    let _ = std::fs::create_dir_all(bin_dir);
    build(&out)?;
    if let Ok(entries) = std::fs::read_dir(bin_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{BUNDLED}-")) && e.path() != out {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(out)
}

/// Compile the helper to `out` (also what bundling does).
pub fn build(out: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("trek-phone-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join("main.swift"), SOURCE).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join("Info.plist"), INFO_PLIST).map_err(|e| e.to_string())?;
    let result = Command::new("/usr/bin/xcrun")
        .args(["swiftc", "-O", "-suppress-warnings"])
        .arg(tmp.join("main.swift"))
        .args([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__info_plist",
            "-Xlinker",
        ])
        .arg(tmp.join("Info.plist"))
        .arg("-o")
        .arg(out)
        .output()
        .map_err(|e| format!("Couldn't run the Swift compiler: {e}"))?;
    let _ = std::fs::remove_dir_all(&tmp);
    if result.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&result.stderr);
    let line = err
        .lines()
        .find(|l| l.contains("error:"))
        .unwrap_or("swiftc failed (install the Command Line Tools: xcode-select --install)")
        .trim()
        .to_string();
    Err(format!("Couldn't build the iPhone helper: {line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_become_events() {
        let status = br#"{"event":"status","camera":"granted","screen":"Phone","width":1320,"height":2868,"frames":9,"frame_age":0.1,"bluetooth":"connected","hosts":1}"#;
        match parse_message(status) {
            Message::Event(Event::Status(s)) => {
                assert_eq!(s.screen, "Phone");
                assert_eq!((s.width, s.height), (1320, 2868));
                assert!(s.input_ready());
            }
            _ => panic!("status"),
        }
        match parse_message(br#"{"event":"shot","error":"locked"}"#) {
            Message::Shot(Err(e)) => assert_eq!(e, "locked"),
            _ => panic!("shot error"),
        }
        match parse_message(
            br#"{"event":"shot","path":"/t/a.png","width":660,"height":1434,"screen_width":1320,"screen_height":2868,"age":0.5}"#,
        ) {
            Message::Shot(Ok(s)) => assert_eq!((s.width, s.screen_width), (660, 1320)),
            _ => panic!("shot"),
        }
        assert!(matches!(
            parse_message(br#"{"event":"hid","sent":4,"missed":0}"#),
            Message::Hid { sent: 4, missed: 0 }
        ));
        assert!(matches!(parse_message(b"not json"), Message::Ignored));
    }

    #[test]
    fn frames_carry_the_screen_size() {
        let mut payload = vec![0x05, 0x28, 0x0B, 0x34];
        payload.extend([0xFF, 0xD8, 0xFF]);
        match parse_frame(payload) {
            Some(Event::Frame {
                jpeg,
                width,
                height,
            }) => {
                assert_eq!((width, height), (1320, 2868));
                assert_eq!(jpeg, [0xFF, 0xD8, 0xFF]);
            }
            _ => panic!("frame"),
        }
        assert!(parse_frame(vec![1, 2, 3, 4]).is_none());
    }
}
