//! Full-text search (SQLite FTS5) over thread titles and what was said in them.
//!
//! - Titles and stored messages (yours and the agent's; not tool output or reasoning) are indexed
//!   by triggers, so every write path keeps the index current, older Trek builds' included. Rows
//!   written before the index existed are indexed in the background (`backfill_search`), and so
//!   are big appends such as an imported transcript's first save (`append_unindexed`): tokenizing
//!   megabytes would hold up whoever saved.
//! - Imported threads keep their transcripts in the other agent's files until you continue them
//!   here, so their messages go into a separate index: transcripts up to `INDEX_MAX_BYTES` are
//!   read in the background after each import, bigger ones the first time they're opened in Trek.
//!   Positions there are positions in `import::load_transcript`'s output, which is what Trek shows.
//! - Background work runs in transactions of about `CHUNK_BYTES` of text, so it never holds the
//!   store for long.

use super::{Item, Store};
use crate::types::ThreadSource;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::ops::Range;

/// Bump to rebuild the index from scratch (new tokenizer, new columns).
const VERSION: i64 = 2;

/// Imported transcripts bigger than this are indexed when opened rather than in the background.
pub const INDEX_MAX_BYTES: u64 = 16 << 20;

/// Only the start of a message is indexed: a pasted log can run to megabytes, and that's where
/// it would blow up the index rather than help anyone find it. Characters, as SQL's `substr`.
const MAX_INDEXED_CHARS: usize = 65_536;

/// Text indexed per background transaction (the store is free for everyone else in between).
const CHUNK_BYTES: usize = 256 << 10;

/// Appends with more message text than this are indexed in the background, not by the trigger.
const DEFER_BYTES: usize = 256 << 10;

/// Imported messages taken out of the index per transaction (deleting tokenizes them again).
const DROP_CHUNK: i64 = 32;

const SCHEMA: &str = r#"
DROP TRIGGER IF EXISTS items_search_insert;
DROP TRIGGER IF EXISTS items_search_update;
DROP TRIGGER IF EXISTS items_search_delete;
DROP TRIGGER IF EXISTS threads_search_insert;
DROP TRIGGER IF EXISTS threads_search_update;
DROP TRIGGER IF EXISTS threads_search_delete;
DROP TABLE IF EXISTS item_search;
DROP TABLE IF EXISTS title_search;
DROP TABLE IF EXISTS title_docs;
DROP TABLE IF EXISTS import_search;
DROP TABLE IF EXISTS import_docs;
DROP TABLE IF EXISTS import_indexed;

