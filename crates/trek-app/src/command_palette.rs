//! ⌘K: one field over threads (titles first, then what was said in them), projects and commands.
//! Choosing a message match opens its thread scrolled to that message.

use crate::panels::RightPanel;
use crate::ui;
use crate::workspace::{ItemRef, PanelTool, Route, SettingsPage, Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;
use std::path::PathBuf;
use trek_core::settings::ThemeChoice;
use trek_core::store::SearchHit;
use trek_core::{AgentId, HandHolding};

const WIDTH: f32 = 560.;
/// Rows per group: recent threads (empty field), title and message matches, projects, commands.
const RECENT: usize = 6;
const TITLE_HITS: usize = 6;
const MESSAGE_HITS: usize = 8;
const PROJECTS: usize = 6;
/// Enough for every settings page ("settings" lists them all).
const COMMANDS: usize = 20;
/// Title matches further in than this are brought forward, so the matched words stay in view.
const TITLE_LEAD: usize = 32;

/// What choosing an entry does.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    OpenThread(String),
    OpenMessage(String, ItemRef),
    NewThreadIn(PathBuf),
    ProjectSettings(String),
    NewThread,
    OpenFolder,
    Basecamp,
    Notes,
    NewChat,
    Glass(bool),
    Settings(SettingsPage),
    ToggleSidebar,
    ToggleTools,
    OpenTool(PanelTool),
    Theme(ThemeChoice),
    HandHolding(HandHolding),
    Settle(String),
    Fork(String),
    CheckForUpdates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Threads,
    Projects,
    Commands,
}

impl Group {
    fn label(self, searching: bool) -> &'static str {
        match self {
            Group::Threads if searching => "Threads",
            Group::Threads => "Recent threads",
            Group::Projects => "Projects",
            Group::Commands => "Commands",
        }
    }
}

#[derive(Clone)]
enum Glyph {
    Agent(AgentId),
    Project(String, ui::ProjectLook),
    Icon(Icon),
}

#[derive(Clone)]
struct Entry {
    group: Group,
    glyph: Glyph,
    label: SharedString,
    /// Matched words in the label (thread titles).
    label_ranges: Vec<Range<usize>>,
    /// A second line: the matching excerpt of a message.
    snippet: Option<(SharedString, Vec<Range<usize>>)>,
    /// Quiet text on the right: a shortcut, a project, a folder.
    hint: Option<SharedString>,
    checked: bool,
    action: Action,
}

impl Entry {
    fn new(group: Group, glyph: Glyph, label: impl Into<SharedString>, action: Action) -> Self {
        Entry { group, glyph, label: label.into(), label_ranges: vec![], snippet: None, hint: None, checked: false, action }
    }

    fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// How well `query` matches a command or project: higher is better, `None` is no match. Case
/// is ignored. The label starting with the query beats a word in it starting with the query,
/// which beats every query word starting some word, then a substring, then a keyword, then the
/// query's letters appearing in order (a typo-tolerant last resort). Within the first two, the
/// whole label or a whole word scores a little higher; other ties keep the candidates' order
/// ("settings" lists the settings pages in the order Settings shows them).
pub fn score(query: &str, label: &str, keywords: &str) -> Option<u32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let words = |s: &str| s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let label_words = words(&label);
    if label == q {
        return Some(1100);
    }
    if let Some(rest) = label.strip_prefix(&q) {
        let whole_word = !rest.starts_with(char::is_alphanumeric);
        return Some(1000 + if whole_word { 10 } else { 0 });
    }
    if label_words.iter().any(|w| w.starts_with(&q)) {
        return Some(800 + if label_words.contains(&q) { 10 } else { 0 });
    }
    let query_words = words(&q);
    if !query_words.is_empty() && query_words.iter().all(|qw| label_words.iter().any(|w| w.starts_with(qw.as_str()))) {
        return Some(600);
    }
    if label.contains(&q) {
        return Some(400);
    }
    // Below here a one- or two-letter query would match nearly everything.
    if q.chars().count() < 3 {
        return None;
    }
    let keyword_words = words(&keywords.to_lowercase());
    if !query_words.is_empty() && query_words.iter().all(|qw| keyword_words.iter().chain(&label_words).any(|w| w.starts_with(qw.as_str()))) {
        return Some(300);
    }
    let mut chars = label.chars();
    if q.chars().filter(|c| !c.is_whitespace()).all(|c| chars.any(|l| l == c)) {
        return Some(100);
    }
    None
}

