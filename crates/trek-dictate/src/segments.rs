//! A take's text from a recognizer that hears it a segment at a time, as macOS's Speech
//! framework does: after a pause it closes the segment it was hearing (a result with its metadata
//! attached, seen on macOS 27 with on-device recognition) and the partial results after that hold
//! only the new segment's words; the request's last result (`isFinal`) holds only the last
//! segment too. A request can also end while the mic is still open (a server recognizer's limit
//! of about a minute), and a new one takes over from nothing. Taken as "the whole take so far",
//! each of those results would put only the last stretch of speech in the composer.
//!
//! `Segments` keeps the closed segments in order and the open one after them, so what the
//! composer hears ([`Heard::Partial`], [`Heard::Final`]) is always the whole take. It's plain
//! code every platform's tests run; the Mac's result handler only forwards what Speech says.

use crate::Heard;

/// What a recognizer's result is to the take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A guess at the segment being spoken, revised by the next one.
    Partial,
    /// The segment is over (a pause); the next result starts a new one.
    Segment,
    /// The request is over: its last segment, settled.
    Done,
}

/// The take's segments: the ones the recognizer closed, and its latest guess at the open one.
#[derive(Debug, Default)]
pub struct Segments {
    settled: Vec<String>,
    open: String,
    /// The current request has heard words: one that ends while the mic is still open is worth
    /// a new request; one that heard nothing is the take going quiet, and ends it.
    heard: bool,
}

/// What a result comes to: what the composer hears, and whether the recognizer needs a new
/// request to carry on the take.
#[derive(Debug, PartialEq)]
pub struct Next {
    pub heard: Heard,
    pub restart: bool,
}

impl Segments {
    /// A result with `text` (the recognizer's words for the open segment only). `listening`:
    /// the mic is still open, so a request that ends is the recognizer's doing, not the take's.
    pub fn result(&mut self, kind: Kind, text: &str, listening: bool) -> Next {
        let text = text.trim();
        self.heard |= !text.is_empty();
        match kind {
            Kind::Partial => self.open = text.to_string(),
            Kind::Segment | Kind::Done => self.settle(text),
        }
        let restart = kind == Kind::Done && listening && std::mem::take(&mut self.heard);
        Next {
            heard: if kind == Kind::Done && !restart {
                Heard::Final(self.text())
            } else {
                Heard::Partial(self.text())
            },
            restart,
        }
    }

    /// The open segment is over, as `text`. A segment the recognizer closes without words keeps
    /// the last guess at it; one it sends again (the request's last result repeating the segment
    /// a pause already closed) replaces its first copy rather than following it.
    fn settle(&mut self, text: &str) {
        let open = std::mem::take(&mut self.open);
        let text = if text.is_empty() { open.as_str() } else { text };
        if text.is_empty() {
            return;
        }
        match self.settled.last_mut() {
            Some(last) if open.is_empty() && same_words(last, text) => *last = text.to_string(),
            _ => self.settled.push(text.to_string()),
        }
    }

    /// Everything heard so far: the closed segments, then the open one.
    pub fn text(&self) -> String {
        join(self.settled.iter().chain(Some(&self.open)))
    }
}

/// Whether two segments say the same words, whatever their capitals and punctuation.
fn same_words(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        s.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    words(a) == words(b)
}

/// Segments one after another: a space between them, except where both sides are written
/// without spaces (Chinese and Japanese).
fn join<'a>(segments: impl Iterator<Item = &'a String>) -> String {
    let mut text = String::new();
    for segment in segments.filter(|s| !s.is_empty()) {
        let joins = text
            .chars()
            .last()
            .zip(segment.chars().next())
            .is_some_and(|(left, right)| !unspaced(left) && !unspaced(right));
        if joins {
            text.push(' ');
        }
        text.push_str(segment);
    }
    text
}

/// Written without spaces between words or sentences (Chinese and Japanese): segments in it
/// follow one another directly.
pub(crate) fn unspaced(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x30FF // CJK punctuation, hiragana, katakana
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK ideographs
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF00..=0xFFEF // full-width forms
    )
}

#[cfg(test)]
mod tests {
    use super::Kind::{Done, Partial, Segment};
    use super::*;

    /// Feed `results` in order, the mic open until the last; what the composer heard after each.
    fn play(take: &mut Segments, results: &[(Kind, &str)]) -> Vec<Next> {
        results
            .iter()
            .enumerate()
            .map(|(i, (kind, text))| take.result(*kind, text, i + 1 < results.len()))
            .collect()
    }

    fn partial(text: &str) -> Next {
        Next {
            heard: Heard::Partial(text.into()),
            restart: false,
        }
    }

