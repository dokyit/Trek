//! Study: learning anything with an agent as a one-to-one tutor. The screen's starter is the main
//! window's composer (so notes, PDFs, photos, `@` files and the mic all reach the tutor; sending
//! on this route calls `Workspace::start_study_with`), with Learn | Practice beside it and the
//! files already in `sources/` to tick. Under it: the learner's concept maps
//! (`trek_core::study::topics`) as cards to carry on with or practise, their past sessions, and
//! the method in four tiles.
//!
//! The folder is read off the main thread when the screen opens and whenever threads change while
//! it's open (a session's turn may have added to a map); frames only draw what was read.

use crate::composer::Composer;
use crate::ui;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::study::{self, Kind, Session, Source, Topic};

/// Past sessions shown before the list stops.
const SESSION_ROWS: usize = 8;

pub struct StudyView {
    workspace: Entity<Workspace>,
    composer: Entity<Composer>,
    topics: Vec<Topic>,
    sessions: Vec<Session>,
    sources: Vec<Source>,
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

    /// Read the maps, sessions and sources in the background.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_spawn(async move {
            let dir = study::dir();
            (
                study::topics(&dir),
                study::sessions(&dir),
                study::sources(&dir),
            )
        });
        self._read = Some(cx.spawn(async move |this, cx| {
            let (topics, sessions, sources) = read.await;
            let _ = this.update(cx, |this, cx| {
                let same =
                    this.topics == topics && this.sessions == sessions && this.sources == sources;
                if !same || !this.loaded {
                    this.topics = topics;
                    this.sessions = sessions;
                    this.sources = sources;
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

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .gap(px(4.))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.))
                    .child(
                        Icon::new(crate::assets::Lucide::GraduationCap)
                            .size(px(22.))
                            .text_color(crate::palette::ember(cx)),
                    )
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
                            .icon(Icon::new(crate::assets::Lucide::FolderCog))
                            .label("Study folder")
                            .tooltip("Your maps, notes about you, and sources to learn from")
                            .on_click(|_, _, cx| {
                                let dir = study::dir();
                                let _ = study::ensure(&dir);
                                cx.open_with_system(&dir);
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child("A tutor that finds the edge of what you know and teaches from there."),
            )
            .into_any_element()
    }

    /// Learn | Practice, and what can come with the message. A pick hands the keys back to the
    /// composer, so the next thing typed goes in.
    fn choice(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.clone();
        let composer = self.composer.clone();
        let kind = ws.read(cx).study.kind;
        let hint = |icon: crate::assets::Lucide, text: &'static str| {
            h_flex()
                .gap(px(5.))
                .child(
                    Icon::new(icon)
                        .size(px(12.))
                        .text_color(theme.muted_foreground),
                )
                .child(text)
        };
        h_flex()
            .w_full()
            .gap(px(14.))
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
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(px(14.))
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(hint(
                        crate::assets::Lucide::Paperclip,
                        "Drop in notes, PDFs or photos",
                    ))
                    .when(crate::dictate::available(), |el| {
                        el.child(hint(crate::assets::Lucide::Mic, "Press the mic to talk"))
                    }),
            )
            .into_any_element()
    }

    /// Files already in `sources/`, to tick for this session.
    fn sources(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.sources.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let picked = self.workspace.read(cx).study.picked.clone();
        Some(
            h_flex()
                .w_full()
                .gap(px(6.))
                .flex_wrap()
                .items_center()
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .pr(px(2.))
                        .child("Use this session:"),
                )
                .children(self.sources.iter().enumerate().map(|(i, s)| {
                    let on = picked.contains(&s.path);
                    let path = s.path.clone();
                    h_flex()
                        .id(SharedString::from(format!("study-source-{i}")))
                        .test_support()
                        .h(px(24.))
                        .px(px(9.))
                        .gap(px(6.))
                        .rounded_full()
                        .border_1()
                        .border_color(if on {
                            ember.opacity(0.7)
                        } else {
                            theme.foreground.opacity(0.1)
                        })
                        .when(on, |el| el.bg(ember.opacity(0.1)))
                        .text_color(if on {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        })
                        .cursor_pointer()
                        .hover(|st| st.border_color(ember.opacity(0.5)))
                        .child(
                            Icon::new(if on {
                                crate::assets::Lucide::Check
                            } else {
                                crate::assets::Lucide::Paperclip
                            })
                            .size(px(11.))
                            .text_color(if on {
                                ember
                            } else {
                                theme.muted_foreground
                            }),
                        )
                        .child(s.label.clone())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let path = path.clone();
                            this.workspace
                                .update(cx, |ws, cx| ws.toggle_study_source(path, cx));
                            this.composer.update(cx, |c, cx| c.focus(window, cx));
                        }))
                }))
                .into_any_element(),
        )
    }

    /// The method, in four tiles.
    fn method(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        use crate::assets::Lucide as L;
        let steps: [(L, &str, &str); 4] = [
            (
                L::Target,
                "Find your edge",
                "Quick questions find what you already know.",
            ),
            (
                L::Route,
                "Plan the path",
                "The fewest steps to your goal, as a map.",
            ),
            (
                L::Footprints,
                "One step at a time",
                "Predict, learn, check.",
            ),
            (
                L::Repeat,
                "Practise to recall",
                "Problems that adapt; nudges, not answers.",
            ),
        ];
        v_flex()
            .gap(px(10.))
            .child(section_title("How it works", None, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(18.))
                    .flex_wrap()
                    .items_start()
                    .children(steps.into_iter().map(|(icon, title, line)| {
                        h_flex()
                            .flex_1()
                            .min_w(px(170.))
                            .items_start()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(30.))
                                    .rounded(px(8.))
                                    .bg(ember.opacity(0.14))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(icon).size(px(16.)).text_color(ember)),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(div().text_size(px(12.5)).font_medium().child(title))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .line_height(px(15.))
                                            .text_color(theme.muted_foreground)
                                            .child(line),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }

    /// The concept maps as cards, to open, carry on with, or practise.
    fn maps(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let body = if self.topics.is_empty() {
            div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(if self.loaded {
                    "Each topic you learn gets a map here."
                } else {
                    "Loading…"
                })
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .gap(px(10.))
                .flex_wrap()
                .items_stretch()
                .children(
                    self.topics
                        .iter()
                        .enumerate()
                        .map(|(i, t)| self.map_card(i, t, cx)),
                )
                .into_any_element()
        };
        v_flex()
            .gap(px(4.))
            .child(section_title("Your maps", Some(self.topics.len()), cx))
            .child(body)
            .into_any_element()
    }

    fn map_card(&self, i: usize, t: &Topic, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        let mut counts = vec![
            plural(t.claims, "idea", "ideas"),
            plural(t.links, "link", "links"),
        ];
        if t.practiced > 0 {
            counts.push(format!("{} practised", t.practiced));
        }
        let path = t.path.clone();
        let (go_on, practise) = (t.clone(), t.clone());
        v_flex()
            .id(SharedString::from(format!("study-map-{i}")))
            .test_support()
            .flex_1()
            .min_w(px(220.))
            .max_w(px(360.))
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(theme.foreground.opacity(0.08))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Icon::new(crate::assets::Lucide::Map)
                            .size(px(15.))
                            .text_color(ember),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_medium()
                            .child(t.title.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(counts.join(" · ")),
            )
            .child(
                h_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(theme.muted_foreground)
                            .when(t.updated_at > 0, |el| {
                                el.child(format!("Studied {}", crate::time::relative(t.updated_at)))
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("study-map-practice-{i}")))
                            .ghost()
                            .xsmall()
                            .label("Practice")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_on(Kind::Practice, &practise, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("study-map-learn-{i}")))
                            .ghost()
                            .xsmall()
                            .label("Learn more")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.start_on(Kind::Learn, &go_on, cx)
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
                    let icon = match s.kind {
                        Kind::Learn => crate::assets::Lucide::BookOpen,
                        Kind::Practice => crate::assets::Lucide::Repeat,
                    };
                    h_flex()
                        .id(SharedString::from(format!("study-session-{}", s.thread)))
                        .test_support()
                        .h(px(30.))
                        .gap(px(10.))
                        .px(px(8.))
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .hover(|st| st.bg(theme.list_hover))
                        .child(
                            Icon::new(icon)
                                .size(px(13.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(52.))
                                .text_right()
                                .text_size(px(11.5))
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
}

impl Render for StudyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::rendered("Study");
        let sessions = self.sessions(cx);
        let sources = self.sources(cx);
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
                    .gap(px(26.))
                    .child(inset(self.header(cx)))
                    .child(
                        v_flex()
                            .id("study-start")
                            .test_support()
                            .w_full()
                            .gap(px(10.))
                            .child(inset(self.choice(cx)))
                            .child(composer)
                            .children(sources.map(inset)),
                    )
                    .child(inset(self.maps(cx)))
                    .children(sessions.map(inset))
                    .child(inset(self.method(cx))),
            )
    }
}

/// A section's title, with a count beside it.
fn section_title(title: &'static str, count: Option<usize>, cx: &App) -> AnyElement {
    h_flex()
        .pb(px(6.))
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
