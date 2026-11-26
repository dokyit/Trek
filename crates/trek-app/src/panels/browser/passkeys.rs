//! Passkeys in the Browser (macOS): Trek's own authenticator behind `navigator.credentials`.
//!
//! WebKit refuses passkeys to an app without Apple's managed web-browser entitlement, which
//! Trek's own signature can't carry, so every page's request failed. Each view now gets two
//! scripts before its first page loads: `passkeys_page.js` in the page's world, which replaces
//! `navigator.credentials.create/get` for `publicKey` and the `PublicKeyCredential` feature
//! checks, and `passkeys_bridge.js` in a content world of its own, the only place Trek's native
//! handler (`trekPasskeys`) exists. The page's side can only raise an event; the bridge passes it
//! on as text.
//!
//! Nothing the page says is believed. The handler takes the frame's origin from WebKit
//! (`WKFrameInfo`), answers only the page itself (no frames), and `trek_passkeys::parse` checks
//! the request against that origin. Then the user decides, in Trek's own bar above the page (or,
//! when an agent is driving the Browser, on its thread's approval card: `agent_core::Sensitive
//! ::Passkey`, asked at every access level), and Touch ID confirms each use
//! (`trek_passkeys::Authenticator`). The bar is Trek's, not the page's: no script, and no agent's
//! browser tool, can press it.

use super::{BrowserPanel, Msg, agent_core as core};
use crate::assets::Lucide;
use crate::scale::{text, to_rems};
use block2::{Block, RcBlock};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::NSString;
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandlerWithReply, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use trek_agents::Decision;
use trek_passkeys::ceremony::account_words;
use trek_passkeys::{Error, Origin, Passkey, Request, Store};
use wry::WebViewExtMacOS as _;

/// The page's side, with whether a request can wait quietly for the user (conditional
/// mediation: Trek offers the passkey on its bar when it holds one for the site).
fn page_js() -> String {
    include_str!("passkeys_page.js").replace("__TREK_CONDITIONAL__", "true")
}

const BRIDGE_JS: &str = include_str!("passkeys_bridge.js");

/// The bridge's content world and the native handler's name in it.
const WORLD: &str = "TrekPasskeys";
const HANDLER: &str = "trekPasskeys";

/// How long an agent thread's approval card waits for the user.
const CARD_WAIT: Duration = Duration::from_secs(15 * 60);

type ReplyBlock = RcBlock<dyn Fn(*mut AnyObject, *mut NSString)>;

thread_local! {
    /// WebKit's reply handlers, by the id each request was given: every one is called once.
    static REPLIES: RefCell<HashMap<u64, ReplyBlock>> = RefCell::default();
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Whether new views get Trek's passkeys: the user's setting (Settings › Tools & MCP › Browser ›
/// Passkeys), as the panel last read it.
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// New views answer passkey requests with Trek's authenticator, or leave them to WebKit.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Answer request `id`'s page with `value` (`{"ok": …}` or `{"error": …}`). Once: a second
/// answer finds nothing to call.
fn reply(id: u64, value: &Value) {
    let Some(block) = REPLIES.with(|r| r.borrow_mut().remove(&id)) else {
        return;
    };
    let text = NSString::from_str(&value.to_string());
    block.call((
        Retained::as_ptr(&text).cast_mut().cast(),
        std::ptr::null_mut(),
    ));
}

fn refuse(id: u64, error: &Error) {
    reply(id, &error.to_json());
}

pub struct Ivars {
    tab: u64,
    tx: async_channel::Sender<Msg>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are dropped with the object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TrekBrowserPasskeyHandler"]
    #[ivars = Ivars]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl WKScriptMessageHandlerWithReply for Handler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:replyHandler:))]
        fn received(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
            done: &Block<dyn Fn(*mut AnyObject, *mut NSString)>,
        ) {
            let v = self.ivars();
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            REPLIES.with(|r| r.borrow_mut().insert(id, done.copy()));
            let body = unsafe { message.body() }
                .downcast::<NSString>()
                .map(|s| s.to_string())
                .unwrap_or_default();
            // Who asks is WebKit's to say: the frame the message came from, and its origin.
            let frame = unsafe { message.frameInfo() };
            let origin = unsafe { frame.securityOrigin() };
            let port = u16::try_from(unsafe { origin.port() }).unwrap_or(0);
            let origin = Origin::new(
                &unsafe { origin.protocol() }.to_string(),
                &unsafe { origin.host() }.to_string(),
                port,
            );
            let sent = v.tx.try_send(Msg::Passkey {
                tab: v.tab,
                id,
                origin,
                main_frame: unsafe { frame.isMainFrame() },
                body,
            });
            if sent.is_err() {
                refuse(id, &Error::not_allowed("The request was cancelled."));
            }
        }
    }
);

/// Give view `wv` (tab `tab`) Trek's passkeys, before it loads its first page. Does nothing
/// where Trek has no authenticator.
pub fn install(wv: &wry::WebView, tab: u64, tx: async_channel::Sender<Msg>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !ENABLED.load(Ordering::Relaxed) || trek_passkeys::platform().is_none() {
        return;
    }
    let controller = unsafe { wv.webview().configuration().userContentController() };
    let script = |source: &str, world: &WKContentWorld| unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            WKUserScript::alloc(mtm),
            &NSString::from_str(source),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
            world,
        )
    };
    let handler: Retained<Handler> = {
        let this = mtm.alloc::<Handler>().set_ivars(Ivars { tab, tx });
        unsafe { msg_send![super(this), init] }
    };
    unsafe {
        let world = WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm);
        let name = NSString::from_str(HANDLER);
        // A name can be taken once per world: adding it twice is an exception.
        controller.removeScriptMessageHandlerForName_contentWorld(&name, &world);
        controller.addScriptMessageHandlerWithReply_contentWorld_name(
            ProtocolObject::from_ref(&*handler),
            &world,
            &name,
        );
        controller.addUserScript(&script(BRIDGE_JS, &world));
        controller.addUserScript(&script(&page_js(), &WKContentWorld::pageWorld(mtm)));
    }
}

