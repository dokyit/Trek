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

/// The question card for `data`, a `trek-study` block's question (JSON; a JavaScript object
/// literal's bare keys and trailing commas are let through too): a `trek-html` page. `None` when
/// it isn't a question (no `prompt`).
pub fn card(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data.trim())
        .or_else(|_| serde_json::from_str(&as_json(data.trim())))
        .ok()?;
    let o = value.as_object()?;
    let text = |k: &str| match o.get(k) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let list = |k: &str| -> Vec<String> {
        o.get(k)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let prompt = text("prompt");
    if prompt.trim().is_empty() {
        return None;
    }
    let kind = match text("kind").as_str() {
        k @ ("question" | "predict" | "check" | "problem") => k.to_string(),
        _ => "question".to_string(),
    };
    let options = if kind == "problem" {
        Vec::new()
    } else {
        list("options")
    };
    let data = serde_json::json!({
        "kind": kind,
        "step": text("step"),
        "prompt": prompt,
        "options": options,
        "targets": list("targets"),
    });
    // In a script: nothing in it may close the script element.
    let js = data.to_string().replace("</", "<\\/");
    // The page's own comments are for whoever reads its source here, not for every card a
    // transcript keeps: whole-line `//` comments are left out.
    let page: String = CARD
        .lines()
        .filter(|l| !l.trim_start().starts_with("// "))
        .flat_map(|l| [l, "\n"])
        .collect();
    Some(page.replacen("__DATA__", &js, 1))
}

/// A JavaScript object literal as JSON, as far as a question needs: bare keys quoted, trailing
/// commas dropped. Strings are copied as they are.
fn as_json(literal: &str) -> String {
    let chars: Vec<char> = literal.chars().collect();
    let mut out = String::with_capacity(literal.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            if next == Some(&':') {
                out.push('"');
                out.push_str(&word);
                out.push('"');
            } else {
                out.push_str(&word);
            }
            continue;
        }
        if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `text` (an answer, as it streams in or once written) with each finished `trek-study` block
/// drawn as the question card it asks for: a `trek-html` block holding `card`'s page. `None`
/// when there's none to draw (a block still being written stays as it is, as does one that isn't
/// a question).
pub fn expand_cards(text: &str) -> Option<String> {
    let open = format!("```{FENCE}");
    if !text.contains(&open) {
        return None;
    }
    let mut out = String::with_capacity(text.len() + CARD.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find(&open) {
        let line_start = (at == 0 && (out.is_empty() || out.ends_with('\n')))
            || (at > 0 && rest.as_bytes()[at - 1] == b'\n');
        let after = at + open.len();
        let Some(eol) = rest[after..].find('\n').map(|n| after + n) else {
            break;
        };
        if !line_start || !rest[after..eol].trim().is_empty() {
            out.push_str(&rest[..after]);
            rest = &rest[after..];
            continue;
        }
        // The closing fence: a line of just ```.
        let body = eol + 1;
        let mut close = None;
        let mut from = body;
        loop {
            let end = rest[from..].find('\n').map_or(rest.len(), |n| from + n);
            if rest[from..end].trim() == "```" {
                close = Some((from, end));
                break;
            }
            if end >= rest.len() {
                break;
            }
            from = end + 1;
        }
        let Some((close_start, close_end)) = close else {
            break;
        };
        let data = &rest[body..close_start];
        match card(data) {
            Some(html) => {
                out.push_str(&rest[..at]);
                out.push_str(&format!("```{}\n{html}\n```", crate::artifact::FENCE));
                changed = true;
            }
            None => out.push_str(&rest[..close_end]),
        }
        rest = &rest[close_end..];
    }
    out.push_str(rest);
    changed.then_some(out)
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
        assert!(card.len() < 16_000, "{} bytes", card.len());
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
        // Not a question: left as written.
        let odd = "```trek-study\n{\"kind\": \"question\"}\n```\nthen\n```trek-study\nnot\n```";
        assert_eq!(expand_cards(odd), None);
        // Named in a line, not a fence: left alone, and a block after it is still drawn.
        let mixed = format!("Use ```trek-study blocks.\n```trek-study\n{q}\n```");
        assert_eq!(
            expand_cards(&mixed),
            Some(format!(
                "Use ```trek-study blocks.\n```trek-html\n{html}\n```"
            ))
        );
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
