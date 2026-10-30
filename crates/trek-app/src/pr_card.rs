//! A live card in the transcript for work a thread's agent pushed out: a pull request it
//! opened (`gh pr create`), watches (`gh run watch`, `gh pr checks`) or merges (`gh pr merge`),
//! and the repo's own releases (`script/release.sh`, `gh workflow run release.yml`,
//! `gh release create`). Where a tool row would only say the command ran, the card follows the
//! thing it made: merge state, checks and conflicts refresh from `gh` while it's open
//! (`Workspace::watch_pr`), and it offers the next step (Resolve conflicts, Merge, Open).
//!
//! The card is a transcript `Item::Tool` whose id starts [`ROW`]; its `detail` is the card as
//! JSON, so it persists like any row and older Treks (and the phone) show a plain tool row. Its
//! `ToolStatus` stays `Done` — it stands for the pull request, not for the call that made it —
//! and `phase` inside says where the PR itself stands.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use trek_core::store::{Item, ToolStatus};

/// `Item::Tool` ids of this form are PR/release cards; the key after it is unique in the thread.
pub const ROW: &str = "trek-pr:";

/// The card row for `key` (a PR's url or `release-<version>`).
pub fn row_id(key: &str) -> String {
    format!("{ROW}{key}")
}

/// The card a transcript row stands for, when it is one.
pub fn of_row(id: &str) -> Option<&str> {
    id.strip_prefix(ROW).filter(|k| !k.is_empty())
}

/// Where the card stands, mirrored on its badge, its merge tip and the composer's chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// `gh pr create` is in flight.
    #[default]
    Creating,
    /// Open; GitHub hasn't said whether it merges yet.
    Open,
    /// Mergeability is being computed or checks are pending (spinner at the tip).
    Checking,
    /// `mergeable: CONFLICTING` (`resolving` says the agent was asked to fix it).
    Conflicted,
    /// `mergeable: MERGEABLE` with no checks pending.
    Ready,
    /// A merge is in flight (the user pressed Merge, or the agent ran `gh pr merge`).
    Merging,
    Merged,
    Closed,
    /// A release is under way (`script/release.sh`, a `release.yml` run).
    Releasing,
    Published,
    /// A workflow run (`gh run watch`) finished green.
    Passed,
    /// The command that made it failed (a run: it finished red or was cancelled).
    Failed,
}

impl Phase {
    /// Nothing more to follow.
    /// How long the composer's chip stays once a card has finished well (merged, published,
    /// passed): long enough to see how it ended, then it goes. The card keeps the record.
    pub const CHIP_LINGER_MS: i64 = 4_000;
    /// How long a finished chip takes to fade out at the end of its linger.
    pub const CHIP_FADE_MS: i64 = 250;

    pub fn terminal(self) -> bool {
        matches!(
            self,
            Phase::Merged | Phase::Closed | Phase::Published | Phase::Passed | Phase::Failed
        )
    }

    /// The badge's words.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Creating => "Creating",
            Phase::Open => "Open",
            Phase::Checking => "Checking",
            Phase::Conflicted => "Conflicts",
            Phase::Ready => "Ready to merge",
            Phase::Merging => "Merging",
            Phase::Merged => "Merged",
            Phase::Closed => "Closed",
            Phase::Releasing => "Releasing",
            Phase::Published => "Published",
            Phase::Passed => "Passed",
            Phase::Failed => "Failed",
        }
    }
}

/// What the card stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Pr,
    Release,
    /// A GitHub Actions run the agent watches (`gh run watch <id>`): its id is `number`, its
    /// workflow's name `title`, its jobs `stages`.
    Run,
}

/// One CI check, as `statusCheckRollup` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    #[default]
    Queued,
    Running,
    Pass,
    Fail,
    /// Skipped, neutral or cancelled: not green, not a failure.
    Skipped,
}

impl CheckState {
    fn pending(self) -> bool {
        matches!(self, CheckState::Queued | CheckState::Running)
    }
}

/// A release stage (`gh run view --json jobs`), or the one step a local release reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub label: String,
    pub state: CheckState,
    /// When the job started and (once it's done) finished, unix ms; 0 until GitHub says. The
    /// card draws how long each took, and places its node by it.
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub ended_ms: i64,
}

/// One commit in a pull request's history. GitHub gives us these alongside the aggregate count;
/// keeping the small human description is what lets the card explain the branch instead of
/// reducing it to anonymous dots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Commit {
    pub title: String,
    pub oid: String,
    pub author: String,
    pub committed_ms: i64,
}

/// The card as kept in `Item::Tool.detail` (JSON). All fields default so a card an older or
/// newer Trek wrote still reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    pub kind: Kind,
    /// The PR's title, or "Release v<version>".
    pub title: String,
    pub number: Option<u64>,
    pub url: Option<String>,
    /// Branch the PR comes from and goes to (`head` empty until known).
    pub head: String,
    pub base: String,
    /// Where `gh` and `git` run for it.
    pub repo_dir: String,
    /// `owner/repo`, once the PR's url says it.
    pub repo: Option<String>,
    pub commits: Option<u32>,
    /// Newest commits shown in the card (GitHub returns the complete PR history).
    pub history: Vec<Commit>,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub phase: Phase,
    pub checks: Vec<Check>,
    /// Conflicting paths, when a trial merge could list them (empty: it can't, or there are none).
    pub conflicts: Vec<String>,
    /// Conflicting paths already resolved, while `resolving` counts the rest down.
    pub resolved: Vec<String>,
    /// Asked to resolve conflicts: the count comes down as files clear.
    pub resolving: bool,
    /// A merge is in flight (the button's own `gh pr merge`, or the agent's).
    pub merging: bool,
    /// The merge deleted the head branch (ours did, or the PR's says so).
    pub branch_deleted: bool,
    /// The line over the card ("Checking it merges cleanly").
    pub step: String,
    /// Release cards: version, channel and stages.
    pub version: String,
    pub channel: String,
    pub stages: Vec<Stage>,
    /// Run cards: what the run is for (its commit's subject, `displayTitle`).
    pub subject: String,
    /// Run cards: what started the run (`push`, `pull_request`, `workflow_dispatch`).
    pub event: String,
    /// When it was made.
    pub created_ms: i64,
    /// When `phase` last changed: draws the phase's graph in (0 until tracked).
    pub phase_at_ms: i64,
    /// Run cards: when the run started and (once completed) ended, for the elapsed label.
    pub started_ms: i64,
    pub ended_ms: i64,
    /// Release cards: the `release.yml` workflow dispatch. Its command ends once the run is
    /// queued, so the run, not the command, says how it went (`fetch_release`).
    pub workflow: bool,
    /// Release cards: the workflow run this dispatch made, once it shows in `gh run list`.
    pub run_id: Option<u64>,
}

impl Card {
    /// The card's short form for the composer's status strip: "#128 Ready to merge".
    pub fn chip(&self) -> String {
        let (what, state) = self.chip_parts();
        format!("{what} {state}")
    }

    /// The chip's two halves, drawn in two tones: "#128" and "Ready to merge".
    pub fn chip_parts(&self) -> (String, String) {
        let what = match self.kind {
            Kind::Pr => self
                .number
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| "PR".to_string()),
            // "v0.4.15" for v0.4.15-nightly.20261025: the card says the rest.
            Kind::Release => match self.version.split(['-', '+']).next() {
                Some(v) if !v.is_empty() => format!("v{v}"),
                _ => "Release".to_string(),
            },
            Kind::Run => run_name(self).to_string(),
        };
        let state = match self.kind {
            // As the reference words it: how many conflicts, not just that there are some.
            Kind::Pr => match (self.resolving, self.phase, self.conflicts.len()) {
                (true, ..) => "Resolving".to_string(),
                (false, Phase::Conflicted, 1) => "1 conflict".to_string(),
                (false, Phase::Conflicted, n) if n > 1 => format!("{n} conflicts"),
                (false, phase, _) => phase.label().to_string(),
            },
            // What failed, by name; else how far it got ("Building 2 of 3", "2 of 3 passed").
            Kind::Release | Kind::Run => match self.phase {
                Phase::Failed | Phase::Closed
                    if self.stages.iter().any(|s| s.state == CheckState::Fail) =>
                {
                    failed_jobs(self)
                }
                _ => pipeline_state(self),
            },
        };
        (what, state)
    }

    /// One line for surfaces that don't draw the card (the phone's transcript).
    pub fn summary(&self) -> String {
        match self.kind {
            Kind::Pr => format!(
                "Pull request{} · {}",
                self.number.map(|n| format!(" #{n}")).unwrap_or_default(),
                self.phase.label()
            ),
            Kind::Release => format!("Release {} · {}", self.version, self.phase.label()),
            Kind::Run => format!("{} run · {}", run_name(self), self.phase.label()),
        }
    }
}

/// A run card's name: its workflow's, once `gh` said it.
fn run_name(card: &Card) -> &str {
    if card.title.is_empty() {
        "Workflow"
    } else {
        &card.title
    }
}

/// The colour a card's chip says its state in: a verdict's, or none (`None`: the ink's muted
/// grey, for work under way).
fn chip_hue(card: &Card, cx: &App) -> Option<Hue> {
    let h = hues(cx);
    if chip_busy(card) {
        return None;
    }
    match card.phase {
        Phase::Conflicted => Some(h.amber),
        Phase::Ready | Phase::Passed | Phase::Published => Some(h.green),
        Phase::Merged => Some(h.purple),
        Phase::Failed => Some(h.red),
        _ => None,
    }
}

/// The reference's busy mark: a short arc running round a faint ring (not a dotted spinner).
/// Still, as a quarter arc, when motion is reduced.
pub fn ring_spinner(size: f32, color: Hsla, cx: &App) -> AnyElement {
    let motion = !cx.reduce_motion();
    canvas(
        move |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let r = size / 2. - 1.;
            let w = px(1.4);
            // The track.
            let mut track = PathBuilder::stroke(w);
            let steps = 32;
            for i in 0..=steps {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                let p = point(c.x + px(r) * a.cos(), c.y + px(r) * a.sin());
                if i == 0 {
                    track.move_to(p);
                } else {
                    track.line_to(p);
                }
            }
            if let Ok(p) = track.build() {
                window.paint_path(p, color.opacity(0.15));
            }
            // The arc: three quarters of the ring, once round every 0.9 s.
            let turn = if motion {
                (now_ms() % 900) as f32 / 900.
            } else {
                0.
            };
            let mut arc = PathBuilder::stroke(w);
            let from = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            for i in 0..=24 {
                let a = from + i as f32 / 24. * std::f32::consts::TAU * 0.75;
                let p = point(c.x + px(r) * a.cos(), c.y + px(r) * a.sin());
                if i == 0 {
                    arc.move_to(p);
                } else {
                    arc.line_to(p);
                }
            }
            if let Ok(p) = arc.build() {
                window.paint_path(p, color);
            }
            if motion {
                window.request_animation_frame();
            }
        },
    )
    .size(px(size))
    .flex_none()
    .into_any_element()
}

/// The card of `Item::Tool` `id`/`detail`, when the row is one and its JSON reads.
pub fn read(id: &str, detail: &str) -> Option<Card> {
    of_row(id)?;
    let card: Card = serde_json::from_str(detail).ok()?;
    // Release cards an older parser made from commands that only mentioned release.sh
    // ("Release v80,135p", "Published v") stay in old transcripts: a release has a version.
    (card.kind != Kind::Release || is_version(&card.version)).then_some(card)
}

/// The one-line form surfaces without the card show (the phone's transcript).
pub fn summary(detail: &str) -> String {
    serde_json::from_str::<Card>(detail)
        .map(|c| c.summary())
        .unwrap_or_else(|_| "Pull request".to_string())
}

/// `detail` rewritten with `card`.
fn write(detail: &mut String, card: &Card) {
    if let Ok(json) = serde_json::to_string(card) {
        *detail = json;
    }
}

/// Every `trek-pr:` row in `live`: (index, row id, card).
fn cards(live: &crate::workspace::LiveThread) -> Vec<(usize, String, Card)> {
    live.items
        .iter()
        .enumerate()
        .filter_map(|(ix, i)| match i {
            Item::Tool { id, detail, .. } => read(id, detail).map(|c| (ix, id.clone(), c)),
            _ => None,
        })
        .collect()
}

/// The latest card standing for a PR (not a release), if any.
fn latest_pr(live: &crate::workspace::LiveThread) -> Option<(usize, String, Card)> {
    cards(live)
        .into_iter()
        .filter(|(_, _, c)| c.kind == Kind::Pr)
        .next_back()
}

/// The thread's latest pull request card (not a release's or a run's): the title bar's hub
/// names it under Git.
pub fn latest_pull_request(live: &crate::workspace::LiveThread) -> Option<Card> {
    latest_pr(live).map(|(_, _, c)| c)
}

/// The colour `card`'s state word is in when it's a verdict (ready, merged, conflicts, failed),
/// as the composer's chip draws it; `None` while work is under way or the state isn't one.
pub fn verdict_color(card: &Card, cx: &App) -> Option<Hsla> {
    chip_hue(card, cx).map(|h| h.text)
}

/// Until when the composer's chip shows `card` (unix ms): the moment it finished well plus
/// [`Phase::CHIP_LINGER_MS`]. `None` while it's live, and for a failure, which stays until
/// it's dealt with (the next message sent dismisses it).
pub fn chip_until(card: &Card) -> Option<i64> {
    matches!(card.phase, Phase::Merged | Phase::Published | Phase::Passed)
        .then(|| card.phase_at_ms.max(card.created_ms) + Phase::CHIP_LINGER_MS)
}

/// The chip shows work under way: a spinner, and its state word in grey.
pub fn chip_busy(card: &Card) -> bool {
    card.resolving
        || matches!(
            card.phase,
            Phase::Creating | Phase::Checking | Phase::Merging | Phase::Releasing
        )
        || (card.kind == Kind::Run && !card.phase.terminal())
}

/// The composer's status chip, as the reference draws it: a 31 px pill of the page's own colour
/// with a hairline and a faint shadow; a mark (a spinner while it works, a dot while it waits or
/// once it's done, the merge glyph once merged), the name in ink, and the state, in its colour
/// only when it's a verdict.
pub fn status_chip(card: &Card, opacity: f32, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let hue = chip_hue(card, cx);
    let (what, state) = card.chip_parts();
    let mark = if chip_busy(card) {
        ring_spinner(12., theme.muted_foreground, cx)
    } else if card.phase == Phase::Merged {
        Icon::new(crate::assets::Lucide::GitMerge)
            .size(px(12.))
            .text_color(hue.map_or(theme.muted_foreground, |h| h.mark))
            .into_any_element()
    } else {
        div()
            .size(px(6.))
            .rounded_full()
            .bg(hue.map_or(theme.muted_foreground, |h| h.mark))
            .into_any_element()
    };
    h_flex()
        .id("pr-chip")
        .test_support()
        .aria_label(format!("{what} {state}"))
        .flex_none()
        .h(px(31.))
        .px(px(12.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .bg(chip_fill(cx))
        .border_1()
        .border_color(chip_border(cx))
        .shadow(chip_shadow())
        .opacity(opacity)
        .text_size(px(13.))
        .child(mark)
        .child(div().text_color(theme.foreground).child(what))
        .child(
            div()
                .text_color(hue.map_or(theme.muted_foreground, |h| h.text))
                .child(state),
        )
        .into_any_element()
}

/// The chips' hairline (the reference's #EBEBEB on white).
pub fn chip_border(cx: &App) -> Hsla {
    let theme = cx.theme();
    theme
        .foreground
        .opacity(if theme.mode.is_dark() { 0.12 } else { 0.085 })
}

/// The chips' fill: the page's own colour, a touch lifted on a dark page so the pill reads.
pub fn chip_fill(cx: &App) -> Hsla {
    let theme = cx.theme();
    if theme.background.l < 0.5 {
        theme.background.blend(theme.foreground.opacity(0.05))
    } else {
        theme.background
    }
}

/// The chips' shadow: faint and low, as the reference's 0 1 2 rgba(0,0,0,.04).
pub fn chip_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.05),
        offset: point(px(0.), px(1.)),
        blur_radius: px(2.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// Whether the composer's chip shows `card` at `now_ms`.
pub fn chip_shown(card: &Card, now_ms: i64) -> bool {
    chip_until(card).is_none_or(|until| now_ms < until)
}

/// The latest card of the thread, of any kind (the composer chip's).
pub fn latest(live: &crate::workspace::LiveThread) -> Option<(String, Card)> {
    cards(live).pop().map(|(_, key, c)| (key, c))
}

// ---------- command tracking ----------

/// A shell command the transcript watches.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// `gh pr create` (flags read when given: `--title`, `--base`, `--head`).
    PrCreate {
        title: Option<String>,
        base: Option<String>,
        head: Option<String>,
    },
    /// `gh pr merge` — the PR argument when there is one (a number or a url).
    PrMerge(Option<String>),
    /// `gh run watch [<run id>]`: a run card follows the run it names (none named: the PR
    /// card's checks are what it's waiting on, or the id `gh` prints once it's done).
    RunWatch(Option<String>),
    /// `gh pr checks [<pr>]`: the PR card's checks are what it's waiting on.
    PrChecks(Option<String>),
    /// `script/release.sh <version> [--channel …]` — a local release.
    Release { version: String, channel: String },
    /// `gh workflow run release.yml -f version=… -f channel=…`.
    ReleaseWorkflow { version: String, channel: String },
    /// `gh release create <tag>`.
    ReleaseTag { tag: String },
    /// A `git` verb that moves a PR's work along (commit, push, fetch, merge, pull, rebase):
    /// tracked nowhere, it only re-words an open card's step line.
    Git(GitVerb),
}

/// The `git` verb a command line leads with (for `Call::Git`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitVerb {
    Commit,
    Push,
    Fetch,
    Merge,
    Pull,
    Rebase,
}

/// Command lines are split per `&&`/`;`/`|`/newline segment: a `gh` call wins over a `git` one
/// (`git commit && gh pr create` opens the card, it doesn't just re-word its step).
pub fn call(title: &str, detail: &str) -> Option<Call> {
    if crate::activity::tool_kind(title) != crate::activity::ToolKind::Command {
        return None;
    }
    let mut git = None;
    for segment in segments(detail) {
        let words = shell_words(&segment);
        match call_in(&words) {
            Some(Call::Git(_)) => git = git.or_else(|| call_in(&words)),
            Some(c) => return Some(c),
            None => {}
        }
    }
    git
}

/// The segment from its command on: past `VAR=value` assignments and wrappers that run the rest
/// (`env`, `time`…). Only a command in this place is one; a word elsewhere is an argument
/// (`sed -n 1,9p script/release.sh`, `echo gh pr merge`).
fn command(words: &[String]) -> &[String] {
    let assignment = |w: &str| {
        w.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        })
    };
    let start = words
        .iter()
        .position(|w| {
            !assignment(w) && !matches!(w.as_str(), "env" | "time" | "command" | "exec" | "nohup")
        })
        .unwrap_or(words.len());
    &words[start..]
}

fn is_gh(words: &[String], sub: &[&str]) -> bool {
    words.len() > sub.len()
        && words[0] == "gh"
        && words[1..=sub.len()].iter().zip(sub).all(|(w, s)| w == s)
}

/// Whether the segment runs a script named `watch-run` (with or without `.sh`), as the command.
fn watches_run(words: &[String]) -> bool {
    words.first().is_some_and(|cmd| {
        let name = cmd.rsplit('/').next().unwrap_or(cmd);
        matches!(name, "watch-run" | "watch-run.sh")
    })
}

/// The words after `gh <sub…>` (none when it isn't there).
fn after<'a>(words: &'a [String], sub: &[&str]) -> &'a [String] {
    if is_gh(words, sub) {
        &words[sub.len() + 1..]
    } else {
        &[]
    }
}

/// Whether the segment runs a release script: as the command, or handed to a shell that runs it
/// (`zsh script/release.sh 0.4.12`), not one only checking its syntax (`zsh -n`).
fn runs_release_script(words: &[String]) -> bool {
    let Some(cmd) = words.first() else {
        return false;
    };
    if cmd.ends_with("release.sh") {
        return true;
    }
    matches!(cmd.rsplit('/').next(), Some("zsh" | "bash" | "sh"))
        && !words[1..].iter().any(|w| w == "-n")
        && words[1..]
            .iter()
            .find(|w| !w.starts_with('-'))
            .is_some_and(|w| w.ends_with("release.sh"))
}

/// A release version as release.sh takes it: `0.4.12`, `v0.4.12`, `0.5.0-beta.2`.
fn is_version(v: &str) -> bool {
    let v = v.strip_prefix('v').unwrap_or(v);
    let core = v.split_once('-').map_or(v, |(core, _)| core);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn is_number(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| c.is_ascii_digit())
}

/// The PR `gh <sub…>` names, when it names one (a number or a url).
fn pr_arg(words: &[String], sub: &[&str]) -> Option<String> {
    after(words, sub)
        .iter()
        .find(|w| !w.starts_with('-'))
        .filter(|w| w.contains("/pull/") || is_number(w))
        .cloned()
}

/// The run id `gh run watch` prints ("✓ main CI · 39779843418", or a `…/actions/runs/<id>`
/// url), for a watch whose id the command line didn't spell out (`gh run watch "$id"`).
pub fn run_id_in(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        if let Some(i) = l.find("/actions/runs/") {
            let id: String = l[i + 14..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
        let (_, tail) = l.rsplit_once(" · ")?;
        let id = tail.trim();
        (id.len() >= 5 && is_number(id)).then(|| id.to_string())
    })
}

