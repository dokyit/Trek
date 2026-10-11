//! A process Trek starts opens no console window on Windows.
//!
//! Trek is a GUI program with no console of its own, so a child started the plain way gets a
//! console window each (an empty dark frame that takes the foreground), and `read_git_info`'s nine
//! git processes opened nine. In a terminal the children share the terminal's console, which is
//! why runs from `cargo` never showed it. This starts the children the way Trek does from a
//! process that has no console (`DETACHED_PROCESS`), and watches for new console windows: a
//! `ConsoleWindowClass` one, or Windows Terminal's (`CASCADIA_HOSTING_WINDOW_CLASS`), which is
//! what a new console becomes where Terminal is the default (Windows 11's). Terminal's window
//! belongs to Terminal, not to the child, so a window counts by being new and visible, not by
//! whose it is.
//!
//! The probe runs in a copy of this test binary, because the test's own process has the terminal's
//! console (or doesn't, depending on who ran it); the copy never has one.
#![cfg(windows)]

use std::collections::HashSet;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};

/// Set in the copy that runs the probe.
const PROBE: &str = "TREK_QUIET_SPAWNS_PROBE";
/// How long a lingering child is watched: "within a second".
const WATCH: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
struct Window {
    hwnd: isize,
    pid: u32,
    class: String,
}

impl Window {
    /// A window a new console opens in.
    fn is_console(&self) -> bool {
        matches!(self.class.as_str(), "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS")
    }
}

/// Every visible top-level window there is, whoever owns it.
fn windows() -> Vec<Window> {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        // SAFETY: `found` is the `&mut Vec<Window>` handed to `EnumWindows` below.
        let found = unsafe { &mut *(found as *mut Vec<Window>) };
        // SAFETY: any window handle may be asked.
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            let mut class = [0u16; 64];
            // SAFETY: a buffer of the length it's told.
            let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
            let mut pid = 0;
            // SAFETY: `pid` is a u32 to write to.
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            found.push(Window { hwnd: hwnd as isize, pid, class: String::from_utf16_lossy(&class[..n.max(0) as usize]) });
        }
        1
    }
    let mut found = Vec::new();
    // SAFETY: the callback only touches `found`, which outlives the call.
    unsafe { EnumWindows(Some(each), &mut found as *mut Vec<Window> as LPARAM) };
    found
}

/// What opened while a child ran: console windows (what the test is about), and any other window
/// (for the failure message: what else did this machine show?).
#[derive(Default, Debug)]
struct Opened {
    consoles: Vec<Window>,
    others: Vec<Window>,
}

