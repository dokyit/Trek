//! Design review without Screen Recording: a build with the `shots` feature watches
//! `$TREK_SHOT_DIR/cmd` and runs the commands it finds there, one per line, against the main
//! window, then deletes the file. `shot <name>` draws the window's current frame to
//! `<name>.png` in that folder; see-through areas (liquid glass) are laid over `$TREK_SHOT_UNDER`,
//! blurred, as macOS would show the desktop. Run it with `TREK_FORCE_ACTIVE=1` so a window kept
//! behind others still draws. When `TREK_SHOT_DIR` is set the process is isolated (see
//! `trek_core::paths::isolate`): the Keychain stays untouched and only the mock agent runs, so a
//! manifest can never reach a real agent or a real account.
//!
//! A batch ends by writing `done` in the same folder: `ok`, or one `err <line>: <message>` per
//! command that failed. Drivers must remove a stale `done` before writing `cmd`, and write `cmd`
//! atomically (a temp file renamed into place) so a half-written batch is never run.
//!
//! Commands: `route draft|no-project|basecamp|notes|appearance|settings:<page>|thread:<id>|title:<q>|project:<path>|first`,
//! `send <prompt>`, `attach <path>[|<path>…]` (into the main composer, as a drop: images as thumbnails, other files as tiles), `note text <markdown>|image <path>…|mode write|split|preview` (Notes; see `run`), `project <folder>` (added, and a draft in it), `diff` (the latest turn's changes in the Git tool),
//! `pair` (a pairing code, its link written to `pair.txt`), `pair-phone [name]|leave` (a stand-in iPhone paired over the protocol, connected or gone), `heic-demo` (an iPhone photo made by the system's codec, attached), `open-link <trek:// link>` (as the system hands Trek one), `file <path> [contains <text>]` and `private <path>` (in the data folder unless absolute, `*name` for a name's end: there, holding `text`, and readable by the user alone), `second-launch <arg>` (Windows: Trek started again, handing `arg` over), `keep-working` / `show-main` (the windows closed with an agent at work, and the main one back), `close-main` (Windows: the close button's own path, with Keep Trek in the tray on), `badge <n>[+]` (the count on the app's icon), `count-all <id> <n>[+]` (`count` over every window), `tray off|on|<state>` (the system's tray or menu bar icon), `programs <name> <n>[+]` / `programs-ended` (programs running under Trek, then gone), `console-windows mark|new <n>` (Windows: console windows on screen), `native` (not translated or emulated), `banners mark|new <n>[+]` (Windows: banners in the notification center), `renders mark|<view> <n>+|<n>-|<=<view>…` (how often views drew), `within <id>[*] <id>` (inside another, as drawn), `banner-permission off|restore` and `file-app <ext> on|off` (Windows: Trek's switch in the notification settings; a default app of the check's own), `cloud-file <abs-path> <text>|fetched <n>[+]|away|off` (Windows: a file that lives only in the cloud, from a sync provider of the check's own), `editor has <text>|note <text>` (the file in front shows it; the preview in front says it), `photos open|shown <0|1>|close` (Mac: the system's Photos picker as the + menu opens it, a sheet on the window), `bonjour on|found|off` (the phone server announced through the system's responder, under a name of the check's own, and found as a phone finds it), `programs-stop`, `motion on|system` (animations whatever the system asks, or back to its setting), `signed-out` (`gh`, as Trek starts it, has no login: a capture run never acts as the user), `self-contained` (no runtime from elsewhere loaded), `browser crash` / `browser expect crashed|failed[:<address>]|loaded|why:<word>` (the page's process ended; the tab as the panel has it, and the reason Trek's page gives), `pair-phone upload <name>` (the stand-in sends a file up), `push on|test|alert`, `new` (⌘N), `settled [on|off]`,
//! `glass on|off`, `tint <0.2–0.95>`, `theme night|paper`, `title <text>` (renames the thread on screen), `dress <Agent>|<Fast model>|<Deep model>` or `dress off` (the mock agent under other names, for marketing frames of a whole window; captioned as examples wherever they're shown), `tools on|off|git|explorer|terminal|browser|sidechat|simulator`,
//! `range today|week|all` (Basecamp's), `accounts-demo` (accounts for Settings, the picker and the Usage card), `compact-demo` (the thread on screen large and idle past its prompt cache, for the Compact chip), `secret-demo` (a secret's request card), `usage-demo` (plans and limits for the Usage card, no real agent asked), `limits-demo` (the same windows for the mock agent, for the composer's context card), `update-demo ready|wait|later|clear` (an update staged, for the updater's states with work running), `resume-demo resuming|failed|offered|waiting` (the thread on screen's offer after a relaunch), `pace <f>` (the mock agent's speed: 1.0 demo, 0 instant),
//! `wait <ms>` or `wait idle|permission|question|plan [cap ms]`, `record <name> <ms> [fps]` (frames
//! to `<name>.frames/` plus `<name>.ffconcat` for ffmpeg's concat demuxer; `record stop`, `record wait`),
//! `editor on|off|root|view|panel|chat|ai|open …` (Trek IDE; see `run`), `terminal feed <text>|link <n>`
//! (the terminal on screen: text as if its shell printed it, or its n-th link opened as a ⌘-click would), `agent-install <id> <percent>|fail <message>|clear`
//! (an ACP Registry install's row, without downloading), `toast [undo|error] <message>` (a toast, with
//! an Undo or the error icon), `shot <name>`, `quit`.
//!
//! On a Mac, `TREK_TEST_DISPLAY=1` gives the run a display of its own, so none of its windows is
//! on the owner's screens and macOS keeps drawing them (`test_display.rs`); `windows` writes
//! every window's number, display and frame to `windows.json`, for a capture of real pixels
//! (`trek-dev screenshot --real`: `screencapture` of the window's rectangle there).
//!
//! Interactive artifacts (`trek-html` cards; their pages are native views too): `artifacts` writes
//! each card's state and frame, and the window's frame on screen, to `artifacts.json`.
//!
//! The Browser tool (open it with `tools browser`; its page is a native view, so `shot` doesn't
//! show it): `browser go <address>|back|forward|reload`, `browser state` (the page, the native
//! view's rectangle beside the panel's and which window has the keys, to `browser.json`),
//! `browser focus|unfocus` (the page takes the keys, or gives them back), `browser eval <js>` (the
//! page's answer, as JSON, to `browser-eval.json`), `browser press <keys>` (macOS: a real key
//! event such as `cmd-v`, through AppKit as the keyboard sends it), and
//! `browser snap <name>` (the page as it draws, to `<name>.png`: WebView2's capture on Windows,
//! WebKit's snapshot on macOS, even with the window behind others).
//!
//! Input without a pointer or a keyboard (events dispatched to the window, never real OS input):
//! `click|rclick|hover <element id>` (`name#3` for a row's id; `elements` writes the ids on screen
//! to `elements.txt`), `type <text>` (into the focused field, else the main composer),
//! `count <element id>[*] <n>[+]` (fails unless n such elements are on screen, or n or more with `+`; `*` ends a prefix), `reveal <element id>` (Settings scrolled until it is on screen; `reveal-in <scrolling element id> <element id>` for another),
//! `key <keystroke>…` (`secondary-k`: ⌘K on a Mac, Ctrl+K on Windows; `escape`, `shift-tab`,
//! `down down enter`; `cmd`/`win`/`super` modifiers are read as `secondary`, so one manifest is
//! right on both platforms), `scroll <element id> <px>`
//! (a wheel over that element, positive down the page), `resize <w> <h>` (the main window, logical
//! px). On Windows `hittest <element id>|<x> <y> [HTCLIENT|HTCAPTION|…]` asks the window what it
//! answers to `WM_NCHITTEST` there (to `hittest.txt`; a different answer than the one given fails the
//! command) and `realclick <element id>|<x> <y>` sends the window the move, press and release of a real
//! click, to its own handle, where it answers `HTCLIENT` (see `hittest`); `realmove <element id>|<x> <y>`
//! is the move alone, with the pointer left there, and `realleave` is Windows telling the window
//! the pointer has left it; `legacy-pointer on|off` handles the pointer as 0.4.1-beta.1 did (every
//! title bar control the window's handle; a pointer leaving the window leaving the sidebar's rows
//! held), to show what it did then. The thread on screen: `approve` / `deny` its waiting permission, plan
//! or question card, `answer <n>|<text>` (option n of each question, or a typed answer), and
//! `rewind [n] [keep]` (undo the n-th turn from the end, 1 = the latest, as its Undo does; `keep`
//! leaves the files).
//!
//! Measurements (with `TREK_PERF=1`; each logs one `perf:` line, see `perf.rs`): `perf frames
//! <ms> <label>` (every frame painted meanwhile, and how late the main thread answers a 5 ms
//! timer, `lag_`), `perf scroll <element id> <px> <steps> <label>`
//! (a wheel step of `px` over that element each 60 Hz frame, each step timed to its paint),
//! `perf type <count> <label>` (`count` characters typed one at a time into the focused field,
//! each timed to its paint), `perf open <title>` (the thread with this in its title opens: until its
//! history is loaded, and the frame that shows it), `perf mem <label>` (what Trek holds: the
//! transcripts in memory and the main window's parsed answers, pictures and page snapshots) and
//! `perf mark <label>`. `perf scroll @center …` turns the wheel over the middle of the window.
//! Lines starting with `#` are comments.

use crate::workspace::{PanelTool, Route, SettingsPage, Workspace};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod cloud_files;
#[cfg(any(windows, target_os = "macos"))]
mod computer_use;
#[cfg(any(windows, target_os = "macos"))]
mod dns_sd;

pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let Some(dir) = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from) else {
        return;
    };
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm)
            .setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
    }
    let _ = std::fs::create_dir_all(&dir);
    // Weak between batches: this loop outlives the app's own wind-down, and a handle it held
    // would be one left when GPUI drops its entities (the leak detector panics on that).
    let workspace = workspace.downgrade();
    // Idle measurements look for commands less often, to count Trek's wakeups and not these.
    let poll = std::env::var("TREK_SHOT_POLL_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(300);
    cx.spawn(async move |cx| {
        let mut recording: Option<Recording> = None;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(poll))
                .await;
            let cmd = dir.join("cmd");
            let Ok(text) = std::fs::read_to_string(&cmd) else {
                continue;
            };
            let _ = std::fs::remove_file(&cmd);
            let Some(workspace) = workspace.upgrade() else {
                break;
            };
            let mut errors = Vec::new();
            for line in text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
                let result = match verb {
                    "wait" => wait(&workspace, arg, cx).await,
                    "shot" => shot(&workspace, &dir, arg, cx).await,
                    "browser" => browser(&workspace, &dir, arg, cx).await,
                    "artifacts" => artifacts(&workspace, &dir, cx).await,
                    "editor" if arg.starts_with("snap ") => {
                        editor_snap(&workspace, &dir, arg["snap ".len()..].trim(), cx).await
                    }
                    "hittest" | "realclick" | "realmove" | "realleave" => {
                        hittest(&workspace, &dir, verb, arg, cx).await
                    }
                    "legacy-pointer" => cx.update(|cx| match arg {
                        "on" | "off" => {
                            crate::chrome::LEGACY_POINTER.store(arg == "on", Ordering::Relaxed);
                            cx.refresh_windows();
                            Ok(())
                        }
                        _ => Err(anyhow::anyhow!("legacy-pointer on|off")),
                    }),
                    "record" => record(&workspace, &dir, arg, &mut recording, cx).await,
                    "perf" => perf(&workspace, arg, cx).await,
                    "second-launch" => second_launch(arg, cx).await,
                    #[cfg(target_os = "macos")]
                    "windows" => cx.update(|cx| windows(&workspace, &dir, cx)),
                    "quit" => quit(cx),
                    _ => cx.update(|cx| run(&workspace, verb, arg, cx)),
                };
                if let Err(e) = result {
                    tracing::warn!("shot command failed: {line}: {e:#}");
                    errors.push(format!("err {line}: {e:#}"));
                }
            }
            let _ = std::fs::write(
                dir.join("done"),
                if errors.is_empty() {
                    "ok".to_string()
                } else {
                    errors.join("\n")
                },
            );
        }
    })
    .detach();
}

/// A capture run (`TREK_SHOT_DIR`) never acts as the user anywhere. `gh` (a share's gist, a pull
/// request's checks) finds the user's GitHub login through its config folder and these variables,
/// so the process gets an empty folder of its own and none of them: whatever it starts is signed
/// out. A live check once made a real gist on the user's account through it. First thing in
/// `main`, before any other thread exists to read the environment.
pub fn isolate_accounts() {
    if std::env::var_os("TREK_SHOT_DIR").is_none() {
        return;
    }
    let base = std::env::var_os("TREK_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::temp_dir().join(format!("trek-shots-{}", std::process::id())));
    let gh = base.join("gh-signed-out");
    let _ = std::fs::create_dir_all(&gh);
    // SAFETY: first thing in `main` (`run`), before any other thread is started.
    unsafe {
        std::env::set_var("GH_CONFIG_DIR", &gh);
        for token in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            std::env::remove_var(token);
        }
    }
}

/// `second-launch <arg>`: Trek started again with `arg` (a trek:// link, a path), as a shortcut or
/// the system starts it. On Windows the new process hands `arg` to this Trek (`single_instance`,
/// within the capture run's own data folder) and exits. It's waited for with the main thread
/// free, since taking what it hands over is the main thread's, and passes only if it exits handed
/// over (0) within the time a launch keeps trying for, rather than staying up as a second Trek. A
/// Mac hands a second launch to the running app itself (`on_open_urls`), as `open-link` does.
async fn second_launch(arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    anyhow::ensure!(
        cfg!(windows),
        "second-launch: a Mac hands a second launch to the running app itself"
    );
    let mut launch = trek_core::procs::command(std::env::current_exe()?)
        .arg(arg.trim())
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = launch.try_wait()? {
            anyhow::ensure!(
                status.success(),
                "second-launch: the second Trek couldn't hand over ({status}; see the log)"
            );
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let _ = launch.kill();
            let _ = launch.wait();
            anyhow::bail!(
                "second-launch: the second Trek was still running after 20 s, rather than handing over and exiting"
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}

/// `native`: on Windows the machine's own processor (`IsWow64Process2`, which sees through x64
/// emulation on Arm) is the one Trek was built for; on a Mac, Trek isn't translated by Rosetta
/// (`sysctl.proc_translated`).
fn native_build() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{
            IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
        let (mut process, mut machine) = (0u16, 0u16);
        // SAFETY: this process's own pseudo handle, and two answers to fill.
        let asked = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut machine) };
        anyhow::ensure!(asked != 0, "native: {}", std::io::Error::last_os_error());
        let built = if cfg!(target_arch = "aarch64") {
            IMAGE_FILE_MACHINE_ARM64
        } else {
            IMAGE_FILE_MACHINE_AMD64
        };
        anyhow::ensure!(
            machine == built,
            "native: this PC's processor is {machine:#06x}, and Trek was built for {built:#06x}"
        );
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let mut translated: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: a NUL-terminated name, and an int-sized answer of `size` bytes.
        let known = unsafe {
            libc::sysctlbyname(
                c"sysctl.proc_translated".as_ptr(),
                (&mut translated as *mut libc::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        // Unknown on a Mac with no Rosetta at all: nothing translates there.
        anyhow::ensure!(
            !(known && translated == 1),
            "native: Trek runs translated, by Rosetta"
        );
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    anyhow::bail!("native: Windows and macOS only")
}

/// `self-contained`; see the verb.
fn self_contained() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        // What a program built against the redistributable loads as it starts.
        for dll in [
            "vcruntime140.dll",
            "vcruntime140_1.dll",
            "msvcp140.dll",
            "vcomp140.dll",
        ] {
            let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
            // SAFETY: a NUL-terminated name; nothing is loaded, or its count changed.
            let loaded = unsafe { GetModuleHandleW(wide.as_ptr()) };
            anyhow::ensure!(loaded.is_null(), "self-contained: {dll} is loaded in Trek");
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // dyld's list of what's loaded, from libSystem (the libc crate's copies are deprecated
        // in favour of another crate, for these two calls alone).
        unsafe extern "C" {
            fn _dyld_image_count() -> u32;
            fn _dyld_get_image_name(image_index: u32) -> *const std::ffi::c_char;
        }
        let exe = std::env::current_exe()?;
        let own = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut foreign = Vec::new();
        // SAFETY: dyld's own list, read by index below its count; each name is NUL-terminated.
        for i in 0..unsafe { _dyld_image_count() } {
            let name = unsafe { _dyld_get_image_name(i) };
            if name.is_null() {
                continue;
            }
            let path = unsafe { std::ffi::CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            let system = ["/usr/lib/", "/System/", "/Library/Apple/"]
                .iter()
                .any(|p| path.starts_with(p));
            if !system && !Path::new(&path).starts_with(&own) {
                foreign.push(path);
            }
        }
        anyhow::ensure!(
            foreign.is_empty(),
            "self-contained: Trek loaded libraries from elsewhere: {}",
            foreign.join(", ")
        );
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    anyhow::bail!("self-contained: Windows and macOS only")
}

/// The banners `banners mark` noted, by tag.
#[cfg(windows)]
static BANNERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The tags of the banners in Windows' notification center under Trek's app id.
#[cfg(windows)]
fn banner_tags() -> anyhow::Result<Vec<String>> {
    use windows::UI::Notifications::ToastNotificationManager;
    let id = windows::core::HSTRING::from(crate::winsys::APP_ID);
    let list = ToastNotificationManager::History()?.GetHistoryWithId(&id)?;
    let mut tags = Vec::new();
    for banner in list {
        tags.push(banner.Tag()?.to_string());
    }
    Ok(tags)
}

/// What `banner-permission off` found, for `restore`: the switch's value then (None: none).
#[cfg(windows)]
static BANNER_SWITCH: Mutex<Option<Option<u32>>> = Mutex::new(None);

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A DWORD under HKEY_CURRENT_USER; None when it isn't there.
#[cfg(windows)]
fn read_dword(key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let (key, value) = (wide(key), wide(value));
    let (mut data, mut size) = (0u32, 4u32);
    // SAFETY: NUL-terminated names, and a DWORD's four bytes to fill.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

/// Set a DWORD under HKEY_CURRENT_USER (the key made if need be), or with None remove it.
#[cfg(windows)]
fn write_dword(key: &str, value: &str, data: Option<u32>) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_DWORD, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let (k, v) = (wide(key), wide(value));
    // SAFETY: NUL-terminated names, and a DWORD's four bytes.
    let status = unsafe {
        match data {
            Some(d) => RegSetKeyValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                v.as_ptr(),
                REG_DWORD,
                (&d as *const u32).cast(),
                4,
            ),
            None => RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr()),
        }
    };
    // A value that's already gone is as asked (ERROR_FILE_NOT_FOUND).
    anyhow::ensure!(
        status == 0 || data.is_none() && status == 2,
        "registry {key}\\{value}: error {status}"
    );
    Ok(())
}

/// Set a key's default string under HKEY_CURRENT_USER, the key made if need be.
#[cfg(windows)]
fn write_string(key: &str, text: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    let (k, data) = (wide(key), wide(text));
    // SAFETY: a NUL-terminated key, the default value (null name), and the string's bytes with
    // its NUL.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            std::ptr::null(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    anyhow::ensure!(status == 0, "registry {key}: error {status}");
    Ok(())
}

/// Remove a key and everything under it from HKEY_CURRENT_USER, if it's there.
#[cfg(windows)]
fn delete_tree(key: &str) {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    let k = wide(key);
    // SAFETY: a NUL-terminated key name.
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
}

/// Tell Explorer the file associations changed, so the next open reads them again.
#[cfg(windows)]
fn associations_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: a plain notice with no items.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
}

/// End process `pid`.
fn end_process(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        // SAFETY: plain calls on a handle that's checked, used once and closed.
        unsafe {
            let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !process.is_null() {
                TerminateProcess(process, 1);
                CloseHandle(process);
            }
        }
    }
    #[cfg(unix)]
    // SAFETY: a signal to a process id; nothing else is touched.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

/// What `programs` found last, for `programs-ended`: process ids and their programs.
static PROGRAMS: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

/// The windows `console-windows mark` noted.
#[cfg(windows)]
static CONSOLES: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Every process under Trek's own, however deep, with the program each runs.
fn trek_programs() -> Vec<(u32, String)> {
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut next = vec![std::process::id()];
    while let Some(pid) = next.pop() {
        for child in trek_core::procs::children(pid) {
            if child == std::process::id() || found.iter().any(|(p, _)| *p == child) {
                continue;
            }
            if let Some(program) = running_program(child) {
                found.push((child, program));
                next.push(child);
            }
        }
    }
    found
}

/// `program` (a file name, as the system gives it) is `name`, `.exe` and case aside.
fn is_program(program: &str, name: &str) -> bool {
    let program = program.to_lowercase();
    let program = program.strip_suffix(".exe").unwrap_or(&program);
    program == name.to_lowercase().trim_end_matches(".exe")
}

/// The program process `pid` runs (its file name), while it runs.
fn running_program(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        // SAFETY: plain calls; the handle is checked, used only here and closed.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut code = 0u32;
            let running =
                GetExitCodeProcess(process, &mut code) != 0 && code == STILL_ACTIVE as u32;
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            let named = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                path.as_mut_ptr(),
                &mut len,
            ) != 0;
            CloseHandle(process);
            if !(running && named) {
                return None;
            }
            let path = String::from_utf16_lossy(&path[..len as usize]);
            Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
        }
    }
    #[cfg(not(windows))]
    {
        let out = trek_core::procs::command("/bin/ps")
            .args(["-o", "stat=,comm=", "-p", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (stat, program) = line.split_once(char::is_whitespace)?;
        // A zombie has ended; only its parent hasn't asked yet.
        if stat.starts_with('Z') {
            return None;
        }
        let program = program.trim();
        Some(program.rsplit('/').next().unwrap_or(program).to_string())
    }
}

/// The visible top-level windows that are consoles (the classic console's, Windows Terminal's)
/// or belong to one of `under` (programs under Trek), each with what it is.
#[cfg(windows)]
fn console_windows(under: &[u32]) -> Vec<(isize, String)> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
    };
    struct Seen<'a> {
        under: &'a [u32],
        found: Vec<(isize, String)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, seen: LPARAM) -> i32 {
        // SAFETY: `seen` is the `Seen` passed to `EnumWindows` below, alive for its whole call.
        let seen = unsafe { &mut *(seen as *mut Seen) };
        // SAFETY: plain calls on a window handle the system just gave.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut class = [0u16; 256];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let console = matches!(
                class.as_str(),
                "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS"
            );
            if console || seen.under.contains(&pid) {
                seen.found
                    .push((hwnd as isize, format!("{class} of process {pid}")));
            }
        }
        1
    }
    let mut seen = Seen {
        under,
        found: Vec::new(),
    };
    // SAFETY: the callback reads `seen` only during this call.
    unsafe { EnumWindows(Some(each), &mut seen as *mut Seen as LPARAM) };
    seen.found
}

