//! "Pull threads from the machine": index sessions created by other agents so they show up
//! in Trek and can be resumed with the agent that made them. Indexing reads metadata only;
//! transcripts are loaded lazily (Claude Code files can exceed 100 MB).
//!
//! Not every session on disk is a conversation: other apps run title generators, test probes
//! and sub-agents through the same agents. Those are recognised by [`Skip`] rules and left out.

pub mod claude;
pub mod codex;
pub mod opencode;

use crate::store::{Item, Store};
use crate::types::{Effort, ThreadSource};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedThread {
    pub source: ThreadSource,
    pub native_id: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub created_at: i64,
    pub updated_at: i64,
    pub additions: i64,
    pub deletions: i64,
    /// Why this session isn't one of the user's conversations, if it isn't. Such sessions are
    /// never added, and threads imported before the rule existed are archived.
    pub skip: Option<Skip>,
    /// The title earlier Trek versions gave this session, to tell their automatic titles apart
    /// from ones the user typed (threads imported before the imported title was recorded).
    pub legacy_title: Option<String>,
}

/// A session that isn't a conversation of the user's, by the evidence that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Skip {
    /// Trek started it; the Trek thread already shows it.
    Trek,
    /// A sub-agent's or side chain's transcript; it belongs to its parent thread.
    Subagent,
    /// A fork that was never continued: a copy of a session that is imported anyway.
    UntouchedFork,
    /// One prompt asking for a title for another thread (T3 Code and similar apps).
    TitleGenerator,
    /// Ran in a system temp folder, started by a program or with a single prompt.
    TempDir,
    /// A single-prompt `codex exec` run (scripts and agents, not the Codex SDK).
    OneShotExec,
    /// Nothing typed and nothing answered: empty, or only local commands like `/clear`.
    NoUserMessage,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Skip::Trek => "started by Trek",
            Skip::Subagent => "sub-agent",
            Skip::UntouchedFork => "untouched fork",
            Skip::TitleGenerator => "title generator",
            Skip::TempDir => "temp folder run",
            Skip::OneShotExec => "one-shot exec",
            Skip::NoUserMessage => "no message",
        }
    }
}

/// What a scanner learned about a session, for [`classify`].
#[derive(Debug, Default)]
pub(crate) struct Evidence<'a> {
    pub cwd: Option<&'a Path>,
    /// Started by a program (SDK, print mode, app server) rather than in the agent's own UI.
    pub scripted: bool,
    /// Prompts the user sent; `None` when the session was too long to count them all.
    pub prompts: Option<usize>,
    /// The first user message as sent, injected context included.
    pub first_message: Option<&'a str>,
    pub replied: bool,
    pub subagent: bool,
    pub untouched_fork: bool,
    pub trek: bool,
    /// A `codex exec` run outside the Codex SDK.
    pub exec: bool,
}

/// The rule a session matches, strongest evidence first. Each rule needs evidence that holds
/// for helper sessions and not for conversations; a false match hides a real conversation.
pub(crate) fn classify(e: &Evidence) -> Option<Skip> {
    let single = e.prompts == Some(1);
    if e.trek {
        Some(Skip::Trek)
    } else if e.subagent {
        Some(Skip::Subagent)
    } else if e.untouched_fork {
        Some(Skip::UntouchedFork)
    } else if single && e.first_message.is_some_and(is_title_request) {
        Some(Skip::TitleGenerator)
    } else if e.cwd.is_some_and(is_temp_dir) && (e.scripted || e.prompts.is_some_and(|n| n <= 1)) {
        Some(Skip::TempDir)
    } else if e.exec && single {
        Some(Skip::OneShotExec)
    } else if e.prompts == Some(0) && !e.replied {
        Some(Skip::NoUserMessage)
    } else {
        None
    }
}

/// System temp folders, where apps run title generators, scratch runs and tests.
pub(crate) fn is_temp_dir(path: &Path) -> bool {
    ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", "/var/tmp", "/private/var/tmp"]
        .iter()
        .any(|root| path.starts_with(root))
        || path.starts_with(std::env::temp_dir())
}

