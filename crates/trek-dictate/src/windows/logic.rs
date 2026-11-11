//! What Trek decides from what Windows' speech recognition says, as plain code every platform's
//! tests run: how a take's phrases make its text (`Phrases`), and which sentence each way of
//! failing gets (`Problem`). The WinRT thread (`super::winrt`) only forwards Windows' events here.

use crate::Heard;

/// The sentences name Windows' own things whatever platform their tests run on.
const WINDOWS: trek_words::Words = trek_words::for_platform(true);

/// `SpeechRecognitionResultStatus`, as Windows numbers it.
pub mod status {
    pub const SUCCESS: i32 = 0;
    pub const TOPIC_LANGUAGE_NOT_SUPPORTED: i32 = 1;
    pub const GRAMMAR_LANGUAGE_MISMATCH: i32 = 2;
    pub const GRAMMAR_COMPILATION_FAILURE: i32 = 3;
    pub const AUDIO_QUALITY_FAILURE: i32 = 4;
    pub const USER_CANCELED: i32 = 5;
    pub const UNKNOWN: i32 = 6;
    pub const TIMEOUT_EXCEEDED: i32 = 7;
    pub const PAUSE_LIMIT_EXCEEDED: i32 = 8;
    pub const NETWORK_FAILURE: i32 = 9;
    pub const MICROPHONE_UNAVAILABLE: i32 = 10;
}

/// One take's words as Windows reports them in a continuous session: the phrase being spoken is
/// a hypothesis, revised as it goes (`HypothesisGenerated`), until it settles as a result
/// (`ResultGenerated`) and the next phrase starts. What the composer hears is always the whole
/// take so far, as the Mac's recognizer reports it.
#[derive(Debug, Default)]
pub struct Phrases {
    /// The phrases Windows settled, in order.
    settled: Vec<String>,
    /// The phrase being spoken: Windows' latest guess at it.
    hearing: String,
    state: State,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Listening,
    /// ✓: the mic is stopping, and Windows settles what it heard last.
    Finishing,
    /// Completed, or called off: nothing more goes to the composer.
    Over,
}

impl Phrases {
    /// The phrase being spoken, revised: the take so far, with it at the end.
    pub fn hypothesis(&mut self, text: &str) -> Option<Heard> {
        if self.state == State::Over {
            return None;
        }
        self.hearing = text.trim().to_string();
        Some(Heard::Partial(self.text()))
    }

    /// A phrase settled. One Windows rejected (too unsure of it, or a result that isn't a
    /// success) is dropped, and its hypothesis with it.
    pub fn result(&mut self, text: &str, rejected: bool) -> Option<Heard> {
        if self.state == State::Over {
            return None;
        }
        self.hearing.clear();
        if !rejected && !text.trim().is_empty() {
            self.settled.push(text.trim().to_string());
        }
        Some(Heard::Partial(self.text()))
    }

    /// ✓ was pressed: the mic stops and the session completes once Windows is done.
    pub fn finish(&mut self) {
        if self.state == State::Listening {
            self.state = State::Finishing;
        }
    }

    pub fn finishing(&self) -> bool {
        self.state == State::Finishing
    }

    /// The session ended with `status`: the take's text (the phrase still being spoken
    /// included: Windows may stop before it settles it), why it failed, or nothing for a take
    /// already over. Silence long enough to end the session, or Windows calling it off itself,
    /// ends the take with what was heard, as the recognizer ending on its own does on a Mac.
    /// `language` names the speech language, for a failure that's about it.
    pub fn completed(&mut self, status: i32, language: &str) -> Option<Heard> {
        if std::mem::replace(&mut self.state, State::Over) == State::Over {
            return None;
        }
        Some(match Problem::ending(status, language) {
            None => Heard::Final(self.text()),
            Some(problem) => Heard::Failed(problem.sentence()),
        })
    }

    /// The take was called off (✕, or the composer went away): whatever Windows says about it
    /// from now on, late as it may be, goes nowhere.
    pub fn cancel(&mut self) {
        self.state = State::Over;
    }

