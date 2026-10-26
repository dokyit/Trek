//! The Share sheet (the thread's "⋯" › Share…, ⌘K's "Share thread…", Thread › Share… on a Mac): a
//! read-only copy of the thread, three ways. **Link** makes a secret GitHub gist with the GitHub
//! CLI and remembers it, so it can be copied, opened, updated or deleted later; **Export** saves the
//! thread as Markdown, a self-contained HTML page or its items as JSON; **Send to** hands it to the
//! paired iPhone (a notification that opens it there), continues it on a paired machine, or, on a
//! Mac, gives it to the system's share sheet. What a copy carries (tool calls, thinking, paths and
//! secrets) is chosen once and goes for each (`render`).

pub mod gist;
pub mod render;
#[cfg(target_os = "macos")]
mod system;

use crate::composer::look;
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const WIDTH: f32 = 430.;
const PAD: f32 = 20.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Link,
    Export,
    SendTo,
}

/// What Export saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Json,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Json => "json",
        }
    }
}

/// Where Send to sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Phone,
    Machine(String),
    /// The system's share sheet (macOS).
    System,
}

/// Open the Share sheet for thread `id`.
pub fn open(workspace: Entity<Workspace>, id: &str, window: &mut Window, cx: &mut App) {
    open_on(workspace, id, Tab::Link, window, cx)
}

/// Open the Share sheet for thread `id` on `tab`.
pub fn open_on(
    workspace: Entity<Workspace>,
    id: &str,
    tab: Tab,
    window: &mut Window,
    cx: &mut App,
) {
    if workspace.read(cx).thread(id).is_none() {
        return;
    }
    let sheet = cx.new(|cx| ShareSheet::new(workspace, id.to_string(), tab, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(WIDTH)).p(px(0.)).child(sheet.clone())
    });
}

pub struct ShareSheet {
    workspace: Entity<Workspace>,
    id: String,
    pub tab: Tab,
    /// "Anyone with the link can view": off, no link is made.
    pub public: bool,
    pub tools: bool,
    pub thinking: bool,
    pub redact: bool,
    /// The thread's gist, made now or before.
    pub gist: Option<gist::Gist>,
    /// What's under way (gh is running).
    busy: Option<&'static str>,
    pub error: Option<String>,
    pub format: Format,
    pub target: Option<Target>,
    _subscriptions: Vec<Subscription>,
}

impl ShareSheet {
    fn new(
        workspace: Entity<Workspace>,
        id: String,
        tab: Tab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = Some(Target::Phone);
        let subs = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        Self {
            gist: gist::remembered(&id),
            workspace,
            id,
            tab,
            public: true,
            tools: true,
            thinking: false,
            redact: true,
            busy: None,
            error: None,
            format: Format::Markdown,
            target,
            _subscriptions: subs,
        }
    }

    fn options(&self) -> render::Options {
        render::Options {
            tools: self.tools,
            thinking: self.thinking,
        }
    }

    /// The thread's title and items as the options have them.
    fn prepared(
        &self,
        options: render::Options,
        cx: &App,
    ) -> (String, Vec<trek_core::store::Item>) {
        let ws = self.workspace.read(cx);
        let thread = ws.thread(&self.id);
        let title = thread.map(|t| t.title.clone()).unwrap_or_default();
        let project = thread.and_then(|t| ws.project_dir(t));
        let redact = self
            .redact
            .then(|| render::Redact::here(project.as_deref()));
        let items = render::prepare(ws.transcript_items(&self.id), options, redact.as_ref());
        (title, items)
    }

    /// The thread as Markdown, as the options have it (what a link and More… carry).
    pub fn markdown(&self, cx: &App) -> String {
        let (title, items) = self.prepared(self.options(), cx);
        render::markdown(&title, &items)
    }

