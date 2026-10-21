//! The phone's requests, answered by the workspace as the server hands them over.

use super::harness::{open, run};
use crate::workspace::Route;
use trek_core::{HandHolding, RunState};
use trek_remote as tr;

/// Make a request the way the server does and take the reply.
fn ask<T>(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, make: impl FnOnce(tr::Reply<T>) -> tr::HostRequest) -> tr::HostResult<T> {
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    trek.update(cx, |ws, cx| ws.remote_request(make(reply), cx));
    cx.run_until_parked();
    rx.try_recv().expect("answered")
}

/// `ask` for requests answered off the main thread (git, Basecamp, usage): waits, in real time.
async fn ask_later<T>(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, make: impl FnOnce(tr::Reply<T>) -> tr::HostRequest) -> tr::HostResult<T> {
    let (reply, mut rx) = tokio::sync::oneshot::channel();
    trek.update(cx, |ws, cx| ws.remote_request(make(reply), cx));
    let deadline = std::time::Instant::now() + super::harness::REAL_TIME_LIMIT;
    loop {
        cx.run_until_parked();
        if let Ok(answer) = rx.try_recv() {
            return answer;
        }
        assert!(std::time::Instant::now() < deadline, "no answer");
        cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
    }
}

#[test]
fn tailscale_reach_requires_and_points_at_its_address() {
    let none = crate::remote::Addresses { lan: Some("192.168.1.2".parse().unwrap()), tailscale: None, tailnet: None };
    assert_eq!(crate::remote::remote_endpoint(&none, trek_core::settings::Reach::Tailscale, 7420).unwrap_err().kind(), std::io::ErrorKind::AddrNotAvailable);
    let addresses = crate::remote::Addresses { lan: none.lan, tailscale: Some("100.64.1.2".parse().unwrap()), tailnet: None };
    let (bind, advertise) = crate::remote::remote_endpoint(&addresses, trek_core::settings::Reach::Tailscale, 7420).unwrap();
    assert_eq!((bind.to_string(), advertise.as_str()), ("100.64.1.2:7420".into(), "100.64.1.2:7420"));
    // Chosen, the tailnet is all phones are told of: not the Wi-Fi it's on as well.
    assert_eq!(crate::remote::other_endpoint(&addresses, trek_core::settings::Reach::Tailscale, 7420), None);
    assert_eq!(crate::remote::remote_routes(&addresses, trek_core::settings::Reach::Tailscale, 7420).unwrap(), ["100.64.1.2:7420"]);
}

#[test]
fn on_wifi_the_tailnet_is_a_second_way_in() {
    use crate::remote::{Addresses, other_endpoint, remote_routes};
    use trek_core::settings::Reach;
    let lan = Some("192.168.1.2".parse().unwrap());
    let tailscale = Some("100.101.1.2".parse().unwrap());
    // Both up: phones are told both, the Wi-Fi first.
    let both = Addresses { lan, tailscale, tailnet: None };
    assert_eq!(other_endpoint(&both, Reach::Wifi, 7420).map(|a| a.to_string()).as_deref(), Some("100.101.1.2:7420"));
    assert_eq!(remote_routes(&both, Reach::Wifi, 7420).unwrap(), ["192.168.1.2:7420", "100.101.1.2:7420"]);
    // One of them: just that one, never the same address twice.
    assert_eq!(remote_routes(&Addresses { lan: None, tailscale, tailnet: None }, Reach::Wifi, 7420).unwrap(), ["100.101.1.2:7420"]);
    assert_eq!(remote_routes(&Addresses { lan, tailscale: None, tailnet: None }, Reach::Wifi, 7420).unwrap(), ["192.168.1.2:7420"]);
}

#[test]
fn the_tailnet_says_whether_an_iphone_is_in_it() {
    use crate::remote::Tailnet;
    let status = |peers: &str| format!(r#"{{"Self":{{"UserID":7,"OS":"macOS"}},"User":{{"7":{{"LoginName":"dokyit@"}}}},"Peer":{{{peers}}}}}"#);
    let alone = Tailnet::parse(&status("")).unwrap();
    assert_eq!(alone, Tailnet { account: Some("dokyit@".into()), phones: 0, phones_online: 0 });
    let asleep = Tailnet::parse(&status(r#""a":{"OS":"iOS","Online":false},"b":{"OS":"linux","Online":true}"#)).unwrap();
    assert_eq!((asleep.phones, asleep.phones_online), (1, 0));
    let on = Tailnet::parse(&status(r#""a":{"OS":"iOS","Online":true}"#)).unwrap();
    assert_eq!((on.phones, on.phones_online), (1, 1));
    assert_eq!(Tailnet::parse("not json"), None);
}

#[test]
fn the_pairing_code_points_at_the_wifi_then_the_tailnet_then_this_mac() {
    use crate::remote::{Addresses, remote_endpoint};
    let wifi = |addresses: &Addresses| {
        let (bind, advertise) = remote_endpoint(addresses, trek_core::settings::Reach::Wifi, 7420).unwrap();
        assert_eq!(bind.to_string(), advertise);
        advertise
    };
    let lan = Some("192.168.1.2".parse().unwrap());
    let tailscale = Some("100.101.1.2".parse().unwrap());
    assert_eq!(wifi(&Addresses { lan, tailscale, tailnet: None }), "192.168.1.2:7420");
    // Without Wi-Fi, the tailnet address; with neither, only this Mac.
    assert_eq!(wifi(&Addresses { lan: None, tailscale, tailnet: None }), "100.101.1.2:7420");
    assert_eq!(wifi(&Addresses::default()), "127.0.0.1:7420");
}

#[test]
fn tailscale_is_the_tunnel_it_names_not_any_shared_address() {
    use crate::remote::Addresses;
    let ifs = |list: &[(&str, &str)]| -> Vec<(String, std::net::Ipv4Addr)> { list.iter().map(|(n, ip)| (n.to_string(), ip.parse().unwrap())).collect() };
    // Carrier-grade NAT on Wi-Fi (a hotspot) is neither the LAN nor Tailscale.
    let hotspot = ifs(&[("en0", "100.72.3.4")]);
    assert_eq!(Addresses::from_interfaces(&hotspot, None), Addresses::default());
    // An IPv6-only hotspot leaves the Mac only macOS's own 192.0.0.2: nothing a phone can dial.
    assert_eq!(Addresses::from_interfaces(&ifs(&[("en0", "192.0.0.2")]), None), Addresses::default());
    // A tunnel in that range is; a VPN's private tunnel address isn't the LAN.
    let tailnet = ifs(&[("utun3", "10.8.0.2"), ("en0", "192.168.1.2"), ("utun4", "100.101.1.2")]);
    let found = Addresses::from_interfaces(&tailnet, None);
    assert_eq!((found.lan, found.tailscale), (Some("192.168.1.2".parse().unwrap()), Some("100.101.1.2".parse().unwrap())));
    // What Tailscale says wins, when this Mac has that address.
    let two = ifs(&[("utun2", "100.64.0.9"), ("utun4", "100.101.1.2")]);
    assert_eq!(Addresses::from_interfaces(&two, Some("100.101.1.2".parse().unwrap())).tailscale, Some("100.101.1.2".parse().unwrap()));
    assert_eq!(Addresses::from_interfaces(&two, Some("100.99.9.9".parse().unwrap())).tailscale, Some("100.64.0.9".parse().unwrap()));
}

#[test]
fn windows_adapters_are_told_apart_by_their_friendly_names() {
    use crate::remote::Addresses;
    let ifs = |list: &[(&str, &str)]| -> Vec<(String, std::net::Ipv4Addr)> { list.iter().map(|(n, ip)| (n.to_string(), ip.parse().unwrap())).collect() };
    let pc = ifs(&[("Ethernet", "169.254.3.4"), ("Wi-Fi", "192.168.1.20"), ("Tailscale", "100.101.1.2"), ("Local Area Connection* 9", "192.168.137.1")]);
    let found = Addresses::from_interfaces(&pc, None);
    // The Wi-Fi is the first private address that isn't a tunnel; the adapter's own name makes
    // the tailnet address Tailscale's (case aside), whatever the CLI said or couldn't.
    assert_eq!((found.lan, found.tailscale), (Some("192.168.1.20".parse().unwrap()), Some("100.101.1.2".parse().unwrap())));
    assert_eq!(Addresses::from_interfaces(&ifs(&[("tailscale0", "100.101.1.2")]), None).tailscale, Some("100.101.1.2".parse().unwrap()));
    // Tailscale's adapter is never taken for the LAN, even with a private address on it, and a
    // 100.x on the Wi-Fi (a hotspot's carrier-grade NAT) is not a tailnet.
    let tunnel_only = ifs(&[("Tailscale", "192.168.99.1")]);
    assert_eq!(Addresses::from_interfaces(&tunnel_only, None), Addresses::default());
    assert_eq!(Addresses::from_interfaces(&ifs(&[("Wi-Fi", "100.72.3.4")]), None), Addresses::default());
    // A localised name whose ninth byte falls inside a character doesn't trip the prefix check.
    assert_eq!(Addresses::from_interfaces(&ifs(&[("Локальная сеть", "192.168.1.30")]), None).lan, Some("192.168.1.30".parse().unwrap()));
}

#[test]
fn the_default_route_wins_over_a_virtual_machine_adapter() {
    use crate::remote::Addresses;
    let ifs = |list: &[(&str, &str)]| -> Vec<(String, std::net::Ipv4Addr)> { list.iter().map(|(n, ip)| (n.to_string(), ip.parse().unwrap())).collect() };
    // if_addrs does not consistently expose Windows' friendly adapter name. Even when the
    // host-only adapter is simply called Ethernet and enumerated first, the address Windows uses
    // for its default route is the one another computer on the LAN can reach.
    let pc = ifs(&[("Ethernet", "192.168.56.1"), ("Wi-Fi", "192.168.1.20")]);
    let found = Addresses::from_interfaces_preferring(&pc, None, Some("192.168.1.20".parse().unwrap()));
    assert_eq!(found.lan, Some("192.168.1.20".parse().unwrap()));

    // Friendly names are useful when the operating system does expose them, even without a
    // usable default route. The full snapshot stays intact so Tailscale can still be found.
    let named = ifs(&[("VirtualBox Host-Only Ethernet Adapter", "192.168.56.1"), ("Wi-Fi", "192.168.1.20"), ("Tailscale", "100.101.1.2")]);
    let found = Addresses::from_interfaces_preferring(&named, None, None);
    assert_eq!((found.lan, found.tailscale), (Some("192.168.1.20".parse().unwrap()), Some("100.101.1.2".parse().unwrap())));

    // A full-tunnel VPN's default route is not mistaken for the LAN; the physical adapter stays
    // the advertised route.
    for tunnel in ["utun4", "OpenVPN Data Channel Offload", "wg0", "WireGuard Tunnel"] {
        let vpn = ifs(&[(tunnel, "10.8.0.2"), ("Wi-Fi", "192.168.1.20")]);
        let found = Addresses::from_interfaces_preferring(&vpn, None, Some("10.8.0.2".parse().unwrap()));
        assert_eq!(found.lan, Some("192.168.1.20".parse().unwrap()), "{tunnel}");
    }
}

#[test]
fn tailscales_answer_is_the_first_address_it_prints() {
    use crate::remote::first_ipv4;
    assert_eq!(first_ipv4("100.101.1.2\r\n"), Some("100.101.1.2".parse().unwrap()));
    assert_eq!(first_ipv4("\n  100.64.0.7 \n100.64.0.8\n"), Some("100.64.0.7".parse().unwrap()));
    // The CLI says why it can't, on stdout or stderr, when it's stopped or logged out.
    assert_eq!(first_ipv4("Tailscale is stopped.\n"), None);
    assert_eq!(first_ipv4("fd7a:115c:a1e0::1\n"), None);
    assert_eq!(first_ipv4(""), None);
}

fn send(id: &str, text: &str) -> impl FnOnce(tr::Reply<Option<tr::Open>>) -> tr::HostRequest {
    let req = tr::SendRequest { thread_id: id.to_string(), text: text.to_string(), mode: None, images: vec![] };
    move |reply| tr::HostRequest::Send { req, reply }
}

/// The last notice in `id`: how Trek answered one of its own commands.
fn last_notice(trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, id: &str) -> String {
    trek.items(cx, id).into_iter().rev().find_map(|i| if let trek_core::store::Item::Notice { text } = i { Some(text) } else { None }).unwrap_or_default()
}

#[test]
fn a_phone_reads_sends_and_answers() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;

        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let row = snapshot.threads.iter().find(|t| t.id == id).expect("listed");
        assert_eq!(row.section, tr::Section::Inbox);
        assert!(snapshot.agents.iter().any(|a| a.key == super::harness::mock().key()), "the mock agent can start threads");

        // Full access stays locked, asked for from the phone like anywhere else.
        let before = trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding);
        ask(&trek, cx, send(&id, "/permissions full")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), before);
        assert_ne!(before, HandHolding::FullAccess);
        assert!(last_notice(&trek, cx, &id).contains("Full access is off"), "{}", last_notice(&trek, cx, &id));

        // A follow-up that makes the agent ask first; the phone answers it, once.
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "ask permission first".into(), mode: None, images: vec![] }, reply }).unwrap();
        trek.wait_needs_you(cx, &id).await;
        let request_id = trek.read(cx, |ws, _| ws.live[&id].permissions[0].request_id.clone());
        let allow = |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Allow } },
            reply,
        };
        ask(&trek, cx, allow).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let again = ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Deny } },
            reply,
        });
        assert_eq!(again.unwrap_err().code, tr::ErrorCode::Conflict, "answered already");
    });
}

#[test]
fn a_thread_started_on_the_phone_leaves_the_mac_where_it_was() {
    run(async |cx| {
        let trek = open(cx);
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let route = trek.read(cx, |ws, _| ws.route.clone());
        let id = ask(&trek, cx, |reply| tr::HostRequest::NewThread {
            req: tr::NewThreadRequest { project_id, agent: super::harness::mock().key(), model: None, text: "from the phone".into(), worktree: false, effort: Some("low".into()), access: None, plan: false, images: vec![] },
            reply,
        })
        .unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "the Mac's screen didn't move");
        assert!(trek.read(cx, |ws, _| !ws.tabs.contains(&id)), "nor did a tab open for it");
        assert!(trek.items(cx, &id).iter().any(|i| matches!(i, trek_core::store::Item::User { text, .. } if text == "from the phone")));
        let _ = Route::Basecamp;
    });
}

