//! The sidebar, inbox first (Figma "11 Sidebar options", Option 1). Under the nav, the Inbox: what
//! needs the user (an approval, a question, a plan, a failure, a sub-agent asking, failing checks,
//! a turn cut short when Trek closed), each saying why in its alert's words, then the threads that
//! finished unread; opening one reads it, "Mark all read" reads every finished one, and with
//! nothing in it the Inbox isn't there. A thread in the Inbox leaves its project's list while it's
//! there. Then each project under its badge with the rest of its threads, one line each (pinned,
//! then at work with its clock, then the quiet ones capped behind "Show N more"), Study's
//! sessions, the other machines and the phone (Devices), and settled history folded at the end,
//! grouped by project. Every row starts with the same state column and ends on the same
//! right-hand column (`look`), so state, badges and glyphs line up at the left and counts and
//! times at the right.

mod look;

use crate::fluent::{self, PillExt as _};
use crate::palette;
use crate::scale::{radius, text, to_rems};
use crate::time;
use crate::ui;
use crate::workspace::attention::Why;
use crate::workspace::{ItemRef, Route, SettingsPage, UpdateStatus, Workspace};
use crate::worktree_ui::Leave;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use look::{Mark, TEXT_INSET};
use std::collections::{HashMap, HashSet};
use trek_core::store::{SearchHit, Section, Thread};
use trek_core::{RunState, ThreadSource};

pub struct Sidebar {
    workspace: Entity<Workspace>,
    search: Entity<InputState>,
    project_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    /// The window is frontmost; spinners hold still when it isn't.
    active: bool,
    open_projects: HashSet<String>,
    /// The live groups (quiet threads by project) that are open: apart from `open_projects`,
    /// which opens the settled ones.
    open_live_projects: HashSet<String>,
    /// Live and settled project groups the user folded away (session state, like the open sets).
    collapsed_live: HashSet<String>,
    collapsed_settled: HashSet<String>,
    filter_open: bool,
    usage_open: bool,
    /// The Usage card lists every provider to pick the ones it shows.
    usage_picking: bool,
    /// A fourth provider was checked: the hint says why it wasn't.
    usage_refused: bool,
    updater_open: bool,
    /// The agent updates card is open.
    agent_updates_open: bool,
    /// Re-renders once a second while a turn runs, so "4m 12s" counts up, and once a minute
    /// otherwise, so "5m" ages (this view is cached; nothing else would redraw it). The flag is
    /// whether it's the fast one.
    _clock: Option<(bool, Task<()>)>,
    /// Which threads have sub-agents, wait on them, or have one waiting on the user: worked out
    /// once as the list is drawn, not for each of its rows (each answer reads every thread).
    graph: Graph,
    /// Why each thread that needs the user does, as of the last render (`Workspace::needing_you`).
    why: HashMap<String, Why>,
    /// The pointer is over the sidebar: the live rows hold still (see `render`).
    hovered: bool,
    /// The live rows as last drawn, held while `hovered`.
    shown: Option<Vec<LiveGroup>>,
    /// The held rows aren't what's current: they're redrawn when the pointer leaves.
    deferred: bool,
    /// The list's layout animation: rows that move between frames slide to their new places,
    /// new ones fade in (`crate::motion`).
    flip: crate::motion::FlipStore,
    /// Live rows whose thread has just gone (archived, deleted): drawn where they were while they
    /// fade, as the rows under them close the gap (AnimatePresence's "pop layout").
    ghosts: Vec<Ghost>,
    /// The live rows last drawn, by thread id: the thread as it was, its group, and whether it
    /// was an Inbox row. What a ghost draws.
    last_rows: HashMap<String, (Thread, String, bool)>,
    /// A search was on last frame: the list it leaves (or comes to) takes its places at once.
    was_searching: bool,
    /// The study folder: a thread working there is a Study session (`look::is_study`).
    study_dir: std::path::PathBuf,
    _subscriptions: Vec<Subscription>,
}

/// A live row fading away after its thread left the list (see `Sidebar::ghosts`).
struct Ghost {
    id: String,
    pid: String,
    /// The row it was under in its group, if any: it's drawn after that one.
    after: Option<String>,
    thread: Thread,
    /// It was an Inbox row.
    card: bool,
    height: f32,
    /// 1 drawn in full, 0 gone.
    left: crate::motion::Spring,
}

/// How far a row's text sits in from its rounded hover box, on both sides: headings and rows
/// line up on it at the left, the right-hand column at the right.
const ROW_INSET: f32 = look::ROW_INSET;
/// What "New thread here" takes from a project's name while the pointer's on its header (the
/// button lies over the count, `HOVER_BUTTON_RIGHT` in from the header's edge).
const HOVER_BUTTON: f32 = 18.;
const HOVER_BUTTON_RIGHT: f32 = 4.;

/// The most a branch takes on a row: long ones (other apps' `someone/what-it-is`) leave the title
/// room to read. The tooltip has it whole.
const ROW_BRANCH_WIDTH: f32 = 110.;
/// The least a project's name keeps beside why a thread is in the Inbox (it gives way first).
const PROJECT_MIN: f32 = 56.;

/// The Inbox's key among the live groups (no project id looks like it).
const INBOX: &str = "\u{0}inbox";
/// Study's sessions', after the projects (they're in none).
const STUDY: &str = "\u{0}study";
/// The "Projects" label's place among the live groups: over the first project's.
const PROJECTS_LABEL: &str = "live-projects-label";
/// What's only news in the Inbox (a turn cut short, a thread finished unread) on show before
/// "Show N more". What calls for the user never waits behind it.
const NEWS_SHOWN: usize = 5;
/// A project's quiet threads on show before "Show more" (unread ones and the open one always are).
const QUIET_SHOWN: usize = 3;
/// A machine's quiet threads on show in Devices before "Show more".
const MACHINE_SHOWN: usize = 3;

/// A project's group in the live list as drawn: its rows top to bottom (thread ids), and whether
/// a "Show more" / "Show less" line closes it.
#[derive(Clone, Debug, PartialEq)]
struct LiveGroup {
    pid: String,
    rows: Vec<String>,
    footer: bool,
}

/// What a live group's header and footer say, worked out with its rows.
#[derive(Default)]
struct GroupInfo {
    name: String,
    folded: bool,
    open: bool,
    /// Quiet rows behind "Show more".
    more: usize,
    /// Everything it holds, shown or not: its header's count.
    total: usize,
}

