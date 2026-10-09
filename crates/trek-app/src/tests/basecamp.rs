//! Basecamp: opening and leaving it, the empty range, what needs the user, the totals, the chart
//! (its bars per range) and the tables by project and model, and the token usage a turn records
//! for it.

use super::harness::{Trek, mock, new_project, open, run, store_items};
use crate::basecamp::{Step, Summary, Waiting, chart, tables};
use crate::workspace::Route;
use chrono::{FixedOffset, TimeZone as _};
use gpui_kit::TestAppContext;
use trek_core::basecamp::{Range, ThreadActivity};
use trek_core::settings::ThemeChoice;
use trek_core::store::{Activity, Item, Store, UsageRow, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState, TokenUsage};

/// A needs row's id.
fn row(thread: &str) -> gpui_kit::SharedString {
    format!("review-{thread}").into()
}

fn theme(cx: &mut TestAppContext, choice: ThemeChoice) {
    cx.update(|cx| crate::apply_theme(choice, None, cx));
}

/// Wait until Basecamp has a summary computed from what the workspace holds now.
fn summary(trek: &Trek, cx: &mut TestAppContext) -> Summary {
    let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
    for _ in 0..500 {
        cx.run_until_parked();
        if let Some(s) = basecamp.read_with(cx, |b, _| b.summary().cloned()) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("no summary");
}

fn recap(trek: &Trek, cx: &mut TestAppContext) -> trek_core::basecamp::Recap {
    summary(trek, cx).recap
}

/// A thread's part in a range, made up: `turns` (end times; `failed` ones stopped), tokens
/// reported per model.
fn activity(store: &Store, project: Option<(&str, &str)>, agent: AgentId, model: Option<&str>, turns: &[(i64, bool)], usage: &[(i64, Option<&str>, u64)]) -> ThreadActivity {
    let mut thread = store.create_thread(None, agent.clone(), model.map(String::from), Effort::Medium, HandHolding::Auto).unwrap();
    thread.project_id = project.map(|(id, _)| id.to_string());
    let activity = turns
        .iter()
        .flat_map(|(at, failed)| {
            let end = if *failed { Activity::TurnStopped { at: *at, took_secs: 10, failed: true } } else { Activity::TurnEnd { at: *at, took_secs: 10 } };
            [Activity::Prompt { at: at - 10_000 }, end]
        })
        .collect();
    let usage = usage
        .iter()
        .map(|(at, model, n)| UsageRow { thread_id: thread.id.clone(), at: *at, agent: agent.clone(), model: model.map(String::from), tokens: TokenUsage { input: *n, ..Default::default() }, cost: None })
        .collect();
    ThreadActivity { thread, project: project.map(|(_, name)| name.to_string()), activity, usage }
}

#[test]
fn a_fresh_install_is_invited_to_start_and_esc_goes_back() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-basecamp");
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Basecamp);
        assert!(recap(&trek, cx).is_empty());
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            assert!(trek.visible(cx, "basecamp-empty"), "no activity yet, and a way to start");
            assert!(trek.visible(cx, "basecamp-needs"), "what needs you is always there");
            assert!(!trek.visible(cx, "basecamp-totals") && !trek.visible(cx, "basecamp-chart"));
        }
        trek.click(cx, "basecamp-new-thread");
        let draft = trek.read(cx, |ws, _| ws.route.clone());
        assert_eq!(draft, Route::Draft { project: None }, "a new thread starts in no project");
        // ⌘⇧H opens it with the sidebar folded away too; Esc goes back where it was opened from.
        trek.update(cx, |ws, _| ws.sidebar_collapsed = true);
        trek.press(cx, "cmd-shift-h");
        trek.render(cx);
        assert!(trek.visible(cx, "basecamp"));
        assert!(!trek.visible(cx, "open-basecamp"), "the sidebar stays folded");
        trek.press(cx, "escape");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), draft);
    });
}

