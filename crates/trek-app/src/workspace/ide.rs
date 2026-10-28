//! The editor (Trek IDE) side of the workspace: which layout the main window shows, the AI side
//! bar's chat tabs, and what the language servers say about open files. The editor keeps chats
//! of its own about the code open there (`Thread::editor_chat`): its tabs and history list only
//! those, and Agents mode's lists (sidebar, inbox, search, phones) never do; the harness stays
//! on whatever it showed. The chat tabs are kept per IDE folder in the store, so a folder opens
//! again on the chats it was left with.

use super::{Prefs, Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::path::{Path, PathBuf};
use trek_agents::{AgentEvent, Command, Decision};
use trek_core::settings::FollowUp;
use trek_core::store::now_ms;

/// Which of the main window's two layouts is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The harness: inbox, thread tabs, transcript and composer.
    #[default]
    Agents,
    /// The workbench: files, editors, the bottom panel and the AI side bar.
    Editor,
}

/// What the AI side bar's chat does with a message (its mode pill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatMode {
    /// The agent works as usual.
    #[default]
    Agent,
    /// It plans first and waits for a go-ahead (plan mode, as the harness's Plan pill).
    Plan,
    /// It answers without changing anything: read-only where the agent can be held to that
    /// (Claude Code without its editing tools, its shell sandboxed; ACP agents in plan mode and
    /// refused writes; Codex in its read-only Supervised sandbox), its requests to change
    /// things declined, and every message saying so (`ide::ai::context::with_ask`) for the rest.
    Ask,
}

impl ChatMode {
    pub const ALL: [ChatMode; 3] = [ChatMode::Agent, ChatMode::Plan, ChatMode::Ask];

    pub fn label(self) -> &'static str {
        match self {
            ChatMode::Agent => "Agent",
            ChatMode::Plan => "Plan",
            ChatMode::Ask => "Ask",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ChatMode::Agent => "Reads, edits and runs commands to get it done.",
            ChatMode::Plan => "Plans first, and waits for your go-ahead before changing anything.",
            ChatMode::Ask => "Answers questions about the code. Changes nothing.",
        }
    }

    /// ⇧Tab's next mode.
    pub fn next(self) -> ChatMode {
        let all = ChatMode::ALL;
        all[(all.iter().position(|m| *m == self).unwrap_or(0) + 1) % all.len()]
    }
}

/// What the store keeps of a folder's chat tabs: thread ids (`None`: a new chat) and the one in
/// front.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedChats {
    tabs: Vec<Option<String>>,
    active: usize,
    /// The ⌘K chat of each file: (path, thread).
    #[serde(default)]
    inline: Vec<(String, String)>,
}

/// One chat tab in the editor's AI side bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdeTab {
    /// A new chat in the IDE folder: its first message starts an editor chat.
    Draft,
    Thread(String),
}

/// The AI side bar's chat tabs. Closing a tab only takes it off the bar: the chat lives on in
/// the history.
#[derive(Debug, Clone)]
pub struct IdeChat {
    pub tabs: Vec<IdeTab>,
    pub active: usize,
    /// What a new chat starts with (agent, model, effort, access), kept apart from the harness
    /// draft's so picking one doesn't change the other.
    pub draft_prefs: Prefs,
    /// A new chat starts in Ask mode (its plan mode is `draft_prefs.plan`).
    pub draft_ask: bool,
    /// Where ⌘K's edits of each file go: a chat of their own per file ("Inline edits ·
    /// main.rs"), made on the first, so they never land in (or retitle) another conversation.
    pub inline: std::collections::HashMap<PathBuf, String>,
}

/// The title of `path`'s ⌘K chat.
pub fn inline_title(path: &Path) -> String {
    format!(
        "Inline edits · {}",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    )
}

impl IdeChat {
    pub fn new(draft_prefs: Prefs) -> Self {
        Self {
            tabs: vec![IdeTab::Draft],
            active: 0,
            draft_prefs,
            draft_ask: false,
            inline: Default::default(),
        }
    }

