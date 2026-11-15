//! A thread's sub-agents, replayed from Claude Code's own stream-json lines (synthetic, shaped as
//! Claude Code 2.1.312 sends them): a sub-agent launched in the background, its end, a message
//! that wakes it again (`SendMessage`), a second one, a failure, a stop, a thread loaded from the
//! store mid-run, a session started again since. After every line, the transcript's live
//! sub-agent rows are what the sidebar counts, the working bar lists and the phone shows.

mod common;

use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use serde_json::{Value, json};
use trek_agents::ClaudeReplay;
use trek_core::store::{Item, ToolStatus};
use trek_remote as tr;

fn init() -> Value {
    json!({"type":"system","subtype":"init","session_id":"sess-1","model":"claude-opus-5-5"})
}

fn say(text: &str) -> Value {
    json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":text}]}})
}

fn result() -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"result":"","duration_ms":1000})
}

/// The agent sends one out in the background (`Agent`, `run_in_background`).
fn launch(call: &str, description: &str) -> Value {
    json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":call,"name":"Agent","input":{"description":description,"prompt":format!("{description}, then report."),"run_in_background":true}}]}})
}

/// Its call returns at once.
fn launched(call: &str, agent: &str) -> Value {
    json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":call,"content":[{"type":"text","text":format!("Async agent launched successfully.\nagentId: {agent} (internal ID)\nThe agent is working in the background.")}]}]},"tool_use_result":{"isAsync":true,"status":"async_launched","agentId":agent}})
}

fn started(agent: &str, call: &str, description: &str) -> Value {
    json!({"type":"system","subtype":"task_started","task_id":agent,"tool_use_id":call,"description":description,"subagent_type":"general-purpose","is_backgrounded":true,"spawn_depth":1,"task_type":"local_agent"})
}

/// What runs in the background: the whole set, each time.
fn listed(agents: &[(&str, &str)]) -> Value {
    let tasks: Vec<Value> = agents
        .iter()
        .map(|(id, d)| json!({"task_id":id,"task_type":"local_agent","description":d}))
        .collect();
    json!({"type":"system","subtype":"background_tasks_changed","tasks":tasks})
}

/// A step the sub-agent takes, keyed by `call`.
fn step(call: &str, id: &str, command: &str) -> Value {
    json!({"type":"assistant","parent_tool_use_id":call,"message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}})
}

fn progress(agent: &str, call: &str, what: &str, n: u64) -> Value {
    json!({"type":"system","subtype":"task_progress","task_id":agent,"tool_use_id":call,"description":what,"usage":{"tool_uses":n}})
}

fn ended(agent: &str, call: &str, status: &str, summary: &str) -> Value {
    json!({"type":"system","subtype":"task_notification","task_id":agent,"tool_use_id":call,"status":status,"output_file":format!("/tmp/{agent}.output"),"summary":summary})
}

/// The agent writes to one of its sub-agents (`SendMessage`).
fn message(call: &str, agent: &str, text: &str) -> Value {
    json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":call,"name":"SendMessage","input":{"to":agent,"summary":"Another round","message":text}}]}})
}

/// The message woke it.
fn resumed(call: &str, agent: &str) -> Value {
    json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":call,"content":[{"type":"text","text":format!("{{\"success\":true,\"message\":\"Resuming agent {agent}\",\"resumedAgentId\":\"{agent}\"}}")}]}]}})
}

/// The message waits for it: it's still at work.
fn queued(call: &str, agent: &str) -> Value {
    json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":call,"content":[{"type":"text","text":format!("{{\"success\":true,\"message\":\"Message queued for delivery to {agent} at its next tool round.\"}}")}]}]}})
}

/// A thread fed Claude Code's lines one at a time, checked after each.
struct Replay<'a> {
    trek: &'a Trek,
    id: String,
    claude: ClaudeReplay,
    lines: usize,
}

impl<'a> Replay<'a> {
    fn new(trek: &'a Trek, cx: &mut TestAppContext) -> Self {
        let id = trek.quiet_thread(cx);
        Replay {
            trek,
            id,
            claude: ClaudeReplay::default(),
            lines: 0,
        }
    }

