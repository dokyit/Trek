//! The passkeys Trek holds: one file in its data folder, `passkeys/passkeys.json`, listing each
//! passkey's site, account and key handle (`keys.rs`). No private key is in it on a Mac with a
//! Secure Enclave: a handle there is the enclave's wrapped blob, which only that enclave, with
//! Touch ID or the password, can use.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One passkey: a discoverable credential for one account on one site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passkey {
    /// The credential id, base64url.
    pub id: String,
    /// The site it signs in to (the relying party id): `github.com`.
    pub rp_id: String,
    /// The site's own name for itself, when it gave one.
    #[serde(default)]
    pub rp_name: String,
    /// The account's id at the site (the user handle), base64url.
    pub user_handle: String,
    /// The account's name as the site gave it: a user name or an email address.
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_display_name: String,
    /// The public key (`04 || x || y`), base64url.
    pub public_key: String,
    /// Where the private key is (`keys.rs`).
    pub key: String,
    /// Seconds since 1970.
    pub created: u64,
    #[serde(default)]
    pub last_used: Option<u64>,
    /// What a newer Trek wrote that this one doesn't know: kept as it was, so an older build
    /// opening the list doesn't strip it.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Passkey {
    /// The account as a person would name it: its user name, else its display name.
    pub fn account(&self) -> String {
        named(&self.user_name, &self.user_display_name).unwrap_or_else(|| "an account".into())
    }
}

/// The longest account name shown, in characters.
const NAME_SHOWN: usize = 64;

/// An account's name for Trek's own words (the bar, the Touch ID prompt, Settings): its user
/// name, else its display name. The site wrote both, so they are shown as one short line: no
/// line breaks or control characters, and no more than a name's length, so a site can't put a
/// paragraph of its own into Trek's prompt.
pub(crate) fn named(user_name: &str, display_name: &str) -> Option<String> {
    [user_name, display_name]
        .into_iter()
        .map(|name| {
            let seen: String = name.chars().filter(|c| !unseen(*c)).collect();
            let line: String = seen
                .split(|c: char| c.is_whitespace() || c.is_control())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if line.chars().count() > NAME_SHOWN {
                let clipped: String = line.chars().take(NAME_SHOWN - 1).collect();
                format!("{clipped}…")
            } else {
                line
            }
        })
        .find(|name| !name.is_empty())
}

/// Characters that draw nothing and change how what's around them reads: the direction
/// overrides and isolates that can show a name backwards ("moc.elgoog" as "google.com") or push
/// Trek's own words about, zero-width characters, and the like. None belongs in a name shown in
/// a prompt.
fn unseen(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{FEFF}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{E0000}'..='\u{E007F}')
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    passkeys: Vec<Passkey>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// One writer at a time: a ceremony ends on a background thread while Settings may delete.
static LOCK: Mutex<()> = Mutex::new(());

/// Trek's passkey list, at its place in the data folder.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Store::at(trek_core::paths::data_dir().join("passkeys"))
    }
}

impl Store {
    /// The list kept in folder `dir`.
    pub fn at(dir: impl AsRef<Path>) -> Store {
        Store {
            path: dir.as_ref().join("passkeys.json"),
        }
    }

    /// The list beside the file: the one before the last change.
    fn backup(&self) -> PathBuf {
        self.path.with_extension("json.bak")
    }

    fn parse(bytes: &[u8]) -> Result<File, String> {
        serde_json::from_slice::<File>(bytes)
            .map_err(|e| format!("Trek's passkey list can't be read: {e}"))
    }

