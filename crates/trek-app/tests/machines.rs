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
        ws.set_draft_machine(Some(&project), Some("machine-b".into()), cx);
    });
    trek.render(cx);
    // What went wrong, if it doesn't open, is said in a toast.
    let toasts = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = toasts.clone();
    let _sub = trek.update(cx, |_, cx| {
        cx.subscribe_self(move |_, e: &trek_app::workspace::WorkspaceEvent, _| {
            if let trek_app::workspace::WorkspaceEvent::Toast { message, .. }
            | trek_app::workspace::WorkspaceEvent::Failure { message } = e
            {
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

#[test]
fn a_new_thread_in_no_project_starts_on_a_paired_machine() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        // The draft with no project offers the machine too.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "machine-chip"), "machine chip");
        trek.click(cx, "machine-chip");
        trek.render(cx);
        assert!(
            trek.visible(cx, "machine-pick-machine-b") && !trek.visible(cx, "machine-add-to"),
            "no project: nothing to clone"
        );
        trek.click(cx, "machine-pick-machine-b");
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.draft_machine(None)).as_deref(),
            Some("machine-b")
        );
        trek.type_text(cx, "hello from no project");
        trek.press(cx, "enter");
        trek.wait(
            cx,
            "the thread on B to open",
            |ws| matches!(&ws.route, Route::Thread(id) if is_remote(id)),
        )
        .await;
        let id = trek.thread_id(cx);
        let (_, theirs) = machines::split_id(&id).unwrap();
        let theirs = theirs.to_string();
        trek.wait(cx, "B's thread to have the message", |ws| {
            ws.live
                .get(&theirs)
                .is_some_and(|l| said(&l.items, "hello from no project"))
        })
        .await;
        // B started it in no project of its own.
        assert!(trek.read(cx, |ws, _| {
            ws.thread(&theirs).is_some_and(|t| t.project_id.is_none())
        }));
    });
}

