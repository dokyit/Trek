//! Permission rules against the real agents' CLIs: what each one really does at each access level,
//! through Trek's own session code. What's on the Settings page about where rules reach is what
//! these tests saw.
//!
//! No model is involved and nothing leaves this computer. A stand-in here answers each CLI's
//! requests with a scripted tool call; the CLIs get empty config folders, and their proxy settings
//! point at a dead port for anything that isn't the stand-in.
//!
//! They need `claude` and `codex` on the PATH, so they're ignored by default:
//!
//! ```sh
//! cargo test -p trek-agents --test rules_live -- --ignored --test-threads=1
//! ```

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use trek_agents::{AgentEvent, Command, Decision, SessionConfig};
use trek_core::permission_rules::{self, PermissionRule, RuleAction, RuleKind};
use trek_core::{AgentId, Effort, HandHolding};

/// Rules and the agents' environment are the process's: one test at a time.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
enum Wire {
    Anthropic,
    Responses,
    Chat,
}

/// What the stand-in has the model do, in order: each request that isn't the follow-up to a tool
/// result takes the next one, and answers in plain text once they run out.
#[derive(Clone)]
enum Step {
    /// A tool call, with its arguments.
    Tool(&'static str, Value),
    /// A shell command, however this wire's CLI wants it.
    Shell(Value),
    /// A file patch (Codex's `apply_patch`).
    Patch(String),
}

struct StandIn {
    port: u16,
    /// Each request's tool results, for what the agent was told.
    results: Arc<Mutex<Vec<String>>>,
    /// Each request as it came.
    bodies: Arc<Mutex<Vec<String>>>,
}

fn sse(events: &[Value]) -> Vec<u8> {
    let mut out = String::new();
    for e in events {
        out.push_str(&format!(
            "event: {}\ndata: {e}\n\n",
            e["type"].as_str().unwrap_or("message")
        ));
    }
    out.into_bytes()
}

fn anthropic(step: Option<Step>) -> Vec<u8> {
    let start = json!({ "type": "message_start", "message": { "id": "msg_1", "type": "message", "role": "assistant",
        "model": "stand-in", "content": [], "stop_reason": null, "stop_sequence": null,
        "usage": { "input_tokens": 10, "output_tokens": 1 } } });
    let (block, delta, stop) = match step {
        Some(Step::Tool(name, input)) => (
            json!({ "type": "tool_use", "id": format!("toolu_{name}"), "name": name, "input": {} }),
            json!({ "type": "input_json_delta", "partial_json": input.to_string() }),
            "tool_use",
        ),
        Some(Step::Shell(input)) => (
            json!({ "type": "tool_use", "id": "toolu_bash", "name": "Bash", "input": {} }),
            json!({ "type": "input_json_delta", "partial_json": input.to_string() }),
            "tool_use",
        ),
        _ => (
            json!({ "type": "text", "text": "" }),
            json!({ "type": "text_delta", "text": "All done." }),
            "end_turn",
        ),
    };
    sse(&[
        start,
        json!({ "type": "content_block_start", "index": 0, "content_block": block }),
        json!({ "type": "content_block_delta", "index": 0, "delta": delta }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": stop, "stop_sequence": null }, "usage": { "output_tokens": 5 } }),
        json!({ "type": "message_stop" }),
    ])
}

fn responses(step: Option<Step>, tools: &[Value]) -> Vec<u8> {
    let usage = json!({ "input_tokens": 10, "input_tokens_details": { "cached_tokens": 0 }, "output_tokens": 5,
        "output_tokens_details": { "reasoning_tokens": 0 }, "total_tokens": 15 });
    let item = match step {
        Some(Step::Shell(args)) => {
            json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1",
            "name": "exec_command", "arguments": args.to_string() })
        }
        Some(Step::Patch(patch)) => {
            let tool = tools.iter().find(|t| t["name"] == "apply_patch");
            if tool.is_some_and(|t| t["type"] == "custom") {
                json!({ "type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "apply_patch", "input": patch })
            } else {
                json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "apply_patch",
                    "arguments": json!({ "input": patch }).to_string() })
            }
        }
        Some(Step::Tool(name, args)) => {
            json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1",
            "name": name, "arguments": args.to_string() })
        }
        None => json!({ "type": "message", "id": "msg_1", "role": "assistant",
            "content": [{ "type": "output_text", "text": "All done." }] }),
    };
    sse(&[
        json!({ "type": "response.created", "response": { "id": "resp_1" } }),
        json!({ "type": "response.output_item.done", "item": item }),
        json!({ "type": "response.completed", "response": { "id": "resp_1", "usage": usage } }),
    ])
}