/// A frame capture running beside the command loop: `record` starts one, `record stop` asks it to
/// finish early and `record wait` joins it (the ffconcat list is written when it ends either way).
struct Recording {
    stop: Arc<AtomicBool>,
    task: Task<anyhow::Result<u32>>,
}

/// `wait <ms>`, or `wait <condition> [cap ms]` — `idle` until no thread has a turn running,
/// `permission`/`question`/`plan` until such a request is showing (for cards on film).
async fn wait(ws: &Entity<Workspace>, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let mut it = arg.split_whitespace();
    let Some(kind) = it.next() else {
        anyhow::bail!(
            "wait needs <ms>, idle|permission|question|plan [cap ms] or shown <id> [cap ms]"
        )
    };
    if let Ok(ms) = kind.parse::<u64>() {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
        return Ok(());
    }
    // `wait shown <element id>[*] [cap ms]`: until it's on screen in the main window, for what
    // comes in its own time (a list the system answers for).
    if kind == "shown" {
        let id = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("wait shown <element id>[*] [cap ms]"))?
            .to_string();
        let cap = match it.next() {
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("wait shown: bad cap {v:?}"))?,
            None => 30_000,
        };
        let main = cx
            .update(|cx| ws.read(cx).main_window)
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        let deadline = Instant::now() + Duration::from_millis(cap);
        loop {
            if main.update(cx, |_, window, cx| shown(window, cx, &id))? > 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("wait shown {id}: not on screen after {cap} ms");
            }
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
        }
    }
    if !matches!(kind, "idle" | "permission" | "question" | "plan") {
        anyhow::bail!(
            "wait: unknown condition {kind:?} (idle|permission|question|plan|shown or milliseconds)"
        );
    }
    let cap = match it.next() {
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("wait {kind}: bad cap {v:?}"))?,
        None => 60_000,
    };
    let deadline = Instant::now() + Duration::from_millis(cap);
    // A turn needs a beat to register as started after `send`; without it `wait idle` could pass
    // before the session had a chance to begin.
    cx.background_executor()
        .timer(Duration::from_millis(250))
        .await;
    loop {
        let hit = cx.update(|cx| {
            let ws = ws.read(cx);
            match kind {
                "idle" => ws.live.values().all(|l| l.turn_started.is_none()),
                "permission" => ws
                    .live
                    .values()
                    .any(|l| l.permissions.iter().any(|p| p.prompt.is_none())),
                "question" => ws.live.values().any(|l| {
                    l.permissions.iter().any(|p| {
                        p.prompt
                            .as_ref()
                            .is_some_and(|p| matches!(p, trek_agents::Prompt::Questions(_)))
                    })
                }),
                _ => ws.live.values().any(|l| {
                    l.permissions.iter().any(|p| {
                        p.prompt
                            .as_ref()
                            .is_some_and(|p| matches!(p, trek_agents::Prompt::Plan(_)))
                    })
                }),
            }
        });
        if hit {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("wait {kind}: still not met after {cap} ms");
        }
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
    }
}

/// Set once a manifest has put the pointer somewhere (`click`, `hover`, `scroll` …): from then on
/// it's where it was put.
static POINTER_SCRIPTED: AtomicBool = AtomicBool::new(false);

/// Until then, a frame is drawn with the pointer outside the window. The window isn't in front, but
/// a real mouse resting over it still hovers what's under it (p08 of the parity set caught a
/// Basecamp bar's tooltip that way), and a capture shouldn't depend on where the mouse is.
fn park_pointer(window: &mut Window, cx: &mut App) {
    if !POINTER_SCRIPTED.load(Ordering::Relaxed) {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(-100.), px(-100.)),
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
    }
}

/// A few frames for whatever the last command changed to be drawn, plus the git reads it set
/// going (the composer's branch chip): a process spawn costs a hundred milliseconds on Windows,
/// so they can land after those few frames.
async fn settle(ws: &Entity<Workspace>, cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut waited = false;
    while cx.update(|cx| ws.read(cx).git_inflight) > 0 && Instant::now() < deadline {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        waited = true;
    }
    if waited {
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
    }
}

async fn shot(
    ws: &Entity<Workspace>,
    dir: &Path,
    arg: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    if arg.is_empty() {
        anyhow::bail!("shot needs a name");
    }
    settle(ws, cx).await;
    let path = dir.join(format!("{arg}.png"));
    let image = cx.update(|cx| -> anyhow::Result<image::RgbaImage> {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |_, window, cx| {
            park_pointer(window, cx);
            window.render_to_image()
        })?
    })?;
    let path2 = path.clone();
    cx.background_executor()
        .spawn(async move {
            if let Some(parent) = path2.parent() {
                std::fs::create_dir_all(parent)?;
            }
            save(image, &path2)
        })
        .await?;
    tracing::info!("shot: {}", path.display());
    Ok(())
}

/// `hittest <element id>|<x> <y> [HTCLIENT|HTCAPTION|HTCLOSE …]` (Windows): what the window answers
/// when Windows asks `WM_NCHITTEST` there — the question asked before every real mouse message,
/// whose answer decides whether a press clicks what's under it or starts a window drag. An element
/// is asked at its middle; a point is in logical px from the window's top left, a negative x from
/// its right edge. The answer goes to `hittest.txt`, and when one is expected a different answer
/// fails the command. `realclick <element id>|<x> <y>` then sends the window the move, press and
/// release a mouse delivers (to this window's own handle, never the desktop) — but only where the
/// answer is `HTCLIENT`: elsewhere a real press is a window drag, and it says so.
#[cfg(windows)]
async fn hittest(
    ws: &Entity<Workspace>,
    dir: &Path,
    verb: &str,
    arg: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    use gpui_kit::test::TestWindowExt as _;
    use std::io::Write as _;
    let mut words: Vec<&str> = arg.split_whitespace().collect();
    // `HTCAPTION` expects the handle (and the press to go through to its move); `HTCAPTION:swallowed`
    // the handle with the window taking the press for something of its own (a dropdown's).
    let expected = words
        .last()
        .filter(|w| w.starts_with("HT"))
        .map(|w| w.to_string());
    if expected.is_some() {
        words.pop();
    }
    let (expect, swallowed) = match expected
        .as_deref()
        .map(|e| e.split_once(':').unwrap_or((e, "")))
    {
        Some((code, flag)) => (Some(code.to_string()), flag == "swallowed"),
        None => (None, false),
    };
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    if verb == "realleave" {
        let hwnd = main
            .update(cx, |_, window, _| crate::winsys::hwnd_of(window))?
            .ok_or_else(|| anyhow::anyhow!("realleave: no window handle"))?;
        // The real cursor isn't where a script can put it: the window is told where it is.
        crate::chrome::SCRIPTED_CURSOR_AWAY.store(true, Ordering::Relaxed);
        crate::winsys::mouse_leave(hwnd);
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("hittest.txt"))?,
            "realleave: Windows told the window the pointer left it"
        )?;
        cx.update(|cx| cx.refresh_windows());
        return Ok(());
    }
    // The window is only asked inside an update; the question itself is sent outside of one, as the
    // window's procedure updates it to answer.
    let (hwnd, at, x, y) = main.update(
        cx,
        |_, window, cx| -> anyhow::Result<(isize, Point<Pixels>, i32, i32)> {
            POINTER_SCRIPTED.store(true, Ordering::Relaxed);
            window.render_frame(cx);
            let at = match words.as_slice() {
                [x, y] if x.parse::<f32>().is_ok() && y.parse::<f32>().is_ok() => {
                    let (x, y): (f32, f32) = (x.parse()?, y.parse()?);
                    point(
                        px(if x < 0. {
                            f32::from(window.viewport_size().width) + x
                        } else {
                            x
                        }),
                        px(y),
                    )
                }
                [id] => {
                    // `card-*` is the one element whose id starts so (a row's id has its thread's in it).
                    let named = |s: &gpui_kit::test::ElementSnapshot| match (
                        id.strip_suffix('*'),
                        s.path().last(),
                    ) {
                        (Some(prefix), Some(last)) => id_text(last).starts_with(prefix),
                        (None, last) => last == Some(&element_id(id)),
                        _ => false,
                    };
                    let found: Vec<_> = gpui_kit::base::test_support::snapshots(window)
                        .into_iter()
                        .filter(|s| named(s) && s.visible())
                        .collect();
                    let [one] = found.as_slice() else {
                        anyhow::bail!("{verb}: {} elements on screen are {id}", found.len())
                    };
                    let b = one.bounds();
                    b.origin + point(b.size.width / 2., b.size.height / 2.)
                }
                _ => anyhow::bail!(
                    "{verb} <element id>|<x> <y>{}",
                    if verb == "hittest" {
                        " [HTCLIENT|HTCAPTION|…]"
                    } else {
                        ""
                    }
                ),
            };
            // The pointer is there first, as a real one is before it asks: what the window answers comes from what it last saw under the pointer.
            if verb != "realmove" {
                window.simulate_mouse_move(at, cx);
            }
            let hwnd = crate::winsys::hwnd_of(window)
                .ok_or_else(|| anyhow::anyhow!("{verb}: no window handle"))?;
            let scale = window.scale_factor();
            Ok((
                hwnd,
                at,
                (f32::from(at.x) * scale).round() as i32,
                (f32::from(at.y) * scale).round() as i32,
            ))
        },
    )??;
    if verb == "realmove" {
        // Only the move: the pointer is left there, in the window.
        crate::chrome::SCRIPTED_CURSOR_AWAY.store(false, Ordering::Relaxed);
        crate::winsys::client_move(hwnd, x, y);
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("hittest.txt"))?,
            "realmove {} @ {x},{y}",
            words.join(" ")
        )?;
        cx.update(|cx| cx.refresh_windows());
        return Ok(());
    }
    let hit = crate::winsys::hit_test(hwnd, x, y);
    let answer = crate::winsys::hit_name(hit);
    // On the window's handle a press goes to what Windows does with it (a move; a double click
    // maximises) only if nothing in the window swallows it first: Windows hands the press to the
    // window before its own handling, and a window that answers it takes it for its own.
    let through = if answer == "HTCAPTION" {
        let press = |click_count| MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        };
        let passed = main.update(cx, |_, window, cx| {
            let first = window
                .dispatch_event(PlatformInput::MouseDown(press(1)), cx)
                .propagate;
            let second = window
                .dispatch_event(PlatformInput::MouseDown(press(2)), cx)
                .propagate;
            // Let go off the window: where it was pressed, the release would be a click on whatever
            // is there (in the beta's title bar, a button).
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(-100.), px(-100.)),
                    modifiers: Modifiers::default(),
                    click_count: 2,
                }),
                cx,
            );
            first && second
        })?;
        Some(passed)
    } else {
        None
    };
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("hittest.txt"))?;
    writeln!(
        log,
        "{verb} {} @ {x},{y} -> {answer}{}",
        words.join(" "),
        through
            .map(|p| if p {
                " (a press goes through to the window's move and maximize)"
            } else {
                " (a press is swallowed in the window)"
            })
            .unwrap_or_default()
    )?;
    if let Some(expect) = expect.filter(|e| e != answer) {
        anyhow::bail!(
            "{verb} {}: the window answers {answer}, not {expect}",
            words.join(" ")
        );
    }
    anyhow::ensure!(
        swallowed || through != Some(false),
        "{verb} {}: the window's handle, but something in the window swallows a press there",
        words.join(" ")
    );
    anyhow::ensure!(
        !swallowed || through == Some(false),
        "{verb} {}: expected the press swallowed, and it goes through",
        words.join(" ")
    );
    if verb == "realclick" {
        anyhow::ensure!(
            answer == "HTCLIENT",
            "realclick {}: the window answers {answer}, so a real press there is a window drag, not a click",
            words.join(" ")
        );
        // A press on a window behind first asks whether to activate it and let the press through.
        let activation = crate::winsys::mouse_activate(hwnd, hit);
        writeln!(log, "  a press with the window behind: {activation}")?;
        anyhow::ensure!(
            activation == "MA_ACTIVATE",
            "realclick {}: a first click on the window behind answers {activation}, so it would be lost or the window left behind",
            words.join(" ")
        );
        crate::chrome::SCRIPTED_CURSOR_AWAY.store(false, Ordering::Relaxed);
        let [moved, pressed, released] = crate::winsys::client_click(hwnd, x, y);
        writeln!(
            log,
            "  the window took {} ms to take the move, {} ms the press, {} ms the release",
            moved.as_millis(),
            pressed.as_millis(),
            released.as_millis()
        )?;
        cx.update(|cx| cx.refresh_windows());
    }
    Ok(())
}

#[cfg(not(windows))]
async fn hittest(
    _: &Entity<Workspace>,
    _: &Path,
    verb: &str,
    _: &str,
    _: &mut AsyncApp,
) -> anyhow::Result<()> {
    anyhow::bail!("{verb}: Windows only (macOS has no WM_NCHITTEST)")
}

/// `editor snap <name>`: the editor's preview in front (a PDF, a video), its web view as WebView2
/// draws it on Windows, or cropped out of a capture of the window on a Mac (Screen Recording
/// needed), to `<name>.png` (a native view: `shot` doesn't show it).
async fn editor_snap(
    ws: &Entity<Workspace>,
    dir: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty(), "editor snap needs a name");
    #[cfg(windows)]
    {
        let main = cx
            .update(|cx| ws.read(cx).main_window)
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        let png = main.update(cx, |root, _, cx| {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let trek = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())?;
            let web = trek
                .read(cx)
                .ide
                .read(cx)
                .active_preview()?
                .read(cx)
                .web_view()?;
            Some(crate::panels::browser::capture_png(web.read(cx).raw()))
        })?;
        let png = png.ok_or_else(|| {
            anyhow::anyhow!("editor snap: no web view in front (open a PDF or a video)")
        })?;
        let bytes = png
            .recv()
            .await
            .map_err(|_| anyhow::anyhow!("editor snap: the view closed"))?
            .map_err(|e| anyhow::anyhow!("editor snap: {e}"))?;
        std::fs::write(dir.join(format!("{name}.png")), bytes)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let main = cx
            .update(|cx| ws.read(cx).main_window)
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        // A beat for the last command's layout to reach the native view.
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        let capture = main.update(cx, |root, window, cx| {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let trek = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())?;
            let web = trek
                .read(cx)
                .ide
                .read(cx)
                .active_preview()?
                .read(cx)
                .web_view()?;
            crate::panels::browser::view_capture(web.read(cx), window)
        })?;
        let capture = capture.ok_or_else(|| {
            anyhow::anyhow!("editor snap: no web view in front (open a PDF or a video)")
        })?;
        let dest = dir.join(format!("{name}.png"));
        cx.background_executor()
            .spawn(async move { capture(&dest) })
            .await
            .map_err(|e| anyhow::anyhow!("editor snap: {e}"))
    }
}

/// `artifacts`: the interactive artifacts in the main window, to `artifacts.json`: each card's
/// title, whether it has a web view and shows it live, its height, its frame and the transcript's
/// visible area (window px), plus the window's frame on screen (for `screencapture -R`: the views
/// are native, so `shot` doesn't show them).
async fn artifacts(ws: &Entity<Workspace>, dir: &Path, cx: &mut AsyncApp) -> anyhow::Result<()> {
    // A beat for the last command's layout to reach the native views.
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let state = main.update(cx, |_, window, cx| {
        let rect = |b: Bounds<Pixels>| serde_json::json!({ "x": f32::from(b.origin.x), "y": f32::from(b.origin.y), "w": f32::from(b.size.width), "h": f32::from(b.size.height) });
        let cards: Vec<serde_json::Value> = crate::artifact::placements(window, cx)
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "title": a.title,
                    "expanded": a.expanded,
                    "has_view": a.has_view,
                    "live": a.live,
                    "loaded": a.loaded,
                    "height": a.height,
                    "has_snapshot": a.has_snapshot,
                    "has_keys": a.has_keys,
                    "body": a.body.map(rect),
                    "visible": a.body.zip(a.mask).map(|(b, m)| rect(b.intersect(&m))),
                })
            })
            .collect();
        serde_json::json!({ "window": rect(window.bounds()), "scale": window.scale_factor(), "artifacts": cards })
    })?;
    std::fs::write(
        dir.join("artifacts.json"),
        serde_json::to_string_pretty(&state)?,
    )?;
    Ok(())
}