fn flag(words: &[String], names: &[&str]) -> Option<String> {
    for (i, w) in words.iter().enumerate() {
        for n in names {
            if *w == *n {
                return words.get(i + 1).cloned();
            }
            if let Some(v) = w.strip_prefix(&format!("{n}=")) {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Positional arguments of `words` (flags and their values skipped).
fn positional(words: &[String]) -> Vec<String> {
    const TAKES_VALUE: &[&str] = &[
        "--title",
        "-t",
        "--body",
        "-b",
        "--base",
        "-B",
        "--head",
        "-H",
        "--repo",
        "-R",
        "--field",
        "-f",
        "--channel",
        "--notes",
        "--ref",
        "--json",
        "--workflow",
        "--limit",
        "--message",
        "-m",
        "--tag",
        "--target",
        "--subject",
    ];
    let mut out = vec![];
    let mut skip = false;
    for w in words {
        if skip {
            skip = false;
            continue;
        }
        if w.starts_with('-') {
            skip = !w.contains('=') && TAKES_VALUE.contains(&w.as_str());
            continue;
        }
        out.push(w.clone());
    }
    out
}

fn call_in(words: &[String]) -> Option<Call> {
    let words = command(words);
    if runs_release_script(words) {
        // A dry run builds and stops before git and GitHub, and `--help` only prints usage:
        // nothing is published, so there's no release to follow.
        if words
            .iter()
            .any(|w| matches!(w.as_str(), "--dry-run" | "--help" | "-h"))
        {
            return None;
        }
        let pos = positional(words);
        return Some(Call::Release {
            version: pos
                .iter()
                .find(|v| is_version(v))
                .map(|v| v.trim_start_matches('v').to_string())
                .unwrap_or_default(),
            channel: flag(words, &["--channel"]).unwrap_or_else(|| "stable".into()),
        });
    }
    if is_gh(words, &["pr", "create"]) {
        return Some(Call::PrCreate {
            title: flag(words, &["--title", "-t"]),
            base: flag(words, &["--base", "-B"]),
            head: flag(words, &["--head", "-H"]),
        });
    }
    if is_gh(words, &["pr", "merge"]) {
        return Some(Call::PrMerge(pr_arg(words, &["pr", "merge"])));
    }
    // A run-watching script named for it (`~/.claude/bin/watch-run.sh <run id> [minutes]`, which
    // ends at the first failed job where `gh run watch` waits for them all): a watch too.
    if watches_run(words) {
        return Some(Call::RunWatch(
            positional(words).get(1).filter(|w| is_number(w)).cloned(),
        ));
    }
    if is_gh(words, &["run", "watch"]) {
        let run = after(words, &["run", "watch"])
            .iter()
            .find(|w| !w.starts_with('-'))
            .filter(|w| is_number(w))
            .cloned();
        return Some(Call::RunWatch(run));
    }
    if is_gh(words, &["pr", "checks"]) {
        return Some(Call::PrChecks(pr_arg(words, &["pr", "checks"])));
    }
    if is_gh(words, &["workflow", "run"]) {
        if positional(words).iter().any(|w| w.contains("release")) {
            let mut version = String::new();
            let mut channel = String::new();
            let mut dry_run = false;
            for (i, w) in words.iter().enumerate() {
                let field = if matches!(w.as_str(), "-f" | "--field") {
                    words.get(i + 1).cloned()
                } else {
                    w.strip_prefix("--field=")
                        .or_else(|| w.strip_prefix("-f="))
                        .map(String::from)
                };
                if let Some(f) = field {
                    if let Some(v) = f.strip_prefix("version=") {
                        version = v.to_string();
                    } else if let Some(v) = f.strip_prefix("channel=") {
                        channel = v.to_string();
                    } else if let Some(v) = f.strip_prefix("dry_run=") {
                        dry_run = v == "true";
                    }
                }
            }
            if dry_run {
                return None;
            }
            return Some(Call::ReleaseWorkflow { version, channel });
        }
        return None;
    }
    if is_gh(words, &["release", "create"]) {
        let pos = positional(words);
        return Some(Call::ReleaseTag {
            tag: pos.last().cloned().unwrap_or_default(),
        });
    }
    // Plain `git` work the card narrates while it's open (a `gh` call elsewhere in the line wins).
    // `git`'s subcommand: its first word past the options (`-C dir` and `-c key=value` take one).
    let sub = (words.first().map(String::as_str) == Some("git"))
        .then(|| {
            let mut rest = words[1..].iter();
            while let Some(w) = rest.next() {
                if matches!(w.as_str(), "-C" | "-c") {
                    rest.next();
                } else if !w.starts_with('-') {
                    return Some(w);
                }
            }
            None
        })
        .flatten();
    let verb = match sub {
        Some(w) if w == "commit" => Some(GitVerb::Commit),
        Some(w) if w == "push" => Some(GitVerb::Push),
        Some(w) if w == "fetch" => Some(GitVerb::Fetch),
        Some(w) if w == "merge" => Some(GitVerb::Merge),
        Some(w) if w == "pull" => Some(GitVerb::Pull),
        Some(w) if w == "rebase" => Some(GitVerb::Rebase),
        _ => None,
    };
    verb.map(Call::Git)
}

/// The command line cut into its commands: at `&`, `;`, `|` and line ends, but not inside quotes
/// (`git commit -m "fix; gh pr merge 5"` is one command). A backslash before a line end joins the
/// lines, and the body of a heredoc (`<<EOF` … `EOF`) is text, not commands.
fn segments(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    // Heredoc terminators waiting for the end of the line that named them.
    let mut heredocs: Vec<(String, bool)> = vec![];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    cur.extend(chars.next());
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '\\' => match chars.next() {
                    Some('\n') => cur.push(' '),
                    Some(next) => {
                        cur.push(c);
                        cur.push(next);
                    }
                    None => cur.push(c),
                },
                '<' if chars.peek() == Some(&'<') => {
                    chars.next();
                    if chars.peek() == Some(&'<') {
                        // `<<<`: a here-string, no body.
                        chars.next();
                        cur.push_str("<<<");
                        continue;
                    }
                    let strip = chars.peek() == Some(&'-');
                    if strip {
                        chars.next();
                    }
                    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                        chars.next();
                    }
                    let mut word = String::new();
                    while let Some(&w) = chars.peek() {
                        if w.is_whitespace() || matches!(w, ';' | '&' | '|' | '<' | '>' | ')') {
                            break;
                        }
                        chars.next();
                        if !matches!(w, '"' | '\'' | '\\') {
                            word.push(w);
                        }
                    }
                    if !word.is_empty() {
                        heredocs.push((word, strip));
                    }
                    cur.push_str("<<");
                }
                '&' | ';' | '|' => out.push(std::mem::take(&mut cur)),
                '\n' => {
                    out.push(std::mem::take(&mut cur));
                    // The bodies of the heredocs this line opened, up to their terminators.
                    for (word, strip) in heredocs.drain(..) {
                        let mut line = String::new();
                        loop {
                            line.clear();
                            let mut ended = true;
                            for c in chars.by_ref() {
                                if c == '\n' {
                                    ended = false;
                                    break;
                                }
                                line.push(c);
                            }
                            let body = if strip {
                                line.trim_start_matches('\t')
                            } else {
                                &line
                            };
                            if body == word || ended {
                                break;
                            }
                        }
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out.retain(|s| !s.trim().is_empty());
    out
}

/// `words` of a shell segment: whitespace-split, single and double quotes honoured.
fn shell_words(segment: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in segment.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A `…/<owner>/<repo>/pull/<n>` url in `output`: (url, number, repo).
pub fn pr_url(output: &str) -> Option<(String, u64, String)> {
    let ix = output.find("/pull/")?;
    let start = output[..ix]
        .rfind(|c: char| c.is_whitespace() || c == '"')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = output[ix..]
        .find(|c: char| c.is_whitespace() || c == '"')
        .map(|i| ix + i)
        .unwrap_or(output.len());
    let url = output[start..end].trim_end_matches(['.', ',', ')']);
    let number: u64 = url.rsplit('/').next()?.parse().ok()?;
    let repo = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .and_then(|r| r.split("/pull/").next())
        .map(String::from);
    Some((url.to_string(), number, repo.unwrap_or_default()))
}

/// `card`'s phase moves to `phase`, marking when for the draw-in (`phase_at_ms`).
fn enter_phase(card: &mut Card, phase: Phase) {
    if card.phase != phase {
        card.phase = phase;
        card.phase_at_ms = now_ms();
    }
}

/// Whether a `gh` watcher can ever finish `card`: a PR that exists, a run it names, a release
/// workflow. A release a local script or `gh release create` made is only ever finished by its
/// own command, which a card read back from the store has lost.
pub fn followable(card: &Card) -> bool {
    match card.kind {
        Kind::Pr => card.phase != Phase::Creating,
        Kind::Run => card.number.is_some(),
        Kind::Release => is_workflow(card),
    }
}

/// The workflow run a failed card can re-run: a run card's own, a release workflow's.
pub fn rerun_id(card: &Card) -> Option<u64> {
    if card.phase != Phase::Failed {
        return None;
    }
    match card.kind {
        Kind::Run => card.number,
        Kind::Release if is_workflow(card) => card.run_id,
        _ => None,
    }
}

/// `card` re-run (`gh run rerun --failed`): its failed and skipped jobs queue again and it's
/// under way, to be followed, until the run says otherwise.
pub fn rerun_started(card: &mut Card) {
    enter_phase(
        card,
        if card.kind == Kind::Release {
            Phase::Releasing
        } else {
            Phase::Checking
        },
    );
    card.step = "Re-running the failed jobs".into();
    card.ended_ms = 0;
    for s in &mut card.stages {
        if matches!(s.state, CheckState::Fail | CheckState::Skipped) {
            *s = Stage {
                label: std::mem::take(&mut s.label),
                ..Default::default()
            };
        }
    }
}

/// A published release's page on GitHub (its notes): the tag, beside the run that made it.
pub fn notes_url(card: &Card) -> Option<String> {
    if card.kind != Kind::Release || card.phase != Phase::Published || card.version.is_empty() {
        return None;
    }
    let url = card.url.as_deref()?;
    if url.contains("/releases/") {
        return Some(url.to_string());
    }
    let (repo, _) = url.split_once("/actions/")?;
    repo.starts_with("https://github.com/")
        .then(|| format!("{repo}/releases/tag/v{}", card.version))
}

/// A card that is working but has nothing left to finish it (its command is gone with the old
/// session, or its watcher gave up or can't start) stops: spinners and the animation loop only
/// belong to a card something is following. `true` when it changed.
pub fn settle(card: &mut Card) -> bool {
    if !matches!(
        card.phase,
        Phase::Creating | Phase::Checking | Phase::Merging | Phase::Releasing
    ) {
        return false;
    }
    match card.kind {
        // The pull request is still there; Trek just isn't looking at it any more.
        Kind::Pr if card.phase != Phase::Creating => {
            card.merging = false;
            enter_phase(card, Phase::Open);
            card.step = "Not following this pull request".into();
        }
        kind => {
            enter_phase(card, Phase::Failed);
            card.step = match kind {
                Kind::Pr => "Lost track of this pull request",
                Kind::Run => "Lost track of this run",
                Kind::Release => "Lost track of this release",
            }
            .into();
            for s in &mut card.stages {
                if s.state.pending() {
                    s.state = CheckState::Skipped;
                }
            }
        }
    }
    true
}

/// What `tool_started`/`tool_finished` ask the workspace to do (their `live` edits are done).
#[derive(Debug, Default)]
pub struct Effects {
    /// Card rows to (re)start `gh` watchers for, by `Item::Tool.id`.
    pub watch: Vec<String>,
    /// Transcript item ids whose card's height may have changed (remeasure).
    pub changed: Vec<String>,
}

/// Where a command's `gh` call runs: `cwd`, as moved by the `cd`s (`pushd`, `Set-Location`)
/// the command line runs before it (`cd ~/Trek && gh run watch 123`). Asked in `cwd` itself,
/// which may be another repository or none (a chat outside any project has a folder of its
/// own), `gh` found no run and its card stayed at "Starting".
fn command_dir(detail: &str, cwd: &Path) -> PathBuf {
    let mut dir = cwd.to_path_buf();
    for segment in segments(detail) {
        let words = shell_words(&segment);
        if matches!(call_in(&words), Some(c) if !matches!(c, Call::Git(_))) {
            break;
        }
        let words = command(&words);
        let moves = words.first().is_some_and(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "cd" | "pushd" | "chdir" | "set-location" | "sl"
            )
        });
        if !moves {
            continue;
        }
        if let Some(to) = words[1..].iter().find(|w| !w.starts_with('-')) {
            dir = moved_to(&dir, to);
        }
    }
    dir
}

/// `to`, a `cd`'s argument, from `dir`: `~` is home, `/c/…` (Git Bash's form) is `C:\…` on
/// Windows, and a relative one is under `dir`.
fn moved_to(dir: &Path, to: &str) -> PathBuf {
    if to == "~" {
        return trek_core::paths::home();
    }
    if let Some(rest) = to.strip_prefix("~/").or_else(|| to.strip_prefix("~\\")) {
        return trek_core::paths::home().join(rest);
    }
    if cfg!(windows) {
        let b = to.as_bytes();
        if b.len() >= 2
            && b[0] == b'/'
            && b[1].is_ascii_alphabetic()
            && (b.len() == 2 || b[2] == b'/')
        {
            let rest = to.get(3..).unwrap_or("").replace('/', "\\");
            return PathBuf::from(format!("{}:\\{rest}", (b[1] as char).to_ascii_uppercase()));
        }
    }
    dir.join(to)
}

/// A command call started: surface or move the card it speaks for. `cwd`: where its `gh` runs
/// unless the command line moves it (`command_dir`).
pub fn tool_started(
    live: &mut crate::workspace::LiveThread,
    call_id: &str,
    title: &str,
    detail: &str,
    cwd: Option<&Path>,
) -> Effects {
    let mut fx = Effects::default();
    let Some(call) = call(title, detail) else {
        return fx;
    };
    let dir = command_dir(detail, cwd.unwrap_or_else(|| Path::new("")));
    match call {
        Call::PrCreate { title, base, head } => {
            let key = format!("creating-{call_id}");
            let card = Card {
                kind: Kind::Pr,
                title: title.unwrap_or_else(|| "Pull request".into()),
                base: base.unwrap_or_else(|| "main".into()),
                head: head.unwrap_or_default(),
                repo_dir: dir.to_string_lossy().into_owned(),
                phase: Phase::Creating,
                step: "Opening the pull request".into(),
                created_ms: now_ms(),
                ..Default::default()
            };
            let row = push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row);
        }
        Call::PrMerge(pr) => {
            // The card of the PR it names, else the thread's latest: it goes to Merging.
            let target = match pr.as_deref() {
                Some(p) => cards(live).into_iter().find(|(_, _, c)| {
                    c.url.as_deref() == Some(p) || c.number.is_some_and(|n| n.to_string() == p)
                }),
                None => latest_pr(live),
            };
            if let Some((ix, id, mut c)) = target {
                c.merging = true;
                enter_phase(&mut c, Phase::Merging);
                c.step = "Merging the pull request".into();
                rewrite(live, ix, &c, &mut fx);
                live.pr_calls.insert(call_id.to_string(), id.clone());
                fx.watch.push(id);
            }
        }
        Call::RunWatch(Some(run)) => watch_run(live, call_id, &run, &dir, &mut fx),
        Call::RunWatch(None) => {
            // No id on the line (`gh run watch "$id"`): the PR card's checks, else the id `gh`
            // prints says which run once the call is over.
            if !watch_checks(live, None, &mut fx) {
                live.pr_calls
                    .insert(call_id.to_string(), row_id(RUN_UNKNOWN));
            }
        }
        Call::PrChecks(pr) => {
            if !watch_checks(live, pr.as_deref(), &mut fx) {
                // No card for it yet: one for the PR it names (the branch's, named none), filled
                // in by `gh pr view`.
                let key = match pr.as_deref() {
                    Some(p) => format!("checks-{p}"),
                    None => format!("checks-{call_id}"),
                };
                let card = Card {
                    kind: Kind::Pr,
                    title: "Pull request".into(),
                    number: pr.as_deref().and_then(|p| p.parse().ok()),
                    url: pr.filter(|p| p.contains("/pull/")),
                    repo_dir: dir.to_string_lossy().into_owned(),
                    phase: Phase::Checking,
                    step: "Watching checks".into(),
                    created_ms: now_ms(),
                    ..Default::default()
                };
                let row = push(live, &key, &card, &mut fx);
                fx.watch.push(row);
            }
        }
        Call::ReleaseWorkflow { version, channel } => {
            let key = format!(
                "release-{}",
                if version.is_empty() {
                    call_id
                } else {
                    &version
                }
            );
            let mut card = release_card(
                &version,
                &channel,
                "release.yml",
                "Release workflow running",
                &dir,
            );
            card.workflow = true;
            let row = push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row.clone());
            fx.watch.push(row);
        }
        Call::Release { version, channel } => {
            let key = format!(
                "release-{}",
                if version.is_empty() {
                    call_id
                } else {
                    &version
                }
            );
            let card = release_card(
                &version,
                &channel,
                "release.sh",
                "Building and publishing",
                &dir,
            );
            let row = push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row);
        }
        Call::ReleaseTag { tag } => {
            let key = format!("release-{}", if tag.is_empty() { call_id } else { &tag });
            let mut card = release_card(
                tag.trim_start_matches('v'),
                "",
                "gh release",
                "Publishing the release",
                &dir,
            );
            card.title = format!("Release {tag}");
            let row = push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row);
        }
        Call::Git(verb) => {
            // The thread's open PR card narrates the step ("Committing the last changes").
            if let Some((ix, _, mut c)) = latest_pr(live) {
                if !c.phase.terminal() {
                    c.step = match verb {
                        GitVerb::Commit => "Committing the last changes".into(),
                        GitVerb::Push => match c.commits {
                            Some(1) => "Pushing 1 commit to origin".into(),
                            Some(n) => format!("Pushing {n} commits to origin"),
                            None => "Pushing to origin".into(),
                        },
                        GitVerb::Fetch => {
                            if c.base.is_empty() {
                                "Fetching the latest".into()
                            } else {
                                format!("Fetching {}", c.base)
                            }
                        }
                        GitVerb::Merge => {
                            if c.base.is_empty() {
                                "Merging the branches".into()
                            } else {
                                format!("Merging {} into the branch", c.base)
                            }
                        }
                        GitVerb::Pull => "Updating the branch".into(),
                        GitVerb::Rebase => "Rebasing the branch".into(),
                    };
                    rewrite(live, ix, &c, &mut fx);
                }
            }
        }
    }
    fx
}

/// `pr_calls`' mark for a `gh run watch` whose run is only known from its output.
const RUN_UNKNOWN: &str = "run-?";

/// The PR card `pr` names (none: the thread's latest) goes to Checking and is followed;
/// `false` when there's no such card still open.
fn watch_checks(
    live: &mut crate::workspace::LiveThread,
    pr: Option<&str>,
    fx: &mut Effects,
) -> bool {
    let target = match pr {
        Some(p) => cards(live).into_iter().find(|(_, _, c)| {
            c.kind == Kind::Pr
                && (c.url.as_deref() == Some(p) || c.number.is_some_and(|n| n.to_string() == p))
        }),
        None => latest_pr(live),
    };
    let Some((ix, id, mut c)) = target.filter(|(_, _, c)| !c.phase.terminal()) else {
        return false;
    };
    enter_phase(&mut c, Phase::Checking);
    c.step = "Watching checks".into();
    rewrite(live, ix, &c, fx);
    fx.watch.push(id);
    true
}

/// `gh run watch <run>`: the run's card (made now, or one the thread has for it, back to
/// watching) is followed until the run is over, whatever state it's in already.
fn watch_run(
    live: &mut crate::workspace::LiveThread,
    call_id: &str,
    run: &str,
    dir: &Path,
    fx: &mut Effects,
) {
    let key = format!("run-{run}");
    let row = row_id(&key);
    let existing = cards(live).into_iter().find(|(_, id, _)| *id == row);
    match existing {
        Some((ix, _, mut c)) => {
            enter_phase(&mut c, Phase::Checking);
            c.step = "Watching the run".into();
            rewrite(live, ix, &c, fx);
        }
        None => {
            let card = Card {
                kind: Kind::Run,
                number: run.parse().ok(),
                repo_dir: dir.to_string_lossy().into_owned(),
                phase: Phase::Checking,
                step: "Watching the run".into(),
                created_ms: now_ms(),
                ..Default::default()
            };
            push(live, &key, &card, fx);
        }
    }
    live.pr_calls.insert(call_id.to_string(), row.clone());
    fx.watch.push(row);
}

fn release_card(version: &str, channel: &str, stage: &str, step: &str, dir: &Path) -> Card {
    Card {
        kind: Kind::Release,
        title: if version.is_empty() {
            "Release".into()
        } else {
            format!("Release v{version}")
        },
        version: version.to_string(),
        channel: channel.to_string(),
        repo_dir: dir.to_string_lossy().into_owned(),
        phase: Phase::Releasing,
        step: step.into(),
        stages: vec![Stage {
            label: stage.into(),
            state: CheckState::Running,
            started_ms: now_ms(),
            ..Default::default()
        }],
        created_ms: now_ms(),
        ..Default::default()
    }
}

