//! The composer (MonoCode-style): a card with context on top, the prompt, then pills for
//! model (Fast · Effort › · Model ›) and access level, plus attach and send.
//! On a new thread it floats over the background hero with Capy-style context chips above it.

use crate::attachments::{self, Attaching, Outbox};
use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{PanelTool, Prefs, Route, Scope, Workspace, WorkspaceEvent};
use crate::TogglePlan;
use gpui_kit::component::input::{Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use crate::mentions::{self, PickIcon, PickItem, PickKind, Trigger};
use std::path::PathBuf;
use std::sync::Arc;
use trek_core::catalog::ModelInfo;
use trek_core::orchestrate::{Consult, Consultant, Style};
use trek_core::{AgentId, Effort, HandHolding, RunState};

#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Effort,
    Model,
}

#[derive(Clone, PartialEq)]
enum Rail {
    Favorites,
    Agent(AgentId),
}

pub struct Composer {
    workspace: Entity<Workspace>,
    /// The main window's composer follows its route; a thread window's is bound to one thread.
    scope: Scope,
    input: Entity<TextareaState>,
    model_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    model_open: bool,
    access_open: bool,
    sub: Option<Sub>,
    rail: Option<Rail>,
    /// Images going out with the next message.
    outbox: Outbox,
    /// The `/`, `@` or `$` token being completed, and the highlighted row.
    trigger: Option<Trigger>,
    picked: usize,
    /// Project files for `@`, indexed once per folder.
    file_index: Option<(PathBuf, Arc<Vec<String>>)>,
    indexing: Option<Task<()>>,
    snapshotting: bool,
    /// Set when Enter picked a row, so the same keypress doesn't also send.
    swallow_enter: Option<std::time::Instant>,
    /// `general.send_with_cmd_enter`: ↩ inserts a newline and ⌘↩ sends (else ↩ sends, ⇧↩ newline).
    cmd_enter: bool,
    picker_scroll: ScrollHandle,
    /// Width of the composer card at last layout; narrow cards get compact pills.
    width: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// Height of the whole composer at last layout. The window caches this view at that height
    /// (cached views need a definite size) on frames where the composer didn't change.
    height: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// The message being edited, to send again in its place.
    editing: Option<Editing>,
    /// Redraws the usage-limit bar's countdown while it's up.
    limit_tick: Option<Task<()>>,
    /// Models to consult with the next message, and how (see `trek_core::orchestrate`).
    consult: Consult,
    /// Keep the consultants after sending (else they clear).
    consult_pinned: bool,
    consult_open: bool,
    /// The agent whose models the consult menu lists.
    consult_rail: Option<AgentId>,
    /// The consultant whose effort the consult menu's side panel is picking.
    consult_effort: Option<usize>,
    /// The consult menu's side panel is picking an arena's judge.
    consult_judge: bool,
    /// Ask the agent to restate the next message in its own words before it does anything
    /// (`trek_core::restate`). Clears once that message is sent.
    restate: bool,
    _subscriptions: Vec<Subscription>,
}

/// A message of yours the composer holds to send again in its place.
struct Editing {
    thread: String,
    item: String,
    /// Put the files back as they were when it was first sent; `None` when there's no checkpoint.
    restore: Option<bool>,
    /// Why the files can't be put back, when they can't.
    why_not: String,
    /// What putting the files back would change, once checked.
    files: EditFiles,
    /// What the composer held before (text, images): back on cancel, and once the edit is sent.
    draft: (String, Vec<PathBuf>),
    _check: Option<Task<()>>,
}

enum EditFiles {
    Checking,
    Changes(Vec<trek_core::checkpoint::FileChange>),
    Failed(String),
}

/// Files a tooltip lists before "and N more".
const FILES_LISTED: usize = 8;

/// True when `id` is `candidate` or a dated snapshot of it (claude-haiku-4-5-20251001).
pub fn same_model(id: &str, candidate: &str) -> bool {
    id == candidate
        || id
            .strip_prefix(candidate)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|date| date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()))
}

/// Display name for a model id.
pub(crate) fn model_name(models: &[ModelInfo], id: &str) -> String {
    models.iter().find(|i| same_model(id, &i.id)).map(|i| i.name.clone()).unwrap_or_else(|| id.to_string())
}

/// The agent's default when the thread hasn't picked one: Opus 5.5 for Claude, the first live model otherwise.
pub(crate) fn default_model(models: &[ModelInfo]) -> Option<&ModelInfo> {
    models.iter().find(|m| m.id == "claude-opus-5-5").or_else(|| models.first())
}

/// `/consult`'s models and message, split at the first colon followed by a space (or ending
/// it): model ids may have colons of their own (`qwen2.5-coder:7b`).
fn consult_split(rest: &str) -> Option<(&str, &str)> {
    let at = rest.char_indices().find(|&(i, c)| c == ':' && rest[i + 1..].chars().next().is_none_or(char::is_whitespace))?.0;
    Some((&rest[..at], &rest[at + 1..]))
}

/// `/restate` alone (`Some(None)`), or with the message to send (`Some(Some(message))`); `None`
/// when `text` isn't the command.
fn restate_command(text: &str) -> Option<Option<String>> {
    let rest = text.trim_start().strip_prefix("/restate")?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let rest = rest.trim();
    Some((!rest.is_empty()).then(|| rest.to_string()))
}

/// A consultant on `model` of `agent`, at `effort` (High, unless asked) within what the model takes.
fn consultant(agent: AgentId, model: &ModelInfo, effort: Option<Effort>) -> Consultant {
    let effort = effort.unwrap_or(Effort::High);
    let effort = if model.efforts.is_empty() { effort } else { effort.clamp_to(&model.efforts) };
    Consultant { agent, model: model.id.clone(), effort }
}

/// The model `query` names among the agents the user can pick: by id or name, whole or in part
/// ("sol", "opus 5.5", "gpt-5.6-luna"), optionally after its agent ("codex sol").
fn find_model(ws: &crate::workspace::Workspace, query: &str) -> Option<(AgentId, ModelInfo)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let all: Vec<(AgentId, ModelInfo)> = ws.ready_agents().into_iter().flat_map(|a| ws.models_for(&a).into_iter().map(move |m| (a.clone(), m))).collect();
    let named = |(a, m): &&(AgentId, ModelInfo)| {
        let agent = a.display_name().to_lowercase();
        [m.id.to_lowercase(), m.name.to_lowercase(), format!("{agent} {}", m.name.to_lowercase()), format!("{} {}", a.key(), m.id.to_lowercase())].contains(&q)
    };
    all.iter()
        .find(named)
        .or_else(|| all.iter().find(|(_, m)| m.name.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q)))
        .cloned()
}

pub(crate) fn hand_icon(level: HandHolding) -> Icon {
    match level {
        HandHolding::Supervised => Icon::new(crate::assets::Lucide::Lock),
        HandHolding::AutoAcceptEdits => Icon::new(crate::assets::Lucide::FilePen),
        HandHolding::Auto => Icon::new(crate::assets::Lucide::Sparkle),
        HandHolding::FullAccess => Icon::new(crate::assets::Lucide::ShieldCheck),
    }
}

