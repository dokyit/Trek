//! The Add agent sheet (Settings › Agents, and onboarding's agents step). Two ways in: an agent
//! from the ACP Registry, installed in a click, or a command of the user's own. Pay-per-token
//! model APIs aren't agents to add: the footer points to where their keys go.

use crate::palette;
use crate::ui;
use crate::workspace::{AgentInstall, Route, SettingsPage, Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;
use trek_core::registry::{self, AgentSource, EnvVar, RegistryAgent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Registry,
    Command,
}

/// Open the sheet on `tab`. The registry's index is read (and fetched when it's old) as it opens.
pub fn open(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut App) {
    workspace.update(cx, |ws, cx| ws.load_registry(false, cx));
    let sheet = cx.new(|cx| AddAgentSheet::new(workspace, tab, window, cx));
    let focus = sheet.clone();
    window.defer(cx, move |window, cx| focus.update(cx, |s, cx| s.focus_first(window, cx)));
    window.open_dialog(cx, move |dialog, _, _| {
        // Esc, a click beside it, its ×: it leaves as it came, rather than vanishing.
        let leaving = sheet.clone();
        dialog.w(px(640.)).p(px(0.)).child(sheet.clone()).on_close(move |_, window, cx| left(&leaving, window, cx))
    });
}

/// The sheet went (the library took it down): let it leave (`motion::sheet_left`).
fn left(sheet: &Entity<AddAgentSheet>, window: &mut Window, cx: &mut App) {
    let (bounds, motion) = {
        let s = sheet.read(cx);
        (s.bounds.clone(), s.workspace.read(cx).motion(cx))
    };
    crate::motion::sheet_left(sheet.clone().into(), &bounds, motion, window, cx);
}

impl AddAgentSheet {
    /// Take the sheet down from inside it, letting it leave.
    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let motion = self.workspace.read(cx).motion(cx);
        window.close_dialog(cx);
        crate::motion::sheet_left(cx.entity().into(), &self.bounds, motion, window, cx);
    }
}

/// One environment variable row of the command form.
struct EnvRow {
    name: Entity<InputState>,
    value: Entity<InputState>,
    secret: bool,
}

pub struct AddAgentSheet {
    workspace: Entity<Workspace>,
    tab: Tab,
    search: Entity<InputState>,
    name: Entity<InputState>,
    command: Entity<InputState>,
    id: Entity<InputState>,
    args: Vec<Entity<InputState>>,
    env: Vec<EnvRow>,
    /// Why the command form wasn't added.
    error: Option<String>,
    /// Where it's drawn, for leaving from there.
    bounds: crate::motion::SheetBounds,
    _subscriptions: Vec<Subscription>,
}

/// Where a registry agent stands in Trek, as its row shows it.
enum RowState {
    BuiltIn,
    Added { update: Option<String> },
    Installing(String),
    Failed(String),
    /// No build for this Mac and no package to run.
    Unavailable,
    Available,
}

/// The sheet's height: switching tabs or filtering never resizes it.
const HEIGHT: f32 = 560.;
const PAD: f32 = 20.;

