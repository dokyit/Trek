//! Agent skills on this Mac: folders holding a `SKILL.md` (front matter `name` + `description`).
//!
//! Turning a skill off moves its folder into Trek's data folder (with a note of where it came
//! from), so every agent stops loading it; turning it back on moves it home. Skills that another
//! tool manages — Claude.ai sync, Codex's built-ins, plugins — are listed read-only, as are the
//! ones Trek ships itself (`SHIPPED`), kept in its data folder for any agent to follow.

use crate::paths;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SkillSource {
    /// `~/.claude/skills`
    ClaudeCode,
    /// `~/.codex/skills`
    Codex,
    /// `~/.agents/skills`, read by several agents.
    Shared,
    /// `<project>/.claude/skills` or `<project>/.agents/skills`
    Project,
    /// `~/.claude/skills/synced/…`, managed by Claude.
    Synced,
    /// `~/.codex/skills/.system`, shipped with Codex.
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
        matches!(self, SkillSource::ClaudeCode | SkillSource::Codex | SkillSource::Shared | SkillSource::Project)
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

impl SkillHome {
    pub fn dir(self) -> PathBuf {
        let home = paths::agents_home();
        match self {
            SkillHome::ClaudeCode => home.join(".claude/skills"),
            SkillHome::Codex => home.join(".codex/skills"),
            SkillHome::Shared => home.join(".agents/skills"),
        }
    }
}

/// The guide that builds a project's verification skill (`crate::verification`).
pub const CREATE_VERIFICATION: &str = "create-verification-skill";
/// The guide that brings one up to date.
pub const MAINTAIN_VERIFICATION: &str = "maintain-verification-skill";

/// Skills Trek ships: (folder name, SKILL.md).
pub const SHIPPED: [(&str, &str); 2] = [
    (CREATE_VERIFICATION, include_str!("../skills/create-verification-skill/SKILL.md")),
    (MAINTAIN_VERIFICATION, include_str!("../skills/maintain-verification-skill/SKILL.md")),
];

/// Where Trek keeps the skills it ships. Not one of the agents' own skill folders: Trek points an
/// agent at one when it starts work that follows it, and the user's agents are left as they are.
pub fn shipped_root() -> PathBuf {
    paths::data_dir().join("skills")
}

/// The SKILL.md of shipped skill `name`, written (or brought up to this version) first.
pub fn shipped(name: &str) -> anyhow::Result<PathBuf> {
    let (_, text) = SHIPPED.iter().find(|(n, _)| *n == name).ok_or_else(|| anyhow::anyhow!("Trek ships no skill called {name}"))?;
    let md = shipped_root().join(name).join("SKILL.md");
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
    let folder = skill_md.parent()?.file_name()?.to_string_lossy().to_string();
    let mut name = None;
    let mut desc = None;
    if let Some(rest) = text.strip_prefix("---") {
        for line in rest.lines().skip(1) {
            if line.trim() == "---" {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                match k.trim() {
                    "name" if !v.is_empty() => name = Some(v),
                    "description" if !v.is_empty() => desc = Some(v),
                    _ => {}
                }
            }
        }
    }
    let desc = desc.or_else(|| {
        // No front matter: the first prose line.
        text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---")).map(str::to_string)
    });
    Some((name.unwrap_or(folder), desc.unwrap_or_default()))
}

/// Skill folders directly inside `root` (following symlinks).
fn scan(root: &Path, source: SkillSource, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let dir = e.path();
        let file_name = e.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') || !dir.is_dir() {
            continue;
        }
        let md = dir.join("SKILL.md");
        if let Some((name, description)) = md.exists().then(|| read_front_matter(&md)).flatten() {
            out.push(Skill { name, description, dir, source: source.clone(), enabled: true });
        }
    }
}

