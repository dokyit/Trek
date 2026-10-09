//! The AI side bar's transcript: the same `Item`s the harness's `ThreadView` draws, compact for a
//! 320–500 px side bar. Your messages are plain blocks on an orange rule (Restore, Edit and Copy
//! on hover); reading and searching fold into one line ("Read main.rs util.rs · searched
//! “parse”"), edits into file chips that open the file, shell commands into a small card with
//! their last lines; answers are markdown at UI size; a finished turn's changes get a card with
//! Keep and Undo per file. Under it all, while a turn runs, a working line; and pinned under the
//! scroll, what the agent asks of the user (an approval, questions, a plan).
//!
//! Only the latest blocks are drawn ("Show earlier" brings more): a side bar's thread is read
//! from the end. The blocks are worked out once per transcript revision, and the view redraws
//! only when its chat moved (not for every token another thread streams).
//!
//! A finished turn has Undo, Retry and Retry with another model (the latest turn's always
//! showing, earlier ones' on hover), and a restatement the agent was asked for gets "That's
//! right — go ahead" and "Not quite…", as in the harness. Undo, Retry and Restore ask first in
//! place, listing which files putting the checkpoint back would change, with that choice on
//! or off.

use super::cards;
use super::context;
use super::restore::{self, Files};
use crate::activity::{Place, ToolKind, op, phrase, placement, tool_kind};
use crate::palette;
use crate::workspace::{Scope, TaskState, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_core::changes::TurnChanges;
use trek_core::orchestrate as orch;
use trek_core::store::{Item, ToolStatus};

/// Blocks drawn at first; "Show earlier" adds as many again.
const SHOWN: usize = 80;
/// Lines of a command's output its card shows.
const TAIL_LINES: usize = 3;

/// What a stretch of the transcript shows as, by item position.
#[derive(Debug, Clone, PartialEq)]
enum Block {
    User(usize),
    Wake(usize),
    /// Consecutive thoughts.
    Thought(Vec<usize>),
    /// Reads, searches and other looking around, with the thoughts between them.
    Explore(Vec<usize>),
    /// Consecutive file edits.
    Edits(Vec<usize>),
    Command(usize),
    SubAgent(usize),
    Assistant(usize),
    TurnEnd(usize),
    Notice(usize),
    Error(usize),
    Limit(usize),
    Handoff(usize),
}

impl Block {
    fn first(&self) -> usize {
        match self {
            Block::Thought(v) | Block::Explore(v) | Block::Edits(v) => v[0],
            Block::User(i) | Block::Wake(i) | Block::Command(i) | Block::SubAgent(i) | Block::Assistant(i) | Block::TurnEnd(i) | Block::Notice(i) | Block::Error(i) | Block::Limit(i) | Block::Handoff(i) => *i,
        }
    }
}

/// Fold `items` into blocks. A thought still streaming (`live`) has none (the working line says
/// "Thinking"), nor has an empty one.
fn blocks(items: &[Item], live: Option<usize>) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    for (ix, item) in items.iter().enumerate() {
        match item {
            Item::Tool { title, .. } => match placement(item) {
                Place::Hidden => {}
                Place::Own => out.push(Block::SubAgent(ix)),
                Place::Group => match tool_kind(title) {
                    ToolKind::Command => out.push(Block::Command(ix)),
                    ToolKind::Edit => match out.last_mut() {
                        Some(Block::Edits(v)) => v.push(ix),
                        _ => out.push(Block::Edits(vec![ix])),
                    },
                    _ if matches!(title.as_str(), "Delete" | "Move") => match out.last_mut() {
                        Some(Block::Edits(v)) => v.push(ix),
                        _ => out.push(Block::Edits(vec![ix])),
                    },
                    _ => match out.last_mut() {
                        Some(Block::Explore(v)) => v.push(ix),
                        _ => out.push(Block::Explore(vec![ix])),
                    },
                },
            },
            Item::Reasoning { text } => {
                if live == Some(ix) || text.trim().is_empty() {
                    continue;
                }
                match out.last_mut() {
                    // Thinking between reads is part of looking around.
                    Some(Block::Explore(v)) | Some(Block::Thought(v)) => v.push(ix),
                    _ => out.push(Block::Thought(vec![ix])),
                }
            }
            Item::User { text, .. } if orch::is_wake(text) => out.push(Block::Wake(ix)),
            Item::User { .. } => out.push(Block::User(ix)),
            Item::Assistant { text } if text.trim().is_empty() => {}
            Item::Assistant { .. } => out.push(Block::Assistant(ix)),
            Item::TurnEnd { .. } => out.push(Block::TurnEnd(ix)),
            Item::Notice { .. } => out.push(Block::Notice(ix)),
            Item::Error { .. } => out.push(Block::Error(ix)),
            Item::Limit { .. } => out.push(Block::Limit(ix)),
            Item::Handoff { .. } => out.push(Block::Handoff(ix)),
        }
    }
    out
}

/// A piece of the Explore line: words, or a file's chip.
#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    File(String),
}

/// "Read a.rs b.rs · searched “x”": what a stretch of looking around did, from its calls
/// (title, detail).
fn explore_parts(calls: &[(String, String)], cwd: Option<&Path>) -> Vec<Part> {
    let mut reads: Vec<String> = vec![];
    let mut searches: Vec<String> = vec![];
    let mut other: Vec<String> = vec![];
    let (mut web, mut fetched) = (0, vec![]);
    for (title, detail) in calls {
        let o = op(title, detail, cwd);
        match tool_kind(title) {
            ToolKind::Read => reads.extend(o.file.or(Some(o.text)).filter(|f| !f.is_empty()).map(|f| f.rsplit('/').next().unwrap_or(&f).to_string())),
            ToolKind::Search => searches.push(o.text),
            ToolKind::WebSearch => web += 1,
            ToolKind::Fetch => fetched.push(o.text),
            _ => other.push(if o.text.is_empty() { o.verb } else { format!("{} {}", o.verb, o.text) }),
        }
    }
    reads.dedup();
    let mut parts: Vec<Vec<Part>> = vec![];
    if !reads.is_empty() {
        let mut p = vec![Part::Text("Read".into())];
        if reads.len() > 3 {
            p.push(Part::Text(format!("{} files", reads.len())));
        } else {
            p.extend(reads.into_iter().map(Part::File));
        }
        parts.push(p);
    }
    if let Some(first) = searches.first() {
        let more = if searches.len() > 1 { format!(" +{}", searches.len() - 1) } else { String::new() };
        parts.push(vec![Part::Text(format!("searched “{}”{more}", orch::preview(first, 40)))]);
    }
    if web > 0 {
        parts.push(vec![Part::Text(if web == 1 { "searched the web".into() } else { format!("searched the web {web} times") })]);
    }
    if let Some(first) = fetched.first() {
        parts.push(vec![Part::Text(format!("fetched {}", orch::preview(first, 40)))]);
    }
    if let Some(first) = other.first() {
        let more = if other.len() > 1 { format!(" +{}", other.len() - 1) } else { String::new() };
        parts.push(vec![Part::Text(format!("{}{more}", orch::preview(first, 48)))]);
    }
    // The first part leads with a capital; the rest follow a dot.
    let mut out = vec![];
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.push(Part::Text("·".into()));
        }
        for (j, part) in p.into_iter().enumerate() {
            out.push(match part {
                Part::Text(t) if i == 0 && j == 0 => Part::Text(capitalize(&t)),
                other => other,
            });
        }
    }
    if out.is_empty() {
        out.push(Part::Text("Thought it through".into()));
    }
    out
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// The footer (`TurnEnd`) of the turn item `ix` is in, once the turn is over.
fn turn_end(items: &[Item], ix: usize) -> Option<usize> {
    let after = items.get(ix + 1..)?;
    let end = after.iter().position(trek_core::rewind::ends_turn)?;
    if after[..end].iter().any(|i| matches!(i, Item::User { aside: false, .. })) {
        return None;
    }
    matches!(after[end], Item::TurnEnd { .. }).then_some(ix + 1 + end)
}

