//! ⌘K: one field over threads (titles first, then what was said in them), projects and commands.
//! Choosing a message match opens its thread scrolled to that message. With the field empty it's a
//! switcher: the latest threads first, each with its project and age, ⌘1–⌘9 opening them.
//!
//! ⌘P in the editor opens the same palette on the IDE folder's files only (Quick Open); `>`
//! there lists the commands.

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

const WIDTH: f32 = 600.;
/// Rows per group: recent threads (empty field, one per ⌘1–⌘9), title and message matches,
/// projects, commands.
const RECENT: usize = 9;
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
    BasecampRange(trek_core::basecamp::Range),
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
    /// Go to file — the IDE's ⌘P.
    OpenFile(PathBuf),
    /// Agents ⇄ Editor.
    SwitchMode(crate::workspace::Mode),
    /// The editor's layout toggles, as their shortcuts do.
    Dispatch(IdeCommand),
}

/// The editor's commands that are window actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdeCommand {
    PrimaryBar,
    AiBar,
    Panel,
    NewChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Go-to-file matches in IDE mode.
    Files,
    Threads,
    Projects,
    Commands,
}

impl Group {
    fn label(self, searching: bool) -> &'static str {
        match self {
            Group::Threads if searching => "Threads",
            Group::Threads => "Recent threads",
            Group::Files => "Files",
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
pub(crate) struct Entry {
    group: Group,
    glyph: Glyph,
    pub(crate) label: SharedString,
    /// Matched words in the label (thread titles).
    label_ranges: Vec<Range<usize>>,
    /// A second line: the matching excerpt of a message.
    snippet: Option<(SharedString, Vec<Range<usize>>)>,
    /// Quiet text on the right: a shortcut, a project, a folder.
    hint: Option<SharedString>,
    checked: bool,
    /// A thread's project on the right: name, look, and the worktree branch it runs on.
    project: Option<(String, ui::ProjectLook, Option<String>)>,
    /// How long ago a thread last changed, after its title.
    age: Option<SharedString>,
    /// ⌘<n> opens it (the recent threads).
    pub(crate) jump: Option<usize>,
    action: Action,
}

impl Entry {
    fn new(group: Group, glyph: Glyph, label: impl Into<SharedString>, action: Action) -> Self {
        Entry { group, glyph, label: label.into(), label_ranges: vec![], snippet: None, hint: None, checked: false, project: None, age: None, jump: None, action }
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
/// group's first row. Moving up onto a group's first row (or onto the very first) brings its label
/// into view too, so that row reports the label's place; moving down, the row itself is the
/// target (the label sits above it, so the row would stay below the fold).
fn scroll_target(groups: &[Group], selected: usize, upward: bool) -> Option<usize> {
    let mut labels = 0;
    for (i, g) in groups.iter().enumerate() {
        let first_of_group = i == 0 || groups[i - 1] != *g;
        if first_of_group {
            labels += 1;
        }
        if i == selected {
            return Some(if first_of_group && (upward || i == 0) { i + labels - 1 } else { i + labels });
        }
    }
    None
}

/// Whether `query` names one of `commands`: it starts the label or one of its words.
fn commands_named<T>(query: &str, commands: &[(T, String, String)]) -> bool {
    query.chars().count() >= 2 && commands.iter().any(|(_, label, keywords)| score(query, label, keywords).is_some_and(|s| s >= 800))
}

/// How long ago `ms` was, as of `now` (both unix ms), for a switcher row: "now", "3m", "2h",
/// "Yesterday", "4d", then the date ("Sep 12"). Hours hold across midnight for a few hours.
pub(crate) fn age(ms: i64, now: i64) -> String {
    let s = ((now - ms) / 1000).max(0);
    let day = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).date_naive());
    let days = match (day(ms), day(now)) {
        (Some(then), Some(today)) => (today - then).num_days(),
        _ => s / 86_400,
    };
    match s {
        0..=59 => "now".into(),
        60..=3_599 => format!("{}m", s / 60),
        _ if days == 0 || s < 6 * 3_600 => format!("{}h", s / 3_600),
        _ if days == 1 => "Yesterday".into(),
        _ if days < 7 => format!("{days}d"),
        _ => chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string()).unwrap_or_default(),
    }
}

/// The footer's keys and what they do; ⌘1–9 only while the recent threads are listed.
fn footer_keys(jump: bool) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![("↑↓", "Navigate"), ("↵", "Open")];
    if jump {
        keys.push(("⌘1–9", "Jump"));
    }
    keys.push(("Esc", "Close"));
    keys
}

