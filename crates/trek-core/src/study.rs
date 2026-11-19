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
    /// Its size in bytes.
    pub size: u64,
}

/// Sources listed before the list stops: the screen shows a few cards, not a file manager.
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
            let size = meta.len();
            out.push((at, Source { path, label, size }));
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
    add_sources_reporting(dir, topic, files).0
}

/// `add_sources`, and the files that exist but couldn't be copied in (a full disk, a read-only
/// folder, an unreadable file), for the learner to be told.
pub fn add_sources_reporting(
    dir: &Path,
    topic: &str,
    files: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let root = sources_dir(dir);
    let into = root.join(topic_slug(topic));
    let mut out: Vec<PathBuf> = vec![];
    let mut failed: Vec<PathBuf> = vec![];
    for file in files {
        if !file.is_file() {
            continue;
        }
        // Already in the study folder (a source, or an earlier map ticked to build on): used
        // where it is.
        if file.starts_with(dir) {
            if !out.contains(file) {
                out.push(file.clone());
            }
            continue;
        }
        let Some(name) = copy_name(file) else {
            failed.push(file.clone());
            continue;
        };
        let name = name.as_os_str();
        if std::fs::create_dir_all(&into).is_err() {
            failed.push(file.clone());
            continue;
        }
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        let mut dest = into.join(name);
        let mut n = 1;
        while dest.exists() && size(&dest) != size(file) {
            n += 1;
            let named = Path::new(name);
            let stem = named
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = named
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            dest = into.join(format!("{stem}-{n}{ext}"));
        }
        if dest.exists() || std::fs::copy(file, &dest).is_ok() {
            if !out.contains(&dest) {
                out.push(dest);
            }
        } else {
            failed.push(file.clone());
        }
    }
    // Committed as Trek's doing, so what the session changes is only the session's. Only what's
    // in sources/: a map ticked to build on keeps whatever an earlier session left uncommitted.
    let rel: Vec<String> = out
        .iter()
        .filter(|p| p.starts_with(&root))
        .filter_map(|p| p.strip_prefix(dir).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    if !rel.is_empty() {
        let rel: Vec<&str> = rel.iter().map(String::as_str).collect();
        commit(dir, &rel, &format!("Add sources: {}", topic_slug(topic)));
    }
    (out, failed)
}

/// What a learner's material is named as in `sources/`: its own name, or for a Trek note (a file
/// named by an id) its title, so the tutor and the learner both see what it is.
fn copy_name(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?;
    if file.parent() == Some(crate::notes::notes_dir().as_path()) {
        let body = std::fs::read_to_string(file).ok()?;
        let slug = crate::skills::slug(&crate::notes::title_of(&body));
        if !slug.is_empty() {
            return Some(PathBuf::from(format!("{slug}.md")));
        }
    }
    Some(PathBuf::from(name))
}

/// What a session starts with besides the learner's words: files (copied into, or already in,
/// the study folder), folders read where they are, and web pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Material {
    pub files: Vec<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub links: Vec<String>,
}

impl Material {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.folders.is_empty() && self.links.is_empty()
    }
}

const FILES_HEAD: &str = "Sources I've added for this session (read them before you start):";
const FOLDERS_HEAD: &str = "Folders to read where they are:";
const LINKS_HEAD: &str = "Pages to read:";

/// The first message of a session: which kind it is, the guide to follow, what the learner
/// asked for (`topic`, in their words), and the material they gave it (`with`: files in the study
/// folder, folders, pages). `map`: the topic's map, when the session continues one.
pub fn prompt(
    kind: Kind,
    guide: &Path,
    dir: &Path,
    topic: &str,
    map: Option<&Path>,
    with: &Material,
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
    let mut list = |head: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("\n\n{head}"));
            for i in items {
                out.push_str(&format!("\n- {i}"));
            }
        }
    };
    let shown = |paths: &[PathBuf]| paths.iter().map(|p| p.display().to_string()).collect();
    list(FILES_HEAD, shown(&with.files));
    list(FOLDERS_HEAD, shown(&with.folders));
    list(LINKS_HEAD, with.links.clone());
    out
}

/// A session's first message as the learner sees it in the thread: what they asked for and what
/// came with it, without the instructions to the tutor (`prompt` read back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intro {
    pub kind: Kind,
    /// What they asked, in their words, without the files and pages it names.
    pub topic: String,
    /// The map it carries on with.
    pub map: Option<PathBuf>,
    pub with: Material,
}

/// `text` read as a session's first message, if it is one.
pub fn intro(text: &str) -> Option<Intro> {
    let (kind, rest) = if let Some(r) = text.strip_prefix("Study session (learn).") {
        (Kind::Learn, r)
    } else if let Some(r) = text.strip_prefix("Study session (practice).") {
        (Kind::Practice, r)
    } else {
        return None;
    };
    // The guide's paragraph, then what was asked, up to the first list.
    let (_, body) = rest.split_once("\n\n")?;
    let first_list = [FILES_HEAD, FOLDERS_HEAD, LINKS_HEAD]
        .iter()
        .filter_map(|h| body.find(&format!("\n\n{h}")))
        .min()
        .unwrap_or(body.len());
    let (ask, lists) = body.split_at(first_list);
    let (map, asked) = if let Some(a) = ask
        .strip_prefix("What I want to learn: ")
        .or_else(|| ask.strip_prefix("What I want to practise: "))
    {
        (None, a)
    } else {
        let a = ask
            .strip_prefix("Continue the topic in ")
            .or_else(|| ask.strip_prefix("Practise the claims in "))?;
        let (map, asked) = a.split_once(".md: ")?;
        (Some(PathBuf::from(format!("{map}.md"))), asked)
    };
    let items = |head: &str| -> Vec<String> {
        let Some(at) = lists.find(&format!("\n\n{head}")) else {
            return vec![];
        };
        lists[at + 2..]
            .lines()
            .skip(1)
            .map_while(|l| l.strip_prefix("- "))
            .map(str::to_string)
            .collect()
    };
    Some(Intro {
        kind,
        topic: words_of(asked),
        map,
        with: Material {
            files: items(FILES_HEAD).into_iter().map(PathBuf::from).collect(),
            folders: items(FOLDERS_HEAD).into_iter().map(PathBuf::from).collect(),
            links: items(LINKS_HEAD),
        },
    })
}