/// A tracked command call ended: `gh pr create`'s output names the PR it made, `gh run
/// watch`'s the run it watched. `cwd`: where its `gh` ran.
pub fn tool_finished(
    live: &mut crate::workspace::LiveThread,
    call_id: &str,
    ok: bool,
    output: &str,
    cwd: Option<&Path>,
) -> Effects {
    let mut fx = Effects::default();
    let Some(row) = live.pr_calls.remove(call_id) else {
        return fx;
    };
    if row == row_id(RUN_UNKNOWN) {
        if let Some(run) = run_id_in(output) {
            let dir = cwd.unwrap_or_else(|| Path::new(""));
            watch_run(live, call_id, &run, dir, &mut fx);
            live.pr_calls.remove(call_id);
        }
        return fx;
    }
    let Some(ix) = live
        .items
        .iter()
        .position(|i| matches!(i, Item::Tool { id, .. } if *id == row))
    else {
        return fx;
    };
    let Some(mut card) = live.items.get(ix).and_then(|i| match i {
        Item::Tool { id, detail, .. } => read(id, detail),
        _ => None,
    }) else {
        return fx;
    };
    match card.kind {
        // `gh` says how the run went (the call's own status is its pipe's, `| tail`'s).
        Kind::Run => {
            fx.watch.push(row);
            return fx;
        }
        Kind::Pr if card.phase == Phase::Creating => {
            if ok {
                if let Some((url, number, repo)) = output.lines().find_map(pr_url) {
                    card.url = Some(url);
                    card.number = Some(number);
                    if !repo.is_empty() {
                        card.repo = Some(repo);
                    }
                }
                enter_phase(&mut card, Phase::Open);
                card.step = "Fetching main".into();
                // The row keyed "creating-…" gets the PR's own key now that it's known.
                if let Some(url) = card.url.clone() {
                    let new_id = row_id(&url);
                    if let Some(Item::Tool { id, .. }) = live.items.get_mut(ix) {
                        *id = new_id.clone();
                    }
                    fx.watch.push(new_id);
                } else {
                    fx.watch.push(row);
                }
            } else {
                enter_phase(&mut card, Phase::Failed);
                card.step = output
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().chars().take(90).collect())
                    .unwrap_or_else(|| "Couldn't create the pull request".into());
            }
        }
        Kind::Pr => {
            card.merging = !ok;
            if ok {
                enter_phase(&mut card, Phase::Merging);
                fx.watch.push(row);
            } else {
                card.step = output
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| format!("Merge failed: {}", l.trim()))
                    .unwrap_or_else(|| "Merge failed".into());
                enter_phase(&mut card, Phase::Open);
            }
        }
        // `gh workflow run` ends once the run is queued: the run says how the release went.
        Kind::Release if ok && is_workflow(&card) => {
            fx.watch.push(row);
            return fx;
        }
        Kind::Release => {
            enter_phase(&mut card, if ok { Phase::Published } else { Phase::Failed });
            card.step = if ok {
                "Published".into()
            } else {
                output
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().chars().take(90).collect())
                    .unwrap_or_else(|| "Release failed".into())
            };
            let state = if ok {
                CheckState::Pass
            } else {
                CheckState::Fail
            };
            for s in &mut card.stages {
                s.state = state;
            }
        }
    }
    rewrite(live, ix, &card, &mut fx);
    fx
}

/// Append `card` as a `trek-pr:` row, and say which id it got: the key's own, unless a row has
/// it already (a release run again, `gh pr checks` on a PR it already carded), which would
/// leave every later look-up by id on the old row and the new card never finished.
fn push(
    live: &mut crate::workspace::LiveThread,
    key: &str,
    card: &Card,
    fx: &mut Effects,
) -> String {
    live.streaming = None;
    let taken = |row: &str| {
        live.items
            .iter()
            .any(|i| matches!(i, Item::Tool { id, .. } if id == row))
    };
    let mut row = row_id(key);
    let mut n = 1;
    while taken(&row) {
        n += 1;
        row = row_id(&format!("{key}~{n}"));
    }
    let ix = live.items.push(Item::Tool {
        id: row.clone(),
        title: match card.kind {
            Kind::Pr => "Pull request".into(),
            Kind::Release => "Release".into(),
            Kind::Run => "Workflow run".into(),
        },
        detail: serde_json::to_string(card).unwrap_or_default(),
        output: String::new(),
        status: ToolStatus::Done,
    });
    if let Some(item) = live.items.id_at(ix) {
        fx.changed.push(item.to_string());
    }
    row
}

/// Write `card` back to the item at `ix` (its transcript id reported for remeasure).
fn rewrite(live: &mut crate::workspace::LiveThread, ix: usize, card: &Card, fx: &mut Effects) {
    if let Some(Item::Tool { detail, .. }) = live.items.get_mut(ix) {
        write(detail, card);
    }
    if let Some(item) = live.items.id_at(ix) {
        fx.changed.push(item.to_string());
    }
}

/// The card at `ix`, rewritten by `f`; `true` when there was one.
pub fn edit(live: &mut crate::workspace::LiveThread, ix: usize, f: impl FnOnce(&mut Card)) -> bool {
    let Some(card) = live.items.get(ix).and_then(|i| match i {
        Item::Tool { id, detail, .. } => read(id, detail),
        _ => None,
    }) else {
        return false;
    };
    let mut card = card;
    f(&mut card);
    if let Some(Item::Tool { detail, .. }) = live.items.get_mut(ix) {
        write(detail, &card);
    }
    true
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- gh ----------

/// What `gh`/`git` say for `card`, off the main thread (`None`: no answer, the card keeps
/// what it had).
pub fn fetch(card: &Card) -> Option<Card> {
    let dir = PathBuf::from(&card.repo_dir);
    match card.kind {
        Kind::Pr => fetch_pr(card, &dir),
        Kind::Release => fetch_release(card, &dir),
        Kind::Run => fetch_run(card, &dir),
    }
}

/// A job of `gh run view --json jobs` (its states lowercase: `completed`, `success`).
#[derive(Deserialize, Default)]
struct JobJson {
    name: String,
    status: String,
    conclusion: String,
    #[serde(default, rename = "startedAt")]
    started: String,
    #[serde(default, rename = "completedAt")]
    completed: String,
}

/// An RFC 3339 time as unix ms; 0 for none (GitHub's `0001-01-01T00:00:00Z` included).
fn unix_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis().max(0))
        .unwrap_or(0)
}

/// Jobs as stage chips.
fn job_stages(jobs: &[JobJson]) -> Vec<Stage> {
    jobs.iter()
        .filter_map(|j| {
            check(&CheckJson {
                name: j.name.clone(),
                status: j.status.to_uppercase(),
                conclusion: j.conclusion.to_uppercase(),
                ..Default::default()
            })
            .map(|c| Stage {
                label: c.name,
                state: c.state,
                started_ms: unix_ms(&j.started),
                ended_ms: if c.state.pending() {
                    0
                } else {
                    unix_ms(&j.completed)
                },
            })
        })
        .collect()
}

/// The run `card.number` names: its workflow, jobs and how it ended.
fn fetch_run(card: &Card, dir: &Path) -> Option<Card> {
    let run = card.number?.to_string();
    let out = gh(dir)?
        .args([
            "run",
            "view",
            &run,
            "--json",
            "workflowName,displayTitle,headBranch,event,status,conclusion,url,jobs,startedAt,updatedAt",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    #[derive(Deserialize)]
    struct RunJson {
        #[serde(rename = "workflowName")]
        workflow: String,
        #[serde(rename = "displayTitle")]
        subject: String,
        #[serde(rename = "headBranch")]
        branch: String,
        #[serde(default)]
        event: String,
        status: String,
        conclusion: String,
        url: String,
        jobs: Vec<JobJson>,
        #[serde(rename = "startedAt")]
        started: String,
        #[serde(rename = "updatedAt")]
        updated: String,
    }
    let view: RunJson = serde_json::from_slice(&out.stdout).ok()?;
    let mut next = run_card(
        card,
        view.workflow,
        view.subject,
        view.branch,
        &view.status,
        &view.conclusion,
        view.url,
        &view.jobs,
    );
    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.timestamp_millis())
            .unwrap_or(0)
    };
    let (started, ended) = (at(&view.started), at(&view.updated));
    next.event = view.event;
    if started > 0 {
        next.started_ms = started;
    }
    next.ended_ms = if next.phase.terminal() {
        ended.max(0)
    } else {
        0
    };
    Some(next)
}

/// `card` as `gh run view` reports its run.
#[allow(clippy::too_many_arguments)]
fn run_card(
    card: &Card,
    workflow: String,
    subject: String,
    branch: String,
    status: &str,
    conclusion: &str,
    url: String,
    jobs: &[JobJson],
) -> Card {
    let mut card = card.clone();
    if !workflow.is_empty() {
        card.title = workflow;
    }
    card.subject = subject;
    card.head = branch;
    if !url.is_empty() {
        card.url = Some(url);
    }
    card.stages = job_stages(jobs);
    let failed: Vec<&str> = card
        .stages
        .iter()
        .filter(|s| s.state == CheckState::Fail)
        .map(|s| s.label.as_str())
        .collect();
    let (phase, step) = match (status, conclusion) {
        ("completed", "success" | "neutral" | "skipped") => (Phase::Passed, "Passed".into()),
        ("completed", "cancelled") => (Phase::Failed, "Cancelled".into()),
        ("completed", _) if failed.is_empty() => (Phase::Failed, "Run failed".into()),
        ("completed", _) => (Phase::Failed, format!("Failed: {}", failed.join(", "))),
        _ => {
            let pending = card.stages.iter().filter(|s| s.state.pending()).count();
            let step = match pending {
                0 => "Waiting for jobs".into(),
                1 => "Watching 1 job".into(),
                n => format!("Watching {n} jobs"),
            };
            (Phase::Checking, step)
        }
    };
    enter_phase(&mut card, phase);
    card.step = step;
    card
}

