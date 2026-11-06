//! Several accounts of one agent: nothing shows for anyone with one; a thread runs on the account
//! picked for it, and moves to another from a recap; a usage limit offers the other account (or
//! moves there by itself, if set); each account has its own meters; and removing one leaves the
//! user's normal login alone. The mock agent has accounts too, with no folder to point it at.

mod common;

use common::{Trek, mock, open, open_with, run, runs};
use gpui_kit::TestAppContext;
use std::rc::Rc;
use trek_agents::{AgentStatus, UsageLimit};
use trek_app::workspace::accounts::{AccountFetch, AccountState, DEFAULT_LABEL};
use trek_app::workspace::{Prefs, Route, Scope, SettingsPage};
use trek_core::AgentId;
use trek_core::RunState;
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::store::Item;

fn add(trek: &Trek, cx: &mut TestAppContext, agent: &AgentId, label: &str) -> String {
    trek.update(cx, |ws, cx| {
        let id = ws.add_account(agent, cx).expect("an account");
        ws.rename_account(&id, label, cx);
        id
    })
}

fn signed_in(percent: f32, resets_at: Option<i64>) -> AgentStatus {
    AgentStatus {
        account: Some("work@example.com".into()),
        plan: Some("Plan".into()),
        logged_in: true,
        limits: vec![UsageLimit {
            label: "5-hour limit".into(),
            percent,
            resets_at,
            window: "5h".into(),
        }],
        ..Default::default()
    }
}

/// Say what `account` reports, as if it had been read.
fn reports(trek: &Trek, cx: &mut TestAppContext, account: &str, status: AgentStatus) {
    trek.update(cx, |ws, cx| {
        ws.accounts.status.insert(account.to_string(), status);
        cx.notify();
    });
}

fn set_account(trek: &Trek, cx: &mut TestAppContext, scope: &Scope, account: Option<&str>) {
    let account = account.map(String::from);
    trek.update(cx, |ws, cx| {
        let prefs = Prefs {
            account,
            ..ws.prefs_in(scope)
        };
        ws.set_prefs_in(scope, prefs, cx)
    });
}

fn account_of(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<String> {
    trek.read(cx, |ws, _| ws.thread(id).and_then(|t| t.account.clone()))
}

async fn hit_limit(trek: &Trek, cx: &mut TestAppContext, text: &str) -> String {
    let id = trek.send(cx, text);
    let thread = id.clone();
    trek.wait(cx, "the limit to pause the thread", |ws| {
        ws.pause(&thread).is_some() && !ws.turn_running(&thread)
    })
    .await;
    id
}

#[test]
fn with_one_account_nothing_of_accounts_shows() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.render(cx);
        assert_eq!(account_of(&trek, cx, &id), None);
        assert!(!trek.visible(cx, "model-pill-account"));
        assert!(trek.read(cx, |ws, _| {
            ws.account_choices(&mock()).is_empty()
                && ws.account_label(&mock(), None).is_none()
                && ws.account_rows(&mock()).is_empty()
                && ws.next_account(&id).is_none()
        }));
        // The menu has no row for it.
        trek.click(cx, "model-pill");
        trek.render(cx);
        assert!(trek.visible(cx, "mm-model"));
        assert!(!trek.visible(cx, "mm-account"));
    });
}

#[test]
fn a_new_thread_runs_on_the_account_picked_for_it() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        trek.render(cx);
        // Named on the composer now that there's more than one: the default until one's picked.
        assert!(trek.visible(cx, "model-pill-account"));
        assert_eq!(
            trek.read(cx, |ws, _| ws.account_label_in(&Scope::Main)),
            Some(DEFAULT_LABEL.to_string())
        );
        set_account(&trek, cx, &Scope::Main, Some(&work));
        assert_eq!(
            trek.read(cx, |ws, _| ws.account_label_in(&Scope::Main)),
            Some("Work".to_string())
        );
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(account_of(&trek, cx, &id).as_deref(), Some(work.as_str()));
        // Saved with the thread.
        let stored = trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().account);
        assert_eq!(stored.as_deref(), Some(work.as_str()));
        // In the picker, under the model.
        trek.click(cx, "model-pill");
        trek.render(cx);
        assert!(trek.visible(cx, "mm-account"));
        trek.click(cx, "mm-account");
        trek.render(cx);
        assert!(trek.visible(cx, format!("account-{work}")));
        assert!(trek.visible(cx, "account-default"));
        assert!(trek.visible(cx, "account-recap-note"));
    });
}