/// One file an Edits block changed: where, its name, its lines (the turn's count once it's over
/// and counted from git; until then, where every call said, what they did).
#[derive(Debug, Clone, PartialEq)]
struct EditFile {
    path: PathBuf,
    name: String,
    lines: Option<(u32, u32)>,
}

/// A block, read out of the transcript for drawing (owned, so drawing can update the view).
enum View {
    User { key: String, text: String, context: Vec<String>, ask: bool, images: Vec<PathBuf>, at: Option<i64>, aside: bool, open: bool },
    Wake { key: String, summary: String, text: String, open: bool },
    /// `secs`: how long the thinking took, when it was watched here (a transcript read back
    /// doesn't say).
    Thought { key: String, text: String, secs: Option<u64>, open: bool },
    Explore { key: String, parts: Vec<Part>, rows: Vec<(String, String, ToolStatus)>, running: bool, open: bool },
    Edits { files: Vec<EditFile>, running: bool },
    Command { key: String, command: String, tail: Vec<String>, output: Option<String>, status: ToolStatus, open: bool },
    SubAgent { label: String, detail: String, state: TaskState, child: Option<String>, agent: Option<trek_core::AgentId> },
    /// `live`: still streaming in.
    Assistant { key: String, text: String, live: bool },
    TurnEnd { ix: usize, key: String },
    Notice(String),
    Error(String),
    Limit(String),
    Handoff(String),
}

/// What a confirmation in the transcript asks before.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// Back to before a message of yours.
    Rewind,
    Undo,
    /// Again, with another model when given.
    Retry(Option<String>),
}

/// A confirmation open in the transcript: under a message (a rewind) or a turn's actions.
struct Confirm {
    ask: Ask,
    /// The item it's under: the message, or the turn's end.
    anchor: String,
    /// The message whose checkpoint the files go back to.
    message: String,
    files: Files,
    restore: bool,
    _check: Option<Task<()>>,
}

/// What the view was last drawn for: it redraws when this moves.
#[derive(Debug, Clone, PartialEq, Default)]
struct Seen {
    thread: Option<String>,
    revision: u64,
    running: bool,
    loading: bool,
    asks: usize,
    picks: usize,
    pending: usize,
    glass: bool,
}

