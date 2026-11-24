//! The Mac's notification center, asked directly: Trek's settings in it (allowed, a banner
//! style), a banner posted with the center's answer to the request, and whether a Focus is on.
//!
//! GPUI posts banners too, and tells nobody how it went; Trek's log wants the center's answer
//! beside every banner (`trek-app`'s `banner`), so on a Mac Trek hands the center the same
//! request itself. Nothing here knows about threads or settings: `examples/probe.rs` runs the
//! same calls from a bundle of its own, to see what macOS makes of a banner like Trek's without
//! posting as Trek.
//!
//! Windows and Linux have only `focus_from` (plain parsing, tested everywhere): their banners
//! are GPUI's.

/// Whether a Focus is on, from macOS's own record of them (`~/Library/DoNotDisturb/DB`):
/// `Some(name)` when one is, its name if the modes' file has it. `None` when none is on, or the
/// record can't be read as expected.
///
/// `assertions` is `Assertions.json`: `data[0].storeAssertionRecords` lists what's on now, each
/// with its mode's id; `modes` is `ModeConfigurations.json`, which names them.
pub fn focus_from(assertions: &str, modes: Option<&str>) -> Option<Option<String>> {
    let assertions: serde_json::Value = serde_json::from_str(assertions).ok()?;
    let on = assertions
        .get("data")?
        .get(0)?
        .get("storeAssertionRecords")?
        .as_array()?
        .first()?;
    let id = on
        .get("assertionDetails")
        .and_then(|d| d.get("assertionDetailsModeIdentifier"))
        .and_then(|id| id.as_str());
    let name = id.zip(modes).and_then(|(id, modes)| {
        let modes: serde_json::Value = serde_json::from_str(modes).ok()?;
        modes
            .get("data")?
            .get(0)?
            .get("modeConfigurations")?
            .get(id)?
            .get("mode")?
            .get("name")?
            .as_str()
            .map(str::to_string)
    });
    Some(name)
}

