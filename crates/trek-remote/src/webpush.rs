//! Notifications on a phone's browser, sent by this computer itself: standard Web Push.
//!
//! The web remote (`site/remote`), installed on a phone's home screen, asks its browser for a push
//! subscription with this computer's public key (VAPID, RFC 8292), and hands it over on its own
//! connection (`web_push`). When a thread needs the user or finishes, the computer encrypts a short
//! note to the phone's key (RFC 8291, `aes128gcm`) and POSTs it to the subscription's endpoint:
//! Apple's, Google's or Mozilla's push service, which wakes the phone. No account, no key from
//! anyone, no server of Trek's in between: each install makes its own key pair, and the push
//! service sees only ciphertext, its size and when it was sent.
//!
//! What's here is the part with no network in it: the subscription, the key pair and the signed
//! `Authorization` header, the encryption, and what a push service's answer means. The app sends.

use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use ring::{aead, agreement, hkdf};
use serde::{Deserialize, Serialize};

/// base64url without padding, as browsers write a subscription's keys; padding is taken on the way
/// in, as some write it.
const B64: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Who sends, as the push services ask (`sub`): a URL or `mailto:`; Apple refuses anything else.
pub const SUBJECT: &str = "https://trekdesktop.com";
/// The record size written in the header: one record holds any note Trek sends.
const RECORD_SIZE: u32 = 4096;
/// The most a note may say before encryption: push services take 4096 bytes of body, and the
/// header (86), tag (16) and padding delimiter (1) come out of that.
pub const MAX_PLAINTEXT: usize = 4096 - 86 - 16 - 1;
/// How long a push service keeps a note for a phone that's off: a day, as news older than that is
/// in the Inbox anyway.
pub const TTL_SECS: u32 = 24 * 60 * 60;
/// How long a signed header is good for (the services take at most a day).
const JWT_SECS: u64 = 12 * 60 * 60;
/// The longest endpoint taken (they run to a few hundred characters).
const MAX_ENDPOINT: usize = 2048;

/// A phone browser's push subscription, as `PushSubscription.toJSON()` gives it, flattened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSubscription {
    /// Where notes go: an `https://` address of the browser's push service.
    pub endpoint: String,
    /// The browser's P-256 public key (65 bytes, uncompressed), base64url.
    pub p256dh: String,
    /// The browser's 16-byte authentication secret, base64url.
    pub auth: String,
}

impl PushSubscription {
    /// Whether it's one the computer can send to: an `https://` endpoint of sensible length and
    /// keys of the right shape.
    pub fn check(&self) -> Result<(), &'static str> {
        let rest = self
            .endpoint
            .strip_prefix("https://")
            .ok_or("A push endpoint must be an https:// address")?;
        let host = rest.split('/').next().unwrap_or_default();
        if host.is_empty()
            || self.endpoint.len() > MAX_ENDPOINT
            || self.endpoint.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("That isn't a push endpoint");
        }
        self.keys().map(|_| ())
    }

    /// The decoded keys: the browser's public key and its authentication secret.
    fn keys(&self) -> Result<([u8; 65], [u8; 16]), &'static str> {
        let p256dh = B64
            .decode(self.p256dh.trim())
            .ok()
            .and_then(|k| <[u8; 65]>::try_from(k).ok())
            .filter(|k| k[0] == 0x04)
            .ok_or("The subscription's key isn't a P-256 public key")?;
        let auth = B64
            .decode(self.auth.trim())
            .ok()
            .and_then(|a| <[u8; 16]>::try_from(a).ok())
            .ok_or("The subscription's secret isn't 16 bytes")?;
        Ok((p256dh, auth))
    }

    /// The push service's origin (`https://web.push.apple.com`): the signed header's audience.
    pub fn audience(&self) -> String {
        let rest = self.endpoint.trim_start_matches("https://");
        format!("https://{}", rest.split('/').next().unwrap_or_default())
    }
}

/// The note a phone shows (the web remote's service worker reads it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushNote {
    pub title: String,
    pub body: String,
    /// The thread it's about, to open when tapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The computer it came from (its host id).
    pub host: String,
    /// A newer note with the same tag replaces the older one on the phone.
    pub tag: String,
}

