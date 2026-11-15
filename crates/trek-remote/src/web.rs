//! Phones in a browser: Trek's web remote (`site/remote`, at `https://trekdesktop.com/remote/`), for a
//! phone without the iPhone app, an Android phone first of all.
//!
//! A page can't pin a WebSocket's certificate, and the phone server's is self-signed: what the
//! iPhone app does with the pairing code's `fp`, a browser won't let a page do. WebTransport will:
//! a page may reach a server whose certificate no authority vouches for if it names the
//! certificate's SHA-256 (`serverCertificateHashes`), so long as the certificate is an ECDSA P-256
//! one valid for 14 days at most. So the web remote comes over WebTransport (HTTP/3 over QUIC, on
//! the phone server's port number, over UDP) to `https://<address>:<port>/trek`, and:
//!
//! - **Its certificates come in a series.** Each is valid 13 days and the next starts 12 days after
//!   it, so two overlap for a day, the newer one served from the middle of it (a phone's clock may
//!   be hours off either way). Together they cover the next six months ([`HORIZON_DAYS`]), and the
//!   series grows as days pass. The pairing link carries the first two's hashes, and `paired` and
//!   `welcome` over WebTransport carry every one (`web_certs`): a page that has connected once in
//!   six months knows the certificate it will meet.
//! - **Only Trek's own page gets in.** A session whose `Origin` isn't one of [`WebConfig::origins`]
//!   is refused (403) before anything is read from it, so no other page can drive the server from
//!   the user's network position; another path gets a 404.
//! - **The same protocol, read by the same code, as the iPhone's.** The page opens one bidirectional
//!   stream and speaks the WebSocket protocol on it (RFC 6455 framing as a client, masked, with no
//!   HTTP upgrade first), so the server reads it as it reads the iPhone's: pairing codes, tokens,
//!   the limits before authenticating, pings, the session.

use std::io;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::protocol::WebCertHash;

/// How long each certificate is valid: a browser takes a pinned one of 14 days at most.
pub const CERT_DAYS: i64 = 13;
/// How far apart certificates start: each overlaps the next for a day.
pub const STEP_DAYS: i64 = 12;
/// How far ahead the series reaches.
pub const HORIZON_DAYS: i64 = 180;
/// The path a page asks for.
pub const PATH: &str = "/trek";

const DAY_MS: i64 = 86_400_000;

/// The page origins let in, by default: the web remote as the site serves it.
pub const ORIGINS: &[&str] = &["https://trekdesktop.com"];

/// Serve the web remote beside the phone server.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Where the certificate series is kept (`web-certs.json`, the user's alone); `None` keeps it in
    /// memory (tests).
    pub certs_path: Option<PathBuf>,
    /// The page origins let in (`https://trekdesktop.com`); a session from any other is refused.
    pub origins: Vec<String>,
    /// Where the web remote is served: the pairing link opens it ([`REMOTE_URL`]).
    pub page: String,
    /// This computer's public key for Web Push (`crate::webpush::VapidKey::public_key`): pages
    /// may ask for notifications (`web_push`) when it's set.
    pub push_key: Option<String>,
}

impl WebConfig {
    pub fn new(certs_path: Option<PathBuf>) -> Self {
        Self {
            certs_path,
            origins: ORIGINS.iter().map(|o| o.to_string()).collect(),
            page: REMOTE_URL.to_string(),
            push_key: None,
        }
    }

    /// Pages may ask for notifications, subscribing with `key` (base64url).
    pub fn with_push_key(mut self, key: String) -> Self {
        self.push_key = Some(key);
        self
    }

    /// The web remote served from `page` instead, its origin the one let in: a copy run on this
    /// machine (`http://localhost:8080/remote/`) while it's being worked on or checked.
    pub fn serving_from(mut self, page: &str) -> Self {
        let origin = page
            .split_once("://")
            .map(|(scheme, rest)| {
                let host = rest.split('/').next().unwrap_or(rest);
                format!("{scheme}://{host}")
            })
            .unwrap_or_else(|| page.to_string());
        self.origins = vec![origin];
        self.page = page.to_string();
        self
    }

