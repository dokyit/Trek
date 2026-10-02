//! The composer (MonoCode-style): a card with context on top, the prompt, then pills for
//! model (Fast · Effort › · Model ›) and access level, plus attach and send.
//! On a new thread it floats over the background hero with Capy-style context chips above it.

use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{PanelTool, Prefs, Route, Workspace, WorkspaceEvent};
use crate::TogglePlan;
use gpui_kit::component::input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use crate::mentions::{self, PickIcon, PickItem, PickKind, Trigger};
use std::path::PathBuf;
use std::sync::Arc;
use trek_core::catalog::ModelInfo;
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
    input: Entity<TextareaState>,
    model_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    model_open: bool,
    access_open: bool,
    sub: Option<Sub>,
    rail: Option<Rail>,
    /// Images going out with the next message.
    attachments: Vec<PathBuf>,
    /// The `/`, `@` or `$` token being completed, and the highlighted row.
    trigger: Option<Trigger>,
    picked: usize,
    /// Project files for `@`, indexed once per folder.
    file_index: Option<(PathBuf, Arc<Vec<String>>)>,
    indexing: Option<Task<()>>,
    snapshotting: bool,
    /// Set when Enter picked a row, so the same keypress doesn't also send.
    swallow_enter: Option<std::time::Instant>,
    picker_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// True when `id` is `candidate` or a dated snapshot of it (claude-haiku-4-5-20251001).
pub fn same_model(id: &str, candidate: &str) -> bool {
    id == candidate
        || id
            .strip_prefix(candidate)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|date| date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()))
}

/// Display name for a model id.
fn model_name(models: &[ModelInfo], id: &str) -> String {
    models.iter().find(|i| same_model(id, &i.id)).map(|i| i.name.clone()).unwrap_or_else(|| id.to_string())
}

/// The agent's default when the thread hasn't picked one: Opus 5.5 for Claude, the first live model otherwise.
fn default_model(models: &[ModelInfo]) -> Option<&ModelInfo> {
    models.iter().find(|m| m.id == "claude-opus-5-5").or_else(|| models.first())
}

fn hand_icon(level: HandHolding) -> Icon {
    match level {
        HandHolding::Supervised => Icon::new(crate::assets::Lucide::Lock),
        HandHolding::AutoAcceptEdits => Icon::new(crate::assets::Lucide::FilePen),
        HandHolding::Auto => Icon::new(crate::assets::Lucide::Sparkle),
        HandHolding::FullAccess => Icon::new(crate::assets::Lucide::ShieldCheck),
    }
}

