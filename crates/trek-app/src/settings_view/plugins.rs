//! Settings › Plugins: the plugins installed (each with what it brings, what it may read or do,
//! its switch, its log and Remove), what's wrong in the folder, which plugin each replaceable
//! surface shows, and the examples Trek ships, a click from installed.

use super::SettingsView;
use crate::plugins::{self, Plugins};
use crate::scale::text;
use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::plugins::{self as core, Origin, Permission, Plugin};

impl SettingsView {
    pub(super) fn plugins_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(p) = cx.try_global::<Plugins>() else {
            return vec![];
        };
        let installed = p.plugins.clone();
        let problems = p.problems.clone();
        let state = p.state.clone();
        let dir = p.dir().clone();
        let log = p.log.clone();
        let mut out = vec![];

        // Waiting first: plugins asked to be turned on (by an agent, from the CLI) that need the
        // user to allow what they ask for.
        let waiting: Vec<&Plugin> = installed
            .iter()
            .filter(|pl| {
                let c = state.choice(&pl.manifest.id);
                (c.waiting || c.on) && !state.missing(&pl.manifest).is_empty()
            })
            .collect();
        for pl in &waiting {
            out.push(self.waiting_card(pl, &state.missing(&pl.manifest), cx));
        }

        out.push(Self::heading("Installed", cx));
        if installed.is_empty() {
            out.push(Self::note(
                "No plugins yet. Ask an agent to make one (\"make Trek look like a terminal\", \"add a focus timer round the composer\"), or install an example below.",
                cx,
            ));
        } else {
            let rows = installed
                .iter()
                .map(|pl| self.plugin_row(pl, &state, log.as_ref(), cx))
                .collect();
            out.push(ui::group(rows, cx));
        }