    /// The file Export saves: its name and what's in it.
    pub fn export(&self, format: Format, cx: &App) -> (String, String) {
        let ws = self.workspace.read(cx);
        let thread = ws.thread(&self.id).cloned();
        let title = thread.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let agent = thread
            .as_ref()
            .map(|t| t.agent.display_name())
            .unwrap_or_default();
        let name = format!("{}.{}", render::slug(&title), format.extension());
        let contents = match format {
            Format::Markdown => self.markdown(cx),
            Format::Html => {
                let (title, items) = self.prepared(self.options(), cx);
                let subtitle = format!(
                    "{} · shared from Trek on {}",
                    agent,
                    chrono::Local::now().format("%-d %B %Y")
                );
                render::html(&title, &agent, &subtitle, &items)
            }
            // Every item, as stored; with paths and secrets hidden if that's on.
            Format::Json => {
                let all = render::Options {
                    tools: true,
                    thinking: true,
                };
                let (_, items) = self.prepared(all, cx);
                match thread {
                    Some(t) => render::json(&t, &items),
                    None => String::new(),
                }
            }
        };
        (name, contents)
    }

    fn title(&self, cx: &App) -> String {
        self.workspace
            .read(cx)
            .thread(&self.id)
            .map(|t| t.title.clone())
            .unwrap_or_default()
    }

    // ── Link ──

    fn create_link(&mut self, cx: &mut Context<Self>) {
        if !self.public || self.busy.is_some() {
            return;
        }
        let markdown = self.markdown(cx);
        let title = self.title(cx);
        let filename = format!("{}.md", render::slug(&title));
        let id = self.id.clone();
        self.run_gh("Creating link…", cx, move || {
            let g = gist::create(gist::gh()?, &title, &filename, &markdown)?;
            gist::remember(&id, Some(g.clone()));
            Ok(Some(g))
        });
    }

    fn update_link(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.gist.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        let markdown = self.markdown(cx);
        let id = self.id.clone();
        self.run_gh("Updating link…", cx, move || {
            let g = gist::update(gist::gh()?, &g, &markdown)?;
            gist::remember(&id, Some(g.clone()));
            Ok(Some(g))
        });
    }

    fn delete_link(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.gist.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        let id = self.id.clone();
        self.run_gh("Deleting link…", cx, move || {
            match gist::delete(gist::gh()?, &g) {
                // Gone already (deleted on GitHub): forgotten here too.
                Ok(()) => {}
                Err(e) if e.contains("doesn't have that gist") => {}
                Err(e) => return Err(e),
            }
            gist::remember(&id, None);
            Ok(None)
        });
    }

    /// Run `job` (gh) off the UI thread with `doing` shown; the gist it leaves is the thread's.
    fn run_gh(
        &mut self,
        doing: &'static str,
        cx: &mut Context<Self>,
        job: impl FnOnce() -> Result<Option<gist::Gist>, String> + Send + 'static,
    ) {
        self.busy = Some(doing);
        self.error = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move { job() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(g) => this.gist = g,
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Export ──

    fn save(&mut self, cx: &mut Context<Self>) {
        let (name, contents) = self.export(self.format, cx);
        let home = trek_core::paths::home();
        let downloads = home.join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { home };
        let asked = cx.prompt_for_new_path(&dir, Some(&name));
        let ws = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(path))) = asked.await else {
                return;
            };
            let message = match std::fs::write(&path, contents) {
                Ok(()) => format!(
                    "Saved {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                Err(e) => format!("Couldn't save it: {e}"),
            };
            ws.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message,
                    undo: None,
                })
            });
        })
        .detach();
    }

    // ── Send to ──

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let id = self.id.clone();
        match target {
            Target::Phone => {
                self.workspace
                    .update(cx, |ws, cx| ws.send_thread_to_phone(&id, cx));
                window.close_dialog(cx);
            }
            Target::Machine(host) => {
                // The conversation as Copy as Markdown has it: the machine is the user's own.
                let markdown = self.workspace.read(cx).transcript_markdown(&id);
                self.workspace.update(cx, |ws, cx| {
                    ws.continue_on_machine(&host, &id, &markdown, cx)
                });
                window.close_dialog(cx);
            }
            Target::System => {
                #[cfg(target_os = "macos")]
                system::share_text(window, &self.markdown(cx));
            }
        }
    }

    fn primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Link if self.gist.is_some() => self.update_link(cx),
            Tab::Link => self.create_link(cx),
            Tab::Export => self.save(cx),
            Tab::SendTo => self.send(window, cx),
        }
    }
}

