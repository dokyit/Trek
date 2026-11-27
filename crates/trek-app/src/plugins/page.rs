//! A plugin's page in the main pane: its HTML run in the system's web view, fenced as
//! `trek_core::plugins::page` says (its own origin and files, no network it wasn't allowed, one
//! channel to Trek).
//!
//! The view exists only while its page is open: it's made when the route becomes the page and
//! dropped when the route leaves it, so a page that isn't on screen has no process. A web view is
//! a native view laid over the window, which GPUI can't draw over: it's hidden while a menu, the
//! palette or a dialog is up (`Workspace::overlay_open`). Before it's hidden or dropped the keys
//! go back to Trek's own view (`release_keys`): a native view that held the keyboard and then
//! went would leave the window with nothing to type into.

use super::Plugins;
use crate::workspace::{Route, Workspace, WorkspaceEvent};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use trek_core::plugins::Manifest;
use trek_core::plugins::page::{self as core, Request};

/// The web view a page runs in: a native view, or (in tests) a stand-in that keeps what it was
/// told.
pub trait Surface {
    fn element(&self) -> AnyElement;
    /// On screen or not. Hiding hands the keys back first.
    fn set_shown(&self, shown: bool, cx: &mut App);
    /// Run `script` in the page.
    fn run(&self, script: &str, cx: &mut App);
    /// Give the keys back to Trek's window (before the view is hidden or dropped).
    fn release_keys(&self, cx: &mut App);
}

enum Web {
    /// Being made, or the page isn't open.
    None,
    Made(Box<dyn Surface>),
    /// No web view could be made (no WebView2 runtime).
    Unavailable,
}

/// What a page's view is made with.
pub struct Spec {
    /// The plugin's id, for its log.
    pub plugin: String,
    pub origin: String,
    /// The page's address under the origin.
    pub start: String,
    /// The plugin's folder, whose files are served.
    pub dir: PathBuf,
    pub csp: String,
    pub bootstrap: String,
    /// The theme's stylesheet as served in each HTML file: replaced when the theme changes.
    pub theme_css: Arc<Mutex<String>>,
    /// The window colour, for the moment before the page draws.
    pub background: (u8, u8, u8),
    pub messages: async_channel::Sender<String>,
    /// The hash the plugin's files were allowed at, for one with permissions (see `respond`).
    pub sealed: Option<String>,
}