/// Where a page's request stands.
#[derive(Debug, Clone, PartialEq)]
enum Stage {
    /// On the bar above the page, for the user.
    Bar,
    /// A request that waits quietly (conditional mediation, autofill's kind) for which Trek has
    /// a passkey: a key in the toolbar offers it, and the bar opens when the user presses it.
    /// Nothing about the page changes until then, so the page can't tell an offer from none.
    Offer,
    /// Nothing shows: a quiet request with no passkey to offer. It ends when the page takes it
    /// back or goes.
    Quiet,
    /// On an agent thread's approval card (`thread`, the card's request).
    Card { thread: String, request: String },
    /// The user said yes: Touch ID is up.
    Verifying,
}

/// A page's passkey request, waiting on the user.
pub(super) struct PasskeyAsk {
    /// The reply's id (`reply`).
    id: u64,
    tab: u64,
    /// The page's own name for the request, which it takes it back by.
    page_id: String,
    request: Request,
    /// The passkeys that could answer a sign-in.
    candidates: Vec<Passkey>,
    /// The site already has one of Trek's passkeys for the account (`excludeCredentials`).
    already: bool,
    /// Trek keeps a passkey for the account, which a new one takes the place of.
    replaces: bool,
    stage: Stage,
    /// When the bar last changed what it asks: its buttons take no click for a moment after
    /// (`BAR_SETTLES`), so a page can't put a request under a click already on its way.
    since: Instant,
    /// The page went (a new page, its tab closed, the request taken back) while Touch ID was
    /// up: what comes of it reaches nobody.
    gone: bool,
}

/// What the bar says and offers for a request.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BarWords {
    pub headline: String,
    pub note: String,
    /// The button that says no, if any.
    pub no: Option<&'static str>,
    /// The buttons that say yes: one, or one per account.
    pub yes: Vec<String>,
}

/// The bar's height, whatever it says. The page's view is laid out under the bar, so a page
/// can measure how tall it is: one height for every request means the measurement says only
/// that a request is on the bar, which the page that made it knows already, and never whether
/// Trek keeps a passkey for it.
const BAR_HEIGHT: f32 = 96.;

/// How long the bar's buttons ignore clicks after what it asks changes.
const BAR_SETTLES: Duration = Duration::from_millis(400);

/// What a page is told when a request ends without a credential, whatever ended it (the user's
/// Cancel, no passkey to use, a refused Touch ID, the page gone): one answer, so it says nothing
/// of what Trek keeps.
const DECLINED: &str = "The request was cancelled or no passkey could be used.";

fn declined() -> Error {
    Error::not_allowed(DECLINED)
}

impl PasskeyAsk {
    fn site(&self) -> &str {
        match &self.request {
            Request::Create(c) => &c.rp_id,
            Request::Get(g) => &g.rp_id,
        }
    }

    fn conditional(&self) -> bool {
        matches!(&self.request, Request::Get(g) if g.conditional)
    }

    fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.since = Instant::now();
    }

    /// What the bar says for this request at this stage (`None`: it isn't on the bar).
    fn words(&self) -> Option<BarWords> {
        // The computer by the name Trek's word table gives it; iCloud Keychain by Apple's own.
        let here = crate::words::words().this_computer;
        let elsewhere = "iCloud Keychain or a password manager"; // words: ok
        let confirms = format!(
            "Touch ID or your password confirms it. The passkey is one Trek keeps on {here}."
        );
        let site = self.site();
        let just = |headline: String, note: &str| BarWords {
            headline,
            note: note.to_string(),
            no: None,
            yes: vec![],
        };
        match &self.stage {
            Stage::Quiet | Stage::Offer => return None,
            Stage::Verifying => {
                let what = match &self.request {
                    Request::Create(_) => "create the passkey",
                    Request::Get(_) => "use your passkey",
                };
                return Some(just(
                    format!("On {site}: confirm it’s you to {what}"),
                    "Touch ID or your Mac’s password, in the system’s own prompt.",
                ));
            }
            Stage::Card { .. } => {
                return Some(BarWords {
                    no: Some("Cancel"),
                    ..just(
                        format!("On {site}: an agent’s passkey request waits on its thread"),
                        "Allow or deny it on the thread’s approval card.",
                    )
                });
            }
            Stage::Bar => {}
        }
        Some(match &self.request {
            Request::Create(_) if self.already => BarWords {
                no: Some("Cancel"),
                yes: vec!["OK".into()],
                ..just(
                    format!("On {site}: Trek already has a passkey for this account"),
                    "Nothing will be added. Sign in with it, or delete it in Settings › Tools & MCP › Browser › Passkeys first.",
                )
            },
            Request::Create(c) => BarWords {
                no: Some("Cancel"),
                yes: vec!["Continue".into()],
                ..just(
                    format!(
                        "On {site}: create a passkey for “{}”?",
                        account_words(&c.user_name, &c.user_display_name)
                    ),
                    &format!(
                        "Trek keeps it on {here} and signs you in with it in this browser{}. It is not saved to {elsewhere}.",
                        if self.replaces {
                            ", in place of the passkey it has for this account now"
                        } else {
                            ""
                        }
                    ),
                )
            },
            Request::Get(_) if self.candidates.is_empty() => BarWords {
                yes: vec!["OK".into()],
                ..just(
                    format!("On {site}: Trek has no passkey for this site"),
                    &format!(
                        "Passkeys in {elsewhere} can’t be used in this browser. Sign in another way, then add a passkey in the account’s security settings here: Trek keeps it for next time."
                    ),
                )
            },
            Request::Get(g) => {
                let no = Some(if g.conditional { "Not Now" } else { "Cancel" });
                if let [one] = self.candidates.as_slice() {
                    BarWords {
                        no,
                        yes: vec![if g.conditional { "Sign In" } else { "Continue" }.into()],
                        ..just(
                            format!("On {site}: sign in as “{}”?", one.account()),
                            &confirms,
                        )
                    }
                } else {
                    BarWords {
                        no,
                        yes: self.candidates.iter().map(Passkey::account).collect(),
                        ..just(
                            format!("On {site}: sign in with a passkey. Which account?"),
                            &confirms,
                        )
                    }
                }
            }
        })
    }

    /// What an agent thread's approval card asks (after "<Agent> wants to:").
    fn card(&self, page: &str) -> core::Ask {
        let site = self.site().to_string();
        let (title, what) = match &self.request {
            Request::Create(c) => (
                format!(
                    "Create a passkey on {site} for “{}”",
                    account_words(&c.user_name, &c.user_display_name)
                ),
                format!(
                    "adds a passkey to that account, kept in Trek on {}",
                    crate::words::words().this_computer
                ),
            ),
            Request::Get(_) => (
                match self.candidates.as_slice() {
                    [one] => format!(
                        "Sign in to {site} with your passkey for “{}”",
                        one.account()
                    ),
                    _ => format!("Sign in to {site} with one of your passkeys"),
                },
                "signs the Browser in to that account".to_string(),
            ),
        };
        core::passkey_ask(
            title,
            format!(
                "A page in the Browser ({page}) asked for it while the agent was using the Browser. Allowing it {what}; Touch ID confirms it. It is asked each time."
            ),
            site,
        )
    }
}

/// The request a page takes back (`{"kind": "cancel", "of": "<its id>"}`), if `body` is one.
fn cancelled_id(body: &Value) -> Option<&str> {
    (body.get("kind")?.as_str()? == "cancel")
        .then(|| body.get("of")?.as_str())
        .flatten()
}

