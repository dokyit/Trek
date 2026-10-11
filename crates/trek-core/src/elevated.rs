//! Commands run as an administrator, on Windows: the package managers whose installs are the
//! machine's (Chocolatey's, a global Scoop's, WinGet's machine-wide packages) upgrade only
//! elevated. Trek is never elevated itself. It writes the commands into one batch script and asks
//! Windows to run that as an administrator (`ShellExecuteExW` with the `runas` verb): the UAC
//! prompt is Windows' own, one for the whole script however many commands it holds. Each
//! command's output and exit code land in files beside the script, read once it's done.
//!
//! The elevated process doesn't inherit Trek's environment (Windows starts it afresh from the
//! user's profile), so the script sets the PATH and variables each command runs with itself.
//!
//! What goes into the script is quoted for cmd.exe, or refused: it runs as an administrator, so
//! nothing in it may be read as anything but the program and its arguments. Each is put in double
//! quotes, where cmd.exe takes `&`, `|`, `<`, `>`, `(`, `)` and spaces literally, and delayed
//! expansion is off, so `!` is too. A `%` is doubled (a batch file's literal percent), or doubled
//! twice for a `.cmd` or `.bat` program, which runs through `call` and its second expansion. What
//! can't be quoted is refused: a `"`, a line break or NUL anywhere, a `^` in a `call` (which
//! doubles carets even in quotes), and an argument ending in `\` (which an `.exe`'s argument
//! parser would read as escaping the closing quote).
//!
//! The script is read by cmd.exe after the user has said yes, from a folder the user's other
//! programs can write to, so the folder is Trek's alone for the run (`Run`): a name nothing can
//! guess, made new; the script and every file the script writes made by Trek first and held open
//! so that nothing can rewrite, replace or delete them while the prompt is up or the script runs;
//! and the script read back, after that, to be what Trek wrote. The elevated process runs from
//! System32, not that folder, and names every program it runs by its full path.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

/// The longest line cmd.exe reads from a batch file is 8191 characters; a line of ours is held to
/// less, or refused.
const MAX_LINE: usize = 8000;

/// The longest PATH a batch file can set: cmd.exe reads and expands a line to 8191 characters at
/// most, and longer is cut without a word (adding to it line by line, with delayed expansion,
/// is cut at the same place).
const PATH_LIMIT: usize = 7500;

/// The window the prompt is owned by (an `HWND`, 0 for none): Trek's main window, so Windows
/// brings its prompt in front of it, not somewhere behind. Set by the app as it opens the window;
/// checked to still be a window when the prompt is asked for.
static OWNER_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Say which window owns the prompts Windows asks for (`0` when there's none).
pub fn set_owner_window(hwnd: isize) {
    OWNER_WINDOW.store(hwnd, Ordering::Relaxed);
}

/// One command of the script.
#[derive(Debug, Clone)]
pub struct Step {
    /// A full path: the elevated process has no PATH of Trek's to look a bare name up on.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The PATH it runs with (what its own children find programs on).
    pub path: String,
}

/// What a step's output and exit code are written to, in the script's folder.
pub fn output_file(dir: &Path, step: usize) -> PathBuf {
    dir.join(format!("{step}.out"))
}

pub fn code_file(dir: &Path, step: usize) -> PathBuf {
    dir.join(format!("{step}.code"))
}

/// The script, run from `dir`: `steps` one after the other, step `i`'s output (stdout and stderr)
/// into `output_file(dir, i)` and its exit code into `code_file(dir, i)`. A step that can't be
/// quoted safely refuses the whole script, as does a folder that can't be.
pub fn script(dir: &Path, steps: &[Step]) -> Result<String, String> {
    folder_fit_for_cmd(dir)?;
    let mut lines = vec![
        "@echo off".to_string(),
        // Lines after this one are read as UTF-8 (a path with an accent), and .NET programs
        // (Chocolatey) write their output in it. By its full path: a bare name is looked up in the
        // current folder first.
        format!("{} 65001 >nul", quote(&system32().join("chcp.com").to_string_lossy(), false, true)?),
        "setlocal DisableDelayedExpansion".to_string(),
        "set \"NO_COLOR=1\"".to_string(),
    ];
    for (i, step) in steps.iter().enumerate() {
        let called = is_batch(&step.program);
        let program = quote(&step.program.to_string_lossy(), called, true)?;
        let args = step.args.iter().map(|a| quote(a, called, false)).collect::<Result<Vec<_>, _>>()?;
        lines.push("setlocal".into());
        // PATH entries that can't be put in quotes are left out, not the PATH.
        let path: Vec<String> = std::env::split_paths(&step.path).filter_map(|p| set_value(&p.to_string_lossy()).ok()).filter(|p| !p.is_empty()).collect();
        lines.push(format!("set \"PATH={}\"", fit_path(path).join(";")));
        for (name, value) in &step.env {
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(format!("{name:?} can't be set safely in cmd.exe"));
            }
            lines.push(format!("set \"{name}={}\"", set_value(value)?));
        }
        let run = std::iter::once(program).chain(args).collect::<Vec<_>>().join(" ");
        let out = output_file(dir, i).display().to_string();
        let code = code_file(dir, i).display().to_string();
        lines.push(format!("{}{run} <nul >\"{out}\" 2>&1", if called { "call " } else { "" }));
        // Redirection first: `echo 0>file` would read the `0` as a handle.
        lines.push(format!(">\"{code}\" echo %ERRORLEVEL%"));
        lines.push("endlocal".into());
    }
    lines.push("exit /b 0".into());
    if let Some(long) = lines.iter().find(|l| l.len() > MAX_LINE) {
        return Err(format!("a line of the script is {} characters long, more than cmd.exe reads ({}…)", long.len(), long.chars().take(60).collect::<String>()));
    }
    Ok(lines.join("\r\n") + "\r\n")
}