/// The page open in the main pane.
pub struct PageHost {
    ws: Entity<Workspace>,
    /// A panel's host shows this one (plugin, panel) whatever the route; the main pane's follows
    /// the route.
    fixed: Option<(String, String)>,
    /// A panel's tab is the one in front.
    active: bool,
    /// (plugin, page): what's open.
    open: Option<(String, String)>,
    title: String,
    web: Web,
    nonce: String,
    shown: bool,
    theme_css: Arc<Mutex<String>>,
    /// The plugins folder's read the page was loaded at: a newer one reloads it.
    generation: u64,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for PageHost {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The answer to a request for `path` under a plugin's origin: one of Trek's fonts, or a file of
/// the plugin's own (an HTML file with Trek's head first: the policy and the theme). A plugin with
/// permissions (`sealed`: the hash its files had when the user allowed it) has its pages served
/// only while its files still hash so: one edited since (even at the same size and time, which
/// the folder watcher wouldn't see) gets no page until the user allows it again.
pub fn respond(
    dir: &std::path::Path,
    path: &str,
    csp: &str,
    theme_css: &str,
    sealed: Option<&str>,
) -> wry::http::Response<Vec<u8>> {
    let fail = |status: u16| {
        wry::http::Response::builder()
            .status(status)
            .header("Cache-Control", "no-store")
            .body(Vec::new())
            .unwrap_or_default()
    };
    let base = |mime: &str| {
        wry::http::Response::builder()
            .header("Content-Type", mime)
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-store")
            .header("Content-Security-Policy", csp)
    };
    if let Some(font) = path.strip_prefix(core::FONTS_PATH) {
        return match crate::fonts::face(font) {
            Some(bytes) => base("font/ttf").body(bytes.to_vec()).unwrap_or_default(),
            None => fail(404),
        };
    }
    let Some(file) = core::resolve(dir, path) else {
        return fail(404);
    };
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > core::MAX_FILE_BYTES) {
        return fail(413);
    }
    let Ok(bytes) = std::fs::read(&file) else {
        return fail(404);
    };
    let mime = core::mime(&file);
    if let Some(hash) = sealed
        && (mime.starts_with("text/html") || mime.contains("javascript"))
        && trek_core::plugins::grant::content_hash(dir).as_deref() != Some(hash)
    {
        return fail(403);
    }
    if mime.starts_with("text/html") {
        let html = String::from_utf8_lossy(&bytes);
        let page = trek_core::artifact::document_with(&html, theme_css, csp);
        return base(mime).body(page.into_bytes()).unwrap_or_default();
    }
    base(mime).body(bytes).unwrap_or_default()
}

/// The stylesheet a page gets first: Trek's fonts, then the theme's variables and quiet defaults
/// (an opaque page in the window's colour: a page fills its pane).
fn theme_css(cx: &App) -> String {
    let mut theme = crate::artifact::theme(cx);
    theme.radius *= corner_scale();
    theme.accent = {
        let c = Rgba::from(cx.theme().primary);
        format!(
            "#{:02x}{:02x}{:02x}",
            (c.r * 255.).round() as u8,
            (c.g * 255.).round() as u8,
            (c.b * 255.).round() as u8
        )
    };
    format!(
        "{}{}:where(html){{background:var(--trek-bg)}}:where(body){{padding:28px 36px 40px}}",
        core::fonts_css(),
        trek_core::artifact::theme_css(&theme)
    )
}

fn theme_value(cx: &App) -> Value {
    json!({
        "dark": cx.theme().mode.is_dark(),
        "id": crate::themes::shown_id(cx),
    })
}

fn nonce() -> String {
    format!("{:032x}", uuid::Uuid::new_v4().as_u128())
}

impl PageHost {
    pub fn new(ws: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions =
            vec![cx.observe_in(&ws, window, |this, _, window, cx| this.sync(window, cx))];
        Self {
            ws,
            fixed: None,
            active: false,
            open: None,
            title: String::new(),
            web: Web::None,
            nonce: String::new(),
            shown: false,
            theme_css: Arc::new(Mutex::new(String::new())),
            generation: 0,
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// A host for plugin `plugin`'s panel `panel`, in the right panel: its view lives while its
    /// tab does.
    pub fn panel(
        ws: Entity<Workspace>,
        plugin: String,
        panel: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(ws, window, cx);
        this.fixed = Some((plugin, panel));
        this.sync(window, cx);
        this
    }

    /// Whether this is the host of plugin `plugin`'s panel `panel`.
    pub fn shows(&self, plugin: &str, panel: &str) -> bool {
        self.fixed
            .as_ref()
            .is_some_and(|(p, g)| p == plugin && g == panel)
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    /// A panel's tab came to the front, or left it (or a menu covers it).
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.show_or_hide(cx);
        }
    }

    fn show_or_hide(&mut self, cx: &mut Context<Self>) {
        let overlay = self.ws.read(cx).overlay_open;
        let shown = self.open.is_some() && !overlay && (self.fixed.is_none() || self.active);
        if shown != self.shown {
            self.shown = shown;
            if let Web::Made(web) = &self.web {
                web.set_shown(shown, cx);
            }
        }
    }

    /// Whether a page's web view exists (a check's question: none unless a page is open).
    pub fn has_view(&self) -> bool {
        matches!(self.web, Web::Made(_))
    }

    /// The route decides: its page is opened, another's (or none) closes this one, an overlay
    /// hides it, a plugin edited on disk reloads it.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = match (&self.fixed, &self.ws.read(cx).route) {
            (Some(fixed), _) => Some(fixed.clone()),
            (None, Route::Page { plugin, page }) => Some((plugin.clone(), page.clone())),
            _ => None,
        };
        // A page whose plugin stopped running, or dropped the page, closes.
        let want = want.filter(|(plugin, page)| page_of(plugin, page, cx).is_some());
        if want != self.open {
            self.close(window, cx);
            if let Some((plugin, page)) = want.clone() {
                self.open(plugin, page, window, cx);
            }
        }
        let generation = cx.try_global::<Plugins>().map_or(0, |p| p.generation);
        if self.open.is_some() && generation != self.generation {
            self.generation = generation;
            if let Web::Made(web) = &self.web {
                web.run("location.reload()", cx);
            }
        }
        self.show_or_hide(cx);
    }

