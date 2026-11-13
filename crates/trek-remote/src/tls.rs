//! The Mac's TLS identity: a self-signed certificate made once and kept beside the paired
//! devices. Phones don't trust it through any authority; they pin it. The QR code carries the
//! SHA-256 of its DER encoding (`fp`), and the phone accepts a server only if its certificate
//! hashes to that. Typed pairing (no camera) checks the first 16 hex characters instead, which
//! the Mac shows beside the code: 64 bits, which no one makes a certificate to match in the ten
//! minutes a code lasts (8, the first design, could be matched on a GPU in minutes).

use std::io;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

#[derive(Clone)]
pub struct TlsIdentity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
    /// SHA-256 of `cert_der`, 64 lowercase hex characters.
    pub fingerprint: String,
}

impl std::fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// SHA-256 of a DER certificate, as lowercase hex.
pub fn fingerprint(cert_der: &[u8]) -> String {
    Sha256::digest(cert_der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl TlsIdentity {
    /// A new self-signed certificate and key.
    pub fn generate(host_name: &str) -> io::Result<Self> {
        let mut names = vec!["trek.local".to_string(), "localhost".to_string()];
        let clean: String = host_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !clean.is_empty() {
            names.push(format!("{clean}.local"));
        }
        let cert = rcgen::generate_simple_self_signed(names).map_err(io::Error::other)?;
        let cert_der = cert.cert.der().to_vec();
        Ok(Self {
            fingerprint: fingerprint(&cert_der),
            cert_der,
            key_der: cert.signing_key.serialize_der(),
        })
    }

    /// The identity kept in `dir` (`identity.der` and `identity.key`, the key readable by the user
    /// alone), made the first time.
    pub fn load_or_create(dir: &Path, host_name: &str) -> io::Result<Self> {
        let (cert_path, key_path) = (dir.join("identity.der"), dir.join("identity.key"));
        if let (Ok(cert_der), Ok(key_der)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            private_permissions(&key_path)?;
            let identity = Self {
                fingerprint: fingerprint(&cert_der),
                cert_der,
                key_der,
            };
            // A damaged pair is replaced (phones pair again) rather than served.
            if identity.acceptor().is_ok() {
                return Ok(identity);
            }
            tracing::warn!("the phone server's TLS identity doesn't load; making a new one");
        }
        std::fs::create_dir_all(dir)?;
        let identity = Self::generate(host_name)?;
        write_private(&key_path, &identity.key_der)?;
        std::fs::write(&cert_path, &identity.cert_der)?;
        Ok(identity)
    }

    /// The first 16 hex characters in fours, `ABCD-1234-EF56-7890`, for typing or checking by eye.
    pub fn short_fingerprint(&self) -> String {
        let f = self.fingerprint[..16].to_uppercase();
        format!("{}-{}-{}-{}", &f[..4], &f[4..8], &f[8..12], &f[12..])
    }

    pub fn acceptor(&self) -> io::Result<TlsAcceptor> {
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone())),
            )
            .map_err(io::Error::other)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// The key, the user's alone from before its bytes go in: mode 0600 on the Mac, an access list of
/// the user's alone on Windows, where a new file would take its folder's (trek-private).
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    trek_private::write(path, bytes)
}