/// A prompt written by an app to name one of its threads, not by a person.
pub(crate) fn is_title_request(message: &str) -> bool {
    let head: String = message.trim_start().chars().take(240).collect::<String>().to_lowercase();
    let asks = ["generate a title", "regenerate the title", "generate a short title", "generate a concise title"]
        .iter()
        .any(|p| head.starts_with(p));
    let about_a_thread = ["thread", "session", "conversation", "chat"].iter().any(|w| head.contains(w));
    // Trek's own namer (it runs without saving a session, but older builds didn't).
    let trek = head.starts_with("<conversation>") && message.contains("Write the title for the conversation above");
    (asks && about_a_thread) || trek
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportSummary {
    /// Conversations found per source (helper sessions not counted).
    pub claude_code: usize,
    pub codex: usize,
    pub opencode: usize,
    pub new_threads: usize,
    /// Threads whose automatic title was replaced by a better one.
    pub retitled: usize,
    /// Sessions left out, by rule.
    pub skipped: BTreeMap<Skip, usize>,
    /// Previously imported threads archived because a rule now matches them.
    pub hidden: usize,
    /// Threads archived by an earlier import that turned out to be conversations after all.
    pub restored: usize,
    /// Threads archived because their session was deleted from the agent's history.
    pub gone: usize,
}

impl ImportSummary {
    pub fn total(&self) -> usize {
        self.claude_code + self.codex + self.opencode
    }

    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }
}

/// Scan all enabled sources and upsert into the store.
pub fn import_all(store: &Store, settings: &crate::settings::Import) -> ImportSummary {
    let min_updated = if settings.max_age_days == 0 {
        0
    } else {
        crate::store::now_ms() - settings.max_age_days as i64 * 86_400_000
    };
    let mut found: Vec<ImportedThread> = Vec::new();
    let mut known = Vec::new();
    if settings.claude_code {
        found.extend(claude::scan(min_updated));
        known.push((ThreadSource::ClaudeCode, claude::session_ids()));
    }
    if settings.codex {
        found.extend(codex::scan(min_updated));
        known.push((ThreadSource::Codex, codex::session_ids()));
    }
    if settings.opencode {
        found.extend(opencode::scan(min_updated));
        known.push((ThreadSource::OpenCode, opencode::session_ids()));
    }
    let mut summary = import_found(store, found);
    // A thread whose session was deleted can't be opened any more. Only when the history
    // could be read: an unreadable one says nothing about what's in it.
    for (source, ids) in known {
        if let Some(ids) = ids {
            summary.gone += store.hide_missing(source, &ids).unwrap_or_else(|e| {
                tracing::warn!("import {}: {e}", source.label());
                0
            });
        }
    }
    summary
}

/// Add what the scanners found, archive what turned out not to be conversations.
fn import_found(store: &Store, mut found: Vec<ImportedThread>) -> ImportSummary {
    // Trek's own threads are written to the agents' history too; don't show them twice.
    let trek: HashSet<String> = store.trek_native_ids().unwrap_or_default();
    for t in found.iter_mut().filter(|t| t.skip.is_none() && trek.contains(&t.native_id)) {
        t.skip = Some(Skip::Trek);
    }
    let mut summary = ImportSummary::default();
    for t in &found {
        match (t.skip, t.source) {
            (Some(rule), _) => *summary.skipped.entry(rule).or_default() += 1,
            (None, ThreadSource::ClaudeCode) => summary.claude_code += 1,
            (None, ThreadSource::Codex) => summary.codex += 1,
            (None, ThreadSource::OpenCode) => summary.opencode += 1,
            (None, _) => {}
        }
    }
    match store.upsert_imported(&found) {
        Ok(outcomes) => {
            for o in outcomes {
                summary.new_threads += o.added as usize;
                summary.retitled += o.retitled as usize;
                summary.hidden += o.hidden as usize;
                summary.restored += o.restored as usize;
            }
        }
        Err(e) => tracing::warn!("import: {e}"),
    }
    summary
}

/// Load the transcript of an imported thread.
pub fn load_transcript(source: ThreadSource, native_id: &str) -> anyhow::Result<Vec<Item>> {
    match source {
        ThreadSource::ClaudeCode => claude::load(native_id),
        ThreadSource::Codex => codex::load(native_id),
        ThreadSource::OpenCode => opencode::load(native_id),
        _ => Ok(vec![]),
    }
}

// ---- titles ----

const TITLE_MAX: usize = 60;
/// Titles the source wrote are already meant as titles; only very long ones are cut.
const SOURCE_TITLE_MAX: usize = 80;

