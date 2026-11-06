//! Where reports go and whether anything may go at all: the PostHog project key this build was
//! made with (`TREK_POSTHOG_KEY`, `TREK_POSTHOG_HOST` at build time), or one in the data folder
//! for trying a build out, and what keeps everything in whatever the user said (`DO_NOT_TRACK`,
//! a test or the isolated verify profile).

use std::path::{Path, PathBuf};

/// PostHog's US cloud, where a key without a host sends.
pub const DEFAULT_HOST: &str = "https://us.i.posthog.com";

/// A PostHog project key and the host it sends to. A project key (`phc_…`) can only add events,
/// never read them, so it's safe in a build anyone can download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub host: String,
    /// The file it came from, if not from the build.
    pub file: Option<PathBuf>,
}

impl Key {
    /// The host without its scheme, for a sentence ("us.i.posthog.com").
    pub fn host_name(&self) -> &str {
        self.host
            .split_once("://")
            .map_or(self.host.as_str(), |(_, rest)| rest)
    }
}

/// Why nothing is sent whatever the user said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// `DO_NOT_TRACK` is set: no usage data, and no asking for it.
    DoNotTrack,
    /// A test, or the isolated verify profile: nothing leaves the machine, feedback included.
    Isolated,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub key: Option<Key>,
    pub blocked: Option<Blocked>,
}

impl Config {
    /// This process's: the override in `data_dir` or the build's key, and what blocks sending.
    pub fn detect(data_dir: &Path) -> Config {
        let blocked = if trek_core::paths::isolated() || cfg!(test) {
            Some(Blocked::Isolated)
        } else if do_not_track(std::env::var("DO_NOT_TRACK").ok().as_deref()) {
            Some(Blocked::DoNotTrack)
        } else {
            None
        };
        let key = override_key(data_dir).or_else(|| {
            key_from(
                option_env!("TREK_POSTHOG_KEY"),
                option_env!("TREK_POSTHOG_HOST"),
                None,
            )
        });
        Config { key, blocked }
    }

    /// Usage data may go out (with the user's yes).
    pub fn may_send(&self) -> bool {
        self.blocked.is_none() && self.key.is_some()
    }

    /// Feedback may go out: it's sent because the user asked, so only a test profile or a missing
    /// key keeps it in.
    pub fn may_send_feedback(&self) -> bool {
        self.blocked != Some(Blocked::Isolated) && self.key.is_some()
    }
}

/// Whether `DO_NOT_TRACK` says no: set to anything but empty, `0` or `false`.
pub fn do_not_track(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    })
}

/// The file a key for trying a build out goes in: `telemetry/posthog.toml` in the data folder,
/// with `key = "phc_…"` and, if not the US cloud, `host = "https://…"`.
pub fn override_file(data_dir: &Path) -> PathBuf {
    data_dir.join("telemetry").join("posthog.toml")
}

fn override_key(data_dir: &Path) -> Option<Key> {
    #[derive(serde::Deserialize)]
    struct File {
        key: Option<String>,
        host: Option<String>,
    }
    let path = override_file(data_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let file: File = match toml::from_str(&text) {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            return None;
        }
    };
    let key = key_from(
        file.key.as_deref(),
        file.host.as_deref(),
        Some(path.clone()),
    );
    if key.is_none() {
        tracing::warn!(
            "{}: not a PostHog project key (phc_…); ignored",
            path.display()
        );
    }
    key
}

/// A key and host as given, if the key is a project key and the host a web address. Anything
/// else (a personal `phx_` key, which can read everything, or nothing at all) is no key.
pub fn key_from(key: Option<&str>, host: Option<&str>, file: Option<PathBuf>) -> Option<Key> {
    let key = key.map(str::trim).filter(|k| valid_key(k))?;
    let host = match host.map(str::trim).filter(|h| !h.is_empty()) {
        None => DEFAULT_HOST.to_string(),
        Some(h) if h.starts_with("https://") || h.starts_with("http://localhost") => {
            h.trim_end_matches('/').to_string()
        }
        Some(_) => return None,
    };
    Some(Key {
        key: key.to_string(),
        host,
        file,
    })
}

fn valid_key(key: &str) -> bool {
    key.strip_prefix("phc_").is_some_and(|rest| {
        rest.len() >= 16 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "phc_0123456789abcdefABCDEF";

    #[test]
    fn do_not_track_is_any_yes() {
        for yes in ["1", "true", "yes", " 1 "] {
            assert!(do_not_track(Some(yes)), "{yes}");
        }
        for no in [None, Some(""), Some("0"), Some("false"), Some("FALSE")] {
            assert!(!do_not_track(no), "{no:?}");
        }
    }

    #[test]
    fn only_a_project_key_is_a_key() {
        let key = key_from(Some(KEY), None, None).unwrap();
        assert_eq!(key.host, DEFAULT_HOST);
        assert_eq!(key.host_name(), "us.i.posthog.com");
        for not in [
            "",
            "phc_",
            "phc_short",
            "phx_0123456789abcdefABCDEF",
            "sk-0123456789abcdefABCDEF",
        ] {
            assert_eq!(key_from(Some(not), None, None), None, "{not}");
        }
        assert_eq!(key_from(None, Some(DEFAULT_HOST), None), None);
        // An empty host (an unset repository variable) is the default one.
        assert_eq!(
            key_from(Some(KEY), Some(""), None).unwrap().host,
            DEFAULT_HOST
        );
        assert_eq!(
            key_from(Some(KEY), Some("https://eu.i.posthog.com/"), None)
                .unwrap()
                .host,
            "https://eu.i.posthog.com"
        );
        // Never in the clear to another machine.
        assert_eq!(key_from(Some(KEY), Some("http://example.com"), None), None);
    }

    #[test]
    fn a_key_in_the_data_folder_wins_and_a_bad_one_is_none() {
        let dir =
            std::env::temp_dir().join(format!("trek-telemetry-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("telemetry")).unwrap();
        assert_eq!(override_key(&dir), None);
        std::fs::write(
            override_file(&dir),
            format!("key = \"{KEY}\"\nhost = \"https://eu.i.posthog.com\"\n"),
        )
        .unwrap();
        let key = override_key(&dir).unwrap();
        assert_eq!(
            (key.key.as_str(), key.host.as_str()),
            (KEY, "https://eu.i.posthog.com")
        );
        assert_eq!(key.file, Some(override_file(&dir)));
        std::fs::write(
            override_file(&dir),
            "key = \"phx_personal_key_0123456789\"\n",
        )
        .unwrap();
        assert_eq!(override_key(&dir), None);
        std::fs::write(override_file(&dir), "not toml [").unwrap();
        assert_eq!(override_key(&dir), None);
        // Tests are a test profile, whatever key there is: nothing leaves.
        std::fs::write(override_file(&dir), format!("key = \"{KEY}\"\n")).unwrap();
        let config = Config::detect(&dir);
        assert!(config.key.is_some());
        assert_eq!(config.blocked, Some(Blocked::Isolated));
        assert!(!config.may_send() && !config.may_send_feedback());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn feedback_goes_past_do_not_track_but_not_a_test_profile() {
        let key = key_from(Some(KEY), None, None);
        let dnt = Config {
            key: key.clone(),
            blocked: Some(Blocked::DoNotTrack),
        };
        assert!(!dnt.may_send() && dnt.may_send_feedback());
        let open = Config { key, blocked: None };
        assert!(open.may_send() && open.may_send_feedback());
        let keyless = Config::default();
        assert!(!keyless.may_send() && !keyless.may_send_feedback());
    }
}