CREATE VIRTUAL TABLE item_search USING fts5(body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE VIRTUAL TABLE title_search USING fts5(title, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
-- The title index's keys. `threads` has no INTEGER PRIMARY KEY, so its rowids aren't stable:
-- VACUUM may renumber them, and INSERT OR REPLACE (older Trek builds) moves the row.
CREATE TABLE title_docs (pk INTEGER PRIMARY KEY, thread_id TEXT NOT NULL UNIQUE);
CREATE VIRTUAL TABLE import_search USING fts5(body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE TABLE import_docs (pk INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, pos INTEGER NOT NULL);
CREATE INDEX import_docs_thread ON import_docs(thread_id);
-- Which imported transcripts are indexed, as of the thread's updated_at. complete = 0: skipped (too big).
CREATE TABLE import_indexed (thread_id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, complete INTEGER NOT NULL);

-- 'defer_items' is set only inside a transaction that leaves its rows to backfill_search.
CREATE TRIGGER items_search_insert AFTER INSERT ON items
WHEN json_extract(new.data, '$.kind') IN ('user', 'assistant') AND NOT EXISTS (SELECT 1 FROM search_state WHERE key = 'defer_items') BEGIN
  INSERT INTO item_search (rowid, body) VALUES (new.pk, substr(json_extract(new.data, '$.text'), 1, {max}));
END;
CREATE TRIGGER items_search_update AFTER UPDATE OF data ON items BEGIN
  DELETE FROM item_search WHERE rowid = old.pk;
  INSERT INTO item_search (rowid, body)
    SELECT new.pk, substr(json_extract(new.data, '$.text'), 1, {max}) WHERE json_extract(new.data, '$.kind') IN ('user', 'assistant');
END;
CREATE TRIGGER items_search_delete AFTER DELETE ON items BEGIN
  DELETE FROM item_search WHERE rowid = old.pk;
END;

-- No conflict clauses in here: an outer INSERT OR REPLACE would override them.
CREATE TRIGGER threads_search_insert AFTER INSERT ON threads BEGIN
  INSERT INTO title_docs (thread_id) SELECT new.id WHERE NOT EXISTS (SELECT 1 FROM title_docs WHERE thread_id = new.id);
  DELETE FROM title_search WHERE rowid = (SELECT pk FROM title_docs WHERE thread_id = new.id);
  INSERT INTO title_search (rowid, title) SELECT pk, new.title FROM title_docs WHERE thread_id = new.id;
END;
CREATE TRIGGER threads_search_update AFTER UPDATE OF title ON threads WHEN old.title IS NOT new.title BEGIN
  INSERT INTO title_docs (thread_id) SELECT new.id WHERE NOT EXISTS (SELECT 1 FROM title_docs WHERE thread_id = new.id);
  DELETE FROM title_search WHERE rowid = (SELECT pk FROM title_docs WHERE thread_id = new.id);
  INSERT INTO title_search (rowid, title) SELECT pk, new.title FROM title_docs WHERE thread_id = new.id;
END;
CREATE TRIGGER threads_search_delete AFTER DELETE ON threads BEGIN
  DELETE FROM title_search WHERE rowid = (SELECT pk FROM title_docs WHERE thread_id = old.id);
  DELETE FROM title_docs WHERE thread_id = old.id;
  DELETE FROM import_search WHERE rowid IN (SELECT pk FROM import_docs WHERE thread_id = old.id);
  DELETE FROM import_docs WHERE thread_id = old.id;
  DELETE FROM import_indexed WHERE thread_id = old.id;
END;
"#;

/// Create (or rebuild) the index. Cheap: rows that already exist are left to `backfill_search`.
pub(super) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS search_state (key TEXT PRIMARY KEY, value INTEGER NOT NULL)")?;
    let version: Option<i64> = conn.query_row("SELECT value FROM search_state WHERE key = 'version'", [], |r| r.get(0)).optional()?;
    if version == Some(VERSION) {
        return Ok(());
    }
    conn.execute_batch(&SCHEMA.replace("{max}", &MAX_INDEXED_CHARS.to_string()))?;
    // Keys for today's threads (cheap); their titles are indexed with the rest.
    conn.execute_batch("INSERT INTO title_docs (thread_id) SELECT id FROM threads ORDER BY rowid")?;
    // Everything up to today's last row is backfilled; the triggers cover what comes after.
    conn.execute(
        "INSERT OR REPLACE INTO search_state (key, value) VALUES
           ('version', ?1),
           ('titles_from', 0), ('titles_upto', (SELECT COALESCE(MAX(pk), 0) FROM title_docs)),
           ('items_from', 0), ('items_upto', (SELECT COALESCE(MAX(pk), 0) FROM items))",
        [VERSION],
    )?;
    Ok(())
}

fn state(c: &Connection, key: &str) -> rusqlite::Result<i64> {
    Ok(c.query_row("SELECT value FROM search_state WHERE key = ?1", [key], |r| r.get(0)).optional()?.unwrap_or(0))
}

fn set_state(c: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO search_state (key, value) VALUES (?1, ?2)", params![key, value]).map(|_| ())
}

/// Index the next rows of one kind waiting in its backfill range: up to `chunk` of them, or
/// about `CHUNK_BYTES` of text. Returns true while more remain.
fn backfill(c: &Connection, name: &str, select: &str, table: &str, column: &str, chunk: usize) -> rusqlite::Result<bool> {
    let (from, upto) = (state(c, &format!("{name}_from"))?, state(c, &format!("{name}_upto"))?);
    if from >= upto {
        return Ok(false);
    }
    // Delete first: a trigger may have indexed the row already (it changed since).
    let mut delete = c.prepare(&format!("DELETE FROM {table} WHERE rowid = ?1"))?;
    let mut insert = c.prepare(&format!("INSERT INTO {table} (rowid, {column}) VALUES (?1, ?2)"))?;
    let mut select = c.prepare(select)?;
    let mut rows = select.query(params![from, upto, chunk as i64])?;
    let (mut n, mut bytes, mut last) = (0, 0, from);
    while let Some(row) = rows.next()? {
        let (rowid, text): (i64, Option<String>) = (row.get(0)?, row.get(1)?);
        delete.execute([rowid])?;
        insert.execute(params![rowid, text])?;
        (n, bytes, last) = (n + 1, bytes + text.map_or(0, |t| t.len()), rowid);
        if bytes >= CHUNK_BYTES {
            break;
        }
    }
    // Fewer rows than asked for, none held back: the range is done.
    let next = if n < chunk && bytes < CHUNK_BYTES { upto } else { last };
    set_state(c, &format!("{name}_from"), next)?;
    Ok(next < upto)
}

/// Append rows without indexing them now: they join the backfill range instead.
pub(super) fn append_unindexed<'a>(c: &Connection, thread_id: &str, items: impl IntoIterator<Item = (&'a str, &'a Item)>) -> rusqlite::Result<()> {
    set_state(c, "defer_items", 1)?;
    let appended = super::append_rows(c, thread_id, items)?;
    c.execute("DELETE FROM search_state WHERE key = 'defer_items'", [])?;
    if let Some((first, last)) = appended {
        let (from, upto) = (state(c, "items_from")?, state(c, "items_upto")?);
        // A finished backfill restarts just before the new rows; one under way reaches them.
        let from = if from >= upto { first - 1 } else { from.min(first - 1) };
        set_state(c, "items_from", from)?;
        set_state(c, "items_upto", upto.max(last))?;
    }
    Ok(())
}

/// Whether appending these items is big enough to leave their indexing to the background.
pub(super) fn defer_indexing<'a>(items: impl IntoIterator<Item = &'a Item>) -> bool {
    let mut bytes = 0;
    for item in items {
        if let Item::User { text, .. } | Item::Assistant { text } = item {
            bytes += text.len().min(MAX_INDEXED_CHARS * 4);
            if bytes > DEFER_BYTES {
                return true;
            }
        }
    }
    false
}

/// A thread that matched a search.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub thread_id: String,
    pub title: String,
    /// The matching message's position in the transcript; `None` when the title matched.
    pub position: Option<usize>,
    /// The matching message's stable id, when its transcript is stored in Trek (imported
    /// history read from another agent's files has none).
    pub item_id: Option<String>,
    /// The title on one line (title hits) or an excerpt of the message.
    pub snippet: String,
    /// Byte ranges of the matched words inside `snippet`.
    pub ranges: Vec<Range<usize>>,
}

/// The FTS5 query for what someone typed: every word must appear, each as a prefix, so
/// "stad light" finds "stadium lighting". `None` when there's nothing to look for.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

const OPEN: &str = "\u{1}";
const CLOSE: &str = "\u{2}";

/// Text marked up by `highlight()` / `snippet()` → one line of plain text plus match ranges.
/// For message excerpts (`markdown`), emphasis and code ticks are dropped: they read as noise
/// in a one-line excerpt. Titles keep every character.
fn parse_marked(marked: &str, markdown: bool) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(marked.len());
    let mut ranges = vec![];
    let mut start = None;
    let mut chars = marked.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '`' if markdown => {}
            '*' | '_' if markdown && chars.peek() == Some(&ch) => {
                chars.next();
            }
            '\u{1}' => start = Some(out.len()),
            '\u{2}' => {
                if let Some(s) = start.take().filter(|s| *s < out.len()) {
                    ranges.push(s..out.len());
                }
            }
            c if c.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    // Trailing whitespace may have closed a range past the end.
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    for r in &mut ranges {
        r.end = r.end.min(trimmed);
    }
    ranges.retain(|r| r.start < r.end);
    (out, ranges)
}

/// An imported thread whose messages aren't indexed yet, or changed since.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedToIndex {
    pub thread_id: String,
    pub source: ThreadSource,
    pub native_id: String,
    pub updated_at: i64,
}

