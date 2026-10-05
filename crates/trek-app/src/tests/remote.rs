//! The phone's requests, answered by the workspace as the server hands them over.

use super::harness::{open, run};
use crate::workspace::Route;
use trek_core::{HandHolding, RunState};
use trek_remote as tr;

/// Make a request the way the server does and take the reply.
fn ask<T>(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, make: impl FnOnce(tr::Reply<T>) -> tr::HostRequest) -> tr::HostResult<T> {
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    trek.update(cx, |ws, cx| ws.remote_request(make(reply), cx));
    cx.run_until_parked();
    rx.try_recv().expect("answered")
}

/// `ask` for requests answered off the main thread (git, Basecamp, usage): waits, in real time.
async fn ask_later<T>(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, make: impl FnOnce(tr::Reply<T>) -> tr::HostRequest) -> tr::HostResult<T> {
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    trek.update(cx, |ws, cx| ws.remote_request(make(reply), cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if let Ok(answer) = rx.try_recv() {
            return answer;
        }
        assert!(std::time::Instant::now() < deadline, "no answer");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

#[test]
fn tailscale_reach_requires_and_binds_its_address() {
    let none = crate::remote::Addresses { lan: Some("192.168.1.2".parse().unwrap()), tailscale: None };
    assert_eq!(crate::remote::remote_endpoint(&none, trek_core::settings::Reach::Tailscale, 7420).unwrap_err().kind(), std::io::ErrorKind::AddrNotAvailable);
    let addresses = crate::remote::Addresses { lan: none.lan, tailscale: Some("100.64.1.2".parse().unwrap()) };
    let (bind, advertise) = crate::remote::remote_endpoint(&addresses, trek_core::settings::Reach::Tailscale, 7420).unwrap();
    assert_eq!((bind.to_string(), advertise.as_str()), ("100.64.1.2:7420".into(), "100.64.1.2:7420"));
    let (bind, advertise) = crate::remote::remote_endpoint(&addresses, trek_core::settings::Reach::Wifi, 7420).unwrap();
    assert_eq!((bind.to_string(), advertise.as_str()), ("0.0.0.0:7420".into(), "192.168.1.2:7420"));
}

fn send(id: &str, text: &str) -> impl FnOnce(tr::Reply<Option<tr::Open>>) -> tr::HostRequest {
    let req = tr::SendRequest { thread_id: id.to_string(), text: text.to_string(), mode: None, images: vec![] };
    move |reply| tr::HostRequest::Send { req, reply }
}

/// The last notice in `id`: how Trek answered one of its own commands.
fn last_notice(trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, id: &str) -> String {
    trek.items(cx, id).into_iter().rev().find_map(|i| if let trek_core::store::Item::Notice { text } = i { Some(text) } else { None }).unwrap_or_default()
}

#[test]
fn a_phone_reads_sends_and_answers() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;

        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let row = snapshot.threads.iter().find(|t| t.id == id).expect("listed");
        assert_eq!(row.section, tr::Section::Inbox);
        assert!(snapshot.agents.iter().any(|a| a.key == super::harness::mock().key()), "the mock agent can start threads");

        // Full access stays locked, asked for from the phone like anywhere else.
        let before = trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding);
        ask(&trek, cx, send(&id, "/permissions full")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), before);
        assert_ne!(before, HandHolding::FullAccess);
        assert!(last_notice(&trek, cx, &id).contains("Full access is off"), "{}", last_notice(&trek, cx, &id));

        // A follow-up that makes the agent ask first; the phone answers it, once.
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "ask permission first".into(), mode: None, images: vec![] }, reply }).unwrap();
        trek.wait_needs_you(cx, &id).await;
        let request_id = trek.read(cx, |ws, _| ws.live[&id].permissions[0].request_id.clone());
        let allow = |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Allow } },
            reply,
        };
        ask(&trek, cx, allow).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let again = ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Deny } },
            reply,
        });
        assert_eq!(again.unwrap_err().code, tr::ErrorCode::Conflict, "answered already");
    });
}

#[test]
fn a_thread_started_on_the_phone_leaves_the_mac_where_it_was() {
    run(async |cx| {
        let trek = open(cx);
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let route = trek.read(cx, |ws, _| ws.route.clone());
        let id = ask(&trek, cx, |reply| tr::HostRequest::NewThread {
            req: tr::NewThreadRequest { project_id, agent: super::harness::mock().key(), model: None, text: "from the phone".into(), worktree: false, effort: Some("low".into()), access: None, plan: false, images: vec![] },
            reply,
        })
        .unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "the Mac's screen didn't move");
        assert!(trek.read(cx, |ws, _| !ws.tabs.contains(&id)), "nor did a tab open for it");
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if text == "from the phone")));
        let _ = Route::Basecamp;
    });
}

#[test]
fn a_phone_changes_a_thread_s_settings_but_not_past_what_the_mac_allows() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let set = |req: tr::PrefsRequest| move |reply| tr::HostRequest::SetPrefs { req, reply };
        ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), effort: Some("low".into()), access: Some(tr::Access::Supervised), plan: Some(true), ..Default::default() })).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        assert_eq!((t.effort, t.hand_holding), (trek_core::Effort::Low, HandHolding::Supervised));
        let row = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!((row.effort.as_deref(), row.access, row.plan), (Some("low"), Some(tr::Access::Supervised), true), "the row says so");

        // Full access stays locked until the Mac unlocks it.
        let refused = ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), access: Some(tr::Access::FullAccess), ..Default::default() }));
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Supervised);
        assert!(!ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().full_access);
        trek.update(cx, |ws, _| ws.settings.permissions.full_access_unlocked = true);
        ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), access: Some(tr::Access::FullAccess), ..Default::default() })).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::FullAccess);
        let bad = ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), effort: Some("ludicrous".into()), ..Default::default() }));
        assert_eq!(bad.unwrap_err().code, tr::ErrorCode::BadRequest);
    });
}

