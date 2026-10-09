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
    let Some(rest) = text.strip_prefix("---") else { return vec![] };
    rest.lines()
        .skip(1)
        .take_while(|l| l.trim() != "---")
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string()))
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
        let Ok(entries) = std::fs::read_dir(project.join(root)) else { continue };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else { continue };
            let fields = front_matter(&text);
            let field = |k: &str| fields.iter().find(|(key, v)| key == k && !v.is_empty()).map(|(_, v)| v.clone());
            let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let name = field("name").unwrap_or(folder);
            let marked = field(MARK.0).as_deref() == Some(MARK.1);
            let named = (name.contains("verif") || name == "control-app") && dir.join("references/features/README.md").is_file();
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
        let parts: Vec<_> = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect();
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
/// so it's the extension, one of `%PATHEXT%` (case-insensitively).
#[cfg(windows)]
fn is_executable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    const FALLBACK: &str = ".exe;.cmd;.bat;.com;.ps1";
    if !metadata.is_file() {
        return false;
    }
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()) else { return false };
    let pathext = std::env::var("PATHEXT").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| FALLBACK.to_string());
    pathext.split(';').filter_map(|e| e.trim().strip_prefix('.')).any(|e| e.eq_ignore_ascii_case(&ext))
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
    let committed = crate::worktree::git(dir, &["log", "-1", "--format=%at", "--", "."]).ok().and_then(|s| s.trim().parse::<i64>().ok());
    committed.map(|s| Changed::Committed(s * 1000)).or_else(|| newest_file(dir).map(Changed::Touched))
}

