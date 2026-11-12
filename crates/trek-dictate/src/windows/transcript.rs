//! What a take's sound becomes, for the on-device engine (`local`): when the user is speaking
//! (`Speech`: the sound's loudness against the room's own noise, which also ends a take that
//! goes quiet), which part of the take each of Whisper's passes decodes and how the passes add up
//! to the take's text (`Transcript`), and which of Whisper's output is words (`clean`). Plain code
//! every platform's tests run; `local` runs Whisper over what these choose.

use super::audio::RATE;
use super::logic::unspaced;
use std::ops::Range;

/// A frame of loudness: 20 ms.
pub const FRAME: usize = RATE as usize / 50;
/// A minute without a word ends a take, as Windows' recognizer was set to (`winrt::SILENCE`):
/// waiting for the first words, and after the last ones.
pub const FIRST_WORDS: usize = 60 * RATE as usize;
pub const SILENCE: usize = 60 * RATE as usize;
/// The open part of a take settles once a pass over it runs past this: everything before its
/// last phrase stays as heard, and later passes decode only from there, so a long take costs no
/// more per pass than a short one.
pub const SETTLE_AFTER: usize = 20 * RATE as usize;
/// Sound kept either side of the speech a pass decodes: a long silence is where Whisper makes up
/// words ("Thank you."), and these are enough for a soft first or last syllable.
pub const PAD: usize = RATE as usize * 3 / 10;

/// Where speech starts in a room: the room's own noise starts here and settles to what it is.
const FLOOR_START: f32 = -60.;
/// Speech is this much louder than the room…
const MARGIN: f32 = 10.;
/// …and at least this loud (dBFS): quieter is never a voice into a microphone.
const QUIETEST: f32 = -52.;
/// Frames in a row (60 ms) before loud sound counts as speech, so a click doesn't.
const RUN: usize = 3;
/// How fast the room's level rises to louder sound, per frame (it falls to quieter sound at
/// once): about 7 s to settle, so a voice, which keeps pausing, never becomes the room.
const RISE: f32 = 0.003;

/// When the user is speaking, frame by frame: louder than the room by `MARGIN` (and than
/// `QUIETEST`) for `RUN` frames in a row. Samples go in as the take records them.
#[derive(Debug)]
pub struct Speech {
    /// The room's own level, dBFS.
    floor: f32,
    /// Loud frames in a row, and the sample their run started at.
    run: usize,
    run_start: usize,
    frames: usize,
    /// The part of a frame not yet whole.
    pending: Vec<f32>,
    /// From the first sample of speech to one past the last, once there's been some.
    heard: Option<Range<usize>>,
}

impl Default for Speech {
    fn default() -> Self {
        Speech {
            floor: FLOOR_START,
            run: 0,
            run_start: 0,
            frames: 0,
            pending: Vec::with_capacity(FRAME),
            heard: None,
        }
    }
}

impl Speech {
    /// The take's next samples.
    pub fn push(&mut self, mut samples: &[f32]) {
        while !samples.is_empty() {
            let take = (FRAME - self.pending.len()).min(samples.len());
            self.pending.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.pending.len() == FRAME {
                let frame = std::mem::take(&mut self.pending);
                self.frame(&frame);
                self.pending = frame;
                self.pending.clear();
            }
        }
    }

    fn frame(&mut self, frame: &[f32]) {
        let power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        let db = (10. * power.max(1e-10).log10()).max(-100.);
        let start = self.frames * FRAME;
        if db > (self.floor + MARGIN).max(QUIETEST) {
            if self.run == 0 {
                self.run_start = start;
            }
            self.run += 1;
            if self.run >= RUN {
                let end = start + FRAME;
                self.heard = Some(match self.heard.take() {
                    Some(h) => h.start..end,
                    None => self.run_start..end,
                });
            }
        } else {
            self.run = 0;
        }
        if db < self.floor {
            self.floor = db;
        } else {
            self.floor += (db - self.floor) * RISE;
        }
        self.frames += 1;
    }