        if !problems.is_empty() {
            out.push(Self::heading("Couldn't be loaded", cx));
            let rows = problems
                .iter()
                .map(|pr| {
                    h_flex()
                        .id(SharedString::from(format!("plugin-problem-{}", pr.id)))
                        .test_support()
                        .items_start()
                        .gap(px(10.))
                        .py(px(12.))
                        .child(
                            div().pt(px(2.)).child(
                                Icon::new(IconName::TriangleAlert)
                                    .small()
                                    .text_color(crate::palette::amber(cx)),
                            ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(div().text_size(px(text::BODY)).child(pr.id.clone()))
                                .child(
                                    div()
                                        .text_size(px(text::LABEL))
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(pr.message.clone()),
                                ),
                        )
                        .into_any_element()
                })
                .collect();
            out.push(ui::group(rows, cx));
        }

        // Each surface a running plugin offers to replace: Trek's own, or a plugin's.
        let surfaces: Vec<(core::Surface, Vec<(String, String)>)> = core::Surface::ALL
            .into_iter()
            .map(|s| (s, plugins::replacements_for(s, cx)))
            .filter(|(_, offers)| !offers.is_empty())
            .collect();
        if !surfaces.is_empty() {
            out.push(Self::heading("In place of Trek's own", cx));
            let rows = surfaces
                .into_iter()
                .map(|(surface, offers)| {
                    let chosen = state.replace.get(&surface).cloned();
                    let current = chosen
                        .as_ref()
                        .and_then(|id| offers.iter().find(|(pid, _)| pid == id))
                        .map(|(_, title)| title.clone())
                        .unwrap_or_else(|| "Trek's own".to_string());
                    let ws = self.workspace.clone();
                    let pick = Button::new(SharedString::from(format!(
                        "plugin-surface-{}",
                        surface.name()
                    )))
                    .small()
                    .outline()
                    .label(current)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let ws0 = ws.clone();
                        let mut menu = menu.min_w(px(180.)).item(
                            PopupMenuItem::new("Trek's own")
                                .checked(chosen.is_none())
                                .on_click(move |_, _, cx| {
                                    plugins::choose_replacement(surface, None, &ws0, cx)
                                }),
                        );
                        for (id, title) in &offers {
                            let (ws, id2) = (ws.clone(), id.clone());
                            menu = menu.item(
                                PopupMenuItem::new(title.clone())
                                    .checked(chosen.as_deref() == Some(id.as_str()))
                                    .on_click(move |_, _, cx| {
                                        plugins::choose_replacement(surface, Some(&id2), &ws, cx)
                                    }),
                            );
                        }
                        menu
                    });
                    Self::row(
                        surface.title(),
                        "Trek's own, or a plugin's in its place. A plugin that stops gives it back.",
                        pick,
                        cx,
                    )
                })
                .collect();
            out.push(ui::group(rows, cx));
        }

        let examples: Vec<&(&str, &[core::examples::File])> = core::examples::ALL
            .iter()
            .filter(|(name, _)| !installed.iter().any(|pl| pl.manifest.id == *name))
            .collect();
        if !examples.is_empty() {
            out.push(Self::heading("Examples", cx));
            let rows = examples
                .into_iter()
                .map(|(name, files)| self.example_row(name, files, cx))
                .collect();
            out.push(ui::group(rows, cx));
        }

        let open_dir = dir.clone();
        out.push(
            h_flex()
                .pt(px(24.))
                .gap(px(6.))
                .flex_wrap()
                .text_size(px(text::LABEL))
                .text_color(theme.muted_foreground)
                .child("Plugins are folders in")
                .child(
                    div()
                        .id("plugins-folder")
                        .test_support()
                        .cursor_pointer()
                        .text_color(theme.foreground.opacity(0.85))
                        .hover(|el| el.text_color(theme.foreground))
                        .child(SharedString::from(format!(
                            "{}{}",
                            core::DIR_NAME,
                            std::path::MAIN_SEPARATOR
                        )))
                        .on_click(move |_, _, cx| {
                            let _ = std::fs::create_dir_all(&open_dir);
                            cx.open_with_system(&open_dir);
                        }),
                )
                .child(
                    "· Trek's program installs them too: trek plugin install <folder or git URL>",
                )
                .into_any_element(),
        );
        out
    }

    /// A plugin asking for permissions before it can run: what it asks, in words, and Allow.
    fn waiting_card(
        &self,
        pl: &Plugin,
        missing: &[Permission],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let m = pl.manifest.clone();
        let ws = self.workspace.clone();
        let id = m.id.clone();
        v_flex()
            .id(SharedString::from(format!("plugin-waiting-{}", m.id)))
            .test_support()
            .mt(px(8.))
            .p(px(16.))
            .gap(px(10.))
            .rounded(px(crate::scale::radius::CARD))
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                div()
                    .text_size(px(text::TITLE))
                    .font_medium()
                    .child(format!("{} asks to be turned on", m.name)),
            )
            .child(
                div()
                    .text_size(px(text::BODY))
                    .text_color(theme.muted_foreground)
                    .child("It may then:"),
            )
            .children(missing.iter().map(|perm| permission_line(perm, cx)))
            .child(
                h_flex()
                    .gap(px(8.))
                    .pt(px(4.))
                    .child(
                        Button::new(SharedString::from(format!("plugin-allow-{id}")))
                            .small()
                            .primary()
                            .label("Allow and turn on")
                            .on_click({
                                let (id, ws) = (id.clone(), ws.clone());
                                move |_, _, cx| plugins::set_on(&id, true, &ws, cx)
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("plugin-deny-{id}")))
                            .small()
                            .ghost()
                            .label("Not now")
                            .on_click(move |_, _, cx| plugins::set_on(&id, false, &ws, cx)),
                    ),
            )
            .into_any_element()
    }

    fn plugin_row(
        &self,
        pl: &Plugin,
        state: &core::State,
        log: Option<&(String, Vec<String>)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let m = pl.manifest.clone();
        let choice = state.choice(&m.id);
        let running = state.running(&m);
        let ws = self.workspace.clone();
        let id = m.id.clone();
        let mut title = h_flex()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(text::TITLE))
                    .font_medium()
                    .when(!running, |el| el.text_color(theme.muted_foreground))
                    .child(m.name.clone()),
            )
            .child(
                div()
                    .text_size(px(text::LABEL))
                    .text_color(theme.muted_foreground)
                    .child(m.version.clone()),
            );
        if pl.origin == Origin::Linked {
            title = title.child(badge("Linked", cx));
        }
        let what = m.summary();
        let mut lines = v_flex()
            .gap(px(3.))
            .max_w(px(500.))
            .text_size(px(text::BODY));
        if !m.description.is_empty() {
            lines = lines.child(
                div()
                    .text_color(theme.muted_foreground)
                    .line_height(relative(1.5))
                    .child(m.description.clone()),
            );
        }
        if !what.is_empty() || !m.author.is_empty() {
            let mut bits = vec![];
            if !what.is_empty() {
                let mut chars = what.chars();
                bits.push(
                    chars
                        .next()
                        .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                        .unwrap_or_default(),
                );
            }
            if !m.author.is_empty() {
                bits.push(format!("by {}", m.author));
            }
            lines = lines.child(
                div()
                    .text_size(px(text::LABEL))
                    .text_color(theme.muted_foreground.opacity(0.85))
                    .child(bits.join(" · ")),
            );
        }
        if !m.permissions.is_empty() {
            lines = lines.child(
                v_flex()
                    .pt(px(4.))
                    .gap(px(2.))
                    .children(m.permissions.iter().map(|perm| permission_line(perm, cx))),
            );
        }
        if let Some(why) = &choice.stopped {
            lines = lines.child(
                h_flex()
                    .pt(px(4.))
                    .gap(px(6.))
                    .text_size(px(text::LABEL))
                    .text_color(crate::palette::amber(cx))
                    .child(Icon::new(IconName::TriangleAlert).xsmall())
                    .child(format!("Stopped: {why}")),
            );
        }
        if let Some((_, tail)) = log.filter(|(lid, _)| *lid == m.id) {
            lines = lines.child(
                div()
                    .id(SharedString::from(format!("plugin-log-{}", m.id)))
                    .test_support()
                    .mt(px(6.))
                    .p(px(8.))
                    .rounded(px(crate::scale::radius::CONTROL))
                    .bg(theme.secondary)
                    .text_size(px(text::META))
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .children(if tail.is_empty() {
                        vec![div().child("Nothing logged yet.")]
                    } else {
                        tail.iter().map(|l| div().child(l.clone())).collect()
                    }),
            );
        }
        let folder = pl.dir.clone();
        let log_open = log.is_some_and(|(lid, _)| *lid == m.id);
        let more = Button::new(SharedString::from(format!("plugin-more-{id}")))
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis).text_color(theme.muted_foreground))
            .dropdown_menu_with_anchor(Anchor::TopRight, {
                let (id, ws, name) = (id.clone(), ws.clone(), m.name.clone());
                move |menu, _, _| {
                    let (id2, id3, ws2, ws3, folder) = (
                        id.clone(),
                        id.clone(),
                        ws.clone(),
                        ws.clone(),
                        folder.clone(),
                    );
                    let name = name.clone();
                    menu.min_w(px(190.))
                        .item(
                            PopupMenuItem::new(if log_open { "Hide log" } else { "Show log" })
                                .on_click(move |_, _, cx| {
                                    plugins::show_log((!log_open).then_some(id2.as_str()), cx)
                                }),
                        )
                        .item(PopupMenuItem::new("Reload").on_click(move |_, _, cx| {
                            plugins::reread(&ws2, cx)
                        }))
                        .item(
                            PopupMenuItem::new(crate::words::words().show_in_file_manager)
                                .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                        )
                        .separator()
                        .item(PopupMenuItem::new("Remove…").icon(IconName::Delete).on_click(
                            move |_, window, cx| {
                                let (id4, ws4, name) = (id3.clone(), ws3.clone(), name.clone());
                                window.open_alert_dialog(cx, move |alert, _, _| {
                                    let (id5, ws5) = (id4.clone(), ws4.clone());
                                    alert
                                        .title(format!("Remove “{name}”?"))
                                        .description("Its folder and what it kept are deleted, and everything it added goes from Trek.")
                                        .confirm()
                                        .ok_text("Remove")
                                        .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                                        .on_ok(move |_, window, cx| {
                                            if let Err(e) = plugins::remove(&id5, &ws5, cx) {
                                                crate::toast::push(window, e, cx);
                                            }
                                            true
                                        })
                                });
                            },
                        ))
                }
            });
        let switch = Switch::new(SharedString::from(format!("plugin-on-{id}")))
            .checked(running)
            .on_click({
                let (id, ws) = (id.clone(), ws.clone());
                move |v: &bool, _, cx| plugins::set_on(&id, *v, &ws, cx)
            });
        h_flex()
            .id(SharedString::from(format!("plugin-row-{id}")))
            .test_support()
            .w_full()
            .items_start()
            .gap(px(32.))
            .py(px(14.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(title)
                    .child(lines),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(more)
                    .child(div().pl(px(4.)).child(switch)),
            )
            .into_any_element()
    }

    fn example_row(
        &self,
        name: &'static str,
        files: &[core::examples::File],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        // The example's own manifest says what it is.
        let (title, about) = core::examples::about(files);
        let field = |k: &str| {
            if k == "name" {
                title.clone()
            } else {
                about.clone()
            }
        };
        let ws = self.workspace.clone();
        let install = Button::new(SharedString::from(format!("plugin-example-{name}")))
            .small()
            .outline()
            .icon(IconName::ArrowDown)
            .label("Install")
            .on_click(move |_, _, cx| plugins::install(format!("example:{name}"), &ws, cx));
        h_flex()
            .w_full()
            .gap(px(32.))
            .py(px(13.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(text::TITLE))
                            .font_medium()
                            .child(field("name")),
                    )
                    .child(
                        div()
                            .max_w(px(460.))
                            .text_size(px(text::BODY))
                            .line_height(relative(1.5))
                            .text_color(theme.muted_foreground)
                            .child(field("description")),
                    ),
            )
            .child(install)
            .into_any_element()
    }
}

fn permission_line(p: &Permission, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap(px(7.))
        .text_size(px(text::LABEL))
        .text_color(theme.muted_foreground)
        .child(
            Icon::new(if p.acts() {
                IconName::CircleUser
            } else {
                IconName::Eye
            })
            .xsmall()
            .text_color(theme.muted_foreground.opacity(0.8)),
        )
        .child(p.words())
        .into_any_element()
}

fn badge(text: &'static str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(crate::scale::radius::TAG))
        .border_1()
        .border_color(theme.border)
        .text_size(px(text::META))
        .text_color(theme.muted_foreground)
        .child(text)
        .into_any_element()
}