impl Composer {
    pub fn new(workspace: Entity<Workspace>, scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cmd_enter = workspace.read(cx).settings.general.send_with_cmd_enter;
        let input = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 12).submit_on_enter(!cmd_enter).placeholder("Ask, build, / for commands, @ for files")
        });
        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, secondary } = event {
                    let send = if this.cmd_enter { *secondary } else { !*shift };
                    let swallowed = this.swallow_enter.take().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250));
                    if send && this.trigger.is_none() && !swallowed {
                        this.submit(state.clone(), window, cx);
                    }
                } else if matches!(event, InputEvent::Change) {
                    this.update_trigger(cx);
                    // The user has started typing: get the agent process up before they hit Return.
                    // Not while editing: sending that starts a session of its own, after the rewind.
                    let typing = { let v = state.read(cx).value(); v.trim().len() >= 2 && !v.starts_with('/') } && this.editing.is_none();
                    if typing {
                        let scope = this.scope.clone();
                        this.workspace.update(cx, |ws, cx| ws.warm_up_in(&scope, cx));
                    }
                    cx.notify();
                }
            }),
            cx.observe_in(&workspace, window, |this, ws, window, cx| {
                let cmd_enter = ws.read(cx).settings.general.send_with_cmd_enter;
                if cmd_enter != this.cmd_enter {
                    this.cmd_enter = cmd_enter;
                    this.input.update(cx, |s, cx| s.set_submit_on_enter(!cmd_enter, cx));
                }
                // The message being edited is gone from view (another thread, or a rewind took it).
                let stale = this.editing.as_ref().is_some_and(|e| {
                    let ws = ws.read(cx);
                    ws.thread_id_in(&this.scope) != Some(e.thread.as_str()) || ws.live.get(&e.thread).is_none_or(|l| l.items.position(&e.item).is_none())
                });
                if stale {
                    this.cancel_edit(window, cx);
                }
                // An open @ picker follows the folder on screen.
                if this.trigger.as_ref().is_some_and(|t| t.kind == PickKind::Mention) {
                    this.ensure_file_index(cx);
                }
                cx.notify()
            }),
            cx.observe(&model_search, |_, _, cx| cx.notify()),
            // The cursor blinks and moves without an input event; this view is cached, so redraw.
            cx.observe(&input, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            workspace,
            scope,
            input,
            model_search,
            clone_input,
            model_open: false,
            access_open: false,
            sub: None,
            rail: None,
            outbox: Outbox::default(),
            trigger: None,
            picked: 0,
            file_index: None,
            indexing: None,
            snapshotting: false,
            swallow_enter: None,
            cmd_enter,
            picker_scroll: ScrollHandle::new(),
            width: std::rc::Rc::new(std::cell::Cell::new(px(760.))),
            height: std::rc::Rc::new(std::cell::Cell::new(px(0.))),
            editing: None,
            limit_tick: None,
            consult: Consult { implement: true, ..Default::default() },
            consult_pinned: false,
            consult_open: false,
            consult_rail: None,
            consult_effort: None,
            consult_judge: false,
            restate: false,
            _subscriptions: subscriptions,
        };
        // TREK_REVIEW_COMPOSER=restate (Restate first on) or arena (the Consult menu open on an
        // arena): states a click reaches, for design review of a window that isn't in front.
        match std::env::var("TREK_REVIEW_COMPOSER").as_deref() {
            Ok("restate") if this.scope == Scope::Main => this.restate = true,
            // Once the agents on this Mac have been found, so every family can take part.
            Ok("arena") if this.scope == Scope::Main => cx
                .spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                    let _ = this.update(cx, |this, cx| {
                        this.consult.style = Style::Arena;
                        this.consult.consultants = this.arena_defaults(cx);
                        this.consult_open = true;
                        cx.notify();
                    });
                })
                .detach(),
            _ => {}
        }
        this
    }

    fn submit(&mut self, state: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target(cx);
        if self.outbox.hold_send(target) {
            cx.notify();
            return;
        }
        let text = state.read(cx).value().to_string();
        if text.trim().is_empty() && self.outbox.paths.is_empty() {
            return;
        }
        // `/consult <models>: <message>` picks consultants (and sends, when there's a message).
        let text = match self.consult_command(&text, cx) {
            Some(Ok(Some(message))) => message,
            Some(Ok(None)) => {
                state.update(cx, |s, cx| s.set_value("", window, cx));
                self.trigger = None;
                self.consult_open = true;
                self.sync_overlay(cx);
                cx.notify();
                return;
            }
            Some(Err(why)) => {
                self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: why, undo: None }));
                return;
            }
            None => text,
        };
        // `/restate <message>` sends it asking for a restatement first. `/restate` alone, in a
        // thread under way, asks the agent to restate the thread so far; in a new thread it turns
        // that on (or off) for the first message.
        let under_way = self.editing.is_none() && self.workspace.read(cx).thread_in(&self.scope).is_some();
        let text = match restate_command(&text) {
            Some(None) if under_way => {
                self.restate = true;
                trek_core::restate::THREAD.to_string()
            }
            Some(None) => {
                state.update(cx, |s, cx| s.set_value("", window, cx));
                self.trigger = None;
                self.restate = !self.restate;
                self.sync_overlay(cx);
                cx.notify();
                return;
            }
            Some(Some(message)) => {
                self.restate = true;
                message
            }
            None => text,
        };
        // With consultants picked, the agent is told to ask them first. Picks clear once it's
        // sent (unless pinned); a message that isn't sent keeps them, and comes back as written.
        let plain = text.clone();
        let restating = self.restate && !text.trim().is_empty() && !text.trim_start().starts_with('/');
        let consulting = !(self.consult.consultants.is_empty() || text.trim().is_empty() || text.trim_start().starts_with('/'));
        if consulting {
            let agent = self.workspace.read(cx).prefs_in(&self.scope).agent;
            if let Some(why) = self.workspace.read(cx).consult_unavailable(&agent) {
                self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: format!("Can't consult: {why}."), undo: None }));
                return;
            }
        }
        let text = if restating { trek_core::restate::with_restate(&text) } else { text };
        // An arena names its judge: the one picked, else Trek's pick.
        let mut consult = self.consult.clone();
        if consulting && consult.style == Style::Arena && consult.judge.is_none() {
            consult.judge = self.arena_judge(cx);
        }
        if let Some(why) = trek_core::orchestrate::arena_problem(&consult).filter(|_| consulting) {
            self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: format!("{why}."), undo: None }));
            return;
        }
        let text = if consulting {
            let names = self.consultant_names(cx);
            trek_core::orchestrate::consult_prompt(&text, &consult, |c| names(c))
        } else {
            text
        };
        if let Some(e) = &self.editing {
            let ws = self.workspace.read(cx);
            let why = if ws.turn_running(&e.thread) {
                Some("Stop the running turn to send the edited message.")
            } else if !ws.can_rewind(&e.thread, &e.item) {
                Some("That message can't be edited any more.")
            } else {
                None
            };
            if let Some(message) = why {
                self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: message.into(), undo: None }));
                return;
            }
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        self.trigger = None;
        self.sync_overlay(cx);
        let images = std::mem::take(&mut self.outbox.paths);
        let scope = self.scope.clone();
        let sent = match self.editing.take() {
            // Sent in place of the message: the conversation goes back to just before it. What
            // the composer held before the edit comes back.
            Some(e) => {
                let restore = e.restore == Some(true) && !matches!(&e.files, EditFiles::Changes(c) if c.is_empty());
                let sent = self.workspace.update(cx, |ws, cx| ws.edit_and_resend(&e.thread, &e.item, text, images.clone(), restore, cx));
                let (text, images) = if sent { e.draft.clone() } else { (plain, images) };
                self.outbox.paths = images;
                state.update(cx, |s, cx| s.set_value(text, window, cx));
                if !sent {
                    self.editing = Some(e);
                }
                sent
            }
            None => {
                self.workspace.update(cx, |ws, cx| ws.send_in(&scope, text, images, cx));
                true
            }
        };
        if consulting && sent && !self.consult_pinned {
            self.consult.consultants.clear();
            self.consult.judge = None;
            self.consult_effort = None;
        }
        if restating && sent {
            self.restate = false;
        }
    }

    /// "Not quite…" on a restatement: the composer takes focus, and the correction is restated too.
    pub fn correct(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restate = true;
        self.focus(window, cx);
        cx.notify();
    }

    /// A message comes back into the composer: put back by a rewind, or (`edit`: its item id) to
    /// edit and send again in its place.
    pub fn compose(&mut self, thread: &str, text: &str, images: &[PathBuf], edit: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        // A message sent with consultants comes back as it was written, its consultants picked
        // (and asking for a restatement again, if it did).
        let (text, consult) = trek_core::orchestrate::split_consult(text);
        if let Some(c) = consult {
            self.consult = c;
        }
        let (text, restate) = trek_core::restate::split_restate(text);
        self.restate |= restate;
        let Some(item) = edit else {
            self.restore(text, images, window, cx);
            return;
        };
        let draft = match self.editing.take() {
            Some(e) => e.draft,
            None => (self.input.read(cx).value().to_string(), std::mem::take(&mut self.outbox.paths)),
        };
        let ws = self.workspace.read(cx);
        let checkpoint = ws.restorable_checkpoint(thread, &item);
        let why_not = ws.no_checkpoint(thread, &item).unwrap_or(crate::workspace::NoCheckpoint::Missing).explain();
        // Which files sending it would put back, found off the main thread.
        let check = checkpoint.as_ref().map(|c| {
            let (repo, sha) = (c.repo.clone(), c.sha.clone());
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let r = trek_core::checkpoint::Repo::find(&repo).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", repo.display()))?;
                        r.changes_since(&sha)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(e) = this.editing.as_mut() {
                        e.files = match result {
                            Ok(changes) => EditFiles::Changes(changes),
                            Err(e) => EditFiles::Failed(format!("{e:#}")),
                        };
                        cx.notify();
                    }
                });
            })
        });
        self.editing = Some(Editing {
            thread: thread.to_string(),
            item,
            restore: checkpoint.is_some().then_some(true),
            why_not,
            files: EditFiles::Checking,
            draft,
            _check: check,
        });
        self.outbox.paths = images.to_vec();
        self.input.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.set_selected_range(text.len()..text.len(), cx);
        });
        self.trigger = None;
        self.sync_overlay(cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Stop editing: what the composer held before comes back.
    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.editing.take() else { return };
        let (text, images) = e.draft;
        self.outbox.paths = images;
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    /// "Editing message" above the prompt while one is.
    fn edit_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.editing.as_ref()?;
        let theme = cx.theme().clone();
        let tooltip = |text: String| move |window: &mut Window, cx: &mut App| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx);
        let restore = match (e.restore, &e.files) {
            (Some(_), EditFiles::Changes(changes)) if changes.is_empty() => {
                ui::check_row("edit-restore", "Restore files", false, true, cx).tooltip(tooltip("The files are as they were when this message was first sent.".into()))
            }
            (Some(on), files) => {
                // What sending it puts back, before it's sent: restoring reverts edits made since
                // (yours too) and removes files created since.
                let (label, tip) = match files {
                    EditFiles::Checking => ("Restore files".to_string(), "Checking which files changed…".to_string()),
                    EditFiles::Failed(why) => ("Restore files".to_string(), format!("Couldn't check the files: {why}")),
                    EditFiles::Changes(changes) => {
                        let mut tip = String::from("Put the files back as they were when this message was first sent:");
                        for f in changes.iter().filter(|f| f.change != trek_core::checkpoint::Change::Nested).take(FILES_LISTED) {
                            let what = match f.change {
                                trek_core::checkpoint::Change::Added => "delete",
                                trek_core::checkpoint::Change::Deleted => "bring back",
                                _ => "revert",
                            };
                            tip.push_str(&format!("\n{what} {}", f.path));
                        }
                        let n = changes.iter().filter(|f| f.change != trek_core::checkpoint::Change::Nested).count();
                        if n > FILES_LISTED {
                            tip.push_str(&format!("\nand {} more", n - FILES_LISTED));
                        }
                        (format!("Restore {n} file{}", if n == 1 { "" } else { "s" }), tip)
                    }
                };
                ui::check_row("edit-restore", label, on, false, cx).tooltip(tooltip(tip)).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(e) = this.editing.as_mut() {
                        e.restore = e.restore.map(|on| !on);
                    }
                    cx.notify();
                }))
            }
            (None, _) => ui::check_row("edit-restore", "Restore files", false, true, cx).tooltip(tooltip(e.why_not.clone())),
        };
        Some(
            h_flex()
                .id("edit-banner")
                .test_support()
                .px(px(14.))
                .pt(px(10.))
                .gap(px(8.))
                .text_size(px(12.5))
                .text_color(theme.muted_foreground)
                .child(Icon::new(crate::assets::Lucide::Pencil).xsmall())
                .child(div().flex_1().min_w_0().truncate().child("Editing message · Esc to cancel"))
                .child(restore)
                .child(
                    gpui_kit::component::button::Button::new("edit-cancel")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Close).text_color(theme.muted_foreground))
                        .tooltip("Cancel editing")
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// The bar above the prompt while the thread is paused at a usage limit: when it resets, what
    /// happens then, and the ways on (resume at the reset, snooze until it, another agent). The
    /// messages waiting for the reset are listed under it.
    fn limit_bar(&mut self, id: &str, compact: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ws = self.workspace.read(cx);
        let pause = ws.pause(id)?.clone();
        if ws.turn_running(id) {
            return None;
        }
        let now = ws.now();
        let snoozed = ws.thread(id).and_then(|t| t.snoozed_until).is_some_and(|u| u > now);
        // The countdown moves on its own: redraw this view (not the window) twice a minute.
        if self.limit_tick.is_none() {
            self.limit_tick = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
                let paused = this.update(cx, |this, cx| {
                    let ws = this.workspace.read(cx);
                    let paused = ws.thread_id_in(&this.scope).is_some_and(|id| ws.pause(id).is_some());
                    if paused {
                        cx.notify();
                    } else {
                        this.limit_tick = None;
                    }
                    paused
                });
                if !paused.unwrap_or(false) {
                    break;
                }
            }));
        }
        let theme = cx.theme().clone();
        let amber = palette::amber(cx);
        let when = |at: i64| {
            let clock = crate::time::reset_clock(at, now);
            if compact { clock } else { format!("{clock} (in {})", crate::time::countdown(at, now)) }
        };
        let (title, detail) = match (pause.resets_at, pause.resume) {
            (Some(at), true) => ("Usage limit reached".to_string(), format!("Resumes at {}", when(at + trek_core::limit::RESUME_GRACE_MS))),
            (Some(at), false) => ("Usage limit reached".to_string(), format!("Resets {}", when(at))),
            (None, _) => ("Usage limit reached".to_string(), "Reset time unknown".to_string()),
        };
        let action = |key: &'static str, label: &'static str, strong: bool| {
            div()
                .id(key)
                .test_support()
                .flex_none()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .text_color(if strong { theme.foreground } else { theme.muted_foreground })
                .when(strong, |el| el.font_medium())
                .hover(|s| s.bg(theme.foreground.opacity(0.06)).text_color(theme.foreground))
                .child(label)
        };
        let ws = self.workspace.clone();
        let on = |f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
            let (ws, id) = (ws.clone(), id.to_string());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| ws.update(cx, |ws, cx| f(ws, &id, cx))
        };
        let mut actions: Vec<AnyElement> = vec![];
        match (pause.resets_at.is_some(), pause.resume) {
            (true, false) => actions.push(action("limit-resume", "Resume at reset", true).on_click(on(|ws, id, cx| ws.resume_at_reset(id, cx))).into_any_element()),
            (true, true) => actions.push(action("limit-cancel", "Cancel", false).on_click(on(|ws, id, cx| ws.cancel_resume(id, cx))).into_any_element()),
            (false, _) => actions.push(action("limit-retry", "Try again", true).on_click(on(|ws, id, cx| ws.end_pause(id, true, cx))).into_any_element()),
        }
        if pause.resets_at.is_some() && !snoozed {
            let label = if compact { "Snooze" } else { "Snooze until reset" };
            actions.push(action("limit-snooze", label, false).on_click(on(|ws, id, cx| ws.snooze_until_reset(id, cx))).into_any_element());
        }
        actions.push(
            action("limit-switch", if compact { "Switch…" } else { "Switch agent…" }, false)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model_open = true;
                    this.sync_overlay(cx);
                    cx.notify();
                }))
                .into_any_element(),
        );
        // Without a reset time, nothing sends them on its own: "Try again" (or the next message) does.
        let waits_for = if pause.resets_at.is_some() { "Sends when your limit resets" } else { "Sends when you try again" };
        let queued = pause.queued.iter().enumerate().map(|(ix, q)| {
            let ws = self.workspace.clone();
            let id = id.to_string();
            let text = if q.text.trim().is_empty() { format!("{} image{}", q.images.len(), if q.images.len() == 1 { "" } else { "s" }) } else { q.text.lines().next().unwrap_or_default().to_string() };
            h_flex()
                .id(("limit-queued", ix))
                .test_support()
                .h(px(28.))
                .px(px(12.))
                .gap(px(8.))
                .border_t_1()
                .border_color(theme.foreground.opacity(0.07))
                .child(Icon::new(crate::assets::Lucide::Clock).xsmall().text_color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().truncate().text_color(theme.foreground.opacity(0.85)).child(text))
                .when(!compact, |el| el.child(div().flex_none().text_color(theme.muted_foreground).child(waits_for)))
                .child(
                    gpui_kit::component::button::Button::new(("limit-unqueue", ix))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Close).text_color(theme.muted_foreground))
                        .tooltip("Don't send this at the reset")
                        .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.unqueue_for_reset(&id, ix, cx))),
                )
                .into_any_element()
        });
        Some(
            v_flex()
                .id("limit-bar")
                .test_support()
                .mx(px(12.))
                .rounded_t(px(12.))
                .border_1()
                .border_b_0()
                .border_color(amber.opacity(0.28))
                .bg(amber.opacity(0.07))
                .text_size(px(12.5))
                .child(
                    h_flex()
                        .h(px(36.))
                        .pl(px(12.))
                        .pr(px(6.))
                        .gap(px(8.))
                        .child(Icon::new(crate::assets::Lucide::Gauge).small().text_color(amber))
                        .child(div().flex_none().font_medium().text_color(theme.foreground).child(title))
                        .child(div().flex_1().min_w_0().truncate().text_color(theme.muted_foreground).child(detail))
                        .children(actions),
                )
                .children(queued)
                .into_any_element(),
        )
    }

    /// Tell the workspace whether one of our popovers is open (native views hide under it). Only
    /// the main window has native views.
    fn sync_overlay(&self, cx: &mut Context<Self>) {
        if self.scope != Scope::Main {
            return;
        }
        let open = self.model_open || self.access_open || self.consult_open || self.trigger.is_some();
        self.workspace.update(cx, |ws, cx| {
            if ws.overlay_open != open {
                ws.overlay_open = open;
                cx.notify();
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The open picker's rows, by label.
    #[cfg(test)]
    pub(crate) fn picks(&self, cx: &App) -> Vec<String> {
        self.picker_items(cx).into_iter().map(|i| i.label).collect()
    }

    /// The consultants picked for the next message (`agent/model/effort`), and whether they stay.
    #[cfg(test)]
    pub(crate) fn consultants(&self) -> (Vec<String>, bool) {
        (self.consult.consultants.iter().map(Consultant::key).collect(), self.consult_pinned)
    }

    /// Whether the next message asks for a restatement first.
    #[cfg(test)]
    pub(crate) fn restating(&self) -> bool {
        self.restate
    }

    /// The judge an arena sent now would name.
    #[cfg(test)]
    pub(crate) fn judge(&self, cx: &App) -> Option<String> {
        self.arena_judge(cx).map(|j| j.key())
    }

    /// Images attached to the next message, and how many are still being saved.
    #[cfg(test)]
    pub(crate) fn attached(&self) -> (Vec<PathBuf>, usize) {
        (self.outbox.paths.clone(), self.outbox.saving)
    }

    #[cfg(test)]
    pub(crate) fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// Height at the last layout (zero before the first).
    pub fn height(&self) -> Pixels {
        self.height.get()
    }

    /// The composer as a window lays it out: cached at its last height on frames where it didn't
    /// change (the working animation's frames, chiefly), laid out from its content when it did.
    /// `changed` is the window's flag, set when the composer notifies and cleared here.
    pub fn element(composer: &Entity<Composer>, changed: &mut bool, cx: &App) -> AnyElement {
        let height = composer.read(cx).height();
        if std::mem::take(changed) || height <= px(0.) {
            return composer.clone().into_any_element();
        }
        composer.clone().cached(StyleRefinement::default().w_full().flex_none().h(height)).into_any_element()
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    /// Insert text at the cursor (on its own line if the cursor is mid-text) and focus the composer.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            let value = s.value().to_string();
            let cursor = s.cursor().min(value.len());
            let needs_break = cursor > 0 && !value[..cursor].ends_with('\n');
            let text = if needs_break { format!("\n{text}") } else { text.to_string() };
            s.insert(text, window, cx);
        });
        self.focus(window, cx);
        cx.notify();
    }

    /// Attach an image file (shown in the attachment strip, sent with the next message).
    pub fn attach_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.outbox.add([path]);
        cx.notify();
    }

    /// Put follow-ups that never went out back into the composer.
    pub fn restore(&mut self, text: &str, images: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let (text, consult) = trek_core::orchestrate::split_consult(text);
        if let Some(c) = consult {
            self.consult = c;
        }
        let (text, restate) = trek_core::restate::split_restate(text);
        self.restate |= restate;
        if !text.is_empty() {
            self.insert_text(text, window, cx);
        }
        for path in images {
            self.attach_image(path.clone(), cx);
        }
    }

    fn update_prefs(&self, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
        let scope = self.scope.clone();
        self.workspace.update(cx, |ws, cx| {
            let mut p = ws.prefs_in(&scope);
            f(&mut p);
            ws.set_prefs_in(&scope, p, cx);
        });
    }

    /// Attach files by inserting @-references into the prompt.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Attach".into()) });
        let input = self.input.clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let (images, files) = attachments::split_files(paths);
                let _ = this.update_in(cx, |this, window, cx| attachments::attach_files(this, images, window, cx));
                if files.is_empty() {
                    return;
                }
                let refs: Vec<String> = files.iter().map(|p| format!("@{}", p.display())).collect();
                let _ = input.update_in(cx, |s, window, cx| {
                    let mut v = s.value().to_string();
                    if !v.is_empty() && !v.ends_with(' ') {
                        v.push(' ');
                    }
                    v.push_str(&refs.join(" "));
                    v.push(' ');
                    s.set_value(v, window, cx);
                });
            }
        })
        .detach();
    }

    /// Take a screenshot with macOS's own picker and attach it. `mode`: "window", "area" or "screen".
    /// The snapshot ⌘⇧S takes (Settings → App Snapshots).
    pub fn snapshot_default(&mut self, cx: &mut Context<Self>) {
        let mode = match self.workspace.read(cx).settings.snapshots.default_mode {
            trek_core::settings::SnapshotMode::Window => "window",
            trek_core::settings::SnapshotMode::Area => "area",
            trek_core::settings::SnapshotMode::Screen => "screen",
        };
        self.snapshot(mode, cx);
    }

    /// Take a screenshot with macOS's own picker and attach it. `mode`: "window", "area" or "screen".
    fn snapshot(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        if self.snapshotting {
            return;
        }
        self.snapshotting = true;
        let prefs = self.workspace.read(cx).settings.snapshots.clone();
        let jpg = prefs.format == trek_core::settings::SnapshotFormat::Jpg;
        let path = mentions::snapshot_path().with_extension(if jpg { "jpg" } else { "png" });
        let out = path.clone();
        if prefs.hide_trek {
            cx.hide();
        }
        cx.spawn(async move |this, cx| {
            if prefs.hide_trek {
                // Let the window finish hiding before the picker appears.
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            }
            let ok = cx
                .background_executor()
                .spawn(async move {
                    let mut cmd = std::process::Command::new("/usr/sbin/screencapture");
                    match mode {
                        "window" => cmd.args(["-i", "-W"]),
                        "area" => cmd.args(["-i", "-s"]),
                        _ => cmd.arg("-m"),
                    };
                    if mode == "window" && !prefs.window_shadow {
                        cmd.arg("-o");
                    }
                    if !prefs.sound {
                        cmd.arg("-x");
                    }
                    cmd.args(["-t", if jpg { "jpg" } else { "png" }]);
                    cmd.arg(&out).status().map(|s| s.success()).unwrap_or(false) && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.snapshotting = false;
                if prefs.hide_trek {
                    cx.activate(true);
                }
                if ok {
                    this.outbox.add([path]);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let (images, files) = attachments::split_files(paths.iter().cloned());
        attachments::attach_files(self, images, window, cx);
        if !files.is_empty() {
            let refs: Vec<String> = files.iter().map(|p| format!("@{} ", p.display())).collect();
            self.input.update(cx, |s, cx| s.insert(refs.concat(), window, cx));
        }
        cx.notify();
    }

    fn insert_trigger(&mut self, ch: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            let v = s.value().to_string();
            let cursor = s.cursor();
            let needs_space = ch != "/" && cursor > 0 && !v[..cursor].ends_with(char::is_whitespace);
            if ch == "/" {
                s.set_value("/", window, cx);
                s.set_selected_range(1..1, cx);
            } else {
                s.insert(if needs_space { format!(" {ch}") } else { ch.to_string() }, window, cx);
            }
        });
        self.focus(window, cx);
        self.update_trigger(cx);
    }

    // ---------- pickers ----------

    fn update_trigger(&mut self, cx: &mut Context<Self>) {
        let state = self.input.read(cx);
        let next = mentions::trigger_at(&state.value(), state.cursor());
        if next.as_ref().map(|t| (&t.kind, &t.query)) != self.trigger.as_ref().map(|t| (&t.kind, &t.query)) {
            self.picked = 0;
        }
        if next.as_ref().is_some_and(|t| t.kind == PickKind::Mention) {
            self.ensure_file_index(cx);
        }
        self.trigger = next;
        self.sync_overlay(cx);
    }

    fn ensure_file_index(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.read(cx).cwd_in(&self.scope) else { return };
        if self.file_index.as_ref().is_some_and(|(r, _)| *r == root) || self.indexing.is_some() {
            return;
        }
        let r = root.clone();
        self.indexing = Some(cx.spawn(async move |this, cx| {
            let files = cx.background_executor().spawn(async move { mentions::index_files(&r) }).await;
            let _ = this.update(cx, |this, cx| {
                this.file_index = Some((root, Arc::new(files)));
                this.indexing = None;
                // The folder on screen changed meanwhile: index that one too.
                if this.trigger.as_ref().is_some_and(|t| t.kind == PickKind::Mention) {
                    this.ensure_file_index(cx);
                }
                cx.notify();
            });
        }));
    }

    fn picker_items(&self, cx: &App) -> Vec<PickItem> {
        let Some(t) = &self.trigger else { return vec![] };
        let ws = self.workspace.read(cx);
        let agent = ws.prefs_in(&self.scope).agent;
        let q = t.query.to_lowercase();
        let commands = ws.slash_commands(&self.scope, &agent);
        let matches = |name: &str, desc: &str| q.is_empty() || name.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q);
        let rank = |name: &str| if name.to_lowercase().starts_with(&q) { 0 } else { 1 };
        let mut items: Vec<PickItem> = match t.kind {
            PickKind::Slash => {
                let mut v: Vec<_> = commands.iter().filter(|c| c.kind != trek_agents::CommandKind::Agent && matches(&c.name, &c.description)).collect();
                v.sort_by_key(|c| rank(&c.name));
                v.into_iter()
                    .map(|c| PickItem {
                        label: format!("/{}", c.name),
                        detail: c.description.clone(),
                        insert: format!("/{}", c.name),
                        icon: if c.kind == trek_agents::CommandKind::Skill { PickIcon::Skill } else { PickIcon::Command },
                    })
                    .collect()
            }
            PickKind::Skill => {
                let codex = agent == AgentId::Codex;
                let mut v: Vec<_> = commands.iter().filter(|c| c.kind == trek_agents::CommandKind::Skill && matches(&c.name, &c.description)).collect();
                v.sort_by_key(|c| rank(&c.name));
                v.into_iter()
                    .map(|c| PickItem {
                        label: c.name.clone(),
                        detail: c.description.clone(),
                        // Codex invokes skills as $name; Claude Code as /name.
                        insert: if codex { format!("${}", c.name) } else { format!("/{}", c.name) },
                        icon: PickIcon::Skill,
                    })
                    .collect()
            }
            PickKind::Mention => {
                let mut v: Vec<PickItem> = commands
                    .iter()
                    .filter(|c| c.kind == trek_agents::CommandKind::Agent && matches(&c.name, &c.description))
                    .take(6)
                    .map(|c| PickItem { label: format!("agent-{}", c.name), detail: c.description.clone(), insert: format!("@agent-{}", c.name), icon: PickIcon::Agent })
                    .collect();
                // Another folder's files (the window moved to a thread elsewhere) aren't offered.
                if let Some((_, files)) = self.file_index.as_ref().filter(|(root, _)| ws.cwd_in(&self.scope).as_ref() == Some(root)) {
                    v.extend(mentions::match_files(files, &t.query, 40).into_iter().map(|f| {
                        let dir = f.ends_with('/');
                        let trimmed = f.trim_end_matches('/');
                        let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
                        PickItem {
                            label: if dir { format!("{name}/") } else { name.to_string() },
                            detail: parent.to_string(),
                            insert: format!("@{f}"),
                            icon: if dir { PickIcon::Folder } else { PickIcon::File },
                        }
                    }));
                }
                v
            }
        };
        items.truncate(60);
        items
    }

    fn accept_pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.trigger.clone() else { return };
        let Some(item) = self.picker_items(cx).into_iter().nth(ix) else { return };
        let folder = item.icon == PickIcon::Folder;
        self.input.update(cx, |s, cx| {
            let cursor = s.cursor();
            s.set_selected_range(t.start..cursor, cx);
            // Folders keep the picker open so you can keep drilling down.
            s.replace(if folder { item.insert.clone() } else { format!("{} ", item.insert) }, window, cx);
        });
        self.picked = 0;
        self.update_trigger(cx);
        cx.notify();
    }

    fn picker_action(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if key == "escape" && self.trigger.is_none() && self.editing.is_some() {
            self.cancel_edit(window, cx);
            cx.stop_propagation();
            return;
        }
        let n = self.picker_items(cx).len();
        if self.trigger.is_none() || (n == 0 && key != "escape") {
            cx.propagate();
            return;
        }
        match key {
            "escape" => {
                self.trigger = None;
                self.sync_overlay(cx);
            }
            "up" => self.picked = (self.picked + n - 1) % n,
            "down" => self.picked = (self.picked + 1) % n,
            _ => self.accept_pick(self.picked.min(n - 1), window, cx),
        }
        self.picker_scroll.scroll_to_item(self.picked);
        cx.stop_propagation();
        cx.notify();
    }

    /// Enter reaches us before the textarea: it picks a row while a picker is open, and sends on
    /// ⌘↩ in "send with ⌘↩" mode (the textarea would otherwise insert a newline first).
    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.trigger.is_some() && !action.shift && !action.secondary {
            self.picker_action("enter", window, cx);
        } else if self.cmd_enter && action.secondary && !action.shift {
            cx.stop_propagation();
            let input = self.input.clone();
            self.submit(input, window, cx);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.trigger.is_none() {
            return;
        }
        let n = self.picker_items(cx).len();
        let ks = &ev.keystroke;
        if ks.modifiers.modified() && !ks.modifiers.shift {
            return;
        }
        match ks.key.as_str() {
            "escape" => {
                self.trigger = None;
                self.sync_overlay(cx);
            }
            "up" if n > 0 => self.picked = (self.picked + n - 1) % n,
            "down" if n > 0 => self.picked = (self.picked + 1) % n,
            "enter" | "tab" if n > 0 && !ks.modifiers.shift => {
                if ks.key == "enter" {
                    self.swallow_enter = Some(std::time::Instant::now());
                }
                self.accept_pick(self.picked.min(n - 1), window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.trigger.as_ref()?;
        let items = self.picker_items(cx);
        let theme = cx.theme().clone();
        let title = match t.kind {
            PickKind::Slash => "Commands",
            PickKind::Mention => "Files and agents",
            PickKind::Skill => "Skills",
        };
        let empty = match t.kind {
            PickKind::Mention if self.indexing.is_some() => "Indexing project files…",
            PickKind::Mention => "No matching files",
            PickKind::Skill => "No matching skills",
            PickKind::Slash => "No matching commands",
        };
        let picked = self.picked.min(items.len().saturating_sub(1));
        Some(
            ui::menu_surface(cx)
                .w_full()
                .child(
                    h_flex()
                        .px(px(10.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_1().child(title))
                        .child("↑↓ to move · ↩ to pick · esc"),
                )
                .when(items.is_empty(), |el| el.child(div().px(px(10.)).py(px(8.)).text_sm().text_color(theme.muted_foreground).child(empty)))
                .child(v_flex().id("picker-list").test_support().max_h(px(280.)).overflow_y_scroll().track_scroll(&self.picker_scroll).children(items.into_iter().enumerate().map(|(i, item)| {
                    let icon = match item.icon {
                        PickIcon::Command => Icon::new(IconName::SquareTerminal),
                        PickIcon::Skill => Icon::new(crate::assets::Lucide::Sparkle),
                        PickIcon::Agent => Icon::new(IconName::Bot),
                        PickIcon::File => Icon::new(IconName::File),
                        PickIcon::Folder => Icon::new(IconName::Folder),
                    };
                    ui::menu_row(("pick", i), false, cx)
                        .min_h(px(30.))
                        .when(i == picked, |el| el.bg(theme.foreground.opacity(0.09)))
                        .child(icon.small().text_color(theme.muted_foreground))
                        .child(div().flex_none().max_w(px(260.)).truncate().child(item.label))
                        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(item.detail))
                        .on_click(cx.listener(move |this, _, window, cx| this.accept_pick(i, window, cx)))
                })))
                .into_any_element(),
        )
    }

    fn attachment_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.outbox.paths.is_empty() && !self.snapshotting && self.outbox.saving == 0 {
            return None;
        }
        let me = cx.entity().downgrade();
        let remove = move |i: usize, _: &mut Window, cx: &mut App| {
            let _ = me.update(cx, |this, cx| {
                if i < this.outbox.paths.len() {
                    this.outbox.paths.remove(i);
                }
                cx.notify();
            });
        };
        Some(div().px(px(14.)).pt(px(12.)).child(attachments::thumbnails(&self.outbox.paths, px(56.), self.snapshotting || self.outbox.saving > 0, remove, cx)).into_any_element())
    }

    /// "+" menu: files and photos, snapshots, the three pickers, and restating first.
    fn plus_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let theme = cx.theme().clone();
        let restate = self.restate;
        gpui_kit::component::button::Button::new("attach")
            .ghost()
            .with_size(px(30.))
            .rounded(px(8.))
            .bg(theme.foreground.opacity(0.065))
            .icon(Icon::new(IconName::Plus).text_color(theme.muted_foreground))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let (a, b, c, d, e, f, g, h) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
                menu.min_w(px(230.))
                    .check_side(gpui_kit::component::Side::Right)
                    .item(PopupMenuItem::new("Add photos & files").icon(crate::assets::Lucide::Image).on_click(move |_, window, cx| a.update(cx, |c, cx| c.attach(window, cx))))
                    .separator()
                    .item(PopupMenuItem::new("Snapshot a window").icon(crate::assets::Lucide::Camera).on_click(move |_, _, cx| b.update(cx, |c, cx| c.snapshot("window", cx))))
                    .item(PopupMenuItem::new("Snapshot an area").icon(crate::assets::Lucide::Crosshair).on_click(move |_, _, cx| c.update(cx, |c, cx| c.snapshot("area", cx))))
                    .item(PopupMenuItem::new("Snapshot the screen").icon(crate::assets::Lucide::Monitor).on_click(move |_, _, cx| d.update(cx, |c, cx| c.snapshot("screen", cx))))
                    .separator()
                    .item(PopupMenuItem::new("Mention a file  @").icon(IconName::File).on_click(move |_, window, cx| e.update(cx, |c, cx| c.insert_trigger("@", window, cx))))
                    .item(PopupMenuItem::new("Use a skill  $").icon(crate::assets::Lucide::Sparkle).on_click(move |_, window, cx| f.update(cx, |c, cx| c.insert_trigger("$", window, cx))))
                    .item(PopupMenuItem::new("Run a command  /").icon(IconName::SquareTerminal).on_click(move |_, window, cx| g.update(cx, |c, cx| c.insert_trigger("/", window, cx))))
                    .separator()
                    .item(PopupMenuItem::new("Restate first").icon(crate::assets::Lucide::MessageSquareQuote).checked(restate).on_click(move |_, window, cx| {
                        h.update(cx, |c, cx| {
                            c.restate = !c.restate;
                            c.focus(window, cx);
                            cx.notify();
                        })
                    }))
            })
    }

    /// MonoCode-style context meter: a ring that fills as the context window does.
    fn context_ring(&self, used: u64, window: u64, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let frac = (used as f32 / window.max(1) as f32).clamp(0.0, 1.0);
        let color = if frac >= 0.9 { palette::red(cx) } else if frac >= 0.75 { palette::amber(cx) } else { theme.foreground.opacity(0.75) };
        let track = theme.foreground.opacity(0.14);
        let tip_title = format!("{:.0}% context used", frac * 100.);
        let tip_detail = format!("{} / {} tokens", crate::workspace::fmt_tokens(used), crate::workspace::fmt_tokens(window));
        div()
            .id("context-ring")
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .hover(|s| s.bg(theme.foreground.opacity(0.065)))
            .tooltip(move |window, cx| {
                let (t, d) = (tip_title.clone(), tip_detail.clone());
                gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                    v_flex().gap(px(2.)).child(div().text_sm().font_medium().child(t.clone())).child(div().text_xs().text_color(cx.theme().muted_foreground).child(d.clone()))
                })
                .build(window, cx)
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let c = bounds.center();
                        let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.5);
                        let ring = |from: f32, to: f32, color: Hsla, window: &mut Window| {
                            let steps = ((to - from) * 96.).ceil().max(2.) as usize;
                            let mut path = PathBuilder::stroke(px(2.));
                            for i in 0..=steps {
                                let t = from + (to - from) * i as f32 / steps as f32;
                                let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                                let p = point(c.x + r * a.cos(), c.y + r * a.sin());
                                if i == 0 { path.move_to(p) } else { path.line_to(p) }
                            }
                            if let Ok(p) = path.build() {
                                window.paint_path(p, color);
                            }
                        };
                        ring(0.0, 1.0, track, window);
                        if frac > 0.004 {
                            ring(0.0, frac, color, window);
                        }
                    },
                )
                .size(px(16.)),
            )
            .into_any_element()
    }

    // ---------- model menu ----------

    fn model_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let models = ws.models_for(&prefs.agent);
        let current = prefs.model.clone().or_else(|| default_model(&models).map(|m| m.id.clone()));
        let current_info = current.as_ref().and_then(|c| models.iter().find(|m| same_model(c, &m.id))).cloned();
        let fast_ok = current_info.as_ref().is_some_and(|m| m.fast.is_some());
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        let main = ui::menu_surface(cx)
            .w(px(260.))
            .when(fast_ok, |el| {
                el.child(
                    ui::menu_row("mm-fast", false, cx)
                        .child(Icon::new(crate::assets::Lucide::Zap).small().text_color(muted))
                        .child(div().flex_1().child("Fast"))
                        .child(Switch::new("mm-fast-switch").small().checked(prefs.fast).on_click(cx.listener(|this, v: &bool, _, cx| {
                            let v = *v;
                            this.update_prefs(cx, |p| p.fast = v);
                        }))),
                )
            })
            .child(
                ui::menu_row("mm-effort", self.sub == Some(Sub::Effort), cx)
                    .child(Icon::new(crate::assets::Lucide::Sparkle).small().text_color(muted))
                    .child(div().flex_1().child("Effort"))
                    .child(div().text_color(muted).child(prefs.effort.label()))
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.sub = Some(Sub::Effort);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sub = Some(Sub::Effort);
                        cx.notify();
                    })),
            )
            .child(
                ui::menu_row("mm-model", self.sub == Some(Sub::Model), cx)
                    .child(ui::agent_glyph(&prefs.agent, cx))
                    .child(div().flex_1().child("Model"))
                    .child(div().text_color(muted).max_w(px(120.)).truncate().child(current.as_deref().map(|c| model_name(&models, c)).unwrap_or_default()))
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.sub = Some(Sub::Model);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sub = Some(Sub::Model);
                        cx.notify();
                    })),
            );

        let sub = match self.sub {
            Some(Sub::Effort) => Some(self.effort_panel(&prefs, current_info.as_ref(), cx)),
            Some(Sub::Model) => Some(self.model_panel(&prefs, current.as_deref(), cx)),
            None => None,
        };
        h_flex().id("model-menu-body").test_support().items_end().gap(px(6.)).child(main).children(sub).into_any_element()
    }

    fn effort_panel(&mut self, prefs: &Prefs, model: Option<&ModelInfo>, cx: &mut Context<Self>) -> AnyElement {
        let efforts: Vec<Effort> = model
            .map(|m| m.efforts.clone())
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| vec![Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
        let current = prefs.effort;
        ui::menu_surface(cx)
            .w(px(200.))
            .children(efforts.into_iter().map(|e| {
                ui::menu_row(SharedString::from(format!("eff-{}", e.as_str())), e == current, cx)
                    .child(div().flex_1().child(e.label()))
                    .when(e == current, |el| el.child(Icon::new(IconName::Check).small()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_prefs(cx, |p| p.effort = e);
                        this.model_open = false;
                        this.sync_overlay(cx);
                        this.sub = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn model_panel(&mut self, prefs: &Prefs, current: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let agents = ws.ready_agents();
        let favorites = ws.settings.general.favorite_models.clone();
        let rail = self.rail.clone().unwrap_or_else(|| Rail::Agent(prefs.agent.clone()));
        let query = self.model_search.read(cx).value().to_lowercase();
        let theme = cx.theme().clone();

        // (agent, model) pairs for the selected rail entry.
        let list: Vec<(AgentId, ModelInfo)> = match &rail {
            Rail::Agent(a) => ws.models_for(a).into_iter().map(|m| (a.clone(), m)).collect(),
            Rail::Favorites => agents
                .iter()
                .flat_map(|a| ws.models_for(a).into_iter().map(move |m| (a.clone(), m)))
                .filter(|(a, m)| favorites.contains(&format!("{}/{}", a.key(), m.id)))
                .collect(),
        };
        let list: Vec<(AgentId, ModelInfo)> =
            list.into_iter().filter(|(_, m)| query.is_empty() || m.name.to_lowercase().contains(&query) || m.id.to_lowercase().contains(&query)).collect();

        let rail_button = |id: SharedString, active: bool, icon: AnyElement, tip: SharedString, target: Rail, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .when(active, |el| el.bg(theme.list_active))
                .hover(|s| s.bg(theme.list_active))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child(icon)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rail = Some(target.clone());
                    cx.notify();
                }))
        };
        let mut rail_col = v_flex().gap_1().p(px(5.)).border_r_1().border_color(theme.border).child(rail_button(
            "rail-fav".into(),
            rail == Rail::Favorites,
            Icon::new(IconName::Star).small().text_color(theme.muted_foreground).into_any_element(),
            "Favorites".into(),
            Rail::Favorites,
            cx,
        ));
        for a in &agents {
            rail_col = rail_col.child(rail_button(
                SharedString::from(format!("rail-{}", a.key())),
                rail == Rail::Agent(a.clone()),
                ui::agent_glyph(a, cx).into_any_element(),
                a.display_name().into(),
                Rail::Agent(a.clone()),
                cx,
            ));
        }

        let rows = list.into_iter().map(|(agent, m)| {
            let selected = agent == prefs.agent && current.is_some_and(|c| same_model(c, &m.id));
            let fav_key = format!("{}/{}", agent.key(), m.id);
            let is_fav = favorites.contains(&fav_key);
            let id = m.id.clone();
            let agent2 = agent.clone();
            ui::menu_row(SharedString::from(format!("m-{}-{}", agent.key(), m.id)), selected, cx)
                .group("model-row")
                .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
                .child(
                    div()
                        .id(SharedString::from(format!("fav-{fav_key}")))
                        .when(!is_fav, |el| el.invisible().group_hover("model-row", |s| s.visible()))
                        .child(Icon::new(if is_fav { IconName::StarFill } else { IconName::Star }).xsmall().text_color(theme.muted_foreground))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let key = fav_key.clone();
                            this.workspace.update(cx, |ws, cx| {
                                let favs = &mut ws.settings.general.favorite_models;
                                if let Some(i) = favs.iter().position(|f| *f == key) {
                                    favs.remove(i);
                                } else {
                                    favs.push(key);
                                }
                                ws.save_settings(cx);
                            });
                        })),
                )
                .when(selected, |el| el.child(Icon::new(IconName::Check).small()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let (agent, id) = (agent2.clone(), id.clone());
                    this.update_prefs(cx, |p| {
                        p.agent = agent;
                        p.model = Some(id);
                    });
                    this.model_open = false;
                        this.sync_overlay(cx);
                    this.sub = None;
                    cx.notify();
                }))
        });

        ui::menu_surface(cx)
            .p_0()
            .w(px(360.))
            .child(
                h_flex()
                    .items_start()
                    .child(rail_col)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().p(px(6.)).border_b_1().border_color(theme.border).child(
                                Input::new(&self.model_search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                            ))
                            .child(
                                v_flex()
                                    .id("model-list")
                                    .p(px(5.))
                                    .max_h(px(360.))
                                    .overflow_y_scroll()
                                    .children(rows)
                                    .when(self.list_is_empty(&rail, cx), |el| {
                                        el.child(div().p_3().text_sm().text_color(theme.muted_foreground).child(match rail {
                                            Rail::Favorites => "Hover a model and press the star to keep it here.",
                                            _ => "No models match.",
                                        }))
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn list_is_empty(&self, rail: &Rail, cx: &App) -> bool {
        let ws = self.workspace.read(cx);
        match rail {
            Rail::Agent(a) => ws.models_for(a).is_empty(),
            Rail::Favorites => ws.settings.general.favorite_models.is_empty(),
        }
    }

    // ---------- consult ----------

    /// "Sol · High": a consultant as the menu and the agent's instructions name it.
    fn consultant_names(&self, cx: &App) -> Box<dyn Fn(&Consultant) -> String> {
        let ws = self.workspace.read(cx);
        let models: Vec<(AgentId, Vec<ModelInfo>)> = self.consult.consultants.iter().map(|c| (c.agent.clone(), ws.models_for(&c.agent))).collect();
        Box::new(move |c: &Consultant| {
            let name = models.iter().find(|(a, _)| *a == c.agent).map(|(_, m)| model_name(m, &c.model)).unwrap_or_else(|| c.model.clone());
            format!("{name} · {}", c.effort.label())
        })
    }

    /// `/consult sol high, opus max: <message>` (also `discuss`, `report`): the consultants it
    /// names are picked; returns the message to send, `None` when there's none (the menu opens).
    /// `None` overall when `text` isn't the command.
    fn consult_command(&mut self, text: &str, cx: &mut Context<Self>) -> Option<Result<Option<String>, String>> {
        let rest = text.trim_start().strip_prefix("/consult")?;
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            return None;
        }
        let agent = self.workspace.read(cx).prefs_in(&self.scope).agent;
        if let Some(why) = self.workspace.read(cx).consult_unavailable(&agent) {
            return Some(Err(format!("Can't consult: {why}.")));
        }
        let (targets, message) = match consult_split(rest) {
            Some((t, m)) => (t, Some(m.trim().to_string()).filter(|m| !m.is_empty())),
            None => (rest, None),
        };
        let ws = self.workspace.read(cx);
        let mut picked = vec![];
        for part in targets.split(',') {
            let mut words: Vec<&str> = part.split_whitespace().collect();
            words.retain(|w| match w.to_lowercase().as_str() {
                "discuss" => {
                    self.consult.style = Style::Discuss;
                    false
                }
                "advise" => {
                    self.consult.style = Style::Advise;
                    false
                }
                "arena" => {
                    self.consult.style = Style::Arena;
                    false
                }
                "report" => {
                    self.consult.implement = false;
                    false
                }
                "implement" => {
                    self.consult.implement = true;
                    false
                }
                _ => true,
            });
            if words.is_empty() {
                continue;
            }
            let effort = words.last().and_then(|w| Effort::parse(w)).filter(|_| words.len() > 1);
            if effort.is_some() {
                words.pop();
            }
            let query = words.join(" ");
            let Some((agent, model)) = find_model(ws, &query) else { return Some(Err(format!("There's no model called “{query}” to consult."))) };
            picked.push(consultant(agent, &model, effort));
        }
        // An arena's designs all run at once, so there are no more than can.
        let most = trek_core::orchestrate::MAX_RUNNING;
        if self.consult.style == Style::Arena && picked.len() > most {
            return Some(Err(format!("An arena drafts {most} designs at most, all at once: name up to {most} models.")));
        }
        if !picked.is_empty() {
            self.consult.consultants = picked;
        }
        // Picks kept from an advice or a discussion: the first that can run at once, as the
        // menu's switch to Arena keeps.
        if self.consult.style == Style::Arena {
            self.consult.consultants.truncate(most);
        }
        // An arena with nobody named draws one design from each model family.
        if self.consult.style == Style::Arena && self.consult.consultants.is_empty() {
            self.consult.consultants = self.arena_defaults(cx);
        }
        match message {
            Some(_) if self.consult.consultants.is_empty() => Some(Err("Name a model to consult: /consult sol high: your question".into())),
            message => Some(Ok(message)),
        }
    }

    /// Every model on offer for an arena, each agent's default first, then its others, smartest
    /// first.
    fn arena_options(&self, cx: &App) -> Vec<(AgentId, ModelInfo)> {
        let ws = self.workspace.read(cx);
        let (mut defaults, mut others) = (vec![], vec![]);
        for agent in ws.ready_agents() {
            let mut models = ws.models_for(&agent);
            let default = default_model(&models).map(|m| m.id.clone());
            models.sort_by_key(|m| std::cmp::Reverse(m.tier));
            for m in models {
                if Some(&m.id) == default.as_ref() {
                    defaults.push((agent.clone(), m));
                } else {
                    others.push((agent.clone(), m));
                }
            }
        }
        defaults.append(&mut others);
        defaults
    }

    /// The thread's own agent and model.
    fn main_model(&self, cx: &App) -> (AgentId, String) {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let models = ws.models_for(&prefs.agent);
        let model = prefs.model.clone().or_else(|| default_model(&models).map(|m| m.id.clone())).unwrap_or_default();
        // As the model list names it, so it's told apart from the others there.
        let model = models.iter().find(|m| same_model(&model, &m.id)).map(|m| m.id.clone()).unwrap_or(model);
        (prefs.agent, model)
    }

    /// An arena's candidates when none are picked: one per model family.
    fn arena_defaults(&self, cx: &App) -> Vec<Consultant> {
        trek_core::orchestrate::arena_defaults(&self.arena_options(cx))
    }

    /// Who judges the arena: the one picked, else a model of another family than the thread's.
    fn arena_judge(&self, cx: &App) -> Option<Consultant> {
        let (agent, model) = self.main_model(cx);
        self.consult.judge.clone().or_else(|| trek_core::orchestrate::pick_judge((&agent, &model), &self.consult.consultants, &self.arena_options(cx)))
    }

    /// The consult menu: who's consulted, at what effort, how, and then what.
    fn consult_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let agents = ws.ready_agents();
        // Another agent than the thread's own comes first: a second opinion from elsewhere.
        let rail = self.consult_rail.clone().filter(|a| agents.contains(a)).or_else(|| agents.iter().find(|a| **a != prefs.agent).cloned()).or_else(|| agents.first().cloned());
        let models = rail.as_ref().map(|a| ws.models_for(a)).unwrap_or_default();
        let picked_models: Vec<Vec<ModelInfo>> = self.consult.consultants.iter().map(|c| ws.models_for(&c.agent)).collect();
        let main_agent = prefs.agent.display_name();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let hairline = theme.foreground.opacity(0.07);
        let me = cx.entity();
        let empty = self.consult.consultants.is_empty();

        let header = h_flex()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(2.))
            .gap(px(6.))
            .child(div().flex_1().text_size(px(13.5)).font_medium().child("Consult other models"))
            .when(!empty, |el| {
                el.child(
                    div()
                        .id("consult-clear")
                        .test_support()
                        .text_size(px(12.5))
                        .text_color(muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child("Clear")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.consult.consultants.clear();
                            this.consult.judge = None;
                            this.consult_effort = None;
                            this.consult_judge = false;
                            cx.notify();
                        })),
                )
            })
            .child(
                gpui_kit::component::button::Button::new("consult-pin")
                    .ghost()
                    .xsmall()
                    .selected(self.consult_pinned)
                    .icon(Icon::new(if self.consult_pinned { crate::assets::Lucide::Pin } else { crate::assets::Lucide::PinOff }).text_color(if self.consult_pinned { theme.foreground } else { muted }))
                    .tooltip(if self.consult_pinned { "Kept for every message: click to clear after sending" } else { "Keep for the next messages too" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.consult_pinned = !this.consult_pinned;
                        cx.notify();
                    })),
            );
        let note = match self.consult.style {
            Style::Advise => format!("They review and suggest; {main_agent} decides."),
            Style::Discuss => format!("{main_agent} and they go back and forth until they agree ({} rounds at most).", trek_core::orchestrate::DISCUSS_ROUNDS),
            Style::Arena => {
                let (agent, model) = self.main_model(cx);
                let own = trek_core::orchestrate::family(&agent, &model);
                let judge = match self.arena_judge(cx) {
                    Some(j) if trek_core::orchestrate::family(&j.agent, &j.model) == own => "another model",
                    _ => "a model of another family",
                };
                format!("{main_agent} grounds the problem; each drafts a design on its own; {judge} judges them blind; {main_agent} synthesises the best.")
            }
        };
        // An arena's designs all run at once: no more than can.
        let most = trek_core::orchestrate::MAX_RUNNING;
        let full = self.consult.style == Style::Arena && self.consult.consultants.len() >= most;
        let note = if full { format!("{note} {most} designs at most.") } else { note };
        let picked = v_flex().px(px(5.)).children(self.consult.consultants.iter().enumerate().map(|(i, c)| {
            let name = picked_models.get(i).map(|m| model_name(m, &c.model)).unwrap_or_else(|| c.model.clone());
            let choosing = self.consult_effort == Some(i);
            ui::menu_row(("consultant", i), choosing, cx)
                .test_support()
                .min_h(px(32.))
                .child(ui::agent_glyph(&c.agent, cx))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(
                    h_flex()
                        .id(("consultant-effort", i))
                        .test_support()
                        .gap(px(2.))
                        .text_color(muted)
                        .hover(|s| s.text_color(theme.foreground))
                        .child(c.effort.label())
                        .child(Icon::new(IconName::ChevronRight).xsmall())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.consult_effort = if this.consult_effort == Some(i) { None } else { Some(i) };
                            this.consult_judge = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("consultant-remove", i))
                        .test_support()
                        .child(Icon::new(IconName::Close).xsmall().text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if i < this.consult.consultants.len() {
                                this.consult.consultants.remove(i);
                            }
                            this.consult_effort = None;
                            cx.notify();
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.consult_effort = if this.consult_effort == Some(i) { None } else { Some(i) };
                    this.consult_judge = false;
                    cx.notify();
                }))
        }));
        // An arena's judge, with a side panel to pick another.
        let judge = (self.consult.style == Style::Arena).then(|| {
            let j = self.arena_judge(cx);
            let name = j.as_ref().map(|j| format!("{} · {}", model_name(&ws.models_for(&j.agent), &j.model), j.effort.label())).unwrap_or_else(|| "No other model on offer".into());
            v_flex().px(px(5.)).pt(px(2.)).child(
                ui::menu_row("consult-judge", self.consult_judge, cx)
                    .test_support()
                    .min_h(px(32.))
                    .child(Icon::new(crate::assets::Lucide::Scale).small().text_color(muted))
                    .child(div().flex_none().text_color(muted).child("Judged by"))
                    .children(j.as_ref().map(|j| ui::agent_glyph(&j.agent, cx)))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.consult_judge = !this.consult_judge;
                        this.consult_effort = None;
                        cx.notify();
                    })),
            )
        });
        // Which agent's models to list: a row of tabs, so no logo sits beside a model it isn't.
        let rail_row = h_flex().flex_wrap().gap(px(2.)).px(px(6.)).pt(px(6.)).children(agents.iter().map(|a| {
            let active = rail.as_ref() == Some(a);
            let (target, tip) = (a.clone(), SharedString::from(a.display_name()));
            div()
                .id(SharedString::from(format!("consult-rail-{}", a.key())))
                .test_support()
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .cursor_pointer()
                .when(active, |el| el.bg(theme.list_active))
                .hover(|s| s.bg(theme.list_active))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child(ui::agent_glyph(a, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.consult_rail = Some(target.clone());
                    cx.notify();
                }))
        }));
        let list = v_flex().id("consult-models").min_w_0().px(px(5.)).pt(px(4.)).max_h(px(220.)).overflow_y_scroll().children(rail.iter().flat_map(|agent| {
            models.iter().map(|m| {
                let on = self.consult.consultants.iter().any(|c| c.agent == *agent && same_model(&c.model, &m.id));
                let (agent, model) = (agent.clone(), m.clone());
                ui::menu_row(SharedString::from(format!("consult-add-{}-{}", agent.key(), m.id)), false, cx)
                    .test_support()
                    .min_h(px(30.))
                    .when(full && !on, |el| el.opacity(0.45).cursor_default())
                    .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
                    .when(on, |el| el.child(Icon::new(IconName::Check).small()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let at = this.consult.consultants.iter().position(|c| c.agent == agent && same_model(&c.model, &model.id));
                        let full = this.consult.style == Style::Arena && this.consult.consultants.len() >= trek_core::orchestrate::MAX_RUNNING;
                        match at {
                            Some(i) => _ = this.consult.consultants.remove(i),
                            None if full => return,
                            None => this.consult.consultants.push(consultant(agent.clone(), &model, None)),
                        }
                        this.consult_effort = None;
                        cx.notify();
                    }))
            })
        }));
        let (m1, m2) = (me.clone(), me.clone());
        let main = ui::menu_surface(cx)
            .id("consult-menu-body")
            .test_support()
            .w(px(330.))
            .p_0()
            .pb(px(8.))
            .child(header)
            .child(div().px(px(10.)).pb(px(6.)).text_size(px(12.5)).line_height(relative(1.4)).text_color(muted).child(note))
            .when(!empty, |el| el.child(picked))
            .children(judge)
            .child(v_flex().mt(px(6.)).border_t_1().border_color(hairline).child(rail_row).child(list))
            .child(
                v_flex()
                    // The switches hug their options: stretched, they'd trail an empty track.
                    .items_start()
                    .gap(px(8.))
                    .px(px(10.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(hairline)
                    .child(ui::segmented(
                        "consult-style",
                        vec![(Style::Advise, "Advise"), (Style::Discuss, "Discuss"), (Style::Arena, "Arena")],
                        self.consult.style,
                        move |v, _, cx| {
                            m1.update(cx, |c, cx| {
                                c.consult.style = v;
                                // An arena starts from one design per model family.
                                if v == Style::Arena && c.consult.consultants.is_empty() {
                                    c.consult.consultants = c.arena_defaults(cx);
                                }
                                if v == Style::Arena {
                                    c.consult.consultants.truncate(trek_core::orchestrate::MAX_RUNNING);
                                }
                                if v != Style::Arena {
                                    c.consult_judge = false;
                                }
                                cx.notify();
                            })
                        },
                        cx,
                    ))
                    .child(ui::segmented(
                        "consult-then",
                        vec![(true, "Then implement"), (false, "Just report")],
                        self.consult.implement,
                        move |v, _, cx| {
                            m2.update(cx, |c, cx| {
                                c.consult.implement = v;
                                cx.notify();
                            })
                        },
                        cx,
                    )),
            );
        let judges = self.consult_judge.then(|| {
            let (agent, model) = self.main_model(cx);
            let current = self.arena_judge(cx);
            let options = trek_core::orchestrate::judges((&agent, &model), &self.arena_options(cx));
            ui::menu_surface(cx)
                .id("consult-judges")
                .test_support()
                .w(px(230.))
                .max_h(px(300.))
                .overflow_y_scroll()
                .when(options.is_empty(), |el| el.child(div().p(px(8.)).text_size(px(12.5)).text_color(muted).child("There's no other model on offer to judge.")))
                .children(options.into_iter().map(|(agent, m)| {
                    let on = current.as_ref().is_some_and(|j| j.agent == agent && same_model(&j.model, &m.id));
                    ui::menu_row(SharedString::from(format!("consult-judge-{}-{}", agent.key(), m.id)), on, cx)
                        .test_support()
                        .child(ui::agent_glyph(&agent, cx))
                        .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
                        .when(on, |el| el.child(Icon::new(IconName::Check).small()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.consult.judge = Some(consultant(agent.clone(), &m, None));
                            this.consult_judge = false;
                            cx.notify();
                        }))
                }))
                .into_any_element()
        });
        let side = self.consult_effort.and_then(|i| {
            let c = self.consult.consultants.get(i)?;
            let efforts = picked_models.get(i).and_then(|ms| ms.iter().find(|m| same_model(&c.model, &m.id))).map(|m| m.efforts.clone()).filter(|e| !e.is_empty());
            let efforts = efforts.unwrap_or_else(|| vec![Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
            let current = c.effort;
            Some(
                ui::menu_surface(cx)
                    .w(px(170.))
                    .children(efforts.into_iter().map(|e| {
                        ui::menu_row(SharedString::from(format!("consult-eff-{}", e.as_str())), e == current, cx)
                            .test_support()
                            .child(div().flex_1().child(e.label()))
                            .when(e == current, |el| el.child(Icon::new(IconName::Check).small()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(c) = this.consult.consultants.get_mut(i) {
                                    c.effort = e;
                                }
                                this.consult_effort = None;
                                cx.notify();
                            }))
                    }))
                    .into_any_element(),
            )
        });
        h_flex().items_end().gap(px(6.)).child(main).children(side.or(judges)).into_any_element()
    }

    /// The Consult pill: quiet until models are picked, then their logos and how they'll be asked.
    fn consult_pill(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let open = self.consult_open;
        let empty = self.consult.consultants.is_empty();
        let names = self.consultant_names(cx);
        let tip: SharedString = if empty {
            "Consult other models before answering".into()
        } else {
            let who: Vec<String> = self.consult.consultants.iter().map(|c| names(c)).collect();
            format!("Consulting {}{}", who.join(", "), if self.consult_pinned { " · kept for every message" } else { "" }).into()
        };
        let style = match self.consult.style {
            Style::Advise => "Advise",
            Style::Discuss => "Discuss",
            Style::Arena => "Arena",
        };
        let logos = h_flex().children(self.consult.consultants.iter().take(4).enumerate().map(|(i, c)| {
            div().when(i > 0, |el| el.ml(px(-5.))).p(px(1.)).rounded(px(5.)).bg(theme.secondary).child(ui::agent_logo(&c.agent, px(14.), cx))
        }));
        // An agent that wouldn't get `delegate_task` can't consult: the pill says why, and opens
        // only to let go of consultants already picked.
        let agent = self.workspace.read(cx).prefs_in(&self.scope).agent;
        if let (Some(why), true) = (self.workspace.read(cx).consult_unavailable(&agent), empty) {
            return Pill::new("consult-pill")
                .ghost(true)
                .tooltip(format!("Can't consult: {why}"))
                .child(Icon::new(crate::assets::Lucide::MessagesSquare).small().text_color(muted.opacity(0.5)))
                .when(!compact, |p| p.child(div().text_color(muted.opacity(0.5)).child("Consult")))
                .into_any_element();
        }
        let entity = cx.entity();
        Popover::new("consult-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.consult_open = *open;
                if !*open {
                    this.consult_effort = None;
                    this.consult_judge = false;
                }
                this.sync_overlay(cx);
                cx.notify();
            }))
            .trigger(
                Pill::new("consult-pill")
                    .ghost(empty)
                    .tooltip(tip)
                    .when(empty, |p| p.child(Icon::new(crate::assets::Lucide::MessagesSquare).small().text_color(muted)).when(!compact, |p| p.child(div().text_color(muted).child("Consult"))))
                    .when(!empty, |p| {
                        p.child(logos)
                            .when(!compact, |p| p.child(div().text_color(muted).child(style)))
                            .when(self.consult_pinned, |p| p.child(Icon::new(crate::assets::Lucide::Pin).xsmall().text_color(muted)))
                    }),
            )
            .content(move |_, _, cx| entity.update(cx, |c, cx| c.consult_menu(cx)))
            .into_any_element()
    }

    // ---------- access menu ----------

    fn access_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let current = ws.prefs_in(&self.scope).hand_holding;
        let unlocked = ws.settings.permissions.full_access_unlocked;
        let theme = cx.theme().clone();
        ui::menu_surface(cx)
            .w(px(340.))
            .children(HandHolding::ALL.into_iter().map(|level| {
                let locked = level == HandHolding::FullAccess && !unlocked;
                let tint = if level == HandHolding::FullAccess { palette::amber(cx) } else { theme.muted_foreground };
                ui::menu_row(SharedString::from(format!("hh-{level:?}")), level == current, cx)
                    .items_start()
                    .py(px(9.))
                    .child(div().pt(px(2.)).child(hand_icon(level).small().text_color(tint)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(div().font_medium().child(level.label()))
                            .child(div().text_xs().whitespace_normal().line_height(relative(1.4)).text_color(theme.muted_foreground).child(if locked {
                                "Turn on in Settings → Permissions to use it."
                            } else {
                                level.description()
                            })),
                    )
                    .when(locked, |el| el.opacity(0.55))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if locked {
                            let route = Route::Settings(crate::workspace::SettingsPage::Permissions);
                            let main = this.scope == Scope::Main;
                            this.workspace.update(cx, |ws, cx| if main { ws.navigate(route, cx) } else { ws.show_in_main(route, cx) });
                        } else {
                            this.update_prefs(cx, |p| p.hand_holding = level);
                        }
                        this.access_open = false;
                        this.sync_overlay(cx);
                        cx.notify();
                    }))
            }))
            .id("access-menu-body")
            .test_support()
            .into_any_element()
    }

    // ---------- project + clone ----------

    fn project_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let project = match &ws.route {
            Route::Draft { project } => project.clone(),
            _ => None,
        };
        let label = project.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "No project".into());
        // The project's colour on its folder, as on its badge in the sidebar.
        let tint = project.as_ref().and_then(|p| ws.project_tint_at(p, cx));
        let projects: Vec<(String, std::path::PathBuf)> = ws
            .workspace_projects()
            .into_iter()
            .map(|p| (p.remote.clone().map(|r| format!("{}  ·  {r}", p.name)).unwrap_or_else(|| p.name.clone()), p.path.clone()))
            .collect();
        let ws_entity = self.workspace.clone();
        let composer = cx.entity();
        let theme = cx.theme().clone();
        gpui_kit::component::button::Button::new("project-picker")
            .ghost()
            .small()
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(theme.foreground.opacity(0.9))
                    .child(Icon::new(IconName::Folder).small().when_some(tint, |i, c| i.text_color(c)))
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let ws = ws_entity.clone();
                // A thread of its own, in no project: a folder of its own to work in.
                let mut menu = menu
                    .min_w(px(280.))
                    .max_h(px(360.))
                    .scrollable(true)
                    .item(PopupMenuItem::new("No project").checked(project.is_none()).on_click(move |_, _, cx| {
                        ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx))
                    }))
                    .separator()
                    .label("Projects");
                for (name, path) in projects.clone() {
                    let ws = ws_entity.clone();
                    let checked = project.as_ref() == Some(&path);
                    menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(move |_, _, cx| {
                        let path = path.clone();
                        ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(path) }, cx))
                    }));
                }
                let ws = ws_entity.clone();
                let c = composer.clone();
                menu.separator()
                    .item(PopupMenuItem::new("Open folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))))
                    .item(PopupMenuItem::new("Clone from GitHub…").icon(IconName::Github).on_click(move |_, window, cx| c.update(cx, |c, cx| c.open_clone_dialog(window, cx))))
            })
    }

    /// Where a draft runs: the project folder, or a worktree of its own (git projects). A quiet
    /// menu; the choice is fixed once the thread starts. `blocked`: why a worktree can't start
    /// from the project folder right now (no branch, or no commit, to start from).
    fn place_chip(&self, worktree: bool, blocked: Option<&'static str>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.clone();
        let (icon, label) = if worktree { (crate::assets::Lucide::GitBranchPlus, "New worktree") } else { (crate::assets::Lucide::Laptop, "Local") };
        gpui_kit::component::button::Button::new("env-place")
            .ghost()
            .small()
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(theme.foreground.opacity(0.82))
                    .child(Icon::new(icon).small().text_color(if worktree && blocked.is_some() { palette::amber(cx) } else { theme.muted_foreground }))
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let pick = |label: &'static str, on: bool| {
                    let ws = ws.clone();
                    PopupMenuItem::new(label).checked(worktree == on).on_click(move |_, _, cx| {
                        ws.update(cx, |ws, cx| {
                            let mut p = ws.prefs_in(&Scope::Main);
                            p.worktree = on;
                            ws.set_prefs_in(&Scope::Main, p, cx);
                        })
                    })
                };
                // The items have icons, which would take the check's place on the left.
                let menu = menu
                    .min_w(px(240.))
                    .check_side(gpui_kit::component::Side::Right)
                    .label("New thread runs in")
                    .item(pick("Local: the project folder", false).icon(crate::assets::Lucide::Laptop))
                    .item(pick("New worktree: a branch of its own", true).icon(crate::assets::Lucide::GitBranchPlus).disabled(blocked.is_some() && !worktree));
                match blocked {
                    Some(why) => menu.separator().label(why),
                    None => menu,
                }
            })
            .into_any_element()
    }

    /// Where the agent runs (this Mac: the project folder or the thread's worktree) and on which
    /// branch, with a branch switcher (not for worktrees: their branch is the thread's).
    fn env_chips(&self, with_project: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let git = ws.git_in(&self.scope).cloned();
        let cwd = ws.cwd_in(&self.scope);
        let has_cwd = cwd.is_some();
        let is_draft = ws.is_draft_in(&self.scope);
        let draft_worktree = ws.prefs_in(&self.scope).worktree;
        let worktree = ws.thread_in(&self.scope).and_then(|t| t.worktree.clone());
        let theme = cx.theme().clone();
        let chip = |id: &'static str| {
            h_flex().id(id).h(px(26.)).px(px(8.)).gap(px(6.)).rounded(px(7.)).text_sm().text_color(theme.foreground.opacity(0.82))
        };
        // A worktree needs a branch to start from (and a commit on it). The menu shows for any git
        // project, and whenever the draft is set to a worktree (its git state may not be in yet):
        // what it says is what sending does, and it can always go back to Local.
        let is_repo = git.as_ref().is_some_and(|g| g.is_repo);
        let blocked = match &git {
            Some(g) if g.is_repo && g.branch.is_none() => Some("The project folder isn't on a branch to start from."),
            Some(g) if g.is_repo && g.branches.is_empty() => Some("The repository has no commits to start from."),
            _ => None,
        };
        let place = if is_draft && (is_repo || draft_worktree) {
            self.place_chip(draft_worktree, blocked, cx)
        } else if let Some(wt) = worktree.clone() {
            let tip = format!("Runs in a worktree of its own: {}", trek_core::paths::tildify(&wt.path));
            chip("env-worktree")
                .test_support()
                .child(Icon::new(crate::assets::Lucide::GitBranchPlus).small().text_color(theme.muted_foreground))
                .child("Worktree")
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element()
        } else {
            chip("env-local")
                .child(Icon::new(crate::assets::Lucide::Laptop).small().text_color(theme.muted_foreground))
                .child("Local")
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Runs on this Mac, in the project folder").build(window, cx))
                .into_any_element()
        };
        let branch: Option<AnyElement> = match git {
            // The thread's own branch, off its base.
            _ if worktree.is_some() => worktree.map(|wt| {
                chip("env-branch")
                    .child(Icon::new(crate::assets::Lucide::GitBranch).small().text_color(palette::indigo(cx)))
                    .child(div().text_color(theme.foreground.opacity(0.9)).child(wt.branch.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("off {}", wt.base)))
                    .into_any_element()
            }),
            // A draft headed for a worktree: the branch it starts from, with no switcher (switching
            // would check another branch out in the project folder, not pick a base).
            Some(g) if is_draft && draft_worktree && g.is_repo => Some(
                chip("env-base")
                    .test_support()
                    .child(Icon::new(crate::assets::Lucide::GitBranch).small().text_color(theme.muted_foreground))
                    .when_some(g.branch.clone(), |el, b| el.child(div().text_xs().text_color(theme.muted_foreground).child("off")).child(div().text_color(theme.foreground.opacity(0.9)).child(b)))
                    .when(g.branch.is_none(), |el| el.text_color(palette::amber(cx)).child("detached HEAD"))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("The worktree's branch starts from the branch the project folder is on").build(window, cx))
                    .into_any_element(),
            ),
            Some(g) if g.is_repo => {
                let name = g.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                let on_default = g.on_default();
                let ws_entity = self.workspace.clone();
                let branches = g.branches.clone();
                let current = g.branch.clone();
                let default = g.default_branch.clone();
                let status = match (g.changed, g.ahead) {
                    (0, 0) => None,
                    (c, 0) => Some(format!("{c} changed")),
                    (0, a) => Some(format!("↑{a}")),
                    (c, a) => Some(format!("{c} changed · ↑{a}")),
                };
                Some(
                    gpui_kit::component::button::Button::new("branch-chip")
                        .ghost()
                        .small()
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .child(Icon::new(crate::assets::Lucide::GitBranch).small().text_color(if on_default { theme.muted_foreground } else { palette::indigo(cx) }))
                                .child(div().text_color(theme.foreground.opacity(0.9)).child(name))
                                .when(on_default, |el| {
                                    el.child(div().px(px(5.)).rounded(px(4.)).text_xs().bg(theme.foreground.opacity(0.08)).text_color(theme.muted_foreground).child("default"))
                                })
                                .when_some(status, |el, s| el.child(div().text_xs().text_color(palette::amber(cx)).child(s)))
                                .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
                        )
                        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                            menu = menu.min_w(px(220.)).max_h(px(320.)).scrollable(true).label("Switch branch");
                            for b in branches.clone() {
                                let (ws, cwd) = (ws_entity.clone(), cwd.clone());
                                let label = if Some(&b) == default.as_ref() { format!("{b}  (default)") } else { b.clone() };
                                menu = menu.item(PopupMenuItem::new(label).checked(Some(&b) == current.as_ref()).on_click(move |_, _, cx| {
                                    let b = b.clone();
                                    if let Some(cwd) = cwd.clone() {
                                        ws.update(cx, |ws, cx| ws.switch_branch(cwd, b, cx))
                                    }
                                }));
                            }
                            menu
                        })
                        .into_any_element(),
                )
            }
            Some(_) if has_cwd => Some(
                chip("env-nogit")
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(crate::assets::Lucide::GitBranch).small())
                    .child("Not a git repo")
                    .into_any_element(),
            ),
            _ => None,
        };
        h_flex()
            .gap_1()
            .when(with_project, |el| el.child(self.project_chip(cx)))
            .child(place)
            .children(branch)
            .into_any_element()
    }

    /// The thread's worktree folder is gone: say so, and offer the ways on. Messages wait.
    fn missing_worktree(&self, wt: &trek_core::worktree::Worktree, id: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (ws1, ws2, id2) = (self.workspace.clone(), self.workspace.clone(), id.clone());
        h_flex()
            .id("worktree-missing")
            .test_support()
            .px(px(14.))
            .py(px(8.))
            .gap(px(10.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(Icon::new(IconName::TriangleAlert).small().text_color(palette::amber(cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_medium().child("Worktree missing"))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("The folder for {} is gone. Messages wait until the thread has a folder again.", wt.branch))),
            )
            .child(gpui_kit::component::button::Button::new("wt-recreate").small().outline().label("Recreate from branch").on_click(move |_, _, cx| {
                let id = id.clone();
                ws1.update(cx, |ws, cx| ws.recreate_worktree(&id, cx))
            }))
            .child(gpui_kit::component::button::Button::new("wt-local").small().ghost().label("Run in project folder").on_click(move |_, _, cx| {
                let id = id2.clone();
                ws2.update(cx, |ws, cx| ws.run_in_project_folder(&id, cx))
            }))
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
}

impl Attaching for Composer {
    fn outbox(&mut self) -> &mut Outbox {
        &mut self.outbox
    }

    fn target(&self, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        match (ws.thread_id_in(&self.scope), &ws.route) {
            (Some(id), _) => format!("thread:{id}"),
            (None, Route::Draft { project }) => format!("draft:{}", project.as_deref().map(|p| p.display().to_string()).unwrap_or_default()),
            (None, _) => String::new(),
        }
    }

    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        self.submit(input, window, cx);
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("Composer");
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let thread = ws.thread_in(&self.scope).cloned();
        let is_draft = ws.is_draft_in(&self.scope);
        let main = self.scope == Scope::Main;
        let own_turn = thread.as_ref().is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou));
        // Its turn is over but it waits on sub-agents (Trek's, or its agent's own in the background).
        let children_working = thread.as_ref().is_some_and(|t| !ws.running_children(&t.id).is_empty() || ws.waiting(&t.id));
        let live = thread.as_ref().and_then(|t| ws.live.get(&t.id));
        // The API cost estimate: changes as usage is reported, not per token.
        let billing = thread.as_ref().and_then(|t| ws.billing_of(t));
        let spend = live.and_then(|l| l.spend.clone()).unwrap_or_default();
        let cost_label = crate::cost::label(billing.as_ref(), &spend);
        let billing_tip = match &billing {
            Some(trek_agents::Billing::Plan(plan)) => Some(format!("Included in {}", crate::cost::plan_phrase(plan))),
            Some(trek_agents::Billing::Metered) => Some("Billed per token by your API provider".to_string()),
            Some(trek_agents::Billing::Local) => Some("Runs on this Mac, nothing is billed".to_string()),
            None => None,
        };
        let queued = thread.as_ref().map_or(0, |t| ws.queued(&t.id));
        let models = ws.models_for(&prefs.agent);
        let model_label = prefs
            .model
            .as_deref()
            .map(|m| model_name(&models, m))
            .or_else(|| default_model(&models).map(|m| m.name.clone()))
            .unwrap_or_else(|| prefs.agent.display_name());
        let theme = cx.theme().clone();
        let empty = self.input.read(cx).value().trim().is_empty() && self.outbox.paths.is_empty() && self.outbox.saving == 0;
        // Stop also ends sub-agents still at work after their parent's turn, unless there's a
        // message to send: the parent is free to take it.
        let running = own_turn || (children_working && empty);
        let thread_id = thread.as_ref().map(|t| t.id.clone());
        let context = thread.as_ref().and_then(|t| ws.live.get(&t.id)).and_then(|l| l.context);
        let compact = self.width.get() < px(600.);
        let plan = prefs.plan;
        let preparing = live.is_some_and(|l| l.preparing);
        // Its agent's CLI is being updated: messages wait for the new version.
        let updating = thread.as_ref().filter(|t| ws.agent_updating(&t.agent.key())).map(|t| t.agent.display_name());
        let missing = thread.as_ref().and_then(|t| Some((t.worktree.clone().filter(|w| !preparing && w.is_missing())?, t.id.clone())));
        // Another thread edits the same folder right now: offer a worktree for the next one.
        let crowded = thread.as_ref().filter(|t| ws.sharing_folder(&t.id)).map(|t| ws.project_dir(t));
        // The way out is a worktree, for git projects.
        let crowded_repo = crowded.clone().flatten().filter(|p| p.join(".git").exists());

        // Model pill + menu.
        let model_open = self.model_open;
        let model_pill = Popover::new("model-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(model_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.model_open = *open;
                this.sync_overlay(cx);
                if !*open {
                    this.sub = None;
                    this.rail = None;
                }
                cx.notify();
            }))
            .trigger(
                Pill::new("model-pill")
                    .flexible()
                    .child(ui::agent_glyph(&prefs.agent, cx))
                    .child(div().min_w_0().max_w(px(if compact { 120. } else { 240. })).truncate().child(model_label))
                    .child(div().flex_none().text_color(theme.muted_foreground).child(prefs.effort.label()))
                    .when(prefs.fast, |el| el.child(Icon::new(crate::assets::Lucide::Zap).xsmall().text_color(palette::amber(cx))))
                    .child(Icon::new(if model_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
            );
        let model_menu_entity = cx.entity();
        let model_pill = model_pill.content({
            let c = model_menu_entity.clone();
            move |_, _, cx| c.update(cx, |c, cx| c.model_menu(cx))
        });

        let access_open = self.access_open;
        let access_entity = cx.entity();
        let hh = prefs.hand_holding;
        let access_tint = if hh == HandHolding::FullAccess { palette::amber(cx) } else { theme.muted_foreground };
        let access_pill = Popover::new("access-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(access_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.access_open = *open;
                this.sync_overlay(cx);
                cx.notify();
            }))
            .trigger(
                Pill::new("access-pill")
                    .when(compact, |p| p.tooltip(hh.label()))
                    .child(hand_icon(hh).small().text_color(access_tint))
                    .when(!compact, |p| p.child(hh.label()))
                    .child(Icon::new(if access_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
            )
            .content(move |_, _, cx| access_entity.update(cx, |c, cx| c.access_menu(cx)));

        let square = |id: &'static str| {
            div().id(id).size(px(30.)).flex_none().rounded(px(8.)).flex().items_center().justify_center()
        };
        let send = if running {
            square("stop")
                .test_support()
                .cursor_pointer()
                .bg(palette::red(cx))
                .child(div().size(px(10.)).rounded(px(2.)).bg(rgb(0xFFFFFF)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = thread_id.clone() {
                        this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
                    }
                }))
                .into_any_element()
        } else {
            square("send")
                .test_support()
                .bg(if empty { theme.foreground.opacity(0.08) } else { theme.foreground })
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(Icon::new(IconName::ArrowUp).small().text_color(if empty { theme.muted_foreground } else { theme.background }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let input = this.input.clone();
                    this.submit(input, window, cx);
                }))
                .into_any_element()
        };

        // Record the card's width; crossing the compact threshold re-renders the pills.
        let width_cell = self.width.clone();
        let me = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let before = width_cell.get();
                width_cell.set(bounds.size.width);
                if (before < px(600.)) != (bounds.size.width < px(600.)) {
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // Under liquid glass the card is a frosted pane too, a touch firmer than the panel it sits on.
        let glass = self.workspace.read(cx).glass().is_some();
        let card = v_flex()
            .relative()
            .child(measure)
            .w_full()
            .rounded(px(16.))
            .bg(if glass { theme.secondary.opacity(0.78) } else { theme.secondary })
            .border_1()
            .border_color(if hh == HandHolding::FullAccess { palette::amber(cx).opacity(0.3) } else { theme.input })
            // Context row (threads): checkout, branch, activity.
            .when(!is_draft, |el| {
                el.child(
                    h_flex()
                        .h(px(38.))
                        .px(px(8.))
                        .gap(px(8.))
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(self.env_chips(false, cx))
                        .when(plan, |el| el.child(h_flex().gap(px(6.)).text_color(palette::indigo(cx)).child(Icon::new(crate::assets::Lucide::ListChecks).small()).child("Plan mode")))
                        .when(preparing, |el| {
                            el.child(h_flex().gap(px(6.)).child(gpui_kit::component::spinner::Spinner::new().xsmall().color(theme.muted_foreground)).child("Creating the worktree…"))
                        })
                        .when_some(updating, |el, name| {
                            el.child(
                                h_flex()
                                    .id("composer-agent-updating")
                                    .test_support()
                                    .min_w_0()
                                    .gap(px(6.))
                                    .child(gpui_kit::component::spinner::Spinner::new().xsmall().color(theme.muted_foreground))
                                    .child(div().min_w_0().truncate().child(format!("Updating {name}. Messages go when it's done."))),
                            )
                        })
                        .child(div().flex_1())

                )
            })
            .when_some(missing, |el, (wt, id)| el.child(self.missing_worktree(&wt, id, cx)))
            .children(self.edit_banner(cx))
            .children(self.attachment_strip(cx))
            .child(div().px(px(14.)).pt(px(if is_draft || !self.outbox.paths.is_empty() || self.editing.is_some() { 12. } else { 2. })).child(Textarea::new(&self.input).appearance(false).on_paste({
                let me = cx.entity().downgrade();
                move |item, window, cx| match attachments::pasted(item) {
                    Some(p) => me
                        .update(cx, |this, cx| {
                            let input = this.input.clone();
                            attachments::paste(this, p, &input, window, cx)
                        })
                        .is_ok(),
                    None => false,
                }
            })))
            .child(
                h_flex()
                    .p(px(8.))
                    .gap(px(6.))
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.plus_button(cx))
                    .child(model_pill)
                    .child(self.consult_pill(compact, cx))
                    .when(self.restate, |el| {
                        el.child(
                            Pill::new("restate-pill")
                                .selected(true)
                                .tooltip("The agent says back what you asked, in its own words, before it does anything. Click to turn off.")
                                .child(Icon::new(crate::assets::Lucide::MessageSquareQuote).small().text_color(theme.foreground.opacity(0.85)))
                                .when(!compact, |p| p.child("Restate first"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.restate = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(access_pill)
                    .when(is_draft, |el| {
                        el.child(
                            Pill::new("plan-pill")
                                .selected(plan)
                                .when(compact, |p| p.tooltip(if plan { "Plan mode on" } else { "Plan mode" }))
                                .child(Icon::new(crate::assets::Lucide::ListChecks).small().text_color(if plan { palette::indigo(cx) } else { theme.muted_foreground }))
                                .when(!compact, |p| p.child("Plan"))
                                .on_click(cx.listener(|this, _, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan))),
                        )
                    })
                    .child(div().flex_1().min_w(px(4.)))
                    .when_some(context, |el, (used, window)| el.child(self.context_ring(used, window, cx)))
                    .child(send),
            );

        let limit_bar = thread.as_ref().and_then(|t| self.limit_bar(&t.id, compact, cx));
        // Capy-style context chips above the card on a new thread.
        let chips = is_draft.then(|| div().pb(px(8.)).child(self.env_chips(true, cx)));
        let picker = self.picker(cx);
        let drop_tint = palette::ember(cx);
        let card = div()
            .relative()
            .w_full()
            .child(card)
            .when_some(picker, |el, p| el.child(div().absolute().bottom_full().left_0().w_full().pb(px(6.)).child(p)))
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.opacity(0.85).border_color(drop_tint))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.add_paths(paths.paths(), window, cx)));

        // Status strip under the card (threads).
        let status = (!is_draft).then(|| {
            h_flex()
                .px(px(6.))
                .pt(px(8.))
                .gap_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(
                    h_flex()
                        .id("agent-label")
                        .gap(px(6.))
                        .child(ui::agent_glyph(&prefs.agent, cx))
                        .child(prefs.agent.display_name())
                        // How the session is billed (what it would cost is the estimate beside it).
                        .when_some(billing_tip, |el, tip| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))),
                )
                .when_some(cost_label, |el, label| el.child(cost_chip(label, billing.clone(), spend, cx)))
                .when(queued > 0, |el| el.child(format!("{queued} queued")))
                .when(crowded.is_some(), |el| {
                    let ws = self.workspace.clone();
                    el.child(
                        h_flex()
                            .min_w_0()
                            .gap(px(6.))
                            .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(palette::amber(cx)))
                            .child(div().min_w_0().truncate().child("Another thread is also editing this folder"))
                            .when_some(crowded_repo, |el, project| el.child(
                                div()
                                    .id("next-in-worktree")
                                    .test_support()
                                    .flex_none()
                                    .cursor_pointer()
                                    .text_color(theme.foreground.opacity(0.85))
                                    .hover(|s| s.text_color(theme.foreground).underline())
                                    .child("Use a worktree next time")
                                    .on_click(move |_, _, cx| {
                                        let project = project.clone();
                                        ws.update(cx, |ws, cx| ws.new_thread_in_worktree(project, cx))
                                    }),
                            )),
                    )
                })
                .child(div().flex_1())
                // The tools panel lives in the main window.
                .when(main, |el| {
                    el.child(
                        h_flex()
                            .id("open-terminal")
                            .gap(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child(Icon::new(IconName::SquareTerminal).xsmall())
                            .child("Terminal")
                            .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(PanelTool::Terminal))))),
                    )
                })
        });

        // Record the composer's height for the window's cache; a change re-renders it uncached.
        let height_cell = self.height.clone();
        let me = cx.entity().downgrade();
        let measure_height = canvas(
            move |bounds, _, cx| {
                if height_cell.replace(bounds.size.height) != bounds.size.height {
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        h_flex()
            .relative()
            .w_full()
            .justify_center()
            .px_6()
            .pb(px(if is_draft { 0. } else { 12. }))
            .child(measure_height)
            .key_context("Composer")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            // The textarea binds these keys to actions, which never reach key listeners.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.picker_action("up", window, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.picker_action("down", window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.picker_action("escape", window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.picker_action("tab", window, cx)))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| this.on_enter(action, window, cx)))
            .on_action(cx.listener(|this, _: &TogglePlan, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan)))
            .child(v_flex().w_full().max_w(self.workspace.read(cx).column()).children(chips).children(limit_bar).child(card).children(status))
    }
}