    /// Close the page: the native view gives the keys back to the window's own view first,
    /// then it goes. Which of Trek's views has the focus is the navigation's to say (it has
    /// already focused the composer, or whatever the new route shows): taking it here would
    /// leave typing going nowhere.
    fn close(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Web::Made(web) = &self.web {
            web.release_keys(cx);
            web.set_shown(false, cx);
        }
        self.web = Web::None;
        self.open = None;
        self.shown = false;
        self._tasks.clear();
    }

    fn open(&mut self, plugin: String, page: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((manifest, dir, entry)) = page_of(&plugin, &page, cx) else {
            return;
        };
        self.title = manifest
            .pages
            .iter()
            .chain(&manifest.panels)
            .find(|p| p.id == page)
            .map(|p| p.title.clone())
            .unwrap_or_default();
        self.open = Some((plugin.clone(), page));
        self.nonce = nonce();
        self.generation = cx.try_global::<Plugins>().map_or(0, |p| p.generation);
        *self.theme_css.lock().unwrap_or_else(|e| e.into_inner()) = theme_css(cx);
        let origin = core::origin(&plugin, cfg!(windows));
        let (tx, rx) = async_channel::unbounded::<String>();
        let bg = Rgba::from(cx.theme().background);
        let spec = Spec {
            plugin: plugin.clone(),
            start: core::url(&origin, &entry),
            origin,
            dir,
            csp: core::csp(&manifest),
            bootstrap: core::bootstrap(&self.nonce, &theme_value(cx)),
            theme_css: self.theme_css.clone(),
            background: (
                (bg.r * 255.) as u8,
                (bg.g * 255.) as u8,
                (bg.b * 255.) as u8,
            ),
            messages: tx,
            sealed: sealed_hash(&plugin, cx),
        };
        self.make(spec, window, cx);
        // What the page asks, answered here.
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(body) = rx.recv().await {
                let Ok(()) = this.update_in(cx, |this, window, cx| this.heard(&body, window, cx))
                else {
                    break;
                };
            }
        }));
    }

    /// Make the web view. The native one is made in a task, between updates: on Windows wry waits
    /// for WebView2 in a message loop of its own, which runs whatever GPUI work falls due
    /// meanwhile, and inside an update that work finds the app taken (`ide::preview`).
    fn make(&mut self, spec: Spec, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-support"))]
        {
            let _ = (spec, window);
            self.web = match fake::ON.with(std::cell::Cell::get) {
                true => Web::Made(Box::new(fake::FakeWeb::default())),
                false => Web::Unavailable,
            };
            cx.notify();
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            let Some(parent) = crate::panels::browser::ParentWindow::of(window) else {
                self.web = Web::Unavailable;
                return;
            };
            let opening = self.open.clone();
            self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                let built = native::build(spec, &parent);
                let _ = this.update_in(cx, |this, window, cx| {
                    // The page may have been left while its view was made.
                    if this.open != opening {
                        return;
                    }
                    this.web = match built {
                        Some(built) => {
                            let web = Box::new(native::NativeWeb(
                                cx.new(|cx| gpui_wry::WebView::new(built, window, cx)),
                            )) as Box<dyn Surface>;
                            web.set_shown(this.shown, cx);
                            Web::Made(web)
                        }
                        None => Web::Unavailable,
                    };
                    cx.notify();
                });
            }));
        }
    }

    /// A message from the page: checked, allowed or refused by the plugin's permissions, done,
    /// and answered.
    fn heard(&mut self, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, request)) = core::parse_request(body, &self.nonce) else {
            return;
        };
        let Some((plugin, page)) = self.open.clone() else {
            return;
        };
        let Some((manifest, _, _)) = page_of(&plugin, &page, cx) else {
            return;
        };
        if let Some(needs) = request.needs()
            && !manifest.wants(&needs)
        {
            let why = format!(
                "this plugin wasn't given {} (add it to permissions in plugin.toml)",
                needs.key()
            );
            trek_core::plugins::log(&plugin, &format!("refused: {why}"));
            self.reply(id, Err(why), cx);
            return;
        }
        match request {
            Request::Escape => {
                if let Web::Made(web) = &self.web {
                    web.release_keys(cx);
                }
                self.focus.focus(window, cx);
            }
            Request::Toast(text) => {
                self.ws.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: text,
                        undo: None,
                    })
                });
                self.reply(id, Ok(Value::Null), cx);
            }
            Request::Open(thread) => {
                let there = self.ws.read(cx).thread(&thread).is_some();
                // Answered first: a panel's page stays while the main pane moves on, and its
                // promise settles.
                if there {
                    self.reply(id, Ok(Value::Null), cx);
                    self.ws
                        .update(cx, |ws, cx| ws.navigate(Route::Thread(thread), cx));
                } else {
                    self.reply(id, Err("there's no such thread".into()), cx);
                }
            }
            Request::NewThread => {
                self.reply(id, Ok(Value::Null), cx);
                self.ws
                    .update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
            }
            Request::Send { thread, text } => {
                let result = super::send_to_thread(&plugin, &thread, &text, &self.ws, cx);
                self.reply(id, result.map(|()| Value::Null), cx);
            }
            Request::StorageGet => {
                let plugin = plugin.clone();
                self.answer_later(id, window, cx, move || {
                    Ok(Value::Object(trek_core::plugins::storage(&plugin)))
                });
            }
            Request::StorageSet(value) => {
                let plugin = plugin.clone();
                self.answer_later(id, window, cx, move || {
                    trek_core::plugins::set_storage(&plugin, &value).map(|()| Value::Null)
                });
            }
            Request::StateGet => {
                let state = cx
                    .try_global::<super::Plugins>()
                    .and_then(|p| p.states.get(&plugin).cloned())
                    .unwrap_or_else(|| Value::Object(Default::default()));
                self.reply(id, Ok(state), cx);
            }
            Request::StateSet(patch) => {
                super::set_state(&plugin, Value::Object(patch), &self.ws, cx);
                self.reply(id, Ok(Value::Null), cx);
            }
            Request::Data(name) => match super::data::now(&name, &manifest, &self.ws, cx) {
                super::data::Answer::Now(value) => self.reply(id, Ok(value), cx),
                super::data::Answer::Later(work) => self.answer_later(id, window, cx, work),
                super::data::Answer::Unknown => self.reply(
                    id,
                    Err(format!(
                        "there's no data called {name}; Trek has {}",
                        core::DATA.map(|(n, _)| n).join(", ")
                    )),
                    cx,
                ),
            },
        }
    }

    /// Work `work` out off the UI thread (it reads the disk or runs git), then answer `id`.
    fn answer_later(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) {
        let opening = self.open.clone();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.open == opening {
                    this.reply(id, result, cx);
                }
            });
        }));
    }

    fn reply(&self, id: u64, result: Result<Value, String>, cx: &mut App) {
        if let Web::Made(web) = &self.web {
            web.run(&core::reply_script(&self.nonce, id, &result), cx);
        }
    }
}

