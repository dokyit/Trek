//! TLS: a phone that pins the Mac's certificate fingerprint pairs over `wss://`; one pinned to
//! another certificate, or speaking plain `ws://`, gets nowhere.

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
use trek_remote::*;

#[derive(Debug, Default)]
struct NoopHost;

impl RemoteHost for NoopHost {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        Ok(Snapshot::default())
    }
    async fn transcript(&self, _: &str) -> HostResult<Transcript> {
        Ok(Transcript::default())
    }
    async fn send(&self, _: SendRequest) -> HostResult<Option<Open>> {
        Ok(None)
    }
    async fn new_thread(&self, _: NewThreadRequest) -> HostResult<String> {
        Ok("t".into())
    }
    async fn answer(&self, _: AnswerRequest) -> HostResult<()> {
        Ok(())
    }
    async fn interrupt(&self, _: &str) -> HostResult<()> {
        Ok(())
    }
}

/// Accepts exactly the certificate whose SHA-256 is `fingerprint`, as the phone does.
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
            Err(tokio_rustls::rustls::Error::General("certificate doesn't match the pinned fingerprint".into()))
        }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

async fn start() -> (RemoteHandle, TlsIdentity) {
    let identity = TlsIdentity::generate("Test Mac").unwrap();
    let mut config = ServerConfig::new(HostInfo { id: "h".into(), name: "Test Mac".into(), version: "0".into() });
    config.bind = "127.0.0.1:0".parse().unwrap();
    config.tls = Some(identity.clone());
    (RemoteServer::start(config, Arc::new(NoopHost)).await.unwrap(), identity)
}

async fn connect_pinned(handle: &RemoteHandle, fingerprint: &str) -> Result<tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>, String> {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { fingerprint: fingerprint.into(), provider }))
        .with_no_client_auth();
    let tcp = TcpStream::connect(handle.local_addr()).await.map_err(|e| e.to_string())?;
    let tls = TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("trek.local").unwrap(), tcp).await.map_err(|e| e.to_string())?;
    let (ws, _) = tokio_tungstenite::client_async("wss://trek.local/", tls).await.map_err(|e| e.to_string())?;
    Ok(ws)
}

#[tokio::test]
async fn a_pinned_phone_pairs_over_tls() {
    let (handle, identity) = start().await;
    let offer = handle.pairing_offer();
    assert!(offer.url.contains(&format!("&fp={}", identity.fingerprint)), "{}", offer.url);
    assert_eq!(offer.fingerprint.as_deref(), Some(identity.short_fingerprint().as_str()));

    let mut ws = connect_pinned(&handle, &identity.fingerprint).await.expect("pinned connection");
    let pair = json!({"type": "pair", "id": "1", "protocol": 1, "code": offer.code, "device_id": "d1", "device_name": "iPhone"});
    ws.send(Message::text(pair.to_string())).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(reply["type"], "paired", "{reply}");
}

#[tokio::test]
async fn another_certificate_or_plain_websocket_is_refused() {
    let (handle, _) = start().await;
    let wrong = "0".repeat(64);
    assert!(connect_pinned(&handle, &wrong).await.is_err(), "a different certificate isn't the Mac");
    // Plain ws:// to a TLS server: no WebSocket.
    let plain = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(format!("ws://{}", handle.local_addr()))).await.unwrap();
    assert!(plain.is_err());
}