impl AddAgentSheet {
    fn new(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search the ACP Registry"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. My agent"));
        let command = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. my-agent or /usr/local/bin/my-agent"));
        let id = cx.new(|cx| InputState::new(window, cx).placeholder("Made from the name"));
        let subs = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.observe(&search, |_, _, cx| cx.notify()),
            // The id field shows the id the name makes until one is typed.
            cx.observe_in(&name, window, |this: &mut Self, name, window, cx| {
                let made = registry::id_from_name(&name.read(cx).value());
                let hint = if made.is_empty() { "Made from the name".to_string() } else { made };
                this.id.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                cx.notify();
            }),
        ];
        // An error is about what was typed: typing in the name or program again clears it.
        let mut subs = subs;
        for input in [&name, &command] {
            subs.push(cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && this.error.take().is_some() {
                    cx.notify();
                }
            }));
        }
        Self { workspace, tab, search, name, command, id, args: vec![], env: vec![], error: None, bounds: Default::default(), _subscriptions: subs }
    }

    /// The tab's first field: the search, or the name.
    fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = if self.tab == Tab::Registry { &self.search } else { &self.name };
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.focus_first(window, cx);
            cx.notify();
        }
    }

    fn state_of(&self, a: &RegistryAgent, cx: &App) -> RowState {
        let ws = self.workspace.read(cx);
        if registry::built_in(&a.id).is_some() {
            return RowState::BuiltIn;
        }
        match ws.added_agents.installs.get(&a.id) {
            Some(i @ AgentInstall::Running(_)) => return RowState::Installing(i.label()),
            Some(AgentInstall::Failed(e)) => return RowState::Failed(e.clone()),
            None => {}
        }
        if ws.settings.added_agents.iter().any(|x| x.id == a.id) {
            return RowState::Added { update: ws.registry_update(&a.id) };
        }
        if registry::plan(a, registry::current_target()).is_none() { RowState::Unavailable } else { RowState::Available }
    }

    // -----------------------------------------------------------------------------------------
    // Registry

    fn registry_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let ws = self.workspace.read(cx);
        let (loading, error, registry) = (ws.added_agents.loading, ws.added_agents.error.clone(), ws.added_agents.registry.clone());
        let query = self.search.read(cx).value().to_string();
        // Fetched when, or what went wrong; and a way to fetch again.
        let status: AnyElement = match (&registry, loading, &error) {
            (_, true, _) => h_flex().gap(px(6.)).child(Spinner::new().xsmall().color(muted)).child("Checking…").into_any_element(),
            (_, false, Some(_)) => div().text_color(palette::amber(cx)).child("Offline").into_any_element(),
            (Some(r), false, None) if r.fetched_at > 0 => div()
                .child(match crate::time::relative(r.fetched_at) {
                    when if when == "now" => "Updated just now".to_string(),
                    when if when.starts_with(|c: char| c.is_ascii_digit()) && when.ends_with(|c: char| c.is_ascii_alphabetic()) => format!("Updated {when} ago"),
                    when => format!("Updated {when}"),
                })
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let refresh = ui::icon_button("registry-refresh", IconName::RefreshCw, "Check the registry again").disabled(loading).on_click(cx.listener(|this, _, _, cx| {
            this.workspace.update(cx, |ws, cx| ws.load_registry(true, cx));
        }));
        let toolbar = h_flex()
            .px(px(PAD))
            .pb(px(10.))
            .gap(px(10.))
            .child(div().flex_1().child(Input::new(&self.search).id("registry-search").small().prefix(Icon::new(IconName::Search).size(px(14.)).text_color(muted))))
            .child(h_flex().id("registry-status").test_support().flex_none().gap(px(2.)).text_size(px(12.)).text_color(muted).child(status).child(refresh));
        let list: AnyElement = match registry {
            None if loading => Self::placeholder(h_flex().gap(px(8.)).child(Spinner::new().small().color(muted)).child("Reading the ACP Registry…"), cx),
            None => {
                let why = error.unwrap_or_else(|| "The ACP Registry hasn't been read yet.".into());
                Self::placeholder(
                    v_flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().max_w(px(420.)).text_center().child(why))
                        .child(Button::new("registry-retry").small().outline().label("Try again").on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| ws.load_registry(true, cx));
                        }))),
                    cx,
                )
            }
            Some(r) => {
                let hits: Vec<RegistryAgent> = r.search(&query).into_iter().cloned().collect();
                if hits.is_empty() {
                    Self::placeholder(div().child(format!("No agent in the registry matches “{}”.", query.trim())), cx)
                } else {
                    let rows: Vec<AnyElement> = hits.iter().map(|a| self.registry_row(a, cx)).collect();
                    div()
                        .id("registry-list")
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(v_flex().pb(px(6.)).children(rows))
                        .into_any_element()
                }
            }
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(div().h(px(1.)).flex_none().bg(theme.foreground.opacity(0.07)))
            .child(list)
            .into_any_element()
    }

    /// A quiet message in the middle of the list area.
    fn placeholder(content: impl IntoElement, cx: &App) -> AnyElement {
        v_flex()
            .id("registry-empty")
            .test_support()
            .flex_1()
            .items_center()
            .justify_center()
            .px(px(PAD * 2.))
            .text_size(px(13.))
            .text_color(cx.theme().muted_foreground)
            .child(content)
            .into_any_element()
    }

    fn registry_row(&self, a: &RegistryAgent, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let state = self.state_of(a, cx);
        let id = a.id.clone();
        let install = {
            let (ws, id) = (self.workspace.clone(), id.clone());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| ws.update(cx, |ws, cx| ws.install_registry_agent(&id, cx))
        };
        let chip = |id: String, child: AnyElement| {
            h_flex()
                .id(SharedString::from(id))
                .test_support()
                .h(px(24.))
                .px(px(8.))
                .gap(px(6.))
                .rounded(px(6.))
                .bg(theme.foreground.opacity(0.05))
                .text_size(px(12.5))
                .child(child)
                .into_any_element()
        };
        let control: AnyElement = match &state {
            RowState::BuiltIn => div().id(SharedString::from(format!("registry-built-in-{id}"))).test_support().text_size(px(12.5)).text_color(muted).child("Built in").into_any_element(),
            RowState::Added { update: None } => h_flex()
                .id(SharedString::from(format!("registry-added-{id}")))
                .test_support()
                .gap(px(5.))
                .text_size(px(12.5))
                .child(Icon::new(IconName::Check).xsmall().text_color(palette::emerald(cx)))
                .child("Added")
                .into_any_element(),
            RowState::Added { update: Some(v) } => {
                Button::new(SharedString::from(format!("registry-update-{id}"))).small().outline().label(format!("Update to {v}")).on_click(install).into_any_element()
            }
            RowState::Installing(label) => chip(format!("registry-installing-{id}"), h_flex().gap(px(6.)).child(Spinner::new().xsmall().color(muted)).child(label.clone()).into_any_element()),
            RowState::Failed(_) => Button::new(SharedString::from(format!("registry-add-{id}"))).small().outline().label("Retry").on_click(install).into_any_element(),
            RowState::Unavailable => div().text_size(px(12.5)).text_color(muted).child(format!("Not for {}", crate::words::words().this_computer)).into_any_element(),
            RowState::Available => Button::new(SharedString::from(format!("registry-add-{id}"))).small().outline().label("Add").on_click(install).into_any_element(),
        };
        // npx and uvx need Node and uv: say which, so a failure to start isn't a surprise.
        let runner = registry::plan(a, registry::current_target()).filter(|p| !matches!(p, registry::Plan::Binary(_))).map(|p| p.kind());
        let detail: AnyElement = match &state {
            RowState::Failed(e) => div().text_size(px(12.5)).line_height(relative(1.45)).text_color(palette::red(cx)).child(e.clone()).into_any_element(),
            _ => div().truncate().text_size(px(12.5)).text_color(muted).child(if a.description.is_empty() { a.authors.join(", ") } else { a.description.clone() }).into_any_element(),
        };
        let link = a.link().map(|url| {
            let url = url.to_string();
            div()
                .id(SharedString::from(format!("registry-link-{id}")))
                .flex_none()
                .cursor_pointer()
                .text_color(muted.opacity(0.7))
                .hover(|s| s.text_color(theme.foreground))
                .child(Icon::new(crate::assets::Lucide::SquareArrowOutUpRight).size(px(11.)))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new("Open its website").build(window, cx))
                .on_click(move |_, _, cx| cx.open_url(&url))
        });
        h_flex()
            .id(SharedString::from(format!("registry-row-{id}")))
            .test_support()
            .w_full()
            .min_h(px(54.))
            .px(px(PAD))
            .py(px(9.))
            .gap(px(12.))
            .hover(|s| s.bg(theme.foreground.opacity(0.025)))
            .child(div().size(px(28.)).flex_none().rounded(px(7.)).bg(theme.foreground.opacity(0.045)).flex().items_center().justify_center().child(ui::registry_logo(&a.name, a.icon_file.as_deref(), px(16.), cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().flex_none().text_size(px(13.5)).font_medium().child(a.name.clone()))
                            .child(div().flex_none().text_size(px(12.)).text_color(muted.opacity(0.8)).child(a.version.clone()))
                            .when_some(runner, |el, r| el.child(div().flex_none().px(px(5.)).rounded(px(4.)).bg(theme.foreground.opacity(0.06)).text_size(px(11.)).text_color(muted).child(r)))
                            .children(link),
                    )
                    .child(detail),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    // -----------------------------------------------------------------------------------------
    // Your own command

    fn add_arg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. --acp"));
        input.update(cx, |s, cx| s.focus(window, cx));
        self.args.push(input);
        cx.notify();
    }

    fn add_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("NAME"));
        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        name.update(cx, |s, cx| s.focus(window, cx));
        self.env.push(EnvRow { name, value, secret: false });
        cx.notify();
    }

    /// A labelled field of the form: the label on the left, the control and a hint under it.
    fn field(label: &str, control: impl IntoElement, hint: Option<&str>, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .items_start()
            .gap(px(16.))
            .child(div().w(px(92.)).flex_none().pt(px(3.)).text_size(px(13.)).text_color(cx.theme().foreground.opacity(0.85)).child(label.to_string()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(control)
                    .when_some(hint, |el, h| el.child(div().text_size(px(12.)).line_height(relative(1.45)).text_color(muted).child(h.to_string()))),
            )
            .into_any_element()
    }

    fn command_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let args: Vec<AnyElement> = self
            .args
            .iter()
            .enumerate()
            .map(|(i, input)| {
                h_flex()
                    .gap(px(4.))
                    .child(div().flex_1().font_family(mono.clone()).child(Input::new(input).id(SharedString::from(format!("arg-{i}"))).small()))
                    .child(ui::icon_button(SharedString::from(format!("arg-remove-{i}")), IconName::Close, "Remove").on_click(cx.listener(move |this, _, _, cx| {
                        if i < this.args.len() {
                            this.args.remove(i);
                        }
                        cx.notify();
                    })))
                    .into_any_element()
            })
            .collect();
        let env: Vec<AnyElement> = self
            .env
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let secret = row.secret;
                let lock = Button::new(SharedString::from(format!("env-secret-{i}")))
                    .ghost()
                    .small()
                    .selected(secret)
                    .icon(Icon::new(if secret { crate::assets::Lucide::Lock } else { crate::assets::Lucide::LockOpen }).text_color(if secret { theme.foreground } else { muted }))
                    .tooltip(if secret { format!("Secret: kept in {}", crate::words::words().your_credential_store) } else { format!("Keep it in {} instead of settings", crate::words::words().your_credential_store) })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(row) = this.env.get_mut(i) {
                            row.secret = !row.secret;
                            let on = row.secret;
                            row.value.update(cx, |s, cx| s.set_masked(on, window, cx));
                        }
                        cx.notify();
                    }));
                h_flex()
                    .gap(px(4.))
                    .child(div().w(px(150.)).flex_none().font_family(mono.clone()).child(Input::new(&row.name).id(SharedString::from(format!("env-name-{i}"))).small()))
                    .child(div().flex_1().min_w_0().child(Input::new(&row.value).id(SharedString::from(format!("env-value-{i}"))).small()))
                    .child(lock)
                    .child(ui::icon_button(SharedString::from(format!("env-remove-{i}")), IconName::Close, "Remove").on_click(cx.listener(move |this, _, _, cx| {
                        if i < this.env.len() {
                            this.env.remove(i);
                        }
                        cx.notify();
                    })))
                    .into_any_element()
            })
            .collect();
        let add_row = |id: &'static str, label: &'static str| Button::new(id).ghost().xsmall().icon(Icon::new(IconName::Plus).text_color(muted)).label(label);
        let id_hint = "Threads and settings know the agent by it. Lowercase letters, digits, dots, dashes and underscores.";
        let env_hint = (!self.env.is_empty()).then(|| format!("A locked value is kept in {}, never in settings.toml.", crate::words::words().your_credential_store));
        div()
            .id("command-form")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .px(px(PAD))
                    .pt(px(4.))
                    .pb(px(16.))
                    .gap(px(16.))
                    .child(Self::field("Name", Input::new(&self.name).id("command-name").small(), None, cx))
                    .child(Self::field(
                        "Program",
                        div().font_family(mono.clone()).child(Input::new(&self.command).id("command-program").small()),
                        Some("Its name on your PATH, or its full path. Trek talks to it over the Agent Client Protocol, on stdin and stdout; it keeps its own login."),
                        cx,
                    ))
                    .child(Self::field(
                        "Arguments",
                        v_flex().gap(px(6.)).children(args).child(h_flex().child(add_row("arg-add", "Add argument").on_click(cx.listener(|this, _, window, cx| this.add_arg(window, cx))))),
                        None,
                        cx,
                    ))
                    .child(Self::field(
                        "Environment",
                        v_flex().gap(px(6.)).children(env).child(h_flex().child(add_row("env-add", "Add variable").on_click(cx.listener(|this, _, window, cx| this.add_env(window, cx))))),
                        env_hint.as_deref(),
                        cx,
                    ))
                    .child(Self::field("ID", div().font_family(mono).child(Input::new(&self.id).id("command-id").small()), Some(id_hint), cx)),
            )
            .into_any_element()
    }

    /// Add the agent the form describes, or say what's wrong with it.
    fn add_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let read = |i: &Entity<InputState>, cx: &App| i.read(cx).value().trim().to_string();
        let (name, id, command) = (read(&self.name, cx), read(&self.id, cx), read(&self.command, cx));
        let args: Vec<String> = self.args.iter().map(|a| read(a, cx)).collect();
        let mut env = vec![];
        let mut secrets = vec![];
        for row in &self.env {
            let (n, v) = (read(&row.name, cx), row.value.read(cx).value().to_string());
            if n.is_empty() && v.is_empty() {
                continue;
            }
            if row.secret {
                secrets.push((n.clone(), v));
                env.push(EnvVar { name: n, value: String::new(), secret: true });
            } else {
                env.push(EnvVar { name: n, value: v, secret: false });
            }
        }
        let taken: Vec<String> = self.workspace.read(cx).settings.added_agents.iter().map(|a| a.id.clone()).collect();
        let added = registry::custom(&name, &id, &command, args, env, &taken).and_then(|agent| {
            let name = agent.name.clone();
            self.workspace.update(cx, |ws, cx| ws.add_custom_agent(agent, secrets, cx)).map(|_| name)
        });
        match added {
            Ok(name) => {
                self.error = None;
                self.close(window, cx);
                self.workspace.update(cx, |ws, cx| {
                    if ws.route != Route::Settings(SettingsPage::Agents) && ws.route != Route::Onboarding {
                        ws.navigate(Route::Settings(SettingsPage::Agents), cx);
                    }
                    cx.emit(WorkspaceEvent::Toast { message: format!("Added {name}"), undo: None });
                });
            }
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }

    fn footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        // Model APIs aren't agents: their keys and endpoints have a page of their own.
        let api = h_flex()
            .id("add-agent-api-keys")
            .test_support()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(muted)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.foreground))
            .child(Icon::new(crate::assets::Lucide::Key).size(px(13.)))
            .child("Paying per token instead? Add an API key")
            .child(Icon::new(IconName::ChevronRight).size(px(12.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.close(window, cx);
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::ApiKeys), cx));
            }));
        h_flex()
            .flex_none()
            .h(px(52.))
            .px(px(PAD))
            .gap(px(8.))
            .border_t_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(div().flex_1().min_w_0().child(match self.error.clone().filter(|_| self.tab == Tab::Command) {
                // What's wrong with the form, beside the button that tried.
                Some(e) => h_flex()
                    .id("command-error")
                    .test_support()
                    .gap(px(7.))
                    .text_size(px(12.5))
                    .line_height(relative(1.35))
                    .text_color(palette::red(cx))
                    .child(Icon::new(IconName::TriangleAlert).xsmall().flex_none())
                    .child(div().line_clamp(2).child(e))
                    .into_any_element(),
                None => api.into_any_element(),
            }))
            .when(self.tab == Tab::Command, |el| {
                el.child(Button::new("command-cancel").small().ghost().label("Cancel").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
                    .child(Button::new("command-add").small().primary().label("Add agent").on_click(cx.listener(|this, _, window, cx| this.add_command(window, cx))))
            })
            .into_any_element()
    }
}