/// Plugin `plugin`'s page or panel `page`, while the plugin runs: its manifest, its folder and
/// the file the page opens at.
/// The hash plugin `plugin`'s files were allowed at, when it has permissions to be allowed.
fn sealed_hash(plugin: &str, cx: &App) -> Option<String> {
    let pl = cx.try_global::<Plugins>()?.find(plugin)?;
    if pl.manifest.permissions.is_empty() {
        return None;
    }
    // With permissions and no hash, nothing hashes as allowed.
    Some(pl.hash.clone().unwrap_or_default())
}

pub fn page_of(plugin: &str, page: &str, cx: &App) -> Option<(Manifest, PathBuf, String)> {
    let p = cx.try_global::<Plugins>()?;
    let pl = p.find(plugin)?;
    if !p.state.running(&pl.manifest) {
        return None;
    }
    let entry = pl
        .manifest
        .pages
        .iter()
        .chain(&pl.manifest.panels)
        .find(|pg| pg.id == page)?
        .entry
        .clone();
    Some((pl.manifest.clone(), pl.dir.clone(), entry))
}

/// Why plugin `plugin`'s page isn't showing, in words: it couldn't be loaded (and what's wrong),
/// Trek stopped it (and why), it waits to be allowed, it's off, or it's gone.
pub fn why_closed(plugin: &str, cx: &App) -> String {
    let Some(p) = cx.try_global::<Plugins>() else {
        return "This page's plugin isn't running.".into();
    };
    if let Some(problem) = p.problems.iter().find(|pr| pr.id == plugin) {
        return format!(
            "This page's plugin couldn't be loaded: {}",
            problem.message.lines().next().unwrap_or_default()
        );
    }
    let Some(pl) = p.find(plugin) else {
        return "This page's plugin isn't installed any more.".into();
    };
    let name = &pl.manifest.name;
    let choice = p.state.choice(plugin);
    if let Some(why) = &choice.stopped {
        format!(
            "{name} was stopped: {}",
            why.lines().next().unwrap_or_default()
        )
    } else if choice.on || choice.waiting {
        format!("{name} is waiting for you to allow what it asks for.")
    } else {
        format!("{name} is off.")
    }
}

