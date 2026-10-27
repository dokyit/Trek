//! Browser use: agents drive the Browser tool's own page, which the user watches. Their
//! `trek-mcp browser` tools reach Trek over its local channel; the workspace hands each call to
//! the main window's Browser (`RightPanel::browser_call`), which runs them here one at a time.
//!
//! Each call runs the page side (`agent.js`) through the view's script evaluation and waits for
//! what it set going (a load, a new tab). An element the agent acts on is outlined in the page
//! for a moment, and a strip over the page says an agent is at work, with Stop: the user takes
//! the browser back, and that turn's calls fail saying so (its next turn may use it again).
//! What needs the user's approval (`agent_core::ask_for`) goes to the thread's approval card
//! first (`Workspace::browser_ask`).

use super::BrowserPanel;
use super::agent_core::{self as core, Wait};
use crate::assets::Lucide;
use crate::ipc::Reply;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};
use trek_agents::Decision;
use trek_core::HandHolding;

/// What a call gets once the user has taken the browser back.
pub const TOOK_OVER: &str = "The user took over the browser: stop using it and wait for them. (They can hand it back, or carry on with you in their next message.)";

/// How long the strip stays after an agent's last call.
const STRIP_LINGER: Duration = Duration::from_secs(12);
/// How long a page gets to load after `browser_open` (and after an action that starts a load).
const LOAD_WAIT: Duration = Duration::from_secs(20);
/// How long a script in the page gets to answer.
const EVAL_WAIT: Duration = Duration::from_secs(10);
/// How long the user has to answer an approval.
const ASK_WAIT: Duration = Duration::from_secs(15 * 60);
/// The longest side of a screenshot of the view, and the size a full page is fitted into.
const SHOT_SIDE: u32 = 1568;
const FULL_WIDTH: u32 = 1280;
const FULL_HEIGHT: u32 = 8000;
/// How many screens a full-page screenshot takes at most.
const FULL_SCREENS: u32 = 12;

/// One tool call from an agent's session, for the Browser to run. It always gets an answer: one
/// dropped unanswered (no main window, the Browser closed mid-call) says so.
pub struct AgentCall {
    pub thread: String,
    /// The thread's title, for the strip.
    pub title: String,
    pub method: String,
    pub params: Value,
    /// The thread's access level: what needs asking.
    pub access: HandHolding,
    /// When the thread's current turn started: the user takes the browser back from a turn.
    pub turn: Option<Instant>,
    /// The Browser may come forward to show it (the setting is on and the thread is in view).
    pub follow: bool,
    reply: Option<async_channel::Sender<Reply>>,
}

/// A call on its way to the main window's Browser, which takes it out (`WorkspaceEvent`s go to
/// every window).
pub type AgentCallSlot = Rc<RefCell<Option<AgentCall>>>;

impl AgentCall {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        thread: String,
        title: String,
        method: String,
        params: Value,
        access: HandHolding,
        turn: Option<Instant>,
        follow: bool,
        reply: async_channel::Sender<Reply>,
    ) -> Self {
        Self {
            thread,
            title,
            method,
            params,
            access,
            turn,
            follow,
            reply: Some(reply),
        }
    }

    pub fn answer(mut self, result: Result<Value, String>) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.try_send(Reply::Done(result));
        }
    }
}

impl Drop for AgentCall {
    fn drop(&mut self) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.try_send(Reply::Done(Err(
                "Trek's Browser didn't get to this call (its window or the Browser tool closed). Try again.".into(),
            )));
        }
    }
}

/// What the strip shows: who's at it and what they did last.
struct Activity {
    title: String,
    action: String,
    at: Instant,
}

/// The Browser's side of browser use.
#[derive(Default)]
pub(super) struct Driver {
    queue: VecDeque<AgentCall>,
    running: bool,
    last: Option<Activity>,
    /// Threads the user took the browser back from, with the turn each was in then.
    taken: HashMap<String, Option<Instant>>,
    /// Each thread's latest turn that used the browser (Stop takes it back from them).
    turns: HashMap<String, Option<Instant>>,
    /// Bumped by Stop: a call under way checks it between steps.
    stops: u64,
    /// What each thread's user allowed for the rest of the run ("Allow for this session").
    allowed: HashSet<(String, core::Sensitive)>,
    shots: u64,
    evals: u64,
    linger: Option<Task<()>>,
    /// What had the keys before the Browser was made for an agent: a new Browser puts them in
    /// its address bar, and the user may be typing elsewhere.
    refocus: Option<FocusHandle>,
}

impl Driver {
    /// Why `call` can't run now: the user has the browser, for the turn it's from.
    fn refused(&mut self, call: &AgentCall) -> Option<String> {
        match self.taken.get(&call.thread) {
            Some(turn) if *turn == call.turn => Some(TOOK_OVER.into()),
            // A new turn: the user carried on with the agent, which may use it again.
            Some(_) => {
                self.taken.remove(&call.thread);
                None
            }
            None => None,
        }
    }

    pub(super) fn taken_over(&self) -> bool {
        !self.taken.is_empty()
    }
}

