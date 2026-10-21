//! The two sheets of machines: "Add a machine…" (Settings › Phone & Machines), which pairs from
//! the other machine's link or its address, code and fingerprint typed; and "Add to another
//! machine…" (the composer's machine chip), which clones this Mac's project there over SSH or
//! HTTPS, copies the files git ignores that are ticked, and starts the draft's thread there.

use super::clone::{self, IgnoredFile, Scheme};
use super::{CloneStage, PairLink, Pairing};
use crate::ui;
use crate::workspace::{Route, SettingsPage, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

const PAD: f32 = 20.;

/// A field as the verify driver reaches it: a wrapper with an id, the input inside.
fn field(id: &'static str, label: impl Into<SharedString>, input: &Entity<InputState>, cx: &App) -> AnyElement {
    v_flex()
        .gap(px(4.))
        .child(div().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().id(id).test_support().child(Input::new(input)))
        .into_any_element()
}

fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_size(px(12.5)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground).child(text.into())
}

// ---------------------------------------------------------------------------------------------
// Add a machine
// ---------------------------------------------------------------------------------------------

/// Open "Add a machine…".
pub fn open_pair(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    workspace.update(cx, |ws, cx| {
        if ws.machines.pairing != Some(Pairing::Busy) {
            ws.machines.pairing = None;
            cx.notify();
        }
    });
    let sheet = cx.new(|cx| PairSheet::new(workspace, window, cx));
    let focus = sheet.clone();
    window.defer(cx, move |window, cx| focus.update(cx, |s, cx| s.link.update(cx, |i, cx| i.focus(window, cx))));
    window.open_dialog(cx, move |dialog, _, _| dialog.w(px(560.)).p(px(0.)).child(sheet.clone()));
}

pub struct PairSheet {
    workspace: Entity<Workspace>,
    typed: bool,
    link: Entity<InputState>,
    host: Entity<InputState>,
    code: Entity<InputState>,
    fingerprint: Entity<InputState>,
    /// What's wrong with what was typed (before anything is dialled).
    error: Option<String>,
    /// Pairing was asked for here: once it's done, the sheet goes.
    asked: bool,
    _subscriptions: Vec<Subscription>,
}

impl PairSheet {
    fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("trek://pair?host=…&code=…&fp=…"));
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.20 or studio.local"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("ABCD-1234"));
        let fingerprint = cx.new(|cx| InputState::new(window, cx).placeholder("64 hex characters"));
        let subs = vec![cx.observe_in(&workspace, window, |this: &mut Self, ws, window, cx| {
            if this.asked && matches!(ws.read(cx).machines.pairing, Some(Pairing::Done(_))) {
                this.asked = false;
                window.close_dialog(cx);
            }
            cx.notify();
        })];
        Self { workspace, typed: false, link, host, code, fingerprint, error: None, asked: false, _subscriptions: subs }
    }

    fn pair(&mut self, cx: &mut Context<Self>) {
        let read = |i: &Entity<InputState>, cx: &App| i.read(cx).value().to_string();
        let link = if self.typed { PairLink::typed(&read(&self.host, cx), &read(&self.code, cx), &read(&self.fingerprint, cx)) } else { PairLink::parse(&read(&self.link, cx)) };
        match link {
            Ok(link) => {
                self.error = None;
                self.asked = true;
                self.workspace.update(cx, |ws, cx| ws.pair_machine(link, cx));
            }
            Err(why) => self.error = Some(why),
        }
        cx.notify();
    }
}