#[test]
fn a_day_of_work_is_counted_and_reviewed() {
    run(async |cx| {
        let trek = open(cx);
        // Kept inside today, even a minute after midnight.
        let today = Range::Today.window(&chrono::Local::now()).start;
        let now = now_ms().max(today + 61_000);
        let (done, failed, quiet) = trek.update(cx, |ws, cx| {
            let mut ids = vec![];
            for (title, model) in [("Ship the parser", "mock-deep"), ("Fix the flaky test", "mock-swift"), ("Read the docs", "mock-swift")] {
                let t = ws.store.create_thread(Some(&trek.project), mock(), Some(model.into()), Effort::Medium, HandHolding::Auto).unwrap();
                let items = vec![
                    Item::User { text: title.into(), images: vec![], at: Some(now - 60_000), resume: None, aside: false },
                    Item::Assistant { text: "Done.".into() },
                    Item::TurnEnd { at: now - 1_000, took_secs: 59 },
                ];
                store_items(&ws.store, &t.id, items);
                let tokens = TokenUsage { input: 1_000, output: 400, cache_read: if model == "mock-deep" { 30_000 } else { 5_000 }, cache_write: 0 };
                ws.store.record_usage(&t.id, now - 1_000, &mock(), Some(model), &tokens, None).unwrap();
                ids.push(t.id);
            }
            ws.store
                .update_thread(&ids[0], |t| {
                    t.last_seen_at = t.updated_at - 1;
                    t.additions = 372;
                    t.deletions = 15;
                })
                .unwrap();
            ws.store.update_thread(&ids[1], |t| t.run_state = RunState::Failed).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Basecamp, cx);
            (ids[0].clone(), ids[1].clone(), ids[2].clone())
        });
        let s = summary(&trek, cx);
        let r = &s.recap;
        assert_eq!((r.prompts, r.threads, r.turns, r.failed), (3, 3, 3, 1));
        assert_eq!(r.tokens.total(), 3 * 1_400 + 40_000);
        // One project, two models, the deep one first (most tokens); the failure on swift.
        assert_eq!(s.projects.len(), 1);
        assert_eq!((s.projects[0].threads, s.projects[0].turns, s.projects[0].tokens, s.projects[0].failed), (3, 3, r.tokens.total(), 1));
        let models: Vec<(&str, usize, usize, u64, usize)> = s.models.iter().map(|m| (m.label.as_str(), m.threads, m.turns, m.tokens, m.failed)).collect();
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!((models[0].1, models[0].2, models[0].3, models[0].4), (1, 1, 31_400, 0));
        assert_eq!((models[1].1, models[1].2, models[1].3, models[1].4), (2, 2, 2 * 6_400, 1));
        // The mock's models have no price: no cost.
        assert!(!s.projects[0].spend.priced());
        // Today, by the hour: the turns sit in the hour they ended.
        assert_eq!((s.chart.step, s.chart.bars.len()), (Step::Hour, 24));
        assert_eq!(s.chart.bars.iter().map(|b| b.turns).sum::<usize>(), 3);
        let hour = chrono::Timelike::hour(&chrono::DateTime::from_timestamp_millis(now - 1_000).unwrap().with_timezone(&chrono::Local)) as usize;
        assert_eq!(s.chart.bars[hour].turns, 3);
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            for id in ["basecamp-totals", "basecamp-chart"] {
                assert!(trek.visible(cx, id), "{id} in {choice:?}");
            }
            assert!(trek.visible(cx, ("basecamp-project", 0usize)) && trek.visible(cx, ("basecamp-model", 1usize)));
            // The failure needs the user, the unread thread waits for review; the read one isn't listed.
            assert!(trek.visible(cx, format!("review-failed-{failed}")));
            assert!(trek.visible(cx, format!("review-done-{done}")));
            assert!(!trek.visible(cx, row(&quiet)));
        }
        // Hovering a bar spells it out.
        let bar = trek.bounds(cx, ("basecamp-bar", hour)).expect("the bar was drawn");
        trek.window(cx, |window, cx| {
            let at = bar.center();
            window.dispatch_event(gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx)
        });
        cx.run_until_parked();
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        assert_eq!(basecamp.read_with(cx, |b, _| b.hovered_bar()), Some(hour));
        let order: Vec<String> = trek.read(cx, |ws, _| ws.ready_for_review().iter().map(|t| t.id.clone()).collect());
        assert_eq!(order, [failed.clone(), done.clone()], "what needs you comes first");
        // Mark all read: the finished one is reviewed; the failure still needs the user.
        trek.click(cx, "basecamp-mark-read");
        trek.render(cx);
        assert!(!trek.visible(cx, row(&done)));
        assert!(trek.visible(cx, row(&failed)));
        // A row opens its thread.
        trek.click(cx, row(&failed));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(failed));
    });
}

