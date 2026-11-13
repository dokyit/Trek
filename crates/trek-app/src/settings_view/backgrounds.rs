//! Settings › Appearance › Background: the gallery of collections (`crate::backgrounds`), where
//! the art shows and how dim it is.

use super::SettingsView;
use crate::backgrounds::{self, Collection, PIECES};
use crate::fluent;
use crate::scale::{radius, text, to_rems};
use crate::ui;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::{BackgroundPlacement, Settings};

/// A landscape thumbnail, and the tall one of a sidebar piece.
const WIDE: (f32, f32) = (100., 66.);
const TALL: (f32, f32) = (56., 92.);

impl SettingsView {
    /// The Background section: heading, note, the gallery, then where it shows and its Dim.
    pub(super) fn background_section(
        &self,
        s: &Settings,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![
            Self::heading("Background", cx),
            Self::note(
                "Off by default. Paintings and Light fill the chat; ASCII art stays in the sidebar, the classic Trek look; Classic goes where you put it.",
                cx,
            ),
            self.background_gallery(s, cx),
            div().h(px(16.)).into_any_element(),
        ];
        // Where the art shows and how dim it is only mean something once there's art.
        let Some(spec) = s.appearance.background.clone() else {
            return out;
        };
        let collection = backgrounds::collection_of(&spec);
        let mut rows = Vec::new();
        if collection.sidebar_only() {
            rows.push(Self::row(
                "Show on",
                "ASCII art shows in the sidebar only: the chat stays plain.",
                div()
                    .id("bg-place-sidebar")
                    .test_support()
                    .text_size(to_rems(text::META))
                    .text_color(cx.theme().muted_foreground)
                    .child("Sidebar"),
                cx,
            ));
        } else {
            rows.push(Self::row(
                "Show on",
                "",
                ui::segmented(
                    "bg-place",
                    vec![
                        (BackgroundPlacement::NewThread, "New thread"),
                        (BackgroundPlacement::Chat, "Whole chat"),
                        (BackgroundPlacement::Everywhere, "Sidebar"),
                    ],
                    s.appearance.background_placement,
                    self.setter(|s, v| s.appearance.background_placement = v),
                    cx,
                ),
                cx,
            ));
        }
        rows.push(Self::row(
            "Dim",
            if collection == Collection::Classic {
                "Darkens the art so text stays readable."
            } else {
                "Text keeps its contrast at any setting; this darkens the art further."
            },
            ui::segmented(
                "bg-dim",
                vec![(0u8, "None"), (1, "Light"), (2, "Medium"), (3, "Strong")],
                match s.appearance.background_dim {
                    d if d < 0.1 => 0u8,
                    d if d < 0.3 => 1,
                    d if d < 0.5 => 2,
                    _ => 3,
                },
                self.setter(|s, v: u8| {
                    s.appearance.background_dim = [0.0, 0.2, 0.4, 0.6][v as usize]
                }),
                cx,
            ),
            cx,
        ));
        out.push(ui::group(rows, cx));
        out
    }