#[test]
fn a_phone_pins_settles_renames_and_sends_photos() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let act = |action: tr::ThreadAction| {
            let id = id.clone();
            move |reply| tr::HostRequest::ThreadAction { req: tr::ThreadActionRequest { thread_id: id, action }, reply }
        };
        ask(&trek, cx, act(tr::ThreadAction::Pin)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().pinned_at.is_some()));
        ask(&trek, cx, act(tr::ThreadAction::Pin)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().pinned_at.is_some()), "pinning twice keeps it pinned");
        ask(&trek, cx, act(tr::ThreadAction::Rename { title: "From the phone".into() })).unwrap();
        ask(&trek, cx, act(tr::ThreadAction::Settle)).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        assert_eq!(t.title, "From the phone");
        assert!(t.settled_at.is_some());

        // A photo: a 1×1 PNG, base64.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let image = tr::ImageUpload { mime: "image/png".into(), data: png.into() };
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "look at this".into(), mode: None, images: vec![image] }, reply }).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = trek.items(cx, &id).into_iter().rev().find_map(|i| match i {
            trek_core::store::Item::User { text, images, .. } if text == "look at this" => Some(images),
            _ => None,
        });
        let images = sent.expect("the message");
        assert_eq!(images.len(), 1);
        assert!(std::path::Path::new(&images[0]).exists() && images[0].ends_with(".png"), "{images:?}");
        // Not a photo: refused.
        let junk = tr::ImageUpload { mime: "image/png".into(), data: "aGVsbG8=".into() };
        let refused = ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "x".into(), mode: None, images: vec![junk] }, reply });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
    });
}

#[test]
fn trek_s_own_commands_run_from_the_phone_but_full_access_waits_for_the_mac() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let route = trek.read(cx, |ws, _| ws.route.clone());

        // Answered in the thread, as typed on the Mac.
        ask(&trek, cx, send(&id, "/permissions auto")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Auto);
        ask(&trek, cx, send(&id, "/model")).unwrap();
        assert!(last_notice(&trek, cx, &id).contains("Mock"), "{}", last_notice(&trek, cx, &id));
        // Every way of asking for Full access needs the Mac's unlock.
        for ask_for in ["/permissions full", "/access full", "/mode yolo"] {
            ask(&trek, cx, send(&id, ask_for)).unwrap();
            assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Auto, "{ask_for}");
        }
        trek.update(cx, |ws, _| ws.settings.permissions.full_access_unlocked = true);
        ask(&trek, cx, send(&id, "/permissions full")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::FullAccess);

        // `/new` opens the phone's sheet, in the thread's project; the Mac stays where it was.
        let project_id = trek.read(cx, |ws, _| ws.thread(&id).unwrap().project_id.clone());
        let open = ask(&trek, cx, send(&id, "/new")).unwrap();
        assert_eq!(open, Some(tr::Open::NewThread { project_id }));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "the Mac's screen didn't move");

        // `/restate` sends the message asking for a restatement, as the composer does.
        ask(&trek, cx, send(&id, "/restate fix the login page")).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let asked = trek.items(cx, &id).into_iter().rev().find_map(|i| if let trek_core::store::Item::User { text, .. } = i { Some(text) } else { None }).unwrap();
        assert_eq!(asked, trek_core::restate::with_restate("fix the login page"));
        // A consultant the Mac doesn't have is refused with why.
        let refused = ask(&trek, cx, send(&id, "/consult nosuchmodel: hi")).unwrap_err();
        assert_eq!(refused.code, tr::ErrorCode::BadRequest, "{}", refused.message);

        // A thread can't start with a command about one under way.
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let refused = ask(&trek, cx, |reply| tr::HostRequest::NewThread {
            req: tr::NewThreadRequest { project_id, agent: super::harness::mock().key(), model: None, text: "/usage".into(), worktree: false, effort: None, access: None, plan: false, images: vec![] },
            reply,
        });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);

        // The `/` picker's list: Trek's own first, marked so.
        let commands = ask(&trek, cx, |reply| tr::HostRequest::Commands { thread_id: id.clone(), reply }).unwrap();
        let full = commands.iter().find(|c| c.name == "permissions full").expect("listed");
        assert!(full.trek && full.kind == tr::CommandKind::Command);
        assert_eq!(commands[0].name, "new");
    });
}

#[test]
fn rows_carry_the_thread_s_details_and_switching_agent_picks_its_default_model() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let row = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext| ask(trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|r| r.id == id).unwrap();
        let r = row(&trek, cx);
        assert_eq!(r.effort_label.as_deref(), Some("High"));
        assert!(r.model_label.is_some(), "the default model is named too");
        assert_eq!(r.agent.key, super::harness::mock().key());

        // What its session reported shows: the context ring and the cost line.
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.context = Some((50_000, 200_000));
        });
        let r = row(&trek, cx);
        assert_eq!(r.context, Some(tr::ContextUse { used: 50_000, window: 200_000, percent: 25 }));

        // Another agent: its default model, and an effort it takes.
        let relay = trek_core::AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        ask(&trek, cx, |reply| tr::HostRequest::SetPrefs { req: tr::PrefsRequest { thread_id: id.clone(), agent: Some(relay.key()), ..Default::default() }, reply }).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        let models = trek.read(cx, |ws, _| ws.models_for(&relay));
        assert_eq!(t.agent, relay);
        assert_eq!(t.model, crate::composer::default_model(&models).map(|m| m.id.clone()));
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let offered = snapshot.agents.iter().find(|a| a.key == relay.key()).expect("offered");
        assert_eq!(offered.default_model, t.model, "the phone's model list starts where the Mac's does");
        let r = snapshot.threads.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.agent.key, relay.key());
        // Logos are the Mac's own keys.
        assert_eq!(crate::remote::agent_ref(&trek_core::AgentId::ClaudeCode).logo.as_deref(), Some("claude-code"));
        assert_eq!(crate::remote::agent_ref(&trek_core::AgentId::Acp("gemini".into())).logo.as_deref(), Some("gemini"));
    });
}

