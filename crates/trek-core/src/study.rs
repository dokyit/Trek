//! Study: learning anything with an agent as a one-to-one tutor. A session is a thread that
//! follows the guide Trek ships (`skills::STUDY`) in the learner's study folder, where the agent
//! keeps a concept map per topic (`map/<slug>.md`: claims and the links between them) and a few
//! notes about the learner (`learner.md`). Trek reads the maps back for the Study screen and keeps
//! which threads are study sessions (`sessions.json`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the learner's study folder is: `~/Trek/Study`, next to the chats (easy to find, and no
/// space in the path for an agent's shell commands to trip on). Under the data folder when that's
/// been moved (tests, `TREK_DATA_DIR`), so a trial run leaves nothing in the home folder.
pub fn dir() -> PathBuf {
    let moved = crate::paths::isolated()
        || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty());
    if moved {
        crate::paths::data_dir().join("study")
    } else {
        crate::paths::home().join("Trek").join("Study")
    }
}

/// The topics' concept maps.
pub fn map_dir(dir: &Path) -> PathBuf {
    dir.join("map")
}

/// The learner's material: what they attach to a session lands in `sources/<topic>/`, and
/// anything they drop in the folder themselves.
pub fn sources_dir(dir: &Path) -> PathBuf {
    dir.join("sources")
}

/// What `learner.md` starts as.
const LEARNER: &str = "# About me\n\n<!-- What helps a tutor teach me: background, goals, how I like to learn. \
The tutor adds to this as it learns about you; edit it freely. -->\n";

/// Make the study folder ready for a session: its `map/` and `sources/`, a `learner.md`, and a git
/// repository of its own, so Trek's file checkpoints (and rewinding a turn) work there as in a
/// project, and the maps keep their history. Trek's own record of sessions stays out of it, and
/// what Trek starts the folder with is committed, so a session's changes are only the session's.
pub fn ensure(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(map_dir(dir))?;
    std::fs::create_dir_all(sources_dir(dir))?;
    let learner = dir.join("learner.md");
    if !learner.exists() {
        std::fs::write(&learner, LEARNER)?;
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "sessions.json\n*.tmp\n")?;
    }
    if !dir.join(".git").exists() && git(dir, &["init", "-q"]) {
        commit(dir, &["learner.md", ".gitignore"], "Start the study folder");
    }
    Ok(())
}

/// Run git in `dir`, quietly; whether it succeeded.
fn git(dir: &Path, args: &[&str]) -> bool {
    crate::procs::command(crate::git::git_program())
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Commit `paths` (relative to `dir`) as `message`, with a name of Trek's own when git has none
/// set up. Best effort: the folder works the same uncommitted.
fn commit(dir: &Path, paths: &[&str], message: &str) {
    let mut add = vec!["add", "--"];
    add.extend_from_slice(paths);
    if !git(dir, &add) {
        return;
    }
    let _ = git(dir, &["commit", "-q", "-m", message])
        || git(
            dir,
            &[
                "-c",
                "user.name=Trek",
                "-c",
                "user.email=trek@localhost",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
}

/// What a session is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Probe, plan, teach: new understanding.
    #[default]
    Learn,
    /// Problems that adapt: making it usable.
    Practice,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Learn => "Learn",
            Kind::Practice => "Practice",
        }
    }
}

/// A thread that's a study session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub thread: String,
    pub kind: Kind,
    /// What the learner asked to learn or practise.
    pub topic: String,
    pub started_at: i64,
}

fn sessions_file(dir: &Path) -> PathBuf {
    dir.join("sessions.json")
}

/// Every session recorded in `dir`, the newest first. None when there's no record (or it can't be
/// read: a session's thread works the same without it).
pub fn sessions(dir: &Path) -> Vec<Session> {
    let mut out: Vec<Session> = std::fs::read(sessions_file(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out
}

/// Remember `session` (once per thread).
pub fn record(dir: &Path, session: Session) -> std::io::Result<()> {
    let mut all = sessions(dir);
    all.retain(|s| s.thread != session.thread);
    all.insert(0, session);
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?;
    let file = sessions_file(dir);
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, file)
}

/// One topic's concept map, as the Study screen lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub path: PathBuf,
    pub title: String,
    pub claims: usize,
    pub links: usize,
    /// Problems logged under Practice.
    pub practiced: usize,
    /// When the map last changed (ms since the epoch; 0 when unknown).
    pub updated_at: i64,
}