/// A folder of this computer's files, for what a test attaches (out of every data folder).
fn attachments_dir(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("trek-machine-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Bytes that look like a QuickTime movie (`ftypqt`) and are `len` long, none alike for long.
fn movie_bytes(len: usize) -> Vec<u8> {
    let mut bytes = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ".to_vec();
    bytes.extend((bytes.len()..len).map(|i| (i * 7919 % 251) as u8));
    bytes
}

/// A small real PNG.
fn png_at(path: &std::path::Path) {
    image::RgbaImage::from_pixel(6, 4, image::Rgba([20, 120, 220, 255]))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

/// Where machine B keeps what it's sent: its data folder's `uploads/` (B is this very workspace).
fn kept_on_b() -> std::path::PathBuf {
    trek_core::paths::data_dir().join("uploads")
}

/// B's user message in its own thread `theirs` that `has` picks.
fn b_message(
    ws: &trek_app::workspace::Workspace,
    theirs: &str,
    has: impl Fn(&str) -> bool,
) -> Option<(String, Vec<String>)> {
    ws.live.get(theirs)?.items.iter().find_map(|i| match i {
        Item::User { text, images, .. } if has(text) => Some((text.clone(), images.clone())),
        _ => None,
    })
}

/// His case (0.4.60 refused it): a 2.6 MB .mov from this Mac and a picture, attached to a new
/// thread set to run on the paired machine. Both go there first, in the tile's progress, and the
/// thread starts on B with B's own copies: the video mentioned by B's path to it, the picture
/// attached as an image. The bytes there are the bytes here, under B's `uploads/`, named for this
/// computer. Here, the transcript shows this computer's files (the video plays from the copy here,
/// the picture shows).
#[test]
fn a_video_and_a_picture_go_with_a_new_thread_on_a_paired_machine() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        assert!(trek.read(cx, |ws, _| {
            ws.machines
                .get("machine-b")
                .is_some_and(|m| m.entry.paired.serves(tr::feature::MACHINE_FILES))
        }));
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project.clone()),
                },
                cx,
            );
            ws.set_draft_machine(Some(&project), Some("machine-b".into()), cx);
        });
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.machine_named_for_send(&Scope::Main))
                .as_deref(),
            Some("Machine B")
        );
        let (heard, _sub) = listen(&trek, cx);
        let dir = attachments_dir("his-case");
        let mov = dir.join("724640981761510973.mov");
        let movie = movie_bytes(2_600_000);
        std::fs::write(&mov, &movie).unwrap();
        let png = dir.join("Screenshot 2026-11-20 at 09.12.44.png");
        png_at(&png);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| {
            c.add_files(std::slice::from_ref(&mov), cx);
            c.attach_image(png.clone(), cx);
        });
        trek.type_text(cx, "what happens in this clip?");
        trek.press(cx, "enter");
        trek.render(cx);
        // On its way: the draft stays as it is while the files go.
        assert!(trek.read(cx, |ws, _| matches!(ws.route, Route::Draft { .. })));
        assert_eq!(trek.composer_text(cx), "what happens in this clip?");
        trek.wait_explained(
            cx,
            "the thread on B to open",
            REAL_TIME_LIMIT,
            |ws| matches!(&ws.route, Route::Thread(id) if is_remote(id)),
            |ws| {
                format!(
                    "route {:?}, toasts {:?}",
                    ws.route,
                    heard.lock().unwrap().toasts
                )
            },
        )
        .await;
        let id = trek.thread_id(cx);
        let (_, theirs) = machines::split_id(&id).unwrap();
        let theirs = theirs.to_string();
        trek.wait(cx, "B's thread to have the message", |ws| {
            b_message(ws, &theirs, |t| t.starts_with("what happens in this clip?")).is_some()
        })
        .await;
        let (text, images) = trek.read(cx, |ws, _| {
            b_message(ws, &theirs, |t| t.starts_with("what happens in this clip?")).unwrap()
        });
        // B's message names B's copy of the video, and attaches B's copy of the picture.
        let named = trek_core::media::mentioned_files(&text, None);
        assert_eq!(named.len(), 1, "{text}");
        let kept = &named[0];
        assert_eq!(kept.parent(), Some(kept_on_b().as_path()), "{text}");
        let kept_name = kept.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            kept_name.ends_with(" 724640981761510973.mov") && !kept_name.starts_with("iPhone"),
            "{kept_name}"
        );
        assert!(
            std::fs::read(kept).unwrap() == movie,
            "the video's bytes, intact"
        );
        assert!(!text.contains(&mov.display().to_string()), "{text}");
        assert_eq!(images.len(), 1, "{images:?}");
        let kept_png = std::path::Path::new(&images[0]);
        assert_eq!(kept_png.parent(), Some(kept_on_b().as_path()));
        assert_eq!(
            std::fs::read(kept_png).unwrap(),
            std::fs::read(&png).unwrap()
        );
        // Here: this computer's own files.
        trek.wait(cx, "the message here, with the files here", |ws| {
            ws.live.get(&id).is_some_and(|l| {
                l.items.iter().any(|i| {
                    matches!(i, Item::User { text, images, .. }
                        if text.contains(&*mov.display().to_string())
                            && images == &[png.display().to_string()])
                })
            })
        })
        .await;
        assert!(trek.read(cx, |ws, _| {
            ws.machines.get("machine-b").is_some_and(|m| {
                m.entry
                    .sent
                    .iter()
                    .any(|c| c.here == mov.display().to_string())
            })
        }));
        // The strip let go of them once the message went.
        assert!(cx.read(|cx| composer.read(cx).attached_files()).is_empty());
        assert!(
            heard.lock().unwrap().toasts.is_empty(),
            "{:?}",
            heard.lock().unwrap().toasts
        );
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// A follow-up in a thread already on B takes files too: one on the strip big enough to go in
/// three pieces, and one only typed as `@/path`, mentioned in place by B's path to it.
#[test]
fn a_follow_up_on_a_machine_takes_a_big_file_in_pieces_and_a_typed_mention() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (_id, theirs) = start_on_b(&trek, cx, "hello there").await;
        let dir = attachments_dir("follow-up");
        let big = dir.join("capture.bin");
        let bytes: Vec<u8> = (0..(2 * tr::MAX_UPLOAD_PART as usize + 1234))
            .map(|i| (i * 31 % 253) as u8)
            .collect();
        std::fs::write(&big, &bytes).unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "the notes").unwrap();
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |c, cx| c.add_files(std::slice::from_ref(&big), cx));
        trek.type_text(cx, &format!("compare @{} with this", notes.display()));
        trek.press(cx, "enter");
        trek.wait(cx, "B's thread to have the follow-up", |ws| {
            b_message(ws, &theirs, |t| t.starts_with("compare @")).is_some()
        })
        .await;
        let (text, _) = trek.read(cx, |ws, _| {
            b_message(ws, &theirs, |t| t.starts_with("compare @")).unwrap()
        });
        let named = trek_core::media::mentioned_files(&text, None);
        assert_eq!(named.len(), 2, "{text}");
        // The typed one where it was typed, the strip's after the words.
        assert!(named[0].to_string_lossy().ends_with(" notes.txt"), "{text}");
        assert!(text.contains(" with this"), "{text}");
        assert_eq!(std::fs::read_to_string(&named[0]).unwrap(), "the notes");
        assert!(
            named[1].to_string_lossy().ends_with(" capture.bin"),
            "{text}"
        );
        assert!(
            std::fs::read(&named[1]).unwrap() == bytes,
            "three pieces, put together"
        );
        for p in &named {
            assert_eq!(p.parent(), Some(kept_on_b().as_path()));
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// What can't go says why and leaves the draft as it was: a machine on an older Trek (no
/// `machine_files`), a folder. Cancel stops files on their way, and nothing is sent.
#[test]
fn an_older_trek_a_folder_and_cancel_leave_the_draft_as_it_was() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.render(cx);
        trek.update(cx, |ws, cx| {
            ws.set_draft_machine(None, Some("machine-b".into()), cx)
        });
        let (heard, _sub) = listen(&trek, cx);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        let dir = attachments_dir("refused");
        let pdf = dir.join("report.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        trek.type_text(cx, "please read this");
        composer.update(cx, |c, cx| c.add_files(std::slice::from_ref(&pdf), cx));

        // B as an older Trek: it doesn't say it takes files.
        let features = trek.update(cx, |ws, _| {
            let m = ws
                .machines
                .list
                .iter_mut()
                .find(|m| m.id() == "machine-b")
                .unwrap();
            let all = m.entry.paired.features.clone();
            m.entry
                .paired
                .features
                .retain(|f| f != tr::feature::MACHINE_FILES);
            all
        });
        trek.press(cx, "enter");
        trek.render(cx);
        let still = |trek: &Trek, cx: &mut TestAppContext| {
            assert!(trek.read(cx, |ws, _| matches!(ws.route, Route::Draft { .. })));
            assert_eq!(trek.composer_text(cx), "please read this");
            assert!(
                cx.read(|cx| composer.read(cx).attached_files())
                    .contains(&pdf)
            );
        };
        still(&trek, cx);
        assert!(
            heard
                .lock()
                .unwrap()
                .toasts
                .iter()
                .any(|t| t.starts_with("Machine B needs Trek 0.4.61 or later to receive files.")),
            "{:?}",
            heard.lock().unwrap().toasts
        );
        trek.update(cx, |ws, _| {
            let m = ws
                .machines
                .list
                .iter_mut()
                .find(|m| m.id() == "machine-b")
                .unwrap();
            m.entry.paired.features = features;
        });

        // A folder can't go yet.
        composer.update(cx, |c, cx| c.add_files(std::slice::from_ref(&dir), cx));
        trek.press(cx, "enter");
        trek.render(cx);
        still(&trek, cx);
        assert!(
            heard
                .lock()
                .unwrap()
                .toasts
                .iter()
                .any(|t| t.starts_with("Folders can't go to Machine B yet.")),
            "{:?}",
            heard.lock().unwrap().toasts
        );
        // Taken off, as its tile's × does.
        composer.update(cx, |c, cx| c.remove_attachment(&dir, cx));

        // On its way (a file big enough to take a while), then Cancel: nothing went, and the
        // draft is as it was.
        let big = dir.join("big.mov");
        std::fs::write(&big, movie_bytes(48 << 20)).unwrap();
        composer.update(cx, |c, cx| c.add_files(std::slice::from_ref(&big), cx));
        trek.press(cx, "enter");
        trek.render(cx);
        assert!(
            trek.visible(cx, "machine-upload"),
            "the line saying how far"
        );
        trek.click(cx, "machine-upload-cancel");
        trek.render(cx);
        assert!(!trek.visible(cx, "machine-upload"));
        for _ in 0..20 {
            cx.run_until_parked();
            cx.background_executor
                .timer(Duration::from_millis(10))
                .await;
        }
        still(&trek, cx);
        let _ = std::fs::remove_dir_all(&dir);
    });
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
            sent: vec![],
        };
        machines::save("this-mac", &[entry("a"), entry("b")]).unwrap();
        let (device, read) = machines::load();
        assert_eq!(device, "this-mac");
        assert_eq!(read, vec![entry("a"), entry("b")]);
        assert!(
            trek_private::is_private(&machines::machines_file()).unwrap(),
            "tokens are the owner's alone, on both platforms"
        );
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
                videos: vec![],
                image_paths: vec![],
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
                watch: None,
                sub_agent: None,
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
                asked_by: None,
                always_allow: None,
            },
        ),
        item(
            "r0",
            tr::ItemBody::Approval {
                request_id: "req-0".into(),
                title: "old".into(),
                detail: String::new(),
                state: tr::ApprovalState::Allowed,
                asked_by: None,
                always_allow: None,
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
            trek.read(cx, |ws, _| ws.draft_machine(Some(&trek.project)))
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
            trek.read(cx, |ws, _| ws.draft_machine(Some(&trek.project))),
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
    let (link, _) = scripted("Machine C", move |req, _| match req {
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
            let _ = reply.send(Ok(transcript_of(&["build it there"])));
        }
        tr::HostRequest::MarkSeen { reply, .. } => {
            let _ = reply.send(Ok(()));
        }
        _ => {}
    });
    link
}

