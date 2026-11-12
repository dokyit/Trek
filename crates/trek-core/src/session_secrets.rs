//! Secrets an agent asked the user for (`request_secret`, Trek's request card). The value goes in
//! a private file the agent is told the path of, so the conversation only ever holds the path.
//! Running sessions can't take a new environment variable, and a file works the same for every
//! agent. Files last until Trek quits, and the next launch sweeps what a crash left.
//!
//! An agent can still print what it reads. `scrub` takes the value back out of whatever Trek is
//! about to show or keep; what the agent's own program writes to its own history is out of reach.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

/// Values shorter than this aren't taken out of text: "abc" is in too many words.
const MIN_SCRUBBED: usize = 4;

/// Whether `name` can name a secret (and so a file): what an environment variable may be called.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every value given this run, `(name, value)`, to take back out of what any thread says: an agent
/// can hand a path to a sub-agent, or tell another thread what it read. Only for `scrub`.
static KNOWN: LazyLock<RwLock<Arc<Vec<(String, String)>>>> = LazyLock::new(Default::default);

pub fn known() -> Arc<Vec<(String, String)>> {
    KNOWN.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `known`, without waiting: `None` while the list is being changed. For a panic hook, which may
/// run on the very thread that's changing it, where waiting would never end.
pub fn try_known() -> Option<Arc<Vec<(String, String)>>> {
    match KNOWN.try_read() {
        Ok(known) => Some(known.clone()),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner().clone()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

fn remember(name: &str, value: &str) {
    let mut known = KNOWN.write().unwrap_or_else(|e| e.into_inner());
    let mut list = known.as_ref().clone();
    if !list.iter().any(|(_, v)| v == value) {
        list.push((name.to_string(), value.to_string()));
    }
    *known = Arc::new(list);
}

fn root() -> PathBuf {
    crate::paths::data_dir().join("session-secrets")
}

/// A thread's folder. Its id is Trek's own, but it becomes a path, so nothing else gets through.
fn thread_dir(thread: &str) -> PathBuf {
    let safe: String = thread
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    root().join(safe)
}

/// Save `value` for `thread` where only this user can read it, and say where.
pub fn save(thread: &str, name: &str, value: &str) -> std::io::Result<PathBuf> {
    if !valid_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "That name can't be a file's.",
        ));
    }
    let dir = thread_dir(thread);
    // The user's alone on both platforms (0700 and 0600 on the Mac, an owner-only access list on
    // Windows), whether or not they were there already: a data folder moved somewhere others can
    // reach doesn't hand them the secrets.
    trek_private::create_dir(&root())?;
    trek_private::create_dir(&dir)?;
    let path = dir.join(name);
    trek_private::write(&path, value.as_bytes())?;
    remember(name, value);
    Ok(path)
}

/// Whether `path` is in the secrets' folder (a secret's file, or the folder itself), however it's
/// written: through a link, `..`, another spelling of the data folder. What's handed out by path
/// (a file an answer links, for the phone) never is.
pub fn holds(path: &std::path::Path) -> bool {
    let root = root();
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).ok();
    let inside = |p: &std::path::Path| {
        p.starts_with(&root) || canon(&root).is_some_and(|r| p.starts_with(r))
    };
    inside(path) || canon(path).is_some_and(|p| inside(&p))
}

/// `thread`'s files go (it was deleted). The values stay known: they can still come up elsewhere.
pub fn forget(thread: &str) {
    let _ = std::fs::remove_dir_all(thread_dir(thread));
}

/// All of them go: Trek is quitting, or a new launch finds what a crash left.
pub fn forget_all() {
    let _ = std::fs::remove_dir_all(root());
    *KNOWN.write().unwrap_or_else(|e| e.into_inner()) = Arc::default();
}

/// `text` with every known value taken out, its name in its place. `secrets`: `(name, value)`.
pub fn scrub<'a>(text: &'a str, secrets: &[(String, String)]) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(text);
    for (name, value) in secrets {
        if value.len() >= MIN_SCRUBBED && out.contains(value.as_str()) {
            out = Cow::Owned(out.replace(value.as_str(), &format!("[{name} hidden]")));
        }
    }
    out
}