impl BrowserPanel {
    /// The Browser was just made for an agent, taking the keys from `focus`: they go back once
    /// its first tab is up.
    pub fn agent_refocus(&mut self, focus: Option<FocusHandle>) {
        self.agent.refocus = focus;
    }

    /// An agent's tool call: run after those before it.
    pub fn agent_call(&mut self, call: AgentCall, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(why) = self.agent.refused(&call) {
            call.answer(Err(why));
            return;
        }
        self.agent.turns.insert(call.thread.clone(), call.turn);
        self.agent.last = Some(Activity {
            title: call.title.clone(),
            action: starting(&call.method).into(),
            at: Instant::now(),
        });
        self.agent.queue.push_back(call);
        cx.notify();
        if self.agent.running {
            return;
        }
        self.agent.running = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let next = this.update(cx, |p, _| {
                    let next = p.agent.queue.pop_front();
                    if next.is_none() {
                        p.agent.running = false;
                    }
                    next.map(|c| (c, p.agent.stops))
                });
                let Ok(Some((call, stops))) = next else { break };
                let result = Run {
                    this: this.clone(),
                    cx,
                    call: &call,
                    stops,
                }
                .go()
                .await;
                let done = match &result {
                    Ok(v) => v["text"]
                        .as_str()
                        .map(first_line)
                        .unwrap_or("Done")
                        .to_string(),
                    Err(e) => first_line(e).to_string(),
                };
                call.answer(result);
                if this.update(cx, |p, cx| p.agent_done(done, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// A call ended with `what`: the strip says so, and goes a while after the last.
    fn agent_done(&mut self, what: String, cx: &mut Context<Self>) {
        if let Some(a) = self.agent.last.as_mut() {
            a.action = what;
            a.at = Instant::now();
        }
        self.agent.linger = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STRIP_LINGER).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        cx.notify();
    }

    /// The user takes the browser back: the call under way stops at its next step, those
    /// waiting fail, and the turns that used it get no more.
    fn agent_stop(&mut self, cx: &mut Context<Self>) {
        self.agent.stops += 1;
        let turns: Vec<_> = self.agent.turns.drain().collect();
        self.agent.taken.extend(turns);
        for call in self.agent.queue.drain(..) {
            call.answer(Err(TOOK_OVER.into()));
        }
        self.agent.last = None;
        cx.notify();
    }

    /// The user hands the browser back to the agents they took it from.
    fn agent_hand_back(&mut self, cx: &mut Context<Self>) {
        self.agent.taken.clear();
        cx.notify();
    }

    /// The strip over the page while an agent uses the browser (or the user took it back).
    pub(super) fn agent_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let row = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(32.))
                .px(px(12.))
                .gap(px(8.))
                .text_xs()
                .border_b_1()
                .border_color(theme.border)
        };
        let busy = self.agent.running || !self.agent.queue.is_empty();
        if let Some(a) = self
            .agent
            .last
            .as_ref()
            .filter(|a| busy || a.at.elapsed() < STRIP_LINGER)
        {
            return Some(
                row("browser-agent-strip")
                    .bg(ember.opacity(0.1))
                    .child(Icon::new(Lucide::Sparkles).xsmall().text_color(ember))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(if busy {
                                "Agent is using the browser"
                            } else {
                                "Agent used the browser"
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} · {}", a.title, a.action)),
                    )
                    .child(
                        Button::new("browser-agent-stop")
                            .xsmall()
                            .outline()
                            .icon(Lucide::Hand)
                            .label("Stop")
                            .tooltip("Take the browser back: the agent's browser tools stop for this turn")
                            .on_click(cx.listener(|this, _, _, cx| this.agent_stop(cx))),
                    )
                    .into_any_element(),
            );
        }
        self.agent.taken_over().then(|| {
            row("browser-agent-taken")
                .bg(theme.foreground.opacity(0.04))
                .child(
                    Icon::new(Lucide::Hand)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(theme.muted_foreground)
                        .child(
                            "You have the browser. The agent can use it again in its next turn.",
                        ),
                )
                .child(
                    Button::new("browser-agent-hand-back")
                        .xsmall()
                        .ghost()
                        .label("Hand back")
                        .on_click(cx.listener(|this, _, _, cx| this.agent_hand_back(cx))),
                )
                .into_any_element()
        })
    }

    /// The tab in front's page, as an agent reads it.
    fn agent_page(&self) -> Option<PageNow> {
        let tab = self.active_tab()?;
        Some(PageNow {
            tab: tab.id,
            url: tab
                .page
                .failed
                .clone()
                .unwrap_or_else(|| tab.page.url.clone()),
            title: tab.page.label(),
            loading: tab.page.loading,
            failed: tab.page.failed.is_some(),
            ready: tab.page.ready,
        })
    }
}