    /// Everything heard so far, settled or not.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for phrase in self.settled.iter().chain(Some(&self.hearing)) {
            if phrase.is_empty() {
                continue;
            }
            let joins = text
                .chars()
                .last()
                .zip(phrase.chars().next())
                .is_some_and(|(left, right)| !unspaced(left) && !unspaced(right));
            if joins {
                text.push(' ');
            }
            text.push_str(phrase);
        }
        text
    }
}

/// Written without spaces between words or sentences (Chinese and Japanese): phrases in it
/// follow one another directly.
fn unspaced(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x30FF // CJK punctuation, hiragana, katakana
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK ideographs
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF00..=0xFFEF // full-width forms
    )
}

/// A peak meter reads about this much above the RMS level the Mac's waveform is drawn from.
const CREST_DB: f32 = 10.;

/// The capture endpoint's peak (0 to 1, linear) as the waveform's 0 to 1, like `loudness_db`
/// of a Mac's RMS reading: a peak is taken down by `CREST_DB` first, so speech draws the same
/// height on both.
pub fn loudness_peak(peak: f32) -> f32 {
    if peak.is_nan() || peak <= 0. {
        return 0.;
    }
    crate::loudness_db(20. * peak.min(1.).log10() - CREST_DB)
}

/// Why dictation couldn't start, or stopped: each way Windows refuses has a sentence that says
/// what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Settings › Privacy & security › Speech › Online speech recognition is off, and Windows
    /// dictates only with it on (`0x80045509`, "the privacy statement was declined").
    OnlineSpeechOff,
    /// An organization turned online speech recognition off for this PC (the
    /// `AllowInputPersonalization` policy).
    OnlineSpeechManaged,
    /// The microphone is off for desktop apps, or for the whole PC, in Settings › Privacy &
    /// security › Microphone (`E_ACCESSDENIED`).
    MicrophoneOff,
    /// No microphone to listen with (`0x80070490`, an unplugged one, or a session that
    /// completed as `MicrophoneUnavailable`).
    NoMicrophone,
    /// Another app has the microphone to itself (`AUDCLNT_E_DEVICE_IN_USE`).
    MicrophoneBusy,
    /// The speech service couldn't be reached (`NetworkFailure`, or WinINet's errors).
    Offline,
    /// Windows can't dictate in its speech language (named, when Windows said).
    Language(String),
    /// Windows' speech recognition isn't on this PC: its class isn't registered, or this process
    /// can't make one.
    Unavailable,
    /// Windows didn't answer in time.
    NoAnswer,
    /// The sound from the microphone was too poor to make out (`AudioQualityFailure`).
    Unclear,
    /// The session ended for a reason Windows didn't give (`Unknown`).
    Stopped,
    /// Anything else, by its code (an HRESULT, or a status that can't be told apart).
    Code(u32),
}

/// Windows' settings pages for the two switches dictation needs on.
pub const SPEECH_PAGE: &str = "ms-settings:privacy-speech";
pub const MICROPHONE_PAGE: &str = "ms-settings:privacy-microphone";

impl Problem {
    /// The problem an HRESULT from a WinRT or COM call stands for.
    pub fn from_code(code: i32) -> Problem {
        match code as u32 {
            // SPERR_PRIVACY_STATEMENT_DECLINED (the speech privacy policy wasn't accepted).
            0x8004_5509 => Problem::OnlineSpeechOff,
            // E_ACCESSDENIED: the microphone's privacy switch.
            0x8007_0005 => Problem::MicrophoneOff,
            // E_NOTFOUND (no default capture endpoint), AUDCLNT_E_DEVICE_INVALIDATED (unplugged).
            0x8007_0490 | 0x8889_0004 => Problem::NoMicrophone,
            // AUDCLNT_E_DEVICE_IN_USE: another app opened it in exclusive mode.
            0x8889_000A => Problem::MicrophoneBusy,
            // WinINet: timed out, name not resolved, can't connect, aborted, reset.
            0x8007_2EE2 | 0x8007_2EE7 | 0x8007_2EFD | 0x8007_2EFE | 0x8007_2EFF => Problem::Offline,
            // REGDB_E_CLASSNOTREG, ERROR_MOD_NOT_FOUND (the class's DLL is gone), and
            // APPMODEL_ERROR_NO_PACKAGE (an API that wants package identity, which Trek hasn't).
            0x8004_0154 | 0x8007_007E | 0x8007_3D54 => Problem::Unavailable,
            code => Problem::Code(code),
        }
    }