/// The threads' standing among their sub-agents, as of the last render (`Sidebar::graph`).
#[derive(Default)]
struct Graph {
    /// Threads with sub-agents in Trek's lists.
    parents: HashSet<String>,
    /// `Workspace::waiting_on_sub_agents`, and `Machines::sub_agents_need_you`.
    needs: HashSet<String>,
    /// `Workspace::waiting_threads`, and `Machines::waiting`.
    waiting: HashSet<String>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let project_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search projects…"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| {
                this.sync_clock(cx);
                cx.notify()
            }),
            // The cursor blinks and moves without an input event.
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.shown = None;
                    let q = state.read(cx).value().to_string();
                    this.workspace.update(cx, |ws, cx| ws.set_search(q, cx));
                }
            }),
        ];
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.observe(&project_search, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            this.active = window.is_window_active();
            // Gone to another app: nothing's under the pointer to hold still for.
            if !this.active {
                this.hovered = false;
            }
            cx.notify();
        }));
        let mut this = Self {
            workspace,
            search,
            project_search,
            clone_input,
            active: window.is_window_active(),
            open_projects: Default::default(),
            open_live_projects: Default::default(),
            collapsed_live: Default::default(),
            collapsed_settled: Default::default(),
            filter_open: false,
            // TREK_OPEN_USAGE=1 opens the Usage card at launch, for design review.
            usage_open: std::env::var_os("TREK_OPEN_USAGE").is_some(),
            usage_picking: false,
            usage_refused: false,
            updater_open: false,
            // TREK_OPEN_AGENT_UPDATES=1 opens the agent updates card at launch, the same way.
            agent_updates_open: std::env::var_os("TREK_OPEN_AGENT_UPDATES").is_some(),
            _clock: None,
            graph: Graph::default(),
            why: HashMap::new(),
            hovered: false,
            shown: None,
            deferred: false,
            flip: Default::default(),
            ghosts: vec![],
            last_rows: HashMap::new(),
            was_searching: false,
            study_dir: trek_core::study::dir(),
            _subscriptions: subscriptions,
        };
        this.sync_clock(cx);
        this
    }

    /// The pointer came onto the sidebar or left it. Leaving shows what changed while it was there.
    fn hover_changed(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hovered = hovered;
        if !hovered && self.deferred {
            cx.notify();
        }
    }

    /// How far the row for `key` (a thread id) is drawn from its place right now, while it
    /// slides there.
    #[cfg(any(test, feature = "test-support"))]
    pub fn row_offset(&self, key: &str, cx: &App) -> Option<f32> {
        self.flip.borrow().offset(key, crate::motion::now(cx))
    }

    /// The threads whose rows are fading away.
    #[cfg(any(test, feature = "test-support"))]
    pub fn ghosts(&self) -> Vec<String> {
        self.ghosts.iter().map(|g| g.id.clone()).collect()
    }

    /// Let go of the held rows: what the user just did here shows at once.
    fn thaw(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
        cx.notify();
    }

    fn sync_clock(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let running = ws.any_turn_running() || ws.machines.any_at_work();
        if self
            ._clock
            .as_ref()
            .is_some_and(|(fast, _)| *fast == running)
        {
            return;
        }
        let every = std::time::Duration::from_secs(if running { 1 } else { 60 });
        self._clock = Some((
            running,
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(every).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }),
        ));
    }

    fn top(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let filtering = self.workspace.read(cx).project_filter.is_some();
        let filter_open = self.filter_open;
        let this = cx.entity();
        let filter = Popover::new("project-filter")
            .anchor(Anchor::TopLeft)
            .appearance(false)
            .open(filter_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.filter_open = *open;
                cx.notify();
            }))
            .trigger(
                ui::icon_button(
                    "filter-projects",
                    IconName::Folder,
                    "Filter threads by project",
                )
                .selected(filtering),
            )
            .content(move |_, _, cx| this.update(cx, |this, cx| this.project_menu(cx)));
        let add = ui::icon_button(
            "add-project",
            crate::assets::Lucide::FolderPlus,
            "Add project",
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, {
            let ws = self.workspace.clone();
            let sidebar = cx.entity();
            move |menu, _, _| {
                let ws2 = ws.clone();
                let ws = ws.clone();
                let sb = sidebar.clone();
                menu.min_w(px(200.))
                    .item(
                        PopupMenuItem::new("Open folder…")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))),
                    )
                    .item(
                        PopupMenuItem::new("Clone from GitHub…")
                            .icon(IconName::Github)
                            .on_click(move |_, window, cx| {
                                sb.update(cx, |s, cx| s.open_clone_dialog(window, cx))
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("New thread without a project")
                            .icon(crate::assets::Lucide::MessageSquarePlus)
                            .on_click({
                                let ws = ws2.clone();
                                move |_, _, cx| {
                                    ws.update(cx, |ws, cx| {
                                        ws.navigate(Route::Draft { project: None }, cx)
                                    })
                                }
                            }),
                    )
            }
        });
        let ws = self.workspace.read(cx);
        let at_basecamp = ws.route == Route::Basecamp;
        let waiting = ws.ready_for_review().len();
        let basecamp = ui::nav_row(
            "open-basecamp",
            Icon::new(crate::assets::Lucide::Tent),
            "Basecamp",
            None,
            at_basecamp,
            cx,
        )
        .test_support()
        .when(waiting > 0, |el| {
            el.child(
                div()
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground)
                    .child(waiting.to_string()),
            )
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared("Basecamp (⌘⇧H)"))
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.workspace
                .update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx))
        }));
        let at_notes = self.workspace.read(cx).route == Route::Notes;
        let notes = ui::nav_row(
            "open-notes",
            Icon::new(crate::assets::Lucide::NotebookPen),
            "Notes",
            None,
            at_notes,
            cx,
        )
        .test_support()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared(
                "Things to jot down (⌘⇧J)", // keys::shared
            ))
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.workspace
                .update(cx, |ws, cx| ws.navigate(Route::Notes, cx))
        }));
        let at_study = self.workspace.read(cx).route == Route::Study;
        let study = ui::nav_row(
            "open-study",
            Icon::new(crate::assets::Lucide::GraduationCap),
            "Study",
            None,
            at_study,
            cx,
        )
        .test_support()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared(
                "Learn anything with a tutor (⌘⇧U)", // keys::shared
            ))
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.workspace
                .update(cx, |ws, cx| ws.navigate(Route::Study, cx))
        }));
        v_flex()
            .child(
                h_flex()
                    .px_3()
                    .pt_1()
                    .pb_2()
                    .gap(px(2.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.search).small().appearance(false).prefix(
                                Icon::new(IconName::Search)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            ),
                        ),
                    )
                    .child(filter)
                    .child(add)
                    .child(
                        ui::icon_button(
                            "new-thread",
                            crate::assets::Lucide::SquarePen,
                            "New thread (⌘N)", // icon_button( localizes
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| ws.new_thread(cx))
                        })),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .child(basecamp)
                    .child(notes)
                    .child(study),
            )
    }

    fn project_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let q = self.project_search.read(cx).value().to_lowercase();
        let current = ws.project_filter.clone();
        let projects: Vec<(String, String, Option<String>, std::path::PathBuf)> = ws
            .workspace_projects()
            .into_iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || p.remote
                        .as_deref()
                        .is_some_and(|r| r.to_lowercase().contains(&q))
            })
            .map(|p| {
                (
                    p.id.clone(),
                    p.name.clone(),
                    p.remote.clone(),
                    p.path.clone(),
                )
            })
            .collect();
        fn pick(
            id: Option<String>,
            cx: &mut Context<Sidebar>,
        ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
            cx.listener(move |this: &mut Sidebar, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.workspace.update(cx, |ws, cx| {
                    ws.project_filter = id;
                    cx.notify();
                });
                this.filter_open = false;
                this.thaw(cx);
            })
        }
        ui::menu_surface(cx)
            .w(px(280.))
            .child(
                div().px(px(6.)).pb(px(4.)).child(
                    Input::new(&self.project_search).small().prefix(
                        Icon::new(IconName::Search)
                            .small()
                            .text_color(theme.muted_foreground),
                    ),
                ),
            )
            .child(
                ui::menu_row("pf-all", current.is_none(), cx)
                    .child(
                        Icon::new(IconName::Folder)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child("All projects")
                    .on_click(pick(None, cx)),
            )
            .child(
                ui::menu_row(
                    "pf-none",
                    current.as_deref() == Some(crate::workspace::NO_PROJECT),
                    cx,
                )
                .child(
                    Icon::new(crate::assets::Lucide::MessageSquare)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child("No project")
                .on_click(pick(Some(crate::workspace::NO_PROJECT.to_string()), cx)),
            )
            .child(
                v_flex()
                    .id("pf-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .children(projects.into_iter().map(|(id, name, remote, path)| {
                        let label = remote.clone().unwrap_or_else(|| name.clone());
                        let ws = self.workspace.clone();
                        let gear_id = id.clone();
                        ui::menu_row(
                            SharedString::from(format!("pf-{id}")),
                            current.as_ref() == Some(&id),
                            cx,
                        )
                        .group("pf-row")
                        .child(ui::project_badge(
                            &name,
                            &self.workspace.read(cx).project_look(&path),
                            cx,
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            gpui_kit::component::button::Button::new(SharedString::from(format!(
                                "pf-gear-{id}"
                            )))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Settings).text_color(theme.muted_foreground))
                            .dropdown_menu_with_anchor(
                                Anchor::TopRight,
                                move |menu, _, _| {
                                    let (p1, p2, ws2, ws3, id3) = (
                                        path.clone(),
                                        path.clone(),
                                        ws.clone(),
                                        ws.clone(),
                                        gear_id.clone(),
                                    );
                                    menu.item(PopupMenuItem::new("New thread here").on_click(
                                        move |_, _, cx| {
                                            let p = p2.clone();
                                            ws2.update(cx, |ws, cx| {
                                                ws.navigate(Route::Draft { project: Some(p) }, cx)
                                            })
                                        },
                                    ))
                                    .item(
                                        PopupMenuItem::new(
                                            crate::words::words().show_in_file_manager,
                                        )
                                        .on_click(move |_, _, cx| cx.reveal_path(&p1)),
                                    )
                                    .item(
                                        PopupMenuItem::new("Project settings").on_click(
                                            move |_, _, cx| {
                                                let id = id3.clone();
                                                ws3.update(cx, |ws, cx| {
                                                    ws.open_project_settings(Some(id), cx)
                                                })
                                            },
                                        ),
                                    )
                                },
                            ),
                        )
                        .on_click(pick(Some(id.clone()), cx))
                    })),
            )
            .into_any_element()
    }

    fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.clone_input.clone();
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let input2 = input.clone();
            let ws = ws.clone();
            dialog
                .title("Clone from GitHub")
                .child(
                    v_flex()
                        .gap_2()
                        .child("Uses your GitHub CLI login. Clones into ~/Developer.")
                        .child(Input::new(&input)),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(
                            gpui_kit::component::dialog::DialogClose::new().child(
                                gpui_kit::component::button::Button::new("cancel-clone")
                                    .outline()
                                    .label("Cancel"),
                            ),
                        )
                        .child(
                            gpui_kit::component::dialog::DialogAction::new().child(
                                gpui_kit::component::button::Button::new("do-clone")
                                    .primary()
                                    .label("Clone")
                                    .on_click(move |_, _, cx| {
                                        let spec = input2.read(cx).value().to_string();
                                        ws.update(cx, |ws, cx| ws.clone_repo(spec, cx));
                                    }),
                            ),
                        ),
                )
        });
    }

    /// Why a thread is in the Inbox, for its row's second line: the words its alert used, in its
    /// run state's colour (its mark is in the state column), with the way back for a turn a quit
    /// cut short and the offer to send failing checks to its agent. A row held in its place under
    /// the pointer after its thread stopped needing anything says where it stands now, or nothing.
    fn status(&self, t: &Thread, cx: &App) -> (Mark, Option<AnyElement>) {
        let theme = cx.theme();
        let ws = self.workspace.read(cx);
        // The words alone: the mark that goes with them is in the row's state column.
        let words = |id: String, color: Hsla, text: String| {
            div()
                .id(SharedString::from(id))
                .test_support()
                .min_w_0()
                .truncate()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        // An action on the row itself: it does its one thing, and the row doesn't open.
        let action = |id: String,
                      label: &'static str,
                      f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
            let (ws, tid) = (self.workspace.clone(), t.id.clone());
            div()
                .id(SharedString::from(id))
                .test_support()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(label)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    ws.update(cx, |ws, cx| f(ws, &tid, cx));
                })
        };
        // Cut off by a quit: what became of it, and on the row itself the way back.
        if let Some(offer) = ws.interrupted_offer(&t.id) {
            use crate::workspace::resume::ResumeState;
            let row = |state: AnyElement, label: Option<&'static str>| {
                h_flex()
                    .min_w_0()
                    .gap(px(6.))
                    .child(state)
                    .children(label.map(|label| {
                        action(format!("inbox-resume-{}", t.id), label, |ws, id, cx| {
                            ws.resume_interrupted(id, cx)
                        })
                    }))
                    .into_any_element()
            };
            let id = format!("inbox-interrupted-{}", t.id);
            match (&offer.state, offer.turn.waiting) {
                (ResumeState::Offered, false) => {
                    return (
                        Mark::NeedsYou,
                        Some(row(
                            words(id, palette::amber(cx), "Interrupted".into()),
                            Some("Resume"),
                        )),
                    );
                }
                (ResumeState::Offered, true) => {
                    return (
                        Mark::NeedsYou,
                        Some(words(id, palette::needs_you(cx), "Waiting for you".into())),
                    );
                }
                (ResumeState::Resuming, _) => {
                    return (
                        Mark::Working,
                        Some(words(id, palette::working(cx), "Resuming…".into())),
                    );
                }
                (ResumeState::Failed(_), _) => {
                    return (
                        Mark::Failed,
                        Some(row(
                            words(id, palette::failed(cx), "Couldn't resume".into()),
                            Some("Retry"),
                        )),
                    );
                }
                (ResumeState::Going, _) => {}
            }
        }
        // Why it needs the user, after the offer above: that one comes with the way back.
        if let Some(why) = self.why.get(&t.id) {
            // Failing checks come with theirs too.
            if matches!(why, Why::Checks(_)) {
                let sending = ws.pr_checks_offer(&t.id).is_some_and(|o| o.sending);
                return (
                    Mark::Failed,
                    Some(
                        h_flex()
                            .min_w_0()
                            .gap(px(6.))
                            .child(words(
                                format!("inbox-checks-failed-{}", t.id),
                                palette::failed(cx),
                                why.words(),
                            ))
                            .when(!sending, |el| {
                                el.child(action(
                                    format!("inbox-send-checks-{}", t.id),
                                    "Send to agent",
                                    |ws, id, cx| ws.pr_checks_send(id, cx),
                                ))
                            })
                            .into_any_element(),
                    ),
                );
            }
            return Self::why_line(t, why, cx);
        }
        // Held in place after it stopped needing anything: where it stands now.
        if t.run_state == RunState::Idle && self.graph.waiting.contains(&t.id) {
            return (
                Mark::Working,
                Some(words(
                    format!("inbox-waiting-{}", t.id),
                    palette::working(cx),
                    "Waiting on its sub-agents".into(),
                )),
            );
        }
        match t.run_state {
            RunState::Working => (
                Mark::Working,
                Some(words(
                    format!("inbox-working-{}", t.id),
                    palette::working(cx),
                    "Working".into(),
                )),
            ),
            // A machine out of reach: its thread as it last was, with nothing to say what for.
            RunState::NeedsYou => (
                Mark::NeedsYou,
                Some(words(
                    format!("inbox-needs-{}", t.id),
                    palette::needs_you(cx),
                    "Needs you".into(),
                )),
            ),
            RunState::Failed => (
                Mark::Failed,
                Some(words(
                    format!("inbox-failed-{}", t.id),
                    palette::failed(cx),
                    "Failed".into(),
                )),
            ),
            RunState::Idle if t.is_unseen() => (Mark::Unread, None),
            RunState::Idle => (Mark::None, None),
        }
    }

    /// Why a thread needs the user, on its Inbox row's second line: the words its alert used, in
    /// its run state's colour (its mark is in the state column); emerald for what only finished.
    /// Words too long for the row end in an ellipsis; the tooltip has them in full.
    fn why_line(t: &Thread, why: &Why, cx: &App) -> (Mark, Option<AnyElement>) {
        let (key, color, mark) = match why {
            Why::Failed(_) | Why::Checks(_) => ("failed", palette::failed(cx), Mark::Failed),
            Why::SubAgent => ("sub-needs", palette::needs_you(cx), Mark::NeedsYou),
            Why::Finished => ("finished", palette::emerald(cx), Mark::Unread),
            Why::CutShort => ("cut-short", palette::needs_you(cx), Mark::NeedsYou),
            _ => ("needs", palette::needs_you(cx), Mark::NeedsYou),
        };
        (
            mark,
            Some(
                div()
                    .id(SharedString::from(format!("inbox-{key}-{}", t.id)))
                    .test_support()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .child(why.words())
                    .into_any_element(),
            ),
        )
    }

    /// While searching, the message that put a thread in the list when its title didn't match.
    fn content_hit(&self, t: &Thread, cx: &App) -> Option<SearchHit> {
        let ws = self.workspace.read(cx);
        let q = ws.search.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        ws.search_results.content_hit(&q, t).cloned()
    }

    /// Open a thread from the list; a thread found by its messages opens at the match.
    fn open(&mut self, id: String, hit: Option<SearchHit>, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |ws, cx| match hit.as_ref().and_then(ItemRef::of_hit) {
                Some(at) => ws.open_thread_at(&id, at, cx),
                None => ws.navigate(Route::Thread(id), cx),
            })
    }

    /// The matching words of a content hit, on one quiet line.
    fn hit_line(hit: &SearchHit, cx: &App) -> Div {
        let (text, ranges) = ui::lead_to_match(&hit.snippet, &hit.ranges, 12);
        div()
            .min_w_0()
            .truncate()
            .text_size(px(text::LABEL))
            .text_color(cx.theme().muted_foreground)
            .child(ui::match_text(&text, &ranges, cx))
    }

    /// What `t` has at work: its sub-agents (Trek's, and its agent's own, in its turn or working
    /// in the background), by logo and name, and what else its agent runs in the background.
    pub fn at_work(
        &self,
        t: &Thread,
        cx: &App,
    ) -> (Vec<(trek_core::AgentId, String)>, Vec<String>) {
        let ws = self.workspace.read(cx);
        if crate::machines::is_remote(&t.id) {
            return ws.machines.at_work(&t.id);
        }
        let children = if self.graph.parents.contains(&t.id) {
            ws.running_children(&t.id)
        } else {
            vec![]
        };
        let mut kids: Vec<(trek_core::AgentId, String)> = children
            .into_iter()
            .map(|c| {
                (
                    c.agent.clone(),
                    format!("{}: {}", ws.model_label(c), c.title),
                )
            })
            .collect();
        let Some(l) = ws.live.get(&t.id) else {
            return (kids, vec![]);
        };
        let out = |id: &str| l.tasks.iter().any(|k| k.id == id && k.done.is_none());
        kids.extend(
            l.tasks
                .iter()
                .filter(|k| k.done.is_none())
                .map(|k| (t.agent.clone(), k.description.clone())),
        );
        kids.extend(
            l.background_agents()
                .filter(|b| !b.task.call.as_deref().is_some_and(out))
                .map(|b| (t.agent.clone(), b.task.title.clone())),
        );
        (
            kids,
            l.background_work().map(|b| b.task.title.clone()).collect(),
        )
    }

    /// A row's tooltip: who runs it ("Claude Code · Opus 5.5"; a machine's thread as the machine
    /// names its model), why it needs the user, whole (the row may have cut it short), then what
    /// it has at work.
    fn runs_tip(
        &self,
        t: &Thread,
        kids: &[(trek_core::AgentId, String)],
        background: &[String],
        cx: &App,
    ) -> SharedString {
        let ws = self.workspace.read(cx);
        let provider = t.agent.display_name();
        let model = ws
            .machines
            .model_label(&t.id)
            .unwrap_or_else(|| ws.model_label(t));
        let runs = if model == provider {
            provider
        } else {
            format!("{provider} · {model}")
        };
        let runs = match self.why.get(&t.id) {
            Some(why) => format!("{}\n\n{runs}", why.words()),
            None => runs,
        };
        SharedString::from(match card_tip(kids, background) {
            Some(rest) => format!("{runs}\n\n{rest}"),
            None => runs,
        })
    }

    /// A thread in the Inbox, on two lines: its mark (amber or red for what needs the user,
    /// emerald for what finished unread), its title in bold and how long ago, then why it's here
    /// in its alert's words and its project ("Finished · Trek").
    fn inbox_row(
        &self,
        t: &Thread,
        project: &str,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (kids, background) = self.at_work(t, cx);
        let tip = self.runs_tip(t, &kids, &background, cx);
        let (mark, words) = self.status(t, cx);
        let listed = self.why.contains_key(&t.id);
        let id = t.id.clone();
        let shown_title = if look::is_study(t, &self.study_dir) {
            look::study_title(&t.title).0.to_string()
        } else {
            t.title.clone()
        };
        let title = h_flex()
            .h(px(20.))
            .gap(px(look::MARK_GAP))
            .child(look::mark(None, mark, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(listed, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(ui::title_text(
                        SharedString::from(format!("inbox-title-{}", t.id)),
                        &shown_title,
                        self.workspace.read(cx).title_reveal(&t.id),
                        theme.foreground,
                        cx,
                    )),
            )
            .child(look::trailing(
                Some(format!("inbox-time-{}", t.id)),
                time::relative(t.updated_at),
                theme.muted_foreground,
            ));
        // Why it's here, then where it's from. The project gives way first, down to a few letters
        // ("· FMCET M…"): why is what the row is for.
        let says_why = words.is_some();
        let place = h_flex()
            .h(px(16.))
            .pl(px(look::MARK_W + look::MARK_GAP))
            .gap(px(4.))
            .min_w_0()
            .text_size(px(look::META_SIZE))
            .children(words)
            .child(
                div()
                    .when(says_why, |el| el.min_w(px(PROJECT_MIN)).flex_shrink(100.))
                    .when(!says_why, |el| el.min_w_0())
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(if says_why {
                        format!("· {project}")
                    } else {
                        project.to_string()
                    }),
            );
        let inset = look::ROW_INSET - look::INBOX_BORDER - look::INBOX_ROW_MX;
        let row = v_flex()
            .id(SharedString::from(format!("inbox-{}", t.id)))
            .test_support()
            .relative()
            .mx(px(look::INBOX_ROW_MX))
            .pl(px(inset))
            .pr(px(inset))
            .pt(px(5.))
            .pb(px(6.))
            .gap(px(1.))
            .rounded(px(fluent::control(look::ROW_RADIUS)))
            .cursor_pointer()
            .text_size(px(look::TITLE_SIZE))
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .children(fluent::selection_bar(selected, cx))
            .child(title)
            .child(place)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), None, cx)));
        let row = self.draggable(row, t, cx);
        self.with_menu(row, t, cx).into_any_element()
    }

    /// Codex-style compact row for settled history, dimmer than live work's, its age in the
    /// right-hand column. One whose sub-agents or background work are still going (settled, it
    /// stays here, as a working thread would) has a quiet dot in its state column that names them
    /// on hover; a Study session has its cap.
    fn line(&self, t: &Thread, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        // Only a thread that ran in this process can have anything going.
        let tip = if self.workspace.read(cx).live.contains_key(&t.id) {
            let (kids, background) = self.at_work(t, cx);
            card_tip(&kids, &background).map(SharedString::from)
        } else {
            None
        };
        let study = look::is_study(t, &self.study_dir);
        let (mark, mark_id) = if tip.is_some() {
            (Mark::Background, Some(format!("line-at-work-{}", t.id)))
        } else if study {
            (Mark::Study, None)
        } else {
            (Mark::None, None)
        };
        let shown_title = if study {
            look::study_title(&t.title).0.to_string()
        } else {
            t.title.clone()
        };
        let title = h_flex()
            .h(px(look::LINE_H))
            .gap(px(look::MARK_GAP))
            .child(look::mark(mark_id.map(SharedString::from), mark, cx))
            .child(div().flex_1().min_w_0().child(ui::title_text(
                SharedString::from(format!("line-title-{}", t.id)),
                &shown_title,
                self.workspace.read(cx).title_reveal(&t.id),
                if selected {
                    theme.foreground
                } else {
                    theme.foreground.opacity(look::SETTLED)
                },
                cx,
            )))
            .child(look::trailing(
                Some(format!("line-time-{}", t.id)),
                time::relative(t.updated_at),
                theme.muted_foreground.opacity(0.8),
            ));
        let row = v_flex()
            .id(SharedString::from(format!("line-{}", t.id)))
            .test_support()
            .relative()
            .mx_2()
            .px(px(ROW_INSET))
            .rounded(px(fluent::control(look::ROW_RADIUS)))
            .cursor_pointer()
            .text_size(px(look::TITLE_SIZE))
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .children(fluent::selection_bar(selected, cx))
            .child(title)
            .when_some(hit.clone(), |el, h| {
                el.child(
                    Self::hit_line(&h, cx)
                        .pl(px(look::MARK_W + look::MARK_GAP))
                        .mt(px(-5.))
                        .pb(px(6.)),
                )
            })
            .when_some(tip, |el, tip| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        let row = self.draggable(row, t, cx);
        self.with_menu(row, t, cx).into_any_element()
    }

    /// A thread in its project (in Study, on its device), on one line. Its state column: the
    /// ember loader while it's at work (its turn, its sub-agents, a resume), amber or red for a
    /// turn a quit cut off, a sky dot while only its background work runs, emerald while unread,
    /// its pin or clock, Study's cap. Its title (full strength while at work, open or unread;
    /// medium while unread), its worktree and its sub-agents' pill. Then the right-hand column:
    /// how long it's been at work in ember, the way back after a quit, "Paused" at a usage limit,
    /// a session's kind, or how long ago it last changed.
    fn live_line(&self, t: &Thread, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        use crate::workspace::resume::ResumeState;
        /// Where a turn a quit cut off stands.
        enum Offer {
            Resume,
            Waiting,
            Resuming,
            Retry,
        }
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        let (kids, background) = self.at_work(t, cx);
        let unseen = t.is_unseen();
        let pinned = t.pinned_at.is_some();
        let snoozed = t
            .snoozed_until
            .is_some_and(|u| u > trek_core::store::now_ms());
        let paused = t.paused.is_some() && t.run_state == RunState::Idle;
        let waiting = t.run_state == RunState::Idle && self.graph.waiting.contains(&t.id);
        let study = look::is_study(t, &self.study_dir);
        let (shown_title, kind) = if study {
            let (title, kind) = look::study_title(&t.title);
            (title.to_string(), kind)
        } else {
            (t.title.clone(), None)
        };
        let offer = ws
            .interrupted_offer(&t.id)
            .and_then(|o| match (&o.state, o.turn.waiting) {
                (ResumeState::Offered, false) => Some(Offer::Resume),
                (ResumeState::Offered, true) => Some(Offer::Waiting),
                (ResumeState::Resuming, _) => Some(Offer::Resuming),
                (ResumeState::Failed(_), _) => Some(Offer::Retry),
                (ResumeState::Going, _) => None,
            });
        let working =
            t.run_state == RunState::Working || waiting || matches!(offer, Some(Offer::Resuming));
        // One column for how it stands: what's running first, then what's asked of the user, then
        // news, then what it is.
        let at_work_id = || Some(format!("live-line-at-work-{}", t.id));
        let (mark, mark_id) = if working {
            (Mark::Working, None)
        } else if !kids.is_empty() {
            (Mark::Working, at_work_id())
        } else if matches!(offer, Some(Offer::Resume | Offer::Waiting))
            || t.run_state == RunState::NeedsYou
        {
            (Mark::NeedsYou, None)
        } else if matches!(offer, Some(Offer::Retry)) || t.run_state == RunState::Failed {
            (Mark::Failed, None)
        } else if !background.is_empty() {
            (Mark::Background, at_work_id())
        } else if unseen {
            (Mark::Unread, Some(format!("live-line-unseen-{}", t.id)))
        } else if pinned {
            (Mark::Pinned, Some(format!("live-line-pinned-{}", t.id)))
        } else if snoozed {
            (Mark::Snoozed, Some(format!("live-line-snoozed-{}", t.id)))
        } else if study {
            (Mark::Study, None)
        } else {
            (Mark::None, None)
        };
        let busy = mark == Mark::Working;
        // The right-hand column.
        let ember = palette::working(cx);
        let link = |label: &'static str| {
            let (ws, tid) = (self.workspace.clone(), t.id.clone());
            div()
                .id(SharedString::from(format!("live-line-resume-{}", t.id)))
                .test_support()
                .flex_none()
                .text_size(px(look::LABEL_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(label)
                .on_click(move |_, _, cx| {
                    // The row opens the thread; this only resumes it.
                    cx.stop_propagation();
                    ws.update(cx, |ws, cx| ws.resume_interrupted(&tid, cx));
                })
                .into_any_element()
        };
        let end = match offer {
            Some(Offer::Resume) => link("Resume"),
            Some(Offer::Retry) => link("Retry"),
            Some(Offer::Resuming) => look::trailing(
                Some(format!("live-line-working-{}", t.id)),
                "Resuming…",
                ember,
            ),
            _ if t.run_state == RunState::Working => {
                // A machine's thread not open here counts from when its row says it started. The
                // loader holds still and the clock ticks: a spinning one would redraw the whole
                // sidebar every frame.
                let clock = ws
                    .live
                    .get(&t.id)
                    .and_then(|l| l.turn_started)
                    .map(|s| s.elapsed())
                    .or_else(|| ws.machines.working_for(&t.id));
                look::trailing(
                    Some(format!("live-line-working-{}", t.id)),
                    match clock {
                        Some(d) => time::elapsed(d),
                        None => crate::mascot::word(&t.id, 0).to_string(),
                    },
                    ember,
                )
            }
            // Its turn is over but its sub-agents are still out: how long it's waited on them.
            _ if waiting => {
                let longest = ws
                    .waiting_on(&t.id)
                    .iter()
                    .map(|w| w.elapsed)
                    .max()
                    .or_else(|| ws.machines.waited_for(&t.id));
                look::trailing(
                    Some(format!("live-line-waiting-{}", t.id)),
                    match longest {
                        Some(d) => time::elapsed(d),
                        None => "Waiting".to_string(),
                    },
                    ember,
                )
            }
            _ if paused => look::trailing(
                Some(format!("live-line-paused-{}", t.id)),
                "Paused",
                palette::amber(cx),
            ),
            // Learn or Practice, always: it says what kind of session this is.
            _ if kind.is_some() => {
                look::trailing(None, kind.unwrap_or_default(), theme.muted_foreground)
            }
            _ => look::trailing(
                Some(format!("live-line-time-{}", t.id)),
                time::relative(t.updated_at),
                theme.muted_foreground,
            ),
        };
        let strong = selected || unseen || busy;
        let title = h_flex()
            .h(px(look::LINE_H))
            .gap(px(look::MARK_GAP))
            .child(look::mark(mark_id.map(SharedString::from), mark, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(unseen && !selected, |el| el.font_medium())
                    .child(ui::title_text(
                        SharedString::from(format!("live-line-title-{}", t.id)),
                        &shown_title,
                        ws.title_reveal(&t.id),
                        if strong {
                            theme.foreground
                        } else {
                            theme.foreground.opacity(look::QUIET)
                        },
                        cx,
                    )),
            )
            .when_some(t.worktree.as_ref(), |el, wt| {
                el.child(crate::worktree_ui::branch_chip_up_to(
                    SharedString::from(format!("live-line-branch-{}", t.id)),
                    wt,
                    ROW_BRANCH_WIDTH,
                    cx,
                ))
            })
            // Its sub-agents at work, in a pill of their own: the logo of each provider they run on,
            // then how many there are (the tooltip names each). (Overlapped logos read as a
            // smudge, and a ring the sidebar's colour shows under glass.)
            .when(!kids.is_empty(), |el| {
                el.child(
                    h_flex()
                        .id(SharedString::from(format!("live-line-kids-{}", t.id)))
                        .test_support()
                        .flex_none()
                        .h(px(16.))
                        .px(px(5.))
                        .gap(px(3.))
                        .rounded_pill()
                        .bg(theme.foreground.opacity(0.07))
                        .children(
                            kid_groups(&kids)
                                .into_iter()
                                .take(3)
                                .map(|(agent, _)| ui::agent_logo(&agent, px(10.), cx)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("live-line-kids-count-{}", t.id)))
                                .test_support()
                                .text_size(px(text::META))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.muted_foreground)
                                .child(kids.len().to_string()),
                        ),
                )
            })
            .child(end);
        // At work: who runs it and what it has out; otherwise only what still runs in the
        // background has anything to name.
        let tip = if busy || offer.is_some() {
            Some(self.runs_tip(t, &kids, &background, cx))
        } else {
            card_tip(&kids, &background).map(SharedString::from)
        };
        let row = v_flex()
            .id(SharedString::from(format!("live-line-{}", t.id)))
            .test_support()
            .relative()
            .mx_2()
            .px(px(ROW_INSET))
            .rounded(px(fluent::control(look::ROW_RADIUS)))
            .cursor_pointer()
            .text_size(px(look::TITLE_SIZE))
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .children(fluent::selection_bar(selected, cx))
            .child(title)
            .when_some(hit.clone(), |el, h| {
                el.child(
                    Self::hit_line(&h, cx)
                        .pl(px(look::MARK_W + look::MARK_GAP))
                        .mt(px(-5.))
                        .pb(px(6.)),
                )
            })
            .when_some(tip, |el, tip| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        let row = self.draggable(row, t, cx);
        self.with_menu(row, t, cx).into_any_element()
    }

    /// A project group's header (live threads and settled history): the project's badge in the
    /// state column, its name in the primary colour (the one row with a badge, so a project never
    /// reads as a section) and how many threads it holds in the right-hand column. The fold
    /// chevron follows the name (folded, always; open, to the pointer), and on hover "New thread
    /// here" lies over the count rather than keeping room for itself, so the count ends where
    /// every other row's right-hand column does. Wrapped in the project menu. Settled history's
    /// are muted. "No project" has a plain glyph for a badge.
    #[allow(clippy::too_many_arguments)]
    fn group_header(
        &self,
        prefix: &'static str,
        pid: &str,
        name: String,
        count: Option<usize>,
        fold: Option<bool>,
        paths: &HashMap<String, std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let settled = prefix == "settled";
        // A new thread in this project (or in none, for "No project"), from its header.
        let here = if pid.is_empty() {
            Some(None)
        } else {
            paths.get(pid).cloned().map(Some)
        };
        let badge = match paths.get(pid) {
            Some(path) => ui::project_badge(&name, &self.workspace.read(cx).project_look(path), cx),
            // Settled Study sessions: the cap, as on their rows.
            None if pid == STUDY => look::mark(None, Mark::Study, cx),
            None => Icon::new(crate::assets::Lucide::MessageSquare)
                .size(px(13.))
                .text_color(theme.muted_foreground)
                .into_any_element(),
        };
        let group = SharedString::from(format!("{prefix}-proj-group-{pid}"));
        // What "New thread here" takes from the name while it's on show (it lies over the count).
        let hover_room = if here.is_some() { HOVER_BUTTON } else { 0. };
        let header = h_flex()
            .id(SharedString::from(format!("{prefix}-proj-head-{pid}")))
            .test_support()
            .group(group.clone())
            .relative()
            .mx_2()
            .px(px(ROW_INSET))
            .h(px(look::LINE_H))
            .rounded(px(fluent::control(look::ROW_RADIUS)))
            .hover(|s| s.bg(theme.list_hover))
            .child(
                h_flex()
                    .id(SharedString::from(format!("{prefix}-fold-{pid}")))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(look::MARK_GAP))
                    .cursor_pointer()
                    .group_hover(group.clone(), |s| s.pr(px(hover_room)))
                    .child(
                        h_flex()
                            .flex_none()
                            .w(px(look::MARK_W))
                            .justify_center()
                            .when(settled, |el| el.opacity(0.7))
                            .child(badge),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(look::TITLE_SIZE))
                                    .when(!settled, |el| {
                                        el.font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.foreground)
                                    })
                                    .when(settled, |el| el.text_color(theme.muted_foreground))
                                    .child(name.clone()),
                            )
                            // Folded, the chevron says so; open, it shows only to the pointer.
                            .when_some(fold, |el, folded| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .when(!folded, |el| {
                                            el.invisible()
                                                .group_hover(group.clone(), |s| s.visible())
                                        })
                                        .child(
                                            Icon::new(if folded {
                                                IconName::ChevronRight
                                            } else {
                                                IconName::ChevronDown
                                            })
                                            .xsmall()
                                            .text_color(theme.muted_foreground),
                                        ),
                                )
                            }),
                    )
                    // How many it holds, in the right-hand column; "New thread here" lies over it
                    // on hover.
                    .when_some(count.filter(|n| *n > 0), |el, n| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("{prefix}-proj-count-{pid}")))
                                .test_support()
                                .flex_none()
                                .text_size(px(look::LABEL_SIZE))
                                .text_color(theme.muted_foreground)
                                .when(here.is_some(), |el| {
                                    el.group_hover(group.clone(), |s| s.invisible())
                                })
                                .child(n.to_string()),
                        )
                    })
                    .when(fold.is_some(), |el| {
                        let pid = pid.to_string();
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            let set = if prefix == "live" {
                                &mut this.collapsed_live
                            } else {
                                &mut this.collapsed_settled
                            };
                            if !set.remove(&pid) {
                                set.insert(pid.clone());
                            }
                            this.thaw(cx);
                        }))
                    }),
            )
            .when_some(here, |el, project| {
                let tip = if project.is_none() {
                    "New thread without a project"
                } else {
                    "New thread here"
                };
                el.child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(HOVER_BUTTON_RIGHT))
                        .items_center()
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .child(
                            ui::icon_button(
                                SharedString::from(format!("{prefix}-proj-new-{pid}")),
                                crate::assets::Lucide::SquarePen,
                                tip,
                            )
                            .xsmall()
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let project = project.clone();
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.navigate(Route::Draft { project }, cx)
                                    })
                                },
                            )),
                        ),
                )
            });
        match paths.get(pid) {
            Some(path) => self
                .with_project_menu(header, pid.to_string(), name, path.clone())
                .into_any_element(),
            None => header.into_any_element(),
        }
    }

    /// A thread's row can be dragged to the main area to open beside what's there
    /// (`root::split`). A click that slipped a little on its way down starts that drag, and lands
    /// back on its own row: it's still a click.
    fn draggable<E: StatefulInteractiveElement>(
        &self,
        row: E,
        t: &Thread,
        cx: &mut Context<Self>,
    ) -> E {
        let hit = self.content_hit(t, cx);
        let dragged = crate::root::split::DraggedThread {
            id: t.id.clone(),
            title: t.title.clone().into(),
        };
        let id = t.id.clone();
        row.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_drop(
                cx.listener(move |this, d: &crate::root::split::DraggedThread, _, cx| {
                    if d.id == id {
                        this.open(id.clone(), hit.clone(), cx);
                    }
                }),
            )
    }

    /// Right-click menu for a thread (T3's set): pin, settle, snooze, rename, copy, project, archive,
    /// delete. What the title bar's "⋯" has too comes from `thread_menu`, so the two agree.
    fn with_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(
        &self,
        row: E,
        t: &Thread,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::thread_menu;
        let ws = self.workspace.downgrade();
        let tid = t.id.clone();
        let title = t.title.clone();
        let settled = t.settled_at.is_some();
        let snoozed = t
            .snoozed_until
            .is_some_and(|u| u > trek_core::store::now_ms());
        let never_settle = t.never_settle;
        let imported = t.source != ThreadSource::Trek;
        let project = t.project_id.clone().and_then(|pid| {
            self.workspace
                .read(cx)
                .project(&pid)
                .map(|p| (p.id.clone(), p.name.clone()))
        });
        row.context_menu(move |menu, window, cx| {
            // Read as the menu opens: pinned, its folder, its sub-agents.
            let Some(f) = ws
                .upgrade()
                .and_then(|ws| thread_menu::Facts::read(ws.read(cx), &tid))
            else {
                return menu;
            };
            let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                let ws = ws.clone();
                let tid = tid.clone();
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                })
            };
            let mut menu = menu
                .min_w(px(210.))
                .item(thread_menu::open_in_new_window(&ws, &tid))
                .item(thread_menu::open_beside(&ws, &tid))
                .item(thread_menu::fork(&ws, &tid))
                .separator()
                .item(thread_menu::pin(&ws, &f))
                .item(if settled { item("Move to inbox", |ws, id, cx| ws.unsettle(id, cx)) } else { item("Settle thread", |ws, id, cx| ws.settle(id, cx)) });
            menu = menu.submenu("Snooze", window, cx, {
                let (ws, tid) = (ws.clone(), tid.clone());
                move |menu, _, _| {
                    let snooze = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                        })
                    };
                    let menu = menu
                        .item(snooze("For 1 hour", |ws, id, cx| ws.snooze(id, 1, cx)))
                        .item(snooze("For 3 hours", |ws, id, cx| ws.snooze(id, 3, cx)))
                        .item(snooze("Until tomorrow morning", |ws, id, cx| ws.snooze_until_morning(id, 1, cx)))
                        .item(snooze("Until next week", |ws, id, cx| ws.snooze_until_morning(id, 7, cx)));
                    if snoozed { menu.separator().item(snooze("Wake now", |ws, id, cx| ws.unsnooze(id, cx))) } else { menu }
                }
            });
            menu = menu.separator().item(thread_menu::rename(&ws, &f));
            menu = menu.item(item("Regenerate title", |ws, id, cx| ws.regenerate_title(id, true, cx))).item(item("Mark unread", |ws, id, cx| ws.mark_unread(id, cx)));
            if let Some((pid, name)) = project.clone() {
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new(format!("Filter by {name}")).on_click(move |_, _, cx| {
                    let pid = pid.clone();
                    let _ = ws.update(cx, |ws, cx| {
                        ws.project_filter = Some(pid);
                        cx.notify();
                    });
                }));
            }
            menu = menu.submenu("Auto-settle behavior", window, cx, {
                let (ws, tid) = (ws.clone(), tid.clone());
                move |menu, _, _| {
                    let set = |label: &'static str, never: bool| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        PopupMenuItem::new(label).checked(never_settle == never).on_click(move |_, _, cx| {
                            let _ = ws.update(cx, |ws, cx| ws.set_never_settle(&tid, never, cx));
                        })
                    };
                    menu.item(set("Follow Trek's setting", false)).item(set("Never settle this thread", true))
                }
            });
            menu = menu.separator();
            menu = thread_menu::copy_submenu(menu, &ws, &f, window, cx);
            if let Some(dir) = f.cwd.clone() {
                menu = menu.item(PopupMenuItem::new(crate::words::words().show_folder_in_file_manager).on_click(move |_, _, cx| cx.reveal_path(&dir)));
            }
            if let Some((pid, _)) = project.clone() {
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new("Project settings").on_click(move |_, _, cx| {
                    let pid = pid.clone();
                    let _ = ws.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
                }));
            }
            menu = menu.separator().item(thread_menu::archive(&ws, &f));
            // A thread in a worktree asks whether its worktree goes too.
            if f.in_worktree {
                let (ws, tid) = (ws.clone(), tid.clone());
                return menu.item(PopupMenuItem::new("Delete…").icon(crate::assets::Lucide::Trash).on_click(move |_, window, cx| {
                    if let Some(ws) = ws.upgrade() {
                        crate::worktree_ui::confirm_leave(ws, tid.clone(), Leave::Delete, window, cx);
                    }
                }));
            }
            // A thread with sub-agents says they go with it.
            let delete_note = ws.upgrade().and_then(|ws| ws.read(cx).sub_agents_note(&tid, "deleted"));
            menu.item(PopupMenuItem::new("Delete…").icon(crate::assets::Lucide::Trash).on_click({
                let (ws, tid, title) = (ws.clone(), tid.clone(), title.clone());
                move |_, window, cx| {
                    let (ws, tid, title, delete_note) = (ws.clone(), tid.clone(), title.clone(), delete_note.clone());
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        let base = if imported {
                            "It leaves Trek for good. The original stays in the agent's own history."
                        } else {
                            "The thread and its transcript are deleted from Trek. This can't be undone."
                        };
                        alert
                            .title(format!("Delete “{title}”?"))
                            .description(match &delete_note {
                                Some(note) => format!("{base} {note}"),
                                None => base.to_string(),
                            })
                            .confirm()
                            .ok_text("Delete")
                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                            .on_ok(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.delete_thread(&tid, cx));
                                true
                            })
                    });
                }
            }))
        })
    }

    /// Right-click menu for a project (settled-history headers and the project filter).
    fn with_project_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(
        &self,
        row: E,
        id: String,
        name: String,
        path: std::path::PathBuf,
    ) -> impl IntoElement {
        let ws = self.workspace.downgrade();
        row.context_menu(move |menu, window, _| {
            let _ = &window;
            let act = |label: &'static str, f: fn(&mut Workspace, &str, &std::path::Path, &mut Context<Workspace>)| {
                let (ws, id, path) = (ws.clone(), id.clone(), path.clone());
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &id, &path, cx));
                })
            };
            let (p1, p2) = (path.clone(), path.clone());
            let (ws2, id2, name2) = (ws.clone(), id.clone(), name.clone());
            menu.min_w(px(200.))
                .item(act("New thread here", |ws, _, path, cx| ws.navigate(Route::Draft { project: Some(path.to_path_buf()) }, cx)))
                .item(act("Show only this project", |ws, id, _, cx| {
                    ws.project_filter = Some(id.to_string());
                    cx.notify();
                }))
                .separator()
                .item(PopupMenuItem::new(crate::words::words().show_in_file_manager).on_click(move |_, _, cx| cx.reveal_path(&p1)))
                .item(PopupMenuItem::new("Copy path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(p2.display().to_string()))))
                .item(act("Project settings", |ws, id, _, cx| ws.open_project_settings(Some(id.to_string()), cx)))
                .separator()
                .item(PopupMenuItem::new("Remove project…").icon(crate::assets::Lucide::Trash).on_click(move |_, window, cx| {
                    let (ws, id, name) = (ws2.clone(), id2.clone(), name2.clone());
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let (ws, id) = (ws.clone(), id.clone());
                        alert
                            .title(format!("Remove “{name}” from Trek?"))
                            .description("Its threads are archived and it leaves the sidebar. Files on disk and your agents' own history are not touched.")
                            .confirm()
                            .ok_text("Remove project")
                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                            .on_ok(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.remove_project(&id, cx));
                                true
                            })
                    });
                }))
        })
    }

    /// "Show 2 more" / "Show less" under a group, on the titles' line (in the Inbox, its rows').
    fn show_more(
        &self,
        id: String,
        label: String,
        in_box: bool,
        cx: &App,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id(SharedString::from(id))
            .when(!in_box, |el| el.mx_2().pl(px(TEXT_INSET)))
            .when(in_box, |el| el.pl(px(TEXT_INSET - look::INBOX_BORDER)))
            .h(px(24.))
            .flex()
            .items_center()
            .text_size(px(look::META_SIZE))
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.foreground))
            .child(label)
            .on_click(on_click)
            .test_support()
    }

    /// The Settled row at the end: the archive glyph, "Settled" with its chevron (right while
    /// folded), and how many threads it holds in the right-hand column.
    fn settled_row(&self, count: usize, open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("settled-toggle")
            .test_support()
            .mx_2()
            .px(px(ROW_INSET))
            .h(px(look::LINE_H))
            .gap(px(look::MARK_GAP))
            .rounded(px(fluent::control(look::ROW_RADIUS)))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(
                h_flex()
                    .flex_none()
                    .w(px(look::MARK_W))
                    .h(px(look::MARK_H))
                    .justify_center()
                    .child(
                        Icon::new(crate::assets::Lucide::Archive)
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(look::TITLE_SIZE))
                            .text_color(theme.muted_foreground)
                            .child("Settled"),
                    )
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                    ),
            )
            .child(look::trailing(
                Some("settled-count".into()),
                count.to_string(),
                theme.muted_foreground,
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    ws.settled_open = !ws.settled_open;
                    cx.notify();
                })
            }))
    }

    fn footer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let in_settings = matches!(ws.route, Route::Settings(_));
        let update = ws.updater.status.clone();
        let theme = cx.theme().clone();
        let importing = ws.importing;
        let (usage_open, updater_open) = (self.usage_open, self.updater_open);
        let this = cx.entity();
        let usage = Popover::new("usage-popover")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(usage_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.usage_open = *open;
                this.usage_picking = false;
                this.usage_refused = false;
                if *open {
                    this.workspace.update(cx, |ws, cx| {
                        ws.refresh_usage(cx);
                        // Devin's plan takes its terminal UI a few seconds: read only when it's shown.
                        if ws.usage_shown().contains(&crate::workspace::devin_agent()) {
                            ws.refresh_devin_usage(cx);
                        }
                        ws.refresh_usage_today(cx);
                    });
                }
                cx.notify();
            }))
            .trigger({
                // The provider in use (the open thread's, or what the composer starts with): its
                // logo, its nearest limit as a ring (how much is used) and what's left, as the card
                // says it; the tooltip names the limit. A plain icon before any is known.
                let focus = self.workspace.read(cx).usage_focus();
                let peak = focus.as_ref().map(|(_, l)| l.percent);
                let tip = focus.as_ref().map_or_else(
                    || "Usage".to_string(),
                    |(agent, l)| {
                        format!(
                            "Usage · {}: {} {:.0}% left",
                            agent.display_name(),
                            l.label.to_lowercase(),
                            (100. - l.percent).clamp(0., 100.)
                        )
                    },
                );
                let tint = match peak {
                    Some(p) if p >= 95. => palette::red(cx),
                    Some(p) if p >= 80. => palette::amber(cx),
                    _ => theme.foreground.opacity(0.75),
                };
                ui::Pill::new("usage")
                    .ghost(true)
                    .small(true)
                    .selected(usage_open)
                    .tooltip(tip)
                    .map(|p| match peak {
                        Some(peak) => p
                            .children(focus.as_ref().map(|(agent, _)| {
                                div().id("usage-agent").test_support().child(ui::agent_logo(
                                    agent,
                                    px(12.),
                                    cx,
                                ))
                            }))
                            .child(usage_ring(peak / 100., tint, cx))
                            .child(
                                div()
                                    .id("usage-left")
                                    .test_support()
                                    .text_size(to_rems(text::META))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{:.0}% left", (100. - peak).clamp(0., 100.))),
                            ),
                        None => p.child(
                            Icon::new(crate::assets::Lucide::ChartNoAxesColumn)
                                .small()
                                .text_color(theme.muted_foreground),
                        ),
                    })
            })
            .content({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |this, cx| this.usage_card(cx))
            });
        let busy = matches!(
            update,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );
        let ready = matches!(
            update,
            UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. }
        );
        // Found but not downloaded (automatic downloads off): say so, in neutral; ember is for
        // the one-click "restart into it".
        let available = matches!(update, UpdateStatus::Available { .. });
        // Just updated: the pill offers what the update brought, until it's been opened.
        let whats_new =
            !ready && !available && !busy && self.workspace.read(cx).whats_new().is_some();
        // An update to install (or what the last one brought): a notice above the footer, only
        // then; the card it opens has the rest. Checking quietly needs no mark at all.
        let notice: Option<(String, &'static str)> = match &update {
            // Waiting for what it was told to: it says for what, and there's nothing more to press.
            UpdateStatus::RestartPending { version, .. } => {
                Some(match self.workspace.read(cx).update_view(cx).choices {
                    Some(c) => (c.wait, ""),
                    None => (format!("Trek {version} is ready"), "Restart"),
                })
            }
            // Work running: the card has the choices.
            UpdateStatus::Ready { version, .. } => Some((
                format!("Trek {version} is ready"),
                if self.workspace.read(cx).update_view(cx).choices.is_some() {
                    "Options"
                } else {
                    "Restart"
                },
            )),
            UpdateStatus::Available { version } => {
                Some((format!("Trek {version} is out"), "Get it"))
            }
            UpdateStatus::Downloading { version, .. } => {
                Some((format!("Getting Trek {version}…"), ""))
            }
            _ if whats_new => Some((format!("What's new in Trek {}", trek_core::VERSION), "Read")),
            _ => None,
        };
        // Later: it waits for the quit, and the pill stops asking for attention.
        let put_off = match &update {
            UpdateStatus::Ready { version, .. } => {
                self.workspace.read(cx).updater.later.as_ref() == Some(version)
            }
            _ => false,
        };
        let waiting_on_work = matches!(update, UpdateStatus::RestartPending { .. });
        let updater = notice.map(|(text, action)| {
            let accent = if ready && !put_off {
                palette::ember(cx)
            } else {
                theme.foreground
            };
            Popover::new("updater-popover")
                .anchor(Anchor::BottomLeft)
                .appearance(false)
                .open(updater_open)
                .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                    this.updater_open = *open;
                    // Seen once the card closes: marking it on open would empty the card being read.
                    if !*open {
                        this.workspace.update(cx, |ws, cx| {
                            if ws.whats_new().is_some() {
                                ws.mark_whats_new_seen(cx);
                            }
                        });
                    }
                    cx.notify();
                }))
                .trigger(
                    ui::Pill::new("updater")
                        .flexible()
                        .selected(updater_open)
                        .child(if busy {
                            Spinner::new()
                                .xsmall()
                                .color(theme.muted_foreground)
                                .into_any_element()
                        } else if whats_new && !ready && !available {
                            Icon::new(crate::assets::Lucide::Sparkles)
                                .small()
                                .text_color(palette::ember(cx))
                                .into_any_element()
                        } else if waiting_on_work {
                            Icon::new(crate::assets::Lucide::Clock)
                                .small()
                                .text_color(accent)
                                .into_any_element()
                        } else {
                            Icon::new(crate::assets::Lucide::CircleArrowUp)
                                .small()
                                .text_color(accent)
                                .into_any_element()
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(to_rems(text::META))
                                .child(text),
                        )
                        .when(!action.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(to_rems(text::META))
                                    .font_medium()
                                    .text_color(accent)
                                    .child(action),
                            )
                        }),
                )
                .content(move |_, _, cx| this.update(cx, |this, cx| this.updater_card(cx)))
        });
        // New agent CLI versions: a quiet pill with how many, only while there's one to install.
        // Kept while its card is open, so the last update finishing doesn't take the card away.
        let agent_updates = {
            let ws = self.workspace.read(cx);
            crate::agent_updates::badge(&ws.agent_updates, ws.settings.updates.check_agents)
        }
        .or(self.agent_updates_open.then_some((0, false)))
        .map(|(count, running)| {
            let ws = self.workspace.clone();
            Popover::new("agent-updates-popover")
                .anchor(Anchor::BottomRight)
                .appearance(false)
                .open(self.agent_updates_open)
                .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                    this.agent_updates_open = *open;
                    cx.notify();
                }))
                .trigger(
                    ui::Pill::new("agent-updates")
                        .selected(self.agent_updates_open)
                        .tooltip(match count {
                            0 => "Agent updates".to_string(),
                            1 => "1 agent update".to_string(),
                            n => format!("{n} agent updates"),
                        })
                        .child(if running {
                            Spinner::new()
                                .xsmall()
                                .color(theme.muted_foreground)
                                .into_any_element()
                        } else {
                            Icon::new(crate::assets::Lucide::CircleArrowUp)
                                .small()
                                .text_color(theme.muted_foreground)
                                .into_any_element()
                        })
                        .when(count > 0, |el| {
                            el.child(
                                div()
                                    .text_size(to_rems(text::META))
                                    .child(count.to_string()),
                            )
                        }),
                )
                .content(move |_, _, cx| crate::agent_updates::card(&ws, cx))
        });
        let interrupted = self.interrupted_notice(cx);
        v_flex()
            .px_3()
            .pb_2()
            .gap(px(6.))
            .children(interrupted)
            .children(updater.map(|u| div().pt_1().child(u)))
            .child(
                h_flex()
                    .pt(px(6.))
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.border.opacity(0.6))
                    .child(
                        ui::Pill::new("open-settings")
                            .ghost(!in_settings)
                            .small(true)
                            .selected(in_settings)
                            .tooltip(crate::keys::shared("Settings (⌘,)"))
                            .child(
                                Icon::new(IconName::Settings)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(div().text_size(to_rems(text::META)).child("Settings"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.navigate(Route::Settings(SettingsPage::General), cx)
                                })
                            })),
                    )
                    .child(div().flex_1())
                    .when(importing, |el| {
                        el.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .children(agent_updates)
                    .child(usage),
            )
    }

    /// Plan usage per agent: 5-hour, weekly and per-model windows with reset times, for the
    /// providers picked (up to three; the first with usage to show until the user picks). The
    /// "…" button lists every provider to pick from.
    fn usage_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let picking = self.usage_picking;
        let picker = picking.then(|| self.usage_picker(cx));
        let ws = self.workspace.read(cx);
        let loading = ws.usage_loading || ws.devin_loading;
        let now = ws.now();
        // An agent's own row, then a row for each of its accounts.
        let rows: Vec<_> = ws
            .usage_rows()
            .into_iter()
            .flat_map(|row| {
                let accounts = ws.account_rows(&row.agent);
                std::iter::once(row).chain(accounts)
            })
            .collect();
        let labels: Vec<Option<String>> = rows.iter().map(|r| ws.usage_row_label(r)).collect();
        // The card keeps to the window, with every account's windows to show.
        let many = rows.iter().any(|r| r.account.is_some());
        let bar = |pct: f32, pace: Option<f32>, cx: &App| {
            let color = if pct >= 90. {
                palette::red(cx)
            } else if pct >= 70. {
                palette::amber(cx)
            } else {
                cx.theme().foreground.opacity(0.85)
            };
            div()
                .h(px(5.))
                .w_full()
                .rounded_full()
                .bg(cx.theme().foreground.opacity(0.08))
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(color)
                        .w(relative((pct / 100.).clamp(0.0, 1.0))),
                )
                // Where an even pace would have it by now: a bar past this is running ahead.
                .when_some(pace, |el, pace| {
                    el.relative().child(
                        div()
                            .absolute()
                            .top(px(-2.))
                            .bottom(px(-2.))
                            .left(relative(pace))
                            .w(px(1.5))
                            .bg(cx.theme().foreground.opacity(0.55)),
                    )
                })
        };
        let muted = |text: String| {
            div()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(text)
        };
        ui::menu_surface(cx)
            .id("usage-card")
            .test_support()
            .w(px(320.))
            .p(px(14.))
            .gap(px(14.))
            .when(many, |el| el.max_h(px(560.)).overflow_y_scroll())
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().text_size(to_rems(text::LABEL)).font_semibold().child(if picking { "Show in Usage" } else { "Usage" }))
                    .when(loading && !picking, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground)))
                    .child(
                        ui::icon_button("usage-choose", if picking { IconName::Check } else { IconName::Ellipsis }, if picking { "Done" } else { "Choose providers" })
                            .selected(picking)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.usage_picking = !this.usage_picking;
                                this.usage_refused = false;
                                cx.notify();
                            })),
                    ),
            )
            .children(picker)
            .when(!picking && rows.is_empty() && !loading, |el| {
                el.child(div().text_size(to_rems(text::LABEL)).text_color(theme.muted_foreground).child(if ws.usage_picked() { "No providers picked. Choose some with …" } else { "No plan usage reported by your agents." }))
            })
            .when(!picking, |el| el.children(rows.into_iter().zip(labels).map(|(u, label)| {
                let agent = u.agent.clone();
                let row_id = match &u.account {
                    Some(account) => format!("usage-row-{}-{account}", agent.key()),
                    None => format!("usage-row-{}", agent.key()),
                };
                v_flex()
                    .id(SharedString::from(row_id))
                    .test_support()
                    .gap(px(if many { 6. } else { 10. }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(ui::agent_logo(&agent, px(16.), cx))
                            .child(div().text_size(to_rems(text::LABEL)).font_medium().child(agent.display_name()))
                            .when_some(label, |el, label| {
                                el.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(to_rems(text::LABEL))
                                        .text_color(theme.muted_foreground)
                                        .child(label),
                                )
                            })
                            .child(div().flex_1())
                            .when_some(u.plan.clone(), |el, p| el.child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child(p))),
                    )
                    .when(u.no_limits, |el| el.child(muted("No usage limits on this plan.".into())))
                    .when_some(u.error.clone(), |el, e| el.child(div().text_size(to_rems(text::META)).text_color(palette::amber(cx)).child(e)))
                    .when_some(u.note.clone(), |el, n| el.child(muted(n)))
                    .children(u.limits.iter().map(|l| {
                        let pace = trek_agents::limits::pace(l, now);
                        let left = (100. - l.percent).clamp(0., 100.);
                        // With every account's windows to show, one line each.
                        if many {
                            let reset = l
                                .resets_at
                                .filter(|at| *at > now)
                                .map(|at| format!("{left:.0}% left · {}", time::countdown(at, now)))
                                .unwrap_or_else(|| format!("{left:.0}% left"));
                            return h_flex()
                                .gap(px(8.))
                                .text_size(to_rems(text::META))
                                .child(div().flex_none().w(px(78.)).truncate().child(l.label.clone()))
                                .child(div().flex_1().min_w_0().child(bar(l.percent, pace, cx)))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(100.))
                                        .text_right()
                                        .text_color(theme.muted_foreground)
                                        .child(reset),
                                )
                                .into_any_element();
                        }
                        let resets = l.resets_at.map(time::until).unwrap_or_default();
                        v_flex()
                            .gap(px(5.))
                            .child(
                                h_flex()
                                    .text_size(to_rems(text::META))
                                    .child(div().flex_1().child(l.label.clone()))
                                    .child(div().text_color(theme.muted_foreground).child(format!("{:.0}% left", (100. - l.percent).clamp(0., 100.)))),
                            )
                            .child(bar(l.percent, pace, cx))
                            .when(!resets.is_empty(), |el| el.child(muted(format!("Resets {resets}"))))
                            .into_any_element()
                    }))
                    // No plan to show: what Trek recorded it using today. Nothing to say, no line:
                    // an account not signed in ends at its own words.
                    .when(u.limits.is_empty(), |el| {
                        let said = match &u.today {
                            Some((n, spend)) if spend.priced() => format!("Today in Trek: {} tokens · ≈ {}", crate::cost::fmt_tokens(*n), crate::cost::usd(spend.usd())),
                            Some((n, _)) => format!("Today in Trek: {} tokens", crate::cost::fmt_tokens(*n)),
                            None if u.plan.is_none() && !u.no_limits && u.error.is_none() => "Nothing used today.".to_string(),
                            None => String::new(),
                        };
                        el.when(!said.is_empty(), |el| el.child(muted(said)))
                    })
                    // Kept from an earlier run: shown until the agent is read again.
                    .when_some(u.as_of, |el, at| {
                        let ago = time::relative(at);
                        el.child(muted(match ago.as_str() {
                            "now" => "As of just now".to_string(),
                            a if a.starts_with(|c: char| c.is_ascii_digit()) => format!("As of {a} ago"),
                            a => format!("As of {a}"),
                        }))
                    })
                    // Granted, unspent rate-limit resets (Codex's "Usage limit resets").
                    .when(!u.resets.is_empty(), |el| {
                        let n = u.resets.len();
                        el.child(
                            v_flex()
                                .id(SharedString::from(format!("resets-{}", agent.key())))
                                .test_support()
                                .gap(px(8.))
                                .p(px(10.))
                                .rounded(px(radius::CONTROL))
                                .when(fluent::WINDOWS, |el| {
                                    el.rounded(px(fluent::SURFACE_RADIUS))
                                })
                                .border_1()
                                .border_color(theme.border)
                                .child(
                                    h_flex()
                                        .child(div().flex_1().text_size(to_rems(text::META)).font_medium().child("Usage limit resets"))
                                        .child(
                                            div()
                                                .text_size(to_rems(text::META))
                                                .font_medium()
                                                .text_color(palette::emerald(cx))
                                                .child(format!("{n} available")),
                                        ),
                                )
                                .children(u.resets.iter().map(|r| {
                                    let expiry = r.expires_at.map(|e| format!("Expires {}", time::until(e))).unwrap_or_default();
                                    let (workspace, rid, title) = (self.workspace.clone(), r.id.clone(), r.title.clone());
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap(px(2.))
                                                .child(div().text_size(to_rems(text::META)).truncate().child(r.title.clone()))
                                                .when(!expiry.is_empty(), |e| {
                                                    e.child(div().text_size(to_rems(text::META)).text_color(theme.muted_foreground).child(expiry))
                                                }),
                                        )
                                        .child(
                                            gpui_kit::component::button::Button::new(SharedString::from(format!("use-reset-{}", r.id)))
                                                .outline()
                                                .small()
                                                .disabled(ws.reset_credit_in_flight(&r.id))
                                                .label("Use reset")
                                                .on_click(move |_, window, cx| {
                                                    let (workspace, rid, title) = (workspace.clone(), rid.clone(), title.clone());
                                                    window.open_alert_dialog(cx, move |alert, _, _| {
                                                        let (workspace, rid) = (workspace.clone(), rid.clone());
                                                        alert
                                                            .title(format!("Use “{title}”?"))
                                                            .description("Your rate limits reset right away. The credit is spent and can't be returned.")
                                                            .confirm()
                                                            .ok_text("Use reset")
                                                            .on_ok(move |_, _, cx| {
                                                                let _ = workspace.update(cx, |ws, cx| ws.use_reset_credit(rid.clone(), cx));
                                                                true
                                                            })
                                                    });
                                                }),
                                        )
                                })),
                        )
                    })
            })))
            .into_any_element()
    }

    /// The Usage card's provider list: every provider it can show, checked when it does. Three
    /// at most: a fourth is refused with a word, until one is unchecked.
    fn usage_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let shown = ws.usage_shown();
        let full = shown.len() >= trek_core::settings::USAGE_SHOWN_MAX;
        let picked = ws.usage_picked();
        let rows: Vec<AnyElement> = ws
            .usage_providers()
            .into_iter()
            .map(|agent| {
                let on = shown.contains(&agent);
                let blocked = full && !on;
                let key = agent.key();
                ui::menu_row(SharedString::from(format!("usage-pick-{key}")), false, cx)
                    .test_support()
                    .min_h(px(30.))
                    .px(px(8.))
                    .when(blocked, |el| el.opacity(0.5).cursor_default())
                    .child(ui::agent_logo(&agent, px(14.), cx))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::BODY))
                            .child(agent.display_name()),
                    )
                    .when(on, |el| {
                        el.child(
                            Icon::new(IconName::Check)
                                .xsmall()
                                .text_color(palette::ember(cx)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let shown = this
                            .workspace
                            .update(cx, |ws, cx| ws.toggle_usage_shown(&agent, cx));
                        this.usage_refused = !shown;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let hint = if self.usage_refused {
            Some(("Up to 3 at a time — uncheck one first.", palette::amber(cx)))
        } else if full {
            Some(("Up to 3 at a time.", theme.muted_foreground))
        } else {
            None
        };
        v_flex()
            .id("usage-picker")
            .test_support()
            .gap(px(2.))
            .mx(px(-6.))
            .children(rows)
            .when_some(hint, |el, (text, color)| {
                el.child(
                    div()
                        .id("usage-pick-hint")
                        .test_support()
                        .px(px(8.))
                        .pt(px(6.))
                        .text_size(to_rems(text::META))
                        .text_color(color)
                        .child(text),
                )
            })
            .child(
                h_flex()
                    .px(px(8.))
                    .pt(px(8.))
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(if picked {
                                "Picked by you"
                            } else {
                                "Picked automatically"
                            }),
                    )
                    .when(picked, |el| {
                        el.child(
                            ui::Pill::new("usage-pick-auto")
                                .ghost(true)
                                .small(true)
                                .child(
                                    div()
                                        .text_size(to_rems(text::META))
                                        .child("Pick automatically"),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.usage_refused = false;
                                    this.workspace
                                        .update(cx, |ws, cx| ws.show_usage_automatically(cx));
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn updater_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let view = ws.update_view(cx);
        let pending = ws.pending_changes();
        let whats_new = ws.whats_new().cloned();
        let channel = format!("{:?}", ws.settings.updates.channel);
        // What an update on offer brings, else what the one just installed brought.
        let changes = match (pending, whats_new) {
            (Some(c), _) => {
                let title = match c.releases.as_slice() {
                    [one] => format!("What's new in {}", one.version),
                    many => format!("What's changed · {} releases", many.len()),
                };
                let link = c.compare.map(|url| ("Compare on GitHub", url)).or_else(|| {
                    c.releases
                        .first()
                        .filter(|r| !r.url.is_empty())
                        .map(|r| ("Release on GitHub", r.url.clone()))
                });
                Some((title, c.releases, link))
            }
            (None, Some(r)) => {
                let link = (!r.url.is_empty()).then(|| ("Release on GitHub", r.url.clone()));
                Some(("What's new".to_string(), vec![r], link))
            }
            (None, None) => None,
        };
        ui::menu_surface(cx)
            .w(px(340.))
            .p(px(14.))
            .gap(px(10.))
            .child(
                h_flex()
                    .gap_2()
                    .child(crate::brand::logo_mark(px(16.)))
                    .child(
                        div()
                            .text_size(to_rems(text::LABEL))
                            .font_semibold()
                            .child(format!("Trek {}", trek_core::VERSION)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(channel),
                    ),
            )
            .child(
                div()
                    .text_size(to_rems(text::LABEL))
                    .text_color(theme.muted_foreground)
                    .child(view.line),
            )
            .when_some(view.progress, |el, p| {
                el.child(
                    div()
                        .h(px(5.))
                        .w_full()
                        .rounded_full()
                        .bg(theme.foreground.opacity(0.08))
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(palette::ember(cx))
                                .w(relative(p)),
                        ),
                )
            })
            .when_some(changes, |el, (title, releases, link)| {
                el.child(
                    v_flex()
                        .gap(px(8.))
                        .pt(px(10.))
                        .border_t_1()
                        .border_color(theme.foreground.opacity(0.07))
                        .child(
                            div()
                                .text_size(to_rems(text::META))
                                .font_medium()
                                .child(title),
                        )
                        .child(ui::releases_notes("updater-notes", &releases, px(260.), cx))
                        .when_some(link, |el, (label, url)| {
                            el.child(ui::web_link("updater-link", label, url, cx))
                        }),
                )
            })
            .when_some(view.action, |el, action| {
                let wait = view
                    .choices
                    .as_ref()
                    .filter(|c| !c.waiting)
                    .map(|c| SharedString::from(c.wait.clone()));
                el.child(
                    gpui_kit::component::button::Button::new("updater-action")
                        .small()
                        .w_full()
                        .when(action.is_primary() || wait.is_some(), |b| b.primary())
                        .when(!action.is_primary() && wait.is_none(), |b| b.outline())
                        .loading(view.busy)
                        .label(wait.unwrap_or(action.label().into()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.updater_open = false;
                            this.workspace
                                .update(cx, |ws, cx| ws.run_update_action(action, cx))
                        })),
                )
            })
            .when_some(view.choices.as_ref().and(view.secondary), |el, action| {
                el.child(
                    gpui_kit::component::button::Button::new("updater-secondary")
                        .small()
                        .w_full()
                        .outline()
                        .label(action.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.updater_open = false;
                            this.workspace
                                .update(cx, |ws, cx| ws.run_update_action(action, cx))
                        })),
                )
            })
            .when(view.choices.as_ref().is_some_and(|c| c.later), |el| {
                el.child(
                    gpui_kit::component::button::Button::new("updater-later")
                        .small()
                        .w_full()
                        .ghost()
                        .label(crate::workspace::UpdateAction::Later.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.updater_open = false;
                            this.workspace.update(cx, |ws, cx| {
                                ws.run_update_action(crate::workspace::UpdateAction::Later, cx)
                            })
                        })),
                )
            })
            .when_some(view.choices.and_then(|c| c.now), |el, cost| {
                el.child(
                    div()
                        .id("updater-cost")
                        .test_support()
                        .text_size(to_rems(text::META))
                        .text_color(theme.muted_foreground)
                        .child(cost),
                )
            })
            .into_any_element()
    }

    /// Above the footer after a relaunch: how many chats were cut off, Resume all, and a way to
    /// leave them be. The words open the first of them. Chats waiting on the user are counted, not
    /// resumed: each says so on its own row and composer.
    fn interrupted_notice(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.workspace.read(cx).interrupted_notice();
        let headline = notice.headline()?;
        let waiting_too = notice.waiting_too();
        let theme = cx.theme().clone();
        let tint = if notice.working.is_empty() {
            palette::needs_you(cx)
        } else {
            palette::amber(cx)
        };
        let first = notice.working.first().or(notice.waiting.first()).cloned();
        Some(
            h_flex()
                .id("interrupted-notice")
                .test_support()
                .pl(px(10.))
                .pr(px(4.))
                .py(px(5.))
                .gap(px(6.))
                .rounded(px(fluent::control(radius::BUTTON)))
                .bg(tint.opacity(0.08))
                .text_size(to_rems(text::META))
                .child(
                    Icon::new(crate::assets::Lucide::RotateCcwClock)
                        .small()
                        .text_color(tint),
                )
                .child(
                    v_flex()
                        .id("interrupted-headline")
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .child(div().truncate().child(headline))
                        .when_some(waiting_too, |el, line| {
                            el.child(
                                div()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(line),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = first.clone() {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.navigate(Route::Thread(id), cx);
                                });
                            }
                        })),
                )
                .when(!notice.working.is_empty(), |el| {
                    el.child(
                        div()
                            .id("resume-all")
                            .test_support()
                            .flex_none()
                            .px(px(6.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .rounded(px(fluent::control(radius::CONTROL)))
                            .font_medium()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                            .child("Resume all")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace
                                    .update(cx, |ws, cx| ws.resume_all_interrupted(cx));
                            })),
                    )
                })
                .child(
                    gpui_kit::component::button::Button::new("interrupted-dismiss")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Close).text_color(theme.muted_foreground))
                        .tooltip("Leave them as they are")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace
                                .update(cx, |ws, cx| ws.dismiss_all_interrupted(cx));
                        })),
                )
                .into_any_element(),
        )
    }
}

