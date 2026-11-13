//! Trek themes in the app: a switch puts a theme's whole look up (the kit's colours and fonts,
//! Trek's accents, the corners) and its settings on, going back to Trek puts back what the user
//! had, the switch cross-fades over time with motion and at once without, and the themes folder
//! is read again with a bad file named and an agent's `apply` request followed.

mod common;

use common::{open, open_with, run};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{Hsla, TestAppContext, rgb};
use std::time::Duration;
use trek_core::settings::ThemeChoice;

/// The corner scale and the springs' pace are the process's: these tests take turns.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

fn look(cx: &mut TestAppContext) -> (bool, String, Hsla, Hsla) {
    cx.update(|cx| {
        let t = cx.theme();
        (
            t.mode.is_dark(),
            t.font_family.to_string(),
            t.background,
            trek_app::palette::ember(cx),
        )
    })
}

#[test]
fn a_theme_puts_up_its_whole_look_and_trek_puts_back_what_the_user_had() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.appearance.theme = ThemeChoice::Paper;
            s.appearance.transcript_font_size = 17.5;
        });
        cx.update(|cx| {
            trek_app::themes::init(&trek.ws, cx);
            trek_app::apply_theme(ThemeChoice::Paper, None, cx);
        });
        let (dark, font, _, ember) = look(cx);
        assert!(!dark);
        assert_eq!(font, "Inter");
        assert_eq!(ember, hex(0xE85D1F), "Trek's own ember on Paper");

        // Reduce motion (the harness's): at once.
        cx.update(|cx| trek_app::themes::switch_to("console", &trek.ws, cx));
        trek.render(cx);
        let a = trek.read(cx, |ws, _| ws.settings.appearance.clone());
        assert_eq!(a.trek_theme, "console");
        assert_eq!(a.theme, ThemeChoice::Night, "Console is dark");
        assert_eq!((a.ui_font_size, a.transcript_font_size), (13., 13.5));
        let (dark, font, background, ember) = look(cx);
        assert!(dark);
        assert_eq!(font, "JetBrains Mono NL");
        assert_eq!(background, hex(0x0B0D0C));
        assert_eq!(ember, hex(0x6BDB9A), "the working beacon takes the accent");
        assert_eq!(
            cx.update(|cx| trek_app::palette::on_send(cx)),
            hex(0x06140C)
        );
        assert_eq!(gpui_kit::corner_scale(), 0.25);
        assert_eq!(
            cx.update(|cx| trek_app::themes::conversation_font(cx)),
            None
        );

        // The user flips to Paper: the theme's own light colours.
        cx.update(|cx| trek_app::apply_theme(ThemeChoice::Paper, None, cx));
        assert_eq!(look(cx).2, hex(0xF6F6F1));

        // Journal sets the conversation apart from the interface.
        cx.update(|cx| trek_app::themes::switch_to("journal", &trek.ws, cx));
        assert_eq!(
            cx.update(|cx| trek_app::themes::conversation_font(cx))
                .map(|f| f.to_string()),
            Some("Spectral".into())
        );
        assert_eq!(look(cx).1, "Inter");

        // Back to Trek: as the user had it, Trek's own colours and corners.
        cx.update(|cx| trek_app::themes::switch_to("trek", &trek.ws, cx));
        trek.render(cx);
        let a = trek.read(cx, |ws, _| ws.settings.appearance.clone());
        assert_eq!(a.trek_theme, "trek");
        assert_eq!(a.theme, ThemeChoice::Paper);
        assert_eq!(a.transcript_font_size, 17.5);
        let (dark, font, background, ember) = look(cx);
        assert!(!dark);
        assert_eq!(font, "Inter");
        assert_eq!(background, hex(0xFFFFFF));
        assert_eq!(ember, hex(0xE85D1F));
        assert_eq!(gpui_kit::corner_scale(), 1.);
    });
}

