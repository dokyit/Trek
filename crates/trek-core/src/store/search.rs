//! Full-text search (SQLite FTS5) over thread titles and what was said in them.
//!
//! - Titles and stored messages (yours and the agent's; not tool output or reasoning) are indexed
//!   by triggers, so every write path keeps the index current. Rows written before the index
//!   existed are indexed in the background, a chunk at a time (`backfill_search`).
//! - Imported threads keep their transcripts in the other agent's files until you continue them
//!   here, so their messages go into a separate index: transcripts up to `INDEX_MAX_BYTES` are
//!   read in the background after each import, bigger ones the first time they're opened in Trek.
//!   Positions there are positions in `import::load_transcript`'s output, which is what Trek shows.

use super::{Item, Store};
use crate::types::ThreadSource;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::ops::Range;

/// Bump to rebuild the index from scratch (new tokenizer, new columns).
const VERSION: i64 = 1;

/// Imported transcripts bigger than this are indexed when opened rather than in the background.
pub const INDEX_MAX_BYTES: u64 = 16 << 20;

/// Only the start of a message is indexed: a pasted log can run to megabytes, and that's where
/// it would blow up the index rather than help anyone find it. Characters, as SQL's `substr`.
const MAX_INDEXED_CHARS: usize = 65_536;

const SCHEMA: &str = r#"
DROP TRIGGER IF EXISTS items_search_insert;
DROP TRIGGER IF EXISTS items_search_update;
DROP TRIGGER IF EXISTS items_search_delete;
DROP TRIGGER IF EXISTS threads_search_insert;
DROP TRIGGER IF EXISTS threads_search_update;
DROP TRIGGER IF EXISTS threads_search_delete;
DROP TABLE IF EXISTS item_search;
DROP TABLE IF EXISTS title_search;
DROP TABLE IF EXISTS import_search;
DROP TABLE IF EXISTS import_docs;
DROP TABLE IF EXISTS import_indexed;

