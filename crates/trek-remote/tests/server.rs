//! Loopback tests: a real server on 127.0.0.1 with a fake host, driven by tokio-tungstenite clients.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
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
        agent: AgentRef::new("claude-code", "Claude Code", Some("claude-code")),
        updated_at: 1,
        ..Default::default()
    }
}

fn note(id: &str, body: &str) -> Note {
    Note { id: id.into(), title: body.lines().next().unwrap_or("Untitled").into(), body: body.into(), modified: 5 }
}

fn settings() -> MacSettings {
    MacSettings {
        default_agent: "claude-code".into(),
        default_model: None,
        default_effort: "high".into(),
        default_access: Access::AutoAcceptEdits,
        follow_up: SendMode::Steer,
        notifications: NotifyMode::BannerAndSound,
        push: PushSettings { enabled: false, when: PushWhen::Away, server: "https://ntfy.sh".into(), topic: String::new(), topic_url: None, subscribe_url: None },
        auto_settle_days: 3,
        theme: Theme::System,
        full_access: false,
    }
}

fn item(id: &str, seq: u64, text: &str) -> Item {
    Item { id: id.into(), seq, at: None, body: ItemBody::Assistant { text: text.into(), streaming: false } }
}

/// Ten items, `i0`…`i9`, with the files `i4`'s turn changed after it and an approval open.
fn long_transcript() -> Transcript {
    let mut items: Vec<Item> = (0..10).map(|n| item(&format!("i{n}"), n + 1, &format!("item {n}"))).collect();
    let changes = ItemBody::Changes { files: vec![], added: 1, removed: 0 };
    items.insert(5, Item { id: "c4".into(), seq: 11, at: None, body: changes });
    let approval = ItemBody::Approval { request_id: "req".into(), title: "Run command".into(), detail: "make".into(), state: ApprovalState::Pending };
    items.push(Item { id: "rreq".into(), seq: 12, at: None, body: approval });
    Transcript { seq: 12, items, ..Default::default() }
}

