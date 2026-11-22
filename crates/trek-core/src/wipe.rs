//! "Delete all Trek data" (Settings › General): what goes and what stays, worked out on its own
//! ([`plan`], which reads the folders and changes nothing) and then done ([`execute`]).
//!
//! The app hands over the folders it writes, as its own functions resolve them (the data folder,
//! the logs, the caches), the folders inside them that stay (Trek's worktrees, an agent's second
//! sign-in), and the secret store's accounts. The plan takes each folder's entries one by one:
//! a folder that holds something that stays is gone into rather than deleted whole. Nothing is
//! followed out of a folder: a link inside one is deleted as a link, never what it points at, and
//! a folder that is itself a link is refused. So is one that could be someone else's: the home
//! folder or a folder above it, a drive or `/`, a relative path, or one with no "trek" in its
//! name (`TREK_DATA_DIR=~/Documents`), so a wrong path can only ever leave things behind.

use std::path::{Component, Path, PathBuf};

/// A folder Trek owns outright: everything in it goes but what [`Request::keep`] names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    /// What's in it, for the confirmation ("Threads, settings and notes").
    pub what: String,
}

/// A path inside a root that stays, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keep {
    pub path: PathBuf,
    pub what: String,
}

/// What the secret store holds of this profile's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Secrets {
    /// These accounts, under Trek's service, go.
    Delete(Vec<String>),
    /// None go, and why (a profile in another folder shares the store with the user's own Trek).
    Keep(String),
}

/// What to delete, as the app resolves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The user's home folder: never a root, nor anything above it.
    pub home: PathBuf,
    pub roots: Vec<Root>,
    /// Single files of Trek's outside its folders (the Mac's cookie file).
    pub files: Vec<Root>,
    pub keep: Vec<Keep>,
    pub secrets: Secrets,
}

/// One thing to delete: a file, a link or a folder (with everything in it), directly inside
/// `parent`, a root or a folder of one that holds something kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// The root it's in: every folder from there down to it must still be a folder, not a link.
    pub root: PathBuf,
}

/// What `execute` will do: worked out by [`plan`] without changing anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub targets: Vec<Target>,
    /// The roots that go too once they're empty (none that keeps something).
    pub roots: Vec<PathBuf>,
    /// The roots there are, and what's in them, for the confirmation.
    pub listed: Vec<Root>,
    /// What stays that's there.
    pub kept: Vec<Keep>,
    /// Roots and files refused, and why: they're left as they are.
    pub refused: Vec<(PathBuf, String)>,
    pub secrets: Vec<String>,
    /// Why no secret goes, when none does.
    pub secrets_kept: Option<String>,
}

/// What couldn't be deleted, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub left: Vec<(PathBuf, String)>,
    pub secrets_left: Vec<String>,
}

/// Why `path` can't be a root (or a file) to delete, if it can't.
pub fn refusal(path: &Path, home: &Path) -> Option<String> {
    if !path.is_absolute() {
        return Some("not a full path".into());
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Some("it has .. in it".into());
    }
    // A drive or `/`, the home folder, or a folder above it.
    if path.parent().is_none() || home.starts_with(path) {
        return Some("it holds the home folder".into());
    }
    // Named for Trek below the home folder (a user called "trekker" doesn't make every folder of
    // theirs Trek's), or anywhere in a path outside it (the temp folder's `trek-…` profiles).
    let named = path
        .strip_prefix(home)
        .unwrap_or(path)
        .components()
        .skip_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("trek")
        });
    if !named {
        return Some("it isn't one of Trek's folders by its name".into());
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Some("it's a link to somewhere else".into()),
        _ => None,
    }
}

