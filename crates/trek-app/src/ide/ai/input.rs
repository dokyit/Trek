//! The AI side bar's input: context chips over the text (the file in front in the editor,
//! attached by itself; lines picked with ⌘L / ⌘⇧L; files added with + or the paperclip), then a
//! bar with the mode (Agent, Plan, Ask), the model, access, the context ring, attach, and send
//! or stop. Return sends; while a turn runs it queues the message for after it, and ⌘Return
//! steers the turn with it now (with "Send with ⌘↩" on: Return is a new line, ⌘Return sends and
//! ⌘⇧Return steers). With nothing typed and changes pending, ⌘Return keeps them all and ⌘⇧⌫
//! undoes them; ⌘⇧⌫ otherwise stops the turn.
//!
//! Each chat tab has its own draft: text, chips, images, a message being edited, Restate first
//! and the models picked to consult stay with their tab. Editing a message works as the
//! harness's composer does: the draft is put aside (and back on Esc, or once the edit is sent),
//! "Restore N files" says what putting the files back would change and can be turned off, and a
//! send that doesn't go through keeps the text. `/consult`, `/restate`, `/new` and `/clear` are
//! Trek's (`commands`).
//!
//! The model and access menus and the `/` `@` `$` picker are the harness composer's
//! (`composer::pickers`).