impl BrowserPanel {
    /// A page in tab `tab` asked for a passkey (or took a request back): WebKit's handler `id`
    /// waits for the answer. `origin` and `main_frame` are WebKit's own account of who asks.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn passkey_message(
        &mut self,
        tab: u64,
        id: u64,
        origin: Origin,
        main_frame: bool,
        body: String,
        cx: &mut Context<Self>,
    ) {
        // Only the page itself, never a frame in it: a frame of another site must not use the
        // user's passkeys unless its embedder says so, and WebKit doesn't tell Trek what the
        // embedder allowed. (WebAuthn's default is the same: `publickey-credentials-get` is
        // `self`.)
        if !main_frame || !self.tabs.iter().any(|t| t.id == tab) {
            return refuse(
                id,
                &Error::not_allowed(
                    "Passkeys are answered for the page itself, not a frame in it.",
                ),
            );
        }
        let parsed: Value = serde_json::from_str(&body).unwrap_or_default();
        if let Some(of) = cancelled_id(&parsed) {
            self.passkeys_end(|a| a.tab == tab && a.page_id == of, cx);
            return reply(id, &json!({ "ok": true }));
        }
        // One at a time per page, before anything is read or looked up (a page can send these
        // as fast as it likes): a request the user is being asked about, or has answered, stays,
        // and the new one is refused, as a browser refuses a second dialog. One that only waits
        // quietly gives way (a site ends its autofill request when its button is pressed).
        let stays = |a: &PasskeyAsk| !matches!(a.stage, Stage::Quiet | Stage::Offer);
        if self.passkeys.iter().any(|a| a.tab == tab && stays(a)) {
            return refuse(
                id,
                &Error::not_allowed("A passkey request is already pending."),
            );
        }
        let request = match trek_passkeys::parse(&origin, &body) {
            Ok(request) => request,
            Err(e) => {
                tracing::info!(
                    "browser: passkey request from {} refused: {}",
                    origin.serialize(),
                    e.name
                );
                return refuse(id, &e);
            }
        };
        let store = Store::default();
        let looked_up = match &request {
            Request::Create(c) => c
                .already(&store)
                .and_then(|already| Ok((vec![], already, c.replaces(&store)?))),
            Request::Get(g) => g.candidates(&store).map(|c| (c, false, false)),
        };
        let Ok((candidates, already, replaces)) = looked_up else {
            return refuse(id, &declined());
        };
        self.passkeys_end(|a| a.tab == tab, cx);
        let mut ask = PasskeyAsk {
            id,
            tab,
            page_id: parsed["id"].as_str().unwrap_or_default().to_string(),
            request,
            candidates,
            already,
            replaces,
            stage: Stage::Bar,
            since: Instant::now(),
            gone: false,
        };
        tracing::info!(
            "browser: {} asks to {} a passkey for {}",
            origin.serialize(),
            if matches!(ask.request, Request::Create(_)) {
                "create"
            } else {
                "use"
            },
            ask.site()
        );
        if ask.conditional() {
            // Autofill's request never changes what the page can see: a key in the toolbar when
            // there is a passkey to offer, nothing when there isn't.
            ask.stage = if ask.candidates.is_empty() {
                Stage::Quiet
            } else {
                Stage::Offer
            };
        } else if let Some(thread) = self.passkey_agent(cx) {
            // An agent is driving: its thread's card asks, at any access level.
            let card = ask.card(&origin.serialize());
            debug_assert!(core::needs_asking(
                &card.kind,
                trek_core::HandHolding::FullAccess
            ));
            if let Some(request) = self.passkey_card(id, &thread, card, cx) {
                ask.stage = Stage::Card { thread, request };
            }
        }
        self.passkeys.push(ask);
        cx.notify();
    }

    /// The thread whose agent a request arriving now is put down to (`Driver::passkey_thread`):
    /// one whose call is under way or just ended, or whose turn is still running, however long
    /// ago its last call was (a page can wait before it asks).
    fn passkey_agent(&self, cx: &App) -> Option<String> {
        let workspace = self.workspace.read(cx);
        self.agent
            .passkey_thread(|thread| workspace.browser_turn_running(thread))
    }

    /// Put `card` on `thread`'s approval card and wait for the user there: the card's request
    /// id, or `None` when the thread has no card to put it on (the bar asks instead).
    fn passkey_card(
        &mut self,
        id: u64,
        thread: &str,
        card: core::Ask,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let workspace = self.workspace.clone();
        let (request, rx) = workspace.update(cx, |ws, cx| {
            ws.browser_ask_each_time(thread, card.title.clone(), card.detail.clone(), cx)
        });
        if !workspace.read(cx).browser_ask_pending(thread, &request) {
            return None;
        }
        let (thread, card_request) = (thread.to_string(), request.clone());
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let decision = loop {
                match rx.try_recv() {
                    Ok(d) => break Some(d),
                    Err(async_channel::TryRecvError::Closed) => break None,
                    Err(async_channel::TryRecvError::Empty) => {}
                }
                // The turn ended (taking its card), the request went, or nobody answered.
                let pending =
                    workspace.update(cx, |ws, _| ws.browser_ask_pending(&thread, &card_request));
                let wanted = this
                    .update(cx, |p, _| p.passkeys.iter().any(|a| a.id == id))
                    .unwrap_or(false);
                if !pending || !wanted || started.elapsed() > CARD_WAIT {
                    tracing::info!(
                        "browser: a passkey's approval card went unanswered (on the thread: {pending}, still asked for: {wanted})"
                    );
                    workspace.update(cx, |ws, cx| ws.browser_withdraw(&thread, &card_request, cx));
                    break None;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
            };
            let _ = this.update(cx, |p, cx| p.passkey_carded(id, decision, cx));
        })
        .detach();
        Some(request)
    }

    /// The thread's card was answered (`None`: it went unanswered).
    fn passkey_carded(&mut self, id: u64, decision: Option<Decision>, cx: &mut Context<Self>) {
        let Some(ask) = self.passkeys.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if !matches!(ask.stage, Stage::Card { .. }) {
            return;
        }
        tracing::info!("browser: a passkey's approval card was answered: {decision:?}");
        match decision {
            // "Allow for session" allows this one too, and no more: a passkey is asked for
            // each time.
            Some(Decision::Allow | Decision::AllowForSession) => {
                ask.set_stage(Stage::Bar);
                let choice = match (&ask.request, ask.candidates.len()) {
                    (Request::Create(_), _) if !ask.already => Some(0),
                    (Request::Get(_), 1) => Some(0),
                    // Which account, or nothing to use: the bar says.
                    _ => None,
                };
                if let Some(choice) = choice {
                    self.passkey_yes(id, choice, cx);
                }
            }
            Some(Decision::Deny) => self.passkey_end(id, &declined(), cx),
            // No card any more (the agent's turn ended): the user is here, the bar asks.
            None => ask.set_stage(Stage::Bar),
        }
        cx.notify();
    }

    /// End request `id` without a credential.
    fn passkey_end(&mut self, id: u64, error: &Error, cx: &mut Context<Self>) {
        let Some(ix) = self.passkeys.iter().position(|a| a.id == id) else {
            return;
        };
        let ask = self.passkeys.remove(ix);
        if let Stage::Card { thread, request } = &ask.stage {
            self.workspace
                .update(cx, |ws, cx| ws.browser_withdraw(thread, request, cx));
        }
        refuse(id, error);
        cx.notify();
    }

    /// The requests `which` picks are over (their page took them back, or went): each ends
    /// without a credential. One Touch ID is up for runs on, marked as gone: what it makes or
    /// signs reaches nobody (`passkey_done`).
    fn passkeys_end(&mut self, which: impl Fn(&PasskeyAsk) -> bool, cx: &mut Context<Self>) {
        let mut over = vec![];
        for ask in self.passkeys.iter_mut().filter(|a| which(a)) {
            if ask.stage == Stage::Verifying {
                ask.gone = true;
            } else {
                over.push(ask.id);
            }
        }
        for id in over {
            self.passkey_end(id, &declined(), cx);
        }
    }

    /// The toolbar key was pressed: the front tab's offer opens on the bar.
    pub(super) fn passkey_open_offer(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        if let Some(ask) = self
            .passkeys
            .iter_mut()
            .find(|a| a.tab == active && a.stage == Stage::Offer)
        {
            ask.set_stage(Stage::Bar);
            cx.notify();
        }
    }

    /// The site the toolbar's key offers a passkey for, when the front tab has an offer waiting.
    pub(super) fn passkey_offer(&self) -> Option<&str> {
        self.passkeys
            .iter()
            .find(|a| a.tab == self.active && a.stage == Stage::Offer)
            .map(|a| a.site())
    }

    /// The bar's no for request `id`: Cancel, or Not Now for one that waits quietly, which goes
    /// back to the toolbar's key and leaves the page waiting (as an ignored autofill does).
    pub(super) fn passkey_no(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ask) = self.passkeys.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if ask.conditional() && ask.stage == Stage::Bar {
            ask.set_stage(Stage::Offer);
            cx.notify();
        } else if ask.stage != Stage::Verifying {
            self.passkey_end(id, &declined(), cx);
        }
    }

    /// Whether request `id`'s bar has only just changed what it asks: a click now was aimed at
    /// what was there before.
    fn passkey_unsettled(&self, id: u64) -> bool {
        self.passkeys
            .iter()
            .any(|a| a.id == id && a.since.elapsed() < BAR_SETTLES)
    }

    /// The bar's yes for request `id`: its button `choice` (the account, when there are several).
    pub(super) fn passkey_yes(&mut self, id: u64, choice: usize, cx: &mut Context<Self>) {
        let Some(ask) = self.passkeys.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if ask.stage != Stage::Bar {
            return;
        }
        // The bars that only tell: nothing to make or use.
        let nothing = match &ask.request {
            Request::Create(_) => ask
                .already
                .then(|| Error::invalid_state("A passkey for this account is already kept here.")),
            Request::Get(_) => ask.candidates.is_empty().then(declined),
        };
        if let Some(error) = nothing {
            return self.passkey_end(id, &error, cx);
        }
        let Some(auth) = trek_passkeys::platform() else {
            return self.passkey_end(id, &declined(), cx);
        };
        let chosen = ask.candidates.get(choice).cloned();
        let request = ask.request.clone();
        ask.set_stage(Stage::Verifying);
        cx.notify();
        // Touch ID is waited for on a thread of its own.
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let store = Store::default();
            let now = trek_passkeys::now();
            let result = match (&request, chosen) {
                (Request::Create(c), _) => c
                    .finish(auth.as_ref(), &store, now)
                    .map(|(answer, passkey)| (answer, Some(passkey))),
                (Request::Get(g), Some(passkey)) => g
                    .finish(&passkey, auth.as_ref(), &store, now)
                    .map(|answer| (answer, None)),
                (Request::Get(_), None) => Err(declined()),
            };
            let _ = tx.send_blocking(result);
        });
        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err(declined()));
            let made = result.as_ref().ok().and_then(|(_, made)| made.clone());
            if this
                .update(cx, |p, cx| p.passkey_done(id, &result, cx))
                .is_err()
            {
                // The Browser closed meanwhile: nobody got the answer.
                if let Some(passkey) = made {
                    settle_created(passkey, false);
                }
                reply(id, &declined().to_json());
            }
        })
        .detach();
    }

    /// Touch ID answered for request `id`.
    fn passkey_done(
        &mut self,
        id: u64,
        result: &Result<(Value, Option<Passkey>), Error>,
        cx: &mut Context<Self>,
    ) {
        let ask = self
            .passkeys
            .iter()
            .position(|a| a.id == id)
            .map(|ix| self.passkeys.remove(ix));
        // Its page is still the one that asked: only then has anyone the answer.
        let delivered = ask.as_ref().is_some_and(|a| !a.gone);
        match result {
            Ok((answer, made)) => {
                if delivered {
                    reply(id, &json!({ "ok": answer }));
                } else {
                    reply(id, &declined().to_json());
                }
                if let Some(passkey) = made {
                    settle_created(passkey.clone(), delivered);
                    if delivered {
                        let w = crate::words::words();
                        // A key the Secure Enclave didn't make is said to be elsewhere.
                        let place = if passkey.key.starts_with("keychain:") {
                            format!(
                                " (in {}: {} has no Secure Enclave)",
                                w.your_credential_store, w.this_computer
                            )
                        } else {
                            String::new()
                        };
                        self.toast(
                            format!(
                                "Passkey for {} on {} saved in Trek on {}{place}",
                                passkey.account(),
                                passkey.rp_id,
                                w.this_computer
                            ),
                            cx,
                        );
                    }
                }
            }
            // One answer for every way it can fail but the one the specification gives a page
            // (a passkey the site excluded, said only after the user's OK).
            Err(e) if e.name == "InvalidStateError" => refuse(id, e),
            Err(_) => refuse(id, &declined()),
        }
        cx.notify();
    }

    /// Tab `tab`'s page is going (a new page, or the tab closed): what it asked for is over.
    pub(super) fn passkeys_gone(&mut self, tab: u64, cx: &mut Context<Self>) {
        self.passkeys_end(|a| a.tab == tab, cx);
    }

    /// The front tab's passkey request on show: its id and what the bar says.
    pub(super) fn passkey_front(&self) -> Option<(u64, BarWords)> {
        self.passkeys
            .iter()
            .filter(|a| a.tab == self.active)
            .find_map(|a| Some((a.id, a.words()?)))
    }

    /// The front tab's passkey request, as a bar above its page: always `BAR_HEIGHT` tall.
    pub(super) fn passkey_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, words) = self.passkey_front()?;
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let several = words.yes.len() > 1;
        let yes = |ix: usize, label: &str, cx: &mut Context<Self>| {
            let button = Button::new(("browser-passkey-yes", ix))
                .xsmall()
                .label(label.to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.passkey_unsettled(id) {
                        this.passkey_yes(id, ix, cx)
                    }
                }));
            if several {
                button.outline().icon(Lucide::Key)
            } else {
                button.primary()
            }
        };
        let no = words.no.map(|label| {
            Button::new("browser-passkey-no")
                .xsmall()
                .outline()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.passkey_unsettled(id) {
                        this.passkey_no(id, cx)
                    }
                }))
        });
        // One account: its button sits by the headline. Several: a row of their own, where the
        // note would be.
        let beside = (!several)
            .then(|| words.yes.first().map(|label| yes(0, label, cx)))
            .flatten();
        let accounts = several.then(|| {
            h_flex().gap(px(6.)).flex_wrap().children(
                words
                    .yes
                    .iter()
                    .enumerate()
                    .map(|(ix, label)| yes(ix, label, cx))
                    .collect::<Vec<_>>(),
            )
        });
        Some(
            v_flex()
                .id("browser-passkey")
                .flex_none()
                .h(px(BAR_HEIGHT))
                .overflow_hidden()
                .px(px(12.))
                .py(px(7.))
                .gap(px(4.))
                .text_size(to_rems(text::META))
                .bg(ember.opacity(0.08))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .flex_none()
                        .h(px(24.))
                        .gap(px(8.))
                        .child(Icon::new(Lucide::Key).xsmall().text_color(ember))
                        .child(
                            div()
                                .id("browser-passkey-headline")
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(words.headline.clone()),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .gap(px(6.))
                                .children(no)
                                .children(beside),
                        ),
                )
                .child(
                    v_flex()
                        .id("browser-passkey-note")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .pl(px(20.))
                        .gap(px(4.))
                        .children(accounts)
                        .child(
                            div()
                                .whitespace_normal()
                                .text_color(theme.muted_foreground)
                                .child(words.note.clone()),
                        ),
                )
                .into_any_element(),
        )
    }

    /// What pages wait on for passkeys, for the capture harness's `state`.
    #[cfg(feature = "shots")]
    pub(super) fn passkeys_state(&self) -> Value {
        json!({
            "asks": self.passkeys.iter().map(|a| {
                let words = a.words();
                json!({
                    "tab": a.tab,
                    "site": a.site(),
                    "kind": if matches!(a.request, Request::Create(_)) { "create" } else { "get" },
                    "conditional": a.conditional(),
                    "stage": match &a.stage {
                        Stage::Bar => "bar",
                        Stage::Offer => "offer",
                        Stage::Quiet => "quiet",
                        Stage::Card { .. } => "card",
                        Stage::Verifying => "verifying",
                    },
                    "headline": words.as_ref().map(|w| w.headline.clone()),
                    "note": words.as_ref().map(|w| w.note.clone()),
                    "no": words.as_ref().and_then(|w| w.no),
                    "yes": words.as_ref().map(|w| w.yes.clone()),
                })
            }).collect::<Vec<_>>(),
            "bar_height": BAR_HEIGHT,
            "kept": Store::default().list().unwrap_or_default().iter().map(|p| json!({
                "site": p.rp_id,
                "account": p.account(),
                "kept_in": trek_passkeys::kept_in(p),
                "created": p.created,
                "last_used": p.last_used,
            })).collect::<Vec<_>>(),
        })
    }
}

