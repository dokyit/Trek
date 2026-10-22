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

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
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
    /// A workflow run (`gh run watch`) finished green.
    Passed,
    /// The command that made it failed (a run: it finished red or was cancelled).
    Failed,
}

impl Phase {
    /// Nothing more to follow.
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

    /// The badge and merge-tip colour.
    fn tint(self, cx: &App) -> Hsla {
        match self {
            Phase::Creating | Phase::Open => cx.theme().muted_foreground,
            Phase::Checking | Phase::Merging | Phase::Releasing => crate::palette::sky(cx),
            Phase::Conflicted => crate::palette::amber(cx),
            Phase::Ready | Phase::Passed => crate::palette::emerald(cx),
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
    /// When it was made.
    pub created_ms: i64,
    /// When `phase` last changed: draws the phase's graph in (0 until tracked).
    pub phase_at_ms: i64,
    /// Run cards: when the run started and (once completed) ended, for the elapsed label.
    pub started_ms: i64,
    pub ended_ms: i64,
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
            Kind::Release => {
                if self.version.is_empty() {
                    "Release".to_string()
                } else {
                    format!("v{}", self.version)
                }
            }
            Kind::Run => run_name(self).to_string(),
        };
        // As the reference words it: how many conflicts, not just that there are some.
        let state = match (self.resolving, self.phase, self.conflicts.len()) {
            (true, ..) => "Resolving".to_string(),
            (false, Phase::Conflicted, 1) => "1 conflict".to_string(),
            (false, Phase::Conflicted, n) if n > 1 => format!("{n} conflicts"),
            (false, phase, _) => phase.label().to_string(),
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

/// Command lines are split per `&&`/`;`/`|` segment: a `gh` call wins over a `git` one
/// (`git commit && gh pr create` opens the card, it doesn't just re-word its step).
pub fn call(title: &str, detail: &str) -> Option<Call> {
    if crate::activity::tool_kind(title) != crate::activity::ToolKind::Command {
        return None;
    }
    let mut git = None;
    for segment in detail.split(['&', ';', '|']) {
        let words = shell_words(segment);
        match call_in(&words) {
            Some(Call::Git(_)) => git = git.or_else(|| call_in(&words)),
            Some(c) => return Some(c),
            None => {}
        }
    }
    git
}

fn is_gh(words: &[String], sub: &[&str]) -> bool {
    words
        .windows(sub.len() + 1)
        .any(|w| w[0] == "gh" && w[1..] == *sub)
}

/// The words after `gh <sub…>` (none when it isn't there).
fn after<'a>(words: &'a [String], sub: &[&str]) -> &'a [String] {
    let ix = words
        .windows(sub.len() + 1)
        .position(|w| w[0] == "gh" && w[1..] == *sub)
        .map(|i| i + sub.len() + 1)
        .unwrap_or(words.len());
    &words[ix..]
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
        return Some(Call::PrMerge(pr_arg(words, &["pr", "merge"])));
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
    // Plain `git` work the card narrates while it's open (a `gh` call elsewhere in the line wins).
    let verb = match words
        .iter()
        .position(|w| w == "git")
        .map(|i| words.get(i + 1))
    {
        Some(Some(w)) if w == "commit" => Some(GitVerb::Commit),
        Some(Some(w)) if w == "push" => Some(GitVerb::Push),
        Some(Some(w)) if w == "fetch" => Some(GitVerb::Fetch),
        Some(Some(w)) if w == "merge" => Some(GitVerb::Merge),
        Some(Some(w)) if w == "pull" => Some(GitVerb::Pull),
        Some(Some(w)) if w == "rebase" => Some(GitVerb::Rebase),
        _ => None,
    };
    verb.map(Call::Git)
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
                push(live, &key, &card, &mut fx);
                fx.watch.push(row_id(&key));
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

/// Append `card` as a `trek-pr:` row.
fn push(live: &mut crate::workspace::LiveThread, key: &str, card: &Card, fx: &mut Effects) {
    live.streaming = None;
    let ix = live.items.push(Item::Tool {
        id: row_id(key),
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
        Kind::Run => fetch_run(card, &dir),
    }
}

/// A job of `gh run view --json jobs` (its states lowercase: `completed`, `success`).
#[derive(Deserialize)]
struct JobJson {
    name: String,
    status: String,
    conclusion: String,
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
            "workflowName,displayTitle,headBranch,status,conclusion,url,jobs,startedAt,updatedAt",
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
    let phase = match view.state.as_str() {
        "MERGED" => Phase::Merged,
        "CLOSED" => Phase::Closed,
        _ if card.merging => Phase::Merging,
        _ => match view.mergeable.as_str() {
            "CONFLICTING" => Phase::Conflicted,
            "MERGEABLE" if pending > 0 => Phase::Checking,
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
    // them apart (best effort: its failure only loses the chips). While it resolves, the list is
    // asked again each pass so the card counts down, remembering what already cleared.
    if card.phase == Phase::Conflicted && (card.conflicts.is_empty() || card.resolving) {
        let next = conflicting_files(&card, dir);
        if card.resolving {
            for f in card.conflicts.drain(..) {
                if !next.contains(&f) && !card.resolved.contains(&f) && card.resolved.len() < 12 {
                    card.resolved.push(f);
                }
            }
        }
        card.conflicts = next;
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
                    jobs: Vec<JobJson>,
                }
                serde_json::from_slice::<Jobs>(&o.stdout).ok()
            })
        {
            card.stages = job_stages(&jobs.jobs);
        }
        card.step = "Release workflow running".into();
        return Some(card);
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
    let tint = state_tint(card, cx);
    let motion = !cx.reduce_motion();
    // How long the phase has stood: its graph draws in, its end mark pops once.
    let age = (now_ms() - card.phase_at_ms.max(card.created_ms)) as f32 / 1000.;
    let draw = if motion {
        (age / 0.45).clamp(0., 1.)
    } else {
        1.
    };
    let pop = if motion && age < 0.4 {
        (age / 0.4 * std::f32::consts::PI).sin()
    } else {
        0.
    };
    let busy = matches!(
        card.phase,
        Phase::Creating | Phase::Checking | Phase::Merging | Phase::Releasing
    ) || card.resolving;

    // --- header: tinted icon square, the title and its quiet number, a soft badge on the right.
    let icon = match card.kind {
        Kind::Pr => crate::assets::Lucide::GitPullRequest,
        Kind::Release => crate::assets::Lucide::Rocket,
        Kind::Run => crate::assets::Lucide::Workflow,
    };
    let (title, tag) = match card.kind {
        Kind::Pr => (card.title.clone(), card.number.map(|n| format!("#{n}"))),
        Kind::Release => (card.title.clone(), None),
        Kind::Run => (
            if card.subject.is_empty() {
                format!("{} run", run_name(card))
            } else {
                card.subject.clone()
            },
            card.number.map(|n| format!("#{n}")),
        ),
    };
    let badge = state_label(card).map(|label| {
        h_flex()
            .flex_none()
            .gap(px(4.))
            .items_center()
            .rounded_full()
            .px(px(8.))
            .py(px(2.5))
            .text_xs()
            .text_color(tint)
            .bg(tint.opacity(0.12))
            .child(match (card.kind, card.phase) {
                (Kind::Pr, Phase::Merged) => Icon::new(crate::assets::Lucide::GitMerge)
                    .size(px(10.))
                    .text_color(tint)
                    .into_any_element(),
                (Kind::Pr, Phase::Closed | Phase::Failed)
                | (_, Phase::Failed)
                | (Kind::Run, Phase::Closed) => Icon::new(IconName::CircleX)
                    .size(px(10.))
                    .text_color(tint)
                    .into_any_element(),
                (_, Phase::Passed) => Icon::new(IconName::CircleCheck)
                    .size(px(10.))
                    .text_color(tint)
                    .into_any_element(),
                (Kind::Pr, _) => Icon::new(crate::assets::Lucide::GitPullRequest)
                    .size(px(10.))
                    .text_color(tint)
                    .into_any_element(),
                (Kind::Release, Phase::Published) => Icon::new(crate::assets::Lucide::Rocket)
                    .size(px(10.))
                    .text_color(tint)
                    .into_any_element(),
                _ => Spinner::new().xsmall().color(tint).into_any_element(),
            })
            .child(label)
    });
    let badge = match (badge, actions.open.clone()) {
        (Some(b), Some(open)) => b
            .id(("pr-open", ix))
            .test_support()
            .cursor_pointer()
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Open in the browser").build(window, cx)
            })
            .on_click(move |_, window, cx| open(window, cx))
            .into_any_element(),
        (Some(b), None) => b.into_any_element(),
        (None, _) => div().into_any_element(),
    };
    let head = h_flex()
        .id(("pr-head", ix))
        .test_support()
        .gap(px(10.))
        .px(px(14.))
        .pt(px(11.))
        .pb(px(9.))
        .child(
            div()
                .size(px(26.))
                .flex_none()
                .rounded(px(7.))
                .bg(tint.opacity(0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size(px(14.)).text_color(tint)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .child(
                    h_flex()
                        .gap(px(5.))
                        .items_baseline()
                        .text_size(px(12.5))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_semibold()
                                .text_color(theme.foreground.opacity(0.92))
                                .child(title),
                        )
                        .when_some(tag, |el, t| {
                            el.child(div().flex_none().text_xs().text_color(muted).child(t))
                        }),
                )
                .children(sub_row(card, cx)),
        )
        .child(badge);

    let step = (!card.step.is_empty() && !card.phase.terminal()).then(|| {
        h_flex()
            .pl(px(2.))
            .pb(px(4.))
            .gap(px(5.))
            .items_center()
            .text_xs()
            .text_color(muted)
            .child(Icon::new(IconName::ChevronRight).xsmall())
            .child(card.step.clone())
            .when(busy && motion, |el| {
                el.child(Spinner::new().xsmall().color(muted))
            })
    });

    let band = band(card, clock, draw, pop, motion, cx);
    let footer = footer(ix, card, &actions, cx);
    let tail = footer.is_none();

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
                .border_color(theme.foreground.opacity(0.08))
                .bg(theme.secondary)
                .overflow_hidden()
                .child(head)
                .child(band)
                .children(footer)
                .child(div().h(px(if tail { 4. } else { 0. }))),
        )
        .into_any_element()
}

/// The header icon's and badge's colour: the card's state (open, merged, failed), not the
/// moment's phase tint, which the graph's tip wears.
fn state_tint(card: &Card, cx: &App) -> Hsla {
    match card.kind {
        Kind::Pr => match card.phase {
            Phase::Merged => crate::palette::indigo(cx),
            Phase::Closed | Phase::Failed => crate::palette::red(cx),
            Phase::Creating => cx.theme().muted_foreground,
            _ => crate::palette::emerald(cx),
        },
        Kind::Run => match card.phase {
            Phase::Passed => crate::palette::emerald(cx),
            Phase::Failed | Phase::Closed => crate::palette::red(cx),
            _ => crate::palette::sky(cx),
        },
        Kind::Release => match card.phase {
            Phase::Published => crate::palette::indigo(cx),
            Phase::Failed | Phase::Closed => crate::palette::red(cx),
            _ => crate::palette::sky(cx),
        },
    }
}

/// The badge's words for its kind; `None` for a PR that isn't on GitHub yet.
fn state_label(card: &Card) -> Option<&'static str> {
    match card.kind {
        Kind::Pr => match card.phase {
            Phase::Merged => Some("Merged"),
            Phase::Closed => Some("Closed"),
            Phase::Failed => Some("Failed"),
            _ if card.number.is_some() => Some("Open"),
            _ => None,
        },
        Kind::Run => Some(match card.phase {
            Phase::Passed => "Passed",
            Phase::Failed | Phase::Closed => "Failed",
            _ => "Running",
        }),
        Kind::Release => Some(match card.phase {
            Phase::Published => "Published",
            Phase::Failed | Phase::Closed => "Failed",
            _ => "Releasing",
        }),
    }
}

