//! The wire format, checked against the literal examples in `docs/MOBILE.md`.

use serde_json::{Value, json};
use trek_remote::*;

fn value(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

/// Parse `line` as a client message, re-serialize it and compare with the original JSON.
fn client_round_trip(line: &str) -> ClientEnvelope {
    let env: ClientEnvelope = serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
    assert_eq!(serde_json::to_value(&env).unwrap(), value(line), "client round trip of {line}");
    env
}

fn server_round_trip(line: &str) -> ServerEnvelope {
    let env: ServerEnvelope = serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
    assert_eq!(serde_json::to_value(&env).unwrap(), value(line), "server round trip of {line}");
    env
}

fn item_round_trip(line: &str) -> Item {
    let item: Item = serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
    assert_eq!(serde_json::to_value(&item).unwrap(), value(line), "item round trip of {line}");
    item
}

#[test]
fn spec_handshake_messages() {
    let pair = client_round_trip(
        r#"{"type":"pair","id":"1","protocol":1,"code":"K7Q2-9XMV","device_id":"6F1C…","device_name":"Tobias's iPhone","app_version":"0.1.0"}"#,
    );
    assert_eq!(pair.id.as_deref(), Some("1"));
    assert_eq!(
        pair.msg,
        ClientMessage::Pair {
            protocol: 1,
            code: "K7Q2-9XMV".into(),
            device_id: "6F1C…".into(),
            device_name: "Tobias's iPhone".into(),
            app_version: Some("0.1.0".into()),
        }
    );
    let hello = client_round_trip(
        r#"{"type":"hello","id":"1","protocol":1,"device_id":"6F1C…","token":"q3t…","app_version":"0.1.0"}"#,
    );
    assert!(matches!(hello.msg, ClientMessage::Hello { protocol: 1, ref token, .. } if token == "q3t…"));

    let paired = server_round_trip(
        r#"{"type":"paired","re":"1","protocol":1,"token":"q3t…","host":{"id":"7f3c…","name":"Tobias's MacBook Pro","version":"0.3.2"}}"#,
    );
    assert_eq!(paired.re.as_deref(), Some("1"));
    server_round_trip(
        r#"{"type":"welcome","re":"1","protocol":1,"host":{"id":"7f3c…","name":"Tobias's MacBook Pro","version":"0.3.2"}}"#,
    );
}

const SPEC_SNAPSHOT: &str = r#"{"type":"snapshot",
 "threads":[{
   "id":"01J…","title":"Fix the flaky auth test",
   "project":{"id":"p1","name":"trek-api","hue":212,"monogram":"TR"},
   "agent":{"key":"claude-code","name":"Claude Code"},
   "model":"claude-opus-5-5","model_label":"Opus 5.5",
   "run_state":"needs-you",
   "needs":{"kind":"approval","text":"Run `cargo test -p auth`"},
   "section":"inbox","unseen":true,"pinned":false,
   "branch":"fix/auth-flake","worktree":true,
   "activity":"Running cargo test","working_since":null,
   "updated_at":1791020400000,"additions":42,"deletions":7}],
 "projects":[{"id":"p1","name":"trek-api","hue":212,"monogram":"TR","branch":"main","is_repo":true}],
 "agents":[{"key":"claude-code","name":"Claude Code","default_model":"claude-opus-5-5",
            "models":[{"id":"claude-opus-5-5","label":"Opus 5.5"},{"id":"claude-sonnet-5-5","label":"Sonnet 5.5"}]}]}"#;

#[test]
fn spec_snapshot() {
    let env = server_round_trip(SPEC_SNAPSHOT);
    let ServerMessage::Snapshot(snapshot) = env.msg else { panic!("not a snapshot") };
    let t = &snapshot.threads[0];
    assert_eq!(t.run_state, RunState::NeedsYou);
    assert_eq!(t.section, Section::Inbox);
    assert_eq!(t.needs.as_ref().unwrap().kind, NeedsKind::Approval);
    assert_eq!(t.updated_at, 1_791_020_400_000);
    assert_eq!(snapshot.agents[0].models.len(), 2);
    assert!(snapshot.projects[0].is_repo);

    // The thread on its own, as a `thread` upsert.
    let thread_line = json!({"type": "thread", "thread": value(SPEC_SNAPSHOT)["threads"][0].clone()}).to_string();
    server_round_trip(&thread_line);
    server_round_trip(r#"{"type":"thread_removed","thread_id":"01J…"}"#);
}

#[test]
fn thread_summary_serializes_like_the_spec() {
    let thread = ThreadSummary {
        id: "01J".into(),
        title: "Fix the flaky auth test".into(),
        project: ProjectRef { id: "p1".into(), name: "trek-api".into(), hue: 212, monogram: "TA".into(), icon: None },
        agent: AgentRef { key: "claude-code".into(), name: "Claude Code".into(), logo: None, icon: None },
        model: Some("claude-opus-5-5".into()),
        model_label: Some("Opus 5.5".into()),
        run_state: RunState::NeedsYou,
        needs: Some(Needs { kind: NeedsKind::Approval, text: "Run `cargo test -p auth`".into() }),
        section: Section::Inbox,
        unseen: true,
        pinned: false,
        branch: Some("fix/auth-flake".into()),
        worktree: true,
        activity: None,
        working_since: None,
        updated_at: 1_791_020_400_000,
        additions: 42,
        deletions: 7,
        effort: Some("high".into()),
        access: Some(Access::Auto),
        plan: false,
        ..Default::default()
    };
    let v = serde_json::to_value(&thread).unwrap();
    assert_eq!(v["run_state"], "needs-you");
    assert_eq!(v["needs"], json!({"kind": "approval", "text": "Run `cargo test -p auth`"}));
    assert_eq!(v["section"], "inbox");
    assert_eq!(v["working_since"], Value::Null);
    assert_eq!(v["activity"], Value::Null);
    assert_eq!(v["updated_at"], json!(1_791_020_400_000i64));
    assert_eq!(v["project"]["hue"], 212);
    assert_eq!(v["agent"]["key"], "claude-code");

    for (state, text) in [
        (RunState::Idle, "idle"),
        (RunState::Working, "working"),
        (RunState::NeedsYou, "needs-you"),
        (RunState::Failed, "failed"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), text);
    }
    for (section, text) in [
        (Section::Pinned, "pinned"),
        (Section::Inbox, "inbox"),
        (Section::Working, "working"),
        (Section::Snoozed, "snoozed"),
        (Section::Settled, "settled"),
    ] {
        assert_eq!(serde_json::to_value(section).unwrap(), text);
    }
    for kind in ["approval", "question", "plan", "failed", "limit"] {
        let k: NeedsKind = serde_json::from_value(json!(kind)).unwrap();
        assert_eq!(serde_json::to_value(k).unwrap(), kind);
    }

    // Minimal rows from a sloppy peer still parse.
    let minimal: ThreadSummary = serde_json::from_value(json!({
        "id": "t", "title": "T",
        "project": {"id": "p", "name": "P", "hue": 1, "monogram": "P"},
        "agent": {"key": "codex", "name": "Codex"},
        "run_state": "idle", "section": "settled",
    }))
    .unwrap();
    assert_eq!(minimal.model, None);
    assert_eq!(minimal.needs, None);
    assert!(!minimal.unseen);
}

