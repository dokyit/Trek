//! Grants that can't be forged from the file system. `state.toml` is a file in Trek's data
//! folder, which an agent with file access can write; so the permissions a plugin was allowed
//! count only with a seal beside them: an HMAC-SHA256, under a key this install keeps in the
//! platform's secret store (`crate::keychain`: the Keychain, Credential Manager), over the
//! plugin's id, the hash of every file in its folder, and the permissions allowed.
//!
//! A grant written by hand has no valid seal and counts for nothing: the plugin waits in
//! Settings › Plugins for the user. A seal is for one plugin as it was when the user allowed it:
//! change its manifest's permissions or any of its files (an update, or another plugin put in its
//! place) and the seal no longer matches, so it asks again.

use super::{MAX_FILES, Permission};
use sha2::{Digest, Sha256};
use std::path::Path;

/// The secret store's name for the key grants are sealed with.
pub const KEY_ACCOUNT: &str = "plugin-grants";

/// A seal's key: 32 bytes.
pub type Key = [u8; 32];

fn from_hex(text: &str) -> Option<Key> {
    let bytes = hex::decode(text.trim()).ok()?;
    bytes.try_into().ok()
}

/// This install's key, made the first time it's needed. `None` when the secret store can't be
/// read (an item another build of Trek saved, a store that's locked): then nothing can be
/// verified, and every plugin that asks for permissions waits.
pub fn key() -> Option<Key> {
    use crate::keychain::{self, Lookup};
    match keychain::read(KEY_ACCOUNT) {
        Lookup::Found(text) => from_hex(&text),
        Lookup::Locked => None,
        Lookup::Missing => {
            // Two v4 UUIDs: 32 bytes from the system's random source.
            let mut key = [0u8; 32];
            key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            keychain::write(KEY_ACCOUNT, &hex::encode(key)).ok()?;
            // Read back: what the store holds is the key, whatever it did with the write.
            from_hex(&keychain::read(KEY_ACCOUNT).found()?)
        }
    }
}

/// HMAC-SHA256 (RFC 2104) of `message` under `key`.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let (mut inner, mut outer) = (Sha256::new(), Sha256::new());
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The hash of everything in plugin folder `dir`: each file's path and bytes, in path order
/// (hidden files and folders left out, as the folder's watch leaves them out). A plugin with
/// more than [`MAX_FILES`] files, or one that can't be read, has no hash: its grant can't be
/// sealed or verified.
pub fn content_hash(dir: &Path) -> Option<String> {
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for e in walkdir::WalkDir::new(dir)
        .max_depth(6)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            e.depth() == 0 || !(n.starts_with('.') || n == "node_modules")
        })
    {
        let e = e.ok()?;
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e
            .path()
            .strip_prefix(dir)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        files.push((rel, e.path().to_path_buf()));
        if files.len() > MAX_FILES {
            return None;
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path).ok()?;
        hash.update((rel.len() as u64).to_le_bytes());
        hash.update(rel.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Some(hex::encode(hash.finalize()))
}

/// The seal for plugin `id`, whose files hash to `hash`, allowed `permissions`.
pub fn seal(key: &Key, id: &str, hash: &str, permissions: &[Permission]) -> String {
    let mut keys: Vec<String> = permissions.iter().map(Permission::key).collect();
    keys.sort();
    keys.dedup();
    let message = format!("trek-plugin-grant-v1\0{id}\0{hash}\0{}", keys.join("\n"));
    hex::encode(hmac(key, message.as_bytes()))
}

/// Whether `given` is the seal for that plugin and those permissions (compared without
/// stopping at the first difference).
pub fn verify(key: &Key, id: &str, hash: &str, permissions: &[Permission], given: &str) -> bool {
    let want = seal(key, id, hash, permissions);
    let (a, b) = (want.as_bytes(), given.trim().as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_is_rfc_4231s() {
        // Test case 2 of RFC 4231.
        assert_eq!(
            hex::encode(hmac(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 1: a 20-byte key of 0x0b.
        assert_eq!(
            hex::encode(hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // A key longer than a block is hashed first (test case 6).
        assert_eq!(
            hex::encode(hmac(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_seal_is_for_one_plugin_its_files_and_its_permissions() {
        let key = [7u8; 32];
        let perms = [Permission::ThreadsRead, Permission::UsageRead];
        let s = seal(&key, "burn", "abc", &perms);
        assert!(verify(&key, "burn", "abc", &perms, &s));
        // The order they're listed in doesn't matter.
        let swapped = [Permission::UsageRead, Permission::ThreadsRead];
        assert!(verify(&key, "burn", "abc", &swapped, &s));
        // Another plugin, other files, more or fewer permissions, another install: no.
        assert!(!verify(&key, "burn2", "abc", &perms, &s));
        assert!(!verify(&key, "burn", "abd", &perms, &s));
        assert!(!verify(&key, "burn", "abc", &perms[..1], &s));
        let more = [
            Permission::ThreadsRead,
            Permission::UsageRead,
            Permission::ThreadsSend,
        ];
        assert!(!verify(&key, "burn", "abc", &more, &s));
        assert!(!verify(&[8u8; 32], "burn", "abc", &perms, &s));
        assert!(!verify(&key, "burn", "abc", &perms, ""));
        assert!(!verify(&key, "burn", "abc", &perms, "not a seal"));
    }

    #[test]
    fn the_hash_is_of_every_file_and_changes_with_any_of_them() {
        let dir = std::env::temp_dir().join(format!(
            "trek-grant-hash-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("plugin.toml"), "name = \"X\"\n").unwrap();
        std::fs::write(dir.join("ui/a.json"), "{}").unwrap();
        let first = content_hash(&dir).unwrap();
        assert_eq!(
            content_hash(&dir).unwrap(),
            first,
            "the same files, the same hash"
        );
        std::fs::write(dir.join("ui/a.json"), "{ }").unwrap();
        let edited = content_hash(&dir).unwrap();
        assert_ne!(edited, first, "a file edited");
        std::fs::write(dir.join("main.js"), "").unwrap();
        assert_ne!(
            content_hash(&dir).unwrap(),
            edited,
            "a file added, even an empty one"
        );
        // A file moved isn't the same plugin either.
        std::fs::remove_file(dir.join("main.js")).unwrap();
        std::fs::rename(dir.join("ui/a.json"), dir.join("ui/b.json")).unwrap();
        assert_ne!(content_hash(&dir).unwrap(), edited);
        // What the watch leaves out, the hash leaves out.
        let before = content_hash(&dir).unwrap();
        std::fs::write(dir.join(".DS_Store"), "x").unwrap();
        assert_eq!(content_hash(&dir).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_key_is_made_once_and_kept() {
        assert!(crate::paths::isolated(), "never the user's keychain");
        let a = key().expect("a key");
        assert_eq!(key(), Some(a));
        assert_ne!(a, [0u8; 32]);
    }
}
