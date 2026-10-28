//! Agent skills on this Mac: folders holding a `SKILL.md` (front matter `name` + `description`).
//!
//! Turning a skill off moves its folder into Trek's data folder (with a note of where it came
//! from), so every agent stops loading it; turning it back on moves it home. Skills that another
//! tool manages — Claude.ai sync, Codex's built-ins, plugins — are listed read-only, as are the
//! ones Trek ships itself (`SHIPPED`), kept in its data folder for any agent to follow.

use crate::import::codex::codex_home_for;
use crate::paths;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SkillSource {
    /// `~/.claude/skills`
    ClaudeCode,
    /// `~/.codex/skills` (under `CODEX_HOME` when that is set)
    Codex,
    /// `~/.agents/skills`, read by several agents.
    Shared,
    /// `<project>/.claude/skills` or `<project>/.agents/skills`
    Project,
    /// `~/.claude/skills/synced/…`, managed by Claude.
    Synced,
    /// `~/.codex/skills/.system`, shipped with Codex (under `CODEX_HOME` when that is set).
    CodexSystem,
    /// A Claude Code plugin's `skills/` folder.
    Plugin(String),
    /// Shipped with Trek (`SHIPPED`), in its data folder.
    Trek,
}

impl SkillSource {
    pub fn label(&self) -> String {
        match self {
            SkillSource::ClaudeCode => "Claude Code".into(),
            SkillSource::Codex => "Codex".into(),
            SkillSource::Shared => "All agents".into(),
            SkillSource::Project => "This project".into(),
            SkillSource::Synced => "Synced from Claude".into(),
            SkillSource::CodexSystem => "Built into Codex".into(),
            SkillSource::Plugin(p) => format!("Plugin · {p}"),
            SkillSource::Trek => "Built into Trek".into(),
        }
    }

    /// Whether Trek may turn it off, edit or remove it.
    pub fn editable(&self) -> bool {
        matches!(
            self,
            SkillSource::ClaudeCode
                | SkillSource::Codex
                | SkillSource::Shared
                | SkillSource::Project
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The skill's folder (where it is now: home, or Trek's disabled area).
    pub dir: PathBuf,
    pub source: SkillSource,
    pub enabled: bool,
}

impl Skill {
    pub fn skill_md(&self) -> PathBuf {
        self.dir.join("SKILL.md")
    }
}

/// Where a new or added skill goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillHome {
    ClaudeCode,
    Codex,
    Shared,
}

/// Codex's skills folder, inside its home (`~/.codex`, or `CODEX_HOME`: `codex_home_for` knows).
fn codex_skills_dir(codex_home: &Path) -> PathBuf {
    codex_home.join("skills")
}

impl SkillHome {
    pub fn dir(self) -> PathBuf {
        let home = paths::agents_home();
        match self {
            SkillHome::ClaudeCode => home.join(".claude/skills"),
            SkillHome::Codex => codex_skills_dir(&codex_home_for(&home)),
            SkillHome::Shared => home.join(".agents/skills"),
        }
    }
}

/// The guide that builds a project's verification skill (`crate::verification`).
pub const CREATE_VERIFICATION: &str = "create-verification-skill";
/// The guide that brings one up to date.
pub const MAINTAIN_VERIFICATION: &str = "maintain-verification-skill";

/// Card recipes (`artifact::AGENT_INSTRUCTIONS` names them): a `trek-html` card for places on a
/// map, a pull request, a database migration.
pub const MAP_CARD: &str = "map-card";
pub const PULL_REQUEST_CARD: &str = "pull-request-card";
pub const MIGRATION_CARD: &str = "database-migration-card";
pub const CARD_RECIPES: [&str; 3] = [MAP_CARD, PULL_REQUEST_CARD, MIGRATION_CARD];

/// The guide a Study session follows (`crate::study`).
pub const STUDY: &str = "study";

/// Skills Trek ships: (folder name, SKILL.md).
pub const SHIPPED: [(&str, &str); 6] = [
    (
        CREATE_VERIFICATION,
        include_str!("../skills/create-verification-skill/SKILL.md"),
    ),
    (
        MAINTAIN_VERIFICATION,
        include_str!("../skills/maintain-verification-skill/SKILL.md"),
    ),
    (MAP_CARD, include_str!("../skills/map-card/SKILL.md")),
    (
        PULL_REQUEST_CARD,
        include_str!("../skills/pull-request-card/SKILL.md"),
    ),
    (
        MIGRATION_CARD,
        include_str!("../skills/database-migration-card/SKILL.md"),
    ),
    (STUDY, include_str!("../skills/study/SKILL.md")),
];

