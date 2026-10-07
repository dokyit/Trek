//! The IDE's project search (⌘⇧F): a query over every file under `ide_root`,
//! results as file:line rows that open the editor right there.

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

pub struct IdeSearch {
    workspace: Entity<Workspace>,
    input: Entity<InputState>,
    hits: Vec<Hit>,
    files: Vec<String>,
    file_root: Option<PathBuf>,
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
                this.searching = true;
                this.search(cx);
            }
        }));
        subs.push(cx.observe(&workspace, |this, _, cx| {
            this.searching = true;
            this.search(cx);
        }));
        Self {
            workspace,
            input,
            hits: vec![],
            files: vec![],
            file_root: None,
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

    /// Scan the folder off the main thread; stale generations drop on the floor.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string().trim().to_lowercase();
        self.query = query.clone();
        let root = self.workspace.read(cx).ide_root.clone();
        if root != self.file_root {
            self.file_root = root.clone();
            self.files = root.as_ref().map(|r| crate::mentions::index_files(r)).unwrap_or_default();
        }
        self.hits.clear();
        cx.notify();
        if query.len() < 2 || root.is_none() {
            self.searching = false;
            return;
        }
        let root = root.unwrap();
        let files: Vec<String> = self.files.iter().filter(|f| !f.ends_with('/')).cloned().collect();
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let watch = self.epoch.clone();
        let (tx, rx) = async_channel::bounded::<Vec<Hit>>(1);
        std::thread::spawn(move || {
            let mut hits = Vec::new();
            'outer: for rel in files {
                if watch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let path = root.join(&rel);
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
            let _ = tx.try_send(hits);
        });
        let watch = self.epoch.clone();
        self._wait = Some(cx.spawn(async move |this, cx| {
            if let Ok(hits) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if watch.load(Ordering::SeqCst) == epoch {
                        this.hits = hits;
                        this.searching = false;
                        cx.notify();
                    }
                });
            }
        }));
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
