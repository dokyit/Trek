//! The iPhone tool on a PC. Windows reaches the phone's own services through Apple Mobile Device
//! Service, as Apple Devices does, so its apps and files work there (`device`); its screen and
//! taps don't yet: they come from the Mac's capture, its Bluetooth keyboard and mouse, iPhone
//! Mirroring, or Trek Driver, which Xcode starts. What the tool shows in each state, and what
//! `phone_status` says, are decided here from what usbmuxd shows (`device::probe`): the same
//! words in both places, as `route` has them for the Mac.
//!
//! Nothing here pairs with the phone: one that hasn't trusted this PC asks when the user taps
//! Trust on it (Apple Devices puts the question to it when it's open with the phone plugged in).

use crate::device::{Phone, Seen};

/// Apple Devices in the Microsoft Store: it brings Apple Mobile Device Service.
pub const STORE_URL: &str = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";

/// The service's name in Windows' Services.
pub const SERVICE: &str = "Apple Mobile Device Service";

/// What the picture area says on a PC, where the Mac shows the screen live.
pub const SCREEN: &str = "Seeing the iPhone's screen and tapping it need Trek on a Mac for now. Here, Trek reaches its apps and files over the cable.";

/// Settings › Tools › iPhone on a PC: what the switch gives agents, and what the tool shows when
/// the agent of the thread in view starts using the phone.
pub const SETTINGS_TOOLS: &str = "Agents use your real iPhone on the cable: its apps (installing one signed for it) and its files, through the Apple Mobile Device Service that Apple Devices installs. Seeing its screen and tapping it need Trek on a Mac for now. One agent at a time; installing and changing files follow the thread's access level. Applies to new sessions.";
pub const SETTINGS_FOLLOW_LABEL: &str = "Show the iPhone tool";
pub const SETTINGS_FOLLOW: &str = "When the agent of the thread you're on starts using the iPhone, the iPhone tool opens beside the chat with what it's doing and a Stop button to take the phone back.";

/// Where Windows' service manager has Apple Mobile Device Service when usbmuxd doesn't answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Not installed: Apple Devices brings it (as iTunes did).
    Missing,
    /// Installed, but not running.
    Stopped,
    /// Couldn't tell (or this isn't Windows).
    Unknown,
}

/// Ask Windows' service manager about Apple Mobile Device Service. Only looks.
#[cfg(windows)]
pub fn service() -> Service {
    use windows_sys::Win32::Foundation::{ERROR_SERVICE_DOES_NOT_EXIST, GetLastError};
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_PAUSED, SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOP_PENDING,
        SERVICE_STOPPED,
    };
    let name: Vec<u16> = SERVICE.encode_utf16().chain(Some(0)).collect();
    // SAFETY: plain service-manager calls. The name is NUL-terminated and outlives them, the
    // status is written into a local, and each handle opened is closed before returning (the
    // last error is read before anything else runs).
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Service::Unknown;
        }
        let handle = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        let found = if handle.is_null() {
            if GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST {
                Service::Missing
            } else {
                Service::Unknown
            }
        } else {
            let mut status: SERVICE_STATUS = std::mem::zeroed();
            let read = QueryServiceStatus(handle, &mut status) != 0;
            CloseServiceHandle(handle);
            match status.dwCurrentState {
                SERVICE_STOPPED | SERVICE_STOP_PENDING | SERVICE_PAUSED if read => Service::Stopped,
                _ => Service::Unknown,
            }
        };
        CloseServiceHandle(manager);
        found
    }
}

/// Ask Windows' service manager about Apple Mobile Device Service: not on this platform.
#[cfg(not(windows))]
pub fn service() -> Service {
    Service::Unknown
}

/// What a call to the phone's services says when usbmuxd doesn't answer on a PC: the one thing
/// to do.
pub fn no_service(service: Service) -> &'static str {
    match service {
        Service::Stopped => {
            "Start Apple Mobile Device Service (in Services, or restart the PC): Trek reaches the iPhone through it."
        }
        Service::Missing | Service::Unknown => {
            "Install Apple Devices from the Microsoft Store: Trek reaches the iPhone through the Apple Mobile Device Service it brings."
        }
    }
}

/// What the tool shows while the phone can't be used yet: a headline, the line under it, and
/// whether to offer Apple Devices in the Microsoft Store; and what an agent should do.
#[derive(Debug, Clone, PartialEq)]
pub struct Say {
    pub title: &'static str,
    pub detail: String,
    pub store: bool,
    /// For `phone_status` and the tools that need the phone: the action first.
    pub advice: String,
}