/// A branch name (or channel): mono, on the soft square the names in the graph wear.
fn mono(text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .rounded(px(4.))
        .px(px(4.))
        .py(px(1.))
        .bg(theme.foreground.opacity(0.06))
        .text_size(px(10.))
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.foreground.opacity(0.7))
        .child(text.to_string())
        .into_any_element()
}

/// The detail line under the title: branches and counts for a PR, the run's or release's facts.
fn sub_row(card: &Card, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mut row = h_flex()
        .gap(px(6.))
        .items_center()
        .min_w_0()
        .text_xs()
        .text_color(muted);
    match card.kind {
        Kind::Pr => {
            let mut any = false;
            if !card.head.is_empty() {
                row = row.child(mono(&card.head, cx)).child("→");
                any = true;
            }
            if !card.base.is_empty() {
                row = row.child(mono(&card.base, cx));
                any = true;
            }
            if let Some(n) = card.commits {
                row = row.child(format!("{n} {}", if n == 1 { "commit" } else { "commits" }));
                any = true;
            }
            let (a, r) = (card.added.unwrap_or(0), card.removed.unwrap_or(0));
            if a > 0 || r > 0 {
                row = row
                    .child(
                        div()
                            .text_color(crate::palette::emerald(cx))
                            .child(format!("+{a}")),
                    )
                    .child(
                        div()
                            .text_color(crate::palette::red(cx))
                            .child(format!("−{r}")),
                    );
                any = true;
            }
            any.then(|| row.into_any_element())
        }
        Kind::Run => {
            if !card.head.is_empty() {
                row = row.child(mono(&card.head, cx));
            }
            row = row.child(div().min_w_0().truncate().child(run_name(card).to_string()));
            if card.started_ms > 0 {
                let end = if card.ended_ms > 0 {
                    card.ended_ms
                } else {
                    now_ms()
                };
                row = row.child(dur_label(end - card.started_ms));
            }
            Some(row.into_any_element())
        }
        Kind::Release => {
            if !card.channel.is_empty() {
                row = row.child(mono(&card.channel, cx));
            }
            if !card.stages.is_empty() {
                row = row.child(format!(
                    "{} {}",
                    card.stages.len(),
                    if card.stages.len() == 1 {
                        "job"
                    } else {
                        "jobs"
                    }
                ));
            }
            Some(row.into_any_element())
        }
    }
}

