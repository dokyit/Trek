//! The web remote over WebTransport (`trek_remote::web`), on loopback: a real server with a small
//! host, and a WebTransport client doing what the page does. It pins the certificate by the hash in
//! the pairing link, names the page's origin, opens one stream and speaks the WebSocket protocol on
//! it as a client.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use trek_remote::web::WebConfig;
use trek_remote::*;
use wtransport::endpoint::ConnectOptions;
use wtransport::tls::Sha256Digest;

/// The page's origin in these tests: a copy of the web remote served on this machine.
const PAGE: &str = "http://localhost:8080/remote/";
const ORIGIN: &str = "http://localhost:8080";

struct Host;

impl RemoteHost for Host {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        Ok(Snapshot {
            threads: vec![ThreadSummary {
                id: "t1".into(),
                title: "Fix the flaky auth test".into(),
                updated_at: 1,
                ..Default::default()
            }],
            ..Default::default()
        })
    }
    async fn transcript(&self, _: &str) -> HostResult<Transcript> {
        Ok(Transcript::default())
    }
    async fn send(&self, _: SendRequest) -> HostResult<Option<Open>> {
        Ok(None)
    }
    async fn new_thread(&self, _: NewThreadRequest) -> HostResult<String> {
        Ok("t2".into())
    }
    async fn answer(&self, _: AnswerRequest) -> HostResult<()> {
        Ok(())
    }
    async fn interrupt(&self, _: &str) -> HostResult<()> {
        Ok(())
    }
}

fn host_info() -> HostInfo {
    HostInfo {
        id: "host-1".into(),
        name: "Toby's PC".into(),
        version: "0.4.53".into(),
    }
}

async fn start() -> RemoteHandle {
    let mut config = ServerConfig::new(host_info());
    config.bind = "127.0.0.1:0".parse().unwrap();
    config.web = Some(WebConfig::new(None).serving_from(PAGE));
    RemoteServer::start(config, Arc::new(Host)).await.unwrap()
}

/// What the pairing link carries after `#p=`.
fn link_payload(url: &str) -> Value {
    let (base, fragment) = url.split_once('#').unwrap();
    assert_eq!(base, PAGE);
    let encoded = fragment.strip_prefix("p=").unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap()
}

fn digest(hash: &str) -> Sha256Digest {
    Sha256Digest::new(B64.decode(hash).unwrap().try_into().unwrap())
}

type Page = WebSocketStream<tokio::io::Join<wtransport::RecvStream, wtransport::SendStream>>;

/// A session as the page opens it: `hashes` pinned, `origin` named (or none), at `path`.
async fn open(
    handle: &RemoteHandle,
    hashes: &[String],
    origin: Option<&str>,
    path: &str,
) -> Result<(wtransport::Connection, Page), String> {
    let port = handle.web_listening()[0].port();
    let config = wtransport::ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes(hashes.iter().map(|h| digest(h)))
        .build();
    let endpoint = wtransport::Endpoint::client(config).unwrap();
    let mut options = ConnectOptions::builder(format!("https://127.0.0.1:{port}{path}"));
    if let Some(origin) = origin {
        options = options.add_header("origin", origin);
    }
    let connection = endpoint
        .connect(options.build())
        .await
        .map_err(|e| e.to_string())?;
    let (send, recv) = connection
        .open_bi()
        .await
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let ws =
        WebSocketStream::from_raw_socket(tokio::io::join(recv, send), Role::Client, None).await;
    Ok((connection, ws))
}

