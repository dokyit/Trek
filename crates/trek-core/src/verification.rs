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
    use std::os::unix::fs::PermissionsExt as _;
    ["scripts", "bin"].iter().find_map(|sub| {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(sub))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
            .collect();
        files.sort();
        let file = files.into_iter().next()?;
        Some(format!("./{}", file.strip_prefix(project).unwrap_or(&file).display()))
    })
}

/// When anything in the skill's folder last changed (ms): when it was last worked on.
pub fn last_change(dir: &Path) -> Option<i64> {
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

/// What Trek records about a skill it found, keeping the user's choices and the latest of when it
/// was last maintained and when it last `changed` (`last_change`, when it was looked at).
pub fn record(found: &Found, before: Option<&Verification>, changed: Option<i64>) -> Verification {
    let maintained = before.and_then(|b| b.maintained_at).max(changed);
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
pub fn maintain_prompt(guide: &Path, skill: &Path) -> String {
    format!(
        "Maintain this project's verification skill in `{}`, following Trek's guide in `{}`: run it, fix what's broken, and bring the CLI, the dev-environment notes and the Feature Map up to date with the app.",
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

/// Whether a command line ran the CLI `needle` stands for.
pub fn runs(needle: &str, command: &str) -> bool {
    if needle.contains(' ') {
        let line = command.split_whitespace().collect::<Vec<_>>().join(" ");
        let edge = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\''));
        return line.match_indices(needle).any(|(at, _)| edge(line[..at].chars().next_back()) && edge(line[at + needle.len()..].chars().next()));
    }
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\''))
        .filter(|w| !w.is_empty())
        .collect();
    // `path` itself, wherever it's run from: `./scripts/app`, `/repo/scripts/app`.
    let is = |w: &str, path: &str| {
        let w = w.trim_end_matches('/');
        let w = w.strip_prefix("./").unwrap_or(w);
        w == path || w.ends_with(&format!("/{path}"))
    };
    if words.iter().any(|w| is(w, needle)) {
        return true;
    }
    // Or the rest of it, from a folder on its path: `cd .agents/skills/x && ./scripts/app check`.
    needle.match_indices('/').any(|(i, _)| {
        let (dir, rest) = (&needle[..i], &needle[i + 1..]);
        words.windows(2).position(|p| p[0] == "cd" && is(p[1], dir)).is_some_and(|at| words[at + 2..].iter().any(|w| is(w, rest)))
    })
}

/// Whether a command line goes into `dir` (or a folder in it) with `cd`.
fn goes_into(command: &str, dir: &Path) -> bool {
    let words: Vec<&str> = command.split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '"' | '\'')).filter(|w| !w.is_empty()).collect();
    words.windows(2).any(|p| p[0] == "cd" && Path::new(p[1].trim_end_matches('/')).starts_with(dir))
}

/// How Trek tells a turn ran the project's verification CLI: what a command running it contains
/// (`needle`), and, for a thread working outside the main checkout (a worktree), that checkout:
/// a run there checks the main checkout's code, not the thread's.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub needle: String,
    pub main: Option<PathBuf>,
}

/// How a turn used the verification CLI: the commands that ran it, and whether the last one
/// worked.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub commands: Vec<String>,
    pub passed: bool,
}