    /// Whether a session from a page at `origin` is let in. A session that names none isn't: only
    /// a browser comes this way, and every browser names its page.
    pub fn admits_origin(&self, origin: Option<&str>) -> bool {
        origin.is_some_and(|o| self.origins.iter().any(|allowed| allowed == o))
    }
}

/// One certificate of the series, with its key.
#[derive(Clone)]
pub struct WebCert {
    /// When it starts and stops being valid (unix ms).
    pub from: i64,
    pub to: i64,
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl std::fmt::Debug for WebCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebCert")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("hash", &self.hash())
            .finish_non_exhaustive()
    }
}

impl WebCert {
    /// A new ECDSA P-256 certificate, valid from `from` to `to` (unix ms, whole seconds kept).
    pub fn generate(from: i64, to: i64) -> io::Result<Self> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(io::Error::other)?;
        let mut params =
            rcgen::CertificateParams::new(vec!["trek.local".to_string(), "localhost".to_string()])
                .map_err(io::Error::other)?;
        let at = |ms: i64| {
            time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).map_err(io::Error::other)
        };
        params.not_before = at(from)?;
        params.not_after = at(to)?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Trek");
        let cert = params.self_signed(&key).map_err(io::Error::other)?;
        Ok(Self {
            from: from.div_euclid(1000) * 1000,
            to: to.div_euclid(1000) * 1000,
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        })
    }

    /// SHA-256 of its DER encoding, base64: what a page names in `serverCertificateHashes`.
    pub fn hash(&self) -> String {
        B64.encode(Sha256::digest(&self.cert_der))
    }

    /// As `serverCertificateHashes` needs it, with when it's valid.
    pub fn web_hash(&self) -> WebCertHash {
        WebCertHash {
            hash: self.hash(),
            from: self.from,
            to: self.to,
        }
    }

    /// The certificate and key as WebTransport serves them.
    pub fn identity(&self) -> io::Result<wtransport::Identity> {
        let cert = wtransport::tls::Certificate::from_der(self.cert_der.clone())
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(wtransport::Identity::new(
            wtransport::tls::CertificateChain::single(cert),
            wtransport::tls::PrivateKey::from_der_pkcs8(self.key_der.clone()),
        ))
    }
}

/// The series of certificates the web remote is served with.
#[derive(Debug)]
pub struct WebCerts {
    certs: Vec<WebCert>,
    path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    certs: Vec<SavedCert>,
}

#[derive(Serialize, Deserialize)]
struct SavedCert {
    from: i64,
    to: i64,
    cert: String,
    key: String,
}

impl WebCerts {
    /// A series kept in memory, reaching [`HORIZON_DAYS`] from `now`.
    pub fn in_memory(now: i64) -> io::Result<Self> {
        let mut certs = Self {
            certs: Vec::new(),
            path: None,
        };
        certs.extend(now)?;
        Ok(certs)
    }