// ───────────────────────────── drawing ─────────────────────────────

fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .line_height(relative(1.45))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A switch with its label (and what it means, under it).
fn switch_row(
    id: &'static str,
    label: &'static str,
    detail: Option<&'static str>,
    on: bool,
    disabled: bool,
    set: impl Fn(&mut ShareSheet, bool) + 'static,
    cx: &mut Context<ShareSheet>,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap(px(12.))
        .py(px(7.))
        .items_start()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().text_size(px(13.)).child(label))
                .when_some(detail, |el, d| el.child(note(d, cx))),
        )
        .child(
            Switch::new(id)
                .checked(on)
                .small()
                .disabled(disabled)
                .on_click(cx.listener(move |this, v: &bool, _, cx| {
                    set(this, *v);
                    this.error = None;
                    cx.notify();
                })),
        )
        .into_any_element()
}

/// A choice in a list (Export's formats, Send to's places): icon, label, what it does; the
/// chosen one on the quiet fill with a check.
fn choice(
    id: impl Into<ElementId>,
    glyph: AnyElement,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    chosen: bool,
    on_pick: impl Fn(&mut ShareSheet, &mut Window, &mut Context<ShareSheet>) + 'static,
    cx: &mut Context<ShareSheet>,
) -> AnyElement {
    let fill = look::chip_fill(cx);
    let hover = look::chip_hover(cx);
    let theme = cx.theme().clone();
    h_flex()
        .id(id)
        .test_support()
        .w_full()
        .min_h(px(44.))
        .px(px(10.))
        .py(px(7.))
        .gap(px(10.))
        .rounded(px(10.))
        .cursor_pointer()
        .when(chosen, |el| el.bg(fill))
        .hover(move |s| s.bg(if chosen { fill } else { hover.opacity(0.6) }))
        .child(
            div()
                .flex_none()
                .size(px(28.))
                .rounded(px(8.))
                .border_1()
                .border_color(look::hairline(cx))
                .bg(look::card_fill(cx))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.into()),
                )
                .child(note(detail, cx)),
        )
        .when(chosen, |el| {
            el.child(
                Icon::new(IconName::Check)
                    .size(px(14.))
                    .text_color(theme.foreground),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            on_pick(this, window, cx);
            cx.notify();
        }))
        .into_any_element()
}

/// When a link was made, in a sentence: "just now", "4m ago", "on Sep 12".
fn shared_when(at: i64) -> String {
    match crate::time::relative(at) {
        now if now == "now" => "just now".into(),
        ago if ago.ends_with(['m', 'h', 'd']) && ago.starts_with(|c: char| c.is_ascii_digit()) => {
            format!("{ago} ago")
        }
        date => format!("on {date}"),
    }
}

fn icon(name: impl Into<Icon>, cx: &App) -> AnyElement {
    let icon: Icon = name.into();
    icon.size(px(15.))
        .text_color(cx.theme().muted_foreground)
        .into_any_element()
}

