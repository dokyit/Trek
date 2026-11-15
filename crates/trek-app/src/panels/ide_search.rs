//! The IDE's project search (⌘⇧F): a query over every file under `ide_root` that git doesn't
//! ignore (every `.gitignore`, the repository's excludes and the user's), matched as typed or
//! with Match Case, Whole Word or as a regular expression. Results are grouped by file (its
//! folder beside its name), a row per line that opens the editor right there; the first
//! `MAX_HITS` are shown, and a note says when there were more, or files were too big or not text
//! to search. The folder is listed and searched off the main thread; the list is kept until files
//! change, and a search runs again only when the query, its options, the folder or the files do
//! (not on every redraw of the workspace).

use crate::fluent;
use crate::scale::{radius, text, to_rems};
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// How a query matches: as typed, or with these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Options {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

/// What a query matches with `options`; an invalid regular expression says why.
pub fn matcher(query: &str, options: Options) -> Result<regex::Regex, String> {
    let pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if options.word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!options.case)
        .build()
        .map_err(|e| match e {
            regex::Error::Syntax(_) => "That isn't a regular expression Trek can read.".into(),
            _ => "That regular expression is too big to search with.".into(),
        })
}

/// What a search found: its hits (the first `MAX_HITS`), whether there were more, and how many
/// files were left out as too big or not text.
#[derive(Default)]
struct Found {
    hits: Vec<Hit>,
    capped: bool,
    skipped: usize,
}

/// The files under `root` git doesn't ignore, relative to it (folders left out).
pub fn list_files(root: &std::path::Path) -> Vec<String> {
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(true)
        .ignore(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    walk.flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()
                .map(crate::mentions::rel_string)
        })
        .filter(|r| !r.is_empty() && !r.ends_with(".DS_Store"))
        .take(MAX_FILES)
        .collect()
}

pub struct IdeSearch {
    workspace: Entity<Workspace>,
    input: Entity<InputState>,
    hits: Vec<Hit>,
    options: Options,
    /// More matched than are shown.
    capped: bool,
    /// Files left out as too big or not text.
    skipped: usize,
    /// Why the query can't be searched (an invalid regular expression).
    problem: Option<String>,
    /// The folder's files, as last listed (`None`: to list again before the next search).
    files: Option<Arc<Vec<String>>>,
    file_root: Option<PathBuf>,
    /// What the folder was last searched at: (root, files changed count, query, options).
    searched: Option<(Option<PathBuf>, u64, String, Options)>,
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
            options: Options::default(),
            capped: false,
            skipped: 0,
            problem: None,
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

    /// Turn one of the match options on or off, and search again.
    pub fn set_options(&mut self, options: Options, cx: &mut Context<Self>) {
        self.options = options;
        self.search(cx);
    }

    /// Search the folder off the main thread (listing it first when it isn't yet); stale
    /// generations drop on the floor. Nothing to do when neither the query, its options nor the
    /// folder's files changed since the last search.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_string();
        let options = self.options;
        let (root, epoch) = {
            let ws = self.workspace.read(cx);
            (
                ws.ide_root.clone(),
                ws.files_epoch + ws.turns_finished + ws.agent_edits,
            )
        };
        let key = (root.clone(), epoch, query.clone(), options);
        if self.searched.as_ref() == Some(&key) {
            return;
        }
        // Files come and go with turns, checkouts and the folder: list them again then.
        if self
            .searched
            .as_ref()
            .is_none_or(|(r, e, _, _)| *r != root || *e != epoch)
        {
            self.files = None;
        }
        self.searched = Some(key);
        self.query = query.clone();
        self.file_root = root.clone();
        self.hits.clear();
        self.capped = false;
        self.skipped = 0;
        self.problem = None;
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (Some(root), true) = (root, query.chars().count() >= 2) else {
            self.searching = false;
            cx.notify();
            return;
        };
        let matcher = match matcher(&query, options) {
            Ok(m) => m,
            Err(why) => {
                self.problem = Some(why);
                self.searching = false;
                cx.notify();
                return;
            }
        };
        self.searching = true;
        cx.notify();
        let listed = self.files.clone();
        let watch = self.epoch.clone();
        let (tx, rx) = async_channel::bounded::<(Arc<Vec<String>>, Found)>(1);
        std::thread::spawn(move || {
            let files = listed.unwrap_or_else(|| Arc::new(list_files(&root)));
            let mut found = Found::default();
            'outer: for rel in files.iter() {
                if watch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let path = root.join(rel);
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if meta.len() > 1_000_000 {
                    found.skipped += 1;
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                if bytes.iter().take(8192).any(|b| *b == 0) {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    found.skipped += 1;
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    if matcher.is_match(line) {
                        if found.hits.len() >= MAX_HITS {
                            found.capped = true;
                            break 'outer;
                        }
                        found.hits.push(Hit {
                            path: path.clone(),
                            rel: rel.clone(),
                            line: i as u32 + 1,
                            text: line.trim().chars().take(160).collect(),
                        });
                    }
                }
            }
            let _ = tx.try_send((files, found));
        });
        let watch = self.epoch.clone();
        self._wait = Some(cx.spawn(async move |this, cx| {
            if let Ok((files, found)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if watch.load(Ordering::SeqCst) == epoch {
                        this.files = Some(files);
                        this.hits = found.hits;
                        this.capped = found.capped;
                        this.skipped = found.skipped;
                        this.searching = false;
                        cx.notify();
                    }
                });
            }
        }));
    }

    /// The notes under the results: more matched than are shown, files left out.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = vec![];
        if self.capped {
            notes.push(format!(
                "Showing the first {MAX_HITS} matches. Narrow the search to see the rest."
            ));
        }
        match self.skipped {
            0 => {}
            1 => notes.push("1 file was left out: too big, or not text.".into()),
            n => notes.push(format!("{n} files were left out: too big, or not text.")),
        }
        notes
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn hits(&self) -> Vec<String> {
        self.hits
            .iter()
            .map(|h| format!("{}:{}", h.rel, h.line))
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn searching(&self) -> bool {
        self.searching
    }
}

