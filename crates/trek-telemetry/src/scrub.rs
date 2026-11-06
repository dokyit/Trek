//! What's taken out of a log line, a panic message or a backtrace before it leaves: secrets
//! agents were given (`trek_core::session_secrets`), anything shaped like a key or token, email
//! addresses, and paths. The home folder is `~`; a project's folder and what's in it, and any
//! other path that could name the user's files, is `[path]`. Trek's own folders and the system's
//! stay readable, as they name nothing of the user's.

use std::sync::LazyLock;

/// A text with what mustn't leave taken out (`scrub`).
#[derive(Clone)]
pub struct Scrubber {
    /// The home folder, shown as `~`.
    home: Option<String>,
    /// Trek's data and log folders (which may have spaces in them: `Application Support`), and
    /// what each is shown as.
    trek_folders: Vec<(String, &'static str)>,
    /// The user's projects' folders, each with whatever path follows it: never shown, not even
    /// a file's name.
    projects: Vec<regex::Regex>,
    /// Values agents were given this run, `(name, value)`.
    secrets: Vec<(String, String)>,
    /// What takes out keys and tokens by their shape (the app's `share::render::redact_secrets`).
    shapes: fn(&str) -> String,
}

impl Scrubber {
    pub fn new(
        home: Option<String>,
        trek_folders: Vec<(String, &'static str)>,
        projects: &[String],
        secrets: Vec<(String, String)>,
        shapes: fn(&str) -> String,
    ) -> Scrubber {
        let mut folders: Vec<&str> = projects
            .iter()
            .map(|p| p.trim_end_matches(['/', '\\']))
            .filter(|p| p.len() > 1)
            .collect();
        // Longest first, so a folder inside another is matched whole.
        folders.sort_by_key(|p| std::cmp::Reverse(p.len()));
        let projects = folders
            .into_iter()
            .filter_map(|p| {
                let p = regex::escape(p);
                // Up to a space, a quote or a `:` (a line number, or a sentence going on).
                regex::Regex::new(&format!(r#"{p}(?:[/\\][^\s"'`,;:()\[\]<>|]*)?"#)).ok()
            })
            .collect();
        let mut trek_folders: Vec<_> = trek_folders
            .into_iter()
            .filter(|(f, _)| f.len() > 1)
            .collect();
        trek_folders.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
        Scrubber {
            home: home.filter(|h| h.len() > 1),
            trek_folders,
            projects,
            secrets,
            shapes,
        }
    }

    /// This user's: their home, Trek's folders, the secrets this run knows, and `projects`.
    pub fn here(projects: &[String], shapes: fn(&str) -> String) -> Scrubber {
        let data = trek_core::paths::data_dir();
        let logs = trek_logging::log_dir();
        Scrubber::new(
            Some(trek_core::paths::home().display().to_string()),
            vec![
                (logs.display().to_string(), "[Trek logs]"),
                (data.display().to_string(), "[Trek data]"),
            ],
            projects,
            trek_core::session_secrets::known().as_ref().clone(),
            shapes,
        )
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut text = trek_core::session_secrets::scrub(text, &self.secrets).into_owned();
        text = (self.shapes)(&text);
        text = EMAIL.replace_all(&text, "[email]").into_owned();
        for project in &self.projects {
            text = project.replace_all(&text, "[path]").into_owned();
        }
        for (folder, name) in &self.trek_folders {
            text = text.replace(folder.as_str(), name);
        }
        // A crate's sources, on whichever machine built it, are the crate's: `gpui-0.2/src/x.rs`.
        text = REGISTRY.replace_all(&text, "").into_owned();
        let home = self.home.as_deref();
        text = UNIX_PATH
            .replace_all(&text, |c: &regex::Captures| {
                format!("{}{}", &c[1], unix_path(&c[2], home))
            })
            .into_owned();
        text = WINDOWS_PATH
            .replace_all(&text, |c: &regex::Captures| {
                format!("{}{}", &c[1], windows_path(&c[2], home))
            })
            .into_owned();
        text = HOME_PATH
            .replace_all(&text, |c: &regex::Captures| {
                format!("{}~{}[path]", &c[1], &c[2])
            })
            .into_owned();
        // The home folder where no path was seen (`HOME=/Users/me`).
        match home {
            Some(h) => text.replace(h, "~"),
            None => text,
        }
    }

    /// `scrub` for each line, which never makes one line two.
    pub fn lines(&self, lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|l| self.scrub(l).replace('\n', " "))
            .collect()
    }
}

/// Where the system's own files are: they name nothing of the user's.
const SYSTEM: &[&str] = &[
    "/usr/",
    "/bin/",
    "/sbin/",
    "/opt/homebrew/",
    "/System/",
    "/Library/",
    "/Applications/",
    "/rustc/",
    "/dev/",
];

fn unix_path(path: &str, home: Option<&str>) -> String {
    if SYSTEM.iter().any(|s| path.starts_with(s)) {
        return path.to_string();
    }
    match home {
        Some(h) if path == h => "~".into(),
        Some(h) if path.starts_with(&format!("{h}/")) => "~/[path]".into(),
        _ => "[path]".into(),
    }
}

fn windows_path(path: &str, home: Option<&str>) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(r"c:\windows\") {
        return path.to_string();
    }
    match home.map(str::to_ascii_lowercase) {
        Some(h) if lower == h => "~".into(),
        Some(h) if lower.starts_with(&format!("{h}\\")) => r"~\[path]".into(),
        _ => "[path]".into(),
    }
}

static EMAIL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")
        .unwrap()
});