#[cfg(target_os = "macos")]
pub use mac::{Answer, Heard, bundled, focus, heard, post, withdraw};

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAlertStyle, UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotificationRequest, UNNotificationSetting, UNNotificationSettings,
        UNUserNotificationCenter,
    };
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// What the center's settings for this app come to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Heard {
        /// Notifications allowed, with a banner (or alert) style.
        Allowed = 1,
        /// The user hasn't been asked yet: the first banner asks.
        NotAsked = 2,
        /// Notifications turned off.
        Denied = 3,
        /// Allowed, with no banner: the style is None, alerts are off, or macOS was only ever
        /// asked provisionally. They go to Notification Center quietly.
        Quiet = 4,
    }

    /// What the center last said: 0 not heard yet, else a `Heard`.
    static HEARD: AtomicU8 = AtomicU8::new(0);

    /// The process is part of a bundle with an identifier. The center aborts one that isn't
    /// (`bundleProxyForCurrentProcess is nil`), so nothing here reaches it then.
    pub fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    /// The center's settings as a line for the log, and what they come to.
    fn read(settings: &UNNotificationSettings) -> (Heard, String) {
        let setting = |s: UNNotificationSetting| match s {
            UNNotificationSetting::Enabled => "on",
            UNNotificationSetting::Disabled => "off",
            _ => "unsupported",
        };
        let status = settings.authorizationStatus();
        let authorization = match status {
            UNAuthorizationStatus::Authorized => "allowed",
            UNAuthorizationStatus::Provisional => "allowed provisionally",
            UNAuthorizationStatus::Ephemeral => "allowed for now",
            UNAuthorizationStatus::Denied => "denied",
            _ => "not asked yet",
        };
        let style = match settings.alertStyle() {
            UNAlertStyle::Banner => "banner",
            UNAlertStyle::Alert => "alert",
            _ => "none",
        };
        let shown = settings.alertSetting() == UNNotificationSetting::Enabled
            && settings.alertStyle() != UNAlertStyle::None;
        let heard = match status {
            UNAuthorizationStatus::Denied => Heard::Denied,
            UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Ephemeral if shown => {
                Heard::Allowed
            }
            UNAuthorizationStatus::Authorized
            | UNAuthorizationStatus::Ephemeral
            | UNAuthorizationStatus::Provisional => Heard::Quiet,
            _ => Heard::NotAsked,
        };
        let said = format!(
            "notifications {authorization}, alerts {}, style {style}, sound {}, notification center {}",
            setting(settings.alertSetting()),
            setting(settings.soundSetting()),
            setting(settings.notificationCenterSetting()),
        );
        (heard, said)
    }

    /// Ask the center for this app's settings; `then` hears them, on a thread of the center's.
    fn ask(then: impl Fn(Heard, String) + 'static) {
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the center hands a valid settings object for the call's length.
            let (heard, said) = read(unsafe { settings.as_ref() });
            HEARD.store(heard as u8, Ordering::Relaxed);
            then(heard, said);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&block);
    }

    /// What the center's settings came to when last heard, and ask again (the answer is in by the
    /// next call): cheap enough for a view to call as it draws. `None`: not heard yet, or no
    /// bundle to ask for.
    pub fn heard() -> Option<Heard> {
        if !bundled() {
            return None;
        }
        ask(|_, _| {});
        match HEARD.load(Ordering::Relaxed) {
            1 => Some(Heard::Allowed),
            2 => Some(Heard::NotAsked),
            3 => Some(Heard::Denied),
            4 => Some(Heard::Quiet),
            _ => None,
        }
    }

    /// The Focus that's on (`focus_from`), read at most every two seconds. macOS keeps the record
    /// behind Full Disk Access: without it nothing is read, and nothing is asked for.
    pub fn focus() -> Option<Option<String>> {
        static LAST: Mutex<Option<(Instant, Option<Option<String>>)>> = Mutex::new(None);
        let mut last = LAST.lock().ok()?;
        if let Some((at, focus)) = last.as_ref()
            && at.elapsed() < Duration::from_secs(2)
        {
            return focus.clone();
        }
        let db = trek_core::paths::home().join("Library/DoNotDisturb/DB");
        let focus = std::fs::read_to_string(db.join("Assertions.json"))
            .ok()
            .and_then(|assertions| {
                let modes = std::fs::read_to_string(db.join("ModeConfigurations.json")).ok();
                super::focus_from(&assertions, modes.as_deref())
            });
        *last = Some((Instant::now(), focus.clone()));
        focus
    }

    /// What the center said to a banner: `Ok` once it took the request, else its error; and its
    /// settings for this app at the time, as a line for the log. Called on a thread of the
    /// center's.
    pub type Answer = Arc<dyn Fn(Result<(), String>, &str) + Send + Sync>;

    /// Post a banner: `title` and `body`, no sound of its own, delivered at once, with `tag` as
    /// the request's identifier, so a newer banner under a tag replaces the older one (the
    /// request GPUI's own post makes). A center that hasn't asked the user yet asks first, and
    /// takes the banner once they've said yes: added before the answer, it would be refused and
    /// lost. `answer` hears how it went.
    pub fn post(tag: &str, title: &str, body: &str, answer: Answer) {
        if !bundled() {
            return answer(
                Err("this build runs outside an app bundle".into()),
                "no app bundle",
            );
        }
        let (tag, title, body) = (tag.to_string(), title.to_string(), body.to_string());
        ask(move |heard, said| {
            if heard != Heard::NotAsked {
                return add(&tag, &title, &body, &said, answer.clone());
            }
            let (tag, title, body, answer) =
                (tag.clone(), title.clone(), body.clone(), answer.clone());
            let asked = RcBlock::new(move |granted: Bool, error: *mut NSError| {
                // SAFETY: a non-null `error` is a valid `NSError` for the call's length.
                let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                // Whatever the user said, the settings are other than they were.
                HEARD.store(0, Ordering::Relaxed);
                if granted.as_bool() {
                    add(
                        &tag,
                        &title,
                        &body,
                        "notifications allowed just now",
                        answer.clone(),
                    )
                } else {
                    answer(
                        Err(error.unwrap_or_else(|| "notifications weren't allowed".into())),
                        "asked just now",
                    )
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &asked,
                );
        });
    }

    fn add(tag: &str, title: &str, body: &str, said: &str, answer: Answer) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        // No trigger: delivered at once.
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(tag),
            &content,
            None,
        );
        let said = said.to_string();
        let done = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: a non-null `error` is a valid `NSError` for the call's length.
            let outcome = match unsafe { error.as_ref() } {
                Some(e) => Err(e.localizedDescription().to_string()),
                None => Ok(()),
            };
            answer(outcome, &said);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&done));
    }

    /// Take the banner posted under `tag` back: off the screen and out of Notification Center.
    pub fn withdraw(tag: &str) {
        if !bundled() {
            return;
        }
        let tags = NSArray::from_retained_slice(&[NSString::from_str(tag)]);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.removePendingNotificationRequestsWithIdentifiers(&tags);
        center.removeDeliveredNotificationsWithIdentifiers(&tags);
    }
}

