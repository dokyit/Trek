//! UI Automation through COM (`IUIAutomation`): a window's elements read for `uia`'s outline, and
//! their patterns' own actions. Every call is made on one thread of its own, in the multithreaded
//! apartment UI Automation wants its clients in (trek-mcp's main thread is single-threaded, for
//! `ShellExecuteW`), and never on a thread that has windows. That thread keeps what's been read
//! to act on; the `Desktop` hands it each request and waits for the answer, for a bounded time
//! (an app that's hung costs that, and the thread is let go and started afresh).
//!
//! Each app has `TIMEOUT_MS` to answer one request (UI Automation's own connection and
//! transaction timeouts); one that doesn't is told apart from one that's gone (`uia::fault`).
//!
//! A classic app's menu bar is read through the Win32 menu calls instead (`menus_of`), and its
//! items pressed with the command their menu would send (`choose`): no menu is opened.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Variant::{
    VARIANT, VT_ARRAY, VT_BOOL, VT_BSTR, VT_I4, VT_INT, VT_R4, VT_R8, VariantClear,
    VariantGetElementCount, VariantToInt32Array,
};
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_Full, CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
    IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationElement3,
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern, IUIAutomationRangeValuePattern,
    IUIAutomationScrollItemPattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, TreeScope, TreeScope_Children, TreeScope_Element,
    UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_ExpandCollapsePatternId,
    UIA_FrameworkIdPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_HeadingLevelPropertyId,
    UIA_HelpTextPropertyId, UIA_InvokePatternId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsPasswordPropertyId, UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_IsScrollItemPatternAvailablePropertyId, UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId,
    UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId, UIA_PROPERTY_ID,
    UIA_ProcessIdPropertyId, UIA_RangeValueIsReadOnlyPropertyId, UIA_RangeValueMaximumPropertyId,
    UIA_RangeValueMinimumPropertyId, UIA_RangeValuePatternId, UIA_RangeValueSmallChangePropertyId,
    UIA_RangeValueValuePropertyId, UIA_RuntimeIdPropertyId, UIA_ScrollItemPatternId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId,
    UIA_ValueValuePropertyId,
};
use windows::core::{BSTR, Interface};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED,
    MFT_OWNERDRAW, MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU,
    PostMessageW, WM_COMMAND,
};

use super::{Fault, MenuItem, Props, Range, Step, Tree};
use crate::desktop::{Elements, Performed, Reading, WindowInfo};
use crate::outline::{Action, Node};

/// How long an app has to answer one request, in milliseconds (as the Mac's 2 s).
const TIMEOUT_MS: u32 = 2000;
/// How long a request may take on UI Automation's thread, beyond a reading's own time.
const ANSWER: Duration = Duration::from_secs(15);
/// How many elements are kept to act on, and how many runtime ids are remembered, before they're
/// let go (their refs then need a fresh reading).
const KEPT_MAX: usize = 5_000;
const IDS_MAX: usize = 50_000;
/// A Chromium app's first reading, with its page not built yet: how long it gets to build it,
/// and how often it's looked at meanwhile (the Mac's waits).
const WAKING: Duration = Duration::from_millis(2500);
const WAKING_POLL: Duration = Duration::from_millis(250);

/// Said for a window or element of Trek's own.
const OWN: &str = "That's one of Trek's own windows: agents don't operate Trek itself, so its elements aren't read or acted on.";

/// What's read of each element, in the round trip that finds it.
const PROPERTIES: [UIA_PROPERTY_ID; 29] = [
    UIA_ControlTypePropertyId,
    UIA_LocalizedControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_HelpTextPropertyId,
    UIA_AutomationIdPropertyId,
    UIA_FrameworkIdPropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_ProcessIdPropertyId,
    UIA_RuntimeIdPropertyId,
    UIA_HeadingLevelPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_RangeValueIsReadOnlyPropertyId,
    UIA_RangeValueMinimumPropertyId,
    UIA_RangeValueMaximumPropertyId,
    UIA_RangeValueSmallChangePropertyId,
];

// ---- what the Desktop asks ----

/// `Desktop::elements`: the window's tree, read as far as `reading` allows.
pub fn elements(w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
    let (w, reading) = (w.clone(), *reading);
    run(reading.within + WAKING + ANSWER, move |s| {
        s.elements(&w, &reading)
    })
}

/// `Desktop::element_at`: the element at `place` as it is now, without its children.
pub fn element_at(w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
    let (w, place) = (w.clone(), place.to_vec());
    run(ANSWER, move |s| s.element_at(&w, &place))
}

/// `Desktop::perform`: the element's own pattern does `action`.
pub fn perform(w: &WindowInfo, identity: u64, action: Action) -> Result<Performed, String> {
    let w = w.clone();
    run(ANSWER, move |s| s.perform(&w, identity, action))
}

/// `Desktop::focus`: the element is given the keyboard focus; whether it says it has it.
pub fn focus(w: &WindowInfo, identity: u64) -> Result<bool, String> {
    let w = w.clone();
    run(ANSWER, move |s| s.focus(&w, identity))
}

/// `Desktop::add_text`: `text` put after what the element's Value holds.
pub fn add_text(w: &WindowInfo, identity: u64, text: &str) -> Result<(), String> {
    let (w, text) = (w.clone(), text.to_string());
    run(ANSWER, move |s| s.add_text(&w, identity, &text))
}

/// `Desktop::reveal`: the element is asked to scroll itself into view.
pub fn reveal(w: &WindowInfo, identity: u64) -> Result<bool, String> {
    let w = w.clone();
    run(ANSWER, move |s| s.reveal(&w, identity))
}

// ---- UI Automation's thread ----

type Job = Box<dyn FnOnce(&mut Result<Session, String>) + Send>;