/// A crate in cargo's registry: everything up to the crate's own folder goes.
static REGISTRY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:[A-Za-z]:)?[^\s:()\[\]<>]*[/\\]\.cargo[/\\]registry[/\\]src[/\\][^/\\\s]+[/\\]",
    )
    .unwrap()
});

/// An absolute path of two or more parts, where a path can start (as `share::render` has it).
static UNIX_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=:,*])(/[^\s/()\[\]<>"'`,;:*|]+(?:/[^\s/()\[\]<>"'`,;:*|]*)+)"#,
    )
    .unwrap()
});

static WINDOWS_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(^|[\s(\[<"'`=,*])([A-Za-z]:\\[^\s\\()\[\]<>"'`,;*|]+(?:\\[^\s\\()\[\]<>"'`,;*|]*)+)"#,
    )
    .unwrap()
});

/// A path already written from the home folder (`~/code/app`).
static HOME_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(^|[\s(\[<"'`=:,*])~([/\\])[^\s()\[\]<>"'`,;*|]+"#).unwrap()
});

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the app's shape matcher: takes out `sk-…` words.
    fn shapes(text: &str) -> String {
        text.split(' ')
            .map(|w| {
                if w.starts_with("sk-") {
                    "[redacted]"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn scrubber() -> Scrubber {
        Scrubber::new(
            Some("/Users/me".into()),
            vec![
                ("/Users/me/Library/Logs/Trek".into(), "[Trek logs]"),
                (
                    "/Users/me/Library/Application Support/dev.trek.Trek".into(),
                    "[Trek data]",
                ),
            ],
            &[
                "/Users/me/code/acme-secret".into(),
                "/Volumes/work/My Client/".into(),
            ],
            vec![("GITHUB_TOKEN".into(), "hunter2-value".into())],
            shapes,
        )
    }

    #[test]
    fn paths_say_where_without_naming_the_users_files() {
        let s = scrubber();
        // A project's folder never shows, nor what's in it.
        assert_eq!(
            s.scrub("open /Users/me/code/acme-secret/src/plan.rs: not found"),
            "open [path]: not found"
        );
        // Spaces and all; and no file's name anywhere else either.
        assert_eq!(
            s.scrub(
                "read /Volumes/work/My Client/notes.md, /Users/me/Desktop/taxes.pdf and /tmp/x/y"
            ),
            "read [path], ~/[path] and [path]"
        );
        assert_eq!(s.scrub("cwd ~/code/app/src"), "cwd ~/[path]");
        assert_eq!(s.scrub("HOME=/Users/me"), "HOME=~");
        // Trek's own folders, spaces and all, and the system's, stay readable.
        assert_eq!(
            s.scrub(
                "database: /Users/me/Library/Application Support/dev.trek.Trek/trek.sqlite locked"
            ),
            "database: [Trek data]/trek.sqlite locked"
        );
        assert_eq!(
            s.scrub("ran /opt/homebrew/bin/codex and /usr/bin/git"),
            "ran /opt/homebrew/bin/codex and /usr/bin/git"
        );
        // A crate's sources are named from the crate, on whichever machine built them.
        assert_eq!(
            s.scrub("at /Users/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.7/src/window.rs:12"),
            "at gpui-pre-0.3.7/src/window.rs:12"
        );
        // Trek's own source paths are relative, and say nothing of the user.
        assert_eq!(
            s.scrub("panicked at crates/trek-app/src/root.rs:120:5"),
            "panicked at crates/trek-app/src/root.rs:120:5"
        );
    }

    #[test]
    fn windows_paths_too() {
        let s = Scrubber::new(
            Some(r"C:\Users\me".into()),
            vec![],
            &[r"D:\work\client".into()],
            vec![],
            shapes,
        );
        assert_eq!(
            s.scrub(r"open C:\Users\me\code\app\x.rs failed"),
            r"open ~\[path] failed"
        );
        assert_eq!(
            s.scrub(r"open D:\work\client\x.rs failed"),
            r"open [path] failed"
        );
        assert_eq!(s.scrub(r"open E:\other\x.rs failed"), r"open [path] failed");
        assert_eq!(
            s.scrub(r"C:\Windows\System32\cmd.exe"),
            r"C:\Windows\System32\cmd.exe"
        );
    }

    #[test]
    fn secrets_and_emails_never_leave() {
        let s = scrubber();
        let line = s.scrub(
            "token hunter2-value from me@example.com, key sk-abc123 and ann.lee+x@mail.co.uk",
        );
        assert_eq!(
            line,
            "token [GITHUB_TOKEN hidden] from [email], key [redacted] and [email]"
        );
        // Lines stay lines.
        assert_eq!(
            s.lines(&["a\nb hunter2-value".into()]),
            ["a b [GITHUB_TOKEN hidden]"]
        );
    }
}
