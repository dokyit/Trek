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
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// The lines of a JSONL session file, each converted lossily: a line that isn't valid UTF-8
/// doesn't end the read as it does with `BufRead::lines`. Stops at a read error.
pub(crate) fn jsonl_lines(reader: impl BufRead) -> impl Iterator<Item = String> {
    reader.split(b'\n').map_while(Result::ok).map(|mut line| {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        String::from_utf8(line).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    })
}

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
    /// A single-prompt run from the command line: `codex exec` (not the Codex SDK), `opencode run`.
    OneShotRun,
    /// Nothing typed and nothing answered: empty, or only local commands like `/clear`.
    NoUserMessage,
}

impl Skip {
    pub const ALL: [Skip; 7] =
        [Skip::Trek, Skip::Subagent, Skip::UntouchedFork, Skip::TitleGenerator, Skip::TempDir, Skip::OneShotRun, Skip::NoUserMessage];

    pub fn label(self) -> &'static str {
        match self {
            Skip::Trek => "started by Trek",
            Skip::Subagent => "sub-agent",
            Skip::UntouchedFork => "untouched fork",
            Skip::TitleGenerator => "title generator",
            Skip::TempDir => "temp folder run",
            Skip::OneShotRun => "one-shot run",
            Skip::NoUserMessage => "no message",
        }
    }

    /// How the store records why the import archived a thread.
    pub fn key(self) -> &'static str {
        match self {
            Skip::Trek => "trek",
            Skip::Subagent => "subagent",
            Skip::UntouchedFork => "untouched-fork",
            Skip::TitleGenerator => "title-generator",
            Skip::TempDir => "temp-dir",
            Skip::OneShotRun => "one-shot-run",
            Skip::NoUserMessage => "no-message",
        }
    }
}

/// Recorded instead of a rule when a thread was archived because its session was deleted.
pub const DELETED: &str = "deleted";

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
    /// A one-shot command-line run: `codex exec` outside the Codex SDK, or `opencode run`.
    pub cli_run: bool,
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
    } else if e.cli_run && single {
        Some(Skip::OneShotRun)
    } else if e.prompts == Some(0) && !e.replied {
        Some(Skip::NoUserMessage)
    } else {
        None
    }
}

/// System temp folders, where apps run title generators, scratch runs and tests.
pub(crate) fn is_temp_dir(path: &Path) -> bool {
    in_temp_dir(path, &std::env::temp_dir())
}

fn in_temp_dir(path: &Path, tmp: &Path) -> bool {
    ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", "/var/tmp", "/private/var/tmp"]
        .iter()
        .any(|root| path.starts_with(root))
        // An empty or relative TMPDIR would make every path look temporary.
        || (tmp.is_absolute() && tmp.components().count() > 1 && path.starts_with(tmp))
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
    /// Sessions a rule keeps out of the sidebar (never added, or archived by an import), newest
    /// first, so a misjudged one can be brought back. Trek's own sessions aren't listed.
    pub left_out: Vec<ImportedThread>,
}

impl ImportSummary {
    pub fn total(&self) -> usize {
        self.claude_code + self.codex + self.opencode
    }
}

