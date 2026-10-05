//! Pairing codes, device tokens and the registry of paired devices.
//!
//! - A pairing code is 8 Crockford base32 characters (40 bits) shown as `XXXX-XXXX`: one use,
//!   short-lived, burned after [`MAX_PAIRING_ATTEMPTS`] wrong attempts from anyone.
//! - A device token is 32 random bytes, base64url without padding. Only its SHA-256 is stored, and
//!   tokens are compared in constant time.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Crockford base32 (no I, L, O, U).
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in a pairing code (without the dash).
pub const CODE_LEN: usize = 8;
/// Wrong attempts (from anyone) before a pairing code is burned.
pub const MAX_PAIRING_ATTEMPTS: u32 = 5;

/// Longest device id / device name accepted from a phone.
pub(crate) const MAX_DEVICE_FIELD: usize = 128;

/// Unix time in milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Codes
// ---------------------------------------------------------------------------------------------

/// A fresh random pairing code, formatted `XXXX-XXXX`.
pub fn generate_code() -> String {
    let mut rng = rand::rng();
    let raw: String = (0..CODE_LEN).map(|_| CODE_ALPHABET[rng.random_range(0..32)] as char).collect();
    format_code(&raw)
}

fn format_code(raw: &str) -> String {
    format!("{}-{}", &raw[..4], &raw[4..])
}

/// The canonical form of a typed code: uppercase, without dashes and spaces, with the letters
/// people confuse mapped (O → 0, I/L → 1). `None` unless it's 8 characters of the alphabet.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !CODE_ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// Why a pairing code was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("No pairing code is active on this Mac")]
    NoOffer,
    #[error("The pairing code has expired")]
    Expired,
    #[error("Wrong pairing code")]
    Wrong,
    #[error("Too many wrong attempts; show a new pairing code")]
    Burned,
}

/// A pairing code being shown on the Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingOffer {
    /// `XXXX-XXXX`.
    pub code: String,
    /// When it stops working (unix ms).
    pub expires_at: i64,
    /// `trek://pair?host=…&code=…&name=…&hid=…&fp=…`, for the QR code.
    pub url: String,
    /// The certificate's short fingerprint (`ABCD-1234-EF56-7890`), to type with the code when
    /// there's no camera; `None` without TLS.
    pub fingerprint: Option<String>,
}

#[derive(Debug)]
struct ActiveCode {
    normalized: String,
    expires: Instant,
    wrong: u32,
}

/// The one active pairing code (single use, expiring, attempt-limited).
#[derive(Debug, Default)]
pub struct Pairing {
    active: Option<ActiveCode>,
}

impl Pairing {
    /// Activate `code` (replacing any active one) until `ttl` from now.
    pub fn activate(&mut self, code: &str, ttl: Duration) -> Result<(), PairingError> {
        let normalized = normalize_code(code).ok_or(PairingError::Wrong)?;
        self.active = Some(ActiveCode { normalized, expires: Instant::now() + ttl, wrong: 0 });
        Ok(())
    }

    /// Stop accepting the active code.
    pub fn cancel(&mut self) {
        self.active = None;
    }

    pub fn is_active(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.expires > Instant::now())
    }

    /// Redeem a typed code. Success consumes the code; a wrong code counts against the attempt
    /// budget and the code is burned when it runs out.
    pub fn redeem(&mut self, input: &str) -> Result<(), PairingError> {
        self.check_at(input, Instant::now())?;
        self.cancel();
        Ok(())
    }

    pub(crate) fn check(&mut self, input: &str) -> Result<(), PairingError> {
        self.check_at(input, Instant::now())
    }

    pub(crate) fn check_at(&mut self, input: &str, now: Instant) -> Result<(), PairingError> {
        let active = self.active.as_mut().ok_or(PairingError::NoOffer)?;
        if now >= active.expires {
            self.active = None;
            return Err(PairingError::Expired);
        }
        let matches = normalize_code(input)
            .is_some_and(|typed| bool::from(typed.as_bytes().ct_eq(active.normalized.as_bytes())));
        if matches {
            return Ok(());
        }
        active.wrong += 1;
        if active.wrong >= MAX_PAIRING_ATTEMPTS {
            self.active = None;
            return Err(PairingError::Burned);
        }
        Err(PairingError::Wrong)
    }
}

/// The QR code's URL: `trek://pair?host=<host:port>&code=<code>&name=<host name>&hid=<host id>`,
/// and `&fp=<SHA-256 of the server's certificate>` when it speaks TLS (the phone pins it).
pub fn pairing_url(advertise: &str, code: &str, host_name: &str, host_id: &str, fingerprint: Option<&str>) -> String {
    let mut url = format!(
        "trek://pair?host={}&code={}&name={}&hid={}",
        percent_encode(advertise, b":[]"),
        percent_encode(code, b""),
        percent_encode(host_name, b""),
        percent_encode(host_id, b""),
    );
    if let Some(fp) = fingerprint {
        url.push_str(&format!("&fp={}", percent_encode(fp, b"")));
    }
    url
}

