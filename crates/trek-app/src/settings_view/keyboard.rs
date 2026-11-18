//! Settings › Keyboard Shortcuts, where every command's keys can be changed, and Settings ›
//! Appearance › Text size, where the interface's zoom and the conversation's text size have their
//! presets and a Custom size each.
//!
//! A shortcut is changed by clicking its keys and pressing new ones: Trek's own keys are off
//! while it listens (`keys::suspend`), so ⌘Q is recorded rather than quitting. Keys the system
//! or text fields need are refused (`keys::refusal`); keys another command has in the same place
//! are offered with the choice of taking them from it (`keys::conflicts`). What's saved is only
//! what differs from Trek's defaults (`keys::store`), for this platform.

use super::SettingsView;
use super::pages::key;
use crate::keys::{self, Chip, Command, Row};
use crate::scale::{radius, text};
use crate::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::{
    FONT_SIZE_RANGE, Settings, TRANSCRIPT_PRESETS, ZOOM_MAX, ZOOM_MIN, ZOOM_PRESETS, zoom_percent,
};

/// The Keyboard Shortcuts page's state: its search, the command listening for keys, and what
/// was pressed when it needs an answer (a conflict) or was refused.
pub(super) struct KeysPage {
    filter: Entity<InputState>,
    recording: Option<String>,
    /// Takes the keys while recording, ahead of everything else.
    _listening: Option<Subscription>,
    /// What has the focus while recording: no text field, so nothing is typed into one.
    focus: FocusHandle,
    offer: Option<Offer>,
}

/// Keys pressed for `name` that need an answer: refused (why), or another command's already.
struct Offer {
    name: String,
    keys: String,
    refused: Option<&'static str>,
    /// The commands that have the keys there now: their names and titles.
    taken_from: Vec<(String, String)>,
}

/// Appearance › Text size: each size's own field, the zoom's slider, and whether Custom is open
/// for a size that's one of the presets (chosen, then not changed yet).
pub(super) struct Sizes {
    zoom_input: Entity<InputState>,
    text_input: Entity<InputState>,
    zoom_slider: Entity<SliderState>,
    custom_zoom: bool,
    custom_text: bool,
    /// The sizes the fields were last given, so one changed elsewhere (⌘+) is shown.
    shown: Option<(f32, f32)>,
}

impl KeysPage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> (Self, Subscription) {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search shortcuts"));
        let sub = cx.observe(&filter, |_, _, cx| cx.notify());
        (
            Self {
                filter,
                recording: None,
                _listening: None,
                focus: cx.focus_handle(),
                offer: None,
            },
            sub,
        )
    }
}

