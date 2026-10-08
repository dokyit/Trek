//! The card under a finished turn's answer when the turn changed files
//! (`Workspace::load_turn_changes`): "Edited N files" with the lines it added and removed in
//! all, then a flat row per file — its folder muted, its name brighter, its own counts on the
//! right — Undo (the turn-end confirmation) and Review (the turn's diff in the Git tool) on the
//! header, and a "Show N more" row past the first few. Drawn with the theme's foreground at low
//! opacities, like the rest of the transcript, so it sits right on Night, Paper and glass alike.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;
use trek_core::changes::{Counted, FileChange, FileStatus, TurnChanges};

/// What the card's controls do.
pub struct Actions {
    /// Show the turn's diff in the Git tool, a file's open; `None` when there's none to show
    /// (counted without git, or no Git tool in this window). Review shows it all.
    pub view_diff: Option<Rc<dyn Fn(Option<String>, &mut Window, &mut App)>>,
    /// A file clicked when there's no diff to show: show it in Finder.
    pub reveal: Rc<dyn Fn(&FileChange, &mut Window, &mut App)>,
    /// Undo the turn; asks the same confirmation the turn-end controls do. `None` when the turn
    /// can't be taken back (one the agent started itself, or a turn still running).
    pub undo: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// The list passed its cap: show the rest (`true`) or the first `SHOWN` again.
    pub show_more: Rc<dyn Fn(bool, &mut App)>,
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

/// Files listed before a "Show N more" row offers the rest.
const SHOWN: usize = 8;

/// The card for the turn ending at transcript item `ix`; `shown_all`: more than `SHOWN` files,
/// opened out.
pub fn card(ix: usize, changes: &TurnChanges, shown_all: bool, actions: Actions, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let line = theme.foreground.opacity(0.07);
    let (green, red) = (crate::palette::emerald(cx), crate::palette::red(cx));
    let (added, removed) = changes.totals();
    let n = changes.files.len();
    let head = h_flex()
        .id(("changes-head", ix))
        .test_support()
        .gap(px(8.))
        .pl(px(14.))
        .pr(px(8.))
        .py(px(8.))
        .text_size(px(12.5))
        .child(
            div()
                .font_semibold()
                .text_color(theme.foreground.opacity(0.92))
                .child(format!("Edited {n} {}", if n == 1 { "file" } else { "files" })),
        )
        .when(added > 0 || removed > 0, |el| {
            el.child(
                h_flex()
                    .gap(px(6.))
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .child(div().text_color(green).child(format!("+{added}")))
                    .child(div().text_color(red).child(format!("−{removed}"))),
            )
        })
        .child(div().flex_1())
        .when_some(actions.undo.clone(), |el, undo| {
            el.child(
                Button::new(("changes-undo", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(crate::assets::Lucide::Undo2).text_color(muted))
                    .label("Undo")
                    .on_click(move |_, window, cx| undo(window, cx)),
            )
        })
        .when_some(actions.view_diff.clone(), |el, view| {
            el.child(Button::new(("changes-review", ix)).small().outline().label("Review").on_click(move |_, window, cx| view(None, window, cx)))
        });
    let files = if shown_all { &changes.files[..] } else { &changes.files[..n.min(SHOWN)] };
    let mut rows: Vec<AnyElement> = files.iter().enumerate().map(|(i, f)| file_row(ix, i, f, &actions, cx)).collect();
    if n > SHOWN {
        let show_more = actions.show_more.clone();
        rows.push(
            div()
                .id(("changes-more", ix))
                .test_support()
                .px(px(8.))
                .h(px(28.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.04)).text_color(theme.foreground))
                .child(if shown_all { "Show less".to_string() } else { format!("Show {} more", n - SHOWN) })
                .on_click(move |_, _, cx| show_more(!shown_all, cx))
                .into_any_element(),
        );
    }
    let note = (changes.counted == Counted::EditTools).then(|| {
        div()
            .px(px(14.))
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
        .border_color(theme.foreground.opacity(0.08))
        .bg(theme.secondary)
        .overflow_hidden()
        .child(head)
        .child(v_flex().py(px(4.)).px(px(6.)).border_t_1().border_color(line).children(rows))
        .children(note)
        .into_any_element()
}

/// One changed file: its status letter as the Git tool marks it, its folder muted, its name
/// struck through when deleted, its lines on the right.
fn file_row(ix: usize, i: usize, f: &FileChange, actions: &Actions, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let (green, red) = (crate::palette::emerald(cx), crate::palette::red(cx));
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), f.path.clone()),
    };
    // The same letters and colours panels/git.rs gives a worktree row ("U" there is a git
    // status the turn's counting never yields).
    let (letter, tint) = match &f.status {
        FileStatus::Added => ("A", green),
        FileStatus::Modified => ("M", crate::palette::amber(cx)),
        FileStatus::Deleted => ("D", red),
        FileStatus::Renamed { .. } => ("R", crate::palette::amber(cx)),
    };
    let deleted = f.status == FileStatus::Deleted;
    let lines = if f.binary {
        div().text_color(muted).child("binary").into_any_element()
    } else if f.lines_known {
        h_flex()
            .gap(px(6.))
            .child(div().text_color(if f.added > 0 { green } else { muted.opacity(0.6) }).child(format!("+{}", f.added)))
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
        .gap(px(4.))
        .px(px(8.))
        .h(px(28.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .cursor_pointer()
        .hover(|s| s.bg(theme.foreground.opacity(0.045)))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, window, cx| match &view {
            Some(view) => view(Some(file.path.clone()), window, cx),
            None => reveal(&file, window, cx),
        })
        .child(div().w(px(12.)).flex_none().text_xs().font_semibold().text_color(tint).child(letter))
        // The path takes what the counts don't: the folder ellipsizes first, then the name.
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .when(!dir.is_empty(), |el| {
                    el.child(div().min_w_0().truncate().text_color(muted).child(dir))
                })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if deleted { muted } else { theme.foreground.opacity(0.92) })
                        .font_medium()
                        .when(deleted, |el| el.line_through())
                        .child(name),
                ),
        )
        .child(div().flex_none().font_family(theme.mono_font_family.clone()).text_xs().child(lines))
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
