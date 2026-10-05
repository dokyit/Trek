//! The card under a finished turn's answer when the turn changed files
//! (`Workspace::load_turn_changes`): "CHANGED FILES (2) · +28 / −6" with Collapse all and View
//! diff, then the files grouped by folder, each in its type's colour with the lines it gained and
//! lost. Drawn with the theme's foreground at low opacities, like the rest of the transcript, so
//! it sits right on Night, Paper and glass alike.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::rc::Rc;
use trek_core::changes::{Counted, FileChange, FileStatus, TurnChanges};

/// What the card's controls do.
pub struct Actions {
    /// Fold or unfold one folder.
    pub toggle_dir: Rc<dyn Fn(&str, &mut App)>,
    /// Fold every folder (`true`), or unfold them all.
    pub fold_all: Rc<dyn Fn(bool, &mut App)>,
    /// Show the turn's diff in the Git tool, a file's open; `None` when there's none to show
    /// (counted without git, or no Git tool in this window).
    pub view_diff: Option<Rc<dyn Fn(Option<String>, &mut Window, &mut App)>>,
    /// A file clicked when there's no diff to show: show it in Finder.
    pub reveal: Rc<dyn Fn(&FileChange, &mut Window, &mut App)>,
}

/// A folder of the card and its files, in order.
pub fn folders(changes: &TurnChanges) -> Vec<(String, Vec<&FileChange>)> {
    let mut out: Vec<(String, Vec<&FileChange>)> = vec![];
    for f in &changes.files {
        let dir = f.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        match out.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, files)) => files.push(f),
            None => out.push((dir, vec![f])),
        }
    }
    // The top folder's own files first, then the folders by path.
    out.sort_by(|a, b| (!a.0.is_empty()).cmp(&!b.0.is_empty()).then_with(|| a.0.cmp(&b.0)));
    out
}

/// The card for the turn ending at transcript item `ix`; `folded`: the folders folded.
pub fn card(ix: usize, changes: &TurnChanges, folded: &HashSet<String>, actions: Actions, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let line = theme.foreground.opacity(0.07);
    let (green, red) = (crate::palette::emerald(cx), crate::palette::red(cx));
    let (added, removed) = changes.totals();
    let groups = folders(changes);
    let all_folded = groups.iter().all(|(d, _)| folded.contains(d));
    let fold_all = actions.fold_all.clone();
    let head = h_flex()
        .id(("changes-head", ix))
        .test_support()
        .gap(px(6.))
        .pl(px(12.))
        .pr(px(6.))
        .h(px(34.))
        .text_size(px(11.))
        .font_medium()
        .text_color(muted)
        .child(format!("CHANGED FILES ({})", changes.files.len()))
        .when(added > 0 || removed > 0, |el| {
            el.child("·").child(
                h_flex()
                    .gap(px(4.))
                    .font_family(theme.mono_font_family.clone())
                    .child(div().text_color(green).child(format!("+{added}")))
                    .child("/")
                    .child(div().text_color(red).child(format!("−{removed}"))),
            )
        })
        .child(div().flex_1())
        .child(
            Button::new(("changes-fold", ix))
                .ghost()
                .xsmall()
                .icon(Icon::new(if all_folded { crate::assets::Lucide::ChevronsUpDown } else { crate::assets::Lucide::ChevronsDownUp }).text_color(muted))
                .label(if all_folded { "Expand all" } else { "Collapse all" })
                .on_click(move |_, _, cx| fold_all(!all_folded, cx)),
        )
        .when_some(actions.view_diff.clone(), |el, view| {
            el.child(Button::new(("changes-diff", ix)).ghost().xsmall().icon(Icon::new(crate::assets::Lucide::FileDiff).text_color(muted)).label("View diff").on_click(move |_, window, cx| view(None, window, cx)))
        });
    let root = changes.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    let mut rows: Vec<AnyElement> = vec![];
    for (g, (dir, files)) in groups.iter().enumerate() {
        let open = !folded.contains(dir);
        let toggle = actions.toggle_dir.clone();
        let key = dir.clone();
        rows.push(
            h_flex()
                .id(SharedString::from(format!("changes-dir-{ix}-{g}")))
                .test_support()
                .gap(px(6.))
                .px(px(8.))
                .h(px(26.))
                .rounded(px(6.))
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.04)))
                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().text_color(muted.opacity(0.8)))
                .child(Icon::new(if open { IconName::FolderOpen } else { IconName::Folder }).xsmall().text_color(muted))
                .child(div().min_w_0().truncate().child(if dir.is_empty() { root.clone() } else { trek_core::paths::tildify(std::path::Path::new(dir)) }))
                .when(!open, |el| el.child(div().flex_none().text_color(muted.opacity(0.7)).child(format!("{}", files.len()))))
                .on_click(move |_, _, cx| toggle(&key, cx))
                .into_any_element(),
        );
        if open {
            for f in files {
                let i = changes.files.iter().position(|c| std::ptr::eq(c, *f)).unwrap_or(0);
                rows.push(file_row(ix, i, f, changes.counted, &actions, cx));
            }
        }
    }
    let note = (changes.counted == Counted::EditTools).then(|| {
        div()
            .px(px(12.))
            .py(px(6.))
            .border_t_1()
            .border_color(line)
            .text_xs()
            .line_height(relative(1.45))
            .text_color(muted)
            .child("Counted from the agent's edits (no git checkpoints for this turn), so changes made by commands aren't in it.")
    });
    v_flex()
        .id(("turn-changes", ix))
        .test_support()
        .mt(px(4.))
        .mb(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme.foreground.opacity(0.1))
        .bg(theme.foreground.opacity(0.025))
        .overflow_hidden()
        .child(head)
        .child(v_flex().py(px(4.)).px(px(4.)).border_t_1().border_color(line).children(rows))
        .children(note)
        .into_any_element()
}

