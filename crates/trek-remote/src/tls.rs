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

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    // `mode` applies only to a new file, and on Windows a new file takes its folder's ACL. Make it
    // the user's alone before new key bytes touch it, then again after the write as an explicit
    // postcondition.
    #[cfg(unix)]
    private_file_permissions(&file)?;
    #[cfg(windows)]
    owner_only::ensure(path)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    private_permissions(path)
}

fn private_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    private_file_permissions(&std::fs::File::open(path)?)?;
    #[cfg(windows)]
    owner_only::ensure(path)?;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}

#[cfg(unix)]
fn private_file_permissions(file: &std::fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = file.metadata()?.permissions();
    if permissions.mode() & 0o777 != 0o600 {
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

/// The key file's ACL on Windows: the user alone, as 0600 is elsewhere. A new file takes its
/// folder's ACL (in the profile: the user, SYSTEM and the Administrators, and whoever else a data
/// folder elsewhere lets in), so the key's is made one entry, the user's, inheriting nothing.
#[cfg(windows)]
mod owner_only {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION,
        EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorControl, GetTokenInformation,
        InitializeAcl, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// `ACCESS_ALLOWED_ACE_TYPE` (in a windows-sys module of its own).
    const ACCESS_ALLOWED: u8 = 0;

    /// The user this process runs as: its token's `TOKEN_USER`, in a buffer that holds the SID
    /// it points to.
    pub(super) struct User(Vec<u64>);

    impl User {
        pub(super) fn current() -> io::Result<User> {
            let mut token: HANDLE = null_mut();
            // SAFETY: this process's pseudo-handle, and a place for the token's handle.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            // SAFETY: a size query, with no buffer; `len` says what's needed.
            unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len) };
            // In u64s, so the TOKEN_USER at its start (a pointer first) is aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` holds at least `len` bytes.
            let ok = unsafe {
                GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            };
            let failed = io::Error::last_os_error();
            // SAFETY: the handle opened above, closed once.
            unsafe { CloseHandle(token) };
            if ok == 0 {
                return Err(failed);
            }
            Ok(User(buf))
        }

        pub(super) fn sid(&self) -> PSID {
            // SAFETY: the buffer starts with the TOKEN_USER that GetTokenInformation wrote, whose
            // SID is in the buffer too, alive as long as `self`.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    /// Make `path`'s ACL allow `sids` everything, and no one else anything: nothing inherited.
    pub(super) fn set(path: &Path, sids: &[PSID]) -> io::Result<()> {
        // Each entry is an ACCESS_ALLOWED_ACE whose last field is the SID's first four bytes.
        let entry = std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>();
        let size = std::mem::size_of::<ACL>()
            + sids
                .iter()
                // SAFETY: each is a valid SID (the caller's).
                .map(|&sid| entry + unsafe { GetLengthSid(sid) } as usize)
                .sum::<usize>();
        // In u64s: an ACL must be aligned to four bytes at least.
        let mut buf = vec![0u64; size.div_ceil(8)];
        let acl = buf.as_mut_ptr().cast::<ACL>();
        // SAFETY: `acl` is `size` bytes, and each SID is valid.
        unsafe {
            if InitializeAcl(acl, size as u32, ACL_REVISION) == 0 {
                return Err(io::Error::last_os_error());
            }
            for &sid in sids {
                if AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        let name = wide(path);
        // SAFETY: a NUL-ended name, and the ACL built above; owner, group and SACL are left.
        let e = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        Ok(())
    }

    /// Whether `path`'s ACL is what `set(path, &[user])` makes: one entry, allowing `user`
    /// everything, and nothing inherited.
    pub(super) fn is_only(path: &Path, user: PSID) -> io::Result<bool> {
        let name = wide(path);
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: a NUL-ended name, and places for the ACL and the descriptor that holds it.
        let e = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        // SAFETY: `dacl` (null for one that lets everyone in) is inside `descriptor`, which is
        // freed once, after.
        let only = unsafe {
            let (mut control, mut revision) = (0u16, 0u32);
            let mut ace: *mut core::ffi::c_void = null_mut();
            let only = GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0
                && !dacl.is_null()
                && (*dacl).AceCount == 1
                && GetAce(dacl, 0, &mut ace) != 0
                && {
                    let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    ace.Header.AceType == ACCESS_ALLOWED
                        && ace.Mask == FILE_ALL_ACCESS
                        && EqualSid((&raw const ace.SidStart).cast_mut().cast(), user) != 0
                };
            LocalFree(descriptor);
            only
        };
        Ok(only)
    }

    /// Make `path` the user's alone, unless it is already.
    pub(super) fn ensure(path: &Path) -> io::Result<()> {
        let user = User::current()?;
        if !is_only(path, user.sid())? {
            set(path, &[user.sid()])?;
        }
        Ok(())
    }
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

    /// The user this test runs as, as SDDL names a SID (`S-1-5-21-…`).
    #[cfg(windows)]
    fn user_sid_text() -> String {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        let user = owner_only::User::current().unwrap();
        let mut text = std::ptr::null_mut();
        // SAFETY: a valid SID, and a place for the string, freed once.
        unsafe {
            assert_ne!(ConvertSidToStringSidW(user.sid(), &mut text), 0);
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            sid
        }
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
            assert!(
                flags.contains('P') && entries == format!("(A;;FA;;;{user})"),
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