/// A phone server named `name` (its id the name in lower case, with dashes) whose host is a
/// script: it says what it is (a Mac with a home folder and the mock agent, no threads listed),
/// and `answer` is asked everything else, with the server's handle to say things unasked
/// (`push`). Its pairing link, and the handle (kept, the server runs for the rest of the test).
fn scripted(
    name: &'static str,
    mut answer: impl FnMut(tr::HostRequest, &tr::RemoteHandle) + Send + 'static,
) -> (PairLink, tr::RemoteHandle) {
    let rt = trek_core::runtime();
    let id = name.to_lowercase().replace(' ', "-");
    let identity =
        tr::TlsIdentity::load_or_create(&new_project(&format!("{id}-tls")), name).unwrap();
    let fingerprint = identity.fingerprint.clone();
    let mut config = tr::ServerConfig::new(tr::HostInfo {
        id,
        name: name.into(),
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
    let server = handle.clone();
    rt.spawn(async move {
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
                        name: name.into(),
                        platform: "macos".into(),
                        trek_version: "0".into(),
                        home: "/Users/c".into(),
                    }));
                }
                req => answer(req, &server),
            }
        }
    });
    (
        PairLink {
            routes: vec![route],
            code,
            fingerprint,
            name: None,
            host_id: None,
        },
        handle,
    )
}

/// What someone typed in a thread on a scripted machine, as its `ix`th item.
fn typed(ix: usize, text: &str) -> tr::Item {
    tr::Item {
        id: format!("i{ix}"),
        seq: ix as u64 + 1,
        at: None,
        body: tr::ItemBody::User {
            text: text.into(),
            images: 0,
            aside: false,
            videos: vec![],
            image_paths: vec![],
        },
    }
}

/// A scripted machine's thread in which `texts` were typed, as it answers `subscribe`.
fn transcript_of(texts: &[&str]) -> tr::Transcript {
    tr::Transcript {
        seq: texts.len() as u64,
        items: texts
            .iter()
            .enumerate()
            .map(|(ix, text)| typed(ix, text))
            .collect(),
        base: 0,
        more: false,
    }
}

/// A scripted machine says `text` was typed in its thread `t-1`, as the thread's `ix`th item.
fn says(machine: &tr::RemoteHandle, ix: usize, text: &str) {
    machine.push(tr::HostEvent::Item {
        thread_id: "t-1".into(),
        item: typed(ix, text),
    });
}