/// A thread title from the first thing the user said: injected context dropped, markdown and
/// image references removed, first line only, cut to about 60 characters on a word boundary.
pub fn title_from(text: &str) -> String {
    user_text(text)
        .and_then(|t| shorten(&t))
        .or_else(|| shorten(text))
        .unwrap_or_else(|| "Untitled".into())
}

/// A source's own title (a summary, a thread name), cleaned up; `None` for placeholders.
pub(crate) fn source_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = collapse_spaces(line);
    if is_placeholder_title(&line) {
        return None;
    }
    // Keep a fork marker visible when the rest is cut.
    let (base, suffix) = match line.rfind(" (fork") {
        Some(i) if line.ends_with(')') => (&line[..i], &line[i..]),
        _ => (line.as_str(), ""),
    };
    let base = clip_words(base.trim(), SOURCE_TITLE_MAX.saturating_sub(suffix.chars().count()).max(TITLE_MAX / 2));
    Some(format!("{base}{suffix}"))
}

/// Titles apps give a session before anything happened in it.
fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim();
    let after = |prefix: &str| t.strip_prefix(prefix).map(str::trim);
    // OpenCode: "New session - 2026-10-02T19:54:53.419Z"; T3 Code: "T3 Code <thread uuid>".
    let stamped = |rest: Option<&str>| rest.is_some_and(|r| r.len() >= 10 && r.as_bytes()[..4].iter().all(u8::is_ascii_digit));
    t.is_empty()
        || t.eq_ignore_ascii_case("untitled")
        || stamped(after("New session -"))
        || stamped(after("Child session -"))
        || after("T3 Code").is_some_and(|r| r.len() == 36 && r.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

/// What the user typed in a message, without the context agents and apps wrap around it;
/// `None` when the whole message is injected (instructions, command output, reminders).
pub(crate) fn user_text(raw: &str) -> Option<String> {
    let mut text = raw.trim();
    // Codex desktop lists attachments first and puts the message under "My request".
    if text.starts_with("# Files mentioned by the user") || text.starts_with("# Files pasted by the user") {
        text = text.split_once("## My request:")?.1.trim();
    }
    if is_injected(text) {
        return None;
    }
    let text = strip_image_refs(&strip_pasted(text));
    // Codex goal mode is a prefix on the request itself.
    let text = text.strip_prefix("/goal ").unwrap_or(&text).trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Context injected into the user's side of the conversation: tag-wrapped blocks
/// (`<environment_context>`, `<command-name>`, `<system-reminder>`, …), instruction files,
/// caveats and resume notes. Pasted text is the user's own, even though it comes in a tag.
pub(crate) fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    if t.is_empty() {
        return true;
    }
    if let Some(tag) = t.strip_prefix('<') {
        return tag.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/') && !tag.starts_with("pasted_content");
    }
    [
        "Caveat:",
        "This session is being continued from a previous conversation",
        "[Request interrupted by user",
        "# AGENTS.md instructions for",
        "Base directory for this skill:",
        "[Image: source:",
        "[Image: original ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// The marker Claude Code leaves when the user stops a turn.
pub(crate) fn is_interruption(text: &str) -> bool {
    text.trim_start().starts_with("[Request interrupted by user")
}

/// `<pasted_content …>text</pasted_content>` blocks: dropped when the user typed something around
/// them, unwrapped when the paste is the whole message.
fn strip_pasted(text: &str) -> String {
    const OPEN: &str = "<pasted_content";
    const CLOSE: &str = "</pasted_content>";
    let (mut rest, mut typed, mut first_paste) = (text, String::new(), None);
    while let Some(start) = rest.find(OPEN) {
        typed.push_str(&rest[..start]);
        let block = &rest[start..];
        let body_start = block.find('>').map_or(block.len(), |i| i + 1);
        let end = block.find(CLOSE).unwrap_or(block.len());
        if first_paste.is_none() && body_start <= end {
            first_paste = Some(block[body_start..end].to_string());
        }
        rest = block.get(end + CLOSE.len()..).unwrap_or("");
    }
    typed.push_str(rest);
    if typed.trim().is_empty() { first_paste.unwrap_or_default() } else { typed }
}

/// A message with pasted text as the user saw it: the paste markers removed, the text kept.
pub(crate) fn unwrap_pasted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("<pasted_content") {
        out.push_str(&rest[..i]);
        let tag = &rest[i..];
        rest = tag.find('>').map_or("", |end| &tag[end + 1..]);
    }
    out.push_str(rest);
    out.replace("</pasted_content>", "").trim().to_string()
}

/// Removes `[Image #3]` / `[Image 3]` references that stand in for attached screenshots.
fn strip_image_refs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("[Image ") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + "[Image ".len()..];
        let digits = tail.strip_prefix('#').unwrap_or(tail);
        let n = digits.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 && digits[n..].starts_with(']') {
            rest = &digits[n + 1..];
        } else {
            out.push_str("[Image ");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The first line with words in it, markdown removed, cut on a word boundary.
fn shorten(text: &str) -> Option<String> {
    let line = text.lines().map(clean_line).find(|l| l.chars().any(char::is_alphanumeric))?;
    Some(clip_words(&line, TITLE_MAX))
}

/// One line without heading/quote/list markers, emphasis, code ticks or link targets.
fn clean_line(line: &str) -> String {
    let l = line.trim().trim_start_matches(['#', '>', '*', '-', '•', ' ']);
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    // [text](target) → text
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|m| open + m) else { break };
        let Some(close) = rest[mid..].find(')').map(|c| mid + c) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..mid]);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    let out = collapse_spaces(&out.replace("**", "").replace('`', ""));
    // A message sent in quotes: "Reply with exactly: OK"
    match out.strip_prefix('"').and_then(|o| o.strip_suffix('"')) {
        Some(inner) if !inner.contains('"') => inner.trim().to_string(),
        _ => out,
    }
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, cut at the last word boundary in the second half, with an ellipsis.
fn clip_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(i) if cut[..i].chars().count() >= max / 2 => &cut[..i],
        _ => cut.as_str(),
    };
    let cut = cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:-–—(".contains(c));
    format!("{cut}…")
}

