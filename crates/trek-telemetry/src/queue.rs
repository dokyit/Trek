//! Events waiting to go: sent in batches, and when a batch fails, tried again later, a few times
//! at most and never after a week. A batch PostHog turns down for good (a bad key) is dropped.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Events kept at most; the oldest go first.
pub const MAX_QUEUED: usize = 500;
/// Events sent in one request at most.
pub const BATCH: usize = 50;
/// Tries an event gets before it's dropped.
pub const MAX_ATTEMPTS: u32 = 5;
/// An event older than this is dropped unsent.
pub const MAX_AGE_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Queued {
    pub event: Value,
    /// When it was queued, in seconds since 1970.
    pub queued_at: i64,
    /// Sends that failed so far.
    pub attempts: u32,
    /// Not tried again before this.
    pub next_try: i64,
}

/// How a send went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// Worth trying again later: no network, a timeout, PostHog busy or down (429, 5xx).
    Retry(String),
    /// Never going to work: the key or the payload was turned down (4xx).
    Rejected(String),
}

impl Outcome {
    /// What an HTTP status means for the batch.
    pub fn of_status(status: u16) -> Outcome {
        match status {
            200..=299 => Outcome::Sent,
            408 | 429 | 500..=599 => Outcome::Retry(format!("PostHog answered {status}")),
            _ => Outcome::Rejected(format!("PostHog turned it down ({status})")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Queued>,
}

/// How long after `attempts` failed sends the next one waits: 1, 4, 16, 64 minutes, then 4 hours.
pub fn backoff(attempts: u32) -> i64 {
    60 * 4i64.pow(attempts.saturating_sub(1).min(4)).min(240)
}

impl Queue {
    pub fn push(&mut self, event: Value, now: i64) {
        self.items.push(Queued {
            event,
            queued_at: now,
            attempts: 0,
            next_try: now,
        });
        if self.items.len() > MAX_QUEUED {
            let over = self.items.len() - MAX_QUEUED;
            self.items.drain(..over);
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Events too old to send go.
    pub fn expire(&mut self, now: i64) -> usize {
        let before = self.items.len();
        self.items.retain(|q| now - q.queued_at < MAX_AGE_SECS);
        before - self.items.len()
    }

    /// Up to `BATCH` events due now, taken out while they're sent (`settle` puts back what
    /// failed). Empty when none is due.
    pub fn take_due(&mut self, now: i64) -> Vec<Queued> {
        self.expire(now);
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.items.len());
        for q in self.items.drain(..) {
            if taken.len() < BATCH && q.next_try <= now {
                taken.push(q);
            } else {
                kept.push(q);
            }
        }
        self.items = kept;
        taken
    }

    /// When the soonest event is due, if any is waiting.
    pub fn next_due(&self) -> Option<i64> {
        self.items.iter().map(|q| q.next_try).min()
    }

    /// What became of `batch`: sent or turned down, it's gone; failed, each event waits longer
    /// before its next try, and goes after its last. Returns how many were dropped.
    pub fn settle(&mut self, batch: Vec<Queued>, outcome: &Outcome, now: i64) -> usize {
        match outcome {
            Outcome::Sent => 0,
            Outcome::Rejected(_) => batch.len(),
            Outcome::Retry(_) => {
                let mut dropped = 0;
                let mut back = Vec::new();
                for mut q in batch {
                    q.attempts += 1;
                    if q.attempts >= MAX_ATTEMPTS {
                        dropped += 1;
                        continue;
                    }
                    q.next_try = now + backoff(q.attempts);
                    back.push(q);
                }
                // Ahead of what was queued meanwhile, as they're older.
                back.append(&mut self.items);
                self.items = back;
                dropped
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statuses_say_whether_to_try_again() {
        assert_eq!(Outcome::of_status(200), Outcome::Sent);
        for again in [408, 429, 500, 502, 503] {
            assert!(
                matches!(Outcome::of_status(again), Outcome::Retry(_)),
                "{again}"
            );
        }
        for never in [400, 401, 403, 404, 413] {
            assert!(
                matches!(Outcome::of_status(never), Outcome::Rejected(_)),
                "{never}"
            );
        }
    }

    #[test]
    fn events_go_in_batches() {
        let mut q = Queue::default();
        for i in 0..(BATCH + 7) {
            q.push(json!(i), 100);
        }
        let first = q.take_due(100);
        assert_eq!(first.len(), BATCH);
        assert_eq!(first[0].event, json!(0));
        assert_eq!(q.len(), 7);
        q.settle(first, &Outcome::Sent, 100);
        assert_eq!(q.take_due(100).len(), 7);
        assert!(q.take_due(100).is_empty());
    }

    #[test]
    fn a_failed_batch_is_tried_again_later_a_few_times_at_most() {
        let mut q = Queue::default();
        q.push(json!("a"), 0);
        let mut now = 0;
        let mut tries = 0;
        loop {
            let batch = q.take_due(now);
            if batch.is_empty() {
                match q.next_due() {
                    // Not due yet: nothing goes before its time.
                    Some(due) => {
                        assert!(q.take_due(due - 1).is_empty());
                        now = due;
                        continue;
                    }
                    None => break,
                }
            }
            tries += 1;
            q.settle(batch, &Outcome::Retry("offline".into()), now);
        }
        assert_eq!(tries, MAX_ATTEMPTS, "never retried forever");
        assert!(q.is_empty());
        // The waits grow: 1, 4, 16 and 64 minutes.
        assert_eq!(now, 60 * (1 + 4 + 16 + 64));
        assert_eq!(backoff(9), 4 * 3600);
    }

    #[test]
    fn a_rejected_batch_is_dropped_and_old_events_expire() {
        let mut q = Queue::default();
        q.push(json!(1), 0);
        let batch = q.take_due(0);
        assert_eq!(q.settle(batch, &Outcome::Rejected("401".into()), 0), 1);
        assert!(q.is_empty());
        q.push(json!(2), 0);
        assert!(q.take_due(MAX_AGE_SECS).is_empty());
        assert!(q.is_empty());
    }

    #[test]
    fn the_queue_is_capped_and_keeps_the_newest() {
        let mut q = Queue::default();
        for i in 0..(MAX_QUEUED + 10) {
            q.push(json!(i), 0);
        }
        assert_eq!(q.len(), MAX_QUEUED);
        assert_eq!(q.items[0].event, json!(10));
    }

    #[test]
    fn retried_events_keep_their_place_ahead_of_new_ones() {
        let mut q = Queue::default();
        q.push(json!("old"), 0);
        let batch = q.take_due(0);
        q.push(json!("new"), 1);
        q.settle(batch, &Outcome::Retry("offline".into()), 1);
        assert_eq!(q.items[0].event, json!("old"));
        assert_eq!(q.items[1].event, json!("new"));
    }
}
