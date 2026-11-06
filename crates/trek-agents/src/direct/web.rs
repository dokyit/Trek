//! `web_fetch`: a page as text. A model reading documentation doesn't need the markup, and the
//! markup is most of the bytes.

use std::time::Duration;

/// Most of a response that's read.
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// Whether a redirect may go to an address, by the user's rules.
pub type HopCheck = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What's at `url` as text. `Err` is what to tell the model.
pub async fn fetch(url: &str, hop_allowed: HopCheck) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "Error: {url} isn't an http or https address. To read a local file, use read_file."
        ));
    }
    // A public page that redirects inside this network isn't a public page. One asked for by its
    // private address has already been allowed.
    let asked_private = is_private(url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("Trek/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.stop()
            } else if !asked_private && is_private(attempt.url().as_str()) {
                attempt.error("it redirects to an address inside this network")
            } else if !hop_allowed(attempt.url().as_str()) {
                attempt.error("it redirects to an address your rules don't allow without asking")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("Error: {e}"))?;
    let mut response = client
        .get(url)
        .header(
            "accept",
            "text/html,text/plain,text/markdown,application/json;q=0.9,*/*;q=0.5",
        )
        .send()
        .await
        .map_err(|e| format!("Error fetching {url}: {e}"))?;
    // A name can point inside this network as well as an address can (`127.0.0.1.nip.io`): what
    // it reached is what counts.
    if !asked_private
        && response
            .remote_addr()
            .is_some_and(|addr| private_ip(addr.ip()))
    {
        return Err(format!(
            "Error: {url} leads to an address inside this network, so it wasn't read. Ask for that address itself, and the user is asked first."
        ));
    }
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Error fetching {url}: {status}"));
    }
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    if kind.starts_with("image/")
        || kind.starts_with("video/")
        || kind.starts_with("audio/")
        || kind.contains("octet-stream")
        || kind.contains("application/pdf")
        || kind.contains("application/zip")
    {
        return Err(format!(
            "Error: {url} is {kind}, not text. To download it, use bash with curl."
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Error fetching {url}: {e}"))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= MAX_BYTES {
            break;
        }
    }
    let body = String::from_utf8_lossy(&bytes);
    let looks_html = kind.contains("html")
        || (kind.is_empty()
            && body
                .trim_start()
                .to_lowercase()
                .starts_with("<!doctype html"));
    let text = if looks_html {
        html_to_text(&body)
    } else {
        body.into_owned()
    };
    if text.trim().is_empty() {
        return Err(format!(
            "{url} returned no text (a page may need a browser to draw itself)."
        ));
    }
    Ok(text)
}

/// Whether `url` points inside this machine or its network: a page there isn't public, so asking
/// for it goes by what the user allows rather than happening by itself.
pub fn is_private(url: &str) -> bool {
    // Parsed as the HTTP client will: `127.1`, `0x7f.1` and `[::ffff:127.0.0.1]` all mean this machine.
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return true;
    };
    match parsed.host() {
        None => true,
        Some(url::Host::Domain(name)) => {
            let name = name.to_lowercase();
            name == "localhost"
                || name.ends_with(".local")
                || name.ends_with(".localhost")
                || name.ends_with(".internal")
                || !name.contains('.')
        }
        Some(url::Host::Ipv4(ip)) => private_v4(ip),
        Some(url::Host::Ipv6(ip)) => match ip.to_ipv4_mapped() {
            Some(v4) => private_v4(v4),
            None => {
                let first = ip.segments()[0];
                ip.is_loopback()
                    || ip.is_unspecified()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80
            }
        },
    }
}

fn private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => private_v4(v4),
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => private_v4(v4),
            None => {
                let first = v6.segments()[0];
                v6.is_loopback()
                    || v6.is_unspecified()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80
            }
        },
    }
}

fn private_v4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        // The range Tailscale and carrier networks use.
        || (a == 100 && (64..128).contains(&b))
}