impl PushNote {
    /// The note as JSON, cut to fit what a push service takes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut note = self.clone();
        loop {
            let bytes = serde_json::to_vec(&note).expect("a note always serializes");
            if bytes.len() <= MAX_PLAINTEXT || note.body.is_empty() {
                return bytes;
            }
            let keep = note.body.chars().count().saturating_sub(64);
            note.body = note.body.chars().take(keep).collect::<String>();
            if !note.body.is_empty() {
                note.body.push('…');
            }
        }
    }
}

/// This computer's key pair for signing what it sends (VAPID): its own, made once and kept in the
/// data folder, readable by the user alone.
pub struct VapidKey {
    pair: EcdsaKeyPair,
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    /// PKCS#8, base64url.
    pkcs8: String,
}

impl VapidKey {
    /// A new key pair, and its PKCS#8 bytes to keep.
    pub fn generate() -> io::Result<(Self, Vec<u8>)> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| io::Error::other("couldn't make a push key"))?;
        let bytes = pkcs8.as_ref().to_vec();
        Ok((Self::from_pkcs8(&bytes)?, bytes))
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> io::Result<Self> {
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &SystemRandom::new())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Self { pair })
    }

    /// The key kept at `path`, or a new one written there. A file that doesn't read as a key is
    /// replaced: the phones that subscribed with the old one subscribe again.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(file) = serde_json::from_slice::<KeyFile>(&bytes)
            && let Ok(pkcs8) = B64.decode(file.pkcs8.trim())
            && let Ok(key) = Self::from_pkcs8(&pkcs8)
        {
            return Ok(key);
        }
        let (key, pkcs8) = Self::generate()?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&KeyFile {
            pkcs8: B64.encode(pkcs8),
        })
        .map_err(io::Error::other)?;
        trek_private::write(path, &json)?;
        Ok(key)
    }

    /// The public key, base64url (65 bytes uncompressed): the page subscribes with it
    /// (`applicationServerKey`).
    pub fn public_key(&self) -> String {
        B64.encode(self.pair.public_key().as_ref())
    }

    /// The `Authorization` header for a note to `sub`, signed now (`now` in Unix seconds):
    /// `vapid t=<JWT>, k=<public key>`.
    pub fn authorization(&self, sub: &PushSubscription, now: u64) -> io::Result<String> {
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64.encode(
            serde_json::json!({
                "aud": sub.audience(),
                "exp": now + JWT_SECS,
                "sub": SUBJECT,
            })
            .to_string(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .map_err(|_| io::Error::other("couldn't sign a push"))?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            B64.encode(signature.as_ref()),
            self.public_key()
        ))
    }
}

/// `plaintext` encrypted to the subscription's key, as the body of a push (RFC 8291 `aes128gcm`):
/// a fresh key pair and salt for each note.
pub fn encrypt(sub: &PushSubscription, plaintext: &[u8]) -> io::Result<Vec<u8>> {
    let bad = |why: &'static str| io::Error::new(io::ErrorKind::InvalidInput, why);
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(bad("The note is too long for a push"));
    }
    let (ua_public, auth) = sub.keys().map_err(bad)?;
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt)
        .map_err(|_| io::Error::other("no randomness"))?;
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| io::Error::other("couldn't make a key"))?;
    let as_public: [u8; 65] = private
        .compute_public_key()
        .map_err(|_| io::Error::other("couldn't make a key"))?
        .as_ref()
        .try_into()
        .map_err(|_| io::Error::other("unexpected key size"))?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let secret = agreement::agree_ephemeral(private, &peer, |s| s.to_vec())
        .map_err(|_| bad("The subscription's key isn't a point on P-256"))?;
    seal(&secret, &auth, &ua_public, &as_public, &salt, plaintext)
}

/// An HKDF output length.
struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &hkdf::Prk, info: &[&[u8]], out: &mut [u8]) -> io::Result<()> {
    prk.expand(info, Len(out.len()))
        .and_then(|okm| okm.fill(out))
        .map_err(|_| io::Error::other("key derivation failed"))
}

/// The content key and nonce for a message, from the ECDH secret (RFC 8291 section 3.4 and
/// RFC 8188 section 2.2).
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8; 16],
    ua_public: &[u8; 65],
    as_public: &[u8; 65],
    salt: &[u8; 16],
) -> io::Result<([u8; 16], [u8; 12])> {
    let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(ecdh_secret);
    let mut ikm = [0u8; 32];
    expand(
        &prk_key,
        &[b"WebPush: info\0", ua_public, as_public],
        &mut ikm,
    )?;
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    expand(&prk, &[b"Content-Encoding: aes128gcm\0"], &mut cek)?;
    expand(&prk, &[b"Content-Encoding: nonce\0"], &mut nonce)?;
    Ok((cek, nonce))
}