/// Whether the tool calls in `turn` (one turn's items) ran the CLI `probe` stands for. Reading
/// the skill isn't verifying: only running its CLI counts, where the thread works.
pub fn verdict(turn: &[Item], probe: &Probe) -> Option<Verdict> {
    let here = |d: &str| probe.main.as_deref().is_none_or(|m| !goes_into(d, m));
    let runs: Vec<(&String, ToolStatus)> = turn
        .iter()
        .filter_map(|i| match i {
            Item::Tool { title, detail, status, .. } if is_command(title) && runs(&probe.needle, detail) && here(detail) && *status != ToolStatus::Running => Some((detail, *status)),
            _ => None,
        })
        .collect();
    let (_, last) = runs.last()?;
    Some(Verdict { passed: *last == ToolStatus::Done, commands: runs.iter().map(|(d, _)| d.to_string()).collect() })
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
        let app = named.join("scripts/app");
        std::fs::write(&app, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&app, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        skill(&p, ".claude/skills", "verification-before-completion", "name: verification-before-completion\ndescription: Check before you say done.");
        assert_eq!(find(&p), None, "a name alone isn't a verification skill");
        std::fs::create_dir_all(named.join("references/features")).unwrap();
        std::fs::write(named.join("references/features/README.md"), "# Features\n").unwrap();
        let found = find(&p).unwrap();
        assert_eq!((found.name.as_str(), found.cli.as_deref()), ("verify-app", Some("./.claude/skills/verify-app/scripts/app")));
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
        let changed = last_change(&dir).unwrap();
        assert!((crate::store::now_ms() - changed).abs() < 60_000);
        let v = record(&found, None, Some(changed));
        assert_eq!((v.maintained_at, v.remind_weekly, v.name.as_str()), (Some(changed), false, "control-app"));
        // A later maintenance run counts; the user's choices stay.
        let before = Verification { maintained_at: Some(changed + 10), remind_weekly: true, reminded_at: Some(5), ..v.clone() };
        let again = record(&found, Some(&before), Some(changed));
        assert_eq!((again.maintained_at, again.remind_weekly, again.reminded_at), (Some(changed + 10), true, Some(5)));
        // Not looked at yet: what was known stays.
        assert_eq!(record(&found, Some(&before), None).maintained_at, Some(changed + 10));
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
        assert!(!runs("scripts/app", "cat scripts/apple.txt"));
        assert!(!runs("scripts/app", "cat myscripts/app"));
        assert!(runs("trekctl", "trekctl open settings"));
        assert!(runs("trekctl", "(cd x; /usr/local/bin/trekctl check)"));
        assert!(!runs("trekctl", "echo trekctl-old"));
        assert!(runs("cargo run -p xtask", "cargo  run -p xtask -- check"));
        // A program run through make (or swift, just, docker) is only that target.
        assert!(runs("make verify", "make verify"));
        assert!(runs("make verify", "cd app && make verify ARGS=check"));
        assert!(!runs("make verify", "make build"));
        assert!(!runs("make verify", "make verify-all"));
        assert!(!runs("swift run appctl", "swift build"));
        // From a folder on its path.
        let cli = ".agents/skills/verify-app/scripts/app";
        assert!(runs(cli, "cd .agents/skills/verify-app && ./scripts/app check"));
        assert!(runs(cli, "cd /wt/.agents/skills/verify-app/scripts; ./app check"));
        assert!(!runs(cli, "cd .agents/skills/verify-app && cat SKILL.md"));
        assert!(!runs(cli, "./scripts/app check"), "another app's scripts/app");
    }

    #[test]
    fn a_turn_is_verified_by_running_the_cli_not_reading_the_skill() {
        let tool = |title: &str, detail: &str, status: ToolStatus| Item::Tool { id: "t".into(), title: title.into(), detail: detail.into(), output: String::new(), status };
        let probe = Probe { needle: "scripts/app".into(), main: None };
        let read = tool("Read", "/repo/.agents/skills/control-app/scripts/app", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone()], &probe), None);
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        let passed = tool("Run command", "./scripts/app check --json", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone(), failed.clone()], &probe), Some(Verdict { commands: vec!["./scripts/app check".into()], passed: false }));
        let v = verdict(&[failed, read, passed.clone()], &probe).unwrap();
        assert!(v.passed && v.commands.len() == 2, "the last run counts");
        assert_eq!(verdict(&[tool("Run command", "./scripts/app check", ToolStatus::Running)], &probe), None, "still running");
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
        for d in [p, wt, bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
