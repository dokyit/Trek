//! The composer (MonoCode-style): a card with context on top, the prompt, then pills for
//! model (Fast · Effort › · Model ›) and access level, plus attach and send.
//! On a new thread it floats over the background hero with Capy-style context chips above it.

use crate::TogglePlan;
use crate::attachments::{self, Attaching, Outbox};
use crate::dictate;
use crate::mentions::{self, PickKind};
use crate::palette;
use crate::screenclip;
use crate::ui::{self, Pill};
use crate::workspace::{Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea,
    TextareaState,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::catalog::ModelInfo;
use trek_core::orchestrate::{Consult, Consultant, Style};
use trek_core::settings::FollowUp;
use trek_core::{AgentId, Effort, HandHolding, RunState};

pub(crate) mod look;
mod machine_chip;
pub use machine_chip::reach_dot;
pub mod pickers;
pub mod voice;

pub use pickers::{PickerHost, Pickers};

pub struct Composer {
    workspace: Entity<Workspace>,
    /// The main window's composer follows its route; a thread window's is bound to one thread.
    scope: Scope,
    input: Entity<TextareaState>,
    clone_input: Entity<InputState>,
    /// The model and access menus, and the `/`, `@` and `$` picker (shared with the editor's AI
    /// input, `pickers`).
    pickers: Pickers,
    /// Images going out with the next message.
    outbox: Outbox,
    snapshotting: bool,
    /// Windows: the wait for what Snipping Tool puts on the clipboard (`snapshot_by_screenclip`).
    /// It ends with the composer, and with `window` (a composer can outlive its window).
    screenclip: Option<Task<()>>,
    window: AnyWindowHandle,
    /// `general.send_with_cmd_enter`: ↩ inserts a newline and ⌘↩ sends (else ↩ sends, ⇧↩ newline).
    cmd_enter: bool,
    /// Width of the composer card at last layout; narrow cards get compact pills.
    width: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// Height of the whole composer at last layout. The window caches this view at that height
    /// (cached views need a definite size) on frames where the composer didn't change.
    height: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// The message being edited, to send again in its place.
    editing: Option<Editing>,
    /// Redraws the usage-limit bar's countdown while it's up.
    limit_tick: Option<Task<()>>,
    /// What the context card's Compact now is doing.
    compact: crate::context_popover::Compact,
    /// The context card is open (from its ring, or from the title bar's hub).
    context_open: bool,
    /// Counts compact runs, so each one's bar animation is its own.
    compact_gen: u64,
    /// Watches the context after Compact now, to say what it freed.
    _compact_watch: Option<Task<()>>,
    /// Redraws when the PR/release chip's linger ends, so a settled chip goes on time.
    _chip_expiry: Option<(i64, Task<()>)>,
    /// The chip the user moved on from by sending a message (its card's row and phase): a
    /// settled chip goes then; a later change to that card brings it back.
    chip_dismissed: Option<(String, crate::pr_card::Phase)>,
    /// When the chip took its current state, for its fade in (row, phase, then).
    chip_since: Option<(String, crate::pr_card::Phase, std::time::Instant)>,
    /// Create PR was clicked and the turn it started is still going.
    creating_pr: bool,
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
    /// Voice mode (`voice`): the toolbar row while dictating, the text heard so far, the levels.
    voice: voice::Voice,
    /// The take in flight: the mic and the Speech task.
    dictation: Option<dictate::Dictation>,
    /// While a take is open: what reads its results, the waveform's 20 fps tick, the wait after ✓.
    voice_tasks: Vec<Task<()>>,
    /// Holds the keyboard while the waveform is up (esc is ✕, ↩ is ✓).
    voice_focus: FocusHandle,
    /// Where the text on screen belongs (`Attaching::target`): a thread, or a new one in a project.
    draft_key: String,
    /// The prompt's placeholder as last set (`placeholder`).
    placeholder: std::borrow::Cow<'static, str>,
    /// What was typed for the threads and drafts not on screen, by `draft_key`.
    drafts: std::collections::HashMap<String, Draft>,
    _subscriptions: Vec<Subscription>,
}

/// A message in progress for a thread or draft the composer isn't showing.
struct Draft {
    text: String,
    cursor: usize,
    images: Vec<PathBuf>,
    restate: bool,
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
pub fn model_name(models: &[ModelInfo], id: &str) -> String {
    models
        .iter()
        .find(|i| same_model(id, &i.id))
        .map(|i| i.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// The agent's default when the thread hasn't picked one: Opus 5.5 for Claude, the first live model otherwise.
pub fn default_model(models: &[ModelInfo]) -> Option<&ModelInfo> {
    models
        .iter()
        .find(|m| m.id == "claude-opus-5-5")
        .or_else(|| models.first())
}

/// `/consult`'s models and message, split at the first colon followed by a space (or ending
/// it): model ids may have colons of their own (`qwen2.5-coder:7b`).
pub fn consult_split(rest: &str) -> Option<(&str, &str)> {
    let at = rest
        .char_indices()
        .find(|&(i, c)| c == ':' && rest[i + 1..].chars().next().is_none_or(char::is_whitespace))?
        .0;
    Some((&rest[..at], &rest[at + 1..]))
}

/// `/restate` alone (`Some(None)`), or with the message to send (`Some(Some(message))`); `None`
/// when `text` isn't the command.
pub fn restate_command(text: &str) -> Option<Option<String>> {
    let rest = text.trim_start().strip_prefix("/restate")?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let rest = rest.trim();
    Some((!rest.is_empty()).then(|| rest.to_string()))
}

/// A consultant on `model` of `agent`, at `effort` (High, unless asked) within what the model takes.
pub fn consultant(agent: AgentId, model: &ModelInfo, effort: Option<Effort>) -> Consultant {
    let effort = effort.unwrap_or(Effort::High);
    let effort = if model.efforts.is_empty() {
        effort
    } else {
        effort.clamp_to(&model.efforts)
    };
    Consultant {
        agent,
        model: model.id.clone(),
        effort,
    }
}

/// The model `query` names among the agents the user can pick: by id or name, whole or in part
/// ("sol", "opus 5.5", "gpt-5.6-luna"), optionally after its agent ("codex sol").
pub fn find_model(ws: &crate::workspace::Workspace, query: &str) -> Option<(AgentId, ModelInfo)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let all: Vec<(AgentId, ModelInfo)> = ws
        .ready_agents()
        .into_iter()
        .flat_map(|a| ws.models_for(&a).into_iter().map(move |m| (a.clone(), m)))
        .collect();
    let named = |(a, m): &&(AgentId, ModelInfo)| {
        let agent = a.display_name().to_lowercase();
        [
            m.id.to_lowercase(),
            m.name.to_lowercase(),
            format!("{agent} {}", m.name.to_lowercase()),
            format!("{} {}", a.key(), m.id.to_lowercase()),
        ]
        .contains(&q)
    };
    all.iter()
        .find(named)
        .or_else(|| {
            all.iter().find(|(_, m)| {
                m.name.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q)
            })
        })
        .cloned()
}

/// Every model on offer for an arena, each agent's default first, then its others, smartest
/// first.
pub fn arena_options(ws: &crate::workspace::Workspace) -> Vec<(AgentId, ModelInfo)> {
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

pub fn hand_icon(level: HandHolding) -> Icon {
    match level {
        HandHolding::Supervised => Icon::new(crate::assets::Lucide::Lock),
        HandHolding::AutoAcceptEdits => Icon::new(crate::assets::Lucide::FilePen),
        HandHolding::Auto => Icon::new(crate::assets::Lucide::Sparkle),
        HandHolding::FullAccess => Icon::new(crate::assets::Lucide::ShieldCheck),
    }
}

impl Composer {
    pub fn new(
        workspace: Entity<Workspace>,
        scope: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cmd_enter = workspace.read(cx).settings.general.send_with_cmd_enter;
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .submit_on_enter(!cmd_enter)
                .placeholder(placeholder(None).to_string())
        });
        let pickers = Pickers::new(window, cx);
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift, secondary } = event {
                        let send = if this.cmd_enter { *secondary } else { !*shift };
                        let swallowed = this.pickers.swallowed();
                        // A picker with no rows (`echo $HOME`, `thanks @bob`) has nothing to pick: send.
                        let picking = this.pickers.trigger.is_some()
                            && !pickers::picker_items(this, cx).is_empty();
                        if send && !picking && !swallowed {
                            this.submit(state.clone(), window, cx);
                        }
                    } else if matches!(event, InputEvent::Change) {
                        pickers::update_trigger(this, cx);
                        // The user has started typing: get the agent process up before they hit Return.
                        // Not while editing: sending that starts a session of its own, after the rewind.
                        let typing = {
                            let v = state.read(cx).value();
                            v.trim().len() >= 2 && !v.starts_with('/')
                        } && this.editing.is_none();
                        if typing {
                            let scope = this.scope.clone();
                            this.workspace
                                .update(cx, |ws, cx| ws.warm_up_in(&scope, cx));
                        }
                        cx.notify();
                    }
                },
            ),
            cx.observe_in(&workspace, window, |this, ws, window, cx| {
                let cmd_enter = ws.read(cx).settings.general.send_with_cmd_enter;
                if cmd_enter != this.cmd_enter {
                    this.cmd_enter = cmd_enter;
                    this.input
                        .update(cx, |s, cx| s.set_submit_on_enter(!cmd_enter, cx));
                }
                // The message being edited is gone from view (another thread, or a rewind took it).
                let stale = this.editing.as_ref().is_some_and(|e| {
                    let ws = ws.read(cx);
                    ws.thread_id_in(&this.scope) != Some(e.thread.as_str())
                        || ws
                            .live
                            .get(&e.thread)
                            .is_none_or(|l| l.items.position(&e.item).is_none())
                });
                if stale {
                    this.cancel_edit(window, cx);
                }
                // Another thread or draft on screen: what was typed stays with the one it was typed for.
                this.follow_route(window, cx);
                // Mid-turn the prompt says where a message goes.
                let placeholder = {
                    let ws = ws.read(cx);
                    let running = ws
                        .thread_id_in(&this.scope)
                        .is_some_and(|id| ws.turn_running(id));
                    placeholder(running.then_some(ws.settings.general.follow_up))
                };
                if this.placeholder != placeholder {
                    this.input.update(cx, |s, cx| {
                        s.set_placeholder(placeholder.to_string(), window, cx)
                    });
                    this.placeholder = placeholder;
                }
                // An open @ picker follows the folder on screen.
                if this
                    .pickers
                    .trigger
                    .as_ref()
                    .is_some_and(|t| t.kind == PickKind::Mention)
                {
                    pickers::ensure_file_index(this, cx);
                }
                cx.notify()
            }),
            cx.observe(pickers.model_search(), |_, _, cx| cx.notify()),
            // The cursor blinks and moves without an input event; this view is cached, so redraw.
            cx.observe(&input, |_, _, cx| cx.notify()),
            // A take ends when Trek goes to the background (what was heard stays, with a toast).
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.voice_step(voice::Event::Background, window, cx);
                }
            }),
        ];
        let mut this = Self {
            workspace,
            scope,
            input,
            clone_input,
            pickers,
            outbox: Outbox::default(),
            snapshotting: false,
            screenclip: None,
            window: window.window_handle(),
            cmd_enter,
            width: std::rc::Rc::new(std::cell::Cell::new(px(760.))),
            height: std::rc::Rc::new(std::cell::Cell::new(px(0.))),
            editing: None,
            limit_tick: None,
            compact: crate::context_popover::Compact::Idle,
            context_open: false,
            compact_gen: 0,
            _compact_watch: None,
            _chip_expiry: None,
            chip_dismissed: None,
            chip_since: None,
            creating_pr: false,
            consult: Consult {
                implement: true,
                ..Default::default()
            },
            consult_pinned: false,
            consult_open: false,
            consult_rail: None,
            consult_effort: None,
            consult_judge: false,
            restate: false,
            voice: voice::Voice::default(),
            dictation: None,
            voice_tasks: Vec::new(),
            voice_focus: cx.focus_handle(),
            draft_key: String::new(),
            placeholder: placeholder(None),
            drafts: Default::default(),
            _subscriptions: subscriptions,
        };
        this.draft_key = this.target(cx);
        // TREK_REVIEW_COMPOSER=restate (Restate first on) or arena (the Consult menu open on an
        // arena): states a click reaches, for design review of a window that isn't in front.
        match std::env::var("TREK_REVIEW_COMPOSER").as_deref() {
            Ok("restate") if this.scope == Scope::Main => this.restate = true,
            // Once the agents on this Mac have been found, so every family can take part.
            Ok("arena") if this.scope == Scope::Main => cx
                .spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(3))
                        .await;
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

    fn submit(
        &mut self,
        state: Entity<TextareaState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_settled_chip(cx);
        self.submit_as(state, None, window, cx);
    }

    /// Sending a message moves on from a chip whose work is done (or failed): it goes now.
    fn dismiss_settled_chip(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let settled = ws
            .thread_id_in(&self.scope)
            .and_then(|id| ws.pr_chip_keyed(id))
            .filter(|(_, card)| card.phase.terminal())
            .map(|(key, card)| (key, card.phase));
        if settled.is_some() {
            self.chip_dismissed = settled;
        }
    }

    /// Send what's typed. `follow`: how it goes if a turn is running (`None`: as Settings ›
    /// General has it; ⌥↩ sends it the other way).
    fn submit_as(
        &mut self,
        state: Entity<TextareaState>,
        follow: Option<FollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                self.pickers.trigger = None;
                self.consult_open = true;
                self.sync_overlay(cx);
                cx.notify();
                return;
            }
            Some(Err(why)) => {
                self.workspace.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: why,
                        undo: None,
                    })
                });
                return;
            }
            None => text,
        };
        // `/restate <message>` sends it asking for a restatement first. `/restate` alone, in a
        // thread under way, asks the agent to restate the thread so far; in a new thread it turns
        // that on (or off) for the first message.
        let under_way =
            self.editing.is_none() && self.workspace.read(cx).thread_in(&self.scope).is_some();
        let text = match restate_command(&text) {
            Some(None) if under_way => {
                self.restate = true;
                trek_core::restate::THREAD.to_string()
            }
            Some(None) => {
                state.update(cx, |s, cx| s.set_value("", window, cx));
                self.pickers.trigger = None;
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
        let restating =
            self.restate && !text.trim().is_empty() && !text.trim_start().starts_with('/');
        let consulting = !(self.consult.consultants.is_empty()
            || text.trim().is_empty()
            || text.trim_start().starts_with('/'));
        if consulting {
            let agent = self.workspace.read(cx).prefs_in(&self.scope).agent;
            if let Some(why) = self.workspace.read(cx).consult_unavailable(&agent) {
                self.workspace.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: format!("Can't consult: {why}."),
                        undo: None,
                    })
                });
                return;
            }
        }
        let text = if restating {
            trek_core::restate::with_restate(&text)
        } else {
            text
        };
        // An arena names its judge: the one picked, else Trek's pick.
        let mut consult = self.consult.clone();
        if consulting && consult.style == Style::Arena && consult.judge.is_none() {
            consult.judge = self.arena_judge(cx);
        }
        if let Some(why) = trek_core::orchestrate::arena_problem(&consult).filter(|_| consulting) {
            self.workspace.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message: format!("{why}."),
                    undo: None,
                })
            });
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
                self.workspace.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: message.into(),
                        undo: None,
                    })
                });
                return;
            }
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        self.pickers.trigger = None;
        self.sync_overlay(cx);
        let images = std::mem::take(&mut self.outbox.paths);
        let scope = self.scope.clone();
        let sent = match self.editing.take() {
            // Sent in place of the message: the conversation goes back to just before it. What
            // the composer held before the edit comes back.
            Some(e) => {
                let restore = e.restore == Some(true)
                    && !matches!(&e.files, EditFiles::Changes(c) if c.is_empty());
                let sent = self.workspace.update(cx, |ws, cx| {
                    ws.edit_and_resend(&e.thread, &e.item, text, images.clone(), restore, cx)
                });
                let (text, images) = if sent {
                    e.draft.clone()
                } else {
                    (plain, images)
                };
                self.outbox.paths = images;
                state.update(cx, |s, cx| s.set_value(text, window, cx));
                if !sent {
                    self.editing = Some(e);
                }
                sent
            }
            None => {
                self.workspace.update(cx, |ws, cx| {
                    let running = ws
                        .thread_id_in(&scope)
                        .map(str::to_string)
                        .filter(|id| ws.turn_running(id));
                    match (follow, running) {
                        (Some(follow), Some(id)) => ws.send_as(&id, text, images, Some(follow), cx),
                        _ => ws.send_in(&scope, text, images, cx),
                    }
                });
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
    pub fn compose(
        &mut self,
        thread: &str,
        text: &str,
        images: &[PathBuf],
        edit: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_route(window, cx);
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
            None => (
                self.input.read(cx).value().to_string(),
                std::mem::take(&mut self.outbox.paths),
            ),
        };
        let ws = self.workspace.read(cx);
        let checkpoint = ws.rewind_files(thread, &item);
        let why_not = ws
            .no_checkpoint(thread, &item)
            .unwrap_or(crate::workspace::NoCheckpoint::Missing)
            .explain();
        // Which files sending it would put back (those its turns changed), found off the main thread.
        let check = checkpoint.as_ref().map(|_| {
            let files = ws.rewind_files(thread, &item);
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { files.map_or(Ok(vec![]), |f| f.changes()) })
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
        self.pickers.trigger = None;
        self.sync_overlay(cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Swap drafts if the thread or draft on screen has changed since the text was typed.
    fn follow_route(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.target(cx);
        if key != self.draft_key {
            self.switch_draft(key, window, cx);
        }
    }

    /// Put away what's typed for the thread or draft that was on screen and bring back what was
    /// typed for `key`'s (nothing, if nothing was).
    fn switch_draft(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        // A message being edited belongs to its thread: leaving it puts the draft it replaced back first.
        if self
            .editing
            .as_ref()
            .is_some_and(|e| format!("thread:{}", e.thread) != key)
        {
            self.cancel_edit(window, cx);
        }
        // A take in progress ends unconfirmed: its words don't follow into a draft put away.
        if self.voice.phase != voice::Phase::Idle {
            self.voice_step(voice::Event::Cancel, window, cx);
        }
        let text = self.input.read(cx).value().to_string();
        let cursor = self.input.read(cx).cursor();
        let images = std::mem::take(&mut self.outbox.paths);
        let restate = std::mem::take(&mut self.restate);
        let old = std::mem::replace(&mut self.draft_key, key.clone());
        if !text.is_empty() || !images.is_empty() || restate {
            self.drafts.insert(
                old,
                Draft {
                    text,
                    cursor,
                    images,
                    restate,
                },
            );
        } else {
            self.drafts.remove(&old);
        }
        let next = self.drafts.remove(&key).unwrap_or(Draft {
            text: String::new(),
            cursor: 0,
            images: Vec::new(),
            restate: false,
        });
        self.outbox.paths = next.images;
        self.restate = next.restate;
        self.pickers.trigger = None;
        let cursor = next.cursor.min(next.text.len());
        self.input.update(cx, |s, cx| {
            s.set_value(next.text, window, cx);
            s.set_selected_range(cursor..cursor, cx);
        });
        self.sync_overlay(cx);
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
        let tooltip = |text: String| {
            move |window: &mut Window, cx: &mut App| {
                gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
            }
        };
        let restore = match (e.restore, &e.files) {
            (Some(_), EditFiles::Changes(changes)) if changes.is_empty() => {
                ui::check_row("edit-restore", "Restore files", false, true, cx).tooltip(tooltip(
                    "The files are as they were when this message was first sent.".into(),
                ))
            }
            (Some(on), files) => {
                // What sending it puts back, before it's sent: restoring reverts edits made since
                // (yours too) and removes files created since.
                let (label, tip) = match files {
                    EditFiles::Checking => (
                        "Restore files".to_string(),
                        "Checking which files changed…".to_string(),
                    ),
                    EditFiles::Failed(why) => (
                        "Restore files".to_string(),
                        format!("Couldn't check the files: {why}"),
                    ),
                    EditFiles::Changes(changes) => {
                        let mut tip = String::from(
                            "Put the files back as they were when this message was first sent:",
                        );
                        for f in changes
                            .iter()
                            .filter(|f| {
                                !matches!(
                                    f.change,
                                    trek_core::checkpoint::Change::Nested
                                        | trek_core::checkpoint::Change::Kept
                                )
                            })
                            .take(FILES_LISTED)
                        {
                            let what = match f.change {
                                trek_core::checkpoint::Change::Added => "delete",
                                trek_core::checkpoint::Change::Deleted => "bring back",
                                _ => "revert",
                            };
                            tip.push_str(&format!("\n{what} {}", f.path));
                        }
                        let n = changes
                            .iter()
                            .filter(|f| {
                                !matches!(
                                    f.change,
                                    trek_core::checkpoint::Change::Nested
                                        | trek_core::checkpoint::Change::Kept
                                )
                            })
                            .count();
                        if n > FILES_LISTED {
                            tip.push_str(&format!("\nand {} more", n - FILES_LISTED));
                        }
                        (
                            format!("Restore {n} file{}", if n == 1 { "" } else { "s" }),
                            tip,
                        )
                    }
                };
                ui::check_row("edit-restore", label, on, false, cx)
                    .tooltip(tooltip(tip))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(e) = this.editing.as_mut() {
                            e.restore = e.restore.map(|on| !on);
                        }
                        cx.notify();
                    }))
            }
            (None, _) => ui::check_row("edit-restore", "Restore files", false, true, cx)
                .tooltip(tooltip(e.why_not.clone())),
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
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("Editing message · Esc to cancel"),
                )
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

    /// Open the context card over its ring (the title bar's hub asks for it). The ring shows
    /// while the card is open, however little of the window is used.
    pub fn open_context_card(&mut self, cx: &mut Context<Self>) {
        if !self.context_open {
            self.context_card_toggled(true, cx);
        }
    }

    /// The context card opened or closed.
    fn context_card_toggled(&mut self, open: bool, cx: &mut Context<Self>) {
        self.context_open = open;
        // Closed after a compaction finished: its result has been seen, and the ring
        // goes back to showing only past `QUIET`.
        if !open
            && matches!(
                self.compact,
                crate::context_popover::Compact::Freed { .. }
                    | crate::context_popover::Compact::Unchanged { .. }
            )
        {
            self.compact = crate::context_popover::Compact::Idle;
        }
        if open {
            // A compaction reported before the context moved on is old news.
            let used = self
                .workspace
                .read(cx)
                .thread_id_in(&self.scope)
                .and_then(|id| self.workspace.read(cx).live.get(id).and_then(|l| l.context))
                .map(|c| c.0);
            let before = match self.compact {
                crate::context_popover::Compact::Freed { before, .. }
                | crate::context_popover::Compact::Unchanged { before } => Some(before),
                _ => None,
            };
            if before.is_some_and(|before| used.is_some_and(|used| used > before)) {
                self.compact = crate::context_popover::Compact::Idle;
            }
            // The plan's limits, read without asking the agent anything, so the card has
            // them even when the Usage card doesn't show this agent.
            let agent = self
                .workspace
                .read(cx)
                .thread_in(&self.scope)
                .map(|t| t.agent.clone());
            if let Some(agent) = agent {
                self.workspace
                    .update(cx, |ws, cx| ws.read_usage_for(agent, cx));
            }
        }
        cx.notify();
    }

    /// The context ring, and the card it opens: what fills the window, the plan's limits, and
    /// Compact now.
    fn context_card(&mut self, used: u64, window: u64, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity().downgrade();
        Popover::new("context-card")
            // Above the ring, right-aligned with it.
            .anchor(Anchor::BottomRight)
            .offset(px(8.))
            .appearance(false)
            .open(self.context_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.context_card_toggled(*open, cx);
            }))
            .trigger(crate::context_popover::RingTrigger::new(
                used,
                window,
                // A turn of `/compact` shows on the ring as a folding arrow.
                matches!(
                    self.compact,
                    crate::context_popover::Compact::Running { .. }
                ),
            ))
            // Gathered only while the card is open: the transcript is walked to estimate its
            // share, which is no work to do on every keystroke behind a closed card.
            .content(move |_, win, cx| match me.upgrade() {
                Some(composer) => {
                    let card = {
                        let composer = composer.read(cx);
                        let ws = composer.workspace.read(cx);
                        let (used, window) = ws
                            .thread_id_in(&composer.scope)
                            .and_then(|id| ws.live.get(id))
                            .and_then(|l| l.context)
                            .unwrap_or((used, window));
                        crate::context_popover::Card::of(
                            ws,
                            &composer.scope,
                            ws.thread_in(&composer.scope),
                            used,
                            window,
                            composer.compact.clone(),
                        )
                    };
                    card.render(me.clone(), win, cx)
                }
                None => div().into_any_element(),
            })
            .into_any_element()
    }

    /// A finished PR/release chip lingers a few seconds, then fades (`pr_card::chip_until`);
    /// nothing else may redraw the composer then, so it asks for a redraw when the fade starts.
    fn redraw_when_chip_goes(&mut self, cx: &mut Context<Self>) {
        let until = {
            let ws = self.workspace.read(cx);
            ws.thread_id_in(&self.scope)
                .and_then(|id| ws.pr_chip(id))
                .and_then(|card| crate::pr_card::chip_until(&card))
                // From the start of the fade, which then asks for its own frames.
                .map(|until| until - crate::pr_card::Phase::CHIP_FADE_MS)
        };
        let now = crate::pr_card::now_ms();
        let Some(until) = until.filter(|until| *until > now) else {
            self._chip_expiry = None;
            return;
        };
        if self
            ._chip_expiry
            .as_ref()
            .is_some_and(|(at, _)| *at == until)
        {
            return;
        }
        let wait = std::time::Duration::from_millis((until - now) as u64 + 50);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        });
        self._chip_expiry = Some((until, task));
    }

    /// The row the reference floats over the composer's top right: the changes chip
    /// ("4 files +128 −36 ■■■■□"), then Create PR (a dark action pill, "◌ Creating PR" once
    /// clicked) or the status chip of the newest PR, run or release card ("● #128 3 conflicts").
    /// A finished chip holds a few seconds and fades (`pr_card::chip_until`); a failure stays
    /// until the next message is sent.
    fn chip_row(
        &mut self,
        running: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::pr_card::{self, Kind, Phase};
        let (id, keyed, turn_end, files, can_open_pr) = {
            let ws = self.workspace.read(cx);
            let id = ws.thread_id_in(&self.scope)?.to_string();
            let keyed = ws.pr_chip_keyed(&id);
            // What the agent last changed: the newest turn that changed files (its "Edited N
            // files" card), as the reference's "4 files +128 −36".
            let turn = ws.live.get(&id).and_then(|live| {
                live.items
                    .iter()
                    .enumerate()
                    .rev()
                    .filter(|(_, item)| matches!(item, trek_core::store::Item::TurnEnd { .. }))
                    .find_map(|(ix, _)| {
                        let changes = ws.turn_changes(&id, ix).filter(|c| !c.files.is_empty())?;
                        Some((live.items.id_at(ix)?.to_string(), changes))
                    })
            });
            let (turn_end, files): (Option<String>, Vec<(u32, u32)>) = match turn {
                Some((end, c)) => (
                    Some(end),
                    c.files.iter().map(|f| (f.added, f.removed)).collect(),
                ),
                None => (None, vec![]),
            };
            let can_open_pr = ws.git_in(&self.scope).is_some_and(|g| {
                g.is_repo && g.branch.is_some() && !g.on_default() && (g.changed > 0 || g.ahead > 0)
            });
            (id, keyed, turn_end, files, can_open_pr)
        };
        let now = pr_card::now_ms();
        let has_pr = keyed.as_ref().is_some_and(|(_, c)| c.kind == Kind::Pr);
        if self.creating_pr && (has_pr || !running) {
            self.creating_pr = false;
        }
        // The status chip, unless it's done and gone, or moved on from.
        let status = keyed.filter(|(key, card)| {
            pr_card::chip_shown(card, now)
                && self.chip_dismissed.as_ref() != Some(&(key.clone(), card.phase))
        });
        let creating = self.creating_pr
            || status
                .as_ref()
                .is_some_and(|(_, c)| c.kind == Kind::Pr && c.number.is_none());

        // Fades: in as a state arrives (180 ms), out at the end of a finished chip's linger.
        let mut moving = false;
        let opacity = status.as_ref().map_or(1., |(key, card)| {
            let fresh = self
                .chip_since
                .as_ref()
                .is_none_or(|(k, p, _)| k != key || *p != card.phase);
            if fresh {
                self.chip_since = Some((key.clone(), card.phase, std::time::Instant::now()));
            }
            let since = self
                .chip_since
                .as_ref()
                .map_or(0., |(.., at)| at.elapsed().as_secs_f32() / 0.18);
            let left = pr_card::chip_until(card).map_or(1., |until| {
                ((until - now) as f32 / Phase::CHIP_FADE_MS as f32).clamp(0., 1.)
            });
            let o = since.min(1.).min(left);
            moving = o < 1.;
            o
        });
        if moving && !cx.reduce_motion() {
            window.request_animation_frame();
        }
        let merged = status
            .as_ref()
            .is_some_and(|(_, c)| c.phase == Phase::Merged);

        let changes = (!files.is_empty() && !merged).then(|| {
            let (added, removed) = files
                .iter()
                .fold((0u32, 0u32), |(a, r), (fa, fr)| (a + fa, r + fr));
            // Its caret opens that turn's diff, as the card's Review does.
            let ws = self.workspace.clone();
            let (thread, end) = (id.clone(), turn_end.clone().unwrap_or_default());
            changes_chip(
                files.len(),
                added,
                removed,
                move |_, _, cx| {
                    let (thread, end) = (thread.clone(), end.clone());
                    ws.update(cx, |_, cx| {
                        cx.emit(crate::workspace::WorkspaceEvent::ShowTurnDiff {
                            thread,
                            end,
                            path: None,
                        })
                    });
                },
                cx,
            )
        });
        let action: Option<AnyElement> = if creating {
            Some(dark_chip("create-pr", true, "Creating PR", None, cx))
        } else if let Some((_, card)) = status.as_ref() {
            Some(pr_card::status_chip(card, opacity, cx))
        } else if can_open_pr && !has_pr && !running {
            let ws = self.workspace.clone();
            let me = cx.entity().downgrade();
            Some(dark_chip(
                "create-pr",
                false,
                "Create PR",
                Some(Box::new(move |_, _, cx| {
                    let id = id.clone();
                    let _ = me.update(cx, |this, cx| {
                        this.creating_pr = true;
                        cx.notify();
                    });
                    ws.update(cx, |ws, cx| {
                        ws.send_to(&id, "Create PR".to_string(), vec![], cx)
                    });
                })),
                cx,
            ))
        } else {
            None
        };
        if changes.is_none() && action.is_none() {
            return None;
        }
        Some(
            h_flex()
                .id("composer-chips")
                .test_support()
                .absolute()
                .bottom_full()
                .right_0()
                .pb(px(8.))
                .gap(px(6.))
                .items_center()
                .children(changes)
                .children(action)
                .into_any_element(),
        )
    }

    /// Compact now: send the agent's own `/compact`, then watch its context reports for what it
    /// freed. The card says what happened; nothing is assumed when the agent says nothing.
    pub(crate) fn compact_now(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.compact,
            crate::context_popover::Compact::Running { .. }
        ) {
            return;
        }
        let (id, before) = {
            let ws = self.workspace.read(cx);
            let Some(id) = ws.thread_id_in(&self.scope).map(str::to_string) else {
                return;
            };
            if ws.turn_running(&id) || ws.pause(&id).is_some() {
                return;
            }
            let Some(before) = ws.live.get(&id).and_then(|l| l.context).map(|c| c.0) else {
                return;
            };
            (id, before)
        };
        self.workspace.update(cx, |ws, cx| {
            ws.send_to(&id, "/compact".to_string(), vec![], cx)
        });
        self.compact_gen += 1;
        self.compact = crate::context_popover::Compact::Running {
            generation: self.compact_gen,
            before,
        };
        cx.notify();
        self._compact_watch = Some(cx.spawn(async move |this, cx| {
            let mut saw_turn = false;
            // Five minutes: longer than a compaction takes, short enough to stop watching.
            for _ in 0..600 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        let (used, running) = {
                            let ws = this.workspace.read(cx);
                            let used = ws.live.get(&id).and_then(|l| l.context).map(|c| c.0);
                            (used, ws.turn_running(&id))
                        };
                        match used {
                            Some(used) if used < before => {
                                this.compact = crate::context_popover::Compact::Freed {
                                    tokens: before - used,
                                    before,
                                };
                                cx.notify();
                                true
                            }
                            // Its turn is over and the context only grew: the agent answered
                            // rather than compacted.
                            Some(used) if !running && used > before => {
                                this.compact =
                                    crate::context_popover::Compact::Unchanged { before };
                                cx.notify();
                                true
                            }
                            _ if saw_turn && !running => {
                                this.compact =
                                    crate::context_popover::Compact::Unchanged { before };
                                cx.notify();
                                true
                            }
                            _ => {
                                saw_turn |= running;
                                false
                            }
                        }
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if matches!(
                    this.compact,
                    crate::context_popover::Compact::Running { .. }
                ) {
                    this.compact = crate::context_popover::Compact::Unchanged { before };
                    cx.notify();
                }
            });
        }));
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
        let snoozed = ws
            .thread(id)
            .and_then(|t| t.snoozed_until)
            .is_some_and(|u| u > now);
        // The countdown moves on its own: redraw this view (not the window) twice a minute.
        if self.limit_tick.is_none() {
            self.limit_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(30))
                        .await;
                    let paused = this.update(cx, |this, cx| {
                        let ws = this.workspace.read(cx);
                        let paused = ws
                            .thread_id_in(&this.scope)
                            .is_some_and(|id| ws.pause(id).is_some());
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
                }
            }));
        }
        let theme = cx.theme().clone();
        let amber = palette::amber(cx);
        let when = |at: i64| {
            let clock = crate::time::reset_clock(at, now);
            if compact {
                clock
            } else {
                format!("{clock} (in {})", crate::time::countdown(at, now))
            }
        };
        // Stopped ahead of the limit, as Trek asked, rather than by it.
        let title = if pause.wrapped {
            "Wrapped up before the limit"
        } else {
            "Usage limit reached"
        }
        .to_string();
        let detail = match (pause.resets_at, pause.resume) {
            (Some(at), true) => format!(
                "Resumes at {}",
                when(at + trek_core::limit::RESUME_GRACE_MS)
            ),
            (Some(at), false) => format!("Resets {}", when(at)),
            (None, _) => "Reset time unknown".to_string(),
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
                .text_color(if strong {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .when(strong, |el| el.font_medium())
                .hover(|s| {
                    s.bg(theme.foreground.opacity(0.06))
                        .text_color(theme.foreground)
                })
                .child(label)
        };
        let ws = self.workspace.clone();
        let on = |f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
            let (ws, id) = (ws.clone(), id.to_string());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                ws.update(cx, |ws, cx| f(ws, &id, cx))
            }
        };
        let mut actions: Vec<AnyElement> = vec![];
        match (pause.resets_at.is_some(), pause.resume) {
            (true, false) => actions.push(
                action("limit-resume", "Resume at reset", true)
                    .on_click(on(|ws, id, cx| ws.resume_at_reset(id, cx)))
                    .into_any_element(),
            ),
            (true, true) => actions.push(
                action("limit-cancel", "Cancel", false)
                    .on_click(on(|ws, id, cx| ws.cancel_resume(id, cx)))
                    .into_any_element(),
            ),
            (false, _) => actions.push(
                action("limit-retry", "Try again", true)
                    .on_click(on(|ws, id, cx| ws.end_pause(id, true, cx)))
                    .into_any_element(),
            ),
        }
        if pause.resets_at.is_some() && !snoozed {
            let label = if compact {
                "Snooze"
            } else {
                "Snooze until reset"
            };
            actions.push(
                action("limit-snooze", label, false)
                    .on_click(on(|ws, id, cx| ws.snooze_until_reset(id, cx)))
                    .into_any_element(),
            );
        }
        actions.push(
            action(
                "limit-switch",
                if compact {
                    "Switch…"
                } else {
                    "Switch agent…"
                },
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                pickers::open_model_menu(this, cx);
            }))
            .into_any_element(),
        );
        // Without a reset time, nothing sends them on its own: "Try again" (or the next message) does.
        let waits_for = if pause.resets_at.is_some() {
            "Sends when your limit resets"
        } else {
            "Sends when you try again"
        };
        let queued = pause.queued.iter().enumerate().map(|(ix, q)| {
            let ws = self.workspace.clone();
            let id = id.to_string();
            let text = if q.text.trim().is_empty() {
                format!(
                    "{} image{}",
                    q.images.len(),
                    if q.images.len() == 1 { "" } else { "s" }
                )
            } else {
                q.text.lines().next().unwrap_or_default().to_string()
            };
            h_flex()
                .id(("limit-queued", ix))
                .test_support()
                .h(px(28.))
                .px(px(12.))
                .gap(px(8.))
                .border_t_1()
                .border_color(theme.foreground.opacity(0.07))
                .child(
                    Icon::new(crate::assets::Lucide::Clock)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.foreground.opacity(0.85))
                        .child(text),
                )
                .when(!compact, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child(waits_for),
                    )
                })
                .child(
                    gpui_kit::component::button::Button::new(("limit-unqueue", ix))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Close).text_color(theme.muted_foreground))
                        .tooltip("Don't send this at the reset")
                        .on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| ws.unqueue_for_reset(&id, ix, cx))
                        }),
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
                        .child(
                            Icon::new(crate::assets::Lucide::Gauge)
                                .small()
                                .text_color(amber),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        )
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
        let open = self.pickers.open() || self.consult_open;
        self.workspace.update(cx, |ws, cx| {
            if ws.overlay_open != open {
                ws.overlay_open = open;
                cx.notify();
            }
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The open picker's rows, by label.
    #[cfg(any(test, feature = "test-support"))]
    pub fn picks(&self, cx: &App) -> Vec<String> {
        pickers::picker_items(self, cx)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn slash_completion(&self, cx: &App) -> (Option<String>, usize, usize) {
        let s = self.input.read(cx);
        (pickers::slash_ghost(self, cx), s.tokens().len(), s.cursor())
    }

    /// The consultants picked for the next message (`agent/model/effort`), and whether they stay.
    #[cfg(any(test, feature = "test-support"))]
    pub fn consultants(&self) -> (Vec<String>, bool) {
        (
            self.consult
                .consultants
                .iter()
                .map(Consultant::key)
                .collect(),
            self.consult_pinned,
        )
    }

    /// Whether the next message asks for a restatement first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn restating(&self) -> bool {
        self.restate
    }

    /// The judge an arena sent now would name.
    #[cfg(any(test, feature = "test-support"))]
    pub fn judge(&self, cx: &App) -> Option<String> {
        self.arena_judge(cx).map(|j| j.key())
    }

    /// Images attached to the next message, and how many are still being saved.
    #[cfg(any(test, feature = "test-support"))]
    pub fn attached(&self) -> (Vec<PathBuf>, usize) {
        (self.outbox.paths.clone(), self.outbox.saving)
    }

    /// A snapshot is waiting on Snipping Tool.
    #[cfg(any(test, feature = "test-support"))]
    pub fn snapshot_waiting(&self) -> bool {
        self.screenclip.is_some()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        composer
            .clone()
            .cached(StyleRefinement::default().w_full().flex_none().h(height))
            .into_any_element()
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    /// Insert text at the cursor (on its own line if the cursor is mid-text) and focus the composer.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_route(window, cx);
        self.input.update(cx, |s, cx| {
            let value = s.value().to_string();
            let cursor = s.cursor().min(value.len());
            let needs_break = cursor > 0 && !value[..cursor].ends_with('\n');
            let text = if needs_break {
                format!("\n{text}")
            } else {
                text.to_string()
            };
            s.insert(text, window, cx);
        });
        self.focus(window, cx);
        cx.notify();
    }

    /// The mic button: open voice mode (a take), or confirm the one that's open.
    pub fn dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.voice_step(voice::Event::Mic, window, cx);
    }

    /// Move voice mode on (`voice::Voice::step`) and do what that calls for.
    pub fn voice_step(&mut self, event: voice::Event, window: &mut Window, cx: &mut Context<Self>) {
        let effect = self.voice.step(event);
        if effect.ask {
            self.ask_to_dictate(window, cx);
        }
        if effect.finish {
            if let Some(d) = self.dictation.as_mut() {
                d.finish();
            }
            // A live take has its words on screen already: if the recognizer is slow to settle
            // them, they go in as shown.
            if self.voice.live {
                self.voice_tasks
                    .push(cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(voice::GRACE).await;
                        let _ = this.update_in(cx, |c, window, cx| {
                            c.voice_step(voice::Event::GraceOver, window, cx)
                        });
                    }));
            }
        }
        if effect.end {
            self.dictation = None;
            // Not dropped here: this can run inside one of them (the listener's own update).
            let tasks = std::mem::take(&mut self.voice_tasks);
            cx.spawn(async move |_, _| drop(tasks)).detach();
            if let Some(text) = effect.commit.as_deref() {
                self.insert_dictated(text, window, cx);
            } else if self.voice_focus.is_focused(window) {
                self.focus(window, cx);
            }
        }
        if let Some(message) = effect.toast {
            self.toast(message, cx);
        }
        cx.notify();
    }

    /// Ask for the mic and Speech; once both are allowed, open the take (unless it was
    /// abandoned while macOS asked).
    fn ask_to_dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let take = self.voice.take;
        let (tx, rx) = async_channel::bounded(1);
        dictate::ensure_permission(tx);
        cx.spawn_in(window, async move |this, cx| {
            let allowed = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("Dictation ended.".into()));
            let _ = this.update_in(cx, |c, window, cx| {
                if c.voice.take != take || c.voice.phase != voice::Phase::Asking {
                    return;
                }
                let (heard, results) = async_channel::unbounded();
                match allowed.and_then(|()| dictate::start(heard)) {
                    Ok(d) => {
                        let live = d.live();
                        c.dictation = Some(d);
                        c.begin_voice(live, results, window, cx);
                    }
                    Err(e) => c.voice_step(voice::Event::Failed(e), window, cx),
                }
            });
        })
        .detach();
    }

    /// The take is open: the waveform row replaces the toolbar, results feed the preview, and
    /// the levels tick at 20 fps.
    pub fn begin_voice(
        &mut self,
        live: bool,
        results: async_channel::Receiver<dictate::Heard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (value, cursor) = {
            let s = self.input.read(cx);
            (s.value().to_string(), s.cursor())
        };
        self.voice_step(
            voice::Event::Started {
                live,
                value,
                cursor,
            },
            window,
            cx,
        );
        self.voice_tasks
            .push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(heard) = results.recv().await {
                    let event = match heard {
                        dictate::Heard::Partial(t) => voice::Event::Partial(t),
                        dictate::Heard::Final(t) => voice::Event::Final(t),
                        dictate::Heard::Failed(e) => voice::Event::Failed(e),
                    };
                    if this
                        .update_in(cx, |c, window, cx| c.voice_step(event, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        self.voice_tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(voice::TICK).await;
                let going = this.update(cx, |c, cx| {
                    if !c.voice.active() {
                        return false;
                    }
                    let level = c.dictation.as_ref().map_or(0., |d| d.level());
                    c.voice.levels.push(level);
                    cx.notify();
                    true
                });
                if !going.unwrap_or(false) {
                    break;
                }
            }
        }));
        self.voice_focus.focus(window, cx);
    }

    /// Transcribe an audio file as if it were a dictated take — the shots harness's
    /// `dictate-file` verb, exercising Speech and the insert path without the mic: voice mode
    /// opens already settling (✓ spinning) and the text goes in when Speech is done.
    #[cfg(feature = "shots")]
    pub fn dictate_file(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let (heard, results) = async_channel::unbounded();
        match dictate::transcribe_file(file, heard) {
            Ok(d) => {
                self.dictation = Some(d);
                self.begin_voice(false, results, window, cx);
                self.voice_step(voice::Event::Confirm, window, cx);
            }
            Err(e) => self.toast(e, cx),
        }
    }

    /// Voice mode with no mic and no Speech, for captures (the shots harness's `dictate-demo
    /// [text]`): a live take opens, and `text`, if any, shows as heard so far with a burst of
    /// speech at the waveform's right end. ✓ and ✕ then work as in a real take.
    #[cfg(feature = "shots")]
    pub fn dictate_demo(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (heard, results) = async_channel::unbounded();
        if !text.trim().is_empty() {
            let _ = heard.try_send(dictate::Heard::Partial(text.trim().to_string()));
        }
        drop(heard);
        self.begin_voice(true, results, window, cx);
        if !text.trim().is_empty() {
            for i in 0..36 {
                let level = ((i as f32) * 0.9).sin().abs() * 0.85 * ((i % 7) as f32 / 7. + 0.3);
                self.voice.levels.push(level);
            }
        }
        cx.notify();
    }

    /// The text area while the waveform is up: what's typed, with what's heard so far at the
    /// caret in italic and muted (it's still being revised).
    fn voice_preview(&self, window: &Window, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (text, interim) = self.voice.preview();
        let body = if text.trim().is_empty() {
            div()
                .text_color(theme.muted_foreground)
                .child("Listening…")
                .into_any_element()
        } else {
            let style = HighlightStyle {
                font_style: Some(FontStyle::Italic),
                color: Some(theme.muted_foreground),
                ..Default::default()
            };
            let ranges = (!interim.is_empty()).then_some((interim, style));
            StyledText::new(text)
                .with_highlights(ranges)
                .into_any_element()
        };
        div()
            .id("voice-preview")
            .test_support()
            .track_focus(&self.voice_focus)
            .min_h(window.line_height() * 2.)
            .max_h(window.line_height() * 12.)
            .overflow_y_scroll()
            .text_color(theme.foreground)
            .child(body)
            .into_any_element()
    }

    /// The text heard so far as the preview shows it, and where the interim part is.
    #[cfg(any(test, feature = "test-support"))]
    pub fn voice_preview_text(&self) -> (String, std::ops::Range<usize>) {
        self.voice.preview()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn voice_phase(&self) -> voice::Phase {
        self.voice.phase
    }

    /// A note for the user (dictation problems, mostly) up as a workspace toast.
    fn toast(&self, message: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        });
    }

    /// What the mic heard, at the cursor — space-separated from the text it lands in.
    fn insert_dictated(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            let padded = voice::spaced(&s.value(), s.cursor(), text);
            s.insert(padded, window, cx);
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
    pub fn restore(
        &mut self,
        text: &str,
        images: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_route(window, cx);
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

    /// Attach files by inserting @-references into the prompt.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let (images, files) = attachments::split_files(paths);
                let _ = this.update_in(cx, |this, window, cx| {
                    attachments::attach_files(this, images, window, cx)
                });
                if files.is_empty() {
                    return;
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    this.append_file_refs(&files, window, cx)
                });
            }
        })
        .detach();
    }

    /// `@path` references after what's typed, the caret behind them.
    pub fn append_file_refs(
        &mut self,
        files: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let refs: Vec<String> = files
            .iter()
            .map(|p| trek_core::media::mention_of(&p.display().to_string()))
            .collect();
        self.input.update(cx, |s, cx| {
            let mut v = s.value().to_string();
            if !v.is_empty() && !v.ends_with(' ') {
                v.push(' ');
            }
            v.push_str(&refs.join(" "));
            v.push(' ');
            let end = v.len();
            // A multi-line input puts the caret at the start of a new value.
            s.set_value(v, window, cx);
            s.set_selected_range(end..end, cx);
        });
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
        if cfg!(windows) {
            // Snipping Tool's overlay picks the window or area itself, so `mode` has no say.
            return self.snapshot_by_screenclip(cx);
        }
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
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
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
                    cmd.arg(&out).status().map(|s| s.success()).unwrap_or(false)
                        && std::fs::metadata(&out)
                            .map(|m| m.len() > 0)
                            .unwrap_or(false)
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

    /// Windows: open Snipping Tool's overlay and attach what is picked in it, which it puts on the
    /// clipboard (`screenclip`). Asking again while waiting gives up on the snip, quietly: a snip
    /// that is cancelled in the overlay leaves nothing to tell it by.
    pub fn snapshot_by_screenclip(&mut self, cx: &mut Context<Self>) {
        if self.snapshotting {
            if self.screenclip.take().is_some() {
                self.snapshotting = false;
                cx.notify();
            }
            return;
        }
        let backend = screenclip::backend(cx);
        let mut source = backend.source();
        // Taken before the overlay opens, so only an image copied after it counts.
        let seen = source.sequence();
        if let Err(e) = backend.launch() {
            self.toast(format!("Couldn't open the snipping tool: {e}."), cx);
            return;
        }
        self.snapshotting = true;
        cx.notify();
        let path = mentions::snapshot_path();
        let (interval, timeout) = (backend.interval(), backend.timeout());
        self.screenclip = Some(cx.spawn(async move |this, cx| {
            let mut pickup = screenclip::Pickup::new(seen, interval, timeout);
            let arrived = loop {
                cx.background_executor().timer(interval).await;
                // The window was closed meanwhile: nobody is left to attach it to.
                let open = this.update(cx, |this, cx| cx.windows().contains(&this.window)).unwrap_or(false);
                if !open {
                    let _ = this.update(cx, |this, _| {
                        this.snapshotting = false;
                        if let Some(task) = this.screenclip.take() {
                            task.detach();
                        }
                    });
                    return;
                }
                let (back, wait, poll) = cx
                    .background_executor()
                    .spawn(async move {
                        let poll = pickup.poll(source.as_mut());
                        (source, pickup, poll)
                    })
                    .await;
                (source, pickup) = (back, wait);
                match poll {
                    screenclip::Poll::Waiting => {}
                    screenclip::Poll::Arrived(clip) => break Some(clip),
                    screenclip::Poll::TimedOut => break None,
                }
            };
            let saved = match arrived {
                Some(clip) => {
                    let out = path.clone();
                    Some(cx.background_executor().spawn(async move { screenclip::to_png(clip).and_then(|png| Ok(std::fs::write(&out, png)?)) }).await)
                }
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.snapshotting = false;
                // This task is the one finishing: let it go on rather than cancel itself.
                if let Some(task) = this.screenclip.take() {
                    task.detach();
                }
                match saved {
                    Some(Ok(())) => this.outbox.add([path]),
                    Some(Err(e)) => this.toast(format!("Couldn't attach the snapshot: {e}."), cx),
                    None => this.toast(crate::keys::localize("Nothing was picked in a minute, so no snapshot was attached. ⌘⇧S tries again.").into_owned(), cx),
                }
                cx.notify();
            });
        }));
    }

    fn add_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let (images, files) = attachments::split_files(paths.iter().cloned());
        attachments::attach_files(self, images, window, cx);
        if !files.is_empty() {
            let refs: Vec<String> = files
                .iter()
                .map(|p| {
                    format!(
                        "{} ",
                        trek_core::media::mention_of(&p.display().to_string())
                    )
                })
                .collect();
            self.input
                .update(cx, |s, cx| s.insert(refs.concat(), window, cx));
        }
        cx.notify();
    }

    // ---------- pickers ----------

    fn insert_trigger(&mut self, ch: &str, window: &mut Window, cx: &mut Context<Self>) {
        pickers::insert_trigger(self, ch, window, cx);
    }

    fn picker_action(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if key == "escape" && self.pickers.trigger.is_none() && self.editing.is_some() {
            self.cancel_edit(window, cx);
            cx.stop_propagation();
            return;
        }
        pickers::picker_action(self, key, window, cx);
    }

    /// Enter reaches us before the textarea: it picks a row while a picker is open, and sends on
    /// ⌘↩ in "send with ⌘↩" mode (the textarea would otherwise insert a newline first).
    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.pickers.trigger.is_some() && !action.shift && !action.secondary {
            self.picker_action("enter", window, cx);
        } else if self.cmd_enter && action.secondary && !action.shift {
            cx.stop_propagation();
            let input = self.input.clone();
            self.submit(input, window, cx);
        }
    }

    fn attachment_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.outbox.paths.is_empty() && !self.snapshotting && self.outbox.saving == 0 {
            return None;
        }
        let me = cx.entity().downgrade();
        let remove = move |path: &std::path::Path, _: &mut Window, cx: &mut App| {
            let _ = me.update(cx, |this, cx| {
                this.outbox.remove(path);
                cx.notify();
            });
        };
        let waiting = self.screenclip.is_some().then(|| {
            div()
                .id("snapshot-wait")
                .test_support()
                .pt(px(6.))
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(crate::keys::localize("Pick a region… ⌘⇧S cancels").into_owned())
        });
        Some(
            div()
                .px(px(14.))
                .pt(px(12.))
                .child(attachments::thumbnails(
                    &self.outbox.paths,
                    px(56.),
                    self.snapshotting || self.outbox.saving > 0,
                    remove,
                    cx,
                ))
                .children(waiting)
                .into_any_element(),
        )
    }

    /// "+" menu: files and photos, snapshots, the three pickers, and restating first.
    fn plus_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let theme = cx.theme().clone();
        let restate = self.restate;
        gpui_kit::component::button::Button::new("attach")
            .ghost()
            // A 30 px circle on the chips' fill.
            .with_size(px(look::CONTROL))
            .w(px(look::CONTROL))
            .h(px(look::CONTROL))
            .p_0()
            .rounded(px(look::CONTROL / 2.))
            .bg(look::chip_fill(cx))
            // A child, not `.icon()`: the kit sizes an icon to 3/4 of its button.
            .child(
                Icon::new(IconName::Plus)
                    .size(px(13.))
                    .text_color(theme.muted_foreground),
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let (a, b, c, d, e, f, g, h) = (
                    me.clone(),
                    me.clone(),
                    me.clone(),
                    me.clone(),
                    me.clone(),
                    me.clone(),
                    me.clone(),
                    me.clone(),
                );
                let menu = menu
                    .min_w(px(230.))
                    .check_side(gpui_kit::component::Side::Right)
                    .item(
                        PopupMenuItem::new("Add photos & files")
                            .icon(crate::assets::Lucide::Image)
                            .on_click(move |_, window, cx| {
                                a.update(cx, |c, cx| c.attach(window, cx))
                            }),
                    )
                    .separator();
                // Snipping Tool has the window, area and screen choice in its own toolbar.
                let menu = if cfg!(windows) {
                    menu.item(
                        PopupMenuItem::new("Take a snapshot")
                            .icon(crate::assets::Lucide::Camera)
                            .on_click(move |_, _, cx| b.update(cx, |c, cx| c.snapshot("area", cx))),
                    )
                } else {
                    menu.item(
                        PopupMenuItem::new("Snapshot a window")
                            .icon(crate::assets::Lucide::Camera)
                            .on_click(move |_, _, cx| {
                                b.update(cx, |c, cx| c.snapshot("window", cx))
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Snapshot an area")
                            .icon(crate::assets::Lucide::Crosshair)
                            .on_click(move |_, _, cx| c.update(cx, |c, cx| c.snapshot("area", cx))),
                    )
                    .item(
                        PopupMenuItem::new("Snapshot the screen")
                            .icon(crate::assets::Lucide::Monitor)
                            .on_click(move |_, _, cx| {
                                d.update(cx, |c, cx| c.snapshot("screen", cx))
                            }),
                    )
                };
                menu.separator()
                    .item(
                        PopupMenuItem::new("Mention a file  @")
                            .icon(IconName::File)
                            .on_click(move |_, window, cx| {
                                e.update(cx, |c, cx| c.insert_trigger("@", window, cx))
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Use a skill  $")
                            .icon(crate::assets::Lucide::Sparkle)
                            .on_click(move |_, window, cx| {
                                f.update(cx, |c, cx| c.insert_trigger("$", window, cx))
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Run a command  /")
                            .icon(IconName::SquareTerminal)
                            .on_click(move |_, window, cx| {
                                g.update(cx, |c, cx| c.insert_trigger("/", window, cx))
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Restate first")
                            .icon(crate::assets::Lucide::MessageSquareQuote)
                            .checked(restate)
                            .on_click(move |_, window, cx| {
                                h.update(cx, |c, cx| {
                                    c.restate = !c.restate;
                                    c.focus(window, cx);
                                    cx.notify();
                                })
                            }),
                    )
            })
    }

    // ---------- consult ----------

    /// "Sol · High": a consultant as the menu and the agent's instructions name it.
    fn consultant_names(&self, cx: &App) -> Box<dyn Fn(&Consultant) -> String> {
        let ws = self.workspace.read(cx);
        let models: Vec<(AgentId, Vec<ModelInfo>)> = self
            .consult
            .consultants
            .iter()
            .map(|c| (c.agent.clone(), ws.models_for(&c.agent)))
            .collect();
        Box::new(move |c: &Consultant| {
            let name = models
                .iter()
                .find(|(a, _)| *a == c.agent)
                .map(|(_, m)| model_name(m, &c.model))
                .unwrap_or_else(|| c.model.clone());
            format!("{name} · {}", c.effort.label())
        })
    }

    /// `/consult sol high, opus max: <message>` (also `discuss`, `report`): the consultants it
    /// names are picked; returns the message to send, `None` when there's none (the menu opens).
    /// `None` overall when `text` isn't the command.
    fn consult_command(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Option<Result<Option<String>, String>> {
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
            let effort = words
                .last()
                .and_then(|w| Effort::parse(w))
                .filter(|_| words.len() > 1);
            if effort.is_some() {
                words.pop();
            }
            let query = words.join(" ");
            let Some((agent, model)) = find_model(ws, &query) else {
                return Some(Err(format!(
                    "There's no model called “{query}” to consult."
                )));
            };
            picked.push(consultant(agent, &model, effort));
        }
        // An arena's designs all run at once, so there are no more than can.
        let most = trek_core::orchestrate::MAX_RUNNING;
        if self.consult.style == Style::Arena && picked.len() > most {
            return Some(Err(format!(
                "An arena drafts {most} designs at most, all at once: name up to {most} models."
            )));
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
            Some(_) if self.consult.consultants.is_empty() => Some(Err(
                "Name a model to consult: /consult sol high: your question".into(),
            )),
            message => Some(Ok(message)),
        }
    }

    fn arena_options(&self, cx: &App) -> Vec<(AgentId, ModelInfo)> {
        arena_options(self.workspace.read(cx))
    }

    /// The thread's own agent and model.
    fn main_model(&self, cx: &App) -> (AgentId, String) {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let models = ws.models_for(&prefs.agent);
        let model = prefs
            .model
            .clone()
            .or_else(|| default_model(&models).map(|m| m.id.clone()))
            .unwrap_or_default();
        // As the model list names it, so it's told apart from the others there.
        let model = models
            .iter()
            .find(|m| same_model(&model, &m.id))
            .map(|m| m.id.clone())
            .unwrap_or(model);
        (prefs.agent, model)
    }

    /// An arena's candidates when none are picked: one per model family.
    fn arena_defaults(&self, cx: &App) -> Vec<Consultant> {
        trek_core::orchestrate::arena_defaults(&self.arena_options(cx))
    }

    /// Who judges the arena: the one picked, else a model of another family than the thread's.
    fn arena_judge(&self, cx: &App) -> Option<Consultant> {
        let (agent, model) = self.main_model(cx);
        self.consult.judge.clone().or_else(|| {
            trek_core::orchestrate::pick_judge(
                (&agent, &model),
                &self.consult.consultants,
                &self.arena_options(cx),
            )
        })
    }

    /// The consult menu: who's consulted, at what effort, how, and then what.
    fn consult_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let agents = ws.ready_agents();
        // Another agent than the thread's own comes first: a second opinion from elsewhere.
        let rail = self
            .consult_rail
            .clone()
            .filter(|a| agents.contains(a))
            .or_else(|| agents.iter().find(|a| **a != prefs.agent).cloned())
            .or_else(|| agents.first().cloned());
        let models = rail.as_ref().map(|a| ws.models_for(a)).unwrap_or_default();
        let picked_models: Vec<Vec<ModelInfo>> = self
            .consult
            .consultants
            .iter()
            .map(|c| ws.models_for(&c.agent))
            .collect();
        let main_agent = prefs.agent.display_name();
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let hairline = look::hairline(cx);
        let me = cx.entity();
        let empty = self.consult.consultants.is_empty();

        let header = h_flex()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(2.))
            .gap(px(6.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .font_medium()
                    .child("Consult other models"),
            )
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
                    .icon(
                        Icon::new(if self.consult_pinned {
                            crate::assets::Lucide::Pin
                        } else {
                            crate::assets::Lucide::PinOff
                        })
                        .text_color(if self.consult_pinned {
                            theme.foreground
                        } else {
                            muted
                        }),
                    )
                    .tooltip(if self.consult_pinned {
                        "Kept for every message: click to clear after sending"
                    } else {
                        "Keep for the next messages too"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.consult_pinned = !this.consult_pinned;
                        cx.notify();
                    })),
            );
        let note = match self.consult.style {
            Style::Advise => format!("They review and suggest; {main_agent} decides."),
            Style::Discuss => format!(
                "{main_agent} and they go back and forth until they agree ({} rounds at most).",
                trek_core::orchestrate::DISCUSS_ROUNDS
            ),
            Style::Arena => {
                let (agent, model) = self.main_model(cx);
                let own = trek_core::orchestrate::family(&agent, &model);
                let judge = match self.arena_judge(cx) {
                    Some(j) if trek_core::orchestrate::family(&j.agent, &j.model) == own => {
                        "another model"
                    }
                    _ => "a model of another family",
                };
                format!(
                    "{main_agent} grounds the problem; each drafts a design on its own; {judge} judges them blind; {main_agent} synthesises the best."
                )
            }
        };
        // An arena's designs all run at once: no more than can.
        let most = trek_core::orchestrate::MAX_RUNNING;
        let full = self.consult.style == Style::Arena && self.consult.consultants.len() >= most;
        let note = if full {
            format!("{note} {most} designs at most.")
        } else {
            note
        };
        let picked = v_flex()
            .px(px(5.))
            .children(self.consult.consultants.iter().enumerate().map(|(i, c)| {
                let name = picked_models
                    .get(i)
                    .map(|m| model_name(m, &c.model))
                    .unwrap_or_else(|| c.model.clone());
                let choosing = self.consult_effort == Some(i);
                look::row(("consultant", i), choosing, cx)
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
                                this.consult_effort = if this.consult_effort == Some(i) {
                                    None
                                } else {
                                    Some(i)
                                };
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
                        this.consult_effort = if this.consult_effort == Some(i) {
                            None
                        } else {
                            Some(i)
                        };
                        this.consult_judge = false;
                        cx.notify();
                    }))
            }));
        // An arena's judge, with a side panel to pick another.
        let judge = (self.consult.style == Style::Arena).then(|| {
            let j = self.arena_judge(cx);
            let name = j
                .as_ref()
                .map(|j| {
                    format!(
                        "{} · {}",
                        model_name(&ws.models_for(&j.agent), &j.model),
                        j.effort.label()
                    )
                })
                .unwrap_or_else(|| "No other model on offer".into());
            v_flex().px(px(5.)).pt(px(2.)).child(
                look::row("consult-judge", self.consult_judge, cx)
                    .test_support()
                    .min_h(px(32.))
                    .child(
                        Icon::new(crate::assets::Lucide::Scale)
                            .small()
                            .text_color(muted),
                    )
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
        let rail_row = h_flex()
            .flex_wrap()
            .gap(px(2.))
            .px(px(6.))
            .pt(px(6.))
            .children(agents.iter().map(|a| {
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
                    .when(active, |el| el.bg(look::pressed(cx)))
                    .hover(|s| s.bg(look::chip_fill(cx)))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .child(ui::agent_glyph(a, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.consult_rail = Some(target.clone());
                        cx.notify();
                    }))
            }));
        let list = v_flex()
            .id("consult-models")
            .min_w_0()
            .px(px(5.))
            .pt(px(4.))
            .max_h(px(220.))
            .overflow_y_scroll()
            .children(rail.iter().flat_map(|agent| {
                models.iter().map(|m| {
                    let on = self
                        .consult
                        .consultants
                        .iter()
                        .any(|c| c.agent == *agent && same_model(&c.model, &m.id));
                    let (agent, model) = (agent.clone(), m.clone());
                    look::row(
                        SharedString::from(format!("consult-add-{}-{}", agent.key(), m.id)),
                        false,
                        cx,
                    )
                    .test_support()
                    .min_h(px(30.))
                    .when(full && !on, |el| el.opacity(0.45).cursor_default())
                    .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
                    .when(on, |el| el.child(Icon::new(IconName::Check).small()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let at = this
                            .consult
                            .consultants
                            .iter()
                            .position(|c| c.agent == agent && same_model(&c.model, &model.id));
                        let full = this.consult.style == Style::Arena
                            && this.consult.consultants.len()
                                >= trek_core::orchestrate::MAX_RUNNING;
                        match at {
                            Some(i) => _ = this.consult.consultants.remove(i),
                            None if full => return,
                            None => this.consult.consultants.push(consultant(
                                agent.clone(),
                                &model,
                                None,
                            )),
                        }
                        this.consult_effort = None;
                        cx.notify();
                    }))
                })
            }));
        let (m1, m2) = (me.clone(), me.clone());
        let main = look::surface(cx)
            .id("consult-menu-body")
            .test_support()
            .w(px(330.))
            .p_0()
            .pb(px(8.))
            .child(header)
            .child(
                div()
                    .px(px(10.))
                    .pb(px(6.))
                    .text_size(px(12.5))
                    .line_height(relative(1.4))
                    .text_color(muted)
                    .child(note),
            )
            .when(!empty, |el| el.child(picked))
            .children(judge)
            .child(
                v_flex()
                    .mt(px(6.))
                    .border_t_1()
                    .border_color(hairline)
                    .child(rail_row)
                    .child(list),
            )
            .child(
                v_flex()
                    // The switches hug their options: stretched, they'd trail an empty track.
                    .items_start()
                    .gap(px(8.))
                    .px(px(10.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(hairline)
                    .child(look::segmented(
                        "consult-style",
                        vec![
                            (Style::Advise, "Advise"),
                            (Style::Discuss, "Discuss"),
                            (Style::Arena, "Arena"),
                        ],
                        self.consult.style,
                        move |v, _, cx| {
                            m1.update(cx, |c, cx| {
                                c.consult.style = v;
                                // An arena starts from one design per model family.
                                if v == Style::Arena && c.consult.consultants.is_empty() {
                                    c.consult.consultants = c.arena_defaults(cx);
                                }
                                if v == Style::Arena {
                                    c.consult
                                        .consultants
                                        .truncate(trek_core::orchestrate::MAX_RUNNING);
                                }
                                if v != Style::Arena {
                                    c.consult_judge = false;
                                }
                                cx.notify();
                            })
                        },
                        cx,
                    ))
                    .child(look::segmented(
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
            look::surface(cx)
                .id("consult-judges")
                .test_support()
                .w(px(230.))
                .max_h(px(300.))
                .overflow_y_scroll()
                .when(options.is_empty(), |el| {
                    el.child(
                        div()
                            .p(px(8.))
                            .text_size(px(12.5))
                            .text_color(muted)
                            .child("There's no other model on offer to judge."),
                    )
                })
                .children(options.into_iter().map(|(agent, m)| {
                    let on = current
                        .as_ref()
                        .is_some_and(|j| j.agent == agent && same_model(&j.model, &m.id));
                    look::row(
                        SharedString::from(format!("consult-judge-{}-{}", agent.key(), m.id)),
                        on,
                        cx,
                    )
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
            let efforts = picked_models
                .get(i)
                .and_then(|ms| ms.iter().find(|m| same_model(&c.model, &m.id)))
                .map(|m| m.efforts.clone())
                .filter(|e| !e.is_empty());
            let efforts = efforts.unwrap_or_else(|| {
                vec![
                    Effort::Low,
                    Effort::Medium,
                    Effort::High,
                    Effort::XHigh,
                    Effort::Max,
                ]
            });
            let current = c.effort;
            Some(
                look::surface(cx)
                    .w(px(170.))
                    .children(efforts.into_iter().map(|e| {
                        look::row(
                            SharedString::from(format!("consult-eff-{}", e.as_str())),
                            e == current,
                            cx,
                        )
                        .test_support()
                        .child(div().flex_1().child(e.label()))
                        .when(e == current, |el| {
                            el.child(Icon::new(IconName::Check).small())
                        })
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
        h_flex()
            .items_end()
            .gap(px(6.))
            .child(main)
            .children(side.or(judges))
            .into_any_element()
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
            format!(
                "Consulting {}{}",
                who.join(", "),
                if self.consult_pinned {
                    " · kept for every message"
                } else {
                    ""
                }
            )
            .into()
        };
        let style = match self.consult.style {
            Style::Advise => "Advise",
            Style::Discuss => "Discuss",
            Style::Arena => "Arena",
        };
        let logos = h_flex().children(self.consult.consultants.iter().take(4).enumerate().map(
            |(i, c)| {
                div()
                    .when(i > 0, |el| el.ml(px(-5.)))
                    .p(px(1.))
                    .rounded(px(5.))
                    .bg(look::card_fill(cx))
                    .child(ui::agent_logo(&c.agent, px(14.), cx))
            },
        ));
        // An agent that wouldn't get `delegate_task` can't consult: the pill says why, and opens
        // only to let go of consultants already picked.
        let agent = self.workspace.read(cx).prefs_in(&self.scope).agent;
        if let (Some(why), true) = (self.workspace.read(cx).consult_unavailable(&agent), empty) {
            return Pill::new("consult-pill")
                .chip()
                .ghost(true)
                .tooltip(format!("Can't consult: {why}"))
                .child(
                    Icon::new(crate::assets::Lucide::MessagesSquare)
                        .size(px(look::ICON))
                        .text_color(muted.opacity(0.5)),
                )
                .when(!compact, |p| {
                    p.child(div().text_color(muted.opacity(0.5)).child("Consult"))
                })
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
                // Quiet until models are picked (no fill at rest, as the design's effort chip),
                // then a chip like the others.
                Pill::new("consult-pill")
                    .chip()
                    .ghost(empty)
                    .selected(open)
                    .tooltip(tip)
                    .when(empty, |p| {
                        p.child(
                            Icon::new(crate::assets::Lucide::MessagesSquare)
                                .size(px(look::ICON))
                                .text_color(muted),
                        )
                        .when(!compact, |p| {
                            p.child(div().text_color(muted).child("Consult"))
                        })
                    })
                    .when(!empty, |p| {
                        p.child(logos)
                            .when(!compact, |p| p.child(div().text_color(muted).child(style)))
                            .when(self.consult_pinned, |p| {
                                p.child(
                                    Icon::new(crate::assets::Lucide::Pin)
                                        .xsmall()
                                        .text_color(muted),
                                )
                            })
                    }),
            )
            .content(move |_, _, cx| entity.update(cx, |c, cx| c.consult_menu(cx)))
            .into_any_element()
    }

    // ---------- project + clone ----------

    fn project_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let project = ws.draft_project_in(&self.scope);
        let label = project
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "No project".into());
        // The project's colour on its folder, as on its badge in the sidebar.
        let tint = project.as_ref().and_then(|p| ws.project_tint_at(p, cx));
        let projects: Vec<(String, std::path::PathBuf)> = ws
            .workspace_projects()
            .into_iter()
            .map(|p| {
                (
                    p.remote
                        .clone()
                        .map(|r| format!("{}  ·  {r}", p.name))
                        .unwrap_or_else(|| p.name.clone()),
                    p.path.clone(),
                )
            })
            .collect();
        let ws_entity = self.workspace.clone();
        let composer = cx.entity();
        look::chip_button(
            gpui_kit::component::button::Button::new("project-picker"),
            vec![
                Icon::new(IconName::FolderOpen)
                    .size(px(look::ICON))
                    .when_some(tint, |i, c| i.text_color(c))
                    .into_any_element(),
                div().child(label).into_any_element(),
            ],
            cx,
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let ws = ws_entity.clone();
            // A thread of its own, in no project: a folder of its own to work in.
            let mut menu = menu
                .min_w(px(280.))
                .max_h(px(360.))
                .scrollable(true)
                .item(
                    PopupMenuItem::new("No project")
                        .checked(project.is_none())
                        .on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx))
                        }),
                )
                .separator()
                .label("Projects");
            for (name, path) in projects.clone() {
                let ws = ws_entity.clone();
                let checked = project.as_ref() == Some(&path);
                menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(
                    move |_, _, cx| {
                        let path = path.clone();
                        ws.update(cx, |ws, cx| {
                            ws.navigate(
                                Route::Draft {
                                    project: Some(path),
                                },
                                cx,
                            )
                        })
                    },
                ));
            }
            let ws = ws_entity.clone();
            let c = composer.clone();
            menu.separator()
                .item(
                    PopupMenuItem::new("Open folder…")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))),
                )
                .item(
                    PopupMenuItem::new("Clone from GitHub…")
                        .icon(IconName::Github)
                        .on_click(move |_, window, cx| {
                            c.update(cx, |c, cx| c.open_clone_dialog(window, cx))
                        }),
                )
        })
    }

    /// Where a draft runs: the project folder, or a worktree of its own (git projects). A quiet
    /// menu; the choice is fixed once the thread starts. `blocked`: why a worktree can't start
    /// from the project folder right now (no branch, or no commit, to start from).
    fn place_chip(
        &self,
        worktree: bool,
        blocked: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.clone();
        let scope = self.scope.clone();
        let (icon, label) = if worktree {
            (crate::assets::Lucide::GitBranchPlus, "New worktree")
        } else {
            (crate::assets::Lucide::Laptop, "Local")
        };
        look::chip_button(
            gpui_kit::component::button::Button::new("env-place"),
            vec![
                Icon::new(icon)
                    .size(px(look::ICON))
                    .text_color(if worktree && blocked.is_some() {
                        palette::amber(cx)
                    } else {
                        theme.muted_foreground
                    })
                    .into_any_element(),
                div().child(label).into_any_element(),
            ],
            cx,
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let pick = |label: &'static str, on: bool| {
                let (ws, scope) = (ws.clone(), scope.clone());
                PopupMenuItem::new(label)
                    .checked(worktree == on)
                    .on_click(move |_, _, cx| {
                        ws.update(cx, |ws, cx| {
                            let mut p = ws.prefs_in(&scope);
                            p.worktree = on;
                            ws.set_prefs_in(&scope, p, cx);
                        })
                    })
            };
            // The items have icons, which would take the check's place on the left.
            let menu = menu
                .min_w(px(240.))
                .check_side(gpui_kit::component::Side::Right)
                .label("New thread runs in")
                .item(pick("Local: the project folder", false).icon(crate::assets::Lucide::Laptop))
                .item(
                    pick("New worktree: a branch of its own", true)
                        .icon(crate::assets::Lucide::GitBranchPlus)
                        .disabled(blocked.is_some() && !worktree),
                );
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
        // The main window's draft in a project picks the machine its thread starts on; a thread
        // on another machine names it. On another machine, this Mac's branch is beside the point.
        let main_draft = is_draft && with_project && self.scope == Scope::Main;
        let draft_project = ws.draft_project_in(&self.scope).filter(|_| main_draft);
        let elsewhere = main_draft && ws.draft_machine(draft_project.as_deref()).is_some();
        let has_machines = !ws.machines.list.is_empty();
        let theme = cx.theme().clone();
        // A chip that only says something (no menu): the chips' size and type, no fill.
        let chip = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(look::CONTROL))
                .px(px(9.))
                .gap(px(6.))
                .rounded_full()
                .text_size(px(look::LABEL))
                .text_color(theme.muted_foreground)
        };
        // A worktree needs a branch to start from (and a commit on it). The menu shows for any git
        // project, and whenever the draft is set to a worktree (its git state may not be in yet):
        // what it says is what sending does, and it can always go back to Local.
        let is_repo = git.as_ref().is_some_and(|g| g.is_repo);
        let blocked = match &git {
            Some(g) if g.is_repo && g.branch.is_none() => {
                Some("The project folder isn't on a branch to start from.")
            }
            Some(g) if g.is_repo && g.branches.is_empty() => {
                Some("The repository has no commits to start from.")
            }
            _ => None,
        };
        let place = if is_draft && (is_repo || draft_worktree) {
            self.place_chip(draft_worktree, blocked, cx)
        } else if let Some(wt) = worktree.clone() {
            let tip = format!(
                "Runs in a worktree of its own: {}",
                trek_core::paths::tildify(&wt.path)
            );
            chip("env-worktree")
                .test_support()
                .child(
                    Icon::new(crate::assets::Lucide::GitBranchPlus)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child("Worktree")
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .into_any_element()
        } else if is_draft {
            chip("env-local")
                .child(
                    Icon::new(crate::assets::Lucide::Laptop)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child("Local")
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "Runs on {}, in the project folder",
                        crate::words::words().this_computer
                    ))
                    .build(window, cx)
                })
                .into_any_element()
        } else {
            // A thread in the project folder is the usual case: only a worktree is worth a chip.
            div().into_any_element()
        };
        let branch: Option<AnyElement> = match git {
            // The thread's own branch, off its base.
            _ if worktree.is_some() => worktree.map(|wt| {
                chip("env-branch")
                    .child(
                        Icon::new(crate::assets::Lucide::GitBranch)
                            .small()
                            .text_color(palette::indigo(cx)),
                    )
                    .child(
                        div()
                            .text_color(theme.foreground.opacity(0.9))
                            .child(wt.branch.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("off {}", wt.base)),
                    )
                    .into_any_element()
            }),
            // A draft headed for a worktree: the branch it starts from, with no switcher (switching
            // would check another branch out in the project folder, not pick a base).
            Some(g) if is_draft && draft_worktree && g.is_repo => Some(
                chip("env-base")
                    .test_support()
                    .child(
                        Icon::new(crate::assets::Lucide::GitBranch)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .when_some(g.branch.clone(), |el, b| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("off"),
                        )
                        .child(div().text_color(theme.foreground.opacity(0.9)).child(b))
                    })
                    .when(g.branch.is_none(), |el| {
                        el.text_color(palette::amber(cx)).child("detached HEAD")
                    })
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "The worktree's branch starts from the branch the project folder is on",
                        )
                        .build(window, cx)
                    })
                    .into_any_element(),
            ),
            Some(g) if g.is_repo => {
                let name = g.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                let on_default = g.on_default();
                let ws_entity = self.workspace.clone();
                let branches = g.branches.clone();
                let elsewhere = g.elsewhere.clone();
                let current = g.branch.clone();
                let default = g.default_branch.clone();
                let status = match (g.changed, g.ahead) {
                    (0, 0) => None,
                    (c, 0) => Some(format!("{c} changed")),
                    (0, a) => Some(format!("↑{a}")),
                    (c, a) => Some(format!("{c} changed · ↑{a}")),
                };
                Some(
                    look::chip_button(
                        gpui_kit::component::button::Button::new("branch-chip"),
                        vec![
                            Icon::new(crate::assets::Lucide::GitBranch)
                                .size(px(look::ICON))
                                .text_color(if on_default {
                                    theme.muted_foreground
                                } else {
                                    palette::indigo(cx)
                                })
                                .into_any_element(),
                            div().child(name).into_any_element(),
                        ],
                        cx,
                    )
                    .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, cx| {
                        // Read as the menu opens: an agent may have started since the chip drew.
                        let blocked = cwd
                            .as_deref()
                            .and_then(|c| ws_entity.read(cx).switch_blocked(c));
                        menu = menu
                            .min_w(px(220.))
                            .max_h(px(320.))
                            .scrollable(true)
                            .label(blocked.clone().unwrap_or_else(|| "Switch branch".into()));
                        for b in branches.clone() {
                            let (ws, cwd) = (ws_entity.clone(), cwd.clone());
                            let is_current = Some(&b) == current.as_ref();
                            // Git won't check out a branch another worktree has (a worktree thread's).
                            let taken = elsewhere.contains(&b);
                            let label = match (Some(&b) == default.as_ref(), taken) {
                                (_, true) => format!("{b}  (in another worktree)"),
                                (true, false) => format!("{b}  (default)"),
                                (false, false) => b.clone(),
                            };
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(is_current)
                                    .disabled(!is_current && (taken || blocked.is_some()))
                                    .on_click(move |_, _, cx| {
                                        let b = b.clone();
                                        if let Some(cwd) = cwd.clone() {
                                            ws.update(cx, |ws, cx| ws.switch_branch(cwd, b, cx))
                                        }
                                    }),
                            );
                        }
                        menu
                    })
                    .into_any_element(),
                )
                .map(|branch| {
                    // What's changed, beside the branch: it opens Source control, where they are.
                    // Uncommitted work is normal, not a warning: quiet, like the branch's details.
                    let Some(s) = status else { return branch };
                    let ws = self.workspace.clone();
                    let muted = theme.muted_foreground;
                    h_flex()
                        .gap(px(2.))
                        .child(branch)
                        .child(
                            gpui_kit::component::button::Button::new("changes-to-git")
                                .ghost()
                                .small()
                                .h(px(look::CONTROL))
                                .rounded(px(look::CONTROL / 2.))
                                .child(
                                    h_flex()
                                        .gap(px(5.))
                                        .text_size(px(look::LABEL))
                                        .text_color(muted)
                                        .child(
                                            Icon::new(crate::assets::Lucide::GitCompare).xsmall(),
                                        )
                                        .child(s),
                                )
                                .tooltip("Source control")
                                .on_click(move |_, _, cx| {
                                    ws.update(cx, |_, cx| {
                                        cx.emit(crate::workspace::WorkspaceEvent::OpenTool(
                                            crate::workspace::PanelTool::Git,
                                        ))
                                    })
                                }),
                        )
                        .into_any_element()
                })
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
        // No project and no machine paired: nowhere else to start, so no chip for it.
        let machine = (main_draft && (draft_project.is_some() || has_machines))
            .then(|| self.machine_chip(draft_project.clone(), cx));
        let on_machine = self.on_machine_chip(cx);
        // A thread in no project on another machine runs in its home folder there, not "Local".
        let place = (!(elsewhere && draft_project.is_none())).then_some(place);
        h_flex()
            .gap(px(look::GAP))
            .when(with_project, |el| el.child(self.project_chip(cx)))
            .children(machine)
            .children(on_machine)
            .children(place)
            .children(branch.filter(|_| !elsewhere))
            .into_any_element()
    }

    /// The thread's worktree folder is gone: say so, and offer the ways on. Messages wait.
    fn missing_worktree(
        &self,
        wt: &trek_core::worktree::Worktree,
        id: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
}

