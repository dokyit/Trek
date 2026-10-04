//! Setup shared by the UI tests: a process-wide sandbox, a Trek window over an in-memory store, and
//! helpers to drive it and wait for the mock agent.
//!
//! Isolation: `trek_core::paths::isolate` points every write at a temp folder for this test process
//! (each test's thread gets a folder of its own in it) and keeps the Keychain out of reach; the
//! store is in memory; notifications are off. Tests run in parallel, each on its own thread with
//! its own GPUI app, so nothing else is shared.

use crate::root::TrekWindow;
use crate::workspace::{GlobalWorkspace, Route, Workspace};
use gpui_kit::component::ThemeRegistry;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, ForegroundExecutor, PlatformTextSystem, TestAppContext, TestDispatcher, Window, WindowBounds, WindowOptions,
    point, px, size,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use trek_core::settings::{NotifyMode, Settings, ThemeChoice};
use trek_core::store::{Item, Store};
use trek_core::{AgentId, HandHolding, RunState};

/// CoreText, so text lays out as in the app. `MacPlatform` must be made on the main thread and
/// tests run on worker threads, so it's made before `main`.
static TEXT: OnceLock<Arc<dyn PlatformTextSystem>> = OnceLock::new();

#[cfg(target_os = "macos")]
#[ctor::ctor(unsafe)]
fn load_text_system() {
    let platform = gpui_macos::MacPlatform::new(true);
    let _ = TEXT.set(gpui_kit::Platform::text_system(&platform));
    // Lives for the whole process; dropping it would tear down state the text system shares.
    std::mem::forget(platform);
}

/// Every test in this process, UI or not, is isolated before any of them runs: the unit tests
/// that share the binary with these never reach the user's data folder or Keychain either. Home
/// moves too, so anything that slips past Trek's own guards (an agent CLI, a login shell) finds
/// none of the user's history or config.
#[ctor::ctor(unsafe)]
fn isolate_process() {
    // Live tests (`TREK_LIVE_AGENT`) run a real agent, which needs the user's sign-in.
    if std::env::var_os("TREK_LIVE_AGENT").is_some() {
        data_dir();
        return;
    }
    let home = data_dir().join("home");
    let _ = std::fs::create_dir_all(&home);
    // SAFETY: before `main`, so before any other thread could be reading the environment.
    unsafe { std::env::set_var("HOME", &home) };
}

/// This process's throwaway data folder. Isolation is set up on first use; folders left by test
/// runs that have exited are cleared then.
pub fn data_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        const PREFIX: &str = "trek-ui-tests-";
        let tmp = std::env::temp_dir();
        for entry in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(PREFIX)).and_then(|p| p.parse::<i32>().ok()) else { continue };
            // SAFETY: signal 0 only checks whether the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let dir = tmp.join(format!("{PREFIX}{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        trek_core::paths::isolate(dir.clone());
        trek_agents::mock::set_pace(0.);
        dir
    })
}

/// Run `test` as `#[gpui_kit::test]` would, with real text layout. Real time may pass: the mock
/// agent runs on Trek's tokio runtime and its events wake the test from other threads.
pub fn run(test: impl AsyncFnOnce(&mut TestAppContext)) {
    // A data folder of the test's own (settings above all): the tests run side by side.
    static N: AtomicUsize = AtomicUsize::new(0);
    trek_core::paths::isolate_thread(data_dir().join("tests").join(N.fetch_add(1, Ordering::Relaxed).to_string()));
    let dispatcher = TestDispatcher::new(0);
    let exec = Arc::new(dispatcher.clone());
    let text = TEXT.get().cloned().unwrap_or_else(|| Arc::new(gpui_kit::NoopTextSystem::new()));
    let mut cx = TestAppContext::build_with_text_system(dispatcher.clone(), None, text);
    cx.executor().allow_parking();
    ForegroundExecutor::new(exec.clone()).block_test(test(&mut cx));
    drop(exec);
    cx.run_until_parked();
    cx.update(|cx| cx.quit());
    cx.run_until_parked();
    drop(cx);
    dispatcher.drain_tasks();
}

/// Settings every test starts from: onboarding done, the mock agent as default, Supervised (so
/// permission prompts appear), no auto titles, and no notifications of any kind.
pub fn settings() -> Settings {
    let mut s = Settings::default();
    s.onboarding.completed = true;
    s.general.default_agent = mock().key();
    s.general.hand_holding = HandHolding::Supervised;
    s.general.auto_title = false;
    s.notifications.mode = NotifyMode::Off;
    s.notifications.dock_badge = false;
    s.notifications.menu_bar_icon = false;
    s.updates.auto_check = false;
    s
}

