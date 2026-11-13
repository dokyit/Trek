//! Permission rules through the app, on the mock agent: a typed reply that declines a card, rules
//! on top of the level, "Always allow…" on a card, and a secret's request card.

mod common;

use common::{Trek, open, open_with, run};
use gpui_kit::TestAppContext;
use std::sync::{Mutex, MutexGuard};
use trek_agents::AgentEvent;
use trek_app::ipc::Reply;
use trek_app::workspace::permission_requests::ReplyNote;
use trek_core::permission_rules::{PermissionRule, RuleAction, RuleKind};
use trek_core::settings::FollowUp;
use trek_core::store::{Item, ToolStatus};
use trek_core::{HandHolding, RunState};

/// The rules in force are the process's, and these tests change them: one at a time.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static RULES: Mutex<()> = Mutex::new(());
    RULES.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_rules(trek: &Trek, cx: &mut TestAppContext, rules: Vec<PermissionRule>) {
    trek.update(cx, |ws, cx| {
        ws.settings.permissions.rules = rules;
        ws.save_settings(cx);
    });
}

fn level(trek: &Trek, cx: &mut TestAppContext, level: HandHolding) {
    trek.update(cx, |ws, cx| {
        ws.settings.permissions.full_access_unlocked = true;
        ws.draft_prefs.hand_holding = level;
        ws.save_settings(cx);
    });
}

fn card_up(trek: &Trek, cx: &TestAppContext, id: &str) -> bool {
    trek.read(cx, |ws, _| ws.pending_request(id).is_some())
}

fn user_said(trek: &Trek, cx: &TestAppContext, id: &str, words: &str) -> bool {
    trek.items(cx, id)
        .iter()
        .any(|i| matches!(i, Item::User { text, .. } if text == words))
}

fn last_tool_status(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<ToolStatus> {
    trek.items(cx, id).into_iter().rev().find_map(|i| match i {
        Item::Tool { status, .. } => Some(status),
        _ => None,
    })
}

#[test]
fn typing_while_a_card_is_up_declines_it_and_sends_the_words() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        assert_eq!(
            trek.read(cx, |ws, _| ws.reply_note(&id)),
            Some(ReplyNote::Declines),
            "the composer says so"
        );
        trek.send(cx, "use the staging script");
        assert!(!card_up(&trek, cx, &id), "the card went");
        assert!(user_said(&trek, cx, &id, "use the staging script"));
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(last_tool_status(&trek, cx, &id), Some(ToolStatus::Failed));
        // The mock says no to the command, then takes the words: as its next message, or folded
        // into the declined turn when they reach it before that turn has ended (a race CI wins).
        let answers = trek.answers(cx, &id);
        assert!(answers.contains("I won't run it"), "{answers}");
        assert!(
            answers.contains("How the app starts")
                || answers.contains("Noted — use the staging script"),
            "{answers}"
        );
        assert_eq!(trek.read(cx, |ws, _| ws.reply_note(&id)), None);
    });
}

#[test]
fn a_reply_goes_now_even_when_messages_queue_by_default() {
    let _one = one_at_a_time();
    run(async |cx| {
        // Queued, it would wait for a turn that's waiting on the very card it answers.
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        trek.send(cx, "not that one");
        assert!(!card_up(&trek, cx, &id));
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
        assert!(user_said(&trek, cx, &id, "not that one"));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn the_queue_key_leaves_the_card_up_and_holds_the_words() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Steer);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        trek.type_text(cx, "and then this");
        trek.press(cx, "alt-enter");
        assert!(card_up(&trek, cx, &id), "not an answer to the card");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(user_said(&trek, cx, &id, "and then this"), "it went after");
    });
}

#[test]
fn only_the_users_own_typing_declines_a_card() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        // Another agent's `send_to_thread`, a wake-up, a queued message: nobody said no.
        trek.update(cx, |ws, cx| {
            ws.send_to(&id, "from another agent".into(), vec![], cx)
        });
        assert!(card_up(&trek, cx, &id));
        // Trek's own commands are answered by Trek: the agent is still waiting.
        trek.send(cx, "/permissions");
        assert!(card_up(&trek, cx, &id));
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn a_question_still_takes_typed_words_as_its_answer() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "question");
        trek.wait_needs_you(cx, &id).await;
        assert_eq!(trek.read(cx, |ws, _| ws.reply_note(&id)), None);
        trek.send(cx, "Postgres");
        // The first of two questions took it; the card is still there for the other.
        assert!(user_said(&trek, cx, &id, "Postgres"));
    });
}

#[test]
fn a_deny_rule_stops_the_command_at_full_access_and_says_so() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        level(&trek, cx, HandHolding::FullAccess);
        set_rules(
            &trek,
            cx,
            vec![PermissionRule::new(
                RuleKind::Command,
                "./scripts/migrate.sh",
                RuleAction::Deny,
            )],
        );
        let id = trek.send(cx, "permission");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(last_tool_status(&trek, cx, &id), Some(ToolStatus::Failed));
        let noted = trek.items(cx, &id).iter().any(|i| {
            matches!(i, Item::Notice { text } if text.contains("Your rule stopped")
                && text.contains("migrate.sh"))
        });
        assert!(noted, "{:?}", trek.items(cx, &id));
        set_rules(&trek, cx, vec![]);
    });
}