pub struct AiTranscript {
    workspace: Entity<Workspace>,
    /// The thread shown.
    current: Option<String>,
    /// Blocks worked out for (thread, revision, items, thought streaming).
    blocks: std::cell::RefCell<Option<((String, u64, usize, Option<usize>), std::rc::Rc<Vec<Block>>)>>,
    seen: Seen,
    confirm: Option<Confirm>,
    scroll: ScrollHandle,
    /// Keep the end in view as it grows (the reader hasn't scrolled up).
    follow: bool,
    /// Markdown documents by item id: answers and opened thoughts, as streamed so far.
    md: HashMap<String, (Entity<TextViewState>, String, Subscription)>,
    /// Blocks and messages opened by the user, by item id.
    expanded: HashSet<String>,
    shown: usize,
    /// Masked fields for the question card's secret questions (with their Return watch), and
    /// the request they're for.
    secrets: (Option<String>, Vec<(Entity<InputState>, Subscription)>),
    /// The plan card's document: (request, markdown).
    plan_md: Option<(String, Entity<TextViewState>, Subscription)>,
    /// Redraws once a second while a turn runs (its time ticks).
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl AiTranscript {
    pub fn new(workspace: Entity<Workspace>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| match event {
                WorkspaceEvent::Transcript { id, .. } | WorkspaceEvent::TurnChanges { id, .. } | WorkspaceEvent::ReviewChanged { id } if this.current.as_ref() == Some(id) => cx.notify(),
                _ => {}
            }),
        ];
        let mut this = Self {
            workspace,
            current: None,
            blocks: Default::default(),
            seen: Seen::default(),
            confirm: None,
            scroll: ScrollHandle::new(),
            follow: true,
            md: HashMap::new(),
            expanded: HashSet::new(),
            shown: SHOWN,
            secrets: (None, vec![]),
            plan_md: None,
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    /// Follow the side bar's active chat; tick while its turn runs. Redraws only when the chat
    /// shown moved.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let id = ws.thread_id_in(&Scope::Ide).map(str::to_string);
        let running = id.as_ref().is_some_and(|id| ws.turn_running(id));
        let live = id.as_ref().and_then(|id| ws.live.get(id));
        let seen = Seen {
            thread: id.clone(),
            revision: live.map_or(0, |l| l.revision),
            running,
            loading: live.is_some_and(|l| l.loading),
            asks: live.map_or(0, |l| l.permissions.len()),
            picks: live.map_or(0, |l| l.picks.values().map(Vec::len).sum()),
            pending: id.as_ref().map_or(0, |id| ws.pending_files(id).len()),
            glass: ws.glass().is_some(),
        };
        if id != self.current {
            self.current = id;
            self.md.clear();
            self.expanded.clear();
            self.confirm = None;
            self.shown = SHOWN;
            self.follow = true;
            self.scroll.scroll_to_bottom();
        }
        let moved = seen != self.seen;
        self.seen = seen;
        match (running, self._ticker.is_some()) {
            (true, false) => {
                self._ticker = Some(cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }))
            }
            (false, true) => self._ticker = None,
            _ => {}
        }
        if moved {
            cx.notify();
        }
    }

    /// The current thread's blocks, worked out once per transcript revision.
    fn blocks_of(&self, cx: &App) -> std::rc::Rc<Vec<Block>> {
        let ws = self.workspace.read(cx);
        let Some((id, live)) = self.current.as_ref().and_then(|id| Some((id, ws.live.get(id)?))) else { return Default::default() };
        let key = (id.clone(), live.revision, live.items.len(), live.reasoning);
        if let Some((k, blocks)) = self.blocks.borrow().as_ref() {
            if *k == key {
                return blocks.clone();
            }
        }
        let blocks = std::rc::Rc::new(blocks(&live.items, live.reasoning));
        *self.blocks.borrow_mut() = Some((key, blocks.clone()));
        blocks
    }

    fn toggle(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
        cx.notify();
    }

    /// Item `key`'s markdown document, brought up to `text` (streamed text is appended).
    fn doc(&mut self, key: &str, text: &str, cx: &mut Context<Self>) -> Entity<TextViewState> {
        if let Some((state, old, _)) = self.md.get_mut(key) {
            if old != text {
                match text.strip_prefix(old.as_str()) {
                    Some(tail) => state.update(cx, |s, cx| s.push_str(tail, cx)),
                    None => state.update(cx, |s, cx| s.set_text(text, cx)),
                }
                *old = text.to_string();
            }
            return state.clone();
        }
        let state = cx.new(|cx| TextViewState::markdown(text, cx));
        let changed = cx.observe(&state, |_, _, cx| cx.notify());
        self.md.insert(key.to_string(), (state.clone(), text.to_string(), changed));
        state
    }

    /// The blocks of the current thread, read out for drawing: the latest `shown`, and how many
    /// came before them.
    fn views(&self, cx: &App) -> (Vec<View>, usize) {
        let all = self.blocks_of(cx);
        let ws = self.workspace.read(cx);
        let Some((id, live)) = self.current.as_ref().and_then(|id| Some((id, ws.live.get(id)?))) else { return (vec![], 0) };
        let cwd = ws.thread(id).and_then(|t| t.cwd.clone());
        // Where the folder really is: turns' changes name files under the repository's top
        // folder as git sees it (links resolved).
        let canon_cwd = cwd.as_deref().and_then(|c| std::fs::canonicalize(c).ok());
        let agent = ws.thread(id).map(|t| t.agent.clone()).unwrap_or(trek_core::AgentId::ClaudeCode);
        let ids = live.item_ids();
        let earlier = all.len().saturating_sub(self.shown);
        let open = |ix: usize| self.expanded.contains(&ids[ix]);
        let views = all[earlier..]
            .iter()
            .map(|b| {
                let key = ids[b.first()].clone();
                match b {
                    Block::User(ix) => {
                        let Item::User { text, images, at, aside, .. } = &live.items[*ix] else { return View::Notice(String::new()) };
                        let (said, _) = orch::split_consult(text);
                        let (said, _) = trek_core::restate::split_restate(said);
                        let sent = context::split(said);
                        View::User { key, text: sent.said.to_string(), context: sent.context, ask: sent.ask, images: images.iter().map(PathBuf::from).collect(), at: *at, aside: *aside, open: open(*ix) }
                    }
                    Block::Wake(ix) => {
                        let Item::User { text, .. } = &live.items[*ix] else { return View::Notice(String::new()) };
                        View::Wake { key, summary: orch::wake_summary(text), text: text.clone(), open: open(*ix) }
                    }
                    Block::Thought(v) => {
                        let text = v.iter().filter_map(|i| if let Item::Reasoning { text } = &live.items[*i] { Some(text.trim()) } else { None }).collect::<Vec<_>>().join("\n\n");
                        let secs = v.iter().filter_map(|i| live.thought_secs(&ids[*i])).reduce(|a, b| a + b);
                        View::Thought { key, text, secs, open: open(v[0]) }
                    }
                    Block::Explore(v) => {
                        let calls: Vec<(String, String, ToolStatus)> = v
                            .iter()
                            .filter_map(|i| match &live.items[*i] {
                                Item::Tool { title, detail, status, .. } => Some((title.clone(), detail.clone(), *status)),
                                _ => None,
                            })
                            .collect();
                        let pairs: Vec<(String, String)> = calls.iter().map(|(t, d, _)| (t.clone(), d.clone())).collect();
                        let running = calls.iter().any(|(_, _, s)| *s == ToolStatus::Running);
                        let rows = calls.iter().map(|(t, d, s)| {
                            let o = op(t, d, cwd.as_deref());
                            (if o.verb.is_empty() || o.text.is_empty() { format!("{}{}", o.verb, o.text) } else { format!("{} {}", o.verb, o.text) }, t.clone(), *s)
                        });
                        View::Explore { key, parts: explore_parts(&pairs, cwd.as_deref()), rows: rows.map(|(a, _, s)| (a, String::new(), s)).collect(), running, open: open(v[0]) }
                    }
                    Block::Edits(v) => {
                        let mut files: Vec<EditFile> = vec![];
                        let mut running = false;
                        for i in v {
                            let Item::Tool { id: call, detail, status, .. } = &live.items[*i] else { continue };
                            running |= *status == ToolStatus::Running;
                            if matches!(status, ToolStatus::Failed | ToolStatus::Denied) {
                                continue;
                            }
                            let paths: Vec<&str> = detail.split(", ").map(str::trim).filter(|p| !p.is_empty()).collect();
                            let counted = if paths.len() == 1 { live.lines.get(call).copied() } else { None };
                            for p in paths {
                                let path = match &cwd {
                                    Some(c) if !Path::new(p).is_absolute() => c.join(p),
                                    _ => PathBuf::from(p),
                                };
                                let name = p.rsplit('/').next().unwrap_or(p).to_string();
                                match files.iter_mut().find(|f| f.path == path) {
                                    Some(f) => f.lines = f.lines.zip(counted).map(|((a, r), (da, dr))| (a + da, r + dr)),
                                    None => files.push(EditFile { path, name, lines: counted }),
                                }
                            }
                        }
                        // A finished turn counted from its checkpoints: each file's lines as its
                        // changes card and the review count them, not as the tools reported them.
                        if let Some(turn) = turn_end(&live.items, v[0]).and_then(|end| ws.turn_changes(id, end)).filter(|t| t.counted == trek_core::changes::Counted::Checkpoints) {
                            let below = canon_cwd.as_deref().and_then(|c| c.strip_prefix(&turn.root).ok());
                            for f in &mut files {
                                let rel = cwd.as_deref().and_then(|c| f.path.strip_prefix(c).ok()).zip(below).map(|(rel, below)| below.join(rel));
                                f.lines = rel.and_then(|rel| turn.files.iter().find(|c| Path::new(&c.path) == rel)).filter(|c| c.lines_known && !c.binary).map(|c| (c.added, c.removed));
                            }
                        }
                        View::Edits { files, running }
                    }
                    Block::Command(ix) => {
                        let Item::Tool { detail, output, status, .. } = &live.items[*ix] else { return View::Notice(String::new()) };
                        let tail: Vec<String> = output.lines().filter(|l| !l.trim().is_empty()).rev().take(TAIL_LINES).map(|l| orch::preview(l, 160)).collect::<Vec<_>>().into_iter().rev().collect();
                        let is_open = open(*ix);
                        View::Command { key, command: detail.lines().next().unwrap_or_default().to_string(), tail, output: is_open.then(|| output.clone()), status: *status, open: is_open }
                    }
                    Block::SubAgent(ix) => {
                        let Item::Tool { id: row, detail, output, status, .. } = &live.items[*ix] else { return View::Notice(String::new()) };
                        let sub = crate::thread_view::SubAgentRow::read(ws, id, &agent, row, detail, output, *status);
                        View::SubAgent { label: sub.label, detail: sub.detail, state: sub.state, child: sub.child, agent: sub.agent }
                    }
                    Block::Assistant(ix) => match &live.items[*ix] {
                        Item::Assistant { text } => View::Assistant { key, text: text.clone(), live: live.streaming == Some(*ix) },
                        _ => View::Notice(String::new()),
                    },
                    Block::TurnEnd(ix) => View::TurnEnd { ix: *ix, key },
                    Block::Notice(ix) => match &live.items[*ix] {
                        Item::Notice { text } => View::Notice(text.replace("**", "").replace('`', "")),
                        _ => View::Notice(String::new()),
                    },
                    Block::Error(ix) => match &live.items[*ix] {
                        Item::Error { text } => View::Error(text.clone()),
                        _ => View::Notice(String::new()),
                    },
                    Block::Limit(ix) => match &live.items[*ix] {
                        Item::Limit { scope, resets_at, .. } => View::Limit(format!("Usage limit reached · {}", crate::thread_view::limit_when(scope, *resets_at, ws.now()))),
                        _ => View::Notice(String::new()),
                    },
                    Block::Handoff(ix) => match &live.items[*ix] {
                        Item::Handoff { to, to_model, to_name, .. } => {
                            let agent = trek_core::AgentId::from_key(to);
                            let name = to_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| crate::thread_view::handoff_name(&agent, to_model.as_deref()));
                            View::Handoff(format!("Handed off to {name}"))
                        }
                        _ => View::Notice(String::new()),
                    },
                }
            })
            .collect();
        (views, earlier)
    }

    /// Ask before `ask` on `anchor` (a message for a rewind, a turn's end otherwise), and find
    /// out, off the main thread, which files putting its checkpoint back would change.
    fn open_confirm(&mut self, ask: Ask, anchor: String, cx: &mut Context<Self>) {
        let Some(thread) = self.current.clone() else { return };
        let ws = self.workspace.read(cx);
        if ws.turn_running(&thread) {
            return;
        }
        let message = match ask {
            Ask::Rewind => Some(anchor.clone()),
            _ => ws.turn_start_item(&thread, &anchor),
        };
        let Some(message) = message else { return };
        let (files, check) = match restore::checkpoint(ws, &thread, &message) {
            Ok((repo, sha)) => {
                let task = cx.spawn(async move |this, cx| {
                    let files = cx.background_executor().spawn(async move { restore::read(repo, sha) }).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(c) = this.confirm.as_mut() {
                            c.files = files;
                            cx.notify();
                        }
                    });
                });
                (Files::Checking, Some(task))
            }
            Err(why) => (Files::Unavailable(why), None),
        };
        let restore = matches!(files, Files::Checking);
        self.confirm = Some(Confirm { ask, anchor, message, files, restore, _check: check });
        cx.notify();
    }

    /// The last turn's Undo, asking (the shots harness).
    #[cfg(feature = "shots")]
    pub fn confirm_last_undo(&mut self, cx: &mut Context<Self>) {
        let end = self.current.as_ref().and_then(|id| {
            let live = self.workspace.read(cx).live.get(id)?;
            let ix = live.items.iter().rposition(|i| matches!(i, Item::TurnEnd { .. }))?;
            live.items.id_at(ix).map(str::to_string)
        });
        if let Some(end) = end {
            self.open_confirm(Ask::Undo, end, cx);
        }
    }

    fn close_confirm(&mut self, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            cx.notify();
        }
    }

    /// Do what the open confirmation asked. A message taken back goes to the input.
    fn run_confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(c), Some(thread)) = (self.confirm.take(), self.current.clone()) else { return };
        let restore = c.restore && !c.files.nothing();
        self.workspace.update(cx, |ws, cx| {
            let back = match c.ask {
                Ask::Rewind => ws.rewind(&thread, &c.message, restore, cx),
                Ask::Undo => ws.undo_turn(&thread, &c.anchor, restore, cx),
                Ask::Retry(model) => {
                    ws.retry(&thread, &c.anchor, model, restore, cx);
                    None
                }
            };
            if let Some((text, images)) = back {
                cx.emit(WorkspaceEvent::ComposeIn { scope: Scope::Ide, thread, text, images, edit: None });
            }
        });
        cx.notify();
    }

    /// The open confirmation, when it's under `anchor`: what will happen, the files restoring
    /// would change (and whether to), and the go-ahead.
    fn confirm_card(&self, anchor: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.confirm.as_ref().filter(|c| c.anchor == anchor)?;
        let ws = self.workspace.read(cx);
        let thread = ws.thread(self.current.as_ref()?)?;
        let agent = thread.agent.display_name();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let (title, action) = match &c.ask {
            Ask::Rewind => ("Restore to before this message?".to_string(), "Restore"),
            Ask::Undo => ("Undo this turn?".to_string(), "Undo turn"),
            Ask::Retry(None) => ("Retry this turn?".to_string(), "Retry"),
            Ask::Retry(Some(m)) => {
                let name = ws.models_for(&thread.agent).into_iter().find(|i| crate::composer::same_model(m, &i.id)).map(|i| i.name).unwrap_or_else(|| m.clone());
                (format!("Retry with {name}?"), "Retry")
            }
        };
        let mut body = match &c.ask {
            Ask::Rewind => format!("This message and everything after it leave the chat, and {agent} forgets them. Your message comes back to the input."),
            Ask::Undo => format!("This turn and everything after it leave the chat, and {agent} forgets them. Your message comes back to the input."),
            Ask::Retry(_) => format!("This turn and everything after it leave the chat, and {agent} gets your message again."),
        };
        if ws.rewind_plan(&thread.id, &c.message) == Some(trek_core::rewind::Reopen::Recap) {
            body.push_str(&format!(" {agent} can't take its own session back to this point, so it continues in a new one with a recap."));
        }
        let note = |text: String| div().text_size(px(11.5)).line_height(relative(1.45)).text_color(muted).child(text).into_any_element();
        let files: AnyElement = match &c.files {
            Files::Checking => h_flex().gap(px(6.)).text_size(px(11.5)).text_color(muted).child(Spinner::new().xsmall()).child("Checking which files changed…").into_any_element(),
            Files::Changes(changes) if changes.iter().all(|f| !restore::acts(f)) => note("The files are as they were then.".into()),
            Files::Changes(changes) => {
                let shown: Vec<_> = changes.iter().take(restore::LISTED).cloned().collect();
                v_flex()
                    .gap(px(4.))
                    .child(crate::ui::check_row("ai-confirm-restore", "Also restore files", c.restore, false, cx).test_support().on_click(cx.listener(|this, _, _, cx| {
                        if let Some(c) = this.confirm.as_mut() {
                            c.restore = !c.restore;
                            cx.notify();
                        }
                    })))
                    .child(
                        v_flex()
                            .pl(px(22.))
                            .gap(px(1.))
                            .when(!c.restore, |el| el.opacity(0.5))
                            .children(shown.iter().map(|f| {
                                h_flex()
                                    .gap(px(8.))
                                    .text_size(px(11.))
                                    .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).child(f.path.clone()))
                                    .child(div().flex_none().text_color(muted).child(restore::verb(f)))
                            }))
                            .when(changes.len() > restore::LISTED, |el| el.child(div().text_size(px(11.)).text_color(muted).child(format!("and {} more", changes.len() - restore::LISTED)))),
                    )
                    .into_any_element()
            }
            Files::Unavailable(why) => v_flex().gap(px(4.)).child(crate::ui::check_row("ai-confirm-restore", "Also restore files", false, true, cx).test_support()).child(note(why.clone())).into_any_element(),
            Files::Failed(e) => note(format!("Couldn't check the files: {e}")),
        };
        Some(
            v_flex()
                .id("ai-confirm")
                .test_support()
                .w_full()
                .p(px(10.))
                .gap(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme.foreground.opacity(0.12))
                .bg(theme.foreground.opacity(0.03))
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme.foreground).child(title))
                .child(note(body))
                .child(div().pt(px(6.)).border_t_1().border_color(theme.foreground.opacity(0.07)).child(files))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .justify_end()
                        .child(Button::new("ai-confirm-cancel").ghost().xsmall().label("Cancel").on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx))))
                        .child(Button::new("ai-confirm-go").primary().xsmall().label(action).on_click(cx.listener(|this, _, _, cx| this.run_confirm(cx)))),
                )
                .into_any_element(),
        )
    }

    /// Undo, Retry and Retry with another model for the turn ending at `key` (shown always on
    /// the latest turn, on hover on earlier ones), and the restatement check when the agent was
    /// asked to say back what it understood.
    fn turn_actions(&self, ix: usize, key: &str, n: usize, last: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let ws = self.workspace.read(cx);
        let Some(thread) = self.current.clone() else { return div().into_any_element() };
        let live = ws.live.get(&thread);
        let started = live.and_then(|l| trek_core::rewind::turn_start(&l.items, ix)).is_some();
        let off = busy || !started;
        let tip = |what: &str| -> SharedString {
            if busy {
                format!("Stop the running turn to {what}").into()
            } else if !started {
                "This turn didn't start from a message of yours".into()
            } else {
                capitalize(what).into()
            }
        };
        let restated = live.is_some_and(|l| trek_core::restate::asked_in_turn(&l.items, ix));
        let (agent, model) = ws.thread(&thread).map(|t| (t.agent.clone(), t.model.clone())).unzip();
        let group = SharedString::from(format!("ai-turn-{n}"));
        let action = |id: (&'static str, usize), icon: Icon, label: &'static str, tooltip: SharedString| {
            h_flex()
                .id(id)
                .test_support()
                .gap(px(3.))
                .px(px(4.))
                .h(px(18.))
                .items_center()
                .rounded(px(4.))
                .when(off, |el| el.opacity(0.45))
                .when(!off, |el| el.cursor_pointer().hover(|s| s.text_color(theme.foreground).bg(theme.foreground.opacity(0.06))))
                .child(icon.size(px(11.)))
                .child(label)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
        };
        let (k1, k2) = (key.to_string(), key.to_string());
        let me = cx.weak_entity();
        let retry_with = {
            let key = key.to_string();
            let ws = self.workspace.clone();
            Button::new(("ai-retry-with", n))
                .ghost()
                .xsmall()
                .icon(Icon::new(IconName::ChevronDown).text_color(muted))
                .tooltip(if off { tip("retry") } else { "Retry with another model".into() })
                .disabled(off)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, cx| {
                    menu = menu.min_w(px(220.)).max_h(px(320.)).scrollable(true).label("Retry with");
                    let Some(agent) = agent.clone() else { return menu };
                    let models = ws.read(cx).models_for(&agent);
                    let current = model.clone().flatten().or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone()));
                    for m in models.iter().filter(|m| current.as_deref().is_none_or(|c| !crate::composer::same_model(c, &m.id))).cloned() {
                        let (me, key) = (me.clone(), key.clone());
                        menu = menu.item(PopupMenuItem::new(m.name.clone()).on_click(move |_, _, cx| {
                            let _ = me.update(cx, |this, cx| this.open_confirm(Ask::Retry(Some(m.id.clone())), key.clone(), cx));
                        }));
                    }
                    menu
                })
        };
        let shown = last || self.confirm.as_ref().is_some_and(|c| c.anchor == key);
        let ws1 = self.workspace.clone();
        let ws2 = self.workspace.clone();
        let (t1, t2) = (thread.clone(), thread.clone());
        v_flex()
            .group(group.clone())
            .gap(px(6.))
            .child(
                h_flex()
                    .id(("ai-turn-actions", n))
                    .test_support()
                    .gap(px(2.))
                    .text_size(px(11.))
                    .text_color(muted.opacity(0.85))
                    .when(!shown, |el| el.invisible().group_hover(group.clone(), |s| s.visible()))
                    .child(action(("ai-undo-turn", n), Icon::new(crate::assets::Lucide::Undo2), "Undo", tip("undo this turn")).when(!off, |el| el.on_click(cx.listener(move |this, _, _, cx| this.open_confirm(Ask::Undo, k1.clone(), cx)))))
                    .child(action(("ai-retry-turn", n), Icon::new(crate::assets::Lucide::RefreshCw), "Retry", tip("retry")).when(!off, |el| el.on_click(cx.listener(move |this, _, _, cx| this.open_confirm(Ask::Retry(None), k2.clone(), cx)))))
                    .child(retry_with),
            )
            // The agent said back what it was asked: the user confirms it, or corrects it.
            .when(restated && last && !busy, |el| {
                el.child(
                    h_flex()
                        .id(("ai-restate-check", n))
                        .test_support()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(div().text_size(px(12.)).text_color(muted).child("Is that what you meant?"))
                        .child(Button::new(("ai-restate-yes", n)).xsmall().primary().label("That's right — go ahead").on_click(move |_, _, cx| ws1.update(cx, |ws, cx| ws.go_ahead(&t1, cx))))
                        .child(Button::new(("ai-restate-no", n)).xsmall().ghost().label("Not quite…").on_click(move |_, _, cx| {
                            let thread = t2.clone();
                            ws2.update(cx, |_, cx| cx.emit(WorkspaceEvent::CorrectRestatement { scope: Scope::Ide, thread }));
                        })),
                )
            })
            .children(self.confirm_card(key, cx))
            .into_any_element()
    }

    /// Answers from the question card, `None` until every question has one.
    fn card_answers(&self, request_id: &str, questions: &[trek_agents::Question], cx: &App) -> Option<Vec<(String, String)>> {
        let picks = &self.workspace.read(cx).live.get(self.current.as_ref()?)?.picks;
        let mut fields = self.secrets.1.iter();
        questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let picked = picks.get(&(request_id.to_string(), i)).filter(|v| !v.is_empty()).map(|v| v.join(", "));
                let typed = q.secret.then(|| fields.next()).flatten().map(|(f, _)| f.read(cx).value().trim().to_string()).filter(|v| !v.is_empty());
                typed.or(picked).map(|a| (q.question.clone(), a))
            })
            .collect()
    }

    fn submit_answers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current.clone() else { return };
        let pending = self.workspace.read(cx).live.get(&id).and_then(|l| l.permissions.first()).and_then(|p| match &p.prompt {
            Some(trek_agents::Prompt::Questions(q)) => Some((p.request_id.clone(), q.clone())),
            _ => None,
        });
        let Some((request_id, questions)) = pending else { return };
        let Some(answers) = self.card_answers(&request_id, &questions, cx) else { return };
        for (f, _) in &self.secrets.1 {
            f.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.workspace.update(cx, |ws, cx| ws.answer(&id, &request_id, answers, cx));
    }

    /// One masked field per secret question, empty for each new card.
    fn secret_fields(&mut self, request_id: &str, questions: &[trek_agents::Question], window: &mut Window, cx: &mut Context<Self>) {
        if self.secrets.0.as_deref() != Some(request_id) {
            self.secrets = (Some(request_id.to_string()), vec![]);
        }
        while self.secrets.1.len() < questions.iter().filter(|q| q.secret).count() {
            let field = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Type it here"));
            // Kept with its field: a new card's fields drop the old ones' watches with them.
            let watch = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_answers(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
            self.secrets.1.push((field, watch));
        }
    }

    /// What waits on the user, pinned under the transcript: an approval, questions or a plan.
    fn request_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.current.clone()?;
        let (p, agent, picks, cwd) = {
            let ws = self.workspace.read(cx);
            let live = ws.live.get(&id)?;
            let p = live.permissions.first()?.clone();
            (p, ws.thread(&id)?.agent.display_name(), live.picks.clone(), ws.cwd_in(&Scope::Ide))
        };
        let card = match &p.prompt {
            None => cards::approval(&self.workspace, &id, &p.request_id, &agent, &p.title, &p.detail, cx),
            Some(trek_agents::Prompt::Questions(questions)) => {
                self.secret_fields(&p.request_id, questions, window, cx);
                let complete = self.card_answers(&p.request_id, questions, cx).is_some();
                let me = cx.entity().downgrade();
                let submit = std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                    let _ = me.update(cx, |this, cx| this.submit_answers(window, cx));
                });
                let fields: Vec<Entity<InputState>> = self.secrets.1.iter().map(|(f, _)| f.clone()).collect();
                cards::questions(&self.workspace, &id, &p.request_id, &agent, questions, &picks, &fields, complete, submit, cx)
            }
            Some(trek_agents::Prompt::Plan(plan)) => {
                if self.plan_md.as_ref().is_none_or(|(rid, _, _)| *rid != p.request_id) {
                    let md = cx.new(|cx| TextViewState::markdown(plan, cx));
                    let parsed = cx.observe(&md, |_, _, cx| cx.notify());
                    self.plan_md = Some((p.request_id.clone(), md, parsed));
                }
                let doc = self.plan_md.as_ref().map(|(_, m, _)| m.clone());
                cards::plan(&self.workspace, &id, &p.request_id, &agent, doc.as_ref(), cwd, cx)
            }
        };
        Some(div().flex_none().px(px(12.)).pt(px(6.)).pb(px(2.)).child(card).into_any_element())
    }

    /// While a turn runs: what it's doing, how long it's been at it, and the follow-ups queued.
    fn working(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.current.clone()?;
        let ws = self.workspace.read(cx);
        let live = ws.live.get(&id)?;
        let started = live.turn_started?;
        if !live.permissions.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let doing = if live.reasoning.is_some() {
            "Thinking".to_string()
        } else {
            live.items
                .iter()
                .rev()
                .take_while(|i| !matches!(i, Item::User { .. } | Item::Assistant { .. }))
                .find_map(|i| if let Item::Tool { title, detail, .. } = i { Some(phrase(title, detail, None)) } else { None })
                .unwrap_or_else(|| "Working".into())
        };
        let secs = started.elapsed().as_secs();
        let queued: Vec<(usize, String)> = live.queued.iter().enumerate().map(|(i, (t, _))| (i, orch::preview(context::split(t).said, 80))).collect();
        Some(
            v_flex()
                .id("ai-working")
                .test_support()
                .gap(px(4.))
                .child(
                    h_flex()
                        .gap(px(7.))
                        .text_size(px(12.))
                        .text_color(muted)
                        .child(Spinner::new().xsmall().color(palette::ember(cx)))
                        .child(div().text_color(theme.foreground.opacity(0.8)).child(format!("{doing}…")))
                        .child(div().child(crate::time::elapsed(std::time::Duration::from_secs(secs)))),
                )
                .children(queued.into_iter().map(|(i, text)| {
                    let ws = self.workspace.clone();
                    let id = id.clone();
                    h_flex()
                        .id(("ai-queued", i))
                        .test_support()
                        .group("ai-queued")
                        .gap(px(6.))
                        .pl(px(19.))
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(div().flex_none().child("Queued"))
                        .child(div().flex_1().min_w_0().truncate().text_color(theme.foreground.opacity(0.75)).child(text))
                        .child(
                            div()
                                .id(("ai-queued-drop", i))
                                .invisible()
                                .group_hover("ai-queued", |s| s.visible())
                                .cursor_pointer()
                                .child(Icon::new(IconName::Close).size(px(11.)))
                                .on_click(move |_, _, cx| {
                                    ws.update(cx, |ws, cx| {
                                        if let Some(l) = ws.live.get_mut(&id).filter(|l| i < l.queued.len()) {
                                            l.queued.remove(i);
                                            l.revision += 1;
                                        }
                                        cx.notify();
                                    })
                                }),
                        )
                }))
                .into_any_element(),
        )
    }
}

