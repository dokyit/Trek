//! Keeping Trek on the iPhone current (`docs/MOBILE.md`, "Updating the iPhone app"). The app is
//! signed with a free Apple ID: no App Store or TestFlight, so it can't update itself; the Mac it
//! pairs with builds it and installs it over USB or Wi-Fi (`ios/scripts/phone-update.sh`).
//!
//! The snapshot carries the newest Trek the Mac knows of ([`TrekRelease`]); a phone older than
//! that offers to update. "Update now" sends `update_phone`, answered with `ack` once the Mac has
//! started; how it goes reaches every phone as `phone_update` pushes ([`PhoneUpdate`]), since the
//! install closes the app that asked. A Mac that serves this lists
//! [`super::feature::UPDATE_PHONE`] in `welcome`/`paired` `features`.

use serde::{Deserialize, Serialize};

/// The newest Trek the Mac knows of: its own version, or a newer release its updater found (the
/// Mac may not have installed it yet: it does when Trek quits, and the phone needn't wait).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct TrekRelease {
    /// "0.4.27".
    pub version: String,
    /// The first line of its release notes, when the Mac has them, without the list marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl TrekRelease {
    /// The first line of `notes` worth showing on its own: the first that isn't empty or a
    /// heading, without its list marker, cut to a line's length.
    pub fn first_note(notes: &str) -> Option<String> {
        const MAX: usize = 160;
        let line = notes
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))?
            .trim_start_matches(['-', '*', '•'])
            .trim_start();
        if line.is_empty() {
            return None;
        }
        Some(if line.chars().count() > MAX {
            let cut: String = line.chars().take(MAX - 1).collect();
            format!("{}…", cut.trim_end())
        } else {
            line.to_string()
        })
    }
}

/// `update_phone`: build Trek for the iPhone and install it, at `version` when the phone asks for
/// one newer than the Mac's (a release the Mac hasn't installed yet), else at the Mac's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdatePhoneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Where an iPhone update is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneUpdateState {
    /// Building the app on the Mac (a few minutes).
    Building,
    /// Installing it on the iPhone: Trek there closes now.
    Installing,
    /// On the iPhone; Trek there opens again (or the user opens it).
    Installed,
    /// It didn't happen; `message` says why, in plain words.
    Failed,
}

/// `phone_update`: how an iPhone update is going, to every phone (the one that asked closes
/// when the app is installed over it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneUpdate {
    pub state: PhoneUpdateState,
    /// The version being installed, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For `failed`, why ("Your Mac couldn't reach the iPhone: plug it in or join the same
    /// Wi-Fi"); otherwise a detail, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
