//! Why iPhone Mirroring can't reach the phone: the Mac-side checks, and the one blocker they
//! point at, in plain words (`mirror diagnose`).
//!
//! What a timed-out Connect looks like from the Mac (seen on macOS 27.2): iPhone Mirroring runs its
//! own checks (Mac eligible, not restricted by a profile, iCloud, Wi-Fi, Replicator paired, the
//! phone onboarded, Bluetooth), then asks rapportd to find the phone's
//! `com.apple.MediaContinuityKit.iPhoneMirroring` service over Bluetooth LE (RSSI threshold -70)
//! for ten seconds, logs "Failed to find endpoint" and tears the session down
//! ("bringupTimedOut … sessionInitializing"). From 2026-10-31 to 11-02 all 24 Connects ended that
//! way, with the phone locked and beside the Mac: rapportd saw it over Bluetooth as a device of
//! the same Apple Account (RSSI -58, idle) and found its other services (Siri's companion link,
//! widgets) over Wi-Fi, but never its iPhone Mirroring one. The phone just isn't offering iPhone
//! Mirroring then, which is the phone's to fix: Handoff and the Mac's entry under iPhone Mirroring
//! in its settings, a restart, then Revoke iPhone Access and setting it up again. A Connect that
//! finds the phone and then drops is a network one (a VPN, the firewall, Wi-Fi). Everything the
//! Mac can say for itself is checked first. See docs/IPHONE-MIRRORING.md.

use std::process::Command;
use std::time::Duration;

/// What the Mac knows, each `None` when it couldn't tell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checks {
    /// The Mac's own verdict on whether iPhone Mirroring is offered where it is (`os_eligibility`,
    /// from its Apple Account's billing country and its location): `Some(false)` in the EU.
    pub mac_eligible: Option<bool>,
    /// A management profile turns iPhone Mirroring off on the Mac (`allowiPhoneMirroring`).
    pub mac_restricted: bool,
    /// The Mac's Handoff setting, which iPhone Mirroring needs.
    pub mac_handoff: Option<bool>,
    /// The Mac's Wi-Fi is on.
    pub wifi_on: Option<bool>,
    /// The Mac's Wi-Fi has an address: it's on a network.
    pub wifi_joined: Option<bool>,
    pub bluetooth_on: Option<bool>,
    /// The firewall's "Block all incoming connections".
    pub firewall_blocks_all: bool,
    /// Internet Sharing is on (iPhone Mirroring doesn't work then).
    pub internet_sharing: bool,
    /// A VPN that's connected, by name.
    pub vpn: Option<String>,
    /// The phone shows on the Mac's network (usbmuxd over Wi-Fi): `Some(false)` only when it
    /// shows itself on Wi-Fi at all and isn't there.
    pub phone_on_wifi: Option<bool>,
    /// The phone's screen is off (from the cable's picture).
    pub phone_locked: Option<bool>,
    /// How iPhone Mirroring's last Connect ended, from its log.
    pub attempt: Option<Attempt>,
}

/// How iPhone Mirroring's last Connect ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// It never found the phone's iPhone Mirroring service ("Failed to find endpoint").
    NotFound,
    /// It found the phone, then lost or never made the connection.
    Dropped,
    /// No iPhone is set up for it on the Mac ("noCompatiblePhone": Replicator has none paired).
    NotSetUp,
    /// The iPhone turned it down for its country or region (`SFAuthenticationErrorCodeRegionLocked`).
    RegionLocked,
}

/// What one Connect's log lines said.
#[derive(Default, Clone, Copy)]
struct Seen {
    not_found: bool,
    down: bool,
    not_set_up: bool,
    region: bool,
}

impl Seen {
    fn attempt(self) -> Option<Attempt> {
        if self.region {
            Some(Attempt::RegionLocked)
        } else if self.not_set_up {
            Some(Attempt::NotSetUp)
        } else if self.not_found {
            Some(Attempt::NotFound)
        } else if self.down {
            Some(Attempt::Dropped)
        } else {
            None
        }
    }
}

