//! Settings › Phone & Machines: the switch for the phone server, which of this Mac's addresses
//! phones use, the pairing code (a QR code, the code and fingerprint to type, and the link another
//! machine pastes), the paired phones and Trek's version on the iPhone, with "Update phone now";
//! then the other machines this one drives (`machines`), with "Add a machine…".

use super::SettingsView;
use crate::fluent;
use crate::palette;
use crate::scale::{radius, text, to_rems};
use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::{Reach, Settings};

/// What the page says of Trek's iPhone app before the phone server runs: it isn't on the App Store
/// yet, and any phone's browser does the same meanwhile.
const PHONE_APP_NOTE: &str = "Trek on iPhone shows every thread and what it needs from you, and lets you answer approvals and questions, review plans, send follow-ups and start threads, while the agents keep running here. It isn't on the App Store yet. Meanwhile any phone, iPhone or Android, can do the same in its browser: turn this on and pair a phone's browser.";

/// The paired devices as the page lists them: the iPhone app's pairings (one from before devices
/// said what they are is a phone's, as the sidebar has it), then the machines that paired with
/// this one.
fn phones_then_machines(
    devices: Vec<trek_remote::DeviceInfo>,
) -> (Vec<trek_remote::DeviceInfo>, Vec<trek_remote::DeviceInfo>) {
    devices
        .into_iter()
        .partition(trek_remote::DeviceInfo::is_phone)
}

/// A QR code as dark squares on white, each `module` points, with the standard quiet zone.
pub(crate) fn qr(url: &str, module: f32) -> AnyElement {
    qr_code(qrcode::QrCode::new(url.as_bytes()), module)
}

/// A longer link's (the web remote's) at the lowest error correction: a screen has no scratches
/// to read past, and fewer squares, each larger, scan sooner.
pub(crate) fn qr_low(url: &str, module: f32) -> AnyElement {
    qr_code(
        qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::L),
        module,
    )
}

fn qr_code(code: Result<qrcode::QrCode, qrcode::types::QrError>, module: f32) -> AnyElement {
    let Ok(code) = code else {
        return div()
            .child("The pairing link is too long for a QR code.")
            .into_any_element();
    };
    let width = code.width();
    let colors = code.to_colors();
    v_flex()
        .p(px(module * 4.))
        .bg(gpui_kit::white())
        .rounded(px(fluent::surface(radius::CARD)))
        .children(colors.chunks(width).map(|row| {
            h_flex().children(row.iter().map(|c| {
                div()
                    .size(px(module))
                    .flex_none()
                    .when(*c == qrcode::Color::Dark, |el| el.bg(rgb(0x111111)))
            }))
        }))
        .into_any_element()
}