/// `gh`, found on the login PATH and run in `dir` (the Share sheet's gists use it too).
pub(crate) fn gh(dir: &Path) -> Option<std::process::Command> {
    let gh = trek_core::detect::which("gh")?;
    let mut c = trek_core::procs::command(gh);
    c.env("PATH", trek_core::detect::login_path())
        .current_dir(dir);
    Some(c)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PrView {
    number: u64,
    title: String,
    url: String,
    state: String,
    mergeable: String,
    #[serde(rename = "mergeStateStatus")]
    merge_state: String,
    #[serde(rename = "headRefName")]
    head: String,
    #[serde(rename = "baseRefName")]
    base: String,
    additions: u64,
    deletions: u64,
    commits: Vec<PrCommitJson>,
    #[serde(rename = "statusCheckRollup")]
    checks: Vec<CheckJson>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PrCommitJson {
    #[serde(rename = "messageHeadline")]
    title: String,
    oid: String,
    authors: Vec<PrAuthorJson>,
    #[serde(rename = "committedDate")]
    committed: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PrAuthorJson {
    login: String,
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CheckJson {
    name: String,
    context: String,
    status: String,
    state: String,
    conclusion: String,
}

const PR_FIELDS: &str = "number,title,url,state,mergeable,mergeStateStatus,headRefName,baseRefName,additions,deletions,statusCheckRollup,commits";

fn check(j: &CheckJson) -> Option<Check> {
    let name = if !j.name.is_empty() {
        j.name.clone()
    } else if !j.context.is_empty() {
        j.context.clone()
    } else {
        return None;
    };
    let state = match j.state.as_str() {
        // StatusContext (a commit status, not a check run).
        "SUCCESS" => CheckState::Pass,
        "FAILURE" | "ERROR" => CheckState::Fail,
        "PENDING" | "EXPECTED" => CheckState::Running,
        _ => match (j.status.as_str(), j.conclusion.as_str()) {
            ("COMPLETED", "SUCCESS" | "" | "NEUTRAL") => CheckState::Pass,
            ("COMPLETED", "SKIPPED" | "CANCELLED") => CheckState::Skipped,
            ("COMPLETED", _) => CheckState::Fail,
            ("QUEUED" | "PENDING" | "WAITING" | "REQUESTED", _) => CheckState::Queued,
            _ => CheckState::Running,
        },
    };
    Some(Check { name, state })
}

fn fetch_pr(card: &Card, dir: &Path) -> Option<Card> {
    // Neither known yet (`gh pr checks` with no PR named): the branch's own PR.
    let pr = card
        .url
        .clone()
        .or_else(|| card.number.map(|n| n.to_string()));
    let out = gh(dir)?
        .args(["pr", "view"])
        .args(pr.as_slice())
        .args(["--json", PR_FIELDS])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let view: PrView = serde_json::from_slice(&out.stdout).ok()?;
    let mut card = pr_from_view(card, view);
    // The conflicting files' names, once GitHub says there are some and a trial merge can tell
    // them apart (best effort: its failure only loses the chips). While it resolves, the list is
    // asked again each pass so the card counts down, remembering what already cleared.
    if card.phase == Phase::Conflicted && (card.conflicts.is_empty() || card.resolving) {
        let next = conflicting_files(&card, dir);
        apply_conflicts(&mut card, next);
    }
    Some(card)
}

/// `card` as `gh pr view` reports its pull request.
fn pr_from_view(card: &Card, view: PrView) -> Card {
    let mut card = card.clone();
    if card.number.is_none() && view.number > 0 {
        card.number = Some(view.number);
    }
    if card.url.is_none() && !view.url.is_empty() {
        card.url = Some(view.url.clone());
    }
    if card.repo.is_none() {
        card.repo = view
            .url
            .strip_prefix("https://github.com/")
            .and_then(|r| r.split("/pull/").next())
            .map(String::from);
    }
    if !view.title.is_empty() {
        card.title = view.title;
    }
    card.head = view.head;
    card.base = view.base;
    card.commits = Some(view.commits.len() as u32);
    card.history = view
        .commits
        .iter()
        .rev()
        .take(6)
        .map(|commit| Commit {
            title: commit.title.clone(),
            oid: commit.oid.chars().take(7).collect(),
            author: commit
                .authors
                .first()
                .map(|author| {
                    if author.login.is_empty() {
                        author.name.clone()
                    } else {
                        author.login.clone()
                    }
                })
                .unwrap_or_default(),
            committed_ms: chrono::DateTime::parse_from_rfc3339(&commit.committed)
                .map(|date| date.timestamp_millis())
                .unwrap_or(0),
        })
        .collect();
    card.added = Some(view.additions as u32);
    card.removed = Some(view.deletions as u32);
    card.checks = view.checks.iter().filter_map(check).collect();
    let pending = card.checks.iter().filter(|c| c.state.pending()).count();
    let failing: Vec<String> = card
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Fail)
        .map(|c| c.name.clone())
        .collect();
    let phase = match view.state.as_str() {
        "MERGED" => Phase::Merged,
        "CLOSED" => Phase::Closed,
        _ if card.merging => Phase::Merging,
        _ => match view.mergeable.as_str() {
            "CONFLICTING" => Phase::Conflicted,
            "MERGEABLE" if pending > 0 => Phase::Checking,
            // A red PR isn't ready to merge: it stays open, its step line naming the failures.
            "MERGEABLE" if !failing.is_empty() => Phase::Open,
            "MERGEABLE" => Phase::Ready,
            _ => Phase::Checking,
        },
    };
    if !card.merging {
        card.resolving = card.resolving && view.mergeable == "CONFLICTING";
    }
    enter_phase(&mut card, phase);
    card.step = match card.phase {
        Phase::Checking if pending > 0 => format!(
            "Watching {pending} {}",
            if pending == 1 { "check" } else { "checks" }
        ),
        Phase::Checking => "Checking it merges cleanly".into(),
        Phase::Conflicted => match card.conflicts.len() {
            0 => format!("Conflicts with {}", card.base),
            n if card.resolving => format!(
                "Resolving {n} {}",
                if n == 1 { "conflict" } else { "conflicts" }
            ),
            n => format!(
                "{n} {} with {}",
                if n == 1 { "conflict" } else { "conflicts" },
                card.base
            ),
        },
        Phase::Open if !failing.is_empty() => format!(
            "{} of {} checks failing: {}",
            failing.len(),
            card.checks.len(),
            failing.join(", ")
        ),
        Phase::Ready => "Ready to merge".into(),
        Phase::Merging => "Merging the pull request".into(),
        Phase::Merged if card.branch_deleted => {
            format!("Merged into {} · {} was deleted", card.base, card.head)
        }
        Phase::Merged => format!("Merged into {}", card.base),
        Phase::Closed => "Closed".into(),
        _ => card.step.clone(),
    };
    card
}

/// The conflict list a trial merge found (`None`: it couldn't tell, so the card keeps what it
/// had) — while it resolves, the files that dropped out of it count as resolved.
fn apply_conflicts(card: &mut Card, next: Option<Vec<String>>) {
    let Some(next) = next else {
        return;
    };
    if card.resolving {
        for f in card.conflicts.drain(..) {
            if !next.contains(&f) && !card.resolved.contains(&f) && card.resolved.len() < 12 {
                card.resolved.push(f);
            }
        }
    }
    card.conflicts = next;
}

/// Conflicting paths between the PR's head and its base, from a `merge-tree` trial in `dir`;
/// `None` when the trial couldn't run (no `origin`, a fetch that failed, a git without
/// `merge-tree --write-tree`), which is not the same as no conflicts.
fn conflicting_files(card: &Card, dir: &Path) -> Option<Vec<String>> {
    let (Some(n), base, head) = (card.number, card.base.clone(), card.head.clone()) else {
        return None;
    };
    if base.is_empty() || head.is_empty() {
        return None;
    }
    let git = || {
        let mut c = trek_core::procs::command("git");
        c.env("PATH", trek_core::detect::login_path())
            .current_dir(dir);
        c
    };
    // The base, and the PR's head — `pull/N/head` reaches it even from a fork. Forced (`+`): a
    // rebased or amended head is no fast-forward of what the last look kept.
    let fetch = git()
        .args([
            "fetch",
            "-q",
            "origin",
            &format!("+{base}:refs/trek/pr-base"),
            &format!("+pull/{n}/head:refs/trek/pr-head"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        || git()
            .args([
                "fetch",
                "-q",
                "origin",
                &format!("+{base}:refs/trek/pr-base"),
                &format!("+{head}:refs/trek/pr-head"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if !fetch {
        return None;
    }
    let out = git()
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "refs/trek/pr-base",
            "refs/trek/pr-head",
        ])
        .output();
    match out {
        // Exit 0: it merges cleanly.
        Ok(o) if o.status.success() => Some(vec![]),
        // Exit 1: conflicts — the tree oid is first, then one path per line, then a blank line
        // and git's messages about them ("CONFLICT (content): …"), which are not paths.
        Ok(o) if o.status.code() == Some(1) => Some(
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .skip(1)
                .map(str::trim)
                .take_while(|l| !l.is_empty())
                .map(String::from)
                .take(24)
                .collect(),
        ),
        // Anything else (an old git's usage error) is no answer.
        _ => None,
    }
}

/// Whether `card` follows a `release.yml` run (the marker; cards from before it have the stage).
fn is_workflow(card: &Card) -> bool {
    card.workflow
        || (card.run_id.is_none()
            && card
                .stages
                .first()
                .is_some_and(|s| s.label == "release.yml"))
}

/// A run of `gh run list --json databaseId,status,conclusion,url,createdAt`.
#[derive(Deserialize)]
struct WorkflowRun {
    #[serde(rename = "databaseId")]
    id: u64,
    status: String,
    conclusion: String,
    url: String,
    #[serde(rename = "createdAt")]
    created: String,
}

/// The run the dispatch behind `card` made: the one it already follows, else the first run
/// started since the dispatch (the list is newest first; an older run is someone else's).
fn pick_run<'a>(card: &Card, runs: &'a [WorkflowRun]) -> Option<&'a WorkflowRun> {
    if let Some(id) = card.run_id {
        return runs.iter().find(|r| r.id == id);
    }
    // A little slack for the clocks of this machine and GitHub's.
    let since = card.created_ms - 2_000;
    runs.iter().rev().find(|r| {
        chrono::DateTime::parse_from_rfc3339(&r.created)
            .is_ok_and(|d| d.timestamp_millis() >= since)
    })
}

/// `card` as `run` (and its `jobs`, when known) say the release is going.
fn release_run_card(card: &Card, run: &WorkflowRun, jobs: &[JobJson]) -> Card {
    let mut card = card.clone();
    card.workflow = true;
    card.run_id = Some(run.id);
    card.url = Some(run.url.clone());
    let stages = job_stages(jobs);
    if !stages.is_empty() {
        card.stages = stages;
    }
    if run.status != "completed" {
        enter_phase(&mut card, Phase::Releasing);
        card.step = "Release workflow running".into();
        return card;
    }
    enter_phase(
        &mut card,
        match run.conclusion.as_str() {
            "success" => Phase::Published,
            _ => Phase::Failed,
        },
    );
    card.step = if card.phase == Phase::Published {
        "Published".into()
    } else {
        "Release workflow failed".into()
    };
    let state = if card.phase == Phase::Published {
        CheckState::Pass
    } else {
        CheckState::Fail
    };
    for s in &mut card.stages {
        if s.state.pending() {
            s.state = state;
        }
    }
    card
}

fn fetch_release(card: &Card, dir: &Path) -> Option<Card> {
    // Only workflow releases are followed past the command that started them.
    if !is_workflow(card) {
        return None;
    }
    let out = gh(dir)?
        .args([
            "run",
            "list",
            "--workflow",
            "release.yml",
            "--limit",
            "20",
            "--json",
            "databaseId,status,conclusion,url,createdAt",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let runs = serde_json::from_slice::<Vec<WorkflowRun>>(&out.stdout).ok()?;
    // None yet: the dispatch is queued and the card keeps waiting.
    let run = pick_run(card, &runs)?;
    let jobs = gh(dir)
        .and_then(|mut c| {
            c.args(["run", "view", &run.id.to_string(), "--json", "jobs"])
                .output()
                .ok()
        })
        .and_then(|o| {
            #[derive(Deserialize)]
            struct Jobs {
                jobs: Vec<JobJson>,
            }
            serde_json::from_slice::<Jobs>(&o.stdout).ok()
        })
        .map(|j| j.jobs)
        .unwrap_or_default();
    Some(release_run_card(card, run, &jobs))
}

// ---------- drawing ----------

/// What the card's controls do (all optional: no Resolve while resolving, no Merge until Ready).
#[derive(Default)]
pub struct Actions {
    pub merge: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    pub resolve: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// Open it on GitHub: the badge, a failure's View log, a passed run's Open run.
    pub open: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// A failed run's (or release workflow's) Re-run.
    pub rerun: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// A published release's notes on GitHub.
    pub notes: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// The card's width in the reference; narrower columns scale the graph's middle to fit.
const CARD_W: f32 = 600.;

/// A verdict colour: `mark` for the graph's dots, lines and nodes, `text` for words in it.
/// Colour is only for verdicts (and the branch the card is about); the rest is the theme's ink.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hue {
    mark: Hsla,
    text: Hsla,
    /// How strongly its pills and tiles are washed with it: the reference's pale tints on a light
    /// page (#F0FDF3 behind green), more on a dark one, where a pale wash disappears.
    wash: f32,
}

impl Hue {
    /// A soft pill or tile behind its text.
    fn fill(self) -> Hsla {
        self.mark.opacity(self.wash)
    }
    fn border(self) -> Hsla {
        self.mark.opacity(self.wash * 3.4)
    }
    /// A lane's line: the mark, quietly.
    fn lane(self) -> Hsla {
        self.mark.opacity(0.36)
    }
}

#[derive(Clone, Copy, Debug)]
struct Hues {
    blue: Hue,
    amber: Hue,
    green: Hue,
    red: Hue,
    purple: Hue,
}

/// The reference's verdict colours on a light page; brighter marks and lighter text on a dark
/// one, so they read the same.
fn hues(cx: &App) -> Hues {
    let dark = cx.theme().mode.is_dark();
    let hue = |mark: u32, text: u32| Hue {
        mark: rgb(mark).into(),
        text: rgb(text).into(),
        wash: if dark { 0.13 } else { 0.075 },
    };
    if dark {
        Hues {
            blue: hue(0x5AA9FF, 0x8CC2FF),
            amber: hue(0xF2A62A, 0xFFC560),
            green: hue(0x34C77B, 0x63D9A0),
            red: hue(0xFF5A5F, 0xFF8A8D),
            purple: hue(0xA27BF7, 0xC4B0FF),
        }
    } else {
        Hues {
            blue: hue(0x4A8CF0, 0x3B5BAB),
            amber: hue(0xE9A80E, 0xA5650F),
            green: hue(0x2FC261, 0x1E8A4C),
            red: hue(0xE5484D, 0xC6283D),
            purple: hue(0x8B5CF6, 0x6227AD),
        }
    }
}

/// The card's neutrals, from the theme's ink: the reference's greys on white are the page's
/// foreground at low strength, which keeps them right on Paper's warm page and on Night.
#[derive(Clone, Copy, Debug)]
struct Ink {
    fg: Hsla,
    muted: Hsla,
    /// Commit dots and the origin.
    dot: Hsla,
    /// The base's lane and finished jobs' lines.
    line: Hsla,
    /// The base coming into the branch (a merge commit).
    merge_in: Hsla,
    /// The graph band, over the card.
    band: Hsla,
    /// Mono pills, the busy pill, the light button.
    pill: Hsla,
    /// The base's lane label.
    lane_pill: Hsla,
    /// The card's and chips' hairline.
    hair: Hsla,
    /// The card's fill (opaque).
    card: Hsla,
    /// The band over the card (opaque): hollow rings and labels sit on it.
    surface: Hsla,
    /// A state still being worked out ("Checking", "Building 2 of 3").
    faint: Hsla,
    /// Mono names: branches, the base's lane, jobs that are done.
    mono: Hsla,
}

fn ink(cx: &App) -> Ink {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let fg = theme.foreground;
    let a = |light: f32, night: f32| fg.opacity(if dark { night } else { light });
    let card = if dark {
        theme.background.blend(fg.opacity(0.03))
    } else {
        theme.background
    };
    let band = a(0.034, 0.045);
    Ink {
        fg,
        muted: theme.muted_foreground,
        dot: a(0.32, 0.36),
        line: a(0.12, 0.15),
        merge_in: a(0.17, 0.2),
        band,
        pill: a(0.045, 0.075),
        lane_pill: a(0.065, 0.09),
        hair: a(0.105, 0.12),
        card,
        surface: card.blend(band),
        faint: a(0.42, 0.45),
        mono: a(0.52, 0.58),
    }
}

/// The card for transcript item `ix` showing `card`; `clock` animates its in-progress marks
/// (seconds; the working bar's pulse drives it).
pub fn card(ix: usize, card: &Card, actions: Actions, clock: f32, cx: &App) -> AnyElement {
    let ink = ink(cx);
    let hue = hues(cx);
    let motion = !cx.reduce_motion();
    let head = header(ix, card, &actions, &hue, &ink, cx);
    let graph = graph(card, clock, motion, cx);
    let footer = footer(ix, card, &actions, &hue, &ink, cx);
    let tail = footer.is_none();
    let frame = v_flex()
        .id(("pr-card", ix))
        .test_support()
        .w_full()
        .max_w(px(CARD_W))
        .mt(px(2.))
        .mb(px(8.))
        .rounded(px(14.))
        .border_1()
        .border_color(ink.hair)
        .bg(ink.card)
        .overflow_hidden()
        .child(head)
        .child(graph)
        .children(footer)
        .when(tail, |el| el.child(div().h(px(7.))));
    if motion {
        frame
            .with_animation(
                SharedString::from(format!("pr-card-enter-{ix}-{}", card.created_ms)),
                Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mt(px(2. + 8. * (1. - t))),
            )
            .into_any_element()
    } else {
        frame.into_any_element()
    }
}

/// A 22 px round tile with a 12 px glyph: the header's and the footer's lead.
fn tile(icon: Icon, fill: Hsla, color: Hsla) -> AnyElement {
    div()
        .size(px(22.))
        .flex_none()
        .rounded_full()
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .child(icon.size(px(12.)).text_color(color))
        .into_any_element()
}

/// A branch, commit or base: mono on the soft square pill the reference's meta row uses.
fn mono_pill(text: &str, ink: &Ink, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .max_w(px(200.))
        .truncate()
        .h(px(15.))
        .px(px(5.))
        .rounded(px(4.))
        .bg(ink.pill)
        .flex()
        .items_center()
        .text_size(px(12.))
        .line_height(px(15.))
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(ink.mono)
        .child(text.to_string())
        .into_any_element()
}

/// "Push", "Pull request": what started a run, as its badge says it.
fn event_label(event: &str) -> String {
    match event {
        "" => String::new(),
        "push" => "Push".into(),
        "pull_request" | "pull_request_target" => "Pull request".into(),
        "workflow_dispatch" => "Manual".into(),
        "schedule" => "Scheduled".into(),
        "release" => "Release".into(),
        other => {
            let mut words = other.replace('_', " ");
            if let Some(first) = words.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            words
        }
    }
}

/// "Nightly", "Beta": a release's channel, as its badge says it.
fn channel_label(channel: &str) -> String {
    let mut c = channel.to_string();
    if let Some(first) = c.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    c
}

/// "Mac + Windows" from a release's jobs; "3 jobs" when the names don't say.
fn platforms(card: &Card) -> String {
    let has = |w: &str| {
        card.stages
            .iter()
            .any(|s| s.label.to_lowercase().contains(w))
    };
    let mut names = vec![];
    if has("mac") {
        names.push("Mac");
    }
    if has("windows") {
        names.push("Windows");
    }
    if has("linux") {
        names.push("Linux");
    }
    if names.is_empty() {
        jobs_count(card.stages.len())
    } else {
        names.join(" + ")
    }
}

fn jobs_count(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "1 job".into(),
        n => format!("{n} jobs"),
    }
}

/// How long a release has taken: to the moment it settled once it has, else up to now.
fn release_ms(card: &Card) -> i64 {
    let began = if card.started_ms > 0 {
        card.started_ms
    } else {
        card.created_ms
    };
    let end = if card.phase.terminal() {
        card.phase_at_ms.max(card.created_ms)
    } else {
        now_ms()
    };
    (end - began).max(0)
}

/// Title, meta row and the state badge, top right.
fn header(
    ix: usize,
    card: &Card,
    actions: &Actions,
    hue: &Hues,
    ink: &Ink,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let lead = |icon: Icon, h: Hue| tile(icon, h.fill(), h.text);
    let neutral = |icon: Icon| tile(icon, ink.pill, ink.muted);
    let tile = match (card.kind, card.phase) {
        (Kind::Pr, Phase::Merged) => lead(Icon::new(crate::assets::Lucide::GitMerge), hue.purple),
        (Kind::Pr, Phase::Creating | Phase::Closed) => {
            neutral(Icon::new(crate::assets::Lucide::GitPullRequest))
        }
        (Kind::Pr, Phase::Failed) => {
            lead(Icon::new(crate::assets::Lucide::GitPullRequest), hue.red)
        }
        (Kind::Pr, _) => lead(Icon::new(crate::assets::Lucide::GitPullRequest), hue.green),
        (Kind::Release, Phase::Published) => {
            lead(Icon::new(crate::assets::Lucide::Rocket), hue.green)
        }
        (Kind::Release, Phase::Failed | Phase::Closed) => {
            lead(Icon::new(crate::assets::Lucide::Rocket), hue.red)
        }
        (Kind::Release, _) => lead(Icon::new(crate::assets::Lucide::Rocket), hue.blue),
        (Kind::Run, Phase::Passed) => lead(Icon::new(IconName::Check), hue.green),
        (Kind::Run, Phase::Failed | Phase::Closed) => {
            lead(Icon::new(crate::assets::Lucide::X), hue.red)
        }
        (Kind::Run, _) => lead(Icon::new(crate::assets::Lucide::Workflow), hue.blue),
    };

    // The title's two halves: the strong name, then what it's about.
    let (name, rest): (String, Option<String>) = match card.kind {
        Kind::Pr => (card.title.clone(), card.number.map(|n| format!("#{n}"))),
        Kind::Release => match card.title.split_once(' ') {
            Some((what, version)) => (what.to_string(), Some(version.to_string())),
            None => (card.title.clone(), None),
        },
        Kind::Run => (
            run_name(card).to_string(),
            (!card.subject.is_empty()).then(|| card.subject.clone()),
        ),
    };
    let rest_muted = card.kind == Kind::Pr;
    let title = h_flex()
        .h(px(20.))
        .min_w_0()
        .gap(px(5.))
        .items_baseline()
        .text_size(px(15.))
        .line_height(px(20.))
        .overflow_hidden()
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_medium()
                .text_color(ink.fg)
                .child(name),
        )
        .when_some(rest, |el, rest| {
            el.child(
                div()
                    .min_w(px(40.))
                    .truncate()
                    .text_color(if rest_muted {
                        ink.muted
                    } else {
                        ink.fg.opacity(0.82)
                    })
                    .child(rest),
            )
        });

    let mut meta = h_flex()
        .h(px(16.))
        .mt(px(2.))
        .min_w_0()
        .items_center()
        .text_size(px(13.))
        .text_color(ink.muted)
        .overflow_hidden();
    let count = |n: u32| format!("{n} {}", if n == 1 { "commit" } else { "commits" });
    match card.kind {
        Kind::Pr => {
            if !card.head.is_empty() {
                meta = meta.child(mono_pill(&card.head, ink, cx)).child(
                    div()
                        .px(px(6.))
                        .text_size(px(11.))
                        .text_color(ink.muted.opacity(0.85))
                        .child("→"),
                );
            }
            if !card.base.is_empty() {
                meta = meta.child(mono_pill(&card.base, ink, cx));
            }
            if let Some(n) = card.commits.filter(|n| *n > 0) {
                meta = meta.child(div().ml(px(11.)).flex_none().child(count(n)));
            }
            let (a, r) = (card.added.unwrap_or(0), card.removed.unwrap_or(0));
            if a > 0 || r > 0 {
                meta = meta.child(
                    h_flex()
                        .ml(px(10.))
                        .gap(px(5.))
                        .flex_none()
                        .child(div().text_color(hue.green.text).child(format!("+{a}")))
                        .child(div().text_color(hue.red.text).child(format!("−{r}"))),
                );
            }
        }
        Kind::Release | Kind::Run => {
            if !card.head.is_empty() {
                meta = meta.child(div().mr(px(8.)).child(mono_pill(&card.head, ink, cx)));
            }
            let what = if card.kind == Kind::Release {
                platforms(card)
            } else {
                jobs_count(card.stages.len())
            };
            if !what.is_empty() {
                meta = meta.child(div().mr(px(8.)).flex_none().child(what));
            }
            let took = if card.kind == Kind::Release {
                release_ms(card)
            } else {
                (card.started_ms > 0).then(|| run_ms(card)).unwrap_or(0)
            };
            if took > 0 {
                meta = meta.child(div().flex_none().child(dur_label(took)));
            }
        }
    }

    // The badge: a PR's state; a release's channel and a run's trigger, which aren't verdicts.
    let badge: Option<(Option<AnyElement>, String, Hsla, Hsla)> = match card.kind {
        Kind::Pr => match card.phase {
            Phase::Merged => Some((
                Some(
                    Icon::new(crate::assets::Lucide::GitMerge)
                        .size(px(12.))
                        .text_color(hue.purple.text)
                        .into_any_element(),
                ),
                "Merged".into(),
                hue.purple.text,
                hue.purple.fill(),
            )),
            Phase::Closed => Some((None, "Closed".into(), ink.muted, ink.pill)),
            Phase::Creating | Phase::Failed => None,
            _ if card.number.is_some() => Some((
                Some(
                    Icon::new(crate::assets::Lucide::GitPullRequest)
                        .size(px(12.))
                        .text_color(hue.green.text)
                        .into_any_element(),
                ),
                "Open".into(),
                hue.green.text,
                hue.green.fill(),
            )),
            _ => None,
        },
        Kind::Release => (!card.channel.is_empty())
            .then(|| (None, channel_label(&card.channel), ink.muted, ink.pill)),
        Kind::Run => {
            let e = event_label(&card.event);
            (!e.is_empty()).then(|| (None, e, ink.muted, ink.pill))
        }
    };
    let badge = badge.map(|(icon, label, color, fill)| {
        let b = h_flex()
            .id(("pr-open", ix))
            .test_support()
            .flex_none()
            .h(px(20.))
            .px(px(8.))
            .gap(px(4.))
            .items_center()
            .rounded_full()
            .bg(fill)
            .text_size(px(13.))
            .text_color(color)
            .children(icon)
            .child(label);
        match actions.open.clone() {
            Some(open) => b
                .cursor_pointer()
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open on GitHub").build(window, cx)
                })
                .on_click(move |_, window, cx| open(window, cx))
                .into_any_element(),
            None => b.into_any_element(),
        }
    });

    h_flex()
        .id(("pr-head", ix))
        .test_support()
        .w_full()
        .items_start()
        .pl(px(15.))
        .pr(px(16.))
        .pt(px(18.))
        .pb(px(4.))
        .gap(px(13.))
        .child(div().mt(px(2.)).child(tile))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(title)
                .child(meta)
                .text_color(theme.foreground),
        )
        .children(badge.map(|b| div().mt(px(2.)).flex_none().child(b)))
        .into_any_element()
}

// ---------- the graph: a scene of marks in card coordinates ----------

/// How a line is drawn: done (solid), on its way (dashed), still to come (dotted).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stroke {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Glyph {
    Check,
    Bang,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Align {
    Left,
    Center,
    Right,
}

/// A lane label (mono on the band), a branch label (mono on its tint) or a status pill.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LabelKind {
    Lane,
    Branch,
    Status,
}

/// One mark of a card's graph. `x` runs from the card's left edge, `y` from the graph's top
/// (57 px below the card's), as the reference measures them.
#[derive(Clone, Debug, PartialEq)]
enum Prim {
    /// A straight run from `a` to `b`, drawn from `a` as far as `t`.
    Line {
        a: (f32, f32),
        b: (f32, f32),
        stroke: Stroke,
        color: Hsla,
        width: f32,
        t: f32,
    },
    /// A lane change: a cubic with horizontal tangents at both ends.
    Curve {
        a: (f32, f32),
        b: (f32, f32),
        stroke: Stroke,
        color: Hsla,
        width: f32,
        t: f32,
    },
    Dot {
        at: (f32, f32),
        r: f32,
        color: Hsla,
    },
    /// A hollow node: a commit being made, a merge commit, a step still to come (`dashed`).
    Ring {
        at: (f32, f32),
        r: f32,
        color: Hsla,
        dashed: bool,
    },
    /// A verdict node: filled, with a white glyph.
    Node {
        at: (f32, f32),
        r: f32,
        color: Hsla,
        glyph: Option<Glyph>,
    },
    /// A soft disc behind a node that wants attention.
    Halo {
        at: (f32, f32),
        r: f32,
        color: Hsla,
    },
    /// A ring turning: work under way at this node.
    Spinner {
        at: (f32, f32),
        r: f32,
        color: Hsla,
    },
    /// A ring spreading out from a node being made.
    Pulse {
        at: (f32, f32),
        color: Hsla,
    },
    Label {
        text: String,
        x: f32,
        y: f32,
        align: Align,
        kind: LabelKind,
        color: Hsla,
        fill: Hsla,
        border: Option<Hsla>,
        strike: bool,
    },
}

/// A graph: its marks, and whether something in it keeps moving.
#[derive(Debug, Default)]
struct Scene {
    prims: Vec<Prim>,
    live: bool,
}

/// x as the reference places it on a 600 px card; a narrower card squeezes the middle so the
/// end node keeps its place 42 px from the right edge.
fn scale_x(w: f32) -> impl Fn(f32) -> f32 {
    move |x: f32| {
        if w >= CARD_W {
            x
        } else {
            34. + (x - 34.) * ((w - 42. - 34.) / (CARD_W - 42. - 34.)).max(0.3)
        }
    }
}

fn label(text: impl Into<String>, x: f32, y: f32, align: Align, kind: LabelKind) -> Prim {
    Prim::Label {
        text: text.into(),
        x,
        y,
        align,
        kind,
        color: hsla(0., 0., 0., 0.),
        fill: hsla(0., 0., 0., 0.),
        border: None,
        strike: false,
    }
}

/// `label` with its colours.
fn styled(mut p: Prim, text_color: Hsla, fill_color: Hsla, border_color: Option<Hsla>) -> Prim {
    if let Prim::Label {
        color,
        fill,
        border,
        ..
    } = &mut p
    {
        *color = text_color;
        *fill = fill_color;
        *border = border_color;
    }
    p
}

/// "3 conflicts", "1 conflict".
fn conflicts_label(n: usize) -> String {
    match n {
        0 => "Conflicts".into(),
        1 => "1 conflict".into(),
        n => format!("{n} conflicts"),
    }
}

/// The pull request's graph, as the reference draws it: the base's lane on top, the branch
/// below, and a connector to the end node at the right, which carries the verdict.
fn pr_scene(card: &Card, w: f32, draw: f32, hue: &Hues, ink: &Ink) -> Scene {
    let mut p: Vec<Prim> = vec![];
    let sx = scale_x(w);
    let (main_y, feat_y) = (42., 79.);
    let end = (w - 42., main_y);
    let phase = card.phase;
    let merged = phase == Phase::Merged;
    let branch = if merged { hue.purple } else { hue.blue };
    let creating = phase == Phase::Creating;
    let committing = creating && card.step.starts_with("Committing");
    let opening = creating && card.step.starts_with("Opening");
    let opened = !matches!(phase, Phase::Creating | Phase::Failed);
    // The base moved on while we worked (the reference's "Fetching main" brings its commits).
    let moved = matches!(
        phase,
        Phase::Checking | Phase::Conflicted | Phase::Ready | Phase::Merging | Phase::Merged
    ) || (phase == Phase::Open && card.step.starts_with("Fetching"));
    let merge_commit = opened && moved && (card.resolving || !card.resolved.is_empty());
    let resolved_all = card.resolving && card.conflicts.is_empty() && !card.resolved.is_empty();
    let commits = card
        .commits
        .map(|n| n as usize)
        .filter(|n| *n > 0)
        .unwrap_or_else(|| card.history.len().max(1))
        .clamp(1, 3);
    let mut live = draw < 1.;

    // The base's lane.
    let base = if card.base.is_empty() {
        "main"
    } else {
        &card.base
    };
    let main_dots: Vec<f32> = if moved {
        vec![34., 109., 289., 379.]
    } else {
        vec![34., 109.]
    }
    .into_iter()
    .map(&sx)
    .collect();
    let last_main = *main_dots.last().unwrap_or(&34.);
    p.push(Prim::Line {
        a: (main_dots[0], main_y),
        b: (last_main, main_y),
        stroke: Stroke::Solid,
        color: ink.line,
        width: 1.5,
        t: 1.,
    });
    if opened || opening {
        p.push(Prim::Line {
            a: (last_main, main_y),
            b: (end.0, main_y),
            stroke: if merged {
                Stroke::Solid
            } else {
                Stroke::Dotted
            },
            color: ink.line,
            width: 1.5,
            t: 1.,
        });
    }

    // The branch: off the base's second commit, its commits 75 apart.
    let xs: Vec<f32> = (0..commits).map(|i| sx(184. + 75. * i as f32)).collect();
    let last_feat = *xs.last().unwrap_or(&sx(184.));
    let lane = branch.lane();
    p.push(Prim::Curve {
        a: (sx(109.), main_y),
        b: (sx(184.) - 5., feat_y),
        stroke: Stroke::Solid,
        color: lane,
        width: 1.5,
        t: 1.,
    });
    if commits > 1 {
        let before_last = xs[xs.len() - 2];
        p.push(Prim::Line {
            a: (xs[0], feat_y),
            b: (before_last, feat_y),
            stroke: Stroke::Solid,
            color: lane,
            width: 1.5,
            t: 1.,
        });
        p.push(Prim::Line {
            a: (before_last, feat_y),
            b: (last_feat, feat_y),
            stroke: if committing {
                Stroke::Dotted
            } else {
                Stroke::Solid
            },
            color: lane,
            width: 1.5,
            t: 1.,
        });
    }
    let merge_x = sx(453.);
    let tip = if merge_commit {
        (merge_x, feat_y)
    } else {
        (last_feat, feat_y)
    };
    if merge_commit {
        p.push(Prim::Line {
            a: (last_feat, feat_y),
            b: (merge_x, feat_y),
            stroke: Stroke::Solid,
            color: lane,
            width: 1.5,
            t: 1.,
        });
        p.push(Prim::Curve {
            a: (sx(379.), main_y),
            b: (merge_x - 4., feat_y),
            stroke: Stroke::Solid,
            color: ink.merge_in,
            width: 1.5,
            t: 1.,
        });
    }

    // The connector to the end node, in the verdict's colour.
    let curve = |from: (f32, f32), stroke: Stroke, color: Hsla, width: f32, t: f32| Prim::Curve {
        a: from,
        b: end,
        stroke,
        color,
        width,
        t,
    };
    let blue = hue.blue.mark.opacity(0.75);
    match phase {
        Phase::Creating if opening => p.push(curve(tip, Stroke::Dashed, blue, 1.5, draw)),
        Phase::Open | Phase::Checking => p.push(curve(tip, Stroke::Dashed, blue, 1.5, 1.)),
        Phase::Conflicted if card.resolving => {
            p.push(curve(
                (last_feat, feat_y),
                Stroke::Dashed,
                hue.amber.mark.opacity(0.3),
                1.5,
                1.,
            ));
            if resolved_all {
                p.push(curve(tip, Stroke::Dashed, blue, 1.5, draw));
            }
        }
        Phase::Conflicted => p.push(curve(tip, Stroke::Dashed, hue.amber.mark, 1.5, draw)),
        Phase::Ready => p.push(curve(tip, Stroke::Dashed, hue.green.mark, 1.5, draw)),
        Phase::Merging => {
            p.push(curve(tip, Stroke::Dashed, hue.green.mark, 1.5, 1.));
            p.push(curve(tip, Stroke::Solid, hue.purple.mark, 1.75, draw));
        }
        Phase::Merged => p.push(curve(tip, Stroke::Solid, hue.purple.mark, 1.75, 1.)),
        Phase::Closed => p.push(curve(tip, Stroke::Dashed, ink.dot, 1.5, 1.)),
        _ => {}
    }

    // Dots over the lines.
    for x in &main_dots {
        p.push(Prim::Dot {
            at: (*x, main_y),
            r: 3.5,
            color: ink.dot,
        });
    }
    for (i, x) in xs.iter().enumerate() {
        if committing && i + 1 == xs.len() {
            p.push(Prim::Ring {
                at: (*x, feat_y),
                r: 4.,
                color: branch.mark.opacity(0.5),
                dashed: false,
            });
        } else {
            p.push(Prim::Dot {
                at: (*x, feat_y),
                r: 3.5,
                color: branch.mark,
            });
        }
    }
    if merge_commit {
        if card.resolving && !resolved_all {
            p.push(Prim::Pulse {
                at: tip,
                color: branch.mark,
            });
            live = true;
        }
        p.push(Prim::Ring {
            at: tip,
            r: 4.,
            color: branch.mark,
            dashed: false,
        });
    }

    // The end node and its pill.
    let white_pill = |text: &str| {
        styled(
            label(
                text,
                end.0 + 7.,
                end.1 - 25.,
                Align::Right,
                LabelKind::Status,
            ),
            ink.faint,
            ink.card,
            Some(ink.hair),
        )
    };
    let verdict_pill = |text: &str, h: Hue| {
        styled(
            label(
                text,
                end.0 + 7.,
                end.1 - 25.,
                Align::Right,
                LabelKind::Status,
            ),
            h.text,
            h.fill(),
            Some(h.border()),
        )
    };
    let pending = Prim::Ring {
        at: end,
        r: 4.,
        color: ink.dot,
        dashed: true,
    };
    match phase {
        Phase::Open => p.push(pending),
        Phase::Checking => {
            p.push(Prim::Spinner {
                at: end,
                r: 8.5,
                color: ink.dot,
            });
            p.push(white_pill("Checking"));
            live = true;
        }
        Phase::Conflicted => {
            p.push(Prim::Halo {
                at: end,
                r: 11.,
                color: hue.amber.mark.opacity(0.25),
            });
            p.push(Prim::Node {
                at: end,
                r: 7.,
                color: hue.amber.mark,
                glyph: Some(Glyph::Bang),
            });
            p.push(if resolved_all {
                white_pill("Resolved")
            } else {
                verdict_pill(&conflicts_label(card.conflicts.len()), hue.amber)
            });
        }
        Phase::Ready | Phase::Merging => {
            p.push(Prim::Node {
                at: end,
                r: 7.,
                color: hue.green.mark,
                glyph: Some(Glyph::Check),
            });
            p.push(verdict_pill("Ready to merge", hue.green));
        }
        Phase::Merged => {
            p.push(Prim::Dot {
                at: end,
                r: 5.5,
                color: hue.purple.mark,
            });
            p.push(verdict_pill("Merged", hue.purple));
        }
        Phase::Closed => {
            p.push(pending);
            p.push(styled(
                label(
                    "Closed",
                    end.0 + 7.,
                    end.1 - 25.,
                    Align::Right,
                    LabelKind::Status,
                ),
                ink.muted,
                ink.pill,
                None,
            ));
        }
        _ => {}
    }

    // Labels last, over everything.
    p.push(styled(
        label(base, sx(25.), 16., Align::Left, LabelKind::Lane),
        ink.mono,
        ink.lane_pill,
        None,
    ));
    if !card.head.is_empty() {
        let mid = if xs.len() > 1 {
            (xs[0] + xs[1]) / 2.
        } else {
            xs[0]
        };
        let mut l = styled(
            label(&card.head, mid, 86., Align::Center, LabelKind::Branch),
            branch.text,
            branch.fill(),
            None,
        );
        if let Prim::Label { strike, .. } = &mut l {
            *strike = merged && card.branch_deleted;
        }
        p.push(l);
    }
    Scene { prims: p, live }
}

/// The publish step of a release: it waits on the builds, so it sits after them on the base's
/// lane rather than beside them.
fn is_publish(stage: &Stage) -> bool {
    let l = stage.label.to_lowercase();
    l.contains("publish") || l.contains("upload") || l.contains("deploy")
}

/// How long a job took (or has taken so far); 0 before it starts.
fn stage_ms(stage: &Stage) -> i64 {
    if stage.started_ms <= 0 {
        return 0;
    }
    let end = if stage.ended_ms > 0 {
        stage.ended_ms
    } else if stage.state.pending() {
        now_ms()
    } else {
        return 0;
    };
    (end - stage.started_ms).max(0)
}

/// A job's label on its lane: "macOS app · 9m 12s", "Windows build · 6m", "Build · failed".
fn stage_label(stage: &Stage) -> String {
    let ms = stage_ms(stage);
    let name = if stage.label.chars().count() > 28 {
        format!("{}…", stage.label.chars().take(27).collect::<String>())
    } else {
        stage.label.clone()
    };
    match stage.state {
        CheckState::Fail => format!("{name} · failed"),
        CheckState::Skipped => format!("{name} · skipped"),
        CheckState::Running if ms >= 60_000 => format!("{name} · {}m", ms / 60_000),
        CheckState::Running if ms > 0 => format!("{name} · {}s", ms / 1000),
        CheckState::Pass if ms > 0 => format!("{name} · {}", dur_label(ms)),
        _ => name,
    }
}

/// A release's or run's state in a few words, said on the end node's pill and in the chip:
/// "Building 2 of 3", "2 of 3 passed", "Published", "1 failed".
fn pipeline_state(card: &Card) -> String {
    let total = card.stages.len();
    let passed = card
        .stages
        .iter()
        .filter(|s| s.state == CheckState::Pass)
        .count();
    let failed = card
        .stages
        .iter()
        .filter(|s| s.state == CheckState::Fail)
        .count();
    match (card.kind, card.phase) {
        (_, Phase::Published) => "Published".into(),
        (_, Phase::Passed) => "Passed".into(),
        (_, Phase::Failed | Phase::Closed) if card.step == "Cancelled" => "Cancelled".into(),
        (Kind::Release, Phase::Failed | Phase::Closed) => "Failed".into(),
        (_, Phase::Failed | Phase::Closed) if failed > 0 => format!("{failed} failed"),
        (_, Phase::Failed | Phase::Closed) => "Failed".into(),
        (Kind::Release, _)
            if card
                .stages
                .iter()
                .any(|s| is_publish(s) && s.state == CheckState::Running) =>
        {
            "Publishing".into()
        }
        (Kind::Release, _) if total > 1 => {
            format!("Building {} of {total}", (passed + 1).min(total))
        }
        (Kind::Release, _) => "Releasing".into(),
        (_, _) if total > 0 => format!("{passed} of {total} passed"),
        _ => "Starting".into(),
    }
}

/// A release's or run's graph: its jobs as lanes from one origin, each node placed by how long
/// the job took, joined at the publish step (a release's) and at the end node, which carries the
/// verdict. `measure` gives a lane label's width.
fn pipeline_scene(
    card: &Card,
    w: f32,
    draw: f32,
    hue: &Hues,
    ink: &Ink,
    measure: &dyn Fn(&str) -> f32,
) -> Scene {
    let mut lines: Vec<Prim> = vec![];
    let mut marks: Vec<Prim> = vec![];
    let mut labels: Vec<Prim> = vec![];
    let release = card.kind == Kind::Release;
    let done = card.phase.terminal();
    let good = matches!(card.phase, Phase::Published | Phase::Passed);
    let end_x = w - 42.;

    let (group, tail): (Vec<&Stage>, Vec<&Stage>) = if release
        && card.stages.iter().any(is_publish)
        && card.stages.iter().any(|s| !is_publish(s))
    {
        card.stages.iter().partition(|s| !is_publish(s))
    } else {
        (card.stages.iter().collect(), vec![])
    };
    let k = group.len();
    let lanes: Vec<f32> = match k {
        0 | 1 => vec![42.],
        2 => vec![42., 79.],
        3 => vec![23., 51., 79.],
        k => (0..k).map(|i| 18. + i as f32 * 20.).collect(),
    };
    let oy = if k <= 2 {
        42.
    } else {
        (lanes[0] + lanes[k - 1]) / 2.
    };
    let origin = (34., oy);
    let tail_x = (end_x - 149.).max(origin.0 + 160.);
    let join_x = if tail.is_empty() { end_x } else { tail_x };
    let hi = if tail.is_empty() {
        end_x - 154.
    } else {
        tail_x - 150.
    };
    let lo = origin.0 + 110.;
    let max_ms = group.iter().map(|s| stage_ms(s)).max().unwrap_or(0).max(1);

    let state_hue = |s: CheckState| match s {
        CheckState::Running => Some(hue.blue),
        CheckState::Fail => Some(hue.red),
        _ => None,
    };
    let lane_color = |s: CheckState| state_hue(s).map_or(ink.line, |h| h.lane());
    let node = |at: (f32, f32), s: CheckState, marks: &mut Vec<Prim>, live: &mut bool| match s {
        CheckState::Pass => marks.push(Prim::Node {
            at,
            r: 5.5,
            color: hue.green.mark,
            glyph: Some(Glyph::Check),
        }),
        CheckState::Fail => {
            marks.push(Prim::Halo {
                at,
                r: 11.,
                color: hue.red.mark.opacity(0.2),
            });
            marks.push(Prim::Node {
                at,
                r: 7.,
                color: hue.red.mark,
                glyph: Some(Glyph::Bang),
            });
        }
        CheckState::Running => {
            marks.push(Prim::Spinner {
                at,
                r: 7.5,
                color: hue.blue.mark,
            });
            marks.push(Prim::Dot {
                at,
                r: 3.5,
                color: hue.blue.mark,
            });
            *live = true;
        }
        CheckState::Queued => marks.push(Prim::Ring {
            at,
            r: 4.,
            color: ink.dot,
            dashed: true,
        }),
        CheckState::Skipped => marks.push(Prim::Ring {
            at,
            r: 3.5,
            color: ink.dot,
            dashed: false,
        }),
    };
    let label_colors = |s: CheckState| match state_hue(s) {
        Some(h) => (h.text, h.fill()),
        None => (ink.mono, ink.surface),
    };
    let mut live = draw < 1.;

    for (i, stage) in group.iter().enumerate() {
        let y = lanes[i];
        let text = stage_label(stage);
        let text_w = measure(&text) + 10.;
        let inline = k >= 3;
        let min_x = if inline {
            origin.0 + 40. + text_w + 12.
        } else {
            origin.0 + text_w / 2. + 8.
        };
        let frac = stage_ms(stage) as f32 / max_ms as f32;
        let x = (lo + (hi - lo) * frac).max(min_x).min(join_x - 40.);
        let before = if stage.state == CheckState::Queued || stage.state == CheckState::Skipped {
            Stroke::Dotted
        } else {
            Stroke::Solid
        };
        let color = lane_color(stage.state);
        // From the origin to the node.
        if (y - oy).abs() < 0.5 {
            lines.push(Prim::Line {
                a: origin,
                b: (x, y),
                stroke: before,
                color,
                width: 1.5,
                t: 1.,
            });
        } else {
            let span = (x - origin.0 - 8.).min(if inline { 110. } else { 200. });
            lines.push(Prim::Curve {
                a: origin,
                b: (origin.0 + span, y),
                stroke: before,
                color,
                width: 1.5,
                t: 1.,
            });
            lines.push(Prim::Line {
                a: (origin.0 + span, y),
                b: (x, y),
                stroke: before,
                color,
                width: 1.5,
                t: 1.,
            });
        }
        // From the node on to where the lanes join.
        let after = match stage.state {
            CheckState::Pass => Stroke::Solid,
            CheckState::Running | CheckState::Fail => Stroke::Dashed,
            _ => Stroke::Dotted,
        };
        let after_color = match stage.state {
            CheckState::Running | CheckState::Fail => color.opacity(0.7),
            _ => ink.line,
        };
        if (y - oy).abs() < 0.5 {
            lines.push(Prim::Line {
                a: (x, y),
                b: (join_x, oy),
                stroke: after,
                color: after_color,
                width: 1.5,
                t: 1.,
            });
        } else {
            let span = (join_x - x - 10.).min(if inline { 140. } else { 220. });
            lines.push(Prim::Line {
                a: (x, y),
                b: (join_x - span, y),
                stroke: after,
                color: after_color,
                width: 1.5,
                t: 1.,
            });
            lines.push(Prim::Curve {
                a: (join_x - span, y),
                b: (join_x, oy),
                stroke: after,
                color: after_color,
                width: 1.5,
                t: 1.,
            });
        }
        node((x, y), stage.state, &mut marks, &mut live);
        let (tc, fc) = label_colors(stage.state);
        let l = if inline {
            label(text, x - 12., y - 7.5, Align::Right, LabelKind::Lane)
        } else if i == 0 {
            label(text, x, y - 26., Align::Center, LabelKind::Lane)
        } else {
            label(text, x, y + 11., Align::Center, LabelKind::Lane)
        };
        labels.push(styled(l, tc, fc, None));
    }
    if k == 0 {
        lines.push(Prim::Line {
            a: origin,
            b: (join_x, oy),
            stroke: if done { Stroke::Solid } else { Stroke::Dotted },
            color: ink.line,
            width: 1.5,
            t: 1.,
        });
    }

    // The publish step(s), one after another on the base's lane.
    let m = tail.len();
    for (j, stage) in tail.iter().enumerate() {
        let x = tail_x - 70. * (m - 1 - j) as f32;
        if j > 0 {
            lines.push(Prim::Line {
                a: (x - 70., oy),
                b: (x, oy),
                stroke: Stroke::Dotted,
                color: ink.line,
                width: 1.5,
                t: 1.,
            });
        }
        node((x, oy), stage.state, &mut marks, &mut live);
        let (tc, fc) = label_colors(stage.state);
        let name = Stage {
            label: if stage.label.to_lowercase().contains("publish") {
                "Publish".into()
            } else {
                stage.label.clone()
            },
            state: if stage.state == CheckState::Skipped {
                CheckState::Queued
            } else {
                stage.state
            },
            ..(*stage).clone()
        };
        labels.push(styled(
            label(
                stage_label(&name),
                x,
                oy - 26.,
                Align::Center,
                LabelKind::Lane,
            ),
            tc,
            fc,
            None,
        ));
    }
    if m > 0 {
        let published = tail.iter().all(|s| s.state == CheckState::Pass);
        lines.push(Prim::Line {
            a: (tail_x, oy),
            b: (end_x, oy),
            stroke: if published {
                Stroke::Solid
            } else {
                Stroke::Dotted
            },
            color: if published && good {
                hue.green.mark
            } else {
                ink.line
            },
            width: 1.5,
            t: if published { draw } else { 1. },
        });
    }

    // The origin, the base's label and the end node with its pill.
    marks.insert(
        0,
        Prim::Dot {
            at: origin,
            r: 3.5,
            color: ink.dot,
        },
    );
    if k <= 2 && !card.head.is_empty() {
        labels.push(styled(
            label(&card.head, 25., oy - 26., Align::Left, LabelKind::Lane),
            ink.mono,
            ink.lane_pill,
            None,
        ));
    }
    let end = (end_x, oy);
    let pill_y = if k <= 2 { oy - 25. } else { oy - 32. };
    let state = pipeline_state(card);
    let pill = |text: &str, color: Hsla, fill: Hsla, border: Option<Hsla>| {
        styled(
            label(text, end_x + 7., pill_y, Align::Right, LabelKind::Status),
            color,
            fill,
            border,
        )
    };
    if good {
        marks.push(Prim::Node {
            at: end,
            r: 7.,
            color: hue.green.mark,
            glyph: Some(Glyph::Check),
        });
        labels.push(pill(
            &state,
            hue.green.text,
            hue.green.fill(),
            Some(hue.green.border()),
        ));
    } else if done && state == "Cancelled" {
        marks.push(Prim::Ring {
            at: end,
            r: 4.,
            color: ink.dot,
            dashed: true,
        });
        labels.push(pill(&state, ink.muted, ink.pill, None));
    } else if done {
        if release {
            // Nothing was published: the end stays a step not taken; the failed job says why.
            marks.push(Prim::Ring {
                at: end,
                r: 4.,
                color: ink.dot,
                dashed: true,
            });
        } else {
            marks.push(Prim::Halo {
                at: end,
                r: 11.,
                color: hue.red.mark.opacity(0.2),
            });
            marks.push(Prim::Node {
                at: end,
                r: 7.,
                color: hue.red.mark,
                glyph: Some(Glyph::Bang),
            });
        }
        labels.push(pill(
            &state,
            hue.red.text,
            hue.red.fill(),
            Some(hue.red.border()),
        ));
    } else {
        marks.push(Prim::Ring {
            at: end,
            r: 4.,
            color: ink.dot,
            dashed: true,
        });
        labels.push(pill(&state, ink.faint, ink.card, Some(ink.hair)));
    }
    lines.extend(marks);
    lines.extend(labels);
    Scene { prims: lines, live }
}

/// How tall a card's graph is, before it's laid out (a run with many jobs grows its band).
fn graph_height(card: &Card) -> f32 {
    match card.kind {
        Kind::Pr => 104.,
        _ => {
            let publish = card.kind == Kind::Release
                && card.stages.iter().any(is_publish)
                && card.stages.iter().any(|s| !is_publish(s));
            let k = card
                .stages
                .iter()
                .filter(|s| !publish || !is_publish(s))
                .count();
            if k >= 4 {
                (18. + (k - 1) as f32 * 20. + 25.).max(104.)
            } else {
                104.
            }
        }
    }
}

/// The graph band: feathered, with the scene painted over it.
fn graph(card: &Card, clock: f32, motion: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let ink = ink(cx);
    let hue = hues(cx);
    let height = graph_height(card);
    let draw = phase_progress(card, motion, 420.);
    let mono = theme.mono_font_family.clone();
    let sans = theme.font_family.clone();
    let painted = card.clone();
    let scene = canvas(
        move |_, _, _| {},
        move |bounds, _, window, cx| {
            let w = bounds.size.width.as_f32();
            let scene = match painted.kind {
                Kind::Pr => pr_scene(&painted, w, draw, &hue, &ink),
                Kind::Release | Kind::Run => {
                    let measure = |s: &str| {
                        window
                            .text_system()
                            .shape_line(
                                SharedString::from(s.to_string()),
                                px(12.),
                                &[TextRun {
                                    len: s.len(),
                                    font: font(mono.clone()),
                                    color: ink.muted,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }],
                                None,
                            )
                            .width
                            .as_f32()
                    };
                    pipeline_scene(&painted, w, draw, &hue, &ink, &measure)
                }
            };
            paint_scene(
                &scene.prims,
                bounds.origin,
                if motion { clock } else { 0.2 },
                motion,
                &ink,
                &mono,
                &sans,
                window,
                cx,
            );
            if motion && scene.live {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .inset_0();
    // The band: no hard rectangle, a soft fill fading out over ~10 px at every edge.
    // A shadow alone (no fill) is a rectangle blurred at every edge.
    let band = div()
        .absolute()
        .left(px(25.))
        .right(px(25.))
        .top(px(11.))
        .bottom(px(12.))
        .rounded(px(6.))
        .shadow(vec![BoxShadow {
            color: ink.band,
            offset: point(px(0.), px(0.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }]);
    div()
        .relative()
        .w_full()
        .h(px(height))
        .child(band)
        .child(scene)
        .into_any_element()
}

/// A crisp phase entrance: fast at the start, settled within the reference's short state morph.
fn phase_progress(card: &Card, motion: bool, duration_ms: f32) -> f32 {
    if !motion {
        return 1.;
    }
    let began = if card.phase_at_ms > 0 {
        card.phase_at_ms
    } else {
        card.created_ms
    };
    if began <= 0 {
        return 1.;
    }
    let t = clamp01((now_ms() - began).max(0) as f32 / duration_ms);
    1. - (1. - t).powi(5)
}

#[allow(clippy::too_many_arguments)]
fn paint_scene(
    prims: &[Prim],
    origin: Point<Pixels>,
    clock: f32,
    motion: bool,
    ink: &Ink,
    mono: &SharedString,
    sans: &SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    let white = hsla(0., 0., 1., 1.);
    for prim in prims {
        match prim {
            Prim::Line {
                a,
                b,
                stroke,
                color,
                width,
                t,
            } => {
                let t = clamp01(*t);
                if t <= 0. {
                    continue;
                }
                let b = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                seg(&[at(*a), at(b)], px(*width), *stroke, *color, window);
            }
            Prim::Curve {
                a,
                b,
                stroke,
                color,
                width,
                t,
            } => {
                let dx = (b.0 - a.0) / 2.;
                barc(
                    at(*a),
                    at((a.0 + dx, a.1)),
                    at((b.0 - dx, b.1)),
                    at(*b),
                    *t,
                    px(*width),
                    *stroke,
                    *color,
                    window,
                );
            }
            Prim::Dot { at: c, r, color } => dot(at(*c), *r, *color, window),
            Prim::Ring {
                at: c,
                r,
                color,
                dashed,
            } => {
                if *dashed {
                    dot(at(*c), *r, ink.surface, window);
                    circle(at(*c), *r, 1., true, *color, window);
                } else {
                    ring(at(*c), *r, 1.5, *color, ink.surface, window);
                }
            }
            Prim::Node {
                at: c,
                r,
                color,
                glyph,
            } => {
                dot(at(*c), *r, *color, window);
                match glyph {
                    Some(Glyph::Check) => tick(at(*c), *r, white, window),
                    Some(Glyph::Bang) => bang(at(*c), *r, white, window),
                    None => {}
                }
            }
            Prim::Halo { at: c, r, color } => dot(at(*c), *r, *color, window),
            Prim::Spinner { at: c, r, color } => {
                circle(at(*c), *r, 1.5, false, color.opacity(0.25), window);
                spin_arc(at(*c), *r, clock, *color, window);
            }
            Prim::Pulse { at: c, color } => {
                if motion {
                    let phase = (clock / 1.2).fract();
                    circle(
                        at(*c),
                        4. + 5. * phase,
                        1.5,
                        false,
                        color.opacity(0.5 * (1. - phase)),
                        window,
                    );
                }
            }
            Prim::Label {
                text,
                x,
                y,
                align,
                kind,
                color,
                fill: fill_color,
                border,
                strike,
            } => {
                let (family, weight, pad, h, radius) = match kind {
                    LabelKind::Lane | LabelKind::Branch => {
                        (mono.clone(), FontWeight::NORMAL, 5., 15., 4.)
                    }
                    LabelKind::Status => (sans.clone(), FontWeight::MEDIUM, 9., 18., 9.),
                };
                let run = TextRun {
                    len: text.len(),
                    font: Font {
                        weight,
                        ..font(family)
                    },
                    color: *color,
                    background_color: None,
                    underline: None,
                    strikethrough: strike.then_some(StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(*color),
                    }),
                };
                let line = window.text_system().shape_line(
                    SharedString::from(text.clone()),
                    px(12.),
                    &[run],
                    None,
                );
                let w = line.width.as_f32() + 2. * pad;
                let left = match align {
                    Align::Left => *x,
                    Align::Center => x - w / 2.,
                    Align::Right => x - w,
                };
                let bounds = Bounds::new(at((left, *y)), size(px(w), px(h)));
                window.paint_quad(fill(bounds, ink.surface).corner_radii(px(radius)));
                let mut quad = fill(bounds, *fill_color).corner_radii(px(radius));
                if let Some(b) = border {
                    quad = quad.border_widths(px(1.)).border_color(*b);
                }
                window.paint_quad(quad);
                let _ = line.paint(
                    at((left + pad, *y)),
                    px(h),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
    }
}

// ---------- footer ----------

/// The footer under the graph when there's something to say or do; `None` while the card is
/// just working (the band says that).
fn footer(
    ix: usize,
    card: &Card,
    actions: &Actions,
    hue: &Hues,
    ink: &Ink,
    cx: &App,
) -> Option<AnyElement> {
    let theme = cx.theme();
    let base = if card.base.is_empty() {
        "main"
    } else {
        &card.base
    };
    let when = card.phase_at_ms.max(card.created_ms);
    let lead = |icon: Icon, h: Hue| tile(icon, h.fill(), h.text);
    let warn = || Icon::new(IconName::TriangleAlert);
    let sub_text = |s: String| -> Option<AnyElement> {
        (!s.is_empty()).then(|| div().min_w_0().truncate().child(s).into_any_element())
    };
    let mut chips: Vec<AnyElement> = vec![];
    let mut buttons: Vec<AnyElement> = vec![];

    let (tile, title, sub): (AnyElement, String, Option<AnyElement>) = match card.kind {
        Kind::Pr => match card.phase {
            Phase::Conflicted => {
                let left = card.conflicts.len();
                let title = if card.resolving && left == 0 && !card.resolved.is_empty() {
                    "Conflicts resolved".to_string()
                } else {
                    format!("{} with {base}", conflicts_label(left))
                };
                for path in card.resolved.iter().take(4) {
                    chips.push(file_chip(path, Some(true), hue, ink, cx));
                }
                for path in card.conflicts.iter().take(4) {
                    chips.push(file_chip(path, None, hue, ink, cx));
                }
                let more = (card.resolved.len() + card.conflicts.len()).saturating_sub(8);
                if more > 0 {
                    chips.push(
                        div()
                            .text_size(px(12.))
                            .text_color(ink.muted)
                            .child(format!("+{more} more"))
                            .into_any_element(),
                    );
                }
                if card.resolving {
                    buttons.push(busy_pill(("pr-resolve", ix), "Resolving", ink, cx));
                } else if let Some(f) = actions.resolve.clone() {
                    buttons.push(dark_button(
                        ("pr-resolve", ix),
                        None,
                        "Resolve conflicts",
                        f,
                        ink,
                        cx,
                    ));
                }
                (
                    lead(warn(), hue.amber),
                    title,
                    sub_text(format!("The same lines changed on {base}")),
                )
            }
            Phase::Ready | Phase::Merging => {
                if card.phase == Phase::Merging {
                    buttons.push(busy_pill(("pr-merge", ix), "Merging", ink, cx));
                } else if let Some(f) = actions.merge.clone() {
                    buttons.push(dark_button(
                        ("pr-merge", ix),
                        Some(crate::assets::Lucide::GitMerge),
                        "Merge",
                        f,
                        ink,
                        cx,
                    ));
                }
                (
                    lead(Icon::new(IconName::Check), hue.green),
                    "Ready to merge".into(),
                    sub_text(ready_detail(card)),
                )
            }
            Phase::Merged => {
                let sub = if card.branch_deleted && !card.head.is_empty() {
                    h_flex()
                        .min_w_0()
                        .gap(px(4.))
                        .items_center()
                        .child(mono_pill(&card.head, ink, cx))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(format!("was deleted · {}", ago(when))),
                        )
                        .into_any_element()
                } else {
                    let n = card.commits.unwrap_or(0);
                    div()
                        .child(if n > 0 {
                            format!(
                                "{n} {} landed · {}",
                                if n == 1 { "commit" } else { "commits" },
                                ago(when)
                            )
                        } else {
                            ago(when)
                        })
                        .into_any_element()
                };
                (
                    lead(Icon::new(crate::assets::Lucide::GitMerge), hue.purple),
                    format!("Merged into {base}"),
                    Some(sub),
                )
            }
            Phase::Closed => (
                tile_neutral(Icon::new(IconName::CircleX), ink),
                "Closed unmerged".into(),
                sub_text(ago(when)),
            ),
            Phase::Failed => (
                lead(warn(), hue.red),
                "Couldn't open the pull request".into(),
                sub_text(card.step.clone()),
            ),
            // A merge that failed, or checks that are red: what went wrong, and where to look.
            Phase::Open
                if card.step.starts_with("Merge failed") || card.step.contains("failing") =>
            {
                let (head, rest) = card
                    .step
                    .split_once(": ")
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .unwrap_or((card.step.clone(), String::new()));
                if let Some(f) = actions.open.clone() {
                    buttons.push(light_button(("pr-view", ix), "View on GitHub", f, ink));
                }
                (lead(warn(), hue.red), head, sub_text(rest))
            }
            _ => return None,
        },
        Kind::Run | Kind::Release => match card.phase {
            Phase::Passed | Phase::Published => {
                let (title, sub) = if card.kind == Kind::Run {
                    let names: Vec<&str> = card.stages.iter().map(|s| s.label.as_str()).collect();
                    let took = dur_label(run_ms(card));
                    (
                        if names.is_empty() {
                            "Passed".to_string()
                        } else {
                            "All checks passed".to_string()
                        },
                        if names.is_empty() {
                            took
                        } else {
                            format!("{} · {took}", and_list(&names))
                        },
                    )
                } else {
                    let channel = card.channel.as_str();
                    (
                        if channel.is_empty() || channel == "stable" {
                            "Published".to_string()
                        } else {
                            format!("Published to {channel}")
                        },
                        format!(
                            "{} downloads are up · {}",
                            platforms_for_sub(card),
                            ago(when)
                        ),
                    )
                };
                let link = if card.kind == Kind::Release {
                    actions
                        .notes
                        .clone()
                        .map(|f| ("Release notes", f))
                        .or_else(|| actions.open.clone().map(|f| ("Open run", f)))
                } else {
                    actions.open.clone().map(|f| ("Open run", f))
                };
                if let Some((text, f)) = link {
                    buttons.push(link_pill(("pr-link", ix), text, hue.green, f));
                }
                (
                    lead(Icon::new(IconName::Check), hue.green),
                    title,
                    sub_text(sub),
                )
            }
            Phase::Failed | Phase::Closed => {
                let skipped_or_failed: Vec<&Stage> = card
                    .stages
                    .iter()
                    .filter(|s| matches!(s.state, CheckState::Fail | CheckState::Skipped))
                    .collect();
                if skipped_or_failed.len() > 1 {
                    for s in skipped_or_failed.iter().take(5) {
                        chips.push(step_chip(s, hue, ink));
                    }
                }
                if let Some(f) = actions.rerun.clone() {
                    buttons.push(light_button(("pr-rerun", ix), "Re-run", f, ink));
                }
                if let Some(f) = actions.open.clone() {
                    buttons.push(dark_button(("pr-logs", ix), None, "View log", f, ink, cx));
                }
                (
                    lead(warn(), hue.red),
                    failure_title(card),
                    sub_text(failure_detail(card)),
                )
            }
            _ => return None,
        },
    };

    Some(
        v_flex()
            .id(("pr-state-row", ix))
            .test_support()
            .pl(px(15.))
            .pr(px(16.))
            .pt(px(23.))
            .pb(px(if chips.is_empty() { 11. } else { 15. }))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(13.))
                            .items_start()
                            .child(div().mt(px(2.)).child(tile))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(14.))
                                            .line_height(px(18.))
                                            .font_medium()
                                            .text_color(theme.foreground)
                                            .child(title),
                                    )
                                    .when_some(sub, |el, sub| {
                                        el.child(
                                            h_flex()
                                                .min_w_0()
                                                .h(px(17.))
                                                .items_center()
                                                .text_size(px(13.))
                                                .line_height(px(17.))
                                                .text_color(ink.muted)
                                                .child(sub),
                                        )
                                    }),
                            ),
                    )
                    .when(!buttons.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .gap(px(6.))
                                .items_center()
                                .children(buttons),
                        )
                    }),
            )
            .when(!chips.is_empty(), |el| {
                el.child(
                    h_flex()
                        .pl(px(35.))
                        .mt(px(6.))
                        .gap(px(4.))
                        .flex_wrap()
                        .items_center()
                        .children(chips),
                )
            })
            .into_any_element(),
    )
}

