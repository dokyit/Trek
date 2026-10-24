//! The Rust client against a real server on 127.0.0.1, over TLS pinned to the server's
//! certificate, as one Trek talks to another.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use trek_remote::client::{
    Auth, Client, ClientError, ClientOptions, Event, Paired, State, StopReason,
};
use trek_remote::*;

#[derive(Default)]
struct Host {
    calls: Mutex<Vec<String>>,
}

impl Host {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn thread(id: &str) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        title: format!("Thread {id}"),
        project: ProjectRef {
            id: "p1".into(),
            name: "trek".into(),
            hue: 212,
            monogram: "TR".into(),
            icon: None,
        },
        agent: AgentRef::new("claude-code", "Claude Code", Some("claude-code")),
        updated_at: 1,
        ..Default::default()
    }
}

fn item(id: &str, seq: u64, text: &str) -> Item {
    Item {
        id: id.into(),
        seq,
        at: None,
        body: ItemBody::Assistant {
            text: text.into(),
            streaming: false,
        },
    }
}

impl RemoteHost for Host {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        Ok(Snapshot {
            threads: vec![thread("t1")],
            ..Default::default()
        })
    }
    async fn transcript(&self, thread_id: &str) -> HostResult<Transcript> {
        self.record(format!("transcript {thread_id}"));
        match thread_id {
            "t1" => Ok(Transcript {
                seq: 2,
                items: vec![item("a", 1, "one"), item("b", 2, "two")],
                ..Default::default()
            }),
            other => Err(HostError::not_found(format!("No thread {other}"))),
        }
    }
    async fn send(&self, _: SendRequest) -> HostResult<Option<Open>> {
        Ok(None)
    }
    async fn new_thread(&self, _: NewThreadRequest) -> HostResult<String> {
        Ok("t-new".into())
    }
    async fn answer(&self, _: AnswerRequest) -> HostResult<()> {
        Ok(())
    }
    async fn interrupt(&self, _: &str) -> HostResult<()> {
        Ok(())
    }
    async fn machine_info(&self) -> HostResult<MachineInfo> {
        Ok(MachineInfo {
            name: "Studio".into(),
            platform: "macos".into(),
            trek_version: "0.4.7".into(),
            home: "/Users/me".into(),
        })
    }
    async fn clone_project(
        &self,
        req: CloneProjectRequest,
        progress: CloneProgress,
    ) -> HostResult<String> {
        self.record(format!(
            "clone_project {} {} {}",
            req.url, req.folder, req.name
        ));
        progress.line("Cloning into 'trek'...");
        progress.line("Receiving objects: 100% (12/12), done.");
        if req.url.contains("missing") {
            return Err(HostError::clone_failed("fatal: repository not found"));
        }
        Ok("p-new".into())
    }
    async fn put_files(&self, project_id: &str, files: Vec<FileUpload>) -> HostResult<()> {
        for f in files {
            self.record(format!(
                "put_file {project_id} {} {} {:o}",
                f.path,
                String::from_utf8_lossy(&f.bytes),
                f.mode
            ));
        }
        Ok(())
    }
}

/// A server on 127.0.0.1 with its own certificate and its paired devices in `dir`.
struct Machine {
    handle: RemoteHandle,
    host: Arc<Host>,
    identity: TlsIdentity,
    dir: PathBuf,
}

async fn serve(bind: SocketAddr, identity: TlsIdentity, dir: PathBuf) -> Machine {
    let mut config = ServerConfig::new(HostInfo {
        id: "studio-id".into(),
        name: "Studio".into(),
        version: "0.4.7".into(),
    });
    config.bind = bind;
    config.tls = Some(identity.clone());
    config.devices_path = Some(dir.join("devices.json"));
    config.features = feature::ALL.iter().map(|f| f.to_string()).collect();
    let host = Arc::new(Host::default());
    let handle = RemoteServer::start(config, host.clone()).await.unwrap();
    Machine {
        handle,
        host,
        identity,
        dir,
    }
}