/// Every skill Trek can see, enabled and disabled, for the given project.
pub fn discover(project: Option<&Path>) -> Vec<Skill> {
    let home = paths::agents_home();
    let mut out = vec![];
    scan(&home.join(".claude/skills"), SkillSource::ClaudeCode, &mut out);
    scan(&home.join(".codex/skills"), SkillSource::Codex, &mut out);
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
    scan(&home.join(".codex/skills/.system"), SkillSource::CodexSystem, &mut out);
    for (name, _) in SHIPPED {
        let _ = shipped(name);
    }
    scan(&shipped_root(), SkillSource::Trek, &mut out);
    for (plugin, dir) in plugin_dirs() {
        scan(&dir.join("skills"), SkillSource::Plugin(plugin), &mut out);
    }
    // Disabled ones, back under the source they came from.
    if let Ok(entries) = std::fs::read_dir(disabled_root()) {
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(origin) = std::fs::read_to_string(dir.join(ORIGIN_FILE)) else { continue };
            let origin = PathBuf::from(origin.trim());
            let source = source_for(&origin, project);
            if let Some((name, description)) = read_front_matter(&dir.join("SKILL.md")) {
                out.push(Skill { name, description, dir, source, enabled: false });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn source_for(origin: &Path, project: Option<&Path>) -> SkillSource {
    let home = paths::agents_home();
    if origin.starts_with(home.join(".claude/skills")) {
        SkillSource::ClaudeCode
    } else if origin.starts_with(home.join(".codex/skills")) {
        SkillSource::Codex
    } else if origin.starts_with(home.join(".agents/skills")) {
        SkillSource::Shared
    } else if project.is_some_and(|p| origin.starts_with(p)) {
        SkillSource::Project
    } else {
        SkillSource::Project
    }
}

/// Installed Claude Code plugins: `(name, install folder)`.
fn plugin_dirs() -> Vec<(String, PathBuf)> {
    let file = paths::agents_home().join(".claude/plugins/installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(file) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    let mut out = vec![];
    for (name, installs) in v["plugins"].as_object().into_iter().flatten() {
        for i in installs.as_array().into_iter().flatten() {
            if let Some(p) = i["installPath"].as_str() {
                out.push((name.split('@').next().unwrap_or(name).to_string(), PathBuf::from(p)));
            }
        }
    }
    out
}

/// Turn a skill off (move it aside) or back on (move it home).
pub fn set_enabled(skill: &Skill, on: bool) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(skill.source.editable(), "{} skills are managed elsewhere", skill.source.label());
    if on == skill.enabled {
        return Ok(skill.dir.clone());
    }
    if on {
        let origin = PathBuf::from(std::fs::read_to_string(skill.dir.join(ORIGIN_FILE))?.trim());
        anyhow::ensure!(!origin.exists(), "A skill already exists at {}", origin.display());
        if let Some(parent) = origin.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(skill.dir.join(ORIGIN_FILE));
        move_dir(&skill.dir, &origin)?;
        Ok(origin)
    } else {
        let root = disabled_root();
        std::fs::create_dir_all(&root)?;
        let folder = skill.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| skill.name.clone());
        let mut dest = root.join(&folder);
        let mut n = 2;
        while dest.exists() {
            dest = root.join(format!("{folder}-{n}"));
            n += 1;
        }
        // A symlinked skill: move the link, not what it points at.
        move_dir(&skill.dir, &dest)?;
        std::fs::write(dest.join(ORIGIN_FILE), skill.dir.display().to_string()).or_else(|e| {
            // A moved symlink to a read-only target: keep the origin beside it instead.
            let _ = move_dir(&dest, &skill.dir);
            Err(e)
        })?;
        Ok(dest)
    }
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
            copy_dir(from, to).map_err(|e| e.context(format!("Couldn't move {}", from.display())))?;
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
    anyhow::ensure!(std::fs::symlink_metadata(to).is_err(), "{} already exists", to.display());
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
    anyhow::ensure!(skill.source.editable(), "{} skills are managed elsewhere", skill.source.label());
    paths::trash(&skill.dir).map_err(|e| e.context(format!("Couldn't move {} to the Trash", skill.dir.display())))
}

/// Copy a skill folder (one containing SKILL.md) into `home`.
pub fn install_from(folder: &Path, home: SkillHome) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(folder.join("SKILL.md").exists(), "That folder has no SKILL.md");
    let name = folder.file_name().ok_or_else(|| anyhow::anyhow!("Not a folder"))?;
    let root = home.dir();
    std::fs::create_dir_all(&root)?;
    let dest = root.join(name);
    anyhow::ensure!(!dest.exists(), "A skill named {} is already there", name.to_string_lossy());
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

/// Create a new skill with a starter SKILL.md; returns the file to open.
pub fn create(name: &str, description: &str, home: SkillHome) -> anyhow::Result<PathBuf> {
    let slug = slug(name);
    anyhow::ensure!(!slug.is_empty(), "Give the skill a name");
    let dir = home.dir().join(&slug);
    anyhow::ensure!(!dir.exists(), "A skill named {slug} already exists");
    std::fs::create_dir_all(&dir)?;
    let description = if description.trim().is_empty() { "Describe when an agent should use this skill." } else { description.trim() };
    let md = dir.join("SKILL.md");
    std::fs::write(
        &md,
        format!("---\nname: {slug}\ndescription: {description}\n---\n\n# {}\n\nWrite the instructions the agent should follow here.\n", name.trim()),
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
        std::fs::write(s.join("SKILL.md"), "---\nname: grill-me\ndescription: \"Ask hard questions.\"\n---\nbody").unwrap();
        assert_eq!(read_front_matter(&s.join("SKILL.md")), Some(("grill-me".into(), "Ask hard questions.".into())));
        std::fs::write(s.join("SKILL.md"), "# Title\n\nDoes a thing.").unwrap();
        assert_eq!(read_front_matter(&s.join("SKILL.md")), Some(("my-skill".into(), "Does a thing.".into())));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shipped_skills_are_written_and_kept_current() {
        crate::paths::isolate_thread(std::env::temp_dir().join(format!("trek-shipped-skills-{}", std::process::id())));
        let md = shipped(CREATE_VERIFICATION).unwrap();
        assert!(md.starts_with(shipped_root()));
        let (name, desc) = read_front_matter(&md).unwrap();
        assert_eq!(name, CREATE_VERIFICATION);
        assert!(desc.contains("verification skill"), "{desc}");
        // An old copy is brought up to this version.
        std::fs::write(&md, "stale").unwrap();
        shipped(CREATE_VERIFICATION).unwrap();
        assert!(std::fs::read_to_string(&md).unwrap().contains("metadata:\n  trek: verification"));
        let listed: Vec<Skill> = discover(None).into_iter().filter(|s| s.source == SkillSource::Trek).collect();
        assert_eq!(listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), [CREATE_VERIFICATION, MAINTAIN_VERIFICATION]);
        assert!(!SkillSource::Trek.editable(), "read-only");
        assert!(shipped("nope").is_err());
    }

    #[test]
    fn an_isolated_process_keeps_to_its_own_skills() {
        let data = std::env::temp_dir().join(format!("trek-isolated-skills-{}", std::process::id()));
        crate::paths::isolate_thread(data.clone());
        let real = crate::paths::home();
        for h in [SkillHome::ClaudeCode, SkillHome::Codex, SkillHome::Shared] {
            assert!(h.dir().starts_with(&data) && !h.dir().starts_with(real.join(".claude")), "{}", h.dir().display());
        }
        // Created, listed, turned off and on again: all of it inside the data folder.
        let md = create("Isolated check", "", SkillHome::ClaudeCode).unwrap();
        assert!(md.starts_with(&data), "{}", md.display());
        let found = discover(None).into_iter().find(|s| s.name == "isolated-check").expect("listed");
        assert_eq!(found.source, SkillSource::ClaudeCode);
        let off = set_enabled(&found, false).unwrap();
        assert!(off.starts_with(&data));
        let off = discover(None).into_iter().find(|s| s.name == "isolated-check").expect("listed off");
        assert!(set_enabled(&off, true).unwrap().starts_with(&data));
        let _ = std::fs::remove_dir_all(data);
    }

    /// A skill with nested folders, an empty folder and (on Unix) an executable script.
    fn sample_skill(dir: &Path) -> PathBuf {
        let skill = dir.join("sample");
        std::fs::create_dir_all(skill.join("scripts/deep")).unwrap();
        std::fs::create_dir_all(skill.join("empty")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: sample\ndescription: A sample.\n---\nbody").unwrap();
        std::fs::write(skill.join("scripts/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(skill.join("scripts/deep/data.bin"), [0u8, 159, 146, 150]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(skill.join("scripts/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        skill
    }

    fn listing(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut all: Vec<_> = walkdir::WalkDir::new(root)
            .min_depth(1)
            .into_iter()
            .map(|e| {
                let e = e.unwrap();
                let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                (rel, e.file_type().is_file().then(|| std::fs::read(e.path()).unwrap()))
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
            assert_eq!(mode(&copy.join("scripts/run.sh")), 0o755, "scripts stay executable");
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
        assert!(std::fs::symlink_metadata(copy.join("out")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(copy.join("out")).unwrap(), outside);
        assert_eq!(std::fs::read_link(copy.join("alias.md")).unwrap(), Path::new("SKILL.md"));
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
        assert!(install_from(&skill, SkillHome::ClaudeCode).unwrap_err().to_string().contains("already there"));
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Review PRs!"), "review-prs");
        assert_eq!(slug("  a  b "), "a-b");
    }
}