impl PickerHost for Composer {
    fn pickers(&mut self) -> &mut Pickers {
        &mut self.pickers
    }

    fn pickers_ref(&self) -> &Pickers {
        &self.pickers
    }

    fn workspace(&self) -> &Entity<Workspace> {
        &self.workspace
    }

    fn scope(&self) -> &Scope {
        &self.scope
    }

    fn input(&self) -> &Entity<TextareaState> {
        &self.input
    }

    fn overlay_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_overlay(cx);
    }
}

impl Attaching for Composer {
    fn outbox(&mut self) -> &mut Outbox {
        &mut self.outbox
    }

    fn target(&self, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        match ws.thread_id_in(&self.scope) {
            Some(id) => format!("thread:{id}"),
            None if ws.is_draft_in(&self.scope) => format!(
                "draft:{}",
                ws.draft_project_in(&self.scope)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            None => String::new(),
        }
    }

    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        self.submit(input, window, cx);
    }

    fn land_elsewhere(&mut self, target: &str, paths: Vec<PathBuf>) {
        let draft = self.drafts.entry(target.to_string()).or_insert(Draft {
            text: String::new(),
            cursor: 0,
            images: Vec::new(),
            restate: false,
        });
        for path in paths {
            if !draft.images.contains(&path) {
                draft.images.push(path);
            }
        }
    }
}

