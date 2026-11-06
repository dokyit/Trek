//! Settings › Privacy: whether anonymous usage data is shared (the answer to the card at
//! launch), what is and never is, the install id and a way to make a new one, what went out
//! last (exactly, as it was sent), and feedback, which works either way.

use super::SettingsView;
use crate::scale::{radius, text};
use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use trek_telemetry::{Blocked, Consent};

/// What was last sent is on show.
static SHOWING_SENT: AtomicBool = AtomicBool::new(false);

/// docs/PRIVACY.md, where every event and what it carries is listed.
const DETAILS: &str = "https://github.com/dokyit/Trek/blob/main/docs/PRIVACY.md";

impl SettingsView {
    pub(super) fn privacy_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let status = trek_telemetry::status();
        let on = status.as_ref().is_some_and(|s| s.consent == Consent::Yes);
        let blocked = status.as_ref().and_then(|s| s.blocked);
        let keyless = status.as_ref().is_none_or(|s| s.key.is_none());
        let share_line = match (blocked, keyless) {
            (Some(Blocked::DoNotTrack), _) => {
                "Off: DO_NOT_TRACK is set, so nothing is shared.".to_string()
            }
            (Some(Blocked::Isolated), _) => {
                "This is a test profile, so nothing is sent whatever this says.".to_string()
            }
            (None, true) => {
                "This build has nowhere to send it yet, so nothing is sent.".to_string()
            }
            (None, false) => "Sent in the background, a few events a day.".to_string(),
        };
        let consent = Switch::new("privacy-consent")
            .checked(on && blocked != Some(Blocked::DoNotTrack))
            .disabled(status.is_none() || blocked == Some(Blocked::DoNotTrack))
            .on_click(|yes: &bool, _, cx| {
                trek_telemetry::set_consent(*yes);
                cx.refresh_windows();
            });
        let mut out = vec![ui::group(
            vec![
                Self::row("Share anonymous usage data", share_line, consent, cx),
                Self::row("What's shared", crate::privacy::SHARED, div(), cx),
                Self::row("What never is", crate::privacy::NEVER, div(), cx),
            ],
            cx,
        )];

        out.push(Self::heading("This install", cx));
        let id = status.as_ref().and_then(|s| s.install_id.clone());
        let id_line = match &id {
            Some(id) => format!(
                "{id}. Random, and nothing else is known of this install. Reset makes a new one."
            ),
            None => "None yet: it's made when you say yes.".to_string(),
        };
        let reset = Button::new("privacy-reset")
            .small()
            .outline()
            .label("Reset")
            .disabled(id.is_none())
            .on_click(|_, _, cx| {
                trek_telemetry::reset_install_id();
                cx.refresh_windows();
            });
        let destination = match status.as_ref().and_then(|s| s.key.as_ref()) {
            Some(key) => {
                let from = key
                    .file
                    .as_ref()
                    .map(|f| format!(", with the key in {}", trek_core::paths::tildify(f)))
                    .unwrap_or_default();
                format!("PostHog ({}){from}.", key.host_name())
            }
            None => "Nowhere yet: this build has no PostHog key.".to_string(),
        };
        let mut rows = vec![
            Self::row("Install ID", id_line, reset, cx),
            Self::row("Where it goes", destination, div(), cx),
        ];
        if let Some(s) = status.as_ref().filter(|s| s.waiting > 0) {
            let mut line = match s.waiting {
                1 => "1 event, sent when Trek can reach PostHog.".to_string(),
                n => format!("{n} events, sent when Trek can reach PostHog."),
            };
            if let Some(why) = &s.last_error {
                line.push_str(&format!(" The last try failed: {why}."));
            }
            rows.push(Self::row("Waiting to send", line, div(), cx));
        }
        out.push(ui::group(rows, cx));

        out.push(Self::heading("What was sent", cx));
        let sent = status.as_ref().and_then(|s| s.last_sent.clone());
        let showing = SHOWING_SENT.load(Ordering::Relaxed) && sent.is_some();
        let sent_line = match &sent {
            Some(s) => format!(
                "{}, {} {}, exactly as it went.",
                super::relative_ago(s.at * 1000),
                s.events,
                if s.events == 1 { "event" } else { "events" }
            ),
            None => "Nothing has been sent from here.".to_string(),
        };
        let show = Button::new("privacy-show-sent")
            .small()
            .outline()
            .label(if showing { "Hide" } else { "Show" })
            .disabled(sent.is_none())
            .on_click(|_, _, cx| {
                SHOWING_SENT.fetch_xor(true, Ordering::Relaxed);
                cx.refresh_windows();
            });
        out.push(ui::group(
            vec![Self::row("Last sent", sent_line, show, cx)],
            cx,
        ));
        if let Some(sent) = sent.filter(|_| showing) {
            let body = sent.body.clone();
            out.push(
                div()
                    .pt(px(10.))
                    .child(
                        div()
                            .id("privacy-sent-body")
                            .test_support()
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .p(px(12.))
                            .rounded(px(crate::fluent::control(radius::BUTTON)))
                            .bg(crate::composer::look::chip_fill(cx))
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(text::LABEL))
                            .line_height(relative(1.45))
                            .child(sent.body.clone()),
                    )
                    .child(
                        h_flex().pt(px(8.)).gap(px(8.)).child(
                            Button::new("privacy-copy-sent")
                                .small()
                                .ghost()
                                .icon(IconName::Copy)
                                .label("Copy")
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(body.clone()));
                                    crate::toast::push(window, "Copied what was sent", cx);
                                }),
                        ),
                    )
                    .into_any_element(),
            );
        }

        out.push(Self::heading("Feedback", cx));
        out.push(ui::group(
            vec![Self::row(
                "Send feedback",
                "A bug, an idea, or anything else. It works whether or not you share usage data.",
                Button::new("privacy-feedback")
                    .small()
                    .outline()
                    .label("Send feedback…")
                    .on_click(|_, _, cx| crate::feedback::open_in_main(cx)),
                cx,
            )],
            cx,
        ));
        out.push(
            h_flex()
                .pt(px(14.))
                .child(
                    Button::new("privacy-details")
                        .small()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label("Every event and what it carries")
                        .on_click(|_, _, cx| cx.open_url(DETAILS)),
                )
                .into_any_element(),
        );
        out
    }
}
