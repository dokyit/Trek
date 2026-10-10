//! One Trek per data folder, on Windows. macOS hands a second launch's links to the running app
//! itself (`on_open_urls`, `on_reopen`); Windows starts another process, so Trek does the
//! handing over: first thing in `main`, a launch takes the data folder's lock (the one the
//! workspace keeps, see `workspace::lock_folder`) or, finding it taken, sends what it was
//! started with to the Trek holding it and exits.
//!
//! The running Trek listens on a named pipe of its own (`trek_ipc::pipe_name`: random in part,
//! carrying its pid, only its user may open it) and writes the pipe's name to `trek.pipe` in the
//! data folder, which is how a second launch finds it. The name isn't derived from the folder:
//! pipe names are machine-wide, and one anyone could work out is one another user could take
//! first. A second launch connects the way trek-mcp does (`PipeStream::connect` checks the pipe
//! is served by its user and by the pid its name carries), sends the arguments (`instance`) and
//! waits, a bounded time, for the running Trek's main thread to take them.
//!
//! The lock taken but no answer: the running Trek may be starting (it listens a moment after it
//! locks) or have just quit, so the launch tries again for a while, and takes the lock itself if
//! it comes free. Past that, the running Trek is hung: the launch says so and exits non-zero,
//! rather than opening a second Trek on the same data.
#![cfg_attr(not(windows), allow(dead_code))]

use gpui_kit::{App, Entity};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use trek_ipc::instance::Open;

use crate::workspace::Workspace;

/// How long a second launch keeps trying to hand over before it gives up on the running Trek.
const WAIT: Duration = Duration::from_secs(15);
/// How long the running Trek gives a connection to say what it wants.
const REQUEST_WITHIN: Duration = Duration::from_secs(5);
/// The file in the data folder naming the running Trek's pipe.
const PIPE_FILE: &str = "trek.pipe";

/// Arguments another launch handed over, for the main thread; `done` takes the answer back.
pub struct Forwarded {
    pub open: Open,
    pub done: async_channel::Sender<Result<(), String>>,
}

/// This launch is the Trek for its data folder.
#[derive(Default)]
pub struct Primary {
    /// The data folder's lock, for the workspace to keep (`workspace::keep_data_folder_lock`).
    pub lock: Option<std::fs::File>,
    /// What later launches hand over (`hear`).
    pub forwarded: Option<async_channel::Receiver<Forwarded>>,
    /// What this launch itself was started with: links and paths to open once the window is up.
    pub own: Vec<String>,
}

pub enum Claim {
    Primary(Primary),
    /// Handed over to the running Trek: exit.
    Forwarded,
    /// Another Trek holds the data folder and didn't take the arguments: say why, exit.
    Failed(String),
}

/// The launch's decision, first thing in `main`. Off the Windows path (and in a capture run,
/// which is isolated from any other Trek), always the Trek for its folder, as before.
pub fn claim() -> Claim {
    #[cfg(windows)]
    if std::env::var_os("TREK_SHOT_DIR").is_none() {
        let cwd = std::env::current_dir().unwrap_or_default();
        let open = Open { args: launch_args(std::env::args_os().skip(1), &cwd), background: background() };
        return claim_in(&trek_core::paths::data_dir(), open, WAIT);
    }
    Claim::Primary(Primary::default())
}

/// `TREK_BACKGROUND=1`: this launch mustn't bring Trek forward (see `main`).
fn background() -> bool {
    std::env::var("TREK_BACKGROUND").is_ok_and(|v| v == "1")
}

/// What a launch hands over: `trek://` links as they are, paths made absolute against `cwd` (the
/// running Trek has its own working folder). Flags, which Trek takes none of, are left out.
pub fn launch_args(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Vec<String> {
    args.into_iter()
        .filter_map(|a| {
            let s = a.to_string_lossy();
            if s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("trek://")) {
                return Some(s.into_owned());
            }
            if s.is_empty() || s.starts_with('-') {
                return None;
            }
            let p = PathBuf::from(&a);
            let p = if p.is_absolute() { p } else { std::path::absolute(cwd.join(&p)).unwrap_or_else(|_| cwd.join(&p)) };
            Some(p.to_string_lossy().into_owned())
        })
        .collect()
}

