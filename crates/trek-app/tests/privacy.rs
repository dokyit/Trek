//! Settings › Privacy, the card asking about usage data, and the feedback sheet. Nothing here
//! sends anything: a test process is a test profile, and the sheet is given a stand-in for
//! PostHog.

mod common;

use common::{Trek, open, run};
use gpui_kit::{Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use trek_app::feedback::{FeedbackSheet, Stage};
use trek_app::workspace::{Route, SettingsPage};
use trek_telemetry::{Consent, Diagnostics, Feedback, NotSent, Platform};

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

/// The sheet, open, with its diagnostics read (there's no log in a test's data folder) and then
/// set to a known few lines.
fn sheet(trek: &Trek, cx: &mut TestAppContext) -> Entity<FeedbackSheet> {
    let ws = trek.ws.clone();
    let sheet = trek.window(cx, |window, cx| trek_app::feedback::open(ws, window, cx));
    trek.render(cx);
    sheet.update(cx, |s, _| {
        s.set_diagnostics(Diagnostics {
            platform: Platform {
                os: "macOS",
                os_version: Some("26.1".into()),
                arch: "aarch64",
                app_version: "0.4.41".into(),
                channel: "stable".into(),
            },
            recent_errors: vec!["WARN trek_app: a warning".into()],
            log_tail: vec!["INFO trek_app: started".into()],
        })
    });
    trek.render(cx);
    sheet
}

type Sent = Rc<RefCell<Vec<Feedback>>>;

/// PostHog's stand-in: answers `answer`, and keeps what it was given.
fn posthog(
    sheet: &Entity<FeedbackSheet>,
    answer: Result<(), NotSent>,
    cx: &mut TestAppContext,
) -> Sent {
    let sent = Sent::default();
    let kept = sent.clone();
    sheet.update(cx, |s, _| {
        s.send_with(
            Ok("us.i.posthog.com".into()),
            Rc::new(move |f| {
                kept.borrow_mut().push(f);
                trek_telemetry::answered(answer.clone())
            }),
        )
    });
    sent
}

/// Click into the field `id` and type `text` there, as a user does.
fn type_into(trek: &Trek, id: &'static str, text: &str, cx: &mut TestAppContext) {
    trek.click(cx, id);
    trek.type_text(cx, text);
    trek.render(cx);
}

#[test]
fn feedback_goes_once_it_says_something_and_says_it_went() {
    run(async |cx| {
        let trek = open(cx);
        let sheet = sheet(&trek, cx);
        assert!(trek.visible(cx, "feedback-sheet"));
        let sent = posthog(&sheet, Ok(()), cx);
        trek.render(cx);
        // Nothing written: it says so, and nothing goes.
        trek.click(cx, "feedback-send");
        trek.render(cx);
        assert!(trek.visible(cx, "feedback-problem"));
        assert!(sent.borrow().is_empty());
        // Typing clears what was wrong.
        type_into(&trek, "feedback-message", "The sidebar froze", cx);
        assert!(!trek.visible(cx, "feedback-problem"));
        // An email that can't be one.
        type_into(&trek, "feedback-email", "me@example", cx);
        trek.click(cx, "feedback-send");
        trek.render(cx);
        assert!(trek.visible(cx, "feedback-problem"));
        assert!(sent.borrow().is_empty());
        // Put right, it goes, and says so.
        type_into(&trek, "feedback-email", ".com", cx);
        assert!(!trek.visible(cx, "feedback-problem"));
        trek.click(cx, "feedback-send");
        trek.render(cx);
        assert_eq!(sheet.read_with(cx, |s, _| s.stage.clone()), Stage::Sent);
        assert!(trek.visible(cx, "feedback-sent"));
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message, "The sidebar froze");
        assert_eq!(sent[0].email.as_deref(), Some("me@example.com"));
        let d = sent[0]
            .diagnostics
            .as_ref()
            .expect("diagnostics are on by default");
        assert_eq!(d.log_tail, ["INFO trek_app: started"]);
    });
}

#[test]
fn a_failed_send_says_why_and_can_go_again() {
    run(async |cx| {
        let trek = open(cx);
        let sheet = sheet(&trek, cx);
        posthog(&sheet, Err(NotSent::Failed("no connection".into())), cx);
        type_into(&trek, "feedback-message", "Crash on quit", cx);
        trek.click(cx, "feedback-send");
        trek.render(cx);
        assert_eq!(
            sheet.read_with(cx, |s, _| s.stage.clone()),
            Stage::NotSent(NotSent::Failed("no connection".into()))
        );
        assert!(trek.visible(cx, "feedback-status"));
        // Copy report works from here too.
        trek.click(cx, "feedback-copy");
        assert!(
            clipboard(cx).is_some_and(|c| c.starts_with("Trek feedback: Bug\n\nCrash on quit"))
        );
        let sent = posthog(&sheet, Ok(()), cx);
        trek.render(cx);
        trek.click(cx, "feedback-send");
        trek.render(cx);
        assert_eq!(sent.borrow().len(), 1);
        assert!(trek.visible(cx, "feedback-sent"));
    });
}