impl ShareSheet {
    fn options_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            switch_row(
                "share-tools",
                "Include tool calls and diffs",
                None,
                self.tools,
                false,
                |this, v| this.tools = v,
                cx,
            ),
            switch_row(
                "share-thinking",
                "Include thinking",
                None,
                self.thinking,
                false,
                |this, v| this.thinking = v,
                cx,
            ),
            switch_row(
                "share-redact",
                "Hide file paths and secrets",
                Some(
                    "Your home folder shows as ~, files outside the project by name only, and keys and tokens are taken out.",
                ),
                self.redact,
                false,
                |this, v| this.redact = v,
                cx,
            ),
        ]
    }

    fn link_tab(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some();
        let mut rows = vec![switch_row(
            "share-public",
            "Anyone with the link can view",
            Some(if self.public {
                "A secret GitHub gist: not listed anywhere, but anyone who has the link can open it."
            } else {
                "Off: no link is made."
            }),
            self.public,
            busy,
            |this, v| this.public = v,
            cx,
        )];
        rows.extend(self.options_rows(cx));
        v_flex()
            .gap(px(2.))
            .children(rows)
            .when_some(self.gist.clone(), |el, g| {
                let (copy_url, open_url) = (g.url.clone(), g.url.clone());
                el.child(
                    v_flex()
                        .pt(px(10.))
                        .gap(px(8.))
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .id("share-url")
                                        .test_support()
                                        .flex_1()
                                        .min_w_0()
                                        .h(px(30.))
                                        .px(px(10.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(look::hairline(cx))
                                        .bg(look::chip_fill(cx))
                                        .text_size(px(12.5))
                                        .child(div().truncate().child(g.url.clone())),
                                )
                                .child(
                                    Button::new("share-url-copy")
                                        .outline()
                                        .small()
                                        .icon(IconName::Copy)
                                        .label("Copy")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy_url.clone(),
                                            ));
                                            this.workspace.update(cx, |_, cx| {
                                                cx.emit(WorkspaceEvent::Toast {
                                                    message: "Copied the link".into(),
                                                    undo: None,
                                                })
                                            });
                                        })),
                                )
                                .child(
                                    Button::new("share-url-open")
                                        .outline()
                                        .small()
                                        .icon(IconName::ExternalLink)
                                        .label("Open")
                                        .on_click(move |_, _, cx| cx.open_url(&open_url)),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(note(
                                    format!(
                                        "Shared {}. Update replaces it with the thread as it is now.",
                                        shared_when(g.at)
                                    ),
                                    cx,
                                )
                                .flex_1())
                                .child(
                                    Button::new("share-delete")
                                        .ghost()
                                        .small()
                                        .label("Delete link")
                                        .text_color(crate::palette::red(cx))
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| this.delete_link(cx))),
                                ),
                        ),
                )
            })
            .when_some(self.busy, |el, doing| {
                el.child(
                    h_flex()
                        .id("share-busy")
                        .test_support()
                        .pt(px(8.))
                        .gap(px(8.))
                        .child(Spinner::new().xsmall())
                        .child(note(doing, cx)),
                )
            })
            .when_some(self.error.clone(), |el, why| {
                el.child(
                    div()
                        .id("share-error")
                        .test_support()
                        .pt(px(8.))
                        .text_size(px(12.5))
                        .line_height(relative(1.45))
                        .text_color(crate::palette::red(cx))
                        .child(why),
                )
            })
    }

    fn export_tab(&self, cx: &mut Context<Self>) -> Div {
        let formats = [
            (
                Format::Markdown,
                "Markdown (.md)",
                "Text with its formatting, for notes and repositories.",
                icon(IconName::FileText, cx),
            ),
            (
                Format::Html,
                "HTML page",
                "One file that opens in any browser, light or dark.",
                icon(IconName::Globe, cx),
            ),
            (
                Format::Json,
                "Transcript (JSON)",
                "Every item as Trek stores it, for scripts and tools.",
                icon(crate::assets::Lucide::Braces, cx),
            ),
        ];
        let mut list = v_flex().gap(px(2.));
        for (i, (format, label, detail, glyph)) in formats.into_iter().enumerate() {
            list = list.child(choice(
                ("share-format", i),
                glyph,
                label,
                detail,
                self.format == format,
                move |this, _, _| this.format = format,
                cx,
            ));
        }
        v_flex().gap(px(10.)).child(list).child(
            v_flex()
                .pt(px(4.))
                .border_t_1()
                .border_color(look::hairline(cx))
                .children(self.options_rows(cx)),
        )
    }

    fn send_tab(&self, cx: &mut Context<Self>) -> Div {
        let ws = self.workspace.read(cx);
        let phone = ws.phone_name().unwrap_or_else(|| "iPhone".to_string());
        let phone_ready = ws.can_send_to_phone();
        let on = crate::machines::split_id(&self.id).map(|(h, _)| h.to_string());
        let machines: Vec<(String, String, bool)> = ws
            .machines
            .list
            .iter()
            .filter(|m| on.as_deref() != Some(m.id()))
            .map(|m| (m.id().to_string(), m.entry.name.clone(), m.reachable()))
            .collect();
        let mut list = v_flex().gap(px(2.)).child(choice(
            "share-to-phone",
            icon(crate::assets::Lucide::Smartphone, cx),
            phone,
            if phone_ready {
                "A notification on your phone that opens the thread in Trek there."
            } else {
                "Turn on phone notifications in Settings › Phone & Machines first."
            },
            self.target == Some(Target::Phone),
            |this, _, _| this.target = Some(Target::Phone),
            cx,
        ));
        for (host, name, reachable) in machines {
            let target = Target::Machine(host.clone());
            let chosen = self.target.as_ref() == Some(&target);
            list = list.child(choice(
                SharedString::from(format!("share-to-machine-{host}")),
                crate::composer::reach_dot(reachable, cx).into_any_element(),
                format!("Continue on {name}"),
                if reachable {
                    format!("A new thread on {name} that starts from this conversation.")
                } else {
                    format!("{name} can't be reached right now.")
                },
                chosen,
                move |this, _, _| this.target = Some(target.clone()),
                cx,
            ));
        }
        if cfg!(target_os = "macos") {
            list = list.child(choice(
                "share-to-system",
                icon(crate::assets::Lucide::Share, cx),
                "More…",
                "AirDrop, Messages, Mail, Notes and the rest of the share menu.",
                self.target == Some(Target::System),
                |this, _, _| this.target = Some(Target::System),
                cx,
            ));
        }
        v_flex().child(list)
    }

    fn primary_label(&self, cx: &App) -> String {
        match self.tab {
            Tab::Link if self.gist.is_some() => "Update link".into(),
            Tab::Link => "Create link".into(),
            Tab::Export => "Save…".into(),
            Tab::SendTo => match &self.target {
                Some(Target::Machine(host)) => format!(
                    "Continue on {}",
                    self.workspace
                        .read(cx)
                        .machines
                        .get(host)
                        .map(|m| m.entry.name.clone())
                        .unwrap_or_default()
                ),
                Some(Target::System) => "Share…".into(),
                _ => "Send".into(),
            },
        }
    }
}

