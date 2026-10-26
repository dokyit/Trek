//! A thread's link: a secret GitHub gist made with the GitHub CLI (`gh gist create`, never
//! `--public`), which anyone with its URL can open and no one can find. Trek remembers each
//! thread's gist (`shared-links.json` in the data folder) so the Share sheet shows it again, can
//! update it (`gh gist edit`) and delete it (`gh gist delete --yes`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A thread's gist, as remembered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gist {
    pub id: String,
    pub url: String,
    /// The file in it (`<slug>.md`), which an update replaces.
    pub filename: String,
    /// When it was made or last updated (unix ms).
    pub at: i64,
}

fn file() -> PathBuf {
    trek_core::paths::data_dir().join("shared-links.json")
}

fn load_from(path: &Path) -> BTreeMap<String, Gist> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The gist made for thread `id`, if one was.
pub fn remembered(id: &str) -> Option<Gist> {
    load_from(&file()).remove(id)
}

/// Remember `gist` for thread `id` (none: forget it).
pub fn remember(id: &str, gist: Option<Gist>) {
    let path = file();
    let mut all = load_from(&path);
    match gist {
        Some(g) => all.insert(id.to_string(), g),
        None => all.remove(id),
    };
    let saved = serde_json::to_string_pretty(&all)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(&path, json));
    if let Err(e) = saved {
        tracing::warn!("save {}: {e}", path.display());
    }
}

/// What's asked of `gh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// A new secret gist of one file, its text on stdin.
    Create {
        filename: String,
        description: String,
    },
    /// Replace the gist's file with `source` (a file: `gh gist edit` reads no stdin).
    Edit {
        id: String,
        filename: String,
        source: PathBuf,
    },
    Delete {
        id: String,
    },
}

/// `gh`'s arguments for `op`.
pub fn args(op: &Op) -> Vec<String> {
    let s = |v: &str| v.to_string();
    match op {
        // Secret is gh's default; `--public` is never passed.
        Op::Create {
            filename,
            description,
        } => vec![
            s("gist"),
            s("create"),
            s("--filename"),
            filename.clone(),
            s("-d"),
            description.clone(),
            s("-"),
        ],
        Op::Edit {
            id,
            filename,
            source,
        } => vec![
            s("gist"),
            s("edit"),
            id.clone(),
            s("--filename"),
            filename.clone(),
            source.display().to_string(),
        ],
        Op::Delete { id } => vec![s("gist"), s("delete"), id.clone(), s("--yes")],
    }
}

/// The description a new gist gets.
pub fn description(title: &str) -> String {
    format!("{title} — shared from Trek")
}

/// The gist's URL in what `gh gist create` printed (its last line).
pub fn url_in(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("https://"))
        .map(str::to_string)
}