#[test]
fn a_phone_changes_a_thread_s_settings_but_not_past_what_the_mac_allows() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let set = |req: tr::PrefsRequest| move |reply| tr::HostRequest::SetPrefs { req, reply };
        ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), effort: Some("low".into()), access: Some(tr::Access::Supervised), plan: Some(true), ..Default::default() })).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        assert_eq!((t.effort, t.hand_holding), (trek_core::Effort::Low, HandHolding::Supervised));
        let row = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!((row.effort.as_deref(), row.access, row.plan), (Some("low"), Some(tr::Access::Supervised), true), "the row says so");

        // Full access stays locked until the Mac unlocks it.
        let refused = ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), access: Some(tr::Access::FullAccess), ..Default::default() }));
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Supervised);
        assert!(!ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().full_access);
        trek.update(cx, |ws, _| ws.settings.permissions.full_access_unlocked = true);
        ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), access: Some(tr::Access::FullAccess), ..Default::default() })).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::FullAccess);
        let bad = ask(&trek, cx, set(tr::PrefsRequest { thread_id: id.clone(), effort: Some("ludicrous".into()), ..Default::default() }));
        assert_eq!(bad.unwrap_err().code, tr::ErrorCode::BadRequest);
    });
}

#[test]
fn a_phone_pins_settles_renames_and_sends_photos() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let act = |action: tr::ThreadAction| {
            let id = id.clone();
            move |reply| tr::HostRequest::ThreadAction { req: tr::ThreadActionRequest { thread_id: id, action }, reply }
        };
        ask(&trek, cx, act(tr::ThreadAction::Pin)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().pinned_at.is_some()));
        ask(&trek, cx, act(tr::ThreadAction::Pin)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().pinned_at.is_some()), "pinning twice keeps it pinned");
        ask(&trek, cx, act(tr::ThreadAction::Rename { title: "From the phone".into() })).unwrap();
        ask(&trek, cx, act(tr::ThreadAction::Settle)).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        assert_eq!(t.title, "From the phone");
        assert!(t.settled_at.is_some());

        // A photo: a 1×1 PNG, base64.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let image = tr::ImageUpload { mime: "image/png".into(), data: png.into() };
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "look at this".into(), mode: None, images: vec![image] }, reply }).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = trek.items(cx, &id).into_iter().rev().find_map(|i| match i {
            trek_core::store::Item::User { text, images, .. } if text == "look at this" => Some(images),
            _ => None,
        });
        let images = sent.expect("the message");
        assert_eq!(images.len(), 1);
        assert!(std::path::Path::new(&images[0]).exists() && images[0].ends_with(".png"), "{images:?}");
        // Two photos sent in the same second are kept apart.
        for text in ["photo one", "photo two"] {
            let image = tr::ImageUpload { mime: "image/png".into(), data: png.into() };
            ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: text.into(), mode: None, images: vec![image] }, reply }).unwrap();
        }
        trek.wait_done(cx, &id, RunState::Idle).await;
        let kept: Vec<String> = trek
            .items(cx, &id)
            .into_iter()
            .filter_map(|i| match i {
                trek_core::store::Item::User { text, images, .. } if text.starts_with("photo ") => images.into_iter().next(),
                _ => None,
            })
            .collect();
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_ne!(kept[0], kept[1], "one photo replaced the other");
        // Not a photo: refused.
        let junk = tr::ImageUpload { mime: "image/png".into(), data: "aGVsbG8=".into() };
        let refused = ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "x".into(), mode: None, images: vec![junk] }, reply });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
    });
}

#[test]
fn a_photo_that_cannot_be_kept_leaves_the_macs_follow_up_setting_alone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let mac = trek.read(cx, |ws, _| ws.settings.general.follow_up);
        let (other, mode) = match mac {
            trek_core::settings::FollowUp::Queue => (trek_core::settings::FollowUp::Steer, tr::SendMode::Steer),
            trek_core::settings::FollowUp::Steer => (trek_core::settings::FollowUp::Queue, tr::SendMode::Queue),
        };
        assert_ne!(mac, other);
        let junk = tr::ImageUpload { mime: "image/png".into(), data: "aGVsbG8=".into() };
        let refused = ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "x".into(), mode: Some(mode), images: vec![junk] }, reply });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| ws.settings.general.follow_up), mac);
    });
}

#[test]
fn trek_s_own_commands_run_from_the_phone_but_full_access_waits_for_the_mac() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let route = trek.read(cx, |ws, _| ws.route.clone());

        // Answered in the thread, as typed on the Mac.
        ask(&trek, cx, send(&id, "/permissions auto")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Auto);
        ask(&trek, cx, send(&id, "/model")).unwrap();
        assert!(last_notice(&trek, cx, &id).contains("Mock"), "{}", last_notice(&trek, cx, &id));
        // Every way of asking for Full access needs the Mac's unlock.
        for ask_for in ["/permissions full", "/access full", "/mode yolo"] {
            ask(&trek, cx, send(&id, ask_for)).unwrap();
            assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::Auto, "{ask_for}");
        }
        trek.update(cx, |ws, _| ws.settings.permissions.full_access_unlocked = true);
        ask(&trek, cx, send(&id, "/permissions full")).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().hand_holding), HandHolding::FullAccess);

        // `/new` opens the phone's sheet, in the thread's project; the Mac stays where it was.
        let project_id = trek.read(cx, |ws, _| ws.thread(&id).unwrap().project_id.clone());
        let open = ask(&trek, cx, send(&id, "/new")).unwrap();
        assert_eq!(open, Some(tr::Open::NewThread { project_id }));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), route, "the Mac's screen didn't move");

        // `/restate` sends the message asking for a restatement, as the composer does.
        ask(&trek, cx, send(&id, "/restate fix the login page")).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let asked = trek.items(cx, &id).into_iter().rev().find_map(|i| if let trek_core::store::Item::User { text, .. } = i { Some(text) } else { None }).unwrap();
        assert_eq!(asked, trek_core::restate::with_restate("fix the login page"));
        // A consultant the Mac doesn't have is refused with why.
        let refused = ask(&trek, cx, send(&id, "/consult nosuchmodel: hi")).unwrap_err();
        assert_eq!(refused.code, tr::ErrorCode::BadRequest, "{}", refused.message);

        // A thread can't start with a command about one under way.
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let refused = ask(&trek, cx, |reply| tr::HostRequest::NewThread {
            req: tr::NewThreadRequest { project_id, agent: super::harness::mock().key(), model: None, text: "/usage".into(), worktree: false, effort: None, access: None, plan: false, images: vec![] },
            reply,
        });
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);

        // The `/` picker's list: Trek's own first, marked so.
        let commands = ask(&trek, cx, |reply| tr::HostRequest::Commands { thread_id: id.clone(), reply }).unwrap();
        let full = commands.iter().find(|c| c.name == "permissions full").expect("listed");
        assert!(full.trek && full.kind == tr::CommandKind::Command);
        assert_eq!(commands[0].name, "new");
    });
}

#[test]
fn rows_carry_the_thread_s_details_and_switching_agent_picks_its_default_model() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let row = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext| ask(trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|r| r.id == id).unwrap();
        let r = row(&trek, cx);
        assert_eq!(r.effort_label.as_deref(), Some("High"));
        assert!(r.model_label.is_some(), "the default model is named too");
        assert_eq!(r.agent.key, super::harness::mock().key());

        // What its session reported shows: the context ring and the cost line.
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.context = Some((50_000, 200_000));
        });
        let r = row(&trek, cx);
        assert_eq!(r.context, Some(tr::ContextUse { used: 50_000, window: 200_000, percent: 25 }));

        // Another agent: its default model, and an effort it takes.
        let relay = trek_core::AgentId::Direct(trek_core::catalog::MOCK_RELAY_PROVIDER.into());
        ask(&trek, cx, |reply| tr::HostRequest::SetPrefs { req: tr::PrefsRequest { thread_id: id.clone(), agent: Some(relay.key()), ..Default::default() }, reply }).unwrap();
        let t = trek.read(cx, |ws, _| ws.thread(&id).cloned().unwrap());
        let models = trek.read(cx, |ws, _| ws.models_for(&relay));
        assert_eq!(t.agent, relay);
        assert_eq!(t.model, crate::composer::default_model(&models).map(|m| m.id.clone()));
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let offered = snapshot.agents.iter().find(|a| a.key == relay.key()).expect("offered");
        assert_eq!(offered.default_model, t.model, "the phone's model list starts where the Mac's does");
        let r = snapshot.threads.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.agent.key, relay.key());
        // Logos are the Mac's own keys.
        assert_eq!(crate::remote::agent_ref(&trek_core::AgentId::ClaudeCode).logo.as_deref(), Some("claude-code"));
        assert_eq!(crate::remote::agent_ref(&trek_core::AgentId::Acp("kimi".into())).logo.as_deref(), Some("kimi"));
    });
}

#[test]
fn a_turn_s_changed_files_follow_its_end() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let end = trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::TurnEnd { .. }));
        let file = tr::ChangedFile { path: "src/a.rs".into(), status: tr::FileStatus::Modified, from: None, added: 3, removed: 1, binary: false };
        crate::remote::tests::TURN_FILES.with(|f| f.borrow_mut().insert((id.clone(), end), vec![file.clone(), tr::ChangedFile { path: "b.png".into(), status: tr::FileStatus::Added, binary: true, added: 0, removed: 0, from: None }]));
        let transcript = ask_later(&trek, cx, |reply| tr::HostRequest::Transcript { thread_id: id.clone(), after_seq: None, limit: None, reply }).await.unwrap();
        let at = transcript.items.iter().position(|i| i.id == format!("i{end}")).expect("the turn end");
        let changes = &transcript.items[at + 1];
        assert_eq!(changes.id, format!("c{end}"));
        let tr::ItemBody::Changes { files, added, removed } = &changes.body else { panic!("{:?}", changes.body) };
        assert_eq!((files[0].clone(), files.len(), *added, *removed), (file, 2, 3, 1));
        assert!(changes.seq > transcript.items[at].seq);
    });
}

#[test]
fn notes_from_the_phone_are_the_mac_s_notes() {
    run(async |cx| {
        let trek = open(cx);
        let note = ask(&trek, cx, |reply| tr::HostRequest::CreateNote { body: "Groceries\n- milk".into(), reply }).unwrap();
        assert_eq!(note.title, "Groceries");
        let dir = trek_core::notes::notes_dir();
        assert!(dir.join(format!("{}.md", note.id)).exists(), "a markdown file in the notes folder");
        assert_eq!(trek.read(cx, |ws, _| ws.notes_epoch), 1, "the Notes screen reads them again");
        let list = ask(&trek, cx, |reply| tr::HostRequest::Notes { reply }).unwrap();
        assert_eq!(list.iter().map(|n| (n.title.as_str(), n.preview.as_str())).collect::<Vec<_>>(), [("Groceries", "milk")]);

        let saved = ask(&trek, cx, |reply| tr::HostRequest::SaveNote { req: tr::SaveNoteRequest { note_id: note.id.clone(), body: "Groceries\n- milk\n- bread".into(), modified: Some(note.modified) }, reply }).unwrap();
        assert!(saved.body.ends_with("bread"));
        // Changed on the Mac since the phone read it: refused, and nothing written.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(dir.join(format!("{}.md", note.id)), "Groceries\n- eggs").unwrap();
        let stale = ask(&trek, cx, |reply| tr::HostRequest::SaveNote { req: tr::SaveNoteRequest { note_id: note.id.clone(), body: "Groceries".into(), modified: Some(saved.modified) }, reply });
        assert_eq!(stale.unwrap_err().code, tr::ErrorCode::Conflict);
        assert_eq!(ask(&trek, cx, |reply| tr::HostRequest::Note { note_id: note.id.clone(), reply }).unwrap().body, "Groceries\n- eggs");
        // An id is never a path.
        let sneaky = ask(&trek, cx, |reply| tr::HostRequest::Note { note_id: "../settings".into(), reply });
        assert_eq!(sneaky.unwrap_err().code, tr::ErrorCode::NotFound);

        ask(&trek, cx, |reply| tr::HostRequest::DeleteNote { note_id: note.id.clone(), reply }).unwrap();
        assert!(dir.join("Deleted").join(format!("{}.md", note.id)).exists(), "kept in Deleted, as on the Mac");
        assert!(ask(&trek, cx, |reply| tr::HostRequest::Notes { reply }).unwrap().is_empty());
    });
}

#[test]
fn a_phone_changes_only_the_settings_it_may() {
    run(async |cx| {
        let trek = open(cx);
        let s = ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap();
        assert_eq!((s.default_agent.as_str(), s.default_access, s.full_access), (super::harness::mock().key().as_str(), tr::Access::Supervised, false));
        let set = |change: tr::SettingsChange| move |reply| tr::HostRequest::SetSettings { change, reply };
        let s = ask(&trek, cx, set(tr::SettingsChange {
            default_effort: Some("low".into()),
            default_access: Some(tr::Access::Auto),
            follow_up: Some(tr::SendMode::Queue),
            notifications: Some(tr::NotifyMode::Banner),
            push: Some(true),
            push_when: Some(tr::PushWhen::Always),
            auto_settle_days: Some(7),
            theme: Some(tr::Theme::Paper),
            ..Default::default()
        }))
        .unwrap();
        assert!(s.push.enabled && s.push.topic.starts_with("trek-"), "turning push on makes a topic");
        assert_eq!(s.push.subscribe_url, Some(format!("ntfy://ntfy.sh/{}", s.push.topic)));
        let mac = trek.read(cx, |ws, _| ws.settings.clone());
        assert_eq!((mac.general.default_effort, mac.general.hand_holding, mac.general.follow_up), (trek_core::Effort::Low, HandHolding::Auto, trek_core::settings::FollowUp::Queue));
        assert_eq!((mac.inbox.auto_settle_days, mac.appearance.theme), (7, trek_core::settings::ThemeChoice::Paper));
        assert_eq!(trek.read(cx, |ws, _| ws.draft_prefs.hand_holding), HandHolding::Auto, "new threads start with it");

        // Full access as the default needs the Mac's unlock; nothing else changes with a refusal.
        let refused = ask(&trek, cx, set(tr::SettingsChange { default_access: Some(tr::Access::FullAccess), auto_settle_days: Some(1), ..Default::default() }));
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(trek.read(cx, |ws, _| (ws.settings.general.hand_holding, ws.settings.inbox.auto_settle_days)), (HandHolding::Auto, 7));
        let bad = ask(&trek, cx, set(tr::SettingsChange { push_server: Some("javascript:alert(1)".into()), ..Default::default() }));
        assert_eq!(bad.unwrap_err().code, tr::ErrorCode::BadRequest);
        // Only https: what goes to ntfy says what the agents are doing.
        for plain in ["http://ntfy.example.com", "https://", "ftp://ntfy.example.com", "https://ntfy.example.com/a b"] {
            let refused = ask(&trek, cx, set(tr::SettingsChange { push_server: Some(plain.into()), ..Default::default() })).unwrap_err();
            assert_eq!(refused.code, tr::ErrorCode::BadRequest, "{plain}");
            assert!(refused.message.contains("https://"), "{}", refused.message);
        }
        assert_eq!(trek.read(cx, |ws, _| ws.settings.mobile.push_server.clone()), trek_core::settings::Settings::default().mobile.push_server);
        let own = ask(&trek, cx, set(tr::SettingsChange { push_server: Some("HTTPS://ntfy.example.com/".into()), ..Default::default() })).unwrap();
        assert_eq!(own.push.server, "HTTPS://ntfy.example.com");
        let unknown = ask(&trek, cx, set(tr::SettingsChange { default_agent: Some("acp:nobody".into()), ..Default::default() }));
        assert_eq!(unknown.unwrap_err().code, tr::ErrorCode::NotFound);
        let topic = trek.read(cx, |ws, _| ws.settings.mobile.push_topic.clone());
        let fresh = ask(&trek, cx, set(tr::SettingsChange { new_push_topic: true, ..Default::default() })).unwrap();
        assert_ne!(fresh.push.topic, topic);
        // A test notification needs notifications on (with them on it goes out to ntfy).
        let off = ask(&trek, cx, set(tr::SettingsChange { push: Some(false), push_test: true, ..Default::default() }));
        assert_eq!(off.unwrap_err().code, tr::ErrorCode::BadRequest);
        assert!(trek.read(cx, |ws, _| ws.settings.mobile.push), "a failed test changes nothing");
        // What a phone can't touch, it can't even say: the change has no field for it.
        assert!(!trek.read(cx, |ws, _| ws.settings.permissions.full_access_unlocked));
    });
}

