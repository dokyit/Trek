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

use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let dir_text = dir.to_string_lossy();
    if let Some(c) = dir_text.chars().find(|c| matches!(c, '"' | '%' | '^' | '\r' | '\n' | '\0')) {
        return Err(format!("its folder ({dir_text}) has a {c:?} in its path, which cmd.exe can't be given safely"));
    }
    let mut lines = vec![
        "@echo off".to_string(),
        // Lines after this one are read as UTF-8 (a path with an accent), and .NET programs
        // (Chocolatey) write their output in it.
        "chcp 65001 >nul".to_string(),
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
        lines.push(format!("set \"PATH={}\"", path.join(";")));
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
    Ok(lines.join("\r\n") + "\r\n")
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
    /// It ran longer than it may; `ended` when Trek could end it.
    TimedOut { ended: bool },
    Other(String),
}

/// How a script is started: as an administrator for real, as it is in tests (which must never
/// bring up a UAC prompt). Returns cmd.exe's exit code.
pub type Launch = fn(&Path, Duration) -> Result<u32, Failure>;

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
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let verb = wide("runas".as_ref());
    let file = wide(cmd_exe().as_os_str());
    let params = wide(command_line(script).as_ref());
    let dir = wide(script.parent().unwrap_or(script).as_os_str());
    // SAFETY: all zeroes is a valid SHELLEXECUTEINFOW (integers, null pointers and handles).
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = dir.as_ptr();
    info.nShow = SW_HIDE;
    // The shell may hand the verb to COM objects: this thread is set up for them, as its
    // documentation asks, and let go after.
    // SAFETY: plain calls; the strings `info` points to outlive the call.
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
        return Err(if error.raw_os_error() == Some(ERROR_CANCELLED as i32) { Failure::Cancelled } else { Failure::Other(error.to_string()) });
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
    command.raw_arg(command_line(script)).current_dir(script.parent().unwrap_or(script)).stdin(std::process::Stdio::null());
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
        assert_eq!(&lines[..6], ["@echo off", "chcp 65001 >nul", "setlocal DisableDelayedExpansion", r#"set "NO_COLOR=1""#, "setlocal", r#"set "PATH=C:\Windows;C:\ab;C:\x%%y""#]);
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
        ];
        let dir = root.join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("run.cmd");
        std::fs::write(&file, script(&dir, &steps).unwrap()).unwrap();
        assert_eq!(run_plain(&file, Duration::from_secs(60)).unwrap(), 0);
        let read = |f: PathBuf| std::fs::read_to_string(f).unwrap().trim_end().to_string();
        assert_eq!(read(output_file(&dir, 0)), "a&b 100% %PATH% x!y (x86) | >");
        assert_eq!(read(output_file(&dir, 1)), r#"["a&b c"]["100%"]["%PATH%"]"#);
        let codes: Vec<String> = (0..5).map(|i| read(code_file(&dir, i))).collect();
        assert_eq!(codes, ["0", "3", "-1978335189", "0", "0"]);
        // A variable is set for its step, and only that one.
        assert_eq!(read(output_file(&dir, 3)), "\"it's 50% & more\"");
        assert_eq!(read(output_file(&dir, 4)), "\"\"", "unset in a batch file: empty");
        let _ = std::fs::remove_dir_all(&root);
    }
}
