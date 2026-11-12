//! A project's verification skill (pstack's "verification is all you need"): a skill that lets
//! an agent drive and check the app on its own, with a small CLI, dev-environment notes and a
//! Feature Map. Trek ships the guides that build and maintain one (`skills::SHIPPED`), finds the
//! skill a project has, tells every agent in the project about it, and notices when a turn ran
//! its CLI.

use crate::settings::Verification;
use crate::store::{Item, ToolStatus};
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A skill older than this is due for maintenance, and a reminder (when asked for) comes this
/// often at most.
pub const WEEK_MS: i64 = 7 * DAY_MS;

/// Where projects keep skills of their own, in the order they're looked through.
const SKILL_DIRS: [&str; 2] = [".agents/skills", ".claude/skills"];

/// The front matter key, under `metadata`, that marks a verification skill, and its value.
const MARK: (&str, &str) = ("trek", "verification");

/// A verification skill found in a project.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub dir: PathBuf,
    pub name: String,
    /// How to run its CLI from the project folder.
    pub cli: Option<String>,
}

/// Every `key: value` in a SKILL.md's front matter, nested ones (under `metadata:`) included.
fn front_matter(text: &str) -> Vec<(String, String)> {
    let Some(rest) = text.strip_prefix("---") else {
        return vec![];
    };
    rest.lines()
        .skip(1)
        .take_while(|l| l.trim() != "---")
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| {
            (
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            )
        })
        .filter(|(k, _)| !k.is_empty() && !k.starts_with('#'))
        .collect()
}

/// The project's verification skill: one marked as such in its front matter (`metadata: trek:
/// verification`), else one named like one ("verify-app", "control-app") that has a Feature Map.
/// A name alone isn't enough: "verification-before-completion" or "verify-pr" are checklists,
/// not a way to drive the app.
pub fn find(project: &Path) -> Option<Found> {
    let mut found: Vec<(bool, Found)> = vec![];
    for root in SKILL_DIRS {
        let Ok(entries) = std::fs::read_dir(project.join(root)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("SKILL.md").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                continue;
            };
            let fields = front_matter(&text);
            let field = |k: &str| {
                fields
                    .iter()
                    .find(|(key, v)| key == k && !v.is_empty())
                    .map(|(_, v)| v.clone())
            };
            let folder = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = field("name").unwrap_or(folder);
            let marked = field(MARK.0).as_deref() == Some(MARK.1);
            let named = (name.contains("verif") || name == "control-app")
                && dir.join("references/features/README.md").is_file();
            if marked || named {
                let cli = field("cli").or_else(|| bundled_cli(&dir, project));
                found.push((marked, Found { dir, name, cli }));
            }
        }
    }
    found.sort_by_key(|(marked, _)| !marked);
    found.into_iter().next().map(|(_, f)| f)
}

/// An executable in the skill's `scripts/` or `bin/`, as run from the project folder.
fn bundled_cli(dir: &Path, project: &Path) -> Option<String> {
    ["scripts", "bin"].iter().find_map(|sub| {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(sub))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.metadata().is_ok_and(|m| is_executable(p, &m)))
            .collect();
        files.sort();
        let file = files.into_iter().next()?;
        let relative = file.strip_prefix(project).unwrap_or(&file);
        // Forward slashes on every platform: shells on Windows take them too.
        let parts: Vec<_> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        Some(format!("./{}", parts.join("/")))
    })
}

/// Whether `path` (a regular file with this `metadata`) is a program: it has an execute bit.
#[cfg(unix)]
fn is_executable(_: &Path, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

/// Whether `path` (a regular file with this `metadata`) is a program: Windows has no execute bit,
/// so it's a file with a program's extension (one of `%PATHEXT%`, case-insensitively, or
/// PowerShell's `.ps1`), or a script that names what runs it on its first line (`#!/bin/sh`), as
/// the guide's `scripts/app` does: Git Bash, which Claude Code runs commands in there, runs it.
#[cfg(windows)]
fn is_executable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    use std::io::Read as _;
    const FALLBACK: &str = ".exe;.cmd;.bat;.com;.ps1";
    if !metadata.is_file() {
        return false;
    }
    let shebang = || {
        let mut start = [0u8; 2];
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut start))
            .is_ok_and(|_| &start == b"#!")
    };
    let Some(ext) = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
    else {
        return shebang();
    };
    let pathext = std::env::var("PATHEXT")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| FALLBACK.to_string());
    ext == "ps1"
        || pathext
            .split(';')
            .filter_map(|e| e.trim().strip_prefix('.'))
            .any(|e| e.eq_ignore_ascii_case(&ext))
        || shebang()
}

/// When a skill's folder last changed, as far as Trek can tell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Changed {
    /// The latest commit that touched it (its author date, which checkouts, pulls and rebases
    /// leave alone): a change someone made.
    Committed(i64),
    /// Not committed: the newest file in it. A checkout rewrites these, so they only date a
    /// skill Trek hasn't seen before.
    Touched(i64),
}

/// When the skill in `dir` last changed (ms).
pub fn last_change(dir: &Path) -> Option<Changed> {
    let committed = crate::worktree::git(dir, &["log", "-1", "--format=%at", "--", "."])
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    committed
        .map(|s| Changed::Committed(s * 1000))
        .or_else(|| newest_file(dir).map(Changed::Touched))
}

