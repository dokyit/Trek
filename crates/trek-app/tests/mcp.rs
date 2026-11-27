//! Settings › Tools › Your MCP servers: a pasted config adds its servers, their tokens go to the
//! Keychain, each is checked, and one can be edited and removed with its tokens.

mod common;

use common::{open, run};
use trek_agents::McpTransport;
use trek_app::workspace::{McpCheck, Route, SettingsPage};
use trek_core::settings::secrets;

// A test's servers have names of its own: in an isolated process the tokens are kept in memory
// (`secrets`), one store for every test running beside each other, so two tests adding, editing
// and removing "fake" took each other's token away.

/// The `fixture` binary; `fixture fake-mcp ok` is the stand-in MCP server.
fn fake() -> String {
    trek_test_fixtures::bin("fixture").display().to_string()
}

#[test]
fn a_pasted_config_adds_its_servers_checks_them_and_edits_keep_their_tokens() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        // Tall enough for the servers without scrolling (the page above them grew by the Mac's
        // Passkeys section).
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(4400.)),
        );
        trek.render(cx);
        // Nothing listens at this address: the remote server's check says so.
        let free = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let config = serde_json::json!({
            "mcpServers": {
                "pasted": { "command": fake(), "args": ["fake-mcp", "ok"], "env": { "FAKE_TOKEN": "abc" } },
                "remote": { "url": format!("http://{free}/mcp"), "headers": { "Authorization": "Bearer t" } },
            }
        })
        .to_string();
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &config);
        trek.click(cx, "mcp-add");
        trek.render(cx);

        // Both are saved, with the names of their env vars and headers only.
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!(
            saved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["pasted", "remote"]
        );
        assert_eq!(
            (
                saved[0].env[0].name.as_str(),
                saved[0].env[0].value.as_str(),
                saved[0].env[0].secret
            ),
            ("FAKE_TOKEN", "", true)
        );
        assert_eq!(
            (
                saved[1].headers[0].name.as_str(),
                saved[1].headers[0].secret
            ),
            ("Authorization", true)
        );
        // The values and arguments only: the fixture's path is the checkout's, which may hold
        // the letters "abc" (a worktree named agent-ababc…).
        let kept: Vec<&str> = saved
            .iter()
            .flat_map(|s| {
                s.env
                    .iter()
                    .map(|e| e.value.as_str())
                    .chain(s.headers.iter().map(|h| h.value.as_str()))
                    .chain(s.args.iter().map(String::as_str))
            })
            .collect();
        assert!(
            !kept
                .iter()
                .any(|v| v.contains("abc") || v.contains("Bearer")),
            "no token in the settings: {kept:?}"
        );
        // A session gets them back from the Keychain.
        let servers = trek.read(cx, |ws, _| ws.mcp_servers());
        let pasted = servers
            .iter()
            .find(|s| s.name == "pasted")
            .expect("pasted is passed on");
        assert_eq!(
            pasted.env(),
            [("FAKE_TOKEN".to_string(), "abc".to_string())]
        );
        let remote = servers
            .iter()
            .find(|s| s.name == "remote")
            .expect("remote is passed on");
        assert!(
            matches!(&remote.transport, McpTransport::Http { headers, .. } if headers == &[("Authorization".to_string(), "Bearer t".to_string())])
        );

        // Each was checked as it was added: the command started with its token, listing its
        // tools (env_ok only shows with FAKE_TOKEN set), and the remote one unreachable.
        trek.wait(cx, "both checks", |ws| {
            ws.mcp_checks
                .values()
                .all(|c| !matches!(c, McpCheck::Checking(_)))
                && ws.mcp_checks.len() == 2
        })
        .await;
        let checks = trek.read(cx, |ws, _| ws.mcp_checks.clone());
        assert_eq!(
            checks["pasted"],
            McpCheck::Works(vec![
                "echo".into(),
                "add".into(),
                "get_time".into(),
                "env_ok".into()
            ])
        );
        assert!(
            matches!(&checks["remote"], McpCheck::Failed(why) if why.starts_with("Couldn't reach it")),
            "{checks:?}"
        );
        trek.render(cx);
        assert!(
            trek.visible(cx, "mcp-status-0") && trek.visible(cx, "mcp-status-1"),
            "each row says how its check went"
        );

        // The same again adds nothing.
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &config);
        trek.click(cx, "mcp-add");
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.len()),
            2,
            "duplicates are skipped"
        );
        trek.click(cx, "mcp-command");
        trek.press(cx, "secondary-a");
        trek.press(cx, "backspace");

        // Edited: renamed, its token left as it was (shown as …), it keeps it under the new name.
        trek.click(cx, "mcp-edit-0");
        trek.render(cx);
        assert!(trek.visible(cx, "mcp-editing"));
        trek.click(cx, "mcp-name");
        trek.press(cx, "secondary-a");
        trek.type_text(cx, "pasted2");
        trek.click(cx, "mcp-add");
        trek.render(cx);
        assert!(
            !trek.visible(cx, "mcp-editing"),
            "saved, the row is for adding again"
        );
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!(
            (saved.len(), saved[0].name.as_str(), saved[0].args.len()),
            (2, "pasted2", 2)
        );
        assert_eq!(
            saved[0].resolved_env(),
            [("FAKE_TOKEN".to_string(), "abc".to_string())]
        );
        assert_eq!(
            secrets::mcp_env("pasted", "FAKE_TOKEN"),
            None,
            "nothing is left under the old name"
        );
        trek.wait(cx, "the edited server's check", |ws| {
            matches!(ws.mcp_checks.get("pasted2"), Some(McpCheck::Works(_)))
        })
        .await;

        // Removed, its tokens go too.
        trek.click(cx, "mcp-del-1");
        trek.click(cx, "mcp-del-0");
        assert!(trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.is_empty()));
        assert_eq!(secrets::mcp_env("pasted2", "FAKE_TOKEN"), None);
        assert_eq!(secrets::mcp_header("remote", "Authorization"), None);
    });
}

