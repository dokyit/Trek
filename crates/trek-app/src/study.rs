//! Study: learning anything with an agent as a one-to-one tutor. A launcher with one job, start
//! or carry on studying, laid out as one bounded column:
//!
//! - the title and the study folder;
//! - the composer, the page's hero (the main window's: notes, PDFs, photos, folders, links, `@`
//!   files and the mic all reach the tutor; sending on this route calls
//!   `Workspace::start_study_with`), with Learn | Practice in its own header row;
//! - what can come with the session: Files or folders, A note, Talk, then the study folder's
//!   sources as file cards to tick;
//! - Continue: the latest session, one card with a button;
//! - the learner's concept maps (`trek_core::study::topics`) as a grid, and earlier sessions.
//!
//! With little in it the column sits a little above the middle of the page, so it reads as a
//! composed launcher rather than a form left at the top of an empty page; once it's longer than
//! the page it simply scrolls.
//!
//! The folder is read off the main thread when the screen opens and whenever threads change while
//! it's open (a session's turn may have added to a map); frames only draw what was read.
//!
//! Motion is kept to state the learner caused: a tick fades in on a file or map chosen for the
//! session. Nothing moves on hover or when the page opens, and nothing when the system asks for
//! less motion.

use crate::composer::Composer;
use crate::ui;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::time::Duration;
use trek_core::study::{self, Intro, Kind, Session, Source, Topic};

use crate::assets::Lucide as L;

/// The column's width: the composer and everything under it (plus the composer's own 24 px
/// padding each side).
const COLUMN: f32 = 760.;
/// Earlier sessions listed under Continue before the list stops.
const SESSION_ROWS: usize = 6;
/// File cards shown before "N more".
const FILE_CARDS: usize = 6;
/// Notes offered in the Note menu, the most recently changed first.
const NOTE_ROWS: usize = 12;
/// Characters of a file's name a card shows; longer names lose their middle, so the end (often
/// the part that tells files apart: a date, a number, the extension) shows.
const NAME_CHARS: usize = 34;
/// A chosen file's tick fades in: feedback, so short.
const TICK: Duration = Duration::from_millis(140);

/// A Trek note the learner can give a session: its file and its title.
#[derive(Debug, Clone, PartialEq)]
struct NoteRef {
    path: PathBuf,
    title: String,
}

pub struct StudyView {
    workspace: Entity<Workspace>,
    composer: Entity<Composer>,
    topics: Vec<Topic>,
    sessions: Vec<Session>,
    sources: Vec<Source>,
    notes: Vec<NoteRef>,
    /// Every file card shows, not just the first `FILE_CARDS`.
    files_open: bool,
    /// The folder has been read at least once.
    loaded: bool,
    /// On screen (the main window's route).
    open: bool,
    /// `Workspace::threads_gen` when the folder was last read.
    read_at: Option<u64>,
    /// The composer notified since it was last laid out (see `Composer::element`).
    composer_changed: bool,
    _read: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// A session whose thread is still there, as the page shows it.
struct Shown {
    thread: String,
    kind: Kind,
    /// The thread's title without its kind ("Learn · Transformers" → "Transformers").
    title: String,
    updated_at: i64,
    /// The map it built, when one matches its topic.
    map: Option<usize>,
}

impl StudyView {
    pub fn new(
        workspace: Entity<Workspace>,
        composer: Entity<Composer>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.observe(&composer, |this, _, cx| {
                this.composer_changed = true;
                cx.notify();
            }),
        ];
        let mut this = Self {
            workspace,
            composer,
            topics: vec![],
            sessions: vec![],
            sources: vec![],
            notes: vec![],
            files_open: false,
            loaded: false,
            open: false,
            read_at: None,
            composer_changed: true,
            _read: None,
            _subscriptions,
        };
        this.sync(cx);
        this
    }