/// When the newest file in `dir` was modified (ms).
fn newest_file(dir: &Path) -> Option<i64> {
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((at, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > 2_000 {
                break;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() && depth < 4 && !e.file_name().to_string_lossy().starts_with('.') {
                stack.push((e.path(), depth + 1));
            } else if let Ok(m) = meta.modified() {
                newest = Some(newest.map_or(m, |n| n.max(m)));
            }
        }
    }
    newest
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// What Trek records about a skill it found, keeping the user's choices. It was last maintained
/// when Trek saw a Maintain run finish, or when a commit last changed it; a skill Trek knows
/// nothing about yet dates from its newest file.
pub fn record(
    found: &Found,
    before: Option<&Verification>,
    changed: Option<Changed>,
) -> Verification {
    let known = before.and_then(|b| b.maintained_at);
    let maintained = match changed {
        Some(Changed::Committed(at)) => known.max(Some(at)),
        Some(Changed::Touched(at)) => known.or(Some(at)),
        None => known,
    };
    Verification {
        skill: found.dir.display().to_string(),
        name: found.name.clone(),
        cli: found.cli.clone(),
        maintained_at: maintained,
        remind_weekly: before.is_some_and(|b| b.remind_weekly),
        reminded_at: before.and_then(|b| b.reminded_at),
    }
}

/// Whether it's a week or more since the skill was maintained.
pub fn due(v: &Verification, now: i64) -> bool {
    v.maintained_at.is_none_or(|at| now - at >= WEEK_MS)
}

/// Whether to remind the user to maintain it now: they asked to be, it's due, and they haven't
/// been reminded this week.
pub fn remind_now(v: &Verification, now: i64) -> bool {
    v.remind_weekly && due(v, now) && v.reminded_at.is_none_or(|at| now - at >= WEEK_MS)
}

/// What every agent working in `project` is told about the skill, for one working in `cwd`: the
/// project folder or a worktree of it. A worktree is told to use its own copy of the skill; one
/// without a copy (a skill not committed yet) reads the main checkout's but runs its CLI from its
/// own folder, so it checks its own changes.
pub fn instructions(v: &Verification, project: &Path, cwd: &Path) -> String {
    let skill = Path::new(&v.skill);
    let rel = skill
        .strip_prefix(project)
        .ok()
        .filter(|r| cwd.join(r).join("SKILL.md").is_file());
    let (place, cli) = match rel {
        Some(rel) => (
            format!("in `{}` in your working folder", rel.display()),
            v.cli.clone(),
        ),
        None => {
            let away = if cwd == project {
                String::new()
            } else {
                " (this working folder has no copy of it yet)".into()
            };
            (
                format!("in {}{away}", crate::paths::tildify_for_agents(skill)),
                v.cli.as_deref().map(|c| absolute(c, project, cwd)),
            )
        }
    };
    let cli = cli.map(|c| format!(" and drive and check the app with its CLI (`{c}`, run from your working folder) rather than throwaway scripts")).unwrap_or_default();
    format!(
        "This project has a verification skill, “{}”, {place}. Before you call a change done, verify it with the skill: read its SKILL.md{cli}. Its Feature Map, references/features/README.md, says what each feature does and how to reach it.",
        v.name
    )
}

/// The CLI `cli` (run from `project`) with a program that `cwd` lacks named by its full path, in
/// the platform's own separators (`C:\repo\.agents\skills\…` on Windows, as its shells take it).
fn absolute(cli: &str, project: &Path, cwd: &Path) -> String {
    let mut words: Vec<String> = cli.split_whitespace().map(String::from).collect();
    if let Some(w) = words.iter_mut().find(|w| w.starts_with("./")) {
        let rel = &w[2..];
        if !cwd.join(rel).exists() {
            let full = rel
                .split('/')
                .filter(|part| !part.is_empty())
                .fold(project.to_path_buf(), |path, part| path.join(part));
            *w = full.display().to_string();
        }
    }
    words.join(" ")
}

/// The message that starts building a project's verification skill, following `guide`.
pub fn setup_prompt(guide: &Path) -> String {
    format!(
        "Set up a verification skill for this project, following Trek's guide in `{}`: an agent-friendly CLI to drive and debug the app, notes on setting up the dev environment, and a Feature Map of what the app does and how to reach each feature. Then show me it working.",
        guide.display()
    )
}

/// The message that brings the skill in `skill` up to date, following `guide`.
pub fn maintain_prompt(guide: &Path, skill: &Path, has_cli: bool) -> String {
    let missing = if has_cli {
        ""
    } else {
        " It names no CLI yet, so Trek can't tell when an agent checked its work with it: build one as the guide describes, and name it in the front matter's `metadata` as `cli:`."
    };
    format!(
        "Maintain this project's verification skill in `{}`, following Trek's guide in `{}`: run it, fix what's broken, and bring the CLI, the dev-environment notes and the Feature Map up to date with the app.{missing}",
        crate::paths::tildify_for_agents(skill),
        guide.display()
    )
}

/// What a command line running the CLI `cli` contains: the program it starts with
/// (`./scripts/app check` → `scripts/app`, `npx tsx tools/app.ts` → `tools/app.ts`), else the
/// whole line but its trailing flags (`make verify` → itself, `appctl --json` → `appctl`): a
/// program run through another (make, cargo, swift) is only that program with its arguments.
pub fn needle(cli: &str) -> Option<String> {
    needle_on(cli, cfg!(windows))
}

/// `needle` for Windows (`windows`), where a path may be written with backslashes
/// (`.\scripts\app.cmd`), or for macOS, where a backslash is part of a name.
fn needle_on(cli: &str, windows: bool) -> Option<String> {
    let cli = if windows {
        cli.replace('\\', "/")
    } else {
        cli.to_string()
    };
    let words: Vec<&str> = cli.split_whitespace().collect();
    // A path is the program when no flag comes before it (`make -C tools/x check` isn't).
    let path = words
        .iter()
        .position(|w| w.contains('/'))
        .filter(|&i| !words[..i].iter().any(|w| w.starts_with('-')));
    if let Some(i) = path {
        return Some(words[i].trim_start_matches("./").to_string());
    }
    let end = words.iter().rposition(|w| !w.starts_with('-'))? + 1;
    Some(words[..end].join(" "))
}

/// The simple commands in a shell command line, each as its words (quotes taken off): split at
/// `;`, `&&`, `||`, `|`, `&`, parentheses, backticks and new lines, outside quotes. Comments are
/// left out. On Windows (`windows`) a backslash before a name (`.\scripts\app.cmd`, `C:\repo`) is
/// a folder separator, read as `/`, as PowerShell and cmd.exe take it (anywhere else it escapes,
/// as in Git Bash), and PowerShell's braces (`& { … }`) split commands too.
fn simple_commands(line: &str, windows: bool) -> Vec<Vec<String>> {
    let (mut out, mut words, mut word) = (vec![], vec![], String::new());
    let (mut quote, mut quoted, mut chars) = (None::<char>, false, line.chars().peekable());
    let end_word = |words: &mut Vec<String>, word: &mut String, quoted: &mut bool| {
        if !word.is_empty() || *quoted {
            words.push(std::mem::take(word));
        }
        *quoted = false;
    };
    let separates = |next: Option<&char>| {
        windows && next.is_some_and(|&n| n.is_alphanumeric() || matches!(n, '.' | '_' | '-' | '~'))
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (_, '\\') if separates(chars.peek()) => word.push('/'),
            (Some('"'), '\\') => word.extend(chars.next()),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                quoted = true;
            }
            (None, '\\') => word.extend(chars.next().filter(|&n| n != '\n')),
            (None, '#') if word.is_empty() => while chars.next_if(|&n| n != '\n').is_some() {},
            (None, c) if c.is_whitespace() && c != '\n' => {
                end_word(&mut words, &mut word, &mut quoted)
            }
            (None, ';' | '&' | '|' | '(' | ')' | '`' | '\n') => {
                end_word(&mut words, &mut word, &mut quoted);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
            }
            (None, '{' | '}') if windows => {
                end_word(&mut words, &mut word, &mut quoted);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
            }
            (None, c) => word.push(c),
        }
    }
    end_word(&mut words, &mut word, &mut quoted);
    if !words.is_empty() {
        out.push(words);
    }
    out
}