/// The body for `plaintext`: header (salt, record size, the sender's public key), then the one
/// record, its padding delimiter `0x02` and the tag.
fn seal(
    ecdh_secret: &[u8],
    auth: &[u8; 16],
    ua_public: &[u8; 65],
    as_public: &[u8; 65],
    salt: &[u8; 16],
    plaintext: &[u8],
) -> io::Result<Vec<u8>> {
    let (cek, nonce) = derive(ecdh_secret, auth, ua_public, as_public, salt)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek)
            .map_err(|_| io::Error::other("bad content key"))?,
    );
    let mut record = Vec::with_capacity(plaintext.len() + 17);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| io::Error::other("encryption failed"))?;
    let mut body = Vec::with_capacity(86 + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(65);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// What a push service's answer means for the subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Taken (201, or any 2xx).
    Sent,
    /// The subscription is gone (404, 410): the phone unsubscribed, or its browser dropped it.
    /// Forget it.
    Gone,
    /// Busy or down (429, 5xx): worth another try in a moment.
    Retry,
    /// Refused (400, 403, 413…): trying again won't help, but the subscription may still be good.
    Refused,
}

impl Outcome {
    pub fn of(status: u16) -> Self {
        match status {
            200..=299 => Self::Sent,
            404 | 410 => Self::Gone,
            429 | 500..=599 => Self::Retry,
            _ => Self::Refused,
        }
    }
}

