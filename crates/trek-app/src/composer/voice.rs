//! Voice mode: the mic swaps the composer's toolbar row for a live waveform, a ✕ and a ✓. What
//! the recognizer hears so far shows at the cursor in italic, muted text and is revised as it
//! goes; ✓ (↩) puts it in as typed text, ✕ (esc) drops it. The textarea itself is untouched
//! until ✓, so a draft put away mid-take never holds words that weren't confirmed.
//!
//! The state machine, the preview and the level smoothing here are pure, so tests drive them
//! without a microphone; `composer.rs` does the effects and `dictate.rs` the audio.

use gpui_kit::*;
use std::collections::VecDeque;
use std::ops::Range;

/// Where the toolbar row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The usual toolbar, with the mic.
    Idle,
    /// The mic was pressed and macOS is asked for the mic and Speech; the mic spins.
    Asking,
    /// The waveform, ✕ and ✓: the take is open and partial text comes in.
    Listening,
    /// ✓ was pressed: the mic is off and the last words are being settled (the ✓ spins).
    Finishing,
}

/// What happens to voice mode.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The mic button (or the shots harness's `dictate`): start a take, or confirm the open one.
    Mic,
    /// The take is recording. `live`: partial text will come (the audio-buffer request); else
    /// the text arrives once, after ✓ (the recorded-file fallback). `value` and `cursor` are the
    /// composer's text and caret at that moment: the preview splices into them.
    Started {
        live: bool,
        value: String,
        cursor: usize,
    },
    /// The recognizer's best text for the whole take so far.
    Partial(String),
    /// The recognizer is done; this is its text.
    Final(String),
    /// Permission refused, no recognizer, the audio failed: why.
    Failed(String),
    /// ✓ or ↩.
    Confirm,
    /// ✕ or esc, or the composer moved to another thread's draft.
    Cancel,
    /// Trek went to the background.
    Background,
    /// The wait for the final text after ✓ ran out ([`GRACE`]); what was shown goes in.
    GraceOver,
}

/// What the composer does about an [`Event`].
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Effect {
    /// Ask for the mic and Speech, then start a take (`Started`) or say why not (`Failed`).
    pub ask: bool,
    /// Stop the mic; the final text follows (and, for a live take, arm [`GRACE`]).
    pub finish: bool,
    /// The take is over: mic off, recognizer cancelled, the waveform gone.
    pub end: bool,
    /// Text to put in at the cursor, as if typed.
    pub commit: Option<String>,
    /// A note for the user.
    pub toast: Option<String>,
}

/// How long ✓ waits for the recognizer's final text before it puts in what was on screen.
pub(crate) const GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// How often the waveform takes a level and redraws while listening (20 fps).
pub(crate) const TICK: std::time::Duration = std::time::Duration::from_millis(50);

/// Voice mode's state: the phase, the text heard so far, and the levels for the waveform.
pub(crate) struct Voice {
    pub phase: Phase,
    /// Partial text keeps coming (else the text only arrives after ✓).
    pub live: bool,
    /// The recognizer's text so far: shown in italic until it's committed.
    pub interim: String,
    /// The composer's text and caret when the take started.
    value: String,
    cursor: usize,
    pub levels: Levels,
    /// Counts takes, so a permission answer for one already abandoned starts nothing.
    pub take: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            live: false,
            interim: String::new(),
            value: String::new(),
            cursor: 0,
            levels: Levels::default(),
            take: 0,
        }
    }
}

impl Voice {
    /// The waveform row is up (rather than the toolbar).
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Listening | Phase::Finishing)
    }

    pub fn step(&mut self, event: Event) -> Effect {
        use Phase::*;
        match (self.phase, event) {
            (Idle, Event::Mic) => {
                self.phase = Asking;
                self.take += 1;
                Effect {
                    ask: true,
                    ..Default::default()
                }
            }
            (
                Idle | Asking,
                Event::Started {
                    live,
                    value,
                    cursor,
                },
            ) => {
                self.phase = Listening;
                self.live = live;
                self.interim.clear();
                self.cursor = cursor.min(value.len());
                while !value.is_char_boundary(self.cursor) {
                    self.cursor -= 1;
                }
                self.value = value;
                self.levels = Levels::default();
                Effect::default()
            }
            (Asking, Event::Failed(why)) => {
                self.phase = Idle;
                Effect {
                    end: true,
                    toast: Some(why),
                    ..Default::default()
                }
            }
            (Asking, Event::Cancel) => {
                self.phase = Idle;
                Effect {
                    end: true,
                    ..Default::default()
                }
            }
            (Listening | Finishing, Event::Partial(text)) => {
                self.interim = text;
                Effect::default()
            }
            (Listening, Event::Mic | Event::Confirm) => {
                self.phase = Finishing;
                Effect {
                    finish: true,
                    ..Default::default()
                }
            }
            (Listening | Finishing, Event::Cancel) => self.close(None, None),
            // The recognizer ended on its own, or settled after ✓: its text goes in.
            (Listening | Finishing, Event::Final(text)) => {
                let text = if text.trim().is_empty() {
                    std::mem::take(&mut self.interim)
                } else {
                    text
                };
                self.close(Some(text), None)
            }
            (Finishing, Event::GraceOver) if self.live => {
                let text = std::mem::take(&mut self.interim);
                self.close(Some(text), None)
            }
            // After ✓ a failure (often "no speech detected" once the audio ends) keeps what
            // was shown, and only says so when there's nothing.
            (Finishing, Event::Failed(why)) => {
                let text = std::mem::take(&mut self.interim);
                let toast = text.trim().is_empty().then_some(why);
                self.close(Some(text), toast)
            }
            // Not chosen by the user: what was heard stays, and a toast says why it stopped.
            (Listening, Event::Failed(why)) => {
                let text = std::mem::take(&mut self.interim);
                self.close(Some(text), Some(format!("Dictation stopped: {why}")))
            }
            (Listening, Event::Background) => {
                let text = std::mem::take(&mut self.interim);
                self.close(
                    Some(text),
                    Some("Dictation stopped when the window went to the background.".into()),
                )
            }
            _ => Effect::default(),
        }
    }

    /// End the take. `text`: what goes in (`None` drops it, as ✕ does); a commit with
    /// nothing in it says "Nothing heard." unless `toast` already says something.
    fn close(&mut self, text: Option<String>, toast: Option<String>) -> Effect {
        self.phase = Phase::Idle;
        self.interim.clear();
        let nothing = text.as_ref().is_some_and(|t| t.trim().is_empty());
        let commit = text.filter(|t| !t.trim().is_empty());
        let toast = toast.or_else(|| nothing.then(|| "Nothing heard.".to_string()));
        Effect {
            end: true,
            commit,
            toast,
            ..Default::default()
        }
    }

    /// The text area while listening: the composer's text with what's heard so far at the
    /// caret, and that part's range (styled as interim).
    pub fn preview(&self) -> (String, Range<usize>) {
        let (before, after) = self.value.split_at(self.cursor);
        if self.interim.trim().is_empty() {
            return (self.value.clone(), self.cursor..self.cursor);
        }
        let padded = spaced(&self.value, self.cursor, &self.interim);
        let lead = padded.len() - padded.trim_start().len();
        let start = before.len() + lead;
        let end = start + padded.trim().len();
        (format!("{before}{padded}{after}"), start..end)
    }
}

