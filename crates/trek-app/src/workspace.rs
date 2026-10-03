//! The application model: threads, live agent sessions, routing, updates. Views observe it.

mod worktrees;

use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter, Task};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use trek_agents::{AcpInfo, AgentEvent, AgentStatus, Billing, Command, Decision, McpServer, SessionConfig, SlashCommand};
use trek_core::catalog::{self, ModelInfo};
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::import::ImportSummary;
use trek_core::settings::{FollowUp, Settings};
use trek_core::rewind::Reopen;
use trek_core::store::{Item, Project, ResumePoint, SearchHit, Section, Store, Thread, ToolStatus, now_ms};
use trek_core::transcript::Transcript;
use trek_core::{AgentId, Effort, HandHolding, RunState, ThreadSource};

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Composing a new thread in a project.
    Draft { project: Option<PathBuf> },
    Thread(String),
    Settings(SettingsPage),
    Onboarding,
}

/// What a transcript or composer is bound to: whatever the main window shows, or one thread
/// (a thread window, which never follows the main window's route).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Main,
    Thread(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Project,
    General,
    Appearance,
    Notifications,
    Snapshots,
    Shortcuts,
    Agents,
    Skills,
    Tools,
    ApiKeys,
    LocalModels,
    Permissions,
    Import,
    Updates,
    About,
}

impl SettingsPage {
    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::Project => "Project",
            SettingsPage::General => "General",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Notifications => "Notifications",
            SettingsPage::Snapshots => "App Snapshots",
            SettingsPage::Skills => "Skills",
            SettingsPage::Shortcuts => "Keyboard Shortcuts",
            SettingsPage::Agents => "Agents & Subscriptions",
            SettingsPage::Tools => "Tools & MCP",
            SettingsPage::ApiKeys => "API Keys",
            SettingsPage::LocalModels => "Local Models",
            SettingsPage::Permissions => "Permissions",
            SettingsPage::Import => "Import Threads",
            SettingsPage::Updates => "Updates",
            SettingsPage::About => "About",
        }
    }
}

/// Composer choices: agent, model, effort, hand-holding, plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Effort,
    pub hand_holding: HandHolding,
    pub plan: bool,
    pub fast: bool,
    /// Runs in a worktree of its own. Chosen on a draft (in a git project); fixed once it starts.
    pub worktree: bool,
}

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: String,
    pub title: String,
    pub detail: String,
    /// A question or a plan, when the agent asked for more than yes/no.
    pub prompt: Option<trek_agents::Prompt>,
    /// Offered once its turn was over (Codex's plan): it outlives the session that offered it,
    /// and the next message answers it instead.
    pub after_turn: bool,
}

/// In-memory state of an open thread: transcript plus its live agent session, if any.
#[derive(Default)]
pub struct LiveThread {
    /// The transcript, with each item's stable id. Change it through `Transcript`'s methods so
    /// saves (`persist_items`) write just the rows that changed.
    pub items: Transcript,
    pub loaded: bool,
    pub loading: bool,
    /// Its worktree is being made (or made again): messages wait for it, as while `loading`.
    pub preparing: bool,
    /// Index of the assistant item currently streaming.
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub permissions: Vec<PendingPermission>,
    /// Options picked so far on the question card: (request id, question index) → labels.
    pub picks: HashMap<(String, usize), Vec<String>>,
    pub commands: Option<async_channel::Sender<Command>>,
    pub turn_started: Option<Instant>,
    pub plan: bool,
    pub fast: bool,
    /// Estimated spend on this thread's agent so far; money only when `billing` is metered.
    pub cost_usd: f64,
    /// The agent's running session total at its last report (see `cost_added`).
    cost_total: f64,
    /// How the current session is billed, once the agent has said.
    pub billing: Option<Billing>,
    /// Tokens in the context window and the window size, as last reported by the agent.
    pub context: Option<(u64, u64)>,
    /// Bumped on every transcript change so views can resync cheaply.
    pub revision: u64,
    /// Follow-ups held while a turn runs (`FollowUp::Queue`), sent one per finished turn.
    pub queued: Vec<(String, Vec<PathBuf>)>,
    /// Sub-agents launched this turn, keyed by the tool call that started them.
    pub tasks: Vec<SubTask>,
    /// Background sub-agents still running; the turn isn't over until this reaches zero.
    pub background: usize,
    /// Last prompt or agent event (idle sessions are shut down; they resume on the next message).
    pub last_active: Option<Instant>,
    /// The latest point the agent's session can be taken back to (`AgentEvent::Mark`); saved to
    /// the thread as turns end (`Thread::native_at`).
    pub mark: Option<String>,
    /// Messages with a file checkpoint (as the store has them).
    pub checkpointed: HashSet<String>,
    /// Messages whose checkpoint couldn't be taken, and why.
    pub checkpoint_failed: HashMap<String, String>,
    /// Git work for this thread (checkpoints, restoring files), done in order off the main thread.
    git_jobs: VecDeque<GitJob>,
    git_busy: bool,
    /// Shared with the git work under way, so deleting the thread can wait for it.
    git_guard: std::sync::Arc<GitGuard>,
    /// Messages for the agent held until the git work before them is done: a turn mustn't start
    /// before its checkpoint is taken, or before files a rewind restores are back.
    held: Vec<Command>,
    _git: Option<Task<()>>,
    /// A save of the transcript on its way (`persist_soon`).
    _save_soon: Option<Task<()>>,
    _events: Option<Task<()>>,
}

/// Held by a thread's git work while it runs. Once the thread is deleted (`gone`), work that
/// hadn't started does nothing, and the cleanup takes the lock, so it runs after any that had.
#[derive(Default)]
struct GitGuard {
    lock: std::sync::Mutex<()>,
    gone: std::sync::atomic::AtomicBool,
}

/// Work queued on a thread before its next message reaches the agent (see `LiveThread::git_jobs`):
/// git, mostly.
#[derive(Debug, Clone)]
enum GitJob {
    /// Snapshot the files in `cwd` (if it's in a git repo) as message `item` goes out.
    Checkpoint { cwd: PathBuf, item: String },
    /// Find where the agent's `session` stands (`trek_agents::session_tail`), for message `item`
    /// sent in a thread that doesn't know yet (imported, or kept by an older Trek).
    FindPoint { agent: AgentId, session: String, item: String },
    /// Put the files back as checkpoint `sha` of `repo` has them.
    Restore { repo: PathBuf, sha: String },
    /// Drop checkpoints by message (theirs left the transcript).
    Forget { repo: PathBuf, items: Vec<String> },
    /// Give a fork the checkpoints of the messages it copied: `(item, commit)`.
    Link { repo: PathBuf, checkpoints: Vec<(String, String)> },
}

/// What a `GitJob` came to.
enum GitDone {
    Nothing,
    /// A checkpoint was taken.
    Taken,
    Restored(trek_core::checkpoint::Restored),
    /// Where the session stands (`GitJob::FindPoint`).
    Point(Option<String>),
}

/// Why a message has no file checkpoint to go back to.
#[derive(Debug, Clone, PartialEq)]
pub enum NoCheckpoint {
    /// The thread's folder isn't in a git repository.
    NotGit,
    /// Sent while a turn ran, it steered that turn: the turn's first message has its checkpoint.
    Steered,
    /// Taking it failed, and why.
    Failed(String),
    /// Sent before Trek kept checkpoints, or outside Trek (imported history).
    Missing,
    /// It has one, but the thread's worktree is missing: it can be restored once that's back.
    WorktreeMissing,
}

impl NoCheckpoint {
    /// Why, for a tooltip.
    pub fn short(&self) -> &'static str {
        match self {
            NoCheckpoint::NotGit => "files aren't restored: not a git repository",
            NoCheckpoint::Steered => "files aren't restored: sent mid-turn",
            NoCheckpoint::Failed(_) => "files aren't restored: the checkpoint failed",
            NoCheckpoint::Missing => "files aren't restored: no checkpoint",
            NoCheckpoint::WorktreeMissing => "files aren't restored: the worktree is missing",
        }
    }

    /// Why, in full.
    pub fn explain(&self) -> String {
        match self {
            NoCheckpoint::NotGit => "This folder isn't a git repository, so Trek keeps no file checkpoints for it.".into(),
            NoCheckpoint::Steered => "This message was sent while a turn was running, so it has no file checkpoint of its own; undo that turn to put the files back.".into(),
            NoCheckpoint::Failed(why) => format!("Trek couldn't take a file checkpoint when this was sent ({why}), so the files stay as they are."),
            NoCheckpoint::Missing => "There's no file checkpoint from before this message (it was sent before Trek kept them, outside Trek, or in a worktree since removed), so the files stay as they are.".into(),
            NoCheckpoint::WorktreeMissing => "This thread's worktree is missing, so its files can't be restored now. To put them back too, recreate it from its branch first.".into(),
        }
    }

    /// Why message `pos` of `live` can't have its files restored now; `None` when it can.
    /// `worktree_missing`: the thread's worktree folder is gone (see `Workspace::restorable_checkpoint`);
    /// it was a git checkout, whatever the folder left behind looks like.
    pub fn now(live: &LiveThread, pos: usize, in_repo: bool, worktree_missing: bool) -> Option<NoCheckpoint> {
        if worktree_missing {
            return Some(NoCheckpoint::of(live, pos, true).unwrap_or(NoCheckpoint::WorktreeMissing));
        }
        NoCheckpoint::of(live, pos, in_repo)
    }

    /// Why message `pos` of `live` has no checkpoint; `None` when it has one. `in_repo`: the
    /// thread's folder is in a git repository.
    pub fn of(live: &LiveThread, pos: usize, in_repo: bool) -> Option<NoCheckpoint> {
        let item = live.items.id_at(pos)?;
        if live.checkpointed.contains(item) {
            return None;
        }
        Some(if let Some(why) = live.checkpoint_failed.get(item) {
            NoCheckpoint::Failed(why.clone())
        } else if !in_repo {
            NoCheckpoint::NotGit
        } else if trek_core::rewind::turn_start(&live.items, pos + 1).is_some_and(|start| start != pos) {
            NoCheckpoint::Steered
        } else {
            NoCheckpoint::Missing
        })
    }
}

/// Whether `t` runs in a worktree whose folder is gone.
pub fn worktree_missing(t: &Thread) -> bool {
    t.worktree.as_ref().is_some_and(|w| w.is_missing())
}

/// Whether `cwd` is in a git repository (as checkpoints see it).
pub fn in_repo(cwd: Option<&std::path::Path>) -> bool {
    cwd.is_some_and(|c| trek_core::store::project_root(c).join(".git").exists())
}

/// Where a fork takes its conversation up to.
#[derive(Debug, Clone, PartialEq)]
pub enum ForkAt {
    /// Up to just before this message (by item id); the message waits in the fork's composer.
    Before(String),
    /// Up to the end of the turn this `TurnEnd` (by item id) closes.
    After(String),
    /// The whole conversation.
    End,
}

/// A sub-agent's progress, shown on its tool row and in the working bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SubTask {
    pub id: String,
    pub description: String,
    /// What it's doing right now ("Running grep …").
    pub activity: String,
    pub tool_uses: u64,
    /// `Some(ok)` once finished.
    pub done: Option<bool>,
}

impl LiveThread {
    pub fn active_tasks(&self) -> usize {
        self.tasks.iter().filter(|t| t.done.is_none()).count()
    }

    /// The turn is over: open sub-agents and running tool rows end with it, done or failed.
    fn close_turn(&mut self, ok: bool) {
        for t in self.tasks.iter_mut().filter(|t| t.done.is_none()) {
            t.done = Some(ok);
        }
        for ix in 0..self.items.len() {
            if matches!(self.items[ix], Item::Tool { status: ToolStatus::Running, .. }) {
                if let Some(Item::Tool { status, .. }) = self.items.get_mut(ix) {
                    *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                }
            }
        }
        self.background = 0;
    }

    /// The prompt `request_id` is settled: its card and picks go.
    fn settle_prompt(&mut self, request_id: &str) {
        self.permissions.retain(|p| p.request_id != request_id);
        self.picks.retain(|(rid, _), _| rid != request_id);
    }

    /// Drop prompts offered after their turn: a new message answers them instead.
    fn drop_after_turn(&mut self) {
        let stale: Vec<String> = self.permissions.iter().filter(|p| p.after_turn).map(|p| p.request_id.clone()).collect();
        for rid in stale {
            self.settle_prompt(&rid);
        }
    }

    /// Stable ids of the transcript's items, parallel to `items` (they survive reloads once saved).
    pub fn item_ids(&self) -> &[String] {
        self.items.ids()
    }
}

/// A message to bring into view: by stable id, or by position for history that isn't stored in
/// Trek (an imported thread you haven't continued here).
#[derive(Debug, Clone, PartialEq)]
pub enum ItemRef {
    Id(String),
    Position(usize),
}

impl ItemRef {
    pub fn of_hit(hit: &SearchHit) -> Option<ItemRef> {
        match (&hit.item_id, hit.position) {
            (Some(id), _) => Some(ItemRef::Id(id.clone())),
            (None, Some(p)) => Some(ItemRef::Position(p)),
            (None, None) => None,
        }
    }
}

/// Answers for a question card when the user typed their own: `typed` answers the first question
/// nothing was picked for (the last one if all have picks), picks answer the rest, and questions
/// left open go unanswered. Also says whether the typed answer is a secret.
/// Card answers as the user's message: the answer alone for one question, else one line per
/// question under its short header. Secret answers are left out; `true` if there were any.
fn answers_text(questions: &[trek_agents::Question], answers: &[(String, String)]) -> (String, bool) {
    let mut secret = false;
    let mut lines = Vec::new();
    for (question, answer) in answers {
        let q = questions.iter().find(|q| q.question == *question);
        if q.is_some_and(|q| q.secret) {
            secret = true;
            continue;
        }
        let label = q.map(|q| if q.header.trim().is_empty() { q.question.as_str() } else { q.header.as_str() }).unwrap_or(question);
        lines.push((label.trim().to_string(), answer.trim().to_string()));
    }
    let text = match lines.as_slice() {
        [(_, answer)] if answers.len() == 1 => answer.clone(),
        _ => lines.iter().map(|(label, answer)| format!("{label}: {answer}")).collect::<Vec<_>>().join("\n"),
    };
    (text, secret)
}

fn typed_answers(questions: &[trek_agents::Question], picked: impl Fn(usize) -> Option<String>, typed: &str) -> (Vec<(String, String)>, bool) {
    let target = (0..questions.len()).find(|i| picked(*i).is_none()).unwrap_or(questions.len().saturating_sub(1));
    let answers = questions
        .iter()
        .enumerate()
        .filter_map(|(i, q)| if i == target { Some(typed.to_string()) } else { picked(i) }.map(|a| (q.question.clone(), a)))
        .collect();
    (answers, questions.get(target).is_some_and(|q| q.secret))
}

/// Scroll the transcript to a message once the thread is on screen (search results).
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    pub thread: String,
    pub item: ItemRef,
    /// Increases with every request, so the view can tell a new one from one it has handled.
    pub seq: u64,
}

/// Full-text results for the sidebar's search text.
#[derive(Debug, Default)]
pub struct SearchResults {
    /// The text they answer; while a newer search runs, results for a text it extends (or that
    /// extends it) stay up rather than blink out.
    pub query: String,
    /// Threads whose title matched: word prefixes, in any order, accents folded.
    pub titles: HashSet<String>,
    /// Threads whose messages matched, with the best match in each.
    pub messages: HashMap<String, SearchHit>,
}

impl SearchResults {
    fn from_hits(query: String, hits: Vec<SearchHit>) -> Self {
        let mut out = SearchResults { query, ..Default::default() };
        for h in hits {
            if h.position.is_some() {
                out.messages.insert(h.thread_id.clone(), h);
            } else {
                out.titles.insert(h.thread_id);
            }
        }
        out
    }

    /// Whether these results can stand in for `query`'s while its own search runs.
    fn relevant_to(&self, query: &str) -> bool {
        !self.query.is_empty() && (query.starts_with(&self.query) || self.query.starts_with(query))
    }

    fn title_matches(&self, query: &str, t: &Thread) -> bool {
        // The plain substring test answers at once, before the full-text results are in.
        t.title.to_lowercase().contains(query) || self.titles.contains(&t.id)
    }

    /// Whether `t` belongs in the list while searching for `query` (trimmed and lowercased, not
    /// empty).
    pub fn matches(&self, query: &str, t: &Thread) -> bool {
        self.title_matches(query, t) || self.messages.contains_key(&t.id)
    }

    /// The message that put `t` in the list, when its title didn't.
    pub fn content_hit(&self, query: &str, t: &Thread) -> Option<&SearchHit> {
        if self.title_matches(query, t) { None } else { self.messages.get(&t.id) }
    }
}

/// What a pre-warmed draft session was started with; it's used only if the draft still matches.
type WarmKey = (AgentId, PathBuf, Option<String>, Effort, HandHolding, bool, bool);

pub use crate::updater::{UpdateAction, UpdateStatus, UpdateView};

/// Tools that open as tabs in the right panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTool {
    Git,
    Explorer,
    Terminal,
    Browser,
    SideChat,
    Simulator,
}

impl PanelTool {
    pub const ALL: [PanelTool; 6] = [PanelTool::Terminal, PanelTool::Browser, PanelTool::Simulator, PanelTool::Explorer, PanelTool::SideChat, PanelTool::Git];
    pub fn label(self) -> &'static str {
        match self {
            PanelTool::Git => "Git",
            PanelTool::Explorer => "Explorer",
            PanelTool::Terminal => "Terminal",
            PanelTool::Browser => "Browser",
            PanelTool::SideChat => "Side chat",
            PanelTool::Simulator => "Simulator",
        }
    }
}

pub enum WorkspaceEvent {
    /// Open (or focus) a right-panel tool.
    OpenTool(PanelTool),
    /// A toast-worthy message with an optional undo.
    Toast { message: String, undo: Option<UndoAction> },
    /// A thread needs the user or finished: an in-app toast when it's off screen, plus a system
    /// banner / sound per the notification settings.
    Attention { message: String, thread: String },
    FocusComposer,
    /// Run a shell command in a new terminal tab (an agent install or sign-in, a project action),
    /// in `cwd` or else the folder on screen, then rescan agents.
    RunInTerminal { command: String, cwd: Option<PathBuf> },
    /// Insert text at the composer's cursor (e.g. an element picked in the browser).
    InsertIntoComposer(String),
    /// Attach an image to the composer (e.g. a browser screenshot).
    AttachImage(std::path::PathBuf),
    /// Follow-ups held for a turn that stopped or failed go back into the composer showing `thread`.
    RestoreQueued { thread: String, text: String, images: Vec<PathBuf> },
    /// A message goes back into the composer of `scope` (on `thread`): taken back by a rewind, or
    /// (`edit`: its item id) to be edited and sent again in its place.
    ComposeIn { scope: Scope, thread: String, text: String, images: Vec<PathBuf>, edit: Option<String> },
    /// Something in a thread window changed what the main window shows: bring it forward.
    ActivateMain,
    /// ⌘K from a thread window: the palette opens in the main window (its commands act there),
    /// reopened if it was closed (`root::show_palette`).
    OpenPalette,
    /// Only this thread's transcript changed (streamed text, tool calls). Sent instead of a
    /// notification, so views that don't draw transcripts aren't redrawn for every batch.
    /// `appended`: all that changed is text added to the messages already streaming.
    Transcript { id: String, appended: bool },
}

#[derive(Debug, Clone)]
pub enum UndoAction {
    Unsettle(String),
    Unarchive(String),
    /// Put the files of `repo` back as they were before a restore (`Restored::undo`).
    Unrestore { thread: String, repo: PathBuf, sha: String },
    /// Call off a restart to update that's counting down (`RESTART_GRACE`); it waits for a click
    /// or a quit again.
    CancelRestart,
}

/// How long a restart the user asked for while agents worked waits once they're done: they may
/// be typing the next message by then, and unsent text doesn't survive a restart.
pub const RESTART_GRACE: Duration = Duration::from_secs(10);

/// Git state of the folder on screen, for the composer's branch chip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitInfo {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub changed: usize,
    pub ahead: u32,
    /// The remote's default branch (origin/HEAD), else main/master.
    pub default_branch: Option<String>,
    /// Local branches, most recently committed first.
    pub branches: Vec<String>,
    pub remote: Option<String>,
}

impl GitInfo {
    pub fn on_default(&self) -> bool {
        self.branch.is_some() && self.branch == self.default_branch
    }
}