#[test]
fn an_ask_rule_puts_a_card_up_at_full_access_and_an_allow_takes_it_down_at_supervised() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        level(&trek, cx, HandHolding::FullAccess);
        set_rules(
            &trek,
            cx,
            vec![PermissionRule::new(
                RuleKind::Command,
                "./scripts/migrate.sh",
                RuleAction::Ask,
            )],
        );
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        trek.render(cx);
        // A rule that always asks: nothing here would stop it asking again.
        assert!(!trek.visible(cx, "allow-session"));
        assert!(!trek.visible(cx, "always-allow"));
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        set_rules(&trek, cx, vec![]);
    });
    run(async |cx| {
        let trek = open(cx);
        set_rules(
            &trek,
            cx,
            vec![PermissionRule::new(
                RuleKind::Command,
                "./scripts/migrate.sh",
                RuleAction::Allow,
            )],
        );
        let id = trek.send(cx, "permission");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(last_tool_status(&trek, cx, &id), Some(ToolStatus::Done));
        set_rules(&trek, cx, vec![]);
    });
}

#[test]
fn always_allow_writes_a_rule_the_user_can_see_and_remove() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        trek.render(cx);
        assert!(trek.visible(cx, "allow-session"));
        trek.click(cx, "always-allow");
        trek.render(cx);
        assert!(trek.visible(cx, "always-allow-pattern"));
        assert!(card_up(&trek, cx, &id), "nothing's decided yet");
        trek.click(cx, "always-allow-add");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(last_tool_status(&trek, cx, &id), Some(ToolStatus::Done));
        let rules = trek.read(cx, |ws, _| ws.settings.permissions.rules.clone());
        assert_eq!(
            rules,
            vec![PermissionRule::new(
                RuleKind::Command,
                "./scripts/migrate.sh --apply",
                RuleAction::Allow
            )],
            "as narrow as what was on the card"
        );
        // The next one needs no card, in this session or any.
        trek.send(cx, "permission again");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Removed, it asks again.
        trek.update(cx, |ws, cx| ws.remove_permission_rule(0, cx));
        trek.send(cx, "permission a third time");
        trek.wait_needs_you(cx, &id).await;
        trek.click(cx, "deny");
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

/// The agent's side of a request: its reply when it comes.
fn ask_secret(trek: &Trek, cx: &mut TestAppContext, id: &str) -> async_channel::Receiver<Reply> {
    let (tx, rx) = async_channel::bounded(1);
    trek.update(cx, |ws, cx| {
        ws.ask_for_secret(id, "GITHUB_TOKEN", "To open the pull request.", tx, cx)
    });
    trek.render(cx);
    rx
}

fn type_secret(trek: &Trek, cx: &mut TestAppContext, value: &str) {
    let field = trek.thread_view(cx).read_with(cx, |v, _| v.secret_field(0));
    let value = value.to_string();
    trek.window(cx, |window, cx| {
        field.update(cx, |s, cx| s.set_value(value, window, cx))
    });
    trek.render(cx);
}

/// A thread with its turn over, to hang a request on.
async fn quiet(trek: &Trek, cx: &mut TestAppContext) -> String {
    let id = trek.send(cx, "hello");
    trek.wait_done(cx, &id, RunState::Idle).await;
    id
}

/// Everything that could carry a value: the transcript in memory, and what was saved.
fn nowhere_in_the_thread(trek: &Trek, cx: &mut TestAppContext, id: &str, value: &str) {
    let items = format!("{:?}", trek.items(cx, id));
    assert!(!items.contains(value), "in the transcript: {items}");
    trek.update(cx, |ws, cx| ws.persist_for_test(id, cx));
    let stored = trek.read(cx, |ws, _| ws.store.items(id).unwrap());
    assert!(
        !format!("{stored:?}").contains(value),
        "in the store: {stored:?}"
    );
}

#[test]
fn a_secret_goes_to_a_private_file_and_the_agent_only_gets_the_path() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let reply = ask_secret(&trek, cx, &id);
        assert_eq!(trek.run_state(cx, &id), RunState::NeedsYou);
        assert!(trek.visible(cx, "secret-value"));
        assert!(trek.visible(cx, "secret-decline"));

        let value = "ghp_correct-horse-battery";
        type_secret(&trek, cx, value);
        trek.click(cx, "secret-save");
        assert!(!card_up(&trek, cx, &id));

        let Ok(Reply::Done(Ok(answer))) = reply.try_recv() else {
            panic!("no reply");
        };
        let told = answer["text"].as_str().unwrap();
        assert!(!told.contains(value), "the agent's reply has it: {told}");
        let path = told
            .split("saved in the file ")
            .nth(1)
            .and_then(|r| r.split(". Its value").next())
            .expect("a path");
        assert_eq!(std::fs::read_to_string(path).unwrap(), value);
        assert!(
            trek_private::is_private(std::path::Path::new(path)).unwrap(),
            "the user's alone, on both platforms"
        );
        nowhere_in_the_thread(&trek, cx, &id, value);
        let noted = trek
            .items(cx, &id)
            .iter()
            .any(|i| matches!(i, Item::Notice { text } if text.starts_with("GITHUB_TOKEN saved")));
        assert!(noted, "the transcript says it was given, by name");

        // The field is empty for the next card.
        let field = trek.thread_view(cx).read_with(cx, |v, _| v.secret_field(0));
        assert_eq!(field.read_with(cx, |s, _| s.value().to_string()), "");

        // Deleting the thread takes the file.
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(!std::path::Path::new(path).exists());
    });
}