/// The API cost estimate in the status strip, with its breakdown on hover (worked out only then).
fn cost_chip(label: String, billing: Option<trek_agents::Billing>, spend: crate::cost::ThreadSpend, cx: &App) -> AnyElement {
    let hover = cx.theme().foreground;
    // TREK_HOVER_COST=<seconds> hovers the first estimate drawn, once, that long after it's
    // drawn (once the agents have said how they're billed), so its breakdown can be reviewed in
    // a window that isn't in front, without moving the real pointer.
    static HOVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static DELAY: std::sync::LazyLock<Option<std::time::Duration>> =
        std::sync::LazyLock::new(|| std::env::var("TREK_HOVER_COST").ok().map(|s| std::time::Duration::from_secs(s.trim().parse().unwrap_or(0))));
    let delay = *DELAY;
    let review = delay.is_some() && !HOVERED.load(std::sync::atomic::Ordering::Relaxed);
    div()
        .id("cost-estimate")
        .test_support()
        .relative()
        .flex_none()
        .hover(move |s| s.text_color(hover.opacity(0.8)))
        .child(label)
        .when(review, |el| {
            el.child(
                canvas(
                    move |bounds, window, cx| {
                        if !HOVERED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            let position = bounds.center();
                            let after = delay.unwrap_or_default();
                            window
                                .spawn(cx, async move |cx| {
                                    cx.background_executor().timer(after).await;
                                    let _ = cx.update(|window, cx| {
                                        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Default::default() }), cx);
                                    });
                                })
                                .detach();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
        })
        .tooltip(move |window, cx| {
            let tip = crate::cost::breakdown(billing.as_ref(), &spend);
            gpui_kit::component::tooltip::Tooltip::element(move |_, cx| tip.as_ref().map_or_else(|| div().into_any_element(), |b| cost_breakdown(b, cx))).build(window, cx)
        })
        .into_any_element()
}