    /// The series kept at `path` (made the first time, the user's alone), with the ended ones gone
    /// and new ones made to reach [`HORIZON_DAYS`] from `now`. A file that doesn't read is
    /// replaced: pages paired with it pair again.
    pub fn load_or_create(path: PathBuf, now: i64) -> io::Result<Self> {
        let certs = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
            .map(|saved| {
                saved
                    .certs
                    .into_iter()
                    .filter_map(|c| {
                        Some(WebCert {
                            from: c.from,
                            to: c.to,
                            cert_der: B64.decode(c.cert).ok()?,
                            key_der: B64.decode(c.key).ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if path.exists() {
            trek_private::restrict(&path)?;
        }
        let mut series = Self {
            certs,
            path: Some(path),
        };
        // One that doesn't load as an identity (a damaged file) starts the series again.
        if series.certs.iter().any(|c| c.identity().is_err()) {
            tracing::warn!("the web remote's certificates don't load; making new ones");
            series.certs.clear();
        }
        series.extend(now)?;
        Ok(series)
    }

    /// Drop the certificates that have ended and make new ones until the series reaches
    /// [`HORIZON_DAYS`] past `now`, saving it when it changed. Whether it did.
    pub fn extend(&mut self, now: i64) -> io::Result<bool> {
        let before = self.certs.len();
        self.certs.retain(|c| c.to > now);
        let mut changed = self.certs.len() != before;
        // The first starts half a day ago: a phone whose clock is behind takes it too.
        let mut start = match self.certs.last() {
            Some(last) => last.from + STEP_DAYS * DAY_MS,
            None => now - DAY_MS / 2,
        };
        while start < now + HORIZON_DAYS * DAY_MS {
            self.certs
                .push(WebCert::generate(start, start + CERT_DAYS * DAY_MS)?);
            start += STEP_DAYS * DAY_MS;
            changed = true;
        }
        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    /// The certificate to serve at `now`: the newest that has been valid for half a day and stays
    /// valid for another half, so a phone's clock may be hours off either way; else any valid one.
    pub fn current(&self, now: i64) -> Option<&WebCert> {
        self.certs
            .iter()
            .rev()
            .find(|c| c.from <= now - DAY_MS / 2 && c.to >= now + DAY_MS / 2)
            .or_else(|| self.certs.iter().find(|c| c.from <= now && now < c.to))
    }

    /// Every certificate's hash and when it's valid, oldest first: what `paired` and `welcome`
    /// over WebTransport carry.
    pub fn hashes(&self) -> Vec<WebCertHash> {
        self.certs.iter().map(WebCert::web_hash).collect()
    }

    /// The hashes for the pairing link: the one served at `now` and the one after it, enough for
    /// the page to pair and connect; it learns the rest as it does.
    pub fn link_hashes(&self, now: i64) -> Vec<WebCertHash> {
        let Some(current) = self.current(now) else {
            return Vec::new();
        };
        self.certs
            .iter()
            .filter(|c| c.from >= current.from)
            .take(2)
            .map(WebCert::web_hash)
            .collect()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let saved = Saved {
            certs: self
                .certs
                .iter()
                .map(|c| SavedCert {
                    from: c.from,
                    to: c.to,
                    cert: B64.encode(&c.cert_der),
                    key: B64.encode(&c.key_der),
                })
                .collect(),
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(&saved).map_err(io::Error::other)?;
        trek_private::write(path, &bytes)
    }
}

/// `server` configured to serve `cert` at `addr`: an IPv6 address takes IPv4 too, as the phone
/// server's TCP listener does.
pub fn server_config(
    addr: std::net::SocketAddr,
    cert: &WebCert,
) -> io::Result<wtransport::ServerConfig> {
    let builder = wtransport::ServerConfig::builder();
    let builder = match addr {
        std::net::SocketAddr::V6(v6) => {
            builder.with_bind_address_v6(v6, wtransport::config::Ipv6DualStackConfig::Allow)
        }
        std::net::SocketAddr::V4(_) => builder.with_bind_address(addr),
    };
    Ok(builder
        .with_identity(cert.identity()?)
        .keep_alive_interval(Some(std::time::Duration::from_secs(10)))
        .max_idle_timeout(Some(std::time::Duration::from_secs(60)))
        .map_err(io::Error::other)?
        .build())
}

/// The pairing link a page opens (`https://trekdesktop.com/remote/#p=…`): everything after `#`,
/// which a browser never sends to the site, is a base64url JSON of the code, the machine, every
/// `host:port` it answers on, and the first certificates' hashes alone (when each is valid, the
/// page is told once it pairs): the shorter the link, the plainer its QR code.
pub fn pairing_link(
    base: &str,
    code: &str,
    host_name: &str,
    host_id: &str,
    routes: &[String],
    hashes: &[WebCertHash],
) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = serde_json::json!({
        "v": 1,
        "code": code,
        "name": host_name,
        "hid": host_id,
        "routes": routes,
        "certs": hashes.iter().map(|h| h.hash.as_str()).collect::<Vec<_>>(),
    });
    format!(
        "{base}#p={}",
        URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes())
    )
}

/// Where the web remote is served: the pairing link opens it.
pub const REMOTE_URL: &str = "https://trekdesktop.com/remote/";

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_600_000_000;

    #[test]
    fn a_series_covers_six_months_in_overlapping_thirteen_day_certificates() {
        let series = WebCerts::in_memory(NOW).unwrap();
        let hashes = series.hashes();
        // 180 days from half a day back, a certificate every 12 days.
        assert_eq!(hashes.len(), 16);
        for w in hashes.windows(2) {
            assert_eq!(
                w[1].from - w[0].from,
                STEP_DAYS * DAY_MS,
                "starts 12 days apart"
            );
            assert!(w[0].to > w[1].from, "each overlaps the next");
        }
        for h in &hashes {
            assert_eq!(
                h.to - h.from,
                CERT_DAYS * DAY_MS,
                "13 days: under a browser's 14"
            );
        }
        assert!(hashes.last().unwrap().to >= NOW + HORIZON_DAYS * DAY_MS);
        let distinct: std::collections::HashSet<_> = hashes.iter().map(|h| &h.hash).collect();
        assert_eq!(distinct.len(), hashes.len(), "each its own");
    }

    #[test]
    fn the_served_certificate_changes_in_the_middle_of_an_overlap() {
        let series = WebCerts::in_memory(NOW).unwrap();
        let [first, second, ..] = &series.certs[..] else {
            panic!("a series");
        };
        assert_eq!(series.current(NOW).unwrap().from, first.from);
        // The second starts a day before the first ends; it's served from half a day into that.
        let mid = second.from + DAY_MS / 2;
        assert_eq!(series.current(mid - 1).unwrap().from, first.from);
        assert_eq!(series.current(mid).unwrap().from, second.from);
        // Whichever is served, a phone half a day off either way finds it valid.
        for now in [NOW, mid - 1, mid, second.from + 5 * DAY_MS] {
            let c = series.current(now).unwrap();
            assert!(
                c.from <= now - DAY_MS / 2 && c.to >= now + DAY_MS / 2,
                "{now}"
            );
        }
        assert_eq!(series.link_hashes(NOW).len(), 2);
        assert_eq!(series.link_hashes(NOW)[0].hash, first.hash());
    }

    #[test]
    fn the_series_is_kept_and_grows_as_days_pass() {
        let dir = std::env::temp_dir().join(format!("trek-web-certs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("web-certs.json");
        let made = WebCerts::load_or_create(path.clone(), NOW).unwrap();
        let again = WebCerts::load_or_create(path.clone(), NOW).unwrap();
        assert_eq!(made.hashes(), again.hashes(), "the same series, read back");
        assert!(again.current(NOW).unwrap().identity().is_ok());
        // Forty days on: the ended ones are gone, the series reaches six months again, and what
        // was there is kept.
        let later = NOW + 40 * DAY_MS;
        let grown = WebCerts::load_or_create(path.clone(), later).unwrap();
        let hashes = grown.hashes();
        assert!(hashes.iter().all(|h| h.to > later));
        assert!(hashes.last().unwrap().to >= later + HORIZON_DAYS * DAY_MS);
        assert!(hashes.iter().any(|h| made.hashes().contains(h)), "kept");
        // A damaged file starts again.
        std::fs::write(&path, b"{not json").unwrap();
        let fresh = WebCerts::load_or_create(path, NOW).unwrap();
        assert_eq!(fresh.hashes().len(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_remotes_page_gets_in() {
        let config = WebConfig::new(None);
        assert!(config.admits_origin(Some("https://trekdesktop.com")));
        assert!(!config.admits_origin(Some("https://evil.example")));
        assert!(!config.admits_origin(Some("http://trekdesktop.com")));
        assert!(!config.admits_origin(Some("https://trekdesktop.com.evil.example")));
        assert!(
            !config.admits_origin(None),
            "no origin: not a browser's page"
        );
    }

    #[test]
    fn the_pairing_link_carries_everything_after_the_hash() {
        let series = WebCerts::in_memory(NOW).unwrap();
        let link = pairing_link(
            REMOTE_URL,
            "K7Q2-9XMV",
            "Toby's PC",
            "7f3c",
            &["192.168.1.20:7420".into(), "100.101.1.2:7420".into()],
            &series.link_hashes(NOW),
        );
        let (base, fragment) = link.split_once('#').unwrap();
        assert_eq!(base, REMOTE_URL);
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let json: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(fragment.strip_prefix("p=").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(json["code"], "K7Q2-9XMV");
        assert_eq!(json["routes"][1], "100.101.1.2:7420");
        assert_eq!(json["certs"].as_array().unwrap().len(), 2);
        assert_eq!(json["certs"][0], series.current(NOW).unwrap().hash());
        assert!(
            link.len() < 420,
            "a plain QR code: {} characters",
            link.len()
        );
    }
}