fn read_git_info(cwd: &std::path::Path) -> GitInfo {
    let run = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(cwd).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if run(&["rev-parse", "--is-inside-work-tree"]).is_none() {
        return GitInfo::default();
    }
    GitInfo {
        is_repo: true,
        branch: run(&["branch", "--show-current"]).filter(|b| !b.is_empty()),
        // Every untracked file, not their folders, so the count matches the Git panel's list.
        changed: run(&["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0),
        ahead: run(&["rev-list", "--count", "@{u}..HEAD"]).and_then(|s| s.parse().ok()).unwrap_or(0),
        default_branch: run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .and_then(|s| s.split_once('/').map(|(_, b)| b.to_string()))
            .or_else(|| {
                let local = run(&["branch", "--format=%(refname:short)"]).unwrap_or_default();
                ["main", "master", "trunk"].iter().find(|b| local.lines().any(|l| l == **b)).map(|b| b.to_string())
            }),
        branches: run(&["for-each-ref", "--sort=-committerdate", "--count=20", "--format=%(refname:short)", "refs/heads"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
        remote: trek_core::store::git_remote(cwd),
    }
}

pub struct Workspace {
    pub store: Store,
    pub settings: Settings,
    pub threads: Vec<Thread>,
    pub projects: Vec<Project>,
    pub agents: Vec<DetectedAgent>,
    /// Live models from the user's Codex setup (custom providers included).
    pub codex_models: Vec<ModelInfo>,
    pub detecting: bool,
    pub importing: bool,
    pub import_summary: Option<ImportSummary>,
    pub live: HashMap<String, LiveThread>,
    pub route: Route,
    pub draft_prefs: Prefs,
    pub updater: crate::updater::Updater,
    pub sidebar_collapsed: bool,
    pub settled_open: bool,
    /// The sidebar's search text. Change it with `set_search`, which also searches messages.
    pub search: String,
    /// Full-text results for `search`.
    pub search_results: SearchResults,
    _search_task: Option<Task<()>>,
    /// Bumped when the search index takes in more than a turn's worth (background indexing, a
    /// finished turn), so searches on screen can run again.
    pub search_epoch: u64,
    /// The message the transcript should scroll to next (`open_thread_at`).
    pub reveal: Option<Reveal>,
    /// Imported transcripts and older rows are being added to the search index.
    indexing: bool,
    /// More was handed to the index while it ran (a big save): go round once more.
    index_again: bool,
    pub project_filter: Option<String>,
    /// Bumped whenever any agent turn finishes (tools refresh on it).
    pub turns_finished: u64,
    pub git_info: HashMap<PathBuf, GitInfo>,
    /// Account, plan, usage limits and slash commands per vendor CLI, keyed by `AgentId::key()`.
    pub agent_status: HashMap<String, AgentStatus>,
    pub status_fetched_at: i64,
    /// What each installed ACP agent reported (models, login state), keyed by `AgentId::key()`.
    pub acp_info: HashMap<String, Result<AcpInfo, String>>,
    /// Slash commands an agent offered in its last session in a folder: (`AgentId::key()`, cwd).
    agent_commands: HashMap<(String, PathBuf), Vec<SlashCommand>>,
    pub usage_loading: bool,
    /// The project open on Settings → Project (project id).
    pub settings_project: Option<String>,
    /// An agent session started while the user was still typing a new thread's first message.
    pub(crate) warm: Option<(WarmKey, trek_agents::SessionHandle, Instant)>,
    /// A `settle_merged` pass is running.
    checking_merges: bool,
    /// When `tidy_inbox` last ran: snoozes that ended since then bring their threads back.
    tidied_at: i64,
    /// A Trek menu is open over the window; native views (the browser) hide so they don't cover it.
    pub overlay_open: bool,
    pub main_window: Option<AnyWindowHandle>,
    /// Threads open in windows of their own.
    pub thread_windows: HashMap<String, AnyWindowHandle>,
    /// Offer Trek's scripted mock agent (`TREK_MOCK_AGENT=1`, and in tests).
    pub mock_agent: bool,
    /// Composer defaults as last copied into `draft_prefs` (agent, model, effort, hand-holding),
    /// so `save_settings` can tell when the user changed them.
    applied_defaults: (String, Option<String>, Effort, HandHolding),
    tasks: Vec<Task<()>>,
    /// A restart the user asked for while agents worked, counting down now that they're done.
    restart_countdown: Option<Task<()>>,
    /// Why Trek's database couldn't be opened, until the main window has said so: this session
    /// runs on an in-memory copy and nothing is saved.
    pub store_error: Option<String>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let (store, store_error) = match Store::open_default() {
            Ok(store) => (store, None),
            // The main window asks whether to quit or go on with nothing saved (`store_error`).
            Err(e) => {
                tracing::error!("database: {e:#}; nothing is saved this session");
                (Store::in_memory().expect("in-memory store"), Some(format!("{e:#}")))
            }
        };
        // Side chats an earlier run started from a draft can't be reopened: they go before any
        // opens. Their file checkpoints go in the background, with those of threads put away
        // long ago, which only keep objects alive in the user's repos.
        let orphans = store.drop_orphan_side_chats().unwrap_or_else(|e| {
            tracing::warn!("drop orphaned side chats: {e:#}");
            vec![]
        });
        let s = store.clone();
        cx.background_executor()
            .spawn(async move {
                for (thread, checkpoints) in orphans {
                    let mut repos: HashSet<PathBuf> = checkpoints.into_iter().map(|c| c.repo).collect();
                    repos.extend(thread.cwd.filter(|c| in_repo(Some(c))));
                    for repo in repos {
                        if let Some(Err(e)) = trek_core::checkpoint::Repo::find(&repo).map(|r| r.delete_all(&thread.id)) {
                            tracing::warn!("drop checkpoints of {}: {e:#}", thread.id);
                        }
                    }
                }
                if let Err(e) = trek_core::checkpoint::prune_stale(&s, now_ms()) {
                    tracing::warn!("prune old checkpoints: {e:#}");
                }
            })
            .detach();
        // TREK_ONBOARDING=1 replays onboarding without resetting anything (design review, support).
        let replay = std::env::var("TREK_ONBOARDING").is_ok_and(|v| v == "1");
        let mut this = Self::with(store, settings, cx);
        this.store_error = store_error;
        if replay {
            this.route = Route::Onboarding;
        }
        this.detect_agents(cx);
        this.refresh_git(cx);
        if this.settings.onboarding.completed {
            this.import_threads(cx);
        } else {
            this.index_for_search(cx);
        }
        // Only a bundled Trek manages updates; a dev build sharing the data folder leaves them be.
        let after_update = if trek_core::update::blocker().is_none() { trek_core::update::after_launch() } else { None };
        if let Some(from) = after_update {
            tracing::info!("updated from {from} to {}", trek_core::VERSION);
            // Spawned so it lands after the window has subscribed to workspace events.
            let message = format!("Trek updated to {} (from {from}).", trek_core::VERSION);
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            })
            .detach();
        }
        if this.settings.updates.auto_check {
            this.check_for_updates(false, cx);
        }
        this.start_housekeeping(cx);
        let keep = this.settings.snapshots.keep_days;
        cx.background_executor().spawn(async move { crate::mentions::prune_snapshots(keep) }).detach();
        // TREK_MOCK_PROMPT starts a mock thread at launch, for performance measurements and demos
        // that can't touch the UI. Only with the mock agent on.
        if let Some(prompt) = std::env::var("TREK_MOCK_PROMPT").ok().filter(|_| this.mock_agent) {
            this.draft_prefs.agent = AgentId::Direct(catalog::MOCK_PROVIDER.into());
            this.draft_prefs.model = None;
            this.send(prompt, vec![], cx);
        }
        this
    }

    /// The model over `store` and `settings` alone: no agent detection, import, update check or
    /// housekeeping is started (`new` adds those). Tests build on this.
    pub fn with(store: Store, settings: Settings, cx: &mut Context<Self>) -> Self {
        // No session runs yet: turns an earlier run left open (it quit or crashed mid-turn, or
        // with a card up) are over, and saying otherwise would leave them "Working" for good.
        match store.close_interrupted_turns() {
            Ok(closed) if !closed.is_empty() => tracing::info!("turns an earlier run left open, now closed: {}", closed.len()),
            Ok(_) => {}
            Err(e) => tracing::warn!("close interrupted turns: {e}"),
        }
        let route = if settings.onboarding.completed { Route::Draft { project: None } } else { Route::Onboarding };
        let applied_defaults = default_prefs_key(&settings);
        let draft_prefs = Prefs {
            agent: AgentId::from_key(&settings.general.default_agent),
            model: settings.general.default_model.clone(),
            effort: settings.general.default_effort,
            hand_holding: settings.general.hand_holding,
            plan: false,
            fast: false,
            worktree: false,
        };
        let mut this = Self {
            store,
            settings,
            threads: vec![],
            projects: vec![],
            agents: vec![],
            codex_models: vec![],
            detecting: false,
            importing: false,
            import_summary: None,
            live: HashMap::new(),
            route,
            draft_prefs,
            updater: Default::default(),
            sidebar_collapsed: false,
            settled_open: false,
            search: String::new(),
            search_results: SearchResults::default(),
            _search_task: None,
            search_epoch: 0,
            reveal: None,
            indexing: false,
            index_again: false,
            project_filter: None,
            turns_finished: 0,
            git_info: HashMap::new(),
            agent_status: HashMap::new(),
            agent_commands: HashMap::new(),
            status_fetched_at: 0,
            acp_info: HashMap::new(),
            usage_loading: false,
            settings_project: None,
            warm: None,
            checking_merges: false,
            tidied_at: 0,
            overlay_open: false,
            main_window: None,
            thread_windows: HashMap::new(),
            mock_agent: trek_agents::mock::enabled(),
            applied_defaults,
            tasks: vec![],
            restart_countdown: None,
            store_error: None,
        };
        this.reload(cx);
        if this.route == (Route::Draft { project: None }) {
            let first = this.workspace_projects().first().map(|p| p.path.clone()).or_else(|| this.projects.first().map(|p| p.path.clone()));
            // The first draft starts from its project's defaults, as drafts opened later do.
            if let Some(p) = &first {
                this.apply_project_defaults(p);
            }
            this.route = Route::Draft { project: first };
        }
        cx.on_app_quit(|this, _| {
            // What streamed since the last save is kept (a turn paused on a card, say); the next
            // launch closes the turns left open.
            this.persist_all();
            this.install_on_quit();
            async {}
        })
        .detach();
        this
    }

    // ---------- data ----------

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.threads = self.store.threads().unwrap_or_default();
        self.projects = self.store.projects().unwrap_or_default();
        cx.notify();
    }

    /// Show a session the import left out after all; later imports leave it in the sidebar.
    pub fn show_left_out(&mut self, session: &trek_core::import::ImportedThread, cx: &mut Context<Self>) {
        match self.store.keep_imported(session) {
            Ok(t) => {
                if let Some(summary) = self.import_summary.as_mut() {
                    summary.left_out.retain(|s| !(s.source == session.source && s.native_id == session.native_id));
                }
                cx.emit(WorkspaceEvent::Toast { message: format!("“{}” is in the sidebar", t.title), undo: None });
            }
            Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't show it: {e}"), undo: None }),
        }
        self.reload(cx);
        // Its messages become searchable like the rest of the import's.
        self.index_for_search(cx);
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn current_thread(&self) -> Option<&Thread> {
        match &self.route {
            Route::Thread(id) => self.thread(id),
            _ => None,
        }
    }

    /// The thread `scope` shows, if it shows one (the main window may be on a draft or settings).
    pub fn thread_id_in<'a>(&'a self, scope: &'a Scope) -> Option<&'a str> {
        scope_thread(scope, &self.route)
    }

    pub fn thread_in(&self, scope: &Scope) -> Option<&Thread> {
        self.thread_id_in(scope).and_then(|id| self.thread(id))
    }

    /// `scope` is composing a new thread (only the main window does).
    pub fn is_draft_in(&self, scope: &Scope) -> bool {
        scope_is_draft(scope, &self.route)
    }

    /// Working directory for `scope`: its thread's folder, or the main window's draft project.
    pub fn cwd_in(&self, scope: &Scope) -> Option<PathBuf> {
        match scope {
            Scope::Main => self.current_cwd(),
            Scope::Thread(id) => self.thread(id).and_then(|t| t.cwd.clone()),
        }
    }

    pub fn git_in(&self, scope: &Scope) -> Option<&GitInfo> {
        self.cwd_in(scope).and_then(|c| self.git_info.get(&c))
    }

    /// Where `id` is on screen: its own window, else the main window when it shows the thread.
    /// Its own window wins, so queued follow-ups go back to that window's composer.
    pub fn shown_in(&self, id: &str) -> Option<Scope> {
        shown_in(id, &self.route, self.thread_windows.contains_key(id), self.main_window.is_some())
    }

    /// The thread is on screen somewhere: in the main window or a window of its own.
    pub fn on_screen(&self, id: &str) -> bool {
        self.shown_in(id).is_some()
    }

    /// Whether the user is looking at `thread`: it's in the frontmost Trek window, or (when no
    /// Trek window is frontmost) in any of them.
    pub fn viewing(&self, thread: &str, active: Option<AnyWindowHandle>) -> bool {
        let in_main = self.main_window.is_some() && matches!(&self.route, Route::Thread(t) if t == thread);
        viewing(in_main, self.thread_windows.get(thread).copied(), self.main_window, active)
    }

    /// A window now shows `id` on its own.
    pub fn thread_window_opened(&mut self, id: &str, handle: AnyWindowHandle, cx: &mut Context<Self>) {
        self.thread_windows.insert(id.to_string(), handle);
        self.mutate_thread(id, cx, |t| t.last_seen_at = now_ms().max(t.updated_at));
        self.ensure_loaded(id, cx);
        if let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()) {
            self.refresh_git_at(cwd, cx);
        }
        if let Some(t) = self.thread(id).filter(|t| t.agent == AgentId::ClaudeCode) {
            let cwd = t.cwd.clone().unwrap_or_else(trek_core::paths::home);
            self.fetch_claude_commands(cwd, cx);
        }
        cx.notify();
    }

    /// Learn Claude Code's commands and skills in `cwd` (a thread window's folder; the status
    /// check covers the main window's), unless they're known. Sends no prompt.
    fn fetch_claude_commands(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let key = (AgentId::ClaudeCode.key(), cwd);
        let ready = self.agents.iter().any(|a| a.agent == AgentId::ClaudeCode && a.availability == Availability::Ready);
        if !ready || self.settings.disabled_agents.contains(&key.0) || self.agent_commands.contains_key(&key) {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        let dir = key.1.clone();
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_agents::claude_status(&dir).await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(Ok(st)) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                this.agent_commands.insert(key, st.commands);
                cx.notify();
            });
        });
        self.tasks.push(task);
    }

    pub fn thread_window_closed(&mut self, id: &str, handle: AnyWindowHandle, cx: &mut Context<Self>) {
        if forget_window(&mut self.thread_windows, id, handle) {
            cx.notify();
        }
    }

    /// The main window closed (thread windows may still be open; it reopens on demand). What it
    /// left here goes with it: a reopened window must neither scroll back to an old search hit
    /// nor keep the browser hidden for a palette or menu that closed with it.
    pub fn main_window_closed(&mut self, handle: AnyWindowHandle, cx: &mut Context<Self>) {
        if self.main_window == Some(handle) {
            self.main_window = None;
            self.reveal = None;
            self.overlay_open = false;
            cx.notify();
        }
    }

    /// Navigate the main window from a thread window, and bring it forward (reopening it if it
    /// was closed).
    pub fn show_in_main(&mut self, route: Route, cx: &mut Context<Self>) {
        self.navigate(route, cx);
        cx.emit(WorkspaceEvent::ActivateMain);
    }

    fn mutate_thread(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Thread)) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == id) {
            f(t);
            if let Err(e) = self.store.save_thread(t) {
                tracing::warn!("save thread: {e}");
            }
            cx.notify();
        }
    }

    pub fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.settings.save() {
            tracing::warn!("save settings: {e}");
        }
        if default_prefs_key(&self.settings) != self.applied_defaults {
            self.apply_default_prefs(cx);
        }
        // Another channel: whatever the old one found, downloaded or staged no longer applies. A
        // nightly staged before the user left Nightly must not install on quit.
        if self.updater.channel_changed(self.settings.updates.channel) {
            self.drop_update(cx);
            self.check_for_updates(true, cx);
        }
        cx.notify();
    }

    /// Copy the default agent, model, effort and hand-holding from settings into the draft
    /// composer, so the next new thread starts with them. Only fields that changed in settings
    /// since the last apply are copied; the draft's other picks (plan, fast) are kept.
    /// `save_settings` calls this on its own when the defaults change.
    pub fn apply_default_prefs(&mut self, cx: &mut Context<Self>) {
        let (agent, model, effort, hand) = default_prefs_key(&self.settings);
        let (old_agent, old_model, old_effort, old_hand) = std::mem::replace(&mut self.applied_defaults, (agent.clone(), model.clone(), effort, hand));
        let p = &mut self.draft_prefs;
        let agent_id = AgentId::from_key(&agent);
        if agent != old_agent && p.agent != agent_id {
            p.agent = agent_id;
            p.model = model.clone();
        }
        if model != old_model {
            p.model = model;
        }
        if effort != old_effort {
            p.effort = effort;
        }
        if hand != old_hand {
            p.hand_holding = hand;
        }
        cx.notify();
    }

    /// Open Trek's data folder (settings, database, backgrounds) in Finder.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn reveal_data_folder(&self, cx: &App) {
        cx.open_with_system(&trek_core::paths::data_dir());
    }

    /// Plain-text report for "Copy diagnostics": versions, detected agents, where settings live.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn diagnostics(&self) -> String {
        let sw = |flag: &str| {
            std::process::Command::new("/usr/bin/sw_vers")
                .arg(flag)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".into())
        };
        let mut out = vec![
            format!("Trek {}", env!("CARGO_PKG_VERSION")),
            format!("macOS {} ({}) · {}", sw("-productVersion"), sw("-buildVersion"), std::env::consts::ARCH),
            format!("Settings: {}", trek_core::paths::settings_file().display()),
            format!("Data: {}", trek_core::paths::data_dir().display()),
            String::new(),
            "Agents:".into(),
        ];
        if self.agents.is_empty() {
            out.push(if self.detecting { "  (detecting…)".into() } else { "  (none detected)".into() });
        }
        for a in &self.agents {
            let state = match a.availability {
                Availability::Ready => "ready",
                Availability::NotInstalled => "not installed",
                Availability::NeedsLogin => "needs login",
                Availability::Offline => "offline",
            };
            let disabled = if self.settings.disabled_agents.contains(&a.agent.key()) { ", disabled" } else { "" };
            let version = a.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            let path = a.path.as_ref().map(|p| format!(" · {}", p.display())).unwrap_or_default();
            out.push(format!("  {}{version}: {state}{disabled}{path}", a.name));
        }
        out.join("\n")
    }

    /// Threads grouped into sidebar sections, filtered by search and project.
    pub fn sections(&self) -> Vec<(Section, Vec<&Thread>)> {
        let now = now_ms();
        let q = self.search.trim().to_lowercase();
        let mut map: HashMap<Section, Vec<&Thread>> = HashMap::new();
        for t in &self.threads {
            if !q.is_empty() && !self.search_results.matches(&q, t) {
                continue;
            }
            if let Some(p) = &self.project_filter {
                if t.project_id.as_ref() != Some(p) {
                    continue;
                }
            }
            if let Some(s) = t.section(now) {
                map.entry(s).or_default().push(t);
            }
        }
        let mut out: Vec<(Section, Vec<&Thread>)> = map.into_iter().collect();
        out.sort_by_key(|(s, _)| *s);
        for (s, list) in &mut out {
            match s {
                Section::Inbox => list.sort_by_key(|t| t.inbox_rank()),
                Section::Pinned => list.sort_by_key(|t| t.pinned_at),
                _ => list.sort_by_key(|t| -t.updated_at),
            }
        }
        out
    }

    /// Threads waiting on the user (approval or failure), as the inbox shows them: a settled
    /// thread that asks again is back in the inbox, so it counts too. Archived ones don't.
    pub fn needs_you_count(&self) -> usize {
        self.threads.iter().filter(|t| t.needs_you() && t.archived_at.is_none()).count()
    }

    /// Follow-ups waiting for the running turn of `id` to finish (`FollowUp::Queue`).
    pub fn queued(&self, id: &str) -> usize {
        self.live.get(id).map_or(0, |l| l.queued.len())
    }

    /// Drop the follow-ups queued on `id` without sending them.
    #[allow(dead_code)] // for the settings / composer UI
    pub fn clear_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(live) = self.live.get_mut(id) {
            if !live.queued.is_empty() {
                live.queued.clear();
                live.revision += 1;
                cx.notify();
            }
        }
    }

    /// A thread is working with a live agent turn in this process. Threads left marked Working by
    /// an earlier run that quit mid-turn don't count.
    pub fn any_turn_running(&self) -> bool {
        self.threads
            .iter()
            .any(|t| t.run_state == RunState::Working && self.live.get(&t.id).is_some_and(|l| l.turn_started.is_some()))
    }

    /// Quitting now would cut agent work short: a turn under way, whether it's working or paused
    /// on an approval, a question or a plan; sub-agents still out; or a plan offered after its
    /// turn, waiting for an answer (it lives only in memory). Updates wait until there's none.
    pub fn work_in_flight(&self) -> bool {
        self.live
            .values()
            .any(|l| (l.commands.is_some() && (l.turn_started.is_some() || l.background > 0)) || l.permissions.iter().any(|p| p.after_turn))
    }

    // ---------- navigation ----------

    /// Projects worth offering in pickers: repos and folders the user added.
    pub fn workspace_projects(&self) -> Vec<&Project> {
        self.projects
            .iter()
            .filter(|p| p.is_workspace(&self.settings.user_projects) && !self.settings.hidden_projects.contains(&p.path.display().to_string()))
            .collect()
    }

    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if let Some(cwd) = self.current_cwd() {
            self.refresh_git_at(cwd, cx);
        }
    }

    pub fn refresh_git_at(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let c = cwd.clone();
            let info = cx.background_executor().spawn(async move { read_git_info(&c) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.git_info.get(&cwd) != Some(&info) {
                    this.git_info.insert(cwd, info);
                    cx.notify();
                }
            });
        });
        self.tasks.push(task);
    }

    /// `git switch <branch>` in `cwd`.
    pub fn switch_branch(&mut self, cwd: PathBuf, branch: String, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let dir = cwd.clone();
            let out = cx
                .background_executor()
                .spawn(async move { std::process::Command::new("git").args(["switch", &branch]).current_dir(&dir).output() })
                .await;
            let _ = this.update(cx, |this, cx| {
                match out {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).lines().find(|l| l.starts_with("error")).unwrap_or("git switch failed").to_string();
                        cx.emit(WorkspaceEvent::Toast { message: err, undo: None });
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("git: {e}"), undo: None }),
                }
                this.refresh_git_at(cwd, cx);
            });
        });
        self.tasks.push(task);
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Thread(id) = &route {
            let id = id.clone();
            self.mutate_thread(&id, cx, |t| t.last_seen_at = now_ms().max(t.updated_at));
            self.ensure_loaded(&id, cx);
        }
        if let Route::Draft { project: Some(p) } = &route {
            if self.route != route {
                self.apply_project_defaults(&p.clone());
            }
        }
        self.route = route;
        self.refresh_git(cx);
        cx.emit(WorkspaceEvent::FocusComposer);
        cx.notify();
    }

    /// Open a thread scrolled to one of its messages (a search result).
    pub fn open_thread_at(&mut self, id: &str, item: ItemRef, cx: &mut Context<Self>) {
        let seq = self.reveal.as_ref().map_or(1, |r| r.seq + 1);
        self.reveal = Some(Reveal { thread: id.to_string(), item, seq });
        self.navigate(Route::Thread(id.to_string()), cx);
    }

    // ---------- search ----------

    /// Set the sidebar's search text. Titles are matched as you type; the full-text search runs
    /// in the background and its threads join the list when the results arrive.
    pub fn set_search(&mut self, text: String, cx: &mut Context<Self>) {
        if text == self.search {
            return;
        }
        self.search = text;
        // Results for an unrelated text would list the wrong threads until the new ones arrive.
        if !self.search_results.relevant_to(self.search.trim()) {
            self.search_results = SearchResults::default();
        }
        self.run_search(cx);
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search.trim().to_string();
        if trek_core::store::fts_query(&query).is_none() {
            self.search_results = SearchResults::default();
            self._search_task = None;
            cx.notify();
            return;
        }
        let store = self.store.clone();
        self._search_task = Some(cx.spawn(async move |this, cx| {
            let q = query.clone();
            let hits = cx.background_executor().spawn(async move { store.search(&q, 500) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.search.trim() != query {
                    return;
                }
                this.search_results = match hits {
                    Ok(hits) => SearchResults::from_hits(query, hits),
                    Err(e) => {
                        tracing::warn!("search: {e}");
                        SearchResults::default()
                    }
                };
                cx.notify();
            });
        }));
    }

    /// The index took in new text: searches on screen run again.
    fn search_index_changed(&mut self, cx: &mut Context<Self>) {
        self.search_epoch += 1;
        if !self.search.trim().is_empty() {
            self.run_search(cx);
        }
        cx.notify();
    }

    /// Fill the search index in the background: rows waiting for it (saved before it existed, or
    /// appended in bulk), then imported threads' transcripts that are new or changed since the
    /// last run (small ones only; bigger ones are indexed when opened, see `ensure_loaded`). Runs
    /// at launch, after every import and after big saves; searches on screen refresh as it goes.
    fn index_for_search(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            self.index_again = true;
            return;
        }
        self.indexing = true;
        self.index_again = false;
        let store = self.store.clone();
        let task = cx.spawn(async move |this, cx| {
            let s = store.clone();
            if let Err(e) = cx.background_executor().spawn(async move { s.prune_search() }).await {
                tracing::warn!("search prune: {e}");
            }
            loop {
                let s = store.clone();
                let more = cx.background_executor().spawn(async move { index_step(&s) }).await;
                if this.update(cx, |this, cx| this.search_index_changed(cx)).is_err() {
                    return;
                }
                if !more {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.indexing = false;
                if this.index_again {
                    this.index_for_search(cx);
                }
            });
        });
        self.tasks.push(task);
    }

    // ---------- projects: preferences ----------

    pub fn project_prefs(&self, path: &std::path::Path) -> trek_core::settings::ProjectPrefs {
        self.settings.projects.get(&path.display().to_string()).cloned().unwrap_or_default()
    }

    /// `lucide:<name>` / `file:<path>` for a project folder, if the user chose one.
    pub fn project_icon(&self, path: &std::path::Path) -> Option<String> {
        self.settings.projects.get(&path.display().to_string()).and_then(|p| p.icon.clone())
    }

    /// Icon for the project a thread belongs to.
    pub fn thread_project_icon(&self, t: &Thread) -> Option<String> {
        let pid = t.project_id.as_ref()?;
        let p = self.projects.iter().find(|p| &p.id == pid)?;
        self.project_icon(&p.path)
    }

    pub fn update_project_prefs(&mut self, path: &std::path::Path, f: impl FnOnce(&mut trek_core::settings::ProjectPrefs), cx: &mut Context<Self>) {
        let key = path.display().to_string();
        let mut prefs = self.settings.projects.get(&key).cloned().unwrap_or_default();
        f(&mut prefs);
        if prefs.is_empty() {
            self.settings.projects.remove(&key);
        } else {
            self.settings.projects.insert(key, prefs);
        }
        self.save_settings(cx);
    }

    /// A new thread in `project` starts from Trek's defaults, overridden by the project's own.
    fn apply_project_defaults(&mut self, project: &std::path::Path) {
        let g = &self.settings.general;
        let prefs = self.project_prefs(project);
        let agent = prefs.agent.as_deref().map(AgentId::from_key).filter(|a| self.ready_agents().contains(a));
        // With no agent of its own, a project can still pick the model Trek's default agent uses
        // (Settings → Project offers that agent's models); one that agent doesn't have is ignored.
        let default_agent = AgentId::from_key(&g.default_agent);
        let model_for_default = prefs
            .model
            .clone()
            .filter(|m| prefs.agent.is_none() && self.models_for(&default_agent).iter().any(|i| crate::composer::same_model(m, &i.id)));
        let p = &mut self.draft_prefs;
        match agent {
            Some(a) => {
                p.model = prefs.model.clone();
                p.agent = a;
            }
            None => {
                p.agent = default_agent;
                p.model = model_for_default.or_else(|| g.default_model.clone());
            }
        }
        p.effort = prefs.effort.unwrap_or(g.default_effort);
        p.hand_holding = prefs.hand_holding.unwrap_or(g.hand_holding);
        p.worktree = prefs.run_in == trek_core::settings::RunIn::Worktree && project.join(".git").exists();
        if p.hand_holding == HandHolding::FullAccess && !self.settings.permissions.full_access_unlocked {
            p.hand_holding = HandHolding::Auto;
        }
    }

    /// Run one of a project's actions in a terminal tab in `dir`: the project's folder (wherever
    /// in it the thread on screen works), or the worktree of a thread that has one. From
    /// Settings, a new thread in the project comes up beside it.
    pub fn run_project_action(&mut self, dir: PathBuf, command: String, cx: &mut Context<Self>) {
        if command.trim().is_empty() {
            return;
        }
        if matches!(self.route, Route::Settings(_)) {
            self.navigate(Route::Draft { project: Some(dir.clone()) }, cx);
        }
        cx.emit(WorkspaceEvent::RunInTerminal { command, cwd: Some(dir) });
    }

    pub fn open_project_settings(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        let current = self.current_cwd().and_then(|c| self.projects.iter().find(|p| p.path == trek_core::store::project_root(&c)).map(|p| p.id.clone()));
        self.settings_project = project_id.or(current).or_else(|| self.workspace_projects().first().map(|p| p.id.clone()));
        self.navigate(Route::Settings(SettingsPage::Project), cx);
    }

    pub fn rename_project(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Err(e) = self.store.rename_project(id, name) {
            cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't rename: {e}"), undo: None });
        }
        self.reload(cx);
    }

    /// Take a project out of Trek: its threads are archived and it leaves every list. Files on
    /// disk and the agents' own history aren't touched; opening the folder again brings it back.
    pub fn remove_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter().find(|p| p.id == id).cloned() else { return };
        let key = project.path.display().to_string();
        let ids: Vec<String> = self.threads.iter().filter(|t| t.project_id.as_deref() == Some(id)).map(|t| t.id.clone()).collect();
        for tid in &ids {
            if let Some(tx) = self.live.get(tid).and_then(|l| l.commands.clone()) {
                let _ = tx.try_send(Command::Shutdown);
            }
            self.live.remove(tid);
        }
        let _ = self.store.archive_project_threads(id);
        self.settings.user_projects.retain(|p| *p != key);
        self.settings.projects.remove(&key);
        if !self.settings.hidden_projects.contains(&key) {
            self.settings.hidden_projects.push(key);
        }
        self.save_settings(cx);
        if self.project_filter.as_deref() == Some(id) {
            self.project_filter = None;
        }
        self.settings_project = None;
        self.reload(cx);
        let gone = match &self.route {
            Route::Thread(t) => ids.contains(t),
            Route::Draft { project: Some(p) } => trek_core::store::project_root(p) == project.path,
            _ => false,
        };
        if gone || matches!(self.route, Route::Settings(_)) {
            let next = self.workspace_projects().first().map(|p| p.path.clone());
            self.navigate(Route::Draft { project: next }, cx);
        }
        cx.emit(WorkspaceEvent::Toast { message: format!("Removed {} from Trek", project.name), undo: None });
    }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        let project = match &self.route {
            Route::Thread(id) => self.thread(id).and_then(|t| self.draft_folder(t)),
            Route::Draft { project } => project.clone(),
            _ => None,
        }
        .or_else(|| self.workspace_projects().first().map(|p| p.path.clone()));
        self.navigate(Route::Draft { project }, cx);
    }

    fn ensure_loaded(&mut self, id: &str, cx: &mut Context<Self>) {
        let thread = self.thread(id).cloned();
        let live = self.live.entry(id.to_string()).or_default();
        if live.loaded || live.loading {
            return;
        }
        live.checkpointed = self.store.checkpoints(id).unwrap_or_default().into_iter().map(|c| c.item_id).collect();
        let rows = self.store.items_with_ids(id).unwrap_or_default();
        if !rows.is_empty() {
            live.items = Transcript::stored(rows);
            live.loaded = true;
            live.revision += 1;
            return;
        }
        let Some(thread) = thread else { return };
        let (Some(native), true) = (thread.native_id.clone(), thread.source != ThreadSource::Trek) else {
            live.loaded = true;
            return;
        };
        live.loading = true;
        let id = id.to_string();
        let store = self.store.clone();
        let task = cx.spawn(async move |this, cx| {
            let (source, updated_at) = (thread.source, thread.updated_at);
            // History read from the agent's own files stays there until the thread is continued
            // here. Unless this version is in the search index already, a copy goes there too (the
            // only way big transcripts get indexed), made off the main thread like the read.
            let (s, tid) = (store.clone(), id.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    let items = trek_core::import::load_transcript(source, &native)?;
                    let to_index = (!s.imported_indexed(&tid, updated_at).unwrap_or(false)).then(|| items.clone());
                    anyhow::Ok((items, to_index))
                })
                .await;
            let (items, to_index) = match result {
                Ok((items, to_index)) => (items, to_index),
                Err(e) => (vec![Item::Error { text: format!("Couldn't load this thread: {e}") }], None),
            };
            let _ = this.update(cx, |this, cx| {
                let live = this.live.entry(id.clone()).or_default();
                live.loading = false;
                live.loaded = true;
                // Anything that arrived meanwhile (a warmed-up session's error, say) wasn't saved
                // (`persist_items` waits for the history) and goes after it.
                let n = items.len();
                live.items.prepend_unsaved(items);
                live.streaming = live.streaming.map(|i| i + n);
                live.reasoning = live.reasoning.map(|i| i + n);
                live.revision += 1;
                // Messages sent while it loaded go out now, after the history.
                this.send_queued(&id, cx);
                cx.notify();
            });
            if let Some(items) = to_index {
                let tid = id.clone();
                match cx.background_executor().spawn(async move { store.index_imported(&tid, Some(&items), updated_at) }).await {
                    Ok(true) => _ = this.update(cx, |this, cx| this.search_index_changed(cx)),
                    Ok(false) => {}
                    Err(e) => tracing::warn!("search index {id}: {e}"),
                }
            }
        });
        self.tasks.push(task);
    }

    // ---------- composer prefs ----------

    pub fn prefs(&self) -> Prefs {
        self.prefs_in(&Scope::Main)
    }

    pub fn prefs_in(&self, scope: &Scope) -> Prefs {
        match self.thread_in(scope) {
            Some(t) => Prefs {
                agent: t.agent.clone(),
                model: t.model.clone(),
                effort: t.effort,
                hand_holding: t.hand_holding,
                plan: self.live.get(&t.id).is_some_and(|l| l.plan),
                fast: self.live.get(&t.id).is_some_and(|l| l.fast),
                worktree: t.worktree.is_some(),
            },
            None => self.draft_prefs.clone(),
        }
    }

    pub fn set_prefs(&mut self, prefs: Prefs, cx: &mut Context<Self>) {
        self.set_prefs_in(&Scope::Main, prefs, cx);
    }

    pub fn set_prefs_in(&mut self, scope: &Scope, prefs: Prefs, cx: &mut Context<Self>) {
        match self.thread_id_in(scope).map(str::to_string) {
            Some(id) => {
                let before = self.thread(&id).cloned();
                self.mutate_thread(&id, cx, |t| {
                    t.model = prefs.model.clone();
                    t.effort = prefs.effort;
                    t.hand_holding = prefs.hand_holding;
                    // Switching agent on an existing thread forks it into a new session.
                    if t.agent != prefs.agent {
                        t.agent = prefs.agent.clone();
                        t.native_id = None;
                    }
                });
                let live = self.live.entry(id.clone()).or_default();
                // Another agent means another login and a fresh session: what the old one cost
                // and how it was billed say nothing about the new one, and the old agent's
                // questions and plans go with it.
                if before.as_ref().is_some_and(|b| b.agent != prefs.agent) {
                    live.billing = None;
                    live.cost_usd = 0.0;
                    live.cost_total = 0.0;
                    live.permissions.clear();
                    live.picks.clear();
                }
                let fast_changed = live.fast != prefs.fast;
                let plan_changed = live.plan != prefs.plan;
                live.plan = prefs.plan;
                live.fast = prefs.fast;
                if let (Some(before), Some(tx)) = (before, live.commands.clone()) {
                    // Claude reads effort, fast mode and plan at launch: restart idle sessions (they resume).
                    let relaunch = live.turn_started.is_none()
                        && (fast_changed || plan_changed || (prefs.agent == AgentId::ClaudeCode && before.effort != prefs.effort));
                    if before.agent != prefs.agent || relaunch {
                        let _ = tx.try_send(Command::Shutdown);
                        live.commands = None;
                    } else {
                        if before.hand_holding != prefs.hand_holding {
                            let _ = tx.try_send(Command::SetHandHolding(prefs.hand_holding));
                        }
                        if before.model != prefs.model || before.effort != prefs.effort {
                            if let Some(m) = prefs.model.clone() {
                                let _ = tx.try_send(Command::SetModel { model: m, effort: prefs.effort });
                            }
                        }
                    }
                }
                self.approve_covered_prompts(&id, prefs.hand_holding, cx);
            }
            None => self.draft_prefs = prefs,
        }
        cx.notify();
    }

    /// ⌘⇧A: move `scope` to the next hand-holding level (skipping Full access until it's allowed).
    pub fn cycle_hand_holding(&mut self, scope: &Scope, cx: &mut Context<Self>) {
        let mut p = self.prefs_in(scope);
        let all = HandHolding::ALL;
        let i = all.iter().position(|h| *h == p.hand_holding).unwrap_or(0);
        let mut next = all[(i + 1) % all.len()];
        if next == HandHolding::FullAccess && !self.settings.permissions.full_access_unlocked {
            next = all[0];
        }
        p.hand_holding = next;
        self.set_prefs_in(scope, p, cx);
    }

    /// Models for an agent: live data for local servers, catalog otherwise.
    pub fn models_for(&self, agent: &AgentId) -> Vec<ModelInfo> {
        if let AgentId::Direct(p) = agent {
            if let Some(found) = self.agents.iter().find(|a| &a.agent == agent) {
                if !found.models.is_empty() {
                    return found.models.iter().map(|m| ModelInfo { id: m.clone(), name: m.clone(), efforts: vec![], tier: 0, fast: None }).collect();
                }
            }
            return match p.as_str() {
                "anthropic" => catalog::default_models(&AgentId::ClaudeCode),
                "openai" => catalog::default_models(&AgentId::Codex),
                _ => catalog::default_models(agent),
            };
        }
        if *agent == AgentId::Codex && !self.codex_models.is_empty() {
            return self.codex_models.clone();
        }
        if let Some(Ok(info)) = self.acp_info.get(&agent.key()) {
            if !info.models.is_empty() {
                return info.models.clone();
            }
        }
        if *agent == AgentId::ClaudeCode {
            if let Some(st) = self.agent_status.get(&agent.key()).filter(|s| !s.models.is_empty()) {
                return st.models.clone();
            }
        }
        catalog::default_models(agent)
    }

    /// The window-wide backdrop image, when the user chose "Everywhere".
    pub fn backdrop(&self) -> Option<(String, f32)> {
        let a = &self.settings.appearance;
        (a.background_placement == trek_core::settings::BackgroundPlacement::Everywhere).then(|| a.background.clone().map(|b| (b, a.background_dim))).flatten()
    }

    /// Working directory for whatever is on screen: the thread's folder or the draft's project.
    pub fn current_cwd(&self) -> Option<PathBuf> {
        match &self.route {
            Route::Thread(id) => self.thread(id).and_then(|t| t.cwd.clone()),
            Route::Draft { project } => project.clone(),
            _ => None,
        }
    }

    /// Copy a user-picked image into Trek's data folder and use it as the background.
    pub fn set_background_image(&mut self, src: PathBuf, cx: &mut Context<Self>) {
        let dir = trek_core::paths::data_dir().join("backgrounds");
        let _ = std::fs::create_dir_all(&dir);
        let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "background".into());
        let dest = dir.join(name);
        match std::fs::copy(&src, &dest) {
            Ok(_) => {
                self.settings.appearance.background = Some(dest.display().to_string());
                self.save_settings(cx);
            }
            Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't use that image: {e}"), undo: None }),
        }
    }

    pub fn pick_background_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use as Background".into()) });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.set_background_image(p, cx));
                }
            }
        });
        self.tasks.push(task);
    }

    /// Start a side chat next to `parent` (or the current draft's project). Hidden from the sidebar.
    pub fn create_side_chat(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let cwd = self.current_cwd();
        let parent = match &self.route {
            Route::Thread(id) => id.clone(),
            _ => "draft".into(),
        };
        let p = self.prefs();
        // Same project as its thread, and the same folder (a worktree, maybe).
        let project = self.current_thread().and_then(|t| self.project_dir(t)).or_else(|| cwd.clone());
        let mut t = self.store.create_thread(project.as_deref(), p.agent, p.model, p.effort, HandHolding::Supervised).ok()?;
        t.cwd = cwd;
        t.title = "Side chat".into();
        t.side_of = Some(parent);
        let _ = self.store.save_thread(&t);
        let id = t.id.clone();
        self.threads.push(t);
        self.live.entry(id.clone()).or_default().loaded = true;
        cx.notify();
        Some(id)
    }

    /// Agents the user can pick right now.
    pub fn ready_agents(&self) -> Vec<AgentId> {
        let mut out: Vec<AgentId> = self
            .agents
            .iter()
            .filter(|a| a.availability == Availability::Ready)
            .filter(|a| match &a.agent {
                AgentId::ClaudeCode | AgentId::Codex => true,
                AgentId::Direct(_) => !a.models.is_empty(),
                // ACP agents are pickable once probed and signed in.
                other => self.acp_info.get(&other.key()).is_some_and(|i| i.as_ref().is_ok_and(|i| !i.needs_auth)),
            })
            .map(|a| a.agent.clone())
            .collect();
        for p in &self.settings.api_providers {
            let id = AgentId::Direct(p.clone());
            if !out.contains(&id) {
                out.push(id);
            }
        }
        if self.mock_agent {
            out.push(AgentId::Direct(catalog::MOCK_PROVIDER.into()));
        }
        out.retain(|a| !self.settings.disabled_agents.contains(&a.key()));
        if out.is_empty() {
            out.push(AgentId::ClaudeCode);
        }
        out
    }

    // ---------- sending ----------

    pub fn send(&mut self, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        if matches!(self.route, Route::Draft { .. }) {
            let mut words = text.split_whitespace();
            if matches!(words.next(), Some("/permissions" | "/access" | "/mode")) {
                let arg = words.next().map(str::to_string);
                let reply = self.permissions_command(None, arg.as_deref(), cx);
                cx.emit(WorkspaceEvent::Toast { message: reply.replace("**", "").replace('`', ""), undo: None });
                return;
            }
        }
        let id = match self.route.clone() {
            Route::Thread(id) => id,
            Route::Draft { project } => {
                let Some(cwd) = project else {
                    cx.emit(WorkspaceEvent::Toast { message: "Pick a project folder first.".into(), undo: None });
                    return;
                };
                let p = self.draft_prefs.clone();
                // A worktree of its own: its branch and folder are picked now, and it's made in
                // the background while the message waits. Other threads' worktrees are taken,
                // made yet or not.
                let taken: Vec<_> = self.threads.iter().filter_map(|t| t.worktree.clone()).collect();
                let planned = match p.worktree.then(|| trek_core::worktree::plan(&trek_core::worktree::worktrees_dir(), &cwd, &text, &taken)) {
                    Some(Err(e)) => {
                        cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't start a worktree: {e}"), undo: None });
                        cx.emit(WorkspaceEvent::InsertIntoComposer(text));
                        for image in images {
                            cx.emit(WorkspaceEvent::AttachImage(image));
                        }
                        return;
                    }
                    Some(Ok(wt)) => Some(wt),
                    None => None,
                };
                let mut thread = match self.store.create_thread(Some(&cwd), p.agent, p.model, p.effort, p.hand_holding) {
                    Ok(t) => t,
                    Err(e) => {
                        cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't create thread: {e}"), undo: None });
                        return;
                    }
                };
                thread.title = trek_core::import_title(&text);
                if let Some(wt) = &planned {
                    thread.cwd = Some(wt.path.clone());
                    thread.worktree = Some(wt.clone());
                }
                let _ = self.store.save_thread(&thread);
                let id = thread.id.clone();
                self.reload(cx);
                let live = self.live.entry(id.clone()).or_default();
                live.loaded = true;
                live.plan = p.plan;
                live.fast = p.fast;
                self.route = Route::Thread(id.clone());
                match planned {
                    Some(wt) => self.make_worktree(&id, cwd, wt, cx),
                    // Use the session that was started while the message was being typed, if it still fits.
                    None => {
                        let key = self.draft_key(&cwd);
                        match self.warm.take() {
                            Some((k, handle, _)) if k == key => self.attach(&id, handle, cx),
                            _ => {}
                        }
                    }
                }
                id
            }
            _ => return,
        };
        self.send_to(&id, text, images, cx);
    }

    /// Send from `scope`'s composer: the main window's (which may start a new thread) or a thread window's.
    pub fn send_in(&mut self, scope: &Scope, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        match scope {
            Scope::Main => self.send(text, images, cx),
            Scope::Thread(id) => {
                let before = self.route.clone();
                self.send_to(id, text, images, cx);
                // `/new` and friends open a draft in the main window.
                if self.route != before {
                    cx.emit(WorkspaceEvent::ActivateMain);
                }
            }
        }
    }

    /// Send a prompt to a specific thread (main view or a side chat).
    pub fn send_to(&mut self, id: &str, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        let id = id.to_string();
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        // The thread's history is still being read from the agent's files, or its worktree is being
        // made or is missing: hold the message until that's sorted (`ensure_loaded`,
        // `worktree_ready` and `run_in_project_folder` send it).
        if self.holds_messages(&id) {
            let live = self.live.entry(id).or_default();
            live.queued.push((text, images));
            live.revision += 1;
            cx.notify();
            return;
        }
        if let Some(reply) = self.run_builtin_command(&id, &text, cx) {
            if reply.is_empty() {
                return;
            }
            let live = self.live.entry(id.clone()).or_default();
            live.items.push(Item::User { text: text.clone(), images: vec![], at: Some(now_ms()), resume: None, aside: true });
            live.items.push(Item::Notice { text: reply });
            live.revision += 1;
            self.persist_items(&id, cx);
            cx.notify();
            return;
        }
        // The agent is waiting on a question: what the user types is their answer, in their own words.
        let question = self.live.get(&id).and_then(|l| l.permissions.first()).and_then(|p| match &p.prompt {
            Some(trek_agents::Prompt::Questions(q)) => Some((p.request_id.clone(), q.clone())),
            _ => None,
        });
        if let Some((request_id, questions)) = question {
            let picks = self.live.get(&id).map(|l| l.picks.clone()).unwrap_or_default();
            let picked = |i: usize| picks.get(&(request_id.clone(), i)).filter(|v| !v.is_empty()).map(|v| v.join(", "));
            let (answers, secret) = typed_answers(&questions, picked, &text);
            if let Some(live) = self.live.get_mut(&id) {
                // A secret (a token, a password) goes to the agent and nowhere else.
                live.items.push(if secret { Item::Notice { text: "Private answer sent".into() } } else { Item::User { text, images: vec![], at: Some(now_ms()), resume: None, aside: true } });
            }
            self.send_answers(&id, &request_id, answers, cx);
            self.persist_items(&id, cx);
            return;
        }
        // Queue mode: hold follow-ups until the running turn finishes (see `apply_events`).
        let running = self.live.get(&id).is_some_and(|l| l.turn_started.is_some() && l.commands.is_some());
        if running && self.settings.general.follow_up == FollowUp::Queue {
            let live = self.live.entry(id.clone()).or_default();
            live.queued.push((text, images));
            live.revision += 1;
            cx.notify();
            return;
        }
        let resume = self.resume_point(&id);
        // Where the session stands isn't known yet (an imported thread, or one an older Trek
        // kept): it's read from the agent's files before the message goes, or a rewind to it
        // couldn't take the agent's own session back.
        let find_point = match (&resume, self.thread(&id)) {
            (Some(ResumePoint { session, after: None }), Some(t)) if !running && t.reopen.is_none() && trek_agents::resumes_partway(&t.agent, t.model.as_deref()) => {
                Some((t.agent.clone(), session.clone()))
            }
            _ => None,
        };
        let cwd = self.thread(&id).and_then(|t| t.cwd.clone());
        self.ensure_session(&id, cx);
        let live = self.live.entry(id.clone()).or_default();
        live.drop_after_turn();
        let ix = live.items.push(Item::User { text: text.clone(), images: images.iter().map(|p| p.display().to_string()).collect(), at: Some(now_ms()), resume, aside: false });
        if let (Some((agent, session)), Some(item)) = (find_point, live.items.id_at(ix)) {
            live.git_jobs.push_back(GitJob::FindPoint { agent, session, item: item.to_string() });
        }
        live.streaming = None;
        live.reasoning = None;
        // A message sent while a turn runs steers that turn: its clock and its sub-agents go on.
        if !running {
            live.turn_started = Some(Instant::now());
            live.tasks.clear();
            // A new turn starts from a checkpoint of the files, taken before the agent has the
            // message (it's held until then): a rewind can put them back.
            if let (Some(cwd), Some(item), true) = (cwd.clone(), live.items.id_at(ix).map(str::to_string), in_repo(cwd.as_deref())) {
                live.git_jobs.push_back(GitJob::Checkpoint { cwd, item });
            }
        }
        live.last_active = Some(cx.background_executor().now());
        live.revision += 1;
        self.dispatch(&id, Command::Prompt { text, images });
        self.run_git(&id, cx);
        self.mutate_thread(&id, cx, |t| {
            t.run_state = RunState::Working;
            t.settled_at = None;
            t.snoozed_until = None;
            t.updated_at = now_ms();
            t.last_seen_at = t.updated_at;
        });
        self.persist_items(&id, cx);
    }

    /// Messages to `id` wait: its history is still being read, or its worktree is being made or
    /// has gone missing.
    fn holds_messages(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| l.loading || l.preparing) || self.thread(id).is_some_and(|t| t.worktree.as_ref().is_some_and(|w| w.is_missing()))
    }

    /// Send what was queued while nothing ran (a thread whose history was loading): the first
    /// message starts a turn, and the rest wait for it as queued follow-ups do.
    fn send_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        while !self.holds_messages(id) && self.live.get(id).is_some_and(|l| l.turn_started.is_none()) {
            let Some((text, images)) = self.live.get_mut(id).and_then(|l| (!l.queued.is_empty()).then(|| l.queued.remove(0))) else { break };
            self.send_to(id, text, images, cx);
        }
    }

    fn fast_tier(&self, agent: &AgentId, model: Option<&String>, fast_on: bool) -> Option<String> {
        if !fast_on {
            return None;
        }
        let models = self.models_for(agent);
        model.and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id))).and_then(|m| m.fast.clone())
    }

    fn ensure_session(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id).cloned() else { return };
        if self.live.get(id).is_some_and(|l| l.commands.is_some() || l.preparing) {
            return;
        }
        // No folder to run in until the worktree is back (or the thread moves to the project's).
        if thread.worktree.as_ref().is_some_and(|w| w.is_missing()) {
            return;
        }
        let (plan, fast_on) = self.live.get(id).map(|l| (l.plan, l.fast)).unwrap_or_default();
        // After a rewind or a fork the session picks up from part of one, or from a recap.
        let recap = || self.live.get(id).map(|l| trek_core::rewind::recap(&l.items)).filter(|r| !r.is_empty());
        let (resume, resume_at, fork, recap) = match thread.reopen.clone() {
            Some(Reopen::Native { session, at, fork }) => (Some(session), at, fork, recap()),
            Some(Reopen::Recap) => (None, None, false, recap()),
            None => (thread.native_id.clone(), None, false, None),
        };
        let handle = trek_agents::start(SessionConfig {
            agent: thread.agent.clone(),
            cwd: thread.cwd.clone().unwrap_or_else(trek_core::paths::home),
            model: thread.model.clone(),
            effort: thread.effort,
            hand_holding: thread.hand_holding,
            plan,
            resume,
            resume_at,
            fork,
            recap,
            fast: self.fast_tier(&thread.agent, thread.model.as_ref(), fast_on),
            mcp_servers: self.mcp_servers(),
        });
        self.attach(id, handle, cx);
    }

    /// Wire a running session to a thread: commands go out, events come back in batches.
    fn attach(&mut self, id: &str, handle: trek_agents::SessionHandle, cx: &mut Context<Self>) {
        let live = self.live.entry(id.to_string()).or_default();
        live.commands = Some(handle.commands);
        live.last_active = Some(cx.background_executor().now());
        // A new process says how it's billed again (the login may have changed since).
        live.billing = None;
        let events = handle.events;
        let id = id.to_string();
        live._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                // Batch everything already queued so a burst of tokens is one update.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.apply_events(&id, batch, cx)).is_err() {
                    break;
                }
                // Cap UI updates at ~60 Hz while streaming.
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
        }));
    }

    fn draft_key(&self, cwd: &std::path::Path) -> WarmKey {
        let p = &self.draft_prefs;
        (p.agent.clone(), cwd.to_path_buf(), p.model.clone(), p.effort, p.hand_holding, p.plan, p.fast)
    }

    /// Start the agent before the first message is sent (called when the user begins typing), so
    /// the process, its login check and its MCP servers are ready by the time they hit Return.
    /// Costs nothing with the provider: no request is made until a prompt is sent.
    pub fn warm_up(&mut self, cx: &mut Context<Self>) {
        match self.route.clone() {
            Route::Thread(id) => self.warm_thread(&id, cx),
            Route::Draft { project: Some(cwd) } => {
                // A thread in a worktree starts its agent there, once the worktree is made.
                if matches!(self.draft_prefs.agent, AgentId::Direct(_)) || self.draft_prefs.worktree {
                    return;
                }
                let key = self.draft_key(&cwd);
                if self.warm.as_ref().is_some_and(|(k, _, _)| *k == key) {
                    return;
                }
                let p = self.draft_prefs.clone();
                let handle = trek_agents::start(SessionConfig {
                    agent: p.agent.clone(),
                    cwd,
                    model: p.model.clone(),
                    effort: p.effort,
                    hand_holding: p.hand_holding,
                    plan: p.plan,
                    resume: None,
                    resume_at: None,
                    fork: false,
                    recap: None,
                    fast: self.fast_tier(&p.agent, p.model.as_ref(), p.fast),
                    mcp_servers: self.mcp_servers(),
                });
                // Replacing the old one drops its command channel, which ends that process.
                self.warm = Some((key, handle, cx.background_executor().now()));
            }
            _ => {}
        }
    }

    pub fn warm_up_in(&mut self, scope: &Scope, cx: &mut Context<Self>) {
        match scope {
            Scope::Main => self.warm_up(cx),
            Scope::Thread(id) => self.warm_thread(id, cx),
        }
    }

    /// Start an existing thread's session ahead of its next message (direct providers have no process to start).
    fn warm_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        let direct = self.thread(id).is_some_and(|t| matches!(t.agent, AgentId::Direct(_)));
        if !direct {
            self.ensure_session(id, cx);
        }
    }

    pub(crate) fn apply_events(&mut self, id: &str, events: Vec<AgentEvent>, cx: &mut Context<Self>) {
        let mut run_state: Option<RunState> = None;
        let mut native: Option<String> = None;
        // A session started (reported its id): whatever a rewind or fork asked of it is done.
        let mut started = false;
        // It's another session than the thread's: its points so far are unknown.
        let mut new_session = false;
        let current_native = self.thread(id).and_then(|t| t.native_id.clone());
        let mut diff: Option<(i64, i64)> = None;
        let mut finished = false;
        // The turn ended cleanly: queued follow-ups may go out. After a stop or failure they go back
        // to the composer instead, so the user can rethink them.
        let mut continue_queue = false;
        let mut notify_text: Option<String> = None;
        // The user stopped the turn: nothing to tell them.
        let mut interrupted = false;
        // The agent stopped to ask the user something.
        let mut asked = false;
        let mut commands: Option<Vec<SlashCommand>> = None;
        // Streamed text and tool calls change nothing but the transcript; mostly they only extend
        // the messages already streaming.
        let mut transcript_only = true;
        let mut appended = true;
        let mut turn_began: Option<i64>;
        {
            let live = self.live.entry(id.to_string()).or_default();
            live.last_active = Some(cx.background_executor().now());
            // When the turn ending here began, as wall time (`note_branch`).
            turn_began = live.turn_started.map(|t| now_ms() - t.elapsed().as_millis() as i64);
            for ev in events {
                // A point the session can be taken back to changes nothing on screen.
                let ev = match ev {
                    AgentEvent::Mark(m) => {
                        live.mark = Some(m);
                        continue;
                    }
                    ev => ev,
                };
                // Output with no turn open: the agent woke itself (a background sub-agent finished).
                if matches!(ev, AgentEvent::TextDelta(_) | AgentEvent::ReasoningDelta(_) | AgentEvent::ToolStarted { .. }) && live.turn_started.is_none() {
                    turn_began = Some(now_ms());
                    live.turn_started = Some(Instant::now());
                    run_state = Some(RunState::Working);
                    transcript_only = false;
                }
                transcript_only &= matches!(
                    ev,
                    AgentEvent::TextDelta(_) | AgentEvent::TextDone(_) | AgentEvent::ReasoningDelta(_) | AgentEvent::ToolStarted { .. } | AgentEvent::ToolFinished { .. }
                );
                // Text after a thought ends the thought, which then gets a row of its own.
                appended &= match ev {
                    AgentEvent::TextDelta(_) => live.streaming.is_some() && live.reasoning.is_none(),
                    AgentEvent::ReasoningDelta(_) => live.reasoning.is_some(),
                    _ => false,
                };
                match ev {
                    AgentEvent::Task { id: tid, description, activity, tool_uses, done } => {
                        let known = live.tasks.iter().position(|t| t.id == tid);
                        let ix = match (known, &description) {
                            (Some(ix), _) => Some(ix),
                            (None, Some(d)) => {
                                live.tasks.push(SubTask { id: tid.clone(), description: d.clone(), activity: String::new(), tool_uses: 0, done: None });
                                Some(live.tasks.len() - 1)
                            }
                            // Progress for something we never saw start (a shell command): ignore.
                            (None, None) => None,
                        };
                        if let Some(ix) = ix {
                            let task = &mut live.tasks[ix];
                            if let Some(a) = activity {
                                task.activity = a;
                            }
                            if let Some(n) = tool_uses {
                                task.tool_uses = n;
                            }
                            if done.is_some() {
                                task.done = done;
                            }
                            let status = match task.done {
                                None => ToolStatus::Running,
                                Some(true) => ToolStatus::Done,
                                Some(false) => ToolStatus::Failed,
                            };
                            if let Some(Item::Tool { status: st, .. }) = live.items.rfind_mut(|i| matches!(i, Item::Tool { id, .. } if *id == tid)) {
                                *st = status;
                            }
                        }
                    }
                    AgentEvent::Background(n) => live.background = n,
                    AgentEvent::Commands(c) => commands = Some(c),
                    AgentEvent::Notice(text) => {
                        live.streaming = None;
                        live.items.push(Item::Notice { text });
                    }
                    AgentEvent::PermissionResolved { request_id } => {
                        live.settle_prompt(&request_id);
                        if live.permissions.is_empty() {
                            run_state = Some(if live.turn_started.is_some() { RunState::Working } else { RunState::Idle });
                        }
                    }
                    AgentEvent::Started { native_id, .. } => {
                        started = true;
                        if !native_id.is_empty() {
                            if current_native.as_ref() != Some(&native_id) {
                                new_session = true;
                                live.mark = None;
                            }
                            native = Some(native_id);
                        }
                    }
                    AgentEvent::TextDelta(t) => {
                        let ix = match live.streaming {
                            Some(ix) => ix,
                            None => {
                                let ix = live.items.push(Item::Assistant { text: String::new() });
                                live.streaming = Some(ix);
                                ix
                            }
                        };
                        if let Some(Item::Assistant { text }) = live.items.get_mut(ix) {
                            text.push_str(&t);
                        }
                        live.reasoning = None;
                    }
                    AgentEvent::TextDone(t) => {
                        match live.streaming.take() {
                            Some(ix) => {
                                if let Some(Item::Assistant { text }) = live.items.get_mut(ix) {
                                    *text = t;
                                }
                            }
                            None if !t.trim().is_empty() => {
                                live.items.push(Item::Assistant { text: t });
                            }
                            None => {}
                        }
                        live.reasoning = None;
                    }
                    AgentEvent::ReasoningDelta(t) => {
                        let ix = match live.reasoning {
                            Some(ix) => ix,
                            None => {
                                let ix = live.items.push(Item::Reasoning { text: String::new() });
                                live.reasoning = Some(ix);
                                ix
                            }
                        };
                        if let Some(Item::Reasoning { text }) = live.items.get_mut(ix) {
                            text.push_str(&t);
                        }
                    }
                    AgentEvent::ToolStarted { id: tid, title, detail } => {
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Tool { id: tid, title, detail, output: String::new(), status: ToolStatus::Running });
                    }
                    AgentEvent::ToolFinished { id: tid, output, ok } => {
                        if let Some(Item::Tool { output: o, status, .. }) = live.items.rfind_mut(|i| matches!(i, Item::Tool { id, .. } if *id == tid)) {
                            *o = output;
                            // A background sub-agent's tool call returns at once; it's done when its task is.
                            if !live.tasks.iter().any(|t| t.id == tid && t.done.is_none()) {
                                *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                            }
                        }
                    }
                    AgentEvent::PermissionRequest { request_id, title, detail, prompt } => {
                        notify_text = Some(match &prompt {
                            Some(trek_agents::Prompt::Questions(_)) => "Needs your approval: a question for you".to_string(),
                            Some(trek_agents::Prompt::Plan(_)) => "Needs your approval: a plan to review".to_string(),
                            None => format!("Needs your approval: {title}"),
                        });
                        let after_turn = live.turn_started.is_none();
                        live.permissions.push(PendingPermission { request_id, title, detail, prompt, after_turn });
                        run_state = Some(RunState::NeedsYou);
                        asked = true;
                    }
                    AgentEvent::DiffStat { additions, deletions } => diff = Some((additions, deletions)),
                    AgentEvent::Context { used, window } => live.context = Some((used, window)),
                    AgentEvent::Billing(b) => live.billing = Some(b),
                    AgentEvent::TurnComplete { cost_usd, error } => {
                        // Models that hide their reasoning leave empty "Thought" rows behind. Removing
                        // them shifts positions; the rows' ids keep saves and views lined up.
                        live.items.retain(|i| !matches!(i, Item::Reasoning { text } if text.trim().is_empty()));
                        live.streaming = None;
                        live.reasoning = None;
                        live.permissions.clear();
                        if let Some(total) = cost_usd {
                            live.cost_usd += cost_added(&mut live.cost_total, total);
                        }
                        // Background sub-agents are still out: the agent will pick the turn back up
                        // when they report, so the thread keeps working.
                        if error.is_none() && live.background > 0 {
                            continue;
                        }
                        let took = live.turn_started.take().map(|t| t.elapsed().as_secs() as u32).unwrap_or(0);
                        live.close_turn(error.is_none());
                        if error.is_none() && matches!(live.items.last(), Some(Item::Assistant { .. })) {
                            live.items.push(Item::TurnEnd { at: now_ms(), took_secs: took });
                        }
                        if let Some(e) = error {
                            if e != "Interrupted" {
                                live.items.push(Item::Error { text: e });
                                run_state = Some(RunState::Failed);
                                continue_queue = false;
                            } else {
                                live.items.push(Item::Notice { text: "Interrupted".into() });
                                run_state = Some(RunState::Idle);
                                continue_queue = false;
                                interrupted = true;
                            }
                        } else {
                            run_state = Some(RunState::Idle);
                            continue_queue = true;
                        }
                        finished = true;
                    }
                    AgentEvent::Mark(_) => {}
                    AgentEvent::Error(e) => {
                        live.close_turn(false);
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Error { text: e });
                        live.turn_started = None;
                        run_state = Some(RunState::Failed);
                        finished = true;
                        continue_queue = false;
                    }
                    AgentEvent::Exited => {
                        continue_queue = false;
                        live.commands = None;
                        // Nobody is left to answer what the agent was asking.
                        live.permissions.retain(|p| p.after_turn);
                        live.picks.retain(|(rid, _), _| live.permissions.iter().any(|p| p.request_id == *rid));
                        // The process ended mid-turn without saying why: the turn failed, and
                        // ends here like any other (saved, queued follow-ups handed back, an alert).
                        if live.turn_started.take().is_some() {
                            live.close_turn(false);
                            live.streaming = None;
                            live.reasoning = None;
                            live.items.push(Item::Error { text: "The agent stopped unexpectedly.".into() });
                            run_state.get_or_insert(RunState::Failed);
                            finished = true;
                        }
                    }
                }
            }
            live.revision += 1;
        }
        let mark = self.live.get(id).and_then(|l| l.mark.clone());
        if let (Some(c), Some(t)) = (commands, self.thread(id)) {
            let key = (t.agent.key(), t.cwd.clone().unwrap_or_else(trek_core::paths::home));
            self.agent_commands.insert(key, c);
        }
        let viewing = self.on_screen(id);
        // Streaming text changes nothing on the thread row: skip the database write (this runs
        // up to 60 times a second) unless something actually changed.
        let changed = finished
            || diff.is_some()
            || self.thread(id).is_some_and(|t| {
                (started && t.reopen.is_some()) || native.as_ref().is_some_and(|n| t.native_id.as_ref() != Some(n)) || run_state.is_some_and(|s| t.run_state != s)
            });
        // The agent moved to another session than the thread's (it couldn't cut its own back, or
        // started over): the one left behind isn't this thread's any more, and mustn't come
        // back as an imported thread of its own.
        let left = native.as_ref().and_then(|n| current_native.clone().filter(|old| old != n));
        if let Some(old) = &left {
            if let Err(e) = self.store.retire_session(old, id) {
                tracing::warn!("retire session {old}: {e}");
            }
        }
        if changed {
            self.mutate_thread(id, cx, |t| {
                if let Some(n) = native {
                    t.native_id = Some(n);
                }
                if left.is_some() {
                    t.become_trek_thread();
                }
                if started {
                    t.reopen = None;
                }
                // The latest point to take the session back to, kept for the next launch.
                match mark {
                    Some(m) => t.native_at = Some(m),
                    None if new_session => t.native_at = None,
                    None => {}
                }
                if let Some(s) = run_state {
                    t.run_state = s;
                }
                if let Some((a, d)) = diff {
                    t.additions = a;
                    t.deletions = d;
                }
                if finished {
                    t.updated_at = now_ms();
                    if viewing {
                        t.last_seen_at = t.updated_at;
                    }
                }
            });
        }
        // Mid-turn, what changed is saved a second later (only changed rows are written), so a
        // crash loses a second at most. A turn that stops to ask is saved as it stops: the user
        // may well leave it waiting, or quit.
        if !finished {
            if asked {
                self.persist_items(id, cx);
            } else {
                self.persist_soon(id, cx);
            }
        }
        if finished {
            self.turns_finished += 1;
            self.refresh_git(cx);
            if let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()).filter(|c| Some(c) != self.current_cwd().as_ref()) {
                self.refresh_git_at(cwd, cx);
            }
            self.persist_items(id, cx);
            self.search_index_changed(cx);
            self.note_branch(id, turn_began, cx);
            let next = if continue_queue {
                self.live.get_mut(id).and_then(|l| (!l.queued.is_empty()).then(|| l.queued.remove(0)))
            } else {
                None
            };
            if !continue_queue && viewing {
                self.restore_queued(id, cx);
            }
            if let Some((text, images)) = next {
                // The thread keeps going with the user's queued follow-up: not "finished" yet.
                self.send_to(id, text, images, cx);
            } else {
                self.maybe_auto_title(id, cx);
                if let Some(t) = self.thread(id).filter(|_| !interrupted) {
                    let verb = if t.run_state == RunState::Failed { "Failed" } else { "Finished" };
                    notify_text.get_or_insert(format!("{verb}: {}", t.title));
                }
                self.maybe_restart_for_update(cx);
            }
        }
        if let Some(message) = notify_text {
            cx.emit(WorkspaceEvent::Attention { message, thread: id.to_string() });
        }
        if transcript_only {
            // Only the transcript views redraw; the sidebar, title bar and composer would
            // otherwise redraw with every batch, up to 60 times a second while text streams.
            cx.emit(WorkspaceEvent::Transcript { id: id.to_string(), appended });
        } else {
            cx.notify();
        }
    }

    /// Put queued follow-ups back into the composer (the thread must be on screen).
    fn restore_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(queued) = self.live.get_mut(id).map(|l| std::mem::take(&mut l.queued)) else { return };
        if queued.is_empty() {
            return;
        }
        let text = queued.iter().map(|(t, _)| t.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        let images = queued.into_iter().flat_map(|(_, images)| images).collect();
        cx.emit(WorkspaceEvent::RestoreQueued { thread: id.to_string(), text, images });
    }

    /// Save `id`'s transcript a second from now, unless a save is on its way already.
    fn persist_soon(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id).filter(|l| l._save_soon.is_none() && l.items.is_dirty()) else { return };
        let id = id.to_string();
        live._save_soon = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(l) = this.live.get_mut(&id) {
                    l._save_soon = None;
                }
                this.persist_items(&id, cx);
            });
        }));
    }

    /// Save every open transcript's unsaved changes (Trek is quitting).
    fn persist_all(&mut self) {
        for (id, live) in self.live.iter_mut().filter(|(_, l)| !l.loading) {
            if let Err(e) = self.store.save_transcript(id, &mut live.items) {
                tracing::warn!("save transcript {id}: {e}");
            }
        }
    }

    /// Save what changed in a thread's transcript: new rows, edited rows (streaming text, tool
    /// status and output) and removed rows. Cheap enough to run mid-turn. A big first save (an
    /// imported thread's history) is indexed for search in the background. Nothing is saved while
    /// the history is still being read: it has to be written first (`ensure_loaded`).
    fn persist_items(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id).filter(|l| !l.loading) else { return };
        match self.store.save_transcript(id, &mut live.items) {
            Ok(true) => self.index_for_search(cx),
            Ok(false) => {}
            Err(e) => tracing::warn!("save transcript: {e}"),
        }
    }

    pub fn respond(&mut self, id: &str, request_id: &str, decision: Decision, cx: &mut Context<Self>) {
        let mut still_waiting = false;
        let mut running = false;
        if let Some(live) = self.live.get_mut(id) {
            live.settle_prompt(request_id);
            still_waiting = !live.permissions.is_empty();
            running = live.turn_started.is_some() && live.commands.is_some();
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::Respond { request_id: request_id.to_string(), decision });
            }
            live.revision += 1;
        }
        // A prompt can outlive its turn (Codex offers its plan once the turn is over): answering
        // it then leaves the thread idle unless it starts new work.
        if !still_waiting {
            let state = if running { RunState::Working } else { RunState::Idle };
            self.mutate_thread(id, cx, |t| t.run_state = state);
        }
        cx.notify();
    }

    /// Answer the agent's questions from the question card: `(question, chosen labels)`. The
    /// answers go in the transcript as the user's message, as typed ones do (`send_to`); secret
    /// ones never do.
    pub fn answer(&mut self, id: &str, request_id: &str, answers: Vec<(String, String)>, cx: &mut Context<Self>) {
        let questions = self.live.get(id).and_then(|l| l.permissions.iter().find(|p| p.request_id == request_id)).and_then(|p| match &p.prompt {
            Some(trek_agents::Prompt::Questions(q)) => Some(q.clone()),
            _ => None,
        });
        if let (Some(questions), Some(live)) = (questions, self.live.get_mut(id)) {
            let (text, secret) = answers_text(&questions, &answers);
            if !text.is_empty() {
                live.items.push(Item::User { text, images: vec![], at: Some(now_ms()), resume: None, aside: true });
            }
            if secret {
                live.items.push(Item::Notice { text: "Private answer sent".into() });
            }
        }
        self.send_answers(id, request_id, answers, cx);
        self.persist_items(id, cx);
    }

    fn send_answers(&mut self, id: &str, request_id: &str, answers: Vec<(String, String)>, cx: &mut Context<Self>) {
        let mut still_waiting = false;
        if let Some(live) = self.live.get_mut(id) {
            live.settle_prompt(request_id);
            still_waiting = !live.permissions.is_empty();
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::Answer { request_id: request_id.to_string(), answers });
            }
            live.revision += 1;
        }
        if !still_waiting {
            self.mutate_thread(id, cx, |t| t.run_state = RunState::Working);
        }
        cx.notify();
    }

    /// Approve the agent's plan: it leaves plan mode and starts the work.
    pub fn approve_plan(&mut self, id: &str, request_id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id) else { return };
        live.plan = false;
        // Offered after its turn ended (Codex), the plan's approval starts a new turn. If the
        // session that offered it has gone (restarted, or quit), a new one, out of plan mode,
        // takes the approval: the agent still has the plan in the thread's history.
        if live.permissions.iter().any(|p| p.request_id == request_id && p.after_turn) {
            self.ensure_session(id, cx);
            if let Some(live) = self.live.get_mut(id).filter(|l| l.commands.is_some()) {
                live.turn_started = Some(Instant::now());
                live.tasks.clear();
            }
        }
        self.respond(id, request_id, Decision::Allow, cx);
    }

    pub fn interrupt(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id) else { return };
        // A stop never waits behind git work. If the message is still held for its checkpoint,
        // the agent never gets it: the turn ends here.
        if live.held.iter().any(|c| matches!(c, Command::Prompt { .. })) {
            live.held.retain(|c| !matches!(c, Command::Prompt { .. }));
            self.apply_events(id, vec![AgentEvent::TurnComplete { cost_usd: None, error: Some("Interrupted".into()) }], cx);
            return;
        }
        if let Some(tx) = &live.commands {
            let _ = tx.try_send(Command::Interrupt);
        }
        cx.notify();
    }

    /// Send `cmd` to `id`'s agent once the git work queued before it is done.
    fn dispatch(&mut self, id: &str, cmd: Command) {
        let Some(live) = self.live.get_mut(id) else { return };
        if live.git_busy || !live.git_jobs.is_empty() {
            live.held.push(cmd);
        } else if let Some(tx) = &live.commands {
            let _ = tx.try_send(cmd);
        }
    }

    /// Do `id`'s git work one job at a time, off the main thread; what was held for it goes to
    /// the agent once it's all done.
    fn run_git(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id).filter(|l| !l.git_busy) else { return };
        let Some(job) = live.git_jobs.pop_front() else {
            let held = std::mem::take(&mut live.held);
            if let Some(tx) = &live.commands {
                for cmd in held {
                    let _ = tx.try_send(cmd);
                }
            }
            return;
        };
        live.git_busy = true;
        let (store, thread, run, guard) = (self.store.clone(), id.to_string(), job.clone(), live.git_guard.clone());
        live._git = Some(cx.spawn(async move |this, cx| {
            let t = thread.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _held = guard.lock.lock().unwrap_or_else(|e| e.into_inner());
                    if guard.gone.load(std::sync::atomic::Ordering::SeqCst) {
                        return Ok(GitDone::Nothing);
                    }
                    git_job(&store, &t, run)
                })
                .await;
            let _ = this.update(cx, |this, cx| this.git_done(&thread, job, result, cx));
        }));
    }

    fn git_done(&mut self, id: &str, job: GitJob, result: anyhow::Result<GitDone>, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id) else { return };
        live.git_busy = false;
        match (job, result) {
            // Taking one may have pruned the oldest.
            (GitJob::Checkpoint { .. }, Ok(GitDone::Taken)) => live.checkpointed = self.store.checkpoints(id).unwrap_or_default().into_iter().map(|c| c.item_id).collect(),
            (GitJob::Checkpoint { item, .. }, Err(e)) => {
                tracing::warn!("checkpoint of {id}: {e:#}");
                // Said once per thread: a repo where it fails tends to fail every time.
                let first = live.checkpoint_failed.is_empty();
                live.checkpoint_failed.insert(item, format!("{e:#}"));
                live.revision += 1;
                if first {
                    cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't checkpoint the files: {e:#}"), undo: None });
                }
            }
            (GitJob::FindPoint { session, item, .. }, Ok(GitDone::Point(Some(at)))) => {
                if let Some(pos) = live.items.position(&item) {
                    if let Some(Item::User { resume: Some(ResumePoint { session: s, after }), .. }) = live.items.get_mut(pos) {
                        if *s == session && after.is_none() {
                            *after = Some(at.clone());
                        }
                    }
                }
                live.mark.get_or_insert(at);
                self.persist_items(id, cx);
            }
            (GitJob::Restore { repo, .. }, result) => {
                let restored = result.as_ref().ok().and_then(|d| if let GitDone::Restored(r) = d { Some(r) } else { None });
                let failed = match &result {
                    Err(e) => Some(format!("Couldn't restore the files: {e:#}")),
                    Ok(_) => restored.and_then(|r| r.failed.first().map(|(path, why)| (r.failed.len(), path, why))).map(|(n, path, why)| {
                        let what = if path.is_empty() { why.clone() } else { format!("{path}: {why}") };
                        let more = if n > 1 { format!(" and {} more", n - 1) } else { String::new() };
                        format!("Couldn't restore {what}{more}")
                    }),
                };
                // Restoring went wrong: the checkpoints it would have dropped stay, so the
                // files can still be had from them.
                if failed.is_some() {
                    live.git_jobs.retain(|j| !matches!(j, GitJob::Forget { .. }));
                }
                let n = restored.map_or(0, |r| r.restored());
                let message = match (n, failed) {
                    (0, Some(f)) => Some(f),
                    (n, Some(f)) => Some(format!("Restored {n} file{} · {f}", if n == 1 { "" } else { "s" })),
                    (0, None) => None,
                    (n, None) => Some(format!("Restored {n} file{}", if n == 1 { "" } else { "s" })),
                };
                let undo = restored.and_then(|r| r.undo.clone()).map(|sha| UndoAction::Unrestore { thread: id.to_string(), repo: repo.clone(), sha });
                if let Some(message) = message {
                    cx.emit(WorkspaceEvent::Toast { message, undo });
                }
                self.refresh_git_at(repo, cx);
                if let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()) {
                    self.refresh_git_at(cwd, cx);
                }
            }
            (GitJob::Forget { items, .. }, Ok(_)) => {
                for i in &items {
                    live.checkpointed.remove(i);
                }
            }
            (GitJob::Link { checkpoints, .. }, Ok(_)) => live.checkpointed.extend(checkpoints.into_iter().map(|(item, _)| item)),
            (_, Err(e)) => tracing::warn!("git work for {id}: {e:#}"),
            _ => {}
        }
        self.run_git(id, cx);
    }

    /// Where the agent's session will stand when the next message reaches it.
    fn resume_point(&self, id: &str) -> Option<ResumePoint> {
        let t = self.thread(id)?;
        match &t.reopen {
            Some(r) => r.point(),
            None => {
                let after = self.live.get(id).and_then(|l| l.mark.clone()).or_else(|| t.native_at.clone());
                t.native_id.clone().map(|session| ResumePoint { session, after })
            }
        }
    }

    /// A turn is under way on `id` (working, waiting on the user, or with sub-agents still out).
    pub fn turn_running(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| l.turn_started.is_some() || l.background > 0)
    }

    /// Take `id` back to just before message `item`: it and everything after it leave the
    /// transcript, the agent's next session doesn't know them either (`Reopen`), and with
    /// `restore` the files go back to the checkpoint taken as it was sent. Returns the message
    /// (text and images) for the composer; `None` when nothing was done (a turn is running).
    pub fn rewind(&mut self, id: &str, item: &str, restore: bool, cx: &mut Context<Self>) -> Option<(String, Vec<PathBuf>)> {
        if self.turn_running(id) {
            cx.emit(WorkspaceEvent::Toast { message: "Stop the running turn first.".into(), undo: None });
            return None;
        }
        let thread = self.thread(id)?.clone();
        let reopen = self.rewind_plan(id, item);
        let checkpoints = self.store.checkpoints(id).unwrap_or_default();
        // A missing worktree's files can't be put back. The refs of its checkpoints are in the
        // repository the project folder shares with it, so they're dropped from there.
        let shared = worktree_missing(&thread).then(|| self.project_dir(&thread)).flatten();
        let restore = restore && !worktree_missing(&thread);
        let live = self.live.get_mut(id).filter(|l| l.loaded && !l.loading)?;
        let pos = live.items.position(item)?;
        let Item::User { text, images, aside: false, .. } = live.items[pos].clone() else { return None };
        let removed: HashSet<String> = live.items.ids()[pos..].iter().cloned().collect();
        // The session goes; the next message starts one that knows only what's kept.
        if let Some(tx) = live.commands.take() {
            let _ = tx.try_send(Command::Shutdown);
        }
        live._events = None;
        live.items.truncate(pos);
        live.streaming = None;
        live.reasoning = None;
        live.permissions.clear();
        live.picks.clear();
        live.tasks.clear();
        live.background = 0;
        live.mark = match &reopen {
            Some(Reopen::Native { at, .. }) => at.clone(),
            _ => None,
        };
        if reopen == Some(Reopen::Recap) {
            live.items.push(Item::Notice { text: recap_notice(&thread.agent) });
        }
        live.revision += 1;
        if restore {
            if let Some(c) = checkpoints.iter().find(|c| c.item_id == item) {
                live.git_jobs.push_back(GitJob::Restore { repo: c.repo.clone(), sha: c.sha.clone() });
            }
        }
        // The checkpoints of messages that left the transcript go too.
        let mut gone: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for c in checkpoints.into_iter().filter(|c| removed.contains(&c.item_id)) {
            gone.entry(shared.clone().unwrap_or(c.repo)).or_default().push(c.item_id);
        }
        live.git_jobs.extend(gone.into_iter().map(|(repo, items)| GitJob::Forget { repo, items }));
        let mark = live.mark.clone();
        self.persist_items(id, cx);
        let same_session = matches!(reopen, Some(Reopen::Native { fork: false, .. }));
        if let Some(old) = thread.native_id.as_deref().filter(|_| !same_session) {
            if let Err(e) = self.store.retire_session(old, id) {
                tracing::warn!("retire session {old}: {e}");
            }
        }
        self.mutate_thread(id, cx, |t| {
            t.reopen = reopen;
            t.native_at = mark;
            if !same_session {
                t.native_id = None;
                // The conversation goes on in a session Trek starts: the thread is Trek's own
                // now, and imports no longer speak for it.
                t.become_trek_thread();
            }
            t.run_state = RunState::Idle;
            t.updated_at = now_ms();
            t.last_seen_at = t.updated_at;
        });
        self.run_git(id, cx);
        Some((text, images.into_iter().map(PathBuf::from).collect()))
    }

    /// Whether `id` can be rewound to just before message `item` now (see `rewind`).
    pub fn can_rewind(&self, id: &str, item: &str) -> bool {
        let live = self.live.get(id).filter(|l| l.loaded && !l.loading);
        !self.turn_running(id) && live.and_then(|l| l.items.position(item).map(|p| matches!(l.items[p], Item::User { aside: false, .. }))).unwrap_or(false)
    }

    /// Why message `item` of `id` has no file checkpoint to go back to now; `None` when it has one.
    pub fn no_checkpoint(&self, id: &str, item: &str) -> Option<NoCheckpoint> {
        let live = self.live.get(id)?;
        let thread = self.thread(id);
        NoCheckpoint::now(live, live.items.position(item)?, in_repo(thread.and_then(|t| t.cwd.as_deref())), thread.is_some_and(worktree_missing))
    }

    /// The checkpoint the files can be put back to as they were when message `item` of `id` was
    /// sent. None while the thread's worktree is missing: there's nowhere to put them back, so a
    /// rewind then takes back only the conversation.
    pub fn restorable_checkpoint(&self, id: &str, item: &str) -> Option<trek_core::store::Checkpoint> {
        if self.thread(id).is_none_or(worktree_missing) {
            return None;
        }
        self.store.checkpoint(id, item).ok().flatten()
    }

    /// How the agent would pick up `id` if it were rewound to just before message `item`.
    pub fn rewind_plan(&self, id: &str, item: &str) -> Option<Reopen> {
        let thread = self.thread(id)?;
        let live = self.live.get(id)?;
        let pos = live.items.position(item)?;
        let Item::User { resume, .. } = &live.items[pos] else { return None };
        let native = trek_agents::resumes_partway(&thread.agent, thread.model.as_deref());
        trek_core::rewind::reopen_before(&live.items[..pos], resume.as_ref(), thread.native_id.as_deref(), native)
    }

    /// The message that started the turn a `TurnEnd` (by item id) closes.
    pub fn turn_start_item(&self, id: &str, end: &str) -> Option<String> {
        let live = self.live.get(id)?;
        let start = trek_core::rewind::turn_start(&live.items, live.items.position(end)?)?;
        live.items.id_at(start).map(str::to_string)
    }

    /// Take back the turn a `TurnEnd` closes (see `rewind`).
    pub fn undo_turn(&mut self, id: &str, end: &str, restore: bool, cx: &mut Context<Self>) -> Option<(String, Vec<PathBuf>)> {
        let start = self.turn_start_item(id, end)?;
        self.rewind(id, &start, restore, cx)
    }

    /// Take back the turn a `TurnEnd` closes and send its message again, with `model` if given.
    pub fn retry(&mut self, id: &str, end: &str, model: Option<String>, restore: bool, cx: &mut Context<Self>) {
        let Some((text, images)) = self.undo_turn(id, end, restore, cx) else { return };
        if let Some(m) = model {
            self.mutate_thread(id, cx, |t| t.model = Some(m));
        }
        self.send_to(id, text, images, cx);
    }

    /// Send an edited message in place of message `item`: the conversation is taken back to just
    /// before it (see `rewind`), then the new text goes out. False when nothing was done.
    pub fn edit_and_resend(&mut self, id: &str, item: &str, text: String, images: Vec<PathBuf>, restore: bool, cx: &mut Context<Self>) -> bool {
        if self.rewind(id, item, restore, cx).is_none() {
            return false;
        }
        self.send_to(id, text, images, cx);
        true
    }

    /// Branch `id` into a new thread in the same project, with the same agent and model, holding
    /// the conversation up to `at`; the agent's session is forked too (or, for agents that can't,
    /// the new one gets a recap). The original is left as it is. The fork opens in the main
    /// window. Returns its id.
    pub fn fork_thread(&mut self, id: &str, at: ForkAt, scope: &Scope, cx: &mut Context<Self>) -> Option<String> {
        if at == ForkAt::End && self.turn_running(id) {
            cx.emit(WorkspaceEvent::Toast { message: "Wait for the turn to finish, or fork from an earlier message.".into(), undo: None });
            return None;
        }
        self.ensure_loaded(id, cx);
        if self.live.get(id).is_some_and(|l| l.loading) {
            // History still being read from the agent's files: fork once it's in.
            let (id, scope) = (id.to_string(), scope.clone());
            let task = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                    let Ok(loading) = this.read_with(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.loading)) else { return };
                    if !loading {
                        break;
                    }
                }
                let _ = this.update(cx, |this, cx| this.fork_thread(&id, at, &scope, cx));
            });
            self.tasks.push(task);
            return None;
        }
        let thread = self.thread(id)?.clone();
        let native = trek_agents::resumes_partway(&thread.agent, thread.model.as_deref());
        let live = self.live.get(id)?;
        let items = &live.items;
        let (cut, point, message) = match &at {
            ForkAt::Before(item) => {
                let pos = items.position(item)?;
                let Item::User { text, images, resume, .. } = &items[pos] else { return None };
                (pos, Some(resume.clone()), Some((text.clone(), images.iter().map(PathBuf::from).collect::<Vec<_>>())))
            }
            ForkAt::After(end) => {
                let pos = items.position(end)? + 1;
                let next = items[pos..].iter().find_map(|i| if let Item::User { resume, .. } = i { Some(resume.clone()) } else { None });
                (pos, next, None)
            }
            ForkAt::End => (items.len(), None, None),
        };
        let kept: Vec<Item> = items[..cut].to_vec();
        let kept_ids: Vec<String> = items.ids()[..cut].to_vec();
        let reopen = match point {
            // Where the session stood as the first message left out was sent.
            Some(point) => trek_core::rewind::reopen_before(&kept, point.as_ref(), None, native),
            // The conversation as it stands: the whole session.
            None => whole_session(&thread, live.mark.clone(), &kept, native),
        };
        // Made in the thread's project, as a new thread in a worktree is (`send`).
        let folder = if thread.worktree.is_some() { self.project_dir(&thread) } else { thread.cwd.clone() };
        let mut fork = match self.store.create_thread(folder.as_deref(), thread.agent.clone(), thread.model.clone(), thread.effort, thread.hand_holding) {
            Ok(t) => t,
            Err(e) => {
                cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't fork: {e}"), undo: None });
                return None;
            }
        };
        fork.title = format!("{} (fork)", thread.title);
        // A thread in a worktree forks into the same one: its files are there, and so is the
        // agent's session (agents keep them per folder). The two share it from then on.
        fork.cwd = thread.cwd.clone();
        fork.worktree = thread.worktree.clone();
        fork.native_at = match &reopen {
            Some(Reopen::Native { at, .. }) => at.clone(),
            _ => None,
        };
        fork.reopen = reopen.clone();
        let _ = self.store.save_thread(&fork);
        let mut transcript = Transcript::unsaved(kept);
        let new_ids: HashMap<&str, &str> = kept_ids.iter().map(String::as_str).zip(transcript.ids().iter().map(String::as_str)).collect();
        // Its messages keep their checkpoints: the fork can take its files back too.
        let mut links: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
        for c in self.store.checkpoints(id).unwrap_or_default() {
            if let Some(new) = new_ids.get(c.item_id.as_str()) {
                links.entry(c.repo).or_default().push((new.to_string(), c.sha));
            }
        }
        if reopen == Some(Reopen::Recap) {
            transcript.push(Item::Notice { text: recap_notice(&thread.agent) });
        }
        match self.store.save_transcript(&fork.id, &mut transcript) {
            // A long conversation's copy is indexed in the background.
            Ok(true) => self.index_for_search(cx),
            Ok(false) => {}
            Err(e) => tracing::warn!("save fork: {e}"),
        }
        let fork_id = fork.id.clone();
        let live = self.live.entry(fork_id.clone()).or_default();
        live.items = transcript;
        live.loaded = true;
        live.mark = fork.native_at.clone();
        live.git_jobs.extend(links.into_iter().map(|(repo, checkpoints)| GitJob::Link { repo, checkpoints }));
        self.reload(cx);
        match scope {
            Scope::Main => self.navigate(Route::Thread(fork_id.clone()), cx),
            Scope::Thread(_) => self.show_in_main(Route::Thread(fork_id.clone()), cx),
        }
        if let Some((text, images)) = message {
            cx.emit(WorkspaceEvent::ComposeIn { scope: Scope::Main, thread: fork_id.clone(), text, images, edit: None });
        }
        self.run_git(&fork_id, cx);
        Some(fork_id)
    }

    // ---------- inbox lifecycle ----------

    /// Out of the inbox. A failure settled stays Failed but no longer needs the user
    /// (`Thread::needs_you`); undone, it's back in the inbox as it was.
    pub fn settle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| {
            t.settled_at = Some(now_ms());
            t.pinned_at = None;
            t.snoozed_until = None;
            t.last_seen_at = t.updated_at.max(t.last_seen_at);
            // Dealt with: a merge of its branch later has nothing left to settle.
            t.branch = None;
        });
        cx.emit(WorkspaceEvent::Toast { message: "Settled".into(), undo: Some(UndoAction::Unsettle(id.into())) });
    }

    /// Back to the inbox, and counted as looked at now: auto-settle (which waits from the last
    /// look) or a merge of its branch would otherwise send an old thread straight back. Its last
    /// activity stays as it was, and an unread thread stays unread.
    pub fn unsettle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| {
            t.settled_at = None;
            t.branch = None;
            if !t.is_unseen() {
                t.last_seen_at = now_ms().max(t.last_seen_at);
            }
        });
    }

    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.pinned_at = if t.pinned_at.is_some() { None } else { Some(now_ms()) });
    }

    /// Out of the inbox until then; when it wakes it's back in the inbox, settled or not before.
    pub fn snooze(&mut self, id: &str, hours: i64, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| {
            t.snoozed_until = Some(now_ms() + hours * 3_600_000);
            t.settled_at = None;
            t.branch = None;
        });
        cx.emit(WorkspaceEvent::Toast { message: format!("Snoozed for {hours} h"), undo: None });
    }

    /// Wake it now, as if its snooze had just run out: auto-settle gives it the full wait again.
    pub fn unsnooze(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.snoozed_until = Some(now_ms()));
    }

    pub fn mark_unread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.last_seen_at = t.updated_at - 1);
    }

    pub fn archive(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.archived_at = Some(now_ms()));
        if self.route == Route::Thread(id.into()) {
            self.new_thread(cx);
        }
        self.threads.retain(|t| t.archived_at.is_none());
        cx.emit(WorkspaceEvent::Toast { message: "Archived".into(), undo: Some(UndoAction::Unarchive(id.into())) });
    }

    pub fn rename(&mut self, id: &str, title: String, cx: &mut Context<Self>) {
        let title = title.trim().to_string();
        if !title.is_empty() {
            self.mutate_thread(id, cx, |t| t.title = title);
        }
    }

    /// Snooze until 9:00 on the morning `days` from now (see `time::morning`).
    pub fn snooze_until_morning(&mut self, id: &str, days: i64, cx: &mut Context<Self>) {
        let Some(at) = crate::time::morning(&chrono::Local::now(), days) else { return };
        self.mutate_thread(id, cx, |t| {
            t.snoozed_until = Some(at.timestamp_millis());
            t.settled_at = None;
            t.branch = None;
        });
        cx.emit(WorkspaceEvent::Toast { message: format!("Snoozed until {}", at.format("%a %-I:%M %p")), undo: None });
    }

    pub fn set_never_settle(&mut self, id: &str, never: bool, cx: &mut Context<Self>) {
        self.mutate_thread(id, cx, |t| t.never_settle = never);
    }

    /// Delete a thread from Trek. A thread imported from another agent stays in that agent's own
    /// history, so here it's archived and its copy of the transcript dropped (it won't come back).
    pub fn delete_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id).cloned() else { return };
        if let Some(tx) = self.live.get(id).and_then(|l| l.commands.clone()) {
            let _ = tx.try_send(Command::Shutdown);
        }
        // Its file checkpoints go with it, and its side chats' (they go too): in the repos they
        // were taken in, and the one its folder is in (a checkpoint may be being taken now).
        let mut threads = vec![thread.clone()];
        if thread.source == ThreadSource::Trek {
            threads.extend(self.threads.iter().filter(|t| t.side_of.as_deref() == Some(id)).cloned());
        }
        let checkpoints = self.store.checkpoints(id).unwrap_or_default();
        let mut refs: HashSet<(String, PathBuf)> = HashSet::new();
        let mut guards = vec![];
        for t in &threads {
            refs.extend(self.store.checkpoints(&t.id).unwrap_or_default().into_iter().map(|c| (t.id.clone(), c.repo)));
            if let Some(cwd) = t.cwd.as_deref().filter(|c| in_repo(Some(c))) {
                refs.insert((t.id.clone(), cwd.to_path_buf()));
            }
            if let Some(live) = self.live.remove(&t.id) {
                live.git_guard.gone.store(true, std::sync::atomic::Ordering::SeqCst);
                guards.push(live.git_guard.clone());
            }
        }
        if !refs.is_empty() {
            let store = self.store.clone();
            cx.background_executor()
                .spawn(async move {
                    // After the git work under way: what it took goes too.
                    let _held: Vec<_> = guards.iter().map(|g| g.lock.lock().unwrap_or_else(|e| e.into_inner())).collect();
                    for (thread, repo) in refs {
                        if let Some(Err(e)) = trek_core::checkpoint::Repo::find(&repo).map(|r| r.delete_all(&thread)) {
                            tracing::warn!("drop checkpoints of {thread}: {e:#}");
                        }
                        let items: Vec<String> = store.checkpoints(&thread).unwrap_or_default().into_iter().map(|c| c.item_id).collect();
                        if let Err(e) = store.delete_checkpoints(&thread, &items) {
                            tracing::warn!("drop checkpoints of {thread}: {e:#}");
                        }
                    }
                })
                .detach();
        }
        let result = if thread.source == ThreadSource::Trek {
            self.store.delete_thread(id)
        } else {
            let items: Vec<String> = checkpoints.into_iter().map(|c| c.item_id).collect();
            self.store
                .delete_checkpoints(id, &items)
                .and_then(|_| self.store.clear_items(id))
                .and_then(|_| self.store.update_thread(id, |t| t.archived_at = Some(now_ms())).map(|_| ()))
        };
        if let Err(e) = result {
            cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't delete: {e}"), undo: None });
        }
        let was_open = self.route == Route::Thread(id.into());
        self.reload(cx);
        if was_open {
            self.new_thread(cx);
        }
    }

    /// The conversation as Markdown: your messages with the images you attached, the agent's
    /// answers, and turns that failed. Thinking, tool calls and sub-agents are left out.
    pub fn transcript_markdown(&self, id: &str) -> String {
        let items = match self.live.get(id).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(id).unwrap_or_default(),
        };
        let title = self.thread(id).map(|t| t.title.clone()).unwrap_or_default();
        let mut out = format!("# {title}\n");
        for item in items {
            match item {
                Item::User { text, images, .. } => {
                    out.push_str("\n## You\n");
                    if !text.trim().is_empty() {
                        out.push_str(&format!("\n{}\n", text.trim()));
                    }
                    for path in images {
                        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        // Angle brackets keep paths with spaces in one link.
                        out.push_str(&format!("\n![{name}](<{path}>)\n"));
                    }
                }
                Item::Assistant { text } if !text.trim().is_empty() => out.push_str(&format!("\n{}\n", text.trim())),
                Item::Error { text } => out.push_str(&format!("\n> **Error:** {}\n", text.trim().replace('\n', "\n> "))),
                _ => {}
            }
        }
        out
    }

    fn title_inputs(&self, id: &str) -> Option<(String, String)> {
        let items = match self.live.get(id).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(id).unwrap_or_default(),
        };
        let request = items.iter().find_map(|i| match i {
            Item::User { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })?;
        let reply = items.iter().find_map(|i| match i {
            Item::Assistant { text } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        });
        Some((request, reply.unwrap_or_default()))
    }

    /// Ask a small model for a short title (through the user's Claude Code login).
    pub fn regenerate_title(&mut self, id: &str, announce: bool, cx: &mut Context<Self>) {
        let Some((request, reply)) = self.title_inputs(id) else { return };
        // The mock agent names its own threads: it makes no model calls, titles included.
        if self.thread(id).is_some_and(|t| matches!(&t.agent, AgentId::Direct(p) if p == catalog::MOCK_PROVIDER)) {
            self.rename(id, trek_agents::mock::title(&request), cx);
            return;
        }
        let claude = self.agents.iter().any(|a| a.agent == AgentId::ClaudeCode && a.availability == Availability::Ready);
        if !claude {
            if announce {
                self.rename(id, trek_core::import_title(&request), cx);
                cx.emit(WorkspaceEvent::Toast { message: "Titles are written with Claude Code, which isn't available. Used the first message instead.".into(), undo: None });
            }
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_agents::generate_title(&request, &reply).await.map_err(|e| format!("{e:#}"))).await;
        });
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| match result {
                Ok(title) => this.rename(&id, title, cx),
                Err(e) if announce => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't write a title: {e}"), undo: None }),
                Err(e) => tracing::warn!("auto title: {e}"),
            });
        });
        self.tasks.push(task);
    }

    /// After a new Trek thread's first answer, replace the truncated first message with a real title.
    fn maybe_auto_title(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.settings.general.auto_title {
            return;
        }
        let Some(t) = self.thread(id) else { return };
        if t.source != ThreadSource::Trek || t.side_of.is_some() {
            return;
        }
        let Some(live) = self.live.get(id) else { return };
        let Some(first) = live.items.iter().find_map(|i| if let Item::User { text, .. } = i { Some(text) } else { None }) else { return };
        // Only on the first turn (answers to the agent's questions on the way are messages too),
        // and only if the title is still the automatic one.
        let turns = live.items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count();
        if turns > 1 || t.title != trek_core::import_title(first) || first.starts_with('/') {
            return;
        }
        self.regenerate_title(id, false, cx);
    }

    pub fn undo(&mut self, action: UndoAction, cx: &mut Context<Self>) {
        match action {
            UndoAction::Unsettle(id) => self.unsettle(&id, cx),
            UndoAction::Unarchive(id) => {
                let _ = self.store.update_thread(&id, |t| t.archived_at = None);
                self.reload(cx);
            }
            // Not for a thread deleted since: its refs are gone.
            UndoAction::Unrestore { thread, repo, sha } if self.thread(&thread).is_some() => {
                self.live.entry(thread.clone()).or_default().git_jobs.push_back(GitJob::Restore { repo, sha });
                self.run_git(&thread, cx);
            }
            UndoAction::Unrestore { .. } => {}
            UndoAction::CancelRestart => {
                self.restart_countdown = None;
                if let UpdateStatus::RestartPending { version, staged } = self.updater.status.clone() {
                    self.updater.status = UpdateStatus::Ready { version, staged };
                    cx.notify();
                }
            }
        }
    }

    /// Auto-settle and snooze wake-ups, once a minute.
    fn start_housekeeping(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            let alive = this.update(cx, |this, cx| {
                let now = now_ms();
                this.tidy_inbox(now, cx);
                // Agent processes idle for a while are shut down (150–250 MB each); they resume on the next message.
                this.reap_idle_sessions(cx.background_executor().now());
                if now - this.status_fetched_at > 5 * 60_000 {
                    this.refresh_usage(cx);
                }
                this.maybe_check_for_updates(cx);
                this.maybe_restart_for_update(cx);
                // No redraw otherwise: the views that show times keep their own clocks.
            });
            if alive.is_err() {
                break;
            }
        });
        self.tasks.push(task);
    }

    /// Settle threads left alone for the configured days and ones whose branch merged. Snoozes
    /// that have ended need nothing but a redraw: `Thread::section` puts them back in the inbox
    /// as of `now`.
    pub(crate) fn tidy_inbox(&mut self, now: i64, cx: &mut Context<Self>) {
        let since = std::mem::replace(&mut self.tidied_at, now);
        if self.threads.iter().any(|t| t.snoozed_until.is_some_and(|u| u > since && u <= now)) {
            cx.notify();
        }
        let days = self.settings.inbox.auto_settle_days;
        let due: Vec<String> = self.threads.iter().filter(|t| t.should_auto_settle(now, days)).map(|t| t.id.clone()).collect();
        for id in due {
            self.mutate_thread(&id, cx, |t| {
                t.settled_at = Some(now);
                t.branch = None;
            });
        }
        self.settle_merged(cx);
    }

    /// After a turn that began at `began` (ms), remember the branch the thread's folder is on if
    /// it holds unmerged commits and was committed to during the turn, so the thread can settle
    /// once that branch is merged (`settle_merged`). A thread that only read or talked on a
    /// branch isn't tied to it.
    fn note_branch(&mut self, id: &str, began: Option<i64>, cx: &mut Context<Self>) {
        let Some(began) = began.filter(|_| self.settings.inbox.auto_settle_on_merge) else { return };
        let Some(cwd) = self.thread(id).filter(|t| t.source == ThreadSource::Trek).and_then(|t| t.cwd.clone()) else { return };
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            // Commit times are in whole seconds.
            let since = began.div_euclid(1000);
            let Some(branch) = cx.background_executor().spawn(async move { trek_core::git::branch_committed_since(&cwd, since) }).await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.thread(&id).is_some_and(|t| t.branch.as_ref() != Some(&branch)) {
                    this.mutate_thread(&id, cx, |t| t.branch = Some(branch));
                }
            });
        })
        .detach();
    }

    /// Settle idle threads whose branch has been merged (`inbox.auto_settle_on_merge`). Checked
    /// with the rest of the housekeeping, off the main thread, one pass at a time.
    fn settle_merged(&mut self, cx: &mut Context<Self>) {
        if !self.settings.inbox.auto_settle_on_merge || self.checking_merges {
            return;
        }
        let candidates: Vec<(String, PathBuf, String)> = self
            .threads
            .iter()
            .filter(|t| t.source == ThreadSource::Trek && t.settled_at.is_none())
            .filter_map(|t| Some((t.id.clone(), t.cwd.clone()?, t.branch.clone()?)))
            .collect();
        if candidates.is_empty() {
            return;
        }
        self.checking_merges = true;
        cx.spawn(async move |this, cx| {
            use trek_core::git::BranchState;
            let checked: Vec<(String, String, BranchState)> = cx
                .background_executor()
                .spawn(async move { candidates.into_iter().map(|(id, cwd, branch)| (id, branch.clone(), trek_core::git::branch_state(&cwd, &branch))).collect() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checking_merges = false;
                let now = now_ms();
                for (id, branch, state) in checked {
                    let Some(t) = this.thread(&id).filter(|t| t.branch.as_ref() == Some(&branch) && t.settled_at.is_none()) else { continue };
                    // Merged, or gone: either way there's nothing left to watch for.
                    let settle = state == BranchState::Merged && settles_on_merge(t, now);
                    if state != BranchState::Unmerged {
                        this.mutate_thread(&id, cx, |t| {
                            t.branch = None;
                            if settle {
                                t.settled_at = Some(now);
                            }
                        });
                    }
                }
            });
        })
        .detach();
    }

    /// Shut down agent processes idle for 15 minutes (150–250 MB each) that nothing waits on,
    /// and the pre-warmed draft session after 10. A thread's next message starts its session
    /// again, resuming the agent's conversation. `now` is the executor's clock (a test's own).
    pub(crate) fn reap_idle_sessions(&mut self, now: Instant) {
        let idle_for = |at: Option<Instant>, limit: u64| at.is_none_or(|t| now.saturating_duration_since(t) > Duration::from_secs(limit * 60));
        let shown: Vec<String> = self.live.keys().filter(|id| self.on_screen(id)).cloned().collect();
        for (id, live) in self.live.iter_mut() {
            let quiet = live.turn_started.is_none() && live.background == 0 && live.permissions.is_empty();
            if quiet && live.commands.is_some() && idle_for(live.last_active, 15) && !shown.contains(id) {
                if let Some(tx) = live.commands.take() {
                    let _ = tx.try_send(Command::Shutdown);
                }
            }
        }
        if self.warm.as_ref().is_some_and(|(_, _, at)| idle_for(Some(*at), 10)) {
            self.warm = None;
        }
    }

    // ---------- projects ----------

    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.add_project(path, cx));
                }
            }
        });
        self.tasks.push(task);
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.store.ensure_project(&path) {
            Ok(p) => {
                let key = p.path.display().to_string();
                let was_hidden = self.settings.hidden_projects.contains(&key);
                self.settings.hidden_projects.retain(|h| *h != key);
                if !self.settings.user_projects.contains(&key) || was_hidden {
                    if !self.settings.user_projects.contains(&key) {
                        self.settings.user_projects.push(key);
                    }
                    let _ = self.settings.save();
                }
                self.reload(cx);
                self.navigate(Route::Draft { project: Some(p.path) }, cx);
            }
            Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't open folder: {e}"), undo: None }),
        }
    }

    /// Clone `owner/repo` (or a URL) with the GitHub CLI, falling back to git.
    pub fn clone_repo(&mut self, spec: String, cx: &mut Context<Self>) {
        let spec = spec.trim().to_string();
        if spec.is_empty() {
            return;
        }
        let name = spec.trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or("repo").to_string();
        let dest = trek_core::paths::home().join("Developer").join(&name);
        cx.emit(WorkspaceEvent::Toast { message: format!("Cloning {spec}…"), undo: None });
        let task = cx.spawn(async move |this, cx| {
            let dest2 = dest.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _ = std::fs::create_dir_all(dest2.parent().unwrap());
                    let path = trek_core::detect::login_path();
                    let gh = trek_core::detect::which("gh");
                    let status = if let (Some(gh), false) = (gh, spec.contains("://")) {
                        std::process::Command::new(gh).args(["repo", "clone", &spec]).arg(&dest2).env("PATH", path).output()
                    } else {
                        let url = if spec.contains("://") || spec.starts_with("git@") { spec.clone() } else { format!("https://github.com/{spec}.git") };
                        std::process::Command::new("git").args(["clone", &url]).arg(&dest2).env("PATH", path).output()
                    };
                    match status {
                        Ok(o) if o.status.success() => Ok(()),
                        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.add_project(dest, cx),
                Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Clone failed: {e}"), undo: None }),
            });
        });
        self.tasks.push(task);
    }

    // ---------- discovery ----------

    pub fn detect_agents(&mut self, cx: &mut Context<Self>) {
        if self.detecting {
            return;
        }
        self.detecting = true;
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::detect::detect_all().await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(agents) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.agents = agents;
                    this.detecting = false;
                    if this.agents.iter().any(|a| a.agent == AgentId::Codex && a.availability == Availability::Ready) {
                        this.fetch_codex_models(cx);
                    }
                    this.status_fetched_at = 0;
                    this.refresh_usage(cx);
                    this.probe_acp_agents(cx);
                    // Default to an agent that's actually installed.
                    let ready = this.ready_agents();
                    if !ready.contains(&this.draft_prefs.agent) {
                        if let Some(a) = ready.first() {
                            this.draft_prefs.agent = a.clone();
                            this.draft_prefs.model = None;
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.tasks.push(task);
    }

    /// Re-read account, plan, usage limits and commands from the installed vendor CLIs. Free:
    /// no prompt is sent. Throttled to once every 30 seconds.
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_loading || now_ms() - self.status_fetched_at < 30_000 {
            return;
        }
        let ready = |id: AgentId| self.agents.iter().any(|a| a.agent == id && a.availability == Availability::Ready) && !self.settings.disabled_agents.contains(&id.key());
        let (claude, codex) = (ready(AgentId::ClaudeCode), ready(AgentId::Codex));
        if !claude && !codex {
            return;
        }
        self.usage_loading = true;
        let cwd = self.current_cwd().unwrap_or_else(trek_core::paths::home);
        let (tx, rx) = async_channel::bounded(1);
        let folder = cwd.clone();
        trek_core::runtime().spawn(async move {
            let (a, b) = tokio::join!(
                async { if claude { Some(trek_agents::claude_status(&cwd).await) } else { None } },
                async { if codex { Some(trek_agents::codex_status(&cwd).await) } else { None } },
            );
            let _ = tx.send([(AgentId::ClaudeCode, a), (AgentId::Codex, b)]).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(results) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                for (agent, res) in results {
                    match res {
                        Some(Ok(st)) => {
                            if agent == AgentId::Codex && !st.models.is_empty() {
                                this.codex_models = st.models.clone();
                            }
                            // Its commands include the folder's own (project commands, skills).
                            this.agent_commands.insert((agent.key(), folder.clone()), st.commands.clone());
                            this.agent_status.insert(agent.key(), st);
                        }
                        Some(Err(e)) => {
                            let st = this.agent_status.entry(agent.key()).or_default();
                            st.error = Some(e.to_string());
                        }
                        None => {}
                    }
                }
                this.usage_loading = false;
                this.status_fetched_at = now_ms();
                cx.notify();
            });
        });
        self.tasks.push(task);
        cx.notify();
    }

    /// Ask each installed ACP agent for its models and login state. Answered from what the agent
    /// reported last time; only a new install or version (or a missing sign-in) is asked afresh.
    pub fn probe_acp_agents(&mut self, cx: &mut Context<Self>) {
        self.probe_acp(None, cx);
    }

    /// Probe one ACP agent (by key) that isn't known yet, e.g. one just turned back on.
    pub fn probe_acp_agent(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.acp_info.contains_key(key) {
            self.probe_acp(Some(key), cx);
        }
    }

    fn probe_acp(&mut self, only: Option<&str>, cx: &mut Context<Self>) {
        // Probing starts the agent and opens a session in its history: not for agents the user turned off.
        let ids: Vec<String> = self
            .agents
            .iter()
            .filter(|a| a.availability == Availability::Ready && matches!(a.agent, AgentId::OpenCode | AgentId::Droid | AgentId::Acp(_)))
            .map(|a| a.agent.key())
            .filter(|k| !self.settings.disabled_agents.contains(k) && only.is_none_or(|o| o == k))
            .collect();
        for id in ids {
            let (tx, rx) = async_channel::bounded(1);
            let probe_id = id.strip_prefix("acp:").unwrap_or(&id).to_string();
            trek_core::runtime().spawn(async move {
                let r = trek_agents::acp_probe(&probe_id).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(r).await;
            });
            let task = cx.spawn(async move |this, cx| {
                if let Ok(r) = rx.recv().await {
                    let _ = this.update(cx, |this, cx| {
                        this.acp_info.insert(id, r);
                        cx.notify();
                    });
                }
            });
            self.tasks.push(task);
        }
    }

    /// Slash commands for an agent in `scope`'s folder: Trek's own first, then the agent's
    /// commands and skills (project ones are the folder's own).
    pub fn slash_commands(&self, scope: &Scope, agent: &AgentId) -> Vec<SlashCommand> {
        let mut out: Vec<SlashCommand> = BUILTIN_COMMANDS
            .iter()
            .map(|(n, d)| SlashCommand { name: n.to_string(), description: d.to_string(), kind: trek_agents::CommandKind::Command })
            .collect();
        let cwd = self.cwd_in(scope).unwrap_or_else(trek_core::paths::home);
        // What the agent offered in this folder; the last status check (made for the main
        // window's folder) only until then.
        let commands = match self.agent_commands.get(&(agent.key(), cwd)) {
            Some(offered) => offered.as_slice(),
            None => self.agent_status.get(&agent.key()).map_or(&[][..], |st| st.commands.as_slice()),
        };
        for c in commands {
            if !out.iter().any(|o| o.name == c.name) {
                out.push(c.clone());
            }
        }
        out
    }

    /// Commands Trek answers itself instead of sending to the agent.
    /// Prompts already on screen when the user raises the level: approve the ones the new level
    /// would never have asked about (everything for Full access, file edits for Auto-accept edits).
    fn approve_covered_prompts(&mut self, id: &str, level: HandHolding, cx: &mut Context<Self>) {
        let covered: Vec<String> = self
            .live
            .get(id)
            .map(|l| {
                l.permissions
                    .iter()
                    // Questions and plans always need a person, whatever the level.
                    .filter(|p| p.prompt.is_none())
                    .filter(|p| match level {
                        HandHolding::FullAccess => true,
                        HandHolding::AutoAcceptEdits | HandHolding::Auto => matches!(p.title.as_str(), "Edit" | "Write"),
                        HandHolding::Supervised => false,
                    })
                    .map(|p| p.request_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for rid in covered {
            self.respond(id, &rid, Decision::Allow, cx);
        }
    }

    /// Change a thread's hand-holding (or the draft's), as the composer menu does.
    pub fn set_hand_holding(&mut self, id: Option<&str>, level: HandHolding, cx: &mut Context<Self>) -> Result<(), String> {
        if level == HandHolding::FullAccess && !self.settings.permissions.full_access_unlocked {
            return Err("Full access is off. Turn on “Allow Full access” in Settings → Permissions first.".into());
        }
        match id {
            Some(id) if self.route == Route::Thread(id.to_string()) => {
                let mut p = self.prefs();
                p.hand_holding = level;
                self.set_prefs(p, cx);
            }
            Some(id) => {
                self.mutate_thread(id, cx, |t| t.hand_holding = level);
                if let Some(tx) = self.live.get(id).and_then(|l| l.commands.clone()) {
                    let _ = tx.try_send(Command::SetHandHolding(level));
                }
                self.approve_covered_prompts(id, level, cx);
            }
            None => self.draft_prefs.hand_holding = level,
        }
        cx.notify();
        Ok(())
    }

    /// `/permissions [supervised|edits|auto|full]` — shows or changes the level.
    fn permissions_command(&mut self, id: Option<&str>, arg: Option<&str>, cx: &mut Context<Self>) -> String {
        let current = id.and_then(|i| self.thread(i)).map(|t| t.hand_holding).unwrap_or(self.draft_prefs.hand_holding);
        let level = match arg.map(|a| a.to_lowercase()) {
            None => {
                return format!(
                    "Hand-holding is **{}**. Switch with `/permissions supervised`, `/permissions edits`, `/permissions auto` or `/permissions full`.",
                    current.label()
                )
            }
            Some(a) => match a.as_str() {
                "supervised" | "ask" | "default" => HandHolding::Supervised,
                "edits" | "accept-edits" | "auto-accept" | "auto-accept-edits" | "acceptedits" => HandHolding::AutoAcceptEdits,
                "auto" => HandHolding::Auto,
                "full" | "full-access" | "bypass" | "yolo" => HandHolding::FullAccess,
                other => return format!("Unknown level “{other}”. Use supervised, edits, auto or full."),
            },
        };
        match self.set_hand_holding(id, level, cx) {
            Ok(()) => format!("Hand-holding set to **{}**. {}", level.label(), level.description()),
            Err(e) => e,
        }
    }

    fn run_builtin_command(&mut self, id: &str, text: &str, cx: &mut Context<Self>) -> Option<String> {
        let cmd = text.strip_prefix('/')?.split_whitespace().next()?;
        let agent = self.thread(id).map(|t| t.agent.clone())?;
        match cmd {
            "permissions" | "access" | "mode" => {
                let arg = text.split_whitespace().nth(1).map(str::to_string);
                Some(self.permissions_command(Some(id), arg.as_deref(), cx))
            }
            "clear" | "new" => {
                let project = self.thread(id).and_then(|t| t.cwd.clone());
                self.navigate(Route::Draft { project }, cx);
                Some(String::new())
            }
            "usage" => {
                self.status_fetched_at = 0;
                self.refresh_usage(cx);
                let st = self.agent_status.get(&agent.key())?;
                let mut lines = vec![format!("**{}** · {}", agent.display_name(), st.plan.clone().unwrap_or_else(|| "no plan reported".into()))];
                for l in &st.limits {
                    lines.push(format!("- {}: {:.0}% used{}", l.label, l.percent, l.resets_at.map(|r| format!(", resets {}", crate::time::until(r))).unwrap_or_default()));
                }
                Some(lines.join("\n"))
            }
            "context" => {
                let (used, window) = self.live.get(id).and_then(|l| l.context)?;
                Some(format!("{} of {} tokens in context ({:.0}%).", fmt_tokens(used), fmt_tokens(window), used as f64 / window.max(1) as f64 * 100.))
            }
            "cost" => {
                let live = self.live.get(id);
                let cost = live.map(|l| l.cost_usd).unwrap_or(0.0);
                // Claude Code reports cost on every turn; ACP agents (OpenCode) only for priced models.
                let reports_cost = agent == AgentId::ClaudeCode || cost > 0.0;
                Some(cost_reply(reports_cost, live.and_then(|l| l.billing.as_ref()), cost))
            }
            "model" => {
                let t = self.thread(id)?;
                Some(format!("{} · {}", agent.display_name(), t.model.clone().unwrap_or_else(|| "default model".into())))
            }
            _ => None,
        }
    }

    /// MCP servers handed to every new agent session: Trek's own tools plus the user's.
    pub fn mcp_servers(&self) -> Vec<McpServer> {
        let tools = &self.settings.tools;
        let mut out = vec![];
        if let Some(bin) = trek_mcp_binary() {
            let bin = bin.display().to_string();
            for (on, family) in [(tools.computer_use, "computer"), (tools.simulator, "simulator")] {
                if on {
                    out.push(McpServer { name: format!("trek-{family}"), command: bin.clone(), args: vec![family.into()], env: vec![] });
                }
            }
        }
        for s in tools.mcp_servers.iter().filter(|s| s.enabled) {
            out.push(McpServer { name: s.name.clone(), command: s.command.clone(), args: s.args.clone(), env: vec![] });
        }
        out
    }

    fn fetch_codex_models(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_agents::codex_models().await.unwrap_or_default()).await;
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(models) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.codex_models = models;
                    cx.notify();
                });
            }
        });
        self.tasks.push(task);
    }

    pub fn import_threads(&mut self, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        self.importing = true;
        cx.notify();
        let store = self.store.clone();
        let settings = self.settings.import.clone();
        let task = cx.spawn(async move |this, cx| {
            let summary = cx.background_executor().spawn(async move { trek_core::import::import_all(&store, &settings) }).await;
            let _ = this.update(cx, |this, cx| {
                this.importing = false;
                this.import_summary = Some(summary);
                // Projects removed from Trek stay removed: archive anything the import brought back.
                if let Ok(projects) = this.store.projects() {
                    for p in projects.iter().filter(|p| this.settings.hidden_projects.contains(&p.path.display().to_string())) {
                        let _ = this.store.archive_project_threads(&p.id);
                    }
                }
                this.reload(cx);
                this.index_for_search(cx);
            });
        });
        self.tasks.push(task);
    }

    pub fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.settings.onboarding.completed = true;
        self.settings.general.hand_holding = self.draft_prefs.hand_holding;
        self.settings.general.default_agent = self.draft_prefs.agent.key();
        self.save_settings(cx);
        let project = self.projects.first().map(|p| p.path.clone());
        self.navigate(Route::Draft { project }, cx);
    }

    // ---------- updates ----------

    pub fn update_view(&self) -> UpdateView {
        crate::updater::describe_update(&self.updater.status, self.settings.updates.auto_check, trek_core::update::blocker())
    }

    /// The update on offer, while it's on offer and has release notes.
    pub fn update_notes(&self) -> Option<&trek_core::update::AvailableUpdate> {
        self.updater.notes()
    }

    pub fn run_update_action(&mut self, action: UpdateAction, cx: &mut Context<Self>) {
        match action {
            UpdateAction::Check => self.check_for_updates(true, cx),
            UpdateAction::Download => self.download_update(cx),
            UpdateAction::Restart => self.restart_to_update(cx),
        }
    }

    /// Ask the release feed for something newer. A background check stays quiet when it fails; a
    /// check the user asked for reports. Never runs while an update is downloading or waiting.
    pub fn check_for_updates(&mut self, user_initiated: bool, cx: &mut Context<Self>) {
        // A dev build (or one macOS runs from a read-only copy) can't replace itself: don't ask.
        if trek_core::update::blocker().is_some() {
            return;
        }
        let Some(ticket) = self.updater.begin_check(self.settings.updates.channel, now_ms()) else { return };
        cx.notify();
        let urls = trek_core::update::manifest_urls(&self.settings.updates);
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::update::check(&urls).await.map_err(|e| format!("{e:#}"))).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                let auto_download = this.settings.updates.auto_download;
                if this.updater.finish_check(ticket, result, user_initiated, auto_download, now_ms()) {
                    this.download_update(cx);
                }
                cx.notify();
            });
        });
        self.tasks.push(task);
    }

    /// Re-check about once a day while Trek stays open (housekeeping calls this every minute).
    fn maybe_check_for_updates(&mut self, cx: &mut Context<Self>) {
        if self.updater.check_due(self.settings.updates.auto_check, now_ms()) {
            self.check_for_updates(false, cx);
        }
    }

    /// Download, verify and unpack the update the last check found; it's `Ready` once the new
    /// bundle sits staged, so installing is a rename.
    pub fn download_update(&mut self, cx: &mut Context<Self>) {
        let Some(crate::updater::DownloadJob { update, ticket, cancel }) = self.updater.begin_download() else { return };
        let version = update.version.to_string();
        cx.notify();
        // The error, and whether it may pass (offline) so it's worth retrying within the hour.
        let (tx, rx) = async_channel::bounded::<Result<PathBuf, (String, bool)>>(1);
        let (ptx, prx) = async_channel::unbounded::<f32>();
        trek_core::runtime().spawn(async move {
            let staged = async {
                let archive = trek_core::update::download(&update, &cancel, |p| {
                    let _ = ptx.try_send(p);
                })
                .await?;
                let expected = update.version.clone();
                tokio::task::spawn_blocking(move || trek_core::update::stage(&archive, &expected, &cancel)).await?
            };
            let result = staged.await.map_err(|e: anyhow::Error| (format!("{e:#}"), trek_core::update::is_transient(&e)));
            let _ = tx.send(result).await;
        });
        let progress_task = cx.spawn(async move |this, cx| {
            while let Ok(p) = prx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if this.updater.download_progress(ticket, p) {
                        cx.notify();
                    }
                });
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(unwanted) = this.updater.finish_download(ticket, version, result, now_ms()) {
                    cx.background_executor().spawn(async move { trek_core::update::discard(&unwanted) }).detach();
                }
                // Test hook (docs/RELEASING.md): restart as soon as the update is ready instead of
                // waiting for a click or for Trek to quit.
                if matches!(this.updater.status, UpdateStatus::Ready { .. }) && std::env::var_os("TREK_UPDATE_AUTO_RESTART").is_some() {
                    this.restart_to_update(cx);
                }
                cx.notify();
            });
        });
        self.tasks.push(progress_task);
        self.tasks.push(task);
    }

    /// Forget the update in flight or waiting (the channel changed): its download stops, late
    /// results are ignored, and the staged copy is deleted rather than installed on quit.
    fn drop_update(&mut self, cx: &mut Context<Self>) {
        if let Some(staged) = self.updater.drop_update() {
            cx.background_executor().spawn(async move { trek_core::update::discard(&staged) }).detach();
        }
    }

    /// Install now unless agent work is in flight (`work_in_flight`); otherwise once it's over.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if let UpdateStatus::Ready { version, staged } = self.updater.status.clone() {
            if self.work_in_flight() {
                self.updater.status = UpdateStatus::RestartPending { version, staged };
                cx.emit(WorkspaceEvent::Toast { message: "Trek will restart when your agents finish.".into(), undo: None });
                cx.notify();
            } else {
                self.install_update(staged, cx);
            }
        }
    }

    /// Once the agents are done, a restart left pending counts down `RESTART_GRACE` first, with a
    /// toast that can call it off.
    fn maybe_restart_for_update(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.updater.status, UpdateStatus::RestartPending { .. }) || self.work_in_flight() || self.restart_countdown.is_some() {
            return;
        }
        let message = format!("Trek restarts to update in {} seconds.", RESTART_GRACE.as_secs());
        cx.emit(WorkspaceEvent::Toast { message, undo: Some(UndoAction::CancelRestart) });
        self.restart_countdown = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESTART_GRACE).await;
            let _ = this.update(cx, |this, cx| {
                this.restart_countdown = None;
                // Work that started meanwhile is waited for again (housekeeping comes back).
                if let UpdateStatus::RestartPending { staged, .. } = this.updater.status.clone()
                    && !this.work_in_flight()
                {
                    this.install_update(staged, cx);
                }
            });
        }));
    }

    fn install_update(&mut self, staged: PathBuf, cx: &mut Context<Self>) {
        let installed = match trek_core::update::install(&staged) {
            Ok(installed) => installed,
            Err(e) => {
                tracing::warn!("update install failed: {e:#}");
                self.updater.status = UpdateStatus::Failed(format!("Couldn't install the update: {e:#}"));
                cx.notify();
                return;
            }
        };
        // Installed: nothing is left for the quit hook to do.
        self.updater.status = UpdateStatus::Idle;
        if let Err(e) = trek_core::update::relaunch(&installed, crate::system::app_is_active()) {
            tracing::warn!("relaunch after update failed: {e:#}");
            self.updater.status = UpdateStatus::Failed("The update is installed. Quit and reopen Trek to start it.".into());
            cx.notify();
            return;
        }
        self.shutdown_sessions();
        cx.quit();
    }

    /// Quitting with an update ready installs it, so the next launch is the new version.
    fn install_on_quit(&mut self) {
        if let UpdateStatus::Ready { staged, .. } | UpdateStatus::RestartPending { staged, .. } = &self.updater.status {
            match trek_core::update::install(staged) {
                Ok(_) => tracing::info!("update installed on quit"),
                Err(e) => tracing::warn!("update install on quit failed: {e:#}"),
            }
        }
    }

    pub fn shutdown_sessions(&mut self) {
        for live in self.live.values() {
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::Shutdown);
            }
        }
    }
}