    pub fn active_tab(&self) -> Option<&IdeTab> {
        self.tabs.get(self.active)
    }

    /// The thread the active tab shows; `None` on a new chat.
    pub fn active_thread(&self) -> Option<&str> {
        match self.tabs.get(self.active) {
            Some(IdeTab::Thread(id)) => Some(id),
            _ => None,
        }
    }

    pub fn is_draft(&self) -> bool {
        matches!(self.active_tab(), Some(IdeTab::Draft))
    }

    /// Make `id` the active tab, adding it after the active one when it has none yet.
    pub(super) fn show(&mut self, id: &str) {
        let tab = IdeTab::Thread(id.to_string());
        match self.tabs.iter().position(|t| *t == tab) {
            Some(ix) => self.active = ix,
            None => {
                let at = (self.active + 1).min(self.tabs.len());
                self.tabs.insert(at, tab);
                self.active = at;
            }
        }
    }

    /// Take `id`'s tab off the bar (archived, deleted). The bar always keeps a tab.
    pub(super) fn forget(&mut self, id: &str) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|t| matches!(t, IdeTab::Thread(t) if t == id))
        {
            self.close(ix);
        }
    }

    /// Close tab `ix`; when it was active, its neighbour takes over. The last tab closing leaves
    /// a new chat.
    pub(super) fn close(&mut self, ix: usize) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            self.tabs.push(IdeTab::Draft);
        }
        if self.active > ix || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }
}

impl Workspace {
    /// The main window shows the editor.
    pub fn ide(&self) -> bool {
        self.mode == Mode::Editor
    }

    /// The scope the main window's AI input is: the harness composer, or the AI side bar's.
    pub fn focused_scope(&self) -> Scope {
        match self.mode {
            Mode::Agents => Scope::Main,
            Mode::Editor => Scope::Ide,
        }
    }

    /// The thread the main window is on: the harness's route, or the AI side bar's active chat.
    /// Window-wide actions (stop, settle, open in a new window) act on it.
    pub fn focused_thread(&self) -> Option<&str> {
        let scope = self.focused_scope();
        match scope {
            Scope::Ide => self.ide_chat.active_thread(),
            _ => match &self.route {
                Route::Thread(id) => Some(id),
                _ => None,
            },
        }
    }