    /// The problem a completed session's status stands for, if it's one: silence ending the
    /// session, a pause too long, and a session called off aren't, and keep what was heard.
    pub fn ending(status: i32, language: &str) -> Option<Problem> {
        match status {
            status::SUCCESS
            | status::USER_CANCELED
            | status::TIMEOUT_EXCEEDED
            | status::PAUSE_LIMIT_EXCEEDED => None,
            status => Some(Problem::failed(status, language)),
        }
    }

    /// Compiling the dictation constraint: anything but success is a reason not to start.
    pub fn compiling(status: i32, language: &str) -> Option<Problem> {
        (status != status::SUCCESS).then(|| Problem::failed(status, language))
    }

    fn failed(status: i32, language: &str) -> Problem {
        match status {
            status::TOPIC_LANGUAGE_NOT_SUPPORTED | status::GRAMMAR_LANGUAGE_MISMATCH => {
                Problem::Language(language.trim().to_string())
            }
            status::AUDIO_QUALITY_FAILURE => Problem::Unclear,
            status::NETWORK_FAILURE => Problem::Offline,
            status::MICROPHONE_UNAVAILABLE => Problem::NoMicrophone,
            status::UNKNOWN => Problem::Stopped,
            status => Problem::Code(status as u32),
        }
    }

    /// What the composer says (and, when a take stops on its own, after "Dictation stopped:").
    pub fn sentence(&self) -> String {
        match self {
            Problem::OnlineSpeechOff => "Dictation uses Windows' online speech recognition, which is off. Turn it on in Settings › Privacy & security › Speech.".into(),
            Problem::OnlineSpeechManaged => "Your organization has turned off online speech recognition, which dictation uses.".into(),
            Problem::MicrophoneOff => "Windows isn't letting Trek use the microphone. Turn on microphone access for desktop apps in Settings › Privacy & security › Microphone.".into(),
            Problem::NoMicrophone => "There's no microphone to listen with. Connect one, or choose one in Settings › System › Sound.".into(),
            Problem::MicrophoneBusy => "Another app has the microphone to itself. Close it, then try again.".into(),
            Problem::Offline => "Windows' online speech recognition couldn't be reached. Check the internet connection, then try again.".into(),
            Problem::Language(name) if name.is_empty() => "Windows can't dictate in its speech language. Choose another in Settings › Time & language › Speech.".into(),
            Problem::Language(name) => format!("Windows can't dictate in {name}. Choose another speech language in Settings › Time & language › Speech."),
            Problem::Unavailable => match WINDOWS.dictate_fallback {
                Some(fallback) => format!("Windows speech recognition isn't available on {}. {fallback}.", WINDOWS.this_computer),
                None => format!("Windows speech recognition isn't available on {}.", WINDOWS.this_computer),
            },
            Problem::NoAnswer => "Windows speech recognition didn't answer. Try again in a moment.".into(),
            Problem::Unclear => "The sound from the microphone was too poor for Windows to make out.".into(),
            Problem::Stopped => "Windows speech recognition stopped without saying why.".into(),
            Problem::Code(code) => format!("Windows speech recognition failed (error 0x{code:08X})."),
        }
    }

    /// Nothing the user can do about it in Trek (no recognizer, an organization's policy): the
    /// mic dims for the rest of the session and points to Windows' own dictation.
    pub fn lasting(&self) -> bool {
        matches!(self, Problem::Unavailable | Problem::OnlineSpeechManaged)
    }