    /// Each collection under its title: Classic (with None first), Paintings, ASCII and Light,
    /// then the user's own image and "Choose image…".
    fn background_gallery(&self, s: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let current = s.appearance.background.clone();
        let dark = cx.theme().mode.is_dark();
        let mut groups = Vec::new();
        for collection in Collection::ALL {
            let mut tiles: Vec<AnyElement> = Vec::new();
            if collection == Collection::Classic {
                tiles.push(self.tile("bg-none", "None", None, None, current.is_none(), WIDE, cx));
            }
            for p in PIECES.iter().filter(|p| p.collection == collection) {
                let spec = p.spec();
                let size = if collection.in_sidebar_column() {
                    TALL
                } else {
                    WIDE
                };
                tiles.push(self.tile(
                    &format!("bg-{}", p.id),
                    p.name,
                    Some((spec.clone(), backgrounds::thumbnail(&spec, dark))),
                    (!p.credit.is_empty()).then_some(p.credit),
                    current.as_deref() == Some(spec.as_str()),
                    size,
                    cx,
                ));
            }
            groups.push(Self::collection_group(
                collection.title(),
                collection.note(),
                tiles,
                cx,
            ));
        }
        let mut own: Vec<AnyElement> = Vec::new();
        if let Some(c) = current.clone().filter(|c| !c.starts_with("builtin:")) {
            let thumb = backgrounds::thumbnail(&c, dark);
            own.push(self.tile("bg-yours", "Yours", Some((c, thumb)), None, true, WIDE, cx));
        }
        own.push(self.choose_tile(cx));
        groups.push(Self::collection_group(
            "Your own",
            "A picture of yours, wherever you put it.",
            own,
            cx,
        ));
        v_flex()
            .id("bg-gallery")
            .test_support()
            .gap(px(22.))
            .px(px(4.))
            .children(groups)
            .into_any_element()
    }

    fn collection_group(
        title: &'static str,
        note: &'static str,
        tiles: Vec<AnyElement>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .gap(px(10.))
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(to_rems(text::LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child(note),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .items_end()
                    .gap(px(12.))
                    .children(tiles),
            )
            .into_any_element()
    }

    /// One choice: its picture (or the ground, for None) in a ring when it's the one chosen, its
    /// name under it, and who made it on hover.
    #[allow(clippy::too_many_arguments)]
    fn tile(
        &self,
        id: &str,
        label: &'static str,
        art: Option<(String, ImageSource)>,
        credit: Option<&'static str>,
        selected: bool,
        (w, h): (f32, f32),
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let spec = art.as_ref().map(|(s, _)| s.clone());
        let ws = self.workspace.clone();
        v_flex()
            .id(SharedString::from(id.to_string()))
            .test_support()
            .w(px(w))
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(fluent::control(radius::BUTTON)))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if selected {
                        theme.foreground
                    } else {
                        theme.border
                    })
                    .bg(theme.background)
                    .when_some(art, |el, (_, thumb)| {
                        el.child(img(thumb).size_full().object_fit(ObjectFit::Cover))
                    }),
            )
            .child(
                div()
                    .text_size(to_rems(text::META))
                    .truncate()
                    .text_color(if selected {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(label),
            )
            .when_some(credit, |el, credit| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(credit).build(window, cx)
                })
            })
            .on_click(move |_, _, cx| {
                let spec = spec.clone();
                ws.update(cx, |ws, cx| {
                    let a = &mut ws.settings.appearance;
                    let before = a.background.as_deref().map(backgrounds::collection_of);
                    let after = spec.as_deref().map(backgrounds::collection_of);
                    // A collection that has a place of its own takes it: Paintings and Light the
                    // whole chat, ASCII the sidebar. Classic stays where the user put it.
                    if let Some(place) = after.and_then(|c| c.placement()) {
                        a.background_placement = place;
                    }
                    // Coming from another collection, Dim starts where that art is meant to be
                    // seen: none over the scrim the new collections bring, Classic's own default.
                    if after.is_some() && after != before {
                        a.background_dim = match after {
                            Some(backgrounds::Collection::Classic) => {
                                trek_core::settings::Appearance::default().background_dim
                            }
                            _ => 0.0,
                        };
                    }
                    a.background = spec;
                    ws.save_settings(cx);
                })
            })
            .into_any_element()
    }

    fn choose_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (w, h) = WIDE;
        v_flex()
            .id("bg-choose")
            .test_support()
            .w(px(w))
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(fluent::control(radius::BUTTON)))
                    .overflow_hidden()
                    .border_2()
                    .border_color(theme.border)
                    .border_dashed()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Plus).text_color(theme.muted_foreground)),
            )
            .child(
                div()
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground)
                    .child("Choose image…"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace
                    .update(cx, |ws, cx| ws.pick_background_image(cx))
            }))
            .into_any_element()
    }
}
