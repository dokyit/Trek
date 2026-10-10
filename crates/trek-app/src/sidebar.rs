//! Inbox sidebar. Live work and anything waiting on you sit on top as quiet cards
//! (project · status, title, agent glyph); settled history folds into a footer grouped by project.

use crate::palette;
use crate::time;
use crate::ui;
use crate::worktree_ui::Leave;
use crate::workspace::{ItemRef, PanelTool, Route, SettingsPage, UpdateAction, UpdateStatus, Workspace, WorkspaceEvent};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_core::store::{SearchHit, Section, Thread};
use trek_core::{RunState, ThreadSource};

pub struct Sidebar {
    workspace: Entity<Workspace>,
    search: Entity<InputState>,
    project_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    rename_input: Entity<InputState>,
    /// The thread being renamed while the dialog is open.
    renaming: Option<String>,
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
    /// Re-renders once a second while a turn runs, so "Working 12s" counts up, and once a minute
    /// otherwise, so "5m" ages (this view is cached; nothing else would redraw it). The flag is
    /// whether it's the fast one.
    _clock: Option<(bool, Task<()>)>,
    /// Which threads have sub-agents, wait on them, or have one waiting on the user: worked out
    /// once as the list is drawn, not for each of its rows (each answer reads every thread).
    graph: Graph,
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
    /// was a card. What a ghost draws.
    last_rows: HashMap<String, (Thread, String, bool)>,
    /// A search was on last frame: the list it leaves (or comes to) takes its places at once.
    was_searching: bool,
    _subscriptions: Vec<Subscription>,
}

/// A live row fading away after its thread left the list (see `Sidebar::ghosts`).
struct Ghost {
    id: String,
    pid: String,
    /// The row it was under in its group, if any: it's drawn after that one.
    after: Option<String>,
    thread: Thread,
    card: bool,
    height: f32,
    /// 1 drawn in full, 0 gone.
    left: crate::motion::Spring,
}

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
    unread: usize,
    needs: usize,
    working: usize,
    total: usize,
    folded: bool,
    open: bool,
    /// Quiet rows behind "Show more".
    more: usize,
}