    /// Switch the main window between the harness and the editor. Into the editor, the folder of
    /// the thread on screen becomes the IDE folder (the thread stays in Agents: the side bar keeps
    /// chats of its own). The harness's route is left as it was.
    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode == mode || self.route == Route::Onboarding {
            return;
        }
        match mode {
            Mode::Editor => {
                let mut adopt = None;
                match self.route.clone() {
                    Route::Thread(id) => {
                        if let Some(cwd) = self.thread(&id).and_then(|t| t.cwd.clone()) {
                            self.move_ide_root(Some(cwd));
                        }
                    }
                    Route::Draft { project } => {
                        if self.ide_root.is_none() {
                            self.move_ide_root(project);
                        }
                    }
                    // A lone file in the harness becomes a tab, and the harness steps back to the
                    // chat it came from.
                    Route::Editor { path } => {
                        if self.ide_root.is_none() {
                            self.move_ide_root(path.parent().map(trek_core::store::project_root));
                        }
                        adopt = Some(path);
                    }
                    _ => {
                        if self.ide_root.is_none() {
                            self.move_ide_root(self.current_cwd());
                        }
                    }
                }
                if !matches!(self.route, Route::Editor { .. }) {
                    self.ide_prev_route = Some(self.route.clone());
                }
                self.mode = Mode::Editor;
                if let Some(path) = adopt {
                    let back = self.ide_prev_route.clone().unwrap_or(Route::Draft {
                        project: self.ide_root.clone(),
                    });
                    self.navigate(back, cx);
                    cx.emit(WorkspaceEvent::OpenEditor {
                        path,
                        line: None,
                        preview: false,
                    });
                }
                if let Some(id) = self.ide_chat.active_thread().map(str::to_string) {
                    self.ensure_loaded(&id, cx);
                }
                if let Some(root) = self.ide_root.clone() {
                    self.refresh_git_at(root, cx);
                }
                cx.emit(WorkspaceEvent::FocusAiInput);
            }
            Mode::Agents => {
                self.mode = Mode::Agents;
                cx.emit(WorkspaceEvent::FocusComposer);
            }
        }
        cx.notify();
    }

    /// The chat tabs of `scope`: the editor's AI side bar, or the side chat.
    pub fn chats(&self, scope: &Scope) -> &IdeChat {
        match scope {
            Scope::Side => &self.side_chat,
            _ => &self.ide_chat,
        }
    }

    fn chats_mut(&mut self, scope: &Scope) -> &mut IdeChat {
        match scope {
            Scope::Side => &mut self.side_chat,
            _ => &mut self.ide_chat,
        }
    }

    /// The editor's tabs are kept with its folder; the side chat's last this run.
    fn tabs_moved(&mut self, scope: &Scope) {
        if *scope == Scope::Ide {
            self.chats_moved();
        }
    }

    /// Open editor chat `id` in the AI side bar as its active chat tab.
    pub fn ide_open_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        self.open_chat_in(&Scope::Ide, id, cx)
    }

    /// Open `id` as `scope`'s active chat tab. The AI side bar opens editor chats only.
    pub fn open_chat_in(&mut self, scope: &Scope, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else {
            return;
        };
        if *scope == Scope::Ide && !t.editor_chat {
            return;
        }
        self.chats_mut(scope).show(id);
        self.tabs_moved(scope);
        self.thread_came_on_screen(id, cx);
        cx.notify();
    }

    /// Open `id` where it lives: an editor chat in the editor's AI side bar, any other thread in
    /// Agents (the window switches to it).
    pub fn open_thread_here(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.thread(id).is_some_and(|t| t.editor_chat) {
            return self.open_editor_chat(id, cx);
        }
        if self.ide() {
            self.set_mode(Mode::Agents, cx);
        }
        self.navigate(Route::Thread(id.to_string()), cx)
    }

    /// Editor chat `id` comes to the front: the editor, on the chat's folder when it's outside
    /// the one open, with the chat as the AI side bar's active tab.
    pub(super) fn open_editor_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()) else {
            return;
        };
        if !self.ide() {
            self.set_mode(Mode::Editor, cx);
        }
        if self.ide_root.as_deref().is_none_or(|r| !cwd.starts_with(r)) {
            self.set_ide_root(cwd, cx);
        }
        self.ide_open_thread(id, cx);
        cx.emit(WorkspaceEvent::FocusAiInput);
    }

    /// Switch the AI side bar to tab `ix`.
    pub fn ide_select_chat(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.select_chat_in(&Scope::Ide, ix, cx)
    }

    /// Switch `scope` to its tab `ix`.
    pub fn select_chat_in(&mut self, scope: &Scope, ix: usize, cx: &mut Context<Self>) {
        let chats = self.chats_mut(scope);
        if ix >= chats.tabs.len() || ix == chats.active {
            return;
        }
        chats.active = ix;
        self.tabs_moved(scope);
        if let Some(id) = self.chats(scope).active_thread().map(str::to_string) {
            self.thread_came_on_screen(&id, cx);
        }
        cx.emit(WorkspaceEvent::FocusAiInput);
        cx.notify();
    }

    /// Close the AI side bar's tab `ix`. Its chat stays, in the history.
    pub fn ide_close_chat(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.close_chat_in(&Scope::Ide, ix, cx)
    }

    /// Close `scope`'s tab `ix`; its thread stays.
    pub fn close_chat_in(&mut self, scope: &Scope, ix: usize, cx: &mut Context<Self>) {
        self.chats_mut(scope).close(ix);
        self.tabs_moved(scope);
        if let Some(id) = self.chats(scope).active_thread().map(str::to_string) {
            self.thread_came_on_screen(&id, cx);
        }
        cx.notify();
    }

    /// Close every chat tab but `keep`.
    pub fn ide_close_other_chats(&mut self, keep: usize, cx: &mut Context<Self>) {
        self.close_other_chats_in(&Scope::Ide, keep, cx)
    }

    /// Close every tab of `scope` but `keep`.
    pub fn close_other_chats_in(&mut self, scope: &Scope, keep: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.chats(scope).tabs.get(keep).cloned() else {
            return;
        };
        let chats = self.chats_mut(scope);
        chats.tabs = vec![tab];
        chats.active = 0;
        self.tabs_moved(scope);
        if let Some(id) = self.chats(scope).active_thread().map(str::to_string) {
            self.thread_came_on_screen(&id, cx);
        }
        cx.notify();
    }

    /// ⌘N in the editor, the side bar's +: a new chat in the IDE folder. There's one new chat at
    /// most: + goes back to it.
    pub fn ide_new_chat(&mut self, cx: &mut Context<Self>) {
        self.new_chat_in(&Scope::Ide, cx)
    }

    /// A new chat in `scope` (one at most: + goes back to it).
    pub fn new_chat_in(&mut self, scope: &Scope, cx: &mut Context<Self>) {
        let chats = self.chats_mut(scope);
        match chats.tabs.iter().position(|t| *t == IdeTab::Draft) {
            Some(ix) => chats.active = ix,
            None => {
                chats.tabs.push(IdeTab::Draft);
                chats.active = chats.tabs.len() - 1;
                // Plan and fast are a thread's choice; a new chat starts without them.
                chats.draft_prefs.plan = false;
                chats.draft_prefs.fast = false;
                chats.draft_ask = false;
            }
        }
        self.tabs_moved(scope);
        cx.emit(WorkspaceEvent::FocusAiInput);
        cx.notify();
    }

    /// The folder a new side chat starts in: the main window's thread's project (and folder: a
    /// worktree, maybe), or its draft's.
    pub fn side_chat_folder(&self) -> Option<PathBuf> {
        self.current_cwd()
    }

    /// The side chats there have been beside the current project, newest first: its history.
    pub fn side_threads(&self) -> Vec<&trek_core::store::Thread> {
        let project = self.current_thread().and_then(|t| t.project_id.clone());
        let mut list: Vec<_> = self
            .threads
            .iter()
            .filter(|t| t.archived_at.is_none() && t.side_of.is_some())
            .filter(|t| project.is_none() || t.project_id == project)
            .collect();
        list.sort_by_key(|t| -t.updated_at);
        list
    }

    /// The side chat's new chat sends its first message: a side chat starts beside the main
    /// window's thread (`create_side_chat`), with what was picked for it, and takes the tab.
    pub(super) fn send_side_draft(
        &mut self,
        text: String,
        images: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        let prefs = self.side_chat.draft_prefs.clone();
        let Some(id) = self.create_side_chat_with(prefs, cx) else {
            return;
        };
        if self.side_chat.draft_ask {
            self.live.entry(id.clone()).or_default().ask = true;
        }
        // Named for its first message till its title comes (`maybe_auto_title`), as a thread is.
        let title = trek_core::import_title(&text);
        self.mutate_thread(&id, cx, |t| t.title = title);
        let ix = self.side_chat.active;
        match self.side_chat.tabs.get_mut(ix) {
            Some(tab @ IdeTab::Draft) => *tab = IdeTab::Thread(id.clone()),
            _ => self.side_chat.show(&id),
        }
        self.send_to(&id, text, images, cx);
        cx.notify();
    }

    /// The AI side bar's new chat sends its first message: an editor chat starts in the IDE
    /// folder and takes the tab's place. The harness stays where it was (no route change, no tab there).
    pub(super) fn send_ide_draft(
        &mut self,
        text: String,
        images: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        if let Some(reply) = self.run_builtin_command(None, &text, cx) {
            if !reply.is_empty() {
                cx.emit(WorkspaceEvent::Toast {
                    message: reply.replace("**", "").replace('`', ""),
                    undo: None,
                });
            }
            return;
        }
        let prefs = self.ide_chat.draft_prefs.clone();
        let root = self.ide_root.clone();
        let editor_chat = |t: &mut trek_core::store::Thread| t.editor_chat = true;
        let Some(id) = self.start_thread_with(root, prefs, &text, &images, editor_chat, cx) else {
            return;
        };
        if self.ide_chat.draft_ask {
            let live = self.live.entry(id.clone()).or_default();
            live.ask = true;
            // A session warmed up meanwhile isn't read-only: the message starts one that is.
            if let Some(tx) = live.commands.take() {
                let _ = tx.try_send(Command::Shutdown);
                live._events = None;
            }
        }
        let ix = self.ide_chat.active;
        match self.ide_chat.tabs.get_mut(ix) {
            Some(tab @ IdeTab::Draft) => *tab = IdeTab::Thread(id.clone()),
            _ => self.ide_chat.show(&id),
        }
        self.chats_moved();
        self.send_to(&id, text, images, cx);
        cx.notify();
    }

    /// Send from the AI side bar: `follow` says what a message sent while a turn runs does
    /// (Return queues it, ⌘Return steers the turn with it now).
    pub fn send_ide(
        &mut self,
        text: String,
        images: Vec<PathBuf>,
        follow: FollowUp,
        cx: &mut Context<Self>,
    ) {
        self.send_chat_in(&Scope::Ide, text, images, follow, cx)
    }

    /// Send from `scope`'s chat panel: `follow` says what a message sent while a turn runs does
    /// (Return queues it, ⌘Return steers the turn with it now).
    pub fn send_chat_in(
        &mut self,
        scope: &Scope,
        text: String,
        images: Vec<PathBuf>,
        follow: FollowUp,
        cx: &mut Context<Self>,
    ) {
        match self.chats(scope).active_thread().map(str::to_string) {
            Some(id) => self.send_as(&id, text, images, Some(follow), cx),
            None if *scope == Scope::Side => self.send_side_draft(text, images, cx),
            None => self.send_ide_draft(text, images, cx),
        }
    }

    /// The chat ⌘K's edits of `path` go to, while it's still there (an editor chat: one made
    /// before there were editor chats is an Agents thread now, and ⌘K starts a new one).
    pub fn inline_chat(&self, path: &Path) -> Option<&str> {
        let id = self.ide_chat.inline.get(path)?;
        self.threads
            .iter()
            .any(|t| t.id == *id && t.archived_at.is_none() && t.editor_chat)
            .then_some(id.as_str())
    }

    /// Where a ⌘K edit of `path` goes, as its card says it: the file's chat (its title), and
    /// the agent and model it works with.
    pub fn inline_target(&self, path: &Path) -> (String, Prefs) {
        match self.inline_chat(path).and_then(|id| self.thread(id)) {
            Some(t) => (
                t.title.clone(),
                Prefs {
                    agent: t.agent.clone(),
                    model: t.model.clone(),
                    effort: t.effort,
                    hand_holding: t.hand_holding,
                    plan: false,
                    fast: self.live.get(&t.id).is_some_and(|l| l.fast),
                    worktree: false,
                },
            ),
            None => (
                inline_title(path),
                Prefs {
                    plan: false,
                    fast: false,
                    ..self.ide_chat.draft_prefs.clone()
                },
            ),
        }
    }

    /// ⌘K in the editor: ask `path`'s own chat ("Inline edits · main.rs", made on the first
    /// edit) to edit `lines` (1-based, inclusive) of it, as `instruction` says. `selected` is
    /// those lines as they are now; they go along quoted. The chat comes to the front of the AI
    /// side bar, the agent edits the file, and its change comes back as a pending hunk to keep or
    /// undo. Never the chat that happened to be open: an unrelated conversation doesn't get the
    /// request (or a new title from it). Not when that chat was put in Ask mode, which changes
    /// nothing.
    pub fn inline_edit(
        &mut self,
        path: &Path,
        lines: (u32, u32),
        selected: String,
        instruction: &str,
        cx: &mut Context<Self>,
    ) {
        let existing = self.inline_chat(path).map(str::to_string);
        if existing
            .as_ref()
            .is_some_and(|id| self.live.get(id).is_some_and(|l| l.ask))
        {
            cx.emit(WorkspaceEvent::Toast { message: "This file's inline edit chat is in Ask mode, which changes nothing. Switch it to Agent to edit.".into(), undo: None });
            return;
        }
        let root = self.ide_root.clone();
        let rel = root
            .as_deref()
            .and_then(|r| path.strip_prefix(r).ok())
            .map_or_else(|| path.display().to_string(), |p| p.display().to_string());
        let chip = crate::ide::ai::context::ContextChip::Selection {
            path: path.to_path_buf(),
            lines,
            text: selected,
        };
        let text =
            crate::ide::ai::context::with_context(instruction.trim(), &[chip], root.as_deref());
        let text = trek_core::inline_edit::with_block(&text, &rel, lines);
        let id = match existing {
            Some(id) => id,
            None => {
                let (title, prefs) = self.inline_target(path);
                let editor_chat = |t: &mut trek_core::store::Thread| t.editor_chat = true;
                let Some(id) = self.start_thread_with(root, prefs, &text, &[], editor_chat, cx)
                else {
                    return;
                };
                // Its own name from the start: the first-answer title never replaces it.
                self.rename(&id, title, cx);
                self.ide_chat.inline.insert(path.to_path_buf(), id.clone());
                id
            }
        };
        self.ide_chat.show(&id);
        self.chats_moved();
        self.thread_came_on_screen(&id, cx);
        self.send_as(&id, text, vec![], Some(FollowUp::Queue), cx);
        cx.notify();
    }

    /// The mode `scope`'s chat is in.
    pub fn chat_mode_in(&self, scope: &Scope) -> ChatMode {
        let ask = match self.thread_id_in(scope) {
            Some(id) => self.live.get(id).is_some_and(|l| l.ask),
            None => matches!(scope, Scope::Ide | Scope::Side) && self.chats(scope).draft_ask,
        };
        if ask {
            ChatMode::Ask
        } else if self.prefs_in(scope).plan {
            ChatMode::Plan
        } else {
            ChatMode::Agent
        }
    }

    /// Put `scope`'s chat in `mode`. Plan is the thread's plan mode; Ask restarts an idle session
    /// read-only (a busy one once its turn is over), as read-only is a setting agents take at launch.
    pub fn set_chat_mode_in(&mut self, scope: &Scope, mode: ChatMode, cx: &mut Context<Self>) {
        let mut prefs = self.prefs_in(scope);
        if prefs.plan != (mode == ChatMode::Plan) {
            prefs.plan = mode == ChatMode::Plan;
            self.set_prefs_in(scope, prefs, cx);
        }
        let ask = mode == ChatMode::Ask;
        match self.thread_id_in(scope).map(str::to_string) {
            Some(id) => {
                let live = self.live.entry(id).or_default();
                if live.ask != ask {
                    live.ask = ask;
                    if live.commands.is_some() {
                        if live.free_to_relaunch() {
                            if let Some(tx) = live.commands.take() {
                                let _ = tx.try_send(Command::Shutdown);
                            }
                            live._events = None;
                            live.relaunch = false;
                        } else {
                            live.relaunch = true;
                        }
                    }
                }
            }
            None if matches!(scope, Scope::Ide | Scope::Side) => {
                self.chats_mut(scope).draft_ask = ask
            }
            None => {}
        }
        cx.notify();
    }

    /// A chat in Ask mode doesn't change things: what its agent asks to do is declined before
    /// anyone sees it. Its questions still come through.
    pub(super) fn screen_ask(&mut self, id: &str, events: Vec<AgentEvent>) -> Vec<AgentEvent> {
        let Some(tx) = self
            .live
            .get(id)
            .filter(|l| l.ask)
            .map(|l| l.commands.clone())
        else {
            return events;
        };
        events
            .into_iter()
            .filter_map(|ev| match ev {
                AgentEvent::PermissionRequest {
                    request_id, prompt, ..
                } if !matches!(prompt, Some(trek_agents::Prompt::Questions(_))) => {
                    if let Some(tx) = &tx {
                        let _ = tx.try_send(Command::Respond {
                            request_id,
                            decision: Decision::Deny,
                        });
                    }
                    None
                }
                other => Some(other),
            })
            .collect()
    }

    /// `id` is on screen now (a chat tab came to the front): seen, loaded, its folder's git read,
    /// and follow-ups a stopped turn left go back to its composer.
    fn thread_came_on_screen(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.thread(id).is_some_and(|t| t.is_unseen()) {
            self.mutate_thread(id, cx, |t| t.last_seen_at = now_ms().max(t.updated_at));
        }
        self.ensure_loaded(id, cx);
        if let Some(cwd) = self.thread(id).and_then(|t| t.cwd.clone()) {
            self.refresh_git_at(cwd, cx);
        }
        self.hand_back_queued(id, cx);
    }

    /// The IDE folder becomes `root`: the chat tabs kept for it come back (chats gone since, and
    /// threads kept from before there were editor chats, left out), or a new chat when none were.
    pub(super) fn move_ide_root(&mut self, root: Option<PathBuf>) {
        if self.ide_root == root {
            return;
        }
        self.ide_root = root;
        let saved = self.ide_root.as_ref().and_then(|r| {
            self.store
                .ide_chats(&r.display().to_string())
                .ok()
                .flatten()
        });
        self.ide_chats_saved = saved.clone();
        let saved = saved.and_then(|d| serde_json::from_str::<SavedChats>(&d).ok());
        let Some(saved) = saved else {
            self.ide_chat.tabs = vec![IdeTab::Draft];
            self.ide_chat.active = 0;
            self.ide_chat.inline.clear();
            return;
        };
        self.ide_chat.inline = saved
            .inline
            .iter()
            .map(|(p, id)| (PathBuf::from(p), id.clone()))
            .collect();
        let live = |id: &str| {
            self.threads
                .iter()
                .any(|t| t.id == id && t.archived_at.is_none() && t.editor_chat)
        };
        let mut tabs = vec![];
        let mut active = 0;
        for (ix, tab) in saved.tabs.into_iter().enumerate() {
            let tab = match tab {
                Some(id) if live(&id) => IdeTab::Thread(id),
                Some(_) => continue,
                None if tabs.contains(&IdeTab::Draft) => continue,
                None => IdeTab::Draft,
            };
            if ix == saved.active {
                active = tabs.len();
            }
            tabs.push(tab);
        }
        if tabs.is_empty() {
            tabs.push(IdeTab::Draft);
        }
        self.ide_chat.active = active.min(tabs.len() - 1);
        self.ide_chat.tabs = tabs;
    }

    /// The chat tabs moved: the store keeps them for the IDE folder.
    pub(super) fn chats_moved(&mut self) {
        let Some(root) = self.ide_root.as_ref().map(|r| r.display().to_string()) else {
            return;
        };
        let mut inline: Vec<(String, String)> = self
            .ide_chat
            .inline
            .iter()
            .map(|(p, id)| (p.display().to_string(), id.clone()))
            .collect();
        inline.sort();
        let saved = SavedChats {
            tabs: self
                .ide_chat
                .tabs
                .iter()
                .map(|t| match t {
                    IdeTab::Draft => None,
                    IdeTab::Thread(id) => Some(id.clone()),
                })
                .collect(),
            active: self.ide_chat.active,
            inline,
        };
        let Ok(data) = serde_json::to_string(&saved) else {
            return;
        };
        if self.ide_chats_saved.as_deref() == Some(data.as_str()) {
            return;
        }
        match self.store.set_ide_chats(&root, &data) {
            Ok(()) => self.ide_chats_saved = Some(data),
            Err(e) => tracing::warn!("keep the editor's chats: {e:#}"),
        }
    }

    /// The language server's diagnostics for `path` (an open file) changed.
    pub fn set_diagnostics(
        &mut self,
        path: PathBuf,
        diags: Vec<lsp_types::Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        if self.diagnostics.get(&path) == Some(&diags) {
            return;
        }
        if diags.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, diags);
        }
        cx.notify();
    }

    /// A file closed: what its server said about it goes from Problems.
    pub fn clear_diagnostics(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.diagnostics.remove(path).is_some() {
            cx.notify();
        }
    }

    /// Errors and warnings across the open files, for the status bar.
    pub fn diagnostic_counts(&self) -> (usize, usize) {
        use lsp_types::DiagnosticSeverity as S;
        let all = self.diagnostics.values().flatten();
        all.fold((0, 0), |(e, w), d| match d.severity {
            Some(S::ERROR) => (e + 1, w),
            Some(S::WARNING) => (e, w + 1),
            _ => (e, w),
        })
    }

    /// The editor chats of the IDE folder (or a worktree of it), for the AI side bar's history
    /// and the bottom panel's Tasks and Output: newest first, sub-agents left to their parents.
    pub fn ide_threads(&self) -> Vec<&trek_core::store::Thread> {
        let Some(root) = self.ide_root.as_deref() else {
            return vec![];
        };
        let mut list: Vec<_> = self
            .threads
            .iter()
            .filter(|t| t.editor_chat && t.archived_at.is_none() && t.parent_id.is_none())
            // A project's thread (in a worktree elsewhere, say), by its project: never by walking
            // up from its folder, which can only find `root` for a folder under it anyway.
            .filter(|t| {
                t.cwd.as_deref().is_some_and(|c| c.starts_with(root))
                    || t.project_id
                        .as_deref()
                        .and_then(|p| self.project(p))
                        .is_some_and(|p| p.path == root)
            })
            .collect();
        list.sort_by_key(|t| -t.updated_at);
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> Prefs {
        Prefs {
            agent: trek_core::AgentId::ClaudeCode,
            model: None,
            effort: trek_core::Effort::Medium,
            hand_holding: trek_core::HandHolding::Auto,
            plan: false,
            fast: false,
            worktree: false,
        }
    }

    #[test]
    fn chat_tabs_open_close_and_keep_one() {
        let mut chat = IdeChat::new(prefs());
        chat.show("a");
        chat.show("b");
        assert_eq!(
            chat.tabs,
            vec![
                IdeTab::Draft,
                IdeTab::Thread("a".into()),
                IdeTab::Thread("b".into())
            ]
        );
        assert_eq!(chat.active_thread(), Some("b"));
        // Showing one that has a tab goes back to it.
        chat.show("a");
        assert_eq!((chat.tabs.len(), chat.active_thread()), (3, Some("a")));
        // Closing the active tab hands over to its neighbour.
        chat.close(1);
        assert_eq!(chat.active_thread(), Some("b"));
        chat.forget("b");
        assert!(chat.is_draft());
        chat.close(0);
        assert_eq!(chat.tabs, vec![IdeTab::Draft], "the bar always keeps a tab");
    }
}
