//! Whether Windows Firewall lets phones on the network reach the phone server: the rules it holds
//! for Trek's own program (and for the server's port), as it applies them to the networks this PC
//! is on now. Nothing noticed a block before, and the phone simply couldn't connect. Windows asks
//! once, the first time Trek listens, and writes rules for the kinds of network ticked then: on a
//! network of another kind (a Wi-Fi marked public, Tailscale's private one) Trek has no rule, the
//! firewall's default keeps phones out, and Windows doesn't ask again. Settings › iPhone says so,
//! naming the network (`settings_view::mobile`).
//!
//! `decide` and `Firewall::note` are plain code, tested on both CIs; `read` asks the firewall (its
//! COM policy, which a standard user may read) and the network list for the networks' names and
//! kinds, Windows' alone.

// Elsewhere only the tests use it; Windows reads the firewall with it.
#![cfg_attr(not(windows), allow(dead_code))]

/// The firewall's network profiles, as its bits.
const DOMAIN: i32 = 1;
const PRIVATE: i32 = 2;
const PUBLIC: i32 = 4;

/// What the firewall does to a phone dialing in on one kind of network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Lets it in: the firewall is off there, a rule allows Trek, or its default lets anything in.
    Open,
    /// No rule for Trek anywhere yet: Windows asks once the server listens, and keeps phones out
    /// until it's answered.
    NotAsked,
    /// Keeps it out: a rule blocks Trek, Block all incoming is on, or Trek has rules for other
    /// kinds of network and none for this one (Windows asked once, and won't again).
    Blocked,
}

/// A network profile this PC is on now, as the firewall applies it.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub bit: i32,
    pub enabled: bool,
    pub block_all: bool,
    /// Its default for a program with no rule is to let it in (Windows' own default blocks).
    pub default_allow: bool,
}

/// An inbound rule that matches Trek's program, or the server's port with no program.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub profiles: i32,
    pub allow: bool,
}

/// What the firewall does to a phone dialing in on each kind of network this PC is on, by the
/// kind's name. A block rule wins over an allowing one, as the firewall's own order has it.
pub fn decide(current: &[Profile], rules: &[Rule]) -> Vec<(&'static str, Verdict)> {
    current
        .iter()
        .map(|p| {
            let applying: Vec<&Rule> = rules.iter().filter(|r| r.profiles & p.bit != 0).collect();
            let verdict = if !p.enabled {
                Verdict::Open
            } else if p.block_all || applying.iter().any(|r| !r.allow) {
                Verdict::Blocked
            } else if !applying.is_empty() || p.default_allow {
                Verdict::Open
            } else if rules.is_empty() {
                Verdict::NotAsked
            } else {
                Verdict::Blocked
            };
            (kind(p.bit), verdict)
        })
        .collect()
}

/// A profile's name, as Windows' network settings say a network's kind.
fn kind(bit: i32) -> &'static str {
    match bit {
        DOMAIN => "domain",
        PRIVATE => "private",
        _ => "public",
    }
}

/// The firewall as it stands for Trek's phone server: each kind of network this PC is on, what
/// the firewall does there, and the connected networks of that kind by name (Wi-Fi's, Tailscale).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firewall {
    pub kinds: Vec<(&'static str, Verdict, Vec<String>)>,
}

impl Firewall {
    /// What Settings › iPhone says while the server runs, when the firewall keeps phones out
    /// somewhere: `None` where it lets them in everywhere.
    pub fn note(&self) -> Option<String> {
        let mut said = vec![];
        for (kind, verdict, names) in &self.kinds {
            let networks = match names.as_slice() {
                [] => format!("this PC's {kind} network"),
                [one] => format!("{one} (a {kind} network)"),
                many => format!("{} ({kind} networks)", many.join(" and ")),
            };
            match verdict {
                Verdict::Open => {}
                Verdict::NotAsked => said.push(format!(
                    "Windows Firewall keeps phones out on {networks} until its question is answered with Allow."
                )),
                Verdict::Blocked => {
                    let other = if *kind == "public" {
                        let name = names.first().map_or("it", String::as_str);
                        format!(", or make {name} a private network in Windows' network settings")
                    } else {
                        String::new()
                    };
                    said.push(format!(
                        "Windows Firewall keeps phones out on {networks}, so one there can't reach this PC. Allow Trek on {kind} networks in the firewall settings{other}."
                    ));
                }
            }
        }
        (!said.is_empty()).then(|| said.join(" "))
    }
}