/// The web pages `text` names (`http://` or `https://`), in order, each once.
pub fn links_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for word in text.split_whitespace() {
        let word = word
            .trim_start_matches(['<', '(', '"', '\''])
            .trim_end_matches(['>', ')', '"', '\'', '.', ',', ';', ':', '!', '?']);
        let is_link = ["https://", "http://"]
            .iter()
            .any(|s| word.len() > s.len() && word.starts_with(s));
        if is_link && !out.iter().any(|l| l == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// `text` without the files it names as `@path` or `@"a path"` or the pages it links, one space
/// between what's left: the topic, in the learner's words.
pub fn words_of(text: &str) -> String {
    let mut out: Vec<&str> = vec![];
    let mut quoted = false;
    for word in text.split_whitespace() {
        if quoted {
            quoted = !word.ends_with('"');
            continue;
        }
        if let Some(rest) = word.strip_prefix("@\"") {
            quoted = !rest.ends_with('"') || rest.len() < 2;
            continue;
        }
        if (word.starts_with('@') && word.len() > 1) || !links_in(word).is_empty() {
            continue;
        }
        out.push(word);
    }
    out.join(" ")
}

/// The fence a tutor asks a question in: ```` ```trek-study ```` around the question as JSON
/// (`kind`, `step`, `prompt`, `options`, `targets`), which Trek draws as the question card
/// (`card`). Writing only the question, not the card's page, keeps a tutor's turn short: the
/// page is ~10 KB, some 3,000 tokens to write out for every question.
pub const FENCE: &str = "trek-study";

/// The question card's page; `__DATA__` is where the question goes.
const CARD: &str = include_str!("study_card.html");

/// Whether a fence's info string (what follows the backticks) names a question block, as models
/// write it: `trek-study`, `Trek-Study`, `trek_study`, `trekstudy`, with anything after a space
/// (`trek-study json`).
pub fn is_question_fence(info: &str) -> bool {
    let word = info.split_whitespace().next().unwrap_or("");
    let word = word.trim_matches(|c: char| c == '{' || c == '}' || c == '.');
    let word = word.to_ascii_lowercase().replace('_', "-");
    word == FENCE || word == "trekstudy"
}

/// A question as the card draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// `question`, `predict`, `check` or `problem`.
    pub kind: String,
    pub step: String,
    pub prompt: String,
    pub options: Vec<String>,
    pub targets: Vec<String>,
}

impl Question {
    /// The question `data` asks: a `trek-study` block's JSON as a model writes it. Strict JSON, or
    /// what a model writes by habit or by slip: a JavaScript object literal (bare keys, single
    /// quotes, trailing commas, comments), curly quotes, LaTeX with its backslashes not doubled
    /// (`"$\frac{1}{2}$"`, which as JSON is a form feed and "rac"), a line break inside a string,
    /// a quote inside a string left bare, other names for the fields (`question`, `choices`, `type`),
    /// options as objects or lettered ("A. …"), the whole in a list. `None` when there's no prompt.
    pub fn read(data: &str) -> Option<Question> {
        let json = as_json(data.trim());
        let value: serde_json::Value = serde_json::from_str(&json)
            .or_else(|_| serde_json::from_str(&closed_json(&json)))
            .ok()?;
        Question::from_value(&value)
    }

    fn from_value(value: &serde_json::Value) -> Option<Question> {
        use serde_json::Value;
        // A list of questions: the first (one card a turn).
        let o = match value {
            Value::Array(a) => return a.iter().find_map(Question::from_value),
            Value::Object(o) => o,
            _ => return None,
        };
        // A question wrapped in a key of its own: `{"question": {...}}`.
        if o.len() == 1
            && let Some(inner @ Value::Object(_)) = o.values().next()
        {
            return Question::from_value(inner);
        }
        let get = |keys: &[&str]| -> Option<&Value> {
            keys.iter().find_map(|k| {
                o.iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(k))
                    .map(|(_, v)| v)
            })
        };
        let text = |v: Option<&Value>| match v {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Null) | None => String::new(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(other) => option_text(other),
        };
        let prompt = text(get(&["prompt", "question", "text", "q", "stem", "body"]));
        if prompt.is_empty() {
            return None;
        }
        let kind = text(get(&["kind", "type", "card"])).to_ascii_lowercase();
        let kind = match kind.replace(['_', ' '], "-").as_str() {
            "predict" | "prediction" => "predict",
            "check" | "check-question" => "check",
            "problem" | "practice" | "exercise" | "practice-problem" => "problem",
            _ => "question",
        };
        let options = match get(&["options", "choices", "answers", "alternatives"]) {
            Some(Value::Array(a)) => a.iter().map(option_text).collect(),
            Some(Value::Object(m)) => m.values().map(option_text).collect(),
            _ => vec![],
        };
        let options = if kind == "problem" {
            vec![]
        } else {
            tidy_options(options)
        };
        let targets = match get(&["targets", "claims", "target"]) {
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| text(Some(v)))
                .filter(|t| !t.is_empty())
                .collect(),
            Some(Value::String(s)) if !s.trim().is_empty() => s
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            _ => vec![],
        };
        Some(Question {
            kind: kind.to_string(),
            step: text(get(&["step", "progress", "where"])),
            prompt,
            options,
            targets,
        })
    }

    /// The question as readable Markdown: what's shown when Trek can't draw it as a card.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        if !self.step.is_empty() {
            out.push_str(&format!("*{}*\n\n", self.step));
        }
        out.push_str(&format!("**{}**", self.prompt));
        for (i, o) in self.options.iter().enumerate() {
            let key = char::from(b'A' + (i as u8 % 26));
            out.push_str(&format!("\n\n{key}. {o}"));
        }
        out
    }
}