/// `entries` (the folders of a PATH) as short as a batch file can set: each once (a PATH often
/// lists the same folder twice, by another case), and if that is still too long, the last of
/// the others dropped until it fits, never Windows' own folders, which its programs need.
fn fit_path(entries: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut entries: Vec<String> = entries.into_iter().filter(|e| seen.insert(e.trim_end_matches('\\').to_lowercase())).collect();
    let windows = system32().parent().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    let system = |e: &String| !windows.is_empty() && e.to_lowercase().starts_with(&windows);
    while entries.iter().map(|e| e.len() + 1).sum::<usize>() > PATH_LIMIT {
        match entries.iter().rposition(|e| !system(e)) {
            Some(last) => {
                entries.remove(last);
            }
            None => break,
        }
    }
    entries
}

/// `dir` is a folder cmd.exe can be given in quotes, and the elevated process can reach: a path
/// on a drive (not a network share, which an elevated process can't see through the user's
/// mapped drives, nor `\\?\` and `\\.\`), without a character that quotes can't hold.
fn folder_fit_for_cmd(dir: &Path) -> Result<(), String> {
    let text = dir.to_string_lossy();
    if let Some(c) = text.chars().find(|c| matches!(c, '"' | '%' | '^' | '\r' | '\n' | '\0')) {
        return Err(format!("its folder ({text}) has a {c:?} in its path, which cmd.exe can't be given safely"));
    }
    let bytes = text.as_bytes();
    if !(bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\') {
        return Err(format!("its folder ({text}) isn't on a local drive, which an administrator's process can't be relied on to reach"));
    }
    Ok(())
}

/// `System32`, where cmd.exe and the programs its script uses are.
fn system32() -> PathBuf {
    cmd_exe().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"))
}

/// One run's folder, Trek's alone (see the module's note): made new under a name that can't be
/// guessed; its script and each file the script writes made here, and held open without the
/// right to delete or replace them (and, for the script, to write to it) until the run is
/// dropped, which removes the folder.
pub struct Run {
    dir: PathBuf,
    held: Vec<std::fs::File>,
}