/// OpenAI's chat completions, for agents that speak that.
fn chat(step: Option<Step>) -> Vec<u8> {
    let mut out = String::new();
    let mut chunk = |delta: Value, finish: Value| {
        let c = json!({ "id": "c1", "object": "chat.completion.chunk", "model": "stand-in",
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
        out.push_str(&format!("data: {c}\n\n"));
    };
    let call = match step {
        Some(Step::Tool(name, args)) => Some((name, args)),
        Some(Step::Shell(args)) => Some(("bash", args)),
        _ => None,
    };
    match call {
        Some((name, args)) => {
            chunk(
                json!({ "role": "assistant", "tool_calls": [{ "index": 0, "id": "call_1", "type": "function",
                    "function": { "name": name, "arguments": args.to_string() } }] }),
                Value::Null,
            );
            chunk(json!({}), json!("tool_calls"));
        }
        None => {
            chunk(
                json!({ "role": "assistant", "content": "All done." }),
                Value::Null,
            );
            chunk(json!({}), json!("stop"));
        }
    }
    out.push_str("data: [DONE]\n\n");
    out.into_bytes()
}

fn answer(
    stream: &mut TcpStream,
    wire: &Wire,
    steps: &Mutex<Vec<Step>>,
    results: &Mutex<Vec<String>>,
    bodies: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = header.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    bodies
        .lock()
        .unwrap()
        .push(String::from_utf8_lossy(&body).into_owned());
    let path = first
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let mut reply = |code: &str, kind: &str, body: Vec<u8>| -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {code}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )?;
        stream.write_all(&body)
    };
    if first.starts_with("GET") {
        return reply(
            "200 OK",
            "application/json",
            br#"{"data":[],"models":[]}"#.to_vec(),
        );
    }
    if path.contains("count_tokens") {
        return reply(
            "200 OK",
            "application/json",
            br#"{"input_tokens":10}"#.to_vec(),
        );
    }
    let (found, has_tools): (Vec<String>, bool) = match wire {
        Wire::Anthropic => (
            request["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["content"].as_array())
                .flatten()
                .filter(|b| b["type"] == "tool_result")
                .map(|b| b["content"].to_string())
                .collect(),
            request["tools"].as_array().is_some_and(|t| !t.is_empty()),
        ),
        Wire::Responses => (
            request["input"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| {
                    matches!(
                        i["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
                })
                .map(|i| i["output"].to_string())
                .collect(),
            true,
        ),
        Wire::Chat => (
            request["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["role"] == "tool")
                .map(|m| m["content"].to_string())
                .collect(),
            request["tools"].as_array().is_some_and(|t| !t.is_empty()),
        ),
    };
    results.lock().unwrap().extend(found.iter().cloned());
    let step = if has_tools && found.is_empty() {
        let mut steps = steps.lock().unwrap();
        (!steps.is_empty()).then(|| steps.remove(0))
    } else {
        None
    };
    let tools: Vec<Value> = request["tools"].as_array().cloned().unwrap_or_default();
    let body = match wire {
        Wire::Anthropic => anthropic(step),
        Wire::Responses => responses(step, &tools),
        Wire::Chat => chat(step),
    };
    reply("200 OK", "text/event-stream", body)
}

fn stand_in(wire: Wire, steps: Vec<Step>) -> StandIn {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let steps = Arc::new(Mutex::new(steps));
    let results = Arc::new(Mutex::new(vec![]));
    let bodies = Arc::new(Mutex::new(vec![]));
    let (seen, kept) = (results.clone(), bodies.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (wire, steps, seen, kept) =
                (wire.clone(), steps.clone(), seen.clone(), kept.clone());
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = answer(&mut stream, &wire, &steps, &seen, &kept);
            });
        }
    });
    StandIn {
        port,
        results,
        bodies,
    }
}

