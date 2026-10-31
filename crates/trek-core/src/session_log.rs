//! Every thread's transcript as a plain-text file of its own (`<data>/transcripts/<thread>.log`),
//! appended to as its items settle. A session that starts from a short orientation (a recap, a
//! handoff's brief, a Direct session's fresh context) gets the exact earlier content back by
//! searching it (`transcript_search`, `transcript_read`, or the agent's own grep) instead of
//! relying on a summary that dropped the errors, paths and numbers it needs.
//!
//! Each entry starts with one header line, then the item's text as it is:
//!
//! ```text
//! ### #12 L340 turn 3 2026-10-31 14:02:11 tool Bash (done)
//! args: cargo test -p trek-core
//! result:
//! …every line of the output…
//! ```
//!
//! `#12` is the entry, `L340` the line the header is on. The log is only ever appended to: an
//! item that changes after it was written is written again (`(revises #12)`), and entries a
//! rewind takes back get a note saying which. A sidecar (`<thread>.idx`) records which items are
//! in the log, so a relaunch carries on where it was. Nothing is cut silently: a result past
//! [`MAX_BODY`] is cut with a line saying so.

use crate::store::{Item, ToolStatus};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// What every header line starts with.
pub const HEADER: &str = "### #";
/// The longest an entry's text runs in the log; past it, the rest is left out and the entry says so.
pub const MAX_BODY: usize = 1 << 20;

/// The sentence in Trek's notes to every agent.
pub const AGENT_NOTE: &str = "After a compaction, a recap or a handoff, the exact earlier content (errors, paths, numbers, code) is in this thread's transcript: search it with transcript_search and transcript_read rather than guess.";

/// Rows of Trek's own transcript tools, whose results (lines of the log itself) aren't written
/// into the log again. Direct names them by these titles; agents by the tool's name.
pub const SEARCH_TITLE: &str = "Search the transcript";
pub const READ_TITLE: &str = "Read the transcript";

pub fn is_own_tool(title: &str) -> bool {
    title == SEARCH_TITLE
        || title == READ_TITLE
        || crate::orchestrate::is_ours(title, "transcript_search")
        || crate::orchestrate::is_ours(title, "transcript_read")
}

/// The retrieval instruction an orientation ends with: where the transcript is and how to use it.
/// `thread`: the thread it belongs to, when that isn't the session's own (a handoff's).
pub fn retrieval(path: &Path, lines: u64, thread: Option<&str>) -> String {
    let path = path.display();
    let (whose, search, read) = match thread {
        Some(id) => (
            "That thread's whole conversation is in its transcript",
            format!("transcript_search with thread \"{id}\""),
            "transcript_read with the same thread",
        ),
        None => (
            "The whole conversation so far is in the transcript",
            "transcript_search".to_string(),
            "transcript_read",
        ),
    };
    format!(
        "{whose} at \"{path}\" ({lines} lines), word for word. This is only an orientation: when you need an exact error, path, number, command or piece of code from earlier, search it with {search} (or read lines with {read}) rather than guess."
    )
}

/// The logs of one data folder: each thread's file, and what's in it.
#[derive(Debug)]
pub struct Logs {
    dir: PathBuf,
    open: HashMap<String, Log>,
}

/// What a thread's log holds so far.
#[derive(Debug, Default)]
struct Log {
    /// Items written, by id: their entry and a hash of what was written.
    written: HashMap<String, (u32, u64)>,
    entries: u32,
    lines: u64,
    /// When items not written yet were first seen this run (ms): their entries' time.
    fresh: HashMap<String, i64>,
}

/// One row of a transcript as `Logs::record` takes it.
pub type Row<'a> = (&'a str, &'a Item);

/// What changed in a transcript since the last record (`transcript::Changes`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Delta<'a> {
    pub appended: &'a [&'a str],
    pub changed: &'a [&'a str],
    pub removed: &'a [String],
}

impl Logs {
    pub fn new(dir: PathBuf) -> Self {
        Logs {
            dir,
            open: HashMap::new(),
        }
    }

    pub fn path(&self, thread: &str) -> PathBuf {
        self.dir.join(format!("{}.log", file_stem(thread)))
    }

    fn index_path(&self, thread: &str) -> PathBuf {
        self.dir.join(format!("{}.idx", file_stem(thread)))
    }