#[test]
fn another_phone_connecting_keeps_the_rows_already_sent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.push_remote_changes(cx);
        });
        let before = trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.clone());
        assert!(!before.is_empty());
        trek.update(cx, |ws, cx| ws.remote_notice(tr::ServerNotice::Connected { device_id: "second-phone".into() }, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.clone()), before);
    });
}

#[test]
fn git_from_the_phone_reviews_commits_and_switches_as_the_panel_does() {
    run(async |cx| {
        let trek = open(cx);
        super::worktrees::make_repo(&trek, cx);
        let project_id = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == trek.project).unwrap().id.clone());
        let target = tr::GitTarget::project(project_id.clone());
        std::fs::write(trek.project.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(trek.project.join("new.txt"), "one\ntwo\n").unwrap();
        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        assert_eq!((status.is_repo, status.branch.as_deref(), status.can_switch), (true, Some("main"), true));
        let files: Vec<(&str, tr::FileStatus, u32)> = status.files.iter().map(|f| (f.path.as_str(), f.status, f.added)).collect();
        assert!(files.contains(&("README.md", tr::FileStatus::Modified, 1)) && files.contains(&("new.txt", tr::FileStatus::Untracked, 2)), "{files:?}");

        let diff = ask_later(&trek, cx, |reply| tr::HostRequest::GitDiff { req: tr::GitDiffRequest { target: target.clone(), path: "README.md".into() }, reply }).await.unwrap();
        assert!(diff.diff.contains("+world"), "{}", diff.diff);
        // Only changed files: a phone reads changes, not the disk.
        let refused = ask_later(&trek, cx, |reply| tr::HostRequest::GitDiff { req: tr::GitDiffRequest { target: target.clone(), path: ".env".into() }, reply }).await;
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::NotFound);

        ask_later(&trek, cx, |reply| tr::HostRequest::GitCommit { req: tr::GitCommitRequest { target: target.clone(), message: "From the phone".into() }, reply }).await.unwrap();
        assert_eq!(trek_core::worktree::git(&trek.project, &["log", "-1", "--format=%s"]).unwrap().trim(), "From the phone");
        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        assert!(status.files.is_empty());

        // Branches, and switching to one (only a local branch, never an option).
        trek_core::worktree::git(&trek.project, &["branch", "dev"]).unwrap();
        let branches = ask_later(&trek, cx, |reply| tr::HostRequest::GitBranches { target: target.clone(), reply }).await.unwrap();
        assert!(branches.branches.contains(&"dev".to_string()) && branches.current.as_deref() == Some("main"));
        let switch = |branch: &str| {
            let req = tr::GitSwitchRequest { target: target.clone(), branch: branch.into() };
            move |reply| tr::HostRequest::GitSwitch { req, reply }
        };
        assert_eq!(ask_later(&trek, cx, switch("--orphan=x")).await.unwrap_err().code, tr::ErrorCode::NotFound);
        assert!(trek.read(cx, |ws, _| ws.switch_blocked(&trek.project)).is_none(), "a refused switch lets go");

        // While the phone's switch runs, the checkout is switching: nothing else switches it (nor
        // starts a turn in it, `switch_blocked`); once done it's free again.
        let (reply, mut switched) = tokio::sync::oneshot::channel();
        let (second, mut refused) = tokio::sync::oneshot::channel();
        let blocked = trek.update(cx, |ws, cx| {
            ws.remote_request(switch("dev")(reply), cx);
            ws.remote_request(switch("main")(second), cx);
            ws.switch_blocked(&trek.project)
        });
        assert_eq!(blocked.as_deref(), Some("Switching branches…"));
        assert_eq!(refused.try_recv().unwrap().unwrap_err().code, tr::ErrorCode::Conflict);
        let deadline = std::time::Instant::now() + super::harness::REAL_TIME_LIMIT;
        let done = loop {
            cx.run_until_parked();
            if let Ok(done) = switched.try_recv() {
                break done;
            }
            assert!(std::time::Instant::now() < deadline, "no answer");
            cx.background_executor.timer(std::time::Duration::from_millis(5)).await;
        };
        done.unwrap();
        assert!(trek.read(cx, |ws, _| ws.switch_blocked(&trek.project)).is_none());
        assert_eq!(trek_core::worktree::git(&trek.project, &["branch", "--show-current"]).unwrap().trim(), "dev");
    });
}

#[test]
fn a_worktree_thread_merges_and_is_removed_from_the_phone_only_when_nothing_is_lost_unasked() {
    run(async |cx| {
        let trek = open(cx);
        super::worktrees::make_repo(&trek, cx);
        super::worktrees::use_worktree(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let wt = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.worktree.clone())).expect("a worktree");
        let target = tr::GitTarget::thread(id.clone());

        let status = ask_later(&trek, cx, |reply| tr::HostRequest::GitStatus { target: target.clone(), reply }).await.unwrap();
        let w = status.worktree.clone().expect("a worktree's review");
        assert_eq!((w.branch.as_str(), w.base.as_str(), w.uncommitted), (wt.branch.as_str(), "main", 1));
        assert!(w.merge_blocked.is_some() && !status.can_switch && status.switch_blocked.is_some());
        assert!(status.files.iter().any(|f| f.path == "NOTES.md"));
        // Its branch is the point of it: no switching here.
        let refused = ask_later(&trek, cx, |reply| tr::HostRequest::GitSwitch { req: tr::GitSwitchRequest { target: target.clone(), branch: "main".into() }, reply }).await;
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::Conflict);
        // Uncommitted work blocks the merge, saying why.
        let blocked = ask_later(&trek, cx, |reply| tr::HostRequest::WorktreeMerge { thread_id: id.clone(), reply }).await.unwrap_err();
        assert_eq!(blocked.code, tr::ErrorCode::Conflict);
        assert!(blocked.message.contains("isn't committed"), "{}", blocked.message);
        // Removing it would lose that work: refused, saying so, until the phone insists.
        let remove = |force: bool| {
            let req = tr::WorktreeRemoveRequest { thread_id: id.clone(), delete_branch: false, force };
            move |reply| tr::HostRequest::WorktreeRemove { req, reply }
        };
        let asked = ask_later(&trek, cx, remove(false)).await.unwrap_err();
        assert_eq!(asked.code, tr::ErrorCode::Conflict);
        assert!(asked.message.contains("1 uncommitted change in the worktree would be lost"), "{}", asked.message);
        assert!(wt.path.exists());

        // Committed and merged, it goes with nothing to ask.
        ask_later(&trek, cx, |reply| tr::HostRequest::GitCommit { req: tr::GitCommitRequest { target: target.clone(), message: "Add a note".into() }, reply }).await.unwrap();
        ask_later(&trek, cx, |reply| tr::HostRequest::WorktreeMerge { thread_id: id.clone(), reply }).await.unwrap();
        assert!(trek.project.join("NOTES.md").exists(), "merged into the project folder");
        ask_later(&trek, cx, remove(false)).await.unwrap();
        assert!(!wt.path.exists());
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().worktree.is_none()));
    });
}

#[test]
fn basecamp_and_usage_for_the_phone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // Finished and not looked at yet.
        trek.update(cx, |ws, _| ws.threads.iter_mut().find(|t| t.id == id).unwrap().last_seen_at = 0);
        let b = ask_later(&trek, cx, |reply| tr::HostRequest::Basecamp { range: tr::BasecampRange::Today, reply }).await.unwrap();
        assert_eq!((b.range, b.title.as_str(), b.empty), (tr::BasecampRange::Today, "Today's trek", false));
        let summary = b.summary.expect("a recap");
        assert_eq!((summary.prompts, summary.threads), (1, 1));
        assert!(matches!(&b.narrative[..2], [tr::Span::Text { text }, tr::Span::Strong { text: n }] if text == "You sent " && n == "1 prompt"), "{:?}", b.narrative);
        let profile = b.profile.expect("a profile");
        assert_eq!(profile.buckets.len(), 24, "an hour a stretch");
        assert!(profile.summit.is_some() && profile.total == "1 prompt");
        assert!(b.review.iter().any(|r| r.thread_id == id && !r.needs_you), "unread, so in Needs review");
        // As the Mac's Basecamp draws it now: totals, turns by the hour, and the tables.
        let totals = b.totals.as_ref().expect("totals");
        assert_eq!((totals.threads, totals.turns, totals.failed), (1, 1, 0));
        assert!(!totals.cost.is_empty() && !totals.agent_time.is_empty() && !totals.tokens_label.is_empty());
        let chart = b.chart.as_ref().expect("a chart");
        assert_eq!((chart.step.as_str(), chart.bars.len(), chart.turns, chart.total.as_str()), ("hour", 24, 1, "1 turn"));
        assert_eq!(chart.ticks.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["12 AM", "6 AM", "Noon", "6 PM"]);
        let busy = chart.bars.iter().find(|b| b.turns == 1).expect("the hour it ran in");
        assert_eq!(busy.line, format!("{} · 1 turn · {} tokens", busy.label, totals.tokens_label));
        assert_eq!(busy.tokens, totals.tokens);
        assert_eq!((b.projects.len(), b.projects[0].threads, b.projects[0].turns), (1, 1, 1));
        assert!(b.projects[0].project.is_some(), "{:?}", b.projects);
        assert_eq!((b.models.len(), b.models[0].turns), (1, 1));
        assert!(b.models[0].agent.is_some());
        // The mock's turn took no time and reported no tokens: the tiles say what's known.
        assert!(b.tiles.iter().all(|t| !t.label.is_empty() && !t.figure.is_empty()), "{:?}", b.tiles);
        let all = ask_later(&trek, cx, |reply| tr::HostRequest::Basecamp { range: tr::BasecampRange::All, reply }).await.unwrap();
        assert!(all.greeting.contains("on the trail since"), "{}", all.greeting);

        // No agent here reports a plan: nothing to show, nothing pending.
        let usage = ask_later(&trek, cx, |reply| tr::HostRequest::Usage { reply }).await.unwrap();
        assert!(!usage.loading);
    });
}