/// One piece of a thread's git work (blocking). Returns how many files a restore changed.
fn git_job(store: &Store, thread: &str, job: GitJob) -> anyhow::Result<GitDone> {
    use trek_core::checkpoint::{KEEP, Repo};
    let repo_at = |path: &std::path::Path| Repo::find(path).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", path.display()));
    match job {
        GitJob::Checkpoint { cwd, item } => {
            // Folders outside git get no checkpoints.
            let Some(repo) = Repo::find(&cwd) else { return Ok(GitDone::Nothing) };
            let sha = repo.snapshot(thread, &item)?;
            store.add_checkpoint(thread, &item, &repo.top, &sha)?;
            prune_checkpoints(store, thread, KEEP)?;
            Ok(GitDone::Taken)
        }
        GitJob::FindPoint { agent, session, .. } => Ok(GitDone::Point(trek_agents::session_tail(&agent, &session))),
        GitJob::Restore { repo, sha } => Ok(GitDone::Restored(repo_at(&repo)?.restore(&sha, thread)?)),
        GitJob::Forget { repo, items } => {
            if let Some(r) = Repo::find(&repo) {
                r.delete(thread, &items)?;
            }
            store.delete_checkpoints(thread, &items)?;
            Ok(GitDone::Nothing)
        }
        GitJob::Link { repo, checkpoints } => {
            repo_at(&repo)?.link(thread, &checkpoints)?;
            for (item, sha) in &checkpoints {
                store.add_checkpoint(thread, item, &repo, sha)?;
            }
            Ok(GitDone::Nothing)
        }
    }
}