#[test]
fn another_account_picks_the_thread_up_from_a_recap() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        let id = trek.send(cx, "remember APPLE");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let session = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.native_id.clone()));
        assert!(session.is_some());
        let scope = Scope::Thread(id.clone());
        let handoffs = |trek: &Trek, cx: &TestAppContext| {
            trek.items(cx, &id)
                .into_iter()
                .filter(|i| matches!(i, Item::Handoff { .. }))
                .collect::<Vec<_>>()
        };
        // There and back before sending: nothing to show, but no session to resume either (the
        // old one is another account's to go back to, and Trek doesn't).
        set_account(&trek, cx, &scope, Some(&work));
        assert_eq!(handoffs(&trek, cx).len(), 1);
        assert_eq!(
            trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.native_id.clone())),
            None,
            "its session stays in the other account's folder"
        );
        set_account(&trek, cx, &scope, None);
        assert!(handoffs(&trek, cx).is_empty());
        set_account(&trek, cx, &scope, Some(&work));
        // The divider names both sides, and the transcript keeps everything.
        let dividers = handoffs(&trek, cx);
        let [
            Item::Handoff {
                from_name, to_name, ..
            },
        ] = dividers.as_slice()
        else {
            panic!("one divider: {dividers:?}");
        };
        let (from, to) = (from_name.clone().unwrap(), to_name.clone().unwrap());
        assert!(from.ends_with(&format!("· {DEFAULT_LABEL}")), "{from}");
        assert!(to.ends_with("· Work"), "{to}");
        // The new session knows the conversation: the recap came with its first message.
        trek.update(cx, |ws, cx| ws.send_to(&id, "recall".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek.answers(cx, &id);
        assert!(
            answer
                .lines()
                .last()
                .is_some_and(|l| l.contains("remember APPLE")),
            "{answer}"
        );
        assert_eq!(account_of(&trek, cx, &id).as_deref(), Some(work.as_str()));
    });
}

#[test]
fn an_account_belongs_to_one_agent() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let scope = Scope::Thread(id.clone());
        set_account(&trek, cx, &scope, Some(&work));
        // Another agent starts on its own normal login.
        let relay = AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        trek.update(cx, |ws, cx| {
            let prefs = Prefs {
                agent: relay.clone(),
                model: None,
                ..ws.prefs_in(&scope)
            };
            ws.set_prefs_in(&scope, prefs, cx)
        });
        assert_eq!(account_of(&trek, cx, &id), None);
        // And another agent's account isn't one it can take.
        set_account(&trek, cx, &scope, Some(&work));
        assert_eq!(account_of(&trek, cx, &id), None);
        // Nor one that's gone.
        set_account(&trek, cx, &scope, Some("gone"));
        assert_eq!(account_of(&trek, cx, &id), None);
    });
}

#[test]
fn a_thread_started_from_a_thread_stays_on_its_account() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        set_account(&trek, cx, &Scope::Main, Some(&work));
        let id = trek.send(cx, "hello");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A side chat of the thread runs where the thread does.
        let side = trek
            .update(cx, |ws, cx| ws.create_side_chat(cx))
            .expect("a side chat");
        assert_eq!(account_of(&trek, cx, &side).as_deref(), Some(work.as_str()));
    });
}

