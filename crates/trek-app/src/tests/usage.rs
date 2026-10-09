//! The sidebar's Usage card: up to three providers, picked on the card or in Settings (a fourth
//! is refused until one is unchecked), each agent's usage read on its own off the main thread
//! and again after a turn on it, only while the card shows it (the others' commands and models
//! are read without it), and what was read kept for the next launch.

use super::harness::{Trek, mock, open, run};
use crate::workspace::Route;
use crate::workspace::SettingsPage;
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::rc::Rc;
use trek_agents::{AgentStatus, CommandKind, SlashCommand, UsageLimit};
use trek_core::AgentId;
use trek_core::RunState;
use trek_core::catalog::ModelInfo;
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::settings::Settings;

fn devin() -> AgentId {
    AgentId::Acp("devin".into())
}

/// Claude Code and Codex installed and ready.
fn install(trek: &Trek, cx: &mut TestAppContext) {
    trek.update(cx, |ws, _| {
        for (agent, name) in [(AgentId::ClaudeCode, "Claude Code"), (AgentId::Codex, "Codex")] {
            ws.agents.push(DetectedAgent { agent, name: name.into(), path: None, version: None, availability: Availability::Ready, models: vec![], install_hint: None });
        }
    });
}

fn plan(name: &str, percent: f32, resets_at: Option<i64>) -> AgentStatus {
    AgentStatus { plan: Some(name.into()), logged_in: true, limits: vec![UsageLimit { label: "5-hour limit".into(), percent, resets_at, window: "5h".into() }], ..Default::default() }
}

fn row(agent: &AgentId) -> String {
    format!("usage-row-{}", agent.key())
}

fn pick(agent: &AgentId) -> String {
    format!("usage-pick-{}", agent.key())
}

fn shown(trek: &Trek, cx: &TestAppContext) -> Vec<AgentId> {
    trek.read(cx, |ws, _| ws.usage_shown())
}

#[test]
fn the_card_shows_up_to_three_providers_picked_on_it_and_refuses_a_fourth() {
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        let now = trek.read(cx, |ws, _| ws.now());
        trek.update(cx, |ws, _| {
            for (agent, name) in [(AgentId::ClaudeCode, "Claude Max"), (AgentId::Codex, "ChatGPT Plus"), (devin(), "Devin Pro")] {
                ws.agent_status.insert(agent.key(), plan(name, 40., Some(now + 3_600_000)));
            }
        });
        // Left to Trek: the first three with usage to show. The mock agent has none.
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, AgentId::Codex, devin()]);
        let all = trek.read(cx, |ws, _| ws.usage_providers());
        assert!(all.contains(&mock()) && all.len() > 3, "{all:?}");
        trek.click(cx, "usage");
        trek.render(cx);
        for a in [AgentId::ClaudeCode, AgentId::Codex, devin()] {
            assert!(trek.visible(cx, row(&a)), "{a:?}");
        }
        // "…" lists every provider, the ones shown checked.
        trek.click(cx, "usage-choose");
        trek.render(cx);
        assert!(trek.visible(cx, "usage-picker"));
        for a in &all {
            assert!(trek.visible(cx, pick(a)), "{a:?}");
        }
        assert!(!trek.visible(cx, row(&AgentId::ClaudeCode)), "the list stands in for the rows while picking");
        // Three are shown: a fourth is refused, with a word, and nothing changes.
        trek.click(cx, pick(&mock()));
        trek.render(cx);
        assert!(trek.visible(cx, "usage-pick-hint"));
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, AgentId::Codex, devin()]);
        assert_eq!(trek.read(cx, |ws, _| ws.settings.usage.shown.clone()), None, "still automatic");
        // One unchecked, the fourth goes in.
        trek.click(cx, pick(&AgentId::Codex));
        trek.click(cx, pick(&mock()));
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, mock(), devin()]);
        let picked = Some(vec![AgentId::ClaudeCode.key(), devin().key(), mock().key()]);
        assert_eq!(trek.read(cx, |ws, _| ws.settings.usage.shown.clone()), picked);
        assert_eq!(Settings::load().usage.shown, picked, "saved for the next launch");
        // Done: only the picked ones are drawn.
        trek.click(cx, "usage-choose");
        trek.render(cx);
        assert!(trek.visible(cx, row(&AgentId::ClaudeCode)) && trek.visible(cx, row(&mock())) && trek.visible(cx, row(&devin())));
        assert!(!trek.visible(cx, row(&AgentId::Codex)));
        // Back to automatic from the list.
        trek.click(cx, "usage-choose");
        trek.click(cx, "usage-pick-auto");
        assert_eq!(trek.read(cx, |ws, _| ws.settings.usage.shown.clone()), None);
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, AgentId::Codex, devin()]);
        // Settings › General picks them too (further down the page).
        trek.click(cx, "usage");
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx));
        trek.render(cx);
        assert!(trek.window(cx, |window, _| gpui_kit::test::TestWindowExt::try_find(window, gpui_kit::ElementId::from("usage-shown")).is_some()));
    });
}