CREATE VIRTUAL TABLE item_search USING fts5(body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE VIRTUAL TABLE title_search USING fts5(title, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE VIRTUAL TABLE import_search USING fts5(body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE TABLE import_docs (pk INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, pos INTEGER NOT NULL);
CREATE INDEX import_docs_thread ON import_docs(thread_id);
-- Which imported transcripts are indexed, as of the thread's updated_at. complete = 0: skipped (too big).
CREATE TABLE import_indexed (thread_id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, complete INTEGER NOT NULL);

CREATE TRIGGER items_search_insert AFTER INSERT ON items
WHEN json_extract(new.data, '$.kind') IN ('user', 'assistant') BEGIN
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

CREATE TRIGGER threads_search_insert AFTER INSERT ON threads BEGIN
  INSERT INTO title_search (rowid, title) VALUES (new.rowid, new.title);
END;
CREATE TRIGGER threads_search_update AFTER UPDATE OF title ON threads WHEN old.title IS NOT new.title BEGIN
  DELETE FROM title_search WHERE rowid = old.rowid;
  INSERT INTO title_search (rowid, title) VALUES (new.rowid, new.title);
END;
CREATE TRIGGER threads_search_delete AFTER DELETE ON threads BEGIN
  DELETE FROM title_search WHERE rowid = old.rowid;
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
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(&SCHEMA.replace("{max}", &MAX_INDEXED_CHARS.to_string()))?;
    // Everything up to today's last row is backfilled; the triggers cover what comes after.
    tx.execute(
        "INSERT OR REPLACE INTO search_state (key, value) VALUES
           ('version', ?1),
           ('titles_from', 0), ('titles_upto', (SELECT COALESCE(MAX(rowid), 0) FROM threads)),
           ('items_from', 0), ('items_upto', (SELECT COALESCE(MAX(pk), 0) FROM items))",
        [VERSION],
    )?;
    tx.commit()
}

fn state(c: &Connection, key: &str) -> rusqlite::Result<i64> {
    Ok(c.query_row("SELECT value FROM search_state WHERE key = ?1", [key], |r| r.get(0)).optional()?.unwrap_or(0))
}

fn set_state(c: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO search_state (key, value) VALUES (?1, ?2)", params![key, value]).map(|_| ())
}

/// Index up to `chunk` pre-existing rows of one kind. Returns true while more remain.
fn backfill(c: &Connection, name: &str, select: &str, table: &str, column: &str, chunk: usize) -> rusqlite::Result<bool> {
    let (from, upto) = (state(c, &format!("{name}_from"))?, state(c, &format!("{name}_upto"))?);
    if from >= upto {
        return Ok(false);
    }
    let rows: Vec<(i64, Option<String>)> =
        c.prepare(select)?.query_map(params![from, upto, chunk as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    // Delete first: a trigger may have indexed the row already (it changed since).
    let mut delete = c.prepare(&format!("DELETE FROM {table} WHERE rowid = ?1"))?;
    let mut insert = c.prepare(&format!("INSERT INTO {table} (rowid, {column}) VALUES (?1, ?2)"))?;
    for (rowid, text) in &rows {
        delete.execute([rowid])?;
        insert.execute(params![rowid, text])?;
    }
    let next = if rows.len() < chunk { upto } else { rows.last().map_or(upto, |r| r.0) };
    set_state(c, &format!("{name}_from"), next)?;
    Ok(next < upto)
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
    /// The title (title hits) or an excerpt of the message, on one line.
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
/// Markdown emphasis and code ticks are dropped; they read as noise in a one-line excerpt.
fn parse_marked(marked: &str) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(marked.len());
    let mut ranges = vec![];
    let mut start = None;
    let mut chars = marked.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '`' => {}
            '*' | '_' if chars.peek() == Some(&ch) => {
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

impl Store {
    /// Index rows written before search existed, up to `chunk` of each kind per call (one short
    /// transaction). Returns true while more remain; call it from a background thread until false.
    pub fn backfill_search(&self, chunk: usize) -> Result<bool> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let titles = backfill(&tx, "titles", "SELECT rowid, title FROM threads WHERE rowid > ?1 AND rowid <= ?2 ORDER BY rowid LIMIT ?3", "title_search", "title", chunk)?;
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

    /// Threads matching `query`, archived threads and side chats left out: title matches first
    /// (best first), then message matches, at most one per thread, best first. Each list holds up
    /// to `limit` hits. Messages are searched once a word has two characters: a one-letter prefix
    /// matches most of the index and would hold the database for nothing useful.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(q) = fts_query(query) else { return Ok(vec![]) };
        let messages = query.split_whitespace().any(|w| w.chars().filter(|c| c.is_alphanumeric()).count() >= 2);
        let limit = limit.max(1) as i64;
        self.with(|c| {
            let mut out = Vec::new();
            let mut st = c.prepare_cached(
                "SELECT t.id, highlight(title_search, 0, ?2, ?3) FROM title_search JOIN threads t ON t.rowid = title_search.rowid
                 WHERE title_search MATCH ?1 AND t.archived_at IS NULL AND t.side_of IS NULL
                 ORDER BY bm25(title_search), t.updated_at DESC LIMIT ?4",
            )?;
            let rows = st.query_map(params![q, OPEN, CLOSE, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (thread_id, marked) = row?;
                let (snippet, ranges) = parse_marked(&marked);
                out.push(SearchHit { thread_id, title: snippet.clone(), position: None, item_id: None, snippet, ranges });
            }
            if !messages {
                return Ok(out);
            }

            // Rank first, cheaply, across both message indexes (stored rows; imported history
            // unless the thread has been continued here); a thread's best match wins. Excerpts are
            // cut afterwards, for the winners only.
            let candidates = limit * 4;
            // (bm25, thread updated_at, stored row?, rowid, thread id)
            let mut found: Vec<(f64, i64, bool, i64, String)> = Vec::new();
            for (stored, sql) in [
                (
                    true,
                    "SELECT item_search.rowid, i.thread_id, bm25(item_search), t.updated_at
                     FROM item_search JOIN items i ON i.pk = item_search.rowid JOIN threads t ON t.id = i.thread_id
                     WHERE item_search MATCH ?1 AND t.archived_at IS NULL AND t.side_of IS NULL
                     ORDER BY bm25(item_search) LIMIT ?2",
                ),
                (
                    false,
                    "SELECT import_search.rowid, d.thread_id, bm25(import_search), t.updated_at
                     FROM import_search JOIN import_docs d ON d.pk = import_search.rowid JOIN threads t ON t.id = d.thread_id
                     WHERE import_search MATCH ?1 AND t.archived_at IS NULL AND t.side_of IS NULL
                       AND NOT EXISTS (SELECT 1 FROM items WHERE items.thread_id = d.thread_id)
                     ORDER BY bm25(import_search) LIMIT ?2",
                ),
            ] {
                let mut st = c.prepare_cached(sql)?;
                let rows = st.query_map(params![q, candidates], |r| Ok((r.get::<_, f64>(2)?, r.get::<_, i64>(3)?, stored, r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    found.push(row?);
                }
            }
            // bm25 is lower-is-better; ties go to the more recent thread.
            found.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
            let mut seen = std::collections::HashSet::new();
            let winners: Vec<(bool, i64)> = found.into_iter().filter(|f| seen.insert(f.4.clone())).take(limit as usize).map(|f| (f.2, f.3)).collect();
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
                let (snippet, ranges) = parse_marked(&r.get::<_, String>(5)?);
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
                let (snippet, ranges) = parse_marked(&r.get::<_, String>(4)?);
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

    /// Index an imported thread's messages (`items` as `import::load_transcript` reads them) as of
    /// the thread's `updated_at`. `None` records that the transcript was skipped (too big to read
    /// in the background) and keeps whatever was indexed before. Returns false when that version
    /// was indexed already.
    pub fn index_imported(&self, thread_id: &str, items: Option<&[Item]>, updated_at: i64) -> Result<bool> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let indexed: Option<(i64, bool)> =
            tx.query_row("SELECT updated_at, complete FROM import_indexed WHERE thread_id = ?1", [thread_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        if indexed.is_some_and(|(at, complete)| at >= updated_at && (complete || items.is_none())) {
            return Ok(false);
        }
        if let Some(items) = items {
            tx.execute("DELETE FROM import_search WHERE rowid IN (SELECT pk FROM import_docs WHERE thread_id = ?1)", [thread_id])?;
            tx.execute("DELETE FROM import_docs WHERE thread_id = ?1", [thread_id])?;
            let mut doc = tx.prepare_cached("INSERT INTO import_docs (thread_id, pos) VALUES (?1, ?2)")?;
            let mut text = tx.prepare_cached("INSERT INTO import_search (rowid, body) VALUES (?1, ?2)")?;
            for (pos, body) in messages(items) {
                doc.execute(params![thread_id, pos as i64])?;
                text.execute(params![tx.last_insert_rowid(), body])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO import_indexed (thread_id, updated_at, complete) VALUES (?1, ?2, ?3)",
            params![thread_id, updated_at, items.is_some()],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Thread;
    use crate::transcript::Transcript;
    use crate::types::{AgentId, Effort, HandHolding};

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None }
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

    #[test]
    fn queries_are_quoted_prefixes() {
        assert_eq!(fts_query("stad light").as_deref(), Some("\"stad\"* \"light\"*"));
        assert_eq!(fts_query(r#"say "hi" NOT -x"#).as_deref(), Some(r#""say"* """hi"""* "NOT"* "-x"*"#));
        assert_eq!(fts_query("  - ** "), None);
    }

    #[test]
    fn marked_text_becomes_one_line_with_ranges() {
        let (text, ranges) = parse_marked("…the **\u{1}stadium\u{2}**\n\n lights `\u{1}glow\u{2}` a*b __x__  ");
        assert_eq!(text, "…the stadium lights glow a*b x");
        assert_eq!(ranges.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["stadium", "glow"]);
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

        // Renames reindex the title; archived threads and side chats don't show up.
        s.update_thread(&t.id, |t| t.title = "Renamed thread".into()).unwrap();
        assert_eq!(s.search("renamed", 5).unwrap().len(), 1);
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
        let indexed: i64 = s.with(|c| c.query_row("SELECT length(body) FROM item_search", [], |r| r.get(0))).unwrap();
        assert_eq!(indexed as usize, MAX_INDEXED_CHARS);
        assert_eq!(messages(&[said(&log)]).next().unwrap().1.chars().count(), MAX_INDEXED_CHARS);
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
    fn rows_from_before_the_index_are_backfilled() {
        let dir = std::env::temp_dir().join(format!("trek-search-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trek.sqlite");
        let (thread_id, item_id) = {
            let s = Store::open(&path).unwrap();
            let t = thread(&s, "Weather station");
            let mut tr = Transcript::default();
            tr.push(user("barometer drift"));
            s.save_transcript(&t.id, &mut tr).unwrap();
            // Forget the index, as a database from before search would be.
            s.with(|c| c.execute_batch("DELETE FROM item_search; DELETE FROM title_search; DELETE FROM search_state;")).unwrap();
            (t.id, tr.ids()[0].clone())
        };
        let s = Store::open(&path).unwrap();
        assert!(s.search("barometer", 5).unwrap().is_empty());
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
        let n: i64 = s.with(|c| c.query_row("SELECT COUNT(*) FROM item_search", [], |r| r.get(0))).unwrap();
        assert_eq!(n, 2);
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
        };
        s.upsert_imported(&imp).unwrap();
        let todo = s.imported_to_index(10).unwrap();
        assert_eq!(todo.len(), 1);
        let id = todo[0].thread_id.clone();
        assert_eq!((todo[0].source, todo[0].native_id.as_str(), todo[0].updated_at), (ThreadSource::Codex, "rollout-1", 100));

        let items = vec![user("where is the flux capacitor"), Item::Reasoning { text: "flux".into() }, said("The flux capacitor is in src/time.rs")];
        assert!(s.index_imported(&id, Some(&items), 100).unwrap());
        assert!(!s.index_imported(&id, Some(&items), 100).unwrap());
        assert!(s.imported_to_index(10).unwrap().is_empty());
        let hits = s.search("capacitor", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].position, hits[0].item_id.as_deref()), (Some(0), None));

        // Newer activity outside Trek: indexed again. Skipped transcripts keep the old index.
        s.upsert_imported(&crate::import::ImportedThread { updated_at: 200, ..imp.clone() }).unwrap();
        assert_eq!(s.imported_to_index(10).unwrap().len(), 1);
        assert!(s.index_imported(&id, None, 200).unwrap());
        assert!(s.imported_to_index(10).unwrap().is_empty());
        assert_eq!(s.search("capacitor", 5).unwrap().len(), 1);

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