#[test]
fn a_note_s_pictures_go_to_the_phone_and_its_photos_come_back() {
    run(async |cx| {
        let trek = open(cx);
        let note = ask(&trek, cx, |reply| tr::HostRequest::CreateNote { body: "Whiteboard".into(), reply }).unwrap();
        // A photo from the phone is kept with the notes, and the phone is told how to link it.
        let mut png = vec![];
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 80, 30])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let add = |data: String, note_id: String| move |reply| tr::HostRequest::AddNoteImage { req: tr::AddNoteImageRequest { note_id, data, name: Some("The [board]".into()) }, reply };
        let added = ask(&trek, cx, add(tr::base64(&png), note.id.clone())).unwrap();
        assert!(added.path.starts_with("attachments/") && added.path.ends_with(".png"), "{}", added.path);
        assert_eq!(added.markdown, format!("![The board]({})", added.path));
        assert!(trek_core::notes::notes_dir().join(&added.path).is_file());
        assert_eq!(ask(&trek, cx, add("aGVsbG8=".into(), note.id.clone())).unwrap_err().code, tr::ErrorCode::BadRequest, "not a photo");
        assert_eq!(ask(&trek, cx, add(tr::base64(&png), "nope".into())).unwrap_err().code, tr::ErrorCode::NotFound);

        // The phone draws it from there, scaled as asked.
        let asset = |path: &str, note_id: &str| {
            let req = tr::NoteAssetRequest { note_id: note_id.into(), path: path.into(), max_px: Some(64) };
            move |reply| tr::HostRequest::NoteAsset { req, reply }
        };
        let got = ask_later(&trek, cx, asset(&added.path, &note.id)).await.unwrap();
        assert_eq!((got.mime.as_str(), got.width, got.height, got.path.as_str()), ("image/jpeg", 40, 20, added.path.as_str()));
        // Nothing but the notes' own pictures.
        std::fs::write(trek_core::notes::notes_dir().join("private.png"), &png).unwrap();
        for path in ["attachments/../private.png", "/etc/hosts", "../notes/private.png", "private.png"] {
            assert_eq!(ask_later(&trek, cx, asset(path, &note.id)).await.unwrap_err().code, tr::ErrorCode::BadRequest, "{path}");
        }
        assert_eq!(ask_later(&trek, cx, asset(&added.path, "../settings")).await.unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

#[test]
fn a_note_s_pdfs_and_videos_go_to_the_phone_in_pieces_byte_for_byte() {
    run(async |cx| {
        let trek = open(cx);
        let dir = trek_core::notes::notes_dir();
        let kept = trek_core::notes::attachments_dir(&dir);
        std::fs::create_dir_all(&kept).unwrap();
        // Bigger than a piece, and not a whole number of them.
        let pattern = |len: usize, seed: u8| (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 11) as u8).collect::<Vec<u8>>();
        let mut pdf = b"%PDF-1.4\n".to_vec();
        pdf.extend(pattern(2_500_000, 7));
        let mp4 = pattern(1_048_576 + 3, 91);
        std::fs::write(kept.join("report.pdf"), &pdf).unwrap();
        std::fs::write(kept.join("clip.mp4"), &mp4).unwrap();
        let piece = |path: &str, offset: u64, length: Option<u64>| {
            let req = tr::NoteFileRequest { path: path.into(), offset, length };
            move |reply| tr::HostRequest::NoteFile { req, reply }
        };
        for (path, bytes, mime, pieces) in [("attachments/report.pdf", &pdf, "application/pdf", 3), ("./attachments/clip.mp4", &mp4, "video/mp4", 2)] {
            let mut got = vec![];
            let mut asked = 0;
            loop {
                let c = ask_later(&trek, cx, piece(path, got.len() as u64, None)).await.unwrap();
                asked += 1;
                assert_eq!((c.path.as_str(), c.offset, c.total, c.mime.as_str()), (path, got.len() as u64, bytes.len() as u64, mime));
                let data = c.bytes().unwrap();
                assert!(!data.is_empty() && data.len() as u64 <= tr::MAX_NOTE_FILE_CHUNK, "{}", data.len());
                got.extend(data);
                if got.len() as u64 >= c.total {
                    break;
                }
            }
            assert_eq!(asked, pieces, "{path}");
            assert!(got == *bytes, "{path} came back different");
        }
        // A piece asked for too big is cut to the most one carries; past the end is refused.
        let c = ask_later(&trek, cx, piece("attachments/report.pdf", 10, Some(50 << 20))).await.unwrap();
        assert_eq!(c.bytes_b64.len(), tr::base64(&pdf[10..10 + (1 << 20)]).len());
        let c = ask_later(&trek, cx, piece("attachments/clip.mp4", mp4.len() as u64, None)).await.unwrap();
        assert!(c.bytes_b64.is_empty());
        let past = ask_later(&trek, cx, piece("attachments/clip.mp4", mp4.len() as u64 + 1, None)).await.unwrap_err();
        assert_eq!(past.code, tr::ErrorCode::BadRequest);
        // Nothing but the notes' own files.
        std::fs::write(dir.join("private.pdf"), &pdf).unwrap();
        for path in ["attachments/../private.pdf", "/etc/hosts", "../notes/private.pdf", "private.pdf", "attachments/sub/clip.mp4"] {
            assert_eq!(ask_later(&trek, cx, piece(path, 0, None)).await.unwrap_err().code, tr::ErrorCode::BadRequest, "{path}");
        }
        assert_eq!(ask_later(&trek, cx, piece("attachments/gone.pdf", 0, None)).await.unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

#[test]
fn links_between_threads_open_on_the_phone_with_how_they_re_doing() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:start-thread explain the startup");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let new = trek.read(cx, |ws, _| ws.started_threads(&id)[0].id.clone());
        let done = new.clone();
        trek.wait(cx, "the new thread to finish", move |ws| ws.thread_state(&done) == crate::workspace::ThreadState::Finished).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let links = |t: tr::Transcript| t.items.into_iter().filter(|i| i.body.kind() == "thread_link").map(|i| i.body).collect::<Vec<_>>();
        let starter = links(ask_later(&trek, cx, transcript_req(&id, None, None)).await.unwrap());
        let [tr::ItemBody::ThreadLink { link, thread_id, title, agent, state, open }] = &starter[..] else { panic!("{starter:?}") };
        assert_eq!((*link, thread_id, title.as_str(), *state, *open), (tr::ThreadLinkKind::Started, &new, "Speed up the slow tests", Some(tr::LinkedThreadState::Finished), true));
        assert_eq!(agent.as_ref().map(|a| a.key.clone()), Some(super::harness::mock().key()));
        // The started thread's row says where it came from, with no state of its own.
        let title_of_id = trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone());
        let kid = links(ask_later(&trek, cx, transcript_req(&new, None, None)).await.unwrap());
        assert!(matches!(&kid[..], [tr::ItemBody::ThreadLink { link: tr::ThreadLinkKind::StartedBy, thread_id, title, state: None, open: true, .. }] if *thread_id == id && *title == title_of_id), "{kid:?}");
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let row = snapshot.threads.iter().find(|t| t.id == new).expect("listed");
        assert_eq!((row.started_by.as_deref(), row.continued_from.as_deref()), (Some(id.as_str()), None));
    });
}

fn transcript_req(id: &str, after_seq: Option<u64>, limit: Option<u32>) -> impl FnOnce(tr::Reply<tr::Transcript>) -> tr::HostRequest {
    let thread_id = id.to_string();
    move |reply| tr::HostRequest::Transcript { thread_id, after_seq, limit, reply }
}

fn page_req(id: &str, before: &str, limit: u32) -> impl FnOnce(tr::Reply<tr::TranscriptPage>) -> tr::HostRequest {
    let (thread_id, before) = (id.to_string(), before.to_string());
    move |reply| tr::HostRequest::TranscriptBefore { thread_id, before, limit, reply }
}

fn turn_action(id: &str, item_id: String, action: tr::TurnAction, model: Option<&str>) -> impl FnOnce(tr::Reply<tr::TurnActionDone>) -> tr::HostRequest {
    let req = tr::TurnActionRequest { thread_id: id.to_string(), item_id, action, model: model.map(str::to_string), restore_files: true };
    move |reply| tr::HostRequest::TurnAction { req, reply }
}

/// A thread whose transcript is `turns` made-up turns (five items each), and a request open.
async fn long_thread(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, turns: usize) -> String {
    let id = trek.send(cx, "first thread");
    trek.wait_done(cx, &id, RunState::Idle).await;
    trek.update(cx, |ws, _| {
        let live = ws.live.get_mut(&id).unwrap();
        live.items = trek_core::transcript::Transcript::unsaved(super::harness::transcript(turns));
        live.permissions.push(crate::workspace::PendingPermission { request_id: "req-1".into(), title: "Run command".into(), detail: "rm -rf build".into(), prompt: None, after_turn: false });
        live.revision += 1;
        ws.start_test_remote();
    });
    id
}

#[test]
fn answers_are_matched_by_thread_and_request_id() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "first thread");
        trek.wait_done(cx, &a, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        let b = trek.send(cx, "second thread");
        trek.wait_done(cx, &b, RunState::Idle).await;
        trek.update(cx, |ws, _| {
            for id in [&a, &b] {
                let live = ws.live.get_mut(id).unwrap();
                live.permissions.push(crate::workspace::PendingPermission { request_id: "same-id".into(), title: "Run command".into(), detail: "make".into(), prompt: None, after_turn: false });
                live.revision += 1;
            }
            ws.start_test_remote();
        });
        _ = ask(&trek, cx, transcript_req(&a, None, None)).unwrap();
        _ = ask(&trek, cx, transcript_req(&b, None, None)).unwrap();
        ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: a.clone(), request_id: "same-id".into(), response: tr::AnswerResponse::Approval { decision: tr::Decision::Allow } },
            reply,
        })
        .unwrap();
        trek.read(cx, |ws, _| {
            let answered = &ws.remote.as_ref().unwrap().answered;
            assert_eq!(answered.get(&(a.clone(), "same-id".into())), Some(&tr::AnswerResponse::Approval { decision: tr::Decision::Allow }));
            assert_eq!(answered.get(&(b.clone(), "same-id".into())), None);
        });
    });
}

#[test]
fn oversized_transcript_text_is_clipped_at_char_boundaries() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 1).await;
        let text = format!("é{}TAIL", "\"".repeat(1_100_000));
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            *live.items.get_mut(0).unwrap() = trek_core::store::Item::User { text, images: vec![], at: None, resume: None, aside: false };
            live.revision += 1;
        });
        let transcript = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        let item = transcript.items.iter().find(|i| i.id == "i0").unwrap();
        let tr::ItemBody::User { text, .. } = &item.body else { panic!() };
        assert!(text.contains("bytes omitted") && text.starts_with('é') && text.ends_with("TAIL"));
        assert!(serde_json::to_vec(item).unwrap().len() < 2 << 20);
    });
}

#[test]
fn clipped_secret_questions_are_restored_before_they_are_answered() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let question = format!("Secret? {}", "q".repeat(2_200_000));
        let secret_label = format!("token-{}", "x".repeat(700_000));
        let public_label = format!("account-{}", "y".repeat(700_000));
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.permissions.push(crate::workspace::PendingPermission {
                request_id: "secret".into(),
                title: String::new(),
                detail: String::new(),
                prompt: Some(trek_agents::Prompt::Questions(vec![
                    trek_agents::Question {
                        question: question.clone(),
                        header: "Credentials".into(),
                        options: vec![(secret_label.clone(), "Use the saved token".into())],
                        multi: false,
                        secret: true,
                    },
                    trek_agents::Question {
                        question: "Which account?".into(),
                        header: "Account".into(),
                        options: vec![(public_label.clone(), "Use this account".into())],
                        multi: false,
                        secret: false,
                    },
                ])),
                after_turn: false,
            });
            live.revision += 1;
            ws.start_test_remote();
        });
        let transcript = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        let item = transcript.items.iter().find(|i| i.id == "rsecret").unwrap();
        let tr::ItemBody::Question { questions, .. } = &item.body else { panic!() };
        let shown_question = questions[0].question.clone();
        let shown_secret_label = questions[0].options[0].label.clone();
        let shown_public_label = questions[1].options[0].label.clone();
        assert!(shown_question.contains("bytes omitted") && shown_question != question);
        assert!(shown_secret_label.contains("bytes omitted") && shown_secret_label != secret_label);
        assert!(shown_public_label.contains("bytes omitted") && shown_public_label != public_label);
        assert!(serde_json::to_vec(item).unwrap().len() < 2 << 20);
        ask(&trek, cx, |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest {
                thread_id: id.clone(),
                request_id: "secret".into(),
                response: tr::AnswerResponse::Questions {
                    answers: vec![
                        tr::QA { question: shown_question.clone(), answer: shown_secret_label.clone() },
                        tr::QA { question: "Which account?".into(), answer: shown_public_label.clone() },
                    ],
                },
            },
            reply,
        })
        .unwrap();
        trek.read(cx, |ws, _| {
            let live = &ws.live[&id];
            assert!(live.items.iter().any(|item| matches!(item, trek_core::store::Item::User { text, aside: true, .. } if text == &format!("Account: {public_label}"))), "the option sent to the agent uses its original label");
            assert!(!live.items.iter().any(|item| matches!(item, trek_core::store::Item::User { text, .. } if text.contains(&secret_label))), "the secret was not kept as a user message");
            assert!(live.items.iter().any(|item| matches!(item, trek_core::store::Item::Notice { text } if text == "Private answer sent")));
            let tr::AnswerResponse::Questions { answers } = &ws.remote.as_ref().unwrap().answered[&(id.clone(), "secret".into())] else { panic!() };
            assert_eq!((answers[0].question.as_str(), answers[0].answer.as_str()), (shown_question.as_str(), shown_secret_label.as_str()));
            assert_eq!(answers[1].answer, shown_public_label);
        });
    });
}

#[test]
fn a_long_thread_opens_at_its_end_and_pages_back_to_its_start() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 100).await;
        let end = trek.read(cx, |ws, _| ws.live[&id].items.len()) - 1;
        crate::remote::tests::TURN_FILES.with(|f| f.borrow_mut().insert((id.clone(), end), vec![tr::ChangedFile { path: "src/parser.rs".into(), status: tr::FileStatus::Modified, from: None, added: 2, removed: 1, binary: false }]));

        // The last 50 items, the changed files after the last turn end, and the open request.
        let t = ask(&trek, cx, transcript_req(&id, None, Some(50))).unwrap();
        assert!(t.more, "earlier items were left out");
        let ids: Vec<&str> = t.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.iter().filter(|i| i.starts_with('i')).count(), 50);
        assert_eq!(ids[0], format!("i{}", end - 49));
        assert_eq!(&ids[ids.len() - 3..], [format!("i{end}").as_str(), &format!("c{end}"), "rreq-1"]);
        assert!(t.items.iter().all(|i| i.seq <= t.seq && i.seq >= t.base));

        // Nothing changed since: an incremental subscribe brings nothing.
        let again = ask(&trek, cx, transcript_req(&id, Some(t.seq), Some(50))).unwrap();
        assert!(again.items.is_empty() && !again.more, "{:?}", again.items.len());
        assert_eq!(again.seq, t.seq);

        // Paging back to the start, 200 at a time (and never more than the Mac's cap).
        let mut first = ids[0].to_string();
        let mut all: Vec<String> = vec![];
        loop {
            let page = ask(&trek, cx, page_req(&id, &first, 200)).unwrap();
            assert!(page.items.iter().filter(|i| i.id.starts_with('i')).count() <= 200);
            let mut ids: Vec<String> = page.items.iter().map(|i| i.id.clone()).collect();
            first = ids.first().cloned().unwrap();
            ids.append(&mut all);
            all = ids;
            if !page.more {
                break;
            }
        }
        // Every earlier item, once, in order, each turn's edits (counted from its edit tools)
        // right after its end.
        let expected: Vec<String> = (0..end - 49).flat_map(|ix| if ix % 5 == 4 { vec![format!("i{ix}"), format!("c{ix}")] } else { vec![format!("i{ix}")] }).collect();
        assert_eq!(all, expected);
        let big = ask(&trek, cx, page_req(&id, &format!("c{end}"), 10_000)).unwrap();
        assert_eq!(big.items.iter().filter(|i| i.id.starts_with('i')).count(), tr::MAX_PAGE as usize);
        // Before a turn's changed files: up to and with its end.
        let page = ask(&trek, cx, page_req(&id, &format!("c{end}"), 1)).unwrap();
        assert_eq!(page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), [format!("i{end}")]);
        assert_eq!(ask(&trek, cx, page_req(&id, "i99999", 10)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, page_req(&id, "rreq-1", 10)).unwrap_err().code, tr::ErrorCode::NotFound);

        // Without a limit (an older phone): all of it.
        let whole = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        assert!(!whole.more);
        assert_eq!(whole.items.iter().filter(|i| i.id.starts_with('i')).count(), end + 1);
    });
}