impl Run {
    /// A folder of its own under `parent`.
    pub fn create(parent: &Path) -> Result<Run, String> {
        folder_fit_for_cmd(parent)?;
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
        let dir = parent.join(format!("elevated-{}", uuid::Uuid::new_v4().simple()));
        // `create_dir`, not `create_dir_all`: a folder that exists already isn't ours.
        std::fs::create_dir(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
        Ok(Run { dir, held: vec![] })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn script_path(&self) -> PathBuf {
        self.dir.join("run.cmd")
    }

    /// Write `text` as the script, make the files `steps` steps will write, and hold them all;
    /// then read the script back: it is what was written, and stays so.
    pub fn seal(&mut self, text: &str, steps: usize) -> Result<PathBuf, String> {
        let script = self.script_path();
        let oops = |what: &str, p: &Path, e: std::io::Error| format!("couldn't {what} {}: {e}", p.display());
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&script).map_err(|e| oops("write", &script, e))?;
        file.write_all(text.as_bytes()).map_err(|e| oops("write", &script, e))?;
        drop(file);
        // Held for reading, with other programs allowed to read it but nothing else: not to write
        // it, nor to delete or rename it. cmd.exe reads it by name as it goes.
        let mut held = open_held(&script, false).map_err(|e| oops("hold", &script, e))?;
        let mut back = Vec::new();
        held.read_to_end(&mut back).map_err(|e| oops("read back", &script, e))?;
        if back != text.as_bytes() {
            return Err(format!("{} isn't what Trek wrote to it", script.display()));
        }
        self.held.push(held);
        // The files the steps write: made here (an existing one, a link, can't be put in their
        // place), and held, so they stay these.
        for step in 0..steps {
            for path in [output_file(&self.dir, step), code_file(&self.dir, step)] {
                self.held.push(open_held(&path, true).map_err(|e| oops("make", &path, e))?);
            }
        }
        Ok(script)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // Let go of the files first: a folder can't go with its files held.
        self.held.clear();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `path` as a file held for a run. `create`: a new, empty one, held for reading and shared for
/// reading and writing (the script writing to it isn't in the way) but not for delete or rename.
/// Otherwise an existing one, held for reading and shared for reading only. (A handle opened for
/// attributes alone holds nothing: Windows lets others delete a file whatever its share mode.)
#[cfg(windows)]
fn open_held(path: &Path, create: bool) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    const GENERIC_READ: u32 = 0x8000_0000;
    let mut options = std::fs::OpenOptions::new();
    if create {
        options.write(true).create_new(true).access_mode(GENERIC_READ).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    } else {
        options.read(true).share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

#[cfg(not(windows))]
fn open_held(path: &Path, create: bool) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    if create { options.write(true).create_new(true) } else { options.read(true) }.open(path)
}

/// A `.cmd` or `.bat`: cmd.exe runs it through `call`, or the script would end with it.
fn is_batch(program: &Path) -> bool {
    program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// `token` in double quotes for a line of the script; `called` for a line run through `call`.
/// `program` for the program itself, which may end with `\` (it never ends a folder).
pub fn quote(token: &str, called: bool, program: bool) -> Result<String, String> {
    if let Some(c) = token.chars().find(|c| matches!(c, '"' | '\r' | '\n' | '\0') || (called && *c == '^')) {
        return Err(format!("{token:?} has a {c:?}, which can't be passed safely through cmd.exe"));
    }
    if !program && token.ends_with('\\') {
        return Err(format!("{token:?} ends with \\, which can't be passed safely through cmd.exe"));
    }
    let percent = if called { "%%%%" } else { "%%" };
    Ok(format!("\"{}\"", token.replace('%', percent)))
}

/// `value` for `set "NAME=value"`: a `%` doubled; refused with a `"` or a line break.
fn set_value(value: &str) -> Result<String, String> {
    if let Some(c) = value.chars().find(|c| matches!(c, '"' | '\r' | '\n' | '\0')) {
        return Err(format!("{value:?} has a {c:?}, which can't be set safely in cmd.exe"));
    }
    Ok(value.replace('%', "%%"))
}

/// What cmd.exe is given to run the script at `script`: no AutoRun commands (`/d`), no delayed
/// expansion, and the script's path in quotes (`/s` strips only the outer pair). The path was
/// checked by `script` (its folder's).
pub fn command_line(script: &Path) -> String {
    format!("/d /v:off /s /c \"\"{}\"\"", script.display())
}

/// cmd.exe, by its full path: the elevated process isn't to find another on a PATH.
pub fn cmd_exe() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join(r"System32\cmd.exe")
}

/// Why an elevated script didn't run to its end.
#[derive(Debug)]
pub enum Failure {
    /// The user said no to Windows' prompt.
    Cancelled,
    /// No one answered it: Windows gives up on its prompt after about two minutes, with the
    /// same error as a "No".
    Unanswered,
    /// It ran longer than it may; `ended` when Trek could end it.
    TimedOut { ended: bool },
    Other(String),
}

/// How a script is started: as an administrator for real, as it is in tests (which must never
/// bring up a UAC prompt). Returns cmd.exe's exit code.
pub type Launch = fn(&Path, Duration) -> Result<u32, Failure>;

/// What a "not allowed" from Windows' prompt was, by how long it was up: a refusal comes in
/// seconds, the prompt giving up unanswered at about two minutes (122 s on Windows 11, 2026-10-11).
pub fn refused(after: Duration) -> Failure {
    if after >= Duration::from_secs(100) { Failure::Unanswered } else { Failure::Cancelled }
}

/// Run the script at `script` as an administrator, through Windows' UAC prompt, and wait up to
/// `timeout` for it. Blocks: call it off the async runtime.
#[cfg(windows)]
pub fn run_as_admin(script: &Path, timeout: Duration) -> Result<u32, Failure> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NO_CONSOLE, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, SW_HIDE};
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let verb = wide("runas".as_ref());
    let file = wide(cmd_exe().as_os_str());
    let params = wide(command_line(script).as_ref());
    // Run from System32, not the script's folder: what the script starts must not find programs
    // or libraries a user's program has put beside it.
    let dir = wide(system32().as_os_str());
    // SAFETY: all zeroes is a valid SHELLEXECUTEINFOW (integers, null pointers and handles).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // Trek's window owns the prompt, if it's still there.
    let owner = OWNER_WINDOW.load(Ordering::Relaxed);
    // SAFETY: IsWindow only asks whether a handle is one.
    if owner != 0 && unsafe { IsWindow(owner as *mut std::ffi::c_void) } != 0 {
        info.hwnd = owner as *mut std::ffi::c_void;
    }
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = dir.as_ptr();
    info.nShow = SW_HIDE;
    // The shell may hand the verb to COM objects: this thread is set up for them, as its
    // documentation asks, and let go after.
    // SAFETY: plain calls; the strings `info` points to outlive the call.
    let asked = std::time::Instant::now();
    let (ok, error) = unsafe {
        let com = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
        let ok = ShellExecuteExW(&mut info) != 0;
        let error = std::io::Error::last_os_error();
        if com >= 0 {
            CoUninitialize();
        }
        (ok, error)
    };
    if !ok {
        return Err(match error.raw_os_error() {
            Some(code) if code == ERROR_CANCELLED as i32 => refused(asked.elapsed()),
            _ => Failure::Other(error.to_string()),
        });
    }
    if info.hProcess.is_null() {
        return Err(Failure::Other("Windows started nothing to wait for".into()));
    }
    // SAFETY: the process handle ShellExecuteExW opened for this call (NOCLOSEPROCESS), owned from here.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: a process handle this owns.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), ms) } {
        WAIT_OBJECT_0 => {
            let mut code = 0u32;
            // SAFETY: a process handle with query rights, and a writable u32.
            if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
                return Err(Failure::Other(std::io::Error::last_os_error().to_string()));
            }
            Ok(code)
        }
        WAIT_TIMEOUT => {
            // An administrator's processes are out of reach of one that isn't: what can be ended is.
            // SAFETY: a process handle this owns.
            let pid = unsafe { GetProcessId(process.as_raw_handle()) } as i32;
            crate::procs::register(pid);
            let ended = crate::procs::end_tree(pid);
            crate::procs::unregister(pid);
            Err(Failure::TimedOut { ended })
        }
        _ => Err(Failure::Other(std::io::Error::last_os_error().to_string())),
    }
}

