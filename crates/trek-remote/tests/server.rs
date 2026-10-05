//! Loopback tests: a real server on 127.0.0.1 with a fake host, driven by tokio-tungstenite clients.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use trek_remote::*;

// ---------------------------------------------------------------------------------------------
// Fake host
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<String>>,
}

fn thread(id: &str) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        title: format!("Thread {id}"),
        project: ProjectRef { id: "p1".into(), name: "trek-api".into(), hue: 212, monogram: "TA".into() },
        agent: AgentRef { key: "claude-code".into(), name: "Claude Code".into() },
        model: None,
        model_label: None,
        run_state: RunState::Idle,
        needs: None,
        section: Section::Inbox,
        unseen: false,
        pinned: false,
        branch: None,
        worktree: false,
        activity: None,
        working_since: None,
        updated_at: 1,
        additions: 0,
        deletions: 0,
        effort: None,
        access: None,
        plan: false,
    }
}

fn item(id: &str, seq: u64, text: &str) -> Item {
    Item { id: id.into(), seq, at: None, body: ItemBody::Assistant { text: text.into(), streaming: false } }
}

impl FakeHost {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl RemoteHost for FakeHost {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        Ok(Snapshot { threads: vec![thread("t1"), thread("t2")], projects: vec![], agents: vec![], full_access: false })
    }

    async fn transcript(&self, thread_id: &str) -> HostResult<Transcript> {
        match thread_id {
            "t1" => Ok(Transcript { seq: 3, items: vec![item("a", 1, "one"), item("b", 2, "two"), item("c", 3, "three")] }),
            "t2" => Ok(Transcript { seq: 0, items: vec![] }),
            "slow" => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Ok(Transcript { seq: 3, items: vec![item("a", 3, "one")] })
            }
            other => Err(HostError::not_found(format!("No thread {other}"))),
        }
    }

    async fn send(&self, req: SendRequest) -> HostResult<()> {
        if req.thread_id == "missing" {
            return Err(HostError::not_found("No thread missing"));
        }
        self.record(format!("send {} {} {:?}", req.thread_id, req.text, req.mode));
        Ok(())
    }

    async fn new_thread(&self, req: NewThreadRequest) -> HostResult<String> {
        self.record(format!("new_thread {} {} {:?} {}", req.project_id, req.agent, req.model, req.worktree));
        Ok("t-new".into())
    }

    async fn answer(&self, req: AnswerRequest) -> HostResult<()> {
        if req.request_id == "done" {
            return Err(HostError::conflict("Already answered"));
        }
        self.record(format!("answer {} {} {:?}", req.thread_id, req.request_id, req.response));
        Ok(())
    }

    async fn interrupt(&self, thread_id: &str) -> HostResult<()> {
        if thread_id == "boom" {
            return Err(HostError::other("The agent crashed"));
        }
        self.record(format!("interrupt {thread_id}"));
        Ok(())
    }

    async fn mark_seen(&self, thread_id: &str) -> HostResult<()> {
        self.record(format!("mark_seen {thread_id}"));
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn host_info() -> HostInfo {
    HostInfo { id: "host-1".into(), name: "Test Mac".into(), version: "0.3.2".into() }
}

async fn start_with(f: impl FnOnce(&mut ServerConfig)) -> (RemoteHandle, Arc<FakeHost>) {
    let mut config = ServerConfig::new(host_info());
    config.bind = "127.0.0.1:0".parse().unwrap();
    f(&mut config);
    let host = Arc::new(FakeHost::default());
    let handle = RemoteServer::start(config, host.clone()).await.unwrap();
    (handle, host)
}

async fn start() -> (RemoteHandle, Arc<FakeHost>) {
    start_with(|_| {}).await
}

async fn connect(handle: &RemoteHandle) -> Client {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}", handle.local_addr())).await.unwrap();
    ws
}

async fn send(c: &mut Client, v: Value) {
    c.send(Message::text(v.to_string())).await.unwrap();
}

