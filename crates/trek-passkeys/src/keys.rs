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
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
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

/// A new P-256 key from the system's random source.
fn new_signing_key() -> Result<SigningKey, KeyError> {
    for _ in 0..8 {
        let mut scalar = [0u8; 32];
        getrandom::fill(&mut scalar)
            .map_err(|e| KeyError::Failed(format!("no random numbers: {e}")))?;
        // Zero and anything at or past the curve's order are no key (one in 2^32 at worst).
        if let Ok(key) = SigningKey::from_slice(&scalar) {
            return Ok(key);
        }
    }
    Err(KeyError::Failed("couldn't make a key".into()))
}

fn point_of(key: &SigningKey) -> [u8; 65] {
    let encoded = key.verifying_key().to_encoded_point(false);
    let mut point = [0u8; 65];
    point.copy_from_slice(encoded.as_bytes());
    point
}

fn sign_with(key: &SigningKey, message: &[u8]) -> Vec<u8> {
    let signature: Signature = key.sign(message);
    signature.to_der().as_bytes().to_vec()
}

fn key_from(scalar: &str) -> Result<SigningKey, KeyError> {
    unb64(scalar)
        .and_then(|bytes| SigningKey::from_slice(&bytes).ok())
        .ok_or_else(|| KeyError::Failed("the passkey's key can't be read".into()))
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
        let key = new_signing_key()?;
        Ok(NewKey {
            handle: format!("plain:{}", b64(&key.to_bytes())),
            point: point_of(&key),
        })
    }

    fn sign(&self, handle: &str, message: &[u8], _reason: &str) -> Result<Vec<u8>, KeyError> {
        self.verify()?;
        let scalar = handle.strip_prefix("plain:").ok_or_else(|| {
            KeyError::Failed("this passkey's key isn't kept in this profile".into())
        })?;
        Ok(sign_with(&key_from(scalar)?, message))
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
        let key = new_signing_key()?;
        let mut name = [0u8; 12];
        getrandom::fill(&mut name).map_err(|e| KeyError::Failed(e.to_string()))?;
        let account = format!("passkey {}", b64(&name));
        keychain::write(&account, &b64(&key.to_bytes()))
            .map_err(|e| KeyError::Failed(format!("the Keychain didn't take the key: {e:#}")))?;
        Ok(NewKey {
            handle: format!("{PREFIX}{account}"),
            point: point_of(&key),
        })
    }

    pub fn sign(handle: &str, message: &[u8]) -> Result<Vec<u8>, KeyError> {
        let account = handle.strip_prefix(PREFIX).unwrap_or_default();
        match keychain::read(account) {
            Lookup::Found(scalar) => Ok(sign_with(&key_from(&scalar)?, message)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier as _;

    #[test]
    fn the_stand_in_makes_keys_that_sign_and_obeys_its_switch() {
        // trek-core's paths are isolated in every test process that links it.
        trek_core::paths::isolate(std::env::temp_dir().join("trek-passkeys-tests"));
        let auth = platform().expect("an isolated process has the stand-in");
        assert!(auth.stand_in());
        let key = auth.create("make a key").unwrap();
        assert!(key.handle.starts_with("plain:"));
        assert_eq!(key.point[0], 4);
        let der = auth.sign(&key.handle, b"message", "sign").unwrap();
        let public = VerifyingKey::from_sec1_bytes(&key.point).unwrap();
        let signature = Signature::from_der(&der).unwrap();
        assert!(public.verify(b"message", &signature).is_ok());
        assert!(public.verify(b"another", &signature).is_err());
        // Another key's handle signs as that key, a handle of another kind not at all.
        let other = auth.create("another").unwrap();
        assert_ne!(other.point, key.point);
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