    /// The Settings page of a privacy switch that's off, which opens the first time it's in the
    /// way, as macOS shows its prompt once. Other problems only say where to look.
    pub fn switch(&self) -> Option<&'static str> {
        match self {
            Problem::OnlineSpeechOff => Some(SPEECH_PAGE),
            Problem::MicrophoneOff => Some(MICROPHONE_PAGE),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(text: &str) -> Option<Heard> {
        Some(Heard::Partial(text.into()))
    }

    #[test]
    fn hypotheses_revise_the_phrase_being_spoken() {
        let mut p = Phrases::default();
        assert_eq!(p.hypothesis("make"), partial("make"));
        assert_eq!(p.hypothesis("make the"), partial("make the"));
        assert_eq!(
            p.hypothesis(" make the buttons "),
            partial("make the buttons"),
            "each one is the whole phrase so far, trimmed"
        );
        assert_eq!(
            p.hypothesis("make the button is"),
            partial("make the button is")
        );
        assert_eq!(p.text(), "make the button is");
    }

    #[test]
    fn phrases_settle_and_the_next_one_follows() {
        let mut p = Phrases::default();
        p.hypothesis("make the buttons");
        assert_eq!(
            p.result("Make the buttons bigger.", false),
            partial("Make the buttons bigger."),
            "the result replaces its hypothesis"
        );
        assert_eq!(
            p.hypothesis("but keep"),
            partial("Make the buttons bigger. but keep")
        );
        assert_eq!(
            p.result("But keep the padding.", false),
            partial("Make the buttons bigger. But keep the padding.")
        );
        // A phrase Windows rejects goes, with its hypothesis, and what settled stays.
        p.hypothesis("um");
        assert_eq!(
            p.result("", true),
            partial("Make the buttons bigger. But keep the padding.")
        );
        p.hypothesis("noise");
        assert_eq!(
            p.result("Noise.", true),
            partial("Make the buttons bigger. But keep the padding.")
        );
        assert_eq!(
            p.result("   ", false),
            partial("Make the buttons bigger. But keep the padding."),
            "an empty result adds nothing"
        );
    }

    #[test]
    fn finishing_keeps_words_windows_never_settled() {
        let mut p = Phrases::default();
        p.result("Note.", false);
        p.hypothesis("the last few words");
        p.finish();
        assert!(p.finishing());
        // Windows may still revise the phrase while it stops.
        assert_eq!(
            p.hypothesis("the last few words here"),
            partial("Note. the last few words here")
        );
        assert_eq!(
            p.completed(status::SUCCESS, "English"),
            Some(Heard::Final("Note. the last few words here".into())),
            "stopped before the phrase settled: it's kept as heard"
        );
        // And with it settled first, the settled text is the take's.
        let mut p = Phrases::default();
        p.hypothesis("all right");
        p.finish();
        p.result("All right.", false);
        assert_eq!(
            p.completed(status::SUCCESS, "English"),
            Some(Heard::Final("All right.".into()))
        );
    }

    #[test]
    fn a_cancelled_take_ignores_what_windows_says_late() {
        let mut p = Phrases::default();
        p.hypothesis("noise");
        p.cancel();
        assert_eq!(p.hypothesis("noise noise"), None);
        assert_eq!(p.result("Noise.", false), None);
        assert_eq!(p.completed(status::USER_CANCELED, "English"), None);
        assert_eq!(p.completed(status::SUCCESS, "English"), None);
        // A finish after it doesn't bring it back either.
        p.finish();
        assert!(!p.finishing());
        assert_eq!(p.hypothesis("again"), None);
    }

    #[test]
    fn a_completed_take_says_its_text_once() {
        let mut p = Phrases::default();
        p.result("Once.", false);
        assert_eq!(
            p.completed(status::SUCCESS, ""),
            Some(Heard::Final("Once.".into()))
        );
        assert_eq!(p.completed(status::SUCCESS, ""), None);
        assert_eq!(p.hypothesis("more"), None);
        assert_eq!(p.result("More.", false), None);
    }

    #[test]
    fn completion_statuses_end_with_the_text_or_why() {
        let ended = |status| {
            let mut p = Phrases::default();
            p.result("Heard this.", false);
            p.completed(status, "Klingon (Qo'noS)")
        };
        let final_text = Some(Heard::Final("Heard this.".into()));
        // Ending as asked, after silence, a pause, or called off by Windows: what was heard.
        for s in [
            status::SUCCESS,
            status::TIMEOUT_EXCEEDED,
            status::PAUSE_LIMIT_EXCEEDED,
            status::USER_CANCELED,
        ] {
            assert_eq!(ended(s), final_text, "status {s}");
        }
        let failed = |problem: Problem| Some(Heard::Failed(problem.sentence()));
        assert_eq!(ended(status::NETWORK_FAILURE), failed(Problem::Offline));
        assert_eq!(
            ended(status::MICROPHONE_UNAVAILABLE),
            failed(Problem::NoMicrophone)
        );
        assert_eq!(
            ended(status::AUDIO_QUALITY_FAILURE),
            failed(Problem::Unclear)
        );
        assert_eq!(ended(status::UNKNOWN), failed(Problem::Stopped));
        assert_eq!(
            ended(status::TOPIC_LANGUAGE_NOT_SUPPORTED),
            failed(Problem::Language("Klingon (Qo'noS)".into()))
        );
        assert_eq!(
            ended(status::GRAMMAR_COMPILATION_FAILURE),
            failed(Problem::Code(3))
        );
        // Nothing heard, ended as asked: an empty text, which the composer calls "Nothing heard."
        let mut p = Phrases::default();
        assert_eq!(
            p.completed(status::SUCCESS, ""),
            Some(Heard::Final(String::new()))
        );
    }

    #[test]
    fn compiling_fails_on_anything_but_success() {
        assert_eq!(Problem::compiling(status::SUCCESS, "English"), None);
        assert_eq!(
            Problem::compiling(status::TOPIC_LANGUAGE_NOT_SUPPORTED, " Welsh "),
            Some(Problem::Language("Welsh".into()))
        );
        // Statuses that end a session gently are still failures to compile.
        assert_eq!(
            Problem::compiling(status::USER_CANCELED, ""),
            Some(Problem::Code(5))
        );
        assert_eq!(
            Problem::compiling(status::NETWORK_FAILURE, ""),
            Some(Problem::Offline)
        );
    }

    #[test]
    fn phrases_join_with_a_space_except_in_scripts_without_them() {
        let mut p = Phrases::default();
        p.result("今日は晴れです。", false);
        p.result("散歩に行きます。", false);
        assert_eq!(p.text(), "今日は晴れです。散歩に行きます。");
        p.hypothesis("OK");
        assert_eq!(p.text(), "今日は晴れです。散歩に行きます。OK");
        let mut p = Phrases::default();
        p.result("我们走吧", false);
        p.result("Then English.", false);
        assert_eq!(p.text(), "我们走吧Then English.");
        let mut p = Phrases::default();
        p.result("안녕하세요.", false);
        p.result("반갑습니다.", false);
        assert_eq!(p.text(), "안녕하세요. 반갑습니다.", "Korean is spaced");
    }

    #[test]
    fn error_codes_say_what_to_do() {
        let say = |code: u32| Problem::from_code(code as i32);
        assert_eq!(say(0x8004_5509), Problem::OnlineSpeechOff);
        assert_eq!(say(0x8007_0005), Problem::MicrophoneOff);
        assert_eq!(say(0x8007_0490), Problem::NoMicrophone);
        assert_eq!(say(0x8889_0004), Problem::NoMicrophone);
        assert_eq!(say(0x8889_000A), Problem::MicrophoneBusy);
        assert_eq!(say(0x8007_2EE7), Problem::Offline);
        assert_eq!(say(0x8007_2EFD), Problem::Offline);
        assert_eq!(say(0x8004_0154), Problem::Unavailable);
        assert_eq!(say(0x8007_3D54), Problem::Unavailable);
        assert_eq!(say(0x8000_4005), Problem::Code(0x8000_4005));

        assert_eq!(
            Problem::OnlineSpeechOff.sentence(),
            "Dictation uses Windows' online speech recognition, which is off. Turn it on in Settings › Privacy & security › Speech."
        );
        assert_eq!(
            Problem::MicrophoneOff.sentence(),
            "Windows isn't letting Trek use the microphone. Turn on microphone access for desktop apps in Settings › Privacy & security › Microphone."
        );
        assert_eq!(
            Problem::Unavailable.sentence(),
            "Windows speech recognition isn't available on this PC. Press Win+H to dictate."
        );
        assert_eq!(
            Problem::Language("Welsh (United Kingdom)".into()).sentence(),
            "Windows can't dictate in Welsh (United Kingdom). Choose another speech language in Settings › Time & language › Speech."
        );
        assert_eq!(
            Problem::Language(String::new()).sentence(),
            "Windows can't dictate in its speech language. Choose another in Settings › Time & language › Speech."
        );
        assert_eq!(
            Problem::Code(0x8000_4005).sentence(),
            "Windows speech recognition failed (error 0x80004005)."
        );
        // Every sentence is one the composer can show as it is: whole, and naming nothing of
        // the Mac's.
        for problem in [
            Problem::OnlineSpeechOff,
            Problem::OnlineSpeechManaged,
            Problem::MicrophoneOff,
            Problem::NoMicrophone,
            Problem::MicrophoneBusy,
            Problem::Offline,
            Problem::Language("Welsh".into()),
            Problem::Unavailable,
            Problem::NoAnswer,
            Problem::Unclear,
            Problem::Stopped,
            Problem::Code(1),
        ] {
            let s = problem.sentence();
            assert!(s.ends_with('.'), "{problem:?}: {s}");
            assert!(!s.contains("Mac") && !s.contains("System Settings"), "{s}");
        }
    }

    #[test]
    fn only_what_trek_cant_change_lasts_and_only_privacy_switches_open_settings() {
        assert!(Problem::Unavailable.lasting());
        assert!(Problem::OnlineSpeechManaged.lasting());
        for p in [
            Problem::OnlineSpeechOff,
            Problem::MicrophoneOff,
            Problem::NoMicrophone,
            Problem::MicrophoneBusy,
            Problem::Offline,
            Problem::NoAnswer,
        ] {
            assert!(!p.lasting(), "{p:?} can be fixed and tried again");
        }
        assert_eq!(Problem::OnlineSpeechOff.switch(), Some(SPEECH_PAGE));
        assert_eq!(Problem::MicrophoneOff.switch(), Some(MICROPHONE_PAGE));
        assert_eq!(Problem::OnlineSpeechManaged.switch(), None);
        assert_eq!(Problem::NoMicrophone.switch(), None);
        assert_eq!(Problem::Unavailable.switch(), None);
    }

    #[test]
    fn a_peak_reads_as_the_macs_rms_would() {
        assert_eq!(loudness_peak(0.), 0.);
        assert_eq!(loudness_peak(-0.5), 0.);
        assert_eq!(loudness_peak(f32::NAN), 0.);
        // Speech peaking at -20 dBFS sits where -30 dB of RMS does on a Mac: mid-strip.
        let speech = loudness_peak(0.1);
        assert!((speech - crate::loudness_db(-30.)).abs() < 1e-4, "{speech}");
        assert!(speech > 0.3 && speech < 0.7, "{speech}");
        // Full scale is near the top, and past it is held there.
        assert!(loudness_peak(1.) > 0.9);
        assert_eq!(loudness_peak(4.), loudness_peak(1.));
        // A whisper of a peak is silence.
        assert_eq!(loudness_peak(0.001), 0.);
    }
}