/// The mock agent's long history, end to end: over 2000 items, opened at its end and paged in.
#[test]
fn a_thread_of_thousands_of_items_from_the_mock_agent() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:history 450");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        assert!(n > 2250, "{n} items");
        let t = ask(&trek, cx, transcript_req(&id, None, Some(200))).unwrap();
        assert!(t.more);
        assert_eq!(t.items.iter().filter(|i| i.id.starts_with('i')).count(), 200);
        let mut before = t.items[0].id.clone();
        let mut seen = 200;
        loop {
            let page = ask(&trek, cx, page_req(&id, &before, 500)).unwrap();
            seen += page.items.iter().filter(|i| i.id.starts_with('i')).count();
            before = page.items[0].id.clone();
            if !page.more {
                break;
            }
        }
        assert_eq!((seen, before.as_str()), (n, "i0"));
        let edits = t.items.iter().filter(|i| matches!(&i.body, tr::ItemBody::Tool { added: Some(_), .. })).count();
        assert!(edits > 30, "edits carry their lines: {edits}");
    });
}

#[test]
fn the_mac_follows_only_threads_a_phone_has_open() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 20).await;
        let t = ask(&trek, cx, transcript_req(&id, None, Some(10))).unwrap();
        let following = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().map(|r| (r.watched.contains_key(&id), r.dormant.contains_key(&id))).unwrap());
        assert_eq!(following(&trek, cx), (true, false));

        // An answer arrives on the Mac: the tick takes it in.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.push(trek_core::store::Item::Notice { text: "Seen on the Mac".into() });
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        let seq = trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().watched[&id].seq);
        assert_eq!(seq, t.seq + 1, "one item changed, one seq");

        // The last phone let go: not followed any more.
        trek.update(cx, |ws, cx| ws.remote_request(tr::HostRequest::Unwatch { thread_id: id.clone() }, cx));
        assert_eq!(following(&trek, cx), (false, true));
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.push(trek_core::store::Item::Notice { text: "While nobody looked".into() });
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        assert_eq!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().dormant[&id].seq), seq, "nothing done for it");

        // Opened again: numbering carries on, and a phone that saw up to `seq` gets just what's new.
        let back = ask(&trek, cx, transcript_req(&id, Some(seq), Some(10))).unwrap();
        assert_eq!(following(&trek, cx), (true, false));
        assert!(back.seq > seq);
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        assert_eq!(back.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(), [format!("i{}", n - 1)]);
        // A phone from before (or another run of Trek) gets it whole.
        let stale = ask(&trek, cx, transcript_req(&id, Some(3), Some(10))).unwrap();
        assert!(stale.items.len() > 10 && stale.more);

        // A rewind: shorter, so phones read it again, and earlier seqs can't be brought up to date.
        trek.update(cx, |ws, cx| {
            ws.live.get_mut(&id).unwrap().items.truncate(5);
            ws.live.get_mut(&id).unwrap().revision += 1;
            ws.push_remote_changes(cx);
        });
        let after = ask(&trek, cx, transcript_req(&id, Some(back.seq), Some(10))).unwrap();
        assert!(after.base > back.seq && after.seq >= after.base);
        assert_eq!(after.items.iter().filter(|i| i.id.starts_with('i')).count(), 5, "whole again");
    });
}

#[test]
fn a_phone_undoes_retries_forks_and_rewinds_turns_without_moving_the_mac() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "apple");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.send_to(&id, "banana".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let (route, tabs) = trek.read(cx, |ws, _| (ws.route.clone(), ws.tabs.clone()));
        let said = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, id: &str| -> Vec<String> {
            trek.items(cx, id).into_iter().filter_map(|i| if let trek_core::store::Item::User { text, .. } = i { Some(text) } else { None }).collect()
        };
        let last_end = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| format!("i{}", trek.items(cx, &id).iter().rposition(|i| matches!(i, trek_core::store::Item::TurnEnd { .. })).unwrap());
        let user = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext, text: &str| format!("i{}", trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::User { text: t, .. } if t == text)));

        // Unknown items, and items that aren't what the action takes.
        assert_eq!(ask(&trek, cx, turn_action(&id, "i9999".into(), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, turn_action(&id, "x".into(), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Undo, None)).unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Rewind, None)).unwrap_err().code, tr::ErrorCode::BadRequest);
        assert_eq!(ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Retry, Some("no-such-model"))).unwrap_err().code, tr::ErrorCode::NotFound);

        // Fork after the first turn, and from before the second message.
        let first_end = format!("i{}", trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::TurnEnd { .. })));
        let fork = ask(&trek, cx, turn_action(&id, first_end, tr::TurnAction::Fork, None)).unwrap();
        let fork_id = fork.thread_id.expect("the new thread");
        assert_eq!((said(&trek, cx, &fork_id), fork.text), (vec!["apple".to_string()], None));
        let before = ask(&trek, cx, turn_action(&id, user(&trek, cx, "banana"), tr::TurnAction::Fork, None)).unwrap();
        assert_eq!(before.text.as_deref(), Some("banana"), "the message, for the phone's composer");
        assert_eq!(said(&trek, cx, &before.thread_id.unwrap()), ["apple"]);

        // Retry the last turn: the same message, answered again.
        ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Retry, None)).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(said(&trek, cx, &id), ["apple", "banana"]);

        // Undo it: the message comes back for the composer.
        let undone = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap();
        assert_eq!((undone.thread_id, undone.text.as_deref()), (None, Some("banana")));
        assert_eq!(said(&trek, cx, &id), ["apple"]);

        // While a turn runs, nothing is taken back.
        trek.update(cx, |ws, cx| ws.send_to(&id, "mock:long 20s".into(), vec![], cx));
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&id)).await;
        let busy = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap_err();
        assert_eq!((busy.code, busy.message.as_str()), (tr::ErrorCode::BadRequest, "Stop the running turn to undo"));
        let busy = ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Rewind, None)).unwrap_err();
        assert_eq!(busy.message, "Stop the running turn to rewind");
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait(cx, "the turn to stop", |ws| !ws.turn_running(&id)).await;

        // A turn no message of the user's started can't be undone.
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.items.push(trek_core::store::Item::Assistant { text: "A sub-agent reported back.".into() });
            live.items.push(trek_core::store::Item::TurnEnd { at: 1, took_secs: 1 });
        });
        let refused = ask(&trek, cx, turn_action(&id, last_end(&trek, cx), tr::TurnAction::Undo, None)).unwrap_err();
        assert_eq!(refused.message, "This turn didn't start from a message of yours");

        // Rewind to before the first message (how "Edit" works on the phone).
        let rewound = ask(&trek, cx, turn_action(&id, user(&trek, cx, "apple"), tr::TurnAction::Rewind, None)).unwrap();
        assert_eq!(rewound.text.as_deref(), Some("apple"));
        assert!(said(&trek, cx, &id).is_empty());

        assert_eq!(trek.read(cx, |ws, _| (ws.route.clone(), ws.tabs.clone())), (route, tabs), "the Mac's screen didn't move");
    });
}

/// How long serving and following a 3000-item thread takes (run with `--ignored --nocapture`).
#[test]
#[ignore]
fn timing_of_a_long_thread() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 600).await;
        let n = trek.read(cx, |ws, _| ws.live[&id].items.len());
        type Step<'a> = &'a dyn Fn(&mut crate::workspace::Workspace, &mut gpui_kit::Context<crate::workspace::Workspace>);
        let time = |label: &str, runs: u32, cx: &mut gpui_kit::TestAppContext, f: Step| {
            let t = std::time::Instant::now();
            for _ in 0..runs {
                trek.update(cx, |ws, cx| f(ws, cx));
            }
            eprintln!("{n} items: {label}: {:?}", t.elapsed() / runs);
        };
        time("subscribe, whole transcript", 1, cx, &|ws, cx| _ = ws.remote_transcript(&id, None, None, cx));
        trek.update(cx, |ws, _| {
            let r = ws.remote.as_mut().unwrap();
            r.watched.clear();
            r.dormant.clear();
        });
        time("subscribe, last 200", 1, cx, &|ws, cx| _ = ws.remote_transcript(&id, None, Some(200), cx));
        time("subscribe again, nothing new (after_seq)", 1, cx, &|ws, cx| {
            let seq = ws.remote.as_ref().unwrap().watched[&id].seq;
            _ = ws.remote_transcript(&id, Some(seq), Some(200), cx)
        });
        time("page of 200", 1, cx, &|ws, cx| _ = ws.remote_transcript_before(&id, &format!("i{}", n - 200), 200, cx));
        time("tick, idle", 50, cx, &|ws, cx| ws.push_remote_changes(cx));
        trek.update(cx, |ws, _| {
            let l = ws.live.get_mut(&id).unwrap();
            let ix = l.items.push(trek_core::store::Item::Assistant { text: "streaming".into() });
            l.streaming = Some(ix);
            l.revision += 1;
        });
        time("tick, streaming", 50, cx, &|ws, cx| {
            let l = ws.live.get_mut(&id).unwrap();
            if let Some(trek_core::store::Item::Assistant { text }) = l.items.last_mut() {
                text.push_str(" more words");
            }
            l.revision += 1;
            ws.push_remote_changes(cx)
        });
    });
}

#[test]
fn the_phone_allows_for_a_whole_session_only_where_the_mac_lets_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "ask permission first");
        trek.wait_needs_you(cx, &id).await;
        let request_id = trek.read(cx, |ws, _| ws.live[&id].permissions[0].request_id.clone());
        let for_session = |reply| tr::HostRequest::Answer {
            req: tr::AnswerRequest { thread_id: id.clone(), request_id: request_id.clone(), response: tr::AnswerResponse::Approval { decision: tr::Decision::AllowForSession } },
            reply,
        };
        // Off (as it starts): refused, whatever the phone's own check was. The card stays, and
        // nothing is remembered as answered.
        let settings = ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap();
        assert!(!settings.session_approvals);
        let refused = ask(&trek, cx, for_session).unwrap_err();
        assert!(refused.message.contains(&format!("off on {}", crate::words::words().this_computer)), "{refused:?}");
        assert_eq!(trek.run_state(cx, &id), RunState::NeedsYou);
        assert!(trek.read(cx, |ws, _| ws.live[&id].permissions.len() == 1 && ws.remote.as_ref().is_none_or(|r| r.answered.is_empty())));
        // Turned on at the Mac, the same answer goes through.
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.session_approvals = true;
            ws.save_settings(cx);
        });
        assert!(ask(&trek, cx, |reply| tr::HostRequest::Settings { reply }).unwrap().session_approvals);
        ask(&trek, cx, for_session).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

#[test]
fn a_long_answer_still_streaming_goes_less_often_and_whole_when_it_ends() {
    run(async |cx| {
        let trek = open(cx);
        let id = long_thread(&trek, cx, 3).await;
        ask(&trek, cx, transcript_req(&id, None, Some(10))).unwrap();
        let seq = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().watched[&id].seq);
        // An answer streams in: `more` is added to it and the tick run.
        let grow = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, more: &str| {
            trek.update(cx, |ws, cx| {
                let l = ws.live.get_mut(&id).unwrap();
                match l.streaming {
                    Some(ix) => {
                        if let Some(trek_core::store::Item::Assistant { text }) = l.items.get_mut(ix) {
                            text.push_str(more)
                        }
                    }
                    None => l.streaming = Some(l.items.push(trek_core::store::Item::Assistant { text: more.into() })),
                }
                l.revision += 1;
                ws.push_remote_changes(cx);
            })
        };
        // Short, it goes with every tick it grew in.
        let start = seq(&trek, cx);
        grow(&trek, cx, "The parser");
        grow(&trek, cx, " reads flags");
        assert_eq!(seq(&trek, cx), start + 2);
        // Long (a megabyte), it's held back between sends: the ticks right after one send nothing.
        grow(&trek, cx, &"x".repeat(1 << 20));
        let long = seq(&trek, cx);
        grow(&trek, cx, " and more");
        grow(&trek, cx, " and more");
        assert_eq!(seq(&trek, cx), long, "held back");
        // It ends: the phone gets it as it ended, at once, though nothing more was added.
        trek.update(cx, |ws, cx| {
            let l = ws.live.get_mut(&id).unwrap();
            l.streaming = None;
            l.revision += 1;
            ws.push_remote_changes(cx);
        });
        assert_eq!(seq(&trek, cx), long + 1);
        let back = ask(&trek, cx, transcript_req(&id, Some(long), Some(10))).unwrap();
        assert!(back.items.iter().any(|i| matches!(&i.body, tr::ItemBody::Assistant { text, streaming: false } if text.ends_with(" and more and more"))), "whole, as it ended");
    });
}

#[test]
fn thread_rows_are_built_only_when_something_of_the_threads_changed() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let row = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.get(&id).map(|r| r.title.clone()));
        trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        assert_eq!(row(&trek, cx).as_deref(), Some(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()).as_str()));
        // A change made the usual way is in the rows at the next tick.
        trek.update(cx, |ws, cx| {
            ws.rename(&id, "Renamed on the Mac".into(), cx);
            ws.push_remote_changes(cx);
        });
        assert_eq!(row(&trek, cx).as_deref(), Some("Renamed on the Mac"));
    });
}

#[test]
fn a_phone_server_that_cant_start_stays_on_and_says_why() {
    use crate::settings_view::shown;
    use crate::workspace::SettingsPage;
    super::harness::run(async |cx| {
        let trek = super::harness::open(cx);
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.enabled = true;
            // What start_remote leaves when binding failed: the switch as it was, and why.
            ws.remote_error = Some("Tailscale isn't connected; choose Wi-Fi or connect Tailscale".into());
            ws.navigate(Route::Settings(SettingsPage::Mobile), cx);
        });
        shown::take();
        trek.render(cx);
        let said = shown::take();
        let why = format!("Couldn't start: Tailscale isn't connected; choose Wi-Fi or connect Tailscale. Trying again every {} seconds.", crate::remote::RETRY.as_secs());
        assert!(said.iter().any(|s| *s == why), "{said:?}");
        assert!(!said.iter().any(|s| s == "Starting…"), "{said:?}");
        assert!(trek.read(cx, |ws, _| ws.settings.mobile.enabled));
        // Turned off, the failure is forgotten.
        trek.update(cx, |ws, cx| {
            ws.settings.mobile.enabled = false;
            ws.sync_remote(cx);
        });
        assert!(trek.read(cx, |ws, _| ws.remote_error.is_none()));
    });
}

fn thread_action(id: &str, action: tr::ThreadAction) -> impl FnOnce(tr::Reply<()>) -> tr::HostRequest {
    let req = tr::ThreadActionRequest { thread_id: id.to_string(), action };
    move |reply| tr::HostRequest::ThreadAction { req, reply }
}