/// What was typed in the thread `id` open here, in order (nothing else is in a scripted
/// machine's thread).
fn typed_in(ws: &trek_app::workspace::Workspace, id: &str) -> Vec<String> {
    ws.live
        .get(id)
        .map(|l| {
            l.items
                .iter()
                .filter_map(|i| match i {
                    Item::User { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pair with the scripted machine `id` at `link` and wait until it has said what it is.
async fn pair_scripted(trek: &Trek, cx: &mut TestAppContext, id: &str, link: PairLink) {
    trek.update(cx, |ws, cx| {
        ws.machines.pairing = None;
        ws.pair_machine(link, cx)
    });
    trek.wait_explained(
        cx,
        "pairing",
        REAL_TIME_LIMIT,
        |ws| matches!(ws.machines.pairing, Some(Pairing::Done(_))),
        |ws| format!("{:?}", ws.machines.pairing),
    )
    .await;
    trek.wait(cx, "the machine's home folder", |ws| {
        ws.machine_home(id).is_some()
    })
    .await;
}

/// What made `a_machine_paired_again_keeps_the_threads_open_here_followed` fail now and then on
/// a loaded runner. A thread opening asked for its transcript, then asked the machine to mark it
/// seen, and only put the transcript in once that was answered; what the machine said of the
/// thread meanwhile came in ahead of it, and the transcript, older, then replaced it. A message
/// sent as the thread opened (as that test sends one, the moment the machine is paired again)
/// was in the thread on its machine and never showed here. Machine D says more of the thread
/// while it's still to answer `mark_seen`: the transcript is here by then, and what follows it
/// stays under it.
#[test]
fn what_a_machine_says_as_its_thread_opens_stays_under_the_transcript() {
    run(async |cx| {
        let trek = open(cx);
        let (seen, answer_seen) = async_channel::bounded::<()>(1);
        let (link, _d) = scripted("Machine D", move |req, d| match req {
            tr::HostRequest::Transcript { reply, .. } => {
                let _ = reply.send(Ok(transcript_of(&["build it there"])));
            }
            // Asked once the transcript has been sent: more is said, and this is answered when
            // the test lets it be, then more again.
            tr::HostRequest::MarkSeen { reply, .. } => {
                says(d, 1, "and test it");
                let (answer_seen, d) = (answer_seen.clone(), d.clone());
                trek_core::runtime().spawn(async move {
                    let _ = answer_seen.recv().await;
                    let _ = reply.send(Ok(()));
                    says(&d, 2, "then ship it");
                });
            }
            _ => {}
        });
        pair_scripted(&trek, cx, "machine-d", link).await;
        let id = local_id("machine-d", "t-1");
        trek.update(cx, |ws, cx| ws.follow_machine_thread(&id, cx));

        trek.wait_explained(
            cx,
            "what the machine said as the thread opened",
            REAL_TIME_LIMIT,
            |ws| typed_in(ws, &id).iter().any(|t| t == "and test it"),
            |ws| open_thread(ws, &id),
        )
        .await;
        let (loaded, typed) = trek.read(cx, |ws, _| {
            (
                ws.live.get(&id).is_some_and(|l| l.loaded),
                typed_in(ws, &id),
            )
        });
        assert_eq!(
            typed,
            ["build it there", "and test it"],
            "the transcript is in before what follows it, whatever the machine has yet to answer"
        );
        assert!(loaded);

        // Its answer to `mark_seen` changes nothing of what the thread shows.
        seen.send(()).await.unwrap();
        trek.wait_explained(
            cx,
            "what the machine said next",
            REAL_TIME_LIMIT,
            |ws| typed_in(ws, &id).iter().any(|t| t == "then ship it"),
            |ws| open_thread(ws, &id),
        )
        .await;
        assert_eq!(
            trek.read(cx, |ws, _| typed_in(ws, &id)),
            ["build it there", "and test it", "then ship it"]
        );
    });
}

/// A thread the old connection couldn't open (here the machine refused it; a connection that
/// dropped as the thread opened, or one the machine no longer took, leaves it the same way)
/// showed why, and went on showing it after the machine was paired again: only the threads that
/// had opened were followed on the new connection, so this one heard nothing until it was left
/// and opened again. Pairing again opens it.
#[test]
fn a_thread_that_couldnt_be_opened_opens_when_its_machine_is_paired_again() {
    run(async |cx| {
        let trek = open(cx);
        let mut asked = 0;
        let (link, d) = scripted("Machine D", move |req, _| match req {
            tr::HostRequest::Transcript { reply, .. } => {
                asked += 1;
                let _ = reply.send(match asked {
                    1 => Err(tr::HostError::other("It can't be read just now")),
                    _ => Ok(transcript_of(&["build it there"])),
                });
            }
            tr::HostRequest::MarkSeen { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
            _ => {}
        });
        pair_scripted(&trek, cx, "machine-d", link.clone()).await;
        let id = local_id("machine-d", "t-1");
        trek.update(cx, |ws, cx| ws.follow_machine_thread(&id, cx));
        trek.wait_explained(
            cx,
            "the thread to say it couldn't be opened",
            REAL_TIME_LIMIT,
            |ws| {
                ws.live.get(&id).is_some_and(|l| {
                    l.items.iter().any(
                        |i| matches!(i, Item::Error { text } if text.contains("It can't be read just now")),
                    )
                })
            },
            |ws| open_thread(ws, &id),
        )
        .await;

        let again = PairLink {
            code: d.pairing_offer().code,
            ..link
        };
        pair_scripted(&trek, cx, "machine-d", again).await;
        trek.wait_explained(
            cx,
            "the thread open on the new connection",
            REAL_TIME_LIMIT,
            |ws| typed_in(ws, &id) == ["build it there"],
            |ws| open_thread(ws, &id),
        )
        .await;
        // And followed there.
        says(&d, 1, "and test it");
        trek.wait_explained(
            cx,
            "what the machine said next",
            REAL_TIME_LIMIT,
            |ws| typed_in(ws, &id) == ["build it there", "and test it"],
            |ws| open_thread(ws, &id),
        )
        .await;
        assert!(
            trek.items(cx, &id)
                .iter()
                .all(|i| !matches!(i, Item::Error { .. })),
            "nothing left of why it couldn't be opened"
        );
    });
}

/// A thread still opening on a machine that is then forgotten: the connection closes, the
/// request for its transcript ends unanswered, and that isn't said of the thread, which is gone
/// from here with its machine (it came back as a thread of no machine, saying it couldn't be
/// opened).
#[test]
fn a_thread_still_opening_goes_with_a_machine_forgotten() {
    run(async |cx| {
        let trek = open(cx);
        // Machine D never answers `subscribe` (it keeps the question).
        let mut kept = vec![];
        let (link, _d) = scripted("Machine D", move |req, _| {
            if let tr::HostRequest::Transcript { reply, .. } = req {
                kept.push(reply);
            }
        });
        pair_scripted(&trek, cx, "machine-d", link).await;
        let id = local_id("machine-d", "t-1");
        trek.update(cx, |ws, cx| ws.follow_machine_thread(&id, cx));
        cx.run_until_parked();
        let client = trek.read(cx, |ws, _| {
            ws.machines
                .get("machine-d")
                .unwrap()
                .client
                .clone()
                .unwrap()
        });
        trek.update(cx, |ws, cx| ws.forget_machine("machine-d", cx));
        // Its connection closed, which is when the request ends.
        trek.wait(cx, "the connection to close", |_| {
            matches!(*client.state().borrow(), tr::client::State::Stopped(_))
        })
        .await;
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.live.get(&id).map(|l| l.items.len())),
            None,
            "nothing of the machine's threads is kept"
        );
    });
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
        // Its list of what git ignores comes from git, off the main thread.
        trek.wait_until(cx, "the sheet to list the ignored files", |trek, cx| {
            trek.render(cx);
            trek.visible(cx, ("clone-file", 2usize))
        })
        .await;
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
            trek.read(cx, |ws, _| ws.draft_machine(Some(&project)))
                .as_deref(),
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
        let working = format!("live-line-working-{id}");
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

/// What `ws` says in toasts, and what it gives back to the composer, from now on; and how often
/// it was told to send the draft.
#[derive(Default)]
struct Heard {
    toasts: Vec<String>,
    given_back: Vec<String>,
    /// Pictures given back to the strip.
    attached: Vec<std::path::PathBuf>,
    sent_draft: usize,
}

fn listen(trek: &Trek, cx: &mut TestAppContext) -> (Arc<Mutex<Heard>>, gpui_kit::Subscription) {
    use trek_app::workspace::WorkspaceEvent;
    let heard = Arc::new(Mutex::new(Heard::default()));
    let seen = heard.clone();
    let sub = trek.update(cx, |_, cx| {
        cx.subscribe_self(move |_, e: &WorkspaceEvent, _| {
            let mut heard = seen.lock().unwrap();
            match e {
                WorkspaceEvent::Toast { message, .. } | WorkspaceEvent::Failure { message } => {
                    heard.toasts.push(message.clone())
                }
                WorkspaceEvent::InsertIntoComposer(text) => heard.given_back.push(text.clone()),
                WorkspaceEvent::AttachImage(path) => heard.attached.push(path.clone()),
                WorkspaceEvent::SendDraft => heard.sent_draft += 1,
                _ => {}
            }
        })
    });
    (heard, sub)
}

#[test]
fn nothing_is_asked_of_a_machine_out_of_reach_it_is_refused_and_given_back() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (id, _) = start_on_b(&trek, cx, "ask permission first").await;
        trek.wait(cx, "the approval card here", |ws| {
            ws.live.get(&id).is_some_and(|l| !l.permissions.is_empty())
        })
        .await;
        let card = trek.request(cx, &id);
        trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().handle.shutdown());
        trek.wait(cx, "B out of reach", |ws| {
            ws.machines.get("machine-b").is_some_and(|m| !m.reachable())
        })
        .await;
        let (heard, _sub) = listen(&trek, cx);
        let project = trek.project.clone();

        // A message: said in a toast, and back in the composer.
        trek.update(cx, |ws, cx| {
            ws.machine_send(&id, "are you there".into(), vec![], None, cx)
        });
        {
            let heard = heard.lock().unwrap();
            assert_eq!(heard.given_back, ["are you there"]);
            assert!(
                heard.toasts[0].contains("Machine B is offline"),
                "{heard:?}",
                heard = heard.toasts
            );
        }
        // Stop.
        trek.update(cx, |ws, cx| ws.machine_interrupt(&id, cx));
        assert_eq!(heard.lock().unwrap().toasts.len(), 2);
        // An answer: the card stays, for when it can be answered.
        trek.update(cx, |ws, cx| {
            ws.machine_answer(
                &id,
                &card,
                tr::AnswerResponse::Plan {
                    approve: true,
                    feedback: None,
                },
                cx,
            )
        });
        assert_eq!(heard.lock().unwrap().toasts.len(), 3);
        assert!(trek.read(cx, |ws, _| {
            ws.live[&id]
                .permissions
                .iter()
                .any(|p| p.request_id == card)
        }));
        // A new thread there: its text goes back too, and its pictures.
        let picture = std::path::PathBuf::from("/tmp/trek-picture.png");
        trek.update(cx, |ws, cx| {
            ws.machine_start_thread(
                "machine-b",
                Some(project.clone()),
                "start it".into(),
                vec![picture.clone()],
                cx,
            )
        });
        {
            let heard = heard.lock().unwrap();
            assert_eq!(heard.given_back, ["are you there", "start it"]);
            assert_eq!(heard.attached, [picture]);
            assert_eq!(heard.toasts.len(), 4);
        }
        // A clone: fails at once, not "Cloning…" for as long as it's away.
        trek.update(cx, |ws, cx| {
            ws.clone_to_machine(
                "machine-b",
                project.clone(),
                "https://example.com/a.git".into(),
                "/tmp/a".into(),
                vec![],
                cx,
            )
        });
        let run = trek.read(cx, |ws, _| ws.machines.clone.clone()).unwrap();
        assert!(
            matches!(&run.stage, CloneStage::Failed(why) if why.contains("offline")),
            "{run:?}"
        );
    });
}

#[test]
fn a_machine_paired_again_keeps_the_threads_open_here_followed() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let (id, _) = start_on_b(&trek, cx, "hello from A").await;
        trek.wait(cx, "B's answer here", |ws| {
            ws.thread(&id)
                .is_some_and(|t| t.run_state == RunState::Idle)
                && ws
                    .live
                    .get(&id)
                    .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::Assistant { .. })))
        })
        .await;

        // Paired again from Settings, without forgetting it first.
        let (code, routes, fingerprint) = trek.read(cx, |ws, _| {
            let paired = &ws.machines.get("machine-b").unwrap().entry.paired;
            (
                ws.remote.as_ref().unwrap().handle.pairing_offer().code,
                paired.routes.clone(),
                paired.fingerprint.clone(),
            )
        });
        trek.update(cx, |ws, cx| {
            ws.machines.pairing = None;
            ws.pair_machine(
                PairLink {
                    routes,
                    code,
                    fingerprint,
                    name: None,
                    host_id: None,
                },
                cx,
            )
        });
        trek.wait_explained(
            cx,
            "pairing again",
            REAL_TIME_LIMIT,
            |ws| matches!(ws.machines.pairing, Some(Pairing::Done(_))),
            |ws| format!("{:?}", ws.machines.pairing),
        )
        .await;
        trek.wait(cx, "B reachable again", |ws| {
            ws.machines.get("machine-b").is_some_and(|m| m.reachable())
        })
        .await;

        // What B says in the thread still reaches the open transcript.
        trek.update(cx, |ws, cx| {
            ws.machine_send(&id, "after pairing again".into(), vec![], None, cx)
        });
        trek.wait_explained(
            cx,
            "the message in the open thread",
            REAL_TIME_LIMIT,
            |ws| {
                ws.live.get(&id).is_some_and(|l| {
                    said(
                        &l.items.iter().cloned().collect::<Vec<_>>(),
                        "after pairing again",
                    )
                })
            },
            |ws| open_thread(ws, &id),
        )
        .await;
        // And it stays there, under what the thread held before, with B's answer under it: the
        // transcript the new connection opened with comes in ahead of what B said since (it
        // could come in after, and replace the message and its answer: what failed this test
        // now and then).
        trek.wait_explained(
            cx,
            "B's answer to it, the turn over",
            REAL_TIME_LIMIT,
            |ws| {
                ws.thread(&id)
                    .is_some_and(|t| t.run_state == RunState::Idle)
                    && ws.live.get(&id).is_some_and(|l| {
                        l.turn_started.is_none()
                            && l.items
                                .iter()
                                .skip_while(|i| !matches!(i, Item::User { text, .. } if text == "after pairing again"))
                                .any(|i| matches!(i, Item::TurnEnd { .. }))
                    })
            },
            |ws| open_thread(ws, &id),
        )
        .await;
        assert_eq!(
            trek.read(cx, |ws, _| typed_in(ws, &id)),
            ["hello from A", "after pairing again"]
        );
    });
}

