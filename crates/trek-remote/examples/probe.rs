//! A phone in a terminal: pairs with a Trek from its `trek://pair` link (pinning the Mac's
//! certificate as the iPhone app does), lists the threads, opens one and prints what changes.
//!
//! ```sh
//! cargo run -p trek-remote --example probe -- 'trek://pair?host=…&code=…&fp=…' [--thread <id> [--send "text"] [--answer allow|deny]] [--request '{"type":"usage"}']… [--secs 20]
//! ```
//!
//! It opens and sends to a thread only when told which (`--thread`): a message reaches a real
//! agent in that thread's folder. `--request` (any number) sends a message as written once the
//! snapshot is in (an `id` is added) and prints the reply whole.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, cert: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        if trek_remote::tls::fingerprint(cert) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(tokio_rustls::rustls::Error::General("not the paired Mac".into()))
        }
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn param<'a>(url: &'a str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}="))).map(|v| v.replace("%3A", ":").replace("%2D", "-"))
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 220 { format!("{}…", &s[..220]) } else { s }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let url = args.first().expect("pass the trek://pair link").clone();
    let send_text = args.iter().position(|a| a == "--send").and_then(|i| args.get(i + 1)).cloned();
    let thread = args.iter().position(|a| a == "--thread").and_then(|i| args.get(i + 1)).cloned();
    let answer = args.iter().position(|a| a == "--answer").and_then(|i| args.get(i + 1)).cloned();
    let effort = args.iter().position(|a| a == "--effort").and_then(|i| args.get(i + 1)).cloned();
    let requests: Vec<Value> = args.windows(2).filter(|w| w[0] == "--request").map(|w| serde_json::from_str(&w[1]).expect("--request takes JSON")).collect();
    assert!(send_text.is_none() || thread.is_some(), "--send needs --thread <id>");
    let secs: u64 = args.iter().position(|a| a == "--secs").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(15);
    let host = param(&url, "host").expect("host=");
    let code = param(&url, "code").expect("code=");
    let fp = param(&url, "fp").expect("fp= (the Mac speaks TLS)");

    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { fingerprint: fp, provider }))
        .with_no_client_auth();
    let tcp = TcpStream::connect(&host).await.expect("connect");
    let tls = TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("trek.local").unwrap(), tcp).await.expect("TLS (pinned)");
    let (mut ws, _) = tokio_tungstenite::client_async(format!("wss://{host}/"), tls).await.expect("websocket");
    println!("connected to {host} over pinned TLS");

    let send = |v: Value| Message::text(v.to_string());
    ws.send(send(json!({"type": "pair", "id": "1", "protocol": 1, "code": code, "device_id": "probe", "device_name": "Probe (terminal)"}))).await.unwrap();
    let mut subscribed = false;
    let mut asked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = msg else { continue };
        let v: Value = serde_json::from_str(&text).unwrap();
        match v["type"].as_str() {
            Some("snapshot") => {
                let threads = v["threads"].as_array().cloned().unwrap_or_default();
                println!("snapshot: {} threads, {} projects, {} agents", threads.len(), v["projects"].as_array().map_or(0, |a| a.len()), v["agents"].as_array().map_or(0, |a| a.len()));
                for t in threads.iter().take(12) {
                    println!("  {} [{}] {} · {} · {}", t["id"].as_str().unwrap_or(""), t["section"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""), t["project"]["name"].as_str().unwrap_or(""), t["run_state"].as_str().unwrap_or(""));
                }
                if !asked {
                    asked = true;
                    for (n, mut r) in requests.iter().cloned().enumerate() {
                        r["id"] = json!(format!("q{n}"));
                        println!("> {r}");
                        ws.send(send(r)).await.unwrap();
                    }
                }
                if let (false, Some(id)) = (subscribed, thread.clone()) {
                    subscribed = true;
                    ws.send(send(json!({"type": "subscribe", "id": "2", "thread_id": id}))).await.unwrap();
                    if let Some(e) = &effort {
                        ws.send(send(json!({"type": "set_prefs", "id": "5", "thread_id": id, "effort": e}))).await.unwrap();
                    }
                    if let Some(text) = &send_text {
                        ws.send(send(json!({"type": "send", "id": "3", "thread_id": id, "text": text}))).await.unwrap();
                    }
                }
            }
            Some("item") if v["item"]["kind"] == "approval" && v["item"]["state"] == "pending" && answer.is_some() => {
                println!("approval asked: {} {}", v["item"]["title"], v["item"]["detail"]);
                let decision = answer.clone().unwrap();
                let reply = json!({"type": "answer", "id": "4", "thread_id": v["thread_id"], "request_id": v["item"]["request_id"], "response": {"kind": "approval", "decision": decision}});
                ws.send(send(reply)).await.unwrap();
                println!("answered: {decision}");
            }
            _ if v["re"].as_str().is_some_and(|re| re.starts_with('q')) => println!("< {}", serde_json::to_string_pretty(&v).unwrap()),
            Some("transcript") => {
                let items = v["items"].as_array().cloned().unwrap_or_default();
                let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap_or("?")).collect();
                println!("transcript of {}: {} items, seq {} ({})", v["thread_id"], items.len(), v["seq"], kinds.join(", "));
                for changes in items.iter().filter(|i| i["kind"] == "changes") {
                    println!("  changes: {changes}");
                }
            }
            _ => println!("{}", short(&v)),
        }
    }
    println!("done");
}