    /// How many lines `thread`'s log has (0 when there's none yet).
    pub fn lines(&mut self, thread: &str) -> u64 {
        self.load(thread).map(|l| l.lines).unwrap_or(0)
    }

    /// The thread is gone: so is its log.
    pub fn forget(&mut self, thread: &str) {
        self.open.remove(thread);
        let _ = std::fs::remove_file(self.path(thread));
        let _ = std::fs::remove_file(self.index_path(thread));
    }

    fn load(&mut self, thread: &str) -> std::io::Result<&mut Log> {
        if !self.open.contains_key(thread) {
            let log = self.read_state(thread)?;
            self.open.insert(thread.to_string(), log);
        }
        Ok(self.open.get_mut(thread).expect("just loaded"))
    }

    /// The state of a log on disk: the sidecar's ids and the log's line count. A log without its
    /// sidecar (or the other way round) starts over, so nothing in it is unaccounted for.
    fn read_state(&self, thread: &str) -> std::io::Result<Log> {
        let (log, index) = (self.path(thread), self.index_path(thread));
        let mut state = Log::default();
        match (std::fs::read(&log), std::fs::read_to_string(&index)) {
            (Ok(bytes), Ok(idx)) => {
                state.lines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
                for line in idx.lines() {
                    if let Some(id) = line.strip_prefix("- ") {
                        state.written.remove(id);
                        continue;
                    }
                    let mut parts = line.splitn(3, ' ');
                    let (Some(n), Some(hash), Some(id)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    if let (Ok(n), Ok(hash)) = (n.parse::<u32>(), u64::from_str_radix(hash, 16)) {
                        state.entries = state.entries.max(n);
                        state.written.insert(id.to_string(), (n, hash));
                    }
                }
            }
            (Err(_), Err(_)) => {}
            _ => {
                let _ = std::fs::remove_file(&log);
                let _ = std::fs::remove_file(&index);
            }
        }
        Ok(state)
    }

    /// Bring `thread`'s log up to date with its transcript `rows`: what `delta` says was taken
    /// back gets a note, items written before that changed are written again, and every settled
    /// item not written yet is appended, in transcript order. `flush`: nothing is in progress,
    /// so the last message and running tools count as settled too (else they wait until they're
    /// done). Returns the log's path.
    pub fn record(
        &mut self,
        thread: &str,
        rows: &[Row<'_>],
        delta: Delta<'_>,
        flush: bool,
    ) -> std::io::Result<PathBuf> {
        let path = self.path(thread);
        let index = self.index_path(thread);
        let first_look = !self.open.contains_key(thread);
        let log = self.load(thread)?;
        let now = crate::store::now_ms();
        for id in delta.appended {
            log.fresh.entry(id.to_string()).or_insert(now);
        }

        let mut text = String::new();
        let mut idx = String::new();
        // Where each row stands: its turn and the time it's known by.
        let mut turn = 0u32;
        let mut known_at: Option<i64> = None;
        let mut placed: Vec<(u32, Option<i64>)> = Vec::with_capacity(rows.len());
        for (_, item) in rows {
            match item {
                Item::User { at, aside, .. } => {
                    if !aside {
                        turn += 1;
                    }
                    known_at = at.or(known_at);
                }
                Item::TurnEnd { at, .. } => known_at = Some(*at),
                _ => {}
            }
            placed.push((turn, known_at));
        }

        // Taken back: what `delta` removed, or on a first look, written items no longer there.
        let gone: Vec<String> = if first_look {
            let present: HashSet<&str> = rows.iter().map(|(id, _)| *id).collect();
            log.written
                .keys()
                .filter(|id| !present.contains(id.as_str()))
                .cloned()
                .collect()
        } else {
            delta
                .removed
                .iter()
                .filter(|id| log.written.contains_key(id.as_str()))
                .cloned()
                .collect()
        };
        for id in delta.removed {
            log.fresh.remove(id);
        }
        if !gone.is_empty() {
            let mut numbers: Vec<u32> = gone
                .iter()
                .filter_map(|id| log.written.remove(id).map(|(n, _)| n))
                .collect();
            numbers.sort_unstable();
            for id in &gone {
                let _ = writeln!(idx, "- {id}");
            }
            log.entries += 1;
            let body = format!(
                "Entries {} were taken back (a rewind, an edit or a retry): what follows replaces them.",
                ranges(&numbers)
            );
            let entry = format_entry(
                log.entries,
                log.lines + 1,
                turn,
                Some(now),
                "taken back",
                &body,
            );
            log.lines += newlines(&entry);
            text.push_str(&entry);
        }

        // Written before and changed since: written again.
        let changed: HashSet<&str> = delta.changed.iter().copied().collect();
        for (i, (id, item)) in rows.iter().enumerate() {
            if !changed.contains(id) || !settled(rows, i, flush) {
                continue;
            }
            let Some(&(before, old)) = log.written.get(*id) else {
                continue;
            };
            let hash = hash_item(item);
            if hash == old {
                continue;
            }
            let Some((kind, body)) = describe(item) else {
                continue;
            };
            log.entries += 1;
            let entry = format_entry(
                log.entries,
                log.lines + 1,
                placed[i].0,
                Some(now),
                &format!("{kind} (revises #{before})"),
                &body,
            );
            log.lines += newlines(&entry);
            text.push_str(&entry);
            log.written.insert(id.to_string(), (log.entries, hash));
            let _ = writeln!(idx, "{} {hash:016x} {id}", log.entries);
        }

        // New, settled items.
        for (i, (id, item)) in rows.iter().enumerate() {
            if log.written.contains_key(*id) || !settled(rows, i, flush) {
                continue;
            }
            let Some((kind, body)) = describe(item) else {
                // Nothing to say (an empty message): never written, nor waited on.
                log.fresh.remove(*id);
                continue;
            };
            let at = log.fresh.remove(*id).or(match item {
                Item::User { at, .. } => *at,
                Item::TurnEnd { at, .. } => Some(*at),
                _ => placed[i].1,
            });
            log.entries += 1;
            let entry = format_entry(log.entries, log.lines + 1, placed[i].0, at, &kind, &body);
            log.lines += newlines(&entry);
            text.push_str(&entry);
            let hash = hash_item(item);
            log.written.insert(id.to_string(), (log.entries, hash));
            let _ = writeln!(idx, "{} {hash:016x} {id}", log.entries);
        }

        if text.is_empty() && idx.is_empty() {
            return Ok(path);
        }
        std::fs::create_dir_all(&self.dir)?;
        let append = |p: &Path, s: &str| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?;
            f.write_all(s.as_bytes())
        };
        // The log first: an id in the sidecar is always in the log too.
        append(&path, &text)?;
        append(&index, &idx)?;
        Ok(path)
    }
}

/// A file name for a thread id (they're uuids; anything else is kept to safe characters).
fn file_stem(thread: &str) -> String {
    thread
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn newlines(s: &str) -> u64 {
    s.bytes().filter(|b| *b == b'\n').count() as u64
}

/// Whether the item at `i` won't change any more: a message once something follows it, a tool
/// call once it has finished. With `flush`, everything is.
fn settled(rows: &[Row<'_>], i: usize, flush: bool) -> bool {
    if flush {
        return true;
    }
    match rows[i].1 {
        Item::Tool {
            status: ToolStatus::Running,
            ..
        } => false,
        Item::Assistant { .. } | Item::Reasoning { .. } => i + 1 < rows.len(),
        _ => true,
    }
}

/// FNV-1a over the item's JSON: stable across builds, so a relaunch knows what it wrote.
fn hash_item(item: &Item) -> u64 {
    let json = serde_json::to_string(item).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `[1, 2, 3, 7]` as "#1-#3, #7".
fn ranges(numbers: &[u32]) -> String {
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < numbers.len() {
        let start = numbers[i];
        let mut end = start;
        while i + 1 < numbers.len() && numbers[i + 1] == end + 1 {
            i += 1;
            end = numbers[i];
        }
        out.push(if start == end {
            format!("#{start}")
        } else {
            format!("#{start}-#{end}")
        });
        i += 1;
    }
    out.join(", ")
}

fn time(at: i64) -> Option<String> {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(at)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// One entry: its header, then `body` (if any), then a blank line.
fn format_entry(n: u32, line: u64, turn: u32, at: Option<i64>, kind: &str, body: &str) -> String {
    let mut s = format!("{HEADER}{n} L{line} turn {turn}");
    if let Some(t) = at.and_then(time) {
        s.push(' ');
        s.push_str(&t);
    }
    s.push(' ');
    s.push_str(kind);
    s.push('\n');
    if !body.is_empty() {
        s.push_str(body);
        if !body.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push('\n');
    s
}

/// `text` as an entry keeps it: whole, or cut at `MAX_BODY` with a line saying so.
fn kept(text: &str) -> String {
    if text.len() <= MAX_BODY {
        return text.to_string();
    }
    let mut end = MAX_BODY;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[Cut here: this is {} bytes, and the transcript keeps the first {MAX_BODY}.]",
        &text[..end],
        text.len()
    )
}

/// A labelled field: on its line when it's one line, else on the lines after it.
fn field(name: &str, value: &str) -> String {
    let value = value.trim_end_matches('\n');
    if value.is_empty() {
        format!("{name}: (none)")
    } else if value.contains('\n') {
        format!("{name}:\n{}", kept(value))
    } else {
        format!("{name}: {}", kept(value))
    }
}

/// An item's kind (for its header) and text. `None` for an empty message or thought.
fn describe(item: &Item) -> Option<(String, String)> {
    Some(match item {
        Item::User {
            text,
            images,
            aside,
            ..
        } => {
            let mut body = kept(text);
            if !images.is_empty() {
                let _ = write!(body, "\n[images: {}]", images.join(", "));
            }
            let kind = if *aside { "user (aside)" } else { "user" };
            (kind.to_string(), body)
        }
        Item::Assistant { text } if !text.trim().is_empty() => ("assistant".into(), kept(text)),
        Item::Reasoning { text } if !text.trim().is_empty() => ("reasoning".into(), kept(text)),
        Item::Assistant { .. } | Item::Reasoning { .. } => return None,
        Item::TurnEnd { took_secs, .. } => {
            (format!("turn end (took {took_secs} s)"), String::new())
        }
        Item::Tool {
            title,
            detail,
            output,
            status,
            ..
        } => {
            let status = match status {
                ToolStatus::Running => "running",
                ToolStatus::Done => "done",
                ToolStatus::Failed => "failed",
                ToolStatus::Denied => "denied",
            };
            let result = if is_own_tool(title) && !output.is_empty() {
                let n = output.lines().count();
                format!(
                    "result: ({n} line{} of this transcript, not repeated here)",
                    if n == 1 { "" } else { "s" }
                )
            } else {
                field("result", output)
            };
            (
                format!("tool {} ({status})", one_line(title)),
                format!("{}\n{result}", field("args", detail)),
            )
        }
        Item::Notice { text } => ("notice".into(), kept(text)),
        Item::Error { text } => ("error".into(), kept(text)),
        Item::Limit {
            text, resets_at, ..
        } => {
            let mut body = kept(text);
            if let Some(t) = resets_at.and_then(time) {
                let _ = write!(body, "\n[resets {t}]");
            }
            ("usage limit".into(), body)
        }
        Item::Handoff {
            from,
            from_name,
            to,
            to_name,
            ..
        } => (
            "agent change".into(),
            format!(
                "From {} to {}: the new agent started from a recap.",
                from_name.as_deref().unwrap_or(from),
                to_name.as_deref().unwrap_or(to)
            ),
        ),
    })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- reading it back ----

/// The most matches a search shows, and lines of context around each.
pub const MAX_MATCHES: usize = 100;
pub const MAX_CONTEXT: usize = 20;
/// The most text one answer carries.
const MAX_ANSWER: usize = 40_000;
/// A line longer than this is shown cut, around the match.
const LINE_SHOWN: usize = 600;
/// The most lines `read` shows at once.
pub const MAX_READ: usize = 400;

/// A `transcript_search`.
#[derive(Debug, Clone)]
pub struct Query {
    pub text: String,
    /// `text` is a regular expression; else words, all of which must be on the line (a
    /// "quoted phrase" counts as one).
    pub regex: bool,
    pub context: usize,
    pub max: usize,
}

impl Query {
    /// From a tool call's arguments: `query`, `regex`, `context`, `max`.
    pub fn from_args(args: &serde_json::Value) -> Result<Query, String> {
        let text = args["query"]
            .as_str()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or("transcript_search needs a query.")?
            .to_string();
        let n = |k: &str, default: usize, max: usize| {
            args[k].as_u64().map_or(default, |v| (v as usize).min(max))
        };
        Ok(Query {
            text,
            regex: args["regex"].as_bool().unwrap_or(false),
            context: n("context", 2, MAX_CONTEXT),
            max: n("max", 20, MAX_MATCHES).max(1),
        })
    }
}

enum Matcher {
    Regex(regex::Regex),
    Words(Vec<String>),
}

impl Matcher {
    fn new(q: &Query) -> Result<Matcher, String> {
        if q.regex {
            return regex::RegexBuilder::new(&q.text)
                .case_insensitive(true)
                .size_limit(1 << 22)
                .build()
                .map(Matcher::Regex)
                .map_err(|e| format!("That isn't a regular expression this search reads: {e}"));
        }
        let mut words = vec![];
        let mut rest = q.text.as_str();
        while !rest.is_empty() {
            rest = rest.trim_start();
            if let Some(quoted) = rest.strip_prefix('"') {
                let (phrase, after) = quoted.split_once('"').unwrap_or((quoted, ""));
                if !phrase.trim().is_empty() {
                    words.push(phrase.to_lowercase());
                }
                rest = after;
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                if end > 0 {
                    words.push(rest[..end].to_lowercase());
                }
                rest = &rest[end..];
            }
        }
        Ok(Matcher::Words(words))
    }

    /// Where on `line` it matches (the first match's byte range), if it does.
    fn find(&self, line: &str) -> Option<(usize, usize)> {
        match self {
            Matcher::Regex(r) => r.find(line).map(|m| (m.start(), m.end())),
            Matcher::Words(words) => {
                let lower = line.to_lowercase();
                // Lowercasing can change byte lengths; the spot is only for showing the line.
                let mut first = None;
                for w in words {
                    let at = lower.find(w.as_str())?;
                    first = Some(first.map_or(at, |f: usize| f.min(at)));
                }
                first.map(|at| (at.min(line.len()), (at + 1).min(line.len())))
            }
        }
    }
}

fn read_lines(path: &Path) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("This thread has no transcript yet.".into())
        }
        Err(e) => Err(format!("Couldn't read the transcript: {e}")),
    }
}

/// `line` for showing: whole, or cut around `at` with a note on how to see all of it.
fn shown(n: usize, line: &str, at: Option<(usize, usize)>) -> String {
    if line.len() <= LINE_SHOWN {
        return line.to_string();
    }
    let center = at.map_or(0, |(s, _)| s);
    let mut start = center.saturating_sub(LINE_SHOWN / 3);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + LINE_SHOWN).min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}{}{} [line {n} is {} chars; transcript_read shows it whole]",
        if start > 0 { "…" } else { "" },
        &line[start..end],
        if end < line.len() { "…" } else { "" },
        line.len()
    )
}