/// Scan all enabled sources and upsert into the store.
pub fn import_all(store: &Store, settings: &crate::settings::Import) -> ImportSummary {
    let min_updated = if settings.max_age_days == 0 {
        0
    } else {
        crate::store::now_ms() - settings.max_age_days as i64 * 86_400_000
    };
    // Threads already in Trek are looked at whatever their age, so imports keep fixing their
    // titles and archiving helpers after they drop out of the "How far back" window.
    let held = |source| {
        store.imported_native_ids(source).unwrap_or_else(|e| {
            tracing::warn!("import: {e}");
            HashSet::new()
        })
    };
    let mut found: Vec<ImportedThread> = Vec::new();
    let mut known = Vec::new();
    if settings.claude_code {
        found.extend(claude::scan(min_updated, &held(ThreadSource::ClaudeCode)));
        known.push((ThreadSource::ClaudeCode, claude::session_ids()));
    }
    if settings.codex {
        found.extend(codex::scan(min_updated, &held(ThreadSource::Codex)));
        known.push((ThreadSource::Codex, codex::session_ids()));
    }
    if settings.opencode {
        found.extend(opencode::scan(min_updated, &held(ThreadSource::OpenCode)));
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
    // Sessions in Trek's worktrees are Trek's too, including ones a thread left behind when it
    // moved to its project folder (with a new session: agents keep sessions per folder).
    let worktrees = crate::worktree::worktrees_dir();
    let in_worktree = |t: &ImportedThread| t.cwd.as_deref().is_some_and(|c| c.starts_with(&worktrees));
    for t in found.iter_mut().filter(|t| t.skip.is_none() && (trek.contains(&t.native_id) || in_worktree(t))) {
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
            for (o, t) in outcomes.iter().zip(found) {
                summary.new_threads += o.added as usize;
                summary.retitled += o.retitled as usize;
                summary.hidden += o.hidden as usize;
                summary.restored += o.restored as usize;
                if o.left_out && t.skip != Some(Skip::Trek) {
                    summary.left_out.push(t);
                }
            }
        }
        Err(e) => tracing::warn!("import: {e}"),
    }
    summary.left_out.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
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

/// Tokens a model used, as an agent's own history records them: when (unix ms), which model,
/// how many.
pub type UsageEntry = (i64, Option<String>, crate::types::TokenUsage);

/// Token usage an imported thread's history records between `from` and `to` (unix ms). Reads
/// the agent's files: run it off the main thread.
pub fn load_usage(source: ThreadSource, native_id: &str, from: i64, to: i64) -> Vec<UsageEntry> {
    match source {
        ThreadSource::ClaudeCode => claude::usage(native_id, from, to),
        ThreadSource::Codex => codex::usage(native_id, from, to),
        ThreadSource::OpenCode => opencode::usage(native_id, from, to),
        _ => vec![],
    }
}

/// Size in bytes of an imported thread's transcript file, when it lives in one.
pub fn transcript_bytes(source: ThreadSource, native_id: &str) -> Option<u64> {
    std::fs::metadata(history_file(source, native_id)?).ok().map(|m| m.len())
}

/// `load_transcript`, from `file` (`history_file`) when the history lives in one.
pub fn load_transcript_from(source: ThreadSource, native_id: &str, file: Option<&std::path::Path>) -> anyhow::Result<Vec<Item>> {
    match (source, file) {
        (ThreadSource::ClaudeCode, Some(f)) => claude::load_file(f),
        (ThreadSource::Codex, Some(f)) => codex::load_rollout(f, native_id),
        _ => load_transcript(source, native_id),
    }
}

/// `load_usage`, from `file` (`history_file`) when the history lives in one.
pub fn load_usage_from(source: ThreadSource, native_id: &str, file: Option<&std::path::Path>, from: i64, to: i64) -> Vec<UsageEntry> {
    match (source, file) {
        (ThreadSource::ClaudeCode, Some(f)) => claude::usage_in(f, from, to),
        (ThreadSource::Codex, Some(f)) => codex::usage_in(f, from, to),
        _ => load_usage(source, native_id, from, to),
    }
}

/// The file an imported thread's history lives in, when it lives in one of its own (Claude
/// Code's sessions, Codex's rollouts; OpenCode keeps all of them in one database).
pub fn history_file(source: ThreadSource, native_id: &str) -> Option<std::path::PathBuf> {
    match source {
        ThreadSource::ClaudeCode => claude::find_session(native_id),
        ThreadSource::Codex => codex::rollout_path(native_id),
        _ => None,
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

/// Names this long that start a message are copies of it, wherever they end: Codex cuts at
/// about 60 characters.
const COPY_CUT: usize = 40;

/// A stored title that is only the start of the first message: what agents call a thread before
/// it's named, cut with or without an ellipsis. Compared with links and spacing flattened.
pub(crate) fn copies_message(title: &str, message: &str) -> bool {
    let trimmed = title.trim();
    let cut = trimmed.trim_end_matches('…').trim_end_matches("...").trim();
    if cut.is_empty() {
        return true;
    }
    // Only a cut copy shows where it was cut: at an ellipsis, mid-word, at about the length
    // agents cut at, or at the end of the message. A short name ending on a word the message
    // happens to start with ("Design", "Fix") is one the user gave it. Nobody types a name over
    // several lines.
    let marked = cut.len() < trimmed.len() || trimmed.contains('\n');
    let copied = |text: String, cut: String| {
        text.strip_prefix(&cut).is_some_and(|rest| {
            marked
                || rest.trim().is_empty()
                || cut.chars().count() >= COPY_CUT
                || (rest.starts_with(char::is_alphanumeric) && cut.ends_with(char::is_alphanumeric))
        })
    };
    let flat = |s: &str| collapse_spaces(&s.lines().map(clean_line).collect::<Vec<_>>().join(" "));
    let typed = user_text(message).unwrap_or_default();
    let (typed_flat, cut_flat) = (flat(&typed), flat(cut));
    copied(collapse_spaces(message), collapse_spaces(cut))
        || copied(collapse_spaces(&typed), collapse_spaces(cut))
        || copied(typed_flat.clone(), cut_flat.clone())
        // Codex names a thread by the message as its app showed it: the request, then the names of
        // the files attached to it, cut off.
        || (cut.len() < trimmed.len() && !typed_flat.is_empty() && cut_flat.starts_with(&typed_flat))
}

/// What the user typed in a message, without the context agents and apps wrap around it;
/// `None` when the whole message is injected (instructions, command output, reminders).
pub(crate) fn user_text(raw: &str) -> Option<String> {
    let unwrapped = codex_request(raw);
    let text = unwrapped.as_str();
    if is_injected(text) {
        return None;
    }
    let text = strip_image_refs(&strip_pasted(text));
    // Codex goal mode is a prefix on the request itself.
    let text = text.strip_prefix("/goal ").unwrap_or(&text).trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Files a Codex desktop message lists above the request (`## name: /path`), as (name, path).
pub(crate) fn codex_attachments(raw: &str) -> Vec<(&str, &str)> {
    let t = raw.trim_start();
    if !(t.starts_with("# Files mentioned by the user") || t.starts_with("# Files pasted by the user")) {
        return vec![];
    }
    let listed = t.split("## My request:").next().unwrap_or_default();
    listed
        .lines()
        .filter_map(|l| {
            let entry = l.strip_prefix("## ")?;
            let at = entry.find(": /")?;
            Some((entry[..at].trim(), entry[at + 2..].trim()))
        })
        .collect()
}

/// A Codex desktop message as the user wrote it: the in-app browser state it attaches dropped,
/// and the request taken from under "My request" when attachments are listed first.
pub(crate) fn codex_request(raw: &str) -> String {
    let text = strip_block(raw, "in-app-browser-context");
    let t = text.trim();
    let wrapped = ["# Files mentioned by the user", "# Files pasted by the user", "## My request:"].iter().any(|p| t.starts_with(p));
    match t.split_once("## My request:") {
        Some((_, request)) if wrapped => request.trim().to_string(),
        // Attachments and nothing typed.
        None if wrapped => String::new(),
        _ => t.to_string(),
    }
}

/// `text` without `<tag …>…</tag>` blocks.
pub(crate) fn strip_block(text: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        rest = rest[i..].find(&close).map_or("", |end| &rest[i + end + close.len()..]);
    }
    out.push_str(rest);
    out
}

/// Tags agents and apps wrap the context they add to the user's side of the conversation in.
const INJECTED_TAGS: &[&str] = &[
    "bash-input",
    "bash-stderr",
    "bash-stdout",
    "codex_internal_context",
    "command-args",
    "command-message",
    "command-name",
    "command-stderr",
    "command-stdout",
    "environment_context",
    "fork-boilerplate",
    "ide_diagnostics",
    "ide_opened_file",
    "ide_selection",
    "image",
    "in-app-browser-context",
    "INSTRUCTIONS",
    "local-command-caveat",
    "local-command-stderr",
    "local-command-stdout",
    "permissions",
    "recommended_plugins",
    "send_user_message_question_reply",
    "subagent_notification",
    "system-reminder",
    "task-notification",
    "turn_aborted",
    "user-prompt-submit-hook",
    "user_instructions",
    "user_shell_command",
];

/// Context injected into the user's side of the conversation: blocks in the tags above, or any
/// one tag wrapping the whole message; instruction files, caveats and resume notes. A message
/// that merely starts with markup (`<div> overflows on mobile`) is the user's, and so is pasted
/// text, even though it comes in a tag.
pub(crate) fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    if t.is_empty() {
        return true;
    }
    if let Some(tag) = t.strip_prefix('<') {
        let tag = tag.strip_prefix('/').unwrap_or(tag);
        let name = &tag[..tag.find(|c: char| !(c.is_ascii_alphanumeric() || "_-:".contains(c))).unwrap_or(tag.len())];
        let opens = !name.is_empty() && tag[name.len()..].starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace());
        return opens && name != "pasted_content" && (INJECTED_TAGS.contains(&name) || t.trim_end().ends_with(&format!("</{name}>")));
    }
    // Instruction files: "# AGENTS.md instructions for /repo", or just "# AGENTS.md instructions".
    let heading = t.lines().next().unwrap_or_default().trim_end();
    let instructions = heading.strip_prefix("# ").and_then(|h| h.split_once(".md instructions")).is_some_and(|(file, rest)| {
        !file.is_empty() && !file.contains(char::is_whitespace) && (rest.is_empty() || rest.starts_with(" for "))
    });
    if instructions {
        return true;
    }
    [
        "Caveat:",
        "This session is being continued from a previous conversation",
        "[Request interrupted by user",
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
    let l = strip_markers(line);
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    // [text](target) → text. A '[' opens a link only when its ']' is followed by '('.
    while let Some(open) = rest.find('[') {
        let inner = &rest[open + 1..];
        let link = inner.find(']').and_then(|close| {
            let target = inner[close + 1..].strip_prefix('(')?;
            Some((close, target.find(')')? + close + 2))
        });
        match link {
            Some((close, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&inner[..close]);
                rest = &inner[end + 1..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = inner;
            }
        }
    }
    out.push_str(rest);
    let out = collapse_spaces(&out.replace("**", "").replace('`', ""));
    // A message sent in quotes: "Reply with exactly: OK"
    match out.strip_prefix('"').and_then(|o| o.strip_suffix('"')) {
        Some(inner) if !inner.contains('"') => inner.trim().to_string(),
        _ => out,
    }
}

/// Leading heading, quote and list markers; only when a space follows, so "-1 is returned"
/// and "#42 crashes" keep their first character.
fn strip_markers(line: &str) -> &str {
    let mut l = line.trim();
    loop {
        let hashes = l.len() - l.trim_start_matches('#').len();
        let rest = if hashes > 0 { &l[hashes..] } else if let Some(r) = l.strip_prefix(['>', '-', '*', '•']) { r } else { return l };
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            return l;
        }
        l = rest.trim_start();
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

/// Builds an imported transcript, closing each finished turn with the footer live turns get:
/// when it finished, and how long it took from what started it. That's the user's message, or,
/// when the agent picks work back up on its own (a background command reporting back, a goal
/// continuing), the moment it woke. As live, a turn that finishes while background sub-agents
/// are still out isn't over: their reports continue it.
#[derive(Default)]
pub(crate) struct Transcript {
    pub items: Vec<Item>,
    start: Option<i64>,
    last: Option<i64>,
    done: bool,
    /// Finished while sub-agents were still out: the reply goes on when they report.
    held: bool,
    /// Background sub-agents launched and not yet reported back.
    background: HashSet<String>,
}

impl Transcript {
    pub fn user(&mut self, text: String, at: Option<i64>) {
        self.user_with(text, vec![], at);
    }

    /// A message with attached images (paths).
    pub fn user_with(&mut self, text: String, images: Vec<String>, at: Option<i64>) {
        self.close();
        // Reports of sub-agents launched before don't hold this reply open: a lost report
        // would otherwise take the footers of every later turn with it.
        self.background.clear();
        self.items.push(Item::User { text, images, at, resume: None, aside: false });
        self.start = at;
        self.last = at;
    }

    /// Where the agent's session stood before the message just added (see `ResumePoint`).
    pub fn resume_from(&mut self, point: Option<crate::store::ResumePoint>) {
        if let Some(Item::User { resume, .. }) = self.items.last_mut() {
            *resume = point;
        }
    }

    /// The user's answer to a question the agent asked mid-turn: shown, and the turn goes on.
    pub fn answer(&mut self, text: String, at: Option<i64>) {
        self.items.push(Item::User { text, images: vec![], at, resume: None, aside: true });
        self.touch(at);
    }

    /// A message the user sent while the agent was working, taken into the running turn: shown,
    /// and the turn goes on, still timed from its start. Not a turn of its own, so not somewhere
    /// to rewind to. Sent after the reply finished, it starts the next turn.
    pub fn steer(&mut self, text: String, images: Vec<String>, at: Option<i64>) {
        if self.done {
            return self.user_with(text, images, at);
        }
        self.items.push(Item::User { text, images, at, resume: None, aside: true });
        self.touch(at);
    }

    /// The agent starts a turn without a message from the user. A new reply, timed from `at`,
    /// unless the last one is waiting for its background sub-agents.
    pub fn wake(&mut self, at: Option<i64>) {
        if self.held {
            return;
        }
        self.close();
        self.start = at;
        self.last = at;
    }

    /// A sub-agent was launched in the background.
    pub fn launched(&mut self, id: &str) {
        self.background.insert(id.to_string());
    }

    /// A background task reported back.
    pub fn reported(&mut self, id: &str) {
        self.background.remove(id);
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
        self.held = !self.background.is_empty();
        self.done = !self.held;
    }

    /// The user stopped the turn: it gets no footer.
    pub fn interrupt(&mut self) {
        self.start = None;
        self.done = false;
        self.held = false;
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
        self.held = false;
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
        // Brackets that aren't links stay; markers need a space after them.
        assert_eq!(title_from("[WIP] fix [docs](https://x.dev/docs) links"), "[WIP] fix docs links");
        assert_eq!(title_from("[a] b [c](d) e"), "[a] b c e");
        assert_eq!(title_from("-1 is returned from parse"), "-1 is returned from parse");
        assert_eq!(title_from("#42 crashes on launch"), "#42 crashes on launch");
        assert_eq!(title_from("## - **Fix** the build"), "Fix the build");
        // The Codex desktop app's in-app browser state isn't part of the request.
        let browser = "\n<in-app-browser-context source=\"ambient-ui-state\">\n# In app browser:\n- Current URL: https://x.dev/\n</in-app-browser-context>\n\n## My request:\nport this page\n";
        assert_eq!(title_from(browser), "port this page");
        let both = format!("# Files mentioned by the user:\n\n## a.png: /tmp/a.png\n{browser}");
        assert_eq!(title_from(&both), "port this page");
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
            // Codex 0.155 and later.
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>",
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
        // Messages that start with markup are the user's.
        for typed in ["<Button onClick={save}> never fires, why?", "<div class=\"card\"> overflows on mobile, fix the CSS", "<section> collapses on Safari, fix it"] {
            assert_eq!(user_text(typed).as_deref(), Some(typed));
        }
        // A tag wrapping the whole message is context, known or not.
        assert!(is_injected("<new_context_kind>\nsomething\n</new_context_kind>"));
        assert!(is_injected("</image>"));
        assert!(!is_injected("# Task: write the AGENTS.md instructions for the repo"));
    }

    #[test]
    fn stored_titles_that_copy_the_first_message_are_not_names() {
        let first = "I've gotten Players accept contract to work(playing role and contract), I also have both";
        assert!(copies_message("I've gotten Players accept contract to work(playing role an…", first));
        assert!(copies_message("I've gotten Players accept contract to work(playing role an", first));
        assert!(!copies_message("Load shortlists from game memory", first));
        let links = "[https://macapp.supply/](https://macapp.supply/)  [https://www.raycast.com/blog/a-technical-look](https://www.raycast.com/blog/a-technical-look)";
        assert!(copies_message("https://macapp.supply/ https://www.raycast.com/blog/a-techn…", links));
        let attached = "# Files mentioned by the user:\n\n## shot.png: /var/folders/x/shot.png\n\n## My request:\nI really don't like two things, (how codex is called/identified)";
        assert!(copies_message("I really don't like two things, (how codex is called/identi…", attached));
        assert!(copies_message("# Files mentioned by the user:\n\n## shot.png", attached));
        assert!(!copies_message("Review storage cleanup options", attached));
        // The request followed by the names of attached files, cut off.
        let pasted = "# Files pasted by the user:\n\n## \"--- Translated Report ---\": /tmp/pasted.txt\n\n## My request:\nfm crashed, when I launched it";
        assert!(copies_message("fm crashed, when I launched it Translated R…", pasted));
        // A name that only starts like the request is a name.
        assert!(!copies_message("fm crashed, when I launched it: Steam overlay", pasted));
        // So is a short one the request happens to start with.
        let request = "Design the settings page so it reads like the rest of the app";
        assert!(!copies_message("Design", request));
        assert!(!copies_message("Design the settings page", request));
        assert!(copies_message("Design the settings page so it reads like the", request));
        assert!(copies_message("Design the sett", request));
        assert!(copies_message("hi", "hi"));
    }

    #[test]
    fn temp_folders_need_a_real_temp_dir() {
        let home = Path::new("/Users/me/code");
        assert!(!in_temp_dir(home, Path::new("")));
        assert!(!in_temp_dir(home, Path::new("tmp")));
        assert!(!in_temp_dir(home, Path::new("/")));
        assert!(in_temp_dir(Path::new("/Users/me/scratch/run"), Path::new("/Users/me/scratch")));
        assert!(in_temp_dir(Path::new("/private/tmp/x"), Path::new("")));
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
        // One-shot command-line runs.
        assert_eq!(e(&|e| { e.cli_run = true; e.prompts = Some(1) }), Some(Skip::OneShotRun));
        assert_eq!(e(&|e| e.cli_run = true), None);
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
        // A session a thread left in its worktree (it runs in the project folder now).
        let left = ImportedThread { cwd: Some(crate::worktree::worktrees_dir().join("repo/fix-it")), ..found("s-left", None) };
        let summary = import_found(&store, vec![found("s-trek", None), found("s-user", None), found("s-tmp", Some(Skip::TempDir)), left]);
        assert_eq!(summary.claude_code, 1);
        assert_eq!(summary.new_threads, 1);
        assert_eq!(summary.skipped, BTreeMap::from([(Skip::Trek, 2), (Skip::TempDir, 1)]));
        assert_eq!(store.threads().unwrap().len(), 2);
        // Left-out sessions are listed for bringing back; Trek's own are in the sidebar already.
        assert_eq!(summary.left_out.iter().map(|t| t.native_id.as_str()).collect::<Vec<_>>(), ["s-tmp"]);
        store.keep_imported(&summary.left_out[0]).unwrap();
        let again = import_found(&store, vec![found("s-trek", None), found("s-user", None), found("s-tmp", Some(Skip::TempDir))]);
        assert!(again.left_out.is_empty());
        assert_eq!(store.threads().unwrap().len(), 3);

        // A deleted thread's session (and its side chats') stays in the agent's history; the
        // next import mustn't bring the conversation back.
        let mut side = store.create_thread(None, crate::AgentId::ClaudeCode, None, Effort::High, crate::HandHolding::Auto).unwrap();
        side.side_of = Some(own.id.clone());
        side.native_id = Some("s-side".into());
        store.save_thread(&side).unwrap();
        store.delete_thread(&own.id).unwrap();
        let after = import_found(&store, vec![found("s-trek", None), found("s-side", None), found("s-user", None)]);
        assert_eq!(after.new_threads, 0);
        assert_eq!(after.skipped, BTreeMap::from([(Skip::Trek, 2)]));
        assert_eq!(store.threads().unwrap().len(), 2);
    }

    fn footers(items: &[Item]) -> Vec<(i64, u32)> {
        items.iter().filter_map(|i| if let Item::TurnEnd { at, took_secs } = i { Some((*at, *took_secs)) } else { None }).collect()
    }

    #[test]
    fn turns_get_a_footer_when_they_finish() {
        let mut t = Transcript::default();
        t.user("hi".into(), Some(1_000));
        t.push(Item::Assistant { text: "hello".into() });
        t.activity(Some(2_000));
        t.complete(Some(4_500));
        t.user("research this".into(), Some(10_000));
        t.push(Item::Assistant { text: "started a background command".into() });
        t.complete(Some(12_000));
        // Hours later the command reports back and the agent carries on: a turn of its own,
        // timed from when it woke, as live.
        t.wake(Some(3_600_000));
        t.push(Item::Assistant { text: "the command finished".into() });
        t.activity(Some(3_660_000));
        t.complete(Some(3_661_000));
        t.user("now this".into(), Some(3_700_000));
        t.push(Item::Assistant { text: "partial".into() });
        t.interrupt();
        t.user("again".into(), Some(3_800_000));
        t.push(Item::Tool { id: "1".into(), title: "Ran command".into(), detail: String::new(), output: String::new(), status: crate::store::ToolStatus::Done });
        t.complete(Some(3_801_000));
        t.user("still there?".into(), Some(3_900_000));
        t.push(Item::Assistant { text: "yes".into() });
        t.activity(Some(3_901_000));
        let items = t.finish();
        // Interrupted replies, ones that ended on a tool call and ones still going get none.
        assert_eq!(footers(&items), vec![(4_500, 3), (12_000, 2), (3_661_000, 61)]);
        assert!(matches!(items[2], Item::TurnEnd { .. }));
    }

    #[test]
    fn background_sub_agents_keep_their_turn_open() {
        let mut t = Transcript::default();
        t.user("research both".into(), Some(0));
        t.push(Item::Tool { id: "a1".into(), title: "Ran subagent".into(), detail: String::new(), output: String::new(), status: crate::store::ToolStatus::Done });
        t.launched("a1");
        t.push(Item::Assistant { text: "Two agents are on it".into() });
        t.complete(Some(5_000));
        // The agent wakes on its report: the same reply, timed from the user's message.
        t.reported("a1");
        t.wake(Some(600_000));
        t.push(Item::Assistant { text: "Here's what they found".into() });
        t.complete(Some(660_000));
        // A question answered mid-turn is shown and doesn't restart the clock.
        t.user("pick one".into(), Some(700_000));
        t.push(Item::Assistant { text: "Which matters more?".into() });
        t.answer("speed".into(), Some(710_000));
        t.push(Item::Assistant { text: "Then the first".into() });
        t.complete(Some(730_000));
        let items = t.finish();
        assert_eq!(footers(&items), vec![(660_000, 660), (730_000, 30)]);
        assert!(matches!(&items[items.len() - 3], Item::User { text, at: Some(710_000), .. } if text == "speed"));
        // A report that never comes doesn't hold the user's next turn.
        let mut t = Transcript::default();
        t.user("go".into(), Some(0));
        t.launched("lost");
        t.push(Item::Assistant { text: "launched".into() });
        t.complete(Some(1_000));
        t.user("next".into(), Some(5_000));
        t.push(Item::Assistant { text: "done".into() });
        t.complete(Some(8_000));
        assert_eq!(footers(&t.finish()), vec![(8_000, 3)]);
    }
}