/// UI Automation's thread, by the requests it takes.
struct Worker {
    jobs: mpsc::Sender<Job>,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

impl Worker {
    fn start() -> Result<Self, String> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("trek-mcp ui automation".into())
            .spawn(move || {
                // SAFETY: COM is started once on this thread of its own, before anything of it
                // is made here, and stopped after all of it is let go (`session` dropped).
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let mut session = if com.is_ok() {
                    Session::new()
                } else {
                    Err(format!(
                        "UI Automation couldn't be started (COM: {}).",
                        windows::core::Error::from(com).message()
                    ))
                };
                while let Ok(job) = inbox.recv() {
                    job(&mut session);
                }
                drop(session);
                if com.is_ok() {
                    // SAFETY: matches the `CoInitializeEx` above, on the same thread.
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|e| format!("UI Automation's thread couldn't be started ({e})."))?;
        Ok(Self { jobs })
    }
}

/// Do `job` on UI Automation's thread and wait for its answer, at most `limit`.
fn run<R: Send + 'static>(
    limit: Duration,
    job: impl FnOnce(&mut Session) -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    let (tx, rx) = mpsc::channel();
    let job: Job = Box::new(move |session| {
        let answer = match session {
            Ok(session) => job(session),
            Err(e) => Err(e.clone()),
        };
        let _ = tx.send(answer);
    });
    {
        let mut worker = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
        let unsent = match worker.as_ref() {
            Some(w) => w.jobs.send(job).err().map(|mpsc::SendError(job)| job),
            None => Some(job),
        };
        // None yet, or the one there was has stopped: another is started.
        if let Some(job) = unsent {
            let fresh = Worker::start()?;
            if fresh.jobs.send(job).is_err() {
                return Err("UI Automation's thread isn't running. Try again.".into());
            }
            *worker = Some(fresh);
        }
    }
    let gone = || *WORKER.lock().unwrap_or_else(PoisonError::into_inner) = None;
    match rx.recv_timeout(limit) {
        Ok(answer) => answer,
        // Stuck on an app that doesn't answer: that thread is let go (it ends once it's free),
        // and the next request starts another.
        Err(mpsc::RecvTimeoutError::Timeout) => {
            gone();
            Err("UI Automation didn't answer in time (an app may be hung), so nothing more was done. Try again, or work from a screenshot, with x and y.".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            gone();
            Err("UI Automation stopped while it was asked. Try again.".into())
        }
    }
}

// ---- the session: UI Automation, and what's kept to act on ----

/// What an element kept to act on is.
#[derive(Clone)]
enum Held {
    /// A UI Automation element.
    Element(IUIAutomationElement),
    /// A classic app's menu item (`uia::menu_bar`), by its window and command.
    Command { hwnd: isize, id: u32 },
}

/// An element kept to act on: the object, and what it was when last read.
struct Kept {
    held: Held,
    props: Props,
}

struct Session {
    uia: IUIAutomation,
    /// What's read of the element asked for, and of an element's children.
    element: IUIAutomationCacheRequest,
    children: IUIAutomationCacheRequest,
    /// Elements kept to act on, by their window and number: a window's are what its latest
    /// reading found (and what's been looked up in it since), whatever other windows are read.
    kept: HashMap<(i64, u64), Kept>,
    /// The window being read, whose elements `Tree::identity` keeps.
    window: i64,
    /// Elements' numbers by runtime id: the same object, read again, gets the same one.
    ids: HashMap<Vec<i32>, u64>,
    next: u64,
    /// Apps whose windows have been read before, by pid.
    seen: HashSet<u32>,
    /// Whether a process is Trek, by pid.
    treks: HashMap<u32, bool>,
}

fn fault_of(e: &windows::core::Error) -> Fault {
    super::fault(e.code().0 as u32)
}

impl Session {
    fn new() -> Result<Self, String> {
        let failed = |e: windows::core::Error| {
            format!("UI Automation couldn't be started ({}).", e.message())
        };
        // SAFETY: COM is started on this thread (`Worker::start`); everything made here stays
        // on it.
        unsafe {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                .map_err(failed)?;
            if let Ok(timed) = uia.cast::<IUIAutomation2>() {
                let _ = timed.SetConnectionTimeout(TIMEOUT_MS);
                let _ = timed.SetTransactionTimeout(TIMEOUT_MS);
            }
            let request = |scope: TreeScope| -> windows::core::Result<IUIAutomationCacheRequest> {
                let r = uia.CreateCacheRequest()?;
                for p in PROPERTIES {
                    // A Windows too old for one (the heading level is 1803's) does without it.
                    let _ = r.AddProperty(p);
                }
                r.SetTreeScope(scope)?;
                r.SetTreeFilter(&uia.ControlViewCondition()?)?;
                r.SetAutomationElementMode(AutomationElementMode_Full)?;
                Ok(r)
            };
            let element = request(TreeScope_Element).map_err(failed)?;
            let children = request(TreeScope_Children).map_err(failed)?;
            Ok(Self {
                uia,
                element,
                children,
                kept: HashMap::new(),
                window: 0,
                ids: HashMap::new(),
                next: 0,
                seen: HashSet::new(),
                treks: HashMap::new(),
            })
        }
    }

    fn is_trek(&mut self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        if self.treks.len() > 1000 {
            self.treks.clear();
        }
        *self.treks.entry(pid).or_insert_with(|| {
            // As computer use tells Trek's windows: its executable's name.
            crate::desktop::windows::process_name(pid).eq_ignore_ascii_case("trek")
        })
    }

    /// Trek's own windows are off limits, whatever asks.
    fn refuse_trek(&mut self, w: &WindowInfo) -> Result<(), String> {
        if w.owner.eq_ignore_ascii_case("trek") || self.is_trek(w.pid as u32) {
            return Err(OWN.into());
        }
        Ok(())
    }

    /// `w`'s own element, with its properties.
    fn root(&self, w: &WindowInfo) -> windows::core::Result<IUIAutomationElement> {
        let hwnd = HWND(w.id as isize as *mut c_void);
        // SAFETY: a window's handle (UI Automation checks it: a closed window's fails) and this
        // thread's cache request.
        unsafe { self.uia.ElementFromHandleBuildCache(hwnd, &self.element) }
    }

    /// An element of `w`'s kept to act on, as last read.
    fn kept(&mut self, w: &WindowInfo, identity: u64) -> Result<(Held, Props), String> {
        let (held, props) = self
            .kept
            .get(&(w.id, identity))
            .map(|k| (k.held.clone(), k.props.clone()))
            .ok_or_else(|| {
                "That element isn't kept any more. Read the elements again and use the ref it has now."
                    .to_string()
            })?;
        if self.is_trek(props.pid) {
            return Err(OWN.into());
        }
        Ok((held, props))
    }