/// "Clicking…" for the strip while a call of `method` runs.
fn starting(method: &str) -> &'static str {
    match method {
        "browser_open" => "Opening a page…",
        "browser_snapshot" | "browser_text" => "Reading the page…",
        "browser_click" => "Clicking…",
        "browser_type" => "Typing…",
        "browser_press" => "Pressing a key…",
        "browser_select" => "Choosing an option…",
        "browser_hover" => "Pointing…",
        "browser_scroll" => "Scrolling…",
        "browser_wait" => "Waiting on the page…",
        "browser_screenshot" => "Taking a screenshot…",
        "browser_eval" => "Running a script…",
        "browser_back" | "browser_forward" | "browser_reload" => "Moving through history…",
        _ => "Working with tabs…",
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

/// The tab in front, as last reported.
#[derive(Debug, Clone, PartialEq)]
struct PageNow {
    tab: u64,
    url: String,
    title: String,
    loading: bool,
    failed: bool,
    ready: bool,
}

/// A call being run.
struct Run<'a> {
    this: WeakEntity<BrowserPanel>,
    cx: &'a mut AsyncWindowContext,
    call: &'a AgentCall,
    /// `Driver::stops` when it started: Stop since means the user took over.
    stops: u64,
}

fn gone<E>(_: E) -> String {
    "Trek's Browser closed.".to_string()
}

const NO_TAB: &str = "The Browser has no tab open.";

