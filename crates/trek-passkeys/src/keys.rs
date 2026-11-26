//! Where a passkey's private key is kept and who may use it. An [`Authenticator`] verifies the
//! user and then makes or uses a key; each key is named by a handle the passkey list stores:
//!
//! - `se:…` a key the Mac's Secure Enclave made and holds (`enclave.rs`). The handle is the
//!   enclave's own wrapped blob: useless on any other Mac, and on this one only with Touch ID or
//!   the account's password, which the enclave itself asks for on every signature.
//! - `keychain:…` a key in the login Keychain, for a Mac without a Secure Enclave. Touch ID or
//!   the password is asked for by Trek before each use.
//! - `plain:…` the key itself, in the handle. Only an isolated process (tests, the verify app,
//!   whose data folder is thrown away) makes or uses these, with a stand-in for the user
//!   verification ([`set_stand_in`]): it never shows a prompt and never reaches the Keychain.

use crate::wire::{b64, unb64};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _,
    UnparsedPublicKey,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Why a key couldn't be made or used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The user said no at the verification prompt (or it timed out).
    Cancelled,
    /// The key is gone or isn't this computer's, or the computer can't verify anyone.
    Failed(String),
}

/// A key just made: how to name it later, and its public half (`04 || x || y`).
pub struct NewKey {
    pub handle: String,
    pub point: [u8; 65],
}

/// Something that holds passkeys' private keys and verifies the user before each use.
pub trait Authenticator: Send + Sync {
    /// Verify the user, then make a key. `reason` finishes the system's sentence, "Trek is
    /// trying to …". This waits on the user: never call it on the main thread.
    fn create(&self, reason: &str) -> Result<NewKey, KeyError>;

    /// Verify the user, then sign `message` with the key `handle` names: ECDSA over its
    /// SHA-256, DER encoded. Waits on the user like `create`.
    fn sign(&self, handle: &str, message: &[u8], reason: &str) -> Result<Vec<u8>, KeyError>;

    /// The key's passkey was deleted: let go of what's kept for it.
    fn forget(&self, handle: &str);

    /// Whether the user is verified by a person (Touch ID, a password) or by the stand-in.
    fn stand_in(&self) -> bool {
        false
    }
}

/// What the stand-in answers when asked to verify the user (an isolated process only).
static STAND_IN_ALLOWS: AtomicBool = AtomicBool::new(true);

/// Make the stand-in verification say yes or no from now on: a test's way to be the person at
/// the Touch ID sensor, or the one who cancels.
pub fn set_stand_in(allows: bool) {
    STAND_IN_ALLOWS.store(allows, Ordering::Relaxed);
}

/// The authenticator this process uses, or `None` where Trek has none of its own (Windows, where
/// WebView2 has Windows Hello; Linux).
pub fn platform() -> Option<Arc<dyn Authenticator>> {
    if trek_core::paths::isolated() {
        return Some(Arc::new(StandIn));
    }
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(crate::enclave::Mac))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// A software key: a P-256 key pair with what keeps it (its PKCS#8 document, base64url).
struct SoftKey {
    pair: EcdsaKeyPair,
    kept: String,
}

impl SoftKey {
    /// A new key from the system's random source.
    fn new() -> Result<SoftKey, KeyError> {
        let failed = |_| KeyError::Failed("couldn't make a key".into());
        let document =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .map_err(failed)?;
        SoftKey::from(&b64(document.as_ref()))
    }

    /// The key `kept` holds.
    fn from(kept: &str) -> Result<SoftKey, KeyError> {
        let unreadable = || KeyError::Failed("the passkey's key can't be read".into());
        let document = unb64(kept).ok_or_else(unreadable)?;
        let pair = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &document,
            &SystemRandom::new(),
        )
        .map_err(|_| unreadable())?;
        Ok(SoftKey {
            pair,
            kept: kept.to_string(),
        })
    }

    /// Its public half, `04 || x || y`.
    fn point(&self) -> Result<[u8; 65], KeyError> {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| KeyError::Failed("the public key isn't a P-256 point".into()))
    }

    /// ECDSA over `message`'s SHA-256, DER encoded.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, KeyError> {
        self.pair
            .sign(&SystemRandom::new(), message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| KeyError::Failed("couldn't sign".into()))
    }

    fn new_key(self, prefix: &str, named: &str) -> Result<NewKey, KeyError> {
        Ok(NewKey {
            handle: format!("{prefix}{named}"),
            point: self.point()?,
        })
    }
}

/// Whether `signature` (DER) is the key `point`'s over `message`: what a relying party checks,
/// and every test of a signature here.
pub fn verifies(point: &[u8], message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
        .verify(message, signature)
        .is_ok()
}