    /// The owner's case, as macOS 27's on-device recognizer reported it for three sentences with
    /// pauses between: each pause closes a segment and the partials after it start over, and the
    /// last result repeats only the last segment. ✓ keeps all three, in order.
    #[test]
    fn three_sentences_with_pauses_are_all_kept() {
        let mut take = Segments::default();
        let heard = play(
            &mut take,
            &[
                (Partial, "The first"),
                (Partial, "The first thing I want is a button that save"),
                (
                    Partial,
                    "The first thing I want is a button that saves the draft",
                ),
                (
                    Segment,
                    "The first thing I want is a button that saves the draft.",
                ),
                (Partial, "Then"),
                (Partial, "Then the second part should show the list of file"),
                (
                    Segment,
                    "Then the second part should show the list of files that changed.",
                ),
                (Partial, "And"),
                (
                    Partial,
                    "And finally the third sentence asks for a summary of the end",
                ),
                (
                    Done,
                    "And finally, the third sentence asks for a summary at the end.",
                ),
            ],
        );
        let all = "The first thing I want is a button that saves the draft. Then the second part \
                   should show the list of files that changed. And finally, the third sentence \
                   asks for a summary at the end.";
        assert_eq!(
            heard[4],
            partial("The first thing I want is a button that saves the draft. Then"),
            "a new segment's first partial follows the closed one"
        );
        assert_eq!(
            heard[8],
            partial(
                "The first thing I want is a button that saves the draft. Then the second part \
                 should show the list of files that changed. And finally the third sentence asks \
                 for a summary of the end"
            ),
            "a revision of the open segment replaces only that segment"
        );
        assert_eq!(
            heard.last(),
            Some(&Next {
                heard: Heard::Final(all.into()),
                restart: false
            })
        );
    }

    /// ✓ in a pause after a segment closed: the request's last result says that segment again,
    /// and it isn't doubled.
    #[test]
    fn a_segment_sent_again_as_final_is_not_doubled() {
        let mut take = Segments::default();
        let heard = play(
            &mut take,
            &[
                (Partial, "First sentence"),
                (Segment, "First sentence."),
                (Partial, "Then a second sentence comes"),
                (Segment, "Then a second sentence comes"),
                (Done, "Then a second sentence comes."),
            ],
        );
        assert_eq!(
            heard.last().map(|n| &n.heard),
            Some(&Heard::Final(
                "First sentence. Then a second sentence comes.".into()
            ))
        );
    }

    /// A segment closed without words keeps what was shown for it, and spaces never double.
    #[test]
    fn a_segment_closed_empty_keeps_its_last_guess() {
        let mut take = Segments::default();
        take.result(Partial, "  hello there  ", true);
        take.result(Segment, "", true);
        take.result(Partial, " ", true);
        let next = take.result(Partial, "general Kenobi", true);
        assert_eq!(next, partial("hello there general Kenobi"));
        let done = take.result(Done, "", false);
        assert_eq!(
            done.heard,
            Heard::Final("hello there general Kenobi".into())
        );
    }

    /// The recognizer ends its request while the mic is open (a server's minute): a new request
    /// takes over, and its words follow the old one's.
    #[test]
    fn a_request_ended_mid_take_is_restarted_and_kept() {
        let mut take = Segments::default();
        take.result(Partial, "A long first stretch", true);
        let ended = take.result(Done, "A long first stretch of talk.", true);
        assert_eq!(
            ended,
            Next {
                heard: Heard::Partial("A long first stretch of talk.".into()),
                restart: true
            }
        );
        take.result(Partial, "and more", true);
        let done = take.result(Done, "And more after the limit.", false);
        assert_eq!(
            done,
            Next {
                heard: Heard::Final(
                    "A long first stretch of talk. And more after the limit.".into()
                ),
                restart: false
            }
        );
    }

    /// A request that ends having heard nothing since the last one is the take gone quiet: it
    /// ends with what was heard rather than restarting for ever.
    #[test]
    fn a_request_that_heard_nothing_ends_the_take() {
        let mut take = Segments::default();
        take.result(Partial, "Something", true);
        assert!(take.result(Done, "Something.", true).restart);
        let quiet = take.result(Done, "", true);
        assert_eq!(
            quiet,
            Next {
                heard: Heard::Final("Something.".into()),
                restart: false
            }
        );
    }

    /// The recorded-file request reports no partials: each segment arrives closed.
    #[test]
    fn segments_without_partials_follow_one_another() {
        let mut take = Segments::default();
        take.result(Segment, "One.", false);
        take.result(Segment, "Two.", false);
        let done = take.result(Done, "Three.", false);
        assert_eq!(done.heard, Heard::Final("One. Two. Three.".into()));
    }

    #[test]
    fn chinese_segments_join_without_a_space() {
        let mut take = Segments::default();
        take.result(Segment, "你好。", true);
        let next = take.result(Partial, "世界", true);
        assert_eq!(next, partial("你好。世界"));
    }
}