async fn say(ws: &mut Page, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn hear(ws: &mut Page) -> Value {
    let deadline = std::time::Duration::from_secs(5);
    loop {
        let frame = tokio::time::timeout(deadline, ws.next())
            .await
            .expect("an answer in time")
            .expect("open")
            .expect("a frame");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

#[tokio::test]
async fn a_page_pairs_and_says_hello_over_webtransport_with_the_certificate_pinned() {
    let handle = start().await;
    assert!(!handle.web_listening().is_empty(), "the web remote listens");
    let offer = handle.pairing_offer_with_code("K7Q2-9XMV");
    let link = link_payload(offer.web_url.as_deref().expect("a web link"));
    assert_eq!(link["code"], "K7Q2-9XMV");
    assert_eq!(link["name"], "Toby's PC");
    let pinned: Vec<String> = link["certs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(pinned.len(), 2, "the one served and the next");

    // Pairing, with only the link's hashes pinned.
    let (_conn, mut page) = open(&handle, &pinned, Some(ORIGIN), "/trek").await.unwrap();
    say(
        &mut page,
        json!({"type":"pair","id":"1","protocol":1,"code":"K7Q2-9XMV","device_id":"web-1","device_name":"Pixel 9","app_version":"web","platform":"web"}),
    )
    .await;
    let paired = hear(&mut page).await;
    assert_eq!(paired["type"], "paired", "{paired}");
    assert_eq!(paired["re"], "1");
    let token = paired["token"].as_str().unwrap().to_string();
    let certs = paired["web_certs"].as_array().expect("every certificate");
    assert_eq!(certs.len(), handle.web_certs().len());
    assert!(certs.len() >= 15, "six months of them: {}", certs.len());
    assert_eq!(certs[0]["hash"], pinned[0].as_str());
    assert_eq!(hear(&mut page).await["type"], "snapshot");
    let device = handle
        .devices()
        .into_iter()
        .find(|d| d.device_id == "web-1")
        .expect("paired");
    assert_eq!(device.name, "Pixel 9");

    // Saying hello again, as the page does each time it opens, with every hash it was given.
    let all: Vec<String> = certs
        .iter()
        .map(|c| c["hash"].as_str().unwrap().to_string())
        .collect();
    let (_conn, mut page) = open(&handle, &all, Some(ORIGIN), "/trek").await.unwrap();
    say(
        &mut page,
        json!({"type":"hello","id":"2","protocol":1,"device_id":"web-1","token":token,"platform":"web"}),
    )
    .await;
    let welcome = hear(&mut page).await;
    assert_eq!(welcome["type"], "welcome", "{welcome}");
    assert_eq!(welcome["web_certs"].as_array().unwrap().len(), certs.len());
    let snapshot = hear(&mut page).await;
    assert_eq!(snapshot["threads"][0]["title"], "Fix the flaky auth test");

    // A wrong token is refused, as over the WebSocket.
    let (_conn, mut page) = open(&handle, &all, Some(ORIGIN), "/trek").await.unwrap();
    say(
        &mut page,
        json!({"type":"hello","id":"3","protocol":1,"device_id":"web-1","token":"nope"}),
    )
    .await;
    let refused = hear(&mut page).await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["code"], "unauthorized");
    handle.shutdown();
}

#[tokio::test]
async fn only_the_remotes_page_gets_in() {
    let handle = start().await;
    let hashes: Vec<String> = handle.web_certs().into_iter().map(|c| c.hash).collect();
    for origin in [
        Some("https://evil.example"),
        Some("http://localhost:9999"),
        None,
    ] {
        let refused = open(&handle, &hashes, origin, "/trek").await;
        assert!(refused.is_err(), "{origin:?} let in");
    }
    assert!(
        open(&handle, &hashes, Some(ORIGIN), "/elsewhere")
            .await
            .is_err(),
        "another path"
    );
    assert!(open(&handle, &hashes, Some(ORIGIN), "/trek").await.is_ok());
    handle.shutdown();
}

#[tokio::test]
async fn a_certificate_not_in_the_link_isnt_trusted() {
    let handle = start().await;
    // A hash of some other certificate: the client refuses the server's.
    let other = B64.encode([7u8; 32]);
    assert!(
        open(&handle, &[other], Some(ORIGIN), "/trek")
            .await
            .is_err()
    );
    handle.shutdown();
}

/// A page asks for notifications: the computer's key comes with its feature, the subscription is
/// kept with the page's pairing (and turns notifications on at once), a bad one is refused, and
/// unsubscribing or unpairing lets it go.
#[tokio::test]
async fn a_page_hands_over_its_push_subscription() {
    let mut config = ServerConfig::new(host_info());
    config.bind = "127.0.0.1:0".parse().unwrap();
    let (key, _) = webpush::VapidKey::generate().unwrap();
    config.web = Some(
        WebConfig::new(None)
            .serving_from(PAGE)
            .with_push_key(key.public_key()),
    );
    let handle = RemoteServer::start(config, Arc::new(Host)).await.unwrap();
    let mut notices = handle.notices();
    let offer = handle.pairing_offer_with_code("K7Q2-9XMV");
    let link = link_payload(offer.web_url.as_deref().unwrap());
    let pinned: Vec<String> = link["certs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    let (_conn, mut page) = open(&handle, &pinned, Some(ORIGIN), "/trek").await.unwrap();
    say(
        &mut page,
        json!({"type":"pair","id":"1","protocol":1,"code":"K7Q2-9XMV","device_id":"web-1","device_name":"Pixel 9","platform":"web"}),
    )
    .await;
    let paired = hear(&mut page).await;
    assert!(
        paired["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "web_push"),
        "{paired}"
    );
    assert_eq!(hear(&mut page).await["type"], "snapshot");

    say(&mut page, json!({"type":"web_push","id":"2"})).await;
    let state = hear(&mut page).await;
    assert_eq!(state["type"], "web_push", "{state}");
    assert_eq!(state["key"], key.public_key().as_str());
    assert_eq!(
        (state["subscribed"].as_bool(), state["enabled"].as_bool()),
        (Some(false), Some(false))
    );

    // Not an https endpoint: refused, nothing kept.
    say(
        &mut page,
        json!({"type":"web_push","id":"3","subscribe":{"endpoint":"http://push.example/x","p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"}}),
    )
    .await;
    assert_eq!(hear(&mut page).await["code"], "bad_request");
    assert!(handle.push_targets().is_empty());

    let sub = json!({"endpoint":"https://fcm.googleapis.com/fcm/send/abc","p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"});
    say(&mut page, json!({"type":"web_push","id":"4","subscribe":sub})).await;
    let on = hear(&mut page).await;
    assert_eq!(on["subscribed"], true, "{on}");
    assert_eq!(
        on["enabled"], true,
        "asking for them turns them on, in this answer already"
    );
    let targets = handle.push_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, "web-1");
    assert_eq!(targets[0].1.endpoint, "https://fcm.googleapis.com/fcm/send/abc");
    let heard = loop {
        if let ServerNotice::PushChanged { device_id, on } = notices.recv().await.unwrap() {
            break (device_id, on);
        }
    };
    assert_eq!(heard, ("web-1".to_string(), true));

    say(&mut page, json!({"type":"web_push","id":"5","unsubscribe":true})).await;
    assert_eq!(hear(&mut page).await["subscribed"], false);
    assert!(handle.push_targets().is_empty());

    say(&mut page, json!({"type":"web_push","id":"6","subscribe":sub})).await;
    assert_eq!(hear(&mut page).await["subscribed"], true);
    assert!(handle.revoke("web-1"));
    assert!(handle.push_targets().is_empty(), "unpairing lets it go");
    handle.shutdown();
}
