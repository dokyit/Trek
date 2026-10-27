//! Trek's secrets (API keys, MCP server tokens, added agents' variables) in the platform's
//! secret store, read without ever asking the user unbidden.
//!
//! On macOS an item saved by another build of Trek (one with a different code signature: an
//! older install, a local build) can't be read without the user's login password; reading it
//! the usual way pops "Trek wants to access key "dev.trek.Trek" in your keychain". So every read
//! here is quiet: user interaction is off for the call, and such an item comes back
//! [`Lookup::Locked`] at once for Settings to show. Only [`unlock`] (an Allow access button,
//! after Trek has said what's coming) lets macOS show its prompt.
//!
//! macOS won't let Trek delete or take over that item either (only its data can be overwritten,
//! which leaves it as unreadable as before), so a value saved while a locked item holds its name
//! goes to the next free slot of that name ([`slot_account`]), created by this Trek and readable
//! by it from then on; reads take the first slot that reads.
//!
//! Windows' Credential Manager and Linux's secret service never prompt: there a read is a read.
//! An isolated process (tests, the verify app) keeps everything in memory and never reaches the
//! user's store.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The service every Trek secret is saved under.
pub const SERVICE: &str = "dev.trek.Trek";

/// How many names one secret may take: its own, then `name (2)` … for each locked item in the way.
const SLOTS: usize = 8;

/// What reading a secret found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(String),
    /// Nothing saved under that name.
    Missing,
    /// Saved, but macOS wants the user's permission before Trek may read it (an item another
    /// build of Trek saved). Reading it quietly found that out without asking.
    Locked,
}

impl Lookup {
    pub fn found(self) -> Option<String> {
        match self {
            Lookup::Found(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_locked(&self) -> bool {
        matches!(self, Lookup::Locked)
    }
}

/// Whether a read may let macOS ask the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Never: a locked item reads as locked.
    Quiet,
    /// The user asked (Allow access): macOS may show its password prompt.
    Ask,
}

/// What reading one item found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Value(String),
    Missing,
    Locked,
}

/// The name slot `n` of `account` is saved under: the account itself, then `account (2)`, …
/// (HTTP header and environment variable names have no spaces, so no real account ends so.)
pub fn slot_account(account: &str, n: usize) -> String {
    if n == 0 {
        account.to_string()
    } else {
        format!("{account} ({})", n + 1)
    }
}

/// A secret store, as the decisions below see it.
pub trait Store {
    /// The accounts saved under [`SERVICE`] (their names only: never a prompt); `None` from a
    /// store that can't list (or couldn't), where a secret is only ever its own account.
    fn accounts(&self) -> Option<BTreeSet<String>>;
    fn read(&self, account: &str, access: Access) -> Read;
    /// Save a new item, owned by this app.
    fn add(&self, account: &str, value: &str) -> anyhow::Result<()>;
    /// Delete an item; fails for one this app may not delete (another build's).
    fn delete(&self, account: &str) -> anyhow::Result<()>;
    /// Replace an existing item's data, leaving who may read it as it was (macOS allows this
    /// quietly even for another build's item).
    fn overwrite(&self, account: &str, value: &str) -> anyhow::Result<()>;
}

/// `account`'s slots that exist, in order.
fn present_slots(store: &dyn Store, account: &str) -> Vec<String> {
    let Some(present) = store.accounts() else {
        return vec![account.to_string()];
    };
    (0..SLOTS)
        .map(|n| slot_account(account, n))
        .filter(|a| present.contains(a))
        .collect()
}

/// Read `account`: the first of its slots that reads. Quiet reads never prompt; `Ask` reads the
/// slots quietly first and asks only for a locked one, and what it gets is saved again in a
/// slot of this app's own, so the next read is quiet.
pub fn read_in(store: &dyn Store, account: &str, access: Access) -> Lookup {
    let slots = present_slots(store, account);
    let mut locked = vec![];
    for a in &slots {
        match store.read(a, Access::Quiet) {
            // Empty: a slot `clear_slots` couldn't delete, so it emptied it.
            Read::Value(v) if v.is_empty() => {}
            Read::Value(v) => return Lookup::Found(v),
            Read::Locked => locked.push(a.clone()),
            Read::Missing => {}
        }
    }
    if locked.is_empty() {
        return Lookup::Missing;
    }
    if access == Access::Quiet {
        return Lookup::Locked;
    }
    for a in &locked {
        if let Read::Value(v) = store.read(a, Access::Ask)
            && !v.is_empty()
        {
            // Copied into a slot of its own: allowed once, it isn't asked for again.
            if let Err(e) = write_in(store, account, &v) {
                tracing::warn!("couldn't save {account} again after it was unlocked: {e:#}");
            }
            return Lookup::Found(v);
        }
    }
    Lookup::Locked
}

/// Save `value` as `account`: every slot this app may delete goes (one it may not gets `value`
/// as its data, so it can't hold on to an old key should it ever be allowed), then the value is
/// saved, new, in the first free slot (the account's own name unless a locked item holds it).
pub fn write_in(store: &dyn Store, account: &str, value: &str) -> anyhow::Result<()> {
    let target = clear_slots(store, account, value)
        .into_iter()
        .next()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{SLOTS} locked items named {account} are in the way: remove them in Keychain Access"
            )
        })?;
    store.add(&target, value)
}