#[test]
fn each_agent_is_read_on_its_own_off_the_main_thread_and_again_after_a_turn() {
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        // Asked about its usage, each agent answers when the test says.
        type Answer = async_channel::Sender<Result<AgentStatus, String>>;
        let asked: Rc<RefCell<Vec<(AgentId, Answer)>>> = Rc::default();
        let calls = asked.clone();
        trek.update(cx, |ws, _| {
            // The mock agent's usage is on the card too, so it's read after its turn.
            ws.settings.usage.shown = Some(vec![AgentId::ClaudeCode.key(), AgentId::Codex.key(), mock().key()]);
            ws.usage_fetch = Some(Rc::new(move |agent, _, _| {
                let (tx, rx) = async_channel::bounded(1);
                calls.borrow_mut().push((agent.clone(), tx));
                rx
            }))
        });
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        let answer = |agent: &AgentId, status: Result<AgentStatus, String>| {
            let tx = asked.borrow().iter().rev().find(|(a, _)| a == agent).map(|(_, tx)| tx.clone()).expect("asked");
            tx.try_send(status).unwrap();
        };
        // All asked at once, none answered yet: the main thread didn't wait on any.
        let who: Vec<AgentId> = asked.borrow().iter().map(|(a, _)| a.clone()).collect();
        assert!(who.contains(&AgentId::ClaudeCode) && who.contains(&AgentId::Codex), "{who:?}");
        assert!(trek.read(cx, |ws, _| ws.usage_loading && ws.agent_status.is_empty()));
        // Codex answers first: its numbers show while Claude Code's read goes on.
        answer(&AgentId::Codex, Ok(plan("ChatGPT Plus", 12., None)));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.agent_status.contains_key(&AgentId::Codex.key()) && !ws.agent_status.contains_key(&AgentId::ClaudeCode.key())));
        assert!(trek.read(cx, |ws, _| ws.usage_loading), "Claude Code is still being read");
        answer(&AgentId::ClaudeCode, Err("claude timed out".into()));
        // The test's own agents have nothing to say.
        for (agent, tx) in asked.borrow().iter() {
            if !matches!(agent, AgentId::ClaudeCode | AgentId::Codex) {
                tx.try_send(Ok(AgentStatus::default())).unwrap();
            }
        }
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| !ws.usage_loading));
        assert_eq!(trek.read(cx, |ws, _| ws.agent_status[&AgentId::ClaudeCode.key()].error.clone()), Some("claude timed out".into()));
        // Asked again within 30 seconds: nothing is read.
        let before = asked.borrow().len();
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        assert_eq!(asked.borrow().len(), before);
        // A turn ends: its agent's usage is read again, and today's tokens summed again.
        trek.update(cx, |ws, _| ws.clock = crate::workspace::Clock::new(|| trek_core::store::now_ms() + 60_000));
        let id = trek.send(cx, "explain the project");
        trek.wait_done(cx, &id, RunState::Idle).await;
        cx.run_until_parked();
        assert_eq!(asked.borrow()[before..].iter().map(|(a, _)| a.clone()).collect::<Vec<_>>(), [mock()], "only the agent that worked");
        let today = trek.read(cx, |ws, _| ws.usage_today.get(&mock().key()).map(|(n, _)| *n));
        assert!(today.is_some_and(|n| n > 0), "{today:?}");
    });
}

