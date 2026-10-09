//! `AiPane`: the AI side bar. Its header is a strip of chat tabs (each a thread, or a new chat
//! in the IDE folder) with a run-state mark and ×, which closes the tab only; then + for a new
//! chat, the folder's history, and ⋯. Under it the active tab: its compact transcript
//! (`transcript`), the bar of changes pending review (`review`), and the input (`input`), all on
//! `Scope::Ide`. A new chat's first message starts an ordinary thread, in the inbox like any
//! other, without moving the harness.

use super::context::ContextChip;
use super::input::AiInput;
use super::transcript::AiTranscript;
use crate::workspace::{IdeTab, Mode, Route, Scope, SettingsPage, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const HEADER_HEIGHT: f32 = 34.;
/// Chat tabs are cut to this width; the title shows in full on hover.
const TAB_MAX: f32 = 168.;
/// Nor do they get narrower than this: past it the strip scrolls, and ⌄ lists them all.
const TAB_MIN: f32 = 120.;

pub struct AiPane {
    workspace: Entity<Workspace>,
    pub(crate) transcript: Entity<AiTranscript>,
    pub(crate) input: Entity<AiInput>,
    /// A chat tab being renamed: its thread and the title being typed.
    renaming: Option<(String, Entity<InputState>, Subscription)>,
    /// The strip of tabs, scrolled to keep the active one in view.
    strip: ScrollHandle,
    /// The tab the strip last scrolled to.
    shown_tab: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl AiPane {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let transcript = cx.new(|cx| AiTranscript::new(workspace.clone(), window, cx));
        let input = cx.new(|cx| AiInput::new(workspace.clone(), window, cx));
        let subscriptions = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.subscribe_in(&workspace, window, |this, ws, event: &WorkspaceEvent, window, cx| {
                let ide = ws.read(cx).ide();
                match event {
                    WorkspaceEvent::FocusAiInput if ide => this.focus(window, cx),
                    WorkspaceEvent::ComposeIn { scope: Scope::Ide, thread, text, images, edit } => {
                        if ws.read(cx).thread_id_in(&Scope::Ide) == Some(thread.as_str()) {
                            this.input.update(cx, |c, cx| c.compose(thread, text, images, edit.clone(), window, cx));
                        }
                    }
                    WorkspaceEvent::CorrectRestatement { scope: Scope::Ide, thread } => {
                        if ws.read(cx).thread_id_in(&Scope::Ide) == Some(thread.as_str()) {
                            this.input.update(cx, |c, cx| c.correct(window, cx));
                        }
                    }
                    WorkspaceEvent::RestoreQueued { thread, text, images } if ws.read(cx).shown_in(thread) == Some(Scope::Ide) => {
                        this.input.update(cx, |c, cx| c.restore(text, images, window, cx));
                    }
                    // In the editor, what's meant for "the composer" (a browser pick, a message
                    // a worktree couldn't take) comes here.
                    WorkspaceEvent::InsertIntoComposer(text) if ide => this.input.update(cx, |c, cx| c.insert_text(text, window, cx)),
                    WorkspaceEvent::AttachImage(path) if ide => {
                        let path = path.clone();
                        this.input.update(cx, |c, cx| c.attach_image(path, cx));
                    }
                    _ => {}
                }
            }),
        ];
        Self { workspace, transcript, input, renaming: None, strip: ScrollHandle::new(), shown_tab: None, _subscriptions: subscriptions }
    }

    /// The input takes the keys.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |c, cx| c.focus(window, cx));
    }

    /// The file in front in the editor (`None`: none, or a Review tab): it goes with messages.
    pub fn set_current_file(&mut self, path: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.input.update(cx, |c, cx| c.set_current_file(path, cx));
    }

    /// Lines picked in the editor (⌘L, ⌘⇧L) go to the input as a chip.
    pub fn add_chip(&mut self, chip: ContextChip, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |c, cx| c.add_chip(chip, window, cx));
    }

    /// Rename chat tab `ix`'s thread: its title becomes an input in the tab.
    pub fn begin_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let Some(IdeTab::Thread(id)) = ws.ide_chat.tabs.get(ix).cloned() else { return };
        let title = ws.thread(&id).map(|t| t.title.clone()).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Chat name"));
        input.update(cx, |s, cx| {
            s.set_value(title.clone(), window, cx);
            s.select_all(window, cx);
            s.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } => this.finish_rename(true, cx),
            InputEvent::Blur => this.finish_rename(true, cx),
            _ => {}
        });
        self.renaming = Some((id, input, sub));
        cx.notify();
    }

    /// The rename ends: the title typed is kept (`keep`, and not empty), or dropped.
    fn finish_rename(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some((id, input, _)) = self.renaming.take() else { return };
        let title = input.read(cx).value().trim().to_string();
        if keep && !title.is_empty() {
            self.workspace.update(cx, |ws, cx| ws.rename(&id, title, cx));
        }
        cx.notify();
    }

    /// The strip of chat tabs, then +, history and ⋯.
    fn header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let glass = ws.glass();
        let line = if glass.is_some() { crate::ui::panel_border(glass, cx) } else { theme.border };
        let editor_bg = crate::ui::panel_bg(glass, cx);
        let active = ws.ide_chat.active;
        let tabs: Vec<(usize, IdeTab, String, Option<AnyElement>)> = ws
            .ide_chat
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| match tab {
                IdeTab::Draft => (ix, tab.clone(), "New chat".to_string(), None),
                IdeTab::Thread(id) => match ws.thread(id) {
                    Some(t) => (ix, tab.clone(), t.title.clone(), Some(crate::ide::agents_view::state_mark(t, cx))),
                    None => (ix, tab.clone(), "Thread".to_string(), None),
                },
            })
            .collect();
        let thread = ws.ide_chat.active_thread().map(str::to_string);
        let count = tabs.len();
        let listed: Vec<(usize, String)> = tabs.iter().map(|(ix, _, title, _)| (*ix, title.clone())).collect();
        // The active tab stays in view as tabs come and go.
        if self.shown_tab != Some(active) {
            self.shown_tab = Some(active);
            self.strip.scroll_to_item(active);
        }
        let renaming = self.renaming.as_ref().map(|(id, input, _)| (id.clone(), input.clone()));
        h_flex()
            .id("ai-header")
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .w_full()
            .border_b_1()
            .border_color(line)
            .child(
                h_flex().id("ai-tabs").flex_1().min_w_0().h_full().overflow_x_scroll().track_scroll(&self.strip).children(tabs.into_iter().map(|(ix, tab, title, mark)| {
                    let on = ix == active;
                    let group = SharedString::from(format!("ai-tab-{ix}"));
                    let tip = title.clone();
                    let thread_id = match &tab {
                        IdeTab::Thread(id) => Some(id.clone()),
                        IdeTab::Draft => None,
                    };
                    let editing = renaming.as_ref().filter(|(id, _)| Some(id) == thread_id.as_ref()).map(|(_, input)| input.clone());
                    let me = cx.weak_entity();
                    let ws = self.workspace.clone();
                    h_flex()
                        .id(("ai-tab", ix))
                        .test_support()
                        .group(group.clone())
                        .h_full()
                        // Never squeezed to a few letters: past what fits, the strip scrolls and
                        // ⌄ lists every chat.
                        .flex_none()
                        .min_w(px(TAB_MIN))
                        .max_w(px(TAB_MAX))
                        .pl(px(10.))
                        .pr(px(4.))
                        .gap(px(6.))
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .border_r_1()
                        .border_color(line)
                        .when(on, |el| el.bg(editor_bg).text_color(theme.foreground))
                        .when(!on, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.list_hover)))
                        .children(mark.map(|m| div().flex_none().child(m)))
                        .child(match editing {
                            Some(input) => div()
                                .id(("ai-tab-rename", ix))
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                                    cx.stop_propagation();
                                    this.finish_rename(false, cx);
                                }))
                                .child(Input::new(&input).xsmall())
                                .into_any_element(),
                            None => div().flex_1().min_w_0().truncate().child(title).into_any_element(),
                        })
                        .child(
                            div()
                                .id(("ai-tab-close", ix))
                                .test_support()
                                .size(px(18.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                                // Behind tabs show × on hover only.
                                .child(
                                    div()
                                        .when(!on, |el| el.invisible().group_hover(group.clone(), |s| s.visible()))
                                        .child(Icon::new(IconName::Close).size(px(12.)).text_color(theme.muted_foreground)),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                // Closes the tab only: the thread stays, in the inbox and history.
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.workspace.update(cx, |ws, cx| ws.ide_close_chat(ix, cx));
                                })),
                        )
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        // A double click renames the chat.
                        .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                            this.workspace.update(cx, |ws, cx| ws.ide_select_chat(ix, cx));
                            if e.click_count() >= 2 {
                                this.begin_rename(ix, window, cx);
                            }
                        }))
                        .context_menu(move |menu, _, _| {
                            let (me1, me2, ws1, ws2, ws3) = (me.clone(), me.clone(), ws.clone(), ws.clone(), ws.clone());
                            let id = thread_id.clone();
                            menu.min_w(px(200.))
                                .item(PopupMenuItem::new("Rename…").disabled(id.is_none()).on_click(move |_, window, cx| {
                                    let _ = me1.update(cx, |this, cx| this.begin_rename(ix, window, cx));
                                }))
                                .item(PopupMenuItem::new("Open in Agents").disabled(id.is_none()).on_click(move |_, _, cx| {
                                    if let Some(id) = id.clone() {
                                        ws1.update(cx, |ws, cx| {
                                            ws.set_mode(Mode::Agents, cx);
                                            ws.navigate(Route::Thread(id), cx);
                                        });
                                    }
                                }))
                                .separator()
                                .item(PopupMenuItem::new("Close").on_click(move |_, _, cx| ws2.update(cx, |ws, cx| ws.ide_close_chat(ix, cx))))
                                .item(PopupMenuItem::new("Close Others").on_click(move |_, _, cx| ws3.update(cx, |ws, cx| ws.ide_close_other_chats(ix, cx))))
                                .item(PopupMenuItem::new("New Chat").on_click(move |_, _, cx| {
                                    let _ = me2.update(cx, |this, cx| this.workspace.update(cx, |ws, cx| ws.ide_new_chat(cx)));
                                }))
                        })
                })),
            )
            .when(count > 1, |el| el.child(self.tabs_menu(listed, active, cx)))
            .child(
                h_flex()
                    .flex_none()
                    .px(px(4.))
                    .gap(px(1.))
                    .child(crate::ui::icon_button("ai-new-chat", IconName::Plus, "New chat (⌘N)").on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.ide_new_chat(cx)))))
                    .child(self.history_button(cx))
                    .child(self.more_button(thread, cx)),
            )
    }

    /// ⌄: every open chat by name (the strip may not show them all); one comes to the front.
    fn tabs_menu(&self, listed: Vec<(usize, String)>, active: usize, _: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.clone();
        Button::new("ai-tabs-menu").ghost().small().icon(IconName::ChevronDown).tooltip("All chats").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.min_w(px(240.)).max_h(px(420.)).scrollable(true).label("Open chats");
            for (ix, title) in &listed {
                let (ws, ix) = (ws.clone(), *ix);
                let title = if title.chars().count() > 48 { format!("{}…", title.chars().take(47).collect::<String>()) } else { title.clone() };
                menu = menu.item(PopupMenuItem::new(title).checked(ix == active).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.ide_select_chat(ix, cx))));
            }
            menu
        })
    }

    /// 🕘: this folder's threads, newest first, by day; one opens as a chat tab.
    fn history_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = self.workspace.clone();
        let _ = cx;
        let today = chrono::Local::now().date_naive().and_hms_opt(0, 0, 0).and_then(|d| d.and_local_timezone(chrono::Local).single()).map_or(0, |d| d.timestamp_millis());
        let week = today - 6 * 86_400_000;
        Button::new("ai-history")
            .ghost()
            .small()
            .icon(crate::assets::Lucide::RotateCcwClock)
            .tooltip("Chats in this folder")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, cx| {
                // Read as it opens, not on every frame of the side bar.
                let threads: Vec<(String, String, i64)> = entity.read(cx).ide_threads().into_iter().map(|t| (t.id.clone(), t.title.clone(), t.updated_at)).collect();
                menu = menu.min_w(px(280.)).max_h(px(420.)).scrollable(true);
                if threads.is_empty() {
                    return menu.label("No chats in this folder yet");
                }
                let mut last: Option<&'static str> = None;
                for (id, title, at) in threads.iter().take(60) {
                    let group = if *at >= today { "Today" } else if *at >= week { "This week" } else { "Older" };
                    if last != Some(group) {
                        if last.is_some() {
                            menu = menu.separator();
                        }
                        menu = menu.label(group);
                        last = Some(group);
                    }
                    let (ws, id) = (entity.clone(), id.clone());
                    let title = if title.chars().count() > 48 { format!("{}…", title.chars().take(47).collect::<String>()) } else { title.clone() };
                    menu = menu.item(PopupMenuItem::new(title).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.ide_open_thread(&id, cx))));
                }
                menu
            })
    }

    /// ⋯: the active chat's actions.
    fn more_button(&self, thread: Option<String>, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.clone();
        let me = cx.weak_entity();
        let active_ix = ws.read(cx).ide_chat.active;
        Button::new("ai-more").ghost().small().icon(IconName::Ellipsis).tooltip("More").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.min_w(px(220.));
            let on_thread = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                let (ws, id) = (ws.clone(), thread.clone());
                PopupMenuItem::new(label).disabled(id.is_none()).on_click(move |_, _, cx| {
                    if let Some(id) = &id {
                        ws.update(cx, |ws, cx| f(ws, id, cx));
                    }
                })
            };
            let me = me.clone();
            let active = active_ix;
            menu = menu
                .item(PopupMenuItem::new("Rename Chat…").disabled(thread.is_none()).on_click(move |_, window, cx| {
                    let _ = me.update(cx, |this, cx| this.begin_rename(active, window, cx));
                }))
                .item(on_thread("Open in Agents", |ws, id, cx| {
                    ws.set_mode(Mode::Agents, cx);
                    ws.navigate(Route::Thread(id.to_string()), cx);
                }))
                .item({
                    let (ws, id) = (ws.clone(), thread.clone());
                    PopupMenuItem::new("Open in New Window").disabled(id.is_none()).on_click(move |_, _, cx| {
                        if let Some(id) = &id {
                            crate::thread_window::open(ws.clone(), id, cx);
                        }
                    })
                })
                .item(on_thread("Fork Chat", |ws, id, cx| _ = ws.fork_thread(id, crate::workspace::ForkAt::End, &Scope::Ide, cx)))
                .item({
                    let (ws, id) = (ws.clone(), thread.clone());
                    PopupMenuItem::new("Copy Transcript").disabled(id.is_none()).on_click(move |_, _, cx| {
                        if let Some(id) = &id {
                            let text = ws.read(cx).transcript_markdown(id);
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    })
                })
                .separator()
                .item(on_thread("Archive", |ws, id, cx| ws.archive(id, cx)))
                .separator();
            let ws = ws.clone();
            menu.item(PopupMenuItem::new("Agent Settings…").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Agents), cx))))
        })
    }

    /// A new chat, before its first message: where it will work, and what it becomes.
    fn draft_note(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let folder = self.workspace.read(cx).ide_root.as_ref().map(|r| r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.display().to_string()));
        v_flex()
            .id("ai-draft")
            .test_support()
            .flex_1()
            .min_h_0()
            .px(px(28.))
            .items_center()
            .justify_center()
            .gap(px(8.))
            .text_center()
            .child(Icon::new(crate::assets::Lucide::Sparkles).size(px(22.)).text_color(crate::palette::ember(cx)))
            .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(match &folder {
                Some(f) => format!("New chat in {f}"),
                None => "New chat".into(),
            }))
            .child(div().text_size(px(12.5)).text_color(theme.muted_foreground).child("Ask about this code, or have an agent change it. It's a thread like any other: it shows in Agents too."))
    }
}

impl Render for AiPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("AiPane");
        let ws = self.workspace.read(cx);
        let draft = ws.ide_chat.is_draft();
        let thread = ws.ide_chat.active_thread().map(str::to_string);
        let glass = ws.glass();
        let line = if glass.is_some() { crate::ui::panel_border(glass, cx) } else { cx.theme().border };
        let pending = thread.and_then(|id| super::review::pending_bar(&self.workspace, &id, line, cx));
        v_flex()
            .id("ai-pane")
            .key_context("AiPane")
            .size_full()
            .child(self.header(cx))
            .when(draft, |el| el.child(self.draft_note(cx)))
            // Cached: the transcript redraws when its own chat moves, not with the header.
            .when(!draft, |el| el.child(div().flex_1().min_h_0().child(self.transcript.clone().cached(StyleRefinement::default().size_full()))))
            .children(pending)
            .child(self.input.clone())
    }
}