/// A passkey just made reached its page (`delivered`) or didn't: on a thread of its own, the
/// account's older passkeys go, or the new one does (`trek_passkeys::ceremony::settle`).
fn settle_created(passkey: Passkey, delivered: bool) {
    std::thread::spawn(move || {
        if let Some(auth) = trek_passkeys::platform() {
            trek_passkeys::ceremony::settle(auth.as_ref(), &Store::default(), &passkey, delivered);
        }
    });
}

#[cfg(test)]
mod tests {
    // Named one by one: the panel's glob of GPUI brings its own `test` attribute with it.
    use super::{
        BRIDGE_JS, BarWords, DECLINED, HANDLER, PasskeyAsk, Stage, cancelled_id, core, declined,
        page_js,
    };
    use serde_json::{Value, json};
    use std::time::Instant;
    use trek_passkeys::{Origin, Passkey};

    fn ask(body: Value, origin: &str, stage: Stage) -> PasskeyAsk {
        let request = trek_passkeys::parse(&Origin::parse(origin).unwrap(), &body.to_string())
            .expect("a request");
        PasskeyAsk {
            id: 1,
            tab: 1,
            page_id: "p-1".into(),
            request,
            candidates: vec![],
            already: false,
            replaces: false,
            stage,
            since: Instant::now(),
            gone: false,
        }
    }

