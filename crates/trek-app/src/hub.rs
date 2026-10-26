//! The title bar's hub: one quiet button (two dots, two lines) whose card says, for the thread or
//! draft on screen, where it works and what it has to hand. Top to bottom, each section only when
//! it has something to say:
//!
//! - **Where**: the machine and the project (its settings behind "⋯"); Open in the file manager
//!   (the caret lists the terminal and editors), and Run the project's actions.
//! - **Git**: the branch (switch it here), the thread's pull request, Pull / Push / Fetch with how
//!   far the branch is behind and ahead, and what's changed (it opens Source control).
//! - **This thread**: how full the context window is (it opens the context card), the plan's
//!   nearest limit, and the thread's sub-agents.
//! - **Sources**: what the thread used, newest first: images attached, files mentioned, read or
//!   edited, pages fetched.
//! - **Lineage**: threads this one came from or started, and agents it was handed between.
//!
//! The card is drawn as the composer's menus are (`composer::look`): a floating card with a
//! hairline and a soft shadow, 28 px rows. Menus a row has (the caret of a split row, the branch
//! list) unfold inside the card, so a click in them never lands outside it.

use crate::assets::Lucide;
use crate::composer::look;
use crate::workspace::{
    GitInfo, LineageRow, PanelTool, Route, Scope, TaskState, Workspace, WorkspaceEvent,
};
use gpui_kit::base::PopoverState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use trek_agents::UsageLimit;
use trek_core::AgentId;
use trek_core::settings::ProjectAction;
use trek_core::store::{Item, Thread, ToolStatus};
use trek_core::worktree::Worktree;

/// The card's width.
const WIDTH: f32 = 300.;
/// What the card leaves of the window's height: past that it scrolls.
const MARGIN: f32 = 80.;
/// Sources listed before "View all".
const SOURCES: usize = 3;
/// A row's icon.
const ICON: f32 = 14.;

/// The hub's button and its card, for `scope` (the main window, or a thread's own window).
pub fn button(ws: Entity<Workspace>, scope: Scope) -> Hub {
    Hub { ws, scope }
}

/// Whether `scope` has anything for the hub: a thread, or a draft (and the main window's editor
/// route, which has a folder to open and actions to run, as the title bar always offered).
pub fn shown(ws: &Workspace, scope: &Scope) -> bool {
    subject(ws, scope).is_some()
}

#[derive(IntoElement)]
pub struct Hub {
    ws: Entity<Workspace>,
    scope: Scope,
}

/// What's unfolded in the open card (folded again each time it opens), and git work under way.
#[derive(Default)]
struct Ui {
    open_in: bool,
    run: bool,
    branch: bool,
    pull: bool,
    agents: bool,
    sources: bool,
    busy: Option<GitOp>,
}

impl RenderOnce for Hub {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = window.use_keyed_state("hub-ui", cx, |_, _| Ui::default());
        // Ask for the computer's name now, so the card has it by the time it opens.
        let _ = this_computer();
        let max_h = window.viewport_size().height - px(MARGIN);
        let (ws, scope) = (self.ws, self.scope);
        let (on_open_ui, on_open_ws, on_open_scope) = (ui.clone(), ws.clone(), scope.clone());
        Popover::new("hub-popover")
            // Under the button, its right edge on the button's.
            .anchor(Anchor::TopRight)
            .offset(px(6.))
            .appearance(false)
            .on_open_change(move |open: &bool, _, cx: &mut App| {
                if !*open {
                    return;
                }
                on_open_ui.update(cx, |u, cx| {
                    let busy = u.busy;
                    *u = Ui {
                        busy,
                        ..Ui::default()
                    };
                    cx.notify();
                });
                // The plan's limits as the agent last left them on disk, as the context card reads
                // them: the Usage row has them without asking the agent anything.
                let agent = on_open_ws
                    .read(cx)
                    .thread_in(&on_open_scope)
                    .map(|t| t.agent.clone());
                if let Some(agent) = agent {
                    on_open_ws.update(cx, |ws, cx| ws.read_usage_for(agent, cx));
                }
            })
            .trigger(
                Button::new("hub")
                    .ghost()
                    .small()
                    .icon(Icon::empty().path("icons/hub.svg"))
                    .tooltip("Overview"),
            )
            .content(move |_, window, cx| {
                let pop = cx.entity();
                card(&ws, &scope, &ui, Closer::Popover(pop), max_h, window, cx)
            })
    }
}

/// The main window's hub button: shows and hides the pinned column (`column`), which stays until
/// it's clicked again (`settings.layout.hub_open`, on to begin with).
pub fn toggle(ws: Entity<Workspace>, open: bool) -> impl IntoElement {
    Button::new("hub")
        .ghost()
        .small()
        .selected(open)
        .icon(Icon::empty().path("icons/hub.svg"))
        .tooltip(if open {
            "Hide overview"
        } else {
            "Show overview"
        })
        .on_click(move |_, _, cx| {
            ws.update(cx, |ws, cx| {
                ws.settings.layout.hub_open = !ws.settings.layout.hub_open;
                ws.save_settings(cx);
                cx.notify();
            })
        })
}

