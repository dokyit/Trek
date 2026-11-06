//! What a restart leaves for the next launch: the turns it cut off (`crate::interrupted`), and
//! messages not sent yet, which otherwise live only in the composer and go with the process.

use super::Store;
use crate::interrupted::Interrupted;
use anyhow::Result;
use rusqlite::params;
use std::path::PathBuf;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS interrupted_turns (
  thread_id TEXT PRIMARY KEY, waiting INTEGER NOT NULL, asked TEXT, at INTEGER NOT NULL, tries INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS unsent_messages (
  key TEXT PRIMARY KEY, text TEXT NOT NULL, images TEXT NOT NULL
);";

/// A message in progress, kept across a restart. `key` says whose: a thread's, or a new thread's
/// in a project (the composer's own key for it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsent {
    pub key: String,
    pub text: String,
    pub images: Vec<PathBuf>,
}

impl Store {
    /// Note a turn cut off now. A thread cut off again keeps its count of tries: that's what stops
    /// one that takes Trek down with it from being resumed at every launch.
    pub fn note_interrupted(
        &self,
        thread: &str,
        waiting: bool,
        asked: Option<&str>,
        at: i64,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO interrupted_turns (thread_id, waiting, asked, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(thread_id) DO UPDATE SET waiting = excluded.waiting, asked = excluded.asked, at = excluded.at",
                params![thread, waiting, asked, at],
            )?;
            Ok(())
        })
    }

    /// The turns cut off and not dealt with yet, oldest first.
    pub fn interrupted_turns(&self) -> Result<Vec<Interrupted>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT thread_id, waiting, asked, at, tries FROM interrupted_turns ORDER BY at, thread_id",
            )?;
            let rows = st.query_map([], |r| {
                Ok(Interrupted {
                    thread: r.get(0)?,
                    waiting: r.get(1)?,
                    asked: r.get(2)?,
                    at: r.get(3)?,
                    tries: r.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    /// A resume of `thread` went out.
    pub fn interrupted_tried(&self, thread: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE interrupted_turns SET tries = tries + 1 WHERE thread_id = ?1",
                params![thread],
            )?;
            Ok(())
        })
    }

    /// `thread`'s cut-off turn is dealt with: resumed and done, or put aside by the user.
    pub fn drop_interrupted(&self, thread: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM interrupted_turns WHERE thread_id = ?1",
                params![thread],
            )?;
            Ok(())
        })
    }

    /// The messages in progress when Trek last saved them.
    pub fn unsent_messages(&self) -> Result<Vec<Unsent>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT key, text, images FROM unsent_messages ORDER BY key")?;
            let rows = st.query_map([], |r| {
                let images: String = r.get(2)?;
                Ok(Unsent {
                    key: r.get(0)?,
                    text: r.get(1)?,
                    images: serde_json::from_str(&images).unwrap_or_default(),
                })
            })?;
            rows.collect()
        })
    }

    /// Keep exactly these messages in progress.
    pub fn save_unsent_messages<'a>(
        &self,
        messages: impl IntoIterator<Item = &'a Unsent>,
    ) -> Result<()> {
        let rows: Vec<(&str, &str, String)> = messages
            .into_iter()
            .map(|m| {
                (
                    m.key.as_str(),
                    m.text.as_str(),
                    serde_json::to_string(&m.images).unwrap_or_else(|_| "[]".into()),
                )
            })
            .collect();
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM unsent_messages", [])?;
            for (key, text, images) in &rows {
                tx.execute(
                    "INSERT INTO unsent_messages (key, text, images) VALUES (?1, ?2, ?3)",
                    params![key, text, images],
                )?;
            }
            tx.commit()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_cut_off_again_keeps_its_count_of_tries() {
        let s = Store::in_memory().unwrap();
        s.note_interrupted("a", false, None, 10).unwrap();
        s.note_interrupted("b", true, Some("Bash rm"), 20).unwrap();
        s.interrupted_tried("a").unwrap();
        s.note_interrupted("a", false, None, 30).unwrap();
        let turns = s.interrupted_turns().unwrap();
        assert_eq!(turns.len(), 2);
        let (b, a) = (&turns[0], &turns[1]);
        assert_eq!(
            (b.thread.as_str(), b.waiting, b.asked.as_deref(), b.tries),
            ("b", true, Some("Bash rm"), 0)
        );
        assert_eq!((a.thread.as_str(), a.at, a.tries), ("a", 30, 1));
        s.drop_interrupted("a").unwrap();
        assert_eq!(s.interrupted_turns().unwrap().len(), 1);
    }

    #[test]
    fn unsent_messages_are_replaced_as_a_set() {
        let s = Store::in_memory().unwrap();
        let one = Unsent {
            key: "thread:a".into(),
            text: "half a thought".into(),
            images: vec![PathBuf::from("/tmp/a.png")],
        };
        s.save_unsent_messages([&one]).unwrap();
        assert_eq!(s.unsent_messages().unwrap(), vec![one]);
        let two = Unsent {
            key: "draft:".into(),
            text: "other".into(),
            images: vec![],
        };
        s.save_unsent_messages([&two]).unwrap();
        assert_eq!(s.unsent_messages().unwrap(), vec![two]);
        s.save_unsent_messages([]).unwrap();
        assert!(s.unsent_messages().unwrap().is_empty());
    }
}