async fn browser(
    ws: &Entity<Workspace>,
    dir: &Path,
    arg: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let (cmd, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let panel = main.update(cx, |root, _, cx| {
        let view = root
            .downcast::<gpui_kit::component::Root>()
            .ok()
            .map(|r| r.read(cx).view().clone());
        view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())
            .and_then(|trek| trek.read(cx).right_panel.read(cx).browser())
    })?;
    let panel = panel
        .ok_or_else(|| anyhow::anyhow!("browser: the Browser tool isn't open (tools browser)"))?;
    match cmd {
        "state" => {
            // A beat for the last command's layout to reach the native view.
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let state = main.update(cx, |_, window, cx| panel.read(cx).shots_state(window, cx))?;
            std::fs::write(
                dir.join("browser.json"),
                serde_json::to_string_pretty(&state)?,
            )?;
            Ok(())
        }
        "snap" => {
            anyhow::ensure!(!rest.is_empty(), "browser snap needs a name");
            #[cfg(windows)]
            {
                let png = cx
                    .update(|cx| panel.read(cx).shots_capture(cx))
                    .ok_or_else(|| anyhow::anyhow!("browser snap: no page"))?;
                let bytes = png
                    .recv()
                    .await
                    .map_err(|_| anyhow::anyhow!("browser snap: the page closed"))?
                    .map_err(|e| anyhow::anyhow!("browser snap: {e}"))?;
                std::fs::write(dir.join(format!("{rest}.png")), bytes)?;
                Ok(())
            }
            #[cfg(target_os = "macos")]
            {
                let tiff = cx
                    .update(|cx| panel.read(cx).shots_capture(cx))
                    .ok_or_else(|| anyhow::anyhow!("browser snap: no page"))?;
                let bytes = tiff
                    .recv()
                    .await
                    .map_err(|_| anyhow::anyhow!("browser snap: the page closed"))?
                    .map_err(|e| anyhow::anyhow!("browser snap: {e}"))?;
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Tiff)?
                    .save_with_format(dir.join(format!("{rest}.png")), image::ImageFormat::Png)?;
                Ok(())
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            anyhow::bail!("browser snap: Windows and macOS only")
        }
        "eval" => {
            anyhow::ensure!(!rest.is_empty(), "browser eval needs a script");
            let answer = cx
                .update(|cx| panel.read(cx).shots_eval(rest, cx))
                .ok_or_else(|| anyhow::anyhow!("browser eval: no page"))?;
            let json = answer
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("browser eval: the page closed"))?;
            std::fs::write(dir.join("browser-eval.json"), json)?;
            Ok(())
        }
        // `browser crash`: end the page's own process, as a crash would (WebView2's renderer,
        // WebKit's web content process).
        #[cfg(any(windows, target_os = "macos"))]
        "crash" => {
            let ended = cx
                .update(|cx| panel.read(cx).shots_crash(cx))
                .map_err(|e| anyhow::anyhow!("browser crash: {e}"))?;
            anyhow::ensure!(ended > 0, "browser crash: no page process to end");
            Ok(())
        }
        // `browser expect <crashed|failed[:<part of its address>]|loaded|why:<word>>…`: the active
        // tab as the panel has it: it heard its page's process end (`crashed`); it shows Trek's
        // page saying why a load failed (`failed`, of an address with that in it; `why`, with that
        // word in its reason); or it shows the page itself, neither failed nor loading (`loaded`).
        "expect" => {
            let state = main.update(cx, |_, window, cx| panel.read(cx).shots_state(window, cx))?;
            let url = state["url"].as_str().unwrap_or_default().to_string();
            for want in rest.split_whitespace() {
                match want.split_once(':').unwrap_or((want, "")) {
                    ("crashed", _) => anyhow::ensure!(
                        state["crashed"] == true,
                        "browser expect: no crash heard of ({url})"
                    ),
                    ("failed", part) => anyhow::ensure!(
                        state["failed"]
                            .as_str()
                            .is_some_and(|failed| failed.contains(part)),
                        "browser expect: no failure of {part:?} shown ({url}: failed {})",
                        state["failed"]
                    ),
                    ("loaded", _) => anyhow::ensure!(
                        state["failed"].is_null() && state["loading"] == false,
                        "browser expect: not loaded ({url}: failed {}, loading {})",
                        state["failed"],
                        state["loading"]
                    ),
                    ("why", part) => anyhow::ensure!(
                        state["why"].as_str().is_some_and(|why| why.contains(part)),
                        "browser expect: Trek's page doesn't say {part:?} ({url}: why {})",
                        state["why"]
                    ),
                    _ => anyhow::bail!("browser expect: {want}? (crashed, failed, loaded, why)"),
                }
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        "press" => {
            anyhow::ensure!(!rest.is_empty(), "browser press needs keys (cmd-v …)");
            let window = cx
                .update(|cx| panel.read(cx).shots_key_window(cx))
                .ok_or_else(|| anyhow::anyhow!("browser press: no page"))?;
            // Outside any update: the key goes through AppKit and back into GPUI. A key pressed
            // with Trek in the background takes another path, so it comes to the front first.
            if crate::panels::browser::activate(&window) {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
            }
            for keys in rest.split_whitespace() {
                crate::panels::browser::press(&window, keys)?;
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
            Ok(())
        }
        // The Browser's extensions, without the page's file picker or its Install prompt.
        "ext-install" | "ext-list" | "ext-on" | "ext-off" | "ext-remove" => {
            use crate::panels::browser_extensions as ext;
            let view = cx.update(|cx| panel.read(cx).shots_view());
            let fail = |e: String| anyhow::anyhow!("browser {cmd}: {e}");
            match cmd {
                "ext-install" => {
                    anyhow::ensure!(
                        !rest.is_empty(),
                        "browser ext-install needs a folder, a .zip/.crx or a store id"
                    );
                    let name = ext::install_for_driver(rest.to_string(), view, cx)
                        .await
                        .map_err(fail)?;
                    tracing::info!("browser ext-install: {name}");
                }
                "ext-list" => {
                    let list = ext::list_for_driver(view, cx).await.map_err(fail)?;
                    std::fs::write(
                        dir.join("browser-extensions.json"),
                        serde_json::to_string_pretty(&list)?,
                    )?;
                }
                _ => {
                    anyhow::ensure!(
                        !rest.is_empty(),
                        "browser {cmd} needs an extension's id or name"
                    );
                    let enable = match cmd {
                        "ext-on" => Some(true),
                        "ext-off" => Some(false),
                        _ => None,
                    };
                    ext::change_for_driver(rest.to_string(), enable, view, cx)
                        .await
                        .map_err(fail)?;
                }
            }
            cx.update(|cx| cx.refresh_windows());
            Ok(())
        }
        "ext-page" => {
            main.update(cx, |_, window, cx| {
                panel.update(cx, |p, cx| p.open_extensions(window, cx))
            })?;
            Ok(())
        }
        "ext-allow" | "ext-deny" => {
            let name = crate::panels::browser_extensions::answer_prompt(cmd == "ext-allow")
                .map_err(|e| anyhow::anyhow!("browser {cmd}: {e}"))?;
            tracing::info!("browser {cmd}: {name}");
            cx.update(|cx| cx.refresh_windows());
            Ok(())
        }
        #[cfg(target_os = "macos")]
        "ext-popup" => {
            anyhow::ensure!(!rest.is_empty(), "browser ext-popup needs a script");
            let answer = cx.update(|_| crate::panels::browser_extensions::webkit::popup_eval(rest));
            let text = answer
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("browser ext-popup: the popup closed"))?
                .map_err(|e| anyhow::anyhow!("browser ext-popup: {e}"))?;
            std::fs::write(dir.join("browser-popup.txt"), text)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        "ext-options" => {
            crate::panels::browser_extensions::options_for_driver(rest)
                .map_err(|e| anyhow::anyhow!("browser ext-options: {e}"))?;
            Ok(())
        }
        _ => {
            // The page takes the keys as a click does, in the front window: AppKit hands them to
            // its own view again when the window comes to the front.
            #[cfg(target_os = "macos")]
            if cmd == "focus"
                && let Some(window) = cx.update(|cx| panel.read(cx).shots_key_window(cx))
                && crate::panels::browser::activate(&window)
            {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
            }
            cx.update(|cx| panel.update(cx, |p, cx| p.shots(cmd, rest, cx)))?;
            cx.update(|cx| cx.refresh_windows());
            Ok(())
        }
    }
}

async fn record(
    ws: &Entity<Workspace>,
    dir: &Path,
    arg: &str,
    slot: &mut Option<Recording>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let mut it = arg.split_whitespace();
    match it.next() {
        Some("wait") => match slot.take() {
            Some(rec) => {
                let frames = rec.task.await?;
                tracing::info!("record: {frames} frames");
                Ok(())
            }
            None => anyhow::bail!("record wait: nothing is recording"),
        },
        Some("stop") => match slot.take() {
            Some(rec) => {
                rec.stop.store(true, Ordering::Relaxed);
                let frames = rec.task.await?;
                tracing::info!("record: {frames} frames (stopped early)");
                Ok(())
            }
            None => anyhow::bail!("record stop: nothing is recording"),
        },
        Some(name) => {
            if slot.is_some() {
                anyhow::bail!(
                    "record {name}: already recording (record wait or record stop first)"
                );
            }
            let ms = it
                .next()
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| anyhow::anyhow!("record {name}: bad duration"))?
                .ok_or_else(|| anyhow::anyhow!("record {name}: needs <ms>"))?;
            let fps = match it.next() {
                Some(v) => v
                    .parse::<u32>()
                    .map_err(|_| anyhow::anyhow!("record {name}: bad fps {v:?}"))?,
                None => 15,
            };
            anyhow::ensure!(
                fps > 0 && fps <= 60,
                "record {name}: fps {fps} out of range 1–60"
            );
            *slot = Some(start_recording(
                ws.clone(),
                dir.to_path_buf(),
                name.to_string(),
                ms,
                fps,
                cx,
            ));
            Ok(())
        }
        None => anyhow::bail!("record needs <name> <ms> [fps] | stop | wait"),
    }
}

/// Render a frame every `1000/fps` ms for `ms` ms, writing `f%05d.png` files under
/// `<name>.frames/`. The real capture instant goes into `<name>.ffconcat` as each frame's
/// duration, so a frame that took long still plays for as long as it was on screen — dropped
/// frames don't speed the clip up. Runs beside the command loop: the `send`/`route`/`wait` lines
/// after `record` are what gets filmed.
fn start_recording(
    ws: Entity<Workspace>,
    dir: PathBuf,
    name: String,
    ms: u64,
    fps: u32,
    cx: &mut AsyncApp,
) -> Recording {
    let frames_dir = dir.join(format!("{name}.frames"));
    let concat_path = dir.join(format!("{name}.ffconcat"));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let task = cx.spawn(async move |cx| {
        std::fs::create_dir_all(&frames_dir)?;
        let started = Instant::now();
        let interval = Duration::from_secs_f64(1. / fps as f64);
        let mut times = Vec::new();
        let mut index: u32 = 0;
        while started.elapsed() < Duration::from_millis(ms) && !flag.load(Ordering::Relaxed) {
            let image = cx.update(|cx| -> anyhow::Result<image::RgbaImage> {
                let main = ws
                    .read(cx)
                    .main_window
                    .ok_or_else(|| anyhow::anyhow!("no main window"))?;
                main.update(cx, |_, window, cx| {
                    park_pointer(window, cx);
                    window.render_to_image()
                })?
            })?;
            times.push(started.elapsed().as_secs_f64());
            let path = frames_dir.join(format!("f{index:05}.png"));
            index += 1;
            cx.background_executor()
                .spawn(async move { save(image, &path) })
                .await?;
            let spent = started.elapsed().as_secs_f64() - times.last().copied().unwrap_or(0.);
            let left = interval.as_secs_f64() - spent;
            if left > 0. {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(left))
                    .await;
            }
        }
        if let Some(parent) = concat_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut concat = String::from("ffconcat version 1.0\n");
        for (i, &t) in times.iter().enumerate() {
            let till = times
                .get(i + 1)
                .copied()
                .unwrap_or(t + interval.as_secs_f64());
            concat.push_str(&format!(
                "file '{name}.frames/f{i:05}.png'\nduration {:.4}\n",
                till - t
            ));
        }
        std::fs::write(&concat_path, concat)?;
        Ok(times.len() as u32)
    });
    Recording { stop, task }
}

/// What `perf type` types, over and over: words and spaces, as prose is typed.
const TYPED: &str =
    "the quick brown fox jumps over the lazy dog while the agent reads the build log ";

/// `perf frames|scroll|type|mark …` (see the header): input dispatched as events, never real OS
/// input, and each step waits for the frame that shows it.
async fn perf(ws: &Entity<Workspace>, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    use crate::perf::{BUDGET, ms, summarize};
    anyhow::ensure!(crate::perf::on(), "perf: launch with TREK_PERF=1");
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let args: Vec<&str> = arg.split_whitespace().collect();
    let num = |i: usize, what: &str| -> anyhow::Result<u64> {
        args.get(i)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("perf: {arg:?}: bad or missing {what}"))
    };
    let frame_pairs = |frames: &[crate::perf::Frame]| {
        let took: Vec<Duration> = frames.iter().map(crate::perf::Frame::took).collect();
        let s = summarize(&took, BUDGET);
        format!("frames={} {}", s.n, s.pairs("frame_"))
    };
    match args.first().copied() {
        Some("mark") => tracing::info!("perf: mark label={}", args.get(1).unwrap_or(&"-")),
        // What Trek itself holds: transcripts in memory, and the main window's parsed answers.
        Some("mem") => {
            let label = args.get(1).unwrap_or(&"mem");
            let held = main.update(cx, |root, _, cx| {
                let view = root
                    .downcast::<gpui_kit::component::Root>()
                    .ok()
                    .map(|r| r.read(cx).view().clone());
                let md = view
                    .and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())
                    .map(|trek| trek.read(cx).thread_view.read(cx).held(cx))
                    .unwrap_or_default();
                let ws = ws.read(cx);
                let loaded = ws.live.values().filter(|l| l.loaded);
                let (threads, items, bytes) = loaded.fold((0, 0, 0), |(t, n, b), l| {
                    let text: usize = l.items.iter().map(item_bytes).sum();
                    (t + 1, n + l.items.len(), b + text)
                });
                // Shaped text kept for the next frame, and how much of it is of text that's gone.
                let (layouts, alive) = gpui_kit::base::text::retained_layouts();
                format!(
                    "live_threads={threads} live_items={items} live_text_kb={} {md} layouts={layouts} layouts_dead={} {} {}",
                    bytes / 1024,
                    layouts - alive,
                    crate::pictures::held(cx),
                    crate::artifact::held(cx)
                )
            })?;
            tracing::info!("perf: mem label={label} {held}");
        }
        Some("frames") => {
            let span = num(1, "<ms>")?;
            let label = args.get(2).unwrap_or(&"frames");
            crate::perf::record();
            // Beside the frames, how late the main thread comes back to a 5 ms timer: work done
            // outside a frame (a save, a parse) shows there and not in frame times.
            let stop = Arc::new(AtomicBool::new(false));
            let sampler = {
                let stop = stop.clone();
                cx.spawn(async move |cx| {
                    let mut lags = vec![];
                    while !stop.load(Ordering::Relaxed) {
                        let asked = Instant::now();
                        cx.background_executor()
                            .timer(Duration::from_millis(5))
                            .await;
                        lags.push(asked.elapsed().saturating_sub(Duration::from_millis(5)));
                    }
                    lags
                })
            };
            cx.background_executor()
                .timer(Duration::from_millis(span))
                .await;
            stop.store(true, Ordering::Relaxed);
            let lags = sampler.await;
            let frames = crate::perf::take_recorded();
            tracing::info!(
                "perf: frames label={label} ms={span} {} {}",
                frame_pairs(&frames),
                summarize(&lags, BUDGET).pairs("lag_")
            );
        }
        Some("scroll") => {
            let id = *args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("perf scroll <element id> <px> <steps> <label>"))?;
            let dy: f32 = args
                .get(2)
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("perf scroll: bad or missing <px>"))?;
            let steps = num(3, "<steps>")?;
            let label = args.get(4).unwrap_or(&"scroll");
            // The pointer goes over the element once, as a hand on a wheel stays put: over the
            // middle of what's on screen of it (a tall row's own middle can be off the window).
            let position =
                main.update(cx, |_, window, cx| -> anyhow::Result<Point<Pixels>> {
                    // `@center`: the middle of the window, which is over the transcript whatever
                    // rows it shows (a long answer's later parts have no id of their own to aim at).
                    if id == "@center" {
                        POINTER_SCRIPTED.store(true, Ordering::Relaxed);
                        let size = window.viewport_size();
                        let position = point(size.width / 2., size.height / 2.);
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Default::default(),
                            }),
                            cx,
                        );
                        return Ok(position);
                    }
                    pointer(window, "hover", id, cx)?;
                    let id = element_id(id);
                    let found = gpui_kit::base::test_support::snapshots(window)
                        .into_iter()
                        .find(|s| s.path().last() == Some(&id));
                    let bounds = found
                        .ok_or_else(|| anyhow::anyhow!("perf scroll: {id:?} went away"))?
                        .bounds();
                    Ok(bounds
                        .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()))
                        .center())
                })??;
            crate::perf::record();
            let (mut latency, mut unpainted) = (vec![], 0);
            for _ in 0..steps {
                let began = Instant::now();
                let (tx, rx) = crate::perf::next_frame();
                main.update(cx, |_, window, cx| {
                    let event = ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
                        ..Default::default()
                    };
                    window.dispatch_event(event.to_platform_input(), cx);
                })?;
                match painted_within(tx, rx, Duration::from_millis(500), cx).await {
                    Some(f) => latency.push(f.painted.saturating_duration_since(began)),
                    // Nothing to draw (the end of the list), or no frame in time.
                    None => unpainted += 1,
                }
                let spent = began.elapsed();
                if spent < BUDGET {
                    cx.background_executor().timer(BUDGET - spent).await;
                }
            }
            let frames = crate::perf::take_recorded();
            let l = summarize(&latency, BUDGET);
            tracing::info!(
                "perf: scroll label={label} steps={steps} unpainted={unpainted} {} {}",
                frame_pairs(&frames),
                l.pairs("latency_")
            );
        }
        Some("type") => {
            let count = num(1, "<count>")? as usize;
            let label = args.get(2).unwrap_or(&"type");
            crate::perf::record();
            let (mut latency, mut unpainted) = (vec![], 0);
            for c in TYPED.chars().cycle().take(count) {
                let began = Instant::now();
                let (tx, rx) = crate::perf::next_frame();
                let typed = main.update(cx, |_, window, cx| {
                    window.focused(cx).is_some()
                        && window.dispatch_keystroke(
                            Keystroke {
                                modifiers: Modifiers::default(),
                                key: String::new(),
                                key_char: Some(c.to_string()),
                            },
                            cx,
                        )
                })?;
                anyhow::ensure!(
                    typed,
                    "perf type: nothing has the keyboard (focus a field first)"
                );
                match painted_within(tx, rx, Duration::from_millis(500), cx).await {
                    Some(f) => latency.push(f.painted.saturating_duration_since(began)),
                    None => unpainted += 1,
                }
                // About 20 keys a second, a fast typist's pace.
                let spent = began.elapsed();
                if spent < Duration::from_millis(50) {
                    cx.background_executor()
                        .timer(Duration::from_millis(50) - spent)
                        .await;
                }
            }
            let frames = crate::perf::take_recorded();
            let l = summarize(&latency, BUDGET);
            tracing::info!(
                "perf: type label={label} keys={count} unpainted={unpainted} {} {} latency_mean={}",
                frame_pairs(&frames),
                l.pairs("latency_"),
                ms(latency.iter().sum::<Duration>() / latency.len().max(1) as u32)
            );
        }
        // The thread with this in its title opens, as a click on it does: how long until its
        // history is in memory, and until the frame showing it is painted.
        Some("open") => {
            let wanted = args[1..].join(" ").to_lowercase();
            let began = Instant::now();
            let id = cx.update(|cx| {
                ws.update(cx, |ws, cx| {
                    let id = ws
                        .threads
                        .iter()
                        .find(|t| t.title.to_lowercase().contains(&wanted))
                        .map(|t| t.id.clone())?;
                    ws.navigate(Route::Thread(id.clone()), cx);
                    Some(id)
                })
            });
            let id = id.ok_or_else(|| anyhow::anyhow!("perf open: no title has {wanted:?}"))?;
            let items = loop {
                let loaded = cx.update(|cx| {
                    let ws = ws.read(cx);
                    let live = ws.live.get(&id).filter(|l| l.loaded);
                    live.map(|l| l.items.len())
                });
                if let Some(items) = loaded {
                    break items;
                }
                anyhow::ensure!(
                    began.elapsed() < Duration::from_secs(120),
                    "perf open: its history didn't load"
                );
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            };
            let loaded = began.elapsed();
            let (tx, rx) = crate::perf::next_frame();
            let frame = painted_within(tx, rx, Duration::from_secs(5), cx).await;
            tracing::info!(
                "perf: open items={items} loaded={} painted={}",
                ms(loaded),
                ms(frame.map_or(began.elapsed(), |f| f
                    .painted
                    .saturating_duration_since(began)))
            );
        }
        // Above other apps' windows, without the keyboard: macOS takes a moment to say whether
        // it's visible, and the display link runs only if it is.
        Some("front") => {
            main.update(cx, |_, window, _| {
                crate::system::order_front_quietly(window)
            })?;
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let visible = main.update(cx, |_, window, _| {
                crate::system::order_front_quietly(window)
            })?;
            tracing::info!("perf: front visible={visible}");
        }
        _ => anyhow::bail!(
            "perf frames <ms> <label> | scroll <element id>|@center <px> <steps> <label> | type <count> <label> | open <title> | front | mark <label> | mem <label>"
        ),
    }
    Ok(())
}

/// The text a transcript item keeps in memory, in bytes (`perf mem`).
fn item_bytes(item: &trek_core::store::Item) -> usize {
    use trek_core::store::Item;
    match item {
        Item::User { text, images, .. } => {
            text.len() + images.iter().map(String::len).sum::<usize>()
        }
        Item::Assistant { text }
        | Item::Reasoning { text }
        | Item::Notice { text }
        | Item::Error { text }
        | Item::Limit { text, .. } => text.len(),
        Item::Tool {
            id,
            title,
            detail,
            output,
            ..
        } => id.len() + title.len() + detail.len() + output.len(),
        Item::TurnEnd { .. } | Item::Handoff { .. } => 0,
    }
}

/// The frame `perf::next_frame` was armed for, or `None` after `limit`.
async fn painted_within(
    tx: async_channel::Sender<Option<crate::perf::Frame>>,
    rx: async_channel::Receiver<Option<crate::perf::Frame>>,
    limit: Duration,
    cx: &mut AsyncApp,
) -> Option<crate::perf::Frame> {
    let executor = cx.background_executor().clone();
    let timeout = cx.background_executor().spawn(async move {
        executor.timer(limit).await;
        let _ = tx.try_send(None);
    });
    let frame = rx.recv().await.ok().flatten();
    drop(timeout);
    frame
}

fn quit(cx: &mut AsyncApp) -> anyhow::Result<()> {
    // As the Quit menu item does.
    cx.update(crate::root::quit);
    Ok(())
}

/// `windows`: every window of the app as the window server has it, to `windows.json`: its
/// number (what `screencapture -l` takes), the display it's on, its frame on the desktop and
/// whether macOS is drawing it. The main window is marked.
#[cfg(target_os = "macos")]
fn windows(ws: &Entity<Workspace>, dir: &Path, cx: &mut App) -> anyhow::Result<()> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let main = ws.read(cx).main_window.and_then(|main| {
        main.update(cx, |_, window, _| {
            let handle = HasWindowHandle::window_handle(&*window).ok()?;
            let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
                return None;
            };
            // SAFETY: `ns_view` is the live NSView GPUI created for this window; `window` is a
            // plain AppKit getter, on the main thread.
            let ns_window: *mut AnyObject =
                unsafe { msg_send![appkit.ns_view.as_ptr().cast::<AnyObject>(), window] };
            (!ns_window.is_null()).then_some(ns_window.cast_const().cast())
        })
        .ok()
        .flatten()
    });
    let list = crate::test_display::windows(main);
    std::fs::write(dir.join("windows.json"), format!("{list:#}\n"))?;
    Ok(())
}