/// Below this card width the composer's pills get their narrowest form.
const NARROW: f32 = 440.;

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::rendered("Composer");
        self.redraw_when_chip_goes(cx);
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs_in(&self.scope);
        let thread = ws.thread_in(&self.scope).cloned();
        let is_draft = ws.is_draft_in(&self.scope);
        let main = self.scope == Scope::Main;
        let follow_up = ws.settings.general.follow_up;
        let own_turn = thread
            .as_ref()
            .is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou));
        // Its turn is over but it waits on sub-agents (Trek's, or its agent's own in the background).
        let children_working = thread
            .as_ref()
            .is_some_and(|t| !ws.running_children(&t.id).is_empty() || ws.waiting(&t.id));
        let live = thread.as_ref().and_then(|t| ws.live.get(&t.id));
        // The API cost estimate: changes as usage is reported, not per token.
        let billing = thread.as_ref().and_then(|t| ws.billing_of(t));
        let spend = live.and_then(|l| l.spend.clone()).unwrap_or_default();
        let cost_label = crate::cost::label(billing.as_ref(), &spend);
        // Beside the estimate: how much of the prompts the cache served (a model on this Mac
        // has no estimate and no cache to speak of).
        let cache_hits = cost_label
            .as_ref()
            .and_then(|_| crate::cost::CacheHits::of(&spend));
        let billing_tip = crate::cost::billing_note(billing.as_ref());
        let queued = thread.as_ref().map_or(0, |t| ws.queued(&t.id));
        let theme = cx.theme().clone();
        let empty = self.input.read(cx).value().trim().is_empty()
            && self.outbox.paths.is_empty()
            && self.outbox.saving == 0;
        // Stop also ends sub-agents still at work after their parent's turn, unless there's a
        // message to send: the parent is free to take it.
        let running = own_turn || (children_working && empty);
        let thread_id = thread.as_ref().map(|t| t.id.clone());
        let context = thread
            .as_ref()
            .and_then(|t| ws.live.get(&t.id))
            .and_then(|l| l.context);
        // No ring to hold the card up (another thread, or one with nothing reported yet).
        if context.is_none() {
            self.context_open = false;
        }
        let compact = self.width.get() < px(600.);
        // A side bar's width (the editor's AI side bar): the model pill drops its effort and
        // cuts its name shorter, so the send button stays in view.
        let narrow = self.width.get() < px(NARROW);
        let plan = prefs.plan;
        let preparing = live.is_some_and(|l| l.preparing);
        // Its agent's CLI is being updated: messages wait for the new version.
        let updating = thread
            .as_ref()
            .filter(|t| ws.agent_updating(&t.agent.key()))
            .map(|t| t.agent.display_name());
        let missing = thread.as_ref().and_then(|t| {
            Some((
                t.worktree
                    .clone()
                    .filter(|w| !preparing && crate::system::lately::worktree_missing(w))?,
                t.id.clone(),
            ))
        });
        // Another thread edits the same folder right now: offer a worktree for the next one.
        let crowded = thread
            .as_ref()
            .filter(|t| ws.sharing_folder(&t.id))
            .map(|t| ws.project_dir(t));
        // The way out is a worktree, for git projects.
        let crowded_repo = crowded
            .clone()
            .flatten()
            .filter(|p| crate::system::lately::exists(&p.join(".git")));

        // Model pill + menu, access pill + menu (`pickers`). Short labels (Consult, Plan) stay
        // until the card is side-bar narrow; the access level's long one goes first.
        let model_pill = pickers::model_pill(self, narrow, compact, false, cx);
        let access_pill = pickers::access_pill(self, compact, false, cx);
        let hh = prefs.hand_holding;

        // Round 30 px buttons: stop / send, the mic, and voice mode's ✕ and ✓.
        let square = |id: &'static str| look::circle(id);
        let blue = palette::send(cx);
        let on_blue = gpui_kit::white();
        // What the keys do, on the button: sending, a new line, and steer or queue mid-turn.
        let hint = delivery_hint(self.cmd_enter, follow_up, own_turn);
        let send = if running {
            square("stop")
                .test_support()
                .cursor_pointer()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        crate::keys::localize(&format!("Stop ⌘. · {hint}")).into_owned(),
                    )
                    .build(window, cx)
                })
                .bg(palette::red(cx))
                .hover(|s| s.opacity(0.9))
                .child(div().size(px(9.)).rounded(px(2.)).bg(on_blue))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = thread_id.clone() {
                        this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
                    }
                }))
                .into_any_element()
        } else {
            square("send")
                .test_support()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
                })
                // Blue at rest too, as the design draws it; it only answers the pointer when
                // there's something to send.
                .bg(blue)
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.9)))
                .child(
                    Icon::new(IconName::ArrowUp)
                        .size(px(16.))
                        .text_color(on_blue),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let input = this.input.clone();
                    this.submit(input, window, cx);
                }))
                .into_any_element()
        };

        // The mic opens voice mode when the bundle can ask for it (see dictate::available). Windows
        // has no mic button: Win+H is its own dictation, which types into the input.
        let asking = self.voice.phase == voice::Phase::Asking;
        let mic = dictate::available().then(|| {
            square("dictate")
                .test_support()
                .cursor_pointer()
                .hover(|s| s.bg(look::chip_fill(cx)))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if asking {
                        "Starting dictation…"
                    } else {
                        crate::words::words().dictate_tip
                    })
                    .build(window, cx)
                })
                .child(if asking {
                    gpui_kit::component::spinner::Spinner::new()
                        .xsmall()
                        .color(theme.muted_foreground)
                        .into_any_element()
                } else {
                    Icon::new(crate::assets::Lucide::Mic)
                        .size(px(look::ICON))
                        .text_color(theme.muted_foreground)
                        .into_any_element()
                })
                .when(!asking, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx)))
                })
                .into_any_element()
        });
        // Voice mode: the waveform, ✕ and ✓ in place of the toolbar.
        let voice_row = self.voice.active().then(|| {
            let finishing = self.voice.phase == voice::Phase::Finishing;
            // `#EDEDED`, a shade darker under the pointer.
            let (rest, hover) = (look::pressed(cx), theme.foreground.opacity(0.12));
            let cancel = square("voice-cancel")
                .test_support()
                .cursor_pointer()
                .bg(rest)
                .hover(move |s| s.bg(hover))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared("Discard (esc)"))
                        .build(window, cx)
                })
                .child(
                    Icon::new(IconName::Close)
                        .size(px(13.))
                        .text_color(theme.muted_foreground),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.voice_step(voice::Event::Cancel, window, cx)
                }));
            let confirm = square("voice-confirm")
                .test_support()
                .bg(blue)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared(if finishing {
                        "Finishing…"
                    } else {
                        "Insert the text (↩)" // keys::shared
                    }))
                    .build(window, cx)
                })
                .child(if finishing {
                    gpui_kit::component::spinner::Spinner::new()
                        .xsmall()
                        .color(on_blue)
                        .into_any_element()
                } else {
                    Icon::new(IconName::Check)
                        .size(px(15.))
                        .text_color(on_blue)
                        .into_any_element()
                })
                .when(!finishing, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.opacity(0.9))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.voice_step(voice::Event::Confirm, window, cx)
                        }))
                })
                // Fades in as the take opens.
                .with_animation(
                    ("voice-confirm-in", self.voice.take),
                    Animation::new(std::time::Duration::from_millis(800)),
                    |el, t| el.opacity(0.6 + 0.4 * t),
                );
            // The strip runs from the text's left edge to 16 px short of ✕; ✕ and ✓ sit where
            // the mic and send were, 4 px apart.
            h_flex()
                .id("voice-row")
                .test_support()
                .px(px(look::PAD))
                .pt(px(4.))
                .pb(px(look::PAD))
                .min_w_0()
                .child(voice::waveform(
                    self.voice.levels.bars().collect(),
                    theme.muted_foreground,
                ))
                .child(div().w(px(16.)).flex_none())
                .child(cancel)
                .child(div().w(px(4.)).flex_none())
                .child(confirm)
        });

        // Record the card's width; crossing the compact threshold re-renders the pills.
        let width_cell = self.width.clone();
        let me = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let before = width_cell.get();
                width_cell.set(bounds.size.width);
                let crossed = |at: f32| (before < px(at)) != (bounds.size.width < px(at));
                if crossed(600.) || crossed(NARROW) {
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
            // A near-white card on a 1 px hairline with a soft shadow (the design's composer).
            .rounded(px(look::RADIUS))
            .bg(if glass { look::card_fill(cx).opacity(0.82) } else { look::card_fill(cx) })
            .border_1()
            .border_color(if hh == HandHolding::FullAccess { palette::amber(cx).opacity(0.3) } else { look::hairline(cx) })
            .shadow(look::card_shadow(cx))
            // Context row (threads): checkout, branch, activity.
            .when(!is_draft, |el| {
                el.child(
                    h_flex()
                        .h(px(40.))
                        .pt(px(4.))
                        .px(px(look::PAD - 4.))
                        .gap(px(8.))
                        .text_size(px(look::LABEL))
                        .text_color(theme.muted_foreground)
                        .child(self.env_chips(false, cx))
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
            .child(div().px(px(if self.voice.active() { look::PAD } else { look::PAD - look::INPUT_INSET })).pt(px(if is_draft || !self.outbox.paths.is_empty() || self.editing.is_some() { look::PAD - 7. } else { 2. })).text_size(rems(look::TEXT)).when(self.voice.active(), |el| el.pb(px(6.))).map(|el| if self.voice.active() { el.child(self.voice_preview(window, cx)) } else { el.relative().child(Textarea::new(&self.input).appearance(false).text_size(rems(look::TEXT)).token(|token, _, cx| {
                // An accepted `/` command or skill: plain blue text, no pill.
                div().text_color(crate::palette::token(cx)).child(token.token().label().clone())
            }).on_paste({
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
            })).children(pickers::slash_ghost_overlay(self, cx)) }))
            .when_some(voice_row, |el, row| el.child(row))
            .when(!self.voice.active(), |el| el.child(
                // 30 px controls, 5 px apart, 13 px in from the card's edges.
                h_flex()
                    .px(px(look::PAD))
                    .pt(px(4.))
                    .pb(px(look::PAD))
                    .gap(px(look::GAP))
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.plus_button(cx))
                    .child(model_pill)
                    .child(self.consult_pill(narrow, cx))
                    .when(self.restate, |el| {
                        el.child(
                            Pill::new("restate-pill")
                                .chip()
                                .selected(true)
                                .tooltip("The agent says back what you asked, in its own words, before it does anything. Click to turn off.")
                                .child(Icon::new(crate::assets::Lucide::MessageSquareQuote).size(px(look::ICON)).text_color(theme.foreground.opacity(0.85)))
                                .when(!narrow, |p| p.child("Restate first"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.restate = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(access_pill)
                    // The same pill on a new thread and in one under way (⇧⇥ either way).
                    .child(
                        Pill::new("plan-pill")
                            .chip()
                            .selected(plan)
                            .tooltip(crate::keys::shared(if plan { "Plan mode is on: the agent plans before it changes anything (⇧⇥)" } else { "Plan mode: the agent plans before it changes anything (⇧⇥)" }))
                            .child(Icon::new(crate::assets::Lucide::ListChecks).size(px(look::ICON)).text_color(if plan { palette::indigo(cx) } else { theme.muted_foreground }))
                            .when(!narrow, |p| p.child(div().when(plan, |el| el.text_color(theme.foreground).font_medium()).child("Plan")))
                            .on_click(cx.listener(|this, _, _, cx| pickers::update_prefs(this, cx, |p| p.plan = !p.plan))),
                    )
                    .child(div().flex_1().min_w(px(4.)))
                    // Past `QUIET`, or while Compact now runs and says what it freed: the
                    // compaction itself drops the window below it, and the card must stay.
                    .when_some(
                        context.filter(|(used, window)| {
                            *used as f32 / *window as f32 >= crate::context_popover::QUIET
                                || !matches!(self.compact, crate::context_popover::Compact::Idle)
                                || self.context_open
                        }),
                        // The ring keeps its own trigger (`context_popover`); here it only gets
                        // the toolbar's 30 px row.
                        |el, (used, window)| {
                            el.child(
                                h_flex()
                                    .h(px(look::CONTROL))
                                    .flex_none()
                                    .items_center()
                                    .child(self.context_card(used, window, cx)),
                            )
                        },
                    )
                    .children(mic)
                    .child(send),
            ));

        let limit_bar = thread
            .as_ref()
            .and_then(|t| self.limit_bar(&t.id, compact, cx));
        // Capy-style context chips above the card on a new thread.
        // The AI side bar's new chat works in the IDE folder: it has no project to pick.
        let chips = is_draft.then(|| {
            div()
                .pb(px(8.))
                .child(self.env_chips(self.scope != Scope::Ide, cx))
        });
        let status_chips = self.chip_row(running, window, cx);
        let picker = pickers::picker(self, cx);
        let drop_tint = palette::ember(cx);
        let card = div()
            .relative()
            .w_full()
            .child(card)
            .when(picker.is_none(), |el| el.children(status_chips))
            .when_some(picker, |el, p| {
                el.child(
                    div()
                        .absolute()
                        .bottom_full()
                        .left_0()
                        .w_full()
                        .pb(px(9.))
                        .child(p),
                )
            })
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.opacity(0.85).border_color(drop_tint))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.add_paths(paths.paths(), window, cx)
            }));

        // Status strip under the card (threads and drafts).
        let status = Some({
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
                        .when_some(billing_tip, |el, tip| {
                            el.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            })
                        }),
                )
                .when_some(cost_label, |el, label| {
                    el.child(cost_chip(label, billing.clone(), spend, cx))
                })
                .when_some(cache_hits, |el, c| {
                    let tip = c.detail();
                    el.child(
                        div()
                            .id("cache-hits")
                            .test_support()
                            .flex_none()
                            .child(c.label())
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            }),
                    )
                })
                .when(queued > 0, |el| el.child(format!("{queued} queued")))
                .when(crowded.is_some(), |el| {
                    let ws = self.workspace.clone();
                    el.child(
                        h_flex()
                            .min_w_0()
                            .gap(px(6.))
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .xsmall()
                                    .text_color(palette::amber(cx)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child("Another thread is also editing this folder"),
                            )
                            .when_some(crowded_repo, |el, project| {
                                el.child(
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
                                            ws.update(cx, |ws, cx| {
                                                ws.new_thread_in_worktree(project, cx)
                                            })
                                        }),
                                )
                            }),
                    )
                })
                .child(div().flex_1())
                // The terminal docks under the chat in the main window (⌃` too); lit while it's out.
                .when(main, |el| {
                    let open = self.workspace.read(cx).settings.layout.terminal_open;
                    let tip = crate::keys::shared(if open {
                        "Hide the terminal (⌃`)" // keys::shared
                    } else {
                        "A terminal under the chat (⌃`)" // keys::shared
                    });
                    el.child(
                        h_flex()
                            .id("open-terminal")
                            .test_support()
                            .gap(px(6.))
                            .cursor_pointer()
                            .when(open, |el| el.text_color(theme.foreground))
                            .hover(|s| s.text_color(theme.foreground))
                            .child(Icon::new(IconName::SquareTerminal).xsmall())
                            .child("Terminal")
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::ToggleTerminal), cx)
                            }),
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
            // Less room around it in the editor's side bar, where every point counts.
            .when(self.scope == Scope::Ide, |el| el.px_3())
            .when(self.scope != Scope::Ide, |el| el.px_6())
            .pb(px(if is_draft { 0. } else { 12. }))
            .child(measure_height)
            .key_context("Composer")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // ⌥↩ sends the other way from the default while a turn runs (steer ⇄ queue).
                let k = &ev.keystroke;
                // Voice mode: esc is ✕ and ↩ is ✓.
                if this.voice.active()
                    && !k.modifiers.modified()
                    && matches!(k.key.as_str(), "escape" | "enter")
                {
                    cx.stop_propagation();
                    let event = if k.key == "escape" {
                        voice::Event::Cancel
                    } else {
                        voice::Event::Confirm
                    };
                    this.voice_step(event, window, cx);
                    return;
                }
                if k.key == "enter"
                    && k.modifiers.alt
                    && !k.modifiers.secondary()
                    && !k.modifiers.shift
                    && this.pickers.trigger.is_none()
                {
                    cx.stop_propagation();
                    let other = match this.workspace.read(cx).settings.general.follow_up {
                        FollowUp::Steer => FollowUp::Queue,
                        FollowUp::Queue => FollowUp::Steer,
                    };
                    let input = this.input.clone();
                    this.submit_as(input, Some(other), window, cx);
                    return;
                }
                pickers::picker_key(this, ev, window, cx)
            }))
            // The textarea binds these keys to actions, which never reach key listeners.
            .capture_action(
                cx.listener(|this, _: &MoveUp, window, cx| this.picker_action("up", window, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MoveDown, window, cx| {
                    this.picker_action("down", window, cx)
                }),
            )
            .capture_action(
                cx.listener(|this, _: &Escape, window, cx| {
                    this.picker_action("escape", window, cx)
                }),
            )
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                this.picker_action("tab", window, cx)
            }))
            .capture_action(
                cx.listener(|this, action: &Enter, window, cx| this.on_enter(action, window, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePlan, _, cx| {
                pickers::update_prefs(this, cx, |p| p.plan = !p.plan)
            }))
            .child(
                v_flex()
                    .w_full()
                    .max_w(self.workspace.read(cx).column())
                    .children(chips)
                    .children(limit_bar)
                    .child(card)
                    .children(status),
            )
    }
}