impl Composer {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 12).submit_on_enter(true).placeholder("Ask, build, / for commands, @ for files")
        });
        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    let swallowed = this.swallow_enter.take().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250));
                    if this.trigger.is_none() && !swallowed {
                        this.submit(state.clone(), window, cx);
                    }
                } else if matches!(event, InputEvent::Change) {
                    this.update_trigger(cx);
                    cx.notify();
                }
            }),
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.observe(&model_search, |_, _, cx| cx.notify()),
        ];
        Self {
            workspace,
            input,
            model_search,
            clone_input,
            model_open: false,
            access_open: false,
            sub: None,
            rail: None,
            attachments: vec![],
            trigger: None,
            picked: 0,
            file_index: None,
            indexing: None,
            snapshotting: false,
            swallow_enter: None,
            picker_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn submit(&mut self, state: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let text = state.read(cx).value().to_string();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        self.trigger = None;
        let images = std::mem::take(&mut self.attachments);
        self.workspace.update(cx, |ws, cx| ws.send(text, images, cx));
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
        if !self.attachments.contains(&path) {
            self.attachments.push(path);
        }
        cx.notify();
    }

    fn update_prefs(&self, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
        self.workspace.update(cx, |ws, cx| {
            let mut p = ws.prefs();
            f(&mut p);
            ws.set_prefs(p, cx);
        });
    }

    /// Attach files by inserting @-references into the prompt.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Attach".into()) });
        let input = self.input.clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let (images, files): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| mentions::is_image(p));
                let _ = this.update(cx, |this, cx| {
                    this.attachments.extend(images);
                    cx.notify();
                });
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
    fn snapshot(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        if self.snapshotting {
            return;
        }
        self.snapshotting = true;
        let path = mentions::snapshot_path();
        let out = path.clone();
        cx.spawn(async move |this, cx| {
            let ok = cx
                .background_executor()
                .spawn(async move {
                    let mut cmd = std::process::Command::new("/usr/sbin/screencapture");
                    match mode {
                        "window" => cmd.args(["-i", "-W", "-o", "-x"]),
                        "area" => cmd.args(["-i", "-s", "-x"]),
                        _ => cmd.args(["-m", "-x"]),
                    };
                    cmd.arg(&out).status().map(|s| s.success()).unwrap_or(false) && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.snapshotting = false;
                if ok {
                    this.attachments.push(path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let (images, files): (Vec<PathBuf>, Vec<PathBuf>) = paths.iter().cloned().partition(|p| mentions::is_image(p));
        self.attachments.extend(images);
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
    }

    fn ensure_file_index(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.read(cx).current_cwd() else { return };
        if self.file_index.as_ref().is_some_and(|(r, _)| *r == root) || self.indexing.is_some() {
            return;
        }
        let r = root.clone();
        self.indexing = Some(cx.spawn(async move |this, cx| {
            let files = cx.background_executor().spawn(async move { mentions::index_files(&r) }).await;
            let _ = this.update(cx, |this, cx| {
                this.file_index = Some((root, Arc::new(files)));
                this.indexing = None;
                cx.notify();
            });
        }));
    }

    fn picker_items(&self, cx: &App) -> Vec<PickItem> {
        let Some(t) = &self.trigger else { return vec![] };
        let ws = self.workspace.read(cx);
        let agent = ws.prefs().agent;
        let q = t.query.to_lowercase();
        let commands = ws.slash_commands(&agent);
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
                if let Some((_, files)) = &self.file_index {
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
        let n = self.picker_items(cx).len();
        if self.trigger.is_none() || (n == 0 && key != "escape") {
            cx.propagate();
            return;
        }
        match key {
            "escape" => self.trigger = None,
            "up" => self.picked = (self.picked + n - 1) % n,
            "down" => self.picked = (self.picked + 1) % n,
            _ => self.accept_pick(self.picked.min(n - 1), window, cx),
        }
        self.picker_scroll.scroll_to_item(self.picked);
        cx.stop_propagation();
        cx.notify();
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
            "escape" => self.trigger = None,
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
                .child(v_flex().id("picker-list").max_h(px(280.)).overflow_y_scroll().track_scroll(&self.picker_scroll).children(items.into_iter().enumerate().map(|(i, item)| {
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
        if self.attachments.is_empty() && !self.snapshotting {
            return None;
        }
        let theme = cx.theme().clone();
        Some(
            h_flex()
                .px(px(14.))
                .pt(px(12.))
                .gap_2()
                .children(self.attachments.iter().enumerate().map(|(i, p)| {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    div()
                        .id(("attachment", i))
                        .group("att")
                        .relative()
                        .size(px(56.))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme.border)
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx))
                        .child(img(p.clone()).size_full().object_fit(ObjectFit::Cover))
                        .child(
                            div()
                                .id(("att-x", i))
                                .absolute()
                                .top(px(3.))
                                .right(px(3.))
                                .size(px(18.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(gpui_kit::black().opacity(0.65))
                                .invisible()
                                .group_hover("att", |s| s.visible())
                                .cursor_pointer()
                                .child(Icon::new(IconName::Close).xsmall().text_color(gpui_kit::white()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if i < this.attachments.len() {
                                        this.attachments.remove(i);
                                    }
                                    cx.notify();
                                })),
                        )
                }))
                .when(self.snapshotting, |el| {
                    el.child(
                        div()
                            .size(px(56.))
                            .rounded(px(10.))
                            .border_1()
                            .border_dashed()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Spinner::new().small().color(theme.muted_foreground)),
                    )
                })
                .into_any_element(),
        )
    }

    /// "+" menu: files and photos, snapshots, and the three pickers.
    fn plus_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let theme = cx.theme().clone();
        gpui_kit::component::button::Button::new("attach")
            .ghost()
            .with_size(px(30.))
            .rounded(px(8.))
            .bg(theme.foreground.opacity(0.065))
            .icon(Icon::new(IconName::Plus).text_color(theme.muted_foreground))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let (a, b, c, d, e, f, g) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
                menu.min_w(px(230.))
                    .item(PopupMenuItem::new("Add photos & files").icon(crate::assets::Lucide::Image).on_click(move |_, window, cx| a.update(cx, |c, cx| c.attach(window, cx))))
                    .separator()
                    .item(PopupMenuItem::new("Snapshot a window").icon(crate::assets::Lucide::Camera).on_click(move |_, _, cx| b.update(cx, |c, cx| c.snapshot("window", cx))))
                    .item(PopupMenuItem::new("Snapshot an area").icon(crate::assets::Lucide::Crosshair).on_click(move |_, _, cx| c.update(cx, |c, cx| c.snapshot("area", cx))))
                    .item(PopupMenuItem::new("Snapshot the screen").icon(crate::assets::Lucide::Monitor).on_click(move |_, _, cx| d.update(cx, |c, cx| c.snapshot("screen", cx))))
                    .separator()
                    .item(PopupMenuItem::new("Mention a file  @").icon(IconName::File).on_click(move |_, window, cx| e.update(cx, |c, cx| c.insert_trigger("@", window, cx))))
                    .item(PopupMenuItem::new("Use a skill  $").icon(crate::assets::Lucide::Sparkle).on_click(move |_, window, cx| f.update(cx, |c, cx| c.insert_trigger("$", window, cx))))
                    .item(PopupMenuItem::new("Run a command  /").icon(IconName::SquareTerminal).on_click(move |_, window, cx| g.update(cx, |c, cx| c.insert_trigger("/", window, cx))))
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
        let prefs = ws.prefs();
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
        h_flex().items_end().gap(px(6.)).child(main).children(sub).into_any_element()
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

    // ---------- access menu ----------

    fn access_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let current = ws.prefs().hand_holding;
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
                            this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(crate::workspace::SettingsPage::Permissions), cx));
                        } else {
                            this.update_prefs(cx, |p| p.hand_holding = level);
                        }
                        this.access_open = false;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    // ---------- project + clone ----------

    fn project_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let project = match &ws.route {
            Route::Draft { project } => project.clone(),
            _ => None,
        };
        let label = project.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Choose project".into());
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
                    .child(Icon::new(IconName::Folder).small())
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = menu.min_w(px(280.)).max_h(px(360.)).scrollable(true).label("Projects");
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

    /// Where the agent runs (this Mac) and on which branch, with a branch switcher.
    fn env_chips(&self, with_project: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let git = ws.current_git().cloned();
        let has_cwd = ws.current_cwd().is_some();
        let theme = cx.theme().clone();
        let chip = |id: &'static str| {
            h_flex().id(id).h(px(26.)).px(px(8.)).gap(px(6.)).rounded(px(7.)).text_sm().text_color(theme.foreground.opacity(0.82))
        };
        let local = chip("env-local")
            .child(Icon::new(crate::assets::Lucide::Laptop).small().text_color(theme.muted_foreground))
            .child("Local")
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Runs on this Mac, in the project folder").build(window, cx));
        let branch: Option<AnyElement> = match git {
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
                                let ws = ws_entity.clone();
                                let label = if Some(&b) == default.as_ref() { format!("{b}  (default)") } else { b.clone() };
                                menu = menu.item(PopupMenuItem::new(label).checked(Some(&b) == current.as_ref()).on_click(move |_, _, cx| {
                                    let b = b.clone();
                                    ws.update(cx, |ws, cx| ws.switch_branch(b, cx))
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
            .child(local)
            .children(branch)
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

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs();
        let thread = ws.current_thread().cloned();
        let is_draft = matches!(ws.route, Route::Draft { .. });
        let running = thread.as_ref().is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou));
        let cost = thread.as_ref().and_then(|t| ws.live.get(&t.id)).map(|l| l.cost_usd).unwrap_or(0.0);
        let models = ws.models_for(&prefs.agent);
        let model_label = prefs
            .model
            .as_deref()
            .map(|m| model_name(&models, m))
            .or_else(|| default_model(&models).map(|m| m.name.clone()))
            .unwrap_or_else(|| prefs.agent.display_name());
        let theme = cx.theme().clone();
        let empty = self.input.read(cx).value().trim().is_empty();
        let thread_id = thread.as_ref().map(|t| t.id.clone());
        let context = thread.as_ref().and_then(|t| ws.live.get(&t.id)).and_then(|l| l.context);
        let plan = prefs.plan;

        // Model pill + menu.
        let model_open = self.model_open;
        let model_pill = Popover::new("model-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(model_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.model_open = *open;
                if !*open {
                    this.sub = None;
                    this.rail = None;
                }
                cx.notify();
            }))
            .trigger(
                Pill::new("model-pill")
                    .child(ui::agent_glyph(&prefs.agent, cx))
                    .child(div().child(model_label))
                    .child(div().text_color(theme.muted_foreground).child(prefs.effort.label()))
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
                cx.notify();
            }))
            .trigger(
                Pill::new("access-pill")
                    .child(hand_icon(hh).small().text_color(access_tint))
                    .child(hh.label())
                    .child(Icon::new(if access_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
            )
            .content(move |_, _, cx| access_entity.update(cx, |c, cx| c.access_menu(cx)));

        let square = |id: &'static str| {
            div().id(id).size(px(30.)).flex_none().rounded(px(8.)).flex().items_center().justify_center()
        };
        let send = if running {
            square("stop")
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
                .bg(if empty { theme.foreground.opacity(0.08) } else { theme.foreground })
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(Icon::new(IconName::ArrowUp).small().text_color(if empty { theme.muted_foreground } else { theme.background }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let input = this.input.clone();
                    this.submit(input, window, cx);
                }))
                .into_any_element()
        };

        let card = v_flex()
            .w_full()
            .rounded(px(16.))
            .bg(theme.secondary)
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
                        .child(div().flex_1())
                        .when(running, |el| el.child(div().pr(px(6.)).child(Spinner::new().small().color(theme.muted_foreground)))),
                )
            })
            .children(self.attachment_strip(cx))
            .child(div().px(px(14.)).pt(px(if is_draft || !self.attachments.is_empty() { 12. } else { 2. })).child(Textarea::new(&self.input).appearance(false)))
            .child(
                h_flex()
                    .p(px(8.))
                    .gap(px(6.))
                    .child(self.plus_button(cx))
                    .child(model_pill)
                    .child(access_pill)
                    .when(is_draft, |el| {
                        el.child(
                            Pill::new("plan-pill")
                                .selected(plan)
                                .child(Icon::new(crate::assets::Lucide::ListChecks).small().text_color(if plan { palette::indigo(cx) } else { theme.muted_foreground }))
                                .child("Plan")
                                .on_click(cx.listener(|this, _, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan))),
                        )
                    })
                    .child(div().flex_1())
                    .when_some(context, |el, (used, window)| el.child(self.context_ring(used, window, cx)))
                    .child(send),
            );

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
                .child(h_flex().gap(px(6.)).child(ui::agent_glyph(&prefs.agent, cx)).child(prefs.agent.display_name()))
                .when(cost >= 0.005, |el| el.child(format!("${cost:.2} this thread")))
                .child(div().flex_1())
                .child(
                    h_flex()
                        .id("open-terminal")
                        .gap(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(Icon::new(IconName::SquareTerminal).xsmall())
                        .child("Terminal")
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(PanelTool::Terminal))))),
                )
        });

        h_flex()
            .w_full()
            .justify_center()
            .px_6()
            .pb(px(if is_draft { 0. } else { 12. }))
            .key_context("Composer")
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            // The textarea binds these keys to actions, which never reach key listeners.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.picker_action("up", window, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.picker_action("down", window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.picker_action("escape", window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.picker_action("tab", window, cx)))
            .on_action(cx.listener(|this, _: &TogglePlan, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan)))
            .child(v_flex().w_full().max_w(px(760.)).children(chips).child(card).children(status))
    }
}