#[test]
fn a_command_line_with_its_token_in_front_is_one_server() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        // Tall enough for the servers without scrolling.
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(4400.)),
        );
        trek.render(cx);
        trek.click(cx, "mcp-command");
        trek.type_text(cx, &format!("FAKE_TOKEN=abc '{}' fake-mcp ok", fake()));
        trek.click(cx, "mcp-add");
        // No name typed: it's told from the command's own name. Return adds it, as the button does.
        trek.press(cx, "enter");
        let saved = trek.read(cx, |ws, _| ws.settings.tools.mcp_servers.clone());
        assert_eq!(
            (
                saved.len(),
                saved[0].name.as_str(),
                saved[0].command.as_str(),
                saved[0].args.clone()
            ),
            (
                1,
                "fake",
                fake().as_str(),
                vec!["fake-mcp".to_string(), "ok".into()]
            )
        );
        assert_eq!(saved[0].env[0].name, "FAKE_TOKEN");
        trek.wait(cx, "the check", |ws| matches!(ws.mcp_checks.get("fake"), Some(McpCheck::Works(t)) if t.contains(&"env_ok".to_string()))).await;
        trek.click(cx, "mcp-del-0");
    });
}

#[test]
fn a_token_an_earlier_copy_saved_is_named_as_locked_and_sent_once_allowed() {
    run(async |cx| {
        let name = "locked-earlier";
        // Its token was saved by "another build": reading it would need the user's password.
        trek_core::keychain::plant_locked(&secrets::mcp_env_account(name, "FAKE_TOKEN"), "abc");
        let trek = common::open_with(cx, |s| {
            let mut server = trek_core::settings::McpServerConfig::stdio(
                name,
                fake(),
                vec!["fake-mcp".into(), "ok".into()],
            );
            server.env.push(trek_core::settings::McpEnvVar {
                name: "FAKE_TOKEN".into(),
                value: String::new(),
                secret: true,
            });
            s.tools.mcp_servers = vec![server];
        });
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Tools), cx)
        });
        cx.simulate_window_resize(
            trek.window,
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(4400.)),
        );
        trek.render(cx);
        trek.render(cx);
        // The page's quiet read found it locked, and says so (no prompt, nothing sent).
        assert!(trek.visible(cx, "mcp-locked"), "the locked token is named");
        let servers = trek.read(cx, |ws, _| ws.mcp_servers());
        let mine = servers.iter().find(|s| s.name == name).expect("passed on");
        assert!(
            matches!(&mine.transport, McpTransport::Stdio { env, .. } if env.is_empty()),
            "a session starts it without the locked token"
        );
        // Checking it explains instead of failing on a missing token.
        trek.click(cx, "mcp-check-0");
        trek.wait(cx, "the check", |ws| {
            matches!(ws.mcp_checks.get(name), Some(McpCheck::Failed(why)) if why.contains("locked"))
        })
        .await;
        // Allow access (the isolated store stands for the user allowing it at the prompt).
        trek.click(cx, "mcp-locked-allow");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while trek.visible(cx, "mcp-locked") {
            assert!(std::time::Instant::now() < deadline, "never unlocked");
            cx.background_executor
                .timer(std::time::Duration::from_millis(5))
                .await;
            trek.render(cx);
        }
        trek.click(cx, "mcp-check-0");
        trek.wait(cx, "the check with its token", |ws| matches!(ws.mcp_checks.get(name), Some(McpCheck::Works(t)) if t.contains(&"env_ok".to_string()))).await;
    });
}