use super::context::{self, ContextChip};
use super::restore::{self, Files};
use std::collections::HashMap;
use trek_core::orchestrate::Consult;
use crate::attachments::{self, Attaching, Outbox};
use crate::composer::{PickerHost, Pickers, pickers};
use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{ChatMode, Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, OutdentInline, Textarea, TextareaState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::RunState;
use trek_core::settings::FollowUp;

actions!(ai_input, [UndoAllOrStop]);

const PLACEHOLDER: &str = "Ask, plan or build… @ for context, / for commands";
/// The placeholder in a narrow side bar, where the full one would be cut off.
const PLACEHOLDER_NARROW: &str = "Ask, plan or build…";
/// Below this side-bar width the pills go compact: the mode as its icon, the model without effort.
const NARROW: f32 = 420.;

pub struct AiInput {
    workspace: Entity<Workspace>,
    /// The side bar is narrow (`NARROW`): compact pills and a short placeholder.
    narrow: bool,
    scope: Scope,
    input: Entity<TextareaState>,
    pickers: Pickers,
    /// Images going out with the next message.
    outbox: Outbox,
    /// The file in front in the editor: attached by itself, unless its chip was removed.
    current: Option<PathBuf>,
    /// The current file's chip was removed; it stays off until another file comes to the front.
    current_off: bool,
    /// Chips added on purpose: lines, files. They go with the next message.
    chips: Vec<ContextChip>,
    /// The message being edited, to send again in its place.
    editing: Option<Editing>,
    /// Ask the agent to restate the next message first (`/restate`, "Not quite…").
    restate: bool,
    /// Models to consult with the next message (`/consult <models>`).
    consult: Option<Consult>,
    /// The chat tab the input holds the draft of (`draft_key`), and the others' drafts.
    tab: String,
    drafts: HashMap<String, Draft>,
    /// `general.send_with_cmd_enter`.
    cmd_enter: bool,
    mode_open: bool,
    _subscriptions: Vec<Subscription>,
}

/// A chat tab's draft, put aside while another tab is in front.
#[derive(Default)]
struct Draft {
    text: String,
    images: Vec<PathBuf>,
    chips: Vec<ContextChip>,
    current_off: bool,
    editing: Option<Editing>,
    restate: bool,
    consult: Option<Consult>,
}

/// A message of yours the input holds, to send again in its place.
struct Editing {
    thread: String,
    item: String,
    /// Put the files back as they were when it was first sent; `None` with no checkpoint.
    restore: Option<bool>,
    /// What putting them back would change.
    files: Files,
    /// What the input held before: back on Esc, and once the edit is sent.
    draft: Box<Draft>,
    _check: Option<Task<()>>,
}

/// Which draft the AI side bar's tab in front has: its thread's, or the folder's new chat's.
fn draft_key(ws: &Workspace) -> String {
    match ws.ide_chat.active_thread() {
        Some(id) => format!("thread:{id}"),
        None => format!("ide-draft:{}", ws.ide_root.as_ref().map(|r| r.display().to_string()).unwrap_or_default()),
    }
}

impl AiInput {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cmd_enter = workspace.read(cx).settings.general.send_with_cmd_enter;
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 10).submit_on_enter(!cmd_enter).placeholder(PLACEHOLDER));
        let pickers = Pickers::new(window, cx);
        let tab = draft_key(workspace.read(cx));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &gpui_kit::component::input::InputEvent, window, cx| {
                use gpui_kit::component::input::InputEvent;
                match event {
                    InputEvent::PressEnter { shift: false, secondary: false } if !this.cmd_enter => {
                        let swallowed = this.pickers.swallowed();
                        if this.pickers.trigger.is_none() && !swallowed {
                            let _ = state;
                            this.submit(FollowUp::Queue, window, cx);
                        }
                    }
                    InputEvent::Change => {
                        pickers::update_trigger(this, cx);
                        let typing = {
                            let v = state.read(cx).value();
                            v.trim().len() >= 2 && !v.starts_with('/')
                        };
                        if typing && this.editing.is_none() {
                            this.workspace.update(cx, |ws, cx| ws.warm_up_in(&Scope::Ide, cx));
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }),
            cx.observe_in(&workspace, window, |this, ws, window, cx| {
                let cmd_enter = ws.read(cx).settings.general.send_with_cmd_enter;
                if cmd_enter != this.cmd_enter {
                    this.cmd_enter = cmd_enter;
                    this.input.update(cx, |s, cx| s.set_submit_on_enter(!cmd_enter, cx));
                }
                // Another chat tab came to the front: its own draft comes back.
                let key = draft_key(ws.read(cx));
                if key != this.tab {
                    this.switch_draft(key, window, cx);
                }
                // The message being edited is gone (a rewind took it): what was put aside comes back.
                let stale = this.editing.as_ref().is_some_and(|e| {
                    let ws = ws.read(cx);
                    ws.thread_id_in(&Scope::Ide) != Some(e.thread.as_str()) || ws.live.get(&e.thread).is_none_or(|l| l.items.position(&e.item).is_none())
                });
                if stale {
                    this.cancel_edit(window, cx);
                }
                cx.notify();
            }),
            cx.observe(pickers.model_search(), |_, _, cx| cx.notify()),
            cx.observe(&input, |_, _, cx| cx.notify()),
        ];
        Self {
            workspace,
            narrow: false,
            scope: Scope::Ide,
            input,
            pickers,
            outbox: Outbox::default(),
            current: None,
            current_off: false,
            chips: vec![],
            editing: None,
            restate: false,
            consult: None,
            tab,
            drafts: HashMap::new(),
            cmd_enter,
            mode_open: false,
            _subscriptions: subscriptions,
        }
    }

    /// What the input holds now, taken out of it (it's left empty).
    fn take_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Draft {
        let text = self.input.read(cx).value().to_string();
        if !text.is_empty() {
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        Draft {
            text,
            images: std::mem::take(&mut self.outbox.paths),
            chips: std::mem::take(&mut self.chips),
            current_off: std::mem::take(&mut self.current_off),
            editing: self.editing.take(),
            restate: std::mem::take(&mut self.restate),
            consult: self.consult.take(),
        }
    }

    /// Put `draft` in the input.
    fn put_draft(&mut self, draft: Draft, window: &mut Window, cx: &mut Context<Self>) {
        let text = draft.text;
        self.input.update(cx, |s, cx| {
            s.set_value(text.clone(), window, cx);
            s.set_selected_range(text.len()..text.len(), cx);
        });
        self.outbox.paths = draft.images;
        self.chips = draft.chips;
        self.current_off = draft.current_off;
        self.editing = draft.editing;
        self.restate = draft.restate;
        self.consult = draft.consult;
        self.pickers.trigger = None;
    }

    /// Chat tab `key` came to the front: the draft in the input goes aside with its tab, and
    /// `key`'s comes back.
    fn switch_draft(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let old = std::mem::replace(&mut self.tab, key.clone());
        let draft = self.take_draft(window, cx);
        if !draft.text.is_empty() || !draft.images.is_empty() || !draft.chips.is_empty() || draft.editing.is_some() || draft.restate || draft.consult.is_some() || draft.current_off {
            self.drafts.insert(old, draft);
        }
        let next = self.drafts.remove(&key).unwrap_or_default();
        self.put_draft(next, window, cx);
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    /// The file in front in the editor changed (`None`: no file, or a Review tab).
    pub fn set_current_file(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.current != path {
            self.current = path;
            self.current_off = false;
            cx.notify();
        }
    }

    /// Add a chip (⌘L lines, a file); one already there isn't added twice.
    pub fn add_chip(&mut self, chip: ContextChip, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chips.contains(&chip) {
            self.chips.push(chip);
        }
        self.focus(window, cx);
        cx.notify();
    }

    /// What goes with the next message: the current file (unless removed, or added on purpose
    /// already), then the chips added.
    pub fn attached(&self) -> Vec<ContextChip> {
        let mut out = vec![];
        if let Some(path) = self.current.clone().filter(|_| !self.current_off) {
            if !self.chips.iter().any(|c| matches!(c, ContextChip::File { path: p } if *p == path)) {
                out.push(ContextChip::File { path });
            }
        }
        out.extend(self.chips.iter().cloned());
        out
    }

    /// Send `text` with `chips` (and nothing typed) right away: "Fix with Agent" in Problems.
    /// It goes to the chat in front, as Return would send it (queued while a turn runs).
    pub fn send_request(&mut self, text: &str, chips: &[ContextChip], cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let text = context::with_context(text, chips, ws.cwd_in(&Scope::Ide).as_deref());
        self.workspace.update(cx, |ws, cx| ws.send_ide(text, vec![], FollowUp::Queue, cx));
        cx.notify();
    }

    /// Insert text at the cursor (a browser pick, a message a worktree couldn't take).
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            let value = s.value().to_string();
            let cursor = s.cursor().min(value.len());
            let needs_break = cursor > 0 && !value[..cursor].ends_with('\n');
            s.insert(if needs_break { format!("\n{text}") } else { text.to_string() }, window, cx);
        });
        self.focus(window, cx);
        cx.notify();
    }

    pub fn attach_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.outbox.add([path]);
        cx.notify();
    }

    /// Follow-ups that never went out come back.
    pub fn restore(&mut self, text: &str, images: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let said = context::split(text).said.to_string();
        if !said.is_empty() {
            self.insert_text(&said, window, cx);
        }
        for path in images {
            self.attach_image(path.clone(), cx);
        }
    }

    /// A message comes back: taken back by a rewind (into the input, after what's typed), or
    /// (`edit`: its item) to edit and send again in its place, the draft put aside meanwhile.
    /// What the side bar added to it stays off (the chips are today's); consultants it was sent
    /// with and a restatement it asked for are picked again.
    pub fn compose(&mut self, thread: &str, text: &str, images: &[PathBuf], edit: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (text, consult) = trek_core::orchestrate::split_consult(text);
        let (text, restate) = trek_core::restate::split_restate(text);
        let sent = context::split(text);
        let said = sent.said.to_string();
        if sent.ask {
            self.workspace.update(cx, |ws, cx| ws.set_chat_mode_in(&Scope::Ide, ChatMode::Ask, cx));
        }
        let Some(item) = edit else {
            self.consult = consult.or(self.consult.take());
            self.restate |= restate;
            self.restore(&said, images, window, cx);
            return;
        };
        let draft = match self.editing.take() {
            Some(e) => *e.draft,
            None => self.take_draft(window, cx),
        };
        // Which files sending it would put back, found off the main thread.
        let (files, check) = match restore::checkpoint(self.workspace.read(cx), thread, &item) {
            Ok((repo, sha)) => {
                let task = cx.spawn(async move |this, cx| {
                    let files = cx.background_executor().spawn(async move { restore::read(repo, sha) }).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(e) = this.editing.as_mut() {
                            e.files = files;
                            cx.notify();
                        }
                    });
                });
                (Files::Checking, Some(task))
            }
            Err(why) => (Files::Unavailable(why), None),
        };
        let restorable = matches!(files, Files::Checking);
        self.editing = Some(Editing { thread: thread.to_string(), item, restore: restorable.then_some(true), files, draft: Box::new(draft), _check: check });
        self.consult = consult;
        self.restate = restate;
        self.outbox.paths = images.to_vec();
        self.input.update(cx, |s, cx| {
            s.set_value(said.clone(), window, cx);
            s.set_selected_range(said.len()..said.len(), cx);
        });
        self.pickers.trigger = None;
        self.focus(window, cx);
        cx.notify();
    }

    /// Stop editing: what the input held before comes back.
    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.editing.take() else { return };
        self.put_draft(*e.draft, window, cx);
        cx.notify();
    }

    /// "Not quite…" on a restatement: the correction is restated too.
    pub fn correct(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restate = true;
        self.focus(window, cx);
        cx.notify();
    }

    /// Type `text` and send it, as Return would (the shots harness).
    #[cfg(feature = "shots")]
    pub fn send_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        self.submit(FollowUp::Queue, window, cx);
    }

    /// Send what's typed, with the chips: `follow` says what it does while a turn runs. Trek's
    /// own commands are taken here first.
    fn submit(&mut self, follow: FollowUp, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target(cx);
        if self.outbox.hold_send(target) {
            cx.notify();
            return;
        }
        let raw = self.input.read(cx).value().to_string();
        if raw.trim().is_empty() && self.outbox.paths.is_empty() {
            return;
        }
        let toast = |this: &mut Self, message: String, cx: &mut Context<Self>| this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
        let clear = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.input.update(cx, |s, cx| s.set_value("", window, cx));
            this.pickers.trigger = None;
            cx.notify();
        };
        // `/new`, `/clear`: a new chat here (this one's draft stays with it, empty).
        if super::commands::is_new_chat(&raw) && self.editing.is_none() {
            clear(self, window, cx);
            self.workspace.update(cx, |ws, cx| ws.ide_new_chat(cx));
            return;
        }
        // `/consult <models>[: <message>]` picks consultants, and sends when there's a message.
        let mut consult = self.consult.clone();
        let mut text = raw.clone();
        let picked = super::commands::consult_command(self.workspace.read(cx), &raw, self.consult.clone().unwrap_or(Consult { implement: true, ..Default::default() }));
        match picked {
            Some(Err(why)) => return toast(self, why, cx),
            Some(Ok((c, None))) => {
                self.consult = Some(c);
                return clear(self, window, cx);
            }
            Some(Ok((c, Some(message)))) => {
                consult = Some(c);
                text = message;
            }
            None => {}
        }
        // `/restate <message>` asks for a restatement first; `/restate` alone, in a chat under
        // way, asks the agent to restate the chat so far, and in a new one turns it on (or off)
        // for the first message.
        let mut restate = self.restate;
        let under_way = self.editing.is_none() && self.workspace.read(cx).thread_in(&Scope::Ide).is_some();
        match crate::composer::restate_command(&text) {
            Some(None) if under_way => {
                restate = true;
                text = trek_core::restate::THREAD.to_string();
            }
            Some(None) => {
                self.restate = !self.restate;
                return clear(self, window, cx);
            }
            Some(Some(message)) => {
                restate = true;
                text = message;
            }
            None => {}
        }
        let ws = self.workspace.read(cx);
        // Another command (Trek's, or the agent's) goes as typed.
        let command = text.trim_start().starts_with('/');
        if !command {
            text = context::with_context(&text, &self.attached(), ws.cwd_in(&Scope::Ide).as_deref());
            if ws.chat_mode_in(&Scope::Ide) == ChatMode::Ask {
                text = context::with_ask(&text);
            }
            if restate {
                text = trek_core::restate::with_restate(&text);
            }
            if let Some(c) = consult.as_ref() {
                if let Some(why) = ws.consult_unavailable(&ws.prefs_in(&Scope::Ide).agent) {
                    return toast(self, format!("Can't consult: {why}."), cx);
                }
                match super::commands::consult_prompt(ws, &text, c) {
                    Ok(t) => text = t,
                    Err(why) => return toast(self, why, cx),
                }
            }
        }
        if let Some(e) = &self.editing {
            let why = if ws.turn_running(&e.thread) {
                Some("Stop the running turn to send the edited message.")
            } else if !ws.can_rewind(&e.thread, &e.item) {
                Some("That message can't be edited any more.")
            } else {
                None
            };
            if let Some(message) = why {
                return toast(self, message.into(), cx);
            }
        }
        let images = self.outbox.paths.clone();
        let sent = match self.editing.as_ref() {
            // Sent in place of the message: the chat goes back to just before it, the files too
            // when picked. What the input held before the edit comes back; a send that doesn't go
            // through keeps the edit as it was.
            Some(e) => {
                let restore = e.restore == Some(true) && !e.files.nothing();
                let (thread, item) = (e.thread.clone(), e.item.clone());
                let sent = self.workspace.update(cx, |ws, cx| ws.edit_and_resend(&thread, &item, text, images, restore, cx));
                if sent {
                    if let Some(e) = self.editing.take() {
                        self.put_draft(*e.draft, window, cx);
                    }
                }
                sent
            }
            None => {
                clear(self, window, cx);
                self.chips.clear();
                self.outbox.paths.clear();
                self.workspace.update(cx, |ws, cx| ws.send_ide(text, images, follow, cx));
                true
            }
        };
        if sent {
            if restate {
                self.restate = false;
            }
            if consult.is_some() {
                self.consult = None;
            }
        }
        cx.notify();
    }

    /// The chat's thread, its turn running (or its sub-agents, with nothing typed), and its
    /// changes pending review.
    fn state(&self, cx: &App) -> (Option<String>, bool, bool) {
        let ws = self.workspace.read(cx);
        let thread = ws.thread_in(&Scope::Ide);
        let own = thread.is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou)) || thread.is_some_and(|t| ws.turn_running(&t.id));
        let children = thread.is_some_and(|t| !ws.running_children(&t.id).is_empty() || ws.waiting(&t.id));
        let pending = thread.is_some_and(|t| !ws.pending_files(&t.id).is_empty());
        (thread.map(|t| t.id.clone()), own || (children && self.is_empty(cx)), pending)
    }

    fn is_empty(&self, cx: &App) -> bool {
        self.input.read(cx).value().trim().is_empty() && self.outbox.paths.is_empty() && self.outbox.saving == 0
    }

    /// ⌘Return: keep every pending change when nothing is typed and no turn runs; else send now
    /// (steering a running turn).
    fn keep_all_or_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (thread, running, pending) = self.state(cx);
        if self.is_empty(cx) {
            if let (Some(id), true, false) = (thread, pending, running) {
                self.workspace.update(cx, |ws, cx| ws.keep_files(&id, None, cx));
            }
            return;
        }
        // With ⌘↩ the way to send, it queues as Return otherwise would; ⌘⇧↩ steers.
        self.submit(if self.cmd_enter { FollowUp::Queue } else { FollowUp::Steer }, window, cx);
    }

    /// ⌘⇧⌫: undo every pending change when nothing is typed and no turn runs; else stop.
    fn undo_all_or_stop(&mut self, cx: &mut Context<Self>) {
        let (thread, running, pending) = self.state(cx);
        let Some(id) = thread else { return };
        if running {
            self.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
        } else if pending && self.is_empty(cx) {
            self.workspace.update(cx, |ws, cx| ws.undo_files(&id, None, cx));
        }
    }

    /// Enter reaches here before the textarea: it picks a row while the picker is open; ⌘↩ is
    /// `keep_all_or_send`.
    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.pickers.trigger.is_some() && !action.shift && !action.secondary {
            pickers::picker_action(self, "enter", window, cx);
        } else if action.secondary && !action.shift {
            cx.stop_propagation();
            self.keep_all_or_send(window, cx);
        } else if action.secondary && action.shift && self.cmd_enter {
            // ⌘⇧↩ steers when ⌘↩ is what sends.
            cx.stop_propagation();
            self.submit(FollowUp::Steer, window, cx);
        }
    }

    fn picker_action(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if key == "escape" && self.pickers.trigger.is_none() && self.editing.is_some() {
            cx.stop_propagation();
            self.cancel_edit(window, cx);
            return;
        }
        pickers::picker_action(self, key, window, cx);
    }

    /// Pick files to attach: images go with the message, others become chips.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let (images, files) = attachments::split_files(paths);
                let _ = this.update_in(cx, |this, window, cx| {
                    attachments::attach_files(this, images, window, cx);
                    for path in files {
                        this.add_chip(ContextChip::File { path }, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn set_mode(&mut self, mode: ChatMode, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.set_chat_mode_in(&Scope::Ide, mode, cx));
        self.mode_open = false;
        cx.notify();
    }

    /// The chips over the text: the current file (dimmed, dashed: it goes unless removed), the
    /// ones added, and + for the `@` picker.
    fn chips_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let line = theme.foreground.opacity(0.12);
        let chip = |id: SharedString, icon: AnyElement, label: String, tip: String| {
            h_flex()
                .id(id)
                .test_support()
                .group("ai-chip")
                .flex_none()
                .max_w(px(200.))
                .h(px(20.))
                .pl(px(5.))
                .pr(px(3.))
                .gap(px(4.))
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(line)
                .bg(theme.foreground.opacity(0.04))
                .text_size(px(11.5))
                .child(icon)
                .child(div().min_w_0().truncate().child(label))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        };
        let close = |id: SharedString| {
            div()
                .id(id)
                .test_support()
                .size(px(14.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                .child(Icon::new(IconName::Close).size(px(10.)).text_color(theme.muted_foreground))
        };
        let current = self.current.clone().filter(|p| !self.current_off && !self.chips.iter().any(|c| matches!(c, ContextChip::File { path } if path == p)));
        let mut row = h_flex().id("ai-chips").w_full().gap(px(5.)).flex_wrap();
        if let Some(path) = current {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            row = row.child(
                chip("ai-chip-current".into(), crate::file_icon::badge(&name, px(11.), cx), name, format!("{}\nThe file in front goes with your message", path.display()))
                    .border_dashed()
                    .opacity(0.65)
                    .child(close("ai-chip-current-off".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.current_off = true;
                        cx.notify();
                    }))),
            );
        }
        for (ix, c) in self.chips.iter().enumerate() {
            let icon = match c {
                ContextChip::File { path } => crate::file_icon::badge(&path.to_string_lossy(), px(11.), cx),
                ContextChip::Selection { .. } => Icon::new(crate::assets::Lucide::TextCursorInput).size(px(11.)).text_color(theme.muted_foreground).into_any_element(),
                ContextChip::Problem { .. } => Icon::new(IconName::TriangleAlert).size(px(11.)).text_color(palette::amber(cx)).into_any_element(),
                ContextChip::Terminal { .. } => Icon::new(IconName::SquareTerminal).size(px(11.)).text_color(theme.muted_foreground).into_any_element(),
            };
            row = row.child(
                chip(SharedString::from(format!("ai-chip-{ix}")), icon, c.label(), c.tip())
                    .child(close(SharedString::from(format!("ai-chip-remove-{ix}"))).on_click(cx.listener(move |this, _, _, cx| {
                        if ix < this.chips.len() {
                            this.chips.remove(ix);
                        }
                        cx.notify();
                    }))),
            );
        }
        // What the next message asks for besides: a restatement first, other models' views.
        if self.restate {
            row = row.child(
                chip("ai-chip-restate".into(), Icon::new(crate::assets::Lucide::MessageSquareQuote).size(px(11.)).text_color(palette::ember(cx)).into_any_element(), "Restate first".into(), "The agent says back what you asked before it starts (/restate)".into())
                    .child(close("ai-chip-restate-off".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.restate = false;
                        cx.notify();
                    }))),
            );
        }
        if let Some(c) = &self.consult {
            let ws = self.workspace.read(cx);
            let names: Vec<String> = c.consultants.iter().map(|c| super::commands::consultant_name(ws, c)).collect();
            row = row.child(
                chip("ai-chip-consult".into(), Icon::new(crate::assets::Lucide::Users).size(px(11.)).text_color(palette::ember(cx)).into_any_element(), format!("Consult {}", names.join(", ")), "These models are consulted with the next message (/consult)".into())
                    .child(close("ai-chip-consult-off".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.consult = None;
                        cx.notify();
                    }))),
            );
        }
        row.child(
            div()
                .id("ai-chip-add")
                .test_support()
                .flex_none()
                .h(px(20.))
                .w(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_color(line)
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Add context (@)").build(window, cx))
                .child(Icon::new(IconName::Plus).size(px(11.)).text_color(theme.muted_foreground))
                .on_click(cx.listener(|this, _, window, cx| pickers::insert_trigger(this, "@", window, cx))),
        )
    }

    /// The mode pill: Agent, Plan or Ask.
    fn mode_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.workspace.read(cx).chat_mode_in(&Scope::Ide);
        let theme = cx.theme().clone();
        let icon = |m: ChatMode| match m {
            ChatMode::Agent => Icon::new(crate::assets::Lucide::Infinity),
            ChatMode::Plan => Icon::new(crate::assets::Lucide::ListChecks),
            ChatMode::Ask => Icon::new(crate::assets::Lucide::MessageCircleQuestionMark),
        };
        let tint = match mode {
            ChatMode::Agent => theme.muted_foreground,
            ChatMode::Plan => palette::indigo(cx),
            ChatMode::Ask => palette::sky(cx),
        };
        let me = cx.entity();
        Popover::new("ai-mode-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(self.mode_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.mode_open = *open;
                cx.notify();
            }))
            .trigger(
                Pill::new("ai-mode-pill")
                    .small(true)
                    .tooltip("Mode (⇧Tab)")
                    .child(icon(mode).size(px(12.)).text_color(tint))
                    .when(!self.narrow, |el| el.child(mode.label()))
                    .child(Icon::new(IconName::ChevronDown).size(px(10.)).text_color(theme.muted_foreground)),
            )
            .content(move |_, _, cx| {
                me.update(cx, |_, cx| {
                    let theme = cx.theme().clone();
                    ui::menu_surface(cx)
                        .id("ai-mode-body")
                        .test_support()
                        .w(px(280.))
                        .children(ChatMode::ALL.into_iter().map(|m| {
                            ui::menu_row(SharedString::from(format!("ai-mode-{}", m.label().to_lowercase())), m == mode, cx)
                                .items_start()
                                .py(px(8.))
                                .child(div().pt(px(2.)).child(icon(m).small().text_color(theme.muted_foreground)))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(px(2.))
                                        .child(div().font_medium().child(m.label()))
                                        .child(div().text_xs().whitespace_normal().text_color(theme.muted_foreground).child(m.description())),
                                )
                                .when(m == mode, |el| el.child(Icon::new(IconName::Check).small()))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_mode(m, cx)))
                                .test_support()
                        }))
                        .into_any_element()
                })
            })
            .into_any_element()
    }

    /// "Editing message · Esc to cancel" over the text while one is, with whether the files go
    /// back too (and which, on hover).
    fn edit_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.editing.as_ref()?;
        let theme = cx.theme().clone();
        let tooltip = |text: String| move |window: &mut Window, cx: &mut App| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx);
        let restore = match (e.restore, &e.files) {
            (Some(_), Files::Changes(changes)) if changes.iter().all(|f| !restore::acts(f)) => {
                ui::check_row("ai-edit-restore", "Restore files", false, true, cx).test_support().tooltip(tooltip("The files are as they were when this message was first sent.".into()))
            }
            (Some(on), files) => {
                let (label, tip) = match files {
                    Files::Changes(changes) => restore::summary(changes),
                    Files::Failed(why) => ("Restore files".into(), format!("Couldn't check the files: {why}")),
                    _ => ("Restore files".into(), "Checking which files changed…".into()),
                };
                ui::check_row("ai-edit-restore", label, on, false, cx).test_support().tooltip(tooltip(tip)).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(e) = this.editing.as_mut() {
                        e.restore = e.restore.map(|on| !on);
                    }
                    cx.notify();
                }))
            }
            (None, Files::Unavailable(why)) => ui::check_row("ai-edit-restore", "Restore files", false, true, cx).test_support().tooltip(tooltip(why.clone())),
            (None, _) => ui::check_row("ai-edit-restore", "Restore files", false, true, cx).test_support(),
        };
        Some(
            v_flex()
                .id("ai-editing")
                .test_support()
                .gap(px(4.))
                .text_size(px(11.5))
                .text_color(theme.muted_foreground)
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(Icon::new(crate::assets::Lucide::Pencil).size(px(11.)))
                        .child(div().flex_1().min_w_0().truncate().child("Editing message · Esc to cancel"))
                        .child(
                            div()
                                .id("ai-editing-cancel")
                                .test_support()
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx))),
                        ),
                )
                .child(div().pl(px(17.)).child(restore))
                .into_any_element(),
        )
    }
}