struct Run {
    events: Vec<AgentEvent>,
    told: Vec<String>,
    dir: PathBuf,
}

impl Run {
    fn cards(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::PermissionRequest { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .collect()
    }

    fn stopped_by_rule(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e, AgentEvent::Notice(n) if n.starts_with("Your rule stopped")))
    }

    fn has(&self, file: &str) -> bool {
        self.dir.join(file).exists()
    }
}

fn temp_dir(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("trek-rules-live-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Everything a CLI might reach beyond the stand-in goes to a port nothing listens on.
fn keep_it_here() {
    for (k, v) in [
        ("HTTP_PROXY", "http://127.0.0.1:1"),
        ("HTTPS_PROXY", "http://127.0.0.1:1"),
        ("ALL_PROXY", "http://127.0.0.1:1"),
        ("http_proxy", "http://127.0.0.1:1"),
        ("https_proxy", "http://127.0.0.1:1"),
        ("all_proxy", "http://127.0.0.1:1"),
        ("NO_PROXY", "127.0.0.1,localhost"),
        ("no_proxy", "127.0.0.1,localhost"),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("DISABLE_AUTOUPDATER", "1"),
        ("DISABLE_TELEMETRY", "1"),
        ("DISABLE_ERROR_REPORTING", "1"),
    ] {
        // One test at a time, and the only threads running are ones it started.
        unsafe { std::env::set_var(k, v) };
    }
}

fn set_env(k: &str, v: &str) {
    unsafe { std::env::set_var(k, v) };
}

/// Words typed while a card is up: what `Workspace::send_as` does with them is say no to the card
/// and send them straight after.
const REPLY: &str = "use-the-staging-script-instead";

async fn session(
    agent: AgentId,
    level: HandHolding,
    dir: &Path,
    answer_cards: Decision,
    standin: &StandIn,
) -> Run {
    session_with(agent, level, dir, answer_cards, false, standin).await
}

async fn session_with(
    agent: AgentId,
    level: HandHolding,
    dir: &Path,
    answer_cards: Decision,
    reply: bool,
    standin: &StandIn,
) -> Run {
    let handle = trek_agents::start(SessionConfig {
        agent,
        cwd: dir.to_path_buf(),
        model: None,
        effort: Effort::Low,
        hand_holding: level,
        plan: false,
        read_only: false,
        resume: None,
        resume_at: None,
        fork: false,
        recap: None,
        fast: None,
        mcp_servers: vec![],
        instructions: None,
        read_dirs: vec![],
        script: false,
        transcript: None,
    });
    handle
        .commands
        .send(Command::Prompt {
            text: "Do the thing.".into(),
            images: vec![],
        })
        .await
        .unwrap();
    let mut events = vec![];
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(90), handle.events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; saw {events:#?}"))
            .unwrap_or_else(|_| panic!("session ended; saw {events:#?}"));
        if let AgentEvent::PermissionRequest { request_id, .. } = &ev {
            handle
                .commands
                .send(Command::Respond {
                    request_id: request_id.clone(),
                    decision: if reply { Decision::Deny } else { answer_cards },
                })
                .await
                .unwrap();
            if reply {
                handle
                    .commands
                    .send(Command::Prompt {
                        text: REPLY.into(),
                        images: vec![],
                    })
                    .await
                    .unwrap();
            }
        }
        let over = matches!(
            ev,
            AgentEvent::TurnComplete { .. } | AgentEvent::Exited | AgentEvent::Error(_)
        );
        events.push(ev);
        // A reply may start a turn of its own once this one has ended: wait for it to be heard.
        let heard = !reply
            || !events
                .iter()
                .any(|e| matches!(e, AgentEvent::PermissionRequest { .. }))
            || standin
                .bodies
                .lock()
                .unwrap()
                .iter()
                .any(|b| b.contains(REPLY));
        if over && heard {
            break;
        }
    }
    let _ = handle.commands.send(Command::Shutdown).await;
    let told = standin.results.lock().unwrap().clone();
    Run {
        events,
        told,
        dir: dir.to_path_buf(),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A Claude Code session, with `steps` from the stand-in and `rules` in force.
fn claude(
    level: HandHolding,
    steps: Vec<Step>,
    rules: &[PermissionRule],
    answer_cards: Decision,
    settings: Option<&str>,
) -> Run {
    keep_it_here();
    permission_rules::set_current(rules);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Anthropic, steps);
    let (dir, config) = (temp_dir("proj"), temp_dir("cfg"));
    std::fs::write(dir.join("victim"), "still here").unwrap();
    if let Some(settings) = settings {
        std::fs::write(config.join("settings.json"), settings).unwrap();
    }
    set_env(
        "ANTHROPIC_BASE_URL",
        &format!("http://127.0.0.1:{}", standin.port),
    );
    set_env("ANTHROPIC_API_KEY", "stand-in");
    set_env("CLAUDE_CONFIG_DIR", config.to_str().unwrap());
    runtime().block_on(session(
        AgentId::ClaudeCode,
        level,
        &dir,
        answer_cards,
        &standin,
    ))
}

fn rm_victim(dir_note: &str) -> Step {
    Step::Shell(json!({ "command": "rm victim", "description": dir_note }))
}

fn command_rule(pattern: &str, action: RuleAction) -> PermissionRule {
    PermissionRule::new(RuleKind::Command, pattern, action)
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_with_no_rule_at_full_access_runs_it_unasked() {
    let _one = one_at_a_time();
    let run = claude(
        HandHolding::FullAccess,
        vec![rm_victim("List files")],
        &[],
        Decision::Deny,
        None,
    );
    assert!(
        !run.has("victim"),
        "the baseline: it ran: {:#?}",
        run.events
    );
    assert!(run.cards().is_empty());
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_obeys_a_deny_rule_at_every_level() {
    let _one = one_at_a_time();
    for level in HandHolding::ALL {
        let run = claude(
            level,
            vec![rm_victim("List files")],
            &[command_rule("rm", RuleAction::Deny)],
            Decision::Allow,
            None,
        );
        assert!(run.has("victim"), "{level:?}: {:#?}", run.events);
        assert!(run.stopped_by_rule(), "{level:?}: {:#?}", run.events);
        assert!(
            run.cards().is_empty(),
            "{level:?}: no card: {:?}",
            run.cards()
        );
        assert!(
            run.told.iter().any(|t| t.contains("Deny command `rm`")),
            "{level:?}: the agent was told why: {:?}",
            run.told
        );
    }
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_shows_a_card_at_full_access_for_an_ask_rule() {
    let _one = one_at_a_time();
    let run = claude(
        HandHolding::FullAccess,
        vec![rm_victim("List files")],
        &[command_rule("rm", RuleAction::Ask)],
        Decision::Deny,
        None,
    );
    assert_eq!(
        run.cards(),
        ["rm victim"],
        "the command, not its description"
    );
    assert!(run.has("victim"), "the card's no held");
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_runs_an_allowed_command_without_a_card_at_supervised() {
    let _one = one_at_a_time();
    let run = claude(
        HandHolding::Supervised,
        vec![rm_victim("List files")],
        &[command_rule("rm", RuleAction::Allow)],
        Decision::Deny,
        None,
    );
    assert!(run.cards().is_empty(), "{:?}", run.cards());
    assert!(!run.has("victim"), "{:#?}", run.events);
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_obeys_a_file_rule_where_it_accepts_edits_by_itself() {
    let _one = one_at_a_time();
    for level in [HandHolding::AutoAcceptEdits, HandHolding::FullAccess] {
        let run = claude(
            level,
            vec![Step::Tool(
                "Write",
                json!({ "file_path": "new.lock", "content": "x" }),
            )],
            &[PermissionRule::new(
                RuleKind::Edit,
                "*.lock",
                RuleAction::Deny,
            )],
            Decision::Allow,
            None,
        );
        assert!(!run.has("new.lock"), "{level:?}: {:#?}", run.events);
        assert!(run.stopped_by_rule(), "{level:?}");
    }
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_obeys_a_deny_rule_inside_a_sub_agent() {
    let _one = one_at_a_time();
    let run = claude(
        HandHolding::FullAccess,
        vec![
            Step::Tool(
                "Agent",
                json!({ "description": "Clean up", "prompt": "Clean up.", "subagent_type": "general-purpose" }),
            ),
            rm_victim("List files"),
        ],
        &[command_rule("rm", RuleAction::Deny)],
        Decision::Allow,
        None,
    );
    assert!(run.has("victim"), "{:#?}", run.events);
    assert!(run.stopped_by_rule(), "{:#?}", run.events);
}

/// `disableAllHooks` in Claude Code's own settings turns off the hooks in its files. The rules'
/// hook comes over the control channel, and holds.
#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_obeys_a_deny_rule_with_its_hooks_disabled() {
    let _one = one_at_a_time();
    for level in [HandHolding::FullAccess, HandHolding::Supervised] {
        let run = claude(
            level,
            vec![rm_victim("List files")],
            &[command_rule("rm", RuleAction::Deny)],
            Decision::Allow,
            Some(r#"{ "disableAllHooks": true }"#),
        );
        assert!(run.has("victim"), "{level:?}: {:#?}", run.events);
        assert!(run.stopped_by_rule(), "{level:?}: {:#?}", run.events);
    }
}

/// A Codex session, with `steps` from the stand-in and `rules` in force.
fn codex(
    level: HandHolding,
    steps: Vec<Step>,
    rules: &[PermissionRule],
    answer_cards: Decision,
) -> Run {
    keep_it_here();
    permission_rules::set_current(rules);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Responses, steps);
    let (dir, home) = (temp_dir("proj"), temp_dir("home"));
    std::fs::write(dir.join("victim"), "still here").unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            r#"
model = "gpt-5.5"
model_provider = "standin"
check_for_update_on_startup = false
[analytics]
enabled = false
[model_providers.standin]
name = "stand-in"
base_url = "http://127.0.0.1:{}/v1"
wire_api = "responses"
env_key = "STANDIN_KEY"
"#,
            standin.port
        ),
    )
    .unwrap();
    set_env("CODEX_HOME", home.to_str().unwrap());
    set_env("STANDIN_KEY", "stand-in");
    runtime().block_on(session(AgentId::Codex, level, &dir, answer_cards, &standin))
}

fn shell(command: &str) -> Step {
    Step::Shell(json!({ "cmd": command }))
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_with_no_rule_runs_what_its_sandbox_allows_without_asking() {
    let _one = one_at_a_time();
    for level in [
        HandHolding::AutoAcceptEdits,
        HandHolding::Auto,
        HandHolding::FullAccess,
    ] {
        let run = codex(level, vec![shell("rm victim")], &[], Decision::Deny);
        assert!(
            !run.has("victim"),
            "{level:?}: the baseline: {:#?}",
            run.events
        );
        assert!(run.cards().is_empty(), "{level:?}: {:?}", run.cards());
    }
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_obeys_a_deny_rule_at_full_access() {
    let _one = one_at_a_time();
    let run = codex(
        HandHolding::FullAccess,
        vec![shell("rm victim")],
        &[command_rule("rm", RuleAction::Deny)],
        Decision::Allow,
    );
    assert!(run.has("victim"), "{:#?}", run.events);
    assert!(run.stopped_by_rule(), "{:#?}", run.events);
    assert!(run.cards().is_empty(), "{:?}", run.cards());
}

/// What the Settings page says about Codex's sandbox: Trek can't stop a command that doesn't ask,
/// and owns up to it.
#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_in_its_sandbox_runs_a_denied_command_first_and_trek_says_so() {
    let _one = one_at_a_time();
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![shell("rm victim")],
        &[command_rule("rm", RuleAction::Deny)],
        Decision::Allow,
    );
    assert!(!run.has("victim"), "it ran: {:#?}", run.events);
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("couldn't stop it"))),
        "{:#?}",
        run.events
    );
    assert!(run.cards().is_empty(), "{:?}", run.cards());
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnComplete { error: Some(_) })),
        "the turn was stopped: {:#?}",
        run.events
    );
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn a_rule_never_costs_codex_its_sandbox() {
    let _one = one_at_a_time();
    let rules = [command_rule("rm", RuleAction::Deny)];
    // What no rule names runs, unasked, in the project.
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![shell("touch made.txt")],
        &rules,
        Decision::Deny,
    );
    assert!(run.has("made.txt"), "{:#?}", run.events);
    assert!(run.cards().is_empty(), "{:?}", run.cards());
    // Out of the project is still refused by the system.
    let outside =
        trek_core::paths::home().join(format!(".trek-rules-live-outside-{}", std::process::id()));
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![shell(&format!("touch '{}'", outside.display()))],
        &rules,
        Decision::Deny,
    );
    let got_out = outside.exists();
    let _ = std::fs::remove_file(&outside);
    assert!(!got_out, "the sandbox went: {:#?}", run.events);
    // Read-only stays read-only.
    let run = codex(
        HandHolding::Supervised,
        vec![shell("touch made.txt")],
        &rules,
        Decision::Deny,
    );
    assert!(!run.has("made.txt"), "{:#?}", run.events);
    // Full access has none, as it should, even while Trek watches.
    let run = codex(
        HandHolding::FullAccess,
        vec![shell(&format!("touch '{}'", outside.display()))],
        &rules,
        Decision::Deny,
    );
    let got_out = outside.exists();
    let _ = std::fs::remove_file(&outside);
    assert!(got_out, "{:#?}", run.events);
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_asking_to_leave_its_sandbox_comes_by_the_rules() {
    let _one = one_at_a_time();
    let outside =
        trek_core::paths::home().join(format!(".trek-rules-live-outside-{}", std::process::id()));
    let asks = |command: String| {
        Step::Shell(json!({
            "cmd": command,
            "sandbox_permissions": "require_escalated",
            "justification": "I need to work outside the project",
        }))
    };
    // No rule names it: the user's card.
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![asks(format!("touch '{}'", outside.display()))],
        &[command_rule("rm", RuleAction::Deny)],
        Decision::Deny,
    );
    assert_eq!(run.cards().len(), 1, "{:#?}", run.events);
    assert!(!outside.exists(), "the card's no held");
    // A deny rule names it: no card, and it doesn't run.
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![asks("rm victim".into())],
        &[command_rule("rm", RuleAction::Deny)],
        Decision::Allow,
    );
    assert!(run.has("victim"), "{:#?}", run.events);
    assert!(run.stopped_by_rule(), "{:#?}", run.events);
    assert!(run.cards().is_empty(), "{:?}", run.cards());
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_shows_a_card_at_full_access_for_an_ask_rule() {
    let _one = one_at_a_time();
    let run = codex(
        HandHolding::FullAccess,
        vec![shell("rm victim")],
        &[command_rule("rm", RuleAction::Ask)],
        Decision::Deny,
    );
    assert_eq!(run.cards().len(), 1, "{:#?}", run.events);
    assert!(run.has("victim"));
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_obeys_a_file_rule_on_its_patches() {
    let _one = one_at_a_time();
    let rules = [PermissionRule::new(
        RuleKind::Edit,
        "*.lock",
        RuleAction::Deny,
    )];
    let patch = |name: &str| {
        Step::Patch(format!(
            "*** Begin Patch\n*** Add File: {name}\n+hi\n*** End Patch\n"
        ))
    };
    // At Full access, where Codex checks first: it doesn't happen.
    let run = codex(
        HandHolding::FullAccess,
        vec![patch("new.lock")],
        &rules,
        Decision::Allow,
    );
    assert!(!run.has("new.lock"), "{:#?}", run.events);
    assert!(run.stopped_by_rule(), "{:#?}", run.events);
    let run = codex(
        HandHolding::FullAccess,
        vec![patch("fine.txt")],
        &rules,
        Decision::Deny,
    );
    assert!(run.has("fine.txt"), "{:#?}", run.events);
    // In its sandbox it patches first, and Trek says so.
    let run = codex(
        HandHolding::AutoAcceptEdits,
        vec![patch("new.lock")],
        &rules,
        Decision::Allow,
    );
    assert!(run.has("new.lock"), "{:#?}", run.events);
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("couldn't stop it"))),
        "{:#?}",
        run.events
    );
}

