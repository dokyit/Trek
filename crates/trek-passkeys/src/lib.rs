//! Trek's own passkey authenticator, for the Browser on a Mac.
//!
//! WebKit gives a page passkeys only in an app that carries Apple's managed web-browser
//! entitlement, which needs an Apple-issued provisioning profile; Trek, signed with its own
//! certificate, can't have it, and `navigator.credentials` there fails every request. So the
//! Browser answers those requests itself: the page's request goes over a narrow bridge to native
//! code (`trek-app`'s `panels/browser/passkeys.rs`), which takes the page's origin from WebKit,
//! checks the request against it here ([`ceremony::parse`], [`rp`]), asks the user, and has an
//! [`Authenticator`] verify them (Touch ID) and sign with a key that never leaves the Mac's
//! Secure Enclave.
//!
//! These passkeys are Trek's: they are not the ones in iCloud Keychain or a password manager, and
//! they stay on this computer ([`store`]). A site gets one when the user adds a passkey to their
//! account from Trek's Browser, and takes it at sign-in from then on.
//!
//! What is here needs no web view and is tested against the specifications' own vectors: the
//! rpId and origin rules ([`rp`]), CBOR and COSE ([`cbor`], [`wire`]), `clientDataJSON` and the
//! authenticator data ([`wire`]), and the two ceremonies end to end as a relying party checks
//! them ([`ceremony`]).

pub mod cbor;
pub mod ceremony;
#[cfg(target_os = "macos")]
mod enclave;
pub mod keys;
pub mod rp;
pub mod store;
pub mod wire;

pub use ceremony::{Create, Error, Get, Request, parse};
pub use keys::{Authenticator, KeyError, platform, set_stand_in};
pub use rp::Origin;
pub use store::{Passkey, Store};

/// Where a passkey's key is kept, in words, for the list in Settings.
pub fn kept_in(passkey: &Passkey) -> &'static str {
    match passkey.key.split_once(':').map(|(kind, _)| kind) {
        Some("se") => "Secure Enclave",
        Some("keychain") => "Keychain",
        Some("plain") => "Test profile",
        _ => "Unknown",
    }
}

/// Delete passkey `id` from `store`, and what's kept for its key. The site still lists it until
/// it's removed from the account there.
pub fn delete(store: &Store, id: &str) -> Result<Option<Passkey>, String> {
    let removed = store.remove(id)?;
    if let (Some(passkey), Some(auth)) = (&removed, platform()) {
        auth.forget(&passkey.key);
    }
    Ok(removed)
}

/// Seconds since 1970, now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