/// The next JSON message (skipping control frames); panics on close or after 5 s.
async fn recv(c: &mut Client) -> Value {
    let next = async {
        loop {
            match c.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str::<Value>(&text).unwrap(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => panic!("expected a message, got {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), next).await.expect("timed out waiting for a message")
}

/// Nothing arrives within `ms`.
async fn quiet(c: &mut Client, ms: u64) {
    let next = async {
        loop {
            match c.next().await {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => return other,
            }
        }
    };
    if let Ok(got) = tokio::time::timeout(Duration::from_millis(ms), next).await {
        panic!("expected silence, got {got:?}");
    }
}

/// The socket closes (after any number of non-text frames) within 5 s.
async fn closed(c: &mut Client) {
    let wait = async {
        loop {
            match c.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Text(text))) => panic!("expected close, got {text}"),
                Some(Ok(_)) => continue,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait).await.expect("socket didn't close");
}

/// Pair a new device; returns the authenticated client (snapshot consumed) and its token.
async fn pair(handle: &RemoteHandle, device_id: &str) -> (Client, String) {
    let offer = handle.pairing_offer();
    let mut c = connect(handle).await;
    send(
        &mut c,
        json!({"type": "pair", "id": "1", "protocol": 1, "code": offer.code, "device_id": device_id, "device_name": "Test iPhone", "app_version": "0.1.0"}),
    )
    .await;
    let paired = recv(&mut c).await;
    assert_eq!(paired["type"], "paired", "{paired}");
    let token = paired["token"].as_str().unwrap().to_string();
    let snapshot = recv(&mut c).await;
    assert_eq!(snapshot["type"], "snapshot");
    (c, token)
}

async fn hello(handle: &RemoteHandle, device_id: &str, token: &str) -> Client {
    let mut c = connect(handle).await;
    send(&mut c, json!({"type": "hello", "id": "h", "protocol": 1, "device_id": device_id, "token": token})).await;
    let welcome = recv(&mut c).await;
    assert_eq!(welcome["type"], "welcome", "{welcome}");
    assert_eq!(recv(&mut c).await["type"], "snapshot");
    c
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn hello_with_a_wrong_token_is_refused() {
    let (handle, _) = start().await;
    let (_phone, token) = pair(&handle, "dev-1").await;

    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "hello", "id": "1", "protocol": 1, "device_id": "dev-1", "token": format!("{token}x")})).await;
    let err = recv(&mut c).await;
    assert_eq!(err, json!({"type": "error", "re": "1", "code": "unauthorized", "message": err["message"]}));
    closed(&mut c).await;

    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "hello", "protocol": 1, "device_id": "nobody", "token": token})).await;
    assert_eq!(recv(&mut c).await["code"], "unauthorized");
    closed(&mut c).await;
}

#[tokio::test]
async fn only_pair_or_hello_before_authenticating() {
    let (handle, host) = start().await;
    for first in [
        json!({"type": "subscribe", "id": "1", "thread_id": "t1"}),
        json!({"type": "send", "id": "1", "thread_id": "t1", "text": "rm -rf /"}),
        json!({"type": "ping", "id": "1"}),
        json!({"type": "nonsense", "id": "1"}),
    ] {
        let mut c = connect(&handle).await;
        send(&mut c, first).await;
        let err = recv(&mut c).await;
        assert_eq!(err["code"], "unauthorized", "{err}");
        assert_eq!(err["re"], "1");
        closed(&mut c).await;
    }
    // Garbage closes too.
    let mut c = connect(&handle).await;
    c.send(Message::text("{not json")).await.unwrap();
    assert_eq!(recv(&mut c).await["code"], "unauthorized");
    closed(&mut c).await;
    // A malformed pair is a bad request, and still closes.
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "pair", "id": "2", "protocol": 1})).await;
    let err = recv(&mut c).await;
    assert_eq!((err["code"].as_str(), err["re"].as_str()), (Some("bad_request"), Some("2")));
    closed(&mut c).await;
    assert!(host.calls().is_empty());
}