#[test]
fn an_agent_that_prints_its_secret_doesnt_get_it_into_the_transcript() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &id);
        let value = "ghp_correct-horse-battery";
        type_secret(&trek, cx, value);
        trek.click(cx, "secret-save");
        let (front, back) = value.split_at(8);
        let events = vec![
            AgentEvent::ToolStarted {
                id: "t1".into(),
                title: "Run command".into(),
                detail: format!("curl -H 'Bearer {value}' https://example.com"),
            },
            AgentEvent::ToolFinished {
                id: "t1".into(),
                output: format!("token: {value}"),
                ok: true,
            },
            // One value in two pieces.
            AgentEvent::ReasoningDelta(format!("the token is {front}")),
            AgentEvent::ReasoningDelta(format!("{back} I think")),
            AgentEvent::TextDelta(format!("It's {front}")),
            AgentEvent::TextDelta(format!("{back}, by the way.")),
            AgentEvent::TextDone(format!("It's {value}, by the way.")),
            AgentEvent::Notice(format!("note {value}")),
        ];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
        nowhere_in_the_thread(&trek, cx, &id, value);
        // And a split one mustn't survive in pieces, in what's kept.
        let text = format!("{:?}", trek.items(cx, &id));
        assert!(text.contains("[GITHUB_TOKEN hidden]"), "{text}");
    });
}

#[test]
fn a_secret_given_to_one_thread_stays_out_of_every_other() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let first = quiet(&trek, cx).await;
        let _reply = ask_secret(&trek, cx, &first);
        let value = "ghp_handed-to-a-sub-agent";
        type_secret(&trek, cx, value);
        trek.click(cx, "secret-save");
        let other = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &other,
                vec![AgentEvent::TextDone(format!("I read {value}"))],
                cx,
            )
        });
        nowhere_in_the_thread(&trek, cx, &other, value);
    });
}

#[test]
fn typing_in_the_composer_doesnt_answer_a_secrets_card() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let reply = ask_secret(&trek, cx, &id);
        assert_eq!(
            trek.read(cx, |ws, _| ws.reply_note(&id)),
            Some(ReplyNote::SecretInCard)
        );
        // Pasted in the wrong place: it isn't sent, and it isn't lost.
        let pasted = "ghp_pasted-in-the-wrong-place";
        trek.send(cx, pasted);
        cx.run_until_parked();
        assert!(card_up(&trek, cx, &id), "the card is still up");
        assert!(reply.try_recv().is_err(), "nothing went to the agent");
        nowhere_in_the_thread(&trek, cx, &id, pasted);
        assert_eq!(trek.composer_text(cx), pasted, "back in the composer");
    });
}

#[test]
fn declining_a_secret_tells_the_agent_and_saves_nothing() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        let reply = ask_secret(&trek, cx, &id);
        type_secret(&trek, cx, "half-typed-value");
        trek.click(cx, "secret-decline");
        assert!(!card_up(&trek, cx, &id));
        let Ok(Reply::Done(Err(why))) = reply.try_recv() else {
            panic!("no refusal");
        };
        assert!(why.contains("declined"), "{why}");
        assert!(!why.contains("half-typed-value"));
        nowhere_in_the_thread(&trek, cx, &id, "half-typed-value");
        let saved = trek_core::paths::data_dir().join("session-secrets");
        let any = std::fs::read_dir(&saved)
            .into_iter()
            .flatten()
            .flatten()
            .count();
        assert_eq!(any, 0, "nothing in {saved:?}");
    });
}

#[test]
fn a_bad_request_for_a_secret_never_reaches_a_card() {
    let _one = one_at_a_time();
    run(async |cx| {
        let trek = open(cx);
        let id = quiet(&trek, cx).await;
        for (name, why) in [
            ("../../.ssh/id", "To help."),
            ("GOOD NAME", "To help."),
            ("", "To help."),
            ("TOKEN", "   "),
        ] {
            let (tx, rx) = async_channel::bounded(1);
            trek.update(cx, |ws, cx| ws.ask_for_secret(&id, name, why, tx, cx));
            assert!(matches!(rx.try_recv(), Ok(Reply::Done(Err(_)))), "{name}");
            assert!(!card_up(&trek, cx, &id), "{name}");
        }
    });
}