#[test]
fn a_phone_opens_a_sub_agent_answers_it_and_stops_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let kid = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Write it", "prompt": "mock:permission", "mode": "implement" }), cx)).unwrap();
        let k = kid.clone();
        trek.wait(cx, "the sub-agent to ask", move |ws| ws.task_state(&k) == crate::workspace::TaskState::NeedsYou).await;
        trek.update(cx, |ws, cx| {
            ws.start_test_remote();
            ws.push_remote_changes(cx);
        });
        // Not listed, as on the Mac: an unlisted row, named by its parent's, which carries its request.
        let (parent, row) = trek.read(cx, |ws, _| {
            let r = ws.remote.as_ref().unwrap();
            (r.sent[&id].clone(), r.unlisted.get(&kid).cloned())
        });
        let row = row.expect("the sub-agent's row");
        assert_eq!((row.parent_id.as_deref(), row.run_state), (Some(id.as_str()), tr::RunState::NeedsYou));
        assert_eq!(parent.sub_agents.iter().map(|s| s.thread_id.clone()).collect::<Vec<_>>(), [Some(kid.clone())]);
        assert_eq!((parent.needs_thread.as_deref(), parent.needs.as_ref().map(|n| n.text.as_str())), (Some(kid.as_str()), Some("Sub-agent needs you")));
        assert_eq!(parent.section, tr::Section::Inbox, "in the inbox for it, as on the Mac");
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        assert!(snapshot.unlisted.iter().any(|t| t.id == kid) && !snapshot.threads.iter().any(|t| t.id == kid));

        // Opened: its transcript, its request, answered from the phone.
        let k = kid.clone();
        let transcript = ask_later(&trek, cx, move |reply| tr::HostRequest::Transcript { thread_id: k, after_seq: None, limit: Some(50), reply }).await.unwrap();
        let request_id = transcript.items.iter().find_map(|i| match &i.body {
            tr::ItemBody::Approval { request_id, state: tr::ApprovalState::Pending, .. } => Some(request_id.clone()),
            _ => None,
        });
        let request_id = request_id.expect("its approval");
        let allow = tr::AnswerRequest { thread_id: kid.clone(), request_id, response: tr::AnswerResponse::Approval { decision: tr::Decision::Allow } };
        ask(&trek, cx, |reply| tr::HostRequest::Answer { req: allow, reply }).unwrap();
        let k = kid.clone();
        trek.wait(cx, "the sub-agent to finish", move |ws| !ws.task_state(&k).live()).await;
        trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        let parent = trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent[&id].clone());
        assert_eq!(parent.needs_thread, None);
        assert!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().unlisted.contains_key(&kid)), "a done sub-agent still opens");

        // Another, stopped from the phone as its own Stop does: its parent hears it was stopped.
        let other = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Write more", "prompt": "mock:permission", "mode": "implement" }), cx)).unwrap();
        let o = other.clone();
        trek.wait(cx, "the other to ask", move |ws| ws.task_state(&o) == crate::workspace::TaskState::NeedsYou).await;
        let o = other.clone();
        ask(&trek, cx, |reply| tr::HostRequest::Interrupt { thread_id: o, reply }).unwrap();
        let o = other.clone();
        trek.wait(cx, "it to stop", move |ws| ws.task_state(&o) == crate::workspace::TaskState::Cancelled).await;
    });
}

#[test]
fn a_phone_snoozes_wakes_marks_unread_retitles_and_deletes_as_the_menu_does() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "tidy the readme");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| ws.start_test_remote());
        let now = trek_core::store::now_ms();
        let snoozed = |trek: &super::harness::Trek, cx: &gpui_kit::TestAppContext| trek.read(cx, |ws, _| ws.thread(&id).unwrap().snoozed_until);

        ask(&trek, cx, thread_action(&id, tr::ThreadAction::Snooze { until: None, preset: Some(tr::SnoozePreset::Hour) })).unwrap();
        let until = snoozed(&trek, cx).unwrap();
        assert!((until - now - 3_600_000).abs() < 60_000, "{until}");
        let row = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|t| t.id == id).unwrap();
        assert_eq!((row.section, row.snoozed_until), (tr::Section::Snoozed, Some(until)));
        ask(&trek, cx, thread_action(&id, tr::ThreadAction::Snooze { until: None, preset: Some(tr::SnoozePreset::Tomorrow) })).unwrap();
        assert_eq!(snoozed(&trek, cx), crate::time::morning(&chrono::Local::now(), 1).map(|t| t.timestamp_millis()), "the Mac's morning");
        let at = now + 5 * 3_600_000;
        ask(&trek, cx, thread_action(&id, tr::ThreadAction::Snooze { until: Some(at), preset: None })).unwrap();
        assert_eq!(snoozed(&trek, cx), Some(at));
        let past = ask(&trek, cx, thread_action(&id, tr::ThreadAction::Snooze { until: Some(now - 1000), preset: None }));
        assert_eq!(past.unwrap_err().code, tr::ErrorCode::BadRequest);
        ask(&trek, cx, thread_action(&id, tr::ThreadAction::Wake)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().section(trek_core::store::now_ms() + 1)) == Some(trek_core::store::Section::Inbox));

        ask(&trek, cx, thread_action(&id, tr::ThreadAction::MarkUnread)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().is_unseen()));

        // The mock agent names its threads itself.
        trek.update(cx, |ws, cx| ws.rename(&id, "Something else".into(), cx));
        ask(&trek, cx, thread_action(&id, tr::ThreadAction::RegenerateTitle)).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()), trek_agents::mock::title("tidy the readme"));
        let empty = trek.quiet_thread(cx);
        assert_eq!(ask(&trek, cx, thread_action(&empty, tr::ThreadAction::RegenerateTitle)).unwrap_err().code, tr::ErrorCode::BadRequest);

        // Delete asks first, in the Mac's words (sub-agents named), and deletes once confirmed.
        let kid = trek.update(cx, |ws, cx| ws.delegate(&id, &serde_json::json!({ "title": "Check it", "prompt": "hello" }), cx)).unwrap();
        let k = kid.clone();
        trek.wait(cx, "the sub-agent to finish", move |ws| !ws.task_state(&k).live()).await;
        let asked = ask(&trek, cx, thread_action(&id, tr::ThreadAction::Delete { confirmed: false })).unwrap_err();
        assert_eq!(asked.code, tr::ErrorCode::Conflict);
        assert!(asked.message.contains("can't be undone") && asked.message.contains("Its sub-agent is deleted with it"), "{}", asked.message);
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some()), "nothing happened yet");
        ask(&trek, cx, thread_action(&id, tr::ThreadAction::Delete { confirmed: true })).unwrap();
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_none() && ws.thread(&kid).is_none()));
        assert_eq!(ask(&trek, cx, thread_action(&id, tr::ThreadAction::Delete { confirmed: true })).unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

#[test]
fn a_phone_searches_titles_and_messages_beyond_its_list() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "look at the flag parser");
        trek.wait_done(cx, &id, RunState::Idle).await;
        ask(&trek, cx, send(&id, "where does the zanzibar flag get parsed")).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let titled = trek.send(cx, "Zanzibar release checklist");
        assert_ne!(titled, id);
        trek.wait_done(cx, &titled, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.rename(&titled, "Zanzibar release checklist".into(), cx));
        let kid = trek.update(cx, |ws, cx| ws.delegate(&titled, &serde_json::json!({ "title": "Zanzibar notes", "prompt": "hello" }), cx)).unwrap();
        let search = |query: &str| {
            let req = tr::SearchRequest { query: query.into(), limit: None };
            move |reply| tr::HostRequest::Search { req, reply }
        };
        let found = ask_later(&trek, cx, search("zanzib")).await.unwrap();
        let ids: Vec<&str> = found.results.iter().map(|r| r.thread.id.as_str()).collect();
        assert_eq!(found.query, "zanzib");
        // Titles first (a sub-agent's too, with its parent), then the message match with its words.
        assert!(ids.contains(&titled.as_str()) && ids.contains(&kid.as_str()), "{ids:?}");
        let message = found.results.iter().position(|r| r.thread.id == id).expect("the message match");
        assert!(found.results[..message].iter().all(|r| r.snippet.is_none()), "titles first");
        let hit = &found.results[message];
        let snippet = hit.snippet.clone().expect("an excerpt");
        let words: Vec<String> = hit.ranges.iter().map(|[a, b]| snippet.chars().skip(*a as usize).take((b - a) as usize).collect()).collect();
        assert!(words.iter().any(|w| w.eq_ignore_ascii_case("zanzibar")), "{snippet} {words:?}");
        assert_eq!(found.results.iter().find(|r| r.thread.id == kid).and_then(|r| r.thread.parent_id.clone()), Some(titled.clone()));
        // Nothing to look for: nothing found.
        assert!(ask_later(&trek, cx, search("  ")).await.unwrap().results.is_empty());
        let one = tr::SearchRequest { query: "zanzibar".into(), limit: Some(1) };
        assert_eq!(ask_later(&trek, cx, move |reply| tr::HostRequest::Search { req: one, reply }).await.unwrap().results.len(), 1);
    });
}

#[test]
fn search_ranges_count_characters() {
    let text = "café — zanzibar";
    let start = text.find("zanzibar").unwrap();
    assert_eq!(crate::remote::search::char_ranges(text, std::slice::from_ref(&(start..start + 8))), [[7, 15]]);
    assert!(crate::remote::search::char_ranges(text, std::slice::from_ref(&(3..99))).is_empty(), "out of the text");
}

// ---- parity with the Mac (`tr::feature`) ----

fn row(trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, id: &str) -> tr::ThreadSummary {
    ask(trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap().threads.into_iter().find(|t| t.id == id).expect("listed")
}

fn queue_action(id: &str, index: u32, reset: bool, text: &str, action: tr::QueueAction) -> impl FnOnce(tr::Reply<Option<String>>) -> tr::HostRequest {
    let req = tr::QueueActionRequest { thread_id: id.to_string(), index, reset, text: text.to_string(), action };
    move |reply| tr::HostRequest::QueueAction { req, reply }
}

#[test]
fn usage_for_the_phone_is_the_usage_card_s_with_its_resets_to_spend() {
    run(async |cx| {
        let trek = open(cx);
        let spent = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        trek.update(cx, |ws, _| {
            let now = ws.now();
            let credit = |id: &str| trek_agents::ResetCredit { id: id.into(), title: "Full reset (Weekly + 5 hr)".into(), description: None, expires_at: Some(now + 86_400_000) };
            let limit = trek_agents::UsageLimit { label: "5-hour limit".into(), percent: 91., resets_at: Some(now + 3_600_000), window: "5h".into() };
            ws.agent_status.insert(
                trek_core::AgentId::Codex.key(),
                trek_agents::AgentStatus { plan: Some("ChatGPT Pro".into()), limits: vec![limit], resets: vec![credit("rc-1"), credit("rc-2")], ..Default::default() },
            );
            let spent = spent.clone();
            ws.reset_consumer = Some(std::sync::Arc::new(move |id| {
                let spent = spent.clone();
                Box::pin(async move {
                    spent.lock().unwrap().push(id.clone());
                    if id == "rc-2" { Err(anyhow::anyhow!("it lapsed")) } else { Ok(()) }
                })
            }));
        });
        let usage = trek.read(cx, |ws, _| ws.usage_now());
        assert_eq!(usage.picked, Some(false), "Trek picked the providers, the user didn't");
        let codex = usage.providers.iter().find(|p| p.agent.key == "codex").expect("on the card");
        assert_eq!(codex.resets.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["rc-1", "rc-2"]);
        assert!(!codex.no_limits && codex.today.is_none(), "limits to show: no line about today");
        let shown = trek.read(cx, |ws, _| ws.usage_shown().len());
        assert_eq!(usage.providers.len(), shown, "the card's providers, no more");

        ask_later(&trek, cx, |reply| tr::HostRequest::UseReset { credit_id: "rc-1".into(), reply }).await.unwrap();
        let left: Vec<String> = trek.read(cx, |ws, _| ws.usage_now().providers.into_iter().flat_map(|p| p.resets).map(|r| r.id).collect());
        assert_eq!(left, ["rc-2"], "spent, and gone from the list");
        let failed = ask_later(&trek, cx, |reply| tr::HostRequest::UseReset { credit_id: "rc-2".into(), reply }).await;
        assert!(failed.unwrap_err().message.contains("it lapsed"), "the phone hears why");
        let gone = ask(&trek, cx, |reply| tr::HostRequest::UseReset { credit_id: "rc-1".into(), reply });
        assert_eq!(gone.unwrap_err().code, tr::ErrorCode::NotFound);
        assert_eq!(*spent.lock().unwrap(), ["rc-1", "rc-2"]);
    });
}

#[test]
fn queued_follow_ups_ride_in_the_row_and_the_phone_changes_or_drops_them() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, _| {
            let live = ws.live.get_mut(&id).unwrap();
            live.queued.push(("then write the docs".into(), vec![]));
            live.queued.push(("and open a PR".into(), vec!["/tmp/shot.png".into()]));
            let mut pause = trek_core::limit::Pause::new("limit".into(), Some(ws.now() + 60_000), trek_core::limit::LimitScope::Session, 0, true);
            pause.queued.push(trek_core::limit::Queued { text: "carry on".into(), images: vec![] });
            ws.threads.iter_mut().find(|t| t.id == id).unwrap().paused = Some(pause);
            ws.threads_gen += 1;
        });
        let queued = row(&trek, cx, &id).queued;
        assert_eq!(queued.iter().map(|q| (q.text.as_str(), q.images, q.reset)).collect::<Vec<_>>(), [("then write the docs", 0, false), ("and open a PR", 1, false), ("carry on", 0, true)]);

        ask(&trek, cx, queue_action(&id, 1, false, "and open a PR", tr::QueueAction::Edit { text: "and open a draft PR".into() })).unwrap();
        let taken = ask(&trek, cx, queue_action(&id, 0, false, "then write the docs", tr::QueueAction::Remove)).unwrap();
        assert_eq!(taken.as_deref(), Some("then write the docs"), "a dropped message comes back for the composer");
        let live: Vec<(String, usize)> = trek.read(cx, |ws, _| ws.live[&id].queued.iter().map(|(t, i)| (t.clone(), i.len())).collect());
        assert_eq!(live, [("and open a draft PR".to_string(), 1)], "changed in place, its photo kept");
        // The queue moved on: what the phone showed at that place isn't there any more.
        let stale = ask(&trek, cx, queue_action(&id, 0, false, "then write the docs", tr::QueueAction::Remove));
        assert_eq!(stale.unwrap_err().code, tr::ErrorCode::Conflict);

        // The one waiting for the reset: words can't go, there's nothing else to send.
        let empty = ask(&trek, cx, queue_action(&id, 0, true, "carry on", tr::QueueAction::Edit { text: " ".into() }));
        assert_eq!(empty.unwrap_err().code, tr::ErrorCode::BadRequest);
        ask(&trek, cx, queue_action(&id, 0, true, "carry on", tr::QueueAction::Edit { text: "carry on, then summarise".into() })).unwrap();
        assert_eq!(trek.read(cx, |ws, _| ws.pause(&id).unwrap().queued[0].text.clone()), "carry on, then summarise");
        ask(&trek, cx, queue_action(&id, 0, true, "carry on, then summarise", tr::QueueAction::Remove)).unwrap();
        assert!(trek.read(cx, |ws, _| ws.pause(&id).unwrap().queued.is_empty()));
        assert_eq!(row(&trek, cx, &id).queued.len(), 1);
    });
}