impl Run<'_> {
    fn text(text: impl Into<String>) -> Result<Value, String> {
        Ok(json!({ "text": text.into() }))
    }

    async fn go(mut self) -> Result<Value, String> {
        self.ready().await?;
        let p = self.call.params.clone();
        let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        let r = s("ref");
        match self.call.method.as_str() {
            "browser_open" => {
                self.open(&s("url"), p["new_tab"].as_bool() == Some(true))
                    .await
            }
            "browser_tabs" => self.tabs(),
            "browser_switch" => self.switch(p["tab"].as_u64()).await,
            "browser_close" => self.close(p["tab"].as_u64()).await,
            "browser_back" | "browser_forward" | "browser_reload" => {
                let method = self.call.method.clone();
                self.history(&method).await
            }
            "browser_snapshot" => {
                let snap = self
                    .page_js(
                        "snapshot",
                        &[json!({ "interactive_only": p["interactive_only"].as_bool() == Some(true) })],
                    )
                    .await?;
                Self::text(core::outline(&snap))
            }
            "browser_text" => {
                let t = self.page_js("text", &[]).await?;
                Self::text(format!(
                    "Page: {}\nURL: {}\n\n{}",
                    t["title"].as_str().unwrap_or_default(),
                    t["url"].as_str().unwrap_or_default(),
                    t["text"].as_str().unwrap_or_default()
                ))
            }
            "browser_click" => self.click(&r).await,
            "browser_type" => {
                self.type_text(
                    &r,
                    &s("text"),
                    p["submit"].as_bool() == Some(true),
                    p["clear"].as_bool() != Some(false),
                )
                .await
            }
            "browser_press" => self.press(&s("key"), p["ref"].as_str()).await,
            "browser_select" => self.select(&r, &s("option")).await,
            "browser_hover" => {
                let v = self.page_js("hover", &[json!(r)]).await?;
                Self::text(did(&v))
            }
            "browser_scroll" => {
                let v = self
                    .page_js(
                        "scroll",
                        &[
                            json!(p["direction"].as_str()),
                            json!(p["amount"].as_f64()),
                            json!(p["ref"].as_str()),
                        ],
                    )
                    .await?;
                Self::text(format!(
                    "{}. Now {} px down, {} px more below.",
                    did(&v),
                    v["y"].as_i64().unwrap_or(0),
                    (v["page_height"].as_i64().unwrap_or(0)
                        - v["height"].as_i64().unwrap_or(0)
                        - v["y"].as_i64().unwrap_or(0))
                    .max(0)
                ))
            }
            "browser_wait" => self.wait().await,
            "browser_screenshot" => {
                self.screenshot(p["full_page"].as_bool() == Some(true))
                    .await
            }
            "browser_eval" => self.eval(&s("js")).await,
            other => Err(format!("Trek's Browser has no tool called {other}.")),
        }
    }

    /// Stop if the user took over (or the Browser went).
    fn check(&mut self) -> Result<(), String> {
        let stops = self
            .this
            .update(self.cx, |p, _| p.agent.stops)
            .map_err(gone)?;
        if stops != self.stops {
            Err(TOOK_OVER.into())
        } else {
            Ok(())
        }
    }

    async fn sleep(&mut self, d: Duration) {
        self.cx.background_executor().timer(d).await;
    }

    /// Wait for what `rx` brings, for `within` at most.
    async fn recv<T: Send + 'static>(
        &mut self,
        rx: async_channel::Receiver<T>,
        within: Duration,
    ) -> Option<T> {
        let (tx, out) = async_channel::bounded::<Option<T>>(1);
        let timer = self.cx.background_executor().timer(within);
        let t2 = tx.clone();
        self.cx
            .background_executor()
            .spawn(async move {
                if let Ok(v) = rx.recv().await {
                    let _ = t2.try_send(Some(v));
                }
            })
            .detach();
        self.cx
            .background_executor()
            .spawn(async move {
                timer.await;
                let _ = tx.try_send(None);
            })
            .detach();
        out.recv().await.ok().flatten()
    }

    fn page(&mut self) -> Result<PageNow, String> {
        self.this
            .update(self.cx, |p, _| p.agent_page())
            .map_err(gone)?
            .ok_or_else(|| NO_TAB.to_string())
    }

    /// A tab to work in: the Browser's first one is made a moment after the tool opens.
    async fn ready(&mut self) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.check()?;
            let (tabs, unavailable) = self
                .this
                .update(self.cx, |p, _| (p.tabs.len(), p.unavailable))
                .map_err(gone)?;
            if tabs > 0 {
                let _ = self.this.update_in(self.cx, |p, window, cx| {
                    if let Some(focus) = p.agent.refocus.take() {
                        p.editing = false;
                        focus.focus(window, cx);
                        cx.notify();
                    }
                });
                return Ok(());
            }
            if unavailable {
                return Err(crate::words::words().browser_unavailable.to_string());
            }
            if started.elapsed() > Duration::from_secs(10) {
                return Err("The Browser didn't open a tab.".into());
            }
            self.sleep(Duration::from_millis(100)).await;
        }
    }

    /// Run `js` in the tab in front and read its answer.
    async fn run_js(&mut self, js: String) -> Result<Value, String> {
        self.check()?;
        let (tx, rx) = async_channel::bounded::<String>(1);
        self.this
            .update(self.cx, |p, cx| -> Result<(), String> {
                let tab = p.active_tab().ok_or(NO_TAB)?;
                tab.view
                    .read(cx)
                    .raw()
                    .evaluate_script_with_callback(&js, move |s| {
                        let _ = tx.try_send(s);
                    })
                    .map_err(|e| format!("The page can't run scripts: {e}"))
            })
            .map_err(gone)??;
        match self.recv(rx, EVAL_WAIT).await {
            Some(raw) => core::answer(&raw),
            None => Err("The page didn't answer in time: it may be busy, or still loading.".into()),
        }
    }

    /// One of the page side's commands.
    async fn page_js(&mut self, command: &str, args: &[Value]) -> Result<Value, String> {
        // A page that's loading may not run it: give the load a moment first.
        self.settle(Duration::from_secs(10), false).await?;
        let first = self.run_js(core::script(command, args)).await;
        match first {
            // Mid-navigation the script can land in the page that's going: once more, in the new one.
            Err(e) if e.contains("didn't run") => {
                self.sleep(Duration::from_millis(400)).await;
                self.settle(Duration::from_secs(10), false).await?;
                self.run_js(core::script(command, args)).await
            }
            other => other,
        }
    }

    /// Let a load under way finish (within `max`). `kick`: an action just ran, which may have
    /// started one a moment later.
    async fn settle(&mut self, max: Duration, kick: bool) -> Result<(), String> {
        let started = Instant::now();
        if kick {
            self.sleep(Duration::from_millis(250)).await;
        }
        loop {
            self.check()?;
            let page = self.page()?;
            if !page.loading && page.ready || started.elapsed() > max {
                return Ok(());
            }
            self.sleep(Duration::from_millis(100)).await;
        }
    }

    fn tab_ids(&mut self) -> Result<Vec<u64>, String> {
        self.this
            .update(self.cx, |p, _| p.tabs.iter().map(|t| t.id).collect())
            .map_err(gone)
    }

    /// What came of an action: where the page went, a tab it opened.
    async fn aftermath(&mut self, before: &PageNow, tabs: &[u64]) -> Result<String, String> {
        self.settle(LOAD_WAIT, true).await?;
        // A link to a new window opens its tab a moment later.
        let mut opened = None;
        for _ in 0..4 {
            opened = self.tab_ids()?.into_iter().find(|id| !tabs.contains(id));
            if opened.is_some() {
                break;
            }
            self.sleep(Duration::from_millis(150)).await;
        }
        if opened.is_some() {
            self.settle(LOAD_WAIT, false).await?;
        }
        let now = self.page()?;
        Ok(if let Some(id) = opened {
            format!(
                " It opened tab {id}, now in front: {} ({}).",
                now.url, now.title
            )
        } else if now.failed {
            format!(" The page couldn't load {}.", now.url)
        } else if now.tab == before.tab && now.url != before.url {
            format!(" The page went to {} ({}).", now.url, now.title)
        } else {
            String::new()
        })
    }

    async fn open(&mut self, url: &str, new_tab: bool) -> Result<Value, String> {
        let target = core::open_target(url)?;
        let id = self
            .this
            .update_in(self.cx, |p, window, cx| {
                // A new tab nothing was opened in yet takes the page rather than a tab beside it.
                let blank = p
                    .active_tab()
                    .is_some_and(|t| t.page.is_start() && !t.page.can_back);
                let new = new_tab && !blank || p.tabs.is_empty();
                if new {
                    let id = p.next_id;
                    p.new_tab(&target, window, cx);
                    Some(id)
                } else {
                    p.editing = false;
                    p.navigate(target.clone(), cx);
                    None
                }
            })
            .map_err(gone)?;
        if let Some(id) = id {
            let started = Instant::now();
            while !self.tab_ids()?.contains(&id) {
                if started.elapsed() > Duration::from_secs(10) {
                    return Err("The Browser couldn't open a new tab.".into());
                }
                self.sleep(Duration::from_millis(100)).await;
            }
        }
        self.settle(LOAD_WAIT, true).await?;
        let now = self.page()?;
        if now.failed {
            return Err(format!(
                "Couldn't load {}: the site can't be reached (Trek's error page is showing).",
                now.url
            ));
        }
        Self::text(format!(
            "Opened {} in tab {}{}: {}. Take a browser_snapshot to read it.",
            now.url,
            now.tab,
            if now.loading { " (still loading)" } else { "" },
            now.title
        ))
    }

    fn tabs(&mut self) -> Result<Value, String> {
        let (tabs, active) = self
            .this
            .update(self.cx, |p, _| {
                (
                    p.tabs
                        .iter()
                        .map(|t| (t.id, t.page.label(), t.page.address(), t.page.loading))
                        .collect::<Vec<_>>(),
                    p.active,
                )
            })
            .map_err(gone)?;
        Self::text(core::tabs_text(&tabs, active))
    }

    async fn switch(&mut self, tab: Option<u64>) -> Result<Value, String> {
        let tab = tab.ok_or("Which tab? Give its id from browser_tabs.")?;
        let ok = self
            .this
            .update(self.cx, |p, cx| {
                let there = p.tabs.iter().any(|t| t.id == tab);
                if there {
                    p.activate(tab, cx);
                }
                there
            })
            .map_err(gone)?;
        if !ok {
            return Err(format!("There's no tab {tab}. browser_tabs lists them."));
        }
        let now = self.page()?;
        Self::text(format!(
            "Tab {tab} is in front: {} ({}).",
            now.url, now.title
        ))
    }

    async fn close(&mut self, tab: Option<u64>) -> Result<Value, String> {
        let closed = self
            .this
            .update_in(self.cx, |p, window, cx| {
                let id = tab.unwrap_or(p.active);
                let there = p.tabs.iter().any(|t| t.id == id);
                if there {
                    p.close_tab(id, window, cx);
                }
                there.then_some(id)
            })
            .map_err(gone)?;
        let Some(id) = closed else {
            return Err(format!(
                "There's no tab {}. browser_tabs lists them.",
                tab.unwrap_or_default()
            ));
        };
        // The last tab closing leaves a new one in its place, made a moment later.
        self.ready().await?;
        let now = self.page()?;
        Self::text(format!(
            "Closed tab {id}. Tab {} is in front: {}.",
            now.tab,
            if now.url.starts_with("trek://") {
                "a new tab".to_string()
            } else {
                now.url
            }
        ))
    }

    async fn history(&mut self, method: &str) -> Result<Value, String> {
        let before = self.page()?;
        self.this
            .update(self.cx, |p, cx| {
                p.with_active(cx, |wv| match method {
                    "browser_back" => super::native::go_back(wv),
                    "browser_forward" => super::native::go_forward(wv),
                    _ => {
                        let _ = wv.reload();
                    }
                })
            })
            .map_err(gone)?;
        self.settle(LOAD_WAIT, true).await?;
        // A history move without a load (a fragment, an app's own routing) shows in the URL.
        self.sleep(Duration::from_millis(150)).await;
        let now = self.page()?;
        let what = match method {
            "browser_back" => "Went back",
            "browser_forward" => "Went forward",
            _ => "Reloaded",
        };
        if method != "browser_reload" && now.url == before.url {
            return Self::text(format!(
                "{what}, but the page stayed at {} (no history that way?).",
                now.url
            ));
        }
        Self::text(format!("{what}: {} ({}).", now.url, now.title))
    }

    /// Ask the user about what `tool` would do, when the thread's access level says to.
    async fn approve(
        &mut self,
        tool: &str,
        args: &Value,
        probe: Option<&Value>,
    ) -> Result<(), String> {
        let Some(ask) = core::ask_for(tool, args, probe) else {
            return Ok(());
        };
        if !core::needs_asking(&ask.kind, self.call.access) {
            return Ok(());
        }
        let thread = self.call.thread.clone();
        let (allowed, workspace) = self
            .this
            .update(self.cx, |p, cx| {
                if let Some(a) = p.agent.last.as_mut() {
                    a.action = "Waiting for your approval…".into();
                    a.at = Instant::now();
                }
                cx.notify();
                (
                    p.agent
                        .allowed
                        .contains(&(thread.clone(), ask.kind.clone())),
                    p.workspace.clone(),
                )
            })
            .map_err(gone)?;
        if allowed {
            return Ok(());
        }
        let (request, rx) = workspace.update(self.cx, |ws, cx| {
            ws.browser_ask(&thread, ask.title.clone(), ask.detail.clone(), cx)
        });
        let started = Instant::now();
        let decision = loop {
            match rx.try_recv() {
                Ok(d) => break d,
                Err(async_channel::TryRecvError::Closed) => break Decision::Deny,
                Err(async_channel::TryRecvError::Empty) => {}
            }
            // The turn ended (stopped, say), taking the card with it; or the user took over.
            let pending =
                workspace.update(self.cx, |ws, _| ws.browser_ask_pending(&thread, &request));
            let took_over = self.check().is_err();
            if !pending || took_over || started.elapsed() > ASK_WAIT {
                workspace.update(self.cx, |ws, cx| ws.browser_withdraw(&thread, &request, cx));
                if took_over {
                    return Err(TOOK_OVER.into());
                }
                break Decision::Deny;
            }
            self.sleep(Duration::from_millis(200)).await;
        };
        match decision {
            Decision::Allow => Ok(()),
            Decision::AllowForSession => {
                let kind = ask.kind.clone();
                let _ = self
                    .this
                    .update(self.cx, |p, _| p.agent.allowed.insert((thread, kind)));
                Ok(())
            }
            Decision::Deny => Err(format!(
                "The user didn't allow it ({}). Don't try another way round; ask them if it's needed.",
                ask.title
            )),
        }
    }

    async fn click(&mut self, r: &str) -> Result<Value, String> {
        let probe = self.page_js("probe", &[json!(r), Value::Null]).await?;
        self.approve("browser_click", &self.call.params.clone(), Some(&probe))
            .await?;
        let (before, tabs) = (self.page()?, self.tab_ids()?);
        let v = self.page_js("click", &[json!(r)]).await?;
        let after = self.aftermath(&before, &tabs).await?;
        let note = v["note"]
            .as_str()
            .map(|n| format!(" {n}"))
            .unwrap_or_default();
        Self::text(format!("{}.{note}{after}", did(&v)))
    }

    async fn type_text(
        &mut self,
        r: &str,
        text: &str,
        submit: bool,
        clear: bool,
    ) -> Result<Value, String> {
        let probe = self
            .page_js(
                "probe",
                &[json!(r), json!(if submit { "Enter" } else { "" })],
            )
            .await?;
        self.approve("browser_type", &self.call.params.clone(), Some(&probe))
            .await?;
        let v = self
            .page_js("type", &[json!(r), json!(text), json!(clear)])
            .await?;
        let mut out = format!(
            "{}; it now reads {}.",
            did(&v),
            Value::from(v["value"].as_str().unwrap_or_default())
        );
        if submit {
            let (before, tabs) = (self.page()?, self.tab_ids()?);
            let key = core::parse_key("Enter")?;
            let pressed = self.page_js("press", &[key, json!(r)]).await?;
            out.push_str(&format!(" {}.", did(&pressed)));
            out.push_str(&self.aftermath(&before, &tabs).await?);
        }
        Self::text(out)
    }

    async fn press(&mut self, key: &str, r: Option<&str>) -> Result<Value, String> {
        let key = core::parse_key(key)?;
        if key["key"] == "Enter" {
            let probe = self.page_js("probe", &[json!(r), json!("Enter")]).await?;
            self.approve("browser_press", &self.call.params.clone(), Some(&probe))
                .await?;
        }
        let (before, tabs) = (self.page()?, self.tab_ids()?);
        let v = self.page_js("press", &[key, json!(r)]).await?;
        let after = self.aftermath(&before, &tabs).await?;
        Self::text(format!("{}.{after}", did(&v)))
    }

    async fn select(&mut self, r: &str, option: &str) -> Result<Value, String> {
        if option.is_empty() {
            return Err("Choose which option? Give its label or value.".into());
        }
        let v = self.page_js("select", &[json!(r), json!(option)]).await?;
        if v["custom"].as_bool() != Some(true) {
            return Self::text(format!("{}.", did(&v)));
        }
        // A list of the page's own: open it, then click the option once it's there.
        let what = v["describe"].as_str().unwrap_or("it").to_string();
        let (before, tabs) = (self.page()?, self.tab_ids()?);
        self.page_js("click", &[json!(r)]).await?;
        let started = Instant::now();
        let option_ref = loop {
            let found = self.page_js("option", &[json!(option)]).await?;
            if let Some(o) = found["ref"].as_str() {
                break o.to_string();
            }
            if started.elapsed() > Duration::from_secs(3) {
                return Err(format!(
                    "Opened {what}, but no option {option:?} showed. Take a browser_snapshot to see what it offers."
                ));
            }
            self.sleep(Duration::from_millis(150)).await;
        };
        let clicked = self.page_js("click", &[json!(option_ref)]).await?;
        let after = self.aftermath(&before, &tabs).await?;
        Self::text(format!(
            "Opened {what} and {}.{after}",
            lower_first(&did(&clicked))
        ))
    }

    async fn wait(&mut self) -> Result<Value, String> {
        let (wait, within) = core::parse_wait(&self.call.params)?;
        let started = Instant::now();
        if let Wait::For(d) = wait {
            while started.elapsed() < d {
                self.check()?;
                let left = d.saturating_sub(started.elapsed());
                self.sleep(left.min(Duration::from_millis(250))).await;
            }
            return Self::text(format!("Waited {} ms.", d.as_millis()));
        }
        let (args, what, gone) = match &wait {
            Wait::Text { text, gone } => (
                [json!(text), Value::Null, json!(gone)],
                format!("the text {text:?}"),
                *gone,
            ),
            Wait::Selector { selector, gone } => (
                [Value::Null, json!(selector), json!(gone)],
                format!("an element matching {selector:?}"),
                *gone,
            ),
            Wait::For(_) => unreachable!(),
        };
        loop {
            let v = self.page_js("check", &args).await?;
            if v["met"].as_bool() == Some(true) {
                return Self::text(format!(
                    "{} after {:.1} s.",
                    if gone {
                        format!("Gone: {what}")
                    } else {
                        format!("Found {what}")
                    },
                    started.elapsed().as_secs_f32()
                ));
            }
            if started.elapsed() > within {
                return Err(format!(
                    "Waited {:.0} s, and {what} {}.",
                    within.as_secs_f32(),
                    if gone {
                        "is still there"
                    } else {
                        "never showed"
                    }
                ));
            }
            self.sleep(Duration::from_millis(250)).await;
        }
    }

    async fn eval(&mut self, js: &str) -> Result<Value, String> {
        if js.trim().is_empty() {
            return Err("Run what? Give the script in js.".into());
        }
        let page = self.page()?.url;
        self.approve(
            "browser_eval",
            &self.call.params.clone(),
            Some(&json!({ "page": page })),
        )
        .await?;
        let id = self
            .this
            .update(self.cx, |p, _| {
                p.agent.evals += 1;
                format!("x{}", p.agent.evals)
            })
            .map_err(gone)?;
        self.settle(Duration::from_secs(10), false).await?;
        // An expression, or statements that return: whichever parses.
        let started = match self.run_js(core::eval_script(&id, js, true)).await {
            Err(e) if e.contains("didn't run") => {
                self.run_js(core::eval_script(&id, js, false)).await
            }
            other => other,
        };
        if let Err(e) = started {
            return Err(if e.contains("didn't run") {
                "The script didn't run: it may not parse (write an expression, or statements that return a value).".into()
            } else {
                e
            });
        }
        let begun = Instant::now();
        loop {
            let v = self.run_js(core::script("take", &[json!(id)])).await?;
            if let Some(value) = v["value"].as_str() {
                let value = if value.len() > 100_000 {
                    format!("{}… (cut at 100,000 characters)", clip_at(value, 100_000))
                } else {
                    value.to_string()
                };
                return Self::text(value);
            }
            if let Some(e) = v["error"].as_str() {
                return Err(format!("The script threw: {e}"));
            }
            if v["waiting"].as_bool() != Some(true) {
                return Err("The page went away before the script finished.".into());
            }
            if begun.elapsed() > Duration::from_secs(30) {
                return Err("The script's promise didn't settle within 30 s.".into());
            }
            self.sleep(Duration::from_millis(100)).await;
        }
    }

    /// A picture of the page in front, as decoded pixels.
    async fn picture(&mut self) -> Result<image::RgbaImage, String> {
        self.check()?;
        let rx = self
            .this
            .update(self.cx, |p, cx| -> Result<_, String> {
                let tab = p.active_tab().ok_or(NO_TAB)?;
                let view = tab.view.read(cx);
                if !p.visible || !view.visible() {
                    return Err("The Browser isn't on screen, and a page draws nothing there: bring it forward (⌘J › Browser, or turn on Settings › Tools › Show the browser live) to take a screenshot. browser_snapshot and browser_text work either way.".into());
                }
                Ok(page_picture(view.raw()))
            })
            .map_err(gone)??;
        let bytes = self
            .recv(rx, Duration::from_secs(10))
            .await
            .ok_or("The page didn't draw for the screenshot.")??;
        self.cx
            .background_executor()
            .spawn(async move {
                image::load_from_memory(&bytes)
                    .map(|i| i.to_rgba8())
                    .map_err(|e| format!("The screenshot couldn't be read: {e}"))
            })
            .await
    }

    async fn screenshot(&mut self, full: bool) -> Result<Value, String> {
        self.settle(LOAD_WAIT, false).await?;
        let state = self.page_js("state", &[]).await?;
        let (view_w, view_h) = (
            state["width"].as_f64().unwrap_or(1.).max(1.),
            state["height"].as_f64().unwrap_or(1.).max(1.),
        );
        let page_h = state["page_height"].as_f64().unwrap_or(view_h);
        let first = self.picture().await?;
        let scale = first.width() as f64 / view_w;
        let mut covered = view_h;
        let image = if full && page_h > view_h + 1. {
            let start_y = state["y"].as_f64().unwrap_or(0.);
            let total = page_h.min(view_h * FULL_SCREENS as f64);
            let mut canvas = image::RgbaImage::new(first.width(), (total * scale).round() as u32);
            let mut y = 0.;
            while y < total {
                let at = self.page_js("scrollTo", &[json!(y)]).await?;
                self.sleep(Duration::from_millis(180)).await;
                let shot = self.picture().await?;
                let actual = at["y"].as_f64().unwrap_or(y);
                image::imageops::overlay(&mut canvas, &shot, 0, (actual * scale).round() as i64);
                y += view_h;
            }
            let _ = self.page_js("scrollTo", &[json!(start_y)]).await;
            covered = total;
            canvas
        } else {
            first
        };
        let n = self
            .this
            .update(self.cx, |p, _| {
                p.agent.shots += 1;
                p.agent.shots
            })
            .map_err(gone)?;
        let page = self.page()?;
        let dir = trek_core::paths::data_dir().join("browser-shots");
        let path = dir.join(format!("{}-{n}.png", std::process::id()));
        let out = path.clone();
        let (w, h) = self
            .cx
            .background_executor()
            .spawn(async move {
                let image = fit(image, full);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                image
                    .save_with_format(&out, image::ImageFormat::Png)
                    .map_err(|e| format!("The screenshot couldn't be saved: {e}"))?;
                Ok::<_, String>((image.width(), image.height()))
            })
            .await?;
        Ok(json!({
            "text": format!(
                "{} of {} ({}), {w}×{h} px{}.",
                if full { "The whole page" } else { "The view" },
                page.url,
                page.title,
                if full && covered < page_h { format!(" (the first {} screens of a longer page)", FULL_SCREENS) } else { String::new() }
            ),
            "image": path,
        }))
    }
}