    /// The number an element is known by, from what tells it apart (its runtime id): the one it
    /// had when read before.
    fn number(&mut self, key: Vec<i32>) -> u64 {
        if self.ids.len() >= IDS_MAX {
            self.ids.clear();
        }
        let next = &mut self.next;
        *self.ids.entry(key).or_insert_with(|| {
            *next += 1;
            *next
        })
    }

    /// Keep what's known as `id` in the window being read, to act on.
    fn keep(&mut self, id: u64, held: Held, props: Props) {
        let at = (self.window, id);
        if self.kept.len() >= KEPT_MAX && !self.kept.contains_key(&at) {
            self.kept.clear();
        }
        self.kept.insert(at, Kept { held, props });
    }

    /// The number a menu item of `w`'s that does something is known by, kept to press: by its
    /// window, its command and its words.
    fn command(&mut self, w: &WindowInfo, item: &MenuItem) -> u64 {
        let name = super::menu_words(&item.text);
        let mut key = vec![
            i32::MIN + 1,
            w.id as i32,
            (w.id >> 32) as i32,
            item.id as i32,
        ];
        key.extend(name.chars().take(32).map(|c| c as i32));
        let id = self.number(key);
        let props = Props {
            kind: super::kind::MENU_ITEM,
            name,
            enabled: !item.disabled,
            invoke: true,
            pid: w.pid as u32,
            ..Props::default()
        };
        self.keep(
            id,
            Held::Command {
                hwnd: w.id as isize,
                id: item.id,
            },
            props,
        );
        id
    }

    fn elements(&mut self, w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
        self.refuse_trek(w)?;
        // What's kept of the window to act on is what this reading finds (and what's looked up
        // in it since).
        self.window = w.id;
        self.kept.retain(|&(window, _), _| window != w.id);
        let first = self.seen.insert(w.pid as u32);
        let started = Instant::now();
        loop {
            let root = self.root(w).map_err(|e| window_error(w, &e))?;
            let p = props(&root);
            let read = super::read(self, root, p, (w.x, w.y), reading);
            // A Chromium app asked for the first time builds its page a moment later.
            if first && read.unbuilt && started.elapsed() < WAKING {
                std::thread::sleep(WAKING_POLL);
                continue;
            }
            if read.roots.is_empty() {
                return Err(format!(
                    "{}'s window didn't answer when asked for its elements. Work from a screenshot, with x and y.",
                    w.owner
                ));
            }
            let mut roots = read.roots;
            // A classic app's menus, as the Mac's menu bar: read whole, with nothing opened.
            let menus = if reading.menu_bar {
                menus_of(w)
            } else {
                vec![]
            };
            if !menus.is_empty() {
                let bar = super::menu_bar(&menus, reading.max_depth, &mut |item, _| {
                    self.command(w, item)
                });
                roots.push(bar);
            }
            return Ok(Elements {
                roots,
                cut: read.cut,
            });
        }
    }

    fn element_at(&mut self, w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
        self.refuse_trek(w)?;
        self.window = w.id;
        if place.first() == Some(&1) {
            let menus = menus_of(w);
            return Ok(super::menu_item_at(&menus, place).map(|item| {
                let id = item.submenu.is_none().then(|| self.command(w, item));
                super::menu_node(item, place, id)
            }));
        }
        // An app that doesn't answer in time has said nothing about what's there: that's no
        // proof the element changed.
        let slow = || {
            format!(
                "{} didn't answer in time when asked about that element, so nothing was done. Try again in a moment.",
                w.owner
            )
        };
        let root = match self.root(w) {
            Ok(root) => root,
            Err(e) => {
                return match fault_of(&e) {
                    Fault::Gone => Ok(None),
                    Fault::Slow => Err(slow()),
                    _ => Err(window_error(w, &e)),
                };
            }
        };
        let p = props(&root);
        match super::find(self, root, p, place) {
            Ok(Some((el, p, parent))) => {
                let identity = super::acts(&p).then(|| self.identity(&el, &p, place));
                Ok(Some(super::node(&p, parent, (w.x, w.y), place, identity)))
            }
            Ok(None) | Err(Fault::Gone) => Ok(None),
            Err(Fault::Slow) => Err(slow()),
            Err(Fault::Denied) => Err(super::DENIED.into()),
            Err(f) => Err(format!(
                "UI Automation couldn't read that element ({f:?}), so nothing was done."
            )),
        }
    }