/// Writes every skill Trek ships into `shipped_root` (or brings it up to this version), so an
/// agent pointed there finds them all. Best effort: one that can't be written is left out.
pub fn write_shipped() {
    for (name, _) in SHIPPED {
        let _ = shipped(name);
    }
}

/// A skill from elsewhere the Skills page suggests, with the command that installs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    /// Its name once installed (what `discover` lists it as).
    pub name: &'static str,
    pub title: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    /// Run in a terminal: it asks where to install, as the tool does.
    pub command: &'static str,
}

/// Skills worth having, shown under "Suggested" until they're installed.
pub const SUGGESTED: [Suggestion; 1] = [Suggestion {
    name: "anatomy",
    title: "Anatomy",
    author: "wheresryan22",
    description: "Name an idea and the agent draws the machine that explains it: interactive isometric wireframes to rotate, inspect and run, with WebGL fire, water and plasma.",
    command: "npx skills add wheresryan22/anatomy",
}];

/// The suggestions not among `installed` (by name, any case).
pub fn suggestions(installed: &[Skill]) -> Vec<Suggestion> {
    SUGGESTED
        .into_iter()
        .filter(|s| {
            !installed
                .iter()
                .any(|i| i.name.eq_ignore_ascii_case(s.name))
        })
        .collect()
}

/// Where Trek keeps the skills it ships. Not one of the agents' own skill folders: Trek points an
/// agent at one when it starts work that follows it (every session may read it), and the user's
/// agents are left as they are. The first ask in a process (per data folder) writes them all there
/// (`write_shipped`), so an agent told of a card recipe finds it.
pub fn shipped_root() -> PathBuf {
    static WRITTEN: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let root = shipped_home();
    let first = WRITTEN
        .lock()
        .map(|mut done| {
            !done.contains(&root) && {
                done.push(root.clone());
                true
            }
        })
        .unwrap_or(false);
    if first {
        write_shipped();
    }
    root
}

fn shipped_home() -> PathBuf {
    paths::data_dir().join("skills")
}

/// The SKILL.md of shipped skill `name`, written (or brought up to this version) first.
pub fn shipped(name: &str) -> anyhow::Result<PathBuf> {
    let (_, text) = SHIPPED
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| anyhow::anyhow!("Trek ships no skill called {name}"))?;
    let md = shipped_home().join(name).join("SKILL.md");
    if std::fs::read_to_string(&md).ok().as_deref() != Some(*text) {
        std::fs::create_dir_all(md.parent().unwrap_or(&md))?;
        std::fs::write(&md, text)?;
    }
    Ok(md)
}

fn disabled_root() -> PathBuf {
    paths::data_dir().join("disabled-skills")
}

const ORIGIN_FILE: &str = ".trek-origin";

/// `(name, description)` from SKILL.md front matter; the folder name when it has none.
pub fn read_front_matter(skill_md: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(skill_md).ok()?;
    let folder = skill_md
        .parent()?
        .file_name()?
        .to_string_lossy()
        .to_string();
    let mut name = None;
    let mut desc = None;
    let mut body = text.as_str();
    if let Some(rest) = text.strip_prefix("---") {
        let close_pos = rest.find("\n---").or_else(|| rest.find("\r\n---"));
        if let Some(pos) = close_pos {
            let front = &rest[..pos];
            let after_dash = rest[pos..]
                .trim_start_matches(|c| c == '\r' || c == '\n')
                .strip_prefix("---")
                .unwrap_or_default();
            body = after_dash;
            let mut lines = front.lines().peekable();
            while let Some(line) = lines.next() {
                if let Some((k, v)) = line.split_once(':') {
                    let v = v.trim();
                    let mut v = match v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                        // A double-quoted scalar, as `yaml_description` writes one.
                        Some(quoted) => unescape_yaml(quoted),
                        None => v.trim_matches('"').trim_matches('\'').to_string(),
                    };
                    // A folded (`>-`) or literal (`|`) block: the text is on the indented lines below.
                    let mut block = v.chars();
                    if matches!(block.next(), Some('>' | '|'))
                        && block.all(|c| matches!(c, '+' | '-' | '0'..='9'))
                    {
                        let folded = v.starts_with('>');
                        let mut text = vec![];
                        while let Some(next) =
                            lines.next_if(|l| l.trim().is_empty() || l.starts_with([' ', '\t']))
                        {
                            text.push(next.trim());
                        }
                        v = text
                            .join(if folded { " " } else { "\n" })
                            .trim()
                            .to_string();
                    }
                    match k.trim() {
                        "name" if !v.is_empty() => name = Some(v),
                        "description" if !v.is_empty() => desc = Some(v),
                        _ => {}
                    }
                }
            }
        }
    }
    let desc = desc.or_else(|| {
        // Fallback: the first prose line in the body.
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
            .map(str::to_string)
    });
    Some((name.unwrap_or(folder), desc.unwrap_or_default()))
}