fn run(ws: &Entity<Workspace>, verb: &str, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    // The picture-in-picture window of background computer use, as its buttons and the menu
    // bar icon drive it.
    if verb == "background-use" {
        let b = ws.read(cx).background_use.clone();
        if arg == "stop" {
            crate::background_use::BackgroundUse::stop_current(&b, cx);
            return Ok(());
        }
        return b.update(cx, |b, cx| {
            match arg {
                "fold" => b.set_folded(true, cx),
                "unfold" => b.set_folded(false, cx),
                "hide" => b.tuck(cx),
                "show" => b.show(cx),
                "toggle" => b.toggle(cx),
                "next" => b.next(cx),
                _ => anyhow::bail!("background-use fold|unfold|hide|show|toggle|next|stop"),
            }
            Ok(())
        });
    }
    if verb == "accounts-demo" {
        usage_demo(ws, cx);
        accounts_demo(ws, cx);
        return Ok(());
    }
    if verb == "usage-demo" {
        usage_demo(ws, cx);
        return Ok(());
    }
    if verb == "limits-demo" {
        limits_demo(ws, cx);
        return Ok(());
    }
    if verb == "compact-demo" {
        compact_demo(ws, cx);
        return Ok(());
    }
    if verb == "update-demo" {
        return update_demo(ws, arg, cx);
    }
    if verb == "resume-demo" {
        return resume_demo(ws, arg, cx);
    }
    if verb == "secret-demo" {
        return secret_demo(ws, cx);
    }
    if verb == "privacy-demo" {
        return privacy_demo(ws, arg, cx);
    }
    if verb == "pr-demo" {
        pr_demo(ws, arg, cx);
        return Ok(());
    }
    if verb == "import-demo" {
        return import_demo(ws, cx);
    }
    if verb == "checks-demo" {
        return checks_demo(ws, arg, cx);
    }
    // `update-task registered|gone` (Windows): the scheduled task that updates Trek while it's
    // closed, as Task Scheduler has it (the capture build's own name: never the installed Trek's).
    if verb == "update-task" {
        anyhow::ensure!(cfg!(windows), "update-task: Windows' alone");
        #[cfg(windows)]
        {
            let there =
                trek_core::update::closed::task::registered(crate::workspace::update_task_name());
            match arg.trim() {
                "registered" => anyhow::ensure!(there, "update-task: no task in Task Scheduler"),
                "gone" => anyhow::ensure!(!there, "update-task: the task is still there"),
                other => anyhow::bail!("update-task {other}? (registered|gone)"),
            }
        }
        return Ok(());
    }
    // `opencode-history <n>`: OpenCode 1.x's and 2's sessions read as one history, as Trek reads
    // OpenCode's folder: a database both wrote (sessions in 1.x's tables, 2's, and both:
    // trek-core's fixture), in a folder of the check's own, holds `n` threads, each once.
    if verb == "opencode-history" {
        let n: usize = arg.trim().parse()?;
        let dir = trek_core::paths::data_dir().join("opencode-demo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let db = rusqlite::Connection::open(dir.join("opencode.db"))?;
        db.execute_batch("PRAGMA foreign_keys = OFF;")?;
        db.execute_batch(include_str!(
            "../../trek-core/fixtures/opencode-v1-and-v2.sql"
        ))?;
        drop(db);
        let threads =
            trek_core::import::opencode::scan_dir(&dir, 0, &std::collections::HashSet::new());
        let mut ids: Vec<&str> = threads.iter().map(|t| t.native_id.as_str()).collect();
        ids.sort_unstable();
        let all = ids.len();
        ids.dedup();
        anyhow::ensure!(
            all == n && ids.len() == all,
            "opencode-history: {all} threads ({ids:?}), not {n} once each"
        );
        return Ok(());
    }
    // `toasts <n>`: how many toasts are up in the main window (ones on their way out included);
    // `toasts clear`: all of them taken down, so none sits over the next check.
    if verb == "toasts" {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        match arg.trim() {
            "clear" => main.update(cx, |_, window, cx| {
                for key in crate::toast::keys(window, cx) {
                    crate::toast::dismiss(window, &key, cx);
                }
            })?,
            n => {
                let n: usize = n.parse().map_err(|_| anyhow::anyhow!("toasts <n>|clear"))?;
                let up = main.update(cx, |_, window, cx| crate::toast::count(window, cx))?;
                anyhow::ensure!(up == n, "toasts: {up} up, not {n}");
            }
        }
        return Ok(());
    }
    if verb == "toast" {
        anyhow::ensure!(!arg.is_empty(), "toast [undo|error|guarded] <message>");
        let (kind, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        match kind {
            // The offer Trek makes at launch from a folder Windows' ransomware protection guards
            // (`relocate::guarded_offer`), as from `rest`: its words, and Move Trek. Where the
            // running Trek is can't change mid-run; finding the guard is update.rs's tests'.
            "guarded" => {
                use trek_core::update::{Blocker, KnownFolder};
                let folder = match rest.trim() {
                    "Documents" => KnownFolder::Documents,
                    "Desktop" => KnownFolder::Desktop,
                    other => anyhow::bail!("toast guarded {other}? (Documents|Desktop)"),
                };
                let (message, undo) = crate::workspace::relocate::guarded_offer(
                    Some(Blocker::GuardedFolder(folder)),
                    true,
                )
                .ok_or_else(|| anyhow::anyhow!("toast guarded: no offer"))?;
                ws.update(cx, |_, cx| {
                    cx.emit(crate::workspace::WorkspaceEvent::Toast {
                        message,
                        undo: Some(undo),
                    })
                });
            }
            "undo" => ws.update(cx, |_, cx| {
                cx.emit(crate::workspace::WorkspaceEvent::Toast {
                    message: rest.into(),
                    undo: Some(crate::workspace::UndoAction::Unsettle(String::new())),
                })
            }),
            "error" => {
                let main = ws
                    .read(cx)
                    .main_window
                    .ok_or_else(|| anyhow::anyhow!("no main window"))?;
                main.update(cx, |_, window, cx| {
                    crate::toast::push(window, crate::toast::Toast::error(rest.to_string()), cx)
                })?;
            }
            _ => ws.update(cx, |_, cx| {
                cx.emit(crate::workspace::WorkspaceEvent::Toast {
                    message: arg.into(),
                    undo: None,
                })
            }),
        }
        return Ok(());
    }
    if verb == "range" {
        use trek_core::basecamp::Range;
        let range = match arg {
            "today" => Range::Today,
            "week" => Range::Week,
            "all" => Range::All,
            _ => anyhow::bail!("range: unknown range {arg:?} (today|week|all)"),
        };
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, _, cx| {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            if let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) {
                let basecamp = trek.read(cx).basecamp.clone();
                basecamp.update(cx, |b, cx| b.set_range(range, cx));
            }
        })?;
        cx.refresh_windows();
        return Ok(());
    }
    // A stand-in iPhone pairs with this Trek as the iPhone app does: Trek's own client, pinned to
    // the phone server's certificate, says `pair` with the code Settings shows and `ios` as its
    // platform, and stays connected. An isolated run starts no phone server, so one starts on the
    // loopback alone (`Workspace::start_loopback_remote`). `pair-phone [name]`; `pair-phone leave`
    // disconnects it (the sidebar then says when it was last seen).
    if verb == "pair-phone" {
        static STAND_IN: std::sync::Mutex<Option<Arc<trek_remote::client::Client>>> =
            std::sync::Mutex::new(None);
        let mut held = STAND_IN.lock().unwrap_or_else(|e| e.into_inner());
        if arg == "leave" {
            if let Some(phone) = held.take() {
                phone.close();
            }
            return Ok(());
        }
        // `pair-phone upload <name>`: the stand-in sends a small file up as the iPhone app does
        // (`upload_part`, the whole file in one piece), and this Trek keeps it in the data
        // folder's `uploads/` once its free space was checked (`file uploads/*<name>`). The
        // answer comes on this thread later, so the piece goes and isn't waited for here.
        if let Some(name) = arg.strip_prefix("upload ") {
            let phone = held
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("pair-phone upload: pair a stand-in first"))?
                .clone();
            let bytes = format!("Sent up by a stand-in iPhone: {name}\n").into_bytes();
            let part = trek_remote::UploadPart {
                upload_id: uuid::Uuid::new_v4().to_string(),
                name: name.trim().to_string(),
                total: bytes.len() as u64,
                offset: 0,
                data: {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                },
            };
            trek_core::runtime().spawn(async move {
                let answer = phone
                    .request(trek_remote::ClientMessage::UploadPart(part))
                    .await;
                tracing::info!("pair-phone upload: {answer:?}");
            });
            return Ok(());
        }
        let link = ws.update(cx, |ws, cx| -> anyhow::Result<crate::machines::PairLink> {
            ws.start_loopback_remote(cx)?;
            ws.offer_pairing(cx);
            let offer = ws
                .remote
                .as_ref()
                .and_then(|r| r.offer.clone())
                .ok_or_else(|| anyhow::anyhow!("pair-phone: no pairing code"))?;
            crate::machines::PairLink::parse(&offer.url).map_err(anyhow::Error::msg)
        })?;
        let _runtime = trek_core::runtime().enter();
        let phone = trek_remote::client::Client::connect_with(
            link.routes,
            &link.fingerprint,
            trek_remote::client::Auth::Pair {
                code: link.code,
                device_id: "stand-in-iphone".into(),
                device_name: if arg.is_empty() {
                    "Stand-in iPhone"
                } else {
                    arg
                }
                .into(),
            },
            trek_remote::client::ClientOptions {
                platform: Some("ios".into()),
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("pair-phone: {e:?}"))?;
        // What the machine sends it (snapshots, rows) is read and let go.
        if let Some(mut events) = phone.events() {
            trek_core::runtime().spawn(async move { while events.recv().await.is_some() {} });
        }
        *held = Some(Arc::new(phone));
        return Ok(());
    }
    // The iPhone tool with a made-up phone (an isolated run never touches the real one): opens the
    // tool, then `phone::PhonePanel::demo` puts it in `arg`'s situation (a PC's has its own).
    #[cfg(any(target_os = "macos", windows))]
    if verb == "phone" {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("phone: no Trek window");
            };
            let panel = trek.read(cx).right_panel.clone();
            // `call <method> <json>`: an agent's tool call, its answer written to
            // `$TREK_SHOT_DIR/phone-reply.json` (with `real`, on the user's real phone).
            if let Some(rest) = arg.strip_prefix("call ") {
                let (method, params) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let params: serde_json::Value = serde_json::from_str(params)?;
                let (call, rx) = crate::workspace::phone_use::PhoneCall::probe(method, params);
                let out = std::env::var_os("TREK_SHOT_DIR")
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("phone call: no TREK_SHOT_DIR"))?
                    .join("phone-reply.json");
                let _ = std::fs::remove_file(&out);
                std::thread::spawn(move || {
                    let reply = match rx.recv_blocking() {
                        Ok(crate::ipc::Reply::Done(Ok(v))) => serde_json::json!({ "ok": v }),
                        Ok(crate::ipc::Reply::Done(Err(e))) => serde_json::json!({ "error": e }),
                        _ => serde_json::json!({ "error": "no answer" }),
                    };
                    let _ = std::fs::write(&out, reply.to_string());
                });
                panel.update(cx, |p, cx| p.phone_call(call, window, cx));
                return Ok(());
            }
            panel.update(cx, |p, cx| p.open_tool(PanelTool::Phone, window, cx));
            let phone = panel
                .read(cx)
                .phone()
                .ok_or_else(|| anyhow::anyhow!("phone: the iPhone tool didn't open"))?;
            phone.update(cx, |p, cx| p.demo(arg, window, cx))
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "tools" {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("tools: no Trek window");
            };
            let panel = trek.read(cx).right_panel.clone();
            match arg {
                "on" => panel.update(cx, |p, cx| {
                    if !p.open {
                        p.toggle(cx);
                    }
                }),
                "off" => panel.update(cx, |p, cx| {
                    if p.open {
                        p.toggle(cx);
                    }
                }),
                // The terminal is the dock under the chat, not a tab of the panel.
                "terminal" => trek.read(cx).terminal_dock.clone().update(cx, |d, cx| d.show(window, cx)),
                "git" | "explorer" | "browser" | "sidechat" | "simulator" | "iphone" => {
                    let tool = match arg {
                        "git" => PanelTool::Git,
                        "explorer" => PanelTool::Explorer,
                        "browser" => PanelTool::Browser,
                        "sidechat" => PanelTool::SideChat,
                        "iphone" => PanelTool::Phone,
                        _ => PanelTool::Simulator,
                    };
                    panel.update(cx, |p, cx| p.open_tool(tool, window, cx));
                }
                _ => anyhow::bail!("tools: unknown argument {arg:?} (on|off|git|explorer|terminal|browser|sidechat|simulator|iphone)"),
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "terminal" {
        // The terminal on screen without a shell (an isolated Trek starts none): `terminal feed
        // <text>` writes text into its screen as if the shell printed it (`\n` a new line, `\e`
        // an escape, `\a` the bell, `\\` a backslash), and `terminal link <n>` opens the n-th
        // link on it (from 1, top to bottom) as ⌘-clicking it does.
        let (sub, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        let in_editor = ws.read(cx).ide();
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("terminal: no Trek window");
            };
            let term = front_terminal(&trek, in_editor, window, cx).ok_or_else(|| {
                anyhow::anyhow!("terminal: none on screen (`tools terminal` shows the dock)")
            })?;
            match sub {
                "feed" => term.update(cx, |t, cx| t.print(unescape(rest).as_bytes(), cx)),
                "link" => {
                    let n: usize = rest
                        .trim()
                        .parse()
                        .map_err(|_| anyhow::anyhow!("terminal link <n>: n counts from 1"))?;
                    term.update(cx, |t, cx| t.open_nth_link(n, window, cx))?;
                }
                _ => anyhow::bail!("terminal feed <text>|link <n>"),
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "note" {
        // Notes without a pointer: `note text <markdown>` (the note on screen's whole text, `\n`
        // for a new line; one is started when there's none), `note image <path>…` / `note attach <path>…` (files
        // put in at the cursor, as dropping them does) and `note mode write|split|preview`.
        let (sub, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("note: no Trek window");
            };
            let notes = trek.read(cx).notes.clone();
            match sub {
                "text" => notes.update(cx, |n, cx| {
                    n.set_text(rest.replace("\\n", "\n"), window, cx)
                }),
                "image" | "attach" => {
                    anyhow::ensure!(!rest.is_empty(), "note image|attach <path>…");
                    let paths: Vec<PathBuf> = rest.split_whitespace().map(PathBuf::from).collect();
                    notes.update(cx, |n, cx| n.add_files(paths, window, cx));
                }
                "mode" => {
                    let mode = match rest {
                        "write" => crate::notes::Mode::Write,
                        "split" => crate::notes::Mode::Split,
                        "preview" => crate::notes::Mode::Preview,
                        _ => anyhow::bail!("note mode write|split|preview"),
                    };
                    notes.update(cx, |n, cx| n.set_mode(mode, cx));
                }
                _ => anyhow::bail!("note text <markdown>|image <path>…|mode write|split|preview"),
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "dictate" || verb == "dictate-file" || verb == "dictate-demo" {
        // Voice input without a pointer: `dictate` runs the mic button's real path (permission
        // prompts included); `dictate-file <path>` skips the mic and transcribes the file;
        // `dictate-demo [text]` opens voice mode with neither (`text` as heard so far).
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("{verb}: no Trek window");
            };
            let composer = trek.read(cx).composer.clone();
            if verb == "dictate" {
                composer.update(cx, |c, cx| c.dictate(window, cx));
            } else if verb == "dictate-demo" {
                composer.update(cx, |c, cx| c.dictate_demo(arg, window, cx));
            } else {
                composer.update(cx, |c, cx| c.dictate_file(PathBuf::from(arg), window, cx));
            }
            Ok(())
        })??;
        return Ok(());
    }
    if verb == "editor" {
        // The editor without a pointer: `editor on|off` (Editor mode), `editor root <folder>`,
        // `editor view explorer|search|scm`, `editor panel terminal|problems|output|tasks|off`,
        // `editor chat new|send <prompt>|<n>`, `editor ai on|off`, and `editor [open] <abs-path> [line]`
        // (a tab in Editor mode, the center surface in the harness). The AI side bar:
        // `editor send <prompt>` (through its input, chips and mode included), `editor type <text>`
        // (typed, not sent), `editor select <first> <last>` (lines in the editor), `editor add`
        // (⌘⇧L), `editor mode agent|plan|ask`, `editor review`, `editor keep|undo all|<path>`.
        // The editor and the chat together: `editor inline [text]` (⌘K's card, typed into),
        // `editor inline-run <text>` (and run), `editor hunk keep|undo|next|prev`,
        // `editor diag <abs-path> <line> <message>` (a problem, as a language server would say),
        // `editor fix` (Fix with Agent on the first problem), `editor term-chat`, and
        // `editor rclick|hover <element id>` (a context menu, a hover, without a pointer; `name#3`
        // for a row's id). `editor snap <name>`: the preview's web view (see `editor_snap`).
        let (sub, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("editor: no Trek window");
            };
            let ide = trek.read(cx).ide.clone();
            let ws = ws.clone();
            match (sub, rest) {
                ("on", _) => ws.update(cx, |ws, cx| ws.set_mode(crate::workspace::Mode::Editor, cx)),
                ("off", _) => ws.update(cx, |ws, cx| ws.set_mode(crate::workspace::Mode::Agents, cx)),
                ("root", path) => ws.update(cx, |ws, cx| ws.set_ide_root(PathBuf::from(path), cx)),
                ("view", v) => {
                    let v = match v {
                        "explorer" => crate::ide::SideView::Explorer,
                        "search" => crate::ide::SideView::Search,
                        "scm" => crate::ide::SideView::Scm,
                        other => anyhow::bail!("editor view: unknown view {other:?} (explorer|search|scm)"),
                    };
                    ide.update(cx, |ide, cx| ide.show_view(v, window, cx));
                }
                ("panel", "off") => ide.update(cx, |ide, cx| {
                    if ide.layout.panel_open {
                        ide.toggle_panel(window, cx);
                    }
                }),
                ("panel", p) => {
                    let tab = match p {
                        "terminal" => crate::ide::PanelTab::Terminal,
                        "problems" => crate::ide::PanelTab::Problems,
                        "output" => crate::ide::PanelTab::Output,
                        "tasks" => crate::ide::PanelTab::Tasks,
                        other => anyhow::bail!("editor panel: unknown tab {other:?} (terminal|problems|output|tasks|off)"),
                    };
                    ide.update(cx, |ide, cx| ide.show_panel(tab, window, cx));
                }
                ("ai", on) => ide.update(cx, |ide, cx| {
                    if ide.layout.ai_open != (on == "on") {
                        ide.toggle_ai(window, cx);
                    }
                }),
                ("send", text) => {
                    let input = ide.read(cx).ai.read(cx).input.clone();
                    input.update(cx, |i, cx| i.send_text(text, window, cx));
                }
                ("type", text) => {
                    let input = ide.read(cx).ai.read(cx).input.clone();
                    input.update(cx, |i, cx| i.insert_text(text, window, cx));
                }
                ("select", lines) => {
                    let (a, b) = lines.split_once(' ').ok_or_else(|| anyhow::anyhow!("editor select <first> <last>"))?;
                    let (a, b): (usize, usize) = (a.parse()?, b.parse()?);
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor select: no file open"))?;
                    let state = editor.read(cx).text_state();
                    state.update(cx, |s, cx| {
                        use gpui_kit::base::input::{Point, RopeExt};
                        let text = s.text().clone();
                        let from = text.point_to_offset(Point::new(a.saturating_sub(1), 0));
                        let to = text.point_to_offset(Point::new(b, 0)).saturating_sub(1).max(from);
                        s.set_selected_range(from..to, cx);
                    });
                }
                ("add", _) => ide.update(cx, |ide, cx| ide.add_selection(false, window, cx)),
                (verb @ ("inline" | "inline-run"), text) => {
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor inline: no file open"))?;
                    editor.update(cx, |e, cx| e.type_inline(text, verb == "inline-run", window, cx));
                }
                ("hunk", what) => {
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor hunk: no file open"))?;
                    editor.update(cx, |e, cx| match what {
                        "keep" => e.act_on_hunk(true, cx),
                        "undo" => e.act_on_hunk(false, cx),
                        "prev" => e.step_hunk(-1, window, cx),
                        _ => e.step_hunk(1, window, cx),
                    });
                }
                ("diag", rest) => {
                    let mut it = rest.splitn(3, ' ');
                    let (Some(path), Some(line), Some(message)) = (it.next(), it.next(), it.next()) else { anyhow::bail!("editor diag <abs-path> <line> <message>") };
                    let line: u32 = line.parse::<u32>()?.saturating_sub(1);
                    let at = lsp_types::Position { line, character: 0 };
                    let d = lsp_types::Diagnostic { range: lsp_types::Range { start: at, end: at }, severity: Some(lsp_types::DiagnosticSeverity::ERROR), message: message.to_string(), source: Some("rustc".into()), ..Default::default() };
                    let path = PathBuf::from(path);
                    ws.update(cx, |ws, cx| {
                        let mut all = ws.diagnostics.get(&path).cloned().unwrap_or_default();
                        all.push(d);
                        ws.set_diagnostics(path, all, cx);
                    });
                }
                ("fix", _) => {
                    let first = ws.read(cx).diagnostics.iter().next().map(|(p, d)| (p.clone(), d[0].clone()));
                    let (path, d) = first.ok_or_else(|| anyhow::anyhow!("editor fix: no problems"))?;
                    ide.update(cx, |ide, cx| ide.fix_problem(path, d, window, cx));
                }
                ("term-chat", _) => ide.update(cx, |ide, cx| ide.terminal_to_chat(window, cx)),
                (verb @ ("rclick" | "hover"), id) => pointer(window, verb, id, cx)?,
                ("mode", m) => {
                    let mode = match m {
                        "agent" => crate::workspace::ChatMode::Agent,
                        "plan" => crate::workspace::ChatMode::Plan,
                        "ask" => crate::workspace::ChatMode::Ask,
                        other => anyhow::bail!("editor mode: unknown mode {other:?} (agent|plan|ask)"),
                    };
                    ws.update(cx, |ws, cx| ws.set_chat_mode_in(&crate::workspace::Scope::Ide, mode, cx));
                }
                ("review", _) => ws.update(cx, |ws, cx| {
                    if let Some(id) = ws.ide_chat.active_thread().map(str::to_string) {
                        ws.open_review(&id, cx);
                    }
                }),
                (verb @ ("keep" | "undo"), which) => ws.update(cx, |ws, cx| {
                    let Some(id) = ws.ide_chat.active_thread().map(str::to_string) else { return };
                    let paths = (which != "all").then(|| vec![which.to_string()]);
                    if verb == "keep" { ws.keep_files(&id, paths, cx) } else { ws.undo_files(&id, paths, cx) }
                }),
                // `editor diff <rel> [staged]`: a file's changes in a diff tab, as Source Control
                // opens them; `editor split on|off`: the diff in front side by side.
                ("diff", rest) => {
                    let (rel, staged) = match rest.strip_suffix(" staged") {
                        Some(r) => (r, true),
                        None => (rest, false),
                    };
                    let root = ws.read(cx).ide_root.clone().ok_or_else(|| anyhow::anyhow!("editor diff: no folder"))?;
                    let top = crate::ide::git::top(&root).ok_or_else(|| anyhow::anyhow!("editor diff: not a repository"))?;
                    let source = crate::ide::diff_view::DiffSource::Git { top, rel: rel.to_string(), staged };
                    ide.update(cx, |ide, cx| _ = ide.open_diff_view(source, false, window, cx));
                }
                ("split", on) => {
                    let diff = ide.read(cx).tabs.get(ide.read(cx).active).and_then(|t| t.diff().cloned()).ok_or_else(|| anyhow::anyhow!("editor split: no diff in front"))?;
                    diff.update(cx, |d, cx| d.set_split(on == "on", cx));
                }
                // `editor new-file`: the Explorer's name input for a new file; `editor rename-chat`:
                // the chat tab's; `editor edit-last`: the chat's last message back to edit;
                // `editor undo-turn`: the last turn's Undo, asking.
                ("new-file", _) => {
                    let explorer = ide.read(cx).explorer.clone();
                    explorer.update(cx, |e, cx| {
                        if let Some(dir) = e.target_dir() {
                            e.begin_new(dir, false, window, cx);
                        }
                    });
                }
                ("rename-chat", _) => {
                    let active = ws.read(cx).ide_chat.active;
                    let ai = ide.read(cx).ai.clone();
                    ai.update(cx, |a, cx| a.begin_rename(active, window, cx));
                }
                ("edit-last", _) => ws.update(cx, |ws, cx| {
                    let Some(id) = ws.ide_chat.active_thread().map(str::to_string) else { return };
                    let Some(live) = ws.live.get(&id) else { return };
                    let last = live.items.iter().enumerate().rev().find_map(|(i, it)| match it {
                        trek_core::store::Item::User { text, aside: false, .. } => Some((live.items.id_at(i).map(str::to_string), text.clone())),
                        _ => None,
                    });
                    if let Some((Some(item), text)) = last {
                        cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: crate::workspace::Scope::Ide, thread: id, text, images: vec![], edit: Some(item) });
                    }
                }),
                ("undo-turn", _) => {
                    let transcript = ide.read(cx).ai.read(cx).transcript.clone();
                    transcript.update(cx, |t, cx| t.confirm_last_undo(cx));
                }
                // `editor has <text>`: the file in front shows `text`; `editor note <text>`: the
                // preview in front says it (a file being fetched, or why one can't be shown).
                ("has", text) => {
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor has: no file in front"))?;
                    let shown = editor.read(cx).text_state().read(cx).text().to_string();
                    anyhow::ensure!(shown.contains(text), "editor has: {text:?} isn't in the file in front ({} bytes)", shown.len());
                }
                ("note", text) => {
                    let preview = ide.read(cx).active_preview().ok_or_else(|| anyhow::anyhow!("editor note: no preview in front"))?;
                    let note = preview.read(cx).note_text().unwrap_or_default();
                    anyhow::ensure!(note.contains(text), "editor note: the preview says {note:?}, not {text:?}");
                }
                ("chat", "new") => ws.update(cx, |ws, cx| ws.ide_new_chat(cx)),
                ("chat", r) if r.starts_with("send ") => ws.update(cx, |ws, cx| ws.send_in(&crate::workspace::Scope::Ide, r["send ".len()..].to_string(), vec![], cx)),
                ("chat", n) => {
                    let n: usize = n.parse().map_err(|_| anyhow::anyhow!("editor chat: new|send <prompt>|<tab number>"))?;
                    ws.update(cx, |ws, cx| ws.ide_select_chat(n, cx));
                }
                _ => {
                    // `editor [open] <abs-path> [line]`.
                    let target = if sub == "open" { rest } else { arg };
                    let (path, line) = match target.rsplit_once(' ') {
                        Some((p, n)) if n.parse::<u32>().is_ok() => (p, n.parse().ok()),
                        _ => (target, None),
                    };
                    let path = PathBuf::from(path);
                    trek.update(cx, |t, cx| t.open_editor(path, line, window, cx));
                }
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if matches!(
        verb,
        "click"
            | "rclick"
            | "hover"
            | "scroll"
            | "reveal"
            | "reveal-in"
            | "type"
            | "key"
            | "resize"
            | "elements"
            | "count"
            | "within"
    ) {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            match verb {
                "click" | "rclick" | "hover" => pointer(window, verb, arg, cx),
                "type" => {
                    anyhow::ensure!(!arg.is_empty(), "type needs text");
                    type_text(root, window, arg, cx)
                }
                "key" => {
                    use gpui_kit::test::TestWindowExt as _;
                    anyhow::ensure!(
                        !arg.is_empty(),
                        "key needs a keystroke (secondary-k, escape, shift-tab …)"
                    );
                    // Manifests write the Mac's keys (`cmd-k`); Trek's bindings read a Mac ⌘ as
                    // Ctrl on Windows, so the harness does too — `secondary` is gpui's spelling
                    // of exactly that, keeping one manifest right on both platforms.
                    let keys: Vec<String> = arg.split_whitespace().map(manifest_key).collect();
                    // All of them parse before any is pressed: a typo doesn't leave half a sequence done.
                    for k in &keys {
                        Keystroke::parse(k)
                            .map_err(|e| anyhow::anyhow!("key: bad keystroke {k:?}: {e}"))?;
                    }
                    for k in &keys {
                        window.press(k, cx);
                    }
                    Ok(())
                }
                "scroll" => {
                    use gpui_kit::test::TestWindowExt as _;
                    let (id, dy) = arg
                        .rsplit_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("scroll <element id> <px>"))?;
                    let dy: f32 = dy.trim().parse()?;
                    pointer(window, "hover", id, cx)?;
                    // Positive is down the page, as a wheel turned towards you.
                    window.scroll(
                        element_id(id),
                        ScrollDelta::Pixels(point(px(0.), px(-dy))),
                        cx,
                    );
                    Ok(())
                }
                // `reveal <element id>`: Settings scrolled from its top, a wheel's turn at a time, until
                // the element is on screen (`count` counts only what is); `reveal-in <scrolling
                // element id> <element id>` scrolls another.
                "reveal" | "reveal-in" => {
                    use gpui_kit::test::TestWindowExt as _;
                    let (within, id) = if verb == "reveal-in" {
                        arg.split_once(' ').ok_or_else(|| {
                            anyhow::anyhow!("reveal-in <scrolling element id> <element id>")
                        })?
                    } else {
                        ("settings-content", arg)
                    };
                    let id = id.trim();
                    anyhow::ensure!(!id.is_empty(), "{verb} needs an element id");
                    // As `count` reads one: `name*` is any id that starts so.
                    let on_screen = |window: &mut Window, cx: &mut App| {
                        window.render_frame(cx);
                        gpui_kit::base::test_support::snapshots(window)
                            .into_iter()
                            .filter(|s| s.visible())
                            .any(|s| match (id.strip_suffix('*'), s.path().last()) {
                                (Some(prefix), Some(last)) => id_text(last).starts_with(prefix),
                                (None, last) => last == Some(&element_id(id)),
                                _ => false,
                            })
                    };
                    pointer(window, "hover", within, cx)?;
                    let wheel = |window: &mut Window, cx: &mut App, dy: f32| {
                        window.scroll(
                            element_id(within),
                            ScrollDelta::Pixels(point(px(0.), px(dy))),
                            cx,
                        )
                    };
                    wheel(window, cx, 100_000.);
                    for _ in 0..40 {
                        if on_screen(window, cx) {
                            return Ok(());
                        }
                        wheel(window, cx, -250.);
                    }
                    anyhow::ensure!(
                        on_screen(window, cx),
                        "{verb}: {id} isn't on screen anywhere in {within}"
                    );
                    Ok(())
                }
                "count" => {
                    let (id, n) = arg
                        .rsplit_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("count <element id>[*] <n>[+]"))?;
                    // `3+`: three or more.
                    let (n, more) = match n.trim().strip_suffix('+') {
                        Some(n) => (n, true),
                        None => (n.trim(), false),
                    };
                    let n: usize = n.parse()?;
                    let found = shown(window, cx, id);
                    anyhow::ensure!(
                        found == n || more && found > n,
                        "count {id}: {found} on screen, not {n}{}",
                        if more { " or more" } else { "" }
                    );
                    Ok(())
                }
                // `within <element id>[*] <element id>`: each element the first names (one at
                // least) lies inside the second, as drawn (half a pixel's leeway).
                "within" => {
                    use gpui_kit::test::TestWindowExt as _;
                    let (inner, outer) = arg
                        .trim()
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("within <element id>[*] <element id>"))?;
                    window.render_frame(cx);
                    let snapshots: Vec<_> = gpui_kit::base::test_support::snapshots(window)
                        .into_iter()
                        .filter(|s| s.visible())
                        .collect();
                    let outer_id = element_id(outer.trim());
                    let outer_bounds = snapshots
                        .iter()
                        .find(|s| s.path().last() == Some(&outer_id))
                        .map(|s| s.bounds())
                        .ok_or_else(|| anyhow::anyhow!("within: no {outer} on screen"))?;
                    let inside: Vec<_> = snapshots
                        .iter()
                        .filter(|s| match (inner.strip_suffix('*'), s.path().last()) {
                            (Some(prefix), Some(last)) => id_text(last).starts_with(prefix),
                            (None, last) => last == Some(&element_id(inner)),
                            _ => false,
                        })
                        .collect();
                    anyhow::ensure!(!inside.is_empty(), "within: no {inner} on screen");
                    let slack = px(0.5);
                    let out: Vec<String> = inside
                        .iter()
                        .filter(|s| {
                            let b = s.bounds();
                            b.left() < outer_bounds.left() - slack
                                || b.right() > outer_bounds.right() + slack
                                || b.top() < outer_bounds.top() - slack
                                || b.bottom() > outer_bounds.bottom() + slack
                        })
                        .map(|s| {
                            let id = s.path().last().map(id_text).unwrap_or_default();
                            format!("{id} at {:?}", s.bounds())
                        })
                        .collect();
                    anyhow::ensure!(
                        out.is_empty(),
                        "within: outside {outer} ({outer_bounds:?}): {}",
                        out.join(", ")
                    );
                    Ok(())
                }
                "resize" => {
                    let (w, h) = arg
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("resize <width> <height>"))?;
                    let (w, h): (f32, f32) = (w.trim().parse()?, h.trim().parse()?);
                    anyhow::ensure!(w >= 100. && h >= 100., "resize: {w}×{h} is too small");
                    window.resize(size(px(w), px(h)));
                    Ok(())
                }
                _ => elements(window, cx),
            }
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    // Files in the composer, as a drop puts them there: images as thumbnails, the rest (videos,
    // PDFs, folders) as tiles. Paths are separated by `|` (they may have spaces).
    // `open-link <trek:// link>`: the link as the system hands it to Trek (a browser button, an
    // editor extension, a banner clicked), through `deep_link::open`.
    if verb == "open-link" {
        anyhow::ensure!(
            arg.starts_with("trek://"),
            "open-link takes a trek:// link, not {arg:?}"
        );
        crate::deep_link::open(arg, cx);
        cx.refresh_windows();
        return Ok(());
    }
    // `file <path> [contains <text>]` and `private <path>`, `path` in the data folder unless it's
    // absolute, and `*name` as its last part any file there whose name ends with `name`: it's
    // there (holding `text`); and the user alone can read it (`trek_private::is_private`), as
    // written by the app.
    if verb == "file" || verb == "private" {
        let (path, text) = match arg.split_once(" contains ") {
            Some((path, text)) => (path.trim(), Some(text)),
            None => (arg.trim(), None),
        };
        let given = PathBuf::from(path);
        let path = if given.is_absolute() {
            given
        } else {
            trek_core::paths::data_dir().join(given)
        };
        let ending = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix('*'))
            .map(str::to_string);
        let path = match ending {
            Some(end) => {
                let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.ends_with(&end))
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("{verb}: nothing in {} ends with {end:?}", dir.display())
                    })?
            }
            None => path,
        };
        anyhow::ensure!(path.exists(), "{verb}: {} isn't there", path.display());
        if let Some(text) = text {
            let held = std::fs::read_to_string(&path)?;
            anyhow::ensure!(
                held.contains(text),
                "file: {} doesn't hold {text:?}",
                path.display()
            );
        }
        if verb == "private" {
            anyhow::ensure!(
                trek_private::is_private(&path)?,
                "private: others can read {}",
                path.display()
            );
        }
        return Ok(());
    }
    // `keep-working`: the main window's close button with an agent at work, answered Keep Working
    // (`root::keep_working`): every window goes, and Trek carries on until the work is done;
    // `show-main` brings the main window back, as the tray or the Dock does.
    if verb == "keep-working" {
        crate::root::keep_working(Duration::from_secs(120), cx);
        return Ok(());
    }
    if verb == "show-main" {
        crate::root::show_main(ws.clone(), cx);
        return Ok(());
    }
    // `close-main`: the main window's close button by its own path (`root::close_main`), where
    // the app ends with that window (Windows): with Keep Trek in the tray and the tray icon on, the
    // windows close and Trek stays. Refused otherwise, as it would quit the run.
    if verb == "close-main" {
        let stays = {
            let settings = &ws.read(cx).settings;
            settings.general.stay_in_tray && settings.notifications.menu_bar_icon
        };
        anyhow::ensure!(
            stays,
            "close-main: only with Keep Trek in the tray and the tray icon on (else it would quit the run)"
        );
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |_, window, cx| crate::root::close_main(window, cx))?;
        return Ok(());
    }
    // `badge <n>[+]`: the count on the app's icon as last put up, the system having taken it
    // (`system::SHOWN_BADGE`): n, or n or more.
    if verb == "badge" {
        let (n, more) = match arg.trim().strip_suffix('+') {
            Some(n) => (n, true),
            None => (arg.trim(), false),
        };
        let n: usize = n.parse()?;
        let shown = crate::system::SHOWN_BADGE.load(Ordering::Relaxed);
        anyhow::ensure!(
            shown == n || more && shown > n,
            "badge: {shown} on the app's icon, not {n}{}",
            if more { " or more" } else { "" }
        );
        return Ok(());
    }
    // `tray off|on|idle|working|attention`: the icon in the system's tray (the menu bar on a
    // Mac): none; or one the system has on screen, showing any state or the one named.
    if verb == "tray" {
        let want = arg.trim();
        match (want, crate::tray::shown(cx)) {
            ("off", None) => {}
            ("off", Some((glyph, _, _))) => anyhow::bail!("tray: the icon is up ({glyph})"),
            (_, None) => anyhow::bail!("tray: no icon"),
            (want, Some((glyph, on_screen, status))) => {
                anyhow::ensure!(
                    on_screen,
                    "tray: the system has no icon on screen ({glyph}: {status})"
                );
                anyhow::ensure!(
                    want == "on" || want == glyph,
                    "tray: {glyph} ({status}), not {want}"
                );
            }
        }
        return Ok(());
    }
    // `native`: Trek runs as this processor's own code, not translated or emulated.
    if verb == "native" {
        return native_build();
    }
    // `self-contained`: Trek runs with nothing installed beside it: none of the Visual C++
    // redistributable's libraries is loaded in it on Windows, and on a Mac each library loaded
    // is the system's or Trek's own.
    if verb == "self-contained" {
        return self_contained();
    }
    // `banners mark` | `banners new <n>[+]`: the banners in Windows' notification center under
    // Trek's app id, as the system lists them (`ToastNotificationHistory`). `mark` notes those
    // there now (an installed Trek's share the id); `new` counts the others, and takes them away
    // again, so a check leaves none behind. A Mac's banners need the installed app.
    if verb == "banners" {
        anyhow::ensure!(
            cfg!(windows),
            "banners: Windows' alone (a Mac's banners need the installed app)"
        );
        #[cfg(windows)]
        {
            let now = banner_tags()?;
            let mut marked = BANNERS.lock().unwrap_or_else(|e| e.into_inner());
            match arg.trim().split_once(' ').unwrap_or((arg.trim(), "")) {
                ("mark", _) => *marked = now,
                ("new", n) => {
                    let (n, more) = match n.trim().strip_suffix('+') {
                        Some(n) => (n, true),
                        None => (n.trim(), false),
                    };
                    let n: usize = n.parse()?;
                    let new: Vec<String> =
                        now.into_iter().filter(|t| !marked.contains(t)).collect();
                    // Taken away by the Trek that posted them, as GPUI takes its own banners
                    // away: Windows won't take one with no group away by its tag.
                    let ids: Vec<String> =
                        ws.read(cx).threads.iter().map(|t| t.id.clone()).collect();
                    for id in &ids {
                        cx.dismiss_system_notification(&crate::root::attention_tag(id));
                    }
                    let left = banner_tags()?.iter().filter(|t| new.contains(t)).count();
                    anyhow::ensure!(
                        left == 0,
                        "banners: {left} of the check's still in the notification center"
                    );
                    anyhow::ensure!(
                        new.len() == n || more && new.len() > n,
                        "banners: {} new in the notification center, not {n}{}",
                        new.len(),
                        if more { " or more" } else { "" }
                    );
                }
                _ => anyhow::bail!("banners mark | new <n>[+]"),
            }
        }
        return Ok(());
    }
    // `motion on|system`: animations go whatever the system asks (as the render tests turn them
    // on; a CI runner's system has them off), or follow the system's own setting again.
    if verb == "motion" {
        let forced = match arg.trim() {
            "on" => true,
            "system" => false,
            other => anyhow::bail!("motion {other}? (on|system)"),
        };
        crate::system::MOTION_FORCED.store(forced, Ordering::Relaxed);
        crate::system::apply_reduce_motion(ws, cx);
        return Ok(());
    }
    // `signed-out`: the capture run is signed out of the user's GitHub account: `gh`, started as
    // Trek starts it, has no login (`isolate_accounts`). Passes where there's no `gh` at all.
    if verb == "signed-out" {
        let Some(gh) = trek_core::detect::which("gh") else {
            return Ok(());
        };
        let status = trek_core::procs::command(gh)
            .args(["auth", "status"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        anyhow::ensure!(
            !status.success(),
            "signed-out: gh is signed in to an account from inside the capture run"
        );
        return Ok(());
    }
    // `renders mark` | `renders <view> <bound>…`: how often each view drew since `mark`, counted
    // as the render tests count them (`test_support::rendered`): at least n (`n+`), at most n
    // (`n-`), or no more often than another view (`<=<view>`).
    if verb == "renders" {
        let arg = arg.trim();
        if arg == "mark" {
            crate::test_support::take_renders();
            return Ok(());
        }
        let counts = crate::test_support::take_renders();
        let words: Vec<&str> = arg.split_whitespace().collect();
        anyhow::ensure!(
            !words.is_empty() && words.len() % 2 == 0,
            "renders mark | <view> <n>+|<n>-|<=<view>…"
        );
        let mut wrong = Vec::new();
        for pair in words.chunks(2) {
            let (view, bound) = (pair[0], pair[1]);
            let drew = counts.get(view).copied().unwrap_or(0);
            let ok = if let Some(other) = bound.strip_prefix("<=") {
                drew <= counts.get(other).copied().unwrap_or(0)
            } else {
                match (bound.strip_suffix('+'), bound.strip_suffix('-')) {
                    (Some(n), _) => drew >= n.parse::<usize>()?,
                    (_, Some(n)) => drew <= n.parse::<usize>()?,
                    _ => anyhow::bail!("renders: {bound}? (n+, n- or <=view)"),
                }
            };
            if !ok {
                wrong.push(format!("{view} drew {drew} times, not {bound}"));
            }
        }
        anyhow::ensure!(
            wrong.is_empty(),
            "renders: {} (all: {counts:?})",
            wrong.join("; ")
        );
        return Ok(());
    }
    // `banner-permission off|restore` (Windows): Trek's switch in Windows' notification settings,
    // where Settings › System › Notifications keeps it, as the user flips it: off, then back to
    // exactly what was there (the installed Trek shares the app id).
    if verb == "banner-permission" {
        anyhow::ensure!(cfg!(windows), "banner-permission: Windows' alone");
        #[cfg(windows)]
        {
            let key = format!(
                r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\{}",
                crate::winsys::APP_ID
            );
            let mut saved = BANNER_SWITCH.lock().unwrap_or_else(|e| e.into_inner());
            match arg.trim() {
                "off" => {
                    if saved.is_none() {
                        *saved = Some(read_dword(&key, "Enabled"));
                    }
                    write_dword(&key, "Enabled", Some(0))?;
                }
                "restore" => {
                    let before = saved.take().ok_or_else(|| {
                        anyhow::anyhow!("banner-permission restore: it wasn't turned off")
                    })?;
                    write_dword(&key, "Enabled", before)?;
                }
                _ => anyhow::bail!("banner-permission off|restore"),
            }
        }
        return Ok(());
    }
    // `file-app <ext> on|off` (Windows): a program of the check's own as Windows' default for
    // `.<ext>`, in the user's classes and gone again with `off`. Opening such a file copies it to
    // `opened.<ext>` in the data folder (a console with no window, `conhost --headless`), so
    // `file opened.<ext>` sees what Trek handed to the file's app.
    if verb == "file-app" {
        anyhow::ensure!(cfg!(windows), "file-app: Windows' alone");
        #[cfg(windows)]
        {
            let (ext, on) = arg
                .trim()
                .split_once(' ')
                .ok_or_else(|| anyhow::anyhow!("file-app <ext> on|off"))?;
            anyhow::ensure!(
                !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()),
                "file-app: {ext:?} isn't an extension"
            );
            let class = format!("TrekLiveCheck.{ext}");
            match on.trim() {
                "on" => {
                    let copy = trek_core::paths::data_dir().join(format!("opened.{ext}"));
                    let _ = std::fs::remove_file(&copy);
                    let system =
                        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
                    let command = format!(
                        r#""{system}\System32\conhost.exe" --headless "{system}\System32\cmd.exe" /d /s /c "copy /y "%1" "{}"""#,
                        copy.display()
                    );
                    write_string(&format!(r"Software\Classes\.{ext}"), &class)?;
                    write_string(
                        &format!(r"Software\Classes\{class}\shell\open\command"),
                        &command,
                    )?;
                }
                "off" => {
                    delete_tree(&format!(r"Software\Classes\.{ext}"));
                    delete_tree(&format!(r"Software\Classes\{class}"));
                }
                other => anyhow::bail!("file-app {ext} {other}? (on|off)"),
            }
            associations_changed();
        }
        return Ok(());
    }
    // `bonjour on|found|off`: the stand-in phone server announced on the local network as Trek
    // announces its own on Wi-Fi (`remote::bonjour::Advert`, through the system's own responder),
    // under a name of the check's own and never this computer's (the user's own Trek may be
    // announcing that), with this Trek's lasting id; found as a phone finds it (`shots/dns_sd.rs`:
    // resolved through the system's DNS-SD, its port the server's and its TXT record this Trek's
    // id and that name); and taken off the network again.
    #[cfg(any(windows, target_os = "macos"))]
    if verb == "bonjour" {
        static ADVERT: Mutex<Option<(crate::remote::bonjour::Advert, u16)>> = Mutex::new(None);
        const NAME: &str = "Trek live check";
        let mut advert = ADVERT.lock().unwrap_or_else(|e| e.into_inner());
        match arg.trim() {
            "on" => {
                let port = ws.update(cx, |ws, cx| -> anyhow::Result<u16> {
                    ws.start_loopback_remote(cx)?;
                    Ok(ws.settings.mobile.port)
                })?;
                let id = crate::remote::host_info().id;
                let made =
                    crate::remote::bonjour::Advert::new(NAME, &id, port).ok_or_else(|| {
                        anyhow::anyhow!("bonjour on: the system's responder took no record")
                    })?;
                *advert = Some((made, port));
            }
            "found" => {
                let port = advert.as_ref().map(|(_, port)| *port).ok_or_else(|| {
                    anyhow::anyhow!("bonjour found: nothing announced (`bonjour on`)")
                })?;
                let found = dns_sd::resolve(NAME, Duration::from_secs(10))?;
                let id = format!("id={}", crate::remote::host_info().id);
                anyhow::ensure!(
                    found.port == port,
                    "bonjour found: port {}, not the server's {port}",
                    found.port
                );
                anyhow::ensure!(
                    found.entries.contains(&id) && found.entries.contains(&format!("name={NAME}")),
                    "bonjour found: its record says {:?}, not this Trek's {id}",
                    found.entries
                );
            }
            "off" => *advert = None,
            other => anyhow::bail!("bonjour {other}? (on|found|off)"),
        }
        return Ok(());
    }
    // `phone-server reach`: the phone server (turned on in Settings, listening on the network as
    // it does for phones) reached at this computer's address on its network, as a phone on the
    // Wi-Fi dials it: a connection to its port there (what's said over it is the server's tests').
    // CI's machines alone (the ledger's `live_ci`): on someone's own, Windows Firewall would ask.
    if verb == "phone-server" {
        match arg.trim() {
            "network" => {
                ws.update(cx, |ws, cx| ws.network_remote(true, cx));
                return Ok(());
            }
            "off" => {
                ws.update(cx, |ws, cx| ws.network_remote(false, cx));
                return Ok(());
            }
            "reach" => {}
            other => anyhow::bail!("phone-server {other}? (network|reach|off)"),
        }
        // The server comes up in the background: the steps wait for it first.
        let w = ws.read(cx);
        let lan = w
            .remote
            .as_ref()
            .and_then(|r| r.addresses.lan)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "phone-server reach: the phone server isn't on a network (on: {})",
                    w.remote.is_some()
                )
            })?;
        let port = w.settings.mobile.port;
        let at = std::net::SocketAddr::from((lan, port));
        std::net::TcpStream::connect_timeout(&at, Duration::from_secs(5))
            .map_err(|e| anyhow::anyhow!("phone-server reach: {at}: {e}"))?;
        return Ok(());
    }
    // `window front` (Windows): Trek's window in front of the others for real, with the keyboard,
    // as a click on it puts it (Windows lets a program come forward only right after input: a
    // press of Alt is that input). `share asked <n>[+]`: how often Windows asked Trek for a share's
    // data, which it does only when its share window opens. `share close`: Escape, as the user
    // presses it, to the share window in front. CI's machines alone (the ledger's `live_ci`).
    if verb == "window" || verb == "share" {
        anyhow::ensure!(cfg!(windows), "{verb}: Windows' alone");
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_ESCAPE,
                VK_MENU,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
                SetForegroundWindow,
            };
            let press = |key: u16| {
                let input = |flags| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: key,
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let both = [input(0), input(KEYEVENTF_KEYUP)];
                // SAFETY: two keyboard inputs, of the size given.
                unsafe { SendInput(2, both.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
            };
            match (verb, arg.trim()) {
                ("window", "front") => {
                    let main = ws
                        .read(cx)
                        .main_window
                        .ok_or_else(|| anyhow::anyhow!("window front: no main window"))?;
                    let hwnd = main
                        .update(cx, |_, window, _| crate::winsys::hwnd_of(window))?
                        .ok_or_else(|| anyhow::anyhow!("window front: no window handle"))?;
                    press(VK_MENU);
                    // SAFETY: this process's own top-level window, from its own (UI) thread; the
                    // front window's input is shared only while it's brought forward, as Windows
                    // lets a program come forward when it shares the input of the one in front.
                    let front = unsafe {
                        let ours = GetCurrentThreadId();
                        let theirs =
                            GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
                        let shared = theirs != 0
                            && theirs != ours
                            && AttachThreadInput(ours, theirs, 1) != 0;
                        SetForegroundWindow(hwnd as _);
                        BringWindowToTop(hwnd as _);
                        if shared {
                            AttachThreadInput(ours, theirs, 0);
                        }
                        GetForegroundWindow() as isize == hwnd
                    };
                    anyhow::ensure!(front, "window front: Windows kept another window in front");
                }
                ("share", "close") => press(VK_ESCAPE),
                ("share", n) if n.starts_with("asked ") => {
                    let n = n.trim_start_matches("asked ").trim();
                    let (n, more) = match n.strip_suffix('+') {
                        Some(n) => (n, true),
                        None => (n, false),
                    };
                    let n: usize = n.parse()?;
                    let asked = crate::share::SHARE_ASKED.load(Ordering::Relaxed);
                    anyhow::ensure!(
                        asked == n || more && asked > n,
                        "share asked: Windows asked {asked} times, not {n}{}",
                        if more { " or more" } else { "" }
                    );
                }
                (verb, other) => anyhow::bail!(
                    "{verb} {other}? (window front | share asked <n>[+] | share close)"
                ),
            }
        }
        return Ok(());
    }
    // `computer open <program>`, `computer elements <window> expect <text>`, `computer close`:
    // computer use by element, live (`shots/computer_use.rs`): another app opened for the check,
    // read through Trek's own computer tools as an agent reads it (its outline must hold `text`),
    // and closed. On CI's machines alone (the ledger's `live_ci`).
    #[cfg(any(windows, target_os = "macos"))]
    if verb == "computer" {
        match arg.trim().split_once(' ').unwrap_or((arg.trim(), "")) {
            ("open", app) => computer_use::open(app.trim())?,
            ("close", "") => computer_use::close(),
            ("elements", rest) => {
                let (window, want) = rest
                    .split_once(" expect ")
                    .ok_or_else(|| anyhow::anyhow!("computer elements <window> expect <text>"))?;
                let outline = computer_use::elements(window.trim())?;
                anyhow::ensure!(
                    outline.contains(want.trim()),
                    "computer elements: {window}'s outline holds no {want:?}:\n{outline}"
                );
            }
            (other, _) => anyhow::bail!("computer {other}? (open|elements|close)"),
        }
        return Ok(());
    }
    // `cloud-file <abs-path> <text…>` (Windows): a file that lives only in the cloud, as OneDrive's
    // online-only files do: a placeholder whose data, `text`, comes from a sync provider of the
    // check's own (`shots/cloud_files.rs`), its folder the provider's sync root. Windows asks the
    // provider for the data when something reads the file, and it answers after a moment.
    // `cloud-file fetched <n>[+]`: how often it was asked; `cloud-file away`: it stops answering,
    // as OneDrive not running does; `cloud-file off`: the folder an ordinary one again.
    if verb == "cloud-file" {
        anyhow::ensure!(cfg!(windows), "cloud-file: Windows' alone");
        #[cfg(windows)]
        {
            match arg.trim().split_once(' ').unwrap_or((arg.trim(), "")) {
                ("away", "") => cloud_files::away()?,
                ("off", "") => cloud_files::off()?,
                ("fetched", n) => {
                    let (n, more) = match n.trim().strip_suffix('+') {
                        Some(n) => (n, true),
                        None => (n.trim(), false),
                    };
                    let n: usize = n.parse()?;
                    let fetched = cloud_files::fetched();
                    anyhow::ensure!(
                        fetched == n || more && fetched > n,
                        "cloud-file fetched: asked {fetched} times, not {n}{}",
                        if more { " or more" } else { "" }
                    );
                }
                (path, text) => cloud_files::file(Path::new(path), text.trim())?,
            }
        }
        return Ok(());
    }
    // `programs-stop`: end the programs `programs` found that still run (a preview window a check
    // opened, say).
    if verb == "programs-stop" {
        let found = PROGRAMS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (pid, program) in &found {
            if running_program(*pid).is_some_and(|now| now == *program) {
                end_process(*pid);
            }
        }
        return Ok(());
    }
    // `programs <name> <n>[+]`: running processes of program `name` (`ping`, `sleep`; `.exe` and
    // case aside) anywhere under Trek's own, as the system lists them; remembered for
    // `programs-ended`, which passes once every one of them has ended.
    if verb == "programs" {
        let (name, n) = arg
            .trim()
            .rsplit_once(' ')
            .ok_or_else(|| anyhow::anyhow!("programs <name> <n>[+]"))?;
        let (n, more) = match n.strip_suffix('+') {
            Some(n) => (n, true),
            None => (n, false),
        };
        let n: usize = n.parse()?;
        let found: Vec<(u32, String)> = trek_programs()
            .into_iter()
            .filter(|(_, program)| is_program(program, name.trim()))
            .collect();
        anyhow::ensure!(
            found.len() == n || more && found.len() > n,
            "programs: {} {name} under Trek, not {n}{}",
            found.len(),
            if more { " or more" } else { "" }
        );
        *PROGRAMS.lock().unwrap_or_else(|e| e.into_inner()) = found;
        return Ok(());
    }
    if verb == "programs-ended" {
        let found = std::mem::take(&mut *PROGRAMS.lock().unwrap_or_else(|e| e.into_inner()));
        anyhow::ensure!(
            !found.is_empty(),
            "programs-ended: no programs to watch (`programs` finds them)"
        );
        let left: Vec<String> = found
            .iter()
            .filter(|(pid, program)| running_program(*pid).is_some_and(|now| now == *program))
            .map(|(pid, program)| format!("{program} ({pid})"))
            .collect();
        anyhow::ensure!(
            left.is_empty(),
            "programs-ended: still running: {}",
            left.join(", ")
        );
        return Ok(());
    }
    // `console-windows mark` | `console-windows new <n>`: console windows on screen (the classic
    // console's, and Windows Terminal's, which Windows opens for a console program started without
    // `CREATE_NO_WINDOW`), and any window of a program under Trek: `mark` notes those there now,
    // `new` counts the others.
    if verb == "console-windows" {
        anyhow::ensure!(cfg!(windows), "console-windows: Windows' alone");
        #[cfg(windows)]
        {
            let under: Vec<u32> = trek_programs().into_iter().map(|(pid, _)| pid).collect();
            let now = console_windows(&under);
            let mut marked = CONSOLES.lock().unwrap_or_else(|e| e.into_inner());
            match arg.trim().split_once(' ').unwrap_or((arg.trim(), "")) {
                ("mark", _) => *marked = now.iter().map(|(hwnd, _)| *hwnd).collect(),
                ("new", n) => {
                    let n: usize = n.trim().parse()?;
                    let new: Vec<&String> = now
                        .iter()
                        .filter(|(hwnd, _)| !marked.contains(hwnd))
                        .map(|(_, what)| what)
                        .collect();
                    anyhow::ensure!(
                        new.len() == n,
                        "console-windows: {} new, not {n}: {new:?}",
                        new.len()
                    );
                }
                _ => anyhow::bail!("console-windows mark | new <n>"),
            }
        }
        return Ok(());
    }
    // `count-all <element id>[*] <n>[+]`: `count` over every one of Trek's windows (a thread's own,
    // the background use picture), not the main window alone.
    if verb == "count-all" {
        let (id, n) = arg
            .rsplit_once(' ')
            .ok_or_else(|| anyhow::anyhow!("count-all <element id>[*] <n>[+]"))?;
        let (n, more) = match n.trim().strip_suffix('+') {
            Some(n) => (n, true),
            None => (n.trim(), false),
        };
        let n: usize = n.parse()?;
        let mut found = 0;
        for window in cx.windows() {
            found += window
                .update(cx, |_, window, cx| shown(window, cx, id))
                .unwrap_or(0);
        }
        anyhow::ensure!(
            found == n || more && found > n,
            "count-all {id}: {found} on screen, not {n}{}",
            if more { " or more" } else { "" }
        );
        return Ok(());
    }
    // `heic-demo`: a photo as an iPhone writes one (HEIC), made by the system's own codec in the
    // data folder (`attachments::heic_photo`), then attached as `attach` does, going in as a
    // picture agents can read.
    if verb == "heic-demo" {
        let photo = trek_core::paths::data_dir()
            .join("heic-demo")
            .join("IMG_0001.HEIC");
        crate::attachments::heic_photo(&photo)?;
        return run(ws, "attach", &photo.to_string_lossy(), cx);
    }
    // `photos open|shown <0|1>|close` (Mac): the system's Photos picker, as the + menu's Photos
    // Library opens it (`Composer::attach_photos`), a sheet on Trek's window; whether that sheet
    // is up, as AppKit has it; and closed as its Cancel does. No open panel in its stead, as the
    // menu has where there's no picker: one left up would hold the rest of the run.
    #[cfg(target_os = "macos")]
    if verb == "photos" {
        use crate::photos::picker::{self, Opened};
        match arg.trim() {
            "open" => {
                let main = ws
                    .read(cx)
                    .main_window
                    .ok_or_else(|| anyhow::anyhow!("no main window"))?;
                main.update(cx, |root, window, cx| -> anyhow::Result<()> {
                    let trek = root
                        .downcast::<gpui_kit::component::Root>()
                        .ok()
                        .and_then(|r| {
                            r.read(cx)
                                .view()
                                .clone()
                                .downcast::<crate::root::TrekWindow>()
                                .ok()
                        })
                        .ok_or_else(|| anyhow::anyhow!("photos: no Trek window"))?;
                    let composer = trek.read(cx).composer.clone();
                    match picker::open(window, crate::photos::dir()) {
                        Opened::Showing(picked) => {
                            composer.update(cx, |c, cx| c.take_photos(picked, window, cx))
                        }
                        Opened::AlreadyShowing => {}
                        Opened::Unavailable => anyhow::bail!("photos open: no Photos picker here"),
                    }
                    Ok(())
                })??;
            }
            "close" => picker::cancel(),
            other => {
                let want = match other.strip_prefix("shown").map(str::trim) {
                    Some("1") => true,
                    Some("0") => false,
                    _ => anyhow::bail!("photos open|shown <0|1>|close"),
                };
                let up = picker::showing();
                anyhow::ensure!(
                    up == want,
                    "photos shown: the picker's sheet {} up",
                    if up { "is" } else { "isn't" }
                );
            }
        }
        return Ok(());
    }
    if verb == "attach" && !arg.is_empty() {
        let main = ws
            .read(cx)
            .main_window
            .ok_or_else(|| anyhow::anyhow!("no main window"))?;
        let paths: Vec<PathBuf> = arg.split('|').map(|p| PathBuf::from(p.trim())).collect();
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let trek = root
                .downcast::<gpui_kit::component::Root>()
                .ok()
                .and_then(|r| {
                    r.read(cx)
                        .view()
                        .clone()
                        .downcast::<crate::root::TrekWindow>()
                        .ok()
                })
                .ok_or_else(|| anyhow::anyhow!("attach: no Trek window"))?;
            let composer = trek.read(cx).composer.clone();
            composer.update(cx, |c, cx| c.add_paths(&paths, window, cx));
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "agent-install" {
        // How an install from the ACP Registry looks under way or failed, without downloading:
        // `agent-install <id> <percent>|fail <message>|clear`.
        use trek_core::registry::Progress;
        let (id, what) = arg
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("agent-install <id> <percent>|fail <message>|clear"))?;
        let state = match what.split_once(' ').map_or((what, ""), |(a, b)| (a, b)) {
            ("clear", _) => None,
            ("fail", why) => Some(crate::workspace::AgentInstall::Failed(why.to_string())),
            (pct, _) => {
                let pct: u64 = pct
                    .parse()
                    .map_err(|_| anyhow::anyhow!("agent-install: bad percent {pct:?}"))?;
                Some(crate::workspace::AgentInstall::Running(Some(
                    Progress::Downloading {
                        done: pct * 1_000_000,
                        total: Some(100_000_000),
                    },
                )))
            }
        };
        ws.update(cx, |ws, cx| {
            match state {
                Some(s) => ws.added_agents.installs.insert(id.to_string(), s),
                None => ws.added_agents.installs.remove(id),
            };
            cx.notify();
        });
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "rewind" {
        // The n-th turn from the end (1 = the latest), undone as its Undo button does: the files
        // too when there's a checkpoint for them (the confirmation's default), and the message
        // back in the composer. `rewind <n> keep` leaves the files alone.
        let (n, files) = arg.split_once(' ').unwrap_or((arg, ""));
        let n: usize = if n.is_empty() {
            1
        } else {
            n.parse()
                .map_err(|_| anyhow::anyhow!("rewind [n] [keep]: bad n {n:?}"))?
        };
        anyhow::ensure!(n >= 1, "rewind: n starts at 1 (the latest turn)");
        ws.update(cx, |ws, cx| -> anyhow::Result<()> {
            let id = ws
                .focused_thread()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("rewind: no thread on screen"))?;
            let live = ws
                .live
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("rewind: the thread isn't loaded"))?;
            let ends: Vec<usize> = live
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| matches!(i, trek_core::store::Item::TurnEnd { .. }))
                .map(|(ix, _)| ix)
                .collect();
            let ix = *ends.iter().rev().nth(n - 1).ok_or_else(|| {
                anyhow::anyhow!("rewind {n}: the thread has {} finished turns", ends.len())
            })?;
            let end = live
                .items
                .id_at(ix)
                .ok_or_else(|| anyhow::anyhow!("rewind {n}: the turn has no id"))?
                .to_string();
            let start = ws
                .turn_start_item(&id, &end)
                .ok_or_else(|| anyhow::anyhow!("rewind {n}: no message starts that turn"))?;
            let restore = files != "keep" && ws.restorable_checkpoint(&id, &start).is_some();
            let (text, images) = ws
                .undo_turn(&id, &end, restore, cx)
                .ok_or_else(|| anyhow::anyhow!("rewind {n}: refused (see the toast)"))?;
            cx.emit(crate::workspace::WorkspaceEvent::ComposeIn {
                scope: ws.focused_scope(),
                thread: id,
                text,
                images,
                edit: None,
            });
            Ok(())
        })?;
        cx.refresh_windows();
        return Ok(());
    }
    if matches!(verb, "approve" | "deny" | "answer") {
        ws.update(cx, |ws, cx| card(ws, verb, arg, cx))?;
        cx.refresh_windows();
        return Ok(());
    }
    // A Trek theme, as the palette's "Use … theme" switches to it (`look <id>`), and the themes
    // folder read again now rather than on its next look (`look reread`).
    if verb == "look" {
        if arg == "reread" {
            crate::themes::reread(&ws, cx);
        } else if let Some(want) = arg.strip_prefix("is ") {
            // A check: that theme on screen and in the settings, its switch over.
            let shown = crate::themes::shown_id(cx);
            let set = ws.read(cx).settings.appearance.trek_theme.clone();
            anyhow::ensure!(
                shown == want.trim() && set == want.trim() && !crate::themes::switching(cx),
                "look is {want}: {shown} is on screen, the settings name {set}"
            );
        } else {
            anyhow::ensure!(
                crate::themes::find(arg, cx).is_some(),
                "look: no theme {arg:?} (one of {})",
                crate::themes::list(cx)
                    .iter()
                    .map(|t| t.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            crate::themes::switch_to(arg, &ws, cx);
        }
        cx.refresh_windows();
        return Ok(());
    }
    ws.update(cx, |ws, cx| -> anyhow::Result<()> {
        match (verb, arg) {
            ("ide", _) => ws.toggle_ide(cx),
            ("route", "draft") => ws.new_thread(cx),
            ("route", "no-project") => ws.navigate(Route::Draft { project: None }, cx),
            ("route", "basecamp") => ws.navigate(Route::Basecamp, cx),
            ("route", "notes") => ws.navigate(Route::Notes, cx),
            ("route", "study") => ws.navigate(Route::Study, cx),
            ("route", "appearance") => ws.navigate(Route::Settings(SettingsPage::Appearance), cx),
            ("route", "onboarding") => ws.open_onboarding(None, cx),
            // Onboarding from a step (`onboarding agents`), or with what a set-up Mac would show
            // (`onboarding demo`: agents signed in on plans, a model server, some permissions on).
            ("onboarding", "demo") => onboarding_demo(ws, cx),
            // The Agents step with Node.js missing for Pi's npm install, being installed by Trek,
            // or failed to install (`node-missing`, `node-installing`, `node-failed`).
            ("onboarding", node) if node.starts_with("node-") => {
                use crate::workspace::NodeSetup;
                let then = trek_core::catalog::install_hint("pi")
                    .map(|h| h.here().to_string())
                    .unwrap_or_default();
                ws.node_setup = Some(match node {
                    "node-missing" => NodeSetup::Missing { then },
                    "node-installing" => NodeSetup::Installing {
                        progress: Some(trek_core::node::Progress::Downloading {
                            version: "v24.22.1".into(),
                            done: 21_000_000,
                            total: Some(50_000_000),
                        }),
                        then: Some(then),
                    },
                    "node-failed" => NodeSetup::Failed {
                        error: "Couldn't reach nodejs.org".into(),
                        then: Some(then),
                    },
                    other => anyhow::bail!(
                        "onboarding {other}: node-missing|node-installing|node-failed"
                    ),
                });
                ws.open_onboarding(crate::onboarding::Step::from_key("agents"), cx)
            }
            ("onboarding", "") => ws.open_onboarding(None, cx),
            ("onboarding", key) => {
                let Some(step) = crate::onboarding::Step::from_key(key) else {
                    anyhow::bail!(
                        "onboarding: no step {key:?} ({})",
                        crate::onboarding::Step::ALL.map(|s| s.key()).join("|")
                    );
                };
                ws.open_onboarding(Some(step), cx)
            }
            ("route", "first") => {
                let Some(id) = ws
                    .threads
                    .iter()
                    .find(|t| t.side_of.is_none() && t.parent_id.is_none())
                    .map(|t| t.id.clone())
                else {
                    anyhow::bail!("route first: no threads");
                };
                ws.navigate(Route::Thread(id), cx)
            }
            ("route", other) if other.starts_with("settings:") => {
                let Some(page) = crate::settings_view::page_named(&other["settings:".len()..])
                else {
                    anyhow::bail!("route {other}: no such settings page");
                };
                ws.navigate(Route::Settings(page), cx)
            }
            ("route", other) if other.starts_with("title:") => {
                let q = other["title:".len()..].to_lowercase();
                let Some(id) = ws
                    .threads
                    .iter()
                    .find(|t| t.title.to_lowercase().contains(&q))
                    .map(|t| t.id.clone())
                else {
                    anyhow::bail!("route {other}: no thread title matches");
                };
                ws.navigate(Route::Thread(id), cx)
            }
            ("route", other) if other.starts_with("project:") => ws.navigate(
                Route::Draft {
                    project: Some(PathBuf::from(&other["project:".len()..])),
                },
                cx,
            ),
            ("route", other) if other.starts_with("thread:") => {
                ws.navigate(Route::Thread(other["thread:".len()..].to_string()), cx)
            }
            ("route", other) => anyhow::bail!("route: unknown route {other:?}"),
            // Files in the composer: handled above, with the window.
            ("attach", _) => anyhow::bail!("attach needs a path (several: a|b)"),
            ("send", "") => anyhow::bail!("send needs a prompt"),
            ("send", text) => ws.send(text.to_string(), vec![], cx),
            ("project", "") => anyhow::bail!("project needs a folder"),
            ("project", path) => {
                let path = std::path::PathBuf::from(path);
                ws.add_project(path.clone(), cx);
                ws.navigate(
                    Route::Draft {
                        project: Some(path),
                    },
                    cx,
                );
            }
            ("push", "on") => ws.set_push(true, cx),
            ("push", "test") => ws.test_push(cx),
            ("push", "alert") => {
                let Some(id) = ws.threads.first().map(|t| t.id.clone()) else {
                    anyhow::bail!("push alert: no threads to alert on");
                };
                ws.settings.mobile.push_when = trek_core::settings::PushWhen::Always;
                ws.push_alert("Needs your approval: Run ./scripts/migrate.sh --apply", &id);
            }
            ("push", other) => anyhow::bail!("push: unknown argument {other:?} (on|test|alert)"),
            ("pair", _) => {
                ws.offer_pairing(cx);
                let Some(offer) = ws.remote.as_ref().and_then(|r| r.offer.clone()) else {
                    anyhow::bail!(
                        "pair: no pairing offer (isolated processes don't run the remote)"
                    );
                };
                if let Some(dir) = std::env::var_os("TREK_SHOT_DIR") {
                    let _ = std::fs::write(Path::new(&dir).join("pair.txt"), offer.url);
                }
            }
            ("settled", "") => ws.settled_open = !ws.settled_open,
            ("settled", "on") => ws.settled_open = true,
            ("settled", "off") => ws.settled_open = false,
            ("settled", other) => anyhow::bail!("settled: unknown argument {other:?} (on|off)"),
            ("diff", _) => {
                // The latest turn's changes on screen, in the Git tool.
                let end = ws.current_thread().map(|t| t.id.clone()).and_then(|id| {
                    let items = &ws.live.get(&id)?.items;
                    let ix = items
                        .iter()
                        .rposition(|i| matches!(i, trek_core::store::Item::TurnEnd { .. }))?;
                    Some((id, items.id_at(ix)?.to_string()))
                });
                let Some((thread, end)) = end else {
                    anyhow::bail!("diff: the current thread has no finished turn");
                };
                cx.emit(crate::workspace::WorkspaceEvent::ShowTurnDiff {
                    thread,
                    end,
                    path: None,
                });
            }
            ("new", _) => ws.new_thread(cx),
            ("glass", "on") => {
                ws.settings.appearance.glass = true;
                ws.save_settings(cx);
            }
            ("glass", "off") => {
                ws.settings.appearance.glass = false;
                ws.save_settings(cx);
            }
            ("glass", other) => anyhow::bail!("glass: unknown argument {other:?} (on|off)"),
            ("tint", t) => {
                ws.settings.appearance.glass_tint = t
                    .parse()
                    .map_err(|_| anyhow::anyhow!("tint: bad value {t:?} (0.2–0.95)"))?;
                ws.save_settings(cx);
            }
            ("theme", t @ ("night" | "paper")) => {
                let choice = if t == "paper" {
                    trek_core::settings::ThemeChoice::Paper
                } else {
                    trek_core::settings::ThemeChoice::Night
                };
                ws.settings.appearance.theme = choice;
                ws.save_settings(cx);
                cx.defer(move |cx| crate::apply_theme(choice, None, cx));
            }
            ("theme", other) => anyhow::bail!("theme: unknown theme {other:?} (night|paper)"),
            // The scripted agent under other names, for a whole window with no "Mock" in it
            // (`trek_core::catalog::DemoDress`). Put it on before the threads are made.
            ("dress", "off") => trek_core::catalog::set_demo_dress(None),
            // The thread on screen, renamed as its owner would rename it (a thread still on its
            // first turn is otherwise titled with its whole prompt, script keywords and all).
            ("title", text) => {
                let Route::Thread(id) = ws.route.clone() else {
                    anyhow::bail!("title: no thread on screen");
                };
                anyhow::ensure!(!text.trim().is_empty(), "title: needs a title");
                ws.rename(&id, text.to_string(), cx);
            }
            ("dress", names) => {
                let dress = trek_core::catalog::parse_demo_dress(names).ok_or_else(|| {
                    anyhow::anyhow!(
                        "dress: {names:?} isn't `Agent|Fast model|Deep model` (or `off`)"
                    )
                })?;
                trek_core::catalog::set_demo_dress(Some(dress));
            }
            ("pace", v) => {
                let pace: f32 = v.parse().map_err(|_| {
                    anyhow::anyhow!("pace: bad value {v:?} (0 = instant, 1 = demo speed)")
                })?;
                trek_agents::mock::set_pace(pace);
            }
            _ => anyhow::bail!("command not understood: {verb} {arg}"),
        }
        Ok(())
    })?;
    cx.refresh_windows();
    Ok(())
}

/// What onboarding shows on a Mac that's set up, for screenshots of the isolated app (which
/// looks for no agents and has no permissions of its own): two agents signed in on plans, one to
/// sign in to, Ollama with models, and some of the permissions on.
fn onboarding_demo(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    use trek_core::AgentId;
    use trek_core::detect::{Availability, DetectedAgent};
    let found = |agent: AgentId, availability: Availability, version: &str, models: &[&str]| {
        DetectedAgent {
            name: agent.display_name(),
            agent,
            path: None,
            version: Some(version.into()),
            availability,
            models: models.iter().map(|m| m.to_string()).collect(),
            install_hint: None,
        }
    };
    ws.agents = vec![
        found(
            AgentId::ClaudeCode,
            Availability::Ready,
            "2.1.287 (Claude Code)",
            &[],
        ),
        found(AgentId::Codex, Availability::Ready, "codex-cli 0.98.0", &[]),
        found(AgentId::OpenCode, Availability::NeedsLogin, "1.4.2", &[]),
        found(
            AgentId::Direct("ollama".into()),
            Availability::Ready,
            "",
            &["qwen3-coder:30b", "gpt-oss:20b", "llama3.2"],
        ),
    ];
    let status = |account: &str, plan: &str| trek_agents::AgentStatus {
        account: Some(account.into()),
        plan: Some(plan.into()),
        logged_in: true,
        billing: Some(trek_agents::Billing::Plan(Some(plan.into()))),
        ..Default::default()
    };
    ws.agent_status.insert(
        AgentId::ClaudeCode.key(),
        status("ada@example.com", "Claude Max"),
    );
    ws.agent_status.insert(
        AgentId::Codex.key(),
        status("ada@example.com", "ChatGPT Plus"),
    );
    crate::permissions::demo();
    ws.open_onboarding(Some(crate::onboarding::Step::Agents), cx);
}

/// An element id as written in a command: `name#3` is a row, ("name", 3).
/// The terminal on screen: the one with the keys, else (in the editor) the editor's, else the
/// dock's, else the tools panel's.
fn front_terminal(
    trek: &Entity<crate::root::TrekWindow>,
    in_editor: bool,
    window: &Window,
    cx: &App,
) -> Option<Entity<crate::panels::terminal::TerminalPanel>> {
    let t = trek.read(cx);
    let (ide, dock, panel) = (
        t.ide.read(cx).shown_terminal(),
        t.terminal_dock.read(cx).shown(cx),
        t.right_panel.read(cx).shown_terminal(),
    );
    let shown: Vec<_> = if in_editor {
        [ide, dock, panel]
    } else {
        [dock, panel, ide]
    }
    .into_iter()
    .flatten()
    .collect();
    shown
        .iter()
        .find(|t| t.read(cx).focus_handle().contains_focused(window, cx))
        .or(shown.first())
        .cloned()
}

/// `terminal feed`'s text as bytes for the screen: `\n` a new line (CR LF, as a shell's output
/// has it), `\r`, `\t`, `\e` an escape, `\a` the bell, `\\` a backslash. One pair of quotes
/// around it all is the manifest's, not the text's.
fn unescape(text: &str) -> String {
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push_str("\r\n"),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('a') => out.push('\x07'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// How many elements `id` names are on screen in `window` as of a fresh frame (`name*`: any
/// whose id starts with `name`).
fn shown(window: &mut Window, cx: &mut App, id: &str) -> usize {
    use gpui_kit::test::TestWindowExt as _;
    window.render_frame(cx);
    gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|s| s.visible())
        .filter(|s| match (id.strip_suffix('*'), s.path().last()) {
            (Some(prefix), Some(last)) => id_text(last).starts_with(prefix),
            (None, last) => last == Some(&element_id(id)),
            _ => false,
        })
        .count()
}

fn element_id(id: &str) -> ElementId {
    match id
        .rsplit_once('#')
        .and_then(|(n, i)| Some((n, i.parse::<u64>().ok()?)))
    {
        Some((name, ix)) => ElementId::NamedInteger(name.to_string().into(), ix),
        None => ElementId::Name(id.to_string().into()),
    }
}

/// How `elements` writes an id, the way `click` reads it back.
fn id_text(id: &ElementId) -> String {
    match id {
        ElementId::Name(n) => n.to_string(),
        ElementId::NamedInteger(n, i) => format!("{n}#{i}"),
        other => format!("{other:?}"),
    }
}

/// `usage-demo`: plans and limits for the Usage card as Claude Code, Codex and Devin report them
/// (Devin's as kept from an earlier run), without asking any real agent.
fn usage_demo(ws: &Entity<Workspace>, cx: &mut App) {
    use trek_agents::{AgentStatus, UsageLimit};
    use trek_core::AgentId;
    ws.update(cx, |ws, cx| {
        let now = ws.now();
        let limit = |label: &str, window: &str, percent: f32, hours: i64| UsageLimit {
            label: label.into(),
            percent,
            resets_at: Some(now + hours * 3_600_000 + 1_380_000),
            window: window.into(),
        };
        for (agent, name) in [
            (AgentId::ClaudeCode, "Claude Code"),
            (AgentId::Codex, "Codex"),
        ] {
            if !ws.agents.iter().any(|a| a.agent == agent) {
                let availability = trek_core::detect::Availability::Ready;
                ws.agents.push(trek_core::detect::DetectedAgent {
                    agent,
                    name: name.into(),
                    path: None,
                    version: None,
                    availability,
                    models: vec![],
                    install_hint: None,
                });
            }
        }
        let claude = vec![
            limit("5-hour limit", "5h", 42., 2),
            limit("Weekly limit", "7d", 68., 50),
            limit("Weekly · Opus", "7d", 91., 50),
        ];
        ws.agent_status.insert(
            AgentId::ClaudeCode.key(),
            AgentStatus {
                plan: Some("Claude Max".into()),
                logged_in: true,
                limits: claude,
                ..Default::default()
            },
        );
        let codex = vec![
            limit("5-hour limit", "5h", 12., 3),
            limit("Weekly limit", "7d", 35., 120),
        ];
        ws.agent_status.insert(
            AgentId::Codex.key(),
            AgentStatus {
                plan: Some("ChatGPT Plus".into()),
                logged_in: true,
                limits: codex,
                ..Default::default()
            },
        );
        let devin = crate::workspace::usage::Snapshot {
            read_at: now - 25 * 60_000,
            plan: Some("Devin Pro".into()),
            limits: vec![limit("Daily quota", "24h", 55., 6)],
            note: None,
        };
        ws.usage_cached
            .insert(crate::workspace::devin_agent().key(), devin);
        cx.notify();
    });
}

/// `accounts-demo`: `usage-demo`, and a second account of Claude Code and of Codex: one signed in
/// with its windows, one not signed in. The mock agent gets a signed-in one too, so a thread can
/// be moved between them. No real agent is asked.
fn accounts_demo(ws: &Entity<Workspace>, cx: &mut App) {
    use trek_agents::{AgentStatus, UsageLimit};
    use trek_core::AgentId;
    ws.update(cx, |ws, cx| {
        let now = ws.now();
        let limit = |label: &str, window: &str, percent: f32, hours: i64| UsageLimit {
            label: label.into(),
            percent,
            resets_at: Some(now + hours * 3_600_000 + 1_380_000),
            window: window.into(),
        };
        let mock = AgentId::Direct("mock".into());
        for (agent, label, signed_in) in [
            (AgentId::ClaudeCode, "Work", true),
            (AgentId::Codex, "Personal", false),
            (mock.clone(), "Work", true),
        ] {
            let Some(id) = ws.add_account(&agent, cx) else {
                continue;
            };
            ws.rename_account(&id, label, cx);
            if signed_in {
                ws.accounts.status.insert(
                    id,
                    AgentStatus {
                        account: Some("work@example.com".into()),
                        plan: Some("Claude Pro".into()),
                        logged_in: true,
                        limits: vec![
                            limit("5-hour limit", "5h", 8., 4),
                            limit("Weekly limit", "7d", 22., 96),
                        ],
                        ..Default::default()
                    },
                );
            }
        }
        cx.notify();
    });
}

/// `pr-demo <state>`: a PR/release card with made-up contents appended to the thread on
/// screen (`pr_card::demo`), so its states can be shot without a real `gh`. States: open,
/// creating, pushing, opening, fetching, checking, conflicted, resolving, resolved, ready,
/// merging, merged, closed, failed, release, release-published, release-failed, run,
/// run-running, run-passed, run-failed.
fn pr_demo(ws: &Entity<Workspace>, arg: &str, cx: &mut App) {
    ws.update(cx, |ws, cx| {
        let Some(id) = ws
            .thread_id_in(&crate::workspace::Scope::Main)
            .map(str::to_string)
        else {
            return;
        };
        let card = crate::pr_card::demo(arg);
        let mut fx = crate::pr_card::Effects::default();
        if let Some(live) = ws.live.get_mut(&id) {
            let key = format!("demo-{}", arg);
            live.items.push(trek_core::store::Item::Tool {
                id: crate::pr_card::row_id(&key),
                title: "Pull request".into(),
                detail: serde_json::to_string(&card).unwrap_or_default(),
                output: String::new(),
                status: trek_core::store::ToolStatus::Done,
            });
            live.revision += 1;
            if let Some(item) = live.items.id_at(live.items.len() - 1) {
                fx.changed.push(item.to_string());
            }
        }
        for item in fx.changed {
            cx.emit(crate::workspace::WorkspaceEvent::CardChanged {
                id: id.to_string(),
                item,
            });
        }
        cx.emit(crate::workspace::WorkspaceEvent::Transcript {
            id: id.to_string(),
            appended: false,
        });
        // The composer's status chip reads the newest card: redraw it too.
        cx.notify();
    });
}

/// `import-demo`: a made-up home folder in the disposable data folder, with what Conductor and T3
/// Code leave on disk (real git worktrees, a database with Conductor's tables, Claude Code
/// sessions), which Settings › Import Threads and onboarding then offer. Bringing it over runs the
/// real thing, on that folder alone.
fn import_demo(ws: &Entity<Workspace>, cx: &mut App) -> anyhow::Result<()> {
    use trek_core::worktree::git;
    let home = trek_core::paths::data_dir().join("import-demo");
    if home.exists() {
        std::fs::remove_dir_all(&home)?;
    }
    let repo = |name: &str| -> anyhow::Result<std::path::PathBuf> {
        let path = home.join("code").join(name);
        std::fs::create_dir_all(&path)?;
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "demo@example.com"],
            &["config", "user.name", "Demo"],
            &["commit", "-q", "--allow-empty", "-m", "First"],
        ] {
            git(&path, args)?;
        }
        Ok(path)
    };
    let worktree =
        |repo: &std::path::Path, at: &str, branch: &str| -> anyhow::Result<std::path::PathBuf> {
            let path = home.join(at);
            std::fs::create_dir_all(path.parent().unwrap_or(&home))?;
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    &path.to_string_lossy(),
                ],
            )?;
            Ok(std::fs::canonicalize(&path)?)
        };
    let session = |id: &str, cwd: &std::path::Path, title: &str| -> anyhow::Result<()> {
        let name: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let folder = home.join(".claude/projects").join(name);
        std::fs::create_dir_all(&folder)?;
        let line = |role: &str, text: &str| {
            serde_json::json!({
                "type": role, "cwd": cwd, "sessionId": id, "entrypoint": "cli",
                "uuid": uuid::Uuid::now_v7().to_string(),
                "timestamp": "2026-11-03T09:30:00Z",
                "message": { "role": role, "content": text },
            })
            .to_string()
        };
        std::fs::write(
            folder.join(format!("{id}.jsonl")),
            format!(
                "{}\n{}\n{}\n{}\n",
                line("user", title),
                line("assistant", "Done."),
                line("user", "And the tests"),
                line("assistant", "They pass."),
            ),
        )?;
        Ok(())
    };
    let acme = repo("acme-web")?;
    let api = repo("billing-api")?;
    let kyoto = worktree(
        &acme,
        "conductor/workspaces/acme-web/kyoto",
        "tobias/refresh-tokens",
    )?;
    let osaka = worktree(
        &acme,
        "conductor/workspaces/acme-web/osaka",
        "tobias/nav-tidy",
    )?;
    worktree(
        &acme,
        "conductor/workspaces/acme-web/nara",
        "tobias/onboarding-copy",
    )?;
    let lima = worktree(
        &api,
        "conductor/workspaces/billing-api/lima",
        "tobias/invoice-export",
    )?;
    session(
        "9a1f6f3e-0b6f-4a9e-8d11-1a2b3c4d5e01",
        &kyoto,
        "Refresh the login token before it expires",
    )?;
    session(
        "9a1f6f3e-0b6f-4a9e-8d11-1a2b3c4d5e02",
        &kyoto,
        "Add a test for the expired token path",
    )?;
    session(
        "9a1f6f3e-0b6f-4a9e-8d11-1a2b3c4d5e03",
        &lima,
        "Export invoices as CSV",
    )?;
    session(
        "9a1f6f3e-0b6f-4a9e-8d11-1a2b3c4d5e04",
        &osaka,
        "Tidy the navigation",
    )?;
    let db = trek_core::import::worktree_apps::conductor_db(&home);
    std::fs::create_dir_all(db.parent().unwrap_or(&home))?;
    let sql = format!(
        "CREATE TABLE repos (id TEXT, name TEXT, default_branch TEXT, root_path TEXT, storage_version INTEGER);
         CREATE TABLE workspaces (repository_id TEXT, directory_name TEXT, branch TEXT, state TEXT, pr_title TEXT, workspace_name TEXT, intended_target_branch TEXT, hosting_server_url TEXT);
         CREATE TABLE sessions (agent_type TEXT, claude_session_id TEXT, is_hidden INTEGER);
         CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO repos VALUES ('a', 'acme-web', 'main', '{}', 3), ('b', 'billing-api', 'main', '{}', 3);
         INSERT INTO workspaces VALUES
           ('a', 'kyoto', 'tobias/refresh-tokens', 'ready', 'Refresh login tokens', NULL, 'main', NULL),
           ('a', 'nara', 'tobias/onboarding-copy', 'ready', NULL, 'Rewrite the onboarding copy', 'main', NULL),
           ('a', 'kobe', 'tobias/old-thing', 'archived', NULL, NULL, 'main', NULL),
           ('b', 'tokyo', 'tobias/cloud', 'ready', NULL, NULL, 'main', 'https://cloud.example');
         INSERT INTO sessions VALUES ('cursor', 'c1', 0);",
        acme.display(),
        api.display(),
    );
    // With the SQLite Trek links, not the sqlite3 program: a Mac has one, Windows doesn't.
    rusqlite::Connection::open(&db)?.execute_batch(&sql)?;
    let t3 = repo("notes-app")?;
    worktree(
        &t3,
        ".t3/worktrees/notes-app/calm-river",
        "t3code/calm-river",
    )?;
    // As git writes it, so paths under it read as `~/…`.
    let home = std::fs::canonicalize(&home)?;
    ws.update(cx, |ws, cx| {
        ws.worktree_apps.fixture_home = Some(home);
        ws.worktree_apps.plans.clear();
        ws.scan_worktree_apps(cx);
    });
    Ok(())
}