/// The windows that open while `child` runs (at most `WATCH`) and for a moment after it exits:
/// the polling is fast because a short-lived child's window is up for a few tens of milliseconds
/// only.
fn opened_during(child: &mut Child, before: &HashSet<isize>) -> Opened {
    let start = Instant::now();
    let mut opened = Opened::default();
    let mut exited = None;
    loop {
        for w in windows().into_iter().filter(|w| !before.contains(&w.hwnd)) {
            let list = if w.is_console() { &mut opened.consoles } else { &mut opened.others };
            if !list.iter().any(|seen| seen.hwnd == w.hwnd) {
                list.push(w);
            }
        }
        if exited.is_none() && child.try_wait().unwrap().is_some() {
            exited = Some(Instant::now());
        }
        let done = match exited {
            Some(at) => at.elapsed() > Duration::from_millis(250),
            None => start.elapsed() > WATCH,
        };
        if done {
            return opened;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn known_windows() -> HashSet<isize> {
    windows().iter().map(|w| w.hwnd).collect()
}

/// How a test starts a program: the plain way, or Trek's.
#[derive(Clone, Copy)]
enum Way {
    Plain,
    Trek,
}

impl Way {
    fn command(self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        match self {
            Way::Plain => Command::new(program),
            Way::Trek => trek_core::procs::command(program),
        }
    }
}

/// Start `command` with its output thrown away and its stdin open (so a program that reads it, as
/// git does when Trek feeds it, waits there), and report the windows that open for it.
fn watch(mut command: Command) -> Opened {
    let before = known_windows();
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the program starts");
    let opened = opened_during(&mut child, &before);
    drop(child.stdin.take());
    trek_core::procs::end_tree(child.id() as i32);
    let _ = child.kill();
    let _ = child.wait();
    opened
}

/// What the probe starts, each a way Trek starts things: git, a `.cmd` wrapper (npm's
/// `codex.cmd`), `cmd /c`, a program. All but `git --version` linger, so a window of theirs has
/// time to show: that one is over in a few tens of milliseconds, often before a window could open
/// (the plain way shows none either), which is why git is also run waiting on its stdin.
fn cases(way: Way, fixture: &str, wrapper: &str) -> Vec<(&'static str, Command)> {
    let mut git = way.command("git");
    git.arg("--version");
    let mut git_waiting = way.command("git");
    git_waiting.args(["hash-object", "--stdin"]);
    let mut script = way.command(wrapper);
    script.arg("30");
    let mut cmd = way.command("cmd");
    cmd.args(["/c", fixture, "sleep", "30"]);
    let mut program = way.command(fixture);
    program.args(["sleep", "30"]);
    vec![("git --version", git), ("git waiting on stdin", git_waiting), ("a .cmd wrapper", script), ("cmd /c", cmd), ("a program that lingers", program)]
}

/// Runs in the copy without a console: panics, with what it saw, if Trek's way of starting things
/// opens a window.
#[test]
fn probe_without_console() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let fixture = trek_test_fixtures::bin("fixture").to_string_lossy().into_owned();
    let dir = std::env::temp_dir().join(format!("trek-quiet-spawns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wrapper = dir.join("wrapper.cmd");
    std::fs::write(&wrapper, format!("@echo off\r\n\"{fixture}\" sleep %1\r\n")).unwrap();
    let wrapper = wrapper.to_string_lossy().into_owned();

    // The control: the plain way does open windows from a process like this one. (If it didn't on
    // some machine, the check below would prove nothing there, and it says so.)
    let mut control = false;
    for (name, command) in cases(Way::Plain, &fixture, &wrapper) {
        let opened = watch(command);
        eprintln!("plain Command::new, {name}: {} console window(s); other new windows: {:?}", opened.consoles.len(), opened.others);
        control |= !opened.consoles.is_empty();
    }
    if !control {
        eprintln!("WARNING: the plain way opened no console window on this machine, so this check proves nothing here");
    }

    let mut wrong = Vec::new();
    for (name, command) in cases(Way::Trek, &fixture, &wrapper) {
        let opened = watch(command);
        eprintln!("trek_core::procs::command, {name}: {} console window(s)", opened.consoles.len());
        if !opened.consoles.is_empty() {
            wrong.push(format!("{name}: {:?}", opened.consoles));
        }
    }

    // The async twin, which agent CLIs and update commands go through.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let windows = runtime.block_on(async {
        let before = known_windows();
        let mut command = trek_core::procs::async_command(&fixture);
        let mut child = command.args(["sleep", "30"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
        let start = Instant::now();
        let mut found: Vec<Window> = Vec::new();
        while start.elapsed() < WATCH {
            for w in windows().into_iter().filter(|w| w.is_console() && !before.contains(&w.hwnd)) {
                if !found.iter().any(|f| f.hwnd == w.hwnd) {
                    found.push(w);
                }
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let _ = child.kill().await;
        found
    });
    eprintln!("trek_core::procs::async_command, a program that lingers: {} console window(s)", windows.len());
    if !windows.is_empty() {
        wrong.push(format!("async_command: {windows:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wrong.is_empty(), "Trek's way of starting a process opened console windows: {wrong:#?}");
}

#[test]
fn trek_spawns_open_no_console_window() {
    if std::env::var_os(PROBE).is_some() {
        return;
    }
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_without_console", "--nocapture", "--test-threads=1"])
        .env(PROBE, "1")
        .stdin(Stdio::null())
        .creation_flags(DETACHED_PROCESS)
        .output()
        .unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{said}");
    assert!(out.status.success(), "the probe in a process with no console failed:\n{said}");
    assert!(said.contains("1 passed"), "the probe didn't run:\n{said}");
}