#[test]
fn spec_transcript_messages() {
    let sub = client_round_trip(r#"{"type":"subscribe","id":"7","thread_id":"01J…","after_seq":null}"#);
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "01J…".into(), after_seq: None, limit: None });
    let sub: ClientEnvelope = serde_json::from_str(r#"{"type":"subscribe","thread_id":"t","after_seq":5}"#).unwrap();
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "t".into(), after_seq: Some(5), limit: None });
    let sub: ClientEnvelope = serde_json::from_str(r#"{"type":"subscribe","thread_id":"t"}"#).unwrap();
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "t".into(), after_seq: None, limit: None });
    client_round_trip(r#"{"type":"unsubscribe","thread_id":"01J…"}"#);

    server_round_trip(r#"{"type":"transcript","re":"7","thread_id":"01J…","reset":true,"seq":118,"items":[]}"#);
    server_round_trip(
        r#"{"type":"transcript","re":"7","thread_id":"01J…","reset":false,"seq":2,"items":[{"id":"a1","seq":1,"at":1791020000000,"kind":"user","text":"hi","images":0}]}"#,
    );
    server_round_trip(
        r#"{"type":"item","thread_id":"01J…","item":{"id":"a6","seq":6,"at":null,"kind":"assistant","text":"The race is in **`refresh()`**…","streaming":false}}"#,
    );
    server_round_trip(r#"{"type":"transcript_reset","thread_id":"01J…"}"#);
}

#[test]
fn long_transcripts_and_turn_actions() {
    let sub = client_round_trip(r#"{"type":"subscribe","id":"7","thread_id":"t","after_seq":null,"limit":200}"#);
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "t".into(), after_seq: None, limit: Some(200) });
    server_round_trip(r#"{"type":"transcript","re":"7","thread_id":"t","reset":true,"seq":118,"items":[],"more":true}"#);
    // `more` is left out when false, and missing reads as false.
    let t = server_round_trip(r#"{"type":"transcript","thread_id":"t","reset":true,"seq":1,"items":[]}"#);
    assert!(matches!(t.msg, ServerMessage::Transcript { more: false, .. }));

    let before = client_round_trip(r#"{"type":"transcript_before","id":"8","thread_id":"t","before":"i340","limit":200}"#);
    assert_eq!(before.msg, ClientMessage::TranscriptBefore { thread_id: "t".into(), before: "i340".into(), limit: 200 });
    assert!(before.msg.is_query(), "a read: it doesn't wait behind actions");
    server_round_trip(
        r#"{"type":"transcript_page","re":"8","thread_id":"t","items":[{"id":"i139","seq":4,"at":null,"kind":"notice","text":"Interrupted"}],"more":true}"#,
    );
    server_round_trip(r#"{"type":"transcript_page","re":"8","thread_id":"t","items":[],"more":false}"#);

    let undo = client_round_trip(r#"{"type":"turn_action","id":"9","thread_id":"t","item_id":"i12","action":"undo","restore_files":true}"#);
    assert_eq!(undo.msg, ClientMessage::TurnAction(TurnActionRequest { thread_id: "t".into(), item_id: "i12".into(), action: TurnAction::Undo, model: None, restore_files: true }));
    client_round_trip(r#"{"type":"turn_action","thread_id":"t","item_id":"i12","action":"retry","model":"gpt-6","restore_files":false}"#);
    // Files are restored unless asked not to.
    let fork: ClientEnvelope = serde_json::from_str(r#"{"type":"turn_action","thread_id":"t","item_id":"i3","action":"fork"}"#).unwrap();
    assert!(matches!(fork.msg, ClientMessage::TurnAction(TurnActionRequest { action: TurnAction::Fork, restore_files: true, .. })));
    for action in ["undo", "retry", "fork", "rewind"] {
        let line = format!(r#"{{"type":"turn_action","thread_id":"t","item_id":"i1","action":"{action}"}}"#);
        assert_eq!(serde_json::from_str::<ClientEnvelope>(&line).unwrap().msg.kind(), "turn_action");
    }
    assert!(serde_json::from_str::<ClientEnvelope>(r#"{"type":"turn_action","thread_id":"t","item_id":"i1","action":"redo"}"#).is_err());
    let ack = server_round_trip(r#"{"type":"ack","re":"9","text":"Fix the parser"}"#);
    assert_eq!(ack.msg, ServerMessage::Ack { thread_id: None, open: None, text: Some("Fix the parser".into()), project_id: None });
    server_round_trip(r#"{"type":"ack","re":"10","thread_id":"t-fork","text":"Fix the parser"}"#);
}

const SPEC_ITEMS: &[&str] = &[
    r#"{"id":"a1","seq":1,"at":1791020000000,"kind":"user","text":"The auth test is flaky, find out why","images":0}"#,
    r#"{"id":"a2","seq":2,"at":null,"kind":"reasoning","text":"Let me look at the test first…"}"#,
    r#"{"id":"a3","seq":3,"at":null,"kind":"tool","call_id":"toolu_1","tool":"read","title":"Read","detail":"src/auth/session.rs","status":"done","output":"","added":null,"removed":null}"#,
    r#"{"id":"a4","seq":4,"at":null,"kind":"tool","call_id":"toolu_2","tool":"edit","title":"Edit","detail":"src/auth/session.rs","status":"done","output":"","added":12,"removed":3}"#,
    r#"{"id":"a5","seq":5,"at":null,"kind":"tool","call_id":"toolu_3","tool":"command","title":"Run","detail":"cargo test -p auth","status":"running","output":"   Compiling auth v0.1.0…","added":null,"removed":null}"#,
    r#"{"id":"a6","seq":6,"at":null,"kind":"assistant","text":"The race is in **`refresh()`**…","streaming":false}"#,
    r#"{"id":"a7","seq":7,"at":null,"kind":"approval","request_id":"r1","title":"Run command","detail":"rm -rf target/","state":"pending"}"#,
    r#"{"id":"a8","seq":8,"at":null,"kind":"question","request_id":"r2","questions":[{"header":"Scope","question":"Fix only the test, or the session code too?","options":[{"label":"Test only","description":"Smallest change"},{"label":"Both","description":"Fix the race"}],"multi":false,"secret":false}],"state":"pending","answers":null}"#,
    r#"{"id":"a9","seq":9,"at":null,"kind":"plan","request_id":"r3","markdown":"1. …","state":"pending"}"#,
    r#"{"id":"b1","seq":10,"at":1791020400000,"kind":"turn_end","took_secs":95}"#,
    r#"{"id":"b2","seq":11,"at":null,"kind":"notice","text":"Switched to Full access"}"#,
    r#"{"id":"b3","seq":12,"at":null,"kind":"error","text":"Interrupted"}"#,
    r#"{"id":"b4","seq":13,"at":null,"kind":"limit","text":"5-hour limit reached","resets_at":1791030000000}"#,
    r#"{"id":"b5","seq":14,"at":null,"kind":"handoff","from":"Claude Opus 5.5","to":"Codex GPT-6"}"#,
];

#[test]
fn spec_items_round_trip() {
    let items: Vec<Item> = SPEC_ITEMS.iter().map(|line| item_round_trip(line)).collect();
    let kinds: Vec<&str> = items.iter().map(|i| i.body.kind()).collect();
    assert_eq!(
        kinds,
        [
            "user",
            "reasoning",
            "tool",
            "tool",
            "tool",
            "assistant",
            "approval",
            "question",
            "plan",
            "turn_end",
            "notice",
            "error",
            "limit",
            "handoff"
        ]
    );
    assert!(matches!(items[3].body, ItemBody::Tool { tool: ToolKind::Edit, added: Some(12), removed: Some(3), .. }));
    assert!(matches!(items[4].body, ItemBody::Tool { tool: ToolKind::Command, status: ToolStatus::Running, .. }));
    assert_eq!(items[0].at, Some(1_791_020_000_000));
    let ItemBody::Question { questions, state, answers, .. } = &items[7].body else { panic!() };
    assert_eq!(questions[0].options[1].label, "Both");
    assert_eq!(*state, QuestionState::Pending);
    assert_eq!(*answers, None);
}

#[test]
fn every_item_state_round_trips() {
    let mk = |body: ItemBody| Item { id: "x".into(), seq: 3, at: Some(1), body };
    let mut bodies = Vec::new();
    for state in [
        ApprovalState::Pending,
        ApprovalState::Allowed,
        ApprovalState::AllowedForSession,
        ApprovalState::Denied,
        ApprovalState::Resolved,
    ] {
        bodies.push(ItemBody::Approval { request_id: "r".into(), title: "Run".into(), detail: "ls".into(), state });
    }
    for state in [QuestionState::Pending, QuestionState::Answered, QuestionState::Resolved] {
        bodies.push(ItemBody::Question {
            request_id: "r".into(),
            questions: vec![Question {
                header: "H".into(),
                question: "Q?".into(),
                options: vec![],
                multi: true,
                secret: true,
            }],
            state,
            answers: Some(vec![QA { question: "Q?".into(), answer: "A".into() }]),
        });
    }
    for state in [PlanState::Pending, PlanState::Approved, PlanState::Rejected, PlanState::Resolved] {
        bodies.push(ItemBody::Plan { request_id: "r".into(), markdown: "1.".into(), state });
    }
    for status in [ToolStatus::Running, ToolStatus::Done, ToolStatus::Failed, ToolStatus::Denied] {
        for tool in
            [ToolKind::Command, ToolKind::Read, ToolKind::Edit, ToolKind::Search, ToolKind::Web, ToolKind::Agent, ToolKind::Other]
        {
            bodies.push(ItemBody::Tool {
                call_id: "c".into(),
                tool,
                title: "T".into(),
                detail: "d".into(),
                status,
                output: "o".into(),
                added: None,
                removed: Some(1),
            });
        }
    }
    bodies.push(ItemBody::User { text: "u".into(), images: 2, aside: false });
    bodies.push(ItemBody::Assistant { text: "a".into(), streaming: true });
    bodies.push(ItemBody::Limit { text: "l".into(), resets_at: None });
    for body in bodies {
        let item = mk(body);
        let json = serde_json::to_string(&item).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        assert_eq!(back, item, "{json}");
        let msg = ServerEnvelope::push(ServerMessage::Item { thread_id: "t".into(), item });
        let back: ServerEnvelope = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(back, msg);
    }
    let v = serde_json::to_value(mk(ItemBody::Approval {
        request_id: "r".into(),
        title: "t".into(),
        detail: String::new(),
        state: ApprovalState::AllowedForSession,
    }))
    .unwrap();
    assert_eq!(v["state"], "allowed_for_session");
    assert_eq!(v["kind"], "approval");
}

#[test]
fn items_tolerate_missing_optional_fields() {
    let item: Item = serde_json::from_str(r#"{"id":"a","seq":1,"kind":"tool","call_id":"c","tool":"read","title":"Read","status":"done","future_field":1}"#).unwrap();
    assert_eq!(item.at, None);
    assert!(matches!(item.body, ItemBody::Tool { added: None, removed: None, ref output, .. } if output.is_empty()));
    let item: Item = serde_json::from_str(r#"{"id":"a","seq":1,"kind":"limit","text":"x"}"#).unwrap();
    assert!(matches!(item.body, ItemBody::Limit { resets_at: None, .. }));
}

#[test]
fn settings_actions_and_photos() {
    let p = client_round_trip(r#"{"type":"set_prefs","id":"1","thread_id":"t","model":"gpt-6","effort":"high","access":"auto-accept-edits","plan":true}"#);
    assert_eq!(
        p.msg,
        ClientMessage::SetPrefs(PrefsRequest {
            thread_id: "t".into(),
            agent: None,
            model: Some("gpt-6".into()),
            effort: Some("high".into()),
            access: Some(Access::AutoAcceptEdits),
            plan: Some(true),
        })
    );
    let a = client_round_trip(r#"{"type":"thread_action","id":"2","thread_id":"t","action":{"kind":"rename","title":"Login limits"}}"#);
    assert_eq!(a.msg, ClientMessage::ThreadAction(ThreadActionRequest { thread_id: "t".into(), action: ThreadAction::Rename { title: "Login limits".into() } }));
    let pin: ClientEnvelope = serde_json::from_str(r#"{"type":"thread_action","thread_id":"t","action":{"kind":"pin"}}"#).unwrap();
    assert!(matches!(pin.msg, ClientMessage::ThreadAction(ThreadActionRequest { action: ThreadAction::Pin, .. })));
    let photo: ClientEnvelope = serde_json::from_str(r#"{"type":"send","thread_id":"t","text":"see","images":[{"mime":"image/png","data":"iVBORw0KGgo="}]}"#).unwrap();
    let ClientMessage::Send(req) = photo.msg else { panic!() };
    assert_eq!(req.images[0].decode().unwrap().1, "png");
    assert!(ImageUpload { mime: "image/png".into(), data: "aGVsbG8=".into() }.decode().is_err(), "not a picture");
    // Older Macs' rows and snapshots, without the new fields, still read.
    let s: Snapshot = serde_json::from_str(r#"{"threads":[],"projects":[],"agents":[{"key":"codex","name":"Codex","models":[{"id":"gpt-6","label":"GPT-6"}]}]}"#).unwrap();
    assert!(!s.full_access && s.agents[0].models[0].efforts.is_empty());
}

#[test]
fn spec_actions() {
    let send = client_round_trip(r#"{"type":"send","id":"9","thread_id":"01J…","text":"Also add a regression test","mode":"steer"}"#);
    assert_eq!(
        send.msg,
        ClientMessage::Send(SendRequest {
            thread_id: "01J…".into(),
            text: "Also add a regression test".into(),
            mode: Some(SendMode::Steer),
            images: vec![],
        })
    );
    let queued: ClientEnvelope = serde_json::from_str(r#"{"type":"send","thread_id":"t","text":"x","mode":"queue"}"#).unwrap();
    assert!(matches!(queued.msg, ClientMessage::Send(SendRequest { mode: Some(SendMode::Queue), .. })));
    let bare: ClientEnvelope = serde_json::from_str(r#"{"type":"send","thread_id":"t","text":"x"}"#).unwrap();
    assert!(matches!(bare.msg, ClientMessage::Send(SendRequest { mode: None, .. })));

    let new = client_round_trip(
        r#"{"type":"new_thread","id":"10","project_id":"p1","agent":"codex","model":"gpt-6","text":"Add rate limiting to /login","worktree":true}"#,
    );
    assert!(matches!(new.msg, ClientMessage::NewThread(NewThreadRequest { worktree: true, .. })));
    let new: ClientEnvelope =
        serde_json::from_str(r#"{"type":"new_thread","project_id":"p1","agent":"codex","model":null,"text":"x"}"#).unwrap();
    assert!(matches!(new.msg, ClientMessage::NewThread(NewThreadRequest { model: None, worktree: false, .. })));

    let a = client_round_trip(
        r#"{"type":"answer","id":"11","thread_id":"01J…","request_id":"r1","response":{"kind":"approval","decision":"allow"}}"#,
    );
    assert!(matches!(
        a.msg,
        ClientMessage::Answer(AnswerRequest { response: AnswerResponse::Approval { decision: Decision::Allow }, .. })
    ));
    let q = client_round_trip(
        r#"{"type":"answer","id":"12","thread_id":"01J…","request_id":"r2","response":{"kind":"questions","answers":[{"question":"Fix only the test, or the session code too?","answer":"Both"}]}}"#,
    );
    let ClientMessage::Answer(AnswerRequest { response: AnswerResponse::Questions { answers }, .. }) = q.msg else {
        panic!()
    };
    assert_eq!(answers[0].answer, "Both");
    let p = client_round_trip(
        r#"{"type":"answer","id":"13","thread_id":"01J…","request_id":"r3","response":{"kind":"plan","approve":false,"feedback":"Skip step 3"}}"#,
    );
    assert!(matches!(
        p.msg,
        ClientMessage::Answer(AnswerRequest { response: AnswerResponse::Plan { approve: false, feedback: Some(_) }, .. })
    ));
    for decision in ["allow", "allow_for_session", "deny"] {
        let d: Decision = serde_json::from_value(json!(decision)).unwrap();
        assert_eq!(serde_json::to_value(d).unwrap(), decision);
    }
    let plan: AnswerResponse = serde_json::from_str(r#"{"kind":"plan","approve":true}"#).unwrap();
    assert_eq!(plan, AnswerResponse::Plan { approve: true, feedback: None });

    client_round_trip(r#"{"type":"interrupt","id":"14","thread_id":"01J…"}"#);
    client_round_trip(r#"{"type":"mark_seen","thread_id":"01J…"}"#);
    let ping = client_round_trip(r#"{"type":"ping","id":"15"}"#);
    assert_eq!(ping.msg, ClientMessage::Ping);
    let ping: ClientEnvelope = serde_json::from_str(r#"{"type":"ping","id":"15","extra":{"a":1}}"#).unwrap();
    assert_eq!(ping.msg, ClientMessage::Ping);

    server_round_trip(r#"{"type":"ack","re":"9"}"#);
    let ack = server_round_trip(r#"{"type":"ack","re":"10","thread_id":"01J…"}"#);
    assert_eq!(ack.msg, ServerMessage::Ack { thread_id: Some("01J…".into()), open: None, text: None, project_id: None });
    server_round_trip(r#"{"type":"pong","re":"15"}"#);
    let err = server_round_trip(r#"{"type":"error","re":"9","code":"not_found","message":"No thread 01J…"}"#);
    assert_eq!(err.msg, ServerMessage::Error { code: ErrorCode::NotFound, message: "No thread 01J…".into() });
}

#[test]
fn exact_server_json() {
    assert_eq!(ServerEnvelope::reply(Some("15".into()), ServerMessage::Pong).to_json(), r#"{"type":"pong","re":"15"}"#);
    assert_eq!(
        ServerEnvelope::reply(Some("9".into()), ServerMessage::Ack { thread_id: None, open: None, text: None, project_id: None }).to_json(),
        r#"{"type":"ack","re":"9"}"#
    );
    assert_eq!(
        ServerEnvelope::push(ServerMessage::ThreadRemoved { thread_id: "t".into() }).to_json(),
        r#"{"type":"thread_removed","thread_id":"t"}"#
    );
}

#[test]
fn every_error_code() {
    for code in [
        "bad_request",
        "unauthorized",
        "pairing_failed",
        "rate_limited",
        "unsupported_protocol",
        "not_found",
        "conflict",
        "host_error",
        "clone_failed",
    ] {
        let c: ErrorCode = serde_json::from_value(json!(code)).unwrap();
        assert_eq!(serde_json::to_value(c).unwrap(), code);
        assert_eq!(c.as_str(), code);
    }
}

#[test]
fn unknown_client_types_fail_to_parse() {
    assert!(serde_json::from_str::<ClientEnvelope>(r#"{"type":"rm_rf","id":"1"}"#).is_err());
    assert!(serde_json::from_str::<ClientEnvelope>(r#"{"type":"send","id":"1"}"#).is_err());
}

#[test]
fn tool_kinds_from_titles() {
    let cases = [
        ("Subagent", ToolKind::Agent),
        ("Fetch", ToolKind::Web),
        ("Search the web", ToolKind::Web),
        ("Web search", ToolKind::Web),
        ("Run command", ToolKind::Command),
        ("Run", ToolKind::Command),
        ("Ran 3 commands", ToolKind::Command),
        ("Edit", ToolKind::Edit),
        ("Edited 2 files", ToolKind::Edit),
        ("Write", ToolKind::Edit),
        ("Wrote", ToolKind::Edit),
        ("Read", ToolKind::Read),
        ("Read 4 files", ToolKind::Read),
        ("Search", ToolKind::Search),
        ("Grep Search", ToolKind::Search),
        ("List files", ToolKind::Search),
        ("Fetching", ToolKind::Other),
        ("Todo", ToolKind::Other),
        ("", ToolKind::Other),
    ];
    for (title, kind) in cases {
        assert_eq!(ToolKind::from_title(title), kind, "{title:?}");
    }
}

#[test]
fn monograms() {
    assert_eq!(monogram("trek"), "TR");
    assert_eq!(monogram("trek-api"), "TA");
    assert_eq!(monogram("my cool project"), "MC");
    assert_eq!(monogram("--"), "·");
    assert_eq!(monogram(""), "·");
    assert_eq!(monogram("x"), "X");
    assert_eq!(monogram("élan vital"), "ÉV");
    assert_eq!(monogram("dotfiles.v2"), "DV");
}

#[test]
fn project_hues() {
    assert_eq!(project_hue("anything", Some(212)), 212);
    assert_eq!(project_hue("anything", Some(400)), 40);
    // djb2-xor over the name's bytes, mod 360 (the desktop's derivation).
    let expected = ("trek-api".bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32) % 360) as u16;
    assert_eq!(project_hue("trek-api", None), expected);
    assert_eq!(project_hue("", None), (5381 % 360) as u16);
    for name in ["a", "trek", "some long project name"] {
        assert!(project_hue(name, None) < 360);
        assert_eq!(project_hue(name, None), project_hue(name, None));
    }
}

#[test]
fn output_truncation_keeps_the_tail() {
    let short = "hello";
    assert_eq!(truncate_output(short), short);
    let long = format!("{}{}", "é".repeat(3000), "end");
    let cut = truncate_output(&long);
    assert!(cut.len() <= MAX_TOOL_OUTPUT);
    assert!(cut.ends_with("end"));
}

#[test]
fn thread_details_ride_on_the_row_and_older_rows_still_read() {
    let row = r#"{"id":"t","title":"Dark mode","project":{"id":"p","name":"web","hue":10,"monogram":"WE"},
        "agent":{"key":"claude-code","name":"Claude Code","logo":"claude-code"},"model":"claude-opus-5-5","model_label":"Opus 5.5",
        "run_state":"working","needs":null,"section":"working","unseen":false,"pinned":false,"branch":"trek/dark","worktree":true,
        "activity":"Editing theme.css","working_since":1791020400000,"updated_at":1791020400000,"additions":65,"deletions":8,
        "effort":"xhigh","access":"auto","effort_label":"Extra high",
        "context":{"used":171000,"window":200000,"percent":85},
        "cost":{"label":"≈ $1.84 at API prices","billing":"plan","plan":"Claude Max","detail":"Included in your Claude Max plan"},
        "sub_agents":[{"agent":{"key":"codex","name":"Codex","logo":"codex"},"model":"Sol","title":"Review the tokens","state":"running","since":1791020300000},
                      {"agent":{"key":"claude-code","name":"Claude Code"},"title":"Find hard-coded colours","state":"needs_you"}],
        "background":["pnpm dev"],"base":"main",
        "git":{"changed":5,"ahead":1,"behind":2,"default_branch":"main"}}"#;
    let t: ThreadSummary = serde_json::from_str(row).unwrap();
    assert_eq!(serde_json::to_value(&t).unwrap(), value(row));
    assert_eq!(t.context, Some(ContextUse { used: 171_000, window: 200_000, percent: 85 }));
    assert_eq!((t.sub_agents[1].state, t.sub_agents[1].since), (SubAgentState::NeedsYou, None));
    assert_eq!(t.cost.unwrap().billing, Some(Billing::Plan));
    // Without them: nothing extra on the wire.
    let plain = ThreadSummary { id: "t".into(), ..Default::default() };
    let v = serde_json::to_value(&plain).unwrap();
    for key in ["effort_label", "context", "cost", "sub_agents", "background", "base", "git"] {
        assert!(v.get(key).is_none(), "{key} should be skipped when empty");
    }
    assert!(v["agent"].get("logo").is_none());
}

#[test]
fn links_between_threads() {
    let item = item_round_trip(
        r#"{"id":"i7","seq":12,"at":null,"kind":"thread_link","link":"started","thread_id":"t2","title":"Fix the cache","agent":{"key":"codex","name":"Codex","logo":"codex"},"state":"needs_approval","open":true}"#,
    );
    let ItemBody::ThreadLink { link, thread_id, state, open, .. } = &item.body else { panic!("{:?}", item.body) };
    assert_eq!((*link, thread_id.as_str(), *state, *open), (ThreadLinkKind::Started, "t2", Some(LinkedThreadState::NeedsApproval), true));
    assert_eq!(item.body.kind(), "thread_link");
    // Where a thread came from: no state, and one archived since doesn't open.
    let item = item_round_trip(r#"{"id":"i0","seq":1,"at":null,"kind":"thread_link","link":"continued_from","thread_id":"t1","title":"Old work","open":false}"#);
    assert!(matches!(item.body, ItemBody::ThreadLink { link: ThreadLinkKind::ContinuedFrom, state: None, agent: None, open: false, .. }));
    let item: Item = serde_json::from_str(r#"{"id":"i0","seq":1,"kind":"thread_link","link":"started_by","thread_id":"t1","title":"x"}"#).unwrap();
    assert!(matches!(item.body, ItemBody::ThreadLink { open: true, .. }), "open unless it says otherwise");

    // Rows say where an agent-started thread came from, and leave it out when it didn't.
    let row = ThreadSummary { id: "t2".into(), started_by: Some("t1".into()), ..Default::default() };
    let v = serde_json::to_value(&row).unwrap();
    assert_eq!((&v["started_by"], v.get("continued_from")), (&json!("t1"), None));
    let back: ThreadSummary = serde_json::from_value(v).unwrap();
    assert_eq!(back, row);
}

#[test]
fn notes_pictures() {
    for line in [
        r#"{"type":"note_asset","id":"1","note_id":"0192abc","path":"attachments/3f2a.png","max_px":960}"#,
        r#"{"type":"add_note_image","id":"2","note_id":"0192abc","data":"/9j/","name":"Whiteboard"}"#,
    ] {
        let env = client_round_trip(line);
        assert_eq!(env.msg.kind(), value(line)["type"].as_str().unwrap());
    }
    assert!(client_round_trip(r#"{"type":"note_asset","note_id":"n","path":"attachments/a.png"}"#).msg.is_query());
    assert!(!client_round_trip(r#"{"type":"add_note_image","note_id":"n","data":""}"#).msg.is_query(), "it writes");
    server_round_trip(r#"{"type":"note_asset","re":"1","note_id":"0192abc","path":"attachments/3f2a.png","mime":"image/jpeg","data":"/9j/","width":960,"height":540}"#);
    server_round_trip(r#"{"type":"note_image","re":"2","note_id":"0192abc","path":"attachments/3f2a.jpg","markdown":"![Whiteboard](attachments/3f2a.jpg)"}"#);
}

#[test]
fn notes_files_in_pieces() {
    let line = r#"{"type":"note_file","id":"3","path":"attachments/9c1e.pdf","offset":1048576,"length":1048576}"#;
    let env = client_round_trip(line);
    assert_eq!(env.msg.kind(), "note_file");
    assert!(env.msg.is_query());
    let ClientMessage::NoteFile(req) = serde_json::from_str::<ClientEnvelope>(r#"{"type":"note_file","path":"attachments/a.mp4"}"#).unwrap().msg else { panic!() };
    assert_eq!((req.offset, req.length), (0, None), "from the start, a whole piece");
    let reply = r#"{"type":"note_file_chunk","re":"3","path":"attachments/9c1e.pdf","offset":1048576,"total":2500000,"bytes_b64":"JVBERg==","mime":"application/pdf"}"#;
    server_round_trip(reply);
    let ServerMessage::NoteFileChunk(c) = serde_json::from_str::<ServerEnvelope>(reply).unwrap().msg else { panic!() };
    assert_eq!(c.bytes().unwrap(), b"%PDF");
}

#[test]
fn a_turn_s_changed_files() {
    let item = item_round_trip(
        r#"{"id":"c12","seq":40,"at":null,"kind":"changes","files":[
            {"path":"README.md","status":"modified","added":31,"removed":54},
            {"path":"docs/quick-start.md","status":"renamed","from":"docs/getting-started.md","added":4,"removed":2},
            {"path":"docs/img/docker.png","status":"deleted","added":0,"removed":0,"binary":true},
            {"path":"src/new.rs","status":"added","added":12,"removed":0}],"added":47,"removed":56}"#,
    );
    let ItemBody::Changes { files, added, removed } = item.body else { panic!("{:?}", item.body) };
    assert_eq!((files.len(), added, removed), (4, 47, 56));
    assert_eq!(files[1].from.as_deref(), Some("docs/getting-started.md"));
    assert!(files[2].binary && files[2].status == FileStatus::Deleted);
    assert_eq!(ItemBody::Changes { files: vec![], added: 0, removed: 0 }.kind(), "changes");
}

#[test]
fn the_newer_requests() {
    let cases = [
        r#"{"type":"usage","id":"1"}"#,
        r#"{"type":"basecamp","id":"2","range":"all"}"#,
        r#"{"type":"notes","id":"3"}"#,
        r#"{"type":"note","id":"4","note_id":"0192abc"}"#,
        r#"{"type":"create_note","id":"5","body":"Groceries"}"#,
        r#"{"type":"save_note","id":"6","note_id":"0192abc","body":"Groceries\n- milk","modified":1791020400000}"#,
        r#"{"type":"delete_note","id":"7","note_id":"0192abc"}"#,
        r#"{"type":"git_status","id":"8","thread_id":"t"}"#,
        r#"{"type":"git_diff","id":"9","project_id":"p","path":"src/main.rs"}"#,
        r#"{"type":"git_commit","id":"10","thread_id":"t","message":"Fix the race"}"#,
        r#"{"type":"git_push","id":"11","thread_id":"t"}"#,
        r#"{"type":"git_branches","id":"12","project_id":"p"}"#,
        r#"{"type":"git_switch","id":"13","project_id":"p","branch":"develop"}"#,
        r#"{"type":"worktree_merge","id":"14","thread_id":"t"}"#,
        r#"{"type":"worktree_remove","id":"15","thread_id":"t","delete_branch":true,"force":true}"#,
        r#"{"type":"commands","id":"16","thread_id":"t"}"#,
        r#"{"type":"settings","id":"17"}"#,
        r#"{"type":"set_settings","id":"18","default_agent":"codex","default_access":"auto","follow_up":"queue","notifications":"banner_and_sound","push":true,"push_when":"always","new_push_topic":true,"auto_settle_days":7,"theme":"paper"}"#,
    ];
    for line in cases {
        let env = client_round_trip(line);
        assert_eq!(env.msg.kind(), value(line)["type"].as_str().unwrap());
    }
    // Defaults: today's recap; a removal that loses nothing.
    let b: ClientEnvelope = serde_json::from_str(r#"{"type":"basecamp"}"#).unwrap();
    assert_eq!(b.msg, ClientMessage::Basecamp { range: BasecampRange::Today });
    let r: ClientEnvelope = serde_json::from_str(r#"{"type":"worktree_remove","thread_id":"t"}"#).unwrap();
    assert_eq!(r.msg, ClientMessage::WorktreeRemove(WorktreeRemoveRequest { thread_id: "t".into(), delete_branch: false, force: false }));
    let d: ClientEnvelope = serde_json::from_str(r#"{"type":"git_diff","thread_id":"t","path":"a"}"#).unwrap();
    assert_eq!(d.msg, ClientMessage::GitDiff(GitDiffRequest { target: GitTarget::thread("t"), path: "a".into() }));
    assert!(d.msg.is_query() && !r.msg.is_query());
}

#[test]
fn the_newer_replies() {
    let cases = [
        r#"{"type":"ack","re":"1","open":{"screen":"new_thread","project_id":"p"}}"#,
        r#"{"type":"usage","re":"2","providers":[{"agent":{"key":"claude-code","name":"Claude Code","logo":"claude-code"},"plan":"Claude Max",
            "limits":[{"label":"5-hour limit","percent":42.5,"resets_at":1791030000000,"window":"5h"},{"label":"Weekly · Opus","percent":64.0}]}],"loading":true}"#,
        r#"{"type":"notes","re":"3","notes":[{"id":"n","title":"Groceries","preview":"milk bread","modified":1}]}"#,
        r#"{"type":"note","re":"4","note":{"id":"n","title":"Groceries","body":"Groceries\n- milk","modified":1}}"#,
        r#"{"type":"git_status","re":"5","is_repo":true,"branch":"trek/fix","default_branch":"main","ahead":2,"behind":0,"has_upstream":false,
            "files":[{"path":"a.rs","status":"untracked","added":9,"removed":0}],
            "worktree":{"branch":"trek/fix","base":"main","uncommitted":1,"merge_blocked":"1 file isn't committed yet. Commit or revert it first.","unmerged":2},
            "can_switch":false,"switch_blocked":"This thread works in a worktree."}"#,
        r#"{"type":"git_diff","re":"6","path":"a.rs","diff":"@@ -1 +1 @@\n-a\n+b","truncated":true}"#,
        r#"{"type":"git_branches","re":"7","current":"main","default_branch":"main","branches":["main","dev"]}"#,
        r#"{"type":"commands","re":"8","thread_id":"t","commands":[{"name":"permissions full","description":"No prompts and no sandbox","kind":"command","trek":true},{"name":"frontend-design","description":"","kind":"skill"}]}"#,
        r#"{"type":"settings","re":"9","default_agent":"claude-code","default_effort":"high","default_access":"auto-accept-edits","follow_up":"steer",
            "notifications":"banner","push":{"enabled":true,"when":"away","server":"https://ntfy.sh","topic":"trek-abc","topic_url":"https://ntfy.sh/trek-abc","subscribe_url":"ntfy://ntfy.sh/trek-abc"},
            "auto_settle_days":3,"theme":"system","full_access":false,"session_approvals":false}"#,
    ];
    for line in cases {
        server_round_trip(line);
    }
}

#[test]
fn a_basecamp_recap() {
    let line = r#"{"type":"basecamp","re":"1","range":"today","greeting":"Good evening, Monday 5 October","title":"Today's trek","updated_at":1791020400000,
        "review":[{"thread_id":"t","title":"Fix the flaky test","status":"needs_you","label":"Approval","agent":{"key":"codex","name":"Codex","logo":"codex"},
                   "project":{"id":"p","name":"api","hue":212,"monogram":"AP"},"additions":4,"deletions":1,"updated_at":1791020000000,"unseen":true}],
        "narrative":[{"kind":"text","text":"You sent "},{"kind":"strong","text":"18 prompts"},{"kind":"project","text":"api","project":{"id":"p","name":"api","hue":212,"monogram":"AP"}},
                     {"kind":"model","text":"GPT-6","agent":{"key":"codex","name":"Codex"}}],
        "summary":{"prompts":18,"threads":4,"turns":20,"agent_secs":3720,"agent_time":"1h 2m","tokens":182000,"failed":1,
                   "top_project":{"project":{"id":"p","name":"api","hue":212,"monogram":"AP"},"prompts":12,"tokens":90000},
                   "best_model":{"agent":{"key":"codex","name":"Codex"},"label":"GPT-6","tokens":140000,"turns":9,"share":77}},
        "profile":{"buckets":[{"value":0.0,"label":"12–1 AM","line":"12–1 AM · quiet","prompts":0,"agent_secs":0,"tokens":0},
                              {"value":12.5,"label":"2–3 PM","line":"2–3 PM · 4 prompts · 12m of agent time","prompts":4,"agent_secs":750,"tokens":3000}],
                   "summit":1,"now":1,"now_at":0.75,"line":"Summit at 2 PM","total":"18 prompts","ticks":[{"at":0.25,"label":"6 AM"}]},
        "tiles":[{"kind":"tokens","label":"You used","figure":"182K tokens","note":"≈ $1.28 at API prices today","sparkline":[0.0,0.5,1.0]},
                 {"kind":"plan_left","label":"Left on Claude Max","figure":"36%","note":"Weekly limit · resets in 4d","agent":{"key":"claude-code","name":"Claude Code"},"percent":36.0,"resets_at":1791030000000}]}"#;
    let env = server_round_trip(line);
    let ServerMessage::Basecamp(b) = env.msg else { panic!() };
    assert_eq!(b.profile.unwrap().summit, Some(1));
    assert_eq!(b.tiles[1].kind, TileKind::PlanLeft);
    assert!(!b.empty);
    // An older Mac's: none of `basecamp_v2`.
    assert!(b.totals.is_none() && b.chart.is_none() && b.projects.is_empty() && !b.review[0].needs_you);
}

#[test]
fn basecamp_as_the_mac_draws_it_now() {
    let line = r#"{"type":"basecamp","re":"1","range":"today","greeting":"Good evening, Monday 5 October","title":"Today's trek","updated_at":1791020400000,
        "review":[{"thread_id":"t","title":"Fix the flaky test","status":"needs_you","label":"Approval","agent":{"key":"codex","name":"Codex"},"additions":0,"deletions":0,"updated_at":1791020000000,"unseen":false,"needs_you":true},
                  {"thread_id":"u","title":"Bump deps","status":"paused","label":"Paused until 3 PM","agent":{"key":"codex","name":"Codex"},"additions":0,"deletions":0,"updated_at":1791020000000,"unseen":true}],
        "narrative":[],"tiles":[],
        "totals":{"threads":4,"turns":20,"tokens":182000,"tokens_label":"182K","cost":"$1.28","failed":1,"agent_secs":3720,"agent_time":"1h 2m"},
        "chart":{"step":"hour","bars":[{"label":"12–1 AM","turns":0,"tokens":0,"line":"12–1 AM · 0 turns"},{"label":"2–3 PM","turns":4,"tokens":12000,"line":"2–3 PM · 4 turns · 12K tokens"}],
                 "ticks":[{"bar":0,"label":"12 AM"}],"turns":4,"total":"4 turns"},
        "projects":[{"label":"api","project":{"id":"p","name":"api","hue":212,"monogram":"AP"},"threads":3,"turns":16,"tokens":150000,"tokens_label":"150K","cost":"$1.02","failed":1},
                    {"label":"No project","threads":1,"turns":4,"tokens":32000,"tokens_label":"32K","cost":"—","failed":0}],
        "models":[{"label":"GPT-6","agent":{"key":"codex","name":"Codex"},"threads":4,"turns":20,"tokens":182000,"tokens_label":"182K","cost":"$1.28","failed":1}]}"#;
    let env = server_round_trip(line);
    let ServerMessage::Basecamp(b) = env.msg else { panic!() };
    assert_eq!((b.review[0].needs_you, b.review[1].needs_you), (true, false));
    assert_eq!(b.totals.unwrap().cost, "$1.28");
    let chart = b.chart.unwrap();
    assert_eq!((chart.step.as_str(), chart.bars[1].turns, chart.ticks[0].bar), ("hour", 4, 0));
    assert_eq!((b.projects.len(), b.projects[1].project.is_none(), b.projects[1].failed), (2, true, 0));
    assert_eq!(b.models[0].agent.as_ref().unwrap().key, "codex");
    // Old fields the Mac now leaves empty still read.
    assert!(b.narrative.is_empty() && b.tiles.is_empty());
}

#[test]
fn ntfy_links_for_a_topic() {
    assert_eq!(ntfy_links("https://ntfy.sh", "trek-abc"), (Some("https://ntfy.sh/trek-abc".into()), Some("ntfy://ntfy.sh/trek-abc".into())));
    assert_eq!(ntfy_links("http://pi.local:8080/", "t"), (Some("http://pi.local:8080/t".into()), Some("ntfy://pi.local:8080/t?secure=false".into())));
    assert_eq!(ntfy_links("https://ntfy.sh", ""), (None, None));
}

#[test]
fn features_sub_agents_the_thread_menu_and_search() {
    // The Mac's list of what it serves; an older Mac's welcome (none) still reads.
    let welcome = server_round_trip(r#"{"type":"welcome","re":"1","protocol":1,"host":{"id":"h","name":"Mac","version":"0.4.0"},"features":["search","sub_agents"]}"#);
    assert!(matches!(welcome.msg, ServerMessage::Welcome { ref features, .. } if features == &["search", "sub_agents"]));
    let old: ServerEnvelope = serde_json::from_str(r#"{"type":"paired","re":"1","protocol":1,"token":"t","host":{"id":"h","name":"Mac","version":"0.3.2"}}"#).unwrap();
    assert!(matches!(old.msg, ServerMessage::Paired { ref features, .. } if features.is_empty()));

    // The thread menu's actions.
    for line in [
        r#"{"type":"thread_action","id":"1","thread_id":"t","action":{"kind":"snooze","preset":"three_hours"}}"#,
        r#"{"type":"thread_action","id":"2","thread_id":"t","action":{"kind":"snooze","until":1791100800000}}"#,
        r#"{"type":"thread_action","id":"3","thread_id":"t","action":{"kind":"wake"}}"#,
        r#"{"type":"thread_action","id":"4","thread_id":"t","action":{"kind":"mark_unread"}}"#,
        r#"{"type":"thread_action","id":"5","thread_id":"t","action":{"kind":"regenerate_title"}}"#,
        r#"{"type":"thread_action","id":"6","thread_id":"t","action":{"kind":"delete"}}"#,
        r#"{"type":"thread_action","id":"7","thread_id":"t","action":{"kind":"delete","confirmed":true}}"#,
        r#"{"type":"search","id":"8","query":"refresh race","limit":20}"#,
        r#"{"type":"search","id":"9","query":"x"}"#,
    ] {
        client_round_trip(line);
    }
    let snooze = client_round_trip(r#"{"type":"thread_action","thread_id":"t","action":{"kind":"snooze","preset":"next_week"}}"#);
    assert!(matches!(snooze.msg, ClientMessage::ThreadAction(ThreadActionRequest { action: ThreadAction::Snooze { until: None, preset: Some(SnoozePreset::NextWeek) }, .. })));
    let search = client_round_trip(r#"{"type":"search","query":"race"}"#);
    assert!(search.msg.is_query());
    assert_eq!(search.msg.kind(), "search");

    // Sub-agents: the row's thread, the parent's pointer to the one waiting, unlisted rows.
    let row = server_round_trip(
        r#"{"type":"unlisted","thread":{"id":"kid","title":"Benchmark it","project":{"id":"p","name":"trek","hue":24,"monogram":"TR"},"agent":{"key":"codex","name":"Codex"},"model":null,"model_label":null,"run_state":"needs-you","needs":{"kind":"approval","text":"Run cargo bench"},"section":"inbox","unseen":false,"pinned":false,"branch":null,"worktree":false,"activity":null,"working_since":null,"updated_at":5,"additions":0,"deletions":0,"parent_id":"t"}}"#,
    );
    assert!(matches!(row.msg, ServerMessage::Unlisted { ref thread } if thread.parent_id.as_deref() == Some("t")));
    let parent = ThreadSummary {
        id: "t".into(),
        needs: Some(Needs { kind: NeedsKind::Approval, text: "Sub-agent needs you".into() }),
        needs_thread: Some("kid".into()),
        snoozed_until: Some(9),
        sub_agents: vec![SubAgent { agent: AgentRef::new("codex", "Codex", None), model: Some("Sol".into()), title: "Benchmark it".into(), state: SubAgentState::NeedsYou, since: None, thread_id: Some("kid".into()) }],
        ..Default::default()
    };
    let v = serde_json::to_value(&parent).unwrap();
    assert_eq!((&v["needs_thread"], &v["snoozed_until"], &v["sub_agents"][0]["thread_id"]), (&json!("kid"), &json!(9), &json!("kid")));
    assert_eq!(v.get("parent_id"), None, "left out when there's nothing to say");
    let back: ThreadSummary = serde_json::from_value(v).unwrap();
    assert_eq!(back, parent);
    // An older Mac's snapshot (no unlisted rows) still reads.
    let snapshot: ServerEnvelope = serde_json::from_str(r#"{"type":"snapshot","threads":[],"projects":[],"agents":[]}"#).unwrap();
    assert!(matches!(snapshot.msg, ServerMessage::Snapshot(ref s) if s.unlisted.is_empty()));

    let results = server_round_trip(
        r#"{"type":"search_results","re":"8","query":"race","results":[{"thread":{"id":"t","title":"Fix the race","project":{"id":"p","name":"trek","hue":24,"monogram":"TR"},"agent":{"key":"codex","name":"Codex"},"model":null,"model_label":null,"run_state":"idle","needs":null,"section":"settled","unseen":false,"pinned":false,"branch":null,"worktree":false,"activity":null,"working_since":null,"updated_at":5,"additions":0,"deletions":0},"snippet":"two refreshes race","ranges":[[14,18]]}]}"#,
    );
    assert!(matches!(results.msg, ServerMessage::SearchResults(ref r) if r.results[0].ranges == [[14, 18]]));
}

#[test]
fn parity_requests_and_replies() {
    let requests = [
        r#"{"type":"use_reset","id":"1","credit_id":"rc-1"}"#,
        r#"{"type":"queue_action","id":"2","thread_id":"t","index":1,"text":"and open a PR","action":{"kind":"remove"}}"#,
        r#"{"type":"queue_action","id":"3","thread_id":"t","index":0,"reset":true,"text":"carry on","action":{"kind":"edit","text":"carry on, then stop"}}"#,
        r#"{"type":"attachment","id":"4","thread_id":"t","item_id":"i12","index":0,"max_px":480}"#,
        r#"{"type":"background_output","id":"5","thread_id":"t","task_id":"b1"}"#,
        r#"{"type":"stop_background","id":"6","thread_id":"t","task_id":"b1"}"#,
        r#"{"type":"icon","id":"7","agent":"acp:mistral-vibe"}"#,
        r#"{"type":"icon","id":"8","project_id":"p"}"#,
    ];
    for line in requests {
        let env = client_round_trip(line);
        assert_eq!(env.msg.kind(), value(line)["type"].as_str().unwrap());
    }
    let reads: Vec<bool> = requests.iter().map(|l| serde_json::from_str::<ClientEnvelope>(l).unwrap().msg.is_query()).collect();
    assert_eq!(reads, [false, false, false, true, true, false, true, true], "only reads may overtake");

    let replies = [
        r#"{"type":"attachment","re":"4","thread_id":"t","item_id":"i12","index":0,"mime":"image/jpeg","data":"/9j/","width":480,"height":160}"#,
        r#"{"type":"background_output","re":"5","thread_id":"t","task_id":"b1","output":"ready on :5173"}"#,
        r#"{"type":"background_output","re":"5","thread_id":"t","task_id":"b1"}"#,
        r#"{"type":"icon","re":"7","agent":"acp:mistral-vibe","hash":"0f3a","png":"iVBORw0K","template":true}"#,
        r#"{"type":"ack","re":"2","text":"and open a PR"}"#,
        r#"{"type":"welcome","re":"1","protocol":1,"host":{"id":"h","name":"Mac","version":"0.4.0"},"features":["search","sub_agents","snooze","mark_unread","delete","regenerate_title","use_reset","queue_action","attachment","background","icon","basecamp_v2","note_images","machines","note_files"]}"#,
        r#"{"type":"usage","re":"9","providers":[{"agent":{"key":"codex","name":"Codex","logo":"codex"},"plan":"ChatGPT Pro","limits":[],
            "resets":[{"id":"rc-1","title":"Full reset (Weekly + 5 hr)","expires_at":1791030000000}]},
            {"agent":{"key":"acp:devin","name":"Devin"},"limits":[],"no_limits":true,"as_of":1791020000000,"today":"Today in Trek: 12K tokens"}],"picked":true}"#,
    ];
    for line in replies {
        server_round_trip(line);
    }
    let welcome: ServerEnvelope = serde_json::from_str(replies[5]).unwrap();
    let ServerMessage::Welcome { features, .. } = welcome.msg else { panic!() };
    assert_eq!(features, feature::ALL, "a Mac that does them all says so");
}

#[test]
fn parity_fields_ride_on_rows_and_older_ones_still_read() {
    let row = r#"{"id":"t","title":"x","project":{"id":"p","name":"web","hue":10,"monogram":"WE","icon":"9c1e"},
        "agent":{"key":"acp:mistral-vibe","name":"Mistral Vibe","icon":"0f3a"},"run_state":"working","section":"working",
        "cost":{"label":"≈ $1.84 at API prices","cached":"92% cached","cached_detail":"157K of 171K prompt tokens came from the prompt cache."},
        "queued":[{"text":"then the docs"},{"text":"","images":2},{"text":"carry on","reset":true}],
        "background_tasks":[{"id":"b1","kind":"shell","title":"pnpm dev","line":"ready on :5173","since":1791020000000,"readable":true,"stoppable":true},
                            {"id":"b2","kind":"something_new","title":"x"}]}"#;
    let t: ThreadSummary = serde_json::from_str(row).unwrap();
    assert_eq!(t.project.icon.as_deref(), Some("9c1e"));
    assert_eq!(t.agent.icon.as_deref(), Some("0f3a"));
    assert_eq!(t.cost.as_ref().unwrap().cached.as_deref(), Some("92% cached"));
    assert_eq!((t.queued[1].images, t.queued[2].reset), (2, true));
    assert_eq!(t.background_tasks[1].kind, BackgroundKind::Other, "a kind this phone doesn't know");

    // Nothing new on the wire when there's nothing to say: older phones read what they did.
    let plain = serde_json::to_value(ThreadSummary { id: "t".into(), ..Default::default() }).unwrap();
    for key in ["queued", "background_tasks"] {
        assert!(plain.get(key).is_none(), "{key} should be skipped when empty");
    }
    assert!(plain["project"].get("icon").is_none() && plain["agent"].get("icon").is_none());
    let user = item_round_trip(r#"{"id":"i3","seq":4,"at":null,"kind":"user","text":"/model sonnet","images":0,"aside":true}"#);
    assert!(matches!(user.body, ItemBody::User { aside: true, .. }));
    let old = item_round_trip(r#"{"id":"i2","seq":3,"at":null,"kind":"user","text":"hi","images":0}"#);
    assert!(matches!(old.body, ItemBody::User { aside: false, .. }), "an older Mac's message is a prompt");
    let welcome: ServerEnvelope = serde_json::from_str(r#"{"type":"welcome","protocol":1,"host":{"id":"h","name":"Mac","version":"0.3"}}"#).unwrap();
    assert!(matches!(welcome.msg, ServerMessage::Welcome { ref features, .. } if features.is_empty()), "an older Mac lists nothing");
}