/// `checks-demo <state>`: the thread on screen has a pull request whose checks say `state`, from a
/// made-up `gh` in the disposable data folder, so the real look, the row, the bar, the card and
/// Send to agent can be seen with nothing going to GitHub. States: failed, failed-more (another
/// check fails on the commit), again (a new commit fails, each time), pending, passing, auto (the project
/// sends by itself), off (it doesn't), offline.
fn checks_demo(ws: &Entity<Workspace>, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    let dir = trek_core::paths::data_dir().join("checks-demo");
    std::fs::create_dir_all(&dir)?;
    // A batch file on Windows, which starts no `#!` script: Rust runs a `.cmd` through cmd.exe,
    // its arguments quoted for it.
    let gh = dir.join(if cfg!(windows) { "gh.cmd" } else { "gh" });
    let arg = arg.trim();
    // A new commit each time `again` is asked for: each is a failure of its own.
    let fresh = format!("{:07x}", crate::pr_card::now_ms() % 0x0fff_ffff);
    let oid_after = if arg == "again" {
        fresh.as_str()
    } else {
        "c0ffee0"
    };
    let check = |name: &str, conclusion: &str, job: u32| {
        serde_json::json!({
            "__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": conclusion,
            "detailsUrl": format!("https://github.com/acme/web/actions/runs/1001/job/{job}"),
        })
    };
    let running = serde_json::json!({
        "__typename": "CheckRun", "name": "test (macos)", "status": "IN_PROGRESS", "conclusion": "",
        "detailsUrl": "https://github.com/acme/web/actions/runs/1002/job/3",
    });
    let (oid, checks) = match arg {
        "failed-more" => (
            "abc1234",
            vec![
                check("clippy", "FAILURE", 1),
                check("test (macos)", "FAILURE", 2),
            ],
        ),
        "again" => (
            oid_after,
            vec![
                check("clippy", "FAILURE", 1),
                check("test (macos)", "SUCCESS", 2),
            ],
        ),
        "pending" => (oid_after, vec![running]),
        "passing" => (
            oid_after,
            vec![
                check("clippy", "SUCCESS", 1),
                check("test (macos)", "SUCCESS", 2),
            ],
        ),
        _ => (
            "abc1234",
            vec![
                check("clippy", "FAILURE", 1),
                check("test (macos)", "SUCCESS", 2),
            ],
        ),
    };
    let view = serde_json::json!({
        "number": 128, "title": "Refresh sessions in the auth middleware",
        "url": "https://github.com/acme/web/pull/128", "state": "OPEN",
        "mergeable": "MERGEABLE", "mergeStateStatus": "UNSTABLE",
        "headRefName": "feat/refresh-sessions", "baseRefName": "main", "headRefOid": oid,
        "additions": 42, "deletions": 7, "statusCheckRollup": checks,
        "commits": [],
    });
    std::fs::write(dir.join("view.json"), view.to_string())?;
    std::fs::write(
        dir.join("log.txt"),
        "clippy\tRun clippy\t2026-11-05T10:00:00.1234567Z ##[group]Run cargo clippy -- -D warnings\n         clippy\tRun clippy\t2026-11-05T10:00:41.1234567Z error: this `if` has identical blocks\n         clippy\tRun clippy\t2026-11-05T10:00:41.2234567Z   --> crates/auth/src/session.rs:88:5\n         clippy\tRun clippy\t2026-11-05T10:00:41.3234567Z error: could not compile `auth` (lib)\n         clippy\tRun clippy\t2026-11-05T10:00:41.4234567Z ##[error]Process completed with exit code 101.\n",
    )?;
    std::fs::write(
        dir.join("rate_limit.json"),
        r#"{"resources":{"graphql":{"limit":5000,"remaining":4900,"reset":9999999999},"core":{"limit":5000,"remaining":4900,"reset":9999999999}}}"#,
    )?;
    let script = if cfg!(windows) {
        format!(
            "@echo off\r\nset \"dir={}\"\r\nif exist \"%dir%\\offline\" (\r\n  echo error connecting to api.github.com 1>&2\r\n  exit /b 1\r\n)\r\nif \"%~1 %~2\"==\"pr view\" type \"%dir%\\view.json\"\r\nif \"%~1 %~2\"==\"run view\" type \"%dir%\\log.txt\"\r\nif \"%~1 %~2\"==\"api rate_limit\" type \"%dir%\\rate_limit.json\"\r\n",
            dir.display()
        )
    } else {
        format!(
            "#!/bin/sh\ndir='{}'\nif [ -e \"$dir/offline\" ]; then echo 'error connecting to api.github.com' >&2; exit 1; fi\ncase \"$1 $2\" in\n  'pr view') cat \"$dir/view.json\" ;;\n  'run view') cat \"$dir/log.txt\" ;;\n  'api rate_limit') cat \"$dir/rate_limit.json\" ;;\nesac\n",
            dir.display()
        )
    };
    std::fs::write(&gh, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))?;
    }
    let offline = dir.join("offline");
    if arg == "offline" {
        std::fs::write(&offline, "")?;
    } else {
        let _ = std::fs::remove_file(&offline);
    }
    ws.update(cx, |ws, cx| {
        ws.pr_checks.set_gh_path(Some(gh.clone()));
        let Some(id) = ws
            .thread_id_in(&crate::workspace::Scope::Main)
            .map(str::to_string)
        else {
            return;
        };
        if matches!(arg, "auto" | "off") {
            if let Some(project) = ws.thread(&id).and_then(|t| ws.project_dir(t)) {
                ws.update_project_prefs(&project, |p| p.send_failed_checks = arg == "auto", cx);
            }
        }
        // A card in the transcript, as `gh pr create` leaves it, the first time.
        let url = "https://github.com/acme/web/pull/128";
        let row = crate::pr_card::row_id(url);
        if let Some(live) = ws.live.get_mut(&id) {
            let has = live
                .items
                .iter()
                .any(|i| matches!(i, trek_core::store::Item::Tool { id, .. } if *id == row));
            if !has {
                let card = crate::pr_card::Card {
                    kind: crate::pr_card::Kind::Pr,
                    title: "Refresh sessions in the auth middleware".into(),
                    url: Some(url.into()),
                    number: Some(128),
                    head: "feat/refresh-sessions".into(),
                    base: "main".into(),
                    repo_dir: dir.display().to_string(),
                    phase: crate::pr_card::Phase::Open,
                    opened: true,
                    created_ms: crate::pr_card::now_ms(),
                    ..Default::default()
                };
                live.items.push(trek_core::store::Item::Tool {
                    id: row.clone(),
                    title: "Pull request".into(),
                    detail: serde_json::to_string(&card).unwrap_or_default(),
                    output: String::new(),
                    status: trek_core::store::ToolStatus::Done,
                });
                live.revision += 1;
                let card = card.clone();
                ws.pr_checks_saw(&id, &row, &card, cx);
            }
        }
        ws.pr_checks_look_now(&id, url, cx);
        cx.emit(crate::workspace::WorkspaceEvent::Transcript {
            id: id.to_string(),
            appended: false,
        });
        cx.notify();
    });
    Ok(())
}

