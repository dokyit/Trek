//! What Windows' Settings and its policies say about dictation, as the registry holds them. They
//! are hints: the recognizer has the last word, and an error from it gets the same sentence. But
//! a switch that's plainly off is said before the mic opens, and `mic_access`/`speech_access`
//! answer from them without asking Windows anything. Trek only ever reads these.
//!
//! Deciding from them is plain code every platform's tests run; reading them is Windows-only.

use super::logic::Problem;
use crate::Access;

/// Windows' speech recognizer, registered as a WinRT class (HKLM).
pub const RECOGNIZER: &str = r"SOFTWARE\Microsoft\WindowsRuntime\ActivatableClassId\Windows.Media.SpeechRecognition.SpeechRecognizer";
/// Settings › Privacy & security › Speech › Online speech recognition (HKCU, `HasAccepted`).
pub const ONLINE_SPEECH: &str = r"Software\Microsoft\Speech_OneCore\Settings\OnlineSpeechPrivacy";
/// "Allow users to enable online speech recognition services" (HKLM, `AllowInputPersonalization`).
pub const SPEECH_POLICY: &str = r"SOFTWARE\Policies\Microsoft\InputPersonalization";
/// Settings › Privacy & security › Microphone › Microphone access, for the whole PC (HKLM, `Value`).
pub const MICROPHONE: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
/// … › Let desktop apps access your microphone, which is Trek's (HKCU, `Value`).
pub const DESKTOP_MICROPHONE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged";

/// The switches as read. `None` is a value that isn't there: a switch never touched, or a
/// policy no one set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// Windows' speech recognizer is registered (`RECOGNIZER` exists).
    pub recognizer: bool,
    /// Online speech recognition: 1 on, 0 off. Unset until it's been chosen, which doesn't say
    /// it's off: the recognizer decides then (with it unset, Windows has been seen to compile the
    /// dictation constraint). Off, Trek doesn't dictate even if Windows would let it: the switch
    /// is for exactly what dictation sends.
    pub online_speech: Option<u32>,
    /// The policy: 0 forbids online speech recognition.
    pub speech_policy: Option<u32>,
    /// Microphone access for the whole PC: "Allow" or "Deny".
    pub microphone: Option<String>,
    /// Microphone access for desktop apps: "Allow" or "Deny".
    pub desktop_microphone: Option<String>,
}

fn denied(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("Deny"))
}

impl Hints {
    /// Every switch as it stands now.
    #[cfg(windows)]
    pub fn read() -> Hints {
        use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};
        let number = |root: &Key, key: &str, name: &str| root.open(key).ok()?.get_u32(name).ok();
        let text = |root: &Key, key: &str, name: &str| root.open(key).ok()?.get_string(name).ok();
        Hints {
            recognizer: recognizer_registered(),
            online_speech: number(CURRENT_USER, ONLINE_SPEECH, "HasAccepted"),
            speech_policy: number(LOCAL_MACHINE, SPEECH_POLICY, "AllowInputPersonalization"),
            microphone: text(LOCAL_MACHINE, MICROPHONE, "Value"),
            desktop_microphone: text(CURRENT_USER, DESKTOP_MICROPHONE, "Value"),
        }
    }

    /// What stops dictation before the mic opens, if something plainly does. A switch that
    /// isn't set stops nothing.
    pub fn precheck(&self) -> Result<(), Problem> {
        if !self.recognizer {
            return Err(Problem::Unavailable);
        }
        if self.speech_policy == Some(0) {
            return Err(Problem::OnlineSpeechManaged);
        }
        if self.online_speech == Some(0) {
            return Err(Problem::OnlineSpeechOff);
        }
        if denied(&self.microphone) || denied(&self.desktop_microphone) {
            return Err(Problem::MicrophoneOff);
        }
        Ok(())
    }

    /// What stops dictation on this PC before the mic opens: the microphone's switches only. It
    /// needs neither Windows' recognizer nor online speech recognition.
    pub fn microphone(&self) -> Result<(), Problem> {
        if denied(&self.microphone) || denied(&self.desktop_microphone) {
            Err(Problem::MicrophoneOff)
        } else {
            Ok(())
        }
    }

    /// The microphone switches, as onboarding would show them: Windows asks desktop apps
    /// nothing, so it's allowed unless one is off.
    pub fn mic_access(&self) -> Access {
        if denied(&self.microphone) || denied(&self.desktop_microphone) {
            Access::Denied
        } else {
            Access::Granted
        }
    }

    /// Online speech recognition: on, off (or forbidden), or never chosen.
    pub fn speech_access(&self) -> Access {
        if !self.recognizer {
            return Access::Unavailable;
        }
        match (self.speech_policy, self.online_speech) {
            (Some(0), _) | (_, Some(0)) => Access::Denied,
            (_, Some(_)) => Access::Granted,
            (_, None) => Access::NotAsked,
        }
    }

    /// Each switch and what it reads, for the probe (`examples/winrt_probe.rs`).
    pub fn described(&self) -> Vec<(&'static str, String)> {
        let number = |v: Option<u32>| v.map_or_else(|| "unset".to_string(), |v| v.to_string());
        let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "unset".to_string());
        vec![
            ("speech recognizer registered", self.recognizer.to_string()),
            (
                "online speech recognition (HasAccepted)",
                number(self.online_speech),
            ),
            (
                "online speech policy (AllowInputPersonalization)",
                number(self.speech_policy),
            ),
            ("microphone access, this PC", text(&self.microphone)),
            (
                "microphone access, desktop apps",
                text(&self.desktop_microphone),
            ),
        ]
    }
}

