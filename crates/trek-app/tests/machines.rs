//! Machines (`docs/MULTI-MACHINE.md`): pairing with another Trek from its link, `machines.json`,
//! the composer's "On this Mac ▾", a thread started on the other machine and followed here (sent
//! to, stopped, its cards answered, its model changed), "Add to another machine…" (clone, copy
//! the ignored files, start the thread there), and nothing of the other machine's threads in the
//! local store.
//!
//! "Machine B" is a real phone server over TLS on the loopback. In most tests it is this very
//! workspace answering (Trek paired with itself, as the verify driver does it); the clone test
//! scripts it, to say exactly what B was asked.

mod common;

use common::{REAL_TIME_LIMIT, Trek, new_project, open, run};
use gpui_kit::TestAppContext;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use trek_app::machines::{self, CloneStage, PairLink, Pairing, is_remote, local_id};
use trek_app::workspace::{Route, Scope, SettingsPage};
use trek_core::store::Item;
use trek_core::{Effort, RunState};
use trek_remote as tr;

/// A phone server over TLS on the loopback, answered by `trek`'s own workspace as Trek answers
/// phones; returns its pairing link (as Settings shows it) parsed.
fn serve_as_machine_b(trek: &Trek, cx: &mut TestAppContext) -> PairLink {
    let rt = trek_core::runtime();
    let identity =
        tr::TlsIdentity::load_or_create(&new_project("machine-b-tls"), "Machine B").unwrap();
    let fingerprint = identity.fingerprint.clone();
    let mut config = tr::ServerConfig::new(tr::HostInfo {
        id: "machine-b".into(),
        name: "Machine B".into(),
        version: "0".into(),
    });
    config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    config.tls = Some(identity);
    config.features = tr::feature::ALL.iter().map(|f| f.to_string()).collect();
    let (host, requests) = tr::ChannelHost::new();
    let handle = rt
        .block_on(tr::RemoteServer::start(config, Arc::new(host)))
        .unwrap();
    let route = handle.local_addr().to_string();
    let offer = handle.pairing_offer();
    let mut notices = handle.notices();
    let tasks = trek.update(cx, |_, cx| {
        let serve = cx.spawn(async move |this, cx| {
            while let Ok(req) = requests.recv().await {
                if this
                    .update(cx, |ws, cx| ws.remote_request(req, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let push = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                if this
                    .update(cx, |ws, cx| ws.push_remote_changes(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let (tx, rx) = async_channel::unbounded();
        trek_core::runtime().spawn(async move {
            loop {
                match notices.recv().await {
                    Ok(n) => {
                        if tx.send(n).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
        let hear = cx.spawn(async move |this, cx| {
            while let Ok(n) = rx.recv().await {
                if this.update(cx, |ws, cx| ws.remote_notice(n, cx)).is_err() {
                    break;
                }
            }
        });
        vec![serve, push, hear]
    });
    // On, with the port and reach it "started" with: a settings save leaves it running.
    let started_with = trek.update(cx, |ws, _| {
        ws.settings.mobile.enabled = true;
        (ws.settings.mobile.port, ws.settings.mobile.reach)
    });
    let remote = trek_app::remote::Remote::new(
        handle,
        Default::default(),
        route.clone(),
        vec![route.clone()],
        started_with,
        None,
        tasks,
    );
    trek.update(cx, |ws, _| ws.remote = Some(remote));
    // The link as Settings › Phone & Machines shows it under "Pair another machine".
    let url = tr::pairing::pairing_url(
        &route,
        &offer.code,
        "Machine B",
        "machine-b",
        Some(&fingerprint),
    );
    PairLink::parse(&url).expect("the server's own link")
}

/// Pair `trek` with machine B and wait for B's snapshot.
async fn pair(trek: &Trek, cx: &mut TestAppContext, link: PairLink) {
    trek.update(cx, |ws, cx| ws.pair_machine(link, cx));
    trek.wait_explained(
        cx,
        "pairing",
        REAL_TIME_LIMIT,
        |ws| matches!(ws.machines.pairing, Some(Pairing::Done(_))),
        |ws| format!("{:?}", ws.machines.pairing),
    )
    .await;
    trek.wait(cx, "machine B's snapshot", |ws| {
        ws.machines
            .get("machine-b")
            .is_some_and(|m| m.reachable() && !m.projects.is_empty())
    })
    .await;
}

/// Start a thread on machine B from the main window's draft in `trek.project`; returns its id
/// here and B's own.
async fn start_on_b(trek: &Trek, cx: &mut TestAppContext, text: &str) -> (String, String) {
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| {
        ws.navigate(
            Route::Draft {
                project: Some(project.clone()),
            },
            cx,
        );
        ws.set_draft_machine(&project, Some("machine-b".into()), cx);
    });
    trek.render(cx);
    // What went wrong, if it doesn't open, is said in a toast.
    let toasts = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = toasts.clone();
    let _sub = trek.update(cx, |_, cx| {
        cx.subscribe_self(move |_, e: &trek_app::workspace::WorkspaceEvent, _| {
            if let trek_app::workspace::WorkspaceEvent::Toast { message, .. } = e {
                seen.lock().unwrap().push(message.clone());
            }
        })
    });
    trek.type_text(cx, text);
    trek.press(cx, "enter");
    trek.wait_explained(
        cx,
        "the thread on B to open",
        REAL_TIME_LIMIT,
        |ws| matches!(&ws.route, Route::Thread(id) if is_remote(id)),
        |ws| format!("route {:?}, toasts {:?}", ws.route, toasts.lock().unwrap()),
    )
    .await;
    let id = trek.thread_id(cx);
    let (_, theirs) = machines::split_id(&id).unwrap();
    let theirs = theirs.to_string();
    (id, theirs)
}

/// Settings › Phone & Machines, scrolled down to its Machines.
fn machines_page(trek: &Trek, cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    trek.update(cx, |ws, cx| {
        ws.navigate(Route::Settings(SettingsPage::Mobile), cx)
    });
    trek.render(cx);
    trek.window(cx, |window, cx| {
        window.scroll(
            "settings-content",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-5000.))),
            cx,
        )
    });
    trek.render(cx);
}

fn said(items: &[Item], text: &str) -> bool {
    items
        .iter()
        .any(|i| matches!(i, Item::User { text: t, .. } if t == text))
}

#[test]
fn pairing_links_and_typed_addresses_read_as_the_client_dials_them() {
    let fp = "AB".repeat(32);
    let link = PairLink::parse(&format!("  trek://pair?host=192.168.1.20:7420&code=K7Q2-9XMV&name=Tobias%E2%80%99s%20Studio&hid=7f3c&alt=100.101.1.2:7420,[fd7a::1]:7420&fp={fp}\n")).unwrap();
    assert_eq!(
        link.routes,
        ["192.168.1.20:7420", "100.101.1.2:7420", "[fd7a::1]:7420"]
    );
    assert_eq!(
        (
            link.code.as_str(),
            link.name.as_deref(),
            link.host_id.as_deref()
        ),
        ("K7Q2-9XMV", Some("Tobias’s Studio"), Some("7f3c"))
    );
    assert_eq!(
        link.fingerprint,
        "ab".repeat(32),
        "pinned as the client takes it"
    );
    assert!(
        PairLink::parse("https://example.com")
            .unwrap_err()
            .contains("trek://pair")
    );
    assert!(
        PairLink::parse("trek://pair?host=h&code=c")
            .unwrap_err()
            .contains("fingerprint"),
        "no TLS, no pin: refused"
    );
    // Typed: the port is the phone server's unless given; the fingerprint whole, however spaced.
    let typed = PairLink::typed("studio.local", " ABCD-1234 ", &"ab12 cd34 ".repeat(8)).unwrap();
    assert_eq!(
        (typed.routes, typed.code.as_str()),
        (vec!["studio.local:7420".to_string()], "ABCD-1234")
    );
    assert_eq!(
        PairLink::typed("10.0.0.2:9000", "c", &"a".repeat(64))
            .unwrap()
            .routes,
        ["10.0.0.2:9000"]
    );
    assert!(
        PairLink::typed("h", "c", "ABCD-1234-EF56-7890")
            .unwrap_err()
            .contains("short fingerprint")
    );
}

#[test]
fn machines_json_keeps_what_reconnecting_needs_and_survives_a_bad_entry() {
    run(async |cx| {
        let _trek = open(cx);
        let entry = |id: &str| machines::MachineEntry {
            paired: trek_remote::client::Paired {
                device_id: "this-mac".into(),
                token: format!("token-{id}"),
                fingerprint: "ab".repeat(32),
                host: tr::HostInfo {
                    id: id.into(),
                    name: format!("Machine {id}"),
                    version: "0.4.7".into(),
                },
                routes: vec!["10.0.0.2:7420".into()],
                features: vec![tr::feature::MACHINES.into()],
            },
            name: format!("Machine {id}"),
            last_seen: Some(5),
            projects: [("/Users/me/trek".to_string(), "p1".to_string())].into(),
        };
        machines::save("this-mac", &[entry("a"), entry("b")]).unwrap();
        let (device, read) = machines::load();
        assert_eq!(device, "this-mac");
        assert_eq!(read, vec![entry("a"), entry("b")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(machines::machines_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "tokens are the owner's alone");
        }
        // One entry another version wrote badly doesn't lose the other.
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(machines::machines_file()).unwrap())
                .unwrap();
        value["machines"][0]["paired"] = serde_json::json!("not a pairing");
        std::fs::write(machines::machines_file(), value.to_string()).unwrap();
        let (device, read) = machines::load();
        assert_eq!((device.as_str(), read), ("this-mac", vec![entry("b")]));
    });
}

#[test]
fn a_machines_items_become_the_thread_views_and_requests_its_cards() {
    let item = |id: &str, body: tr::ItemBody| tr::Item {
        id: id.into(),
        seq: 1,
        at: Some(10),
        body,
    };
    let items = vec![
        item(
            "i0",
            tr::ItemBody::User {
                text: "fix it".into(),
                images: 1,
                aside: false,
            },
        ),
        item(
            "i1",
            tr::ItemBody::Tool {
                call_id: "call-1".into(),
                tool: tr::ToolKind::Command,
                title: "Run command".into(),
                detail: "cargo test".into(),
                status: tr::ToolStatus::Done,
                output: "ok".into(),
                added: None,
                removed: None,
            },
        ),
        item(
            "i2",
            tr::ItemBody::Assistant {
                text: "Working on".into(),
                streaming: true,
            },
        ),
        item(
            "c2",
            tr::ItemBody::Changes {
                files: vec![],
                added: 0,
                removed: 0,
            },
        ),
        item(
            "r1",
            tr::ItemBody::Approval {
                request_id: "req-1".into(),
                title: "Run rm".into(),
                detail: "rm -rf target".into(),
                state: tr::ApprovalState::Pending,
            },
        ),
        item(
            "r0",
            tr::ItemBody::Approval {
                request_id: "req-0".into(),
                title: "old".into(),
                detail: String::new(),
                state: tr::ApprovalState::Allowed,
            },
        ),
        item(
            "r2",
            tr::ItemBody::Plan {
                request_id: "req-2".into(),
                markdown: "1. do".into(),
                state: tr::PlanState::Pending,
            },
        ),
    ];
    let mapped = machines::map_items(&items);
    let ids: Vec<&str> = mapped.rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        ["i0", "i1", "i2"],
        "the turn's files and the cards aren't transcript rows"
    );
    assert!(matches!(&mapped.rows[0].1, Item::User { text, .. } if text == "fix it\n\n(1 image)"));
    assert!(
        matches!(&mapped.rows[1].1, Item::Tool { id, status: trek_core::store::ToolStatus::Done, .. } if id == "call-1")
    );
    assert_eq!(mapped.streaming, Some(2));
    let requests: Vec<(&str, bool)> = mapped
        .requests
        .iter()
        .map(|p| (p.request_id.as_str(), p.prompt.is_some()))
        .collect();
    assert_eq!(
        requests,
        [("req-1", false), ("req-2", true)],
        "only what's still waiting, an approval and a plan"
    );
}

#[test]
fn a_thread_started_on_machine_b_runs_there_and_is_followed_here_without_touching_the_store() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (_, saved) = machines::load();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (saved[0].name.as_str(), saved[0].paired.host.id.as_str()),
            ("Machine B", "machine-b")
        );
        assert!(!saved[0].paired.token.is_empty() && saved[0].last_seen.is_some());

        // "On this Mac ▾" says where the draft's thread goes; its menu lists B.
        trek.render(cx);
        assert!(trek.visible(cx, "machine-chip"));
        trek.click(cx, "machine-chip");
        trek.render(cx);
        assert!(trek.visible(cx, "machine-pick-here") && trek.visible(cx, "machine-add-to"));
        trek.click(cx, "machine-pick-machine-b");
        assert_eq!(
            trek.read(cx, |ws, _| ws.draft_machine(&trek.project))
                .as_deref(),
            Some("machine-b")
        );
        let (id, theirs) = start_on_b(&trek, cx, "hello from A").await;
        trek.wait(cx, "B's answer here", |ws| {
            ws.thread(&id)
                .is_some_and(|t| t.run_state == RunState::Idle)
                && ws
                    .live
                    .get(&id)
                    .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Assistant { .. })))
        })
        .await;
        assert!(said(&trek.items(cx, &id), "hello from A"));
        // B ran it as a thread of its own (this same workspace, here).
        assert!(
            said(&trek.items(cx, &theirs), "hello from A"),
            "B's own thread has the message"
        );
        trek.render(cx);
        assert!(
            trek.visible(cx, "machine-head-machine-b"),
            "the sidebar lists it under B's name"
        );
        assert!(
            trek.visible(cx, "env-machine"),
            "its composer says where it runs"
        );
        assert!(!trek.rows(cx).is_empty(), "the normal thread view draws it");

        // A follow-up from this composer goes to B.
        trek.type_text(cx, "and again");
        trek.press(cx, "enter");
        trek.wait(cx, "the follow-up's answer", |ws| {
            ws.live.get(&id).is_some_and(|l| {
                l.items
                    .iter()
                    .filter(|i| matches!(i, Item::Assistant { .. }))
                    .count()
                    >= 2
            }) && ws
                .thread(&id)
                .is_some_and(|t| t.run_state == RunState::Idle)
        })
        .await;
        assert!(said(&trek.items(cx, &theirs), "and again"));

        // Effort changed here changes it there.
        trek.update(cx, |ws, cx| {
            let mut prefs = ws.prefs_in(&Scope::Main);
            prefs.effort = Effort::High;
            ws.set_prefs_in(&Scope::Main, prefs, cx);
        });
        trek.wait(cx, "B's thread at High", |ws| {
            ws.threads
                .iter()
                .any(|t| t.id == theirs && t.effort == Effort::High)
        })
        .await;

        // None of it is in this Mac's store: no thread row, no transcript row.
        let store = trek.read(cx, |ws, _| ws.store.clone());
        assert!(store.threads().unwrap().iter().all(|t| !is_remote(&t.id)));
        assert!(store.items_with_ids(&id).unwrap().is_empty());
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        assert!(
            store.items_with_ids(&id).unwrap().is_empty(),
            "leaving it saves nothing either"
        );
    });
}