fn tile_neutral(icon: Icon, ink: &Ink) -> AnyElement {
    tile(icon, ink.pill, ink.muted)
}

/// "Clippy, Test (macOS) and Test (Windows)".
fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "Mac and Windows" for a published release's line; "The" when its jobs don't say.
fn platforms_for_sub(card: &Card) -> String {
    let p = platforms(card);
    if p.contains('+') {
        p.replace(" + ", " and ")
    } else if p.ends_with("job") || p.ends_with("jobs") || p.is_empty() {
        "The".into()
    } else {
        p
    }
}

/// Under Ready to merge: no conflicts, and which checks passed.
fn ready_detail(card: &Card) -> String {
    let names: Vec<&str> = card
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Pass)
        .map(|c| c.name.as_str())
        .collect();
    match names.len() {
        0 => "No conflicts".into(),
        1..=3 => format!("No conflicts · {} passed", and_list(&names)),
        n => format!("No conflicts · all {n} checks passed"),
    }
}

/// A conflicting file: its language's monogram and name; a green ✓ once resolved.
fn file_chip(path: &str, resolved: Option<bool>, hue: &Hues, ink: &Ink, cx: &App) -> AnyElement {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let tag = lang_tag(path);
    let full = path.to_string();
    h_flex()
        .id(SharedString::from(format!("pr-file-{path}")))
        .flex_none()
        .h(px(21.))
        .px(px(9.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(ink.hair.opacity(0.8))
        .bg(ink.card)
        .text_size(px(13.))
        .when(!tag.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(10.))
                    .font_bold()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(hue.blue.text.opacity(0.8))
                    .child(tag),
            )
        })
        .child(div().text_color(ink.fg.opacity(0.72)).child(name))
        .when(resolved == Some(true), |el| {
            el.child(
                Icon::new(IconName::Check)
                    .size(px(11.))
                    .text_color(hue.green.text),
            )
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
        })
        .into_any_element()
}