/// What the open thread `id` on another machine shows, and how its machine stands: for a wait
/// that timed out to say.
fn open_thread(ws: &trek_app::workspace::Workspace, id: &str) -> String {
    let items = ws.live.get(id).map(|l| {
        (
            l.loaded,
            l.items
                .iter()
                .map(|i| match i {
                    Item::User { text, .. } => format!("user {text:?}"),
                    Item::Assistant { text } => format!("assistant {text:?}"),
                    Item::Error { text } => format!("error {text:?}"),
                    Item::TurnEnd { .. } => "turn end".to_string(),
                    _ => "other".to_string(),
                })
                .collect::<Vec<_>>(),
        )
    });
    let reach = machines::split_id(id)
        .and_then(|(host, _)| ws.machines.get(host))
        .map(|m| m.reach.clone());
    format!("(loaded, items) {items:?}, its machine {reach:?}")
}

/// Machine C (scripted), paired and with its home folder known.
async fn pair_c(trek: &Trek, cx: &mut TestAppContext) -> Arc<Mutex<Asked>> {
    let asked = Arc::new(Mutex::new(Asked::default()));
    let link = scripted_machine(asked.clone());
    trek.update(cx, |ws, cx| ws.pair_machine(link, cx));
    trek.wait(cx, "machine C's home folder", |ws| {
        ws.machine_home("machine-c").is_some()
    })
    .await;
    asked
}

#[test]
fn a_finished_clone_sends_the_draft_only_while_that_draft_is_on_screen() {
    run(async |cx| {
        let project = ignoring_project();
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.add_project(project.clone(), cx);
            // Somewhere else, typing something else, as the clone runs.
            ws.navigate(Route::Draft { project: None }, cx);
        });
        let _asked = pair_c(&trek, cx).await;
        let (heard, _sub) = listen(&trek, cx);
        let clone = |trek: &Trek, cx: &mut TestAppContext, folder: &str| {
            let (project, folder) = (project.clone(), folder.to_string());
            trek.update(cx, |ws, cx| {
                ws.clone_to_machine(
                    "machine-c",
                    project,
                    "https://github.com/dokyit/trek.git".into(),
                    folder,
                    vec![],
                    cx,
                )
            });
        };
        let done = |ws: &trek_app::workspace::Workspace| matches!(&ws.machines.clone, Some(r) if matches!(r.stage, CloneStage::Done { .. }));
        clone(&trek, cx, "/Users/c/first");
        trek.wait(cx, "the first clone done", done).await;
        assert_eq!(
            heard.lock().unwrap().sent_draft,
            0,
            "the draft on screen isn't the one cloned"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.draft_machine(Some(&project)))
                .as_deref(),
            Some("machine-c"),
            "the project's draft still starts there"
        );

        // On that draft, what's typed goes.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Draft {
                    project: Some(project.clone()),
                },
                cx,
            )
        });
        clone(&trek, cx, "/Users/c/second");
        trek.wait(cx, "the second clone done", |ws| {
            done(ws)
                && ws
                    .machines
                    .clone
                    .as_ref()
                    .is_some_and(|r| r.folder == "/Users/c/second")
        })
        .await;
        assert_eq!(heard.lock().unwrap().sent_draft, 1);
    });
}

#[test]
fn a_clone_that_finishes_leaves_the_run_of_a_clone_started_since_alone() {
    run(async |cx| {
        let project = ignoring_project();
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.add_project(project.clone(), cx));
        let _asked = pair_c(&trek, cx).await;
        let other = machines::CloneRun {
            host: "machine-d".into(),
            folder: "/Users/d/trek".into(),
            lines: vec![],
            stage: CloneStage::Cloning,
        };
        // The clone to C starts; the sheet is then closed and one to D started (it takes the
        // run's place: there is only the one).
        trek.update(cx, |ws, cx| {
            ws.clone_to_machine(
                "machine-c",
                project.clone(),
                "https://github.com/dokyit/trek.git".into(),
                "/Users/c/trek".into(),
                vec![],
                cx,
            );
            ws.machines.clone = Some(other.clone());
        });
        trek.wait(cx, "C's clone finished", |ws| {
            ws.machines.get("machine-c").is_some_and(|m| {
                m.entry
                    .projects
                    .contains_key(&project.display().to_string())
            })
        })
        .await;
        assert_eq!(
            trek.read(cx, |ws, _| ws.machines.clone.clone()),
            Some(other),
            "D's run isn't marked done by C's clone"
        );
    });
}