#[test]
fn a_turn_s_changed_files_follow_its_end() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let end = trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::TurnEnd { .. }));
        let file = tr::ChangedFile { path: "src/a.rs".into(), status: tr::FileStatus::Modified, from: None, added: 3, removed: 1, binary: false };
        crate::remote::tests::TURN_FILES.with(|f| f.borrow_mut().insert((id.clone(), end), vec![file.clone(), tr::ChangedFile { path: "b.png".into(), status: tr::FileStatus::Added, binary: true, added: 0, removed: 0, from: None }]));
        let transcript = ask_later(&trek, cx, |reply| tr::HostRequest::Transcript { thread_id: id.clone(), after_seq: None, limit: None, reply }).await.unwrap();
        let at = transcript.items.iter().position(|i| i.id == format!("i{end}")).expect("the turn end");
        let changes = &transcript.items[at + 1];
        assert_eq!(changes.id, format!("c{end}"));
        let tr::ItemBody::Changes { files, added, removed } = &changes.body else { panic!("{:?}", changes.body) };
        assert_eq!((files[0].clone(), files.len(), *added, *removed), (file, 2, 3, 1));
        assert!(changes.seq > transcript.items[at].seq);
    });
}

#[test]
fn notes_from_the_phone_are_the_mac_s_notes() {
    run(async |cx| {
        let trek = open(cx);
        let note = ask(&trek, cx, |reply| tr::HostRequest::CreateNote { body: "Groceries\n- milk".into(), reply }).unwrap();
        assert_eq!(note.title, "Groceries");
        let dir = trek_core::notes::notes_dir();
        assert!(dir.join(format!("{}.md", note.id)).exists(), "a markdown file in the notes folder");
        assert_eq!(trek.read(cx, |ws, _| ws.notes_epoch), 1, "the Notes screen reads them again");
        let list = ask(&trek, cx, |reply| tr::HostRequest::Notes { reply }).unwrap();
        assert_eq!(list.iter().map(|n| (n.title.as_str(), n.preview.as_str())).collect::<Vec<_>>(), [("Groceries", "milk")]);

        let saved = ask(&trek, cx, |reply| tr::HostRequest::SaveNote { req: tr::SaveNoteRequest { note_id: note.id.clone(), body: "Groceries\n- milk\n- bread".into(), modified: Some(note.modified) }, reply }).unwrap();
        assert!(saved.body.ends_with("bread"));
        // Changed on the Mac since the phone read it: refused, and nothing written.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(dir.join(format!("{}.md", note.id)), "Groceries\n- eggs").unwrap();
        let stale = ask(&trek, cx, |reply| tr::HostRequest::SaveNote { req: tr::SaveNoteRequest { note_id: note.id.clone(), body: "Groceries".into(), modified: Some(saved.modified) }, reply });
        assert_eq!(stale.unwrap_err().code, tr::ErrorCode::Conflict);
        assert_eq!(ask(&trek, cx, |reply| tr::HostRequest::Note { note_id: note.id.clone(), reply }).unwrap().body, "Groceries\n- eggs");
        // An id is never a path.
        let sneaky = ask(&trek, cx, |reply| tr::HostRequest::Note { note_id: "../settings".into(), reply });
        assert_eq!(sneaky.unwrap_err().code, tr::ErrorCode::NotFound);

        ask(&trek, cx, |reply| tr::HostRequest::DeleteNote { note_id: note.id.clone(), reply }).unwrap();
        assert!(dir.join("Deleted").join(format!("{}.md", note.id)).exists(), "kept in Deleted, as on the Mac");
        assert!(ask(&trek, cx, |reply| tr::HostRequest::Notes { reply }).unwrap().is_empty());
    });
}

#[test]
fn a_phone_changes_only_the_settings_it_may() {
    run(async |cx| {
        let trek = open(cx);
        let s = ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap();
        assert_eq!((s.default_agent.as_str(), s.default_access, s.full_access), (super::harness::mock().key().as_str(), tr::Access::Supervised, false));
        let set = |change: tr::SettingsChange| move |reply| tr::HostRequest::SetSettings { change, reply };
        let s = ask(&trek, cx, set(tr::SettingsChange {
            default_effort: Some("low".into()),
            default_access: Some(tr::Access::Auto),
            follow_up: Some(tr::SendMode::Queue),
            notifications: Some(tr::NotifyMode::Banner),
            push: Some(true),
            push_when: Some(tr::PushWhen::Always),
            auto_settle_days: Some(7),
            theme: Some(tr::Theme::Paper),
            ..Default::default()
        }))
        .unwrap();
        assert!(s.push.enabled && s.push.topic.starts_with("trek-"), "turning push on makes a topic");
        assert_eq!(s.push.subscribe_url, Some(format!("ntfy://ntfy.sh/{}", s.push.topic)));
        let mac = trek.read(cx, |ws, _| ws.settings.clone());
        assert_eq!((mac.general.default_effort, mac.general.hand_holding, mac.general.follow_up), (trek_core::Effort::Low, HandHolding::Auto, trek_core::settings::FollowUp::Queue));
        assert_eq!((mac.inbox.auto_settle_days, mac.appearance.theme), (7, trek_core::settings::ThemeChoice::Paper));
        assert_eq!(trek.read(cx, |ws, _| ws.draft_prefs.hand_holding), HandHolding::Auto, "new threads start with it");

        // Full access as the default needs the Mac's unlock; nothing else changes with a refusal.
        let refused = ask(&trek, cx, set(tr::SettingsChange { default_access: Some(tr::Access::FullAccess), auto_settle_days: Some(1), ..Default::default() }));
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| (ws.settings.general.hand_holding, ws.settings.inbox.auto_settle_days)), (HandHolding::Auto, 7));
        let bad = ask(&trek, cx, set(tr::SettingsChange { push_server: Some("javascript:alert(1)".into()), ..Default::default() }));
        assert_eq!(bad.unwrap_err().code, tr::ErrorCode::BadRequest);
        let unknown = ask(&trek, cx, set(tr::SettingsChange { default_agent: Some("acp:nobody".into()), ..Default::default() }));
        assert_eq!(unknown.unwrap_err().code, tr::ErrorCode::NotFound);
        let topic = trek.read(cx, |ws, _| ws.settings.mobile.push_topic.clone());
        let fresh = ask(&trek, cx, set(tr::SettingsChange { new_push_topic: true, ..Default::default() })).unwrap();
        assert_ne!(fresh.push.topic, topic);
        // A test notification needs notifications on (with them on it goes out to ntfy).
        let off = ask(&trek, cx, set(tr::SettingsChange { push: Some(false), push_test: true, ..Default::default() }));
        assert_eq!(off.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert!(trek.read(cx, |ws, _| ws.settings.mobile.push), "a failed test changes nothing");
        // What a phone can't touch, it can't even say: the change has no field for it.
        assert!(!trek.read(cx, |ws, _| ws.settings.permissions.full_access_unlocked));
    });
}

