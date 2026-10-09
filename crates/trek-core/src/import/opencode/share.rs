//! One `opencode.db` for OpenCode 1.x and 2.
//!
//! 1.x opens a database only when it's empty or has 1.x's `session` table. One 1.x made and 2.0
//! opened later works for both: 2.0 adds its tables beside 1.x's, copies 1.x's sessions into
//! them once (`migration.v1-v2` in its `kv` table records that it has), and leaves 1.x's
//! `session`, `message`, `part`, `todo` and `session_share` as they were. A database 2.0 made
//! has none of those five, so 1.x refuses it. `share` adds them as 1.x itself writes them (read
//! from a scratch database 1.x was asked to create), which makes it the first kind. Neither
//! version sees the sessions the other starts later: 2.0 copies 1.x's only when it migrates,
//! and 1.x never reads 2.0's.
//!
//! A copy of the database is made beside it first. Nothing is changed when a table 1.x needs
//! is there in another shape, or 1.x has schema changes the database's journal doesn't list
//! (1.x would make them again on start): 1.x keeps `opencode-1x.db` of its own then.

use super::{DB_1X, MAIN_DB};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 1.x's tables that 2.0 leaves in a database it migrates and doesn't create in its own.
const TABLES_1X: [&str; 5] = ["session", "message", "part", "todo", "session_share"];

/// 2.0's `kv` key for its copy of 1.x's sessions.
const COPIED: &str = "migration.v1-v2";

/// How long a write waits for OpenCode (2.0's background server may have the file open).
const BUSY: Duration = Duration::from_secs(10);

/// What OpenCode 1.x makes of `opencode.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// It opens it: 1.x's tables are there, there's nothing in it yet, or there's no file (1.x
    /// creates one). Also when it can't be read: that's left to 1.x to report.
    Opens,
    /// OpenCode 2 made it: 1.x refuses it.
    Made2,
}

pub fn state(dir: &Path) -> State {
    // Read-only: a missing file isn't created.
    let Some(conn) = super::open(&dir.join(MAIN_DB)) else { return State::Opens };
    match names(&conn, "main", "table") {
        Ok(tables) if !tables.is_empty() && !tables.contains("session") => State::Made2,
        _ => State::Opens,
    }
}

/// Where 1.x keeps its sessions when `opencode.db` can't be shared.
pub fn own_db(dir: &Path) -> PathBuf {
    dir.join(DB_1X)
}

/// Why `share` left the database alone.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// It isn't one 1.x's tables can be added to; that won't change while the versions don't.
    Shape(String),
    /// It couldn't be done this time (the file busy, the disk full…).
    Failed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Shape(why) | Refused::Failed(why) => f.write_str(why),
        }
    }
}

/// What a change to `opencode.db` did.
#[derive(Debug, PartialEq)]
pub struct Changed {
    /// The copy made before it.
    pub backup: PathBuf,
    /// Sessions moved in from `opencode-1x.db`.
    pub merged: usize,
}

fn failed(e: impl std::fmt::Display) -> Refused {
    Refused::Failed(e.to_string())
}

