//! The transcript: user turns, streaming markdown answers, collapsible reasoning and tool rows,
//! plus the cards the agent puts to the user (approvals, questions, plans). The working bar above
//! the composer is its own view (`working_bar`).

use crate::palette;
use crate::workspace::{Route, Workspace, WorkspaceEvent};
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
use trek_core::store::{Item, ToolStatus};

const COLUMN: f32 = 760.;

/// A transcript row. Rows point into the thread's items; text is read when a row is drawn, so
/// building rows for a long thread copies nothing.
#[derive(Clone)]
enum Row {
    User { ix: usize, open: bool },
    /// End of a response: copy the whole answer, when it finished, how long it took.
    TurnEnd { ix: usize },
    Assistant { ix: usize },
    Reasoning { ix: usize, open: bool },
    Tool { ix: usize, open: bool, activity: Option<SharedString> },
    /// Consecutive tool calls folded into one summary line ("Ran 3 commands and edited 2 files").
    ToolGroup { ix: usize, summary: SharedString, kind: ToolKind, running: bool, open: bool, tools: Vec<Row> },
    Notice { ix: usize },
    Error { ix: usize },
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
    /// Markdown documents by transcript index, with the text they hold. Built when a row is first
    /// drawn, so opening a long thread doesn't parse all of it.
    md: HashMap<usize, Markdown>,
    expanded: HashSet<usize>,
    /// Picks so far for the question card on screen: (request, question index) → chosen labels.
    picks: HashMap<(String, usize), Vec<String>>,
    /// Rendered plan for the plan card on screen (request id, markdown).
    plan_md: Option<(String, Entity<TextViewState>, Subscription)>,
    /// Rows built for (thread, transcript revision, expansion state); rebuilt only when one changes.
    rows_cache: std::cell::RefCell<Option<((Option<String>, u64, u64), std::rc::Rc<Vec<Row>>)>>,
    expanded_gen: u64,
    /// (transcript, UI) font sizes the rows were measured at; a change remeasures every row.
    fonts: (f32, f32),
    /// What the last render showed, to skip workspace changes that don't touch the transcript.
    shown: Option<Shown>,
    _subscriptions: Vec<Subscription>,
}

struct Markdown {
    state: Entity<TextViewState>,
    /// The text the document holds. Items can move to another index (empty thoughts are dropped
    /// when a turn ends), so a document only takes the tail of text that extends this.
    text: String,
    /// Parsing finishes in the background and streamed text fades in; this view is cached, so it
    /// re-renders when the document says so.
    _changed: Subscription,
}

