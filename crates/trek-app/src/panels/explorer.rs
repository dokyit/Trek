//! Explorer: the project's file tree with a read-only preview.

use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HIDDEN: &[&str] = &[".git", ".DS_Store"];

fn children(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut v: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !HIDDEN.contains(&e.file_name().to_string_lossy().as_ref()))
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
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    preview: Option<Vec<String>>,
    _subscription: Subscription,
}

impl ExplorerPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let root = workspace.read(cx).current_cwd();
        let sub = cx.observe(&workspace, |this, ws, cx| {
            let root = ws.read(cx).current_cwd();
            if root != this.root {
                this.root = root;
                this.expanded.clear();
                this.selected = None;
                this.preview = None;
                cx.notify();
            }
        });
        Self { workspace, root, expanded: HashSet::new(), selected: None, preview: None, _subscription: sub }
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        self.preview = None;
        cx.spawn(async move |this, cx| {
            let lines = cx
                .background_executor()
                .spawn(async move {
                    match std::fs::read(&path) {
                        Ok(bytes) if bytes.len() > 2_000_000 => vec!["File is too large to preview.".to_string()],
                        Ok(bytes) if bytes.iter().take(8000).any(|b| *b == 0) => vec!["Binary file.".to_string()],
                        Ok(bytes) => String::from_utf8_lossy(&bytes).lines().map(|l| l.replace('\t', "    ")).collect(),
                        Err(e) => vec![format!("Couldn't read: {e}")],
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.preview = Some(lines);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
            return super::empty("Open a project to browse its files.", cx).into_any_element();
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
                .child(Icon::new(if is_dir { if open { IconName::FolderOpen } else { IconName::Folder } } else { IconName::File }).small().text_color(theme.muted_foreground))
                .child(div().truncate().child(name))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if is_dir {
                        if !this.expanded.remove(&p) {
                            this.expanded.insert(p.clone());
                        }
                        cx.notify();
                    } else {
                        this.open(p.clone(), cx);
                    }
                }))
        }));

        let preview = self.selected.clone().filter(|p| p.is_file()).map(|path| {
            let lines = self.preview.clone().unwrap_or_default();
            let mono = theme.mono_font_family.clone();
            let rel = path.strip_prefix(&root).map(|p| p.display().to_string()).unwrap_or_default();
            let width = lines.len().to_string().len();
            v_flex()
                .h(relative(0.55))
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .px_3()
                        .h(px(34.))
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_1().truncate().child(rel))
                        .child(crate::ui::icon_button("explorer-open", IconName::ExternalLink, "Open in default app").on_click({
                            let path = path.clone();
                            move |_, _, cx| cx.open_with_system(&path)
                        })),
                )
                .child(
                    uniform_list("explorer-preview", lines.len(), move |range, _, cx| {
                        let theme = cx.theme();
                        range
                            .map(|i| {
                                h_flex()
                                    .h(px(19.))
                                    .px_3()
                                    .gap_3()
                                    .whitespace_nowrap()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .child(div().text_color(theme.muted_foreground.opacity(0.6)).child(format!("{:>width$}", i + 1)))
                                    .child(lines[i].clone())
                            })
                            .collect()
                    })
                    .flex_1(),
                )
        });

        v_flex().size_full().child(tree).children(preview).into_any_element()
    }
}
