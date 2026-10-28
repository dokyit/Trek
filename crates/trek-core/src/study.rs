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

/// What `learner.md` starts as.
const LEARNER: &str = "# About me\n\n<!-- What helps a tutor teach me: background, goals, how I like to learn. \
The tutor adds to this as it learns about you; edit it freely. -->\n";

/// Make the study folder ready for a session: its `map/` and `sources/`, a `learner.md`, and a git
/// repository of its own, so Trek's file checkpoints (and rewinding a turn) work there as in a
/// project, and the maps keep their history. Trek's own record of sessions stays out of it, and
/// what Trek starts the folder with is committed, so a session's changes are only the session's.
pub fn ensure(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(map_dir(dir))?;
    std::fs::create_dir_all(dir.join("sources"))?;
    let learner = dir.join("learner.md");
    if !learner.exists() {
        std::fs::write(&learner, LEARNER)?;
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "sessions.json\n*.tmp\n")?;
    }
    if !dir.join(".git").exists() {
        let git = |args: &[&str]| {
            crate::procs::command(crate::git::git_program())
                .args(args)
                .current_dir(dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if git(&["init", "-q"]) && git(&["add", "learner.md", ".gitignore"]) {
            // A name of its own for the first commit when git has none set up.
            let _ = git(&["commit", "-q", "-m", "Start the study folder"])
                || git(&[
                    "-c",
                    "user.name=Trek",
                    "-c",
                    "user.email=trek@localhost",
                    "commit",
                    "-q",
                    "-m",
                    "Start the study folder",
                ]);
        }
    }
    Ok(())
}

/// What a session is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Probe, plan, teach: new understanding.
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

/// The first message of a session: which kind it is, the guide to follow, and what the learner
/// asked for (`topic`, in their words). `map`: the topic's map, when the session continues one.
pub fn prompt(kind: Kind, guide: &Path, dir: &Path, topic: &str, map: Option<&Path>) -> String {
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
        );
        assert!(p.starts_with("Study session (practice)."));
        assert!(p.contains("/g/study/SKILL.md") && p.contains("/s/map/llms.md"));
        assert_eq!(
            title(Kind::Learn, "the transformer"),
            "Learn · the transformer"
        );
    }
}
