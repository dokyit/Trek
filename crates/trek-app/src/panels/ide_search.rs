//! The IDE's project search (⌘⇧F): a query over every file under `ide_root` that git doesn't
//! ignore (every `.gitignore`, the repository's excludes and the user's), results as file:line
//! rows that open the editor right there. The folder is listed and searched off the main
//! thread; the list is kept until files change, and a search runs again only when the query,
//! the folder or the files do (not on every redraw of the workspace).

use crate::workspace::Workspace;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_HITS: usize = 200;

/// One match: the line a click jumps to.
struct Hit {
    path: PathBuf,
    rel: String,
    line: u32,
    text: String,
}

/// Files listed at most.
const MAX_FILES: usize = 30_000;

/// The files under `root` git doesn't ignore, relative to it (folders left out).
pub(crate) fn list_files(root: &std::path::Path) -> Vec<String> {
    let walk = ignore::WalkBuilder::new(root).hidden(false).parents(true).ignore(false).git_ignore(true).git_exclude(true).git_global(true).follow_links(false).filter_entry(|e| e.file_name() != ".git").build();
    walk.flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(crate::mentions::rel_string))
        .filter(|r| !r.is_empty() && !r.ends_with(".DS_Store"))
        .take(MAX_FILES)
        .collect()
}

pub struct IdeSearch {
    workspace: Entity<Workspace>,
    input: Entity<InputState>,
    hits: Vec<Hit>,
    /// The folder's files, as last listed (`None`: to list again before the next search).
    files: Option<Arc<Vec<String>>>,
    file_root: Option<PathBuf>,
    /// What the folder was last searched at: (root, files changed count, query).
    searched: Option<(Option<PathBuf>, u64, String)>,
    query: String,
    searching: bool,
    epoch: Arc<AtomicU64>,
    _wait: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl IdeSearch {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search in folder"));
        let mut subs = vec![];
        subs.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search(cx);
            }
        }));
        // Only when the folder or its files changed: not for every token a thread streams.
        subs.push(cx.observe(&workspace, |this, _, cx| this.search(cx)));
        Self {
            workspace,
            input,
            hits: vec![],
            files: None,
            file_root: None,
            searched: None,
            query: String::new(),
            searching: false,
            epoch: Arc::new(AtomicU64::new(0)),
            _wait: None,
            _subs: subs,
        }
    }

    /// ⌘⇧F lands here.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |i, cx| i.focus(window, cx));
    }

    /// Search the folder off the main thread (listing it first when it isn't yet); stale
    /// generations drop on the floor. Nothing to do when neither the query nor the folder's
    /// files changed since the last search.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string().trim().to_lowercase();
        let (root, epoch) = {
            let ws = self.workspace.read(cx);
            (ws.ide_root.clone(), ws.files_epoch + ws.turns_finished + ws.agent_edits)
        };
        let key = (root.clone(), epoch, query.clone());
        if self.searched.as_ref() == Some(&key) {
            return;
        }
        // Files come and go with turns, checkouts and the folder: list them again then.
        if self.searched.as_ref().is_none_or(|(r, e, _)| *r != root || *e != epoch) {
            self.files = None;
        }
        self.searched = Some(key);
        self.query = query.clone();
        self.file_root = root.clone();
        self.hits.clear();
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (Some(root), true) = (root, query.len() >= 2) else {
            self.searching = false;
            cx.notify();
            return;
        };
        self.searching = true;
        cx.notify();
        let listed = self.files.clone();
        let watch = self.epoch.clone();
        let (tx, rx) = async_channel::bounded::<(Arc<Vec<String>>, Vec<Hit>)>(1);
        std::thread::spawn(move || {
            let files = listed.unwrap_or_else(|| Arc::new(list_files(&root)));
            let mut hits = Vec::new();
            'outer: for rel in files.iter() {
                if watch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let path = root.join(rel);
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                if meta.len() > 1_000_000 {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                if bytes.iter().take(8192).any(|b| *b == 0) {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else { continue };
                for (i, line) in text.lines().enumerate() {
                    if line.to_lowercase().contains(&query) {
                        hits.push(Hit { path: path.clone(), rel: rel.clone(), line: i as u32 + 1, text: line.trim().chars().take(160).collect() });
                        if hits.len() >= MAX_HITS {
                            break 'outer;
                        }
                    }
                }
            }
            let _ = tx.try_send((files, hits));
        });
        let watch = self.epoch.clone();
        self._wait = Some(cx.spawn(async move |this, cx| {
            if let Ok((files, hits)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if watch.load(Ordering::SeqCst) == epoch {
                        this.files = Some(files);
                        this.hits = hits;
                        this.searching = false;
                        cx.notify();
                    }
                });
            }
        }));
    }

    #[cfg(test)]
    pub(crate) fn hits(&self) -> Vec<String> {
        self.hits.iter().map(|h| format!("{}:{}", h.rel, h.line)).collect()
    }

    #[cfg(test)]
    pub(crate) fn searching(&self) -> bool {
        self.searching
    }
}

impl Render for IdeSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = &self.hits;
        let query = self.query.clone();
        let searching = self.searching;
        v_flex()
            .size_full()
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.input).h(px(28.))),
            )
            .child(
                v_flex()
                    .id("ide-search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .when(query.len() < 2, |el| {
                        el.child(
                            div().p_4().text_sm().text_color(theme.muted_foreground)
                                .child("Type to search every file in the folder."),
                        )
                    })
                    .when(!searching && query.len() >= 2 && rows.is_empty(), |el| {
                        el.child(
                            div().p_4().text_sm().text_color(theme.muted_foreground)
                                .child("Nothing matches."),
                        )
                    })
                    .children(rows.iter().enumerate().map(|(ix, hit)| {
                        let theme = theme.clone();
                        let name = hit.rel.rsplit('/').next().unwrap_or(&hit.rel).to_string();
                        let path = hit.path.clone();
                        let line = hit.line;
                        let text = hit.text.clone();
                        h_flex()
                            .id(("ide-search-hit", ix))
                            .test_support()
                            .mx_1()
                            .px_2()
                            .py(px(5.))
                            .gap_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .child(crate::file_icon::badge(&name, px(14.), cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(div().text_xs().truncate().child(name))
                                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!(":{line}"))),
                                    )
                                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(text)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace.update(cx, |ws, cx| ws.open_editor(path.clone(), Some(line), cx));
                            }))
                    })),
            )
    }
}