/// Delete `account`'s slots that can be deleted, emptying those that can't. A locked one stays
/// (only Keychain Access can remove it) and the account then reads as locked, not missing.
pub fn delete_in(store: &dyn Store, account: &str) {
    clear_slots(store, account, "");
}

/// Delete what can be deleted of `account`'s slots and set the rest's data to `value`; the
/// free slots after that, in order.
fn clear_slots(store: &dyn Store, account: &str, value: &str) -> Vec<String> {
    let Some(mut present) = store.accounts() else {
        // Nothing to step around: the account's own item is replaced in place.
        let _ = store.delete(account);
        return vec![account.to_string()];
    };
    for n in 0..SLOTS {
        let a = slot_account(account, n);
        if !present.contains(&a) {
            continue;
        }
        if store.delete(&a).is_ok() {
            present.remove(&a);
        } else {
            let _ = store.overwrite(&a, value);
        }
    }
    (0..SLOTS)
        .map(|n| slot_account(account, n))
        .filter(|a| !present.contains(a))
        .collect()
}

// ---------- the process's store ----------

/// Accounts the last read found locked (for Settings to say so without reading again).
static LOCKED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn locked_set() -> MutexGuard<'static, BTreeSet<String>> {
    LOCKED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn noted(account: &str, found: Lookup) -> Lookup {
    if found.is_locked() {
        locked_set().insert(account.to_string());
    } else {
        locked_set().remove(account);
    }
    found
}

fn with_store<T>(f: impl FnOnce(&dyn Store) -> T) -> T {
    if crate::paths::isolated() {
        return f(&memory::Memory);
    }
    f(&platform::Platform(SERVICE))
}

/// Read `account` without ever prompting.
pub fn read(account: &str) -> Lookup {
    noted(account, with_store(|s| read_in(s, account, Access::Quiet)))
}

/// Read `account`, letting macOS ask the user for a locked one. Only for a user's explicit
/// action, after Trek has said a prompt is coming; it blocks until they answer, so call it off
/// the main thread.
pub fn unlock(account: &str) -> Lookup {
    noted(account, with_store(|s| read_in(s, account, Access::Ask)))
}

pub fn write(account: &str, value: &str) -> anyhow::Result<()> {
    with_store(|s| write_in(s, account, value))?;
    locked_set().remove(account);
    Ok(())
}

pub fn delete(account: &str) -> anyhow::Result<()> {
    let found = with_store(|s| {
        delete_in(s, account);
        read_in(s, account, Access::Quiet)
    });
    noted(account, found);
    Ok(())
}

/// Whether the last read of `account` found it locked.
pub fn is_locked(account: &str) -> bool {
    locked_set().contains(account)
}

/// The accounts reads have found locked, for an "Allow access" that unlocks them all.
pub fn locked() -> Vec<String> {
    locked_set().iter().cloned().collect()
}

/// Unlock every account found locked (one macOS prompt each, unless the user picks Always
/// Allow); how many of them read now. Off the main thread, as [`unlock`].
pub fn unlock_all() -> usize {
    locked()
        .iter()
        .filter(|a| matches!(unlock(a), Lookup::Found(_)))
        .count()
}