#[test]
fn with_motion_the_switch_takes_its_time_and_changes_light_for_dark_at_the_middle() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = open_with(cx, |s| s.appearance.theme = ThemeChoice::Night);
        cx.update(|cx| {
            trek_app::themes::init(&trek.ws, cx);
            trek_app::apply_theme(ThemeChoice::Night, None, cx);
            cx.set_reduce_motion(false);
        });
        assert!(look(cx).0, "Night");
        cx.update(|cx| trek_app::themes::switch_to("journal", &trek.ws, cx));
        // A fifth of the way: still Night, the settings as they were.
        cx.executor().advance_clock(Duration::from_millis(110));
        cx.run_until_parked();
        assert!(look(cx).0, "still dark before the middle");
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone()),
            "trek"
        );
        // Past the middle: Journal's light and its settings, under the veil.
        cx.executor().advance_clock(Duration::from_millis(220));
        cx.run_until_parked();
        assert!(!look(cx).0, "light past the middle");
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone()),
            "journal"
        );
        // Done: Journal's own paper, the corners its own.
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        trek.render(cx);
        assert_eq!(look(cx).2, hex(0xFBF8F2));
        assert_eq!(gpui_kit::corner_scale(), 1.25);
        // A motion scale of 0 switches at once.
        trek.update(cx, |ws, cx| {
            ws.settings.appearance.motion_scale = 0.;
            ws.save_settings(cx);
        });
        cx.update(|cx| trek_app::themes::switch_to("trek", &trek.ws, cx));
        assert!(look(cx).0, "Trek again, Night, at once");
    });
}

#[test]
fn the_themes_folder_is_read_again_with_bad_files_named_and_apply_requests_followed() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    run(async |cx| {
        let trek = open(cx);
        cx.update(|cx| trek_app::themes::init(&trek.ws, cx));
        let dir = trek_core::themes::dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ninety-terminal.toml"),
            "name = \"Ninety Terminal\"\nmode = \"dark\"\n[fonts]\nui = \"mono\"\n[dark]\naccent = \"#FFB000\"\nwindow = \"#0A0805\"\ntext = \"#FFCC66\"\nmuted = \"#A3803F\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("broken.toml"),
            "name = \"B\"\n[fonts]\nui = \"comic\"\n",
        )
        .unwrap();
        std::fs::write(dir.join(trek_core::themes::APPLY_FILE), "ninety-terminal\n").unwrap();
        cx.update(|cx| trek_app::themes::reread(&trek.ws, cx));
        trek.render(cx);
        let ids: Vec<String> = cx.update(|cx| {
            trek_app::themes::list(cx)
                .into_iter()
                .map(|t| t.id)
                .collect()
        });
        assert_eq!(
            ids,
            ["trek", "console", "journal", "alpenglow", "ninety-terminal"]
        );
        // The request was followed and taken away.
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone()),
            "ninety-terminal"
        );
        assert!(!dir.join(trek_core::themes::APPLY_FILE).exists());
        assert_eq!(look(cx).2, hex(0x0A0805));
        // The bad file is named where an agent reads it, and in Settings.
        let problems = std::fs::read_to_string(dir.join(trek_core::themes::PROBLEMS_FILE)).unwrap();
        assert!(
            problems.starts_with("broken.toml: line 3: ") && problems.contains("comic"),
            "{problems}"
        );
        trek.update(cx, |ws, cx| {
            ws.navigate(
                trek_app::workspace::Route::Settings(trek_app::workspace::SettingsPage::Appearance),
                cx,
            )
        });
        trek.render(cx);
        assert!(trek.visible(cx, "trek-theme-ninety-terminal"));
        let shown: Vec<String> = cx.update(|cx| {
            trek_app::themes::problems(cx)
                .into_iter()
                .map(|p| p.message)
                .collect()
        });
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(shown[0].contains("comic"), "{shown:?}");
        // The theme in use, edited on disk: what the file changed goes on.
        std::fs::write(
            dir.join("ninety-terminal.toml"),
            "name = \"Ninety Terminal\"\nmode = \"dark\"\n[fonts]\nui = \"mono\"\n[shape]\ndensity = \"roomy\"\n[dark]\naccent = \"#FFB000\"\nwindow = \"#0A0805\"\ntext = \"#FFCC66\"\nmuted = \"#A3803F\"\n",
        )
        .unwrap();
        cx.update(|cx| trek_app::themes::reread(&trek.ws, cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.ui_font_size),
            15.
        );
        // Taken away while in use: Trek's own again.
        std::fs::remove_file(dir.join("ninety-terminal.toml")).unwrap();
        cx.update(|cx| trek_app::themes::reread(&trek.ws, cx));
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.appearance.trek_theme.clone()),
            "trek"
        );
        assert_eq!(look(cx).1, "Inter");
    });
}
