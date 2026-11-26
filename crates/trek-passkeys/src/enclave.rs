//! The Mac's authenticator: keys the Secure Enclave makes and never gives up, and Touch ID (or
//! the account's password) before each use.
//!
//! A key is made in the enclave without being added to the Keychain (which, for enclave keys,
//! needs an entitlement Trek's signature can't carry). What macOS hands back for it is the
//! enclave's own wrapped blob (the key's `toid` attribute, as CryptoKit's `dataRepresentation`
//! is): Trek keeps that in its passkey list, and gives it back to the enclave to sign. The blob
//! is no use on another Mac, and on this one the enclave itself demands the user's presence for
//! every signature (the key's access control: `userPresence`), whoever asks.
//!
//! On a Mac with no Secure Enclave the key goes to the login Keychain instead and Trek asks for
//! Touch ID or the password itself before each use (`keys::keychain`).

use crate::keys::{Authenticator, KeyError, NewKey, keychain};
use crate::wire::{b64, unb64};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_foundation::{NSData, NSError, NSNumber, NSString};
use objc2_local_authentication::{LAContext, LAPolicy};
use std::ffi::c_void;

type Ref = *const c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecAttrKeyType: Ref;
    static kSecAttrKeyTypeECSECPrimeRandom: Ref;
    static kSecAttrKeySizeInBits: Ref;
    static kSecAttrKeyClass: Ref;
    static kSecAttrKeyClassPrivate: Ref;
    static kSecAttrTokenID: Ref;
    static kSecAttrTokenIDSecureEnclave: Ref;
    static kSecPrivateKeyAttrs: Ref;
    static kSecAttrIsPermanent: Ref;
    static kSecAttrAccessControl: Ref;
    static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: Ref;
    static kSecUseAuthenticationContext: Ref;
    static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: Ref;

    fn SecAccessControlCreateWithFlags(
        allocator: Ref,
        protection: Ref,
        flags: usize,
        error: *mut Ref,
    ) -> Ref;
    fn SecKeyCreateRandomKey(parameters: Ref, error: *mut Ref) -> Ref;
    fn SecKeyCreateWithData(data: Ref, attributes: Ref, error: *mut Ref) -> Ref;
    fn SecKeyCopyPublicKey(key: Ref) -> Ref;
    fn SecKeyCopyExternalRepresentation(key: Ref, error: *mut Ref) -> Ref;
    fn SecKeyCopyAttributes(key: Ref) -> Ref;
    fn SecKeyCreateSignature(key: Ref, algorithm: Ref, data: Ref, error: *mut Ref) -> Ref;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: Ref);
}

/// `kSecAccessControlUserPresence`: Touch ID, or the account's password.
const USER_PRESENCE: usize = 1 << 0;
/// `kSecAccessControlPrivateKeyUsage`: the enclave signs with it.
const PRIVATE_KEY_USAGE: usize = 1 << 30;

/// A Core Foundation object this code owns.
struct Owned(Ref);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: created by a Create or Copy function, released once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// What a Security call reported, and whether it was the user saying no.
fn key_error(error: Ref, what: &str) -> KeyError {
    if error.is_null() {
        return KeyError::Failed(format!("{what} failed"));
    }
    let owned = Owned(error);
    // SAFETY: a CFError is an NSError.
    let error: &NSError = unsafe { &*(owned.0 as *const NSError) };
    let (domain, code) = (error.domain().to_string(), error.code());
    if cancelled(&domain, code) {
        return KeyError::Cancelled;
    }
    KeyError::Failed(format!(
        "{what} failed: {} ({domain} {code})",
        error.localizedDescription()
    ))
}

/// Whether an error is the user (or the system, for them) calling the prompt off:
/// LocalAuthentication's userCancel, userFallback, systemCancel and appCancel, and Security's
/// errSecUserCanceled. The same numbers mean other things in other domains.
fn cancelled(domain: &str, code: isize) -> bool {
    match domain {
        "com.apple.LocalAuthentication" => matches!(code, -2 | -3 | -4 | -9),
        "NSOSStatusErrorDomain" => code == -128,
        _ => false,
    }
}

/// A dictionary of Security attributes.
struct Attributes(Retained<AnyObject>);

impl Attributes {
    fn new() -> Attributes {
        // SAFETY: plain Foundation.
        Attributes(unsafe { msg_send![class!(NSMutableDictionary), new] })
    }

    fn set(&self, key: Ref, value: Ref) {
        // SAFETY: Security's keys are CFStrings, and every value given here is an object.
        unsafe {
            let _: () = msg_send![&*self.0, setObject: value as *const AnyObject, forKey: key as *const AnyObject];
        }
    }

    fn as_ref(&self) -> Ref {
        Retained::as_ptr(&self.0).cast()
    }
}

fn bytes_of(data: &Owned) -> Vec<u8> {
    // SAFETY: a CFData is an NSData.
    let data: &NSData = unsafe { &*(data.0 as *const NSData) };
    data.to_vec()
}