#[test]
fn the_tables_add_up_by_project_and_by_model() {
    let store = Store::in_memory().unwrap();
    let window = Range::Today.window(&chrono::Local::now());
    let at = window.start + 3_600_000;
    let before = window.start - 1_000;
    let claude = AgentId::ClaudeCode;
    let mut threads = vec![
        // Trek: two turns on Opus, one failed; a report from yesterday stays out.
        activity(&store, Some(("p1", "Trek")), claude.clone(), Some("claude-opus-5-5"), &[(at, false), (at + 60_000, true), (before, false)], &[(at, None, 1_000), (before, None, 50)]),
        // Trek again: left on the agent's default, its reports name Sonnet (a dated snapshot too).
        activity(&store, Some(("p1", "Trek")), claude.clone(), None, &[(at, false)], &[(at, Some("claude-sonnet-5")), (at + 1, Some("claude-sonnet-5-20260101"))].map(|(a, m)| (a, m, 500))),
        // Website, on Codex.
        activity(&store, Some(("p2", "Website")), AgentId::Codex, Some("gpt-5.6-luna"), &[(at, false)], &[(at, None, 4_000)]),
        // No project.
        activity(&store, None, claude.clone(), Some("claude-opus-5-5"), &[(at, false)], &[(at, None, 10)]),
    ];
    // A sub-agent of the first: its work counts, not as a thread of the user's.
    let mut sub = activity(&store, Some(("p1", "Trek")), claude.clone(), Some("claude-opus-5-5"), &[(at, false)], &[(at, None, 200)]);
    sub.thread.parent_id = Some(threads[0].thread.id.clone());
    threads.push(sub);
    // Failed earlier with no record of its turns: one failure, nothing else.
    let mut failed = activity(&store, Some(("p2", "Website")), AgentId::Codex, Some("gpt-5.6-luna"), &[], &[]);
    failed.thread.run_state = RunState::Failed;
    failed.thread.updated_at = at;
    threads.push(failed);

    let recap = trek_core::basecamp::Recap::compute(window, at + 120_000, &threads);
    let (projects, models) = tables(&window, &threads);
    let by: Vec<(&str, usize, usize, u64, usize)> = projects.iter().map(|r| (r.label.as_str(), r.threads, r.turns, r.tokens, r.failed)).collect();
    assert_eq!(by, [("Website", 1, 1, 4_000, 1), ("Trek", 2, 4, 2_200, 1), ("No project", 1, 1, 10, 0)]);
    assert_eq!(projects[0].project.as_deref(), Some("p2"));
    assert_eq!(projects[2].project, None);
    // Every column adds up to the range's totals.
    let sum = |rows: &[crate::basecamp::Row], f: fn(&crate::basecamp::Row) -> u64| rows.iter().map(f).sum::<u64>();
    for rows in [&projects, &models] {
        assert_eq!(sum(rows, |r| r.turns as u64), recap.turns as u64);
        assert_eq!(sum(rows, |r| r.tokens), recap.tokens.total());
        assert_eq!(sum(rows, |r| r.failed as u64), recap.failed as u64);
    }
    // Cost by project and by model comes to the same, reports that name no model priced as
    // their thread's (which the recap's own total can't).
    let usd = |rows: &[crate::basecamp::Row]| rows.iter().map(|r| r.spend.usd()).sum::<f64>();
    assert!((usd(&projects) - usd(&models)).abs() < 1e-9 && usd(&models) > recap.spend.usd());
    assert_eq!(sum(&projects, |r| r.threads as u64), recap.threads as u64);
    // By model: Opus (three threads' turns, the sub-agent's among them), Sonnet (the snapshot
    // counted as its model), Luna.
    let by: Vec<(&str, Option<&AgentId>, usize, usize, u64, usize)> = models.iter().map(|r| (r.label.as_str(), r.agent.as_ref(), r.threads, r.turns, r.tokens, r.failed)).collect();
    assert_eq!(
        by,
        [
            ("GPT-5.6 Luna", Some(&AgentId::Codex), 1, 1, 4_000, 1),
            ("Claude Opus 5.5", Some(&claude), 2, 4, 1_210, 1),
            ("Claude Sonnet 5", Some(&claude), 1, 1, 1_000, 0),
        ]
    );
    // Each priced at API prices.
    assert!(models.iter().all(|m| m.spend.priced() && m.spend.unpriced() == 0 && m.spend.usd() > 0.), "{models:?}");
}

