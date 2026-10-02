//! A thread's transcript in memory: every item with its stable row id, plus what changed since the
//! last save, so saving writes only new, changed and removed rows (`Store::save_transcript`).
//!
//! Mutate through these methods, not by index juggling: removing items shifts positions, and the
//! ids are what keeps rows, search hits and views pointing at the right message.

use crate::store::Item;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    items: Vec<Item>,
    ids: Vec<String>,
    /// Ids whose rows aren't in the store yet.
    unsaved: HashSet<String>,
    /// Ids of stored rows whose item changed.
    changed: HashSet<String>,
    /// Ids of stored rows that left the transcript.
    removed: Vec<String>,
}

/// What a save has to write.
#[derive(Debug, Default, PartialEq)]
pub struct Changes<'a> {
    pub removed: &'a [String],
    pub changed: Vec<(&'a str, &'a Item)>,
    /// New rows, in transcript order (they always follow the stored ones).
    pub appended: Vec<(&'a str, &'a Item)>,
}

impl Changes<'_> {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.changed.is_empty() && self.appended.is_empty()
    }
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

impl std::ops::Deref for Transcript {
    type Target = [Item];
    fn deref(&self) -> &[Item] {
        &self.items
    }
}

impl Transcript {
    /// Rows as loaded from the store: nothing to save.
    pub fn stored(rows: Vec<(String, Item)>) -> Self {
        let (ids, items) = rows.into_iter().unzip();
        Transcript { items, ids, ..Default::default() }
    }

    /// Items that aren't in the store (history read from another agent): each gets a fresh id
    /// and is written on the first save.
    pub fn unsaved(items: Vec<Item>) -> Self {
        let ids: Vec<String> = items.iter().map(|_| new_id()).collect();
        Transcript { unsaved: ids.iter().cloned().collect(), items, ids, ..Default::default() }
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn id_at(&self, ix: usize) -> Option<&str> {
        self.ids.get(ix).map(String::as_str)
    }

    /// Where the item with this id is now.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|i| i == id)
    }

    /// Append an item; returns its index.
    pub fn push(&mut self, item: Item) -> usize {
        let id = new_id();
        self.unsaved.insert(id.clone());
        self.ids.push(id);
        self.items.push(item);
        self.items.len() - 1
    }

    fn touch(&mut self, ix: usize) {
        let id = &self.ids[ix];
        if !self.unsaved.contains(id) {
            self.changed.insert(id.clone());
        }
    }

    /// Edit the item at `ix`; its row is rewritten on the next save.
    pub fn get_mut(&mut self, ix: usize) -> Option<&mut Item> {
        if ix >= self.items.len() {
            return None;
        }
        self.touch(ix);
        self.items.get_mut(ix)
    }

    pub fn last_mut(&mut self) -> Option<&mut Item> {
        let ix = self.items.len().checked_sub(1)?;
        self.get_mut(ix)
    }

    /// Edit the last item matching `f` (a tool call by id, say).
    pub fn rfind_mut(&mut self, f: impl Fn(&Item) -> bool) -> Option<&mut Item> {
        let ix = self.items.iter().rposition(f)?;
        self.get_mut(ix)
    }

    fn forget(&mut self, id: String) {
        self.changed.remove(&id);
        if !self.unsaved.remove(&id) {
            self.removed.push(id);
        }
    }

    /// Keep only the items `keep` accepts; the rest are deleted on the next save.
    pub fn retain(&mut self, mut keep: impl FnMut(&Item) -> bool) {
        let mut kept_items = Vec::with_capacity(self.items.len());
        let mut kept_ids = Vec::with_capacity(self.ids.len());
        for (item, id) in std::mem::take(&mut self.items).into_iter().zip(std::mem::take(&mut self.ids)) {
            if keep(&item) {
                kept_items.push(item);
                kept_ids.push(id);
            } else {
                self.forget(id);
            }
        }
        self.items = kept_items;
        self.ids = kept_ids;
    }