impl Render for PageHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The theme changed since the page last heard: its variables go in place, and its
        // listeners are told. A string compared, nothing read.
        if self.open.is_some() {
            let css = theme_css(cx);
            let changed = {
                let mut kept = self.theme_css.lock().unwrap_or_else(|e| e.into_inner());
                let changed = *kept != css;
                if changed {
                    *kept = css.clone();
                }
                changed
            };
            if changed && let Web::Made(web) = &self.web {
                web.run(&core::theme_script(&self.nonce, &css, &theme_value(cx)), cx);
            }
        }
        let note = |text: &'static str| {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_size(px(crate::scale::text::BODY))
                .text_color(theme.muted_foreground)
                .child(Icon::empty().path(super::icon_path("puzzle")).size(px(20.)))
                .child(text)
        };
        let body = match (&self.web, self.open.is_some()) {
            (Web::Made(web), _) => div().size_full().child(web.element()).into_any_element(),
            (Web::Unavailable, _) => note(if cfg!(windows) {
                "Pages show in Microsoft Edge WebView2, which couldn't be started on this PC."
            } else {
                "This page couldn't be shown."
            })
            .into_any_element(),
            (Web::None, true) => div().size_full().into_any_element(),
            (Web::None, false) => {
                let plugin = match (&self.fixed, &self.ws.read(cx).route) {
                    (Some((plugin, _)), _) | (None, Route::Page { plugin, .. }) => plugin.clone(),
                    _ => String::new(),
                };
                let ws = self.ws.clone();
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .px(px(32.))
                    .text_size(px(crate::scale::text::BODY))
                    .text_color(theme.muted_foreground)
                    .child(Icon::empty().path(super::icon_path("puzzle")).size(px(20.)))
                    .child(
                        div()
                            .id("plugin-page-closed")
                            .test_support()
                            .max_w(px(440.))
                            .text_center()
                            .child(why_closed(&plugin, cx)),
                    )
                    .child(
                        Button::new("plugin-page-settings")
                            .small()
                            .outline()
                            .label("Open Settings › Plugins")
                            .on_click(move |_, _, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.navigate(
                                        Route::Settings(crate::workspace::SettingsPage::Plugins),
                                        cx,
                                    )
                                })
                            }),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .id("plugin-page")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .id("plugin-page-title")
                    .test_support()
                    .flex_none()
                    .h(px(0.))
                    .overflow_hidden()
                    .child(self.title.clone()),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    //! A stand-in web view for tests: it keeps the scripts it was told to run and whether it's
    //! shown, and never takes the keys.
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    thread_local! {
        /// Whether tests get a stand-in web view (none, as with no WebView2, when not).
        pub static ON: Cell<bool> = const { Cell::new(true) };
        /// What the stand-ins on this thread were told, newest last.
        pub static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[derive(Default)]
    pub struct FakeWeb {
        shown: Rc<Cell<bool>>,
    }

    fn log(line: String) {
        LOG.with(|l| l.borrow_mut().push(line));
    }

    /// What the stand-ins were told since the last take.
    pub fn take() -> Vec<String> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    impl Surface for FakeWeb {
        fn element(&self) -> AnyElement {
            div()
                .id("plugin-page-web-fake")
                .test_support()
                .size_full()
                .into_any_element()
        }
        fn set_shown(&self, shown: bool, _: &mut App) {
            self.shown.set(shown);
            log(format!("shown {shown}"));
        }
        fn run(&self, script: &str, _: &mut App) {
            log(format!("run {script}"));
        }
        fn release_keys(&self, _: &mut App) {
            log("release-keys".into());
        }
    }
}

#[cfg_attr(any(test, feature = "test-support"), allow(dead_code))]
mod native {
    use super::*;