pub fn mock() -> AgentId {
    AgentId::Direct(trek_core::catalog::MOCK_PROVIDER.into())
}

/// A fresh, empty folder to use as a project.
pub fn new_project(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = data_dir().join("projects").join(format!("{name}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).expect("project dir");
    dir
}

/// A Trek window on a draft in a fresh project, composer focused.
pub struct Trek {
    pub ws: Entity<Workspace>,
    pub root: Entity<TrekWindow>,
    pub window: AnyWindowHandle,
    pub project: PathBuf,
}

pub fn open(cx: &mut TestAppContext) -> Trek {
    open_with(cx, |_| {})
}

pub fn open_with(cx: &mut TestAppContext, tweak: impl FnOnce(&mut Settings)) -> Trek {
    let project = new_project("project");
    let mut settings = settings();
    settings.user_projects.push(project.display().to_string());
    tweak(&mut settings);
    let store = Store::in_memory().expect("store");
    store.ensure_project(&project).expect("project");
    let (ws, root, window) = launch(cx, store, settings);
    let trek = Trek { ws, root, window, project };
    // Navigating focuses the composer, as at launch.
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
    trek.render(cx);
    trek
}

/// Trek's main window over `store` and `settings`, on whatever route they lead to (onboarding on
/// a first run).
pub fn launch(cx: &mut TestAppContext, store: Store, settings: Settings) -> (Entity<Workspace>, Entity<TrekWindow>, AnyWindowHandle) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let _ = ThemeRegistry::global_mut(cx).load_themes_from_str(&crate::assets::theme_json());
        crate::apply_theme(ThemeChoice::Night, None, cx);
        cx.bind_keys(crate::key_bindings());
        // Animations run on the real clock: a toast sliding in under load would move away from
        // where a click was aimed between frames. With reduced motion they start finished.
        cx.set_reduce_motion(true);
    });
    let ws = cx.new(|cx| {
        let mut ws = Workspace::with(store, settings, cx);
        ws.mock_agent = true;
        ws
    });
    cx.update(|cx| cx.set_global(GlobalWorkspace(ws.clone())));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(1280.), px(820.)) })),
        ..Default::default()
    };
    let (window, root) = cx
        .update(|cx| gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| TrekWindow::new(ws.clone(), window, cx))))
        .expect("open window");
    (ws, root, window)
}