/// What the tool says for `seen`: `None` once the phone is ready.
pub fn say(seen: &Seen) -> Option<Say> {
    let say = |title, detail: &str, advice: &str| Say {
        title,
        detail: detail.to_string(),
        store: false,
        advice: advice.to_string(),
    };
    Some(match seen {
        Seen::Ready(_) => return None,
        Seen::NoService(Service::Stopped) => say(
            "Start Apple Mobile Device Service",
            "It's installed but not running. Start it in Services (Win+R, services.msc), or restart the PC.",
            "Apple Mobile Device Service is stopped on this PC: ask the user to start it in Services, or to restart the PC.",
        ),
        Seen::NoService(_) => Say {
            store: true,
            ..say(
                "Install Apple Devices from the Microsoft Store",
                "Trek reaches an iPhone on this PC through the Apple Mobile Device Service that comes with it. Then plug the iPhone in.",
                "Apple Mobile Device Service isn't on this PC: ask the user to install Apple Devices from the Microsoft Store, then plug the iPhone in.",
            )
        },
        Seen::NoPhone => say(
            "Plug your iPhone in with a cable and unlock it.",
            "Use a cable that carries data, not one that only charges. The iPhone shows here a moment after.",
            "No iPhone is connected: ask the user to plug it in with a cable and unlock it.",
        ),
        Seen::Untrusted(_) => say(
            "Tap Trust on the iPhone.",
            "Unlock it to see the question, and enter its passcode if it asks. No question? Open Apple Devices with the iPhone plugged in: it asks the iPhone to trust this PC.",
            "The iPhone hasn't trusted this PC yet: ask the user to unlock it and tap Trust (if it doesn't ask, opening Apple Devices with the iPhone plugged in makes it ask).",
        ),
        Seen::Locked(_) => say(
            "Unlock the iPhone.",
            "It hasn't been unlocked since it restarted: Trek reaches its apps and files once it has.",
            "The iPhone hasn't been unlocked since it restarted: ask the user to unlock it.",
        ),
        Seen::Failed(why) => say("Trek can't reach the iPhone", why, why),
    })
}

/// The phone, as the tool's header and `phone_status` name it: its name ("iPhone" when it
/// didn't say), then iOS, the link and its model, as far as it said them.
pub fn describe(phone: &Phone) -> (String, String) {
    let name = if phone.name.trim().is_empty() {
        "iPhone".to_string()
    } else {
        phone.name.trim().to_string()
    };
    let mut detail = vec![];
    if !phone.ios.is_empty() {
        detail.push(format!("iOS {}", phone.ios));
    }
    detail.push(if phone.usb { "USB" } else { "Wi-Fi" }.to_string());
    if let Some(model) = crate::device::model_name(&phone.model)
        .or(Some(phone.model.as_str()).filter(|m| !m.is_empty()))
    {
        detail.push(model.to_string());
    }
    (name, detail.join(" · "))
}

/// The phone tools a PC answers (the others need the Mac for now), as `phone_status` lists them.
const SERVED: &str = "phone_apps, phone_install, phone_files, phone_push_file, phone_pull_file and phone_remove_file";