/// Machine C's row `t-9` as its Mac would send it: working for a minute and a quarter in its
/// `trek` project, with one of Trek's sub-agents and one of its agent's own out.
fn row_at_work() -> tr::ThreadSummary {
    let agent = tr::AgentRef {
        key: common::mock().key(),
        name: "Mock".into(),
        ..Default::default()
    };
    tr::ThreadSummary {
        id: "t-9".into(),
        title: "Port the sidebar".into(),
        project: tr::ProjectRef {
            id: "p-9".into(),
            name: "trek".into(),
            ..Default::default()
        },
        agent: agent.clone(),
        run_state: tr::RunState::Working,
        section: tr::Section::Working,
        working_since: Some(trek_core::store::now_ms() - 75_000),
        updated_at: trek_core::store::now_ms(),
        sub_agents: vec![
            tr::SubAgent {
                agent: agent.clone(),
                model: Some("Sol".into()),
                title: "Review the diff".into(),
                state: tr::SubAgentState::Running,
                since: Some(trek_core::store::now_ms() - 30_000),
                thread_id: Some("t-10".into()),
                call_id: None,
            },
            tr::SubAgent {
                agent,
                model: None,
                title: "Explore the tests".into(),
                state: tr::SubAgentState::Running,
                since: Some(trek_core::store::now_ms() - 20_000),
                thread_id: None,
                call_id: None,
            },
        ],
        ..Default::default()
    }
}

#[test]
fn a_machines_thread_at_work_stays_on_its_machine_with_its_clock_and_sub_agents() {
    run(async |cx| {
        let trek = open(cx);
        // A quiet thread here, so there's a project group to sit above.
        let here = trek.quiet_thread(cx);
        pair_c(&trek, cx).await;
        let send = |trek: &Trek, cx: &mut TestAppContext, row: tr::ThreadSummary| {
            trek.update(cx, |ws, cx| {
                ws.machine_event(
                    "machine-c",
                    tr::client::Event::Message(tr::ServerMessage::Thread { thread: row }),
                    cx,
                )
            });
            trek.render(cx);
        };
        let id = local_id("machine-c", "t-9");
        send(&trek, cx, row_at_work());

        // At work: its row says so with the clock its row started, as a thread here does.
        assert!(trek.visible(cx, format!("live-line-working-{id}")));
        let clock = trek.read(cx, |ws, _| ws.machines.working_for(&id)).unwrap();
        assert!(clock >= Duration::from_secs(75), "{clock:?}");
        assert!(
            trek.read(cx, |ws, _| ws.any_turn_running()
                || ws.machines.any_at_work()),
            "the sidebar's clock ticks by the second"
        );
        // Its sub-agents: the pill on its row, and their names for its tooltip.
        assert!(trek.visible(cx, format!("live-line-kids-{id}")));
        let sidebar = cx.read(|cx| trek.root.read(cx).sidebar.clone());
        let thread = trek.read(cx, |ws, _| ws.thread(&id).cloned()).unwrap();
        let (kids, _) = sidebar.read_with(cx, |s, cx| s.at_work(&thread, cx));
        let names: Vec<&str> = kids.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["Sol: Review the diff", "Explore the tests"]);
        // How many sub-agents it has out (the tooltip names its provider and model).
        assert!(trek.visible(cx, format!("live-line-kids-count-{id}")));
        // It stays under its machine in Devices, after this Mac's projects, as a thread at work
        // here stays in its project.
        let pid = trek.read(cx, |ws, _| {
            ws.thread(&here).unwrap().project_id.clone().unwrap()
        });
        let row = trek.bounds(cx, format!("live-line-{id}")).unwrap().origin.y;
        let project = trek
            .bounds(cx, format!("live-proj-head-{pid}"))
            .unwrap()
            .origin
            .y;
        let machine = trek
            .bounds(cx, "machine-head-machine-c")
            .expect("its machine's header")
            .origin
            .y;
        assert!(
            project < machine && machine < row,
            "{project:?}, then {machine:?}, then {row:?}"
        );
        assert!(!trek.visible(cx, "section-active"), "no Active section");
        // A machine's count ends on the same right-hand column as a project's (it used to sit
        // further right, past the room a project's header kept for its hover buttons).
        let right = |trek: &Trek, cx: &mut TestAppContext, id: String| {
            trek.bounds(cx, id).expect("drawn").right().as_f32()
        };
        let (project_count, machine_count, clock) = (
            right(&trek, cx, format!("live-proj-count-{pid}")),
            right(&trek, cx, "machine-count-machine-c".to_string()),
            right(&trek, cx, format!("live-line-working-{id}")),
        );
        assert!(
            (project_count - machine_count).abs() < 0.5 && (clock - machine_count).abs() < 0.5,
            "{project_count} {machine_count} {clock}"
        );
        let place = trek.read(cx, |ws, _| ws.machines.place(&id));
        assert_eq!(place.as_deref(), Some("trek on Machine C"));

        // Its turn over with its sub-agents still out: waiting on them, still at work.
        let mut waiting = row_at_work();
        waiting.run_state = tr::RunState::Idle;
        waiting.section = tr::Section::Inbox;
        waiting.working_since = None;
        send(&trek, cx, waiting.clone());
        assert!(trek.visible(cx, format!("live-line-waiting-{id}")));
        assert!(trek.visible(cx, format!("live-line-kids-{id}")));

        // One of them waits on the user: its Inbox row says so.
        let mut asking = waiting.clone();
        asking.needs = Some(tr::Needs {
            kind: tr::NeedsKind::Approval,
            text: "Sub-agent needs you".into(),
        });
        asking.needs_thread = Some("t-10".into());
        send(&trek, cx, asking);
        assert!(trek.visible(cx, format!("inbox-sub-needs-{id}")));
        // In the Inbox with what waits here, and on the badge.
        assert!(trek.visible(cx, "inbox"));
        assert!(!trek.visible(cx, format!("live-line-{id}")));
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 1);

        // Settled there with its sub-agents out, it's still listed here; done, it goes.
        let mut settled = waiting.clone();
        settled.section = tr::Section::Settled;
        send(&trek, cx, settled.clone());
        assert!(trek.visible(cx, format!("live-line-waiting-{id}")));
        settled.sub_agents.clear();
        send(&trek, cx, settled);
        assert!(!trek.visible(cx, format!("inbox-{id}")));
        assert!(!trek.visible(cx, format!("live-line-{id}")));

        // Out of reach, nothing it runs can be seen from here: no clock, no sub-agents.
        send(&trek, cx, row_at_work());
        trek.update(cx, |ws, cx| {
            ws.machine_event(
                "machine-c",
                tr::client::Event::Disconnected {
                    reason: "gone".into(),
                },
                cx,
            )
        });
        trek.render(cx);
        assert!(!trek.visible(cx, format!("live-line-working-{id}")));
        assert!(!trek.visible(cx, format!("live-line-kids-{id}")));
        assert!(trek.read(cx, |ws, _| ws.machines.working_for(&id).is_none()));
        assert!(!trek.read(cx, |ws, _| ws.machines.any_at_work()));
    });
}