impl Render for PairSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let words = crate::words::words();
        let pairing = self.workspace.read(cx).machines.pairing.clone();
        let busy = pairing == Some(Pairing::Busy);
        let failed = match &pairing {
            Some(Pairing::Failed(why)) if self.error.is_none() => Some(why.clone()),
            _ => self.error.clone(),
        };
        let view = cx.entity();
        let pick = move |typed: bool, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.typed = typed;
                this.error = None;
                let first = if typed { this.host.clone() } else { this.link.clone() };
                first.update(cx, |i, cx| i.focus(window, cx));
                cx.notify();
            })
        };
        let fields = if self.typed {
            v_flex()
                .gap(px(10.))
                .child(field("pair-host-input", "Address", &self.host, cx))
                .child(field("pair-code-input", "Code", &self.code, cx))
                .child(field("pair-fp-input", "Fingerprint (the whole one, as the link carries it)", &self.fingerprint, cx))
        } else {
            v_flex().gap(px(10.)).child(field("pair-link-input", "Pairing link", &self.link, cx))
        };
        v_flex()
            .id("pair-sheet")
            .test_support()
            .w_full()
            .p(px(PAD))
            .gap(px(14.))
            .child(div().text_size(px(16.)).font_semibold().child("Add a machine"))
            .child(note(
                format!(
                    "On the other machine, open Settings › Phone & Machines, let phones connect, and choose “Show pairing code”. Copy the link under “Pair another machine” and paste it here. Threads you start there then show here, in the sidebar under its name. Pairing {} with itself works too.",
                    words.this_computer
                ),
                cx,
            ))
            .child(h_flex().child(ui::segmented("pair-mode", vec![(false, "Pairing link"), (true, "Address and code")], self.typed, pick, cx)))
            .child(fields)
            .when(busy, |el| el.child(h_flex().id("pair-busy").test_support().gap(px(8.)).child(Spinner::new().xsmall()).child(note("Pairing…", cx))))
            .when_some(failed, |el, why| el.child(div().id("pair-error").test_support().text_size(px(12.5)).text_color(crate::palette::red(cx)).child(why)))
            .child(
                h_flex()
                    .pt(px(4.))
                    .gap(px(8.))
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.foreground.opacity(0.07))
                    .pt(px(12.))
                    .child(Button::new("pair-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("pair-submit").primary().label("Pair").disabled(busy).on_click(cx.listener(|this, _, _, cx| this.pair(cx)))),
            )
    }
}

// ---------------------------------------------------------------------------------------------
// Add to another machine
// ---------------------------------------------------------------------------------------------

