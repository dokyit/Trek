//! The transcript: user turns, streaming markdown answers, collapsible reasoning and tool rows,
//! plus the live footer (thinking indicator, approval cards).

use crate::palette;
use crate::time;
use crate::workspace::{ItemRef, Route, Workspace};
use gpui_kit::component::button::Button;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_agents::Decision;
use trek_core::RunState;
use trek_core::store::{Item, ToolStatus};

const COLUMN: f32 = 760.;
/// How long a message a search result led to stays tinted (it holds, then fades).
const FLASH: std::time::Duration = std::time::Duration::from_millis(2200);

/// Rows that open and close carry their item's stable id (`key`): what's expanded follows the
/// item, not its position, which shifts when empty thoughts are dropped at the end of a turn.
#[derive(Clone)]
enum Row {
    User { ix: usize, key: SharedString, text: SharedString, open: bool, images: Vec<String>, at: Option<i64> },
    /// End of a response: copy the whole answer, when it finished, how long it took.
    TurnEnd { ix: usize, text: SharedString, at: i64, took_secs: u32 },
    Assistant(Entity<TextViewState>),
    Reasoning { ix: usize, key: SharedString, md: Entity<TextViewState>, live: bool, open: bool },
    Tool { ix: usize, key: SharedString, title: SharedString, detail: SharedString, output: SharedString, status: ToolStatus, open: bool, activity: Option<SharedString> },
    /// Consecutive tool calls folded into one summary line ("Ran 3 commands and edited 2 files").
    ToolGroup { ix: usize, key: SharedString, summary: SharedString, kind: ToolKind, running: bool, open: bool, tools: Vec<Row> },
    Notice(SharedString),
    Error(SharedString),
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
    for (ix, item) in items.iter().enumerate() {
        match item {
            Item::Tool { .. } => group.push(ix),
            Item::Reasoning { text } => {
                item_row[ix] = slots.len();
                if live_reasoning != Some(ix) && !text.trim().is_empty() {
                    group.push(ix);
                }
            }
            _ => {
                flush(&mut group, &mut slots, &mut item_row);
                item_row[ix] = slots.len();
                slots.push(Slot::Item(ix));
            }
        }
    }
    flush(&mut group, &mut slots, &mut item_row);
    (slots, item_row)
}

#[derive(Clone, Copy, PartialEq)]
enum ToolKind {
    Command,
    Edit,
    Read,
    Search,
    Other,
    Agent,
    Thought,
}

fn tool_kind(title: &str) -> ToolKind {
    match title {
        t if t.starts_with("Run") || t.starts_with("Ran") => ToolKind::Command,
        t if t.starts_with("Edit") || t.starts_with("Wr") || t.starts_with("Wrote") => ToolKind::Edit,
        t if t.starts_with("Read") || t.starts_with("Fetch") => ToolKind::Read,
        t if t.contains("Search") || t.starts_with("List") => ToolKind::Search,
        "Subagent" => ToolKind::Agent,
        _ => ToolKind::Other,
    }
}

fn kind_icon(kind: ToolKind) -> Icon {
    match kind {
        ToolKind::Command => Icon::new(IconName::SquareTerminal),
        ToolKind::Edit => Icon::new(crate::assets::Lucide::FilePen),
        ToolKind::Read => Icon::new(IconName::FileText),
        ToolKind::Search => Icon::new(IconName::Search),
        ToolKind::Other => Icon::new(crate::assets::Lucide::Wrench),
        ToolKind::Agent => Icon::new(crate::assets::Lucide::Users),
        ToolKind::Thought => Icon::new(crate::assets::Lucide::Sparkle),
    }
}