#[test]
fn the_chart_is_drawn_in_hours_days_or_weeks_by_range() {
    let store = Store::in_memory().unwrap();
    let tz = FixedOffset::east_opt(2 * 3_600).unwrap();
    // Thursday 8 October 2026, 3:30 PM.
    let now = tz.with_ymd_and_hms(2026, 10, 8, 15, 30, 0).unwrap();
    let ms = |d: i64, h: u32| (tz.with_ymd_and_hms(2026, 10, 8, h, 10, 0).unwrap() - chrono::Duration::days(d)).timestamp_millis();
    let threads = [activity(&store, None, mock(), None, &[(ms(0, 14), false), (ms(0, 14), false), (ms(2, 9), false), (ms(41, 9), false)], &[(ms(0, 14), None, 700), (ms(2, 9), None, 300)])];
    let turns = |c: &crate::basecamp::Chart| c.bars.iter().map(|b| b.turns).collect::<Vec<_>>();
    let ticks = |c: &crate::basecamp::Chart| c.ticks.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(" ");

    // Today: 24 hours, the turns in the 2 PM bar.
    let c = chart(Range::Today, &now, None, &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Hour, 24));
    assert_eq!((c.bars[14].turns, c.bars[14].tokens, turns(&c).iter().sum::<usize>()), (2, 700, 2));
    assert_eq!(c.bars[14].label, "2 PM–3 PM");
    assert_eq!(ticks(&c), "12 AM 6 AM Noon 6 PM");

    // This week: Monday to Sunday, a day a bar.
    let c = chart(Range::Week, &now, None, &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 7));
    assert_eq!(turns(&c), [0, 1, 0, 2, 0, 0, 0]);
    assert_eq!(ticks(&c), "Mon Tue Wed Thu Fri Sat Sun");
    assert_eq!(c.bars[3].label, "Thu, Oct 8");

    // All time, with only today's work: still a week of days, never a single day.
    let today_only = [activity(&store, None, mock(), None, &[(ms(0, 14), false)], &[])];
    let c = chart(Range::All, &now, Some(ms(0, 14)), &today_only);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 7));
    assert_eq!(turns(&c), [0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(ticks(&c), "Oct 2 Oct 3 Oct 4 Oct 5 Oct 6 Oct 7 Oct 8");
    let c = chart(Range::All, &now, None, &[]);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 7), "nothing at all: the last week");

    // All time over six weeks: a day a bar from the first day, dates spread under them.
    let c = chart(Range::All, &now, Some(ms(41, 9)), &threads);
    assert_eq!((c.step, c.bars.len()), (Step::Day, 42));
    assert_eq!((c.bars[0].turns, c.bars[39].turns, c.bars[41].turns), (1, 1, 2));
    assert_eq!(c.ticks.len(), 6);
    assert_eq!((c.ticks[0].0, c.ticks[5].0), (0, 41));
    assert_eq!(c.ticks[0].1, "Aug 28");
    assert!(c.ticks.windows(2).all(|w| w[0].0 < w[1].0));

    // A year and more: a week a bar, from the first Monday; years: several weeks a bar.
    let c = chart(Range::All, &now, Some(ms(400, 9)), &threads);
    assert_eq!(c.step, Step::Weeks(1));
    assert_eq!(c.bars.len(), 58);
    assert!(c.bars[0].label.starts_with("Week of ") && c.bars[0].label.ends_with(", 2025"), "{}", c.bars[0].label);
    assert_eq!(turns(&c).iter().sum::<usize>(), 4);
    assert_eq!(c.bars.last().unwrap().turns, 3, "this week's: Tuesday's and today's");
    let c = chart(Range::All, &now, Some(ms(3 * 365, 9)), &threads);
    assert_eq!(c.step, Step::Weeks(2));
    assert!(c.bars.len() <= 104 && c.bars[0].label.contains(" – "));
}

#[test]
fn a_turn_records_the_tokens_its_agent_reported() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "explain the project");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let rows = trek.read(cx, |ws, _| ws.store.usage_between(0, i64::MAX).unwrap());
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!((rows[0].thread_id.as_str(), &rows[0].agent, rows[0].model.as_deref()), (id.as_str(), &mock(), Some("mock-swift")));
        assert!(rows[0].tokens.output > 0);
        // Basecamp counts it.
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let s = summary(&trek, cx);
        assert_eq!((s.recap.prompts, s.recap.turns, s.recap.tokens.total()), (1, 1, rows[0].tokens.total()));
        assert_eq!(s.models.first().and_then(|m| m.agent.clone()), Some(AgentId::Direct("mock".into())));
        assert_eq!(s.models.first().map(|m| (m.turns, m.tokens)), Some((1, rows[0].tokens.total())));
    });
}