/// Open "Add to another machine…" for this Mac's project at `project`.
pub fn open_clone(workspace: Entity<Workspace>, project: PathBuf, window: &mut Window, cx: &mut App) {
    workspace.update(cx, |ws, cx| {
        // A finished or failed run from before isn't this one's.
        if ws.machines.clone.as_ref().is_some_and(|r| !matches!(r.stage, CloneStage::Cloning | CloneStage::Copying { .. })) {
            ws.machines.clone = None;
            cx.notify();
        }
    });
    let sheet = cx.new(|cx| CloneSheet::new(workspace, project, window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.w(px(620.)).p(px(0.)).child(sheet.clone()));
}

pub struct CloneSheet {
    workspace: Entity<Workspace>,
    project: PathBuf,
    /// The origin's URL as `git remote get-url origin` gives it; `None` until read, then
    /// `Some(None)` for a project without one.
    origin: Option<Option<String>>,
    scheme: Scheme,
    /// The machine to clone to (host id).
    target: Option<String>,
    folder: Entity<InputState>,
    /// The folder last filled in for the target (replaced when the target changes, unless edited).
    folder_default: String,
    files: Option<Result<Vec<IgnoredFile>, String>>,
    /// What's wrong with what was asked (before anything was sent).
    error: Option<String>,
    started: bool,
    _subscriptions: Vec<Subscription>,
}

impl CloneSheet {
    fn new(workspace: Entity<Workspace>, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("~/repo"));
        let ws = workspace.read(cx);
        let target = ws.draft_machine(&project).or_else(|| ws.machines.list.iter().find(|m| m.reachable()).or(ws.machines.list.first()).map(|m| m.id().to_string()));
        let subs = vec![cx.observe_in(&workspace, window, |this: &mut Self, ws, window, cx| {
            this.sync_folder(window, cx);
            if this.started && ws.read(cx).machines.clone.as_ref().is_some_and(|r| matches!(r.stage, CloneStage::Done { .. })) {
                this.started = false;
                window.close_dialog(cx);
            }
            cx.notify();
        })];
        let root = project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (origin, files) = cx.background_executor().spawn(async move { (clone::origin_url(&root), clone::ignored_files(&root).map_err(|e| e.to_string())) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(url) = &origin {
                    this.scheme = clone::scheme_of(url);
                }
                this.origin = Some(origin);
                this.files = Some(files);
                // The folder's default is named for the repository, now that it's known.
                this.sync_folder(window, cx);
                cx.notify();
            });
        })
        .detach();
        let mut this = Self {
            workspace,
            project,
            origin: None,
            scheme: Scheme::Ssh,
            target,
            folder,
            folder_default: String::new(),
            files: None,
            error: None,
            started: false,
            _subscriptions: subs,
        };
        this.sync_folder(window, cx);
        this
    }

    /// The repository's name, for the default folder.
    fn repo(&self) -> String {
        self.origin.clone().flatten().and_then(|u| clone::repo_name(&u)).or_else(|| self.project.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "repo".into())
    }

    /// `~/<repo>`, or `<home>/<repo>` once the target said where its home is; filled in unless the
    /// user typed another.
    fn sync_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.repo();
        let ws = self.workspace.read(cx);
        let home = self.target.as_deref().and_then(|t| ws.machine_home(t));
        let windows = self.target.as_deref().and_then(|t| ws.machines.get(t)).and_then(|m| m.info.as_ref()).is_some_and(|i| i.platform == "windows");
        let default = match home {
            Some(home) if windows => format!("{}\\{repo}", home.trim_end_matches('\\')),
            Some(home) => format!("{}/{repo}", home.trim_end_matches('/')),
            None => format!("~/{repo}"),
        };
        let now = self.folder.read(cx).value().to_string();
        if default != self.folder_default && (now.is_empty() || now == self.folder_default) {
            self.folder.update(cx, |i, cx| i.set_value(default.clone(), window, cx));
        }
        self.folder_default = default;
    }

    fn url(&self) -> Option<String> {
        self.origin.clone().flatten().map(|u| clone::rewrite_url(&u, self.scheme))
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(url)) = (self.target.clone(), self.url()) else { return };
        let folder = self.folder.read(cx).value().trim().to_string();
        let name = self.project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Err(why) = (trek_remote::CloneProjectRequest { url: url.clone(), folder: folder.clone(), name }).check() {
            self.error = Some(why);
            cx.notify();
            return;
        }
        let files: Vec<String> = match &self.files {
            Some(Ok(files)) => files.iter().filter(|f| f.ticked).map(|f| f.path.clone()).collect(),
            _ => vec![],
        };
        self.error = None;
        self.started = true;
        let project = self.project.clone();
        self.workspace.update(cx, |ws, cx| ws.clone_to_machine(&target, project, url, folder, files, cx));
        cx.notify();
    }

    fn files_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let files = match &self.files {
            None => return h_flex().gap(px(8.)).child(Spinner::new().xsmall()).child(note("Looking for ignored files…", cx)).into_any_element(),
            Some(Err(why)) => return note(format!("Couldn't list the ignored files: {why}"), cx).into_any_element(),
            Some(Ok(files)) if files.is_empty() => return note("Nothing git ignores needs copying.", cx).into_any_element(),
            Some(Ok(files)) => files,
        };
        let ticked: Vec<&IgnoredFile> = files.iter().filter(|f| f.ticked).collect();
        let total: u64 = ticked.iter().map(|f| f.size).sum();
        v_flex()
            .gap(px(4.))
            .child(note(format!("{} of {} ticked, {}. Folders ignored whole (node_modules, target) stay behind.", ticked.len(), files.len(), clone::size_label(total)), cx))
            .child(
                v_flex()
                    .id("clone-files")
                    .test_support()
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.foreground.opacity(0.08))
                    .children(files.iter().enumerate().map(|(ix, f)| {
                        let ticked = f.ticked;
                        h_flex()
                            .id(("clone-file", ix))
                            .test_support()
                            .px(px(10.))
                            .h(px(26.))
                            .gap(px(8.))
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .hover(|s| s.bg(theme.list_hover))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(14.))
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme.foreground.opacity(0.3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(ticked, |el| el.bg(theme.foreground).child(Icon::new(IconName::Check).xsmall().text_color(theme.background))),
                            )
                            .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).child(f.path.clone()))
                            .child(div().flex_none().text_color(theme.muted_foreground).child(clone::size_label(f.size)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Ok(files)) = this.files.as_mut()
                                    && let Some(f) = files.get_mut(ix)
                                {
                                    f.ticked = !f.ticked;
                                }
                                cx.notify();
                            }))
                    })),
            )
            .into_any_element()
    }
}