async fn machine() -> Machine {
    let dir = std::env::temp_dir().join(format!(
        "trek-remote-client-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    serve(
        "127.0.0.1:0".parse().unwrap(),
        TlsIdentity::generate("Studio").unwrap(),
        dir,
    )
    .await
}

/// Unique within the run (tests starting at once can read the same nanosecond): each server has
/// its own devices.json, as two sharing one forget each other's devices.
fn rand_suffix() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "{n}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

impl Machine {
    fn routes(&self) -> Vec<String> {
        vec![self.handle.local_addr().to_string()]
    }

    fn pair_auth(&self, device_id: &str) -> Auth {
        Auth::Pair {
            code: self.handle.pairing_offer().code,
            device_id: device_id.into(),
            device_name: "Machine A".into(),
        }
    }
}

fn quick() -> ClientOptions {
    ClientOptions {
        backoff_first: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        ..ClientOptions::default()
    }
}

/// The next event that `pick` takes, skipping others; panics after 15 s (a hang, not a slow
/// machine: a full test run in parallel kept a 5 s wait from seeing a probe's 500 ms).
async fn next<T>(
    events: &mut mpsc::UnboundedReceiver<Event>,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    let wait = async {
        loop {
            let event = events.recv().await.expect("the client's events ended");
            if let Some(found) = pick(event) {
                return found;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), wait)
        .await
        .expect("timed out waiting for an event")
}

async fn connected(events: &mut mpsc::UnboundedReceiver<Event>) -> (Paired, bool) {
    next(events, |e| match e {
        Event::Connected {
            paired,
            newly_paired,
            ..
        } => Some((paired, newly_paired)),
        _ => None,
    })
    .await
}

async fn state(state: &mut watch::Receiver<State>, want: impl Fn(&State) -> bool) -> State {
    tokio::time::timeout(Duration::from_secs(5), state.wait_for(|s| want(s)))
        .await
        .expect("timed out waiting for a state")
        .expect("the client is gone")
        .clone()
}

/// Pair a client; returns it, its events and what it kept.
async fn paired_client(
    m: &Machine,
    device_id: &str,
) -> (Client, mpsc::UnboundedReceiver<Event>, Paired) {
    let client = Client::connect_with(
        m.routes(),
        &m.identity.fingerprint,
        m.pair_auth(device_id),
        quick(),
    )
    .unwrap();
    let mut events = client.events().unwrap();
    let (paired, newly) = connected(&mut events).await;
    assert!(newly);
    (client, events, paired)
}

#[tokio::test]
async fn pairs_then_says_hello_with_what_it_kept() {
    let m = machine().await;
    let client = Client::connect_with(
        m.routes(),
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        quick(),
    )
    .unwrap();
    let mut events = client.events().unwrap();
    assert!(client.events().is_none(), "one receiver");
    let (paired, newly) = connected(&mut events).await;
    assert!(newly);
    assert_eq!(
        (
            paired.device_id.as_str(),
            paired.fingerprint.as_str(),
            paired.host.name.as_str()
        ),
        ("machine-a", m.identity.fingerprint.as_str(), "Studio")
    );
    assert_eq!(paired.token.len(), 43, "32 random bytes, base64url");
    assert_eq!(paired.routes, m.routes());
    assert!(paired.serves(feature::MACHINES));
    let snapshot = next(&mut events, |e| match e {
        Event::Message(ServerMessage::Snapshot(s)) => Some(s),
        _ => None,
    })
    .await;
    assert_eq!(snapshot.threads[0].id, "t1");
    assert_eq!(*client.state().borrow(), State::Connected);
    client.close();
    assert_eq!(
        state(&mut client.state(), |s| matches!(s, State::Stopped(_))).await,
        State::Stopped(StopReason::Closed)
    );
    assert!(matches!(
        client.machine_info().await,
        Err(ClientError::Stopped(StopReason::Closed))
    ));

    // What it kept takes it back in, with `hello`.
    let again = Client::resume(&paired).unwrap();
    let mut events = again.events().unwrap();
    let (kept, newly) = connected(&mut events).await;
    assert!(!newly);
    assert_eq!(kept, paired, "same token, same routes");
    assert!(
        m.handle
            .devices()
            .iter()
            .any(|d| d.device_id == "machine-a" && d.name == "Machine A")
    );
}

#[tokio::test]
async fn a_request_gets_its_typed_reply() {
    let m = machine().await;
    let (client, _events, _) = paired_client(&m, "machine-a").await;
    let info = client.machine_info().await.unwrap();
    assert_eq!(
        info,
        MachineInfo {
            name: "Studio".into(),
            platform: "macos".into(),
            trek_version: "0.4.7".into(),
            home: "/Users/me".into()
        }
    );
    // The machine's `error`, as such.
    let err = client.request(ClientMessage::Usage).await.unwrap_err();
    assert!(
        matches!(err, ClientError::Server { code: ErrorCode::BadRequest, ref message } if message.starts_with("This Mac can't")),
        "{err:?}"
    );
    // Several at once each get their own.
    let (a, b) = tokio::join!(client.machine_info(), client.request(ClientMessage::Ping));
    assert_eq!(a.unwrap().name, "Studio");
    assert_eq!(b.unwrap(), ServerMessage::Pong);
}

#[tokio::test]
async fn pushes_arrive_after_subscribe() {
    let m = machine().await;
    let (client, mut events, _) = paired_client(&m, "machine-a").await;
    let reply = client
        .request(ClientMessage::Subscribe {
            thread_id: "t1".into(),
            after_seq: None,
            limit: None,
        })
        .await
        .unwrap();
    let ServerMessage::Transcript {
        thread_id,
        seq,
        items,
        reset,
        ..
    } = reply
    else {
        panic!("{reply:?}")
    };
    assert_eq!(
        (thread_id.as_str(), seq, items.len(), reset),
        ("t1", 2, 2, true)
    );

    // An item of a thread it doesn't have open doesn't come; one of t1 does, and a row change.
    m.handle.push(HostEvent::Item {
        thread_id: "t9".into(),
        item: item("x", 1, "elsewhere"),
    });
    m.handle.push(HostEvent::Item {
        thread_id: "t1".into(),
        item: item("c", 3, "three"),
    });
    m.handle.push(HostEvent::Thread(ThreadSummary {
        title: "Renamed".into(),
        ..thread("t1")
    }));
    let (thread_id, got) = next(&mut events, |e| match e {
        Event::Message(ServerMessage::Item { thread_id, item }) => Some((thread_id, item)),
        _ => None,
    })
    .await;
    assert_eq!((thread_id.as_str(), got.id.as_str()), ("t1", "c"));
    let row = next(&mut events, |e| match e {
        Event::Message(ServerMessage::Thread { thread }) => Some(thread),
        _ => None,
    })
    .await;
    assert_eq!(row.title, "Renamed");
}

/// A fixed address in front of a server that may go away and come back elsewhere (`None`: nothing
/// answers): the machine's route, as the client sees it, without its port given up meanwhile for
/// anything else on this computer to take.
async fn front(target: Arc<Mutex<Option<SocketAddr>>>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = listener.accept().await {
            let Some(to) = *target.lock().unwrap() else {
                continue;
            };
            tokio::spawn(async move {
                if let Ok(mut outbound) = tokio::net::TcpStream::connect(to).await {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn reconnects_after_the_server_drops_and_subscribes_again_after_welcome() {
    let m = machine().await;
    let target = Arc::new(Mutex::new(Some(m.handle.local_addr())));
    let route = front(target.clone()).await.to_string();
    let client = Client::connect_with(
        vec![route],
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        quick(),
    )
    .unwrap();
    let mut events = client.events().unwrap();
    let (paired, _) = connected(&mut events).await;
    client
        .request(ClientMessage::Subscribe {
            thread_id: "t1".into(),
            after_seq: None,
            limit: None,
        })
        .await
        .unwrap();
    m.handle.push(HostEvent::Item {
        thread_id: "t1".into(),
        item: item("c", 3, "three"),
    });
    next(&mut events, |e| {
        matches!(e, Event::Message(ServerMessage::Item { .. })).then_some(())
    })
    .await;

    // The machine goes away (Trek quits), and comes back.
    *target.lock().unwrap() = None;
    m.handle.shutdown();
    let reason = next(&mut events, |e| match e {
        Event::Disconnected { reason } => Some(reason),
        _ => None,
    })
    .await;
    assert!(!reason.is_empty());
    let mut states = client.state();
    state(&mut states, |s| *s == State::Connecting).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let back = serve(
        "127.0.0.1:0".parse().unwrap(),
        m.identity.clone(),
        m.dir.clone(),
    )
    .await;
    *target.lock().unwrap() = Some(back.handle.local_addr());

    // `welcome` first, then the thread it had open, asked for from where it was (seq 3).
    let mut order = Vec::new();
    let transcript = next(&mut events, |e| match e {
        Event::Connected {
            paired: p,
            newly_paired,
            ..
        } => {
            assert!(!newly_paired);
            assert_eq!(p.token, paired.token);
            order.push("connected");
            None
        }
        Event::Message(ServerMessage::Snapshot(_)) => {
            order.push("snapshot");
            None
        }
        Event::Message(t @ ServerMessage::Transcript { .. }) => Some(t),
        _ => None,
    })
    .await;
    assert_eq!(order.first(), Some(&"connected"), "{order:?}");
    let ServerMessage::Transcript {
        thread_id,
        reset,
        seq,
        ..
    } = transcript
    else {
        unreachable!()
    };
    assert_eq!((thread_id.as_str(), seq), ("t1", 2));
    assert!(
        reset,
        "seq 3 is past what the new host has (2): it sends it whole"
    );
    assert_eq!(
        *client.state().borrow(),
        State::Connected,
        "never refused for subscribing before hello"
    );
    assert!(back.host.calls().contains(&"transcript t1".to_string()));

    // And the subscription is live there.
    back.handle.push(HostEvent::Item {
        thread_id: "t1".into(),
        item: item("d", 3, "again"),
    });
    let got = next(&mut events, |e| match e {
        Event::Message(ServerMessage::Item { item, .. }) => Some(item),
        _ => None,
    })
    .await;
    assert_eq!(got.id, "d");
    // Requests work on the new connection.
    assert_eq!(client.machine_info().await.unwrap().name, "Studio");
}

#[tokio::test]
async fn stops_on_unauthorized_and_on_another_certificate() {
    let m = machine().await;
    let (client, _events, paired) = paired_client(&m, "machine-a").await;
    // Revoked on the machine: the live connection is told, and the client stops for good.
    assert!(m.handle.revoke("machine-a"));
    let stopped = state(&mut client.state(), |s| matches!(s, State::Stopped(_))).await;
    assert!(
        matches!(stopped, State::Stopped(StopReason::Unauthorized(_))),
        "{stopped:?}"
    );

    // A token it doesn't know.
    let refused =
        Client::connect_with(m.routes(), &paired.fingerprint, paired.auth(), quick()).unwrap();
    let stopped = state(&mut refused.state(), |s| matches!(s, State::Stopped(_))).await;
    assert!(
        matches!(stopped, State::Stopped(StopReason::Unauthorized(_))),
        "{stopped:?}"
    );
    assert!(matches!(
        refused.request(ClientMessage::Ping).await,
        Err(ClientError::Stopped(StopReason::Unauthorized(_)))
    ));

    // A code it didn't show.
    let wrong = Auth::Pair {
        code: "ZZZZ-ZZZZ".into(),
        device_id: "machine-b".into(),
        device_name: "B".into(),
    };
    let refused =
        Client::connect_with(m.routes(), &m.identity.fingerprint, wrong, quick()).unwrap();
    let stopped = state(&mut refused.state(), |s| matches!(s, State::Stopped(_))).await;
    assert!(
        matches!(stopped, State::Stopped(StopReason::PairingFailed(_))),
        "{stopped:?}"
    );

    // Another certificate: nothing is sent, and it stops.
    let other = TlsIdentity::generate("Impostor").unwrap();
    let pinned = Client::connect_with(
        m.routes(),
        &other.fingerprint,
        m.pair_auth("machine-c"),
        quick(),
    )
    .unwrap();
    let stopped = state(&mut pinned.state(), |s| matches!(s, State::Stopped(_))).await;
    assert_eq!(
        stopped,
        State::Stopped(StopReason::FingerprintMismatch {
            expected: other.fingerprint.clone(),
            seen: m.identity.fingerprint.clone()
        })
    );
    assert!(
        !m.handle
            .devices()
            .iter()
            .any(|d| d.device_id == "machine-c"),
        "the code never left"
    );
}

#[tokio::test]
async fn the_first_route_that_answers_wins() {
    let m = machine().await;
    // Nothing listens on the first one.
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let routes = vec![dead.clone(), m.handle.local_addr().to_string()];
    let client = Client::connect_with(
        routes,
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        quick(),
    )
    .unwrap();
    let mut events = client.events().unwrap();
    let route = next(&mut events, |e| match e {
        Event::Connected { route, .. } => Some(route),
        _ => None,
    })
    .await;
    assert_eq!(route, m.handle.local_addr().to_string());
}

#[tokio::test]
async fn put_files_refuses_paths_outside_the_project() {
    let m = machine().await;
    let (client, _events, _) = paired_client(&m, "machine-a").await;
    for path in ["../x", "a/../../x", "/etc/passwd", ".git/config"] {
        let req = PutFilesRequest {
            project_id: "p1".into(),
            files: vec![PutFile::new(path, b"x", None)],
        };
        let err = client
            .request(ClientMessage::PutFiles(req))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ClientError::Server {
                    code: ErrorCode::BadRequest,
                    ..
                }
            ),
            "{path}: {err:?}"
        );
    }
    assert!(
        m.host.calls().is_empty(),
        "none reached the host: {:?}",
        m.host.calls()
    );

    let files = vec![
        FileUpload {
            path: ".env".into(),
            bytes: b"KEY=1".to_vec(),
            mode: 0o600,
        },
        FileUpload {
            path: "config/local.toml".into(),
            bytes: b"a = 1".to_vec(),
            mode: 0o644,
        },
    ];
    client.put_files("p1", &files).await.unwrap();
    assert_eq!(
        m.host.calls(),
        [
            "put_file p1 .env KEY=1 600",
            "put_file p1 config/local.toml a = 1 644"
        ]
    );
}

#[tokio::test]
async fn clone_project_reaches_the_host() {
    let m = machine().await;
    let (client, mut events, _) = paired_client(&m, "machine-a").await;
    let req = CloneProjectRequest {
        url: "git@github.com:dokyit/trek.git".into(),
        folder: "~/trek".into(),
        name: "trek".into(),
    };
    assert_eq!(client.clone_project(req).await.unwrap(), "p-new");
    assert_eq!(
        m.host.calls(),
        ["clone_project git@github.com:dokyit/trek.git ~/trek trek"]
    );
    // Git's output came first.
    let mut lines = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let Event::Message(ServerMessage::CloneProgress { folder, line }) = event {
            assert_eq!(folder, "~/trek");
            lines.push(line);
        }
    }
    assert_eq!(
        lines,
        [
            "Cloning into 'trek'...",
            "Receiving objects: 100% (12/12), done."
        ]
    );

    let failing = CloneProjectRequest {
        url: "https://github.com/dokyit/missing.git".into(),
        folder: "~/missing".into(),
        name: "missing".into(),
    };
    let err = client.clone_project(failing).await.unwrap_err();
    assert_eq!(
        err,
        ClientError::Server {
            code: ErrorCode::CloneFailed,
            message: "fatal: repository not found".into()
        }
    );

    // What git could take for an option, or a command, never reaches it.
    let sneaky = CloneProjectRequest {
        url: "ext::sh -c touch% /tmp/pwned".into(),
        folder: "~/x".into(),
        name: "x".into(),
    };
    let err = client.clone_project(sneaky).await.unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Server {
                code: ErrorCode::BadRequest,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(m.host.calls().len(), 2);
}

#[tokio::test]
async fn messages_over_the_limit_are_not_sent() {
    let m = machine().await;
    let (client, _events, _) = paired_client(&m, "machine-a").await;
    let huge = SendRequest {
        thread_id: "t1".into(),
        text: "x".repeat(client::MAX_OUTBOUND),
        mode: None,
        images: vec![],
    };
    assert!(matches!(
        client.request(ClientMessage::Send(huge)).await,
        Err(ClientError::TooLarge(_))
    ));
    assert_eq!(
        client.machine_info().await.unwrap().name,
        "Studio",
        "the connection is still up"
    );
}

/// A route that takes the connection and then says nothing (TLS never answers): a machine's old
/// address on a network this computer has left, as the client meets it.
async fn silent_route() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let route = listener.local_addr().unwrap().to_string();
    (listener, route)
}

#[tokio::test]
async fn a_route_that_never_answers_doesnt_hold_up_the_next() {
    let m = machine().await;
    let (_held, silent) = silent_route().await;
    let options = ClientOptions {
        connect_timeout: Duration::from_secs(8),
        ..quick()
    };
    let started = std::time::Instant::now();
    let client = Client::connect_with(
        vec![silent.clone(), m.handle.local_addr().to_string()],
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        options,
    )
    .unwrap();
    let mut events = client.events().unwrap();
    let route = next(&mut events, |e| match e {
        Event::Connected { route, .. } => Some(route),
        _ => None,
    })
    .await;
    assert_eq!(route, m.handle.local_addr().to_string());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "dialled alongside the silent one, not after its timeout: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_network_change_dials_at_once_instead_of_waiting_out_the_backoff() {
    let m = machine().await;
    let target = Arc::new(Mutex::new(Some(m.handle.local_addr())));
    let route = front(target.clone()).await.to_string();
    let slow = ClientOptions {
        backoff_first: Duration::from_secs(60),
        backoff_max: Duration::from_secs(60),
        ..ClientOptions::default()
    };
    let client = Client::connect_with(
        vec![route],
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        slow,
    )
    .unwrap();
    let mut events = client.events().unwrap();
    connected(&mut events).await;

    // The machine goes away; the client waits a minute before dialling again.
    *target.lock().unwrap() = None;
    m.handle.shutdown();
    next(&mut events, |e| {
        matches!(e, Event::Disconnected { .. }).then_some(())
    })
    .await;
    let back = serve(
        "127.0.0.1:0".parse().unwrap(),
        m.identity.clone(),
        m.dir.clone(),
    )
    .await;
    *target.lock().unwrap() = Some(back.handle.local_addr());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let now = client.state().borrow().clone();
    assert_eq!(now, State::Connecting, "still waiting");

    // This computer joined another network: it dials now.
    client.network_changed();
    let (_, newly) = connected(&mut events).await;
    assert!(!newly);
}

#[tokio::test]
async fn a_connection_lost_with_the_old_network_is_noticed_at_once() {
    let m = machine().await;
    // A route that forwards, then goes quiet without closing (Wi-Fi gone under it).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let route = listener.local_addr().unwrap().to_string();
    let to = m.handle.local_addr();
    let quiet = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let q = quiet.clone();
    tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            let q = q.clone();
            tokio::spawn(async move {
                let outbound = tokio::net::TcpStream::connect(to).await.unwrap();
                let (mut ir, mut iw) = inbound.into_split();
                let (mut or, mut ow) = outbound.into_split();
                let q1 = q.clone();
                let up = async move {
                    let mut buf = [0u8; 8192];
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    while let Ok(n) = ir.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        if !q1.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = ow.write_all(&buf[..n]).await;
                        }
                    }
                };
                let down = async move {
                    let mut buf = [0u8; 8192];
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    while let Ok(n) = or.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        if !q.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = iw.write_all(&buf[..n]).await;
                        }
                    }
                };
                tokio::join!(up, down);
            });
        }
    });
    let options = ClientOptions {
        probe: Duration::from_millis(500),
        ..quick()
    };
    let client = Client::connect_with(
        vec![route],
        &m.identity.fingerprint,
        m.pair_auth("machine-a"),
        options,
    )
    .unwrap();
    let mut events = client.events().unwrap();
    connected(&mut events).await;
    quiet.store(true, std::sync::atomic::Ordering::SeqCst);
    client.network_changed();
    let reason = next(&mut events, |e| match e {
        Event::Disconnected { reason } => Some(reason),
        _ => None,
    })
    .await;
    assert!(reason.contains("network changed"), "{reason}");
}

#[tokio::test]
async fn a_suggested_route_is_dialled_and_kept_once_it_answers_as_the_machine() {
    let m = machine().await;
    let (_m, paired) = {
        let (client, _events, paired) = paired_client(&m, "machine-a").await;
        client.close();
        (client, paired)
    };
    // Only an address it had on another network (nothing answers there now)...
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let mut away = paired.clone();
    away.routes = vec![dead.clone()];
    let client =
        Client::connect_with(away.routes.clone(), &away.fingerprint, away.auth(), quick()).unwrap();
    let mut events = client.events().unwrap();

    // ...and a guess that is another Trek: another certificate there doesn't stop it.
    let impostor = machine().await;
    client.also_try(vec![impostor.handle.local_addr().to_string()]);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(*client.state().borrow(), State::Connecting);

    // The right guess (its tailnet address, say) answers with the pinned certificate.
    let real = m.handle.local_addr().to_string();
    client.also_try(vec![real.clone()]);
    client.network_changed();
    let (kept, newly) = connected(&mut events).await;
    assert!(!newly);
    assert_eq!(kept.routes.first(), Some(&real), "{:?}", kept.routes);
    assert!(
        kept.routes.contains(&dead),
        "the address it had is kept too: {:?}",
        kept.routes
    );
}