/// Where row `selected` sits among the list's children, which include a label before each
/// group's first row. Moving onto a group's first row brings its label into view too, so that
/// row reports the label's place.
fn scroll_target(groups: &[Group], selected: usize) -> Option<usize> {
    let mut labels = 0;
    for (i, g) in groups.iter().enumerate() {
        let first_of_group = i == 0 || groups[i - 1] != *g;
        if first_of_group {
            labels += 1;
        }
        if i == selected {
            return Some(if first_of_group { i + labels - 1 } else { i + labels });
        }
    }
    None
}

/// Whether `query` names one of `commands`: it starts the label or one of its words.
fn commands_named<T>(query: &str, commands: &[(T, String, String)]) -> bool {
    query.chars().count() >= 2 && commands.iter().any(|(_, label, keywords)| score(query, label, keywords).is_some_and(|s| s >= 800))
}

/// Characters of a folder shown as a hint, at most.
const FOLDER_HINT: usize = 30;

/// `path` cut to its last folders, to fit in about `max` characters: projects side by side in one
/// parent differ only at the end. A last folder that's too long on its own is kept whole.
fn path_tail(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let mut tail: Vec<&str> = vec![];
    let mut len = 2;
    for part in path.rsplit('/').filter(|p| !p.is_empty()) {
        let n = part.chars().count() + 1;
        if !tail.is_empty() && len + n > max {
            break;
        }
        len += n;
        tail.push(part);
    }
    tail.reverse();
    format!("…/{}", tail.join("/"))
}

/// The search results the Threads group shows: title matches, then message matches (one per
/// thread, `Store::search` sees to that) from threads not listed already.
fn thread_hits(hits: &[SearchHit]) -> Vec<&SearchHit> {
    let (titles, messages): (Vec<&SearchHit>, Vec<&SearchHit>) = hits.iter().partition(|h| h.position.is_none());
    let titles: Vec<&SearchHit> = titles.into_iter().take(TITLE_HITS).collect();
    let listed: std::collections::HashSet<&str> = titles.iter().map(|h| h.thread_id.as_str()).collect();
    let messages = messages.into_iter().filter(|h| !listed.contains(h.thread_id.as_str())).take(MESSAGE_HITS);
    titles.into_iter().chain(messages).collect()
}