/// The latest header at or before line index `i` (0-based), as "#12 turn 3 … tool Bash".
fn entry_of<'a>(headers: &[usize], lines: &[&'a str], i: usize) -> Option<&'a str> {
    let h = headers.partition_point(|&h| h <= i).checked_sub(1)?;
    lines[headers[h]].strip_prefix("### ")
}

/// Search the transcript at `path`: the latest `max` matching lines, each with `context` lines
/// around it and the entry it's in, numbered as `read` takes them.
pub fn search(path: &Path, q: &Query) -> Result<String, String> {
    let matcher = Matcher::new(q)?;
    let text = read_lines(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let headers: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with(HEADER))
        .map(|(i, _)| i)
        .collect();
    let hits: Vec<(usize, (usize, usize))> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| matcher.find(l).map(|at| (i, at)))
        .collect();
    let total = hits.len();
    if total == 0 {
        return Ok(format!(
            "No line of the transcript ({} lines) matches {:?}{}.",
            lines.len(),
            q.text,
            if q.regex {
                " (a regular expression, any case)"
            } else {
                " (all of its words on one line, any case)"
            }
        ));
    }
    let skipped = total.saturating_sub(q.max);
    let hits = &hits[skipped..];
    let at: HashMap<usize, (usize, usize)> = hits.iter().copied().collect();
    // Runs of lines to show: each hit with its context, overlapping runs joined.
    let mut runs: Vec<(usize, usize)> = vec![];
    for &(i, _) in hits {
        let (from, to) = (
            i.saturating_sub(q.context),
            (i + q.context).min(lines.len() - 1),
        );
        match runs.last_mut() {
            Some(last) if from <= last.1 + 1 => last.1 = last.1.max(to),
            _ => runs.push((from, to)),
        }
    }
    let mut out = format!(
        "{} matching line{} in the transcript ({} lines){}:\n",
        total,
        if total == 1 { "" } else { "s" },
        lines.len(),
        if skipped > 0 {
            format!(
                "; the latest {} shown, {skipped} earlier ones before line {} left out (narrow the query, or transcript_read earlier lines)",
                hits.len(),
                hits[0].0 + 1
            )
        } else {
            String::new()
        }
    );
    for (from, to) in runs {
        if out.len() > MAX_ANSWER {
            let _ = writeln!(
                out,
                "[Stopped at {MAX_ANSWER} characters: narrow the query or lower context to see the rest.]"
            );
            break;
        }
        // Which entry the run is in, unless its header is the run's first line.
        if !lines[from].starts_with(HEADER)
            && let Some(entry) = entry_of(&headers, &lines, from)
        {
            let _ = writeln!(out, "-- in {entry}");
        } else {
            out.push_str("--\n");
        }
        for (i, line) in lines.iter().enumerate().take(to + 1).skip(from) {
            let mark = if at.contains_key(&i) { ':' } else { '-' };
            let _ = writeln!(
                out,
                "{}{mark}\t{}",
                i + 1,
                shown(i + 1, line, at.get(&i).copied())
            );
        }
    }
    Ok(out)
}