#[test]
fn a_machine_in_devices_shows_three_quiet_threads_and_the_rest_behind_show_more() {
    run(async |cx| {
        let trek = open(cx);
        pair_c(&trek, cx).await;
        let now = trek_core::store::now_ms();
        for n in 1..=5 {
            let mut row = row_at_work();
            row.id = format!("t-{n}");
            row.title = format!("Quiet thread {n}");
            row.run_state = tr::RunState::Idle;
            row.section = tr::Section::Inbox;
            row.working_since = None;
            row.sub_agents = vec![];
            row.unseen = false;
            row.updated_at = now - n * 60_000;
            trek.update(cx, |ws, cx| {
                ws.machine_event(
                    "machine-c",
                    tr::client::Event::Message(tr::ServerMessage::Thread { thread: row }),
                    cx,
                )
            });
        }
        trek.render(cx);
        let line = |n: i64| format!("live-line-{}", local_id("machine-c", &format!("t-{n}")));

        // The newest three, then the rest behind "Show 2 more"; the header counts all five.
        assert!(trek.visible(cx, "machine-head-machine-c"));
        for n in 1..=3 {
            assert!(trek.visible(cx, line(n)), "thread {n} on show");
        }
        for n in 4..=5 {
            assert!(!trek.visible(cx, line(n)), "thread {n} behind Show more");
        }
        assert!(trek.visible(cx, "machine-more-machine-c"));

        // Opened, all five; closed again, three.
        trek.click(cx, "machine-more-machine-c");
        trek.render(cx);
        for n in 1..=5 {
            assert!(trek.visible(cx, line(n)), "thread {n} after Show more");
        }
        trek.click(cx, "machine-less-machine-c");
        trek.render(cx);
        assert!(!trek.visible(cx, line(5)));
        assert!(trek.visible(cx, "machine-more-machine-c"));
    });
}

// ---- A thread on another machine at work beyond its own turn ----
//
// Trek paired with itself: each thread here is also one of machine B's, under its own id
// (`local_id("machine-b", …)`). The same thread is read at home and from "this" machine, and
// both must show the same sub-agents, the same working bar and the same background work.

/// The transcript's sub-agent rows on screen, as text ("subagent: Map the routes (Running)").
fn sub_rows(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    trek.rows(cx)
        .into_iter()
        .filter(|r| r.starts_with("subagent: "))
        .collect()
}

/// The sub-agents the transcript on screen shows at work, by the row each stands for.
fn working_rows(trek: &Trek, cx: &TestAppContext) -> Vec<String> {
    let view = trek.thread_view(cx);
    let mut rows = cx.read(|cx| view.read(cx).working_sub_agent_rows(cx));
    rows.sort();
    rows
}

/// The one list of `id`'s sub-agents at work (working bar, hub, phone), by row.
fn at_work(trek: &Trek, cx: &TestAppContext, id: &str) -> Vec<String> {
    let mut rows: Vec<String> = trek.read(cx, |ws, _| {
        ws.sub_agents_at_work(id)
            .into_iter()
            .map(|a| a.row)
            .collect()
    });
    rows.sort();
    rows
}

/// How many sub-agents `id`'s sidebar row counts.
fn badge(trek: &Trek, cx: &TestAppContext, id: &str) -> usize {
    let t = trek
        .read(cx, |ws, _| ws.thread(id).cloned())
        .expect("the thread");
    cx.read(|cx| trek.root.read(cx).sidebar.read(cx).at_work(&t, cx).0.len())
}

/// The working bar's words before its clock ("Waiting on 2 sub-agents").
fn bar_words(trek: &Trek, cx: &TestAppContext) -> Option<String> {
    trek.working_bar(cx)
        .map(|l| l.split(" · ").next().unwrap_or_default().to_string())
}

/// Wait (in real time) until the sub-agent rows on screen satisfy `until`.
async fn wait_rows(
    trek: &Trek,
    cx: &mut TestAppContext,
    what: &str,
    until: impl Fn(&[String]) -> bool,
) {
    let deadline = std::time::Instant::now() + REAL_TIME_LIMIT;
    loop {
        trek.render(cx);
        let rows = sub_rows(trek, cx);
        if until(&rows) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:#?}",
            trek.rows(cx)
        );
        cx.background_executor
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn show(trek: &Trek, cx: &mut TestAppContext, id: &str) {
    let route = Route::Thread(id.to_string());
    trek.update(cx, |ws, cx| ws.navigate(route, cx));
    trek.render(cx);
}

/// The owner's case: a thread on another machine whose agent sent its own sub-agents out (Claude
/// Code's Agent tool) and ended its turn. The sidebar here counted them, but the thread showed no
/// row for any of them, no "Waiting on 2 sub-agents" and nothing to open: a row per sub-agent in
/// the turn that started it now says where it stands as at home, steps and all as they come, the
/// working bar waits on them, the badge and the rows agree, and it all clears when they report.
#[test]
fn a_thread_on_another_machine_shows_its_agents_sub_agents_at_work_as_it_does_at_home() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let home = trek.send(cx, "subagents 9s");
        let h = home.clone();
        trek.wait(cx, "two sub-agents out, its turn over", move |ws| {
            ws.live.get(&h).is_some_and(|l| l.turn_started.is_none())
                && ws.sub_agents_at_work(&h).len() == 2
        })
        .await;
        trek.render(cx);
        let rows_home = sub_rows(&trek, cx);
        let bar_home = bar_words(&trek, cx);
        assert_eq!(rows_home.len(), 2, "{:#?}", trek.rows(cx));
        assert_eq!(bar_home.as_deref(), Some("Waiting on 2 sub-agents"));
        let calls = working_rows(&trek, cx);

        // The same thread from the other machine.
        let id = local_id("machine-b", &home);
        show(&trek, cx, &id);
        let r = id.clone();
        trek.wait(cx, "its sub-agents at work, here", move |ws| {
            ws.thread(&r).is_some() && ws.sub_agents_at_work(&r).len() == 2
        })
        .await;
        wait_rows(&trek, cx, "a row per sub-agent, as at home", |rows| {
            rows == rows_home.as_slice()
        })
        .await;
        assert_eq!(
            working_rows(&trek, cx),
            calls,
            "each at work, on its call's row"
        );
        assert_eq!(at_work(&trek, cx, &id), calls, "the one list agrees");
        assert_eq!(badge(&trek, cx, &id), 2, "and so does the sidebar's badge");
        assert_eq!(
            bar_words(&trek, cx),
            bar_home,
            "the working bar waits on them"
        );
        assert!(trek.read(cx, |ws, _| ws.waiting(&id)));

        // A step each reports shows here as it comes: its row's status line and, opened, the call.
        wait_rows(&trek, cx, "the first scout's step", |rows| {
            rows.iter()
                .any(|r| r.contains("Map the HTTP routes (Reading src/routes.rs · 2 steps)"))
        })
        .await;
        let first = trek.item_ix(
            cx,
            &id,
            |i| matches!(i, Item::Tool { detail, .. } if detail == "Map the HTTP routes"),
        );
        trek.click(cx, ("subagent", first));
        let opened = trek.rows(cx);
        assert!(
            opened.iter().any(|r| r == "  Read src/routes.rs"),
            "opened, it lists its call: {opened:#?}"
        );

        // They report: nothing at work, nothing waited on, each row done, in step with home.
        let (r, h) = (id.clone(), home.clone());
        trek.wait_up_to(
            cx,
            "the scouts to report",
            Duration::from_secs(40),
            move |ws| {
                ws.sub_agents_at_work(&h).is_empty()
                    && ws.live.get(&h).is_some_and(|l| l.turn_started.is_none())
                    && ws.sub_agents_at_work(&r).is_empty()
                    && !ws.waiting(&r)
                    && ws.thread(&r).is_some_and(|t| t.run_state == RunState::Idle)
            },
        )
        .await;
        wait_rows(&trek, cx, "both rows done", |rows| {
            rows.len() == 2 && rows.iter().all(|r| r.contains("(Done"))
        })
        .await;
        assert!(working_rows(&trek, cx).is_empty());
        assert_eq!(badge(&trek, cx, &id), 0);
        assert_eq!(trek.working_bar(cx), None);
        let rows = sub_rows(&trek, cx);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert!(rows.iter().all(|r| r.contains("(Done")), "{rows:#?}");
    });
}