    fn passkey(account: &str) -> Passkey {
        Passkey {
            id: account.into(),
            rp_id: "example.com".into(),
            rp_name: "Example".into(),
            user_handle: "dQ".into(),
            user_name: account.into(),
            user_display_name: String::new(),
            public_key: String::new(),
            key: "plain:AAAA".into(),
            created: 1,
            last_used: None,
            extra: Default::default(),
        }
    }

    fn create() -> Value {
        json!({ "kind": "create", "id": "p-1", "publicKey": {
            "rp": { "name": "Example" },
            "user": { "id": "dQ", "name": "ada@example.com", "displayName": "Ada" },
            "challenge": "AAAA",
            "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
        }})
    }

    fn get(conditional: bool) -> Value {
        json!({ "kind": "get", "id": "p-2", "mediation": if conditional { "conditional" } else { "optional" },
            "publicKey": { "challenge": "AAAA", "rpId": "example.com" } })
    }

    fn buttons(words: &BarWords) -> (Option<&'static str>, Vec<&str>) {
        (words.no, words.yes.iter().map(String::as_str).collect())
    }

    #[test]
    fn the_bar_says_where_a_new_passkey_is_kept() {
        let mut new = ask(create(), "https://login.example.com", Stage::Bar);
        let words = new.words().unwrap();
        // The site first, and it is the page's own host here (it named none): what the passkey
        // is bound to. The account's name, which the site wrote, comes after it, in quotes.
        assert_eq!(
            words.headline,
            "On login.example.com: create a passkey for “ada@example.com”?"
        );
        assert!(words.note.contains("on this Mac"), "{}", words.note);
        assert!(
            words.note.contains("not saved to iCloud Keychain"),
            "{}",
            words.note
        );
        assert!(!words.note.contains("in place of"));
        assert_eq!(buttons(&words), (Some("Cancel"), vec!["Continue"]));
        // One Trek keeps for the account already: said before it's replaced.
        new.replaces = true;
        assert!(
            new.words()
                .unwrap()
                .note
                .contains("in place of the passkey")
        );

        // One the site excludes: OK tells the page so (as the specification has it), Cancel
        // tells it nothing.
        new.already = true;
        let words = new.words().unwrap();
        assert!(
            words
                .headline
                .ends_with("Trek already has a passkey for this account"),
            "{}",
            words.headline
        );
        assert_eq!(buttons(&words), (Some("Cancel"), vec!["OK"]));
    }