    /// Follow the route: read the folder on opening, and again when threads change while open.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let open = ws.route == Route::Study;
        let generation = ws.threads_gen;
        let opened = open && !self.open;
        self.open = open;
        if opened || (open && self.read_at != Some(generation)) {
            self.read_at = Some(generation);
            self.refresh(cx);
        }
    }

    /// Read the maps, sessions, sources and notes in the background.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_spawn(async move {
            let dir = study::dir();
            let notes: Vec<NoteRef> = trek_core::notes::list_in(&trek_core::notes::notes_dir())
                .into_iter()
                .filter(|n| !n.body.trim().is_empty())
                .take(NOTE_ROWS)
                .map(|n| NoteRef {
                    title: n.title(),
                    path: n.path,
                })
                .collect();
            (
                study::topics(&dir),
                study::sessions(&dir),
                study::sources(&dir),
                notes,
            )
        });
        self._read = Some(cx.spawn(async move |this, cx| {
            let (topics, sessions, sources, notes) = read.await;
            let _ = this.update(cx, |this, cx| {
                let same = this.topics == topics
                    && this.sessions == sessions
                    && this.sources == sources
                    && this.notes == notes;
                if !same || !this.loaded {
                    this.topics = topics;
                    this.sessions = sessions;
                    this.sources = sources;
                    this.notes = notes;
                    this.loaded = true;
                    cx.notify();
                }
            });
        }));
    }

    /// A new session carrying on with `t`'s map: more of it, or practice on it.
    fn start_on(&mut self, kind: Kind, t: &Topic, cx: &mut Context<Self>) {
        let (title, path) = (t.title.clone(), t.path.clone());
        self.workspace
            .update(cx, |ws, cx| ws.start_study(kind, title, Some(path), cx));
    }

    /// Tick or untick `path` for the next session, and give the keys back to the composer.
    fn toggle(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |ws, cx| ws.toggle_study_source(path, cx));
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// Past sessions whose threads are still there, the newest first, each with the map it
    /// built if one matches.
    fn shown_sessions(&self, cx: &App) -> Vec<Shown> {
        let ws = self.workspace.read(cx);
        self.sessions
            .iter()
            .filter_map(|s| {
                let t = ws.thread(&s.thread)?;
                let title = ["Learn · ", "Practice · "]
                    .iter()
                    .find_map(|p| t.title.strip_prefix(p))
                    .unwrap_or(&t.title)
                    .to_string();
                let slug = study::topic_slug(&s.topic);
                let map = self.topics.iter().position(|m| {
                    m.title.eq_ignore_ascii_case(s.topic.trim())
                        || m.path
                            .file_stem()
                            .is_some_and(|f| f.to_string_lossy() == slug)
                });
                Some(Shown {
                    thread: s.thread.clone(),
                    kind: s.kind,
                    title,
                    updated_at: t.updated_at,
                    map,
                })
            })
            .collect()
    }

    /// The page's title, and the study folder.
    fn header(&self) -> AnyElement {
        h_flex()
            .w_full()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(22.))
                    .font_semibold()
                    .line_height(px(32.))
                    .child("Study"),
            )
            .child(div().flex_1())
            .child(
                Button::new("study-folder")
                    .ghost()
                    .small()
                    .icon(Icon::new(L::FolderOpen))
                    .label("Study folder")
                    .tooltip("Your maps, notes about you, and sources to learn from")
                    .on_click(|_, _, cx| {
                        let dir = study::dir();
                        let _ = study::ensure(&dir);
                        cx.open_with_system(&dir);
                    }),
            )
            .into_any_element()
    }

    /// What can come with the session: files or folders (the + menu's picker), a Trek note and
    /// the voice; under them the study folder's files and the notes chosen, as cards to tick.
    fn context(&self, picked: &[PathBuf], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let composer = self.composer.clone();
        let files = action_chip("study-add-files", L::Paperclip, "Files or folders", cx)
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Notes, PDFs, photos, problem sheets or a whole folder. Links can go in the text.",
                )
                .build(window, cx)
            })
            .on_click(move |_, window, cx| composer.update(cx, |c, cx| c.attach(window, cx)));
        let note = (!self.notes.is_empty()).then(|| {
            let notes = self.notes.clone();
            let me = cx.entity();
            let muted = theme.muted_foreground;
            // A menu needs a Button under it; drawn as the chips beside it are (a child, not
            // `.icon()` and `.label()`: the kit sizes those to the button).
            Button::new("study-add-note")
                .ghost()
                .h(px(32.))
                .px(px(12.))
                .rounded(px(16.))
                .border_1()
                .border_color(theme.foreground.opacity(0.08))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(muted)
                        .child(Icon::new(L::NotebookPen).size(px(14.)).text_color(muted))
                        .child("A note"),
                )
                .tooltip("Give the tutor one of your Trek notes")
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                    let picked = me.read(cx).workspace.read(cx).study.picked.clone();
                    notes.iter().fold(menu.min_w(px(240.)), |menu, n| {
                        let (me, path) = (me.clone(), n.path.clone());
                        menu.item(
                            PopupMenuItem::new(n.title.clone())
                                .icon(L::StickyNote)
                                .checked(picked.contains(&n.path))
                                .on_click(move |_, window, cx| {
                                    me.update(cx, |this, cx| this.toggle(path.clone(), window, cx))
                                }),
                        )
                    })
                })
                .into_any_element()
        });
        let talk = crate::dictate::available().then(|| {
            let composer = self.composer.clone();
            action_chip("study-talk", L::Mic, "Talk", cx)
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        "Say it instead: your words go into the box above",
                    )
                    .build(window, cx)
                })
                .on_click(move |_, window, cx| {
                    composer.update(cx, |c, cx| {
                        c.focus(window, cx);
                        c.dictate(window, cx)
                    })
                })
        });

        // The cards: notes chosen first (they're only here once chosen), then the folder's files.
        let mut cards: Vec<AnyElement> = vec![];
        for (i, n) in self.notes.iter().enumerate() {
            if !picked.contains(&n.path) {
                continue;
            }
            let path = n.path.clone();
            cards.push(
                file_card(
                    SharedString::from(format!("study-note-{i}")),
                    L::StickyNote,
                    middle(&n.title, NAME_CHARS),
                    "Trek note".into(),
                    true,
                    cx,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle(path.clone(), window, cx)),
                )
                .into_any_element(),
            );
        }
        let notes_shown = cards.len();
        for (i, s) in self.sources.iter().enumerate() {
            let path = s.path.clone();
            let name = s
                .path
                .file_name()
                .map_or_else(|| s.label.clone(), |n| n.to_string_lossy().to_string());
            // What it is and how big; its folder under sources/ is the tooltip (topic folders'
            // names are as long as the topics).
            let mut meta = vec![size_label(s.size)];
            if let Some(ext) = s.path.extension() {
                meta.insert(0, ext.to_string_lossy().to_uppercase());
            }
            cards.push(
                file_card(
                    SharedString::from(format!("study-source-{i}")),
                    file_icon(&s.path),
                    middle(&name, NAME_CHARS),
                    meta.join(" · "),
                    picked.contains(&s.path),
                    cx,
                )
                .tooltip({
                    let label = s.label.clone();
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                    }
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle(path.clone(), window, cx)),
                )
                .into_any_element(),
            );
        }
        let total = cards.len();
        let limit = FILE_CARDS.max(notes_shown);
        let shown: Vec<AnyElement> = if self.files_open || total <= limit {
            cards
        } else {
            cards.into_iter().take(limit).collect()
        };
        let more = total - shown.len();
        let included = picked.len();
        let grid = (!shown.is_empty()).then(|| {
            v_flex()
                .id("study-library")
                .test_support()
                .w_full()
                .gap(px(8.))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child("From your study folder")
                        .when(included > 0, |el| {
                            el.child("·").child(
                                div()
                                    .text_color(theme.foreground)
                                    .child(format!("{included} included")),
                            )
                        })
                        .child(div().flex_1())
                        .when(more > 0 || (self.files_open && total > limit), |el| {
                            el.child(
                                div()
                                    .id("study-library-more")
                                    .test_support()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.foreground))
                                    .child(if more > 0 {
                                        format!("{more} more")
                                    } else {
                                        "Fewer".to_string()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.files_open = !this.files_open;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(2)
                        .gap(px(8.))
                        .children(shown),
                )
        });
        v_flex()
            .w_full()
            .gap(px(16.))
            .child(
                h_flex()
                    .id("study-context")
                    .test_support()
                    .w_full()
                    .gap(px(8.))
                    .flex_wrap()
                    .items_center()
                    .child(files)
                    .children(note)
                    .children(talk),
            )
            .children(grid)
            .into_any_element()
    }

    /// The latest session, to carry on with: what it was, when, what its map holds, and one
    /// button.
    fn continue_card(&self, s: &Shown, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut meta = vec![
            s.kind.label().to_string(),
            crate::time::relative(s.updated_at),
        ];
        if let Some(t) = s.map.and_then(|i| self.topics.get(i)) {
            meta.push(format!("{} on your map", plural(t.claims, "idea", "ideas")));
        }
        let open = {
            let id = s.thread.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                let id = id.clone();
                this.workspace
                    .update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
            }
        };
        let open2 = open.clone();
        h_flex()
            .id(SharedString::from(format!("study-session-{}", s.thread)))
            .test_support()
            .w_full()
            .p(px(16.))
            .gap(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_color(theme.foreground.opacity(0.08))
            .cursor_pointer()
            .hover(|st| st.bg(theme.list_hover))
            .child(
                div()
                    .flex_none()
                    .size(px(40.))
                    .rounded(px(10.))
                    .bg(theme.foreground.opacity(0.06))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(kind_icon(s.kind))
                            .size(px(18.))
                            .text_color(theme.foreground),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .font_medium()
                            .child(s.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(meta.join(" · ")),
                    ),
            )
            .child(
                Button::new("study-continue")
                    .primary()
                    .px(px(16.))
                    .label("Continue")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        open(this, cx)
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| open2(this, cx)))
            .into_any_element()
    }

    /// The concept maps, two to a row: open one, learn more of it, practise it, or tick it for
    /// the next session to build on.
    fn maps(&self, picked: &[PathBuf], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<AnyElement> = self
            .topics
            .iter()
            .enumerate()
            .map(|(i, t)| self.map_card(i, t, picked.contains(&t.path), cx))
            .collect();
        v_flex()
            .id("study-maps")
            .test_support()
            .w_full()
            .child(section_title("Your maps", Some(self.topics.len()), cx))
            .child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(2)
                    .gap(px(8.))
                    .children(cards),
            )
            .into_any_element()
    }

    fn map_card(&self, i: usize, t: &Topic, on: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let mut facts = vec![
            plural(t.claims, "idea", "ideas"),
            plural(t.links, "link", "links"),
        ];
        if t.practiced > 0 {
            facts.push(format!("{} practised", t.practiced));
        }
        let when = (t.updated_at > 0).then(|| crate::time::relative(t.updated_at));
        let path = t.path.clone();
        let tick_path = t.path.clone();
        let (go_on, practise) = (t.clone(), t.clone());
        v_flex()
            .id(SharedString::from(format!("study-map-{i}")))
            .test_support()
            .min_w_0()
            .p(px(12.))
            .gap(px(8.))
            .rounded(px(10.))
            .border_1()
            .border_color(if on {
                ember.opacity(0.55)
            } else {
                theme.foreground.opacity(0.08)
            })
            .when(on, |el| el.bg(ember.opacity(0.06)))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Icon::new(L::Map)
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_medium()
                            .child(t.title.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("study-use-map-{i}")))
                            .test_support()
                            .flex_none()
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Build the next session on this map",
                                )
                                .build(window, cx)
                            })
                            .child(tick_box(
                                SharedString::from(format!("study-use-map-{i}")),
                                on,
                                cx,
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.toggle(tick_path.clone(), window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(
                        facts
                            .into_iter()
                            .chain(when)
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(4.))
                    .ml(px(-6.))
                    .child(
                        Button::new(SharedString::from(format!("study-map-learn-{i}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(L::BookOpen))
                            .label("Learn more")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_on(Kind::Learn, &go_on, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("study-map-practice-{i}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(L::Repeat))
                            .label("Practice")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_on(Kind::Practice, &practise, cx)
                            })),
                    ),
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Open the map").build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                let path: PathBuf = path.clone();
                this.workspace
                    .update(cx, |ws, cx| ws.navigate(Route::Editor { path }, cx))
            }))
            .into_any_element()
    }

    /// Sessions before the latest, a row each between hairlines.
    fn earlier(&self, rest: &[Shown], cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let rows: Vec<AnyElement> = rest
            .iter()
            .take(SESSION_ROWS)
            .map(|s| {
                let id = s.thread.clone();
                h_flex()
                    .id(SharedString::from(format!("study-session-{}", s.thread)))
                    .test_support()
                    .h(px(40.))
                    .gap(px(12.))
                    .px(px(8.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|st| st.bg(theme.list_hover))
                    .child(
                        Icon::new(kind_icon(s.kind))
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(s.title.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · {}",
                                s.kind.label(),
                                crate::time::relative(s.updated_at)
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.workspace
                            .update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
                    }))
                    .into_any_element()
            })
            .collect();
        (!rows.is_empty()).then(|| {
            v_flex()
                .w_full()
                .child(section_title("Earlier sessions", Some(rest.len()), cx))
                .child(ui::group(rows, cx))
                .into_any_element()
        })
    }

    /// Before the first session: the one sentence that says what the page will hold.
    fn first_time(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("study-how")
            .test_support()
            .w_full()
            .px(px(16.))
            .py(px(12.))
            .gap(px(12.))
            .rounded(px(10.))
            .bg(theme.foreground.opacity(0.03))
            .text_size(px(13.))
            .text_color(theme.muted_foreground)
            .child(
                Icon::new(L::Map)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child("Each session leaves a map of what you learned; the next one builds on it.")
            .into_any_element()
    }
}

impl Render for StudyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::rendered("Study");
        let picked = self.workspace.read(cx).study.picked.clone();
        let mut sessions = self.shown_sessions(cx);
        let latest = (!sessions.is_empty()).then(|| sessions.remove(0));
        let first_time = self.loaded && self.topics.is_empty() && latest.is_none();
        let context = self.context(&picked, cx);
        let resume = latest.as_ref().map(|s| {
            v_flex()
                .w_full()
                .child(section_title("Continue", None, cx))
                .child(self.continue_card(s, cx))
                .into_any_element()
        });
        let maps = (!self.topics.is_empty()).then(|| self.maps(&picked, cx));
        let earlier = self.earlier(&sessions, cx);
        let how = first_time.then(|| self.first_time(cx));
        let composer = Composer::element(&self.composer, &mut self.composer_changed, cx);
        // Bias the column above the middle: more room under it than over it, so with little in
        // it the composer sits in the page's upper third.
        let tall = window.viewport_size().height;
        let below = (tall * 0.18).clamp(px(48.), px(240.));
        let inset = |el: AnyElement| div().w_full().px(px(24.)).child(el);
        div()
            .id("study")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .min_h_full()
                    .justify_center()
                    .px(px(16.))
                    .pt(px(40.))
                    .pb(below)
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(COLUMN + 48.))
                            .mx_auto()
                            .gap(px(40.))
                            .child(
                                v_flex()
                                    .id("study-start")
                                    .test_support()
                                    .w_full()
                                    .gap(px(16.))
                                    .child(inset(self.header()))
                                    .child(composer)
                                    .child(inset(context)),
                            )
                            .children(resume.map(inset))
                            .children(maps.map(inset))
                            .children(earlier.map(inset))
                            .children(how.map(inset)),
                    ),
            )
    }
}