/// What a map says: (title, claims, links, problems practised). `fallback` is the title when the
/// map has no `# heading`.
pub fn read_map(text: &str, fallback: &str) -> (String, usize, usize, usize) {
    #[derive(PartialEq)]
    enum In {
        Other,
        Claims,
        Links,
        Practice,
    }
    let mut title = None;
    let mut section = In::Other;
    let (mut claims, mut links, mut practiced) = (0, 0, 0);
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(h) = line.strip_prefix("# ") {
            title.get_or_insert_with(|| h.trim().to_string());
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            section = match h.trim().to_ascii_lowercase().as_str() {
                "claims" => In::Claims,
                "links" => In::Links,
                "practice" => In::Practice,
                _ => In::Other,
            };
            continue;
        }
        let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else {
            continue;
        };
        match section {
            In::Claims => claims += 1,
            In::Links if item.contains('→') || item.contains("->") => links += 1,
            In::Practice => practiced += 1,
            _ => {}
        }
    }
    let title = title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    (title, claims, links, practiced)
}

/// Every topic's map in `dir`, the most recently changed first.
pub fn topics(dir: &Path) -> Vec<Topic> {
    let Ok(entries) = std::fs::read_dir(map_dir(dir)) else {
        return vec![];
    };
    let mut out: Vec<Topic> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let stem = path.file_stem()?.to_string_lossy().replace(['-', '_'], " ");
            let (title, claims, links, practiced) = read_map(&text, &stem);
            let updated_at = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            Some(Topic {
                path,
                title,
                claims,
                links,
                practiced,
                updated_at,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    out
}

/// A file in `sources/`, as the Study screen offers it for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    /// Its path under `sources/`, as shown: `transformers/notes.pdf`.
    pub label: String,
}

/// Sources listed before the list stops: the screen shows chips, not a file manager.
const SOURCE_ROWS: usize = 24;

/// Every file under `sources/` (two levels deep: the folder, and a topic's folder in it), the
/// newest first. Hidden files are skipped.
pub fn sources(dir: &Path) -> Vec<Source> {
    let root = sources_dir(dir);
    let mut out: Vec<(i64, Source)> = vec![];
    let walk = |at: &Path, depth: usize, out: &mut Vec<(i64, Source)>| {
        let Ok(entries) = std::fs::read_dir(at) else {
            return vec![];
        };
        let mut dirs = vec![];
        for path in entries.flatten().map(|e| e.path()) {
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                if depth == 0 {
                    dirs.push(path);
                }
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            let label = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((at, Source { path, label }));
        }
        dirs
    };
    for sub in walk(&root, 0, &mut out) {
        walk(&sub, 1, &mut out);
    }
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    out.into_iter().map(|(_, s)| s).take(SOURCE_ROWS).collect()
}

/// The folder in `sources/` for `topic`'s material: "The transformer, heavy math" →
/// `the-transformer-heavy-math`.
pub fn topic_slug(topic: &str) -> String {
    let slug = crate::skills::slug(topic);
    let mut cut = slug.as_str();
    if cut.len() > 40 {
        cut = &cut[..40];
        cut = cut.rsplit_once('-').map_or(cut, |(head, _)| head);
    }
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "topic".into()
    } else {
        cut.to_string()
    }
}