/// Words that come before the program a simple command runs: shell keywords and commands that
/// run the rest of the line as it is.
const PREFIXES: [&str; 16] = [
    "!", "{", "if", "then", "else", "elif", "do", "while", "until", "time", "exec", "command",
    "builtin", "nohup", "sudo", "env",
];

/// Programs that run a script or package named after them (`sh x`, `npx tsx x.ts`, `uv run x`,
/// `powershell -File x.ps1`), or a command line given them (`cmd /c …`, `pwsh -c …`).
const RUNNERS: [&str; 25] = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "node",
    "deno",
    "bun",
    "bunx",
    "npx",
    "pnpx",
    "tsx",
    "ts-node",
    "python",
    "python3",
    "uv",
    "ruby",
    "perl",
    "php",
    "swift",
    "run",
    "exec",
    "pwsh",
    "powershell",
    "cmd",
];

/// What Windows runs for a program named without its extension (`.\scripts\app` is
/// `scripts\app.cmd`): `path` without one of those, lowercased as the caller has it.
fn without_program_extension(path: &str) -> &str {
    [".exe", ".cmd", ".bat", ".com", ".ps1"]
        .iter()
        .find_map(|ext| path.strip_suffix(ext))
        .unwrap_or(path)
}

/// Where a simple command (from its program on) goes, if it changes folder: `cd`, `pushd`, and on
/// Windows (`windows`) PowerShell's `Set-Location` and `sl` and cmd.exe's `chdir` and `cd /d`.
/// `Some(None)` for one that names no folder (`cd` alone goes home).
fn changes_into(words: &[String], windows: bool) -> Option<Option<&String>> {
    let first = words.first()?;
    let cd = if windows {
        matches!(
            first.to_lowercase().as_str(),
            "cd" | "chdir" | "pushd" | "set-location" | "sl"
        )
    } else {
        first == "cd" || first == "pushd"
    };
    cd.then(|| {
        words[1..]
            .iter()
            .find(|w| !w.starts_with('-') && !(windows && w.eq_ignore_ascii_case("/d")))
    })
}

/// A simple command's words from its program on: past variables set for it (`FOO=1`), the
/// prefixes above and their flags, and `timeout`'s duration.
fn program(words: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let assigns = w.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        });
        if assigns
            || PREFIXES.contains(&w.as_str())
            || (i > 0 && w.starts_with('-') && PREFIXES.contains(&words[i - 1].as_str()))
        {
            i += 1;
        } else if w == "timeout" {
            i += 1;
            while let Some(w) = words.get(i).filter(|w| w.starts_with('-')) {
                // `-s SIGNAL` and `-k DURATION` take the next word as their value.
                i += if matches!(w.as_str(), "-s" | "-k" | "--signal" | "--kill-after") {
                    2
                } else {
                    1
                };
            }
            i += words
                .get(i)
                .is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit()))
                as usize;
        } else {
            break;
        }
    }
    &words[i.min(words.len())..]
}

/// Whether the command line `command` runs the CLI `needle` stands for: as the program of one
/// of its simple commands (`cd app && make verify`, `FOO=1 ./scripts/app check`), through a
/// runner (`sh scripts/app`, `npx tsx tools/app.ts`, `bash -c "./scripts/app check"`), or from a
/// folder on its path (`cd .agents/skills/x && ./scripts/app`). Naming it to another program
/// (`cat`, `sed -n`, `chmod +x`, `git diff`, `echo`) or to a shell that only parses it
/// (`bash -n`) isn't running it. On Windows, as PowerShell and cmd.exe write it too
/// (`.\scripts\app check`, `powershell -Command "…"`, `cmd /c …`).
pub fn runs(needle: &str, command: &str) -> bool {
    !invocations(needle, command, cfg!(windows)).is_empty()
}

