//! What Trek keeps about telemetry, in `telemetry/` in the data folder: the user's answer, the
//! install id, what was counted, the queue, and what was last sent. Small JSON files, each
//! written whole.

use crate::queue::Queue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The user's answer to "Share anonymous usage data?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Consent {
    /// Not asked yet: the card shows once at the next launch.
    #[default]
    Unasked,
    Yes,
    No,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub consent: Consent,
    /// When the user answered, in seconds since 1970.
    pub answered_at: Option<i64>,
    /// A random id that tells one install from another, made at the first yes. Nothing else
    /// is known of the install.
    pub install_id: Option<String>,
    /// `install` went out for this data folder (a new id after a reset doesn't send it again).
    pub install_sent: bool,
    /// The day `app_opened` last went out (`YYYY-MM-DD`, local time).
    pub opened_day: Option<String>,
    /// The version that last ran here, for `app_updated`.
    pub last_version: Option<String>,
    /// Feature counts not sent yet, by day, then `kind:name`.
    pub counts: BTreeMap<String, BTreeMap<String, u32>>,
}

/// What went out last, as it was sent, for Settings › Privacy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    /// When, in seconds since 1970.
    pub at: i64,
    pub url: String,
    /// How many events.
    pub events: usize,
    /// The body, as JSON with line breaks.
    pub body: String,
}

pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new(data_dir: &Path) -> Files {
        Files {
            dir: data_dir.join("telemetry"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_state(&self) -> State {
        read(&self.dir.join("state.json")).unwrap_or_default()
    }

    pub fn save_state(&self, state: &State) {
        write(&self.dir, "state.json", state)
    }

    pub fn load_queue(&self) -> Queue {
        read(&self.dir.join("queue.json")).unwrap_or_default()
    }

    pub fn save_queue(&self, queue: &Queue) {
        write(&self.dir, "queue.json", queue)
    }

    pub fn load_sent(&self) -> Option<Sent> {
        read(&self.dir.join("last-sent.json"))
    }

    pub fn save_sent(&self, sent: &Sent) {
        write(&self.dir, "last-sent.json", sent)
    }
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("{}: {e}; starting over", path.display());
            None
        }
    }
}

/// Written beside and moved over, so a crash mid-write never leaves half a file.
fn write<T: Serialize>(dir: &Path, name: &str, value: &T) {
    let Ok(json) = serde_json::to_string_pretty(value) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("{}: {e}", dir.display());
        return;
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    let done = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, dir.join(name)));
    if let Err(e) = done {
        tracing::warn!("{}: {e}", dir.join(name).display());
    }
}
