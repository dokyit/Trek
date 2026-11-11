//! Caches bounded by what they hold, not by how many things.
//!
//! A count says nothing about memory: 400 short answers and 400 answers with a file in each are
//! the same 400. `ByteLru` keeps a budget in bytes instead (each entry says what it weighs), lets
//! the least recently used go first, and has a cap for one entry, so a single huge one can't take
//! the whole budget and push everything else out.
//!
//! Nothing goes on `insert`: the cache's owner calls `trim` when it's safe to let go (decoded
//! pictures also have to come out of the GPU's atlas, parsed answers are entities), and says
//! what's `pinned`: in use right now, on screen. Pinned entries stay whatever the budget, so
//! drawing more than the budget at once is slower to settle but never thrashes. An entry over the
//! cap is kept exactly as long as it's pinned: it works, and isn't cached.

use std::collections::HashMap;
use std::hash::Hash;

struct Entry<V> {
    value: V,
    bytes: usize,
    /// When it was last used (`ByteLru::tick`).
    used: u64,
}

pub struct ByteLru<K, V> {
    budget: usize,
    entry_cap: usize,
    bytes: usize,
    tick: u64,
    entries: HashMap<K, Entry<V>>,
}

impl<K: Eq + Hash + Clone, V> ByteLru<K, V> {
    /// A cache of at most `budget` bytes, no entry of more than `entry_cap` kept past its use.
    pub fn new(budget: usize, entry_cap: usize) -> Self {
        Self {
            budget,
            entry_cap,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
        }
    }