/// The isolated process's authenticator: keys in their handles, and [`set_stand_in`] for the
/// user. It refuses to exist anywhere else.
pub struct StandIn;

impl StandIn {
    fn verify(&self) -> Result<(), KeyError> {
        if !trek_core::paths::isolated() {
            return Err(KeyError::Failed(
                "the stand-in authenticator is for isolated runs only".into(),
            ));
        }
        if STAND_IN_ALLOWS.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(KeyError::Cancelled)
        }
    }
}

impl Authenticator for StandIn {
    fn create(&self, _reason: &str) -> Result<NewKey, KeyError> {
        self.verify()?;
        let key = SoftKey::new()?;
        let kept = key.kept.clone();
        key.new_key("plain:", &kept)
    }

    fn sign(&self, handle: &str, message: &[u8], _reason: &str) -> Result<Vec<u8>, KeyError> {
        self.verify()?;
        let kept = handle.strip_prefix("plain:").ok_or_else(|| {
            KeyError::Failed("this passkey's key isn't kept in this profile".into())
        })?;
        SoftKey::from(kept)?.sign(message)
    }

    fn forget(&self, _handle: &str) {}

    fn stand_in(&self) -> bool {
        true
    }
}

/// Keys in the login Keychain (through Trek's own quiet reads): what a Mac without a Secure
/// Enclave falls back to. The caller verifies the user first.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod keychain {
    use super::*;
    use trek_core::keychain::{self, Lookup};

    const PREFIX: &str = "keychain:";

    pub fn is(handle: &str) -> bool {
        handle.starts_with(PREFIX)
    }

    pub fn create() -> Result<NewKey, KeyError> {
        let key = SoftKey::new()?;
        let mut name = [0u8; 12];
        getrandom::fill(&mut name).map_err(|e| KeyError::Failed(e.to_string()))?;
        let account = format!("passkey {}", b64(&name));
        keychain::write(&account, &key.kept)
            .map_err(|e| KeyError::Failed(format!("the Keychain didn't take the key: {e:#}")))?;
        key.new_key(PREFIX, &account)
    }

    pub fn sign(handle: &str, message: &[u8]) -> Result<Vec<u8>, KeyError> {
        let account = handle.strip_prefix(PREFIX).unwrap_or_default();
        match keychain::read(account) {
            Lookup::Found(kept) => SoftKey::from(&kept)?.sign(message),
            Lookup::Missing => Err(KeyError::Failed(
                "this passkey's key is no longer in the Keychain".into(),
            )),
            Lookup::Locked => Err(KeyError::Failed(
                "the Keychain holds this passkey's key for another copy of Trek".into(),
            )),
        }
    }

    pub fn forget(handle: &str) {
        if let Some(account) = handle.strip_prefix(PREFIX) {
            let _ = keychain::delete(account);
        }
    }
}

/// For this crate's tests: an isolated process with the stand-in saying yes, and the turn to
/// use it. The switch is the process's and tests run side by side, so every test that asks the
/// stand-in to verify (or flips it) holds this while it does: one that didn't had its yes turned
/// into a no by another's "the user cancels".
#[cfg(test)]
pub(crate) fn stand_in_for_test() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    trek_core::paths::isolate(std::env::temp_dir().join("trek-passkeys-tests"));
    let guard = TURN.lock().unwrap_or_else(|e| e.into_inner());
    set_stand_in(true);
    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_makes_keys_that_sign_and_obeys_its_switch() {
        let _turn = stand_in_for_test();
        let auth = platform().expect("an isolated process has the stand-in");
        assert!(auth.stand_in());
        let key = auth.create("make a key").unwrap();
        assert!(key.handle.starts_with("plain:"));
        assert_eq!(key.point[0], 4);
        let der = auth.sign(&key.handle, b"message", "sign").unwrap();
        assert!(verifies(&key.point, b"message", &der));
        assert!(!verifies(&key.point, b"another", &der));
        assert!(!verifies(&key.point, b"message", &der[..der.len() - 1]));
        // Another key's handle signs as that key, a handle of another kind not at all.
        let other = auth.create("another").unwrap();
        assert_ne!(other.point, key.point);
        assert!(!verifies(&other.point, b"message", &der));
        // Told to say no (the person who cancels at the sensor), it makes and signs nothing.
        set_stand_in(false);
        assert!(matches!(auth.create("a third"), Err(KeyError::Cancelled)));
        assert!(matches!(
            auth.sign(&key.handle, b"message", "sign"),
            Err(KeyError::Cancelled)
        ));
        set_stand_in(true);
        assert!(matches!(
            auth.sign("se:AAAA", b"m", "sign"),
            Err(KeyError::Failed(_))
        ));
        assert!(matches!(
            auth.sign("plain:AAAA", b"m", "sign"),
            Err(KeyError::Failed(_))
        ));
    }
}
