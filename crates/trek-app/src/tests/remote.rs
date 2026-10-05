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
        let refused = ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "/permissions full".into(), mode: None }, reply });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), before);
        assert_ne!(before, HandHolding::FullAccess);

        // A follow-up that makes the agent ask first; the phone answers it, once.
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "ask permission first".into(), mode: None }, reply }).unwrap();
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
            req: tr::NewThreadRequest { project_id, agent: super::harness::mock().key(), model: None, text: "from the phone".into(), worktree: false },
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