/// Messages of a transcript worth searching: (position, the indexed start of the text).
fn messages(items: &[Item]) -> impl Iterator<Item = (usize, &str)> {
    items.iter().enumerate().filter_map(|(pos, item)| match item {
        Item::User { text, .. } | Item::Assistant { text } if !text.trim().is_empty() => {
            let end = text.char_indices().nth(MAX_INDEXED_CHARS).map_or(text.len(), |(i, _)| i);
            Some((pos, &text[..end]))
        }
        _ => None,
    })
}

/// Matches ranked per message index before grouping by thread: a common prefix matches most of
/// the index, and only the best of those can make the list.
const RANKED_MATCHES: i64 = 2000;

/// Best message match per thread in one index, best first: (bm25, thread updated_at, rowid,
/// thread id). `sql` selects (rowid, thread_id, score, updated_at) from the index's best
/// matches, `?3` of them. Grouping happens before the limit, so one thread with many matching
/// messages can't crowd out the rest.
fn best_per_thread(c: &Connection, sql: &str, query: &str, limit: i64) -> rusqlite::Result<Vec<(f64, i64, i64, String)>> {
    let sql = format!("WITH m AS MATERIALIZED ({sql}) SELECT rowid, thread_id, MIN(score) AS best, updated_at FROM m GROUP BY thread_id ORDER BY best, updated_at DESC LIMIT ?2");
    let mut st = c.prepare_cached(&sql)?;
    let rows = st.query_map(params![query, limit, RANKED_MATCHES.max(limit * 4)], |r| Ok((r.get(2)?, r.get(3)?, r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// bm25 scores from one index relative to its best match there, best = 1. Scores from two FTS
/// tables aren't comparable as they are: each depends on its own table's size, document lengths
/// and term frequencies.
fn relative_scores(found: &[(f64, i64, i64, String)]) -> Vec<f64> {
    let best = found.iter().map(|f| f.0).fold(0.0, f64::min);
    found.iter().map(|f| if best < 0.0 { f.0 / best } else { 1.0 }).collect()
}

/// The words of `query` worth looking for in messages: one-letter prefixes match most of the
/// index and narrow nothing. `None` unless a word has three characters: with only short
/// prefixes, ranking would score most of the index for hits that tell nobody anything.
fn message_query(query: &str) -> Option<String> {
    let len = |w: &str| w.chars().filter(|c| c.is_alphanumeric()).count();
    if !query.split_whitespace().any(|w| len(w) >= 3) {
        return None;
    }
    fts_query(&query.split_whitespace().filter(|w| len(w) >= 2).collect::<Vec<_>>().join(" "))
}

impl Store {
    /// Index rows waiting in the backfill ranges (written before search existed, or appended in
    /// bulk), one short transaction per call. Returns true while more remain; call it from a
    /// background thread until false.
    pub fn backfill_search(&self, chunk: usize) -> Result<bool> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let titles = backfill(
            &tx,
            "titles",
            "SELECT d.pk, t.title FROM title_docs d JOIN threads t ON t.id = d.thread_id WHERE d.pk > ?1 AND d.pk <= ?2 ORDER BY d.pk LIMIT ?3",
            "title_search",
            "title",
            chunk,
        )?;
        let items = backfill(
            &tx,
            "items",
            &format!(
                "SELECT pk, substr(json_extract(data, '$.text'), 1, {MAX_INDEXED_CHARS}) FROM items
                 WHERE pk > ?1 AND pk <= ?2 AND json_extract(data, '$.kind') IN ('user', 'assistant') ORDER BY pk LIMIT ?3"
            ),
            "item_search",
            "body",
            chunk,
        )?;
        tx.commit()?;
        Ok(titles || items)
    }

    /// Clear out rows and index entries nobody can reach: transcripts of threads that are gone
    /// (older Trek builds left side chats' behind), threads deleted without their delete trigger
    /// firing (an older Trek's INSERT OR REPLACE taking another thread's agent session), and
    /// imported history of threads since continued here (their stored rows are what's searched).
    pub fn prune_search(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch(
                "DELETE FROM items WHERE thread_id NOT IN (SELECT id FROM threads);
                 DELETE FROM title_search WHERE rowid IN (SELECT pk FROM title_docs WHERE thread_id NOT IN (SELECT id FROM threads));
                 DELETE FROM title_docs WHERE thread_id NOT IN (SELECT id FROM threads);
                 DELETE FROM import_indexed WHERE thread_id NOT IN (SELECT id FROM threads);",
            )
        })?;
        let stale: Vec<String> = self.with(|c| {
            let mut st = c.prepare(
                "SELECT thread_id FROM (SELECT DISTINCT thread_id FROM import_docs) d
                 WHERE d.thread_id NOT IN (SELECT id FROM threads) OR EXISTS (SELECT 1 FROM items WHERE items.thread_id = d.thread_id)",
            )?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })?;
        let _one = self.indexer.lock().expect("indexer lock");
        for thread_id in stale {
            self.drop_imported(&thread_id)?;
        }
        Ok(())
    }

    /// Threads matching `query`, archived threads and side chats left out: title matches first
    /// (best first), then message matches, the best one per thread, best first. Each list holds
    /// up to `limit` hits. Messages are searched once a word has three characters (`message_query`).
    /// Runs on the read-only connection, so saves go on meanwhile.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(q) = fts_query(query) else { return Ok(vec![]) };
        let messages = message_query(query);
        let limit = limit.max(1) as i64;
        self.reading(|c| {
            let mut out = Vec::new();
            let mut st = c.prepare_cached(
                "SELECT t.id, t.title, highlight(title_search, 0, ?2, ?3)
                 FROM title_search JOIN title_docs d ON d.pk = title_search.rowid JOIN threads t ON t.id = d.thread_id
                 WHERE title_search MATCH ?1 AND t.archived_at IS NULL AND t.side_of IS NULL
                 ORDER BY bm25(title_search), t.updated_at DESC LIMIT ?4",
            )?;
            let rows = st.query_map(params![q, OPEN, CLOSE, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
            for row in rows {
                let (thread_id, title, marked) = row?;
                let (snippet, ranges) = parse_marked(&marked, false);
                out.push(SearchHit { thread_id, title, position: None, item_id: None, snippet, ranges });
            }
            let Some(q) = messages else { return Ok(out) };

            // Rank first, cheaply, across both message indexes (stored rows; imported history
            // unless the thread has been continued here); a thread's best match wins. Excerpts are
            // cut afterwards, for the winners only.
            let stored = best_per_thread(
                c,
                "SELECT m.rowid AS rowid, i.thread_id AS thread_id, m.score AS score, t.updated_at AS updated_at
                 FROM (SELECT rowid, rank AS score FROM item_search WHERE item_search MATCH ?1 ORDER BY rank LIMIT ?3) m
                 JOIN items i ON i.pk = m.rowid JOIN threads t ON t.id = i.thread_id
                 WHERE t.archived_at IS NULL AND t.side_of IS NULL",
                &q,
                limit,
            )?;
            let imported = best_per_thread(
                c,
                "SELECT m.rowid AS rowid, d.thread_id AS thread_id, m.score AS score, t.updated_at AS updated_at
                 FROM (SELECT rowid, rank AS score FROM import_search WHERE import_search MATCH ?1 ORDER BY rank LIMIT ?3) m
                 JOIN import_docs d ON d.pk = m.rowid JOIN threads t ON t.id = d.thread_id
                 WHERE t.archived_at IS NULL AND t.side_of IS NULL
                   AND NOT EXISTS (SELECT 1 FROM items WHERE items.thread_id = d.thread_id)",
                &q,
                limit,
            )?;
            // (score relative to its index's best, thread updated_at, stored row?, rowid). A
            // thread is in one index or the other.
            let (stored_rel, imported_rel) = (relative_scores(&stored), relative_scores(&imported));
            let mut found: Vec<(f64, i64, bool, i64)> = stored
                .iter()
                .zip(stored_rel)
                .map(|(f, rel)| (rel, f.1, true, f.2))
                .chain(imported.iter().zip(imported_rel).map(|(f, rel)| (rel, f.1, false, f.2)))
                .collect();
            // Higher is better; ties go to the more recent thread.
            found.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
            found.truncate(limit as usize);
            let winners: Vec<(bool, i64)> = found.into_iter().map(|f| (f.2, f.3)).collect();
            let rowids = |stored: bool| format!("[{}]", winners.iter().filter(|w| w.0 == stored).map(|w| w.1.to_string()).collect::<Vec<_>>().join(","));
            let mut hits: std::collections::HashMap<(bool, i64), SearchHit> = std::collections::HashMap::new();
            let mut st = c.prepare_cached(
                "SELECT item_search.rowid, i.thread_id, t.title, i.id,
                        (SELECT COUNT(*) FROM items j WHERE j.thread_id = i.thread_id AND j.seq < i.seq),
                        snippet(item_search, 0, ?3, ?4, '…', 14)
                 FROM item_search JOIN items i ON i.pk = item_search.rowid JOIN threads t ON t.id = i.thread_id
                 WHERE item_search MATCH ?1 AND item_search.rowid IN (SELECT value FROM json_each(?2))",
            )?;
            let rows = st.query_map(params![q, rowids(true), OPEN, CLOSE], |r| {
                let (snippet, ranges) = parse_marked(&r.get::<_, String>(5)?, true);
                let hit = SearchHit { thread_id: r.get(1)?, title: r.get(2)?, item_id: Some(r.get(3)?), position: Some(r.get::<_, i64>(4)? as usize), snippet, ranges };
                Ok(((true, r.get::<_, i64>(0)?), hit))
            })?;
            for row in rows {
                let (key, hit) = row?;
                hits.insert(key, hit);
            }
            let mut st = c.prepare_cached(
                "SELECT import_search.rowid, d.thread_id, t.title, d.pos, snippet(import_search, 0, ?3, ?4, '…', 14)
                 FROM import_search JOIN import_docs d ON d.pk = import_search.rowid JOIN threads t ON t.id = d.thread_id
                 WHERE import_search MATCH ?1 AND import_search.rowid IN (SELECT value FROM json_each(?2))",
            )?;
            let rows = st.query_map(params![q, rowids(false), OPEN, CLOSE], |r| {
                let (snippet, ranges) = parse_marked(&r.get::<_, String>(4)?, true);
                let hit = SearchHit { thread_id: r.get(1)?, title: r.get(2)?, item_id: None, position: Some(r.get::<_, i64>(3)? as usize), snippet, ranges };
                Ok(((false, r.get::<_, i64>(0)?), hit))
            })?;
            for row in rows {
                let (key, hit) = row?;
                hits.insert(key, hit);
            }
            out.extend(winners.iter().filter_map(|w| hits.remove(w)));
            Ok(out)
        })
    }

    /// Imported threads whose messages need (re)indexing, most recent first. Threads continued in
    /// Trek are left out: their stored rows are indexed already.
    pub fn imported_to_index(&self, limit: usize) -> Result<Vec<ImportedToIndex>> {
        self.with(|c| {
            let mut st = c.prepare_cached(
                "SELECT t.id, t.source, t.native_id, t.updated_at FROM threads t LEFT JOIN import_indexed x ON x.thread_id = t.id
                 WHERE t.source <> 'trek' AND t.native_id IS NOT NULL AND t.archived_at IS NULL
                   AND (x.thread_id IS NULL OR x.updated_at < t.updated_at)
                   AND NOT EXISTS (SELECT 1 FROM items WHERE items.thread_id = t.id)
                 ORDER BY t.updated_at DESC LIMIT ?1",
            )?;
            let rows = st.query_map([limit as i64], |r| {
                Ok(ImportedToIndex {
                    thread_id: r.get(0)?,
                    source: ThreadSource::from_key(&r.get::<_, String>(1)?),
                    native_id: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Whether an imported thread's messages are fully indexed as of `updated_at`.
    pub fn imported_indexed(&self, thread_id: &str, updated_at: i64) -> Result<bool> {
        Ok(self.import_state(thread_id)?.is_some_and(|(at, complete)| complete && at >= updated_at))
    }

    fn import_state(&self, thread_id: &str) -> Result<Option<(i64, bool)>> {
        self.with(|c| {
            c.query_row("SELECT updated_at, complete FROM import_indexed WHERE thread_id = ?1", [thread_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
        })
    }

    /// Index an imported thread's messages (`items` as `import::load_transcript` reads them) as of
    /// the thread's `updated_at`. `None` records that the transcript was skipped (too big to read
    /// in the background) and keeps whatever was indexed before. Returns false when that version
    /// was indexed already. Works a chunk per transaction; searches meanwhile may see part of it.
    pub fn index_imported(&self, thread_id: &str, items: Option<&[Item]>, updated_at: i64) -> Result<bool> {
        // One indexer at a time, so chunks for the same thread can't interleave.
        let _one = self.indexer.lock().expect("indexer lock");
        if self.import_state(thread_id)?.is_some_and(|(at, complete)| at >= updated_at && (complete || items.is_none())) {
            return Ok(false);
        }
        if let Some(items) = items {
            self.drop_imported(thread_id)?;
            let mut docs = messages(items).peekable();
            while docs.peek().is_some() {
                self.with(|c| {
                    let tx = c.unchecked_transaction()?;
                    {
                        let mut doc = tx.prepare_cached("INSERT INTO import_docs (thread_id, pos) VALUES (?1, ?2)")?;
                        let mut text = tx.prepare_cached("INSERT INTO import_search (rowid, body) VALUES (?1, ?2)")?;
                        let mut bytes = 0;
                        while bytes < CHUNK_BYTES {
                            let Some((pos, body)) = docs.next() else { break };
                            doc.execute(params![thread_id, pos as i64])?;
                            text.execute(params![tx.last_insert_rowid(), body])?;
                            bytes += body.len();
                        }
                    }
                    tx.commit()
                })?;
            }
        }
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO import_indexed (thread_id, updated_at, complete) VALUES (?1, ?2, ?3)",
                params![thread_id, updated_at, items.is_some()],
            )
        })?;
        Ok(true)
    }

    /// Take a thread's imported messages out of the index, a few per transaction. Callers hold
    /// `indexer`.
    fn drop_imported(&self, thread_id: &str) -> Result<()> {
        loop {
            let dropped = self.with(|c| {
                let tx = c.unchecked_transaction()?;
                let pks: Vec<i64> =
                    tx.prepare_cached("SELECT pk FROM import_docs WHERE thread_id = ?1 LIMIT ?2")?.query_map(params![thread_id, DROP_CHUNK], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                for pk in &pks {
                    tx.execute("DELETE FROM import_search WHERE rowid = ?1", [pk])?;
                    tx.execute("DELETE FROM import_docs WHERE pk = ?1", [pk])?;
                }
                tx.commit()?;
                Ok(pks.len())
            })?;
            if dropped == 0 {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Thread;
    use crate::transcript::Transcript;
    use crate::types::{AgentId, Effort, HandHolding};

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }

    fn thread(s: &Store, title: &str) -> Thread {
        let mut t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        t.title = title.into();
        s.save_thread(&t).unwrap();
        t
    }

    fn marked(hit: &SearchHit) -> Vec<&str> {
        hit.ranges.iter().map(|r| &hit.snippet[r.clone()]).collect()
    }

    fn count(s: &Store, sql: &str) -> i64 {
        s.with(|c| c.query_row(sql, [], |r| r.get(0))).unwrap()
    }

    fn temp_db(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("trek-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("trek.sqlite"), dir)
    }

    #[test]
    fn queries_are_quoted_prefixes() {
        assert_eq!(fts_query("stad light").as_deref(), Some("\"stad\"* \"light\"*"));
        assert_eq!(fts_query(r#"say "hi" NOT -x"#).as_deref(), Some(r#""say"* """hi"""* "NOT"* "-x"*"#));
        assert_eq!(fts_query("  - ** "), None);
    }

    #[test]
    fn marked_text_becomes_one_line_with_ranges() {
        let (text, ranges) = parse_marked("…the **\u{1}stadium\u{2}**\n\n lights `\u{1}glow\u{2}` a*b __x__  ", true);
        assert_eq!(text, "…the stadium lights glow a*b x");
        assert_eq!(ranges.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["stadium", "glow"]);
        // Titles keep every character; only line breaks fold.
        let (text, ranges) = parse_marked("\u{1}Fix\u{2} __init__.py and **kwargs\nin `main`", false);
        assert_eq!(text, "Fix __init__.py and **kwargs in `main`");
        assert_eq!(&text[ranges[0].clone()], "Fix");
    }

    #[test]
    fn finds_titles_then_messages_with_positions() {
        let s = Store::in_memory().unwrap();
        let a = thread(&s, "Fix the stadium lighting");
        let b = thread(&s, "Kit colours");
        let mut tr = Transcript::default();
        tr.push(user("why are the kits pink"));
        tr.push(Item::Tool { id: "t".into(), title: "Ran".into(), detail: "grep stadium".into(), output: "stadium.rs".into(), status: crate::store::ToolStatus::Done });
        tr.push(said("The stadium shader overrides the kit tint."));
        s.save_transcript(&b.id, &mut tr).unwrap();

        let hits = s.search("stadium", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].thread_id.as_str(), hits[0].position), (a.id.as_str(), None));
        assert_eq!(marked(&hits[0]), ["stadium"]);
        // Tool output isn't indexed: the hit is the answer, third in the transcript.
        assert_eq!((hits[1].thread_id.as_str(), hits[1].position), (b.id.as_str(), Some(2)));
        assert_eq!(hits[1].item_id.as_deref(), Some(tr.ids()[2].as_str()));
        assert_eq!(hits[1].title, "Kit colours");
        assert_eq!(marked(&hits[1]), ["stadium"]);
        // Prefixes, any order, diacritics folded.
        assert_eq!(s.search("shad stad", 10).unwrap().len(), 1);
        assert_eq!(s.search("pínk", 10).unwrap()[0].position, Some(0));
        assert!(s.search("nothing-like-this", 10).unwrap().is_empty());
        // One letter: titles only.
        let hits = s.search("k", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_id, b.id);
    }

    #[test]
    fn titles_come_back_as_written() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Fix __init__.py and **kwargs in `main`");
        let hit = &s.search("fix", 5).unwrap()[0];
        assert_eq!((hit.thread_id.as_str(), hit.title.as_str()), (t.id.as_str(), "Fix __init__.py and **kwargs in `main`"));
        assert_eq!(hit.snippet, hit.title);
        assert_eq!(marked(hit), ["Fix"]);
    }

    #[test]
    fn index_follows_inserts_updates_and_deletes() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Thread");
        let mut tr = Transcript::default();
        let ix = tr.push(said("first draft"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert_eq!(s.search("draft", 5).unwrap().len(), 1);
        if let Some(Item::Assistant { text }) = tr.get_mut(ix) {
            *text = "final answer".into();
        }
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert!(s.search("draft", 5).unwrap().is_empty());
        assert_eq!(s.search("final", 5).unwrap().len(), 1);
        tr.truncate(0);
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert!(s.search("final", 5).unwrap().is_empty());
        // A row appended after the last one was deleted reuses its key, and is indexed all the same.
        tr.push(said("second wind"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert_eq!(s.search("wind", 5).unwrap().len(), 1);

        // Renames reindex the title; archived threads and side chats don't show up.
        s.update_thread(&t.id, |t| t.title = "Renamed thread".into()).unwrap();
        assert_eq!(s.search("renamed", 5).unwrap().len(), 1);
        assert!(s.search("thre", 5).unwrap().iter().all(|h| h.position.is_some() || h.title == "Renamed thread"));
        s.update_thread(&t.id, |t| t.archived_at = Some(1)).unwrap();
        assert!(s.search("renamed", 5).unwrap().is_empty());
        let mut side = thread(&s, "Side question");
        side.side_of = Some(t.id.clone());
        s.save_thread(&side).unwrap();
        assert!(s.search("side", 5).unwrap().is_empty());

        // Deleting a thread takes its rows out of the index.
        let gone = thread(&s, "Doomed");
        let mut tr = Transcript::default();
        tr.push(user("ephemeral words"));
        s.save_transcript(&gone.id, &mut tr).unwrap();
        s.delete_thread(&gone.id).unwrap();
        assert!(s.search("ephemeral", 5).unwrap().is_empty());
        assert!(s.search("doomed", 5).unwrap().is_empty());
        assert_eq!(count(&s, "SELECT COUNT(*) FROM title_docs WHERE thread_id NOT IN (SELECT id FROM threads)"), 0);
    }

    #[test]
    fn one_message_hit_per_thread() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Chat");
        let mut tr = Transcript::default();
        tr.push(user("compile error"));
        tr.push(said("the compile error is a missing import"));
        tr.push(user("another compile error"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        let hits = s.search("compile", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].position.is_some());
    }

    #[test]
    fn a_chatty_thread_doesnt_crowd_out_the_others() {
        let s = Store::in_memory().unwrap();
        // Sixty short matches in one thread score better than one longer match anywhere else.
        let chatty = thread(&s, "Chatty");
        let mut tr = Transcript::default();
        for i in 0..60 {
            tr.push(said(&format!("deploy {i}")));
        }
        s.save_transcript(&chatty.id, &mut tr).unwrap();
        for i in 0..10 {
            let t = thread(&s, &format!("Thread {i}"));
            let mut tr = Transcript::default();
            tr.push(user("we should deploy the new build to staging after the tests pass and then check the logs"));
            s.save_transcript(&t.id, &mut tr).unwrap();
        }
        let hits = s.search("deploy", 12).unwrap();
        assert_eq!(hits.len(), 11);
        assert_eq!(hits[0].thread_id, chatty.id);
        assert_eq!(s.search("deploy", 5).unwrap().len(), 5);
    }

    #[test]
    fn only_the_start_of_huge_messages_is_indexed() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Logs");
        let mut log = "noise ".repeat(MAX_INDEXED_CHARS / 6 + 10);
        log.push_str("needle");
        let mut tr = Transcript::default();
        tr.push(user(&format!("start {log}")));
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert_eq!(s.search("start", 5).unwrap().len(), 1);
        assert!(s.search("needle", 5).unwrap().is_empty());
        assert_eq!(count(&s, "SELECT length(body) FROM item_search") as usize, MAX_INDEXED_CHARS);
        assert_eq!(messages(&[said(&log)]).next().unwrap().1.chars().count(), MAX_INDEXED_CHARS);
    }

    #[test]
    fn big_appends_are_indexed_in_the_background() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Imported");
        // An imported transcript saved for the first time: megabytes of history in one go.
        let history: Vec<Item> = (0..40).map(|i| said(&format!("chapter {i} {}", "lorem ipsum ".repeat(1000)))).collect();
        let mut tr = Transcript::unsaved(history);
        assert!(s.save_transcript(&t.id, &mut tr).unwrap());
        assert!(s.search("chapter", 5).unwrap().is_empty());
        // Small saves meanwhile are indexed as they're written.
        tr.push(user("and an epilogue"));
        assert!(!s.save_transcript(&t.id, &mut tr).unwrap());
        assert_eq!(s.search("epilogue", 5).unwrap()[0].position, Some(40));
        // About CHUNK_BYTES a transaction, not everything at once.
        let mut transactions = 1;
        while s.backfill_search(500).unwrap() {
            transactions += 1;
        }
        assert_eq!(transactions, 2);
        let hit = &s.search("chapter", 5).unwrap()[0];
        assert_eq!(hit.item_id.as_deref(), Some(tr.ids()[0].as_str()));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM item_search"), 41);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM search_state WHERE key = 'defer_items'"), 0);
    }

    #[test]
    fn rows_from_before_the_index_are_backfilled() {
        let (path, dir) = temp_db("search");
        let (thread_id, item_id) = {
            let s = Store::open(&path).unwrap();
            let t = thread(&s, "Weather station");
            let mut tr = Transcript::default();
            tr.push(user("barometer drift"));
            s.save_transcript(&t.id, &mut tr).unwrap();
            // Forget the index, as a database from before search would be.
            s.with(|c| c.execute_batch("DELETE FROM item_search; DELETE FROM title_search; DELETE FROM title_docs; DELETE FROM search_state;")).unwrap();
            (t.id, tr.ids()[0].clone())
        };
        let s = Store::open(&path).unwrap();
        assert!(s.search("barometer", 5).unwrap().is_empty());
        assert!(s.search("weather", 5).unwrap().is_empty());
        // Written after the index came back: found at once.
        let mut tr = Transcript::stored(s.items_with_ids(&thread_id).unwrap());
        tr.push(said("barometer recalibrated"));
        s.save_transcript(&thread_id, &mut tr).unwrap();
        assert_eq!(s.search("recalibrated", 5).unwrap().len(), 1);
        while s.backfill_search(1).unwrap() {}
        let hits = s.search("barometer", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_id.as_deref(), Some(item_id.as_str()));
        assert_eq!(s.search("weather", 5).unwrap().len(), 1);
        // Nothing indexed twice.
        assert_eq!(count(&s, "SELECT COUNT(*) FROM item_search"), 2);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn title_hits_survive_renumbered_thread_rows() {
        let s = Store::in_memory().unwrap();
        let a = thread(&s, "Alpha stadium");
        let b = thread(&s, "Beta lights");
        // What VACUUM may do to a table without an INTEGER PRIMARY KEY.
        s.with(|c| c.execute_batch("UPDATE threads SET rowid = rowid + 100 WHERE id = (SELECT id FROM threads ORDER BY rowid LIMIT 1); UPDATE threads SET rowid = 1 WHERE rowid = 2;")).unwrap();
        assert_eq!(s.search("alpha", 5).unwrap()[0].thread_id, a.id);
        assert_eq!(s.search("beta", 5).unwrap()[0].thread_id, b.id);
    }

    #[test]
    fn older_trek_builds_can_still_write() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Alpha stadium");
        // Trek before stable ids saved transcripts and threads like this.
        let data = serde_json::to_string(&said("legacy words")).unwrap();
        s.with(|c| c.execute("INSERT INTO items(thread_id, seq, data, created_at) VALUES (?1, 0, ?2, 1)", params![t.id, data])).unwrap();
        let rows = s.items_with_ids(&t.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(uuid::Uuid::parse_str(&rows[0].0).is_ok());
        assert_eq!(s.search("legacy", 5).unwrap()[0].item_id.as_deref(), Some(rows[0].0.as_str()));
        let cols = Store::THREAD_COLS;
        let renamed: Vec<&str> = cols.split(", ").map(|c| if c == "title" { "'Beta stadium'" } else { c }).collect();
        let replace = format!("INSERT OR REPLACE INTO threads ({cols}) SELECT {} FROM threads WHERE id = ?1", renamed.join(", "));
        for _ in 0..2 {
            s.with(|c| c.execute(&replace, [&t.id])).unwrap();
        }
        let hits = s.search("stadium", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].thread_id.as_str(), hits[0].title.as_str()), (t.id.as_str(), "Beta stadium"));
        assert!(s.search("alpha", 5).unwrap().is_empty());
        assert_eq!(count(&s, "SELECT COUNT(*) FROM title_search"), 1);
    }

    #[test]
    fn unreachable_entries_are_pruned() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Kept");
        let continued = thread(&s, "Continued");
        s.index_imported(&continued.id, Some(&[user("old history")]), 1).unwrap();
        s.index_imported(&t.id, Some(&[user("still imported")]), 1).unwrap();
        let mut tr = Transcript::unsaved(vec![user("old history")]);
        s.save_transcript(&continued.id, &mut tr).unwrap();
        // A thread gone without its delete trigger (an older build's INSERT OR REPLACE).
        let gone = thread(&s, "Orphan");
        s.index_imported(&gone.id, Some(&[user("orphaned words")]), 1).unwrap();
        s.with(|c| c.execute_batch("DROP TRIGGER threads_search_delete")).unwrap();
        s.with(|c| c.execute("DELETE FROM threads WHERE id = ?1", [&gone.id])).unwrap();
        s.prune_search().unwrap();
        assert_eq!(count(&s, "SELECT COUNT(*) FROM title_docs"), 2);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM title_search"), 2);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM import_docs"), 1);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM import_search"), 1);
        assert_eq!(s.search("still", 5).unwrap()[0].thread_id, t.id);
        assert_eq!(s.search("history", 5).unwrap()[0].item_id.as_deref(), Some(tr.ids()[0].as_str()));
    }

    #[test]
    fn deleted_threads_leave_no_rows_behind() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Parent");
        let mut side = thread(&s, "Side chat");
        side.side_of = Some(t.id.clone());
        s.save_thread(&side).unwrap();
        for (id, text) in [(&t.id, "parent words"), (&side.id, "aside words")] {
            let mut tr = Transcript::default();
            tr.push(user(text));
            s.save_transcript(id, &mut tr).unwrap();
            s.add_checkpoint(id, &tr.ids()[0], std::path::Path::new("/tmp/repo"), "abc").unwrap();
        }
        s.delete_thread(&t.id).unwrap();
        assert_eq!(count(&s, "SELECT COUNT(*) FROM items"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM item_search"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM checkpoints"), 0);

        // A thread taking over another's agent session replaces it, transcript and all.
        let mut a = thread(&s, "First");
        a.native_id = Some("sess".into());
        s.save_thread(&a).unwrap();
        let mut tr = Transcript::default();
        tr.push(user("replaced words"));
        s.save_transcript(&a.id, &mut tr).unwrap();
        let mut b = thread(&s, "Second");
        b.native_id = Some("sess".into());
        s.save_thread(&b).unwrap();
        assert_eq!(count(&s, "SELECT COUNT(*) FROM items"), 0);

        // Rows older builds left behind go at the next prune.
        s.with(|c| c.execute("INSERT INTO items (thread_id, seq, data, created_at) VALUES ('gone', 0, ?1, 1)", [serde_json::to_string(&user("stray words")).unwrap()])).unwrap();
        assert_eq!(count(&s, "SELECT COUNT(*) FROM item_search"), 1);
        s.prune_search().unwrap();
        assert_eq!(count(&s, "SELECT COUNT(*) FROM items"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM item_search"), 0);
    }

    #[test]
    fn draft_side_chats_go_at_the_next_launch() {
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Parent");
        let mut kept = thread(&s, "Side chat");
        kept.side_of = Some(t.id.clone());
        s.save_thread(&kept).unwrap();
        let mut draft = thread(&s, "Side chat");
        draft.side_of = Some("draft".into());
        draft.native_id = Some("draft-session".into());
        s.save_thread(&draft).unwrap();
        let mut tr = Transcript::default();
        tr.push(user("draft words"));
        s.save_transcript(&draft.id, &mut tr).unwrap();
        s.add_checkpoint(&draft.id, &tr.ids()[0], std::path::Path::new("/tmp/repo"), "abc").unwrap();
        let gone = s.drop_orphan_side_chats().unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!((gone[0].0.id.as_str(), gone[0].1.len()), (draft.id.as_str(), 1));
        assert!(s.thread(&draft.id).unwrap().is_none());
        assert!(s.thread(&kept.id).unwrap().is_some());
        assert_eq!(count(&s, "SELECT COUNT(*) FROM items"), 0);
        assert!(s.trek_native_ids().unwrap().contains("draft-session"));
    }

    #[test]
    fn short_prefixes_search_titles_only() {
        assert_eq!(message_query("th"), None);
        assert_eq!(message_query("co de"), None);
        assert_eq!(message_query("a fix").as_deref(), Some("\"fix\"*"));
        assert_eq!(message_query("ui the").as_deref(), Some("\"ui\"* \"the\"*"));
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "Chat");
        let mut tr = Transcript::default();
        tr.push(user("the thing about the theme"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        assert!(s.search("th", 5).unwrap().is_empty());
        assert_eq!(s.search("a them", 5).unwrap()[0].position, Some(0));
    }

    #[test]
    fn stored_and_imported_hits_rank_on_the_same_scale() {
        let s = Store::in_memory().unwrap();
        // In the stored index "deploy" is in every message, so bm25 gives it almost no weight
        // there; in the imported one it's rare and weighs a lot. Raw scores would always put
        // imported history first.
        let imported = thread(&s, "Imported");
        s.index_imported(&imported.id, Some(&[user("deploy it"), user("unrelated"), user("other"), user("more"), user("words")]), 1).unwrap();
        let mut stored = thread(&s, "Stored");
        let mut tr = Transcript::default();
        tr.push(user("deploy it"));
        tr.push(said("deploy done"));
        s.save_transcript(&stored.id, &mut tr).unwrap();
        stored.updated_at = imported.updated_at + 1;
        s.save_thread(&stored).unwrap();
        let raw: f64 = s.with(|c| c.query_row("SELECT rank FROM item_search WHERE item_search MATCH '\"deploy\"*' ORDER BY rank LIMIT 1", [], |r| r.get(0))).unwrap();
        let imported_raw: f64 = s.with(|c| c.query_row("SELECT rank FROM import_search WHERE import_search MATCH '\"deploy\"*' ORDER BY rank LIMIT 1", [], |r| r.get(0))).unwrap();
        assert!(imported_raw < raw, "the imported hit scores better raw");
        // Each index's best is as good as the other's; the more recent thread goes first.
        let hits: Vec<String> = s.search("deploy", 5).unwrap().into_iter().filter(|h| h.position.is_some()).map(|h| h.thread_id).collect();
        assert_eq!(hits, [stored.id.clone(), imported.id.clone()]);
    }

    #[test]
    fn searches_run_while_a_save_holds_the_store() {
        let (path, dir) = temp_db("search-reader");
        let s = Store::open(&path).unwrap();
        let t = thread(&s, "Harbour");
        let mut tr = Transcript::default();
        tr.push(user("lighthouse keeper"));
        s.save_transcript(&t.id, &mut tr).unwrap();
        let held = s.conn.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let searcher = s.clone();
        std::thread::spawn(move || {
            let _ = tx.send(searcher.search("lighthouse", 5).map(|h| h.len()).ok());
        });
        let found = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("search waited for the writer");
        assert_eq!(found, Some(1));
        drop(held);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imported_transcripts_are_indexed_by_position_until_continued() {
        let s = Store::in_memory().unwrap();
        let imp = crate::import::ImportedThread {
            source: ThreadSource::Codex,
            native_id: "rollout-1".into(),
            title: "Imported".into(),
            cwd: None,
            branch: None,
            model: None,
            effort: None,
            created_at: 1,
            updated_at: 100,
            additions: 0,
            deletions: 0,
            skip: None,
            legacy_title: None,
        };
        s.upsert_imported(std::slice::from_ref(&imp)).unwrap();
        let todo = s.imported_to_index(10).unwrap();
        assert_eq!(todo.len(), 1);
        let id = todo[0].thread_id.clone();
        assert_eq!((todo[0].source, todo[0].native_id.as_str(), todo[0].updated_at), (ThreadSource::Codex, "rollout-1", 100));

        let items = vec![user("where is the flux capacitor"), Item::Reasoning { text: "flux".into() }, said("The flux capacitor is in src/time.rs")];
        assert!(!s.imported_indexed(&id, 100).unwrap());
        assert!(s.index_imported(&id, Some(&items), 100).unwrap());
        assert!(s.imported_indexed(&id, 100).unwrap());
        assert!(!s.index_imported(&id, Some(&items), 100).unwrap());
        assert!(s.imported_to_index(10).unwrap().is_empty());
        let hits = s.search("capacitor", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].position, hits[0].item_id.as_deref()), (Some(0), None));

        // Newer activity outside Trek: indexed again. Skipped transcripts keep the old index.
        s.upsert_imported(&[crate::import::ImportedThread { updated_at: 200, ..imp.clone() }]).unwrap();
        assert_eq!(s.imported_to_index(10).unwrap().len(), 1);
        assert!(s.index_imported(&id, None, 200).unwrap());
        assert!(!s.imported_indexed(&id, 200).unwrap());
        assert!(s.imported_to_index(10).unwrap().is_empty());
        assert_eq!(s.search("capacitor", 5).unwrap().len(), 1);
        // A full read replaces the old version, many transactions' worth of it.
        let long: Vec<Item> = (0..200).map(|i| said(&format!("hoverboard {i} {}", "x".repeat(4000)))).collect();
        assert!(s.index_imported(&id, Some(&long), 200).unwrap());
        assert_eq!(count(&s, "SELECT COUNT(*) FROM import_docs"), 200);
        assert!(s.search("capacitor", 5).unwrap().is_empty());
        assert!(s.index_imported(&id, Some(&items), 300).unwrap());

        // Continued in Trek: the stored rows take over.
        let mut tr = Transcript::unsaved(items);
        tr.push(user("and the hoverboard?"));
        s.save_transcript(&id, &mut tr).unwrap();
        let hits = s.search("capacitor", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_id.as_deref(), Some(tr.ids()[0].as_str()));
        assert_eq!(s.search("hoverboard", 5).unwrap()[0].position, Some(3));
    }
}
