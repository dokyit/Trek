//! The bottom panel (⌘J): Problems from the language servers (each with "Fix with Agent"),
//! Output, terminals in the IDE folder (their output goes to the AI side bar with "Add to Chat"),
//! and the agents' background tasks.

use super::IdeWorkbench;
use crate::panels::terminal::{Job, TerminalPanel};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types::DiagnosticSeverity;

/// Lines of a terminal's output "Add to Chat" takes.
const TERMINAL_LINES: usize = 40;
/// Lines either side of a problem that go with "Fix with Agent".
const PROBLEM_CONTEXT: u32 = 3;

/// What the Output panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputChannel {
    Servers,
    /// What a chat's agent wrote to stderr.
    Thread(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Problems,
    Output,
    Terminal,
    Tasks,
}

impl PanelTab {
    const ALL: [PanelTab; 4] = [PanelTab::Problems, PanelTab::Output, PanelTab::Terminal, PanelTab::Tasks];

    fn label(self) -> &'static str {
        match self {
            PanelTab::Problems => "Problems",
            PanelTab::Output => "Output",
            PanelTab::Terminal => "Terminal",
            PanelTab::Tasks => "Agent Tasks",
        }
    }

    fn id(self) -> &'static str {
        match self {
            PanelTab::Problems => "ide-panel-problems",
            PanelTab::Output => "ide-panel-output",
            PanelTab::Terminal => "ide-panel-terminal",
            PanelTab::Tasks => "ide-panel-tasks",
        }
    }
}

impl IdeWorkbench {
    /// Show the panel on `tab` (a terminal is started the first time Terminal shows).
    pub fn show_panel(&mut self, tab: PanelTab, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_tab = tab;
        if !self.layout.panel_open {
            self.layout.panel_open = true;
            self.save_layout(cx);
        }
        if tab == PanelTab::Terminal {
            if self.terminals.is_empty() {
                self.new_terminal(cx);
            }
            if let Some(t) = self.terminals.get(self.terminal) {
                t.read(cx).focus_handle().focus(window, cx);
            }
        }
        cx.notify();
    }