/// One of Trek's own sub-agents (`delegate_task`) of a thread on another machine, itself with
/// two of its agent's own out: the parent's row for it says where it stands and the working bar
/// waits on it, its thread opens here (not listed in the sidebar, as at home) with its own
/// sub-agents' rows and bar, and its Stop stops it there.
#[test]
fn trek_s_own_sub_agent_on_another_machine_opens_here_with_its_own_and_stops_from_here() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let home = trek.send(cx, "mock:delegate subagents 60s");
        let h = home.clone();
        trek.wait(cx, "the sub-agent with two of its own out", move |ws| {
            ws.children(&h)
                .first()
                .and_then(|c| ws.live.get(&c.id))
                .is_some_and(|l| l.turn_started.is_none() && l.background_agents().count() == 2)
                && ws.live.get(&h).is_some_and(|l| l.turn_started.is_none())
        })
        .await;
        let child = trek.read(cx, |ws, _| ws.children(&home)[0].id.clone());
        let row = trek_core::orchestrate::task_row(&child);
        trek.render(cx);
        let rows_home = sub_rows(&trek, cx);
        let bar_home = bar_words(&trek, cx);
        assert_eq!(rows_home.len(), 1, "{:#?}", trek.rows(cx));

        let id = local_id("machine-b", &home);
        show(&trek, cx, &id);
        let (r, k) = (id.clone(), row.clone());
        trek.wait(cx, "the sub-agent at work, here", move |ws| {
            ws.thread(&r).is_some() && ws.sub_agents_at_work(&r).iter().any(|a| a.row == k)
        })
        .await;
        wait_rows(&trek, cx, "its row, as at home", |rows| {
            rows == rows_home.as_slice()
        })
        .await;
        assert_eq!(working_rows(&trek, cx), [row.clone()]);
        assert_eq!(at_work(&trek, cx, &id), [row.clone()]);
        assert_eq!(badge(&trek, cx, &id), 1);
        assert_eq!(bar_words(&trek, cx), bar_home);
        assert!(bar_home.is_some_and(|b| b.starts_with("Waiting on ")));

        // Its thread is the machine's, here: opened from its row, never in the sidebar's list.
        let kid = local_id("machine-b", &child);
        assert!(trek.read(cx, |ws, _| ws.thread(&kid).is_some()));
        assert!(trek.read(cx, |ws, _| {
            ws.machines
                .get("machine-b")
                .is_some_and(|m| m.listed().iter().all(|t| t.id != kid))
        }));
        let ix = trek.item_ix(
            cx,
            &id,
            |i| matches!(i, Item::Tool { id, .. } if *id == row),
        );
        assert!(trek.visible(cx, ("subagent-open", ix)), "it opens");
        assert!(trek.visible(cx, ("subagent-stop", ix)), "it stops");
        // Sub-agents of the sub-agent: shown in its thread, as at home.
        show(&trek, cx, &child);
        let nested_home = sub_rows(&trek, cx);
        assert_eq!(nested_home.len(), 2, "{:#?}", trek.rows(cx));
        show(&trek, cx, &kid);
        let k = kid.clone();
        trek.wait(cx, "its own sub-agents at work, here", move |ws| {
            ws.sub_agents_at_work(&k).len() == 2
        })
        .await;
        wait_rows(&trek, cx, "its own sub-agents' rows, as at home", |rows| {
            rows == nested_home.as_slice()
        })
        .await;
        assert_eq!(
            bar_words(&trek, cx).as_deref(),
            Some("Waiting on 2 sub-agents")
        );

        // Stopped from its row on the parent here: stopped there, and nothing at work here.
        show(&trek, cx, &id);
        trek.click(cx, ("subagent-stop", ix));
        let (r, c) = (id.clone(), child.clone());
        trek.wait_up_to(
            cx,
            "the sub-agent stopped",
            Duration::from_secs(30),
            move |ws| !ws.task_state(&c).live() && ws.sub_agents_at_work(&r).is_empty(),
        )
        .await;
        trek.render(cx);
        assert!(working_rows(&trek, cx).is_empty());
        assert_eq!(badge(&trek, cx, &id), 0);
    });
}

/// A dev server a thread on another machine left running: the strip above the composer lists it
/// here with its last line, as at home; its output is read from the machine, and Stop stops it
/// there.
#[test]
fn background_work_of_a_thread_on_another_machine_shows_reads_and_stops_from_here() {
    run(async |cx| {
        let trek = open(cx);
        let link = serve_as_machine_b(&trek, cx);
        pair(&trek, cx, link).await;
        let home = trek.send(cx, "mock:server 60s");
        trek.wait_done(cx, &home, RunState::Idle).await;
        let h = home.clone();
        trek.wait(cx, "the server's output", move |ws| {
            ws.live[&h]
                .background
                .iter()
                .any(|b| b.last_line().is_some_and(|l| l.contains("localhost")))
        })
        .await;
        trek.render(cx);
        let strip = |trek: &Trek, cx: &TestAppContext| -> Vec<String> {
            cx.read(|cx| trek.root.read(cx).background_strip.read(cx).rows())
        };
        let at_home = strip(&trek, cx);
        assert_eq!(at_home.len(), 1);

        let id = local_id("machine-b", &home);
        show(&trek, cx, &id);
        let r = id.clone();
        trek.wait(cx, "the server, here", move |ws| {
            ws.live.get(&r).is_some_and(|l| {
                l.background
                    .iter()
                    .any(|b| b.last_line().is_some_and(|l| l.contains("localhost")))
            })
        })
        .await;
        trek.render(cx);
        assert!(trek.visible(cx, "background-strip"));
        assert_eq!(strip(&trek, cx), at_home);
        // Its output, read from the machine.
        trek.click(cx, ("bg-task", 0usize));
        let r = id.clone();
        trek.wait(cx, "its output from the machine", move |ws| {
            ws.live
                .get(&r)
                .is_some_and(|l| l.background.iter().any(|b| b.output_rev > 0))
        })
        .await;
        trek.render(cx);
        assert!(trek.visible(cx, "bg-output-text"));
        assert!(
            !trek.visible(cx, "bg-output-full"),
            "the machine's file can't be opened here"
        );
        trek.click(cx, "bg-output-stop");
        let (h, r) = (home.clone(), id.clone());
        trek.wait(cx, "the server stopped there and here", move |ws| {
            ws.live[&h].background.is_empty()
                && ws.live.get(&r).is_some_and(|l| l.background.is_empty())
        })
        .await;
        trek.render(cx);
        assert!(!trek.visible(cx, "background-strip"));
    });
}