#[test]
fn approvals_plans_and_stop_on_machine_b_are_answered_from_here() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;

        // An approval: its card is drawn here, and Allow reaches B's agent.
        let (id, theirs) = start_on_b(&trek, cx, "ask permission first").await;
        trek.wait(cx, "the approval card here", |ws| {
            ws.live.get(&id).is_some_and(|l| !l.permissions.is_empty())
        })
        .await;
        trek.render(cx);
        assert!(trek.visible(cx, "allow"));
        trek.click(cx, "allow");
        trek.wait(cx, "B's turn to finish", |ws| {
            ws.threads
                .iter()
                .any(|t| t.id == theirs && t.run_state == RunState::Idle)
                && ws
                    .live
                    .get(&theirs)
                    .is_some_and(|l| l.turn_started.is_none())
        })
        .await;
        trek.wait(cx, "the card gone and the thread idle here", |ws| {
            ws.live.get(&id).is_some_and(|l| l.permissions.is_empty())
                && ws
                    .thread(&id)
                    .is_some_and(|t| t.run_state == RunState::Idle)
        })
        .await;

        // A plan, approved from here.
        let (id, theirs) = start_on_b(&trek, cx, "make a plan").await;
        trek.wait_explained(
            cx,
            "the plan card here",
            REAL_TIME_LIMIT,
            |ws| {
                ws.live.get(&id).is_some_and(|l| {
                    l.permissions
                        .iter()
                        .any(|p| matches!(p.prompt, Some(trek_agents::Prompt::Plan(_))))
                })
            },
            |ws| {
                let here = ws.live.get(&id).map(|l| {
                    (
                        l.items.to_vec(),
                        l.permissions
                            .iter()
                            .map(|p| p.request_id.clone())
                            .collect::<Vec<_>>(),
                    )
                });
                let there = ws.live.get(&theirs).map(|l| {
                    (
                        l.items.len(),
                        l.permissions
                            .iter()
                            .map(|p| (p.request_id.clone(), p.prompt.is_some()))
                            .collect::<Vec<_>>(),
                    )
                });
                format!(
                    "here {here:?}; there {there:?}; row {:?}",
                    ws.thread(&id).map(|t| t.run_state)
                )
            },
        )
        .await;
        let request = trek.request(cx, &id);
        trek.update(cx, |ws, cx| ws.approve_plan(&id, &request, cx));
        trek.wait(cx, "B to take the plan", |ws| {
            ws.live
                .get(&theirs)
                .is_some_and(|l| l.permissions.is_empty() && l.turn_started.is_none())
        })
        .await;

        // Stop, from this composer's button.
        let (id, theirs) = start_on_b(&trek, cx, "mock:long 600s").await;
        trek.wait(cx, "B's long turn here", |ws| {
            ws.thread(&id)
                .is_some_and(|t| t.run_state == RunState::Working)
        })
        .await;
        trek.render(cx);
        trek.click(cx, "stop");
        trek.wait(cx, "B's turn stopped", |ws| {
            ws.live
                .get(&theirs)
                .is_some_and(|l| l.turn_started.is_none())
        })
        .await;
        trek.wait(cx, "and stopped here", |ws| {
            ws.thread(&id)
                .is_some_and(|t| t.run_state != RunState::Working)
        })
        .await;
    });
}

