//! Study: learning anything with an agent as a one-to-one tutor. The screen's starter is the main
//! window's composer (so notes, PDFs, photos, folders, links, `@` files and the mic all reach the
//! tutor; sending on this route calls `Workspace::start_study_with`), with Learn | Practice above
//! it and what can come with the session under it: files and folders, a note, the voice, and what
//! the study folder already holds (sources and earlier maps) to tick. Under that: the learner's
//! concept maps (`trek_core::study::topics`), each to carry on with or practise, and their past
//! sessions. Until there are any, how a session of the chosen kind goes.
//!
//! The folder is read off the main thread when the screen opens and whenever threads change while
//! it's open (a session's turn may have added to a map); frames only draw what was read.
//!
//! Motion is kept to state changes the learner caused, seen a few times a visit: the kind's line
//! fades in when the kind changes, and a tick fades in on a chip that's chosen. Nothing moves on
//! hover or when the page opens, and nothing at all when the system asks for less motion.

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

/// Past sessions shown before the list stops.
const SESSION_ROWS: usize = 8;
/// Chips from the study folder shown before "N more".
const LIBRARY_CHIPS: usize = 8;
/// Notes offered in the Note menu, the most recently changed first.
const NOTE_ROWS: usize = 12;
/// How long the kind's line takes to fade in, and a chip's tick: feedback, so short.
const FADE: Duration = Duration::from_millis(180);
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
    /// Every chip from the study folder shows, not just the first `LIBRARY_CHIPS`.
    library_open: bool,
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
            library_open: false,
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
                    .line_height(px(30.))
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

    /// Learn | Practice, and in a line what the chosen one does. A pick hands the keys back to
    /// the composer, so the next thing typed goes in.
    fn choice(&self, kind: Kind, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.clone();
        let composer = self.composer.clone();
        let line = div()
            .min_w_0()
            .text_size(px(13.))
            .text_color(theme.muted_foreground)
            .child(kind_line(kind));
        h_flex()
            .w_full()
            .gap(px(12.))
            .flex_wrap()
            .child(ui::segmented(
                "study-kind",
                vec![(Kind::Learn, "Learn"), (Kind::Practice, "Practice")],
                kind,
                move |k, window, cx| {
                    ws.update(cx, |ws, cx| ws.set_study_kind(k, cx));
                    composer.update(cx, |c, cx| c.focus(window, cx));
                },
                cx,
            ))
            .child(fade_in(line, ("study-kind-line", kind as usize), FADE, cx))
            .into_any_element()
    }

    /// What can come with the session: files or folders (the + menu's picker), a Trek note, and
    /// the voice; then the notes chosen, each a chip to take back off.
    fn context(&self, picked: &[PathBuf], cx: &mut Context<Self>) -> AnyElement {
        let composer = self.composer.clone();
        let files = action_chip("study-add-files", L::Paperclip, "Files or folders", cx)
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Notes, PDFs, photos, problem sheets, or a whole folder. Links can go straight into the text.",
                )
                .build(window, cx)
            })
            .on_click(move |_, window, cx| composer.update(cx, |c, cx| c.attach(window, cx)));
        let note = (!self.notes.is_empty()).then(|| {
            let notes = self.notes.clone();
            let me = cx.entity();
            // A menu needs a Button under it; drawn as the chips beside it are (a child, not
            // `.icon()` and `.label()`: the kit sizes those to the button).
            let muted = cx.theme().muted_foreground;
            Button::new("study-add-note")
                .ghost()
                .h(px(28.))
                .px(px(10.))
                .rounded(px(14.))
                .border_1()
                .border_color(cx.theme().foreground.opacity(0.08))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(12.5))
                        .text_color(muted)
                        .child(Icon::new(L::NotebookPen).size(px(13.)).text_color(muted))
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
                        "Say what you want to learn: your words go into the box above",
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
        let chosen_notes: Vec<AnyElement> = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| picked.contains(&n.path))
            .map(|(i, n)| {
                let path = n.path.clone();
                pick_chip(
                    SharedString::from(format!("study-note-{i}")),
                    L::StickyNote,
                    n.title.clone(),
                    true,
                    cx,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle(path.clone(), window, cx)),
                )
                .into_any_element()
            })
            .collect();
        h_flex()
            .id("study-context")
            .test_support()
            .w_full()
            .gap(px(6.))
            .flex_wrap()
            .items_center()
            .child(files)
            .children(note)
            .children(talk)
            .children(chosen_notes)
            .into_any_element()
    }

    /// What the study folder already holds, to tick for this session: earlier maps to build on,
    /// then the sources, newest first.
    fn library(&self, picked: &[PathBuf], cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.topics.is_empty() && self.sources.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let mut chips: Vec<AnyElement> = vec![];
        for (i, t) in self.topics.iter().enumerate() {
            let path = t.path.clone();
            let on = picked.contains(&path);
            chips.push(
                pick_chip(
                    SharedString::from(format!("study-use-map-{i}")),
                    L::Map,
                    t.title.clone(),
                    on,
                    cx,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle(path.clone(), window, cx)),
                )
                .into_any_element(),
            );
        }
        for (i, s) in self.sources.iter().enumerate() {
            let path = s.path.clone();
            let on = picked.contains(&path);
            chips.push(
                pick_chip(
                    SharedString::from(format!("study-source-{i}")),
                    file_icon(&s.path),
                    s.label.clone(),
                    on,
                    cx,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle(path.clone(), window, cx)),
                )
                .into_any_element(),
            );
        }
        let total = chips.len();
        // A chip that's ticked stays in view even past the fold.
        let shown: Vec<AnyElement> = if self.library_open || total <= LIBRARY_CHIPS {
            chips
        } else {
            chips.into_iter().take(LIBRARY_CHIPS).collect()
        };
        let more = total - shown.len();
        let ticked = self
            .topics
            .iter()
            .map(|t| &t.path)
            .chain(self.sources.iter().map(|s| &s.path))
            .filter(|p| picked.contains(p))
            .count();
        Some(
            v_flex()
                .id("study-library")
                .test_support()
                .w_full()
                .gap(px(8.))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child("From your study folder")
                        .when(ticked > 0, |el| {
                            el.child("·").child(
                                div()
                                    .text_color(theme.foreground)
                                    .child(format!("{ticked} for this session")),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(6.))
                        .flex_wrap()
                        .items_center()
                        .children(shown)
                        .when(
                            more > 0 || (self.library_open && total > LIBRARY_CHIPS),
                            |el| {
                                el.child(
                                    div()
                                        .id("study-library-more")
                                        .test_support()
                                        .h(px(26.))
                                        .px(px(6.))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.))
                                        .text_color(theme.muted_foreground)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme.foreground))
                                        .child(if more > 0 {
                                            format!("{more} more")
                                        } else {
                                            "Fewer".to_string()
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.library_open = !this.library_open;
                                            cx.notify();
                                        })),
                                )
                            },
                        ),
                )
                .into_any_element(),
        )
    }

    /// The concept maps, a row each: open it, learn more of it, or practise it.
    fn maps(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let body = if self.topics.is_empty() {
            div()
                .py(px(6.))
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(theme.muted_foreground)
                .child(if self.loaded {
                    "Each topic you study gets a map: the ideas you've got and how they connect. \
                     Later sessions build on it."
                } else {
                    "Loading…"
                })
                .into_any_element()
        } else {
            let rows = self
                .topics
                .iter()
                .enumerate()
                .map(|(i, t)| self.map_row(i, t, cx))
                .collect();
            ui::group(rows, cx)
        };
        v_flex()
            .id("study-maps")
            .test_support()
            .child(section_title("Your maps", Some(self.topics.len()), cx))
            .child(body)
            .into_any_element()
    }

    fn map_row(&self, i: usize, t: &Topic, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut facts = vec![
            plural(t.claims, "idea", "ideas"),
            plural(t.links, "link", "links"),
        ];
        if t.practiced > 0 {
            facts.push(format!("{} practised", t.practiced));
        }
        if t.updated_at > 0 {
            facts.push(format!("studied {}", crate::time::relative(t.updated_at)));
        }
        let path = t.path.clone();
        let (go_on, practise) = (t.clone(), t.clone());
        h_flex()
            .id(SharedString::from(format!("study-map-{i}")))
            .test_support()
            .w_full()
            .min_h(px(52.))
            .px(px(8.))
            .gap(px(12.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Open the map").build(window, cx)
            })
            .child(
                Icon::new(L::Map)
                    .size(px(15.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .font_medium()
                            .child(t.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(facts.join(" · ")),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(2.))
                    .child(
                        Button::new(SharedString::from(format!("study-map-learn-{i}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(L::BookOpen))
                            .label("Learn more")
                            .tooltip("A new session that carries on with this map")
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
                            .tooltip("Problems on what this map holds, that adapt as you go")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_on(Kind::Practice, &practise, cx)
                            })),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let path: PathBuf = path.clone();
                this.workspace
                    .update(cx, |ws, cx| ws.navigate(Route::Editor { path }, cx))
            }))
            .into_any_element()
    }

    /// Past sessions whose threads are still there, the newest first.
    fn sessions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let rows: Vec<AnyElement> = {
            let ws = self.workspace.read(cx);
            self.sessions
                .iter()
                .filter_map(|s| {
                    ws.thread(&s.thread)
                        .map(|t| (s, t.title.clone(), t.updated_at))
                })
                .take(SESSION_ROWS)
                .map(|(s, title, at)| {
                    let id = s.thread.clone();
                    h_flex()
                        .id(SharedString::from(format!("study-session-{}", s.thread)))
                        .test_support()
                        .h(px(34.))
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
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(52.))
                                .text_right()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(crate::time::relative(at)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = id.clone();
                            this.workspace
                                .update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
                        }))
                        .into_any_element()
                })
                .collect()
        };
        (!rows.is_empty()).then(|| {
            v_flex()
                .child(section_title("Sessions", Some(rows.len()), cx))
                .child(ui::group(rows, cx))
                .into_any_element()
        })
    }

    /// Before the first session: how one of the chosen kind goes, in order.
    fn how(&self, kind: Kind, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let steps: &[(L, &str, &str)] = match kind {
            Kind::Learn => &[
                (
                    L::Target,
                    "Find your edge",
                    "a few quick questions show what you already know",
                ),
                (
                    L::Route,
                    "Plan the path",
                    "the fewest steps to your goal, for you to approve",
                ),
                (
                    L::Footprints,
                    "One step at a time",
                    "predict, learn, then check it landed",
                ),
                (
                    L::Map,
                    "Keep it on your map",
                    "what you've got, for the next session to build on",
                ),
            ],
            Kind::Practice => &[
                (
                    L::Repeat,
                    "One problem at a time",
                    "harder or easier as your working shows",
                ),
                (
                    L::MessageCircleQuestionMark,
                    "Nudges, not answers",
                    "ask for a hint whenever you're stuck",
                ),
                (
                    L::Map,
                    "Logged on your map",
                    "what's solid and what to practise next",
                ),
            ],
        };
        let rows: Vec<AnyElement> = steps
            .iter()
            .map(|(icon, title, line)| {
                h_flex()
                    .min_h(px(34.))
                    .py(px(6.))
                    .px(px(8.))
                    .gap(px(12.))
                    .text_size(px(13.))
                    .child(
                        Icon::new(*icon)
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().min_w_0().flex_1().child(
                        StyledText::new(format!("{title}  {line}")).with_highlights([(
                            title.len()..title.len() + 2 + line.len(),
                            HighlightStyle {
                                color: Some(theme.muted_foreground),
                                ..Default::default()
                            },
                        )]),
                    ))
                    .into_any_element()
            })
            .collect();
        let list = v_flex()
            .id("study-how")
            .test_support()
            .child(section_title(
                match kind {
                    Kind::Learn => "How a session goes",
                    Kind::Practice => "How practice goes",
                },
                None,
                cx,
            ))
            .child(ui::group(rows, cx));
        fade_in(list, ("study-how", kind as usize), FADE, cx)
    }
}

