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
/// verification`), else one named like one ("verify-app", "control-app").
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
            let named = name.contains("verif") || name == "control-app";
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
/// was last maintained and when it last changed.
pub fn record(found: &Found, before: Option<&Verification>) -> Verification {
    let changed = last_change(&found.dir);
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

/// What every agent working in the project is told about the skill.
pub fn instructions(v: &Verification) -> String {
    let cli = v.cli.as_deref().map(|c| format!(" and drive and check the app with its CLI (`{c}`, run from the project folder) rather than throwaway scripts")).unwrap_or_default();
    format!(
        "This project has a verification skill, “{}”, in {}. Before you call a change done, verify it with the skill: read its SKILL.md{cli}. Its Feature Map, references/features/README.md, says what each feature does and how to reach it.",
        v.name,
        crate::paths::tildify(Path::new(&v.skill))
    )
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
/// (`./scripts/app check` → `scripts/app`), or the whole line for one run through another
/// program (`cargo run -p xtask --` → itself).
pub fn needle(cli: &str) -> Option<String> {
    const RUNNERS: [&str; 18] = ["npx", "npm", "pnpm", "yarn", "bun", "bunx", "node", "deno", "python", "python3", "uv", "uvx", "cargo", "go", "sh", "bash", "zsh", "ruby"];
    let words: Vec<&str> = cli.split_whitespace().collect();
    if let Some(path) = words.iter().find(|w| w.contains('/')) {
        return Some(path.trim_start_matches("./").to_string());
    }
    let first = *words.first()?;
    Some(if RUNNERS.contains(&first) { words.join(" ") } else { first.to_string() })
}

/// Whether a command line ran the CLI `needle` stands for.
pub fn runs(needle: &str, command: &str) -> bool {
    if needle.contains(' ') {
        return command.split_whitespace().collect::<Vec<_>>().join(" ").contains(needle);
    }
    // The program itself, wherever it's run from: `./scripts/app`, `/repo/scripts/app`.
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\''))
        .any(|w| w == needle || w.ends_with(&format!("/{needle}")))
}

/// How a turn used the verification CLI: the commands that ran it, and whether the last one
/// worked.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub commands: Vec<String>,
    pub passed: bool,
}

/// Whether the tool calls in `turn` (one turn's items) ran the CLI `needle` stands for. Reading
/// the skill isn't verifying: only running its CLI counts.
pub fn verdict(turn: &[Item], needle: &str) -> Option<Verdict> {
    let runs: Vec<(&String, ToolStatus)> = turn
        .iter()
        .filter_map(|i| match i {
            Item::Tool { title, detail, status, .. } if is_command(title) && runs(needle, detail) && *status != ToolStatus::Running => Some((detail, *status)),
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
        // Named like one, with a script of its own.
        let named = skill(&p, ".claude/skills", "verify-app", "name: verify-app\ndescription: Check the app.");
        std::fs::create_dir_all(named.join("scripts")).unwrap();
        std::fs::write(named.join("scripts/notes.txt"), "not a program").unwrap();
        let app = named.join("scripts/app");
        std::fs::write(&app, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&app, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
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
        let dir = skill(&p, ".agents/skills", "control-app", "name: control-app");
        let found = find(&p).unwrap();
        let changed = last_change(&dir).unwrap();
        assert!((crate::store::now_ms() - changed).abs() < 60_000);
        let v = record(&found, None);
        assert_eq!((v.maintained_at, v.remind_weekly, v.name.as_str()), (Some(changed), false, "control-app"));
        // A later maintenance run counts; the user's choices stay.
        let before = Verification { maintained_at: Some(changed + 10), remind_weekly: true, reminded_at: Some(5), ..v.clone() };
        let again = record(&found, Some(&before));
        assert_eq!((again.maintained_at, again.remind_weekly, again.reminded_at), (Some(changed + 10), true, Some(5)));
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
        assert_eq!(needle("cargo run -p xtask --").as_deref(), Some("cargo run -p xtask --"));
        assert_eq!(needle("trekctl").as_deref(), Some("trekctl"));
        assert_eq!(needle("  "), None);
        assert!(runs("scripts/app", "cd /repo && ./scripts/app check --json"));
        assert!(runs("scripts/app", "/repo/scripts/app status"));
        assert!(!runs("scripts/app", "cat scripts/apple.txt"));
        assert!(!runs("scripts/app", "cat myscripts/app"));
        assert!(runs("trekctl", "trekctl open settings"));
        assert!(runs("trekctl", "(cd x; /usr/local/bin/trekctl check)"));
        assert!(!runs("trekctl", "echo trekctl-old"));
        assert!(runs("cargo run -p xtask --", "cargo  run -p xtask -- check"));
    }

    #[test]
    fn a_turn_is_verified_by_running_the_cli_not_reading_the_skill() {
        let tool = |title: &str, detail: &str, status: ToolStatus| Item::Tool { id: "t".into(), title: title.into(), detail: detail.into(), output: String::new(), status };
        let read = tool("Read", "/repo/.agents/skills/control-app/scripts/app", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone()], "scripts/app"), None);
        let failed = tool("Run command", "./scripts/app check", ToolStatus::Failed);
        let passed = tool("Run command", "./scripts/app check --json", ToolStatus::Done);
        assert_eq!(verdict(&[read.clone(), failed.clone()], "scripts/app"), Some(Verdict { commands: vec!["./scripts/app check".into()], passed: false }));
        let v = verdict(&[failed, read, passed], "scripts/app").unwrap();
        assert!(v.passed && v.commands.len() == 2, "the last run counts");
        assert_eq!(verdict(&[tool("Run command", "./scripts/app check", ToolStatus::Running)], "scripts/app"), None, "still running");
    }

    #[test]
    fn agents_are_told_where_it_is() {
        let v = Verification { skill: "/repo/.agents/skills/control-app".into(), name: "control-app".into(), cli: Some("./scripts/app".into()), ..Default::default() };
        let text = instructions(&v);
        assert!(text.contains("“control-app”") && text.contains("(`./scripts/app`") && text.contains("references/features/README.md"), "{text}");
        assert!(!instructions(&Verification { cli: None, ..v }).contains("CLI"));
        assert!(setup_prompt(Path::new("/d/skills/create-verification-skill/SKILL.md")).contains("`/d/skills/create-verification-skill/SKILL.md`"));
    }
}