/// A server of the user's (pasted, or written into the settings by an agent) can call itself what
/// Trek's background computer use is called and set its mode: it still never gets the session's
/// channel (`TREK_IPC_*`), with Trek's own computer server there or not, and under that name
/// it isn't handed to a session at all.
fn a_posing_server_gets_no_channel(computer_use: bool) {
    use trek_core::settings::{McpEnvVar, McpServerConfig};
    let posing = |name: &str| {
        let mut server = McpServerConfig::stdio(name, "posing-as-trek", vec![]);
        server.env = vec![
            McpEnvVar {
                name: trek_ipc::ENV_COMPUTER_MODE.into(),
                value: trek_ipc::COMPUTER_BACKGROUND.into(),
                secret: false,
            },
            // Asking for the token by name, in a case Windows takes as the same variable.
            McpEnvVar {
                name: "trek_ipc_token".into(),
                value: "x".into(),
                secret: false,
            },
        ];
        server
    };
    {
        run(async move |cx| {
            let trek = common::open_with(cx, |s| {
                s.tools.computer_use = computer_use;
                s.tools.computer_background = true;
                s.tools.mcp_servers = vec![
                    posing("trek-computer"),
                    posing("Trek-Computer"),
                    posing("trek-orchestrate"),
                    posing("mine"),
                ];
            });
            let mock = trek_core::AgentId::Direct(trek_core::catalog::MOCK_PROVIDER.into());
            let (servers, key) = trek.update(cx, |ws, _| ws.session_mcp(&mock, None));
            assert!(key.is_some(), "the session has a channel to keep");
            let theirs: Vec<_> = servers
                .iter()
                .filter(|s| {
                    matches!(&s.transport, McpTransport::Stdio { command, .. } if command == "posing-as-trek")
                })
                .collect();
            assert_eq!(
                theirs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                ["mine"],
                "one named as Trek's own is left out"
            );
            for server in theirs {
                let env = server.env();
                assert!(
                    !env.iter()
                        .any(|(name, _)| name.to_ascii_uppercase().starts_with("TREK_IPC_")),
                    "{} got Trek's channel: {env:?}",
                    server.name
                );
            }
            // Trek's own orchestration server still has it.
            let own = servers
                .iter()
                .find(|s| s.name == "trek-orchestrate")
                .expect("the session's own tools");
            assert!(
                own.env()
                    .iter()
                    .any(|(name, _)| name == trek_ipc::ENV_TOKEN)
            );
        });
    }
}

#[test]
fn a_users_server_named_as_treks_own_never_gets_the_sessions_channel() {
    a_posing_server_gets_no_channel(false);
}

#[test]
fn nor_with_treks_own_computer_server_there() {
    a_posing_server_gets_no_channel(true);
}