#[test]
fn a_limit_offers_the_other_account_and_going_there_carries_on_now() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        reports(&trek, cx, &work, signed_in(10., None));
        let id = hit_limit(&trek, cx, "remember APPLE then mock:limit 2m").await;
        // What's typed meanwhile waits for the reset, and goes with the move instead.
        trek.update(cx, |ws, cx| ws.send_to(&id, "and PEAR".into(), vec![], cx));
        trek.render(cx);
        assert!(trek.visible(cx, "limit-bar"));
        assert!(trek.visible(cx, "limit-continue"));
        // Nothing happens by itself: it's offered.
        assert_eq!(account_of(&trek, cx, &id), None);
        assert!(trek.read(cx, |ws, _| ws.pause(&id).is_some()));
        trek.click(cx, "limit-continue");
        assert_eq!(account_of(&trek, cx, &id).as_deref(), Some(work.as_str()));
        assert!(trek.read(cx, |ws, _| ws.pause(&id).is_none()));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let said: Vec<String> = trek
            .items(cx, &id)
            .into_iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert!(said.iter().any(|t| t == "and PEAR"), "{said:?}");
        // The new session knows the conversation: the recap came with what went first.
        trek.update(cx, |ws, cx| ws.send_to(&id, "recall".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let answer = trek.answers(cx, &id);
        assert!(
            answer
                .lines()
                .last()
                .is_some_and(|l| l.contains("remember APPLE") && l.contains("and PEAR")),
            "from a recap: {answer}"
        );
        // The account it left met its limit: not offered until that resets.
        assert_eq!(trek.read(cx, |ws, _| ws.next_account(&id)), None);
    });
}

#[test]
fn only_an_account_with_room_is_offered() {
    run(async |cx| {
        let trek = open(cx);
        let id = hit_limit(&trek, cx, "mock:limit 2m").await;
        let next = |trek: &Trek, cx: &TestAppContext| {
            trek.read(cx, |ws, _| ws.next_account(&id).map(|c| c.label))
        };
        // Nowhere to go with one account.
        assert_eq!(next(&trek, cx), None);
        trek.render(cx);
        assert!(!trek.visible(cx, "limit-continue"));
        // Not signed in as far as it has said: not yet.
        let work = add(&trek, cx, &mock(), "Work");
        assert_eq!(next(&trek, cx), None);
        reports(
            &trek,
            cx,
            &work,
            AgentStatus {
                logged_in: false,
                ..Default::default()
            },
        );
        assert_eq!(next(&trek, cx), None);
        // Signed in with a window used up, that's still to reset: no.
        let later = trek.read(cx, |ws, _| ws.now()) + 3_600_000;
        reports(&trek, cx, &work, signed_in(100., Some(later)));
        assert_eq!(next(&trek, cx), None);
        // A window that's used up but has reset since: yes.
        let earlier = trek.read(cx, |ws, _| ws.now()) - 1_000;
        reports(&trek, cx, &work, signed_in(100., Some(earlier)));
        assert_eq!(next(&trek, cx).as_deref(), Some("Work"));
        // Of several, the one with most room.
        let home = add(&trek, cx, &mock(), "Home");
        reports(&trek, cx, &work, signed_in(70., Some(later)));
        reports(&trek, cx, &home, signed_in(20., Some(later)));
        assert_eq!(next(&trek, cx).as_deref(), Some("Home"));
        trek.render(cx);
        assert!(trek.visible(cx, "limit-continue"));
    });
}

#[test]
fn set_to_it_a_limit_moves_the_thread_by_itself_and_never_back_and_forth() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.continue_on_other_account = true);
        let work = add(&trek, cx, &mock(), "Work");
        reports(&trek, cx, &work, signed_in(10., None));
        let id = trek.send(cx, "remember APPLE then mock:limit 2m");
        let thread = id.clone();
        let on = work.clone();
        trek.wait(cx, "the thread to go on the other account", |ws| {
            ws.thread(&thread)
                .is_some_and(|t| t.account.as_deref() == Some(on.as_str()))
        })
        .await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(trek.read(cx, |ws, _| ws.pause(&id).is_none()));
        // That one meets its limit too. The one it came from is still at its own: it stays, paused.
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, "mock:limit 2m".into(), vec![], cx)
        });
        let thread = id.clone();
        trek.wait(cx, "the limit to pause the thread", |ws| {
            ws.pause(&thread).is_some() && !ws.turn_running(&thread)
        })
        .await;
        assert_eq!(account_of(&trek, cx, &id).as_deref(), Some(work.as_str()));
    });
}

