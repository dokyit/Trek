//! The status bar along the editor's foot: the branch, problems and agents at work on the left;
//! where the caret is and what the file is on the right. A view of its own, so the caret moving
//! redraws only this row.

use super::{IdeWorkbench, PanelTab, SideView};
use crate::editor::EditorView;
use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::RunState;

pub const HEIGHT: f32 = 24.;

pub struct IdeStatus {
    workspace: Entity<Workspace>,
    workbench: WeakEntity<IdeWorkbench>,
    editor: Option<Entity<EditorView>>,
    /// Watches the editor in front (dirty, read-only) and its text (the caret).
    _editor_watch: Vec<Subscription>,
    _subscription: Subscription,
}

/// The language a file is in, by its extension, as the status bar names it.
pub fn language(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or_default() {
        "rs" => "Rust",
        "swift" => "Swift",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JavaScript JSX",
        "py" => "Python",
        "go" => "Go",
        "c" | "h" => "C",
        "cc" | "cpp" | "hpp" | "cxx" => "C++",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "rb" => "Ruby",
        "lua" => "Lua",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" | "markdown" => "Markdown",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "sh" | "bash" | "zsh" => "Shell Script",
        _ => "Plain Text",
    }
}

impl IdeStatus {
    pub fn new(workspace: Entity<Workspace>, workbench: WeakEntity<IdeWorkbench>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self { workspace, workbench, editor: None, _editor_watch: vec![], _subscription }
    }

    /// The editor in front changed (or none is open).
    pub fn set_editor(&mut self, editor: Option<Entity<EditorView>>, cx: &mut Context<Self>) {
        if self.editor == editor {
            return;
        }
        self._editor_watch = editor
            .as_ref()
            .map(|e| {
                let text = e.read(cx).text_state();
                vec![cx.observe(e, |_, _, cx| cx.notify()), cx.observe(&text, |_, _, cx| cx.notify())]
            })
            .unwrap_or_default();
        self.editor = editor;
        cx.notify();
    }

    fn item(id: &'static str, cx: &App) -> Stateful<Div> {
        let theme = cx.theme();
        h_flex().id(id).h_full().px(px(7.)).gap(px(5.)).items_center().cursor_pointer().hover(|s| s.bg(theme.foreground.opacity(0.06)))
    }

    fn on_workbench(&self, f: impl Fn(&mut IdeWorkbench, &mut Window, &mut Context<IdeWorkbench>) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let wb = self.workbench.clone();
        move |_, window, cx| {
            let _ = wb.update(cx, |wb, cx| f(wb, window, cx));
        }
    }
}

impl Render for IdeStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let glass = ws.glass();
        let git = ws.ide_root.as_ref().and_then(|r| ws.git_info.get(r)).filter(|g| g.is_repo).cloned();
        let (errors, warnings) = ws.diagnostic_counts();
        let threads = ws.ide_threads();
        let working = threads.iter().filter(|t| t.run_state == RunState::Working).count();
        let needs = threads.iter().filter(|t| t.needs_you()).count();
        let ember = crate::palette::ember(cx);
        let agents = match (working, needs) {
            (0, 0) => None,
            (w, 0) => Some(format!("{w} agent{} working", if w == 1 { "" } else { "s" })),
            (0, n) => Some(format!("{n} need{} you", if n == 1 { "s" } else { "" })),
            (w, n) => Some(format!("{w} working · {n} need{} you", if n == 1 { "s" } else { "" })),
        };
        let editor = self.editor.as_ref().map(|e| {
            let e = e.read(cx);
            (e.caret(cx), language(&e.path), e.lsp_name, e.problem().is_some())
        });
        let muted = theme.muted_foreground;
        h_flex()
            .id("ide-status")
            .test_support()
            .size_full()
            .px(px(5.))
            .bg(crate::ui::chrome_bg(glass, cx))
            .border_t_1()
            .border_color(if glass.is_some() { crate::ui::panel_border(glass, cx) } else { theme.border })
            .text_size(px(11.5))
            .text_color(muted)
            .when_some(git, |el, g| {
                let branch = g.branch.clone().unwrap_or_else(|| "detached".into());
                el.child(
                    Self::item("ide-status-branch", cx)
                        .child(Icon::new(crate::assets::Lucide::GitBranch).size(px(12.)))
                        .child(format!("{branch}{}", if g.changed > 0 { "*" } else { "" }))
                        .when(g.ahead + g.behind > 0, |el| el.child(format!("↓{} ↑{}", g.behind, g.ahead)))
                        .on_click(self.on_workbench(|wb, window, cx| wb.show_view(SideView::Scm, window, cx))),
                )
            })
            .child(
                Self::item("ide-status-problems", cx)
                    .child(Icon::new(IconName::CircleX).size(px(12.)).when(errors > 0, |i| i.text_color(crate::palette::red(cx))))
                    .child(errors.to_string())
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.)).when(warnings > 0, |i| i.text_color(crate::palette::amber(cx))))
                    .child(warnings.to_string())
                    .on_click(self.on_workbench(|wb, window, cx| wb.show_panel(PanelTab::Problems, window, cx))),
            )
            .when_some(agents, |el, text| {
                el.child(
                    Self::item("ide-status-agents", cx)
                        .text_color(if needs > 0 { crate::palette::amber(cx) } else { ember })
                        .child(Icon::new(crate::assets::Lucide::Sparkles).size(px(12.)))
                        .child(text)
                        .on_click(self.on_workbench(|wb, window, cx| wb.show_view(SideView::Agents, window, cx))),
                )
            })
            .child(div().flex_1())
            .when_some(editor, |el, ((line, col), lang, lsp, read_only)| {
                el.child(Self::item("ide-status-caret", cx).child(format!("Ln {line}, Col {col}")))
                    .when(read_only, |el| el.child(Self::item("ide-status-readonly", cx).child("Read-only")))
                    .child(Self::item("ide-status-indent", cx).child("Spaces: 4"))
                    .child(Self::item("ide-status-encoding", cx).child("UTF-8"))
                    .child(Self::item("ide-status-eol", cx).child("LF"))
                    .child(Self::item("ide-status-language", cx).child(lang))
                    .when_some(lsp, |el, name| el.child(Self::item("ide-status-lsp", cx).child(div().text_color(crate::palette::emerald(cx)).child("●")).child(name)))
            })
            .child(
                Self::item("ide-status-ai", cx)
                    .child(Icon::new(IconName::PanelRight).size(px(12.)))
                    .on_click(self.on_workbench(|wb, window, cx| wb.toggle_ai(window, cx))),
            )
    }
}