    #[test]
    fn a_sign_in_names_the_account_or_asks_which() {
        let mut one = ask(get(false), "https://www.example.com", Stage::Bar);
        // None kept: the bar says so, and how to get one.
        let words = one.words().unwrap();
        assert_eq!(
            words.headline,
            "On example.com: Trek has no passkey for this site"
        );
        assert!(words.note.contains("add a passkey"));
        assert_eq!(buttons(&words), (None, vec!["OK"]));

        one.candidates = vec![passkey("ada")];
        let words = one.words().unwrap();
        assert_eq!(words.headline, "On example.com: sign in as “ada”?");
        assert_eq!(buttons(&words), (Some("Cancel"), vec!["Continue"]));

        one.candidates.push(passkey("bob"));
        let words = one.words().unwrap();
        assert_eq!(buttons(&words), (Some("Cancel"), vec!["ada", "bob"]));
    }

    #[test]
    fn autofills_request_shows_nothing_until_the_toolbars_key_is_pressed() {
        // With a passkey to offer or without, nothing is on the bar: the page's view is the
        // same size either way, so the page can't tell which.
        let mut quiet = ask(get(true), "https://www.example.com", Stage::Quiet);
        assert_eq!(quiet.words(), None);
        quiet.candidates = vec![passkey("ada")];
        quiet.stage = Stage::Offer;
        assert_eq!(quiet.words(), None);
        // Opened by the user: put off, not refused.
        quiet.stage = Stage::Bar;
        let words = quiet.words().unwrap();
        assert_eq!(buttons(&words), (Some("Not Now"), vec!["Sign In"]));
    }