/// Make a key that's there the user's alone again, if it got looser.
fn private_permissions(path: &Path) -> io::Result<()> {
    trek_private::restrict(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_is_kept_and_its_fingerprint_is_its_certificates() {
        let dir = std::env::temp_dir().join(format!("trek-remote-tls-{}", rand::random::<u64>()));
        let a = TlsIdentity::load_or_create(&dir, "Tobias's MacBook").unwrap();
        assert_eq!(a.fingerprint.len(), 64);
        assert_eq!(a.fingerprint, fingerprint(&a.cert_der));
        assert_eq!(a.short_fingerprint().len(), 19);
        assert!(
            a.fingerprint
                .to_uppercase()
                .starts_with(&a.short_fingerprint().replace('-', ""))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                dir.join("identity.key"),
                std::fs::Permissions::from_mode(0o400),
            )
            .unwrap();
        }
        let b = TlsIdentity::load_or_create(&dir, "Tobias's MacBook").unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "loaded, not made again");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("identity.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the key is the user's alone");
        }
        std::fs::remove_file(dir.join("identity.der")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                dir.join("identity.key"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        let c = TlsIdentity::load_or_create(&dir, "Tobias's MacBook").unwrap();
        assert_ne!(
            b.fingerprint, c.fingerprint,
            "the incomplete identity was replaced"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("identity.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "a replaced key is private too");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `path`'s ACL as Windows writes it down (SDDL), read with calls of its own rather than the
    /// code under test's.
    #[cfg(windows)]
    fn acl_of(path: &Path) -> String {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SDDL_REVISION_1, SE_FILE_OBJECT,
        };
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let (mut dacl, mut descriptor) = (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut text, mut len) = (std::ptr::null_mut(), 0u32);
        // SAFETY: a NUL-ended name and places for what's returned; both returned buffers are
        // freed once.
        unsafe {
            let e = GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(e, 0, "reading the ACL of {}", path.display());
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                &mut len,
            );
            LocalFree(descriptor);
            assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len as usize));
            LocalFree(text.cast());
            sddl.trim_end_matches('\0').to_string()
        }
    }

    /// Two SIDs as SDDL may write them (an alias such as `LA`, or `S-1-…`) are the same SID.
    #[cfg(windows)]
    fn same_sid(a: &str, b: &str) -> bool {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
        use windows_sys::Win32::Security::EqualSid;
        let sid = |text: &str| {
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            let mut sid = std::ptr::null_mut();
            // SAFETY: a NUL-terminated string, and a place for the SID, freed below.
            (unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } != 0).then_some(sid)
        };
        let (Some(x), Some(y)) = (sid(a), sid(b)) else {
            return false;
        };
        // SAFETY: two SIDs Windows made above, each freed once after.
        unsafe {
            let same = EqualSid(x, y) != 0;
            LocalFree(x.cast());
            LocalFree(y.cast());
            same
        }
    }

    /// The user this test runs as, as SDDL names a SID (`S-1-5-21-…`).
    #[cfg(windows)]
    fn user_sid_text() -> String {
        trek_private::user_sid().unwrap()
    }

    /// Let the Everyone group read `path` too, inheriting the folder's entries as well: a key
    /// readable by more than its owner.
    #[cfg(windows)]
    fn open_to_everyone(path: &Path) {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
            SetNamedSecurityInfoW,
        };
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let sddl = wide(&format!("D:(A;;FA;;;{})(A;;FR;;;WD)", user_sid_text()));
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: NUL-ended strings, places for what's returned, and the descriptor freed once.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                ),
                0
            );
            let (mut present, mut dacl, mut defaulted) = (0, std::ptr::null_mut(), 0);
            assert_ne!(
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted),
                0
            );
            let e = SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            );
            LocalFree(descriptor);
            assert_eq!(e, 0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_key_is_the_users_alone_on_windows_too() {
        let dir = std::env::temp_dir().join(format!("trek-remote-tls-{}", rand::random::<u64>()));
        let key = dir.join("identity.key");
        // One entry, the user's, allowing everything (FA) and not inherited (no ID in it), and
        // the ACL protected from inheriting (P; Windows adds AI, which only says how it was made).
        let user = user_sid_text();
        let alone = |path: &Path| {
            let sddl = acl_of(path);
            let (flags, entries) = sddl
                .strip_prefix("D:")
                .and_then(|rest| rest.find('(').map(|at| rest.split_at(at)))
                .unwrap_or_default();
            // The entry's SID as SDDL writes it: an alias for a well-known one (CI's user is the
            // built-in Administrator, `LA`), else S-1-…: compared as SIDs, not as text.
            let sid = entries
                .strip_prefix("(A;;FA;;;")
                .and_then(|rest| rest.strip_suffix(')'))
                .filter(|sid| !sid.contains('('));
            assert!(
                flags.contains('P') && sid.is_some_and(|sid| same_sid(sid, &user)),
                "{} isn't the user's alone: {sddl}",
                path.display()
            );
        };
        let a = TlsIdentity::load_or_create(&dir, "Tobias's PC").unwrap();
        alone(&key);
        // The certificate is public, and keeps its folder's ACL.
        assert!(!acl_of(&dir.join("identity.der")).starts_with("D:P"));

        // Readable by everyone: loading it narrows it again, and keeps the key.
        open_to_everyone(&key);
        let opened = acl_of(&key);
        assert!(
            opened.contains(";;;WD)") && !opened.starts_with("D:P"),
            "{opened}"
        );
        let b = TlsIdentity::load_or_create(&dir, "Tobias's PC").unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "loaded, not made again");
        alone(&key);

        // Readable by everyone and replaced (its certificate is gone): the new key is private too.
        open_to_everyone(&key);
        std::fs::remove_file(dir.join("identity.der")).unwrap();
        let c = TlsIdentity::load_or_create(&dir, "Tobias's PC").unwrap();
        assert_ne!(
            b.fingerprint, c.fingerprint,
            "the incomplete identity was replaced"
        );
        alone(&key);
        let _ = std::fs::remove_dir_all(dir);
    }
}
