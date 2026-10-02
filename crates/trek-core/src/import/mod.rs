//! "Pull threads from the machine": index sessions created by other agents so they show up
//! in Trek and can be resumed with the agent that made them. Indexing reads metadata only;
//! transcripts are loaded lazily (Claude Code files can exceed 100 MB).

pub mod claude;
pub mod codex;
pub mod opencode;

use crate::store::{Item, Store};
use crate::types::{Effort, ThreadSource};
use std::path::PathBuf;

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
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportSummary {
    pub claude_code: usize,
    pub codex: usize,
    pub opencode: usize,
    pub new_threads: usize,
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
    let mut summary = ImportSummary::default();
    let mut found: Vec<ImportedThread> = Vec::new();
    if settings.claude_code {
        let v = claude::scan(min_updated);
        summary.claude_code = v.len();
        found.extend(v);
    }
    if settings.codex {
        let v = codex::scan(min_updated);
        summary.codex = v.len();
        found.extend(v);
    }
    if settings.opencode {
        let v = opencode::scan(min_updated);
        summary.opencode = v.len();
        found.extend(v);
    }
    for t in &found {
        match store.upsert_imported(t) {
            Ok(true) => summary.new_threads += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!("import {}: {e}", t.native_id),
        }
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

/// First line of user text, cleaned up for use as a title.
pub fn title_from(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled");
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    t
}

/// Injected context (system reminders, command wrappers) that isn't something the user typed.
pub(crate) fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<')
        || t.starts_with("Caveat:")
        || t.starts_with("This session is being continued from a previous conversation")
        || t.is_empty()
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