/// An option's text: itself when a string, else its text field (`{"id": "A", "text": "…"}`).
fn option_text(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Object(o) => [
            "text", "label", "option", "content", "value", "answer", "choice",
        ]
        .iter()
        .find_map(|k| o.get(*k).and_then(Value::as_str))
        .or_else(|| o.values().filter_map(Value::as_str).max_by_key(|s| s.len()))
        .unwrap_or_default()
        .trim()
        .to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Options as the card shows them: the card letters them and offers "I don't know" itself, so
/// a letter a model put in front of each ("A. ", "(b) ", "C: ") and an "I don't know" of its own
/// are left out, as are empty ones.
fn tidy_options(options: Vec<String>) -> Vec<String> {
    let lettered = |i: usize, o: &str| -> Option<usize> {
        let mut c = o.chars();
        let mut first = c.next()?;
        let mut len = 0;
        if first == '(' {
            first = c.next()?;
            len += 1;
        }
        let want = char::from(b'A' + (i as u8 % 26));
        if !first.eq_ignore_ascii_case(&want) {
            return None;
        }
        len += 1;
        let sep = c.next()?;
        if !matches!(sep, '.' | ')' | ':') {
            return None;
        }
        len += 1;
        let rest = c.as_str();
        rest.starts_with(' ').then_some(len)
    };
    let all_lettered = options.len() > 1
        && options
            .iter()
            .enumerate()
            .all(|(i, o)| lettered(i, o).is_some());
    options
        .into_iter()
        .enumerate()
        .map(
            |(i, o)| match all_lettered.then(|| lettered(i, &o)).flatten() {
                Some(n) => o[n..].trim().to_string(),
                None => o,
            },
        )
        .filter(|o| {
            let plain = o
                .trim()
                .trim_end_matches(['.', '!'])
                .replace('’', "'")
                .to_ascii_lowercase();
            !o.is_empty() && plain != "i don't know" && plain != "i dont know"
        })
        .collect()
}

/// The question card for `data`, a `trek-study` block's question (`Question::read`): a
/// `trek-html` page. `None` when it isn't a question (no `prompt`).
pub fn card(data: &str) -> Option<String> {
    Question::read(data).map(|q| card_for(&q))
}

/// The card's page for `q`.
pub fn card_for(q: &Question) -> String {
    let data = serde_json::json!({
        "kind": q.kind,
        "step": q.step,
        "prompt": q.prompt,
        "options": q.options,
        "targets": q.targets,
    });
    // In a script: nothing in it may close the script element.
    let js = data.to_string().replace("</", "<\\/");
    // The page's own comments are for whoever reads its source here, not for every card a
    // transcript keeps: whole-line `//` comments are left out, and blank lines and each line's indent.
    let page: String = CARD
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.is_empty() && !l.starts_with("// "))
        .flat_map(|l| [l, "\n"])
        .collect();
    page.replacen("__DATA__", &js, 1)
}

/// LaTeX commands that start with a letter JSON reads as an escape (`\n`, `\t`, `\r`): written
/// outside `$…$` with one backslash they're still taken as LaTeX, not a line break or a tab.
const LATEX_NTR: &[&str] = &[
    "nabla",
    "neq",
    "ne",
    "nu",
    "not",
    "notin",
    "ni",
    "nmid",
    "newline",
    "neg",
    "nleq",
    "ngeq",
    "theta",
    "tau",
    "times",
    "to",
    "text",
    "textbf",
    "textit",
    "tfrac",
    "tan",
    "tanh",
    "top",
    "triangle",
    "tilde",
    "rho",
    "right",
    "rightarrow",
    "rangle",
    "rceil",
    "rfloor",
    "Rightarrow",
];

/// A question as a model writes it, as strict JSON (`Question::read`). Outside strings: bare keys
/// quoted, Python's `True`/`False`/`None` and trailing commas and comments seen to. Strings in
/// double, single or curly quotes come out as JSON strings: a backslash that isn't a JSON escape,
/// or that starts a LaTeX command (`\frac`, `\beta`, and inside maths `\nabla` or `\theta` too), is
/// kept as a backslash; a line break is escaped; a quote that doesn't end the string (one not
/// followed by `,` `:` `}` `]` or the end) is kept as a quote. Strict JSON comes out as it went in,
/// but for LaTeX that it would otherwise have read as control characters.
fn as_json(literal: &str) -> String {
    let chars: Vec<char> = literal.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(literal.len() + 16);
    // Whether the quote at `at` ends its string: what follows it is `,` `:` `}` `]` or nothing.
    let ends = |at: usize| -> bool {
        matches!(
            chars[at + 1..].iter().find(|c| !c.is_whitespace()),
            None | Some(',' | ':' | '}' | ']')
        )
    };
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’') {
            let closers: &[char] = match c {
                '"' => &['"'],
                '\'' => &['\''],
                '‘' | '’' => &['’', '‘', '\''],
                _ => &['”', '“', '"'],
            };
            i += 1;
            out.push('"');
            let mut math = false;
            while i < n {
                let c = chars[i];
                if closers.contains(&c) && ends(i) {
                    i += 1;
                    break;
                }
                match c {
                    '\\' if i + 1 < n => {
                        let next = chars[i + 1];
                        let word: String = chars[i + 1..]
                            .iter()
                            .take_while(|c| c.is_ascii_alphabetic())
                            .collect();
                        let latex = match next {
                            'b' | 'f' => word.len() > 1 || math,
                            'n' | 't' | 'r' => math || LATEX_NTR.contains(&word.as_str()),
                            _ => false,
                        };
                        if latex {
                            out.push_str("\\\\");
                            i += 1;
                            continue;
                        }
                        match next {
                            '\\' => out.push_str("\\\\"),
                            '"' => out.push_str("\\\""),
                            '/' => out.push('/'),
                            'b' | 'f' | 'n' | 'r' | 't' => {
                                out.push('\\');
                                out.push(next);
                            }
                            'u' if i + 5 < n
                                && chars[i + 2..i + 6].iter().all(|c| c.is_ascii_hexdigit()) =>
                            {
                                out.push('\\');
                                out.push('u');
                            }
                            '\'' | '’' | '”' | '“' => out.push(next),
                            _ => {
                                // Not a JSON escape: LaTeX (`\sqrt`, `\(`, `\,`), kept.
                                out.push_str("\\\\");
                                if next == '(' || next == '[' {
                                    math = true;
                                } else if next == ')' || next == ']' {
                                    math = false;
                                }
                                i += 1;
                                continue;
                            }
                        }
                        i += 2;
                        continue;
                    }
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '$' => {
                        if chars.get(i + 1) == Some(&'$') {
                            out.push('$');
                            i += 1;
                        }
                        math = !math;
                        out.push('$');
                    }
                    '\n' => out.push_str("\\n"),
                    '\r' => {}
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
                i += 1;
            }
            out.push('"');
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            if next == Some(&':') {
                out.push('"');
                out.push_str(&word);
                out.push('"');
            } else {
                out.push_str(match word.as_str() {
                    "True" => "true",
                    "False" => "false",
                    "None" | "undefined" => "null",
                    w => w,
                });
            }
            continue;
        }
        if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}' | ']') | None) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `json` (as `as_json` writes it) with whatever is left open closed: a question cut off part
