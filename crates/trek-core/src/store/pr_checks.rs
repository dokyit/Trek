//! The pull requests threads opened that Trek keeps an eye on, as the app saved them: a thread's
//! transcript isn't read until it's opened, and its row has to say a check failed before that.

use super::Store;
use anyhow::Result;
use rusqlite::params;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pr_checks (
  thread_id TEXT NOT NULL, url TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (thread_id, url)
);";

impl Store {
    /// Every watched pull request: (thread, its url, the app's data).
    pub fn pr_checks(&self) -> Result<Vec<(String, String, String)>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT thread_id, url, data FROM pr_checks")?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Keep what's known about `thread`'s pull request `url`, or forget it with `None`.
    pub fn set_pr_check(&self, thread: &str, url: &str, data: Option<&str>) -> Result<()> {
        self.with(|c| {
            match data {
                Some(d) => c.execute(
                    "INSERT OR REPLACE INTO pr_checks (thread_id, url, data) VALUES (?1, ?2, ?3)",
                    params![thread, url, d],
                )?,
                None => c.execute(
                    "DELETE FROM pr_checks WHERE thread_id = ?1 AND url = ?2",
                    params![thread, url],
                )?,
            };
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentId, Effort, HandHolding};

    #[test]
    fn a_watched_pull_request_is_kept_until_forgotten_or_its_thread_goes() {
        let s = Store::in_memory().unwrap();
        let t = s
            .create_thread(
                None,
                AgentId::ClaudeCode,
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .unwrap();
        s.set_pr_check(&t.id, "https://github.com/a/b/pull/1", Some("{}"))
            .unwrap();
        s.set_pr_check(&t.id, "https://github.com/a/b/pull/2", Some("{\"x\":1}"))
            .unwrap();
        s.set_pr_check(&t.id, "https://github.com/a/b/pull/2", Some("{\"x\":2}"))
            .unwrap();
        let mut all = s.pr_checks().unwrap();
        all.sort();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].2, "{\"x\":2}", "saving again replaces");
        s.set_pr_check(&t.id, "https://github.com/a/b/pull/1", None)
            .unwrap();
        assert_eq!(s.pr_checks().unwrap().len(), 1);
        s.delete_thread(&t.id).unwrap();
        assert!(s.pr_checks().unwrap().is_empty());
    }
}