/// The arguments of each run of the CLI `needle` stands for in `command` (see `runs`), read as
/// Windows' shells write it (`windows`) or as the Mac's do.
fn invocations(needle: &str, command: &str, windows: bool) -> Vec<Vec<String>> {
    let target: Vec<&str> = needle.split_whitespace().collect();
    // A name that is `path`: on Windows in any case, and without its extension
    // (`.\scripts\app` runs `scripts\app.cmd`).
    let same = |w: &str, path: &str| {
        if !windows {
            return w == path;
        }
        let (w, path) = (w.to_lowercase(), path.to_lowercase());
        w == path || w == without_program_extension(&path)
    };
    // `path` itself, wherever it's run from: `./scripts/app`, `/repo/scripts/app`.
    let is = |w: &str, path: &str| {
        let w = w.trim_end_matches('/');
        let w = w.strip_prefix("./").unwrap_or(w);
        same(w, path) || w.match_indices('/').any(|(i, _)| same(&w[i + 1..], path))
    };
    let mut out = Vec::new();
    let mut cwd: Option<String> = None;
    for words in simple_commands(command, windows) {
        let words = program(&words);
        if words.is_empty() {
            continue;
        }
        if let Some(to) = changes_into(words, windows) {
            cwd = to.cloned();
            continue;
        }
        if target.len() > 1 {
            // A program run through another (make, cargo, swift): that command line.
            if words.len() >= target.len() && words.iter().zip(&target).all(|(w, t)| w == t) {
                out.push(words[target.len()..].to_vec());
            }
            continue;
        }
        let mut rest = words;
        while let Some((w, after)) = rest.split_first() {
            let from_cwd = cwd.as_deref().is_some_and(|d| {
                needle.match_indices('/').any(|(i, _)| {
                    is(d, &needle[..i]) && same(w.strip_prefix("./").unwrap_or(w), &needle[i + 1..])
                })
            });
            if is(w, needle) || from_cwd {
                out.push(after.to_vec());
                break;
            }
            let name = w.rsplit('/').next().unwrap_or(w);
            let name = if windows {
                let lower = name.to_lowercase();
                lower
                    .strip_suffix(".exe")
                    .map(str::to_string)
                    .unwrap_or(lower)
            } else {
                name.to_string()
            };
            if !RUNNERS.contains(&name.as_str()) {
                break;
            }
            if matches!(name.as_str(), "sh" | "bash" | "zsh" | "dash") {
                // `bash -n` parses the script without running it.
                if noexec(after) {
                    break;
                }
                // A shell given a command line (`bash -lc "…"`) runs that.
                if let Some(at) = after
                    .iter()
                    .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))
                {
                    if let Some(line) = after.get(at + 1) {
                        out.extend(invocations(needle, line, windows));
                    }
                    break;
                }
            }
            if matches!(name.as_str(), "pwsh" | "powershell") {
                // `-Command` (`-c`, in any case, or any shortening) runs the rest of the line;
                // `-File` (`-f`) the script after it, past whatever options come first.
                let flag =
                    |a: &str, full: &str| a.len() >= 2 && full.starts_with(&a.to_lowercase());
                if let Some(at) = after.iter().position(|a| flag(a, "-command")) {
                    out.extend(invocations(needle, &after[at + 1..].join(" "), windows));
                    break;
                }
                if let Some(at) = after.iter().position(|a| flag(a, "-file")) {
                    rest = &after[at + 1..];
                    continue;
                }
            }
            if name == "cmd" {
                // `/c` (or `/k`) runs the rest of the line.
                if let Some(at) = after
                    .iter()
                    .position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))
                {
                    out.extend(invocations(needle, &after[at + 1..].join(" "), windows));
                    break;
                }
            }
            rest = after;
            while rest.first().is_some_and(|w| w.starts_with('-')) {
                rest = &rest[1..];
            }
        }
    }
    out
}

/// Whether a shell's options (the words after it) tell it to read commands without running
/// them: `-n`, `-xn`, `--noexec`, `-o noexec`.
fn noexec(options: &[String]) -> bool {
    let mut words = options.iter();
    while let Some(w) = words.next() {
        match w.as_str() {
            "-o" | "+o" => {
                if words.next().is_some_and(|o| o == "noexec") {
                    return w == "-o";
                }
            }
            "--noexec" => return true,
            w if w.starts_with("--") => {}
            w if w.starts_with('-') && w.len() > 1 => {
                if w.contains('n') {
                    return true;
                }
                // `-c` takes the command line next: nothing past it is an option.
                if w.contains('c') {
                    return false;
                }
            }
            _ => return false,
        }
    }
    false
}

/// Whether a run of the CLI with `args` only asks it something (`--help`, `help`, `--version`)
/// or shows what it would do (`--dry-run`): it checks nothing.
fn inspects(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--help" | "-h" | "--version" | "-V" | "--dry-run"
        )
    }) || args
        .iter()
        .find(|a| !a.starts_with('-'))
        .is_some_and(|a| a == "help")
}

/// What a run of the CLI with `args` checks, for telling a failure from a later pass of the same
/// thing: its subcommand (`check`, `ui`), or nothing for a bare run.
fn subcommand(args: &[String]) -> &str {
    args.iter()
        .find(|a| !a.starts_with('-'))
        .map_or("", |a| a.as_str())
}