impl Render for AiTranscript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("AiTranscript");
        let Some(thread) = self.current.clone() else { return div().into_any_element() };
        // Keep the end in view while the reader is there.
        let (offset, max) = (self.scroll.offset().y, self.scroll.max_offset().y);
        self.follow = -offset >= max - px(24.);
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        let (views, earlier) = self.views(cx);
        // The last turn's end: its actions show without a hover.
        let last_end = self.blocks_of(cx).iter().rev().find_map(|b| if let Block::TurnEnd(ix) = b { Some(*ix) } else { None });
        let (cwd, folder, busy, loading) = {
            let ws = self.workspace.read(cx);
            let t = ws.thread(&thread);
            (t.and_then(|t| t.cwd.clone()), t.and_then(|t| ws.thread_project_tint(t, cx)), ws.turn_running(&thread), ws.live.get(&thread).is_some_and(|l| l.loading))
        };
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let ember = palette::ember(cx);
        let ui = px(13.);
        let mut rows: Vec<AnyElement> = vec![];
        if earlier > 0 {
            rows.push(
                div()
                    .id("ai-earlier")
                    .test_support()
                    .text_size(px(11.5))
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .child(format!("Show {earlier} earlier"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.shown += SHOWN;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let chevron = |open: bool| Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(11.)).text_color(muted.opacity(0.7));
        let file_chip = |id: SharedString, name: &str, lines: Option<(u32, u32)>, cx: &App| {
            h_flex()
                .id(id)
                .test_support()
                .flex_none()
                .h(px(19.))
                .px(px(5.))
                .gap(px(4.))
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.foreground.opacity(0.1))
                .bg(theme.foreground.opacity(0.04))
                .text_size(px(11.5))
                .text_color(theme.foreground.opacity(0.85))
                .child(crate::file_icon::badge(name, px(11.), cx))
                .child(name.to_string())
                .when_some(lines, |el, (a, r)| {
                    el.when(a > 0, |el| el.child(div().font_family(theme.mono_font_family.clone()).text_color(palette::emerald(cx)).child(format!("+{a}"))))
                        .when(r > 0, |el| el.child(div().font_family(theme.mono_font_family.clone()).text_color(palette::red(cx)).child(format!("−{r}"))))
                })
        };
        for (n, view) in views.into_iter().enumerate() {
            let el: AnyElement = match view {
                View::User { key, text, context: chips, ask, images, at, aside, open } => {
                    let long = text.lines().count() > 8 || text.len() > 600;
                    let group = SharedString::from(format!("ai-user-{n}"));
                    let copy = text.clone();
                    let (k_restore, k_edit, k_more) = (key.clone(), key.clone(), key.clone());
                    let action = |id: &'static str, icon: Icon, label: &'static str| {
                        h_flex()
                            .id((id, n))
                            .test_support()
                            .gap(px(3.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child(icon.size(px(11.)))
                            .child(label)
                    };
                    v_flex()
                        .id(("ai-user", n))
                        .test_support()
                        .group(group.clone())
                        .pl(px(10.))
                        .py(px(2.))
                        .gap(px(3.))
                        .border_l_2()
                        .border_color(if aside { muted.opacity(0.4) } else { ember })
                        .child(div().text_size(ui).line_height(relative(1.45)).text_color(theme.foreground).when(long && !open, |el| el.line_clamp(8)).child(text))
                        .when(long, |el| {
                            el.child(
                                div()
                                    .id(("ai-user-more", n))
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .cursor_pointer()
                                    .child(if open { "Show less" } else { "Show more" })
                                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&k_more, cx))),
                            )
                        })
                        // One quiet line under it: what it carried, when it went, and (on hover)
                        // Restore, Edit and Copy.
                        .child(
                            h_flex()
                                .min_h(px(16.))
                                .gap(px(6.))
                                .flex_wrap()
                                .text_size(px(11.))
                                .text_color(muted)
                                .when(ask, |el| el.child(h_flex().gap(px(3.)).text_color(palette::sky(cx)).child(Icon::new(crate::assets::Lucide::MessageCircleQuestionMark).size(px(11.))).child("Ask")))
                                .children(chips.into_iter().map(|c| h_flex().gap(px(3.)).child(crate::file_icon::badge(c.split(':').next().unwrap_or(&c), px(10.), cx)).child(c)))
                                // What it carried: a click previews the images.
                                .when(!images.is_empty(), |el| {
                                    let count = images.len();
                                    el.child(
                                        h_flex()
                                            .id(("ai-user-images", n))
                                            .test_support()
                                            .gap(px(3.))
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(theme.foreground))
                                            .child(Icon::new(crate::assets::Lucide::Image).size(px(11.)))
                                            .child(format!("{count} image{}", if count == 1 { "" } else { "s" }))
                                            .on_click(move |_, window, cx| crate::image_preview::open(images.clone(), 0, window, cx)),
                                    )
                                })
                                .children(at.map(|at| div().text_color(muted.opacity(0.7)).child(crate::time::clock(at))))
                                .child(
                                    h_flex()
                                        .gap(px(8.))
                                        .text_color(muted.opacity(0.85))
                                        .invisible()
                                        .group_hover(group, |s| s.visible())
                                        .when(!aside && !busy, |el| {
                                            el.child(action("ai-restore", Icon::new(crate::assets::Lucide::Undo2), "Restore").on_click(cx.listener(move |this, _, _, cx| this.open_confirm(Ask::Rewind, k_restore.clone(), cx))))
                                                .child(action("ai-edit", Icon::new(crate::assets::Lucide::Pencil), "Edit").on_click(cx.listener(move |this, _, _, cx| {
                                                    let Some(thread) = this.current.clone() else { return };
                                                    let item = k_edit.clone();
                                                    let msg = this.workspace.read(cx).live.get(&thread).and_then(|l| l.items.position(&item).and_then(|p| match &l.items[p] {
                                                        Item::User { text, images, .. } => Some((text.clone(), images.iter().map(PathBuf::from).collect::<Vec<_>>())),
                                                        _ => None,
                                                    }));
                                                    if let Some((text, images)) = msg {
                                                        this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::ComposeIn { scope: Scope::Ide, thread, text, images, edit: Some(item) }));
                                                    }
                                                })))
                                        })
                                        .child(action("ai-copy", Icon::new(IconName::Copy), "Copy").on_click(move |_, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                            window.push_notification("Copied", cx);
                                        })),
                                ),
                        )
                        .children(self.confirm_card(&key, cx))
                        .into_any_element()
                }
                View::Wake { key, summary, text, open } => v_flex()
                    .child(
                        h_flex()
                            .id(("ai-wake", n))
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(muted)
                            .cursor_pointer()
                            .child(Icon::new(crate::assets::Lucide::CornerDownRight).size(px(11.)))
                            .child(div().min_w_0().truncate().child(summary))
                            .child(chevron(open))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&key, cx))),
                    )
                    .when(open, |el| el.child(div().pl(px(17.)).text_size(px(11.5)).text_color(muted).child(text)))
                    .into_any_element(),
                View::Thought { key, text, secs, open } => {
                    let doc = open.then(|| self.doc(&key, &text, cx));
                    v_flex()
                        .child(
                            h_flex()
                                .id(("ai-thought", n))
                                .test_support()
                                .gap(px(5.))
                                .text_size(px(12.))
                                .text_color(muted)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child(chevron(open))
                                .child(match secs {
                                    Some(s) => format!("Thought for {s}s"),
                                    None => "Thought".to_string(),
                                })
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle(&key, cx))),
                        )
                        .when_some(doc, |el, doc| el.child(div().ml(px(5.)).pl(px(10.)).border_l_1().border_color(theme.foreground.opacity(0.1)).child(crate::md::thought(&doc, cwd.clone(), folder, px(12.), cx))))
                        .into_any_element()
                }
                View::Explore { key, parts, rows: calls, running, open } => v_flex()
                    .child(
                        h_flex()
                            .id(("ai-explore", n))
                            .test_support()
                            .gap(px(5.))
                            .flex_wrap()
                            .text_size(px(12.))
                            .text_color(muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child(if running { Spinner::new().xsmall().color(muted).into_any_element() } else { chevron(open).into_any_element() })
                            .children(parts.into_iter().enumerate().map(|(i, p)| match p {
                                Part::Text(t) => div().child(t).into_any_element(),
                                Part::File(f) => file_chip(SharedString::from(format!("ai-read-{n}-{i}")), &f, None, cx).into_any_element(),
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&key, cx))),
                    )
                    .when(open, |el| {
                        el.child(v_flex().ml(px(5.)).pl(px(10.)).border_l_1().border_color(theme.foreground.opacity(0.1)).gap(px(2.)).children(calls.into_iter().map(|(text, _, status)| {
                            h_flex()
                                .gap(px(6.))
                                .text_size(px(11.5))
                                .text_color(muted)
                                .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).child(text))
                                .when(matches!(status, ToolStatus::Failed | ToolStatus::Denied), |el| el.child(Icon::new(IconName::CircleX).size(px(11.)).text_color(palette::red(cx))))
                        })))
                    })
                    .into_any_element(),
                View::Edits { files, running } => h_flex()
                    .id(("ai-edits", n))
                    .test_support()
                    .gap(px(5.))
                    .flex_wrap()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(if running { Spinner::new().xsmall().color(muted).into_any_element() } else { Icon::new(crate::assets::Lucide::Pencil).size(px(11.)).text_color(muted.opacity(0.8)).into_any_element() })
                    .child("Edited")
                    .children(files.into_iter().enumerate().map(|(i, f)| {
                        let ws = self.workspace.clone();
                        let path = f.path.clone();
                        file_chip(SharedString::from(format!("ai-edit-{n}-{i}")), &f.name, f.lines, cx)
                            .cursor_pointer()
                            .hover(|s| s.border_color(ember.opacity(0.6)))
                            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_editor(path.clone(), None, cx)))
                    }))
                    .into_any_element(),
                View::Command { key, command, tail, output, status, open } => {
                    let status_el = match status {
                        ToolStatus::Running => Spinner::new().xsmall().color(muted).into_any_element(),
                        ToolStatus::Done => Icon::new(IconName::Check).size(px(11.)).text_color(palette::emerald(cx)).into_any_element(),
                        ToolStatus::Failed | ToolStatus::Denied => Icon::new(IconName::CircleX).size(px(11.)).text_color(palette::red(cx)).into_any_element(),
                    };
                    let ws = self.workspace.clone();
                    let has_tail = !tail.is_empty();
                    v_flex()
                        .id(("ai-command", n))
                        .test_support()
                        .w_full()
                        .rounded(px(7.))
                        .border_1()
                        .border_color(theme.foreground.opacity(0.09))
                        .bg(theme.foreground.opacity(0.025))
                        .overflow_hidden()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(11.5))
                        .child(
                            h_flex()
                                .id(("ai-command-head", n))
                                .gap(px(6.))
                                .px(px(8.))
                                .py(px(4.))
                                .text_color(muted)
                                .cursor_pointer()
                                .child(div().flex_1().min_w_0().truncate().child(format!("$ {command}")))
                                .child(status_el)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle(&key, cx))),
                        )
                        .when(has_tail || open, |el| {
                            el.child(
                                v_flex()
                                    .px(px(8.))
                                    .py(px(4.))
                                    .border_t_1()
                                    .border_color(theme.foreground.opacity(0.07))
                                    .text_color(theme.foreground.opacity(0.7))
                                    .map(|el| match output {
                                        Some(full) => el.child(div().id(("ai-command-out", n)).max_h(px(240.)).overflow_y_scroll().whitespace_normal().child(full)),
                                        None => el.children(tail.into_iter().map(|l| div().truncate().child(l))),
                                    })
                                    .child(
                                        h_flex()
                                            .id(("ai-command-terminal", n))
                                            .test_support()
                                            .pt(px(2.))
                                            .gap(px(4.))
                                            .font_family(theme.font_family.clone())
                                            .text_size(px(11.))
                                            .text_color(muted.opacity(0.8))
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(theme.foreground))
                                            .child("Open in Terminal")
                                            .child(Icon::new(crate::assets::Lucide::ArrowUpRight).size(px(10.)))
                                            .on_click(move |_, _, cx| ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(crate::workspace::PanelTool::Terminal)))),
                                    ),
                            )
                        })
                        .into_any_element()
                }
                View::SubAgent { label, detail, state, child, agent } => {
                    let dot = match state {
                        TaskState::Running => palette::sky(cx),
                        TaskState::NeedsYou => palette::amber(cx),
                        TaskState::Done => palette::emerald(cx),
                        TaskState::Failed => palette::red(cx),
                        TaskState::Cancelled => muted.opacity(0.7),
                    };
                    let ws = self.workspace.clone();
                    h_flex()
                        .id(("ai-subagent", n))
                        .test_support()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .child(div().relative().flex_none().size(px(16.)).child(match &agent {
                            Some(a) => crate::ui::agent_logo(a, px(16.), cx),
                            None => Icon::new(crate::assets::Lucide::Users).size(px(14.)).text_color(muted).into_any_element(),
                        }).child(div().absolute().right(px(-2.)).bottom(px(-2.)).size(px(7.)).rounded_full().bg(dot)))
                        .child(v_flex().flex_1().min_w_0().child(div().truncate().child(label)).child(div().truncate().text_size(px(11.)).text_color(muted).child(detail)))
                        .when_some(child, |el, child| {
                            el.child(
                                Button::new(("ai-subagent-open", n))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(crate::assets::Lucide::SquareArrowOutUpRight).text_color(muted))
                                    .tooltip("Open its thread in a window")
                                    .on_click(move |_, _, cx| crate::thread_window::open(ws.clone(), &child, cx)),
                            )
                        })
                        .into_any_element()
                }
                View::Assistant { key, text, live } => {
                    let doc = self.doc(&key, &text, cx);
                    div().id(("ai-answer", n)).test_support().child(crate::md::answer(&doc, cwd.clone(), folder, ui, live, cx).motion(crate::md::streaming())).into_any_element()
                }
                View::TurnEnd { ix, key } => {
                    let changes: Option<Arc<TurnChanges>> = self.workspace.update(cx, |ws, cx| ws.load_turn_changes(&thread, ix, cx));
                    let card = changes.map(|c| {
                        let ws = self.workspace.read(cx);
                        let review = ws.review(&thread);
                        let pending = |p: &str| review.is_some_and(|r| r.is_pending(p));
                        let root = match (&c.counted, &cwd) {
                            (trek_core::changes::Counted::Checkpoints, Some(cwd)) => crate::workspace::as_given(&c.root, cwd),
                            _ => c.root.clone(),
                        };
                        cards::changes(ix, &c, &root, &pending, busy, &self.workspace, &thread, cx)
                    });
                    let last = Some(ix) == last_end;
                    v_flex().gap(px(6.)).children(card).child(self.turn_actions(ix, &key, n, last, busy, cx)).into_any_element()
                }
                View::Notice(text) if text.is_empty() => div().into_any_element(),
                View::Notice(text) => div().text_size(px(11.5)).text_color(muted).child(text).into_any_element(),
                View::Error(text) => div()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(palette::red(cx).opacity(0.45))
                    .bg(palette::red(cx).opacity(0.07))
                    .text_size(px(12.))
                    .child(text)
                    .into_any_element(),
                View::Limit(text) => h_flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette::amber(cx))
                    .child(Icon::new(crate::assets::Lucide::Gauge).size(px(12.)))
                    .child(div().min_w_0().truncate().child(text))
                    .into_any_element(),
                View::Handoff(text) => h_flex()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .text_color(muted)
                    .child(Icon::new(crate::assets::Lucide::ArrowLeftRight).size(px(11.)))
                    .child(text)
                    .into_any_element(),
            };
            rows.push(el);
        }
        if let Some(w) = self.working(cx) {
            rows.push(w);
        }
        let empty = rows.is_empty();
        let card = self.request_card(window, cx);
        v_flex()
            .id("ai-transcript-view")
            .size_full()
            .child(
                div()
                    .id("ai-transcript")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(v_flex().w_full().px(px(14.)).pt(px(12.)).pb(px(14.)).gap(px(10.)).children(rows))
                    .when(empty && loading, |el| el.child(h_flex().w_full().justify_center().pt(px(24.)).child(Spinner::new()))),
            )
            .children(card)
            .into_any_element()
    }
}