/// Keep only `thread`'s newest `keep` checkpoints, so refs don't pile up in the repo.
fn prune_checkpoints(store: &Store, thread: &str, keep: usize) -> anyhow::Result<()> {
    let all = store.checkpoints(thread)?;
    let Some(old) = all.len().checked_sub(keep).filter(|n| *n > 0).map(|n| &all[..n]) else { return Ok(()) };
    let mut by_repo: HashMap<&std::path::Path, Vec<String>> = HashMap::new();
    for c in old {
        by_repo.entry(c.repo.as_path()).or_default().push(c.item_id.clone());
    }
    for (path, items) in by_repo {
        if let Some(r) = trek_core::checkpoint::Repo::find(path) {
            r.delete(thread, &items)?;
        }
        store.delete_checkpoints(thread, &items)?;
    }
    Ok(())
}

/// How a fork of `thread`'s whole conversation (`kept`) picks it up: the agent's session copied
/// as it stands (`mark`: its latest point), or a recap.
fn whole_session(thread: &Thread, mark: Option<String>, kept: &[Item], native: bool) -> Option<Reopen> {
    if !kept.iter().any(|i| matches!(i, Item::User { .. })) {
        return None;
    }
    // A rewind or fork not yet picked up says where the conversation stands.
    let (session, at) = match &thread.reopen {
        Some(Reopen::Native { session, at, .. }) => (Some(session.clone()), at.clone()),
        Some(Reopen::Recap) => (None, None),
        None => (thread.native_id.clone(), mark.or_else(|| thread.native_at.clone())),
    };
    match session {
        Some(session) if native => Some(Reopen::Native { session, at, fork: true }),
        _ => Some(Reopen::Recap),
    }
}

