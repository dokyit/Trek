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
const COMMANDS: usize = 12;

/// What choosing an entry does.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    OpenThread(String),
    OpenMessage(String, ItemRef),
    NewThreadIn(PathBuf),
    ProjectSettings(String),
    NewThread,
    OpenFolder,
    Settings(SettingsPage),
    ToggleSidebar,
    ToggleTools,
    OpenTool(PanelTool),
    Theme(ThemeChoice),
    HandHolding(HandHolding),
    Settle(String),
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
    Project(String, Option<String>),
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
/// query's letters appearing in order (a typo-tolerant last resort).
pub fn score(query: &str, label: &str, keywords: &str) -> Option<u32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let words = |s: &str| s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let label_words = words(&label);
    // Shorter labels win ties: "Git" over "Git settings" for "git".
    let tighter = 100u32.saturating_sub(label.chars().count() as u32);
    if label.starts_with(&q) {
        return Some(1000 + tighter);
    }
    if label_words.iter().any(|w| w.starts_with(&q)) {
        return Some(800 + tighter);
    }
    let query_words = words(&q);
    if !query_words.is_empty() && query_words.iter().all(|qw| label_words.iter().any(|w| w.starts_with(qw.as_str()))) {
        return Some(600 + tighter);
    }
    if label.contains(&q) {
        return Some(400 + tighter);
    }
    // Below here a one- or two-letter query would match nearly everything.
    if q.chars().count() < 3 {
        return None;
    }
    let keyword_words = words(&keywords.to_lowercase());
    if !query_words.is_empty() && query_words.iter().all(|qw| keyword_words.iter().chain(&label_words).any(|w| w.starts_with(qw.as_str()))) {
        return Some(300 + tighter);
    }
    let mut chars = label.chars();
    if q.chars().filter(|c| !c.is_whitespace()).all(|c| chars.any(|l| l == c)) {
        return Some(100 + tighter);
    }
    None
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
    scroll: ScrollHandle,
    /// Where focus was before the palette opened; it goes back there on close.
    restore: Option<FocusHandle>,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub fn new(workspace: Entity<Workspace>, right_panel: Entity<RightPanel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads, projects and commands"));
        let subscriptions = vec![cx.subscribe(&input, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let q = state.read(cx).value().to_string();
                this.set_query(q, cx);
            }
        })];
        Self {
            workspace,
            right_panel,
            input,
            open: false,
            query: String::new(),
            selected: 0,
            hits: vec![],
            hits_for: String::new(),
            scroll: ScrollHandle::new(),
            restore: None,
            _search: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open { self.dismiss(window, cx) } else { self.show(window, cx) }
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.restore = window.focused(cx);
        self.selected = 0;
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.set_query(String::new(), cx);
        self.set_overlay(true, cx);
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self._search = None;
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

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        let q = self.query.trim().to_string();
        if trek_core::store::fts_query(&q).is_none() {
            self.hits.clear();
            self.hits_for = q;
            self._search = None;
            cx.notify();
            return;
        }
        let store = self.workspace.read(cx).store.clone();
        self._search = Some(cx.spawn(async move |this, cx| {
            let query = q.clone();
            let hits = cx.background_executor().spawn(async move { store.search(&query, TITLE_HITS.max(MESSAGE_HITS) * 2) }).await;
            let _ = this.update(cx, |this, cx| {
                match hits {
                    Ok(hits) => this.hits = hits,
                    Err(e) => {
                        tracing::warn!("search: {e}");
                        this.hits.clear();
                    }
                }
                this.hits_for = q;
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

        // Threads: titles that match, then messages (one per thread, threads not already listed).
        if searching {
            let (titles, messages): (Vec<&SearchHit>, Vec<&SearchHit>) = self.hits.iter().partition(|h| h.position.is_none());
            let mut listed = std::collections::HashSet::new();
            for h in titles.into_iter().take(TITLE_HITS) {
                let Some(t) = ws.thread(&h.thread_id) else { continue };
                listed.insert(h.thread_id.clone());
                let mut e = Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), h.snippet.clone(), Action::OpenThread(t.id.clone()));
                e.label_ranges = h.ranges.clone();
                if let Some(p) = project_name(&t.project_id) {
                    e = e.hint(p);
                }
                out.push(e);
            }
            for h in messages.into_iter().filter(|h| !listed.contains(&h.thread_id)).take(MESSAGE_HITS) {
                let (Some(t), Some(at)) = (ws.thread(&h.thread_id), ItemRef::of_hit(h)) else { continue };
                let mut e = Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), one_line(&t.title), Action::OpenMessage(t.id.clone(), at));
                let (snippet, ranges) = ui::lead_to_match(&h.snippet, &h.ranges, 32);
                e.snippet = Some((snippet.into(), ranges));
                if let Some(p) = project_name(&t.project_id) {
                    e = e.hint(p);
                }
                out.push(e);
            }
        } else {
            let mut recent: Vec<_> = ws.threads.iter().filter(|t| t.side_of.is_none() && t.archived_at.is_none()).collect();
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
            let glyph = Glyph::Project(p.name.clone(), ws.project_icon(&p.path));
            let folder = trek_core::paths::tildify(&p.path);
            let keywords = format!("{} {}", p.remote.clone().unwrap_or_default(), folder);
            let new = Entry::new(Group::Projects, glyph.clone(), format!("New thread in {}", p.name), Action::NewThreadIn(p.path.clone())).hint(folder.clone());
            projects.push((new, p.name.clone(), keywords.clone()));
            if searching {
                let settings = Entry::new(Group::Projects, glyph, format!("{} settings", p.name), Action::ProjectSettings(p.id.clone())).hint(folder);
                projects.push((settings, format!("{} settings", p.name), keywords));
            }
        }
        out.extend(rank(q, projects).into_iter().take(if searching { PROJECTS } else { 4 }));

        let commands = rank(q, self.commands(cx));
        let n = commands.len();
        out.extend(commands.into_iter().take(if searching { COMMANDS } else { n }));
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
        let thread = ws.current_thread().cloned();
        if let Some(t) = thread.as_ref().filter(|t| t.settled_at.is_none()) {
            add(Entry::new(c, icon(Icon::new(IconName::Check)), "Settle thread", Action::Settle(t.id.clone())).hint("⌘E"), "done finish inbox archive");
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
        let entries = self.entries(cx);
        let n = entries.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
        // The list's children include the group labels: find the selected row among them.
        let mut child = 0;
        for (i, e) in entries.iter().enumerate() {
            if i == 0 || entries[i - 1].group != e.group {
                child += 1;
            }
            if i == self.selected {
                // Bring the group label along when moving onto a group's first row.
                let first_of_group = i == 0 || entries[i - 1].group != e.group;
                self.scroll.scroll_to_item(if first_of_group { child - 1 } else { child });
                break;
            }
            child += 1;
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
            Glyph::Project(name, icon) => ui::project_badge(&name, icon.as_deref(), cx),
            Glyph::Icon(i) => i.size(px(14.)).text_color(theme.muted_foreground).into_any_element(),
        };
        let label = div().min_w_0().truncate().child(ui::match_text(&e.label, &e.label_ranges, cx));
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
        let pending = searching && self.hits_for != self.query.trim();
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
                        let ix = this.selected;
                        this.confirm(ix, window, cx);
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
    use super::{rank, score};

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
        assert_eq!(ranked("open", COMMANDS), ["Open Git", "Open folder…", "Open Terminal"]);
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
    fn ties_go_to_the_shorter_label_then_the_original_order() {
        assert_eq!(ranked("git", &[("Git settings", ""), ("Git", ""), ("Gitlab", "")]), ["Git", "Gitlab", "Git settings"]);
        assert_eq!(ranked("x", &[("Xa", ""), ("Xb", "")]), ["Xa", "Xb"]);
    }
}