#[tokio::test]
async fn browsers_are_refused() {
    let (handle, _) = start().await;
    let mut req = format!("ws://{}", handle.local_addr()).into_client_request().unwrap();
    req.headers_mut().insert("Origin", "http://evil.example".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected a 403, got {other:?}"),
    }
}

#[tokio::test]
async fn pairing_then_hello() {
    let (handle, _) = start().await;
    let mut notices = handle.notices();
    let offer = handle.pairing_offer();
    assert_eq!(offer.code.len(), 9);
    assert!(offer.url.starts_with(&format!("trek://pair?host={}&code={}&name=Test%20Mac&hid=host-1", handle.local_addr(), offer.code)));

    // A wrong protocol doesn't burn the code.
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "pair", "id": "0", "protocol": 2, "code": offer.code, "device_id": "dev-1", "device_name": "Phone"})).await;
    assert_eq!(recv(&mut c).await["code"], "unsupported_protocol");
    closed(&mut c).await;

    // Lowercase, without the dash.
    let typed = offer.code.replace('-', "").to_lowercase();
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "pair", "id": "1", "protocol": 1, "code": typed, "device_id": "dev-1", "device_name": "Tobias's iPhone"})).await;
    let paired = recv(&mut c).await;
    assert_eq!(paired["type"], "paired");
    assert_eq!(paired["re"], "1");
    assert_eq!(paired["protocol"], 1);
    assert_eq!(paired["host"], json!({"id": "host-1", "name": "Test Mac", "version": "0.3.2"}));
    let token = paired["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 43);
    let snapshot = recv(&mut c).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["threads"][0]["id"], "t1");
    assert_eq!(snapshot.get("re"), None);

    assert_eq!(
        notices.recv().await.unwrap(),
        ServerNotice::Paired { device_id: "dev-1".into(), name: "Tobias's iPhone".into() }
    );
    assert_eq!(notices.recv().await.unwrap(), ServerNotice::Connected { device_id: "dev-1".into() });
    assert_eq!(handle.connected_devices(), ["dev-1"]);
    let devices = handle.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Tobias's iPhone");

    // The paired connection is live.
    send(&mut c, json!({"type": "ping", "id": "p"})).await;
    assert_eq!(recv(&mut c).await, json!({"type": "pong", "re": "p"}));
    drop(c);
    assert_eq!(notices.recv().await.unwrap(), ServerNotice::Disconnected { device_id: "dev-1".into() });

    // Reconnect with the token.
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "hello", "id": "1", "protocol": 1, "device_id": "dev-1", "token": token, "app_version": "0.1.0"})).await;
    let welcome = recv(&mut c).await;
    assert_eq!(welcome, json!({"type": "welcome", "re": "1", "protocol": 1, "host": {"id": "host-1", "name": "Test Mac", "version": "0.3.2"}}));
    assert_eq!(recv(&mut c).await["type"], "snapshot");

    // The code was single use.
    let mut c2 = connect(&handle).await;
    send(&mut c2, json!({"type": "pair", "id": "1", "protocol": 1, "code": offer.code, "device_id": "dev-2", "device_name": "Other"})).await;
    let err = recv(&mut c2).await;
    assert_eq!(err["code"], "pairing_failed");
    closed(&mut c2).await;
    assert_eq!(handle.devices().len(), 1);
}

#[tokio::test]
async fn wrong_codes_burn_the_offer() {
    let (handle, _) = start().await;
    let offer = handle.pairing_offer_with_code("K7Q2-9XMV");
    assert_eq!(offer.code, "K7Q2-9XMV");
    for _ in 0..5 {
        let mut c = connect(&handle).await;
        send(&mut c, json!({"type": "pair", "protocol": 1, "code": "AAAA-AAAA", "device_id": "x", "device_name": "x"})).await;
        assert_eq!(recv(&mut c).await["code"], "pairing_failed");
        closed(&mut c).await;
    }
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "pair", "protocol": 1, "code": "K7Q2-9XMV", "device_id": "x", "device_name": "x"})).await;
    assert_eq!(recv(&mut c).await["code"], "pairing_failed");
    closed(&mut c).await;
}