/// "45s", "3m 12s", "1h 4m" — coarse, like the working bar's.
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

/// A file (or failed job) in the footer: hairline chip, tag, name, a trailing mark.
fn file_chip(tag: &str, name: &str, mark: Option<(&'static str, Hsla)>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    h_flex()
        .flex_none()
        .gap(px(5.))
        .items_center()
        .rounded(px(5.))
        .border_1()
        .border_color(theme.foreground.opacity(0.10))
        .px(px(6.))
        .py(px(2.))
        .text_size(px(10.))
        .when(!tag.is_empty(), |el| {
            el.child(
                div()
                    .font_family(mono.clone())
                    .text_color(theme.muted_foreground)
                    .child(tag.to_string()),
            )
        })
        .child(
            div()
                .font_family(mono)
                .text_color(theme.foreground.opacity(0.8))
                .child(name.to_string()),
        )
        .when_some(mark, |el, (m, c)| {
            el.child(div().text_color(c).child(m.to_string()))
        })
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

/// The footer under the graph when there's something to say or do: conflicts, ready, merged,
/// failed; a run's or release's result. `None` while the card is still just working.
fn footer(ix: usize, card: &Card, actions: &Actions, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let (green, red, amber, indigo) = (
        crate::palette::emerald(cx),
        crate::palette::red(cx),
        crate::palette::amber(cx),
        crate::palette::indigo(cx),
    );
    let when = card.phase_at_ms.max(card.created_ms);
    // (icon, tint, title, subtitle)
    let (icon, tint, title, sub): (AnyElement, Hsla, String, String) = match card.kind {
        Kind::Pr => match card.phase {
            Phase::Conflicted => {
                let n = card.conflicts.len();
                (
                    Icon::new(IconName::TriangleAlert)
                        .size(px(12.))
                        .text_color(amber)
                        .into_any_element(),
                    amber,
                    if card.resolving {
                        "Resolving conflicts".to_string()
                    } else {
                        format!(
                            "{} with {}",
                            if n == 0 {
                                format!("{} conflicts", card.resolved.len().max(1))
                            } else if n == 1 {
                                "1 conflict".to_string()
                            } else {
                                format!("{n} conflicts")
                            },
                            card.base
                        )
                    },
                    if card.resolving && n == 0 {
                        "Pushing the merge".to_string()
                    } else if card.resolving {
                        format!("{n} left to clear")
                    } else {
                        "These files changed on both branches".to_string()
                    },
                )
            }
            Phase::Ready | Phase::Merging => {
                let n = card.checks.len();
                (
                    Icon::new(IconName::CircleCheck)
                        .size(px(12.))
                        .text_color(green)
                        .into_any_element(),
                    green,
                    "Ready to merge".to_string(),
                    if n == 0 {
                        "No conflicts".to_string()
                    } else {
                        format!("No conflicts · all {n} checks passed")
                    },
                )
            }
            Phase::Merged => (
                Icon::new(crate::assets::Lucide::GitMerge)
                    .size(px(12.))
                    .text_color(indigo)
                    .into_any_element(),
                indigo,
                format!("Merged into {}", card.base),
                if card.branch_deleted && !card.head.is_empty() {
                    format!("{} was deleted · {}", card.head, ago(when))
                } else {
                    ago(when)
                },
            ),
            Phase::Closed => (
                Icon::new(IconName::CircleX)
                    .size(px(12.))
                    .text_color(muted)
                    .into_any_element(),
                muted,
                "Closed unmerged".to_string(),
                ago(when),
            ),
            Phase::Failed => (
                Icon::new(IconName::CircleAlert)
                    .size(px(12.))
                    .text_color(red)
                    .into_any_element(),
                red,
                "Couldn't open the pull request".to_string(),
                card.step.clone(),
            ),
            _ => return None,
        },
        Kind::Run => match card.phase {
            Phase::Passed => (
                Icon::new(IconName::CircleCheck)
                    .size(px(12.))
                    .text_color(green)
                    .into_any_element(),
                green,
                "All jobs passed".to_string(),
                format!(
                    "{} jobs · {}",
                    card.stages.len(),
                    dur_label(card.ended_ms.max(now_ms()) - card.started_ms)
                ),
            ),
            Phase::Failed => (
                Icon::new(IconName::CircleX)
                    .size(px(12.))
                    .text_color(red)
                    .into_any_element(),
                red,
                "Run failed".to_string(),
                card.step.clone(),
            ),
            _ => return None,
        },
        Kind::Release => match card.phase {
            Phase::Published => (
                Icon::new(crate::assets::Lucide::Rocket)
                    .size(px(12.))
                    .text_color(indigo)
                    .into_any_element(),
                indigo,
                format!("Published v{}", card.version),
                if card.channel.is_empty() {
                    ago(when)
                } else {
                    format!("{} channel · {}", card.channel, ago(when))
                },
            ),
            Phase::Failed => (
                Icon::new(IconName::CircleAlert)
                    .size(px(12.))
                    .text_color(red)
                    .into_any_element(),
                red,
                "Release failed".to_string(),
                card.step.clone(),
            ),
            _ => return None,
        },
    };

    let mut chips = vec![];
    match card.kind {
        Kind::Pr => {
            chips.extend(
                card.resolved
                    .iter()
                    .take(6)
                    .map(|f| file_chip(&lang_tag(f), f, Some(("✓", green)), cx)),
            );
            chips.extend(
                card.conflicts
                    .iter()
                    .take(6)
                    .map(|f| file_chip(&lang_tag(f), f, Some(("!", amber)), cx)),
            );
        }
        Kind::Run | Kind::Release if card.phase == Phase::Failed => {
            chips.extend(
                card.stages
                    .iter()
                    .filter(|s| s.state == CheckState::Fail)
                    .take(6)
                    .map(|s| file_chip("", &s.label, Some(("✕", red)), cx)),
            );
        }
        _ => {}
    }

    // The right-hand action: a dark filled pill while it asks for a click, a quiet busy one after.
    let action = match (card.kind, card.phase) {
        (Kind::Pr, Phase::Conflicted) if !card.resolving => actions
            .resolve
            .clone()
            .map(|f| action_pill(("pr-resolve", ix), None, "Resolve conflicts", f, cx)),
        (Kind::Pr, Phase::Conflicted) => Some(busy_pill(("pr-resolve", ix), "Resolving", cx)),
        (Kind::Pr, Phase::Ready) => actions.merge.clone().map(|f| {
            action_pill(
                ("pr-merge", ix),
                Some(crate::assets::Lucide::GitMerge),
                "Merge",
                f,
                cx,
            )
        }),
        (Kind::Pr, Phase::Merging) => Some(busy_pill(("pr-merge", ix), "Merging", cx)),
        (Kind::Run, Phase::Failed) | (Kind::Release, Phase::Failed) => actions
            .open
            .clone()
            .map(|f| action_pill(("pr-logs", ix), None, "View logs", f, cx)),
        (Kind::Release, Phase::Published) => actions
            .open
            .clone()
            .map(|f| action_pill(("pr-logs", ix), None, "View release", f, cx)),
        _ => None,
    };

    Some(
        v_flex()
            .border_t_1()
            .border_color(theme.foreground.opacity(0.07))
            .px(px(14.))
            .py(px(9.))
            .gap(px(7.))
            .child(
                h_flex()
                    .gap(px(9.))
                    .items_center()
                    .child(
                        div()
                            .size(px(22.))
                            .flex_none()
                            .rounded(px(6.))
                            .bg(tint.opacity(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_medium()
                                    .text_color(theme.foreground.opacity(0.9))
                                    .child(title),
                            )
                            .when(!sub.is_empty(), |el| {
                                el.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(sub),
                                )
                            }),
                    )
                    .children(action),
            )
            .when(!chips.is_empty(), |el| {
                el.child(h_flex().pl(px(31.)).gap(px(5.)).flex_wrap().children(chips))
            })
            .into_any_element(),
    )
}

/// The footer's call to act: a filled pill in the text colour (dark on Paper, light on Night),
/// as the reference draws "Resolve conflicts" and "Merge".
fn action_pill(
    id: impl Into<ElementId>,
    icon: Option<crate::assets::Lucide>,
    label: &'static str,
    on_click: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fg, bg) = (theme.background, theme.foreground);
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .gap(px(5.))
        .items_center()
        .h(px(24.))
        .px(px(11.))
        .rounded_full()
        .bg(bg.opacity(0.92))
        .hover(move |s| s.bg(bg))
        .cursor_pointer()
        .text_size(px(12.))
        .font_medium()
        .text_color(fg)
        .when_some(icon, |el, icon| {
            el.child(Icon::new(icon).size(px(12.)).text_color(fg))
        })
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

/// The same place once clicked: a quiet grey pill with a small spinner.
fn busy_pill(id: impl Into<ElementId>, label: &'static str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .id(id.into())
        .test_support()
        .aria_label(label)
        .flex_none()
        .gap(px(6.))
        .items_center()
        .h(px(24.))
        .px(px(11.))
        .rounded_full()
        .bg(theme.foreground.opacity(0.06))
        .text_size(px(12.))
        .text_color(theme.muted_foreground)
        .child(Spinner::new().xsmall().color(theme.muted_foreground))
        .child(label)
        .into_any_element()
}

// ---------- the graph band ----------

/// The band's height: room for two lanes and their curves (a run's single track).
const BAND_H: f32 = 66.;
/// A PR's band: main, the branch below it, and the branch's name under that, clear of its line.
const PR_BAND_H: f32 = 86.;
/// The gap between a release's lanes, so each has room for its name.
const LANE_GAP: f32 = 20.;

/// How tall `card`'s band is.
fn band_height(card: &Card) -> f32 {
    match card.kind {
        Kind::Pr => PR_BAND_H,
        Kind::Run => BAND_H,
        Kind::Release => 24. + LANE_GAP * card.stages.len().clamp(1, 4) as f32,
    }
}

/// Where a release's lane `i` runs.
fn lane_y(i: usize) -> f32 {
    22. + LANE_GAP * i as f32
}

/// The graph strip under the header, per kind.
fn band(card: &Card, clock: f32, draw: f32, pop: f32, motion: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let strip = div()
        .relative()
        .h(px(band_height(card)))
        .w_full()
        .overflow_hidden()
        .bg(theme.muted);
    match card.kind {
        Kind::Pr => strip
            .child(pr_graph(card, clock, draw, pop, motion, cx))
            .children(pr_labels(card, cx))
            .into_any_element(),
        Kind::Run => strip
            .child(pipeline_graph(card, clock, draw, pop, motion, cx))
            .children(pipeline_labels(card, cx))
            .into_any_element(),
        Kind::Release => strip
            .child(lanes_graph(card, clock, draw, pop, motion, cx))
            .children(lane_labels(card, cx))
            .into_any_element(),
    }
}

/// The small soft pill above the graph's target node.
fn band_pill(text: String, tint: Hsla) -> AnyElement {
    div()
        .flex_none()
        .rounded_full()
        .px(px(6.))
        .py(px(1.))
        .bg(tint.opacity(0.12))
        .text_size(px(10.))
        .text_color(tint)
        .child(text)
        .into_any_element()
}

// ---- painted helpers (as `panels::branch_graph` draws them) ----

fn clamp01(v: f32) -> f32 {
    v.clamp(0., 1.)
}

/// `t` of the way from `p0` to `p1` on a cubic Bézier.
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

/// Stroke `points` as a polyline.
fn seg(points: &[Point<Pixels>], width: Pixels, dash: bool, color: Hsla, window: &mut Window) {
    if points.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(width);
    if dash {
        b = b.dash_array(&[px(3.5), px(3.)]);
    }
    b.move_to(points[0]);
    for p in &points[1..] {
        b.line_to(*p);
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// A stroked cubic, drawn `t` of the way.
fn barc(
    p0: Point<Pixels>,
    c1: Point<Pixels>,
    c2: Point<Pixels>,
    p1: Point<Pixels>,
    t: f32,
    width: Pixels,
    dash: bool,
    color: Hsla,
    window: &mut Window,
) {
    let t = clamp01(t);
    if t <= 0. {
        return;
    }
    let n = 20;
    let points: Vec<Point<Pixels>> = (0..=n)
        .map(|i| bcubic(p0, c1, c2, p1, t * i as f32 / n as f32))
        .collect();
    seg(&points, width, dash, color, window);
}

/// A filled circle.
fn dot(at: Point<Pixels>, r: f32, color: Hsla, window: &mut Window) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - r), px(at.y.as_f32() - r)),
                size(px(2. * r), px(2. * r)),
            ),
            color,
        )
        .corner_radii(px(r)),
    );
}