fn ids(v: &Value) -> Vec<&str> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect()
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
            "t1" => Ok(Transcript { seq: 3, items: vec![item("a", 1, "one"), item("b", 2, "two"), item("c", 3, "three")], ..Default::default() }),
            "t2" => Ok(Transcript::default()),
            "slow" => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Ok(Transcript { seq: 3, items: vec![item("a", 3, "one")], ..Default::default() })
            }
            "long" => Ok(long_transcript()),
            other => Err(HostError::not_found(format!("No thread {other}"))),
        }
    }

    async fn transcript_before(&self, thread_id: &str, before: &str, limit: u32) -> HostResult<TranscriptPage> {
        self.record(format!("transcript_before {thread_id} {before} {limit}"));
        if thread_id == "slow" {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let transcript = if thread_id == "slow" { long_transcript() } else { self.transcript(thread_id).await? };
        transcript.page_before(before, limit).ok_or_else(|| HostError::not_found("No such item"))
    }

    async fn turn_action(&self, req: TurnActionRequest) -> HostResult<TurnActionDone> {
        self.record(format!("turn_action {} {} {:?} {:?} {}", req.thread_id, req.item_id, req.action, req.model, req.restore_files));
        match req.action {
            TurnAction::Undo if req.item_id == "i9" => Err(HostError::bad_request("Stop the running turn to undo")),
            TurnAction::Undo | TurnAction::Rewind => Ok(TurnActionDone { thread_id: None, text: Some("Fix the parser".into()) }),
            TurnAction::Retry => Ok(TurnActionDone::default()),
            TurnAction::Fork => Ok(TurnActionDone { thread_id: Some("t-fork".into()), text: None }),
        }
    }

    fn unwatch(&self, thread_id: &str) {
        self.record(format!("unwatch {thread_id}"));
    }

    async fn send(&self, req: SendRequest) -> HostResult<Option<Open>> {
        if req.thread_id == "missing" {
            return Err(HostError::not_found("No thread missing"));
        }
        self.record(format!("send {} {} {:?}", req.thread_id, req.text, req.mode));
        // `/new` in a thread: the phone opens its own new-thread sheet.
        Ok((req.text == "/new").then(|| Open::NewThread { project_id: Some("p1".into()) }))
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

    async fn usage(&self) -> HostResult<Usage> {
        self.record("usage".into());
        let limit = UsageLimit { label: "5-hour limit".into(), percent: 42.0, resets_at: Some(9), window: "5h".into() };
        Ok(Usage { providers: vec![ProviderUsage { agent: thread("x").agent, plan: Some("Claude Max".into()), limits: vec![limit], note: None, error: None }], loading: false })
    }

    async fn basecamp(&self, range: BasecampRange) -> HostResult<Basecamp> {
        self.record(format!("basecamp {range:?}"));
        if range == BasecampRange::All {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Ok(Basecamp {
            range,
            greeting: "Good evening".into(),
            title: "This week's trek".into(),
            updated_at: 1,
            review: vec![],
            empty: true,
            invitation: Some("Nothing on the trail yet this week — start a thread.".into()),
            narrative: vec![],
            summary: None,
            profile: None,
            tiles: vec![],
        })
    }

    async fn notes(&self) -> HostResult<Vec<NoteSummary>> {
        self.record("notes".into());
        Ok(vec![NoteSummary { id: "n1".into(), title: "Groceries".into(), preview: "milk".into(), modified: 5 }])
    }

    async fn note(&self, note_id: &str) -> HostResult<Note> {
        self.record(format!("note {note_id}"));
        match note_id {
            "n1" => Ok(note("n1", "Groceries\nmilk")),
            other => Err(HostError::not_found(format!("No note {other}"))),
        }
    }

    async fn create_note(&self, body: String) -> HostResult<Note> {
        self.record(format!("create_note {body}"));
        Ok(note("n2", &body))
    }

    async fn save_note(&self, req: SaveNoteRequest) -> HostResult<Note> {
        self.record(format!("save_note {} {:?}", req.note_id, req.modified));
        if req.modified == Some(1) {
            return Err(HostError::conflict("It changed on the Mac"));
        }
        Ok(note(&req.note_id, &req.body))
    }

    async fn delete_note(&self, note_id: &str) -> HostResult<()> {
        self.record(format!("delete_note {note_id}"));
        Ok(())
    }

    async fn git_status(&self, target: GitTarget) -> HostResult<GitStatus> {
        self.record(format!("git_status {target:?}"));
        let file = ChangedFile { path: "src/a.rs".into(), status: FileStatus::Modified, from: None, added: 3, removed: 1, binary: false };
        Ok(GitStatus { is_repo: true, branch: Some("main".into()), files: vec![file], can_switch: true, ..Default::default() })
    }

    async fn git_diff(&self, req: GitDiffRequest) -> HostResult<GitDiff> {
        self.record(format!("git_diff {:?} {}", req.target, req.path));
        Ok(GitDiff { path: req.path, diff: "@@ -1 +1 @@\n-a\n+b".into(), truncated: false })
    }

    async fn git_commit(&self, req: GitCommitRequest) -> HostResult<()> {
        self.record(format!("git_commit {:?} {}", req.target, req.message));
        Ok(())
    }

    async fn git_push(&self, target: GitTarget) -> HostResult<()> {
        self.record(format!("git_push {target:?}"));
        Ok(())
    }

    async fn git_branches(&self, target: GitTarget) -> HostResult<GitBranches> {
        self.record(format!("git_branches {target:?}"));
        Ok(GitBranches { current: Some("main".into()), default_branch: Some("main".into()), branches: vec!["main".into(), "dev".into()] })
    }

    async fn git_switch(&self, req: GitSwitchRequest) -> HostResult<()> {
        self.record(format!("git_switch {:?} {}", req.target, req.branch));
        Ok(())
    }

    async fn worktree_merge(&self, thread_id: &str) -> HostResult<()> {
        self.record(format!("worktree_merge {thread_id}"));
        Ok(())
    }

    async fn worktree_remove(&self, req: WorktreeRemoveRequest) -> HostResult<()> {
        self.record(format!("worktree_remove {} {} {}", req.thread_id, req.delete_branch, req.force));
        if !req.force {
            return Err(HostError::conflict("2 uncommitted changes in the worktree would be lost."));
        }
        Ok(())
    }

    async fn commands(&self, thread_id: &str) -> HostResult<Vec<CommandInfo>> {
        self.record(format!("commands {thread_id}"));
        Ok(vec![CommandInfo { name: "usage".into(), description: "Show plan usage".into(), kind: CommandKind::Command, trek: true }])
    }

    async fn settings(&self) -> HostResult<MacSettings> {
        self.record("settings".into());
        Ok(settings())
    }

    async fn set_settings(&self, change: SettingsChange) -> HostResult<MacSettings> {
        self.record(format!("set_settings {:?}", change.theme));
        Ok(MacSettings { theme: change.theme.unwrap_or(Theme::System), ..settings() })
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
async fn replacing_a_connection_does_not_report_the_device_disconnected() {
    let (handle, _) = start().await;
    let mut notices = handle.notices();
    let (old, token) = pair(&handle, "dev-1").await;
    assert!(matches!(notices.recv().await.unwrap(), ServerNotice::Paired { .. }));
    assert_eq!(notices.recv().await.unwrap(), ServerNotice::Connected { device_id: "dev-1".into() });

    let new = hello(&handle, "dev-1", &token).await;
    assert_eq!(notices.recv().await.unwrap(), ServerNotice::Connected { device_id: "dev-1".into() });
    drop(old);
    assert!(tokio::time::timeout(Duration::from_millis(200), notices.recv()).await.is_err());
    assert_eq!(handle.connected_devices(), ["dev-1"]);

    drop(new);
    assert_eq!(notices.recv().await.unwrap(), ServerNotice::Disconnected { device_id: "dev-1".into() });
}

#[tokio::test]
async fn unauthenticated_connections_are_capped() {
    let (handle, _) = start_with(|config| config.auth_timeout = Duration::from_secs(5)).await;
    let mut held = Vec::new();
    for _ in 0..16 {
        held.push(TcpStream::connect(handle.local_addr()).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut refused = TcpStream::connect(handle.local_addr()).await.unwrap();
    let mut byte = [0];
    let read = tokio::time::timeout(Duration::from_secs(1), refused.read(&mut byte)).await.unwrap().unwrap();
    assert_eq!(read, 0, "the seventeenth unauthenticated socket is closed");
    assert_eq!(held.len(), 16);
}

#[tokio::test]
async fn pairing_keeps_the_offer_when_devices_cannot_be_saved() {
    let dir = std::env::temp_dir().join(format!("trek-remote-pair-save-{}", rand::random::<u64>()));
    let path = dir.join("devices.json");
    let (handle, _) = start_with(|config| config.devices_path = Some(path.clone())).await;
    std::fs::create_dir_all(&path).unwrap();
    let offer = handle.pairing_offer_with_code("K7Q2-9XMV");

    let mut failed = connect(&handle).await;
    send(&mut failed, json!({"type": "pair", "id": "1", "protocol": 1, "code": offer.code, "device_id": "dev-1", "device_name": "Phone"})).await;
    let err = recv(&mut failed).await;
    assert_eq!((err["code"].as_str(), handle.devices().len()), (Some("host_error"), 0));
    closed(&mut failed).await;

    std::fs::remove_dir(&path).unwrap();
    let mut retry = connect(&handle).await;
    send(&mut retry, json!({"type": "pair", "id": "2", "protocol": 1, "code": offer.code, "device_id": "dev-1", "device_name": "Phone"})).await;
    assert_eq!(recv(&mut retry).await["type"], "paired", "the same offer still works");
    assert_eq!(recv(&mut retry).await["type"], "snapshot");
    drop(retry);
    let _ = std::fs::remove_dir_all(&dir);
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

/// Every request added after the first phones, sent over the wire as the phone sends it: each
/// must get past the server's list of known types and reach the host.
#[tokio::test]
async fn newer_requests_reach_the_host_over_the_wire() {
    let (handle, host) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;

    let requests = [
        (json!({"type": "usage", "id": "u"}), "usage"),
        (json!({"type": "basecamp", "id": "b", "range": "week"}), "basecamp"),
        (json!({"type": "notes", "id": "n"}), "notes"),
        (json!({"type": "note", "id": "n1", "note_id": "n1"}), "note"),
        (json!({"type": "create_note", "id": "n2", "body": "Groceries"}), "note"),
        (json!({"type": "save_note", "id": "n3", "note_id": "n1", "body": "Groceries\nbread", "modified": 5}), "note"),
        (json!({"type": "delete_note", "id": "n4", "note_id": "n1"}), "ack"),
        (json!({"type": "git_status", "id": "g1", "thread_id": "t1"}), "git_status"),
        (json!({"type": "git_diff", "id": "g2", "project_id": "p1", "path": "src/a.rs"}), "git_diff"),
        (json!({"type": "git_commit", "id": "g3", "thread_id": "t1", "message": "Fix the race"}), "ack"),
        (json!({"type": "git_push", "id": "g4", "thread_id": "t1"}), "ack"),
        (json!({"type": "git_branches", "id": "g5", "project_id": "p1"}), "git_branches"),
        (json!({"type": "git_switch", "id": "g6", "project_id": "p1", "branch": "dev"}), "ack"),
        (json!({"type": "worktree_merge", "id": "w1", "thread_id": "t1"}), "ack"),
        (json!({"type": "worktree_remove", "id": "w2", "thread_id": "t1", "delete_branch": true, "force": true}), "ack"),
        (json!({"type": "commands", "id": "c", "thread_id": "t1"}), "commands"),
        (json!({"type": "settings", "id": "s1"}), "settings"),
        (json!({"type": "set_settings", "id": "s2", "theme": "paper"}), "settings"),
        (json!({"type": "transcript_before", "id": "tb", "thread_id": "long", "before": "i5", "limit": 2}), "transcript_page"),
        (json!({"type": "turn_action", "id": "ta", "thread_id": "t1", "item_id": "i4", "action": "fork"}), "ack"),
    ];
    for (request, kind) in requests {
        let id = request["id"].clone();
        send(&mut c, request.clone()).await;
        let reply = recv(&mut c).await;
        assert_eq!((reply["type"].as_str(), &reply["re"]), (Some(kind), &id), "{request} → {reply}");
    }
    // What came back is what the host said, whole.
    send(&mut c, json!({"type": "usage", "id": "u2"})).await;
    let usage = recv(&mut c).await;
    assert_eq!(usage["providers"][0]["limits"][0], json!({"label": "5-hour limit", "percent": 42.0, "resets_at": 9, "window": "5h"}));
    assert_eq!(usage["providers"][0]["agent"]["logo"], "claude-code");
    send(&mut c, json!({"type": "git_status", "id": "g7", "project_id": "p1"})).await;
    let status = recv(&mut c).await;
    assert_eq!(status["files"][0], json!({"path": "src/a.rs", "status": "modified", "added": 3, "removed": 1}));
    send(&mut c, json!({"type": "commands", "id": "c2", "thread_id": "t9"})).await;
    let commands = recv(&mut c).await;
    assert_eq!((commands["thread_id"].as_str(), commands["commands"][0]["trek"].as_bool()), (Some("t9"), Some(true)));
    send(&mut c, json!({"type": "set_settings", "id": "s3", "theme": "night"})).await;
    assert_eq!(recv(&mut c).await["theme"], "night");

    // Refusals keep their codes: a note changed on the Mac, a worktree with work to lose.
    send(&mut c, json!({"type": "save_note", "id": "x1", "note_id": "n1", "body": "x", "modified": 1})).await;
    assert_eq!(recv(&mut c).await["code"], "conflict");
    send(&mut c, json!({"type": "worktree_remove", "id": "x2", "thread_id": "t1"})).await;
    let refused = recv(&mut c).await;
    assert_eq!((refused["code"].as_str(), refused["message"].as_str()), (Some("conflict"), Some("2 uncommitted changes in the worktree would be lost.")));
    send(&mut c, json!({"type": "note", "id": "x3", "note_id": "zz"})).await;
    assert_eq!(recv(&mut c).await["code"], "not_found");
    // Malformed ones don't reach it.
    send(&mut c, json!({"type": "git_diff", "id": "x4", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");
    send(&mut c, json!({"type": "basecamp", "id": "x5", "range": "decade"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");

    // `/new` sent to a thread asks the phone to open its new-thread sheet.
    send(&mut c, json!({"type": "send", "id": "o", "thread_id": "t1", "text": "/new"})).await;
    assert_eq!(recv(&mut c).await, json!({"type": "ack", "re": "o", "open": {"screen": "new_thread", "project_id": "p1"}}));

    let calls = host.calls();
    for expected in [
        "usage",
        "basecamp Week",
        "notes",
        "note n1",
        "create_note Groceries",
        "save_note n1 Some(5)",
        "delete_note n1",
        "git_status GitTarget { thread_id: Some(\"t1\"), project_id: None }",
        "git_diff GitTarget { thread_id: None, project_id: Some(\"p1\") } src/a.rs",
        "git_commit GitTarget { thread_id: Some(\"t1\"), project_id: None } Fix the race",
        "git_push GitTarget { thread_id: Some(\"t1\"), project_id: None }",
        "git_branches GitTarget { thread_id: None, project_id: Some(\"p1\") }",
        "git_switch GitTarget { thread_id: None, project_id: Some(\"p1\") } dev",
        "worktree_merge t1",
        "worktree_remove t1 true true",
        "commands t1",
        "settings",
        "set_settings Some(Paper)",
        "transcript_before long i5 2",
        "turn_action t1 i4 Fork None true",
    ] {
        assert!(calls.iter().any(|c| c == expected), "{expected} not in {calls:?}");
    }
}

#[tokio::test]
async fn a_connection_can_have_only_eight_queries_in_flight() {
    let (handle, _) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    for n in 0..9 {
        send(&mut c, json!({"type": "basecamp", "id": format!("q{n}"), "range": "all"})).await;
    }
    let limited = recv(&mut c).await;
    assert_eq!((limited["re"].as_str(), limited["code"].as_str()), (Some("q8"), Some("rate_limited")));
    let mut replies = Vec::new();
    for _ in 0..8 {
        replies.push(recv(&mut c).await["re"].as_str().unwrap().to_string());
    }
    replies.sort();
    assert_eq!(replies, (0..8).map(|n| format!("q{n}")).collect::<Vec<_>>());
}

/// A slow read (usage asks every agent) doesn't hold up what the phone does next.
#[tokio::test]
async fn slow_reads_dont_hold_up_actions() {
    let (handle, _) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    send(&mut c, json!({"type": "basecamp", "id": "slow", "range": "all"})).await;
    send(&mut c, json!({"type": "interrupt", "id": "fast", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut c).await["re"], "fast");
    assert_eq!(recv(&mut c).await["re"], "slow");
    // Paging a long transcript in is a read too.
    send(&mut c, json!({"type": "transcript_before", "id": "page", "thread_id": "slow", "before": "i5", "limit": 2})).await;
    send(&mut c, json!({"type": "interrupt", "id": "fast2", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut c).await["re"], "fast2");
    assert_eq!(recv(&mut c).await["re"], "page");
}

#[tokio::test]
async fn a_limited_subscribe_sends_the_end_and_earlier_items_come_in_pages() {
    let (handle, host) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;

    // The last three items and the open request; more before them.
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "long", "limit": 3})).await;
    let t = recv(&mut c).await;
    assert_eq!((t["type"].as_str(), t["reset"].as_bool(), t["more"].as_bool()), (Some("transcript"), Some(true), Some(true)));
    assert_eq!(ids(&t), ["i7", "i8", "i9", "rreq"]);
    // A turn's files go with the turn end they follow, and only with it.
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "long", "limit": 6})).await;
    assert_eq!(ids(&recv(&mut c).await), ["i4", "c4", "i5", "i6", "i7", "i8", "i9", "rreq"]);
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "long", "limit": 5})).await;
    assert_eq!(ids(&recv(&mut c).await), ["i5", "i6", "i7", "i8", "i9", "rreq"]);
    // All of it fits: no `more`.
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "long", "limit": 10})).await;
    let t = recv(&mut c).await;
    assert_eq!((ids(&t).len(), t.get("more")), (12, None));
    // What changed since a seq is unchanged by a limit.
    send(&mut c, json!({"type": "subscribe", "id": "s", "thread_id": "long", "after_seq": 9, "limit": 1})).await;
    let t = recv(&mut c).await;
    assert_eq!((t["reset"].as_bool(), t.get("more"), ids(&t)), (Some(false), None, vec!["c4", "i9", "rreq"]));

    // Pages, back to the start.
    send(&mut c, json!({"type": "transcript_before", "id": "p1", "thread_id": "long", "before": "i7", "limit": 3})).await;
    let page = recv(&mut c).await;
    assert_eq!((page["type"].as_str(), page["re"].as_str(), page["thread_id"].as_str(), page["more"].as_bool()), (Some("transcript_page"), Some("p1"), Some("long"), Some(true)));
    assert_eq!(ids(&page), ["i4", "c4", "i5", "i6"]);
    send(&mut c, json!({"type": "transcript_before", "id": "p2", "thread_id": "long", "before": "i4", "limit": 3})).await;
    let page = recv(&mut c).await;
    assert_eq!((ids(&page), page["more"].as_bool()), (vec!["i1", "i2", "i3"], Some(true)));
    send(&mut c, json!({"type": "transcript_before", "id": "p3", "thread_id": "long", "before": "i1", "limit": 3})).await;
    let page = recv(&mut c).await;
    assert_eq!((ids(&page), page["more"].as_bool()), (vec!["i0"], Some(false)));
    // The page size is capped on the Mac; unknown items and malformed requests are refused.
    send(&mut c, json!({"type": "transcript_before", "id": "p4", "thread_id": "long", "before": "i9", "limit": 100000})).await;
    assert_eq!(recv(&mut c).await["more"], false);
    assert!(host.calls().contains(&"transcript_before long i9 500".to_string()));
    send(&mut c, json!({"type": "transcript_before", "id": "p5", "thread_id": "long", "before": "i99", "limit": 3})).await;
    assert_eq!(recv(&mut c).await["code"], "not_found");
    send(&mut c, json!({"type": "transcript_before", "id": "p6", "thread_id": "long", "before": "i3"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");
}

#[tokio::test]
async fn turn_actions_reach_the_host() {
    let (handle, host) = start().await;
    let (mut c, _) = pair(&handle, "dev-1").await;
    let requests = [
        (json!({"type": "turn_action", "id": "1", "thread_id": "t1", "item_id": "i4", "action": "undo"}), json!({"type": "ack", "re": "1", "text": "Fix the parser"})),
        (
            json!({"type": "turn_action", "id": "2", "thread_id": "t1", "item_id": "i4", "action": "retry", "model": "gpt-6", "restore_files": false}),
            json!({"type": "ack", "re": "2"}),
        ),
        (json!({"type": "turn_action", "id": "3", "thread_id": "t1", "item_id": "i4", "action": "fork"}), json!({"type": "ack", "re": "3", "thread_id": "t-fork"})),
        (json!({"type": "turn_action", "id": "4", "thread_id": "t1", "item_id": "i0", "action": "rewind"}), json!({"type": "ack", "re": "4", "text": "Fix the parser"})),
        (
            json!({"type": "turn_action", "id": "5", "thread_id": "t1", "item_id": "i9", "action": "undo"}),
            json!({"type": "error", "re": "5", "code": "bad_request", "message": "Stop the running turn to undo"}),
        ),
    ];
    for (request, reply) in requests {
        send(&mut c, request).await;
        assert_eq!(recv(&mut c).await, reply);
    }
    send(&mut c, json!({"type": "turn_action", "id": "6", "thread_id": "t1", "item_id": "i4", "action": "explode"})).await;
    assert_eq!(recv(&mut c).await["code"], "bad_request");
    let calls: Vec<String> = host.calls().into_iter().filter(|c| c.starts_with("turn_action")).collect();
    assert_eq!(
        calls,
        [
            "turn_action t1 i4 Undo None true",
            "turn_action t1 i4 Retry Some(\"gpt-6\") false",
            "turn_action t1 i4 Fork None true",
            "turn_action t1 i0 Rewind None true",
            "turn_action t1 i9 Undo None true",
        ]
    );
}

#[tokio::test]
async fn the_host_hears_when_no_phone_has_a_thread_open() {
    let (handle, host) = start().await;
    let (mut a, _) = pair(&handle, "dev-a").await;
    let (mut b, _) = pair(&handle, "dev-b").await;
    let unwatched = |host: &FakeHost| host.calls().into_iter().filter(|c| c.starts_with("unwatch")).collect::<Vec<_>>();
    for c in [&mut a, &mut b] {
        send(c, json!({"type": "subscribe", "id": "s", "thread_id": "t1"})).await;
        assert_eq!(recv(c).await["type"], "transcript");
    }
    // Subscribing twice is still one subscription.
    send(&mut a, json!({"type": "subscribe", "id": "s2", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut a).await["type"], "transcript");
    send(&mut a, json!({"type": "unsubscribe", "id": "u", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut a).await["type"], "ack");
    assert!(unwatched(&host).is_empty(), "b still has it open");
    // b goes away: nobody has it open.
    drop(b);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while unwatched(&host).is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the host wasn't told");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(unwatched(&host), ["unwatch t1"]);
    // Opened again, and let go again.
    send(&mut a, json!({"type": "subscribe", "id": "s3", "thread_id": "t1"})).await;
    assert_eq!(recv(&mut a).await["seq"], 3);
    send(&mut a, json!({"type": "unsubscribe", "thread_id": "t1"})).await;
    // A thread that couldn't be opened was never open.
    send(&mut a, json!({"type": "subscribe", "id": "x", "thread_id": "nope"})).await;
    assert_eq!(recv(&mut a).await["code"], "not_found");
    send(&mut a, json!({"type": "ping", "id": "p"})).await;
    assert_eq!(recv(&mut a).await["type"], "pong");
    assert_eq!(unwatched(&host), ["unwatch t1", "unwatch t1", "unwatch nope"]);
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
                HostRequest::Transcript { thread_id, reply, .. } => {
                    let _ = reply.send(Err(HostError::not_found(format!("No thread {thread_id}"))));
                }
                HostRequest::Send { req, reply } => {
                    seen.push(req.text);
                    let _ = reply.send(Ok(None));
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
                HostRequest::Notes { reply } => {
                    let _ = reply.send(Ok(vec![NoteSummary { id: "n1".into(), title: "Groceries".into(), preview: String::new(), modified: 1 }]));
                }
                HostRequest::Settings { reply } => {
                    let _ = reply.send(Ok(settings()));
                }
                // The rest go unanswered here (a host error).
                _ => {}
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
    assert_eq!(host.notes().await.unwrap()[0].title, "Groceries");
    assert_eq!(host.settings().await.unwrap().default_agent, "claude-code");
    assert_eq!(host.usage().await.unwrap_err().code, ErrorCode::HostError);

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