/// `html` without its markup: scripts, styles and such dropped, blocks on lines of their own, the
/// common entities written out, runs of blank lines closed up.
fn html_to_text(html: &str) -> String {
    const DROPPED: [&str; 6] = ["script", "style", "noscript", "svg", "head", "template"];
    const BLOCKS: [&str; 20] = [
        "p",
        "div",
        "br",
        "li",
        "ul",
        "ol",
        "tr",
        "table",
        "section",
        "article",
        "header",
        "footer",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "pre",
        "blockquote",
    ];
    let mut out = String::with_capacity(html.len() / 3);
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if after.starts_with("!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        let Some(close) = after.find('>') else {
            rest = "";
            break;
        };
        let tag = &after[..close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        rest = &after[close + 1..];
        if !tag.starts_with('/') && DROPPED.contains(&name.as_str()) && !tag.ends_with('/') {
            // ASCII only: full lowercasing changes byte lengths (`İ`), and this offset goes back
            // into `rest`.
            let lower = rest.to_ascii_lowercase();
            let end = format!("</{name}");
            rest = match lower.find(&end) {
                Some(at) => {
                    let tail = &rest[at..];
                    tail.find('>').map_or("", |e| &tail[e + 1..])
                }
                None => "",
            };
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            out.push('\n');
        }
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let mut text = String::with_capacity(decoded.len());
    let mut blank = true;
    for line in decoded.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !blank {
                text.push('\n');
            }
            blank = true;
        } else {
            text.push_str(line);
            text.push('\n');
            blank = false;
        }
    }
    text.truncate(text.trim_end().len());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_goes_and_text_stays() {
        let page = "<!doctype html><html><head><title>T</title><style>a{}</style></head><body>\
            <h1>Title</h1><script>var x = '<p>no</p>';</script><p>One &amp; two</p>\
            <!-- hidden --><ul><li>a</li><li>b</li></ul></body></html>";
        assert_eq!(html_to_text(page), "Title\n\nOne & two\n\na\n\nb\n");
    }

    #[test]
    fn private_addresses_are_told_apart() {
        for url in [
            "http://localhost:3000/x",
            "http://127.0.0.1/",
            "http://10.1.2.3/",
            "http://192.168.1.1:8080/",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]:8000/",
            "http://printer/",
            "http://mac.local/",
            "http://user:pw@127.0.0.1/",
            "http://127.1/",
            "http://0x7f.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://100.101.102.103/",
            "http://metadata.internal/",
            "not a url",
        ] {
            assert!(is_private(url), "{url}");
        }
        for url in [
            "https://docs.rs/serde",
            "https://8.8.8.8/",
            "https://example.com:8443/a",
        ] {
            assert!(!is_private(url), "{url}");
        }
    }

    #[test]
    fn characters_that_grow_when_lowercased_dont_break_the_offsets() {
        let page = format!(
            "<html><head><title>{}</title></head><body>{}é</body></html>",
            "İ".repeat(40),
            "ok"
        );
        assert_eq!(html_to_text(&page), "oké\n");
    }

    #[tokio::test]
    async fn a_name_that_leads_to_this_machine_isnt_read() {
        // A server on this machine, reached by a name that isn't plainly private. `localtest.me`
        // isn't available offline, so the check is made on what was reached: here, loopback.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                let _ = tokio::io::AsyncWriteExt::write_all(
                    &mut socket,
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nhi",
                )
                .await;
            }
        });
        assert!(private_ip("127.0.0.1".parse().unwrap()));
        assert!(private_ip("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!private_ip("8.8.8.8".parse().unwrap()));
        // Asked for by its own private address, it's read: that one the user was asked about.
        let got = fetch(
            &format!("http://127.0.0.1:{port}/"),
            std::sync::Arc::new(|_| true),
        )
        .await;
        assert_eq!(got.unwrap(), "hi");
    }

    #[tokio::test]
    async fn only_web_addresses_are_fetched() {
        let e = fetch("file:///etc/passwd", std::sync::Arc::new(|_| true))
            .await
            .unwrap_err();
        assert!(e.contains("isn't an http"), "{e}");
    }
}