fn kind_icon(kind: Kind) -> L {
    match kind {
        Kind::Learn => L::BookOpen,
        Kind::Practice => L::Repeat,
    }
}

/// The icon for a file by its kind: a picture, an archive, or a document.
fn file_icon(path: &Path) -> L {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" => L::Image,
        "zip" | "gz" | "tgz" | "tar" | "rar" | "7z" => L::Package,
        "md" | "txt" | "pdf" | "doc" | "docx" | "rtf" | "tex" => L::FileText,
        _ => L::File,
    }
}

/// `name` in at most `max` characters: the start and the end, with the middle let go.
fn middle(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_string();
    }
    let tail = (max - 1) * 2 / 5;
    let head = max - 1 - tail;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{}…{}", start.trim_end(), end.trim_start())
}

/// "820 B", "14 KB", "2.3 MB".
fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.;
    let b = bytes as f64;
    if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.0} KB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.1} GB", b / (K * K * K))
    }
}

/// A chip the page lays out, findable by its id in tests and the shots harness.
type Chip = gpui_kit::base::ObservedElement<Stateful<Div>>;

/// A quiet pill that adds something to the session: an icon and what it adds.
fn action_chip(id: &'static str, icon: L, label: &'static str, cx: &App) -> Chip {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .test_support()
        .h(px(32.))
        .px(px(12.))
        .gap(px(8.))
        .rounded_full()
        .border_1()
        .border_color(theme.foreground.opacity(0.08))
        .text_size(px(13.))
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .hover(|s| {
            s.bg(theme.foreground.opacity(0.04))
                .text_color(theme.foreground)
        })
        .active(|s| s.bg(theme.foreground.opacity(0.08)))
        .child(
            Icon::new(icon)
                .size(px(14.))
                .text_color(theme.muted_foreground),
        )
        .child(label)
}