/// A hollow circle, filled so what's under it doesn't show through.
fn ring(at: Point<Pixels>, r: f32, w: f32, color: Hsla, fill_c: Hsla, window: &mut Window) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - r), px(at.y.as_f32() - r)),
                size(px(2. * r), px(2. * r)),
            ),
            fill_c,
        )
        .corner_radii(px(r))
        .border_widths(px(w))
        .border_color(color),
    );
}

/// A ✓ inside a node's dot.
fn tick(at: Point<Pixels>, color: Hsla, window: &mut Window) {
    let (x, y) = (at.x.as_f32(), at.y.as_f32());
    seg(
        &[
            point(px(x - 2.2), px(y + 0.1)),
            point(px(x - 0.6), px(y + 1.7)),
            point(px(x + 2.4), px(y - 1.6)),
        ],
        px(1.4),
        false,
        color,
        window,
    );
}

/// A ✕ inside a node's dot.
fn ex(at: Point<Pixels>, color: Hsla, window: &mut Window) {
    let (x, y) = (at.x.as_f32(), at.y.as_f32());
    for (a, b, c, d) in [(-1.6, -1.6, 1.6, 1.6), (-1.6, 1.6, 1.6, -1.6)] {
        seg(
            &[point(px(x + a), px(y + b)), point(px(x + c), px(y + d))],
            px(1.4),
            false,
            color,
            window,
        );
    }
}