#[test]
fn a_message_s_images_go_to_the_phone_scaled_down() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A wide screenshot, with a transparent corner.
        let mut image = image::RgbaImage::from_pixel(3000, 1000, image::Rgba([40, 90, 200, 255]));
        image.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let mut png = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let upload = tr::ImageUpload { mime: "image/png".into(), data: tr::base64(&png.into_inner()) };
        ask(&trek, cx, |reply| tr::HostRequest::Send { req: tr::SendRequest { thread_id: id.clone(), text: "look".into(), mode: None, images: vec![upload] }, reply }).unwrap();
        trek.wait_done(cx, &id, RunState::Idle).await;
        let ix = trek.items(cx, &id).iter().rposition(|i| matches!(i, trek_core::store::Item::User { text, .. } if text == "look")).unwrap();
        let get = |index: u32, max_px: Option<u32>| {
            let req = tr::AttachmentRequest { thread_id: id.clone(), item_id: format!("i{ix}"), index, max_px };
            move |reply| tr::HostRequest::Attachment { req, reply }
        };
        let thumb = ask_later(&trek, cx, get(0, Some(512))).await.unwrap();
        assert_eq!((thumb.width, thumb.height, thumb.mime.as_str()), (512, 171, "image/jpeg"));
        let bytes = base64_decode(&thumb.data);
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "a JPEG");
        let full = ask_later(&trek, cx, get(0, Some(100_000))).await.unwrap();
        assert_eq!(full.width, tr::MAX_ATTACHMENT_PX, "capped");
        let missing = ask_later(&trek, cx, get(3, None)).await;
        assert_eq!(missing.unwrap_err().code, tr::ErrorCode::NotFound);
        let not_a_message = ask_later(&trek, cx, |reply| tr::HostRequest::Attachment { req: tr::AttachmentRequest { thread_id: id.clone(), item_id: format!("i{}", ix + 1), index: 0, max_px: None }, reply }).await;
        assert_eq!(not_a_message.unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

fn base64_decode(s: &str) -> Vec<u8> {
    tr::ImageUpload { mime: String::new(), data: s.into() }.decode().map(|(b, _)| b).unwrap_or_default()
}

#[test]
fn background_tasks_show_their_output_and_stop_from_the_phone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:server");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let tasks = row(&trek, cx, &id).background_tasks;
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        let task = tasks[0].clone();
        assert!(task.readable && task.stoppable && !task.stopping && task.since.is_some(), "{task:?}");
        assert_eq!(task.kind, tr::BackgroundKind::Shell);

        let (thread_id, task_id) = (id.clone(), task.id.clone());
        let out = ask_later(&trek, cx, move |reply| tr::HostRequest::BackgroundOutput { thread_id, task_id, reply }).await.unwrap();
        assert!(out.output.as_deref().is_some_and(|o| !o.trim().is_empty()), "the end of what it printed: {out:?}");

        let (thread_id, task_id) = (id.clone(), task.id.clone());
        ask(&trek, cx, move |reply| tr::HostRequest::StopBackground { thread_id, task_id, reply }).unwrap();
        trek.wait(cx, "the server to stop", |ws| ws.live.values().all(|l| l.background_work().next().is_none())).await;
        assert!(row(&trek, cx, &id).background_tasks.is_empty());
        let (thread_id, task_id) = (id.clone(), task.id.clone());
        let gone = ask(&trek, cx, move |reply| tr::HostRequest::StopBackground { thread_id, task_id, reply });
        assert_eq!(gone.unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

#[test]
fn project_icons_go_to_the_phone_by_hash() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(row(&trek, cx, &id).project.icon, None, "a monogram");
        let (project_id, key) = trek.read(cx, |ws, _| {
            let p = ws.projects.iter().find(|p| p.path == trek.project).unwrap();
            (p.id.clone(), p.path.display().to_string())
        });
        let set_icon = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext, icon: &str| {
            let (key, icon) = (key.clone(), icon.to_string());
            trek.update(cx, move |ws, _| {
                ws.settings.projects.entry(key).or_default().icon = Some(icon);
                ws.threads_gen += 1;
            });
        };
        let fetch = |project_id: &str| {
            let req = tr::IconRequest { agent: None, project_id: Some(project_id.to_string()) };
            move |reply| tr::HostRequest::Icon { req, reply }
        };

        set_icon(&trek, cx, "lucide:rocket");
        let row_icon = row(&trek, cx, &id).project.icon.expect("an icon");
        let snapshot = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        assert_eq!(snapshot.projects.iter().find(|p| p.id == project_id).unwrap().icon.as_ref(), Some(&row_icon));
        let icon = ask_later(&trek, cx, fetch(&project_id)).await.unwrap();
        assert_eq!((icon.hash.as_str(), icon.template), (row_icon.as_str(), true), "a shape, drawn in the project's ink");
        let png = image::load_from_memory(&base64_decode(&icon.png)).unwrap();
        assert_eq!((png.width(), png.height()), (96, 96));
        assert!(png.to_rgba8().pixels().any(|p| p.0[3] > 0), "something drawn");

        // A picture of the user's own.
        let file = std::env::temp_dir().join(format!("trek-phone-icon-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 30, 30, 255])).save(&file).unwrap();
        set_icon(&trek, cx, &format!("file:{}", file.display()));
        let picture = row(&trek, cx, &id).project.icon.expect("an icon");
        assert_ne!(picture, row_icon, "another icon, another hash");
        let icon = ask_later(&trek, cx, fetch(&project_id)).await.unwrap();
        assert!(!icon.template);
        let png = image::load_from_memory(&base64_decode(&icon.png)).unwrap().to_rgba8();
        assert_eq!((png.width(), png.height(), png.get_pixel(48, 48).0), (96, 96, [200, 30, 30, 255]), "cropped to fill");
        let _ = std::fs::remove_file(&file);

        // Agents the phone has logos for get none.
        let none = ask(&trek, cx, |reply| tr::HostRequest::Icon { req: tr::IconRequest { agent: Some("codex".into()), project_id: None }, reply });
        assert_eq!(none.unwrap_err().code, tr::ErrorCode::NotFound);
    });
}

#[test]
fn asides_are_marked_so_the_phone_offers_nothing_to_rewind_to() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        ask(&trek, cx, send(&id, "/permissions full")).unwrap();
        trek.update(cx, |ws, _| ws.start_test_remote());
        let t = ask(&trek, cx, transcript_req(&id, None, None)).unwrap();
        let users: Vec<(String, bool)> = t.items.iter().filter_map(|i| match &i.body {
            tr::ItemBody::User { aside, .. } => Some((i.id.clone(), *aside)),
            _ => None,
        }).collect();
        assert_eq!(users.iter().map(|(_, a)| *a).collect::<Vec<_>>(), [false, true], "the command Trek answered is an aside");
        let refused = ask(&trek, cx, turn_action(&id, users[1].0.clone(), tr::TurnAction::Rewind, None));
        assert_eq!(refused.unwrap_err().code, tr::ErrorCode::BadRequest);
        let wire = serde_json::to_value(&t.items.iter().find(|i| i.id == users[0].0).unwrap().body).unwrap();
        assert!(wire.get("aside").is_none(), "a prompt says nothing more than before");
    });
}

#[test]
fn projects_agents_and_full_access_reach_a_phone_connected_before_they_changed() {
    use crate::remote::tests::SNAPSHOTS_PUSHED;
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, _| ws.start_test_remote());
        let pushed = || SNAPSHOTS_PUSHED.with(|n| n.get());
        let tick = |trek: &super::harness::Trek, cx: &mut gpui_kit::TestAppContext| trek.update(cx, |ws, cx| ws.push_remote_changes(cx));
        // The phone connected and read its snapshot: a tick has nothing more of it to send.
        let first = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        let start = pushed();
        tick(&trek, cx);
        assert_eq!(pushed(), start, "nothing changed");

        // Claude Code is found after the phone connected (Trek just launched): a new snapshot, once.
        assert!(!first.agents.iter().any(|a| a.key == trek_core::AgentId::ClaudeCode.key()));
        trek.update(cx, |ws, _| {
            ws.agents.push(trek_core::detect::DetectedAgent {
                agent: trek_core::AgentId::ClaudeCode,
                name: "Claude Code".into(),
                path: None,
                version: None,
                availability: trek_core::detect::Availability::Ready,
                models: vec![],
                install_hint: None,
            })
        });
        tick(&trek, cx);
        assert_eq!(pushed(), start + 1, "agents found");
        tick(&trek, cx);
        assert_eq!(pushed(), start + 1, "once");

        // A project added on the Mac, then Full access unlocked there.
        let project = super::harness::new_project("added-later");
        trek.update(cx, |ws, cx| ws.add_project(project, cx));
        tick(&trek, cx);
        assert_eq!(pushed(), start + 2, "a project added");
        trek.update(cx, |ws, _| ws.settings.permissions.full_access_unlocked = true);
        tick(&trek, cx);
        assert_eq!(pushed(), start + 3, "full access unlocked");

        let now = ask(&trek, cx, |reply| tr::HostRequest::Snapshot { reply }).unwrap();
        assert!(now.full_access && now.agents.iter().any(|a| a.key == trek_core::AgentId::ClaudeCode.key()));
        assert_eq!(now.projects.len(), first.projects.len() + 1);
        // A second phone reading its snapshot hides nothing from the first.
        tick(&trek, cx);
        assert_eq!(pushed(), start + 3);
    });
}

#[test]
fn a_phone_that_connected_before_the_mac_listened_is_sent_changes() {
    use std::sync::Arc;
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        // A server as `start_remote` makes one (over TLS), and a phone pairing with it as soon as
        // it listens, before the Mac has subscribed to its notices.
        let rt = trek_core::runtime();
        let identity = tr::TlsIdentity::load_or_create(&super::harness::new_project("phone-tls"), "Test Mac").unwrap();
        let fingerprint = identity.fingerprint.clone();
        let mut config = tr::ServerConfig::new(tr::HostInfo { id: "test".into(), name: "Test Mac".into(), version: "0".into() });
        config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        config.tls = Some(identity);
        let (host, requests) = tr::ChannelHost::new();
        rt.spawn(async move {
            while let Ok(req) = requests.recv().await {
                if let tr::HostRequest::Snapshot { reply } = req {
                    let _ = reply.send(Ok(tr::Snapshot::default()));
                }
            }
        });
        let handle = rt.block_on(tr::RemoteServer::start(config, Arc::new(host))).unwrap();
        let code = handle.pairing_offer().code;
        let route = handle.local_addr().to_string();
        let phone = {
            let _rt = rt.enter();
            tr::client::Client::connect(vec![route], &fingerprint, tr::client::Auth::Pair { code, device_id: "early-phone".into(), device_name: "iPhone".into() }).unwrap()
        };
        let deadline = std::time::Instant::now() + super::harness::REAL_TIME_LIMIT;
        while handle.connected_devices().is_empty() {
            assert!(std::time::Instant::now() < deadline, "the phone never connected");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Its `Connected` notice went to no one; the server knows it's there.
        let remote = crate::remote::Remote::new(handle, Default::default(), String::new(), vec![], (0, trek_core::settings::Reach::Wifi), None, vec![]);
        assert!(remote.connected.contains("early-phone"), "{:?}", remote.connected);
        trek.update(cx, |ws, cx| {
            ws.remote = Some(remote);
            ws.push_remote_changes(cx);
        });
        assert!(trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().sent.contains_key(&id)), "rows go to it");
        phone.close();
        trek.read(cx, |ws, _| ws.remote.as_ref().unwrap().handle.shutdown());
    });
}

#[test]
fn the_reach_setting_decides_per_connection() {
    use crate::remote::link::{LinkPolicy, Via, via};
    use trek_core::settings::Reach;
    let ip = |s: &str| -> std::net::IpAddr { s.parse().unwrap() };
    let tailscale = Some("100.101.1.2".parse().unwrap());
    let (lan_mac, ts_mac) = (ip("192.168.1.2"), ip("100.101.1.2"));
    // Which network a connection came over, from both ends.
    assert_eq!(via(ip("192.168.1.30"), lan_mac, tailscale), Via::Wifi);
    assert_eq!(via(ip("::ffff:192.168.1.30"), ip("::ffff:192.168.1.2"), tailscale), Via::Wifi, "as a dual-stack socket sees IPv4");
    assert_eq!(via(ip("100.88.7.6"), ts_mac, tailscale), Via::Tailnet);
    assert_eq!(via(ip("fd7a:115c:a1e0::5"), ip("fd7a:115c:a1e0::9"), tailscale), Via::Tailnet);
    assert_eq!(via(ip("127.0.0.1"), ip("127.0.0.1"), None), Via::ThisMac);
    assert_eq!(via(ip("fe80::1c2b"), ip("fe80::1"), None), Via::Wifi);
    assert_eq!(via(ip("2a02:1:2:3::99"), ip("2a02:1:2:3::5"), None), Via::Wifi, "IPv6 on this Mac's own /64");
    assert_eq!(via(ip("2a02:9:9:9::99"), ip("2a02:1:2:3::5"), None), Via::Elsewhere, "IPv6 from another network");
    assert_eq!(via(ip("8.8.8.8"), lan_mac, tailscale), Via::Elsewhere);
    // A carrier's shared addresses (a hotspot, an ISP) aren't the tailnet: only Tailscale's own is.
    assert_eq!(via(ip("100.72.3.9"), ip("100.72.3.4"), tailscale), Via::Elsewhere);
    assert_eq!(via(ip("100.88.7.6"), ts_mac, None), Via::Elsewhere, "Tailscale gone: its old address is no one's");
    // Reaching the tailnet address from the Wi-Fi isn't coming over the tailnet.
    assert_eq!(via(ip("192.168.1.30"), ts_mac, tailscale), Via::Elsewhere);

    // "Wi-Fi": the local network and the tailnet (a phone paired at home finds the Mac away
    // from it), never the internet.
    let wifi = LinkPolicy { reach: Reach::Wifi, tailscale };
    assert!(wifi.admits(ip("192.168.1.30"), lan_mac) && wifi.admits(ip("100.88.7.6"), ts_mac) && wifi.admits(ip("127.0.0.1"), ip("127.0.0.1")));
    assert!(!wifi.admits(ip("8.8.8.8"), lan_mac) && !wifi.admits(ip("100.72.3.9"), ip("100.72.3.4")));
    // "Tailscale": the tailnet alone. A phone on the same Wi-Fi is refused.
    let tailnet = LinkPolicy { reach: Reach::Tailscale, tailscale };
    assert!(tailnet.admits(ip("100.88.7.6"), ts_mac) && tailnet.admits(ip("127.0.0.1"), ip("127.0.0.1")));
    assert!(!tailnet.admits(ip("192.168.1.30"), lan_mac) && !tailnet.admits(ip("fe80::1c2b"), ip("fe80::1")) && !tailnet.admits(ip("8.8.8.8"), lan_mac));
}