/// The workspace state the transcript is drawn from.
#[derive(Clone, PartialEq)]
struct Shown {
    route: Route,
    revision: u64,
    agent: Option<trek_core::AgentId>,
    loading: bool,
    appearance: trek_core::settings::Appearance,
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(false, cx)),
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| {
                if let WorkspaceEvent::Transcript { appended, .. } = event {
                    this.sync(*appended, cx);
                }
            }),
            cx.observe(&scroller, |_, _, cx| cx.notify()),
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
            fonts: (0., 0.),
            shown: None,
            _subscriptions: subscriptions,
        };
        this.sync(false, cx);
        this
    }

    /// Bring row state in line with the workspace transcript without rebuilding everything.
    /// `appended`: only text was added to the messages already streaming. Their documents redraw
    /// this view once the new text is parsed, so it doesn't redraw before that (it would show the
    /// same text again).
    fn sync(&mut self, appended: bool, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let id = match &ws.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        let live = id.as_ref().and_then(|id| ws.live.get(id));
        let revision = live.map_or(0, |l| l.revision);
        let shown = Shown {
            route: ws.route.clone(),
            revision,
            agent: id.as_ref().and_then(|id| ws.thread(id)).map(|t| t.agent.clone()),
            loading: live.is_some_and(|l| l.loading),
            appearance: ws.settings.appearance.clone(),
        };
        // Only the documents already built need updating, and only where the text changed (while
        // streaming, that's the tail).
        let changed: Vec<(usize, String)> = match live {
            Some(l) if revision != self.revision && id == self.current => self
                .md
                .iter()
                .filter_map(|(ix, m)| match l.items.get(*ix) {
                    Some(Item::Assistant { text } | Item::Reasoning { text }) if *text != m.text => Some((*ix, text.clone())),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        let fonts = (ws.settings.appearance.transcript_font_size(), ws.settings.appearance.ui_font_size());
        if fonts != self.fonts {
            self.fonts = fonts;
            let count = self.count;
            if count > 0 {
                self.scroller.update(cx, |s, cx| _ = s.remeasure_items(0..count, cx));
            }
        }
        let switched = id != self.current;
        if switched {
            self.current = id;
            self.md.clear();
            self.expanded.clear();
            self.expanded_gen += 1;
            self.count = 0;
            self.revision = 0;
            self.scroller.update(cx, |s, cx| s.reset(0, cx));
        }
        let quiet = appended && !switched && self.shown.as_ref().is_some_and(|s| Shown { revision, ..s.clone() } == shown);
        if self.shown.as_ref() != Some(&shown) {
            self.shown = Some(shown);
            if !quiet {
                cx.notify();
            }
        }
        if revision == self.revision && !switched {
            return;
        }
        self.revision = revision;
        for (ix, text) in changed {
            if let Some(m) = self.md.get_mut(&ix) {
                // Streaming appends (parsed incrementally); anything else replaces the document.
                match text.strip_prefix(m.text.as_str()) {
                    Some(tail) => m.state.update(cx, |s, cx| s.push_str(tail, cx)),
                    None => m.state.update(cx, |s, cx| s.set_text(&text, cx)),
                }
                m.text = text;
            }
        }
        let new_count = self.rows(cx).len();
        let old = self.count;
        if quiet && new_count == old {
            return;
        }
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
        cx.notify();
    }

    /// The markdown document for transcript item `ix`, built the first time its row is drawn.
    fn markdown(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<Entity<TextViewState>> {
        if let Some(m) = self.md.get(&ix) {
            return Some(m.state.clone());
        }
        let text = match self.current.as_ref().and_then(|id| self.workspace.read(cx).live.get(id)).and_then(|l| l.items.get(ix))? {
            Item::Assistant { text } | Item::Reasoning { text } => text.clone(),
            _ => return None,
        };
        let state = cx.new(|cx| TextViewState::markdown(&text, cx));
        let changed = cx.observe(&state, |_, _, cx| cx.notify());
        self.md.insert(ix, Markdown { state: state.clone(), text, _changed: changed });
        Some(state)
    }

    fn rows(&self, cx: &App) -> std::rc::Rc<Vec<Row>> {
        let revision = self.current.as_ref().and_then(|id| self.workspace.read(cx).live.get(id)).map(|l| l.revision).unwrap_or(0);
        let key = (self.current.clone(), revision, self.expanded_gen);
        if let Some((k, rows)) = self.rows_cache.borrow().as_ref() {
            if *k == key {
                return rows.clone();
            }
        }
        let rows = std::rc::Rc::new(self.build_rows(cx));
        *self.rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    fn build_rows(&self, cx: &App) -> Vec<Row> {
        let ws = self.workspace.read(cx);
        let Some(live) = self.current.as_ref().and_then(|id| ws.live.get(id)) else { return vec![] };
        let mut out: Vec<Row> = Vec::new();
        let mut pending: Vec<(usize, Row, ToolKind, bool)> = Vec::new();
        let flush = |pending: &mut Vec<(usize, Row, ToolKind, bool)>, out: &mut Vec<Row>, expanded: &HashSet<usize>| {
            if pending.is_empty() {
                return;
            }
            let ix = pending[0].0;
            let kinds: Vec<ToolKind> = pending.iter().map(|p| p.2).collect();
            let tool_kinds: Vec<ToolKind> = kinds.iter().copied().filter(|k| *k != ToolKind::Thought).collect();
            let running = pending.iter().any(|p| p.3);
            let tools: Vec<Row> = pending.drain(..).map(|p| p.1).collect();
            out.push(Row::ToolGroup {
                ix,
                summary: summarize(&tool_kinds).into(),
                kind: tool_kinds.last().copied().unwrap_or(ToolKind::Thought),
                running,
                open: expanded.contains(&ix),
                tools,
            });
        };
        for (ix, item) in live.items.iter().enumerate() {
            match item {
                Item::Tool { id: tool_id, title, status, .. } => {
                    // A running sub-agent shows what it's doing right now.
                    let activity = live.tasks.iter().find(|t| &t.id == tool_id && t.done.is_none()).map(|t| {
                        let steps = if t.tool_uses == 1 { "1 step".to_string() } else { format!("{} steps", t.tool_uses) };
                        SharedString::from(if t.activity.is_empty() { steps } else { format!("{} · {steps}", t.activity) })
                    });
                    pending.push((ix, Row::Tool { ix, open: self.expanded.contains(&ix), activity }, tool_kind(title), *status == ToolStatus::Running));
                }
                // Live thinking has no row of its own: the working bar above the composer is the one
                // "working" indicator. Finished thoughts fold into the tool group.
                Item::Reasoning { text } => {
                    if live.reasoning != Some(ix) && !text.trim().is_empty() {
                        pending.push((ix, Row::Reasoning { ix, open: self.expanded.contains(&ix) }, ToolKind::Thought, false));
                    }
                }
                _ => {
                    flush(&mut pending, &mut out, &self.expanded);
                    out.push(match item {
                        Item::User { .. } => Row::User { ix, open: self.expanded.contains(&ix) },
                        Item::TurnEnd { .. } => Row::TurnEnd { ix },
                        Item::Assistant { .. } => Row::Assistant { ix },
                        Item::Notice { .. } => Row::Notice { ix },
                        Item::Error { .. } => Row::Error { ix },
                        Item::Tool { .. } | Item::Reasoning { .. } => unreachable!(),
                    });
                }
            }
        }
        flush(&mut pending, &mut out, &self.expanded);
        out
    }

    /// Everything the agent said between the user's message before `ix` and `ix`.
    fn response_text(items: &[Item], ix: usize) -> String {
        let start = items[..ix.min(items.len())].iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |u| u + 1);
        items[start..ix.min(items.len())]
            .iter()
            .filter_map(|i| if let Item::Assistant { text } = i { Some(text.trim()) } else { None })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn render_row(row: Row, view: &WeakEntity<ThreadView>, at: &RowContext, cx: &mut App) -> AnyElement {
        // Documents and nested rows need the app mutably; everything else only reads it.
        let md = match &row {
            Row::Assistant { ix } | Row::Reasoning { ix, open: true } => view.update(cx, |this, cx| this.markdown(*ix, cx)).ok().flatten(),
            _ => None,
        };
        let children: Vec<AnyElement> = match &row {
            Row::ToolGroup { open: true, tools, .. } => tools.iter().map(|t| Self::render_row(t.clone(), view, at, cx)).collect(),
            _ => vec![],
        };
        let cx: &App = cx;
        let Some(item) = at.workspace.read(cx).live.get(&at.thread).and_then(|l| l.items.get(row_ix(&row))).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let text_size = at.text_size;
        let column = |el: Div| h_flex().w_full().justify_center().px_6().child(el.w_full().max_w(px(COLUMN)));
        let toggle = move |ix: usize| {
            let view = view.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| {
                    if !this.expanded.remove(&ix) {
                        this.expanded.insert(ix);
                    }
                    this.expanded_gen += 1;
                    this.scroller.update(cx, |s, cx| _ = s.remeasure_items(ix..ix + 1, cx));
                    cx.notify();
                });
            }
        };
        match (row, item) {
            (Row::User { ix, open }, Item::User { text, images, at: sent }) => {
                let text = SharedString::from(text);
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
                    .children(sent.map(crate::time::clock))
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
                                        .on_click(toggle(ix)),
                                )
                            }),
                    ))
                    .child(meta),
                )
                .into_any_element()
            }
            (Row::TurnEnd { ix }, Item::TurnEnd { at: finished, took_secs }) => {
                let (workspace, thread) = (at.workspace.clone(), at.thread.clone());
                column(
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
                                    let text = workspace.read(cx).live.get(&thread).map(|l| Self::response_text(&l.items, ix)).unwrap_or_default();
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    window.push_notification("Copied", cx);
                                }),
                        )
                        .child(crate::time::clock(finished))
                        .when(took_secs >= 1, |el| el.child(div().text_color(theme.muted_foreground.opacity(0.7)).child(format!("· {}", crate::time::took(took_secs))))),
                )
                .into_any_element()
            }
            (Row::Assistant { ix }, _) => match md {
                Some(md) => column(div().py_2().text_size(text_size).line_height(relative(1.62)).child(crate::md::view(&md, at.cwd.clone(), cx).stream_fade(true)))
                    .id(("answer", ix))
                    .test_support()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            (Row::Reasoning { ix, open }, _) => column(
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
                            .child(div().child("Thought"))
                            .on_click(toggle(ix)),
                    )
                    .when_some(md.filter(|_| open), |el, md| {
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
            (Row::ToolGroup { ix, summary, kind, running, open, .. }, _) => {
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
                                .child(kind_icon(kind).small())
                                .child(div().when(running, |el| el.text_color(theme.foreground.opacity(0.85))).child(summary))
                                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().opacity(0.6))
                                .on_click(toggle(ix)),
                        )
                        .when(open, |el| el.child(v_flex().ml(px(7.)).pl_4().border_l_1().border_color(theme.border).children(children))),
                )
                .into_any_element()
            }
            (Row::Tool { ix, open, activity }, Item::Tool { title, detail, output, status, .. }) => {
                let icon = match title.as_str() {
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
                    ToolStatus::Failed | ToolStatus::Denied => Icon::new(IconName::CircleX).xsmall().text_color(palette::red(cx)).into_any_element(),
                };
                let has_output = !output.is_empty();
                let subagent = title == "Subagent";
                let label = SharedString::from(if subagent { detail.clone() } else { title });
                let code = activity.is_none();
                let detail = match activity {
                    Some(a) => a,
                    None if subagent => SharedString::default(),
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
                            .when(has_output, |el| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(toggle(ix)))
                            .child(icon.small().text_color(theme.muted_foreground))
                            .child(div().font_medium().flex_none().max_w(relative(0.5)).truncate().child(label))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(code, |el| el.font_family(theme.mono_font_family.clone()))
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            )
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
            // Notices are plain text; drop the light markdown the built-in commands use.
            (Row::Notice { .. }, Item::Notice { text }) => column(
                h_flex().justify_center().py_1().text_xs().text_color(theme.muted_foreground).child(text.replace("**", "").replace('`', "")),
            )
            .into_any_element(),
            (Row::Error { .. }, Item::Error { text }) => column(
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
            // The transcript changed under the row (it's rebuilt on the next render).
            _ => div().into_any_element(),
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
        if self.plan_md.as_ref().is_none_or(|(rid, _, _)| *rid != request_id) {
            let text = plan.clone();
            let md = cx.new(|cx| TextViewState::markdown(&text, cx));
            let parsed = cx.observe(&md, |_, _, cx| cx.notify());
            self.plan_md = Some((request_id.clone(), md, parsed));
        }
        let md = self.plan_md.as_ref().map(|(_, m, _)| m.clone());
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

    /// An approval, question or plan waiting on the user, in place of the working bar.
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

/// What every row of one render shares.
struct RowContext {
    workspace: Entity<Workspace>,
    thread: String,
    text_size: Pixels,
    cwd: Option<std::path::PathBuf>,
}

fn row_ix(row: &Row) -> usize {
    match row {
        Row::User { ix, .. }
        | Row::TurnEnd { ix }
        | Row::Assistant { ix }
        | Row::Reasoning { ix, .. }
        | Row::Tool { ix, .. }
        | Row::ToolGroup { ix, .. }
        | Row::Notice { ix }
        | Row::Error { ix } => *ix,
    }
}

impl Render for ThreadView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("ThreadView");
        let rows = self.rows(cx);
        let footer = self.live_footer(cx);
        let (Some(thread), false) = (self.current.clone(), rows.is_empty()) else {
            return v_flex().size_full().child(self.empty_state(cx)).children(footer);
        };
        let view = cx.entity().downgrade();
        let ws = self.workspace.read(cx);
        let at = RowContext { workspace: self.workspace.clone(), thread, text_size: px(ws.settings.appearance.transcript_font_size()), cwd: ws.current_cwd() };
        v_flex()
            .size_full()
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("transcript", self.scroller.clone(), move |ix, _, cx| match rows.get(ix).cloned() {
                        Some(row) => ThreadView::render_row(row, &view, &at, cx),
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
impl ThreadView {
    /// Markdown documents built so far.
    pub(crate) fn markdown_states(&self) -> usize {
        self.md.len()
    }

    /// The markdown documents built so far, by transcript index.
    pub(crate) fn markdown_documents(&self) -> Vec<(usize, Entity<TextViewState>)> {
        let mut docs: Vec<_> = self.md.iter().map(|(ix, m)| (*ix, m.state.clone())).collect();
        docs.sort_by_key(|(ix, _)| *ix);
        docs
    }

    /// The rows as text: "user", "assistant", "group: Ran 1 command" (with "  tool: Read" lines
    /// under an open group), "end", "notice: …", "error: …".
    pub(crate) fn describe(&self, cx: &App) -> Vec<String> {
        fn line(row: &Row, out: &mut Vec<String>, indent: &str) {
            out.push(match row {
                Row::User { .. } => format!("{indent}user"),
                Row::TurnEnd { .. } => format!("{indent}end"),
                Row::Assistant { .. } => format!("{indent}assistant"),
                Row::Reasoning { .. } => format!("{indent}thought"),
                Row::Tool { activity, .. } => format!("{indent}tool{}", activity.as_ref().map(|a| format!(" ({a})")).unwrap_or_default()),
                Row::ToolGroup { summary, running, .. } => format!("{indent}group: {summary}{}", if *running { " (running)" } else { "" }),
                Row::Notice { .. } => format!("{indent}notice"),
                Row::Error { .. } => format!("{indent}error"),
            });
            if let Row::ToolGroup { open: true, tools, .. } = row {
                for t in tools {
                    line(t, out, "  ");
                }
            }
        }
        let mut out = vec![];
        for row in self.rows(cx).iter() {
            line(row, &mut out, "");
        }
        out
    }
}
