//! Explorer: the project's file tree. A file opens in the in-app editor; a folder expands.

use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HIDDEN: &[&str] = &[".git", ".DS_Store", "target"];

fn children(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut v: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                !HIDDEN.contains(&n.as_str()) && !n.ends_with(".nosync")
            })
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    (e.path(), is_dir)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.file_name().cmp(&b.0.file_name())));
    v
}

pub struct ExplorerPanel {
    workspace: Entity<Workspace>,
    root: Option<PathBuf>,
    empty_hint: &'static str,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    _subscription: Subscription,
}


impl ExplorerPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(workspace, |ws| ws.current_cwd(), "Open a project to browse its files.", cx)
    }

    /// The IDE's tree: roots at `ide_root` instead of the route's folder, so it stays put while
    /// the chat column on the right moves between drafts and threads.
    pub fn for_ide(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::from(workspace, |ws| ws.ide_root.clone(), "Open a folder — or a file — to start.", cx)
    }

    fn from(workspace: Entity<Workspace>, root: impl Fn(&Workspace) -> Option<PathBuf> + 'static, empty_hint: &'static str, cx: &mut Context<Self>) -> Self {
        let initial = root(&workspace.read(cx));
        let sub = cx.observe(&workspace, move |this, ws, cx| {
            let root = root(&ws.read(cx));
            if root != this.root {
                this.root = root;
                this.expanded.clear();
                this.selected = None;
                cx.notify();
            }
        });
        Self { workspace, root: initial, empty_hint, expanded: HashSet::new(), selected: None, _subscription: sub }
    }

    fn rows(&self, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, bool, usize)>) {
        for (path, is_dir) in children(dir) {
            let open = is_dir && self.expanded.contains(&path);
            out.push((path.clone(), is_dir, depth));
            if open && out.len() < 4000 {
                self.rows(&path, depth + 1, out);
            }
        }
    }
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.workspace;
        let theme = cx.theme().clone();
        let Some(root) = self.root.clone() else {
            return super::empty(self.empty_hint, cx).into_any_element();
        };
        let mut rows = Vec::new();
        self.rows(&root, 0, &mut rows);
        let selected = self.selected.clone();
        let tree = v_flex().id("explorer-tree").flex_1().min_h_0().overflow_y_scroll().py_1().children(rows.into_iter().map(|(path, is_dir, depth)| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let open = self.expanded.contains(&path);
            let is_sel = selected.as_ref() == Some(&path);
            let p = path.clone();
            h_flex()
                .id(SharedString::from(path.display().to_string()))
                .test_support()
                .mx_1()
                .pl(px(8. + depth as f32 * 14.))
                .pr_2()
                .h(px(26.))
                .gap(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_sm()
                .when(is_sel, |el| el.bg(theme.list_active))
                .when(!is_sel, |el| el.hover(|s| s.bg(theme.list_hover)))
                .child(if is_dir {
                    Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().text_color(theme.muted_foreground).into_any_element()
                } else {
                    div().w(px(12.)).into_any_element()
                })
                .child(if is_dir {
                    crate::file_icon::folder(&name, open, px(14.), cx)
                } else {
                    crate::file_icon::badge(&name, px(14.), cx)
                })
                .child(div().truncate().child(name))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if is_dir {
                        if !this.expanded.remove(&p) {
                            this.expanded.insert(p.clone());
                        }
                        cx.notify();
                    } else {
                        // A file opens in the in-app editor; it highlights here while open.
                        this.selected = Some(p.clone());
                        this.workspace.update(cx, |ws, cx| ws.open_editor(p.clone(), None, cx));
                        cx.notify();
                    }
                }))
        }));

        v_flex().size_full().child(tree).into_any_element()
    }
}
