//! The application model: threads, live agent sessions, routing, updates. Views observe it.

mod agent_updates;
mod limits;
mod orchestrate;
mod tabs;
#[cfg(test)]
pub use tabs::MAX_TABS;
mod turn_changes;
mod verification;
mod worktrees;

pub use agent_updates::Hold;
pub use limits::Clock;
pub use orchestrate::{TaskState, waiting_label};
pub use turn_changes::TurnRange;
pub use verification::ago;

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

/// The project filter that keeps the threads without a project.
pub const NO_PROJECT: &str = "";

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Composing a new thread in a project.
    Draft { project: Option<PathBuf> },
    Thread(String),
    Settings(SettingsPage),
    /// The recap of the day's (or week's) work, with what's ready for review.
    Basecamp,
    /// Notes: things jotted down, in markdown.
    Notes,
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
    Mobile,
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
            SettingsPage::Mobile => "Phone",
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

/// How long a new title takes to animate in.
pub const TITLE_REVEAL: Duration = Duration::from_millis(380);

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
    /// Its worktree is being removed: messages wait, so no session starts in a folder that's going.
    pub removing: bool,
    /// Index of the assistant item currently streaming.
    pub streaming: Option<usize>,
    pub reasoning: Option<usize>,
    pub permissions: Vec<PendingPermission>,
    /// Options picked so far on the question card: (request id, question index) → labels.
    pub picks: HashMap<(String, usize), Vec<String>>,
    pub commands: Option<async_channel::Sender<Command>>,
    /// Counts the sessions attached so far. A session that was let finish after another took its
    /// place (`end_session`) speaks for the thread no more: its events are dropped.
    session_gen: u64,
    pub turn_started: Option<Instant>,
    /// The running turn reported an error that didn't end it (an image the agent couldn't read).
    turn_error: bool,
    pub plan: bool,
    pub fast: bool,
    /// Settings the agent reads only at launch changed mid-turn: the session restarts (and
    /// resumes) once the turn is over, or is told them then if it has work in the background.
    relaunch: bool,
    /// What the thread and its sub-agents spent, as the store has it (`None`: not read yet).
    /// Updated as the agent reports usage, never per token.
    pub spend: Option<crate::cost::ThreadSpend>,
    /// How the current session is billed, once the agent has said.
    pub billing: Option<Billing>,
    /// Tokens in the context window and the window size, as last reported by the agent.
    pub context: Option<(u64, u64)>,
    /// Bumped on every transcript change so views can resync cheaply.
    pub revision: u64,
    /// Lines each tool call that changed a file added and removed, by call id (kept in the store).
    pub lines: HashMap<String, (u32, u32)>,
    /// The group of tool calls folding into its summary row now that it's over (`activity`).
    pub fold: Option<crate::activity::Fold>,
    /// Shows the folded group in the transcript once the fold is over.
    _fold_done: Option<Task<()>>,
    /// A live group the user opened from the working bar: it shows in the transcript instead,
    /// open, with the call they clicked open too (`open_live_group`).
    pub opened: Option<crate::activity::Opened>,
    /// Follow-ups held while a turn runs (`FollowUp::Queue`), sent one per finished turn.
    pub queued: Vec<(String, Vec<PathBuf>)>,
    /// The turn running is a wake-up Trek sent while `queued` was left over from a turn that
    /// failed or stopped: those were written for that turn and don't follow this one out. They
    /// go back to the composer once the thread is on screen (`hand_back_queued`).
    hold_queue: bool,
    /// Follow-ups a failed turn left queued, set aside when the messages held for an agent
    /// update went without them: back to the composer once the thread is on screen
    /// (`hand_back_queued`).
    left_over: Vec<(String, Vec<PathBuf>)>,
    /// Sub-agents launched this turn (and those still out from earlier ones), keyed by the tool
    /// call that started them.
    pub tasks: Vec<SubTask>,
    /// What the agent has running in the background (`AgentEvent::Background`): shells, monitors
    /// and its own sub-agents. They outlive the turn that started them; the session ending ends
    /// them.
    pub background: Vec<Background>,
    /// When the user last stopped its agent's own sub-agents between turns. Claude Code takes a
    /// turn of its own to say they were stopped; that turn is no news (`quiet_turn`).
    stopped_out: Option<Instant>,
    /// The turn running is the agent saying the sub-agents the user stopped were stopped: it
    /// raises no alert as it ends.
    quiet_turn: bool,
    /// The turn running is one the agent took by itself, woken by work in the background.
    self_started: bool,
    /// When a turn its agent took by itself last raised an alert as it ended.
    woke_alert: Option<Instant>,
    /// Last prompt or agent event (idle sessions are shut down; they resume on the next message).
    pub last_active: Option<Instant>,
    /// The latest point the agent's session can be taken back to (`AgentEvent::Mark`); saved to
    /// the thread as turns end (`Thread::native_at`).
    pub mark: Option<String>,
    /// The model the session said it runs (`AgentEvent::Started`), for token reports that name
    /// none in a thread that names none either.
    session_model: Option<String>,
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
    /// A usage limit the running turn reported: the thread pauses when the turn ends.
    limit: Option<limits::Hit>,
    /// The running turn is a resume after a usage limit: hitting the limit again resumes again.
    resuming: Option<limits::Resuming>,
    /// A save of the transcript on its way (`persist_soon`).
    _save_soon: Option<Task<()>>,
    _events: Option<Task<()>>,
    /// The key its session's orchestration tools use to reach Trek (`ipc`).
    ipc_session: Option<String>,
    /// Rows for sub-agents it started whose `delegate_task` call hasn't reached the transcript
    /// yet (`place_task_rows`).
    pending_rows: VecDeque<Item>,
    /// Project notes its session gives the agent with the next message: recorded as told once
    /// that message goes (`Store::told_notes`).
    pub notes_pending: Option<String>,
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

/// Said under calls an earlier run left going in the background when Trek quit.
pub const ORPHANS_ENDED: &str = "Work left running in the background stopped when Trek closed";

/// A task the agent left running in the background, as a thread keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    pub task: trek_agents::BackgroundTask,
    /// When Trek first heard of it.
    pub started: Instant,
    /// The end of its output as last read (`Workspace::read_background`), once it has been.
    pub output: Option<String>,
    /// Bumped each time `output` changes, so views can tell without comparing it.
    pub output_rev: u64,
    /// Asked to stop: it goes once the agent says it has.
    pub stopping: bool,
}

impl Background {
    pub fn is_agent(&self) -> bool {
        self.task.kind == trek_agents::BackgroundKind::Agent
    }

    /// Its output's last line with something on it.
    pub fn last_line(&self) -> Option<&str> {
        self.output.as_deref()?.lines().rev().map(str::trim).find(|l| !l.is_empty())
    }
}

/// Calls a sub-agent made that its row keeps, newest last (`SubTask::steps`).
pub const SUB_STEPS: usize = 6;

/// A sub-agent's progress, shown on its tool row and in the working bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SubTask {
    pub id: String,
    pub description: String,
    /// What it's doing right now ("Running grep …").
    pub activity: String,
    /// Its latest tool calls, (title, detail), as agents report them: at most `SUB_STEPS`.
    pub steps: Vec<(String, String)>,
    /// How many it has made in all (`steps` keeps the last few).
    pub stepped: usize,
    pub tool_uses: u64,
    /// `Some(ok)` once finished.
    pub done: Option<bool>,
    /// When it started, and when it finished.
    pub started: Instant,
    pub ended: Option<Instant>,
}

impl LiveThread {
    pub fn active_tasks(&self) -> usize {
        self.tasks.iter().filter(|t| t.done.is_none()).count()
    }

    /// Its agent's own sub-agents its turn is blocked on: tool calls are running, every one of
    /// them is a sub-agent at work (a foreground `Task`), and the agent hasn't gone on to say or
    /// think anything since (Codex's spawn call returns at once; only its `wait`, which gets no
    /// row, blocks). Empty otherwise, mid-turn or not.
    pub fn blocked_on_tasks(&self) -> impl Iterator<Item = &SubTask> {
        let calls_last = self.items.iter().rev().find(|i| matches!(i, Item::Assistant { .. } | Item::Reasoning { .. } | Item::Tool { .. } | Item::User { .. }));
        let running: Vec<&str> = if self.turn_started.is_some() && self.permissions.is_empty() && matches!(calls_last, Some(Item::Tool { .. })) {
            // The turn's own calls: back to the message that started it.
            self.items
                .iter()
                .rev()
                .take_while(|i| !matches!(i, Item::User { .. }))
                .filter_map(|i| if let Item::Tool { id, status: ToolStatus::Running, .. } = i { Some(id.as_str()) } else { None })
                .collect()
        } else {
            vec![]
        };
        let out = |id: &str| self.tasks.iter().any(|k| k.id == id && k.done.is_none());
        let blocked = !running.is_empty() && running.iter().all(|id| out(id));
        self.tasks.iter().filter(move |k| blocked && k.done.is_none() && running.contains(&k.id.as_str()))
    }

    /// The agent's own sub-agents working in the background (it takes a turn when they report).
    pub fn background_agents(&self) -> impl Iterator<Item = &Background> {
        self.background.iter().filter(|b| b.is_agent())
    }

    /// The rest of its background work: shells, monitors, anything that isn't a sub-agent.
    pub fn background_work(&self) -> impl Iterator<Item = &Background> {
        self.background.iter().filter(|b| !b.is_agent())
    }

    /// Take in the agent's report of what it has running in the background (the whole set).
    fn set_background(&mut self, tasks: Vec<trek_agents::BackgroundTask>) {
        let mut old = std::mem::take(&mut self.background);
        self.background = tasks
            .into_iter()
            .map(|task| match old.iter().position(|b| b.task.id == task.id) {
                Some(ix) => Background { task, ..old.swap_remove(ix) },
                None => Background { task, started: Instant::now(), output: None, output_rev: 0, stopping: false },
            })
            .collect();
    }

    /// Nothing would be cut short by restarting its session: no turn, nothing in the background.
    fn free_to_relaunch(&self) -> bool {
        self.turn_started.is_none() && self.background.is_empty()
    }

    /// The session is gone, and what it had running in the background with it: its sub-agents
    /// that were still out end, failed, and the transcript says what else stopped (the dev server
    /// the user was told to try).
    fn lose_background(&mut self) {
        let work: Vec<&str> = self.background_work().map(|b| b.task.title.as_str()).collect();
        let notice = match work[..] {
            [] => None,
            [one] => Some(format!("{} stopped with the session", one.lines().next().unwrap_or_default().trim().chars().take(80).collect::<String>())),
            _ => Some(format!("{} background tasks stopped with the session", work.len())),
        };
        let out: Vec<String> = self.background_agents().filter_map(|b| b.task.call.clone()).collect();
        for t in self.tasks.iter_mut().filter(|t| t.done.is_none() && out.contains(&t.id)) {
            t.done = Some(false);
            t.ended = Some(Instant::now());
        }
        for ix in 0..self.items.len() {
            if matches!(&self.items[ix], Item::Tool { id, status: ToolStatus::Running, .. } if out.contains(id)) {
                if let Some(Item::Tool { status, .. }) = self.items.get_mut(ix) {
                    *status = ToolStatus::Failed;
                }
            }
        }
        self.background.clear();
        if let Some(text) = notice {
            self.streaming = None;
            self.items.push(Item::Notice { text });
        }
    }

    /// Just loaded, with no turn and nothing in the background: calls its transcript has still
    /// running are an earlier run's, which Trek quit under (the agent's own sub-agents, out in
    /// the background while the thread was answered, aren't closed with turns). They failed, and
    /// the transcript says so. Trek's own sub-agents' rows aside: they end with their threads.
    /// `true` if anything changed.
    fn end_orphaned_calls(&mut self) -> bool {
        if self.turn_started.is_some() || !self.background.is_empty() {
            return false;
        }
        let mut ended = false;
        for ix in 0..self.items.len() {
            let orphan = matches!(&self.items[ix], Item::Tool { id, status: ToolStatus::Running, .. } if trek_core::orchestrate::task_of_row(id).is_none());
            if let (true, Some(Item::Tool { status, .. })) = (orphan, self.items.get_mut(ix)) {
                *status = ToolStatus::Failed;
                ended = true;
            }
        }
        if ended {
            self.items.push(Item::Notice { text: ORPHANS_ENDED.into() });
            self.revision += 1;
        }
        ended
    }

    /// Append the rows of sub-agents started since, each after the agent's own call to
    /// `delegate_task` (transcripts only grow at the end, and the call may reach Trek before the
    /// agent's report of it does). `flush`: the turn is over; whatever waits goes in now.
    pub(crate) fn place_task_rows(&mut self, flush: bool) {
        while !self.pending_rows.is_empty() {
            let calls = self.items.iter().filter(|i| matches!(i, Item::Tool { title, status, .. } if trek_core::orchestrate::is_delegate_call(title) && *status != ToolStatus::Failed)).count();
            let rows = self.items.iter().filter(|i| matches!(i, Item::Tool { id, .. } if trek_core::orchestrate::task_of_row(id).is_some())).count();
            if !flush && calls <= rows {
                return;
            }
            let Some(row) = self.pending_rows.pop_front() else { return };
            self.streaming = None;
            self.reasoning = None;
            self.items.push(row);
            self.revision += 1;
        }
    }