    fn perform(
        &mut self,
        w: &WindowInfo,
        identity: u64,
        action: Action,
    ) -> Result<Performed, String> {
        self.refuse_trek(w)?;
        let (held, p) = self.kept(w, identity)?;
        let step =
            super::plan(&p, action).ok_or_else(|| super::refused(Fault::Unsupported, action))?;
        let el = match held {
            Held::Element(el) => el,
            Held::Command { hwnd, id } => return choose(hwnd, id),
        };
        // SAFETY: a live element of this thread's; each pattern is asked for as the interface it
        // is, and a missing one comes back as an error.
        let done = unsafe {
            match step {
                Step::Invoke => el
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .and_then(|x| x.Invoke()),
                Step::Toggle => el
                    .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    .and_then(|x| x.Toggle()),
                Step::Select => el
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                    .and_then(|x| x.Select()),
                Step::Expand => el
                    .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )
                    .and_then(|x| x.Expand()),
                Step::Collapse => el
                    .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )
                    .and_then(|x| x.Collapse()),
                Step::ContextMenu => el
                    .cast::<IUIAutomationElement3>()
                    .and_then(|x| x.ShowContextMenu()),
                Step::Change(by) => el
                    .GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId)
                    .and_then(|x| {
                        let now = x.CurrentValue()?;
                        let min = x.CurrentMinimum().unwrap_or(f64::NAN);
                        let max = x.CurrentMaximum().unwrap_or(f64::NAN);
                        x.SetValue(within(now + by, min, max))
                    }),
            }
        };
        match done {
            Ok(()) => Ok(Performed::Done),
            Err(e) => match fault_of(&e) {
                // Not an error: it may have been done (an action that opens a dialog often ends
                // this way).
                Fault::Slow => Ok(Performed::Unknown("its app didn't answer in time".into())),
                f => Err(super::refused(f, action)),
            },
        }
    }

    fn focus(&mut self, w: &WindowInfo, identity: u64) -> Result<bool, String> {
        self.refuse_trek(w)?;
        let Held::Element(el) = self.kept(w, identity)?.0 else {
            return Ok(false);
        };
        // SAFETY: a live element of this thread's.
        if let Err(e) = unsafe { el.SetFocus() }
            && fault_of(&e) == Fault::Gone
        {
            return Err("It's gone from its window.".into());
        }
        // An app that takes the request says so at once or a moment later (a web page's field).
        for wait in [0u64, 60, 120, 200] {
            std::thread::sleep(Duration::from_millis(wait));
            // SAFETY: as above.
            if unsafe { el.CurrentHasKeyboardFocus() }.is_ok_and(|f| f.as_bool()) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn add_text(&mut self, w: &WindowInfo, identity: u64, text: &str) -> Result<(), String> {
        self.refuse_trek(w)?;
        let (held, p) = self.kept(w, identity)?;
        if p.password {
            return Err(NO_PASSWORD.into());
        }
        let Held::Element(el) = held else {
            return Err("It has no value of its own to add to.".into());
        };
        // SAFETY: a live element of this thread's and its Value pattern.
        unsafe {
            let value = el
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .map_err(not_added)?;
            if value.CurrentIsReadOnly().is_ok_and(|r| r.as_bool()) {
                return Err("It's read-only.".into());
            }
            let now = value.CurrentValue().map_err(not_added)?.to_string();
            value
                .SetValue(&BSTR::from(format!("{now}{text}").as_str()))
                .map_err(not_added)
        }
    }

    fn reveal(&mut self, w: &WindowInfo, identity: u64) -> Result<bool, String> {
        self.refuse_trek(w)?;
        let (held, p) = self.kept(w, identity)?;
        let (Held::Element(el), true) = (held, p.scroll_item) else {
            return Ok(false);
        };
        // SAFETY: a live element of this thread's and its ScrollItem pattern.
        let done = unsafe {
            el.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
                .and_then(|x| x.ScrollIntoView())
        };
        match done {
            Ok(()) => Ok(true),
            Err(e) if fault_of(&e) == Fault::Gone => Err("It's gone from its window.".into()),
            Err(_) => Ok(false),
        }
    }
}

impl Tree for Session {
    type El = IUIAutomationElement;

    fn children(
        &mut self,
        el: &IUIAutomationElement,
    ) -> Result<Vec<(IUIAutomationElement, Props)>, Fault> {
        // SAFETY: a live element and this thread's cache request: one round trip, and the
        // children's properties are read from what it brought back.
        unsafe {
            let updated = el
                .BuildUpdatedCache(&self.children)
                .map_err(|e| fault_of(&e))?;
            // None at all comes back as no array.
            let Ok(all) = updated.GetCachedChildren() else {
                return Ok(vec![]);
            };
            let n = all.Length().unwrap_or(0);
            // In order, so each keeps its index: the place `find` walks again.
            Ok((0..n)
                .map_while(|i| all.GetElement(i).ok())
                .map(|child| {
                    let p = props(&child);
                    (child, p)
                })
                .collect())
        }
    }

    fn identity(&mut self, el: &IUIAutomationElement, p: &Props, place: &[usize]) -> u64 {
        // One with no runtime id (rare) goes by what it is and where.
        let key = if p.runtime_id.is_empty() {
            let mut key = vec![i32::MIN, p.pid as i32, p.kind];
            key.extend(place.iter().map(|&i| i as i32));
            key.extend(p.name.chars().take(32).map(|c| c as i32));
            key
        } else {
            p.runtime_id.clone()
        };
        let id = self.number(key);
        self.keep(id, Held::Element(el.clone()), p.clone());
        id
    }

    fn off_limits(&mut self, p: &Props) -> bool {
        self.is_trek(p.pid)
    }
}

/// The most of a classic app's menu that's read (items in all, and submenus deep).
const MENU_ITEMS_MAX: usize = 2000;
const MENU_DEPTH: usize = 8;

/// Window `w`'s Win32 menu bar (a classic app's), read whole with nothing opened; empty when it
/// has none (most apps today draw their own).
fn menus_of(w: &WindowInfo) -> Vec<MenuItem> {
    // SAFETY: a window's handle, which Windows checks (a closed window has no menu).
    let menu = unsafe { GetMenu(w.id as isize as *mut c_void) };
    if menu.is_null() {
        return vec![];
    }
    let mut left = MENU_ITEMS_MAX;
    read_menu(menu, 0, &mut left)
}

fn read_menu(menu: HMENU, depth: usize, left: &mut usize) -> Vec<MenuItem> {
    // SAFETY: a menu's handle (Windows checks it), read item by item into a buffer as long as
    // each says its words are.
    unsafe {
        let mut items = vec![];
        for i in 0..GetMenuItemCount(menu).max(0) as u32 {
            if *left == 0 {
                break;
            }
            *left -= 1;
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING,
                ..Default::default()
            };
            if GetMenuItemInfoW(menu, i, 1, &mut info) == 0 {
                // Unreadable: it says nothing, and keeps its place.
                items.push(MenuItem {
                    separator: true,
                    ..MenuItem::default()
                });
                continue;
            }
            let mut words = vec![0u16; info.cch as usize + 1];
            if info.cch > 0 && info.fType & MFT_OWNERDRAW == 0 {
                info.dwTypeData = words.as_mut_ptr();
                info.cch += 1;
                if GetMenuItemInfoW(menu, i, 1, &mut info) == 0 {
                    info.cch = 0;
                }
            } else {
                info.cch = 0;
            }
            // An item that opens a submenu is never taken for a command, however deep.
            let submenu = (!info.hSubMenu.is_null()).then(|| {
                if depth < MENU_DEPTH {
                    read_menu(info.hSubMenu, depth + 1, left)
                } else {
                    vec![]
                }
            });
            items.push(MenuItem {
                text: String::from_utf16_lossy(&words[..(info.cch as usize).min(words.len())]),
                id: info.wID,
                separator: info.fType & MFT_SEPARATOR != 0,
                disabled: info.fState & MFS_DISABLED != 0,
                checked: info.fState & MFS_CHECKED != 0,
                submenu,
            });
        }
        items
    }
}

