//! "Open in…" on Windows: which of Windows Terminal and the editors are installed, and how each is
//! started on a folder. (On a Mac the menu hands the app's name to `open -a`; this is the part
//! that has no such thing. The file manager needs no lookup: the system opens a folder itself.)
//!
//! Lookup is a pure function over three questions (the App Paths registry, the PATH, whether a
//! file exists) so a test answers them without a registry; `windows_targets` asks the real ones.

use crate::detect;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What one entry of the menu starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum App {
    /// Windows Terminal, started in the folder with `-d`.
    Terminal(PathBuf),
    /// An editor that takes the folder as its argument.
    Editor(PathBuf),
}

/// One row of the menu, after the file manager's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub label: &'static str,
    pub app: App,
}

/// A program to start: the path and its arguments, each one argument (never a joined string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// What to look for, for one program.
struct Spec {
    label: &'static str,
    /// Its name under `...\CurrentVersion\App Paths`, which is also the exe's own name.
    exe: &'static str,
    /// Its name on the PATH, without an extension (`PATHEXT` supplies it).
    command: &'static str,
    terminal: bool,
    /// Ask the PATH before App Paths. Windows Terminal's App Paths entry points inside
    /// `C:\Program Files\WindowsApps`, which only its execution alias on the PATH may start.
    path_first: bool,
}

/// In the order the Mac's menu has them (after the file manager): Terminal, then Zed, Cursor and
/// VS Code. Ghostty and Xcode are Mac programs.
const SPECS: [Spec; 4] = [
    Spec { label: "Windows Terminal", exe: "wt.exe", command: "wt", terminal: true, path_first: true },
    Spec { label: "Zed", exe: "Zed.exe", command: "zed", terminal: false, path_first: false },
    Spec { label: "Cursor", exe: "Cursor.exe", command: "cursor", terminal: false, path_first: false },
    Spec { label: "VS Code", exe: "Code.exe", command: "code", terminal: false, path_first: false },
];

/// How many folders up from a `.cmd`'s own to look for the editor's exe: VS Code's `code.cmd` is in
/// `<root>\bin`, Cursor's in `<root>\resources\app\bin`.
const ROOT_SEARCH: usize = 4;

/// The menu's rows for this machine (after the file manager's): each program found, as
/// `windows_targets_with` answers them from the machine's registry and PATH.
pub fn windows_targets() -> Vec<Target> {
    windows_targets_with(app_path, |name| detect::which_in(detect::login_path(), name), |p| p.is_file())
}