/// Whether rule program `app` is `exe` (any case, `%VAR%`s as they stand for).
fn same_program(app: &str, exe: &std::path::Path) -> bool {
    let mut expanded = app.trim().to_string();
    while let Some(start) = expanded.find('%') {
        let Some(len) = expanded[start + 1..].find('%') else {
            break;
        };
        let var = &expanded[start + 1..start + 1 + len];
        let Ok(value) = std::env::var(var) else { break };
        expanded.replace_range(start..start + len + 2, &value);
    }
    expanded.eq_ignore_ascii_case(&exe.to_string_lossy())
}

/// Whether a rule's local ports (`7431`, `7000-8000`, `7431,8080`, `*`) take `port`.
fn takes_port(ports: &str, port: u16) -> bool {
    ports
        .split(',')
        .map(str::trim)
        .any(|p| match p.split_once('-') {
            _ if p == "*" => true,
            Some((from, to)) => matches!(
                (from.trim().parse::<u16>(), to.trim().parse::<u16>()),
                (Ok(from), Ok(to)) if (from..=to).contains(&port)
            ),
            None => p.parse() == Ok(port),
        })
}

/// The firewall as it stands for `exe` serving on `port`. `None` when it can't be read.
#[cfg(windows)]
pub fn read(exe: &std::path::Path, port: u16) -> Option<Firewall> {
    use windows::Win32::NetworkManagement::WindowsFirewall::{
        INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_IP_PROTOCOL_ANY,
        NET_FW_IP_PROTOCOL_TCP, NET_FW_PROFILE_TYPE2, NET_FW_RULE_DIR_IN, NetFwPolicy2,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IDispatch,
    };
    use windows::Win32::System::Ole::IEnumVARIANT;
    use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH};
    use windows::core::Interface as _;

    // SAFETY: COM on this (background) thread for the length of the read, and the firewall's own
    // objects, used as its documentation has them.
    unsafe {
        let started = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let read = (|| -> windows::core::Result<Firewall> {
            let policy: INetFwPolicy2 =
                CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
            let bits = policy.CurrentProfileTypes()?;
            let mut current = vec![];
            for bit in [DOMAIN, PRIVATE, PUBLIC] {
                if bits & bit == 0 {
                    continue;
                }
                let profile = NET_FW_PROFILE_TYPE2(bit);
                current.push(Profile {
                    bit,
                    enabled: policy.get_FirewallEnabled(profile)?.as_bool(),
                    block_all: policy.get_BlockAllInboundTraffic(profile)?.as_bool(),
                    default_allow: policy.get_DefaultInboundAction(profile)? == NET_FW_ACTION_ALLOW,
                });
            }
            let mut rules = vec![];
            let all: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
            loop {
                let mut item = [VARIANT::default()];
                let mut fetched = 0;
                if all.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                // Each rule comes as its IDispatch (VT_DISPATCH), which the item lets go of.
                let value = &item[0].Anonymous.Anonymous;
                if value.vt != VT_DISPATCH {
                    continue;
                }
                let dispatch: Option<IDispatch> = (*value.Anonymous.pdispVal).clone();
                let Some(Ok(rule)) = dispatch.map(|d| d.cast::<INetFwRule>()) else {
                    continue;
                };
                let inbound = rule.Direction().is_ok_and(|d| d == NET_FW_RULE_DIR_IN);
                let enabled = rule.Enabled().is_ok_and(|e| e.as_bool());
                let tcp = rule
                    .Protocol()
                    .is_ok_and(|p| p == NET_FW_IP_PROTOCOL_TCP.0 || p == NET_FW_IP_PROTOCOL_ANY.0);
                if !(inbound && enabled && tcp) {
                    continue;
                }
                let app = rule
                    .ApplicationName()
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                let ours = if app.trim().is_empty() {
                    rule.LocalPorts()
                        .is_ok_and(|ports| takes_port(&ports.to_string(), port))
                } else {
                    same_program(&app, exe)
                };
                if ours {
                    rules.push(Rule {
                        profiles: rule.Profiles()?,
                        allow: rule.Action()? == NET_FW_ACTION_ALLOW,
                    });
                }
            }
            let names = networks();
            let kinds = decide(&current, &rules)
                .into_iter()
                .map(|(kind, verdict)| {
                    let of_kind = names
                        .iter()
                        .filter(|(_, k)| *k == kind)
                        .map(|(name, _)| name.clone())
                        .collect();
                    (kind, verdict, of_kind)
                })
                .collect();
            Ok(Firewall { kinds })
        })();
        if started {
            CoUninitialize();
        }
        read.inspect_err(|e| tracing::info!("firewall: couldn't read it ({e})"))
            .ok()
    }
}