/// A ! inside a node's dot.
fn bang(at: Point<Pixels>, color: Hsla, window: &mut Window) {
    let (x, y) = (at.x.as_f32(), at.y.as_f32());
    seg(
        &[point(px(x), px(y - 2.1)), point(px(x), px(y + 0.3))],
        px(1.4),
        false,
        color,
        window,
    );
    dot(at + point(px(0.), px(2.2)), 0.9, color, window);
}

/// A three-quarter arc turning at `at` — the spinner ring live phases wear.
fn spin_arc(at: Point<Pixels>, r: f32, clock: f32, color: Hsla, window: &mut Window) {
    let mut b = PathBuilder::stroke(px(1.4));
    let steps = 24;
    let start = clock * std::f32::consts::TAU;
    for i in 0..=steps * 3 / 4 {
        let a = start + (i as f32 / steps as f32) * std::f32::consts::TAU;
        let p = point(at.x + px(r) * a.cos(), at.y + px(r) * a.sin());
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// The state colours a graph's marks wear (resolved before the paint closure: `cx` can't
/// be captured by the canvas).
#[derive(Clone, Copy)]
struct Marks {
    sky: Hsla,
    green: Hsla,
    red: Hsla,
    muted: Hsla,
}

fn marks(cx: &App) -> Marks {
    Marks {
        sky: crate::palette::sky(cx),
        green: crate::palette::emerald(cx),
        red: crate::palette::red(cx),
        muted: cx.theme().muted_foreground.opacity(0.6),
    }
}

/// A job or stage's node: its state as a mark.
fn job_mark(
    at: Point<Pixels>,
    state: CheckState,
    clock: f32,
    band_bg: Hsla,
    m: Marks,
    window: &mut Window,
) {
    let (sky, green, red, muted) = (m.sky, m.green, m.red, m.muted);
    match state {
        CheckState::Pass => {
            dot(at, 4.5, green, window);
            tick(at, band_bg, window);
        }
        CheckState::Fail => {
            dot(at, 4.5, red, window);
            ex(at, band_bg, window);
        }
        CheckState::Skipped => dot(at, 3.5, muted, window),
        CheckState::Queued => ring(at, 3.5, 1.4, muted, band_bg, window),
        CheckState::Running => {
            ring(at, 4., 1.4, sky, band_bg, window);
            spin_arc(at, 6.5, clock, sky, window);
        }
    }
}

// ---- PR: the branch graph ----

/// The pill over the merge target, per phase.
fn tip_label(card: &Card, cx: &App) -> Option<(String, Hsla)> {
    let muted = cx.theme().muted_foreground;
    match card.phase {
        Phase::Checking => Some(("Checking".to_string(), muted)),
        Phase::Conflicted => {
            let n = card.conflicts.len();
            if n == 0 {
                Some(("Resolved".to_string(), crate::palette::amber(cx)))
            } else if n == 1 {
                Some(("1 conflict".to_string(), crate::palette::amber(cx)))
            } else {
                Some((format!("{n} conflicts"), crate::palette::amber(cx)))
            }
        }
        Phase::Ready | Phase::Merging => {
            Some(("Ready to merge".to_string(), crate::palette::emerald(cx)))
        }
        Phase::Merged => Some(("Merged".to_string(), crate::palette::indigo(cx))),
        Phase::Closed => Some(("Closed".to_string(), crate::palette::red(cx))),
        Phase::Failed => Some(("Failed".to_string(), crate::palette::red(cx))),
        _ => None,
    }
}

/// `main` across the top of the band, the head branch curving off it, a merge curve home.
fn pr_graph(card: &Card, clock: f32, draw: f32, pop: f32, motion: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let band_bg = theme.muted;
    let muted = theme.muted_foreground.opacity(0.55);
    let tip = card.phase.tint(cx);
    let merged = card.phase == Phase::Merged;
    let branch = if merged { tip } else { crate::palette::sky(cx) };
    let card = card.clone();
    canvas(
        move |_, _, _| {},
        move |b, _, window, _| {
            let w = b.size.width.as_f32();
            let (ox, oy) = (b.origin.x.as_f32(), b.origin.y.as_f32());
            let at = |x: f32, y: f32| point(px(ox + x), px(oy + y));
            let (my, by) = (PR_BAND_H * 0.3, PR_BAND_H * 0.64);
            let (x0, x1) = (w * 0.08, w * 0.94);
            let fork = x0 + (x1 - x0) * 0.18;
            let tip_x = x0 + (x1 - x0) * 0.58;
            let join = x0 + (x1 - x0) * 0.94;
            let committing = card.phase == Phase::Creating && card.step.starts_with("Committing");
            let resolving = card.phase == Phase::Conflicted && card.resolving;
            let spinning =
                (matches!(card.phase, Phase::Checking | Phase::Merging) || resolving) && motion;
            let commits = card.commits.unwrap_or(3).clamp(1, 4) as usize;

            // main: its line and dots; two it fetched since the branch left it.
            seg(&[at(x0, my), at(x1, my)], px(1.5), false, muted, window);
            for i in 0..2 {
                dot(
                    at(x0 + (fork - x0) * (i as f32 + 0.5) / 2., my),
                    3.,
                    muted,
                    window,
                );
            }
            if card.phase != Phase::Creating {
                for i in 0..2 {
                    dot(
                        at(fork + (join - fork) * (i as f32 + 0.45) / 2.4, my),
                        3.,
                        muted,
                        window,
                    );
                }
            }

            // The branch: a curve down off the fork, then flat to `tip_x`; while it commits its
            // last stretch is still dashed and its new commit hollow.
            let flat = fork + 18.;
            barc(
                at(fork, my),
                at(fork, my + 18.),
                at(flat - 12., by),
                at(flat, by),
                1.,
                px(1.5),
                false,
                branch,
                window,
            );
            let tail = flat + (tip_x - flat) * 0.72;
            seg(
                &[at(flat, by), at(if committing { tail } else { tip_x }, by)],
                px(1.5),
                false,
                branch,
                window,
            );
            if committing {
                seg(
                    &[at(tail, by), at(tip_x, by)],
                    px(1.5),
                    true,
                    branch,
                    window,
                );
            }
            for i in 0..commits {
                let x = flat + (tip_x - flat) * (i as f32 + 0.6) / (commits as f32 + 0.4);
                if committing && i == commits - 1 {
                    ring(at(x.min(tail), by), 3.4, 1.4, branch, band_bg, window);
                } else {
                    dot(at(x, by), 3., branch, window);
                }
            }

            // Resolving pours main into the branch's end, whose ring then waits.
            if resolving {
                let mx = fork + (join - fork) * 0.62;
                barc(
                    at(mx, my),
                    at(mx, my + 22.),
                    at(tip_x + 26., by - 20.),
                    at(tip_x + 6., by),
                    draw,
                    px(1.5),
                    false,
                    tip,
                    window,
                );
                ring(at(tip_x, by), 4.5, 1.6, tip, band_bg, window);
            }

            // The merge curve: dashed while it's a "could", solid for the merge itself.
            if !matches!(card.phase, Phase::Creating | Phase::Closed | Phase::Failed) {
                let dashed = !matches!(card.phase, Phase::Merging | Phase::Merged);
                barc(
                    at(tip_x, by),
                    at(tip_x, by - 26.),
                    at(join, my + 26.),
                    at(join, my),
                    draw,
                    px(1.5),
                    dashed,
                    tip.opacity(if dashed { 0.85 } else { 1. }),
                    window,
                );
                let r = 4.5 * (1. + 0.35 * pop);
                match card.phase {
                    Phase::Open => ring(at(join, my), 3.4, 1.4, muted, band_bg, window),
                    Phase::Checking => {
                        dot(at(join, my), 3.4, tip.opacity(0.4), window);
                        spin_arc(at(join, my), 6., clock, tip, window);
                    }
                    Phase::Conflicted => {
                        dot(at(join, my), r, tip, window);
                        bang(at(join, my), band_bg, window);
                    }
                    Phase::Ready => {
                        dot(at(join, my), r + 1.5, tip, window);
                        tick(at(join, my), band_bg, window);
                    }
                    Phase::Merging => {
                        ring(at(join, my), r + 1.5, 1.6, tip, band_bg, window);
                        spin_arc(at(join, my), 8.5, clock, tip, window);
                    }
                    Phase::Merged => {
                        dot(at(join, my), r, tip, window);
                        dot(at(join, my), 1.8, band_bg, window);
                    }
                    _ => {}
                }
            }
            if (spinning || draw < 1.) && motion {
                window.request_animation_frame();
            }
        },
    )
    .w_full()
    .h(px(PR_BAND_H))
    .into_any_element()
}

/// The band's written marks: the base and head's names, the phase's pill over the target.
fn pr_labels(card: &Card, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let mut out = vec![];
    if !card.base.is_empty() {
        out.push(
            div()
                .absolute()
                .left(relative(0.045))
                .top(px(4.))
                .text_size(px(9.5))
                .font_family(mono.clone())
                .text_color(theme.muted_foreground)
                .child(card.base.clone())
                .into_any_element(),
        );
    }
    if !card.head.is_empty() {
        let c = if card.phase == Phase::Merged {
            crate::palette::indigo(cx)
        } else {
            crate::palette::sky(cx)
        };
        out.push(
            div()
                .absolute()
                .left(relative(0.27))
                .bottom(px(6.))
                .text_size(px(9.5))
                .font_family(mono)
                .text_color(c)
                .child(card.head.clone())
                .into_any_element(),
        );
    }
    if let Some((text, tint)) = tip_label(card, cx) {
        out.push(
            div()
                .absolute()
                .right(relative(0.04))
                .top(px(4.))
                .child(band_pill(text, tint))
                .into_any_element(),
        );
    }
    out
}

// ---- Run: the pipeline ----

fn pipeline_graph(
    card: &Card,
    clock: f32,
    draw: f32,
    pop: f32,
    motion: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let band_bg = theme.muted;
    let muted = theme.muted_foreground.opacity(0.55);
    let tip = state_tint(card, cx);
    let m = marks(cx);
    let jobs = card.stages.clone();
    let card = card.clone();
    canvas(
        move |_, _, _| {},
        move |b, _, window, _| {
            let w = b.size.width.as_f32();
            let (ox, oy) = (b.origin.x.as_f32(), b.origin.y.as_f32());
            let at = |x: f32, y: f32| point(px(ox + x), px(oy + y));
            let y = BAND_H * 0.42;
            let (x0, x1) = (w * 0.07, w * 0.78);
            let join = w * 0.90;
            let n = jobs.len();
            let node_x = |i: usize| x0 + (x1 - x0) * (i as f32 + 0.5) / n.max(1) as f32;

            // The track, filled in job colour where the work already passed through it.
            seg(&[at(x0, y), at(join, y)], px(1.5), false, muted, window);
            for (i, s) in jobs.iter().enumerate() {
                if s.state.pending() {
                    break;
                }
                let c = if s.state == CheckState::Fail {
                    m.red
                } else {
                    m.sky
                };
                seg(
                    &[
                        at(node_x(i), y),
                        at(if i + 1 < n { node_x(i + 1) } else { join }, y),
                    ],
                    px(1.5),
                    false,
                    c,
                    window,
                );
            }
            for (i, s) in jobs.iter().enumerate() {
                job_mark(at(node_x(i), y), s.state, clock, band_bg, m, window);
            }
            // The run's verdict node at the end.
            let r = 5. * (1. + 0.35 * pop);
            match card.phase {
                Phase::Passed => {
                    dot(at(join, y), r, tip, window);
                    tick(at(join, y), band_bg, window);
                }
                Phase::Failed => {
                    dot(at(join, y), r, tip, window);
                    ex(at(join, y), band_bg, window);
                }
                _ => {
                    ring(at(join, y), 4.5, 1.4, tip, band_bg, window);
                    spin_arc(at(join, y), 7., clock, tip, window);
                }
            }
            if motion && (matches!(card.phase, Phase::Checking) || draw < 1.) {
                window.request_animation_frame();
            }
        },
    )
    .w_full()
    .h(px(BAND_H))
    .into_any_element()
}

/// The run's job names under their nodes, its verdict over the target.
fn pipeline_labels(card: &Card, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let tip = state_tint(card, cx);
    let mut out = vec![];
    let n = card.stages.len();
    if n > 0 {
        out.push(
            div()
                .absolute()
                .left(relative(0.07))
                .right(relative(0.22))
                .bottom(px(6.))
                .child(h_flex().children(card.stages.iter().map(|s| {
                    h_flex().flex_1().min_w_0().justify_center().child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(9.5))
                            .text_color(theme.muted_foreground)
                            .child(s.label.clone()),
                    )
                })))
                .into_any_element(),
        );
    }
    let label = match card.phase {
        Phase::Passed => "Passed".to_string(),
        Phase::Failed => "Failed".to_string(),
        _ => {
            let done = card.stages.iter().filter(|s| !s.state.pending()).count();
            if n == 0 {
                "Running".to_string()
            } else {
                format!("{done} of {n} done")
            }
        }
    };
    out.push(
        div()
            .absolute()
            .right(relative(0.05))
            .top(px(4.))
            .child(band_pill(label, tip))
            .into_any_element(),
    );
    out
}