/// Skill folders directly inside `root` (following symlinks).
fn scan(root: &Path, source: SkillSource, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let file_name = e.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') || !dir.is_dir() {
            continue;
        }
        let md = dir.join("SKILL.md");
        if let Some((name, description)) = md.exists().then(|| read_front_matter(&md)).flatten() {
            out.push(Skill {
                name,
                description,
                dir,
                source: source.clone(),
                enabled: true,
            });
        }
    }
}

/// Every skill Trek can see, enabled and disabled, for the given project.
pub fn discover(project: Option<&Path>) -> Vec<Skill> {
    let home = paths::agents_home();
    let codex_skills = codex_skills_dir(&codex_home_for(&home));
    let mut out = vec![];
    scan(
        &home.join(".claude/skills"),
        SkillSource::ClaudeCode,
        &mut out,
    );
    scan(&codex_skills, SkillSource::Codex, &mut out);
    scan(&home.join(".agents/skills"), SkillSource::Shared, &mut out);
    if let Some(p) = project.filter(|p| *p != home.as_path()) {
        scan(&p.join(".claude/skills"), SkillSource::Project, &mut out);
        scan(&p.join(".agents/skills"), SkillSource::Project, &mut out);
    }
    // Claude.ai-synced skills: synced/<bucket>/<skill>/SKILL.md
    if let Ok(buckets) = std::fs::read_dir(home.join(".claude/skills/synced")) {
        for b in buckets.flatten().filter(|b| b.path().is_dir()) {
            scan(&b.path(), SkillSource::Synced, &mut out);
        }
    }
    scan(
        &codex_skills.join(".system"),
        SkillSource::CodexSystem,
        &mut out,
    );
    write_shipped();
    scan(&shipped_root(), SkillSource::Trek, &mut out);
    for (plugin, dir) in plugin_dirs() {
        scan(&dir.join("skills"), SkillSource::Plugin(plugin), &mut out);
    }
    // Disabled ones, back under the source they came from.
    if let Ok(entries) = std::fs::read_dir(disabled_root()) {
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(origin) = std::fs::read_to_string(dir.join(ORIGIN_FILE)) else {
                continue;
            };
            let origin = PathBuf::from(origin.trim());
            let source = source_for(&origin, project);
            if let Some((name, description)) = read_front_matter(&dir.join("SKILL.md")) {
                out.push(Skill {
                    name,
                    description,
                    dir,
                    source,
                    enabled: false,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn source_for(origin: &Path, project: Option<&Path>) -> SkillSource {
    let home = paths::agents_home();
    if origin.starts_with(home.join(".claude/skills/synced")) {
        SkillSource::Synced
    } else if origin.starts_with(home.join(".claude/skills")) {
        SkillSource::ClaudeCode
    } else if origin.starts_with(codex_skills_dir(&codex_home_for(&home))) {
        SkillSource::Codex
    } else if origin.starts_with(home.join(".agents/skills")) {
        SkillSource::Shared
    } else if project.is_some_and(|p| origin.starts_with(p)) {
        SkillSource::Project
    } else {
        SkillSource::Shared
    }
}

/// Installed Claude Code plugins: `(name, install folder)`.
fn plugin_dirs() -> Vec<(String, PathBuf)> {
    let file = paths::agents_home().join(".claude/plugins/installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(file) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let mut out = vec![];
    for (name, installs) in v["plugins"].as_object().into_iter().flatten() {
        for i in installs.as_array().into_iter().flatten() {
            if let Some(p) = i["installPath"].as_str() {
                out.push((
                    name.split('@').next().unwrap_or(name).to_string(),
                    PathBuf::from(p),
                ));
            }
        }
    }
    out
}

/// Turn a skill off (move it aside) or back on (move it home).
pub fn set_enabled(skill: &Skill, on: bool) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        skill.source.editable(),
        "{} skills are managed elsewhere",
        skill.source.label()
    );
    if on == skill.enabled {
        return Ok(skill.dir.clone());
    }
    if on {
        let origin = PathBuf::from(std::fs::read_to_string(skill.dir.join(ORIGIN_FILE))?.trim());
        anyhow::ensure!(
            !origin.exists(),
            "A skill already exists at {}",
            origin.display()
        );
        if let Some(parent) = origin.parent() {
            std::fs::create_dir_all(parent)?;
        }
        move_dir(&skill.dir, &origin)?;
        let _ = std::fs::remove_file(origin.join(ORIGIN_FILE));
        Ok(origin)
    } else {
        let root = disabled_root();
        std::fs::create_dir_all(&root)?;
        let folder = skill
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| skill.name.clone());
        let mut dest = root.join(&folder);
        let mut n = 2;
        while dest.exists() {
            dest = root.join(format!("{folder}-{n}"));
            n += 1;
        }
        // A symlinked skill: move the link, not what it points at.
        if !move_absolute_link(&skill.dir, &dest)? {
            move_dir(&skill.dir, &dest)?;
        }
        std::fs::write(dest.join(ORIGIN_FILE), skill.dir.display().to_string()).or_else(|e| {
            // A moved symlink to a read-only target: keep the origin beside it instead.
            let _ = move_dir(&dest, &skill.dir);
            Err(e)
        })?;
        Ok(dest)
    }
}

/// Move a symlink whose target is relative (`../../.agents/skills/foo`) to `to` as a link to the
/// absolute target, so it doesn't dangle from its new folder. False when `from` isn't such a link.
#[cfg(unix)]
fn move_absolute_link(from: &Path, to: &Path) -> anyhow::Result<bool> {
    let relative = std::fs::read_link(from).is_ok_and(|t| t.is_relative());
    let Some(target) = relative.then(|| std::fs::canonicalize(from).ok()).flatten() else {
        return Ok(false);
    };
    std::os::unix::fs::symlink(target, to)?;
    std::fs::remove_file(from)?;
    Ok(true)
}

#[cfg(not(unix))]
fn move_absolute_link(_from: &Path, _to: &Path) -> anyhow::Result<bool> {
    Ok(false)
}

fn move_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Different volume: copy, then remove.
        Err(_) => {
            // A symlinked skill moves as the link, as a rename would have moved it.
            #[cfg(unix)]
            if let Ok(target) = std::fs::read_link(from) {
                std::os::unix::fs::symlink(target, to)?;
                std::fs::remove_file(from)?;
                return Ok(());
            }
            copy_dir(from, to)
                .map_err(|e| e.context(format!("Couldn't move {}", from.display())))?;
            std::fs::remove_dir_all(from)?;
            Ok(())
        }
    }
}

/// Copy the folder `from` to the new path `to`, files and folders alike, like `cp -R`. A file keeps
/// its permissions (so a skill's scripts stay executable). A symlink inside the folder is never
/// followed, so nothing outside `from` is read: on Unix it is recreated as the same link, and
/// where links can't be made without a privilege (Windows) it is left out. A failed copy leaves
/// no half-copied `to`.
fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::fs::symlink_metadata(to).is_err(),
        "{} already exists",
        to.display()
    );
    let copied = (|| {
        // The folder itself may be a symlink to the real one; its contents are what is wanted, so
        // the walk starts from the real folder (walking a root that is a link failed on macOS).
        let root = std::fs::canonicalize(from)?;
        for entry in walkdir::WalkDir::new(&root).follow_links(false) {
            let entry = entry?;
            let rel = entry.path().strip_prefix(&root)?;
            let dest = to.join(rel);
            let kind = entry.file_type();
            if kind.is_dir() {
                std::fs::create_dir(&dest)?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), &dest)?;
            } else if kind.is_symlink() {
                #[cfg(unix)]
                std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &dest)?;
            }
        }
        anyhow::Ok(())
    })();
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(to);
    }
    copied
}