/// Said in a transcript whose next session starts with a recap.
fn recap_notice(agent: &AgentId) -> String {
    format!("{} can't take its own session back to this point, so your next message starts a new session with a recap of the conversation so far.", agent.display_name())
}

/// One round of background indexing: everything waiting in the backfill, then up to 16 imported
/// transcripts. Returns true while imported threads remain.
fn index_step(store: &Store) -> bool {
    loop {
        match store.backfill_search(500) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                tracing::warn!("search backfill: {e}");
                break;
            }
        }
    }
    let batch = match store.imported_to_index(16) {
        Ok(batch) => batch,
        Err(e) => {
            tracing::warn!("search index: {e}");
            return false;
        }
    };
    for t in &batch {
        let items = match trek_core::import::transcript_bytes(t.source, &t.native_id) {
            Some(n) if n > trek_core::store::INDEX_MAX_BYTES => None,
            _ => trek_core::import::load_transcript(t.source, &t.native_id).ok(),
        };
        if let Err(e) = store.index_imported(&t.thread_id, items.as_deref(), t.updated_at) {
            tracing::warn!("search index {}: {e}", t.thread_id);
            return false;
        }
    }
    !batch.is_empty()
}

/// The settings that seed a new thread's composer.
fn default_prefs_key(s: &Settings) -> (String, Option<String>, Effort, HandHolding) {
    let g = &s.general;
    (g.default_agent.clone(), g.default_model.clone(), g.default_effort, g.hand_holding)
}