#[test]
fn without_a_key_the_report_is_copied_and_diagnostics_can_stay_home() {
    run(async |cx| {
        let trek = open(cx);
        let sheet = sheet(&trek, cx);
        // A test process has nowhere to send to: it's a test profile.
        assert_eq!(
            sheet.read_with(cx, |s, _| s.destination.clone()),
            Err(NotSent::TestProfile)
        );
        sheet.update(cx, |s, _| {
            s.send_with(
                Err(NotSent::NoKey),
                Rc::new(|_| -> trek_telemetry::Answer { panic!("nothing is sent without a key") }),
            )
        });
        type_into(&trek, "feedback-message", "An idea", cx);
        trek.click(cx, ("feedback-kind", 1usize));
        trek.click(cx, "feedback-diagnostics");
        trek.render(cx);
        assert!(!sheet.read_with(cx, |s, _| s.include_diagnostics));
        // The primary button copies instead of sending.
        trek.click(cx, "feedback-send");
        let report = clipboard(cx).expect("copied");
        assert!(
            report.starts_with("Trek feedback: Idea\n\nAn idea"),
            "{report}"
        );
        assert!(report.ends_with("No diagnostics.\n"), "{report}");
        assert_eq!(sheet.read_with(cx, |s, _| s.stage.clone()), Stage::Writing);
        // What it would send can be read whole first.
        trek.click(cx, "feedback-show");
        trek.render(cx);
        assert!(trek.visible(cx, "feedback-report"));
    });
}

#[test]
fn help_and_the_palette_open_the_sheet() {
    run(async |cx| {
        let trek = open(cx);
        let help = trek_app::menus()
            .into_iter()
            .find(|m| m.name.as_ref() == "Help")
            .expect("a Help menu");
        assert_eq!(help.items.len(), 1);
        // The app's own actions, as `run` has them.
        cx.update(trek_app::app_actions);
        trek.window(cx, |window, cx| {
            window.dispatch_action(Box::new(trek_app::SendFeedback), cx)
        });
        trek.render(cx);
        assert!(trek.visible(cx, "feedback-sheet"));
    });
}

#[test]
fn the_privacy_page_takes_the_answer_the_card_asks_for() {
    run(async |cx| {
        let trek = open(cx);
        // Started as at launch: a test process is a test profile, so it never asks by itself.
        let ws = trek.ws.clone();
        cx.update(|cx| trek_app::privacy::start(&ws, cx));
        assert!(!trek_telemetry::should_ask());
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Privacy), cx)
        });
        trek.render(cx);
        for id in ["privacy-consent", "privacy-reset", "privacy-show-sent"] {
            assert!(trek.visible(cx, id), "{id}");
        }
        // The card, as the verify profile puts it up: an answer, and it's gone.
        trek.window(cx, |window, cx| trek_app::privacy::open_card(window, cx));
        trek.render(cx);
        assert!(trek.visible(cx, "consent-card"));
        trek.click(cx, "consent-yes");
        trek.render(cx);
        assert!(!trek.visible(cx, "consent-card"));
        let status = trek_telemetry::status().expect("started");
        assert_eq!(status.consent, Consent::Yes);
        let id = status.install_id.expect("an id with the yes");
        // Nothing is waiting: a test profile queues nothing.
        assert_eq!(status.waiting, 0);
        trek.click(cx, "privacy-reset");
        let again = trek_telemetry::status().unwrap().install_id.unwrap();
        assert_ne!(id, again);
        trek.click(cx, "privacy-consent");
        assert_eq!(trek_telemetry::consent(), Some(Consent::No));
    });
}

#[test]
fn what_was_sent_shows_the_project_key_cut_to_its_ends() {
    let body = serde_json::to_string_pretty(&serde_json::json!({
        "api_key": "phc_qLaKdAVD2NvszYYrRjtMXtjHpgiQ5HTQH8wXE3d93urq",
        "batch": [{ "event": "app_opened" }],
    }))
    .unwrap();
    let shown = trek_app::settings_view::privacy::shown_body(&body);
    assert!(shown.contains("\"api_key\": \"phc_qLaK…3urq\""), "{shown}");
    assert!(!shown.contains("dAVD2NvszYY"), "{shown}");
    // Everything else as it went.
    assert!(shown.contains("\"event\": \"app_opened\""), "{shown}");
    // Not JSON, or no key: as it is.
    assert_eq!(trek_app::settings_view::privacy::shown_body("{"), "{");
    let keyless = serde_json::to_string_pretty(&serde_json::json!({ "batch": [] })).unwrap();
    assert_eq!(
        trek_app::settings_view::privacy::shown_body(&keyless),
        keyless
    );
    assert!(trek_app::settings_view::privacy::KEY_NOTE.contains("can only add events"));
}
