//! Send feedback (Help › Send Feedback…, ⌘K's "Send feedback…", Settings › About and Privacy):
//! what happened, whether it's a bug, an idea or something else, an email if the user would like
//! a reply, and diagnostics unless they turn them off. What would go is said before it goes, and
//! can be read whole. It's sent to PostHog (`trek_telemetry::send_feedback`), which needs no yes
//! to telemetry: sending it is the yes. A build without a key, or the verify profile, copies the
//! report instead; either way it can be filed as a GitHub issue.

use crate::composer::look;
use crate::scale::{radius, text};
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;
use trek_telemetry::{Diagnostics, Feedback, Kind, NotSent};

const WIDTH: f32 = 470.;
const PAD: f32 = 20.;

/// What sends a report: `trek_telemetry::send_feedback`, or a stand-in in tests.
pub type Send = Rc<dyn Fn(Feedback) -> trek_telemetry::Answer>;

/// Where the sheet is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Writing,
    Sending,
    Sent,
    /// It didn't go: the network, or nowhere to send it.
    NotSent(NotSent),
}

/// Open the sheet in the main window, bringing it forward (the menu, Settings' buttons). Once
/// the click or the action is through: until then the window it came from is busy, and the
/// sheet couldn't go up in it.
pub fn open_in_main(cx: &mut App) {
    cx.defer(|cx| {
        let ws = crate::workspace::workspace_global(cx);
        crate::root::show_main(ws.clone(), cx);
        let Some(main) = ws.read(cx).main_window else {
            return;
        };
        let _ = main.update(cx, |_, window, cx| {
            open(ws, window, cx);
        });
    });
}

/// Open the sheet in `window`.
pub fn open(ws: Entity<Workspace>, window: &mut Window, cx: &mut App) -> Entity<FeedbackSheet> {
    let sheet = cx.new(|cx| FeedbackSheet::new(ws, window, cx));
    let focus = sheet.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |s, cx| {
            s.message.update(cx, |m, cx| m.focus(window, cx))
        })
    });
    let shown = sheet.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(WIDTH)).p(px(0.)).child(shown.clone())
    });
    sheet
}

pub struct FeedbackSheet {
    pub kind: Kind,
    message: Entity<TextareaState>,
    email: Entity<InputState>,
    pub include_diagnostics: bool,
    /// Read off the UI thread as the sheet opens; `None` until then.
    diagnostics: Option<Diagnostics>,
    /// The whole report is on show.
    pub showing: bool,
    pub stage: Stage,
    /// Where it goes (PostHog's host), or why it can't go.
    pub destination: Result<String, NotSent>,
    send: Send,
    /// What's wrong with what was typed.
    pub problem: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

fn placeholder(kind: Kind) -> &'static str {
    match kind {
        Kind::Bug => "What happened, and what did you expect?",
        Kind::Idea => "What would you like Trek to do?",
        Kind::Other => "What's on your mind?",
    }
}