impl Trek {
    pub fn update<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>) -> R) -> R {
        let r = self.ws.update(cx, f);
        cx.run_until_parked();
        r
    }

    pub fn read<R>(&self, cx: &TestAppContext, f: impl FnOnce(&Workspace, &App) -> R) -> R {
        self.ws.read_with(cx, f)
    }

    pub fn window<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        cx.update_window(self.window, |_, window, cx| f(window, cx)).expect("window")
    }

    /// A full frame, as after a window refresh (cached views re-render too).
    pub fn render(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        self.window(cx, |window, cx| window.render_frame(cx));
    }

    /// Type into whatever has focus (the composer, after `open`).
    pub fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        self.window(cx, |window, cx| window.input(text, cx));
        cx.run_until_parked();
    }

    pub fn press(&self, cx: &mut TestAppContext, keys: &str) {
        self.window(cx, |window, cx| window.press(keys, cx));
        cx.run_until_parked();
    }

    /// Type into whatever has focus a key at a time, without the full refresh `type_text` does:
    /// the window draws after each key as the platform would, so cached views show only what
    /// the key actually redrew.
    pub fn type_live(&self, cx: &mut TestAppContext, text: &str) {
        for c in text.chars() {
            let mut key = gpui_kit::Keystroke::parse(&c.to_string()).expect("keystroke");
            key.key_char = Some(c.to_string());
            self.window(cx, |window, cx| window.dispatch_keystroke(key, cx));
            cx.run_until_parked();
        }
    }

    /// Press `keys` (e.g. "enter") the same way.
    pub fn press_live(&self, cx: &mut TestAppContext, keys: &str) {
        let key = gpui_kit::Keystroke::parse(keys).expect("keystroke");
        self.window(cx, |window, cx| window.dispatch_keystroke(key, cx));
        cx.run_until_parked();
    }

    pub fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
        let id = id.into();
        self.window(cx, |window, cx| window.click(id, cx));
        cx.run_until_parked();
    }

    /// Whether `id` is on screen in the last frame. No refresh: the frame is what the test platform
    /// drew when the window last went dirty, cached views included, as on screen.
    pub fn visible(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> bool {
        let id = id.into();
        cx.run_until_parked();
        self.window(cx, |window, _| window.try_find(id).is_some_and(|e| e.visible()))
    }

    /// Where `id` was drawn in the last frame, if it was.
    pub fn bounds(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        let id = id.into();
        cx.run_until_parked();
        self.window(cx, |window, _| window.try_find(id).filter(|e| e.visible()).map(|e| e.bounds()))
    }

    /// `visible`, in another of Trek's windows (a thread window).
    pub fn visible_in(&self, cx: &mut TestAppContext, window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>) -> bool {
        let id = id.into();
        cx.run_until_parked();
        cx.update_window(window, |_, window, _| window.try_find(id).is_some_and(|e| e.visible())).expect("window")
    }

    /// Open `id` in a window of its own; returns that window.
    pub fn open_thread_window(&self, cx: &mut TestAppContext, id: &str) -> AnyWindowHandle {
        let ws = self.ws.clone();
        cx.update(|cx| crate::thread_window::open(ws, id, cx));
        cx.run_until_parked();
        self.read(cx, |ws, _| ws.thread_windows.get(id).copied()).expect("a window of its own")
    }

    pub fn thread_view(&self, cx: &TestAppContext) -> Entity<crate::thread_view::ThreadView> {
        cx.read(|cx| self.root.read(cx).thread_view.clone())
    }

    /// The transcript's rows (see `ThreadView::describe`).
    pub fn rows(&self, cx: &TestAppContext) -> Vec<String> {
        cx.read(|cx| self.root.read(cx).thread_view.read(cx).describe(cx))
    }

    /// The transcript's markdown documents by transcript index, each with the source text it
    /// holds (once parsing in flight has landed). Built only for rows that were drawn.
    pub fn drawn_markdown(&self, cx: &mut TestAppContext) -> Vec<(usize, String)> {
        use gpui_kit::component::text::SelectionFormat;
        cx.run_until_parked();
        let docs = self.thread_view(cx).read_with(cx, |v, cx| v.markdown_documents(cx));
        docs.into_iter()
            .map(|(ix, doc)| {
                // Select-all copies the source in Source format.
                let text = doc.update(cx, |d, cx| {
                    d.set_selection_format(SelectionFormat::Source, cx);
                    d.select_all(cx);
                    let text = d.selected_text();
                    d.clear_selection(cx);
                    d.set_selection_format(SelectionFormat::Plain, cx);
                    text
                });
                (ix, text)
            })
            .collect()
    }

    /// A thread in the project with no session, on screen. Feed it with `Workspace::apply_events`.
    pub fn quiet_thread(&self, cx: &mut TestAppContext) -> String {
        self.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&self.project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        })
    }

    /// The working bar's text, `None` while it's hidden.
    pub fn working_bar(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| self.root.read(cx).working_bar.read(cx).label())
    }

    /// The working bar's live group (see `WorkingBar::live_group`), `None` when there's none.
    pub fn live_group(&self, cx: &TestAppContext) -> Option<Vec<String>> {
        cx.read(|cx| self.root.read(cx).working_bar.read(cx).live_group())
    }

    /// The summary of the group folding away in the working bar, if one is.
    pub fn folding(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| self.root.read(cx).working_bar.read(cx).folding())
    }

    pub fn composer_text(&self, cx: &TestAppContext) -> String {
        cx.read(|cx| self.root.read(cx).composer.read(cx).text(cx))
    }

    /// The pending request on `id` (an approval, question or plan).
    pub fn request(&self, cx: &TestAppContext, id: &str) -> String {
        self.read(cx, |ws, _| ws.live.get(id).and_then(|l| l.permissions.first()).map(|p| p.request_id.clone()).expect("a pending request"))
    }

    /// Index of the first transcript item matching `f`.
    pub fn item_ix(&self, cx: &TestAppContext, id: &str, f: impl Fn(&Item) -> bool) -> usize {
        self.items(cx, id).iter().position(f).expect("item")
    }

    /// The thread on screen.
    pub fn thread_id(&self, cx: &TestAppContext) -> String {
        self.read(cx, |ws, _| match &ws.route {
            Route::Thread(id) => id.clone(),
            other => panic!("not on a thread: {other:?}"),
        })
    }

    pub fn items(&self, cx: &TestAppContext, id: &str) -> Vec<Item> {
        self.read(cx, |ws, _| ws.live.get(id).map(|l| l.items.to_vec()).unwrap_or_default())
    }

    pub fn run_state(&self, cx: &TestAppContext, id: &str) -> RunState {
        self.read(cx, |ws, _| ws.thread(id).map(|t| t.run_state).expect("thread"))
    }

    /// Everything the agent said in `id`, joined.
    pub fn answers(&self, cx: &TestAppContext, id: &str) -> String {
        self.items(cx, id).into_iter().filter_map(|i| if let Item::Assistant { text } = i { Some(text) } else { None }).collect::<Vec<_>>().join("\n")
    }

    /// Wait (in real time; the agent runs on other threads) until `until` holds.
    pub async fn wait(&self, cx: &mut TestAppContext, what: &str, until: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if self.ws.read_with(cx, |ws, _| until(ws)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            cx.background_executor.timer(Duration::from_millis(5)).await;
        }
    }

    /// Wait until `id`'s turn is over and the thread is settled in `state`.
    pub async fn wait_done(&self, cx: &mut TestAppContext, id: &str, state: RunState) {
        let id = id.to_string();
        self.wait(cx, &format!("thread to be {state:?}"), |ws| {
            ws.thread(&id).is_some_and(|t| t.run_state == state) && ws.live.get(&id).is_some_and(|l| l.turn_started.is_none())
        })
        .await;
    }

    /// Wait until `id` needs the user (an approval, a question or a plan is on screen).
    pub async fn wait_needs_you(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.wait(cx, "the agent to ask", |ws| ws.thread(&id).is_some_and(|t| t.run_state == RunState::NeedsYou)).await;
    }

    /// Type `text` in the composer and press Return. Returns the thread it went to.
    pub fn send(&self, cx: &mut TestAppContext, text: &str) -> String {
        self.type_text(cx, text);
        self.press(cx, "enter");
        self.thread_id(cx)
    }
}