impl Attempt {
    /// The last finished attempt in `log show --style compact` lines of iPhone Mirroring:
    /// each starts at "activate is in flight"; one still going counts as none.
    pub fn from_log(log: &str) -> Option<Attempt> {
        let mut last = None;
        let mut current: Option<Seen> = None;
        let finish = |c: Option<Seen>, last: &mut Option<Attempt>| {
            if let Some(a) = c.and_then(Seen::attempt) {
                *last = Some(a);
            }
        };
        for line in log.lines() {
            // Set-up and region failures can end a Connect before one is in flight.
            if line.contains("noCompatiblePhone") || line.contains("RegionLocked") {
                let c = current.get_or_insert_with(Seen::default);
                c.not_set_up |= line.contains("noCompatiblePhone");
                c.region |= line.contains("RegionLocked");
                c.down = true;
            } else if line.contains("activate is in flight") {
                finish(current.take(), &mut last);
                current = Some(Seen::default());
            } else if let Some(c) = current.as_mut() {
                if line.contains("Failed to find endpoint") {
                    c.not_found = true;
                } else if line.contains("Tearing down the session due to")
                    || line.contains("was interrupted by")
                {
                    c.down = true;
                }
            }
        }
        finish(current, &mut last);
        last
    }
}

/// The one thing in the way, most likely first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// Apple doesn't offer iPhone Mirroring where the Mac is (the EU).
    MacNotEligible,
    /// A management profile turns it off on the Mac.
    MacRestricted,
    MacWifiOff,
    MacBluetoothOff,
    MacHandoffOff,
    InternetSharing,
    MacWifiNotJoined,
    PhoneUnlocked,
    PhoneOffWifi,
    FirewallBlocksAll,
    /// Found the phone, then lost the connection; a VPN by name when one is on.
    ConnectionDrops(Option<String>),
    /// No iPhone set up for iPhone Mirroring on the Mac.
    NotSetUp,
    /// The iPhone turns iPhone Mirroring down in its country or region (the EU).
    PhoneRegionLocked,
    /// Locked, nearby and on the network, but not offering iPhone Mirroring.
    PhoneNotOffering,
    Unknown,
}

/// The single most likely blocker.
pub fn blocker(c: &Checks) -> Blocker {
    if c.mac_eligible == Some(false) {
        Blocker::MacNotEligible
    } else if c.mac_restricted {
        Blocker::MacRestricted
    } else if c.wifi_on == Some(false) {
        Blocker::MacWifiOff
    } else if c.bluetooth_on == Some(false) {
        Blocker::MacBluetoothOff
    } else if c.mac_handoff == Some(false) {
        Blocker::MacHandoffOff
    } else if c.internet_sharing {
        Blocker::InternetSharing
    } else if c.wifi_joined == Some(false) {
        Blocker::MacWifiNotJoined
    } else if c.phone_locked == Some(false) {
        Blocker::PhoneUnlocked
    } else if c.phone_on_wifi == Some(false) {
        Blocker::PhoneOffWifi
    } else if c.firewall_blocks_all {
        Blocker::FirewallBlocksAll
    } else {
        match c.attempt {
            Some(Attempt::RegionLocked) => Blocker::PhoneRegionLocked,
            Some(Attempt::NotSetUp) => Blocker::NotSetUp,
            Some(Attempt::Dropped) => Blocker::ConnectionDrops(c.vpn.clone()),
            Some(Attempt::NotFound) => Blocker::PhoneNotOffering,
            None => Blocker::Unknown,
        }
    }
}

