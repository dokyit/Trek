//! What one Trek does for a thread that runs on another, beyond what a phone does (Trek 0.4.68):
//! a secret the thread's agent asked for, given from the other computer (feature
//! `secret_answer`). The computer that runs the thread keeps the thread, its files, its locks and
//! the secret's file; the other one only asks and answers over its paired, pinned connection.

use serde::{Deserialize, Serialize};

/// The first Trek that serves what's here. A machine on an older one is told so by name
/// ("T-Book needs Trek 0.4.68 or later to take a secret from here"), never "do it there".
pub const FROM_HERE_SINCE: &str = "0.4.68";

/// A thread waits for a secret (`ThreadSummary::secret`): the request a `secret` answer goes to.
/// The row's `needs` says the same in words, for clients that can't answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretAsk {
    pub request_id: String,
    /// What the agent calls it (`GITHUB_TOKEN`).
    pub name: String,
    /// Why it wants it, in its words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// A secret's value on its way to the computer that asked for it. It prints as `…` wherever a
/// message is formatted (a log line, a test's failure), and is never kept by either end's
/// protocol code: the host hands it to the thread's private file and lets go.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, for the one place that saves it.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(…)")
    }
}