#[cfg(test)]
impl AiTranscript {
    /// The blocks as text: "user: <text> [ask] [chips]", "thought", "explore: Read main.rs ·
    /// searched “x”", "edits: cli.rs +9 −1, main.rs +5 −2", "command: cargo test", "subagent",
    /// "assistant", "end", "notice", "error", "limit", "handoff", "wake".
    pub(crate) fn describe(&self, cx: &App) -> Vec<String> {
        self.views(cx)
            .0
            .into_iter()
            .map(|v| match v {
                View::User { text, context, ask, .. } => format!("user: {text}{}{}", if ask { " [ask]" } else { "" }, if context.is_empty() { String::new() } else { format!(" [{}]", context.join(", ")) }),
                View::Wake { .. } => "wake".into(),
                View::Thought { .. } => "thought".into(),
                View::Explore { parts, .. } => format!(
                    "explore: {}",
                    parts.into_iter().map(|p| match p {
                        Part::Text(t) | Part::File(t) => t,
                    }).collect::<Vec<_>>().join(" ")
                ),
                View::Edits { files, .. } => format!(
                    "edits: {}",
                    files.iter().map(|f| match f.lines {
                        Some((a, r)) => format!("{} +{a} −{r}", f.name),
                        None => f.name.clone(),
                    }).collect::<Vec<_>>().join(", ")
                ),
                View::Command { command, .. } => format!("command: {command}"),
                View::SubAgent { .. } => "subagent".into(),
                View::Assistant { .. } => "assistant".into(),
                View::TurnEnd { .. } => "end".into(),
                View::Notice(_) => "notice".into(),
                View::Error(_) => "error".into(),
                View::Limit(_) => "limit".into(),
                View::Handoff(_) => "handoff".into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Block, Part, blocks, explore_parts};
    use trek_core::store::{Item, ToolStatus};

    fn tool(id: &str, title: &str, detail: &str) -> Item {
        Item::Tool { id: id.into(), title: title.into(), detail: detail.into(), output: String::new(), status: ToolStatus::Done }
    }

    #[test]
    fn calls_fold_by_kind_and_thoughts_join_the_looking_around() {
        let items = vec![
            Item::User { text: "go".into(), images: vec![], at: None, resume: None, aside: false },
            Item::Reasoning { text: "First, look.".into() },
            tool("a", "Read", "src/a.rs"),
            Item::Reasoning { text: "And search.".into() },
            tool("b", "Search", "parse"),
            tool("c", "Edit", "src/a.rs"),
            tool("d", "Write", "src/b.rs"),
            tool("e", "Run command", "cargo test"),
            Item::Reasoning { text: String::new() },
            Item::Assistant { text: "Done.".into() },
            Item::TurnEnd { at: 1, took_secs: 3 },
        ];
        assert_eq!(
            blocks(&items, None),
            vec![Block::User(0), Block::Thought(vec![1]), Block::Explore(vec![2, 3, 4]), Block::Edits(vec![5, 6]), Block::Command(7), Block::Assistant(9), Block::TurnEnd(10)]
        );
        // A thought still streaming has no block of its own yet.
        assert_eq!(blocks(&items[..2], Some(1)), vec![Block::User(0)]);
    }

    #[test]
    fn looking_around_reads_as_one_line() {
        let calls = |c: &[(&str, &str)]| c.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let parts = explore_parts(&calls(&[("Read", "/p/src/editor.rs"), ("Read", "/p/src/root.rs"), ("Search", "files_epoch")]), Some(std::path::Path::new("/p")));
        assert_eq!(parts, vec![Part::Text("Read".into()), Part::File("editor.rs".into()), Part::File("root.rs".into()), Part::Text("·".into()), Part::Text("searched “files_epoch”".into())]);
        let many = explore_parts(&calls(&[("Read", "a"), ("Read", "b"), ("Read", "c"), ("Read", "d")]), None);
        assert_eq!(many, vec![Part::Text("Read".into()), Part::Text("4 files".into())]);
        assert_eq!(explore_parts(&calls(&[("Search", "x")]), None), vec![Part::Text("Searched “x”".into())]);
    }
}