/// The threads' standing among their sub-agents, as of the last render (`Sidebar::graph`).
#[derive(Default)]
struct Graph {
    /// Threads with sub-agents in Trek's lists.
    parents: HashSet<String>,
    /// `Workspace::waiting_on_sub_agents`.
    needs: HashSet<String>,
    /// `Workspace::waiting_threads`.
    waiting: HashSet<String>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let project_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects…"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Thread title"));
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
        subscriptions.push(cx.subscribe_in(&rename_input, window, |this: &mut Self, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. }) {
                return;
            }
            if let Some(id) = this.renaming.take() {
                let title = input.read(cx).value().to_string();
                this.workspace.update(cx, |ws, cx| ws.rename(&id, title, cx));
                window.close_dialog(cx);
            }
        }));
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
            rename_input,
            renaming: None,
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
            hovered: false,
            shown: None,
            deferred: false,
            flip: Default::default(),
            ghosts: vec![],
            last_rows: HashMap::new(),
            was_searching: false,
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
    #[cfg(test)]
    pub fn row_offset(&self, key: &str, cx: &App) -> Option<f32> {
        self.flip.borrow().offset(key, crate::motion::now(cx))
    }

    /// The threads whose rows are fading away.
    #[cfg(test)]
    pub fn ghosts(&self) -> Vec<String> {
        self.ghosts.iter().map(|g| g.id.clone()).collect()
    }

    /// Let go of the held rows: what the user just did here shows at once.
    fn thaw(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
        cx.notify();
    }

    fn sync_clock(&mut self, cx: &mut Context<Self>) {
        let running = self.workspace.read(cx).any_turn_running();
        if self._clock.as_ref().is_some_and(|(fast, _)| *fast == running) {
            return;
        }
        let every = std::time::Duration::from_secs(if running { 1 } else { 60 });
        self._clock = Some((
            running,
            cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(every).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
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
            .trigger(ui::icon_button("filter-projects", IconName::Folder, "Filter threads by project").selected(filtering))
            .content(move |_, _, cx| this.update(cx, |this, cx| this.project_menu(cx)));
        let add = ui::icon_button("add-project", crate::assets::Lucide::FolderPlus, "Add project").dropdown_menu_with_anchor(Anchor::TopLeft, {
            let ws = self.workspace.clone();
            let sidebar = cx.entity();
            move |menu, _, _| {
                let ws2 = ws.clone();
                let ws = ws.clone();
                let sb = sidebar.clone();
                menu.min_w(px(200.))
                    .item(PopupMenuItem::new("Open folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))))
                    .item(PopupMenuItem::new("Clone from GitHub…").icon(IconName::Github).on_click(move |_, window, cx| sb.update(cx, |s, cx| s.open_clone_dialog(window, cx))))
                    .separator()
                    .item(PopupMenuItem::new("New thread without a project").icon(crate::assets::Lucide::MessageSquarePlus).on_click({
                        let ws = ws2.clone();
                        move |_, _, cx| ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx))
                    }))
            }
        });
        let ws = self.workspace.read(cx);
        let at_basecamp = ws.route == Route::Basecamp;
        let waiting = ws.ready_for_review().len();
        let basecamp = ui::nav_row("open-basecamp", Icon::new(crate::assets::Lucide::Tent), "Basecamp", None, at_basecamp, cx)
            .test_support()
            .when(waiting > 0, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(waiting.to_string())))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared("Basecamp (⌘⇧H)")).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx))));
        let at_notes = self.workspace.read(cx).route == Route::Notes;
        let notes = ui::nav_row("open-notes", Icon::new(crate::assets::Lucide::NotebookPen), "Notes", None, at_notes, cx)
            .test_support()
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared("Things to jot down (⌘⇧J)")).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Notes, cx))));
        v_flex()
            .child(
                h_flex()
                    .px_3()
                    .pt_1()
                    .pb_2()
                    .gap(px(2.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                        ),
                    )
                    .child(filter)
                    .child(add)
                    .child(ui::icon_button("new-thread", crate::assets::Lucide::SquarePen, "New thread (⌘N)").on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| ws.new_thread(cx))
                    }))),
            )
            .child(div().px_2().pb_1().child(basecamp).child(notes))
    }

    fn project_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let q = self.project_search.read(cx).value().to_lowercase();
        let current = ws.project_filter.clone();
        let projects: Vec<(String, String, Option<String>, std::path::PathBuf)> = ws
            .workspace_projects()
            .into_iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.remote.as_deref().is_some_and(|r| r.to_lowercase().contains(&q)))
            .map(|p| (p.id.clone(), p.name.clone(), p.remote.clone(), p.path.clone()))
            .collect();
        fn pick(id: Option<String>, cx: &mut Context<Sidebar>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
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
            .child(div().px(px(6.)).pb(px(4.)).child(Input::new(&self.project_search).small().prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground))))
            .child(
                ui::menu_row("pf-all", current.is_none(), cx)
                    .child(Icon::new(IconName::Folder).small().text_color(theme.muted_foreground))
                    .child("All projects")
                    .on_click(pick(None, cx)),
            )
            .child(
                ui::menu_row("pf-none", current.as_deref() == Some(crate::workspace::NO_PROJECT), cx)
                    .child(Icon::new(crate::assets::Lucide::MessageSquare).small().text_color(theme.muted_foreground))
                    .child("No project")
                    .on_click(pick(Some(crate::workspace::NO_PROJECT.to_string()), cx)),
            )
            .child(
                v_flex().id("pf-list").max_h(px(320.)).overflow_y_scroll().children(projects.into_iter().map(|(id, name, remote, path)| {
                    let label = remote.clone().unwrap_or_else(|| name.clone());
                    let ws = self.workspace.clone();
                    let gear_id = id.clone();
                    ui::menu_row(SharedString::from(format!("pf-{id}")), current.as_ref() == Some(&id), cx)
                        .group("pf-row")
                        .child(ui::project_badge(&name, &self.workspace.read(cx).project_look(&path), cx))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            gpui_kit::component::button::Button::new(SharedString::from(format!("pf-gear-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Settings).text_color(theme.muted_foreground))
                                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                    let (p1, p2, ws2, ws3, id3) = (path.clone(), path.clone(), ws.clone(), ws.clone(), gear_id.clone());
                                    menu.item(PopupMenuItem::new("New thread here").on_click(move |_, _, cx| {
                                        let p = p2.clone();
                                        ws2.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(p) }, cx))
                                    }))
                                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&p1)))
                                    .item(PopupMenuItem::new("Project settings").on_click(move |_, _, cx| {
                                        let id = id3.clone();
                                        ws3.update(cx, |ws, cx| ws.open_project_settings(Some(id), cx))
                                    }))
                                }),
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
                .child(v_flex().gap_2().child("Uses your GitHub CLI login. Clones into ~/Developer.").child(Input::new(&input)))
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(gpui_kit::component::button::Button::new("cancel-clone").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("do-clone").primary().label("Clone").on_click(move |_, _, cx| {
                                let spec = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.clone_repo(spec, cx));
                            }),
                        )),
                )
        });
    }

    /// What a live row says at its end: a mark and a few words in its run state's colour (the
    /// same colours as the IDE's, `palette::run_state`), or its age while there's nothing to say.
    fn status(&self, t: &Thread, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let ws = self.workspace.read(cx);
        let mark = |id: String, color: Hsla, icon: Option<Icon>, dot: bool, text: String| {
            h_flex()
                .id(SharedString::from(id))
                .test_support()
                .flex_none()
                .gap(px(5.))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .when_some(icon, |el, icon| el.child(icon.xsmall().text_color(color)))
                .when(dot, |el| el.child(div().flex_none().size(px(6.)).rounded_full().bg(color)))
                .child(text)
                .into_any_element()
        };
        // A sub-agent of its own waits on an approval: sub-agents have no rows, so this one says it.
        if t.run_state != RunState::NeedsYou && self.graph.needs.contains(&t.id) {
            return mark(format!("card-sub-needs-{}", t.id), palette::needs_you(cx), None, true, "Sub-agent needs you".into());
        }
        // Paused at a usage limit: until when (it's no failure, and nothing to do yet).
        if let Some(p) = t.paused.as_ref().filter(|_| t.run_state == RunState::Idle) {
            let text = match p.resets_at {
                Some(at) => format!("Paused until {}", time::reset_clock(at, ws.now())),
                None => "Paused at its limit".to_string(),
            };
            return mark(format!("paused-{}", t.id), palette::amber(cx), None, false, text);
        }
        // Its turn is over but its sub-agents are still out: it's at work, waiting on them.
        if t.run_state == RunState::Idle && self.graph.waiting.contains(&t.id) {
            let longest = ws.waiting_on(&t.id).iter().map(|w| w.elapsed).max();
            let text = match longest {
                Some(d) => format!("Waiting {}", time::elapsed(d)),
                None => "Waiting".to_string(),
            };
            return mark(format!("card-waiting-{}", t.id), palette::working(cx), Some(Icon::new(crate::assets::Lucide::LoaderCircle)), false, text);
        }
        match t.run_state {
            RunState::Working => {
                let elapsed = ws.live.get(&t.id).and_then(|l| l.turn_started).map(|s| time::elapsed(s.elapsed())).unwrap_or_default();
                // The loader holds still and the clock beside it ticks: a spinning one would redraw
                // the whole sidebar every frame.
                let text = if elapsed.is_empty() { "Working".to_string() } else { format!("Working {elapsed}") };
                mark(format!("card-working-{}", t.id), palette::working(cx), Some(Icon::new(crate::assets::Lucide::LoaderCircle)), false, text)
            }
            RunState::NeedsYou => mark(format!("card-needs-{}", t.id), palette::needs_you(cx), None, true, needs_label(ws, &t.id).into()),
            RunState::Failed => mark(format!("card-failed-{}", t.id), palette::failed(cx), None, true, "Failed".into()),
            RunState::Idle if t.is_unseen() => h_flex()
                .flex_none()
                .gap_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().size(px(6.)).rounded_full().bg(palette::emerald(cx)))
                .child(time::relative(t.updated_at))
                .into_any_element(),
            RunState::Idle => div().flex_none().text_xs().text_color(theme.muted_foreground.opacity(0.8)).child(time::relative(t.updated_at)).into_any_element(),
        }
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
        self.workspace.update(cx, |ws, cx| match hit.as_ref().and_then(ItemRef::of_hit) {
            Some(at) => ws.open_thread_at(&id, at, cx),
            None => ws.navigate(Route::Thread(id), cx),
        })
    }

    /// The matching words of a content hit, on one quiet line.
    fn hit_line(hit: &SearchHit, cx: &App) -> Div {
        let (text, ranges) = ui::lead_to_match(&hit.snippet, &hit.ranges, 12);
        div().min_w_0().truncate().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(ui::match_text(&text, &ranges, cx))
    }

    /// What `t` has at work: its sub-agents (Trek's, and its agent's own, in its turn or working
    /// in the background), by logo and name, and what else its agent runs in the background.
    pub(crate) fn at_work(&self, t: &Thread, cx: &App) -> (Vec<(trek_core::AgentId, String)>, Vec<String>) {
        let ws = self.workspace.read(cx);
        let children = if self.graph.parents.contains(&t.id) { ws.running_children(&t.id) } else { vec![] };
        let mut kids: Vec<(trek_core::AgentId, String)> = children.into_iter().map(|c| (c.agent.clone(), format!("{}: {}", ws.model_label(c), c.title))).collect();
        let Some(l) = ws.live.get(&t.id) else { return (kids, vec![]) };
        let out = |id: &str| l.tasks.iter().any(|k| k.id == id && k.done.is_none());
        kids.extend(l.tasks.iter().filter(|k| k.done.is_none()).map(|k| (t.agent.clone(), k.description.clone())));
        kids.extend(l.background_agents().filter(|b| !b.task.call.as_deref().is_some_and(out)).map(|b| (t.agent.clone(), b.task.title.clone())));
        (kids, l.background_work().map(|b| b.task.title.clone()).collect())
    }

    /// A live thread with something to say (it needs you, it failed, it's at work), or a pinned
    /// or snoozed one, at a line's height like the quiet rows: its title, then where it stands
    /// in its run state's colour. Rows under a project's header leave the project out; pinned and
    /// snoozed ones (`badge`) aren't under one, so they carry its badge.
    fn card(&self, t: &Thread, project: &str, badge: bool, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (kids, background) = self.at_work(t, cx);
        let tip = card_tip(&kids, &background).map(SharedString::from);
        let quiet = t.run_state == RunState::Idle && !t.is_unseen() && !selected && kids.is_empty() && !self.graph.waiting.contains(&t.id);
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        let title = h_flex()
            .h(px(30.))
            .gap_2()
            .when(badge, |el| {
                el.child(if t.project_id.is_none() { no_project_badge(cx) } else { ui::project_badge(project, &self.workspace.read(cx).thread_project_look(t), cx) })
            })
            .child(
                div().flex_1().min_w_0().when(t.is_unseen() && !selected, |el| el.font_medium()).child(ui::title_text(
                    SharedString::from(format!("card-title-{}", t.id)),
                    &t.title,
                    self.workspace.read(cx).title_reveal(&t.id),
                    if quiet { theme.foreground.opacity(0.78) } else { theme.foreground },
                    cx,
                )),
            )
            .when_some(t.worktree.as_ref(), |el, wt| el.child(crate::worktree_ui::branch_chip(SharedString::from(format!("card-branch-{}", t.id)), wt, cx)))
            // Its sub-agents at work: one logo per agent with how many, in a pill of their own.
            // (Overlapped logos of the same agent read as a smudge, and a ring the sidebar's colour
            // shows under glass.)
            .when(!kids.is_empty(), |el| {
                el.child(
                    h_flex()
                        .id(SharedString::from(format!("card-kids-{}", t.id)))
                        .test_support()
                        .flex_none()
                        .h(px(18.))
                        .px(px(5.))
                        .gap(px(5.))
                        .rounded_full()
                        .bg(theme.foreground.opacity(0.07))
                        .children(kid_groups(&kids).into_iter().take(3).map(|(agent, n)| {
                            h_flex()
                                .gap(px(2.))
                                .child(ui::agent_logo(&agent, px(12.), cx))
                                .when(n > 1, |el| el.child(div().text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(theme.muted_foreground).child(n.to_string())))
                        })),
                )
            })
            // What its agent runs in the background after answering (a dev server, a watcher): a
            // dot that holds still (one that breathed would redraw the whole cached sidebar); the
            // tooltip names them.
            .when(!background.is_empty(), |el| {
                el.child(div().id(SharedString::from(format!("card-background-{}", t.id))).test_support().flex_none().size(px(6.)).rounded_full().bg(palette::sky(cx)))
            })
            .child(self.status(t, cx));
        let row = v_flex()
            .id(SharedString::from(format!("card-{}", t.id)))
            .test_support()
            .mx_2()
            .pl(px(if badge { 12. } else { 30. }))
            .pr_3()
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(title)
            .when_some(hit.clone(), |el, h| el.child(Self::hit_line(&h, cx).mt(px(-6.)).pb(px(6.))))
            .when_some(tip, |el, tip| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        self.with_menu(row, t, cx).into_any_element()
    }

    /// Codex-style compact row for settled history. One whose sub-agents or background work are
    /// still going (settled, it stays here, as a working thread would) has a quiet dot that names
    /// them on hover, as a card does.
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
        let title = h_flex()
            .h(px(30.))
            .gap_2()
            .child(div().flex_1().min_w_0().child(ui::title_text(
                SharedString::from(format!("line-title-{}", t.id)),
                &t.title,
                self.workspace.read(cx).title_reveal(&t.id),
                if selected { theme.foreground } else { theme.foreground.opacity(0.78) },
                cx,
            )))
            .when(tip.is_some(), |el| {
                el.child(div().id(SharedString::from(format!("line-at-work-{}", t.id))).test_support().flex_none().size(px(6.)).rounded_full().bg(palette::sky(cx)))
            })
            .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.8)).child(time::relative(t.updated_at)));
        let row = v_flex()
            .id(SharedString::from(format!("line-{}", t.id)))
            .test_support()
            .mx_2()
            .pl(px(30.))
            .pr_3()
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(title)
            .when_some(hit.clone(), |el, h| el.child(Self::hit_line(&h, cx).mt(px(-6.)).pb(px(6.))))
            .when_some(tip, |el, tip| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        self.with_menu(row, t, cx).into_any_element()
    }

    /// A live thread that needs no attention, at a line's height: its title (medium and dotted
    /// emerald while unread), its worktree, a sky dot while its own or its sub-agents' work still
    /// runs, and its age — "Paused" instead while it sits at a usage limit.
    fn live_line(&self, t: &Thread, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        let (kids, background) = self.at_work(t, cx);
        let tip = card_tip(&kids, &background).map(SharedString::from);
        let unseen = t.is_unseen();
        let paused = t.paused.is_some() && t.run_state == RunState::Idle;
        let at_work = !kids.is_empty() || !background.is_empty();
        let title = h_flex()
            .h(px(30.))
            .gap_2()
            .child(
                div().flex_1().min_w_0().when(unseen && !selected, |el| el.font_medium()).child(ui::title_text(
                    SharedString::from(format!("live-line-title-{}", t.id)),
                    &t.title,
                    ws.title_reveal(&t.id),
                    if !selected && !unseen { theme.foreground.opacity(0.78) } else { theme.foreground },
                    cx,
                )),
            )
            .when_some(t.worktree.as_ref(), |el, wt| {
                el.child(crate::worktree_ui::branch_chip(SharedString::from(format!("live-line-branch-{}", t.id)), wt, cx))
            })
            .when(unseen, |el| {
                el.child(div().id(SharedString::from(format!("live-line-unseen-{}", t.id))).test_support().flex_none().size(px(6.)).rounded_full().bg(palette::emerald(cx)))
            })
            .when(at_work, |el| {
                let color = if kids.is_empty() { palette::sky(cx) } else { palette::working(cx) };
                el.child(div().id(SharedString::from(format!("live-line-at-work-{}", t.id))).test_support().flex_none().size(px(6.)).rounded_full().bg(color))
            })
            .child(if paused {
                div().id(SharedString::from(format!("live-line-paused-{}", t.id))).test_support().text_xs().text_color(palette::amber(cx)).child("Paused").into_any_element()
            } else {
                div().text_xs().text_color(theme.muted_foreground.opacity(0.8)).child(time::relative(t.updated_at)).into_any_element()
            });
        let row = v_flex()
            .id(SharedString::from(format!("live-line-{}", t.id)))
            .test_support()
            .mx_2()
            .pl(px(30.))
            .pr_3()
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(title)
            .when_some(hit.clone(), |el, h| el.child(Self::hit_line(&h, cx).mt(px(-6.)).pb(px(6.))))
            .when_some(tip, |el, tip| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        self.with_menu(row, t, cx).into_any_element()
    }

    /// A project group's header (live quiet threads and settled history): badge, name, `extra`
    /// (a count), and a "new thread here" button on hover; wrapped in the project menu.
    fn group_header(
        &self,
        prefix: &'static str,
        pid: &str,
        name: String,
        extra: Option<AnyElement>,
        fold: Option<bool>,
        paths: &HashMap<String, std::path::PathBuf>,
        looks: &HashMap<String, crate::ui::ProjectLook>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        // A new thread in this project (or in none, for "No project"), from its header.
        let here = if pid.is_empty() { Some(None) } else { paths.get(pid).cloned().map(Some) };
        let header = h_flex()
            .id(SharedString::from(format!("{prefix}-proj-head-{pid}")))
            .test_support()
            .group("proj-head")
            .mx_2()
            .pl_3()
            .pr_1()
            .h(px(30.))
            .mt_1()
            .gap_2()
            .rounded(px(8.))
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(
                h_flex()
                    .id(SharedString::from(format!("{prefix}-fold-{pid}")))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .cursor_pointer()
                    .map(|el| {
                        if pid.is_empty() {
                            el.child(no_project_badge(cx))
                        } else {
                            el.child(ui::project_badge(&name, &looks.get(pid).cloned().unwrap_or_default(), cx))
                        }
                    })
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .when_some(extra, |el, extra| el.child(extra))
                    .when_some(fold, |el, folded| {
                        el.child(
                            Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown })
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                    })
                    .when(fold.is_some(), |el| {
                        let pid = pid.to_string();
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            let set = if prefix == "live" { &mut this.collapsed_live } else { &mut this.collapsed_settled };
                            if !set.remove(&pid) {
                                set.insert(pid.clone());
                            }
                            this.thaw(cx);
                        }))
                    }),
            )
            .when_some(here, |el, project| {
                let tip = if project.is_none() { "New thread without a project" } else { "New thread here" };
                el.child(
                    div().invisible().group_hover("proj-head", |s| s.visible()).child(
                        ui::icon_button(SharedString::from(format!("{prefix}-proj-new-{pid}")), crate::assets::Lucide::SquarePen, tip).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let project = project.clone();
                                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Draft { project }, cx))
                            },
                        )),
                    ),
                )
            });
        match paths.get(pid) {
            Some(path) => self.with_project_menu(header, pid.to_string(), name, path.clone()).into_any_element(),
            None => header.into_any_element(),
        }
    }

    /// Right-click menu for a thread (T3's set): pin, settle, snooze, rename, copy, project, archive, delete.
    fn with_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(&self, row: E, t: &Thread, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.downgrade();
        let sidebar = cx.entity().downgrade();
        let tid = t.id.clone();
        let title = t.title.clone();
        let pinned = t.pinned_at.is_some();
        let settled = t.settled_at.is_some();
        let snoozed = t.snoozed_until.is_some_and(|u| u > trek_core::store::now_ms());
        let never_settle = t.never_settle;
        let imported = t.source != ThreadSource::Trek;
        let resume_cmd = match (&t.agent, &t.native_id) {
            (trek_core::AgentId::ClaudeCode, Some(n)) => Some(format!("claude --resume {n}")),
            (trek_core::AgentId::Codex, Some(n)) => Some(format!("codex resume {n}")),
            (trek_core::AgentId::OpenCode, Some(n)) => Some(format!("opencode -s {n}")),
            _ => None,
        };
        let cwd = t.cwd.clone();
        let in_worktree = t.worktree.is_some();
        let (archive_note, delete_note) = match self.graph.parents.contains(&t.id) {
            true => {
                let ws = self.workspace.read(cx);
                (ws.sub_agents_note(&t.id, "archived"), ws.sub_agents_note(&t.id, "deleted"))
            }
            false => (None, None),
        };
        let project = t.project_id.clone().and_then(|pid| self.workspace.read(cx).project(&pid).map(|p| (p.id.clone(), p.name.clone())));
        row.context_menu(move |menu, window, cx| {
            let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                let ws = ws.clone();
                let tid = tid.clone();
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                })
            };
            let copy = |label: &'static str, text: String| PopupMenuItem::new(label).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
            let mut menu = menu
                .min_w(px(210.))
                .item(PopupMenuItem::new("Open in new window").icon(crate::assets::Lucide::SquareArrowOutUpRight).on_click({
                    let (ws, tid) = (ws.clone(), tid.clone());
                    move |_, _, cx| {
                        if let Some(ws) = ws.upgrade() {
                            crate::thread_window::open(ws, &tid, cx);
                        }
                    }
                }))
                .item(PopupMenuItem::new("Fork thread").icon(crate::assets::Lucide::GitFork).on_click({
                    let (ws, tid) = (ws.clone(), tid.clone());
                    move |_, _, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.fork_thread(&tid, crate::workspace::ForkAt::End, &crate::workspace::Scope::Main, cx));
                    }
                }))
                .separator()
                .item(item(if pinned { "Unpin thread" } else { "Pin thread" }, |ws, id, cx| ws.toggle_pin(id, cx)))
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
            menu = menu.separator();
            menu = menu.item(PopupMenuItem::new("Rename thread").on_click({
                let (sidebar, tid, title) = (sidebar.clone(), tid.clone(), title.clone());
                move |_, window, cx| {
                    let _ = sidebar.update(cx, |s, cx| s.open_rename_dialog(tid.clone(), title.clone(), window, cx));
                }
            }));
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
            menu = menu.submenu("Copy", window, cx, {
                let (ws, tid, title, resume_cmd, cwd) = (ws.clone(), tid.clone(), title.clone(), resume_cmd.clone(), cwd.clone());
                move |menu, _, _| {
                    let mut menu = menu.item(copy("Title", title.clone()));
                    menu = menu.item(PopupMenuItem::new("Conversation as Markdown").on_click({
                        let (ws, tid) = (ws.clone(), tid.clone());
                        move |_, _, cx| {
                            if let Ok(text) = ws.update(cx, |ws, _| ws.transcript_markdown(&tid)) {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                        }
                    }));
                    if let Some(cmd) = resume_cmd.clone() {
                        menu = menu.item(copy("Resume command", cmd));
                    }
                    if let Some(dir) = cwd.clone() {
                        menu = menu.item(copy("Folder path", dir.display().to_string()));
                    }
                    menu
                }
            });
            if let Some(dir) = cwd.clone() {
                menu = menu.item(PopupMenuItem::new("Show folder in Finder").on_click(move |_, _, cx| cx.reveal_path(&dir)));
            }
            if let Some((pid, _)) = project.clone() {
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new("Project settings").on_click(move |_, _, cx| {
                    let pid = pid.clone();
                    let _ = ws.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
                }));
            }
            // A thread in a worktree asks whether its worktree goes too.
            let leave = |label: &'static str, leave: Leave| {
                let (ws, tid) = (ws.clone(), tid.clone());
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    if let Some(ws) = ws.upgrade() {
                        crate::worktree_ui::confirm_leave(ws, tid.clone(), leave, window, cx);
                    }
                })
            };
            if in_worktree {
                return menu.separator().item(leave("Archive thread…", Leave::Archive)).item(leave("Delete…", Leave::Delete).icon(crate::assets::Lucide::Trash));
            }
            // A thread with sub-agents asks first: they go with it.
            let archive = match archive_note.clone() {
                None => item("Archive thread", |ws, id, cx| ws.archive(id, cx)),
                Some(note) => PopupMenuItem::new("Archive thread…").on_click({
                    let (ws, tid, title) = (ws.clone(), tid.clone(), title.clone());
                    move |_, window, cx| {
                        let (ws, tid, title, note) = (ws.clone(), tid.clone(), title.clone(), note.clone());
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            let (ws, tid) = (ws.clone(), tid.clone());
                            alert.title(format!("Archive “{title}”?")).description(note.clone()).confirm().ok_text("Archive").on_ok(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.archive(&tid, cx));
                                true
                            })
                        });
                    }
                }),
            };
            menu.separator().item(archive).item(PopupMenuItem::new("Delete…").icon(crate::assets::Lucide::Trash).on_click({
                let (ws, tid, title, delete_note) = (ws.clone(), tid.clone(), title.clone(), delete_note.clone());
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
    fn with_project_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(&self, row: E, id: String, name: String, path: std::path::PathBuf) -> impl IntoElement {
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
                .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&p1)))
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

    fn open_rename_dialog(&mut self, id: String, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.rename_input.clone();
        input.update(cx, |s, cx| s.set_value(title, window, cx));
        self.renaming = Some(id.clone());
        // Put the cursor in the field once the dialog is up, with the old title selected.
        let focus = input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| {
                s.focus(window, cx);
                let len = s.value().len();
                s.set_selected_range(0..len, cx);
            })
        });
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let (input2, ws, id) = (input.clone(), ws.clone(), id.clone());
            dialog
                .title("Rename thread")
                .w(px(440.))
                .child(Input::new(&input))
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(gpui_kit::component::button::Button::new("cancel-rename").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("do-rename").primary().label("Rename").on_click(move |_, _, cx| {
                                let title = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.rename(&id, title, cx));
                            }),
                        )),
                )
        });
    }

    fn label(text: &str, cx: &App) -> impl IntoElement {
        div().px_5().pt_3().pb_1().text_xs().text_color(cx.theme().muted_foreground).child(text.to_string())
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
                let peak = self.workspace.read(cx).usage_rows().iter().flat_map(|r| r.limits.iter().map(|l| l.percent)).fold(0.0f32, f32::max);
                let tint = if peak >= 95. { Some(palette::red(cx)) } else if peak >= 80. { Some(palette::amber(cx)) } else { None };
                let icon = Icon::new(crate::assets::Lucide::ChartNoAxesColumn);
                ui::icon_button("usage", match tint { Some(c) => icon.text_color(c), None => icon }, "Usage").selected(usage_open)
            })
            .content({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |this, cx| this.usage_card(cx))
            });
        let busy = matches!(update, UpdateStatus::Checking | UpdateStatus::Downloading { .. });
        let ready = matches!(update, UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. });
        // Found but not downloaded (automatic downloads off): say so, in neutral; ember is for
        // the one-click "restart into it".
        let available = matches!(update, UpdateStatus::Available { .. });
        let label_color = if ready { palette::ember(cx) } else { theme.foreground };
        // Just updated: the pill offers what the update brought, until it's been opened.
        let whats_new = !ready && !available && !busy && self.workspace.read(cx).whats_new().is_some();
        let updater = Popover::new("updater-popover")
            .anchor(Anchor::BottomRight)
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
                    .ghost(!ready && !available && !whats_new)
                    .selected(updater_open)
                    .child(if busy {
                        Spinner::new().xsmall().color(theme.muted_foreground).into_any_element()
                    } else if whats_new {
                        Icon::new(crate::assets::Lucide::Sparkles).small().text_color(palette::ember(cx)).into_any_element()
                    } else {
                        let color = if ready || available { label_color } else { theme.muted_foreground };
                        Icon::new(IconName::RefreshCw).small().text_color(color).into_any_element()
                    })
                    .when(ready || available, |el| el.child(div().text_xs().text_color(label_color).child("Update")))
                    .when(whats_new, |el| el.child(div().text_xs().child("What's new"))),
            )
            .content(move |_, _, cx| this.update(cx, |this, cx| this.updater_card(cx)));
        // New agent CLI versions: a quiet pill with how many, only while there's one to install.
        // Kept while its card is open, so the last update finishing doesn't take the card away.
        let agent_updates = {
            let ws = self.workspace.read(cx);
            crate::agent_updates::badge(&ws.agent_updates, ws.settings.updates.check_agents)
        }.or(self.agent_updates_open.then_some((0, false))).map(|(count, running)| {
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
                            Spinner::new().xsmall().color(theme.muted_foreground).into_any_element()
                        } else {
                            Icon::new(crate::assets::Lucide::CircleArrowUp).small().text_color(theme.muted_foreground).into_any_element()
                        })
                        .when(count > 0, |el| el.child(div().text_xs().child(count.to_string()))),
                )
                .content(move |_, _, cx| crate::agent_updates::card(&ws, cx))
        });
        h_flex()
            .px_3()
            .py_2()
            .gap_1()
            .child(ui::icon_button("open-settings", IconName::Settings, "Settings (⌘,)").selected(in_settings).on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx))
            })))
            .child(ui::icon_button("open-git", crate::assets::Lucide::GitCompare, "Source control").on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(PanelTool::Git)))
            })))
            .child(usage)
            .child(div().flex_1())
            .when(importing, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground)))
            .children(agent_updates)
            .child(updater)
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
        let rows = ws.usage_rows();
        let bar = |pct: f32, cx: &App| {
            let color = if pct >= 90. { palette::red(cx) } else if pct >= 70. { palette::amber(cx) } else { cx.theme().foreground.opacity(0.85) };
            div().h(px(5.)).w_full().rounded_full().bg(cx.theme().foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(color).w(relative((pct / 100.).clamp(0.0, 1.0))))
        };
        let muted = |text: String| div().text_xs().text_color(theme.muted_foreground).child(text);
        ui::menu_surface(cx)
            .id("usage-card")
            .test_support()
            .w(px(320.))
            .p(px(14.))
            .gap(px(14.))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().text_sm().font_semibold().child(if picking { "Show in Usage" } else { "Usage" }))
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
                el.child(div().text_sm().text_color(theme.muted_foreground).child(if ws.usage_picked() { "No providers picked. Choose some with …" } else { "No plan usage reported by your agents." }))
            })
            .when(!picking, |el| el.children(rows.into_iter().map(|u| {
                let agent = u.agent.clone();
                v_flex()
                    .id(SharedString::from(format!("usage-row-{}", agent.key())))
                    .test_support()
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(ui::agent_logo(&agent, px(16.), cx))
                            .child(div().text_sm().font_medium().child(agent.display_name()))
                            .child(div().flex_1())
                            .when_some(u.plan.clone(), |el, p| el.child(div().text_xs().text_color(theme.muted_foreground).child(p))),
                    )
                    .when(u.no_limits, |el| el.child(muted("No usage limits on this plan.".into())))
                    .when_some(u.error.clone(), |el, e| el.child(div().text_xs().text_color(palette::amber(cx)).child(e)))
                    .when_some(u.note.clone(), |el, n| el.child(muted(n)))
                    .children(u.limits.iter().map(|l| {
                        let resets = l.resets_at.map(time::until).unwrap_or_default();
                        v_flex()
                            .gap(px(5.))
                            .child(
                                h_flex()
                                    .text_xs()
                                    .child(div().flex_1().child(l.label.clone()))
                                    .child(div().text_color(theme.muted_foreground).child(format!("{:.0}% left", (100. - l.percent).clamp(0., 100.)))),
                            )
                            .child(bar(l.percent, cx))
                            .when(!resets.is_empty(), |el| el.child(muted(format!("Resets {resets}"))))
                    }))
                    // No plan to show: what Trek recorded it using today.
                    .when(u.limits.is_empty(), |el| {
                        el.child(muted(match &u.today {
                            Some((n, spend)) if spend.priced() => format!("Today in Trek: {} tokens · ≈ {}", crate::cost::fmt_tokens(*n), crate::cost::usd(spend.usd())),
                            Some((n, _)) => format!("Today in Trek: {} tokens", crate::cost::fmt_tokens(*n)),
                            None if u.plan.is_none() && !u.no_limits && u.error.is_none() => "Nothing used today.".to_string(),
                            None => String::new(),
                        }))
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
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .child(
                                    h_flex()
                                        .child(div().flex_1().text_xs().font_medium().child("Usage limit resets"))
                                        .child(
                                            div()
                                                .text_xs()
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
                                                .child(div().text_xs().truncate().child(r.title.clone()))
                                                .when(!expiry.is_empty(), |e| {
                                                    e.child(div().text_xs().text_color(theme.muted_foreground).child(expiry))
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
                    .child(div().flex_1().text_size(px(12.5)).child(agent.display_name()))
                    .when(on, |el| el.child(Icon::new(IconName::Check).xsmall().text_color(palette::ember(cx))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let shown = this.workspace.update(cx, |ws, cx| ws.toggle_usage_shown(&agent, cx));
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
            .when_some(hint, |el, (text, color)| el.child(div().id("usage-pick-hint").test_support().px(px(8.)).pt(px(6.)).text_xs().text_color(color).child(text)))
            .child(
                h_flex()
                    .px(px(8.))
                    .pt(px(8.))
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(if picked { "Picked by you" } else { "Picked automatically" }))
                    .when(picked, |el| {
                        el.child(
                            ui::Pill::new("usage-pick-auto").ghost(true).small(true).child(div().text_xs().child("Pick automatically")).on_click(cx.listener(|this, _, _, cx| {
                                this.usage_refused = false;
                                this.workspace.update(cx, |ws, cx| ws.show_usage_automatically(cx));
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
        let view = ws.update_view();
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
                let link = c.compare.map(|url| ("Compare on GitHub", url)).or_else(|| c.releases.first().filter(|r| !r.url.is_empty()).map(|r| ("Release on GitHub", r.url.clone())));
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
            .child(h_flex().gap_2().child(crate::brand::logo_mark(px(16.))).child(div().text_sm().font_semibold().child(format!("Trek {}", trek_core::VERSION))).child(div().flex_1()).child(div().text_xs().text_color(theme.muted_foreground).child(channel)))
            .child(div().text_sm().text_color(theme.muted_foreground).child(view.line))
            .when_some(view.progress, |el, p| {
                el.child(div().h(px(5.)).w_full().rounded_full().bg(theme.foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(palette::ember(cx)).w(relative(p))))
            })
            .when_some(changes, |el, (title, releases, link)| {
                el.child(
                    v_flex()
                        .gap(px(8.))
                        .pt(px(10.))
                        .border_t_1()
                        .border_color(theme.foreground.opacity(0.07))
                        .child(div().text_xs().font_medium().child(title))
                        .child(ui::releases_notes("updater-notes", &releases, px(260.), cx))
                        .when_some(link, |el, (label, url)| el.child(ui::web_link("updater-link", label, url, cx))),
                )
            })
            .when_some(view.action, |el, action| {
                el.child(
                    gpui_kit::component::button::Button::new("updater-action")
                        .small()
                        .w_full()
                        .when(action == UpdateAction::Restart, |b| b.primary())
                        .when(action != UpdateAction::Restart, |b| b.outline())
                        .loading(view.busy)
                        .label(action.label())
                        .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.run_update_action(action, cx)))),
                )
            })
            .into_any_element()
    }
}

/// A card's sub-agents by agent, in the order they first appear, with how many of each.
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

/// A card's tooltip while it has work out: its sub-agents, then what runs in the background.
/// Where a project's badge goes, for threads in no project: a chat bubble, not a monogram.
fn no_project_badge(cx: &App) -> AnyElement {
    div()
        .flex_none()
        .h(px(16.))
        .w(px(20.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().foreground.opacity(0.07))
        .child(Icon::new(crate::assets::Lucide::MessageSquare).size(px(11.)).text_color(cx.theme().muted_foreground))
        .into_any_element()
}

pub(crate) fn card_tip(kids: &[(trek_core::AgentId, String)], background: &[String]) -> Option<String> {
    let kids: Vec<String> = kids.iter().map(|(_, name)| name.clone()).collect();
    let parts: Vec<String> = [("Sub-agents at work", &kids[..]), ("Running in the background", background)]
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(head, names)| format!("{head}:\n{}", names.join("\n")))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// What a thread waiting on the user waits for, from its pending request.
pub(crate) fn needs_label(ws: &Workspace, id: &str) -> &'static str {
    match ws.live.get(id).and_then(|l| l.permissions.first()).map(|p| p.prompt.as_ref()) {
        Some(Some(trek_agents::Prompt::Questions(_))) => "Question",
        Some(Some(trek_agents::Prompt::Plan(_))) => "Plan ready",
        Some(None) => "Needs approval",
        None => "Needs you",
    }
}

/// The inbox's cards in order: what waits on the user (approvals, failures), then what's running
/// now, then the rest of the inbox, newest first as `Thread::inbox_rank` has them. Running threads
/// mustn't sink below a day's worth of finished ones and out of view.
fn live_order(inbox: Vec<Thread>, working: Vec<Thread>) -> Vec<Thread> {
    let (waiting, rest): (Vec<Thread>, Vec<Thread>) = inbox.into_iter().partition(|t| t.needs_you());
    waiting.into_iter().chain(working).chain(rest).collect()
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("Sidebar");
        let ws = self.workspace.read(cx);
        self.graph = Graph { parents: ws.threads.iter().filter_map(|t| t.parent_id.clone()).collect(), needs: ws.waiting_on_sub_agents(), waiting: ws.waiting_threads() };
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
        let mut settled_count = 0;
        let sections: Vec<(Section, Vec<Thread>)> = ws
            .sections()
            .into_iter()
            .map(|(s, v)| {
                if s == Section::Settled {
                    settled_count = v.len();
                    if !settled_open {
                        return (s, vec![]);
                    }
                }
                (s, v.into_iter().cloned().collect())
            })
            .collect();
        let names: HashMap<String, String> = ws.projects.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let paths: HashMap<String, std::path::PathBuf> = ws.projects.iter().map(|p| (p.id.clone(), p.path.clone())).collect();
        let looks: HashMap<String, crate::ui::ProjectLook> = ws.projects.iter().map(|p| (p.id.clone(), ws.project_look(&p.path))).collect();
        let project_of = |t: &Thread| t.project_id.as_ref().and_then(|p| names.get(p).cloned()).unwrap_or_else(|| "No project".into());
        let theme = cx.theme().clone();

        let mut list = v_flex().pt_1().pb_3();
        let mut settled: Vec<Thread> = vec![];
        // Inbox and Working are one run of cards: see `live_order`.
        let live: Vec<Thread> = {
            let of = |s: Section| sections.iter().find(|(x, _)| *x == s).map(|(_, v)| v.clone()).unwrap_or_default();
            live_order(of(Section::Inbox), of(Section::Working))
        };
        // Pinned or snoozed threads showing mean the live run isn't really empty.
        let named_sections_busy = sections.iter().any(|(s, v)| matches!(s, Section::Pinned | Section::Snoozed) && !v.is_empty());
        let mut pinned: HashSet<String> = HashSet::new();
        for (section, threads) in sections {
            match section {
                Section::Settled => settled = threads,
                Section::Pinned | Section::Snoozed => {
                    list = list.child(crate::motion::flip_row(&flip, format!("label-{}", section.label()), Self::label(section.label(), cx), now, window));
                    for t in &threads {
                        pinned.insert(t.id.clone());
                        list = list.child(crate::motion::flip_row(&flip, t.id.clone(), self.card(t, &project_of(t), true, selected.as_deref() == Some(&t.id), cx), now, window));
                    }
                }
                Section::Inbox | Section::Working => {}
            }
        }
        // Every live thread sits under its project: what has something to say (it needs you, it
        // failed, it's at work) first, then the quiet rest capped at three (every unread row and
        // the open one stay out), all at a line's height. A folded group keeps its badges —
        // needs-you, working, unread — on the header.
        let attention = |t: &Thread| t.needs_you() || t.run_state == RunState::Working || self.graph.needs.contains(&t.id) || self.graph.waiting.contains(&t.id);
        let mut by_id: HashMap<String, Thread> = HashMap::new();
        let mut info: HashMap<String, GroupInfo> = HashMap::new();
        let mut fresh: Vec<LiveGroup> = vec![];
        {
            let mut groups: Vec<(String, String, Vec<Thread>, Vec<Thread>)> = Vec::new();
            for t in live {
                let pid = t.project_id.clone().unwrap_or_default();
                let ix = groups.iter().position(|g| g.0 == pid).unwrap_or_else(|| {
                    groups.push((pid, project_of(&t), vec![], vec![]));
                    groups.len() - 1
                });
                let g = &mut groups[ix];
                if attention(&t) {
                    g.2.push(t);
                } else {
                    g.3.push(t);
                }
            }
            // Groups with something that needs you first, then unread, then their newest activity.
            groups.sort_by(|a, b| {
                let attention = |g: &(String, String, Vec<Thread>, Vec<Thread>)| !g.2.is_empty();
                let unseen = |g: &(String, String, Vec<Thread>, Vec<Thread>)| g.2.iter().chain(g.3.iter()).any(|t| t.is_unseen());
                let newest = |g: &(String, String, Vec<Thread>, Vec<Thread>)| g.2.iter().chain(g.3.iter()).map(|t| t.updated_at).max().unwrap_or(0);
                attention(b)
                    .cmp(&attention(a))
                    .then(unseen(b).cmp(&unseen(a)))
                    .then(newest(b).cmp(&newest(a)))
                    .then_with(|| a.1.cmp(&b.1))
            });
            for (pid, name, cards, mut items) in groups {
                items.sort_by(|a, b| b.is_unseen().cmp(&a.is_unseen()).then(b.updated_at.cmp(&a.updated_at)));
                let folded = self.collapsed_live.contains(&pid) && !searching;
                let open = self.open_live_projects.contains(&pid);
                let shown: HashSet<&str> = if open || searching {
                    items.iter().map(|t| t.id.as_str()).collect()
                } else {
                    let mut shown: HashSet<&str> = items
                        .iter()
                        .filter(|t| t.is_unseen() || selected.as_deref() == Some(t.id.as_str()))
                        .map(|t| t.id.as_str())
                        .collect();
                    for t in &items {
                        if shown.len() >= 3 {
                            break;
                        }
                        shown.insert(t.id.as_str());
                    }
                    shown
                };
                let more = items.len() - shown.len();
                let rows: Vec<String> = if folded {
                    vec![]
                } else {
                    cards.iter().map(|t| t.id.clone()).chain(items.iter().filter(|t| shown.contains(t.id.as_str())).map(|t| t.id.clone())).collect()
                };
                let footer = !folded && !searching && ((!open && more > 0) || (open && items.len() > 3));
                info.insert(
                    pid.clone(),
                    GroupInfo {
                        name,
                        unread: cards.iter().chain(items.iter()).filter(|t| t.is_unseen()).count(),
                        needs: cards.iter().filter(|t| t.needs_you() || self.graph.needs.contains(&t.id)).count(),
                        working: cards.iter().filter(|t| t.run_state == RunState::Working || self.graph.waiting.contains(&t.id)).count(),
                        total: cards.len() + items.len(),
                        folded,
                        open,
                        more,
                    },
                );
                fresh.push(LiveGroup { pid, rows, footer });
                by_id.extend(cards.into_iter().chain(items).map(|t| (t.id.clone(), t)));
            }
        }
        // While the pointer is over the sidebar its rows hold their places: a thread answered,
        // stopped or come back with news there mustn't slide another under the next click. What
        // changed shows once the pointer leaves (`hover_changed`); what the user does here (folding,
        // "Show more", searching) shows at once.
        let before = self.shown.clone();
        let layout: Vec<LiveGroup> = match self.shown.take().filter(|_| self.hovered && !searching) {
            Some(held) => {
                self.deferred = held != fresh;
                let ws = self.workspace.read(cx);
                held.into_iter()
                    .map(|mut g| {
                        // A row that left the live list (settled, say) keeps its place till then;
                        // one archived or deleted goes.
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
            let drawn: HashSet<&str> = layout.iter().flat_map(|g| g.rows.iter().map(String::as_str)).collect();
            let in_history: HashSet<&str> = if settled_open { settled.iter().map(|t| t.id.as_str()).collect() } else { HashSet::new() };
            for g in &before {
                for (ix, id) in g.rows.iter().enumerate() {
                    if drawn.contains(id.as_str()) || pinned.contains(id) || in_history.contains(id.as_str()) || by_id.contains_key(id) || self.ghosts.iter().any(|x| &x.id == id) {
                        continue;
                    }
                    let (Some((thread, _, card)), Some(height)) = (self.last_rows.get(id), self.flip.borrow().height(id)) else { continue };
                    let after = g.rows[..ix].iter().rev().find(|r| drawn.contains(r.as_str())).cloned();
                    let mut left = crate::motion::Spring::new(crate::motion::SURFACE, crate::motion::UNIT, 1., now);
                    left.set(0., true, now);
                    self.ghosts.push(Ghost { id: id.clone(), pid: g.pid.clone(), after, thread: thread.clone(), card: *card, height, left });
                }
            }
        }
        self.ghosts.retain(|g| g.left.moving(now) && layout.iter().any(|l| l.pid == g.pid));
        let mut last_rows: HashMap<String, (Thread, String, bool)> = HashMap::new();
        // Each project's header and rows move as one (`flip_stack`).
        let mut live_groups: Vec<(SharedString, AnyElement)> = vec![];
        for g in layout {
            let pid = g.pid;
            let mut group = v_flex().w_full();
            let i = info.remove(&pid).unwrap_or_else(|| GroupInfo { name: names.get(&pid).cloned().unwrap_or_else(|| "No project".into()), ..GroupInfo::default() });
            let extra = h_flex()
                .gap_2()
                .when(i.folded && i.needs > 0, |el| el.child(div().text_xs().text_color(palette::needs_you(cx)).child(i.needs.to_string())))
                .when(i.folded && i.working > 0, |el| el.child(div().text_xs().text_color(palette::working(cx)).child(i.working.to_string())))
                .when(i.unread > 0, |el| el.child(div().text_xs().text_color(palette::emerald(cx)).child(format!("{} new", i.unread))))
                .child(div().text_xs().child(i.total.to_string()))
                .into_any_element();
            group = group.child(crate::motion::flip_row(&flip, format!("live-h-{pid}"), self.group_header("live", &pid, i.name.clone(), Some(extra), Some(i.folded), &paths, &looks, cx), now, window));
            // The group's rows top to bottom, with what's fading away where it was.
            let mut seq: Vec<Result<&String, usize>> = g.rows.iter().map(Ok).collect();
            for (gx, ghost) in self.ghosts.iter().enumerate().filter(|(_, x)| x.pid == pid) {
                let at = match &ghost.after {
                    None => 0,
                    Some(a) => seq.iter().position(|r| matches!(r, Ok(id) if *id == a)).map_or(seq.len(), |p| p + 1),
                };
                seq.insert(at, Err(gx));
            }
            for row in seq {
                match row {
                    Ok(id) => {
                        let Some(t) = by_id.get(id) else { continue };
                        let sel = selected.as_deref() == Some(id.as_str());
                        let card = attention(t);
                        last_rows.insert(id.clone(), (t.clone(), pid.clone(), card));
                        let row = if card { self.card(t, &i.name, false, sel, cx) } else { self.live_line(t, sel, cx) };
                        group = group.child(crate::motion::flip_row(&flip, id.clone(), row, now, window));
                    }
                    Err(gx) => {
                        let ghost = &self.ghosts[gx];
                        let left = ghost.left.frame(now, window).clamp(0., 1.);
                        let row = if ghost.card { self.card(&ghost.thread, &i.name, false, false, cx) } else { self.live_line(&ghost.thread, false, cx) };
                        // Out of the layout at once (the rows under it slide up to close the gap),
                        // it fades where it was, under them, inert: a click on it does nothing.
                        let row = div().h(px(0.)).child(crate::motion::inert(div().absolute().top_0().left_0().w_full().h(px(ghost.height)).opacity(left).child(row)));
                        group = group.child(crate::motion::flip_row(&flip, ghost.id.clone(), row, now, window));
                    }
                }
            }
            if g.footer {
                let pid2 = pid.clone();
                let (id, label) = match i.open {
                    true => (format!("live-less-{pid}"), "Show less".to_string()),
                    false if i.more > 0 => (format!("live-more-{pid}"), format!("Show {} more", i.more)),
                    false => (format!("live-more-{pid}"), "Show more".to_string()),
                };
                group = group.child(crate::motion::flip_row(
                    &flip,
                    format!("live-f-{pid}"),
                    div()
                        .id(SharedString::from(id))
                        .test_support()
                        .mx_2()
                        .pl(px(30.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.open_live_projects.remove(&pid2) {
                                this.open_live_projects.insert(pid2.clone());
                            }
                            this.thaw(cx);
                        })),
                    now,
                    window,
                ));
            }
            live_groups.push((format!("live-g-{pid}").into(), group.into_any_element()));
        }
        list = list.child(crate::motion::flip_stack(&flip, live_groups, now));
        self.last_rows = last_rows;
        if !live_shown && !searching && !named_sections_busy {
            list = list.child(
                v_flex()
                    .mx_4()
                    .my_2()
                    .px_3()
                    .py_3()
                    .gap_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().text_sm().text_color(theme.foreground.opacity(0.8)).child(if importing { "Finding your threads…" } else { "Inbox zero" }))
                    .child("Running agents and anything that needs you will show up here."),
            );
        }

        // Settled history, grouped by project (Codex style).
        let mut history = v_flex();
        if settled_count > 0 {
            let mut groups: Vec<(String, String, Vec<Thread>)> = Vec::new();
            for t in settled.iter().cloned() {
                let pid = t.project_id.clone().unwrap_or_default();
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
                h_flex()
                    .id("settled-toggle")
                    .mx_2()
                    .px_3()
                    .h(px(30.))
                    .gap_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("Settled ({settled_count})"))
                    .child(div().flex_1().h(px(1.)).bg(theme.border))
                    .child(Icon::new(if settled_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| {
                            ws.settled_open = !ws.settled_open;
                            cx.notify();
                        })
                    })),
                now,
                window,
            ));
            if settled_open {
                let mut settled_groups: Vec<(SharedString, AnyElement)> = vec![];
                for (pid, name, items) in groups {
                    let open = self.open_projects.contains(&pid);
                    let shown = if open || searching { items.len() } else { items.len().min(5) };
                    let pid2 = pid.clone();
                    let folded = self.collapsed_settled.contains(&pid) && !searching;
                    let mut group = v_flex().w_full().child(crate::motion::flip_row(&flip, format!("settled-h-{pid}"), self.group_header("settled", &pid, name.clone(), None, Some(folded), &paths, &looks, cx), now, window));
                    for t in items.iter().take(if folded { 0 } else { shown }) {
                        group = group.child(crate::motion::flip_row(&flip, t.id.clone(), self.line(t, selected.as_deref() == Some(&t.id), cx), now, window));
                    }
                    if items.len() > 5 && !searching && !folded {
                        group = group.child(
                            div()
                                .id(SharedString::from(format!("more-{pid}")))
                                .mx_2()
                                .pl(px(30.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child(if open { "Show less" } else { "Show more" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.open_projects.remove(&pid2) {
                                        this.open_projects.insert(pid2.clone());
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    settled_groups.push((format!("settled-g-{pid}").into(), group.into_any_element()));
                }
                history = history.child(crate::motion::flip_stack(&flip, settled_groups, now));
            }
        }

        self.flip.borrow_mut().end();
        v_flex()
            .id("sidebar")
            .test_support()
            .w(px(crate::root::SIDEBAR_WIDTH))
            .h_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.hover_changed(*hovered, cx)))
            .flex_none()
            .when(!self.workspace.read(cx).see_through(), |el| el.bg(theme.sidebar))
            .child(self.top(cx))
            .child(div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().child(crate::motion::flip_scope(&flip, v_flex().w_full().child(list).child(history))))
            .child(self.footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::live_order;
    use trek_core::store::{Store, Thread};
    use trek_core::{AgentId, Effort, HandHolding, RunState};

    fn thread(s: &Store, title: &str, state: RunState) -> Thread {
        let mut t = s.create_thread(None, AgentId::ClaudeCode, None, Effort::Medium, HandHolding::Auto).unwrap();
        t.title = title.into();
        t.run_state = state;
        t
    }

    #[test]
    fn running_threads_come_after_what_waits_on_you_and_before_the_rest() {
        let s = Store::in_memory().unwrap();
        let inbox = vec![thread(&s, "asks", RunState::NeedsYou), thread(&s, "failed", RunState::Failed), thread(&s, "done", RunState::Idle), thread(&s, "older", RunState::Idle)];
        let working = vec![thread(&s, "running", RunState::Working)];
        let titles: Vec<String> = live_order(inbox, working).into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["asks", "failed", "running", "done", "older"]);
    }
}