#[test]
fn another_phone_connecting_keeps_the_rows_already_sent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.push_remote_changes(cx);
        });
        let before = trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.clone());
        assert!(!before.is_empty());
        trek.update(cx, |ws, cx| ws.remote_notice(tr::ServerNotice::Connected { device_id: "second-phone".into() }, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.clone()), before);
    });
}

#[test]
fn git_from_the_phone_reviews_commits_and_switches_as_the_panel_does() {
    run(async |cx| {
        let trek = open(cx);
        super::worktrees::make_repo(&trek, cx);
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let target = tr::GitTarget::project(project_id.clone());
        std::fs::write(trek.project.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(trek.project.join("new.txt"), "one\ntwo\n").unwrap();
        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        assert_eq!((status.is_repo, status.branch.as_deref(), status.can_switch), (true, Some("main"), true));
        let files: Vec<(&str, tr::FileStatus, u32)> = status.files.iter().map(|f| (f.path.as_str(), f.status, f.added)).collect();
        assert!(files.contains(&("README.md", tr::FileStatus::Modified, 1)) && files.contains(&("new.txt", tr::FileStatus::Untracked, 2)), "{files:?}");

        let diff = ask_later(&trek, cx, |reply| tr::HostRequest::GitDiff { req: tr::GitDiffRequest { target: target.clone(), path: "README.md".into() }, reply }).await.unwrap();
        assert!(diff.diff.contains("+world"), "{}", diff.diff);
        // Only changed files: a phone reads changes, not the disk.
        let refused = ask_later(&trek, cx, |reply| tr::HostRequest::GitDiff { req: tr::GitDiffRequest { target: target.clone(), path: ".env".into() }, reply }).await;
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::NotFound);

        ask_later(&trek, cx, |reply| tr::HostRequest::GitCommit { req: tr::GitCommitRequest { target: target.clone(), message: "From the phone".into() }, reply }).await.unwrap();
        assert_eq!(trek_core::worktree::git(&trek.project, &["log", "-1", "--format=%s"]).unwrap().trim(), "From the phone");
        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        assert!(status.files.is_empty());

        // Branches, and switching to one (only a local branch, never an option).
        trek_core::worktree::git(&trek.project, &["branch", "dev"]).unwrap();
        let branches = ask_later(&trek, cx, |reply| tr::HostRequest::GitBranches { target: target.clone(), reply }).await.unwrap();
        assert!(branches.branches.contains(&"dev".to_string()) && branches.current.as_deref() == Some("main"));
        let switch = |branch: &str| {
            let req = tr::GitSwitchRequest { target: target.clone(), branch: branch.into() };
            move |reply| tr::HostRequest::GitSwitch { req, reply }
        };
        assert_eq!(ask_later(&trek, cx, switch("--orphan=x")).await.unwrap_err().code, tr::ErrorCode::NotFound);
        ask_later(&trek, cx, switch("dev")).await.unwrap();
        assert_eq!(trek_core::worktree::git(&trek.project, &["branch", "--show-current"]).unwrap().trim(), "dev");
    });
}

#[test]
fn a_worktree_thread_merges_and_is_removed_from_the_phone_only_when_nothing_is_lost_unasked() {
    run(async |cx| {
        let trek = open(cx);
        super::worktrees::make_repo(&trek, cx);
        super::worktrees::use_worktree(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let wt = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())).expect("a worktree");
        let target = tr::GitTarget::thread(id.clone());

        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        let w = status.worktree.clone().expect("a worktree's review");
        assert_eq!((w.branch.as_str(), w.base.as_str(), w.uncommitted), (wt.branch.as_str(), "main", 1));
        assert!(w.merge_blocked.is_some() && !status.can_switch && status.switch_blocked.is_some());
        assert!(status.files.iter().any(|f| f.path == "NOTES.md"));
        // Its branch is the point of it: no switching here.
        let refused = ask_later(&trek, cx, |reply| tr::HostRequest::GitSwitch { req: tr::GitSwitchRequest { target: target.clone(), branch: "main".into() }, reply }).await;
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::Conflict);
        // Uncommitted work blocks the merge, saying why.
        let blocked = ask_later(&trek, cx, |reply| tr::HostRequest::WorktreeMerge { thread_id: id.clone(), reply }).await.unwrap_err();
        assert_eq!(blocked.code, tr::ErrorCode::Conflict);
        assert!(blocked.message.contains("isn't committed"), "{}", blocked.message);
        // Removing it would lose that work: refused, saying so, until the phone insists.
        let remove = |force: bool| {
            let req = tr::WorktreeRemoveRequest { thread_id: id.clone(), delete_branch: false, force };
            move |reply| tr::HostRequest::WorktreeRemove { req, reply }
        };
        let asked = ask_later(&trek, cx, remove(false)).await.unwrap_err();
        assert_eq!(asked.code, tr::ErrorCode::Conflict);
        assert!(asked.message.contains("1 uncommitted change in the worktree would be lost"), "{}", asked.message);
        assert!(wt.path.exists());

        // Committed and merged, it goes with nothing to ask.
        ask_later(&trek, cx, |reply| tr::HostRequest::GitCommit { req: tr::GitCommitRequest { target: target.clone(), message: "Add a note".into() }, reply }).await.unwrap();
        ask_later(&trek, cx, |reply| tr::HostRequest::WorktreeMerge { thread_id: id.clone(), reply }).await.unwrap();
        assert!(trek.project.join("NOTES.md").exists(), "merged into the project folder");
        ask_later(&trek, cx, remove(false)).await.unwrap();
        assert!(!wt.path.exists());
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().worktree.is_none()));
    });
}