/// How Trek titled sessions before titles were cleaned up: first line, 80 characters.
pub(crate) fn legacy_title_from(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled");
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    t
}

/// What Trek used to treat as injected when picking the first prompt.
pub(crate) fn legacy_is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') || t.starts_with("Caveat:") || t.starts_with("This session is being continued from a previous conversation") || t.is_empty()
}

// ---- transcripts ----

/// Builds an imported transcript, closing each answered turn with the footer live turns get
/// (when the reply finished, and how long it took from the message that started it). As in a
/// live thread, a reply runs until the user's next message: work the agent picks back up on its
/// own (a background task reporting back, a goal nudging it on) belongs to the same reply.
#[derive(Default)]
pub(crate) struct Transcript {
    pub items: Vec<Item>,
    start: Option<i64>,
    last: Option<i64>,
    done: bool,
}

impl Transcript {
    pub fn user(&mut self, text: String, at: Option<i64>) {
        self.close();
        self.items.push(Item::User { text, images: vec![], at });
        self.start = at;
        self.last = at;
    }

    /// The agent wrote or did something: the reply isn't finished yet.
    pub fn activity(&mut self, at: Option<i64>) {
        self.done = false;
        self.touch(at);
    }

    /// Something happened in the current turn at `at`.
    pub fn touch(&mut self, at: Option<i64>) {
        if let Some(t) = at {
            self.last = Some(self.last.map_or(t, |l| l.max(t)));
        }
    }

    /// The agent reported the turn finished.
    pub fn complete(&mut self, at: Option<i64>) {
        self.touch(at);
        self.done = true;
    }

    /// The user stopped the turn: it gets no footer.
    pub fn interrupt(&mut self) {
        self.start = None;
        self.done = false;
    }

    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    fn close(&mut self) {
        if let (true, Some(start), Some(end)) = (self.done, self.start, self.last)
            && matches!(self.items.last(), Some(Item::Assistant { .. }))
        {
            self.items.push(Item::TurnEnd { at: end, took_secs: ((end - start).max(0) / 1000) as u32 });
        }
        self.start = None;
        self.done = false;
    }

    pub fn finish(mut self) -> Vec<Item> {
        self.close();
        self.items
    }
}

pub(crate) fn ms_from_rfc3339(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

pub(crate) fn file_mtime_ms(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Keep transcripts readable: long tool output is clipped.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes)", &s[..end], s.len() - end)
}

/// A scratch folder for fixture-based tests, removed on drop.
#[cfg(test)]
pub(crate) struct Scratch(pub PathBuf);

