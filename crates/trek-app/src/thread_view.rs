//! The transcript: user turns, streaming markdown answers, collapsible reasoning and tool rows,
//! plus the cards the agent puts to the user (approvals, questions, plans). The working bar above
//! the composer is its own view (`working_bar`).

use crate::activity::{Place, ToolKind, kind_icon, placement, summarize, tool_kind};
use crate::palette;
use crate::workspace::{ForkAt, ItemRef, Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_agents::Decision;
use trek_core::checkpoint::{Change, FileChange};
use trek_core::orchestrate as orch;
use trek_core::store::{Item, ToolStatus};
use crate::workspace::TaskState;

/// How long a message a search result led to stays tinted (it holds, then fades).
const FLASH: std::time::Duration = std::time::Duration::from_millis(2200);

/// A transcript row. Rows point into the thread's items (`ix`, their position when the rows were
/// built); text is read when a row is drawn, so building rows for a long thread copies nothing.
/// Rows that open and close, and answers, carry their item's stable id (`key`): what's expanded
/// and each markdown document follow the item, not its position, which shifts when empty
/// thoughts are dropped at the end of a turn.
#[derive(Clone)]
enum Row {
    User { ix: usize, key: SharedString, open: bool },
    /// End of a response: copy the whole answer, when it finished, how long it took.
    TurnEnd { ix: usize },
    Assistant { ix: usize, key: SharedString },
    Reasoning { ix: usize, key: SharedString, open: bool },
    Tool { ix: usize, key: SharedString, open: bool, activity: Option<SharedString> },
    /// Consecutive tool calls folded into one summary line ("Ran 3 commands and edited 2 files").
    ToolGroup { ix: usize, key: SharedString, summary: SharedString, kind: ToolKind, running: bool, open: bool, tools: Vec<Row> },
    /// A sub-agent: one Trek runs as a thread of its own (`delegate_task`), or the agent's own
    /// (Claude's Task, Codex's agents). `open`: its answer shows in full.
    SubAgent { ix: usize, key: SharedString, open: bool },
    /// Trek woke the agent with what its sub-agents came back with (a message the user didn't write).
    Wake { ix: usize, key: SharedString, open: bool },
    Notice { ix: usize },
    Error { ix: usize },
    /// The agent stopped at a usage limit.
    Limit { ix: usize },
    /// The thread moved to another agent, which picks up from a recap.
    Handoff { ix: usize },
}

impl Row {
    /// The transcript item the row shows (a group's first member).
    fn item(&self) -> usize {
        match self {
            Row::User { ix, .. }
            | Row::TurnEnd { ix }
            | Row::Assistant { ix, .. }
            | Row::Reasoning { ix, .. }
            | Row::Tool { ix, .. }
            | Row::ToolGroup { ix, .. }
            | Row::SubAgent { ix, .. }
            | Row::Wake { ix, .. }
            | Row::Notice { ix }
            | Row::Error { ix }
            | Row::Limit { ix }
            | Row::Handoff { ix } => *ix,
        }
    }
}

/// The transcript as rows, and which row shows each item (items without a row of their own,
/// like a live thought, map to the row after them).
struct Rows {
    rows: Vec<Row>,
    item_row: Vec<usize>,
}

impl Rows {
    fn row_of(&self, item: usize) -> Option<usize> {
        row_of(&self.item_row, self.rows.len(), item)
    }
}

fn row_of(item_row: &[usize], rows: usize, item: usize) -> Option<usize> {
    let last = rows.checked_sub(1)?;
    Some(item_row.get(item).copied().unwrap_or(last).min(last))
}

/// What a row shows, by item position.
#[derive(Debug, PartialEq)]
enum Slot {
    /// A message, notice or turn end: a row of its own.
    Item(usize),
    /// Consecutive tool calls and finished thoughts, folded into one row.
    Group(Vec<usize>),
}

/// How a transcript lays out as rows, and which row shows each item. A live thought (the
/// working bar shows it) and an empty one have no row: they map to the row after them, the
/// group they sit in, or past the end when nothing follows.
fn layout(items: &[Item], live_reasoning: Option<usize>) -> (Vec<Slot>, Vec<usize>) {
    let mut slots = Vec::new();
    let mut item_row = vec![0; items.len()];
    let mut group: Vec<usize> = Vec::new();
    let flush = |group: &mut Vec<usize>, slots: &mut Vec<Slot>, item_row: &mut [usize]| {
        if group.is_empty() {
            return;
        }
        for &ix in group.iter() {
            item_row[ix] = slots.len();
        }
        slots.push(Slot::Group(std::mem::take(group)));
    };
    // Rows without a row of their own map to the row that comes next (resolved as it comes).
    let mut hidden: Vec<usize> = Vec::new();
    let resolve = |hidden: &mut Vec<usize>, item_row: &mut [usize], row: usize| {
        for h in hidden.drain(..) {
            item_row[h] = row;
        }
    };
    for (ix, item) in items.iter().enumerate() {
        match item {
            Item::Tool { .. } => match placement(item) {
                Place::Group => {
                    // The group's row is the next one pushed.
                    resolve(&mut hidden, &mut item_row, slots.len());
                    group.push(ix);
                }
                Place::Hidden => hidden.push(ix),
                Place::Own => {
                    flush(&mut group, &mut slots, &mut item_row);
                    resolve(&mut hidden, &mut item_row, slots.len());
                    item_row[ix] = slots.len();
                    slots.push(Slot::Item(ix));
                }
            },
            Item::Reasoning { text } => {
                item_row[ix] = slots.len();
                if live_reasoning != Some(ix) && !text.trim().is_empty() {
                    group.push(ix);
                }
            }
            _ => {
                flush(&mut group, &mut slots, &mut item_row);
                resolve(&mut hidden, &mut item_row, slots.len());
                item_row[ix] = slots.len();
                slots.push(Slot::Item(ix));
            }
        }
    }
    flush(&mut group, &mut slots, &mut item_row);
    resolve(&mut hidden, &mut item_row, slots.len());
    (slots, item_row)
}

pub struct ThreadView {
    workspace: Entity<Workspace>,
    /// The main window's transcript follows its route; a thread window's shows one thread.
    scope: Scope,
    scroller: Entity<MessageScrollerState>,
    current: Option<String>,
    revision: u64,
    count: usize,
    /// Markdown documents by item id. Built when a row is first drawn, so opening a long thread
    /// doesn't parse all of it.
    md: HashMap<String, Markdown>,
    /// How many times the transcript has been drawn (`Markdown::drawn`).
    draws: u64,
    /// Items (by id) opened by the user: long messages, thoughts, tool groups, tool output.
    expanded: HashSet<String>,
    /// Masked fields for the question card's secret questions, in order, and the request they
    /// were last shown for (a new card starts them empty).
    secrets: (Option<String>, Vec<Entity<InputState>>),
    /// Rendered plan for the plan card on screen (request id, markdown, redraw when it's parsed).
    plan_md: Option<(String, Entity<TextViewState>, Subscription)>,
    /// Rows built for (thread, transcript revision, expansion state, where the transcript stops);
    /// rebuilt only when one changes.
    rows_cache: std::cell::RefCell<Option<((Option<String>, u64, u64, Option<usize>), std::rc::Rc<Rows>)>>,
    /// Where the transcript stops for now: the live group of tool calls (and anything after it)
    /// shows in the working bar instead (`activity::transcript_end`).
    end: Option<usize>,
    /// The live group last opened from the working bar (`Workspace::open_live_group`), once its
    /// row here is open.
    opened: Option<crate::activity::Opened>,
    /// Where the transcript's last row ends on screen, as last laid out (`None` when it's out of
    /// view): the working bar sits just under it when the transcript doesn't reach the bar.
    pub tail: std::rc::Rc<std::cell::Cell<Option<Pixels>>>,
    expanded_gen: u64,
    /// The last `Workspace::reveal` request handled.
    revealed: u64,
    /// The row a reveal scrolled to, tinted for a moment: (row, request).
    flash: Option<(usize, u64)>,
    /// Clears `flash` once its fade is over (a row scrolled back into view would replay it).
    _flash_timer: Option<Task<()>>,
    /// (transcript, UI) font sizes the rows were measured at; a change remeasures every row.
    fonts: (f32, f32),
    /// The window is frontmost. The search-result tint fades only then.
    active: bool,
    /// What the last render showed, to skip workspace changes that don't touch the transcript.
    shown: Option<Shown>,
    /// The confirmation open over a message or a turn's footer (rewind, undo, retry).
    confirm: Option<Confirm>,
    /// Redraws while a sub-agent in this thread works: its time ticks and its dot breathes. The
    /// flag: at the working bar's rate, for an opened sub-agent's live activity (its shimmer).
    _ticker: Option<(bool, Task<()>)>,
    /// Lines each opened sub-agent's live activity showed when last measured, by its row's key:
    /// its row is measured again when that changes.
    activity_lines: HashMap<String, usize>,
    _subscriptions: Vec<Subscription>,
}

/// What a confirmation asks to do.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// Rewind to just before a message.
    Rewind,
    /// Undo a turn.
    Undo,
    /// Retry a turn, with another model when given.
    Retry(Option<String>),
}

struct Confirm {
    ask: Ask,
    /// The item it hangs from: the message (rewind), or the turn's footer.
    anchor: String,
    /// The message whose checkpoint the files go back to (a turn's first).
    message: String,
    files: Files,
    /// Also put the files back.
    restore: bool,
    _check: Option<Task<()>>,
}

/// What restoring the files would do.
enum Files {
    Checking,
    Changes(Vec<FileChange>),
    /// There's nothing to restore them from, and why.
    Unavailable(String),
    Failed(String),
}

/// Files a confirmation lists before "and N more".
const FILES_SHOWN: usize = 8;

/// How many parsed markdown documents a transcript keeps at most: each is an entity with its
/// text laid out, and a day's thread has thousands of answers. Scrolled far past, a document is
/// dropped and parsed again when its row next comes into view.
const MD_KEPT: usize = 400;

struct Markdown {
    state: Entity<TextViewState>,
    /// The text the document holds. Text that extends it is appended (parsed incrementally, as
    /// it streams); anything else replaces it.
    text: String,
    /// Where the item was last seen, to find it again without a search.
    ix: usize,
    /// The draw (`ThreadView::draws`) its row was last part of: the documents drawn longest
    /// ago go first when there are too many (`MD_KEPT`).
    drawn: u64,
    /// Parsing finishes in the background and streamed text fades in; this view is cached, so it
    /// re-renders when the document says so.
    _changed: Subscription,
}

/// The workspace state the transcript is drawn from.
#[derive(Clone, PartialEq)]
struct Shown {
    thread: Option<String>,
    /// The main window is on a new thread (its backdrop may show behind an empty transcript).
    draft: bool,
    revision: u64,
    agent: Option<trek_core::AgentId>,
    loading: bool,
    /// The question card's picks, which change without a new transcript revision.
    picks: HashMap<(String, usize), Vec<String>>,
    /// Where the transcript stops (`ThreadView::end`), which moves without a new revision too.
    end: Option<usize>,
    appearance: trek_core::settings::Appearance,
    /// Where its sub-agents stand: they move on without a new transcript revision.
    tasks: Vec<(String, TaskState)>,
    /// Its project's colour (path chips' folders), which can change in another window.
    folder: Option<Hsla>,
}

/// One side of a handoff divider: the model with its agent's name ("Claude Opus 5.5", "Codex
/// Sol"), as a model's name alone can be ambiguous; the agent's alone without one.
pub(crate) fn handoff_name(agent: &trek_core::AgentId, model: Option<&str>) -> String {
    let agent_name = match agent {
        trek_core::AgentId::ClaudeCode => "Claude".to_string(),
        other => other.display_name(),
    };
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else { return agent_name };
    let lower = model.to_lowercase();
    // "Mock Swift" from the Mock agent, "GPT-5.6-Sol" from Codex: the model says whose it is.
    let named = agent_name.split_whitespace().any(|w| w.len() > 2 && lower.contains(&w.to_lowercase())) || (*agent == trek_core::AgentId::Codex && lower.starts_with("gpt"));
    if named { model.to_string() } else { format!("{agent_name} {model}") }
}