#[test]
fn basecamp_and_usage_for_the_phone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Finished and not looked at yet.
        trek.update(cx, |ws, _| ws.threads.iter_mut().find(|t| t.id == id).unwrap().last_seen_at = 0);
        let b = ask_later(&trek, cx, |reply| tr::HostRequest::Basecamp { range: tr::BasecampRange::Today, reply }).await.unwrap();
        assert_eq!((b.range, b.title.as_str(), b.empty), (tr::BasecampRange::Today, "Today's trek", false));
        let summary = b.summary.expect("a recap");
        assert_eq!((summary.prompts, summary.threads), (1, 1));
        assert!(matches!(&b.narrative[..2], [tr::Span::Text { text }, tr::Span::Strong { text: n }] if text == "You sent " && n == "1 prompt"), "{:?}", b.narrative);
        let profile = b.profile.expect("a profile");
        assert_eq!(profile.buckets.len(), 24, "an hour a stretch");
        assert!(profile.summit.is_some() && profile.total == "1 prompt");
        assert!(b.review.iter().any(|r| r.thread_id == id), "unread, so ready for review");
        // The mock's turn took no time and reported no tokens: the tiles say what's known.
        assert!(b.tiles.iter().all(|t| !t.label.is_empty() && !t.figure.is_empty()), "{:?}", b.tiles);
        let all = ask_later(&trek, cx, |reply| tr::HostRequest::Basecamp { range: tr::BasecampRange::All, reply }).await.unwrap();
        assert!(all.greeting.contains("on the trail since"), "{}", all.greeting);

        // No agent here reports a plan: nothing to show, nothing pending.
        let usage = ask_later(&trek, cx, |reply| tr::HostRequest::Usage { reply }).await.unwrap();
        assert!(!usage.loading);
    });
}

fn transcript_req(id: &str, after_seq: Option<u64>, limit: Option<u32>) -> impl FnOnce(tr::Reply<tr::Transcript>) -> tr::HostRequest {
    let thread_id = id.to_string();
    move |reply| tr::HostRequest::Transcript { thread_id, after_seq, limit, reply }
}

fn page_req(id: &str, before: &str, limit: u32) -> impl FnOnce(tr::Reply<tr::TranscriptPage>) -> tr::HostRequest {
    let (thread_id, before) = (id.to_string(), before.to_string());
    move |reply| tr::HostRequest::TranscriptBefore { thread_id, before, limit, reply }
}

fn turn_action(id: &str, item_id: String, action: tr::TurnAction, model: Option<&str>) -> impl FnOnce(tr::Reply<tr::TurnActionDone>) -> tr::HostRequest {
    let req = tr::TurnActionRequest { thread_id: id.to_string(), item_id, action, model: model.map(str::to_string), restore_files: true };
    move |reply| tr::HostRequest::TurnAction { req, reply }
}

/// A thread whose transcript is `turns` made-up turns (five items each), and a request open.
async fn long_thread(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, turns: usize) -> String {
    let id = trek.send(cx, "first thread");
    trek.wait_done(cx, &id, RunState::Idle).await;
    trek.update(cx, |ws, _| {
        let live = ws.live.get_mut(&id).unwrap();
        live.items = trek_core::transcript::Transcript::unsaved(super::harness::transcript(turns));
        live.permissions.push(crate::workspace::PendingPermission { request_id: "req-1".into(), title: "Run command".into(), detail: "rm -rf build".into(), prompt: None, after_turn: false });
        live.revision += 1;
        ws.start_test_remote();
    });
    id
}

#[test]
fn answers_are_matched_by_thread_and_request_id() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "first thread");
        trek.wait_done(cx, &a, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        let b = trek.send(cx, "second thread");
        trek.wait_done(cx, &b, RunState::Idle).await;
        trek.update(cx, |ws, _| {
            for id in [&a, &b] {
                let live = ws.live.get_mut(id).unwrap();
                live.permissions.push(crate::workspace::PendingPermission { request_id: "same-id".into(), title: "Run command".into(), detail: "make".into(), prompt: None, after_turn: false });
                live.revision += 1;
            }
            ws.start_test_remote();
        });
        _ = ask(&trek, cx, transcript_req(&a, None, None)).unwrap();
        _ = ask(&trek, cx, transcript_req(&b, None, None)).unwrap();
        ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: a.clone(), request_id: "same-id".into(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Allow } },
            reply,
        })
        .unwrap();
        trek.read(cx, |ws, _| {
            let answered = &ws.remote.as_ref().unwrap().answered;
            assert_eq!(answered.get(&(a.clone(), "same-id".into())), Some(&tr::AnswerResponse::Approval { decision: tr::Decision::Allow }));
            assert_eq!(answered.get(&(b.clone(), "same-id".into())), None);
        });
    });
}

#[test]
fn oversized_transcript_text_is_clipped_at_char_boundaries() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 1).await;
        let text = format!("é{}TAIL", "\"".repeat(1_100_000));
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            *live.items.get_mut(0).unwrap() = trek_core::store::Item::User { text, images: vec![], at: None, resume: None, aside: false };
            live.revision += 1;
        });
        let transcript = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        let item = transcript.items.iter().find(|i| i.id == "i0").unwrap();
        let tr::ItemBody::User { text, .. } = &item.body else { panic!() };
        assert!(text.contains("bytes omitted") && text.starts_with('é') && text.ends_with("TAIL"));
        assert!(serde_json::to_vec(item).unwrap().len() < 2 << 20);
    });
}