/// Work out what `request` deletes: every entry of each root but what's kept (a folder that holds
/// something kept is gone into, entry by entry), the files, and the secrets. Reads the folders;
/// changes nothing.
pub fn plan(request: &Request) -> Plan {
    let mut plan = Plan::default();
    let keeps: Vec<&Keep> = request.keep.iter().collect();
    let mut seen = Vec::<PathBuf>::new();
    for root in &request.roots {
        let path = &root.path;
        // The same folder twice (the local data folder is the data folder on a Mac), or one
        // inside another already listed.
        if seen.iter().any(|s| path.starts_with(s)) {
            continue;
        }
        if let Some(why) = refusal(path, &request.home) {
            if path.exists() || std::fs::symlink_metadata(path).is_ok() {
                plan.refused.push((path.clone(), why));
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        seen.push(path.clone());
        plan.listed.push(root.clone());
        let holds_kept = keeps.iter().any(|k| k.path.starts_with(path));
        walk(path, path, &keeps, &mut plan.targets);
        if !holds_kept {
            plan.roots.push(path.clone());
        }
    }
    for file in &request.files {
        if seen.iter().any(|s| file.path.starts_with(s)) {
            continue;
        }
        if let Some(why) = refusal(&file.path, &request.home) {
            if std::fs::symlink_metadata(&file.path).is_ok() {
                plan.refused.push((file.path.clone(), why));
            }
            continue;
        }
        let (Ok(_), Some(parent)) = (std::fs::symlink_metadata(&file.path), file.path.parent())
        else {
            continue;
        };
        plan.listed.push(file.clone());
        plan.targets.push(Target {
            path: file.path.clone(),
            root: parent.to_path_buf(),
        });
    }
    plan.kept = request
        .keep
        .iter()
        .filter(|k| std::fs::symlink_metadata(&k.path).is_ok())
        .filter(|k| plan.listed.iter().any(|r| k.path.starts_with(&r.path)))
        .cloned()
        .collect();
    match &request.secrets {
        Secrets::Delete(accounts) => {
            let mut accounts = accounts.clone();
            accounts.sort();
            accounts.dedup();
            plan.secrets = accounts;
        }
        Secrets::Keep(why) => plan.secrets_kept = Some(why.clone()),
    }
    plan
}

/// `dir`'s entries as targets, but those kept: a folder that holds something kept is gone into.
/// Links are taken as links, never followed.
fn walk(root: &Path, dir: &Path, keeps: &[&Keep], out: &mut Vec<Target>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if keeps.iter().any(|k| k.path == path) {
            continue;
        }
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir());
        if is_dir && keeps.iter().any(|k| k.path.starts_with(&path)) {
            walk(root, &path, keeps, out);
            continue;
        }
        out.push(Target {
            path,
            root: root.to_path_buf(),
        });
    }
}

/// Delete what `plan` lists, `delete_secret` taking each secret: a link as a link, a folder with
/// everything in it (its links as links), each only while its folder is still the one planned
/// (not swapped for a link since). Then the roots left empty. What couldn't go is listed.
pub fn execute(plan: &Plan, delete_secret: impl Fn(&str) -> anyhow::Result<()>) -> Outcome {
    let mut outcome = Outcome::default();
    for account in &plan.secrets {
        if let Err(e) = delete_secret(account) {
            tracing::warn!("delete all data: secret {account}: {e:#}");
            outcome.secrets_left.push(account.clone());
        }
    }
    for target in &plan.targets {
        if let Err(e) = delete(target) {
            if e.kind() != std::io::ErrorKind::NotFound {
                outcome.left.push((target.path.clone(), e.to_string()));
            }
        }
    }
    for root in &plan.roots {
        // Only if it's empty: anything left in it is in `left` already.
        match std::fs::symlink_metadata(root) {
            Ok(m) if m.is_dir() => {
                let _ = std::fs::remove_dir(root);
            }
            _ => {}
        }
    }
    outcome
}