/// A server as `start_remote` makes one (over TLS, its reach policy in front), on the loopback,
/// told to give phones `addresses`' routes; its fingerprint.
fn tls_remote(addresses: crate::remote::Addresses, answer: impl Fn(tr::HostRequest) + Send + 'static) -> (crate::remote::Remote, String) {
    use std::sync::{Arc, Mutex};
    let rt = trek_core::runtime();
    let identity = tr::TlsIdentity::load_or_create(&super::harness::new_project("link-tls"), "Test Mac").unwrap();
    let fingerprint = identity.fingerprint.clone();
    let reach = trek_core::settings::Reach::Wifi;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let routes = crate::remote::remote_routes(&addresses, reach, port).unwrap();
    let policy = Arc::new(Mutex::new(crate::remote::link::LinkPolicy { reach, tailscale: addresses.tailscale }));
    let mut config = tr::ServerConfig::new(tr::HostInfo { id: "test".into(), name: "Test Mac".into(), version: "0".into() });
    config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    config.tls = Some(identity);
    config.advertise = Some(routes[0].clone());
    config.routes = routes.clone();
    config.admit = Some({
        let policy = policy.clone();
        tr::Admission::new(move |peer, local| policy.lock().unwrap().admits(peer.ip(), local.ip()))
    });
    let (host, requests) = tr::ChannelHost::new();
    rt.spawn(async move {
        while let Ok(req) = requests.recv().await {
            answer(req);
        }
    });
    let handle = rt.block_on(tr::RemoteServer::start(config, Arc::new(host))).unwrap();
    let mut remote = crate::remote::Remote::new(handle, addresses, routes[0].clone(), routes, (port, reach), None, vec![]);
    remote.policy = policy;
    (remote, fingerprint)
}

/// The next event from a phone, waiting in real time.
fn next_event(events: &mut tokio::sync::mpsc::UnboundedReceiver<tr::client::Event>) -> tr::client::Event {
    trek_core::runtime().block_on(async { tokio::time::timeout(std::time::Duration::from_secs(10), events.recv()).await }).expect("an event in time").expect("the phone's events")
}

#[test]
fn the_mac_moving_networks_keeps_its_phones_and_tells_them_the_new_address() {
    use crate::remote::{Addresses, link::Via};
    run(async |cx| {
        let trek = open(cx);
        let home = Addresses { lan: Some("192.168.1.2".parse().unwrap()), tailscale: None, tailnet: None };
        let (remote, fingerprint) = tls_remote(home, |req| {
            if let tr::HostRequest::Snapshot { reply } = req {
                let _ = reply.send(Ok(tr::Snapshot::default()));
            }
        });
        let handle = remote.handle.clone();
        let port = handle.local_addr().port();
        assert_eq!(handle.routes(), [format!("192.168.1.2:{port}")]);
        trek.update(cx, |ws, _| ws.remote = Some(remote));

        let rt = trek_core::runtime();
        let code = handle.pairing_offer().code;
        let phone = {
            let _rt = rt.enter();
            tr::client::Client::connect(vec![handle.local_addr().to_string()], &fingerprint, tr::client::Auth::Pair { code, device_id: "phone".into(), device_name: "iPhone".into() }).unwrap()
        };
        let mut events = phone.events().unwrap();
        let paired = match next_event(&mut events) {
            tr::client::Event::Connected { paired, .. } => paired,
            other => panic!("expected the phone to pair, got {other:?}"),
        };
        assert!(paired.routes.contains(&format!("192.168.1.2:{port}")), "{:?}", paired.routes);
        trek.update(cx, |ws, cx| ws.remote_notice(tr::ServerNotice::Paired { device_id: "phone".into(), name: "iPhone".into() }, cx));

        // Another Wi-Fi, and Tailscale came up.
        let away = Addresses { lan: Some("10.0.0.7".parse().unwrap()), tailscale: Some("100.101.1.2".parse().unwrap()), tailnet: None };
        trek.update(cx, |ws, cx| ws.remote_addresses_changed(away, cx));
        let told = vec![format!("10.0.0.7:{port}"), format!("100.101.1.2:{port}")];
        assert_eq!(handle.routes(), told, "the next welcome's");
        trek.read(cx, |ws, _| {
            let remote = ws.remote.as_ref().unwrap();
            assert_eq!((remote.routes.clone(), remote.advertise.clone()), (told.clone(), told[0].clone()));
            assert_eq!(remote.handle.local_addr(), handle.local_addr(), "the same server");
            assert_eq!(remote.policy.lock().unwrap().tailscale, Some("100.101.1.2".parse().unwrap()), "the tailnet is let in");
        });

        // The phone that was connected still is: it's answered, and never heard it drop.
        let pong = rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(10), phone.request(tr::ClientMessage::Ping)).await });
        assert!(matches!(pong, Ok(Ok(tr::ServerMessage::Pong))), "{pong:?}");
        assert_eq!(*phone.state().borrow(), tr::client::State::Connected);
        assert!(events.try_recv().into_iter().all(|e| !matches!(e, tr::client::Event::Disconnected { .. })));

        // One connecting now hears the new routes.
        let again = {
            let _rt = rt.enter();
            tr::client::Client::resume(&paired).unwrap()
        };
        let mut again_events = again.events().unwrap();
        match next_event(&mut again_events) {
            tr::client::Event::Connected { paired, .. } => assert!(told.iter().all(|r| paired.routes.contains(r)), "{:?}", paired.routes),
            other => panic!("expected the phone to connect again, got {other:?}"),
        }

        // Settings › Phone can say how the link stands.
        let link = trek.read(cx, |ws, _| ws.phone_link()).expect("the server runs");
        assert_eq!((link.listening, link.routes), (vec![handle.local_addr()], told));
        assert_eq!(link.advertised_name, None, "a test server isn't announced");
        let last = link.last_connection.expect("a phone connected");
        assert_eq!((last.device_id.as_str(), last.device_name.as_deref(), last.route, last.via), ("phone", Some("iPhone"), handle.local_addr().to_string(), Via::ThisMac));
        phone.close();
        again.close();
        handle.shutdown();
    });
}

#[test]
fn machine_info_says_what_this_machine_is() {
    run(async |cx| {
        let trek = open(cx);
        let info = ask_later(&trek, cx, |reply| tr::HostRequest::MachineInfo { reply }).await.unwrap();
        assert!(!info.name.trim().is_empty());
        assert_eq!((info.platform.as_str(), info.trek_version.as_str()), (std::env::consts::OS, env!("CARGO_PKG_VERSION")));
        assert_eq!(info.home, trek_core::paths::home().display().to_string());
    });
}

/// A bare repo with one commit (`README.md`, "hello"), to clone from.
fn bare_repo() -> std::path::PathBuf {
    let work = super::harness::new_project("clone-source");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&work, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgSign", "false")] {
        git(&work, &["config", k, v]);
    }
    std::fs::write(work.join("README.md"), "hello\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "init"]);
    let bare = work.with_extension("git");
    git(&work, &["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
    bare
}

fn clone_request(url: &std::path::Path, folder: &std::path::Path, name: &str) -> tr::CloneProjectRequest {
    tr::CloneProjectRequest { url: url.display().to_string(), folder: folder.display().to_string(), name: name.into() }
}

#[test]
fn a_clone_is_registered_as_a_project() {
    use std::sync::{Arc, Mutex};
    run(async |cx| {
        let trek = open(cx);
        let source = bare_repo();
        let folder = super::harness::new_project("clones").join("nested").join("cloned");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let progress = {
            let lines = lines.clone();
            tr::CloneProgress::new(folder.display().to_string(), move |msg| lines.lock().unwrap().push(msg))
        };
        let req = clone_request(&source, &folder, "From another Mac");
        let route = trek.read(cx, |ws, _| ws.route.clone());
        let id = ask_later(&trek, cx, |reply| tr::HostRequest::CloneProject { req, progress, reply }).await.unwrap();
        // git on Windows checks the file out with CRLF line endings by default.
        assert_eq!(std::fs::read_to_string(folder.join("README.md")).unwrap().replace("\r\n", "\n"), "hello\n");
        trek.read(cx, |ws, _| {
            let project = ws.project(&id).expect("registered");
            assert_eq!((project.path.clone(), project.name.as_str()), (super::harness::canonical(&folder), "From another Mac"));
            assert!(ws.workspace_projects().iter().any(|p| p.id == id), "among the user's projects, offered to phones");
            assert_eq!(ws.route, route, "this Mac's window stays where it was");
        });
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|m| matches!(m, tr::ServerMessage::CloneProgress { line, .. } if line.starts_with("Cloning into"))),
            "git's output goes to the machine that asked: {lines:?}"
        );
    });
}

#[test]
fn a_clone_into_a_folder_with_files_in_it_is_refused() {
    run(async |cx| {
        let trek = open(cx);
        let source = bare_repo();
        let folder = super::harness::new_project("occupied");
        std::fs::write(folder.join("notes.txt"), "mine\n").unwrap();
        let before = trek.read(cx, |ws, _| ws.projects.len());
        let req = clone_request(&source, &folder, "occupied");
        let progress = tr::CloneProgress::new(folder.display().to_string(), |_| {});
        let err = ask_later(&trek, cx, |reply| tr::HostRequest::CloneProject { req, progress, reply }).await.unwrap_err();
        assert_eq!(err.code, tr::ErrorCode::Conflict, "{err}");
        assert_eq!(std::fs::read_to_string(folder.join("notes.txt")).unwrap(), "mine\n", "left as it was");
        assert!(!folder.join("README.md").exists());
        assert_eq!(trek.read(cx, |ws, _| ws.projects.len()), before, "nothing registered");

        // A clone git can't make says what git said.
        let missing = super::harness::new_project("nowhere").join("missing.git");
        let into = super::harness::new_project("failed").join("x");
        let req = clone_request(&missing, &into, "x");
        let progress = tr::CloneProgress::new(into.display().to_string(), |_| {});
        let err = ask_later(&trek, cx, |reply| tr::HostRequest::CloneProject { req, progress, reply }).await.unwrap_err();
        assert_eq!(err.code, tr::ErrorCode::CloneFailed, "{err}");
        assert!(err.message.contains("fatal:"), "{err}");
    });
}

#[test]
fn put_files_writes_inside_the_project_and_never_through_a_symlink() {
    run(async |cx| {
        let trek = open(cx);
        let root = super::harness::new_project("uploads");
        let id = trek.update(cx, |ws, cx| ws.register_remote_project(&root, "Uploads", cx)).unwrap();
        let file = |path: &str, bytes: &[u8], mode: u32| tr::FileUpload { path: path.into(), bytes: bytes.to_vec(), mode };
        let files = vec![file(".env", b"KEY=1\n", 0o600), file("config/dev/local.toml", b"port = 1\n", 0o644)];
        let project_id = id.clone();
        ask_later(&trek, cx, |reply| tr::HostRequest::PutFiles { project_id, files, reply }).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join(".env")).unwrap(), "KEY=1\n");
        assert_eq!(std::fs::read_to_string(super::harness::join(&root, "config/dev/local.toml")).unwrap(), "port = 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(root.join(".env")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Again: replaced.
        let (project_id, files) = (id.clone(), vec![file(".env", b"KEY=2\n", 0o600)]);
        ask_later(&trek, cx, |reply| tr::HostRequest::PutFiles { project_id, files, reply }).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join(".env")).unwrap(), "KEY=2\n");

        // A path out of the project, or an unknown project, is refused whatever the server checked.
        let (project_id, files) = (id.clone(), vec![file("../escaped", b"x", 0o644)]);
        let err = ask_later(&trek, cx, |reply| tr::HostRequest::PutFiles { project_id, files, reply }).await.unwrap_err();
        assert_eq!(err.code, tr::ErrorCode::BadRequest, "{err}");
        assert!(!root.parent().unwrap().join("escaped").exists());
        let files = vec![file("a", b"x", 0o644)];
        let err = ask_later(&trek, cx, |reply| tr::HostRequest::PutFiles { project_id: "nope".into(), files, reply }).await.unwrap_err();
        assert_eq!(err.code, tr::ErrorCode::NotFound);

        #[cfg(unix)]
        {
            let outside = super::harness::new_project("outside");
            std::fs::write(outside.join("secret"), "untouched\n").unwrap();
            std::os::unix::fs::symlink(outside.join("secret"), root.join("linked")).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
            for path in ["linked", "out/new", "out/secret"] {
                // With a good file first: nothing is written when one can't be.
                let (project_id, files) = (id.clone(), vec![file("first.txt", b"1", 0o644), file(path, b"overwritten\n", 0o644)]);
                let err = ask_later(&trek, cx, |reply| tr::HostRequest::PutFiles { project_id, files, reply }).await.unwrap_err();
                assert_eq!(err.code, tr::ErrorCode::Conflict, "{path}: {err}");
                assert!(err.message.contains("symbolic link"), "{path}: {err}");
                assert!(!root.join("first.txt").exists(), "{path}");
            }
            assert_eq!(std::fs::read_to_string(outside.join("secret")).unwrap(), "untouched\n");
            assert!(!outside.join("new").exists());
            assert!(std::fs::symlink_metadata(root.join("linked")).unwrap().file_type().is_symlink(), "the link itself stays");
        }
    });
}