/// OpenCode's environment, for a stand-in model.
fn opencode_env(standin: &StandIn) {
    for var in [
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        set_env(var, temp_dir("xdg").to_str().unwrap());
    }
    set_env("OPENCODE_DISABLE_AUTOUPDATE", "1");
    set_env("OPENCODE_DISABLE_LSP_DOWNLOAD", "1");
    set_env("OPENCODE_DISABLE_MODELS_FETCH", "1");
    set_env(
        "OPENCODE_CONFIG_CONTENT",
        &json!({
            "model": "standin/m",
            "provider": { "standin": {
                "npm": "@ai-sdk/openai-compatible",
                "options": { "baseURL": format!("http://127.0.0.1:{}/v1", standin.port), "apiKey": "stand-in" },
                "models": { "m": { "name": "m", "tool_call": true, "limit": { "context": 100000, "output": 1000 } } },
            } },
        })
        .to_string(),
    );
}

/// An OpenCode session, the ACP agent that can run with no account, with `steps` from the stand-in
/// and `rules` in force.
fn opencode(
    level: HandHolding,
    steps: Vec<Step>,
    rules: &[PermissionRule],
    answer_cards: Decision,
) -> Run {
    keep_it_here();
    permission_rules::set_current(rules);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Chat, steps);
    let dir = temp_dir("proj");
    std::fs::write(dir.join("victim"), "still here").unwrap();
    opencode_env(&standin);
    runtime().block_on(session(
        AgentId::OpenCode,
        level,
        &dir,
        answer_cards,
        &standin,
    ))
}