/// Titles can hold line breaks (a pasted first message); rows show them on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep the candidates `query` matches, best first; equal scores keep their order.
fn rank<T>(query: &str, candidates: Vec<(T, String, String)>) -> Vec<T> {
    let mut scored: Vec<(u32, usize, T)> =
        candidates.into_iter().enumerate().filter_map(|(i, (item, label, keywords))| score(query, &label, &keywords).map(|s| (s, i, item))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

pub struct CommandPalette {
    workspace: Entity<Workspace>,
    right_panel: Entity<RightPanel>,
    input: Entity<InputState>,
    pub open: bool,
    query: String,
    selected: usize,
    /// Full-text results and the query they answer (kept on screen until the next ones arrive).
    hits: Vec<SearchHit>,
    hits_for: String,
    /// `Workspace::search_epoch` the hits were searched at; a newer one searches again.
    epoch: u64,
    /// Enter came before the results for the current text: confirm once they're in.
    confirm_when_ready: bool,
    scroll: ScrollHandle,
    /// Where focus was before the palette opened; it goes back there on close.
    restore: Option<FocusHandle>,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub fn new(workspace: Entity<Workspace>, right_panel: Entity<RightPanel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads, projects and commands"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    this.set_query(q, window, cx);
                }
            }),
            // The index took in more (background indexing, a finished turn): search again.
            cx.observe_in(&workspace, window, |this, ws, window, cx| {
                if this.open && ws.read(cx).search_epoch != this.epoch && !this.query.trim().is_empty() {
                    this.search(window, cx);
                }
            }),
        ];
        Self {
            workspace,
            right_panel,
            input,
            open: false,
            query: String::new(),
            selected: 0,
            hits: vec![],
            hits_for: String::new(),
            epoch: 0,
            confirm_when_ready: false,
            scroll: ScrollHandle::new(),
            restore: None,
            _search: None,
            _subscriptions: subscriptions,
        }
    }

    /// The row the keyboard is on.
    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open { self.dismiss(window, cx) } else { self.show(window, cx) }
    }

    /// Show the palette, or leave it as it is when it's already open (⌘K from a thread window).
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.show(window, cx);
        }
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.restore = window.focused(cx);
        self.selected = 0;
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.set_query(String::new(), window, cx);
        self.set_overlay(true, cx);
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self._search = None;
        self.confirm_when_ready = false;
        if let Some(h) = self.restore.take() {
            h.focus(window, cx);
        }
        self.set_overlay(false, cx);
        cx.notify();
    }

    /// Native views (the browser) hide while a Trek surface is over them.
    fn set_overlay(&self, open: bool, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            if ws.overlay_open != open {
                ws.overlay_open = open;
                cx.notify();
            }
        });
    }

    fn set_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.query = query;
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        self.search(window, cx);
    }

    /// Results for the current text are still on their way.
    fn pending(&self) -> bool {
        let q = self.query.trim();
        !q.is_empty() && self.hits_for != q
    }

    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.query.trim().to_string();
        let ws = self.workspace.read(cx);
        self.epoch = ws.search_epoch;
        if trek_core::store::fts_query(&q).is_none() {
            self.hits.clear();
            self.hits_for = q;
            self._search = None;
            cx.notify();
            return;
        }
        let store = ws.store.clone();
        self._search = Some(cx.spawn_in(window, async move |this, cx| {
            let query = q.clone();
            // Title matches crowd out message matches from the same thread, so ask for enough
            // of each that MESSAGE_HITS remain.
            let hits = cx.background_executor().spawn(async move { store.search(&query, TITLE_HITS + MESSAGE_HITS) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match hits {
                    Ok(hits) => this.hits = hits,
                    Err(e) => {
                        tracing::warn!("search: {e}");
                        this.hits.clear();
                    }
                }
                this.hits_for = q;
                if std::mem::take(&mut this.confirm_when_ready) {
                    this.confirm(this.selected, window, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn entries(&self, cx: &App) -> Vec<Entry> {
        let ws = self.workspace.read(cx);
        let q = self.query.trim();
        let searching = !q.is_empty();
        let project_name = |pid: &Option<String>| pid.as_ref().and_then(|p| ws.project(p)).map(|p| p.name.clone());
        let mut out = Vec::new();
        // Typing a command's name ("theme", "terminal") puts the commands first: below a page of
        // threads that merely mention the word, the command would be out of sight.
        let commands = self.commands(cx);
        let named = commands_named(q, &commands);
        let commands = rank(q, commands);
        let n = commands.len();
        let commands: Vec<Entry> = commands.into_iter().take(if searching { COMMANDS } else { n }).collect();
        if named {
            out.extend(commands.iter().cloned());
        }

        if searching {
            for h in thread_hits(&self.hits) {
                let Some(t) = ws.thread(&h.thread_id) else { continue };
                let mut e = match ItemRef::of_hit(h) {
                    // A message: the thread's title, and the excerpt under it.
                    Some(at) => {
                        let mut e = Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), one_line(&t.title), Action::OpenMessage(t.id.clone(), at));
                        let (snippet, ranges) = ui::lead_to_match(&h.snippet, &h.ranges, 32);
                        e.snippet = Some((snippet.into(), ranges));
                        e
                    }
                    None => {
                        // Long titles (an imported thread's first prompt) can hide the match past the cut.
                        let (label, ranges) = ui::lead_to_match(&h.snippet, &h.ranges, TITLE_LEAD);
                        let mut e = Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), label, Action::OpenThread(t.id.clone()));
                        e.label_ranges = ranges;
                        e
                    }
                };
                if let Some(p) = project_name(&t.project_id) {
                    e = e.hint(p);
                }
                out.push(e);
            }
        } else {
            // Sub-agents show in their parent; a search still finds them.
            let mut recent: Vec<_> = ws.threads.iter().filter(|t| t.side_of.is_none() && t.parent_id.is_none() && t.archived_at.is_none()).collect();
            recent.sort_by_key(|t| -t.updated_at);
            for t in recent.into_iter().take(RECENT) {
                let mut e = Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), one_line(&t.title), Action::OpenThread(t.id.clone()));
                if let Some(p) = project_name(&t.project_id) {
                    e = e.hint(p);
                }
                out.push(e);
            }
        }

        // Projects: a new thread in one, or its settings (those only when searching).
        let mut projects = vec![];
        for p in ws.workspace_projects() {
            let glyph = Glyph::Project(p.name.clone(), ws.project_look(&p.path));
            let folder = trek_core::paths::tildify(&p.path);
            let keywords = format!("{} {}", p.remote.clone().unwrap_or_default(), folder);
            let folder = path_tail(&folder, FOLDER_HINT);
            let new = Entry::new(Group::Projects, glyph.clone(), format!("New thread in {}", p.name), Action::NewThreadIn(p.path.clone())).hint(folder.clone());
            projects.push((new, p.name.clone(), keywords.clone()));
            if searching {
                let settings = Entry::new(Group::Projects, glyph, format!("{} settings", p.name), Action::ProjectSettings(p.id.clone())).hint(folder);
                projects.push((settings, format!("{} settings", p.name), keywords));
            }
        }
        out.extend(rank(q, projects).into_iter().take(if searching { PROJECTS } else { 4 }));
        if !named {
            out.extend(commands);
        }
        out
    }

    /// Every command, with its ranking label and keywords.
    fn commands(&self, cx: &App) -> Vec<(Entry, String, String)> {
        let ws = self.workspace.read(cx);
        let mut out: Vec<(Entry, String, String)> = vec![];
        let mut add = |entry: Entry, keywords: &str| {
            let label = entry.label.to_string();
            out.push((entry, label, keywords.to_string()));
        };
        let icon = |i: Icon| Glyph::Icon(i);
        let c = Group::Commands;
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::SquarePen)), "New thread", Action::NewThread).hint("⌘N"), "create start chat compose");
        add(Entry::new(c, icon(Icon::new(IconName::FolderOpen)), "Open folder…", Action::OpenFolder).hint("⌘O"), "project add repository");
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::Tent)), "Basecamp", Action::Basecamp).hint("⌘⇧H"), "recap today week summary inbox review usage tokens stats");
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::NotebookPen)), "Notes", Action::Notes).hint("⌘⇧J"), "jot write todo checklist scratch pad memo");
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::MessageSquarePlus)), "New thread without a project", Action::NewChat), "chat scratch no project question");
        let glass = ws.settings.appearance.glass;
        add(
            Entry::new(c, icon(Icon::new(crate::assets::Lucide::Sparkles)), if glass { "Turn off liquid glass" } else { "Turn on liquid glass" }, Action::Glass(!glass)),
            "appearance translucent blur transparent vibrancy theme",
        );
        let thread = ws.current_thread().cloned();
        if let Some(t) = thread.as_ref().filter(|t| t.settled_at.is_none()) {
            add(Entry::new(c, icon(Icon::new(IconName::Check)), "Settle thread", Action::Settle(t.id.clone())).hint("⌘E"), "done finish inbox archive");
        }
        if let Some(t) = thread.as_ref() {
            add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::GitFork)), "Fork thread", Action::Fork(t.id.clone())), "branch copy duplicate conversation");
        }
        // Hand-holding applies to the thread on screen, or to the next new thread.
        if matches!(ws.route, Route::Thread(_) | Route::Draft { .. }) {
            let current = ws.prefs().hand_holding;
            let unlocked = ws.settings.permissions.full_access_unlocked;
            for level in HandHolding::ALL {
                let mut e = Entry::new(c, icon(crate::composer::hand_icon(level)), format!("Hand-holding: {}", level.label()), Action::HandHolding(level)).checked(level == current);
                if level == HandHolding::FullAccess && !unlocked {
                    e = e.hint("Off in Settings");
                }
                add(e, "permissions access mode approval supervise autonomy");
            }
        }
        add(Entry::new(c, icon(Icon::new(IconName::PanelLeft)), "Toggle sidebar", Action::ToggleSidebar).hint("⌘B"), "hide show inbox");
        add(Entry::new(c, icon(Icon::new(IconName::PanelRight)), "Toggle tools panel", Action::ToggleTools).hint("⌘J"), "right panel hide show");
        for tool in PanelTool::ALL {
            add(Entry::new(c, icon(crate::panels::tool_icon(tool)), format!("Open {}", tool.label()), Action::OpenTool(tool)), "tools panel");
        }
        let theme = ws.settings.appearance.theme;
        for (choice, label, glyph) in [
            (ThemeChoice::Night, "Theme: Night", Icon::new(crate::assets::Lucide::Moon)),
            (ThemeChoice::Paper, "Theme: Paper", Icon::new(crate::assets::Lucide::Sun)),
            (ThemeChoice::System, "Theme: Match macOS", Icon::new(crate::assets::Lucide::Monitor)),
        ] {
            add(Entry::new(c, icon(glyph), label, Action::Theme(choice)).checked(choice == theme), "appearance dark light system mode colors");
        }
        for page in crate::settings_view::pages() {
            let mut e = Entry::new(c, icon(crate::settings_view::page_icon(page)), format!("Settings: {}", page.label()), Action::Settings(page));
            if page == SettingsPage::General {
                e = e.hint("⌘,");
            }
            add(e, crate::settings_view::page_blurb(page));
        }
        add(Entry::new(c, icon(Icon::new(IconName::RefreshCw)), "Check for updates", Action::CheckForUpdates), "update version release upgrade");
        out
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let groups: Vec<Group> = self.entries(cx).iter().map(|e| e.group).collect();
        if groups.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(groups.len() as isize) as usize;
        if let Some(child) = scroll_target(&groups, self.selected) {
            self.scroll.scroll_to_item(child);
        }
        cx.notify();
    }

    fn confirm(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries(cx).into_iter().nth(ix) else { return };
        self.dismiss(window, cx);
        let ws = self.workspace.clone();
        match entry.action {
            Action::OpenThread(id) => ws.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx)),
            Action::OpenMessage(id, at) => ws.update(cx, |ws, cx| ws.open_thread_at(&id, at, cx)),
            Action::NewThreadIn(path) => ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(path) }, cx)),
            Action::ProjectSettings(id) => ws.update(cx, |ws, cx| ws.open_project_settings(Some(id), cx)),
            Action::NewThread => ws.update(cx, |ws, cx| ws.new_thread(cx)),
            Action::Basecamp => ws.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx)),
            Action::Notes => ws.update(cx, |ws, cx| ws.navigate(Route::Notes, cx)),
            Action::NewChat => ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx)),
            Action::Glass(on) => ws.update(cx, |ws, cx| {
                ws.settings.appearance.glass = on;
                ws.save_settings(cx);
            }),
            Action::OpenFolder => ws.update(cx, |ws, cx| ws.open_folder(cx)),
            Action::Settings(SettingsPage::Project) => ws.update(cx, |ws, cx| ws.open_project_settings(None, cx)),
            Action::Settings(page) => ws.update(cx, |ws, cx| ws.navigate(Route::Settings(page), cx)),
            Action::ToggleSidebar => ws.update(cx, |ws, cx| {
                ws.sidebar_collapsed = !ws.sidebar_collapsed;
                cx.notify();
            }),
            Action::ToggleTools => self.right_panel.update(cx, |p, cx| p.toggle(cx)),
            Action::OpenTool(tool) => self.right_panel.update(cx, |p, cx| p.open_tool(tool, window, cx)),
            Action::Theme(choice) => {
                ws.update(cx, |ws, cx| {
                    ws.settings.appearance.theme = choice;
                    ws.save_settings(cx);
                });
                crate::set_theme(choice, window, cx);
            }
            Action::HandHolding(level) => ws.update(cx, |ws, cx| {
                let id = match &ws.route {
                    Route::Thread(id) => Some(id.clone()),
                    _ => None,
                };
                if let Err(message) = ws.set_hand_holding(id.as_deref(), level, cx) {
                    cx.emit(WorkspaceEvent::Toast { message, undo: None });
                }
            }),
            Action::Settle(id) => ws.update(cx, |ws, cx| ws.settle(&id, cx)),
            Action::Fork(id) => ws.update(cx, |ws, cx| _ = ws.fork_thread(&id, crate::workspace::ForkAt::End, &crate::workspace::Scope::Main, cx)),
            Action::CheckForUpdates => ws.update(cx, |ws, cx| {
                ws.check_for_updates(true, cx);
                ws.navigate(Route::Settings(SettingsPage::Updates), cx);
            }),
        }
    }

    fn row(&self, ix: usize, e: Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = ix == self.selected;
        let glyph = match e.glyph {
            Glyph::Agent(a) => ui::agent_logo(&a, px(14.), cx),
            Glyph::Project(name, look) => ui::project_badge(&name, &look, cx),
            Glyph::Icon(i) => i.size(px(14.)).text_color(theme.muted_foreground).into_any_element(),
        };
        // Matched words in full colour against a quieter rest, so they read at a glance.
        let label = div()
            .min_w_0()
            .truncate()
            .when(!e.label_ranges.is_empty(), |el| el.text_color(theme.foreground.opacity(0.72)))
            .child(ui::match_text(&e.label, &e.label_ranges, cx));
        let text = match e.snippet {
            Some((snippet, ranges)) => v_flex()
                .flex_1()
                .min_w_0()
                .py(px(6.))
                .gap(px(1.))
                .child(label)
                .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme.muted_foreground).child(ui::match_text(&snippet, &ranges, cx)))
                .into_any_element(),
            None => h_flex().flex_1().min_w_0().child(label).into_any_element(),
        };
        h_flex()
            .id(("palette-row", ix))
            .test_support()
            .relative()
            .flex_shrink_0()
            .mx(px(5.))
            .px(px(10.))
            .min_h(px(32.))
            .gap(px(10.))
            .rounded(px(8.))
            .text_size(px(13.))
            .cursor_pointer()
            .when(selected, |el| {
                el.bg(theme.list_active).child(div().absolute().left_0().top(px(8.)).bottom(px(8.)).w(px(2.)).rounded_full().bg(crate::palette::ember(cx)))
            })
            .child(div().flex_none().w(px(20.)).flex().justify_center().child(glyph))
            .child(text)
            .when_some(e.hint, |el, h| el.child(div().flex_none().max_w(px(200.)).truncate().text_size(px(12.)).text_color(theme.muted_foreground).child(h)))
            .when(e.checked, |el| el.child(Icon::new(IconName::Check).size(px(14.)).text_color(theme.muted_foreground)))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.selected != ix {
                    this.selected = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.confirm(ix, window, cx)))
            .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = cx.theme().clone();
        let entries = self.entries(cx);
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let searching = !self.query.trim().is_empty();
        let pending = self.pending();
        let mut list = v_flex().id("palette-list").max_h(px(400.)).overflow_y_scroll().track_scroll(&self.scroll).pb(px(5.));
        let mut group = None;
        let empty = entries.is_empty();
        for (ix, e) in entries.into_iter().enumerate() {
            if group != Some(e.group) {
                group = Some(e.group);
                list = list.child(div().flex_shrink_0().px(px(15.)).pt(px(10.)).pb(px(4.)).text_size(px(11.5)).font_medium().text_color(theme.muted_foreground.opacity(0.8)).child(e.group.label(searching)));
            }
            list = list.child(self.row(ix, e, cx));
        }
        if empty {
            list = list.child(div().px(px(15.)).py(px(12.)).text_size(px(13.)).text_color(theme.muted_foreground).child(if pending { "Searching…" } else { "No matches" }));
        }
        div()
            .id("palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme.overlay)
            .flex()
            .justify_center()
            .items_start()
            .pt(px(76.))
            .child(
                ui::menu_surface(cx)
                    .id("palette")
                    .test_support()
                    .w(px(WIDTH))
                    .max_w(relative(0.9))
                    .p_0()
                    .overflow_hidden()
                    .key_context("CommandPalette")
                    // The field binds these keys for itself; take them first.
                    .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }))
                    .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }))
                    .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        // Rows on screen may answer the text as it was a keystroke ago.
                        if this.pending() {
                            this.confirm_when_ready = true;
                        } else {
                            this.confirm(this.selected, window, cx);
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        cx.stop_propagation();
                        this.dismiss(window, cx);
                    }))
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.dismiss(window, cx)))
                    .child(
                        h_flex()
                            .h(px(44.))
                            .px(px(8.))
                            .border_b_1()
                            .border_color(theme.foreground.opacity(0.07))
                            .child(Input::new(&self.input).appearance(false).prefix(Icon::new(IconName::Search).size(px(15.)).text_color(theme.muted_foreground))),
                    )
                    .child(list)
                    .child(
                        h_flex()
                            .px(px(15.))
                            .h(px(30.))
                            .gap(px(14.))
                            .border_t_1()
                            .border_color(theme.foreground.opacity(0.07))
                            .text_size(px(11.5))
                            .text_color(theme.muted_foreground.opacity(0.8))
                            .child("↑↓ to move")
                            .child("↩ to open")
                            .child("esc to close"),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS as CAP, Group, MESSAGE_HITS, TITLE_HITS, commands_named, path_tail, rank, score, scroll_target, thread_hits};
    use trek_core::store::SearchHit;

    fn ranked(query: &str, labels: &[(&str, &str)]) -> Vec<String> {
        rank(query, labels.iter().map(|(l, k)| (l.to_string(), l.to_string(), k.to_string())).collect())
    }

    const COMMANDS: &[(&str, &str)] = &[
        ("New thread", "create start chat"),
        ("Open folder…", "project add"),
        ("Toggle tools panel", "right panel"),
        ("Open Terminal", "tools panel"),
        ("Open Git", "tools panel"),
        ("Theme: Night", "appearance dark light"),
        ("Settings: General", ""),
        ("Settings: Appearance", "Theme, text size, background art and motion."),
        ("Settings: Agents & Subscriptions", ""),
        ("Check for updates", "update version"),
    ];

    #[test]
    fn prefixes_beat_word_starts_beat_substrings_beat_keywords() {
        // Equally good matches keep the order they were offered in.
        assert_eq!(ranked("open", COMMANDS), ["Open folder…", "Open Terminal", "Open Git"]);
        // "theme" starts a label, then shows up only in keywords.
        assert_eq!(ranked("theme", COMMANDS), ["Theme: Night", "Settings: Appearance"]);
        assert_eq!(ranked("appear", COMMANDS), ["Settings: Appearance", "Theme: Night"]);
        // Every word must start some word, in any order.
        assert_eq!(ranked("term op", COMMANDS), ["Open Terminal"]);
        assert_eq!(ranked("pdate", COMMANDS), ["Check for updates"]);
    }

    #[test]
    fn letters_in_order_are_a_last_resort() {
        assert_eq!(ranked("chkupd", COMMANDS), ["Check for updates"]);
        assert!(ranked("zzz", COMMANDS).is_empty());
        // Short queries don't reach into keywords or scattered letters.
        assert!(score("sg", "Settings: General", "").is_none());
        assert!(score("pa", "Settings: General", "panel").is_none());
        assert!(score("pan", "Settings: General", "panel").is_some());
    }

    #[test]
    fn an_empty_query_keeps_everything_in_order() {
        assert_eq!(ranked("", COMMANDS).len(), COMMANDS.len());
        assert_eq!(ranked("  ", COMMANDS)[0], "New thread");
    }

    #[test]
    fn whole_labels_and_words_break_ties_then_the_original_order() {
        assert_eq!(ranked("git", &[("Gitlab", ""), ("Git settings", ""), ("Git", "")]), ["Git", "Git settings", "Gitlab"]);
        assert_eq!(ranked("term", &[("Open Terminals", ""), ("Open Term", "")]), ["Open Term", "Open Terminals"]);
        assert_eq!(ranked("x", &[("Xb", ""), ("Xa", "")]), ["Xb", "Xa"]);
    }

    #[test]
    fn settings_lists_every_page_in_page_order() {
        let labels: Vec<String> = crate::settings_view::pages().map(|p| format!("Settings: {}", p.label())).collect();
        let offered: Vec<(&str, &str)> = labels.iter().map(|l| (l.as_str(), "")).chain(COMMANDS.iter().copied()).collect();
        let found = ranked("settings", &offered);
        assert!(labels.len() <= CAP, "{} pages, {CAP} rows", labels.len());
        assert_eq!(found[..labels.len()], labels[..]);
    }

    #[test]
    fn a_commands_name_puts_the_commands_first() {
        let offered: Vec<((), String, String)> = COMMANDS.iter().map(|(l, k)| ((), l.to_string(), k.to_string())).collect();
        assert!(commands_named("theme", &offered));
        assert!(commands_named("term", &offered), "a word of the label");
        assert!(!commands_named("parser", &offered));
        // Only in keywords, or a letter: threads keep the top.
        assert!(!commands_named("dark", &offered));
        assert!(!commands_named("t", &offered));
    }

    #[test]
    fn folder_hints_keep_their_last_folders() {
        assert_eq!(path_tail("~/Code/trek", 30), "~/Code/trek");
        assert_eq!(path_tail("~/Documents/Documents - Toby's MacBook Air/Direct", 30), "…/Direct");
        assert_eq!(path_tail("~/Documents/Projects/Clients/acme/web", 30), "…/Projects/Clients/acme/web");
        assert_eq!(path_tail("~/a/an-uncommonly-long-project-folder-name", 30), "…/an-uncommonly-long-project-folder-name");
    }

    #[test]
    fn keyboard_moves_scroll_rows_and_group_labels_into_view() {
        use Group::*;
        let groups = [Threads, Threads, Projects, Commands, Commands];
        // Children: [Threads label, t0, t1, Projects label, p0, Commands label, c0, c1].
        let targets: Vec<usize> = (0..groups.len()).map(|i| scroll_target(&groups, i).unwrap()).collect();
        assert_eq!(targets, [0, 2, 3, 5, 7]);
        assert_eq!(scroll_target(&groups, 5), None);
        assert_eq!(scroll_target(&[], 0), None);
    }

    #[test]
    fn message_matches_fill_in_after_titles_from_other_threads() {
        let hit = |thread: usize, position: Option<usize>| SearchHit {
            thread_id: format!("t{thread}"),
            title: format!("Thread {thread}"),
            position,
            item_id: None,
            snippet: String::new(),
            ranges: vec![],
        };
        // What Store::search returns: titles first, then the best message per thread.
        let hits: Vec<SearchHit> = (0..TITLE_HITS + 2).map(|t| hit(t, None)).chain((0..TITLE_HITS + MESSAGE_HITS).map(|t| hit(t, Some(t)))).collect();
        let rows: Vec<(String, Option<usize>)> = thread_hits(&hits).into_iter().map(|h| (h.thread_id.clone(), h.position)).collect();
        let titles: Vec<_> = (0..TITLE_HITS).map(|t| (format!("t{t}"), None)).collect();
        // Threads past the title cap still show up by their message; listed ones don't twice.
        let messages: Vec<_> = (TITLE_HITS..TITLE_HITS + MESSAGE_HITS).map(|t| (format!("t{t}"), Some(t))).collect();
        assert_eq!(rows, [titles, messages].concat());
    }
}