/// The in-memory store of an isolated process. `TREK_TEST_LOCKED_KEYS` (comma-separated
/// accounts) starts it with those saved by "another build" for the verify app to show; tests
/// plant them with [`plant_locked`].
mod memory {
    use super::{Access, Read, Store};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, MutexGuard, PoisonError};

    #[derive(Clone)]
    pub(super) enum Item {
        Ours(String),
        /// Saved by another build: unreadable without asking, never deletable.
        Theirs(String),
    }

    static ITEMS: Mutex<Option<BTreeMap<String, Item>>> = Mutex::new(None);

    pub(super) fn items() -> MutexGuard<'static, Option<BTreeMap<String, Item>>> {
        let mut items = ITEMS.lock().unwrap_or_else(PoisonError::into_inner);
        items.get_or_insert_with(|| {
            std::env::var("TREK_TEST_LOCKED_KEYS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(|a| (a.to_string(), Item::Theirs(format!("{a}-from-before"))))
                .collect()
        });
        items
    }

    pub(super) struct Memory;

    impl Store for Memory {
        fn accounts(&self) -> Option<BTreeSet<String>> {
            Some(items().iter().flatten().map(|(k, _)| k.clone()).collect())
        }

        fn read(&self, account: &str, access: Access) -> Read {
            match items().iter().flatten().find(|(k, _)| *k == account) {
                None => Read::Missing,
                Some((_, Item::Ours(v))) => Read::Value(v.clone()),
                // Asking stands for the user allowing it.
                Some((_, Item::Theirs(v))) if access == Access::Ask => Read::Value(v.clone()),
                Some((_, Item::Theirs(_))) => Read::Locked,
            }
        }

        fn add(&self, account: &str, value: &str) -> anyhow::Result<()> {
            let mut items = items();
            let items = items.get_or_insert_default();
            anyhow::ensure!(!items.contains_key(account), "already saved");
            items.insert(account.to_string(), Item::Ours(value.to_string()));
            Ok(())
        }

        fn delete(&self, account: &str) -> anyhow::Result<()> {
            let mut items = items();
            let items = items.get_or_insert_default();
            match items.get(account) {
                Some(Item::Theirs(_)) => anyhow::bail!("not this app's to delete"),
                _ => {
                    items.remove(account);
                    Ok(())
                }
            }
        }

        fn overwrite(&self, account: &str, value: &str) -> anyhow::Result<()> {
            let mut items = items();
            match items.get_or_insert_default().get_mut(account) {
                Some(Item::Ours(v) | Item::Theirs(v)) => *v = value.to_string(),
                None => anyhow::bail!("not saved"),
            }
            Ok(())
        }
    }
}