#[test]
fn settings_lists_the_machine_with_its_dot_and_forgetting_it_clears_everything() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (id, _) = start_on_b(&trek, cx, "hello").await;
        machines_page(&trek, cx);
        assert!(trek.visible(cx, "machine-row-machine-b"));
        assert!(trek.visible(cx, "machine-dot-machine-b"));
        assert!(trek.visible(cx, "machine-add"));

        trek.click(cx, "machine-forget-machine-b");
        trek.render(cx);
        assert!(!trek.visible(cx, "machine-row-machine-b"));
        assert!(trek.read(cx, |ws, _| ws.machines.list.is_empty()
            && !ws.live.contains_key(&id)
            && ws.thread(&id).is_none()));
        assert!(machines::load().1.is_empty(), "gone from machines.json");
        assert_eq!(
            trek.read(cx, |ws, _| ws.draft_machine(&trek.project)),
            None,
            "the project starts threads here again"
        );
    });
}

#[test]
fn add_a_machine_pairs_from_the_link_pasted_in_its_sheet() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        machines_page(&trek, cx);
        trek.click(cx, "machine-add");
        trek.render(cx);
        assert!(trek.visible(cx, "pair-sheet"));
        // Nothing pasted: it says what's wrong and dials nothing.
        trek.click(cx, "pair-submit");
        trek.render(cx);
        assert!(trek.visible(cx, "pair-error"));
        // The link field has the keys as the sheet opens.
        let url = tr::pairing::pairing_url(
            &link.routes[0],
            &link.code,
            "Machine B",
            "machine-b",
            Some(&link.fingerprint),
        );
        trek.type_text(cx, &url);
        trek.click(cx, "pair-submit");
        trek.wait(cx, "pairing from the sheet", |ws| {
            ws.machines.get("machine-b").is_some_and(|m| m.reachable())
        })
        .await;
        machines_page(&trek, cx);
        assert!(
            !trek.visible(cx, "pair-sheet"),
            "the sheet goes once paired"
        );
        assert!(trek.visible(cx, "machine-row-machine-b"));
    });
}