fn delete(target: &Target) -> std::io::Result<()> {
    // Every folder from the root down to it must still be a folder: a link put in the place of
    // one would lead out of Trek's folders.
    let Ok(below) = target.path.strip_prefix(&target.root) else {
        return Err(std::io::Error::other("it isn't in its folder"));
    };
    let mut parts: Vec<_> = below.components().collect();
    parts.pop();
    let mut folder = target.root.clone();
    if !std::fs::symlink_metadata(&folder)?.is_dir() {
        return Err(std::io::Error::other("its folder isn't the one planned"));
    }
    for part in parts {
        folder.push(part);
        if !std::fs::symlink_metadata(&folder)?.is_dir() {
            return Err(std::io::Error::other("its folder isn't the one planned"));
        }
    }
    let meta = std::fs::symlink_metadata(&target.path)?;
    if meta.file_type().is_symlink() {
        // A link to a folder is a folder on Windows (and a junction is one too).
        return std::fs::remove_file(&target.path).or_else(|_| std::fs::remove_dir(&target.path));
    }
    if meta.is_dir() {
        // Since Rust 1.58.1 this takes links inside as links, on every platform.
        return std::fs::remove_dir_all(&target.path);
    }
    std::fs::remove_file(&target.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder of the test's own, with a "trek" name like Trek's own.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trek-wipe-tests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn root(path: &Path) -> Root {
        Root {
            path: path.to_path_buf(),
            what: "Trek's data".into(),
        }
    }

    fn keep(path: &Path) -> Keep {
        Keep {
            path: path.to_path_buf(),
            what: "kept".into(),
        }
    }

    #[cfg(unix)]
    fn link(to: &Path, at: &Path) {
        std::os::unix::fs::symlink(to, at).unwrap();
    }

    #[cfg(windows)]
    fn link(to: &Path, at: &Path) {
        // A directory symlink needs Developer Mode or admin; a junction doesn't.
        if std::os::windows::fs::symlink_dir(to, at).is_err() {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(at)
                .arg(to)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    /// The profile goes, entry by entry, with what's kept left where it is; the profile beside it
    /// (the verify app's, a dev build's) and a folder a link inside led to are untouched.
    #[test]
    fn a_profile_goes_and_its_sibling_and_what_a_link_led_to_stay() {
        let base = scratch("profiles");
        let profile = base.join("trek-data");
        let sibling = base.join("trek-data-verify");
        let outside = scratch("outside");
        write(&profile.join("settings.toml"), "x");
        write(&profile.join("trek.sqlite"), "x");
        write(&profile.join("transcripts/t1.log"), "x");
        write(&profile.join("notes/a.md"), "x");
        write(&profile.join("worktrees/repo/slug/file.rs"), "work");
        write(&profile.join("accounts/second/.credentials.json"), "login");
        write(&profile.join("accounts-usage.json"), "x");
        write(&sibling.join("settings.toml"), "sibling");
        write(&outside.join("decoy.txt"), "keep me");
        link(&outside, &profile.join("link-out"));
        link(&outside, &profile.join("notes/link-out"));

        let request = Request {
            home: PathBuf::from("/nowhere-home"),
            roots: vec![root(&profile)],
            files: vec![],
            keep: vec![
                keep(&profile.join("worktrees")),
                keep(&profile.join("accounts")),
            ],
            secrets: Secrets::Delete(vec!["openai".into(), "anthropic".into(), "openai".into()]),
        };
        let plan = plan(&request);
        let names: Vec<String> = plan
            .targets
            .iter()
            .map(|t| t.path.strip_prefix(&profile).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "accounts-usage.json",
                "link-out",
                "notes",
                "settings.toml",
                "transcripts",
                "trek.sqlite"
            ],
            "the kept folders aren't targets, and nothing outside is"
        );
        assert!(
            plan.roots.is_empty(),
            "the profile keeps something: it stays"
        );
        assert_eq!(plan.kept.len(), 2);
        assert_eq!(plan.secrets, ["anthropic", "openai"]);
        assert!(plan.refused.is_empty());
        // Planning changed nothing.
        assert!(profile.join("settings.toml").exists());

        let deleted = std::cell::RefCell::new(vec![]);
        let outcome = execute(&plan, |a| {
            deleted.borrow_mut().push(a.to_string());
            Ok(())
        });
        assert_eq!(outcome, Outcome::default());
        assert_eq!(*deleted.borrow(), ["anthropic", "openai"]);
        let mut left: Vec<String> = std::fs::read_dir(&profile)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["accounts", "worktrees"]);
        assert!(profile.join("worktrees/repo/slug/file.rs").exists());
        assert!(profile.join("accounts/second/.credentials.json").exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("decoy.txt")).unwrap(),
            "keep me",
            "a link inside was deleted as a link"
        );
        assert_eq!(
            std::fs::read_to_string(sibling.join("settings.toml")).unwrap(),
            "sibling"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A root with nothing kept in it goes too, and one inside another is taken once; a kept
    /// folder deeper down (threads without a project, under a moved data folder) is gone round.
    #[test]
    fn an_empty_root_goes_and_a_deep_keep_is_gone_round() {
        let base = scratch("roots");
        let data = base.join("trek-data");
        let logs = base.join("trek-logs");
        write(&data.join("chats/2026-01-01/notes.md"), "mine");
        write(&data.join("chats-index.json"), "x");
        write(&data.join("cache/videos/a/frame.jpg"), "x");
        write(&logs.join("trek-2026-11-22.log"), "x");
        let request = Request {
            home: PathBuf::from("/nowhere-home"),
            roots: vec![root(&data), root(&logs), root(&data.join("cache"))],
            files: vec![],
            keep: vec![keep(&data.join("chats"))],
            secrets: Secrets::Keep("shared".into()),
        };
        let plan = plan(&request);
        assert_eq!(plan.roots, [logs.clone()]);
        assert_eq!(plan.listed.len(), 2, "cache is inside the data folder");
        assert_eq!(plan.secrets_kept.as_deref(), Some("shared"));
        assert!(plan.secrets.is_empty());
        let outcome = execute(&plan, |_| panic!("no secret goes"));
        assert!(outcome.left.is_empty(), "{:?}", outcome.left);
        assert!(!logs.exists(), "an emptied root goes");
        assert!(data.join("chats/2026-01-01/notes.md").exists());
        assert!(!data.join("cache").exists() && !data.join("chats-index.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A folder that could be someone else's is refused, and left as it is.
    #[test]
    fn roots_that_could_be_anyones_are_refused() {
        let base = scratch("refused");
        let home = base.join("home");
        let documents = home.join("Documents");
        write(&documents.join("thesis.md"), "mine");
        let elsewhere = scratch("link-target");
        write(&elsewhere.join("x"), "x");
        let linked = base.join("trek-linked");
        link(&elsewhere, &linked);
        for (path, why) in [
            (home.clone(), "it holds the home folder"),
            (base.clone(), "it holds the home folder"),
            (
                documents.clone(),
                "it isn't one of Trek's folders by its name",
            ),
            (linked.clone(), "it's a link to somewhere else"),
            (
                home.join("trek").join("..").join("Documents"),
                "it has .. in it",
            ),
        ] {
            assert_eq!(refusal(&path, &home).as_deref(), Some(why), "{path:?}");
        }
        assert_eq!(
            refusal(Path::new("relative/trek"), &home).as_deref(),
            Some("not a full path")
        );
        let top = if cfg!(windows) { "C:\\" } else { "/" };
        assert_eq!(
            refusal(Path::new(top), &home).as_deref(),
            Some("it holds the home folder")
        );
        assert_eq!(refusal(&home.join("Library/Logs/Trek"), &home), None);

        let request = Request {
            home: home.clone(),
            roots: vec![root(&home), root(&documents), root(&linked)],
            files: vec![root(&documents.join("thesis.md"))],
            keep: vec![],
            secrets: Secrets::Delete(vec![]),
        };
        let plan = plan(&request);
        assert!(plan.targets.is_empty() && plan.roots.is_empty());
        assert_eq!(plan.refused.len(), 4);
        execute(&plan, |_| Ok(()));
        assert!(documents.join("thesis.md").exists() && elsewhere.join("x").exists());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    /// A folder swapped for a link between planning and deleting isn't followed.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_after_planning_is_left() {
        let base = scratch("swap");
        let data = base.join("trek-data");
        let outside = scratch("swap-outside");
        write(&data.join("notes/sub/a.md"), "x");
        write(&outside.join("sub/a.md"), "keep me");
        let request = Request {
            home: PathBuf::from("/nowhere-home"),
            roots: vec![root(&data)],
            files: vec![],
            keep: vec![keep(&data.join("notes/sub/keep.md"))],
            secrets: Secrets::Delete(vec![]),
        };
        let plan = plan(&request);
        assert_eq!(plan.targets.len(), 1, "{:?}", plan.targets);
        // The folder above the one it's in, swapped.
        std::fs::remove_dir_all(data.join("notes")).unwrap();
        link(&outside, &data.join("notes"));
        let outcome = execute(&plan, |_| Ok(()));
        assert_eq!(outcome.left.len(), 1);
        assert!(outside.join("sub/a.md").exists());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