/// "Ran 3 commands, read 2 files, and edited 1 file"
fn summarize(kinds: &[ToolKind]) -> String {
    let count = |k: ToolKind| kinds.iter().filter(|x| **x == k).count();
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut parts = vec![];
    let (c, e, r, s, o) = (count(ToolKind::Command), count(ToolKind::Edit), count(ToolKind::Read), count(ToolKind::Search), count(ToolKind::Other));
    if c > 0 { parts.push(format!("ran {}", plural(c, "command", "commands"))); }
    if e > 0 { parts.push(format!("edited {}", plural(e, "file", "files"))); }
    if r > 0 { parts.push(format!("read {}", plural(r, "file", "files"))); }
    if s > 0 { parts.push(format!("ran {}", plural(s, "search", "searches"))); }
    if o > 0 { parts.push(format!("used {}", plural(o, "tool", "tools"))); }
    let a = count(ToolKind::Agent);
    if a > 0 { parts.push(format!("started {}", plural(a, "agent", "agents"))); }
    let text = match parts.len() {
        0 => "thought it through".to_string(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap();
            format!("{}, and {last}", parts.join(", "))
        }
    };
    let mut chars = text.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

pub struct ThreadView {
    workspace: Entity<Workspace>,
    scroller: Entity<MessageScrollerState>,
    current: Option<String>,
    revision: u64,
    count: usize,
    /// Markdown state per item id, with the byte length already pushed.
    md: HashMap<String, (Entity<TextViewState>, usize)>,
    /// Items (by id) opened by the user: long messages, thoughts, tool groups, tool output.
    expanded: HashSet<String>,
    /// Rows built for (thread, transcript revision, expansion state); rebuilt only when one changes,
    /// not on every animation frame.
    /// Picks so far for the question card on screen: (request, question index) → chosen labels.
    picks: HashMap<(String, usize), Vec<String>>,
    /// Rendered plan for the plan card on screen (request id, markdown).
    plan_md: Option<(String, Entity<TextViewState>)>,
    rows_cache: std::cell::RefCell<Option<((Option<String>, u64, u64, usize), std::rc::Rc<Rows>)>>,
    expanded_gen: u64,
    /// The last `Workspace::reveal` request handled.
    revealed: u64,
    /// The row a reveal scrolled to, tinted for a moment: (row, request).
    flash: Option<(usize, u64)>,
    /// Clears `flash` once its fade is over (a row scrolled back into view would replay it).
    _flash_timer: Option<Task<()>>,
    /// (transcript, UI) font sizes the rows were measured at; a change remeasures every row.
    fonts: (f32, f32),
    /// The window is frontmost. Animations stop when it isn't (they'd redraw 60×/s for nobody).
    active: bool,
    _subscriptions: Vec<Subscription>,
    _ticker: Option<Task<()>>,
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active();
                cx.notify();
            }),
        ];
        let mut this = Self {
            workspace,
            scroller,
            current: None,
            revision: 0,
            count: 0,
            md: HashMap::new(),
            expanded: HashSet::new(),
            picks: HashMap::new(),
            plan_md: None,
            rows_cache: Default::default(),
            expanded_gen: 0,
            revealed: 0,
            flash: None,
            _flash_timer: None,
            fonts: (0., 0.),
            active: window.is_window_active(),
            _subscriptions: subscriptions,
            _ticker: None,
        };
        this.sync(cx);
        this
    }

    fn animate(&self, _: &Window, cx: &App) -> bool {
        self.active && !self.workspace.read(cx).settings.appearance.reduce_motion
    }

    /// Bring row state in line with the workspace transcript without rebuilding everything.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let id = match &ws.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        let switched = id != self.current;
        let live = id.as_ref().and_then(|id| ws.live.get(id));
        let (revision, working) = live.map_or((0, false), |l| (l.revision, l.turn_started.is_some()));
        // Markdown that's new or changed since the last sync, by item id. Only these texts are
        // copied out; streaming touches one or two items per frame.
        let mut texts: Vec<(String, String)> = Vec::new();
        let mut text_items = 0;
        if let Some(l) = live.filter(|_| switched || revision != self.revision) {
            for (item, key) in l.items.iter().zip(l.item_ids()) {
                let (Item::Assistant { text } | Item::Reasoning { text }) = item else { continue };
                text_items += 1;
                let known = if switched { None } else { self.md.get(key).map(|(_, pushed)| *pushed) };
                if known != Some(text.len()) {
                    texts.push((key.clone(), text.clone()));
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
            self.md.clear();
            self.expanded.clear();
            self.expanded_gen += 1;
            self.count = 0;
            self.revision = 0;
            self.flash = None;
            self.scroller.update(cx, |s, cx| s.reset(0, cx));
        }
        if revision == self.revision && !switched {
            self.reveal(cx);
            cx.notify();
            return;
        }
        self.revision = revision;
        for (key, text) in texts {
            match self.md.get_mut(&key) {
                None => {
                    let len = text.len();
                    let state = cx.new(|cx| TextViewState::markdown(&text, cx));
                    self.md.insert(key, (state, len));
                }
                Some((state, pushed)) => {
                    let fits = text.len() > *pushed && text.is_char_boundary(*pushed);
                    let (state, from) = (state.clone(), *pushed);
                    *pushed = text.len();
                    state.update(cx, |s, cx| if fits { s.push_str(&text[from..], cx) } else { s.set_text(&text, cx) });
                }
            }
        }
        // Items removed from the transcript leave their markdown behind; drop it.
        if self.md.len() > text_items {
            let ws = self.workspace.read(cx);
            if let Some(l) = self.current.as_ref().and_then(|id| ws.live.get(id)) {
                let ids: HashSet<&String> = l.item_ids().iter().collect();
                self.md.retain(|k, _| ids.contains(k));
            }
        }
        let new_count = self.rows(cx).rows.len();
        let old = self.count;
        self.count = new_count;
        self.scroller.update(cx, |s, cx| {
            if switched || new_count < old {
                s.reset(new_count, cx);
                if switched {
                    s.scroll_to_end(cx);
                }
            } else if new_count > old {
                let _ = s.append(new_count - old, cx);
            }
            // Streaming only ever changes the tail; remeasure the last couple of rows.
            if new_count > 0 {
                let from = new_count.saturating_sub(2);
                let _ = s.remeasure_items(from..new_count, cx);
            }
        });
        // Tick the elapsed-time label once a second while a turn runs.
        if working && self._ticker.is_none() {
            // One ticker drives everything that moves while a turn runs: the hiker and the word
            // sweep at mascot::FPS when the window is in front, the elapsed time once a second otherwise.
            self._ticker = Some(cx.spawn(async move |this, cx| loop {
                let Ok(fast) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.active && !this.workspace.read(cx).settings.appearance.reduce_motion
                }) else {
                    break;
                };
                let wait = if fast { std::time::Duration::from_millis(1000 / crate::mascot::FPS) } else { std::time::Duration::from_secs(1) };
                cx.background_executor().timer(wait).await;
            }));
        } else if !working {
            self._ticker = None;
        }
        self.reveal(cx);
        cx.notify();
    }

    /// Scroll to the message `Workspace::reveal` asks for, once its thread is on screen and loaded.
    fn reveal(&mut self, cx: &mut Context<Self>) {
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
        let key = (self.current.clone(), revision, self.expanded_gen, self.md.len());
        if let Some((k, rows)) = self.rows_cache.borrow().as_ref() {
            if *k == key {
                return rows.clone();
            }
        }
        let rows = std::rc::Rc::new(self.build_rows(cx));
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
                        .filter_map(|ix| {
                            let key = &ids[ix];
                            match &live.items[ix] {
                                Item::Tool { id: tool_id, title, detail, output, status } => {
                                    // A running sub-agent shows what it's doing right now.
                                    let activity = live.tasks.iter().find(|t| &t.id == tool_id && t.done.is_none()).map(|t| {
                                        let steps = if t.tool_uses == 1 { "1 step".to_string() } else { format!("{} steps", t.tool_uses) };
                                        SharedString::from(if t.activity.is_empty() { steps } else { format!("{} · {steps}", t.activity) })
                                    });
                                    kinds.push(tool_kind(title));
                                    running |= *status == ToolStatus::Running;
                                    Some(Row::Tool {
                                        ix,
                                        key: key.clone().into(),
                                        title: title.clone().into(),
                                        detail: detail.clone().into(),
                                        output: output.clone().into(),
                                        status: *status,
                                        open: self.expanded.contains(key),
                                        activity,
                                    })
                                }
                                // Finished thoughts fold into the group; live thinking has no row
                                // (the trail bar above the composer is the one "working" indicator).
                                _ => self.md.get(key).map(|(md, _)| {
                                    kinds.push(ToolKind::Thought);
                                    Row::Reasoning { ix, key: key.clone().into(), md: md.clone(), live: false, open: self.expanded.contains(key) }
                                }),
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
                        Item::User { text, images, at } => {
                            Row::User { ix, key: key.clone().into(), text: text.clone().into(), open: self.expanded.contains(key), images: images.clone(), at: *at }
                        }
                        Item::TurnEnd { at, took_secs } => {
                            // Everything the agent said since the last message of yours.
                            let start = live.items[..ix].iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |u| u + 1);
                            let text = live.items[start..ix]
                                .iter()
                                .filter_map(|i| if let Item::Assistant { text } = i { Some(text.trim()) } else { None })
                                .filter(|t| !t.is_empty())
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            Row::TurnEnd { ix, text: text.into(), at: *at, took_secs: *took_secs }
                        }
                        Item::Assistant { .. } => match self.md.get(key) {
                            Some((s, _)) => Row::Assistant(s.clone()),
                            None => Row::Notice("".into()),
                        },
                        // Notices are plain text; drop the light markdown the built-in commands use.
                        Item::Notice { text } => Row::Notice(text.replace("**", "").replace('`', "").into()),
                        Item::Error { text } => Row::Error(text.clone().into()),
                        Item::Tool { .. } | Item::Reasoning { .. } => unreachable!("grouped by layout()"),
                    }
                }
            })
            .collect();
        Rows { rows, item_row }
    }

    /// One transcript row. `row_ix` is its place in the list (tool rows inside a group pass the
    /// group's); `flash` tints it for a moment after a search result scrolled to it.
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        row: Row,
        row_ix: usize,
        flash: Option<u64>,
        view: WeakEntity<ThreadView>,
        text_size: Pixels,
        cwd: Option<std::path::PathBuf>,
        animate: bool,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let child_view = view.clone();
        let tint = theme.foreground.opacity(0.07);
        let column = |el: Div| {
            let el = el.w_full().max_w(px(COLUMN));
            let el = match flash {
                // A soft band behind the message that holds briefly, then fades.
                Some(seq) => {
                    let band = div().absolute().top(px(-2.)).bottom(px(-2.)).left(px(-12.)).right(px(-12.)).rounded(px(10.)).bg(tint);
                    let band = if animate {
                        band.with_animation(("reveal", seq), Animation::new(FLASH), |el, t| el.opacity(1. - ((t - 0.45) / 0.55).clamp(0., 1.))).into_any_element()
                    } else {
                        band.into_any_element()
                    };
                    div().relative().w_full().max_w(px(COLUMN)).child(band).child(el)
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
                    cx.notify();
                });
            }
        };
        match row {
            Row::User { ix, key, text, open, images, at } => {
                let long = text.len() > 700 || text.lines().count() > 10;
                let has_text = !text.trim().is_empty();
                let copy_text = text.clone();
                // Time sent and a copy button, shown while the pointer is over the message.
                let meta = h_flex()
                    .h(px(20.))
                    .gap(px(6.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .invisible()
                    .group_hover("user-msg", |s| s.visible())
                    .children(at.map(crate::time::clock))
                    .child(
                        Button::new(("copy-user", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).text_color(theme.muted_foreground))
                            .tooltip("Copy message")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
                                window.push_notification("Copied", cx);
                            }),
                    );
                column(
                    v_flex().group("user-msg").items_end().pt_4().gap(px(4.))
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
                    .child(meta),
                )
            }
            .into_any_element(),
            Row::TurnEnd { ix, text, at, took_secs } => column(
                h_flex()
                    .pt(px(2.))
                    .pb(px(10.))
                    .gap(px(6.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        Button::new(("copy-turn", ix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).text_color(theme.muted_foreground))
                            .tooltip("Copy response")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                                window.push_notification("Copied", cx);
                            }),
                    )
                    .child(crate::time::clock(at))
                    .when(took_secs >= 1, |el| el.child(div().text_color(theme.muted_foreground.opacity(0.7)).child(format!("· {}", crate::time::took(took_secs))))),
            )
            .into_any_element(),
            Row::Assistant(md) => column(
                div().py_2().text_size(text_size).line_height(relative(1.62)).child(crate::md::view(&md, cwd.clone(), cx).stream_fade(true)),
            )
                .into_any_element(),
            Row::Reasoning { ix, key, md, live, open } => column(
                v_flex()
                    .py_1()
                    .child(
                        h_flex()
                            .id(("reasoning", ix))
                            .gap_1()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall())
                            .child(div().child(if live { "Thinking" } else { "Thought" }))
                            .on_click(toggle(key.clone())),
                    )
                    .when(open, |el| {
                        el.child(
                            div()
                                .ml_2()
                                .pl_3()
                                .border_l_2()
                                .border_color(theme.border)
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(TextView::new(&md).selectable(true)),
                        )
                    }),
            )
            .into_any_element(),
            Row::ToolGroup { ix, key, summary, kind, running, open, tools } => {
                let muted = theme.muted_foreground;
                column(
                    v_flex()
                        .py(px(3.))
                        .child(
                            h_flex()
                                .id(("tool-group", ix))
                                .gap_2()
                                .py_1()
                                .text_sm()
                                .text_color(muted)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child(kind_icon(kind).small())
                                .child(div().when(running, |el| el.text_color(theme.foreground.opacity(0.85))).child(summary))
                                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().opacity(0.6))
                                .on_click(toggle(key.clone())),
                        )
                        .when(open, |el| {
                            el.child(
                                v_flex()
                                    .ml(px(7.))
                                    .pl_4()
                                    .border_l_1()
                                    .border_color(theme.border)
                                    .children(tools.into_iter().map(|t| Self::render_row(t, row_ix, None, child_view.clone(), text_size, cwd.clone(), animate, cx))),
                            )
                        }),
                )
                .into_any_element()
            }
            Row::Tool { ix, key, title, detail, output, status, open, activity } => {
                let icon = match title.as_ref() {
                    "Subagent" => Icon::new(crate::assets::Lucide::Users),
                    t if t.starts_with("Run") || t.starts_with("Ran") => Icon::new(IconName::SquareTerminal),
                    t if t.starts_with("Edit") || t.starts_with("Wr") => Icon::new(crate::assets::Lucide::FilePen),
                    t if t.starts_with("Read") => Icon::new(IconName::FileText),
                    t if t.contains("Search") || t.starts_with("List") => Icon::new(IconName::Search),
                    _ => Icon::new(crate::assets::Lucide::Wrench),
                };
                let status_el = match status {
                    ToolStatus::Running => Icon::new(crate::assets::Lucide::LoaderCircle).xsmall().text_color(theme.muted_foreground).into_any_element(),
                    ToolStatus::Done => Icon::new(IconName::Check).xsmall().text_color(theme.muted_foreground).into_any_element(),
                    ToolStatus::Failed | ToolStatus::Denied => {
                        Icon::new(IconName::CircleX).xsmall().text_color(palette::red(cx)).into_any_element()
                    }
                };
                let has_output = !output.is_empty();
                (
                    v_flex()
                        .py(px(1.))
                        .child(
                            h_flex()
                                .id(("tool", ix))
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded(theme.radius)
                                .text_sm()
                                .when(has_output, |el| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(toggle(key.clone())))
                                .child(icon.small().text_color(theme.muted_foreground))
                                .child(div().font_medium().flex_none().max_w(relative(0.5)).truncate().child(if title.as_ref() == "Subagent" { detail.clone() } else { title.clone() }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(activity.is_none(), |el| el.font_family(theme.mono_font_family.clone()))
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(match (activity, title.as_ref()) {
                                            (Some(a), _) => a,
                                            (None, "Subagent") => SharedString::default(),
                                            (None, _) => detail,
                                        }),
                                )
                                .child(status_el),
                        )
                        .when(open && has_output, |el| {
                            el.child(
                                div()
                                    .id(("tool-out", ix))
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
                )
                .into_any_element()
            }
            Row::Notice(text) => column(
                h_flex().justify_center().py_1().text_xs().text_color(theme.muted_foreground).child(text),
            )
            .into_any_element(),
            Row::Error(text) => column(
                div()
                    .my_2()
                    .px_3()
                    .py_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(palette::red(cx).opacity(0.5))
                    .bg(palette::red(cx).opacity(0.08))
                    .text_sm()
                    .child(text),
            )
            .into_any_element(),
        }
    }

    /// The agent asked something only a person can answer: multiple-choice questions.
    fn question_card(&mut self, id: String, request_id: String, questions: Vec<trek_agents::Question>, agent: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ember = palette::ember(cx);
        let complete = questions.iter().enumerate().all(|(i, _)| self.picks.get(&(request_id.clone(), i)).is_some_and(|v| !v.is_empty()));
        let answers: Vec<(String, String)> =
            questions.iter().enumerate().map(|(i, q)| (q.question.clone(), self.picks.get(&(request_id.clone(), i)).map(|v| v.join(", ")).unwrap_or_default())).collect();
        let (ws, ws2) = (self.workspace.clone(), self.workspace.clone());
        let (id2, rid2, id3, rid3) = (id.clone(), request_id.clone(), id.clone(), request_id.clone());
        let body = v_flex().gap(px(14.)).children(questions.iter().enumerate().map(|(qi, q)| {
            let picked = self.picks.get(&(request_id.clone(), qi)).cloned().unwrap_or_default();
            v_flex()
                .gap(px(6.))
                .child(div().text_size(px(13.5)).font_medium().child(q.question.clone()))
                .when(q.multi, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("Choose any that apply.")))
                .children(q.options.iter().enumerate().map(|(oi, (label, desc))| {
                    let on = picked.contains(label);
                    let (rid, label2, multi) = (request_id.clone(), label.clone(), q.multi);
                    h_flex()
                        .id(SharedString::from(format!("q-{request_id}-{qi}-{oi}")))
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let entry = this.picks.entry((rid.clone(), qi)).or_default();
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
                        }))
                }))
        }));
        v_flex()
            .w_full()
            .max_w(px(COLUMN))
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
                    .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child("Or type your own answer below and send it."))
                    .child(Button::new("q-skip").small().ghost().label("Skip").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.respond(&id2, &rid2, Decision::Deny, cx))))
                    .child(Button::new("q-send").small().primary().label("Answer").disabled(!complete).on_click(move |_, _, cx| {
                        let answers = answers.clone();
                        ws2.update(cx, |ws, cx| ws.answer(&id3, &rid3, answers, cx))
                    })),
            )
            .into_any_element()
    }

    /// The agent finished planning and wants a go-ahead before changing anything.
    fn plan_card(&mut self, id: String, request_id: String, plan: String, agent: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.plan_md.as_ref().is_none_or(|(rid, _)| *rid != request_id) {
            let text = plan.clone();
            self.plan_md = Some((request_id.clone(), cx.new(|cx| TextViewState::markdown(&text, cx))));
        }
        let md = self.plan_md.as_ref().map(|(_, m)| m.clone());
        let cwd = self.workspace.read(cx).current_cwd();
        let (ws, ws2) = (self.workspace.clone(), self.workspace.clone());
        let (id2, rid2, id3, rid3) = (id.clone(), request_id.clone(), id, request_id);
        v_flex()
            .w_full()
            .max_w(px(COLUMN))
            .max_h(px(440.))
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(14.))
            .border_1()
            .border_color(palette::indigo(cx).opacity(0.45))
            .bg(theme.secondary)
            .child(h_flex().gap_2().text_sm().child(Icon::new(crate::assets::Lucide::ListChecks).small().text_color(palette::indigo(cx))).child(div().font_semibold().child(format!("{agent}'s plan"))))
            .child(div().id("plan-scroll").flex_1().min_h_0().overflow_y_scroll().text_size(px(13.5)).line_height(relative(1.55)).children(md.map(|m| crate::md::view(&m, cwd, cx))))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child("Nothing has been changed yet."))
                    .child(Button::new("plan-revise").small().outline().label("Keep planning").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.respond(&id2, &rid2, Decision::Deny, cx))))
                    .child(Button::new("plan-approve").small().primary().label("Approve and start").on_click(move |_, _, cx| ws2.update(cx, |ws, cx| ws.approve_plan(&id3, &rid3, cx)))),
            )
            .into_any_element()
    }

    fn live_footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
                trek_agents::Prompt::Questions(q) => self.question_card(id.clone(), request_id, q, agent, cx),
                trek_agents::Prompt::Plan(plan) => self.plan_card(id.clone(), request_id, plan, agent, cx),
            };
            return Some(h_flex().w_full().justify_center().px_6().pb_2().child(card).into_any_element());
        }
        let ws = self.workspace.read(cx);
        let live = ws.live.get(&id)?;
        let thread = ws.thread(&id)?;
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
                            .max_w(px(COLUMN))
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
        if thread.run_state == RunState::Working {
            let started = live.turn_started.map(|t| t.elapsed());
            let elapsed = started.map(time::elapsed).unwrap_or_default();
            let word = crate::mascot::word(&id, started.map(|d| d.as_secs()).unwrap_or(0));
            let still = ws.settings.appearance.reduce_motion || !self.active;
            let clock = live.turn_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.);
            let agents = live.active_tasks().max(live.background);
            return Some(
                h_flex()
                    .w_full()
                    .justify_center()
                    .px_6()
                    .pb(px(6.))
                    .child(
                        h_flex()
                            .w_full()
                            .max_w(px(COLUMN))
                            .px(px(4.))
                            .gap(px(14.))
                            .items_end()
                            .child(
                                h_flex()
                                    .flex_none()
                                    // Fixed width so the trail doesn't jump when the word changes.
                                    .w(px(330.))
                                    .pb(px(4.))
                                    .gap(px(8.))
                                    .text_size(px(13.))
                                    .child(crate::mascot::word_label(word, clock, still, cx))
                                    .when(!elapsed.is_empty(), |el| el.child(div().text_color(theme.muted_foreground.opacity(0.8)).child(elapsed)))
                                    .when(agents > 0, |el| {
                                        el.child(div().text_color(theme.muted_foreground.opacity(0.8)).child(if agents == 1 { "· 1 agent out".to_string() } else { format!("· {agents} agents out") }))
                                    }),
                            )
                            .child(div().flex_1().min_w_0().child(crate::mascot::trail(clock, still, cx))),
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
        let show = a.background_placement == trek_core::settings::BackgroundPlacement::NewThread && matches!(ws.route, Route::Draft { .. });
        let spec = if show { a.background.clone() } else { None };
        div().relative().size_full().child(crate::ui::hero_background(spec.as_deref(), a.background_dim, cx)).into_any_element()
    }
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let footer = self.live_footer(cx);
        if rows.rows.is_empty() {
            return v_flex().size_full().child(self.empty_state(cx)).children(footer);
        }
        let view = cx.entity().downgrade();
        let text_size = px(self.workspace.read(cx).settings.appearance.transcript_font_size());
        let cwd = self.workspace.read(cx).current_cwd();
        let animate = self.animate(window, cx);
        let flash = self.flash;
        v_flex()
            .size_full()
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("transcript", self.scroller.clone(), move |ix, _, cx| match rows.rows.get(ix).cloned() {
                        Some(row) => {
                            let flash = flash.filter(|(row, _)| *row == ix).map(|(_, seq)| seq);
                            ThreadView::render_row(row, ix, flash, view.clone(), text_size, cwd.clone(), animate, cx)
                        }
                        None => div().into_any_element(),
                    })
                    .with_list_style(StyleRefinement::default().pt_4().pb_6())
                    .with_row_style(StyleRefinement::default().pb_0())
                    .with_bottom_fade(cx.theme().background),
                ),
            )
            .children(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::{Slot, layout, row_of};
    use trek_core::store::{Item, ToolStatus};

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None }
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
    fn groups_split_at_messages() {
        let items = [tool("a"), said("between"), thought("t"), tool("b"), user("next")];
        let (slots, item_row) = layout(&items, None);
        assert_eq!(slots, [Slot::Group(vec![0]), Slot::Item(1), Slot::Group(vec![2, 3]), Slot::Item(4)]);
        assert_eq!(item_row, [0, 1, 2, 2, 3]);
    }
}