impl Render for StudyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::rendered("Study");
        let (kind, picked) = {
            let ws = self.workspace.read(cx);
            (ws.study.kind, ws.study.picked.clone())
        };
        let sessions = self.sessions(cx);
        let library = self.library(&picked, cx);
        let first_time = self.loaded && self.topics.is_empty() && sessions.is_none();
        // The composer card is as wide as a draft's (the transcript column); it pads itself by
        // 24 px each side, so the page's sections sit inside that padding.
        let column = self.workspace.read(cx).column();
        let composer = Composer::element(&self.composer, &mut self.composer_changed, cx);
        let inset = |el: AnyElement| div().w_full().px(px(24.)).child(el);
        div()
            .id("study")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(column + px(48.))
                    .mx_auto()
                    .px(px(16.))
                    .pt(px(28.))
                    .pb(px(48.))
                    .gap(px(32.))
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(20.))
                            .child(inset(self.header()))
                            .child(
                                v_flex()
                                    .id("study-start")
                                    .test_support()
                                    .w_full()
                                    .gap(px(12.))
                                    .child(inset(self.choice(kind, cx)))
                                    .child(composer)
                                    .child(inset(self.context(&picked, cx)))
                                    .children(library.map(|l| inset(l).pt(px(4.)))),
                            ),
                    )
                    .child(inset(self.maps(cx)))
                    .children(sessions.map(inset))
                    .when(first_time, |el| el.child(inset(self.how(kind, cx)))),
            )
    }
}