impl IdeSearch {
    /// One of the match options' buttons beside the field.
    fn option_button(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        on: bool,
        flip: fn(&mut Options),
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .ghost()
            .xsmall()
            .label(label)
            .selected(on)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                let mut options = this.options;
                flip(&mut options);
                this.set_options(options, cx);
            }))
    }
}

impl Render for IdeSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.query.clone();
        let searching = self.searching;
        let typed = query.chars().count() >= 2;
        let options = self.options;
        let note = |text: String, cx: &App| {
            div()
                .p_4()
                .text_size(to_rems(text::LABEL))
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let message = if !typed {
            Some("Type to search every file in the folder.".to_string())
        } else if let Some(problem) = &self.problem {
            Some(problem.clone())
        } else if searching && self.hits.is_empty() {
            Some("Searching…".to_string())
        } else if !searching && self.hits.is_empty() {
            Some("Nothing matches.".to_string())
        } else {
            None
        };
        // The hits by file, in the order found: a header row for each file, then its lines.
        let mut rows: Vec<AnyElement> = vec![];
        let mut ix = 0;
        while ix < self.hits.len() {
            let rel = self.hits[ix].rel.clone();
            let count = self.hits[ix..].iter().take_while(|h| h.rel == rel).count();
            let (dir, name) = match rel.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (String::new(), rel.clone()),
            };
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("ide-search-file-{rel}")))
                    .mx_1()
                    .px_2()
                    .pt(px(6.))
                    .pb(px(2.))
                    .gap_2()
                    .child(crate::file_icon::badge(&name, px(14.), cx))
                    .child(
                        div()
                            .flex_none()
                            .text_size(to_rems(text::META))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(dir),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(count.to_string()),
                    )
                    .into_any_element(),
            );
            for (at, hit) in self.hits[ix..ix + count].iter().enumerate() {
                let theme = theme.clone();
                let path = hit.path.clone();
                let line = hit.line;
                rows.push(
                    h_flex()
                        .id(("ide-search-hit", ix + at))
                        .test_support()
                        .mx_1()
                        .pl(px(30.))
                        .pr_2()
                        .py(px(3.))
                        .gap_2()
                        .rounded(px(fluent::control(radius::CONTROL)))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(24.))
                                .text_size(to_rems(text::META))
                                .text_color(theme.muted_foreground)
                                .child(line.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(to_rems(text::META))
                                .child(hit.text.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace
                                .update(cx, |ws, cx| ws.open_editor(path.clone(), Some(line), cx));
                        }))
                        .into_any_element(),
                );
            }
            ix += count;
        }
        let notes = if searching { vec![] } else { self.notes() };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.input).h(px(28.))),
                    )
                    .child(self.option_button(
                        "ide-search-case",
                        "Aa",
                        "Match Case",
                        options.case,
                        |o| o.case = !o.case,
                        cx,
                    ))
                    .child(self.option_button(
                        "ide-search-word",
                        "ab",
                        "Match Whole Word",
                        options.word,
                        |o| o.word = !o.word,
                        cx,
                    ))
                    .child(self.option_button(
                        "ide-search-regex",
                        ".*",
                        "Use Regular Expression",
                        options.regex,
                        |o| o.regex = !o.regex,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .id("ide-search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .children(message.map(|m| note(m, cx)))
                    .children(rows)
                    .children(notes.into_iter().map(|n| note(n, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Options, matcher};

    #[test]
    fn a_query_matches_as_typed_or_with_its_options() {
        let plain = Options::default();
        let m = matcher("Foo", plain).unwrap();
        assert!(m.is_match("a foo here") && m.is_match("FOOD"));
        assert!(
            matcher("a.b", plain).unwrap().is_match("a.b")
                && !matcher("a.b", plain).unwrap().is_match("axb")
        );
        let case = Options {
            case: true,
            ..plain
        };
        assert!(!matcher("Foo", case).unwrap().is_match("foo"));
        let word = Options {
            word: true,
            ..plain
        };
        assert!(matcher("foo", word).unwrap().is_match("a foo()"));
        assert!(!matcher("foo", word).unwrap().is_match("food"));
        let regex = Options {
            regex: true,
            ..plain
        };
        assert!(matcher(r"fo+\(", regex).unwrap().is_match("call fooo(1)"));
        assert_eq!(
            matcher("(unclosed", regex).unwrap_err(),
            "That isn't a regular expression Trek can read."
        );
    }
}