/// The request headers for a note: its encoding, how long the service keeps it, how urgent it is
/// (`high` wakes a phone in low-power mode; for what needs the user), and the signed header.
pub fn headers(authorization: String, urgent: bool) -> Vec<(&'static str, String)> {
    vec![
        ("Content-Encoding", "aes128gcm".into()),
        ("Content-Type", "application/octet-stream".into()),
        ("TTL", TTL_SECS.to_string()),
        ("Urgency", if urgent { "high" } else { "normal" }.into()),
        ("Authorization", authorization),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    fn b64(s: &str) -> Vec<u8> {
        B64.decode(s.replace(char::is_whitespace, "")).unwrap()
    }

    /// RFC 8291 section 5 and appendix A: the same inputs give the same message, byte for byte.
    #[test]
    fn encryption_matches_the_rfc_example() {
        let ecdh = b64("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let auth: [u8; 16] = b64("BTBZMqHH6r4Tts7J_aSIgg").try_into().unwrap();
        let ua: [u8; 65] = b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        )
        .try_into()
        .unwrap();
        let sender: [u8; 65] = b64(
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
        )
        .try_into()
        .unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let (cek, nonce) = derive(&ecdh, &auth, &ua, &sender, &salt).unwrap();
        assert_eq!(B64.encode(cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(B64.encode(nonce), "4h_95klXJ5E_qnoN");
        let body = seal(
            &ecdh,
            &auth,
            &ua,
            &sender,
            &salt,
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        assert_eq!(
            B64.encode(&body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
        // The header, the 41 bytes of text, the delimiter and the tag.
        assert_eq!(body.len(), 86 + 41 + 1 + 16);
    }

    /// What `encrypt` sends, a phone opens: the browser's side, with a key pair made for the test.
    #[test]
    fn a_note_round_trips_to_the_phone() {
        let rng = SystemRandom::new();
        let phone = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let phone_public: [u8; 65] = phone
            .compute_public_key()
            .unwrap()
            .as_ref()
            .try_into()
            .unwrap();
        let auth = [7u8; 16];
        let sub = PushSubscription {
            endpoint: "https://web.push.apple.com/QGuQyavXutnMH".into(),
            p256dh: B64.encode(phone_public),
            auth: B64.encode(auth),
        };
        sub.check().unwrap();
        let note = PushNote {
            title: "Needs you · api".into(),
            body: "Fix the login bug".into(),
            thread: Some("t1".into()),
            host: "m1".into(),
            tag: "m1:t1".into(),
        };
        let body = encrypt(&sub, &note.to_bytes()).unwrap();

        // The phone reads the header, agrees on the secret with the sender's key and opens it.
        let salt: [u8; 16] = body[..16].try_into().unwrap();
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
        assert_eq!(body[20], 65);
        let sender: [u8; 65] = body[21..86].try_into().unwrap();
        let secret = agreement::agree_ephemeral(
            phone,
            &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender),
            |s| s.to_vec(),
        )
        .unwrap();
        let (cek, nonce) = derive(&secret, &auth, &phone_public, &sender, &salt).unwrap();
        let key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = body[86..].to_vec();
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut record,
            )
            .unwrap();
        assert_eq!(opened.last(), Some(&0x02), "the last record's delimiter");
        let read: PushNote = serde_json::from_slice(&opened[..opened.len() - 1]).unwrap();
        assert_eq!(read, note);
        // Each note has its own salt and key: two of the same say nothing about each other.
        assert_ne!(encrypt(&sub, b"same").unwrap(), encrypt(&sub, b"same").unwrap());
    }

    #[test]
    fn the_signed_header_names_the_push_service_and_verifies() {
        let (key, pkcs8) = VapidKey::generate().unwrap();
        let again = VapidKey::from_pkcs8(&pkcs8).unwrap();
        assert_eq!(key.public_key(), again.public_key(), "kept, it's the same key");
        assert_eq!(b64(&key.public_key()).len(), 65);
        let sub = PushSubscription {
            endpoint: "https://fcm.googleapis.com/fcm/send/abc:def".into(),
            p256dh: String::new(),
            auth: String::new(),
        };
        let header = key.authorization(&sub, 1_700_000_000).unwrap();
        let (jwt, k) = header
            .strip_prefix("vapid t=")
            .unwrap()
            .split_once(", k=")
            .unwrap();
        assert_eq!(k, key.public_key());
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], SUBJECT);
        assert_eq!(claims["exp"], 1_700_000_000u64 + JWT_SECS);
        let signed = format!("{}.{}", parts[0], parts[1]);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, b64(k))
            .verify(signed.as_bytes(), &b64(parts[2]))
            .expect("signed with the key it names");
    }

    #[test]
    fn the_key_is_kept_and_a_bad_file_replaced() {
        let dir = std::env::temp_dir().join(format!("trek-vapid-{}", std::process::id()));
        let path = dir.join("push-key.json");
        let _ = std::fs::remove_dir_all(&dir);
        let first = VapidKey::load_or_create(&path).unwrap().public_key();
        assert_eq!(VapidKey::load_or_create(&path).unwrap().public_key(), first);
        std::fs::write(&path, b"not a key").unwrap();
        assert_ne!(VapidKey::load_or_create(&path).unwrap().public_key(), first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_https_endpoints_with_real_keys_are_taken() {
        let good = PushSubscription {
            endpoint: "https://updates.push.services.mozilla.com/wpush/v2/gAAA".into(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".into(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg==".into(),
        };
        assert_eq!(good.check(), Ok(()), "padding or not");
        let with = |f: fn(&mut PushSubscription)| {
            let mut s = good.clone();
            f(&mut s);
            s.check()
        };
        assert!(with(|s| s.endpoint = "http://push.example/x".into()).is_err());
        assert!(with(|s| s.endpoint = "https:///x".into()).is_err());
        assert!(with(|s| s.endpoint = "https://a.example/x y".into()).is_err());
        assert!(with(|s| s.endpoint = format!("https://a.example/{}", "x".repeat(3000))).is_err());
        assert!(with(|s| s.p256dh = "BTBZMqHH6r4Tts7J_aSIgg".into()).is_err());
        assert!(with(|s| s.auth = "AAAA".into()).is_err());
        assert_eq!(good.audience(), "https://updates.push.services.mozilla.com");
    }

    #[test]
    fn answers_say_what_to_do_with_the_subscription() {
        assert_eq!(Outcome::of(201), Outcome::Sent);
        assert_eq!(Outcome::of(404), Outcome::Gone);
        assert_eq!(Outcome::of(410), Outcome::Gone);
        assert_eq!(Outcome::of(429), Outcome::Retry);
        assert_eq!(Outcome::of(503), Outcome::Retry);
        assert_eq!(Outcome::of(403), Outcome::Refused);
        assert_eq!(Outcome::of(413), Outcome::Refused);
    }

    #[test]
    fn a_long_note_is_cut_to_fit() {
        let note = PushNote {
            title: "Finished".into(),
            body: "é".repeat(5000),
            thread: None,
            host: "m1".into(),
            tag: "m1".into(),
        };
        let bytes = note.to_bytes();
        assert!(bytes.len() <= MAX_PLAINTEXT);
        let read: PushNote = serde_json::from_slice(&bytes).unwrap();
        assert!(read.body.ends_with('…'));
    }
}