/// A job that failed (✕ and how long it ran) or was skipped (–).
fn step_chip(stage: &Stage, hue: &Hues, ink: &Ink) -> AnyElement {
    let (mark, color, what) = match stage.state {
        CheckState::Fail => {
            let ms = stage_ms(stage);
            (
                "✕",
                hue.red.text,
                if ms > 0 {
                    format!("{} · {}", stage.label, dur_label(ms))
                } else {
                    stage.label.clone()
                },
            )
        }
        _ => ("–", ink.muted, format!("{} · skipped", stage.label)),
    };
    h_flex()
        .flex_none()
        .h(px(21.))
        .px(px(9.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(ink.hair.opacity(0.8))
        .bg(ink.card)
        .text_size(px(13.))
        .child(div().text_size(px(11.)).text_color(color).child(mark))
        .child(div().text_color(ink.fg.opacity(0.72)).child(what))
        .into_any_element()
}

/// The short language tag a file chip leads with ("TS", "RS"), from the path's extension.
fn lang_tag(path: &str) -> String {
    path.rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_uppercase())
        .filter(|e| e.len() <= 4)
        .unwrap_or_default()
}

/// The footer's one action while it asks for a click: the page's ink as its fill.
fn dark_button(
    id: impl Into<ElementId>,
    icon: Option<crate::assets::Lucide>,
    label: &'static str,
    on_click: Act,
    ink: &Ink,
    cx: &App,
) -> AnyElement {
    let (bg, fg) = (ink.fg, cx.theme().background);
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .h(px(if icon.is_some() { 26. } else { 28. }))
        .px(px(14.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .bg(bg.opacity(0.92))
        .hover(move |s| s.bg(bg.opacity(0.8)))
        .active(move |s| s.bg(bg))
        .cursor_pointer()
        .text_size(px(13.))
        .font_medium()
        .text_color(fg)
        .when_some(icon, |el, icon| {
            el.child(Icon::new(icon).size(px(12.)).text_color(fg))
        })
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

/// A second, quieter action beside the dark one (Re-run).
fn light_button(
    id: impl Into<ElementId>,
    label: &'static str,
    on_click: Act,
    ink: &Ink,
) -> AnyElement {
    let (bg, fg) = (ink.pill, ink.fg);
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .items_center()
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(fg.opacity(0.08)))
        .active(move |s| s.bg(fg.opacity(0.11)))
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(fg.opacity(0.85))
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

/// What's left to do with something that finished well: open it on GitHub. A quiet link on its
/// verdict's tint, with the arrow that says it leaves Trek.
fn link_pill(id: impl Into<ElementId>, label: &'static str, hue: Hue, on_click: Act) -> AnyElement {
    let tint = hue.mark;
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .h(px(26.))
        .px(px(10.))
        .gap(px(3.))
        .items_center()
        .rounded_full()
        .bg(tint.opacity(0.09))
        .hover(move |s| s.bg(tint.opacity(0.15)))
        .cursor_pointer()
        .text_size(px(13.))
        .font_medium()
        .text_color(hue.text)
        .child(label)
        .child(
            Icon::new(crate::assets::Lucide::ArrowUpRight)
                .size(px(11.))
                .text_color(hue.text),
        )
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

/// The same place once clicked: a quiet grey pill with a small spinner.
fn busy_pill(id: impl Into<ElementId>, label: &'static str, ink: &Ink, cx: &App) -> AnyElement {
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .gap(px(6.))
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .rounded_full()
        .bg(ink.pill)
        .text_size(px(13.))
        .text_color(ink.muted)
        .child(ring_spinner(12., ink.muted, cx))
        .child(label)
        .into_any_element()
}

/// "45s", "3m 12s", "1h 4m" — coarse, like the working bar's.
/// How long a run took: to its end once it has one, else up to now.
fn run_ms(card: &Card) -> i64 {
    let end = if card.ended_ms > 0 {
        card.ended_ms
    } else {
        now_ms()
    };
    end - card.started_ms
}

fn dur_label(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, s % 3600 / 60)
    }
}

/// "just now", "2m ago" — how long ago `ms` was.
fn ago(ms: i64) -> String {
    let s = ((now_ms() - ms) / 1000).max(0);
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
    }
}

/// A watcher's step that only restates the failure ("Failed: Test (Windows)").
fn generic_failure(step: &str) -> bool {
    let step = step.trim();
    step.is_empty()
        || step.starts_with("Failed: ")
        || step == "Run failed"
        || step == "Release workflow failed"
}

/// Under a failed run's or release's title: what the step line adds (the error), or how the
/// other jobs did when it adds nothing.
fn failure_detail(card: &Card) -> String {
    if !generic_failure(&card.step) && card.step.trim() != failure_title(card) {
        return card.step.trim().to_string();
    }
    let total = card.stages.len();
    let passed = card
        .stages
        .iter()
        .filter(|s| s.state == CheckState::Pass)
        .count();
    match (passed, total) {
        (_, 0) => String::new(),
        (p, t) if p + 1 == t => "The other jobs passed".to_string(),
        (0, _) => "No job passed".to_string(),
        (p, t) => format!("{p} of {t} jobs passed"),
    }
}

/// A failed run or release's title: the job that failed, or how many did.
fn failed_jobs(card: &Card) -> String {
    let failed: Vec<&Stage> = card
        .stages
        .iter()
        .filter(|s| s.state == CheckState::Fail)
        .collect();
    match failed.as_slice() {
        [] => "Something failed".to_string(),
        [one] => format!("{} failed", one.label),
        many => format!("{} jobs failed", many.len()),
    }
}

/// A failed run or release's title: the job that failed when one did ("Windows build failed",
/// the error going under it); else the watcher's or command's own explanation ("Cancelled", a
/// release script's error).
fn failure_title(card: &Card) -> String {
    if card.stages.iter().any(|s| s.state == CheckState::Fail) || generic_failure(&card.step) {
        failed_jobs(card)
    } else {
        card.step.trim().to_string()
    }
}

// ---------- painted graph helpers ----------

fn clamp01(v: f32) -> f32 {
    v.clamp(0., 1.)
}

fn bcubic(
    p0: Point<Pixels>,
    c1: Point<Pixels>,
    c2: Point<Pixels>,
    p1: Point<Pixels>,
    t: f32,
) -> Point<Pixels> {
    let u = 1. - t;
    let (x0, y0) = (p0.x.as_f32(), p0.y.as_f32());
    let (c1x, c1y) = (c1.x.as_f32(), c1.y.as_f32());
    let (c2x, c2y) = (c2.x.as_f32(), c2.y.as_f32());
    let (x1, y1) = (p1.x.as_f32(), p1.y.as_f32());
    point(
        px(u * u * u * x0 + 3. * u * u * t * c1x + 3. * u * t * t * c2x + t * t * t * x1),
        px(u * u * u * y0 + 3. * u * u * t * c1y + 3. * u * t * t * c2y + t * t * t * y1),
    )
}

/// A polyline: dashed 4 / 2.5 as the reference's connectors, dotted 1.5 / 5 for what's to come.
fn seg(points: &[Point<Pixels>], width: Pixels, stroke: Stroke, color: Hsla, window: &mut Window) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(width);
    match stroke {
        Stroke::Solid => {}
        Stroke::Dashed => builder = builder.dash_array(&[px(4.), px(2.5)]),
        Stroke::Dotted => builder = builder.dash_array(&[px(1.5), px(4.5)]),
    }
    builder.move_to(points[0]);
    for point in &points[1..] {
        builder.line_to(*point);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[allow(clippy::too_many_arguments)]
fn barc(
    p0: Point<Pixels>,
    c1: Point<Pixels>,
    c2: Point<Pixels>,
    p1: Point<Pixels>,
    t: f32,
    width: Pixels,
    stroke: Stroke,
    color: Hsla,
    window: &mut Window,
) {
    let t = clamp01(t);
    if t <= 0. {
        return;
    }
    let points: Vec<Point<Pixels>> = (0..=32)
        .map(|i| bcubic(p0, c1, c2, p1, t * i as f32 / 32.))
        .collect();
    seg(&points, width, stroke, color, window);
}

fn dot(at: Point<Pixels>, radius: f32, color: Hsla, window: &mut Window) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - radius), px(at.y.as_f32() - radius)),
                size(px(2. * radius), px(2. * radius)),
            ),
            color,
        )
        .corner_radii(px(radius)),
    );
}