#[cfg(test)]
impl Scratch {
    pub fn new() -> Scratch {
        let dir = std::env::temp_dir().join(format!("trek-import-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_what_the_user_typed() {
        assert_eq!(title_from("[Image #1] stadium and game is bugging out"), "stadium and game is bugging out");
        assert_eq!(title_from("[Image 1] [Image 2] connect copilot"), "connect copilot");
        assert_eq!(
            title_from("# Files mentioned by the user:\n\n## shot.png: /var/folders/x/shot.png\n\n## My request:\nfix the crash on launch\n"),
            "fix the crash on launch"
        );
        assert_eq!(title_from("/goal make my WiFi coverage the best"), "make my WiFi coverage the best");
        assert_eq!(title_from("\n\n<pasted_content id=\"1\">\n# Port FM Season Hub to Mac\n</pasted_content>\n"), "Port FM Season Hub to Mac");
        assert_eq!(title_from("<pasted_content id=\"1\">log…</pasted_content> why does this crash?"), "why does this crash?");
        assert_eq!(title_from("[https://fmseasonhub.com/](https://fmseasonhub.com/) ;port it"), "https://fmseasonhub.com/ ;port it");
        assert_eq!(title_from("Hey\nWorking on Trek"), "Hey");
        assert_eq!(title_from("\"Reply with exactly: OK\""), "Reply with exactly: OK");
    }

    #[test]
    fn titles_are_cut_on_a_word_boundary() {
        let t = title_from("I want to build an ai agent harness app similar to T3 code, Claude Code, Codex, Opencode");
        assert_eq!(t, "I want to build an ai agent harness app similar to T3 code…");
        assert!(t.chars().count() <= 61);
        let long_word = "x".repeat(90);
        assert_eq!(title_from(&long_word).chars().count(), 61);
    }

    #[test]
    fn injected_context_is_not_a_title() {
        for raw in [
            "<environment_context>\n  <cwd>/x</cwd>\n</environment_context>",
            "<command-name>/model</command-name>\n<command-message>model</command-message>",
            "<local-command-stdout>Set model</local-command-stdout>",
            "# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>",
            "Caveat: The messages below were generated by the user while running local commands.",
            "Base directory for this skill: /tmp/skills/claude-api",
            "[Request interrupted by user]",
            "# Files mentioned by the user:\n\n## a.png: /tmp/a.png\n",
            "   ",
        ] {
            assert_eq!(user_text(raw), None, "{raw}");
        }
        assert!(!is_injected("<pasted_content id=\"a\">x</pasted_content>"));
        assert_eq!(unwrap_pasted("\n\n<pasted_content id=\"a\">\nlog line\n</pasted_content>\nwhy?"), "log line\n\nwhy?");
        assert!(!is_injected("< 5 items is fine"));
    }

    #[test]
    fn source_titles_drop_placeholders_and_keep_fork_marks() {
        assert_eq!(source_title("New session - 2026-10-02T19:54:53.419Z"), None);
        assert_eq!(source_title("Child session - 2026-10-02T19:54:53.419Z"), None);
        assert_eq!(source_title("T3 Code 8e72d07c-b653-46c1-ade2-27747b6b23a3"), None);
        assert_eq!(source_title("New session about auth").as_deref(), Some("New session about auth"));
        assert_eq!(source_title("  Squad and  player display\n").as_deref(), Some("Squad and player display"));
        let fork = source_title(&format!("{} (fork)", "I need you to do a complete overhaul. Look at competitors such as Bartender and others, take screenshots")).unwrap();
        assert!(fork.ends_with("… (fork)"), "{fork}");
        assert!(fork.chars().count() <= 81, "{fork}");
        let ai = "FM26MacEditor performance instrumentation and autoload scan optimization";
        assert_eq!(source_title(ai).as_deref(), Some(ai));
    }

    #[test]
    fn rules_need_strong_evidence() {
        let tmp = Path::new("/private/tmp/claude-501/scratchpad");
        let repo = Path::new("/Users/me/code/app");
        let e = |f: &dyn Fn(&mut Evidence)| {
            let mut e = Evidence { cwd: Some(repo), prompts: Some(3), first_message: Some("fix the build"), replied: true, ..Default::default() };
            f(&mut e);
            classify(&e)
        };
        assert_eq!(e(&|_| {}), None);
        assert_eq!(e(&|e| e.trek = true), Some(Skip::Trek));
        assert_eq!(e(&|e| e.subagent = true), Some(Skip::Subagent));
        assert_eq!(e(&|e| e.untouched_fork = true), Some(Skip::UntouchedFork));
        // Temp folders: programs, or a single prompt; a long hand-typed session there stays.
        assert_eq!(e(&|e| { e.cwd = Some(tmp); e.scripted = true }), Some(Skip::TempDir));
        assert_eq!(e(&|e| { e.cwd = Some(tmp); e.prompts = Some(1) }), Some(Skip::TempDir));
        assert_eq!(e(&|e| e.cwd = Some(tmp)), None);
        assert_eq!(e(&|e| { e.cwd = Some(tmp); e.prompts = None }), None);
        // Title generators: one prompt, worded as a request to name a thread.
        let request = "Generate a title that will help the user recognize this T3 Code thread weeks later.\nReturn JSON";
        assert_eq!(e(&|e| { e.first_message = Some(request); e.prompts = Some(1) }), Some(Skip::TitleGenerator));
        assert_eq!(e(&|e| e.first_message = Some(request)), None);
        assert_eq!(e(&|e| { e.first_message = Some("Generate a title for my blog post"); e.prompts = Some(1) }), None);
        // One-shot exec runs.
        assert_eq!(e(&|e| { e.exec = true; e.prompts = Some(1) }), Some(Skip::OneShotExec));
        assert_eq!(e(&|e| e.exec = true), None);
        // Nothing typed: only when nothing was answered either.
        assert_eq!(e(&|e| { e.prompts = Some(0); e.replied = false }), Some(Skip::NoUserMessage));
        assert_eq!(e(&|e| e.prompts = Some(0)), None);
        assert_eq!(e(&|e| { e.prompts = None; e.replied = false }), None);
    }

    #[test]
    fn trek_sessions_are_not_imported_twice() {
        let store = Store::in_memory().unwrap();
        let mut own = store.create_thread(None, crate::AgentId::ClaudeCode, None, Effort::High, crate::HandHolding::Auto).unwrap();
        own.native_id = Some("s-trek".into());
        store.save_thread(&own).unwrap();
        let found = |id: &str, skip| ImportedThread {
            source: ThreadSource::ClaudeCode,
            native_id: id.into(),
            title: "t".into(),
            cwd: None,
            branch: None,
            model: None,
            effort: None,
            created_at: 1,
            updated_at: 2,
            additions: 0,
            deletions: 0,
            skip,
            legacy_title: None,
        };
        let summary = import_found(&store, vec![found("s-trek", None), found("s-user", None), found("s-tmp", Some(Skip::TempDir))]);
        assert_eq!(summary.claude_code, 1);
        assert_eq!(summary.new_threads, 1);
        assert_eq!(summary.skipped, BTreeMap::from([(Skip::Trek, 1), (Skip::TempDir, 1)]));
        assert_eq!(store.threads().unwrap().len(), 2);
    }

    #[test]
    fn turns_get_a_footer_when_they_finish() {
        let mut t = Transcript::default();
        t.user("hi".into(), Some(1_000));
        t.push(Item::Assistant { text: "hello".into() });
        t.activity(Some(2_000));
        t.complete(Some(4_500));
        t.user("research this".into(), Some(10_000));
        t.push(Item::Assistant { text: "started a background task".into() });
        t.complete(Some(12_000));
        // The task reports back and the agent carries on: still the same reply.
        t.push(Item::Assistant { text: "the task finished".into() });
        t.activity(Some(70_000));
        t.complete(Some(71_000));
        t.user("now this".into(), Some(100_000));
        t.push(Item::Assistant { text: "partial".into() });
        t.interrupt();
        t.user("again".into(), Some(200_000));
        t.push(Item::Tool { id: "1".into(), title: "Ran command".into(), detail: String::new(), output: String::new(), status: crate::store::ToolStatus::Done });
        t.complete(Some(201_000));
        t.user("still there?".into(), Some(300_000));
        t.push(Item::Assistant { text: "yes".into() });
        t.activity(Some(301_000));
        let items = t.finish();
        let ends: Vec<_> = items.iter().filter_map(|i| if let Item::TurnEnd { at, took_secs } = i { Some((*at, *took_secs)) } else { None }).collect();
        // Interrupted replies, ones that ended on a tool call and ones still going get none.
        assert_eq!(ends, vec![(4_500, 3), (71_000, 61)]);
        assert!(matches!(items[2], Item::TurnEnd { .. }));
    }
}