/// Add 1.x's tables to the `opencode.db` in `dir` that OpenCode 2 made, as they are in
/// `scratch` (an empty database 1.x created). With `merge`, `opencode-1x.db`'s sessions move in
/// too (see `merge_own`). `None`: there was nothing to do (1.x opens it already).
pub fn share(dir: &Path, scratch: &Path, merge: bool) -> Result<Option<Changed>, Refused> {
    let mut conn = open_rw(&dir.join(MAIN_DB)).map_err(failed)?;
    let one_x = Connection::open_with_flags(scratch, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(failed)?;
    if plan(&conn, &one_x)?.is_none() {
        return Ok(None);
    }
    let backup = backup(&conn, dir).map_err(failed)?;
    {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
        // Again now that it's held: another process may have got there first.
        let Some(statements) = plan(&tx, &one_x)? else { return Ok(None) };
        for sql in &statements {
            tx.execute_batch(sql).map_err(failed)?;
        }
        note_copy(&tx).map_err(failed)?;
        tx.commit().map_err(failed)?;
    }
    // Shared now, whatever becomes of the sessions 1.x kept on its own.
    let own = dir.join(DB_1X);
    let merged = match merge && own.is_file() {
        true => merge_into(conn, &own).unwrap_or_else(|e| {
            tracing::warn!("opencode: couldn't move opencode-1x.db's sessions into opencode.db: {e}");
            0
        }),
        false => 0,
    };
    Ok(Some(Changed { backup, merged }))
}

/// Move the sessions in `opencode-1x.db` (where Trek had 1.x keep them while it couldn't open
/// `opencode.db`) into `opencode.db`, once that has 1.x's tables, and set the file aside as
/// `opencode-1x.db.merged-<time>`. Rows already there stay as they are. `None`: there's no such
/// file, `opencode.db` isn't ready for it, or there was nothing to move.
pub fn merge_own(dir: &Path) -> Result<Option<Changed>, String> {
    let (main, own) = (dir.join(MAIN_DB), dir.join(DB_1X));
    if !own.is_file() || !main.is_file() {
        return Ok(None);
    }
    let err = |e: rusqlite::Error| e.to_string();
    let conn = open_rw(&main).map_err(err)?;
    if !names(&conn, "main", "table").map_err(err)?.contains("session") {
        return Ok(None);
    }
    // Nothing to move (it has nothing, or it's all there already): no copy needed.
    conn.execute("ATTACH DATABASE ?1 AS own", [own.to_string_lossy()]).map_err(err)?;
    let new = match names(&conn, "own", "table").map_err(err)?.contains("session") {
        true => conn.query_row("SELECT COUNT(*) FROM own.session WHERE id NOT IN (SELECT id FROM main.session)", [], |r| r.get::<_, i64>(0)).map_err(err)?,
        false => 0,
    };
    conn.execute_batch("DETACH DATABASE own").map_err(err)?;
    if new == 0 {
        drop(conn);
        set_aside(&own);
        return Ok(None);
    }
    let backup = backup(&conn, dir).map_err(err)?;
    let merged = merge_into(conn, &own).map_err(err)?;
    Ok(Some(Changed { backup, merged }))
}

/// Copy `own`'s sessions into `conn`'s database in one go, then set `own` aside.
fn merge_into(mut conn: Connection, own: &Path) -> rusqlite::Result<usize> {
    conn.execute("ATTACH DATABASE ?1 AS own", [own.to_string_lossy()])?;
    let merged: rusqlite::Result<usize> = (|| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = copy_rows(&tx)?;
        tx.commit()?;
        Ok(n)
    })();
    let _ = conn.execute_batch("DETACH DATABASE own");
    drop(conn);
    let merged = merged?;
    set_aside(own);
    Ok(merged)
}

fn open_rw(path: &Path) -> rusqlite::Result<Connection> {
    // Without CREATE: a missing file is an error, not a new empty database.
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(BUSY)?;
    Ok(conn)
}

/// The names of `schema`'s tables or indexes.
fn names(conn: &Connection, schema: &str, kind: &str) -> rusqlite::Result<HashSet<String>> {
    let sql = format!("SELECT name FROM \"{schema}\".sqlite_master WHERE type = ?1 AND name NOT LIKE 'sqlite_%'");
    conn.prepare(&sql)?.query_map([kind], |r| r.get(0))?.collect()
}