#[test]
#[ignore = "runs the real opencode CLI against a local stand-in"]
fn opencode_obeys_a_deny_rule_at_every_level() {
    let _one = one_at_a_time();
    for level in HandHolding::ALL {
        let run = opencode(
            level,
            vec![rm_victim("List files")],
            &[command_rule("rm", RuleAction::Deny)],
            Decision::Allow,
        );
        assert!(run.has("victim"), "{level:?}: {:#?}", run.events);
        assert!(run.stopped_by_rule(), "{level:?}: {:#?}", run.events);
        assert!(run.cards().is_empty(), "{level:?}: {:?}", run.cards());
    }
}

#[test]
#[ignore = "runs the real opencode CLI against a local stand-in"]
fn opencode_hears_a_typed_reply_after_the_refusal() {
    let _one = one_at_a_time();
    keep_it_here();
    permission_rules::set_current(&[command_rule("rm", RuleAction::Ask)]);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Chat, vec![rm_victim("List files")]);
    let dir = temp_dir("proj");
    std::fs::write(dir.join("victim"), "still here").unwrap();
    opencode_env(&standin);
    let run = runtime().block_on(session_with(
        AgentId::OpenCode,
        HandHolding::FullAccess,
        &dir,
        Decision::Deny,
        true,
        &standin,
    ));
    assert!(
        standin
            .bodies
            .lock()
            .unwrap()
            .iter()
            .any(|b| b.contains(REPLY)),
        "never reached the model: {:#?}",
        run.events
    );
    assert!(run.has("victim"), "the card's no held: {:#?}", run.events);
    assert_eq!(run.cards().len(), 1, "{:#?}", run.events);
}