#[test]
fn what_was_read_is_kept_and_shown_at_the_next_launch_until_read_again() {
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        let now = trek.read(cx, |ws, _| ws.now());
        trek.update(cx, |ws, _| {
            ws.usage_fetch = Some(Rc::new(move |agent, _, _| {
                let (tx, rx) = async_channel::bounded(1);
                // The session window reset half an hour ago; the weekly one is a day off.
                let mut st = plan("Claude Max", 80., Some(now - 1_800_000));
                st.limits.push(UsageLimit { label: "Weekly limit".into(), percent: 30., resets_at: Some(now + 86_400_000), window: "7d".into() });
                let _ = tx.try_send(if *agent == AgentId::ClaudeCode { Ok(st) } else { Err("not this one".into()) });
                rx
            }))
        });
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        cx.run_until_parked();
        // Written off the main thread; the next launch reads it.
        let kept = crate::workspace::usage::load_snapshots();
        assert_eq!(kept.get(&AgentId::ClaudeCode.key()).and_then(|s| s.plan.clone()).as_deref(), Some("Claude Max"));
        assert!(!kept.contains_key(&AgentId::Codex.key()), "a failed read keeps nothing");
        // As at a launch, before any agent has answered: what was kept shows, marked as such,
        // and a window that has reset since shows empty.
        trek.update(cx, |ws, _| {
            ws.agent_status.clear();
            ws.usage_cached = crate::workspace::usage::load_snapshots();
        });
        let rows = trek.read(cx, |ws, _| ws.usage_rows());
        let claude = rows.iter().find(|r| r.agent == AgentId::ClaudeCode).expect("a row for Claude Code");
        assert!(claude.as_of.is_some());
        assert_eq!(claude.limits.iter().map(|l| (l.percent, l.resets_at.is_some())).collect::<Vec<_>>(), [(0., false), (30., true)]);
        trek.click(cx, "usage");
        trek.render(cx);
        assert!(trek.visible(cx, row(&AgentId::ClaudeCode)));
    });
}

/// What a test agent says when asked: its commands and models always, its limits only when its
/// usage is asked for.
fn report(agent: &AgentId, with_usage: bool) -> AgentStatus {
    let key = agent.key();
    let mut st = if with_usage { plan("Live plan", 12., None) } else { AgentStatus { logged_in: true, ..Default::default() } };
    st.commands = vec![SlashCommand { name: format!("{key}-skill"), description: String::new(), kind: CommandKind::Skill }];
    st.models = vec![ModelInfo { id: format!("{key}-model"), name: format!("{key} model"), efforts: vec![], tier: 0, fast: None }];
    st
}

type Asked = Rc<RefCell<Vec<(AgentId, bool, async_channel::Sender<Result<AgentStatus, String>>)>>>;

/// Every read asked for, with or without the usage, answered when the test says.
fn record(trek: &Trek, cx: &mut TestAppContext) -> Asked {
    let asked: Asked = Rc::default();
    let calls = asked.clone();
    trek.update(cx, |ws, _| {
        ws.usage_fetch = Some(Rc::new(move |agent, _, with_usage| {
            let (tx, rx) = async_channel::bounded(1);
            calls.borrow_mut().push((agent.clone(), with_usage, tx));
            rx
        }))
    });
    asked
}

/// The reads of `agent` so far: with its usage, and in all.
fn reads(asked: &Asked, agent: &AgentId) -> (usize, usize) {
    let all: Vec<bool> = asked.borrow().iter().filter(|(a, ..)| a == agent).map(|(_, u, _)| *u).collect();
    (all.iter().filter(|u| **u).count(), all.len())
}

/// Answer every read not answered yet.
fn answer_all(asked: &Asked, cx: &mut TestAppContext) {
    for (agent, with_usage, tx) in asked.borrow().iter() {
        let _ = tx.try_send(Ok(report(agent, *with_usage)));
    }
    cx.run_until_parked();
}