/// Whether a command line goes into `dir` (or a folder in it) with `cd` (or, on Windows, as
/// PowerShell and cmd.exe do, in any case).
fn goes_into(command: &str, dir: &Path, windows: bool) -> bool {
    simple_commands(command, windows).iter().any(|words| {
        changes_into(program(words), windows)
            .flatten()
            .is_some_and(|d| {
                let d = d.trim_end_matches('/');
                if windows {
                    Path::new(&d.to_lowercase())
                        .starts_with(dir.to_string_lossy().to_lowercase().replace('\\', "/"))
                } else {
                    Path::new(d).starts_with(dir)
                }
            })
    })
}

/// How Trek tells a turn ran the project's verification CLI: what a command running it contains
/// (`needle`), and, for a thread working outside the main checkout (a worktree), that checkout:
/// a run there checks the main checkout's code, not the thread's.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub needle: String,
    pub main: Option<PathBuf>,
}

/// How a turn used the verification CLI: the commands that ran it, and whether it passed: every
/// subcommand that failed ran again and passed later in the turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub commands: Vec<String>,
    pub passed: bool,
}

/// Whether the tool calls in `turn` (one turn's items) ran the CLI `probe` stands for. Reading
/// the skill isn't verifying: only running its CLI counts, where the thread works, and not to ask
/// it for help or a dry run.
pub fn verdict(turn: &[Item], probe: &Probe) -> Option<Verdict> {
    verdict_on(turn, probe, cfg!(windows))
}

/// `verdict` with commands read as Windows' shells write them (`windows`) or as the Mac's do.
fn verdict_on(turn: &[Item], probe: &Probe, windows: bool) -> Option<Verdict> {
    let here = |d: &str| {
        probe
            .main
            .as_deref()
            .is_none_or(|m| !goes_into(d, m, windows))
    };
    let mut commands = Vec::new();
    // Each subcommand's last result: a failed `check` isn't made good by a `--help` or `logs`.
    let mut last: Vec<(String, ToolStatus)> = Vec::new();
    for item in turn {
        let Item::Tool {
            title,
            detail,
            status,
            ..
        } = item
        else {
            continue;
        };
        if !is_command(title) || *status == ToolStatus::Running || !here(detail) {
            continue;
        }
        let Some(args) = invocations(&probe.needle, detail, windows)
            .into_iter()
            .find(|a| !inspects(a))
        else {
            continue;
        };
        commands.push(detail.clone());
        let what = subcommand(&args).to_string();
        last.retain(|(w, _)| *w != what);
        last.push((what, *status));
    }
    (!commands.is_empty()).then(|| Verdict {
        passed: last.iter().all(|(_, s)| *s == ToolStatus::Done),
        commands,
    })
}