    #[test]
    fn every_bar_has_a_note_so_none_is_shorter_for_what_trek_keeps() {
        // The bar is one height (`BAR_HEIGHT`); each state fills it the same way, a headline
        // and a note, whether there is a passkey, none, one excluded, or several.
        let mut states = vec![
            ask(create(), "https://example.com", Stage::Bar),
            ask(get(false), "https://example.com", Stage::Bar),
            ask(get(false), "https://example.com", Stage::Verifying),
            ask(create(), "https://example.com", Stage::Verifying),
            ask(
                get(false),
                "https://example.com",
                Stage::Card {
                    thread: "t".into(),
                    request: "r".into(),
                },
            ),
        ];
        let mut with_one = ask(get(false), "https://example.com", Stage::Bar);
        with_one.candidates = vec![passkey("ada")];
        let mut with_two = ask(get(false), "https://example.com", Stage::Bar);
        with_two.candidates = vec![passkey("ada"), passkey("bob")];
        let mut excluded = ask(create(), "https://example.com", Stage::Bar);
        excluded.already = true;
        states.extend([with_one, with_two, excluded]);
        for state in &states {
            let words = state.words().unwrap();
            assert!(!words.note.trim().is_empty(), "{}", words.headline);
            assert!(
                words.headline.starts_with("On example.com: "),
                "{}",
                words.headline
            );
        }
    }

    #[test]
    fn a_page_is_told_one_thing_however_a_request_fails() {
        assert_eq!(declined().name, "NotAllowedError");
        assert_eq!(declined().message, DECLINED);
        assert!(!DECLINED.to_lowercase().contains("touch id"));
    }

    #[test]
    fn an_agents_request_is_worded_for_its_threads_card_and_always_asked() {
        let mut one = ask(get(false), "https://www.example.com", Stage::Bar);
        one.candidates = vec![passkey("ada")];
        let card = one.card("https://www.example.com");
        assert_eq!(card.kind, core::Sensitive::Passkey);
        assert_eq!(
            card.title,
            "Sign in to example.com with your passkey for “ada”"
        );
        assert!(card.detail.contains("https://www.example.com"));
        assert!(card.detail.contains("Touch ID"));
        for access in [
            trek_core::HandHolding::Supervised,
            trek_core::HandHolding::AutoAcceptEdits,
            trek_core::HandHolding::Auto,
            trek_core::HandHolding::FullAccess,
        ] {
            assert!(core::needs_asking(&card.kind, access), "{access:?}");
        }
        let card = ask(create(), "https://example.com", Stage::Bar).card("https://example.com");
        assert_eq!(
            card.title,
            "Create a passkey on example.com for “ada@example.com”"
        );
        // While it waits there, the bar only says so (and can call it off).
        one.stage = Stage::Card {
            thread: "t".into(),
            request: "r".into(),
        };
        let words = one.words().unwrap();
        assert_eq!(buttons(&words), (Some("Cancel"), vec![]));
    }

    #[test]
    fn a_page_takes_a_request_back_by_its_own_id() {
        assert_eq!(
            cancelled_id(&json!({ "kind": "cancel", "of": "p-1", "id": "p-1-x" })),
            Some("p-1")
        );
        assert_eq!(cancelled_id(&json!({ "kind": "get", "of": "p-1" })), None);
        assert_eq!(cancelled_id(&json!({ "kind": "cancel" })), None);
        assert_eq!(cancelled_id(&Value::Null), None);
    }

    /// The two scripts against a stand-in document and handler (`fixtures/passkeys_page_test.js`),
    /// where there is a Node new enough to have `CustomEvent` (19).
    #[test]
    fn the_page_and_the_bridge_work_together() {
        let node = |args: &[&str]| std::process::Command::new("node").args(args).output();
        let version = node(&["--version"])
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .and_then(|v| {
                v.trim_start_matches('v')
                    .split('.')
                    .next()?
                    .parse::<u32>()
                    .ok()
            });
        if version.is_none_or(|major| major < 19) {
            eprintln!("skipped: no Node 19 or later here ({version:?})");
            return;
        }
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/panels/browser/fixtures/passkeys_page_test.js"
        );
        let out = node(&[script]).expect("node runs");
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_pages_script_is_whole() {
        let js = page_js();
        assert!(!js.contains("__TREK_"), "every placeholder is filled in");
        assert!(js.contains("const CONDITIONAL = true;"));
        // The bridge names the handler the native side registers, and nothing else of Trek's.
        assert!(BRIDGE_JS.contains(&format!("messageHandlers.{HANDLER}")));
        assert!(
            !js.contains("messageHandlers"),
            "the page's side can't reach the handler"
        );
    }
}
