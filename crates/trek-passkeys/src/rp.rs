//! Which site a passkey belongs to, and which pages may ask for it. This is the rule the whole
//! feature rests on: a passkey made for `example.com` signs only for pages of `example.com`, so a
//! page elsewhere can never be handed a signature it could replay there.
//!
//! WebAuthn (Level 3, 5.1.3 step 8 and 5.1.4.1 step 7): the page's origin must be one that can use
//! credentials at all (https, or http on localhost), its host a domain (never an IP address), and
//! the relying party id it names must be that host or a registrable suffix of it (HTML's "is a
//! registrable domain suffix of or is equal to"): `login.example.com` may name `example.com`, and
//! may not name `com`, `co.uk`, `github.io` or `ample.com`.
//!
//! The origin itself always comes from the web view (WebKit's security origin of the frame that
//! asked), never from the page's script.

use std::net::Ipv4Addr;

/// A page's origin, as the web view reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// Lower case, without the colon.
    pub scheme: String,
    /// Lower case; an IPv6 address without its brackets.
    pub host: String,
    /// `None` for the scheme's own port.
    pub port: Option<u16>,
}

impl Origin {
    /// From the parts a web view gives (WebKit's `WKSecurityOrigin`: a port of 0 is "the
    /// scheme's own").
    pub fn new(scheme: &str, host: &str, port: u16) -> Origin {
        let scheme = scheme.trim_end_matches(':').to_ascii_lowercase();
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        let own = match scheme.as_str() {
            "https" | "wss" => 443,
            "http" | "ws" => 80,
            _ => 0,
        };
        Origin {
            port: (port != 0 && port != own).then_some(port),
            scheme,
            host,
        }
    }

    /// From an address (`https://example.com:8443/path`). `None` for one without a scheme and
    /// host, or with a port that isn't one.
    pub fn parse(url: &str) -> Option<Origin> {
        let (scheme, rest) = url.split_once("://")?;
        let scheme_byte = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.');
        if scheme.is_empty() || !scheme.bytes().all(scheme_byte) {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next()?;
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (host, after) = v6.split_once(']')?;
            (host, after.strip_prefix(':'))
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        if host.is_empty() {
            return None;
        }
        let port = match port {
            None | Some("") => 0,
            Some(p) => p.parse::<u16>().ok().filter(|p| *p != 0)?,
        };
        Some(Origin::new(scheme, host, port))
    }

    /// The origin as `clientDataJSON` carries it (HTML's serialization): scheme, host, and the
    /// port when it isn't the scheme's own.
    pub fn serialize(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{}://{host}:{port}", self.scheme),
            None => format!("{}://{host}", self.scheme),
        }
    }

    /// The origin's host when pages there may use passkeys at all: https, or http on localhost
    /// (what browsers treat as secure for this), and a host that is a domain. An IP address is
    /// not one (WebAuthn: "effective domain is not a valid domain").
    pub fn effective_domain(&self) -> Result<&str, &'static str> {
        let local = self.host == "localhost";
        match self.scheme.as_str() {
            "https" => {}
            "http" if local => {}
            "http" => return Err("Passkeys need a secure page (https)."),
            _ => return Err("Passkeys work on web pages only (https)."),
        }
        if !is_domain(&self.host) {
            return Err("Passkeys need a site with a name, not an IP address.");
        }
        Ok(&self.host)
    }
}

/// Whether `host` is a domain name rather than an IP address or nothing at all.
fn is_domain(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.contains(':') {
        return false;
    }
    if host.parse::<Ipv4Addr>().is_ok() {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    let mut labels = host.split('.');
    let well_formed = labels.clone().all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    });
    // A host whose last label is a number is an IPv4 address to a URL parser (`1.2.3.4`,
    // `0x7f.1`, `127.1`), whatever form it's written in.
    let last = labels.next_back().unwrap_or_default();
    let numeric = last.bytes().all(|b| b.is_ascii_digit())
        || (last.starts_with("0x") && last[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    well_formed && !numeric
}

/// The public suffix `host` ends in (`com`, `co.uk`, `github.io`; a name's own last label when
/// the list doesn't know it, as the list's default rule says).
fn public_suffix(host: &str) -> &str {
    psl::suffix(host.as_bytes())
        .and_then(|s| std::str::from_utf8(s.as_bytes()).ok())
        .unwrap_or(host)
}

/// The relying party id a page at `origin` may use: the one it named (`claimed`), or its own
/// host when it named none. `Err`: the page may not use it, and why (a `SecurityError`).
pub fn relying_party(origin: &Origin, claimed: Option<&str>) -> Result<String, &'static str> {
    let host = origin.effective_domain()?;
    let Some(claimed) = claimed else {
        return Ok(host.to_string());
    };
    if registrable_suffix_or_equal(claimed, host) {
        Ok(claimed.to_string())
    } else {
        Err("The relying party id is not this page's site or a registrable part of it.")
    }
}