/// The square that says a file or map goes with the next session; its tick fades in.
fn tick_box(id: SharedString, on: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let ember = crate::palette::ember(cx);
    div()
        .size(px(16.))
        .rounded(px(4.))
        .border_1()
        .border_color(if on {
            ember
        } else {
            theme.foreground.opacity(0.25)
        })
        .when(on, |el| el.bg(ember))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |el| {
            let tick = Icon::new(L::Check)
                .size(px(11.))
                .text_color(theme.background);
            el.child(if cx.reduce_motion() {
                tick.into_any_element()
            } else {
                let bg = theme.background;
                tick.with_animation(
                    SharedString::from(format!("{id}-tick")),
                    Animation::new(TICK).with_easing(ease_out_quint()),
                    move |i, t| i.text_color(bg.opacity(t)),
                )
                .into_any_element()
            })
        })
        .into_any_element()
}

/// A file the session can take: its kind's icon, its name (middle let go), what it is, and the
/// tick that includes it.
fn file_card(id: SharedString, icon: L, name: String, meta: String, on: bool, cx: &App) -> Chip {
    let theme = cx.theme();
    let ember = crate::palette::ember(cx);
    h_flex()
        .id(id.clone())
        .test_support()
        .min_w_0()
        .h(px(56.))
        .px(px(12.))
        .gap(px(12.))
        .rounded(px(10.))
        .border_1()
        .border_color(if on {
            ember.opacity(0.55)
        } else {
            theme.foreground.opacity(0.08)
        })
        .when(on, |el| el.bg(ember.opacity(0.06)))
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .child(
            Icon::new(icon)
                .size(px(16.))
                .text_color(theme.muted_foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().truncate().text_size(px(13.)).child(name))
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(meta),
                ),
        )
        .child(tick_box(id, on, cx))
}