/// way (a turn that ended inside it) reads as far as it got. A key left without its value, or a
/// trailing comma, is dropped.
fn closed_json(json: &str) -> String {
    let mut open: Vec<char> = vec![];
    let mut in_string = false;
    let mut escaped = false;
    for c in json.chars() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => open.push('}'),
            '[' => open.push(']'),
            '}' | ']' => {
                open.pop();
            }
            _ => {}
        }
    }
    let mut out = json.trim_end().to_string();
    if in_string {
        out.push('"');
    }
    loop {
        let t = out.trim_end();
        if let Some(t) = t.strip_suffix(',') {
            out = t.to_string();
        } else if t.ends_with(':') {
            out = format!("{t} null");
        } else {
            break;
        }
    }
    while let Some(c) = open.pop() {
        out.push(c);
    }
    out
}

/// The object literal that starts at the first `{` in `s`, through its matching `}` (strings
/// skipped over), or to the end when it isn't closed.
fn object_in(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        if let Some(q) = quote {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, c) if c == q => quote = None,
                (false, '\n') => quote = None,
                _ => {}
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    Some(&s[start..])
}

/// What the page a tutor wrote as a whole `trek-html` card asks (the way Study asked before Trek
/// drew the card itself, and what a tutor may still write in a thread that began then): the
/// `DATA` object of the question card's script. `None` for any other page.
fn question_in_page(html: &str) -> Option<Question> {
    // However it's spaced: a tutor may write the page out squeezed (Codex on GPT-5.6 Luna did).
    let at = html
        .match_indices("DATA")
        .map(|(at, _)| at)
        .find(|&at| html[at + 4..].trim_start().starts_with('='))?;
    let squeezed: String = html.chars().filter(|c| !c.is_whitespace()).collect();
    let is_card =
        squeezed.contains("question:\"Question\"") && squeezed.contains("problem:\"Problem\"");
    if !is_card {
        return None;
    }
    Question::read(object_in(&html[at..])?)
}

/// Fences a block as Markdown has them: three or more backticks (or tildes), indented by up to
/// three spaces. The fence and what follows it (the info string).
fn fence_of(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|c| *c == ch).count();
    (n >= 3).then(|| (&t[..n], t[n..].trim()))
}

/// `text` (an answer, as it streams in) with each finished question block drawn as the question
/// card it asks: a `trek-html` block holding `card`'s page. `None` when there's none to draw. A
/// question is a `trek-study` block however the fence is spelt (`is_question_fence`), a `json`
/// block holding a question, or a whole card page a tutor wrote out (`question_in_page`). A block
/// still being written stays as it is. One that can't be read as a question shows as what it
/// says, as text, not as code. One card a turn: a second question in the same answer shows as
/// text too, under the first.
pub fn expand_cards(text: &str) -> Option<String> {
    expand(text, false)
}

/// `expand_cards` for an answer that's finished: a question block it ended inside (the turn
/// stopped before the closing fence) is drawn as far as it got.
pub fn expand_cards_done(text: &str) -> Option<String> {
    expand(text, true)
}

fn expand(text: &str, done: bool) -> Option<String> {
    if !(text.contains("```") || text.contains("~~~")) {
        return None;
    }
    // Each line with where it starts.
    let mut lines: Vec<(usize, &str)> = vec![];
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        lines.push((at, line.trim_end_matches(['\n', '\r'])));
        at += line.len();
    }
    let mut out = String::with_capacity(text.len() + CARD.len());
    let mut copied = 0;
    let mut changed = false;
    let mut cards = 0;
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        let Some((fence, info)) = fence_of(line) else {
            i += 1;
            continue;
        };
        let ch = fence.chars().next().unwrap_or('`');
        let close = lines[i + 1..].iter().position(|(_, l)| {
            let t = l.trim();
            t.len() >= fence.len() && t.chars().all(|c| c == ch)
        });
        let lang = info
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let question_fence = is_question_fence(info);
        let maybe = question_fence
            || matches!(
                lang.as_str(),
                "json" | "json5" | "js" | "javascript" | "trek-html" | "html"
            );
        let (body_end, end, block_end) = match close {
            Some(k) => {
                let (cs, cl) = lines[i + 1 + k];
                (cs, cs + cl.len(), i + 2 + k)
            }
            // Still being written: wait for it (and for anything after it).
            None if !done || !maybe => break,
            None => (text.len(), text.len(), lines.len()),
        };
        let body_start = lines
            .get(i + 1)
            .map_or(text.len(), |(s, _)| *s)
            .min(body_end);
        let body = &text[body_start..body_end];
        let asked = if question_fence {
            Some(Question::read(body).ok_or(body))
        } else if matches!(lang.as_str(), "json" | "json5" | "js" | "javascript") {
            // Only what reads as a question card's fields: other JSON stays code.
            let looks = body.contains("prompt")
                && body.contains("kind")
                && (body.contains("options") || body.contains("targets"));
            looks.then(|| Question::read(body)).flatten().map(Ok)
        } else if matches!(lang.as_str(), "trek-html" | "html") {
            match question_in_page(body) {
                // Already the card Trek draws: counted, left as it is.
                Some(q) if card_for(&q).trim_end() == body.trim_end() => {
                    cards += 1;
                    None
                }
                q => q.map(Ok),
            }
        } else {
            None
        };
        if let Some(asked) = asked {
            out.push_str(&text[copied..start]);
            match asked {
                Ok(q) if cards == 0 => {
                    out.push_str(&format!(
                        "```{}\n{}\n```",
                        crate::artifact::FENCE,
                        card_for(&q)
                    ));
                    cards += 1;
                }
                Ok(q) => out.push_str(&q.markdown()),
                // Not a question Trek can read: what it says, as text.
                Err(body) => out.push_str(body.trim()),
            }
            copied = end;
            changed = true;
        }
        i = block_end;
    }
    if !changed {
        return None;
    }
    out.push_str(&text[copied..]);
    Some(out)
}

/// What a question card sent, read back for the thread to show: its first line (the card's
/// label, with where in the plan and the claims it tests: "Check · Step 3 of 8 · Scaled scores
/// [c4]") and its fields in order ("Answer", "Confidence", "Status", "Asking for", "Working"),
/// each value as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub head: String,
    pub fields: Vec<(String, String)>,
}