impl Render for ShareSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let words = crate::words::words();
        let title = self.title(cx);
        let view = cx.entity();
        let pick = move |tab: Tab, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.tab = tab;
                this.error = None;
                cx.notify();
            })
        };
        let body = match self.tab {
            Tab::Link => self.link_tab(cx),
            Tab::Export => self.export_tab(cx),
            Tab::SendTo => self.send_tab(cx),
        };
        let primary_off = match self.tab {
            Tab::Link => self.busy.is_some() || (!self.public && self.gist.is_none()),
            Tab::SendTo => self.target.is_none(),
            Tab::Export => false,
        };
        let primary = self.primary_label(cx);
        v_flex()
            .id("share-sheet")
            .test_support()
            .w_full()
            .p(px(PAD))
            .gap(px(14.))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_semibold()
                            .line_clamp(2)
                            .child(format!("Share “{title}”")),
                    )
                    .child(note(
                        format!(
                            "A read-only copy of this thread. The thread itself stays on your {}.",
                            words.computer
                        ),
                        cx,
                    )),
            )
            .child(h_flex().child(look::segmented(
                "share-tab",
                vec![
                    (Tab::Link, "Link"),
                    (Tab::Export, "Export"),
                    (Tab::SendTo, "Send to"),
                ],
                self.tab,
                pick,
                cx,
            )))
            .child(body)
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.foreground.opacity(0.07))
                    .pt(px(12.))
                    .child(
                        Button::new("share-done")
                            .outline()
                            .label("Done")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("share-primary")
                            .primary()
                            .label(primary)
                            .disabled(primary_off)
                            .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx))),
                    ),
            )
    }
}