/// Lines `start` to `end` (1-based, inclusive; `end` defaults to `MAX_READ` lines on) of the
/// transcript at `path`, numbered.
pub fn read(path: &Path, start: usize, end: Option<usize>) -> Result<String, String> {
    let text = read_lines(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = start.max(1);
    if start > total {
        return Err(format!(
            "The transcript has {total} lines: start at {total} or before."
        ));
    }
    let asked_end = end.unwrap_or(start + MAX_READ - 1).max(start).min(total);
    let end = asked_end.min(start + MAX_READ - 1);
    let mut body = String::new();
    let mut last = start - 1;
    for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
        if body.len() + line.len() > MAX_ANSWER && i + 1 > start {
            break;
        }
        let line = if line.len() > MAX_ANSWER {
            let mut cut = MAX_ANSWER;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            format!(
                "{} [cut: line {} is {} chars; the rest isn't shown]",
                &line[..cut],
                i + 1,
                line.len()
            )
        } else {
            line.to_string()
        };
        let _ = writeln!(body, "{}\t{line}", i + 1);
        last = i + 1;
    }
    let more = if last < asked_end {
        format!(
            " (stopped at line {last} to keep this short: read on from {})",
            last + 1
        )
    } else if last < total {
        format!("; {} more after it", total - last)
    } else {
        String::new()
    };
    Ok(format!(
        "Transcript lines {start}-{last} of {total}{more}:\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(t: &str) -> Item {
        Item::User {
            text: t.into(),
            images: vec![],
            at: Some(1_761_900_000_000),
            resume: None,
            aside: false,
        }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }
    fn tool(id: &str, output: &str, status: ToolStatus) -> Item {
        Item::Tool {
            id: id.into(),
            title: "Bash".into(),
            detail: "cargo test".into(),
            output: output.into(),
            status,
        }
    }
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "trek-session-log-{name}-{}-{}",
            std::process::id(),
            crate::store::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn rows(items: &[(String, Item)]) -> Vec<Row<'_>> {
        items.iter().map(|(id, i)| (id.as_str(), i)).collect()
    }
    fn log_text(logs: &Logs, thread: &str) -> String {
        std::fs::read_to_string(logs.path(thread)).unwrap_or_default()
    }

    #[test]
    fn entries_have_numbered_headers_and_exact_bodies() {
        let d = dir("headers");
        let mut logs = Logs::new(d.clone());
        let output = "error[E0308]: mismatched types\n --> src/parser.rs:42:17\n";
        let items = vec![
            ("u1".to_string(), user("fix the parser")),
            ("t1".to_string(), tool("c1", output, ToolStatus::Done)),
            ("a1".to_string(), said("Fixed it.")),
            (
                "e1".to_string(),
                Item::TurnEnd {
                    at: 1_761_900_060_000,
                    took_secs: 60,
                },
            ),
        ];
        logs.record("th", &rows(&items), Delta::default(), false)
            .unwrap();
        let text = log_text(&logs, "th");
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("### #1 L1 turn 1 ") && lines[0].ends_with(" user"),
            "{}",
            lines[0]
        );
        assert_eq!(lines[1], "fix the parser");
        assert!(
            lines[3].starts_with("### #2 L4 turn 1") && lines[3].ends_with("tool Bash (done)"),
            "{}",
            lines[3]
        );
        assert_eq!(lines[4], "args: cargo test");
        assert_eq!(lines[5], "result:");
        assert_eq!(lines[6], "error[E0308]: mismatched types");
        assert_eq!(lines[7], " --> src/parser.rs:42:17");
        // Every header says the line it's on.
        for (i, l) in lines.iter().enumerate() {
            if let Some(rest) = l.strip_prefix(HEADER) {
                let line: usize = rest.split(' ').nth(1).unwrap()[1..].parse().unwrap();
                assert_eq!(line, i + 1, "{l}");
            }
        }
        assert!(text.contains("turn end (took 60 s)"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn items_wait_until_settled_and_are_written_once() {
        let d = dir("settle");
        let mut logs = Logs::new(d.clone());
        let mut items = vec![
            ("u1".to_string(), user("go")),
            ("a1".to_string(), said("Look")),
            ("t1".to_string(), tool("c1", "", ToolStatus::Running)),
        ];
        let appended: Vec<&str> = vec!["u1", "a1", "t1"];
        logs.record(
            "th",
            &rows(&items),
            Delta {
                appended: &appended,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        let text = log_text(&logs, "th");
        assert!(
            text.contains("Look"),
            "followed by a tool call, the message is done"
        );
        assert!(!text.contains("tool Bash"), "a running call waits");
        // It finishes, and a streaming answer starts.
        items[2].1 = tool("c1", "ok: 12 passed", ToolStatus::Done);
        items.push(("a2".to_string(), said("All gr")));
        let changed = ["t1"];
        logs.record(
            "th",
            &rows(&items),
            Delta {
                changed: &changed,
                appended: &["a2"],
                ..Default::default()
            },
            false,
        )
        .unwrap();
        let text = log_text(&logs, "th");
        assert!(text.contains("ok: 12 passed"));
        assert!(!text.contains("All gr"), "the last message may still grow");
        items[3].1 = said("All green.");
        logs.record(
            "th",
            &rows(&items),
            Delta {
                changed: &["a2"],
                ..Default::default()
            },
            true,
        )
        .unwrap();
        let text = log_text(&logs, "th");
        assert!(text.contains("All green.") && !text.contains("All gr\n"));
        assert_eq!(text.matches("ok: 12 passed").count(), 1);
        // A relaunch knows what's written.
        let mut again = Logs::new(d.clone());
        again
            .record("th", &rows(&items), Delta::default(), true)
            .unwrap();
        assert_eq!(log_text(&again, "th"), text);
        assert_eq!(again.lines("th"), text.lines().count() as u64);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn changes_and_rewinds_append_rather_than_rewrite() {
        let d = dir("rewind");
        let mut logs = Logs::new(d.clone());
        let mut items = vec![
            ("u1".to_string(), user("remember APPLE")),
            ("a1".to_string(), said("ok")),
            ("u2".to_string(), user("remember PEAR")),
            ("a2".to_string(), said("ok 2")),
        ];
        logs.record("th", &rows(&items), Delta::default(), true)
            .unwrap();
        let before = log_text(&logs, "th");
        // A changed item comes again; one touched but unchanged doesn't.
        items[1].1 = said("ok, APPLE");
        logs.record(
            "th",
            &rows(&items),
            Delta {
                changed: &["a1", "a2"],
                ..Default::default()
            },
            true,
        )
        .unwrap();
        let text = log_text(&logs, "th");
        assert!(text.starts_with(&before), "nothing written is rewritten");
        assert!(text.contains("assistant (revises #2)\nok, APPLE"), "{text}");
        assert_eq!(text.matches("ok 2").count(), 1);
        // The second message is taken back and another sent.
        let removed = vec!["u2".to_string(), "a2".to_string()];
        items.truncate(2);
        items.push(("u3".to_string(), user("remember PLUM")));
        logs.record(
            "th",
            &rows(&items),
            Delta {
                removed: &removed,
                appended: &["u3"],
                ..Default::default()
            },
            true,
        )
        .unwrap();
        let text = log_text(&logs, "th");
        assert!(text.contains("Entries #3-#4 were taken back"), "{text}");
        assert!(text.trim_end().ends_with("remember PLUM"));
        // A relaunch that finds an item gone notes it too.
        let mut again = Logs::new(d.clone());
        items.remove(0);
        again
            .record("th", &rows(&items), Delta::default(), true)
            .unwrap();
        assert!(log_text(&again, "th").contains("Entries #1 were taken back"));
        again.forget("th");
        assert!(!again.path("th").exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn own_results_arent_logged_again_and_long_ones_say_where_they_were_cut() {
        let d = dir("own");
        let mut logs = Logs::new(d.clone());
        let big = "x".repeat(MAX_BODY + 10);
        let items = vec![
            ("u1".to_string(), user("go")),
            (
                "t1".to_string(),
                Item::Tool {
                    id: "c".into(),
                    title: "mcp__trek-orchestrate__transcript_search".into(),
                    detail: "{\"query\":\"E0308\"}".into(),
                    output: "3 matching lines\n12:\terror".into(),
                    status: ToolStatus::Done,
                },
            ),
            ("t2".to_string(), tool("c2", &big, ToolStatus::Done)),
        ];
        logs.record("th", &rows(&items), Delta::default(), true)
            .unwrap();
        let text = log_text(&logs, "th");
        assert!(text.contains("result: (2 lines of this transcript, not repeated here)"));
        assert!(!text.contains("12:\terror"));
        assert!(text.contains(&format!("[Cut here: this is {} bytes", MAX_BODY + 10)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn searches_find_exact_lines_with_their_entry() {
        let d = dir("search");
        let mut logs = Logs::new(d.clone());
        let items = vec![
            ("u1".to_string(), user("fix the parser")),
            (
                "t1".to_string(),
                tool(
                    "c1",
                    "line one\nerror[E0308]: mismatched types\n --> src/parser.rs:42:17\nline four",
                    ToolStatus::Failed,
                ),
            ),
            ("a1".to_string(), said("The Mismatched type is on line 42.")),
        ];
        let path = logs
            .record("th", &rows(&items), Delta::default(), true)
            .unwrap();
        let q = |args: serde_json::Value| Query::from_args(&args).unwrap();
        let out = search(&path, &q(json!({"query": "mismatched", "context": 0}))).unwrap();
        assert!(out.starts_with("2 matching lines"), "{out}");
        assert!(out.contains("-- in #2 L4 turn 1"), "{out}");
        assert!(out.contains(":\terror[E0308]: mismatched types"));
        // Words in any order; a quoted phrase as one.
        let out = search(&path, &q(json!({"query": "parser.rs \"42:17\""}))).unwrap();
        assert!(out.starts_with("1 matching line "), "{out}");
        assert!(
            out.contains("-\terror[E0308]"),
            "context lines around it: {out}"
        );
        let out = search(&path, &q(json!({"query": r"E0\d+", "regex": true}))).unwrap();
        assert!(out.contains("E0308"));
        assert!(search(&path, &q(json!({"query": "(", "regex": true}))).is_err());
        let none = search(&path, &q(json!({"query": "nowhere"}))).unwrap();
        assert!(none.starts_with("No line"));
        // Only the latest matches past `max`.
        let out = search(&path, &q(json!({"query": "e", "max": 1, "context": 0}))).unwrap();
        assert!(out.contains("the latest 1 shown"), "{out}");
        // Reading lines back.
        let lines = read(&path, 4, Some(6)).unwrap();
        assert!(lines.starts_with("Transcript lines 4-6 of"), "{lines}");
        assert!(lines.contains("4\t### #2"));
        assert!(read(&path, 999, None).is_err());
        assert!(Query::from_args(&json!({})).is_err());
        assert!(search(&d.join("none.log"), &q(json!({"query": "x"}))).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_retrieval_instruction_names_the_file_and_the_tool() {
        let r = retrieval(Path::new("/data/transcripts/t.log"), 120, None);
        assert!(
            r.contains("\"/data/transcripts/t.log\" (120 lines)")
                && r.contains("transcript_search")
        );
        let r = retrieval(Path::new("/x.log"), 3, Some("abc"));
        assert!(r.contains("transcript_search with thread \"abc\""));
        assert_eq!(ranges(&[1, 2, 3, 7, 9, 10]), "#1-#3, #7, #9-#10");
        assert!(is_own_tool("trek-orchestrate_transcript_read") && is_own_tool(SEARCH_TITLE));
        assert!(!is_own_tool("Bash"));
    }
}
