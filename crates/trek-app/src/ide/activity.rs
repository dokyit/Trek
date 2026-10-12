//! The primary side bar: a row of view icons along its top (as Cursor has them, saving VS Code's
//! 48 px rail), and the view they pick: Explorer, Search, Source Control or Agents.

use super::IdeWorkbench;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideView {
    Explorer,
    Search,
    Scm,
    Agents,
}

impl SideView {
    pub const ALL: [SideView; 4] = [SideView::Explorer, SideView::Search, SideView::Scm, SideView::Agents];

    fn icon(self) -> Icon {
        match self {
            SideView::Explorer => Icon::new(crate::assets::Lucide::Files),
            SideView::Search => Icon::new(IconName::Search),
            SideView::Scm => Icon::new(crate::assets::Lucide::GitBranch),
            SideView::Agents => Icon::new(crate::assets::Lucide::Sparkles),
        }
    }

    fn tooltip(self) -> std::borrow::Cow<'static, str> {
        match self {
            SideView::Explorer => crate::keys::localize("Explorer (⌘⇧E)"),
            SideView::Search => crate::keys::localize("Search (⌘⇧F)"),
            SideView::Scm => crate::keys::localize("Source Control (⌃⇧G)"),
            SideView::Agents => "Agents in this folder".into(),
        }
    }

    fn id(self) -> &'static str {
        match self {
            SideView::Explorer => "ide-view-explorer",
            SideView::Search => "ide-view-search",
            SideView::Scm => "ide-view-scm",
            SideView::Agents => "ide-view-agents",
        }
    }
}

/// A side bar section's heading: small caps text with its actions on the right.
pub(super) fn section_header(title: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .h(px(28.))
        .flex_none()
        .px(px(12.))
        .gap(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .child(div().flex_1().min_w_0().truncate().child(title.into().to_uppercase()))
}

impl IdeWorkbench {
    /// The view icons, each with a count where it has news (changed files, agents needing you).
    pub(super) fn activity_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let changed = ws.ide_root.as_ref().and_then(|r| ws.git_info.get(r)).map_or(0, |g| g.changed);
        let needs = ws.ide_threads().iter().filter(|t| t.needs_you()).count();
        let ember = crate::palette::ember(cx);
        h_flex()
            .h(px(36.))
            .flex_none()
            .px(px(8.))
            .gap(px(2.))
            .border_b_1()
            .border_color(Self::line(ws.glass(), cx))
            .children(SideView::ALL.into_iter().map(|v| {
                let on = v == self.view;
                let badge = match v {
                    SideView::Scm => changed,
                    SideView::Agents => needs,
                    _ => 0,
                };
                div()
                    .id(v.id())
                    .test_support()
                    .relative()
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |el| el.bg(theme.foreground.opacity(0.08)))
                    .when(!on, |el| el.hover(|s| s.bg(theme.foreground.opacity(0.05))))
                    .child(v.icon().size(px(16.)).text_color(if on { theme.foreground } else { theme.muted_foreground }))
                    .when(badge > 0, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .right(px(0.))
                                .min_w(px(14.))
                                .h(px(13.))
                                .px(px(3.))
                                .rounded_full()
                                .bg(ember)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(gpui_kit::white())
                                .child(if badge > 99 { "99+".to_string() } else { badge.to_string() }),
                        )
                    })
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(v.tooltip().into_owned()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_view(v, window, cx)))
            }))
    }

    /// The view picked in the activity row.
    pub(super) fn side_view(&mut self, fill: impl Fn() -> StyleRefinement, cx: &mut Context<Self>) -> AnyElement {
        match self.view {
            SideView::Explorer => self.explorer_view(fill, cx),
            SideView::Search => v_flex()
                .flex_1()
                .min_h_0()
                .child(section_header("Search", cx))
                .child(self.search.clone().cached(StyleRefinement::default().w_full().flex_1().min_h_0()))
                .into_any_element(),
            SideView::Scm => v_flex()
                .flex_1()
                .min_h_0()
                .child(section_header("Source Control", cx))
                .children(self.scm.clone().map(|g| g.cached(StyleRefinement::default().w_full().flex_1().min_h_0())))
                .into_any_element(),
            SideView::Agents => self.agents_view(cx),
        }
    }

    /// The folder's tree under its name (with New file, Refresh and Collapse), then the agents
    /// working in it.
    fn explorer_view(&mut self, fill: impl Fn() -> StyleRefinement, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let name = ws.ide_root.as_ref().map(|r| r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.display().to_string()));
        let has_root = name.is_some();
        let _ = fill;
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                section_header(match &name {
                    Some(n) => format!("Explorer · {n}"),
                    None => "Explorer".into(),
                }, cx)
                .when(has_root, |el| {
                    el.child(
                        crate::ui::icon_button("ide-explorer-new-file", crate::assets::Lucide::FilePlus, "New File…").on_click(cx.listener(|this, _, window, cx| {
                            this.explorer.update(cx, |e, cx| {
                                if let Some(dir) = e.target_dir() {
                                    e.begin_new(dir, false, window, cx);
                                }
                            })
                        })),
                    )
                    .child(
                        crate::ui::icon_button("ide-explorer-new-folder", crate::assets::Lucide::FolderPlus, "New Folder…").on_click(cx.listener(|this, _, window, cx| {
                            this.explorer.update(cx, |e, cx| {
                                if let Some(dir) = e.target_dir() {
                                    e.begin_new(dir, true, window, cx);
                                }
                            })
                        })),
                    )
                    .child(
                        crate::ui::icon_button("ide-explorer-refresh", IconName::RefreshCw, "Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.explorer.update(cx, |e, cx| e.refresh(cx)))),
                    )
                    .child(
                        crate::ui::icon_button("ide-explorer-collapse", crate::assets::Lucide::ChevronsDownUp, "Collapse folders")
                            .on_click(cx.listener(|this, _, _, cx| this.explorer.update(cx, |e, cx| e.collapse_all(cx)))),
                    )
                })
                .when(!has_root, |el| {
                    el.child(
                        crate::ui::icon_button("ide-explorer-open", IconName::FolderOpen, "Open folder")
                            .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.open_ide_folder(cx)))),
                    )
                }),
            )
            .child(self.explorer.clone().cached(StyleRefinement::default().w_full().flex_1().min_h_0()))
            .children(has_root.then(|| self.agents_section(cx)))
            .into_any_element()
    }
}