/// What the chosen kind does, in a line.
fn kind_line(kind: Kind) -> &'static str {
    match kind {
        Kind::Learn => {
            "Finds what you already know, plans the shortest path, teaches step by step."
        }
        Kind::Practice => "Problems that adapt to your working, with nudges rather than answers.",
    }
}

fn kind_icon(kind: Kind) -> L {
    match kind {
        Kind::Learn => L::BookOpen,
        Kind::Practice => L::Repeat,
    }
}

/// The icon for a file by its kind: a picture, or a document.
fn file_icon(path: &Path) -> L {
    let image = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp"));
    if image { L::Image } else { L::FileText }
}

/// `el` fading in over `over` when it first shows under `id` (a new id: a new fade); as it is
/// when the system asks for less motion.
fn fade_in(
    el: impl IntoElement + 'static,
    id: impl Into<ElementId>,
    over: Duration,
    cx: &App,
) -> AnyElement {
    if cx.reduce_motion() {
        return el.into_any_element();
    }
    div()
        .child(el)
        .with_animation(
            id,
            Animation::new(over).with_easing(ease_out_quint()),
            |d, t| d.opacity(t),
        )
        .into_any_element()
}

/// A chip the page lays out, findable by its id in tests and the shots harness.
type Chip = gpui_kit::base::ObservedElement<Stateful<Div>>;