/// A limit with the setting off moves nothing: the default is the user's choice.
#[test]
fn the_setting_is_off_unless_turned_on() {
    let s = trek_core::settings::Settings::default();
    assert!(!s.general.continue_on_other_account);
    assert!(s.accounts.is_empty());
}

#[test]
fn each_account_has_its_own_meters_and_they_outlast_a_restart() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        let later = trek.read(cx, |ws, _| ws.now()) + 3_600_000;
        let fetch: AccountFetch = Rc::new(move |account, _| {
            assert_eq!(account.label, "Work");
            let (tx, rx) = async_channel::bounded(1);
            tx.try_send(Ok(signed_in(30., Some(later)))).unwrap();
            rx
        });
        trek.update(cx, |ws, cx| {
            ws.accounts.fetch = Some(fetch);
            ws.refresh_usage_now(cx);
        });
        cx.run_until_parked();
        let rows = trek.read(cx, |ws, _| ws.account_rows(&mock()));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account.as_deref(), Some(work.as_str()));
        assert_eq!(rows[0].limits[0].percent, 30.);
        // The agent's own row is what it was: the normal login's.
        assert!(trek.read(cx, |ws, _| {
            ws.usage_rows().iter().all(|r| r.account.is_none())
        }));
        // Kept for the next launch.
        cx.run_until_parked();
        let kept = AccountState::load();
        assert_eq!(kept.cached[&work].limits[0].percent, 30.);
    });
}

#[test]
fn an_account_not_signed_in_says_so_on_the_usage_card() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        let row = |trek: &Trek, cx: &TestAppContext| {
            let rows = trek.read(cx, |ws, _| ws.account_rows(&mock()));
            (rows[0].error.clone(), rows[0].no_limits)
        };
        let not_signed_in = (Some("Not signed in".to_string()), false);
        // Before it has said anything, and when it says it isn't: not "no limits on this plan".
        assert_eq!(row(&trek, cx), not_signed_in);
        reports(&trek, cx, &work, AgentStatus::default());
        assert_eq!(row(&trek, cx), not_signed_in);
        // The picker says so too, and who it is once it knows.
        let who = |trek: &Trek, cx: &TestAppContext| {
            trek.read(cx, |ws, _| ws.account_choices(&mock())[1].detail.clone())
        };
        assert_eq!(who(&trek, cx).as_deref(), Some("Not signed in"));
        reports(&trek, cx, &work, signed_in(10., None));
        assert_eq!(row(&trek, cx), (None, false));
        assert_eq!(who(&trek, cx).as_deref(), Some("work@example.com"));
    });
}

#[test]
fn what_a_turn_says_of_its_windows_goes_to_its_own_account() {
    run(async |cx| {
        let trek = open(cx);
        let work = add(&trek, cx, &mock(), "Work");
        reports(&trek, cx, &work, signed_in(10., None));
        trek.update(cx, |ws, _| {
            ws.agent_status.insert(mock().key(), signed_in(50., None));
        });
        set_account(&trek, cx, &Scope::Main, Some(&work));
        // The footer follows the account the thread would run on, not the normal login's.
        let focus = |trek: &Trek, cx: &TestAppContext| {
            trek.read(cx, |ws, _| ws.usage_focus().map(|(_, l)| l.percent))
        };
        assert_eq!(focus(&trek, cx), Some(10.));
        // The mock says its 5-hour window is 96% used partway through the turn: Trek asks it to
        // wrap up, and the thread pauses until the reset, all on the account it runs on.
        let id = trek.send(cx, "mock:nearlimit 10m");
        let thread = id.clone();
        trek.wait(cx, "the wrapped-up turn to pause the thread", |ws| {
            ws.pause(&thread).is_some() && !ws.turn_running(&thread)
        })
        .await;
        let percent = |trek: &Trek, cx: &TestAppContext, account: Option<&str>| {
            trek.read(cx, |ws, _| {
                ws.status_on(&mock(), account).map(|s| s.limits[0].percent)
            })
        };
        assert_eq!(percent(&trek, cx, Some(&work)), Some(96.));
        assert_eq!(percent(&trek, cx, None), Some(50.), "the other login's own");
        assert_eq!(focus(&trek, cx), Some(96.));
        // A new thread on the normal login reads that one again.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        set_account(&trek, cx, &Scope::Main, None);
        assert_eq!(focus(&trek, cx), Some(50.));
    });
}