    /// The turn is over: open sub-agents and running tool rows end with it, done or failed. Those
    /// still at work in the background go on: Trek's own (each ends with its thread), and the
    /// agent's that it says run in the background (they end when it says).
    fn close_turn(&mut self, ok: bool) {
        self.place_task_rows(true);
        let out: Vec<String> = self.background_agents().filter_map(|b| b.task.call.clone()).collect();
        for t in self.tasks.iter_mut().filter(|t| t.done.is_none() && !out.contains(&t.id)) {
            t.done = Some(ok);
            t.ended = Some(Instant::now());
        }
        for ix in 0..self.items.len() {
            let ends = matches!(&self.items[ix], Item::Tool { status: ToolStatus::Running, id, .. } if trek_core::orchestrate::task_of_row(id).is_none() && !out.contains(id));
            if let (true, Some(Item::Tool { status, .. })) = (ends, self.items.get_mut(ix)) {
                *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
            }
        }
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
/// The last: the project notes it was given, so a skill set up or changed since starts afresh.
type WarmKey = (AgentId, PathBuf, Option<String>, Effort, HandHolding, bool, bool, Option<String>);

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
    /// The user said a restatement isn't quite right: the composer of `scope` (on `thread`) takes
    /// focus, asking for a restatement of the correction too.
    CorrectRestatement { scope: Scope, thread: String },
    /// Only this thread's transcript changed (streamed text, tool calls). Sent instead of a
    /// notification, so views that don't draw transcripts aren't redrawn for every batch.
    /// `appended`: all that changed is text added to the messages already streaming.
    Transcript { id: String, appended: bool },
    /// Only the output of this thread's background tasks changed (it's read while they run).
    Background { id: String },
    /// What the turn ending at `end` (its `TurnEnd`, by item id) changed is in, or may have moved
    /// (`Workspace::load_turn_changes`).
    TurnChanges { id: String, end: String },
    /// Show the changes of the turn ending at `end` (by item id) of `thread` in the Git tool,
    /// with `path`'s diff open (relative to the repository's top folder).
    ShowTurnDiff { thread: String, end: String, path: Option<String> },
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
    /// Not an undo: the weekly reminder's way to maintain the project's verification skill.
    MaintainVerification(PathBuf),
}

/// How long a restart the user asked for while agents worked waits once they're done: they may
/// be typing the next message by then, and unsent text doesn't survive a restart.
pub const RESTART_GRACE: Duration = Duration::from_secs(10);

/// How long after one alert from a turn an agent took by itself (a watcher or a monitor woke it),
/// the next such turn ends without another.
const WOKE_ALERT_GAP: Duration = Duration::from_secs(10 * 60);

/// How soon after the user stops its sub-agents an agent's turn of its own is taken to be it
/// saying so (Claude Code takes one within seconds).
const STOPPED_ECHO: Duration = Duration::from_secs(30);

/// How long a background task asked to stop shows as stopping before Stop is offered again.
const STOP_WAIT: Duration = Duration::from_secs(10);

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
    /// The agent CLIs' versions and updates asked for.
    pub agent_updates: crate::agent_updates::AgentUpdates,
    pub sidebar_collapsed: bool,
    /// The threads open as tabs in the main window, by id, in order (see `tabs`).
    pub tabs: Vec<String>,
    /// The phone server, while Settings › Phone has it on (see `remote`).
    pub remote: Option<crate::remote::Remote>,
    pub(crate) remote_starting: bool,
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
    /// Threads whose title just changed, with the title before and when: it animates in where
    /// it's shown (`title_reveal`).
    pub retitled: HashMap<String, (String, Instant)>,
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
    /// When Devin was last asked for its plan and quota (`refresh_devin_usage`).
    devin_status_at: i64,
    /// Devin's plan and quota are being read.
    pub devin_loading: bool,
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
    /// A message for the main window's composer while that window is being reopened: a window
    /// opened just now hears no events until later, so `TrekWindow::new` takes it from here.
    pub(crate) pending_compose: Option<(String, String, Vec<PathBuf>)>,
    /// Offer Trek's scripted mock agent (`TREK_MOCK_AGENT=1`, and in tests).
    pub mock_agent: bool,
    /// Where Basecamp goes back to (Esc): the screen it was opened from.
    basecamp_back: Option<Route>,
    /// Composer defaults as last copied into `draft_prefs` (agent, model, effort, hand-holding),
    /// so `save_settings` can tell when the user changed them.
    applied_defaults: (String, Option<String>, Effort, HandHolding),
    tasks: Vec<Task<()>>,
    /// A restart the user asked for while agents worked, counting down now that they're done.
    restart_countdown: Option<Task<()>>,
    /// Why Trek's database couldn't be opened, until the main window has said so: this session
    /// runs on an in-memory copy and nothing is saved.
    pub store_error: Option<String>,
    /// Wall-clock time for usage limits (tests set their own).
    pub clock: Clock,
    /// Wakes when the next usage-limit pause ends (`schedule_limits`).
    pub(crate) limit_timer: Option<Task<()>>,
    /// No resume goes before this (unix ms): just after launch, Trek finds its feet first.
    resumes_from: i64,
    /// Threads whose agent is being asked whether its limit really reset.
    limit_checks: HashSet<String>,
    /// Asks agents for their usage windows before a resume; Claude Code's and Codex's own report
    /// when unset (tests set their own).
    pub usage_probe: Option<limits::UsageProbe>,
    /// Trek's end of its agents' orchestration tools, when its socket could be opened.
    pub(crate) ipc: Option<crate::ipc::IpcServer>,
    _ipc_calls: Option<Task<()>>,
    /// Sub-agents started this run, by thread id (`workspace::orchestrate`).
    pub delegations: HashMap<String, orchestrate::Delegation>,
    /// Sub-agents' reports for parents that haven't heard them yet (kept in the store too).
    wakes: HashMap<String, Vec<trek_core::orchestrate::Report>>,
    /// Reports for parents whose own turn the last quit cut off: the user picks those threads
    /// up, and the reports go out once a turn of theirs has ended (kept in the store too).
    parked: HashMap<String, Vec<trek_core::orchestrate::Report>>,
    /// Parents about to be woken: reports that come in meanwhile go in the same message.
    gathering: HashMap<String, Task<()>>,
    /// The orchestration key of the pre-warmed draft session (`warm`), until it has a thread.
    warm_ipc: Option<String>,
    /// Threads setting up or maintaining a project's verification skill (`workspace::verification`).
    verify_runs: HashMap<String, verification::VerifyRun>,
    /// What finished turns changed (`turn_changes`).
    changes_cache: turn_changes::Cache,
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
        let alone = hold_data_folder();
        // Side chats an earlier run started from a draft can't be reopened: they go before any
        // opens. Their file checkpoints go in the background, with those of threads put away
        // long ago, which only keep objects alive in the user's repos. Not while another Trek
        // has the data folder: its draft side chats are open right now.
        if alone {
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
        }
        // TREK_ONBOARDING=1 replays onboarding without resetting anything (design review, support).
        let replay = std::env::var("TREK_ONBOARDING").is_ok_and(|v| v == "1");
        let mut this = Self::build(store, settings, alone, cx);
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
        if trek_core::update::blocker().is_none() {
            // A first launch has nothing new to show; one after an update shows what it brought
            // (from a build older than "What's new", whose setting is still empty).
            if this.settings.updates.seen_notes.is_empty() {
                this.settings.updates.seen_notes = after_update.clone().unwrap_or_else(|| trek_core::VERSION.into());
                this.save_settings(cx);
            }
            this.fetch_changelog(cx);
        }
        if let Some(from) = after_update {
            tracing::info!("updated from {from} to {}", trek_core::VERSION);
            // Spawned so it lands after the window has subscribed to workspace events.
            let message = format!("Trek updated to {} (from {from}). See what changed under What's new, at the bottom of the sidebar.", trek_core::VERSION);
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            })
            .detach();
        }
        if this.settings.updates.auto_check {
            this.check_for_updates(false, cx);
        }
        this.agent_updates = crate::agent_updates::AgentUpdates::at_launch();
        if this.agent_updates.mock && this.agent_updates.start_at_launch() {
            this.update_all_agents(cx);
        }
        // At every launch, whatever the last check found: a CLI may have been updated (or a
        // new version come out) while Trek was closed. Its cached finds show meanwhile.
        this.check_agent_updates(false, cx);
        this.start_housekeeping(cx);
        // The phone server, if it was on.
        this.sync_remote(cx);
        let keep = this.settings.snapshots.keep_days;
        cx.background_executor().spawn(async move { crate::mentions::prune_snapshots(keep) }).detach();
        // TREK_MOCK_PROMPT starts a mock thread at launch, for performance measurements and demos
        // that can't touch the UI. Only with the mock agent on, and in a scratch folder of its
        // own: in one of the user's repos its turns would leave checkpoint refs behind.
        if let Some(prompt) = std::env::var("TREK_MOCK_PROMPT").ok().filter(|_| this.mock_agent) {
            let scratch = trek_core::paths::data_dir().join("mock-project");
            match std::fs::create_dir_all(&scratch).map_err(anyhow::Error::from).and_then(|_| this.store.ensure_project(&scratch)) {
                Ok(p) => {
                    this.reload(cx);
                    this.route = Route::Draft { project: Some(p.path) };
                    this.draft_prefs.agent = AgentId::Direct(catalog::MOCK_PROVIDER.into());
                    this.draft_prefs.model = None;
                    this.draft_prefs.worktree = false;
                    this.send(prompt, vec![], cx);
                }
                Err(e) => tracing::warn!("mock prompt: {e:#}"),
            }
        }
        this
    }

    /// The model over `store` and `settings` alone: no agent detection, import, update check or
    /// housekeeping is started (`new` adds those). Tests build on this.
    #[cfg(test)]
    pub fn with(store: Store, settings: Settings, cx: &mut Context<Self>) -> Self {
        Self::build(store, settings, true, cx)
    }

    /// `with`, where `alone`: no other Trek is using the data folder (`hold_data_folder`).
    fn build(store: Store, settings: Settings, alone: bool, cx: &mut Context<Self>) -> Self {
        // No session runs yet: turns an earlier run left open (it quit or crashed mid-turn, or
        // with a card up) are over, and saying otherwise would leave them "Working" for good.
        // Unless another Trek has the data folder: those turns may well be its own, running.
        let closed = if alone { store.close_interrupted_turns() } else { Ok(vec![]) };
        let closed = match closed {
            Ok(closed) => {
                if !closed.is_empty() {
                    tracing::info!("turns an earlier run left open, now closed: {}", closed.len());
                }
                closed
            }
            Err(e) => {
                tracing::warn!("close interrupted turns: {e}");
                vec![]
            }
        };
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
            agent_updates: Default::default(),
            sidebar_collapsed: false,
            tabs: if cfg!(test) { vec![] } else { tabs::load_tabs() },
            remote: None,
            remote_starting: false,
            settled_open: false,
            search: String::new(),
            search_results: SearchResults::default(),
            _search_task: None,
            search_epoch: 0,
            reveal: None,
            retitled: HashMap::new(),
            indexing: false,
            index_again: false,
            project_filter: None,
            turns_finished: 0,
            git_info: HashMap::new(),
            agent_status: HashMap::new(),
            agent_commands: HashMap::new(),
            status_fetched_at: 0,
            devin_status_at: 0,
            devin_loading: false,
            acp_info: HashMap::new(),
            usage_loading: false,
            settings_project: None,
            warm: None,
            checking_merges: false,
            tidied_at: 0,
            overlay_open: false,
            main_window: None,
            thread_windows: HashMap::new(),
            pending_compose: None,
            mock_agent: trek_agents::mock::enabled(),
            basecamp_back: None,
            applied_defaults,
            tasks: vec![],
            restart_countdown: None,
            store_error: None,
            clock: Clock::default(),
            limit_timer: None,
            resumes_from: 0,
            limit_checks: HashSet::new(),
            usage_probe: None,
            ipc: None,
            _ipc_calls: None,
            delegations: HashMap::new(),
            wakes: HashMap::new(),
            parked: HashMap::new(),
            gathering: HashMap::new(),
            warm_ipc: None,
            verify_runs: HashMap::new(),
            changes_cache: Default::default(),
        };
        this.reload(cx);
        this.start_ipc(cx);
        // Sub-agents' reports that hadn't reached their parents: they're delivered now. Not while
        // another Trek has the data folder: they may well be on their way there.
        if alone {
            this.restore_wakes(&closed, cx);
        }
        this.refresh_all_verification(cx);
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
            // The socket goes with the process.
            this.ipc = None;
            async {}
        })
        .detach();
        this.resume_overdue(cx);
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
        self.hand_back_queued(id, cx);
        cx.notify();
    }

    /// Learn Claude Code's commands and skills in `cwd` (a thread window's folder; the status
    /// check covers the main window's), unless they're known. Sends no prompt.
    fn fetch_claude_commands(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let key = (AgentId::ClaudeCode.key(), cwd);
        let ready = self.agents.iter().any(|a| a.agent == AgentId::ClaudeCode && a.availability == Availability::Ready);
        if !ready || trek_core::paths::isolated() || self.settings.disabled_agents.contains(&key.0) || self.agent_commands.contains_key(&key) {
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
        self.keep(task);
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

    /// Keep `task` running for as long as the workspace lives; finished ones are let go here, so
    /// the list doesn't grow with every navigation and turn.
    fn keep(&mut self, task: Task<()>) {
        self.tasks.retain(|t| !t.is_ready());
        self.tasks.push(task);
    }

    /// The transcript column's widest at the current text size (`md::column`): the transcript,
    /// its cards, the working bar and the composer line up on it.
    pub fn column(&self) -> gpui_kit::Pixels {
        crate::md::column(gpui_kit::px(self.settings.appearance.transcript_font_size()))
    }

    /// Motion is allowed: neither Trek's Reduce motion setting nor the system asks for less.
    pub fn motion(&self, cx: &App) -> bool {
        !self.settings.appearance.reduce_motion && !cx.reduce_motion()
    }

    /// How far `id`'s new title has animated in (0 to 1), and the title it replaces, while it does.
    pub fn title_reveal(&self, id: &str) -> Option<(f32, &str)> {
        let (old, at) = self.retitled.get(id)?;
        let t = at.elapsed().as_secs_f32() / TITLE_REVEAL.as_secs_f32();
        (t < 1.).then_some((t, old.as_str()))
    }

    fn mutate_thread(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Thread)) {
        let motion = self.motion(cx);
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == id) {
            let before = t.title.clone();
            f(t);
            if t.title != before && motion {
                self.retitled.retain(|_, (_, at)| at.elapsed() < TITLE_REVEAL);
                self.retitled.insert(id.to_string(), (before, Instant::now()));
            }
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
        self.sync_remote(cx);
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
        let waiting = self.waiting_threads();
        let q = self.search.trim().to_lowercase();
        let mut map: HashMap<Section, Vec<&Thread>> = HashMap::new();
        for t in &self.threads {
            if !q.is_empty() && !self.search_results.matches(&q, t) {
                continue;
            }
            if let Some(p) = &self.project_filter {
                // `NO_PROJECT` keeps the threads that belong to none.
                if t.project_id.as_ref() != Some(p) && !(p == NO_PROJECT && t.project_id.is_none()) {
                    continue;
                }
            }
            // A search finds sub-agents too, though the inbox lists them only in their parent.
            let section = if q.is_empty() { t.section(now) } else { t.own_section(now) };
            // Waiting on its sub-agents, a thread is still at work.
            let section = match section {
                Some(Section::Inbox) if waiting.contains(&t.id) => Some(Section::Working),
                s => s,
            };
            if let Some(s) = section {
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
    /// thread that asks again is back in the inbox, so it counts too. Archived ones don't, nor do
    /// side chats: the inbox doesn't list them, so nothing there could settle them. Sub-agents
    /// aren't listed either: one waiting on an approval counts as its parent's card, which says
    /// so (a failed one reports to its parent instead).
    pub fn needs_you_count(&self) -> usize {
        let sub_agents = self.waiting_on_sub_agents();
        self.threads
            .iter()
            .filter(|t| t.parent_id.is_none() && t.archived_at.is_none() && t.side_of.is_none() && (t.needs_you() || sub_agents.contains(&t.id)))
            .count()
    }

    /// Follow-ups waiting for the running turn of `id` to finish (`FollowUp::Queue`).
    pub fn queued(&self, id: &str) -> usize {
        self.live.get(id).map_or(0, |l| l.queued.len())
    }

    /// An agent is at work in this process: a live turn, or sub-agents of an agent's own working
    /// in the background (Trek's run turns of their own). Threads left marked Working by an
    /// earlier run that quit mid-turn don't count, nor do shells and monitors left running: a dev
    /// server can run all day, and whatever it reports wakes its agent into a turn.
    pub fn any_turn_running(&self) -> bool {
        self.threads.iter().any(|t| t.run_state == RunState::Working && self.live.get(&t.id).is_some_and(|l| l.turn_started.is_some()))
            || self.live.values().any(|l| l.commands.is_some() && l.background_agents().next().is_some())
    }

    /// Ask `thread`'s agent for the end of background task `task`'s output (it arrives as a
    /// `WorkspaceEvent::Background`). Nothing happens when its session is gone.
    pub fn read_background(&mut self, thread: &str, task: &str) {
        let Some(live) = self.live.get(thread) else { return };
        if live.background.iter().any(|b| b.task.id == task && b.task.readable) {
            if let Some(tx) = &live.commands {
                let _ = tx.try_send(Command::ReadTask { id: task.to_string() });
            }
        }
    }

    /// Stop background task `task` of `thread` (the agent says when it has). Still running
    /// `STOP_WAIT` later (the agent couldn't stop it, and said so in the transcript), it can be
    /// asked again.
    pub fn stop_background(&mut self, thread: &str, task: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(thread) else { return };
        let Some(b) = live.background.iter_mut().find(|b| b.task.id == task && b.task.stoppable && !b.stopping) else { return };
        b.stopping = true;
        if let Some(tx) = &live.commands {
            let _ = tx.try_send(Command::StopTask { id: task.to_string() });
        }
        let (id, task) = (thread.to_string(), task.to_string());
        let again = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STOP_WAIT).await;
            let _ = this.update(cx, |ws, cx| {
                if let Some(b) = ws.live.get_mut(&id).and_then(|l| l.background.iter_mut().find(|b| b.task.id == task && b.stopping)) {
                    b.stopping = false;
                    cx.emit(WorkspaceEvent::Background { id: id.clone() });
                    cx.notify();
                }
            });
        });
        self.keep(again);
        cx.notify();
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
        self.keep(task);
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
        self.keep(task);
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Thread(id) = &route {
            let id = id.clone();
            self.open_tab(&id);
            self.mutate_thread(&id, cx, |t| t.last_seen_at = now_ms().max(t.updated_at));
            self.ensure_loaded(&id, cx);
        }
        if let Route::Draft { project: Some(p) } = &route {
            if self.route != route {
                self.apply_project_defaults(&p.clone());
                // Its first session is told about the project's verification skill as it is now.
                self.refresh_verification(&p.clone(), cx);
            }
        }
        if route == Route::Settings(SettingsPage::Project) && self.route != route {
            // Shown as last recorded until the look through every project is back.
            self.refresh_all_verification(cx);
        }
        if route == Route::Basecamp && self.route != Route::Basecamp {
            self.basecamp_back = Some(self.route.clone()).filter(|r| !matches!(r, Route::Onboarding));
            // Plan limits for its "Left on" tiles (asked at most every 30 seconds).
            self.refresh_usage(cx);
            self.refresh_devin_usage(cx);
        }
        if route == Route::Settings(SettingsPage::Agents) && self.route != route {
            // Devin's plan for its account line, as Claude Code's and Codex's have theirs.
            self.refresh_devin_usage(cx);
        }
        self.route = route;
        self.refresh_git(cx);
        cx.emit(WorkspaceEvent::FocusComposer);
        if let Route::Thread(id) = self.route.clone() {
            self.hand_back_queued(&id, cx);
        }
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
        self.keep(task);
    }

    // ---------- projects: preferences ----------

    pub fn project_prefs(&self, path: &std::path::Path) -> trek_core::settings::ProjectPrefs {
        self.settings.projects.get(&path.display().to_string()).cloned().unwrap_or_default()
    }

    /// The icon and colour chosen for a project folder, if any.
    pub fn project_look(&self, path: &std::path::Path) -> crate::ui::ProjectLook {
        self.settings.projects.get(&path.display().to_string()).map(crate::ui::ProjectLook::of).unwrap_or_default()
    }

    /// The look of the project a thread belongs to.
    pub fn thread_project_look(&self, t: &Thread) -> crate::ui::ProjectLook {
        let project = t.project_id.as_ref().and_then(|pid| self.projects.iter().find(|p| &p.id == pid));
        project.map(|p| self.project_look(&p.path)).unwrap_or_default()
    }

    /// The colour the folders of a thread's project are tinted with (`ui::project_tint`).
    pub fn thread_project_tint(&self, t: &Thread, cx: &App) -> Option<gpui_kit::Hsla> {
        let pid = t.project_id.as_ref()?;
        let p = self.projects.iter().find(|p| &p.id == pid)?;
        Some(crate::ui::project_tint(&p.name, &self.project_look(&p.path), cx))
    }

    /// The folder tint of the project in the project folder `path`.
    pub fn project_tint_at(&self, path: &std::path::Path, cx: &App) -> Option<gpui_kit::Hsla> {
        let p = self.projects.iter().find(|p| p.path == path)?;
        Some(crate::ui::project_tint(&p.name, &self.project_look(&p.path), cx))
    }

    /// The folder tint of the project on screen: the thread's, or the draft's.
    pub fn current_project_tint(&self, cx: &App) -> Option<gpui_kit::Hsla> {
        match &self.route {
            Route::Thread(id) => self.thread(id).and_then(|t| self.thread_project_tint(t, cx)),
            Route::Draft { project: Some(path) } => self.project_tint_at(path, cx),
            _ => None,
        }
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
        let current = self
            .current_thread()
            .and_then(|t| t.project_id.clone())
            .or_else(|| self.current_cwd().and_then(|c| self.projects.iter().find(|p| p.path == trek_core::store::project_root(&c)).map(|p| p.id.clone())));
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
            // From a thread without a project, or a draft without one, the next has none either.
            Route::Thread(id) if self.thread(id).is_some_and(|t| t.project_id.is_none() && t.cwd.as_deref().is_some_and(trek_core::paths::is_chat_dir)) => None,
            Route::Draft { project: None } => None,
            Route::Thread(id) => self.thread(id).and_then(|t| self.draft_folder(t)).or_else(|| self.workspace_projects().first().map(|p| p.path.clone())),
            Route::Draft { project } => project.clone(),
            _ => self.workspace_projects().first().map(|p| p.path.clone()),
        };
        self.navigate(Route::Draft { project }, cx);
    }

    /// Leave Basecamp for the screen it was opened from (a thread that's gone since: a new one).
    pub fn leave_basecamp(&mut self, cx: &mut Context<Self>) {
        match self.basecamp_back.take() {
            Some(Route::Thread(id)) if self.thread(&id).is_none() => self.new_thread(cx),
            Some(route) => self.navigate(route, cx),
            None => self.new_thread(cx),
        }
    }

    /// Threads waiting on the user and finished ones they haven't looked at, for Basecamp's
    /// "Ready for review": what needs them first, then newest first. A thread whose sub-agent
    /// waits on an approval needs them, working or not.
    pub fn ready_for_review(&self) -> Vec<&Thread> {
        let now = now_ms();
        let sub_agents = self.waiting_on_sub_agents();
        let needs = |t: &Thread| t.needs_you() || sub_agents.contains(&t.id);
        // Still waiting on its own sub-agents, a thread isn't ready yet.
        let waiting = self.waiting_threads();
        let mut out: Vec<&Thread> = self
            .threads
            .iter()
            .filter(|t| match t.section(now) {
                Some(_) if sub_agents.contains(&t.id) => true,
                Some(Section::Inbox | Section::Pinned) => t.run_state != RunState::Working && !waiting.contains(&t.id) && (t.needs_you() || t.is_unseen()),
                _ => false,
            })
            .collect();
        out.sort_by_key(|t| (!needs(t), std::cmp::Reverse(t.updated_at)));
        out
    }

    /// What a thread waiting on the user is waiting for, while its session holds the request.
    pub fn pending_request(&self, id: &str) -> Option<&PendingPermission> {
        self.live.get(id)?.permissions.first()
    }

    /// Mark what "Ready for review" lists read (Basecamp's "Mark all read"). What needs the
    /// user stays.
    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        let unread: Vec<String> = self.ready_for_review().into_iter().filter(|t| t.is_unseen()).map(|t| t.id.clone()).collect();
        for id in unread {
            self.mutate_thread(&id, cx, |t| t.last_seen_at = now.max(t.updated_at));
        }
    }

    pub(crate) fn ensure_loaded(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.live.get(id).is_none_or(|l| l.spend.is_none()) {
            let spend = self.load_spend(id);
            self.live.entry(id.to_string()).or_default().spend = Some(spend);
        }
        let thread = self.thread(id).cloned();
        let live = self.live.entry(id.to_string()).or_default();
        if live.loaded || live.loading {
            return;
        }
        live.checkpointed = self.store.checkpoints(id).unwrap_or_default().into_iter().map(|c| c.item_id).collect();
        live.lines = self.store.tool_lines(id).unwrap_or_default();
        let rows = self.store.items_with_ids(id).unwrap_or_default();
        if !rows.is_empty() {
            live.items = Transcript::stored(rows);
            // Turns saved before Trek marked their ends have none: the last one gets its end
            // (and with it the footer that copies, undoes, retries and forks it).
            if matches!(live.items.last(), Some(Item::Assistant { .. })) {
                live.items.push(Item::TurnEnd { at: thread.as_ref().map_or_else(now_ms, |t| t.updated_at), took_secs: 0 });
            }
            live.loaded = true;
            live.revision += 1;
            if live.end_orphaned_calls() {
                self.persist_items(id, cx);
            }
            self.settle_cut_off_rows(id, cx);
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
                this.settle_cut_off_rows(&id, cx);
                // Messages sent while it loaded go out now, after the history; then reports from
                // its sub-agents.
                this.send_queued(&id, cx);
                this.deliver_wakes(&id, cx);
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
        self.keep(task);
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
                let was = before.as_ref().map(|b| (b.agent.clone(), b.model.clone()));
                self.mutate_thread(&id, cx, |t| {
                    t.model = prefs.model.clone();
                    t.effort = prefs.effort;
                    t.hand_holding = prefs.hand_holding;
                    // Switching agent on an existing thread forks it into a new session.
                    if t.agent != prefs.agent {
                        t.agent = prefs.agent.clone();
                        t.native_id = None;
                        // A rewind's way back into the old agent's session is no way into the new
                        // one's: it starts from a recap of what the transcript keeps.
                        t.reopen = None;
                    }
                });
                let fast_tier = self.fast_tier(&prefs.agent, prefs.model.as_ref(), prefs.fast);
                let live = self.live.entry(id.clone()).or_default();
                let switched = before.as_ref().is_some_and(|b| b.agent != prefs.agent);
                // Another agent means another login and a fresh session: how the old one was
                // billed says nothing about the new one, and the old agent's questions and plans
                // go with it, as does a turn it was running. What it spent stays the thread's.
                let mut cut = false;
                if switched {
                    live.billing = None;
                    live.permissions.clear();
                    live.picks.clear();
                    if live.turn_started.take().is_some() {
                        live.close_turn(false);
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Notice { text: "Interrupted".into() });
                        live.revision += 1;
                        cut = true;
                    }
                    // The old agent's session goes, and what it ran in the background with it.
                    if !live.background.is_empty() {
                        live.lose_background();
                        live.revision += 1;
                        cut = true;
                    }
                }
                let fast_changed = live.fast != prefs.fast;
                let plan_changed = live.plan != prefs.plan;
                live.plan = prefs.plan;
                live.fast = prefs.fast;
                if let (Some(before), Some(tx)) = (before, live.commands.clone()) {
                    // Claude reads effort, fast mode and plan at launch: restart idle sessions (they
                    // resume), and running ones once their turn is over. One with work running in
                    // the background can't restart (it would end that work, and its sub-agents
                    // would never report): it's told the new settings as it runs.
                    let at_launch = fast_changed || plan_changed || (prefs.agent == AgentId::ClaudeCode && before.effort != prefs.effort);
                    let relaunch = at_launch && live.free_to_relaunch();
                    let in_place = at_launch && !relaunch && live.turn_started.is_none();
                    live.relaunch |= at_launch && !relaunch && !in_place;
                    if in_place && before.agent == prefs.agent {
                        let _ = tx.try_send(Command::SetModes { plan: prefs.plan, fast: fast_tier, effort: prefs.effort });
                    }
                    if before.agent != prefs.agent || relaunch {
                        let _ = tx.try_send(Command::Shutdown);
                        live.commands = None;
                        // What it says from here on (its exit above all) isn't the thread's news.
                        live._events = None;
                        live.relaunch = false;
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
                // Nothing is left running or waiting on the user (a plan offered after its turn
                // went with the old agent), unless its worktree is still being made.
                if switched && !self.live.get(&id).is_some_and(|l| l.preparing) {
                    if cut {
                        self.persist_items(&id, cx);
                    }
                    self.mutate_thread(&id, cx, |t| {
                        if matches!(t.run_state, RunState::Working | RunState::NeedsYou) {
                            t.run_state = RunState::Idle;
                        }
                    });
                }
                self.approve_covered_prompts(&id, prefs.hand_holding, cx);
                if let Some((agent, model)) = was {
                    // Another agent picks the conversation up from a recap: the transcript says
                    // so, and a usage limit on the old one holds nothing back any more. Within one
                    // agent, only another model gets past a limit on a model.
                    let past_limit = if agent != prefs.agent {
                        if self.note_handoff(&id, (&agent, model), (&prefs.agent, prefs.model.clone())) {
                            self.persist_items(&id, cx);
                        }
                        true
                    } else {
                        model != prefs.model && self.pause(&id).is_some_and(|p| matches!(p.scope, trek_agents::LimitScope::Model(_)))
                    };
                    if past_limit && self.pause(&id).is_some() {
                        self.end_pause(&id, false, cx);
                        // Reports from sub-agents held while it was paused go to the new agent now,
                        // ahead of whatever the user sends next.
                        self.deliver_wakes(&id, cx);
                    }
                }
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

    /// Liquid glass's tint, when it's on and no window-wide image covers the desktop (and macOS
    /// isn't set to reduce transparency).
    pub fn glass(&self) -> Option<f32> {
        if self.backdrop().is_some() || (!cfg!(test) && crate::system::reduce_transparency()) {
            return None;
        }
        self.settings.appearance.glass_tint()
    }

    /// The window's chrome is see-through: over the backdrop image, or the desktop through glass.
    pub fn see_through(&self) -> bool {
        self.backdrop().is_some() || self.glass().is_some()
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
        self.keep(task);
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
            out.push(AgentId::Direct(catalog::MOCK_RELAY_PROVIDER.into()));
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
        // Trek's own commands are answered in a draft too, without starting a thread for them.
        if matches!(self.route, Route::Draft { .. }) {
            if let Some(reply) = self.run_builtin_command(None, &text, cx) {
                if !reply.is_empty() {
                    cx.emit(WorkspaceEvent::Toast { message: reply.replace("**", "").replace('`', ""), undo: None });
                }
                return;
            }
        }
        let id = match self.route.clone() {
            Route::Thread(id) => id,
            Route::Draft { project } => {
                // No project: the thread gets a folder of its own to work in, and belongs to none.
                let chat = project.is_none();
                let cwd = match project {
                    Some(cwd) => cwd,
                    None => match trek_core::paths::new_chat_dir() {
                        Ok(dir) => dir,
                        Err(e) => {
                            cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't make a folder for the thread: {e}"), undo: None });
                            return;
                        }
                    },
                };
                let p = self.draft_prefs.clone();
                // A worktree of its own: its branch and folder are picked now, and it's made in
                // the background while the message waits. Other threads' worktrees are taken,
                // made yet or not.
                let taken: Vec<_> = self.threads.iter().filter_map(|t| t.worktree.clone()).collect();
                let planned = match (p.worktree && !chat).then(|| trek_core::worktree::plan(&trek_core::worktree::worktrees_dir(), &cwd, &text, &taken)) {
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
                let mut thread = match self.store.create_thread((!chat).then_some(cwd.as_path()), p.agent, p.model, p.effort, p.hand_holding) {
                    Ok(t) => t,
                    Err(e) => {
                        cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't create thread: {e}"), undo: None });
                        return;
                    }
                };
                thread.title = trek_core::import_title(&text);
                if chat {
                    thread.cwd = Some(cwd.clone());
                }
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
                // The draft's tab becomes the thread's.
                self.open_tab(&id);
                self.route = Route::Thread(id.clone());
                match planned {
                    Some(wt) => self.make_worktree(&id, cwd, wt, cx),
                    // Use the session that was started while the message was being typed, if it still fits.
                    None => {
                        let key = self.draft_key(&cwd);
                        match self.warm.take() {
                            Some((k, handle, _)) if k == key => {
                                // A new session, given the project notes the key says.
                                let notes = k.7.filter(|_| !self.thread(&id).is_some_and(|t| trek_agents::notes_in_system_prompt(&t.agent)));
                                self.live.entry(id.clone()).or_default().notes_pending = notes;
                                self.attach(&id, handle, cx);
                                let ipc = self.warm_ipc.take();
                                self.adopt_ipc_session(&id, ipc);
                            }
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
            // Held for an agent update: sent when it's done (`agent_released`). Follow-ups a
            // failed turn left queued ahead of it aren't.
            if self.thread(&id).is_some_and(|t| self.agent_updating(&t.agent.key())) && !self.agent_updates.held.contains_key(&id) {
                let left = if self.stale_queue(&id) && !self.holds_queue(&id) { self.queued(&id) } else { 0 };
                self.agent_updates.held.insert(id.clone(), left);
            }
            let live = self.live.entry(id).or_default();
            live.queued.push((text, images));
            live.revision += 1;
            cx.notify();
            return;
        }
        if let Some(reply) = self.run_builtin_command(Some(&id), &text, cx) {
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
        // Paused at a usage limit: it goes when the limit resets.
        let (text, images) = match running {
            false => match self.send_while_paused(&id, text, images, cx) {
                Some(message) => message,
                None => return,
            },
            true => (text, images),
        };
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
            live.turn_error = false;
            live.stopped_out = None;
            live.quiet_turn = false;
            live.self_started = false;
            // The agent's own sub-agents still out in the background carry on into this turn.
            live.tasks.retain(|t| t.done.is_none());
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

    /// The agent restated the request and got it right: it goes ahead, with whatever else the
    /// request asked for (a consult, an arena) that waited for the restatement.
    pub fn go_ahead(&mut self, id: &str, cx: &mut Context<Self>) {
        let users = self.live.get(id).map(|l| l.items.iter().rev().filter_map(|i| if let Item::User { text, aside: false, .. } = i { Some(text.as_str()) } else { None }).collect::<Vec<_>>()).unwrap_or_default();
        let text = trek_core::restate::go_ahead_after(users);
        self.send_to(id, text, vec![], cx);
    }

    /// Messages to `id` wait: its history is still being read, its worktree is being made,
    /// removed or has gone missing, or its agent's CLI is being updated.
    fn holds_messages(&self, id: &str) -> bool {
        self.holds_queue(id) || self.thread(id).is_some_and(|t| self.agent_updating(&t.agent.key()))
    }

    /// What's queued for `id` waits to go: its history is still being read, its worktree is
    /// being made, removed or has gone missing, or it was sent while its agent's CLI updated.
    /// Otherwise follow-ups queued while it has no turn were left by one that failed or stopped.
    fn holds_queue(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| l.loading || l.preparing || l.removing)
            || self.thread(id).is_some_and(|t| t.worktree.as_ref().is_some_and(|w| w.is_missing()))
            || self.agent_updates.held.contains_key(id)
    }

    /// Send what was queued while nothing ran (a thread whose history was loading): the first
    /// message starts a turn, and the rest wait for it as queued follow-ups do.
    fn send_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        while !self.holds_messages(id) && self.live.get(id).is_some_and(|l| l.turn_started.is_none()) {
            let Some((text, images)) = self.live.get_mut(id).and_then(|l| (!l.queued.is_empty()).then(|| l.queued.remove(0))) else { break };
            self.send_to(id, text, images, cx);
        }
        // Ready to go but for an agent update that started meanwhile (its worktree or history
        // came in while it ran): held for it, so they go once it's done (`agent_released`).
        if self.live.get(id).is_some_and(|l| !l.queued.is_empty() && l.turn_started.is_none()) && self.thread(id).is_some_and(|t| self.agent_updating(&t.agent.key())) {
            self.agent_updates.held.entry(id.to_string()).or_insert(0);
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
        if self.live.get(id).is_some_and(|l| l.commands.is_some() || l.preparing || l.removing) {
            return;
        }
        // No folder to run in until the worktree is back (or the thread moves to the project's),
        // and no CLI to run while it's being updated.
        if thread.worktree.as_ref().is_some_and(|w| w.is_missing()) || self.agent_updating(&thread.agent.key()) {
            return;
        }
        let (plan, fast_on) = self.live.get(id).map(|l| (l.plan, l.fast)).unwrap_or_default();
        // After a rewind or a fork the session picks up from part of one, or from a recap.
        let recap = || self.live.get(id).map(|l| trek_core::rewind::recap(&l.items)).filter(|r| !r.is_empty());
        let (resume, resume_at, fork, recap) = match thread.reopen.clone() {
            Some(Reopen::Native { session, at, fork }) => (Some(session), at, fork, recap()),
            Some(Reopen::Recap) => (None, None, false, recap()),
            // No session of the agent's own to go back to (direct models keep none, and a thread
            // that changed agent left its old one): the conversation so far comes as a recap.
            None if thread.native_id.is_none() => (None, None, false, recap()),
            None => (thread.native_id.clone(), None, false, None),
        };
        let (mcp_servers, ipc) = self.session_mcp(&thread.agent, Some(id));
        // What the project's verification skill is now (made outside Trek, or since the last look).
        let project = self.project_dir(&thread);
        if let Some(p) = &project {
            self.refresh_verification(p, cx);
        }
        let cwd = thread.cwd.clone().unwrap_or_else(trek_core::paths::home);
        let told = resume.is_some().then(|| self.store.told_notes(id).ok().flatten()).flatten();
        // A sub-agent that only advises, and a side chat, change nothing: they aren't asked for a recap.
        let recaps = !self.advising(id) && thread.side_of.is_none();
        let instructions = verification::notes_to_give(self.session_notes(project.as_deref(), &cwd, recaps), &thread.agent, resume.is_some(), told.as_deref());
        self.live.entry(id.to_string()).or_default().notes_pending = instructions.clone().filter(|_| !trek_agents::notes_in_system_prompt(&thread.agent));
        let handle = start_session(SessionConfig {
            agent: thread.agent.clone(),
            cwd,
            model: thread.model.clone(),
            effort: thread.effort,
            hand_holding: thread.hand_holding,
            plan,
            // A sub-agent that only advises can't change anything.
            read_only: self.advising(id),
            resume,
            resume_at,
            fork,
            recap,
            fast: self.fast_tier(&thread.agent, thread.model.as_ref(), fast_on),
            mcp_servers,
            instructions,
            read_dirs: vec![trek_core::skills::shipped_root()],
        });
        self.attach(id, handle, cx);
        self.adopt_ipc_session(id, ipc);
    }

    /// Wire a running session to a thread: commands go out, events come back in batches.
    fn attach(&mut self, id: &str, handle: trek_agents::SessionHandle, cx: &mut Context<Self>) {
        let live = self.live.entry(id.to_string()).or_default();
        live.commands = Some(handle.commands);
        live.relaunch = false;
        live.last_active = Some(cx.background_executor().now());
        // A new process says how it's billed again (the login may have changed since).
        live.billing = None;
        live.session_gen += 1;
        let session = live.session_gen;
        let events = handle.events;
        let id = id.to_string();
        live._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                // Batch everything already queued so a burst of tokens is one update.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let applied = this.update(cx, |this, cx| {
                    // Another session has taken this one's place (it was let finish, see
                    // `end_session`): its last words, its exit above all, aren't the thread's.
                    if this.live.get(&id).is_some_and(|l| l.session_gen == session) {
                        this.apply_events(&id, batch, cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
                // Cap UI updates at ~60 Hz while streaming.
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
        }));
    }

    fn draft_key(&self, cwd: &std::path::Path) -> WarmKey {
        let p = &self.draft_prefs;
        (p.agent.clone(), cwd.to_path_buf(), p.model.clone(), p.effort, p.hand_holding, p.plan, p.fast, self.session_notes(Some(cwd), cwd, true))
    }

    /// What a new session is told besides its messages (`SessionConfig::instructions`): the
    /// project's notes, and the recap Trek asks for at the end of work while that's on and the
    /// session `recaps` (it can change things).
    pub(crate) fn session_notes(&self, project: Option<&std::path::Path>, cwd: &std::path::Path, recaps: bool) -> Option<String> {
        let recap = (recaps && self.settings.general.ask_recap).then(|| trek_core::changes::RECAP.to_string());
        let notes: Vec<String> = self.project_notes(project, cwd).into_iter().chain(recap).collect();
        (!notes.is_empty()).then(|| notes.join("\n\n"))
    }

    /// Start the agent before the first message is sent (called when the user begins typing), so
    /// the process, its login check and its MCP servers are ready by the time they hit Return.
    /// Costs nothing with the provider: no request is made until a prompt is sent.
    pub fn warm_up(&mut self, cx: &mut Context<Self>) {
        match self.route.clone() {
            Route::Thread(id) => self.warm_thread(&id, cx),
            Route::Draft { project: Some(cwd) } => {
                // A thread in a worktree starts its agent there, once the worktree is made.
                if matches!(self.draft_prefs.agent, AgentId::Direct(_)) || self.draft_prefs.worktree || self.agent_updating(&self.draft_prefs.agent.key()) {
                    return;
                }
                if self.warm.as_ref().is_some_and(|(k, _, _)| *k == self.draft_key(&cwd)) {
                    return;
                }
                let p = self.draft_prefs.clone();
                let (mcp_servers, ipc) = self.session_mcp(&p.agent, None);
                if let (Some(old), Some(server)) = (std::mem::replace(&mut self.warm_ipc, ipc), &self.ipc) {
                    server.close_session(&old);
                }
                self.refresh_verification(&cwd, cx);
                let key = self.draft_key(&cwd);
                let instructions = key.7.clone();
                let handle = start_session(SessionConfig {
                    agent: p.agent.clone(),
                    cwd,
                    model: p.model.clone(),
                    effort: p.effort,
                    hand_holding: p.hand_holding,
                    plan: p.plan,
                    read_only: false,
                    resume: None,
                    resume_at: None,
                    fork: false,
                    recap: None,
                    fast: self.fast_tier(&p.agent, p.model.as_ref(), p.fast),
                    mcp_servers,
                    instructions,
                    read_dirs: vec![trek_core::skills::shipped_root()],
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
        // An advising sub-agent's requests are declined before anyone sees them.
        let events = self.screen_advice(id, events);
        // Background tasks' output is read while it runs (polled): it goes to whatever shows it,
        // and changes nothing else.
        let (outputs, events): (Vec<AgentEvent>, Vec<AgentEvent>) = events.into_iter().partition(|e| matches!(e, AgentEvent::TaskOutput { .. }));
        if !outputs.is_empty() {
            if let Some(live) = self.live.get_mut(id) {
                for ev in outputs {
                    if let AgentEvent::TaskOutput { id: task, output } = ev
                        && let Some(b) = live.background.iter_mut().find(|b| b.task.id == task)
                    {
                        if b.output.as_ref() != Some(&output) {
                            b.output = Some(output);
                            b.output_rev += 1;
                        }
                    }
                }
            }
            cx.emit(WorkspaceEvent::Background { id: id.to_string() });
        }
        if events.is_empty() {
            return;
        }
        // The session ended: its orchestration key goes.
        let exited = events.iter().any(|e| matches!(e, AgentEvent::Exited));
        let mut run_state: Option<RunState> = None;
        let mut native: Option<String> = None;
        // A session started (reported its id): whatever a rewind or fork asked of it is done.
        let mut started = false;
        // It's another session than the thread's: its points so far are unknown.
        let mut new_session = false;
        let current_native = self.thread(id).and_then(|t| t.native_id.clone());
        // The group of tool calls on show in the working bar, which these events may end.
        let live_group = self.live_group_start(id);
        let mut diff: Option<(i64, i64)> = None;
        let mut finished = false;
        // The turn ended cleanly: queued follow-ups may go out. After a stop or failure they go back
        // to the composer instead, so the user can rethink them.
        let mut continue_queue = false;
        let mut notify_text: Option<String> = None;
        // The user stopped the turn: nothing to tell them.
        let mut interrupted = false;
        // The turn ended at a usage limit: the thread pauses until it resets.
        let mut hit: Option<limits::Hit> = None;
        // The agent stopped to ask the user something.
        let mut asked = false;
        let mut commands: Option<Vec<SlashCommand>> = None;
        // Tokens the agent reported (by the model it named) and their cost, kept once the batch
        // is through.
        let mut used: Vec<(Option<String>, trek_core::TokenUsage, Option<trek_core::UsageCost>)> = vec![];
        // A turn that failed (true) or was stopped: transcripts keep no turn end for it, but the
        // time it ran counts in Basecamp.
        let mut stopped: Option<(u32, bool)> = None;
        // Streamed text and tool calls change nothing but the transcript; mostly they only extend
        // the messages already streaming.
        let mut transcript_only = true;
        let mut appended = true;
        let mut turn_began: Option<i64>;
        // The agent's own sub-agents all ended with no turn of its own open: their reports went
        // nowhere yet (the agent may still take a turn for them), or the session took them.
        let agents_gone;
        let mut agents_lost = false;
        {
            let live = self.live.entry(id.to_string()).or_default();
            let had_agents = live.background_agents().next().is_some();
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
                    live.quiet_turn = live.stopped_out.is_some_and(|at| at.elapsed() < STOPPED_ECHO);
                    live.self_started = true;
                    live.turn_started = Some(Instant::now());
                    run_state = Some(RunState::Working);
                    transcript_only = false;
                }
                transcript_only &= matches!(
                    ev,
                    AgentEvent::TextDelta(_)
                        | AgentEvent::TextDone(_)
                        | AgentEvent::ReasoningDelta(_)
                        | AgentEvent::ToolStarted { .. }
                        | AgentEvent::ToolFinished { .. }
                        | AgentEvent::ToolLines { .. }
                        | AgentEvent::TaskStep { .. }
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
                                live.tasks.push(SubTask {
                                    id: tid.clone(),
                                    description: d.clone(),
                                    activity: String::new(),
                                    steps: vec![],
                                    stepped: 0,
                                    tool_uses: 0,
                                    done: None,
                                    started: Instant::now(),
                                    ended: None,
                                });
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
                                task.ended.get_or_insert_with(Instant::now);
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
                    AgentEvent::TaskStep { task, title, detail } => {
                        if let Some(t) = live.tasks.iter_mut().find(|t| t.id == task) {
                            t.steps.push((title, detail));
                            if t.steps.len() > SUB_STEPS {
                                t.steps.remove(0);
                            }
                            t.stepped += 1;
                        }
                    }
                    AgentEvent::Background(tasks) => {
                        live.set_background(tasks);
                    }
                    // Taken out above.
                    AgentEvent::TaskOutput { .. } => {}
                    // One row says it, in place of the error the turn ends with.
                    AgentEvent::LimitReached { message, resets_at, scope } => {
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Limit { text: message.clone(), resets_at, scope: scope.clone() });
                        live.limit = Some(limits::Hit { message, resets_at, scope });
                    }
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
                    AgentEvent::Started { native_id, model } => {
                        started = true;
                        live.session_model = model.or(live.session_model.take());
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
                        // A sub-agent row waiting for this call goes right after it, before any
                        // text later in the batch opens a new message.
                        live.place_task_rows(false);
                    }
                    AgentEvent::ToolLines { id: tid, added, removed } => {
                        let lines = (added > 0 || removed > 0).then_some((added, removed));
                        if let Err(e) = self.store.set_tool_lines(id, &tid, lines) {
                            tracing::warn!("save tool lines: {e}");
                        }
                        match lines {
                            Some(l) => live.lines.insert(tid, l),
                            None => live.lines.remove(&tid),
                        };
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
                    AgentEvent::Usage { model, tokens, cost } => used.push((model, tokens, cost)),
                    AgentEvent::Billing(b) => live.billing = Some(b),
                    AgentEvent::TurnComplete { error } => {
                        // Models that hide their reasoning leave empty "Thought" rows behind. Removing
                        // them shifts positions; the rows' ids keep saves and views lined up.
                        live.items.retain(|i| !matches!(i, Item::Reasoning { text } if text.trim().is_empty()));
                        live.streaming = None;
                        live.reasoning = None;
                        live.permissions.clear();
                        // The turn is over, whatever it left running in the background: its answer
                        // is in. What it waits on (its sub-agents) keeps it among the working
                        // (`Workspace::waiting`); a shell, a monitor or a browser left running doesn't.
                        let took = live.turn_started.take().map(|t| t.elapsed().as_secs() as u32).unwrap_or(0);
                        live.turn_error = false;
                        live.close_turn(error.is_none());
                        if error.is_none() && matches!(live.items.last(), Some(Item::Assistant { .. })) {
                            live.items.push(Item::TurnEnd { at: now_ms(), took_secs: took });
                        }
                        if let Some(e) = &error {
                            // Paused at a usage limit isn't failed, but its time on the trail counts.
                            stopped = Some((took, e != "Interrupted" && live.limit.is_none()));
                        }
                        if let Some(h) = live.limit.take() {
                            // Paused, not failed: its limit row says why.
                            hit = Some(h);
                            run_state = Some(RunState::Idle);
                            continue_queue = false;
                        } else if let Some(e) = error {
                            live.resuming = None;
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
                            live.resuming = None;
                            run_state = Some(RunState::Idle);
                            continue_queue = true;
                        }
                        finished = true;
                    }
                    AgentEvent::Mark(_) => {}
                    // Shown, and that's all: agents report trouble that ends nothing this way too
                    // (an image they couldn't read, a model change they turned down). A turn that
                    // fails says so as it completes, or its session ends (`Exited`).
                    AgentEvent::Error(e) => {
                        live.streaming = None;
                        live.reasoning = None;
                        live.items.push(Item::Error { text: e });
                        live.turn_error |= live.turn_started.is_some();
                    }
                    AgentEvent::Exited => {
                        continue_queue = false;
                        live.commands = None;
                        // What it ran in the background went with it.
                        agents_lost |= live.turn_started.is_none() && live.background_agents().next().is_some();
                        live.lose_background();
                        // Nobody is left to answer what the agent was asking.
                        live.permissions.retain(|p| p.after_turn);
                        live.picks.retain(|(rid, _), _| live.permissions.iter().any(|p| p.request_id == *rid));
                        // The process ended mid-turn: the turn failed, and ends here like any other
                        // (saved, queued follow-ups handed back, an alert). Unless the error it
                        // reported says why, that it stopped is all there is to say.
                        if let Some(began) = live.turn_started.take() {
                            stopped = Some((began.elapsed().as_secs() as u32, live.limit.is_none()));
                            live.close_turn(false);
                            live.streaming = None;
                            live.reasoning = None;
                            if let Some(h) = live.limit.take() {
                                hit = Some(h);
                                run_state = Some(RunState::Idle);
                            } else {
                                if !std::mem::take(&mut live.turn_error) {
                                    live.items.push(Item::Error { text: "The agent stopped unexpectedly.".into() });
                                }
                                run_state.get_or_insert(RunState::Failed);
                            }
                            finished = true;
                        }
                    }
                }
            }
            live.place_task_rows(false);
            live.revision += 1;
            agents_gone = had_agents && !exited && live.turn_started.is_none() && live.background_agents().next().is_none();
        }
        if agents_lost {
            self.background_agents_lost(id, cx);
        } else if agents_gone {
            self.background_agents_gone(id, cx);
        }
        if let Some((took, failed)) = stopped
            && let Err(e) = self.store.record_stop(id, now_ms(), took, failed)
        {
            tracing::warn!("record stopped turn: {e:#}");
        }
        if !used.is_empty() {
            self.record_usage(id, used);
        }
        if exited {
            self.retire_ipc_session(id);
        }
        let mark = self.live.get(id).and_then(|l| l.mark.clone());
        if let (Some(c), Some(t)) = (commands, self.thread(id)) {
            let key = (t.agent.key(), t.cwd.clone().unwrap_or_else(trek_core::paths::home));
            self.agent_commands.insert(key, c);
        }
        // Before anything hands queued messages back: they wait for the reset now.
        let paused = hit.is_some();
        if let Some(hit) = hit {
            self.pause_at_limit(id, hit, cx);
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
        // Settings read at launch changed while it was busy: now it isn't, it restarts (and
        // resumes) before anything more goes to it. Unless it has work running in the background:
        // then it's told them instead.
        if let Some(live) = self.live.get_mut(id).filter(|l| l.relaunch && l.turn_started.is_none()) {
            live.relaunch = false;
            let (free, plan, fast) = (live.free_to_relaunch(), live.plan, live.fast);
            if free {
                if let Some(tx) = live.commands.take() {
                    let _ = tx.try_send(Command::Shutdown);
                }
            } else if let (Some(tx), Some(t)) = (live.commands.clone(), self.thread(id)) {
                let fast = self.fast_tier(&t.agent, t.model.as_ref(), fast);
                let _ = tx.try_send(Command::SetModes { plan, fast, effort: t.effort });
            }
        }
        // Its background work over between turns, an agent update held back for it may start.
        if !finished && self.live.get(id).is_some_and(|l| l.free_to_relaunch()) {
            self.pump_agent_updates(cx);
        }
        if finished {
            self.turns_finished += 1;
            // The turn before is counted against the files as they are, or waits on this one.
            self.forget_turn_changes(id, false, cx);
            self.refresh_git(cx);
            if let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()).filter(|c| Some(c) != self.current_cwd().as_ref()) {
                self.refresh_git_at(cwd, cx);
            }
            self.persist_items(id, cx);
            self.search_index_changed(cx);
            self.note_branch(id, turn_began, cx);
            if self.live.get_mut(id).is_some_and(|l| std::mem::take(&mut l.hold_queue)) {
                continue_queue = false;
            }
            let next = if continue_queue {
                self.live.get_mut(id).and_then(|l| (!l.queued.is_empty()).then(|| l.queued.remove(0)))
            } else {
                None
            };
            if !continue_queue && viewing {
                self.restore_queued(id, cx);
            }
            // A sub-agent reports how its turn ended; a parent stopped by the user isn't woken.
            self.task_turn_ended(id, interrupted, cx);
            self.verification_turn_ended(id, !interrupted && !paused, cx);
            if interrupted {
                self.forget_wakes(id);
            }
            if let Some((text, images)) = next {
                // The thread keeps going with the user's queued follow-up: not "finished" yet.
                self.send_to(id, text, images, cx);
            } else {
                self.maybe_auto_title(id, cx);
                // Waiting on its sub-agents, it isn't done: it says so when it is.
                let waiting = self.waiting(id) || self.wakes.get(id).is_some_and(|w| !w.is_empty());
                // The agent saying that what the user stopped has stopped isn't news either. Nor is
                // every turn a watcher or a monitor wakes it for: one alert a while is enough,
                // unless it failed.
                let (echo, woke) = self.live.get_mut(id).map_or((false, false), |l| (std::mem::take(&mut l.quiet_turn), std::mem::take(&mut l.self_started)));
                let woke_again = woke && self.live.get(id).and_then(|l| l.woke_alert).is_some_and(|at| at.elapsed() < WOKE_ALERT_GAP);
                let failed = self.thread(id).is_some_and(|t| t.run_state == RunState::Failed);
                if let Some(t) = self.thread(id).filter(|_| !interrupted && !paused && !waiting && !echo && (failed || !woke_again)) {
                    let verb = if failed { "Failed" } else { "Finished" };
                    notify_text.get_or_insert(format!("{verb}: {}", t.title));
                    if let Some(l) = self.live.get_mut(id).filter(|_| woke) {
                        l.woke_alert = Some(Instant::now());
                    }
                }
                self.maybe_restart_for_update(cx);
            }
            // An agent update held back for this turn may start now.
            self.pump_agent_updates(cx);
        }
        // Picked up again after a quit cut its turn off, it hears what was kept for it then.
        if finished && !interrupted && !paused {
            self.unpark_wakes(id);
        }
        // Free now (its turn over, an answer settled, a request withdrawn), it hears from its
        // sub-agents that reported meanwhile.
        if self.wakes.contains_key(id) {
            self.deliver_wakes(id, cx);
        }
        // A side chat answers in the panel it was asked in, beside its thread; the inbox doesn't
        // list it, so an alert would lead nowhere. A sub-agent's end goes to its parent, not the
        // user; only its requests for approval are theirs.
        let side_chat = self.thread(id).is_some_and(|t| t.side_of.is_some() || (t.parent_id.is_some() && !asked));
        if let Some(message) = notify_text.filter(|_| !side_chat) {
            cx.emit(WorkspaceEvent::Attention { message, thread: id.to_string() });
        }
        if let Some(first) = live_group {
            // Text, the turn's end or a stop ended it (not a card, which shows it as it is): it
            // folds into its summary row.
            let card = self.live.get(id).is_some_and(|l| !l.permissions.is_empty());
            if self.live_group_start(id).as_ref() != Some(&first) && !card && self.motion(cx) {
                self.fold_group(id, first, cx);
            }
        }
        if transcript_only {
            // Only the transcript views redraw; the sidebar, title bar and composer would
            // otherwise redraw with every batch, up to 60 times a second while text streams.
            cx.emit(WorkspaceEvent::Transcript { id: id.to_string(), appended });
        } else {
            cx.notify();
        }
    }

    /// The id of the first item of `id`'s live group of tool calls (`activity::live`).
    fn live_group_start(&self, id: &str) -> Option<String> {
        let start = crate::activity::live(self, id)?.start;
        self.live.get(id)?.items.id_at(start).map(str::to_string)
    }

    /// Show `id`'s live group in the transcript, open (and its call `call` too), rather than in
    /// the working bar: every call can be read there, output and all, while the turn goes on.
    pub fn open_live_group(&mut self, id: &str, call: Option<String>, cx: &mut Context<Self>) {
        let Some(first) = self.live_group_start(id) else { return };
        let Some(live) = self.live.get_mut(id) else { return };
        live.opened = Some(crate::activity::Opened { first, call });
        cx.emit(WorkspaceEvent::Transcript { id: id.to_string(), appended: false });
    }

    /// The live group starting at item `first` is over: the working bar folds it away while the
    /// transcript holds it (and what came after) back, then shows it as a summary row.
    fn fold_group(&mut self, id: &str, first: String, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id) else { return };
        live.fold = Some(crate::activity::Fold { first, until: Instant::now() + crate::activity::FOLD });
        let id = id.to_string();
        live._fold_done = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::activity::FOLD).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(l) = this.live.get_mut(&id) {
                    l.fold = None;
                }
                cx.emit(WorkspaceEvent::Transcript { id, appended: false });
            });
        }));
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

    /// Follow-ups held for a turn that stopped or failed while `id` was off screen go back to the
    /// composer now that it's on screen. Left queued, they'd go out after some later turn, long
    /// after the turn they were written for.
    pub(crate) fn hand_back_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.on_screen(id) {
            return;
        }
        if let Some(left) = self.live.get_mut(id).map(|l| std::mem::take(&mut l.left_over)).filter(|l| !l.is_empty()) {
            let text = left.iter().map(|(t, _)| t.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
            let images = left.into_iter().flat_map(|(_, images)| images).collect();
            cx.emit(WorkspaceEvent::RestoreQueued { thread: id.to_string(), text, images });
        }
        // While its agent updates too: only messages sent meanwhile are held for that.
        if self.stale_queue(id) && !self.holds_queue(id) {
            self.restore_queued(id, cx);
        }
    }

    /// `id` has follow-ups queued with no turn of its own, nor its agent's sub-agents, to follow.
    fn stale_queue(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| !l.queued.is_empty() && l.turn_started.is_none() && l.background_agents().next().is_none())
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
        // it then leaves the thread idle unless it starts new work, free for reports from its
        // sub-agents that waited.
        if !still_waiting {
            let state = if running { RunState::Working } else { RunState::Idle };
            self.mutate_thread(id, cx, |t| t.run_state = state);
            if !running {
                self.deliver_wakes(id, cx);
            }
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
        // A plan offered after its turn needs a session to take its approval: none starts while
        // the agent's CLI is being updated, so the plan stays offered until it's done.
        let after_turn = self.live.get(id).is_some_and(|l| l.permissions.iter().any(|p| p.request_id == request_id && p.after_turn));
        if let Some(agent) = self.thread(id).map(|t| t.agent.clone()).filter(|a| after_turn && self.agent_updating(&a.key())) {
            let message = format!("{} is updating. Approve the plan once it's done.", agent.display_name());
            cx.emit(WorkspaceEvent::Toast { message, undo: None });
            return;
        }
        let Some(live) = self.live.get_mut(id) else { return };
        live.plan = false;
        // Offered after its turn ended (Codex), the plan's approval starts a new turn. If the
        // session that offered it has gone (restarted, or quit), a new one, out of plan mode,
        // takes the approval: the agent still has the plan in the thread's history.
        if live.permissions.iter().any(|p| p.request_id == request_id && p.after_turn) {
            self.ensure_session(id, cx);
            if let Some(live) = self.live.get_mut(id).filter(|l| l.commands.is_some()) {
                live.turn_started = Some(Instant::now());
                live.tasks.retain(|t| t.done.is_none());
            }
        }
        self.respond(id, request_id, Decision::Allow, cx);
    }

    pub fn interrupt(&mut self, id: &str, cx: &mut Context<Self>) {
        // A sub-agent the user stops between turns, while it waits on sub-agents of its own, has
        // no turn whose end would report that: it reports now, stopped. (Stopped from above, it
        // ends silently, `stop_task`.)
        if self.live.get(id).is_some_and(|l| l.turn_started.is_none()) && self.delegations.get(id).is_some_and(|d| d.outcome.is_none() && !d.is_cancelled()) {
            self.task_turn_ended(id, true, cx);
        }
        // Its sub-agents stop with it.
        self.stop_children(id, cx);
        // Between turns, waiting on its agent's own sub-agents: they stop (there's no turn to
        // interrupt them with), and nothing that was due wakes it.
        if self.live.get(id).is_some_and(|l| l.turn_started.is_none()) {
            let agents: Vec<String> = self.live.get(id).map(|l| l.background_agents().filter(|b| b.task.stoppable).map(|b| b.task.id.clone()).collect()).unwrap_or_default();
            if let Some(live) = self.live.get_mut(id).filter(|_| !agents.is_empty()) {
                live.stopped_out = Some(Instant::now());
            }
            for task in agents {
                self.stop_background(id, &task, cx);
            }
            self.forget_wakes(id);
        }
        let Some(live) = self.live.get_mut(id) else { return };
        // A stop never waits behind git work. If the message is still held for its checkpoint,
        // the agent never gets it: the turn ends here.
        if live.held.iter().any(|c| matches!(c, Command::Prompt { .. })) {
            live.held.retain(|c| !matches!(c, Command::Prompt { .. }));
            self.apply_events(id, vec![AgentEvent::TurnComplete { error: Some("Interrupted".into()) }], cx);
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
            let prompt = matches!(cmd, Command::Prompt { .. });
            let _ = tx.try_send(cmd);
            if prompt {
                self.notes_told(id);
            }
        }
    }

    /// A message reached `id`'s session: the project notes it had to give went with it. Not
    /// before: one a stop kept from the agent (`interrupt`) took nothing with it.
    fn notes_told(&mut self, id: &str) {
        if let Some(notes) = self.live.get_mut(id).and_then(|l| l.notes_pending.take()) {
            let _ = self.store.set_told_notes(id, &notes);
        }
    }

    /// Do `id`'s git work one job at a time, off the main thread; what was held for it goes to
    /// the agent once it's all done.
    fn run_git(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.get_mut(id).filter(|l| !l.git_busy) else { return };
        let Some(job) = live.git_jobs.pop_front() else {
            let held = std::mem::take(&mut live.held);
            let Some(tx) = &live.commands else { return };
            let prompt = held.iter().any(|c| matches!(c, Command::Prompt { .. }));
            for cmd in held {
                let _ = tx.try_send(cmd);
            }
            if prompt {
                self.notes_told(id);
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
        // A checkpoint ends the latest turn's count; restored files move it.
        let files_moved = matches!(job, GitJob::Checkpoint { .. } | GitJob::Restore { .. });
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
        if files_moved {
            self.forget_turn_changes(id, false, cx);
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

    /// A turn is under way on `id` (working, or waiting on the user). Sub-agents still out after
    /// it don't count: the thread is free for a message (`waiting` says it waits on them).
    pub fn turn_running(&self, id: &str) -> bool {
        self.live.get(id).is_some_and(|l| l.turn_started.is_some())
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
        live.background.clear();
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
        self.forget_turn_changes(id, true, cx);
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
        // Asked for now, by hand: no waiting for a usage limit's reset.
        self.end_pause(id, false, cx);
        self.send_to(id, text, images, cx);
    }

    /// Send an edited message in place of message `item`: the conversation is taken back to just
    /// before it (see `rewind`), then the new text goes out. False when nothing was done.
    pub fn edit_and_resend(&mut self, id: &str, item: &str, text: String, images: Vec<PathBuf>, restore: bool, cx: &mut Context<Self>) -> bool {
        if self.rewind(id, item, restore, cx).is_none() {
            return false;
        }
        self.end_pause(id, false, cx);
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
            self.keep(task);
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
        let kept_lines: HashMap<String, (u32, u32)> = kept
            .iter()
            .filter_map(|i| match i {
                Item::Tool { id: call, .. } => live.lines.get(call).map(|l| (call.clone(), *l)),
                _ => None,
            })
            .collect();
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
        for (call, lines) in &kept_lines {
            if let Err(e) = self.store.set_tool_lines(&fork.id, call, Some(*lines)) {
                tracing::warn!("save tool lines: {e}");
            }
        }
        let fork_id = fork.id.clone();
        let live = self.live.entry(fork_id.clone()).or_default();
        live.items = transcript;
        live.lines = kept_lines;
        live.loaded = true;
        live.mark = fork.native_at.clone();
        live.git_jobs.extend(links.into_iter().map(|(repo, checkpoints)| GitJob::Link { repo, checkpoints }));
        self.reload(cx);
        match scope {
            Scope::Main => self.navigate(Route::Thread(fork_id.clone()), cx),
            Scope::Thread(_) => self.show_in_main(Route::Thread(fork_id.clone()), cx),
        }
        if let Some((text, images)) = message {
            // A main window reopened for the fork can't hear it yet: it takes the message itself.
            if self.main_window.is_some() {
                cx.emit(WorkspaceEvent::ComposeIn { scope: Scope::Main, thread: fork_id.clone(), text, images, edit: None });
            } else {
                self.pending_compose = Some((fork_id.clone(), text, images));
            }
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

    /// Out of every list until it's unarchived. Its agent stops: nothing could answer it once
    /// the thread is out of sight, and nothing would save what it does. A turn under way ends
    /// here, saved as it stands.
    pub fn archive(&mut self, id: &str, cx: &mut Context<Self>) {
        self.archive_children(id, cx);
        let mut cut = false;
        if let Some(live) = self.live.get_mut(id) {
            if let Some(tx) = live.commands.take() {
                let _ = tx.try_send(Command::Shutdown);
            }
            live._events = None;
            live.held.clear();
            live.queued.clear();
            live.permissions.clear();
            live.picks.clear();
            if live.turn_started.take().is_some() {
                live.close_turn(false);
                live.streaming = None;
                live.reasoning = None;
                live.items.push(Item::Notice { text: "Interrupted".into() });
                cut = true;
            }
            if !live.background.is_empty() {
                live.lose_background();
                cut = true;
            }
            live.revision += 1;
        }
        if cut {
            self.persist_items(id, cx);
        }
        self.mutate_thread(id, cx, |t| {
            t.archived_at = Some(now_ms());
            if matches!(t.run_state, RunState::Working | RunState::NeedsYou) {
                t.run_state = RunState::Idle;
            }
        });
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
        // Its sub-agents go too, theirs as well.
        let children = self.drop_children(id, cx);
        let direct: Vec<String> = children.iter().filter(|c| c.parent_id.as_deref() == Some(id)).map(|c| c.id.clone()).collect();
        threads.extend(children);
        // A thread in no project takes its folder to the Trash (it's the thread's own), unless a
        // thread that stays (a fork) still works in it.
        let gone: HashSet<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        let chat_dirs: Vec<PathBuf> = threads
            .iter()
            .filter_map(|t| t.cwd.clone().filter(|c| trek_core::paths::is_chat_dir(c) && c != &trek_core::paths::chats_dir()))
            .filter(|c| !self.threads.iter().any(|o| !gone.contains(o.id.as_str()) && o.cwd.as_ref() == Some(c)))
            .collect();
        for dir in chat_dirs.into_iter().filter(|d| d.exists()) {
            if let Err(e) = crate::system::trash(&dir) {
                tracing::warn!("trash {}: {e:#}", dir.display());
            }
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
            // It stays (archived); its sub-agents, Trek's own, don't.
            direct
                .iter()
                .try_for_each(|c| self.store.delete_thread(c))
                .and_then(|_| self.store.delete_checkpoints(id, &items).map(|_| ()))
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
                    // As written: what Trek adds for the agent (a consult, a restatement) isn't the user's words.
                    let text = trek_core::restate::as_written(&text);
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
                Item::Limit { text, .. } => out.push_str(&format!("\n> **Usage limit:** {}\n", text.trim().replace('\n', "\n> "))),
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
            Item::User { text, .. } if !text.trim().is_empty() => Some(trek_core::restate::as_written(text).to_string()),
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
        // Not while Claude Code's CLI is being replaced: written once it's back.
        if self.agent_updating(&AgentId::ClaudeCode.key()) {
            self.agent_updates.titles.retain(|(t, _)| t != id);
            self.agent_updates.titles.push((id.to_string(), announce));
            return;
        }
        let Some((request, reply)) = self.title_inputs(id) else { return };
        // The mock agent names its own threads: it makes no model calls, titles included.
        if self.thread(id).is_some_and(|t| matches!(&t.agent, AgentId::Direct(p) if catalog::is_mock(p))) {
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
        self.keep(task);
    }

    /// After a new Trek thread's first answer, replace the truncated first message with a real title.
    fn maybe_auto_title(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.settings.general.auto_title {
            return;
        }
        let Some(t) = self.thread(id) else { return };
        // Sub-agents are named by the agent that started them.
        if t.source != ThreadSource::Trek || t.side_of.is_some() || t.parent_id.is_some() {
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
                self.unarchive_children(&id);
                self.reload(cx);
            }
            // Not under a running turn: the agent's edits since would be lost under it.
            UndoAction::Unrestore { thread, .. } if self.turn_running(&thread) => {
                cx.emit(WorkspaceEvent::Toast { message: "Stop the running turn first.".into(), undo: None });
            }
            // Not for a thread deleted since: its refs are gone.
            UndoAction::Unrestore { thread, repo, sha } if self.thread(&thread).is_some() => {
                self.live.entry(thread.clone()).or_default().git_jobs.push_back(GitJob::Restore { repo, sha });
                self.run_git(&thread, cx);
            }
            UndoAction::Unrestore { .. } => {}
            UndoAction::MaintainVerification(project) => self.start_verification(project, true, cx),
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
                this.maybe_check_agent_updates(cx);
                this.maybe_restart_for_update(cx);
                this.remind_verification(now, cx);
                // No redraw otherwise: the views that show times keep their own clocks.
            });
            if alive.is_err() {
                break;
            }
        });
        self.keep(task);
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
            // Not while it runs anything in the background: the session takes that with it.
            let quiet = live.turn_started.is_none() && live.background.is_empty() && live.permissions.is_empty();
            if quiet && live.commands.is_some() && idle_for(live.last_active, 15) && !shown.contains(id) {
                if let Some(tx) = live.commands.take() {
                    let _ = tx.try_send(Command::Shutdown);
                }
            }
        }
        if self.warm.as_ref().is_some_and(|(_, _, at)| idle_for(Some(*at), 10)) {
            self.warm = None;
            if let (Some(key), Some(ipc)) = (self.warm_ipc.take(), &self.ipc) {
                ipc.close_session(&key);
            }
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
        self.keep(task);
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
        self.keep(task);
    }

    // ---------- discovery ----------

    /// Look for installed agents, then ask the ones found about their models, logins and usage.
    /// Not in an isolated (test) process: that would run the user's own CLIs.
    pub fn detect_agents(&mut self, cx: &mut Context<Self>) {
        if self.detecting || trek_core::paths::isolated() {
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
                    // Opened on something that shows Devin's plan before Devin was found.
                    if matches!(this.route, Route::Basecamp | Route::Settings(SettingsPage::Agents)) || std::env::var_os("TREK_OPEN_USAGE").is_some() {
                        this.refresh_devin_usage(cx);
                    }
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
        self.keep(task);
    }

    /// Re-read account, plan, usage limits and commands from the installed vendor CLIs. Free:
    /// no prompt is sent. Throttled to once every 30 seconds. Devin is asked on its own
    /// (`refresh_devin_usage`).
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_loading || now_ms() - self.status_fetched_at < 30_000 || trek_core::paths::isolated() {
            return;
        }
        // Not of a CLI being replaced: asked again once it's back (`agent_released`).
        let ready = |a: AgentId| self.agent_ready(&a) && !self.agent_updating(&a.key());
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
                this.fill_unknown_resets(cx);
                cx.notify();
            });
        });
        self.keep(task);
        cx.notify();
    }

    /// Re-read Devin's plan and quota. Devin shows them only in its terminal UI, which takes a
    /// few seconds to run and leaves Devin a session lock each time, so it's asked only when
    /// something shows them (the Usage popover, Basecamp, Settings → Agents, a Devin thread
    /// paused at its limit), at most every ten minutes.
    pub fn refresh_devin_usage(&mut self, cx: &mut Context<Self>) {
        if self.devin_loading
            || now_ms() - self.devin_status_at < DEVIN_STATUS_EVERY
            || trek_core::paths::isolated()
            || !self.agent_ready(&devin_agent())
            || self.agent_updating(&devin_agent().key())
        {
            return;
        }
        self.devin_loading = true;
        self.devin_status_at = now_ms();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_agents::devin_status().await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            let Ok(res) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| this.apply_devin_status(res, cx));
        });
        self.keep(task);
        cx.notify();
    }

    /// Take in what Devin said of its plan and quota. It says nothing of its commands, which
    /// its ACP sessions report per folder and stay as they were.
    pub(crate) fn apply_devin_status(&mut self, res: anyhow::Result<AgentStatus>, cx: &mut Context<Self>) {
        let key = devin_agent().key();
        match res {
            Ok(st) => {
                self.agent_status.insert(key, st);
            }
            // Without a status of its own, Settings keeps what Devin's ACP probe said of its login.
            Err(e) => {
                if let Some(st) = self.agent_status.get_mut(&key) {
                    st.error = Some(e.to_string());
                }
            }
        }
        self.devin_loading = false;
        self.fill_unknown_resets(cx);
        cx.notify();
    }

    /// Installed, signed in as far as Trek knows, and not turned off in Settings.
    fn agent_ready(&self, id: &AgentId) -> bool {
        self.agents.iter().any(|a| a.agent == *id && a.availability == Availability::Ready) && !self.settings.disabled_agents.contains(&id.key())
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
        // Probing starts the agent and opens a session in its history: not for agents the user
        // turned off, nor from an isolated (test) process.
        if trek_core::paths::isolated() {
            return;
        }
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
            self.keep(task);
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

    /// Answer one of Trek's own commands (`BUILTIN_COMMANDS`, `/clear`) in thread `id`, or in the
    /// draft (`None`). `None` when `text` isn't one Trek answers there: it goes to the agent.
    fn run_builtin_command(&mut self, id: Option<&str>, text: &str, cx: &mut Context<Self>) -> Option<String> {
        let cmd = text.strip_prefix('/')?.split_whitespace().next()?;
        let thread = match id {
            Some(id) => Some(self.thread(id)?.clone()),
            None => None,
        };
        let draft_project = match &self.route {
            Route::Draft { project } => project.clone(),
            _ => None,
        };
        let (agent, model) = match &thread {
            Some(t) => (t.agent.clone(), t.model.clone()),
            None => (self.draft_prefs.agent.clone(), self.draft_prefs.model.clone()),
        };
        let live = id.and_then(|id| self.live.get(id));
        match cmd {
            "permissions" | "access" | "mode" => {
                let arg = text.split_whitespace().nth(1).map(str::to_string);
                Some(self.permissions_command(id, arg.as_deref(), cx))
            }
            "clear" | "new" => {
                let project = match &thread {
                    Some(t) => self.draft_folder(t),
                    None => draft_project,
                };
                self.navigate(Route::Draft { project }, cx);
                Some(String::new())
            }
            "usage" => {
                self.status_fetched_at = 0;
                self.refresh_usage(cx);
                if agent == devin_agent() {
                    self.refresh_devin_usage(cx);
                }
                let Some(st) = self.agent_status.get(&agent.key()) else {
                    // In a thread the agent may answer it itself; a draft has no agent to ask.
                    return thread.is_none().then(|| format!("{} hasn't reported its usage yet.", agent.display_name()));
                };
                let mut lines = vec![format!("**{}** · {}", agent.display_name(), st.plan.clone().unwrap_or_else(|| "no plan reported".into()))];
                for l in &st.limits {
                    lines.push(format!("- {}: {:.0}% used{}", l.label, l.percent, l.resets_at.map(|r| format!(", resets {}", crate::time::until(r))).unwrap_or_default()));
                }
                lines.extend(st.note.clone());
                Some(lines.join("\n"))
            }
            "context" => match live.and_then(|l| l.context) {
                Some((used, window)) => Some(format!("{} of {} tokens in context ({:.0}%).", fmt_tokens(used), fmt_tokens(window), used as f64 / window.max(1) as f64 * 100.)),
                None => thread.is_none().then(|| "Nothing is in context yet: this thread hasn't started.".to_string()),
            },
            "cost" => match &thread {
                None => Some("Nothing is spent yet: this thread hasn't started.".into()),
                Some(t) => {
                    let spend = self.spend_of(&t.id);
                    Some(cost_reply(self.billing_of(t).as_ref(), &spend))
                }
            },
            "model" => Some(format!("{} · {}", agent.display_name(), model.unwrap_or_else(|| "default model".into()))),
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
                    out.push(McpServer { name: format!("trek-{family}"), command: bin.clone(), args: vec![family.into()], env: vec![], tool_timeout_secs: None });
                }
            }
        }
        for s in tools.mcp_servers.iter().filter(|s| s.enabled) {
            out.push(McpServer { name: s.name.clone(), command: s.command.clone(), args: s.args.clone(), env: vec![], tool_timeout_secs: None });
        }
        out
    }

    fn fetch_codex_models(&mut self, cx: &mut Context<Self>) {
        if trek_core::paths::isolated() {
            return;
        }
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
        self.keep(task);
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
        self.keep(task);
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

    pub fn run_update_action(&mut self, action: UpdateAction, cx: &mut Context<Self>) {
        match action {
            UpdateAction::Check => self.check_for_updates(true, cx),
            UpdateAction::Download => self.download_update(cx),
            UpdateAction::Restart => self.restart_to_update(cx),
            // The user's call: what's running stops.
            UpdateAction::RestartNow => {
                if let UpdateStatus::RestartPending { staged, .. } = self.updater.status.clone() {
                    self.restart_countdown = None;
                    self.install_update(staged, cx);
                }
            }
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
                let before = this.updater.offer.as_ref().map(|o| o.version.clone());
                if this.updater.finish_check(ticket, result, user_initiated, auto_download, now_ms()) {
                    this.download_update(cx);
                }
                let found = this.updater.offer.as_ref().map(|o| o.version.clone());
                if found.is_some() && found != before {
                    this.fetch_changelog(cx);
                }
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Read the published release notes (`trek_core::changelog`) for "What's new" and the release
    /// history; an update that was found reads them again, as they now include it.
    pub fn fetch_changelog(&mut self, cx: &mut Context<Self>) {
        let updates = self.settings.updates.clone();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let _ = tx.send(trek_core::changelog::fetch(&updates).await).await;
        });
        let task = cx.spawn(async move |this, cx| {
            match rx.recv().await {
                Ok(Ok(releases)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.updater.changelog = releases;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => tracing::info!("release notes: {e:#}"),
                Err(_) => {}
            }
        });
        self.keep(task);
    }

    /// What this version brought, until the user has seen it: only after an update, and only for
    /// a release (a dev build has no notes of its own).
    pub fn whats_new(&self) -> Option<&trek_core::changelog::Release> {
        if trek_core::update::blocker().is_some() || self.settings.updates.seen_notes == trek_core::VERSION {
            return None;
        }
        self.updater.installed_release().filter(|r| !r.notes.is_empty())
    }

    pub fn mark_whats_new_seen(&mut self, cx: &mut Context<Self>) {
        if self.settings.updates.seen_notes != trek_core::VERSION {
            self.settings.updates.seen_notes = trek_core::VERSION.into();
            self.save_settings(cx);
            cx.notify();
        }
    }

    /// Release notes for the update on offer: every release since this one.
    pub fn pending_changes(&self) -> Option<crate::updater::Changes> {
        self.updater.pending_changes(self.settings.updates.channel, trek_core::changelog::github_repo(&self.settings.updates).as_deref())
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
        self.keep(progress_task);
        self.keep(task);
    }

    /// Forget the update in flight or waiting (the channel changed): its download stops, late
    /// results are ignored, and the staged copy is deleted rather than installed on quit.
    fn drop_update(&mut self, cx: &mut Context<Self>) {
        if let Some(staged) = self.updater.drop_update() {
            cx.background_executor().spawn(async move { trek_core::update::discard(&staged) }).detach();
        }
    }

    /// What holds an update's restart back: agent turns and Trek's own work on files. An agent's
    /// background sub-agents don't: they can run for an hour after their turn is over, and the
    /// restart notice says it stops them, as it does shells.
    pub fn work_in_flight(&self) -> bool {
        let update_due = self.agent_updates.queued_where(|a| self.agent_hold(a) != Some(Hold::Background));
        self.agent_updates.running() || update_due || self.live.values().any(|l| {
            (l.commands.is_some() && l.turn_started.is_some())
                || l.permissions.iter().any(|p| p.after_turn)
                || l.preparing
                || l.loading
                || l.removing
                || l.git_busy
                || !l.queued.is_empty()
                || !l.left_over.is_empty()
                || !l.held.is_empty()
        })
    }

    /// Install now unless agent turns are under way (`work_in_flight`); otherwise once they're over.
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
        // Shells left running don't hold the update back (a dev server can run all day), but the
        // restart ends them: the toast says so while it can still be cancelled.
        let work: Vec<&str> = self.live.values().filter(|l| l.commands.is_some()).flat_map(|l| l.background.iter().map(|b| b.task.title.as_str())).collect();
        let message = restart_message(RESTART_GRACE.as_secs(), &work);
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

/// The toast before an update restart, naming the background work it will end.
fn restart_message(secs: u64, work: &[&str]) -> String {
    let name = |t: &str| t.lines().next().unwrap_or_default().trim().chars().take(60).collect::<String>();
    match work {
        [] => format!("Trek restarts to update in {secs} seconds."),
        [one] => format!("Trek restarts to update in {secs} seconds, stopping {}.", name(one)),
        _ => format!("Trek restarts to update in {secs} seconds, stopping {} background tasks.", work.len()),
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

/// Start an agent session. In an isolated (test) process only the mock agent runs: any other
/// would be one of the user's own CLIs, signed in to their account, writing to their history.
/// Live tests ask for a real one (`TREK_LIVE_AGENT`).
fn start_session(config: SessionConfig) -> trek_agents::SessionHandle {
    let mock = matches!(&config.agent, AgentId::Direct(p) if catalog::is_mock(p));
    if mock || !trek_core::paths::isolated() || std::env::var_os("TREK_LIVE_AGENT").is_some() {
        return trek_agents::start(config);
    }
    let (commands, _) = async_channel::unbounded();
    let (tx, events) = async_channel::unbounded();
    let _ = tx.try_send(AgentEvent::Error(format!("{} isn't started in tests.", config.agent.display_name())));
    let _ = tx.try_send(AgentEvent::Exited);
    trek_agents::SessionHandle { commands, events }
}

/// Lock `dir` for this process (an advisory `flock` on a file in it, released when the process
/// ends): `Ok(Some(lock))` to keep while the folder is ours, `Ok(None)` when another process
/// holds it.
fn lock_folder(dir: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    use std::os::fd::AsRawFd as _;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("trek.lock"))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    match std::io::Error::last_os_error() {
        e if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        e => Err(e),
    }
}

/// Take the data folder for this process, for as long as it runs. False when another Trek has
/// it (a dev build sharing the folder, say): what that one has open isn't this one's to close.
fn hold_data_folder() -> bool {
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    match lock_folder(&trek_core::paths::data_dir()) {
        Ok(Some(lock)) => {
            let _ = LOCK.set(lock);
            true
        }
        Ok(None) => {
            tracing::warn!("another Trek is using {}; turns it left open are left to it", trek_core::paths::data_dir().display());
            false
        }
        Err(e) => {
            tracing::warn!("lock the data folder: {e}");
            true
        }
    }
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
    ("consult", "Ask other models first: /consult sol high, opus max: your message"),
    ("consult arena", "Have other models each design it, judged blind: /consult arena: your message"),
    ("restate", "Have the agent say back what you asked before it starts: /restate your message, or /restate alone for the thread so far"),
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

impl Workspace {
    /// What `id` and the sub-agents under it spent, from the store.
    fn load_spend(&self, id: &str) -> crate::cost::ThreadSpend {
        let own = self.store.usage_of(&[id.to_string()]).unwrap_or_default();
        let subs = self.store.descendants(id).unwrap_or_default();
        let theirs = self.store.usage_of(&subs).unwrap_or_default();
        let sub_threads = theirs.iter().map(|r| r.thread_id.as_str()).collect::<HashSet<_>>().len();
        // Started to advise (the default), as far as this run knows.
        let consults = subs.iter().all(|c| self.delegations.get(c).is_none_or(|d| d.mode == trek_core::orchestrate::Mode::Advise));
        crate::cost::ThreadSpend { own: trek_core::store::UsageRow::spend(&own), subs: trek_core::store::UsageRow::spend(&theirs), sub_threads, consults }
    }

    /// What `id` spent, as the status strip shows it.
    pub fn spend_of(&self, id: &str) -> crate::cost::ThreadSpend {
        self.live.get(id).and_then(|l| l.spend.clone()).unwrap_or_else(|| self.load_spend(id))
    }

    /// How `t`'s tokens are paid for: as its session said, else as its agent's login is (a plan,
    /// a key), or as its provider is.
    pub fn billing_of(&self, t: &Thread) -> Option<Billing> {
        if let Some(b) = self.live.get(&t.id).and_then(|l| l.billing.clone()) {
            return Some(b);
        }
        match &t.agent {
            AgentId::Direct(p) if catalog::is_mock(p) || catalog::direct_provider(p).is_some_and(|d| d.local) => Some(Billing::Local),
            AgentId::Direct(_) => Some(Billing::Metered),
            agent => self.agent_status.get(&agent.key()).and_then(|s| s.billing.clone()),
        }
    }

    /// Keep what a turn of `id` used, priced: by the agent, else from the price table at
    /// today's prices, so later price changes don't rewrite what it cost. The thread's spend,
    /// and its share in the threads above it, follow.
    fn record_usage(&mut self, id: &str, used: Vec<(Option<String>, trek_core::TokenUsage, Option<trek_core::UsageCost>)>) {
        let at = now_ms();
        let Some(t) = self.thread(id) else { return };
        // Unnamed, it's the thread's model (picked since the session started, maybe), or the one
        // the session said it runs when the thread leaves it to the agent.
        let fallback = t.model.clone().or_else(|| self.live.get(id).and_then(|l| l.session_model.clone()));
        let (agent, parent) = (t.agent.clone(), t.parent_id.clone());
        let mut kept = vec![];
        for (model, tokens, cost) in used {
            let model = model.or_else(|| fallback.clone());
            let cost = cost.or_else(|| model.as_deref().and_then(|m| trek_core::pricing::estimate(m, &agent, &tokens, at)).map(trek_core::UsageCost::priced));
            if let Err(e) = self.store.record_usage(id, at, &agent, model.as_deref(), &tokens, cost) {
                tracing::warn!("record token usage: {e:#}");
            }
            kept.push((model, tokens, cost));
        }
        match self.live.get_mut(id).and_then(|l| l.spend.as_mut()) {
            Some(spend) => {
                for (model, tokens, cost) in &kept {
                    spend.own.add(&agent, model.as_deref(), tokens, *cost, at);
                }
            }
            // Started here, it hasn't been read yet: the store has it all now.
            None => {
                let spend = self.load_spend(id);
                self.live.entry(id.to_string()).or_default().spend = Some(spend);
            }
        }
        let mut up = parent;
        while let Some(p) = up {
            if self.live.get(&p).is_some_and(|l| l.spend.is_some()) {
                let spend = self.load_spend(&p);
                if let Some(l) = self.live.get_mut(&p) {
                    l.spend = Some(spend);
                }
            }
            up = self.thread(&p).and_then(|t| t.parent_id.clone());
        }
    }
}

/// Devin, the ACP agent whose plan and quota Trek reads (`trek_agents::devin_status`).
pub fn devin_agent() -> AgentId {
    AgentId::Acp("devin".into())
}

/// How often Devin is asked for its quota, at most.
const DEVIN_STATUS_EVERY: i64 = 10 * 60_000;

/// The `/cost` answer: the breakdown the status strip's tooltip shows, as text.
fn cost_reply(billing: Option<&Billing>, spend: &crate::cost::ThreadSpend) -> String {
    match (billing, crate::cost::breakdown(billing, spend)) {
        (Some(Billing::Local), _) => "This session runs on a local model, so nothing is billed.".into(),
        (_, Some(b)) => crate::cost::reply(&b),
        (Some(Billing::Plan(plan)), None) => format!("Nothing used yet. This thread is included in {}.", crate::cost::plan_phrase(plan)),
        (_, None) => "Nothing used yet: the agent hasn't reported any tokens in this thread.".into(),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0}K", n as f64 / 1_000.),
        1_000_000..=999_999_999 => {
            let m = n as f64 / 1_000_000.;
            if m.fract() < 0.05 { format!("{m:.0}M") } else { format!("{m:.1}M") }
        }
        _ => {
            let b = n as f64 / 1_000_000_000.;
            if b.fract() < 0.05 { format!("{b:.0}B") } else { format!("{b:.1}B") }
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
    fn the_update_toast_names_what_the_restart_stops() {
        assert_eq!(restart_message(10, &[]), "Trek restarts to update in 10 seconds.");
        assert_eq!(restart_message(10, &["npm run dev\n# more"]), "Trek restarts to update in 10 seconds, stopping npm run dev.");
        assert_eq!(restart_message(10, &["npm run dev", "cargo watch"]), "Trek restarts to update in 10 seconds, stopping 2 background tasks.");
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
    fn cost_command_answers_per_billing() {
        let plus = Billing::Plan(Some("ChatGPT Plus".into()));
        let none = crate::cost::ThreadSpend::default();
        assert_eq!(cost_reply(Some(&plus), &none), "Nothing used yet. This thread is included in your ChatGPT Plus plan.");
        assert_eq!(cost_reply(Some(&Billing::Local), &none), "This session runs on a local model, so nothing is billed.");
        let mut spend = crate::cost::ThreadSpend::default();
        let tokens = trek_core::TokenUsage { input: 1_000, output: 100, ..Default::default() };
        spend.own.add(&AgentId::ClaudeCode, Some("claude-opus-5-5"), &tokens, Some(trek_core::UsageCost::reported(3.5)), 0);
        assert!(cost_reply(Some(&Billing::Plan(Some("Claude Max".into()))), &spend).starts_with("≈ $3.50 at API prices\nIncluded in your Claude Max plan."));
        assert!(cost_reply(Some(&Billing::Metered), &spend).starts_with("$3.50 so far\nBilled per token by your API provider."));
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
    fn one_process_at_a_time_holds_a_data_folder() {
        let dir = std::env::temp_dir().join(format!("trek-lock-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = lock_folder(&dir).unwrap().expect("free");
        // Another holder (flock locks are per open file, so this stands in for a second Trek).
        assert!(lock_folder(&dir).unwrap().is_none());
        drop(first);
        // A child another test forks shares the lock's descriptor until it execs: wait that out.
        let again = (0..100).any(|_| {
            lock_folder(&dir).unwrap().is_some() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        });
        assert!(again, "free again once the first is gone");
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