/// A quiet pill that adds something to the session: an icon and what it adds.
fn action_chip(id: &'static str, icon: L, label: &'static str, cx: &App) -> Chip {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .test_support()
        .h(px(28.))
        .px(px(10.))
        .gap(px(6.))
        .rounded_full()
        .border_1()
        .border_color(theme.foreground.opacity(0.08))
        .text_size(px(12.5))
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .hover(|s| {
            s.bg(theme.foreground.opacity(0.04))
                .text_color(theme.foreground)
        })
        .active(|s| s.bg(theme.foreground.opacity(0.08)))
        .child(
            Icon::new(icon)
                .size(px(13.))
                .text_color(theme.muted_foreground),
        )
        .child(label)
}

/// A chip to tick for the session: its kind's icon, or a tick that fades in once it's chosen.
fn pick_chip(
    id: SharedString,
    icon: L,
    label: impl Into<SharedString>,
    on: bool,
    cx: &App,
) -> Chip {
    let theme = cx.theme();
    let ember = crate::palette::ember(cx);
    let mark = if on {
        let tick = Icon::new(L::Check).size(px(12.)).text_color(ember);
        if cx.reduce_motion() {
            tick.into_any_element()
        } else {
            tick.with_animation(
                SharedString::from(format!("{id}-tick")),
                Animation::new(TICK).with_easing(ease_out_quint()),
                move |i, t| i.text_color(ember.opacity(t)),
            )
            .into_any_element()
        }
    } else {
        Icon::new(icon)
            .size(px(12.))
            .text_color(theme.muted_foreground)
            .into_any_element()
    };
    h_flex()
        .id(id)
        .test_support()
        .h(px(26.))
        .max_w(px(260.))
        .px(px(9.))
        .gap(px(6.))
        .rounded_full()
        .border_1()
        .border_color(if on {
            ember.opacity(0.55)
        } else {
            theme.foreground.opacity(0.08)
        })
        .when(on, |el| el.bg(ember.opacity(0.08)))
        .text_size(px(12.))
        .text_color(if on {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .cursor_pointer()
        .hover(|s| s.border_color(theme.foreground.opacity(0.22)))
        .child(div().flex_none().child(mark))
        .child(div().min_w_0().truncate().child(label.into()))
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
            (file_icon(f), name(f))
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
        .when_some(count.filter(|n| *n > 0), |el, n| {
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