    /// The file as it stands. One that can't be read is an error, never an empty list: saving
    /// over it would lose every passkey in it, and each is the only copy of its key. A file cut
    /// short (the power went mid-write on a disk that reorders writes) falls back to the copy
    /// kept from before the last change, which is at most one change behind.
    fn read(&self) -> Result<File, String> {
        let main = match std::fs::read(&self.path) {
            Ok(bytes) => Self::parse(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
            Err(e) => return Err(format!("Trek's passkey list can't be read: {e}")),
        };
        let Err(broken) = main else { return main };
        match std::fs::read(self.backup()).map(|bytes| Self::parse(&bytes)) {
            Ok(Ok(kept)) => {
                tracing::warn!("passkeys: {broken}; using the copy from before the last change");
                Ok(kept)
            }
            _ => Err(broken),
        }
    }

    /// Every passkey, oldest first.
    pub fn list(&self) -> Result<Vec<Passkey>, String> {
        self.read().map(|f| f.passkeys)
    }

    /// The passkeys for site `rp_id`, exactly: `example.com`'s are not `login.example.com`'s.
    pub fn for_site(&self, rp_id: &str) -> Result<Vec<Passkey>, String> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|p| p.rp_id == rp_id)
            .collect())
    }

    /// Write `file` in the list's place: to a file of its own beside it (the user's alone to
    /// read from the start), onto the disk, and then moved over the list, whose last readable
    /// state is kept as the backup first. A crash at any point leaves a whole list.
    fn write(&self, file: &File) -> Result<(), String> {
        use std::io::Write as _;
        let fail = |e: std::io::Error| format!("Trek's passkey list can't be saved: {e}");
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(fail)?;
        let text = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let written = options.open(&tmp).and_then(|mut out| {
            out.write_all(&text)?;
            out.sync_all()
        });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(fail(e));
        }
        // Only a list that reads is worth keeping as the backup: a broken one would replace the
        // good copy this very save may be recovering from.
        if std::fs::read(&self.path).is_ok_and(|bytes| Self::parse(&bytes).is_ok()) {
            let _ = std::fs::copy(&self.path, self.backup());
        }
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(fail(e));
        }
        // The move itself, onto the disk (a folder can be synced where it can be opened).
        if let Ok(folder) = std::fs::File::open(dir) {
            let _ = folder.sync_all();
        }
        Ok(())
    }

    /// Change the list under the lock.
    fn update<T>(&self, change: impl FnOnce(&mut Vec<Passkey>) -> T) -> Result<T, String> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read()?;
        let out = change(&mut file.passkeys);
        self.write(&file)?;
        Ok(out)
    }

    /// Add `passkey`, beside whatever is kept for the same account: the older ones go only once
    /// the site has the new one (`replace_older`).
    pub fn add(&self, passkey: Passkey) -> Result<(), String> {
        self.update(|all| {
            all.retain(|p| p.id != passkey.id);
            all.push(passkey);
        })
    }

    /// The passkeys kept for the account `user_handle` (base64url) on site `rp_id`.
    pub fn for_account(&self, rp_id: &str, user_handle: &str) -> Result<Vec<Passkey>, String> {
        Ok(self
            .for_site(rp_id)?
            .into_iter()
            .filter(|p| p.user_handle == user_handle)
            .collect())
    }

    /// An account has one passkey per site here (WebAuthn's rule for discoverable credentials):
    /// delete the others kept for `newest`'s account, handing them back for their keys to be
    /// forgotten.
    pub fn replace_older(&self, newest: &Passkey) -> Result<Vec<Passkey>, String> {
        self.update(|all| {
            let older = |p: &Passkey| {
                p.rp_id == newest.rp_id && p.user_handle == newest.user_handle && p.id != newest.id
            };
            let replaced: Vec<Passkey> = all.iter().filter(|p| older(p)).cloned().collect();
            all.retain(|p| !older(p));
            replaced
        })
    }

    /// Delete the passkey with credential id `id`: the one removed, for its key to be forgotten.
    pub fn remove(&self, id: &str) -> Result<Option<Passkey>, String> {
        self.update(|all| {
            let at = all.iter().position(|p| p.id == id)?;
            Some(all.remove(at))
        })
    }

    /// Passkey `id` was just used.
    pub fn used(&self, id: &str, now: u64) -> Result<(), String> {
        self.update(|all| {
            if let Some(p) = all.iter_mut().find(|p| p.id == id) {
                p.last_used = Some(now);
            }
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "trek-passkeys-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Store::at(dir)
    }

    fn passkey(id: &str, rp: &str, user: &str) -> Passkey {
        Passkey {
            id: id.into(),
            rp_id: rp.into(),
            rp_name: String::new(),
            user_handle: user.into(),
            user_name: format!("{user}@example"),
            user_display_name: String::new(),
            public_key: String::new(),
            key: format!("plain:{id}"),
            created: 1,
            last_used: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn passkeys_are_kept_per_site_and_survive_a_reload() {
        let store = temp_store("list");
        assert_eq!(store.list().unwrap(), vec![]);
        store.add(passkey("a", "example.com", "u1")).unwrap();
        store.add(passkey("b", "example.com", "u2")).unwrap();
        store.add(passkey("c", "login.example.com", "u1")).unwrap();
        let again = Store::at(store.path.parent().unwrap());
        assert_eq!(again.list().unwrap().len(), 3);
        let ids = |rp: &str| -> Vec<String> {
            again
                .for_site(rp)
                .unwrap()
                .into_iter()
                .map(|p| p.id)
                .collect()
        };
        assert_eq!(ids("example.com"), ["a", "b"]);
        // A site's passkeys are its own: not its subdomains', not its parent's.
        assert_eq!(ids("login.example.com"), ["c"]);
        assert_eq!(ids("com"), Vec::<String>::new());
        assert_eq!(ids("EXAMPLE.com"), Vec::<String>::new());
        store.used("b", 99).unwrap();
        assert_eq!(store.list().unwrap()[1].last_used, Some(99));
        assert_eq!(store.remove("a").unwrap().unwrap().id, "a");
        assert_eq!(store.remove("a").unwrap(), None);
        assert_eq!(ids("example.com"), ["b"]);
    }

    #[test]
    fn a_new_passkey_replaces_the_accounts_old_one_only_when_told() {
        let store = temp_store("replace");
        store.add(passkey("old", "example.com", "u1")).unwrap();
        store.add(passkey("other", "example.com", "u2")).unwrap();
        store
            .add(passkey("elsewhere", "example.org", "u1"))
            .unwrap();
        // Added beside the old one: the site may never take the new one.
        let new = passkey("new", "example.com", "u1");
        store.add(new.clone()).unwrap();
        let ids =
            |s: &Store| -> Vec<String> { s.list().unwrap().into_iter().map(|p| p.id).collect() };
        assert_eq!(ids(&store), ["old", "other", "elsewhere", "new"]);
        let both: Vec<String> = store
            .for_account("example.com", "u1")
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(both, ["old", "new"]);
        // Once it has: the account's older ones go, and nobody else's.
        let replaced = store.replace_older(&new).unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].id, "old");
        assert_eq!(ids(&store), ["other", "elsewhere", "new"]);
        assert_eq!(store.replace_older(&new).unwrap(), vec![]);
    }

    #[test]
    fn a_list_cut_short_falls_back_to_the_one_before_the_last_change() {
        let store = temp_store("backup");
        store.add(passkey("a", "example.com", "u1")).unwrap();
        store.add(passkey("b", "example.com", "u2")).unwrap();
        // The backup is the list before the last change.
        let before: File = serde_json::from_slice(&std::fs::read(store.backup()).unwrap()).unwrap();
        assert_eq!(before.passkeys.len(), 1);
        // The list is cut short: what's read is the backup, and a save keeps it, not the wreck.
        std::fs::write(&store.path, b"{ \"passkeys\": [ { \"id\": \"a").unwrap();
        let ids =
            |s: &Store| -> Vec<String> { s.list().unwrap().into_iter().map(|p| p.id).collect() };
        assert_eq!(ids(&store), ["a"]);
        store.add(passkey("c", "example.com", "u3")).unwrap();
        assert_eq!(ids(&store), ["a", "c"]);
        let kept: File = serde_json::from_slice(&std::fs::read(store.backup()).unwrap()).unwrap();
        assert_eq!(kept.passkeys.len(), 1, "the good copy, not the broken list");
        // Nothing is left beside the list but its backup.
        let mut names: Vec<String> = std::fs::read_dir(store.path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["passkeys.json", "passkeys.json.bak"]);
    }

    #[test]
    fn what_a_newer_trek_wrote_is_kept() {
        let store = temp_store("extra");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            br#"{ "version": 2, "passkeys": [ { "id": "a", "rp_id": "example.com", "user_handle": "u1",
                "public_key": "", "key": "se:AAAA", "created": 1, "synced": true } ] }"#,
        )
        .unwrap();
        store.used("a", 9).unwrap();
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(after["version"], 2);
        assert_eq!(after["passkeys"][0]["synced"], true);
        assert_eq!(after["passkeys"][0]["last_used"], 9);
    }

    #[test]
    fn a_list_that_cant_be_read_is_never_written_over() {
        let store = temp_store("broken");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, b"{ not json").unwrap();
        assert!(store.list().is_err());
        assert!(store.add(passkey("a", "example.com", "u1")).is_err());
        assert!(store.remove("a").is_err());
        assert_eq!(std::fs::read(&store.path).unwrap(), b"{ not json");
    }

    #[cfg(unix)]
    #[test]
    fn the_list_is_the_users_alone_to_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let store = temp_store("mode");
        store.add(passkey("a", "example.com", "u1")).unwrap();
        let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_account_is_named_by_what_the_site_gave() {
        let mut p = passkey("a", "example.com", "u1");
        assert_eq!(p.account(), "u1@example");
        p.user_name = "  ".into();
        p.user_display_name = "Ada".into();
        assert_eq!(p.account(), "Ada");
        p.user_display_name.clear();
        assert_eq!(p.account(), "an account");
        // A site's words stay a name: one line, and short.
        p.user_name = "ada\n\nTrek: enter your password\u{7}\tnow".into();
        assert_eq!(p.account(), "ada Trek: enter your password now");
        p.user_name = "a".repeat(200);
        assert_eq!(p.account().chars().count(), NAME_SHOWN);
        assert!(p.account().ends_with('…'));
        // And reads the way it's written: no override turns "moc.elgoog" into "google.com", and
        // nothing unseen pads it.
        p.user_name = "\u{202e}moc.elgoog\u{202c} \u{2066}ada\u{2069}\u{200b}\u{feff}".into();
        assert_eq!(p.account(), "moc.elgoog ada");
        p.user_name = "\u{200b}\u{202e}".into();
        assert_eq!(p.account(), "an account");
    }
}