/// The prompt's placeholder: what to type, or mid-turn (`running`: the default way a message
/// goes), where a message sent now goes.
fn placeholder(running: Option<FollowUp>) -> std::borrow::Cow<'static, str> {
    match running {
        None => "Ask or build · / commands · @ files".into(),
        Some(FollowUp::Steer) => {
            crate::keys::localize("Steer the running turn · ⌥↩ queues for after it")
        }
        Some(FollowUp::Queue) => {
            crate::keys::localize("Queue a follow-up · ⌥↩ steers the running turn")
        }
    }
}

/// The send button's tooltip: which keys send and add a line, and while a turn runs (`running`),
/// which key steers it and which queues for after it.
pub fn delivery_hint(cmd_enter: bool, follow: FollowUp, running: bool) -> String {
    let (send, line) = if cmd_enter {
        ("⌘↩", "↩") // keys::localize below
    } else {
        ("↩", "⇧↩") // keys::localize below
    }; // keys::localize below
    let text = if !running {
        format!("Send {send} · New line {line}")
    } else {
        match follow {
            FollowUp::Steer => format!("{send} steers the running turn · ⌥↩ queues for after it"), // keys::localize below
            FollowUp::Queue => format!("{send} queues for after the turn · ⌥↩ steers it now"), // keys::localize below
        }
    };
    crate::keys::localize(&text).into_owned()
}