// ---- Release: lanes converging on the publish ----

fn lanes_graph(card: &Card, clock: f32, draw: f32, pop: f32, motion: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let band_bg = theme.muted;
    let muted = theme.muted_foreground.opacity(0.55);
    let tip = state_tint(card, cx);
    let m = marks(cx);
    let stages = card.stages.clone();
    let height = band_height(card);
    let card = card.clone();
    canvas(
        move |_, _, _| {},
        move |b, _, window, _| {
            let w = b.size.width.as_f32();
            let (ox, oy) = (b.origin.x.as_f32(), b.origin.y.as_f32());
            let at = |x: f32, y: f32| point(px(ox + x), px(oy + y));
            let n = stages.len().clamp(1, 4);
            let mid = lane_y(0) + LANE_GAP * (n as f32 - 1.) / 2.;
            let (xl, xr) = (w * 0.13, w * 0.62);
            let join = w * 0.88;
            for (i, s) in stages.iter().take(4).enumerate() {
                let y = lane_y(i);
                let c = match s.state {
                    CheckState::Pass => m.green,
                    CheckState::Fail => m.red,
                    CheckState::Running => m.sky,
                    _ => muted,
                };
                // The lane, then the curve into the publish.
                seg(&[at(xl, y), at(xr, y)], px(1.5), false, c, window);
                job_mark(at(xr, y), s.state, clock, band_bg, m, window);
                barc(
                    at(xr, y),
                    at(xr + 30., y),
                    at(join - 40., mid),
                    at(join, mid),
                    if s.state.pending() { draw * 0.7 } else { 1. },
                    px(1.5),
                    s.state.pending(),
                    c.opacity(0.85),
                    window,
                );
            }
            let r = 5. * (1. + 0.35 * pop);
            match card.phase {
                Phase::Published => {
                    dot(at(join, mid), r, tip, window);
                    tick(at(join, mid), band_bg, window);
                }
                Phase::Failed => {
                    dot(at(join, mid), r, tip, window);
                    ex(at(join, mid), band_bg, window);
                }
                _ => {
                    ring(at(join, mid), 4.5, 1.4, tip, band_bg, window);
                    spin_arc(at(join, mid), 7., clock, tip, window);
                }
            }
            if motion && (matches!(card.phase, Phase::Releasing) || draw < 1.) {
                window.request_animation_frame();
            }
        },
    )
    .w_full()
    .h(px(height))
    .into_any_element()
}

