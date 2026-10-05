//! The Mac's TLS identity: a self-signed certificate made once and kept beside the paired
//! devices. Phones don't trust it through any authority; they pin it. The QR code carries the
//! SHA-256 of its DER encoding (`fp`), and the phone accepts a server only if its certificate
//! hashes to that. Typed pairing (no camera) checks the first 8 hex characters instead, which the
//! Mac shows beside the code.

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
        f.debug_struct("TlsIdentity").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

/// SHA-256 of a DER certificate, as lowercase hex.
pub fn fingerprint(cert_der: &[u8]) -> String {
    Sha256::digest(cert_der).iter().map(|b| format!("{b:02x}")).collect()
}

impl TlsIdentity {
    /// A new self-signed certificate and key.
    pub fn generate(host_name: &str) -> io::Result<Self> {
        let mut names = vec!["trek.local".to_string(), "localhost".to_string()];
        let clean: String = host_name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if !clean.is_empty() {
            names.push(format!("{clean}.local"));
        }
        let cert = rcgen::generate_simple_self_signed(names).map_err(io::Error::other)?;
        let cert_der = cert.cert.der().to_vec();
        Ok(Self { fingerprint: fingerprint(&cert_der), cert_der, key_der: cert.signing_key.serialize_der() })
    }

    /// The identity kept in `dir` (`identity.der` and `identity.key`, the key readable by the user
    /// alone), made the first time.
    pub fn load_or_create(dir: &Path, host_name: &str) -> io::Result<Self> {
        let (cert_path, key_path) = (dir.join("identity.der"), dir.join("identity.key"));
        if let (Ok(cert_der), Ok(key_der)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            let identity = Self { fingerprint: fingerprint(&cert_der), cert_der, key_der };
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

    /// The first 8 hex characters, grouped `ABCD-1234`, for checking by eye or typing.
    pub fn short_fingerprint(&self) -> String {
        let f = self.fingerprint[..8].to_uppercase();
        format!("{}-{}", &f[..4], &f[4..])
    }

    pub fn acceptor(&self) -> io::Result<TlsAcceptor> {
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(self.cert_der.clone())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone())))
            .map_err(io::Error::other)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
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
        assert_eq!(a.short_fingerprint().len(), 9);
        let b = TlsIdentity::load_or_create(&dir, "Tobias's MacBook").unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "loaded, not made again");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("identity.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the key is the user's alone");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
