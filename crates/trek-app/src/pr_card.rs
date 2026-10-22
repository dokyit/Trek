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

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::rc::Rc;
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
    /// The command that made it failed.
    Failed,
}

impl Phase {
    /// Nothing more to follow.
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Phase::Merged | Phase::Closed | Phase::Published | Phase::Failed
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
            Phase::Failed => "Failed",
        }
    }

    /// The badge and merge-tip colour.
    fn tint(self, cx: &App) -> Hsla {
        match self {
            Phase::Creating | Phase::Open => cx.theme().muted_foreground,
            Phase::Checking | Phase::Merging | Phase::Releasing => crate::palette::sky(cx),
            Phase::Conflicted => crate::palette::amber(cx),
            Phase::Ready => crate::palette::emerald(cx),
            Phase::Merged | Phase::Published => crate::palette::indigo(cx),
            Phase::Closed | Phase::Failed => crate::palette::red(cx),
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
}

/// One CI check, as `statusCheckRollup` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub label: String,
    pub state: CheckState,
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
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub phase: Phase,
    pub checks: Vec<Check>,
    /// Conflicting paths, when a trial merge could list them (empty: it can't, or there are none).
    pub conflicts: Vec<String>,
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
    /// When it was made.
    pub created_ms: i64,
}

impl Card {
    /// The card's short form for the composer's status strip: "#128 Ready to merge".
    pub fn chip(&self) -> String {
        let what = match self.kind {
            Kind::Pr => self
                .number
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| "PR".to_string()),
            Kind::Release => {
                if self.version.is_empty() {
                    "Release".to_string()
                } else {
                    format!("v{}", self.version)
                }
            }
        };
        let state = if self.resolving {
            "Resolving"
        } else {
            self.phase.label()
        };
        format!("{what} {state}")
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
        }
    }
}

/// The colour a card's chip/badge wears (amber wins while it resolves conflicts).
pub fn chip_tint(card: &Card, cx: &App) -> Hsla {
    if card.resolving {
        crate::palette::amber(cx)
    } else {
        card.phase.tint(cx)
    }
}

/// The card of `Item::Tool` `id`/`detail`, when the row is one and its JSON reads.
pub fn read(id: &str, detail: &str) -> Option<Card> {
    of_row(id)?;
    serde_json::from_str(detail).ok()
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
    /// `gh run watch`, `gh pr checks`: the card's checks are what it's waiting on.
    Checks,
    /// `script/release.sh <version> [--channel …]` — a local release.
    Release { version: String, channel: String },
    /// `gh workflow run release.yml -f version=… -f channel=…`.
    ReleaseWorkflow { version: String, channel: String },
    /// `gh release create <tag>`.
    ReleaseTag { tag: String },
}

/// Command lines are split per `&&`/`;`/`|` segment, the first matching call wins.
pub fn call(title: &str, detail: &str) -> Option<Call> {
    if crate::activity::tool_kind(title) != crate::activity::ToolKind::Command {
        return None;
    }
    for segment in detail.split(['&', ';', '|']) {
        let words = shell_words(segment);
        if let Some(c) = call_in(&words) {
            return Some(c);
        }
    }
    None
}