#[test]
#[ignore = "runs the real opencode CLI against a local stand-in"]
fn opencode_with_no_rule_at_full_access_runs_it_because_trek_said_yes() {
    let _one = one_at_a_time();
    let run = opencode(
        HandHolding::FullAccess,
        vec![rm_victim("List files")],
        &[],
        Decision::Deny,
    );
    assert!(!run.has("victim"), "{:#?}", run.events);
    assert!(run.cards().is_empty(), "{:?}", run.cards());
}

/// The model hears both: that the card was refused, and why, in the user's words.
#[track_caller]
fn heard_the_reply(run: &Run, standin: &StandIn, refusal: &str) {
    let bodies = standin.bodies.lock().unwrap();
    let last = bodies.iter().rfind(|b| b.contains(REPLY));
    let last = last.unwrap_or_else(|| panic!("never reached the model: {:#?}", run.events));
    assert!(last.contains(refusal), "{refusal:?} not in {last}");
    assert!(run.has("victim"), "the card's no held: {:#?}", run.events);
    assert_eq!(run.cards().len(), 1, "{:#?}", run.events);
}

#[test]
#[ignore = "runs the real claude CLI against a local stand-in"]
fn claude_hears_a_typed_reply_with_the_refusal() {
    let _one = one_at_a_time();
    keep_it_here();
    permission_rules::set_current(&[]);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Anthropic, vec![rm_victim("List files")]);
    let (dir, config) = (temp_dir("proj"), temp_dir("cfg"));
    std::fs::write(dir.join("victim"), "still here").unwrap();
    set_env(
        "ANTHROPIC_BASE_URL",
        &format!("http://127.0.0.1:{}", standin.port),
    );
    set_env("ANTHROPIC_API_KEY", "stand-in");
    set_env("CLAUDE_CONFIG_DIR", config.to_str().unwrap());
    let run = runtime().block_on(session_with(
        AgentId::ClaudeCode,
        HandHolding::Supervised,
        &dir,
        Decision::Deny,
        true,
        &standin,
    ));
    heard_the_reply(&run, &standin, "The user declined this action.");
}