#[test]
fn clipped_secret_questions_are_restored_before_they_are_answered() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let question = format!("Secret? {}", "q".repeat(2_200_000));
        let secret_label = format!("token-{}", "x".repeat(700_000));
        let public_label = format!("account-{}", "y".repeat(700_000));
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.permissions.push(crate::workspace::PendingPermission {
                request_id: "secret".into(),
                title: String::new(),
                detail: String::new(),
                prompt: Some(trek_agents::Prompt::Questions(vec![
                    trek_agents::Question {
                        question: question.clone(),
                        header: "Credentials".into(),
                        options: vec![(secret_label.clone(), "Use the saved token".into())],
                        multi: false,
                        secret: true,
                    },
                    trek_agents::Question {
                        question: "Which account?".into(),
                        header: "Account".into(),
                        options: vec![(public_label.clone(), "Use this account".into())],
                        multi: false,
                        secret: false,
                    },
                ])),
                after_turn: false,
            });
            live.revision += 1;
            ws.start_test_remote();
        });
        let transcript = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        let item = transcript.items.iter().find(|i| i.id == "rsecret").unwrap();
        let tr::ItemBody::Question { questions, .. } = &item.body else { panic!() };
        let shown_question = questions[0].question.clone();
        let shown_secret_label = questions[0].options[0].label.clone();
        let shown_public_label = questions[1].options[0].label.clone();
        assert!(shown_question.contains("bytes omitted") && shown_question != question);
        assert!(shown_secret_label.contains("bytes omitted") && shown_secret_label != secret_label);
        assert!(shown_public_label.contains("bytes omitted") && shown_public_label != public_label);
        assert!(serde_json::to_vec(item).unwrap().len() < 2 << 20);
        ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest {
                thread_id: id.clone(),
                request_id: "secret".into(),
                response: tr::AnswerResponse::Questions {
                    answers: vec![
                        tr::QA { question: shown_question.clone(), answer: shown_secret_label.clone() },
                        tr::QA { question: "Which account?".into(), answer: shown_public_label.clone() },
                    ],
                },
            },
            reply,
        })
        .unwrap();
        trek.read(cx, |ws, _| {
            let live = &ws.live[&id];
            assert!(live.items.iter().any(|item| matches!(item, trek_core::store::Item::User { text, aside: true, .. } if text == &format!("Account: {public_label}"))), "the option sent to the agent uses its original label");
            assert!(!live.items.iter().any(|item| matches!(item, trek_core::store::Item::User { text, .. } if text.contains(&secret_label))), "the secret was not kept as a user message");
            assert!(live.items.iter().any(|item| matches!(item, trek_core::store::Item::Notice { text } if text == "Private answer sent")));
            let tr::AnswerResponse::Questions { answers } = &ws.remote.as_ref().unwrap().answered[&(id.clone(), "secret".into())] else { panic!() };
            assert_eq!((answers[0].question.as_str(), answers[0].answer.as_str()), (shown_question.as_str(), shown_secret_label.as_str()));
            assert_eq!(answers[1].answer, shown_public_label);
        });
    });
}

#[test]
fn a_long_thread_opens_at_its_end_and_pages_back_to_its_start() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 100).await;
        let end = trek.read(cx, |ws, _| ws.live[&id].items.len()) - 1;
        crate::remote::tests::TURN_FILES.with(|f| f.borrow_mut().insert((id.clone(), end), vec![tr::ChangedFile { path: "src/parser.rs".into(), status: tr::FileStatus::Modified, from: None, added: 2, removed: 1, binary: false }]));

        // The last 50 items, the changed files after the last turn end, and the open request.
        let t = ask(&trek, cx, transcript_req(&id, None, Some(50))).unwrap();
        assert!(t.more, "earlier items were left out");
        let ids: Vec<&str> = t.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.iter().filter(|i| i.starts_with('i')).count(), 50);
        assert_eq!(ids[0], format!("i{}", end - 49));
        assert_eq!(&ids[ids.len() - 3..], [format!("i{end}").as_str(), &format!("c{end}"), "rreq-1"]);
        assert!(t.items.iter().all(|i| i.seq <= t.seq && i.seq >= t.base));

        // Nothing changed since: an incremental subscribe brings nothing.
        let again = ask(&trek, cx, transcript_req(&id, Some(t.seq), Some(50))).unwrap();
        assert!(again.items.is_empty() && !again.more, "{:?}", again.items.len());
        assert_eq!(again.seq, t.seq);

        // Paging back to the start, 200 at a time (and never more than the Mac's cap).
        let mut first = ids[0].to_string();
        let mut all: Vec<String> = vec![];
        loop {
            let page = ask(&trek, cx, page_req(&id, &first, 200)).unwrap();
            assert!(page.items.iter().filter(|i| i.id.starts_with('i')).count() <= 200);
            let mut ids: Vec<String> = page.items.iter().map(|i| i.id.clone()).collect();
            first = ids.first().cloned().unwrap();
            ids.append(&mut all);
            all = ids;
            if !page.more {
                break;
            }
        }
        // Every earlier item, once, in order, each turn's edits (counted from its edit tools)
        // right after its end.
        let expected: Vec<String> = (0..end - 49).flat_map(|ix| if ix % 5 == 4 { vec![format!("i{ix}"), format!("c{ix}")] } else { vec![format!("i{ix}")] }).collect();
        assert_eq!(all, expected);
        let big = ask(&trek, cx, page_req(&id, &format!("c{end}"), 10_000)).unwrap();
        assert_eq!(big.items.iter().filter(|i| i.id.starts_with('i')).count(), tr::MAX_PAGE as usize);
        // Before a turn's changed files: up to and with its end.
        let page = ask(&trek, cx, page_req(&id, &format!("c{end}"), 1)).unwrap();
        assert_eq!(page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), [format!("i{end}")]);
        assert_eq!(ask(&trek, cx, page_req(&id, "i99999", 10)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, page_req(&id, "rreq-1", 10)).unwrap_err().code, tr::ErrorCode::NotFound);

        // Without a limit (an older phone): all of it.
        let whole = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        assert!(!whole.more);
        assert_eq!(whole.items.iter().filter(|i| i.id.starts_with('i')).count(), end + 1);
    });
}