/// Convenience for views.
pub fn workspace_global(cx: &App) -> Entity<Workspace> {
    cx.global::<GlobalWorkspace>().0.clone()
}

pub struct GlobalWorkspace(pub Entity<Workspace>);
impl gpui_kit::Global for GlobalWorkspace {}

pub fn init(cx: &mut App) -> Entity<Workspace> {
    let ws = cx.new(Workspace::new);
    cx.set_global(GlobalWorkspace(ws.clone()));
    ws
}

/// Commands Trek handles itself, shown first in the `/` menu.
pub const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("new", "Start a new thread in this project"),
    ("usage", "Show plan usage and reset times"),
    ("context", "Show how much of the context window is used"),
    ("cost", "Show this session's estimated cost"),
    ("model", "Show the model this thread uses"),
    ("permissions", "Show or change how much the agent asks first"),
    ("permissions supervised", "Ask before every edit and command"),
    ("permissions edits", "Apply file edits; ask before commands"),
    ("permissions auto", "Work on its own; check before risky actions"),
    ("permissions full", "No prompts and no sandbox"),
];

/// The thread `scope` shows: a thread window's own, or whatever thread the main window is on.
fn scope_thread<'a>(scope: &'a Scope, route: &'a Route) -> Option<&'a str> {
    match (scope, route) {
        (Scope::Thread(id), _) | (Scope::Main, Route::Thread(id)) => Some(id),
        _ => None,
    }
}