impl Sizes {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> (Self, Vec<Subscription>) {
        let zoom_input = cx.new(|cx| InputState::new(window, cx).placeholder("100"));
        let text_input = cx.new(|cx| InputState::new(window, cx).placeholder("14.5"));
        let zoom_slider = cx.new(|_| {
            SliderState::new()
                .min(ZOOM_MIN * 100.)
                .max(ZOOM_MAX * 100.)
                .step(5.)
                .default_value(100.)
        });
        let mut subs = vec![];
        // A number typed is taken on Enter or when the field is left, kept in range.
        subs.push(cx.subscribe_in(
            &zoom_input,
            window,
            |this: &mut SettingsView, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let typed = input
                        .read(cx)
                        .value()
                        .trim()
                        .trim_end_matches('%')
                        .to_string();
                    match typed.parse::<f32>() {
                        Ok(percent) => crate::zoom::set(percent / 100., cx),
                        Err(_) => this.sizes.shown = None,
                    }
                    this.sync_sizes(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &text_input,
            window,
            |this: &mut SettingsView, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let typed = input
                        .read(cx)
                        .value()
                        .trim()
                        .trim_end_matches("pt")
                        .to_string();
                    if let Ok(size) = typed.trim().parse::<f32>() {
                        this.set_text_size(size, cx);
                    }
                    this.sizes.shown = None;
                    this.sync_sizes(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe(
            &zoom_slider,
            |_: &mut SettingsView, _, event: &SliderEvent, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
                if let SliderValue::Single(percent) = *v {
                    crate::zoom::set(percent / 100., cx);
                }
            },
        ));
        (
            Self {
                zoom_input,
                text_input,
                zoom_slider,
                custom_zoom: false,
                custom_text: false,
                shown: None,
            },
            subs,
        )
    }
}

/// Which of the four presets `value` is (`None`: a custom size).
fn preset_of(value: f32, presets: &[f32; 4]) -> Option<u8> {
    presets
        .iter()
        .position(|p| (p - value).abs() < 0.001)
        .map(|i| i as u8)
}

/// A conversation text size kept in its range, on half points.
fn clamp_text(size: f32) -> f32 {
    let (lo, hi) = (*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end());
    ((size * 2.).round() / 2.).clamp(lo, hi)
}

const CUSTOM: u8 = 4;

impl SettingsView {
    /// The size fields show the sizes as they are, when they changed elsewhere (⌘+, a theme).
    pub(super) fn sync_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let a = &self.workspace.read(cx).settings.appearance;
        let now = (a.zoom(), a.transcript_font_size());
        if self.sizes.shown == Some(now) {
            return;
        }
        self.sizes.shown = Some(now);
        let (zoom, size) = now;
        if !self
            .sizes
            .zoom_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            let v = format!("{:.0}", zoom * 100.);
            self.sizes
                .zoom_input
                .update(cx, |s, cx| s.set_value(v, window, cx));
        }
        if !self
            .sizes
            .text_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            let v = format!("{size}");
            self.sizes
                .text_input
                .update(cx, |s, cx| s.set_value(v, window, cx));
        }
        self.sizes
            .zoom_slider
            .update(cx, |s, cx| s.set_value(zoom * 100., window, cx));
    }

    fn set_text_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = clamp_text(size);
        self.workspace.update(cx, |ws, cx| {
            ws.settings.appearance.transcript_font_size = size;
            ws.save_settings(cx);
        });
    }

    /// Appearance › Text size: the interface's zoom and the conversation's text, each with its
    /// four presets and Custom… after Larger, which opens a field (and for the zoom, a slider)
    /// for any size in range.
    pub(super) fn text_size_section(
        &mut self,
        s: &Settings,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let zoom = s.appearance.zoom();
        let size = s.appearance.transcript_font_size();
        let zoom_pick = match preset_of(zoom, &ZOOM_PRESETS) {
            Some(i) if !self.sizes.custom_zoom => i,
            _ => CUSTOM,
        };
        let text_pick = match preset_of(size, &TRANSCRIPT_PRESETS) {
            Some(i) if !self.sizes.custom_text => i,
            _ => CUSTOM,
        };
        let view = cx.entity().downgrade();
        let zoom_choice = {
            let view = view.clone();
            move |v: u8, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| {
                    this.sizes.custom_zoom = v == CUSTOM;
                    if let Some(z) = ZOOM_PRESETS.get(v as usize) {
                        crate::zoom::set(*z, cx);
                    }
                    cx.notify();
                });
            }
        };
        let text_choice = move |v: u8, _: &mut Window, cx: &mut App| {
            let _ = view.update(cx, |this, cx| {
                this.sizes.custom_text = v == CUSTOM;
                if let Some(p) = TRANSCRIPT_PRESETS.get(v as usize) {
                    this.set_text_size(*p, cx);
                }
                cx.notify();
            });
        };
        let mut rows = vec![Self::row(
            "Interface",
            zoom_note(),
            ui::segmented(
                "zoom-size",
                vec![
                    (0u8, "Small"),
                    (1, "Default"),
                    (2, "Large"),
                    (3, "Larger"),
                    (CUSTOM, "Custom…"),
                ],
                zoom_pick,
                zoom_choice,
                cx,
            ),
            cx,
        )];
        if zoom_pick == CUSTOM {
            rows.push(self.custom_zoom_row(zoom, cx));
        }
        rows.push(Self::row(
            "Conversation text",
            "Messages from you and your agents.",
            ui::segmented(
                "text-size",
                vec![
                    (0u8, "Small"),
                    (1, "Default"),
                    (2, "Large"),
                    (3, "Larger"),
                    (CUSTOM, "Custom…"),
                ],
                text_pick,
                text_choice,
                cx,
            ),
            cx,
        ));
        if text_pick == CUSTOM {
            rows.push(self.custom_text_row(size, cx));
        }
        vec![Self::heading("Text size", cx), ui::group(rows, cx)]
    }

    /// − field + and a slider: the zoom in steps of 5%, from 50% to 300%.
    fn custom_zoom_row(&self, zoom: f32, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let nudge = |id: &'static str, by: f32| {
            Button::new(id)
                .small()
                .outline()
                .icon(if by > 0. {
                    IconName::Plus
                } else {
                    IconName::Minus
                })
                .on_click(move |_, _, cx| {
                    let z = crate::zoom::current(cx);
                    crate::zoom::set(((z * 100. + by) / 5.).round() * 5. / 100., cx)
                })
        };
        Self::row(
            "Custom interface size",
            format!(
                "Any size from {} to {}.",
                zoom_percent(ZOOM_MIN),
                zoom_percent(ZOOM_MAX)
            ),
            h_flex()
                .id("zoom-custom")
                .test_support()
                .gap(px(8.))
                .items_center()
                .child(
                    div()
                        .w(px(140.))
                        .child(Slider::new(&self.sizes.zoom_slider)),
                )
                .child(nudge("zoom-custom-less", -5.).disabled(zoom <= ZOOM_MIN))
                .child(
                    div()
                        .w(px(64.))
                        .child(Input::new(&self.sizes.zoom_input).small()),
                )
                .child(div().text_size(px(text::BODY)).text_color(muted).child("%"))
                .child(nudge("zoom-custom-more", 5.).disabled(zoom >= ZOOM_MAX)),
            cx,
        )
    }

    /// − field +: the conversation's text in half points, from 9 to 32.
    fn custom_text_row(&self, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (lo, hi) = (*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end());
        let view = cx.entity().downgrade();
        let nudge = move |id: &'static str, by: f32| {
            let view = view.clone();
            Button::new(id)
                .small()
                .outline()
                .icon(if by > 0. {
                    IconName::Plus
                } else {
                    IconName::Minus
                })
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        let now = this
                            .workspace
                            .read(cx)
                            .settings
                            .appearance
                            .transcript_font_size();
                        this.set_text_size(now + by, cx);
                    });
                })
        };
        Self::row(
            "Custom conversation text",
            format!("Any size from {lo} to {hi} points."),
            h_flex()
                .id("text-custom")
                .test_support()
                .gap(px(8.))
                .items_center()
                .child(nudge("text-custom-less", -0.5).disabled(size <= lo))
                .child(
                    div()
                        .w(px(64.))
                        .child(Input::new(&self.sizes.text_input).small()),
                )
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(muted)
                        .child("pt"),
                )
                .child(nudge("text-custom-more", 0.5).disabled(size >= hi)),
            cx,
        )
    }

    /// The page's rows as the user has them now.
    fn key_page(&self, cx: &App) -> Vec<(String, Vec<Row>)> {
        let send_cmd = self.workspace.read(cx).settings.general.send_with_cmd_enter;
        keys::page(keys::is_mac(), send_cmd, &crate::plugins::key_commands(cx))
    }

    fn start_recording(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.keys.offer = None;
        self.keys.recording = Some(name);
        window.focus(&self.keys.focus, cx);
        keys::suspend(true, cx);
        let view = cx.entity().downgrade();
        self.keys._listening = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let keystroke = event.keystroke.clone();
            let mut took = false;
            view.update(cx, |this, cx| took = this.pressed(&keystroke, cx));
            if took {
                cx.stop_propagation();
            }
        }));
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.keys.recording = None;
        self.keys._listening = None;
        keys::suspend(false, cx);
        cx.notify();
    }

    /// A key pressed while recording: Escape alone stops, a modifier alone waits for the key.
    /// Whether it was the recording's.
    fn pressed(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(name) = self.keys.recording.clone() else {
            return false;
        };
        let m = keystroke.modifiers;
        if keystroke.key == "escape" && !(m.control || m.alt || m.shift || m.platform) {
            self.stop_recording(cx);
            return true;
        }
        let Some(keys) = keys::recorded(keystroke) else {
            return true;
        };
        self.stop_recording(cx);
        self.offer_keys(name, keys, cx);
        true
    }

    /// `keys` for `name`: given it, unless refused or another command's (then asked).
    fn offer_keys(&mut self, name: String, keys: String, cx: &mut Context<Self>) {
        let mac = keys::is_mac();
        let page = self.key_page(cx);
        let all = keys::commands(&page);
        let Some(me) = all.iter().find(|c| c.name == name) else {
            return;
        };
        if let Some(why) = keys::refusal(&keys, mac, me.plain_ok) {
            self.keys.offer = Some(Offer {
                name,
                keys,
                refused: Some(why),
                taken_from: vec![],
            });
            cx.notify();
            return;
        }
        let taken: Vec<(String, String)> = keys::conflicts(&all, &name, &keys, mac)
            .into_iter()
            .map(|c| (c.name.clone(), c.title.clone()))
            .collect();
        if taken.is_empty() {
            self.set_keys(&name, Some(&keys), cx);
        } else {
            self.keys.offer = Some(Offer {
                name,
                keys,
                refused: None,
                taken_from: taken,
            });
        }
        cx.notify();
    }

    /// Give `name` these keys (`None`: none), saved as a difference from its default.
    fn set_keys(&mut self, name: &str, new: Option<&str>, cx: &mut Context<Self>) {
        let mac = keys::is_mac();
        let page = self.key_page(cx);
        let Some(command) = keys::commands(&page)
            .into_iter()
            .find(|c| c.name == name)
            .cloned()
        else {
            return;
        };
        self.workspace.update(cx, |ws, cx| {
            keys::store(&mut ws.settings.keys, mac, &command, new);
            ws.save_settings(cx);
        });
    }

    /// The offer's keys to its command, taken from the ones that had them (they're left with
    /// none).
    fn take_offered(&mut self, cx: &mut Context<Self>) {
        let Some(offer) = self.keys.offer.take() else {
            return;
        };
        for (other, _) in &offer.taken_from {
            self.set_keys(other, None, cx);
        }
        self.set_keys(&offer.name, Some(&offer.keys), cx);
        cx.notify();
    }

    fn reset_keys(&mut self, name: Option<&str>, cx: &mut Context<Self>) {
        let mac = keys::is_mac();
        self.keys.offer = None;
        self.workspace.update(cx, |ws, cx| {
            let changes = ws.settings.keys.for_platform_mut(mac);
            match name {
                Some(name) => {
                    changes.remove(name);
                }
                None => changes.clear(),
            }
            ws.save_settings(cx);
        });
    }

    pub(super) fn shortcuts_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mac = keys::is_mac();
        let theme = cx.theme().clone();
        let page = self.key_page(cx);
        let query = self.keys.filter.read(cx).value().trim().to_lowercase();
        let any_changed = !self
            .workspace
            .read(cx)
            .settings
            .keys
            .for_platform(mac)
            .is_empty();
        let mut out = vec![
            Self::note(
                &keys::localize(
                    "Click a shortcut, then press the keys you want it on. Esc stops listening. Only what you change is kept, so Trek's other shortcuts stay its own, new ones included.",
                ),
                cx,
            ),
            h_flex()
                .gap(px(10.))
                .pb(px(6.))
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(&self.keys.filter).small().cleanable(true)),
                )
                .when(any_changed, |el| {
                    el.child(
                        Button::new("shortcuts-reset-all")
                            .small()
                            .outline()
                            .label("Reset all")
                            .on_click(cx.listener(|this, _, _, cx| this.reset_keys(None, cx))),
                    )
                })
                .into_any_element(),
        ];
        let label_of = |c: &Command| c.keys.as_deref().map(|k| keys::label_for(k, mac));
        for (section, rows) in page {
            let rows: Vec<Row> = rows
                .into_iter()
                .filter(|r| {
                    if query.is_empty() {
                        return true;
                    }
                    let keys_text = match r {
                        Row::Command(c) => label_of(c).unwrap_or_default(),
                        Row::Fixed(..) => String::new(),
                    };
                    r.title().to_lowercase().contains(&query)
                        || section.to_lowercase().contains(&query)
                        || keys_text.to_lowercase().contains(&query)
                        || matches!(r, Row::Command(c) if c.section.to_lowercase().contains(&query))
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            out.push(Self::heading(&section, cx));
            let mut drawn = vec![];
            for row in rows {
                drawn.push(match row {
                    Row::Fixed(title, chips) => self.fixed_row(title, chips, cx),
                    Row::Command(c) => self.command_row(&c, mac, &theme, cx),
                });
            }
            out.push(ui::group(drawn, cx));
        }
        out.push(div().track_focus(&self.keys.focus).into_any_element());
        out
    }

    fn fixed_row(&self, title: &str, chips: Vec<Chip>, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .id(SharedString::from(format!("shortcut-{title}")))
            .test_support()
            .h(px(42.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(text::BODY))
                    .child(title.to_string()),
            )
            .child(chips_row(chips, cx))
            .into_any_element()
    }

    fn command_row(
        &self,
        c: &Command,
        mac: bool,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let recording = self.keys.recording.as_deref() == Some(c.name.as_str());
        let offer = self.keys.offer.as_ref().filter(|o| o.name == c.name);
        let name = c.name.clone();
        let keys_button = div()
            .id(SharedString::from(format!("shortcut-keys-{}", c.name)))
            .test_support()
            .min_w(px(96.))
            .h(px(30.))
            .px(px(6.))
            .flex()
            .items_center()
            .justify_end()
            .rounded(px(crate::fluent::control(radius::CONTROL)))
            .cursor_pointer()
            .hover(|s| s.bg(theme.foreground.opacity(0.05)))
            .when(recording, |el| {
                el.border_1()
                    .border_color(theme.ring)
                    .bg(theme.ring.opacity(0.08))
            })
            .child(if recording {
                div()
                    .text_size(px(text::LABEL))
                    .text_color(theme.foreground)
                    .child("Press keys…")
                    .into_any_element()
            } else if c.keys.is_none() {
                div()
                    .text_size(px(text::LABEL))
                    .text_color(theme.muted_foreground)
                    .child("None")
                    .into_any_element()
            } else {
                chips_row(c.chips(mac), cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.keys.recording.as_deref() == Some(name.as_str()) {
                    this.stop_recording(cx);
                } else {
                    this.start_recording(name.clone(), window, cx);
                }
            }));
        let taken_by_trek = c.plugin
            && c.keys.is_some()
            && c.name
                .strip_prefix("plugin:")
                .and_then(|r| r.split_once('/'))
                .is_some_and(|(p, command)| {
                    crate::plugins::key_taken(p, command, c.keys.as_deref(), cx)
                });
        let name = c.name.clone();
        let line = h_flex()
            .id(SharedString::from(format!("shortcut-{}", c.name)))
            .test_support()
            .min_h(px(42.))
            .gap(px(8.))
            .child(
                v_flex()
                    .flex_1()
                    .child(div().text_size(px(text::BODY)).child(c.title.clone()))
                    .when(c.changed() || taken_by_trek, |el| {
                        let default = c
                            .default
                            .as_deref()
                            .map(|d| keys::label_for(d, mac))
                            .unwrap_or_else(|| "none".into());
                        el.child(
                            div()
                                .text_size(px(text::META))
                                .text_color(theme.muted_foreground)
                                .child(if taken_by_trek {
                                    "One of Trek's shortcuts has these keys: it's in the palette without them.".to_string()
                                } else {
                                    format!("Changed from {default}")
                                }),
                        )
                    }),
            )
            .when(recording, |el| {
                el.child(
                    Button::new("shortcut-clear")
                        .small()
                        .ghost()
                        .label("No shortcut")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(name) = this.keys.recording.clone() {
                                this.stop_recording(cx);
                                this.set_keys(&name, None, cx);
                            }
                        })),
                )
            })
            .when(c.changed() && !recording, |el| {
                el.child(
                    ui::icon_button(
                        SharedString::from(format!("shortcut-reset-{}", c.name)),
                        IconName::Undo2,
                        "Back to the default",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reset_keys(Some(&name), cx)
                    })),
                )
            })
            .child(keys_button);
        let Some(offer) = offer else {
            return line.into_any_element();
        };
        let label = keys::label_for(&offer.keys, mac);
        let note = match (offer.refused, offer.taken_from.as_slice()) {
            (Some(why), _) => format!("{label} can't be a shortcut. {why}"),
            (None, [(_, title)]) => format!("{label} is already {title}'s."),
            (None, many) => format!(
                "{label} is already {}'s.",
                many.iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join("'s and ")
            ),
        };
        let refused = offer.refused.is_some();
        v_flex()
            .child(line)
            .child(
                h_flex()
                    .id("shortcut-offer")
                    .test_support()
                    .gap(px(8.))
                    .pb(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::LABEL))
                            .text_color(if refused {
                                theme.danger
                            } else {
                                theme.foreground
                            })
                            .child(note),
                    )
                    .when(!refused, |el| {
                        el.child(
                            Button::new("shortcut-take")
                                .small()
                                .primary()
                                .label("Use it here")
                                .on_click(cx.listener(|this, _, _, cx| this.take_offered(cx))),
                        )
                    })
                    .child(
                        Button::new("shortcut-cancel")
                            .small()
                            .ghost()
                            .label(if refused { "OK" } else { "Cancel" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.keys.offer = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// The Interface row's note, with the zoom's keys as the user has them.
fn zoom_note() -> String {
    let hint = |id| {
        let h = keys::hint(id);
        if h.is_empty() {
            "no keys".to_string()
        } else {
            h
        }
    };
    format!(
        "Everything in Trek's windows: text, controls and the room between them. {} and {} step through the sizes and on past them; {} is the default again.",
        hint(keys::Id::ZoomIn),
        hint(keys::Id::ZoomOut),
        hint(keys::Id::ResetZoom)
    )
}

fn chips_row(chips: Vec<Chip>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap(px(4.))
        .children(chips.into_iter().map(|chip| {
            match chip {
                Chip::Key(k) => key(&k, cx),
                Chip::Word(w) => div()
                    .text_size(px(text::LABEL))
                    .text_color(muted)
                    .child(w)
                    .into_any_element(),
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_presets_or_custom_and_kept_in_range() {
        assert_eq!(preset_of(1.0, &ZOOM_PRESETS), Some(1));
        assert_eq!(preset_of(1.25, &ZOOM_PRESETS), Some(3));
        assert_eq!(preset_of(1.5, &ZOOM_PRESETS), None);
        assert_eq!(preset_of(17.5, &TRANSCRIPT_PRESETS), Some(3));
        assert_eq!(clamp_text(100.), 32.);
        assert_eq!(clamp_text(2.), 9.);
        assert_eq!(clamp_text(15.3), 15.5);
        assert_eq!(trek_core::settings::clamp_zoom(9.), ZOOM_MAX);
    }
}