/// What a usage-limit row says after "Usage limit reached": "5-hour limit resets 3:27 PM".
pub(crate) fn limit_when(scope: &trek_core::limit::LimitScope, resets_at: Option<i64>, now: i64) -> String {
    match resets_at {
        Some(r) if r > now => format!("{} resets {}", scope.label(), crate::time::reset_clock(r, now)),
        Some(r) => format!("{} reset {}", scope.label(), crate::time::reset_clock(r, now)),
        None => scope.label(),
    }
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(false, cx)),
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| match event {
                WorkspaceEvent::Transcript { id, appended } if this.current.as_ref() == Some(id) => this.sync(*appended, cx),
                WorkspaceEvent::TurnChanges { id, end } if this.current.as_ref() == Some(id) => this.changes_in(end, cx),
                _ => {}
            }),
            cx.observe(&scroller, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active() || crate::mascot::force_active();
                if this.flash.is_some() {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            workspace,
            scope,
            scroller,
            current: None,
            revision: 0,
            count: 0,
            md: HashMap::new(),
            draws: 0,
            expanded: HashSet::new(),
            secrets: (None, vec![]),
            plan_md: None,
            rows_cache: Default::default(),
            end: None,
            opened: None,
            tail: Default::default(),
            expanded_gen: 0,
            revealed: 0,
            flash: None,
            _flash_timer: None,
            fonts: (0., 0.),
            active: window.is_window_active() || crate::mascot::force_active(),
            shown: None,
            confirm: None,
            _ticker: None,
            activity_lines: HashMap::new(),
            _subscriptions: subscriptions,
        };
        this.sync(false, cx);
        this
    }

    fn animate(&self, _: &Window, cx: &App) -> bool {
        self.active && !self.workspace.read(cx).settings.appearance.reduce_motion
    }

    /// Bring row state in line with the workspace transcript without rebuilding everything.
    /// `appended`: only text was added to the messages already streaming. Their documents redraw
    /// this view once the new text is parsed, so it doesn't redraw before that (it would show the
    /// same text again).
    fn sync(&mut self, appended: bool, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let id = ws.thread_id_in(&self.scope).map(str::to_string);
        let switched = id != self.current;
        let live = id.as_ref().and_then(|id| ws.live.get(id));
        let revision = live.map_or(0, |l| l.revision);
        let opened = live.and_then(|l| l.opened.clone());
        let shown = Shown {
            thread: id.clone(),
            draft: ws.is_draft_in(&self.scope),
            revision,
            agent: id.as_ref().and_then(|id| ws.thread(id)).map(|t| t.agent.clone()),
            loading: live.is_some_and(|l| l.loading),
            picks: live.map(|l| l.picks.clone()).unwrap_or_default(),
            end: id.as_ref().and_then(|id| crate::activity::transcript_end(ws, id)),
            appearance: ws.settings.appearance.clone(),
            tasks: id.as_ref().map(|id| ws.children(id).into_iter().map(|t| (t.id.clone(), ws.task_state(&t.id))).collect()).unwrap_or_default(),
            folder: id.as_ref().and_then(|id| ws.thread(id)).and_then(|t| ws.thread_project_tint(t, cx)),
        };
        let end = shown.end;
        let ticking = ws.any_task_live_in(&self.scope);
        // Text added to messages already streaming moves no row: the rows built for the last
        // revision stand for this one (building them reads every item, and this runs for each
        // batch of tokens).
        if appended && !switched && end == self.end {
            if let Some((key, rows)) = self.rows_cache.borrow_mut().as_mut() {
                if key.0 == self.current && key.1 == self.revision && live.is_some_and(|l| l.items.len() == rows.item_row.len()) {
                    key.1 = revision;
                }
            }
        }
        // Only the documents already built need updating: they follow their item to where it moved
        // and to its new text (while streaming, a longer tail). Those whose item left the
        // transcript go (`None`).
        let mut docs: Vec<(String, Option<(usize, Option<String>)>)> = Vec::new();
        if let Some(l) = live.filter(|_| !switched && revision != self.revision) {
            for (key, m) in &self.md {
                let ix = if l.items.id_at(m.ix) == Some(key.as_str()) { Some(m.ix) } else { l.items.position(key) };
                let now = ix.and_then(|ix| match &l.items[ix] {
                    Item::Assistant { text } | Item::Reasoning { text } | Item::Tool { output: text, .. } => Some((ix, text)),
                    _ => None,
                });
                match now {
                    None => docs.push((key.clone(), None)),
                    Some((ix, text)) if ix != m.ix || *text != m.text => docs.push((key.clone(), Some((ix, (*text != m.text).then(|| text.clone()))))),
                    Some(_) => {}
                }
            }
        }
        let fonts = (ws.settings.appearance.transcript_font_size(), ws.settings.appearance.ui_font_size());
        if fonts != self.fonts {
            self.fonts = fonts;
            let count = self.count;
            if count > 0 {
                self.scroller.update(cx, |s, cx| _ = s.remeasure_items(0..count, cx));
            }
        }
        if switched {
            self.current = id;
            self.confirm = None;
            self.md.clear();
            self.expanded.clear();
            self.expanded_gen += 1;
            self.count = 0;
            self.revision = 0;
            self.flash = None;
            self.scroller.update(cx, |s, cx| s.reset(0, cx));
        }
        let quiet = appended && !switched && self.shown.as_ref().is_some_and(|s| Shown { revision, ..s.clone() } == shown);
        // A sub-agent that moved on may have changed its row's height (its answer's preview).
        let tasks_moved = !switched && (revision != self.revision || self.shown.as_ref().is_some_and(|s| s.tasks != shown.tasks));
        self.sync_ticker(ticking, cx);
        if self.shown.as_ref() != Some(&shown) {
            self.shown = Some(shown);
            if !quiet {
                cx.notify();
            }
        }
        if revision == self.revision && !switched && end == self.end {
            if tasks_moved {
                self.remeasure_sub_agents(cx);
            }
            self.reveal(cx);
            return;
        }
        self.revision = revision;
        self.end = end;
        // A live group opened from the bar: its row opens here, with the call clicked.
        let mut follow = false;
        if opened != self.opened {
            self.opened = opened.clone();
            if let Some(o) = opened {
                let rows = self.rows(cx);
                let group = self.workspace.read(cx).live.get(self.current.as_deref().unwrap_or_default()).and_then(|l| l.items.position(&o.first));
                if let Some(Row::ToolGroup { key, .. }) = group.and_then(|ix| rows.row_of(ix)).and_then(|r| rows.rows.get(r)) {
                    self.expanded.insert(key.to_string());
                }
                self.expanded.extend(o.call);
                self.expanded_gen += 1;
                follow = true;
            }
        }
        for (key, now) in docs {
            let Some((ix, text)) = now else {
                self.md.remove(&key);
                continue;
            };
            let Some(m) = self.md.get_mut(&key) else { continue };
            m.ix = ix;
            if let Some(text) = text {
                // Streaming appends (parsed incrementally); anything else replaces the document.
                match text.strip_prefix(m.text.as_str()) {
                    Some(tail) => m.state.update(cx, |s, cx| s.push_str(tail, cx)),
                    None => m.state.update(cx, |s, cx| s.set_text(&text, cx)),
                }
                m.text = text;
            }
        }
        let new_count = self.rows(cx).rows.len();
        let old = self.count;
        if quiet && new_count == old {
            return;
        }
        self.count = new_count;
        self.scroller.update(cx, |s, cx| {
            if switched {
                s.reset(new_count, cx);
                s.scroll_to_end(cx);
            } else if new_count < old {
                // Rows went from the end (a rewind, a group folded away): the rest stay where
                // they are on screen. Starting the list over would jump a reader up in the
                // history to its end, and have the next answer drag them along with it.
                if !s.splice(new_count..old, 0, cx) {
                    s.reset(new_count, cx);
                }
            } else if new_count > old {
                let _ = s.append(new_count - old, cx);
            }
            // Streaming only ever changes the tail; remeasure the last couple of rows (a few
            // more where rows went: what's left at the end may have regrouped).
            if new_count > 0 {
                let from = new_count.saturating_sub(if new_count < old { 6 } else { 2 });
                let _ = s.remeasure_items(from..new_count, cx);
            }
            if follow {
                s.scroll_to_end(cx);
            }
        });
        if tasks_moved {
            self.remeasure_sub_agents(cx);
        }
        self.reveal(cx);
        cx.notify();
    }

    /// Redraw on a ticker while a sub-agent works: fast enough for its dot to breathe while the
    /// window is in front (at the working bar's rate while one is open, its activity shimmering),
    /// once a second (for its time) otherwise.
    fn sync_ticker(&mut self, live: bool, cx: &mut Context<Self>) {
        if !live {
            self._ticker = None;
            self.activity_lines.clear();
            return;
        }
        let shimmer = !self.open_activity(cx).is_empty();
        if self._ticker.as_ref().is_some_and(|(s, _)| *s == shimmer) {
            return;
        }
        let task = cx.spawn(async move |this, cx| loop {
            let Ok(moving) = this.update(cx, |this, cx| {
                cx.notify();
                this.remeasure_activity(cx);
                this.active && !this.workspace.read(cx).settings.appearance.reduce_motion
            }) else {
                break;
            };
            let fps = if shimmer { crate::mascot::FPS } else { PULSE_FPS };
            let wait = if moving { std::time::Duration::from_millis(1000 / fps) } else { std::time::Duration::from_secs(1) };
            cx.background_executor().timer(wait).await;
        });
        self._ticker = Some((shimmer, task));
    }

    /// What the turn ending at item `end` (by id) changed is in, or may have moved: its footer's
    /// row is measured again, with or without its card.
    fn changes_in(&mut self, end: &str, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let Some(ix) = self.current.as_ref().and_then(|id| ws.live.get(id)).and_then(|l| l.items.position(end)) else { return };
        let rows = self.rows(cx);
        if let Some(row) = rows.row_of(ix).filter(|r| matches!(rows.rows.get(*r), Some(Row::TurnEnd { ix: i }) if *i == ix)) {
            self.scroller.update(cx, |s, cx| _ = s.remeasure_items(row..row + 1, cx));
        }
        cx.notify();
    }

    /// Opened sub-agent rows showing what their sub-agent is doing: (row, key, lines shown).
    fn open_activity(&self, cx: &App) -> Vec<(usize, String, usize)> {
        let ws = self.workspace.read(cx);
        let Some((thread, live)) = self.current.as_ref().and_then(|t| Some((t, ws.live.get(t)?))) else { return vec![] };
        let agent = ws.thread(thread).map(|t| t.agent.clone()).unwrap_or(trek_core::AgentId::ClaudeCode);
        self.rows(cx)
            .rows
            .iter()
            .enumerate()
            .filter_map(|(r, row)| match (row, row.item()) {
                (Row::SubAgent { key, open: true, .. }, ix) => match live.items.get(ix)? {
                    Item::Tool { id, detail, output, status, .. } => SubAgentRow::read(ws, thread, &agent, id, detail, output, *status).activity.map(|g| (r, key.to_string(), g.lines())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// Measure again the opened sub-agent rows whose activity grew or shrank since last time.
    fn remeasure_activity(&mut self, cx: &mut Context<Self>) {
        let open = self.open_activity(cx);
        let changed: Vec<usize> = open.iter().filter(|(_, key, n)| self.activity_lines.get(key) != Some(n)).map(|(r, ..)| *r).collect();
        self.activity_lines = open.into_iter().map(|(_, key, n)| (key, n)).collect();
        if changed.is_empty() {
            return;
        }
        self.scroller.update(cx, |s, cx| {
            for r in changed {
                let _ = s.remeasure_items(r..r + 1, cx);
            }
        });
    }

    /// Measure the sub-agent rows again (their answers' previews come and go).
    fn remeasure_sub_agents(&mut self, cx: &mut Context<Self>) {
        let rows: Vec<usize> = self.rows(cx).rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::SubAgent { .. })).map(|(i, _)| i).collect();
        if rows.is_empty() {
            return;
        }
        self.scroller.update(cx, |s, cx| {
            for r in rows {
                let _ = s.remeasure_items(r..r + 1, cx);
            }
        });
        cx.notify();
    }

    /// The markdown document for item `key` (at `ix`), built the first time its row is drawn.
    fn markdown(&mut self, key: &str, ix: usize, cx: &mut Context<Self>) -> Option<Entity<TextViewState>> {
        let drawn = self.draws;
        if let Some(m) = self.md.get_mut(key) {
            m.drawn = drawn;
            return Some(m.state.clone());
        }
        let ws = self.workspace.read(cx);
        let items = &ws.live.get(self.current.as_ref()?)?.items;
        let text = match (items.id_at(ix) == Some(key)).then(|| items.get(ix)).flatten()? {
            // A sub-agent's answer is markdown too.
            Item::Assistant { text } | Item::Reasoning { text } | Item::Tool { output: text, .. } => text.clone(),
            _ => return None,
        };
        let state = cx.new(|cx| TextViewState::markdown(&text, cx));
        let changed = cx.observe(&state, |_, _, cx| cx.notify());
        self.md.insert(key.to_string(), Markdown { state: state.clone(), text, ix, drawn, _changed: changed });
        if self.md.len() > MD_KEPT {
            // Down to three quarters, so this isn't done for every row scrolled into view.
            let mut by_age: Vec<(u64, String)> = self.md.iter().map(|(k, m)| (m.drawn, k.clone())).collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(self.md.len() - MD_KEPT * 3 / 4) {
                self.md.remove(&k);
            }
        }
        Some(state)
    }

    /// Scroll to the message `Workspace::reveal` asks for, once its thread is on screen and loaded.
    /// Search results open in the main window; a thread window on the same thread keeps its place.
    fn reveal(&mut self, cx: &mut Context<Self>) {
        if self.scope != Scope::Main {
            return;
        }
        let ws = self.workspace.read(cx);
        let Some(r) = ws.reveal.clone().filter(|r| r.seq > self.revealed) else { return };
        // Asked for another thread: the user has moved on before it loaded.
        if self.current.as_ref() != Some(&r.thread) {
            self.revealed = r.seq;
            return;
        }
        let Some(live) = ws.live.get(&r.thread).filter(|l| l.loaded && !l.loading) else { return };
        let item = match &r.item {
            ItemRef::Id(id) => live.items.position(id),
            ItemRef::Position(p) => Some(*p),
        };
        // A long message of yours shows ten lines until opened; the match may lie further down.
        let open = item.filter(|ix| matches!(live.items.get(*ix), Some(Item::User { .. }))).and_then(|ix| live.item_ids().get(ix).cloned());
        self.revealed = r.seq;
        if let Some(key) = open {
            if self.expanded.insert(key) {
                self.expanded_gen += 1;
            }
        }
        let Some(row) = item.and_then(|ix| self.rows(cx).row_of(ix)) else { return };
        self.flash = Some((row, r.seq));
        let seq = r.seq;
        self._flash_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |this, cx| {
                if this.flash.is_some_and(|(_, s)| s == seq) {
                    this.flash = None;
                    cx.notify();
                }
            });
        }));
        self.scroller.update(cx, |s, cx| {
            let _ = s.remeasure_items(row..row + 1, cx);
            let _ = s.scroll_to_item(row, cx);
        });
        cx.notify();
    }

    fn rows(&self, cx: &App) -> std::rc::Rc<Rows> {
        let revision = self.current.as_ref().and_then(|id| self.workspace.read(cx).live.get(id)).map(|l| l.revision).unwrap_or(0);
        // As of the last sync, so the rows match the count the scroller was given.
        let end = self.end;
        let key = (self.current.clone(), revision, self.expanded_gen, end);
        if let Some((k, rows)) = self.rows_cache.borrow().as_ref() {
            if *k == key {
                return rows.clone();
            }
        }
        let mut rows = self.build_rows(cx);
        if let Some(end) = end {
            let keep = rows.rows.iter().position(|r| r.item() >= end).unwrap_or(rows.rows.len());
            rows.rows.truncate(keep);
        }
        let rows = std::rc::Rc::new(rows);
        *self.rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    fn build_rows(&self, cx: &App) -> Rows {
        let ws = self.workspace.read(cx);
        let Some(live) = self.current.as_ref().and_then(|id| ws.live.get(id)) else { return Rows { rows: vec![], item_row: vec![] } };
        let ids = live.item_ids();
        let (slots, item_row) = layout(&live.items, live.reasoning);
        let rows = slots
            .into_iter()
            .map(|slot| match slot {
                Slot::Group(members) => {
                    let ix = members[0];
                    let key: SharedString = ids[ix].clone().into();
                    let mut kinds = Vec::with_capacity(members.len());
                    let mut running = false;
                    let tools: Vec<Row> = members
                        .into_iter()
                        .map(|ix| {
                            let key = &ids[ix];
                            match &live.items[ix] {
                                Item::Tool { id: tool_id, title, status, .. } => {
                                    // A running sub-agent shows what it's doing right now.
                                    let activity = live.tasks.iter().find(|t| &t.id == tool_id && t.done.is_none()).map(|t| {
                                        let steps = if t.tool_uses == 1 { "1 step".to_string() } else { format!("{} steps", t.tool_uses) };
                                        SharedString::from(if t.activity.is_empty() { steps } else { format!("{} · {steps}", t.activity) })
                                    });
                                    kinds.push(tool_kind(title));
                                    running |= *status == ToolStatus::Running;
                                    Row::Tool { ix, key: key.clone().into(), open: self.expanded.contains(key), activity }
                                }
                                // Finished thoughts fold into the group; live thinking has no row
                                // (the working bar above the composer is the one "working" indicator).
                                _ => {
                                    kinds.push(ToolKind::Thought);
                                    Row::Reasoning { ix, key: key.clone().into(), open: self.expanded.contains(key) }
                                }
                            }
                        })
                        .collect();
                    let tool_kinds: Vec<ToolKind> = kinds.into_iter().filter(|k| *k != ToolKind::Thought).collect();
                    Row::ToolGroup {
                        ix,
                        open: self.expanded.contains(key.as_ref()),
                        key,
                        summary: summarize(&tool_kinds).into(),
                        kind: tool_kinds.last().copied().unwrap_or(ToolKind::Thought),
                        running,
                        tools,
                    }
                }
                Slot::Item(ix) => {
                    let key = &ids[ix];
                    match &live.items[ix] {
                        Item::User { text, .. } if orch::is_wake(text) => Row::Wake { ix, key: key.clone().into(), open: self.expanded.contains(key) },
                        Item::User { .. } => Row::User { ix, key: key.clone().into(), open: self.expanded.contains(key) },
                        // `TREK_OPEN_BACKGROUND=1` (design review) opens them all.
                        Item::Tool { .. } => Row::SubAgent { ix, key: key.clone().into(), open: self.expanded.contains(key) || review_open() },
                        Item::TurnEnd { .. } => Row::TurnEnd { ix },
                        Item::Assistant { .. } => Row::Assistant { ix, key: key.clone().into() },
                        Item::Notice { .. } => Row::Notice { ix },
                        Item::Error { .. } => Row::Error { ix },
                        Item::Limit { .. } => Row::Limit { ix },
                        Item::Handoff { .. } => Row::Handoff { ix },
                        Item::Reasoning { .. } => unreachable!("grouped by layout()"),
                    }
                }
            })
            .collect();
        Rows { rows, item_row }
    }

    /// Everything the agent said between the user's message before `ix` and `ix`.
    fn response_text(items: &[Item], ix: usize) -> String {
        let ix = ix.min(items.len());
        let start = items[..ix].iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |u| u + 1);
        items[start..ix]
            .iter()
            .filter_map(|i| if let Item::Assistant { text } = i { Some(text.trim()) } else { None })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// One transcript row. `row_ix` is its place in the list (tool rows inside a group pass the
    /// group's); `flash` tints it for a moment after a search result scrolled to it.
    fn render_row(row: Row, row_ix: usize, flash: Option<u64>, view: &WeakEntity<ThreadView>, at: &RowContext, cx: &mut App) -> AnyElement {
        // Documents and nested rows need the app mutably; everything else only reads it.
        let md = match &row {
            Row::Assistant { ix, key } | Row::Reasoning { ix, key, open: true } | Row::SubAgent { ix, key, open: true } => view.update(cx, |this, cx| this.markdown(key, *ix, cx)).ok().flatten(),
            _ => None,
        };
        let children: Vec<AnyElement> = match &row {
            Row::ToolGroup { open: true, tools, .. } => tools.iter().map(|t| Self::render_row(t.clone(), row_ix, None, view, at, cx)).collect(),
            _ => vec![],
        };
        // What a finished turn changed, worked out (off the main thread) the first time its
        // footer is drawn.
        let changes = match &row {
            Row::TurnEnd { ix } => at.workspace.update(cx, |ws, cx| ws.load_turn_changes(&at.thread, *ix, cx)),
            _ => None,
        };
        let cx: &App = cx;
        let Some(item) = at.workspace.read(cx).live.get(&at.thread).and_then(|l| l.items.get(row.item())).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let text_size = at.text_size;
        let tint = theme.foreground.opacity(0.07);
        let animate = at.animate;
        let width = at.column;
        let column = |el: Div| {
            let el = el.w_full().max_w(width);
            let el = match flash {
                // A soft band behind the message that holds briefly, then fades.
                Some(seq) => {
                    let band = div().absolute().top(px(-2.)).bottom(px(-2.)).left(px(-12.)).right(px(-12.)).rounded(px(10.)).bg(tint);
                    let band = if animate {
                        band.with_animation(("reveal", seq), Animation::new(FLASH), |el, t| el.opacity(1. - ((t - 0.45) / 0.55).clamp(0., 1.))).into_any_element()
                    } else {
                        band.into_any_element()
                    };
                    div().relative().w_full().max_w(width).child(band).child(el)
                }
                None => el,
            };
            h_flex().w_full().justify_center().px_6().child(el)
        };
        let toggle = move |key: SharedString| {
            let view = view.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| {
                    if !this.expanded.remove(key.as_ref()) {
                        this.expanded.insert(key.to_string());
                    }
                    this.expanded_gen += 1;
                    this.scroller.update(cx, |s, cx| _ = s.remeasure_items(row_ix..row_ix + 1, cx));
                    // An opened sub-agent's activity shimmers at the working bar's rate.
                    let live = this.workspace.read(cx).any_task_live_in(&this.scope);
                    this.sync_ticker(live, cx);
                    cx.notify();
                });
            }
        };
        let ends_turn = trek_core::rewind::ends_turn(&item);
        // The confirmation open on this row, if any.
        let asking = view.upgrade().and_then(|v| v.read(cx).confirm.as_ref().map(|c| (c.anchor.clone(), c.ask.clone())));
        let muted = theme.muted_foreground;
        let action = |id: (&'static str, usize), icon: Icon, tooltip: SharedString| Button::new(id).ghost().xsmall().icon(icon.text_color(muted)).tooltip(tooltip);
        let live = at.workspace.read(cx).live.get(&at.thread);
        // A tooltip, with why the files won't be restored when they won't (message `pos`).
        let with_files = |tooltip: &str, pos: Option<usize>| -> SharedString {
            match live.zip(pos).and_then(|(l, pos)| crate::workspace::NoCheckpoint::now(l, pos, at.in_repo, at.worktree_missing)) {
                Some(why) => format!("{tooltip} ({})", why.short()).into(),
                None => tooltip.to_string().into(),
            }
        };
        // The popover a rewind, undo or retry asks for confirmation in.
        let confirm_popover = |id: (&'static str, usize), anchor: Anchor, ask: Ask, item: &str, open: bool, trigger: Button| {
            let (v1, v2, item) = (view.clone(), view.clone(), item.to_string());
            Popover::new(id)
                .anchor(anchor)
                .appearance(false)
                .open(open)
                .on_open_change(move |open, _, cx| {
                    let (ask, item) = (ask.clone(), item.clone());
                    let _ = v1.update(cx, |this, cx| if *open { this.open_confirm(ask, item, cx) } else { this.close_confirm(cx) });
                })
                .trigger(trigger)
                .content(move |_, _, cx| v2.update(cx, |this, cx| this.confirm_card(cx)).unwrap_or_else(|_| div().into_any_element()))
        };
        // Undo, retry (or with another model) and fork for the turn ending at item `ix` (its
        // footer, its error, or its interruption). The latest turn's are always there; earlier
        // ones show on hover.
        let turn_actions = |ix: usize| -> AnyElement {
            let end = live.and_then(|l| l.items.id_at(ix)).unwrap_or_default().to_string();
            // A turn the agent started itself (a sub-agent reporting back) has no message to take back.
            let start = live.and_then(|l| trek_core::rewind::turn_start(&l.items, ix));
            let ask = asking.as_ref().filter(|(a, _)| *a == end).map(|(_, ask)| ask.clone());
            let busy = at.busy;
            let tip = |what: &str, label: &str| -> SharedString {
                if busy {
                    format!("Stop the running turn to {what}").into()
                } else if start.is_none() {
                    "This turn didn't start from a message of yours".into()
                } else {
                    with_files(label, start)
                }
            };
            let off = busy || start.is_none();
            let undo = confirm_popover(
                ("undo-pop", ix),
                Anchor::BottomLeft,
                Ask::Undo,
                &end,
                ask == Some(Ask::Undo),
                action(("undo-turn", ix), Icon::new(crate::assets::Lucide::Undo2), tip("undo", "Undo this turn")).disabled(off),
            );
            let retry_with = ask.as_ref().and_then(|a| if let Ask::Retry(m) = a { Some(m.clone()) } else { None });
            let retry = confirm_popover(
                ("retry-pop", ix),
                Anchor::BottomLeft,
                Ask::Retry(retry_with.clone().flatten()),
                &end,
                retry_with.is_some(),
                action(("retry-turn", ix), Icon::new(crate::assets::Lucide::RefreshCw), tip("retry", "Retry")).disabled(off),
            );
            let models = {
                let (ws, view, end, agent, current) = (at.workspace.clone(), view.clone(), end.clone(), at.agent.clone(), at.model.clone());
                let tooltip = if off { tip("retry", "") } else { "Retry with another model".into() };
                action(("retry-with", ix), Icon::new(IconName::ChevronDown), tooltip).disabled(off).dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, cx| {
                    menu = menu.min_w(px(220.)).max_h(px(320.)).scrollable(true).label("Retry with");
                    let models = ws.read(cx).models_for(&agent);
                    // The model it ran with: the thread's, else the agent's default (as the composer shows it).
                    let current = current.clone().or_else(|| crate::composer::default_model(&models).map(|m| m.id.clone()));
                    for m in models.iter().filter(|m| current.as_deref().is_none_or(|c| !crate::composer::same_model(c, &m.id))).cloned() {
                        let (view, end) = (view.clone(), end.clone());
                        menu = menu.item(PopupMenuItem::new(m.name.clone()).on_click(move |_, _, cx| {
                            let (id, end) = (m.id.clone(), end.clone());
                            let _ = view.update(cx, |this, cx| this.open_confirm(Ask::Retry(Some(id)), end, cx));
                        }));
                    }
                    menu
                })
            };
            let fork = {
                let (ws, thread, scope, end) = (at.workspace.clone(), at.thread.clone(), at.scope.clone(), end.clone());
                action(("fork-turn", ix), Icon::new(crate::assets::Lucide::GitFork), "Fork from here: a new thread with the conversation up to this point".into()).on_click(move |_, _, cx| {
                    let (thread, scope, end) = (thread.clone(), scope.clone(), end.clone());
                    ws.update(cx, |ws, cx| _ = ws.fork_thread(&thread, ForkAt::After(end), &scope, cx));
                })
            };
            let shown = at.last_end == Some(ix) || ask.is_some();
            h_flex()
                .gap(px(2.))
                .when(!shown, |el| el.invisible().group_hover("turn-end", |s| s.visible()))
                .child(undo)
                .child(h_flex().child(retry).child(models))
                .child(fork)
                .into_any_element()
        };
        match (row, item) {
            (Row::User { ix, key, open }, Item::User { text: full, images, at: sent, aside, .. }) => {
                // A message sent with consultants shows as written, with who it consults under it.
                let (said, consult) = orch::split_consult(&full);
                let (said, restated) = trek_core::restate::split_restate(said);
                let consulting = consult.map(|c| {
                    let ws = at.workspace.read(cx);
                    let name = |k: &orch::Consultant| {
                        let models = ws.models_for(&k.agent);
                        let name = models.iter().find(|m| crate::composer::same_model(&k.model, &m.id)).map(|m| m.name.clone()).unwrap_or_else(|| k.model.clone());
                        format!("{name} · {}", k.effort.label())
                    };
                    let who: Vec<String> = c.consultants.iter().map(name).collect();
                    let then = if c.implement { "then implement" } else { "then report" };
                    let line = match c.style {
                        orch::Style::Advise => format!("Consulting {} · {}", who.join(", "), if c.implement { "advice, then implement" } else { "advice only" }),
                        orch::Style::Discuss => format!("Consulting {} · discuss, {then}", who.join(", ")),
                        orch::Style::Arena => match &c.judge {
                            Some(j) => format!("Design arena: {} · judged by {} · {then}", who.join(", "), name(j)),
                            None => format!("Design arena: {} · {then}", who.join(", ")),
                        },
                    };
                    (c.consultants.iter().chain(c.judge.iter()).map(|k| k.agent.clone()).collect::<Vec<_>>(), line)
                });
                let text = SharedString::from(said.to_string());
                let long = text.len() > 700 || text.lines().count() > 10;
                let has_text = !text.trim().is_empty();
                let copy_text = text.clone();
                let rewinding = asking.as_ref().is_some_and(|(a, ask)| *a == *key && *ask == Ask::Rewind);
                let busy = at.busy;
                let (ws, thread, scope, item) = (at.workspace.clone(), at.thread.clone(), at.scope.clone(), key.to_string());
                let edit = {
                    let (ws, thread, scope, item, text) = (ws.clone(), thread.clone(), scope.clone(), item.clone(), full.clone());
                    let images: Vec<std::path::PathBuf> = images.iter().map(std::path::PathBuf::from).collect();
                    action(("edit-user", ix), Icon::new(crate::assets::Lucide::Pencil), if busy { "Stop the running turn to edit".into() } else { with_files("Edit and send again", Some(ix)) })
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let (thread, scope, item, text, images) = (thread.clone(), scope.clone(), item.clone(), text.clone(), images.clone());
                            ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::ComposeIn { scope, thread, text, images, edit: Some(item) }));
                        })
                };
                let rewind = confirm_popover(
                    ("rewind-pop", ix),
                    Anchor::BottomRight,
                    Ask::Rewind,
                    &item,
                    rewinding,
                    action(("rewind-user", ix), Icon::new(crate::assets::Lucide::Undo2), if busy { "Stop the running turn to rewind".into() } else { with_files("Rewind to here", Some(ix)) }).disabled(busy),
                );
                let fork = action(("fork-user", ix), Icon::new(crate::assets::Lucide::GitFork), "Fork from here: a new thread with the conversation before this message".into())
                    .on_click(move |_, _, cx| {
                        let (thread, scope, item) = (thread.clone(), scope.clone(), item.clone());
                        ws.update(cx, |ws, cx| _ = ws.fork_thread(&thread, ForkAt::Before(item), &scope, cx));
                    });
                // Time sent and the message's actions, shown while the pointer is over it.
                let meta = h_flex()
                    .h(px(20.))
                    .gap(px(6.))
                    .text_xs()
                    .text_color(muted)
                    .when(!rewinding, |el| el.invisible().group_hover("user-msg", |s| s.visible()))
                    .children(sent.map(crate::time::clock))
                    // A command Trek answered or an answer to the agent's question isn't somewhere
                    // to go back to: it only gets copy.
                    .child(h_flex().gap(px(2.)).when(!aside, |el| el.child(edit).child(rewind).child(fork)).child(
                        Button::new(("copy-user", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).text_color(muted))
                            .tooltip("Copy message")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
                                window.push_notification("Copied", cx);
                            }),
                    ));
                column(div().w_full().child(
                    v_flex().id(("user-msg", ix)).test_support().w_full().group("user-msg").items_end().pt_4().gap(px(4.))
                    .when(!images.is_empty(), |el| {
                        el.child(h_flex().gap_2().flex_wrap().justify_end().children(images.into_iter().enumerate().map(|(i, p)| {
                            let path = std::path::PathBuf::from(&p);
                            div()
                                .id(("user-img", ix * 100 + i))
                                .h(px(120.))
                                .max_w(px(220.))
                                .rounded(px(12.))
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .child(img(path.clone()).h_full().object_fit(ObjectFit::Contain))
                                .on_click(move |_, _, cx| cx.open_with_system(&path))
                        })))
                    })
                    .when(has_text, |el| el.child(
                        v_flex()
                            .max_w(relative(0.78))
                            .px(px(16.))
                            .py(px(10.))
                            .gap_1()
                            .rounded(px(18.))
                            .bg(theme.secondary)
                            .text_size(text_size)
                            .line_height(relative(1.5))
                            .child(div().when(long && !open, |el| el.line_clamp(10)).child(text))
                            .when(long, |el| {
                                el.child(
                                    div()
                                        .id(("user-more", ix))
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme.foreground))
                                        .child(if open { "Show less" } else { "Show more" })
                                        .on_click(toggle(key.clone())),
                                )
                            }),
                    ))
                    .when_some(consulting, |el, (agents, line)| {
                        el.child(
                            h_flex()
                                .id(("consulting", ix))
                                .test_support()
                                .gap(px(6.))
                                .max_w(relative(0.78))
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(h_flex().children(agents.iter().take(4).enumerate().map(|(i, a)| div().when(i > 0, |el| el.ml(px(-4.))).child(crate::ui::agent_logo(a, px(13.), cx)))))
                                .child(div().min_w_0().truncate().child(line)),
                        )
                    })
                    .when(restated, |el| {
                        el.child(
                            h_flex()
                                .id(("restating", ix))
                                .test_support()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(Icon::new(crate::assets::Lucide::MessageSquareQuote).xsmall())
                                .child("Asked to restate it first"),
                        )
                    })
                    .child(meta),
                ))
                .into_any_element()
            }
            (Row::TurnEnd { ix }, Item::TurnEnd { at: finished, took_secs }) => {
                let (workspace, thread) = (at.workspace.clone(), at.thread.clone());
                let items = live.map(|l| &l.items[..]).unwrap_or_default();
                let from = items[..ix.min(items.len())].iter().rposition(trek_core::rewind::ends_turn).map_or(0, |b| b + 1);
                let turn = &items[from.min(ix)..ix.min(items.len())];
                // Did the turn run the project's verification CLI, and did its last run pass?
                let verdict = at.verify_probe.as_ref().and_then(|p| trek_core::verification::verdict(turn, p));
                let verified = verdict.map(|v| {
                    let color = if v.passed { palette::emerald(cx) } else { palette::amber(cx) };
                    let n = v.commands.len();
                    let tip = format!(
                        "{} the project's verification skill {}:\n{}",
                        if v.passed { "Checked with" } else { "The last check with" },
                        if v.passed { format!("({} run{})", n, if n == 1 { "" } else { "s" }) } else { "failed".into() },
                        v.commands.iter().rev().take(3).map(|c| format!("$ {}", orch::preview(c, 90))).collect::<Vec<_>>().join("\n")
                    );
                    h_flex()
                        .id((if v.passed { "verified" } else { "verify-failed" }, ix))
                        .test_support()
                        .gap(px(4.))
                        .text_color(color)
                        .child(Icon::new(if v.passed { crate::assets::Lucide::BadgeCheck } else { crate::assets::Lucide::TriangleAlert }).xsmall())
                        .child(if v.passed { "Verified" } else { "Verification failed" })
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                });
                // The agent said back what it was asked: the user confirms or corrects it.
                let restated = trek_core::restate::asked_in_turn(items, ix);
                let confirm = (restated && at.last_end == Some(ix) && !at.busy).then(|| {
                    let (ws, ws2, thread, thread2, scope) = (at.workspace.clone(), at.workspace.clone(), at.thread.clone(), at.thread.clone(), at.scope.clone());
                    h_flex()
                        .id(("restate-check", ix))
                        .test_support()
                        .pt(px(6.))
                        .pb(px(4.))
                        .gap(px(8.))
                        .child(div().min_w_0().pr(px(4.)).text_size(px(12.5)).text_color(muted).child("Is that what you meant?"))
                        .child(Button::new(("restate-yes", ix)).small().primary().label("That's right — go ahead").on_click(move |_, _, cx| {
                            let thread = thread2.clone();
                            ws2.update(cx, |ws, cx| ws.go_ahead(&thread, cx));
                        }))
                        .child(Button::new(("restate-no", ix)).small().ghost().label("Not quite…").on_click(move |_, _, cx| {
                            let (scope, thread) = (scope.clone(), thread.clone());
                            ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::CorrectRestatement { scope, thread }));
                        }))
                });
                let card = changes.map(|c| {
                    let end = live.and_then(|l| l.items.id_at(ix)).unwrap_or_default().to_string();
                    let prefix = format!("changes:{end}:");
                    let folded: HashSet<String> = view.upgrade().map(|v| v.read(cx).expanded.iter().filter_map(|k| k.strip_prefix(&prefix)).map(str::to_string).collect()).unwrap_or_default();
                    let fold = {
                        let (view, prefix) = (view.clone(), prefix.clone());
                        move |dirs: Vec<String>, fold: Option<bool>, cx: &mut App| {
                            let _ = view.update(cx, |this, cx| {
                                for dir in dirs {
                                    let key = format!("{prefix}{dir}");
                                    let folded = this.expanded.contains(&key);
                                    match fold.unwrap_or(!folded) {
                                        true => this.expanded.insert(key),
                                        false => this.expanded.remove(&key),
                                    };
                                }
                                this.expanded_gen += 1;
                                this.scroller.update(cx, |s, cx| _ = s.remeasure_items(row_ix..row_ix + 1, cx));
                                cx.notify();
                            });
                        }
                    };
                    let fold = std::rc::Rc::new(fold);
                    let dirs: Vec<String> = crate::changes_card::folders(&c).into_iter().map(|(d, _)| d).collect();
                    let (fold1, fold2) = (fold.clone(), fold);
                    // The diff shows in the Git tool, which only the main window has.
                    let view_diff = (c.counted == trek_core::changes::Counted::Checkpoints && at.scope == Scope::Main).then(|| {
                        let (ws, thread, end) = (at.workspace.clone(), at.thread.clone(), end.clone());
                        std::rc::Rc::new(move |path: Option<String>, _: &mut Window, cx: &mut App| {
                            let (thread, end) = (thread.clone(), end.clone());
                            ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::ShowTurnDiff { thread, end, path }));
                        }) as std::rc::Rc<dyn Fn(Option<String>, &mut Window, &mut App)>
                    });
                    let root = c.root.clone();
                    let actions = crate::changes_card::Actions {
                        toggle_dir: std::rc::Rc::new(move |dir: &str, cx: &mut App| fold1(vec![dir.to_string()], None, cx)),
                        fold_all: std::rc::Rc::new(move |fold: bool, cx: &mut App| fold2(dirs.clone(), Some(fold), cx)),
                        view_diff,
                        reveal: std::rc::Rc::new(move |f: &trek_core::changes::FileChange, _: &mut Window, cx: &mut App| {
                            let path = root.join(&f.path);
                            match path.exists() {
                                true => cx.reveal_path(&path),
                                false => {
                                    if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                                        cx.open_with_system(dir);
                                    }
                                }
                            }
                        }),
                    };
                    crate::changes_card::card(ix, &c, &folded, actions, cx)
                });
                column(
                    v_flex().children(card).children(confirm).child(
                        h_flex()
                            .group("turn-end")
                            .pt(px(2.))
                            .pb(px(10.))
                            .gap(px(6.))
                            .text_xs()
                            .text_color(muted)
                            .child(
                                Button::new(("copy-turn", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Copy).text_color(muted))
                                    .tooltip("Copy response")
                                    .on_click(move |_, window, cx| {
                                        let text = workspace.read(cx).live.get(&thread).map(|l| Self::response_text(&l.items, ix)).unwrap_or_default();
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        window.push_notification("Copied", cx);
                                    }),
                            )
                            .child(crate::time::clock(finished))
                            .when(took_secs >= 1, |el| el.child(div().text_color(muted.opacity(0.7)).child(format!("· {}", crate::time::took(took_secs)))))
                            .children(verified)
                            .child(turn_actions(ix)),
                    ),
                )
                .into_any_element()
            }
            (Row::Assistant { ix, .. }, _) => match md {
                Some(md) => column(div().py_2().child(div().id(("answer-text", ix)).test_support().child(crate::md::view(&md, at.cwd.clone(), at.folder, text_size, cx).motion(crate::md::streaming()))))
                    .id(("answer", ix))
                    .test_support()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            (Row::Reasoning { ix, key, open }, _) => column(
                v_flex()
                    .py_1()
                    .child(
                        h_flex()
                            .id(("reasoning", ix))
                            .test_support()
                            .gap_1()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall())
                            .child(div().child("Thought"))
                            .on_click(toggle(key.clone())),
                    )
                    .when_some(md.filter(|_| open), |el, md| {
                        el.child(
                            div().ml_2().pl_3().py_1().border_l_2().border_color(theme.border).child(crate::md::thought(&md, at.cwd.clone(), at.folder, text_size * 0.93, cx)),
                        )
                    }),
            )
            .into_any_element(),
            (Row::ToolGroup { ix, key, summary, kind, running, open, .. }, _) => {
                let muted = theme.muted_foreground;
                column(
                    v_flex()
                        .py(px(3.))
                        .child(
                            h_flex()
                                .id(("tool-group", ix))
                                .test_support()
                                .gap_2()
                                .py_1()
                                .text_sm()
                                .text_color(muted)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child(crate::activity::group_icon(kind).small())
                                .child(div().when(running, |el| el.text_color(theme.foreground.opacity(0.85))).child(summary))
                                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().opacity(0.6))
                                .on_click(toggle(key.clone())),
                        )
                        .when(open, |el| el.child(v_flex().ml(px(7.)).pl_4().border_l_1().border_color(theme.border).children(children))),
                )
                .into_any_element()
            }
            (Row::Tool { ix, key, open, activity }, Item::Tool { id: tool_id, title, detail, output, status }) => {
                let kind = tool_kind(&title);
                let icon = kind_icon(kind);
                // Files read or changed get their type's badge; changes, the lines they touched.
                let file = matches!(kind, ToolKind::Read | ToolKind::Edit).then(|| detail.split(", ").next().unwrap_or_default().to_string()).filter(|f| !f.is_empty());
                let lines = live.and_then(|l| l.lines.get(&tool_id)).copied().filter(|_| !matches!(status, ToolStatus::Failed | ToolStatus::Denied));
                let status_el = match status {
                    ToolStatus::Running => Icon::new(crate::assets::Lucide::LoaderCircle).xsmall().text_color(theme.muted_foreground).into_any_element(),
                    ToolStatus::Done => Icon::new(IconName::Check).xsmall().text_color(theme.muted_foreground).into_any_element(),
                    ToolStatus::Failed | ToolStatus::Denied => Icon::new(IconName::CircleX).xsmall().text_color(palette::red(cx)).into_any_element(),
                };
                let has_output = !output.is_empty();
                let subagent = title == "Subagent";
                // Trek's own orchestration tools read as what they did; their arguments are noise.
                let ours = orch::tool_label(&title).or_else(|| orch::is_delegate_call(&title).then_some("Couldn't start a sub-agent"));
                let label = SharedString::from(match ours {
                    Some(l) => l.to_string(),
                    None if subagent => detail.clone(),
                    None => title,
                });
                let code = activity.is_none();
                let detail = match activity {
                    Some(a) => a,
                    None if subagent || ours.is_some() => SharedString::default(),
                    None => detail.into(),
                };
                v_flex()
                    .py(px(1.))
                    .child(
                        h_flex()
                            .id(("tool", ix))
                            .test_support()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .text_sm()
                            .when(has_output, |el| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(toggle(key.clone())))
                            .child(icon.small().text_color(theme.muted_foreground))
                            .child(div().font_medium().flex_none().max_w(relative(0.5)).truncate().child(label))
                            // A file read or changed sits in a chip of its type's colour.
                            .map(|el| match file {
                                Some(f) => el.child(
                                    div().flex_1().min_w_0().flex().child(crate::file_icon::chip(("tool-file", ix), &f, detail.clone(), px(11.5), cx)),
                                ),
                                None => el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(code, |el| el.font_family(theme.mono_font_family.clone()))
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(detail),
                                ),
                            })
                            .when_some(lines, |el, (a, r)| el.child(crate::working_bar::lines_chip(a, r, cx)))
                            .child(status_el),
                    )
                    .when(open && has_output, |el| {
                        el.child(
                            div()
                                .id(("tool-out", ix))
                                .test_support()
                                .mt_1()
                                .ml_8()
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded(theme.radius)
                                .bg(theme.muted)
                                .font_family(theme.mono_font_family.clone())
                                .text_xs()
                                .whitespace_normal()
                                .child(output),
                        )
                    })
                    .into_any_element()
            }
            (Row::SubAgent { ix, key, open }, Item::Tool { id: row_id, detail, output, status, .. }) => {
                let ws = at.workspace.read(cx);
                let sub = SubAgentRow::read(ws, &at.thread, &at.agent, &row_id, &detail, &output, status);
                let finished = !sub.state.live();
                let has_result = finished && !output.trim().is_empty() && sub.state != TaskState::Failed;
                // Open, it shows what it's doing while it works, and its answer once it's done.
                let expandable = has_result || sub.activity.is_some();
                let dot = match sub.state {
                    TaskState::Running => palette::sky(cx),
                    TaskState::NeedsYou => palette::amber(cx),
                    TaskState::Done => palette::emerald(cx),
                    TaskState::Failed => palette::red(cx),
                    TaskState::Cancelled => muted.opacity(0.7),
                };
                // A slow breath while it works (the view re-renders on a ticker meanwhile).
                let breath = if sub.state == TaskState::Running && at.pulse { 0.45 + 0.55 * (0.5 - 0.5 * (at.clock * std::f32::consts::TAU / 2.4).cos()) } else { 1. };
                let logo = div()
                    .relative()
                    .flex_none()
                    .size(px(22.))
                    .child(match &sub.agent {
                        Some(a) => crate::ui::agent_logo(a, px(22.), cx),
                        None => Icon::new(crate::assets::Lucide::Users).size(px(18.)).text_color(muted).into_any_element(),
                    })
                    .child(div().absolute().right(px(-2.)).bottom(px(-2.)).size(px(9.)).rounded_full().border_2().border_color(theme.background).bg(dot.opacity(breath)));
                let opener = sub.child.clone().map(|child| {
                    let ws = at.workspace.clone();
                    Button::new(("subagent-open", ix))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(crate::assets::Lucide::SquareArrowOutUpRight).text_color(muted))
                        .tooltip("Open the sub-agent's thread in a window")
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            crate::thread_window::open(ws.clone(), &child, cx);
                        })
                });
                let chevron = expandable.then(|| Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().text_color(muted));
                let activity = sub.activity.clone().filter(|_| open).map(|g| g.element(at.clock, !at.pulse, cx));
                let preview = (has_result && !open).then(|| orch::plain_preview(&output, 240));
                column(
                    v_flex()
                        .py(px(3.))
                        .child(
                            h_flex()
                                .id(("subagent", ix))
                                .test_support()
                                .gap(px(10.))
                                .px(px(8.))
                                .py(px(6.))
                                .mx(px(-8.))
                                .rounded(px(8.))
                                .when(expandable, |el| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(toggle(key.clone())))
                                .child(logo)
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().text_size(px(13.5)).font_medium().truncate().child(sub.label))
                                        .child(div().text_size(px(12.5)).text_color(muted).truncate().child(sub.detail)),
                                )
                                .when_some(sub.elapsed, |el, d| el.child(div().flex_none().text_xs().text_color(muted).child(crate::time::elapsed(d))))
                                .children(opener)
                                .children(chevron),
                        )
                        .when_some(activity, |el, a| el.child(div().id(("subagent-activity", ix)).test_support().pl(px(32.)).pr(px(28.)).pb(px(2.)).child(a)))
                        .when_some(preview, |el, p| {
                            el.child(div().pl(px(32.)).pr(px(28.)).pb(px(2.)).text_size(px(12.5)).line_height(relative(1.45)).text_color(muted).line_clamp(2).child(p))
                        })
                        .when_some(md.filter(|_| open && has_result), |el, md| {
                            el.child(
                                div()
                                    .ml(px(10.))
                                    .mt_1()
                                    .pl(px(21.))
                                    .border_l_1()
                                    .border_color(theme.foreground.opacity(0.07))
                                    .child(crate::md::view(&md, at.cwd.clone(), at.folder, text_size * 0.93, cx)),
                            )
                        }),
                )
                .into_any_element()
            }
            (Row::Wake { ix, key, open }, Item::User { text, .. }) => column(
                v_flex()
                    .py_1()
                    .child(
                        h_flex()
                            .id(("wake", ix))
                            .test_support()
                            .gap(px(8.))
                            .text_size(px(12.5))
                            .text_color(muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child(Icon::new(crate::assets::Lucide::CornerDownRight).xsmall())
                            .child(div().min_w_0().truncate().child(orch::wake_summary(&text)))
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().opacity(0.6))
                            .on_click(toggle(key.clone())),
                    )
                    .when(open, |el| {
                        el.child(
                            div()
                                .id(("wake-text", ix))
                                .mt_1()
                                .ml(px(6.))
                                .pl(px(15.))
                                .border_l_1()
                                .border_color(theme.foreground.opacity(0.07))
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .text_size(px(12.5))
                                .line_height(relative(1.5))
                                .text_color(muted)
                                .whitespace_normal()
                                .child(text),
                        )
                    }),
            )
            .into_any_element(),
            // Notices are plain text; drop the light markdown the built-in commands use.
            (Row::Notice { ix }, Item::Notice { text }) => {
                // An interrupted turn can be taken back or tried again from here.
                let turn = ends_turn && live.is_some_and(|l| trek_core::rewind::turn_start(&l.items, ix).is_some());
                column(
                    h_flex()
                        .group("turn-end")
                        .justify_center()
                        .gap(px(6.))
                        .py_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(text.replace("**", "").replace('`', ""))
                        .when(turn, |el| el.child(turn_actions(ix))),
                )
                .into_any_element()
            }
            (Row::Error { ix }, Item::Error { text }) => {
                let turn = live.is_some_and(|l| trek_core::rewind::turn_start(&l.items, ix).is_some());
                column(
                    v_flex()
                        .group("turn-end")
                        .my_2()
                        .gap(px(2.))
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(palette::red(cx).opacity(0.5))
                                .bg(palette::red(cx).opacity(0.08))
                                .text_sm()
                                .child(text),
                        )
                        .when(turn, |el| el.child(turn_actions(ix))),
                )
                .into_any_element()
            }
            (Row::Limit { ix }, Item::Limit { text, resets_at, scope }) => {
                let turn = live.is_some_and(|l| trek_core::rewind::turn_start(&l.items, ix).is_some());
                let amber = palette::amber(cx);
                let when = limit_when(&scope, resets_at, at.workspace.read(cx).now());
                let words = SharedString::from(text);
                column(
                    v_flex().group("turn-end").my_1().gap(px(2.)).child(
                        h_flex()
                            .id(("limit-row", ix))
                            .test_support()
                            .min_w_0()
                            .gap(px(8.))
                            .py_1()
                            .text_sm()
                            // What the agent said, in its words.
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(words.clone()).build(window, cx))
                            .child(Icon::new(crate::assets::Lucide::Gauge).small().text_color(amber))
                            .child(div().flex_none().font_medium().text_color(amber).child("Usage limit reached"))
                            .child(div().min_w_0().truncate().text_color(muted).child(when)),
                    )
                    .when(turn, |el| el.child(turn_actions(ix))),
                )
                .into_any_element()
            }
            (Row::Handoff { ix }, Item::Handoff { from, from_model, to, to_model, from_name, to_name }) => {
                let ws = at.workspace.read(cx);
                let side = |agent: &str, model: &Option<String>, name: Option<String>| {
                    let agent = trek_core::AgentId::from_key(agent);
                    // Named when it was written; else by its model, or today's default.
                    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| {
                        let models = ws.models_for(&agent);
                        let model = model.as_deref().map(|m| crate::composer::model_name(&models, m)).or_else(|| crate::composer::default_model(&models).map(|m| m.name.clone()));
                        handoff_name(&agent, model.as_deref())
                    });
                    h_flex().gap(px(6.)).text_color(theme.foreground.opacity(0.8)).child(crate::ui::agent_glyph(&agent, cx)).child(name)
                };
                column(
                    h_flex().justify_center().child(
                        h_flex()
                            .id(("handoff-row", ix))
                            .test_support()
                            .gap(px(8.))
                            .py_2()
                            .text_xs()
                            .text_color(muted)
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("The new agent picks the conversation up from a recap of it").build(window, cx))
                            .child(Icon::new(crate::assets::Lucide::ArrowLeftRight).xsmall().text_color(muted))
                            .child("Context handoff")
                            .child(side(&from, &from_model, from_name))
                            .child(Icon::new(IconName::ArrowRight).xsmall().text_color(muted))
                            .child(side(&to, &to_model, to_name)),
                    ),
                )
                .into_any_element()
            }
            // The transcript changed under the row (it's rebuilt on the next render).
            _ => div().into_any_element(),
        }
    }

    /// Ask before `ask` on `anchor` (a message for a rewind, a turn's footer otherwise), and
    /// find out, off the main thread, which files restoring its checkpoint would change.
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
        let checkpoint = ws.restorable_checkpoint(&thread, &message);
        let files = match &checkpoint {
            Some(_) => Files::Checking,
            None => Files::Unavailable(ws.no_checkpoint(&thread, &message).unwrap_or(crate::workspace::NoCheckpoint::Missing).explain()),
        };
        let check = checkpoint.as_ref().map(|c| {
            let (repo, sha) = (c.repo.clone(), c.sha.clone());
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let r = trek_core::checkpoint::Repo::find(&repo).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", repo.display()))?;
                        r.changes_since(&sha)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(c) = this.confirm.as_mut() {
                        c.files = match result {
                            Ok(changes) => Files::Changes(changes),
                            Err(e) => Files::Failed(format!("{e:#}")),
                        };
                        cx.notify();
                    }
                });
            })
        });
        self.confirm = Some(Confirm { ask, anchor, message, files, restore: checkpoint.is_some(), _check: check });
        cx.notify();
    }

    fn close_confirm(&mut self, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            cx.notify();
        }
    }

    /// Do what the open confirmation asked. A rewound message goes back into this composer.
    fn run_confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(c), Some(thread)) = (self.confirm.take(), self.current.clone()) else { return };
        // Nothing to put back when the files are as they were (or when there's no checkpoint).
        let nothing = match &c.files {
            Files::Unavailable(_) => true,
            Files::Changes(changes) => changes.is_empty(),
            Files::Checking | Files::Failed(_) => false,
        };
        let restore = c.restore && !nothing;
        let scope = self.scope.clone();
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
                cx.emit(WorkspaceEvent::ComposeIn { scope, thread, text, images, edit: None });
            }
        });
        cx.notify();
    }

    /// The open confirmation: what will happen, the files restoring would change, and the go-ahead.
    fn confirm_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = &self.confirm else { return div().into_any_element() };
        let ws = self.workspace.read(cx);
        let Some(thread) = self.current.as_ref().and_then(|id| ws.thread(id)) else { return div().into_any_element() };
        let agent = thread.agent.display_name();
        let recap = ws.rewind_plan(&thread.id, &c.message) == Some(trek_core::rewind::Reopen::Recap);
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let (title, action) = match &c.ask {
            Ask::Rewind => ("Rewind to before this message?".to_string(), "Rewind"),
            Ask::Undo => ("Undo this turn?".to_string(), "Undo turn"),
            Ask::Retry(None) => ("Retry this turn?".to_string(), "Retry"),
            Ask::Retry(Some(m)) => {
                let name = ws.models_for(&thread.agent).into_iter().find(|i| crate::composer::same_model(m, &i.id)).map(|i| i.name).unwrap_or_else(|| m.clone());
                (format!("Retry with {name}?"), "Retry")
            }
        };
        let mut body = match &c.ask {
            Ask::Rewind => format!("This message and everything after it leave the conversation, and {agent} forgets them. Your message goes back in the composer."),
            Ask::Undo => format!("This turn and everything after it leave the conversation, and {agent} forgets them. Your message goes back in the composer."),
            Ask::Retry(_) => format!("This turn and everything after it leave the conversation, and {agent} gets your message again."),
        };
        if recap {
            body.push_str(&format!(" {agent} can't take its own session back to this point, so it continues in a new one with a recap."));
        }
        let restore = c.restore;
        let toggle = cx.listener(|this, _: &ClickEvent, _, cx| {
            if let Some(c) = this.confirm.as_mut() {
                c.restore = !c.restore;
                cx.notify();
            }
        });
        let note = |text: String| div().text_size(px(12.5)).line_height(relative(1.45)).text_color(muted).child(text).into_any_element();
        let files: AnyElement = match &c.files {
            Files::Checking => h_flex().gap(px(8.)).text_size(px(12.5)).text_color(muted).child(Spinner::new().xsmall()).child("Checking which files changed…").into_any_element(),
            Files::Changes(changes) if changes.is_empty() => note("The files are as they were then.".into()),
            Files::Changes(changes) => v_flex()
                .gap(px(6.))
                .child(crate::ui::check_row("restore-files", "Also restore files", restore, false, cx).on_click(toggle).test_support())
                .child(
                    v_flex()
                        .pl(px(22.))
                        .gap(px(2.))
                        .when(!restore, |el| el.opacity(0.5))
                        .children(changes.iter().take(FILES_SHOWN).map(|f| {
                            let what = match f.change {
                                Change::Modified => "revert",
                                Change::Added => "delete",
                                Change::Deleted => "bring back",
                                Change::Nested => "left as is",
                            };
                            h_flex()
                                .gap(px(8.))
                                .text_xs()
                                .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).child(f.path.clone()))
                                .child(div().flex_none().text_color(muted).child(what))
                        }))
                        .when(changes.len() > FILES_SHOWN, |el| el.child(div().text_xs().text_color(muted).child(format!("and {} more", changes.len() - FILES_SHOWN)))),
                )
                .into_any_element(),
            Files::Unavailable(why) => v_flex()
                .gap(px(6.))
                .child(crate::ui::check_row("restore-files", "Also restore files", false, true, cx))
                .child(note(why.to_string()))
                .into_any_element(),
            Files::Failed(e) => note(format!("Couldn't check the files: {e}")),
        };
        crate::ui::menu_surface(cx)
            .id("confirm-card")
            .test_support()
            .w(px(340.))
            .p(px(12.))
            .gap(px(10.))
            .child(div().text_size(px(13.5)).font_medium().text_color(theme.foreground).child(title))
            .child(note(body))
            .child(div().pt(px(10.)).border_t_1().border_color(theme.foreground.opacity(0.07)).child(files))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("confirm-cancel").small().ghost().label("Cancel").on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx))))
                    .child(Button::new("confirm-go").small().primary().label(action).on_click(cx.listener(|this, _, _, cx| this.run_confirm(cx)))),
            )
            .into_any_element()
    }

    /// Answers from the question card: picked options, and the masked fields for secrets. `None`
    /// until every question has one.
    fn card_answers(&self, request_id: &str, questions: &[trek_agents::Question], cx: &App) -> Option<Vec<(String, String)>> {
        let ws = self.workspace.read(cx);
        let picks = &ws.live.get(self.current.as_ref()?)?.picks;
        let mut fields = self.secrets.1.iter();
        questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let picked = picks.get(&(request_id.to_string(), i)).filter(|v| !v.is_empty()).map(|v| v.join(", "));
                let typed = q.secret.then(|| fields.next()).flatten().map(|f| f.read(cx).value().trim().to_string()).filter(|v| !v.is_empty());
                typed.or(picked).map(|a| (q.question.clone(), a))
            })
            .collect()
    }

    /// One masked field per secret question, empty for each new card.
    fn secret_fields(&mut self, request_id: &str, questions: &[trek_agents::Question], window: &mut Window, cx: &mut Context<Self>) {
        if self.secrets.0.as_deref() != Some(request_id) {
            self.secrets.0 = Some(request_id.to_string());
            self.clear_secrets(window, cx);
        }
        while self.secrets.1.len() < questions.iter().filter(|q| q.secret).count() {
            let field = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Type it here"));
            self._subscriptions.push(cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_answers(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }));
            self.secrets.1.push(field);
        }
    }

    fn clear_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for f in &self.secrets.1 {
            f.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    /// Send the question card's answers, if it's complete.
    fn submit_answers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current.clone() else { return };
        let pending = self.workspace.read(cx).live.get(&id).and_then(|l| l.permissions.first()).and_then(|p| match &p.prompt {
            Some(trek_agents::Prompt::Questions(q)) => Some((p.request_id.clone(), q.clone())),
            _ => None,
        });
        let Some((request_id, questions)) = pending else { return };
        let Some(answers) = self.card_answers(&request_id, &questions, cx) else { return };
        self.clear_secrets(window, cx);
        self.workspace.update(cx, |ws, cx| ws.answer(&id, &request_id, answers, cx));
    }

    /// The agent asked something only a person can answer: multiple-choice questions, or a secret.
    fn question_card(&mut self, id: String, request_id: String, questions: Vec<trek_agents::Question>, agent: String, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.secret_fields(&request_id, &questions, window, cx);
        let mut fields = self.secrets.1.clone().into_iter();
        let theme = cx.theme().clone();
        let ember = palette::ember(cx);
        let complete = self.card_answers(&request_id, &questions, cx).is_some();
        let picks = self.workspace.read(cx).live.get(&id).map(|l| l.picks.clone()).unwrap_or_default();
        let (id2, rid2) = (id.clone(), request_id.clone());
        let width = self.workspace.read(cx).column();
        let hint = if questions.len() > 1 { "Or type below to answer the first open question in your own words." } else { "Or type your own answer below and send it." };
        let body = v_flex().gap(px(14.)).children(questions.iter().enumerate().map(|(qi, q)| {
            let picked = picks.get(&(request_id.clone(), qi)).cloned().unwrap_or_default();
            v_flex()
                .gap(px(6.))
                .child(div().text_size(px(13.5)).font_medium().child(q.question.clone()))
                .when(q.multi, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("Choose any that apply.")))
                .children(q.options.iter().enumerate().map(|(oi, (label, desc))| {
                    let on = picked.contains(label);
                    let (ws, id, rid, label2, multi) = (self.workspace.clone(), id.clone(), request_id.clone(), label.clone(), q.multi);
                    h_flex()
                        .id(SharedString::from(format!("q-{request_id}-{qi}-{oi}")))
                        .test_support()
                        .px(px(10.))
                        .py(px(7.))
                        .gap(px(10.))
                        .items_start()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if on { ember.opacity(0.7) } else { theme.foreground.opacity(0.1) })
                        .when(on, |el| el.bg(ember.opacity(0.08)))
                        .when(!on, |el| el.hover(|s| s.bg(theme.foreground.opacity(0.04))))
                        .cursor_pointer()
                        .child(
                            div()
                                .mt(px(3.))
                                .size(px(14.))
                                .flex_none()
                                .when(!multi, |el| el.rounded_full())
                                .when(multi, |el| el.rounded(px(3.)))
                                .border_1()
                                .border_color(if on { ember } else { theme.foreground.opacity(0.3) })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(on, |el| el.child(div().size(px(7.)).when(!multi, |d| d.rounded_full()).when(multi, |d| d.rounded(px(1.))).bg(ember))),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(13.)).child(label.clone()))
                                .when(!desc.is_empty(), |el| el.child(div().text_xs().line_height(relative(1.45)).text_color(theme.muted_foreground).child(desc.clone()))),
                        )
                        .on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| {
                                let Some(live) = ws.live.get_mut(&id) else { return };
                                let entry = live.picks.entry((rid.clone(), qi)).or_default();
                                if multi {
                                    if let Some(pos) = entry.iter().position(|l| *l == label2) {
                                        entry.remove(pos);
                                    } else {
                                        entry.push(label2.clone());
                                    }
                                } else {
                                    *entry = vec![label2.clone()];
                                }
                                cx.notify();
                            })
                        })
                }))
                .when_some(q.secret.then(|| fields.next()).flatten(), |el, field| {
                    el.child(Input::new(&field).small().mask_toggle())
                        .child(div().text_xs().text_color(theme.muted_foreground).child("Private: Trek sends it to the agent without showing or saving it."))
                })
        }));
        v_flex()
            .w_full()
            .max_w(width)
            .max_h(px(420.))
            .gap(px(12.))
            .p(px(14.))
            .rounded(px(14.))
            .border_1()
            .border_color(theme.foreground.opacity(0.12))
            .bg(theme.secondary)
            .child(h_flex().gap_2().text_sm().child(Icon::new(crate::assets::Lucide::MessageSquare).small().text_color(theme.muted_foreground)).child(div().font_semibold().child(format!("{agent} has a question"))))
            .child(div().id("question-scroll").flex_1().min_h_0().overflow_y_scroll().child(body))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(hint))
                    .child(Button::new("q-skip").small().ghost().label("Skip").on_click(cx.listener(move |this, _, window, cx| {
                        this.clear_secrets(window, cx);
                        this.workspace.update(cx, |ws, cx| ws.respond(&id2, &rid2, Decision::Deny, cx));
                    })))
                    .child(Button::new("q-send").small().primary().label("Answer").disabled(!complete).on_click(cx.listener(|this, _, window, cx| this.submit_answers(window, cx)))),
            )
            .into_any_element()
    }

    /// The agent finished planning and wants a go-ahead before changing anything.
    fn plan_card(&mut self, id: String, request_id: String, plan: String, agent: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.plan_md.as_ref().is_none_or(|(rid, _, _)| *rid != request_id) {
            let text = plan.clone();
            let md = cx.new(|cx| TextViewState::markdown(&text, cx));
            let parsed = cx.observe(&md, |_, _, cx| cx.notify());
            self.plan_md = Some((request_id.clone(), md, parsed));
        }
        let md = self.plan_md.as_ref().map(|(_, m, _)| m.clone());
        let cwd = self.workspace.read(cx).cwd_in(&self.scope);
        let folder = self.workspace.read(cx).thread_in(&self.scope).and_then(|t| self.workspace.read(cx).thread_project_tint(t, cx));
        let (ws, ws2) = (self.workspace.clone(), self.workspace.clone());
        let (id2, rid2, id3, rid3) = (id.clone(), request_id.clone(), id, request_id);
        let width = self.workspace.read(cx).column();
        v_flex()
            .w_full()
            .max_w(width)
            .max_h(px(440.))
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(14.))
            .border_1()
            .border_color(palette::indigo(cx).opacity(0.45))
            .bg(theme.secondary)
            .child(h_flex().gap_2().text_sm().child(Icon::new(crate::assets::Lucide::ListChecks).small().text_color(palette::indigo(cx))).child(div().font_semibold().child(format!("{agent}'s plan"))))
            .child(div().id("plan-scroll").flex_1().min_h_0().overflow_y_scroll().children(md.map(|m| crate::md::view(&m, cwd, folder, px(13.5), cx))))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child("Nothing has been changed yet."))
                    .child(Button::new("plan-revise").small().outline().label("Keep planning").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.respond(&id2, &rid2, Decision::Deny, cx))))
                    .child(Button::new("plan-approve").small().primary().label("Approve and start").on_click(move |_, _, cx| ws2.update(cx, |ws, cx| ws.approve_plan(&id3, &rid3, cx)))),
            )
            .into_any_element()
    }

    /// An approval, question or plan waiting on the user, in place of the working bar.
    fn live_footer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.current.clone()?;
        // Questions and plans get their own cards.
        let special = {
            let ws = self.workspace.read(cx);
            let live = ws.live.get(&id)?;
            let agent = ws.thread(&id)?.agent.display_name();
            live.permissions.first().and_then(|p| p.prompt.clone().map(|prompt| (p.request_id.clone(), prompt, agent)))
        };
        if let Some((request_id, prompt, agent)) = special {
            let card = match prompt {
                trek_agents::Prompt::Questions(q) => self.question_card(id.clone(), request_id, q, agent, window, cx),
                trek_agents::Prompt::Plan(plan) => self.plan_card(id.clone(), request_id, plan, agent, cx),
            };
            return Some(h_flex().w_full().justify_center().px_6().pb_2().child(card).into_any_element());
        }
        let ws = self.workspace.read(cx);
        let live = ws.live.get(&id)?;
        let thread = ws.thread(&id)?;
        let width = ws.column();
        let theme = cx.theme().clone();
        if let Some(p) = live.permissions.first().cloned() {
            let agent = thread.agent.display_name();
            let rid_anim = p.request_id.clone();
            let ws_handle = self.workspace.clone();
            let respond = move |decision: Decision| {
                let ws = ws_handle.clone();
                let id = id.clone();
                let rid = p.request_id.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    ws.update(cx, |ws, cx| ws.respond(&id, &rid, decision, cx));
                }
            };
            let amber = palette::amber(cx);
            return Some(
                h_flex()
                    .w_full()
                    .justify_center()
                    .px_6()
                    .pb_2()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(width)
                            .gap_2()
                            .p_3()
                            .rounded(theme.radius_lg)
                            .border_1()
                            .border_color(amber.opacity(0.6))
                            .bg(amber.opacity(0.07))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(Icon::new(crate::assets::Lucide::ShieldCheck).small().text_color(amber))
                                    .child(div().font_semibold().child(format!("{agent} wants to: {}", p.title))),
                            )
                            .when(!p.detail.is_empty(), |el| {
                                el.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded(theme.radius)
                                        .bg(theme.muted)
                                        .font_family(theme.mono_font_family.clone())
                                        .text_xs()
                                        .child(p.detail.clone()),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(Button::new("deny").small().ghost().label("Deny").on_click(respond(Decision::Deny)))
                                    .child(
                                        Button::new("allow-session")
                                            .small()
                                            .outline()
                                            .label("Always allow")
                                            .on_click(respond(Decision::AllowForSession)),
                                    )
                                    .child(Button::new("allow").small().primary().label("Allow").on_click(respond(Decision::Allow))),
                            ),
                    )
                    // Keyed on the request, not the transcript revision, so streaming doesn't replay it.
                    .with_animation(
                        SharedString::from(format!("perm-in-{rid_anim}")),
                        Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_out_quint()),
                        |el, t| el.opacity(t).mt(px(10. * (1. - t))),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let loading = self.current.as_ref().and_then(|id| ws.live.get(id)).is_some_and(|l| l.loading);
        if loading {
            return v_flex().size_full().items_center().justify_center().child(Spinner::new()).into_any_element();
        }
        let a = &ws.settings.appearance;
        let show = a.background_placement == trek_core::settings::BackgroundPlacement::NewThread && ws.is_draft_in(&self.scope);
        let spec = if show { a.background.clone() } else { None };
        div().relative().size_full().child(crate::ui::hero_background(spec.as_deref(), a.background_dim, cx)).into_any_element()
    }
}

