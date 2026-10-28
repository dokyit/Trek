//! Study: learning anything with an agent as a one-to-one tutor. The screen starts a session (a
//! thread following Trek's study guide in the study folder: `Workspace::start_study`), lists the
//! learner's concept maps (`trek_core::study::topics`) to carry on with or practise, and their
//! past sessions.
//!
//! The folder is read off the main thread when the screen opens and whenever threads change while
//! it's open (a session's turn may have added to a map); frames only draw what was read.

use crate::ui;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::study::{self, Kind, Session, Topic};

/// Past sessions shown before the list stops.
const SESSION_ROWS: usize = 12;

pub struct StudyView {
    workspace: Entity<Workspace>,
    focus: FocusHandle,
    topic: Entity<InputState>,
    topics: Vec<Topic>,
    sessions: Vec<Session>,
    /// The folder has been read at least once.
    loaded: bool,
    /// On screen (the main window's route).
    open: bool,
    /// `Workspace::threads_gen` when the folder was last read.
    read_at: Option<u64>,
    _read: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl StudyView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let topic = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                "What do you want to learn? Say how deep, and name any notes or PDFs to use",
            )
        });
        let _subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.subscribe(&topic, |this, _, event: &InputEvent, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.start(Kind::Learn, cx),
                    // The buttons wake up once there's something to learn.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            }),
        ];
        let mut this = Self {
            workspace,
            focus: cx.focus_handle(),
            topic,
            topics: vec![],
            sessions: vec![],
            loaded: false,
            open: false,
            read_at: None,
            _read: None,
            _subscriptions,
        };
        this.sync(cx);
        this
    }

    /// What takes the keys when the screen opens: the topic field.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.topic.read(cx).focus_handle(cx)
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

    /// Read the maps and sessions in the background.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_spawn(async move {
            let dir = study::dir();
            (study::topics(&dir), study::sessions(&dir))
        });
        self._read = Some(cx.spawn(async move |this, cx| {
            let (topics, sessions) = read.await;
            let _ = this.update(cx, |this, cx| {
                if (this.topics != topics) || (this.sessions != sessions) || !this.loaded {
                    this.topics = topics;
                    this.sessions = sessions;
                    this.loaded = true;
                    cx.notify();
                }
            });
        }));
    }

    /// A new session on what's typed.
    fn start(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let topic = self.topic.read(cx).value().trim().to_string();
        if topic.is_empty() {
            return;
        }
        self.workspace
            .update(cx, |ws, cx| ws.start_study(kind, topic, None, cx));
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
                    .child(
                        "Learn anything with a tutor that teaches to the edge of what you know, \
                         then practise until it comes to mind when you need it.",
                    ),
            )
            .into_any_element()
    }

    /// The field and its two ways in.
    fn starter(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let empty = self.topic.read(cx).value().trim().is_empty();
        v_flex()
            .id("study-start")
            .test_support()
            .w_full()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(theme.foreground.opacity(0.08))
            .bg(theme.foreground.opacity(0.025))
            .child(Input::new(&self.topic).appearance(false))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.))
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("Practice works on your own problems too: paste them, or name the file."),
                    )
                    .child(
                        Button::new("study-practice")
                            .outline()
                            .small()
                            .icon(Icon::new(crate::assets::Lucide::Brain))
                            .label("Practice")
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.start(Kind::Practice, cx))),
                    )
                    .child(
                        Button::new("study-learn")
                            .primary()
                            .small()
                            .icon(Icon::new(crate::assets::Lucide::BookOpen))
                            .label("Start learning")
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.start(Kind::Learn, cx))),
                    ),
            )
            .into_any_element()
    }

    /// The method, in four steps, and what the working box is for.
    fn method(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        use crate::assets::Lucide as L;
        let steps: [(L, &str, &str); 4] = [
            (
                L::Crosshair,
                "Find your edge",
                "Quick questions pin down what you already know, so nothing is taught twice or too soon.",
            ),
            (
                L::Workflow,
                "Plan the path",
                "The fewest steps from there to your goal, drawn as a map you approve first.",
            ),
            (
                L::Lightbulb,
                "One step at a time",
                "Predict, learn, check. Each step should feel obviously true before the next.",
            ),
            (
                L::Brain,
                "Practise to recall",
                "Problems that adapt to where you struggle, with nudges instead of answers.",
            ),
        ];
        v_flex()
            .gap(px(10.))
            .child(section_title("How it works", None, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.))
                    .flex_wrap()
                    .children(steps.into_iter().enumerate().map(|(i, (icon, title, line))| {
                        v_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .gap(px(6.))
                            .p(px(12.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.foreground.opacity(0.07))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .child(
                                        Icon::new(icon)
                                            .size(px(14.))
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .child(format!("{}. {title}", i + 1)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(theme.muted_foreground)
                                    .child(line),
                            )
                    })),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(
                        "Every question has a box for your working: think out loud, messy is fine. \
                         It shows your tutor how you reason, not just whether you're right. Your maps \
                         keep what you've learned and how it connects, across sessions.",
                    ),
            )
            .into_any_element()
    }

    /// The concept maps, to open, carry on with, or practise.
    fn maps(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let body = if self.topics.is_empty() {
            div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(if self.loaded {
                    "Topics you learn show up here, with the ideas covered and how they connect."
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
            .child(section_title("Your maps", Some(self.topics.len()), cx))
            .child(body)
            .into_any_element()
    }

    fn map_row(&self, i: usize, t: &Topic, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut counts = vec![
            plural(t.claims, "idea", "ideas"),
            plural(t.links, "link", "links"),
        ];
        if t.practiced > 0 {
            counts.push(format!("{} practised", t.practiced));
        }
        let path = t.path.clone();
        let (go_on, practise) = (t.clone(), t.clone());
        h_flex()
            .id(SharedString::from(format!("study-map-{i}")))
            .test_support()
            .h(px(36.))
            .gap(px(10.))
            .px(px(8.))
            .text_size(px(13.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(
                Icon::new(crate::assets::Lucide::Workflow)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().truncate().child(t.title.clone()))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(counts.join(" · ")),
            )
            .when(t.updated_at > 0, |el| {
                el.child(
                    div()
                        .flex_none()
                        .min_w(px(52.))
                        .text_right()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(crate::time::relative(t.updated_at)),
                )
            })
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
                        Kind::Practice => crate::assets::Lucide::Brain,
                    };
                    h_flex()
                        .id(SharedString::from(format!("study-session-{}", s.thread)))
                        .test_support()
                        .h(px(34.))
                        .gap(px(10.))
                        .px(px(8.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(|st| st.bg(theme.list_hover))
                        .child(
                            Icon::new(icon)
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
}

impl Render for StudyView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::rendered("Study");
        let sessions = self.sessions(cx);
        div()
            .id("study")
            .test_support()
            .key_context("Study")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(960.))
                    .mx_auto()
                    .px(px(40.))
                    .pt(px(28.))
                    .pb(px(48.))
                    .gap(px(28.))
                    .child(self.header(cx))
                    .child(self.starter(cx))
                    .child(self.maps(cx))
                    .children(sessions)
                    .child(self.method(cx)),
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