/// A table's columns as SQLite has them: name, type, not null, primary key.
fn shape(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<(String, String, bool, i64)>> {
    conn.prepare("SELECT name, upper(type), \"notnull\", pk FROM pragma_table_info(?1, ?2) ORDER BY cid")?
        .query_map([table, schema], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect()
}

fn columns(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<String>> {
    Ok(shape(conn, schema, table)?.into_iter().map(|c| c.0).collect())
}

/// The statements that add what 1.x needs to `conn`, as `one_x` has them. `None`: 1.x opens it
/// as it is.
fn plan(conn: &Connection, one_x: &Connection) -> Result<Option<Vec<String>>, Refused> {
    let tables = names(conn, "main", "table").map_err(failed)?;
    if tables.is_empty() || tables.contains("session") {
        return Ok(None);
    }
    if !tables.contains("migration") {
        return Err(Refused::Shape("opencode.db has no migration journal, so it isn't one OpenCode made".into()));
    }
    let indexes = names(conn, "main", "index").map_err(failed)?;
    let mut out = vec![];
    for table in TABLES_1X {
        let sql: Option<String> = one_x
            .query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))
            .optional()
            .map_err(failed)?;
        let Some(sql) = sql else {
            return Err(Refused::Shape(format!("this OpenCode 1.x has no `{table}` table, so it isn't the 1.x Trek knows")));
        };
        if tables.contains(table) {
            if shape(conn, "main", table).map_err(failed)? != shape(one_x, "main", table).map_err(failed)? {
                return Err(Refused::Shape(format!("opencode.db has a `{table}` table that isn't 1.x's")));
            }
            continue;
        }
        out.push(sql);
        let mut st = one_x.prepare("SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL").map_err(failed)?;
        let made: Vec<(String, String)> = st.query_map([table], |r| Ok((r.get(0)?, r.get(1)?))).and_then(|rows| rows.collect()).map_err(failed)?;
        for (name, sql) in made {
            if indexes.contains(&name) {
                return Err(Refused::Shape(format!("opencode.db has an index `{name}` of its own, which 1.x's `{table}` needs")));
            }
            out.push(sql);
        }
        // What it refers to must be there.
        let mut st = one_x.prepare("SELECT DISTINCT \"table\" FROM pragma_foreign_key_list(?1)").map_err(failed)?;
        let targets: Vec<String> = st.query_map([table], |r| r.get(0)).and_then(|rows| rows.collect()).map_err(failed)?;
        if let Some(t) = targets.iter().find(|t| !TABLES_1X.contains(&t.as_str()) && !tables.contains(*t)) {
            return Err(Refused::Shape(format!("opencode.db has no `{t}` table, which 1.x's `{table}` refers to")));
        }
    }
    // 1.x makes the changes its journal lacks on start; ones already in what's copied would fail.
    let journal = |c: &Connection| -> rusqlite::Result<HashSet<String>> { c.prepare("SELECT id FROM migration")?.query_map([], |r| r.get(0))?.collect() };
    let theirs = journal(one_x).map_err(|_| Refused::Shape("this OpenCode 1.x keeps no migration journal Trek can read".into()))?;
    let ours = journal(conn).map_err(failed)?;
    let mut missing: Vec<&String> = theirs.difference(&ours).collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(Refused::Shape(format!(
            "OpenCode 1.x has schema changes opencode.db's journal doesn't list ({})",
            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(Some(out))
}

/// With 1.x's tables there, 2.0 would copy their sessions on its next start, and first clear its
/// `event` table (as it does for a 1.x database it hasn't seen). Telling it the copy is under
/// way leaves the events: it copies the sessions there by then and notes it's done.
fn note_copy(conn: &Connection) -> rusqlite::Result<()> {
    let cols = columns(conn, "main", "kv")?;
    if !["key", "value", "time_created", "time_updated"].iter().all(|c| cols.iter().any(|k| k == c)) {
        return Ok(());
    }
    let now = crate::store::now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO kv (key, value, time_created, time_updated) VALUES (?1, ?2, ?3, ?3)",
        rusqlite::params![COPIED, r#"{"phase":"sessions"}"#, now],
    )?;
    Ok(())
}

/// Copy the attached `own` database's 1.x sessions, with what belongs to them, into `main`.
/// Rows already there are kept. Returns how many sessions came over.
fn copy_rows(conn: &Connection) -> rusqlite::Result<usize> {
    let theirs = names(conn, "own", "table")?;
    if !theirs.contains("session") {
        return Ok(0);
    }
    let ours = names(conn, "main", "table")?;
    let of_sessions = "IN (SELECT id FROM own.session)";
    let of_projects = "IN (SELECT project_id FROM own.session)";
    let tables = [
        ("project", format!("id {of_projects}")),
        ("session", "1".to_string()),
        ("message", "1".to_string()),
        ("part", "1".to_string()),
        ("todo", "1".to_string()),
        ("session_share", "1".to_string()),
        ("permission", format!("project_id {of_projects}")),
        ("event_sequence", format!("aggregate_id {of_sessions}")),
        ("event", format!("aggregate_id {of_sessions}")),
    ];
    let mut sessions = 0;
    for (table, filter) in tables {
        if !theirs.contains(table) || !ours.contains(table) {
            continue;
        }
        let from = columns(conn, "own", table)?;
        let cols: Vec<String> = columns(conn, "main", table)?.into_iter().filter(|c| from.contains(c)).map(|c| format!("\"{c}\"")).collect();
        let cols = cols.join(", ");
        let n = conn.execute(&format!("INSERT OR IGNORE INTO main.\"{table}\" ({cols}) SELECT {cols} FROM own.\"{table}\" WHERE {filter}"), [])?;
        if table == "session" {
            sessions = n;
        }
    }
    Ok(sessions)
}

/// A copy of `conn`'s database as it is now, beside it: `opencode.db.backup-<time>`. To restore
/// it, quit OpenCode, delete `opencode.db-wal` and `-shm`, and copy it over `opencode.db`.
fn backup(conn: &Connection, dir: &Path) -> rusqlite::Result<PathBuf> {
    let path = unused(dir, &format!("{MAIN_DB}.backup-{}", stamp()));
    conn.execute("VACUUM INTO ?1", [path.to_string_lossy()])?;
    Ok(path)
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// `dir/name`, or `name-2`, `name-3`… when that's taken.
fn unused(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name}-{n}"));
        n += 1;
    }
    path
}

/// Rename `opencode-1x.db` (and what SQLite keeps beside it) to `opencode-1x.db.merged-<time>`:
/// kept, not read again.
fn set_aside(own: &Path) {
    let Some(dir) = own.parent() else { return };
    let to = unused(dir, &format!("{DB_1X}.merged-{}", stamp()));
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", own.display()));
        if from.exists()
            && let Err(e) = std::fs::rename(&from, format!("{}{suffix}", to.display()))
        {
            tracing::warn!("opencode: couldn't set {} aside: {e}", from.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::opencode::{dbs_in, scan_all};

    const EMPTY_1X: &str = include_str!("../../../fixtures/opencode-1x-empty.sql");
    const FRESH_2: &str = include_str!("../../../fixtures/opencode-2-fresh.sql");
    /// OpenCode 2's session in `FRESH_2`.
    const SESSION_2: &str = "ses_ede7c1be5ffePy3gmdS6lug9B9";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-opencode-share-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make(path: &Path, sql: &str) {
        Connection::open(path).unwrap().execute_batch(sql).unwrap();
    }

    /// `dir` with OpenCode 2's database, and 1.x's empty one where `share` is told to look.
    fn made_by_2(name: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(name);
        // In WAL mode, as OpenCode keeps it.
        make(&dir.join(MAIN_DB), &format!("PRAGMA journal_mode = WAL; {FRESH_2}"));
        let scratch = dir.join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        make(&scratch.join("1x.db"), EMPTY_1X);
        (dir.clone(), scratch.join("1x.db"))
    }

    /// Every row of `tables`, in order: enough to tell whether any changed.
    fn rows(path: &Path, tables: &[&str]) -> Vec<String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut out = vec![];
        for t in tables {
            let mut st = conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY 1, 2")).unwrap();
            let n = st.column_count();
            let found: Vec<String> = st
                .query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect::<Vec<_>>().join("|")))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            out.extend(found.into_iter().map(|r| format!("{t}: {r}")));
        }
        out
    }

    /// OpenCode 2's tables (and its journal).
    const TABLES_2: [&str; 6] = ["session_v2", "session_message", "project", "migration", "event_sequence", "workspace"];

    fn backups(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".backup-") || n.contains(".merged-")).collect();
        found.sort();
        found
    }

    /// What Trek lists from the OpenCode data folder in `TREK_OPENCODE_DATA`.
    #[test]
    #[ignore = "reads TREK_OPENCODE_DATA"]
    fn list_trek_opencode_data() {
        let dir = PathBuf::from(std::env::var_os("TREK_OPENCODE_DATA").expect("TREK_OPENCODE_DATA"));
        println!("{:?}", state(&dir));
        for t in scan_all(&dbs_in(&dir), 0, &HashSet::new()) {
            println!("{} {:?} {:?} skip={:?}", t.native_id, t.title, t.model, t.skip);
        }
    }

    #[test]
    fn a_database_opencode_2_made_gets_1x_tables() {
        let (dir, scratch) = made_by_2("made-by-2");
        let main = dir.join(MAIN_DB);
        let before = rows(&main, &TABLES_2);
        assert_eq!(state(&dir), State::Made2);
        let done = share(&dir, &scratch, true).unwrap().unwrap();
        assert_eq!(done.merged, 0);
        assert_eq!(state(&dir), State::Opens);
        let conn = Connection::open(&main).unwrap();
        let one_x = Connection::open(&scratch).unwrap();
        // 1.x's tables and indexes, as 1.x makes them.
        for t in TABLES_1X {
            assert_eq!(shape(&conn, "main", t).unwrap(), shape(&one_x, "main", t).unwrap(), "{t}");
        }
        let indexes = names(&conn, "main", "index").unwrap();
        for i in ["session_project_idx", "session_parent_idx", "message_session_time_created_id_idx", "part_session_idx", "todo_session_idx"] {
            assert!(indexes.contains(i), "{i}");
        }
        // OpenCode 2's own are as they were; it's told to copy 1.x's sessions without clearing its events.
        assert_eq!(rows(&main, &TABLES_2), before);
        let copy: String = conn.query_row("SELECT value FROM kv WHERE key = ?1", [COPIED], |r| r.get(0)).unwrap();
        assert_eq!(copy, r#"{"phase":"sessions"}"#);
        drop(conn);
        // Once is enough.
        assert_eq!(share(&dir, &scratch, true).unwrap(), None);
        // The copy from before is OpenCode 2's database as it was: put back, 1.x refuses it again.
        assert_eq!(backups(&dir), [done.backup.file_name().unwrap().to_string_lossy()]);
        std::fs::copy(&done.backup, &main).unwrap();
        assert_eq!(state(&dir), State::Made2);
        assert_eq!(rows(&main, &TABLES_2), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opencode_2_can_have_it_open_meanwhile() {
        let (dir, scratch) = made_by_2("open");
        // OpenCode 2's background server, reading.
        let server = Connection::open(dir.join(MAIN_DB)).unwrap();
        let count = |c: &Connection| c.query_row("SELECT COUNT(*) FROM session_v2", [], |r| r.get::<_, i64>(0)).unwrap();
        server.execute_batch("BEGIN").unwrap();
        assert_eq!(count(&server), 1);
        assert!(share(&dir, &scratch, false).unwrap().is_some());
        server.execute_batch("COMMIT").unwrap();
        // It sees the new tables, and goes on as before.
        assert_eq!(server.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        server.execute("UPDATE session_v2 SET title = 'renamed' WHERE id = ?1", [SESSION_2]).unwrap();
        assert_eq!(count(&server), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_1x_made_is_left_alone() {
        let dir = temp_dir("made-by-1x");
        let scratch = dir.join("1x-scratch.db");
        make(&scratch, EMPTY_1X);
        // Recorded with 1.x and then 2.0: what users who had 1.x first have.
        let main = dir.join(MAIN_DB);
        make(&main, include_str!("../../../fixtures/opencode-1x-then-2.sql"));
        let bytes = std::fs::read(&main).unwrap();
        assert_eq!(state(&dir), State::Opens);
        assert_eq!(share(&dir, &scratch, true).unwrap(), None);
        assert_eq!(merge_own(&dir).unwrap(), None);
        assert_eq!(std::fs::read(&main).unwrap(), bytes);
        assert!(backups(&dir).is_empty());
        // Nothing yet, or nothing in it: 1.x makes its own.
        std::fs::remove_file(&main).unwrap();
        assert_eq!(state(&dir), State::Opens);
        assert!(!main.exists(), "looking doesn't create it");
        make(&main, "");
        assert_eq!(state(&dir), State::Opens);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_table_of_another_shape_is_left_alone() {
        let (dir, scratch) = made_by_2("collision");
        let main = dir.join(MAIN_DB);
        Connection::open(&main).unwrap().execute_batch("CREATE TABLE message (id TEXT PRIMARY KEY, body TEXT)").unwrap();
        let bytes = std::fs::read(&main).unwrap();
        let why = share(&dir, &scratch, true).unwrap_err();
        assert_eq!(why, Refused::Shape("opencode.db has a `message` table that isn't 1.x's".into()));
        assert_eq!(std::fs::read(&main).unwrap(), bytes);
        assert!(backups(&dir).is_empty());
        assert_eq!(state(&dir), State::Made2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_1x_is_left_alone() {
        let (dir, scratch) = made_by_2("journal");
        // 1.x has a schema change OpenCode 2 doesn't know of.
        Connection::open(&scratch).unwrap().execute("INSERT INTO migration VALUES ('20991231000000_later', 0)", []).unwrap();
        let why = share(&dir, &scratch, true).unwrap_err();
        assert_eq!(why, Refused::Shape("OpenCode 1.x has schema changes opencode.db's journal doesn't list (20991231000000_later)".into()));
        // And one with a table missing isn't the 1.x this knows.
        Connection::open(&scratch).unwrap().execute_batch("DELETE FROM migration WHERE id = '20991231000000_later'; DROP TABLE session_share").unwrap();
        assert!(matches!(share(&dir, &scratch, true), Err(Refused::Shape(why)) if why.contains("`session_share`")));
        assert!(backups(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 1.x's own database, as Trek had it keep one, with a session in it.
    fn own_with_a_session(dir: &Path) {
        let own = dir.join(DB_1X);
        make(&own, EMPTY_1X);
        Connection::open(&own)
            .unwrap()
            .execute_batch(
                "INSERT INTO project VALUES ('p1', '/Users/me/app', 'git', NULL, NULL, NULL, NULL, 1, 1, NULL, '[]', NULL);
                 INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
                     VALUES ('ses_1x', 'p1', 'calm-owl', '/Users/me/app', 'Fix GitHub connector', '1.18.35', 1791560000000, 1791560000500);
                 INSERT INTO message VALUES ('msg_1', 'ses_1x', 1791560000000, 1791560000000,
                     '{\"role\":\"user\",\"time\":{\"created\":1791560000000},\"agent\":\"build\",\"model\":{\"providerID\":\"opencode\",\"modelID\":\"big-pickle\"}}');
                 INSERT INTO part VALUES ('prt_1', 'msg_1', 'ses_1x', 1791560000000, 1791560000000, '{\"type\":\"text\",\"text\":\"my github connector keeps failing\"}');
                 INSERT INTO event_sequence VALUES ('ses_1x', 2, NULL);
                 INSERT INTO event VALUES ('evt_1', 'ses_1x', 1, 'session.created.1', '{}');",
            )
            .unwrap();
    }

    #[test]
    fn sessions_1x_kept_on_its_own_move_in() {
        let (dir, scratch) = made_by_2("merge");
        own_with_a_session(&dir);
        let done = share(&dir, &scratch, true).unwrap().unwrap();
        assert_eq!(done.merged, 1);
        // Set aside, not deleted, and not read again.
        assert!(!dir.join(DB_1X).exists());
        assert_eq!(backups(&dir).iter().filter(|n| n.starts_with("opencode-1x.db.merged-")).count(), 1);
        let main = dir.join(MAIN_DB);
        assert_eq!(rows(&main, &["session"]).len(), 1);
        assert_eq!((rows(&main, &["message"]).len(), rows(&main, &["part"]).len(), rows(&main, &["event"]).len()), (1, 1, 1));
        assert!(rows(&main, &["project"]).iter().any(|r| r.contains("\"p1\"")));
        // Trek lists both versions' sessions from the one file.
        let dbs = dbs_in(&dir);
        assert_eq!(dbs.len(), 1);
        let ids: Vec<String> = scan_all(&dbs, 0, &HashSet::new()).into_iter().map(|t| t.native_id).collect();
        assert_eq!(ids, ["ses_1x", SESSION_2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_1x_kept_on_its_own_move_in_later() {
        // Shared while 1.x was still running on its own file: that moves in afterwards.
        let (dir, scratch) = made_by_2("merge-later");
        own_with_a_session(&dir);
        let shared = share(&dir, &scratch, false).unwrap().unwrap();
        assert_eq!(shared.merged, 0);
        assert!(dir.join(DB_1X).exists());
        let main = dir.join(MAIN_DB);
        // 1.x went on in the shared file meanwhile; what it wrote stays.
        Connection::open(&main)
            .unwrap()
            .execute_batch(
                "INSERT INTO project VALUES ('global', '/', NULL, NULL, NULL, NULL, NULL, 5, 5, NULL, '[]', NULL);
                 INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated) VALUES ('ses_later', 'global', 's', '/', 'Later', '1.18.35', 5, 5);",
            )
            .unwrap();
        let done = merge_own(&dir).unwrap().unwrap();
        assert_eq!(done.merged, 1);
        assert_ne!(done.backup, shared.backup);
        assert_eq!(rows(&main, &["session"]).len(), 2);
        assert!(!dir.join(DB_1X).exists());
        // Nothing left to move.
        assert_eq!(merge_own(&dir).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