/// `update-demo ready|wait|later|clear`: an update staged, as this build couldn't get one any other
/// way (a dev build can't update itself), to look at what the updater says with work running. The
/// staged path names nothing: Install now only fails, as this isn't an app bundle anyway.
fn update_demo(ws: &Entity<Workspace>, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    use crate::workspace::{UpdateAction, UpdateStatus};
    ws.update(cx, |ws, cx| {
        ws.blocked = Some(None);
        let staged = trek_core::paths::data_dir().join("no-such-update/Trek.app");
        let ready = UpdateStatus::Ready {
            version: "9.9.9".into(),
            staged,
        };
        match arg.trim() {
            "ready" => {
                ws.updater.later = None;
                ws.updater.status = ready;
            }
            "wait" => {
                ws.updater.later = None;
                ws.updater.status = ready;
                ws.run_update_action(UpdateAction::Restart, cx);
            }
            "later" => {
                ws.updater.status = ready;
                ws.run_update_action(UpdateAction::Later, cx);
            }
            "clear" => {
                ws.updater.later = None;
                ws.updater.status = UpdateStatus::Idle;
            }
            other => anyhow::bail!("update-demo: unknown state {other:?} (ready|wait|later|clear)"),
        }
        cx.notify();
        Ok(())
    })
}