/// A key as the footer and the rows show it: small, in a hairline box.
fn keycap(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(theme.foreground.opacity(0.1))
        .bg(theme.foreground.opacity(0.03))
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(text.into())
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
    /// The IDE's go-to-file index, rebuilt (off the main thread) when the palette opens on a
    /// different root.
    file_index: Vec<String>,
    file_root: Option<PathBuf>,
    _index: Option<Task<()>>,
    /// Quick Open (⌘P in the editor): files only, `>` for commands.
    pub files_only: bool,
    workspace: Entity<Workspace>,
    right_panel: Entity<RightPanel>,
    basecamp: Entity<crate::basecamp::Basecamp>,
    input: Entity<InputState>,
    pub open: bool,
    /// Coming in and going out: dismissed, it stays drawn (rows and all) until it has faded.
    presence: crate::motion::Presence<()>,
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
    pub fn new(workspace: Entity<Workspace>, right_panel: Entity<RightPanel>, basecamp: Entity<crate::basecamp::Basecamp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            basecamp,
            file_index: vec![],
            file_root: None,
            _index: None,
            files_only: false,
            input,
            open: false,
            presence: crate::motion::Presence::new(crate::motion::SURFACE),
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
        if self.open && !self.files_only {
            self.dismiss(window, cx)
        } else {
            self.set_files_only(false, window, cx);
            self.show(window, cx)
        }
    }

    /// ⌘P in the editor: Quick Open on the IDE folder's files (again: closes it).
    pub fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open && self.files_only {
            self.dismiss(window, cx)
        } else {
            self.set_files_only(true, window, cx);
            self.show(window, cx)
        }
    }

    fn set_files_only(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.files_only = on;
        let hint = if on { "Go to file, or > for commands" } else { "Search threads, projects and commands" };
        self.input.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
    }

    /// Show the palette, or leave it as it is when it's already open (⌘K from a thread window).
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.show(window, cx);
        }
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The editor's go-to-file: index the IDE root the first time the palette opens on it, in
        // the background (a big folder takes a while); rows come in when it's done.
        let root = self.workspace.read(cx).ide().then(|| self.workspace.read(cx).ide_root.clone()).flatten();
        if root != self.file_root {
            self.file_root = root.clone();
            self.file_index.clear();
            self._index = root.map(|r| {
                cx.spawn(async move |this, cx| {
                    let files = cx.background_executor().spawn(async move { crate::mentions::index_files(&r) }).await;
                    let _ = this.update(cx, |this, cx| {
                        this.file_index = files;
                        cx.notify();
                    });
                })
            });
        }
        self.open = true;
        let motion = self.workspace.read(cx).motion(cx);
        self.presence.enter((), motion, crate::motion::now(cx));
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
        let motion = self.workspace.read(cx).motion(cx);
        self.presence.exit(motion, crate::motion::now(cx));
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
        // Quick Open lists no threads: nothing to search for.
        if trek_core::store::fts_query(&q).is_none() || self.files_only {
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

    pub(crate) fn entries(&self, cx: &App) -> Vec<Entry> {
        if self.files_only {
            return self.file_entries(cx);
        }
        let ws = self.workspace.read(cx);
        let q = self.query.trim();
        let searching = !q.is_empty();
        let now = trek_core::store::now_ms();
        // A thread's project badge and age.
        let about = |mut e: Entry, t: &trek_core::store::Thread| {
            if let Some(p) = t.project_id.as_ref().and_then(|p| ws.project(p)) {
                let branch = t.worktree.as_ref().map(|w| w.branch.clone());
                e.project = Some((p.name.clone(), ws.project_look(&p.path), branch));
            }
            e.age = Some(age(t.updated_at, now).into());
            e
        };
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

        // In the editor, files lead ⌘K too.
        if searching && ws.ide() {
            out.extend(self.matching_files(q, 12));
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
                e = about(e, t);
                out.push(e);
            }
        } else {
            // Sub-agents show in their parent; a search still finds them.
            let mut recent: Vec<_> = ws.threads.iter().filter(|t| t.side_of.is_none() && t.parent_id.is_none() && t.archived_at.is_none()).collect();
            recent.sort_by_key(|t| -t.updated_at);
            for (n, t) in recent.into_iter().take(RECENT).enumerate() {
                let mut e = about(Entry::new(Group::Threads, Glyph::Agent(t.agent.clone()), one_line(&t.title), Action::OpenThread(t.id.clone())), t);
                e.jump = Some(n + 1);
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

    /// The IDE folder's files matching `q`, best first.
    fn matching_files(&self, q: &str, limit: usize) -> Vec<Entry> {
        let Some(root) = &self.file_root else { return vec![] };
        crate::mentions::match_files(&self.file_index, q, limit)
            .into_iter()
            .filter(|f| !f.ends_with('/'))
            .map(|rel| {
                let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string());
                let mut e = Entry::new(Group::Files, Glyph::Icon(Icon::new(IconName::File)), name, Action::OpenFile(root.join(&rel)));
                if let Some(d) = dir {
                    e = e.hint(path_tail(&d, FOLDER_HINT));
                }
                e
            })
            .collect()
    }

    /// Quick Open's rows: the files matching the text (with none, the ones opened lately, then
    /// the folder's first), or with `>`, the commands.
    fn file_entries(&self, cx: &App) -> Vec<Entry> {
        let q = self.query.trim();
        if let Some(rest) = q.strip_prefix('>') {
            return rank(rest.trim(), self.commands(cx)).into_iter().take(COMMANDS * 2).collect();
        }
        if !q.is_empty() {
            return self.matching_files(q, 40);
        }
        let Some(root) = &self.file_root else { return vec![] };
        let ws = self.workspace.read(cx);
        let recent = ws.settings.ide.recent_files.iter().map(PathBuf::from).filter(|p| p.starts_with(root)).filter_map(|p| p.strip_prefix(root).ok().map(|r| r.display().to_string()));
        let mut seen = std::collections::HashSet::new();
        recent
            .chain(self.file_index.iter().filter(|f| !f.ends_with('/')).cloned())
            .filter(|rel| seen.insert(rel.clone()))
            .take(30)
            .map(|rel| {
                let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string());
                let mut e = Entry::new(Group::Files, Glyph::Icon(Icon::new(IconName::File)), name, Action::OpenFile(root.join(&rel)));
                if let Some(d) = dir {
                    e = e.hint(path_tail(&d, FOLDER_HINT));
                }
                e
            })
            .collect()
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
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::SquarePen)), "New thread", Action::NewThread).hint(if ws.ide() { "" } else { "⌘N" }), "create start chat compose");
        // Agents ⇄ Editor, and the editor's layout.
        if ws.ide() {
            add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::MessageSquare)), "Switch to Agents", Action::SwitchMode(crate::workspace::Mode::Agents)).hint("⌥⌘E"), "harness inbox chat threads mode layout");
            add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::MessageSquarePlus)), "New chat", Action::Dispatch(IdeCommand::NewChat)).hint("⌘N"), "agent ai side bar conversation");
            add(Entry::new(c, icon(Icon::new(IconName::PanelLeft)), "Toggle primary side bar", Action::Dispatch(IdeCommand::PrimaryBar)).hint("⌘B"), "explorer files hide show layout");
            add(Entry::new(c, icon(Icon::new(IconName::PanelRight)), "Toggle AI side bar", Action::Dispatch(IdeCommand::AiBar)).hint("⌥⌘B"), "agent chat hide show layout");
            add(Entry::new(c, icon(Icon::new(IconName::PanelBottom)), "Toggle panel", Action::Dispatch(IdeCommand::Panel)).hint("⌘J"), "terminal problems output bottom hide show layout");
        } else {
            add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::CodeXml)), "Switch to Editor", Action::SwitchMode(crate::workspace::Mode::Editor)).hint("⌥⌘E"), "ide code files workbench mode layout");
        }
        add(Entry::new(c, icon(Icon::new(IconName::FolderOpen)), "Open folder…", Action::OpenFolder).hint("⌘O"), "project add repository");
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::Tent)), "Basecamp", Action::Basecamp).hint("⌘⇧H"), "recap today week all time summary inbox review usage tokens stats");
        // On Basecamp, the span its recap covers.
        if ws.route == Route::Basecamp {
            let current = self.basecamp.read(cx).range();
            for range in [trek_core::basecamp::Range::Today, trek_core::basecamp::Range::Week, trek_core::basecamp::Range::All] {
                let e = Entry::new(c, icon(Icon::new(crate::assets::Lucide::Tent)), format!("Basecamp: {}", range.label()), Action::BasecampRange(range)).checked(range == current);
                add(e, "recap range span today week all time ever history");
            }
        }
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::NotebookPen)), "Notes", Action::Notes).hint("⌘⇧J"), "jot write todo checklist scratch pad memo");
        add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::MessageSquarePlus)), "New thread without a project", Action::NewChat), "chat scratch no project question");
        let glass = ws.settings.appearance.glass;
        add(
            Entry::new(c, icon(Icon::new(crate::assets::Lucide::Sparkles)), if glass { "Turn off liquid glass" } else { "Turn on liquid glass" }, Action::Glass(!glass)),
            "appearance translucent blur transparent vibrancy theme",
        );
        // The thread on screen: the harness's, or the editor's AI side bar chat.
        let thread = ws.focused_thread().and_then(|id| ws.thread(id)).cloned();
        if let Some(t) = thread.as_ref().filter(|t| t.settled_at.is_none()) {
            add(Entry::new(c, icon(Icon::new(IconName::Check)), "Settle thread", Action::Settle(t.id.clone())).hint("⌘E"), "done finish inbox archive");
        }
        if let Some(t) = thread.as_ref() {
            add(Entry::new(c, icon(Icon::new(crate::assets::Lucide::GitFork)), "Fork thread", Action::Fork(t.id.clone())), "branch copy duplicate conversation");
        }
        // Hand-holding applies to the thread on screen, or to the next new thread.
        if ws.ide() || matches!(ws.route, Route::Thread(_) | Route::Draft { .. }) {
            let current = ws.prefs_in(&ws.focused_scope()).hand_holding;
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
        if let Some(child) = scroll_target(&groups, self.selected, delta < 0) {
            self.scroll.scroll_to_item(child);
        }
        cx.notify();
    }

    /// ⌘<n>: open the n-th recent thread, when they're listed. False when there's none to open.
    pub(crate) fn jump(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.entries(cx).iter().position(|e| e.jump == Some(n)) else { return false };
        self.confirm(ix, window, cx);
        true
    }

    fn confirm(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries(cx).into_iter().nth(ix) else { return };
        self.dismiss(window, cx);
        let ws = self.workspace.clone();
        match entry.action {
            Action::OpenThread(id) => ws.update(cx, |ws, cx| ws.open_thread_here(&id, cx)),
            // In the editor a message match opens its thread in the AI side bar.
            Action::OpenMessage(id, _) if ws.read(cx).ide() => ws.update(cx, |ws, cx| ws.ide_open_thread(&id, cx)),
            Action::OpenMessage(id, at) => ws.update(cx, |ws, cx| ws.open_thread_at(&id, at, cx)),
            Action::NewThreadIn(path) => ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(path) }, cx)),
            Action::ProjectSettings(id) => ws.update(cx, |ws, cx| ws.open_project_settings(Some(id), cx)),
            Action::NewThread => ws.update(cx, |ws, cx| ws.new_thread(cx)),
            Action::Basecamp => ws.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx)),
            Action::BasecampRange(range) => self.basecamp.update(cx, |b, cx| b.set_range(range, cx)),
            Action::Notes => ws.update(cx, |ws, cx| ws.navigate(Route::Notes, cx)),
            Action::NewChat => ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx)),
            Action::Glass(on) => ws.update(cx, |ws, cx| {
                ws.settings.appearance.glass = on;
                ws.save_settings(cx);
            }),
            Action::OpenFolder => ws.update(cx, |ws, cx| ws.open_folder(cx)),
            Action::Settings(SettingsPage::Project) => ws.update(cx, |ws, cx| ws.open_project_settings(None, cx)),
            Action::Settings(page) => ws.update(cx, |ws, cx| ws.navigate(Route::Settings(page), cx)),
            // The window's own action: in the editor it's the primary side bar.
            Action::ToggleSidebar => window.dispatch_action(Box::new(crate::ToggleSidebar), cx),
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
                let id = ws.focused_thread().map(str::to_string);
                if let Err(message) = ws.set_hand_holding(id.as_deref(), level, cx) {
                    cx.emit(WorkspaceEvent::Toast { message, undo: None });
                }
            }),
            Action::Settle(id) => ws.update(cx, |ws, cx| ws.settle(&id, cx)),
            Action::Fork(id) => ws.update(cx, |ws, cx| {
                let scope = ws.focused_scope();
                _ = ws.fork_thread(&id, crate::workspace::ForkAt::End, &scope, cx)
            }),
            Action::OpenFile(path) => ws.update(cx, |ws, cx| ws.open_editor(path, None, cx)),
            Action::SwitchMode(mode) => ws.update(cx, |ws, cx| ws.set_mode(mode, cx)),
            Action::Dispatch(command) => window.dispatch_action(
                match command {
                    IdeCommand::PrimaryBar => Box::new(crate::ToggleSidebar),
                    IdeCommand::AiBar => Box::new(crate::ToggleAiBar),
                    IdeCommand::Panel => Box::new(crate::ToggleRightPanel),
                    IdeCommand::NewChat => Box::new(crate::NewThread),
                },
                cx,
            ),
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
        let age_in_text = e.snippet.is_some();
        let text = match e.snippet {
            Some((snippet, ranges)) => v_flex()
                .flex_1()
                .min_w_0()
                .py(px(6.))
                .gap(px(1.))
                .child(label)
                .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme.muted_foreground).child(ui::match_text(&snippet, &ranges, cx)))
                .into_any_element(),
            None => h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.))
                .child(label)
                .when_some(e.age.clone(), |el, age| el.child(div().flex_none().text_size(px(11.5)).text_color(theme.muted_foreground.opacity(0.8)).child(age)))
                .into_any_element(),
        };
        // The project on the right, as the sidebar marks it, and the worktree branch it runs on.
        let project = e.project.map(|(name, look, branch)| {
            h_flex()
                .flex_none()
                .max_w(px(200.))
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(ui::project_badge(&name, &look, cx))
                .child(div().min_w_0().truncate().child(name))
                .when_some(branch, |el, b| el.child(div().min_w_0().truncate().text_color(theme.muted_foreground.opacity(0.7)).child(format!("· {b}"))))
        });

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
            .children(project)
            // A message match has its excerpt under the title: the age sits on the right.
            .when_some(e.age.filter(|_| age_in_text), |el, age| el.child(div().flex_none().text_size(px(11.5)).text_color(theme.muted_foreground.opacity(0.8)).child(age)))
            .when_some(e.jump, |el, n| el.child(keycap(format!("⌘{n}"), cx)))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Dismissed, it stays (inert) while it fades and lifts away.
        let Some(t) = self.presence.sample(crate::motion::now(cx), window) else { return div().into_any_element() };
        let leaving = !self.open;
        let theme = cx.theme().clone();
        let entries = self.entries(cx);
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let searching = !self.query.trim().is_empty();
        let pending = self.pending();
        let mut list = v_flex().id("palette-list").max_h(px(400.)).overflow_y_scroll().track_scroll(&self.scroll).pb(px(5.));
        let mut group = None;
        let empty = entries.is_empty();
        let jump = entries.iter().any(|e| e.jump.is_some());
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
            .when(!leaving, |el| el.occlude())
            .bg(theme.overlay.opacity(t))
            .flex()
            .justify_center()
            .items_start()
            .pt(px(76.))
            .child(
                ui::menu_surface(cx)
                    .id("palette")
                    .test_support()
                    .when(t < 1., |el| el.opacity(t).mt(px(-8. * (1. - t))))
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
                    // ⌘1–⌘9: the recent thread with that number.
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        let m = ev.keystroke.modifiers;
                        if !m.platform || m.control || m.alt || m.shift || m.function {
                            return;
                        }
                        let Some(n) = ev.keystroke.key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) else { return };
                        if this.jump(n, window, cx) {
                            cx.stop_propagation();
                        }
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
                            .id("palette-footer")
                            .test_support()
                            .px(px(12.))
                            .h(px(34.))
                            .gap(px(14.))
                            .justify_end()
                            .border_t_1()
                            .border_color(theme.foreground.opacity(0.07))
                            .text_size(px(11.5))
                            .text_color(theme.muted_foreground.opacity(0.8))
                            .children(footer_keys(jump).into_iter().map(|(key, what)| h_flex().gap(px(6.)).child(keycap(key, cx)).child(what))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS as CAP, Group, MESSAGE_HITS, TITLE_HITS, age, commands_named, footer_keys, path_tail, rank, score, scroll_target, thread_hits};
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
        let up: Vec<usize> = (0..groups.len()).map(|i| scroll_target(&groups, i, true).unwrap()).collect();
        assert_eq!(up, [0, 2, 3, 5, 7]);
        // Going down, a group's first row is the target, not the label above it (the top row
        // still brings the list's first label).
        let down: Vec<usize> = (0..groups.len()).map(|i| scroll_target(&groups, i, false).unwrap()).collect();
        assert_eq!(down, [0, 2, 4, 6, 7]);
        assert_eq!(scroll_target(&groups, 5, false), None);
        assert_eq!(scroll_target(&[], 0, true), None);
    }

    #[test]
    fn ages_read_as_minutes_hours_yesterday_days_then_a_date() {
        use chrono::TimeZone as _;
        // Noon, local time: hours ago are still today.
        let noon = chrono::Local.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap().timestamp_millis();
        let ago = |minutes: i64| age(noon - minutes * 60_000, noon);
        assert_eq!(ago(0), "now");
        assert_eq!(ago(3), "3m");
        assert_eq!(ago(59), "59m");
        assert_eq!(ago(2 * 60), "2h");
        assert_eq!(ago(11 * 60 + 59), "11h");
        // Yesterday evening, then the days before.
        assert_eq!(ago(16 * 60), "Yesterday");
        assert_eq!(ago(3 * 24 * 60), "3d");
        assert_eq!(ago(10 * 24 * 60), "Sep 28");
        // Just after midnight, an hour ago is an hour ago.
        let late = chrono::Local.with_ymd_and_hms(2026, 10, 8, 0, 30, 0).unwrap().timestamp_millis();
        assert_eq!(age(late - 3_600_000, late), "1h");
        assert_eq!(age(late - 10 * 3_600_000, late), "Yesterday");
    }

    #[test]
    fn the_footer_names_the_keys_and_jumping_only_with_recents() {
        assert_eq!(footer_keys(true), [("↑↓", "Navigate"), ("↵", "Open"), ("⌘1–9", "Jump"), ("Esc", "Close")]);
        assert!(!footer_keys(false).iter().any(|(k, _)| *k == "⌘1–9"));
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