/// A screenshot sized for a model: a view's longest side at most `SHOT_SIDE`, a full page at most
/// `FULL_WIDTH` wide and `FULL_HEIGHT` tall.
fn fit(image: image::RgbaImage, full: bool) -> image::RgbaImage {
    let (w, h) = (image.width() as f64, image.height() as f64);
    let k = if full {
        (FULL_WIDTH as f64 / w).min(FULL_HEIGHT as f64 / h)
    } else {
        SHOT_SIDE as f64 / w.max(h)
    };
    if k >= 1. {
        return image;
    }
    image::imageops::resize(
        &image,
        ((w * k).round() as u32).max(1),
        ((h * k).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

/// The page side's "did" line, or a stand-in.
fn did(v: &Value) -> String {
    v["did"].as_str().unwrap_or("Done").to_string()
}

/// `s` up to `n` bytes, cut on a character.
fn clip_at(s: &str, n: usize) -> &str {
    let mut end = n.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The page as the view draws it: TIFF from WebKit, PNG from WebView2.
#[cfg(target_os = "macos")]
fn page_picture(wv: &wry::WebView) -> async_channel::Receiver<Result<Vec<u8>, String>> {
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSError;
    use wry::WebViewExtMacOS as _;
    let (tx, rx) = async_channel::bounded(1);
    let done = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        // SAFETY: WebKit hands a valid image or nil.
        let tiff = unsafe { image.as_ref() }
            .and_then(|i| i.TIFFRepresentation())
            .map(|d| d.to_vec());
        let _ = tx.try_send(tiff.ok_or_else(|| "WebKit took no picture of the page".to_string()));
    });
    // SAFETY: called on the main thread with a live view; WebKit copies the block.
    unsafe {
        wv.webview()
            .takeSnapshotWithConfiguration_completionHandler(None, &done)
    };
    rx
}

#[cfg(windows)]
fn page_picture(wv: &wry::WebView) -> async_channel::Receiver<Result<Vec<u8>, String>> {
    super::capture_png(wv)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn page_picture(_: &wry::WebView) -> async_channel::Receiver<Result<Vec<u8>, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let _ = tx.try_send(Err("Screenshots of the page aren't available here.".into()));
    rx
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: that would bring GPUI's `test` attribute over the standard one.
    use super::{AgentCall, Driver, HandHolding, Reply, TOOK_OVER, fit, lower_first, starting};
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn call(thread: &str, turn: Option<Instant>) -> (AgentCall, async_channel::Receiver<Reply>) {
        let (tx, rx) = async_channel::unbounded();
        (
            AgentCall::new(
                thread.into(),
                "T".into(),
                "browser_tabs".into(),
                json!({}),
                HandHolding::Auto,
                turn,
                true,
                tx,
            ),
            rx,
        )
    }

    #[test]
    fn every_call_is_answered_even_when_dropped() {
        let (c, rx) = call("a", None);
        c.answer(Ok(json!({"text": "ok"})));
        assert_eq!(
            rx.try_recv().unwrap(),
            Reply::Done(Ok(json!({"text": "ok"})))
        );
        assert!(rx.try_recv().is_err(), "once");
        let (c, rx) = call("a", None);
        drop(c);
        match rx.try_recv().unwrap() {
            Reply::Done(Err(e)) => assert!(e.contains("didn't get to this call"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn taking_over_stops_the_turn_not_the_next() {
        let mut d = Driver::default();
        let turn = Some(Instant::now());
        d.taken.insert("a".into(), turn);
        let (same, _rx) = call("a", turn);
        assert_eq!(d.refused(&same).as_deref(), Some(TOOK_OVER));
        let (other, _rx) = call("b", turn);
        assert_eq!(
            d.refused(&other),
            None,
            "another thread's agent wasn't stopped"
        );
        let (next, _rx) = call("a", Some(Instant::now() + Duration::from_secs(1)));
        assert_eq!(d.refused(&next), None, "its next turn may use it");
        assert!(!d.taken_over());
    }

    #[test]
    fn screenshots_fit_a_model() {
        let big = image::RgbaImage::new(3200, 2000);
        let v = fit(big.clone(), false);
        assert_eq!((v.width(), v.height()), (1568, 980));
        let tall = image::RgbaImage::new(1600, 20_000);
        let f = fit(tall, true);
        assert_eq!((f.width(), f.height()), (640, 8000));
        let small = image::RgbaImage::new(800, 600);
        assert_eq!(fit(small, false).dimensions(), (800, 600));
    }

    #[test]
    fn the_strip_says_what_is_starting() {
        assert_eq!(starting("browser_click"), "Clicking…");
        assert_eq!(starting("browser_switch"), "Working with tabs…");
        assert_eq!(lower_first("Clicked option \"B\""), "clicked option \"B\"");
    }
}
