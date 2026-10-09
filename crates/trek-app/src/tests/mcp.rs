//! Settings › Tools › Your MCP servers: a pasted config adds its servers, their tokens go to the
//! Keychain, each is checked, and one can be edited and removed with its tokens.

use super::harness::{open, run};
use crate::workspace::{McpCheck, Route, SettingsPage};
use trek_agents::McpTransport;
use trek_core::settings::secrets;

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../trek-agents/fixtures/fake-mcp.pl");

#[test]
fn a_pasted_config_adds_its_servers_checks_them_and_edits_keep_their_tokens() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Tools), cx));
        // Tall enough for the servers without scrolling.
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)));
        trek.render(cx);
        // Nothing listens at this address: the remote server's check says so.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let config = format!(
            r#"{{"mcpServers": {{"fake": {{"command": "perl", "args": ["{FAKE}", "ok"], "env": {{"FAKE_TOKEN": "abc"}}}}, "remote": {{"url": "http://{free}/mcp", "headers": {{"Authorization": "Bearer t"}}}}}}}}"#
        );
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &config);
        trek.click(cx, "mcp-add");
        trek.render(cx);

        // Both are saved, with the names of their env vars and headers only.
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!(saved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["fake", "remote"]);
        assert_eq!((saved[0].env[0].name.as_str(), saved[0].env[0].value.as_str(), saved[0].env[0].secret), ("FAKE_TOKEN", "", true));
        assert_eq!((saved[1].headers[0].name.as_str(), saved[1].headers[0].secret), ("Authorization", true));
        assert!(!format!("{saved:?}").contains("abc") && !format!("{saved:?}").contains("Bearer"), "no token in the settings");
        // A session gets them back from the Keychain.
        let servers = trek.read(cx, |ws, _| ws.mcp_servers());
        let fake = servers.iter().find(|s| s.name == "fake").expect("fake is passed on");
        assert_eq!(fake.env(), [("FAKE_TOKEN".to_string(), "abc".to_string())]);
        let remote = servers.iter().find(|s| s.name == "remote").expect("remote is passed on");
        assert!(matches!(&remote.transport, McpTransport::Http { headers, .. } if headers == &[("Authorization".to_string(), "Bearer t".to_string())]));

        // Each was checked as it was added: the command started with its token, listing its
        // tools (env_ok only shows with FAKE_TOKEN set), and the remote one unreachable.
        trek.wait(cx, "both checks", |ws| ws.mcp_checks.values().all(|c| !matches!(c, McpCheck::Checking(_))) && ws.mcp_checks.len() == 2).await;
        let checks = trek.read(cx, |ws, _| ws.mcp_checks.clone());
        assert_eq!(checks["fake"], McpCheck::Works(vec!["echo".into(), "add".into(), "get_time".into(), "env_ok".into()]));
        assert!(matches!(&checks["remote"], McpCheck::Failed(why) if why.starts_with("Couldn't reach it")), "{checks:?}");
        trek.render(cx);
        assert!(trek.visible(cx, "mcp-status-0") && trek.visible(cx, "mcp-status-1"), "each row says how its check went");

        // The same again adds nothing.
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &config);
        trek.click(cx, "mcp-add");
        assert_eq!(trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.len()), 2, "duplicates are skipped");
        trek.click(cx, "mcp-command");
        trek.press(cx, "cmd-a");
        trek.press(cx, "backspace");

        // Edited: renamed, its token left as it was (shown as …), it keeps it under the new name.
        trek.click(cx, "mcp-edit-0");
        trek.render(cx);
        assert!(trek.visible(cx, "mcp-editing"));
        trek.click(cx, "mcp-name");
        trek.press(cx, "cmd-a");
        trek.type_text(cx, "fake2");
        trek.click(cx, "mcp-add");
        trek.render(cx);
        assert!(!trek.visible(cx, "mcp-editing"), "saved, the row is for adding again");
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!((saved.len(), saved[0].name.as_str(), saved[0].args.len()), (2, "fake2", 2));
        assert_eq!(saved[0].resolved_env(), [("FAKE_TOKEN".to_string(), "abc".to_string())]);
        assert_eq!(secrets::mcp_env("fake", "FAKE_TOKEN"), None, "nothing is left under the old name");
        trek.wait(cx, "the edited server's check", |ws| matches!(ws.mcp_checks.get("fake2"), Some(McpCheck::Works(_)))).await;

        // Removed, its tokens go too.
        trek.click(cx, "mcp-del-1");
        trek.click(cx, "mcp-del-0");
        assert!(trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.is_empty()));
        assert_eq!(secrets::mcp_env("fake2", "FAKE_TOKEN"), None);
        assert_eq!(secrets::mcp_header("remote", "Authorization"), None);
    });
}

#[test]
fn a_command_line_with_its_token_in_front_is_one_server() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Tools), cx));
        // Tall enough for the servers without scrolling.
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(2400.)));
        trek.render(cx);
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &format!("FAKE_TOKEN=abc perl '{FAKE}' ok"));
        trek.click(cx, "mcp-add");
        // No name typed: it's told from the script. Return adds it, as the button does.
        trek.press(cx, "enter");
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!((saved.len(), saved[0].name.as_str(), saved[0].command.as_str(), saved[0].args.clone()), (1, "fake", "perl", vec![FAKE.to_string(), "ok".into()]));
        assert_eq!(saved[0].env[0].name, "FAKE_TOKEN");
        trek.wait(cx, "the check", |ws| matches!(ws.mcp_checks.get("fake"), Some(McpCheck::Works(t)) if t.contains(&"env_ok".to_string()))).await;
        trek.click(cx, "mcp-del-0");
    });
}