    fn feed(&mut self, cx: &mut TestAppContext, lines: impl IntoIterator<Item = Value>) {
        for line in lines {
            let events = self.claude.line(&line);
            let id = self.id.clone();
            self.trek
                .update(cx, |ws, cx| ws.apply_events(&id, events, cx));
            self.trek.render(cx);
            self.lines += 1;
            self.check(cx, &line);
        }
    }

    /// The transcript's live sub-agent rows, by the call each stands for.
    fn working_rows(&self, cx: &TestAppContext) -> Vec<String> {
        let view = self.trek.thread_view(cx);
        let mut rows = cx.read(|cx| view.read(cx).working_sub_agent_rows(cx));
        rows.sort();
        rows
    }

    /// The one list everything reads.
    fn at_work(&self, cx: &TestAppContext) -> Vec<String> {
        let mut rows: Vec<String> = self.trek.read(cx, |ws, _| {
            ws.sub_agents_at_work(&self.id)
                .into_iter()
                .map(|a| a.row)
                .collect()
        });
        rows.sort();
        rows
    }

    /// What the sidebar's badge counts.
    fn sidebar(&self, cx: &TestAppContext) -> usize {
        let t = self
            .trek
            .read(cx, |ws, _| ws.thread(&self.id).cloned().expect("thread"));
        cx.read(|cx| {
            self.trek
                .root
                .read(cx)
                .sidebar
                .read(cx)
                .at_work(&t, cx)
                .0
                .len()
        })
    }

    /// The invariant, for this prefix of the sequence.
    fn check(&self, cx: &TestAppContext, line: &Value) {
        let (rows, listed, sidebar) = (self.working_rows(cx), self.at_work(cx), self.sidebar(cx));
        assert_eq!(
            rows,
            listed,
            "line {} ({line}): the transcript shows {rows:?} at work, the list has {listed:?}\n{:#?}",
            self.lines,
            self.trek.rows(cx)
        );
        assert_eq!(
            sidebar,
            rows.len(),
            "line {} ({line}): the sidebar counts {sidebar}, the transcript shows {rows:?}",
            self.lines
        );
        // No row says Done while its sub-agent works.
        let at_work: Vec<String> = self.trek.read(cx, |ws, _| {
            ws.sub_agents_at_work(&self.id)
                .into_iter()
                .map(|a| a.name)
                .collect()
        });
        for row in self.trek.rows(cx) {
            if let Some(rest) = row.strip_prefix("subagent: ") {
                let working = at_work.iter().any(|n| rest.starts_with(n.as_str()));
                assert!(
                    !(working && rest.contains("(Done")),
                    "line {} ({line}): {row:?} says Done while it works",
                    self.lines
                );
            }
        }
    }

    /// The rows for sub-agents, as text.
    fn sub_rows(&self, cx: &TestAppContext) -> Vec<String> {
        self.trek
            .rows(cx)
            .into_iter()
            .filter(|r| r.starts_with("subagent: "))
            .collect()
    }

    fn row(&self, cx: &TestAppContext, call: &str) -> Item {
        self.trek
            .items(cx, &self.id)
            .into_iter()
            .rev()
            .find(|i| matches!(i, Item::Tool { id, .. } if id == call))
            .unwrap_or_else(|| panic!("no row {call}"))
    }
}

const VIDEO: &str = "Launch video research and production";
const THEMES: &str = "Trek themes: widgets and pages";

/// Launched in the background in one turn, finished between turns, answered in a turn of its own.
fn first_run(r: &mut Replay, cx: &mut TestAppContext) {
    r.feed(
        cx,
        [
            init(),
            say("I'll send the video research out."),
            launch("toolu_a", VIDEO),
            started("a382", "toolu_a", VIDEO),
            launched("toolu_a", "a382"),
            listed(&[("a382", VIDEO)]),
            say("It's working in the background."),
            result(),
            step("toolu_a", "toolu_k1", "yt-dlp ref1"),
            progress("a382", "toolu_a", "Downloading references", 4),
            listed(&[]),
            ended("a382", "toolu_a", "completed", "Round one is cut."),
            init(),
            say("Round one is in."),
            result(),
        ],
    );
}