impl Render for CloneSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let words = crate::words::words();
        let ws = self.workspace.read(cx);
        let machines: Vec<(String, String, bool)> = ws.machines.list.iter().map(|m| (m.id().to_string(), m.entry.name.clone(), m.reachable())).collect();
        let run = ws.machines.clone.clone().filter(|r| Some(&r.host) == self.target.as_ref());
        let running = run.as_ref().is_some_and(|r| matches!(r.stage, CloneStage::Cloning | CloneStage::Copying { .. }));
        let target_name = self.target.as_ref().and_then(|t| machines.iter().find(|(id, ..)| id == t)).map(|(_, n, _)| n.clone());
        let source = format!("{} · {}", capitalized(words.this_computer), trek_core::paths::tildify(&self.project));
        let label = |text: &str| div().w(px(130.)).flex_none().text_size(px(12.5)).text_color(theme.muted_foreground).child(text.to_string());

        if machines.is_empty() {
            let ws = self.workspace.clone();
            return v_flex()
                .id("clone-sheet")
                .test_support()
                .w_full()
                .p(px(PAD))
                .gap(px(14.))
                .child(div().text_size(px(16.)).font_semibold().child("Add to another machine"))
                .child(note("No machine is paired with Trek here yet. Pair one in Settings › Phone & Machines, then come back.", cx))
                .child(h_flex().justify_end().child(Button::new("clone-pair-first").primary().label("Open Phone & Machines").on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    ws.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Mobile), cx));
                })))
                .into_any_element();
        }

        let view = cx.entity();
        let ids: Vec<String> = machines.iter().map(|(id, ..)| id.clone()).collect();
        let current = self.target.as_ref().and_then(|t| ids.iter().position(|id| id == t)).unwrap_or(usize::MAX);
        let pick_target = {
            let view = view.clone();
            move |ix: usize, window: &mut Window, cx: &mut App| {
                let id = ids.get(ix).cloned();
                view.update(cx, |this, cx| {
                    this.target = id;
                    this.sync_folder(window, cx);
                    cx.notify();
                })
            }
        };
        let pick_scheme = move |scheme: Scheme, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.scheme = scheme;
                cx.notify();
            })
        };
        let url = match &self.origin {
            None => note("Reading the origin…", cx).into_any_element(),
            Some(None) => div().id("clone-no-origin").test_support().child(note("This project has no origin remote to clone from. Push it somewhere the other machine can reach first.", cx)).into_any_element(),
            Some(Some(_)) => div()
                .id("clone-url")
                .test_support()
                .min_w_0()
                .truncate()
                .text_size(px(12.5))
                .font_family(theme.mono_font_family.clone())
                .child(self.url().unwrap_or_default())
                .into_any_element(),
        };
        let can_clone = self.target.is_some() && self.url().is_some() && !running;
        let progress = run.as_ref().map(|r| {
            let stage = match &r.stage {
                CloneStage::Cloning => "Cloning…".to_string(),
                CloneStage::Copying { files } => format!("Copying {files} ignored file{}…", if *files == 1 { "" } else { "s" }),
                CloneStage::Done { .. } => "Done".to_string(),
                CloneStage::Failed(_) => "Didn't clone".to_string(),
            };
            v_flex()
                .id("clone-progress")
                .test_support()
                .gap(px(4.))
                .child(h_flex().gap(px(8.)).when(running, |el| el.child(Spinner::new().xsmall())).child(div().text_size(px(12.5)).child(stage)))
                .when(!r.lines.is_empty(), |el| {
                    el.child(
                        v_flex()
                            .p(px(8.))
                            .rounded(px(6.))
                            .bg(theme.foreground.opacity(0.05))
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(11.5))
                            .text_color(theme.muted_foreground)
                            .children(r.lines.iter().rev().take(6).rev().map(|l| div().truncate().child(l.clone()))),
                    )
                })
        });
        let failed = match run.as_ref().map(|r| &r.stage) {
            Some(CloneStage::Failed(why)) => Some(why.clone()),
            _ => self.error.clone(),
        };
        let row = |name: &str, el: AnyElement| h_flex().gap(px(10.)).items_center().child(label(name)).child(div().flex_1().min_w_0().child(el));
        v_flex()
            .id("clone-sheet")
            .test_support()
            .w_full()
            .p(px(PAD))
            .gap(px(12.))
            .child(div().text_size(px(16.)).font_semibold().child("Add to another machine"))
            .child(note("Trek clones the project there with git, copies the files git ignores that you tick (an .env), and starts your thread there.", cx))
            .child(row("From", div().id("clone-source").test_support().text_size(px(12.5)).child(source).into_any_element()))
            .child(row("git clone", url))
            .child(row("Over", ui::segmented("clone-scheme", vec![(Scheme::Ssh, "SSH"), (Scheme::Https, "HTTPS")], self.scheme, pick_scheme, cx)))
            .child(row(
                "To",
                ui::segmented("clone-target", machines.iter().enumerate().map(|(ix, (_, name, reachable))| (ix, if *reachable { name.clone() } else { format!("{name} (offline)") })).collect(), current, pick_target, cx),
            ))
            .child(row(&format!("Folder on {}", target_name.clone().unwrap_or_else(|| "it".into())), field("clone-folder-input", "", &self.folder, cx)))
            .child(v_flex().gap(px(6.)).child(div().text_size(px(12.5)).font_medium().child("Copy ignored files")).child(self.files_list(cx)))
            .children(progress)
            .when_some(failed, |el, why| el.child(div().id("clone-error").test_support().text_size(px(12.5)).text_color(crate::palette::red(cx)).child(why)))
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.foreground.opacity(0.07))
                    .pt(px(12.))
                    .child(Button::new("clone-cancel").outline().label(if running { "Close" } else { "Cancel" }).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("clone-submit")
                            .primary()
                            .label(format!("Clone to {}", target_name.unwrap_or_else(|| "it".into())))
                            .disabled(!can_clone)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .into_any_element()
    }
}

fn capitalized(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}