#[test]
#[ignore = "runs the real codex CLI against a local stand-in"]
fn codex_hears_a_typed_reply_with_the_refusal() {
    let _one = one_at_a_time();
    keep_it_here();
    // Full access, where a command waits for an answer before it runs.
    permission_rules::set_current(&[command_rule("rm", RuleAction::Ask)]);
    trek_core::paths::isolate(temp_dir("data"));
    let standin = stand_in(Wire::Responses, vec![shell("rm victim")]);
    let (dir, home) = (temp_dir("proj"), temp_dir("home"));
    std::fs::write(dir.join("victim"), "still here").unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "model = \"gpt-5.5\"\nmodel_provider = \"standin\"\ncheck_for_update_on_startup = false\n[analytics]\nenabled = false\n[model_providers.standin]\nname = \"stand-in\"\nbase_url = \"http://127.0.0.1:{}/v1\"\nwire_api = \"responses\"\nenv_key = \"STANDIN_KEY\"\n",
            standin.port
        ),
    )
    .unwrap();
    set_env("CODEX_HOME", home.to_str().unwrap());
    set_env("STANDIN_KEY", "stand-in");
    let run = runtime().block_on(session_with(
        AgentId::Codex,
        HandHolding::FullAccess,
        &dir,
        Decision::Deny,
        true,
        &standin,
    ));
    let bodies = standin.bodies.lock().unwrap();
    let last = bodies.iter().rfind(|b| b.contains(REPLY));
    assert!(last.is_some(), "never reached the model: {:#?}", run.events);
    assert!(run.has("victim"), "the card's no held: {:#?}", run.events);
    assert_eq!(run.cards().len(), 1, "{:#?}", run.events);
}