/// The owner's case: a sub-agent that finished long ago, woken by a message mid-turn beside one
/// launched fresh in that turn. Both work in rows near the bottom; the old row says it works on
/// below; each ends on its own row, Done only on its notification.
#[test]
fn a_sub_agent_woken_by_a_message_works_on_a_row_where_it_was_woken() {
    run(async |cx| {
        let trek = open(cx);
        let mut r = Replay::new(&trek, cx);
        first_run(&mut r, cx);
        assert!(
            r.sub_rows(cx)[0].starts_with(&format!("subagent: {VIDEO} (Done")),
            "{:?}",
            r.sub_rows(cx)
        );
        r.feed(
            cx,
            [
                init(),
                say("Themes next, and another video round."),
                launch("toolu_b", THEMES),
                started("ae3e", "toolu_b", THEMES),
                launched("toolu_b", "ae3e"),
                listed(&[("ae3e", THEMES)]),
                message("toolu_s", "a382", "Cut round two from the six references."),
                started("a382", "toolu_s", VIDEO),
                listed(&[("ae3e", THEMES), ("a382", VIDEO)]),
                resumed("toolu_s", "a382"),
                say("Both are at work."),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_b", "toolu_s"]);
        assert_eq!(r.sidebar(cx), 2);
        // The woken one's row is the message's, at the bottom, and says so in the store too.
        assert_eq!(
            r.row(cx, "toolu_s"),
            Item::Tool {
                id: "toolu_s".into(),
                title: "Subagent".into(),
                detail: VIDEO.into(),
                output: r.row(cx, "toolu_s").output_of(),
                status: ToolStatus::Running,
            }
        );
        let rows = r.sub_rows(cx);
        assert!(
            rows[0].starts_with(&format!("subagent: {VIDEO} (Working again further down")),
            "{rows:?}"
        );
        assert_eq!(rows.len(), 3, "{rows:?}");
        // The working bar lists both.
        let header = trek.working_bar(cx);
        assert!(
            header
                .as_deref()
                .is_some_and(|h| h.contains("2 sub-agents")),
            "{header:?}"
        );
        // Its steps, still keyed by its first call, are its new row's.
        r.feed(
            cx,
            [
                step("toolu_a", "toolu_k2", "ffmpeg -i round2.mov"),
                progress("a382", "toolu_a", "Rendering round two", 9),
            ],
        );
        assert!(
            r.sub_rows(cx)
                .iter()
                .any(|row| row.contains("Rendering round two · 9 steps")),
            "{:?}",
            r.sub_rows(cx)
        );
        // The fresh one ends first, then the woken one, each on its own row.
        r.feed(
            cx,
            [
                listed(&[("a382", VIDEO)]),
                ended("ae3e", "toolu_b", "completed", "Themed."),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_s"]);
        r.feed(
            cx,
            [
                listed(&[]),
                ended("a382", "toolu_s", "completed", "Round two is cut."),
            ],
        );
        assert!(r.working_rows(cx).is_empty());
        let rows = r.sub_rows(cx);
        assert!(
            rows[0].starts_with(&format!(
                "subagent: {VIDEO} (Woken again further down · Done"
            )),
            "{rows:?}"
        );
        assert!(
            matches!(r.row(cx, "toolu_s"), Item::Tool { status: ToolStatus::Done, output, .. } if output == "Round two is cut.")
        );
    });
}

/// Woken a second time, it moves on again; a failure and a stop end it there, never Done.
#[test]
fn a_sub_agent_woken_twice_ends_on_its_latest_row_failed_or_stopped() {
    run(async |cx| {
        let trek = open(cx);
        let mut r = Replay::new(&trek, cx);
        first_run(&mut r, cx);
        r.feed(
            cx,
            [
                init(),
                message("toolu_s", "a382", "Round two."),
                started("a382", "toolu_s", VIDEO),
                resumed("toolu_s", "a382"),
                result(),
                ended("a382", "toolu_s", "completed", "Round two is cut."),
                init(),
                message("toolu_t", "a382", "Round three."),
                started("a382", "toolu_t", VIDEO),
                listed(&[("a382", VIDEO)]),
                resumed("toolu_t", "a382"),
                result(),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_t"]);
        let rows = r.sub_rows(cx);
        assert!(
            rows[..2]
                .iter()
                .all(|row| row.contains("Working again further down")),
            "both earlier runs point on: {rows:?}"
        );
        // A message to it while it works wakes nothing new.
        r.feed(
            cx,
            [
                init(),
                message("toolu_q", "a382", "Also this."),
                queued("toolu_q", "a382"),
                result(),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_t"]);
        r.feed(
            cx,
            [
                listed(&[]),
                ended("a382", "toolu_t", "failed", "The render broke."),
            ],
        );
        assert!(r.working_rows(cx).is_empty());
        assert!(matches!(
            r.row(cx, "toolu_t"),
            Item::Tool {
                status: ToolStatus::Failed,
                ..
            }
        ));
        // Woken once more, and stopped by the user.
        r.feed(
            cx,
            [
                init(),
                message("toolu_u", "a382", "Try again."),
                started("a382", "toolu_u", VIDEO),
                listed(&[("a382", VIDEO)]),
                resumed("toolu_u", "a382"),
                result(),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_u"]);
        r.feed(
            cx,
            [
                listed(&[]),
                ended("a382", "toolu_u", "killed", "Stopped by the user."),
            ],
        );
        assert!(r.working_rows(cx).is_empty());
        assert!(!r.sub_rows(cx).iter().any(|row| row.contains("(Running")));
    });
}

/// The phone lists the woken one, and opens it from its row: its message and its steps.
#[test]
fn the_phone_opens_a_woken_sub_agent_from_its_row() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let mut r = Replay::new(&trek, cx);
        first_run(&mut r, cx);
        r.feed(
            cx,
            [
                init(),
                launch("toolu_b", THEMES),
                started("ae3e", "toolu_b", THEMES),
                launched("toolu_b", "ae3e"),
                message("toolu_s", "a382", "Cut round two."),
                started("a382", "toolu_s", VIDEO),
                listed(&[("ae3e", THEMES), ("a382", VIDEO)]),
                resumed("toolu_s", "a382"),
                step("toolu_s", "toolu_k3", "ffmpeg -i round2.mov"),
            ],
        );
        let id = r.id.clone();
        trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        let listed: Vec<(String, Option<String>)> = trek.read(cx, |ws, _| {
            ws.remote.as_ref().unwrap().sent[&id]
                .sub_agents
                .iter()
                .map(|s| (s.title.clone(), s.call_id.clone()))
                .collect()
        });
        assert_eq!(
            listed,
            [
                (THEMES.to_string(), Some("toolu_b".to_string())),
                (VIDEO.to_string(), Some("toolu_s".to_string()))
            ]
        );
        // Its row carries its link, at work.
        let (reply, mut rx) = tokio::sync::oneshot::channel();
        trek.update(cx, |ws, cx| {
            ws.remote_request(
                tr::HostRequest::Transcript {
                    thread_id: id.clone(),
                    after_seq: None,
                    limit: None,
                    reply,
                },
                cx,
            )
        });
        let mut transcript = None;
        for _ in 0..400 {
            cx.run_until_parked();
            if let Ok(t) = rx.try_recv() {
                transcript = Some(t.expect("a transcript"));
                break;
            }
            cx.background_executor
                .timer(std::time::Duration::from_millis(5))
                .await;
        }
        let transcript = transcript.expect("answered");
        let link = transcript
            .items
            .iter()
            .find_map(|i| match &i.body {
                tr::ItemBody::Tool {
                    call_id,
                    sub_agent: Some(link),
                    ..
                } if call_id == "toolu_s" => Some(link.clone()),
                _ => None,
            })
            .expect("the woken one's row is a sub-agent's");
        assert_eq!(
            (link.label.as_str(), link.state),
            (VIDEO, tr::SubAgentState::Running)
        );
        // Opened: what woke it, and what it has done since.
        let (reply, mut rx) = tokio::sync::oneshot::channel();
        trek.update(cx, |ws, cx| {
            ws.remote_request(
                tr::HostRequest::SubAgent {
                    req: tr::SubAgentRequest {
                        thread_id: id.clone(),
                        call_id: "toolu_s".into(),
                    },
                    reply,
                },
                cx,
            )
        });
        cx.run_until_parked();
        let view = rx.try_recv().expect("answered").expect("it opens");
        assert_eq!(view.prompt.as_deref(), Some("Cut round two."));
        assert!(
            view.items.iter().any(|i| matches!(&i.body, tr::ItemBody::Tool { detail, .. } if detail.contains("round2.mov"))),
            "{:?}",
            view.items
        );
        assert!(view.can_stop || view.no_message.is_some() || !view.gone);
    });
}

/// Loaded again from the store while it works, the thread still shows it at work on its row; once
/// its session is gone (Trek quit and started again), it isn't at work anywhere.
#[test]
fn a_thread_loaded_again_mid_run_keeps_its_woken_sub_agent_at_work() {
    run(async |cx| {
        let trek = open(cx);
        let mut r = Replay::new(&trek, cx);
        first_run(&mut r, cx);
        r.feed(
            cx,
            [
                init(),
                message("toolu_s", "a382", "Round two."),
                started("a382", "toolu_s", VIDEO),
                listed(&[("a382", VIDEO)]),
                resumed("toolu_s", "a382"),
                result(),
            ],
        );
        let id = r.id.clone();
        trek.update(cx, |ws, cx| ws.persist_now(&id, cx));
        // Its live state goes; its session's next report of what runs comes before the
        // transcript is read back.
        trek.update(cx, |ws, cx| {
            ws.live.remove(&id);
            ws.apply_events(
                &id,
                vec![trek_agents::AgentEvent::Background(vec![
                    trek_agents::BackgroundTask {
                        id: "a382".into(),
                        kind: trek_agents::BackgroundKind::Agent,
                        title: VIDEO.into(),
                        call: Some("toolu_s".into()),
                        readable: false,
                        stoppable: true,
                    },
                ])],
                cx,
            );
            ws.live.get_mut(&id).unwrap().loaded = false;
            ws.ensure_loaded(&id, cx);
        });
        trek.render(cx);
        r.check(cx, &json!("loaded again"));
        assert_eq!(r.working_rows(cx), ["toolu_s"]);
        // Trek started again: the session went with Trek, and the sub-agent with it.
        trek.update(cx, |ws, cx| {
            ws.live.remove(&id);
            ws.ensure_loaded(&id, cx);
        });
        trek.render(cx);
        r.check(cx, &json!("Trek started again"));
        assert!(r.working_rows(cx).is_empty());
        assert!(!r.sub_rows(cx).iter().any(|row| row.contains("(Running")));
    });
}

/// A session started again since (Trek reaps idle ones, and resumes them on the next message)
/// doesn't know the sub-agent's earlier call: the woken run still gets its row, and the earlier
/// row it continues points there.
#[test]
fn a_sub_agent_woken_in_a_session_started_since_still_finds_its_earlier_row() {
    run(async |cx| {
        let trek = open(cx);
        let mut r = Replay::new(&trek, cx);
        first_run(&mut r, cx);
        r.claude = ClaudeReplay::default();
        r.feed(
            cx,
            [
                init(),
                message("toolu_r", "a382", "Round two."),
                started("a382", "toolu_r", VIDEO),
                listed(&[("a382", VIDEO)]),
                resumed("toolu_r", "a382"),
                result(),
            ],
        );
        assert_eq!(r.working_rows(cx), ["toolu_r"]);
        let rows = r.sub_rows(cx);
        assert!(rows[0].contains("Working again further down"), "{rows:?}");
    });
}

trait Output {
    fn output_of(&self) -> String;
}

impl Output for Item {
    fn output_of(&self) -> String {
        match self {
            Item::Tool { output, .. } => output.clone(),
            _ => String::new(),
        }
    }
}