impl Render for AddAgentSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let pick = move |tab: Tab, window: &mut Window, cx: &mut App| view.update(cx, |this, cx| this.set_tab(tab, window, cx));
        let header = v_flex()
            .flex_none()
            .px(px(PAD))
            .pt(px(18.))
            .pb(px(14.))
            .gap(px(4.))
            .child(div().text_size(px(16.)).font_semibold().child("Add an agent"))
            .child(
                div()
                    .max_w(px(520.))
                    .text_size(px(12.5))
                    .line_height(relative(1.5))
                    .text_color(theme.muted_foreground)
                    .child("Any agent that speaks the Agent Client Protocol runs in Trek like the built-in ones, signed in with its own account."),
            )
            .child(h_flex().pt(px(12.)).child(ui::segmented("add-agent-tab", vec![(Tab::Registry, "ACP Registry"), (Tab::Command, "Your own command")], self.tab, pick, cx)));
        let body = match self.tab {
            Tab::Registry => self.registry_body(cx),
            Tab::Command => self.command_body(cx),
        };
        v_flex().id("add-agent-sheet").test_support().relative().w_full().h(px(HEIGHT)).child(header).child(body).child(self.footer(cx)).child(crate::motion::sheet_marker(&self.bounds))
    }
}

/// What the Agents page says of an added agent under its name: where it came from.
pub fn source_line(agent: &AgentId) -> Option<String> {
    let AgentId::Acp(id) = agent else { return None };
    let a = trek_core::catalog::added_agent(id)?;
    Some(match a.source {
        AgentSource::Registry => match a.distribution.as_deref() {
            Some(kind @ ("npx" | "uvx")) => format!("ACP Registry · runs with {kind}"),
            _ => "ACP Registry".to_string(),
        },
        AgentSource::Command => a.command_line(),
    })
}