/// Move a skill folder to the Trash.
pub fn trash(skill: &Skill) -> anyhow::Result<()> {
    anyhow::ensure!(
        skill.source.editable(),
        "{} skills are managed elsewhere",
        skill.source.label()
    );
    paths::trash(&skill.dir).map_err(|e| {
        e.context(format!(
            "Couldn't move {} to the Trash",
            skill.dir.display()
        ))
    })
}

/// Copy a skill folder (one containing SKILL.md) into `home`.
pub fn install_from(folder: &Path, home: SkillHome) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        folder.join("SKILL.md").exists(),
        "That folder has no SKILL.md"
    );
    let name = folder
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Not a folder"))?;
    let root = home.dir();
    std::fs::create_dir_all(&root)?;
    let dest = root.join(name);
    anyhow::ensure!(
        !dest.exists(),
        "A skill named {} is already there",
        name.to_string_lossy()
    );
    copy_dir(folder, &dest).map_err(|e| e.context("Couldn't copy the skill"))?;
    Ok(dest)
}

/// "Review PRs" → "review-prs".
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    s.trim_end_matches('-').to_string()
}

/// A description as one line of a front matter value that strict YAML readers (Codex's) accept:
/// plain when it can be, else double-quoted with `\` and `"` escaped.
fn yaml_description(description: &str) -> String {
    let one_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let plain = !one_line.contains(": ")
        && !one_line.contains(" #")
        && !one_line.contains(['"', '\\'])
        && !one_line.ends_with(':')
        && !one_line.starts_with(|c| "-?:,[]{}#&*!|>'%@`".contains(c));
    if plain {
        one_line
    } else {
        format!(
            "\"{}\"",
            one_line.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

/// The text of a double-quoted YAML scalar: `\\` and `\"` stand for themselves.
fn unescape_yaml(quoted: &str) -> String {
    let mut out = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(n @ ('\\' | '"'))) => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// Create a new skill with a starter SKILL.md; returns the file to open.
pub fn create(name: &str, description: &str, home: SkillHome) -> anyhow::Result<PathBuf> {
    let slug = slug(name);
    anyhow::ensure!(!slug.is_empty(), "Give the skill a name");
    let dir = home.dir().join(&slug);
    anyhow::ensure!(!dir.exists(), "A skill named {slug} already exists");
    std::fs::create_dir_all(&dir)?;
    let description = if description.trim().is_empty() {
        "Describe when an agent should use this skill.".to_string()
    } else {
        yaml_description(description)
    };
    let md = dir.join("SKILL.md");
    std::fs::write(
        &md,
        format!(
            "---\nname: {slug}\ndescription: {description}\n---\n\n# {}\n\nWrite the instructions the agent should follow here.\n",
            name.trim()
        ),
    )?;
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_fallbacks() {
        let dir = std::env::temp_dir().join(format!("trek-skill-test-{}", std::process::id()));
        let s = dir.join("my-skill");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(
            s.join("SKILL.md"),
            "---\nname: grill-me\ndescription: \"Ask hard questions.\"\n---\nbody",
        )
        .unwrap();
        assert_eq!(
            read_front_matter(&s.join("SKILL.md")),
            Some(("grill-me".into(), "Ask hard questions.".into()))
        );
        std::fs::write(s.join("SKILL.md"), "# Title\n\nDoes a thing.").unwrap();
        assert_eq!(
            read_front_matter(&s.join("SKILL.md")),
            Some(("my-skill".into(), "Does a thing.".into()))
        );
        // Front matter with name but no description does not use 'name:' as fallback description.
        std::fs::write(
            s.join("SKILL.md"),
            "---\nname: no-desc\n---\n# Heading\n\nBody description line.",
        )
        .unwrap();
        assert_eq!(
            read_front_matter(&s.join("SKILL.md")),
            Some(("no-desc".into(), "Body description line.".into()))
        );
        // A folded or literal block holds its text on the indented lines below.
        std::fs::write(s.join("SKILL.md"), "---\nname: folded\ndescription: >-\n  Coordinate workers:\n  threaded messages.\n\nother: x\n---\nbody").unwrap();
        assert_eq!(
            read_front_matter(&s.join("SKILL.md")),
            Some((
                "folded".into(),
                "Coordinate workers: threaded messages.".into()
            ))
        );
        std::fs::write(
            s.join("SKILL.md"),
            "---\ndescription: |\n  line one\n  line two\nname: literal\n---\nbody",
        )
        .unwrap();
        assert_eq!(
            read_front_matter(&s.join("SKILL.md")),
            Some(("literal".into(), "line one\nline two".into()))
        );
        // Synced skill source classification
        let home = paths::agents_home();
        assert_eq!(
            source_for(&home.join(".claude/skills/synced/bucket/my-skill"), None),
            SkillSource::Synced
        );
        assert_eq!(
            source_for(&home.join(".claude/skills/local-skill"), None),
            SkillSource::ClaudeCode
        );
        assert_eq!(
            source_for(Path::new("/somewhere/else/my-skill"), None),
            SkillSource::Shared
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shipped_skills_are_written_and_kept_current() {
        crate::paths::isolate_thread(
            std::env::temp_dir().join(format!("trek-shipped-skills-{}", std::process::id())),
        );
        let md = shipped(CREATE_VERIFICATION).unwrap();
        assert!(md.starts_with(shipped_root()));
        let (name, desc) = read_front_matter(&md).unwrap();
        assert_eq!(name, CREATE_VERIFICATION);
        assert!(desc.contains("verification skill"), "{desc}");
        // An old copy is brought up to this version.
        std::fs::write(&md, "stale").unwrap();
        shipped(CREATE_VERIFICATION).unwrap();
        assert!(
            std::fs::read_to_string(&md)
                .unwrap()
                .contains("metadata:\n  trek: verification")
        );
        let listed: Vec<Skill> = discover(None)
            .into_iter()
            .filter(|s| s.source == SkillSource::Trek)
            .collect();
        assert_eq!(
            listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            [
                CREATE_VERIFICATION,
                MIGRATION_CARD,
                MAINTAIN_VERIFICATION,
                MAP_CARD,
                PULL_REQUEST_CARD,
                STUDY
            ]
        );
        assert!(!SkillSource::Trek.editable(), "read-only");
        assert!(shipped("nope").is_err());
    }

    /// The card in a recipe: its one `trek-html` block.
    fn recipe_card(md: &str) -> &str {
        let open = format!("```{}\n", crate::artifact::FENCE);
        assert_eq!(md.matches(&open).count(), 1, "one card per recipe");
        let start = md.find(&open).unwrap() + open.len();
        let len = md[start..].find("\n```").expect("closed");
        &md[start..start + len]
    }

    #[test]
    fn each_card_recipe_is_a_skill_with_a_card_that_runs_within_the_limits() {
        use crate::artifact;
        for name in CARD_RECIPES {
            let (_, md) = SHIPPED.iter().find(|(n, _)| *n == name).expect("shipped");
            let front = md
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---\n"))
                .map(|(front, _)| front)
                .expect("front matter");
            assert!(front.contains(&format!("name: {name}\n")), "{name}");
            let description = front
                .lines()
                .find_map(|l| l.strip_prefix("description: "))
                .expect("a description");
            assert!(
                description.contains("Use when") && !description.contains(": "),
                "{name}: says when, and stays plain YAML"
            );
            assert!(
                md.contains("## What you fill in") && md.contains("## Rules"),
                "{name}"
            );

            let html = recipe_card(md);
            // A whole document with a title, as the fence takes it, well under what agents are
            // asked to keep to (and what Trek runs at all).
            let block = format!("```{}\n{html}\n```", artifact::FENCE);
            assert!(artifact::fence_closed(&block), "{name}");
            assert!(
                html.len() < artifact::ADVISED_BYTES / 4 && html.len() < artifact::MAX_BYTES,
                "{name}: {} bytes",
                html.len()
            );
            assert!(
                html.starts_with("<!doctype html>") && html.trim_end().ends_with("</html>"),
                "{name}"
            );
            assert!(
                artifact::title(html).is_some_and(|t| !t.is_empty()),
                "{name}: a title"
            );
            let doc = artifact::document(html, ":root{}");
            assert!(
                doc.contains(artifact::CSP) && doc.contains("const DATA = {"),
                "{name}"
            );
            // Nothing the policy blocks: https only.
            assert!(
                !html.contains("http://") && !html.contains("file://"),
                "{name}"
            );
            for host in artifact::hosts(html, 8) {
                assert!(
                    [
                        "unpkg.com",
                        "tile.openstreetmap.org",
                        "tiles.openfreemap.org",
                        "github.com"
                    ]
                    .iter()
                    .any(|h| host.ends_with(h)),
                    "{name}: {host}"
                );
            }
            // Themed by Trek, in Night and Paper alike.
            for var in [
                "var(--trek-fg",
                "var(--trek-muted",
                "var(--trek-border",
                "var(--trek-surface",
                "var(--trek-font",
            ] {
                assert!(html.contains(var), "{name}: {var}");
            }
            // It acts only through the page's helper, and only when that's there.
            assert!(
                html.contains("window.trek.send(") && html.contains("!window.trek"),
                "{name}"
            );
            for banned in [
                "window.webkit",
                "chrome.webview",
                "postMessage",
                "localStorage",
                "document.cookie",
            ] {
                assert!(!html.contains(banned), "{name}: {banned}");
            }
        }
        // What each one sends.
        let card = |n: &str| recipe_card(SHIPPED.iter().find(|(name, _)| *name == n).unwrap().1);
        assert!(
            card(MAP_CARD).contains(&format!("send: \"{}\"", artifact::OPEN_ON_PHONE))
                && card(MAP_CARD).contains("leaflet@1.9.4")
                && card(MAP_CARD).contains("tile.openstreetmap.org")
                && card(MAP_CARD).contains("tiles.openfreemap.org")
        );
        assert!(
            card(PULL_REQUEST_CARD)
                .contains(r#""Resolve " + keep.dataset.name + ": keep " + b.dataset.keep"#)
        );
        assert!(
            card(MIGRATION_CARD).contains(r#"window.trek.send("Apply the migration")"#)
                && card(MIGRATION_CARD).contains("Applied in")
        );
    }

    #[test]
    fn the_folder_agents_may_read_holds_the_card_recipes_once_asked_for() {
        crate::paths::isolate_thread(
            std::env::temp_dir().join(format!("trek-shipped-root-{}", std::process::id())),
        );
        let root = shipped_root();
        for name in CARD_RECIPES {
            assert!(root.join(name).join("SKILL.md").is_file(), "{name}");
        }
    }

    #[test]
    fn suggestions_go_once_installed() {
        assert_eq!(SUGGESTED[0].command, "npx skills add wheresryan22/anatomy");
        assert_eq!(suggestions(&[]), SUGGESTED);
        let installed = Skill {
            name: "Anatomy".into(),
            description: String::new(),
            dir: PathBuf::from("/x/anatomy"),
            source: SkillSource::ClaudeCode,
            enabled: true,
        };
        assert!(suggestions(&[installed]).is_empty());
    }

    #[test]
    fn codex_skills_live_under_the_codex_home_wherever_that_is() {
        let default = Path::new("/home/u/.codex");
        assert_eq!(codex_skills_dir(default), default.join("skills"));
        // A CODEX_HOME elsewhere moves the skills with it, built-ins included.
        let moved = std::env::temp_dir().join("elsewhere-codex");
        assert_eq!(codex_skills_dir(&moved), moved.join("skills"));
        assert_eq!(
            codex_skills_dir(&moved).join(".system"),
            moved.join("skills").join(".system")
        );
    }

    #[test]
    fn an_isolated_process_keeps_to_its_own_skills() {
        let data =
            std::env::temp_dir().join(format!("trek-isolated-skills-{}", std::process::id()));
        crate::paths::isolate_thread(data.clone());
        let real = crate::paths::home();
        for h in [SkillHome::ClaudeCode, SkillHome::Codex, SkillHome::Shared] {
            assert!(
                h.dir().starts_with(&data) && !h.dir().starts_with(real.join(".claude")),
                "{}",
                h.dir().display()
            );
        }
        // Created, listed, turned off and on again: all of it inside the data folder.
        let md = create("Isolated check", "", SkillHome::ClaudeCode).unwrap();
        assert!(md.starts_with(&data), "{}", md.display());
        let found = discover(None)
            .into_iter()
            .find(|s| s.name == "isolated-check")
            .expect("listed");
        assert_eq!(found.source, SkillSource::ClaudeCode);
        let off = set_enabled(&found, false).unwrap();
        assert!(off.starts_with(&data));
        let off = discover(None)
            .into_iter()
            .find(|s| s.name == "isolated-check")
            .expect("listed off");
        assert!(set_enabled(&off, true).unwrap().starts_with(&data));
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn a_description_is_written_as_front_matter_strict_readers_accept() {
        assert_eq!(
            yaml_description("Use when reviewing PRs"),
            "Use when reviewing PRs"
        );
        assert_eq!(
            yaml_description("Use when: reviewing PRs"),
            "\"Use when: reviewing PRs\""
        );
        assert_eq!(yaml_description("Fix it # now"), "\"Fix it # now\"");
        assert_eq!(
            yaml_description("Say \"hi\" \\ bye"),
            r#""Say \"hi\" \\ bye""#
        );
        assert_eq!(yaml_description("[draft]\nnotes"), "\"[draft] notes\"");
        // What `create` writes reads back as typed.
        let data = std::env::temp_dir().join(format!("trek-skill-yaml-{}", std::process::id()));
        crate::paths::isolate_thread(data.clone());
        for typed in [
            "Use when: reviewing PRs",
            "Say \"hi\" \\ bye # ok",
            "Plain one",
        ] {
            let md = create(typed, typed, SkillHome::ClaudeCode).unwrap();
            let text = std::fs::read_to_string(&md).unwrap();
            assert!(text.contains(&format!("description: {}\n", yaml_description(typed))));
            assert_eq!(read_front_matter(&md).unwrap().1, typed);
        }
        let _ = std::fs::remove_dir_all(data);
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_linked_by_a_relative_path_can_be_turned_off_and_on() {
        let data = std::env::temp_dir().join(format!("trek-relative-skill-{}", std::process::id()));
        crate::paths::isolate_thread(data.clone());
        let home = SkillHome::ClaudeCode.dir();
        std::fs::create_dir_all(&home).unwrap();
        let real = home.parent().unwrap().join("shared-skills/linked");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("SKILL.md"), "---\nname: linked\n---\n").unwrap();
        let link = home.join("linked");
        std::os::unix::fs::symlink("../shared-skills/linked", &link).unwrap();
        let found = discover(None)
            .into_iter()
            .find(|s| s.name == "linked")
            .expect("listed");
        let off = set_enabled(&found, false).unwrap();
        assert!(
            off.join("SKILL.md").is_file(),
            "the moved link still resolves"
        );
        assert!(std::fs::symlink_metadata(&link).is_err());
        let off = discover(None)
            .into_iter()
            .find(|s| s.name == "linked")
            .expect("listed off");
        assert_eq!(set_enabled(&off, true).unwrap(), link);
        assert!(link.join("SKILL.md").is_file());
        assert!(!real.join(ORIGIN_FILE).exists());
        let _ = std::fs::remove_dir_all(data);
    }

    /// A skill with nested folders, an empty folder and (on Unix) an executable script.
    fn sample_skill(dir: &Path) -> PathBuf {
        let skill = dir.join("sample");
        std::fs::create_dir_all(skill.join("scripts/deep")).unwrap();
        std::fs::create_dir_all(skill.join("empty")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: sample\ndescription: A sample.\n---\nbody",
        )
        .unwrap();
        std::fs::write(skill.join("scripts/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(skill.join("scripts/deep/data.bin"), [0u8, 159, 146, 150]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                skill.join("scripts/run.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        skill
    }

    fn listing(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut all: Vec<_> = walkdir::WalkDir::new(root)
            .min_depth(1)
            .into_iter()
            .map(|e| {
                let e = e.unwrap();
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                (
                    rel,
                    e.file_type()
                        .is_file()
                        .then(|| std::fs::read(e.path()).unwrap()),
                )
            })
            .collect();
        all.sort();
        all
    }

    #[test]
    fn a_skill_folder_is_copied_whole() {
        let dir = std::env::temp_dir().join(format!("trek-skill-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skill = sample_skill(&dir);
        let copy = dir.join("copy");
        copy_dir(&skill, &copy).unwrap();
        assert_eq!(listing(&copy), listing(&skill));
        assert!(copy.join("empty").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode(&copy.join("scripts/run.sh")),
                0o755,
                "scripts stay executable"
            );
            assert_eq!(mode(&copy.join("SKILL.md")), mode(&skill.join("SKILL.md")));
        }
        // Like `cp -R` into a name that is taken: refused, and what was there is left alone.
        let err = copy_dir(&skill, &copy).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(listing(&copy), listing(&skill));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_a_skill_is_copied_as_a_link_and_never_followed() {
        let dir = std::env::temp_dir().join(format!("trek-skill-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skill = sample_skill(&dir);
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "not part of the skill").unwrap();
        std::os::unix::fs::symlink(&outside, skill.join("out")).unwrap();
        std::os::unix::fs::symlink("SKILL.md", skill.join("alias.md")).unwrap();
        let copy = dir.join("copy");
        copy_dir(&skill, &copy).unwrap();
        assert!(
            std::fs::symlink_metadata(copy.join("out"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(copy.join("out")).unwrap(), outside);
        assert_eq!(
            std::fs::read_link(copy.join("alias.md")).unwrap(),
            Path::new("SKILL.md")
        );
        // The skill folder itself being a link is fine: its contents are copied.
        let linked = dir.join("linked");
        std::os::unix::fs::symlink(&skill, &linked).unwrap();
        let from_link = dir.join("from-link");
        copy_dir(&linked, &from_link).unwrap();
        assert!(from_link.join("scripts/run.sh").is_file() && !from_link.is_symlink());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installing_a_skill_copies_it_into_its_home_once() {
        let data = std::env::temp_dir().join(format!("trek-skill-install-{}", std::process::id()));
        crate::paths::isolate_thread(data.clone());
        let skill = sample_skill(&data.join("source"));
        let installed = install_from(&skill, SkillHome::ClaudeCode).unwrap();
        assert!(installed.starts_with(&data), "{}", installed.display());
        assert_eq!(listing(&installed), listing(&skill));
        assert!(
            install_from(&skill, SkillHome::ClaudeCode)
                .unwrap_err()
                .to_string()
                .contains("already there")
        );
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Review PRs!"), "review-prs");
        assert_eq!(slug("  a  b "), "a-b");
    }
}