fn scope_is_draft(scope: &Scope, route: &Route) -> bool {
    *scope == Scope::Main && matches!(route, Route::Draft { .. })
}

/// See `Workspace::shown_in`. `own_window`: the thread has a window of its own; `main_open`: the
/// main window hasn't been closed.
fn shown_in(id: &str, route: &Route, own_window: bool, main_open: bool) -> Option<Scope> {
    if own_window {
        Some(Scope::Thread(id.to_string()))
    } else if main_open && matches!(route, Route::Thread(t) if t == id) {
        Some(Scope::Main)
    } else {
        None
    }
}

/// Drop `id`'s window from the registry if it's still `handle` (a newer window for the same
/// thread may have replaced it). Returns whether anything changed.
fn forget_window<W: PartialEq>(windows: &mut HashMap<String, W>, id: &str, handle: W) -> bool {
    let current = windows.get(id) == Some(&handle);
    if current {
        windows.remove(id);
    }
    current
}

/// `scope`'s view of who's looking: shown in the main window (`in_main`) or its own `window`,
/// against the frontmost Trek window (`active`, none when another app is in front).
fn viewing<W: PartialEq + Copy>(in_main: bool, window: Option<W>, main: Option<W>, active: Option<W>) -> bool {
    match active {
        None => in_main || window.is_some(),
        Some(a) => (in_main && main == Some(a)) || window == Some(a),
    }
}