#[tokio::test]
async fn subscriptions_route_items() {
    let (handle, _) = start().await;
    let (mut a, _) = pair(&handle, "dev-a").await;
    let (mut b, _) = pair(&handle, "dev-b").await;

    send(&mut a, json!({"type": "subscribe", "id": "7", "thread_id": "t1", "after_seq": null})).await;
    let t = recv(&mut a).await;
    assert_eq!(t["type"], "transcript");
    assert_eq!(t["re"], "7");
    assert_eq!(t["thread_id"], "t1");
    assert_eq!(t["reset"], true);
    assert_eq!(t["seq"], 3);
    assert_eq!(t["items"].as_array().unwrap().len(), 3);
    assert_eq!(t["items"][0], json!({"id": "a", "seq": 1, "at": null, "kind": "assistant", "text": "one", "streaming": false}));

    handle.push(HostEvent::Item { thread_id: "t1".into(), item: item("d", 4, "four") });
    let pushed = recv(&mut a).await;
    assert_eq!(pushed["type"], "item");
    assert_eq!(pushed["thread_id"], "t1");
    assert_eq!(pushed["item"]["id"], "d");
    assert_eq!(pushed.get("re"), None);

    // Items for other threads don't reach `a`; nothing reaches the unsubscribed `b`.
    handle.push(HostEvent::Item { thread_id: "t2".into(), item: item("z", 9, "other") });
    handle.push(HostEvent::TranscriptReset("t2".into()));
    // Thread events reach everyone, in order after the items.
    handle.push(HostEvent::Thread(thread("t9")));
    assert_eq!(recv(&mut a).await["type"], "thread");
    let to_b = recv(&mut b).await;
    assert_eq!(to_b["type"], "thread", "b only gets the thread upsert: {to_b}");
    assert_eq!(to_b["thread"]["id"], "t9");

    handle.push(HostEvent::TranscriptReset("t1".into()));
    assert_eq!(recv(&mut a).await, json!({"type": "transcript_reset", "thread_id": "t1"}));

    handle.push(HostEvent::ThreadRemoved("t9".into()));
    assert_eq!(recv(&mut a).await, json!({"type": "thread_removed", "thread_id": "t9"}));
    assert_eq!(recv(&mut b).await, json!({"type": "thread_removed", "thread_id": "t9"}));

    handle.push(HostEvent::Snapshot(Snapshot::default()));
    assert_eq!(recv(&mut b).await, json!({"type": "snapshot", "threads": [], "projects": [], "agents": []}));
    assert_eq!(recv(&mut a).await["type"], "snapshot");

    // Incremental and stale resubscribes.
    send(&mut a, json!({"type": "subscribe", "id": "8", "thread_id": "t1", "after_seq": 1})).await;
    let t = recv(&mut a).await;
    assert_eq!((t["reset"].as_bool(), t["seq"].as_u64()), (Some(false), Some(3)));
    let seqs: Vec<u64> = t["items"].as_array().unwrap().iter().map(|i| i["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [2, 3]);
    send(&mut a, json!({"type": "subscribe", "id": "9", "thread_id": "t1", "after_seq": 99})).await;
    let t = recv(&mut a).await;
    assert_eq!(t["reset"], true);
    assert_eq!(t["items"].as_array().unwrap().len(), 3);

    // Unsubscribing stops items.
    send(&mut a, json!({"type": "unsubscribe", "id": "u", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut a).await, json!({"type": "ack", "re": "u"}));
    handle.push(HostEvent::Item { thread_id: "t1".into(), item: item("e", 5, "five") });
    quiet(&mut a, 200).await;
    quiet(&mut b, 50).await;

    // Unknown threads.
    send(&mut b, json!({"type": "subscribe", "id": "x", "thread_id": "nope"})).await;
    assert_eq!(recv(&mut b).await["code"], "not_found");
}

#[tokio::test]
async fn items_pushed_while_a_transcript_loads_follow_it() {
    let (handle, _) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "slow"})).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Already in the transcript the host is building (seq <= 3): dropped.
    handle.push(HostEvent::Item { thread_id: "slow".into(), item: item("a", 2, "stale") });
    // Newer: delivered right after the transcript.
    handle.push(HostEvent::Item { thread_id: "slow".into(), item: item("b", 4, "new") });
    handle.push(HostEvent::TranscriptReset("slow".into()));
    let t = recv(&mut c).await;
    assert_eq!((t["type"].as_str(), t["re"].as_str()), (Some("transcript"), Some("s")));
    let next = recv(&mut c).await;
    assert_eq!((next["type"].as_str(), next["item"]["seq"].as_u64()), (Some("item"), Some(4)));
    assert_eq!(recv(&mut c).await, json!({"type": "transcript_reset", "thread_id": "slow"}));
    handle.push(HostEvent::Item { thread_id: "slow".into(), item: item("c", 5, "live") });
    assert_eq!(recv(&mut c).await["item"]["id"], "c");
}

