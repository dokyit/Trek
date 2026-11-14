//! The Mac's look for phones that match it (`trek_remote::MacLook`, carried by the snapshot): the
//! Trek theme Settings names, whole, and the background art and where it shows.

use crate::workspace::Workspace;
use gpui_kit::App;
use trek_core::settings::{Appearance, BackgroundPlacement};
use trek_core::themes::{Colour, Density, Font, Mode, Motion, Palette, Theme};
use trek_remote as tr;

impl Workspace {
    /// How this Mac looks now: the theme Settings names (Trek's own when it's gone, as the
    /// window shows it) and the art.
    pub(super) fn remote_look(&self, cx: &App) -> tr::MacLook {
        let a = &self.settings.appearance;
        let theme = crate::themes::find(&a.trek_theme, cx).unwrap_or_else(Theme::trek);
        look(&theme, a)
    }
}

/// `theme` and `a`'s art as a phone reads them.
pub fn look(theme: &Theme, a: &Appearance) -> tr::MacLook {
    tr::MacLook {
        theme: tr::LookTheme {
            id: theme.id.clone(),
            name: theme.name.clone(),
            description: theme.description.clone(),
            mode: match theme.mode {
                Mode::Dark => "dark",
                Mode::Light => "light",
                Mode::Any => "any",
            }
            .into(),
            ui_font: font(theme.ui_font).into(),
            conversation_font: font(theme.conversation_font).into(),
            ui_size: theme.ui_size,
            conversation_size: theme.conversation_size,
            corners: theme.corners,
            density: match theme.density {
                Density::Compact => "compact",
                Density::Regular => "regular",
                Density::Roomy => "roomy",
            }
            .into(),
            motion: match theme.motion {
                Motion::Still => "still",
                Motion::Brisk => "brisk",
                Motion::Standard => "standard",
                Motion::Calm => "calm",
            }
            .into(),
            dark: palette(&theme.dark),
            light: palette(&theme.light),
        },
        // Only Trek's own art: a picture of the user's own isn't on the phone.
        background: a
            .background
            .as_deref()
            .and_then(crate::backgrounds::piece_for)
            .map(|p| p.spec()),
        placement: match crate::backgrounds::placement(a) {
            BackgroundPlacement::Chat => "chat",
            BackgroundPlacement::Everywhere => "sidebar",
            BackgroundPlacement::NewThread => "new_thread",
        }
        .into(),
        dim: a.background_dim,
    }
}

fn font(f: Font) -> &'static str {
    match f {
        Font::Sans => "sans",
        Font::Mono => "mono",
        Font::Serif => "serif",
    }
}

fn palette(p: &Palette) -> tr::LookPalette {
    let hex = |c: Option<Colour>| c.map(Colour::hex);
    tr::LookPalette {
        accent: hex(p.accent),
        on_accent: hex(p.on_accent),
        window: hex(p.window),
        sidebar: hex(p.sidebar),
        surface: hex(p.surface),
        raised: hex(p.raised),
        text: hex(p.text),
        muted: hex(p.muted),
        border: hex(p.border),
    }
}

/// Trek's own themes, by id, for tests.
#[cfg(test)]
fn built_in(id: &str) -> Theme {
    trek_core::themes::built_in()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_theme_goes_whole() {
        let a = Appearance {
            background: Some("builtin:paintings/matterhorn".into()),
            background_placement: BackgroundPlacement::Chat,
            ..Appearance::default()
        };
        let l = look(&built_in("journal"), &a);
        assert_eq!(l.theme.name, "Journal");
        assert_eq!(l.theme.mode, "light");
        assert_eq!(
            (l.theme.ui_font.as_str(), l.theme.conversation_font.as_str()),
            ("sans", "serif")
        );
        assert_eq!(l.theme.conversation_size, 16.);
        assert_eq!(l.theme.density, "roomy");
        assert_eq!(l.theme.light.accent.as_deref(), Some("#B0552CFF"));
        assert_eq!(l.theme.dark.window.as_deref(), Some("#14110EFF"));
        assert_eq!(
            l.background.as_deref(),
            Some("builtin:paintings/matterhorn")
        );
        assert_eq!(l.placement, "chat");
        let console = look(&built_in("console"), &a);
        assert_eq!(console.theme.ui_font, "mono");
        assert_eq!(console.theme.corners, 0.25);
        assert_eq!(console.theme.motion, "brisk");
    }

    #[test]
    fn trek_sends_no_colours_and_the_users_own_picture_stays() {
        let a = Appearance {
            background: Some("/Users/me/Pictures/hills.png".into()),
            ..Appearance::default()
        };
        let l = look(&Theme::trek(), &a);
        assert_eq!(l.theme.id, trek_core::themes::DEFAULT);
        assert!(l.theme.dark.is_empty() && l.theme.light.is_empty());
        assert_eq!(l.background, None);
        let json = serde_json::to_value(&l).unwrap();
        assert!(json["theme"].get("dark").is_none(), "{json}");
    }

    #[test]
    fn ascii_art_goes_to_the_sidebar_and_old_names_are_canonical() {
        let mut a = Appearance {
            background: Some("builtin:ascii/braille".into()),
            background_placement: BackgroundPlacement::Chat,
            ..Appearance::default()
        };
        assert_eq!(look(&Theme::trek(), &a).placement, "sidebar");
        a.background = Some("builtin:dawn".into());
        a.background_placement = BackgroundPlacement::NewThread;
        let l = look(&Theme::trek(), &a);
        assert_eq!(
            (l.background.as_deref(), l.placement.as_str()),
            (Some("builtin:dawn"), "new_thread")
        );
        // A name this Mac doesn't know isn't sent.
        a.background = Some("builtin:paintings/elsewhere".into());
        assert_eq!(look(&Theme::trek(), &a).background, None);
    }
}