/// What a sub-agent row shows.
struct SubAgentRow {
    /// Whose logo it wears: the agent running it.
    agent: Option<trek_core::AgentId>,
    /// "Sol: Review the cache design"
    label: String,
    /// Where it stands, and what it's doing or why it failed.
    detail: String,
    state: TaskState,
    elapsed: Option<std::time::Duration>,
    /// The thread it runs in, for one Trek runs.
    child: Option<String>,
    /// What it's doing, while it works: its latest calls, as the working bar shows its parent's.
    activity: Option<crate::working_bar::Group>,
}

impl SubAgentRow {
    /// The row for transcript item `row_id` of `thread` (whose agent is `agent`).
    fn read(ws: &Workspace, thread: &str, agent: &trek_core::AgentId, row_id: &str, detail: &str, output: &str, status: ToolStatus) -> SubAgentRow {
        let by_status = match status {
            ToolStatus::Running => TaskState::Running,
            ToolStatus::Done => TaskState::Done,
            ToolStatus::Failed => TaskState::Failed,
            ToolStatus::Denied => TaskState::Cancelled,
        };
        let with_error = |state: TaskState| match state {
            TaskState::Failed if !output.trim().is_empty() => format!("{} · {}", state.label(), orch::preview(output, 140)),
            _ => state.label().to_string(),
        };
        if let Some(child) = orch::task_of_row(row_id) {
            let Some(t) = ws.thread(child) else {
                // Its thread is gone (deleted, or archived on its own): what the row kept.
                let state = if by_status == TaskState::Running { TaskState::Cancelled } else { by_status };
                return SubAgentRow { agent: None, label: detail.to_string(), detail: with_error(state), state, elapsed: None, child: None, activity: None };
            };
            let state = ws.task_state(child);
            let detail = match state {
                TaskState::Running => {
                    // Its latest step, worded as the working bar words it ("Read src/auth.rs").
                    let step = ws.live.get(child).and_then(|l| {
                        l.items.iter().rev().find_map(|i| match i {
                            Item::Tool { title, detail, .. } => {
                                let op = crate::activity::op(title, detail, t.cwd.as_deref());
                                let words = if op.verb.is_empty() || op.text.is_empty() { format!("{}{}", op.verb, op.text) } else { format!("{} {}", op.verb, op.text) };
                                Some(orch::preview(&words, 90))
                            }
                            _ => None,
                        })
                    });
                    step.map_or_else(|| state.label().to_string(), |s| format!("{} · {s}", state.label()))
                }
                other => with_error(other),
            };
            return SubAgentRow {
                agent: Some(t.agent.clone()),
                label: format!("{}: {}", ws.model_label(t), t.title),
                detail,
                state,
                elapsed: Some(ws.task_elapsed(child)),
                child: Some(child.to_string()),
                activity: state.live().then(|| crate::working_bar::child_group(ws, child)).flatten(),
            };
        }
        // One of the agent's own.
        let task = ws.live.get(thread).and_then(|l| l.tasks.iter().find(|t| t.id == row_id));
        let state = match task.map(|t| t.done) {
            Some(None) => TaskState::Running,
            Some(Some(true)) => TaskState::Done,
            Some(Some(false)) => TaskState::Failed,
            None => by_status,
        };
        let steps = |n: u64| if n == 1 { "1 step".to_string() } else { format!("{n} steps") };
        let line = match task {
            Some(t) if state == TaskState::Running && !t.activity.is_empty() => format!("{} · {}", t.activity, steps(t.tool_uses)),
            Some(t) if t.tool_uses > 0 => format!("{} · {}", state.label(), steps(t.tool_uses)),
            _ => with_error(state),
        };
        let cwd = ws.thread(thread).and_then(|t| t.cwd.clone());
        SubAgentRow {
            agent: Some(agent.clone()),
            label: if detail.trim().is_empty() { "Sub-agent".into() } else { detail.to_string() },
            detail: line,
            state,
            elapsed: task.map(|t| t.ended.unwrap_or_else(std::time::Instant::now).saturating_duration_since(t.started)),
            child: None,
            activity: task
                .filter(|_| state.live())
                .and_then(|t| crate::working_bar::steps_group(&t.steps, t.stepped.saturating_sub(t.steps.len()), true, cwd.as_deref())),
        }
    }
}

