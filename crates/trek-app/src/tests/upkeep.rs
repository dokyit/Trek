//! The main thread stays free and state stays bounded: settings pages read the Mac off the main
//! thread, a thread's history loads in the background, a transcript view doesn't pile up
//! subscriptions, and onboarding always says where each agent stands.

use super::harness::{Trek, launch, mock, open, run, store_items, transcript};
use crate::workspace::{Route, SettingsPage};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use std::path::PathBuf;
use std::time::Duration;
use trek_core::detect::{Availability, DetectedAgent};
use trek_core::settings::Settings;
use trek_core::store::{Item, Store};
use trek_core::{AgentId, HandHolding, RunState};

fn detected(agent: AgentId, availability: Availability) -> DetectedAgent {
    DetectedAgent { name: agent.display_name(), agent, path: None, version: Some("1.2.3".into()), availability, models: vec![], install_hint: None }
}

#[test]
fn onboarding_lists_every_agent_with_the_command_that_sets_it_up() {
    run(async |cx| {
        let mut s = Settings::default();
        s.general.default_agent = mock().key();
        s.updates.auto_check = false;
        s.notifications.menu_bar_icon = false;
        let (ws, root, window) = launch(cx, Store::in_memory().expect("store"), s);
        let trek = Trek { ws, root, window, project: PathBuf::new() };
        trek.render(cx);
        std::thread::sleep(Duration::from_millis(220));
        trek.click(cx, "ob-next");
        trek.render(cx);
        // Nothing detected (an isolated process doesn't look): every agent Trek knows is listed as
        // not installed, the first few with how to install them, never a blank page.
        assert!(trek.read(cx, |ws, _| ws.agents.iter().all(|a| matches!(a.agent, AgentId::Direct(_)))));
        for id in ["claude-code", "codex", "opencode", "droid"] {
            assert!(trek.visible(cx, format!("ob-agent-{id}")), "{id} is listed");
            assert!(trek.visible(cx, format!("ob-copy-{id}")), "{id} has its install command");
        }
        assert!(!trek.visible(cx, "ob-agent-acp-cursor"), "the rest wait behind Show more");
        trek.click(cx, "ob-agents-more");
        trek.render(cx);
        assert!(trek.visible(cx, "ob-agent-acp-cursor"));
        trek.click(cx, "ob-copy-claude-code");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("npm i -g @anthropic-ai/claude-code"));

        // Found: connected ones need nothing; one that isn't signed in shows its sign-in command.
        trek.update(cx, |ws, cx| {
            ws.agents = vec![detected(AgentId::ClaudeCode, Availability::Ready), detected(AgentId::Codex, Availability::NeedsLogin), detected(AgentId::OpenCode, Availability::NotInstalled)];
            cx.notify();
        });
        trek.render(cx);
        assert!(trek.visible(cx, "ob-agent-claude-code") && !trek.visible(cx, "ob-copy-claude-code"), "connected");
        trek.click(cx, "ob-copy-codex");
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("codex login"));
        assert!(trek.visible(cx, "ob-copy-opencode"), "still to install");
    });
}

#[test]
fn the_tools_page_reads_the_mac_off_the_main_thread() {
    run(async |cx| {
        let trek = open(cx);
        trek.ws.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Tools), cx));
        // The first frame asks; it doesn't wait for the answer.
        let reading = trek.window(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find("ax-perm-checking").is_some_and(|e| e.visible()) && window.try_find("tools-reading").is_some()
        });
        assert!(reading, "a loading state until it's read");
        cx.run_until_parked();
        trek.render(cx);
        assert!(!trek.visible(cx, "ax-perm-checking"));
        assert!(trek.window(cx, |window, _| window.try_find("tools-reading").is_none() && window.try_find("tools-refresh").is_some()));
    });
}

#[test]
fn settings_skills_stay_inside_an_isolated_data_folder() {
    run(async |cx| {
        let trek = open(cx);
        let data = trek_core::paths::data_dir();
        let md = trek_core::skills::create("Isolated skill", "Only here.", trek_core::skills::SkillHome::ClaudeCode).expect("skill");
        assert!(md.starts_with(&data), "{}", md.display());
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Skills), cx));
        trek.render(cx);
        assert!(trek.visible(cx, "skill-on-Claude Code-0"));
        // Turned off from the page: moved aside inside the data folder, nowhere else.
        trek.click(cx, "skill-on-Claude Code-0");
        trek.render(cx);
        let skills = trek_core::skills::discover(None);
        let skill = skills.iter().find(|s| s.name == "isolated-skill").expect("still listed");
        assert!(!skill.enabled);
        assert!(skills.iter().filter(|s| s.source.editable()).all(|s| s.dir.starts_with(&data)), "{skills:?}");
    });
}

#[test]
fn a_thread_view_keeps_the_same_subscriptions_however_often_it_switches() {
    run(async |cx| {
        let trek = open(cx);
        let ids: Vec<String> = trek.update(cx, |ws, cx| {
            let ids = (0..2)
                .map(|_| {
                    let t = ws.store.create_thread(Some(&trek.project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
                    store_items(&ws.store, &t.id, transcript(2));
                    t.id
                })
                .collect();
            ws.reload(cx);
            ids
        });
        let count = |trek: &Trek, cx: &mut TestAppContext| trek.thread_view(cx).read_with(cx, |v, _| v.subscription_count());
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[0].clone()), cx));
        trek.render(cx);
        let before = count(&trek, cx);
        // Each switch parks one thread's view and restores the other's.
        for i in 0..12 {
            trek.update(cx, |ws, cx| ws.navigate(Route::Thread(ids[i % 2].clone()), cx));
            trek.render(cx);
        }
        assert_eq!(count(&trek, cx), before);
        // Scrolling still repaints the view on screen.
        let scroller = trek.thread_view(cx).read_with(cx, |v, _| v.scroller());
        let view = trek.thread_view(cx);
        let repainted = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = repainted.clone();
        let _watch = cx.update(|cx| cx.observe(&view, move |_, _| flag.set(true)));
        scroller.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(repainted.get());
    });
}

#[test]
fn opening_a_stored_thread_reads_its_history_in_the_background() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&ws.store, &t.id, transcript(4));
            ws.reload(cx);
            t.id
        });
        let stored = trek.read(cx, |ws, _| ws.store.items(&id).expect("items").len());
        // The click returns at once: the history isn't read on the main thread, and a message
        // sent meanwhile waits for it.
        trek.ws.update(cx, |ws, cx| {
            ws.navigate(Route::Thread(id.clone()), cx);
            let live = ws.live.get(&id).expect("live");
            assert!(live.loading && !live.loaded && live.items.is_empty());
            ws.send_to(&id, "And the tests?".into(), vec![], cx);
        });
        cx.run_until_parked();
        trek.render(cx);
        trek.read(cx, |ws, _| {
            let live = ws.live.get(&id).expect("live");
            assert!(live.loaded && !live.loading);
            assert!(live.items.len() > stored, "the history, then the message sent while it loaded");
        });
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        let asked = items.iter().position(|i| matches!(i, Item::User { text, .. } if text == "And the tests?")).expect("sent");
        assert!(asked >= stored, "after the history: {asked} < {stored}");
        // Saved in that order, so it reads back the same.
        assert_eq!(trek.read(cx, |ws, _| ws.store.items(&id).expect("items").len()), items.len());
    });
}