/// The release's lane names at the left edge, its verdict over the target.
fn lane_labels(card: &Card, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let tip = state_tint(card, cx);
    let mut out = vec![];
    for (i, s) in card.stages.iter().take(4).enumerate() {
        let y = lane_y(i);
        out.push(
            div()
                .absolute()
                .left(px(12.))
                .top(px(y - 7.))
                .text_size(px(9.5))
                .text_color(theme.muted_foreground)
                .max_w(relative(0.18))
                .truncate()
                .child(s.label.clone())
                .into_any_element(),
        );
    }
    let label = match card.phase {
        Phase::Published => "Published",
        Phase::Failed => "Failed",
        _ => "Releasing",
    };
    out.push(
        div()
            .absolute()
            .right(relative(0.05))
            .top(px(4.))
            .child(band_pill(label.to_string(), tip))
            .into_any_element(),
    );
    out
}

/// A card in `state` with made-up contents, for design review (`shots`' `pr-demo`).
/// Nothing here is fetched: it's drawn as given.
#[cfg(feature = "shots")]
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
        added: Some(128),
        removed: Some(36),
        created_ms: now - 300_000,
        phase_at_ms: now - 20_000,
        ..Default::default()
    };
    let conflicts = vec![
        "src/middleware.ts".into(),
        "src/auth.ts".into(),
        "src/session.rs".into(),
    ];
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
            step: "3 conflicts with main".into(),
            conflicts,
            ..base
        },
        "resolving" => Card {
            phase: Phase::Conflicted,
            resolving: true,
            step: "Resolving conflict 2 of 3".into(),
            resolved: vec!["src/middleware.ts".into()],
            conflicts: vec!["src/auth.ts".into(), "src/session.rs".into()],
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
            merging: true,
            step: "Merging the pull request".into(),
            checks: vec![],
            commits: Some(4),
            ..base
        },
        "merged" => Card {
            phase: Phase::Merged,
            branch_deleted: true,
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
        "release" => Card {
            kind: Kind::Release,
            title: "Release v0.4.12".into(),
            version: "0.4.12".into(),
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
        "release-published" => Card {
            kind: Kind::Release,
            title: "Release v0.4.12".into(),
            version: "0.4.12".into(),
            channel: "stable".into(),
            number: None,
            url: Some("https://github.com/dokyit/Trek/releases/tag/v0.4.12".into()),
            head: String::new(),
            phase: Phase::Published,
            step: "Published".into(),
            stages: vec![
                Stage {
                    label: "macOS".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Windows".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "publish".into(),
                    state: CheckState::Pass,
                },
            ],
            ..base
        },
        "release-failed" => Card {
            kind: Kind::Release,
            title: "Release v0.4.12".into(),
            version: "0.4.12".into(),
            channel: "beta".into(),
            number: None,
            url: Some("https://github.com/dokyit/Trek/actions/runs/1".into()),
            head: String::new(),
            phase: Phase::Failed,
            step: "Release workflow failed".into(),
            stages: vec![
                Stage {
                    label: "macOS".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Windows".into(),
                    state: CheckState::Fail,
                },
                Stage {
                    label: "publish".into(),
                    state: CheckState::Skipped,
                },
            ],
            ..base
        },
        "run" | "run-running" => Card {
            kind: Kind::Run,
            title: "CI".into(),
            number: Some(39779843418),
            url: Some("https://github.com/dokyit/Trek/actions/runs/39779843418".into()),
            head: "main".into(),
            base: String::new(),
            subject: "Give the PR status chip a pill treatment".into(),
            commits: None,
            added: None,
            removed: None,
            phase: Phase::Checking,
            step: "Watching 2 jobs".into(),
            started_ms: now - 150_000,
            stages: vec![
                Stage {
                    label: "Clippy".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Test (macOS)".into(),
                    state: CheckState::Running,
                },
                Stage {
                    label: "Test (Windows)".into(),
                    state: CheckState::Queued,
                },
            ],
            ..base
        },
        "run-passed" => Card {
            kind: Kind::Run,
            title: "CI".into(),
            number: Some(39779843418),
            url: Some("https://github.com/dokyit/Trek/actions/runs/39779843418".into()),
            head: "main".into(),
            base: String::new(),
            subject: "Give the PR status chip a pill treatment".into(),
            commits: None,
            added: None,
            removed: None,
            phase: Phase::Passed,
            step: "Passed".into(),
            started_ms: now - 160_000,
            ended_ms: now - 20_000,
            stages: vec![
                Stage {
                    label: "Clippy".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Test (macOS)".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Test (Windows)".into(),
                    state: CheckState::Pass,
                },
            ],
            ..base
        },
        "run-failed" => Card {
            kind: Kind::Run,
            title: "CI".into(),
            number: Some(39779843418),
            url: Some("https://github.com/dokyit/Trek/actions/runs/39779843418".into()),
            head: "main".into(),
            base: String::new(),
            subject: "Give the PR status chip a pill treatment".into(),
            commits: None,
            added: None,
            removed: None,
            phase: Phase::Failed,
            step: "Failed: Test (Windows)".into(),
            started_ms: now - 160_000,
            ended_ms: now - 20_000,
            stages: vec![
                Stage {
                    label: "Clippy".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Test (macOS)".into(),
                    state: CheckState::Pass,
                },
                Stage {
                    label: "Test (Windows)".into(),
                    state: CheckState::Fail,
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