/// What the scripted machine was asked.
#[derive(Default)]
struct Asked {
    clone: Option<tr::CloneProjectRequest>,
    files: Vec<tr::FileUpload>,
    new_thread: Option<tr::NewThreadRequest>,
}

/// A phone server whose host is a script: a home folder, a clone that says one line and makes
/// project `p-1`, files taken, a thread `t-1` started and its transcript.
fn scripted_machine(asked: Arc<Mutex<Asked>>) -> PairLink {
    let rt = trek_core::runtime();
    let identity =
        tr::TlsIdentity::load_or_create(&new_project("machine-c-tls"), "Machine C").unwrap();
    let fingerprint = identity.fingerprint.clone();
    let mut config = tr::ServerConfig::new(tr::HostInfo {
        id: "machine-c".into(),
        name: "Machine C".into(),
        version: "0".into(),
    });
    config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    config.tls = Some(identity);
    config.features = tr::feature::ALL.iter().map(|f| f.to_string()).collect();
    let (host, requests) = tr::ChannelHost::new();
    let handle = rt
        .block_on(tr::RemoteServer::start(config, Arc::new(host)))
        .unwrap();
    let route = handle.local_addr().to_string();
    let code = handle.pairing_offer().code;
    rt.spawn(async move {
        let _handle = handle;
        let agent = tr::AgentOption {
            key: common::mock().key(),
            name: "Mock".into(),
            logo: None,
            default_model: None,
            models: vec![],
            icon: None,
        };
        while let Ok(req) = requests.recv().await {
            match req {
                tr::HostRequest::Snapshot { reply } => {
                    let _ = reply.send(Ok(tr::Snapshot {
                        agents: vec![agent.clone()],
                        ..Default::default()
                    }));
                }
                tr::HostRequest::MachineInfo { reply } => {
                    let _ = reply.send(Ok(tr::MachineInfo {
                        name: "Machine C".into(),
                        platform: "macos".into(),
                        trek_version: "0".into(),
                        home: "/Users/c".into(),
                    }));
                }
                tr::HostRequest::CloneProject {
                    req,
                    progress,
                    reply,
                } => {
                    progress.line("Cloning into 'trek'...");
                    asked.lock().unwrap().clone = Some(req);
                    let _ = reply.send(Ok("p-1".into()));
                }
                tr::HostRequest::PutFiles {
                    project_id,
                    files,
                    reply,
                } => {
                    assert_eq!(project_id, "p-1");
                    asked.lock().unwrap().files.extend(files);
                    let _ = reply.send(Ok(()));
                }
                tr::HostRequest::NewThread { req, reply } => {
                    asked.lock().unwrap().new_thread = Some(req);
                    let _ = reply.send(Ok("t-1".into()));
                }
                tr::HostRequest::Transcript { reply, .. } => {
                    let item = tr::Item {
                        id: "i0".into(),
                        seq: 1,
                        at: None,
                        body: tr::ItemBody::User {
                            text: "build it there".into(),
                            images: 0,
                            aside: false,
                        },
                    };
                    let _ = reply.send(Ok(tr::Transcript {
                        seq: 1,
                        items: vec![item],
                        base: 0,
                        more: false,
                    }));
                }
                tr::HostRequest::MarkSeen { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
                _ => {}
            }
        }
    });
    PairLink {
        routes: vec![route],
        code,
        fingerprint,
        name: None,
        host_id: None,
    }
}

/// A git project with an origin over SSH, and files git ignores: small ones, a big one, and a
/// folder ignored whole.
fn ignoring_project() -> std::path::PathBuf {
    let dir = new_project("trek");
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "git {args:?}"
        )
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "git@github.com:dokyit/trek.git"]);
    std::fs::write(
        dir.join(".gitignore"),
        ".env\n*.log\n*.dat\nnode_modules/\n",
    )
    .unwrap();
    std::fs::write(dir.join(".env"), "KEY=1\n").unwrap();
    std::fs::write(dir.join("debug.log"), "log\n").unwrap();
    std::fs::write(dir.join("huge.dat"), vec![0u8; 2 << 20]).unwrap();
    std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
    std::fs::write(dir.join("node_modules/x/index.js"), "x").unwrap();
    std::fs::write(dir.join("notes.txt"), "not ignored").unwrap();
    dir
}