/// Make a key in the Secure Enclave that the enclave uses under `flags`: its handle and its
/// public point. `Err` on a Mac without one.
fn enclave_create(flags: usize) -> Result<NewKey, KeyError> {
    let mut error: Ref = std::ptr::null();
    // SAFETY: the calls follow Security's rules: every Create is released, errors are taken.
    unsafe {
        let access = Owned(SecAccessControlCreateWithFlags(
            std::ptr::null(),
            kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            flags,
            &mut error,
        ));
        if access.0.is_null() {
            return Err(key_error(error, "setting the key's access"));
        }
        let private = Attributes::new();
        let no = NSNumber::new_bool(false);
        private.set(kSecAttrIsPermanent, Retained::as_ptr(&no).cast());
        private.set(kSecAttrAccessControl, access.0);
        let attributes = Attributes::new();
        let bits = NSNumber::new_i32(256);
        attributes.set(kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom);
        attributes.set(kSecAttrKeySizeInBits, Retained::as_ptr(&bits).cast());
        attributes.set(kSecAttrTokenID, kSecAttrTokenIDSecureEnclave);
        attributes.set(kSecPrivateKeyAttrs, private.as_ref());
        let key = Owned(SecKeyCreateRandomKey(attributes.as_ref(), &mut error));
        if key.0.is_null() {
            return Err(key_error(error, "making a Secure Enclave key"));
        }
        let point = public_point(&key)?;
        let described = Owned(SecKeyCopyAttributes(key.0));
        if described.0.is_null() {
            return Err(KeyError::Failed("the new key has no attributes".into()));
        }
        let name = NSString::from_str("toid");
        let blob: Option<Retained<NSData>> =
            msg_send![described.0 as *const AnyObject, objectForKey: &*name];
        let blob = blob.ok_or_else(|| KeyError::Failed("the new key has no blob".into()))?;
        Ok(NewKey {
            handle: format!("se:{}", b64(&blob.to_vec())),
            point,
        })
    }
}

fn public_point(key: &Owned) -> Result<[u8; 65], KeyError> {
    let mut error: Ref = std::ptr::null();
    // SAFETY: as above.
    unsafe {
        let public = Owned(SecKeyCopyPublicKey(key.0));
        if public.0.is_null() {
            return Err(KeyError::Failed("the key has no public half".into()));
        }
        let data = Owned(SecKeyCopyExternalRepresentation(public.0, &mut error));
        if data.0.is_null() {
            return Err(key_error(error, "reading the public key"));
        }
        bytes_of(&data)
            .try_into()
            .map_err(|_| KeyError::Failed("the public key isn't a P-256 point".into()))
    }
}

/// Sign `message` with the enclave key `blob` names. The enclave asks for the user itself when
/// the key's access says so, with `reason` in its prompt.
fn enclave_sign(blob: &str, message: &[u8], reason: &str) -> Result<Vec<u8>, KeyError> {
    let blob =
        unb64(blob).ok_or_else(|| KeyError::Failed("the passkey's key can't be read".into()))?;
    let mut error: Ref = std::ptr::null();
    // SAFETY: as above.
    unsafe {
        let context = LAContext::new();
        context.setLocalizedReason(&NSString::from_str(reason));
        let attributes = Attributes::new();
        let blob = NSData::with_bytes(&blob);
        let name = NSString::from_str("toid");
        attributes.set(kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom);
        attributes.set(kSecAttrKeyClass, kSecAttrKeyClassPrivate);
        attributes.set(kSecAttrTokenID, kSecAttrTokenIDSecureEnclave);
        attributes.set(
            Retained::as_ptr(&name).cast(),
            Retained::as_ptr(&blob).cast(),
        );
        attributes.set(
            kSecUseAuthenticationContext,
            Retained::as_ptr(&context).cast(),
        );
        let empty = NSData::new();
        let key = Owned(SecKeyCreateWithData(
            Retained::as_ptr(&empty).cast(),
            attributes.as_ref(),
            &mut error,
        ));
        if key.0.is_null() {
            // A blob from another Mac, or from before this one was erased.
            return Err(key_error(
                error,
                "finding the passkey's key in the Secure Enclave",
            ));
        }
        let message = NSData::with_bytes(message);
        let signature = Owned(SecKeyCreateSignature(
            key.0,
            kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
            Retained::as_ptr(&message).cast(),
            &mut error,
        ));
        if signature.0.is_null() {
            return Err(key_error(error, "signing"));
        }
        Ok(bytes_of(&signature))
    }
}