#[cfg(not(windows))]
pub fn run_as_admin(_script: &Path, _timeout: Duration) -> Result<u32, Failure> {
    Err(Failure::Other("only Windows runs commands as an administrator this way".into()))
}

/// For tests: the script run as it is, not elevated, through the same cmd.exe command line.
#[cfg(all(windows, test))]
pub fn run_plain(script: &Path, timeout: Duration) -> Result<u32, Failure> {
    use std::os::windows::process::CommandExt as _;
    let mut command = crate::procs::command(cmd_exe());
    command.raw_arg(command_line(script)).current_dir(system32()).stdin(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|e| Failure::Other(e.to_string()))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| Failure::Other(e.to_string()))? {
            return Ok(status.code().unwrap_or(1) as u32);
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err(Failure::TimedOut { ended: true });
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_cant_be_quoted_for_cmd_is_refused() {
        assert_eq!(quote("upgrade", false, false).unwrap(), "\"upgrade\"");
        assert_eq!(quote("a&b|c<d>e(f)g h!", false, false).unwrap(), "\"a&b|c<d>e(f)g h!\"");
        assert_eq!(quote("100%", false, false).unwrap(), "\"100%%\"");
        assert_eq!(quote("100%", true, false).unwrap(), "\"100%%%%\"", "twice for a `call`");
        assert_eq!(quote("^x", false, false).unwrap(), "\"^x\"");
        assert!(quote("^x", true, false).is_err(), "`call` doubles carets, even quoted");
        for bad in ["a\"b", "a\nb", "a\rb", "a\0b"] {
            assert!(quote(bad, false, false).is_err(), "{bad:?}");
            assert!(quote(bad, true, true).is_err(), "{bad:?}");
        }
        assert!(quote(r"C:\dir\", false, false).is_err(), "an exe would read `\\\"` as a quote");
        assert!(quote(r"C:\dir\x.exe", false, true).is_ok());
        // A folder that can't be quoted refuses the script, as does a step.
        let step = |args: &[&str]| Step { program: r"C:\ProgramData\chocolatey\bin\choco.exe".into(), args: args.iter().map(|a| a.to_string()).collect(), env: vec![], path: String::new() };
        assert!(script(Path::new(r"C:\Users\100%\tmp"), &[step(&["upgrade"])]).is_err());
        assert!(script(Path::new(r"C:\tmp"), &[step(&["a\"b"])]).is_err());
        let env = Step { env: vec![("A B".into(), "x".into())], ..step(&[]) };
        assert!(script(Path::new(r"C:\tmp"), &[env]).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn the_script_runs_each_step_into_files_of_its_own() {
        let path = "C:\\Windows;C:\\a\"b;C:\\x%y".to_string();
        let steps = [
            Step { program: r"C:\ProgramData\chocolatey\bin\choco.exe".into(), args: vec!["upgrade".into(), "opencode".into(), "-y".into()], env: vec![], path: path.clone() },
            Step { program: r"C:\ProgramData\scoop\shims\scoop.cmd".into(), args: vec!["update".into(), "opencode".into(), "-g".into()], env: vec![("K".into(), "50%".into())], path },
        ];
        let text = script(Path::new(r"C:\Trek data\tmp\run 1"), &steps).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // `chcp` by its full path: a bare name is looked up in the current folder first.
        let chcp = format!("\"{}\" 65001 >nul", system32().join("chcp.com").display());
        assert_eq!(&lines[..6], ["@echo off", chcp.as_str(), "setlocal DisableDelayedExpansion", r#"set "NO_COLOR=1""#, "setlocal", r#"set "PATH=C:\Windows;C:\ab;C:\x%%y""#]);
        // A PATH's quotes group (`C:\a"b` is `C:\ab`, as Windows reads it); its `%` is doubled.
        assert!(lines.contains(&r#""C:\ProgramData\chocolatey\bin\choco.exe" "upgrade" "opencode" "-y" <nul >"C:\Trek data\tmp\run 1\0.out" 2>&1"#), "{text}");
        assert!(lines.contains(&r#">"C:\Trek data\tmp\run 1\0.code" echo %ERRORLEVEL%"#), "{text}");
        assert!(lines.contains(&r#"call "C:\ProgramData\scoop\shims\scoop.cmd" "update" "opencode" "-g" <nul >"C:\Trek data\tmp\run 1\1.out" 2>&1"#), "{text}");
        assert!(lines.contains(&r#"set "K=50%%""#), "{text}");
        assert!(text.ends_with("exit /b 0\r\n"));
        assert_eq!(command_line(Path::new(r"C:\Trek data\tmp\run 1\run.cmd")), r#"/d /v:off /s /c ""C:\Trek data\tmp\run 1\run.cmd"""#);
    }

    /// The script, run (not elevated: the same script and cmd.exe line as the elevated run) with
    /// programs in folders whose names cmd.exe would read as operators, and arguments too.
    #[test]
    #[cfg(windows)]
    fn the_script_passes_paths_and_arguments_through_cmd_whole() {
        let root = std::env::temp_dir().join(format!("trek-elevated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let odd = root.join("Program Files (x86) & 100% é!");
        std::fs::create_dir_all(&odd).unwrap();
        // An .exe: the fixture, echoing its arguments.
        let exe = odd.join("tool.exe");
        std::fs::copy(trek_test_fixtures::bin("fixture"), &exe).unwrap();
        // A .cmd (run through `call`), logging its arguments as cmd.exe hands them over.
        let cmd = odd.join("tool.cmd");
        std::fs::write(&cmd, "@echo off\r\necho [%1][%2][%3]\r\nexit /b 3\r\n").unwrap();
        // One that says what a variable the step set is.
        let env = odd.join("env.cmd");
        std::fs::write(&env, "@echo off\r\necho \"%K%\"\r\n").unwrap();
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let step = |program: &PathBuf, a: &[&str], env: &[(&str, &str)]| Step {
            program: program.clone(),
            args: args(a),
            env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            path: std::env::var("PATH").unwrap(),
        };
        let steps = [
            step(&exe, &["echo", "a&b", "100%", "%PATH%", "x!y", "(x86)", "|", ">"], &[]),
            step(&cmd, &["a&b c", "100%", "%PATH%"], &[]),
            step(&exe, &["exit", "-1978335189"], &[]),
            step(&env, &[], &[("K", "it's 50% & more")]),
            step(&env, &[], &[]),
            // More of what cmd.exe makes something of: carets, doubled and variable-looking
            // percents, `&&` and `||`, a stray parenthesis, a redirection to a file, a tilde
            // modifier, delayed expansion's `!`, non-ASCII, a UNC-looking path.
            step(&exe, &["echo", "^&", "%%", "%%PATH%%", "%~dp0", "&&", "||", ")", "2>nul", ">out.txt", "!PATH!", "!", "ünï", r"\\server\share", "a b"], &[]),
            step(&cmd, &["x&calc", "%1", "!x!"], &[]),
        ];
        // The run's folder is odd too: parentheses, an ampersand, an accent, a bang.
        let mut run = Run::create(&root.join("tmp (x86) & é!")).unwrap();
        let file = run.seal(&script(run.dir(), &steps).unwrap(), steps.len()).unwrap();
        assert_eq!(run_plain(&file, Duration::from_secs(60)).unwrap(), 0);
        let dir = run.dir().to_path_buf();
        let read = |f: PathBuf| std::fs::read_to_string(f).unwrap().trim_end().to_string();
        assert_eq!(read(output_file(&dir, 0)), "a&b 100% %PATH% x!y (x86) | >");
        assert_eq!(read(output_file(&dir, 1)), r#"["a&b c"]["100%"]["%PATH%"]"#);
        let codes: Vec<String> = (0..steps.len()).map(|i| read(code_file(&dir, i))).collect();
        assert_eq!(codes, ["0", "3", "-1978335189", "0", "0", "0", "3"]);
        // A variable is set for its step, and only that one.
        assert_eq!(read(output_file(&dir, 3)), "\"it's 50% & more\"");
        assert_eq!(read(output_file(&dir, 4)), "\"\"", "unset in a batch file: empty");
        assert_eq!(read(output_file(&dir, 5)), r"^& %% %%PATH%% %~dp0 && || ) 2>nul >out.txt !PATH! ! ünï \\server\share a b");
        assert_eq!(read(output_file(&dir, 6)), r#"["x&calc"]["%1"]["!x!"]"#);
        assert!(!dir.join("out.txt").exists() && !system32().join("out.txt").exists(), "a `>` in an argument redirects nothing");
        drop(run);
        assert!(!dir.exists(), "the folder goes with the run");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder of the run's own, whose files nothing else can rewrite, replace or delete, while it
    /// is held, and which cmd.exe can all the same read and write to.
    #[test]
    #[cfg(windows)]
    fn a_runs_folder_is_its_own_and_its_files_stay_what_they_were_made() {
        let root = std::env::temp_dir().join(format!("trek-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let exe = root.join("tool.exe");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(trek_test_fixtures::bin("fixture"), &exe).unwrap();
        let steps = [Step { program: exe, args: vec!["echo".into(), "sealed".into()], env: vec![], path: std::env::var("PATH").unwrap() }];
        let (mut a, b) = (Run::create(&root.join("tmp")).unwrap(), Run::create(&root.join("tmp")).unwrap());
        // A name no one could guess, and a folder of its own: two runs share nothing.
        let name = |r: &Run| r.dir().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name(&a).starts_with("elevated-") && name(&a).len() >= "elevated-".len() + 32, "{}", name(&a));
        assert_ne!(a.dir(), b.dir());
        drop(b);
        let text = script(a.dir(), &steps).unwrap();
        let file = a.seal(&text, steps.len()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        // Nothing can change the script between Trek's check and cmd.exe's reading...
        assert!(std::fs::OpenOptions::new().write(true).open(&file).is_err(), "no writing to it");
        assert!(std::fs::write(&file, "calc\r\n").is_err(), "no rewriting it");
        assert!(std::fs::remove_file(&file).is_err(), "no deleting it");
        assert!(std::fs::rename(&file, a.dir().join("other.cmd")).is_err(), "no renaming it away");
        // ...nor put another in its place, nor its output files: a link or a hard link there would
        // have an administrator's script write wherever it pointed.
        for held in [output_file(a.dir(), 0), code_file(a.dir(), 0)] {
            assert!(std::fs::remove_file(&held).is_err(), "{}", held.display());
            assert!(std::fs::rename(&held, a.dir().join("moved")).is_err(), "{}", held.display());
            assert!(std::fs::OpenOptions::new().create_new(true).write(true).open(&held).is_err(), "{}", held.display());
        }
        // The folder itself can't be moved from under it.
        assert!(std::fs::rename(a.dir(), root.join("elsewhere")).is_err(), "no renaming the folder");
        // And cmd.exe reads and writes what is held all the same.
        assert_eq!(run_plain(&file, Duration::from_secs(60)).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(output_file(a.dir(), 0)).unwrap().trim_end(), "sealed");
        assert_eq!(std::fs::read_to_string(code_file(a.dir(), 0)).unwrap().trim(), "0");
        let dir = a.dir().to_path_buf();
        drop(a);
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_cmd_cant_be_given_or_the_elevated_process_cant_reach_is_refused() {
        let step = Step { program: r"C:\ProgramData\chocolatey\bin\choco.exe".into(), args: vec!["upgrade".into()], env: vec![], path: String::new() };
        for bad in [r"\\server\share\tmp", r"\\?\C:\tmp", r"\\.\C:\tmp", r"tmp", r"C:tmp", r"C:\", r"C:\a^b", r"C:\a%b", "C:\\a\"b"] {
            assert!(script(Path::new(bad), std::slice::from_ref(&step)).is_err(), "{bad}");
            assert!(Run::create(Path::new(bad)).is_err(), "{bad}");
        }
        assert!(script(Path::new(r"D:\Users\é & (x86)\tmp"), std::slice::from_ref(&step)).is_ok());
    }

    // Windows only: `script` splits the step's PATH as this platform does, and on macOS that
    // cuts `C:\…` at the colon.
    #[cfg(windows)]
    #[test]
    fn a_line_longer_than_cmd_reads_is_refused_and_a_long_path_is_split() {
        let path = |n: usize| (0..n).map(|i| format!(r"C:\Tools\folder number {i}")).collect::<Vec<_>>().join(";");
        let step = |args: Vec<String>, path: String| Step { program: r"C:\ProgramData\chocolatey\bin\choco.exe".into(), args, env: vec![], path };
        // An argument of 9000 characters is more than a line holds: refused, not cut.
        let long = script(Path::new(r"C:\tmp"), &[step(vec!["x".repeat(9000)], String::new())]);
        assert!(long.is_err_and(|e| e.contains("more than cmd.exe reads")));
        assert!(script(Path::new(r"C:\tmp"), &[step(vec!["x".repeat(3000)], String::new())]).is_ok());
        // A PATH too long for a line (cmd.exe cuts one at 8191 characters, and a variable built up
        // over several lines at the same place) is cut by Trek instead: each folder once, and the
        // last of the others dropped until it fits; Windows' own folders stay.
        let windows = system32();
        let long_path = [path(900), windows.display().to_string(), r"C:\Tools\folder number 1".into(), windows.parent().unwrap().display().to_string().to_uppercase()].join(";");
        let text = script(Path::new(r"C:\tmp"), &[step(vec![], long_path)]).unwrap();
        let sets: Vec<&str> = text.lines().filter(|l| l.starts_with("set \"PATH=")).collect();
        assert_eq!(sets.len(), 1, "one line");
        assert!(text.lines().all(|l| l.len() <= MAX_LINE));
        let entries: Vec<&str> = sets[0].trim_start_matches("set \"PATH=").trim_end_matches('"').split(';').collect();
        assert_eq!(entries[0], r"C:\Tools\folder number 0", "the first stay, in order");
        assert_eq!(entries[1], r"C:\Tools\folder number 1");
        assert!(entries.len() < 900 && entries.len() > 100, "{}", entries.len());
        assert_eq!(entries.iter().filter(|e| **e == r"C:\Tools\folder number 1").count(), 1, "once");
        assert_eq!(entries[entries.len() - 2..], [windows.display().to_string(), windows.parent().unwrap().display().to_string().to_uppercase()], "Windows' own are kept");
        // A short one is as it was, `!` and `^` and all.
        let short = script(Path::new(r"C:\tmp"), &[step(vec![], r"C:\Tools\hey!;C:\Tools\a^b;C:\Tools\other\".into())]).unwrap();
        assert!(short.contains(r#"set "PATH=C:\Tools\hey!;C:\Tools\a^b;C:\Tools\other\""#), "{short}");
        assert_eq!(fit_path(vec![r"C:\a".into(), r"c:\A\".into(), r"C:\b".into()]), [r"C:\a", r"C:\b"], "a folder twice, by case or a trailing backslash");
    }

    /// The longest PATH Trek sets, as cmd.exe makes of it: whole.
    #[test]
    #[cfg(windows)]
    fn a_long_path_arrives_whole() {
        let root = std::env::temp_dir().join(format!("trek-longpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // `set PATH` prints it without the line limit that `echo %PATH%` would meet.
        let show = root.join("show.cmd");
        std::fs::write(&show, "@echo off\r\nset PATH\r\n").unwrap();
        let path = (0..900).map(|i| format!(r"C:\Tools\folder number {i} (x86)")).collect::<Vec<_>>().join(";");
        let steps = [Step { program: show, args: vec![], env: vec![], path: path.clone() }];
        // What Trek fits to a line (about 7500 characters), and no less.
        let path = fit_path(path.split(';').map(String::from).collect()).join(";");
        assert!(path.len() > 7000 && path.len() <= PATH_LIMIT, "{}", path.len());
        let mut run = Run::create(&root.join("tmp")).unwrap();
        let file = run.seal(&script(run.dir(), &steps).unwrap(), 1).unwrap();
        assert_eq!(run_plain(&file, Duration::from_secs(60)).unwrap(), 0);
        let out = std::fs::read_to_string(output_file(run.dir(), 0)).unwrap();
        // (Named as the environment already had it: `Path`.)
        let got = out.lines().find(|l| l.get(..5).is_some_and(|n| n.eq_ignore_ascii_case("path="))).unwrap_or_default();
        assert_eq!((got.len() - 5, got[5..].get(..200)), (path.len(), path.get(..200)), "{}", &got[got.len().saturating_sub(200)..]);
        assert_eq!(&got[5..], path);
        drop(run);
        let _ = std::fs::remove_dir_all(&root);
    }
}