/// `windows_targets` with the three lookups given: `app_path` a program's registered path (App
/// Paths) from its exe name, `on_path` a command's place on the PATH, `exists` whether a file is
/// there. A registered path whose file is gone (an uninstalled editor leaves its entry behind) is
/// as good as none. A program found as a `.cmd` shim is started through the exe beside it when
/// there is one: `code.cmd` runs through cmd.exe, which makes every argument a hazard.
pub fn windows_targets_with(app_path: impl Fn(&str) -> Option<PathBuf>, on_path: impl Fn(&str) -> Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> Vec<Target> {
    let registered = |spec: &Spec| app_path(spec.exe).filter(|p| exists(p));
    let mut out = vec![];
    for spec in &SPECS {
        let found = if spec.path_first { on_path(spec.command).or_else(|| registered(spec)) } else { registered(spec).or_else(|| on_path(spec.command)) };
        let Some(found) = found else { continue };
        let program = if is_batch(&found) { exe_beside(&found, spec.exe, &exists).unwrap_or(found) } else { found };
        out.push(Target { label: spec.label, app: if spec.terminal { App::Terminal(program) } else { App::Editor(program) } });
    }
    out
}

/// A `.cmd` or `.bat`, which Windows starts through cmd.exe.
fn is_batch(program: &Path) -> bool {
    program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// `exe` in the folder of `shim` or one of the few above it.
fn exe_beside(shim: &Path, exe: &str, exists: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    shim.parent()?.ancestors().take(ROOT_SEARCH).map(|dir| dir.join(exe)).find(|p| exists(p))
}

impl Target {
    /// What to start to open `folder` with this; `None` for one that isn't an absolute path (a
    /// name that began with `-` would be taken for an option).
    pub fn launch(&self, folder: &Path) -> Option<Launch> {
        if !folder.is_absolute() {
            return None;
        }
        Some(match &self.app {
            App::Terminal(program) => Launch { program: program.clone(), args: vec!["-d".into(), folder.into()] },
            App::Editor(program) => Launch { program: program.clone(), args: vec![folder.into()] },
        })
    }
}

/// Start `launch` and leave it running: no console window, no input, and nothing waited for.
pub fn spawn(launch: &Launch) -> std::io::Result<()> {
    if let Some(problem) = detect::batch_args_problem(&launch.program, &launch.args) {
        return Err(std::io::Error::other(problem));
    }
    let mut command = std::process::Command::new(&launch.program);
    command.args(&launch.args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    // Reaped when it exits, so a long-lived editor doesn't leave a zombie on the Mac or Linux.
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// `exe`'s registered path: the default value of `...\CurrentVersion\App Paths\<exe>`, the user's
/// before the machine's, as the shell looks.
#[cfg(windows)]
fn app_path(exe: &str) -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| {
        let raw: String = winreg::RegKey::predef(hive).open_subkey_with_flags(&key, KEY_READ).ok()?.get_value("").ok()?;
        registered_value(&raw)
    })
}

#[cfg(not(windows))]
fn app_path(_: &str) -> Option<PathBuf> {
    None
}

/// The path an App Paths value names: it may be quoted, and may hold `%VAR%`s.
#[cfg(any(windows, test))]
fn registered_value(raw: &str) -> Option<PathBuf> {
    let value = detect::expand_env_refs(raw.trim().trim_matches('"'), |name| std::env::var(name).ok());
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A machine: its registered programs (by exe name), its PATH commands, and the files that exist.
    #[derive(Default)]
    struct Machine {
        registered: HashMap<&'static str, &'static str>,
        path: HashMap<&'static str, &'static str>,
        files: HashSet<&'static str>,
    }

    impl Machine {
        fn targets(&self) -> Vec<(&'static str, App)> {
            let found = windows_targets_with(
                |exe| self.registered.get(exe).map(PathBuf::from),
                |name| self.path.get(name).map(PathBuf::from),
                |p| self.files.iter().any(|f| Path::new(f) == p),
            );
            found.into_iter().map(|t| (t.label, t.app)).collect()
        }
    }

    fn labels(targets: &[(&'static str, App)]) -> Vec<&'static str> {
        targets.iter().map(|(label, _)| *label).collect()
    }

    #[test]
    fn a_bare_machine_offers_nothing() {
        assert_eq!(Machine::default().targets(), []);
    }

    #[test]
    fn what_is_installed_is_listed_in_the_order_of_the_mac_menu() {
        let m = Machine {
            registered: HashMap::from([("Code.exe", "C:/vs/Code.exe"), ("Cursor.exe", "C:/cur/Cursor.exe")]),
            path: HashMap::from([("zed", "C:/zed/zed.exe"), ("wt", "C:/apps/wt.exe")]),
            files: HashSet::from(["C:/vs/Code.exe", "C:/cur/Cursor.exe"]),
        };
        let got = m.targets();
        assert_eq!(labels(&got), ["Windows Terminal", "Zed", "Cursor", "VS Code"]);
        assert_eq!(got[0].1, App::Terminal("C:/apps/wt.exe".into()));
        assert_eq!(got[1].1, App::Editor("C:/zed/zed.exe".into()));
        assert_eq!(got[3].1, App::Editor("C:/vs/Code.exe".into()));
    }

    #[test]
    fn only_what_is_installed_appears() {
        let m = Machine { path: HashMap::from([("code", "C:/vs/Code.exe")]), ..Machine::default() };
        assert_eq!(labels(&m.targets()), ["VS Code"]);
    }

    #[test]
    fn the_registered_path_is_asked_before_the_path_for_an_editor() {
        let m = Machine {
            registered: HashMap::from([("Code.exe", "C:/system/Code.exe")]),
            path: HashMap::from([("code", "C:/other/Code.exe")]),
            files: HashSet::from(["C:/system/Code.exe"]),
        };
        assert_eq!(m.targets()[0].1, App::Editor("C:/system/Code.exe".into()));
    }

    #[test]
    fn a_registered_path_whose_file_is_gone_falls_back_to_the_path() {
        // An uninstalled editor leaves its App Paths entry behind.
        let m = Machine { registered: HashMap::from([("Cursor.exe", "C:/gone/Cursor.exe")]), ..Machine::default() };
        assert_eq!(m.targets(), [], "nothing is found");
        let m = Machine { path: HashMap::from([("cursor", "C:/cur/Cursor.exe")]), ..m };
        assert_eq!(m.targets()[0].1, App::Editor("C:/cur/Cursor.exe".into()));
    }

    #[test]
    fn windows_terminal_is_started_through_its_alias_not_its_package_folder() {
        // App Paths names the file inside WindowsApps, which a program that isn't packaged may not start.
        let package = "C:/Program Files/WindowsApps/Microsoft.WindowsTerminal_1/wt.exe";
        let alias = "C:/Users/me/AppData/Local/Microsoft/WindowsApps/wt.exe";
        let m = Machine { registered: HashMap::from([("wt.exe", package)]), path: HashMap::from([("wt", alias)]), files: HashSet::from([package]) };
        assert_eq!(m.targets()[0].1, App::Terminal(alias.into()));
        // With no alias on the PATH, the registered one is better than nothing.
        let m = Machine { path: HashMap::new(), ..m };
        assert_eq!(m.targets()[0].1, App::Terminal(package.into()));
    }

    #[test]
    fn a_cmd_shim_is_started_through_the_exe_beside_it() {
        let m = Machine {
            path: HashMap::from([("code", "C:/vs/bin/code.cmd"), ("cursor", "C:/cur/resources/app/bin/cursor.cmd")]),
            files: HashSet::from(["C:/vs/Code.exe", "C:/cur/Cursor.exe"]),
            ..Machine::default()
        };
        let got = m.targets();
        assert_eq!(got[0], ("Cursor", App::Editor("C:/cur/Cursor.exe".into())));
        assert_eq!(got[1], ("VS Code", App::Editor("C:/vs/Code.exe".into())));
    }

    #[test]
    fn a_cmd_shim_with_no_exe_near_it_is_kept() {
        let m = Machine { path: HashMap::from([("code", "C:/vs/bin/CODE.CMD")]), ..Machine::default() };
        assert_eq!(m.targets()[0].1, App::Editor("C:/vs/bin/CODE.CMD".into()));
    }

    #[test]
    fn a_folder_goes_to_each_program_as_one_argument() {
        let folder = Path::new(if cfg!(windows) { r"C:\My Projects\a;b" } else { "/My Projects/a;b" });
        let term = Target { label: "Windows Terminal", app: App::Terminal("wt.exe".into()) };
        let edit = Target { label: "VS Code", app: App::Editor("Code.exe".into()) };
        assert_eq!(term.launch(folder), Some(Launch { program: "wt.exe".into(), args: vec!["-d".into(), folder.into()] }));
        assert_eq!(edit.launch(folder), Some(Launch { program: "Code.exe".into(), args: vec![folder.into()] }));
    }

    #[test]
    fn a_folder_that_could_be_an_option_is_refused() {
        let edit = Target { label: "VS Code", app: App::Editor("Code.exe".into()) };
        assert_eq!(edit.launch(Path::new("--remote=evil")), None);
        assert_eq!(edit.launch(Path::new("relative")), None);
    }

    #[test]
    fn an_app_paths_value_may_be_quoted_or_name_an_environment_variable() {
        assert_eq!(registered_value(r#""C:/vs/Code.exe""#), Some("C:/vs/Code.exe".into()));
        assert_eq!(registered_value("  C:/vs/Code.exe "), Some("C:/vs/Code.exe".into()));
        assert_eq!(registered_value(""), None);
        assert_eq!(registered_value(r#""""#), None);
        // A reference to a variable that isn't set stays as written.
        assert_eq!(registered_value("%TREK_OPEN_IN_NOT_SET%/Code.exe"), Some("%TREK_OPEN_IN_NOT_SET%/Code.exe".into()));
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_is_not_there_does_not_start() {
        let launch = Launch { program: PathBuf::from(r"C:\definitely\not\here\editor.exe"), args: vec![] };
        assert!(spawn(&launch).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_script_that_cannot_take_the_folder_is_refused_before_it_starts() {
        let launch = Launch { program: PathBuf::from(r"C:\x\code.cmd"), args: vec!["a\nb".into()] };
        let err = spawn(&launch).unwrap_err();
        assert!(err.to_string().contains("line break"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn this_machines_rows_are_each_listed_once() {
        let targets = windows_targets();
        let mut seen = HashSet::new();
        assert!(targets.iter().all(|t| seen.insert(t.label)), "each row once: {targets:?}");
    }
}