fn ring(
    at: Point<Pixels>,
    radius: f32,
    width: f32,
    color: Hsla,
    fill_color: Hsla,
    window: &mut Window,
) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - radius), px(at.y.as_f32() - radius)),
                size(px(2. * radius), px(2. * radius)),
            ),
            fill_color,
        )
        .corner_radii(px(radius))
        .border_widths(px(width))
        .border_color(color),
    );
}

/// A circle's outline as a path (dashed for a node still to come).
fn circle(
    at: Point<Pixels>,
    radius: f32,
    width: f32,
    dashed: bool,
    color: Hsla,
    window: &mut Window,
) {
    let steps = 40;
    let points: Vec<Point<Pixels>> = (0..=steps)
        .map(|i| {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            point(at.x + px(radius * a.cos()), at.y + px(radius * a.sin()))
        })
        .collect();
    let mut builder = PathBuilder::stroke(px(width));
    if dashed {
        builder = builder.dash_array(&[px(2.), px(1.6)]);
    }
    builder.move_to(points[0]);
    for p in &points[1..] {
        builder.line_to(*p);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A ✓ sized to a node of radius `r`.
fn tick(at: Point<Pixels>, r: f32, color: Hsla, window: &mut Window) {
    let (x, y) = (at.x.as_f32(), at.y.as_f32());
    let s = r / 7.;
    seg(
        &[
            point(px(x - 3.2 * s), px(y + 0.2 * s)),
            point(px(x - 1. * s), px(y + 2.4 * s)),
            point(px(x + 3.3 * s), px(y - 2.3 * s)),
        ],
        px(1.6 * s.max(0.85)),
        Stroke::Solid,
        color,
        window,
    );
}

/// A ! sized to a node of radius `r`.
fn bang(at: Point<Pixels>, r: f32, color: Hsla, window: &mut Window) {
    let (x, y) = (at.x.as_f32(), at.y.as_f32());
    let s = r / 7.;
    seg(
        &[point(px(x), px(y - 3.6 * s)), point(px(x), px(y + 0.8 * s))],
        px(2. * s),
        Stroke::Solid,
        color,
        window,
    );
    dot(at + point(px(0.), px(3.2 * s)), 1.1 * s, color, window);
}

fn spin_arc(at: Point<Pixels>, radius: f32, clock: f32, color: Hsla, window: &mut Window) {
    let mut builder = PathBuilder::stroke(px(1.5));
    let steps = 24;
    let start = clock * std::f32::consts::TAU;
    for i in 0..=steps * 3 / 4 {
        let angle = start + (i as f32 / steps as f32) * std::f32::consts::TAU;
        let point = point(
            at.x + px(radius) * angle.cos(),
            at.y + px(radius) * angle.sin(),
        );
        if i == 0 {
            builder.move_to(point);
        } else {
            builder.line_to(point);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A card in `state` with made-up contents, for design review (`shots`' `pr-demo`).
/// Nothing here is fetched: it's drawn as given.
#[cfg(any(test, feature = "shots"))]
pub fn demo(state: &str) -> Card {
    let now = now_ms();
    let base = Card {
        kind: Kind::Pr,
        title: "Refresh sessions in the auth middleware".into(),
        number: Some(128),
        url: Some("https://github.com/dokyit/Trek/pull/128".into()),
        head: "feat/session-refresh".into(),
        base: "main".into(),
        repo_dir: ".".into(),
        repo: Some("dokyit/Trek".into()),
        commits: Some(3),
        history: vec![
            Commit {
                title: "Add session refresh tests".into(),
                oid: "d29f50".into(),
                author: "You".into(),
                committed_ms: now - 20_000,
            },
            Commit {
                title: "Refresh sessions near expiry".into(),
                oid: "c70a92".into(),
                author: "You".into(),
                committed_ms: now - 25 * 60_000,
            },
            Commit {
                title: "Read session expiry from the token".into(),
                oid: "1f8e3b".into(),
                author: "You".into(),
                committed_ms: now - 40 * 60_000,
            },
        ],
        added: Some(128),
        removed: Some(36),
        created_ms: now - 300_000,
        phase_at_ms: now,
        ..Default::default()
    };
    let conflicts = vec!["src/middleware.ts".into(), "src/auth.ts".into()];
    let checks = vec![
        Check {
            name: "build (macos)".into(),
            state: CheckState::Pass,
        },
        Check {
            name: "build (windows)".into(),
            state: CheckState::Running,
        },
        Check {
            name: "clippy".into(),
            state: CheckState::Queued,
        },
    ];
    match state {
        "creating" => Card {
            phase: Phase::Creating,
            step: "Committing the last changes".into(),
            number: None,
            commits: None,
            added: None,
            removed: None,
            ..base
        },
        "pushing" => Card {
            phase: Phase::Creating,
            step: "Pushing 3 commits to origin".into(),
            number: None,
            ..base
        },
        "opening" => Card {
            phase: Phase::Creating,
            step: "Opening the pull request".into(),
            number: None,
            ..base
        },
        "fetching" => Card {
            phase: Phase::Open,
            step: "Fetching main".into(),
            ..base
        },
        "checking" => Card {
            phase: Phase::Checking,
            step: "Checking it merges cleanly".into(),
            checks,
            ..base
        },
        "conflicted" => Card {
            phase: Phase::Conflicted,
            step: "2 conflicts with main".into(),
            conflicts,
            ..base
        },
        "resolving" => Card {
            phase: Phase::Conflicted,
            resolving: true,
            step: "Resolving conflict 2 of 2".into(),
            resolved: vec!["src/middleware.ts".into()],
            conflicts: vec!["src/auth.ts".into()],
            commits: Some(4),
            ..base
        },
        "resolved" => Card {
            phase: Phase::Conflicted,
            resolving: true,
            step: "Conflicts resolved".into(),
            resolved: conflicts,
            conflicts: vec![],
            commits: Some(4),
            ..base
        },
        "ready" => Card {
            phase: Phase::Ready,
            step: "Ready to merge".into(),
            resolved: conflicts.clone(),
            checks: vec![
                Check {
                    name: "build (macos)".into(),
                    state: CheckState::Pass,
                },
                Check {
                    name: "build (windows)".into(),
                    state: CheckState::Pass,
                },
                Check {
                    name: "clippy".into(),
                    state: CheckState::Pass,
                },
            ],
            commits: Some(4),
            ..base
        },
        "merging" => Card {
            phase: Phase::Merging,
            resolved: conflicts.clone(),
            merging: true,
            step: "Merging the pull request".into(),
            checks: vec![],
            commits: Some(4),
            ..base
        },
        "merged" => Card {
            phase: Phase::Merged,
            branch_deleted: true,
            resolved: conflicts.clone(),
            step: String::new(),
            commits: Some(4),
            ..base
        },
        "closed" => Card {
            phase: Phase::Closed,
            step: String::new(),
            ..base
        },
        "failed" => Card {
            phase: Phase::Failed,
            step: "remote: Permission to dokyit/Trek.git denied to trek-bot".into(),
            number: None,
            ..base
        },
        "release" | "release-published" | "release-failed" => {
            let job = |label: &str, state: CheckState, from_s: i64, took_s: i64| Stage {
                label: label.into(),
                state,
                started_ms: if from_s > 0 { now - from_s * 1000 } else { 0 },
                ended_ms: if state.pending() || from_s == 0 {
                    0
                } else {
                    now - (from_s - took_s) * 1000
                },
            };
            let (phase, step, stages, took) = match state {
                "release-published" => (
                    Phase::Published,
                    "Published",
                    vec![
                        job("macOS app", CheckState::Pass, 791, 552),
                        job("Windows build", CheckState::Pass, 791, 700),
                        job("Windows publish", CheckState::Pass, 91, 91),
                    ],
                    791,
                ),
                "release-failed" => (
                    Phase::Failed,
                    "error[E0433]: failed to resolve: use of undeclared crate `windows`",
                    vec![
                        job("macOS app", CheckState::Pass, 422, 300),
                        job("Windows build", CheckState::Fail, 422, 362),
                        job("Windows publish", CheckState::Skipped, 0, 0),
                    ],
                    422,
                ),
                _ => (
                    Phase::Releasing,
                    "Release workflow running",
                    vec![
                        job("macOS app", CheckState::Pass, 580, 552),
                        job("Windows build", CheckState::Running, 380, 0),
                        job("Windows publish", CheckState::Queued, 0, 0),
                    ],
                    580,
                ),
            };
            Card {
                kind: Kind::Release,
                title: "Release v0.4.15-nightly.20261025".into(),
                version: "0.4.15-nightly.20261025".into(),
                channel: "nightly".into(),
                number: None,
                url: Some("https://github.com/dokyit/Trek/actions/runs/1".into()),
                head: "main".into(),
                commits: None,
                added: None,
                removed: None,
                phase,
                step: step.into(),
                stages,
                workflow: true,
                run_id: Some(1),
                started_ms: now - took * 1000,
                created_ms: now - took * 1000,
                phase_at_ms: now,
                ..base
            }
        }
        "run" | "run-running" | "run-passed" | "run-failed" => {
            let job = |label: &str, state: CheckState, took_s: i64| Stage {
                label: label.into(),
                state,
                started_ms: now - 1_713_000,
                ended_ms: if state.pending() {
                    0
                } else {
                    now - 1_713_000 + took_s * 1000
                },
            };
            let (phase, step, stages, took) = match state {
                "run-passed" => (
                    Phase::Passed,
                    "Passed",
                    vec![
                        job("Clippy", CheckState::Pass, 250),
                        job("Test (macOS)", CheckState::Pass, 1102),
                        job("Test (Windows)", CheckState::Pass, 1681),
                    ],
                    1713,
                ),
                "run-failed" => (
                    Phase::Failed,
                    "Failed: Test (Windows)",
                    vec![
                        job("Clippy", CheckState::Pass, 250),
                        job("Test (macOS)", CheckState::Pass, 1102),
                        job("Test (Windows)", CheckState::Fail, 1450),
                    ],
                    1450,
                ),
                _ => (
                    Phase::Checking,
                    "Watching 1 job",
                    vec![
                        job("Clippy", CheckState::Pass, 250),
                        job("Test (macOS)", CheckState::Pass, 1102),
                        job("Test (Windows)", CheckState::Running, 0),
                    ],
                    1265,
                ),
            };
            Card {
                kind: Kind::Run,
                title: "CI".into(),
                number: Some(39779843418),
                url: Some("https://github.com/dokyit/Trek/actions/runs/39779843418".into()),
                head: "main".into(),
                base: String::new(),
                subject: "Side chat and AI side bar: one VS Code-style chat panel".into(),
                event: "push".into(),
                commits: None,
                added: None,
                removed: None,
                phase,
                step: step.into(),
                started_ms: now - 1_713_000,
                ended_ms: if phase.terminal() {
                    now - 1_713_000 + took * 1000
                } else {
                    0
                },
                stages,
                ..base
            }
        }
        // "open"
        _ => Card {
            phase: Phase::Open,
            step: String::new(),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: that would bring in gpui_kit's own `test` macro.
    use super::{
        Align, Card, CheckState, Glyph, Hue, Hues, Ink, JobJson, Kind, LabelKind, Phase, PrView,
        Prim, Scene, Stage, Stroke, WorkflowRun, apply_conflicts, conflicting_files,
        failure_detail, failure_title, graph_height, is_workflow, job_stages, notes_url, now_ms,
        pick_run, pipeline_scene, pr_from_view, pr_scene, release_card, release_run_card, rerun_id,
        rerun_started, run_ms, segments, stage_label,
    };
    use std::path::Path;

    /// A watch the command line runs after `cd`-ing elsewhere asks `gh` there, not in the
    /// thread's folder (a chat outside any project: a repository with no remote, where `gh`
    /// finds no run and the card never left "Starting").
    #[test]
    fn a_watch_after_a_cd_asks_gh_where_the_cd_went() {
        use super::command_dir;
        let here = std::env::temp_dir().join("thread");
        let home = trek_core::paths::home();
        assert_eq!(
            command_dir("cd ~/Trek && gh run watch 40359338793 --exit-status", &here),
            home.join("Trek")
        );
        assert_eq!(
            command_dir("cd repo; pushd sub && gh run watch 1", &here),
            here.join("repo").join("sub")
        );
        assert_eq!(
            command_dir("gh run watch 1", &here),
            here,
            "no cd: the thread's folder"
        );
        assert_eq!(
            command_dir("gh run watch 1 && cd elsewhere", &here),
            here,
            "a cd after the watch doesn't move it"
        );
        assert_eq!(
            command_dir(r#"Set-Location -Path "other dir"; gh run watch 1"#, &here),
            here.join("other dir")
        );
        if cfg!(windows) {
            assert_eq!(
                command_dir("cd /c/Users/me/Trek && gh run watch 1", &here),
                Path::new(r"C:\Users\me\Trek")
            );
        }
    }

    fn words(s: &str) -> Vec<String> {
        segments(s).iter().map(|s| s.trim().to_string()).collect()
    }

    fn hues() -> Hues {
        let hue = |m: u32, t: u32| Hue {
            mark: gpui_kit::rgb(m).into(),
            text: gpui_kit::rgb(t).into(),
            wash: 0.075,
        };
        Hues {
            blue: hue(0x4A8CF0, 0x3B5BAB),
            amber: hue(0xE9A80E, 0xA5650F),
            green: hue(0x2FC261, 0x1E8A4C),
            red: hue(0xE5484D, 0xC6283D),
            purple: hue(0x8B5CF6, 0x6227AD),
        }
    }

    fn ink() -> Ink {
        let g = |l: f32| gpui_kit::hsla(0., 0., l, 1.);
        Ink {
            fg: g(0.1),
            muted: g(0.45),
            dot: g(0.69),
            line: g(0.89),
            merge_in: g(0.84),
            band: g(0.98),
            pill: g(0.96),
            lane_pill: g(0.94),
            hair: g(0.89),
            card: g(1.),
            surface: g(0.98),
            faint: g(0.65),
            mono: g(0.55),
        }
    }

    fn texts(scene: &Scene) -> Vec<(String, f32, f32, Align, LabelKind)> {
        scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Label {
                    text,
                    x,
                    y,
                    align,
                    kind,
                    ..
                } => Some((text.clone(), *x, *y, *align, *kind)),
                _ => None,
            })
            .collect()
    }

    fn status(scene: &Scene) -> Option<(String, f32, f32)> {
        texts(scene)
            .into_iter()
            .find(|t| t.4 == LabelKind::Status)
            .map(|t| (t.0, t.1, t.2))
    }

    fn end_node(scene: &Scene, at: (f32, f32)) -> Option<Prim> {
        scene
            .prims
            .iter()
            .rev()
            .find(|p| match p {
                Prim::Node { at: a, .. }
                | Prim::Dot { at: a, .. }
                | Prim::Ring { at: a, .. }
                | Prim::Spinner { at: a, .. } => *a == at,
                _ => false,
            })
            .cloned()
    }

    fn pr(phase: Phase) -> Card {
        Card {
            kind: Kind::Pr,
            title: "Refresh sessions in the auth middleware".into(),
            number: Some(128),
            head: "feat/session-refresh".into(),
            base: "main".into(),
            commits: Some(3),
            phase,
            ..Default::default()
        }
    }

    /// The x of everything a scene draws, labels at their anchor.
    fn xs(scene: &Scene) -> Vec<f32> {
        scene
            .prims
            .iter()
            .flat_map(|p| match p {
                Prim::Line { a, b, .. } | Prim::Curve { a, b, .. } => vec![a.0, b.0],
                Prim::Dot { at, .. }
                | Prim::Ring { at, .. }
                | Prim::Node { at, .. }
                | Prim::Halo { at, .. }
                | Prim::Spinner { at, .. }
                | Prim::Pulse { at, .. } => vec![at.0],
                Prim::Label { x, .. } => vec![*x],
            })
            .collect()
    }

    #[test]
    fn a_conflicted_pull_request_is_drawn_where_the_reference_draws_it() {
        let card = Card {
            conflicts: vec!["src/middleware.ts".into(), "src/auth.ts".into()],
            ..pr(Phase::Conflicted)
        };
        let scene = pr_scene(&card, 600., 1., &hues(), &ink());
        // The verdict on the end node, 42 px in from the right, on the base's lane (card y 99).
        assert!(matches!(
            end_node(&scene, (558., 42.)),
            Some(Prim::Node {
                glyph: Some(Glyph::Bang),
                r: 7.,
                ..
            })
        ));
        // Its pill ends 7 px past the node, 18 px tall, 6 px above it.
        assert_eq!(status(&scene), Some(("2 conflicts".into(), 565., 17.)));
        let labels = texts(&scene);
        assert!(labels.contains(&("main".into(), 25., 16., Align::Left, LabelKind::Lane)));
        // The branch's name under its first two commits (x 184 and 259).
        assert!(labels.contains(&(
            "feat/session-refresh".into(),
            221.5,
            86.,
            Align::Center,
            LabelKind::Branch
        )));
        assert!(scene.prims.iter().any(|p| matches!(
            p,
            Prim::Dot { at: (x, y), .. } if *x == 334. && *y == 79.
        )));
        // Nothing moves once it's drawn in: the conflict waits on the user, still.
        assert!(!scene.live);
    }

    #[test]
    fn resolving_draws_the_merge_commit_and_counts_down() {
        let card = Card {
            resolving: true,
            conflicts: vec!["src/auth.ts".into()],
            resolved: vec!["src/middleware.ts".into()],
            commits: Some(4),
            ..pr(Phase::Conflicted)
        };
        let scene = pr_scene(&card, 600., 1., &hues(), &ink());
        assert!(scene.prims.iter().any(|p| matches!(
            p,
            Prim::Ring { at, dashed: false, .. } if *at == (453., 79.)
        )));
        assert!(scene.prims.iter().any(|p| matches!(p, Prim::Pulse { .. })));
        assert!(scene.live, "the merge commit is being made");
        assert_eq!(status(&scene).unwrap().0, "1 conflict");

        let done = Card {
            conflicts: vec![],
            resolved: vec!["src/middleware.ts".into(), "src/auth.ts".into()],
            ..card
        };
        let scene = pr_scene(&done, 600., 1., &hues(), &ink());
        assert_eq!(status(&scene).unwrap().0, "Resolved");
        assert!(!scene.prims.iter().any(|p| matches!(p, Prim::Pulse { .. })));
    }

    #[test]
    fn a_merged_pull_request_ends_on_a_purple_node_and_strikes_its_deleted_branch() {
        let card = Card {
            branch_deleted: true,
            ..pr(Phase::Merged)
        };
        let h = hues();
        let scene = pr_scene(&card, 600., 1., &h, &ink());
        assert!(matches!(
            end_node(&scene, (558., 42.)),
            Some(Prim::Dot { r: 5.5, color, .. }) if color == h.purple.mark
        ));
        assert_eq!(status(&scene).unwrap().0, "Merged");
        assert!(scene.prims.iter().any(|p| matches!(
            p,
            Prim::Label {
                kind: LabelKind::Branch,
                strike: true,
                ..
            }
        )));
        assert!(scene.prims.iter().any(|p| matches!(
            p,
            Prim::Curve { stroke: Stroke::Solid, color, b, .. }
                if *color == h.purple.mark && *b == (558., 42.)
        )));
    }

    #[test]
    fn a_narrow_card_keeps_its_graph_inside_it() {
        for phase in [
            Phase::Checking,
            Phase::Conflicted,
            Phase::Ready,
            Phase::Merged,
        ] {
            let card = Card {
                resolved: vec!["a.ts".into()],
                ..pr(phase)
            };
            let scene = pr_scene(&card, 420., 1., &hues(), &ink());
            assert!(
                xs(&scene).iter().all(|x| *x >= 0. && *x <= 420. - 35.),
                "{phase:?}: {:?}",
                xs(&scene)
            );
            assert!(end_node(&scene, (378., 42.)).is_some(), "{phase:?}");
        }
    }

    fn stage(label: &str, state: CheckState, took_s: i64) -> Stage {
        let now = now_ms();
        Stage {
            label: label.into(),
            state,
            started_ms: if took_s > 0 { now - took_s * 1000 } else { 0 },
            ended_ms: if took_s > 0 && !state.pending() {
                now
            } else {
                0
            },
        }
    }

    fn measure(s: &str) -> f32 {
        s.chars().count() as f32 * 7.2
    }

    #[test]
    fn a_release_builds_side_by_side_and_publishes_after() {
        let card = Card {
            kind: Kind::Release,
            title: "Release v0.4.15-nightly.20261025".into(),
            version: "0.4.15-nightly.20261025".into(),
            channel: "nightly".into(),
            head: "main".into(),
            phase: Phase::Releasing,
            stages: vec![
                stage("macOS app", CheckState::Pass, 552),
                stage("Windows build", CheckState::Running, 380),
                stage("Windows publish", CheckState::Queued, 0),
            ],
            ..Default::default()
        };
        let scene = pipeline_scene(&card, 600., 1., &hues(), &ink(), &measure);
        let labels = texts(&scene);
        // The builds: the first on the base's lane (label above), the second a lane below.
        assert!(
            labels
                .iter()
                .any(|l| l.0 == "macOS app · 9m 12s" && l.2 == 16.)
        );
        assert!(
            labels
                .iter()
                .any(|l| l.0 == "Windows build · 6m" && l.2 == 90.)
        );
        // Publish after both, 149 px before the end node, said plainly.
        assert!(labels.contains(&("Publish".into(), 409., 16., Align::Center, LabelKind::Lane)));
        assert_eq!(status(&scene), Some(("Building 2 of 3".into(), 565., 17.)));
        assert!(scene.live, "a job is running");
        assert_eq!(graph_height(&card), 104.);
        // Every job's node sits before the publish step.
        assert!(scene.prims.iter().all(|p| match p {
            Prim::Spinner { at, .. } => at.0 < 409. - 39.,
            _ => true,
        }));
    }

    #[test]
    fn a_run_fans_its_jobs_out_and_ends_on_its_verdict() {
        let mut card = Card {
            kind: Kind::Run,
            title: "CI".into(),
            phase: Phase::Checking,
            stages: vec![
                stage("Clippy", CheckState::Pass, 250),
                stage("Test (macOS)", CheckState::Pass, 1102),
                stage("Test (Windows)", CheckState::Running, 1265),
            ],
            ..Default::default()
        };
        let scene = pipeline_scene(&card, 600., 1., &hues(), &ink(), &measure);
        assert_eq!(status(&scene), Some(("2 of 3 passed".into(), 565., 19.)));
        assert!(scene.prims.iter().any(|p| matches!(
            p,
            Prim::Dot { at, .. } if *at == (34., 51.)
        )));
        // A longer job ends further along its lane.
        let node_x = |y: f32| {
            scene
                .prims
                .iter()
                .find_map(|p| match p {
                    Prim::Node { at, .. } | Prim::Spinner { at, .. } if at.1 == y => Some(at.0),
                    _ => None,
                })
                .unwrap()
        };
        assert!(node_x(23.) < node_x(51.));
        assert!(node_x(51.) < node_x(79.));
        // Labels end 12 px before their node, inline on the lane.
        assert!(
            texts(&scene).iter().any(|l| l.0 == "Clippy · 4m 10s"
                && l.1 == node_x(23.) - 12.
                && l.3 == Align::Right)
        );

        card.phase = Phase::Failed;
        card.step = "Failed: Test (Windows)".into();
        card.stages[2] = stage("Test (Windows)", CheckState::Fail, 1450);
        let h = hues();
        let scene = pipeline_scene(&card, 600., 1., &h, &ink(), &measure);
        assert_eq!(status(&scene).unwrap().0, "1 failed");
        assert!(matches!(
            end_node(&scene, (558., 51.)),
            Some(Prim::Node { glyph: Some(Glyph::Bang), color, .. }) if color == h.red.mark
        ));
        assert!(!scene.live, "nothing moves once it's final");
        assert_eq!(failure_title(&card), "Test (Windows) failed");
        assert_eq!(failure_detail(&card), "The other jobs passed");
    }

    #[test]
    fn many_jobs_grow_the_band_instead_of_overlapping() {
        let card = Card {
            kind: Kind::Run,
            phase: Phase::Passed,
            stages: (0..6)
                .map(|i| stage(&format!("job {i}"), CheckState::Pass, 60 + i * 30))
                .collect(),
            ..Default::default()
        };
        assert!(graph_height(&card) > 104.);
        let scene = pipeline_scene(&card, 600., 1., &hues(), &ink(), &measure);
        let mut ys: Vec<f32> = scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Node { at, .. } => Some(at.1),
                _ => None,
            })
            .collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        // Six lanes, at least 20 px apart, plus the end node's.
        assert!(ys.len() >= 6, "{ys:?}");
        assert!(ys.windows(2).all(|w| w[1] - w[0] >= 9.9), "{ys:?}");
        assert!(ys.iter().all(|y| *y + 25. <= graph_height(&card)), "{ys:?}");
    }

    #[test]
    fn a_failed_release_leads_with_the_job_and_keeps_the_error_under_it() {
        let card = Card {
            kind: Kind::Release,
            phase: Phase::Failed,
            step: "error[E0433]: failed to resolve: use of undeclared crate `windows`".into(),
            stages: vec![
                stage("macOS app", CheckState::Pass, 300),
                stage("Windows build", CheckState::Fail, 362),
                stage("Windows publish", CheckState::Skipped, 0),
            ],
            version: "0.4.15-nightly.20261025".into(),
            ..Default::default()
        };
        assert_eq!(failure_title(&card), "Windows build failed");
        assert!(failure_detail(&card).starts_with("error[E0433]"));
        assert_eq!(
            card.chip_parts(),
            ("v0.4.15".to_string(), "Windows build failed".to_string())
        );
        let scene = pipeline_scene(&card, 600., 1., &hues(), &ink(), &measure);
        assert_eq!(status(&scene).unwrap().0, "Failed");
        // Nothing was published: the end stays a step not taken.
        assert!(matches!(
            end_node(&scene, (558., 42.)),
            Some(Prim::Ring { dashed: true, .. })
        ));
    }

    #[test]
    fn chips_say_how_far_a_release_or_run_got() {
        let mut release = Card {
            kind: Kind::Release,
            version: "0.4.15-nightly.20261025".into(),
            phase: Phase::Releasing,
            stages: vec![
                stage("macOS app", CheckState::Pass, 552),
                stage("Windows build", CheckState::Running, 300),
                stage("Windows publish", CheckState::Queued, 0),
            ],
            ..Default::default()
        };
        assert_eq!(release.chip(), "v0.4.15 Building 2 of 3");
        release.stages[1].state = CheckState::Pass;
        release.stages[2].state = CheckState::Running;
        assert_eq!(release.chip(), "v0.4.15 Publishing");
        release.phase = Phase::Published;
        assert_eq!(release.chip(), "v0.4.15 Published");
        let mut run = Card {
            kind: Kind::Run,
            title: "CI".into(),
            phase: Phase::Checking,
            stages: vec![
                stage("Clippy", CheckState::Pass, 250),
                stage("Test (macOS)", CheckState::Pass, 1102),
                stage("Test (Windows)", CheckState::Running, 1265),
            ],
            ..Default::default()
        };
        assert_eq!(run.chip(), "CI 2 of 3 passed");
        run.phase = Phase::Passed;
        assert_eq!(run.chip(), "CI Passed");
    }

    #[test]
    fn jobs_keep_how_long_they_took() {
        let jobs = [
            JobJson {
                name: "Clippy".into(),
                status: "completed".into(),
                conclusion: "success".into(),
                started: "2026-10-25T06:00:00Z".into(),
                completed: "2026-10-25T06:04:10Z".into(),
            },
            JobJson {
                name: "Test (Windows)".into(),
                status: "in_progress".into(),
                conclusion: "".into(),
                started: "2026-10-25T06:00:00Z".into(),
                completed: "0001-01-01T00:00:00Z".into(),
            },
        ];
        let stages = job_stages(&jobs);
        assert_eq!(stages[0].ended_ms - stages[0].started_ms, 250_000);
        assert_eq!(stage_label(&stages[0]), "Clippy · 4m 10s");
        assert!(stages[1].started_ms > 0);
        assert_eq!(stages[1].ended_ms, 0, "still running");
    }

    #[test]
    fn a_published_release_links_its_notes_and_a_failed_run_can_rerun() {
        let release = Card {
            kind: Kind::Release,
            version: "0.4.15".into(),
            phase: Phase::Published,
            url: Some("https://github.com/dokyit/Trek/actions/runs/42".into()),
            workflow: true,
            run_id: Some(42),
            ..Default::default()
        };
        assert_eq!(
            notes_url(&release).as_deref(),
            Some("https://github.com/dokyit/Trek/releases/tag/v0.4.15")
        );
        assert_eq!(rerun_id(&release), None, "it didn't fail");
        let mut failed = Card {
            phase: Phase::Failed,
            stages: vec![
                stage("macOS app", CheckState::Pass, 552),
                stage("Windows build", CheckState::Fail, 300),
                stage("Windows publish", CheckState::Skipped, 0),
            ],
            ..release
        };
        assert_eq!(rerun_id(&failed), Some(42));
        rerun_started(&mut failed);
        assert_eq!(failed.phase, Phase::Releasing);
        assert_eq!(
            failed.stages.iter().map(|s| s.state).collect::<Vec<_>>(),
            [CheckState::Pass, CheckState::Queued, CheckState::Queued]
        );
        let local = Card {
            kind: Kind::Release,
            phase: Phase::Failed,
            ..Default::default()
        };
        assert_eq!(
            rerun_id(&local),
            None,
            "a local script has no run to re-run"
        );
    }

    #[test]
    fn a_failed_card_keeps_the_explanation_the_watcher_recorded() {
        let cancelled = Card {
            kind: Kind::Run,
            phase: Phase::Failed,
            step: "Cancelled".into(),
            ..Default::default()
        };
        assert_eq!(failure_title(&cancelled), "Cancelled");
        let release = Card {
            kind: Kind::Release,
            phase: Phase::Failed,
            step: "codesign rejected the archive".into(),
            ..Default::default()
        };
        assert_eq!(failure_title(&release), "codesign rejected the archive");
    }

    #[test]
    fn a_finished_chip_lingers_and_goes_a_live_or_failed_one_stays() {
        use super::{chip_shown, chip_until};
        let at = 1_000_000;
        let live = Card {
            phase: Phase::Checking,
            phase_at_ms: at,
            ..Default::default()
        };
        assert_eq!(chip_until(&live), None);
        assert!(chip_shown(&live, at + 10 * Phase::CHIP_LINGER_MS));
        // A failure stays (until the next message dismisses it); a good finish goes.
        let failed = Card {
            phase: Phase::Failed,
            phase_at_ms: at,
            ..Default::default()
        };
        assert!(chip_shown(&failed, at + 10 * Phase::CHIP_LINGER_MS));
        for phase in [Phase::Merged, Phase::Published, Phase::Passed] {
            let done = Card {
                phase,
                phase_at_ms: at,
                ..Default::default()
            };
            assert!(
                chip_shown(&done, at + 1_000),
                "{phase:?} shows as it settles"
            );
            assert!(
                !chip_shown(&done, at + Phase::CHIP_LINGER_MS),
                "{phase:?} goes after its linger"
            );
        }
    }

    #[test]
    fn lines_split_commands_but_quotes_and_heredocs_do_not() {
        assert_eq!(
            words("git add -A\ngit commit -m \"x\"\ngh pr create --title t"),
            [
                "git add -A",
                "git commit -m \"x\"",
                "gh pr create --title t"
            ]
        );
        // A separator inside quotes is text.
        assert_eq!(
            words("git commit -m \"fix; gh pr merge 5\" && echo 'a | b'"),
            ["git commit -m \"fix; gh pr merge 5\"", "echo 'a | b'"]
        );
        // A backslash joins the lines.
        assert_eq!(
            words("gh pr create \\\n  --title t"),
            ["gh pr create    --title t"]
        );
        // A heredoc's body is not a command; what follows it is.
        assert_eq!(
            words("cat > f <<'EOF'\ngh pr merge 9\nEOF\nscript/release.sh 0.4.14"),
            ["cat > f <<", "script/release.sh 0.4.14"]
        );
        assert_eq!(
            words("cat <<-EOF\n\tgh pr merge 9\n\tEOF\ngh pr merge 1"),
            ["cat <<", "gh pr merge 1"]
        );
        // A here-string and a lone `<<` in quotes open no body.
        assert_eq!(
            words("cat <<< hi\ngh pr merge 2"),
            ["cat <<< hi", "gh pr merge 2"]
        );
        assert_eq!(
            words("git commit -m \"a << b\"\ngh pr merge 3"),
            ["git commit -m \"a << b\"", "gh pr merge 3"]
        );
    }

    fn run(id: u64, status: &str, conclusion: &str, created: &str) -> WorkflowRun {
        WorkflowRun {
            id,
            status: status.into(),
            conclusion: conclusion.into(),
            url: format!("https://github.com/o/r/actions/runs/{id}"),
            created: created.into(),
        }
    }

    fn dispatched() -> Card {
        let mut card = release_card("0.4.14", "stable", "release.yml", "x", Path::new(""));
        card.workflow = true;
        // 2026-10-23T12:00:00Z
        card.created_ms = 1_792_756_800_000;
        card
    }

    #[test]
    fn a_workflow_card_follows_the_run_its_dispatch_made() {
        let card = dispatched();
        // The newest completed run is the previous release's, from before the dispatch.
        let old = run(1, "completed", "success", "2026-10-23T09:00:00Z");
        assert!(pick_run(&card, std::slice::from_ref(&old)).is_none());
        // Once the dispatch's run shows (newest first), that is the one, not the old one.
        let mine = run(2, "in_progress", "", "2026-10-23T12:00:03Z");
        let runs = [mine, old];
        let picked = pick_run(&card, &runs).unwrap();
        assert_eq!(picked.id, 2);
        let jobs = [JobJson {
            name: "build".into(),
            status: "in_progress".into(),
            conclusion: "".into(),
            ..Default::default()
        }];
        let running = release_run_card(&card, picked, &jobs);
        assert_eq!(running.phase, Phase::Releasing);
        assert_eq!(running.run_id, Some(2));
        assert_eq!(
            running.stages[0].label, "build",
            "jobs replace the dispatch stage"
        );
        // Job names no longer defeat the check that this is a workflow card.
        assert!(is_workflow(&running));
        // It sticks to that run when a newer run (someone else's) turns up.
        let other = run(3, "queued", "", "2026-10-23T12:30:00Z");
        let done = run(2, "completed", "success", "2026-10-23T12:00:03Z");
        let runs = [other, done];
        let picked = pick_run(&running, &runs).unwrap();
        assert_eq!(picked.id, 2);
        let published = release_run_card(&running, picked, &[]);
        assert_eq!(published.phase, Phase::Published);
        assert!(published.stages.iter().all(|s| s.state == CheckState::Pass));
        // And a failed run is Failed.
        let failed = run(2, "completed", "failure", "2026-10-23T12:00:03Z");
        assert_eq!(
            release_run_card(&running, &failed, &[]).phase,
            Phase::Failed
        );
    }

    fn view(mergeable: &str, checks: &str) -> PrView {
        serde_json::from_str(&format!(
            r#"{{"number": 9, "state": "OPEN", "mergeable": "{mergeable}", "headRefName": "feat",
                "baseRefName": "main", "statusCheckRollup": {checks}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_pull_request_with_failing_checks_is_not_ready_to_merge() {
        let card = Card {
            kind: Kind::Pr,
            ..Default::default()
        };
        let check = |name: &str, conclusion: &str| {
            format!(r#"{{"name": "{name}", "status": "COMPLETED", "conclusion": "{conclusion}"}}"#)
        };
        let red = format!(
            "[{}, {}, {}]",
            check("build", "SUCCESS"),
            check("test", "FAILURE"),
            check("lint", "SUCCESS")
        );
        let c = pr_from_view(&card, view("MERGEABLE", &red));
        assert_eq!(c.phase, Phase::Open, "{c:?}");
        assert_eq!(c.step, "1 of 3 checks failing: test");
        // All green is ready; still running is still checking; red outranks nothing but a conflict.
        let green = format!(
            "[{}, {}]",
            check("build", "SUCCESS"),
            check("lint", "SKIPPED")
        );
        assert_eq!(
            pr_from_view(&card, view("MERGEABLE", &green)).phase,
            Phase::Ready
        );
        let running = r#"[{"name": "t", "status": "IN_PROGRESS", "conclusion": ""}]"#;
        assert_eq!(
            pr_from_view(&card, view("MERGEABLE", running)).phase,
            Phase::Checking
        );
        assert_eq!(
            pr_from_view(&card, view("CONFLICTING", &red)).phase,
            Phase::Conflicted
        );
    }

    #[test]
    fn a_pull_request_keeps_the_human_parts_of_its_commit_history() {
        let card = Card {
            kind: Kind::Pr,
            ..Default::default()
        };
        let view: PrView = serde_json::from_str(
            r#"{
                "number": 9,
                "state": "OPEN",
                "mergeable": "MERGEABLE",
                "headRefName": "feat/session-refresh",
                "baseRefName": "main",
                "commits": [{
                    "messageHeadline": "Refresh sessions near expiry",
                    "oid": "c70a928812345678",
                    "committedDate": "2026-10-24T12:00:00Z",
                    "authors": [{"login": "tobias", "name": "Tobias"}]
                }],
                "statusCheckRollup": []
            }"#,
        )
        .unwrap();
        let rendered = pr_from_view(&card, view);
        assert_eq!(rendered.commits, Some(1));
        assert_eq!(rendered.history.len(), 1);
        assert_eq!(rendered.history[0].title, "Refresh sessions near expiry");
        assert_eq!(rendered.history[0].oid, "c70a928");
        assert_eq!(rendered.history[0].author, "tobias");
        assert!(rendered.history[0].committed_ms > 0);
    }

    #[test]
    fn a_finished_runs_duration_stops_at_its_end() {
        let card = Card {
            started_ms: now_ms() - 10 * 60_000,
            ended_ms: now_ms() - 8 * 60_000,
            ..Default::default()
        };
        assert_eq!(run_ms(&card), 2 * 60_000);
    }

    #[test]
    fn conflicts_the_trial_could_not_list_are_kept_not_resolved() {
        let mut card = Card {
            phase: Phase::Conflicted,
            resolving: true,
            conflicts: vec!["src/a.rs".into(), "src/b.rs".into()],
            ..Default::default()
        };
        apply_conflicts(&mut card, None);
        assert_eq!(card.conflicts, ["src/a.rs", "src/b.rs"]);
        assert!(card.resolved.is_empty());
        // A real answer moves what cleared to resolved.
        apply_conflicts(&mut card, Some(vec!["src/b.rs".into()]));
        assert_eq!(card.resolved, ["src/a.rs"]);
        assert_eq!(card.conflicts, ["src/b.rs"]);
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", text]);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// The PR's head being rewritten (rebase, amend, force-push) must not blind the trial merge.
    #[test]
    fn the_conflict_list_survives_a_force_pushed_head() {
        let root = std::env::temp_dir().join(format!("trek-pr-card-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (origin, work) = (root.join("origin"), root.join("work"));
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        let base = commit(&origin, "a.rs", "one\n");
        git(&origin, &["checkout", "-q", "-b", "feat"]);
        let first = commit(&origin, "a.rs", "feat\n");
        git(&origin, &["checkout", "-q", "main"]);
        commit(&origin, "a.rs", "main\n");
        git(&origin, &["checkout", "-q", "feat"]);
        git(&origin, &["update-ref", "refs/pull/7/head", &first]);
        git(&work, &["clone", "-q", origin.to_str().unwrap(), "."]);

        let card = Card {
            kind: Kind::Pr,
            number: Some(7),
            base: "main".into(),
            head: "feat".into(),
            ..Default::default()
        };
        assert_eq!(
            conflicting_files(&card, &work),
            Some(vec!["a.rs".to_string()])
        );

        // The PR head is rewritten onto a different parent: no fast-forward of what was fetched.
        git(&origin, &["checkout", "-q", "--detach", &base]);
        let rewritten = commit(&origin, "a.rs", "feat again\n");
        git(&origin, &["update-ref", "refs/pull/7/head", &rewritten]);
        assert_eq!(
            conflicting_files(&card, &work),
            Some(vec!["a.rs".to_string()])
        );

        // No answer is not "no conflicts".
        let lost = Card {
            head: String::new(),
            ..card
        };
        assert_eq!(conflicting_files(&lost, &work), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