/// Ask for Touch ID or the account's password, and wait for the answer.
fn verify_user(reason: &str) -> Result<(), KeyError> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), KeyError>>();
    let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
        let answer = if ok.as_bool() {
            Ok(())
        } else {
            // SAFETY: LocalAuthentication hands an NSError or nil.
            match unsafe { error.as_ref() } {
                Some(e) if !cancelled(&e.domain().to_string(), e.code()) => {
                    Err(KeyError::Failed(format!(
                        "this Mac couldn't verify you: {} ({})",
                        e.localizedDescription(),
                        e.code()
                    )))
                }
                _ => Err(KeyError::Cancelled),
            }
        };
        let _ = tx.send(answer);
    });
    // SAFETY: the context lives until the reply (it's held here while this thread waits).
    let context = unsafe { LAContext::new() };
    unsafe {
        context.evaluatePolicy_localizedReason_reply(
            LAPolicy::DeviceOwnerAuthentication,
            &NSString::from_str(reason),
            &reply,
        );
    }
    // The system holds its own copy of the reply now. Letting go of this one means a reply
    // that is never made closes the channel, and the wait ends, instead of lasting for good.
    drop(reply);
    rx.recv().unwrap_or(Err(KeyError::Cancelled))
}

/// The Mac's authenticator.
pub struct Mac;

impl Mac {
    /// Never from an isolated process: a test must not put a prompt on the user's screen.
    fn real(&self) -> Result<(), KeyError> {
        if trek_core::paths::isolated() {
            Err(KeyError::Failed("an isolated run has no Touch ID".into()))
        } else {
            Ok(())
        }
    }
}

impl Authenticator for Mac {
    fn create(&self, reason: &str) -> Result<NewKey, KeyError> {
        self.real()?;
        // Making a key takes no presence, so it's asked for here: a passkey is the user's to make.
        verify_user(reason)?;
        match enclave_create(PRIVATE_KEY_USAGE | USER_PRESENCE) {
            Ok(key) => Ok(key),
            Err(why) => {
                tracing::info!(
                    "passkeys: no Secure Enclave key ({why:?}); it goes to the Keychain"
                );
                keychain::create()
            }
        }
    }

    fn sign(&self, handle: &str, message: &[u8], reason: &str) -> Result<Vec<u8>, KeyError> {
        self.real()?;
        if let Some(blob) = handle.strip_prefix("se:") {
            // One prompt, the enclave's own.
            enclave_sign(blob, message, reason)
        } else if keychain::is(handle) {
            verify_user(reason)?;
            keychain::sign(handle, message)
        } else {
            Err(KeyError::Failed(
                "this passkey's key isn't this profile's".into(),
            ))
        }
    }

    fn forget(&self, handle: &str) {
        // An enclave key is its blob: once the list drops it, nothing can use it again.
        keychain::forget(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::verifies;

    /// The mechanism, on a Mac with a Secure Enclave: a key made there signs when its blob is
    /// handed back, and what it signs verifies against its public point. The key here needs no
    /// presence, so nothing is asked of anyone; the passkeys' own keys differ only in that flag.
    #[test]
    fn an_enclave_key_signs_from_its_blob_alone() {
        let key = match enclave_create(PRIVATE_KEY_USAGE) {
            Ok(key) => key,
            Err(why) => {
                // A virtual machine (CI) or an Intel Mac without a T2 chip.
                eprintln!("skipped: no Secure Enclave here ({why:?})");
                return;
            }
        };
        let blob = key.handle.strip_prefix("se:").expect("an enclave handle");
        assert!(
            unb64(blob).unwrap().len() > 100,
            "a wrapped key, not a name"
        );
        let der =
            enclave_sign(blob, b"what a relying party checks", "sign a test message").unwrap();
        assert!(verifies(&key.point, b"what a relying party checks", &der));
        assert!(!verifies(&key.point, b"something else", &der));
        // A blob that isn't the enclave's is refused, not signed with something else.
        let refused = enclave_sign(&b64(&[7u8; 300]), b"m", "sign");
        assert!(matches!(refused, Err(KeyError::Failed(_))), "{refused:?}");
    }

    #[test]
    fn only_the_prompts_own_cancels_count_as_the_user_saying_no() {
        assert!(cancelled("com.apple.LocalAuthentication", -2));
        assert!(cancelled("com.apple.LocalAuthentication", -4));
        assert!(cancelled("NSOSStatusErrorDomain", -128));
        // The enclave's own failures share the numbers.
        assert!(!cancelled("CryptoTokenKit", -2));
        assert!(!cancelled("NSOSStatusErrorDomain", -2));
        assert!(!cancelled("com.apple.LocalAuthentication", -1));
    }

    #[test]
    fn a_key_that_needs_presence_is_made_without_asking_anyone() {
        // Made, and its blob read back: no prompt (signing with it would show one, so the test
        // stops here).
        match enclave_create(PRIVATE_KEY_USAGE | USER_PRESENCE) {
            Ok(key) => {
                assert!(key.handle.starts_with("se:"));
                assert_eq!(key.point[0], 4);
            }
            Err(why) => eprintln!("skipped: no Secure Enclave here ({why:?})"),
        }
    }

    #[test]
    fn the_macs_authenticator_refuses_an_isolated_process() {
        trek_core::paths::isolate(std::env::temp_dir().join("trek-passkeys-tests"));
        assert!(matches!(Mac.create("test"), Err(KeyError::Failed(_))));
        assert!(matches!(
            Mac.sign("se:AAAA", b"m", "test"),
            Err(KeyError::Failed(_))
        ));
    }
}
