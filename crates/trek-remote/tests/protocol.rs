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
        project: ProjectRef { id: "p1".into(), name: "trek-api".into(), hue: 212, monogram: "TA".into() },
        agent: AgentRef { key: "claude-code".into(), name: "Claude Code".into() },
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
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "01J…".into(), after_seq: None });
    let sub: ClientEnvelope = serde_json::from_str(r#"{"type":"subscribe","thread_id":"t","after_seq":5}"#).unwrap();
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "t".into(), after_seq: Some(5) });
    let sub: ClientEnvelope = serde_json::from_str(r#"{"type":"subscribe","thread_id":"t"}"#).unwrap();
    assert_eq!(sub.msg, ClientMessage::Subscribe { thread_id: "t".into(), after_seq: None });
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
    bodies.push(ItemBody::User { text: "u".into(), images: 2 });
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
fn spec_actions() {
    let send = client_round_trip(r#"{"type":"send","id":"9","thread_id":"01J…","text":"Also add a regression test","mode":"steer"}"#);
    assert_eq!(
        send.msg,
        ClientMessage::Send(SendRequest {
            thread_id: "01J…".into(),
            text: "Also add a regression test".into(),
            mode: Some(SendMode::Steer)
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
    assert_eq!(ack.msg, ServerMessage::Ack { thread_id: Some("01J…".into()) });
    server_round_trip(r#"{"type":"pong","re":"15"}"#);
    let err = server_round_trip(r#"{"type":"error","re":"9","code":"not_found","message":"No thread 01J…"}"#);
    assert_eq!(err.msg, ServerMessage::Error { code: ErrorCode::NotFound, message: "No thread 01J…".into() });
}

#[test]
fn exact_server_json() {
    assert_eq!(ServerEnvelope::reply(Some("15".into()), ServerMessage::Pong).to_json(), r#"{"type":"pong","re":"15"}"#);
    assert_eq!(
        ServerEnvelope::reply(Some("9".into()), ServerMessage::Ack { thread_id: None }).to_json(),
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