#[tokio::test]
async fn actions_reach_the_host() {
    let (handle, host) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;

    let requests = [
        (json!({"type": "send", "id": "9", "thread_id": "t1", "text": "Also add a regression test", "mode": "steer"}), json!({"type": "ack", "re": "9"})),
        (json!({"type": "send", "id": "9b", "thread_id": "t1", "text": "later"}), json!({"type": "ack", "re": "9b"})),
        (
            json!({"type": "new_thread", "id": "10", "project_id": "p1", "agent": "codex", "model": "gpt-6", "text": "Add rate limiting", "worktree": true}),
            json!({"type": "ack", "re": "10", "thread_id": "t-new"}),
        ),
        (
            json!({"type": "answer", "id": "11", "thread_id": "t1", "request_id": "r1", "response": {"kind": "approval", "decision": "allow_for_session"}}),
            json!({"type": "ack", "re": "11"}),
        ),
        (
            json!({"type": "answer", "id": "12", "thread_id": "t1", "request_id": "r2", "response": {"kind": "questions", "answers": [{"question": "Q", "answer": "Both"}]}}),
            json!({"type": "ack", "re": "12"}),
        ),
        (
            json!({"type": "answer", "id": "13", "thread_id": "t1", "request_id": "r3", "response": {"kind": "plan", "approve": false, "feedback": "Skip step 3"}}),
            json!({"type": "ack", "re": "13"}),
        ),
        (json!({"type": "interrupt", "id": "14", "thread_id": "t1"}), json!({"type": "ack", "re": "14"})),
        (json!({"type": "mark_seen", "thread_id": "t1"}), json!({"type": "ack"})),
        (json!({"type": "ping", "id": "15"}), json!({"type": "pong", "re": "15"})),
        // Over the wire to the host (which here, like an older Mac, can't do them).
        (
            json!({"type": "set_prefs", "id": "16", "thread_id": "t1", "effort": "low"}),
            json!({"type": "error", "re": "16", "code": "bad_request", "message": "This Mac can't change a thread's settings"}),
        ),
        (
            json!({"type": "thread_action", "id": "17", "thread_id": "t1", "action": {"kind": "pin"}}),
            json!({"type": "error", "re": "17", "code": "bad_request", "message": "This Mac can't do that to a thread"}),
        ),
    ];
    for (request, reply) in requests {
        send(&mut c, request).await;
        assert_eq!(recv(&mut c).await, reply);
    }
    assert_eq!(
        host.calls(),
        [
            "send t1 Also add a regression test Some(Steer)",
            "send t1 later None",
            "new_thread p1 codex Some(\"gpt-6\") true",
            "answer t1 r1 Approval { decision: AllowForSession }",
            "answer t1 r2 Questions { answers: [QA { question: \"Q\", answer: \"Both\" }] }",
            "answer t1 r3 Plan { approve: false, feedback: Some(\"Skip step 3\") }",
            "interrupt t1",
            "mark_seen t1",
        ]
    );

    // Host errors keep their codes.
    send(&mut c, json!({"type": "send", "id": "e1", "thread_id": "missing", "text": "x"})).await;
    assert_eq!(recv(&mut c).await, json!({"type": "error", "re": "e1", "code": "not_found", "message": "No thread missing"}));
    send(&mut c, json!({"type": "answer", "id": "e2", "thread_id": "t1", "request_id": "done", "response": {"kind": "approval", "decision": "deny"}})).await;
    assert_eq!(recv(&mut c).await["code"], "conflict");
    send(&mut c, json!({"type": "interrupt", "id": "e3", "thread_id": "boom"})).await;
    assert_eq!(recv(&mut c).await["code"], "host_error");

    // Bad input is answered and the connection stays.
    c.send(Message::text("{oops")).await.unwrap();
    assert_eq!(recv(&mut c).await["code"], "bad_request");
    send(&mut c, json!({"type": "teleport", "id": "b1"})).await;
    let err = recv(&mut c).await;
    assert_eq!((err["code"].as_str(), err["re"].as_str()), (Some("bad_request"), Some("b1")));
    send(&mut c, json!({"type": "send", "id": "b2", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");
    send(&mut c, json!({"type": "hello", "id": "b3", "protocol": 1, "device_id": "x", "token": "y"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");
    send(&mut c, json!({"type": "ping", "id": "still"})).await;
    assert_eq!(recv(&mut c).await, json!({"type": "pong", "re": "still"}));
}

#[tokio::test]
async fn revoking_closes_the_connection() {
    let (handle, _) = start().await;
    let (mut c, token) = pair(&handle, "dev-1").await;
    assert!(handle.revoke("dev-1"));
    let err = recv(&mut c).await;
    assert_eq!(err["code"], "unauthorized");
    closed(&mut c).await;
    assert!(handle.devices().is_empty());
    assert!(!handle.revoke("dev-1"));

    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "hello", "id": "1", "protocol": 1, "device_id": "dev-1", "token": token})).await;
    assert_eq!(recv(&mut c).await["code"], "unauthorized");
    closed(&mut c).await;
}

#[tokio::test]
async fn one_connection_per_device() {
    let (handle, _) = start().await;
    let (mut first, token) = pair(&handle, "dev-1").await;
    let mut second = hello(&handle, "dev-1", &token).await;
    closed(&mut first).await;
    send(&mut second, json!({"type": "ping", "id": "p"})).await;
    assert_eq!(recv(&mut second).await["type"], "pong");
    assert_eq!(handle.connected_devices(), ["dev-1"]);
}

#[tokio::test]
async fn silent_sockets_time_out() {
    let (handle, _) = start_with(|c| c.auth_timeout = Duration::from_millis(300)).await;
    let mut c = connect(&handle).await;
    let started = std::time::Instant::now();
    closed(&mut c).await;
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn shutdown_closes_everything() {
    let (handle, _) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    handle.shutdown();
    closed(&mut c).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(tokio_tungstenite::connect_async(format!("ws://{}", handle.local_addr())).await.is_err());
}

#[tokio::test]
async fn oversized_messages_are_refused() {
    let (handle, host) = start_with(|c| c.max_message = 1024).await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    let text = "x".repeat(4096);
    let _ = c.send(Message::text(json!({"type": "send", "id": "1", "thread_id": "t1", "text": text}).to_string())).await;
    closed(&mut c).await;
    assert!(host.calls().is_empty());
}

#[tokio::test]
async fn devices_persist_across_restarts() {
    let dir = std::env::temp_dir().join(format!("trek-remote-it-{}-{}", std::process::id(), rand_suffix()));
    let path = dir.join("devices.json");
    let (handle, _) = start_with(|c| c.devices_path = Some(path.clone())).await;
    let (_c, token) = pair(&handle, "dev-1").await;
    handle.shutdown();

    let (handle, _) = start_with(|c| c.devices_path = Some(path.clone())).await;
    assert_eq!(handle.devices()[0].device_id, "dev-1");
    let _c = hello(&handle, "dev-1", &token).await;
    let _ = std::fs::remove_dir_all(&dir);
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

#[tokio::test]
async fn channel_host_round_trip() {
    let (host, requests) = ChannelHost::new();
    let app = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Ok(req) = requests.recv().await {
            match req {
                HostRequest::Snapshot { reply } => {
                    let _ = reply.send(Ok(Snapshot { threads: vec![thread("c1")], ..Default::default() }));
                }
                HostRequest::Transcript { thread_id, reply } => {
                    let _ = reply.send(Err(HostError::not_found(format!("No thread {thread_id}"))));
                }
                HostRequest::Send { req, reply } => {
                    seen.push(req.text);
                    let _ = reply.send(Ok(()));
                }
                HostRequest::SetPrefs { reply, .. } | HostRequest::ThreadAction { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
                HostRequest::NewThread { reply, .. } => {
                    let _ = reply.send(Ok("c2".into()));
                }
                // Dropping the reply: the phone gets a host error.
                HostRequest::Answer { .. } => {}
                HostRequest::Interrupt { reply, .. } | HostRequest::MarkSeen { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
        seen
    });

    assert_eq!(host.snapshot().await.unwrap().threads[0].id, "c1");
    assert_eq!(host.transcript("zz").await.unwrap_err().code, ErrorCode::NotFound);
    host.send(SendRequest { thread_id: "c1".into(), text: "hi".into(), mode: None, images: vec![] }).await.unwrap();
    let req = NewThreadRequest { project_id: "p".into(), agent: "codex".into(), model: None, text: "go".into(), worktree: false, effort: None, access: None, plan: false, images: vec![] };
    assert_eq!(host.new_thread(req).await.unwrap(), "c2");
    let answer = AnswerRequest {
        thread_id: "c1".into(),
        request_id: "r".into(),
        response: AnswerResponse::Approval { decision: Decision::Deny },
    };
    assert_eq!(host.answer(answer).await.unwrap_err().code, ErrorCode::HostError);
    host.interrupt("c1").await.unwrap();
    host.mark_seen("c1").await.unwrap();

    // Through the server too.
    let mut config = ServerConfig::new(host_info());
    config.bind = "127.0.0.1:0".parse().unwrap();
    let handle = RemoteServer::start(config, Arc::new(host.clone())).await.unwrap();
    let offer = handle.pairing_offer();
    let mut c = connect(&handle).await;
    send(&mut c, json!({"type": "pair", "id": "1", "protocol": 1, "code": offer.code, "device_id": "d", "device_name": "P"})).await;
    assert_eq!(recv(&mut c).await["type"], "paired");
    assert_eq!(recv(&mut c).await["threads"][0]["id"], "c1");
    send(&mut c, json!({"type": "send", "id": "2", "thread_id": "c1", "text": "from the phone"})).await;
    assert_eq!(recv(&mut c).await, json!({"type": "ack", "re": "2"}));
    handle.shutdown();
    drop(c);

    drop(host);
    drop(handle);
    // The server's copy of the host lives on in its tasks; the app's loop is still draining.
    app.abort();

    // A host whose receiver is gone answers with host_error.
    let (host, requests) = ChannelHost::new();
    drop(requests);
    assert_eq!(host.snapshot().await.unwrap_err().code, ErrorCode::HostError);
}