/// Frames a second for a working sub-agent's breathing dot (once a second when the window is in
/// the background or motion is reduced).
const PULSE_FPS: u64 = 5;

/// `TREK_OPEN_BACKGROUND=1`: sub-agents' rows (and the background strip) open at launch, for
/// design review.
fn review_open() -> bool {
    static OPEN: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("TREK_OPEN_BACKGROUND").is_ok_and(|v| v == "1"));
    *OPEN
}

/// What every row of one render shares.
struct RowContext {
    workspace: Entity<Workspace>,
    thread: String,
    scope: Scope,
    agent: trek_core::AgentId,
    model: Option<String>,
    /// A turn is running: nothing can be taken back yet.
    busy: bool,
    /// Where the last turn ended (its actions stay in view).
    last_end: Option<usize>,
    /// The thread's folder is in a git repository (it gets file checkpoints).
    in_repo: bool,
    /// The thread's worktree is missing: no files can be restored until it's back.
    worktree_missing: bool,
    text_size: Pixels,
    /// The column's widest (`Workspace::column`).
    column: Pixels,
    cwd: Option<std::path::PathBuf>,
    /// Folder icons' tint in path chips: the thread's project colour.
    folder: Option<Hsla>,
    /// The search-result tint fades (the window is in front and motion isn't reduced).
    animate: bool,
    /// Working sub-agents' dots breathe, at `clock` (seconds).
    pulse: bool,
    clock: f32,
    /// How to tell a turn ran the project's verification CLI, when it has one.
    verify_probe: Option<trek_core::verification::Probe>,
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("ThreadView");
        self.draws += 1;
        let rows = self.rows(cx);
        let footer = self.live_footer(window, cx);
        // Where the last row ends is noted as it's laid out below; until then (and when it's out
        // of view, or a card sits under it) there's no tail.
        let tail = self.tail.clone();
        let forget = canvas(move |_, _, _| tail.set(None), |_, _, _, _| {}).absolute().size_0();
        let (Some(thread), false) = (self.current.clone(), rows.rows.is_empty()) else {
            return v_flex().size_full().child(forget).child(self.empty_state(cx)).children(footer);
        };
        let tail = (footer.is_none()).then(|| self.tail.clone());
        let last = rows.rows.len() - 1;
        let view = cx.entity().downgrade();
        let ws = self.workspace.read(cx);
        let t = ws.thread(&thread);
        let at = RowContext {
            workspace: self.workspace.clone(),
            scope: self.scope.clone(),
            agent: t.map(|t| t.agent.clone()).unwrap_or(trek_core::AgentId::ClaudeCode),
            model: t.and_then(|t| t.model.clone()),
            busy: ws.turn_running(&thread),
            last_end: ws.live.get(&thread).and_then(|l| l.items.iter().rposition(trek_core::rewind::ends_turn)),
            in_repo: crate::system::lately::in_repo(t.and_then(|t| t.cwd.as_deref())),
            worktree_missing: t.and_then(|t| t.worktree.as_ref()).is_some_and(crate::system::lately::worktree_missing),
            thread,
            text_size: px(ws.settings.appearance.transcript_font_size()),
            column: ws.column(),
            cwd: ws.cwd_in(&self.scope),
            folder: t.and_then(|t| ws.thread_project_tint(t, cx)),
            animate: self.animate(window, cx),
            pulse: self.animate(window, cx) && self._ticker.is_some(),
            clock: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 1_000_000).unwrap_or(0) as f32) / 1000.,
            verify_probe: t.and_then(|t| ws.verify_probe(t)),
        };
        let flash = self.flash;
        let jump = self.jump_to_latest(window, cx);
        v_flex()
            .size_full()
            .child(forget)
            .child(
                div().relative().flex_1().min_h_0().child(
                    MessageScroller::new("transcript", self.scroller.clone(), move |ix, _, cx| match rows.rows.get(ix).cloned() {
                        Some(row) => {
                            let flash = flash.filter(|(row, _)| *row == ix).map(|(_, seq)| seq);
                            let el = ThreadView::render_row(row, ix, flash, &view, &at, cx);
                            match tail.clone().filter(|_| ix == last) {
                                Some(tail) => v_flex()
                                    .w_full()
                                    .child(el)
                                    .child(canvas(move |b, _, _| tail.set(Some(b.bottom())), |_, _, _, _| {}).w_full().h(px(0.)))
                                    .into_any_element(),
                                None => el,
                            }
                        }
                        None => div().into_any_element(),
                    })
                    .with_list_style(StyleRefinement::default().pt_4().pb(px(TAIL_ROOM)))
                    .with_row_style(StyleRefinement::default().pb_0())
                    .jump_button(false),
                )
                .children(jump),
            )
            .children(footer)
    }
}