/// The working bar's header split at its trail word: what follows it ("4s · 2 agents out"), or
/// `None` when it doesn't start with one ("Breaking trail… 4s").
pub fn trail_word(header: &str) -> Option<&str> {
    crate::mascot::WORDS.iter().find_map(|w| header.strip_prefix(*w)?.strip_prefix('…')).map(str::trim_start)
}

/// Store `items` as `thread`'s transcript, as if it had been saved there.
pub fn store_items(store: &Store, thread: &str, items: Vec<Item>) {
    store.save_transcript(thread, &mut trek_core::transcript::Transcript::unsaved(items)).expect("items");
}

/// A realistic transcript of `turns` turns: a question, a couple of tool calls, a markdown answer
/// with a list, inline paths and a code block, and the footer.
pub fn transcript(turns: usize) -> Vec<Item> {
    let mut items = Vec::with_capacity(turns * 6);
    for i in 0..turns {
        items.push(Item::User { text: format!("Step {i}: tighten the error handling in the request parser and add a test."), images: vec![], at: Some(1_759_400_000_000 + i as i64 * 60_000), resume: None, aside: false });
        items.push(Item::Tool { id: format!("t{i}a"), title: "Read".into(), detail: "src/parser.rs".into(), output: "pub fn parse(input: &str) -> Result<Request> { … }".into(), status: trek_core::store::ToolStatus::Done });
        items.push(Item::Tool { id: format!("t{i}b"), title: "Edit".into(), detail: "src/parser.rs".into(), output: "Applied 1 edit".into(), status: trek_core::store::ToolStatus::Done });
        items.push(Item::Assistant {
            text: format!(
                "### Step {i}\n\nThe parser in `src/parser.rs` now returns **typed errors** instead of panicking:\n\n- `ParseError::Truncated` when the body ends early\n- `ParseError::BadHeader` for malformed headers, with the line number\n- a regression test in `tests/parser.rs`\n\n```rust\nmatch parse(input) {{\n    Ok(req) => handle(req),\n    Err(ParseError::Truncated) => reply(400, \"truncated body\"),\n    Err(e) => reply(400, &e.to_string()),\n}}\n```\n\nAll {} tests pass.",
                40 + i
            ),
        });
        items.push(Item::TurnEnd { at: 1_759_400_000_000 + i as i64 * 60_000 + 30_000, took_secs: 30 });
    }
    items
}

/// Fill the store with an inbox like a real one: a handful of live threads, a pinned and a
/// snoozed one, and a long settled history across several projects.
pub fn populate(store: &Store, settled: usize) {
    use trek_core::store::now_ms;
    let projects: Vec<PathBuf> = (0..8).map(|i| new_project(&format!("repo{i}"))).collect();
    let now = now_ms();
    for i in 0..(settled + 15) {
        let cwd = &projects[i % projects.len()];
        let mut t = store.create_thread(Some(cwd), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
        t.title = format!("Thread {i}: fix the flaky integration test in the payments service");
        t.updated_at = now - i as i64 * 3_600_000;
        t.last_seen_at = if i % 3 == 0 { t.updated_at - 1 } else { t.updated_at };
        match i {
            0..12 => {}
            12 => t.pinned_at = Some(now),
            13 => t.snoozed_until = Some(now + 3_600_000),
            _ => t.settled_at = Some(t.updated_at),
        }
        store.save_thread(&t).expect("save");
    }
}