/// Whether a tool row is a shell command, as agents title them.
fn is_command(title: &str) -> bool {
    title.starts_with("Run")
        || title.starts_with("Ran")
        || matches!(title, "Bash" | "Shell" | "Terminal" | "exec")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-verify-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill(project: &Path, root: &str, folder: &str, front: &str) -> PathBuf {
        let dir = project.join(root).join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\n{front}\n---\n\n# Skill\n"),
        )
        .unwrap();
        dir
    }

    #[test]
    fn finds_the_marked_skill_and_its_cli() {
        let p = project("find");
        assert_eq!(find(&p), None);
        skill(
            &p,
            ".claude/skills",
            "review",
            "name: review\ndescription: Review code.",
        );
        assert_eq!(find(&p), None, "not a verification skill");
        // Named like one, with a script of its own: only a checklist until it has a Feature Map.
        let named = skill(
            &p,
            ".claude/skills",
            "verify-app",
            "name: verify-app\ndescription: Check the app.",
        );
        std::fs::create_dir_all(named.join("scripts")).unwrap();
        std::fs::write(named.join("scripts/notes.txt"), "not a program").unwrap();
        // Windows has no execute bit: a program is a file with a program's extension.
        let (app, cli) = if cfg!(windows) {
            (named.join("scripts/app.cmd"), "app.cmd")
        } else {
            (named.join("scripts/app"), "app")
        };
        std::fs::write(&app, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&app, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        skill(
            &p,
            ".claude/skills",
            "verification-before-completion",
            "name: verification-before-completion\ndescription: Check before you say done.",
        );
        assert_eq!(find(&p), None, "a name alone isn't a verification skill");
        std::fs::create_dir_all(named.join("references/features")).unwrap();
        std::fs::write(named.join("references/features/README.md"), "# Features\n").unwrap();
        let found = find(&p).unwrap();
        assert_eq!(
            (found.name.as_str(), found.cli.as_deref()),
            (
                "verify-app",
                Some(format!("./.claude/skills/verify-app/scripts/{cli}").as_str())
            )
        );
        // One marked in its front matter wins, with the CLI it names.
        let marked = skill(
            &p,
            ".agents/skills",
            "control",
            "name: control\ndescription: \"Drive it.\"\nmetadata:\n  trek: verification\n  cli: ./tools/app --json",
        );
        let found = find(&p).unwrap();
        assert_eq!(
            found,
            Found {
                dir: marked,
                name: "control".into(),
                cli: Some("./tools/app --json".into())
            }
        );
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn records_when_it_was_last_maintained() {
        let p = project("record");
        let dir = skill(
            &p,
            ".agents/skills",
            "control-app",
            "name: control-app\nmetadata:\n  trek: verification",
        );
        let found = find(&p).unwrap();
        // Not committed: dated by its newest file, the first time Trek sees it.
        let Some(Changed::Touched(touched)) = last_change(&dir) else {
            panic!("{:?}", last_change(&dir))
        };
        assert!((crate::store::now_ms() - touched).abs() < 60_000);
        let v = record(&found, None, Some(Changed::Touched(touched)));
        assert_eq!(
            (v.maintained_at, v.remind_weekly, v.name.as_str()),
            (Some(touched), false, "control-app")
        );
        // A later Maintain run counts; the user's choices stay.
        let before = Verification {
            maintained_at: Some(touched - 10 * DAY_MS),
            remind_weekly: true,
            reminded_at: Some(5),
            ..v.clone()
        };
        let again = record(&found, Some(&before), Some(Changed::Touched(touched)));
        assert_eq!(
            (again.maintained_at, again.remind_weekly, again.reminded_at),
            (Some(touched - 10 * DAY_MS), true, Some(5)),
            "files a checkout rewrote aren't maintenance"
        );
        assert_eq!(
            record(&found, Some(&before), None).maintained_at,
            Some(touched - 10 * DAY_MS),
            "not looked at yet"
        );
        // A commit that changed it is.
        assert_eq!(
            record(&found, Some(&before), Some(Changed::Committed(touched))).maintained_at,
            Some(touched)
        );
        assert_eq!(
            record(&found, Some(&before), Some(Changed::Committed(5))).maintained_at,
            Some(touched - 10 * DAY_MS)
        );
        // Committed, it's dated by the commit, whatever the files say.
        let git = |args: &[&str]| crate::worktree::git(&p, args).unwrap();
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "skill",
            "--date",
            "2020-01-02T00:00:00Z",
        ]);
        assert_eq!(
            last_change(&dir),
            Some(Changed::Committed(1_577_923_200_000))
        );
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn due_weekly_and_reminded_once_a_week() {
        let now = 100 * WEEK_MS;
        let mut v = Verification {
            maintained_at: Some(now - 2 * DAY_MS),
            ..Default::default()
        };
        assert!(!due(&v, now));
        v.maintained_at = Some(now - WEEK_MS);
        assert!(due(&v, now) && !remind_now(&v, now), "not asked for");
        v.remind_weekly = true;
        assert!(remind_now(&v, now));
        v.reminded_at = Some(now - DAY_MS);
        assert!(!remind_now(&v, now), "reminded this week");
        v.reminded_at = Some(now - WEEK_MS);
        assert!(remind_now(&v, now));
    }

    #[test]
    fn commands_that_run_the_cli_are_recognised() {
        assert_eq!(
            needle("./scripts/app check").as_deref(),
            Some("scripts/app")
        );
        assert_eq!(
            needle("npx tsx tools/app.ts").as_deref(),
            Some("tools/app.ts")
        );
        assert_eq!(
            needle("cargo run -p xtask --").as_deref(),
            Some("cargo run -p xtask")
        );
        assert_eq!(needle("make verify").as_deref(), Some("make verify"));
        assert_eq!(
            needle("make -C tools/verify check").as_deref(),
            Some("make -C tools/verify check"),
            "the path isn't the program"
        );
        assert_eq!(needle("trekctl --json").as_deref(), Some("trekctl"));
        assert_eq!(needle("  "), None);
        assert!(runs(
            "scripts/app",
            "cd /repo && ./scripts/app check --json"
        ));
        assert!(runs("scripts/app", "/repo/scripts/app status"));
        assert!(runs(
            "scripts/app",
            "APP_ENV=test ./scripts/app check 2>&1 | tail -5"
        ));
        assert!(runs("scripts/app", "timeout 60 sh scripts/app check"));
        // `timeout`'s -s and -k take a value of their own.
        assert!(runs(
            "scripts/app",
            "timeout -s KILL 60 sh scripts/app check"
        ));
        assert!(runs("scripts/app", "timeout -k 5 60 sh scripts/app check"));
        assert!(runs(
            "scripts/app",
            "timeout --signal=KILL --kill-after 5 60 sh scripts/app check"
        ));
        assert!(runs(
            "scripts/app",
            "bash -lc 'cd /repo && ./scripts/app check'"
        ));
        assert!(runs("scripts/app", "bash -x scripts/app check"));
        assert!(runs(
            "scripts/app",
            "bash -o pipefail -c './scripts/app check | tail'"
        ));
        assert!(runs("scripts/app", "out=$(./scripts/app check --json)"));
        assert!(runs("tools/app.ts", "npx tsx tools/app.ts check"));
        assert!(!runs("scripts/app", "cat scripts/apple.txt"));
        assert!(!runs("scripts/app", "cat myscripts/app"));
        // Naming it to another program isn't running it.
        let cli = ".agents/skills/verify-app/scripts/app";
        for read in [
            "sed -n '1,200p' .agents/skills/verify-app/scripts/app",
            "cat .agents/skills/verify-app/scripts/app",
            "chmod +x .agents/skills/verify-app/scripts/app",
            "git diff -- .agents/skills/verify-app/scripts/app",
            "echo 'run ./.agents/skills/verify-app/scripts/app later'",
            "ls -la .agents/skills/verify-app/scripts/app && wc -l .agents/skills/verify-app/scripts/app",
            "# ./.agents/skills/verify-app/scripts/app check",
            "bash -n .agents/skills/verify-app/scripts/app.bak",
            "bash -n .agents/skills/verify-app/scripts/app",
            "sh -xn ./.agents/skills/verify-app/scripts/app",
            "zsh --noexec .agents/skills/verify-app/scripts/app",
            "bash -o noexec .agents/skills/verify-app/scripts/app",
            "bash -n -c './.agents/skills/verify-app/scripts/app check'",
        ] {
            assert!(!runs(cli, read), "{read}");
        }
        assert!(runs("trekctl", "trekctl open settings"));
        assert!(runs("trekctl", "(cd x; /usr/local/bin/trekctl check)"));
        assert!(!runs("trekctl", "echo trekctl-old"));
        assert!(!runs("trekctl", "which trekctl"));
        assert!(runs("cargo run -p xtask", "cargo  run -p xtask -- check"));
        // A program run through make (or swift, just, docker) is only that target.
        assert!(runs("make verify", "make verify"));
        assert!(runs("make verify", "cd app && make verify ARGS=check"));
        assert!(!runs("make verify", "make build"));
        assert!(!runs("make verify", "make verify-all"));
        assert!(!runs("make verify", "echo 'run make verify later'"));
        assert!(!runs(
            "make verify",
            "grep -n verify Makefile # make verify"
        ));
        assert!(!runs("swift run appctl", "swift build"));
        // From a folder on its path.
        assert!(runs(
            cli,
            "cd .agents/skills/verify-app && ./scripts/app check"
        ));
        assert!(runs(
            cli,
            "cd /wt/.agents/skills/verify-app/scripts; ./app check"
        ));
        assert!(!runs(cli, "cd .agents/skills/verify-app && cat SKILL.md"));
        assert!(!runs(
            cli,
            "cd .agents/skills/verify-app && cat scripts/app"
        ));
        assert!(
            !runs(cli, "./scripts/app check"),
            "another app's scripts/app"
        );
    }

    /// Commands as Windows' shells write them: PowerShell and cmd.exe take a backslash as a folder
    /// separator, find `app.cmd` for `app`, ignore case, and run a command line given them. Read
    /// with Windows' rules on every platform; the Mac's own reading is the test above.
    #[test]
    fn windows_commands_that_run_the_cli_are_recognised() {
        let on = |needle: &str, command: &str| !invocations(needle, command, true).is_empty();
        let cli = ".agents/skills/verify-app/scripts/app.cmd";
        assert_eq!(
            needle_on(r".\.agents\skills\verify-app\scripts\app.cmd check", true).as_deref(),
            Some(cli)
        );
        assert_eq!(
            needle_on("./.agents/skills/verify-app/scripts/app.cmd", true).as_deref(),
            Some(cli)
        );
        for command in [
            r".\.agents\skills\verify-app\scripts\app.cmd check --json",
            r".\.agents\skills\verify-app\scripts\app check",
            r"C:\repo\.agents\skills\verify-app\scripts\app.cmd check",
            r".\.Agents\Skills\Verify-App\Scripts\App.CMD check",
            r"& .\.agents\skills\verify-app\scripts\app.cmd check",
            r"& '.\.agents\skills\verify-app\scripts\app.cmd' check",
            r#"powershell.exe -NoProfile -Command ".\.agents\skills\verify-app\scripts\app.cmd check""#,
            r#"pwsh -c "& {.\.agents\skills\verify-app\scripts\app.cmd check}""#,
            r#"cmd /d /s /c ".\.agents\skills\verify-app\scripts\app.cmd check""#,
            r"cd .agents\skills\verify-app; .\scripts\app.cmd check",
            r"Set-Location C:\repo\.agents\skills\verify-app\scripts; .\app check",
            "./.agents/skills/verify-app/scripts/app.cmd check",
        ] {
            assert!(on(cli, command), "{command}");
        }
        for command in [
            r"Get-Content .\.agents\skills\verify-app\scripts\app.cmd",
            r"type .agents\skills\verify-app\scripts\app.cmd",
            r".\.agents\skills\verify-app\scripts\app.cmd.bak check",
            r".\other\scripts\app.cmd check",
            r#"powershell -Command "Get-Content .\.agents\skills\verify-app\scripts\app.cmd""#,
        ] {
            assert!(!on(cli, command), "{command}");
        }
        // PowerShell's -File runs the script after it, past the options before it.
        assert!(on(
            "scripts/app.ps1",
            r"powershell -ExecutionPolicy Bypass -File .\scripts\app.ps1 check"
        ));
        // A Git Bash escape is still one: `\"` inside double quotes.
        assert!(on(
            "scripts/app",
            r#"bash -c "./scripts/app check \"a b\"""#
        ));
        // On the Mac a backslash escapes: `.\scripts\app` is `.scriptsapp` there.
        assert!(invocations("scripts/app", r".\scripts\app check", false).is_empty());

        // A thread in a worktree that goes into the main checkout, the PowerShell way, hasn't
        // checked its own changes; from its own folder it has.
        let run = |detail: &str| Item::Tool {
            id: "t".into(),
            title: "Run command".into(),
            detail: detail.into(),
            output: String::new(),
            status: ToolStatus::Done,
        };
        let away = Probe {
            needle: "scripts/app.cmd".into(),
            main: Some(r"C:\Repo".into()),
        };
        assert_eq!(
            verdict_on(&[run(r"cd c:\repo; .\scripts\app.cmd check")], &away, true),
            None
        );
        assert_eq!(
            verdict_on(
                &[run(r"Set-Location C:\Repo\src; ..\scripts\app check")],
                &away,
                true
            ),
            None
        );
        assert!(verdict_on(&[run(r".\scripts\app check")], &away, true).is_some_and(|v| v.passed));
    }

    /// Windows has no execute bit: a script that names what runs it (`#!/bin/sh`, as the guide's
    /// `scripts/app`) is a program, as a file with a program's extension is; a text file isn't.
    #[cfg(windows)]
    #[test]
    fn a_script_that_names_its_interpreter_is_a_program_on_windows() {
        let p = project("shebang");
        let file = |name: &str, body: &str| {
            let path = p.join(name);
            std::fs::write(&path, body).unwrap();
            let metadata = path.metadata().unwrap();
            is_executable(&path, &metadata)
        };
        assert!(file("app", "#!/bin/sh\necho ok\n"));
        assert!(file("app.sh", "#!/usr/bin/env bash\n"));
        assert!(file("app.cmd", "@echo off\r\n"));
        assert!(file("app.ps1", "Write-Output ok\r\n"));
        assert!(!file("notes", "# not a program\n"));
        assert!(!file("notes.txt", "not a program\n"));
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn a_turn_is_verified_by_running_the_cli_not_reading_the_skill() {
        let tool = |title: &str, detail: &str, status: ToolStatus| Item::Tool {
            id: "t".into(),
            title: title.into(),
            detail: detail.into(),
            output: String::new(),
            status,
        };
        let probe = Probe {
            needle: "scripts/app".into(),
            main: None,
        };
        let read = tool(
            "Read",
            "/repo/.agents/skills/control-app/scripts/app",
            ToolStatus::Done,
        );
        assert_eq!(verdict(&[read.clone()], &probe), None);
        // Codex reads through shell commands: still a read.
        assert_eq!(
            verdict(
                &[tool(
                    "Run command",
                    "sed -n '1,200p' scripts/app",
                    ToolStatus::Done
                )],
                &probe
            ),
            None
        );
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        let passed = tool(
            "Run command",
            "./scripts/app check --json",
            ToolStatus::Done,
        );
        assert_eq!(
            verdict(&[read.clone(), failed.clone()], &probe),
            Some(Verdict {
                commands: vec!["./scripts/app check".into()],
                passed: false
            })
        );
        let v = verdict(&[failed, read, passed.clone()], &probe).unwrap();
        assert!(v.passed && v.commands.len() == 2, "the last run counts");
        assert_eq!(
            verdict(
                &[tool(
                    "Run command",
                    "./scripts/app check",
                    ToolStatus::Running
                )],
                &probe
            ),
            None,
            "still running"
        );
        // A failed check stays failed through help, dry runs and other subcommands that work…
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        for after in [
            "./scripts/app --help",
            "./scripts/app help check",
            "./scripts/app check --help",
            "./scripts/app logs --since 1m",
            "./scripts/app reset --dry-run",
        ] {
            let v = verdict(
                &[failed.clone(), tool("Run command", after, ToolStatus::Done)],
                &probe,
            )
            .unwrap();
            assert!(!v.passed, "{after}");
        }
        // …until the check itself passes.
        let fixed = [
            failed.clone(),
            tool("Run command", "./scripts/app logs", ToolStatus::Done),
            tool("Run command", "./scripts/app check", ToolStatus::Done),
        ];
        assert!(verdict(&fixed, &probe).is_some_and(|v| v.passed && v.commands.len() == 3));
        // Asking it for help alone checks nothing.
        assert_eq!(
            verdict(
                &[tool(
                    "Run command",
                    "./scripts/app --help",
                    ToolStatus::Done
                )],
                &probe
            ),
            None
        );
        assert_eq!(
            verdict(
                &[tool(
                    "Run command",
                    "./scripts/app --help && ./scripts/app check",
                    ToolStatus::Done
                )],
                &probe
            )
            .map(|v| v.passed),
            Some(true)
        );
        // A thread in a worktree that checks the main checkout hasn't checked its own changes.
        let away = Probe {
            main: Some("/repo".into()),
            ..probe
        };
        assert_eq!(
            verdict(
                &[tool(
                    "Run command",
                    "cd /repo && ./scripts/app check",
                    ToolStatus::Done
                )],
                &away
            ),
            None
        );
        assert!(verdict(&[passed], &away).is_some_and(|v| v.passed));
    }

    #[test]
    fn agents_are_told_where_it_is() {
        let p = project("tell");
        let dir = skill(&p, ".agents/skills", "control-app", "name: control-app");
        let v = Verification {
            skill: dir.display().to_string(),
            name: "control-app".into(),
            cli: Some("./.agents/skills/control-app/scripts/app".into()),
            ..Default::default()
        };
        let text = instructions(&v, &p, &p);
        assert!(
            text.contains(&format!(
                "“control-app”, in `{}` in your working folder",
                Path::new(".agents/skills").join("control-app").display()
            )) && text.contains(
                "(`./.agents/skills/control-app/scripts/app`, run from your working folder)"
            ) && text.contains("references/features/README.md"),
            "{text}"
        );
        assert!(
            !instructions(
                &Verification {
                    cli: None,
                    ..v.clone()
                },
                &p,
                &p
            )
            .contains("CLI")
        );
        // A worktree with the skill committed uses its own copy.
        let wt = project("tell-wt");
        skill(&wt, ".agents/skills", "control-app", "name: control-app");
        assert_eq!(instructions(&v, &p, &wt), text);
        // One without a copy reads the main checkout's, and runs its CLI by full path from its own folder.
        let bare = project("tell-bare");
        let text = instructions(&v, &p, &bare);
        // In the platform's own separators, as its shells take it.
        let cli = p
            .join(".agents")
            .join("skills")
            .join("control-app")
            .join("scripts")
            .join("app");
        assert!(
            text.contains("has no copy of it yet")
                && text.contains(&format!(
                    "(`{}`, run from your working folder)",
                    cli.display()
                )),
            "{text}"
        );
        assert!(
            setup_prompt(Path::new("/d/skills/create-verification-skill/SKILL.md"))
                .contains("`/d/skills/create-verification-skill/SKILL.md`")
        );
        // Maintaining one with no CLI named asks for one.
        let guide = Path::new("/d/skills/maintain-verification-skill/SKILL.md");
        assert!(!maintain_prompt(guide, &dir, true).contains("names no CLI"));
        assert!(
            maintain_prompt(guide, &dir, false)
                .contains("name it in the front matter's `metadata` as `cli:`")
        );
        for d in [p, wt, bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