/// A gist's id: the last part of its URL.
pub fn id_of(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// What went wrong, in words with the fix, from what `gh` said.
pub fn explain(stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if lower.contains("gh auth login")
        || lower.contains("not logged")
        || lower.contains("authentication")
    {
        "GitHub CLI isn't signed in. Run `gh auth login` in a terminal, then try again.".into()
    } else if lower.contains("gist") && (lower.contains("scope") || lower.contains("403")) {
        "Your GitHub CLI login can't make gists. Run `gh auth refresh -s gist`, then try again."
            .into()
    } else if lower.contains("404") || lower.contains("not found") {
        "GitHub doesn't have that gist any more.".into()
    } else {
        let line = stderr.trim().lines().last().unwrap_or("").trim();
        if line.is_empty() {
            "GitHub CLI couldn't do it.".into()
        } else {
            format!("GitHub CLI said: {line}")
        }
    }
}

/// When `gh` isn't there at all.
pub const NO_GH: &str =
    "GitHub CLI (gh) isn't installed. Install it from cli.github.com, then run `gh auth login`.";

/// Run `gh` (`command`, set up to run it) with `op`'s arguments and `stdin`; what it printed, or
/// why it failed. Blocks: call it off the UI thread.
pub fn run(
    mut command: std::process::Command,
    op: &Op,
    stdin: Option<&str>,
) -> Result<String, String> {
    command
        .args(args(op))
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // No prompts: gh would wait for a terminal that isn't there.
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => NO_GH.to_string(),
        _ => format!("Couldn't run GitHub CLI: {e}"),
    })?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())
            .map_err(|e| format!("Couldn't hand the thread to GitHub CLI: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("GitHub CLI didn't finish: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(explain(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// `gh`, found as Trek finds it for pull requests, run from the home folder.
pub fn gh() -> Result<std::process::Command, String> {
    crate::pr_card::gh(&trek_core::paths::home()).ok_or_else(|| NO_GH.to_string())
}

/// Make a secret gist of `markdown`.
pub fn create(
    command: std::process::Command,
    title: &str,
    filename: &str,
    markdown: &str,
) -> Result<Gist, String> {
    let op = Op::Create {
        filename: filename.to_string(),
        description: description(title),
    };
    let out = run(command, &op, Some(markdown))?;
    let url = url_in(&out).ok_or_else(|| "GitHub CLI made no link.".to_string())?;
    Ok(Gist {
        id: id_of(&url),
        url,
        filename: filename.to_string(),
        at: trek_core::store::now_ms(),
    })
}

/// Replace `gist`'s file with `markdown`.
pub fn update(command: std::process::Command, gist: &Gist, markdown: &str) -> Result<Gist, String> {
    let dir = std::env::temp_dir().join(format!("trek-share-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let source = dir.join(&gist.filename);
    std::fs::write(&source, markdown).map_err(|e| e.to_string())?;
    let op = Op::Edit {
        id: gist.id.clone(),
        filename: gist.filename.clone(),
        source: source.clone(),
    };
    let result = run(command, &op, None);
    let _ = std::fs::remove_file(&source);
    result.map(|_| Gist {
        at: trek_core::store::now_ms(),
        ..gist.clone()
    })
}

/// Delete `gist` on GitHub.
pub fn delete(command: std::process::Command, gist: &Gist) -> Result<(), String> {
    run(
        command,
        &Op::Delete {
            id: gist.id.clone(),
        },
        None,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_lines() {
        let create = Op::Create {
            filename: "fix-the-bug.md".into(),
            description: description("Fix the bug"),
        };
        assert_eq!(
            args(&create),
            [
                "gist",
                "create",
                "--filename",
                "fix-the-bug.md",
                "-d",
                "Fix the bug — shared from Trek",
                "-"
            ]
        );
        assert!(!args(&create).iter().any(|a| a == "--public" || a == "-p"));
        assert_eq!(
            args(&Op::Delete { id: "abc".into() }),
            ["gist", "delete", "abc", "--yes"]
        );
        assert_eq!(
            args(&Op::Edit {
                id: "abc".into(),
                filename: "x.md".into(),
                source: "/tmp/x.md".into()
            }),
            ["gist", "edit", "abc", "--filename", "x.md", "/tmp/x.md"]
        );
    }

    #[test]
    fn urls_ids_and_errors() {
        let out = "- Creating gist fix.md\n✓ Created secret gist fix.md\nhttps://gist.github.com/dokyit/0123abcd\n";
        assert_eq!(
            url_in(out).as_deref(),
            Some("https://gist.github.com/dokyit/0123abcd")
        );
        assert_eq!(id_of("https://gist.github.com/dokyit/0123abcd"), "0123abcd");
        assert!(
            explain("To get started with GitHub CLI, please run:  gh auth login")
                .contains("Run `gh auth login`")
        );
        assert!(explain("HTTP 404: Not Found").contains("doesn't have that gist"));
        assert_eq!(explain("boom\n"), "GitHub CLI said: boom");
    }

    /// A stand-in `gh` that writes its arguments and stdin where the test reads them, then
    /// answers as `gh` would.
    #[cfg(unix)]
    fn fake_gh(dir: &Path, reply: &str, code: i32) -> std::process::Command {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("gh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{d}/args\"\ncat > \"{d}/stdin\"\nprintf '%s' '{reply}' >&{fd}\nexit {code}\n",
                d = dir.display(),
                fd = if code == 0 { 1 } else { 2 },
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::process::Command::new(script)
    }

    #[cfg(unix)]
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-gist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn create_hands_gh_the_markdown_and_keeps_the_link() {
        let dir = temp_dir("create");
        let gh = fake_gh(&dir, "https://gist.github.com/me/feed1234\n", 0);
        let gist = create(gh, "Fix the bug", "fix-the-bug.md", "# Fix the bug\n").unwrap();
        assert_eq!(gist.url, "https://gist.github.com/me/feed1234");
        assert_eq!(gist.id, "feed1234");
        assert_eq!(gist.filename, "fix-the-bug.md");
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(
            args,
            "gist\ncreate\n--filename\nfix-the-bug.md\n-d\nFix the bug — shared from Trek\n-\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("stdin")).unwrap(),
            "# Fix the bug\n"
        );

        let failing = fake_gh(
            &dir,
            "To get started with GitHub CLI, please run:  gh auth login",
            4,
        );
        let err = create(failing, "T", "t.md", "x").unwrap_err();
        assert!(err.contains("Run `gh auth login`"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn update_and_delete_name_the_gist() {
        let dir = temp_dir("edit");
        let gist = Gist {
            id: "feed1234".into(),
            url: "https://gist.github.com/me/feed1234".into(),
            filename: "t.md".into(),
            at: 0,
        };
        let updated = update(fake_gh(&dir, "", 0), &gist, "new text").unwrap();
        assert_eq!(updated.id, "feed1234");
        assert!(updated.at > 0);
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert!(
            args.starts_with("gist\nedit\nfeed1234\n--filename\nt.md\n"),
            "{args}"
        );
        delete(fake_gh(&dir, "", 0), &gist).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("args")).unwrap(),
            "gist\ndelete\nfeed1234\n--yes\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remembered_per_thread() {
        let dir = std::env::temp_dir().join(format!("trek-gist-file-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("shared-links.json");
        let mut all = BTreeMap::new();
        all.insert(
            "t1".to_string(),
            Gist {
                id: "a".into(),
                url: "https://gist.github.com/me/a".into(),
                filename: "a.md".into(),
                at: 1,
            },
        );
        std::fs::write(&path, serde_json::to_string(&all).unwrap()).unwrap();
        assert_eq!(load_from(&path).get("t1").map(|g| g.id.as_str()), Some("a"));
        assert!(load_from(&dir.join("missing.json")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