/// The tooltip: the estimate, how it's billed, each model's tokens by kind at their price, the
/// sub-agents' share, and where the prices come from.
fn cost_breakdown(b: &crate::cost::Breakdown, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let rule = theme.foreground.opacity(0.07);
    let models = b.models.iter().map(|m| {
        v_flex()
            .gap(px(2.))
            .child(h_flex().gap(px(12.)).text_xs().font_medium().child(div().flex_1().child(m.name.clone())).child(m.amount.clone()))
            .children(m.rows.iter().map(|r| {
                h_flex()
                    .gap(px(8.))
                    .text_xs()
                    .text_color(muted)
                    .child(div().w(px(76.)).child(r.kind))
                    .child(div().w(px(44.)).flex().justify_end().child(r.tokens.clone()))
                    .child(div().pl(px(4.)).child(r.rate.clone().map(|r| format!("× {r}")).unwrap_or_else(|| "—".into())))
            }))
    });
    v_flex()
        .id("cost-breakdown")
        .test_support()
        .max_w(px(340.))
        .gap(px(8.))
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_medium().child(b.headline.clone()))
                .when_some(b.billing.clone(), |el, l| el.child(div().text_xs().text_color(muted).child(l))),
        )
        .when(!b.models.is_empty(), |el| el.child(v_flex().gap(px(8.)).pt(px(8.)).border_t_1().border_color(rule).children(models)))
        .when(b.subs.is_some() || b.unpriced.is_some(), |el| {
            el.child(
                v_flex()
                    .gap(px(2.))
                    .text_xs()
                    .when_some(b.subs.clone(), |el, s| el.child(s))
                    .when_some(b.unpriced.clone(), |el, u| el.child(div().text_color(muted).child(u))),
            )
        })
        .when(!b.sources.is_empty(), |el| {
            el.child(v_flex().gap(px(2.)).pt(px(8.)).border_t_1().border_color(rule).text_xs().text_color(muted).children(b.sources.iter().cloned()))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{consult_split, restate_command};

    #[test]
    fn restate_takes_a_message_or_none() {
        assert_eq!(restate_command("/restate"), Some(None));
        assert_eq!(restate_command("  /restate  "), Some(None));
        assert_eq!(restate_command("/restate why does it flicker?"), Some(Some("why does it flicker?".into())));
        assert_eq!(restate_command("/restate: why?"), Some(Some("why?".into())));
        assert_eq!(restate_command("/restated"), None);
        assert_eq!(restate_command("please /restate"), None);
    }

    #[test]
    fn consult_splits_at_a_colon_and_space() {
        assert_eq!(consult_split(" sol high: why?"), Some((" sol high", " why?")));
        assert_eq!(consult_split(" qwen2.5-coder:7b high: why?"), Some((" qwen2.5-coder:7b high", " why?")), "a model id's colon stays");
        assert_eq!(consult_split(" sol:"), Some((" sol", "")));
        assert_eq!(consult_split(" qwen2.5-coder:7b"), None);
        assert_eq!(consult_split(" sol"), None);
    }
}