/// Keep `files` as sources for `topic`: each is copied into `sources/<topic_slug>/` (one already
/// in `sources/` stays where it is). Returns where each now is, in order, each once; a file
/// that can't be read is left out. A copy that already exists with the same size is reused, so
/// attaching the same notes twice doesn't double them.
pub fn add_sources(dir: &Path, topic: &str, files: &[PathBuf]) -> Vec<PathBuf> {
    let root = sources_dir(dir);
    let into = root.join(topic_slug(topic));
    let mut out: Vec<PathBuf> = vec![];
    for file in files {
        if !file.is_file() {
            continue;
        }
        if file.starts_with(&root) {
            if !out.contains(file) {
                out.push(file.clone());
            }
            continue;
        }
        let Some(name) = file.file_name() else {
            continue;
        };
        if std::fs::create_dir_all(&into).is_err() {
            continue;
        }
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        let mut dest = into.join(name);
        let mut n = 1;
        while dest.exists() && size(&dest) != size(file) {
            n += 1;
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = file
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            dest = into.join(format!("{stem}-{n}{ext}"));
        }
        if dest.exists() || std::fs::copy(file, &dest).is_ok() {
            if !out.contains(&dest) {
                out.push(dest);
            }
        }
    }
    // Committed as Trek's doing, so what the session changes is only the session's.
    let rel: Vec<String> = out
        .iter()
        .filter_map(|p| p.strip_prefix(dir).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    if !rel.is_empty() {
        let rel: Vec<&str> = rel.iter().map(String::as_str).collect();
        commit(dir, &rel, &format!("Add sources: {}", topic_slug(topic)));
    }
    out
}

/// The first message of a session: which kind it is, the guide to follow, what the learner
/// asked for (`topic`, in their words), and the sources they attached (`sources`, in the study
/// folder). `map`: the topic's map, when the session continues one.
pub fn prompt(
    kind: Kind,
    guide: &Path,
    dir: &Path,
    topic: &str,
    map: Option<&Path>,
    sources: &[PathBuf],
) -> String {
    let what = match kind {
        Kind::Learn => "learn",
        Kind::Practice => "practice",
    };
    let mut out = format!(
        "Study session ({what}). Follow Trek's study guide at {} — read it first, then the study \
         folder you're working in ({}): learner.md, and the concept maps in map/ that bear on this.\n\n",
        guide.display(),
        dir.display(),
    );
    match (kind, map) {
        (Kind::Learn, Some(m)) => {
            out.push_str(&format!("Continue the topic in {}: {topic}", m.display()))
        }
        (Kind::Learn, None) => out.push_str(&format!("What I want to learn: {topic}")),
        (Kind::Practice, Some(m)) => {
            out.push_str(&format!("Practise the claims in {}: {topic}", m.display()))
        }
        (Kind::Practice, None) => out.push_str(&format!("What I want to practise: {topic}")),
    }
    if !sources.is_empty() {
        out.push_str("\n\nSources I've added for this session (read them before you start):");
        for s in sources {
            out.push_str(&format!("\n- {}", s.display()));
        }
    }
    out
}

/// A session thread's title: "Learn · Transformers".
pub fn title(kind: Kind, topic: &str) -> String {
    let topic = crate::import_title(topic);
    format!("{} · {topic}", kind.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_is_read_for_its_title_and_counts() {
        let map = "# Transformers\n\n## Claims\n- **c1** Rows are tokens.\n- **c2** Softmax rows sum to 1.\n\n\
                   ## Links\n- c1, c2 → c3: mixing.\n- not a link\n- c3 -> other#c1: across maps\n\n\
                   ## Practice\n- 2026-10-28 · c1 · solved\n";
        assert_eq!(read_map(map, "x"), ("Transformers".into(), 2, 2, 1));
        // No heading: the file's name; nothing listed: none of each.
        assert_eq!(
            read_map("just text\n- loose", "attention"),
            ("attention".into(), 0, 0, 0)
        );
    }

    #[test]
    fn sessions_are_kept_newest_first_once_per_thread() {
        let dir = std::env::temp_dir().join(format!("trek-study-{}", uuid::Uuid::new_v4()));
        let s = |thread: &str, at| Session {
            thread: thread.into(),
            kind: Kind::Learn,
            topic: "LLMs".into(),
            started_at: at,
        };
        assert!(sessions(&dir).is_empty());
        record(&dir, s("a", 1)).unwrap();
        record(&dir, s("b", 2)).unwrap();
        record(
            &dir,
            Session {
                kind: Kind::Practice,
                ..s("a", 3)
            },
        )
        .unwrap();
        let all = sessions(&dir);
        assert_eq!(
            all.iter().map(|s| s.thread.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(all[0].kind, Kind::Practice);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_folder_is_made_ready_and_its_maps_listed() {
        let dir = std::env::temp_dir().join(format!("trek-study-{}", uuid::Uuid::new_v4()));
        ensure(&dir).unwrap();
        assert!(dir.join("learner.md").exists() && dir.join("sources").is_dir());
        // Nothing Trek put there shows as a change: it's committed, or ignored.
        record(
            &dir,
            Session {
                thread: "t".into(),
                kind: Kind::Learn,
                topic: "x".into(),
                started_at: 1,
            },
        )
        .unwrap();
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(String::from_utf8_lossy(&status.stdout), "");
        std::fs::write(
            map_dir(&dir).join("gradient-descent.md"),
            "## Claims\n- **c1** x\n",
        )
        .unwrap();
        std::fs::write(map_dir(&dir).join("notes.txt"), "not a map").unwrap();
        let topics = topics(&dir);
        assert_eq!(topics.len(), 1);
        assert_eq!(
            (topics[0].title.as_str(), topics[0].claims),
            ("gradient descent", 1)
        );
        // A learner's own notes survive another session.
        std::fs::write(dir.join("learner.md"), "mine").unwrap();
        ensure(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("learner.md")).unwrap(),
            "mine"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_session_says_what_it_is_and_where_its_guide_is() {
        let p = prompt(
            Kind::Practice,
            Path::new("/g/study/SKILL.md"),
            Path::new("/s"),
            "attention",
            Some(Path::new("/s/map/llms.md")),
            &[],
        );
        assert!(p.starts_with("Study session (practice)."));
        assert!(p.contains("/g/study/SKILL.md") && p.contains("/s/map/llms.md"));
        assert!(!p.contains("Sources"));
        assert_eq!(
            title(Kind::Learn, "the transformer"),
            "Learn · the transformer"
        );
        // What the learner attached is listed for the tutor to read first.
        let p = prompt(
            Kind::Learn,
            Path::new("/g/study/SKILL.md"),
            Path::new("/s"),
            "attention",
            None,
            &[PathBuf::from("/s/sources/attention/notes.pdf")],
        );
        assert!(p.ends_with(
            "What I want to learn: attention\n\nSources I've added for this session (read them \
             before you start):\n- /s/sources/attention/notes.pdf"
        ));
    }

    #[test]
    fn attached_files_are_copied_into_the_topics_sources() {
        let dir = std::env::temp_dir().join(format!("trek-study-{}", uuid::Uuid::new_v4()));
        ensure(&dir).unwrap();
        // The learner's own files, outside the folder.
        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let notes = outside.join("notes.md");
        std::fs::write(&notes, "# notes").unwrap();
        let other = outside.join("other.txt");
        std::fs::write(&other, "x").unwrap();
        // Already in sources/: used where it is.
        let mine = sources_dir(&dir).join("sheet.md");
        std::fs::write(&mine, "sheet").unwrap();
        let got = add_sources(
            &dir,
            "The transformer, heavy math!",
            &[
                notes.clone(),
                mine.clone(),
                dir.join("missing.pdf"),
                notes.clone(),
            ],
        );
        let copy = sources_dir(&dir)
            .join("the-transformer-heavy-math")
            .join("notes.md");
        assert_eq!(got, vec![copy.clone(), mine.clone()]);
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "# notes");
        // Committed as Trek's doing: nothing shows as changed before the session starts.
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "");
        // The same file again reuses its copy; a different file of the same name sits beside it.
        assert_eq!(
            add_sources(&dir, "the transformer, heavy math", &[notes]),
            vec![copy.clone()]
        );
        std::fs::write(outside.join("notes.md"), "# longer notes").unwrap();
        let again = add_sources(
            &dir,
            "the transformer, heavy math",
            &[outside.join("notes.md")],
        );
        assert_eq!(again[0].file_name().unwrap(), "notes-2.md");
        // Listed newest first, with their place under sources/.
        let listed = sources(&dir);
        let labels: Vec<&str> = listed.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels.len(), 3, "{labels:?}");
        assert!(labels.contains(&"sheet.md"));
        assert!(labels.contains(&"the-transformer-heavy-math/notes.md"));
        assert_eq!(topic_slug("   "), "topic");
        assert_eq!(
            topic_slug("a very long topic name that goes on and on and on forever"),
            "a-very-long-topic-name-that-goes-on-and"
        );
        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(dir);
    }
}
