//! Settings › Phone: the switch for the phone server, which of this Mac's addresses phones use,
//! the pairing code (a QR code, and the code and fingerprint to type), and the paired phones.

use super::SettingsView;
use crate::palette;
use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::{Reach, Settings};

/// A QR code as dark squares on white, each `module` points, with the standard quiet zone.
fn qr(url: &str, module: f32) -> AnyElement {
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else {
        return div().child("The pairing link is too long for a QR code.").into_any_element();
    };
    let width = code.width();
    let colors = code.to_colors();
    v_flex()
        .p(px(module * 4.))
        .bg(gpui_kit::white())
        .rounded(px(10.))
        .children(colors.chunks(width).map(|row| {
            h_flex().children(row.iter().map(|c| {
                div().size(px(module)).flex_none().when(*c == qrcode::Color::Dark, |el| el.bg(rgb(0x111111)))
            }))
        }))
        .into_any_element()
}

impl SettingsView {
    /// Notifications on the phone, through the ntfy app: on its own switch, as it works without
    /// the phone server.
    fn push_section(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let m = &s.mobile;
        let ws = self.workspace.clone();
        let mut out = vec![
            Self::heading("Notifications on your phone", cx),
            ui::group(
                vec![Self::row(
                    "Tell my phone when a thread needs me",
                    "Also when one finishes or fails: what this Mac would alert you for, sent through the free ntfy app. Tapping one opens the thread in Trek on iPhone.",
                    gpui_kit::component::switch::Switch::new("push-on").checked(m.push).on_click(move |v: &bool, _, cx| ws.update(cx, |ws, cx| ws.set_push(*v, cx))),
                    cx,
                )],
                cx,
            ),
        ];
        if !m.push {
            return out;
        }
        let topic = m.push_topic.clone();
        let url = format!("{}/{}", m.push_server.trim_end_matches('/'), topic);
        out.push(div().h(px(12.)).into_any_element());
        out.push(ui::group(
            vec![Self::row(
                "Send them",
                "Away means no keyboard or mouse on this Mac for two minutes, or the screen locked.",
                ui::segmented("push-when", vec![(trek_core::settings::PushWhen::Away, "When I'm away"), (trek_core::settings::PushWhen::Always, "Always")], m.push_when, self.setter(|s, v| s.mobile.push_when = v), cx),
                cx,
            )],
            cx,
        ));
        let (copy_topic, test, fresh) = (topic.clone(), self.workspace.clone(), self.workspace.clone());
        out.push(div().h(px(12.)).into_any_element());
        out.push(
            h_flex()
                .id("push-setup")
                .test_support()
                .gap(px(28.))
                .p(px(20.))
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .items_start()
                .child(qr(&url, 3.))
                .child(
                    v_flex()
                        .gap(px(10.))
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .child(div().text_size(px(13.5)).font_semibold().child("Set up ntfy on your iPhone"))
                        .child(div().text_color(theme.muted_foreground).child("1. Install ntfy from the App Store."))
                        .child(div().text_color(theme.muted_foreground).child("2. In ntfy, tap + and subscribe to this topic:"))
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(7.))
                                .bg(theme.foreground.opacity(0.06))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(12.))
                                .child(topic.clone()),
                        )
                        .child(div().text_color(theme.muted_foreground).child("3. Send a test. Keep the topic to yourself: anyone who has it can read what's sent."))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(Button::new("push-test").small().outline().label("Send a test").on_click(move |_, _, cx| test.update(cx, |ws, cx| ws.test_push(cx))))
                                .child(Button::new("push-copy").small().ghost().label("Copy topic").on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy_topic.clone()));
                                    gpui_kit::component::WindowExt::push_notification(window, "Copied", cx);
                                }))
                                .child(Button::new("push-new-topic").small().ghost().label("New topic").on_click(move |_, _, cx| {
                                    fresh.update(cx, |ws, cx| {
                                        ws.settings.mobile.push_topic = crate::push::new_topic();
                                        ws.save_settings(cx);
                                    })
                                })),
                        )
                        .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Through {}", m.push_server))),
                )
                .into_any_element(),
        );
        out
    }

    pub(super) fn mobile_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let enabled = s.mobile.enabled;
        let running = ws.remote.as_ref().map(|r| (r.offer.clone(), r.addresses.clone(), r.advertise.clone(), r.devices.clone(), r.connected.clone()));
        let starting = enabled && running.is_none();
        let mut out = vec![ui::group(
            vec![Self::row(
                "Let your iPhone connect",
                "Trek listens for phones you've paired, on your network or tailnet. Off, nothing listens.",
                self.switch("mobile-on", enabled, |s, v| s.mobile.enabled = v),
                cx,
            )],
            cx,
        )];
        if starting {
            out.push(Self::note("Starting…", cx));
        }
        out.extend(self.push_section(s, cx));
        let Some((offer, addresses, advertise, devices, connected)) = running else {
            out.push(div().h(px(16.)).into_any_element());
            out.push(Self::note(
                "Trek on iPhone shows every thread and what it needs from you, and lets you answer approvals and questions, review plans, send follow-ups and start threads, while the agents keep running here. Build it from ios/ in Trek's repository; it isn't on the App Store yet.",
                cx,
            ));
            return out;
        };

        // Where phones reach this Mac.
        let reach_row = match addresses.tailscale {
            Some(ts) => Self::row(
                "Reach this Mac over",
                format!("Phones dial {advertise}. Wi-Fi works on the same network; Tailscale ({ts}) works anywhere your phone is on your tailnet."),
                ui::segmented("mobile-reach", vec![(Reach::Wifi, "Wi-Fi"), (Reach::Tailscale, "Tailscale")], s.mobile.reach, self.setter(|s, v| s.mobile.reach = v), cx),
                cx,
            ),
            None => Self::row(
                "Reached at",
                "Phones on this Wi-Fi network dial this address. With Tailscale on this Mac and your phone, you can choose your tailnet address here to reach it from anywhere.",
                div().text_size(px(12.5)).font_family(theme.mono_font_family.clone()).child(advertise.clone()),
                cx,
            ),
        };
        out.push(ui::group(vec![reach_row], cx));

        // Pairing.
        out.push(Self::heading("Pair an iPhone", cx));
        let ws_entity = self.workspace.clone();
        match offer {
            Some(offer) => {
                let left = ((offer.expires_at - trek_core::store::now_ms()) / 60_000).max(0) + 1;
                let cancel = ws_entity.clone();
                let again = ws_entity.clone();
                out.push(
                    h_flex()
                        .id("mobile-pairing")
                        .test_support()
                        .gap(px(28.))
                        .p(px(20.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme.border)
                        .items_start()
                        .child(qr(&offer.url, 4.))
                        .child(
                            v_flex()
                                .gap(px(12.))
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(13.5)).font_semibold().child("Scan this with Trek on your iPhone"))
                                .child(div().text_size(px(12.5)).text_color(theme.muted_foreground).child("No camera? Choose “Enter address and code” on the phone and type these:"))
                                .child(
                                    h_flex()
                                        .gap(px(20.))
                                        .child(
                                            v_flex()
                                                .gap(px(2.))
                                                .child(div().text_xs().text_color(theme.muted_foreground).child("Code"))
                                                .child(div().text_size(px(20.)).font_semibold().font_family(theme.mono_font_family.clone()).child(offer.code.clone())),
                                        )
                                        .when_some(offer.fingerprint.clone(), |el, fp| {
                                            el.child(
                                                v_flex()
                                                    .gap(px(2.))
                                                    .child(div().text_xs().text_color(theme.muted_foreground).child("This Mac's fingerprint"))
                                                    .child(div().text_size(px(20.)).font_semibold().font_family(theme.mono_font_family.clone()).child(fp)),
                                            )
                                        }),
                                )
                                .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Address {advertise} · works once, for about {left} min")))
                                .child(
                                    h_flex()
                                        .gap(px(8.))
                                        .child(Button::new("mobile-new-code").small().outline().label("New code").on_click(move |_, _, cx| again.update(cx, |ws, cx| ws.offer_pairing(cx))))
                                        .child(Button::new("mobile-cancel-code").small().ghost().label("Cancel").on_click(move |_, _, cx| cancel.update(cx, |ws, cx| ws.cancel_pairing(cx)))),
                                ),
                        )
                        .into_any_element(),
                );
            }
            None => {
                out.push(
                    h_flex()
                        .child(
                            Button::new("mobile-pair")
                                .small()
                                .outline()
                                .icon(Icon::new(crate::assets::Lucide::Smartphone))
                                .label("Show pairing code")
                                .on_click(move |_, _, cx| ws_entity.update(cx, |ws, cx| ws.offer_pairing(cx))),
                        )
                        .into_any_element(),
                );
            }
        }

        // Paired phones.
        out.push(Self::heading("Paired phones", cx));
        if devices.is_empty() {
            out.push(Self::note("None yet.", cx));
        } else {
            let rows = devices
                .into_iter()
                .map(|d| {
                    let online = connected.contains(&d.device_id);
                    let seen = match (online, d.last_seen_at) {
                        (true, _) => "Connected now".to_string(),
                        (false, Some(at)) => format!("Last seen {}", crate::time::relative(at)),
                        (false, None) => format!("Paired {}", crate::time::relative(d.paired_at)),
                    };
                    let ws = self.workspace.clone();
                    let id = d.device_id.clone();
                    Self::row(
                        h_flex()
                            .gap(px(8.))
                            .child(div().size(px(7.)).rounded_full().bg(if online { palette::emerald(cx) } else { theme.foreground.opacity(0.2) }))
                            .child(d.name.clone()),
                        seen,
                        Button::new(SharedString::from(format!("mobile-revoke-{}", d.device_id)))
                            .small()
                            .ghost()
                            .label("Unpair")
                            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.revoke_device(&id, cx))),
                        cx,
                    )
                })
                .collect();
            out.push(ui::group(rows, cx));
        }
        out.push(div().h(px(16.)).into_any_element());
        out.push(Self::note(
            "Connections are encrypted, and each phone checks it's talking to this Mac (its certificate is in the pairing code). A phone can answer and steer threads but can't change how much agents may do or run Trek's own commands, and nothing is ever answered for you.",
            cx,
        ));
        out
    }
}