/// A study session's first message in its thread: the kind, what was asked, and what came with
/// it, as chips (the instructions to the tutor stay out of sight).
pub fn intro_bubble(intro: &Intro, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.display().to_string())
    };
    let chip = |icon: L, label: String| {
        h_flex()
            .h(px(22.))
            .max_w(px(240.))
            .px(px(8.))
            .gap(px(5.))
            .rounded_full()
            .bg(theme.foreground.opacity(0.06))
            .text_size(px(12.))
            .text_color(muted)
            .child(Icon::new(icon).size(px(12.)).text_color(muted))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    };
    let map_dir = study::map_dir(&study::dir());
    let mut chips: Vec<AnyElement> = vec![];
    for f in &intro.with.files {
        let (icon, label) = if f.starts_with(&map_dir) {
            (L::Map, map_title(f))
        } else {
            (file_icon(f), middle(&name(f), 32))
        };
        chips.push(chip(icon, label));
    }
    for d in &intro.with.folders {
        chips.push(chip(L::FolderOpen, name(d)));
    }
    for l in &intro.with.links {
        let shown = l
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("www.")
            .trim_end_matches('/')
            .to_string();
        chips.push(chip(L::Link, shown));
    }
    let what = match (&intro.map, intro.kind) {
        (Some(m), Kind::Learn) => format!("Learn · carrying on with {}", map_title(m)),
        (Some(m), Kind::Practice) => format!("Practice · on {}", map_title(m)),
        (None, k) => k.label().to_string(),
    };
    v_flex()
        .id("study-intro")
        .test_support()
        .gap(px(6.))
        .child(
            h_flex()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(muted)
                .child(
                    Icon::new(kind_icon(intro.kind))
                        .size(px(13.))
                        .text_color(muted),
                )
                .child(what),
        )
        .when(!intro.topic.is_empty(), |el| {
            el.child(div().child(intro.topic.clone()))
        })
        .when(!chips.is_empty(), |el| {
            el.child(h_flex().pt(px(2.)).gap(px(6.)).flex_wrap().children(chips))
        })
        .into_any_element()
}

/// A map's name from its file: "gradient-descent.md" → "Gradient descent" (the heading would
/// need a read on every frame; the name is enough for a chip).
fn map_title(path: &Path) -> String {
    let words = path
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
        .unwrap_or_default();
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A section's title, with a count beside it.
fn section_title(title: &'static str, count: Option<usize>, cx: &App) -> AnyElement {
    h_flex()
        .pb(px(8.))
        .gap(px(6.))
        .text_size(px(13.))
        .child(div().font_medium().child(title))
        .when_some(count.filter(|n| *n > 1), |el, n| {
            el.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(n.to_string()),
            )
        })
        .into_any_element()
}

/// "1 idea", "12 ideas".
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