impl PickerHost for AiInput {
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

    /// Native views live in the harness only: nothing to hide here.
    fn overlay_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }
}

impl Attaching for AiInput {
    fn outbox(&mut self) -> &mut Outbox {
        &mut self.outbox
    }

    /// The tab a send held for images is for: its thread, or the new chat of this IDE folder (a
    /// send held across a folder change doesn't start a thread in the new one).
    fn target(&self, cx: &App) -> String {
        draft_key(self.workspace.read(cx))
    }

    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(FollowUp::Queue, window, cx);
    }
}

impl Render for AiInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("AiInput");
        let theme = cx.theme().clone();
        let narrow = self.workspace.read(cx).settings.ide.layout.ai_width < NARROW;
        if self.narrow != narrow {
            self.narrow = narrow;
            let placeholder = if narrow { PLACEHOLDER_NARROW } else { PLACEHOLDER };
            self.input.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let (thread, running, _) = self.state(cx);
        let empty = self.is_empty(cx);
        let context = thread.as_ref().and_then(|id| self.workspace.read(cx).live.get(id)).and_then(|l| l.context);
        let hh = self.workspace.read(cx).prefs_in(&Scope::Ide).hand_holding;
        let model_pill = pickers::model_pill(self, narrow, true, true, cx);
        let access_pill = pickers::access_pill(self, true, true, cx);
        let mode_pill = self.mode_pill(cx);
        let picker = pickers::picker(self, cx);
        let square = |id: &'static str| div().id(id).test_support().size(px(24.)).flex_none().rounded_full().flex().items_center().justify_center();
        let ember = palette::ember(cx);
        let send = if running {
            square("ai-stop")
                .cursor_pointer()
                .bg(theme.foreground.opacity(0.85))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Stop (⌘.)").build(window, cx))
                .child(div().size(px(8.)).rounded(px(1.5)).bg(theme.background))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = thread.clone() {
                        this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
                    }
                }))
                .into_any_element()
        } else {
            square("ai-send")
                .bg(if empty { theme.foreground.opacity(0.08) } else { ember })
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(Icon::new(IconName::ArrowUp).size(px(13.)).text_color(if empty { theme.muted_foreground } else { gpui_kit::white() }))
                .on_click(cx.listener(|this, _, window, cx| this.submit(FollowUp::Queue, window, cx)))
                .into_any_element()
        };
        let attachments_strip = (!self.outbox.paths.is_empty() || self.outbox.saving > 0).then(|| {
            let me = cx.entity().downgrade();
            let remove = move |path: &std::path::Path, _: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |this, cx| {
                    this.outbox.remove(path);
                    cx.notify();
                });
            };
            attachments::thumbnails(&self.outbox.paths, px(44.), self.outbox.saving > 0, remove, cx)
        });
        let glass = self.workspace.read(cx).glass().is_some();
        let border = if hh == trek_core::HandHolding::FullAccess { palette::amber(cx).opacity(0.35) } else { theme.foreground.opacity(0.12) };
        let card = v_flex()
            .id("ai-input")
            .test_support()
            .relative()
            .w_full()
            .p(px(8.))
            .gap(px(7.))
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .bg(if glass { theme.secondary.opacity(0.78) } else { theme.secondary })
            .children(self.edit_banner(cx))
            .child(self.chips_row(cx))
            .children(attachments_strip)
            .child(div().px(px(2.)).text_size(px(13.)).child(Textarea::new(&self.input).appearance(false).on_paste({
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
                    .w_full()
                    .min_w_0()
                    .gap(px(4.))
                    .items_center()
                    // The pills give way first: attach and send always keep their place.
                    .child(h_flex().flex_1().min_w_0().overflow_hidden().gap(px(4.)).items_center().child(mode_pill).child(model_pill).child(access_pill))
                    .when_some(context, |el, (used, window)| el.child(div().flex_none().child(pickers::context_ring(used, window, cx))))
                    .child(
                        div()
                            .id("ai-attach")
                            .test_support()
                            .size(px(24.))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.foreground.opacity(0.07)))
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Attach files").build(window, cx))
                            .child(Icon::new(crate::assets::Lucide::Paperclip).size(px(13.)).text_color(theme.muted_foreground))
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    )
                    .child(send),
            );
        let drop_tint = ember;
        div()
            .relative()
            .w_full()
            .px(px(10.))
            .pt(px(8.))
            .pb(px(10.))
            .key_context("AiInput")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| pickers::picker_key(this, ev, window, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.picker_action("up", window, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.picker_action("down", window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.picker_action("escape", window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.picker_action("tab", window, cx)))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| this.on_enter(action, window, cx)))
            // ⇧Tab cycles the mode.
            .capture_action(cx.listener(|this, _: &OutdentInline, _, cx| {
                if this.pickers.trigger.is_none() {
                    cx.stop_propagation();
                    let next = this.workspace.read(cx).chat_mode_in(&Scope::Ide).next();
                    this.set_mode(next, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::TogglePlan, _, cx| {
                let mode = if this.workspace.read(cx).chat_mode_in(&Scope::Ide) == ChatMode::Plan { ChatMode::Agent } else { ChatMode::Plan };
                this.set_mode(mode, cx);
            }))
            .on_action(cx.listener(|this, _: &UndoAllOrStop, _, cx| this.undo_all_or_stop(cx)))
            .child(card)
            .when_some(picker, |el, p| el.child(div().absolute().bottom_full().left(px(10.)).right(px(10.)).pb(px(2.)).child(p)))
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.opacity(0.85).border_color(drop_tint))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let (images, files) = attachments::split_files(paths.paths().iter().cloned());
                attachments::attach_files(this, images, window, cx);
                for path in files {
                    this.add_chip(ContextChip::File { path }, window, cx);
                }
            }))
    }
}

#[cfg(test)]
impl AiInput {
    pub(crate) fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub(crate) fn set_text_for_test(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
    }
}