/// `phone_status` on a PC: the phone and what works here, AssistiveTouch, and who holds it; or,
/// before the phone can be used, what to do first.
pub fn status(seen: &Seen, assistive: Option<bool>, holder: Option<&str>) -> String {
    let held = format!("Held by: {}.", holder.unwrap_or("nobody"));
    let Seen::Ready(phone) = seen else {
        let say = say(seen).map(|s| s.advice).unwrap_or_default();
        return format!("No iPhone to use yet.\nTo do: {say}\n{held}");
    };
    let (name, detail) = describe(phone);
    let link = if phone.usb { "the cable" } else { "Wi-Fi" };
    [
        format!("{name} · {detail}."),
        format!("Apps and files: ready over {link} ({SERVED})."),
        "Screen and taps: not on Windows yet, they need Trek on a Mac for now (so no phone_screenshot, phone_elements or phone_tap here).".to_string(),
        format!(
            "AssistiveTouch: {}.",
            match assistive {
                Some(true) => "on",
                Some(false) => "off",
                None => "unknown",
            }
        ),
        held,
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Phone {
        Phone {
            udid: "00008140-0000".into(),
            name: "Toby's iPhone".into(),
            ios: "27.2".into(),
            model: "iPhone17,2".into(),
            usb: true,
        }
    }

    /// Every state but the ready one, each with what the tool says.
    fn states() -> Vec<Seen> {
        vec![
            Seen::NoService(Service::Missing),
            Seen::NoService(Service::Stopped),
            Seen::NoService(Service::Unknown),
            Seen::NoPhone,
            Seen::Untrusted(phone()),
            Seen::Locked(phone()),
            Seen::Failed("Couldn't open lockdown: device socket io failed".into()),
        ]
    }

    #[test]
    fn each_state_says_the_one_thing_to_do() {
        let title = |s: &Seen| say(s).unwrap().title;
        assert_eq!(
            title(&Seen::NoService(Service::Missing)),
            "Install Apple Devices from the Microsoft Store"
        );
        // Not knowing is not installed, as far as the user can tell.
        assert_eq!(
            title(&Seen::NoService(Service::Unknown)),
            "Install Apple Devices from the Microsoft Store"
        );
        assert_eq!(
            title(&Seen::NoService(Service::Stopped)),
            "Start Apple Mobile Device Service"
        );
        assert_eq!(
            title(&Seen::NoPhone),
            "Plug your iPhone in with a cable and unlock it."
        );
        assert_eq!(title(&Seen::Untrusted(phone())), "Tap Trust on the iPhone.");
        assert_eq!(title(&Seen::Locked(phone())), "Unlock the iPhone.");
        assert!(say(&Seen::Ready(phone())).is_none());
        // Only the missing service offers the Store.
        for s in states() {
            let offers = say(&s).unwrap().store;
            assert_eq!(
                offers,
                matches!(s, Seen::NoService(Service::Missing | Service::Unknown)),
                "{s:?}"
            );
        }
        assert!(STORE_URL.starts_with("ms-windows-store://pdp/?productid=9NP83LWLPZ9K"));
    }

    /// The PC's words name nothing it hasn't got: no Bluetooth keyboard, iPhone Mirroring, Mac
    /// settings or Finder, and no "this Mac". (That the screen and taps need a Mac is said.)
    #[test]
    fn a_pc_is_never_told_of_the_macs_routes() {
        let mut said = vec![
            SCREEN.to_string(),
            SETTINGS_TOOLS.to_string(),
            SETTINGS_FOLLOW_LABEL.to_string(),
            SETTINGS_FOLLOW.to_string(),
            no_service(Service::Missing).to_string(),
            no_service(Service::Stopped).to_string(),
            status(&Seen::Ready(phone()), Some(false), Some("Fix login")),
        ];
        for s in states() {
            let say = say(&s).unwrap();
            said.extend([say.title.to_string(), say.detail, say.advice]);
            said.push(status(&s, None, None));
        }
        for text in &said {
            for word in [
                "Bluetooth",
                "Mirroring",
                "this Mac",
                "the Mac's",
                "Finder",
                "System Settings",
                "Xcode",
                "Lock the iPhone",
            ] {
                assert!(!text.contains(word), "{word}: {text}");
            }
        }
        assert!(SCREEN.contains("need Trek on a Mac for now"));
    }

    #[test]
    fn status_says_what_works_here_and_who_holds_it() {
        let s = status(&Seen::Ready(phone()), Some(true), Some("Fix login"));
        assert!(
            s.starts_with("Toby's iPhone · iOS 27.2 · USB · iPhone 16 Pro Max."),
            "{s}"
        );
        assert!(
            s.contains("Apps and files: ready over the cable (phone_apps,"),
            "{s}"
        );
        assert!(s.contains("need Trek on a Mac for now"), "{s}");
        assert!(s.contains("AssistiveTouch: on."), "{s}");
        assert!(s.ends_with("Held by: Fix login."), "{s}");
        // Before the phone can be used: the one thing to do, for the agent to pass on.
        let s = status(&Seen::NoPhone, None, None);
        assert!(
            s.contains("To do: No iPhone is connected: ask the user"),
            "{s}"
        );
        assert!(s.ends_with("Held by: nobody."), "{s}");
        let s = status(&Seen::NoService(Service::Missing), None, None);
        assert!(
            s.contains("install Apple Devices from the Microsoft Store"),
            "{s}"
        );
    }

    #[test]
    fn the_header_names_what_the_phone_said() {
        assert_eq!(
            describe(&phone()),
            (
                "Toby's iPhone".to_string(),
                "iOS 27.2 · USB · iPhone 16 Pro Max".to_string()
            )
        );
        // A phone that hasn't trusted this PC may say less; a model the table doesn't know is
        // named as it is.
        let quiet = Phone {
            name: String::new(),
            ios: String::new(),
            model: "iPhone18,1".into(),
            usb: false,
            ..phone()
        };
        assert_eq!(
            describe(&quiet),
            ("iPhone".to_string(), "Wi-Fi · iPhone18,1".to_string())
        );
    }

    /// On Windows the service manager answers for the real service (installed or not, it's one
    /// of the three): asking never fails.
    #[test]
    fn the_service_manager_is_asked_without_failing() {
        let s = service();
        if !cfg!(windows) {
            assert_eq!(s, Service::Unknown);
        }
    }
}