impl FeedbackSheet {
    fn new(ws: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder(placeholder(Kind::Bug))
        });
        let email = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Email, if you'd like a reply (optional)")
        });
        // A problem is about what was typed: typing again clears it.
        let typed = |this: &mut Self, event: &InputEvent, cx: &mut Context<Self>| {
            if matches!(event, InputEvent::Change) {
                this.problem = None;
                cx.notify();
            }
        };
        let subs = vec![
            cx.subscribe(&message, move |this, _, event: &InputEvent, cx| {
                typed(this, event, cx)
            }),
            cx.subscribe(&email, move |this, _, event: &InputEvent, cx| {
                typed(this, event, cx)
            }),
        ];
        let channel = format!("{:?}", ws.read(cx).settings.updates.channel).to_lowercase();
        let platform = trek_telemetry::platform()
            .unwrap_or_else(|| trek_telemetry::Platform::here(None, &channel));
        let scrubber = crate::privacy::scrubber(ws.read(cx));
        let logs = crate::logging::log_dir();
        let read = cx.background_executor().spawn(async move {
            trek_telemetry::feedback::diagnostics(platform, &logs, &scrubber)
        });
        cx.spawn(async move |this, cx| {
            let diagnostics = read.await;
            let _ = this.update(cx, |this, cx| {
                this.diagnostics = Some(diagnostics);
                cx.notify();
            });
        })
        .detach();
        FeedbackSheet {
            kind: Kind::Bug,
            message,
            email,
            include_diagnostics: true,
            diagnostics: None,
            showing: false,
            stage: Stage::Writing,
            destination: trek_telemetry::feedback_destination(),
            send: Rc::new(trek_telemetry::send_feedback),
            problem: None,
            _subscriptions: subs,
        }
    }

    /// Send through `send` to `destination` instead (tests).
    pub fn send_with(&mut self, destination: Result<String, NotSent>, send: Send) {
        self.destination = destination;
        self.send = send;
    }

    /// Use these diagnostics rather than the ones read from the log (tests).
    pub fn set_diagnostics(&mut self, diagnostics: Diagnostics) {
        self.diagnostics = Some(diagnostics);
    }

    pub fn set_message(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.message
            .update(cx, |m, cx| m.set_value(text.to_string(), window, cx));
    }

    pub fn set_email(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.email
            .update(cx, |m, cx| m.set_value(text.to_string(), window, cx));
    }

    fn set_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = kind;
        self.message
            .update(cx, |m, cx| m.set_placeholder(placeholder(kind), window, cx));
        cx.notify();
    }

    /// The report as it stands.
    pub fn feedback(&self, cx: &App) -> Feedback {
        let email = self.email.read(cx).value().trim().to_string();
        Feedback {
            kind: self.kind,
            message: self.message.read(cx).value().trim().to_string(),
            email: (!email.is_empty()).then_some(email),
            diagnostics: self
                .include_diagnostics
                .then(|| self.diagnostics.clone())
                .flatten(),
        }
    }

    /// Whether it may go as typed; says what's wrong if not.
    fn check(&mut self, cx: &App) -> bool {
        let f = self.feedback(cx);
        self.problem = if f.message.is_empty() {
            Some("Write what happened first.")
        } else if f
            .email
            .as_deref()
            .is_some_and(|e| !trek_telemetry::feedback::plausible_email(e))
        {
            Some("That email address doesn't look right.")
        } else {
            None
        };
        self.problem.is_none()
    }

    /// Send it, or with nowhere to send it, copy it.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage == Stage::Sending || !self.check(cx) {
            cx.notify();
            return;
        }
        if self.destination.is_err() {
            return self.copy_report(window, cx);
        }
        self.stage = Stage::Sending;
        cx.notify();
        let answer = (self.send)(self.feedback(cx));
        cx.spawn(async move |this, cx| {
            let result = answer
                .await
                .unwrap_or_else(|_| Err(NotSent::Failed("it stopped".into())));
            let _ = this.update(cx, |this, cx| {
                this.stage = match result {
                    Ok(()) => Stage::Sent,
                    Err(why) => Stage::NotSent(why),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn copy_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.check(cx) {
            cx.notify();
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.feedback(cx).report()));
        crate::toast::push(window, "Report copied", cx);
    }

    fn file_on_github(&mut self, cx: &mut Context<Self>) {
        if !self.check(cx) {
            cx.notify();
            return;
        }
        cx.open_url(&self.feedback(cx).github_issue_url());
    }

    /// What it sends, in a line.
    fn summary(&self, cx: &App) -> String {
        let f = self.feedback(cx);
        let you = if f.email.is_some() {
            "your message and email"
        } else {
            "your message"
        };
        match (&f.diagnostics, self.include_diagnostics) {
            (_, false) => format!("Sends {you}, and nothing else."),
            (None, true) => format!("Sends {you} and diagnostics (reading the log…)."),
            (Some(d), true) => {
                let errors = match d.recent_errors.len() {
                    0 => "no recent errors".to_string(),
                    1 => "1 recent error".to_string(),
                    n => format!("{n} recent errors"),
                };
                let os = match &d.platform.os_version {
                    Some(v) => format!("{} {v}", d.platform.os),
                    None => d.platform.os.to_string(),
                };
                format!(
                    "Sends {you}, Trek {} on {os} ({}), {errors} and the last {} lines of its log.",
                    d.platform.app_version,
                    d.platform.arch,
                    d.log_tail.len()
                )
            }
        }
    }
}

fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(text::LABEL))
        .line_height(relative(1.45))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

impl FeedbackSheet {
    fn sent(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let reply = self
            .email
            .read(cx)
            .value()
            .trim()
            .is_empty()
            .then_some("It went to Trek's developer.")
            .unwrap_or("It went to Trek's developer, who may write back to the email you gave.");
        v_flex()
            .id("feedback-sent")
            .test_support()
            .w_full()
            .p(px(PAD))
            .gap(px(14.))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(
                        Icon::new(crate::assets::Lucide::CircleCheck)
                            .size(px(20.))
                            .text_color(crate::palette::emerald(cx)),
                    )
                    .child(
                        div()
                            .text_size(px(text::HEADING))
                            .font_semibold()
                            .child("Thanks, it's sent"),
                    ),
            )
            .child(note(reply, cx))
            .child(
                h_flex().justify_end().child(
                    Button::new("feedback-done")
                        .primary()
                        .label("Done")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}

impl Render for FeedbackSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stage == Stage::Sent {
            return self.sent(cx).into_any_element();
        }
        let theme = cx.theme().clone();
        let view = cx.entity();
        let pick = move |kind: Kind, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.set_kind(kind, window, cx))
        };
        let sending = self.stage == Stage::Sending;
        let reading = self.include_diagnostics && self.diagnostics.is_none();
        let can_send = self.destination.is_ok();
        // Where it goes, or why it can't, or why it didn't.
        let (status, failed) = match (&self.stage, &self.destination) {
            (Stage::NotSent(why), _) => (why.message(), true),
            (_, Err(why)) => (why.message(), false),
            (_, Ok(host)) => (
                format!("Goes to Trek's developer through PostHog ({host})."),
                false,
            ),
        };
        let report = self.showing.then(|| self.feedback(cx).report());
        v_flex()
            .id("feedback-sheet")
            .test_support()
            .w_full()
            .p(px(PAD))
            .gap(px(12.))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(div().text_size(px(text::HEADING)).font_semibold().child("Send feedback"))
                    .child(note("A bug, an idea, or anything else about Trek.", cx)),
            )
            .child(h_flex().child(look::segmented(
                "feedback-kind",
                vec![(Kind::Bug, "Bug"), (Kind::Idea, "Idea"), (Kind::Other, "Other")],
                self.kind,
                pick,
                cx,
            )))
            .child(
                div()
                    .id("feedback-message")
                    .test_support()
                    .child(Textarea::new(&self.message).text_size(px(text::BODY))),
            )
            .child(
                div()
                    .id("feedback-email")
                    .test_support()
                    .child(Input::new(&self.email).small()),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(text::BODY)).child("Include diagnostics"))
                            .child(note(
                                "The version, OS and architecture, recent errors and the end of Trek's log, with paths, emails and keys taken out.",
                                cx,
                            )),
                    )
                    .child(
                        Switch::new("feedback-diagnostics")
                            .checked(self.include_diagnostics)
                            .small()
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.include_diagnostics = *on;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(crate::fluent::control(radius::BUTTON)))
                    .bg(look::chip_fill(cx))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_start()
                            .child(note(self.summary(cx), cx).flex_1().min_w_0())
                            .child(
                                Button::new("feedback-show")
                                    .ghost()
                                    .xsmall()
                                    .label(if self.showing { "Hide" } else { "Show all" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.showing = !this.showing;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(report, |el, report| {
                        el.child(
                            div()
                                .id("feedback-report")
                                .test_support()
                                .max_h(px(180.))
                                .overflow_y_scroll()
                                .border_t_1()
                                .border_color(look::hairline(cx))
                                .pt(px(6.))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(text::META))
                                .line_height(relative(1.45))
                                .text_color(theme.muted_foreground)
                                .child(report),
                        )
                    }),
            )
            .when_some(self.problem, |el, problem| {
                el.child(
                    div()
                        .id("feedback-problem")
                        .test_support()
                        .text_size(px(text::BODY))
                        .text_color(crate::palette::red(cx))
                        .child(problem),
                )
            })
            .child(
                div()
                    .id("feedback-status")
                    .test_support()
                    .text_size(px(text::LABEL))
                    .line_height(relative(1.45))
                    .text_color(if failed {
                        crate::palette::red(cx)
                    } else {
                        theme.muted_foreground
                    })
                    .child(status),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(theme.foreground.opacity(0.07))
                    .pt(px(12.))
                    .child(
                        Button::new("feedback-github")
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label("Report a bug on GitHub")
                            .on_click(cx.listener(|this, _, _, cx| this.file_on_github(cx))),
                    )
                    .child(div().flex_1())
                    .when(sending, |el| el.child(Spinner::new().xsmall()))
                    .when(can_send, |el| {
                        el.child(
                            Button::new("feedback-copy")
                                .outline()
                                .small()
                                .label("Copy report")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.copy_report(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("feedback-send")
                            .primary()
                            .small()
                            .label(match (&self.stage, can_send) {
                                (_, false) => "Copy report",
                                (Stage::NotSent(_), true) => "Try again",
                                _ => "Send",
                            })
                            .disabled(sending || reading)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