/// A menu command chosen as its menu would choose it: `WM_COMMAND`, posted to the window whose
/// menu it is. No menu opens, and no pointer or key is sent.
fn choose(hwnd: isize, id: u32) -> Result<Performed, String> {
    // SAFETY: a window's handle (a closed window's fails) and a message that carries no pointer.
    let posted =
        unsafe { PostMessageW(hwnd as *mut c_void, WM_COMMAND, (id & 0xFFFF) as usize, 0) };
    if posted != 0 {
        return Ok(Performed::Done);
    }
    let e = std::io::Error::last_os_error();
    Err(match e.raw_os_error() {
        // ERROR_ACCESS_DENIED: the app runs above trek-mcp's level.
        Some(5) => super::DENIED.into(),
        // ERROR_INVALID_WINDOW_HANDLE.
        Some(1400) => "It's gone from its window.".into(),
        _ => format!("Windows didn't take it ({e})."),
    })
}

/// Said when text isn't added to a password field's value.
const NO_PASSWORD: &str =
    "It's a password field: what it holds isn't read, so nothing can be added to it this way.";

/// Why text wasn't added to an element's value.
fn not_added(e: windows::core::Error) -> String {
    match fault_of(&e) {
        Fault::Unsupported => "It has no value of its own to add to.".into(),
        Fault::Slow => "Its app didn't answer in time, so whether the text reached it isn't known: read the elements before trying again.".into(),
        f => super::refused(f, Action::Press),
    }
}

/// `v` kept within `min` and `max`, where they make a range.
fn within(v: f64, min: f64, max: f64) -> f64 {
    if min <= max { v.clamp(min, max) } else { v }
}

/// Said when a window's elements couldn't be read at all.
fn window_error(w: &WindowInfo, e: &windows::core::Error) -> String {
    match fault_of(e) {
        Fault::Denied => format!(
            "{} runs as administrator (or at a higher level than trek-mcp), so Windows doesn't let trek-mcp read its window's elements. Clicks and keys can't reach it either.",
            w.owner
        ),
        Fault::Gone => format!(
            "{}'s window is gone (it closed since it was found). Look again: list_windows, or a screenshot.",
            w.owner
        ),
        Fault::Slow => format!(
            "{} didn't answer when asked for its elements (it may be busy). Work from a screenshot, with x and y.",
            w.owner
        ),
        _ => format!(
            "{}'s window didn't answer when asked for its elements ({}). Work from a screenshot, with x and y.",
            w.owner,
            e.message()
        ),
    }
}

/// A VARIANT an element's cache gave, cleared when it's dropped.
struct Var(VARIANT);

impl Drop for Var {
    fn drop(&mut self) {
        // SAFETY: a VARIANT this owns, cleared once.
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}

impl Var {
    fn cached(el: &IUIAutomationElement, id: UIA_PROPERTY_ID) -> Option<Self> {
        // SAFETY: a live element; the VARIANT it fills is this one's from here.
        unsafe { el.GetCachedPropertyValue(id) }.ok().map(Var)
    }

    fn kind(&self) -> u16 {
        // SAFETY: `vt` is set in every VARIANT.
        unsafe { self.0.Anonymous.Anonymous.vt.0 }
    }

    fn flag(&self) -> Option<bool> {
        // SAFETY (here and below): the union's member that `vt` says it holds.
        (self.kind() == VT_BOOL.0)
            .then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.boolVal.0 != 0 })
    }

    fn int(&self) -> Option<i32> {
        let k = self.kind();
        (k == VT_I4.0 || k == VT_INT.0)
            .then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.lVal })
    }

    fn number(&self) -> Option<f64> {
        match self.kind() {
            k if k == VT_R8.0 => Some(unsafe { self.0.Anonymous.Anonymous.Anonymous.dblVal }),
            k if k == VT_R4.0 => {
                Some(unsafe { self.0.Anonymous.Anonymous.Anonymous.fltVal } as f64)
            }
            _ => self.int().map(f64::from),
        }
    }

    fn text(&self) -> Option<String> {
        (self.kind() == VT_BSTR.0)
            .then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string() })
    }

    /// A runtime id: an array of ints.
    fn ints(&self) -> Option<Vec<i32>> {
        if self.kind() != VT_ARRAY.0 | VT_I4.0 {
            return None;
        }
        // SAFETY: an array VARIANT, read into a buffer as long as it says it is.
        unsafe {
            let n = VariantGetElementCount(&self.0) as usize;
            let mut out = vec![0i32; n];
            let mut got = 0u32;
            VariantToInt32Array(&self.0, &mut out, &mut got).ok()?;
            out.truncate(got as usize);
            Some(out)
        }
    }
}

