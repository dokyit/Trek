//! Installing an agent on a computer without Node.js: a line that needs no Node runs as it is; an
//! install that stopped because npm or node wasn't found is said plainly on the Agents step, with
//! one button that sets Node.js up; and Trek's own install of it failing says so too. A test
//! process never downloads, so the last is what the button leads to here.

mod common;

use common::{open, run, runs};
use trek_app::workspace::NodeSetup;

const PI: &str = "npm i -g @earendil-works/pi-coding-agent pi-acp";

#[test]
fn an_install_that_found_no_npm_offers_node_js_instead_of_the_shell_s_error() {
    run(async |cx| {
        let trek = open(cx);
        let ran = runs(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.open_onboarding(Some(trek_app::onboarding::Step::Agents), cx)
        });
        trek.render(cx);
        assert!(!trek.visible(cx, "node-setup"), "nothing to say yet");

        // Claude Code's line is its own installer: it runs as it is, needing no Node.
        let claude = trek_core::catalog::install_hint("claude-code")
            .unwrap()
            .here();
        assert!(!trek_core::node::needs_node(claude));
        trek.click(cx, "ob-install-claude-code");
        trek.render(cx);
        assert_eq!(ran.borrow().last().map(|(c, _)| c.as_str()), Some(claude));
        trek.update(cx, |ws, cx| {
            ws.setup_finished(claude, "zsh:1: command not found: npm", cx)
        });
        assert_eq!(trek.read(cx, |ws, _| ws.node_setup.clone()), None);

        // Pi's npm install ended with the shell unable to find npm.
        trek.update(cx, |ws, cx| {
            ws.setup_finished(
                PI,
                &format!(
                    "$ {PI}\n\nzsh:1: command not found: npm\n\n[finished with exit code 127]"
                ),
                cx,
            )
        });
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.node_setup.clone()),
            Some(NodeSetup::Missing { then: PI.into() })
        );
        assert!(trek.visible(cx, "node-setup"));
        assert!(trek.visible(cx, "node-install"), "one button that fixes it");

        // Put away, it's gone.
        trek.click(cx, "node-dismiss");
        trek.render(cx);
        assert!(!trek.visible(cx, "node-setup"));

        // Again, and the fix: Trek installs Node.js itself, which a test process turns down,
        // so the card says Trek couldn't, with the install to run once it can.
        trek.update(cx, |ws, cx| {
            ws.setup_finished(
                PI,
                "'npm' is not recognized as an internal or external command,",
                cx,
            )
        });
        trek.render(cx);
        trek.click(cx, "node-install");
        trek.wait(cx, "Node.js's install to be turned down", |ws| {
            matches!(ws.node_setup, Some(NodeSetup::Failed { .. }))
        })
        .await;
        trek.render(cx);
        let failed = trek.read(cx, |ws, _| ws.node_setup.clone()).unwrap();
        assert_eq!(failed.then(), Some(PI));
        assert_eq!(failed.agent().as_deref(), Some("Pi"));
        assert!(trek.visible(cx, "node-install"), "Try again");
        assert!(
            !ran.borrow().iter().any(|(c, _)| c == PI),
            "the install waits for Node.js"
        );
    });
}