/// The API cost estimate in the status strip, with its breakdown on hover (worked out only then).
fn cost_chip(
    label: String,
    billing: Option<trek_agents::Billing>,
    spend: crate::cost::ThreadSpend,
    cx: &App,
) -> AnyElement {
    let hover = cx.theme().foreground;
    // TREK_HOVER_COST=<seconds> hovers the first estimate drawn, once, that long after it's
    // drawn (once the agents have said how they're billed), so its breakdown can be reviewed in
    // a window that isn't in front, without moving the real pointer.
    static HOVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static DELAY: std::sync::LazyLock<Option<std::time::Duration>> =
        std::sync::LazyLock::new(|| {
            std::env::var("TREK_HOVER_COST")
                .ok()
                .map(|s| std::time::Duration::from_secs(s.trim().parse().unwrap_or(0)))
        });
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
                                        window.dispatch_event(
                                            PlatformInput::MouseMove(MouseMoveEvent {
                                                position,
                                                pressed_button: None,
                                                modifiers: Default::default(),
                                            }),
                                            cx,
                                        );
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
            gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                tip.as_ref()
                    .map_or_else(|| div().into_any_element(), |b| cost_breakdown(b, cx))
            })
            .build(window, cx)
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
            .child(
                h_flex()
                    .gap(px(12.))
                    .text_xs()
                    .font_medium()
                    .child(div().flex_1().child(m.name.clone()))
                    .child(m.amount.clone()),
            )
            .children(m.rows.iter().map(|r| {
                h_flex()
                    .gap(px(8.))
                    .text_xs()
                    .text_color(muted)
                    .child(div().w(px(76.)).child(r.kind))
                    .child(
                        div()
                            .w(px(44.))
                            .flex()
                            .justify_end()
                            .child(r.tokens.clone()),
                    )
                    .child(
                        div().pl(px(4.)).child(
                            r.rate
                                .clone()
                                .map(|r| format!("× {r}"))
                                .unwrap_or_else(|| "—".into()),
                        ),
                    )
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
                .when_some(b.billing.clone(), |el, l| {
                    el.child(div().text_xs().text_color(muted).child(l))
                }),
        )
        .when(!b.models.is_empty(), |el| {
            el.child(
                v_flex()
                    .gap(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(rule)
                    .children(models),
            )
        })
        .when(b.subs.is_some() || b.unpriced.is_some(), |el| {
            el.child(
                v_flex()
                    .gap(px(2.))
                    .text_xs()
                    .when_some(b.subs.clone(), |el, s| el.child(s))
                    .when_some(b.unpriced.clone(), |el, u| {
                        el.child(div().text_color(muted).child(u))
                    }),
            )
        })
        .when(!b.sources.is_empty(), |el| {
            el.child(
                v_flex()
                    .gap(px(2.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(rule)
                    .text_xs()
                    .text_color(muted)
                    .children(b.sources.iter().cloned()),
            )
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
        assert_eq!(
            restate_command("/restate why does it flicker?"),
            Some(Some("why does it flicker?".into()))
        );
        assert_eq!(restate_command("/restate: why?"), Some(Some("why?".into())));
        assert_eq!(restate_command("/restated"), None);
        assert_eq!(restate_command("please /restate"), None);
    }

    #[test]
    fn consult_splits_at_a_colon_and_space() {
        assert_eq!(
            consult_split(" sol high: why?"),
            Some((" sol high", " why?"))
        );
        assert_eq!(
            consult_split(" qwen2.5-coder:7b high: why?"),
            Some((" qwen2.5-coder:7b high", " why?")),
            "a model id's colon stays"
        );
        assert_eq!(consult_split(" sol:"), Some((" sol", "")));
        assert_eq!(consult_split(" qwen2.5-coder:7b"), None);
        assert_eq!(consult_split(" sol"), None);
    }
}

/// The changes chip: "4 files +128 −36 ■■■■□ ⌃", the thread's pending edits; opens the review.
fn changes_chip(
    files: usize,
    added: u32,
    removed: u32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (green, red) = (palette::emerald(cx), palette::red(cx));
    // The squares in the reference's bright green and red (#38DE6F, #DB3B4F) on a light page
    // (the palette's are for text there, and go muddy as small blocks).
    let (sq_green, sq_red) = if theme.mode.is_dark() {
        (green, red)
    } else {
        (rgb(0x38DE6F).into(), rgb(0xDB3B4F).into())
    };
    // Five squares split by what was added and removed, as the reference's.
    let total = (added + removed).max(1) as f32;
    let greens = if added + removed == 0 {
        0
    } else {
        ((added as f32 / total) * 5.).round().clamp(0., 5.) as usize
    };
    let square = |c: Hsla| div().size(px(6.)).rounded(px(1.5)).bg(c);
    h_flex()
        .id("changes-chip")
        .test_support()
        .aria_label(format!("{files} files changed"))
        .flex_none()
        .h(px(31.))
        .px(px(12.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .bg(crate::pr_card::chip_fill(cx))
        .border_1()
        .border_color(crate::pr_card::chip_border(cx))
        .shadow(crate::pr_card::chip_shadow())
        .cursor_pointer()
        .hover(|s| s.border_color(theme.foreground.opacity(0.16)))
        .text_size(px(13.))
        .child(
            Icon::new(crate::assets::Lucide::FileText)
                .size(px(12.))
                .text_color(theme.muted_foreground),
        )
        .child(div().text_color(theme.foreground).child(if files == 1 {
            "1 file".to_string()
        } else {
            format!("{files} files")
        }))
        .child(div().text_color(green).child(format!("+{added}")))
        .child(div().text_color(red).child(format!("−{removed}")))
        .child(h_flex().gap(px(2.)).children((0..5).map(|i| {
            square(if added + removed == 0 {
                theme.foreground.opacity(0.15)
            } else if i < greens {
                sq_green
            } else {
                sq_red
            })
        })))
        .child(
            Icon::new(IconName::ChevronUp)
                .size(px(11.))
                .text_color(theme.muted_foreground.opacity(0.6)),
        )
        .on_click(on_click)
        .into_any_element()
}

/// A dark action pill (the reference's "Create PR"): the page's ink as its fill; with a white
/// spinner while what it started is under way.
fn dark_chip(
    id: &'static str,
    busy: bool,
    label: &'static str,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (fg, bg) = (theme.background, theme.foreground);
    h_flex()
        .id(id)
        .test_support()
        .aria_label(label)
        .flex_none()
        .h(px(31.))
        .px(px(12.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .bg(bg.opacity(0.92))
        .when(!busy, |el| {
            el.hover(move |s| s.bg(bg.opacity(0.8))).cursor_pointer()
        })
        .shadow(crate::pr_card::chip_shadow())
        .text_size(px(13.))
        .font_medium()
        .text_color(fg)
        .child(if busy {
            crate::pr_card::ring_spinner(12., fg, cx)
        } else {
            Icon::new(crate::assets::Lucide::GitPullRequest)
                .size(px(12.))
                .text_color(fg)
                .into_any_element()
        })
        .child(label)
        .when_some(on_click, |el, f| el.on_click(f))
        .into_any_element()
}