/// The hub pinned at the main window's right edge, under the tabs, for `scope`: the card's
/// sections in a column of their own beside the transcript (which keeps its room, never covered).
pub fn column(
    ws: Entity<Workspace>,
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ui = window.use_keyed_state("hub-column-ui", cx, |_, _| Ui::default());
    let _ = this_computer();
    // The plan's limits for the thread's agent, read once per agent shown (the popover read them
    // as it opened).
    let agent = ws.read(cx).thread_in(&scope).map(|t| t.agent.clone());
    let asked = window.use_keyed_state("hub-column-usage", cx, |_, _| None::<AgentId>);
    if agent.is_some() && *asked.read(cx) != agent {
        asked.update(cx, |a, _| *a = agent.clone());
        let ws2 = ws.clone();
        window.defer(cx, move |_, cx| {
            if let Some(agent) = agent {
                ws2.update(cx, |ws, cx| ws.read_usage_for(agent, cx));
            }
        });
    }
    // Under the title bar and the tabs; past that the card scrolls.
    let max_h = window.viewport_size().height - px(110.);
    div()
        .id("hub-column")
        .test_support()
        .flex_none()
        .h_full()
        .pt(px(8.))
        .pr(px(12.))
        .pl(px(4.))
        .child(card(
            &ws,
            &scope,
            &ui,
            Closer::Pinned(ui.clone()),
            max_h,
            window,
            cx,
        ))
        .into_any_element()
}

// ---------- what the card shows ----------

/// What the hub is about: the thread on screen, or the folder a draft (or the editor) is in.
struct Subject {
    thread: Option<Thread>,
    /// Where Open opens: the thread's folder (its worktree's, for one in a worktree), the draft's
    /// project.
    folder: Option<PathBuf>,
}

fn subject(ws: &Workspace, scope: &Scope) -> Option<Subject> {
    let of_thread = |id: &str| {
        ws.thread(id).map(|t| Subject {
            thread: Some(t.clone()),
            folder: t.cwd.clone(),
        })
    };
    match scope {
        Scope::Main => match &ws.route {
            Route::Thread(id) => of_thread(id),
            Route::Draft { project } => Some(Subject {
                thread: None,
                folder: project.clone(),
            }),
            Route::Editor { path } => Some(Subject {
                thread: None,
                folder: path.parent().map(trek_core::store::project_root),
            }),
            _ => None,
        },
        Scope::Thread(id) => of_thread(id),
        Scope::Ide | Scope::Side => None,
    }
}

/// Something the thread used.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    /// A file: attached, mentioned, read or edited (`verb` says which, last first).
    File { path: PathBuf, verb: &'static str },
    /// A page the agent fetched.
    Link { url: String },
}

impl Source {
    fn key(&self) -> String {
        match self {
            Source::File { path, .. } => path.display().to_string(),
            Source::Link { url } => url.clone(),
        }
    }
}

/// A thread related to this one, or a hand-over between agents in it.
struct Lineage {
    label: String,
    title: String,
    /// The thread it opens, when it's still in Trek.
    open: Option<String>,
    state: Option<String>,
}

/// One section's worth of data each; `None` where there's nothing to show.
struct Model {
    machine: String,
    remote: bool,
    project: Option<String>,
    project_id: Option<String>,
    folder: Option<PathBuf>,
    run_dir: Option<PathBuf>,
    actions: Vec<ProjectAction>,
    git_cwd: Option<PathBuf>,
    git: Option<GitInfo>,
    worktree: Option<Worktree>,
    switch_blocked: Option<String>,
    pr: Option<crate::pr_card::Card>,
    changes: Option<String>,
    context: Option<(u64, u64)>,
    usage: Option<(AgentId, UsageLimit)>,
    agents: Vec<(String, String, TaskState)>,
    sources: Vec<Source>,
    lineage: Vec<Lineage>,
    thread: Option<String>,
}

fn model(ws: &Workspace, scope: &Scope) -> Option<Model> {
    let Subject { thread, folder } = subject(ws, scope)?;
    let chat = folder.as_deref().is_some_and(trek_core::paths::is_chat_dir);
    // A thread knows its project (a worktree's folder isn't the project's).
    let root = thread.as_ref().and_then(|t| ws.project_dir(t)).or_else(|| {
        folder
            .as_deref()
            .filter(|_| !chat)
            .map(trek_core::store::project_root)
    });
    let entry = root
        .as_ref()
        .and_then(|r| ws.projects.iter().find(|p| &p.path == r));
    let project = entry.map(|p| p.name.clone()).or_else(|| {
        root.as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().to_string())
    });
    let worktree = thread.as_ref().and_then(|t| t.worktree.clone());
    // A worktree thread's actions run on its own copy of the code, where its changes are.
    let run_dir = worktree
        .as_ref()
        .filter(|w| !crate::system::lately::worktree_missing(w))
        .map(|w| w.path.clone())
        .or_else(|| root.clone());
    let actions = root
        .as_ref()
        .map(|r| ws.project_prefs(r).actions)
        .unwrap_or_default();
    let remote_on = ws.on_machine_in(scope).filter(|_| {
        thread
            .as_ref()
            .is_some_and(|t| crate::machines::is_remote(&t.id))
    });
    let remote = remote_on.is_some();
    let machine = remote_on.unwrap_or_else(this_computer);
    let git_cwd = ws.cwd_in(scope).filter(|_| !remote && !chat);
    let git = git_cwd
        .as_ref()
        .and_then(|c| ws.git_info.get(c))
        .filter(|g| g.is_repo)
        .cloned();
    let switch_blocked = git_cwd.as_deref().and_then(|c| ws.switch_blocked(c));
    let live = thread.as_ref().and_then(|t| ws.live.get(&t.id));
    let pr = live.and_then(crate::pr_card::latest_pull_request);
    // The thread's own lines when it has them; else what the working tree has changed.
    let changes = match &thread {
        Some(t) if t.additions > 0 || t.deletions > 0 => {
            Some(format!("+{} −{}", t.additions, t.deletions))
        }
        _ => git
            .as_ref()
            .filter(|g| g.changed > 0)
            .map(|g| format!("{} changed", g.changed)),
    };
    let context = live.and_then(|l| l.context);
    let usage = thread
        .as_ref()
        .and_then(|t| ws.usage_focus_for(t.agent.clone(), t.model.clone()));
    let agents = thread
        .as_ref()
        .map(|t| {
            ws.children(&t.id)
                .into_iter()
                .map(|c| (c.id.clone(), c.title.clone(), ws.task_state(&c.id)))
                .collect()
        })
        .unwrap_or_default();
    let sources = match (&thread, live) {
        (Some(t), Some(l)) => sources(&l.items, t.cwd.as_deref()),
        _ => vec![],
    };
    let lineage = thread
        .as_ref()
        .map(|t| lineage(ws, t, live.map(|l| &l.items[..]).unwrap_or_default()))
        .unwrap_or_default();
    Some(Model {
        machine,
        remote,
        project,
        project_id: entry.map(|p| p.id.clone()),
        folder,
        run_dir,
        actions,
        git_cwd,
        git,
        worktree,
        switch_blocked,
        pr,
        changes,
        context,
        usage,
        agents,
        sources,
        lineage,
        thread: thread.map(|t| t.id),
    })
}