    /// ⌃`: the terminal, opened (or hidden when it's what has the panel already).
    pub fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.panel_open && self.panel_tab == PanelTab::Terminal {
            self.layout.panel_open = false;
            self.save_layout(cx);
        } else {
            self.show_panel(PanelTab::Terminal, window, cx);
        }
    }

    /// Run a command (an agent install or sign-in, a project action) in a terminal of its own
    /// in the panel; agents are looked for again when it exits.
    pub fn run_command(&mut self, job: Job, cwd: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cwd.or_else(|| self.workspace.read(cx).ide_root.clone());
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        let ws = self.workspace.downgrade();
        let term = cx.new(|cx| {
            let mut t = TerminalPanel::with_command(cwd, job, shell, cx);
            t.on_exit(move |cx| {
                let _ = ws.update(cx, |ws, cx| ws.detect_agents(cx));
            });
            t
        });
        self.terminals.push(term);
        self.terminal = self.terminals.len() - 1;
        self.show_panel(PanelTab::Terminal, window, cx);
    }

    /// The terminal's latest output goes to the AI side bar as a chip.
    pub fn terminal_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminals.get(self.terminal) else { return };
        let text = t.read(cx).recent_output(TERMINAL_LINES);
        if text.trim().is_empty() {
            return;
        }
        self.add_chip(crate::ide::ai::context::ContextChip::Terminal { text }, window, cx);
    }

    /// "Fix with Agent": the problem, with the lines around it, goes to the AI side bar's chat
    /// as a request to fix it (a new chat when it's on none).
    pub fn fix_problem(&mut self, path: std::path::PathBuf, d: lsp_types::Diagnostic, window: &mut Window, cx: &mut Context<Self>) {
        let line = d.range.start.line;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let (a, b) = (line.saturating_sub(PROBLEM_CONTEXT), (d.range.end.line + PROBLEM_CONTEXT).min(lines.len().saturating_sub(1) as u32));
        let snippet = lines.get(a as usize..=b as usize).map(|l| l.join("\n")).unwrap_or_default();
        let severity = match d.severity {
            Some(DiagnosticSeverity::ERROR) => "error",
            Some(DiagnosticSeverity::WARNING) => "warning",
            _ => "note",
        };
        let chip = crate::ide::ai::context::ContextChip::Problem { path, line: line + 1, severity: severity.into(), message: d.message.clone(), lines: (a + 1, b + 1), text: snippet };
        if !self.layout.ai_open {
            self.layout.ai_open = true;
            self.save_layout(cx);
        }
        let input = self.ai.read(cx).input.clone();
        input.update(cx, |i, cx| i.send_request("Fix this problem.", &[chip], cx));
        self.ai.update(cx, |a, cx| a.focus(window, cx));
    }

    fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let cwd = self.workspace.read(cx).ide_root.clone();
        let shell = self.workspace.read(cx).settings.terminal.shell.clone();
        self.terminals.push(cx.new(|cx| TerminalPanel::new(cwd, shell, cx)));
        self.terminal = self.terminals.len() - 1;
        cx.notify();
    }

    fn kill_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal < self.terminals.len() {
            self.terminals.remove(self.terminal);
        }
        self.terminal = self.terminal.min(self.terminals.len().saturating_sub(1));
        if self.terminals.is_empty() {
            self.layout.panel_open = false;
            self.save_layout(cx);
        }
        cx.notify();
    }

    pub(super) fn bottom_panel(&self, height: f32, glass: Option<f32>, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let line = Self::line(glass, cx);
        let ws = self.workspace.read(cx);
        let (errors, warnings) = ws.diagnostic_counts();
        let tasks: usize = ws.ide_threads().iter().filter_map(|t| ws.live.get(&t.id)).map(|l| l.background.len()).sum();
        let tab = self.panel_tab;
        let ember = crate::palette::ember(cx);
        let tabs = h_flex()
            .h(px(32.))
            .flex_none()
            .px(px(10.))
            .gap(px(4.))
            .children(PanelTab::ALL.into_iter().map(|t| {
                let on = t == tab;
                let count = match t {
                    PanelTab::Problems => errors + warnings,
                    PanelTab::Tasks => tasks,
                    _ => 0,
                };
                h_flex()
                    .id(t.id())
                    .test_support()
                    .relative()
                    .h_full()
                    .px(px(8.))
                    .gap(px(5.))
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if on { theme.foreground } else { theme.muted_foreground })
                    .when(!on, |el| el.hover(|s| s.text_color(theme.foreground)))
                    .child(t.label().to_uppercase())
                    .when(count > 0, |el| {
                        el.child(div().px(px(5.)).rounded_full().bg(theme.foreground.opacity(0.1)).text_size(px(10.)).child(count.to_string()))
                    })
                    .when(on, |el| el.child(div().absolute().bottom(px(4.)).left(px(8.)).right(px(8.)).h(px(1.5)).bg(ember)))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_panel(t, window, cx)))
            }))
            .child(div().flex_1())
            .when(tab == PanelTab::Terminal && !self.terminals.is_empty(), |el| {
                el.child(
                    crate::ui::icon_button("ide-terminal-chat", crate::assets::Lucide::MessageSquarePlus, "Add to Chat: the latest output").on_click(cx.listener(|this, _, window, cx| this.terminal_to_chat(window, cx))),
                )
            })
            .when(tab == PanelTab::Terminal, |el| {
                el.child(crate::ui::icon_button("ide-terminal-new", IconName::Plus, "New terminal").on_click(cx.listener(|this, _, window, cx| {
                    this.new_terminal(cx);
                    this.show_panel(PanelTab::Terminal, window, cx);
                })))
                .child(crate::ui::icon_button("ide-terminal-kill", crate::assets::Lucide::Trash, "Kill terminal").on_click(cx.listener(|this, _, _, cx| this.kill_terminal(cx))))
            })
            .child(crate::ui::icon_button("ide-panel-close", IconName::Close, "Hide panel (⌘J)").on_click(cx.listener(|this, _, window, cx| this.toggle_panel(window, cx))));

        let body: AnyElement = match tab {
            PanelTab::Terminal => self.terminal_body(cx),
            PanelTab::Problems => self.problems(cx),
            PanelTab::Output => self.output(cx),
            PanelTab::Tasks => self.agent_tasks(cx),
        };
        v_flex()
            .id("ide-panel")
            .test_support()
            .relative()
            .h(px(height))
            .w_full()
            .flex_none()
            .border_t_1()
            .border_color(line)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .child(self.handle(super::layout::Split::Panel, cx))
    }

    fn terminal_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let fill = StyleRefinement::default().size_full();
        if self.terminals.len() > 1 {
            // More than one: their names down the right, as VS Code lists them.
            return h_flex()
                .size_full()
                .child(div().flex_1().min_w_0().h_full().children(self.terminals.get(self.terminal).map(|t| t.clone().cached(fill.clone()))))
                .child(
                    v_flex().w(px(140.)).h_full().flex_none().border_l_1().border_color(theme.border).py_1().children(self.terminals.iter().enumerate().map(|(ix, _)| {
                        let on = ix == self.terminal;
                        h_flex()
                            .id(("ide-terminal", ix))
                            .h(px(24.))
                            .px(px(10.))
                            .gap(px(6.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .when(on, |el| el.bg(theme.list_active))
                            .when(!on, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.list_hover)))
                            .child(Icon::new(IconName::SquareTerminal).xsmall())
                            .child(format!("zsh {}", ix + 1))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.terminal = ix;
                                cx.notify();
                            }))
                    })),
                )
                .into_any_element();
        }
        match self.terminals.first() {
            Some(t) => div().size_full().pl(px(10.)).child(t.clone().cached(fill)).into_any_element(),
            None => crate::panels::empty("No terminal. + starts one in this folder.", cx).into_any_element(),
        }
    }

    /// What the language servers say about the open files, by file; a row opens the file there.
    fn problems(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let ws = self.workspace.read(cx);
        let root = ws.ide_root.clone();
        let mut files: Vec<_> = ws.diagnostics.iter().map(|(p, d)| (p.clone(), d.clone())).collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        if files.is_empty() {
            return crate::panels::empty("No problems in the open files.", cx).into_any_element();
        }
        let mut list = v_flex().id("ide-problems").size_full().overflow_y_scroll().py_1();
        let mut row_ix = 0usize;
        for (path, diags) in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dir = path.parent().map(|d| root.as_ref().and_then(|r| d.strip_prefix(r).ok()).map(|r| r.display().to_string()).unwrap_or_else(|| trek_core::paths::tildify(d))).unwrap_or_default();
            list = list.child(
                h_flex()
                    .h(px(22.))
                    .px(px(12.))
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .child(crate::file_icon::badge(&name, px(13.), cx))
                    .child(name)
                    .child(div().text_size(px(11.5)).text_color(theme.muted_foreground).child(dir))
                    .child(div().px(px(5.)).rounded_full().bg(theme.foreground.opacity(0.08)).text_size(px(10.)).child(diags.len().to_string())),
            );
            for d in diags {
                let (icon, color) = match d.severity {
                    Some(DiagnosticSeverity::ERROR) => (IconName::CircleX, crate::palette::red(cx)),
                    Some(DiagnosticSeverity::WARNING) => (IconName::TriangleAlert, crate::palette::amber(cx)),
                    _ => (IconName::Info, crate::palette::sky(cx)),
                };
                let line = d.range.start.line + 1;
                let col = d.range.start.character + 1;
                let p = path.clone();
                let (fix_path, fix) = (path.clone(), d.clone());
                let group = SharedString::from(format!("ide-problem-{row_ix}"));
                list = list.child(
                    h_flex()
                        .id(("ide-problem", row_ix))
                        .test_support()
                        .group(group.clone())
                        .h(px(22.))
                        .pl(px(32.))
                        .pr(px(12.))
                        .gap(px(8.))
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(Icon::new(icon).size(px(13.)).text_color(color))
                        .child(div().flex_1().min_w_0().truncate().child(d.message.lines().next().unwrap_or_default().to_string()))
                        .child(div().flex_none().text_size(px(11.5)).text_color(theme.muted_foreground).child(format!("[Ln {line}, Col {col}]")))
                        .child(
                            h_flex()
                                .id(("ide-problem-fix", row_ix))
                                .test_support()
                                .flex_none()
                                .h(px(18.))
                                .px(px(7.))
                                .gap(px(4.))
                                .items_center()
                                .rounded(px(4.))
                                .text_size(px(11.5))
                                .text_color(theme.muted_foreground)
                                .group_hover(group, |s| s.text_color(ember))
                                .hover(|s| s.bg(ember.opacity(0.12)))
                                .child(Icon::new(crate::assets::Lucide::Sparkles).size(px(11.)))
                                .child("Fix with Agent")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.fix_problem(fix_path.clone(), fix.clone(), window, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.open(p.clone(), Some(line), false, window, cx))),
                );
                row_ix += 1;
            }
        }
        list.into_any_element()
    }

    /// Output: a channel at a time, picked from its menu. "Language Servers": the servers on
    /// duty for the open files; an agent's: what its processes wrote to stderr this session
    /// (warnings, a crash's last words), for the folder's chats.
    fn output(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let threads: Vec<(String, String, Option<trek_agents::SessionLog>)> =
            ws.ide_threads().into_iter().map(|t| (t.id.clone(), format!("{} · {}", t.agent.display_name(), t.title), ws.live.get(&t.id).and_then(|l| l.stderr.clone()))).collect();
        // The chat in front's channel unless another was picked.
        let channel = match &self.output_channel {
            Some(c) => c.clone(),
            None => ws.ide_chat.active_thread().filter(|id| threads.iter().any(|(t, _, _)| t == *id)).map_or(OutputChannel::Servers, |id| OutputChannel::Thread(id.to_string())),
        };
        let label = match &channel {
            OutputChannel::Servers => "Language Servers".to_string(),
            OutputChannel::Thread(id) => threads.iter().find(|(t, _, _)| t == id).map_or_else(|| "Agent".into(), |(_, l, _)| l.clone()),
        };
        let me = cx.weak_entity();
        let listed: Vec<(String, String)> = threads.iter().map(|(id, label, _)| (id.clone(), label.clone())).collect();
        let picker = gpui_kit::component::button::Button::new("ide-output-channel")
            .ghost()
            .xsmall()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                let pick = |c: OutputChannel| {
                    let me = me.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = me.update(cx, |this, cx| {
                            this.output_channel = Some(c.clone());
                            cx.notify();
                        });
                    }
                };
                menu = menu.min_w(px(240.)).item(PopupMenuItem::new("Language Servers").on_click(pick(OutputChannel::Servers)));
                if !listed.is_empty() {
                    menu = menu.separator().label("Agents");
                }
                for (id, label) in &listed {
                    menu = menu.item(PopupMenuItem::new(label.clone()).on_click(pick(OutputChannel::Thread(id.clone()))));
                }
                menu
            });
        let mono = cx.theme().mono_font_family.clone();
        let body: AnyElement = match &channel {
            OutputChannel::Servers => {
                let mut servers: Vec<(&'static str, Vec<String>)> = vec![];
                for e in self.editors() {
                    let e = e.read(cx);
                    let Some(name) = e.lsp_name else { continue };
                    let file = e.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    match servers.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, files)) => files.push(file),
                        None => servers.push((name, vec![file])),
                    }
                }
                if servers.is_empty() {
                    crate::panels::empty("No language server is running. They start as files open.", cx).into_any_element()
                } else {
                    v_flex()
                        .id("ide-output-servers")
                        .test_support()
                        .size_full()
                        .font_family(mono.clone())
                        .overflow_y_scroll()
                        .gap(px(4.))
                        .children(servers.into_iter().map(|(name, files)| {
                            h_flex().gap(px(8.)).child(div().text_color(crate::palette::emerald(cx)).child("●")).child(name).child(div().text_color(theme.muted_foreground).child(format!("· {}", files.join(", "))))
                        }))
                        .into_any_element()
                }
            }
            OutputChannel::Thread(id) => {
                let lines = threads.iter().find(|(t, _, _)| t == id).and_then(|(_, _, l)| l.as_ref()).map(|l| l.lines()).unwrap_or_default();
                if lines.is_empty() {
                    crate::panels::empty("Nothing on stderr from this chat's agent in its current session.", cx).into_any_element()
                } else {
                    v_flex()
                        .id("ide-output-log")
                        .test_support()
                        .size_full()
                        .overflow_y_scroll()
                        .font_family(mono.clone())
                        .children(lines.into_iter().map(|l| div().whitespace_normal().child(l)))
                        .into_any_element()
                }
            }
        };
        v_flex()
            .id("ide-output")
            .test_support()
            .size_full()
            .child(h_flex().h(px(26.)).flex_none().px(px(8.)).child(picker))
            .child(div().flex_1().min_h_0().px(px(14.)).py(px(4.)).text_size(px(12.)).child(body))
            .into_any_element()
    }

    /// The background tasks of the folder's threads (shells left running, monitors, sub-agents),
    /// each with Stop where its agent can stop it.
    fn agent_tasks(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let rows: Vec<(String, String, String, String, bool, std::time::Duration)> = ws
            .ide_threads()
            .into_iter()
            .filter_map(|t| ws.live.get(&t.id).map(|l| (t, l)))
            .flat_map(|(t, l)| l.background.iter().map(move |b| (t.id.clone(), t.title.clone(), b.task.id.clone(), b.task.title.clone(), b.task.stoppable && !b.stopping, b.started.elapsed())))
            .collect();
        if rows.is_empty() {
            return crate::panels::empty("No agent is running anything in the background.", cx).into_any_element();
        }
        v_flex()
            .id("ide-tasks")
            .size_full()
            .overflow_y_scroll()
            .py_1()
            .children(rows.into_iter().enumerate().map(|(ix, (thread, title, task, what, stoppable, took))| {
                h_flex()
                    .id(("ide-task", ix))
                    .h(px(26.))
                    .px(px(12.))
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .hover(|s| s.bg(theme.list_hover))
                    .child(gpui_kit::component::spinner::Spinner::new().xsmall().color(theme.muted_foreground))
                    .child(div().flex_1().min_w_0().truncate().font_family(cx.theme().mono_font_family.clone()).child(what))
                    .child(div().flex_none().max_w(px(220.)).truncate().text_color(theme.muted_foreground).child(title))
                    .child(div().flex_none().text_color(theme.muted_foreground).child(crate::time::elapsed(took)))
                    .when(stoppable, |el| {
                        el.child(crate::ui::icon_button(("ide-task-stop", ix), crate::assets::Lucide::Square, "Stop").on_click(cx.listener(move |this, _, _, cx| {
                            let (thread, task) = (thread.clone(), task.clone());
                            this.workspace.update(cx, |ws, cx| ws.stop_background(&thread, &task, cx));
                        })))
                    })
            }))
            .into_any_element()
    }
}