/// When the newest file in `dir` was modified (ms).
fn newest_file(dir: &Path) -> Option<i64> {
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((at, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
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
    newest.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64)
}

/// What Trek records about a skill it found, keeping the user's choices. It was last maintained
/// when Trek saw a Maintain run finish, or when a commit last changed it; a skill Trek knows
/// nothing about yet dates from its newest file.
pub fn record(found: &Found, before: Option<&Verification>, changed: Option<Changed>) -> Verification {
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
    let rel = skill.strip_prefix(project).ok().filter(|r| cwd.join(r).join("SKILL.md").is_file());
    let (place, cli) = match rel {
        Some(rel) => (format!("in `{}` in your working folder", rel.display()), v.cli.clone()),
        None => {
            let away = if cwd == project { String::new() } else { " (this working folder has no copy of it yet)".into() };
            (format!("in {}{away}", crate::paths::tildify(skill)), v.cli.as_deref().map(|c| absolute(c, project, cwd)))
        }
    };
    let cli = cli.map(|c| format!(" and drive and check the app with its CLI (`{c}`, run from your working folder) rather than throwaway scripts")).unwrap_or_default();
    format!(
        "This project has a verification skill, “{}”, {place}. Before you call a change done, verify it with the skill: read its SKILL.md{cli}. Its Feature Map, references/features/README.md, says what each feature does and how to reach it.",
        v.name
    )
}

/// The CLI `cli` (run from `project`) with a program that `cwd` lacks named by its full path.
fn absolute(cli: &str, project: &Path, cwd: &Path) -> String {
    let mut words: Vec<String> = cli.split_whitespace().map(String::from).collect();
    if let Some(w) = words.iter_mut().find(|w| w.starts_with("./")) {
        let rel = &w[2..];
        if !cwd.join(rel).exists() {
            *w = project.join(rel).display().to_string();
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
    let missing = if has_cli { "" } else { " It names no CLI yet, so Trek can't tell when an agent checked its work with it: build one as the guide describes, and name it in the front matter's `metadata` as `cli:`." };
    format!(
        "Maintain this project's verification skill in `{}`, following Trek's guide in `{}`: run it, fix what's broken, and bring the CLI, the dev-environment notes and the Feature Map up to date with the app.{missing}",
        crate::paths::tildify(skill),
        guide.display()
    )
}

/// What a command line running the CLI `cli` contains: the program it starts with
/// (`./scripts/app check` → `scripts/app`, `npx tsx tools/app.ts` → `tools/app.ts`), else the
/// whole line but its trailing flags (`make verify` → itself, `appctl --json` → `appctl`): a
/// program run through another (make, cargo, swift) is only that program with its arguments.
pub fn needle(cli: &str) -> Option<String> {
    let words: Vec<&str> = cli.split_whitespace().collect();
    // A path is the program when no flag comes before it (`make -C tools/x check` isn't).
    let path = words.iter().position(|w| w.contains('/')).filter(|&i| !words[..i].iter().any(|w| w.starts_with('-')));
    if let Some(i) = path {
        return Some(words[i].trim_start_matches("./").to_string());
    }
    let end = words.iter().rposition(|w| !w.starts_with('-'))? + 1;
    Some(words[..end].join(" "))
}

/// The simple commands in a shell command line, each as its words (quotes taken off): split at
/// `;`, `&&`, `||`, `|`, `&`, parentheses, backticks and new lines, outside quotes. Comments are
/// left out.
fn simple_commands(line: &str) -> Vec<Vec<String>> {
    let (mut out, mut words, mut word) = (vec![], vec![], String::new());
    let (mut quote, mut quoted, mut chars) = (None::<char>, false, line.chars().peekable());
    let end_word = |words: &mut Vec<String>, word: &mut String, quoted: &mut bool| {
        if !word.is_empty() || *quoted {
            words.push(std::mem::take(word));
        }
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => word.extend(chars.next()),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                quoted = true;
            }
            (None, '\\') => word.extend(chars.next().filter(|&n| n != '\n')),
            (None, '#') if word.is_empty() => {
                while chars.next_if(|&n| n != '\n').is_some() {}
            }
            (None, c) if c.is_whitespace() && c != '\n' => end_word(&mut words, &mut word, &mut quoted),
            (None, ';' | '&' | '|' | '(' | ')' | '`' | '\n') => {
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
const PREFIXES: [&str; 16] = ["!", "{", "if", "then", "else", "elif", "do", "while", "until", "time", "exec", "command", "builtin", "nohup", "sudo", "env"];

/// Programs that run a script or package named after them (`sh x`, `npx tsx x.ts`, `uv run x`).
const RUNNERS: [&str; 22] = [
    "sh", "bash", "zsh", "dash", "fish", "node", "deno", "bun", "bunx", "npx", "pnpx", "tsx", "ts-node", "python", "python3", "uv", "ruby", "perl", "php", "swift", "run", "exec",
];

/// A simple command's words from its program on: past variables set for it (`FOO=1`), the
/// prefixes above and their flags, and `timeout`'s duration.
fn program(words: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let assigns = w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()));
        if assigns || PREFIXES.contains(&w.as_str()) || (i > 0 && w.starts_with('-') && PREFIXES.contains(&words[i - 1].as_str())) {
            i += 1;
        } else if w == "timeout" {
            i += 1;
            while words.get(i).is_some_and(|w| w.starts_with('-')) {
                i += 1;
            }
            i += words.get(i).is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit())) as usize;
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
/// (`bash -n`) isn't running it.
pub fn runs(needle: &str, command: &str) -> bool {
    !invocations(needle, command).is_empty()
}

/// The arguments of each run of the CLI `needle` stands for in `command` (see `runs`).
fn invocations(needle: &str, command: &str) -> Vec<Vec<String>> {
    let target: Vec<&str> = needle.split_whitespace().collect();
    // `path` itself, wherever it's run from: `./scripts/app`, `/repo/scripts/app`.
    let is = |w: &str, path: &str| {
        let w = w.trim_end_matches('/');
        let w = w.strip_prefix("./").unwrap_or(w);
        w == path || w.ends_with(&format!("/{path}"))
    };
    let mut out = Vec::new();
    let mut cwd: Option<String> = None;
    for words in simple_commands(command) {
        let words = program(&words);
        let Some(first) = words.first() else { continue };
        if first == "cd" {
            cwd = words.get(1).cloned();
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
                needle.match_indices('/').any(|(i, _)| is(d, &needle[..i]) && w.strip_prefix("./").unwrap_or(w) == &needle[i + 1..])
            });
            if is(w, needle) || from_cwd {
                out.push(after.to_vec());
                break;
            }
            let name = w.rsplit('/').next().unwrap_or(w);
            if !RUNNERS.contains(&name) {
                break;
            }
            if matches!(name, "sh" | "bash" | "zsh" | "dash") {
                // `bash -n` parses the script without running it.
                if noexec(after) {
                    break;
                }
                // A shell given a command line (`bash -lc "…"`) runs that.
                if let Some(at) = after.iter().position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c')) {
                    if let Some(line) = after.get(at + 1) {
                        out.extend(invocations(needle, line));
                    }
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
    args.iter().any(|a| matches!(a.as_str(), "--help" | "-h" | "--version" | "-V" | "--dry-run"))
        || args.iter().find(|a| !a.starts_with('-')).is_some_and(|a| a == "help")
}

/// What a run of the CLI with `args` checks, for telling a failure from a later pass of the same
/// thing: its subcommand (`check`, `ui`), or nothing for a bare run.
fn subcommand(args: &[String]) -> &str {
    args.iter().find(|a| !a.starts_with('-')).map_or("", |a| a.as_str())
}

/// Whether a command line goes into `dir` (or a folder in it) with `cd`.
fn goes_into(command: &str, dir: &Path) -> bool {
    simple_commands(command).iter().any(|words| {
        let words = program(words);
        words.first().is_some_and(|w| w == "cd") && words.get(1).is_some_and(|d| Path::new(d.trim_end_matches('/')).starts_with(dir))
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
    let here = |d: &str| probe.main.as_deref().is_none_or(|m| !goes_into(d, m));
    let mut commands = Vec::new();
    // Each subcommand's last result: a failed `check` isn't made good by a `--help` or `logs`.
    let mut last: Vec<(String, ToolStatus)> = Vec::new();
    for item in turn {
        let Item::Tool { title, detail, status, .. } = item else { continue };
        if !is_command(title) || *status == ToolStatus::Running || !here(detail) {
            continue;
        }
        let Some(args) = invocations(&probe.needle, detail).into_iter().find(|a| !inspects(a)) else { continue };
        commands.push(detail.clone());
        let what = subcommand(&args).to_string();
        last.retain(|(w, _)| *w != what);
        last.push((what, *status));
    }
    (!commands.is_empty()).then(|| Verdict { passed: last.iter().all(|(_, s)| *s == ToolStatus::Done), commands })
}

/// Whether a tool row is a shell command, as agents title them.
fn is_command(title: &str) -> bool {
    title.starts_with("Run") || title.starts_with("Ran") || matches!(title, "Bash" | "Shell" | "Terminal" | "exec")
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
        std::fs::write(dir.join("SKILL.md"), format!("---\n{front}\n---\n\n# Skill\n")).unwrap();
        dir
    }

    #[test]
    fn finds_the_marked_skill_and_its_cli() {
        let p = project("find");
        assert_eq!(find(&p), None);
        skill(&p, ".claude/skills", "review", "name: review\ndescription: Review code.");
        assert_eq!(find(&p), None, "not a verification skill");
        // Named like one, with a script of its own: only a checklist until it has a Feature Map.
        let named = skill(&p, ".claude/skills", "verify-app", "name: verify-app\ndescription: Check the app.");
        std::fs::create_dir_all(named.join("scripts")).unwrap();
        std::fs::write(named.join("scripts/notes.txt"), "not a program").unwrap();
        // Windows has no execute bit: a program is a file with a program's extension.
        let (app, cli) = if cfg!(windows) { (named.join("scripts/app.cmd"), "app.cmd") } else { (named.join("scripts/app"), "app") };
        std::fs::write(&app, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&app, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        skill(&p, ".claude/skills", "verification-before-completion", "name: verification-before-completion\ndescription: Check before you say done.");
        assert_eq!(find(&p), None, "a name alone isn't a verification skill");
        std::fs::create_dir_all(named.join("references/features")).unwrap();
        std::fs::write(named.join("references/features/README.md"), "# Features\n").unwrap();
        let found = find(&p).unwrap();
        assert_eq!((found.name.as_str(), found.cli.as_deref()), ("verify-app", Some(format!("./.claude/skills/verify-app/scripts/{cli}").as_str())));
        // One marked in its front matter wins, with the CLI it names.
        let marked = skill(&p, ".agents/skills", "control", "name: control\ndescription: \"Drive it.\"\nmetadata:\n  trek: verification\n  cli: ./tools/app --json");
        let found = find(&p).unwrap();
        assert_eq!(found, Found { dir: marked, name: "control".into(), cli: Some("./tools/app --json".into()) });
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn records_when_it_was_last_maintained() {
        let p = project("record");
        let dir = skill(&p, ".agents/skills", "control-app", "name: control-app\nmetadata:\n  trek: verification");
        let found = find(&p).unwrap();
        // Not committed: dated by its newest file, the first time Trek sees it.
        let Some(Changed::Touched(touched)) = last_change(&dir) else { panic!("{:?}", last_change(&dir)) };
        assert!((crate::store::now_ms() - touched).abs() < 60_000);
        let v = record(&found, None, Some(Changed::Touched(touched)));
        assert_eq!((v.maintained_at, v.remind_weekly, v.name.as_str()), (Some(touched), false, "control-app"));
        // A later Maintain run counts; the user's choices stay.
        let before = Verification { maintained_at: Some(touched - 10 * DAY_MS), remind_weekly: true, reminded_at: Some(5), ..v.clone() };
        let again = record(&found, Some(&before), Some(Changed::Touched(touched)));
        assert_eq!((again.maintained_at, again.remind_weekly, again.reminded_at), (Some(touched - 10 * DAY_MS), true, Some(5)), "files a checkout rewrote aren't maintenance");
        assert_eq!(record(&found, Some(&before), None).maintained_at, Some(touched - 10 * DAY_MS), "not looked at yet");
        // A commit that changed it is.
        assert_eq!(record(&found, Some(&before), Some(Changed::Committed(touched))).maintained_at, Some(touched));
        assert_eq!(record(&found, Some(&before), Some(Changed::Committed(5))).maintained_at, Some(touched - 10 * DAY_MS));
        // Committed, it's dated by the commit, whatever the files say.
        let git = |args: &[&str]| crate::worktree::git(&p, args).unwrap();
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "skill", "--date", "2020-01-02T00:00:00Z"]);
        assert_eq!(last_change(&dir), Some(Changed::Committed(1_577_923_200_000)));
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn due_weekly_and_reminded_once_a_week() {
        let now = 100 * WEEK_MS;
        let mut v = Verification { maintained_at: Some(now - 2 * DAY_MS), ..Default::default() };
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
        assert_eq!(needle("./scripts/app check").as_deref(), Some("scripts/app"));
        assert_eq!(needle("npx tsx tools/app.ts").as_deref(), Some("tools/app.ts"));
        assert_eq!(needle("cargo run -p xtask --").as_deref(), Some("cargo run -p xtask"));
        assert_eq!(needle("make verify").as_deref(), Some("make verify"));
        assert_eq!(needle("make -C tools/verify check").as_deref(), Some("make -C tools/verify check"), "the path isn't the program");
        assert_eq!(needle("trekctl --json").as_deref(), Some("trekctl"));
        assert_eq!(needle("  "), None);
        assert!(runs("scripts/app", "cd /repo && ./scripts/app check --json"));
        assert!(runs("scripts/app", "/repo/scripts/app status"));
        assert!(runs("scripts/app", "APP_ENV=test ./scripts/app check 2>&1 | tail -5"));
        assert!(runs("scripts/app", "timeout 60 sh scripts/app check"));
        assert!(runs("scripts/app", "bash -lc 'cd /repo && ./scripts/app check'"));
        assert!(runs("scripts/app", "bash -x scripts/app check"));
        assert!(runs("scripts/app", "bash -o pipefail -c './scripts/app check | tail'"));
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
        assert!(!runs("make verify", "grep -n verify Makefile # make verify"));
        assert!(!runs("swift run appctl", "swift build"));
        // From a folder on its path.
        assert!(runs(cli, "cd .agents/skills/verify-app && ./scripts/app check"));
        assert!(runs(cli, "cd /wt/.agents/skills/verify-app/scripts; ./app check"));
        assert!(!runs(cli, "cd .agents/skills/verify-app && cat SKILL.md"));
        assert!(!runs(cli, "cd .agents/skills/verify-app && cat scripts/app"));
        assert!(!runs(cli, "./scripts/app check"), "another app's scripts/app");
    }

    #[test]
    fn a_turn_is_verified_by_running_the_cli_not_reading_the_skill() {
        let tool = |title: &str, detail: &str, status: ToolStatus| Item::Tool { id: "t".into(), title: title.into(), detail: detail.into(), output: String::new(), status };
        let probe = Probe { needle: "scripts/app".into(), main: None };
        let read = tool("Read", "/repo/.agents/skills/control-app/scripts/app", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone()], &probe), None);
        // Codex reads through shell commands: still a read.
        assert_eq!(verdict(&[tool("Run command", "sed -n '1,200p' scripts/app", ToolStatus::Done)], &probe), None);
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        let passed = tool("Run command", "./scripts/app check --json", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone(), failed.clone()], &probe), Some(Verdict { commands: vec!["./scripts/app check".into()], passed: false }));
        let v = verdict(&[failed, read, passed.clone()], &probe).unwrap();
        assert!(v.passed && v.commands.len() == 2, "the last run counts");
        assert_eq!(verdict(&[tool("Run command", "./scripts/app check", ToolStatus::Running)], &probe), None, "still running");
        // A failed check stays failed through help, dry runs and other subcommands that work…
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        for after in ["./scripts/app --help", "./scripts/app help check", "./scripts/app check --help", "./scripts/app logs --since 1m", "./scripts/app reset --dry-run"] {
            let v = verdict(&[failed.clone(), tool("Run command", after, ToolStatus::Done)], &probe).unwrap();
            assert!(!v.passed, "{after}");
        }
        // …until the check itself passes.
        let fixed = [failed.clone(), tool("Run command", "./scripts/app logs", ToolStatus::Done), tool("Run command", "./scripts/app check", ToolStatus::Done)];
        assert!(verdict(&fixed, &probe).is_some_and(|v| v.passed && v.commands.len() == 3));
        // Asking it for help alone checks nothing.
        assert_eq!(verdict(&[tool("Run command", "./scripts/app --help", ToolStatus::Done)], &probe), None);
        assert_eq!(verdict(&[tool("Run command", "./scripts/app --help && ./scripts/app check", ToolStatus::Done)], &probe).map(|v| v.passed), Some(true));
        // A thread in a worktree that checks the main checkout hasn't checked its own changes.
        let away = Probe { main: Some("/repo".into()), ..probe };
        assert_eq!(verdict(&[tool("Run command", "cd /repo && ./scripts/app check", ToolStatus::Done)], &away), None);
        assert!(verdict(&[passed], &away).is_some_and(|v| v.passed));
    }

    #[test]
    fn agents_are_told_where_it_is() {
        let p = project("tell");
        let dir = skill(&p, ".agents/skills", "control-app", "name: control-app");
        let v = Verification { skill: dir.display().to_string(), name: "control-app".into(), cli: Some("./.agents/skills/control-app/scripts/app".into()), ..Default::default() };
        let text = instructions(&v, &p, &p);
        assert!(text.contains("“control-app”, in `.agents/skills/control-app` in your working folder") && text.contains("(`./.agents/skills/control-app/scripts/app`, run from your working folder)") && text.contains("references/features/README.md"), "{text}");
        assert!(!instructions(&Verification { cli: None, ..v.clone() }, &p, &p).contains("CLI"));
        // A worktree with the skill committed uses its own copy.
        let wt = project("tell-wt");
        skill(&wt, ".agents/skills", "control-app", "name: control-app");
        assert_eq!(instructions(&v, &p, &wt), text);
        // One without a copy reads the main checkout's, and runs its CLI by full path from its own folder.
        let bare = project("tell-bare");
        let text = instructions(&v, &p, &bare);
        let cli = p.join(".agents/skills/control-app/scripts/app");
        assert!(text.contains("has no copy of it yet") && text.contains(&format!("(`{}`, run from your working folder)", cli.display())), "{text}");
        assert!(setup_prompt(Path::new("/d/skills/create-verification-skill/SKILL.md")).contains("`/d/skills/create-verification-skill/SKILL.md`"));
        // Maintaining one with no CLI named asks for one.
        let guide = Path::new("/d/skills/maintain-verification-skill/SKILL.md");
        assert!(!maintain_prompt(guide, &dir, true).contains("names no CLI"));
        assert!(maintain_prompt(guide, &dir, false).contains("name it in the front matter's `metadata` as `cli:`"));
        for d in [p, wt, bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