/// `scrub`, in place.
pub fn scrub_string(text: &mut String, secrets: &[(String, String)]) {
    if let Cow::Owned(clean) = scrub(text, secrets) {
        *text = clean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// These tests move the process's data folder.
    static DATA: Mutex<()> = Mutex::new(());

    fn with_data_dir(f: impl FnOnce(&std::path::Path)) {
        let _lock = DATA.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "trek-session-secrets-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::paths::isolate_thread(dir.clone());
        f(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_has_to_be_a_plain_variable_name() {
        for good in ["GITHUB_TOKEN", "a", "_x1", "npmToken"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "1X",
            "../x",
            "a/b",
            "a b",
            "a.b",
            "a\\b",
            "..",
            "x\n",
            &"x".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
            assert!(save("t", bad, "v").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_secret_is_a_private_file_until_it_is_forgotten() {
        with_data_dir(|_| {
            let path = save("thread-1", "TOKEN", "hunter2-value").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "hunter2-value");
            assert!(
                known()
                    .iter()
                    .any(|(n, v)| n == "TOKEN" && v == "hunter2-value")
            );
            // The user's alone, the file and both folders, on both platforms.
            let private = |p: &std::path::Path| trek_private::is_private(p).unwrap();
            assert!(private(&path));
            assert!(private(path.parent().unwrap()));
            assert!(private(path.parent().unwrap().parent().unwrap()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = |p: &std::path::Path| {
                    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
                };
                assert_eq!(mode(&path), 0o600);
                assert_eq!(mode(path.parent().unwrap()), 0o700);
                // Got looser: saved again, it's tight again.
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            #[cfg(windows)]
            {
                // Got looser (a copy that took its folder's access list): saved again, it's the
                // user's alone again.
                std::fs::remove_file(&path).unwrap();
                std::fs::write(&path, "loose").unwrap();
            }
            assert!(!private(&path));
            save("thread-1", "TOKEN", "second").unwrap();
            assert!(private(&path));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
            let other = save("thread-2", "TOKEN", "other").unwrap();
            assert_ne!(path, other);
            forget("thread-1");
            assert!(!path.exists());
            assert!(other.exists(), "another thread's stays");
            assert!(known().iter().any(|(_, v)| v == "second"), "still known");
            forget_all();
            assert!(!other.exists());
            assert!(known().is_empty());
        });
    }

    #[test]
    fn a_secrets_file_is_known_for_one_however_its_path_is_written() {
        with_data_dir(|data| {
            let path = save("thread-1", "TOKEN", "hunter2-value").unwrap();
            assert!(holds(&path));
            assert!(holds(&data.join("session-secrets")), "the folder too");
            let dotted = data
                .join("x")
                .join("..")
                .join("session-secrets/thread-1/TOKEN");
            std::fs::create_dir_all(data.join("x")).unwrap();
            assert!(holds(&dotted), "{dotted:?}");
            #[cfg(unix)]
            {
                let link = data.join("innocent.png");
                std::os::unix::fs::symlink(&path, &link).unwrap();
                assert!(holds(&link), "through a link");
            }
            let other = data.join("notes.md");
            std::fs::write(&other, "hi").unwrap();
            assert!(!holds(&other));
            assert!(!holds(std::path::Path::new("/tmp/not-there")));
        });
    }

    #[test]
    fn a_thread_id_cant_lead_out_of_the_folder() {
        with_data_dir(|data| {
            let path = save("../../etc", "TOKEN", "v").unwrap();
            assert!(path.starts_with(data.join("session-secrets")), "{path:?}");
            assert!(
                !path
                    .components()
                    .any(|c| c == std::path::Component::ParentDir),
                "{path:?}"
            );
        });
    }

    #[test]
    fn scrubbing_takes_every_value_out_and_names_it() {
        let secrets = vec![
            ("TOKEN".to_string(), "ghp_abc123".to_string()),
            ("PIN".to_string(), "12".to_string()),
        ];
        assert_eq!(
            scrub("Bearer ghp_abc123 and ghp_abc123", &secrets),
            "Bearer [TOKEN hidden] and [TOKEN hidden]"
        );
        assert_eq!(scrub("nothing here", &secrets), "nothing here");
        assert!(matches!(scrub("nothing", &secrets), Cow::Borrowed(_)));
        // Too short to tell from ordinary text.
        assert_eq!(scrub("room 12", &secrets), "room 12");
        let mut text = "x ghp_abc123".to_string();
        scrub_string(&mut text, &secrets);
        assert_eq!(text, "x [TOKEN hidden]");
    }
}