/// The mock agent's long history, end to end: over 2000 items, opened at its end and paged in.
#[test]
fn a_thread_of_thousands_of_items_from_the_mock_agent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:history 450");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        assert!(n > 2250, "{n} items");
        let t = ask(&trek, cx, transcript_req(&id, None, Some(200))).unwrap();
        assert!(t.more);
        assert_eq!(t.items.iter().filter(|i| i.id.starts_with('i')).count(), 200);
        let mut before = t.items[0].id.clone();
        let mut seen = 200;
        loop {
            let page = ask(&trek, cx, page_req(&id, &before, 500)).unwrap();
            seen += page.items.iter().filter(|i| i.id.starts_with('i')).count();
            before = page.items[0].id.clone();
            if !page.more {
                break;
            }
        }
        assert_eq!((seen, before.as_str()), (n, "i0"));
        let edits = t.items.iter().filter(|i| matches!(&i.body, tr::ItemBody::Tool { added: Some(_), .. })).count();
        assert!(edits > 30, "edits carry their lines: {edits}");
    });
}

#[test]
fn the_mac_follows_only_threads_a_phone_has_open() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 20).await;
        let t = ask(&trek, cx, transcript_req(&id, None, Some(10))).unwrap();
        let following = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().map(|r| (r.watched.contains_key(&id), r.dormant.contains_key(&id))).unwrap());
        assert_eq!(following(&trek, cx), (true, false));

        // An answer arrives on the Mac: the tick takes it in.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.push(trek_core::store::Item::Notice { text: "Seen on the Mac".into() });
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        let seq = trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().watched[&id].seq);
        assert_eq!(seq, t.seq + 1, "one item changed, one seq");

        // The last phone let go: not followed any more.
        trek.update(cx, |ws, cx| ws.remote_request(tr::HostRequest::Unwatch { thread_id: id.clone() }, cx));
        assert_eq!(following(&trek, cx), (false, true));
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.push(trek_core::store::Item::Notice { text: "While nobody looked".into() });
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        assert_eq!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().dormant[&id].seq), seq, "nothing done for it");

        // Opened again: numbering carries on, and a phone that saw up to `seq` gets just what's new.
        let back = ask(&trek, cx, transcript_req(&id, Some(seq), Some(10))).unwrap();
        assert_eq!(following(&trek, cx), (true, false));
        assert!(back.seq > seq);
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        assert_eq!(back.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(), [format!("i{}", n - 1)]);
        // A phone from before (or another run of Trek) gets it whole.
        let stale = ask(&trek, cx, transcript_req(&id, Some(3), Some(10))).unwrap();
        assert!(stale.items.len() > 10 && stale.more);

        // A rewind: shorter, so phones read it again, and earlier seqs can't be brought up to date.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.truncate(5);
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        let after = ask(&trek, cx, transcript_req(&id, Some(back.seq), Some(10))).unwrap();
        assert!(after.base > back.seq && after.seq >= after.base);
        assert_eq!(after.items.iter().filter(|i| i.id.starts_with('i')).count(), 5, "whole again");
    });
}

#[test]
fn a_phone_undoes_retries_forks_and_rewinds_turns_without_moving_the_mac() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "apple");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "banana".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let (route, tabs) = trek.read(cx, |ws, _| (ws.route.clone(), ws.tabs.clone()));
        let said = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, id: &str| -> Vec<String> {
            trek.items(cx, id).into_iter().filter_map(|i| if let trek_core::store::Item::User { text, .. } = i { Some(text) } else { None }).collect()
        };
        let last_end = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| format!("i{}", trek.items(cx, &id).iter().rposition(|i| matches!(i, trek_core::store::Item::TurnEnd { .. })).unwrap());
        let user = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, text: &str| format!("i{}", trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::User { text: t, .. } if t == text)));

        // Unknown items, and items that aren't what the action takes.
        assert_eq!(ask(&trek, cx, turn_action(&id, "i9999".into(), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, turn_action(&id, "x".into(), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Rewind, None)).unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Retry, Some("no-such-model"))).unwrap_err().code, tr::ErrorCode::NotFound);

        // Fork after the first turn, and from before the second message.
        let first_end = format!("i{}", trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::TurnEnd { .. })));
        let fork = ask(&trek, cx, turn_action(&id, first_end, tr::TurnAction::Fork, None)).unwrap();
        let fork_id = fork.thread_id.expect("the new thread");
        assert_eq!((said(&trek, cx, &fork_id), fork.text), (vec!["apple".to_string()], None));
        let before = ask(&trek, cx, turn_action(&id, user(&trek, cx, "banana"), tr::TurnAction::Fork, None)).unwrap();
        assert_eq!(before.text.as_deref(), Some("banana"), "the message, for the phone's composer");
        assert_eq!(said(&trek, cx, &before.thread_id.unwrap()), ["apple"]);

        // Retry the last turn: the same message, answered again.
        ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Retry, None)).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["apple", "banana"]);

        // Undo it: the message comes back for the composer.
        let undone = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap();
        assert_eq!((undone.thread_id, undone.text.as_deref()), (None, Some("banana")));
        assert_eq!(said(&trek, cx, &id), ["apple"]);

        // While a turn runs, nothing is taken back.
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 20s".into(), vec![], cx));
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&id)).await;
        let busy = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap_err();
        assert_eq!((busy.code, busy.message.as_str()), (tr::ErrorCode::BadRequest, "Stop the running turn to undo"));
        let busy = ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Rewind, None)).unwrap_err();
        assert_eq!(busy.message, "Stop the running turn to rewind");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait(cx, "the turn to stop", |ws| !ws.turn_running(&id)).await;

        // A turn no message of the user's started can't be undone.
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.items.push(trek_core::store::Item::Assistant { text: "A sub-agent reported back.".into() });
            live.items.push(trek_core::store::Item::TurnEnd { at: 1, took_secs: 1 });
        });
        let refused = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap_err();
        assert_eq!(refused.message, "This turn didn't start from a message of yours");

        // Rewind to before the first message (how "Edit" works on the phone).
        let rewound = ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Rewind, None)).unwrap();
        assert_eq!(rewound.text.as_deref(), Some("apple"));
        assert!(said(&trek, cx, &id).is_empty());

        assert_eq!(trek.read(cx, |ws, _| (ws.route.clone(), ws.tabs.clone())), (route, tabs), "the Mac's screen didn't move");
    });
}