/// Dictated `text` as it goes in at `cursor` of `value`: trimmed, with a space either side
/// where it would otherwise run into a word.
pub(crate) fn spaced(value: &str, cursor: usize, text: &str) -> String {
    let cursor = cursor.min(value.len());
    let left = cursor > 0 && !value[..cursor].ends_with(char::is_whitespace);
    let right = cursor < value.len() && !value[cursor..].starts_with(char::is_whitespace);
    format!(
        "{}{}{}",
        if left { " " } else { "" },
        text.trim(),
        if right { " " } else { "" }
    )
}

/// The waveform's history: one smoothed level per tick, newest last, each 0 (silence, a dot)
/// to 1 (loud, a full bar).
pub(crate) struct Levels {
    bars: VecDeque<f32>,
    /// The smoothed level, between ticks.
    now: f32,
}

/// Bars kept: more than the widest strip shows at [`PITCH`].
pub(crate) const KEPT: usize = 400;
/// A bar and its gap, in px: at 20 ticks a second the strip scrolls 80 px/s.
pub(crate) const PITCH: f32 = 4.;

impl Default for Levels {
    fn default() -> Self {
        Self {
            bars: VecDeque::with_capacity(KEPT),
            now: 0.,
        }
    }
}

impl Levels {
    /// Take one reading (0 to 1, the loudest since the last tick). Rises quickly and falls
    /// back more slowly, so syllables read as bursts rather than flicker.
    pub fn push(&mut self, raw: f32) {
        let raw = if raw.is_finite() {
            raw.clamp(0., 1.)
        } else {
            0.
        };
        let rate = if raw > self.now { 0.7 } else { 0.35 };
        self.now += (raw - self.now) * rate;
        if self.now < 0.01 {
            self.now = 0.;
        }
        if self.bars.len() == KEPT {
            self.bars.pop_front();
        }
        self.bars.push_back(self.now);
    }

    /// Oldest first.
    pub fn bars(&self) -> impl DoubleEndedIterator<Item = f32> + ExactSizeIterator + '_ {
        self.bars.iter().copied()
    }
}

/// The scrolling bar strip: newest at the right edge, silence as dots, the left end faded.
pub(crate) fn waveform(levels: Vec<f32>, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let h = f32::from(bounds.size.height);
            let w = f32::from(bounds.size.width);
            let mid = f32::from(bounds.origin.y) + h / 2.;
            let right = f32::from(bounds.origin.x) + w;
            let slots = (w / PITCH).floor() as usize;
            let fade = 48f32.min(w / 3.);
            // Every slot has a dot; slots with sound rise into bars.
            let heard = levels.iter().rev().copied().chain(std::iter::repeat(0.));
            for (i, level) in heard.take(slots).enumerate() {
                let x = right - (i as f32 + 1.) * PITCH;
                let from_left = x - f32::from(bounds.origin.x);
                let alpha = if fade > 0. {
                    (from_left / fade).clamp(0., 1.)
                } else {
                    1.
                };
                let bar = (2. + level * (h - 2.)).min(h);
                let shade = if level > 0. {
                    color
                } else {
                    color.opacity(0.45)
                };
                window.paint_quad(
                    fill(
                        Bounds::new(point(px(x), px(mid - bar / 2.)), size(px(2.), px(bar))),
                        shade.opacity(shade.a * alpha),
                    )
                    .corner_radii(px(1.)),
                );
            }
        },
    )
    .flex_1()
    .min_w_0()
    .h(px(22.))
}