#[test]
fn only_the_providers_on_the_card_have_their_usage_read_the_others_commands_still_load() {
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        let now = trek.read(cx, |ws, _| ws.now());
        // Only Codex is on the card. Claude Code said its plan's usage at the last launch.
        trek.update(cx, |ws, _| {
            ws.settings.usage.shown = Some(vec![AgentId::Codex.key()]);
            let kept = UsageLimit { label: "5-hour limit".into(), percent: 55., resets_at: Some(now + 3_600_000), window: "5h".into() };
            ws.usage_cached.insert(AgentId::ClaudeCode.key(), crate::workspace::usage::Snapshot { read_at: now - 3_600_000, plan: Some("Claude Max".into()), limits: vec![kept], note: None });
        });
        let asked = record(&trek, cx);
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        answer_all(&asked, cx);
        assert_eq!(reads(&asked, &AgentId::Codex), (1, 1), "on the card: its usage is read");
        assert_eq!(reads(&asked, &AgentId::ClaudeCode), (0, 1), "off it: read without its usage");
        assert_eq!(reads(&asked, &mock()), (0, 1));
        // Claude Code's commands and models load all the same; nothing of its usage is taken in.
        let claude = AgentId::ClaudeCode;
        assert!(trek.read(cx, |ws, _| ws.slash_commands(&crate::workspace::Scope::Main, &claude).iter().any(|c| c.name == "claude-code-skill")));
        assert!(trek.read(cx, |ws, _| ws.models_for(&claude).iter().any(|m| m.id == "claude-code-model")));
        assert!(trek.read(cx, |ws, _| ws.usage_status(&claude.key()).is_none() && ws.agent_status[&claude.key()].logged_in));
        let kept = crate::workspace::usage::load_snapshots().get(&claude.key()).map(|s| (s.plan.clone(), s.limits.len()));
        assert_eq!(kept, Some((Some("Claude Max".into()), 1)), "what the last launch kept stays");

        // Later, on the card's cadence, after a turn, and when asked to read now: Codex again;
        // Claude Code and the mock agent not at all.
        trek.update(cx, |ws, _| ws.clock = crate::workspace::Clock::new(|| trek_core::store::now_ms() + 60_000));
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        answer_all(&asked, cx);
        let id = trek.send(cx, "explain the project");
        trek.wait_done(cx, &id, RunState::Idle).await;
        answer_all(&asked, cx);
        trek.update(cx, |ws, cx| ws.refresh_usage_now(cx));
        answer_all(&asked, cx);
        assert_eq!(reads(&asked, &AgentId::Codex), (3, 3), "on the cadence, then asked for now");
        assert_eq!(reads(&asked, &claude), (0, 1));
        assert_eq!(reads(&asked, &mock()), (0, 1), "its turn didn't read its usage");
        // Its commands changed (a skill added): read again, still without its usage.
        trek.update(cx, |ws, cx| ws.refresh_commands(cx));
        answer_all(&asked, cx);
        assert_eq!(reads(&asked, &claude), (0, 2));

        // Claude Code goes on the card: read right away, what was kept shown meanwhile.
        trek.click(cx, "usage");
        trek.click(cx, "usage-choose");
        trek.click(cx, pick(&claude));
        assert_eq!(reads(&asked, &claude), (1, 3));
        let row_of = |trek: &Trek, cx: &TestAppContext| trek.read(cx, |ws, _| ws.usage_rows().into_iter().find(|r| r.agent == AgentId::ClaudeCode)).expect("a row");
        let before = row_of(&trek, cx);
        assert_eq!((before.as_of, before.plan.as_deref(), before.limits[0].percent), (Some(now - 3_600_000), Some("Claude Max"), 55.));
        trek.click(cx, "usage-choose");
        trek.render(cx);
        assert!(trek.visible(cx, row(&claude)));
        answer_all(&asked, cx);
        let after = row_of(&trek, cx);
        assert_eq!((after.as_of, after.plan.as_deref(), after.limits[0].percent), (None, Some("Live plan"), 12.));
        // And after its turns from now on (the mock agent's, put on the card too).
        trek.click(cx, "usage");
        assert!(trek.update(cx, |ws, cx| ws.toggle_usage_shown(&mock(), cx)));
        answer_all(&asked, cx);
        assert_eq!(reads(&asked, &mock()), (1, 3), "its commands were read twice before");
        trek.update(cx, |ws, _| ws.clock = crate::workspace::Clock::new(|| trek_core::store::now_ms() + 120_000));
        let id = trek.send(cx, "and again");
        trek.wait_done(cx, &id, RunState::Idle).await;
        answer_all(&asked, cx);
        assert_eq!(reads(&asked, &mock()), (2, 4));
    });
}

#[test]
fn picked_automatically_only_what_the_card_would_show_has_its_usage_read() {
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        let asked = record(&trek, cx);
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        answer_all(&asked, cx);
        // Claude Code and Codex report a plan's usage; the test's own agents have none to show.
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, AgentId::Codex]);
        assert_eq!(reads(&asked, &AgentId::ClaudeCode), (1, 1));
        assert_eq!(reads(&asked, &AgentId::Codex), (1, 1));
        assert_eq!(reads(&asked, &mock()), (0, 1));
        assert!(trek.read(cx, |ws, _| ws.slash_commands(&crate::workspace::Scope::Main, &mock()).iter().any(|c| c.name == format!("{}-skill", mock().key()))));
        assert_eq!(shown(&trek, cx), [AgentId::ClaudeCode, AgentId::Codex], "read without its usage, the mock agent has none to show");
    });
}