    /// Samples taken so far.
    pub fn samples(&self) -> usize {
        self.frames * FRAME + self.pending.len()
    }

    /// From the first speech to the end of the last, if any.
    pub fn heard(&self) -> Option<Range<usize>> {
        self.heard.clone()
    }

    /// The take should end on its own: a minute with nothing said, from its start or since the
    /// last words.
    pub fn over(&self) -> bool {
        let now = self.samples();
        match &self.heard {
            None => now >= FIRST_WORDS,
            Some(h) => now.saturating_sub(h.end) >= SILENCE,
        }
    }
}

/// A phrase Whisper heard, and where it is in the take, in samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub span: Range<usize>,
    pub text: String,
}

/// A take's text as Whisper's passes make it. A pass decodes the open part of the take (from
/// `from` to as far as it's been recorded) and replaces what the last pass made of it; once the
/// open part is long, its phrases but the last settle and the open part starts after them.
#[derive(Debug, Default)]
pub struct Transcript {
    settled: Vec<String>,
    from: usize,
    /// The latest pass's phrases over the open part.
    open: Vec<String>,
}

impl Transcript {
    /// Where the open part starts.
    pub fn from(&self) -> usize {
        self.from
    }

    /// What a pass over the take recorded up to `to` decodes, given where speech was `heard`:
    /// the open part, less the silence before its first speech and after its last but for
    /// `PAD` either side. `None` when there's been no speech in it to decode.
    pub fn window(&self, to: usize, heard: Option<&Range<usize>>) -> Option<Range<usize>> {
        let heard = heard?;
        if heard.end <= self.from {
            return None;
        }
        let start = heard
            .start
            .max(self.from)
            .saturating_sub(PAD)
            .max(self.from);
        let end = heard.end.saturating_add(PAD).min(to);
        (end > start).then_some(start..end)
    }

    /// A pass over `window` heard `segments`: the take's text so far. A pass that ran past
    /// `SETTLE_AFTER` from the open part's start settles its phrases but the last.
    pub fn heard(&mut self, window: Range<usize>, segments: Vec<Segment>) -> String {
        let phrases: Vec<Segment> = segments
            .into_iter()
            .filter_map(|s| {
                let text = clean(&s.text);
                (!text.is_empty()).then_some(Segment { text, ..s })
            })
            .collect();
        if window.end.saturating_sub(self.from) > SETTLE_AFTER && phrases.len() >= 2 {
            let (done, last) = phrases.split_at(phrases.len() - 1);
            self.settled.extend(done.iter().map(|s| s.text.clone()));
            if let Some(end) = done.last().map(|s| s.span.end) {
                self.from = end.clamp(self.from, window.end);
            }
            self.open = last.iter().map(|s| s.text.clone()).collect();
        } else {
            self.open = phrases.into_iter().map(|s| s.text).collect();
        }
        self.text()
    }

    /// Everything heard so far.
    pub fn text(&self) -> String {
        join(self.settled.iter().chain(&self.open))
    }
}