fn is_gh(words: &[String], sub: &[&str]) -> bool {
    words
        .windows(sub.len() + 1)
        .any(|w| w[0] == "gh" && w[1..] == *sub)
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
    if words.iter().any(|w| w.ends_with("release.sh")) {
        let pos = positional(words);
        return Some(Call::Release {
            version: pos
                .iter()
                .find(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .cloned()
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
        let ix = words
            .windows(3)
            .position(|w| w[0] == "gh" && w[1..] == ["pr", "merge"])
            .map(|i| i + 3)
            .unwrap_or(words.len());
        let pr = words[ix..]
            .iter()
            .find(|w| !w.starts_with('-'))
            .filter(|w| w.contains("/pull/") || w.chars().all(|c| c.is_ascii_digit()))
            .cloned();
        return Some(Call::PrMerge(pr));
    }
    if is_gh(words, &["run", "watch"]) || is_gh(words, &["pr", "checks"]) {
        return Some(Call::Checks);
    }
    if is_gh(words, &["workflow", "run"]) {
        if positional(words).iter().any(|w| w.contains("release")) {
            let mut version = String::new();
            let mut channel = String::new();
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
                    }
                }
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
    None
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

/// What `tool_started`/`tool_finished` ask the workspace to do (their `live` edits are done).
#[derive(Debug, Default)]
pub struct Effects {
    /// Card rows to (re)start `gh` watchers for, by `Item::Tool.id`.
    pub watch: Vec<String>,
    /// Transcript item ids whose card's height may have changed (remeasure).
    pub changed: Vec<String>,
}

/// A command call started: surface or move the card it speaks for. `cwd`: where its `gh` runs.
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
    let dir = cwd.unwrap_or_else(|| Path::new("")).to_path_buf();
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
            push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row_id(&key));
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
                c.phase = Phase::Merging;
                c.step = "Merging the pull request".into();
                rewrite(live, ix, &c, &mut fx);
                live.pr_calls.insert(call_id.to_string(), id.clone());
                fx.watch.push(id);
            }
        }
        Call::Checks => {
            if let Some((ix, id, mut c)) = latest_pr(live) {
                if !c.phase.terminal() {
                    c.phase = Phase::Checking;
                    c.step = "Watching checks".into();
                    rewrite(live, ix, &c, &mut fx);
                    fx.watch.push(id);
                }
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
            let card = release_card(
                &version,
                &channel,
                "release.yml",
                "Release workflow running",
                &dir,
            );
            push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row_id(&key));
            fx.watch.push(row_id(&key));
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
            push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row_id(&key));
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
            push(live, &key, &card, &mut fx);
            live.pr_calls.insert(call_id.to_string(), row_id(&key));
        }
    }
    fx
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
        }],
        created_ms: now_ms(),
        ..Default::default()
    }
}

/// A tracked command call ended: `gh pr create`'s output names the PR it made.
pub fn tool_finished(
    live: &mut crate::workspace::LiveThread,
    call_id: &str,
    ok: bool,
    output: &str,
) -> Effects {
    let mut fx = Effects::default();
    let Some(row) = live.pr_calls.remove(call_id) else {
        return fx;
    };
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
        Kind::Pr if card.phase == Phase::Creating => {
            if ok {
                if let Some((url, number, repo)) = output.lines().find_map(pr_url) {
                    card.url = Some(url);
                    card.number = Some(number);
                    if !repo.is_empty() {
                        card.repo = Some(repo);
                    }
                }
                card.phase = Phase::Open;
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
                card.phase = Phase::Failed;
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
                card.phase = Phase::Merging;
                fx.watch.push(row);
            } else {
                card.step = output
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| format!("Merge failed: {}", l.trim()))
                    .unwrap_or_else(|| "Merge failed".into());
                card.phase = Phase::Open;
            }
        }
        Kind::Release => {
            card.phase = if ok { Phase::Published } else { Phase::Failed };
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

/// Append `card` as a `trek-pr:` row.
fn push(live: &mut crate::workspace::LiveThread, key: &str, card: &Card, fx: &mut Effects) {
    live.streaming = None;
    let ix = live.items.push(Item::Tool {
        id: row_id(key),
        title: match card.kind {
            Kind::Pr => "Pull request".into(),
            Kind::Release => "Release".into(),
        },
        detail: serde_json::to_string(card).unwrap_or_default(),
        output: String::new(),
        status: ToolStatus::Done,
    });
    if let Some(item) = live.items.id_at(ix) {
        fx.changed.push(item.to_string());
    }
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

fn now_ms() -> i64 {
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
    }
}

fn gh(dir: &Path) -> Option<std::process::Command> {
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
    commits: Vec<serde_json::Value>,
    #[serde(rename = "statusCheckRollup")]
    checks: Vec<CheckJson>,
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
    let pr = card
        .url
        .clone()
        .or_else(|| card.number.map(|n| n.to_string()))?;
    let out = gh(dir)?
        .args(["pr", "view", &pr, "--json", PR_FIELDS])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let view: PrView = serde_json::from_slice(&out.stdout).ok()?;
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
    card.added = Some(view.additions as u32);
    card.removed = Some(view.deletions as u32);
    card.checks = view.checks.iter().filter_map(check).collect();
    let pending = card.checks.iter().filter(|c| c.state.pending()).count();
    card.phase = match view.state.as_str() {
        "MERGED" => Phase::Merged,
        "CLOSED" => Phase::Closed,
        _ if card.merging => Phase::Merging,
        _ => {
            card.resolving = card.resolving && view.mergeable == "CONFLICTING";
            match view.mergeable.as_str() {
                "CONFLICTING" => Phase::Conflicted,
                "MERGEABLE" if pending > 0 => Phase::Checking,
                "MERGEABLE" => Phase::Ready,
                _ => Phase::Checking,
            }
        }
    };
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
        Phase::Ready => "Ready to merge".into(),
        Phase::Merging => "Merging the pull request".into(),
        Phase::Merged if card.branch_deleted => {
            format!("Merged into {} · {} was deleted", card.base, card.head)
        }
        Phase::Merged => format!("Merged into {}", card.base),
        Phase::Closed => "Closed".into(),
        _ => card.step.clone(),
    };
    // The conflicting files' names, once GitHub says there are some and a trial merge can tell
    // them apart (best effort: its failure only loses the chips).
    if card.phase == Phase::Conflicted && card.conflicts.is_empty() {
        card.conflicts = conflicting_files(&card, dir);
    }
    Some(card)
}

