//! Signing an agent in from the terminal Trek opens for its login: Settings flips to "Signed in"
//! as soon as it's true, whether or not the terminal ever ends (a login that's the agent's own TUI
//! never does), and says in plain words that it worked and the terminal can be closed. When the
//! terminal does end, the agent is read again at once, whatever was read a moment ago.

mod common;

use common::{open, run, toasts};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use trek_agents::{AcpInfo, AgentStatus};
use trek_app::workspace::{Route, SettingsPage};
use trek_core::AgentId;
use trek_core::detect::{Availability, DetectedAgent};

fn detected(agent: AgentId, availability: Availability) -> DetectedAgent {
    DetectedAgent {
        name: agent.display_name(),
        agent,
        path: None,
        version: None,
        availability,
        models: vec![],
        install_hint: None,
    }
}

fn needs_auth(needs: bool) -> Result<AcpInfo, String> {
    Ok(AcpInfo {
        needs_auth: needs,
        ..Default::default()
    })
}

fn account_line(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext, key: &str) -> bool {
    trek.visible(cx, format!("account-{key}"))
}

/// Droid's login is its TUI, which stays open after it has signed in: Trek asks Droid again while
/// the terminal is open, and the moment it answers, says so.
#[test]
fn a_login_that_never_exits_is_noticed_while_its_terminal_is_open() {
    run(async |cx| {
        let trek = open(cx);
        let said = toasts(&trek, cx);
        let droid = AgentId::Droid;
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(droid.clone(), Availability::Ready)];
            ws.acp_info.insert(droid.key(), needs_auth(true));
            ws.login_recheck = Duration::from_secs(4);
            // What Settings › Agents' Sign in runs in a terminal.
            ws.setup_started("droid", cx);
            assert_eq!(ws.agent_signed_in(&droid.key()), Some(false));
        });
        // Still signed out a few ticks later: nothing is said.
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        assert!(said.borrow().is_empty(), "{:?}", said.borrow());
        assert!(trek.read(cx, |ws, _| ws.signed_in_now(&droid.key()).is_none()));
        // The user signs in in the TUI, which stays open. Trek's next look at Droid (the probe
        // that ran in between) finds it, and the next tick says so.
        trek.update(cx, |ws, cx| {
            ws.acp_info.insert(droid.key(), needs_auth(false));
            cx.notify();
        });
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        assert_eq!(
            said.borrow().last().map(|(m, _)| m.as_str()),
            Some("Signed in to Droid. You can close the terminal.")
        );
        let done = trek.read(cx, |ws, _| ws.signed_in_now(&droid.key()).cloned());
        assert!(done.is_some_and(|d| !d.ended && d.command == "droid"));
        // The terminal that ran it says so on its tab and beside it.
        assert_eq!(
            trek.read(cx, |ws, _| ws.login_note("droid")),
            Some("Signed in to Droid. You can close the terminal.".into())
        );
        // Nothing more is asked or said once it's done.
        let n = said.borrow().len();
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert_eq!(said.borrow().len(), n);
        // Settings' row for it says it worked.
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Agents), cx)
        });
        trek.render(cx);
        assert!(account_line(&trek, cx, "droid"));
    });
}

/// Someone already signed in who picks "Switch account…" has nothing to wait for: the login's
/// terminal isn't watched, and nothing is said until it ends.
#[test]
fn switching_account_while_signed_in_is_not_announced() {
    run(async |cx| {
        let trek = open(cx);
        let said = toasts(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(AgentId::Droid, Availability::Ready)];
            ws.acp_info.insert(AgentId::Droid.key(), needs_auth(false));
            ws.setup_started("droid", cx);
        });
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert!(said.borrow().is_empty());
        assert!(trek.read(cx, |ws, _| {
            ws.signed_in_now(&AgentId::Droid.key()).is_none()
        }));
    });
}

/// A command that isn't an agent's sign-in (an install, a project's own) is left alone.
#[test]
fn only_an_agents_own_sign_in_is_watched() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(AgentId::Droid, Availability::Ready)];
            ws.acp_info.insert(AgentId::Droid.key(), needs_auth(true));
            ws.setup_started("cargo test", cx);
            ws.setup_started("curl -fsSL https://app.factory.ai/cli | sh", cx);
        });
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.login_note("cargo test").is_none()));
    });
}

/// `claude auth login` ends when it's done. Claude Code had been read already (signed out, a
/// minute ago, so the usual 30 and 120 second spacing would have skipped it): when the terminal
/// ends it's read again at once and Settings shows the account, with no restart.
#[test]
fn a_login_that_ends_has_its_agent_read_again_at_once() {
    run(async |cx| {
        let trek = open(cx);
        let claude = AgentId::ClaudeCode;
        let asked: Rc<RefCell<Vec<(AgentId, bool)>>> = Rc::default();
        let calls = asked.clone();
        let signed_out = Rc::new(RefCell::new(true));
        let state = signed_out.clone();
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(claude.clone(), Availability::Ready)];
            ws.usage_fetch = Some(Rc::new(move |agent, _, with_usage| {
                calls.borrow_mut().push((agent.clone(), with_usage));
                let (tx, rx) = async_channel::bounded(1);
                let status = if *state.borrow() {
                    AgentStatus::default()
                } else {
                    AgentStatus {
                        logged_in: true,
                        account: Some("ada@example.com".into()),
                        plan: Some("Max".into()),
                        ..Default::default()
                    }
                };
                tx.try_send(Ok(status)).unwrap();
                rx
            }));
            ws.refresh_usage(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            trek.read(cx, |ws, _| ws.agent_signed_in(&claude.key())),
            Some(false),
            "signed out, as read"
        );
        // Asked again as a refresh would: not read, it was just read.
        let claude_reads = |asked: &Rc<RefCell<Vec<(AgentId, bool)>>>| {
            asked
                .borrow()
                .iter()
                .filter(|(a, _)| *a == AgentId::ClaudeCode)
                .count()
        };
        let before = claude_reads(&asked);
        assert!(before > 0);
        trek.update(cx, |ws, cx| ws.refresh_usage(cx));
        cx.run_until_parked();
        assert_eq!(claude_reads(&asked), before);
        // The login runs in a terminal and ends; the agent has an account now.
        *signed_out.borrow_mut() = false;
        trek.update(cx, |ws, cx| {
            ws.setup_started("claude auth login", cx);
            ws.setup_finished("claude auth login", "Login successful", cx);
        });
        cx.run_until_parked();
        assert!(claude_reads(&asked) > before, "read again");
        assert_eq!(
            trek.read(cx, |ws, _| ws.agent_signed_in(&claude.key())),
            Some(true)
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws
                .agent_status
                .get(&claude.key())
                .and_then(|s| s.account.clone())),
            Some("ada@example.com".into())
        );
    });
}