fn detected(agent: AgentId, name: &str) -> DetectedAgent {
    DetectedAgent {
        agent,
        name: name.into(),
        path: None,
        version: None,
        availability: Availability::Ready,
        models: vec![],
        install_hint: None,
    }
}

#[test]
fn settings_add_an_account_and_sign_it_in_with_the_agents_own_login() {
    run(async |cx| {
        let trek = open(cx);
        let ran = runs(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(AgentId::ClaudeCode, "Claude Code")];
            ws.navigate(Route::Settings(SettingsPage::Agents), cx);
        });
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.settings.accounts.is_empty()));
        assert!(!trek.visible(cx, "continue-on-other-account"));
        trek.click(cx, "add-account-claude-code");
        trek.render(cx);
        // The choice to carry on by itself comes with the accounts, and is off.
        assert!(trek.visible(cx, "continue-on-other-account"));
        let (id, dir) = trek.read(cx, |ws, _| {
            let a = &ws.settings.accounts[0];
            (a.id.clone(), a.dir().unwrap())
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.accounts[0].label.clone()),
            "Account 2"
        );
        assert!(trek.visible(cx, format!("account-{id}")));
        // It's named on the spot.
        assert!(trek.visible(cx, format!("account-name-{id}")));
        trek.update(cx, |ws, cx| ws.rename_account(&id, "  Work  ", cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.accounts[0].label.clone()),
            "Work"
        );
        // The agent's own login, pointed at the account's folder, which is there for it.
        trek.click(cx, format!("account-login-{id}"));
        let command = ran.borrow().last().expect("a command").0.clone();
        assert!(command.ends_with("claude auth login"), "{command}");
        assert!(command.contains("CLAUDE_CONFIG_DIR"), "{command}");
        assert!(command.contains(&dir.display().to_string()), "{command}");
        assert!(dir.is_dir());
        // Saved for the next launch.
        assert_eq!(
            trek_core::settings::Settings::load().accounts[0].label,
            "Work"
        );
    });
}

#[test]
fn removing_an_account_moves_its_threads_and_leaves_the_normal_login_alone() {
    run(async |cx| {
        let trek = open(cx);
        // The user's normal login, in their own folder.
        let normal = trek_core::paths::agents_home().join(".codex");
        std::fs::create_dir_all(&normal).unwrap();
        std::fs::write(normal.join("auth.json"), "normal").unwrap();
        std::fs::write(normal.join("config.toml"), "kept").unwrap();
        let codex = add(&trek, cx, &AgentId::Codex, "Personal");
        let account = trek.read(cx, |ws, _| ws.account(&codex).cloned()).unwrap();
        let home = account.open().unwrap();
        assert_eq!(home.var, "CODEX_HOME");
        // Its own login stays its own; the setup comes from the normal one.
        std::fs::write(home.dir.join("auth.json"), "personal").unwrap();
        assert_eq!(
            std::fs::read_to_string(home.dir.join("config.toml")).unwrap(),
            "kept"
        );
        // A thread on it, and an archived one, with sessions in its folder.
        let on_it = |trek: &Trek, cx: &mut TestAppContext, archived: bool| {
            trek.update(cx, |ws, cx| {
                let mut t = ws
                    .store
                    .create_thread(
                        None,
                        AgentId::Codex,
                        None,
                        trek_core::Effort::High,
                        trek_core::HandHolding::Auto,
                    )
                    .unwrap();
                t.account = Some(codex.clone());
                t.native_id = Some(format!("session-{}", t.id));
                if archived {
                    t.archived_at = Some(1);
                }
                ws.store.save_thread(&t).unwrap();
                ws.reload(cx);
                t.id
            })
        };
        let live = on_it(&trek, cx, false);
        let archived = on_it(&trek, cx, true);
        trek.update(cx, |ws, cx| ws.remove_account(&codex, cx));
        // Gone from the list, with its threads on the normal login and nothing to resume.
        assert!(trek.read(cx, |ws, _| ws.settings.accounts.is_empty()));
        for id in [&live, &archived] {
            let t = trek.read(cx, |ws, _| ws.store.thread(id).unwrap().unwrap());
            assert_eq!((t.account, t.native_id), (None, None), "{id}");
        }
        // Its folder goes, off the main thread. The links in it took nothing with them.
        let dir = home.dir.clone();
        trek.wait(cx, "the folder to go", move |_| !dir.exists())
            .await;
        assert_eq!(
            std::fs::read_to_string(normal.join("auth.json")).unwrap(),
            "normal"
        );
        assert_eq!(
            std::fs::read_to_string(normal.join("config.toml")).unwrap(),
            "kept"
        );
    });
}