/// `secret-demo`: a secret's request card on the thread on screen, as an agent's `request_secret`
/// would put it up (the mock has no MCP tools to call). What's typed goes to the disposable
/// profile's own data folder.
fn secret_demo(ws: &Entity<Workspace>, cx: &mut App) -> anyhow::Result<()> {
    ws.update(cx, |ws, cx| {
        let Route::Thread(id) = ws.route.clone() else {
            anyhow::bail!("secret-demo: no thread on screen");
        };
        let (reply, _nobody) = async_channel::bounded(1);
        ws.ask_for_secret(
            &id,
            "GITHUB_TOKEN",
            "To open the pull request for this branch.",
            reply,
            cx,
        );
        Ok(())
    })
}

/// `privacy-demo card|feedback|nokey|failed|sent`: the card asking about usage data, which a
/// capture run never puts up by itself, or the feedback sheet as a build with a PostHog key draws
/// it (`feedback`), one without (`nokey`), after a failed send, or once sent. Nothing is sent: a
/// capture run is a test profile.
fn privacy_demo(ws: &Entity<Workspace>, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    use crate::feedback::Stage;
    use trek_telemetry::NotSent;
    let main = ws
        .read(cx)
        .main_window
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let ws = ws.clone();
    let arg = arg.trim().to_string();
    main.update(cx, |_, window, cx| match arg.as_str() {
        "" | "card" => {
            crate::privacy::open_card(window, cx);
            Ok(())
        }
        "feedback" | "nokey" | "failed" | "sent" => {
            let sheet = crate::feedback::open(ws, window, cx);
            sheet.update(cx, |s, cx| {
                s.set_message(
                    "The sidebar froze after I opened a long thread.",
                    window,
                    cx,
                );
                s.destination = match arg.as_str() {
                    "nokey" => Err(NotSent::NoKey),
                    _ => Ok(trek_telemetry::config::DEFAULT_HOST
                        .trim_start_matches("https://")
                        .to_string()),
                };
                s.stage = match arg.as_str() {
                    "failed" => Stage::NotSent(NotSent::Failed("no connection".into())),
                    "sent" => Stage::Sent,
                    _ => Stage::Writing,
                };
                cx.notify();
            });
            Ok(())
        }
        _ => Err(anyhow::anyhow!(
            "privacy-demo card|feedback|nokey|failed|sent"
        )),
    })?
}