/// Conflicting paths between the PR's head and its base, from a `merge-tree` trial in `dir`.
fn conflicting_files(card: &Card, dir: &Path) -> Vec<String> {
    let (Some(n), base, head) = (card.number, card.base.clone(), card.head.clone()) else {
        return vec![];
    };
    if base.is_empty() || head.is_empty() {
        return vec![];
    }
    let git = || {
        let mut c = trek_core::procs::command("git");
        c.env("PATH", trek_core::detect::login_path())
            .current_dir(dir);
        c
    };
    // The base, and the PR's head — `pull/N/head` reaches it even from a fork.
    let fetch = git()
        .args([
            "fetch",
            "-q",
            "origin",
            &format!("{base}:refs/trek/pr-base"),
            &format!("pull/{n}/head:refs/trek/pr-head"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        || git()
            .args([
                "fetch",
                "-q",
                "origin",
                &format!("{base}:refs/trek/pr-base"),
                &format!("{head}:refs/trek/pr-head"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if !fetch {
        return vec![];
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
        // Exit 1: conflicts — the tree oid is first, then one path per line.
        Ok(o) if !o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .take(24)
            .collect(),
        _ => vec![],
    }
}

fn fetch_release(card: &Card, dir: &Path) -> Option<Card> {
    // Only workflow releases are followed past the command that started them.
    if card.stages.first().is_none_or(|s| s.label != "release.yml") {
        return None;
    }
    let out = gh(dir)?
        .args([
            "run",
            "list",
            "--workflow",
            "release.yml",
            "--limit",
            "1",
            "--json",
            "databaseId,status,conclusion,url",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    #[derive(Deserialize)]
    struct Run {
        #[serde(rename = "databaseId")]
        id: u64,
        status: String,
        conclusion: String,
        url: String,
    }
    let run = serde_json::from_slice::<Vec<Run>>(&out.stdout)
        .ok()?
        .into_iter()
        .next()?;
    let mut card = card.clone();
    card.url = Some(run.url);
    if run.status != "completed" {
        if let Some(jobs) = gh(dir)
            .and_then(|mut c| {
                c.args(["run", "view", &run.id.to_string(), "--json", "jobs"])
                    .output()
                    .ok()
            })
            .and_then(|o| {
                #[derive(Deserialize)]
                struct Jobs {
                    jobs: Vec<JobJ>,
                }
                #[derive(Deserialize)]
                struct JobJ {
                    name: String,
                    status: String,
                    conclusion: String,
                }
                let j: Jobs = serde_json::from_slice(&o.stdout).ok()?;
                Some(
                    j.jobs
                        .into_iter()
                        .map(|j| CheckJson {
                            name: j.name,
                            status: j.status,
                            conclusion: j.conclusion,
                            ..Default::default()
                        })
                        .collect::<Vec<_>>(),
                )
            })
        {
            card.stages = jobs
                .iter()
                .filter_map(|j| {
                    check(j).map(|c| Stage {
                        label: c.name,
                        state: c.state,
                    })
                })
                .collect();
        }
        card.step = "Release workflow running".into();
        return Some(card);
    }
    card.phase = match run.conclusion.as_str() {
        "success" => Phase::Published,
        _ => Phase::Failed,
    };
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
    Some(card)
}

// ---------- drawing ----------

/// What the card's controls do (all optional: no Resolve while resolving, no Merge until Ready).
#[derive(Default)]
pub struct Actions {
    pub merge: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    pub resolve: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    pub open: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

/// The card for transcript item `ix` showing `card`; `clock` animates its in-progress marks
/// (seconds; the working bar's pulse drives it).
pub fn card(ix: usize, card: &Card, actions: Actions, clock: f32, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let tint = card.phase.tint(cx);
    let merged = matches!(card.phase, Phase::Merged | Phase::Published);

    let badge = h_flex()
        .flex_none()
        .gap(px(5.))
        .items_center()
        .rounded_full()
        .px(px(8.))
        .py(px(2.))
        .text_xs()
        .text_color(tint)
        .bg(tint.opacity(0.12))
        .when(
            matches!(
                card.phase,
                Phase::Checking | Phase::Merging | Phase::Creating | Phase::Releasing
            ) || card.resolving,
            |el| el.child(Spinner::new().xsmall().color(tint)),
        )
        .child(if card.resolving {
            "Resolving".to_string()
        } else {
            card.phase.label().to_string()
        });

    let title = match card.kind {
        Kind::Pr => match card.number {
            Some(n) => format!("{} #{n}", card.title),
            None => card.title.clone(),
        },
        Kind::Release => card.title.clone(),
    };
    let head = h_flex()
        .id(("pr-head", ix))
        .test_support()
        .gap(px(8.))
        .pl(px(14.))
        .pr(px(8.))
        .pt(px(8.))
        .text_size(px(12.5))
        .child(
            Icon::new(match card.kind {
                Kind::Pr => crate::assets::Lucide::GitPullRequest,
                Kind::Release => crate::assets::Lucide::Rocket,
            })
            .size(px(15.))
            .text_color(tint),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_semibold()
                .text_color(theme.foreground.opacity(0.92))
                .child(title),
        )
        .child(div().flex_1())
        .when_some(actions.open.clone(), |el, open| {
            el.child(
                Button::new(("pr-open", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::ExternalLink).text_color(muted))
                    .tooltip("Open in the browser")
                    .on_click(move |_, window, cx| open(window, cx)),
            )
        })
        .child(badge);

    // `head → base · N commits · +a −b` — each only when known.
    let mut sub: Vec<String> = vec![];
    if card.phase == Phase::Merged {
        let mut tail = format!("Merged into {}", card.base);
        if card.branch_deleted && !card.head.is_empty() {
            tail.push_str(&format!(" · {} was deleted", card.head));
        }
        sub.push(tail);
    } else {
        if !card.head.is_empty() && !card.base.is_empty() {
            sub.push(format!("{} → {}", card.head, card.base));
        } else if !card.base.is_empty() {
            sub.push(format!("→ {}", card.base));
        }
        if let Some(n) = card.commits {
            sub.push(format!("{n} {}", if n == 1 { "commit" } else { "commits" }));
        }
        let mut diff = String::new();
        if let Some(a) = card.added {
            diff = format!("+{a}");
        }
        if let Some(r) = card.removed {
            if !diff.is_empty() {
                diff.push(' ');
            }
            diff.push_str(&format!("−{r}"));
        }
        if !diff.is_empty() {
            sub.push(diff);
        }
    }
    let subtitle = (!sub.is_empty()).then(|| {
        div()
            .pl(px(14.))
            .pr(px(10.))
            .pb(px(6.))
            .text_xs()
            .text_color(muted)
            .child(sub.join(" · "))
    });

    let graph = (card.kind == Kind::Pr).then(|| branch_graph(ix, card, clock, cx));

    let checks = (!card.checks.is_empty() && !merged).then(|| {
        h_flex()
            .px(px(12.))
            .pb(px(8.))
            .gap(px(6.))
            .flex_wrap()
            .children(card.checks.iter().take(8).map(|c| check_chip(c, cx)))
    });

    let stages = (!card.stages.is_empty()).then(|| {
        h_flex()
            .px(px(12.))
            .pb(px(8.))
            .gap(px(6.))
            .flex_wrap()
            .children(card.stages.iter().take(5).map(|s| {
                check_chip(
                    &Check {
                        name: s.label.clone(),
                        state: s.state,
                    },
                    cx,
                )
            }))
    });

    // The conflicts callout: warning row, a chip per file, Resolve on the right.
    let conflicts = (card.phase == Phase::Conflicted).then(|| {
        let amber = crate::palette::amber(cx);
        h_flex()
            .mx(px(10.))
            .mb(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(amber.opacity(0.35))
            .bg(amber.opacity(0.07))
            .px(px(10.))
            .py(px(7.))
            .gap(px(8.))
            .items_center()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size(px(13.))
                    .text_color(amber),
            )
            .child(v_flex().flex_1().min_w_0().gap(px(4.)).child(
                div().text_xs().text_color(amber).child(if card.resolving {
                    "Resolving the conflicts…".to_string()
                } else {
                    format!(
                        "{} with {}",
                        match card.conflicts.len() {
                            0 => "Conflicts".to_string(),
                            1 => "1 conflict".to_string(),
                            n => format!("{n} conflicts"),
                        },
                        card.base
                    )
                }),
            ))
            .children((!card.conflicts.is_empty()).then(|| {
                h_flex()
                    .pb(px(2.))
                    .pl(px(21.))
                    .gap(px(4.))
                    .flex_wrap()
                    .children(card.conflicts.iter().take(6).map(|f| {
                        div()
                            .rounded(px(4.))
                            .px(px(5.))
                            .py(px(1.))
                            .bg(theme.foreground.opacity(0.06))
                            .text_size(px(10.))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.foreground.opacity(0.8))
                            .child(f.clone())
                    }))
            }))
            .when_some(actions.resolve.clone(), |el, resolve| {
                el.child(
                    Button::new(("pr-resolve", ix))
                        .small()
                        .outline()
                        .label("Resolve conflicts")
                        .disabled(card.resolving)
                        .on_click(move |_, window, cx| resolve(window, cx)),
                )
            })
    });

    let merge = (card.phase == Phase::Ready).then(|| {
        h_flex().px(px(12.)).pb(px(8.)).child(
            Button::new(("pr-merge", ix))
                .small()
                .primary()
                .icon(Icon::new(crate::assets::Lucide::GitMerge).size(px(13.)))
                .label("Merge")
                .when_some(actions.merge.clone(), |b, merge| {
                    b.on_click(move |_, window, cx| merge(window, cx))
                }),
        )
    });

    let step = (!card.step.is_empty() && !merged).then(|| {
        h_flex()
            .pl(px(2.))
            .pb(px(4.))
            .gap(px(6.))
            .items_center()
            .text_xs()
            .text_color(muted)
            .when(
                matches!(
                    card.phase,
                    Phase::Creating | Phase::Checking | Phase::Merging | Phase::Releasing
                ) || card.resolving,
                |el| el.child(Spinner::new().xsmall().color(muted)),
            )
            .child(card.step.clone())
    });

    let padded = card.checks.is_empty()
        && card.stages.is_empty()
        && card.phase != Phase::Ready
        && card.phase != Phase::Conflicted;
    v_flex()
        .id(("pr-card", ix))
        .test_support()
        .mt(px(4.))
        .mb(px(8.))
        .children(step)
        .child(
            v_flex()
                .rounded(px(10.))
                .border_1()
                .border_color(if merged {
                    tint.opacity(0.35)
                } else {
                    theme.foreground.opacity(0.08)
                })
                .bg(if merged {
                    tint.opacity(0.08)
                } else {
                    theme.secondary
                })
                .overflow_hidden()
                .child(head)
                .children(subtitle)
                .children(graph)
                .children(conflicts)
                .children(checks)
                .children(stages)
                .children(merge)
                .child(div().h(px(if padded { 4. } else { 0. }))),
        )
        .into_any_element()
}

/// One check or stage chip: a state mark, then its name.
fn check_chip(c: &Check, cx: &App) -> AnyElement {
    let (mark, tint) = match c.state {
        CheckState::Pass => (
            Icon::new(IconName::CircleCheck)
                .size(px(11.))
                .into_any_element(),
            crate::palette::emerald(cx),
        ),
        CheckState::Fail => (
            Icon::new(IconName::CircleX)
                .size(px(11.))
                .into_any_element(),
            crate::palette::red(cx),
        ),
        CheckState::Skipped => (
            Icon::new(IconName::Minus).size(px(11.)).into_any_element(),
            cx.theme().muted_foreground,
        ),
        CheckState::Queued => (
            div()
                .size(px(7.))
                .m(px(2.))
                .rounded_full()
                .bg(cx.theme().muted_foreground.opacity(0.5))
                .into_any_element(),
            cx.theme().muted_foreground,
        ),
        CheckState::Running => (
            Spinner::new()
                .xsmall()
                .color(crate::palette::sky(cx))
                .into_any_element(),
            crate::palette::sky(cx),
        ),
    };
    h_flex()
        .gap(px(4.))
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(tint.opacity(0.3))
        .px(px(7.))
        .py(px(2.))
        .text_size(px(10.5))
        .text_color(tint)
        .child(mark)
        .child(c.name.clone())
        .into_any_element()
}

/// The branch graph: `main` along the bottom with its commits, the head branch curving off it
/// and a dashed merge curve coming back — the tip wearing the phase's mark.
fn branch_graph(ix: usize, card: &Card, clock: f32, cx: &App) -> AnyElement {
    let card = card.clone();
    let _ = ix;
    let theme = cx.theme();
    let muted = theme.muted_foreground.opacity(0.55);
    let branch = crate::palette::sky(cx);
    let tip = card.phase.tint(cx);
    let (main_c, branch_c) = if card.phase == Phase::Merged {
        (tip, tip)
    } else {
        (muted, branch)
    };
    let commits = card.commits.unwrap_or(2).clamp(1, 4) as usize;
    let merged = card.phase == Phase::Merged;
    let spin = matches!(
        card.phase,
        Phase::Creating | Phase::Checking | Phase::Merging
    ) || card.resolving;
    // Creating draws the branch still reaching toward main; it settles as the PR opens.
    let grow = match card.phase {
        Phase::Creating => 0.55 + 0.2 * (0.5 + 0.5 * (clock * 2.2).sin()),
        Phase::Open | Phase::Checking => 0.85,
        _ => 1.0,
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let w = bounds.size.width;
            let h = bounds.size.height;
            let (my, by) = (o.y + h * 0.70, o.y + h * 0.26);
            let x0 = o.x + w * 0.10;
            let x1 = o.x + w * 0.94;
            let fork = o.x + w * 0.26;
            let join = o.x + w * (0.26 + 0.68 * grow.clamp(0., 1.));
            let r = px(3.2);

            let mut main_line = PathBuilder::stroke(px(1.6));
            main_line.move_to(point(x0, my));
            main_line.line_to(point(x1, my));
            if let Ok(p) = main_line.build() {
                window.paint_path(p, main_c);
            }
            for i in 0..3 {
                let x = x0 + (fork - x0) * (i as f32 + 0.4) / 3.4;
                dot(window, point(x, my), r, main_c);
            }

            // The branch: leaves main at `fork`, levels at `by`, runs to `join`.
            let lift = px(22.);
            let mut br = PathBuilder::stroke(px(1.6));
            br.move_to(point(fork, my));
            br.cubic_bezier_to(
                point(fork + lift * 1.4, by),
                point(fork + lift * 0.4, my),
                point(fork + lift * 0.9, by),
            );
            br.line_to(point(join - lift, by));
            if merged || grow >= 1. {
                br.cubic_bezier_to(
                    point(join, my),
                    point(join - lift * 0.15, by),
                    point(join - lift * 0.55, my),
                );
            }
            if let Ok(p) = br.build() {
                window.paint_path(p, branch_c);
            }
            // Branch commits along the flat part.
            let flat0 = fork + lift * 1.55;
            let flat1 = join - lift * 1.15;
            if flat1 > flat0 {
                for i in 0..commits {
                    let x = flat0 + (flat1 - flat0) * (i as f32 + 0.5) / commits as f32;
                    dot(window, point(x, by), r, branch_c);
                }
            }
            // The merge curve, dashed until it's done.
            if !merged && grow >= 1. {
                let mut m = PathBuilder::stroke(px(1.4)).dash_array(&[px(3.5), px(3.)]);
                m.move_to(point(join - lift, by));
                m.cubic_bezier_to(
                    point(join, my),
                    point(join - lift * 0.15, by),
                    point(join - lift * 0.55, my),
                );
                if let Ok(p) = m.build() {
                    window.paint_path(p, tip.opacity(0.8));
                }
            }
            // The merge tip: a ringed dot; in progress, a turning arc.
            let tip_p = point(join, my);
            if card.phase != Phase::Creating {
                if spin {
                    let mut halo = PathBuilder::stroke(px(1.4));
                    let steps = 24;
                    let start = clock * std::f32::consts::TAU;
                    for i in 0..=steps * 3 / 4 {
                        let a = start + (i as f32 / steps as f32) * std::f32::consts::TAU;
                        let p = point(tip_p.x + px(6.5) * a.cos(), tip_p.y + px(6.5) * a.sin());
                        if i == 0 {
                            halo.move_to(p)
                        } else {
                            halo.line_to(p)
                        }
                    }
                    if let Ok(p) = halo.build() {
                        window.paint_path(p, tip);
                    }
                }
                dot(window, tip_p, r + px(1.2), tip);
            }
        },
    )
    .w_full()
    .h(px(52.))
    .into_any_element()
}

/// A filled circle at `c`, as a polygon (at these sizes it reads as a dot).
fn dot(window: &mut Window, c: Point<Pixels>, r: Pixels, color: Hsla) {
    let mut b = PathBuilder::fill();
    const N: usize = 10;
    for i in 0..N {
        let a = i as f32 / N as f32 * std::f32::consts::TAU;
        let p = point(c.x + r * a.cos(), c.y + r * a.sin());
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close();
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// A card in `state` with made-up contents, for design review (`shots`' `pr-demo`).
/// Nothing here is fetched: it's drawn as given.
#[cfg(feature = "shots")]
pub fn demo(state: &str) -> Card {
    let base = Card {
        kind: Kind::Pr,
        title: "Add the context-ring popover".into(),
        number: Some(128),
        url: Some("https://github.com/dokyit/Trek/pull/128".into()),
        head: "feat/context-ring".into(),
        base: "main".into(),
        repo_dir: ".".into(),
        repo: Some("dokyit/Trek".into()),
        commits: Some(3),
        added: Some(128),
        removed: Some(36),
        created_ms: now_ms(),
        ..Default::default()
    };
    match state {
        "creating" => Card {
            phase: Phase::Creating,
            step: "Opening the pull request".into(),
            number: None,
            commits: None,
            added: None,
            removed: None,
            ..base
        },
        "checking" => Card {
            phase: Phase::Checking,
            step: "Checking it merges cleanly".into(),
            checks: vec![
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
            ],
            ..base
        },
        "conflicted" => Card {
            phase: Phase::Conflicted,
            step: "3 conflicts with main".into(),
            conflicts: vec![
                "src/composer.rs".into(),
                "src/workspace.rs".into(),
                "Cargo.lock".into(),
            ],
            ..base
        },
        "resolving" => Card {
            phase: Phase::Conflicted,
            resolving: true,
            step: "Resolving 3 conflicts".into(),
            conflicts: vec![
                "src/composer.rs".into(),
                "src/workspace.rs".into(),
                "Cargo.lock".into(),
            ],
            ..base
        },
        "ready" => Card {
            phase: Phase::Ready,
            step: "Ready to merge".into(),
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
            ..base
        },
        "merged" => Card {
            phase: Phase::Merged,
            branch_deleted: true,
            step: String::new(),
            ..base
        },
        "release" => Card {
            kind: Kind::Release,
            title: "Release v0.4.1".into(),
            version: "0.4.1".into(),
            channel: "beta".into(),
            number: None,
            url: None,
            head: String::new(),
            phase: Phase::Releasing,
            step: "Release workflow running".into(),
            stages: vec![
                Stage {
                    label: "macOS".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Windows".into(),
                    state: CheckState::Running,
                },
                Stage {
                    label: "publish".into(),
                    state: CheckState::Queued,
                },
            ],
            ..base
        },
        // "open"
        _ => Card {
            phase: Phase::Open,
            step: String::new(),
            ..base
        },
    }
}