#[test]
fn ignored_files_leave_wholly_ignored_folders_behind_and_tick_the_small_ones() {
    use trek_app::machines::clone::{Scheme, ignored_files, repo_name, rewrite_url};
    let dir = ignoring_project();
    let files = ignored_files(&dir).unwrap();
    let listed: Vec<(&str, bool)> = files.iter().map(|f| (f.path.as_str(), f.ticked)).collect();
    assert_eq!(
        listed,
        [(".env", true), ("debug.log", true), ("huge.dat", false)],
        "node_modules stays behind; 2 MiB isn't ticked"
    );
    assert_eq!(files[2].size, 2 << 20);

    let ssh = "git@github.com:dokyit/trek.git";
    assert_eq!(
        rewrite_url(ssh, Scheme::Https),
        "https://github.com/dokyit/trek.git"
    );
    assert_eq!(rewrite_url(ssh, Scheme::Ssh), ssh);
    assert_eq!(
        rewrite_url("https://github.com/dokyit/trek.git", Scheme::Ssh),
        ssh
    );
    assert_eq!(
        rewrite_url("ssh://git@github.com:22/dokyit/trek", Scheme::Https),
        "https://github.com/dokyit/trek"
    );
    assert_eq!(
        rewrite_url("https://token@github.com/dokyit/trek.git", Scheme::Https),
        "https://token@github.com/dokyit/trek.git",
        "the scheme it has: as written"
    );
    assert_eq!(repo_name(ssh).as_deref(), Some("trek"));
}