impl Blocker {
    /// One short line for the iPhone tool, the action first.
    pub fn action(&self) -> String {
        match self {
            Blocker::MacNotEligible => "iPhone Mirroring isn't offered in the Mac's country or region (it's off in the EU), so Trek can't tap through it here.".into(),
            Blocker::MacRestricted => "The Mac's management profile turns iPhone Mirroring off: ask whoever manages the Mac to allow it.".into(),
            Blocker::MacWifiOff => "Turn the Mac's Wi-Fi on: iPhone Mirroring needs it.".into(),
            Blocker::MacBluetoothOff => {
                "Turn the Mac's Bluetooth on: iPhone Mirroring finds the iPhone over it.".into()
            }
            Blocker::MacHandoffOff => "Turn Handoff on in System Settings › General › AirDrop & Continuity: iPhone Mirroring needs it on the Mac and the iPhone.".into(),
            Blocker::InternetSharing => "Turn Internet Sharing off in System Settings › General › Sharing: iPhone Mirroring can't connect while the Mac shares its connection.".into(),
            Blocker::MacWifiNotJoined => "Join the Mac to the iPhone's Wi-Fi network.".into(),
            Blocker::PhoneUnlocked => "Lock the iPhone: iPhone Mirroring connects only while it's locked.".into(),
            Blocker::PhoneOffWifi => "Put the iPhone on the Mac's Wi-Fi network, with its Wi-Fi and Bluetooth on: it isn't on that network now.".into(),
            Blocker::FirewallBlocksAll => "Let the Mac's firewall take incoming connections: System Settings › Network › Firewall › Options, turn off Block all incoming connections.".into(),
            Blocker::ConnectionDrops(Some(vpn)) => format!("iPhone Mirroring finds the iPhone but loses the connection: turn {vpn} off on the Mac and the iPhone, then try again."),
            Blocker::ConnectionDrops(None) => "iPhone Mirroring finds the iPhone but loses the connection: keep it close, on the Mac's Wi-Fi, then try again.".into(),
            Blocker::NotSetUp => "Set iPhone Mirroring up again: open it on the Mac with the iPhone locked and nearby, and follow its steps.".into(),
            Blocker::PhoneRegionLocked => "The iPhone turns iPhone Mirroring down in its country or region (it's off in the EU), so Trek can't tap through it there.".into(),
            Blocker::PhoneNotOffering => "The iPhone is nearby but isn't offering iPhone Mirroring: on the iPhone, in Settings › General › AirPlay & Continuity, turn Handoff on and check the Mac is listed under iPhone Mirroring; then restart the iPhone and lock it.".into(),
            Blocker::Unknown => "Turn Wi-Fi and Bluetooth on for both, on the same network, with the iPhone nearby.".into(),
        }
    }

    /// For an agent to pass on, the action first.
    pub fn advice(&self) -> String {
        match self {
            Blocker::PhoneNotOffering => "Ask the user, on the iPhone, to open Settings › General › AirPlay & Continuity, turn Handoff on and check the Mac is listed under iPhone Mirroring, then restart the iPhone, lock it and leave it by the Mac: iPhone Mirroring can't find it although it's locked, nearby and on the same Wi-Fi (its log: \"Failed to find endpoint\"), so the phone isn't offering iPhone Mirroring. If the Mac isn't listed, or that doesn't do it: remove the Mac there, choose Revoke iPhone Access in iPhone Mirroring › Settings on the Mac, then set iPhone Mirroring up again. Screenshots, apps and files still work over USB.".into(),
            Blocker::NotSetUp => "Ask the user to open iPhone Mirroring on the Mac with the iPhone locked and nearby, and follow its steps: no iPhone is set up for it (its log: \"noCompatiblePhone\"). If it won't set up: remove the Mac on the iPhone under Settings › General › AirPlay & Continuity › iPhone Mirroring, choose Revoke iPhone Access in iPhone Mirroring › Settings on the Mac, then try again.".into(),
            Blocker::MacNotEligible | Blocker::PhoneRegionLocked => format!("Tell the user there's nothing to fix: {} Taps still go over USB through Trek Driver while the iPhone is unlocked; screenshots, apps and files work over USB.", self.action()),
            Blocker::PhoneUnlocked => "Ask the user to lock the iPhone (side button) and leave it nearby, then try again: iPhone Mirroring connects only while the iPhone is locked.".into(),
            other => format!("Ask the user to fix this, then try again: {}", other.action()),
        }
    }
}

/// The diagnosis: what was checked and what it points at.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnosis {
    pub checks: Checks,
    pub blocker: Blocker,
}