/// A row's sub-agents by agent, in the order they first appear, with how many of each.
fn kid_groups(kids: &[(trek_core::AgentId, String)]) -> Vec<(trek_core::AgentId, usize)> {
    let mut out: Vec<(trek_core::AgentId, usize)> = vec![];
    for (agent, _) in kids {
        match out.iter_mut().find(|(a, _)| a == agent) {
            Some((_, n)) => *n += 1,
            None => out.push((agent.clone(), 1)),
        }
    }
    out
}

/// A small ring filled to `fraction` (0–1) in `color`, on a faint track: the footer's usage.
fn usage_ring(fraction: f32, color: Hsla, cx: &App) -> AnyElement {
    let track = cx.theme().foreground.opacity(0.15);
    let fraction = fraction.clamp(0., 1.);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let r = 5.;
            let arc = |from: f32, to: f32, color: Hsla, window: &mut Window| {
                let mut path = PathBuilder::stroke(px(1.8));
                let steps = 32;
                for i in 0..=steps {
                    let a = -std::f32::consts::FRAC_PI_2
                        + (from + (to - from) * i as f32 / steps as f32) * std::f32::consts::TAU;
                    let p = point(c.x + px(r) * a.cos(), c.y + px(r) * a.sin());
                    if i == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, color);
                }
            };
            arc(0., 1., track, window);
            if fraction > 0. {
                arc(0., fraction, color, window);
            }
        },
    )
    .size(px(14.))
    .flex_none()
    .into_any_element()
}