/// Whether Windows' speech recognizer is registered: cheap enough to ask once per launch for
/// the mic button.
#[cfg(windows)]
pub fn recognizer_registered() -> bool {
    windows_registry::LOCAL_MACHINE.open(RECOGNIZER).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_up() -> Hints {
        Hints {
            recognizer: true,
            online_speech: Some(1),
            speech_policy: None,
            microphone: Some("Allow".into()),
            desktop_microphone: Some("Allow".into()),
        }
    }

    #[test]
    fn a_set_up_pc_is_ready() {
        let h = set_up();
        assert_eq!(h.precheck(), Ok(()));
        assert_eq!(h.mic_access(), Access::Granted);
        assert_eq!(h.speech_access(), Access::Granted);
    }

    #[test]
    fn switches_never_touched_leave_it_to_the_recognizer() {
        let h = Hints {
            recognizer: true,
            ..Hints::default()
        };
        assert_eq!(h.precheck(), Ok(()), "unset isn't off");
        assert_eq!(h.mic_access(), Access::Granted);
        assert_eq!(h.speech_access(), Access::NotAsked);
    }

    #[test]
    fn what_is_off_is_said_before_the_mic_opens() {
        let h = Hints {
            recognizer: false,
            ..set_up()
        };
        assert_eq!(h.precheck(), Err(Problem::Unavailable));
        assert_eq!(h.speech_access(), Access::Unavailable);

        let h = Hints {
            online_speech: Some(0),
            ..set_up()
        };
        assert_eq!(h.precheck(), Err(Problem::OnlineSpeechOff));
        assert_eq!(h.speech_access(), Access::Denied);

        // An organization's policy wins over the switch, and can't be changed in Settings.
        let h = Hints {
            speech_policy: Some(0),
            online_speech: Some(1),
            ..set_up()
        };
        assert_eq!(h.precheck(), Err(Problem::OnlineSpeechManaged));
        assert_eq!(h.speech_access(), Access::Denied);
        let h = Hints {
            speech_policy: Some(1),
            ..set_up()
        };
        assert_eq!(
            h.precheck(),
            Ok(()),
            "a policy that allows it stops nothing"
        );

        for (microphone, desktop) in [("Deny", "Allow"), ("Allow", "Deny"), ("deny", "deny")] {
            let h = Hints {
                microphone: Some(microphone.into()),
                desktop_microphone: Some(desktop.into()),
                ..set_up()
            };
            assert_eq!(
                h.precheck(),
                Err(Problem::MicrophoneOff),
                "{microphone} {desktop}"
            );
            assert_eq!(h.mic_access(), Access::Denied);
        }
    }

    /// Dictation on this PC needs the microphone's switches on, and nothing of Windows' own
    /// recognizer: not the recognizer, not online speech recognition, not its policy.
    #[test]
    fn on_this_pc_only_the_microphone_switches_count() {
        let h = Hints {
            recognizer: false,
            online_speech: Some(0),
            speech_policy: Some(0),
            ..set_up()
        };
        assert_eq!(h.microphone(), Ok(()));
        assert_eq!(Hints::default().microphone(), Ok(()), "unset isn't off");
        for (microphone, desktop) in [("Deny", "Allow"), ("Allow", "Deny")] {
            let h = Hints {
                microphone: Some(microphone.into()),
                desktop_microphone: Some(desktop.into()),
                ..set_up()
            };
            assert_eq!(h.microphone(), Err(Problem::MicrophoneOff));
        }
    }

    #[test]
    fn the_probe_names_every_switch() {
        let said = Hints::default().described();
        assert_eq!(said.len(), 5);
        assert!(said.iter().skip(1).all(|(_, v)| v == "unset"), "{said:?}");
        let said = set_up().described();
        assert_eq!(said[0].1, "true");
        assert_eq!(said[1].1, "1");
        assert_eq!(said[4].1, "Allow");
    }

    /// The real registry, read: whatever this PC has set, reading it never fails, and a PC
    /// with Windows' recognizer says so.
    #[cfg(windows)]
    #[test]
    fn this_pc_can_be_read() {
        let h = Hints::read();
        assert_eq!(h.recognizer, recognizer_registered());
        let _ = (h.precheck(), h.mic_access(), h.speech_access());
    }
}