/// Percent-encode everything but RFC 3986 unreserved characters and `keep`.
pub fn percent_encode(s: &str, keep: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------------

/// A fresh device token: 32 random bytes, base64url without padding.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Lowercase hex SHA-256 of a token.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Whether `token` hashes to `stored_hash`, compared in constant time.
pub fn verify_token(token: &str, stored_hash: &str) -> bool {
    let presented = hash_token(token);
    presented.len() == stored_hash.len() && bool::from(presented.as_bytes().ct_eq(stored_hash.as_bytes()))
}

// ---------------------------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------------------------

/// A paired device as stored (`devices.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    /// Lowercase hex SHA-256 of the device token.
    pub token_sha256: String,
    /// Unix ms.
    pub paired_at: i64,
    /// Unix ms.
    #[serde(default)]
    pub last_seen_at: Option<i64>,
}

/// A paired device, as the app lists it (no token hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
}

impl From<&DeviceRecord> for DeviceInfo {
    fn from(r: &DeviceRecord) -> Self {
        Self { device_id: r.device_id.clone(), name: r.name.clone(), paired_at: r.paired_at, last_seen_at: r.last_seen_at }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<DeviceRecord>,
}

/// Paired devices, optionally persisted as JSON (saved atomically on every change).
#[derive(Debug, Default)]
pub struct DeviceRegistry {
    path: Option<PathBuf>,
    devices: Vec<DeviceRecord>,
}

impl DeviceRegistry {
    /// A registry that isn't saved anywhere.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Load from `path` (a missing file is an empty registry). A file that doesn't parse is moved
    /// aside to `<path>.corrupt` and the registry starts empty: those devices pair again.
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let devices = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<DevicesFile>(&bytes) {
                Ok(file) => file.devices,
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "devices file is corrupt; starting empty");
                    let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                    Vec::new()
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err),
        };
        Ok(Self { path: Some(path), devices })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Pair a device: store a fresh token's hash (replacing any earlier token of the same device)
    /// and return the token.
    pub fn register(&mut self, device_id: &str, name: &str, now: i64) -> io::Result<String> {
        let token = generate_token();
        let record = DeviceRecord {
            device_id: device_id.to_string(),
            name: name.to_string(),
            token_sha256: hash_token(&token),
            paired_at: now,
            last_seen_at: Some(now),
        };
        let before = self.devices.clone();
        match self.devices.iter_mut().find(|d| d.device_id == device_id) {
            Some(existing) => *existing = record,
            None => self.devices.push(record),
        }
        if let Err(err) = self.save() {
            self.devices = before;
            return Err(err);
        }
        Ok(token)
    }

    /// Whether `token` is the current token of `device_id`.
    pub fn verify(&self, device_id: &str, token: &str) -> bool {
        self.devices.iter().find(|d| d.device_id == device_id).is_some_and(|d| verify_token(token, &d.token_sha256))
    }

    /// Record that the device was seen at `now`.
    pub fn touch(&mut self, device_id: &str, now: i64) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.device_id == device_id) {
            d.last_seen_at = Some(now);
            self.save_logged();
        }
    }

    /// Forget a device; returns whether it was paired.
    pub fn revoke(&mut self, device_id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.device_id != device_id);
        let removed = self.devices.len() != before;
        if removed {
            self.save_logged();
        }
        removed
    }

    pub fn get(&self, device_id: &str) -> Option<DeviceInfo> {
        self.devices.iter().find(|d| d.device_id == device_id).map(DeviceInfo::from)
    }

    pub fn list(&self) -> Vec<DeviceInfo> {
        self.devices.iter().map(DeviceInfo::from).collect()
    }

    /// Write to the file (if any): a temp file next to it, then a rename.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&DevicesFile { devices: self.devices.clone() }).map_err(io::Error::other)?;
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        {
            use std::io::Write as _;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    fn save_logged(&self) {
        if let Err(err) = self.save() {
            tracing::warn!(%err, "couldn't save paired devices");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_have_the_right_shape() {
        for _ in 0..200 {
            let code = generate_code();
            assert_eq!(code.len(), 9);
            assert_eq!(&code[4..5], "-");
            assert!(code.bytes().filter(|&b| b != b'-').all(|b| CODE_ALPHABET.contains(&b)));
            assert_eq!(normalize_code(&code).unwrap(), code.replace('-', ""));
        }
    }

    #[test]
    fn normalization_is_forgiving() {
        assert_eq!(normalize_code("K7Q2-9XMV").as_deref(), Some("K7Q29XMV"));
        assert_eq!(normalize_code("k7q2 9xmv").as_deref(), Some("K7Q29XMV"));
        assert_eq!(normalize_code(" k7q29xmv ").as_deref(), Some("K7Q29XMV"));
        assert_eq!(normalize_code("OIL0-1234").as_deref(), Some("01101234"));
        assert_eq!(normalize_code("oil0-1234").as_deref(), Some("01101234"));
        assert_eq!(normalize_code("K7Q2-9XM"), None);
        assert_eq!(normalize_code("K7Q2-9XMVV"), None);
        assert_eq!(normalize_code("K7Q2-9XMU"), None);
        assert_eq!(normalize_code("TREK-DEMO").as_deref(), Some("TREKDEM0"));
        assert_eq!(normalize_code("K7Q2-9XM€"), None);
    }

    #[test]
    fn a_code_works_once() {
        let mut p = Pairing::default();
        p.activate("K7Q2-9XMV", Duration::from_secs(60)).unwrap();
        assert!(p.is_active());
        assert_eq!(p.redeem("k7q2 9xmv"), Ok(()));
        assert_eq!(p.redeem("K7Q2-9XMV"), Err(PairingError::NoOffer));
        assert!(!p.is_active());
    }

    #[test]
    fn a_code_expires() {
        let mut p = Pairing::default();
        p.activate("K7Q2-9XMV", Duration::from_secs(60)).unwrap();
        assert_eq!(p.check_at("K7Q2-9XMV", Instant::now() + Duration::from_secs(61)), Err(PairingError::Expired));
        assert_eq!(p.redeem("K7Q2-9XMV"), Err(PairingError::NoOffer));
    }

    #[test]
    fn wrong_attempts_burn_the_code() {
        let mut p = Pairing::default();
        p.activate("K7Q2-9XMV", Duration::from_secs(60)).unwrap();
        for _ in 0..MAX_PAIRING_ATTEMPTS - 1 {
            assert_eq!(p.redeem("AAAA-AAAA"), Err(PairingError::Wrong));
        }
        // Garbage counts too.
        assert_eq!(p.redeem("nope"), Err(PairingError::Burned));
        assert_eq!(p.redeem("K7Q2-9XMV"), Err(PairingError::NoOffer));
    }

    #[test]
    fn a_new_offer_resets_attempts() {
        let mut p = Pairing::default();
        p.activate("K7Q2-9XMV", Duration::from_secs(60)).unwrap();
        for _ in 0..MAX_PAIRING_ATTEMPTS - 1 {
            let _ = p.redeem("AAAA-AAAA");
        }
        p.activate("K7Q2-9XMV", Duration::from_secs(60)).unwrap();
        assert_eq!(p.redeem("AAAA-AAAA"), Err(PairingError::Wrong));
        assert_eq!(p.redeem("K7Q2-9XMV"), Ok(()));
    }

    #[test]
    fn invalid_codes_cannot_be_activated() {
        assert_eq!(Pairing::default().activate("TREK-DEMU", Duration::from_secs(1)), Err(PairingError::Wrong));
    }

    #[test]
    fn tokens_verify_in_constant_time() {
        let token = generate_token();
        assert_eq!(token.len(), 43);
        assert!(!token.contains(['+', '/', '=']));
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert!(verify_token(&token, &hash));
        assert!(!verify_token("nope", &hash));
        assert!(!verify_token(&token, "short"));
        assert_ne!(generate_token(), token);
        assert_eq!(hash_token("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn registry_pairs_replaces_and_revokes() {
        let mut r = DeviceRegistry::in_memory();
        let t1 = r.register("dev-1", "Tobias's iPhone", 1).unwrap();
        assert!(r.verify("dev-1", &t1));
        assert!(!r.verify("dev-2", &t1));
        let t2 = r.register("dev-1", "Tobias's iPhone", 2).unwrap();
        assert!(!r.verify("dev-1", &t1), "re-pairing replaces the old token");
        assert!(r.verify("dev-1", &t2));
        assert_eq!(r.list().len(), 1);
        r.touch("dev-1", 5);
        assert_eq!(r.get("dev-1").unwrap().last_seen_at, Some(5));
        assert!(r.revoke("dev-1"));
        assert!(!r.revoke("dev-1"));
        assert!(!r.verify("dev-1", &t2));
    }

    #[test]
    fn registry_persists() {
        let dir = std::env::temp_dir().join(format!("trek-remote-test-{}-{}", std::process::id(), generate_token()));
        let path = dir.join("devices.json");
        let token = {
            let mut r = DeviceRegistry::load(&path).unwrap();
            assert!(r.list().is_empty());
            r.register("dev-1", "Phone", 10).unwrap()
        };
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&token), "the token itself is never stored");
        assert!(raw.contains(&hash_token(&token)));
        let r = DeviceRegistry::load(&path).unwrap();
        assert!(r.verify("dev-1", &token));
        assert_eq!(r.list()[0].name, "Phone");

        std::fs::write(&path, b"{not json").unwrap();
        let r = DeviceRegistry::load(&path).unwrap();
        assert!(r.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn urls_are_encoded() {
        let url = pairing_url("192.168.1.20:7420", "K7Q2-9XMV", "Tobias\u{2019}s MacBook Pro", "7f3c", None);
        assert_eq!(url, "trek://pair?host=192.168.1.20:7420&code=K7Q2-9XMV&name=Tobias%E2%80%99s%20MacBook%20Pro&hid=7f3c");
        let pinned = pairing_url("192.168.1.20:7420", "K7Q2-9XMV", "Mac", "7f3c", Some("ab12"));
        assert!(pinned.ends_with("&hid=7f3c&fp=ab12"), "{pinned}");
        assert_eq!(percent_encode("a&b=c/d?", b""), "a%26b%3Dc%2Fd%3F");
    }
}