#[test]
fn add_to_another_machine_clones_copies_the_ticked_files_and_starts_the_thread_there() {
    run(async |cx| {
        let project = ignoring_project();
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.add_project(project.clone(), cx);
            ws.navigate(
                Route::Draft {
                    project: Some(project.clone()),
                },
                cx,
            );
        });
        let asked = Arc::new(Mutex::new(Asked::default()));
        let link = scripted_machine(asked.clone());
        trek.update(cx, |ws, cx| ws.pair_machine(link, cx));
        trek.wait(cx, "machine C's home folder", |ws| {
            ws.machine_home("machine-c").is_some()
        })
        .await;

        // What's typed in the draft goes there once it's cloned.
        trek.render(cx);
        trek.type_text(cx, "build it there");
        let ws = trek.ws.clone();
        trek.window(cx, |window, cx| {
            trek_app::machines::sheets::open_clone(ws, project.clone(), window, cx)
        });
        for _ in 0..200 {
            trek.render(cx);
            if trek.visible(cx, ("clone-file", 2usize)) {
                break;
            }
            cx.background_executor
                .timer(Duration::from_millis(10))
                .await;
        }
        assert!(trek.visible(cx, "clone-sheet"));
        assert!(trek.visible(cx, "clone-url"));
        // Over HTTPS rather than the origin's SSH; debug.log stays behind.
        trek.click(cx, ("clone-scheme", 1usize));
        trek.click(cx, ("clone-file", 1usize));
        trek.render(cx);
        trek.click(cx, "clone-submit");
        trek.wait_explained(
            cx,
            "the clone, the files and the thread",
            REAL_TIME_LIMIT,
            |ws| matches!(&ws.route, Route::Thread(id) if id == &local_id("machine-c", "t-1")),
            |ws| format!("{:?}", ws.machines.clone),
        )
        .await;
        let run = trek.read(cx, |ws, _| ws.machines.clone.clone()).unwrap();
        assert_eq!(
            run.stage,
            CloneStage::Done {
                project_id: "p-1".into()
            }
        );
        assert_eq!(
            run.lines,
            ["Cloning into 'trek'..."],
            "git's progress, as B said it"
        );
        {
            let asked = asked.lock().unwrap();
            let clone = asked.clone.clone().unwrap();
            assert_eq!(clone.url, "https://github.com/dokyit/trek.git");
            assert_eq!(
                clone.folder, "/Users/c/trek",
                "under its home, by the repo's name"
            );
            let files: Vec<(&str, &[u8])> = asked
                .files
                .iter()
                .map(|f| (f.path.as_str(), f.bytes.as_slice()))
                .collect();
            assert_eq!(
                files,
                [(".env", b"KEY=1\n".as_slice())],
                "only what stayed ticked"
            );
            let started = asked.new_thread.clone().unwrap();
            assert_eq!(
                (started.project_id.as_str(), started.text.as_str()),
                ("p-1", "build it there")
            );
        }
        assert_eq!(
            trek.read(cx, |ws, _| ws.draft_machine(&project)).as_deref(),
            Some("machine-c"),
            "the chip now says machine C"
        );
        let (_, saved) = machines::load();
        assert_eq!(
            saved[0]
                .projects
                .get(&project.display().to_string())
                .map(String::as_str),
            Some("p-1")
        );
        trek.wait(cx, "its transcript", |ws| {
            ws.live
                .get(&local_id("machine-c", "t-1"))
                .is_some_and(|l| l.loaded && !l.items.is_empty())
        })
        .await;
        trek.render(cx);
        assert!(
            !trek.visible(cx, "clone-sheet"),
            "the sheet goes once it's done"
        );
    });
}

