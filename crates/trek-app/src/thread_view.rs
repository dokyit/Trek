//! The transcript: user turns, streaming markdown answers, collapsible reasoning and tool rows,
//! plus the live footer (thinking indicator, approval cards).

use crate::palette;
use crate::time;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::Button;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_agents::Decision;
use trek_core::RunState;
use trek_core::store::{Item, ToolStatus};

const COLUMN: f32 = 760.;

#[derive(Clone)]
enum Row {
    User { ix: usize, text: SharedString, open: bool, images: Vec<String> },
    Assistant(Entity<TextViewState>),
    Reasoning { ix: usize, md: Entity<TextViewState>, live: bool, open: bool },
    Tool { ix: usize, title: SharedString, detail: SharedString, output: SharedString, status: ToolStatus, open: bool },
    /// Consecutive tool calls folded into one summary line ("Ran 3 commands and edited 2 files").
    ToolGroup { ix: usize, summary: SharedString, kind: ToolKind, running: bool, open: bool, tools: Vec<Row> },
    Notice(SharedString),
    Error(SharedString),
}

#[derive(Clone, Copy, PartialEq)]
enum ToolKind {
    Command,
    Edit,
    Read,
    Search,
    Other,
    Thought,
}

fn tool_kind(title: &str) -> ToolKind {
    match title {
        t if t.starts_with("Run") || t.starts_with("Ran") => ToolKind::Command,
        t if t.starts_with("Edit") || t.starts_with("Wr") || t.starts_with("Wrote") => ToolKind::Edit,
        t if t.starts_with("Read") || t.starts_with("Fetch") => ToolKind::Read,
        t if t.contains("Search") || t.starts_with("List") => ToolKind::Search,
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
    /// Markdown state per transcript index, with the byte length already pushed.
    md: HashMap<usize, (Entity<TextViewState>, usize)>,
    expanded: HashSet<usize>,
    /// (transcript, UI) font sizes the rows were measured at; a change remeasures every row.
    fonts: (f32, f32),
    _subscriptions: Vec<Subscription>,
    _ticker: Option<Task<()>>,
}

impl ThreadView {
    pub fn new(workspace: Entity<Workspace>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = vec![cx.observe(&workspace, |this, _, cx| this.sync(cx))];
        let mut this = Self {
            workspace,
            scroller,
            current: None,
            revision: 0,
            count: 0,
            md: HashMap::new(),
            expanded: HashSet::new(),
            fonts: (0., 0.),
            _subscriptions: subscriptions,
            _ticker: None,
        };
        this.sync(cx);
        this
    }

    /// Bring row state in line with the workspace transcript without rebuilding everything.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let id = match &ws.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        let (items, revision, working) = match id.as_ref().and_then(|id| ws.live.get(id)) {
            Some(l) => (l.items.clone(), l.revision, l.turn_started.is_some()),
            None => (vec![], 0, false),
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
            self.count = 0;
            self.revision = 0;
            self.scroller.update(cx, |s, cx| s.reset(0, cx));
        }
        if revision == self.revision && !switched {
            cx.notify();
            return;
        }
        self.revision = revision;
        let mut changed: Vec<usize> = Vec::new();
        for (ix, item) in items.iter().enumerate() {
            let text = match item {
                Item::Assistant { text } | Item::Reasoning { text } => text,
                _ => continue,
            };
            match self.md.get_mut(&ix) {
                None => {
                    let t = text.clone();
                    let state = cx.new(|cx| TextViewState::markdown(&t, cx));
                    self.md.insert(ix, (state, text.len()));
                }
                Some((state, pushed)) if text.len() != *pushed => {
                    let fits = text.len() > *pushed && text.is_char_boundary(*pushed);
                    let (state, from) = (state.clone(), *pushed);
                    let t = text.clone();
                    state.update(cx, |s, cx| if fits { s.push_str(&t[from..], cx) } else { s.set_text(&t, cx) });
                    *pushed = text.len();
                    changed.push(ix);
                }
                _ => {}
            }
        }
        let new_count = self.rows(cx).len();
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
            let _ = changed;
            if new_count > 0 {
                let from = new_count.saturating_sub(2);
                let _ = s.remeasure_items(from..new_count, cx);
            }
        });
        // Tick the elapsed-time label once a second while a turn runs.
        if working && self._ticker.is_none() {
            self._ticker = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }));
        } else if !working {
            self._ticker = None;
        }
        cx.notify();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
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
            if let Item::Tool { title, detail, output, status, .. } = item {
                pending.push((
                    ix,
                    Row::Tool {
                        ix,
                        title: title.clone().into(),
                        detail: detail.clone().into(),
                        output: output.clone().into(),
                        status: *status,
                        open: self.expanded.contains(&ix),
                    },
                    tool_kind(title),
                    *status == ToolStatus::Running,
                ));
                continue;
            }
            if let Item::Reasoning { text } = item {
                if live.reasoning != Some(ix) {
                    if !text.trim().is_empty() {
                        if let Some((md, _)) = self.md.get(&ix) {
                            pending.push((ix, Row::Reasoning { ix, md: md.clone(), live: false, open: self.expanded.contains(&ix) }, ToolKind::Thought, false));
                        }
                    }
                    continue;
                }
            }
            flush(&mut pending, &mut out, &self.expanded);
            out.push(match item {
                Item::User { text, images } => Row::User { ix, text: text.clone().into(), open: self.expanded.contains(&ix), images: images.clone() },
                Item::Assistant { .. } => match self.md.get(&ix) {
                    Some((s, _)) => Row::Assistant(s.clone()),
                    None => Row::Notice("".into()),
                },
                Item::Reasoning { .. } => match self.md.get(&ix) {
                    Some((s, _)) => Row::Reasoning { ix, md: s.clone(), live: live.reasoning == Some(ix), open: self.expanded.contains(&ix) },
                    None => Row::Notice("".into()),
                },
                Item::Notice { text } => Row::Notice(text.clone().into()),
                Item::Error { text } => Row::Error(text.clone().into()),
                Item::Tool { .. } => unreachable!(),
            });
        }
        flush(&mut pending, &mut out, &self.expanded);
        out
    }

    fn render_row(row: Row, view: WeakEntity<ThreadView>, text_size: Pixels, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let child_view = view.clone();
        let column = |el: Div| h_flex().w_full().justify_center().px_6().child(el.w_full().max_w(px(COLUMN)));
        let toggle = move |ix: usize| {
            let view = view.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| {
                    if !this.expanded.remove(&ix) {
                        this.expanded.insert(ix);
                    }
                    this.scroller.update(cx, |s, cx| _ = s.remeasure_items(ix..ix + 1, cx));
                    cx.notify();
                });
            }
        };
        match row {
            Row::User { ix, text, open, images } => {
                let long = text.len() > 700 || text.lines().count() > 10;
                let has_text = !text.trim().is_empty();
                column(
                    v_flex().items_end().pt_4().pb_2().gap_2()
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
                    )),
                )
            }
            .into_any_element(),
            Row::Assistant(md) => column(
                div().py_2().text_size(text_size).line_height(relative(1.6)).child(TextView::new(&md).selectable(true).stream_fade(true)),
            )
                .into_any_element(),
            Row::Reasoning { ix, md, live, open } => column(
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
                            .child(if live {
                                ShimmerText::new("Thinking").id(("thinking", ix)).highlight_color(palette::ember(cx)).into_any_element()
                            } else {
                                div().child("Thought").into_any_element()
                            })
                            .on_click(toggle(ix)),
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
            Row::ToolGroup { ix, summary, kind, running, open, tools } => {
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
                                .child(if running {
                                    ShimmerText::new(summary).id(("tool-shimmer", ix)).highlight_color(theme.foreground).into_any_element()
                                } else {
                                    div().child(summary).into_any_element()
                                })
                                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().opacity(0.6))
                                .on_click(toggle(ix)),
                        )
                        .when(open, |el| {
                            el.child(
                                v_flex()
                                    .ml(px(7.))
                                    .pl_4()
                                    .border_l_1()
                                    .border_color(theme.border)
                                    .children(tools.into_iter().map(|t| Self::render_row(t, child_view.clone(), text_size, cx))),
                            )
                        }),
                )
                .into_any_element()
            }
            Row::Tool { ix, title, detail, output, status, open } => {
                let icon = match title.as_ref() {
                    t if t.starts_with("Run") || t.starts_with("Ran") => Icon::new(IconName::SquareTerminal),
                    t if t.starts_with("Edit") || t.starts_with("Wr") => Icon::new(crate::assets::Lucide::FilePen),
                    t if t.starts_with("Read") => Icon::new(IconName::FileText),
                    t if t.contains("Search") || t.starts_with("List") => Icon::new(IconName::Search),
                    _ => Icon::new(crate::assets::Lucide::Wrench),
                };
                let status_el = match status {
                    ToolStatus::Running => Spinner::new().xsmall().into_any_element(),
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
                                .when(has_output, |el| el.cursor_pointer().hover(|s| s.bg(theme.list_hover)).on_click(toggle(ix)))
                                .child(icon.small().text_color(theme.muted_foreground))
                                .child(div().font_medium().flex_none().child(title))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
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

    fn live_footer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ws = self.workspace.read(cx);
        let id = self.current.clone()?;
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
            let elapsed = live.turn_started.map(|t| time::elapsed(t.elapsed())).unwrap_or_default();
            return Some(
                h_flex()
                    .w_full()
                    .justify_center()
                    .px_6()
                    .pb_2()
                    .child(
                        h_flex()
                            .w_full()
                            .max_w(px(COLUMN))
                            .gap_3()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(ShimmerText::new(format!("Working for {elapsed}")).id("working-shimmer").highlight_color(theme.foreground))
                            .child(div().flex_1().h(px(1.)).bg(theme.border)),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let footer = self.live_footer(cx);
        if rows.is_empty() {
            return v_flex().size_full().child(self.empty_state(cx)).children(footer);
        }
        let view = cx.entity().downgrade();
        let text_size = px(self.workspace.read(cx).settings.appearance.transcript_font_size());
        v_flex()
            .size_full()
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("transcript", self.scroller.clone(), move |ix, _, cx| match rows.get(ix).cloned() {
                        Some(row) => ThreadView::render_row(row, view.clone(), text_size, cx),
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