    /// Keep the first `len` items.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.items.len() {
            return;
        }
        self.items.truncate(len);
        for id in self.ids.split_off(len) {
            self.forget(id);
        }
    }

    /// Drop everything after the item with this id (it stays). False if there's no such item.
    pub fn truncate_after(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(ix) => {
                self.truncate(ix + 1);
                true
            }
            None => false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.unsaved.is_empty() || !self.changed.is_empty() || !self.removed.is_empty()
    }

    pub fn changes(&self) -> Changes<'_> {
        let mut out = Changes { removed: &self.removed, ..Default::default() };
        for (id, item) in self.ids.iter().zip(&self.items) {
            if self.unsaved.contains(id) {
                out.appended.push((id, item));
            } else if self.changed.contains(id) {
                out.changed.push((id, item));
            }
        }
        out
    }

    /// Everything is in the store now.
    pub fn mark_saved(&mut self) {
        self.unsaved.clear();
        self.changed.clear();
        self.removed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }

    fn appended(t: &Transcript) -> Vec<Item> {
        t.changes().appended.into_iter().map(|(_, i)| i.clone()).collect()
    }

    #[test]
    fn pushes_are_appended_once() {
        let mut t = Transcript::default();
        t.push(text("a"));
        t.push(text("b"));
        assert_eq!(appended(&t), vec![text("a"), text("b")]);
        t.mark_saved();
        assert!(!t.is_dirty());
        t.push(text("c"));
        assert_eq!(appended(&t), vec![text("c")]);
    }

    #[test]
    fn edits_to_stored_rows_are_updates_and_to_new_rows_stay_appends() {
        let mut t = Transcript::stored(vec![("x".into(), text("old"))]);
        let ix = t.push(text("stream"));
        if let Some(Item::Assistant { text }) = t.get_mut(ix) {
            text.push_str("ing");
        }
        if let Some(Item::Assistant { text }) = t.get_mut(0) {
            *text = "new".into();
        }
        let c = t.changes();
        assert_eq!(c.changed, vec![("x", &text("new"))]);
        assert_eq!(c.appended.len(), 1);
        assert_eq!(c.appended[0].1, &text("streaming"));
    }

    #[test]
    fn retain_keeps_ids_aligned_and_records_removed_rows() {
        let mut t = Transcript::stored(vec![
            ("u".into(), Item::User { text: "hi".into(), images: vec![], at: None }),
            ("r".into(), Item::Reasoning { text: " ".into() }),
            ("a".into(), text("answer")),
        ]);
        t.push(Item::Reasoning { text: String::new() });
        let tail = t.push(text("more"));
        let tail_id = t.id_at(tail).unwrap().to_string();
        t.retain(|i| !matches!(i, Item::Reasoning { text } if text.trim().is_empty()));
        assert_eq!(t.len(), 3);
        assert_eq!(t.ids()[..2], ["u".to_string(), "a".to_string()]);
        assert_eq!(t.position(&tail_id), Some(2));
        let c = t.changes();
        // The stored empty row is deleted; the unsaved one simply never gets written.
        assert_eq!(c.removed, ["r".to_string()]);
        assert_eq!(c.appended, vec![(tail_id.as_str(), &text("more"))]);
    }

    #[test]
    fn truncate_after_drops_later_rows() {
        let mut t = Transcript::stored(vec![("a".into(), text("1")), ("b".into(), text("2")), ("c".into(), text("3"))]);
        let _ = t.get_mut(2);
        t.push(text("4"));
        assert!(t.truncate_after("a"));
        assert!(!t.truncate_after("zzz"));
        assert_eq!(t.len(), 1);
        let c = t.changes();
        assert_eq!(c.removed, ["b".to_string(), "c".to_string()]);
        assert!(c.changed.is_empty() && c.appended.is_empty());
    }

    #[test]
    fn unsaved_transcripts_are_written_in_full() {
        let t = Transcript::unsaved(vec![text("1"), text("2")]);
        assert_eq!(t.ids().len(), 2);
        assert_ne!(t.ids()[0], t.ids()[1]);
        assert_eq!(appended(&t), vec![text("1"), text("2")]);
    }
}