#[test]
fn a_machine_known_only_on_its_local_network_says_to_put_tailscale_on_both() {
    use trek_app::remote::{Addresses, Tailnet, TailnetPeer};
    let lan = vec!["192.168.1.186:7420".to_string()];
    assert!(!machines::tailnet_route(&lan[0]));
    assert!(
        machines::tailnet_route("100.77.1.2:7420")
            && machines::tailnet_route("[fd7a:115c:a1e0::4]:7420")
            && machines::tailnet_route("t-book.tail1234.ts.net:7420")
    );
    assert!(
        !machines::tailnet_route("100.20.1.2:7420"),
        "not in 100.64/10"
    );

    // Tailscale on neither.
    let nowhere = Addresses::default();
    assert_eq!(
        machines::reach_advice("T-Book", &lan, &nowhere, "this Mac").as_deref(),
        Some(
            "T-Book is only reachable on its local network. Install Tailscale on it and on this Mac, signed in to the same account, to reach it from anywhere."
        )
    );
    assert!(machines::tailnet_routes("T-Book", &lan, &nowhere).is_empty());

    // On this Mac, signed in as dokyit@; T-Book isn't in the tailnet.
    let mut here = Addresses {
        lan: Some("10.220.61.2".parse().unwrap()),
        tailscale: Some("100.120.68.5".parse().unwrap()),
        tailnet: Some(Tailnet {
            account: Some("dokyit@".into()),
            ..Default::default()
        }),
    };
    let advice = machines::reach_advice("T-Book", &lan, &here, "this Mac").unwrap();
    assert!(
        advice.contains("Install Tailscale on it, signed in as dokyit@ like this Mac"),
        "{advice}"
    );

    // T-Book joins it: its tailnet address is what the client also tries, on its port, IPv4
    // first; while it's online there there's nothing more to say.
    here.tailnet.as_mut().unwrap().peers = vec![TailnetPeer {
        host_name: "T-BOOK".into(),
        dns_name: "t-book.tail1234.ts.net.".into(),
        ips: vec![
            "fd7a:115c:a1e0::4".parse().unwrap(),
            "100.77.1.2".parse().unwrap(),
        ],
        online: true,
    }];
    assert_eq!(
        machines::tailnet_routes("T-Book", &lan, &here),
        vec![
            "100.77.1.2:7420".to_string(),
            "[fd7a:115c:a1e0::4]:7420".to_string()
        ]
    );
    assert_eq!(
        machines::reach_advice("T-Book", &lan, &here, "this Mac"),
        None
    );
    here.tailnet.as_mut().unwrap().peers[0].online = false;
    assert!(
        machines::reach_advice("T-Book", &lan, &here, "this Mac")
            .unwrap()
            .contains("offline in your tailnet")
    );
    assert!(
        machines::tailnet_routes("Studio", &lan, &here).is_empty(),
        "another name is another machine"
    );

    // Its tailnet address known (it said so while it was reachable), Tailscale off here.
    let advice = machines::reach_advice(
        "T-Book",
        &["100.77.1.2:7420".to_string()],
        &nowhere,
        "this Mac",
    )
    .unwrap();
    assert!(
        advice.contains("Tailscale isn't on on this Mac"),
        "{advice}"
    );
}

