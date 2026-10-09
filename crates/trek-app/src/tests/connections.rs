//! Settings › Tools › Connections: Figma's desktop server reaches new sessions when it's on.

use super::harness::{open, run};
use crate::workspace::{Route, SettingsPage};
use trek_core::RunState;
use trek_core::settings::{FIGMA_DESKTOP_SERVER, FIGMA_DESKTOP_URL};

#[test]
fn the_figma_switch_gives_new_sessions_the_desktop_server() {
    run(async |cx| {
        let trek = open(cx);
        let desktop = format!("{FIGMA_DESKTOP_SERVER} (http: {FIGMA_DESKTOP_URL})");
        // Off until it's turned on: a session gets no Figma server.
        let before = trek.send(cx, "mock:mcp");
        trek.wait_done(cx, &before, RunState::Idle).await;
        assert!(!trek.answers(cx, &before).contains(FIGMA_DESKTOP_SERVER), "{}", trek.answers(cx, &before));

        // Turned on from the Tools page.
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Tools), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "figma-desktop-status"), "the desktop app's state is shown");
        trek.click(cx, "figma-desktop");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| ws.settings.tools.figma_desktop));

        // A new session has it, as a remote (HTTP) server, alongside Trek's own tools.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        trek.render(cx);
        let after = trek.send(cx, "mock:mcp");
        assert_ne!(after, before);
        trek.wait_done(cx, &after, RunState::Idle).await;
        let listed = trek.answers(cx, &after);
        assert!(listed.contains(&desktop), "{listed}");
        assert!(listed.contains("trek-orchestrate (stdio:"), "{listed}");
    });
}