/// `resume-demo resuming|failed|offered|waiting`: the thread on screen's offer in that state, for
/// the states a mock turn passes through too fast to catch. In memory only.
fn resume_demo(ws: &Entity<Workspace>, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    use crate::workspace::resume::{Offer, ResumeState};
    ws.update(cx, |ws, cx| {
        let Route::Thread(id) = ws.route.clone() else {
            anyhow::bail!("resume-demo: no thread on screen");
        };
        let (state, waiting) = match arg.trim() {
            "resuming" => (ResumeState::Resuming, false),
            "failed" => (
                ResumeState::Failed("Claude Code isn't signed in.".into()),
                false,
            ),
            "offered" => (ResumeState::Offered, false),
            "waiting" => (ResumeState::Offered, true),
            other => {
                anyhow::bail!(
                    "resume-demo: unknown state {other:?} (resuming|failed|offered|waiting)"
                )
            }
        };
        ws.interrupted.insert(
            id.clone(),
            Offer {
                turn: trek_core::interrupted::Interrupted {
                    thread: id,
                    waiting,
                    asked: waiting.then(|| "Bash npm publish --tag latest".to_string()),
                    at: ws.now(),
                    tries: 0,
                },
                state,
            },
        );
        cx.notify();
        Ok(())
    })
}

/// `compact-demo`: the thread on screen as if it were large, billed per token and left alone
/// long enough for its prompt cache to go: the composer's Compact chip, with no wait and no bill.
fn compact_demo(ws: &Entity<Workspace>, cx: &mut App) {
    ws.update(cx, |ws, cx| {
        let Some(id) = ws
            .thread_id_in(&crate::workspace::Scope::Main)
            .map(str::to_string)
        else {
            return;
        };
        let live = ws.live.entry(id).or_default();
        live.context = Some((124_000, 200_000));
        live.billing = Some(trek_agents::Billing::Metered);
        let later = 2 * 60 * 60 * 1_000;
        ws.clock = crate::workspace::Clock::new(move || trek_core::store::now_ms() + later);
        cx.notify();
    });
}

/// `limits-demo`: a plan's usage windows for the mock agent the isolated app runs threads on, as
/// Claude Code reports them, so the composer's context card has limits to draw (and hover) without
/// asking any real agent.
fn limits_demo(ws: &Entity<Workspace>, cx: &mut App) {
    use trek_agents::{AgentStatus, CommandKind, SlashCommand, UsageLimit};
    ws.update(cx, |ws, cx| {
        let now = ws.now();
        let limit = |label: &str, window: &str, percent: f32, hours: i64| UsageLimit {
            label: label.into(),
            percent,
            resets_at: Some(now + hours * 3_600_000 + 1_380_000),
            window: window.into(),
        };
        ws.agent_status.insert(
            trek_core::AgentId::Direct(trek_core::catalog::MOCK_PROVIDER.into()).key(),
            AgentStatus {
                plan: Some("Max".into()),
                logged_in: true,
                limits: vec![
                    limit("5-hour limit", "5h", 68., 2),
                    limit("Weekly limit", "7d", 24., 50),
                    limit("Weekly · Opus", "7d", 17., 50),
                    limit("Weekly · Sonnet", "7d", 7., 50),
                ],
                // A few skills, so the context card has their share to estimate too.
                commands: ["verify-trek", "trek-bug-finder", "emil-design-eng", "higs"]
                    .into_iter()
                    .map(|name| SlashCommand {
                        name: name.into(),
                        description: String::new(),
                        kind: CommandKind::Skill,
                    })
                    .collect(),
                ..Default::default()
            },
        );
        // Three servers for the MCP row's breakdown, and a context report plus a long
        // transcript for the card to divide up: messages the bulk, as a real thread has.
        if ws.settings.tools.mcp_servers.is_empty() {
            for name in ["Figma", "Linear", "GitHub"] {
                ws.settings
                    .tools
                    .mcp_servers
                    .push(trek_core::settings::McpServerConfig::stdio(
                        name,
                        "npx",
                        vec![format!("mcp-{name}")],
                    ));
            }
        }
        if let Some(id) = ws
            .thread_id_in(&crate::workspace::Scope::Main)
            .map(str::to_string)
        {
            let live = ws.live.entry(id).or_default();
            live.context = Some((120_400, 200_000));
            if live.items.is_empty() {
                for _ in 0..30 {
                    live.items.push(trek_core::store::Item::Assistant {
                        text: "The agent's answer, worked through step by step. ".repeat(200),
                    });
                }
            }
        }
        cx.notify();
    });
}

/// A manifest's `key` keystroke as this platform means it: `cmd`/`win`/`super` name the Mac's ⌘,
/// which Trek's bindings read as Ctrl off the Mac — gpui's `secondary` is exactly that mapping.
/// Anything else (a bare `ctrl-`, `shift-`, a key) passes through.
fn manifest_key(key: &str) -> String {
    let mut parts: Vec<&str> = key.split('-').collect();
    let modifiers = parts.len().saturating_sub(1);
    for part in parts.iter_mut().take(modifiers) {
        if matches!(part.to_ascii_lowercase().as_str(), "cmd" | "win" | "super") {
            *part = "secondary";
        }
    }
    parts.join("-")
}

/// `click|rclick|hover <id>`: the pointer on the one element on screen with that id, as events
/// dispatched to the window (no real input). The kit's lookup panics on a missing or ambiguous id,
/// so both are checked first.
fn pointer(window: &mut Window, verb: &str, id: &str, cx: &mut App) -> anyhow::Result<()> {
    use gpui_kit::test::TestWindowExt as _;
    anyhow::ensure!(
        !id.is_empty(),
        "{verb} needs an element id (see `elements`)"
    );
    POINTER_SCRIPTED.store(true, Ordering::Relaxed);
    window.render_frame(cx);
    let id = element_id(id);
    let found: Vec<_> = gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|s| s.path().last() == Some(&id))
        .collect();
    match found.as_slice() {
        [] => anyhow::bail!("{verb}: nothing on screen is {}", id_text(&id)),
        [one] if !one.visible() => {
            anyhow::bail!("{verb}: {} is hidden or scrolled away", id_text(&id))
        }
        [_] => {}
        many => anyhow::bail!(
            "{verb}: {} elements are {}: {:?}",
            many.len(),
            id_text(&id),
            many.iter().map(|s| s.path().to_vec()).collect::<Vec<_>>()
        ),
    }
    match verb {
        "rclick" => window.right_click(id, cx),
        "hover" => window.hover(id, cx),
        _ => window.click(id, cx),
    }
    Ok(())
}

/// `type <text>`: into the focused field as typed text, or into the main composer when no field
/// has the keyboard.
fn type_text(root: AnyView, window: &mut Window, text: &str, cx: &mut App) -> anyhow::Result<()> {
    let typed = window.focused(cx).is_some()
        && window.dispatch_keystroke(
            Keystroke {
                modifiers: Modifiers::default(),
                key: String::new(),
                key_char: Some(text.to_string()),
            },
            cx,
        );
    if typed {
        return Ok(());
    }
    let view = root
        .downcast::<gpui_kit::component::Root>()
        .ok()
        .map(|r| r.read(cx).view().clone());
    let trek = view
        .and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())
        .ok_or_else(|| anyhow::anyhow!("type: no Trek window"))?;
    let composer = trek.read(cx).composer.clone();
    composer.update(cx, |c, cx| {
        c.focus(window, cx);
        c.insert_text(text, window, cx);
    });
    Ok(())
}

/// `elements`: every visible element `click` can name, one per line, to `elements.txt` — its id,
/// then its role and label when it has them, then where it is.
fn elements(window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    use gpui_kit::test::TestWindowExt as _;
    window.render_frame(cx);
    let mut lines: Vec<String> = gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|s| s.visible())
        .filter_map(|s| {
            let id = id_text(s.path().last()?);
            let b = s.bounds();
            let role = s.role().map(|r| format!(" {r:?}")).unwrap_or_default();
            let label = s.label().map(|l| format!(" {l:?}")).unwrap_or_default();
            Some(format!(
                "{id}{role}{label} @ {:.0},{:.0} {:.0}×{:.0}",
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height)
            ))
        })
        .collect();
    lines.sort();
    let dir = std::env::var_os("TREK_SHOT_DIR")
        .ok_or_else(|| anyhow::anyhow!("elements: no TREK_SHOT_DIR"))?;
    std::fs::write(
        Path::new(&dir).join("elements.txt"),
        lines.join("\n") + "\n",
    )?;
    Ok(())
}

/// `card`, for tests: what `control approve|deny|answer` does.
#[cfg(any(test, feature = "test-support"))]
pub fn control_card(
    ws: &mut Workspace,
    verb: &str,
    arg: &str,
    cx: &mut Context<Workspace>,
) -> anyhow::Result<()> {
    card(ws, verb, arg, cx)
}

/// `approve`, `deny`, `answer <text|n>`: the thread on screen's waiting card, through what its
/// buttons call. `approve` allows a permission or starts a plan; `deny` is Deny, Keep planning or
/// Skip; `answer <n>` picks option n (from 1) of every question and sends, `answer <text>` is the
/// answer typed in the composer.
fn card(
    ws: &mut Workspace,
    verb: &str,
    arg: &str,
    cx: &mut Context<Workspace>,
) -> anyhow::Result<()> {
    use trek_agents::{Decision, Prompt};
    let id = ws
        .focused_thread()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{verb}: no thread on screen"))?;
    let p = ws
        .live
        .get(&id)
        .and_then(|l| l.permissions.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{verb}: nothing is waiting on the thread"))?;
    match (verb, &p.prompt) {
        ("approve", None) => ws.respond(&id, &p.request_id, Decision::Allow, cx),
        ("approve", Some(Prompt::Plan(_))) => ws.approve_plan(&id, &p.request_id, cx),
        ("approve", Some(Prompt::Questions(_))) => {
            anyhow::bail!("approve: it's a question (answer <text|n>, or deny to skip)")
        }
        // A secret only ever goes in through its card, never a command: there's no yes to give.
        ("approve" | "answer", Some(Prompt::Secret { .. })) => {
            anyhow::bail!("{verb}: it's asking for a secret (type it in the card, or deny)")
        }
        ("deny", _) => ws.respond(&id, &p.request_id, Decision::Deny, cx),
        ("answer", Some(Prompt::Questions(questions))) => {
            anyhow::ensure!(!arg.is_empty(), "answer needs <text> or an option number");
            match arg.parse::<usize>() {
                Ok(n) => {
                    let mut answers = vec![];
                    for (qi, q) in questions.iter().enumerate() {
                        let (label, _) = q.options.get(n.wrapping_sub(1)).ok_or_else(|| {
                            anyhow::anyhow!(
                                "answer {n}: “{}” has {} options",
                                q.question,
                                q.options.len()
                            )
                        })?;
                        if let Some(live) = ws.live.get_mut(&id) {
                            live.picks
                                .insert((p.request_id.clone(), qi), vec![label.clone()]);
                        }
                        answers.push((q.question.clone(), label.clone()));
                    }
                    ws.answer(&id, &p.request_id, answers, cx);
                }
                Err(_) => {
                    let scope = ws.focused_scope();
                    ws.send_in(&scope, arg.to_string(), vec![], cx);
                }
            }
        }
        ("answer", _) => anyhow::bail!(
            "answer: the thread is waiting on a {}, not a question (approve or deny)",
            if p.prompt.is_some() {
                "plan"
            } else {
                "permission"
            }
        ),
        _ => unreachable!(),
    }
    Ok(())
}

/// The see-through part of a frame is laid over `$TREK_SHOT_UNDER` (else a dusky gradient),
/// blurred once per (source, size) — recording calls [`save`] for every frame, so the 30-sigma
/// blur must not run per frame.
static UNDERLAY: LazyLock<Mutex<Option<Underlay>>> = LazyLock::new(|| Mutex::new(None));

struct Underlay {
    source: Option<PathBuf>,
    w: u32,
    h: u32,
    image: image::RgbaImage,
}

fn underlay(w: u32, h: u32) -> image::RgbaImage {
    let source = std::env::var_os("TREK_SHOT_UNDER").map(PathBuf::from);
    let mut cached = UNDERLAY.lock().unwrap();
    if let Some(u) = cached.as_ref() {
        if u.w == w && u.h == h && u.source == source {
            return u.image.clone();
        }
    }
    let raw = source
        .as_ref()
        .and_then(|p| image::open(p).ok())
        .map(|i| {
            image::imageops::resize(&i.to_rgba8(), w, h, image::imageops::FilterType::Triangle)
        })
        .unwrap_or_else(|| {
            image::RgbaImage::from_fn(w, h, |x, y| {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                image::Rgba([
                    (40. + 160. * fx) as u8,
                    (70. + 60. * fy) as u8,
                    (150. - 60. * fx) as u8,
                    255,
                ])
            })
        });
    let image = image::imageops::blur(&raw, 30.);
    *cached = Some(Underlay {
        source,
        w,
        h,
        image: image.clone(),
    });
    image
}

/// Write `image`, with its see-through parts over the blurred `$TREK_SHOT_UNDER` (else a
/// dusky gradient), as the desktop would show through glass.
fn save(mut image: image::RgbaImage, path: &Path) -> anyhow::Result<()> {
    let (w, h) = image.dimensions();
    if image.pixels().any(|p| p[3] < 255) {
        let under = underlay(w, h);
        for (x, y, p) in image.enumerate_pixels_mut() {
            if p[3] == 255 {
                continue;
            }
            let a = p[3] as f32 / 255.;
            let u = under.get_pixel(x, y);
            for c in 0..3 {
                // The frame is premultiplied by alpha.
                p[c] = (p[c] as f32 + u[c] as f32 * (1. - a)).min(255.) as u8;
            }
            p[3] = 255;
        }
    }
    image.save(path)?;
    Ok(())
}