/// An element's properties, from what its round trip brought back (no further trip).
fn props(el: &IUIAutomationElement) -> Props {
    let var = |id| Var::cached(el, id);
    let has = |id| var(id).and_then(|v| v.flag()).unwrap_or(false);
    let number = |id| var(id).and_then(|v| v.number());
    let text = |got: windows::core::Result<BSTR>| got.map(|b| b.to_string()).unwrap_or_default();
    // SAFETY: a live element, read from its cache.
    unsafe {
        let password = el.CachedIsPassword().is_ok_and(|b| b.as_bool());
        let rect = el
            .CachedBoundingRectangle()
            .ok()
            .filter(|r| r.right > r.left || r.bottom > r.top)
            .map(|r| {
                (
                    r.left as f64,
                    r.top as f64,
                    (r.right - r.left) as f64,
                    (r.bottom - r.top) as f64,
                )
            });
        let range = has(UIA_IsRangeValuePatternAvailablePropertyId).then(|| Range {
            value: number(UIA_RangeValueValuePropertyId).unwrap_or(0.0),
            min: number(UIA_RangeValueMinimumPropertyId).unwrap_or(f64::NAN),
            max: number(UIA_RangeValueMaximumPropertyId).unwrap_or(f64::NAN),
            small: number(UIA_RangeValueSmallChangePropertyId).unwrap_or(0.0),
            read_only: has(UIA_RangeValueIsReadOnlyPropertyId),
        });
        Props {
            kind: el.CachedControlType().map_or(0, |t| t.0),
            kind_words: text(el.CachedLocalizedControlType()),
            name: text(el.CachedName()),
            help: text(el.CachedHelpText()),
            automation_id: text(el.CachedAutomationId()),
            framework: text(el.CachedFrameworkId()),
            rect,
            enabled: el.CachedIsEnabled().map_or(true, |b| b.as_bool()),
            focused: el.CachedHasKeyboardFocus().is_ok_and(|b| b.as_bool()),
            password,
            pid: el.CachedProcessId().map_or(0, |p| p as u32),
            runtime_id: var(UIA_RuntimeIdPropertyId)
                .and_then(|v| v.ints())
                .unwrap_or_default(),
            // HeadingLevel1 is 80051, …9 80059; HeadingLevel_None 80050.
            heading: var(UIA_HeadingLevelPropertyId)
                .and_then(|v| v.int())
                .filter(|l| (80051..=80059).contains(l))
                .map(|l| (l - 80050) as u8),
            invoke: has(UIA_IsInvokePatternAvailablePropertyId),
            toggle: has(UIA_IsTogglePatternAvailablePropertyId).then(|| {
                var(UIA_ToggleToggleStatePropertyId)
                    .and_then(|v| v.int())
                    .unwrap_or(0)
            }),
            selection_item: has(UIA_IsSelectionItemPatternAvailablePropertyId)
                .then(|| has(UIA_SelectionItemIsSelectedPropertyId)),
            expand: has(UIA_IsExpandCollapsePatternAvailablePropertyId).then(|| {
                var(UIA_ExpandCollapseExpandCollapseStatePropertyId)
                    .and_then(|v| v.int())
                    .unwrap_or(3)
            }),
            // A password's value is never read, whatever its app would give.
            value: (has(UIA_IsValuePatternAvailablePropertyId) && !password).then(|| {
                var(UIA_ValueValuePropertyId)
                    .and_then(|v| v.text())
                    .unwrap_or_default()
            }),
            value_read_only: has(UIA_ValueIsReadOnlyPropertyId),
            range,
            scroll_item: has(UIA_IsScrollItemPatternAvailablePropertyId),
        }
    }
}

#[cfg(test)]
pub mod testing {
    //! A window of the test's own, in its own process, with a few Win32 controls for UI
    //! Automation to read and act on. It's shown at the bottom of the z-order and never
    //! activated, so it stays under the user's windows and never takes the keyboard; nothing of
    //! the user's is read or touched. Dropped, it's closed.

    use std::sync::mpsc;

    use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CreateMenu, CreatePopupMenu,
        CreateWindowExW, DefWindowProcW, DispatchMessageW, ES_AUTOHSCROLL, ES_PASSWORD, GetDlgItem,
        GetMessageW, GetWindowTextW, HMENU, HWND_BOTTOM, KillTimer, MF_CHECKED, MF_GRAYED,
        MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetMenu, SetTimer, SetWindowPos,
        TranslateMessage, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_CHILD,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::desktop::{Desktop, WindowInfo};

    const NAME: i32 = 101;
    const PASSWORD: i32 = 102;
    const REMEMBER: i32 = 103;
    const HELLO: i32 = 104;
    const GREETING: i32 = 105;
    /// The menu's commands: File ▸ Greet, ▸ Word Wrap (checked), ▸ Print (greyed).
    const GREET: i32 = 201;
    const WRAP: i32 = 202;
    const PRINT: i32 = 203;
    const MENU_GREETING: i32 = 204;

    /// The window's menu bar: File, with a command, a separator, an item checked and one greyed.
    fn menus(hwnd: HWND) {
        let item = |menu: HMENU, flags: u32, id: usize, text: &str| {
            let text = wide(text);
            // SAFETY: a menu this thread made, and NUL-terminated words that outlive the call.
            unsafe { AppendMenuW(menu, flags, id, text.as_ptr()) };
        };
        // SAFETY: menus made here, given to this thread's window, which destroys them with it.
        unsafe {
            let (bar, file) = (CreateMenu(), CreatePopupMenu());
            item(file, MF_STRING, GREET as usize, "&Greet\tCtrl+G");
            item(file, MF_SEPARATOR, 0, "");
            item(file, MF_STRING | MF_CHECKED, WRAP as usize, "&Word Wrap");
            item(file, MF_STRING | MF_GRAYED, PRINT as usize, "&Print…");
            item(bar, MF_POPUP, file as usize, "&File");
            SetMenu(hwnd, bar);
        }
    }

