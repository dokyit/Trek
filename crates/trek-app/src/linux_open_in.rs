//! Linux's "Open in" programs: the file manager (`xdg-open`, as GPUI opens a folder), then the
//! editors and the terminal this computer has on its PATH, each started in the folder (the
//! editors with it as their argument). macOS finds its apps through `open -a`, Windows through
//! the registry (`trek_core::open_in`).

use std::path::{Path, PathBuf};

/// (label, program, whether the folder goes as an argument): editors take it, a terminal starts
/// in it.
const PROGRAMS: &[(&str, &str, bool)] = &[
    ("VS Code", "code", true),
    ("Cursor", "cursor", true),
    ("Windsurf", "windsurf", true),
    ("Zed", "zeditor", true),
    ("Zed", "zed", true),
    ("Sublime Text", "subl", true),
    ("Terminal", "x-terminal-emulator", false),
    ("Terminal", "gnome-terminal", false),
    ("Terminal", "konsole", false),
];

/// A program found on the PATH, for the folder it opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Opener {
    pub label: &'static str,
    pub program: PathBuf,
    pub folder_as_argument: bool,
}

impl Opener {
    /// Start it on `dir`, detached from Trek's own input and output, in a process group of its
    /// own: the editor or terminal stays open when Trek quits.
    pub fn open(&self, dir: &Path) {
        use std::os::unix::process::CommandExt as _;
        let mut cmd = trek_core::procs::command(&self.program);
        cmd.process_group(0);
        if self.folder_as_argument {
            cmd.arg(dir);
        }
        let started = cmd
            .current_dir(dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(e) = started {
            tracing::warn!("open in {}: {e}", self.label);
        }
    }
}

/// Those of `PROGRAMS` on the PATH, one per label (the first found).
pub fn openers() -> Vec<Opener> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    openers_on(&std::env::split_paths(&path).collect::<Vec<_>>())
}

fn openers_on(dirs: &[PathBuf]) -> Vec<Opener> {
    let mut out: Vec<Opener> = vec![];
    for (label, name, folder_as_argument) in PROGRAMS {
        if out.iter().any(|o| o.label == *label) {
            continue;
        }
        if let Some(program) = dirs.iter().map(|d| d.join(name)).find(|p| is_program(p)) {
            out.push(Opener {
                label,
                program,
                folder_as_argument: *folder_as_argument,
            });
        }
    }
    out
}

fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_on_the_path_are_offered_once_each() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("trek-open-in-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["code", "zed", "zeditor", "konsole", "not-run"] {
            let p = dir.join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            let mode = if name == "not-run" { 0o644 } else { 0o755 };
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let found = openers_on(std::slice::from_ref(&dir));
        let labels: Vec<_> = found.iter().map(|o| o.label).collect();
        assert_eq!(labels, ["VS Code", "Zed", "Terminal"]);
        assert_eq!(
            found[1].program,
            dir.join("zeditor"),
            "the first of Zed's names"
        );
        assert!(
            !found[2].folder_as_argument,
            "a terminal starts in the folder"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