/// HTML's "is a registrable domain suffix of or is equal to": whether a page on `host` may act
/// for `suffix`. `host` is a domain already (`effective_domain`).
fn registrable_suffix_or_equal(suffix: &str, host: &str) -> bool {
    if suffix.is_empty() {
        return false;
    }
    if suffix == host {
        return true;
    }
    // The id is compared as written: one in capitals, with a trailing dot, or that isn't a
    // domain at all names no site a host could end in.
    if !is_domain(suffix) || suffix.ends_with('.') {
        return false;
    }
    // On a label boundary: `ample.com` is no suffix of `example.com`.
    if !host
        .strip_suffix(suffix)
        .is_some_and(|rest| rest.ends_with('.'))
    {
        return false;
    }
    // Never a public suffix: every site under `co.uk` would share its passkeys.
    if suffix == public_suffix(suffix) {
        return false;
    }
    // Nor the host's own public suffix or anything it ends in: under an exception rule
    // (`!city.kobe.jp` beside `*.kobe.jp`) a host's suffix is one the list doesn't call public
    // when asked about it alone.
    let host_suffix = public_suffix(host);
    suffix != host_suffix
        && !host_suffix
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(url: &str) -> Origin {
        Origin::parse(url).unwrap_or_else(|| panic!("{url} is an origin"))
    }

    fn allowed(url: &str, claimed: Option<&str>) -> Result<String, &'static str> {
        relying_party(&origin(url), claimed)
    }

    #[test]
    fn an_origin_serializes_as_client_data_wants_it() {
        for (url, want) in [
            ("https://example.com/", "https://example.com"),
            ("https://example.com:443/login", "https://example.com"),
            (
                "https://example.com:8443/login?x#y",
                "https://example.com:8443",
            ),
            ("HTTPS://Example.COM/", "https://example.com"),
            ("http://localhost:3000/", "http://localhost:3000"),
            ("http://localhost:80/", "http://localhost"),
            ("https://user:pw@example.com/", "https://example.com"),
            ("https://[::1]:8080/", "https://[::1]:8080"),
        ] {
            assert_eq!(origin(url).serialize(), want, "{url}");
        }
        // WebKit's own parts: port 0 is the scheme's own.
        assert_eq!(
            Origin::new("https", "webauthn.io", 0).serialize(),
            "https://webauthn.io"
        );
        assert_eq!(
            Origin::new("https:", "Example.com", 444).serialize(),
            "https://example.com:444"
        );
        for bad in [
            "example.com",
            "https://",
            "https://example.com:x/",
            "://a",
            "https://a:0/",
        ] {
            assert_eq!(Origin::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_page_may_name_its_own_host_or_a_registrable_part_of_it() {
        assert_eq!(allowed("https://example.com", None).unwrap(), "example.com");
        assert_eq!(
            allowed("https://login.example.com", None).unwrap(),
            "login.example.com"
        );
        assert_eq!(
            allowed("https://login.example.com", Some("example.com")).unwrap(),
            "example.com"
        );
        assert_eq!(
            allowed("https://a.b.c.example.com", Some("c.example.com")).unwrap(),
            "c.example.com"
        );
        assert_eq!(
            allowed("https://www.example.co.uk", Some("example.co.uk")).unwrap(),
            "example.co.uk"
        );
        // The port is the origin's business, not the id's.
        assert_eq!(
            allowed("https://example.com:8443", Some("example.com")).unwrap(),
            "example.com"
        );
    }

    #[test]
    fn suffix_tricks_name_no_site() {
        for (url, claimed) in [
            // Not on a label boundary.
            ("https://example.com", "ample.com"),
            ("https://notexample.com", "example.com"),
            ("https://example.com.evil.net", "example.com"),
            // Another site, a longer name, a sibling.
            ("https://evil.net", "example.com"),
            ("https://example.com", "login.example.com"),
            ("https://a.example.com", "b.example.com"),
            // The id is taken as written.
            ("https://example.com", "EXAMPLE.com"),
            ("https://login.example.com", "Example.com"),
            ("https://login.example.com", "example.com."),
            ("https://login.example.com", ".example.com"),
            ("https://login.example.com", "example.com:443"),
            ("https://login.example.com", "https://example.com"),
            ("https://login.example.com", "example.com/"),
            ("https://login.example.com", " example.com"),
            ("https://login.example.com", ""),
            ("https://login.example.com", "."),
        ] {
            assert!(allowed(url, Some(claimed)).is_err(), "{url} as {claimed:?}");
        }
    }

    #[test]
    fn a_public_suffix_is_nobodys_to_name() {
        for (url, claimed) in [
            ("https://example.com", "com"),
            ("https://www.example.co.uk", "co.uk"),
            ("https://www.example.co.uk", "uk"),
            // Private suffixes: each name under them is someone else's site.
            ("https://alice.github.io", "github.io"),
            ("https://app.herokuapp.com", "herokuapp.com"),
            ("https://bucket.s3.amazonaws.com", "s3.amazonaws.com"),
            // A wildcard rule (`*.ck`): `anything.ck` is a public suffix.
            ("https://www.shop.foo.ck", "foo.ck"),
            // A name the list doesn't know: its last label is the suffix.
            ("https://printer.corp.internal", "internal"),
            ("https://app.localhost", "localhost"),
        ] {
            assert!(allowed(url, Some(claimed)).is_err(), "{url} as {claimed:?}");
        }
        // Their own names are theirs, and the list's exceptions hold (`!www.ck`).
        assert!(allowed("https://alice.github.io", Some("alice.github.io")).is_ok());
        assert!(allowed("https://x.alice.github.io", Some("alice.github.io")).is_ok());
        assert!(allowed("https://a.www.ck", Some("www.ck")).is_ok());
        assert!(allowed("https://www.shop.foo.ck", Some("shop.foo.ck")).is_ok());
        assert!(allowed("https://printer.corp.internal", Some("corp.internal")).is_ok());
        // An exception's own suffix is still a public one to the hosts under it: `city.kobe.jp`
        // is a site (`!city.kobe.jp`), and `kobe.jp` above it is everyone's.
        assert!(allowed("https://x.city.kobe.jp", Some("city.kobe.jp")).is_ok());
        assert!(allowed("https://x.city.kobe.jp", Some("kobe.jp")).is_err());
        assert!(allowed("https://x.city.kobe.jp", Some("jp")).is_err());
        // A site on a public suffix itself may be its own id, as its host.
        assert!(allowed("https://github.io", Some("github.io")).is_ok());
    }

    #[test]
    fn ip_addresses_have_no_passkeys() {
        for url in [
            "https://127.0.0.1",
            "https://127.0.0.1:8443",
            "https://192.168.1.10",
            "https://[::1]",
            "https://[2001:db8::1]:8443",
            "http://127.0.0.1:3000",
            // What a URL parser reads as IPv4.
            "https://127.1",
            "https://0x7f.0.0.1",
            "https://2130706433",
        ] {
            let o = origin(url);
            assert!(relying_party(&o, None).is_err(), "{url}");
            assert!(relying_party(&o, Some(&o.host)).is_err(), "{url} as itself");
        }
        // Nor may a name's page claim an address, or the other way round.
        assert!(allowed("https://example.com", Some("127.0.0.1")).is_err());
        assert!(allowed("https://1.0.0.127.example.com", Some("0.0.127.example.com")).is_ok());
    }

    #[test]
    fn only_https_and_http_on_localhost() {
        assert_eq!(allowed("http://localhost:3000", None).unwrap(), "localhost");
        assert_eq!(
            allowed("http://localhost", Some("localhost")).unwrap(),
            "localhost"
        );
        assert_eq!(
            allowed("https://localhost:8443", None).unwrap(),
            "localhost"
        );
        for url in [
            "http://example.com",
            "http://login.example.com:8080",
            // A name that only looks local.
            "http://localhost.example.com",
            "http://app.localhost",
            "http://notlocalhost",
            "file://localhost/etc/passwd",
            "trek://start",
            "about://blank",
            "webkit-extension://abcdef",
            "ws://localhost",
            "ftp://example.com",
        ] {
            assert!(allowed(url, None).is_err(), "{url}");
        }
        assert!(allowed("http://localhost:3000", Some("example.com")).is_err());
    }

    #[test]
    fn hosts_that_are_no_domain_are_refused() {
        for host in [
            "",
            "exa mple.com",
            "example..com",
            "ex\u{e4}mple.com",
            "a/b",
            "-",
        ] {
            let o = Origin::new("https", host, 0);
            // "-" is a well-formed label; the rest aren't names at all.
            assert_eq!(relying_party(&o, None).is_ok(), host == "-", "{host:?}");
        }
        let long = format!("{}.com", "a".repeat(64));
        assert!(relying_party(&Origin::new("https", &long, 0), None).is_err());
    }
}