impl SettingsView {
    /// Notifications on the phone: on its own switch. They go to each phone whose web remote asked
    /// for them (its browser's push service; no account, no other app), and through the ntfy app
    /// for whoever sets that up as well.
    fn push_section(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let this = crate::words::words().this_computer;
        let theme = cx.theme().clone();
        let m = &s.mobile;
        let ws = self.workspace.clone();
        // A phone's browser is reached through the phone server; ntfy isn't.
        let needs_server = if m.enabled {
            ""
        } else {
            " A phone's browser gets them only while Let your phone connect is on."
        };
        let mut out = vec![
            Self::heading("Notifications on your phone", cx),
            ui::group(
                vec![Self::row(
                    "Tell my phone when a thread needs me",
                    format!(
                        "Also when one finishes or fails: what {this} would alert you for. Tapping one opens the thread on the phone.{needs_server}"
                    ),
                    gpui_kit::component::switch::Switch::new("push-on")
                        .checked(m.push)
                        .on_click(move |v: &bool, _, cx| {
                            ws.update(cx, |ws, cx| ws.set_push(*v, cx))
                        }),
                    cx,
                )],
                cx,
            ),
        ];
        if !m.push {
            return out;
        }
        out.push(div().h(px(12.)).into_any_element());
        let names = self.switch("push-names", m.push_names, |s, v| s.mobile.push_names = v);
        out.push(ui::group(
            vec![
                Self::row(
                    "Send them",
                    format!("Away means no keyboard or mouse on {this} for two minutes, or the screen locked."),
                    ui::segmented("push-when", vec![(trek_core::settings::PushWhen::Away, "When I'm away"), (trek_core::settings::PushWhen::Always, "Always")], m.push_when, self.setter(|s, v| s.mobile.push_when = v), cx),
                    cx,
                ),
                Self::row(
                    "Name the thread and its project",
                    format!("Off, a notification says only that a thread needs you, finished or failed. On, it says which thread and project, as {this}'s own alerts do."),
                    names,
                    cx,
                ),
            ],
            cx,
        ));

        // The phones whose web remote asked for notifications, by name.
        let (phones, can_test) = {
            let ws = self.workspace.read(cx);
            let ids = ws.web_push_devices();
            let names: Vec<String> = ws
                .remote
                .as_ref()
                .map(|r| {
                    r.devices
                        .iter()
                        .filter(|d| ids.contains(&d.device_id))
                        .map(|d| d.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let can_test = !names.is_empty() || !m.push_topic.is_empty();
            (names, can_test)
        };
        let test = self.workspace.clone();
        let muted = theme.muted_foreground;
        out.push(div().h(px(12.)).into_any_element());
        out.push(
            v_flex()
                .id("push-web")
                .test_support()
                .gap(px(8.))
                .p(px(20.))
                .rounded(px(fluent::surface(radius::CARD)))
                .border_1()
                .border_color(theme.border)
                .text_size(px(text::BODY))
                .child(div().text_size(px(text::TITLE)).font_semibold().child("On your phone"))
                .child(if !m.enabled {
                    div()
                        .id("push-web-off")
                        .test_support()
                        .text_color(muted)
                        .child("No phone gets them while Let your phone connect is off. Turn it on above, then pair a phone's browser with Show pairing code › A phone's browser.")
                        .into_any_element()
                } else if phones.is_empty() {
                    div()
                        .id("push-web-none")
                        .test_support()
                        .text_color(muted)
                        .child("No phone gets them yet. Pair Trek's web remote with Show pairing code › A phone's browser, add it to the phone's Home Screen, and in its Settings tap Turn on notifications. On an iPhone, open it from the Home Screen first (iOS 26.4 or later).")
                        .into_any_element()
                } else {
                    div()
                        .id("push-web-phones")
                        .test_support()
                        .child(format!("Going to {}.", phones.join(", ")))
                        .into_any_element()
                })
                .child(div().text_size(to_rems(text::META)).text_color(muted).child("Sent by this computer through the phone's own push service, encrypted to the phone. No account, no other app."))
                .child(
                    h_flex().child(
                        Button::new("push-test")
                            .small()
                            .outline()
                            .label("Send a test")
                            .disabled(!can_test)
                            .on_click(move |_, _, cx| test.update(cx, |ws, cx| ws.test_push(cx))),
                    ),
                )
                .into_any_element(),
        );

        // ntfy: optional, for an app the user already has.
        let topic = m.push_topic.clone();
        let ntfy_on = !topic.is_empty();
        let toggle = self.workspace.clone();
        out.push(div().h(px(12.)).into_any_element());
        out.push(ui::group(
            vec![Self::row(
                "Also through the ntfy app",
                "Optional, for anyone who uses ntfy: Trek sends the same notes to a private topic there. The ntfy server can read what's sent.",
                gpui_kit::component::switch::Switch::new("push-ntfy")
                    .checked(ntfy_on)
                    .on_click(move |v: &bool, _, cx| toggle.update(cx, |ws, cx| ws.set_ntfy(*v, cx))),
                cx,
            )],
            cx,
        ));
        if !ntfy_on {
            return out;
        }
        let url = format!("{}/{}", m.push_server.trim_end_matches('/'), topic);
        let (copy_topic, fresh) = (topic.clone(), self.workspace.clone());
        out.push(div().h(px(12.)).into_any_element());
        out.push(
            h_flex()
                .id("push-setup")
                .test_support()
                .gap(px(28.))
                .p(px(20.))
                .rounded(px(fluent::surface(radius::CARD)))
                .border_1()
                .border_color(theme.border)
                .items_start()
                .child(qr(&url, 3.))
                .child(
                    v_flex()
                        .gap(px(10.))
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::BODY))
                        .child(
                            div()
                                .text_size(px(text::TITLE))
                                .font_semibold()
                                .child("Subscribe in ntfy"),
                        )
                        .child(
                            div()
                                .text_color(muted)
                                .child("In the ntfy app, tap + and subscribe to this topic:"),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(fluent::control(radius::BUTTON)))
                                .bg(theme.foreground.opacity(0.06))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(text::LABEL))
                                .child(topic.clone()),
                        )
                        .child(div().text_color(muted).child(
                            "Keep the topic to yourself: anyone who has it can read what's sent.",
                        ))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    Button::new("push-copy")
                                        .small()
                                        .ghost()
                                        .label("Copy topic")
                                        .on_click(move |_, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy_topic.clone(),
                                            ));
                                            crate::toast::push(window, "Copied", cx);
                                        }),
                                )
                                .child(
                                    Button::new("push-new-topic")
                                        .small()
                                        .ghost()
                                        .label("New topic")
                                        .on_click(move |_, _, cx| {
                                            fresh.update(cx, |ws, cx| {
                                                ws.settings.mobile.push_topic =
                                                    crate::push::new_topic();
                                                ws.save_settings(cx);
                                            })
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_size(to_rems(text::META))
                                .text_color(muted)
                                .child(format!("Through {}", m.push_server)),
                        ),
                )
                .into_any_element(),
        );
        out
    }

    pub(super) fn mobile_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let words = crate::words::words();
        let this = words.this_computer;
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let enabled = s.mobile.enabled;
        let running = ws.remote.as_ref().map(|r| {
            (
                r.offer.clone(),
                r.addresses.clone(),
                r.advertise.clone(),
                r.routes.clone(),
                r.devices.clone(),
                r.connected.clone(),
            )
        });
        let starting = enabled && running.is_none();
        let failed = ws.remote_error.clone().filter(|_| starting);
        let session_approvals = self.switch(
            "mobile-session-approvals",
            s.mobile.session_approvals,
            |s, v| s.mobile.session_approvals = v,
        );
        let phone_rules = self.switch("mobile-phone-rules", s.mobile.phone_rules, |s, v| {
            s.mobile.phone_rules = v
        });
        let mut rows = vec![Self::row(
            "Let your phone connect",
            "Trek listens for phones you've paired, on your network or tailnet: Trek on iPhone, or any phone's browser. Off, nothing listens.",
            self.switch("mobile-on", enabled, |s, v| s.mobile.enabled = v),
            cx,
        )];
        if enabled {
            rows.push(Self::row(
                "Let a phone's browser connect",
                format!("An Android phone, or any phone without Trek's app, pairs with its camera and uses Trek's web remote in its browser. It connects straight to {this}, over UDP on the same port. Off, only Trek on iPhone can."),
                self.switch("mobile-web", s.mobile.web, |s, v| s.mobile.web = v),
                cx,
            ));
            rows.push(Self::row(
                "Let it allow for a whole session",
                format!("Off, the phone can allow or deny what an agent asks, one request at a time. On, it can also allow that kind of request for the rest of the session. Only {this} changes this: Trek can't check from here who is holding the phone."),
                session_approvals,
                cx,
            ));
            rows.push(Self::row(
                "Let it write permission rules",
                format!("Off, the phone can read your rules in Settings › Permissions and try them out. On, it can also add and remove rules, and allow a request with a rule from its card. A rule lasts until it's removed. Only {this} changes this."),
                phone_rules,
                cx,
            ));
        }
        let mut out = vec![ui::group(rows, cx)];
        // Before the server runs, so it's read before the system asks (and still there if the
        // question went unanswered or the answer was no).
        if let (Some(firewall), None) = (words.firewall_note, &running) {
            out.push(div().h(px(8.)).into_any_element());
            out.push(Self::note(firewall, cx));
        }
        if let Some(why) = failed {
            out.push(Self::note(
                &format!(
                    "Couldn't start: {why}. Trying again every {} seconds.",
                    crate::remote::RETRY.as_secs()
                ),
                cx,
            ));
        } else if starting {
            out.push(Self::note("Starting…", cx));
        }
        out.extend(self.push_section(s, cx));
        let Some((offer, addresses, advertise, routes, devices, connected)) = running else {
            // The paired phones stay listed with the server off, so a lost one can be unpaired
            // without letting phones connect first.
            let devices = self.workspace.read(cx).paired_devices();
            out.push(div().h(px(16.)).into_any_element());
            out.push(Self::note(PHONE_APP_NOTE, cx));
            let phone_update = self.phone_update_section(&[], cx);
            out.extend(self.paired_sections(devices, &Default::default(), false, cx));
            out.extend(phone_update);
            out.extend(self.machines_section(cx));
            return out;
        };

        // Where phones reach this Mac.
        let reach_row = match addresses.tailscale {
            Some(ts) => Self::row(
                format!("Reach {this} over"),
                match (s.mobile.reach, routes.get(1)) {
                    (Reach::Wifi, Some(tailnet)) => format!(
                        "Phones dial {advertise} on this Wi-Fi, and {tailnet} over Tailscale from anywhere: a phone paired here keeps both, and uses whichever answers."
                    ),
                    (Reach::Wifi, None) => format!(
                        "Phones dial {advertise}. Wi-Fi works on the same network; Tailscale ({ts}) works anywhere your phone is on your tailnet."
                    ),
                    (Reach::Tailscale, _) => format!(
                        "Phones dial {advertise}, from anywhere your phone is on your tailnet. Nothing listens on the Wi-Fi."
                    ),
                },
                ui::segmented(
                    "mobile-reach",
                    vec![(Reach::Wifi, "Wi-Fi"), (Reach::Tailscale, "Tailscale")],
                    s.mobile.reach,
                    self.setter(|s, v| s.mobile.reach = v),
                    cx,
                ),
                cx,
            ),
            None => Self::row(
                "Reached at",
                format!(
                    "Phones on this Wi-Fi network dial this address. With Tailscale on {this} and your phone, you can choose your tailnet address here to reach it from anywhere."
                ),
                div()
                    .text_size(px(text::LABEL))
                    .font_family(theme.mono_font_family.clone())
                    .child(advertise.clone()),
                cx,
            ),
        };
        out.push(ui::group(vec![reach_row], cx));
        // Tailscale is up here, but a phone can only use it if it's in the same tailnet.
        if let Some(tailnet) = addresses
            .tailnet
            .as_ref()
            .filter(|_| addresses.tailscale.is_some())
        {
            let account = tailnet
                .account
                .as_deref()
                .map(|a| format!(" ({a})"))
                .unwrap_or_default();
            let note = if tailnet.phones == 0 {
                Some(format!(
                    "No iPhone is in your tailnet{account} yet, so phones reach {this} only on this Wi-Fi. Install Tailscale on your iPhone and sign in with the same account, then pair again here (or just reconnect: a paired phone learns the tailnet address)."
                ))
            } else if tailnet.phones_online == 0 {
                Some(format!(
                    "Your iPhone is in your tailnet{account}, but Tailscale isn't on on it right now. Turn it on in the Tailscale app to reach {this} away from this Wi-Fi."
                ))
            } else {
                None
            };
            if let Some(note) = note {
                out.push(div().h(px(8.)).into_any_element());
                out.push(Self::note(&note, cx));
            }
        }
        // Windows Firewall keeping phones out on a network this PC is on, which nothing else says
        // (`remote::firewall`): read when the page opens, and again on Check again. Only for a
        // server listening on a network (a loopback one is asked nothing).
        let on_a_network = addresses.lan.is_some() || addresses.tailscale.is_some();
        if let Some(note) = self
            .firewall
            .value
            .as_ref()
            .filter(|_| on_a_network)
            .and_then(|read| read.as_ref())
            .and_then(|firewall| firewall.note())
        {
            out.push(div().h(px(8.)).into_any_element());
            out.push(ui::group(
                vec![Self::row(
                    "Windows Firewall",
                    note,
                    h_flex()
                        .gap(px(8.))
                        .child(
                            Button::new("mobile-firewall-open")
                                .small()
                                .outline()
                                .label("Open firewall settings")
                                .on_click(|_, _, _| crate::remote::firewall::open_settings()),
                        )
                        .child(
                            Button::new("mobile-firewall-check")
                                .small()
                                .ghost()
                                .label("Check again")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.firewall.invalidate();
                                    cx.notify();
                                })),
                        ),
                    cx,
                )],
                cx,
            ));
        }

        // Pairing. Each card is drawn by a function of its own: an unoptimised build gives every
        // element's temporaries a slot in the frame of the function that builds them, and with
        // both cards here this page's frame overflowed the 1 MB main thread Windows gives Trek.
        out.push(Self::heading("Pair a phone", cx));
        let ws_entity = self.workspace.clone();
        match offer {
            Some(offer) => {
                // For a phone's browser: the web remote's link, which its camera opens.
                let browser = self.pair_in_browser && offer.web_url.is_some();
                if offer.web_url.is_some() {
                    out.push(self.pair_for(browser, cx));
                }
                out.push(if browser {
                    self.web_pairing_card(&offer, &advertise, cx)
                } else {
                    self.phone_pairing_card(&offer, &advertise, cx)
                });
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
                                .on_click(move |_, _, cx| {
                                    ws_entity.update(cx, |ws, cx| ws.offer_pairing(cx))
                                }),
                        )
                        .into_any_element(),
                );
            }
        }

        let phone_update = self.phone_update_section(&devices, cx);
        out.extend(self.paired_sections(devices, &connected, true, cx));
        out.extend(phone_update);
        out.push(div().h(px(16.)).into_any_element());
        out.push(Self::note(
            &format!("Connections are encrypted, and each phone checks it's talking to {this} (its certificate is in the pairing code). A phone can answer and steer threads but can't change how much agents may do or run Trek's own commands, and nothing is ever answered for you."),
            cx,
        ));
        out.extend(self.machines_section(cx));
        out
    }

    /// Paired phones, then the machines that paired with this one, each with Unpair: shown
    /// whether or not the phone server runs (`running`). Off, an unpaired device can't be told,
    /// so the page says what happens instead: it's turned away when it next connects.
    fn paired_sections(
        &mut self,
        devices: Vec<trek_remote::DeviceInfo>,
        connected: &std::collections::HashSet<String>,
        running: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let this = crate::words::words().this_computer;
        let mut out = vec![];
        // Paired phones: the iPhone app's pairings. A machine that paired with this one (to start
        // threads here) is a machine, not a phone: it's listed apart, where it can be unpaired too.
        let (phones, machines) = phones_then_machines(devices);
        let paired_row = |d: trek_remote::DeviceInfo, cx: &mut Context<Self>| {
            let online = connected.contains(&d.device_id);
            let seen = match (online, d.last_seen_at) {
                (true, _) => "Connected now".to_string(),
                (false, Some(at)) => format!("Last seen {}", crate::time::relative(at)),
                (false, None) => format!("Paired {}", crate::time::relative(d.paired_at)),
            };
            // A phone's browser says so: its name is the phone's ("Pixel 9 (Chrome)").
            let seen = if d.is_web() {
                format!("Web remote · {seen}")
            } else {
                seen
            };
            let ws = self.workspace.clone();
            let id = d.device_id.clone();
            Self::row(
                h_flex()
                    .gap(px(8.))
                    .child(div().size(px(7.)).rounded_full().bg(if online {
                        palette::emerald(cx)
                    } else {
                        theme.foreground.opacity(0.2)
                    }))
                    .child(d.name.clone()),
                seen,
                Button::new(SharedString::from(format!("mobile-revoke-{}", d.device_id)))
                    .small()
                    .ghost()
                    .label("Unpair")
                    .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.revoke_device(&id, cx))),
                cx,
            )
        };
        let any = !phones.is_empty() || !machines.is_empty();
        out.push(Self::heading("Paired phones", cx));
        if phones.is_empty() {
            out.push(
                div()
                    .id("mobile-paired-none")
                    .test_support()
                    .child(Self::note(
                        &if running {
                            format!("None yet. A paired phone shows your threads and answers what they ask, from Trek on iPhone or a phone's browser, while the agents keep running on {this}. Show pairing code above, then scan it with the phone.")
                        } else {
                            format!("None yet. A paired phone shows your threads and answers what they ask, from Trek on iPhone or a phone's browser, while the agents keep running on {this}. To pair one, turn on Let your phone connect above, then Show pairing code.")
                        },
                        cx,
                    ))
                    .into_any_element(),
            );
        } else {
            let rows = phones.into_iter().map(|d| paired_row(d, cx)).collect();
            out.push(ui::group(rows, cx));
        }
        if !machines.is_empty() {
            out.push(Self::heading(&format!("Machines paired with {this}"), cx));
            let rows = machines.into_iter().map(|d| paired_row(d, cx)).collect();
            out.push(ui::group(rows, cx));
        }
        if any && !running {
            out.push(div().h(px(8.)).into_any_element());
            out.push(
                div()
                    .id("mobile-unpair-offline")
                    .test_support()
                    .child(Self::note(
                        "Let your phone connect is off, so an unpaired phone or machine can't be told now. It's turned away the next time it tries to connect.",
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        out
    }

    /// The pairing code for a phone's browser: the web remote's link, as a QR code its camera opens
    /// and as a link to copy.
    #[inline(never)]
    fn web_pairing_card(
        &self,
        offer: &trek_remote::PairingOffer,
        advertise: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let this = crate::words::words().this_computer;
        let ws_entity = self.workspace.clone();
        let web_url = offer.web_url.clone().unwrap_or_default();
        let left = ((offer.expires_at - trek_core::store::now_ms()) / 60_000).max(0) + 1;
        let cancel = ws_entity.clone();
        let again = ws_entity.clone();
        h_flex()
        .id("mobile-pairing-web")
        .test_support()
        .gap(px(28.))
        .p(px(20.))
        .rounded(px(fluent::surface(radius::CARD)))
        .border_1()
        .border_color(theme.border)
        .items_start()
        .child(qr_low(&web_url, 3.))
        .child(
            v_flex()
                .gap(px(12.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::TITLE)).font_semibold().child("Scan this with your phone's camera"))
                .child(div().text_size(px(text::BODY)).text_color(theme.muted_foreground).child(format!("It opens Trek's web remote in the phone's browser, which pairs with {this} and connects straight to it. When the browser asks to look for devices on your network, allow it. Then add the page to the home screen to open it like an app.")))
                .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child(format!("Address {advertise} · works once, for about {left} min")))
                .child({
                    let url = web_url.clone();
                    v_flex()
                        .gap(px(4.))
                        .pt(px(4.))
                        .child(div().text_size(px(text::BODY)).font_medium().child("No camera?"))
                        .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child("Open this link on the phone (it holds the code, so send it only to yourself):"))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .id("pair-web-link")
                                        .test_support()
                                        .flex_1()
                                        .min_w_0()
                                        .px(px(8.))
                                        .py(px(5.))
                                        .rounded(px(fluent::control(radius::CONTROL)))
                                        .bg(theme.foreground.opacity(0.06))
                                        .font_family(theme.mono_font_family.clone())
                                        .text_size(px(text::META))
                                        .truncate()
                                        .child(web_url.clone()),
                                )
                                .child(Button::new("pair-web-link-copy").small().ghost().label("Copy link").on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                    crate::toast::push(window, "Copied", cx);
                                })),
                        )
                })
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(Button::new("mobile-new-code").small().outline().label("New code").on_click(move |_, _, cx| again.update(cx, |ws, cx| ws.offer_pairing(cx))))
                        .child(Button::new("mobile-cancel-code").small().ghost().label("Cancel").on_click(move |_, _, cx| cancel.update(cx, |ws, cx| ws.cancel_pairing(cx)))),
                ),
        )
        .into_any_element()
    }

    /// The pairing code for Trek on iPhone: a QR code, the code and fingerprint to type, and the
    /// link another machine pastes.
    #[inline(never)]
    fn phone_pairing_card(
        &self,
        offer: &trek_remote::PairingOffer,
        advertise: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let words = crate::words::words();
        let ws_entity = self.workspace.clone();
        let left = ((offer.expires_at - trek_core::store::now_ms()) / 60_000).max(0) + 1;
        let cancel = ws_entity.clone();
        let again = ws_entity.clone();
        h_flex()
        .id("mobile-pairing")
        .test_support()
        .gap(px(28.))
        .p(px(20.))
        .rounded(px(fluent::surface(radius::CARD)))
        .border_1()
        .border_color(theme.border)
        .items_start()
        .child(qr(&offer.url, 4.))
        .child(
            v_flex()
                .gap(px(12.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::TITLE)).font_semibold().child("Scan this with Trek on your iPhone"))
                .child(div().text_size(px(text::BODY)).text_color(theme.muted_foreground).child("No camera? Choose “Enter address and code” on the phone and type these:"))
                // The fingerprint goes under the code where the card is too narrow for both.
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_x(px(20.))
                        .gap_y(px(12.))
                        .child(
                            v_flex()
                                .gap(px(2.))
                                .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child("Code"))
                                .child(div().text_size(px(text::DISPLAY_S)).font_semibold().font_family(theme.mono_font_family.clone()).child(offer.code.clone())),
                        )
                        .when_some(offer.fingerprint.clone(), |el, fp| {
                            el.child(
                                v_flex()
                                    .id("pair-fingerprint")
                                    .test_support()
                                    .gap(px(2.))
                                    .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child(words.computer_fingerprint))
                                    .child(div().text_size(px(text::DISPLAY_S)).font_semibold().font_family(theme.mono_font_family.clone()).child(fp)),
                            )
                        }),
                )
                .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child(format!("Address {advertise} · works once, for about {left} min")))
                // Another computer running Trek pairs from the same code, pasted as a link.
                .child({
                    let url = offer.url.clone();
                    v_flex()
                        .gap(px(4.))
                        .pt(px(4.))
                        .child(div().text_size(px(text::BODY)).font_medium().child("Pair another machine"))
                        .child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child("In Trek on the other machine, choose Add a machine… and paste this link:"))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .id("pair-link")
                                        .test_support()
                                        .flex_1()
                                        .min_w_0()
                                        .px(px(8.))
                                        .py(px(5.))
                                        .rounded(px(fluent::control(radius::CONTROL)))
                                        .bg(theme.foreground.opacity(0.06))
                                        .font_family(theme.mono_font_family.clone())
                                        .text_size(px(text::META))
                                        .truncate()
                                        .child(offer.url.clone()),
                                )
                                .child(Button::new("pair-link-copy").small().ghost().label("Copy link").on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                    crate::toast::push(window, "Copied", cx);
                                })),
                        )
                })
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(Button::new("mobile-new-code").small().outline().label("New code").on_click(move |_, _, cx| again.update(cx, |ws, cx| ws.offer_pairing(cx))))
                        .child(Button::new("mobile-cancel-code").small().ghost().label("Cancel").on_click(move |_, _, cx| cancel.update(cx, |ws, cx| ws.cancel_pairing(cx)))),
                ),
        )
        .into_any_element()
    }

    /// Which phone the pairing code is for: Trek on iPhone, or a phone's browser (the web
    /// remote, whose code is a link its camera opens).
    fn pair_for(&self, browser: bool, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        h_flex()
            .pb(px(10.))
            .child(ui::segmented(
                "mobile-pair-for",
                vec![(false, "Trek on iPhone"), (true, "A phone's browser")],
                browser,
                move |v, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.pair_in_browser = v;
                        cx.notify();
                    });
                },
                cx,
            ))
            .into_any_element()
    }

    /// Trek on the iPhone: its version, how the last update went (or the one going now), and
    /// "Update phone now", which builds and installs it as the phone's own "Update now" does
    /// (`remote::phone_update`). Only where the iPhone app builds (Xcode), and once there's an
    /// iPhone to speak of.
    fn phone_update_section(
        &mut self,
        devices: &[trek_remote::DeviceInfo],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use crate::remote::phone_update;
        use trek_remote::PhoneUpdateState;
        if !cfg!(target_os = "macos") {
            return vec![];
        }
        let phone = phone_update::phone_version(devices);
        let last = phone_update::last_attempt();
        if phone.is_none() && last.is_none() {
            return vec![];
        }
        let ws = self.workspace.read(cx);
        let running = ws.phone_update.running;
        let now = ws.phone_update.now.clone();
        let title = match &phone {
            Some((version, Some(name))) => format!("Trek {version} on {name}"),
            Some((version, None)) => format!("Trek {version} on your iPhone"),
            None => "Trek on your iPhone".to_string(),
        };
        let version =
            |v: &Option<String>| v.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        let line = match (&now, running, &last) {
            (Some(step), true, _) => match step.state {
                PhoneUpdateState::Building => format!("Building Trek{}…", version(&step.version)),
                PhoneUpdateState::Installing => {
                    format!("Installing Trek{} on the iPhone…", version(&step.version))
                }
                PhoneUpdateState::Installed => format!("Installed Trek{}.", version(&step.version)),
                PhoneUpdateState::Failed => step.message.clone().unwrap_or_default(),
            },
            (None, true, _) => "Starting…".to_string(),
            // What the run from here ended with, until the updater's record says otherwise.
            (Some(step), false, last)
                if step.state == PhoneUpdateState::Failed
                    && last.as_ref().is_none_or(|l| l.ok) =>
            {
                step.message.clone().unwrap_or_default()
            }
            (_, false, Some(last)) => format!(
                "{} {} · {}",
                if last.ok { "Last update:" } else { "Last update failed:" },
                last.line,
                crate::time::relative(last.at)
            ),
            (_, false, None) => {
                "Kept current every half hour while it's in reach, by the phone updater (ios/scripts/install-phone-update.sh).".to_string()
            }
        };
        let ws_entity = self.workspace.clone();
        let button = Button::new("mobile-update-phone")
            .small()
            .outline()
            .label("Update phone now")
            // A run already going is joined, not started again (`update_phone`).
            .loading(running)
            .on_click(move |_, window, cx| {
                if let Err(e) = ws_entity.update(cx, |ws, cx| ws.update_phone(None, cx)) {
                    crate::toast::push(window, e.as_str(), cx);
                }
            });
        vec![ui::group(vec![Self::row(title, line, button, cx)], cx)]
    }

    /// The other machines running Trek that this one drives: each with a dot while it's reachable,
    /// when it was last seen, and Forget; then "Add a machine…".
    fn machines_section(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let this = crate::words::words().this_computer;
        let mut out = vec![Self::heading("Machines", cx)];
        let ws = self.workspace.read(cx);
        let machines: Vec<(String, String, bool, String)> = ws
            .machines
            .list
            .iter()
            .map(|m| {
                let route = m.entry.paired.routes.first().cloned().unwrap_or_default();
                // Where it stands: connected on which address, or since when it isn't, and
                // why and what to do when Trek can tell.
                let seen = match crate::machines::offline_note(m, &ws.machines.here, this) {
                    None => format!("Connected now · {route}"),
                    Some(note) => note,
                };
                (
                    m.id().to_string(),
                    m.entry.name.clone(),
                    m.reachable(),
                    seen,
                )
            })
            .collect();
        if machines.is_empty() {
            out.push(Self::note(&format!("Pair another computer running Trek to start threads on it from here: they run there, and show here in the sidebar under its name, as live as {this}'s own."), cx));
        } else {
            let rows = machines
                .into_iter()
                .map(|(id, name, reachable, seen)| {
                    let ws = self.workspace.clone();
                    let host = id.clone();
                    Self::row(
                        h_flex()
                            .id(SharedString::from(format!("machine-row-{id}")))
                            .test_support()
                            .gap(px(8.))
                            .child(
                                crate::composer::reach_dot(reachable, cx)
                                    .id(SharedString::from(format!("machine-dot-{id}")))
                                    .test_support(),
                            )
                            .child(name),
                        seen,
                        Button::new(SharedString::from(format!("machine-forget-{id}")))
                            .small()
                            .ghost()
                            .label("Forget")
                            .on_click(move |_, _, cx| {
                                ws.update(cx, |ws, cx| ws.forget_machine(&host, cx))
                            }),
                        cx,
                    )
                })
                .collect();
            out.push(ui::group(rows, cx));
        }
        let ws = self.workspace.clone();
        out.push(
            h_flex()
                .pt(px(12.))
                .child(
                    Button::new("machine-add")
                        .small()
                        .outline()
                        .icon(Icon::new(crate::assets::Lucide::Laptop))
                        .label("Add a machine…")
                        .on_click(move |_, window, cx| {
                            crate::machines::sheets::open_pair(ws.clone(), window, cx)
                        }),
                )
                .into_any_element(),
        );
        out.push(div().h(px(12.)).into_any_element());
        out.push(div().text_size(px(text::LABEL)).text_color(theme.muted_foreground).child("A machine is paired like a phone: it lists this one among its paired devices, where it can be unpaired.").into_any_element());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::phones_then_machines;

    /// A Mac (or PC) that paired with this one to start threads here is listed with the
    /// machines, not under Paired phones.
    #[test]
    fn paired_machines_are_not_phones() {
        let device = |id: &str, platform: Option<&str>| trek_remote::DeviceInfo {
            device_id: id.into(),
            name: id.into(),
            paired_at: 0,
            last_seen_at: None,
            app_version: None,
            platform: platform.map(str::to_string),
        };
        let (phones, machines) = phones_then_machines(vec![
            device("iphone", Some("ios")),
            device("pixel", Some("web")),
            device("macbook", Some("macos")),
            device("old-iphone", None),
            device("desktop", Some("windows")),
        ]);
        let ids = |v: &[trek_remote::DeviceInfo]| {
            v.iter().map(|d| d.device_id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(&phones), ["iphone", "pixel", "old-iphone"]);
        assert_eq!(ids(&machines), ["macbook", "desktop"]);
    }
}