    unsafe extern "C" {
        /// Where the linker put this module's image: its instance handle.
        static __ImageBase: u8;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A control of class `class` saying `text`, at `(x, y, w, h)` in `parent`.
    fn control(
        parent: HWND,
        class: &str,
        text: &str,
        style: u32,
        id: i32,
        at: (i32, i32, i32, i32),
    ) {
        let (class, text) = (wide(class), wide(text));
        // SAFETY: NUL-terminated strings that outlive the call, a live parent on this thread,
        // and a child's id where a menu would go.
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                at.0,
                at.1,
                at.2,
                at.3,
                parent,
                id as isize as HMENU,
                std::ptr::null_mut(),
                std::ptr::null(),
            );
        }
    }

    /// "Say hello" greets whoever the name field names, a moment later (as an app's answer
    /// comes): something to wait for. The menu's Greet says it was chosen.
    unsafe extern "system" fn answer(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: called by this window's own message loop, with its own handle.
        unsafe {
            match msg {
                WM_COMMAND
                    if (wp & 0xFFFF) as i32 == HELLO
                        && ((wp >> 16) & 0xFFFF) as u32 == BN_CLICKED =>
                {
                    SetTimer(hwnd, 1, 300, None);
                    0
                }
                // As a menu sends it: the command, and 0 for "from a menu".
                WM_COMMAND if wp == GREET as usize && lp == 0 => {
                    control(
                        hwnd,
                        "STATIC",
                        "Greeted from the menu",
                        0,
                        MENU_GREETING,
                        (150, 100, 150, 20),
                    );
                    0
                }
                WM_TIMER if wp == 1 => {
                    KillTimer(hwnd, 1);
                    let mut buf = [0u16; 128];
                    let n =
                        GetWindowTextW(GetDlgItem(hwnd, NAME), buf.as_mut_ptr(), buf.len() as i32);
                    let name = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                    control(
                        hwnd,
                        "STATIC",
                        &format!("Hello, {name}!"),
                        0,
                        GREETING,
                        (10, 140, 280, 20),
                    );
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    pub struct TestWindow {
        pub info: WindowInfo,
        hwnd: isize,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl TestWindow {
        /// A window called `title`: a name field holding "Ada", a password field, a "Remember me"
        /// checkbox, a "Say hello" button, and a menu bar (File ▸ Greet).
        pub fn open(title: &str) -> Self {
            let (tx, rx) = mpsc::channel();
            let title = title.to_string();
            let thread = std::thread::spawn(move || {
                let class = wide("TrekMcpUiaTest");
                let title = wide(&title);
                // SAFETY: the linker's symbol; only its address is taken.
                #[allow(unused_unsafe)]
                let instance: HINSTANCE = unsafe { (&raw const __ImageBase).cast_mut().cast() };
                // SAFETY: the class and title outlive the calls that use them; every handle is
                // this thread's own, and the loop runs until the window is destroyed.
                unsafe {
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(answer),
                        hInstance: instance,
                        lpszClassName: class.as_ptr(),
                        ..Default::default()
                    };
                    // Already registered by another test: the same class, and that's fine.
                    RegisterClassW(&wc);
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class.as_ptr(),
                        title.as_ptr(),
                        WS_OVERLAPPEDWINDOW,
                        40,
                        40,
                        320,
                        220,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance,
                        std::ptr::null(),
                    );
                    if !hwnd.is_null() {
                        control(hwnd, "STATIC", "Name:", 0, 0, (10, 10, 70, 20));
                        control(
                            hwnd,
                            "EDIT",
                            "Ada",
                            WS_TABSTOP | ES_AUTOHSCROLL as u32,
                            NAME,
                            (90, 10, 200, 22),
                        );
                        control(hwnd, "STATIC", "Password:", 0, 0, (10, 40, 70, 20));
                        control(
                            hwnd,
                            "EDIT",
                            "hunter2",
                            WS_TABSTOP | (ES_PASSWORD | ES_AUTOHSCROLL) as u32,
                            PASSWORD,
                            (90, 40, 200, 22),
                        );
                        control(
                            hwnd,
                            "BUTTON",
                            "Remember me",
                            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
                            REMEMBER,
                            (10, 70, 150, 20),
                        );
                        control(
                            hwnd,
                            "BUTTON",
                            "Say hello",
                            WS_TABSTOP | BS_PUSHBUTTON as u32,
                            HELLO,
                            (10, 100, 100, 26),
                        );
                        menus(hwnd);
                        // Under every other window, and never the active one.
                        SetWindowPos(
                            hwnd,
                            HWND_BOTTOM,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                    let _ = tx.send(hwnd as isize);
                    if hwnd.is_null() {
                        return;
                    }
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            });
            let hwnd = rx.recv().expect("the test window's thread");
            assert_ne!(hwnd, 0, "the test window couldn't be made");
            // As computer use lists it: a window on screen, by its handle.
            let info = crate::desktop::Native::default()
                .window(hwnd as i64)
                .expect("the window list")
                .expect("the test window is listed");
            Self {
                info,
                hwnd,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: a window this made; a closed one just fails the call.
            unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use windows::Win32::UI::Accessibility as uia;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    use super::testing::TestWindow;
    use crate::desktop::uia::kind;
    use crate::desktop::{Desktop, Performed, Reading};
    use crate::outline::{self, Action, Checked, Node, Refs, View};

    #[test]
    fn control_types_are_ui_automations_own() {
        for (ours, theirs) in [
            (kind::BUTTON, uia::UIA_ButtonControlTypeId),
            (kind::CHECK_BOX, uia::UIA_CheckBoxControlTypeId),
            (kind::EDIT, uia::UIA_EditControlTypeId),
            (kind::MENU_BAR, uia::UIA_MenuBarControlTypeId),
            (kind::SCROLL_BAR, uia::UIA_ScrollBarControlTypeId),
            (kind::TEXT, uia::UIA_TextControlTypeId),
            (kind::CUSTOM, uia::UIA_CustomControlTypeId),
            (kind::DOCUMENT, uia::UIA_DocumentControlTypeId),
            (kind::WINDOW, uia::UIA_WindowControlTypeId),
            (kind::PANE, uia::UIA_PaneControlTypeId),
            (kind::TITLE_BAR, uia::UIA_TitleBarControlTypeId),
            (kind::SEPARATOR, uia::UIA_SeparatorControlTypeId),
            (kind::APP_BAR, uia::UIA_AppBarControlTypeId),
        ] {
            assert_eq!(ours, theirs.0);
        }
    }

    /// The first element whose role is `role` and whose name starts with `name`.
    fn find(nodes: &[Node], role: &str, name: &str) -> Option<Node> {
        nodes.iter().find_map(|n| {
            if n.role == role && n.name.starts_with(name) {
                Some(n.clone())
            } else {
                find(&n.children, role, name)
            }
        })
    }

    /// A window of the test's own (never the user's): read as an outline, and acted on by its
    /// elements' own patterns. Nothing takes the focus or posts input, so it never comes in
    /// front of the user's windows.
    #[test]
    fn a_window_of_the_tests_own_is_read_and_acted_on_through_its_elements() {
        let win = crate::desktop::Native::default();
        let window = TestWindow::open("Trek elements test");
        let w = window.info.clone();
        let started = Instant::now();
        let read = win.elements(&w, &Reading::default()).expect("its elements");
        eprintln!("read in {:?}", started.elapsed());
        let mut refs = Refs::new("elements");
        let out = refs.outline(&read.roots, &View::default());
        eprintln!("{}", out.text);
        assert_eq!(read.cut, None);
        assert!(
            out.text
                .starts_with("- window \"Trek elements test\" [frame="),
            "{}",
            out.text
        );
        for line in [
            "- button \"Close\" [ref=",
            "- text \"Name:\" [frame=",
            "- checkbox \"Remember me\" [ref=",
            "[unchecked]",
            "- button \"Say hello\" [ref=",
            ": Ada\n",
            "[secure]",
        ] {
            assert!(out.text.contains(line), "{line}: {}", out.text);
        }
        // The title bar's system menu is the system's; a password is never read.
        assert!(!out.text.contains("System"), "{}", out.text);
        assert!(!out.text.contains("hunter2"), "{}", out.text);

        // The checkbox, pressed: its Toggle pattern; read again, it's checked, the same object.
        let remember = find(&read.roots, "checkbox", "Remember me").expect("the checkbox");
        assert_eq!(remember.checked, Some(Checked::No));
        let id = remember.identity.expect("kept to act on");
        assert_eq!(win.perform(&w, id, Action::Press), Ok(Performed::Done));
        let now = win
            .element_at(&w, &remember.place)
            .unwrap()
            .expect("still there");
        assert_eq!(now.checked, Some(Checked::Yes));
        assert_eq!(now.identity, Some(id), "the same object");
        // What it hasn't can't be asked of it.
        assert!(
            win.perform(&w, id, Action::Increment)
                .unwrap_err()
                .contains("no increment action")
        );

        // The name field: text added at the end of its value, read back from it.
        let name = find(&read.roots, "text field", "Name").expect("the name field");
        assert_eq!(name.value.as_deref(), Some("Ada"));
        let field = win.element_at(&w, &name.place).unwrap().expect("the field");
        win.add_text(&w, field.identity.unwrap(), " Lovelace")
            .expect("its Value takes it");
        let now = win.element_at(&w, &name.place).unwrap().unwrap();
        assert_eq!(now.value.as_deref(), Some("Ada Lovelace"));
        // Never a password's.
        let password = find(&read.roots, "text field", "Password").expect("the password");
        assert!(password.secure && password.value.is_none());
        let kept = win.element_at(&w, &password.place).unwrap().unwrap();
        assert!(win.add_text(&w, kept.identity.unwrap(), "x").is_err());

        // The button, pressed: its Invoke pattern; its window answers a moment later, and the
        // greeting is waited for in its elements.
        let hello = find(&read.roots, "button", "Say hello").expect("the button");
        let found = win.element_at(&w, &hello.place).unwrap().unwrap();
        assert_eq!(found.identity, hello.identity);
        assert_eq!(
            win.perform(&w, hello.identity.unwrap(), Action::Press),
            Ok(Performed::Done)
        );
        let greeted = Instant::now();
        let greeting = loop {
            let read = win.elements(&w, &Reading::default()).unwrap();
            if outline::has_text(&read.roots, "Hello, Ada Lovelace!") {
                break read;
            }
            assert!(
                greeted.elapsed() < Duration::from_secs(5),
                "no greeting: {}",
                Refs::new("elements")
                    .outline(&read.roots, &View::default())
                    .text
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let again = refs.outline(&greeting.roots, &View::default());
        assert!(
            again
                .text
                .contains("- text \"Hello, Ada Lovelace!\" [frame="),
            "{}",
            again.text
        );
        // The elements that were there keep their refs.
        let line = |text: &str, what: &str| {
            text.lines()
                .find(|l| l.contains(what))
                .and_then(|l| l.split("[ref=").nth(1))
                .and_then(|r| r.split(']').next())
                .map(str::to_string)
        };
        assert_eq!(
            line(&again.text, "\"Say hello\""),
            line(&out.text, "\"Say hello\"")
        );

        // Its menus, as the Mac's menu bar: read whole as a second root with nothing opened
        // (UI Automation sees a menu's items only while it's open), and an item pressed by its
        // command, as the menu would choose it.
        let with_menus = Reading {
            menu_bar: true,
            ..Reading::default()
        };
        let read = win.elements(&w, &with_menus).unwrap();
        assert_eq!(read.roots.len(), 2);
        let bar = Refs::new("elements").outline(&read.roots[1..], &View::default());
        assert!(
            bar.text
                .starts_with("- menu bar:\n  - menu \"File\":\n    - menu item \"Greet\" [ref="),
            "{}",
            bar.text
        );
        for line in [
            "- menu item \"Word Wrap\" [ref=e2] [checked]",
            "- menu item \"Print…\" [ref=e3] [disabled]",
        ] {
            assert!(bar.text.contains(line), "{line}: {}", bar.text);
        }
        let greet = find(&read.roots, "menu item", "Greet").expect("the menu's Greet");
        let found = win
            .element_at(&w, &greet.place)
            .unwrap()
            .expect("found again");
        assert_eq!(
            (found.identity, found.name.as_str()),
            (greet.identity, "Greet")
        );
        assert_eq!(
            win.perform(&w, greet.identity.unwrap(), Action::Press),
            Ok(Performed::Done)
        );
        let chosen = Instant::now();
        while !outline::has_text(
            &win.elements(&w, &Reading::default()).unwrap().roots,
            "Greeted from the menu",
        ) {
            assert!(
                chosen.elapsed() < Duration::from_secs(5),
                "Greet wasn't chosen"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        // Asked for without them, a window is one root: its menu bar's titles are in it.
        let read = win.elements(&w, &Reading::default()).unwrap();
        assert_eq!(read.roots.len(), 1);
        assert!(find(&read.roots, "menu", "File").is_some_and(|m| m.acts));

        // None of it took the front from whatever window the user has there.
        // SAFETY: takes nothing; the handle is only compared.
        let front = unsafe { GetForegroundWindow() };
        assert_ne!(front as i64, w.id, "the test's window came in front");
        drop(window);
        // Closed: nothing's there to read.
        assert!(win.elements(&w, &Reading::default()).is_err());
    }
}