/// Room under the transcript's last line, so it ends clear of the working bar and composer. As
/// tall as the "Jump to latest" band: scrolling up a little shows the band in that room, under
/// the last line rather than over it.
const TAIL_ROOM: f32 = JUMP_FADE + JUMP_BAND;
/// The band "Jump to latest" sits in while the reader is scrolled up: the text above fades into
/// it over `JUMP_FADE`, and the button has the solid rest to itself, never lying over a line.
pub(crate) const JUMP_FADE: f32 = 16.;
const JUMP_BAND: f32 = 36.;
const JUMP_IN: std::time::Duration = std::time::Duration::from_millis(200);

impl ThreadView {
    /// While the reader is scrolled up, a band across the transcript's foot with the button that
    /// brings them back down. It eases in and out (at once with reduced motion).
    fn jump_to_latest(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let away = self.scroller.read(cx).is_scrolled_up();
        let pace = if self.animate(window, cx) { JUMP_IN } else { std::time::Duration::ZERO };
        let shown: f32 = gpui_kit::base::motion::transition("transcript-jump", if away { 1. } else { 0. }, gpui_kit::base::motion::Transition::new(pace), window, cx);
        if shown <= 0. {
            return None;
        }
        let theme = cx.theme();
        let bg = theme.background;
        let scroller = self.scroller.clone();
        let button = Button::new("jump-to-latest")
            .secondary()
            .small()
            .icon(IconName::ArrowDown)
            .tooltip("Jump to latest")
            .rounded(theme.radius_full())
            .border_1()
            .border_color(theme.border)
            .bg(bg)
            .disabled(!away)
            .on_click(move |_, _, cx| scroller.update(cx, |s, cx| s.scroll_to_end(cx)));
        Some(
            v_flex()
                .id("transcript-foot")
                .test_support()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(JUMP_FADE + JUMP_BAND))
                .opacity(shown)
                // It covers the rows under it: clicks and hovers stop here (a hidden path chip
                // mustn't open), the wheel still scrolls the transcript.
                .block_mouse_except_scroll()
                .child(div().w_full().h(px(JUMP_FADE)).bg(linear_gradient(180., linear_color_stop(bg.opacity(0.), 0.), linear_color_stop(bg, 1.))))
                .child(h_flex().w_full().flex_1().justify_center().items_center().bg(bg).pb(px(4. * (1. - shown))).child(button))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
impl ThreadView {
    /// Scroll the transcript to its first row, as a reader going back up would.
    pub(crate) fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.scroller.update(cx, |s, cx| _ = s.scroll_to_item(0, cx));
    }

    /// Whether the transcript redraws on a ticker, and at the working bar's rate if so.
    pub(crate) fn ticker(&self) -> Option<bool> {
        self._ticker.as_ref().map(|(fast, _)| *fast)
    }

    /// The question card's masked field for its `n`th secret question.
    pub(crate) fn secret_field(&self, n: usize) -> Entity<InputState> {
        self.secrets.1[n].clone()
    }

    /// The open confirmation's files: "checking", "changes: <paths>", "unavailable" or
    /// "failed"; `None` when none is open.
    pub(crate) fn confirm_files(&self) -> Option<String> {
        self.confirm.as_ref().map(|c| match &c.files {
            Files::Checking => "checking".to_string(),
            Files::Changes(v) => format!("changes: {}", v.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(" ")),
            Files::Unavailable(_) => "unavailable".to_string(),
            Files::Failed(e) => format!("failed: {e}"),
        })
    }

    /// Markdown documents built so far.
    pub(crate) fn markdown_states(&self) -> usize {
        self.md.len()
    }

    /// The markdown documents built so far, by where their item is in the transcript now.
    pub(crate) fn markdown_documents(&self, cx: &App) -> Vec<(usize, Entity<TextViewState>)> {
        let ws = self.workspace.read(cx);
        let Some(l) = self.current.as_ref().and_then(|id| ws.live.get(id)) else { return vec![] };
        let mut docs: Vec<_> = self.md.iter().filter_map(|(key, m)| Some((l.items.position(key)?, m.state.clone()))).collect();
        docs.sort_by_key(|(ix, _)| *ix);
        docs
    }

    /// The turns' changes as their cards list them, in order: "changes (2): +3 −1", then
    /// "  NOTES.md new +3 −0" per file. Only what has been worked out (cards drawn).
    pub(crate) fn describe_changes(&self, cx: &App) -> Vec<String> {
        let ws = self.workspace.read(cx);
        let thread = self.current.clone().unwrap_or_default();
        let Some(live) = ws.live.get(&thread) else { return vec![] };
        let mut out = vec![];
        for (ix, item) in live.items.iter().enumerate() {
            let (Item::TurnEnd { .. }, Some(c)) = (item, ws.turn_changes(&thread, ix)) else { continue };
            let (a, r) = c.totals();
            out.push(format!("changes ({}): +{a} −{r}", c.files.len()));
            for f in &c.files {
                let status = match &f.status {
                    trek_core::changes::FileStatus::Added => "new".to_string(),
                    trek_core::changes::FileStatus::Modified => "changed".to_string(),
                    trek_core::changes::FileStatus::Deleted => "deleted".to_string(),
                    trek_core::changes::FileStatus::Renamed { from } => format!("from {from}"),
                };
                let lines = match (f.binary, f.lines_known) {
                    (true, _) => "binary".to_string(),
                    (false, true) => format!("+{} −{}", f.added, f.removed),
                    (false, false) => "?".to_string(),
                };
                out.push(format!("  {} {status} {lines}", f.path));
            }
        }
        out
    }

    /// The rows as text: "user", "assistant", "group: Ran 1 command" (with "  tool: Read" lines
    /// under an open group), "subagent: Sol: Review (Running)" (with "  Read src/main.rs" lines
    /// of its activity, opened), "wake", "end", "notice", "error".
    pub(crate) fn describe(&self, cx: &App) -> Vec<String> {
        let ws = self.workspace.read(cx);
        let thread = self.current.clone().unwrap_or_default();
        let agent = ws.thread(&thread).map(|t| t.agent.clone()).unwrap_or(trek_core::AgentId::ClaudeCode);
        let items = ws.live.get(&thread).map(|l| l.items.to_vec()).unwrap_or_default();
        let line = |row: &Row, out: &mut Vec<String>, indent: &str| {
            out.push(match row {
                Row::SubAgent { ix, .. } => match items.get(*ix) {
                    Some(Item::Tool { id, detail, output, status, .. }) => {
                        let sub = SubAgentRow::read(ws, &thread, &agent, id, detail, output, *status);
                        format!("{indent}subagent: {} ({})", sub.label, sub.detail)
                    }
                    _ => format!("{indent}subagent"),
                },
                Row::Wake { .. } => format!("{indent}wake"),
                Row::User { .. } => format!("{indent}user"),
                Row::TurnEnd { .. } => format!("{indent}end"),
                Row::Assistant { .. } => format!("{indent}assistant"),
                Row::Reasoning { .. } => format!("{indent}thought"),
                Row::Tool { activity, .. } => format!("{indent}tool{}", activity.as_ref().map(|a| format!(" ({a})")).unwrap_or_default()),
                Row::ToolGroup { summary, running, .. } => format!("{indent}group: {summary}{}", if *running { " (running)" } else { "" }),
                Row::Notice { .. } => format!("{indent}notice"),
                Row::Error { .. } => format!("{indent}error"),
                Row::Limit { .. } => format!("{indent}limit"),
                Row::Handoff { .. } => format!("{indent}handoff"),
            });
        };
        let mut out = vec![];
        for row in self.rows(cx).rows.iter() {
            line(row, &mut out, "");
            if let Row::ToolGroup { open: true, tools, .. } = row {
                for t in tools {
                    line(t, &mut out, "  ");
                }
            }
            if let (Row::SubAgent { open: true, .. }, Some(Item::Tool { id, detail, output, status, .. })) = (row, items.get(row.item())) {
                let sub = SubAgentRow::read(ws, &thread, &agent, id, detail, output, *status);
                out.extend(sub.activity.iter().flat_map(|g| g.describe()).map(|l| format!("  {l}")));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{Slot, handoff_name, layout, row_of};
    use trek_core::AgentId;
    use trek_core::store::{Item, ToolStatus};

    #[test]
    fn handoff_sides_name_the_agent_with_its_model() {
        assert_eq!(handoff_name(&AgentId::ClaudeCode, Some("Opus 5.5")), "Claude Opus 5.5");
        assert_eq!(handoff_name(&AgentId::Codex, Some("Sol")), "Codex Sol");
        assert_eq!(handoff_name(&AgentId::Codex, Some("GPT-5.6-Sol")), "GPT-5.6-Sol");
        assert_eq!(handoff_name(&AgentId::Direct("mock".into()), Some("Mock Swift")), "Mock Swift");
        assert_eq!(handoff_name(&AgentId::Direct("mock-relay".into()), Some("Relay Swift")), "Relay Swift");
        assert_eq!(handoff_name(&AgentId::Codex, None), "Codex");
    }

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }
    fn thought(t: &str) -> Item {
        Item::Reasoning { text: t.into() }
    }
    fn tool(id: &str) -> Item {
        Item::Tool { id: id.into(), title: "Ran ls".into(), detail: String::new(), output: String::new(), status: ToolStatus::Done }
    }

    #[test]
    fn tool_calls_and_finished_thoughts_fold_into_one_row() {
        let items = [user("go"), thought("plan"), tool("a"), tool("b"), said("done"), Item::TurnEnd { at: 1, took_secs: 2 }];
        let (slots, item_row) = layout(&items, None);
        assert_eq!(slots, [Slot::Item(0), Slot::Group(vec![1, 2, 3]), Slot::Item(4), Slot::Item(5)]);
        assert_eq!(item_row, [0, 1, 1, 1, 2, 3]);
        // A message hit on the answer lands on its own row, past the folded group.
        assert_eq!(row_of(&item_row, slots.len(), 4), Some(2));
    }

    #[test]
    fn empty_and_live_thoughts_point_at_the_next_row() {
        // An empty thought between two messages; a live one at the tail.
        let items = [user("go"), thought(" "), said("answer"), tool("a"), thought("still thinking")];
        let (slots, item_row) = layout(&items, Some(4));
        assert_eq!(slots, [Slot::Item(0), Slot::Item(2), Slot::Group(vec![3])]);
        assert_eq!(item_row[1], 1);
        // The live thought sits inside the open group (it isn't flushed yet), so it maps there.
        assert_eq!(item_row[4], 2);
        // Nothing after it: clamped to the last row.
        let items = [user("go"), thought("live")];
        let (slots, item_row) = layout(&items, Some(1));
        assert_eq!(slots, [Slot::Item(0)]);
        assert_eq!(row_of(&item_row, slots.len(), 1), Some(0));
        assert_eq!(row_of(&item_row, slots.len(), 9), Some(0));
        assert_eq!(row_of(&[], 0, 0), None);
    }

    #[test]
    fn sub_agents_get_rows_of_their_own_and_stand_for_the_call_that_started_them() {
        let call = |status| Item::Tool { id: "c".into(), title: "mcp__trek-orchestrate__delegate_task".into(), detail: String::new(), output: String::new(), status };
        let task = Item::Tool { id: trek_core::orchestrate::task_row("child"), title: "Sub-agent".into(), detail: "Review".into(), output: String::new(), status: ToolStatus::Running };
        let native = Item::Tool { id: "n".into(), title: "Subagent".into(), detail: "Scout".into(), output: String::new(), status: ToolStatus::Running };
        let items = [user("go"), tool("a"), call(ToolStatus::Running), task, tool("b"), native, said("done")];
        let (slots, item_row) = layout(&items, None);
        assert_eq!(slots, [Slot::Item(0), Slot::Group(vec![1]), Slot::Item(3), Slot::Group(vec![4]), Slot::Item(5), Slot::Item(6)]);
        assert_eq!(item_row[2], 2, "the hidden call maps to the sub-agent's row");
        // A call that failed started nothing: it shows, with why.
        let (slots, _) = layout(&[call(ToolStatus::Failed)], None);
        assert_eq!(slots, [Slot::Group(vec![0])]);
    }

    #[test]
    fn groups_split_at_messages() {
        let items = [tool("a"), said("between"), thought("t"), tool("b"), user("next")];
        let (slots, item_row) = layout(&items, None);
        assert_eq!(slots, [Slot::Group(vec![0]), Slot::Item(1), Slot::Group(vec![2, 3]), Slot::Item(4)]);
        assert_eq!(item_row, [0, 1, 2, 2, 3]);
    }
}