    /// A native web view for a page.
    pub struct NativeWeb(pub Entity<gpui_wry::WebView>);

    impl Surface for NativeWeb {
        fn element(&self) -> AnyElement {
            self.0.clone().into_any_element()
        }

        fn set_shown(&self, shown: bool, cx: &mut App) {
            self.0.update(cx, |v, _| {
                if shown {
                    v.show();
                } else {
                    // `hide` gives the keys back to the window's own view before the view goes.
                    v.hide();
                }
            });
        }

        fn run(&self, script: &str, cx: &mut App) {
            self.0.update(cx, |v, _| {
                let _ = v.evaluate_script(script);
            });
        }

        fn release_keys(&self, cx: &mut App) {
            self.0.update(cx, |v, _| {
                let _ = v.focus_parent();
            });
        }
    }

    /// The native view for `spec`, as a child of `parent`. Never from inside an update.
    pub fn build(
        spec: Spec,
        parent: &crate::panels::browser::ParentWindow,
    ) -> Option<wry::WebView> {
        let Spec {
            plugin,
            origin,
            start,
            dir,
            csp,
            bootstrap,
            theme_css,
            background,
            messages,
            sealed,
        } = spec;
        // The profile artifacts use, beside the Browser's and not in it: a plugin's page shares
        // no cookies, logins or storage with the user's own browsing.
        #[cfg(windows)]
        let mut context = wry::WebContext::new(Some(
            crate::panels::browser::webview_data_dir().with_file_name("WebView2-pages"),
        ));
        // It mustn't take the keys from Trek (WebView2 focuses a new view): a click on it does.
        #[cfg(windows)]
        let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_focused(false)
            .with_background_color((background.0, background.1, background.2, 255));
        #[cfg(not(windows))]
        let builder = wry::WebViewBuilder::new().with_background_color((
            background.0,
            background.1,
            background.2,
            255,
        ));
        let nav_origin = origin.clone();
        builder
            .with_url(start)
            .with_visible(false)
            .with_devtools(cfg!(debug_assertions))
            .with_incognito(true)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(false)
            .with_autoplay(false)
            .with_hotkeys_zoom(false)
            .with_initialization_script_for_main_only(bootstrap, true)
            .with_asynchronous_custom_protocol(core::SCHEME.into(), move |_, request, responder| {
                let (dir, csp, theme_css) = (dir.clone(), csp.clone(), theme_css.clone());
                let sealed = sealed.clone();
                let path = request.uri().path().to_string();
                let plugin = plugin.clone();
                // Files are read off the UI thread.
                std::thread::spawn(move || {
                    let css = theme_css.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let answer = respond(&dir, &path, &csp, &css, sealed.as_deref());
                    // A file the page asked for that isn't there is said in the plugin's log:
                    // it's how its author finds a wrong path.
                    if answer.status() != 200 && path != "/favicon.ico" {
                        trek_core::plugins::log(
                            &plugin,
                            &format!("page asked for {path}: {}", answer.status()),
                        );
                    }
                    responder.respond(answer)
                });
            })
            .with_ipc_handler(move |request| {
                let _ = messages.try_send(request.into_body());
            })
            .with_navigation_handler(move |url| core::navigation_allowed(&url, &nav_origin))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .build_as_child(parent)
            .inspect_err(|e| tracing::warn!("plugin page: no web view: {e}"))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::respond;

    #[test]
    fn a_sealed_plugins_page_is_served_only_as_it_was_allowed() {
        let dir =
            std::env::temp_dir().join(format!("trek_test_sealed_page_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<p>a</p>").unwrap();
        std::fs::write(dir.join("page.js"), "let a = 1;").unwrap();
        let hash = trek_core::plugins::grant::content_hash(&dir).unwrap();
        let get = |path: &str, sealed: Option<&str>| respond(&dir, path, "", "", sealed).status();
        assert_eq!(get("/index.html", Some(&hash)), 200);
        assert_eq!(get("/page.js", Some(&hash)), 200);
        // Edited at the same size: the folder watcher, which goes by size and time, can miss it.
        std::fs::write(dir.join("page.js"), "let a = 2;").unwrap();
        assert_eq!(get("/index.html", Some(&hash)), 403);
        assert_eq!(get("/page.js", Some(&hash)), 403);
        // A plugin with no permissions has nothing sealed.
        assert_eq!(get("/index.html", None), 200);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