#[test]
fn the_week_the_palette_and_the_clock() {
    run(async |cx| {
        let trek = open(cx);
        // ⌘K finds it.
        trek.press(cx, "cmd-k");
        trek.type_text(cx, "Basecamp");
        trek.press(cx, "enter");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Basecamp);
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), Some("mock-swift".into()), Effort::Medium, HandHolding::Auto).unwrap();
            let at = now_ms() - 1_000;
            store_items(&ws.store, &t.id, vec![Item::User { text: "go".into(), images: vec![], at: Some(at), resume: None, aside: false }, Item::Assistant { text: "ok".into() }, Item::TurnEnd { at, took_secs: 1 }]);
            ws.reload(cx);
            t.id
        });
        assert!(!recap(&trek, cx).is_empty(), "{id} is today's");
        // This week: the same work, a day a bar from Monday.
        trek.click(cx, ("basecamp-range", 1usize));
        let week = summary(&trek, cx);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        assert_eq!(basecamp.read_with(cx, |b, _| b.range()), Range::Week);
        assert_eq!(week.recap.window.range, Range::Week);
        assert_eq!((week.chart.step, week.chart.bars.len()), (Step::Day, 7));
        assert_eq!((week.recap.prompts, week.recap.turns), (1, 1));
        trek.render(cx);
        assert!(trek.visible(cx, "basecamp-totals") && trek.visible(cx, "basecamp-chart"));
        // A minute on, with nothing changed: the summary stays, its "now" moves.
        let later = week.recap.now + 60_000;
        basecamp.update(cx, |b, cx| b.tick(later, cx));
        assert_eq!(recap(&trek, cx).now, later);
    });
}

#[test]
fn all_time_reaches_back_to_the_first_thing_done() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        recap(&trek, cx);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        // Nothing yet.
        trek.click(cx, ("basecamp-range", 2usize));
        let empty = summary(&trek, cx);
        assert_eq!(basecamp.read_with(cx, |b, _| b.range()), Range::All);
        assert!(empty.recap.is_empty() && empty.recap.first.is_none());
        trek.render(cx);
        assert!(trek.visible(cx, "basecamp-empty"));
        // Work today (even a minute after midnight), and a thread from six weeks ago in another
        // project.
        let now = now_ms().max(Range::Today.window(&chrono::Local::now()).start + 61_000);
        let long_ago = now - 42 * 24 * 3_600_000;
        let website = new_project("website");
        trek.update(cx, |ws, cx| {
            for (at, model, project) in [(now - 60_000, "mock-swift", &trek.project), (long_ago, "mock-deep", &website)] {
                let t = ws.store.create_thread(Some(project), mock(), Some(model.into()), Effort::Medium, HandHolding::Auto).unwrap();
                let items = vec![
                    Item::User { text: "go".into(), images: vec![], at: Some(at), resume: None, aside: false },
                    Item::Assistant { text: "ok".into() },
                    Item::TurnEnd { at: at + 30_000, took_secs: 30 },
                ];
                store_items(&ws.store, &t.id, items);
                ws.store.record_usage(&t.id, at + 30_000, &mock(), Some(model), &TokenUsage { input: 1_000, output: 500, cache_read: 0, cache_write: 0 }, None).unwrap();
            }
            ws.reload(cx);
        });
        let all = summary(&trek, cx);
        let r = &all.recap;
        assert_eq!((r.prompts, r.threads, r.turns, r.agent_secs, r.tokens.total()), (2, 2, 2, 60, 3_000));
        assert_eq!(r.first, Some(long_ago));
        // Six weeks, a day a bar from the first day; both projects in the table.
        assert_eq!((all.chart.step, all.chart.bars.len()), (Step::Day, 43));
        assert_eq!((all.chart.bars[0].turns, all.chart.bars[42].turns), (1, 1));
        assert_eq!(all.projects.len(), 2);
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            assert!(trek.visible(cx, "basecamp-totals") && trek.visible(cx, "basecamp-chart"), "{choice:?}");
            assert!(trek.visible(cx, ("basecamp-project", 1usize)));
        }
        // Today's leaves the old thread out; ⌘K on Basecamp switches the range.
        trek.press(cx, "cmd-k");
        trek.type_text(cx, "Basecamp: Today");
        trek.press(cx, "enter");
        let today = recap(&trek, cx);
        assert_eq!(basecamp.read_with(cx, |b, _| b.range()), Range::Today);
        assert_eq!((today.prompts, today.threads), (1, 1));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Basecamp);
    });
}