/// One file under its folder: its type's badge, its name (struck through when deleted), a tag
/// for a new, deleted or renamed one, and its lines on the right.
fn file_row(ix: usize, i: usize, f: &FileChange, counted: Counted, actions: &Actions, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let (green, red) = (crate::palette::emerald(cx), crate::palette::red(cx));
    let name = f.path.rsplit_once('/').map_or(f.path.as_str(), |(_, n)| n).to_string();
    let deleted = f.status == FileStatus::Deleted;
    let tag = |text: &str, color: Hsla| div().flex_none().px(px(5.)).rounded(px(4.)).text_size(px(10.5)).text_color(color).bg(color.opacity(0.12)).child(text.to_string());
    let tag = match &f.status {
        FileStatus::Added if counted == Counted::Checkpoints => Some(tag("new", green)),
        FileStatus::Deleted => Some(tag("deleted", red)),
        FileStatus::Renamed { from } => Some(tag(&format!("from {}", from.rsplit_once('/').map_or(from.as_str(), |(_, n)| n)), muted)),
        _ => None,
    };
    let mono = theme.mono_font_family.clone();
    let lines = if f.binary {
        div().text_color(muted).child("binary").into_any_element()
    } else if f.lines_known {
        h_flex()
            .gap(px(4.))
            .child(div().text_color(if f.added > 0 { green } else { muted.opacity(0.6) }).child(format!("+{}", f.added)))
            .child(div().text_color(muted.opacity(0.6)).child("/"))
            .child(div().text_color(if f.removed > 0 { red } else { muted.opacity(0.6) }).child(format!("−{}", f.removed)))
            .into_any_element()
    } else {
        div().into_any_element()
    };
    let tip: SharedString = match (&f.status, actions.view_diff.is_some()) {
        (FileStatus::Renamed { from }, true) => format!("{} · renamed from {from}\nShow the diff", f.path).into(),
        (_, true) => format!("{}\nShow the diff", f.path).into(),
        (_, false) => format!("{}\nShow in Finder", f.path).into(),
    };
    let (view, reveal, file) = (actions.view_diff.clone(), actions.reveal.clone(), f.clone());
    h_flex()
        .id(SharedString::from(format!("changed-{ix}-{i}")))
        .test_support()
        .gap(px(8.))
        .pl(px(30.))
        .pr(px(8.))
        .h(px(26.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .cursor_pointer()
        .hover(|s| s.bg(theme.foreground.opacity(0.045)))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, window, cx| match &view {
            Some(view) => view(Some(file.path.clone()), window, cx),
            None => reveal(&file, window, cx),
        })
        .child(crate::file_icon::badge(&f.path, px(14.), cx))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(if deleted { muted } else { theme.foreground.opacity(0.92) })
                .when(deleted, |el| el.line_through())
                .child(name),
        )
        .children(tag)
        .child(div().flex_1())
        .child(div().flex_none().font_family(mono).text_xs().child(lines))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::folders;
    use trek_core::changes::{Counted, FileChange, FileStatus, TurnChanges};

    fn file(path: &str) -> FileChange {
        FileChange { path: path.into(), status: FileStatus::Modified, added: 1, removed: 0, binary: false, lines_known: true }
    }

    #[test]
    fn files_group_by_folder_the_top_folders_own_first() {
        let changes = TurnChanges { files: vec![file("README.md"), file("a/x.rs"), file("a/y.rs"), file("a/b/z.rs"), file("Z.md")], root: "/p/trek".into(), counted: Counted::Checkpoints };
        let got: Vec<(String, Vec<&str>)> = folders(&changes).into_iter().map(|(d, fs)| (d, fs.iter().map(|f| f.path.as_str()).collect())).collect();
        assert_eq!(
            got,
            [("".to_string(), vec!["README.md", "Z.md"]), ("a".to_string(), vec!["a/x.rs", "a/y.rs"]), ("a/b".to_string(), vec!["a/b/z.rs"])]
        );
    }
}