/// How long serving and following a 3000-item thread takes (run with `--ignored --nocapture`).
#[test]
#[ignore]
fn timing_of_a_long_thread() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 600).await;
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        type Step<'a> = &'a dyn Fn(&mut crate::workspace::Workspace, &mut gpui_kit::Context<crate::workspace::Workspace>);
        let time = |label: &str, runs: u32, cx: &mut gpui_kit::TestAppContext, f: Step| {
            let t = std::time::Instant::now();
            for _ in 0..runs {
                trek.update(cx, |ws, cx| f(ws, cx));
            }
            eprintln!("{n} items: {label}: {:?}", t.elapsed() / runs);
        };
        time("subscribe, whole transcript", 1, cx, &|ws, cx| _ = ws.remote_transcript(&id, None, None, cx));
        trek.update(cx, |ws, _| {
            let r = ws.remote.as_mut().unwrap();
            r.watched.clear();
            r.dormant.clear();
        });
        time("subscribe, last 200", 1, cx, &|ws, cx| _ = ws.remote_transcript(&id, None, Some(200), cx));
        time("subscribe again, nothing new (after_seq)", 1, cx, &|ws, cx| {
            let seq = ws.remote.as_ref().unwrap().watched[&id].seq;
            _ = ws.remote_transcript(&id, Some(seq), Some(200), cx)
        });
        time("page of 200", 1, cx, &|ws, cx| _ = ws.remote_transcript_before(&id, &format!("i{}", n - 200), 200, cx));
        time("tick, idle", 50, cx, &|ws, cx| ws.push_remote_changes(cx));
        trek.update(cx, |ws, _| {
            let l = ws.live.get_mut(&id).unwrap();
            let ix = l.items.push(trek_core::store::Item::Assistant { text: "streaming".into() });
            l.streaming = Some(ix);
            l.revision += 1;
        });
        time("tick, streaming", 50, cx, &|ws, cx| {
            let l = ws.live.get_mut(&id).unwrap();
            if let Some(trek_core::store::Item::Assistant { text }) = l.items.last_mut() {
                text.push_str(" more words");
            }
            l.revision += 1;
            ws.push_remote_changes(cx)
        });
    });
}

#[test]
fn the_phone_allows_for_a_whole_session_only_where_the_mac_lets_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "ask permission first");
        trek.wait_needs_you(cx, &id).await;
        let request_id = trek.read(cx, |ws, _| ws.live[&id].permissions[0].request_id.clone());
        let for_session = |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::AllowForSession } },
            reply,
        };
        // Off (as it starts): refused, whatever the phone's own check was. The card stays, and
        // nothing is remembered as answered.
        let settings = ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap();
        assert!(!settings.session_approvals);
        let refused = ask(&trek, cx, for_session).unwrap_err();
        assert!(refused.message.contains("off on this Mac"), "{refused:?}");
        assert_eq!(trek.run_state(cx, &id), RunState::NeedsYou);
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.len() == 1 && ws.remote.as_ref().is_none_or(|r| r.answered.is_empty())));
        // Turned on at the Mac, the same answer goes through.
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.session_approvals = true;
            ws.save_settings(cx);
        });
        assert!(ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap().session_approvals);
        ask(&trek, cx, for_session).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn a_long_answer_still_streaming_goes_less_often_and_whole_when_it_ends() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 3).await;
        ask(&trek, cx, transcript_req(&id, None, Some(10))).unwrap();
        let seq = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().watched[&id].seq);
        // An answer streams in: `more` is added to it and the tick run.
        let grow = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, more: &str| {
            trek.update(cx, |ws, cx| {
                let l = ws.live.get_mut(&id).unwrap();
                match l.streaming {
                    Some(ix) => {
                        if let Some(trek_core::store::Item::Assistant { text }) = l.items.get_mut(ix) {
                            text.push_str(more)
                        }
                    }
                    None => l.streaming = Some(l.items.push(trek_core::store::Item::Assistant { text: more.into() })),
                }
                l.revision += 1;
                ws.push_remote_changes(cx);
            })
        };
        // Short, it goes with every tick it grew in.
        let start = seq(&trek, cx);
        grow(&trek, cx, "The parser");
        grow(&trek, cx, " reads flags");
        assert_eq!(seq(&trek, cx), start + 2);
        // Long (a megabyte), it's held back between sends: the ticks right after one send nothing.
        grow(&trek, cx, &"x".repeat(1 << 20));
        let long = seq(&trek, cx);
        grow(&trek, cx, " and more");
        grow(&trek, cx, " and more");
        assert_eq!(seq(&trek, cx), long, "held back");
        // It ends: the phone gets it as it ended, at once, though nothing more was added.
        trek.update(cx, |ws, cx| {
            let l = ws.live.get_mut(&id).unwrap();
            l.streaming = None;
            l.revision += 1;
            ws.push_remote_changes(cx);
        });
        assert_eq!(seq(&trek, cx), long + 1);
        let back = ask(&trek, cx, transcript_req(&id, Some(long), Some(10))).unwrap();
        assert!(back.items.iter().any(|i| matches!(&i.body, tr::ItemBody::Assistant { text, streaming: false } if text.ends_with(" and more and more"))), "whole, as it ended");
    });
}

#[test]
fn thread_rows_are_built_only_when_something_of_the_threads_changed() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let row = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.get(&id).map(|r| r.title.clone()));
        trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        assert_eq!(row(&trek, cx).as_deref(), Some(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()).as_str()));
        // A change made the usual way is in the rows at the next tick.
        trek.update(cx, |ws, cx| {
            ws.rename(&id, "Renamed on the Mac".into(), cx);
            ws.push_remote_changes(cx);
        });
        assert_eq!(row(&trek, cx).as_deref(), Some("Renamed on the Mac"));
    });
}