/// Against the user's own `claude` and `codex` (logged in to a plan), in a data folder of the
/// test's own: with Claude Code off the card its usage isn't asked for, yet its commands and
/// models load; put on the card, its usage is read. Sends no prompt. Not run by default:
/// `TREK_LIVE_AGENT=usage cargo test -p trek-app live_usage -- --ignored` (with `SHELL` pointing
/// at a script that puts logging shims of the CLIs first on the PATH, to see what was sent).
#[test]
#[ignore = "live: runs the installed claude and codex"]
fn live_usage_is_read_only_while_on_the_card() {
    assert!(std::env::var_os("TREK_LIVE_AGENT").is_some(), "needs TREK_LIVE_AGENT: the CLIs need the user's sign-in");
    run(async |cx| {
        let trek = open(cx);
        install(&trek, cx);
        trek.update(cx, |ws, _| {
            ws.settings.usage.shown = Some(vec![AgentId::Codex.key()]);
            ws.usage_fetch = Some(Rc::new(|agent, cwd, with_usage| match agent {
                AgentId::ClaudeCode | AgentId::Codex => crate::workspace::usage::read_agent(agent, cwd, with_usage),
                _ => {
                    let (tx, rx) = async_channel::bounded(1);
                    let _ = tx.try_send(Err("not read here".into()));
                    rx
                }
            }));
        });
        // The CLIs take seconds (Claude Code's usage, up to ten), more than the harness waits.
        async fn until(trek: &Trek, cx: &mut TestAppContext, what: &str, done: impl Fn(&crate::workspace::Workspace) -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                cx.run_until_parked();
                if trek.read(cx, |ws, _| done(ws)) {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
                cx.background_executor.timer(std::time::Duration::from_millis(20)).await;
            }
        }
        let (claude, codex) = (AgentId::ClaudeCode, AgentId::Codex);
        let started = std::time::Instant::now();
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        until(&trek, cx, "both to answer", |ws| ws.agent_status.contains_key(&claude.key()) && ws.usage_status(&codex.key()).is_some()).await;
        eprintln!("read in {:.2}s", started.elapsed().as_secs_f32());

        let st = trek.read(cx, |ws, _| ws.agent_status[&claude.key()].clone());
        assert!(st.logged_in && st.plan.is_some() && st.error.is_none(), "{:?} {:?}", st.plan, st.error);
        assert!(st.limits.is_empty(), "off the card: no usage");
        assert!(trek.read(cx, |ws, _| ws.usage_status(&claude.key()).is_none()));
        let commands = trek.read(cx, |ws, _| ws.slash_commands(&crate::workspace::Scope::Main, &claude));
        assert!(commands.iter().any(|c| c.kind == CommandKind::Skill) && commands.iter().any(|c| c.kind == CommandKind::Command), "{} commands", commands.len());
        assert!(trek.read(cx, |ws, _| ws.models_for(&claude).iter().any(|m| m.id.starts_with("claude-"))));
        let st = trek.read(cx, |ws, _| ws.agent_status[&codex.key()].clone());
        assert!(st.logged_in && st.error.is_none(), "{:?}", st.error);
        assert!(!st.limits.is_empty() && st.limits.iter().all(|l| l.resets_at.is_some()), "{:?}", st.limits);
        assert!(!st.commands.is_empty() && !st.models.is_empty());
        eprintln!("claude: {} commands, {} models; codex: {} limits, {} skills, {} models", commands.len(), trek.read(cx, |ws, _| ws.models_for(&claude).len()), st.limits.len(), st.commands.len(), st.models.len());

        // On the card: read with its usage.
        let started = std::time::Instant::now();
        assert!(trek.update(cx, |ws, cx| ws.toggle_usage_shown(&claude, cx)));
        until(&trek, cx, "Claude Code's usage", |ws| ws.usage_status(&claude.key()).is_some()).await;
        eprintln!("claude usage read in {:.2}s", started.elapsed().as_secs_f32());
        let st = trek.read(cx, |ws, _| ws.agent_status[&claude.key()].clone());
        assert!(st.error.is_none(), "{:?}", st.error);
        assert!(st.limits.iter().any(|l| l.window == "5h") && st.limits.iter().any(|l| l.window == "7d"), "{:?}", st.limits);
        assert!(st.limits.iter().all(|l| l.resets_at.is_some_and(|t| t > trek_core::store::now_ms())));
        assert!(trek.read(cx, |ws, _| !ws.slash_commands(&crate::workspace::Scope::Main, &claude).is_empty()));
    });
}