#[test]
fn needs_you_says_what_each_thread_waits_for_and_failed_turns_count() {
    run(async |cx| {
        let trek = open(cx);
        let mut asked = vec![];
        for (prompt, waiting) in [("mock:questions", Waiting::Question), ("mock:permission", Waiting::Approval)] {
            trek.update(cx, |ws, cx| ws.new_thread(cx));
            let id = trek.send(cx, prompt);
            trek.wait_needs_you(cx, &id).await;
            asked.push((id, waiting));
        }
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        let failed = trek.send(cx, "mock:error");
        trek.wait_done(cx, &failed, RunState::Failed).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        let s = summary(&trek, cx);
        assert_eq!(s.recap.failed, 1, "the failed turn, recorded");
        assert_eq!(s.models.iter().map(|m| m.failed).sum::<usize>(), 1);
        assert!(s.recap.agent_secs <= s.recap.turns as u64 * 60);
        trek.render(cx);
        // Each in "Needs you", saying what it waits for.
        for (id, waiting) in &asked {
            assert!(trek.visible(cx, format!("review-needs-{id}")));
            assert_eq!(trek.read(cx, |ws, _| Waiting::of(ws.pending_request(id))), *waiting);
        }
        assert!(trek.visible(cx, format!("review-failed-{failed}")));
        assert_eq!(Waiting::of(None).label(), "Needs you");
        // Mark all read leaves what isn't listed alone: a settled thread stays unread.
        let settled = asked[0].0.clone();
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&settled, |t| {
                    t.run_state = RunState::Idle;
                    t.settled_at = Some(now_ms());
                    t.last_seen_at = 0;
                })
                .unwrap();
            ws.reload(cx);
        });
        assert!(!trek.read(cx, |ws, _| ws.ready_for_review().iter().any(|t| t.id == settled)));
        trek.update(cx, |ws, cx| ws.mark_all_read(cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&settled).unwrap().is_unseen()));
    });
}

#[test]
fn a_long_review_list_folds_after_a_few() {
    run(async |cx| {
        let trek = open(cx);
        let ids = trek.update(cx, |ws, cx| {
            let ids: Vec<String> = (0..11)
                .map(|i| {
                    let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).unwrap();
                    ws.store
                        .update_thread(&t.id, |t| {
                            t.updated_at += i;
                            t.last_seen_at = 0;
                        })
                        .unwrap();
                    t.id
                })
                .collect();
            ws.reload(cx);
            ws.navigate(Route::Basecamp, cx);
            ids
        });
        summary(&trek, cx);
        trek.render(cx);
        let shown = |trek: &Trek, cx: &mut TestAppContext| ids.iter().filter(|id| trek.visible(cx, row(id))).count();
        assert_eq!(shown(&trek, cx), 8);
        trek.click(cx, "basecamp-review-more");
        trek.render(cx);
        assert_eq!(shown(&trek, cx), 11);
    });
}

#[test]
fn a_thread_paused_at_its_limit_waits_for_review_without_failing() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:limit 5s");
        let thread = id.clone();
        trek.wait(cx, "the limit to pause the thread", |ws| ws.pause(&thread).is_some() && !ws.turn_running(&thread)).await;
        trek.update(cx, |ws, cx| {
            // Another thread resumed at its reset: the "Continue" it sent on its own isn't one of
            // the user's prompts, though its turn counts.
            let resumed = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).unwrap();
            let at = now_ms() - 1_000;
            let continued = Item::User { text: trek_core::limit::CONTINUE.into(), images: vec![], at: Some(at), resume: None, aside: false };
            store_items(&ws.store, &resumed.id, vec![continued, Item::Assistant { text: "Done.".into() }, Item::TurnEnd { at, took_secs: 1 }]);
            ws.store.update_thread(&id, |t| t.last_seen_at = 0).unwrap();
            ws.reload(cx);
            ws.navigate(Route::Basecamp, cx);
        });
        let r = recap(&trek, cx);
        assert_eq!((r.prompts, r.failed), (1, 0), "paused, not failed; one prompt of the user's");
        assert!(r.turns >= 1);
        trek.render(cx);
        assert!(trek.visible(cx, row(&id)));
        assert!(trek.visible(cx, format!("review-paused-{id}")));
    });
}