/// `text` if a question card sent it (`Submission`).
pub fn submission(text: &str) -> Option<Submission> {
    let text = text.trim_end();
    let (head, rest) = text.split_once('\n')?;
    let head = head.trim();
    let labelled = ["Question", "Predict", "Check", "Problem"]
        .iter()
        .any(|l| head == *l || head.starts_with(&format!("{l} ")));
    let working = if rest.starts_with("Working: ") {
        0
    } else {
        rest.find("\nWorking: ")? + 1
    };
    if !labelled {
        return None;
    }
    let mut fields: Vec<(String, String)> = vec![];
    for line in rest[..working].lines() {
        match line.split_once(": ") {
            Some((k, v)) if ["Answer", "Confidence", "Status", "Asking for"].contains(&k) => {
                fields.push((k.to_string(), v.to_string()))
            }
            _ => match fields.last_mut() {
                Some((_, v)) => {
                    v.push('\n');
                    v.push_str(line);
                }
                None => return None,
            },
        }
    }
    fields.push((
        "Working".to_string(),
        rest[working + "Working: ".len()..].to_string(),
    ));
    Some(Submission {
        head: head.to_string(),
        fields,
    })
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
            &Material::default(),
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
            &Material {
                files: vec![PathBuf::from("/s/sources/attention/notes.pdf")],
                ..Material::default()
            },
        );
        assert!(p.ends_with(
            "What I want to learn: attention\n\nSources I've added for this session (read them \
             before you start):\n- /s/sources/attention/notes.pdf"
        ));
    }

    #[test]
    fn the_first_message_reads_back_as_what_the_learner_asked_for() {
        let with = Material {
            files: vec![
                PathBuf::from("/s/sources/attention/notes.pdf"),
                PathBuf::from("/s/map/llms.md"),
            ],
            folders: vec![PathBuf::from("/Users/me/course")],
            links: vec!["https://arxiv.org/abs/1706.03762".into()],
        };
        let asked = "attention @/Users/me/notes.pdf from https://arxiv.org/abs/1706.03762, \
                     with: maths\n\nand pictures";
        let p = prompt(
            Kind::Learn,
            Path::new("/g/study/SKILL.md"),
            Path::new("/s"),
            asked,
            None,
            &with,
        );
        assert!(p.contains("\n\nFolders to read where they are:\n- /Users/me/course"));
        assert!(p.ends_with("\n\nPages to read:\n- https://arxiv.org/abs/1706.03762"));
        let i = intro(&p).expect("a session's first message");
        assert_eq!(i.kind, Kind::Learn);
        assert_eq!(i.topic, "attention from with: maths and pictures");
        assert_eq!((i.map, i.with), (None, with));
        // Carrying on with a map, nothing attached.
        let p = prompt(
            Kind::Practice,
            Path::new("/g/study/SKILL.md"),
            Path::new("/s"),
            "Gradient descent",
            Some(Path::new("/s/map/gradient-descent.md")),
            &Material::default(),
        );
        let i = intro(&p).unwrap();
        assert_eq!(
            (i.kind, i.topic.as_str(), i.map),
            (
                Kind::Practice,
                "Gradient descent",
                Some(PathBuf::from("/s/map/gradient-descent.md"))
            )
        );
        assert!(i.with.is_empty());
        assert_eq!(intro("Study the code in src/"), None);
        assert_eq!(intro("Study session (learn). no paragraphs"), None);
    }

    #[test]
    fn a_question_is_drawn_as_the_card_that_sends_what_the_tutor_reads() {
        let data = r#"{"kind": "check", "step": "Step 2 of 5 · Scaling", "prompt": "Why $\\sqrt{d}$? </script>", "options": [], "targets": ["c4"]}"#;
        let card = card(data).expect("a card");
        assert!(card.starts_with("<!doctype html>") && card.trim_end().ends_with("</html>"));
        assert_eq!(card.matches("const DATA = ").count(), 1);
        assert!(!card.contains("__DATA__"));
        // The question as the page reads it, and nothing in it ends the script.
        // (The order of the keys is serde_json's, which differs with the features a build turns
        // on: each one is looked for by itself.)
        let line = &card[card.find("const DATA = ").unwrap()..];
        let line = &line[..line.find('\n').unwrap()];
        assert!(line.trim_end().ends_with("};"), "{line}");
        for part in [
            r#""kind":"check""#,
            r#""options":[]"#,
            r#""prompt":"Why $\\sqrt{d}$? <\/script>""#,
            r#""step":"Step 2 of 5 · Scaling""#,
            r#""targets":["c4"]"#,
        ] {
            assert!(line.contains(part), "{part} in {line}");
        }
        // It answers through Trek, says where the learner is, takes the keyboard, keeps still when
        // the system asks for less motion, takes dictation through Trek, and shows an answer sent
        // before (the card drawn again) as it was sent.
        for needed in [
            "T.send(text)",
            "(\\d+)\\s*of\\s*(\\d+)",
            "addEventListener(\"keydown\"",
            "prefers-reduced-motion",
            "Confidence: ",
            "Working: ",
            "T.dictate(",
            "\"trekdictation\"",
            "T.replies",
        ] {
            assert!(card.contains(needed), "{needed}");
        }
        // Inside Trek's own card: no box drawn around it again.
        assert!(!card.contains(".card{border"));
        assert!(card.len() < 17_000, "{} bytes", card.len());
        // What a model writes by habit (bare keys, trailing commas) still makes a card; a problem
        // has no options; what isn't a question makes none.
        let loose = r#"{ kind: "problem", prompt: "Say \"why\": 2, 0, -1", options: ["x"], targets: ["c2",], }"#;
        let card = super::card(loose).expect("a card");
        let line = &card[card.find("const DATA = ").unwrap()..];
        let line = &line[..line.find('\n').unwrap()];
        for part in [
            r#""kind":"problem""#,
            r#""options":[]"#,
            r#""prompt":"Say \"why\": 2, 0, -1""#,
            r#""step":"""#,
            r#""targets":["c2"]"#,
        ] {
            assert!(line.contains(part), "{part} in {line}");
        }
        assert_eq!(super::card(r#"{"kind": "question"}"#), None);
        assert_eq!(super::card("not json"), None);
    }

    /// The card's own script, in node with a stand-in page (`study_card_test.js`): an answer sent
    /// before shows as sent, dictation fills the thinking box, answering keeps it all.
    #[test]
    fn the_card_keeps_its_answer_and_takes_dictation() {
        let dir = env!("CARGO_MANIFEST_DIR");
        let out = match std::process::Command::new("node")
            .arg(format!("{dir}/src/study_card_test.js"))
            .arg(format!("{dir}/src/study_card.html"))
            .output()
        {
            Ok(out) => out,
            Err(_) => return, // no node here (CI on Windows): the text checks above still run
        };
        assert!(
            out.status.success() && out.stdout == b"ok",
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_finished_question_block_becomes_its_card_and_one_being_written_waits() {
        let q = r#"{"kind": "question", "prompt": "Which?", "options": ["A", "B"]}"#;
        let html = card(q).unwrap();
        let text = format!("Next one.\n\n```trek-study\n{q}\n```\n\nTake your time.");
        assert_eq!(
            expand_cards(&text).as_deref(),
            Some(format!("Next one.\n\n```trek-html\n{html}\n```\n\nTake your time.").as_str())
        );
        // Drawn already, or nothing to draw: unchanged.
        assert_eq!(expand_cards(&expand_cards(&text).unwrap()), None);
        assert_eq!(expand_cards("No card here."), None);
        // Still being written: left for later.
        assert_eq!(expand_cards(&format!("Next.\n\n```trek-study\n{q}")), None);
        assert_eq!(expand_cards("Next.\n\n```trek-study"), None);
        // At the very start, closed by the answer's end.
        assert_eq!(
            expand_cards(&format!("```trek-study\n{q}\n```")),
            Some(format!("```trek-html\n{html}\n```"))
        );
        // Not a question: what it says, as text rather than code.
        let odd = "```trek-study\n{\"kind\": \"question\"}\n```\nthen\n```trek-study\nnot\n```";
        assert_eq!(
            expand_cards(odd).as_deref(),
            Some("{\"kind\": \"question\"}\nthen\nnot")
        );
        // Named in a line, not a fence: left alone, and a block after it is still drawn.
        let mixed = format!("Use ```trek-study blocks.\n```trek-study\n{q}\n```");
        assert_eq!(
            expand_cards(&mixed),
            Some(format!(
                "Use ```trek-study blocks.\n```trek-html\n{html}\n```"
            ))
        );
    }

    /// The question in a drawn card's page, as the page reads it.
    fn drawn(page: &str) -> Question {
        let line = &page[page.find("const DATA = ").expect("a card") + 13..];
        let line = &line[..line.find('\n').unwrap()];
        let v: serde_json::Value =
            serde_json::from_str(&line.trim_end_matches(';').replace("<\\/", "</")).unwrap();
        Question::from_value(&v).unwrap()
    }

    /// Every card drawn in `text`, as its question.
    fn cards_drawn(text: &str) -> Vec<Question> {
        text.split("```trek-html\n").skip(1).map(drawn).collect()
    }

    fn q(data: &str) -> Question {
        Question::read(data).unwrap_or_else(|| panic!("a question: {data}"))
    }

    /// What real tutors wrote (Claude Code on Haiku 4.5, Sonnet 5.5 and Opus 5.5, 17 November):
    /// each is drawn with its maths and Markdown as written.
    #[test]
    fn real_tutors_questions_are_drawn_as_written() {
        let haiku = r#"{"kind": "question", "step": "", "prompt": "If you have a function $f(x) = x^2 - 4x + 5$, what does it mean to find the **minimum** of this function?", "options": ["The smallest value that $f(x)$ can take (and the $x$ where it occurs)", "The point where the function crosses the x-axis", "The point where the function is steepest", "The value of $x$ that makes $f(x) = 0$"], "targets": []}"#;
        let opus = r#"{"kind": "question", "step": "", "prompt": "What is $\\dfrac{d}{dx}\\,\\sin(x^2)$?", "options": ["$\\cos(x^2)$", "$2x\\cos(x^2)$", "$\\cos(2x)$", "$2x\\cos(x)$"], "targets": []}"#;
        let sonnet = r#"{"kind": "question", "step": "", "prompt": "What is the derivative of $h(x) = (3x+1)^2$ with respect to $x$? Pick whichever you think is right, or work it out another way if you prefer, and write your method in the working box.", "options": ["$2(3x+1)$", "$6(3x+1)$", "$2x \\cdot 3$", "$(3x+1)^2 \\cdot 3$"], "targets": []}"#;
        let h = q(haiku);
        assert!(h.prompt.contains("**minimum**") && h.options.len() == 4);
        let o = q(opus);
        assert_eq!(o.prompt, r"What is $\dfrac{d}{dx}\,\sin(x^2)$?");
        assert_eq!(o.options[1], r"$2x\cos(x^2)$");
        assert_eq!(q(sonnet).options[2], r"$2x \cdot 3$");
        for data in [haiku, opus, sonnet] {
            let text = format!("Starting broad:\n\n```trek-study\n{data}\n```");
            let drawn = cards_drawn(&expand_cards(&text).unwrap());
            assert_eq!(drawn, vec![q(data)]);
        }
    }

    /// LaTeX written with single backslashes, as a model often does in a JSON string: `\f`, `\b`,
    /// `\n`, `\t` and `\r` start LaTeX commands, not control characters; a line break outside
    /// maths is still one.
    #[test]
    fn latex_with_single_backslashes_keeps_its_commands() {
        let data = r#"{"kind": "check", "prompt": "Take $\frac{1}{2}\beta$ and $\nabla f \cdot \theta \times \rho$, then \left(\sqrt{x}\right). \nWhich is right?", "options": ["$\to 0$", "$\neq 1$", "$\textbf{x}$", "\(\tfrac{a}{b}\)"]}"#;
        let got = q(data);
        assert_eq!(
            got.prompt,
            "Take $\\frac{1}{2}\\beta$ and $\\nabla f \\cdot \\theta \\times \\rho$, then \\left(\\sqrt{x}\\right). \nWhich is right?"
        );
        assert_eq!(
            got.options,
            [
                "$\\to 0$",
                "$\\neq 1$",
                "$\\textbf{x}$",
                "\\(\\tfrac{a}{b}\\)"
            ]
        );
        // Strict JSON with doubled backslashes reads the same.
        let strict = r#"{"kind": "check", "prompt": "$\\frac{1}{2}\\beta$\nnext", "options": []}"#;
        assert_eq!(q(strict).prompt, "$\\frac{1}{2}\\beta$\nnext");
    }

    /// What models write instead of strict JSON still makes the question it means.
    #[test]
    fn loose_questions_are_read_as_meant() {
        // Single quotes with an apostrophe inside, a trailing comma, a comment, Python's True.
        let py = "{'kind': 'predict', 'prompt': \"What's next?\", 'options': ['It's 2', 'It's 3',], // x\n 'hard': True}";
        let p = q(py);
        assert_eq!(
            (p.kind.as_str(), p.prompt.as_str()),
            ("predict", "What's next?")
        );
        assert_eq!(p.options, ["It's 2", "It's 3"]);
        // Curly quotes, as a word processor (or a model imitating one) writes them.
        let curly = "{“kind”: “question”, “prompt”: “Which is “steepest”?”, “options”: [“a”, “b”]}";
        let c = q(curly);
        assert_eq!(c.prompt, "Which is “steepest”?");
        assert_eq!(c.options, ["a", "b"]);
        // A quote left bare inside a string, and a line break written as one.
        let bare = "{\"kind\": \"question\", \"prompt\": \"What does \"minimum\" mean\nhere?\", \"options\": [\"x\"]}";
        assert_eq!(q(bare).prompt, "What does \"minimum\" mean\nhere?");
        // Other names for the fields, options as objects and lettered, its own "I don't know".
        let named = r#"{"type": "Multiple_Choice", "question": "Pick", "choices": [{"id": "A", "text": "A. one"}, {"label": "B) two"}, {"text": "C. three"}, "D. I don't know"], "claims": "c1, c2", "step": 3}"#;
        let n = q(named);
        assert_eq!(n.kind, "question");
        assert_eq!(n.prompt, "Pick");
        assert_eq!(n.options, ["one", "two", "three"]);
        assert_eq!(n.targets, ["c1", "c2"]);
        assert_eq!(n.step, "3");
        // Options keyed by letter.
        let keyed =
            r#"{"kind": "question", "prompt": "Pick", "options": {"A": "one", "B": "two"}}"#;
        assert_eq!(q(keyed).options, ["one", "two"]);
        // Options that only start with a letter aren't stripped.
        let words = r#"{"prompt": "Pick", "options": ["A. one", "Because"]}"#;
        assert_eq!(q(words).options, ["A. one", "Because"]);
        // Wrapped, or a list: the (first) question.
        assert_eq!(
            q(r#"{"question": {"prompt": "In", "kind": "check"}}"#).kind,
            "check"
        );
        assert_eq!(q(r#"[{"prompt": "One"}, {"prompt": "Two"}]"#).prompt, "One");
        // A problem has no options; a kind spelt otherwise is the one meant.
        let problem =
            r#"{"kind": "Practice", "prompt": "Differentiate $\sin(x^2)$", "options": ["x"]}"#;
        let p = q(problem);
        assert_eq!((p.kind.as_str(), p.options.len()), ("problem", 0));
        assert_eq!(p.prompt, "Differentiate $\\sin(x^2)$");
        // Cut off part way: as far as it got.
        let cut = r#"{"kind": "question", "prompt": "Which rule?", "options": ["Chain", "Prod"#;
        assert_eq!(q(cut).options, ["Chain", "Prod"]);
        assert_eq!(Question::read("no question here"), None);
        assert_eq!(Question::read(r#"{"options": ["a"]}"#), None);
    }

    /// The block however its fence is spelt; a `json` block that holds a question; text around
    /// it kept; a block the turn ended inside drawn once the answer is done.
    #[test]
    fn question_blocks_are_found_however_fenced() {
        let data = r#"{"kind": "question", "prompt": "Which?", "options": ["A", "B"]}"#;
        let html = card(data).unwrap();
        let want = format!("Lead in.\n\n```trek-html\n{html}\n```\n\nAfter.");
        for open in [
            "```trek-study",
            "```Trek-Study",
            "```trek_study",
            "```trek-study json",
            "````trek-study",
            "  ```trek-study",
            "```json",
        ] {
            let close = if open.contains("````") { "````" } else { "```" };
            let text = format!("Lead in.\n\n{open}\n{data}\n{close}\n\nAfter.");
            assert_eq!(expand_cards(&text), Some(want.clone()), "{open}");
        }
        let tilde = format!("Lead in.\n\n~~~trek-study\n{data}\n~~~\n\nAfter.");
        assert_eq!(expand_cards(&tilde), Some(want.clone()));
        // Other JSON, and other code, stays code.
        let other = "```json\n{\"prompt\": \"system\", \"options\": {\"temperature\": 0}}\n```";
        assert_eq!(expand_cards(other), None);
        let code = format!("```text\n```trek-study\n{data}\n```\n");
        assert_eq!(expand_cards(&code), None);
        // The turn ended before the closing fence: waiting while it streams, drawn when done.
        let open = format!("Here:\n\n```trek-study\n{data}");
        assert_eq!(expand_cards(&open), None);
        assert_eq!(
            expand_cards_done(&open),
            Some(format!("Here:\n\n```trek-html\n{html}\n```"))
        );
        let cut = "Here:\n\n```trek-study\n{\"kind\": \"question\", \"prompt\": \"Which?\", \"options\": [\"A\", \"B";
        assert_eq!(cards_drawn(&expand_cards_done(cut).unwrap()), vec![q(data)]);
    }

    /// One card a turn: a second question in the same answer reads as text under the first.
    #[test]
    fn a_second_question_in_one_answer_is_text_not_a_second_card() {
        let one = r#"{"kind": "question", "prompt": "First?", "options": ["a", "b"]}"#;
        let two = r#"{"kind": "question", "step": "Step 1 of 3", "prompt": "Second?", "options": ["c", "d"]}"#;
        let text = format!("Q1\n```trek-study\n{one}\n```\nQ2\n```trek-study\n{two}\n```");
        let out = expand_cards(&text).unwrap();
        assert_eq!(cards_drawn(&out), vec![q(one)]);
        assert!(
            out.ends_with("Q2\n*Step 1 of 3*\n\n**Second?**\n\nA. c\n\nB. d"),
            "{out}"
        );
        // Drawn already (the answer as it streams): still one card.
        assert_eq!(expand_cards(&out), None);
    }

    /// The card a tutor wrote out whole, as Study asked before Trek drew it (0.4.55 and earlier),
    /// and as a tutor may still write in a thread begun then: drawn again from its question, so
    /// a page copied wrong, cut off, or with its maths' backslashes not doubled still works.
    #[test]
    fn a_card_page_a_tutor_wrote_out_is_drawn_from_its_question() {
        // A real one (Opus 5.5, 29 October), its question in a JavaScript object.
        let page = include_str!("study_fixtures/old-card-opus-2026-10-29.html");
        let text = format!("First question.\n\n```trek-html\n{page}```\n");
        let out = expand_cards(&text).unwrap();
        let got = cards_drawn(&out);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].prompt,
            "Using the code above: what value is bound to r?"
        );
        assert_eq!(got[0].options, ["2", "3", "4", "7"]);
        assert!(out.starts_with("First question.\n\n```trek-html\n<!doctype html>"));
        assert!(out.contains("T.replies"), "the card of today");
        // Cut off after its question, with maths written with single backslashes (in a script a
        // string's `\f` is a form feed: the old card showed "rac{1}{2}").
        let cut = "```trek-html\n<!doctype html><html><head><style>.q{}</style></head><body>\n<script>\nconst DATA = {\n  kind: \"check\",\n  step: \"Step 2 of 5 · Scaling\",\n  prompt: \"Why $\\frac{1}{\\sqrt{d}}$?\",\n  options: [],\n  targets: [\"c4\"]\n};\nconst LABEL = {question: \"Question\", predict: \"Predict\", check: \"Check\", problem: \"Problem\"}[DATA.kind] || \"Question\";\nconst problem = DATA.kind";
        let out = expand_cards_done(cut).unwrap();
        let got = cards_drawn(&out);
        assert_eq!(got[0].prompt, "Why $\\frac{1}{\\sqrt{d}}$?");
        assert_eq!(
            (got[0].kind.as_str(), got[0].targets.clone()),
            ("check", vec!["c4".to_string()])
        );
        // Still being written: left until it's done.
        assert_eq!(expand_cards(cut), None);
        // Written out squeezed and rewritten (Codex on GPT-5.6 Luna under the 0.4.55 guide, 17
        // November: its "I don't know" sent "Answer: A. undefined"), drawn as today's card.
        let page = include_str!("study_fixtures/old-card-codex-squeezed-2026-11-17.html");
        let out = expand_cards(&format!("```trek-html\n{page}```")).unwrap();
        let got = cards_drawn(&out);
        assert_eq!(
            got[0].prompt,
            "For $h(x)=(3x+1)^2$, which correctly identifies the inner and outer functions?"
        );
        assert_eq!(got[0].options.len(), 4);
        // Any other page is left as it is.
        let other = "```trek-html\n<!doctype html><script>const DATA = {a: 1};</script>\n```";
        assert_eq!(expand_cards(other), None);
    }

    #[test]
    fn what_a_card_sent_is_read_back_field_by_field() {
        let s = submission(
            "Check · Step 3 of 8 · Scaled scores [c4]\nAnswer: C. Population growth $\\bar n$, scaled by $\\gamma$\nConfidence: Unsure (45%)\nWorking: first line\nsecond: line",
        )
        .expect("a submission");
        assert_eq!(s.head, "Check · Step 3 of 8 · Scaled scores [c4]");
        let f: Vec<(&str, &str)> = s
            .fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            f,
            [
                (
                    "Answer",
                    "C. Population growth $\\bar n$, scaled by $\\gamma$"
                ),
                ("Confidence", "Unsure (45%)"),
                ("Working", "first line\nsecond: line"),
            ]
        );
        let p = submission("Problem [c2]\nAsking for: a hint\nWorking: (none)").unwrap();
        assert_eq!(
            p.fields[0],
            ("Asking for".to_string(), "a hint".to_string())
        );
        // Not from a card: typed, or a label with no working.
        assert_eq!(submission("Question: which plan?\nWorking: x"), None);
        assert_eq!(submission("Question\nAnswer: A. yes"), None);
        assert_eq!(submission("Hello\nWorking: x"), None);
    }

    #[test]
    fn the_topic_is_the_text_without_the_files_and_pages_it_names() {
        assert_eq!(
            words_of("attention, from @/tmp/notes.pdf and @\"/tmp/my sheet.md\" please"),
            "attention, from and please"
        );
        assert_eq!(words_of("@/tmp/notes.pdf"), "");
        assert_eq!(words_of("mail me@home"), "mail me@home");
        assert_eq!(words_of("see (https://a.b/c). ok"), "see ok");
        assert_eq!(
            links_in("see (https://a.b/c). and http://x.y, https://a.b/c and https://"),
            vec!["https://a.b/c".to_string(), "http://x.y".to_string()]
        );
    }

    #[test]
    fn a_file_that_cannot_be_copied_in_is_reported_not_dropped_quietly() {
        let dir = std::env::temp_dir().join(format!("trek-study-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // `sources` is a plain file, so nothing can be made under it.
        std::fs::write(dir.join("sources"), "in the way").unwrap();
        let outside = std::env::temp_dir().join(format!("trek-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        let paper = outside.join("paper.pdf");
        std::fs::write(&paper, "pdf").unwrap();
        let missing = outside.join("never-there.pdf");
        let (kept, failed) = add_sources_reporting(&dir, "attention", &[paper.clone(), missing]);
        assert!(kept.is_empty());
        assert_eq!(
            failed,
            vec![paper],
            "a file that isn't there isn't a failure"
        );
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn a_map_ticked_stays_put_and_a_note_is_copied_under_its_title() {
        let dir = std::env::temp_dir().join(format!("trek-study-{}", uuid::Uuid::new_v4()));
        ensure(&dir).unwrap();
        let map = map_dir(&dir).join("llms.md");
        std::fs::write(&map, "# LLMs\n").unwrap();
        let notes = crate::notes::notes_dir();
        std::fs::create_dir_all(&notes).unwrap();
        let note = notes.join(format!("{}.md", uuid::Uuid::new_v4().simple()));
        std::fs::write(&note, "# Lecture 4: attention\n\nQ, K, V").unwrap();
        let got = add_sources(&dir, "attention", &[map.clone(), note.clone()]);
        let copy = sources_dir(&dir)
            .join("attention")
            .join("lecture-4-attention.md");
        assert_eq!(got, vec![map.clone(), copy.clone()]);
        assert!(copy.is_file());
        // The map an earlier session changed isn't committed by Trek: only the copy is.
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "?? map/\n");
        let _ = std::fs::remove_file(note);
        let _ = std::fs::remove_dir_all(dir);
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