impl Diagnosis {
    /// The checks as lines, for `mirror diagnose`.
    pub fn report(&self) -> String {
        let c = &self.checks;
        let yes = |v: Option<bool>, y: &str, n: &str| match v {
            Some(true) => y.to_string(),
            Some(false) => n.to_string(),
            None => "unknown".to_string(),
        };
        [
            format!(
                "iPhone Mirroring offered where the Mac is: {}",
                yes(c.mac_eligible, "yes", "no")
            ),
            format!(
                "Turned off by a management profile: {}",
                if c.mac_restricted { "yes" } else { "no" }
            ),
            format!("Mac Wi-Fi: {}", yes(c.wifi_on, "on", "off")),
            format!("Mac on a network: {}", yes(c.wifi_joined, "yes", "no")),
            format!("Mac Bluetooth: {}", yes(c.bluetooth_on, "on", "off")),
            format!("Mac Handoff: {}", yes(c.mac_handoff, "on", "off")),
            format!(
                "Internet Sharing: {}",
                if c.internet_sharing { "on" } else { "off" }
            ),
            format!(
                "Firewall blocks all incoming: {}",
                if c.firewall_blocks_all { "yes" } else { "no" }
            ),
            format!("VPN: {}", c.vpn.as_deref().unwrap_or("none")),
            format!(
                "iPhone on the Mac's network: {}",
                yes(c.phone_on_wifi, "yes", "no")
            ),
            format!("iPhone locked: {}", yes(c.phone_locked, "yes", "no")),
            format!(
                "Last Connect: {}",
                match c.attempt {
                    Some(Attempt::NotFound) => "never found the iPhone (Failed to find endpoint)",
                    Some(Attempt::Dropped) => "found the iPhone, then lost it",
                    Some(Attempt::NotSetUp) => "no iPhone set up for it (noCompatiblePhone)",
                    Some(Attempt::RegionLocked) =>
                        "the iPhone turned it down for its region (RegionLocked)",
                    None => "none in the last minutes",
                }
            ),
            format!("Most likely: {}", self.blocker.action()),
        ]
        .join("\n")
    }
}

// MARK: reading the Mac

/// The Wi-Fi interface ("en0") in `networksetup -listallhardwareports`.
pub fn wifi_device(ports: &str) -> Option<String> {
    let mut lines = ports.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "Hardware Port: Wi-Fi" {
            return lines
                .next()
                .and_then(|l| l.trim().strip_prefix("Device:"))
                .map(|d| d.trim().to_string());
        }
    }
    None
}