/// Two threads of the mock's in the store: one with a turn `ago` ms before now, in `project`.
fn worked(trek: &Trek, cx: &mut TestAppContext, ago: &[(i64, &std::path::PathBuf)]) {
    trek.update(cx, |ws, cx| {
        for (ago, project) in ago {
            let at = now_ms().max(Range::Today.window(&chrono::Local::now()).start + 61_000) - ago - 60_000;
            let t = ws.store.create_thread(Some(project), mock(), Some("mock-swift".into()), Effort::Medium, HandHolding::Auto).unwrap();
            let items = vec![Item::User { text: "go".into(), images: vec![], at: Some(at), resume: None, aside: false }, Item::Assistant { text: "ok".into() }, Item::TurnEnd { at: at + 30_000, took_secs: 30 }];
            store_items(&ws.store, &t.id, items);
            ws.store.record_usage(&t.id, at + 30_000, &mock(), Some("mock-swift"), &TokenUsage { input: 1_000, output: 500, cache_read: 0, cache_write: 0 }, None).unwrap();
        }
        ws.reload(cx);
    });
}

#[test]
fn every_range_is_worked_out_in_one_pass_and_switching_reads_nothing() {
    run(async |cx| {
        let trek = open(cx);
        let website = new_project("website");
        worked(&trek, cx, &[(0, &trek.project.clone()), (42 * 24 * 3_600_000, &website)]);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        assert_eq!(recap(&trek, cx).prompts, 1);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        let passes = basecamp.read_with(cx, |b, _| b.passes);
        for (i, range, prompts) in [(2usize, Range::All, 2), (1, Range::Week, 1), (0, Range::Today, 1), (2, Range::All, 2)] {
            trek.click(cx, ("basecamp-range", i));
            // Drawn at once from what the one pass worked out: nothing is read again.
            let s = basecamp.read_with(cx, |b, _| b.summary().cloned()).expect("up to date at once");
            assert_eq!((s.recap.window.range, s.recap.prompts), (range, prompts));
            assert_eq!(basecamp.read_with(cx, |b, _| b.passes), passes, "no pass for {range:?}");
            trek.render(cx);
            assert!(trek.visible(cx, "basecamp-totals"));
        }
        // A turn ends: one pass brings every range up to date, reading only that thread.
        worked(&trek, cx, &[(0, &trek.project.clone())]);
        let all = summary(&trek, cx);
        assert_eq!(all.recap.prompts, 3);
        assert_eq!(basecamp.read_with(cx, |b, _| b.passes), passes + 1);
        trek.click(cx, ("basecamp-range", 0usize));
        assert_eq!(basecamp.read_with(cx, |b, _| b.summary().map(|s| s.recap.prompts)), Some(2));
    });
}

#[test]
fn reopened_basecamp_shows_what_it_had_at_once_while_it_catches_up() {
    run(async |cx| {
        let trek = open(cx);
        worked(&trek, cx, &[(0, &trek.project.clone())]);
        trek.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        assert_eq!(recap(&trek, cx).prompts, 1);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        // More work while it's closed: nothing is read until it opens.
        worked(&trek, cx, &[(0, &trek.project.clone())]);
        let basecamp = cx.read(|cx| trek.root.read(cx).basecamp.clone());
        let passes = basecamp.read_with(cx, |b, _| b.passes);
        trek.ws.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        // At once: the numbers it had, while the new ones are worked out off the main thread.
        let (shown, current) = basecamp.read_with(cx, |b, _| (b.shown_summary().map(|s| s.recap.prompts), b.summary().is_some()));
        assert_eq!((shown, current), (Some(1), false));
        trek.window(cx, |window, cx| gpui_kit::test::TestWindowExt::render_frame(window, cx));
        assert!(trek.visible(cx, "basecamp-totals"), "drawn before the pass is done");
        assert_eq!(recap(&trek, cx).prompts, 2);
        assert_eq!(basecamp.read_with(cx, |b, _| b.passes), passes + 1);
    });
}