/// Elsewhere there's no Windows Firewall to read.
#[cfg(not(windows))]
pub fn read(_exe: &std::path::Path, _port: u16) -> Option<Firewall> {
    None
}

/// Windows' page of the apps its firewall lets in (Control Panel's Allowed apps), where Trek can
/// be allowed on a kind of network; changing it there asks for an administrator.
pub fn open_settings() {
    #[cfg(windows)]
    if let Err(e) = trek_core::procs::command("control.exe")
        .args([
            "/name",
            "Microsoft.WindowsFirewall",
            "/page",
            "pageConfigureApps",
        ])
        .spawn()
    {
        tracing::warn!("firewall: couldn't open its settings ({e})");
    }
}

/// The networks this PC is connected to, by name, with their kind (Windows' network list, on a
/// thread where COM is up). Empty when it can't be read: the note then says the kind alone.
#[cfg(windows)]
unsafe fn networks() -> Vec<(String, &'static str)> {
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, NLM_ENUM_NETWORK_CONNECTED, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
        NLM_NETWORK_CATEGORY_PRIVATE, NetworkListManager,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    // SAFETY: the network list's own objects, used as its documentation has them.
    let read = || -> windows::core::Result<Vec<(String, &'static str)>> {
        let list: INetworkListManager =
            unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)? };
        let all = unsafe { list.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)? };
        let mut out = vec![];
        loop {
            let mut one = [None];
            let mut fetched = 0u32;
            if unsafe { all.Next(&mut one, Some(&mut fetched as *mut u32)) }.is_err()
                || fetched == 0
            {
                break;
            }
            let Some(network) = one[0].take() else { break };
            let name = unsafe { network.GetName() }
                .map(|n| n.to_string())
                .unwrap_or_default();
            let kind = match unsafe { network.GetCategory() } {
                Ok(c) if c == NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => "domain",
                Ok(c) if c == NLM_NETWORK_CATEGORY_PRIVATE => "private",
                _ => "public",
            };
            if !name.is_empty() {
                out.push((name, kind));
            }
        }
        Ok(out)
    };
    read().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(bit: i32) -> Profile {
        Profile {
            bit,
            enabled: true,
            block_all: false,
            default_allow: false,
        }
    }

    fn rule(profiles: i32, allow: bool) -> Rule {
        Rule { profiles, allow }
    }

    /// Windows asks once and writes rules for the kinds ticked then: another kind of network has
    /// none, the firewall's default keeps phones out, and nothing asks again. As found on a PC
    /// whose Wi-Fi is public, Trek allowed there, and Tailscale private.
    #[test]
    fn a_kind_of_network_the_first_question_left_out_keeps_phones_out() {
        let answered = [rule(PUBLIC, true)];
        assert_eq!(
            decide(&[on(PRIVATE), on(PUBLIC)], &answered),
            [("private", Verdict::Blocked), ("public", Verdict::Open)]
        );
        // Asked on a private network, with public left unticked: a block rule there.
        let answered = [rule(PRIVATE, true), rule(PUBLIC, false)];
        assert_eq!(
            decide(&[on(PUBLIC)], &answered),
            [("public", Verdict::Blocked)]
        );
        // The firewall off there, or letting anything in by default: nothing in the way.
        let off = Profile {
            enabled: false,
            ..on(PRIVATE)
        };
        let lenient = Profile {
            default_allow: true,
            ..on(PRIVATE)
        };
        let only_public = [rule(PUBLIC, true)];
        assert_eq!(decide(&[off], &only_public), [("private", Verdict::Open)]);
        assert_eq!(
            decide(&[lenient], &only_public),
            [("private", Verdict::Open)]
        );
    }

    /// No rule at all is a question Windows will ask; Block all incoming, and a block beside an
    /// allow, keep phones out whatever the rules.
    #[test]
    fn no_rule_is_a_question_and_a_block_wins() {
        assert_eq!(
            decide(&[on(PRIVATE)], &[]),
            [("private", Verdict::NotAsked)]
        );
        let all = Profile {
            block_all: true,
            ..on(PRIVATE)
        };
        let both = rule(PRIVATE | PUBLIC, true);
        assert_eq!(decide(&[all], &[both]), [("private", Verdict::Blocked)]);
        assert_eq!(
            decide(&[on(PRIVATE)], &[both, rule(PRIVATE, false)]),
            [("private", Verdict::Blocked)]
        );
        assert_eq!(
            decide(&[on(DOMAIN)], &[both]),
            [("domain", Verdict::Blocked)]
        );
    }

    /// The note names the network and says what to do, and says nothing where phones get in.
    #[test]
    fn the_note_names_the_network_and_the_way_in() {
        let firewall = |kinds: Vec<(&'static str, Verdict, Vec<String>)>| Firewall { kinds };
        assert_eq!(
            firewall(vec![("public", Verdict::Open, vec!["Home".into()])]).note(),
            None
        );
        let note = firewall(vec![
            ("private", Verdict::Blocked, vec!["Tailscale".into()]),
            ("public", Verdict::Open, vec!["Home".into()]),
        ])
        .note()
        .unwrap();
        assert_eq!(
            note,
            "Windows Firewall keeps phones out on Tailscale (a private network), so one there can't reach this PC. Allow Trek on private networks in the firewall settings."
        );
        let note = firewall(vec![("public", Verdict::Blocked, vec!["Cafe".into()])])
            .note()
            .unwrap();
        assert!(
            note.ends_with("Allow Trek on public networks in the firewall settings, or make Cafe a private network in Windows' network settings."),
            "{note}"
        );
        let note = firewall(vec![("private", Verdict::NotAsked, vec![])])
            .note()
            .unwrap();
        assert!(
            note.contains("this PC's private network until its question is answered"),
            "{note}"
        );
    }

    #[test]
    fn a_rule_is_trek_s_by_its_program_or_the_server_s_port() {
        let exe = std::path::Path::new(r"C:\Users\me\AppData\Local\Programs\Trek\trek.exe");
        assert!(same_program(
            r"c:\users\me\appdata\local\programs\trek\TREK.EXE",
            exe
        ));
        assert!(!same_program(r"C:\Program Files\Other\trek.exe", exe));
        for (ports, takes) in [
            ("7431", true),
            ("7000-8000", true),
            ("80,7431", true),
            ("*", true),
            ("80", false),
            ("8000-9000", false),
            ("", false),
        ] {
            assert_eq!(takes_port(ports, 7431), takes, "{ports}");
        }
    }

    /// A real Windows Firewall and network list read, for whatever program (the test's own has
    /// no rule): each kind of network the PC is on has its verdict.
    #[cfg(windows)]
    #[test]
    fn the_real_firewall_reads() {
        let exe = std::env::current_exe().unwrap();
        let firewall = read(&exe, 7431).expect("the firewall's policy reads");
        assert!(!firewall.kinds.is_empty(), "{firewall:?}");
    }

    /// What the firewall does to the installed Trek on this PC's networks, said as Settings ›
    /// iPhone says it. Read only. By hand: `cargo test -p trek-app --lib
    /// installed_trek_s_firewall -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads this PC's firewall for the installed Trek, which a CI runner has none of"]
    fn installed_trek_s_firewall() {
        let local = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA");
        let exe = std::path::Path::new(&local).join(r"Programs\Trek\trek.exe");
        let firewall = read(&exe, 7431).expect("the firewall's policy reads");
        println!("{firewall:?}\n{:?}", firewall.note());
    }
}