/// This computer's name ("Tobias's MacBook Pro"), read once off the main thread; "this Mac"
/// until it's in.
fn this_computer() -> String {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    static NAME: OnceLock<String> = OnceLock::new();
    static ASKED: AtomicBool = AtomicBool::new(false);
    if let Some(name) = NAME.get() {
        return name.clone();
    }
    if !ASKED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(|| {
            let _ = NAME.set(crate::machines::this_name());
        });
    }
    let words = crate::words::words();
    let mut name = words.this_computer.to_string();
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    name
}

/// What the thread used, newest first, each once: images attached, files mentioned (`@path`),
/// read or edited, pages fetched. Relative paths are the thread's folder's.
fn sources(items: &[Item], cwd: Option<&Path>) -> Vec<Source> {
    let resolve = |p: &str| -> Option<PathBuf> {
        let p = p.trim();
        if p.is_empty() {
            return None;
        }
        let path = match p.strip_prefix("~/") {
            Some(rest) => trek_core::paths::home().join(rest),
            None if Path::new(p).is_absolute() => PathBuf::from(p),
            None => cwd?.join(p),
        };
        Some(path)
    };
    let mut out: Vec<Source> = vec![];
    let mut push = |s: Source| {
        let key = s.key();
        if !out.iter().any(|o| o.key() == key) {
            out.push(s);
        }
    };
    for item in items.iter().rev() {
        match item {
            Item::User {
                text,
                images,
                aside: false,
                ..
            } => {
                for image in images.iter().rev() {
                    push(Source::File {
                        path: PathBuf::from(image),
                        verb: "Attached",
                    });
                }
                for word in text.split_whitespace().rev() {
                    let Some(m) = word.strip_prefix('@') else {
                        continue;
                    };
                    let m = m.trim_end_matches([',', '.', ';', ':', ')', '!', '?']);
                    if !(m.contains('/') || m.contains('.')) {
                        continue;
                    }
                    if let Some(path) = resolve(m).filter(|p| p.exists()) {
                        push(Source::File {
                            path,
                            verb: "Mentioned",
                        });
                    }
                }
            }
            Item::Tool {
                title,
                detail,
                status,
                ..
            } if !matches!(status, ToolStatus::Failed | ToolStatus::Denied) => {
                use crate::activity::{ToolKind, tool_kind};
                match tool_kind(title) {
                    kind @ (ToolKind::Read | ToolKind::Edit) => {
                        let verb = if kind == ToolKind::Edit {
                            "Edited"
                        } else {
                            "Read"
                        };
                        for p in detail.split(", ") {
                            if let Some(path) = resolve(p) {
                                push(Source::File { path, verb });
                            }
                        }
                    }
                    ToolKind::Fetch => {
                        if let Some(url) = detail
                            .split_whitespace()
                            .find(|w| w.starts_with("https://") || w.starts_with("http://"))
                        {
                            push(Source::Link {
                                url: url.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// Threads `t` came from or started, and the agents it was handed between, in that order.
fn lineage(ws: &Workspace, t: &Thread, items: &[Item]) -> Vec<Lineage> {
    let title_of = |id: &str| {
        ws.thread(id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "A thread no longer in Trek".to_string())
    };
    let open = |id: &str| ws.thread(id).map(|_| id.to_string());
    let mut out = vec![];
    use trek_core::lineage::Link;
    if let Some(from) = &t.continued_from {
        out.push(Lineage {
            label: Link::ContinuedFrom.label().to_string(),
            title: title_of(from),
            open: open(from),
            state: None,
        });
    }
    if let Some(by) = &t.started_by
        && t.continued_from.as_ref() != Some(by)
    {
        out.push(Lineage {
            label: Link::StartedBy.label().to_string(),
            title: title_of(by),
            open: open(by),
            state: None,
        });
    }
    for item in items {
        if let Item::Handoff {
            from,
            to,
            from_name,
            to_name,
            ..
        } = item
        {
            let name = |key: &str, named: &Option<String>| {
                named
                    .clone()
                    .unwrap_or_else(|| AgentId::from_key(key).display_name())
            };
            out.push(Lineage {
                label: "Handed over".to_string(),
                title: format!("{} → {}", name(from, from_name), name(to, to_name)),
                open: None,
                state: None,
            });
        }
    }
    for c in ws.started_threads(&t.id) {
        let link = if c.continued_from.as_deref() == Some(t.id.as_str()) {
            Link::ContinuedIn
        } else {
            Link::Started
        };
        out.push(Lineage {
            label: link.label().to_string(),
            title: c.title.clone(),
            open: Some(c.id.clone()),
            state: Some(ws.thread_state(&c.id).label().to_string()),
        });
    }
    out
}

// ---------- git ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitOp {
    Pull,
    Push,
    Fetch,
}

impl GitOp {
    fn label(self) -> &'static str {
        match self {
            GitOp::Pull => "Pull",
            GitOp::Push => "Push",
            GitOp::Fetch => "Fetch",
        }
    }

    /// Off the main thread. A pull only fast-forwards: one that would merge says so instead.
    fn run(self, cwd: &Path, wt: Option<&Worktree>) -> Result<(), String> {
        use crate::panels::git::{git, git_read};
        let run = |args: &[&str]| git(cwd, args).map(|_| ());
        match self {
            GitOp::Pull => run(&["pull", "--ff-only"]),
            GitOp::Fetch => run(&["fetch", "--prune"]),
            // As the Git panel pushes: a worktree's branch to origin, else with its upstream (or
            // setting one).
            GitOp::Push => match wt {
                Some(wt) => trek_core::worktree::push(wt).map_err(|e| format!("{e:#}")),
                None if git_read(cwd, &["rev-parse", "--abbrev-ref", "@{u}"]).is_ok() => {
                    run(&["push"])
                }
                None => run(&["push", "-u", "origin", "HEAD"]),
            },
        }
    }
}

/// Run `op` in `cwd`, the row spinning meanwhile; say how it went, then read the branch again.
fn start_git(
    op: GitOp,
    cwd: PathBuf,
    wt: Option<Worktree>,
    ws: Entity<Workspace>,
    ui: Entity<Ui>,
    window: &mut Window,
    cx: &mut App,
) {
    if ui.read(cx).busy.is_some() {
        return;
    }
    ui.update(cx, |u, cx| {
        u.busy = Some(op);
        cx.notify();
    });
    let dir = cwd.clone();
    let work = cx
        .background_executor()
        .spawn(async move { op.run(&dir, wt.as_ref()) });
    window
        .spawn(cx, async move |cx| {
            let result = work.await;
            let _ = cx.update(|window, cx| {
                ui.update(cx, |u, cx| {
                    u.busy = None;
                    cx.notify();
                });
                match result {
                    Ok(()) => crate::toast::push(window, format!("{} done", op.label()), cx),
                    Err(e) => crate::toast::push(window, crate::toast::Toast::error(e), cx),
                }
                ws.update(cx, |ws, cx| ws.refresh_checkout(cwd, cx));
            });
        })
        .detach();
}

// ---------- Open in, Run ----------

/// Where a folder opens: the file manager first, then the terminal and editors that are installed.
/// On a Mac the apps are launched through `open -a`; on Windows each is found through the
/// registry and PATH (`trek_core::open_in`) and started with the folder as its argument.
type Opener = (SharedString, Rc<dyn Fn(&mut App)>);

fn openers(dir: &Path) -> Vec<Opener> {
    let mut out: Vec<Opener> = vec![];
    let folder = dir.to_path_buf();
    out.push((
        crate::words::words().file_manager.into(),
        Rc::new(move |cx: &mut App| cx.open_with_system(&folder)),
    ));
    if cfg!(windows) {
        for target in trek_core::open_in::windows_targets() {
            let (label, dir) = (target.label, dir.to_path_buf());
            out.push((
                label.into(),
                Rc::new(move |_: &mut App| {
                    let Some(launch) = target.launch(&dir) else {
                        return tracing::warn!(
                            "open in {label}: {} isn't an absolute path",
                            dir.display()
                        );
                    };
                    if let Err(e) = trek_core::open_in::spawn(&launch) {
                        tracing::warn!("open in {label}: {e}");
                    }
                }),
            ));
        }
        return out;
    }
    let apps = [
        ("Terminal", "Terminal"),
        ("Ghostty", "Ghostty"),
        ("Zed", "Zed"),
        ("Cursor", "Cursor"),
        ("VS Code", "Visual Studio Code"),
        ("Xcode", "Xcode"),
    ]
    .into_iter()
    .filter(|(_, app)| {
        *app == "Terminal" || Path::new(&format!("/Applications/{app}.app")).exists()
    });
    for (label, app) in apps {
        let dir = dir.to_path_buf();
        out.push((
            label.into(),
            Rc::new(move |_: &mut App| {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("-a")
                    .arg(app)
                    .arg(&dir)
                    .spawn();
            }),
        ));
    }
    out
}

// ---------- the card ----------

/// What picking a row does to the card around it: a popover (a thread's own window) closes; the
/// main window's pinned column stays until the hub button hides it, its unfolded lists folding
/// back as a menu would.
#[derive(Clone)]
enum Closer {
    Popover(Entity<PopoverState>),
    Pinned(Entity<Ui>),
}

fn close(pop: &Closer, window: &mut Window, cx: &mut App) {
    match pop {
        Closer::Popover(pop) => pop.update(cx, |p, cx| p.dismiss(window, cx)),
        Closer::Pinned(ui) => ui.update(cx, |u, cx| {
            let busy = u.busy;
            *u = Ui {
                busy,
                ..Ui::default()
            };
            cx.notify();
        }),
    }
}

fn header(text: &'static str, cx: &App) -> Div {
    div()
        .px(px(9.))
        .pt(px(7.))
        .pb(px(3.))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn rule(cx: &App) -> Div {
    div().h(px(1.)).mx(px(5.)).my(px(4.)).bg(look::hairline(cx))
}

fn icon(icon: impl Into<Icon>, cx: &App) -> Icon {
    let icon: Icon = icon.into();
    icon.size(px(ICON)).text_color(cx.theme().muted_foreground)
}

/// A row's label, cut short when it doesn't fit.
fn label(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
}

/// What a row says on its right, quietly.
fn trailing(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_none()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// The caret half of a split row: it unfolds the row's menu in the card.
fn caret(id: impl Into<ElementId>, open: bool, cx: &App) -> Stateful<Div> {
    look::row(id, open, cx)
        .flex_none()
        .px(px(7.))
        .justify_center()
        .child(
            Icon::new(Lucide::ChevronDown)
                .size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .when(open, |i| {
                    i.transform(Transformation::rotate(radians(std::f32::consts::PI)))
                }),
        )
}

/// A row and its caret, side by side with a hairline between.
fn split(main: impl IntoElement, caret: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .items_center()
        .child(div().flex_1().min_w_0().child(main))
        .child(div().w(px(1.)).h(px(14.)).mx(px(2.)).bg(look::hairline(cx)))
        .child(caret)
}

/// A row that only says something (where, the usage): `look::row`'s shape, with no highlight.
fn plain_row(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .px(px(9.))
        .min_h(px(28.))
        .gap(px(8.))
        .rounded(px(7.))
        .text_size(px(13.))
}

/// A row in an unfolded menu, set in under its row.
fn sub_row(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    look::row(id, active, cx).pl(px(31.))
}

fn card(
    ws: &Entity<Workspace>,
    scope: &Scope,
    ui: &Entity<Ui>,
    pop: Closer,
    max_h: Pixels,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(m) = model(ws.read(cx), scope) else {
        return div().into_any_element();
    };
    let theme = cx.theme().clone();
    let u = ui.read(cx);
    let (open_in, run_open, branch_open, pull_open, agents_open, all_sources, busy) = (
        u.open_in, u.run, u.branch, u.pull, u.agents, u.sources, u.busy,
    );
    let from_window = matches!(scope, Scope::Thread(_));
    let toggle = |ui: &Entity<Ui>, f: fn(&mut Ui)| {
        let ui = ui.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            ui.update(cx, |u, cx| {
                f(u);
                cx.notify();
            })
        }
    };
    // A thread window's actions surface in the main window: bring it forward.
    let activate = move |cx: &mut Context<Workspace>| {
        if from_window {
            cx.emit(WorkspaceEvent::ActivateMain);
        }
    };
    let mut sections: Vec<AnyElement> = vec![];

    // ----- Where -----
    let mut place = v_flex().child(header("Where", cx)).child(
        plain_row("hub-where")
            .test_support()
            .child(icon(
                if m.remote {
                    Lucide::Monitor
                } else {
                    Lucide::Laptop
                },
                cx,
            ))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    // The machine keeps its name whole as far as it can; the project gives way first.
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(170.))
                            .truncate()
                            .child(m.machine.clone()),
                    )
                    .when_some(m.project.clone(), |el, p| {
                        el.child(
                            div()
                                .flex_none()
                                .text_color(theme.muted_foreground)
                                .child("·"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(p),
                        )
                    }),
            )
            .when(m.project_id.is_some(), |el| {
                let (ws, pid, pop) = (ws.clone(), m.project_id.clone(), pop.clone());
                el.child(
                    Button::new("hub-project-settings")
                        .ghost()
                        .xsmall()
                        .icon(icon(Lucide::Ellipsis, cx))
                        .tooltip("Project settings")
                        .on_click(move |_, window, cx| {
                            close(&pop, window, cx);
                            let pid = pid.clone();
                            ws.update(cx, |ws, cx| {
                                ws.open_project_settings(pid, cx);
                                activate(cx);
                            });
                        }),
                )
            }),
    );
    if let Some(dir) = m.folder.clone() {
        let openers = openers(&dir);
        let first = openers[0].clone();
        let pop1 = pop.clone();
        let main = look::row("hub-open", false, cx)
            .test_support()
            .child(icon(Lucide::FolderOpen, cx))
            .child(label(format!("Open in {}", first.0)))
            .on_click(move |_, window, cx| {
                close(&pop1, window, cx);
                (first.1)(cx);
            });
        // A caret only when there's somewhere else to open it (a Windows with no editor or
        // terminal found has the file manager alone).
        let more = openers.len() > 1;
        place = if more {
            place.child(split(
                main,
                caret("hub-open-more", open_in, cx)
                    .test_support()
                    .on_click(toggle(ui, |u| u.open_in = !u.open_in)),
                cx,
            ))
        } else {
            place.child(main)
        };
        if open_in && more {
            for (i, (name, open)) in openers.into_iter().enumerate().skip(1) {
                let pop = pop.clone();
                place = place.child(
                    sub_row(("hub-open-in", i), false, cx)
                        .test_support()
                        .child(label(name))
                        .on_click(move |_, window, cx| {
                            close(&pop, window, cx);
                            open(cx);
                        }),
                );
            }
        }
    }
    if let Some(dir) = m.run_dir.clone() {
        let edit = {
            let (ws, pid, pop) = (ws.clone(), m.project_id.clone(), pop.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                close(&pop, window, cx);
                let pid = pid.clone();
                ws.update(cx, |ws, cx| {
                    ws.open_project_settings(pid, cx);
                    activate(cx);
                });
            }
        };
        let run = |a: &ProjectAction| {
            let (ws, dir, pop, cmd) = (ws.clone(), dir.clone(), pop.clone(), a.command.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                close(&pop, window, cx);
                let (cmd, dir) = (cmd.clone(), dir.clone());
                ws.update(cx, |ws, cx| {
                    ws.run_project_action(dir, cmd, cx);
                    activate(cx);
                })
            }
        };
        match m.actions.first() {
            // Nothing to run yet: the row adds an action, in this project's settings.
            None => {
                place = place.child(
                    look::row("hub-run", false, cx)
                        .test_support()
                        .child(icon(Lucide::Play, cx))
                        .child(label("Add a project action…"))
                        .on_click(edit),
                );
            }
            Some(first) => {
                let main = look::row("hub-run", false, cx)
                    .test_support()
                    .child(icon(Lucide::Play, cx))
                    .child(label(format!("Run {}", first.name)))
                    .on_click(run(first));
                place = place.child(split(
                    main,
                    caret("hub-run-more", run_open, cx)
                        .test_support()
                        .on_click(toggle(ui, |u| u.run = !u.run)),
                    cx,
                ));
                if run_open {
                    for (i, a) in m.actions.iter().enumerate() {
                        place = place.child(
                            sub_row(("hub-run-action", i), false, cx)
                                .test_support()
                                .child(label(a.name.clone()))
                                .child(trailing(a.command.clone(), cx).max_w(px(120.)).truncate())
                                .on_click(run(a)),
                        );
                    }
                    place = place.child(
                        sub_row("hub-run-edit", false, cx)
                            .test_support()
                            .text_color(theme.muted_foreground)
                            .child(label("Edit actions…"))
                            .on_click(edit),
                    );
                }
            }
        }
    }
    sections.push(place.into_any_element());

    // ----- Git -----
    let mut git = v_flex();
    let mut git_rows = 0;
    if let (Some(g), Some(cwd)) = (m.git.clone(), m.git_cwd.clone()) {
        git_rows += 1;
        match &m.worktree {
            // The thread's own branch, off its base: nothing to switch to here.
            Some(wt) => {
                git = git.child(
                    plain_row("hub-branch")
                        .test_support()
                        .child(icon(Lucide::GitBranch, cx))
                        .child(label(wt.branch.clone()))
                        .child(trailing(format!("off {}", wt.base), cx)),
                );
            }
            None => {
                let name = g.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                git = git.child(
                    look::row("hub-branch", branch_open, cx)
                        .test_support()
                        .child(icon(Lucide::GitBranch, cx))
                        .child(label(name))
                        .child(
                            Icon::new(Lucide::ChevronDown)
                                .size(px(12.))
                                .text_color(theme.muted_foreground),
                        )
                        .on_click(toggle(ui, |u| u.branch = !u.branch)),
                );
                if branch_open {
                    if let Some(why) = m.switch_blocked.clone() {
                        git = git.child(
                            div()
                                .pl(px(31.))
                                .pr(px(9.))
                                .py(px(4.))
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(why),
                        );
                    }
                    let list = g.branches.iter().enumerate().map(|(i, b)| {
                        let current = g.branch.as_ref() == Some(b);
                        // Git won't check out a branch another worktree has (a worktree thread's).
                        let taken = g.elsewhere.contains(b);
                        let off = !current && (taken || m.switch_blocked.is_some());
                        let note = if taken {
                            Some("in another worktree")
                        } else if g.default_branch.as_ref() == Some(b) {
                            Some("default")
                        } else {
                            None
                        };
                        let (ws, cwd, b2, pop) = (ws.clone(), cwd.clone(), b.clone(), pop.clone());
                        sub_row(("hub-branch-to", i), current, cx)
                            .test_support()
                            .when(off, |el| el.opacity(0.5).cursor_default())
                            .child(label(b.clone()))
                            .children(note.map(|n| trailing(n, cx)))
                            .when(current, |el| {
                                el.child(
                                    Icon::new(Lucide::Check)
                                        .size(px(12.))
                                        .text_color(theme.muted_foreground),
                                )
                            })
                            .when(!off && !current, |el| {
                                el.on_click(move |_, window, cx| {
                                    close(&pop, window, cx);
                                    let (cwd, b) = (cwd.clone(), b2.clone());
                                    ws.update(cx, |ws, cx| ws.switch_branch(cwd, b, cx));
                                })
                            })
                    });
                    git = git.child(
                        // Its own height, up to 180: a scroll box may shrink to nothing in the card
                        // (which scrolls too), and its rows then ran over the ones below.
                        v_flex()
                            .id("hub-branches")
                            .flex_shrink_0()
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .children(list),
                    );
                }
            }
        }
        // ↓ behind ↑ ahead, as the branch chip counts them.
        let counts = h_flex()
            .flex_none()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .when(g.behind > 0, |el| el.child(format!("↓{}", g.behind)))
            .when(g.ahead > 0, |el| el.child(format!("↑{}", g.ahead)));
        let pull_busy = busy.is_some();
        let git_op = |op: GitOp| {
            let (ws, ui, cwd, wt) = (ws.clone(), ui.clone(), cwd.clone(), m.worktree.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                start_git(
                    op,
                    cwd.clone(),
                    wt.clone(),
                    ws.clone(),
                    ui.clone(),
                    window,
                    cx,
                )
            }
        };
        // A worktree's branch has no upstream to pull from until it's pushed: Push is the row.
        let main_op = if m.worktree.is_some() && g.behind == 0 {
            GitOp::Push
        } else {
            GitOp::Pull
        };
        let main = look::row("hub-pull", false, cx)
            .test_support()
            .child(match busy {
                Some(_) => Icon::new(Lucide::LoaderCircle)
                    .size(px(ICON))
                    .text_color(theme.muted_foreground)
                    .with_animation(
                        "hub-git-busy",
                        Animation::new(std::time::Duration::from_secs(1)).repeat(),
                        |i, t| i.transform(Transformation::rotate(percentage(t))),
                    )
                    .into_any_element(),
                None => icon(
                    if main_op == GitOp::Push {
                        Lucide::ArrowUp
                    } else {
                        Lucide::ArrowDown
                    },
                    cx,
                )
                .into_any_element(),
            })
            .child(label(match busy {
                Some(op) => format!("{}…", op.label()),
                None => main_op.label().to_string(),
            }))
            .child(counts)
            .when(!pull_busy, |el| el.on_click(git_op(main_op)));
        git = git.child(split(
            main,
            caret("hub-pull-more", pull_open, cx)
                .test_support()
                .on_click(toggle(ui, |u| u.pull = !u.pull)),
            cx,
        ));
        if pull_open {
            for (id, op) in [
                ("hub-git-pull", GitOp::Pull),
                ("hub-git-push", GitOp::Push),
                ("hub-git-fetch", GitOp::Fetch),
            ] {
                git = git.child(
                    sub_row(id, false, cx)
                        .test_support()
                        .when(pull_busy, |el| el.opacity(0.5).cursor_default())
                        .child(label(op.label()))
                        .when(!pull_busy, |el| el.on_click(git_op(op))),
                );
            }
        }
    }
    if let Some(card) = m.pr.clone() {
        git_rows += 1;
        let (what, state) = card.chip_parts();
        let hue = crate::pr_card::verdict_color(&card, cx).unwrap_or(theme.muted_foreground);
        let url = card.url.clone();
        let pop = pop.clone();
        git = git.child(
            look::row("hub-pr", false, cx)
                .test_support()
                .child(icon(
                    if card.phase == crate::pr_card::Phase::Merged {
                        Lucide::GitMerge
                    } else {
                        Lucide::GitPullRequest
                    },
                    cx,
                ))
                .child(div().flex_none().child(what))
                .child(label(card.title.clone()).text_color(theme.muted_foreground))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(hue)
                        .child(state),
                )
                .when_some(url, |el, url| {
                    el.on_click(move |_, window, cx| {
                        close(&pop, window, cx);
                        cx.open_url(&url);
                    })
                }),
        );
    }
    if let Some(changes) = m.changes.clone() {
        git_rows += 1;
        let (ws, pop) = (ws.clone(), pop.clone());
        let (plus, minus) = match changes.split_once(' ') {
            Some((p, m)) if p.starts_with('+') => (Some(p.to_string()), Some(m.to_string())),
            _ => (None, None),
        };
        git = git.child(
            look::row("hub-changes", false, cx)
                .test_support()
                .child(icon(Lucide::GitCompare, cx))
                .child(label("Changes"))
                .map(|el| match (plus, minus) {
                    (Some(p), Some(m)) => el.child(
                        h_flex()
                            .flex_none()
                            .gap(px(5.))
                            .text_size(px(12.))
                            .child(div().text_color(crate::palette::emerald(cx)).child(p))
                            .child(div().text_color(crate::palette::red(cx)).child(m)),
                    ),
                    _ => el.child(trailing(changes.clone(), cx)),
                })
                .on_click(move |_, window, cx| {
                    close(&pop, window, cx);
                    ws.update(cx, |_, cx| {
                        cx.emit(WorkspaceEvent::OpenTool(PanelTool::Git));
                        activate(cx);
                    });
                }),
        );
    }
    if git_rows > 0 {
        sections.push(
            v_flex()
                .child(header("Git", cx))
                .child(git)
                .into_any_element(),
        );
    }

    // ----- This thread -----
    if m.thread.is_some() {
        let mut this = v_flex();
        let mut rows = 0;
        if let Some((used, window)) = m.context {
            rows += 1;
            let frac = crate::context_popover::fraction(used, window);
            let (ws, pop, scope) = (ws.clone(), pop.clone(), scope.clone());
            this = this.child(
                look::row("hub-context", false, cx)
                    .test_support()
                    .child(
                        div()
                            .size(px(ICON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::context_popover::ring(used, window, cx)),
                    )
                    .child(label("Context"))
                    .child(trailing(
                        format!(
                            "{:.0}% · {} of {}",
                            frac * 100.,
                            crate::context_popover::fmt_tokens(used),
                            crate::context_popover::fmt_tokens(window)
                        ),
                        cx,
                    ))
                    .on_click(move |_, window, cx| {
                        close(&pop, window, cx);
                        let scope = scope.clone();
                        ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenContextCard(scope)));
                    }),
            );
        }
        if let Some((agent, limit)) = m.usage.clone() {
            rows += 1;
            let left = (100. - limit.percent).clamp(0., 100.);
            let resets = limit
                .resets_at
                .map(|r| format!(" · resets {}", crate::time::until(r)))
                .unwrap_or_default();
            let tint = match limit.percent {
                p if p >= 95. => crate::palette::red(cx),
                p if p >= 80. => crate::palette::amber(cx),
                _ => theme.muted_foreground,
            };
            this = this.child(
                plain_row("hub-usage")
                    .test_support()
                    .child(
                        div()
                            .size(px(ICON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::ui::agent_logo(&agent, px(12.), cx)),
                    )
                    .child(label("Usage"))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(tint)
                            .child(format!("{left:.0}% left{resets}")),
                    )
                    .tooltip({
                        let tip = format!("{} · {}", agent.display_name(), limit.label);
                        move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                .build(window, cx)
                        }
                    }),
            );
        }
        if !m.agents.is_empty() {
            rows += 1;
            let count = |s: fn(TaskState) -> bool| m.agents.iter().filter(|a| s(a.2)).count();
            let running = count(|s| s == TaskState::Running);
            let waiting = count(|s| s == TaskState::NeedsYou);
            let done = count(|s| s == TaskState::Done);
            let failed = count(|s| matches!(s, TaskState::Failed | TaskState::Cancelled));
            let summary = [
                (running, "running"),
                (waiting, "waiting"),
                (done, "done"),
                (failed, "stopped"),
            ]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} {w}"))
            .collect::<Vec<_>>()
            .join(" · ");
            this = this.child(
                look::row("hub-subagents", agents_open, cx)
                    .test_support()
                    .child(icon(Lucide::Users, cx))
                    .child(label(format!("Sub-agents · {}", m.agents.len())))
                    .child(trailing(summary, cx))
                    .on_click(toggle(ui, |u| u.agents = !u.agents)),
            );
            if agents_open {
                for (i, (id, title, state)) in m.agents.iter().enumerate() {
                    let (ws, pop, id) = (ws.clone(), pop.clone(), id.clone());
                    this = this.child(
                        sub_row(("hub-subagent", i), false, cx)
                            .test_support()
                            .child(label(title.clone()))
                            .child(trailing(state.label(), cx))
                            .on_click(move |_, window, cx| {
                                close(&pop, window, cx);
                                let id = id.clone();
                                ws.update(cx, |ws, cx| LineageRow::go(ws, &id, from_window, cx));
                            }),
                    );
                }
            }
        }
        if rows > 0 {
            sections.push(
                v_flex()
                    .child(header("This thread", cx))
                    .child(this)
                    .into_any_element(),
            );
        }
    }

    // ----- Sources -----
    if !m.sources.is_empty() {
        let n = m.sources.len();
        let shown = if all_sources { n } else { n.min(SOURCES) };
        let mut list = v_flex().child(header("Sources", cx));
        for (i, s) in m.sources.iter().take(shown).enumerate() {
            let pop = pop.clone();
            let (glyph, name, detail, s2) = match s {
                Source::File { path, verb } => {
                    let image = crate::image_preview::showable(path);
                    (
                        if image {
                            Lucide::Image
                        } else if *verb == "Edited" {
                            Lucide::Pencil
                        } else {
                            Lucide::FileText
                        },
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.display().to_string()),
                        verb.to_string(),
                        s.clone(),
                    )
                }
                Source::Link { url } => (
                    Lucide::Globe,
                    url.split("://")
                        .nth(1)
                        .unwrap_or(url)
                        .trim_end_matches('/')
                        .to_string(),
                    "Fetched".to_string(),
                    s.clone(),
                ),
            };
            let tip = s.key();
            list = list.child(
                look::row(("hub-source", i), false, cx)
                    .test_support()
                    .child(icon(glyph, cx))
                    .child(label(name))
                    .child(trailing(detail, cx))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(move |_, window, cx| {
                        close(&pop, window, cx);
                        open_source(&s2, window, cx);
                    }),
            );
        }
        if n > SOURCES {
            list = list.child(
                look::row("hub-sources-all", false, cx)
                    .test_support()
                    .text_color(theme.muted_foreground)
                    .child(div().w(px(ICON)).flex_none())
                    .child(label(if all_sources {
                        "Show fewer".to_string()
                    } else {
                        format!("View all {n}")
                    }))
                    .on_click(toggle(ui, |u| u.sources = !u.sources)),
            );
        }
        sections.push(list.into_any_element());
    }

    // ----- Lineage -----
    if !m.lineage.is_empty() {
        let mut list = v_flex().child(header("Lineage", cx));
        for (i, l) in m.lineage.iter().enumerate() {
            let (ws, pop) = (ws.clone(), pop.clone());
            let row = match l.open {
                Some(_) => look::row(("hub-lineage", i), false, cx),
                None => plain_row(("hub-lineage", i)),
            };
            list = list.child(
                row.test_support()
                    .child(icon(
                        if l.open.is_some() {
                            Lucide::GitFork
                        } else {
                            Lucide::ArrowLeftRight
                        },
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .py(px(3.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme.muted_foreground)
                                    .child(l.label.clone()),
                            )
                            .child(div().min_w_0().truncate().child(l.title.clone())),
                    )
                    .children(l.state.clone().map(|s| trailing(s, cx)))
                    .map(|el| match l.open.clone() {
                        Some(id) => el.on_click(move |_, window, cx| {
                            close(&pop, window, cx);
                            let id = id.clone();
                            ws.update(cx, |ws, cx| LineageRow::go(ws, &id, from_window, cx));
                        }),
                        None => el,
                    }),
            );
        }
        sections.push(list.into_any_element());
    }

    let count = sections.len();
    look::surface(cx)
        .id("hub-card")
        .test_support()
        .w(px(WIDTH))
        .max_h(max_h.max(px(160.)))
        .overflow_y_scroll()
        .children(sections.into_iter().enumerate().flat_map(move |(i, s)| {
            let mut out = vec![];
            if i > 0 && i < count {
                out.push(rule(cx).into_any_element());
            }
            out.push(s);
            out
        }))
        .into_any_element()
}

/// Open what the thread used as Trek opens it elsewhere: an image in the preview, a folder or
/// another file with the system, a page in the browser.
fn open_source(s: &Source, window: &mut Window, cx: &mut App) {
    match s {
        Source::File { path, .. } if !path.exists() => crate::toast::push(
            window,
            format!("{} isn't there any more", path.display()),
            cx,
        ),
        Source::File { path, .. } if crate::image_preview::showable(path) => {
            crate::image_preview::open(vec![path.clone()], 0, window, cx)
        }
        Source::File { path, .. } => cx.open_with_system(path),
        Source::Link { url } => cx.open_url(url),
    }
}

#[cfg(test)]
mod tests {
    use super::{Source, sources};
    use std::path::{Path, PathBuf};
    use trek_core::store::{Item, ToolStatus};

    fn tool(title: &str, detail: &str, status: ToolStatus) -> Item {
        Item::Tool {
            id: format!("{title}-{detail}"),
            title: title.into(),
            detail: detail.into(),
            output: String::new(),
            status,
        }
    }

    #[test]
    fn sources_are_newest_first_once_each_and_skip_failed_calls() {
        let items = vec![
            Item::User {
                text: "look at this".into(),
                images: vec!["/tmp/shot.png".into()],
                at: None,
                resume: None,
                aside: false,
            },
            tool("Read", "src/a.rs", ToolStatus::Done),
            tool("Fetch", "https://example.com/docs", ToolStatus::Done),
            tool("Edit", "src/a.rs, src/b.rs", ToolStatus::Done),
            tool("Read", "src/c.rs", ToolStatus::Failed),
            tool("Run command", "cargo test", ToolStatus::Done),
        ];
        let got = sources(&items, Some(Path::new("/p")));
        assert_eq!(
            got,
            vec![
                Source::File {
                    path: PathBuf::from("/p/src/a.rs"),
                    verb: "Edited"
                },
                Source::File {
                    path: PathBuf::from("/p/src/b.rs"),
                    verb: "Edited"
                },
                Source::Link {
                    url: "https://example.com/docs".into()
                },
                Source::File {
                    path: PathBuf::from("/tmp/shot.png"),
                    verb: "Attached"
                },
            ]
        );
    }
}