/// Two real Codex logins, one the user's normal one and one an account: each thread runs on its
/// own login (its own plan, its own session folder), each account's meters are read from its own
/// login, and removing the account leaves the normal login as it was. Not run by default (two tiny
/// turns, and a second login to give it): sign an account in with
/// `CODEX_HOME=<folder> codex login`, then
/// `TREK_LIVE_AGENT=accounts TREK_LIVE_CODEX_HOME=<folder> cargo test -p trek-app --test accounts live_accounts -- --ignored --nocapture`.
#[test]
#[ignore = "live: runs real agents on two logins"]
fn live_accounts_keep_each_thread_on_its_own_login() {
    let given = std::path::PathBuf::from(
        std::env::var_os("TREK_LIVE_CODEX_HOME").expect("TREK_LIVE_CODEX_HOME: a signed-in folder"),
    );
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.general.default_agent = AgentId::Codex.key();
            s.general.default_model = Some("gpt-5.6-luna".into());
            s.general.default_effort = trek_core::Effort::Low;
        });
        let project = std::env::temp_dir().join("trek-accounts-e2e");
        std::fs::create_dir_all(&project).unwrap();
        trek.update(cx, |ws, cx| {
            ws.store.ensure_project(&project).unwrap();
            ws.reload(cx);
        });
        // The account, with the given login in its own folder.
        let id = add(&trek, cx, &AgentId::Codex, "Personal");
        // Where the agent will look: worked out on Trek's runtime, as a session does (this test's
        // own thread has a data folder of its own).
        let account = trek
            .read(cx, |ws, _| ws.account(&id).cloned())
            .expect("the account");
        let (tx, rx) = std::sync::mpsc::channel();
        trek_core::runtime().spawn(async move {
            let _ = tx.send(account.open());
        });
        let folder = rx.recv().unwrap().expect("the account's folder").dir;
        std::fs::copy(given.join("auth.json"), folder.join("auth.json")).unwrap();
        let default_auth = trek_core::paths::home().join(".codex").join("auth.json");
        let default_before = std::fs::read(&default_auth).ok();

        let done = async |cx: &mut TestAppContext, id: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
            loop {
                cx.run_until_parked();
                let state = trek.read(cx, |ws, _| {
                    (ws.thread(id).map(|t| t.run_state), ws.turn_running(id))
                });
                match state {
                    (Some(RunState::Idle), false) => return,
                    (Some(RunState::Failed), _) => {
                        panic!("the turn failed: {:?}", trek.items(cx, id))
                    }
                    _ => {}
                }
                assert!(std::time::Instant::now() < deadline, "timed out");
                cx.background_executor
                    .timer(std::time::Duration::from_millis(50))
                    .await;
            }
        };
        let start_on =
            async |cx: &mut TestAppContext, account: Option<&str>, word: &str| -> String {
                trek.update(cx, |ws, cx| {
                    ws.navigate(
                        Route::Draft {
                            project: Some(project.clone()),
                        },
                        cx,
                    )
                });
                set_account(&trek, cx, &Scope::Main, account);
                let word = word.to_string();
                trek.update(cx, |ws, cx| {
                    ws.send(
                        format!("Reply with just the word {word}, nothing else, and run no tools."),
                        vec![],
                        cx,
                    )
                });
                let thread = trek.thread_id(cx);
                done(cx, &thread).await;
                thread
            };

        let school = start_on(cx, None, "ONE").await;
        let personal = start_on(cx, Some(&id), "TWO").await;
        for (thread, word) in [(&school, "ONE"), (&personal, "TWO")] {
            let answer = trek.answers(cx, thread).to_uppercase();
            assert!(answer.contains(word), "{thread}: {answer}");
        }
        let (school_thread, personal_thread) = trek.read(cx, |ws, _| {
            (
                ws.thread(&school).cloned().unwrap(),
                ws.thread(&personal).cloned().unwrap(),
            )
        });
        assert_eq!(school_thread.account, None);
        assert_eq!(personal_thread.account.as_deref(), Some(id.as_str()));
        // Each session is in its own login's folder.
        let session_in = |root: &std::path::Path, native: &str| -> bool {
            walkdir_has(&root.join("sessions"), native)
        };
        let personal_native = personal_thread.native_id.clone().expect("a session");
        let school_native = school_thread.native_id.clone().expect("a session");
        assert!(
            session_in(&folder, &personal_native),
            "the account's session is in its folder"
        );
        assert!(
            !session_in(&folder, &school_native),
            "the normal login's isn't"
        );
        assert!(
            !session_in(&trek_core::paths::home().join(".codex"), &personal_native),
            "the account's session isn't in the normal login's folder"
        );
        // Each login says its own plan.
        let billing = |thread: &str| {
            trek.read(cx, |ws, _| {
                ws.live.get(thread).and_then(|l| l.billing.clone())
            })
        };
        let (school_bill, personal_bill) = (billing(&school), billing(&personal));
        println!("plans: normal {school_bill:?}, account {personal_bill:?}");
        assert_ne!(school_bill, personal_bill, "two logins, two plans");
        // The account's meters, read from its own login.
        trek.update(cx, |ws, cx| ws.sign_in_account(&id, cx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while trek.read(cx, |ws, _| ws.accounts.status.get(&id).is_none()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the account's status was never read"
            );
            cx.run_until_parked();
            cx.background_executor
                .timer(std::time::Duration::from_millis(100))
                .await;
        }
        let status = trek.read(cx, |ws, _| ws.accounts.status.get(&id).cloned().unwrap());
        println!(
            "account status: signed in {} as {:?}, plan {:?}, limits {:?}",
            status.logged_in, status.account, status.plan, status.limits
        );
        assert!(status.logged_in);
        // Removing it leaves the normal login as it was, and takes its folder.
        // The folder removal works out (on this thread: in the app there's one data folder; in a
        // test this thread has its own, so it's that one that goes, and the runtime's copy is
        // cleared after).
        let removed = trek
            .read(cx, |ws, _| ws.account(&id).and_then(|a| a.dir()))
            .expect("a folder");
        std::fs::create_dir_all(&removed).unwrap();
        trek.update(cx, |ws, cx| ws.remove_account(&id, cx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while removed.exists() {
            assert!(std::time::Instant::now() < deadline, "the folder stayed");
            cx.background_executor
                .timer(std::time::Duration::from_millis(100))
                .await;
        }
        assert_eq!(
            std::fs::read(&default_auth).ok(),
            default_before,
            "the normal login is untouched"
        );
        let _ = std::fs::remove_dir_all(&folder);
        println!("live accounts: ok");
    });
}

/// Whether any file under `dir` has `needle` in its name or its first line.
fn walkdir_has(dir: &std::path::Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        if path.is_dir() {
            return walkdir_has(&path, needle);
        }
        path.file_name()
            .is_some_and(|n| n.to_string_lossy().contains(needle))
            || std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| t.lines().next().map(|l| l.contains(needle)))
                .unwrap_or(false)
    })
}