/// What a turn added to a thread's spend, given the agent's running session `total` and the
/// total it reported last. A zero total (a failed start) is ignored. A total below the last one
/// means the agent started counting again (a new process without a saved total, or `/clear`),
/// so all of it is new. A resumed session that carries on from its saved total adds only the
/// difference, which is why `last` outlives the process.
fn cost_added(last: &mut f64, total: f64) -> f64 {
    if total <= 0.0 {
        return 0.0;
    }
    let added = if total >= *last { total - *last } else { total };
    *last = total;
    added
}

/// "your Claude Max plan", or "your subscription" when the plan has no name.
fn plan_phrase(plan: &Option<String>) -> String {
    plan.as_deref().map(|p| format!("your {p} plan")).unwrap_or_else(|| "your subscription".into())
}

/// What the composer says about a thread's spend: `(status strip text, agent tooltip)`. Only
/// metered sessions show a cost in the strip. On a subscription the figure is what the same
/// tokens would cost through the API, which the plan already covers, so it stays in the tooltip.
pub fn cost_note(billing: Option<&Billing>, cost: f64) -> (Option<String>, Option<String>) {
    let spent = cost >= 0.005;
    match billing {
        Some(Billing::Metered) => (spent.then(|| format!("${cost:.2} this thread")), Some("Billed per token by your API provider".into())),
        Some(Billing::Plan(plan)) => {
            let plan = plan_phrase(plan);
            (None, Some(if spent { format!("≈${cost:.2} at API prices — included in {plan}") } else { format!("Included in {plan}") }))
        }
        Some(Billing::Local) => (None, Some("Runs on this Mac, nothing is billed".into())),
        None => (None, spent.then(|| format!("≈${cost:.2} at API prices"))),
    }
}

/// The `/cost` answer. `reports_cost`: the agent reports what the session costs (Claude Code always,
/// ACP agents for priced models).
fn cost_reply(reports_cost: bool, billing: Option<&Billing>, cost: f64) -> String {
    match (billing, reports_cost) {
        (Some(Billing::Local), _) => "This session runs on a local model, so nothing is billed.".into(),
        (Some(Billing::Plan(plan)), true) => {
            format!("About ${cost:.2} at API prices so far. It's included in {}, so nothing is charged per token.", plan_phrase(plan))
        }
        (Some(Billing::Plan(plan)), false) => format!("This session is included in {}.", plan_phrase(plan)),
        (Some(Billing::Metered), true) => format!("This session has cost ${cost:.2} so far."),
        (Some(Billing::Metered), false) => "This session is billed per token by your API provider; the agent doesn't report what it has cost.".into(),
        (None, true) => format!("About ${cost:.2} so far at API prices."),
        (None, false) => "This agent doesn't report what a session costs.".into(),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0}K", n as f64 / 1_000.),
        _ => {
            let m = n as f64 / 1_000_000.;
            if m.fract() < 0.05 { format!("{m:.0}M") } else { format!("{m:.1}M") }
        }
    }
}

/// The bundled `trek-mcp` server: next to the app binary, else a dev build.
pub fn trek_mcp_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("trek-mcp"), dir.join("../Resources/trek-mcp")].into_iter().find(|p| p.exists())
}

/// Whether merging a thread's work settles it: only one sitting idle in the inbox. Merged while
/// it was busy, pinned, kept or snoozed, it stays where the user put it. Shared by the merge
/// watch (`settle_merged`) and the Git tool's "Merge into <base>".
pub(crate) fn settles_on_merge(t: &Thread, now: i64) -> bool {
    t.settled_at.is_none() && t.run_state == RunState::Idle && t.pinned_at.is_none() && !t.never_settle && !t.snoozed_until.is_some_and(|u| u > now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_agents::Question;

    fn q(text: &str, secret: bool) -> Question {
        Question { question: text.into(), header: String::new(), options: vec![("Yes".into(), String::new())], multi: false, secret }
    }

    #[test]
    fn typed_text_answers_the_first_open_question() {
        let qs = [q("Name?", false), q("Color?", false), q("Size?", false)];
        // "Color?" was picked on the card; the typed text answers "Name?", "Size?" stays open.
        let picked = |i: usize| (i == 1).then(|| "Blue".to_string());
        let (answers, secret) = typed_answers(&qs, picked, "Trek");
        assert_eq!(answers, vec![("Name?".to_string(), "Trek".to_string()), ("Color?".into(), "Blue".into())]);
        assert!(!secret);
        // Everything picked: the typed text replaces the last answer.
        let (answers, _) = typed_answers(&qs, |_| Some("Yes".to_string()), "No, large");
        assert_eq!(answers.last(), Some(&("Size?".to_string(), "No, large".to_string())));
        assert_eq!(answers.len(), 3);
    }

    #[test]
    fn a_typed_secret_is_flagged() {
        let qs = [q("User?", false), q("Token?", true)];
        let (answers, secret) = typed_answers(&qs, |i| (i == 0).then(|| "me".to_string()), "s3cr3t");
        assert_eq!(answers[1], ("Token?".to_string(), "s3cr3t".to_string()));
        assert!(secret);
        assert!(!typed_answers(&qs, |_| None, "me").1);
    }

    #[test]
    fn card_answers_read_as_the_users_message_without_secrets() {
        let one = [q("Which database?", false)];
        assert_eq!(answers_text(&one, &[("Which database?".into(), "SQLite".into())]), ("SQLite".to_string(), false));
        let db = Question { header: "Database".into(), ..q("Which database?", false) };
        let qs = [db, q("What ships with it?", false), q("Token?", true)];
        let answers = [("Which database?".to_string(), "Postgres".to_string()), ("What ships with it?".into(), "Migrations, Backups".into()), ("Token?".into(), "s3cr3t".into())];
        let (text, secret) = answers_text(&qs, &answers);
        assert_eq!(text, "Database: Postgres\nWhat ships with it?: Migrations, Backups");
        assert!(secret);
        assert_eq!(answers_text(&qs, &answers[2..]), (String::new(), true));
    }

    #[test]
    fn prompts_offered_after_their_turn_go_when_new_work_starts() {
        let mut live = LiveThread::default();
        let card = |rid: &str, after_turn| PendingPermission { request_id: rid.into(), title: "Plan".into(), detail: String::new(), prompt: None, after_turn };
        live.permissions = vec![card("codex-plan-1", true), card("codex-7", false)];
        live.picks.insert(("codex-plan-1".into(), 0), vec!["x".into()]);
        live.drop_after_turn();
        assert_eq!(live.permissions.iter().map(|p| p.request_id.as_str()).collect::<Vec<_>>(), vec!["codex-7"]);
        assert!(live.picks.is_empty());
    }

    fn thread(s: &Store, title: &str) -> Thread {
        let mut t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
        t.title = title.into();
        s.save_thread(&t).unwrap();
        t
    }

    #[test]
    fn sidebar_search_lists_title_and_message_matches() {
        let s = Store::in_memory().unwrap();
        let lights = thread(&s, "Stadium lighting");
        let kits = thread(&s, "Kit colours");
        let other = thread(&s, "Unrelated");
        let mut tr = Transcript::default();
        tr.push(Item::Assistant { text: "the stadium shader tints the kits".into() });
        s.save_transcript(&kits.id, &mut tr).unwrap();

        let q = "stad light";
        let results = SearchResults::from_hits(q.into(), s.search(q, 50).unwrap());
        // Word prefixes find the title the substring test misses; no message line under it.
        assert!(results.matches(q, &lights) && results.content_hit(q, &lights).is_none());
        assert!(!results.matches(q, &kits) && !results.matches(q, &other));

        let q = "shader";
        let results = SearchResults::from_hits(q.into(), s.search(q, 50).unwrap());
        assert!(results.matches(q, &kits));
        assert_eq!(results.content_hit(q, &kits).and_then(|h| h.position), Some(0));
        assert!(!results.matches(q, &lights));
        // Typed straight into the title: matched before the full-text search answers.
        assert!(SearchResults::default().matches("colo", &kits));
    }

    #[test]
    fn results_stand_in_only_for_related_text() {
        let results = SearchResults { query: "dark".into(), ..Default::default() };
        assert!(results.relevant_to("dark t") && results.relevant_to("dar"));
        assert!(!results.relevant_to("light") && !SearchResults::default().relevant_to("dark"));
    }

    #[test]
    fn claude_running_totals_are_not_summed() {
        // Three $1 turns report running totals 1, 2, 3: the thread has spent $3, not $6.
        let (mut last, mut spent) = (0.0, 0.0);
        for total in [1.0, 2.0, 3.0] {
            spent += cost_added(&mut last, total);
        }
        assert_eq!(spent, 3.0);
        // A failed start reports zero: ignored, and the next total still counts from 3.
        spent += cost_added(&mut last, 0.0);
        spent += cost_added(&mut last, 3.5);
        assert_eq!(spent, 3.5);
        // A relaunched process that resumes from its saved total carries on from it.
        spent += cost_added(&mut last, 4.0);
        assert_eq!(spent, 4.0);
        // One that starts again from zero (no saved total, or /clear) adds all of its total.
        spent += cost_added(&mut last, 0.25);
        spent += cost_added(&mut last, 0.75);
        assert_eq!(spent, 4.75);
    }

    #[test]
    fn scopes_resolve_against_the_main_route() {
        let on_a = Route::Thread("a".into());
        let draft = Route::Draft { project: None };
        let settings = Route::Settings(super::SettingsPage::General);
        let window_b = Scope::Thread("b".into());
        // The main window follows its route.
        assert_eq!(scope_thread(&Scope::Main, &on_a), Some("a"));
        assert_eq!(scope_thread(&Scope::Main, &draft), None);
        assert_eq!(scope_thread(&Scope::Main, &settings), None);
        assert!(scope_is_draft(&Scope::Main, &draft));
        assert!(!scope_is_draft(&Scope::Main, &on_a));
        // A thread window stays on its thread whatever the main window shows.
        assert_eq!(scope_thread(&window_b, &on_a), Some("b"));
        assert_eq!(scope_thread(&window_b, &draft), Some("b"));
        assert!(!scope_is_draft(&window_b, &draft));
    }

    #[test]
    fn queued_follow_ups_go_where_the_thread_is_shown() {
        let on_a = Route::Thread("a".into());
        assert_eq!(shown_in("a", &on_a, false, true), Some(Scope::Main));
        // Its own window wins over the main window showing it too.
        assert_eq!(shown_in("a", &on_a, true, true), Some(Scope::Thread("a".into())));
        assert_eq!(shown_in("b", &on_a, true, true), Some(Scope::Thread("b".into())));
        assert_eq!(shown_in("b", &on_a, false, true), None);
        // A closed main window shows nothing, whatever its route says.
        assert_eq!(shown_in("a", &on_a, false, false), None);
        assert_eq!(shown_in("a", &on_a, true, false), Some(Scope::Thread("a".into())));
    }

    #[test]
    fn closing_a_thread_window_forgets_only_that_window() {
        let mut windows = HashMap::from([("a".to_string(), 1), ("b".to_string(), 2)]);
        assert!(forget_window(&mut windows, "a", 1));
        assert!(!windows.contains_key("a"));
        // A window released after the thread got a newer one leaves the newer one registered.
        windows.insert("b".into(), 3);
        assert!(!forget_window(&mut windows, "b", 2));
        assert_eq!(windows.get("b"), Some(&3));
        assert!(!forget_window(&mut windows, "c", 9));
    }

    #[test]
    fn subscription_cost_is_an_estimate_in_the_tooltip() {
        let max = Billing::Plan(Some("Claude Max".into()));
        assert_eq!(cost_note(Some(&max), 10.468), (None, Some("≈$10.47 at API prices — included in your Claude Max plan".into())));
        assert_eq!(cost_note(Some(&max), 0.0), (None, Some("Included in your Claude Max plan".into())));
        assert_eq!(cost_note(Some(&Billing::Plan(None)), 2.0).1.as_deref(), Some("≈$2.00 at API prices — included in your subscription"));
        assert_eq!(cost_note(Some(&Billing::Metered), 0.42), (Some("$0.42 this thread".into()), Some("Billed per token by your API provider".into())));
        assert_eq!(cost_note(Some(&Billing::Metered), 0.001).0, None);
        assert_eq!(cost_note(Some(&Billing::Local), 0.0).0, None);
        assert_eq!(cost_note(None, 1.0), (None, Some("≈$1.00 at API prices".into())));
        assert_eq!(cost_note(None, 0.0), (None, None));
    }

    #[test]
    fn cost_command_answers_per_billing() {
        let plus = Billing::Plan(Some("ChatGPT Plus".into()));
        assert_eq!(cost_reply(false, Some(&plus), 0.0), "This session is included in your ChatGPT Plus plan.");
        assert!(cost_reply(true, Some(&Billing::Plan(Some("Claude Max".into()))), 3.5).starts_with("About $3.50 at API prices so far. It's included in your Claude Max plan"));
        assert_eq!(cost_reply(true, Some(&Billing::Metered), 1.25), "This session has cost $1.25 so far.");
        assert_eq!(cost_reply(true, Some(&Billing::Local), 0.0), "This session runs on a local model, so nothing is billed.");
    }

    #[test]
    fn messages_without_a_checkpoint_say_why() {
        let user = |t: &str| Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false };
        let mut live = LiveThread::default();
        live.items = Transcript::unsaved(vec![user("a"), Item::Assistant { text: "1".into() }, user("steer"), Item::TurnEnd { at: 1, took_secs: 1 }, user("b"), user("c")]);
        let id = |ix: usize| live.items.id_at(ix).unwrap().to_string();
        live.checkpointed.insert(id(0));
        live.checkpoint_failed.insert(id(5), "git add took longer than 10s".into());
        assert_eq!(NoCheckpoint::of(&live, 0, true), None);
        assert_eq!(NoCheckpoint::of(&live, 2, true), Some(NoCheckpoint::Steered));
        assert_eq!(NoCheckpoint::of(&live, 4, true), Some(NoCheckpoint::Missing));
        assert_eq!(NoCheckpoint::of(&live, 4, false), Some(NoCheckpoint::NotGit));
        assert!(matches!(NoCheckpoint::of(&live, 5, true), Some(NoCheckpoint::Failed(why)) if why.contains("longer")));
        assert!(NoCheckpoint::Failed("boom".into()).explain().contains("(boom)"));
        // In a worktree that's gone: checkpoints wait for it; the rest keep their reason.
        assert_eq!(NoCheckpoint::now(&live, 0, false, true), Some(NoCheckpoint::WorktreeMissing));
        assert_eq!(NoCheckpoint::now(&live, 2, false, true), Some(NoCheckpoint::Steered));
        assert_eq!(NoCheckpoint::now(&live, 0, true, false), None);
        assert_eq!(NoCheckpoint::now(&live, 4, false, false), Some(NoCheckpoint::NotGit));
    }

    #[test]
    fn only_the_newest_checkpoints_are_kept() {
        let dir = std::env::temp_dir().join(format!("trek-prune-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&dir).output().unwrap().status.success());
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let s = Store::in_memory().unwrap();
        let t = thread(&s, "checkpoints");
        let repo = trek_core::checkpoint::Repo::find(&dir).unwrap();
        for item in ["m1", "m2", "m3"] {
            let sha = repo.snapshot(&t.id, item).unwrap();
            s.add_checkpoint(&t.id, item, &repo.top, &sha).unwrap();
        }
        prune_checkpoints(&s, &t.id, 2).unwrap();
        assert_eq!(s.checkpoints(&t.id).unwrap().into_iter().map(|c| c.item_id).collect::<Vec<_>>(), ["m2", "m3"]);
        assert_eq!(repo.items(&t.id).unwrap(), ["m2", "m3"]);
        prune_checkpoints(&s, &t.id, 2).unwrap();
        assert_eq!(repo.items(&t.id).unwrap().len(), 2, "nothing past the limit: nothing goes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn viewing_follows_the_frontmost_window() {
        let (main, thread_win, other) = (Some(1), Some(2), Some(3));
        // Trek in the background: on screen anywhere counts.
        assert!(viewing(true, None, main, None));
        assert!(viewing(false, thread_win, main, None));
        assert!(!viewing(false, None, main, None));
        // Trek in front: only the frontmost window counts.
        assert!(viewing(true, None, main, main));
        assert!(!viewing(true, None, main, thread_win));
        assert!(viewing(false, thread_win, main, thread_win));
        assert!(!viewing(false, thread_win, main, main));
        assert!(!viewing(false, thread_win, main, other));
    }
}