/// A row's tooltip while it has work out: its sub-agents, then what runs in the background.
pub fn card_tip(kids: &[(trek_core::AgentId, String)], background: &[String]) -> Option<String> {
    let head = match kids.len() {
        1 => "1 sub-agent at work".to_string(),
        n => format!("{n} sub-agents at work"),
    };
    // Each by the provider it runs on: "Codex · Sol: Review the cache".
    let kids: Vec<String> = kids
        .iter()
        .map(|(agent, name)| format!("{} · {name}", agent.display_name()))
        .collect();
    let parts: Vec<String> = [
        (head.as_str(), &kids[..]),
        ("Running in the background", background),
    ]
    .into_iter()
    .filter(|(_, names)| !names.is_empty())
    .map(|(head, names)| format!("{head}:\n{}", names.join("\n")))
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A project's threads in the order its rows go: pinned first (as pinned), then what's at work,
/// then the rest, unread first and newest first within.
fn row_order(mut threads: Vec<Thread>, at_work: impl Fn(&Thread) -> bool) -> Vec<Thread> {
    threads.sort_by(|a, b| {
        let pin = |t: &Thread| t.pinned_at.unwrap_or(i64::MAX);
        pin(a)
            .cmp(&pin(b))
            .then(at_work(b).cmp(&at_work(a)))
            .then(b.is_unseen().cmp(&a.is_unseen()))
            .then(b.updated_at.cmp(&a.updated_at))
    });
    threads
}

/// The Inbox's rows: everything that calls for the user, then what's only news (a turn cut
/// short, then what finished) up to `NEWS_SHOWN` unless it's open, in `needing_you`'s order. With
/// how many wait behind "Show more".
fn inbox_rows(inbox: &[(Thread, Why)], open: bool) -> (Vec<String>, usize) {
    let news = inbox.iter().filter(|(_, w)| !w.calls_for_you()).count();
    let shown = if open { news } else { news.min(NEWS_SHOWN) };
    let rows = inbox
        .iter()
        .filter(|(_, w)| w.calls_for_you())
        .chain(inbox.iter().filter(|(_, w)| !w.calls_for_you()).take(shown))
        .map(|(t, _)| t.id.clone())
        .collect();
    (rows, news - shown)
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support", feature = "shots"))]
        crate::test_support::rendered("Sidebar");
        let ws = self.workspace.read(cx);
        self.graph = Graph {
            parents: ws
                .threads
                .iter()
                .filter_map(|t| t.parent_id.clone())
                .collect(),
            // A machine's threads, as their rows say.
            needs: ws
                .waiting_on_sub_agents()
                .into_iter()
                .chain(ws.machines.sub_agents_need_you())
                .collect(),
            waiting: ws
                .waiting_threads()
                .into_iter()
                .chain(ws.machines.waiting())
                .collect(),
        };
        let ws = self.workspace.read(cx);
        // A title animating in draws a frame at a time, for the moment it takes.
        if ws.retitled.keys().any(|id| ws.title_reveal(id).is_some()) {
            window.request_animation_frame();
        }
        let selected = match &ws.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        let searching = !ws.search.is_empty();
        // What needs the user and what finished unread, in the order the jump key goes through them.
        let needing: Vec<(Thread, Why)> = ws
            .needing_you()
            .into_iter()
            .map(|(t, why)| (t.clone(), why))
            .collect();
        self.why = needing
            .iter()
            .map(|(t, why)| (t.id.clone(), why.clone()))
            .collect();
        let settled_open = ws.settled_open || searching;
        let importing = ws.importing;
        // Rows slide to new places only when the list changes under the user, not as a search
        // narrows it (or gives it back).
        let now = crate::motion::now(cx);
        let animate = ws.motion(cx) && !searching && !self.was_searching;
        self.was_searching = searching;
        self.flip.borrow_mut().begin(animate);
        if !animate {
            self.ghosts.clear();
        }
        let flip = self.flip.clone();
        // Settled history can be hundreds of threads; while it's folded only the count is needed.
        // Everything else is live: pinned, snoozed, at work, and the rest of the inbox.
        let mut settled_count = 0;
        let mut settled: Vec<Thread> = vec![];
        let mut live: Vec<Thread> = vec![];
        for (section, threads) in ws.sections() {
            if section == Section::Settled {
                settled_count = threads.len();
                if settled_open {
                    settled = threads.into_iter().cloned().collect();
                }
            } else {
                live.extend(threads.into_iter().cloned());
            }
        }
        // The Inbox: what needs the user (whichever project it's in) and what finished unread, out
        // of a search, which is a list of its own. A finished thread in a project the filter
        // leaves out stays out, as it would in its project.
        let inbox: Vec<(Thread, Why)> = if searching {
            vec![]
        } else {
            let in_view: HashSet<&str> = live.iter().map(|t| t.id.as_str()).collect();
            needing
                .into_iter()
                .filter(|(t, why)| !matches!(why, Why::Finished) || in_view.contains(t.id.as_str()))
                .collect()
        };
        // A thread in the Inbox leaves its project's list while it's there.
        let in_inbox: HashSet<String> = inbox.iter().map(|(t, _)| t.id.clone()).collect();
        let finished: Vec<String> = inbox
            .iter()
            .filter(|(_, why)| matches!(why, Why::Finished))
            .map(|(t, _)| t.id.clone())
            .collect();
        let names: HashMap<String, String> = ws
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let paths: HashMap<String, std::path::PathBuf> = ws
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.path.clone()))
            .collect();
        // A machine's thread names its project there and the machine ("trek on T-Book").
        let places: HashMap<String, String> = ws
            .machines
            .list
            .iter()
            .flat_map(|m| m.threads.iter())
            .filter_map(|t| Some((t.id.clone(), ws.machines.place(&t.id)?)))
            .collect();
        let study_dir = self.study_dir.clone();
        let project_of = |t: &Thread| {
            if let Some(place) = places.get(&t.id) {
                return place.clone();
            }
            if look::is_study(t, &study_dir) {
                return "Study".into();
            }
            t.project_id
                .as_ref()
                .and_then(|p| names.get(p).cloned())
                .unwrap_or_else(|| "No project".into())
        };
        let theme = cx.theme().clone();
        let cut_off: HashSet<String> = ws.interrupted.keys().cloned().collect();
        // Something to say: it needs the user, it's at work, a quit cut it off. At work, it stays
        // in its project, on show whatever the project's cap.
        let attention = |t: &Thread| {
            t.needs_you()
                || t.run_state == RunState::Working
                || cut_off.contains(&t.id)
                || self.graph.needs.contains(&t.id)
                || self.graph.waiting.contains(&t.id)
        };

        // Air at the end is the history's (under Settled), so the gap above Settled is a section's.
        let mut list = v_flex().pt_1();
        // Every live thread is in its project (Study's sessions in Study), at a line's height:
        // pinned ones first, what's at work, then the quiet rest capped at three (an unread row
        // and the open one stay out). A project counts the threads it has in the Inbox too.
        let mut by_id: HashMap<String, Thread> = HashMap::new();
        let mut info: HashMap<String, GroupInfo> = HashMap::new();
        let mut fresh: Vec<LiveGroup> = vec![];
        {
            let mut groups: Vec<(String, String, Vec<Thread>)> = Vec::new();
            for t in live {
                let pid = if look::is_study(&t, &study_dir) {
                    STUDY.to_string()
                } else {
                    t.project_id.clone().unwrap_or_default()
                };
                let ix = groups.iter().position(|g| g.0 == pid).unwrap_or_else(|| {
                    groups.push((pid, project_of(&t), vec![]));
                    groups.len() - 1
                });
                groups[ix].2.push(t);
            }
            // Groups with something to say first, then unread, then their newest activity.
            groups.sort_by(|a, b| {
                let busy = |g: &(String, String, Vec<Thread>)| g.2.iter().any(&attention);
                let unseen = |g: &(String, String, Vec<Thread>)| g.2.iter().any(|t| t.is_unseen());
                let newest = |g: &(String, String, Vec<Thread>)| {
                    g.2.iter().map(|t| t.updated_at).max().unwrap_or(0)
                };
                // Study's sessions go after every project: they're in none.
                (a.0 == STUDY)
                    .cmp(&(b.0 == STUDY))
                    .then(busy(b).cmp(&busy(a)))
                    .then(unseen(b).cmp(&unseen(a)))
                    .then(newest(b).cmp(&newest(a)))
                    .then_with(|| a.1.cmp(&b.1))
            });
            for (pid, name, threads) in groups {
                let total = threads.len();
                let rows = row_order(
                    threads
                        .into_iter()
                        .filter(|t| !in_inbox.contains(&t.id))
                        .collect(),
                    &attention,
                );
                // Pinned and at work: always on show, folded or not. The quiet rest fold away.
                let (kept, quiet): (Vec<Thread>, Vec<Thread>) = rows
                    .into_iter()
                    .partition(|t| t.pinned_at.is_some() || attention(t));
                let folded = self.collapsed_live.contains(&pid) && !searching;
                let open = self.open_live_projects.contains(&pid);
                let shown: HashSet<&str> = if open || searching {
                    quiet.iter().map(|t| t.id.as_str()).collect()
                } else {
                    let mut shown: HashSet<&str> = quiet
                        .iter()
                        .filter(|t| t.is_unseen() || selected.as_deref() == Some(t.id.as_str()))
                        .map(|t| t.id.as_str())
                        .collect();
                    for t in &quiet {
                        if shown.len() >= QUIET_SHOWN {
                            break;
                        }
                        shown.insert(t.id.as_str());
                    }
                    shown
                };
                let more = quiet.len() - shown.len();
                let rows: Vec<String> = kept
                    .iter()
                    .map(|t| t.id.clone())
                    .chain(
                        quiet
                            .iter()
                            .filter(|t| !folded && shown.contains(t.id.as_str()))
                            .map(|t| t.id.clone()),
                    )
                    .collect();
                let footer = !folded
                    && !searching
                    && ((!open && more > 0) || (open && quiet.len() > QUIET_SHOWN));
                info.insert(
                    pid.clone(),
                    GroupInfo {
                        name,
                        folded,
                        open,
                        more,
                        total,
                    },
                );
                fresh.push(LiveGroup { pid, rows, footer });
                by_id.extend(kept.into_iter().chain(quiet).map(|t| (t.id.clone(), t)));
            }
            if !inbox.is_empty() {
                let open = self.open_live_projects.contains(INBOX);
                let (rows, more) = inbox_rows(&inbox, open);
                let news = inbox.iter().filter(|(_, w)| !w.calls_for_you()).count();
                info.insert(
                    INBOX.to_string(),
                    GroupInfo {
                        name: "Inbox".into(),
                        open,
                        more,
                        total: inbox.len(),
                        ..GroupInfo::default()
                    },
                );
                fresh.insert(
                    0,
                    LiveGroup {
                        pid: INBOX.to_string(),
                        footer: news > NEWS_SHOWN,
                        rows,
                    },
                );
                by_id.extend(inbox.into_iter().map(|(t, _)| (t.id.clone(), t)));
            }
        }
        // While the pointer is over the sidebar its rows hold their places: a thread answered,
        // stopped or come back with news there mustn't slide another under the next click. What
        // changed shows once the pointer leaves (`hover_changed`); what the user does here (folding,
        // "Show more", "Mark all read", searching) shows at once.
        let before = self.shown.clone();
        let layout: Vec<LiveGroup> = match self.shown.take().filter(|_| self.hovered && !searching)
        {
            Some(held) => {
                self.deferred = held != fresh;
                let ws = self.workspace.read(cx);
                held.into_iter()
                    .map(|mut g| {
                        // A row that left the live list (settled, read, say) keeps its place till
                        // then; one archived or deleted goes.
                        for id in &g.rows {
                            if !by_id.contains_key(id) {
                                if let Some(t) = ws.thread(id).filter(|t| t.archived_at.is_none()) {
                                    by_id.insert(id.clone(), t.clone());
                                }
                            }
                        }
                        g.rows.retain(|id| by_id.contains_key(id));
                        g
                    })
                    .filter(|g| info.contains_key(&g.pid) || !g.rows.is_empty())
                    .collect()
            }
            None => {
                self.deferred = false;
                fresh
            }
        };
        self.shown = Some(layout.clone());
        let live_shown = !layout.is_empty();
        // Live rows whose thread went from every list drawn (archived, deleted; settled with the
        // history folded) fade where they were.
        if animate && let Some(before) = before {
            let drawn: HashSet<&str> = layout
                .iter()
                .flat_map(|g| g.rows.iter().map(String::as_str))
                .collect();
            let in_history: HashSet<&str> = if settled_open {
                settled.iter().map(|t| t.id.as_str()).collect()
            } else {
                HashSet::new()
            };
            for g in &before {
                for (ix, id) in g.rows.iter().enumerate() {
                    if drawn.contains(id.as_str())
                        || in_history.contains(id.as_str())
                        || by_id.contains_key(id)
                        || self.ghosts.iter().any(|x| &x.id == id)
                    {
                        continue;
                    }
                    let (Some((thread, _, card)), Some(height)) =
                        (self.last_rows.get(id), self.flip.borrow().height(id))
                    else {
                        continue;
                    };
                    let after = g.rows[..ix]
                        .iter()
                        .rev()
                        .find(|r| drawn.contains(r.as_str()))
                        .cloned();
                    let mut left = crate::motion::Spring::new(
                        crate::motion::SURFACE,
                        crate::motion::UNIT,
                        1.,
                        now,
                    );
                    left.set(0., true, now);
                    self.ghosts.push(Ghost {
                        id: id.clone(),
                        pid: g.pid.clone(),
                        after,
                        thread: thread.clone(),
                        card: *card,
                        height,
                        left,
                    });
                }
            }
        }
        self.ghosts
            .retain(|g| g.left.moving(now) && layout.iter().any(|l| l.pid == g.pid));
        let mut last_rows: HashMap<String, (Thread, String, bool)> = HashMap::new();
        // Each group's header and rows move as one (`flip_stack`).
        let mut live_groups: Vec<(SharedString, AnyElement)> = vec![];
        let projects_shown = layout
            .iter()
            .filter(|g| ![INBOX, STUDY].contains(&g.pid.as_str()))
            .count();
        let mut first_project = true;
        for g in layout {
            let pid = g.pid;
            let in_box = pid == INBOX;
            let is_project = !in_box && pid != STUDY;
            // Projects are things under one label; the Inbox above them is listed by what it's for.
            if is_project && first_project {
                live_groups.push((
                    PROJECTS_LABEL.into(),
                    look::section_label("Projects", Some(projects_shown), cx).into_any_element(),
                ));
                first_project = false;
            }
            // Each group leaves a little air under it, the same wherever it sorts, so its rows never
            // move against it as groups trade places.
            let mut group = if in_box {
                look::inbox_card(cx)
            } else {
                v_flex().w_full()
            };
            let i = info.remove(&pid).unwrap_or_else(|| GroupInfo {
                name: names
                    .get(&pid)
                    .cloned()
                    .unwrap_or_else(|| "No project".into()),
                ..GroupInfo::default()
            });
            let head = if in_box {
                // "Mark all read" clears what only finished; what needs the user stays.
                let mark_read = (!finished.is_empty()).then(|| {
                    let ids = finished.clone();
                    div()
                        .id("inbox-mark-read")
                        .test_support()
                        .flex_none()
                        .text_size(px(look::SMALL_SIZE))
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child("Mark all read")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| ws.mark_read(&ids, cx));
                            this.thaw(cx);
                        }))
                        .into_any_element()
                });
                div()
                    .id("inbox")
                    .test_support()
                    .child(look::inbox_header(i.total, mark_read, cx))
                    .into_any_element()
            } else if pid == STUDY {
                // Study's sessions are no project's: a section of their own, its cap on each row.
                div()
                    .id("study-sessions")
                    .test_support()
                    .child(look::section_label("Study", Some(i.total), cx))
                    .into_any_element()
            } else {
                self.group_header(
                    "live",
                    &pid,
                    i.name.clone(),
                    Some(i.total),
                    Some(i.folded),
                    &paths,
                    cx,
                )
            };
            group = group.child(crate::motion::flip_row(
                &flip,
                format!("live-h-{pid}"),
                head,
                now,
                window,
            ));
            // The group's rows top to bottom, with what's fading away where it was.
            let mut seq: Vec<Result<&String, usize>> = g.rows.iter().map(Ok).collect();
            for (gx, ghost) in self.ghosts.iter().enumerate().filter(|(_, x)| x.pid == pid) {
                let at = match &ghost.after {
                    None => 0,
                    Some(a) => seq
                        .iter()
                        .position(|r| matches!(r, Ok(id) if *id == a))
                        .map_or(seq.len(), |p| p + 1),
                };
                seq.insert(at, Err(gx));
            }
            for row in seq {
                match row {
                    Ok(id) => {
                        let Some(t) = by_id.get(id) else { continue };
                        let sel = selected.as_deref() == Some(id.as_str());
                        last_rows.insert(id.clone(), (t.clone(), pid.clone(), in_box));
                        let row = if in_box {
                            self.inbox_row(t, &project_of(t), sel, cx)
                        } else {
                            self.live_line(t, sel, cx)
                        };
                        group = group.child(crate::motion::flip_row(
                            &flip,
                            id.clone(),
                            row,
                            now,
                            window,
                        ));
                    }
                    Err(gx) => {
                        let ghost = &self.ghosts[gx];
                        let left = ghost.left.frame(now, window).clamp(0., 1.);
                        let row = if ghost.card {
                            self.inbox_row(&ghost.thread, &project_of(&ghost.thread), false, cx)
                        } else {
                            self.live_line(&ghost.thread, false, cx)
                        };
                        // Out of the layout at once (the rows under it slide up to close the gap),
                        // it fades where it was, under them, inert: a click on it does nothing.
                        let row = div().h(px(0.)).child(crate::motion::inert(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(ghost.height))
                                .opacity(left)
                                .child(row),
                        ));
                        group = group.child(crate::motion::flip_row(
                            &flip,
                            ghost.id.clone(),
                            row,
                            now,
                            window,
                        ));
                    }
                }
            }
            if g.footer {
                let pid2 = pid.clone();
                let (id, label) = match (in_box, i.open) {
                    (true, true) => ("inbox-less".to_string(), "Show less".to_string()),
                    (true, false) => ("inbox-more".to_string(), format!("Show {} more", i.more)),
                    (false, true) => (format!("live-less-{pid}"), "Show less".to_string()),
                    (false, false) if i.more > 0 => {
                        (format!("live-more-{pid}"), format!("Show {} more", i.more))
                    }
                    (false, false) => (format!("live-more-{pid}"), "Show more".to_string()),
                };
                let toggle = cx.listener(move |this, _, _, cx| {
                    if !this.open_live_projects.remove(&pid2) {
                        this.open_live_projects.insert(pid2.clone());
                    }
                    this.thaw(cx);
                });
                group = group.child(crate::motion::flip_row(
                    &flip,
                    format!("live-f-{pid}"),
                    self.show_more(id, label, in_box, cx, toggle),
                    now,
                    window,
                ));
            }
            live_groups.push((
                format!("live-g-{pid}").into(),
                div()
                    .w_full()
                    .pb(px(look::GROUP_GAP))
                    .child(group)
                    .into_any_element(),
            ));
        }
        list = list.child(crate::motion::flip_stack(&flip, live_groups, now));
        self.last_rows = last_rows;
        // Nothing live: the list says so, quietly, on the titles' line (before the devices, whose
        // threads live elsewhere).
        if !live_shown && !searching {
            list = list.child(look::empty_note(
                "inbox-zero",
                if importing {
                    "Finding your threads…"
                } else {
                    "Inbox zero"
                },
                "Running agents and anything that needs you will show up here.",
                cx,
            ));
        }
        // Threads living on other machines, under each machine's name (`machines`), as a project's
        // are under its own: what's at work there with its clock, the quiet rest capped at three,
        // what needs the user in the Inbox. One that can't be reached says since when, and why when
        // Trek can tell. Then the phones paired with this one. Both are Devices: places, not
        // projects, so a device's glyph stands where a project's badge would.
        let machines: Vec<(String, String, bool, Option<String>, Vec<Thread>)> = {
            let ws = self.workspace.read(cx);
            let this = crate::words::words().this_computer;
            ws.machines
                .list
                .iter()
                .map(|m| {
                    (
                        m.id().to_string(),
                        m.entry.name.clone(),
                        m.reachable(),
                        crate::machines::offline_note(m, &ws.machines.here, this),
                        m.listed().into_iter().cloned().collect::<Vec<_>>(),
                    )
                })
                .filter(|(.., threads)| !threads.is_empty())
                .collect()
        };
        // The iPhone app's pairings (a machine paired the other way round is a machine, not a
        // phone): connected now, or when each was last seen. Out of a search, which they have no
        // threads for.
        let phones: Vec<(String, String, bool, String)> = if searching {
            vec![]
        } else {
            self.workspace
                .read(cx)
                .remote
                .as_ref()
                .map(|r| {
                    r.devices
                        .iter()
                        .filter(|d| d.platform.as_deref().is_none_or(|p| p == "ios"))
                        .map(|d| {
                            let on = r.connected.contains(&d.device_id);
                            (
                                d.device_id.clone(),
                                d.name.clone(),
                                on,
                                look::phone_seen(on, d.last_seen_at, d.paired_at),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut devices = v_flex().w_full();
        let mut device_count = 0;
        for (host, name, reachable, offline, threads) in machines {
            let total = threads.len();
            let mut threads: Vec<Thread> = row_order(
                threads
                    .into_iter()
                    .filter(|t| !in_inbox.contains(&t.id))
                    .collect(),
                &attention,
            );
            // Like a project's: what's at work, unread and the open thread, then the newest up to
            // three; the rest behind "Show more".
            let quiet = threads.iter().filter(|t| !attention(t)).count();
            let more_key = format!("\u{0}machine:{host}");
            let open = searching || self.open_live_projects.contains(&more_key);
            if !open {
                let mut kept = 0;
                threads.retain(|t| {
                    if attention(t) {
                        return true;
                    }
                    let keep = t.is_unseen()
                        || selected.as_deref() == Some(t.id.as_str())
                        || kept < MACHINE_SHOWN;
                    kept += keep as usize;
                    keep
                });
            }
            let more = quiet - threads.iter().filter(|t| !attention(t)).count();
            let header = h_flex()
                .id(SharedString::from(format!("machine-head-{host}")))
                .test_support()
                .mx_2()
                .px(px(ROW_INSET))
                .h(px(look::LINE_H))
                .gap(px(look::MARK_GAP))
                .child(look::device_glyph(false, reachable, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(look::TITLE_SIZE))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if reachable {
                            theme.foreground
                        } else {
                            theme.foreground.opacity(0.6)
                        })
                        .child(name),
                )
                .when(total > 0, |el| {
                    el.child(look::trailing(
                        Some(format!("machine-count-{host}")),
                        total.to_string(),
                        theme.muted_foreground,
                    ))
                });
            let mut group = v_flex().w_full().pb(px(look::GROUP_GAP)).child(header);
            device_count += 1;
            if let Some(note) = offline {
                group = group.child(
                    div()
                        .id(SharedString::from(format!("machine-offline-{host}")))
                        .test_support()
                        .mx_2()
                        .pl(px(TEXT_INSET))
                        .pr_3()
                        .pb_1()
                        .text_size(px(look::META_SIZE))
                        .text_color(theme.muted_foreground.opacity(0.85))
                        .child(note),
                );
            }
            for t in &threads {
                let sel = selected.as_deref() == Some(t.id.as_str());
                group = group.child(self.live_line(t, sel, cx));
            }
            if !searching && (more > 0 || open && quiet > MACHINE_SHOWN) {
                let (id, label) = if open {
                    (format!("machine-less-{host}"), "Show less".to_string())
                } else {
                    (format!("machine-more-{host}"), format!("Show {more} more"))
                };
                let toggle = cx.listener(move |this, _, _, cx| {
                    if !this.open_live_projects.remove(&more_key) {
                        this.open_live_projects.insert(more_key.clone());
                    }
                    cx.notify();
                });
                group = group.child(self.show_more(id, label, false, cx, toggle));
            }
            devices = devices.child(group);
        }
        for (id, name, connected, seen) in phones {
            devices = devices.child(
                div().w_full().pb(px(look::GROUP_GAP)).child(
                    h_flex()
                        .id(SharedString::from(format!("phone-{id}")))
                        .test_support()
                        .mx_2()
                        .px(px(ROW_INSET))
                        .h(px(look::LINE_H))
                        .gap(px(look::MARK_GAP))
                        .rounded(px(fluent::control(look::ROW_RADIUS)))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(look::device_glyph(true, connected, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(look::TITLE_SIZE))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if connected {
                                    theme.foreground
                                } else {
                                    theme.foreground.opacity(0.6)
                                })
                                .child(name),
                        )
                        .child(look::trailing(
                            Some(format!("phone-seen-{id}")),
                            seen,
                            theme.muted_foreground,
                        ))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Phone & Machines settings")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.navigate(Route::Settings(SettingsPage::Mobile), cx)
                            })
                        })),
                ),
            );
            device_count += 1;
        }
        let any_device = device_count > 0;
        if any_device {
            list = list
                .child(
                    div()
                        .id("devices")
                        .test_support()
                        .child(look::section_label("Devices", Some(device_count), cx)),
                )
                .child(devices);
        }

        // Settled history, grouped by project (Codex style): a row that folds, at the end, its
        // count in the right-hand column.
        let mut history = v_flex().pb_3();
        if settled_count > 0 {
            let mut groups: Vec<(String, String, Vec<Thread>)> = Vec::new();
            for t in settled {
                let pid = if look::is_study(&t, &study_dir) {
                    STUDY.to_string()
                } else {
                    t.project_id.clone().unwrap_or_default()
                };
                match groups.iter_mut().find(|g| g.0 == pid) {
                    Some(g) => g.2.push(t),
                    None => {
                        let name = project_of(&t);
                        groups.push((pid, name, vec![t]));
                    }
                }
            }
            history = history.child(crate::motion::flip_row(
                &flip,
                "settled-toggle",
                div()
                    .pt(px(look::SECTION_GAP - look::GROUP_GAP))
                    .pb(px(look::GROUP_GAP))
                    .child(self.settled_row(settled_count, settled_open, cx)),
                now,
                window,
            ));
            if settled_open {
                let mut settled_groups: Vec<(SharedString, AnyElement)> = vec![];
                for (pid, name, items) in groups {
                    let open = self.open_projects.contains(&pid);
                    let shown = if open || searching {
                        items.len()
                    } else {
                        items.len().min(5)
                    };
                    let pid2 = pid.clone();
                    let folded = self.collapsed_settled.contains(&pid) && !searching;
                    let mut group =
                        v_flex()
                            .w_full()
                            .pb(px(look::GROUP_GAP))
                            .child(crate::motion::flip_row(
                                &flip,
                                format!("settled-h-{pid}"),
                                self.group_header(
                                    "settled",
                                    &pid,
                                    name.clone(),
                                    Some(items.len()),
                                    Some(folded),
                                    &paths,
                                    cx,
                                ),
                                now,
                                window,
                            ));
                    for t in items.iter().take(if folded { 0 } else { shown }) {
                        group = group.child(crate::motion::flip_row(
                            &flip,
                            t.id.clone(),
                            self.line(t, selected.as_deref() == Some(&t.id), cx),
                            now,
                            window,
                        ));
                    }
                    if items.len() > 5 && !searching && !folded {
                        let toggle = cx.listener(move |this, _, _, cx| {
                            if !this.open_projects.remove(&pid2) {
                                this.open_projects.insert(pid2.clone());
                            }
                            cx.notify();
                        });
                        group = group.child(self.show_more(
                            format!("more-{pid}"),
                            if open { "Show less" } else { "Show more" }.to_string(),
                            false,
                            cx,
                            toggle,
                        ));
                    }
                    settled_groups
                        .push((format!("settled-g-{pid}").into(), group.into_any_element()));
                }
                history = history.child(crate::motion::flip_stack(&flip, settled_groups, now));
            }
        }
        // A search that finds nothing says so, in place of an empty list.
        if searching && !live_shown && settled_count == 0 && !any_device {
            let q = self.workspace.read(cx).search.trim().to_string();
            list = list.child(look::empty_note(
                "search-empty",
                &format!("Nothing matches “{q}”"),
                "Titles and messages are both searched.",
                cx,
            ));
        }

        self.flip.borrow_mut().end();
        v_flex()
            .id("sidebar")
            .test_support()
            .w(px(crate::root::SIDEBAR_WIDTH))
            .h_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.hover_changed(*hovered, cx)))
            .flex_none()
            .when(!self.workspace.read(cx).see_through(), |el| {
                el.bg(theme.sidebar)
            })
            .child(self.top(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(crate::motion::flip_scope(
                        &flip,
                        v_flex().w_full().child(list).child(history),
                    )),
            )
            .child(self.footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{inbox_rows, row_order};
    use crate::workspace::attention::Why;
    use trek_core::store::{Store, Thread};
    use trek_core::{AgentId, Effort, HandHolding, RunState};

    fn thread(s: &Store, title: &str, state: RunState, age: i64) -> Thread {
        let mut t = s
            .create_thread(
                None,
                AgentId::ClaudeCode,
                None,
                Effort::Medium,
                HandHolding::Auto,
            )
            .unwrap();
        t.title = title.into();
        t.run_state = state;
        t.updated_at = 1_000_000 - age;
        t.last_seen_at = t.updated_at;
        t
    }

    fn titles(threads: Vec<Thread>) -> Vec<String> {
        threads.into_iter().map(|t| t.title).collect()
    }

    #[test]
    fn a_project_lists_what_is_pinned_then_what_is_at_work_then_the_rest_newest_first() {
        let s = Store::in_memory().unwrap();
        let mut pinned = thread(&s, "pinned", RunState::Idle, 9_000);
        pinned.pinned_at = Some(5);
        let mut unread = thread(&s, "unread", RunState::Idle, 8_000);
        unread.last_seen_at = 0;
        let threads = vec![
            thread(&s, "older", RunState::Idle, 3_000),
            thread(&s, "running", RunState::Working, 7_000),
            unread,
            thread(&s, "newer", RunState::Idle, 1_000),
            pinned,
        ];
        let order = row_order(threads, |t| t.run_state == RunState::Working);
        assert_eq!(
            titles(order),
            ["pinned", "running", "unread", "newer", "older"]
        );
    }

    #[test]
    fn the_inbox_shows_all_that_calls_for_you_and_caps_the_news() {
        let s = Store::in_memory().unwrap();
        let asks = |n: usize| {
            (
                thread(&s, &format!("asks {n}"), RunState::NeedsYou, 0),
                Why::Approval("run x".into()),
            )
        };
        let done = |n: usize| {
            (
                thread(&s, &format!("done {n}"), RunState::Idle, 0),
                Why::Finished,
            )
        };
        let mut inbox: Vec<(Thread, Why)> = (0..6).map(asks).collect();
        inbox.push((thread(&s, "cut", RunState::Idle, 0), Why::CutShort));
        inbox.extend((0..7).map(done));
        let name = |id: &String| {
            inbox
                .iter()
                .find(|(t, _)| &t.id == id)
                .map(|(t, _)| t.title.clone())
                .unwrap()
        };
        let (rows, more) = inbox_rows(&inbox, false);
        let shown: Vec<String> = rows.iter().map(name).collect();
        assert_eq!(shown.len(), 6 + super::NEWS_SHOWN);
        assert!(shown[..6].iter().all(|t| t.starts_with("asks")));
        assert_eq!(
            shown[6], "cut",
            "a turn cut short comes before what finished"
        );
        assert_eq!(more, 8 - super::NEWS_SHOWN);
        let (rows, more) = inbox_rows(&inbox, true);
        assert_eq!((rows.len(), more), (inbox.len(), 0));
    }
}