/// `networksetup -getairportpower en0`: "Wi-Fi Power (en0): On".
pub fn power(text: &str) -> Option<bool> {
    let word = text.trim().rsplit(':').next()?.trim().to_ascii_lowercase();
    match word.as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// `socketfilterfw --getblockall`: "Firewall has block all state set to enabled."
pub fn blocks_all(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("enabled") && !t.contains("disabled")
}

/// The connected VPN's name in `scutil --nc list`.
pub fn connected_vpn(list: &str) -> Option<String> {
    list.lines()
        .filter(|l| l.contains("(Connected)"))
        .find_map(|l| l.split('"').nth(1).map(str::to_string))
}

/// `system_profiler SPBluetoothDataType -json`: the controller's state.
pub fn bluetooth_state(json: &str) -> Option<bool> {
    let at = json.find("\"controller_state\"")?;
    let line = json[at..].lines().next()?;
    if line.contains("attrib_on") {
        Some(true)
    } else if line.contains("attrib_off") {
        Some(false)
    } else {
        None
    }
}

/// `plutil -extract OS_ELIGIBILITY_DOMAIN_IRON.os_eligibility_answer_t raw eligibility.plist`:
/// 4 is eligible, 2 is not (iPhone Mirroring's domain is IRON; its inputs are the Apple Account's
/// billing country and the Mac's location).
pub fn eligible(answer: &str) -> Option<bool> {
    match answer.trim() {
        "4" => Some(true),
        "2" => Some(false),
        _ => None,
    }
}

/// `plutil -extract allowiPhoneMirroring raw` on a managed `com.apple.applicationaccess` plist:
/// "false" when a profile turns iPhone Mirroring off.
pub fn disallowed(value: &str) -> bool {
    matches!(value.trim(), "false" | "0")
}

/// `defaults -currentHost read com.apple.coreservices.useractivityd` for
/// `ActivityAdvertisingAllowed` and `ActivityReceivingAllowed`: Handoff is off when either is 0;
/// a key that isn't there is the default, on. `None` when neither could be read.
pub fn handoff_on(values: &[Option<String>]) -> Option<bool> {
    if values.iter().all(Option::is_none) {
        None
    } else {
        Some(!values.iter().flatten().any(|v| v.trim() == "0"))
    }
}

/// A command's output, or nothing when it fails or takes too long.
fn output(program: &str, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = Command::new(program) // spawn: macOS only, and trek-phone has no trek-core
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
    Some(out)
}

/// iPhone Mirroring's last few minutes of Connects, from the unified log.
pub fn mirroring_log(minutes: u32) -> Option<String> {
    output(
        "/usr/bin/log",
        &[
            "show",
            "--last",
            &format!("{minutes}m"),
            "--style",
            "compact",
            "--predicate",
            "process == \"iPhone Mirroring\" AND (eventMessage CONTAINS \"activate is in flight\" OR eventMessage CONTAINS \"noCompatiblePhone\" OR eventMessage CONTAINS \"RegionLocked\" OR eventMessage CONTAINS \"Failed to find endpoint\" OR eventMessage CONTAINS \"Tearing down the session due to\" OR eventMessage CONTAINS \"was interrupted by\")",
        ],
        Duration::from_secs(20),
    )
}

/// Whether the phone shows on the Mac's network now (`None`: it can't tell). Blocking.
pub fn phone_seen(udid: Option<&str>) -> Option<bool> {
    match crate::device::on_network(udid) {
        Ok(_) => Some(true),
        Err(true) => Some(false),
        Err(false) => None,
    }
}

/// Run every check (blocking, a few seconds): `bluetooth_on` and `phone_locked` come from the
/// helper when Trek knows them.
pub fn run(
    udid: Option<&str>,
    phone_locked: Option<bool>,
    bluetooth_on: Option<bool>,
) -> Diagnosis {
    let quick = Duration::from_secs(5);
    let wifi = output("/usr/sbin/networksetup", &["-listallhardwareports"], quick)
        .and_then(|t| wifi_device(&t))
        .unwrap_or_else(|| "en0".into());
    let wifi_on = output(
        "/usr/sbin/networksetup",
        &["-getairportpower", &wifi],
        quick,
    )
    .and_then(|t| power(&t));
    let wifi_joined =
        output("/usr/sbin/ipconfig", &["getifaddr", &wifi], quick).map(|t| !t.trim().is_empty());
    let bluetooth_on = bluetooth_on.or_else(|| {
        output(
            "/usr/sbin/system_profiler",
            &["SPBluetoothDataType", "-json"],
            Duration::from_secs(15),
        )
        .and_then(|t| bluetooth_state(&t))
    });
    let firewall_blocks_all = output(
        "/usr/libexec/ApplicationFirewall/socketfilterfw",
        &["--getblockall"],
        quick,
    )
    .is_some_and(|t| blocks_all(&t));
    let internet_sharing = output("/bin/ps", &["-axco", "comm"], quick)
        .is_some_and(|t| t.lines().any(|l| l.trim() == "InternetSharing"));
    let vpn = output("/usr/sbin/scutil", &["--nc", "list"], quick).and_then(|t| connected_vpn(&t));
    let mac_eligible = output(
        "/usr/bin/plutil",
        &[
            "-extract",
            "OS_ELIGIBILITY_DOMAIN_IRON.os_eligibility_answer_t",
            "raw",
            "/private/var/db/os_eligibility/eligibility.plist",
        ],
        quick,
    )
    .and_then(|t| eligible(&t));
    let user = std::env::var("USER").unwrap_or_default();
    let mac_restricted = [
        "/Library/Managed Preferences/com.apple.applicationaccess.plist".to_string(),
        format!("/Library/Managed Preferences/{user}/com.apple.applicationaccess.plist"),
    ]
    .iter()
    .filter(|p| std::path::Path::new(p.as_str()).exists())
    .any(|p| {
        output(
            "/usr/bin/plutil",
            &["-extract", "allowiPhoneMirroring", "raw", p],
            quick,
        )
        .is_some_and(|t| disallowed(&t))
    });
    let mac_handoff = handoff_on(
        &["ActivityAdvertisingAllowed", "ActivityReceivingAllowed"].map(|key| {
            output(
                "/usr/bin/defaults",
                &[
                    "-currentHost",
                    "read",
                    "com.apple.coreservices.useractivityd",
                    key,
                ],
                quick,
            )
        }),
    );
    let checks = Checks {
        mac_eligible,
        mac_restricted,
        mac_handoff,
        wifi_on,
        wifi_joined,
        bluetooth_on,
        firewall_blocks_all,
        internet_sharing,
        vpn,
        phone_on_wifi: phone_seen(udid),
        phone_locked,
        attempt: mirroring_log(5).and_then(|t| Attempt::from_log(&t)),
    };
    Diagnosis {
        blocker: blocker(&checks),
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything fine on the Mac, the phone locked and on the network.
    fn fine() -> Checks {
        Checks {
            wifi_on: Some(true),
            wifi_joined: Some(true),
            bluetooth_on: Some(true),
            phone_on_wifi: Some(true),
            phone_locked: Some(true),
            ..Checks::default()
        }
    }

    // Seen on macOS 27.2, 2026-11-01: the phone locked and beside the Mac, Connect pressed.
    const NOT_FOUND: &str = "\
2026-11-01 07:38:46.507 Df  iPhone Mirroring[90477:9178f0] [com.apple.screensharing:ScreenContinuityApp] 18.1 MirroringSession activate is in flight
2026-11-01 07:38:56.684 E  iPhone Mirroring[90477:a17c37] [com.apple.MediaContinuityKit:Connection] Failed to find endpoint for serviceName: <private> deviceID: <private>
2026-11-01 07:38:56.684 E  iPhone Mirroring[90477:9178f0] [com.apple.screensharing:ScreenContinuityApp] Tearing down the session due to: bringupTimedOut(during: ScreenContinuityUI.BringupStep.sessionInitializing, isDebugging: false)
2026-11-01 07:38:56.700 E  iPhone Mirroring[90477:9178f0] [com.apple.screensharing:ScreenContinuityApp] MirroringSession was interrupted by: timedOut";

    #[test]
    fn the_log_says_how_the_last_connect_ended() {
        assert_eq!(Attempt::from_log(NOT_FOUND), Some(Attempt::NotFound));
        assert_eq!(Attempt::from_log(""), None);
        // Found, then dropped (no endpoint failure in that attempt).
        let dropped =
            "x activate is in flight\ny MirroringSession was interrupted by: connectionLost";
        assert_eq!(Attempt::from_log(dropped), Some(Attempt::Dropped));
        // The last finished one counts; a new one still going doesn't hide it.
        let both = format!("{dropped}\n{NOT_FOUND}\nz activate is in flight");
        assert_eq!(Attempt::from_log(&both), Some(Attempt::NotFound));
        let later = format!("{NOT_FOUND}\n{dropped}");
        assert_eq!(Attempt::from_log(&later), Some(Attempt::Dropped));
    }

    #[test]
    fn the_case_seen_live_is_the_phone_not_offering_mirroring() {
        // The Mac fine, Tailscale on (turning it off didn't change a thing), the phone locked,
        // on the same Wi-Fi, and iPhone Mirroring never finding it.
        let mut c = fine();
        c.vpn = Some("Tailscale 2".into());
        c.attempt = Attempt::from_log(NOT_FOUND);
        let b = blocker(&c);
        assert_eq!(b, Blocker::PhoneNotOffering);
        assert!(
            b.action()
                .starts_with("The iPhone is nearby but isn't offering")
        );
        // What to change is on the iPhone, and the action line says where.
        assert!(
            b.action()
                .contains("on the iPhone, in Settings › General › AirPlay & Continuity")
        );
        assert!(b.action().contains("turn Handoff on"));
        assert!(b.action().contains("listed under iPhone Mirroring"));
        assert!(b.action().contains("restart the iPhone"));
        assert!(b.advice().contains("Revoke iPhone Access"));
        // The same on 2026-11-02 with the Mac's own checks all passing.
        c.mac_eligible = Some(true);
        c.mac_handoff = Some(true);
        assert_eq!(blocker(&c), Blocker::PhoneNotOffering);
    }

    // The Mirroring app's tear-downs for the other two ends (from Apple's forums and the
    // eligibility write-ups; not seen on this Mac).
    const NOT_SET_UP: &str = "\
2026-09-01 09:00:00.000 E  iPhone Mirroring[1:2] [com.apple.screensharing:ScreenContinuityApp] Tearing down the session due to: noCompatiblePhone";
    const REGION: &str = "\
2026-09-01 09:00:00.000 Df iPhone Mirroring[1:2] [com.apple.screensharing:ScreenContinuityApp] 18.1 MirroringSession activate is in flight
2026-09-01 09:00:01.000 E  iPhone Mirroring[1:2] [com.apple.sharing:Authentications] Error Domain=com.apple.sharing.authentication Code=35 \"SFAuthenticationErrorCodeRegionLocked\"
2026-09-01 09:00:01.100 E  iPhone Mirroring[1:2] [com.apple.screensharing:ScreenContinuityApp] Tearing down the session due to: genericError";

    #[test]
    fn set_up_and_region_failures_are_read_from_the_log() {
        assert_eq!(Attempt::from_log(NOT_SET_UP), Some(Attempt::NotSetUp));
        assert_eq!(Attempt::from_log(REGION), Some(Attempt::RegionLocked));
        // The latest Connect counts: a later endpoint failure replaces an old set-up one.
        let both = format!("{NOT_SET_UP}\n{NOT_FOUND}");
        assert_eq!(Attempt::from_log(&both), Some(Attempt::NotFound));
        let mut c = fine();
        c.attempt = Attempt::from_log(NOT_SET_UP);
        let b = blocker(&c);
        assert_eq!(b, Blocker::NotSetUp);
        assert!(b.action().starts_with("Set iPhone Mirroring up again"));
        assert!(b.advice().contains("Revoke iPhone Access"));
        c.attempt = Attempt::from_log(REGION);
        let b = blocker(&c);
        assert_eq!(b, Blocker::PhoneRegionLocked);
        assert!(b.action().contains("country or region"));
        assert!(
            b.advice()
                .starts_with("Tell the user there's nothing to fix")
        );
        // An unlocked phone still comes first: it's the one the user can change now.
        c.phone_locked = Some(false);
        assert_eq!(blocker(&c), Blocker::PhoneUnlocked);
    }

    #[test]
    fn the_macs_region_profile_and_handoff_come_first() {
        let with = |f: fn(&mut Checks)| {
            let mut c = fine();
            c.attempt = Some(Attempt::NotFound);
            f(&mut c);
            blocker(&c)
        };
        assert_eq!(
            with(|c| c.mac_eligible = Some(false)),
            Blocker::MacNotEligible
        );
        assert_eq!(with(|c| c.mac_restricted = true), Blocker::MacRestricted);
        assert_eq!(
            with(|c| c.mac_handoff = Some(false)),
            Blocker::MacHandoffOff
        );
        // Not offered where the Mac is outranks a switch that's off: no switch fixes it.
        let mut c = fine();
        c.mac_eligible = Some(false);
        c.wifi_on = Some(false);
        assert_eq!(blocker(&c), Blocker::MacNotEligible);
        // Bluetooth off is named before Handoff off; both are on the Mac.
        let mut c = fine();
        c.bluetooth_on = Some(false);
        c.mac_handoff = Some(false);
        assert_eq!(blocker(&c), Blocker::MacBluetoothOff);
        assert!(
            Blocker::MacHandoffOff
                .action()
                .starts_with("Turn Handoff on")
        );
        assert!(
            Blocker::MacRestricted
                .action()
                .contains("management profile")
        );
        assert!(Blocker::MacNotEligible.action().contains("EU"));
        assert!(Blocker::MacNotEligible.advice().contains("Trek Driver"));
    }

    #[test]
    fn the_macs_eligibility_profile_and_handoff_are_read() {
        assert_eq!(eligible("4\n"), Some(true));
        assert_eq!(eligible("2"), Some(false));
        assert_eq!(eligible(""), None);
        assert_eq!(eligible("Could not extract value"), None);
        assert!(disallowed("false\n"));
        assert!(!disallowed("true"));
        assert!(!disallowed(""));
        // Keys that aren't there: the default, on.
        assert_eq!(
            handoff_on(&[Some(String::new()), Some(String::new())]),
            Some(true)
        );
        assert_eq!(
            handoff_on(&[Some("1\n".into()), Some("0\n".into())]),
            Some(false)
        );
        assert_eq!(handoff_on(&[None, None]), None);
    }

    #[test]
    fn the_report_names_the_new_checks() {
        let mut c = fine();
        c.mac_eligible = Some(true);
        c.mac_handoff = Some(false);
        let d = Diagnosis {
            blocker: blocker(&c),
            checks: c,
        };
        let report = d.report();
        assert!(report.contains("iPhone Mirroring offered where the Mac is: yes"));
        assert!(report.contains("Turned off by a management profile: no"));
        assert!(report.contains("Mac Handoff: off"));
        assert!(report.ends_with(&Blocker::MacHandoffOff.action()));
    }

    #[test]
    fn mac_side_blockers_come_first_in_order() {
        let with = |f: fn(&mut Checks)| {
            let mut c = fine();
            c.attempt = Some(Attempt::NotFound);
            f(&mut c);
            blocker(&c)
        };
        assert_eq!(with(|c| c.wifi_on = Some(false)), Blocker::MacWifiOff);
        assert_eq!(
            with(|c| c.bluetooth_on = Some(false)),
            Blocker::MacBluetoothOff
        );
        assert_eq!(
            with(|c| c.internet_sharing = true),
            Blocker::InternetSharing
        );
        assert_eq!(
            with(|c| c.wifi_joined = Some(false)),
            Blocker::MacWifiNotJoined
        );
        assert_eq!(
            with(|c| c.phone_locked = Some(false)),
            Blocker::PhoneUnlocked
        );
        assert_eq!(
            with(|c| c.phone_on_wifi = Some(false)),
            Blocker::PhoneOffWifi
        );
        assert_eq!(
            with(|c| c.firewall_blocks_all = true),
            Blocker::FirewallBlocksAll
        );
        // Wi-Fi off outranks everything else that's wrong at once.
        let mut c = fine();
        c.wifi_on = Some(false);
        c.bluetooth_on = Some(false);
        c.internet_sharing = true;
        assert_eq!(blocker(&c), Blocker::MacWifiOff);
        // Not knowing isn't a blocker.
        let mut c = Checks::default();
        c.attempt = Some(Attempt::NotFound);
        assert_eq!(blocker(&c), Blocker::PhoneNotOffering);
    }

    #[test]
    fn a_dropped_connection_names_the_vpn() {
        let mut c = fine();
        c.attempt = Some(Attempt::Dropped);
        c.vpn = Some("Tailscale".into());
        let b = blocker(&c);
        assert_eq!(b, Blocker::ConnectionDrops(Some("Tailscale".into())));
        assert!(b.action().contains("turn Tailscale off"));
        c.vpn = None;
        assert_eq!(blocker(&c), Blocker::ConnectionDrops(None));
        c.attempt = None;
        assert_eq!(blocker(&c), Blocker::Unknown);
    }

    #[test]
    fn the_macs_answers_are_read() {
        let ports = "Hardware Port: Ethernet Adapter (en2)\nDevice: en2\nEthernet Address: 5a:86\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb\n";
        assert_eq!(wifi_device(ports).as_deref(), Some("en0"));
        assert_eq!(wifi_device("Hardware Port: Ethernet\nDevice: en1"), None);
        assert_eq!(power("Wi-Fi Power (en0): On\n"), Some(true));
        assert_eq!(power("Wi-Fi Power (en0): Off"), Some(false));
        assert_eq!(power("garbage"), None);
        assert!(!blocks_all("Firewall has block all state set to disabled."));
        assert!(blocks_all("Firewall has block all state set to enabled."));
        let nc = "Available network connection services in the current set (*=enabled):\n* (Connected)      3906-E9F2 VPN (io.tailscale.ipn.macsys) \"Tailscale 2\"   [VPN:io.tailscale.ipn.macsys]\n* (Disconnected)   1234 VPN (com.other) \"Work\"  [VPN:com.other]";
        assert_eq!(connected_vpn(nc).as_deref(), Some("Tailscale 2"));
        assert_eq!(
            connected_vpn(&nc.replace("(Connected)", "(Disconnected)")),
            None
        );
        let bt = "{\n  \"SPBluetoothDataType\" : [\n    {\n      \"controller_properties\" : {\n        \"controller_state\" : \"attrib_on\",\n        \"controller_discoverable\" : \"attrib_off\",\n";
        assert_eq!(bluetooth_state(bt), Some(true));
        assert_eq!(
            bluetooth_state(&bt.replace("\"attrib_on\"", "\"attrib_off\"")),
            Some(false)
        );
        assert_eq!(bluetooth_state("{}"), None);
    }

    #[test]
    fn the_report_ends_with_the_one_blocker() {
        let mut c = fine();
        c.attempt = Some(Attempt::NotFound);
        let d = Diagnosis {
            blocker: blocker(&c),
            checks: c,
        };
        let report = d.report();
        assert!(report.contains("Last Connect: never found the iPhone"));
        assert!(
            report
                .lines()
                .last()
                .unwrap()
                .starts_with("Most likely: The iPhone is nearby")
        );
    }
}
