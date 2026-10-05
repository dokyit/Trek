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

#[test]
fn a_phone_reads_sends_and_answers_but_runs_no_trek_commands() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;

        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let row = snapshot.threads.iter().find(|t| t.id == id).expect("listed");
        assert_eq!(row.section, tr::Section::Inbox);
        assert!(snapshot.agents.iter().any(|a| a.key == super::harness::mock().key()), "the mock agent can start threads");

        // Trek's own commands stay on the Mac: the phone can't raise what agents may do.
        let before = trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding);
        let refused = ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "/permissions full".into(), mode: None, images: vec![] }, reply });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), before);
        assert_ne!(before, HandHolding::FullAccess);

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
