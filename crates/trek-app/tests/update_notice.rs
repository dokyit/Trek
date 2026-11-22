//! Linux's .deb and tarball don't update themselves, but still say when a new version is out:
//! they check the same feed, never download, and the Updates page's button (which the sidebar's
//! notice opens too) goes to the release's page. `Blocker`s stand in for how Trek was installed.

mod common;

use common::{open_with, run};
use trek_app::updater::{UpdateAction, UpdateStatus};
use trek_app::workspace::{Route, SettingsPage};
use trek_core::update::{Artifact, AvailableUpdate, Blocker, Found};

fn release(version: &str) -> AvailableUpdate {
    AvailableUpdate {
        version: version.parse().unwrap(),
        notes: "- faster".into(),
        pub_date: String::new(),
        artifact: Artifact {
            // What the manifest says, which the notice never opens.
            url: "https://elsewhere.example/Trek.AppImage".into(),
            sha256: "s".into(),
            signature: "sig".into(),
        },
    }
}

#[test]
fn a_deb_install_says_a_new_version_is_out_and_opens_its_release_page() {
    run(async |cx| {
        // A feed nothing answers: the check starts, and that's all this needs to see.
        let trek = open_with(cx, |s| s.updates.auto_download = true);
        for blocker in [Blocker::Packaged, Blocker::NotAppImage] {
            trek.update(cx, |ws, cx| {
                ws.blocked = Some(Some(blocker));
                // Whatever an earlier check is still doing no longer counts.
                let _ = ws.updater.drop_update();
                ws.settings.updates.feed_url = "http://127.0.0.1:9/{channel}.json".into();
                ws.navigate(Route::Settings(SettingsPage::Updates), cx);
            });
            trek.render(cx);
            // Before any check: why it doesn't update itself, and Check now.
            let idle = trek.read(cx, |ws, app| ws.update_view(app));
            assert_eq!(
                (idle.line, idle.action),
                (blocker.message(), Some(UpdateAction::Check))
            );

            // It does check (a dev build or a read-only place wouldn't): still asking, or already
            // told nothing answers there.
            trek.click(cx, "update-action");
            let status = trek.read(cx, |ws, _| ws.updater.status.clone());
            assert!(
                match &status {
                    UpdateStatus::Checking => true,
                    UpdateStatus::Failed(e) => e.starts_with("Couldn't check for updates"),
                    _ => false,
                },
                "{blocker:?}: the check ran ({status:?})"
            );

            // What a check found: said, with the page to download it from, and not downloaded
            // even with automatic downloads on.
            trek.update(cx, |ws, cx| {
                let _ = ws.updater.drop_update();
                ws.settings.updates.feed_url = trek_core::update::OFFICIAL_FEED.into();
                let ticket = ws
                    .updater
                    .begin_check(ws.settings.updates.channel, 0)
                    .unwrap();
                let _ = ws.updater.finish_check(
                    ticket,
                    Ok(Found::Update(release("99.0.0"))),
                    false,
                    false,
                    0,
                );
                ws.download_update(cx);
            });
            trek.render(cx);
            assert_eq!(
                trek.read(cx, |ws, _| ws.updater.status.clone()),
                UpdateStatus::Available {
                    version: "99.0.0".into()
                },
                "{blocker:?}: nothing downloads"
            );
            let view = trek.read(cx, |ws, app| ws.update_view(app));
            assert_eq!(view.line, blocker.out_message("99.0.0"));
            assert_eq!(view.action, Some(UpdateAction::OpenDownloadPage));

            trek.click(cx, "update-action");
            assert_eq!(
                cx.opened_url().as_deref(),
                Some("https://github.com/dokyit/Trek/releases/tag/v99.0.0"),
                "{blocker:?}: the release's page, not the manifest's link"
            );
        }
    });
}

#[test]
fn a_copy_that_cant_update_for_other_reasons_still_doesnt_check() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.updates.feed_url = "http://127.0.0.1:9/{channel}.json".into();
        });
        trek.update(cx, |ws, cx| {
            ws.blocked = Some(Some(Blocker::DevBuild));
            ws.check_for_updates(true, cx);
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.updater.status.clone()),
            UpdateStatus::Idle
        );
    });
}