/// `claim` for the data folder `dir`, trying for at most `wait` to reach a running Trek.
#[cfg(windows)]
pub(crate) fn claim_in(dir: &Path, open: Open, wait: Duration) -> Claim {
    use trek_ipc::instance::ForwardError;
    let deadline = std::time::Instant::now() + wait;
    let mut why = "it hasn't said where it listens".to_string();
    loop {
        match crate::workspace::lock_folder(dir) {
            Ok(Some(lock)) => return Claim::Primary(listen(dir, lock, open.args)),
            Ok(None) => {}
            // As the workspace does when it can't lock: carry on without.
            Err(e) => {
                tracing::warn!("lock the data folder: {e}");
                return Claim::Primary(Primary { own: open.args, ..Primary::default() });
            }
        }
        if let Some(address) = published(dir) {
            if !open.background {
                allow_foreground(&address);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now()).max(Duration::from_millis(500));
            match trek_ipc::instance::forward(&address, &open, left) {
                Ok(()) => {
                    tracing::info!("single instance: handed {} argument(s) to the running Trek (pid {:?})", open.args.len(), trek_ipc::instance::served_by(&address));
                    return Claim::Forwarded;
                }
                // Not listening yet (or any more), or a pipe that isn't the running Trek's.
                Err(ForwardError::Unreachable(e)) => why = e,
                Err(e) => return Claim::Failed(format!("Trek is already running with this data folder, but {e}.")),
            }
        }
        if std::time::Instant::now() >= deadline {
            return Claim::Failed(format!("Trek is already running with this data folder ({}), but it isn't answering: {why}.", dir.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The running Trek's pipe, as it wrote it to the data folder.
#[cfg(windows)]
fn published(dir: &Path) -> Option<PathBuf> {
    let name = std::fs::read_to_string(dir.join(PIPE_FILE)).ok()?;
    let name = name.trim();
    (!name.is_empty() && name.len() < 512).then(|| PathBuf::from(name))
}

/// Let the running Trek (the pipe's process) bring its window in front of ours. Windows lets
/// only the process the user is working in do that; this launch is it, and passes it on.
#[cfg(windows)]
fn allow_foreground(address: &Path) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    if let Some(pid) = trek_ipc::instance::served_by(address) {
        // SAFETY: plain call; it fails harmlessly when this process may not pass the right on.
        unsafe { AllowSetForegroundWindow(pid) };
    }
}

/// The Trek for `dir`: listen for later launches and say where.
#[cfg(windows)]
fn listen(dir: &Path, lock: std::fs::File, own: Vec<String>) -> Primary {
    let (tx, rx) = async_channel::unbounded();
    let forwarded = match start_listening(dir, tx) {
        Ok(()) => Some(rx),
        Err(e) => {
            tracing::warn!("single instance: later launches can't reach this Trek: {e}");
            None
        }
    };
    Primary { lock: Some(lock), forwarded, own }
}

#[cfg(windows)]
fn start_listening(dir: &Path, tx: async_channel::Sender<Forwarded>) -> std::io::Result<()> {
    let address = trek_ipc::pipe_name(0)?;
    let rt = trek_core::runtime();
    let mut listener = {
        let _enter = rt.enter();
        trek_ipc::server::Listener::bind(&address)?
    };
    // Only once it's listening, so the name in the file is always one with a Trek behind it.
    publish(dir, &address)?;
    rt.spawn(async move {
        loop {
            match listener.accept().await {
                Ok(stream) => {
                    let tx = tx.clone();
                    tokio::spawn(trek_ipc::server::serve_open(stream, REQUEST_WITHIN, move |open| hand_over(tx, open)));
                }
                // The error comes straight back: pause rather than spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    });
    Ok(())
}

/// Pass `open` to the main thread and wait for it to take it.
async fn hand_over(tx: async_channel::Sender<Forwarded>, open: Open) -> Result<(), String> {
    let (done, answer) = async_channel::bounded(1);
    tx.send(Forwarded { open, done }).await.map_err(|_| "Trek is quitting".to_string())?;
    answer.recv().await.unwrap_or_else(|_| Err("Trek is quitting".into()))
}

/// Write the pipe's name where a second launch looks, whole or not at all.
fn publish(dir: &Path, address: &Path) -> std::io::Result<()> {
    let tmp = dir.join(format!("{PIPE_FILE}.{}", std::process::id()));
    std::fs::write(&tmp, address.to_string_lossy().as_bytes())?;
    std::fs::rename(&tmp, dir.join(PIPE_FILE)).inspect_err(|_| _ = std::fs::remove_file(&tmp))
}

/// A launch that couldn't hand over: say why where the user will see it (a message box, as a
/// release build has no console; not for a launch in the background), and in the log.
pub fn tell(why: &str) {
    tracing::error!("single instance: {why}");
    eprintln!("{why}");
    #[cfg(windows)]
    if !background() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, title) = (wide(&format!("{why}\n\nClose it (from Task Manager if need be) and open Trek again.")), wide("Trek"));
        // SAFETY: NUL-terminated strings that outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
    }
}

/// Once the main window is up: open what this launch was started with, then take what later
/// launches hand over, on the main thread, as macOS's `on_open_urls` and `on_reopen` would.
pub fn hear(primary_forwarded: Option<async_channel::Receiver<Forwarded>>, own: Vec<String>, ws: Entity<Workspace>, links: async_channel::Sender<String>, cx: &mut App) {
    for arg in own {
        dispatch(arg, &links, cx);
    }
    let Some(rx) = primary_forwarded else { return };
    cx.spawn(async move |cx| {
        while let Ok(Forwarded { open, done }) = rx.recv().await {
            let _ = cx.update(|cx| take(open, &ws, &links, cx));
            let _ = done.try_send(Ok(()));
        }
    })
    .detach();
}

/// Another launch's arguments: Trek comes forward (unless that launch was in the background),
/// then each opens.
pub(crate) fn take(open: Open, ws: &Entity<Workspace>, links: &async_channel::Sender<String>, cx: &mut App) {
    let links_n = open.args.iter().filter(|a| is_link(a)).count();
    tracing::info!(
        "single instance: another launch handed over {} link(s) and {} path(s){}",
        links_n,
        open.args.len() - links_n,
        if open.background { ", in the background" } else { "" }
    );
    if !open.background {
        crate::root::show_main(ws.clone(), cx);
    }
    for arg in open.args {
        dispatch(arg, links, cx);
    }
}

fn is_link(arg: &str) -> bool {
    arg.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("trek://"))
}

/// A link goes where macOS's `on_open_urls` sends links; a file opens in the editor as a
/// `trek://edit` link to it would (asking first when it's outside the user's projects).
/// Anything else (a folder, a path that isn't there) only brought Trek forward.
fn dispatch(arg: String, links: &async_channel::Sender<String>, cx: &mut App) {
    if is_link(&arg) {
        let _ = links.try_send(arg);
        return;
    }
    let path = PathBuf::from(arg);
    if path.is_file() {
        crate::deep_link::open_file(path, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_go_as_they_are_and_paths_are_made_absolute() {
        let cwd = std::env::temp_dir();
        let args = ["trek://edit?path=%2Fx&line=2", "TREK://ask?path=%2Fy", "notes.md", "-flag", "", "sub/dir"].map(OsString::from);
        let got = launch_args(args, &cwd);
        assert_eq!(got[..2], ["trek://edit?path=%2Fx&line=2", "TREK://ask?path=%2Fy"]);
        assert_eq!(got.len(), 4, "no flags, no empty arguments: {got:?}");
        assert_eq!(PathBuf::from(&got[2]), std::path::absolute(cwd.join("notes.md")).unwrap());
        assert!(PathBuf::from(&got[3]).is_absolute() && got[3].ends_with("dir"));
        let abs = cwd.join("already.rs");
        assert_eq!(launch_args([abs.clone().into_os_string()], Path::new("/elsewhere")), [abs.to_string_lossy().into_owned()]);
    }

    #[test]
    fn the_pipe_s_name_is_published_whole() {
        let dir = std::env::temp_dir().join(format!("trek-publish-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        publish(&dir, Path::new(r"\\.\pipe\trek-1-0-ab")).unwrap();
        publish(&dir, Path::new(r"\\.\pipe\trek-2-0-cd")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(PIPE_FILE)).unwrap(), r"\\.\pipe\trek-2-0-cd", "the newer replaces the older");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use std::time::Instant;

        fn data_dir(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("trek-si-{tag}-{}-{}", std::process::id(), &trek_ipc::token().unwrap()[..8]));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn open(args: &[&str]) -> Open {
            Open { args: args.iter().map(|a| a.to_string()).collect(), background: true }
        }

        /// The first launch takes the folder and listens; a second hands its arguments to the first
        /// (whose main thread a test thread stands in for) and is done.
        #[test]
        fn a_second_launch_hands_over_to_the_first() {
            let dir = data_dir("pair");
            let Claim::Primary(first) = claim_in(&dir, open(&["trek://edit?path=%2Fown"]), Duration::from_secs(2)) else { panic!("the folder was free") };
            assert!(first.lock.is_some());
            assert_eq!(first.own, ["trek://edit?path=%2Fown"], "its own arguments wait for the window");
            let rx = first.forwarded.expect("listening");
            let main = std::thread::spawn(move || {
                let f = rx.recv_blocking().unwrap();
                f.done.try_send(Ok(())).unwrap();
                f.open
            });
            let started = Instant::now();
            assert!(matches!(claim_in(&dir, open(&["trek://ask?path=%2Fa", r"C:\src\b.rs"]), Duration::from_secs(10)), Claim::Forwarded));
            assert!(started.elapsed() < Duration::from_secs(5), "quickly: {:?}", started.elapsed());
            assert_eq!(main.join().unwrap(), open(&["trek://ask?path=%2Fa", r"C:\src\b.rs"]));
            let _ = std::fs::remove_dir_all(dir);
        }

        /// The folder locked, but nothing listening yet (a Trek starting up): the launch keeps
        /// trying, and takes the folder once the lock comes free rather than giving up.
        #[test]
        fn a_lock_without_a_listener_is_waited_out() {
            let dir = data_dir("race");
            let lock = crate::workspace::lock_folder(&dir).unwrap().unwrap();
            let freed = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                drop(lock);
            });
            let started = Instant::now();
            let Claim::Primary(p) = claim_in(&dir, open(&[]), Duration::from_secs(10)) else { panic!("should start once the lock is free") };
            assert!(p.lock.is_some() && started.elapsed() >= Duration::from_millis(350));
            freed.join().unwrap();
            drop(p);
            let _ = std::fs::remove_dir_all(dir);
        }

        /// The folder locked by a Trek that never answers: the launch gives up after its time,
        /// and says so, rather than starting a second Trek on the same folder.
        #[test]
        fn a_trek_that_never_answers_is_given_up_on() {
            let dir = data_dir("hung");
            let _lock = crate::workspace::lock_folder(&dir).unwrap().unwrap();
            let started = Instant::now();
            let Claim::Failed(why) = claim_in(&dir, open(&["trek://edit?path=%2Fx"]), Duration::from_millis(600)) else { panic!("must not start") };
            assert!(started.elapsed() < Duration::from_secs(3));
            assert!(why.contains("isn't answering"), "{why}");

            // Listening, but its main thread never takes the arguments: hung, not starting.
            let other = data_dir("hung2");
            let Claim::Primary(first) = claim_in(&other, open(&[]), Duration::from_secs(2)) else { panic!() };
            let _rx = first.forwarded;
            let started = Instant::now();
            let Claim::Failed(why) = claim_in(&other, open(&[]), Duration::from_millis(800)) else { panic!("must not start") };
            assert!(started.elapsed() < Duration::from_secs(4));
            assert!(why.contains("didn't answer in time"), "{why}");
            let _ = std::fs::remove_dir_all(dir);
            let _ = std::fs::remove_dir_all(other);
        }

        /// A stale `trek.pipe` (a Trek that crashed) with the folder free: this launch is the Trek
        /// now, and the file names its own pipe. With the folder locked, a stale or foreign name
        /// is never trusted: only an answer counts.
        #[test]
        fn a_stale_pipe_name_is_replaced_not_trusted() {
            let dir = data_dir("stale");
            let gone = trek_ipc::pipe_name(5).unwrap();
            publish(&dir, &gone).unwrap();
            let Claim::Primary(p) = claim_in(&dir, open(&[]), Duration::from_secs(2)) else { panic!("the folder was free") };
            let now = published(&dir).unwrap();
            assert_ne!(now, gone);
            assert!(trek_ipc::Stream::connect(&now).is_ok(), "the new name has a Trek behind it");
            drop(p);

            let locked = data_dir("stale2");
            let _lock = crate::workspace::lock_folder(&locked).unwrap().unwrap();
            for name in [gone.to_string_lossy().into_owned(), r"\\.\pipe\not-trek".into(), "garbage".into()] {
                std::fs::write(locked.join(PIPE_FILE), &name).unwrap();
                assert!(matches!(claim_in(&locked, open(&[]), Duration::from_millis(300)), Claim::Failed(_)), "{name}");
            }
            let _ = std::fs::remove_dir_all(dir);
            let _ = std::fs::remove_dir_all(locked);
        }
    }
}