#[cfg(test)]
mod tests {
    use super::focus_from;

    /// `Assertions.json` as macOS 27 writes it with Do Not Disturb turned on from Control Center
    /// (ids shortened), and with nothing on: what's on is under `storeAssertionRecords`, what
    /// ended under the invalidation lists.
    const FOCUS_ON: &str = r#"{"data":[{"storeAssertionRecords":[{"assertionUUID":"C3996947","assertionSource":{"assertionClientIdentifier":"com.apple.controlcenter.dnd"},"assertionStartDateTimestamp":816912431.75,"assertionDetails":{"assertionDetailsIdentifier":"com.apple.controlcenter.dnd","assertionDetailsModeIdentifier":"com.apple.donotdisturb.mode.default","assertionDetailsReason":"user-action"}}],"storeInvalidationRecords":[]}],"header":{"version":8,"timestamp":817138324.99}}"#;
    const FOCUS_OFF: &str = r#"{"data":[{"storeInvalidationRequestRecords":[{"invalidationRequestReason":"user-changed-state"}],"storeInvalidationRecords":[{"invalidationAssertion":{"assertionDetails":{"assertionDetailsModeIdentifier":"com.apple.sleep.sleep-mode"}},"invalidationReason":"client-ended"}]}],"header":{"version":8,"timestamp":817138324.99}}"#;
    const MODES: &str = r#"{"data":[{"modeConfigurations":{"com.apple.donotdisturb.mode.default":{"mode":{"name":"Do Not Disturb","modeIdentifier":"com.apple.donotdisturb.mode.default","semanticType":0}},"com.apple.sleep.sleep-mode":{"mode":{"name":"Sleep","modeIdentifier":"com.apple.sleep.sleep-mode"}}}}]}"#;

    #[test]
    fn a_focus_is_read_from_macos_s_own_record_of_them() {
        assert_eq!(
            focus_from(FOCUS_ON, Some(MODES)),
            Some(Some("Do Not Disturb".into()))
        );
        // On, with the modes' file unreadable or without that mode: on, unnamed.
        assert_eq!(focus_from(FOCUS_ON, None), Some(None));
        assert_eq!(focus_from(FOCUS_ON, Some("{}")), Some(None));
        // One that ended is in the record still, as ended: none is on.
        assert_eq!(focus_from(FOCUS_OFF, Some(MODES)), None);
        assert_eq!(
            focus_from(r#"{"data":[{"storeAssertionRecords":[]}]}"#, None),
            None
        );
        // Anything else (another macOS's shape, half a file): not known to be on.
        assert_eq!(focus_from("", None), None);
        assert_eq!(focus_from("[]", None), None);
    }
}