#[test]
fn a_machine_that_stops_answering_holds_its_threads_still_and_says_why() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (id, _) = start_on_b(&trek, cx, "mock:long 600s").await;
        trek.wait(cx, "B's long turn here, its clock running", |ws| {
            ws.thread(&id)
                .is_some_and(|t| t.run_state == RunState::Working)
                && ws.live.get(&id).is_some_and(|l| l.turn_started.is_some())
        })
        .await;
        trek.render(cx);
        let working = format!("card-working-{id}");
        assert!(trek.visible(cx, working.clone()));
        assert!(!trek.visible(cx, "machine-offline-machine-b"));

        // B goes away (it quit, or the network between went).
        trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().handle.shutdown());
        trek.wait(cx, "B offline here", |ws| {
            ws.machines
                .get("machine-b")
                .is_some_and(|m| !m.reachable() && m.offline_since.is_some())
        })
        .await;
        let (state, clock) = trek.read(cx, |ws, _| {
            (
                ws.thread(&id).map(|t| t.run_state),
                ws.live.get(&id).and_then(|l| l.turn_started),
            )
        });
        assert_eq!(
            state,
            Some(RunState::Idle),
            "not running, as far as here knows"
        );
        assert!(clock.is_none(), "no turn clock counting");
        trek.render(cx);
        assert!(
            !trek.visible(cx, working.clone()),
            "no spinner, no working phrase, no timer"
        );
        assert!(
            trek.visible(cx, "machine-head-machine-b"),
            "its threads are still listed, as they last were"
        );
        assert!(trek.visible(cx, "machine-offline-machine-b"));
        let note = trek
            .read(cx, |ws, _| {
                machines::offline_note(
                    ws.machines.get("machine-b").unwrap(),
                    &ws.machines.here,
                    "this Mac",
                )
            })
            .unwrap();
        assert!(note.starts_with("Offline since "), "{note}");
        assert!(
            note.contains("Install Tailscale on it and on this Mac"),
            "only a loopback address is known for it: {note}"
        );
        machines_page(&trek, cx);
        assert!(trek.visible(cx, "machine-row-machine-b"));

        // Back: live again, as its rows say.
        let paired = trek.read(cx, |ws, _| {
            ws.machines.get("machine-b").unwrap().entry.paired.clone()
        });
        trek.update(cx, |ws, cx| {
            ws.machine_event(
                "machine-b",
                tr::client::Event::Connected {
                    route: paired.routes[0].clone(),
                    paired,
                    newly_paired: false,
                },
                cx,
            )
        });
        let (state, clock, offline) = trek.read(cx, |ws, _| {
            let m = ws.machines.get("machine-b").unwrap();
            (
                ws.thread(&id).map(|t| t.run_state),
                ws.live.get(&id).and_then(|l| l.turn_started),
                machines::offline_note(m, &ws.machines.here, "this Mac"),
            )
        });
        assert_eq!(state, Some(RunState::Working));
        assert!(clock.is_some());
        assert_eq!(offline, None);
    });
}