/// In an isolated process, save `value` as `account` the way another build of Trek would have:
/// it reads as locked until unlocked. Does nothing outside one.
pub fn plant_locked(account: &str, value: &str) {
    if crate::paths::isolated() {
        memory::items()
            .get_or_insert_default()
            .insert(account.to_string(), memory::Item::Theirs(value.to_string()));
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! The login keychain through Security.framework. Quiet calls run with
    //! `SecKeychainSetUserInteractionAllowed(false)`, the one switch macOS honours for the
    //! access-control prompt of a file-keychain item (`kSecUseAuthenticationUISkip` doesn't
    //! stop it): a locked item then fails with errSecAuthFailed or errSecInteractionNotAllowed
    //! instead of asking.
    use super::{Access, Read, Store};
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    use security_framework::os::macos::keychain::{KeychainUserInteractionLock, SecKeychain};
    use security_framework::passwords;
    use std::collections::BTreeSet;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// The store of one keychain service (`SERVICE`; tests use one of their own).
    pub(super) struct Platform(pub &'static str);

    /// The switch is the whole process's, and each quiet call turns it back on when done, so
    /// quiet calls go one at a time. An `Ask` read takes no part: its prompt, however long it's
    /// up, holds up nothing else.
    static QUIET: Mutex<()> = Mutex::new(());

    /// Fields drop in order: interaction is back on before the next quiet call may start.
    struct Quiet {
        _off: Option<KeychainUserInteractionLock>,
        _turn: MutexGuard<'static, ()>,
    }

    fn quiet() -> Quiet {
        let turn = QUIET.lock().unwrap_or_else(PoisonError::into_inner);
        Quiet {
            _off: SecKeychain::disable_user_interaction().ok(),
            _turn: turn,
        }
    }

    impl Store for Platform {
        fn accounts(&self) -> Option<BTreeSet<String>> {
            let _quiet = quiet();
            let found = ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .service(self.0)
                .load_attributes(true)
                .limit(Limit::All)
                .search();
            match found {
                Ok(items) => Some(
                    items
                        .iter()
                        .filter_map(|i| i.simplify_dict()?.remove("acct"))
                        .collect(),
                ),
                // errSecItemNotFound: nothing saved yet.
                Err(e) if e.code() == -25300 => Some(BTreeSet::new()),
                Err(e) => {
                    tracing::warn!("couldn't list the keychain's Trek items: {e}");
                    None
                }
            }
        }

        fn read(&self, account: &str, access: Access) -> Read {
            let _quiet = match access {
                Access::Quiet => Some(quiet()),
                Access::Ask => {
                    // A quiet call under way has interaction off: let it finish, or macOS
                    // refuses this one without asking.
                    drop(QUIET.lock().unwrap_or_else(PoisonError::into_inner));
                    None
                }
            };
            match passwords::get_generic_password(self.0, account) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(v) => Read::Value(v),
                    Err(_) => Read::Missing,
                },
                Err(e) => super::classify(e.code()),
            }
        }

        fn add(&self, account: &str, value: &str) -> anyhow::Result<()> {
            let _quiet = quiet();
            passwords::set_generic_password(self.0, account, value.as_bytes())?;
            Ok(())
        }

        fn delete(&self, account: &str) -> anyhow::Result<()> {
            let _quiet = quiet();
            passwords::delete_generic_password(self.0, account)?;
            Ok(())
        }

        fn overwrite(&self, account: &str, value: &str) -> anyhow::Result<()> {
            // set_generic_password updates an item that's there (SecItemUpdate).
            self.add(account, value)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    //! Credential Manager on Windows, the secret service on Linux: neither prompts, and
    //! neither lists, so a secret is only ever its own account.
    use super::{Access, Read, Store};
    use std::collections::BTreeSet;

    /// The store of one service (`SERVICE`).
    pub(super) struct Platform(pub &'static str);

    impl Store for Platform {
        fn accounts(&self) -> Option<BTreeSet<String>> {
            None
        }

        fn read(&self, account: &str, _: Access) -> Read {
            match keyring::Entry::new(self.0, account).and_then(|e| e.get_password()) {
                Ok(v) => Read::Value(v),
                Err(_) => Read::Missing,
            }
        }

        fn add(&self, account: &str, value: &str) -> anyhow::Result<()> {
            keyring::Entry::new(self.0, account)?.set_password(value)?;
            Ok(())
        }

        fn delete(&self, account: &str) -> anyhow::Result<()> {
            keyring::Entry::new(self.0, account)?.delete_credential()?;
            Ok(())
        }

        fn overwrite(&self, account: &str, value: &str) -> anyhow::Result<()> {
            self.add(account, value)
        }
    }
}

/// What a Security.framework status means for a read. errSecItemNotFound: nothing saved.
/// errSecAuthFailed (a quiet read of an item another build saved), errSecInteractionNotAllowed
/// (the same, or the keychain itself locked) and errSecUserCanceled (Deny at the prompt): saved
/// but not readable now. Anything else is treated as nothing to read.
pub fn classify(status: i32) -> Read {
    match status {
        -25300 => Read::Missing,
        -25293 | -25308 | -128 => Read::Locked,
        other => {
            tracing::warn!("keychain read failed with status {other}");
            Read::Missing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    /// A keychain as macOS behaves: items another build saved can't be read quietly or deleted,
    /// only overwritten; asking for one is a prompt the user answers with `allow`.
    #[derive(Default)]
    struct Fake {
        /// account → (value, saved by another build, always allowed)
        items: RefCell<BTreeMap<String, (String, bool, bool)>>,
        prompts: Cell<usize>,
        allow: bool,
        /// Credential Manager: no listing.
        cannot_list: bool,
    }

    impl Fake {
        fn theirs(self, account: &str, value: &str) -> Self {
            self.items
                .borrow_mut()
                .insert(account.into(), (value.into(), true, false));
            self
        }
        fn ours(self, account: &str, value: &str) -> Self {
            self.items
                .borrow_mut()
                .insert(account.into(), (value.into(), false, false));
            self
        }
        fn names(&self) -> Vec<String> {
            self.items.borrow().keys().cloned().collect()
        }
    }

    impl Store for Fake {
        fn accounts(&self) -> Option<BTreeSet<String>> {
            (!self.cannot_list).then(|| self.items.borrow().keys().cloned().collect())
        }
        fn read(&self, account: &str, access: Access) -> Read {
            let mut items = self.items.borrow_mut();
            let Some((v, theirs, always)) = items.get_mut(account) else {
                return Read::Missing;
            };
            if !*theirs || *always {
                return Read::Value(v.clone());
            }
            if access == Access::Quiet {
                return Read::Locked;
            }
            self.prompts.set(self.prompts.get() + 1);
            if self.allow {
                Read::Value(v.clone())
            } else {
                Read::Locked
            }
        }
        fn add(&self, account: &str, value: &str) -> anyhow::Result<()> {
            let mut items = self.items.borrow_mut();
            if let Some(item) = items.get_mut(account) {
                item.0 = value.into();
            } else {
                items.insert(account.into(), (value.into(), false, false));
            }
            Ok(())
        }
        fn delete(&self, account: &str) -> anyhow::Result<()> {
            let mut items = self.items.borrow_mut();
            match items.get(account) {
                Some((_, true, _)) => anyhow::bail!("errSecInvalidOwnerEdit"),
                Some(_) => {
                    items.remove(account);
                    Ok(())
                }
                None => anyhow::bail!("errSecItemNotFound"),
            }
        }
        fn overwrite(&self, account: &str, value: &str) -> anyhow::Result<()> {
            match self.items.borrow_mut().get_mut(account) {
                Some(item) => item.0 = value.into(),
                None => anyhow::bail!("errSecItemNotFound"),
            }
            Ok(())
        }
    }

    #[test]
    fn a_quiet_read_never_prompts_for_another_builds_key() {
        let kc = Fake::default().theirs("openrouter", "sk-old");
        assert_eq!(read_in(&kc, "openrouter", Access::Quiet), Lookup::Locked);
        assert_eq!(read_in(&kc, "anthropic", Access::Quiet), Lookup::Missing);
        assert_eq!(kc.prompts.get(), 0, "no prompt without the user asking");
    }

    #[test]
    fn allowing_once_saves_a_copy_trek_can_read_from_then_on() {
        let kc = Fake {
            allow: true,
            ..Fake::default()
        }
        .theirs("openrouter", "sk-old");
        assert_eq!(
            read_in(&kc, "openrouter", Access::Ask),
            Lookup::Found("sk-old".into())
        );
        assert_eq!(kc.prompts.get(), 1);
        // The copy is Trek's own, beside the item it can't take over.
        assert_eq!(kc.names(), ["openrouter", "openrouter (2)"]);
        assert_eq!(
            read_in(&kc, "openrouter", Access::Quiet),
            Lookup::Found("sk-old".into())
        );
        // Asking again reads the copy quietly: no second prompt.
        assert_eq!(
            read_in(&kc, "openrouter", Access::Ask),
            Lookup::Found("sk-old".into())
        );
        assert_eq!(kc.prompts.get(), 1);
    }

    #[test]
    fn deny_leaves_it_locked_and_saves_nothing() {
        let kc = Fake::default().theirs("openrouter", "sk-old");
        assert_eq!(read_in(&kc, "openrouter", Access::Ask), Lookup::Locked);
        assert_eq!(kc.prompts.get(), 1);
        assert_eq!(kc.names(), ["openrouter"]);
    }

    #[test]
    fn a_key_entered_again_goes_beside_the_locked_one_and_reads_quietly() {
        let kc = Fake::default().theirs("openrouter", "sk-old");
        write_in(&kc, "openrouter", "sk-new").unwrap();
        assert_eq!(
            read_in(&kc, "openrouter", Access::Quiet),
            Lookup::Found("sk-new".into())
        );
        assert_eq!(kc.prompts.get(), 0);
        // The locked one now holds the new key too: allowed some day, it can't bring back the old.
        assert_eq!(kc.items.borrow()["openrouter"].0, "sk-new");
    }

    #[test]
    fn an_always_allowed_item_trek_cant_delete_doesnt_keep_an_old_key() {
        let kc = Fake::default().theirs("openrouter", "sk-old");
        kc.items.borrow_mut().get_mut("openrouter").unwrap().2 = true;
        write_in(&kc, "openrouter", "sk-new").unwrap();
        assert_eq!(
            read_in(&kc, "openrouter", Access::Quiet),
            Lookup::Found("sk-new".into())
        );
        // And Remove empties it: the key is gone, not back to the old one.
        delete_in(&kc, "openrouter");
        assert_eq!(read_in(&kc, "openrouter", Access::Quiet), Lookup::Missing);
    }

    #[test]
    fn remove_deletes_treks_own_and_says_what_is_still_locked() {
        let kc = Fake::default().ours("mine", "v");
        delete_in(&kc, "mine");
        assert_eq!(read_in(&kc, "mine", Access::Quiet), Lookup::Missing);
        assert!(kc.names().is_empty());

        let kc = Fake::default().theirs("openrouter", "sk-old");
        write_in(&kc, "openrouter", "sk-new").unwrap();
        delete_in(&kc, "openrouter");
        assert_eq!(kc.names(), ["openrouter"], "Trek's copy went");
        assert_eq!(read_in(&kc, "openrouter", Access::Quiet), Lookup::Locked);
    }

    #[test]
    fn a_store_that_cant_list_keeps_one_item_per_secret() {
        let kc = Fake {
            cannot_list: true,
            ..Fake::default()
        };
        assert_eq!(read_in(&kc, "deepseek", Access::Quiet), Lookup::Missing);
        write_in(&kc, "deepseek", "a").unwrap();
        write_in(&kc, "deepseek", "b").unwrap();
        assert_eq!(kc.names(), ["deepseek"]);
        assert_eq!(
            read_in(&kc, "deepseek", Access::Quiet),
            Lookup::Found("b".into())
        );
        delete_in(&kc, "deepseek");
        assert_eq!(read_in(&kc, "deepseek", Access::Quiet), Lookup::Missing);
    }

    #[test]
    fn security_framework_statuses() {
        assert_eq!(classify(-25300), Read::Missing, "errSecItemNotFound");
        assert_eq!(classify(-25293), Read::Locked, "errSecAuthFailed");
        assert_eq!(
            classify(-25308),
            Read::Locked,
            "errSecInteractionNotAllowed"
        );
        assert_eq!(classify(-128), Read::Locked, "errSecUserCanceled");
        assert_eq!(classify(-50), Read::Missing);
    }

    /// The real login keychain, under a service of the test's own (never Trek's): an item
    /// `/usr/bin/security` saves is one this test binary isn't allowed to read, as an older
    /// build's is for Trek. Run by hand: `cargo test -p trek-core keychain -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "uses the login keychain (a throwaway service), run by hand"]
    fn the_login_keychain_reads_another_apps_item_as_locked_without_a_prompt() {
        const TEST_SERVICE: &str = "dev.trek.Trek.locktest";
        let security = |args: &[&str]| {
            std::process::Command::new("/usr/bin/security")
                .args(args)
                .output()
                .unwrap()
        };
        let clean = || {
            for n in 0..SLOTS {
                let a = slot_account("probe", n);
                while security(&["delete-generic-password", "-s", TEST_SERVICE, "-a", &a])
                    .status
                    .success()
                {}
            }
        };
        clean();
        assert!(
            security(&[
                "add-generic-password",
                "-s",
                TEST_SERVICE,
                "-a",
                "probe",
                "-w",
                "theirs"
            ])
            .status
            .success()
        );
        // On a thread, so a prompt (the bug) fails the test instead of waiting for a click.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let kc = platform::Platform(TEST_SERVICE);
            let first = read_in(&kc, "probe", Access::Quiet);
            write_in(&kc, "probe", "mine").unwrap();
            let after_write = read_in(&kc, "probe", Access::Quiet);
            let names = kc.accounts();
            delete_in(&kc, "probe");
            let after_delete = read_in(&kc, "probe", Access::Quiet);
            let _ = tx.send((first, after_write, names, after_delete));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(20));
        clean();
        let (first, after_write, names, after_delete) =
            got.expect("a keychain call waited for the user: it prompted");
        assert_eq!(first, Lookup::Locked);
        assert_eq!(after_write, Lookup::Found("mine".into()));
        assert_eq!(
            names.unwrap().into_iter().collect::<Vec<_>>(),
            ["probe", "probe (2)"]
        );
        assert_eq!(after_delete, Lookup::Locked);
    }

    #[test]
    fn the_isolated_store_is_quiet_and_remembers_what_is_locked() {
        assert!(crate::paths::isolated(), "never the user's keychain");
        let account = format!("test-locked:{}", uuid::Uuid::new_v4());
        plant_locked(&account, "from-before");
        assert_eq!(read(&account), Lookup::Locked);
        assert!(is_locked(&account) && locked().contains(&account));
        assert_eq!(unlock(&account), Lookup::Found("from-before".into()));
        assert!(!is_locked(&account));
        assert_eq!(read(&account), Lookup::Found("from-before".into()));
        write(&account, "new").unwrap();
        assert_eq!(read(&account), Lookup::Found("new".into()));
        delete(&account).unwrap();
        assert_eq!(read(&account), Lookup::Locked, "the planted item stays");
    }
}