    fn touch(&mut self, key: &K) -> Option<&mut Entry<V>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(key)?;
        entry.used = tick;
        Some(entry)
    }

    /// A hit: the entry, now the most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.touch(key).map(|e| &e.value)
    }

    /// `get`, to change it. Its weight is `set_bytes`'s to change.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.touch(key).map(|e| &mut e.value)
    }

    /// The entry, without counting as a use.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// `peek`, to change it.
    pub fn peek_mut(&mut self, key: &K) -> Option<&mut V> {
        self.entries.get_mut(key).map(|e| &mut e.value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Keep `value`, `bytes` big, as the most recently used. What it replaces comes back.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) -> Option<V> {
        self.tick += 1;
        let old = self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                used: self.tick,
            },
        );
        self.bytes += bytes;
        old.map(|old| {
            self.bytes -= old.bytes;
            old.value
        })
    }

    /// An entry grew or shrank (a streaming answer's document).
    pub fn set_bytes(&mut self, key: &K, bytes: usize) {
        if let Some(entry) = self.entries.get_mut(key) {
            self.bytes = self.bytes - entry.bytes + bytes;
            entry.bytes = bytes;
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.bytes -= entry.bytes;
        Some(entry.value)
    }

    /// Let go of what there's no room for, and return it: every entry over the per-entry cap, then
    /// the least recently used until the rest fits the budget. Entries `pinned` says are in use
    /// stay, even past the budget.
    pub fn trim(&mut self, pinned: impl Fn(&K, &V) -> bool) -> Vec<(K, V)> {
        let cap = self.entry_cap;
        let over_cap: usize = (self.entries.values().map(|e| e.bytes))
            .filter(|bytes| *bytes > cap)
            .sum();
        // An entry over the cap isn't the cache's to budget for: it's only there while in use.
        let mut cached = self.bytes - over_cap;
        if over_cap == 0 && cached <= self.budget {
            return Vec::new();
        }
        let mut free: Vec<(bool, u64, K)> = self
            .entries
            .iter()
            .filter(|(k, e)| !pinned(k, &e.value))
            .map(|(k, e)| (e.bytes <= cap, e.used, k.clone()))
            .collect();
        // Those over the cap first, then oldest first.
        free.sort_by_key(|(fits, used, _)| (*fits, *used));
        let mut gone = Vec::new();
        for (fits, _, key) in free {
            if fits && cached <= self.budget {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.bytes;
                if fits {
                    cached -= entry.bytes;
                }
                gone.push((key, entry.value));
            }
        }
        gone
    }

    /// Keep only the entries `keep` says to. The rest come back, in no order.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) -> Vec<(K, V)> {
        let going: Vec<K> = self
            .entries
            .iter()
            .filter(|(k, e)| !keep(k, &e.value))
            .map(|(k, _)| k.clone())
            .collect();
        going
            .into_iter()
            .filter_map(|k| self.remove(&k).map(|v| (k, v)))
            .collect()
    }

    /// Everything, in no order (to release what each entry holds elsewhere).
    pub fn clear(&mut self) -> Vec<(K, V)> {
        self.bytes = 0;
        self.entries.drain().map(|(k, e)| (k, e.value)).collect()
    }

    /// The entries, in no order, without counting as a use.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, e)| (k, &e.value))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|e| &e.value)
    }

    /// The entries to change, in no order, without counting as a use.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.values_mut().map(|e| &mut e.value)
    }

    /// What the entries weigh together.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ByteLru;

    const NONE: fn(&&str, &u32) -> bool = |_, _| false;

    fn keys(gone: Vec<(&'static str, u32)>) -> Vec<&'static str> {
        gone.into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn the_least_recently_used_go_first_and_only_as_many_as_it_takes() {
        let mut lru = ByteLru::new(100, 60);
        lru.insert("a", 1, 40);
        lru.insert("b", 2, 40);
        assert!(lru.trim(NONE).is_empty(), "80 of 100: all of it fits");
        lru.insert("c", 3, 40);
        lru.insert("d", 4, 40);
        assert_eq!(lru.bytes(), 160);
        // 160 of 100: the two oldest go, in the order they were used, and no more than those.
        assert_eq!(keys(lru.trim(NONE)), ["a", "b"]);
        assert_eq!((lru.bytes(), lru.len()), (80, 2));
        assert!(lru.contains(&"c") && lru.contains(&"d"));
        assert!(lru.trim(NONE).is_empty());
    }

    #[test]
    fn a_hit_makes_an_entry_the_most_recent_and_a_peek_does_not() {
        let mut lru = ByteLru::new(100, 60);
        lru.insert("a", 1, 40);
        lru.insert("b", 2, 40);
        lru.insert("c", 3, 40);
        // Read: `a` is now newer than `b` and `c`. Peeked at: `b` is as old as it was.
        assert_eq!(lru.get(&"a"), Some(&1));
        assert_eq!(lru.peek(&"b"), Some(&2));
        assert_eq!(keys(lru.trim(NONE)), ["b"]);
        lru.insert("d", 4, 40);
        assert_eq!(
            keys(lru.trim(NONE)),
            ["c"],
            "then `c`, older than the `a` that was read"
        );
        assert!(lru.contains(&"a"));
        // A change through `get_mut` counts as a use too; one through `peek_mut` doesn't.
        *lru.get_mut(&"a").unwrap() += 10;
        *lru.peek_mut(&"d").unwrap() += 10;
        lru.insert("e", 5, 40);
        assert_eq!(lru.trim(NONE), [("d", 14)]);
        assert_eq!(lru.peek(&"a"), Some(&11));
    }

    #[test]
    fn one_entry_over_the_cap_cannot_fill_the_budget() {
        let mut lru = ByteLru::new(100, 30);
        lru.insert("small", 1, 20);
        lru.insert("also small", 2, 20);
        // 90 big where 30 is the most for one: it goes, and takes nothing with it, though the
        // three fit the budget together (130 wouldn't, and the small ones are the older).
        lru.insert("huge", 3, 90);
        assert_eq!(keys(lru.trim(NONE)), ["huge"]);
        assert_eq!((lru.bytes(), lru.len()), (40, 2));
        // The same within the budget: 50 + 40 is under 100, and it still isn't kept.
        lru.insert("big", 4, 50);
        assert_eq!(keys(lru.trim(NONE)), ["big"]);
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn an_entry_over_the_cap_still_works_while_it_is_in_use() {
        let mut lru = ByteLru::new(100, 30);
        lru.insert("small", 1, 20);
        lru.insert("huge", 2, 90);
        // On screen: it's there to be used, however big.
        let in_use = |k: &&str, _: &u32| *k == "huge";
        // And it doesn't count against the budget, or it would push the small ones out.
        assert!(
            lru.trim(in_use).is_empty(),
            "110 of 100, 90 of it in use and over the cap"
        );
        assert_eq!(lru.bytes(), 110);
        assert_eq!(lru.get(&"huge"), Some(&2));
        // No longer in use: it isn't kept for next time.
        assert_eq!(keys(lru.trim(NONE)), ["huge"]);
        assert_eq!(lru.get(&"huge"), None);
        assert_eq!(lru.get(&"small"), Some(&1));
    }

    #[test]
    fn what_is_in_use_stays_even_past_the_budget() {
        let mut lru = ByteLru::new(100, 60);
        for (key, value) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            lru.insert(key, value, 40);
        }
        // `a` and `b` are the oldest, and on screen with `d`: `c` goes instead, and the cache
        // stays over its budget rather than drop what's being drawn.
        let on_screen = |k: &&str, _: &u32| matches!(*k, "a" | "b" | "d");
        assert_eq!(keys(lru.trim(on_screen)), ["c"]);
        assert_eq!(lru.bytes(), 120);
        assert!(lru.trim(on_screen).is_empty(), "nothing more to give");
        // Scrolled on, with only `a` still there: the oldest of the rest goes, and that's enough.
        assert_eq!(keys(lru.trim(|k, _| *k == "a")), ["b"]);
        assert_eq!((lru.bytes(), lru.len()), (80, 2));
    }

    #[test]
    fn weights_follow_replacing_resizing_and_removing() {
        let mut lru = ByteLru::new(100, 60);
        assert_eq!(lru.insert("a", 1, 40), None);
        assert_eq!(lru.insert("a", 2, 10), Some(1), "the old value comes back");
        assert_eq!((lru.bytes(), lru.len()), (10, 1));
        // A streaming answer grows: so does what it weighs, and past the cap it's let go.
        lru.set_bytes(&"a", 50);
        assert_eq!(lru.bytes(), 50);
        lru.insert("b", 3, 30);
        lru.set_bytes(&"a", 70);
        assert_eq!(keys(lru.trim(NONE)), ["a"]);
        assert_eq!(lru.remove(&"b"), Some(3));
        assert_eq!(lru.remove(&"b"), None);
        assert_eq!((lru.bytes(), lru.len()), (0, 0));
        lru.insert("c", 4, 20);
        lru.insert("d", 5, 20);
        lru.insert("e", 6, 20);
        // Only what's wanted is kept, and the weight follows.
        assert_eq!(keys(lru.retain(|_, v| *v != 5)), ["d"]);
        assert_eq!((lru.bytes(), lru.len()), (40, 2));
        assert_eq!(lru.clear().len(), 2);
        assert!(lru.is_empty() && lru.bytes() == 0);
    }
}