/// Phrases one after another: a space between them, except where both sides are written
/// without spaces (Chinese and Japanese).
fn join<'a>(phrases: impl Iterator<Item = &'a String>) -> String {
    let mut text = String::new();
    for phrase in phrases {
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

/// A phrase of Whisper's as words: what it marks as no speech at all goes (`[BLANK_AUDIO]`,
/// `[Music]`, a phrase that's all `(sighs)`, ♪), spaces are tidied, and a phrase with no letter
/// or digit left is nothing.
pub fn clean(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            '♪' | '♫' | '♬' => {}
            c if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    let mut text = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let whole = |open: char, close: char, t: &str| {
        t.starts_with(open) && t.ends_with(close) && t[1..].find(close) == Some(t.len() - 2)
    };
    if whole('(', ')', &text) || whole('*', '*', &text) && text.len() > 1 {
        text.clear();
    }
    if !text.chars().any(char::is_alphanumeric) {
        text.clear();
    }
    text
}

/// A take's text as it goes in, in `language` (Whisper's short name): English starts with a
/// capital and ends with a stop, as the Mac's recognizer writes every take; Whisper now and then
/// leaves the last stop off ("…of x squared"). One that ends in a quote or a bracket, or another
/// language, goes in as Whisper wrote it.
pub fn finished(text: &str, language: Option<&str>) -> String {
    let text = text.trim();
    if language != Some("en") {
        return text.to_string();
    }
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    out.push_str(chars.as_str());
    if out.chars().last().is_some_and(char::is_alphanumeric) {
        out.push('.');
    }
    out
}

/// More words a second than anyone says (a fast talker manages four or five).
pub const WORDS_A_SECOND: f32 = 7.;

/// A pass over `samples` of sound heard more words than anyone could say in that time: Whisper
/// went round and round. A quick pass over the first words, cut off mid-sentence, does ("Use the
/// chain rule to use the chain rule to …", forty times over a second and a half), as its greedy
/// search has no second try. Phrases said again and again by a person ("Hello? Hello? Hello?")
/// stay within it.
pub fn babbled(samples: usize, segments: &[Segment]) -> bool {
    let words: usize = segments
        .iter()
        .map(|s| clean(&s.text).split_whitespace().count())
        .sum();
    words as f32 > WORDS_A_SECOND * samples as f32 / RATE as f32 + 4.
}

/// What Whisper reads before decoding a pass again, for the style it writes in. Left to itself
/// Whisper sometimes writes English plainly, all lowercase and without stops ("use the chain
/// rule to find the derivative of sine of x squared"), where the Mac's recognizer punctuates
/// every take. Tried beside others on Windows' voices, this one gets the capitals and stops back
/// and never comes back itself; one that did ("Hi. Okay, so.") heard noise as "Thank you very
/// much".
pub const WRITTEN: &str = "Hello, how are you?";

/// A pass in `language` (Whisper's short name) came back as English written plainly: letters,
/// not one of them a capital. Only such a pass is decoded again with [`WRITTEN`]: the prompt is
/// English, and given to every English pass it changes takes that were right ("Hello? Hello?"
/// comes back "Hello Hello", now and then with a "Hello" too many).
pub fn plain_english(language: Option<&str>, text: &str) -> bool {
    language == Some("en")
        && text.chars().any(char::is_alphabetic)
        && !text.chars().any(char::is_uppercase)
}

/// The take's language: Whisper's guess from a pass with enough speech to go on, then kept, so
/// later passes skip guessing (it costs a pass of the encoder) and a short pause can't flip it.
#[derive(Debug, Default)]
pub struct Language {
    kept: Option<i32>,
    /// The latest guess, kept or not.
    last: Option<i32>,
}

/// Speech to go on before a guess is kept: 2 s.
pub const ENOUGH: usize = 2 * RATE as usize;

/// The languages of the speech model (Whisper base), by id: 0 to 98. whisper.cpp guesses among
/// one more, Cantonese (99, "yue"), which only large-v3 has. For base, that one's chance is
/// another token's (the one that asks for a translation), and on a quick pass over the first
/// seconds it can come out on top: plain English was kept as Cantonese, and the take's last
/// pass decoded as Cantonese lost its stops.
pub const LANGUAGES: i32 = 99;

impl Language {
    /// The language to decode in, once kept (Whisper's id for it).
    pub fn kept(&self) -> Option<i32> {
        self.kept
    }

    /// The take's language as well as it's known: kept, else the latest guess.
    pub fn best(&self) -> Option<i32> {
        self.kept.or(self.last)
    }

    /// A pass over `speech` samples of speech guessed `id` (negative: no guess). A guess
    /// outside the model's languages is no guess.
    pub fn guessed(&mut self, id: i32, speech: usize) {
        if !(0..LANGUAGES).contains(&id) {
            return;
        }
        self.last = Some(id);
        if self.kept.is_none() && speech >= ENOUGH {
            self.kept = Some(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: usize = RATE as usize;

    /// `seconds` of a tone at `db` dBFS (RMS), or of silence below -100.
    fn sound(db: f32, seconds: f32) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        if db <= -100. {
            return vec![0.; n];
        }
        let amp = 10f32.powf(db / 20.) * std::f32::consts::SQRT_2;
        (0..n)
            .map(|i| amp * (i as f32 * 2. * std::f32::consts::PI * 220. / RATE as f32).sin())
            .collect()
    }

    fn seg(from: f32, to: f32, text: &str) -> Segment {
        Segment {
            span: (from * SECOND as f32) as usize..(to * SECOND as f32) as usize,
            text: text.into(),
        }
    }

    #[test]
    fn speech_is_what_rises_above_the_room() {
        let mut s = Speech::default();
        s.push(&sound(-70., 1.));
        assert_eq!(s.heard(), None, "a quiet room");
        s.push(&sound(-30., 0.5));
        let heard = s.heard().expect("a voice");
        assert_eq!(heard, SECOND..SECOND + SECOND / 2);
        s.push(&sound(-70., 0.4));
        assert_eq!(s.heard(), Some(heard), "the pause after isn't speech");
        assert_eq!(s.samples(), SECOND * 19 / 10);
    }

    #[test]
    fn a_click_isnt_speech_and_packets_can_split_frames() {
        let mut s = Speech::default();
        s.push(&sound(-70., 0.5));
        // 40 ms of loud: two frames, short of a run.
        s.push(&sound(-20., 0.04));
        s.push(&sound(-70., 0.5));
        assert_eq!(s.heard(), None);
        // The same speech in odd packets is found where it is.
        let mut s = Speech::default();
        let all = [sound(-70., 0.5), sound(-25., 0.3), sound(-70., 0.2)].concat();
        for chunk in all.chunks(77) {
            s.push(chunk);
        }
        assert_eq!(s.heard(), Some(SECOND / 2..SECOND / 2 + SECOND * 3 / 10));
    }

    #[test]
    fn a_noisy_room_becomes_the_floor_and_speech_still_stands_out() {
        let mut s = Speech::default();
        // A fan at -45 dB: loud enough to pass for speech while the floor finds it…
        s.push(&sound(-45., 15.));
        let early = s.heard();
        // …but not once it has: ten seconds more of it are the room.
        s.push(&sound(-45., 10.));
        assert_eq!(s.heard(), early, "the fan stopped counting as speech");
        assert!(
            early.as_ref().is_some_and(|h| h.end < 10 * SECOND),
            "{early:?}"
        );
        s.push(&sound(-25., 1.));
        assert_eq!(s.heard().map(|h| h.end), Some(26 * SECOND));
    }

    #[test]
    fn digital_silence_doesnt_make_everything_speech() {
        let mut s = Speech::default();
        s.push(&vec![0.; SECOND]);
        s.push(&sound(-56., 2.));
        assert_eq!(s.heard(), None, "below QUIETEST whatever the floor");
    }

    #[test]
    fn a_minute_with_nothing_said_ends_the_take() {
        let mut s = Speech::default();
        s.push(&sound(-70., 59.9));
        assert!(!s.over());
        s.push(&sound(-70., 0.2));
        assert!(s.over(), "no first words in a minute");

        let mut s = Speech::default();
        s.push(&sound(-70., 30.));
        s.push(&sound(-25., 2.));
        s.push(&sound(-70., 59.));
        assert!(!s.over(), "a long pause to think");
        s.push(&sound(-70., 1.5));
        assert!(s.over(), "a minute since the last words");
    }

    #[test]
    fn a_pass_decodes_the_speech_in_the_open_part() {
        let t = Transcript::default();
        assert_eq!(t.window(10 * SECOND, None), None, "nothing said yet");
        let heard = 2 * SECOND..5 * SECOND;
        assert_eq!(
            t.window(10 * SECOND, Some(&heard)),
            Some(2 * SECOND - PAD..5 * SECOND + PAD),
            "the silence either side goes, but for a pad"
        );
        assert_eq!(
            t.window(5 * SECOND, Some(&(0..5 * SECOND))),
            Some(0..5 * SECOND),
            "the pad never runs past either end"
        );
    }

    #[test]
    fn passes_revise_the_text_and_a_long_take_settles() {
        let mut t = Transcript::default();
        assert_eq!(
            t.heard(0..3 * SECOND, vec![seg(0.2, 2.8, " Make the")]),
            "Make the"
        );
        assert_eq!(
            t.heard(
                0..6 * SECOND,
                vec![
                    seg(0.2, 3.1, " Make the buttons bigger."),
                    seg(3.4, 5.8, " But")
                ]
            ),
            "Make the buttons bigger. But",
            "each pass replaces the last"
        );
        assert_eq!(t.from(), 0, "short: nothing settles");
        // Past 20 s, all but the last phrase settle, and the open part starts after them.
        let said = t.heard(
            0..21 * SECOND,
            vec![
                seg(0.2, 3.1, " Make the buttons bigger."),
                seg(3.4, 12.0, " But keep the padding."),
                seg(12.5, 20.9, " And the font"),
            ],
        );
        assert_eq!(
            said,
            "Make the buttons bigger. But keep the padding. And the font"
        );
        assert_eq!(t.from(), 12 * SECOND);
        // The next pass decodes from there, and its phrases follow the settled ones.
        let heard = 0..23 * SECOND;
        assert_eq!(
            t.window(24 * SECOND, Some(&heard)),
            Some(12 * SECOND..23 * SECOND + PAD)
        );
        assert_eq!(
            t.heard(
                12 * SECOND..23 * SECOND,
                vec![seg(12.5, 22.8, " And the font size too.")]
            ),
            "Make the buttons bigger. But keep the padding. And the font size too."
        );
    }

    #[test]
    fn a_long_single_phrase_waits_to_settle() {
        let mut t = Transcript::default();
        t.heard(0..25 * SECOND, vec![seg(0., 25., " On and on")]);
        assert_eq!(t.from(), 0, "one phrase: nowhere to cut");
        assert_eq!(t.text(), "On and on");
    }

    #[test]
    fn whispers_marks_for_no_speech_are_dropped() {
        assert_eq!(clean(" [BLANK_AUDIO]"), "");
        assert_eq!(clean("[Music] ♪ la la ♪"), "la la");
        assert_eq!(clean(" (sighs)"), "");
        assert_eq!(clean("*laughs*"), "");
        assert_eq!(clean(" ..."), "");
        assert_eq!(clean("  Hello,   world.  "), "Hello, world.");
        assert_eq!(
            clean("Call it (the new one) Trek."),
            "Call it (the new one) Trek.",
            "brackets inside a phrase are words"
        );
        assert_eq!(clean("(one) and (two)"), "(one) and (two)");
        let mut t = Transcript::default();
        assert_eq!(
            t.heard(
                0..4 * SECOND,
                vec![seg(0., 1., "[BLANK_AUDIO]"), seg(1., 3., " Hello.")]
            ),
            "Hello."
        );
    }

    #[test]
    fn phrases_join_with_a_space_except_in_scripts_without_them() {
        let mut t = Transcript::default();
        t.heard(
            0..4 * SECOND,
            vec![
                seg(0., 1., "今日は晴れです。"),
                seg(1., 2., "散歩に行きます。"),
            ],
        );
        assert_eq!(t.text(), "今日は晴れです。散歩に行きます。");
        t.heard(
            0..4 * SECOND,
            vec![seg(0., 1., "안녕하세요."), seg(1., 2., "반갑습니다.")],
        );
        assert_eq!(t.text(), "안녕하세요. 반갑습니다.");
    }

    #[test]
    fn the_language_is_kept_once_theres_enough_to_go_on() {
        let mut l = Language::default();
        assert_eq!(l.best(), None);
        l.guessed(3, ENOUGH - 1);
        assert_eq!(l.kept(), None, "too little speech to trust");
        assert_eq!(l.best(), Some(3), "but it's the best guess so far");
        l.guessed(-1, ENOUGH);
        assert_eq!(l.kept(), None, "no guess");
        // Cantonese, which the base model doesn't have: its "guess" is another token's chance.
        l.guessed(LANGUAGES, 10 * ENOUGH);
        assert_eq!((l.kept(), l.best()), (None, Some(3)), "no guess either");
        l.guessed(0, ENOUGH);
        assert_eq!(l.kept(), Some(0));
        l.guessed(5, 10 * ENOUGH);
        assert_eq!(l.kept(), Some(0), "kept for the rest of the take");
    }

    #[test]
    fn an_english_take_goes_in_with_a_capital_and_a_stop() {
        let en = Some("en");
        assert_eq!(
            finished(
                "Use the chain rule to find the derivative of sine of x squared",
                en
            ),
            "Use the chain rule to find the derivative of sine of x squared."
        );
        assert_eq!(finished(" fix it ", en), "Fix it.");
        // Written already, or ending in what a stop shouldn't follow: as it is.
        assert_eq!(finished("Can you hear me?", en), "Can you hear me?");
        assert_eq!(finished("Done.", en), "Done.");
        assert_eq!(finished("Call it \"done\"", en), "Call it \"done\"");
        assert_eq!(finished("Run it (twice)", en), "Run it (twice)");
        assert_eq!(finished("Use version 2", en), "Use version 2.");
        assert_eq!(finished("", en), "");
        // Another language, or one not known: as Whisper wrote it.
        assert_eq!(finished("usa la regla", Some("es")), "usa la regla");
        assert_eq!(finished("今日は晴れです", Some("ja")), "今日は晴れです");
        assert_eq!(finished("use the chain rule", None), "use the chain rule");
    }

    #[test]
    fn a_pass_that_went_round_and_round_is_told_from_speech() {
        let looped = "Use the chain rule to ".repeat(40);
        assert!(babbled(SECOND * 3 / 2, &[seg(0., 1.5, &looped)]));
        // What people say, quickly or again and again, isn't.
        let said = [seg(0., 3.6, "Trek can now take dictation on Windows.")];
        assert!(!babbled(SECOND * 36 / 10, &said));
        let again = [
            seg(0., 1., "Hello."),
            seg(3., 4., "Hello."),
            seg(6., 7., "Hello."),
        ];
        assert!(!babbled(7 * SECOND, &again));
        assert!(!babbled(
            SECOND / 2,
            &[seg(0., 0.5, "Yes, go ahead and do it")]
        ));
        assert!(!babbled(SECOND, &[]));
        // Whisper's marks for what isn't speech aren't words.
        assert!(!babbled(
            SECOND,
            &[seg(0., 1., "[BLANK_AUDIO] (music) ♪ ♪ ♪ ♪ ♪ ♪ ♪ ♪ ♪ ♪ ♪ ♪")]
        ));
    }

    #[test]
    fn only_english_written_plainly_is_decoded_again() {
        // Plain: decoded again, prompted to write as written.
        let plain = "use the chain rule to find the derivative of sine of x squared";
        assert!(plain_english(Some("en"), plain));
        // As written, another language's (the prompt is English), or no words: kept.
        assert!(!plain_english(Some("en"), "Use the chain rule."));
        assert!(!plain_english(Some("en"), "Hello Hello Hello"));
        assert!(!plain_english(Some("en"), "fix the bug in iOS"));
        assert!(!plain_english(Some("es"), "usa la regla de la cadena"));
        assert!(!plain_english(None, plain));
        assert!(!plain_english(Some("en"), ""));
        assert!(!plain_english(Some("en"), " 42 "));
    }
}
